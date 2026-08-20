use crate::{
    ghostty,
    terminal_session::{TerminalEvent, TerminalEvents, TerminalSession, TerminalSize},
};
#[cfg(unix)]
use interprocess::local_socket::GenericFilePath;
#[cfg(windows)]
use interprocess::local_socket::GenericNamespaced;
use interprocess::local_socket::{
    Listener as LocalSocketListener, ListenerOptions, Stream as LocalSocketStream, prelude::*,
};
use ring::{
    hmac,
    rand::{SecureRandom, SystemRandom},
};
#[cfg(any(unix, test))]
use std::io::Read;
use std::{
    io::{self, BufReader, Write},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

mod wire;

// Version 2 replaces JSON with binary dirty-row updates and carries each
// cell's display width. An old core must fail the handshake rather than let a
// new UI interpret an incompatible terminal grid.
const PROTOCOL_VERSION: u16 = 2;
const MAX_MESSAGE_BYTES: u64 = 16 * 1024 * 1024;
const SESSION_TICK: Duration = Duration::from_millis(10);
const AUTH_SECRET_BYTES: usize = 32;
const AUTH_NONCE_BYTES: usize = 32;

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

    #[cfg(windows)]
    fn name(
        &self,
        auth_secret: &[u8; AUTH_SECRET_BYTES],
    ) -> io::Result<interprocess::local_socket::Name<'static>> {
        protected_endpoint_name(auth_secret, self)
            .to_ns_name::<GenericNamespaced>()
            .map(interprocess::local_socket::Name::into_owned)
    }

    #[cfg(unix)]
    fn name(
        &self,
        _auth_secret: &[u8; AUTH_SECRET_BYTES],
    ) -> io::Result<interprocess::local_socket::Name<'static>> {
        self.socket_path()?
            .to_fs_name::<GenericFilePath>()
            .map(interprocess::local_socket::Name::into_owned)
    }

    #[cfg(unix)]
    fn socket_path(&self) -> io::Result<std::path::PathBuf> {
        Ok(private_runtime_directory()?
            .join(format!("{:016x}.sock", stable_endpoint_hash(&self.0))))
    }

    #[cfg(unix)]
    fn lock_path(&self) -> io::Result<std::path::PathBuf> {
        Ok(private_runtime_directory()?
            .join(format!("{:016x}.lock", stable_endpoint_hash(&self.0))))
    }
}

#[cfg(unix)]
fn private_runtime_directory() -> io::Result<std::path::PathBuf> {
    let effective_user = unsafe { libc::geteuid() };
    let parent = if let Some(runtime_directory) = std::env::var_os("XDG_RUNTIME_DIR") {
        let runtime_directory = std::path::PathBuf::from(runtime_directory);
        verify_owned_directory(&runtime_directory, effective_user)?;
        runtime_directory
    } else {
        let fallback = std::env::temp_dir().join(format!("agent-terminal-{effective_user}"));
        create_owned_directory(&fallback, effective_user)?;
        fallback
    };
    let directory = parent.join("agent-terminal");
    create_owned_directory(&directory, effective_user)?;
    Ok(directory)
}

#[cfg(unix)]
fn create_owned_directory(directory: &std::path::Path, effective_user: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    match std::fs::create_dir(directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    verify_owned_directory(directory, effective_user)?;
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    verify_owned_directory(directory, effective_user)
}

#[cfg(unix)]
fn verify_owned_directory(directory: &std::path::Path, effective_user: u32) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    let metadata = std::fs::symlink_metadata(directory)?;
    if !metadata.file_type().is_dir() || metadata.uid() != effective_user {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "Resident Core runtime directory is not owned by the current user: {}",
                directory.display()
            ),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn stable_endpoint_hash(endpoint: &str) -> u64 {
    endpoint.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn load_auth_secret() -> Result<[u8; AUTH_SECRET_BYTES], String> {
    let path = auth_secret_path()?;
    match std::fs::read(&path) {
        Ok(bytes) => parse_auth_secret(bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let secret = random_bytes::<AUTH_SECRET_BYTES>()?;
            create_auth_secret(&path, secret)
        }
        Err(error) => Err(format!("read Resident Core authentication secret: {error}")),
    }
}

fn create_auth_secret(
    path: &std::path::Path,
    secret: [u8; AUTH_SECRET_BYTES],
) -> Result<[u8; AUTH_SECRET_BYTES], String> {
    let nonce = u64::from_le_bytes(random_bytes::<8>()?);
    let temporary = path.with_extension(format!("tmp-{}-{nonce:016x}", std::process::id()));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|error| format!("create Resident Core authentication secret: {error}"))?;
        file.write_all(&secret)
            .map_err(|error| format!("write Resident Core authentication secret: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("persist Resident Core authentication secret: {error}"))?;
        drop(file);

        match std::fs::hard_link(&temporary, path) {
            Ok(()) => Ok(secret),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => std::fs::read(path)
                .map_err(|error| format!("read raced Resident Core authentication secret: {error}"))
                .and_then(parse_auth_secret),
            Err(error) => Err(format!(
                "publish Resident Core authentication secret: {error}"
            )),
        }
    })();
    let _ = std::fs::remove_file(temporary);
    result
}

fn parse_auth_secret(bytes: Vec<u8>) -> Result<[u8; AUTH_SECRET_BYTES], String> {
    bytes.try_into().map_err(|bytes: Vec<u8>| {
        format!(
            "Resident Core authentication secret has invalid length: {}",
            bytes.len()
        )
    })
}

#[cfg(unix)]
fn auth_secret_path() -> Result<std::path::PathBuf, String> {
    private_runtime_directory()
        .map(|directory| directory.join("core-auth-v1"))
        .map_err(|error| format!("locate Resident Core authentication directory: {error}"))
}

#[cfg(windows)]
fn auth_secret_path() -> Result<std::path::PathBuf, String> {
    let directory = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| "LOCALAPPDATA is required for Resident Core authentication".to_string())?
        .join("AgentTerminal");
    // LOCALAPPDATA inherits the owning user's protected profile ACL. Other
    // local users cannot pre-create or read this shared client/core secret.
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("create Resident Core authentication directory: {error}"))?;
    Ok(directory.join("core-auth-v1"))
}

fn random_bytes<const N: usize>() -> Result<[u8; N], String> {
    let mut bytes = [0_u8; N];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "generate Resident Core authentication randomness".to_string())?;
    Ok(bytes)
}

#[cfg(any(windows, test))]
fn protected_endpoint_name(
    auth_secret: &[u8; AUTH_SECRET_BYTES],
    endpoint: &CoreEndpoint,
) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, auth_secret);
    let mut message = b"agent-terminal-core-endpoint-v1\0".to_vec();
    message.extend_from_slice(endpoint.argument().as_bytes());
    let digest = hmac::sign(&key, &message);
    let mut name = String::from("agent-terminal-v1-");
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in digest.as_ref() {
        name.push(char::from(HEX[usize::from(byte >> 4)]));
        name.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    name
}

fn authentication_message(endpoint: &CoreEndpoint, nonce: &[u8; AUTH_NONCE_BYTES]) -> Vec<u8> {
    let mut message = b"agent-terminal-core-auth-v1\0".to_vec();
    message.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    message.extend_from_slice(endpoint.argument().as_bytes());
    message.push(0);
    message.extend_from_slice(nonce);
    message
}

fn server_proof(
    auth_secret: &[u8; AUTH_SECRET_BYTES],
    endpoint: &CoreEndpoint,
    nonce: &[u8; AUTH_NONCE_BYTES],
) -> [u8; 32] {
    let key = hmac::Key::new(hmac::HMAC_SHA256, auth_secret);
    hmac::sign(&key, &authentication_message(endpoint, nonce))
        .as_ref()
        .try_into()
        .expect("HMAC-SHA256 proof is 32 bytes")
}

fn verify_server_proof(
    auth_secret: &[u8; AUTH_SECRET_BYTES],
    endpoint: &CoreEndpoint,
    nonce: &[u8; AUTH_NONCE_BYTES],
    proof: &[u8; 32],
) -> Result<(), String> {
    let key = hmac::Key::new(hmac::HMAC_SHA256, auth_secret);
    hmac::verify(&key, &authentication_message(endpoint, nonce), proof)
        .map_err(|_| "Resident Core authentication failed".to_string())
}

pub struct CoreClient {
    connection: BufReader<LocalSocketStream>,
    snapshot: Option<TerminalSnapshot>,
}

impl CoreClient {
    pub fn connect(endpoint: &CoreEndpoint, timeout: Duration) -> Result<Self, String> {
        let deadline = Instant::now() + timeout;
        let auth_secret = load_auth_secret()?;

        loop {
            let connection_error = match LocalSocketStream::connect(
                endpoint
                    .name(&auth_secret)
                    .map_err(|error| format!("name Resident Core endpoint: {error}"))?,
            ) {
                Ok(stream) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    configure_stream_timeouts(
                        &stream,
                        Some(remaining.max(Duration::from_millis(1))),
                    )?;
                    let mut client = Self {
                        connection: BufReader::new(stream),
                        snapshot: None,
                    };
                    let nonce = random_bytes::<AUTH_NONCE_BYTES>()?;
                    match client.handshake(nonce, deadline)? {
                        Response::Ready {
                            version: PROTOCOL_VERSION,
                            proof,
                        } => {
                            verify_server_proof(&auth_secret, endpoint, &nonce, &proof)?;
                            configure_stream_timeouts(client.connection.get_ref(), None)?;
                            return Ok(client);
                        }
                        Response::Ready { version, .. } => {
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
        match self.request(Request::Snapshot { since: None })? {
            Response::Snapshot(Some(update)) => self.accept_update(update),
            Response::Snapshot(None) => {
                Err("Resident Core omitted an unconditional snapshot".into())
            }
            Response::Error(error) => Err(error),
            response => Err(format!("invalid snapshot response: {response:?}")),
        }
    }

    pub fn snapshot_since(&mut self, revision: u64) -> Result<Option<TerminalSnapshot>, String> {
        if let Some(snapshot) = &self.snapshot
            && snapshot.revision != revision
        {
            return Ok(Some(snapshot.clone()));
        }
        let since = self
            .snapshot
            .as_ref()
            .filter(|snapshot| snapshot.revision == revision)
            .map(|snapshot| snapshot.revision);
        match self.request(Request::Snapshot { since })? {
            Response::Snapshot(Some(update)) => match self.accept_update(update) {
                Ok(snapshot) => Ok(Some(snapshot)),
                Err(_) => match self.request(Request::Snapshot { since: None })? {
                    Response::Snapshot(Some(update)) => self.accept_update(update).map(Some),
                    Response::Error(error) => Err(error),
                    response => Err(format!("invalid recovery snapshot response: {response:?}")),
                },
            },
            Response::Snapshot(None) => Ok(None),
            Response::Error(error) => Err(error),
            response => Err(format!(
                "invalid conditional snapshot response: {response:?}"
            )),
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
        wire::write_request(self.connection.get_mut(), &request)
            .map_err(|error| format!("send Resident Core command: {error}"))?;
        wire::read_response(&mut self.connection)
            .map_err(|error| format!("receive Resident Core response: {error}"))?
            .ok_or_else(|| "Resident Core disconnected before responding".into())
    }

    fn accept_update(&mut self, update: TerminalUpdate) -> Result<TerminalSnapshot, String> {
        if update.base_revision.is_none() {
            self.snapshot = Some(TerminalSnapshot::from_update(update)?);
        } else {
            self.snapshot
                .as_mut()
                .ok_or_else(|| "Resident Core sent a delta before a full snapshot".to_string())?
                .apply_update(update)?;
        }
        Ok(self
            .snapshot
            .as_ref()
            .expect("accepted update stores a snapshot")
            .clone())
    }

    fn handshake(
        &mut self,
        nonce: [u8; AUTH_NONCE_BYTES],
        deadline: Instant,
    ) -> Result<Response, String> {
        wire::write_request(
            self.connection.get_mut(),
            &Request::Hello {
                version: PROTOCOL_VERSION,
                nonce,
            },
        )
        .map_err(|error| format!("send Resident Core handshake: {error}"))?;
        read_handshake_response(&mut self.connection, deadline)
            .map_err(|error| format!("receive Resident Core handshake: {error}"))?
            .ok_or_else(|| "Resident Core disconnected during handshake".into())
    }
}

#[cfg(unix)]
fn read_handshake_response(
    connection: &mut BufReader<LocalSocketStream>,
    deadline: Instant,
) -> io::Result<Option<Response>> {
    read_response_until(connection, deadline)
}

#[cfg(any(unix, test))]
fn read_response_until<R: Read>(reader: &mut R, deadline: Instant) -> io::Result<Option<Response>> {
    let mut bytes = Vec::new();
    let mut expected_len = None;
    let mut chunk = [0_u8; 8 * 1024];

    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Resident Core handshake deadline elapsed",
            ));
        }

        let read_capacity = expected_len
            .map(|expected_len| expected_len - bytes.len())
            .unwrap_or(chunk.len())
            .min(chunk.len());
        match reader.read(&mut chunk[..read_capacity]) {
            Ok(0) if bytes.is_empty() => return Ok(None),
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Resident Core disconnected inside its handshake frame",
                ));
            }
            Ok(read) => {
                bytes.extend_from_slice(&chunk[..read]);
                if bytes.len() as u64 > MAX_MESSAGE_BYTES + 4 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Resident Core protocol message exceeds 16 MiB",
                    ));
                }
                if expected_len.is_none() && bytes.len() >= 4 {
                    expected_len = Some(wire::expected_frame_len(
                        bytes[..4].try_into().expect("four-byte frame prefix"),
                    )?);
                }
                if let Some(expected_len) = expected_len {
                    if bytes.len() == expected_len {
                        return wire::decode_response(&bytes).map(Some);
                    }
                    if bytes.len() > expected_len {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Resident Core handshake contains trailing frame bytes",
                        ));
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                thread::sleep(
                    deadline
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(10)),
                );
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(windows)]
fn read_handshake_response(
    connection: &mut BufReader<LocalSocketStream>,
    deadline: Instant,
) -> io::Result<Option<Response>> {
    read_windows_response_until(connection, deadline)
}

#[cfg(windows)]
fn read_windows_response_until(
    reader: &mut BufReader<LocalSocketStream>,
    deadline: Instant,
) -> io::Result<Option<Response>> {
    let mut bytes = Vec::new();
    let mut expected_len = None;
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Resident Core handshake deadline elapsed",
            ));
        }

        let available = windows_available_bytes(reader.get_ref())?;
        if available > 0 {
            let remaining_capacity = (MAX_MESSAGE_BYTES + 4 - bytes.len() as u64) as usize;
            let mut chunk = vec![0_u8; available.min(remaining_capacity).min(8 * 1024)];
            let read = std::io::Read::read(reader.get_mut(), &mut chunk)?;
            if read == 0 {
                return Ok(None);
            }
            bytes.extend_from_slice(&chunk[..read]);
            if bytes.len() as u64 > MAX_MESSAGE_BYTES + 4 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Resident Core protocol message exceeds 16 MiB",
                ));
            }
            if expected_len.is_none() && bytes.len() >= 4 {
                expected_len = Some(wire::expected_frame_len(
                    bytes[..4].try_into().expect("four-byte frame prefix"),
                )?);
            }
            if let Some(expected_len) = expected_len {
                if bytes.len() == expected_len {
                    return wire::decode_response(&bytes).map(Some);
                }
                if bytes.len() > expected_len {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Resident Core handshake contains trailing frame bytes",
                    ));
                }
            }
        }
        thread::sleep(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(10)),
        );
    }
}

#[cfg(windows)]
fn windows_available_bytes(stream: &LocalSocketStream) -> io::Result<usize> {
    use std::os::windows::io::AsRawHandle;
    use std::ptr::null_mut;
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;

    let LocalSocketStream::NamedPipe(stream) = stream;
    let handle = stream.inner().as_raw_handle();
    let mut available = 0;
    let result = unsafe {
        PeekNamedPipe(
            handle,
            null_mut(),
            0,
            null_mut(),
            &mut available,
            null_mut(),
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(available as usize)
    }
}

#[cfg(unix)]
fn configure_stream_timeouts(
    stream: &LocalSocketStream,
    timeout: Option<Duration>,
) -> Result<(), String> {
    stream
        .set_recv_timeout(timeout)
        .map_err(|error| format!("set Resident Core receive timeout: {error}"))?;
    stream
        .set_send_timeout(timeout)
        .map_err(|error| format!("set Resident Core send timeout: {error}"))?;
    Ok(())
}

#[cfg(windows)]
fn configure_stream_timeouts(
    _stream: &LocalSocketStream,
    _timeout: Option<Duration>,
) -> Result<(), String> {
    // Windows named pipes do not support socket-style I/O timeouts. The
    // handshake reader polls available bytes and enforces the deadline itself.
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalSnapshot {
    pub revision: u64,
    pub lifecycle: TerminalLifecycle,
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

    fn from_update(update: TerminalUpdate) -> Result<Self, String> {
        if update.base_revision.is_some() {
            return Err("Resident Core sent a delta without a base snapshot".into());
        }
        update.validate()?;
        Ok(Self {
            revision: update.revision,
            lifecycle: update.lifecycle,
            cols: update.cols,
            rows: update.rows,
            cursor: update.cursor,
            default_fg: update.default_fg,
            default_bg: update.default_bg,
            cells: update.cells,
        })
    }

    fn apply_update(&mut self, update: TerminalUpdate) -> Result<(), String> {
        update.validate()?;
        if update.base_revision != Some(self.revision) {
            return Err(format!(
                "Resident Core terminal revision gap: local {}, update base {:?}",
                self.revision, update.base_revision
            ));
        }
        if (update.cols, update.rows) != (self.cols, self.rows) {
            return Err("Resident Core sent changed geometry in a row delta".into());
        }

        let mut replace_row = vec![false; usize::from(self.rows)];
        for &row in &update.dirty_rows {
            replace_row[usize::from(row)] = true;
        }
        self.cells.retain(|cell| !replace_row[usize::from(cell.y)]);
        self.cells.extend(update.cells);
        self.cells.sort_unstable_by_key(|cell| (cell.y, cell.x));
        self.revision = update.revision;
        self.lifecycle = update.lifecycle;
        self.cursor = update.cursor;
        self.default_fg = update.default_fg;
        self.default_bg = update.default_bg;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TerminalUpdate {
    base_revision: Option<u64>,
    revision: u64,
    lifecycle: TerminalLifecycle,
    cols: u16,
    rows: u16,
    cursor: Option<(u16, u16)>,
    default_fg: [u8; 3],
    default_bg: [u8; 3],
    dirty_rows: Vec<u16>,
    cells: Vec<TerminalCell>,
}

impl TerminalUpdate {
    fn from_terminal(
        snapshot: ghostty::Snapshot,
        base_revision: Option<u64>,
        revision: u64,
        lifecycle: TerminalLifecycle,
    ) -> Self {
        Self {
            base_revision: if snapshot.full { None } else { base_revision },
            revision,
            lifecycle,
            cols: snapshot.cols,
            rows: snapshot.rows,
            cursor: snapshot.cursor,
            default_fg: snapshot.default_fg,
            default_bg: snapshot.default_bg,
            dirty_rows: snapshot.dirty_rows,
            cells: snapshot.cells.into_iter().map(TerminalCell::from).collect(),
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.cols == 0 || self.rows == 0 {
            return Err("terminal update has zero geometry".into());
        }
        if self.dirty_rows.windows(2).any(|rows| rows[0] >= rows[1])
            || self.dirty_rows.iter().any(|&row| row >= self.rows)
        {
            return Err("terminal update has invalid dirty rows".into());
        }
        if self.base_revision.is_none() && self.dirty_rows.len() != usize::from(self.rows) {
            return Err("full terminal update does not contain every row".into());
        }
        if self.base_revision.is_none()
            && self
                .dirty_rows
                .iter()
                .copied()
                .enumerate()
                .any(|(expected, actual)| usize::from(actual) != expected)
        {
            return Err("full terminal update rows are not complete".into());
        }
        let mut dirty = vec![false; usize::from(self.rows)];
        for &row in &self.dirty_rows {
            dirty[usize::from(row)] = true;
        }
        if self.cells.iter().any(|cell| {
            cell.x >= self.cols
                || cell.y >= self.rows
                || cell.width > 2
                || !dirty[usize::from(cell.y)]
        }) {
            return Err("terminal update cell is outside its dirty rows".into());
        }
        if let Some((x, y)) = self.cursor
            && (x >= self.cols || y >= self.rows)
        {
            return Err("terminal update cursor is outside the grid".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalLifecycle {
    Running,
    Exited,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalCell {
    pub x: u16,
    pub y: u16,
    pub width: u8,
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
            width: cell.width,
            text: cell.text,
            fg: cell.fg,
            bg: cell.bg,
            has_explicit_bg: cell.has_explicit_bg,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Request {
    Hello {
        version: u16,
        nonce: [u8; AUTH_NONCE_BYTES],
    },
    Input {
        bytes: Vec<u8>,
    },
    Resize {
        size: TerminalSize,
    },
    Snapshot {
        since: Option<u64>,
    },
    StopResidentCore,
}

#[derive(Debug, PartialEq, Eq)]
enum Response {
    Ready { version: u16, proof: [u8; 32] },
    Ack,
    Snapshot(Option<TerminalUpdate>),
    Error(String),
}

enum WorkerRequest {
    Attach,
    Input(Vec<u8>),
    Resize(TerminalSize),
    Snapshot { since: Option<u64> },
    Stop,
}

enum WorkerResponse {
    Ack,
    Snapshot(Option<TerminalUpdate>),
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
                let mut revision = 0_u64;
                let mut lifecycle = TerminalLifecycle::Running;
                let mut last_snapshot_revision = None;

                loop {
                    refresh_terminal_state(&mut session, &events, &mut revision, &mut lifecycle);
                    match commands_rx.recv_timeout(SESSION_TICK) {
                        Ok(command) => {
                            let stop = matches!(command.request, WorkerRequest::Stop);
                            let result = match command.request {
                                WorkerRequest::Attach => {
                                    last_snapshot_revision = None;
                                    Ok(WorkerResponse::Ack)
                                }
                                WorkerRequest::Input(bytes) => {
                                    session.input(&bytes).map(|()| WorkerResponse::Ack)
                                }
                                WorkerRequest::Resize(size) => match size.validate() {
                                    Err(error) => Err(error),
                                    Ok(size) if size == session.size() => Ok(WorkerResponse::Ack),
                                    Ok(size) => {
                                        let result = session.resize(size);
                                        // Resize drains bytes accepted at the old geometry before
                                        // touching the transport. Advance even when the transport
                                        // resize fails so those consumed bytes remain observable.
                                        revision = revision.saturating_add(1);
                                        result.map(|()| WorkerResponse::Ack)
                                    }
                                },
                                WorkerRequest::Snapshot { since } if since == Some(revision) => {
                                    Ok(WorkerResponse::Snapshot(None))
                                }
                                WorkerRequest::Snapshot { since } => {
                                    let force_full =
                                        since.is_none() || since != last_snapshot_revision;
                                    session.render_update(force_full).map(|snapshot| {
                                        let update = TerminalUpdate::from_terminal(
                                            snapshot,
                                            last_snapshot_revision,
                                            revision,
                                            lifecycle.clone(),
                                        );
                                        last_snapshot_revision = Some(revision);
                                        WorkerResponse::Snapshot(Some(update))
                                    })
                                }
                                WorkerRequest::Stop => Ok(WorkerResponse::Ack),
                            };
                            let _ = command.response.send(result);
                            if stop {
                                break;
                            }
                        }
                        Err(flume::RecvTimeoutError::Timeout) => {}
                        Err(flume::RecvTimeoutError::Disconnected) => break,
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

fn refresh_terminal_state(
    session: &mut TerminalSession,
    events: &TerminalEvents,
    revision: &mut u64,
    lifecycle: &mut TerminalLifecycle,
) {
    match session.drain_pending_output() {
        Ok(true) => *revision = revision.saturating_add(1),
        Ok(false) => {}
        Err(error) => {
            *lifecycle = TerminalLifecycle::Failed(error);
            *revision = revision.saturating_add(1);
        }
    }
    while let Some(event) = events.try_recv() {
        match event {
            TerminalEvent::Changed => {}
            TerminalEvent::Exited => {
                *lifecycle = match session.reap_process() {
                    Ok(()) => TerminalLifecycle::Exited,
                    Err(error) => TerminalLifecycle::Failed(error),
                };
                *revision = revision.saturating_add(1);
            }
            TerminalEvent::Failed(error) => {
                *lifecycle = TerminalLifecycle::Failed(error);
                *revision = revision.saturating_add(1);
            }
        }
    }
}

pub fn run_resident_core(endpoint: CoreEndpoint) -> Result<(), String> {
    let auth_secret = load_auth_secret()?;
    let Some((listener, _endpoint_guard)) = create_listener(&endpoint, &auth_secret)? else {
        return Ok(());
    };
    let core = ResidentCore::start()?;

    loop {
        let stream = listener
            .accept()
            .map_err(|error| format!("accept UI Client: {error}"))?;
        if !same_user(&stream)? {
            continue;
        }
        match handle_client(stream, &core, &endpoint, &auth_secret) {
            Ok(ClientOutcome::Disconnected) => {}
            Ok(ClientOutcome::StopResidentCore) => return Ok(()),
            Err(error) if is_disconnect(&error) => {}
            Err(error) => eprintln!("UI Client connection failed: {error}"),
        }
    }
}

#[cfg(unix)]
struct EndpointGuard {
    _lock: std::fs::File,
}

#[cfg(windows)]
struct EndpointGuard;

#[cfg(unix)]
fn create_listener(
    endpoint: &CoreEndpoint,
    auth_secret: &[u8; AUTH_SECRET_BYTES],
) -> Result<Option<(LocalSocketListener, EndpointGuard)>, String> {
    use std::os::{fd::AsRawFd, unix::fs::FileTypeExt};

    let lock_path = endpoint
        .lock_path()
        .map_err(|error| format!("name Resident Core startup lock: {error}"))?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| format!("open Resident Core startup lock: {error}"))?;
    let lock_result = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if lock_result == -1 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::WouldBlock {
            return Ok(None);
        }
        return Err(format!("lock Resident Core startup: {error}"));
    }
    let guard = EndpointGuard { _lock: lock };

    match bind_listener(endpoint, auth_secret) {
        Ok(listener) => Ok(Some((listener, guard))),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::AddrInUse | io::ErrorKind::AlreadyExists
            ) =>
        {
            if endpoint_is_reachable(endpoint, auth_secret)? {
                return Ok(None);
            }

            let socket_path = endpoint
                .socket_path()
                .map_err(|error| format!("name stale Resident Core endpoint: {error}"))?;
            let metadata = std::fs::symlink_metadata(&socket_path)
                .map_err(|error| format!("inspect stale Resident Core endpoint: {error}"))?;
            if !metadata.file_type().is_socket()
                || std::os::unix::fs::MetadataExt::uid(&metadata) != unsafe { libc::geteuid() }
            {
                return Err(format!(
                    "refuse to reclaim unverified Resident Core endpoint: {}",
                    socket_path.display()
                ));
            }
            std::fs::remove_file(&socket_path)
                .map_err(|error| format!("reclaim stale Resident Core endpoint: {error}"))?;
            bind_listener(endpoint, auth_secret)
                .map(|listener| Some((listener, guard)))
                .map_err(|error| format!("listen at reclaimed Resident Core endpoint: {error}"))
        }
        Err(error) => Err(format!("listen at Resident Core endpoint: {error}")),
    }
}

#[cfg(windows)]
fn create_listener(
    endpoint: &CoreEndpoint,
    auth_secret: &[u8; AUTH_SECRET_BYTES],
) -> Result<Option<(LocalSocketListener, EndpointGuard)>, String> {
    bind_listener(endpoint, auth_secret)
        .map(|listener| Some((listener, EndpointGuard)))
        .map_err(|error| format!("listen at Resident Core endpoint: {error}"))
}

fn bind_listener(
    endpoint: &CoreEndpoint,
    auth_secret: &[u8; AUTH_SECRET_BYTES],
) -> io::Result<LocalSocketListener> {
    ListenerOptions::new()
        .name(endpoint.name(auth_secret)?)
        .create_sync()
}

#[cfg(unix)]
fn endpoint_is_reachable(
    endpoint: &CoreEndpoint,
    auth_secret: &[u8; AUTH_SECRET_BYTES],
) -> Result<bool, String> {
    match LocalSocketStream::connect(
        endpoint
            .name(auth_secret)
            .map_err(|error| format!("name existing Resident Core endpoint: {error}"))?,
    ) {
        Ok(_) => Ok(true),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(format!("probe existing Resident Core endpoint: {error}")),
    }
}

enum ClientOutcome {
    Disconnected,
    StopResidentCore,
}

fn handle_client(
    stream: LocalSocketStream,
    core: &ResidentCore,
    endpoint: &CoreEndpoint,
    auth_secret: &[u8; AUTH_SECRET_BYTES],
) -> io::Result<ClientOutcome> {
    let mut connection = BufReader::new(stream);
    let Some(Request::Hello { version, nonce }) = wire::read_request(&mut connection)? else {
        wire::write_response(
            connection.get_mut(),
            &Response::Error("UI Client must begin with a protocol handshake".into()),
        )?;
        return Ok(ClientOutcome::Disconnected);
    };
    if version != PROTOCOL_VERSION {
        wire::write_response(
            connection.get_mut(),
            &Response::Error(format!(
                "Resident Core protocol mismatch: client {version}, core {PROTOCOL_VERSION}"
            )),
        )?;
        return Ok(ClientOutcome::Disconnected);
    }
    core.call(WorkerRequest::Attach).map_err(io::Error::other)?;
    wire::write_response(
        connection.get_mut(),
        &Response::Ready {
            version: PROTOCOL_VERSION,
            proof: server_proof(auth_secret, endpoint, &nonce),
        },
    )?;

    loop {
        let Some(request) = wire::read_request(&mut connection)? else {
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
            Request::Snapshot { since } => (
                worker_response(core.call(WorkerRequest::Snapshot { since })),
                None,
            ),
            Request::StopResidentCore => (
                worker_response(core.call(WorkerRequest::Stop)),
                Some(ClientOutcome::StopResidentCore),
            ),
        };
        wire::write_response(connection.get_mut(), &response)?;
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

#[cfg(test)]
mod tests {
    use super::{
        CoreEndpoint, TerminalCell, TerminalLifecycle, TerminalSnapshot, TerminalUpdate,
        protected_endpoint_name, read_response_until, server_proof, verify_server_proof,
    };

    struct TransientWouldBlock {
        bytes: std::io::Cursor<Vec<u8>>,
        would_block_next: bool,
    }

    impl std::io::Read for TransientWouldBlock {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            if self.would_block_next {
                self.would_block_next = false;
                return Err(std::io::ErrorKind::WouldBlock.into());
            }
            self.would_block_next = true;
            let capacity = output.len().min(2);
            self.bytes.read(&mut output[..capacity])
        }
    }

    struct AlwaysWouldBlock;

    impl std::io::Read for AlwaysWouldBlock {
        fn read(&mut self, _output: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::WouldBlock.into())
        }
    }

    fn cell(x: u16, y: u16, text: &str) -> TerminalCell {
        TerminalCell {
            x,
            y,
            width: 1,
            text: text.into(),
            fg: [0xdd; 3],
            bg: [0x11; 3],
            has_explicit_bg: false,
        }
    }

    #[test]
    fn ordered_dirty_row_updates_replace_only_the_rows_they_name() {
        let mut snapshot = TerminalSnapshot {
            revision: 7,
            lifecycle: TerminalLifecycle::Running,
            cols: 2,
            rows: 2,
            cursor: Some((0, 0)),
            default_fg: [0xdd; 3],
            default_bg: [0x11; 3],
            cells: vec![
                cell(0, 0, "a"),
                cell(1, 0, "b"),
                cell(0, 1, "c"),
                cell(1, 1, "d"),
            ],
        };
        let update = TerminalUpdate {
            base_revision: Some(7),
            revision: 8,
            lifecycle: TerminalLifecycle::Running,
            cols: 2,
            rows: 2,
            cursor: Some((1, 1)),
            default_fg: [0xdd; 3],
            default_bg: [0x11; 3],
            dirty_rows: vec![1],
            cells: vec![cell(0, 1, "x"), cell(1, 1, "y")],
        };

        snapshot.apply_update(update).expect("apply ordered delta");

        assert_eq!(snapshot.revision, 8);
        assert_eq!(snapshot.cursor, Some((1, 1)));
        assert_eq!(snapshot.text(), "ab\nxy\n");
    }

    #[test]
    fn a_revision_gap_requires_a_full_recovery_snapshot() {
        let mut snapshot = TerminalSnapshot {
            revision: 7,
            lifecycle: TerminalLifecycle::Running,
            cols: 1,
            rows: 1,
            cursor: None,
            default_fg: [0xdd; 3],
            default_bg: [0x11; 3],
            cells: vec![cell(0, 0, "a")],
        };
        let update = TerminalUpdate {
            base_revision: Some(6),
            revision: 8,
            lifecycle: TerminalLifecycle::Running,
            cols: 1,
            rows: 1,
            cursor: None,
            default_fg: [0xdd; 3],
            default_bg: [0x11; 3],
            dirty_rows: vec![0],
            cells: vec![cell(0, 0, "b")],
        };

        let error = snapshot
            .apply_update(update)
            .expect_err("reject out-of-order delta");
        assert!(error.contains("revision gap"), "{error}");
        assert_eq!(snapshot.revision, 7);
        assert_eq!(snapshot.text(), "a\n");
    }

    #[test]
    fn protected_endpoint_names_are_bound_to_the_authentication_secret() {
        let endpoint = CoreEndpoint::for_profile("endpoint-binding-test").unwrap();
        let first = protected_endpoint_name(&[7_u8; 32], &endpoint);
        let repeated = protected_endpoint_name(&[7_u8; 32], &endpoint);
        let other_secret = protected_endpoint_name(&[8_u8; 32], &endpoint);

        assert_eq!(first, repeated);
        assert_ne!(first, other_secret);
        assert!(!first.contains(endpoint.argument()));
    }

    #[test]
    fn server_proof_binds_the_secret_endpoint_and_nonce() {
        let endpoint = CoreEndpoint::for_profile("authentication-test").unwrap();
        let other_endpoint = CoreEndpoint::for_profile("other-authentication-test").unwrap();
        let secret = [7_u8; 32];
        let other_secret = [8_u8; 32];
        let nonce = [9_u8; 32];
        let other_nonce = [10_u8; 32];
        let proof = server_proof(&secret, &endpoint, &nonce);

        verify_server_proof(&secret, &endpoint, &nonce, &proof).unwrap();
        assert!(verify_server_proof(&other_secret, &endpoint, &nonce, &proof).is_err());
        assert!(verify_server_proof(&secret, &other_endpoint, &nonce, &proof).is_err());
        assert!(verify_server_proof(&secret, &endpoint, &other_nonce, &proof).is_err());
    }

    #[test]
    fn handshake_reader_survives_transient_would_block() {
        let encoded = super::wire::encode_response(&super::Response::Ready {
            version: 2,
            proof: [7; 32],
        })
        .expect("encode handshake response");
        let mut reader = TransientWouldBlock {
            bytes: std::io::Cursor::new(encoded),
            would_block_next: true,
        };

        let response = read_response_until(
            &mut reader,
            std::time::Instant::now() + std::time::Duration::from_secs(1),
        )
        .expect("transient WouldBlock must not abort the handshake");

        assert!(matches!(response, Some(super::Response::Ready { .. })));
    }

    #[test]
    fn handshake_reader_enforces_its_deadline_while_would_blocked() {
        let mut reader = AlwaysWouldBlock;

        let error = read_response_until(
            &mut reader,
            std::time::Instant::now() + std::time::Duration::from_millis(20),
        )
        .expect_err("a silent handshake must reach its deadline");

        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }

    #[cfg(windows)]
    #[test]
    fn client_handshake_deadline_survives_a_silent_named_pipe() {
        use super::{CoreClient, bind_listener, load_auth_secret};
        use interprocess::local_socket::traits::Listener;
        use std::time::{Duration, Instant};

        let endpoint =
            CoreEndpoint::for_profile(&format!("silent-handshake-{}", std::process::id())).unwrap();
        let auth_secret = load_auth_secret().unwrap();
        let listener = bind_listener(&endpoint, &auth_secret).unwrap();
        let server = std::thread::spawn(move || {
            let _connection = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(250));
        });

        let started = Instant::now();
        let error = CoreClient::connect(&endpoint, Duration::from_millis(50))
            .err()
            .expect("silent named pipe must not complete the handshake");
        assert!(error.contains("deadline elapsed"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(1));
        server.join().unwrap();
    }
}
