#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtySize {
    pub rows: u16,
    pub cols: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

pub(crate) enum PtyOutput {
    Bytes(Vec<u8>),
    Paused,
}

#[derive(Clone, Copy)]
pub(crate) enum ReaderControl {
    Pause,
    Resume,
}

pub(crate) fn send_or_shutdown<T>(
    sender: &flume::Sender<T>,
    shutdown: &flume::Receiver<()>,
    value: T,
) -> bool {
    flume::Selector::new()
        .send(sender, value, |result| result.is_ok())
        .recv(shutdown, |_| false)
        .wait()
}

pub(crate) fn reader_checkpoint(
    control: &flume::Receiver<ReaderControl>,
    output: &flume::Sender<PtyOutput>,
    shutdown: &flume::Receiver<()>,
) -> bool {
    match control.try_recv() {
        Ok(ReaderControl::Pause) => {
            if !send_or_shutdown(output, shutdown, PtyOutput::Paused) {
                return false;
            }
            flume::Selector::new()
                .recv(control, |message| {
                    matches!(message, Ok(ReaderControl::Resume))
                })
                .recv(shutdown, |_| false)
                .wait()
        }
        Ok(ReaderControl::Resume) => false,
        Err(flume::TryRecvError::Empty) => true,
        Err(flume::TryRecvError::Disconnected) => false,
    }
}

#[cfg(windows)]
pub use crate::windows_pty::PtySession;

#[cfg(unix)]
mod unix {
    use super::{PtyOutput, PtySize, ReaderControl, reader_checkpoint, send_or_shutdown};
    use crate::terminal_session::TerminalEvent;
    use portable_pty::{
        Child, CommandBuilder, MasterPty, PtySize as PortablePtySize, native_pty_system,
    };
    use std::io::{Read, Write};

    pub struct PtySession {
        _master: Box<dyn MasterPty + Send>,
        writer: Box<dyn Write + Send>,
        child: Option<Box<dyn Child + Send>>,
        control: flume::Sender<ReaderControl>,
        shutdown: Option<flume::Sender<()>>,
    }

    impl PtySession {
        pub fn spawn(
            size: PtySize,
            working_directory: &std::path::Path,
            events: flume::Sender<TerminalEvent>,
        ) -> Result<(Self, flume::Receiver<PtyOutput>), String> {
            let pair = native_pty_system()
                .openpty(size.into())
                .map_err(|error| format!("open PTY: {error}"))?;

            let mut command = CommandBuilder::new(shell());
            command.cwd(working_directory);
            command.env("TERM", "xterm-256color");
            command.env("COLORTERM", "truecolor");

            let child = pair
                .slave
                .spawn_command(command)
                .map_err(|error| format!("spawn shell: {error}"))?;
            drop(pair.slave);

            let mut reader = pair
                .master
                .try_clone_reader()
                .map_err(|error| format!("clone PTY reader: {error}"))?;
            let reader_fd = pair
                .master
                .as_raw_fd()
                .ok_or_else(|| "Unix PTY does not expose a pollable file descriptor".to_string())?;
            let writer = pair
                .master
                .take_writer()
                .map_err(|error| format!("take PTY writer: {error}"))?;
            let (output_tx, output_rx) = flume::bounded(256);
            let (control_tx, control_rx) = flume::bounded(1);
            let (shutdown_tx, shutdown_rx) = flume::bounded(1);

            std::thread::Builder::new()
                .name("terminal-pty-reader".into())
                .spawn(move || {
                    let mut buffer = [0_u8; 16 * 1024];
                    loop {
                        if !reader_checkpoint(&control_rx, &output_tx, &shutdown_rx) {
                            break;
                        }
                        match wait_until_readable(reader_fd) {
                            Ok(false) => continue,
                            Ok(true) => {}
                            Err(error) => {
                                let _ = send_or_shutdown(
                                    &events,
                                    &shutdown_rx,
                                    TerminalEvent::Failed(format!("wait for PTY output: {error}")),
                                );
                                break;
                            }
                        }
                        match reader.read(&mut buffer) {
                            Ok(0) => {
                                let _ =
                                    send_or_shutdown(&events, &shutdown_rx, TerminalEvent::Exited);
                                break;
                            }
                            Ok(read)
                                if !send_or_shutdown(
                                    &output_tx,
                                    &shutdown_rx,
                                    PtyOutput::Bytes(buffer[..read].to_vec()),
                                ) =>
                            {
                                break;
                            }
                            Ok(_) if events.try_send(TerminalEvent::Changed).is_err() => {}
                            Ok(_) => {}
                            Err(error) if is_normal_pty_exit(&error) => {
                                let _ =
                                    send_or_shutdown(&events, &shutdown_rx, TerminalEvent::Exited);
                                break;
                            }
                            Err(error) => {
                                let _ = send_or_shutdown(
                                    &events,
                                    &shutdown_rx,
                                    TerminalEvent::Failed(format!("PTY read stopped: {error}")),
                                );
                                break;
                            }
                        }
                    }
                })
                .map_err(|error| format!("spawn PTY reader thread: {error}"))?;

            Ok((
                Self {
                    _master: pair.master,
                    writer,
                    child: Some(child),
                    control: control_tx,
                    shutdown: Some(shutdown_tx),
                },
                output_rx,
            ))
        }

        pub fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
            self.writer
                .write_all(bytes)
                .and_then(|_| self.writer.flush())
                .map_err(|error| format!("write PTY: {error}"))
        }

        pub fn resize(&mut self, size: PtySize) -> Result<(), String> {
            self._master
                .resize(size.into())
                .map_err(|error| format!("resize PTY: {error}"))
        }

        pub fn pause_reader(&mut self) -> Result<(), String> {
            self.control
                .send(ReaderControl::Pause)
                .map_err(|_| "pause PTY reader: reader stopped".to_string())
        }

        pub fn resume_reader(&mut self) -> Result<(), String> {
            self.control
                .send(ReaderControl::Resume)
                .map_err(|_| "resume PTY reader: reader stopped".to_string())
        }

        #[cfg(all(test, target_os = "linux"))]
        pub fn process_id(&self) -> Option<u32> {
            self.child.as_ref().and_then(|child| child.process_id())
        }

        pub fn reap(&mut self) -> Result<(), String> {
            let Some(mut child) = self.child.take() else {
                return Ok(());
            };
            child
                .wait()
                .map(|_| ())
                .map_err(|error| format!("reap terminal process: {error}"))
        }
    }

    impl Drop for PtySession {
        fn drop(&mut self) {
            drop(self.shutdown.take());
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    impl From<PtySize> for PortablePtySize {
        fn from(size: PtySize) -> Self {
            Self {
                rows: size.rows,
                cols: size.cols,
                pixel_width: size.pixel_width,
                pixel_height: size.pixel_height,
            }
        }
    }

    fn shell() -> String {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
    }

    fn is_normal_pty_exit(error: &std::io::Error) -> bool {
        #[cfg(target_os = "linux")]
        {
            error.raw_os_error() == Some(libc::EIO)
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = error;
            false
        }
    }

    fn wait_until_readable(fd: libc::c_int) -> std::io::Result<bool> {
        let mut descriptor = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        loop {
            let result = unsafe { libc::poll(&mut descriptor, 1, 10) };
            if result >= 0 {
                return Ok(result > 0);
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        #[cfg(target_os = "linux")]
        #[test]
        fn linux_eio_is_a_normal_pty_exit() {
            let error = std::io::Error::from_raw_os_error(libc::EIO);
            assert!(super::is_normal_pty_exit(&error));
        }
    }
}

#[cfg(unix)]
pub use unix::PtySession;

#[cfg(test)]
mod tests {
    #[test]
    fn shutdown_cancels_a_blocked_worker_send() {
        let (events, _event_receiver) = flume::bounded(1);
        events.send(()).expect("fill worker queue");
        let (shutdown, shutdown_receiver) = flume::bounded::<()>(1);
        drop(shutdown);

        assert!(!super::send_or_shutdown(&events, &shutdown_receiver, ()));
    }
}
