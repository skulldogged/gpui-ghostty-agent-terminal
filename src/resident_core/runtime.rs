use super::{TerminalLifecycle, TerminalUpdate};
use crate::{
    CoreCommand, CoreEffect, CoreModel, PaneId, PaneLayout, TerminalSessionId,
    terminal_session::{TerminalEvent, TerminalEvents, TerminalSession, TerminalSize},
};
use crate::{CoreCommit, CoreModelError, CoreSnapshot};
use std::{collections::HashMap, path::Path};

pub(super) struct CoreRuntime {
    model: CoreModel,
    terminals: HashMap<TerminalSessionId, RuntimeTerminal>,
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
}

impl CoreRuntime {
    pub(super) fn start(working_directory: &Path) -> Result<Self, String> {
        let mut model = CoreModel::new();
        let space_name = working_directory
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("Terminal")
            .to_owned();
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
            return Ok(false);
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
        let runtime = self.runtime_terminal_mut(terminal_session_id)?;
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
        let update = TerminalUpdate::from_terminal(
            snapshot,
            runtime.last_snapshot_revision,
            runtime.revision,
            runtime.lifecycle.clone(),
        );
        runtime.last_snapshot_revision = Some(runtime.revision);
        Ok(Some(update))
    }

    pub(super) fn refresh(&mut self) -> Vec<RuntimeEvent> {
        let mut updates = Vec::new();
        let mut exited = Vec::new();
        for (&terminal_session_id, runtime) in &mut self.terminals {
            let revision_before = runtime.revision;
            let lifecycle_before = runtime.lifecycle.clone();
            runtime.refresh();
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

    fn execute_effects(&mut self, effects: &[CoreEffect]) {
        for effect in effects {
            match effect {
                CoreEffect::LaunchTerminal {
                    terminal_session_id,
                    launch,
                } => {
                    let runtime = RuntimeTerminal::spawn(&launch.working_directory)
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
    fn spawn(working_directory: &Path) -> Result<Self, String> {
        let (session, events) =
            TerminalSession::spawn_in(TerminalSize::default(), working_directory)?;
        Ok(Self {
            session: Some(session),
            events: Some(events),
            revision: 0,
            lifecycle: TerminalLifecycle::Running,
            last_snapshot_revision: None,
        })
    }

    fn failed(error: String) -> Self {
        Self {
            session: None,
            events: None,
            revision: 1,
            lifecycle: TerminalLifecycle::Failed(error),
            last_snapshot_revision: None,
        }
    }

    fn refresh(&mut self) {
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
    }
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
    fn registry_runs_multiple_terminal_sessions_in_their_space_directories() {
        let directory = std::env::current_dir().expect("current directory");
        let mut runtime = CoreRuntime::start(&directory).expect("start Core runtime");
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
        let mut runtime = CoreRuntime::start(&directory).expect("start Core runtime");
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
