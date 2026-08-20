use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};

const COLS: u16 = 80;
const ROWS: u16 = 24;

pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send>,
}

impl PtySession {
    pub fn spawn() -> Result<(Self, flume::Receiver<Vec<u8>>), String> {
        let pair = native_pty_system()
            .openpty(size(COLS, ROWS))
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

        std::thread::Builder::new()
            .name("foundation-spike-pty-reader".into())
            .spawn(move || {
                let mut buffer = [0_u8; 16 * 1024];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(read) if output_tx.send(buffer[..read].to_vec()).is_err() => break,
                        Ok(_) => {}
                        Err(error) => {
                            eprintln!("PTY read stopped: {error}");
                            break;
                        }
                    }
                }
            })
            .map_err(|error| format!("spawn PTY reader thread: {error}"))?;

        Ok((
            Self {
                master: pair.master,
                writer,
                child,
            },
            output_rx,
        ))
    }

    pub fn write(&mut self, bytes: &[u8]) {
        if let Err(error) = self.writer.write_all(bytes).and_then(|_| self.writer.flush()) {
            eprintln!("PTY write failed: {error}");
        }
    }

    #[allow(dead_code)]
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<(), String> {
        self.master
            .resize(size(cols, rows))
            .map_err(|error| format!("resize PTY: {error}"))
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        cols,
        rows,
        pixel_width: cols.saturating_mul(9),
        pixel_height: rows.saturating_mul(18),
    }
}

fn shell() -> String {
    #[cfg(windows)]
    return std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());

    #[cfg(not(windows))]
    return std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
}
