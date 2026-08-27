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
    commands: DriverCommandSender,
    updates: DriverUpdates,
    counters: Arc<DriverCounters>,
    next_command_id: AtomicU64,
}

struct DriverCommandSender {
    queue: flume::Sender<Command>,
    resize_batches: Arc<Mutex<ResizeBatchState>>,
}

#[derive(Default)]
struct ResizeBatchState {
    next_batch_id: u64,
    trailing_batch: Option<u64>,
    batches: HashMap<u64, HashMap<TerminalSessionId, TerminalSize>>,
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
    Scroll {
        terminal_session_id: TerminalSessionId,
        input: crate::ghostty::ScrollInput,
    },
    ResizeBatch {
        batch_id: u64,
    },
    SetTerminalTheme {
        theme: TerminalTheme,
    },
}

impl DriverCommandSender {
    fn new(queue: flume::Sender<Command>) -> Self {
        Self {
            queue,
            resize_batches: Arc::new(Mutex::new(ResizeBatchState::default())),
        }
    }

    fn worker_resize_batches(&self) -> Arc<Mutex<ResizeBatchState>> {
        Arc::clone(&self.resize_batches)
    }

    fn send(&self, command: Command) -> Result<(), String> {
        let mut resize_batches = self
            .resize_batches
            .lock()
            .expect("window driver resize mutex poisoned");
        resize_batches.trailing_batch = None;
        self.queue.try_send(command).map_err(command_send_error)
    }

    fn resize(
        &self,
        terminal_session_id: TerminalSessionId,
        size: TerminalSize,
    ) -> Result<(), String> {
        let mut resize_batches = self
            .resize_batches
            .lock()
            .expect("window driver resize mutex poisoned");
        if let Some(batch_id) = resize_batches.trailing_batch
            && let Some(batch) = resize_batches.batches.get_mut(&batch_id)
        {
            batch.insert(terminal_session_id, size);
            return Ok(());
        }

        let batch_id = resize_batches.next_batch_id;
        resize_batches.next_batch_id = resize_batches
            .next_batch_id
            .checked_add(1)
            .expect("window driver resize batch identifiers exhausted");
        resize_batches
            .batches
            .insert(batch_id, HashMap::from([(terminal_session_id, size)]));
        match self.queue.try_send(Command::ResizeBatch { batch_id }) {
            Ok(()) => {
                resize_batches.trailing_batch = Some(batch_id);
                Ok(())
            }
            Err(error) => {
                resize_batches.batches.remove(&batch_id);
                Err(command_send_error(error))
            }
        }
    }
}

impl ResizeBatchState {
    fn take_batch(
        &mut self,
        batch_id: u64,
    ) -> Result<Vec<(TerminalSessionId, TerminalSize)>, String> {
        let batch = self
            .batches
            .remove(&batch_id)
            .ok_or_else(|| format!("window driver resize batch {batch_id} is missing"))?;
        if self.trailing_batch == Some(batch_id) {
            self.trailing_batch = None;
        }
        let mut batch = batch.into_iter().collect::<Vec<_>>();
        batch.sort_unstable_by_key(|(terminal_session_id, _)| *terminal_session_id);
        Ok(batch)
    }
}

fn command_send_error(error: flume::TrySendError<Command>) -> String {
    match error {
        flume::TrySendError::Full(_) => "window driver command queue is busy".into(),
        flume::TrySendError::Disconnected(_) => "window driver stopped".into(),
    }
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
        let commands = DriverCommandSender::new(commands_tx);
        let worker_resize_batches = commands.worker_resize_batches();
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
                    worker_resize_batches,
                )
            })
            .map_err(|error| format!("spawn window driver: {error}"))?;
        Ok((
            Self {
                commands,
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

    pub(crate) fn scroll_terminal(
        &self,
        terminal_session_id: TerminalSessionId,
        input: crate::ghostty::ScrollInput,
    ) -> Result<(), String> {
        self.send(Command::Scroll {
            terminal_session_id,
            input,
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
        self.commands.resize(terminal_session_id, size)
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
        self.commands.send(command)
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
    resize_batches: Arc<Mutex<ResizeBatchState>>,
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
            DriverEvent::Command(Ok(Command::Scroll {
                terminal_session_id,
                input,
            })) => core
                .scroll_terminal(terminal_session_id, input)
                .map(|()| Vec::new())
                .map_err(|error| error.to_string()),
            DriverEvent::Command(Ok(Command::ResizeBatch { batch_id })) => {
                let batch = resize_batches
                    .lock()
                    .expect("window driver resize mutex poisoned")
                    .take_batch(batch_id);
                batch.and_then(|batch| apply_resize_batch(&core, batch))
            }
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

fn apply_resize_batch(
    core: &ApplicationCore,
    batch: Vec<(TerminalSessionId, TerminalSize)>,
) -> Result<Vec<DriverUpdate>, String> {
    let mut first_error = None;
    for (terminal_session_id, size) in batch {
        if let Err(error) = core.resize_terminal(terminal_session_id, size)
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(Vec::new()),
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
    use super::{Command, DriverCommandSender, DriverUpdate, load_projection};
    use crate::{ApplicationCore, TerminalSessionId, TerminalSize, core_driver::CoreDriver};
    use std::{
        thread,
        time::{Duration, Instant},
    };

    #[test]
    fn loads_initial_application_projection() {
        let core = ApplicationCore::start().expect("start application core");
        let projection = load_projection(&core).expect("load initial projection");

        assert_eq!(projection.hierarchy.spaces.len(), 1);
        assert_eq!(projection.terminals.len(), 1);
    }

    #[test]
    fn resize_burst_accepts_and_promptly_applies_the_latest_geometry() {
        let core = ApplicationCore::start().expect("start application core");
        let (driver, projection) = CoreDriver::start(core.clone()).expect("start window driver");
        let updates = driver.updates();
        let terminal_session_id = projection.hierarchy.terminal_sessions[0].id;
        let mut final_size = TerminalSize::new(80, 24, 9, 20);

        for step in 0..512_u16 {
            final_size = TerminalSize::new(80 + step % 40, 24 + step % 10, 9, 20);
            if let Err(error) = driver.resize_terminal(terminal_session_id, final_size) {
                panic!("interactive resize request {step} filled the driver queue: {error}");
            }
        }

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let snapshot = core
                .terminal_snapshot(terminal_session_id)
                .expect("read resized terminal snapshot");
            if snapshot.cols == final_size.cols && snapshot.rows == final_size.rows {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "latest requested geometry was not applied promptly: expected {}x{}, got {}x{}",
                final_size.cols,
                final_size.rows,
                snapshot.cols,
                snapshot.rows,
            );
            thread::sleep(Duration::from_millis(5));
        }

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match updates.take_pending() {
                Some(DriverUpdate::Terminal {
                    terminal_session_id: updated_terminal_session_id,
                    snapshot,
                }) if updated_terminal_session_id == terminal_session_id
                    && snapshot.cols == final_size.cols
                    && snapshot.rows == final_size.rows =>
                {
                    break;
                }
                Some(_) => {}
                None => thread::sleep(Duration::from_millis(5)),
            }
            assert!(
                Instant::now() < deadline,
                "latest requested geometry did not reach the UI projection promptly"
            );
        }
    }

    #[test]
    fn repeated_resize_target_republishes_the_current_projection() {
        let core = ApplicationCore::start().expect("start application core");
        let (driver, projection) = CoreDriver::start(core).expect("start window driver");
        let updates = driver.updates();
        let terminal_session_id = projection.hierarchy.terminal_sessions[0].id;
        let size = TerminalSize::new(100, 30, 9, 20);

        let receive_matching_projection = |minimum_revision| {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                match updates.take_pending() {
                    Some(DriverUpdate::Terminal {
                        terminal_session_id: updated_terminal_session_id,
                        snapshot,
                    }) if updated_terminal_session_id == terminal_session_id
                        && snapshot.cols == size.cols
                        && snapshot.rows == size.rows
                        && snapshot.revision > minimum_revision =>
                    {
                        return snapshot.revision;
                    }
                    Some(_) => {}
                    None => thread::sleep(Duration::from_millis(5)),
                }
                assert!(
                    Instant::now() < deadline,
                    "repeated resize target did not republish its current projection"
                );
            }
        };

        driver
            .resize_terminal(terminal_session_id, size)
            .expect("apply initial resize");
        let initial_revision = receive_matching_projection(0);

        driver
            .resize_terminal(terminal_session_id, size)
            .expect("repeat current resize");
        let repeated_revision = receive_matching_projection(initial_revision);

        assert!(repeated_revision > initial_revision);
    }

    #[test]
    fn pending_resize_batch_keeps_only_the_latest_geometry_per_terminal() {
        let (queue, commands) = flume::bounded(1);
        let sender = DriverCommandSender::new(queue);
        let terminal_session_id = TerminalSessionId::from_u64(7);
        let mut final_size = TerminalSize::new(80, 24, 9, 20);

        for step in 0..512_u16 {
            final_size = TerminalSize::new(80 + step % 40, 24 + step % 10, 9, 20);
            sender
                .resize(terminal_session_id, final_size)
                .expect("coalesce pending resize");
        }

        assert_eq!(commands.len(), 1, "one wake command represents the burst");
        let Command::ResizeBatch { batch_id } = commands.recv().expect("receive resize batch")
        else {
            panic!("resize burst queued a non-resize command");
        };
        let batch = sender
            .resize_batches
            .lock()
            .expect("resize mutex")
            .take_batch(batch_id)
            .expect("take resize batch");

        assert_eq!(batch, vec![(terminal_session_id, final_size)]);
    }

    #[test]
    fn input_command_is_a_barrier_between_resize_batches() {
        let (queue, commands) = flume::unbounded();
        let sender = DriverCommandSender::new(queue);
        let terminal_session_id = TerminalSessionId::from_u64(7);
        let before_input = TerminalSize::new(80, 24, 9, 20);
        let after_input = TerminalSize::new(120, 36, 9, 20);

        sender
            .resize(terminal_session_id, before_input)
            .expect("queue resize before input");
        sender
            .send(Command::Input {
                terminal_session_id,
                bytes: vec![b'x'],
            })
            .expect("queue input barrier");
        sender
            .resize(terminal_session_id, after_input)
            .expect("queue resize after input");

        let first_batch_id = match commands.recv().expect("first command") {
            Command::ResizeBatch { batch_id } => batch_id,
            _ => panic!("first command must be the pre-input resize"),
        };
        assert!(matches!(
            commands.recv().expect("second command"),
            Command::Input { .. }
        ));
        let second_batch_id = match commands.recv().expect("third command") {
            Command::ResizeBatch { batch_id } => batch_id,
            _ => panic!("third command must be the post-input resize"),
        };
        let mut batches = sender.resize_batches.lock().expect("resize mutex");

        assert_eq!(
            batches.take_batch(first_batch_id).expect("first batch"),
            vec![(terminal_session_id, before_input)]
        );
        assert_eq!(
            batches.take_batch(second_batch_id).expect("second batch"),
            vec![(terminal_session_id, after_input)]
        );
    }
}
