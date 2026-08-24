use crate::{
    ApplicationCore, CoreCommand, CoreCommandOutcome, CoreSnapshot, SemanticEvent,
    SemanticEventKind, TerminalChange, TerminalSessionId, TerminalSize, TerminalSnapshot,
    terminal_theme::TerminalTheme,
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

const COMMAND_CAPACITY: usize = 256;
const MAX_HIERARCHY_REFRESH_ATTEMPTS: usize = 8;

pub(crate) struct CoreDriver {
    commands: flume::Sender<Command>,
    updates: DriverUpdates,
    counters: Arc<DriverCounters>,
    next_command_id: AtomicU64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CoreProjection {
    pub hierarchy: CoreSnapshot,
    pub terminals: HashMap<TerminalSessionId, TerminalSnapshot>,
}

pub(crate) enum DriverUpdate {
    Hierarchy(CoreSnapshot),
    CommandAccepted {
        command_id: u64,
        outcome: CoreCommandOutcome,
    },
    CommandRejected {
        command_id: u64,
        error: String,
    },
    Terminal {
        terminal_session_id: TerminalSessionId,
        snapshot: TerminalSnapshot,
    },
    Error(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct DriverStats {
    pushed_changes: u64,
    snapshot_requests: u64,
    snapshots_published: u64,
}

#[derive(Default)]
struct DriverCounters {
    pushed_changes: AtomicU64,
    snapshot_requests: AtomicU64,
    snapshots_published: AtomicU64,
}

#[derive(Clone)]
pub(crate) struct DriverUpdates {
    state: Arc<Mutex<DriverUpdateState>>,
    wakes: flume::Receiver<()>,
    wake: flume::Sender<()>,
}

#[derive(Default)]
struct DriverUpdateState {
    hierarchy: Option<CoreSnapshot>,
    command_results: VecDeque<DriverCommandResult>,
    terminals: HashMap<TerminalSessionId, TerminalSnapshot>,
    error: Option<String>,
    stopped: bool,
}

struct DriverUpdatePublisher {
    state: Arc<Mutex<DriverUpdateState>>,
    wake: flume::Sender<()>,
}

enum DriverCommandResult {
    Accepted {
        command_id: u64,
        outcome: CoreCommandOutcome,
    },
    Rejected {
        command_id: u64,
        error: String,
    },
}

impl DriverUpdates {
    pub(crate) async fn next(&self) -> Option<DriverUpdate> {
        loop {
            if let Some(update) = self.take_pending() {
                return Some(update);
            }
            if self.is_stopped() {
                return None;
            }
            if self.wakes.recv_async().await.is_err() {
                return None;
            }
        }
    }

    fn take_pending(&self) -> Option<DriverUpdate> {
        let mut state = self
            .state
            .lock()
            .expect("window driver update mutex poisoned");
        let update = if let Some(hierarchy) = state.hierarchy.take() {
            Some(DriverUpdate::Hierarchy(hierarchy))
        } else if let Some(result) = state.command_results.pop_front() {
            Some(match result {
                DriverCommandResult::Accepted {
                    command_id,
                    outcome,
                } => DriverUpdate::CommandAccepted {
                    command_id,
                    outcome,
                },
                DriverCommandResult::Rejected { command_id, error } => {
                    DriverUpdate::CommandRejected { command_id, error }
                }
            })
        } else if let Some(error) = state.error.take() {
            Some(DriverUpdate::Error(error))
        } else {
            let terminal_session_id = state.terminals.keys().copied().min()?;
            let snapshot = state
                .terminals
                .remove(&terminal_session_id)
                .expect("selected pending Terminal Session exists");
            Some(DriverUpdate::Terminal {
                terminal_session_id,
                snapshot,
            })
        };
        if state.has_pending() {
            let _ = self.wake.try_send(());
        }
        update
    }

    fn is_stopped(&self) -> bool {
        self.state
            .lock()
            .expect("window driver update mutex poisoned")
            .stopped
    }
}

impl DriverUpdateState {
    fn has_pending(&self) -> bool {
        self.hierarchy.is_some()
            || !self.command_results.is_empty()
            || !self.terminals.is_empty()
            || self.error.is_some()
    }
}

impl DriverUpdatePublisher {
    fn publish(&self, update: DriverUpdate) {
        let mut state = self
            .state
            .lock()
            .expect("window driver update mutex poisoned");
        match update {
            DriverUpdate::Hierarchy(hierarchy) => state.hierarchy = Some(hierarchy),
            DriverUpdate::CommandAccepted {
                command_id,
                outcome,
            } => state
                .command_results
                .push_back(DriverCommandResult::Accepted {
                    command_id,
                    outcome,
                }),
            DriverUpdate::CommandRejected { command_id, error } => state
                .command_results
                .push_back(DriverCommandResult::Rejected { command_id, error }),
            DriverUpdate::Terminal {
                terminal_session_id,
                snapshot,
            } => {
                state.terminals.insert(terminal_session_id, snapshot);
            }
            DriverUpdate::Error(error) => state.error = Some(error),
        }
        drop(state);
        let _ = self.wake.try_send(());
    }
}

impl Drop for DriverUpdatePublisher {
    fn drop(&mut self) {
        self.state
            .lock()
            .expect("window driver update mutex poisoned")
            .stopped = true;
        let _ = self.wake.try_send(());
    }
}

enum Command {
    Apply {
        command_id: u64,
        command: CoreCommand,
    },
    Input {
        terminal_session_id: TerminalSessionId,
        bytes: Vec<u8>,
    },
    Paste {
        terminal_session_id: TerminalSessionId,
        bytes: Vec<u8>,
    },
    Resize {
        terminal_session_id: TerminalSessionId,
        size: TerminalSize,
    },
    SetTerminalTheme {
        theme: TerminalTheme,
    },
}

enum DriverEvent {
    Command(Result<Command, flume::RecvError>),
    TerminalChanged(Result<TerminalChange, flume::RecvError>),
    Semantic(Result<SemanticEvent, flume::RecvError>),
}

struct DriverReceivers {
    commands: flume::Receiver<Command>,
    terminal_changes: flume::Receiver<TerminalChange>,
    semantic_events: flume::Receiver<SemanticEvent>,
}

impl CoreDriver {
    pub(crate) fn start(core: ApplicationCore) -> Result<(Self, CoreProjection), String> {
        let terminal_changes = core.terminal_changes();
        let semantic_events = core.semantic_events();
        let projection = load_projection(&core)?;
        let revisions = projection
            .terminals
            .iter()
            .map(|(&terminal_session_id, snapshot)| (terminal_session_id, snapshot.revision))
            .collect();
        let hierarchy_revision = projection.hierarchy.revision;
        let (commands_tx, commands_rx) = flume::bounded(COMMAND_CAPACITY);
        let (wake_tx, wake_rx) = flume::bounded(1);
        let state = Arc::new(Mutex::new(DriverUpdateState::default()));
        let updates = DriverUpdates {
            state: Arc::clone(&state),
            wakes: wake_rx,
            wake: wake_tx.clone(),
        };
        let publisher = DriverUpdatePublisher {
            state,
            wake: wake_tx,
        };
        let counters = Arc::new(DriverCounters::default());
        let worker_counters = Arc::clone(&counters);
        let receivers = DriverReceivers {
            commands: commands_rx,
            terminal_changes,
            semantic_events,
        };
        std::thread::Builder::new()
            .name("window-driver".into())
            .spawn(move || {
                run_driver(
                    core,
                    hierarchy_revision,
                    revisions,
                    receivers,
                    publisher,
                    worker_counters,
                )
            })
            .map_err(|error| format!("spawn window driver: {error}"))?;
        Ok((
            Self {
                commands: commands_tx,
                updates,
                counters,
                next_command_id: AtomicU64::new(1),
            },
            projection,
        ))
    }

    pub(crate) fn input_to(
        &self,
        terminal_session_id: TerminalSessionId,
        bytes: Vec<u8>,
    ) -> Result<(), String> {
        self.send(Command::Input {
            terminal_session_id,
            bytes,
        })
    }

    pub(crate) fn paste_to(
        &self,
        terminal_session_id: TerminalSessionId,
        bytes: Vec<u8>,
    ) -> Result<(), String> {
        self.send(Command::Paste {
            terminal_session_id,
            bytes,
        })
    }

    pub(crate) fn apply_core_command(&self, command: CoreCommand) -> Result<u64, String> {
        let command_id = self.next_command_id.fetch_add(1, Ordering::Relaxed);
        self.send(Command::Apply {
            command_id,
            command,
        })?;
        Ok(command_id)
    }

    pub(crate) fn resize_terminal(
        &self,
        terminal_session_id: TerminalSessionId,
        size: TerminalSize,
    ) -> Result<(), String> {
        self.send(Command::Resize {
            terminal_session_id,
            size,
        })
    }

    pub(crate) fn set_terminal_theme(&self, theme: TerminalTheme) -> Result<(), String> {
        self.send(Command::SetTerminalTheme { theme })
    }

    pub(crate) fn updates(&self) -> DriverUpdates {
        self.updates.clone()
    }

    fn stats(&self) -> DriverStats {
        DriverStats {
            pushed_changes: self.counters.pushed_changes.load(Ordering::Relaxed),
            snapshot_requests: self.counters.snapshot_requests.load(Ordering::Relaxed),
            snapshots_published: self.counters.snapshots_published.load(Ordering::Relaxed),
        }
    }

    fn send(&self, command: Command) -> Result<(), String> {
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                flume::TrySendError::Full(_) => "window driver command queue is busy".into(),
                flume::TrySendError::Disconnected(_) => "window driver stopped".into(),
            })
    }
}

impl Drop for CoreDriver {
    fn drop(&mut self) {
        if std::env::var_os("AGENT_TERMINAL_DRIVER_STATS").is_some() {
            let stats = self.stats();
            eprintln!(
                "window driver: {} pushed changes, {} snapshot requests, {} snapshots published",
                stats.pushed_changes, stats.snapshot_requests, stats.snapshots_published
            );
        }
    }
}

fn run_driver(
    core: ApplicationCore,
    mut hierarchy_revision: u64,
    mut revisions: HashMap<TerminalSessionId, u64>,
    receivers: DriverReceivers,
    updates: DriverUpdatePublisher,
    counters: Arc<DriverCounters>,
) {
    loop {
        let event = flume::Selector::new()
            .recv(&receivers.commands, DriverEvent::Command)
            .recv(&receivers.terminal_changes, DriverEvent::TerminalChanged)
            .recv(&receivers.semantic_events, DriverEvent::Semantic)
            .wait();
        let result = match event {
            DriverEvent::Command(Err(_)) => break,
            DriverEvent::Command(Ok(Command::Apply {
                command_id,
                command,
            })) => match core.apply_core_command(command) {
                Ok(outcome) => synchronize_hierarchy(
                    &core,
                    outcome.snapshot.clone(),
                    &mut hierarchy_revision,
                    &mut revisions,
                    &counters,
                )
                .map(|mut pending| {
                    pending.push(DriverUpdate::CommandAccepted {
                        command_id,
                        outcome,
                    });
                    pending
                }),
                Err(error) => Ok(vec![DriverUpdate::CommandRejected { command_id, error }]),
            },
            DriverEvent::Command(Ok(Command::Input {
                terminal_session_id,
                bytes,
            })) => core
                .input_to(terminal_session_id, &bytes)
                .map(|()| Vec::new())
                .map_err(|error| error.to_string()),
            DriverEvent::Command(Ok(Command::Paste {
                terminal_session_id,
                bytes,
            })) => core
                .paste_to(terminal_session_id, &bytes)
                .map(|()| Vec::new())
                .map_err(|error| error.to_string()),
            DriverEvent::Command(Ok(Command::Resize {
                terminal_session_id,
                size,
            })) => core
                .resize_terminal(terminal_session_id, size)
                .map(|()| Vec::new())
                .map_err(|error| error.to_string()),
            DriverEvent::Command(Ok(Command::SetTerminalTheme { theme })) => core
                .set_terminal_theme(theme)
                .map(|()| Vec::new())
                .map_err(|error| error.to_string()),
            DriverEvent::TerminalChanged(Ok(change)) => {
                counters.pushed_changes.fetch_add(1, Ordering::Relaxed);
                refresh_terminal(
                    &core,
                    change.terminal_session_id,
                    change.terminal_revision,
                    &mut revisions,
                    &counters,
                )
                .map(|update| update.into_iter().collect())
            }
            DriverEvent::TerminalChanged(Err(_)) => {
                Err("terminal runtime stopped while waiting for a change".into())
            }
            DriverEvent::Semantic(Ok(event)) => match event.kind {
                SemanticEventKind::TerminalLifecycleChanged {
                    terminal_session_id,
                    terminal_revision,
                    ..
                } => refresh_terminal(
                    &core,
                    terminal_session_id,
                    terminal_revision,
                    &mut revisions,
                    &counters,
                )
                .map(|update| update.into_iter().collect()),
                SemanticEventKind::HierarchyChanged { revision }
                    if revision > hierarchy_revision =>
                {
                    core.refresh_core_snapshot().and_then(|snapshot| {
                        synchronize_hierarchy(
                            &core,
                            snapshot,
                            &mut hierarchy_revision,
                            &mut revisions,
                            &counters,
                        )
                    })
                }
                SemanticEventKind::HierarchyChanged { .. } => Ok(Vec::new()),
            },
            DriverEvent::Semantic(Err(_)) => {
                Err("terminal runtime stopped while waiting for a hierarchy event".into())
            }
        };

        match result {
            Ok(pending) => {
                for update in pending {
                    updates.publish(update);
                }
            }
            Err(error) => {
                updates.publish(DriverUpdate::Error(error));
            }
        }
    }
}

fn load_projection(core: &ApplicationCore) -> Result<CoreProjection, String> {
    let mut hierarchy = core.core_snapshot();
    for _ in 0..MAX_HIERARCHY_REFRESH_ATTEMPTS {
        let terminal_ids = terminal_ids(&hierarchy);
        let mut terminals = HashMap::with_capacity(terminal_ids.len());
        let mut superseded = None;
        for terminal_session_id in terminal_ids {
            match snapshot_for_hierarchy(core, terminal_session_id, hierarchy.revision)? {
                HierarchySnapshot::Terminal(snapshot) => {
                    terminals.insert(terminal_session_id, snapshot);
                }
                HierarchySnapshot::Superseded(snapshot) => {
                    superseded = Some(snapshot);
                    break;
                }
            }
        }
        if let Some(snapshot) = superseded {
            hierarchy = snapshot;
            continue;
        }
        return Ok(CoreProjection {
            hierarchy,
            terminals,
        });
    }
    Err("application hierarchy kept changing while loading terminal snapshots".into())
}

fn synchronize_hierarchy(
    core: &ApplicationCore,
    snapshot: CoreSnapshot,
    hierarchy_revision: &mut u64,
    revisions: &mut HashMap<TerminalSessionId, u64>,
    counters: &DriverCounters,
) -> Result<Vec<DriverUpdate>, String> {
    let mut snapshot = snapshot;
    for _ in 0..MAX_HIERARCHY_REFRESH_ATTEMPTS {
        let terminal_ids = terminal_ids(&snapshot).into_iter().collect::<HashSet<_>>();
        let mut next_revisions = revisions.clone();
        next_revisions.retain(|terminal_session_id, _| terminal_ids.contains(terminal_session_id));
        let mut updates = vec![DriverUpdate::Hierarchy(snapshot.clone())];
        let mut new_terminals = terminal_ids
            .into_iter()
            .filter(|terminal_session_id| !next_revisions.contains_key(terminal_session_id))
            .collect::<Vec<_>>();
        new_terminals.sort_unstable();
        let mut superseded = None;
        for terminal_session_id in new_terminals {
            counters.snapshot_requests.fetch_add(1, Ordering::Relaxed);
            match snapshot_for_hierarchy(core, terminal_session_id, snapshot.revision)? {
                HierarchySnapshot::Terminal(terminal) => {
                    next_revisions.insert(terminal_session_id, terminal.revision);
                    updates.push(DriverUpdate::Terminal {
                        terminal_session_id,
                        snapshot: terminal,
                    });
                }
                HierarchySnapshot::Superseded(current) => {
                    superseded = Some(current);
                    break;
                }
            }
        }
        if let Some(current) = superseded {
            snapshot = current;
            continue;
        }
        *hierarchy_revision = snapshot.revision;
        *revisions = next_revisions;
        counters
            .snapshots_published
            .fetch_add(updates.len().saturating_sub(1) as u64, Ordering::Relaxed);
        return Ok(updates);
    }
    Err("application hierarchy kept changing while synchronizing terminal snapshots".into())
}

enum HierarchySnapshot {
    Terminal(TerminalSnapshot),
    Superseded(CoreSnapshot),
}

fn terminal_ids(snapshot: &CoreSnapshot) -> Vec<TerminalSessionId> {
    snapshot
        .terminal_sessions
        .iter()
        .map(|session| session.id)
        .collect()
}

fn snapshot_for_hierarchy(
    core: &ApplicationCore,
    terminal_session_id: TerminalSessionId,
    hierarchy_revision: u64,
) -> Result<HierarchySnapshot, String> {
    match core.terminal_snapshot(terminal_session_id) {
        Ok(snapshot) => Ok(HierarchySnapshot::Terminal(snapshot)),
        Err(snapshot_error) => {
            let current = core.refresh_core_snapshot()?;
            let still_present = current
                .terminal_sessions
                .iter()
                .any(|session| session.id == terminal_session_id);
            if current.revision >= hierarchy_revision && !still_present {
                Ok(HierarchySnapshot::Superseded(current))
            } else {
                Err(snapshot_error)
            }
        }
    }
}

fn refresh_terminal(
    core: &ApplicationCore,
    terminal_session_id: TerminalSessionId,
    announced_revision: u64,
    revisions: &mut HashMap<TerminalSessionId, u64>,
    counters: &DriverCounters,
) -> Result<Option<DriverUpdate>, String> {
    let Some(&revision) = revisions.get(&terminal_session_id) else {
        return Ok(None);
    };
    if announced_revision <= revision {
        return Ok(None);
    }
    counters.snapshot_requests.fetch_add(1, Ordering::Relaxed);
    let Some(snapshot) = core.terminal_snapshot_since(terminal_session_id, revision)? else {
        return Ok(None);
    };
    revisions.insert(terminal_session_id, snapshot.revision);
    counters.snapshots_published.fetch_add(1, Ordering::Relaxed);
    Ok(Some(DriverUpdate::Terminal {
        terminal_session_id,
        snapshot,
    }))
}
#[cfg(test)]
mod tests {
    use super::load_projection;
    use crate::ApplicationCore;

    #[test]
    fn loads_initial_application_projection() {
        let core = ApplicationCore::start().expect("start application core");
        let projection = load_projection(&core).expect("load initial projection");

        assert_eq!(projection.hierarchy.spaces.len(), 1);
        assert_eq!(projection.terminals.len(), 1);
    }
}
