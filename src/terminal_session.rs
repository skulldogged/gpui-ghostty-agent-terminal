use crate::{
    ghostty,
    pty::{PtySession, PtySize},
};

/// The complete geometry shared by the VT engine and the platform PTY.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

    fn validate(self) -> Result<Self, String> {
        if self.cols == 0 || self.rows == 0 {
            return Err("terminal grid must contain at least one row and column".into());
        }
        if self.cell_width_px == 0 || self.cell_height_px == 0 {
            return Err("terminal cells must have non-zero pixel dimensions".into());
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
    pub async fn recv(&self) -> Option<TerminalEvent> {
        self.receiver.recv_async().await.ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalEvent {
    Changed,
    Exited,
    Failed(String),
}

/// Owns one live shell, its platform process transport, and its Ghostty VT
/// state. The UI uses this interface without knowing whether the process is
/// attached through a Unix PTY or Windows ConPTY.
pub struct TerminalSession {
    terminal: ghostty::Terminal,
    process: PtySession,
    output: Option<flume::Receiver<Vec<u8>>>,
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
                process,
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

    #[allow(dead_code)] // The renderer stack adds the first live geometry caller.
    pub fn resize(&mut self, size: TerminalSize) -> Result<(), String> {
        let size = size.validate()?;
        if size == self.size {
            return Ok(());
        }

        // Resize the terminal model first. If the process resize fails, the
        // caller receives the error and can retry the complete operation.
        self.terminal.resize(
            size.cols,
            size.rows,
            u32::from(size.cell_width_px),
            u32::from(size.cell_height_px),
        )?;
        self.process.resize(size.pty_size())?;
        self.size = size;
        Ok(())
    }

    pub fn snapshot(&mut self) -> Result<ghostty::Snapshot, String> {
        let output = self
            .output
            .as_ref()
            .expect("Terminal Session output is available before drop");
        while let Ok(bytes) = output.try_recv() {
            self.terminal.feed(&bytes);
        }
        self.terminal.snapshot()
    }
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
    use super::{TerminalEvent, TerminalSession, TerminalSize};
    use crate::ghostty::snapshot_text;
    use std::time::{Duration, Instant};

    #[test]
    fn terminal_size_rejects_zero_dimensions() {
        assert!(TerminalSize::new(0, 24, 10, 20).validate().is_err());
        assert!(TerminalSize::new(80, 0, 10, 20).validate().is_err());
        assert!(TerminalSize::new(80, 24, 0, 20).validate().is_err());
        assert!(TerminalSize::new(80, 24, 10, 0).validate().is_err());
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
        session.input(b"exit\r").expect("write shell exit command");

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            match events.receiver.recv_timeout(Duration::from_millis(250)) {
                Ok(TerminalEvent::Changed) => {
                    session.snapshot().expect("drain terminal output");
                }
                Ok(TerminalEvent::Exited) => return,
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
