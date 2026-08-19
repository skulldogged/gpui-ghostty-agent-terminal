use crate::{
    ghostty,
    terminal_session::{TerminalEvent, TerminalSession, TerminalSize},
};
use interprocess::local_socket::{
    GenericNamespaced, ListenerOptions, Stream as LocalSocketStream, prelude::*,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    io::{self, BufRead, BufReader, Write},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const PROTOCOL_VERSION: u16 = 1;
const MAX_MESSAGE_BYTES: u64 = 16 * 1024 * 1024;
const SESSION_TICK: Duration = Duration::from_millis(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreEndpoint(String);

impl CoreEndpoint {
    pub fn for_profile(profile: &str) -> Result<Self, String> {
        if profile.is_empty() || profile.len() > 80 {
            return Err("Resident Core profile must contain 1 to 80 characters".into());
        }
        if !profile
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(
                "Resident Core profile may contain only ASCII letters, numbers, '.', '_' and '-'"
                    .into(),
            );
        }

        Ok(Self(format!("agent-terminal-{profile}")))
    }

    pub fn for_current_user() -> Result<Self, String> {
        #[cfg(unix)]
        let identity = unsafe { libc::geteuid() }.to_string();

        #[cfg(windows)]
        let identity = {
            let domain = std::env::var("USERDOMAIN").unwrap_or_default();
            let user = std::env::var("USERNAME")
                .map_err(|_| "USERNAME is required to name the Resident Core endpoint")?;
            format!("{domain}-{user}")
                .chars()
                .map(|character| {
                    if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                        character
                    } else {
                        '-'
                    }
                })
                .collect::<String>()
        };

        Self::for_profile(&format!("{identity}-default"))
    }

    pub fn from_argument(argument: String) -> Result<Self, String> {
        let profile = argument
            .strip_prefix("agent-terminal-")
            .ok_or_else(|| "invalid Resident Core endpoint prefix".to_string())?;
        Self::for_profile(profile)
    }

    pub fn argument(&self) -> &str {
        &self.0
    }

    fn name(&self) -> io::Result<interprocess::local_socket::Name<'_>> {
        self.0.as_str().to_ns_name::<GenericNamespaced>()
    }
}

pub struct CoreClient {
    connection: BufReader<LocalSocketStream>,
}

impl CoreClient {
    pub fn connect(endpoint: &CoreEndpoint, timeout: Duration) -> Result<Self, String> {
        let deadline = Instant::now() + timeout;

        loop {
            let connection_error = match LocalSocketStream::connect(
                endpoint
                    .name()
                    .map_err(|error| format!("name Resident Core endpoint: {error}"))?,
            ) {
                Ok(stream) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    stream
                        .set_recv_timeout(Some(remaining.max(Duration::from_millis(1))))
                        .map_err(|error| format!("set Resident Core receive timeout: {error}"))?;
                    stream
                        .set_send_timeout(Some(remaining.max(Duration::from_millis(1))))
                        .map_err(|error| format!("set Resident Core send timeout: {error}"))?;
                    let mut client = Self {
                        connection: BufReader::new(stream),
                    };
                    match client.request(Request::Hello {
                        version: PROTOCOL_VERSION,
                    })? {
                        Response::Ready {
                            version: PROTOCOL_VERSION,
                        } => return Ok(client),
                        Response::Ready { version } => {
                            return Err(format!(
                                "Resident Core protocol mismatch: client {PROTOCOL_VERSION}, core {version}"
                            ));
                        }
                        Response::Error(error) => return Err(error),
                        response => {
                            return Err(format!(
                                "Resident Core returned an invalid handshake response: {response:?}"
                            ));
                        }
                    }
                }
                Err(error) => error,
            };

            if Instant::now() >= deadline {
                return Err(format!("connect Resident Core: {connection_error}"));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn connect_or_spawn() -> Result<Self, String> {
        let endpoint = CoreEndpoint::for_current_user()?;
        if let Ok(client) = Self::connect(&endpoint, Duration::from_millis(100)) {
            return Ok(client);
        }

        spawn_resident_core(&endpoint)?;
        Self::connect(&endpoint, Duration::from_secs(10))
    }

    pub fn input(&mut self, bytes: &[u8]) -> Result<(), String> {
        match self.request(Request::Input {
            bytes: bytes.to_vec(),
        })? {
            Response::Ack => Ok(()),
            Response::Error(error) => Err(error),
            response => Err(format!("invalid input response: {response:?}")),
        }
    }

    pub fn resize(&mut self, size: TerminalSize) -> Result<(), String> {
        match self.request(Request::Resize { size })? {
            Response::Ack => Ok(()),
            Response::Error(error) => Err(error),
            response => Err(format!("invalid resize response: {response:?}")),
        }
    }

    pub fn snapshot(&mut self) -> Result<TerminalSnapshot, String> {
        match self.request(Request::Snapshot)? {
            Response::Snapshot(snapshot) => Ok(snapshot),
            Response::Error(error) => Err(error),
            response => Err(format!("invalid snapshot response: {response:?}")),
        }
    }

    pub fn stop_resident_core(&mut self) -> Result<(), String> {
        match self.request(Request::StopResidentCore)? {
            Response::Ack => Ok(()),
            Response::Error(error) => Err(error),
            response => Err(format!("invalid stop response: {response:?}")),
        }
    }

    fn request(&mut self, request: Request) -> Result<Response, String> {
        write_message(self.connection.get_mut(), &request)
            .map_err(|error| format!("send Resident Core command: {error}"))?;
        read_message(&mut self.connection)
            .map_err(|error| format!("receive Resident Core response: {error}"))?
            .ok_or_else(|| "Resident Core disconnected before responding".into())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TerminalSnapshot {
    pub cols: u16,
    pub rows: u16,
    pub cursor: Option<(u16, u16)>,
    pub default_fg: [u8; 3],
    pub default_bg: [u8; 3],
    pub cells: Vec<TerminalCell>,
}

impl TerminalSnapshot {
    pub fn text(&self) -> String {
        let mut output = String::new();
        for y in 0..self.rows {
            for x in 0..self.cols {
                match self.cells.iter().find(|cell| cell.x == x && cell.y == y) {
                    Some(cell) if !cell.text.is_empty() => output.push_str(&cell.text),
                    _ => output.push(' '),
                }
            }
            output.push('\n');
        }
        output
    }
}

impl From<ghostty::Snapshot> for TerminalSnapshot {
    fn from(snapshot: ghostty::Snapshot) -> Self {
        Self {
            cols: snapshot.cols,
            rows: snapshot.rows,
            cursor: snapshot.cursor,
            default_fg: snapshot.default_fg,
            default_bg: snapshot.default_bg,
            cells: snapshot.cells.into_iter().map(TerminalCell::from).collect(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TerminalCell {
    pub x: u16,
    pub y: u16,
    pub text: String,
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    pub has_explicit_bg: bool,
}

impl From<ghostty::Cell> for TerminalCell {
    fn from(cell: ghostty::Cell) -> Self {
        Self {
            x: cell.x,
            y: cell.y,
            text: cell.text,
            fg: cell.fg,
            bg: cell.bg,
            has_explicit_bg: cell.has_explicit_bg,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
enum Request {
    Hello { version: u16 },
    Input { bytes: Vec<u8> },
    Resize { size: TerminalSize },
    Snapshot,
    StopResidentCore,
}

#[derive(Debug, Serialize, Deserialize)]
enum Response {
    Ready { version: u16 },
    Ack,
    Snapshot(TerminalSnapshot),
    Error(String),
}

enum WorkerRequest {
    Input(Vec<u8>),
    Resize(TerminalSize),
    Snapshot,
    Stop,
}

enum WorkerResponse {
    Ack,
    Snapshot(TerminalSnapshot),
}

struct WorkerCommand {
    request: WorkerRequest,
    response: flume::Sender<Result<WorkerResponse, String>>,
}

struct ResidentCore {
    commands: flume::Sender<WorkerCommand>,
}

impl ResidentCore {
    fn start() -> Result<Self, String> {
        let (commands_tx, commands_rx) = flume::bounded::<WorkerCommand>(32);
        let (ready_tx, ready_rx) = flume::bounded(1);
        thread::Builder::new()
            .name("resident-core-terminal".into())
            .spawn(move || {
                let spawned = TerminalSession::spawn(TerminalSize::default());
                let (mut session, events) = match spawned {
                    Ok(spawned) => {
                        let _ = ready_tx.send(Ok(()));
                        spawned
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };

                loop {
                    match commands_rx.recv_timeout(SESSION_TICK) {
                        Ok(command) => {
                            let stop = matches!(command.request, WorkerRequest::Stop);
                            let result = match command.request {
                                WorkerRequest::Input(bytes) => {
                                    session.input(&bytes).map(|()| WorkerResponse::Ack)
                                }
                                WorkerRequest::Resize(size) => {
                                    session.resize(size).map(|()| WorkerResponse::Ack)
                                }
                                WorkerRequest::Snapshot => session
                                    .snapshot()
                                    .map(TerminalSnapshot::from)
                                    .map(WorkerResponse::Snapshot),
                                WorkerRequest::Stop => Ok(WorkerResponse::Ack),
                            };
                            let _ = command.response.send(result);
                            if stop {
                                break;
                            }
                        }
                        Err(flume::RecvTimeoutError::Timeout) => {
                            let _ = session.drain_pending_output();
                        }
                        Err(flume::RecvTimeoutError::Disconnected) => break,
                    }

                    while let Some(event) = events.try_recv() {
                        if let TerminalEvent::Failed(error) = event {
                            eprintln!("Terminal Session failed: {error}");
                        }
                    }
                }
            })
            .map_err(|error| format!("spawn Resident Core terminal thread: {error}"))?;
        ready_rx
            .recv()
            .map_err(|_| "Resident Core terminal thread stopped during startup".to_string())??;
        Ok(Self {
            commands: commands_tx,
        })
    }

    fn call(&self, request: WorkerRequest) -> Result<WorkerResponse, String> {
        let (response_tx, response_rx) = flume::bounded(1);
        self.commands
            .send(WorkerCommand {
                request,
                response: response_tx,
            })
            .map_err(|_| "Resident Core terminal thread stopped".to_string())?;
        response_rx
            .recv()
            .map_err(|_| "Resident Core terminal thread stopped before responding".to_string())?
    }
}

pub fn run_resident_core(endpoint: CoreEndpoint) -> Result<(), String> {
    let name = endpoint
        .name()
        .map_err(|error| format!("name Resident Core endpoint: {error}"))?;
    let options = ListenerOptions::new().name(name);
    #[cfg(unix)]
    let options = {
        use interprocess::os::unix::local_socket::ListenerOptionsExt;
        options.mode(0o600)
    };
    let listener = options
        .create_sync()
        .map_err(|error| format!("listen at Resident Core endpoint: {error}"))?;
    let core = ResidentCore::start()?;

    loop {
        let stream = listener
            .accept()
            .map_err(|error| format!("accept UI Client: {error}"))?;
        if !same_user(&stream)? {
            continue;
        }
        match handle_client(stream, &core) {
            Ok(ClientOutcome::Disconnected) => {}
            Ok(ClientOutcome::StopResidentCore) => return Ok(()),
            Err(error) if is_disconnect(&error) => {}
            Err(error) => eprintln!("UI Client connection failed: {error}"),
        }
    }
}

enum ClientOutcome {
    Disconnected,
    StopResidentCore,
}

fn handle_client(stream: LocalSocketStream, core: &ResidentCore) -> io::Result<ClientOutcome> {
    let mut connection = BufReader::new(stream);
    let Some(Request::Hello { version }) = read_message(&mut connection)? else {
        write_message(
            connection.get_mut(),
            &Response::Error("UI Client must begin with a protocol handshake".into()),
        )?;
        return Ok(ClientOutcome::Disconnected);
    };
    if version != PROTOCOL_VERSION {
        write_message(
            connection.get_mut(),
            &Response::Error(format!(
                "Resident Core protocol mismatch: client {version}, core {PROTOCOL_VERSION}"
            )),
        )?;
        return Ok(ClientOutcome::Disconnected);
    }
    write_message(
        connection.get_mut(),
        &Response::Ready {
            version: PROTOCOL_VERSION,
        },
    )?;

    loop {
        let Some(request) = read_message::<_, Request>(&mut connection)? else {
            return Ok(ClientOutcome::Disconnected);
        };
        let (response, outcome) = match request {
            Request::Hello { .. } => (
                Response::Error("UI Client already completed its handshake".into()),
                None,
            ),
            Request::Input { bytes } if bytes.len() > 1024 * 1024 => (
                Response::Error("terminal input command exceeds 1 MiB".into()),
                None,
            ),
            Request::Input { bytes } => (
                worker_response(core.call(WorkerRequest::Input(bytes))),
                None,
            ),
            Request::Resize { size } => (
                worker_response(core.call(WorkerRequest::Resize(size))),
                None,
            ),
            Request::Snapshot => (worker_response(core.call(WorkerRequest::Snapshot)), None),
            Request::StopResidentCore => (
                worker_response(core.call(WorkerRequest::Stop)),
                Some(ClientOutcome::StopResidentCore),
            ),
        };
        write_message(connection.get_mut(), &response)?;
        if let Some(outcome) = outcome {
            return Ok(outcome);
        }
    }
}

fn worker_response(response: Result<WorkerResponse, String>) -> Response {
    match response {
        Ok(WorkerResponse::Ack) => Response::Ack,
        Ok(WorkerResponse::Snapshot(snapshot)) => Response::Snapshot(snapshot),
        Err(error) => Response::Error(error),
    }
}

fn write_message<W: Write, T: Serialize>(writer: &mut W, message: &T) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, message).map_err(io::Error::other)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

fn read_message<R: BufRead, T: DeserializeOwned>(reader: &mut R) -> io::Result<Option<T>> {
    let mut bytes = Vec::new();
    let mut limited = std::io::Read::take(reader, MAX_MESSAGE_BYTES + 1);
    let read = limited.read_until(b'\n', &mut bytes)?;
    if read == 0 {
        return Ok(None);
    }
    if bytes.len() as u64 > MAX_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Resident Core protocol message exceeds 16 MiB",
        ));
    }
    if bytes.last() != Some(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Resident Core protocol message is not newline terminated",
        ));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(io::Error::other)
}

#[cfg(unix)]
fn same_user(stream: &LocalSocketStream) -> Result<bool, String> {
    let credentials = stream
        .peer_creds()
        .map_err(|error| format!("read UI Client credentials: {error}"))?;
    Ok(credentials.euid() == Some(unsafe { libc::geteuid() }))
}

#[cfg(windows)]
fn same_user(_stream: &LocalSocketStream) -> Result<bool, String> {
    // The duplex named-pipe open requires write access. Windows' default pipe
    // descriptor grants that to the creator owner, LocalSystem, and admins;
    // Everyone and anonymous receive read-only access and cannot handshake.
    Ok(true)
}

fn is_disconnect(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::UnexpectedEof
    )
}

fn spawn_resident_core(endpoint: &CoreEndpoint) -> Result<(), String> {
    let mut command = Command::new(
        std::env::current_exe()
            .map_err(|error| format!("locate application executable: {error}"))?,
    );
    command
        .arg("--resident-core")
        .arg(endpoint.argument())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach_command(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("spawn Resident Core process: {error}"))?;
    thread::Builder::new()
        .name("resident-core-reaper".into())
        .spawn(move || {
            let _ = child.wait();
        })
        .map_err(|error| format!("spawn Resident Core reaper: {error}"))?;
    Ok(())
}

#[cfg(unix)]
fn detach_command(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

#[cfg(windows)]
fn detach_command(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
}
