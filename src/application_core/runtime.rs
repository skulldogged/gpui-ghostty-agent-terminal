use super::{TerminalLifecycle, TerminalUpdate};
use crate::{
    AgentProgram, CoreCommand, CoreEffect, CoreModel, PaneId, PaneLayout, TerminalSessionId,
    core_model::default_space_name,
    pty::ProcessSnapshot,
    terminal_session::{TerminalEvent, TerminalEvents, TerminalSession, TerminalSize},
    terminal_theme::TerminalTheme,
};
use crate::{CoreCommit, CoreModelError, CoreSnapshot};
use std::{
    collections::HashMap,
    path::Path,
    time::{Duration, Instant},
};

const ACTIVE_WORK_POLL_INTERVAL: Duration = Duration::from_millis(100);

pub(super) struct CoreRuntime {
    model: CoreModel,
    terminals: HashMap<TerminalSessionId, RuntimeTerminal>,
    processes: ProcessSnapshot,
    next_process_refresh: Instant,
    theme: Option<TerminalTheme>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum RuntimeEvent {
    TerminalChanged {
        terminal_session_id: TerminalSessionId,
        terminal_revision: u64,
    },
    TerminalLifecycleChanged {
        terminal_session_id: TerminalSessionId,
        lifecycle: TerminalLifecycle,
        terminal_revision: u64,
    },
    PaneClosed {
        terminal_session_id: TerminalSessionId,
        revision: u64,
    },
}

struct RuntimeTerminal {
    session: Option<TerminalSession>,
    events: Option<TerminalEvents>,
    revision: u64,
    lifecycle: TerminalLifecycle,
    last_snapshot_revision: Option<u64>,
    interactive_prompt_seen: bool,
    bracketed_paste: bool,
    alternate_screen: bool,
    active_work: bool,
    agent_program: Option<AgentProgram>,
}

impl CoreRuntime {
    pub(super) fn start(working_directory: &Path, first_resource_id: u64) -> Result<Self, String> {
        let mut model = CoreModel::with_id_namespace(first_resource_id);
        let space_name = default_space_name(working_directory);
        let commit = model
            .apply(
                0,
                CoreCommand::CreateSpace {
                    name: space_name,
                    directory: working_directory.to_owned(),
                },
            )
            .map_err(|error| format!("create initial Space: {error}"))?;
        let default_terminal = commit
            .snapshot
            .terminal_sessions
            .first()
            .map(|session| session.id)
            .ok_or_else(|| "initial Space did not create a Terminal Session".to_string())?;
        let mut runtime = Self {
            model,
            terminals: HashMap::new(),
            processes: ProcessSnapshot::new(),
            next_process_refresh: Instant::now() + ACTIVE_WORK_POLL_INTERVAL,
            theme: None,
        };
        runtime.execute_effects(&commit.effects);
        let terminal = runtime
            .terminals
            .get(&default_terminal)
            .expect("initial launch effect creates a runtime entry");
        if let TerminalLifecycle::Failed(error) = &terminal.lifecycle {
            return Err(error.clone());
        }
        Ok(runtime)
    }

    #[cfg(test)]
    pub(super) fn default_terminal(&self) -> TerminalSessionId {
        self.model
            .snapshot()
            .terminal_sessions
            .first()
            .map(|session| session.id)
            .expect("a running Core has an initial Terminal Session")
    }

    pub(super) fn model_snapshot(&self) -> CoreSnapshot {
        self.model.snapshot()
    }

    pub(super) fn terminal_revision(
        &self,
        terminal_session_id: TerminalSessionId,
    ) -> Result<u64, String> {
        self.terminals
            .get(&terminal_session_id)
            .map(|terminal| terminal.revision)
            .ok_or_else(|| {
                format!(
                    "Terminal Session {} does not exist",
                    terminal_session_id.as_u64()
                )
            })
    }

    pub(super) fn contains_terminal(&self, terminal_session_id: TerminalSessionId) -> bool {
        self.terminals.contains_key(&terminal_session_id)
    }

    pub(super) fn set_theme(
        &mut self,
        theme: TerminalTheme,
    ) -> Result<Vec<(TerminalSessionId, u64)>, String> {
        let mut changes = Vec::new();
        for (&terminal_session_id, runtime) in &mut self.terminals {
            if let Some(session) = runtime.session.as_mut() {
                session.set_theme(theme)?;
                runtime.revision = runtime.revision.saturating_add(1);
                changes.push((terminal_session_id, runtime.revision));
            }
        }
        self.theme = Some(theme);
        Ok(changes)
    }

    pub(super) fn apply(
        &mut self,
        expected_revision: u64,
        command: CoreCommand,
    ) -> Result<CoreCommit, CoreModelError> {
        let commit = self.model.apply(expected_revision, command)?;
        self.execute_effects(&commit.effects);
        Ok(commit)
    }

    pub(super) fn input(
        &mut self,
        terminal_session_id: TerminalSessionId,
        bytes: &[u8],
    ) -> Result<(), String> {
        self.live_terminal_mut(terminal_session_id)?.input(bytes)
    }

    pub(super) fn paste(
        &mut self,
        terminal_session_id: TerminalSessionId,
        bytes: &[u8],
    ) -> Result<bool, String> {
        let runtime = self.runtime_terminal_mut(terminal_session_id)?;
        let session = runtime
            .session
            .as_mut()
            .ok_or_else(|| lifecycle_error(&runtime.lifecycle))?;
        let result = session.paste(bytes);
        match result {
            Ok(changed) => {
                if changed {
                    runtime.revision = runtime.revision.saturating_add(1);
                }
                Ok(changed)
            }
            Err(error) => {
                // The ordered reader barrier may have fed terminal output even
                // when encoding, writing, or resuming subsequently failed.
                runtime.revision = runtime.revision.saturating_add(1);
                Err(error)
            }
        }
    }

    pub(super) fn resize(
        &mut self,
        terminal_session_id: TerminalSessionId,
        size: TerminalSize,
    ) -> Result<bool, String> {
        let runtime = self.runtime_terminal_mut(terminal_session_id)?;
        let session = runtime
            .session
            .as_mut()
            .ok_or_else(|| lifecycle_error(&runtime.lifecycle))?;
        let size = size.validate()?;
        if size == session.size() {
            // A repeated target is still an observable resize acknowledgement. Interactive
            // resize coalescing can return to a geometry whose earlier frame was deferred by a
            // client while an intermediate target was pending, so advance the projection
            // revision and let subscribers request the current frame again.
            runtime.revision = runtime.revision.saturating_add(1);
            return Ok(true);
        }
        let result = session.resize(size);
        // The ordered resize barrier drains bytes accepted at the previous
        // geometry even when the platform resize subsequently fails.
        runtime.revision = runtime.revision.saturating_add(1);
        result.map(|()| true)
    }

    pub(super) fn snapshot(
        &mut self,
        terminal_session_id: TerminalSessionId,
        since: Option<u64>,
    ) -> Result<Option<TerminalUpdate>, String> {
        let processes = &self.processes;
        let runtime = self
            .terminals
            .get_mut(&terminal_session_id)
            .ok_or_else(|| {
                format!(
                    "Terminal Session {} does not exist",
                    terminal_session_id.as_u64()
                )
            })?;
        if since == Some(runtime.revision) {
            return Ok(None);
        }
        let Some(session) = runtime.session.as_mut() else {
            runtime.last_snapshot_revision = Some(runtime.revision);
            return Ok(Some(TerminalUpdate::empty(
                runtime.revision,
                runtime.lifecycle.clone(),
            )));
        };
        let force_full = since.is_none() || since != runtime.last_snapshot_revision;
        let snapshot = session.render_update(force_full)?;
        runtime.interactive_prompt_seen |= snapshot.bracketed_paste;
        runtime.bracketed_paste = snapshot.bracketed_paste;
        runtime.alternate_screen = snapshot.alternate_screen;
        let foreground_process = session.has_foreground_process(processes).unwrap_or(true);
        runtime.agent_program = session
            .process_id()
            .and_then(|process_id| processes.agent_program(process_id));
        runtime.active_work = close_confirmation_required(
            runtime.interactive_prompt_seen,
            runtime.bracketed_paste,
            runtime.alternate_screen,
            foreground_process,
        );
        let update = TerminalUpdate::from_terminal(
            snapshot,
            runtime.last_snapshot_revision,
            runtime.revision,
            runtime.lifecycle.clone(),
            runtime.active_work,
            runtime.agent_program,
        );
        runtime.last_snapshot_revision = Some(runtime.revision);
        Ok(Some(update))
    }

    pub(super) fn refresh(&mut self) -> Vec<RuntimeEvent> {
        let mut updates = Vec::new();
        let mut exited = Vec::new();
        let poll_active_work = self.refresh_process_snapshot_if_due();
        let processes = &self.processes;
        for (&terminal_session_id, runtime) in &mut self.terminals {
            let revision_before = runtime.revision;
            let lifecycle_before = runtime.lifecycle.clone();
            runtime.refresh(poll_active_work.then_some(processes));
            let exited_naturally = runtime.lifecycle != lifecycle_before
                && matches!(runtime.lifecycle, TerminalLifecycle::Exited);
            if runtime.lifecycle != lifecycle_before {
                if exited_naturally {
                    exited.push(terminal_session_id);
                } else {
                    updates.push(RuntimeEvent::TerminalLifecycleChanged {
                        terminal_session_id,
                        lifecycle: runtime.lifecycle.clone(),
                        terminal_revision: runtime.revision,
                    });
                }
            }
            if runtime.revision != revision_before && !exited_naturally {
                updates.push(RuntimeEvent::TerminalChanged {
                    terminal_session_id,
                    terminal_revision: runtime.revision,
                });
            }
        }
        for terminal_session_id in exited {
            let pane_id = pane_for_terminal(&self.model.snapshot(), terminal_session_id)
                .expect("a live Terminal Session must belong to one Pane");
            let commit = self
                .apply(
                    self.model.snapshot().revision,
                    CoreCommand::ClosePane { pane_id },
                )
                .expect(
                    "closing a naturally exited Terminal Session must preserve Core invariants",
                );
            updates.push(RuntimeEvent::PaneClosed {
                terminal_session_id,
                revision: commit.revision,
            });
        }
        updates
    }

    fn refresh_process_snapshot_if_due(&mut self) -> bool {
        let now = Instant::now();
        if now < self.next_process_refresh {
            return false;
        }
        self.processes.refresh();
        self.next_process_refresh = now + ACTIVE_WORK_POLL_INTERVAL;
        true
    }

    fn execute_effects(&mut self, effects: &[CoreEffect]) {
        for effect in effects {
            match effect {
                CoreEffect::LaunchTerminal {
                    terminal_session_id,
                    launch,
                } => {
                    let runtime = RuntimeTerminal::spawn(&launch.working_directory, self.theme)
                        .unwrap_or_else(RuntimeTerminal::failed);
                    self.terminals.insert(*terminal_session_id, runtime);
                }
                CoreEffect::StopTerminal {
                    terminal_session_id,
                } => {
                    self.terminals.remove(terminal_session_id);
                }
            }
        }
    }

    fn live_terminal_mut(
        &mut self,
        terminal_session_id: TerminalSessionId,
    ) -> Result<&mut TerminalSession, String> {
        let runtime = self.runtime_terminal_mut(terminal_session_id)?;
        runtime
            .session
            .as_mut()
            .ok_or_else(|| lifecycle_error(&runtime.lifecycle))
    }

    fn runtime_terminal_mut(
        &mut self,
        terminal_session_id: TerminalSessionId,
    ) -> Result<&mut RuntimeTerminal, String> {
        self.terminals.get_mut(&terminal_session_id).ok_or_else(|| {
            format!(
                "Terminal Session {} does not exist",
                terminal_session_id.as_u64()
            )
        })
    }
}

impl RuntimeTerminal {
    fn spawn(working_directory: &Path, theme: Option<TerminalTheme>) -> Result<Self, String> {
        let (mut session, events) =
            TerminalSession::spawn_in(TerminalSize::default(), working_directory)?;
        if let Some(theme) = theme {
            session.set_theme(theme)?;
        }
        Ok(Self {
            session: Some(session),
            events: Some(events),
            revision: 0,
            lifecycle: TerminalLifecycle::Running,
            last_snapshot_revision: None,
            interactive_prompt_seen: false,
            bracketed_paste: false,
            alternate_screen: false,
            active_work: false,
            agent_program: None,
        })
    }

    fn failed(error: String) -> Self {
        Self {
            session: None,
            events: None,
            revision: 1,
            lifecycle: TerminalLifecycle::Failed(error),
            last_snapshot_revision: None,
            interactive_prompt_seen: false,
            bracketed_paste: false,
            alternate_screen: false,
            active_work: false,
            agent_program: None,
        }
    }

    fn refresh(&mut self, processes: Option<&ProcessSnapshot>) {
        let (Some(session), Some(events)) = (&mut self.session, &self.events) else {
            return;
        };
        match session.drain_pending_output() {
            Ok(true) => self.revision = self.revision.saturating_add(1),
            Ok(false) => {}
            Err(error) => {
                self.lifecycle = TerminalLifecycle::Failed(error);
                self.revision = self.revision.saturating_add(1);
            }
        }
        while let Some(event) = events.try_recv() {
            match event {
                TerminalEvent::Changed => {}
                TerminalEvent::Exited => {
                    let output_error = match session.drain_pending_output() {
                        Ok(true) => {
                            self.revision = self.revision.saturating_add(1);
                            None
                        }
                        Ok(false) => None,
                        Err(error) => Some(error),
                    };
                    let reap_error = session.reap_process().err();
                    self.lifecycle = match (output_error, reap_error) {
                        (None, None) => TerminalLifecycle::Exited,
                        (Some(error), None) | (None, Some(error)) => {
                            TerminalLifecycle::Failed(error)
                        }
                        (Some(output_error), Some(reap_error)) => {
                            TerminalLifecycle::Failed(format!(
                                "{output_error}; also failed to reap terminal process: {reap_error}"
                            ))
                        }
                    };
                    self.revision = self.revision.saturating_add(1);
                }
                TerminalEvent::Failed(error) => {
                    self.lifecycle = TerminalLifecycle::Failed(error);
                    self.revision = self.revision.saturating_add(1);
                }
            }
        }
        if let Some(processes) = processes {
            let foreground_process = session.has_foreground_process(processes).unwrap_or(true);
            let agent_program = session
                .process_id()
                .and_then(|process_id| processes.agent_program(process_id));
            let active_work = close_confirmation_required(
                self.interactive_prompt_seen,
                self.bracketed_paste,
                self.alternate_screen,
                foreground_process,
            );
            if active_work != self.active_work {
                self.active_work = active_work;
                self.revision = self.revision.saturating_add(1);
            }
            if agent_program != self.agent_program {
                self.agent_program = agent_program;
                self.revision = self.revision.saturating_add(1);
            }
        }
    }
}

fn close_confirmation_required(
    interactive_prompt_seen: bool,
    bracketed_paste: bool,
    alternate_screen: bool,
    foreground_process: bool,
) -> bool {
    foreground_process || alternate_screen || (interactive_prompt_seen && !bracketed_paste)
}

fn pane_for_terminal(
    snapshot: &CoreSnapshot,
    terminal_session_id: TerminalSessionId,
) -> Option<PaneId> {
    snapshot
        .spaces
        .iter()
        .flat_map(|space| &space.tabs)
        .find_map(|tab| pane_for_terminal_in_layout(&tab.layout, terminal_session_id))
}

fn pane_for_terminal_in_layout(
    layout: &PaneLayout,
    terminal_session_id: TerminalSessionId,
) -> Option<PaneId> {
    match layout {
        PaneLayout::Pane(pane) => {
            (pane.terminal_session_id == terminal_session_id).then_some(pane.id)
        }
        PaneLayout::Split(split) => pane_for_terminal_in_layout(&split.first, terminal_session_id)
            .or_else(|| pane_for_terminal_in_layout(&split.second, terminal_session_id)),
    }
}

fn lifecycle_error(lifecycle: &TerminalLifecycle) -> String {
    match lifecycle {
        TerminalLifecycle::Running => "Terminal Session runtime is unavailable".into(),
        TerminalLifecycle::Exited => "Terminal Session has exited".into(),
        TerminalLifecycle::Failed(error) => format!("Terminal Session failed: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CreatedResource, SplitAxis, SplitPlacement, SplitRatio};
    use std::time::{Duration, Instant};

    #[test]
    fn close_confirmation_tracks_work_after_an_interactive_prompt() {
        assert!(!close_confirmation_required(false, false, false, false));
        assert!(!close_confirmation_required(true, true, false, false));
        assert!(close_confirmation_required(true, false, false, false));
        assert!(close_confirmation_required(false, true, true, false));
        assert!(close_confirmation_required(false, false, false, true));
    }

    #[test]
    fn registry_runs_multiple_terminal_sessions_in_their_space_directories() {
        let directory = std::env::current_dir().expect("current directory");
        let mut runtime = CoreRuntime::start(&directory, 1).expect("start Core runtime");
        let first = runtime.default_terminal();
        let revision = runtime.model_snapshot().revision;
        let second = runtime
            .apply(
                revision,
                CoreCommand::CreateSpace {
                    name: "Second".into(),
                    directory: directory.clone(),
                },
            )
            .expect("create second Space");
        let CreatedResource::Space {
            terminal_session_id: second,
            ..
        } = second.created
        else {
            panic!("Space creation must identify its Terminal Session");
        };

        runtime
            .input(first, b"echo FIRST_RUNTIME_SESSION\r")
            .expect("write first Terminal Session");
        runtime
            .input(second, b"echo SECOND_RUNTIME_SESSION\r")
            .expect("write second Terminal Session");

        wait_for_text(&mut runtime, first, "FIRST_RUNTIME_SESSION");
        wait_for_text(&mut runtime, second, "SECOND_RUNTIME_SESSION");
    }

    #[test]
    fn moving_a_pane_does_not_move_or_restart_its_runtime_terminal() {
        let directory = std::env::current_dir().expect("current directory");
        let mut runtime = CoreRuntime::start(&directory, 1).expect("start Core runtime");
        let snapshot = runtime.model_snapshot();
        let source_pane = match &snapshot.spaces[0].tabs[0].layout {
            crate::PaneLayout::Pane(pane) => pane.id,
            crate::PaneLayout::Split(_) => panic!("initial Tab must contain one Pane"),
        };
        let source_terminal = runtime.default_terminal();
        let destination = runtime
            .apply(
                snapshot.revision,
                CoreCommand::CreateSpace {
                    name: "Destination".into(),
                    directory,
                },
            )
            .expect("create destination Space");
        let CreatedResource::Space {
            pane_id: target_pane,
            ..
        } = destination.created
        else {
            panic!("Space creation must identify its initial Pane");
        };

        let moved = runtime
            .apply(
                destination.revision,
                CoreCommand::MovePane {
                    pane_id: source_pane,
                    target_pane_id: target_pane,
                    axis: SplitAxis::Horizontal,
                    placement: SplitPlacement::After,
                    ratio: SplitRatio::EQUAL,
                },
            )
            .expect("move Pane");

        assert!(moved.effects.is_empty());
        runtime
            .input(source_terminal, b"echo MOVED_RUNTIME_SESSION\r")
            .expect("moved Terminal Session remains live");
        wait_for_text(&mut runtime, source_terminal, "MOVED_RUNTIME_SESSION");
    }

    fn wait_for_text(runtime: &mut CoreRuntime, terminal: TerminalSessionId, expected: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            runtime.refresh();
            if runtime
                .snapshot(terminal, None)
                .expect("take Terminal Session snapshot")
                .is_some_and(|snapshot| snapshot_text(&snapshot).contains(expected))
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("Terminal Session did not render {expected:?} before timeout");
    }

    fn snapshot_text(snapshot: &TerminalUpdate) -> String {
        let mut output = String::new();
        for y in 0..snapshot.rows {
            for x in 0..snapshot.cols {
                match snapshot
                    .cells
                    .iter()
                    .find(|cell| cell.x == x && cell.y == y)
                {
                    Some(cell) if !cell.text.is_empty() => output.push_str(&cell.text),
                    _ => output.push(' '),
                }
            }
            output.push('\n');
        }
        output
    }
}
