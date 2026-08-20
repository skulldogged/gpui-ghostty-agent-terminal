use crate::{
    ghostty,
    pty::{PtyOutput, PtySession, PtySize},
};
use serde::{Deserialize, Serialize};

/// The complete geometry shared by the VT engine and the platform PTY.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width_px: u16,
    pub cell_height_px: u16,
}

impl TerminalSize {
    pub const fn new(cols: u16, rows: u16, cell_width_px: u16, cell_height_px: u16) -> Self {
        Self {
            cols,
            rows,
            cell_width_px,
            cell_height_px,
        }
    }

    pub(crate) fn validate(self) -> Result<Self, String> {
        if self.cols == 0 || self.rows == 0 {
            return Err("terminal grid must contain at least one row and column".into());
        }
        if self.cell_width_px == 0 || self.cell_height_px == 0 {
            return Err("terminal cells must have non-zero pixel dimensions".into());
        }
        if usize::from(self.cols) * usize::from(self.rows) > ghostty::SNAPSHOT_CELL_CAPACITY {
            return Err(format!(
                "terminal grid exceeds the {}-cell snapshot capacity",
                ghostty::SNAPSHOT_CELL_CAPACITY
            ));
        }
        Ok(self)
    }

    fn pty_size(self) -> PtySize {
        PtySize {
            cols: self.cols,
            rows: self.rows,
            pixel_width: self.cols.saturating_mul(self.cell_width_px),
            pixel_height: self.rows.saturating_mul(self.cell_height_px),
        }
    }
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self::new(80, 24, 10, 20)
    }
}

/// A project-owned notification stream. It deliberately carries no PTY bytes
/// or libghostty-vt types; callers only learn that observable session state
/// changed and can request a fresh snapshot.
pub struct TerminalEvents {
    receiver: flume::Receiver<TerminalEvent>,
}

impl TerminalEvents {
    pub(crate) fn try_recv(&self) -> Option<TerminalEvent> {
        self.receiver.try_recv().ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalEvent {
    Changed,
    Exited,
    Failed(String),
}

trait TerminalTransport: Send {
    fn write(&mut self, bytes: &[u8]) -> Result<(), String>;
    fn pause_reader(&mut self) -> Result<(), String>;
    fn resize(&mut self, size: PtySize) -> Result<(), String>;
    fn resume_reader(&mut self) -> Result<(), String>;
    #[cfg(all(test, target_os = "linux"))]
    fn process_id(&self) -> Option<u32> {
        None
    }
    fn reap(&mut self) -> Result<(), String> {
        Ok(())
    }
}

impl TerminalTransport for PtySession {
    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        PtySession::write(self, bytes)
    }

    fn pause_reader(&mut self) -> Result<(), String> {
        PtySession::pause_reader(self)
    }

    fn resize(&mut self, size: PtySize) -> Result<(), String> {
        PtySession::resize(self, size)
    }

    fn resume_reader(&mut self) -> Result<(), String> {
        PtySession::resume_reader(self)
    }

    #[cfg(all(test, target_os = "linux"))]
    fn process_id(&self) -> Option<u32> {
        PtySession::process_id(self)
    }

    fn reap(&mut self) -> Result<(), String> {
        PtySession::reap(self)
    }
}

/// Owns one live shell, its platform process transport, and its Ghostty VT
/// state. The UI uses this interface without knowing whether the process is
/// attached through a Unix PTY or Windows ConPTY.
pub struct TerminalSession {
    terminal: ghostty::Terminal,
    process: Box<dyn TerminalTransport>,
    output: Option<flume::Receiver<PtyOutput>>,
    #[allow(dead_code)] // Read by resize, which the next stacked PR drives from GPUI geometry.
    size: TerminalSize,
}

impl TerminalSession {
    pub fn spawn(size: TerminalSize) -> Result<(Self, TerminalEvents), String> {
        let size = size.validate()?;
        let terminal = ghostty::Terminal::new(size.cols, size.rows)?;
        let (events_tx, events_rx) = flume::bounded(1);
        let (process, output) = PtySession::spawn(size.pty_size(), events_tx)?;
        Ok((
            Self {
                terminal,
                process: Box::new(process),
                output: Some(output),
                size,
            },
            TerminalEvents {
                receiver: events_rx,
            },
        ))
    }

    pub fn input(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.process.write(bytes)
    }

    pub(crate) fn size(&self) -> TerminalSize {
        self.size
    }

    #[allow(dead_code)] // The renderer stack adds the first live geometry caller.
    pub fn resize(&mut self, size: TerminalSize) -> Result<(), String> {
        let size = size.validate()?;
        if size == self.size {
            return Ok(());
        }

        self.process.pause_reader()?;
        if let Err(error) = self.drain_until_reader_paused() {
            let resume_error = self.process.resume_reader().err();
            return Err(combine_errors(error, resume_error));
        }

        let resize_result = self.resize_while_reader_paused(size);
        let resume_error = self.process.resume_reader().err();
        match resize_result {
            Ok(()) => resume_error.map_or(Ok(()), Err),
            Err(error) => Err(combine_errors(error, resume_error)),
        }
    }

    fn resize_while_reader_paused(&mut self, size: TerminalSize) -> Result<(), String> {
        let previous_size = self.size;
        self.terminal.resize(
            size.cols,
            size.rows,
            u32::from(size.cell_width_px),
            u32::from(size.cell_height_px),
        )?;
        if let Err(process_error) = self.process.resize(size.pty_size()) {
            let rollback = self.terminal.resize(
                previous_size.cols,
                previous_size.rows,
                u32::from(previous_size.cell_width_px),
                u32::from(previous_size.cell_height_px),
            );
            return match rollback {
                Ok(()) => Err(process_error),
                Err(rollback_error) => Err(format!(
                    "{process_error}; also failed to restore terminal geometry: {rollback_error}"
                )),
            };
        }
        self.size = size;
        Ok(())
    }

    #[cfg(test)]
    pub fn snapshot(&mut self) -> Result<ghostty::Snapshot, String> {
        let _changed = self.drain_output()?;
        self.snapshot_current()
    }

    pub(crate) fn snapshot_current(&mut self) -> Result<ghostty::Snapshot, String> {
        self.terminal.snapshot()
    }

    pub(crate) fn drain_pending_output(&mut self) -> Result<bool, String> {
        self.drain_output()
    }

    pub(crate) fn reap_process(&mut self) -> Result<(), String> {
        self.process.reap()
    }

    #[cfg(all(test, target_os = "linux"))]
    fn process_id(&self) -> Option<u32> {
        self.process.process_id()
    }

    fn drain_output(&mut self) -> Result<bool, String> {
        let mut changed = false;
        loop {
            let message = self
                .output
                .as_ref()
                .expect("Terminal Session output is available before drop")
                .try_recv();
            match message {
                Ok(PtyOutput::Bytes(bytes)) => {
                    self.terminal.feed(&bytes);
                    changed = true;
                }
                Ok(PtyOutput::Paused) => {
                    return Err("terminal reader paused outside a resize barrier".into());
                }
                Err(flume::TryRecvError::Empty | flume::TryRecvError::Disconnected) => {
                    return Ok(changed);
                }
            }
        }
    }

    fn drain_until_reader_paused(&mut self) -> Result<(), String> {
        loop {
            let message = self
                .output
                .as_ref()
                .expect("Terminal Session output is available before drop")
                .recv()
                .map_err(|_| "pause terminal reader: output stream stopped".to_string())?;
            match message {
                PtyOutput::Bytes(bytes) => self.terminal.feed(&bytes),
                PtyOutput::Paused => return Ok(()),
            }
        }
    }
}

fn combine_errors(error: String, followup: Option<String>) -> String {
    followup.map_or_else(|| error.clone(), |followup| format!("{error}; {followup}"))
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        // A platform reader may be blocked while delivering a full output
        // queue. Disconnect it before dropping the process transport so it
        // can close its pipe while ConPTY or the Unix PTY shuts down.
        drop(self.output.take());
    }
}

#[cfg(test)]
mod tests {
    use super::{TerminalEvent, TerminalSession, TerminalSize, TerminalTransport};
    use crate::ghostty::snapshot_text;
    use crate::{
        ghostty,
        pty::{PtyOutput, PtySize},
    };
    use std::time::{Duration, Instant};

    struct OutputDuringResize {
        output: flume::Sender<PtyOutput>,
    }

    impl TerminalTransport for OutputDuringResize {
        fn write(&mut self, _bytes: &[u8]) -> Result<(), String> {
            Ok(())
        }

        fn pause_reader(&mut self) -> Result<(), String> {
            self.output
                .send(PtyOutput::Bytes(b"\x1b[H\x1b[999C".to_vec()))
                .and_then(|_| self.output.send(PtyOutput::Paused))
                .map_err(|error| format!("pause test reader: {error}"))
        }

        fn resize(&mut self, _size: PtySize) -> Result<(), String> {
            Ok(())
        }

        fn resume_reader(&mut self) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn terminal_size_rejects_zero_dimensions() {
        assert!(TerminalSize::new(0, 24, 10, 20).validate().is_err());
        assert!(TerminalSize::new(80, 0, 10, 20).validate().is_err());
        assert!(TerminalSize::new(80, 24, 0, 20).validate().is_err());
        assert!(TerminalSize::new(80, 24, 10, 0).validate().is_err());
    }

    #[test]
    fn terminal_size_rejects_grids_larger_than_snapshot_capacity() {
        assert!(TerminalSize::new(256, 256, 10, 20).validate().is_ok());
        assert!(TerminalSize::new(400, 200, 10, 20).validate().is_err());
    }

    #[test]
    fn pty_geometry_uses_the_same_cell_metrics() {
        let size = TerminalSize::new(100, 40, 9, 18);
        let pty = size.pty_size();
        assert_eq!(pty.cols, 100);
        assert_eq!(pty.rows, 40);
        assert_eq!(pty.pixel_width, 900);
        assert_eq!(pty.pixel_height, 720);
    }

    #[test]
    fn resize_consumes_in_flight_output_at_previous_geometry() {
        let previous_size = TerminalSize::new(4, 4, 10, 20);
        let next_size = TerminalSize::new(8, 4, 10, 20);
        let (output_tx, output_rx) = flume::unbounded();
        let terminal = ghostty::Terminal::new(previous_size.cols, previous_size.rows)
            .expect("create test terminal");
        let mut session = TerminalSession {
            terminal,
            process: Box::new(OutputDuringResize { output: output_tx }),
            output: Some(output_rx),
            size: previous_size,
        };

        session.resize(next_size).expect("resize terminal session");

        let snapshot = session.snapshot().expect("snapshot resized terminal");
        assert_eq!(
            snapshot.cursor,
            Some((previous_size.cols - 1, 0)),
            "bytes accepted before the transport resize must use the previous grid"
        );
    }

    #[test]
    fn interactive_shell_round_trips_input_through_the_session() {
        let (mut session, events) =
            TerminalSession::spawn(TerminalSize::default()).expect("spawn terminal session");
        session
            .input(b"echo TERMINAL_SESSION_RUNTIME_LIVE\r")
            .expect("write terminal input");

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            match events.receiver.recv_timeout(Duration::from_millis(250)) {
                Ok(TerminalEvent::Changed) => {
                    let snapshot = session.snapshot().expect("snapshot terminal session");
                    if snapshot_text(&snapshot).contains("TERMINAL_SESSION_RUNTIME_LIVE") {
                        return;
                    }
                }
                Ok(TerminalEvent::Exited) => panic!("shell exited before returning input"),
                Ok(TerminalEvent::Failed(error)) => panic!("terminal session failed: {error}"),
                Err(flume::RecvTimeoutError::Timeout) => {}
                Err(flume::RecvTimeoutError::Disconnected) => {
                    panic!("terminal event stream disconnected")
                }
            }
        }

        panic!("shell did not return the input marker before the timeout");
    }

    #[test]
    fn normal_shell_exit_is_reported_as_exited() {
        let (mut session, events) =
            TerminalSession::spawn(TerminalSize::default()).expect("spawn terminal session");
        #[cfg(target_os = "linux")]
        let process_id = session.process_id();
        session
            .input(b"echo TERMINAL_SESSION_EXIT_OUTPUT; exit\r")
            .expect("write shell exit command");

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            match events.receiver.recv_timeout(Duration::from_millis(250)) {
                Ok(TerminalEvent::Changed) => {
                    session.snapshot().expect("drain terminal output");
                }
                Ok(TerminalEvent::Exited) => {
                    let snapshot = session.snapshot().expect("drain final terminal output");
                    session.reap_process().expect("reap exited shell");
                    assert!(
                        snapshot_text(&snapshot).contains("TERMINAL_SESSION_EXIT_OUTPUT"),
                        "shell exit was reported before final output was available"
                    );
                    #[cfg(target_os = "linux")]
                    if let Some(process_id) = process_id {
                        assert!(
                            !std::path::Path::new(&format!("/proc/{process_id}")).exists(),
                            "reaped shell must not remain as a zombie"
                        );
                    }
                    return;
                }
                Ok(TerminalEvent::Failed(error)) => {
                    panic!("normal shell exit was reported as failure: {error}")
                }
                Err(flume::RecvTimeoutError::Timeout) => {}
                Err(flume::RecvTimeoutError::Disconnected) => {
                    panic!("terminal event stream disconnected")
                }
            }
        }

        panic!("shell exit was not reported before the timeout");
    }
}
