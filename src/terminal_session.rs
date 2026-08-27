use crate::{
    ghostty,
    pty::{ProcessSnapshot, PtyOutput, PtySession, PtySize},
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
    fn has_foreground_process(&self, processes: &ProcessSnapshot) -> Result<bool, String>;
    fn pause_reader(&mut self) -> Result<(), String>;
    fn resize(&mut self, size: PtySize) -> Result<(), String>;
    fn resume_reader(&mut self) -> Result<(), String>;
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

    fn has_foreground_process(&self, processes: &ProcessSnapshot) -> Result<bool, String> {
        PtySession::has_foreground_process(self, processes)
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
    size: TerminalSize,
}

impl TerminalSession {
    #[cfg(test)]
    pub fn spawn(size: TerminalSize) -> Result<(Self, TerminalEvents), String> {
        let working_directory = std::env::current_dir()
            .map_err(|error| format!("resolve terminal working directory: {error}"))?;
        Self::spawn_in(size, &working_directory)
    }

    pub(crate) fn spawn_in(
        size: TerminalSize,
        working_directory: &std::path::Path,
    ) -> Result<(Self, TerminalEvents), String> {
        let size = size.validate()?;
        let terminal = ghostty::Terminal::new(size.cols, size.rows)?;
        let (events_tx, events_rx) = flume::bounded(1);
        let (process, output) = PtySession::spawn(size.pty_size(), working_directory, events_tx)?;
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

    pub fn input(&mut self, bytes: &[u8]) -> Result<bool, String> {
        let viewport_changed = self.terminal.scroll_to_bottom()?;
        self.process.write(bytes)?;
        Ok(viewport_changed)
    }

    pub(crate) fn scroll(&mut self, input: ghostty::ScrollInput) -> Result<bool, String> {
        self.process.pause_reader()?;
        let mut changed = match self.drain_until_reader_paused() {
            Ok(changed) => changed,
            Err(error) => {
                let resume_error = self.process.resume_reader().err();
                return Err(combine_errors(error, resume_error));
            }
        };

        let scroll_result = self.terminal.scroll(input).and_then(|result| {
            changed |= result.viewport_changed;
            if !result.input.is_empty() {
                self.process.write(&result.input)?;
            }
            Ok(())
        });
        let resume_error = self.process.resume_reader().err();
        match scroll_result {
            Ok(()) => resume_error.map_or(Ok(changed), Err),
            Err(error) => Err(combine_errors(error, resume_error)),
        }
    }

    pub(crate) fn selection_event(
        &mut self,
        input: ghostty::SelectionInput,
    ) -> Result<bool, String> {
        let _output_changed = self.drain_output()?;
        self.terminal.selection_event(input)?;
        Ok(true)
    }

    pub fn paste(&mut self, bytes: &[u8]) -> Result<bool, String> {
        self.process.pause_reader()?;
        let mut changed = match self.drain_until_reader_paused() {
            Ok(changed) => changed,
            Err(error) => {
                let resume_error = self.process.resume_reader().err();
                return Err(combine_errors(error, resume_error));
            }
        };

        let paste_result = self
            .terminal
            .scroll_to_bottom()
            .and_then(|viewport_changed| {
                changed |= viewport_changed;
                self.terminal
                    .encode_paste(bytes)
                    .and_then(|encoded| self.process.write(&encoded))
            });
        let resume_error = self.process.resume_reader().err();
        match paste_result {
            Ok(()) => resume_error.map_or(Ok(changed), Err),
            Err(error) => Err(combine_errors(error, resume_error)),
        }
    }

    pub(crate) fn size(&self) -> TerminalSize {
        self.size
    }

    pub(crate) fn set_theme(
        &mut self,
        theme: crate::terminal_theme::TerminalTheme,
    ) -> Result<(), String> {
        self.terminal.set_theme(theme)
    }

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
        if let Err(error) = self.resize_terminal_state(size) {
            let rollback_error = self.resize_terminal_state(previous_size).err();
            return Err(combine_errors(error, rollback_error));
        }
        if let Err(process_error) = self.process.resize(size.pty_size()) {
            let rollback = self.resize_terminal_state(previous_size);
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

    fn resize_terminal_state(&mut self, size: TerminalSize) -> Result<(), String> {
        let response = self.terminal.resize(
            size.cols,
            size.rows,
            u32::from(size.cell_width_px),
            u32::from(size.cell_height_px),
        )?;
        self.write_terminal_response(&response)
    }

    #[cfg(test)]
    pub fn snapshot(&mut self) -> Result<ghostty::Snapshot, String> {
        let _changed = self.drain_output()?;
        self.render_update(true)
    }

    pub(crate) fn render_update(&mut self, force_full: bool) -> Result<ghostty::Snapshot, String> {
        self.terminal.render_update(force_full)
    }

    pub(crate) fn drain_pending_output(&mut self) -> Result<bool, String> {
        self.drain_output()
    }

    pub(crate) fn reap_process(&mut self) -> Result<(), String> {
        self.process.reap()
    }

    pub(crate) fn has_foreground_process(
        &self,
        processes: &ProcessSnapshot,
    ) -> Result<bool, String> {
        self.process.has_foreground_process(processes)
    }

    pub(crate) fn process_id(&self) -> Option<u32> {
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
                    self.feed_process_output(&bytes)?;
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

    fn drain_until_reader_paused(&mut self) -> Result<bool, String> {
        let mut changed = false;
        loop {
            let message = self
                .output
                .as_ref()
                .expect("Terminal Session output is available before drop")
                .recv()
                .map_err(|_| "pause terminal reader: output stream stopped".to_string())?;
            match message {
                PtyOutput::Bytes(bytes) => {
                    self.feed_process_output(&bytes)?;
                    changed = true;
                }
                PtyOutput::Paused => return Ok(changed),
            }
        }
    }

    fn feed_process_output(&mut self, bytes: &[u8]) -> Result<(), String> {
        let response = self.terminal.feed(bytes)?;
        self.write_terminal_response(&response)
    }

    fn write_terminal_response(&mut self, response: &[u8]) -> Result<(), String> {
        if response.is_empty() {
            return Ok(());
        }
        self.process
            .write(response)
            .map_err(|error| format!("write terminal query response: {error}"))
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
        pty::{ProcessSnapshot, PtyOutput, PtySize},
    };
    use std::{
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };

    struct OutputDuringResize {
        output: flume::Sender<PtyOutput>,
    }

    struct OutputBeforeCommand {
        output: flume::Sender<PtyOutput>,
        output_before_pause: Vec<u8>,
        bytes: Arc<Mutex<Vec<u8>>>,
        resumed: Arc<Mutex<bool>>,
        fail_write: bool,
    }

    struct RecordingTransport {
        bytes: Arc<Mutex<Vec<u8>>>,
        output: flume::Sender<PtyOutput>,
    }

    impl TerminalTransport for RecordingTransport {
        fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
            self.bytes
                .lock()
                .expect("recording transport mutex poisoned")
                .extend_from_slice(bytes);
            Ok(())
        }

        fn has_foreground_process(&self, _processes: &ProcessSnapshot) -> Result<bool, String> {
            Ok(false)
        }

        fn pause_reader(&mut self) -> Result<(), String> {
            self.output
                .send(PtyOutput::Paused)
                .map_err(|error| format!("pause test reader: {error}"))
        }

        fn resize(&mut self, _size: PtySize) -> Result<(), String> {
            Ok(())
        }

        fn resume_reader(&mut self) -> Result<(), String> {
            Ok(())
        }
    }

    impl TerminalTransport for OutputDuringResize {
        fn write(&mut self, _bytes: &[u8]) -> Result<(), String> {
            Ok(())
        }

        fn has_foreground_process(&self, _processes: &ProcessSnapshot) -> Result<bool, String> {
            Ok(false)
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

    impl TerminalTransport for OutputBeforeCommand {
        fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
            if self.fail_write {
                return Err("injected paste write failure".into());
            }
            self.bytes
                .lock()
                .expect("recording transport mutex poisoned")
                .extend_from_slice(bytes);
            Ok(())
        }

        fn has_foreground_process(&self, _processes: &ProcessSnapshot) -> Result<bool, String> {
            Ok(false)
        }

        fn pause_reader(&mut self) -> Result<(), String> {
            self.output
                .send(PtyOutput::Bytes(self.output_before_pause.clone()))
                .and_then(|_| self.output.send(PtyOutput::Paused))
                .map_err(|error| format!("pause test reader: {error}"))
        }

        fn resize(&mut self, _size: PtySize) -> Result<(), String> {
            Ok(())
        }

        fn resume_reader(&mut self) -> Result<(), String> {
            *self.resumed.lock().expect("resume marker mutex poisoned") = true;
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
    fn terminal_query_responses_return_to_the_process_transport() {
        let size = TerminalSize::default();
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let (output_tx, output_rx) = flume::unbounded();
        let terminal = ghostty::Terminal::new(size.cols, size.rows).expect("create test terminal");
        let mut session = TerminalSession {
            terminal,
            process: Box::new(RecordingTransport {
                bytes: Arc::clone(&bytes),
                output: output_tx.clone(),
            }),
            output: Some(output_rx),
            size,
        };

        output_tx
            .send(PtyOutput::Bytes(b"\x1b[0c".to_vec()))
            .expect("send primary device attributes query");
        session.snapshot().expect("process terminal query");

        assert_eq!(
            bytes
                .lock()
                .expect("recording transport mutex poisoned")
                .as_slice(),
            b"\x1b[?62;22c"
        );
    }

    #[test]
    fn resize_reports_return_to_processes_using_in_band_size_reports() {
        let size = TerminalSize::default();
        let next_size = TerminalSize::new(100, 40, 9, 18);
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let (output_tx, output_rx) = flume::unbounded();
        let terminal = ghostty::Terminal::new(size.cols, size.rows).expect("create test terminal");
        let mut session = TerminalSession {
            terminal,
            process: Box::new(RecordingTransport {
                bytes: Arc::clone(&bytes),
                output: output_tx.clone(),
            }),
            output: Some(output_rx),
            size,
        };

        output_tx
            .send(PtyOutput::Bytes(b"\x1b[?2048h".to_vec()))
            .expect("enable in-band size reports");
        session.snapshot().expect("process terminal mode");
        session.resize(next_size).expect("resize terminal session");

        assert_eq!(
            bytes
                .lock()
                .expect("recording transport mutex poisoned")
                .as_slice(),
            b"\x1b[48;40;100;720;900t"
        );
    }

    #[test]
    fn input_restores_a_scrolled_viewport_before_writing() {
        let size = TerminalSize::new(8, 3, 10, 20);
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let (output_tx, output_rx) = flume::unbounded();
        let mut terminal =
            ghostty::Terminal::new(size.cols, size.rows).expect("create test terminal");
        terminal
            .feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfive")
            .expect("feed terminal output");
        assert!(
            terminal
                .scroll(ghostty::ScrollInput {
                    delta_rows: -2,
                    delta_columns: 0,
                    pointer_x: 0.0,
                    pointer_y: 0.0,
                    viewport_width: 80,
                    viewport_height: 60,
                    cell_width: 10,
                    cell_height: 20,
                    modifiers: 0,
                })
                .expect("scroll terminal")
                .viewport_changed
        );
        let mut session = TerminalSession {
            terminal,
            process: Box::new(RecordingTransport {
                bytes: Arc::clone(&bytes),
                output: output_tx,
            }),
            output: Some(output_rx),
            size,
        };

        assert!(session.input(b"x").expect("write terminal input"));

        let snapshot = session.snapshot().expect("snapshot restored viewport");
        assert!(ghostty::snapshot_text(&snapshot).contains("five"));
        assert_eq!(
            bytes
                .lock()
                .expect("recording transport mutex poisoned")
                .as_slice(),
            b"x"
        );
    }

    #[test]
    fn paste_writes_ghostty_encoded_unicode_and_multiline_bytes_once() {
        let size = TerminalSize::default();
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let (output_tx, output_rx) = flume::unbounded();
        let mut terminal =
            ghostty::Terminal::new(size.cols, size.rows).expect("create test terminal");
        terminal
            .feed(b"\x1b[?2004h")
            .expect("enable bracketed paste");
        let mut session = TerminalSession {
            terminal,
            process: Box::new(RecordingTransport {
                bytes: Arc::clone(&bytes),
                output: output_tx,
            }),
            output: Some(output_rx),
            size,
        };

        session
            .paste("first 雪\nsecond".as_bytes())
            .expect("paste through terminal session");

        assert_eq!(
            bytes
                .lock()
                .expect("recording transport mutex poisoned")
                .as_slice(),
            b"\x1b[200~first \xe9\x9b\xaa\nsecond\x1b[201~"
        );
    }

    #[test]
    fn paste_restores_a_scrolled_viewport_before_writing() {
        let size = TerminalSize::new(8, 3, 10, 20);
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let (output_tx, output_rx) = flume::unbounded();
        let mut terminal =
            ghostty::Terminal::new(size.cols, size.rows).expect("create test terminal");
        terminal
            .feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfive")
            .expect("feed terminal output");
        assert!(
            terminal
                .scroll(ghostty::ScrollInput {
                    delta_rows: -2,
                    delta_columns: 0,
                    pointer_x: 0.0,
                    pointer_y: 0.0,
                    viewport_width: 80,
                    viewport_height: 60,
                    cell_width: 10,
                    cell_height: 20,
                    modifiers: 0,
                })
                .expect("scroll terminal")
                .viewport_changed
        );
        let mut session = TerminalSession {
            terminal,
            process: Box::new(RecordingTransport {
                bytes: Arc::clone(&bytes),
                output: output_tx,
            }),
            output: Some(output_rx),
            size,
        };

        assert!(session.paste(b"x").expect("paste terminal input"));

        let snapshot = session.snapshot().expect("snapshot restored viewport");
        assert!(ghostty::snapshot_text(&snapshot).contains("five"));
        assert_eq!(
            bytes
                .lock()
                .expect("recording transport mutex poisoned")
                .as_slice(),
            b"x"
        );
    }

    #[test]
    fn paste_consumes_output_accepted_before_the_reader_barrier() {
        let size = TerminalSize::default();
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let resumed = Arc::new(Mutex::new(false));
        let (output_tx, output_rx) = flume::unbounded();
        let terminal = ghostty::Terminal::new(size.cols, size.rows)
            .expect("create test terminal without bracketed paste enabled");
        let mut session = TerminalSession {
            terminal,
            process: Box::new(OutputBeforeCommand {
                output: output_tx,
                output_before_pause: b"\x1b[?2004h".to_vec(),
                bytes: Arc::clone(&bytes),
                resumed: Arc::clone(&resumed),
                fail_write: false,
            }),
            output: Some(output_rx),
            size,
        };

        session
            .paste(b"first\nsecond")
            .expect("paste through ordered terminal session barrier");

        assert_eq!(
            bytes
                .lock()
                .expect("recording transport mutex poisoned")
                .as_slice(),
            b"\x1b[200~first\nsecond\x1b[201~",
            "paste encoding must observe preceding PTY mode changes"
        );
        assert!(
            *resumed.lock().expect("resume marker mutex poisoned"),
            "the reader must resume after an ordered paste"
        );
    }

    #[test]
    fn scroll_consumes_mode_changes_accepted_before_the_reader_barrier() {
        let size = TerminalSize::default();
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let resumed = Arc::new(Mutex::new(false));
        let (output_tx, output_rx) = flume::unbounded();
        let terminal = ghostty::Terminal::new(size.cols, size.rows)
            .expect("create test terminal on the primary screen");
        let mut session = TerminalSession {
            terminal,
            process: Box::new(OutputBeforeCommand {
                output: output_tx,
                output_before_pause: b"\x1b[?1049h".to_vec(),
                bytes: Arc::clone(&bytes),
                resumed: Arc::clone(&resumed),
                fail_write: false,
            }),
            output: Some(output_rx),
            size,
        };

        assert!(
            session
                .scroll(ghostty::ScrollInput {
                    delta_rows: -1,
                    delta_columns: 0,
                    pointer_x: 0.0,
                    pointer_y: 0.0,
                    viewport_width: 800,
                    viewport_height: 480,
                    cell_width: 10,
                    cell_height: 20,
                    modifiers: 0,
                })
                .expect("scroll through ordered terminal session barrier")
        );

        assert_eq!(
            bytes
                .lock()
                .expect("recording transport mutex poisoned")
                .as_slice(),
            b"\x1b[A",
            "scroll routing must observe preceding alternate-screen mode changes"
        );
        assert!(
            *resumed.lock().expect("resume marker mutex poisoned"),
            "the reader must resume after an ordered scroll"
        );
    }

    #[test]
    fn paste_resumes_the_reader_after_a_write_failure() {
        let size = TerminalSize::default();
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let resumed = Arc::new(Mutex::new(false));
        let (output_tx, output_rx) = flume::unbounded();
        let terminal = ghostty::Terminal::new(size.cols, size.rows)
            .expect("create test terminal without bracketed paste enabled");
        let mut session = TerminalSession {
            terminal,
            process: Box::new(OutputBeforeCommand {
                output: output_tx,
                output_before_pause: b"\x1b[?2004h".to_vec(),
                bytes,
                resumed: Arc::clone(&resumed),
                fail_write: true,
            }),
            output: Some(output_rx),
            size,
        };

        assert!(session.paste(b"payload").is_err());
        assert!(
            *resumed.lock().expect("resume marker mutex poisoned"),
            "a failed paste must not leave the PTY reader paused"
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

    #[cfg(windows)]
    #[test]
    fn control_c_interrupts_a_windows_conpty_foreground_process() {
        let (mut session, events) =
            TerminalSession::spawn(TerminalSize::default()).expect("spawn terminal session");
        session
            .input(b"ping -t 127.0.0.1\r")
            .expect("start foreground ping process");

        // Give the shell time to start ping. The command's own echo is not a
        // sufficient readiness signal because it precedes process creation.
        std::thread::sleep(Duration::from_secs(1));
        session.input(&[0x03]).expect("send Ctrl+C through ConPTY");
        // Console control handlers run asynchronously. Do not let the
        // foreground process consume the marker before it handles Ctrl+C.
        std::thread::sleep(Duration::from_secs(1));
        session
            .input(b"cmd /d /c echo CONPTY_^CONTROL_C_RETURNED\r")
            .expect("write marker after Ctrl+C");

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut last_screen = String::new();
        while Instant::now() < deadline {
            match events.receiver.recv_timeout(Duration::from_millis(250)) {
                Ok(TerminalEvent::Changed) => {
                    let snapshot = session.snapshot().expect("snapshot terminal session");
                    last_screen = snapshot_text(&snapshot);
                    if last_screen.contains("CONPTY_CONTROL_C_RETURNED") {
                        return;
                    }
                }
                Ok(TerminalEvent::Exited) => {
                    panic!("shell exited instead of returning after Ctrl+C")
                }
                Ok(TerminalEvent::Failed(error)) => panic!("terminal session failed: {error}"),
                Err(flume::RecvTimeoutError::Timeout) => {}
                Err(flume::RecvTimeoutError::Disconnected) => {
                    panic!("terminal event stream disconnected")
                }
            }
        }

        panic!(
            "Ctrl+C did not return control from ping to the Windows shell; final screen:\n{last_screen}"
        );
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
