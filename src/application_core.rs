use crate::{
    CoreCommand, CoreModelError, CoreSnapshot, CreatedResource, TerminalSessionId, ghostty,
    terminal_session::TerminalSize, terminal_theme::TerminalTheme,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

mod runtime;

use runtime::{CoreRuntime, RuntimeEvent};

const RUNTIME_TICK: Duration = Duration::from_millis(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalChange {
    pub terminal_session_id: TerminalSessionId,
    pub terminal_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticEvent {
    pub kind: SemanticEventKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SemanticEventKind {
    TerminalLifecycleChanged {
        terminal_session_id: TerminalSessionId,
        lifecycle: TerminalLifecycle,
        terminal_revision: u64,
    },
    HierarchyChanged {
        revision: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreCommandOutcome {
    pub revision: u64,
    pub snapshot: CoreSnapshot,
    pub created: CreatedResource,
}

#[derive(Clone)]
pub struct ApplicationCore {
    inner: Arc<ApplicationCoreInner>,
    hierarchy: Arc<Mutex<CoreSnapshot>>,
    terminal_snapshots: Arc<Mutex<HashMap<TerminalSessionId, TerminalSnapshot>>>,
}

struct ApplicationCoreInner {
    commands: flume::Sender<WorkerCommand>,
    model_commands: Mutex<()>,
    terminal_subscribers: Arc<Mutex<Vec<flume::Sender<TerminalChange>>>>,
    semantic_subscribers: Arc<Mutex<Vec<flume::Sender<SemanticEvent>>>>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

enum WorkerRequest {
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
    Snapshot {
        terminal_session_id: TerminalSessionId,
        since: Option<u64>,
    },
    CoreSnapshot,
    ApplyCoreCommand {
        expected_revision: u64,
        command: CoreCommand,
    },
    Stop,
}

enum WorkerResponse {
    Ack,
    Snapshot(Option<TerminalUpdate>),
    CoreSnapshot(CoreSnapshot),
    CoreCommandAccepted(CoreCommandOutcome),
    CoreCommandRejected(CoreModelError),
}

struct WorkerCommand {
    request: WorkerRequest,
    response: flume::Sender<Result<WorkerResponse, String>>,
}

impl ApplicationCore {
    pub fn start() -> Result<Self, String> {
        let (commands, worker_commands) = flume::bounded::<WorkerCommand>(32);
        let (ready, worker_ready) = flume::bounded(1);
        let terminal_subscribers = Arc::new(Mutex::new(Vec::new()));
        let worker_terminal_subscribers = Arc::clone(&terminal_subscribers);
        let semantic_subscribers = Arc::new(Mutex::new(Vec::new()));
        let worker_semantic_subscribers = Arc::clone(&semantic_subscribers);
        let worker = thread::Builder::new()
            .name("terminal-runtime".into())
            .spawn(move || {
                run_terminal_runtime(
                    worker_commands,
                    ready,
                    worker_terminal_subscribers,
                    worker_semantic_subscribers,
                )
            })
            .map_err(|error| format!("spawn terminal runtime: {error}"))?;
        let initial_snapshot = worker_ready
            .recv()
            .map_err(|_| "terminal runtime stopped during startup".to_string())??;
        Ok(Self {
            inner: Arc::new(ApplicationCoreInner {
                commands,
                model_commands: Mutex::new(()),
                terminal_subscribers,
                semantic_subscribers,
                worker: Mutex::new(Some(worker)),
            }),
            hierarchy: Arc::new(Mutex::new(initial_snapshot)),
            terminal_snapshots: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn core_snapshot(&self) -> CoreSnapshot {
        self.hierarchy
            .lock()
            .expect("application hierarchy mutex poisoned")
            .clone()
    }

    pub fn refresh_core_snapshot(&self) -> Result<CoreSnapshot, String> {
        let snapshot = match self.call(WorkerRequest::CoreSnapshot)? {
            WorkerResponse::CoreSnapshot(snapshot) => snapshot,
            _ => return Err("invalid hierarchy snapshot response".into()),
        };
        *self
            .hierarchy
            .lock()
            .expect("application hierarchy mutex poisoned") = snapshot.clone();
        Ok(snapshot)
    }

    pub fn terminal_snapshot(
        &self,
        terminal_session_id: TerminalSessionId,
    ) -> Result<TerminalSnapshot, String> {
        let update = match self.call(WorkerRequest::Snapshot {
            terminal_session_id,
            since: None,
        })? {
            WorkerResponse::Snapshot(Some(update)) => update,
            WorkerResponse::Snapshot(None) => {
                return Err("terminal runtime returned no initial snapshot".into());
            }
            _ => return Err("invalid terminal snapshot response".into()),
        };
        let snapshot = TerminalSnapshot::from_update(update)?;
        self.terminal_snapshots
            .lock()
            .expect("terminal snapshot mutex poisoned")
            .insert(terminal_session_id, snapshot.clone());
        Ok(snapshot)
    }

    pub fn terminal_snapshot_since(
        &self,
        terminal_session_id: TerminalSessionId,
        revision: u64,
    ) -> Result<Option<TerminalSnapshot>, String> {
        let update = match self.call(WorkerRequest::Snapshot {
            terminal_session_id,
            since: Some(revision),
        })? {
            WorkerResponse::Snapshot(update) => update,
            _ => return Err("invalid terminal snapshot response".into()),
        };
        let Some(update) = update else {
            return Ok(None);
        };
        let mut snapshots = self
            .terminal_snapshots
            .lock()
            .expect("terminal snapshot mutex poisoned");
        apply_terminal_update(&mut snapshots, terminal_session_id, update).map(Some)
    }

    pub fn apply_core_command(&self, command: CoreCommand) -> Result<CoreCommandOutcome, String> {
        let _command = self
            .inner
            .model_commands
            .lock()
            .expect("application model command mutex poisoned");
        let expected_revision = self
            .hierarchy
            .lock()
            .expect("application hierarchy mutex poisoned")
            .revision;
        let response = self.call(WorkerRequest::ApplyCoreCommand {
            expected_revision,
            command,
        })?;
        match response {
            WorkerResponse::CoreCommandAccepted(outcome) => {
                *self
                    .hierarchy
                    .lock()
                    .expect("application hierarchy mutex poisoned") = outcome.snapshot.clone();
                Ok(outcome)
            }
            WorkerResponse::CoreCommandRejected(error) => Err(error.to_string()),
            _ => Err("invalid hierarchy command response".into()),
        }
    }

    pub fn input_to(
        &self,
        terminal_session_id: TerminalSessionId,
        bytes: &[u8],
    ) -> Result<(), String> {
        self.expect_ack(WorkerRequest::Input {
            terminal_session_id,
            bytes: bytes.to_vec(),
        })
    }

    pub fn paste_to(
        &self,
        terminal_session_id: TerminalSessionId,
        bytes: &[u8],
    ) -> Result<(), String> {
        self.expect_ack(WorkerRequest::Paste {
            terminal_session_id,
            bytes: bytes.to_vec(),
        })
    }

    pub fn resize_terminal(
        &self,
        terminal_session_id: TerminalSessionId,
        size: TerminalSize,
    ) -> Result<(), String> {
        self.expect_ack(WorkerRequest::Resize {
            terminal_session_id,
            size,
        })
    }

    pub(crate) fn set_terminal_theme(&self, theme: TerminalTheme) -> Result<(), String> {
        self.expect_ack(WorkerRequest::SetTerminalTheme { theme })
    }

    pub fn terminal_changes(&self) -> flume::Receiver<TerminalChange> {
        let (sender, receiver) = flume::unbounded();
        self.inner
            .terminal_subscribers
            .lock()
            .expect("terminal subscriber mutex poisoned")
            .push(sender);
        receiver
    }

    pub fn semantic_events(&self) -> flume::Receiver<SemanticEvent> {
        let (sender, receiver) = flume::unbounded();
        self.inner
            .semantic_subscribers
            .lock()
            .expect("semantic subscriber mutex poisoned")
            .push(sender);
        receiver
    }

    fn expect_ack(&self, request: WorkerRequest) -> Result<(), String> {
        match self.call(request)? {
            WorkerResponse::Ack => Ok(()),
            _ => Err("invalid terminal command response".into()),
        }
    }

    fn call(&self, request: WorkerRequest) -> Result<WorkerResponse, String> {
        let (response, worker_response) = flume::bounded(1);
        self.inner
            .commands
            .send(WorkerCommand { request, response })
            .map_err(|_| "terminal runtime stopped".to_string())?;
        worker_response
            .recv()
            .map_err(|_| "terminal runtime stopped before responding".to_string())?
    }
}

fn apply_terminal_update(
    snapshots: &mut HashMap<TerminalSessionId, TerminalSnapshot>,
    terminal_session_id: TerminalSessionId,
    update: TerminalUpdate,
) -> Result<TerminalSnapshot, String> {
    if update.base_revision.is_none() {
        let snapshot = TerminalSnapshot::from_update(update)?;
        snapshots.insert(terminal_session_id, snapshot.clone());
        return Ok(snapshot);
    }

    let snapshot = snapshots.get_mut(&terminal_session_id).ok_or_else(|| {
        format!(
            "Terminal Session {} has no base snapshot",
            terminal_session_id.as_u64()
        )
    })?;
    snapshot.apply_update(update)?;
    Ok(snapshot.clone())
}

impl Drop for ApplicationCoreInner {
    fn drop(&mut self) {
        let (response, worker_response) = flume::bounded(1);
        let _ = self.commands.send(WorkerCommand {
            request: WorkerRequest::Stop,
            response,
        });
        let _ = worker_response.recv_timeout(Duration::from_secs(2));
        if let Some(worker) = self
            .worker
            .lock()
            .expect("terminal worker mutex poisoned")
            .take()
        {
            let _ = worker.join();
        }
    }
}

fn run_terminal_runtime(
    commands: flume::Receiver<WorkerCommand>,
    ready: flume::Sender<Result<CoreSnapshot, String>>,
    terminal_subscribers: Arc<Mutex<Vec<flume::Sender<TerminalChange>>>>,
    semantic_subscribers: Arc<Mutex<Vec<flume::Sender<SemanticEvent>>>>,
) {
    let started = std::env::current_dir()
        .map_err(|error| format!("resolve initial Space directory: {error}"))
        .and_then(|directory| CoreRuntime::start(&directory, 1));
    let mut runtime = match started {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let _ = ready.send(Ok(runtime.model_snapshot()));

    loop {
        publish_runtime_events(
            runtime.refresh(),
            &terminal_subscribers,
            &semantic_subscribers,
        );
        match commands.recv_timeout(RUNTIME_TICK) {
            Ok(command) => {
                publish_runtime_events(
                    runtime.refresh(),
                    &terminal_subscribers,
                    &semantic_subscribers,
                );
                let stop = matches!(command.request, WorkerRequest::Stop);
                let mut changed_terminals = Vec::new();
                let result = match command.request {
                    WorkerRequest::Input {
                        terminal_session_id,
                        bytes,
                    } => {
                        if runtime.contains_terminal(terminal_session_id) {
                            runtime
                                .input(terminal_session_id, &bytes)
                                .map(|()| WorkerResponse::Ack)
                        } else {
                            Ok(WorkerResponse::Ack)
                        }
                    }
                    WorkerRequest::Paste {
                        terminal_session_id,
                        bytes,
                    } => {
                        if runtime.contains_terminal(terminal_session_id) {
                            runtime.paste(terminal_session_id, &bytes).map(|changed| {
                                if changed
                                    && let Ok(revision) =
                                        runtime.terminal_revision(terminal_session_id)
                                {
                                    changed_terminals.push((terminal_session_id, revision));
                                }
                                WorkerResponse::Ack
                            })
                        } else {
                            Ok(WorkerResponse::Ack)
                        }
                    }
                    WorkerRequest::Resize {
                        terminal_session_id,
                        size,
                    } => {
                        if runtime.contains_terminal(terminal_session_id) {
                            runtime.resize(terminal_session_id, size).map(|changed| {
                                if changed
                                    && let Ok(revision) =
                                        runtime.terminal_revision(terminal_session_id)
                                {
                                    changed_terminals.push((terminal_session_id, revision));
                                }
                                WorkerResponse::Ack
                            })
                        } else {
                            Ok(WorkerResponse::Ack)
                        }
                    }
                    WorkerRequest::Snapshot {
                        terminal_session_id,
                        since,
                    } => match runtime.snapshot(terminal_session_id, since) {
                        Ok(snapshot) => Ok(WorkerResponse::Snapshot(snapshot)),
                        Err(_)
                            if since.is_some()
                                && !runtime.contains_terminal(terminal_session_id) =>
                        {
                            Ok(WorkerResponse::Snapshot(None))
                        }
                        Err(error) => Err(error),
                    },
                    WorkerRequest::CoreSnapshot => {
                        Ok(WorkerResponse::CoreSnapshot(runtime.model_snapshot()))
                    }
                    WorkerRequest::SetTerminalTheme { theme } => {
                        runtime.set_theme(theme).map(|changes| {
                            changed_terminals.extend(changes);
                            WorkerResponse::Ack
                        })
                    }
                    WorkerRequest::ApplyCoreCommand {
                        expected_revision,
                        command,
                    } => match runtime.apply(expected_revision, command) {
                        Ok(commit) => Ok(WorkerResponse::CoreCommandAccepted(CoreCommandOutcome {
                            revision: commit.revision,
                            snapshot: commit.snapshot,
                            created: commit.created,
                        })),
                        Err(error) => Ok(WorkerResponse::CoreCommandRejected(error)),
                    },
                    WorkerRequest::Stop => Ok(WorkerResponse::Ack),
                };
                for (terminal_session_id, terminal_revision) in changed_terminals {
                    publish_terminal_change(
                        &terminal_subscribers,
                        terminal_session_id,
                        terminal_revision,
                    );
                }
                let _ = command.response.send(result);
                if stop {
                    break;
                }
            }
            Err(flume::RecvTimeoutError::Timeout) => {}
            Err(flume::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn publish_runtime_events(
    events: Vec<RuntimeEvent>,
    terminal_subscribers: &Arc<Mutex<Vec<flume::Sender<TerminalChange>>>>,
    semantic_subscribers: &Arc<Mutex<Vec<flume::Sender<SemanticEvent>>>>,
) {
    for event in events {
        match event {
            RuntimeEvent::TerminalChanged {
                terminal_session_id,
                terminal_revision,
            } => publish_terminal_change(
                terminal_subscribers,
                terminal_session_id,
                terminal_revision,
            ),
            RuntimeEvent::TerminalLifecycleChanged {
                terminal_session_id,
                lifecycle,
                terminal_revision,
            } => publish_semantic_event(
                semantic_subscribers,
                SemanticEventKind::TerminalLifecycleChanged {
                    terminal_session_id,
                    lifecycle,
                    terminal_revision,
                },
            ),
            RuntimeEvent::PaneClosed { revision, .. } => publish_semantic_event(
                semantic_subscribers,
                SemanticEventKind::HierarchyChanged { revision },
            ),
        }
    }
}

fn publish_terminal_change(
    subscribers: &Arc<Mutex<Vec<flume::Sender<TerminalChange>>>>,
    terminal_session_id: TerminalSessionId,
    terminal_revision: u64,
) {
    let change = TerminalChange {
        terminal_session_id,
        terminal_revision,
    };
    subscribers
        .lock()
        .expect("terminal subscriber mutex poisoned")
        .retain(|subscriber| subscriber.send(change.clone()).is_ok());
}

fn publish_semantic_event(
    subscribers: &Arc<Mutex<Vec<flume::Sender<SemanticEvent>>>>,
    kind: SemanticEventKind,
) {
    let event = SemanticEvent { kind };
    subscribers
        .lock()
        .expect("semantic subscriber mutex poisoned")
        .retain(|subscriber| subscriber.send(event.clone()).is_ok());
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalSnapshot {
    pub revision: u64,
    pub lifecycle: TerminalLifecycle,
    pub active_work: bool,
    pub cols: u16,
    pub rows: u16,
    pub cursor: Option<(u16, u16)>,
    pub default_fg: [u8; 3],
    pub default_bg: [u8; 3],
    pub cells: Vec<TerminalCell>,
}

impl TerminalSnapshot {
    pub fn text(&self) -> String {
        let mut output = String::new();
        for y in 0..self.rows {
            for x in 0..self.cols {
                match self.cells.iter().find(|cell| cell.x == x && cell.y == y) {
                    Some(cell) if !cell.text.is_empty() => output.push_str(&cell.text),
                    _ => output.push(' '),
                }
            }
            output.push('\n');
        }
        output
    }

    fn from_update(update: TerminalUpdate) -> Result<Self, String> {
        if update.base_revision.is_some() {
            return Err("terminal runtime returned a delta without a base snapshot".into());
        }
        update.validate()?;
        Ok(Self {
            revision: update.revision,
            lifecycle: update.lifecycle,
            active_work: update.active_work,
            cols: update.cols,
            rows: update.rows,
            cursor: update.cursor,
            default_fg: update.default_fg,
            default_bg: update.default_bg,
            cells: update.cells,
        })
    }

    fn apply_update(&mut self, update: TerminalUpdate) -> Result<(), String> {
        update.validate()?;
        if update.base_revision != Some(self.revision) {
            return Err(format!(
                "terminal revision gap: local {}, update base {:?}",
                self.revision, update.base_revision
            ));
        }
        if (update.cols, update.rows) != (self.cols, self.rows) {
            return Err("terminal runtime changed geometry in a row delta".into());
        }
        let mut replace_row = vec![false; usize::from(self.rows)];
        for &row in &update.dirty_rows {
            replace_row[usize::from(row)] = true;
        }
        self.cells.retain(|cell| !replace_row[usize::from(cell.y)]);
        self.cells.extend(update.cells);
        self.cells.sort_unstable_by_key(|cell| (cell.y, cell.x));
        self.revision = update.revision;
        self.lifecycle = update.lifecycle;
        self.active_work = update.active_work;
        self.cursor = update.cursor;
        self.default_fg = update.default_fg;
        self.default_bg = update.default_bg;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TerminalUpdate {
    base_revision: Option<u64>,
    revision: u64,
    lifecycle: TerminalLifecycle,
    active_work: bool,
    cols: u16,
    rows: u16,
    cursor: Option<(u16, u16)>,
    default_fg: [u8; 3],
    default_bg: [u8; 3],
    dirty_rows: Vec<u16>,
    cells: Vec<TerminalCell>,
}

impl TerminalUpdate {
    fn from_terminal(
        snapshot: ghostty::Snapshot,
        base_revision: Option<u64>,
        revision: u64,
        lifecycle: TerminalLifecycle,
        active_work: bool,
    ) -> Self {
        Self {
            base_revision: if snapshot.full { None } else { base_revision },
            revision,
            lifecycle,
            active_work,
            cols: snapshot.cols,
            rows: snapshot.rows,
            cursor: snapshot.cursor,
            default_fg: snapshot.default_fg,
            default_bg: snapshot.default_bg,
            dirty_rows: snapshot.dirty_rows,
            cells: snapshot.cells.into_iter().map(TerminalCell::from).collect(),
        }
    }

    fn empty(revision: u64, lifecycle: TerminalLifecycle) -> Self {
        const COLS: u16 = 80;
        const ROWS: u16 = 24;
        Self {
            base_revision: None,
            revision,
            lifecycle,
            active_work: false,
            cols: COLS,
            rows: ROWS,
            cursor: None,
            default_fg: [0xd8, 0xde, 0xe9],
            default_bg: [0x0b, 0x0e, 0x13],
            dirty_rows: (0..ROWS).collect(),
            cells: Vec::new(),
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.cols == 0 || self.rows == 0 {
            return Err("terminal update has zero geometry".into());
        }
        if self.dirty_rows.windows(2).any(|rows| rows[0] >= rows[1])
            || self.dirty_rows.iter().any(|&row| row >= self.rows)
        {
            return Err("terminal update has invalid dirty rows".into());
        }
        if self.base_revision.is_none() && self.dirty_rows.len() != usize::from(self.rows) {
            return Err("full terminal update does not contain every row".into());
        }
        if self.base_revision.is_none()
            && self
                .dirty_rows
                .iter()
                .copied()
                .enumerate()
                .any(|(expected, actual)| usize::from(actual) != expected)
        {
            return Err("full terminal update rows are not complete".into());
        }
        let mut dirty = vec![false; usize::from(self.rows)];
        for &row in &self.dirty_rows {
            dirty[usize::from(row)] = true;
        }
        if self.cells.iter().any(|cell| {
            cell.x >= self.cols
                || cell.y >= self.rows
                || cell.width > 2
                || !dirty[usize::from(cell.y)]
        }) {
            return Err("terminal update cell is outside its dirty rows".into());
        }
        if let Some((x, y)) = self.cursor
            && (x >= self.cols || y >= self.rows)
        {
            return Err("terminal update cursor is outside the grid".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalLifecycle {
    Running,
    Exited,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalCell {
    pub x: u16,
    pub y: u16,
    pub width: u8,
    pub text: String,
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    pub has_explicit_bg: bool,
}

impl From<ghostty::Cell> for TerminalCell {
    fn from(cell: ghostty::Cell) -> Self {
        Self {
            x: cell.x,
            y: cell.y,
            width: cell.width,
            text: cell.text,
            fg: cell.fg,
            bg: cell.bg,
            has_explicit_bg: cell.has_explicit_bg,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TerminalLifecycle, TerminalSnapshot, TerminalUpdate, apply_terminal_update};
    use crate::TerminalSessionId;
    use std::collections::HashMap;

    #[test]
    fn a_new_full_frame_replaces_an_older_cached_snapshot() {
        let terminal_session_id = TerminalSessionId::from_u64(1);
        let mut snapshots = HashMap::from([(
            terminal_session_id,
            TerminalSnapshot::from_update(TerminalUpdate::empty(1, TerminalLifecycle::Running))
                .expect("create initial snapshot"),
        )]);

        let replacement = apply_terminal_update(
            &mut snapshots,
            terminal_session_id,
            TerminalUpdate::empty(2, TerminalLifecycle::Running),
        )
        .expect("replace cache with full frame");

        assert_eq!(replacement.revision, 2);
        assert_eq!(snapshots[&terminal_session_id].revision, 2);
    }
}
