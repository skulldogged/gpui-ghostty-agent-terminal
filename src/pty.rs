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

pub(crate) const PROCESS_INPUT_QUEUE_CAPACITY: usize = 64;

pub(crate) fn enqueue_process_input(
    input: &flume::Sender<Vec<u8>>,
    bytes: &[u8],
    transport: &str,
) -> Result<(), String> {
    input.try_send(bytes.to_vec()).map_err(|error| match error {
        flume::TrySendError::Full(_) => format!("{transport} input queue is full"),
        flume::TrySendError::Disconnected(_) => format!("{transport} input writer stopped"),
    })
}

pub(crate) fn receive_or_shutdown<T>(
    receiver: &flume::Receiver<T>,
    shutdown: &flume::Receiver<()>,
) -> Option<T> {
    flume::Selector::new()
        .recv(receiver, |message| message.ok())
        .recv(shutdown, |_| None)
        .wait()
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
    drain_pending: impl FnOnce() -> bool,
) -> bool {
    match control.try_recv() {
        Ok(ReaderControl::Pause) => {
            // The marker is an ordering guarantee, not merely an acknowledgement:
            // every byte already readable from the platform PTY must be published first.
            if !drain_pending() {
                return false;
            }
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

pub(crate) struct ProcessSnapshot {
    system: sysinfo::System,
}

impl ProcessSnapshot {
    pub(crate) fn new() -> Self {
        let mut snapshot = Self {
            system: sysinfo::System::new(),
        };
        snapshot.refresh();
        snapshot
    }

    pub(crate) fn refresh(&mut self) {
        self.system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::All,
            sysinfo::ProcessRefreshKind::new()
                .with_cmd(sysinfo::UpdateKind::OnlyIfNotSet)
                .with_exe(sysinfo::UpdateKind::OnlyIfNotSet),
        );
    }

    pub(crate) fn has_child_process(&self, process_id: u32) -> bool {
        let process_id = sysinfo::Pid::from_u32(process_id);
        self.system
            .processes()
            .values()
            .any(|process| process.parent() == Some(process_id))
    }

    pub(crate) fn agent_program(
        &self,
        shell_process_id: u32,
    ) -> Option<crate::agent_integration::AgentProgram> {
        let shell_process_id = sysinfo::Pid::from_u32(shell_process_id);
        if let Some(process) = self.system.process(shell_process_id)
            && let Some(agent) = agent_program_for_process(process)
        {
            return Some(agent);
        }
        let mut frontier = vec![shell_process_id];
        let mut visited = std::collections::HashSet::from([shell_process_id]);

        while !frontier.is_empty() {
            let mut next = Vec::new();
            for parent in frontier {
                for (&pid, process) in self
                    .system
                    .processes()
                    .iter()
                    .filter(|(_, process)| process.parent() == Some(parent))
                {
                    if !visited.insert(pid) {
                        continue;
                    }
                    if let Some(agent) = agent_program_for_process(process) {
                        return Some(agent);
                    }
                    next.push(pid);
                }
            }
            frontier = next;
        }
        None
    }
}

fn agent_program_for_process(
    process: &sysinfo::Process,
) -> Option<crate::agent_integration::AgentProgram> {
    let command = process
        .cmd()
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    crate::agent_integration::AgentProgram::from_process(
        &process.name().to_string_lossy(),
        &command,
    )
}

#[cfg(windows)]
pub use crate::windows_pty::PtySession;

#[cfg(unix)]
mod unix {
    use super::{
        PROCESS_INPUT_QUEUE_CAPACITY, ProcessSnapshot, PtyOutput, PtySize, ReaderControl,
        enqueue_process_input, reader_checkpoint, receive_or_shutdown, send_or_shutdown,
    };
    use crate::terminal_session::TerminalEvent;
    use portable_pty::{
        Child, CommandBuilder, MasterPty, PtySize as PortablePtySize, native_pty_system,
    };
    use std::io::{Read, Write};

    pub struct PtySession {
        _master: Box<dyn MasterPty + Send>,
        input: flume::Sender<Vec<u8>>,
        child: Option<Box<dyn Child + Send>>,
        shell_process_id: Option<u32>,
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
            let shell_process_id = child.process_id();
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
            let (input_tx, input_rx) = flume::bounded::<Vec<u8>>(PROCESS_INPUT_QUEUE_CAPACITY);

            let writer_events = events.clone();
            let writer_shutdown = shutdown_rx.clone();
            std::thread::Builder::new()
                .name("terminal-pty-writer".into())
                .spawn(move || {
                    let mut writer = writer;
                    while let Some(bytes) = receive_or_shutdown(&input_rx, &writer_shutdown) {
                        if let Err(error) = writer.write_all(&bytes).and_then(|_| writer.flush()) {
                            let _ = send_or_shutdown(
                                &writer_events,
                                &writer_shutdown,
                                TerminalEvent::Failed(format!("write PTY: {error}")),
                            );
                            break;
                        }
                    }
                })
                .map_err(|error| format!("spawn PTY writer thread: {error}"))?;

            std::thread::Builder::new()
                .name("terminal-pty-reader".into())
                .spawn(move || {
                    let mut buffer = [0_u8; 16 * 1024];
                    loop {
                        if !reader_checkpoint(&control_rx, &output_tx, &shutdown_rx, || {
                            drain_ready_output(
                                reader_fd,
                                &mut reader,
                                &mut buffer,
                                &output_tx,
                                &events,
                                &shutdown_rx,
                            )
                        }) {
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
                        if !read_and_publish(
                            &mut reader,
                            &mut buffer,
                            &output_tx,
                            &events,
                            &shutdown_rx,
                        ) {
                            break;
                        }
                    }
                })
                .map_err(|error| format!("spawn PTY reader thread: {error}"))?;

            Ok((
                Self {
                    _master: pair.master,
                    input: input_tx,
                    child: Some(child),
                    shell_process_id,
                    control: control_tx,
                    shutdown: Some(shutdown_tx),
                },
                output_rx,
            ))
        }

        pub fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
            enqueue_process_input(&self.input, bytes, "PTY")
        }

        pub fn resize(&mut self, size: PtySize) -> Result<(), String> {
            self._master
                .resize(size.into())
                .map_err(|error| format!("resize PTY: {error}"))
        }

        pub fn has_foreground_process(&self, processes: &ProcessSnapshot) -> Result<bool, String> {
            let shell_process_id = self
                .shell_process_id
                .ok_or_else(|| "terminal shell does not expose its process ID".to_string())?;
            let foreground_process_group = self
                ._master
                .process_group_leader()
                .ok_or_else(|| "inspect foreground PTY process group".to_string())?;
            let foreground_process_group = u32::try_from(foreground_process_group)
                .map_err(|_| "foreground PTY process group is invalid".to_string())?;
            Ok(foreground_process_group != shell_process_id
                || processes.has_child_process(shell_process_id))
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

    fn drain_ready_output(
        reader_fd: libc::c_int,
        reader: &mut dyn Read,
        buffer: &mut [u8],
        output: &flume::Sender<PtyOutput>,
        events: &flume::Sender<TerminalEvent>,
        shutdown: &flume::Receiver<()>,
    ) -> bool {
        loop {
            match poll_readable(reader_fd, 0) {
                Ok(false) => return true,
                Ok(true) => {}
                Err(error) => {
                    let _ = send_or_shutdown(
                        events,
                        shutdown,
                        TerminalEvent::Failed(format!("wait for PTY output barrier: {error}")),
                    );
                    return false;
                }
            }
            if !read_and_publish(reader, buffer, output, events, shutdown) {
                return false;
            }
        }
    }

    fn read_and_publish(
        reader: &mut dyn Read,
        buffer: &mut [u8],
        output: &flume::Sender<PtyOutput>,
        events: &flume::Sender<TerminalEvent>,
        shutdown: &flume::Receiver<()>,
    ) -> bool {
        match reader.read(buffer) {
            Ok(0) => {
                let _ = send_or_shutdown(events, shutdown, TerminalEvent::Exited);
                false
            }
            Ok(read)
                if !send_or_shutdown(
                    output,
                    shutdown,
                    PtyOutput::Bytes(buffer[..read].to_vec()),
                ) =>
            {
                false
            }
            Ok(_) if events.try_send(TerminalEvent::Changed).is_err() => true,
            Ok(_) => true,
            Err(error) if is_normal_pty_exit(&error) => {
                let _ = send_or_shutdown(events, shutdown, TerminalEvent::Exited);
                false
            }
            Err(error) => {
                let _ = send_or_shutdown(
                    events,
                    shutdown,
                    TerminalEvent::Failed(format!("PTY read stopped: {error}")),
                );
                false
            }
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
        poll_readable(fd, 10)
    }

    fn poll_readable(fd: libc::c_int, timeout_ms: libc::c_int) -> std::io::Result<bool> {
        let mut descriptor = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        loop {
            let result = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
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
    use super::{
        ProcessSnapshot, PtyOutput, ReaderControl, enqueue_process_input, reader_checkpoint,
    };
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };

    #[test]
    fn process_snapshot_detects_a_running_child() {
        #[cfg(windows)]
        let mut child = Command::new("ping.exe")
            .args(["127.0.0.1", "-n", "10"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn child process");
        #[cfg(unix)]
        let mut child = Command::new("sleep")
            .arg("10")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn child process");
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut processes = ProcessSnapshot::new();
        let detected = loop {
            processes.refresh();
            if processes.has_child_process(std::process::id()) {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let _ = child.kill();
        let _ = child.wait();

        assert!(detected);
    }

    #[test]
    fn shutdown_cancels_a_blocked_worker_send() {
        let (events, _event_receiver) = flume::bounded(1);
        events.send(()).expect("fill worker queue");
        let (shutdown, shutdown_receiver) = flume::bounded::<()>(1);
        drop(shutdown);

        assert!(!super::send_or_shutdown(&events, &shutdown_receiver, ()));
    }

    #[test]
    fn process_input_queue_reports_backpressure_without_blocking() {
        let (input, _queued) = flume::bounded(1);
        enqueue_process_input(&input, b"first", "test PTY").expect("fill input queue");

        assert_eq!(
            enqueue_process_input(&input, b"second", "test PTY").unwrap_err(),
            "test PTY input queue is full"
        );
    }

    #[test]
    fn pause_barrier_drains_platform_output_before_its_marker() {
        let (control, controls) = flume::unbounded();
        control.send(ReaderControl::Pause).expect("queue pause");
        control.send(ReaderControl::Resume).expect("queue resume");
        let (output, outputs) = flume::unbounded();
        let (_shutdown, shutdown) = flume::bounded::<()>(1);

        assert!(reader_checkpoint(&controls, &output, &shutdown, || {
            output
                .send(PtyOutput::Bytes(b"\x1b[?2004h".to_vec()))
                .is_ok()
        }));

        assert!(matches!(
            outputs.recv().expect("receive buffered platform bytes"),
            PtyOutput::Bytes(bytes) if bytes == b"\x1b[?2004h"
        ));
        assert!(matches!(
            outputs.recv().expect("receive pause marker"),
            PtyOutput::Paused
        ));
    }
}
