#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtySize {
    pub rows: u16,
    pub cols: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
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

#[cfg(windows)]
pub use crate::windows_pty::PtySession;

#[cfg(unix)]
mod unix {
    use super::{PtySize, send_or_shutdown};
    use crate::terminal_session::TerminalEvent;
    use portable_pty::{
        Child, CommandBuilder, MasterPty, PtySize as PortablePtySize, native_pty_system,
    };
    use std::io::{Read, Write};

    pub struct PtySession {
        _master: Box<dyn MasterPty + Send>,
        writer: Box<dyn Write + Send>,
        child: Box<dyn Child + Send>,
        shutdown: Option<flume::Sender<()>>,
    }

    impl PtySession {
        pub fn spawn(
            size: PtySize,
            events: flume::Sender<TerminalEvent>,
        ) -> Result<(Self, flume::Receiver<Vec<u8>>), String> {
            let pair = native_pty_system()
                .openpty(size.into())
                .map_err(|error| format!("open PTY: {error}"))?;

            let mut command = CommandBuilder::new(shell());
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
            let writer = pair
                .master
                .take_writer()
                .map_err(|error| format!("take PTY writer: {error}"))?;
            let (output_tx, output_rx) = flume::bounded(256);
            let (shutdown_tx, shutdown_rx) = flume::bounded(1);

            std::thread::Builder::new()
                .name("terminal-pty-reader".into())
                .spawn(move || {
                    let mut buffer = [0_u8; 16 * 1024];
                    loop {
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
                                    buffer[..read].to_vec(),
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
                    child,
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
    }

    impl Drop for PtySession {
        fn drop(&mut self) {
            drop(self.shutdown.take());
            let _ = self.child.kill();
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
