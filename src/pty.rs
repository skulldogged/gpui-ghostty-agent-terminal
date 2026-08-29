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
    Synchronized,
}

#[derive(Clone, Copy)]
pub(crate) enum ReaderControl {
    Pause,
    Resume,
    Synchronize,
}

pub(crate) const PROCESS_WRITE_QUEUE_CAPACITY: usize = 64;
// The final slot is reserved for the ordered Flush/Resize barrier used by a
// viewport resize. Ordinary writes report backpressure before consuming it.
pub(crate) const PROCESS_INPUT_QUEUE_CAPACITY: usize = PROCESS_WRITE_QUEUE_CAPACITY + 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProcessInputStatus {
    Accepted,
    Backpressured,
}

pub(crate) enum ProcessInput {
    Write(Vec<u8>),
    Resize {
        size: PtySize,
        completed: flume::Sender<Result<(), String>>,
    },
    Flush {
        completed: flume::Sender<Result<(), String>>,
    },
}

pub(crate) fn enqueue_process_input(
    input: &flume::Sender<ProcessInput>,
    bytes: &[u8],
    transport: &str,
) -> Result<ProcessInputStatus, String> {
    if input.len() >= PROCESS_WRITE_QUEUE_CAPACITY {
        return Ok(ProcessInputStatus::Backpressured);
    }
    match input.try_send(ProcessInput::Write(bytes.to_vec())) {
        Ok(()) => Ok(ProcessInputStatus::Accepted),
        Err(flume::TrySendError::Full(_)) => Ok(ProcessInputStatus::Backpressured),
        Err(flume::TrySendError::Disconnected(_)) => {
            Err(format!("{transport} input writer stopped"))
        }
    }
}

pub(crate) fn enqueue_process_resize(
    input: &flume::Sender<ProcessInput>,
    size: PtySize,
    transport: &str,
) -> Result<(), String> {
    let (completed, completion) = flume::bounded(1);
    enqueue_process_command(input, ProcessInput::Resize { size, completed }, transport)?;
    completion
        .recv()
        .map_err(|_| format!("{transport} input writer stopped before resize completed"))?
}

pub(crate) fn flush_process_input(
    input: &flume::Sender<ProcessInput>,
    transport: &str,
) -> Result<(), String> {
    let (completed, completion) = flume::bounded(1);
    enqueue_process_command(input, ProcessInput::Flush { completed }, transport)?;
    completion
        .recv()
        .map_err(|_| format!("{transport} input writer stopped before flush completed"))?
}

fn enqueue_process_command(
    input: &flume::Sender<ProcessInput>,
    command: ProcessInput,
    transport: &str,
) -> Result<(), String> {
    input.try_send(command).map_err(|error| match error {
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

pub(crate) fn publish_writer_failure(
    events: &crate::terminal_session::TerminalEventSender,
    error: String,
) {
    // The writer owns resize/flush acknowledgements. It must be able to exit
    // and drop queued completion senders even when a coalesced Changed event
    // already occupies the lifecycle queue.
    let _ = events.lifecycle(crate::terminal_session::TerminalEvent::Failed(error));
}

pub(crate) fn reader_checkpoint(
    control: &flume::Receiver<ReaderControl>,
    output: &flume::Sender<PtyOutput>,
    shutdown: &flume::Receiver<()>,
    drain_pending: impl FnOnce() -> bool,
) -> bool {
    match control.try_recv() {
        Ok(ReaderControl::Synchronize) => {
            drain_pending() && send_or_shutdown(output, shutdown, PtyOutput::Synchronized)
        }
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
        PROCESS_INPUT_QUEUE_CAPACITY, ProcessInput, ProcessInputStatus, ProcessSnapshot, PtyOutput,
        PtySize, ReaderControl, enqueue_process_input, enqueue_process_resize, flush_process_input,
        publish_writer_failure, reader_checkpoint, receive_or_shutdown, send_or_shutdown,
    };
    use crate::terminal_session::{TerminalEvent, TerminalEventSender};
    use portable_pty::{
        Child, CommandBuilder, MasterPty, PtySize as PortablePtySize, native_pty_system,
    };
    use std::{
        io::{Read, Write},
        sync::{Arc, Mutex},
    };

    pub struct PtySession {
        master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
        input: flume::Sender<ProcessInput>,
        child: Option<Box<dyn Child + Send>>,
        shell_process_id: Option<u32>,
        control: flume::Sender<ReaderControl>,
        shutdown: Option<flume::Sender<()>>,
    }

    impl PtySession {
        pub fn spawn(
            size: PtySize,
            working_directory: &std::path::Path,
            events: TerminalEventSender,
        ) -> Result<(Self, flume::Receiver<PtyOutput>), String> {
            let pair = native_pty_system()
                .openpty(size.into())
                .map_err(|error| format!("open PTY: {error}"))?;

            let mut command = CommandBuilder::new(shell());
            command.cwd(working_directory);
            command.env("TERM", "xterm-256color");
            command.env("COLORTERM", "truecolor");

            let mut child = pair
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
            let (input_tx, input_rx) = flume::bounded(PROCESS_INPUT_QUEUE_CAPACITY);
            let master = Arc::new(Mutex::new(pair.master));

            let writer_events = events.clone();
            let writer_shutdown = shutdown_rx.clone();
            let writer_master = Arc::clone(&master);
            let writer_spawn = std::thread::Builder::new()
                .name("terminal-pty-writer".into())
                .spawn(move || {
                    let mut writer = writer;
                    while let Some(command) = receive_or_shutdown(&input_rx, &writer_shutdown) {
                        let (result, completion) = match command {
                            ProcessInput::Write(bytes) => (
                                writer
                                    .write_all(&bytes)
                                    .and_then(|_| writer.flush())
                                    .map_err(|error| format!("update PTY transport: {error}")),
                                None,
                            ),
                            ProcessInput::Resize { size, completed } => {
                                let result = writer_master
                                    .lock()
                                    .map_err(|_| "lock PTY master for resize".to_string())
                                    .and_then(|master| {
                                        master
                                            .resize(size.into())
                                            .map_err(|error| format!("resize PTY: {error}"))
                                    });
                                (result, Some(completed))
                            }
                            ProcessInput::Flush { completed } => {
                                let result = writer
                                    .flush()
                                    .map_err(|error| format!("flush PTY input: {error}"));
                                (result, Some(completed))
                            }
                        };
                        if let Some(completion) = completion {
                            let _ = completion.send(result.clone());
                        }
                        if let Err(error) = result {
                            publish_writer_failure(&writer_events, error);
                            break;
                        }
                    }
                });
            if let Err(error) = writer_spawn {
                let kill_error = child.kill().err();
                let wait_error = child.wait().err();
                let cleanup_error = kill_error.or(wait_error);
                return Err(cleanup_error.map_or_else(
                    || format!("spawn PTY writer thread: {error}"),
                    |cleanup_error| {
                        format!(
                            "spawn PTY writer thread: {error}; also failed to clean up shell: {cleanup_error}"
                        )
                    },
                ));
            }

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
                                let _ = events.lifecycle(TerminalEvent::Failed(format!(
                                    "wait for PTY output: {error}"
                                )));
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
                    master,
                    input: input_tx,
                    child: Some(child),
                    shell_process_id,
                    control: control_tx,
                    shutdown: Some(shutdown_tx),
                },
                output_rx,
            ))
        }

        pub fn write(&mut self, bytes: &[u8]) -> Result<ProcessInputStatus, String> {
            enqueue_process_input(&self.input, bytes, "PTY")
        }

        pub fn flush_input(&mut self) -> Result<(), String> {
            flush_process_input(&self.input, "PTY")
        }

        pub fn resize(&mut self, size: PtySize) -> Result<(), String> {
            enqueue_process_resize(&self.input, size, "PTY")
        }

        pub fn has_foreground_process(&self, processes: &ProcessSnapshot) -> Result<bool, String> {
            let shell_process_id = self
                .shell_process_id
                .ok_or_else(|| "terminal shell does not expose its process ID".to_string())?;
            let foreground_process_group = self
                .master
                .lock()
                .map_err(|_| "lock PTY master for process inspection".to_string())?
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

        pub fn synchronize_reader(&mut self) -> Result<(), String> {
            self.control
                .send(ReaderControl::Synchronize)
                .map_err(|_| "synchronize PTY reader: reader stopped".to_string())
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
        events: &TerminalEventSender,
        shutdown: &flume::Receiver<()>,
    ) -> bool {
        loop {
            match poll_readable(reader_fd, 0) {
                Ok(false) => return true,
                Ok(true) => {}
                Err(error) => {
                    let _ = events.lifecycle(TerminalEvent::Failed(format!(
                        "wait for PTY output barrier: {error}"
                    )));
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
        events: &TerminalEventSender,
        shutdown: &flume::Receiver<()>,
    ) -> bool {
        match reader.read(buffer) {
            Ok(0) => {
                let _ = events.lifecycle(TerminalEvent::Exited);
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
            Ok(_) => events.changed(),
            Err(error) if is_normal_pty_exit(&error) => {
                let _ = events.lifecycle(TerminalEvent::Exited);
                false
            }
            Err(error) => {
                let _ =
                    events.lifecycle(TerminalEvent::Failed(format!("PTY read stopped: {error}")));
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
        PROCESS_INPUT_QUEUE_CAPACITY, PROCESS_WRITE_QUEUE_CAPACITY, ProcessInput,
        ProcessInputStatus, ProcessSnapshot, PtyOutput, PtySize, ReaderControl,
        enqueue_process_input, enqueue_process_resize, flush_process_input, publish_writer_failure,
        reader_checkpoint,
    };
    use crate::terminal_session::{TerminalEvent, TerminalEventSender};
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
    fn writer_failure_is_retained_beside_a_coalesced_change() {
        let (events, queued) = TerminalEventSender::channel();
        assert!(events.changed());
        assert!(events.changed(), "duplicate changes should coalesce");

        publish_writer_failure(&events, "injected writer failure".into());

        assert!(matches!(
            queued.recv_timeout(Duration::from_secs(1)),
            Ok(TerminalEvent::Failed(error)) if error == "injected writer failure"
        ));
        assert!(matches!(
            queued.recv_timeout(Duration::from_secs(1)),
            Ok(TerminalEvent::Changed)
        ));
    }

    #[test]
    fn process_input_queue_reports_backpressure_without_blocking() {
        let (input, _queued) = flume::bounded(1);
        assert_eq!(
            enqueue_process_input(&input, b"first", "test PTY").expect("fill input queue"),
            ProcessInputStatus::Accepted
        );

        assert_eq!(
            enqueue_process_input(&input, b"second", "test PTY")
                .expect("report input backpressure"),
            ProcessInputStatus::Backpressured
        );
    }

    #[test]
    fn process_resize_stays_ordered_after_earlier_input() {
        let (input, queued) = flume::bounded(2);
        enqueue_process_input(&input, b"before resize", "test PTY").expect("queue input");
        let size = PtySize {
            rows: 40,
            cols: 100,
            pixel_width: 900,
            pixel_height: 720,
        };
        let resize_input = input.clone();
        let resize =
            std::thread::spawn(move || enqueue_process_resize(&resize_input, size, "test PTY"));

        assert!(matches!(
            queued.recv().expect("receive input"),
            ProcessInput::Write(bytes) if bytes == b"before resize"
        ));
        let ProcessInput::Resize {
            size: queued_size,
            completed,
        } = queued.recv().expect("receive resize")
        else {
            panic!("second transport command must be the resize");
        };
        assert_eq!(queued_size, size);
        completed.send(Ok(())).expect("complete resize");
        resize
            .join()
            .expect("join resize sender")
            .expect("finish ordered resize");
    }

    #[test]
    fn process_flush_waits_behind_earlier_input() {
        let (input, queued) = flume::bounded(2);
        assert_eq!(
            enqueue_process_input(&input, b"before flush", "test PTY")
                .expect("queue input before flush"),
            ProcessInputStatus::Accepted
        );
        let flush_input = input.clone();
        let flush = std::thread::spawn(move || flush_process_input(&flush_input, "test PTY"));

        assert!(matches!(
            queued.recv().expect("receive input"),
            ProcessInput::Write(bytes) if bytes == b"before flush"
        ));
        let ProcessInput::Flush { completed } = queued.recv().expect("receive flush") else {
            panic!("second transport command must be the flush");
        };
        completed.send(Ok(())).expect("complete flush");
        flush
            .join()
            .expect("join flush sender")
            .expect("finish ordered flush");
    }

    #[test]
    fn process_flush_uses_capacity_reserved_from_ordinary_writes() {
        let (input, queued) = flume::bounded(PROCESS_INPUT_QUEUE_CAPACITY);
        for index in 0..PROCESS_WRITE_QUEUE_CAPACITY {
            assert_eq!(
                enqueue_process_input(&input, &[index as u8], "test PTY")
                    .expect("fill ordinary write capacity"),
                ProcessInputStatus::Accepted
            );
        }
        assert_eq!(
            enqueue_process_input(&input, b"overflow", "test PTY")
                .expect("reserve control capacity"),
            ProcessInputStatus::Backpressured
        );

        let flush_input = input.clone();
        let flush = std::thread::spawn(move || flush_process_input(&flush_input, "test PTY"));
        for _ in 0..PROCESS_WRITE_QUEUE_CAPACITY {
            assert!(matches!(
                queued.recv().expect("receive queued write"),
                ProcessInput::Write(_)
            ));
        }
        let ProcessInput::Flush { completed } = queued.recv().expect("receive reserved flush")
        else {
            panic!("reserved transport command must be the flush");
        };
        completed.send(Ok(())).expect("complete reserved flush");
        flush
            .join()
            .expect("join reserved flush sender")
            .expect("finish reserved flush");
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

    #[test]
    fn synchronization_drains_platform_output_without_pausing() {
        let (control, controls) = flume::unbounded();
        control
            .send(ReaderControl::Synchronize)
            .expect("queue synchronization");
        let (output, outputs) = flume::unbounded();
        let (_shutdown, shutdown) = flume::bounded::<()>(1);

        assert!(reader_checkpoint(&controls, &output, &shutdown, || {
            output
                .send(PtyOutput::Bytes(b"\x1b[?1049h".to_vec()))
                .is_ok()
        }));
        assert!(matches!(
            outputs.recv().expect("receive buffered platform bytes"),
            PtyOutput::Bytes(bytes) if bytes == b"\x1b[?1049h"
        ));
        assert!(matches!(
            outputs.recv().expect("receive synchronization marker"),
            PtyOutput::Synchronized
        ));
    }
}
