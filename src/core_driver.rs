use crate::{
    CoreClient, CoreCommand, CoreCommandOutcome, CoreSnapshot, SemanticEvent, SemanticEventKind,
    TerminalChange, TerminalSessionId, TerminalSize, TerminalSnapshot,
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
    Projection(CoreProjection),
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
    projection: Option<CoreProjection>,
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

    #[cfg(test)]
    fn recv_timeout(&self, timeout: std::time::Duration) -> Option<DriverUpdate> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(update) = self.take_pending() {
                return Some(update);
            }
            if self.is_stopped() {
                return None;
            }
            self.wakes
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .ok()?;
        }
    }

    fn take_pending(&self) -> Option<DriverUpdate> {
        let mut state = self
            .state
            .lock()
            .expect("UI Core driver update mutex poisoned");
        let update = if let Some(projection) = state.projection.take() {
            Some(DriverUpdate::Projection(projection))
        } else if let Some(hierarchy) = state.hierarchy.take() {
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
            .expect("UI Core driver update mutex poisoned")
            .stopped
    }
}

impl DriverUpdateState {
    fn has_pending(&self) -> bool {
        self.projection.is_some()
            || self.hierarchy.is_some()
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
            .expect("UI Core driver update mutex poisoned");
        match update {
            DriverUpdate::Projection(projection) => {
                state.hierarchy = None;
                state.terminals.clear();
                state.projection = Some(projection);
            }
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
            .expect("UI Core driver update mutex poisoned")
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
}

enum DriverEvent {
    Command(Result<Command, flume::RecvError>),
    TerminalChanged(Result<TerminalChange, flume::RecvError>),
    Semantic(Result<SemanticEvent, flume::RecvError>),
}

impl CoreDriver {
    pub(crate) fn start(mut core: CoreClient) -> Result<(Self, CoreProjection), String> {
        let endpoint = core.endpoint().clone();
        let projection = load_projection(&mut core)?;
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
        std::thread::Builder::new()
            .name("ui-core-driver".into())
            .spawn(move || {
                run_driver(
                    core,
                    endpoint,
                    hierarchy_revision,
                    revisions,
                    commands_rx,
                    publisher,
                    worker_counters,
                )
            })
            .map_err(|error| format!("spawn UI Core driver: {error}"))?;
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
                flume::TrySendError::Full(_) => "UI Core driver command queue is busy".into(),
                flume::TrySendError::Disconnected(_) => "UI Core driver stopped".into(),
            })
    }
}

impl Drop for CoreDriver {
    fn drop(&mut self) {
        if std::env::var_os("AGENT_TERMINAL_DRIVER_STATS").is_some() {
            let stats = self.stats();
            eprintln!(
                "UI Core driver: {} pushed changes, {} snapshot requests, {} snapshots published",
                stats.pushed_changes, stats.snapshot_requests, stats.snapshots_published
            );
        }
    }
}

fn run_driver(
    mut core: CoreClient,
    endpoint: crate::CoreEndpoint,
    mut hierarchy_revision: u64,
    mut revisions: HashMap<TerminalSessionId, u64>,
    commands: flume::Receiver<Command>,
    updates: DriverUpdatePublisher,
    counters: Arc<DriverCounters>,
) {
    let mut terminal_changes = core.terminal_changes();
    let mut semantic_events = core.semantic_events();
    loop {
        let event = flume::Selector::new()
            .recv(&commands, DriverEvent::Command)
            .recv(&terminal_changes, DriverEvent::TerminalChanged)
            .recv(&semantic_events, DriverEvent::Semantic)
            .wait();
        let result = match event {
            DriverEvent::Command(Err(_)) => break,
            DriverEvent::Command(Ok(Command::Apply {
                command_id,
                command,
            })) => match core.apply_core_command(command) {
                Ok(outcome) => synchronize_hierarchy(
                    &mut core,
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
                Err(error) => {
                    let error = error.to_string();
                    if is_connection_failure(&error) {
                        Err(error)
                    } else {
                        Ok(vec![DriverUpdate::CommandRejected { command_id, error }])
                    }
                }
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
            DriverEvent::TerminalChanged(Ok(change)) => {
                counters.pushed_changes.fetch_add(1, Ordering::Relaxed);
                refresh_terminal(
                    &mut core,
                    change.terminal_session_id,
                    change.terminal_revision,
                    &mut revisions,
                    &counters,
                )
                .map(|update| update.into_iter().collect())
            }
            DriverEvent::TerminalChanged(Err(_)) => {
                Err("Resident Core disconnected while waiting for a terminal change".into())
            }
            DriverEvent::Semantic(Ok(event)) => {
                core.accept_semantic_event(&event);
                match event.kind {
                    SemanticEventKind::TerminalLifecycleChanged {
                        terminal_session_id,
                        terminal_revision,
                        ..
                    } => refresh_terminal(
                        &mut core,
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
                                &mut core,
                                snapshot,
                                &mut hierarchy_revision,
                                &mut revisions,
                                &counters,
                            )
                        })
                    }
                    SemanticEventKind::ControlLeaseChanged { .. }
                    | SemanticEventKind::HierarchyChanged { .. } => Ok(Vec::new()),
                }
            }
            DriverEvent::Semantic(Err(_)) => {
                Err("Resident Core disconnected while waiting for a semantic event".into())
            }
        };

        match result {
            Ok(pending) => {
                for update in pending {
                    updates.publish(update);
                }
            }
            Err(error) => {
                let reconnect = is_connection_failure(&error);
                updates.publish(DriverUpdate::Error(error));
                if !reconnect {
                    continue;
                }
                let recovered =
                    CoreClient::connect_or_spawn_at(&endpoint).and_then(|mut replacement| {
                        let projection = load_projection(&mut replacement)?;
                        Ok((replacement, projection))
                    });
                match recovered {
                    Ok((replacement, projection)) => {
                        terminal_changes = replacement.terminal_changes();
                        semantic_events = replacement.semantic_events();
                        hierarchy_revision = projection.hierarchy.revision;
                        revisions = projection
                            .terminals
                            .iter()
                            .map(|(&terminal_session_id, snapshot)| {
                                (terminal_session_id, snapshot.revision)
                            })
                            .collect();
                        core = replacement;
                        counters
                            .snapshots_published
                            .fetch_add(projection.terminals.len() as u64, Ordering::Relaxed);
                        updates.publish(DriverUpdate::Projection(projection));
                    }
                    Err(error) => {
                        updates.publish(DriverUpdate::Error(format!(
                            "Resident Core reconnect failed: {error}"
                        )));
                        break;
                    }
                }
            }
        }
    }
}

fn load_projection(core: &mut CoreClient) -> Result<CoreProjection, String> {
    let mut hierarchy = core.core_snapshot().clone();
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
    Err("Resident Core hierarchy kept changing while loading terminal snapshots".into())
}

fn synchronize_hierarchy(
    core: &mut CoreClient,
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
        counters.snapshots_published.fetch_add(
            updates.len().saturating_sub(1) as u64,
            Ordering::Relaxed,
        );
        return Ok(updates);
    }
    Err("Resident Core hierarchy kept changing while synchronizing terminal snapshots".into())
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
    core: &mut CoreClient,
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
    core: &mut CoreClient,
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

fn is_connection_failure(error: &str) -> bool {
    error.starts_with("send Resident Core command:")
        || error.starts_with("receive Resident Core response:")
        || error.starts_with("Resident Core disconnected")
}

#[cfg(test)]
mod tests {
    use super::{
        CoreDriver, DriverCounters, DriverUpdate, DriverUpdatePublisher, DriverUpdateState,
        DriverUpdates, is_connection_failure, synchronize_hierarchy,
    };
    use crate::{
        CoreClient, CoreCommand, CoreEndpoint, CoreSnapshot, CreatedResource, SemanticEventKind,
        SpaceId, TerminalLifecycle, TerminalSessionId, TerminalSnapshot, run_resident_core,
    };
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    struct TestCore {
        endpoint: CoreEndpoint,
        thread: Option<thread::JoinHandle<Result<(), String>>>,
    }

    impl TestCore {
        fn start(scenario: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock after Unix epoch")
                .as_nanos();
            let endpoint = CoreEndpoint::for_profile(&format!(
                "core-driver-{scenario}-{}-{nonce}",
                std::process::id()
            ))
            .expect("create isolated Resident Core endpoint");
            let server_endpoint = endpoint.clone();
            let thread = thread::spawn(move || run_resident_core(server_endpoint));
            Self {
                endpoint,
                thread: Some(thread),
            }
        }
    }

    impl Drop for TestCore {
        fn drop(&mut self) {
            if let Ok(mut client) = CoreClient::connect(&self.endpoint, Duration::from_secs(10)) {
                let _ = client.stop_resident_core();
            }
            if let Some(thread) = self.thread.take() {
                assert_eq!(thread.join().expect("join Resident Core thread"), Ok(()));
            }
        }
    }

    #[test]
    fn terminal_output_reaches_the_view_without_snapshot_polling() {
        let core = TestCore::start("pushed-update");
        let client =
            CoreClient::connect(&core.endpoint, Duration::from_secs(10)).expect("attach UI Client");
        let (driver, projection) = CoreDriver::start(client).expect("start UI Core driver");
        let terminal_session_id = projection.hierarchy.terminal_sessions[0].id;
        let updates = driver.updates();

        while updates.recv_timeout(Duration::from_millis(100)).is_some() {}
        let idle = driver.stats();
        assert!(
            updates.recv_timeout(Duration::from_millis(100)).is_none(),
            "a quiet UI Core driver must not publish polling updates"
        );
        assert_eq!(driver.stats(), idle);

        driver
            .input_to(terminal_session_id, b"echo DRIVER_PUSHED_UPDATE\r".to_vec())
            .expect("send terminal input");

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match updates.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Some(DriverUpdate::Terminal {
                    terminal_session_id: changed_terminal,
                    snapshot,
                }) if changed_terminal == terminal_session_id
                    && snapshot.text().contains("DRIVER_PUSHED_UPDATE") =>
                {
                    break;
                }
                Some(DriverUpdate::Error(error)) => panic!("UI Core driver failed: {error}"),
                _ if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                _ => panic!("UI Core driver did not publish pushed terminal output"),
            }
        }
        let active = driver.stats();
        assert!(active.pushed_changes > 0);
        assert!(active.snapshot_requests > 0);
        assert!(active.snapshots_published > 0);
    }

    #[test]
    fn driver_start_recovers_when_the_handshake_terminal_has_already_exited() {
        let (_core, client, _) = client_with_superseded_handshake("driver-start");

        let (_driver, projection) =
            CoreDriver::start(client).expect("start from the authoritative current hierarchy");
        assert!(projection.hierarchy.spaces.is_empty());
        assert!(projection.hierarchy.terminal_sessions.is_empty());
        assert!(projection.terminals.is_empty());
    }

    #[test]
    fn hierarchy_sync_recovers_when_a_new_terminal_has_already_exited() {
        let (_core, mut client, stale_hierarchy) =
            client_with_superseded_handshake("hierarchy-sync");
        let mut hierarchy_revision = 0;
        let mut revisions = HashMap::new();

        let updates = synchronize_hierarchy(
            &mut client,
            stale_hierarchy,
            &mut hierarchy_revision,
            &mut revisions,
            &DriverCounters::default(),
        )
        .expect("synchronize from the authoritative current hierarchy");

        assert_eq!(updates.len(), 1);
        assert!(matches!(
            &updates[0],
            DriverUpdate::Hierarchy(snapshot)
                if snapshot.spaces.is_empty() && snapshot.terminal_sessions.is_empty()
        ));
        assert!(revisions.is_empty());
    }

    #[test]
    fn slow_views_keep_the_latest_snapshot_for_each_terminal() {
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
        let first = TerminalSessionId::from_u64(1);
        let second = TerminalSessionId::from_u64(2);
        publisher.publish(terminal_update(first, 1));
        publisher.publish(terminal_update(second, 1));
        publisher.publish(terminal_update(first, 3));

        assert!(matches!(
            updates.recv_timeout(Duration::from_secs(1)),
            Some(DriverUpdate::Terminal {
                terminal_session_id,
                snapshot
            }) if terminal_session_id == first && snapshot.revision == 3
        ));
        assert!(matches!(
            updates.recv_timeout(Duration::from_secs(1)),
            Some(DriverUpdate::Terminal {
                terminal_session_id,
                snapshot
            }) if terminal_session_id == second && snapshot.revision == 1
        ));
        assert!(updates.recv_timeout(Duration::from_millis(10)).is_none());
    }

    #[test]
    fn hierarchy_events_add_independent_terminal_projections() {
        let core = TestCore::start("hierarchy-update");
        let client =
            CoreClient::connect(&core.endpoint, Duration::from_secs(10)).expect("attach UI Client");
        let (driver, initial) = CoreDriver::start(client).expect("start UI Core driver");
        let updates = driver.updates();
        let mut mutator =
            CoreClient::connect(&core.endpoint, Duration::from_secs(10)).expect("attach mutator");
        let created = mutator
            .apply_core_command(CoreCommand::CreateSpace {
                name: "Driver Space".into(),
                directory: std::env::current_dir().expect("current directory"),
            })
            .expect("create Space through another attached UI Client");
        let CreatedResource::Space {
            terminal_session_id,
            ..
        } = created.created
        else {
            panic!("Space creation must identify its Terminal Session");
        };

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut saw_hierarchy = false;
        let mut saw_terminal = false;
        while Instant::now() < deadline && !(saw_hierarchy && saw_terminal) {
            match updates.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Some(DriverUpdate::Hierarchy(snapshot)) => {
                    saw_hierarchy = snapshot.revision == created.revision
                        && snapshot.spaces.len() == initial.hierarchy.spaces.len() + 1;
                }
                Some(DriverUpdate::Terminal {
                    terminal_session_id: projected,
                    ..
                }) => saw_terminal |= projected == terminal_session_id,
                Some(DriverUpdate::Error(error)) => panic!("UI Core driver failed: {error}"),
                Some(DriverUpdate::Projection(_))
                | Some(DriverUpdate::CommandAccepted { .. })
                | Some(DriverUpdate::CommandRejected { .. })
                | None => {}
            }
        }
        assert!(saw_hierarchy, "driver did not publish the new hierarchy");
        assert!(
            saw_terminal,
            "driver did not project the new Terminal Session"
        );
    }

    #[test]
    fn commands_publish_the_created_resource_after_the_authoritative_hierarchy() {
        let core = TestCore::start("command-outcome");
        let client =
            CoreClient::connect(&core.endpoint, Duration::from_secs(10)).expect("attach UI Client");
        let (driver, initial) = CoreDriver::start(client).expect("start UI Core driver");
        let updates = driver.updates();
        let space_id = initial.hierarchy.spaces[0].id;

        let queued_command_id = driver
            .apply_core_command(CoreCommand::CreateTab {
                space_id,
                name: "Created Tab".into(),
            })
            .expect("queue Core command");

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut hierarchy_revision = None;
        loop {
            match updates.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Some(DriverUpdate::Hierarchy(snapshot)) => {
                    hierarchy_revision = Some(snapshot.revision);
                }
                Some(DriverUpdate::CommandAccepted {
                    command_id,
                    outcome,
                }) => {
                    assert_eq!(command_id, queued_command_id);
                    assert_eq!(hierarchy_revision, Some(outcome.revision));
                    assert!(matches!(outcome.created, CreatedResource::Tab { .. }));
                    break;
                }
                Some(DriverUpdate::CommandRejected { error, .. }) => {
                    panic!("UI Core command was rejected: {error}")
                }
                Some(DriverUpdate::Error(error)) => panic!("UI Core driver failed: {error}"),
                Some(DriverUpdate::Projection(_)) | Some(DriverUpdate::Terminal { .. })
                    if Instant::now() < deadline => {}
                _ => panic!("UI Core driver did not publish the command outcome"),
            }
        }
    }

    #[test]
    fn rejected_commands_keep_their_driver_command_identity() {
        let core = TestCore::start("command-rejection");
        let client =
            CoreClient::connect(&core.endpoint, Duration::from_secs(10)).expect("attach UI Client");
        let (driver, _) = CoreDriver::start(client).expect("start UI Core driver");
        let updates = driver.updates();
        let command_id = driver
            .apply_core_command(CoreCommand::CreateTab {
                space_id: SpaceId::from_u64(u64::MAX),
                name: "Nowhere".into(),
            })
            .expect("queue invalid Core command");

        match updates.recv_timeout(Duration::from_secs(2)) {
            Some(DriverUpdate::CommandRejected {
                command_id: rejected_id,
                error,
            }) => {
                assert_eq!(rejected_id, command_id);
                assert!(error.contains("does not exist"));
            }
            Some(DriverUpdate::Error(error)) => {
                panic!("semantic rejection was reported as a connection failure: {error}")
            }
            _ => panic!("UI Core driver did not publish the command rejection"),
        }
    }

    #[test]
    fn semantic_command_errors_do_not_open_a_second_control_connection() {
        assert!(!is_connection_failure("terminal grid exceeds capacity"));
        assert!(is_connection_failure(
            "receive Resident Core response: connection reset"
        ));
    }

    fn terminal_update(terminal_session_id: TerminalSessionId, revision: u64) -> DriverUpdate {
        DriverUpdate::Terminal {
            terminal_session_id,
            snapshot: TerminalSnapshot {
                revision,
                lifecycle: TerminalLifecycle::Running,
                cols: 1,
                rows: 1,
                cursor: None,
                default_fg: [0xdd; 3],
                default_bg: [0x11; 3],
                cells: Vec::new(),
            },
        }
    }

    fn client_with_superseded_handshake(
        scenario: &str,
    ) -> (TestCore, CoreClient, CoreSnapshot) {
        let core = TestCore::start(scenario);
        let mut client =
            CoreClient::connect(&core.endpoint, Duration::from_secs(10)).expect("attach UI Client");
        let stale_hierarchy = client.core_snapshot().clone();
        let terminal_session_id = stale_hierarchy.terminal_sessions[0].id;
        client
            .input_to(terminal_session_id, b"exit\r")
            .expect("exit the handshake Terminal Session");

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let event = client
                .wait_for_semantic_event(deadline.saturating_duration_since(Instant::now()))
                .expect("wait for natural-exit hierarchy event")
                .unwrap_or_else(|| panic!("natural exit did not update the hierarchy"));
            if matches!(event.kind, SemanticEventKind::HierarchyChanged { .. }) {
                break;
            }
        }
        (core, client, stale_hierarchy)
    }
}
