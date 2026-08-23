use crate::{
    CoreCommand, CoreModelError, CoreSnapshot, CreatedResource, TerminalSessionId, ghostty,
    terminal_session::TerminalSize,
};
use interprocess::TryClone;
#[cfg(unix)]
use interprocess::local_socket::GenericFilePath;
#[cfg(windows)]
use interprocess::local_socket::GenericNamespaced;
use interprocess::local_socket::{
    Listener as LocalSocketListener, ListenerNonblockingMode, ListenerOptions,
    Stream as LocalSocketStream, prelude::*,
};
use ring::{
    hmac,
    rand::{SecureRandom, SystemRandom},
};
use std::io::Read;
use std::{
    collections::{HashMap, HashSet},
    io::{self, BufReader, Write},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

mod runtime;
mod wire;

use runtime::{CoreRuntime, RuntimeEvent};

// Version 8 adds lease-controlled terminal paste commands whose encoding is
// resolved inside the Resident Core against its authoritative VT modes. Layout
// persistence was later removed without changing the protocol: Terminal launch
// frames retain and ignore the former Restore Disposition byte so an upgraded
// Desktop Shell can attach to an already-running v8 Resident Core.
const PROTOCOL_VERSION: u16 = 8;
const MAX_MESSAGE_BYTES: u64 = 16 * 1024 * 1024;
const SEMANTIC_EVENT_CAPACITY: usize = 64;
const SESSION_TICK: Duration = Duration::from_millis(10);
const CORE_ID_ALLOCATION_HEADROOM: u64 = 1 << 32;
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
        Self::for_current_user_profile("default")
    }

    pub fn for_current_user_profile(profile: &str) -> Result<Self, String> {
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

        Self::for_profile(&format!("{identity}-{profile}"))
    }

    pub fn for_development_launch() -> Result<Self, String> {
        let nonce = u64::from_le_bytes(random_bytes::<8>()?);
        Self::for_current_user_profile(&format!("development-{}-{nonce:016x}", std::process::id()))
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

fn fresh_core_resource_id() -> Result<u64, String> {
    let random = u64::from_le_bytes(random_bytes::<8>()?);
    let maximum_start = u64::MAX - CORE_ID_ALLOCATION_HEADROOM;
    Ok(random % maximum_start + 1)
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalChange {
    pub sequence: u64,
    pub terminal_session_id: TerminalSessionId,
    pub terminal_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticEvent {
    pub sequence: u64,
    pub kind: SemanticEventKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SemanticEventKind {
    ControlLeaseChanged {
        lease: ControlLease,
    },
    TerminalLifecycleChanged {
        terminal_session_id: TerminalSessionId,
        lifecycle: TerminalLifecycle,
        terminal_revision: u64,
    },
    HierarchyChanged {
        revision: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct UiClientId(u64);

impl UiClientId {
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlLease {
    pub terminal_session_id: TerminalSessionId,
    pub generation: u64,
    pub controller: Option<UiClientId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreCommandOutcome {
    pub revision: u64,
    pub snapshot: CoreSnapshot,
    pub created: CreatedResource,
    pub control_leases: Vec<ControlLease>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlLeaseDenial {
    HeldByOther,
    NoController,
    StaleGeneration,
    TargetUnavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoreCommandError {
    ControlLeaseDenied {
        reason: ControlLeaseDenial,
        lease: ControlLease,
    },
    Rejected(CoreModelError),
    Message(String),
}

impl std::fmt::Display for CoreCommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ControlLeaseDenied { reason, lease } => write!(
                formatter,
                "Control Lease denied ({reason:?}); controller {:?}, generation {}",
                lease.controller, lease.generation
            ),
            Self::Rejected(error) => error.fmt(formatter),
            Self::Message(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for CoreCommandError {}

pub struct CoreClient {
    endpoint: CoreEndpoint,
    connection: Arc<Mutex<LocalSocketStream>>,
    responses: flume::Receiver<Result<Response, String>>,
    terminal_changes: flume::Receiver<TerminalChange>,
    semantic_events: flume::Receiver<SemanticEvent>,
    terminal_snapshots: HashMap<TerminalSessionId, TerminalSnapshot>,
    core_snapshot: CoreSnapshot,
    active_terminal_session_id: Option<TerminalSessionId>,
    client_id: UiClientId,
    control_leases: HashMap<TerminalSessionId, ControlLease>,
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
                    let mut connection = BufReader::new(stream);
                    let nonce = random_bytes::<AUTH_NONCE_BYTES>()?;
                    wire::write_request(
                        connection.get_mut(),
                        &Request::Hello {
                            version: PROTOCOL_VERSION,
                            nonce,
                        },
                    )
                    .map_err(|error| format!("send Resident Core handshake: {error}"))?;
                    let handshake = read_handshake_response(&mut connection, deadline)
                        .map_err(|error| format!("receive Resident Core handshake: {error}"))?
                        .ok_or_else(|| "Resident Core disconnected during handshake".to_string())?;
                    match handshake {
                        Response::Ready {
                            version: PROTOCOL_VERSION,
                            proof,
                            client_id,
                            snapshot,
                            leases,
                            semantic_sequence,
                        } => {
                            verify_server_proof(&auth_secret, endpoint, &nonce, &proof)?;
                            configure_stream_timeouts(connection.get_ref(), None)?;
                            let writer = connection
                                .get_ref()
                                .try_clone()
                                .map_err(|error| format!("clone Resident Core stream: {error}"))?;
                            let writer = Arc::new(Mutex::new(writer));
                            let (responses_tx, responses_rx) = flume::unbounded();
                            let (changes_tx, changes_rx) = flume::bounded(1);
                            let (change_wake_tx, change_wake_rx) = flume::bounded(1);
                            let pending_changes = Arc::new(Mutex::new(HashMap::new()));
                            let dispatch_pending = Arc::clone(&pending_changes);
                            thread::Builder::new()
                                .name("resident-core-terminal-changes".into())
                                .spawn(move || {
                                    dispatch_terminal_changes(
                                        change_wake_rx,
                                        dispatch_pending,
                                        changes_tx,
                                    )
                                })
                                .map_err(|error| {
                                    format!("spawn Resident Core terminal dispatcher: {error}")
                                })?;
                            let (semantic_tx, semantic_rx) =
                                flume::bounded(SEMANTIC_EVENT_CAPACITY);
                            let reader_writer = Arc::clone(&writer);
                            thread::Builder::new()
                                .name("resident-core-responses".into())
                                .spawn(move || {
                                    read_core_responses(
                                        connection,
                                        responses_tx,
                                        pending_changes,
                                        change_wake_tx,
                                        semantic_tx,
                                        semantic_sequence,
                                        reader_writer,
                                    )
                                })
                                .map_err(|error| {
                                    format!("spawn Resident Core response reader: {error}")
                                })?;
                            let active_terminal_session_id =
                                snapshot.terminal_sessions.first().map(|session| session.id);
                            return Ok(Self {
                                endpoint: endpoint.clone(),
                                connection: writer,
                                responses: responses_rx,
                                terminal_changes: changes_rx,
                                semantic_events: semantic_rx,
                                terminal_snapshots: HashMap::new(),
                                core_snapshot: snapshot,
                                active_terminal_session_id,
                                client_id,
                                control_leases: leases
                                    .into_iter()
                                    .map(|lease| (lease.terminal_session_id, lease))
                                    .collect(),
                            });
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
        Self::connect_or_spawn_at(&endpoint)
    }

    pub fn connect_or_spawn_at(endpoint: &CoreEndpoint) -> Result<Self, String> {
        if let Ok(client) = Self::connect(endpoint, Duration::from_millis(100)) {
            return Ok(client);
        }

        spawn_resident_core(endpoint)?;
        Self::connect(endpoint, Duration::from_secs(10))
    }

    pub fn endpoint(&self) -> &CoreEndpoint {
        &self.endpoint
    }

    pub fn client_id(&self) -> UiClientId {
        self.client_id
    }

    pub fn control_lease(&self) -> Option<&ControlLease> {
        self.active_terminal_session_id
            .and_then(|terminal_session_id| self.control_lease_for(terminal_session_id))
    }

    pub fn control_lease_for(
        &self,
        terminal_session_id: TerminalSessionId,
    ) -> Option<&ControlLease> {
        self.control_leases.get(&terminal_session_id)
    }

    pub fn core_snapshot(&self) -> &CoreSnapshot {
        &self.core_snapshot
    }

    pub fn active_terminal_session_id(&self) -> Option<TerminalSessionId> {
        self.active_terminal_session_id
    }

    pub fn set_active_terminal_session(
        &mut self,
        terminal_session_id: TerminalSessionId,
    ) -> Result<(), String> {
        if self
            .core_snapshot
            .terminal_sessions
            .iter()
            .any(|session| session.id == terminal_session_id)
        {
            self.active_terminal_session_id = Some(terminal_session_id);
            Ok(())
        } else {
            Err(format!(
                "Terminal Session {} does not exist",
                terminal_session_id.as_u64()
            ))
        }
    }

    pub fn input(&mut self, bytes: &[u8]) -> Result<(), CoreCommandError> {
        let terminal_session_id = self
            .active_terminal_session_id
            .ok_or_else(|| CoreCommandError::Message("no active Terminal Session".into()))?;
        self.input_to(terminal_session_id, bytes)
    }

    pub fn input_to(
        &mut self,
        terminal_session_id: TerminalSessionId,
        bytes: &[u8],
    ) -> Result<(), CoreCommandError> {
        let lease_generation = self
            .control_leases
            .get(&terminal_session_id)
            .ok_or_else(|| {
                CoreCommandError::Message(format!(
                    "Terminal Session {} has no Control Lease",
                    terminal_session_id.as_u64()
                ))
            })?
            .generation;
        match self
            .request(Request::Input {
                terminal_session_id,
                lease_generation,
                bytes: bytes.to_vec(),
            })
            .map_err(CoreCommandError::Message)?
        {
            Response::Ack => Ok(()),
            Response::ControlLeaseDenied { reason, lease } => {
                self.control_leases
                    .insert(lease.terminal_session_id, lease.clone());
                Err(CoreCommandError::ControlLeaseDenied { reason, lease })
            }
            Response::Error(error) => Err(CoreCommandError::Message(error)),
            response => Err(CoreCommandError::Message(format!(
                "invalid input response: {response:?}"
            ))),
        }
    }

    pub fn paste(&mut self, bytes: &[u8]) -> Result<(), CoreCommandError> {
        let terminal_session_id = self
            .active_terminal_session_id
            .ok_or_else(|| CoreCommandError::Message("no active Terminal Session".into()))?;
        self.paste_to(terminal_session_id, bytes)
    }

    pub fn paste_to(
        &mut self,
        terminal_session_id: TerminalSessionId,
        bytes: &[u8],
    ) -> Result<(), CoreCommandError> {
        let lease_generation = self
            .control_leases
            .get(&terminal_session_id)
            .ok_or_else(|| {
                CoreCommandError::Message(format!(
                    "Terminal Session {} has no Control Lease",
                    terminal_session_id.as_u64()
                ))
            })?
            .generation;
        match self
            .request(Request::Paste {
                terminal_session_id,
                lease_generation,
                bytes: bytes.to_vec(),
            })
            .map_err(CoreCommandError::Message)?
        {
            Response::Ack => Ok(()),
            Response::ControlLeaseDenied { reason, lease } => {
                self.control_leases
                    .insert(lease.terminal_session_id, lease.clone());
                Err(CoreCommandError::ControlLeaseDenied { reason, lease })
            }
            Response::Error(error) => Err(CoreCommandError::Message(error)),
            response => Err(CoreCommandError::Message(format!(
                "invalid paste response: {response:?}"
            ))),
        }
    }

    pub fn resize(&mut self, size: TerminalSize) -> Result<(), CoreCommandError> {
        let terminal_session_id = self
            .active_terminal_session_id
            .ok_or_else(|| CoreCommandError::Message("no active Terminal Session".into()))?;
        self.resize_terminal(terminal_session_id, size)
    }

    pub fn resize_terminal(
        &mut self,
        terminal_session_id: TerminalSessionId,
        size: TerminalSize,
    ) -> Result<(), CoreCommandError> {
        let lease_generation = self
            .control_leases
            .get(&terminal_session_id)
            .ok_or_else(|| {
                CoreCommandError::Message(format!(
                    "Terminal Session {} has no Control Lease",
                    terminal_session_id.as_u64()
                ))
            })?
            .generation;
        match self
            .request(Request::Resize {
                terminal_session_id,
                lease_generation,
                size,
            })
            .map_err(CoreCommandError::Message)?
        {
            Response::Ack => Ok(()),
            Response::ControlLeaseDenied { reason, lease } => {
                self.control_leases
                    .insert(lease.terminal_session_id, lease.clone());
                Err(CoreCommandError::ControlLeaseDenied { reason, lease })
            }
            Response::Error(error) => Err(CoreCommandError::Message(error)),
            response => Err(CoreCommandError::Message(format!(
                "invalid resize response: {response:?}"
            ))),
        }
    }

    pub fn refresh_control_lease(&mut self) -> Result<ControlLease, CoreCommandError> {
        let terminal_session_id = self
            .active_terminal_session_id
            .ok_or_else(|| CoreCommandError::Message("no active Terminal Session".into()))?;
        self.refresh_control_lease_for(terminal_session_id)
    }

    pub fn refresh_control_lease_for(
        &mut self,
        terminal_session_id: TerminalSessionId,
    ) -> Result<ControlLease, CoreCommandError> {
        match self
            .request(Request::ControlLease {
                terminal_session_id,
            })
            .map_err(CoreCommandError::Message)?
        {
            Response::ControlLease(lease) => {
                self.control_leases
                    .insert(lease.terminal_session_id, lease.clone());
                Ok(lease)
            }
            Response::Error(error) => Err(CoreCommandError::Message(error)),
            response => Err(CoreCommandError::Message(format!(
                "invalid Control Lease response: {response:?}"
            ))),
        }
    }

    pub fn transfer_control(
        &mut self,
        target: UiClientId,
    ) -> Result<ControlLease, CoreCommandError> {
        let terminal_session_id = self
            .active_terminal_session_id
            .ok_or_else(|| CoreCommandError::Message("no active Terminal Session".into()))?;
        self.transfer_terminal_control(terminal_session_id, target)
    }

    pub fn transfer_terminal_control(
        &mut self,
        terminal_session_id: TerminalSessionId,
        target: UiClientId,
    ) -> Result<ControlLease, CoreCommandError> {
        let lease_generation = self
            .control_leases
            .get(&terminal_session_id)
            .ok_or_else(|| CoreCommandError::Message("Control Lease is unavailable".into()))?
            .generation;
        match self
            .request(Request::TransferControl {
                terminal_session_id,
                lease_generation,
                target,
            })
            .map_err(CoreCommandError::Message)?
        {
            Response::ControlLease(lease) => {
                self.control_leases
                    .insert(lease.terminal_session_id, lease.clone());
                Ok(lease)
            }
            Response::ControlLeaseDenied { reason, lease } => {
                self.control_leases
                    .insert(lease.terminal_session_id, lease.clone());
                Err(CoreCommandError::ControlLeaseDenied { reason, lease })
            }
            Response::Error(error) => Err(CoreCommandError::Message(error)),
            response => Err(CoreCommandError::Message(format!(
                "invalid Control Lease transfer response: {response:?}"
            ))),
        }
    }

    pub fn acquire_control(&mut self) -> Result<ControlLease, CoreCommandError> {
        let terminal_session_id = self
            .active_terminal_session_id
            .ok_or_else(|| CoreCommandError::Message("no active Terminal Session".into()))?;
        self.acquire_terminal_control(terminal_session_id)
    }

    pub fn acquire_terminal_control(
        &mut self,
        terminal_session_id: TerminalSessionId,
    ) -> Result<ControlLease, CoreCommandError> {
        let lease_generation = self
            .control_leases
            .get(&terminal_session_id)
            .ok_or_else(|| CoreCommandError::Message("Control Lease is unavailable".into()))?
            .generation;
        match self
            .request(Request::AcquireControl {
                terminal_session_id,
                lease_generation,
            })
            .map_err(CoreCommandError::Message)?
        {
            Response::ControlLease(lease) => {
                self.control_leases
                    .insert(lease.terminal_session_id, lease.clone());
                Ok(lease)
            }
            Response::ControlLeaseDenied { reason, lease } => {
                self.control_leases
                    .insert(lease.terminal_session_id, lease.clone());
                Err(CoreCommandError::ControlLeaseDenied { reason, lease })
            }
            Response::Error(error) => Err(CoreCommandError::Message(error)),
            response => Err(CoreCommandError::Message(format!(
                "invalid Control Lease acquisition response: {response:?}"
            ))),
        }
    }

    pub fn snapshot(&mut self) -> Result<TerminalSnapshot, String> {
        let terminal_session_id = self
            .active_terminal_session_id
            .ok_or_else(|| "no active Terminal Session".to_string())?;
        self.terminal_snapshot(terminal_session_id)
    }

    pub fn terminal_snapshot(
        &mut self,
        terminal_session_id: TerminalSessionId,
    ) -> Result<TerminalSnapshot, String> {
        match self.request(Request::Snapshot {
            terminal_session_id,
            since: None,
        })? {
            Response::Snapshot(Some(update)) => self.accept_update(terminal_session_id, update),
            Response::Snapshot(None) => {
                Err("Resident Core omitted an unconditional snapshot".into())
            }
            Response::Error(error) => Err(error),
            response => Err(format!("invalid snapshot response: {response:?}")),
        }
    }

    pub fn snapshot_since(&mut self, revision: u64) -> Result<Option<TerminalSnapshot>, String> {
        let terminal_session_id = self
            .active_terminal_session_id
            .ok_or_else(|| "no active Terminal Session".to_string())?;
        self.terminal_snapshot_since(terminal_session_id, revision)
    }

    pub fn terminal_snapshot_since(
        &mut self,
        terminal_session_id: TerminalSessionId,
        revision: u64,
    ) -> Result<Option<TerminalSnapshot>, String> {
        if let Some(snapshot) = self.terminal_snapshots.get(&terminal_session_id)
            && snapshot.revision != revision
        {
            return Ok(Some(snapshot.clone()));
        }
        let since = self
            .terminal_snapshots
            .get(&terminal_session_id)
            .filter(|snapshot| snapshot.revision == revision)
            .map(|snapshot| snapshot.revision);
        match self.request(Request::Snapshot {
            terminal_session_id,
            since,
        })? {
            Response::Snapshot(Some(update)) => match self
                .accept_update(terminal_session_id, update)
            {
                Ok(snapshot) => Ok(Some(snapshot)),
                Err(_) => match self.request(Request::Snapshot {
                    terminal_session_id,
                    since: None,
                })? {
                    Response::Snapshot(Some(update)) => {
                        self.accept_update(terminal_session_id, update).map(Some)
                    }
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

    pub fn refresh_core_snapshot(&mut self) -> Result<CoreSnapshot, String> {
        match self.request(Request::CoreSnapshot)? {
            Response::CoreSnapshot(snapshot) => {
                self.accept_core_snapshot(snapshot.clone());
                Ok(snapshot)
            }
            Response::Error(error) => Err(error),
            response => Err(format!("invalid Core snapshot response: {response:?}")),
        }
    }

    pub fn apply_core_command(
        &mut self,
        command: CoreCommand,
    ) -> Result<CoreCommandOutcome, CoreCommandError> {
        match self
            .request(Request::ApplyCoreCommand {
                expected_revision: self.core_snapshot.revision,
                command,
            })
            .map_err(CoreCommandError::Message)?
        {
            Response::CoreCommandAccepted(outcome) => {
                self.accept_core_snapshot(outcome.snapshot.clone());
                for lease in &outcome.control_leases {
                    self.control_leases
                        .insert(lease.terminal_session_id, lease.clone());
                }
                Ok(outcome)
            }
            Response::CoreCommandRejected(error) => Err(CoreCommandError::Rejected(error)),
            Response::Error(error) => Err(CoreCommandError::Message(error)),
            response => Err(CoreCommandError::Message(format!(
                "invalid Core command response: {response:?}"
            ))),
        }
    }

    pub fn stop_resident_core(&mut self) -> Result<(), String> {
        match self.request(Request::StopResidentCore)? {
            Response::Ack => Ok(()),
            Response::Error(error) => Err(error),
            response => Err(format!("invalid stop response: {response:?}")),
        }
    }

    pub fn wait_for_terminal_change(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<TerminalChange>, String> {
        match self.terminal_changes.recv_timeout(timeout) {
            Ok(change) => Ok(Some(change)),
            Err(flume::RecvTimeoutError::Timeout) => Ok(None),
            Err(flume::RecvTimeoutError::Disconnected) => {
                Err("Resident Core disconnected while waiting for a terminal change".into())
            }
        }
    }

    pub fn wait_for_semantic_event(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<SemanticEvent>, String> {
        match self.semantic_events.recv_timeout(timeout) {
            Ok(event) => {
                self.accept_semantic_event(&event);
                Ok(Some(event))
            }
            Err(flume::RecvTimeoutError::Timeout) => Ok(None),
            Err(flume::RecvTimeoutError::Disconnected) => {
                Err("Resident Core semantic event stream requires reconnect and resnapshot".into())
            }
        }
    }

    pub(crate) fn terminal_changes(&self) -> flume::Receiver<TerminalChange> {
        self.terminal_changes.clone()
    }

    pub(crate) fn semantic_events(&self) -> flume::Receiver<SemanticEvent> {
        self.semantic_events.clone()
    }

    pub(crate) fn accept_semantic_event(&mut self, event: &SemanticEvent) {
        if let SemanticEventKind::ControlLeaseChanged { lease } = &event.kind {
            self.control_leases
                .insert(lease.terminal_session_id, lease.clone());
        }
    }

    fn request(&mut self, request: Request) -> Result<Response, String> {
        if let Ok(pending) = self.responses.try_recv() {
            return pending;
        }
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "Resident Core command writer mutex poisoned".to_string())?;
        if let Err(error) = wire::write_request(&mut *connection, &request) {
            drop(connection);
            if let Ok(pending) = self.responses.try_recv() {
                return pending;
            }
            return Err(format!("send Resident Core command: {error}"));
        }
        drop(connection);
        self.responses
            .recv()
            .map_err(|_| "Resident Core disconnected before responding".to_string())?
    }

    fn accept_update(
        &mut self,
        terminal_session_id: TerminalSessionId,
        update: TerminalUpdate,
    ) -> Result<TerminalSnapshot, String> {
        if update.base_revision.is_none() {
            self.terminal_snapshots
                .insert(terminal_session_id, TerminalSnapshot::from_update(update)?);
        } else {
            self.terminal_snapshots
                .get_mut(&terminal_session_id)
                .ok_or_else(|| "Resident Core sent a delta before a full snapshot".to_string())?
                .apply_update(update)?;
        }
        Ok(self
            .terminal_snapshots
            .get(&terminal_session_id)
            .expect("accepted update stores a snapshot")
            .clone())
    }

    fn accept_core_snapshot(&mut self, snapshot: CoreSnapshot) {
        let session_ids = snapshot
            .terminal_sessions
            .iter()
            .map(|session| session.id)
            .collect::<HashSet<_>>();
        self.terminal_snapshots
            .retain(|terminal_session_id, _| session_ids.contains(terminal_session_id));
        self.control_leases
            .retain(|terminal_session_id, _| session_ids.contains(terminal_session_id));
        if self
            .active_terminal_session_id
            .is_none_or(|terminal_session_id| !session_ids.contains(&terminal_session_id))
        {
            self.active_terminal_session_id =
                snapshot.terminal_sessions.first().map(|session| session.id);
        }
        self.core_snapshot = snapshot;
    }
}

impl Drop for CoreClient {
    fn drop(&mut self) {
        // The response reader owns a cloned stream. Ask the server to close its
        // end so that reader exits and the Control Lease is released promptly.
        if let Ok(mut connection) = self.connection.lock() {
            let _ = wire::write_request(&mut *connection, &Request::Detach);
        }
    }
}

fn read_core_responses(
    mut connection: BufReader<LocalSocketStream>,
    responses: flume::Sender<Result<Response, String>>,
    terminal_changes: Arc<Mutex<HashMap<TerminalSessionId, TerminalChange>>>,
    terminal_change_wake: flume::Sender<()>,
    semantic_events: flume::Sender<SemanticEvent>,
    mut semantic_sequence: u64,
    writer: Arc<Mutex<LocalSocketStream>>,
) {
    loop {
        match wire::read_response(&mut connection) {
            Ok(Some(Response::TerminalChanged(change))) => {
                terminal_changes
                    .lock()
                    .expect("UI terminal pending-map mutex poisoned")
                    .insert(change.terminal_session_id, change);
                let _ = terminal_change_wake.try_send(());
            }
            Ok(Some(Response::SemanticEvent(event))) => {
                let expected = semantic_sequence.saturating_add(1);
                if event.sequence != expected {
                    fail_response_reader(
                        &responses,
                        &writer,
                        format!(
                            "Resident Core semantic event gap: expected {expected}, received {}; reconnect and resnapshot required",
                            event.sequence
                        ),
                    );
                    return;
                }
                semantic_sequence = event.sequence;
                if semantic_events.try_send(event).is_err() {
                    fail_response_reader(
                        &responses,
                        &writer,
                        "Resident Core semantic event queue overflowed; reconnect and resnapshot required".into(),
                    );
                    return;
                }
            }
            Ok(Some(Response::ResnapshotRequired)) => {
                fail_response_reader(
                    &responses,
                    &writer,
                    "Resident Core semantic event delivery overflowed; reconnect and resnapshot required".into(),
                );
                return;
            }
            Ok(Some(response)) => {
                if responses.send(Ok(response)).is_err() {
                    return;
                }
            }
            Ok(None) => {
                let _ = responses.send(Err(
                    "Resident Core disconnected before responding".to_string()
                ));
                return;
            }
            Err(error) => {
                let _ = responses.send(Err(format!("receive Resident Core response: {error}")));
                return;
            }
        }
    }
}

fn dispatch_terminal_changes(
    wakes: flume::Receiver<()>,
    pending: Arc<Mutex<HashMap<TerminalSessionId, TerminalChange>>>,
    changes: flume::Sender<TerminalChange>,
) {
    while wakes.recv().is_ok() {
        let mut batch = pending
            .lock()
            .expect("UI terminal pending-map mutex poisoned")
            .drain()
            .map(|(_, change)| change)
            .collect::<Vec<_>>();
        batch.sort_unstable_by_key(|change| change.sequence);
        for change in batch {
            if changes.send(change).is_err() {
                return;
            }
        }
    }
}

fn fail_response_reader(
    responses: &flume::Sender<Result<Response, String>>,
    writer: &Arc<Mutex<LocalSocketStream>>,
    error: String,
) {
    let _ = responses.send(Err(error));
    if let Ok(mut writer) = writer.lock() {
        let _ = wire::write_request(&mut *writer, &Request::Detach);
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
                "Resident Core response deadline elapsed",
            ));
        }

        let read_capacity = expected_len
            .map(|expected_len| expected_len - bytes.len())
            .unwrap_or_else(|| 4 - bytes.len())
            .min(chunk.len());
        match reader.read(&mut chunk[..read_capacity]) {
            Ok(0) if bytes.is_empty() => return Ok(None),
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Resident Core disconnected inside its response frame",
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
                            "Resident Core response contains trailing frame bytes",
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
    let mut chunk = [0_u8; 8 * 1024];
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Resident Core response deadline elapsed",
            ));
        }

        let available = windows_available_bytes(reader.get_ref())?;
        if available > 0 {
            let remaining_capacity = (MAX_MESSAGE_BYTES + 4 - bytes.len() as u64) as usize;
            let frame_remaining = expected_len
                .map(|expected_len| expected_len - bytes.len())
                .unwrap_or_else(|| 4 - bytes.len());
            let read_capacity = available
                .min(remaining_capacity)
                .min(frame_remaining)
                .min(chunk.len());
            let read = std::io::Read::read(reader.get_mut(), &mut chunk[..read_capacity])?;
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
                        "Resident Core response contains trailing frame bytes",
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

    fn empty(revision: u64, lifecycle: TerminalLifecycle) -> Self {
        const COLS: u16 = 80;
        const ROWS: u16 = 24;
        Self {
            base_revision: None,
            revision,
            lifecycle,
            cols: COLS,
            rows: ROWS,
            cursor: None,
            default_fg: [0xd8, 0xde, 0xe9],
            default_bg: [0x0b, 0x0e, 0x13],
            dirty_rows: (0..ROWS).collect(),
            cells: Vec::new(),
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
        terminal_session_id: TerminalSessionId,
        lease_generation: u64,
        bytes: Vec<u8>,
    },
    Paste {
        terminal_session_id: TerminalSessionId,
        lease_generation: u64,
        bytes: Vec<u8>,
    },
    Resize {
        terminal_session_id: TerminalSessionId,
        lease_generation: u64,
        size: TerminalSize,
    },
    Snapshot {
        terminal_session_id: TerminalSessionId,
        since: Option<u64>,
    },
    CoreSnapshot,
    ApplyCoreCommand {
        expected_revision: u64,
        command: CoreCommand,
    },
    ControlLease {
        terminal_session_id: TerminalSessionId,
    },
    TransferControl {
        terminal_session_id: TerminalSessionId,
        lease_generation: u64,
        target: UiClientId,
    },
    AcquireControl {
        terminal_session_id: TerminalSessionId,
        lease_generation: u64,
    },
    Detach,
    StopResidentCore,
}

#[derive(Debug, PartialEq, Eq)]
enum Response {
    Ready {
        version: u16,
        proof: [u8; 32],
        client_id: UiClientId,
        snapshot: CoreSnapshot,
        leases: Vec<ControlLease>,
        semantic_sequence: u64,
    },
    Ack,
    Snapshot(Option<TerminalUpdate>),
    CoreSnapshot(CoreSnapshot),
    CoreCommandAccepted(CoreCommandOutcome),
    CoreCommandRejected(CoreModelError),
    TerminalChanged(TerminalChange),
    SemanticEvent(SemanticEvent),
    ResnapshotRequired,
    ControlLease(ControlLease),
    ControlLeaseDenied {
        reason: ControlLeaseDenial,
        lease: ControlLease,
    },
    Error(String),
}

enum WorkerRequest {
    Input {
        terminal_session_id: TerminalSessionId,
        bytes: Vec<u8>,
    },
    Paste {
        terminal_session_id: TerminalSessionId,
        bytes: Vec<u8>,
    },
    Resize {
        terminal_session_id: TerminalSessionId,
        size: TerminalSize,
    },
    Snapshot {
        terminal_session_id: TerminalSessionId,
        since: Option<u64>,
    },
    CoreSnapshot,
    ApplyCoreCommand {
        expected_revision: u64,
        command: CoreCommand,
    },
    Stop,
}

enum WorkerResponse {
    Ack,
    Snapshot(Option<TerminalUpdate>),
    CoreSnapshot(CoreSnapshot),
    CoreCommandAccepted(CoreCommandOutcome),
    CoreCommandRejected(CoreModelError),
}

struct WorkerCommand {
    request: WorkerRequest,
    response: flume::Sender<Result<WorkerResponse, String>>,
}

struct ResidentCore {
    commands: flume::Sender<WorkerCommand>,
    model_commands: Mutex<()>,
    subscribers: Arc<Mutex<Vec<TerminalSubscriber>>>,
    next_subscriber_id: AtomicU64,
    control: Arc<Mutex<ControlState>>,
    semantic: Arc<Mutex<SemanticState>>,
    next_client_id: AtomicU64,
}

struct ControlState {
    connected: HashSet<UiClientId>,
    leases: HashMap<TerminalSessionId, ControlLease>,
}

struct ClientRegistration {
    id: UiClientId,
    control: Arc<Mutex<ControlState>>,
    semantic: Arc<Mutex<SemanticState>>,
}

impl Drop for ClientRegistration {
    fn drop(&mut self) {
        let mut control = self
            .control
            .lock()
            .expect("Resident Core Control Lease mutex poisoned");
        control.connected.remove(&self.id);
        let changed = control
            .leases
            .values_mut()
            .filter_map(|lease| {
                if lease.controller == Some(self.id) {
                    lease.generation = lease.generation.saturating_add(1);
                    lease.controller = None;
                    Some(lease.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        drop(control);
        for lease in changed {
            publish_semantic_event(
                &self.semantic,
                SemanticEventKind::ControlLeaseChanged { lease },
            );
        }
    }
}

struct SemanticState {
    sequence: u64,
    subscribers: Vec<SemanticSubscriber>,
    next_subscriber_id: u64,
}

struct SemanticSubscriber {
    id: u64,
    sender: flume::Sender<SemanticEvent>,
    overflowed: Arc<AtomicBool>,
}

struct SemanticSubscription {
    id: u64,
    state: Arc<Mutex<SemanticState>>,
}

impl Drop for SemanticSubscription {
    fn drop(&mut self) {
        self.state
            .lock()
            .expect("Resident Core semantic subscriber mutex poisoned")
            .subscribers
            .retain(|subscriber| subscriber.id != self.id);
    }
}

struct TerminalSubscriber {
    id: u64,
    wake: flume::Sender<()>,
    pending: Arc<Mutex<HashMap<TerminalSessionId, TerminalChange>>>,
}

struct TerminalSubscription {
    id: u64,
    subscribers: Arc<Mutex<Vec<TerminalSubscriber>>>,
}

struct TerminalChangeSubscription {
    wakes: flume::Receiver<()>,
    pending: Arc<Mutex<HashMap<TerminalSessionId, TerminalChange>>>,
    registration: TerminalSubscription,
}

impl Drop for TerminalSubscription {
    fn drop(&mut self) {
        self.subscribers
            .lock()
            .expect("Resident Core subscriber mutex poisoned")
            .retain(|subscriber| subscriber.id != self.id);
    }
}

impl ResidentCore {
    fn start(_endpoint: &CoreEndpoint) -> Result<Self, String> {
        let first_resource_id = fresh_core_resource_id()?;
        let (commands_tx, commands_rx) = flume::bounded::<WorkerCommand>(32);
        let (ready_tx, ready_rx) = flume::bounded(1);
        let subscribers = Arc::new(Mutex::new(Vec::new()));
        let worker_subscribers = Arc::clone(&subscribers);
        let semantic = Arc::new(Mutex::new(SemanticState {
            sequence: 0,
            subscribers: Vec::new(),
            next_subscriber_id: 1,
        }));
        let worker_semantic = Arc::clone(&semantic);
        thread::Builder::new()
            .name("resident-core-terminal".into())
            .spawn(move || {
                let started = std::env::current_dir()
                    .map_err(|error| format!("resolve initial Space directory: {error}"))
                    .and_then(|directory| CoreRuntime::start(&directory, first_resource_id));
                let mut runtime = match started {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(runtime.model_snapshot()));
                let mut event_sequence = 0_u64;

                loop {
                    publish_runtime_events(
                        runtime.refresh(),
                        &worker_subscribers,
                        &mut event_sequence,
                        &worker_semantic,
                    );
                    match commands_rx.recv_timeout(SESSION_TICK) {
                        Ok(command) => {
                            // A terminal may exit while this worker is blocked waiting for
                            // the next command. Reconcile that exit before touching its
                            // transport so an already queued resize or input becomes a
                            // harmless no-op instead of a closed-pipe error.
                            publish_runtime_events(
                                runtime.refresh(),
                                &worker_subscribers,
                                &mut event_sequence,
                                &worker_semantic,
                            );
                            let stop = matches!(command.request, WorkerRequest::Stop);
                            let mut changed_terminal = None;
                            let result = match command.request {
                                WorkerRequest::Input {
                                    terminal_session_id,
                                    bytes,
                                } => {
                                    if runtime.contains_terminal(terminal_session_id) {
                                        runtime
                                            .input(terminal_session_id, &bytes)
                                            .map(|()| WorkerResponse::Ack)
                                    } else {
                                        Ok(WorkerResponse::Ack)
                                    }
                                }
                                WorkerRequest::Paste {
                                    terminal_session_id,
                                    bytes,
                                } => {
                                    if runtime.contains_terminal(terminal_session_id) {
                                        match runtime.paste(terminal_session_id, &bytes) {
                                            Ok(changed) => {
                                                if changed {
                                                    changed_terminal = runtime
                                                        .terminal_revision(terminal_session_id)
                                                        .ok()
                                                        .map(|revision| {
                                                            (terminal_session_id, revision)
                                                        });
                                                }
                                                Ok(WorkerResponse::Ack)
                                            }
                                            Err(error) => Err(error),
                                        }
                                    } else {
                                        Ok(WorkerResponse::Ack)
                                    }
                                }
                                WorkerRequest::Resize {
                                    terminal_session_id,
                                    size,
                                } => {
                                    if runtime.contains_terminal(terminal_session_id) {
                                        match runtime.resize(terminal_session_id, size) {
                                            Ok(changed) => {
                                                if changed {
                                                    changed_terminal = runtime
                                                        .terminal_revision(terminal_session_id)
                                                        .ok()
                                                        .map(|revision| {
                                                            (terminal_session_id, revision)
                                                        });
                                                }
                                                Ok(WorkerResponse::Ack)
                                            }
                                            Err(error) => Err(error),
                                        }
                                    } else {
                                        Ok(WorkerResponse::Ack)
                                    }
                                }
                                WorkerRequest::Snapshot {
                                    terminal_session_id,
                                    since,
                                } => match runtime.snapshot(terminal_session_id, since) {
                                    Ok(snapshot) => Ok(WorkerResponse::Snapshot(snapshot)),
                                    Err(_)
                                        if since.is_some()
                                            && !runtime.contains_terminal(terminal_session_id) =>
                                    {
                                        // A visual invalidation may already be in a UI Client's
                                        // stream when natural exit removes the Terminal Session.
                                        // Conditional snapshots are coalescible wake follow-ups,
                                        // so removal supersedes the announced revision rather
                                        // than turning normal exit into a persistent UI error.
                                        Ok(WorkerResponse::Snapshot(None))
                                    }
                                    Err(error) => Err(error),
                                },
                                WorkerRequest::CoreSnapshot => {
                                    Ok(WorkerResponse::CoreSnapshot(runtime.model_snapshot()))
                                }
                                WorkerRequest::ApplyCoreCommand {
                                    expected_revision,
                                    command,
                                } => match runtime.apply(expected_revision, command) {
                                    Ok(commit) => Ok(WorkerResponse::CoreCommandAccepted(
                                        CoreCommandOutcome {
                                            revision: commit.revision,
                                            snapshot: commit.snapshot,
                                            created: commit.created,
                                            control_leases: Vec::new(),
                                        },
                                    )),
                                    Err(error) => Ok(WorkerResponse::CoreCommandRejected(error)),
                                },
                                WorkerRequest::Stop => Ok(WorkerResponse::Ack),
                            };
                            if let Some((terminal_session_id, revision)) = changed_terminal {
                                publish_terminal_change(
                                    &worker_subscribers,
                                    &mut event_sequence,
                                    terminal_session_id,
                                    revision,
                                );
                            }
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
        let initial_snapshot = ready_rx
            .recv()
            .map_err(|_| "Resident Core terminal thread stopped during startup".to_string())??;
        Ok(Self {
            commands: commands_tx,
            model_commands: Mutex::new(()),
            subscribers,
            next_subscriber_id: AtomicU64::new(1),
            control: Arc::new(Mutex::new(ControlState {
                connected: HashSet::new(),
                leases: initial_snapshot
                    .terminal_sessions
                    .into_iter()
                    .map(|session| {
                        (
                            session.id,
                            ControlLease {
                                terminal_session_id: session.id,
                                generation: 0,
                                controller: None,
                            },
                        )
                    })
                    .collect(),
            })),
            semantic,
            next_client_id: AtomicU64::new(1),
        })
    }

    fn attach_client(&self) -> ClientRegistration {
        let id = UiClientId(self.next_client_id.fetch_add(1, Ordering::Relaxed));
        let mut control = self
            .control
            .lock()
            .expect("Resident Core Control Lease mutex poisoned");
        let first_connection = control.connected.is_empty();
        control.connected.insert(id);
        let changed = if first_connection {
            control
                .leases
                .values_mut()
                .filter_map(|lease| {
                    if lease.controller.is_none() {
                        lease.generation = lease.generation.saturating_add(1);
                        lease.controller = Some(id);
                        Some(lease.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        drop(control);
        for lease in changed {
            publish_semantic_event(
                &self.semantic,
                SemanticEventKind::ControlLeaseChanged { lease },
            );
        }
        ClientRegistration {
            id,
            control: Arc::clone(&self.control),
            semantic: Arc::clone(&self.semantic),
        }
    }

    fn control_lease(&self, terminal_session_id: TerminalSessionId) -> Option<ControlLease> {
        self.control
            .lock()
            .expect("Resident Core Control Lease mutex poisoned")
            .leases
            .get(&terminal_session_id)
            .cloned()
    }

    fn controlled_call(
        &self,
        client_id: UiClientId,
        terminal_session_id: TerminalSessionId,
        lease_generation: u64,
        request: WorkerRequest,
    ) -> Response {
        let control = self
            .control
            .lock()
            .expect("Resident Core Control Lease mutex poisoned");
        let Some(lease) = control.leases.get(&terminal_session_id) else {
            return Response::Error(format!(
                "Terminal Session {} does not exist",
                terminal_session_id.as_u64()
            ));
        };
        let reason = if lease.controller.is_none() {
            Some(ControlLeaseDenial::NoController)
        } else if lease.controller != Some(client_id) {
            Some(ControlLeaseDenial::HeldByOther)
        } else if lease.generation != lease_generation {
            Some(ControlLeaseDenial::StaleGeneration)
        } else {
            None
        };
        if let Some(reason) = reason {
            return Response::ControlLeaseDenied {
                reason,
                lease: lease.clone(),
            };
        }

        // Keep the lease stable until this command is acknowledged by the
        // terminal worker, so transfer cannot overtake authorized input/resize.
        worker_response(self.call(request))
    }

    fn transfer_control(
        &self,
        client_id: UiClientId,
        terminal_session_id: TerminalSessionId,
        lease_generation: u64,
        target: UiClientId,
    ) -> Response {
        let mut control = self
            .control
            .lock()
            .expect("Resident Core Control Lease mutex poisoned");
        let connected_target = control.connected.contains(&target);
        let Some(lease) = control.leases.get_mut(&terminal_session_id) else {
            return Response::Error(format!(
                "Terminal Session {} does not exist",
                terminal_session_id.as_u64()
            ));
        };
        let reason = if lease.controller.is_none() {
            Some(ControlLeaseDenial::NoController)
        } else if lease.controller != Some(client_id) {
            Some(ControlLeaseDenial::HeldByOther)
        } else if lease.generation != lease_generation {
            Some(ControlLeaseDenial::StaleGeneration)
        } else if !connected_target {
            Some(ControlLeaseDenial::TargetUnavailable)
        } else {
            None
        };
        if let Some(reason) = reason {
            return Response::ControlLeaseDenied {
                reason,
                lease: lease.clone(),
            };
        }

        lease.generation = lease.generation.saturating_add(1);
        lease.controller = Some(target);
        let lease = lease.clone();
        drop(control);
        publish_semantic_event(
            &self.semantic,
            SemanticEventKind::ControlLeaseChanged {
                lease: lease.clone(),
            },
        );
        Response::ControlLease(lease)
    }

    fn acquire_control(
        &self,
        client_id: UiClientId,
        terminal_session_id: TerminalSessionId,
        lease_generation: u64,
    ) -> Response {
        let mut control = self
            .control
            .lock()
            .expect("Resident Core Control Lease mutex poisoned");
        let Some(lease) = control.leases.get_mut(&terminal_session_id) else {
            return Response::Error(format!(
                "Terminal Session {} does not exist",
                terminal_session_id.as_u64()
            ));
        };
        let reason = if lease.controller.is_some() {
            Some(ControlLeaseDenial::HeldByOther)
        } else if lease.generation != lease_generation {
            Some(ControlLeaseDenial::StaleGeneration)
        } else {
            None
        };
        if let Some(reason) = reason {
            return Response::ControlLeaseDenied {
                reason,
                lease: lease.clone(),
            };
        }

        lease.generation = lease.generation.saturating_add(1);
        lease.controller = Some(client_id);
        let lease = lease.clone();
        drop(control);
        publish_semantic_event(
            &self.semantic,
            SemanticEventKind::ControlLeaseChanged {
                lease: lease.clone(),
            },
        );
        Response::ControlLease(lease)
    }

    fn apply_core_command(
        &self,
        client_id: UiClientId,
        expected_revision: u64,
        command: CoreCommand,
    ) -> Response {
        // Keep model mutation, runtime effects, lease reconciliation, and the
        // acknowledged hierarchy event in one service-level order even when
        // multiple UI Client handler threads submit concurrently.
        let _model_command = self
            .model_commands
            .lock()
            .expect("Resident Core model command mutex poisoned");
        let result = self.call(WorkerRequest::ApplyCoreCommand {
            expected_revision,
            command,
        });
        let mut outcome = match result {
            Ok(WorkerResponse::CoreCommandAccepted(outcome)) => outcome,
            Ok(WorkerResponse::CoreCommandRejected(error)) => {
                return Response::CoreCommandRejected(error);
            }
            Ok(_) => return Response::Error("invalid Core command worker response".into()),
            Err(error) => return Response::Error(error),
        };

        let session_ids = outcome
            .snapshot
            .terminal_sessions
            .iter()
            .map(|session| session.id)
            .collect::<HashSet<_>>();
        let mut created_leases = Vec::new();
        let mut control = self
            .control
            .lock()
            .expect("Resident Core Control Lease mutex poisoned");
        control
            .leases
            .retain(|terminal_session_id, _| session_ids.contains(terminal_session_id));
        for terminal_session_id in session_ids {
            control
                .leases
                .entry(terminal_session_id)
                .or_insert_with(|| {
                    let lease = ControlLease {
                        terminal_session_id,
                        generation: 1,
                        controller: Some(client_id),
                    };
                    created_leases.push(lease.clone());
                    lease
                });
        }
        outcome.control_leases = control.leases.values().cloned().collect();
        outcome
            .control_leases
            .sort_unstable_by_key(|lease| lease.terminal_session_id);
        drop(control);

        publish_semantic_event(
            &self.semantic,
            SemanticEventKind::HierarchyChanged {
                revision: outcome.revision,
            },
        );
        for lease in created_leases {
            publish_semantic_event(
                &self.semantic,
                SemanticEventKind::ControlLeaseChanged { lease },
            );
        }
        Response::CoreCommandAccepted(outcome)
    }

    fn hierarchy_snapshot(&self) -> Result<(CoreSnapshot, Vec<ControlLease>), String> {
        let _model_command = self
            .model_commands
            .lock()
            .expect("Resident Core model command mutex poisoned");
        let snapshot = match self.call(WorkerRequest::CoreSnapshot)? {
            WorkerResponse::CoreSnapshot(snapshot) => snapshot,
            _ => return Err("invalid Core snapshot worker response".into()),
        };
        let session_ids = snapshot
            .terminal_sessions
            .iter()
            .map(|session| session.id)
            .collect::<HashSet<_>>();
        let mut control = self
            .control
            .lock()
            .expect("Resident Core Control Lease mutex poisoned");
        control
            .leases
            .retain(|terminal_session_id, _| session_ids.contains(terminal_session_id));
        let mut leases = control.leases.values().cloned().collect::<Vec<_>>();
        leases.sort_unstable_by_key(|lease| lease.terminal_session_id);
        Ok((snapshot, leases))
    }

    fn subscribe_semantic(
        &self,
    ) -> (
        flume::Receiver<SemanticEvent>,
        SemanticSubscription,
        Arc<AtomicBool>,
        u64,
    ) {
        let (sender, receiver) = flume::bounded(SEMANTIC_EVENT_CAPACITY);
        let overflowed = Arc::new(AtomicBool::new(false));
        let mut state = self
            .semantic
            .lock()
            .expect("Resident Core semantic subscriber mutex poisoned");
        let id = state.next_subscriber_id;
        state.next_subscriber_id = state.next_subscriber_id.saturating_add(1);
        let sequence = state.sequence;
        state.subscribers.push(SemanticSubscriber {
            id,
            sender,
            overflowed: Arc::clone(&overflowed),
        });
        drop(state);
        (
            receiver,
            SemanticSubscription {
                id,
                state: Arc::clone(&self.semantic),
            },
            overflowed,
            sequence,
        )
    }

    fn subscribe(&self) -> TerminalChangeSubscription {
        let (wake, receiver) = flume::bounded(1);
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let id = self.next_subscriber_id.fetch_add(1, Ordering::Relaxed);
        self.subscribers
            .lock()
            .expect("Resident Core subscriber mutex poisoned")
            .push(TerminalSubscriber {
                id,
                wake,
                pending: Arc::clone(&pending),
            });
        TerminalChangeSubscription {
            wakes: receiver,
            pending,
            registration: TerminalSubscription {
                id,
                subscribers: Arc::clone(&self.subscribers),
            },
        }
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

fn publish_runtime_events(
    events: Vec<RuntimeEvent>,
    subscribers: &Arc<Mutex<Vec<TerminalSubscriber>>>,
    terminal_sequence: &mut u64,
    semantic: &Arc<Mutex<SemanticState>>,
) {
    for event in events {
        match event {
            RuntimeEvent::TerminalLifecycleChanged {
                terminal_session_id,
                lifecycle,
                terminal_revision,
            } => publish_semantic_event(
                semantic,
                SemanticEventKind::TerminalLifecycleChanged {
                    terminal_session_id,
                    lifecycle,
                    terminal_revision,
                },
            ),
            RuntimeEvent::TerminalChanged {
                terminal_session_id,
                terminal_revision,
            } => publish_terminal_change(
                subscribers,
                terminal_sequence,
                terminal_session_id,
                terminal_revision,
            ),
            RuntimeEvent::PaneClosed {
                terminal_session_id,
                revision,
            } => {
                for subscriber in subscribers
                    .lock()
                    .expect("Resident Core subscriber mutex poisoned")
                    .iter()
                {
                    subscriber
                        .pending
                        .lock()
                        .expect("Resident Core pending-change mutex poisoned")
                        .remove(&terminal_session_id);
                }
                publish_semantic_event(semantic, SemanticEventKind::HierarchyChanged { revision });
            }
        }
    }
}

fn publish_semantic_event(state: &Arc<Mutex<SemanticState>>, kind: SemanticEventKind) {
    let mut state = state
        .lock()
        .expect("Resident Core semantic subscriber mutex poisoned");
    state.sequence = state.sequence.saturating_add(1);
    let event = SemanticEvent {
        sequence: state.sequence,
        kind,
    };
    state.subscribers.retain(
        |subscriber| match subscriber.sender.try_send(event.clone()) {
            Ok(()) => true,
            Err(flume::TrySendError::Full(_)) => {
                subscriber.overflowed.store(true, Ordering::Release);
                false
            }
            Err(flume::TrySendError::Disconnected(_)) => false,
        },
    );
}

fn publish_terminal_change(
    subscribers: &Arc<Mutex<Vec<TerminalSubscriber>>>,
    sequence: &mut u64,
    terminal_session_id: TerminalSessionId,
    terminal_revision: u64,
) {
    *sequence = sequence.saturating_add(1);
    let change = TerminalChange {
        sequence: *sequence,
        terminal_session_id,
        terminal_revision,
    };
    subscribers
        .lock()
        .expect("Resident Core subscriber mutex poisoned")
        .retain(|subscriber| {
            subscriber
                .pending
                .lock()
                .expect("Resident Core terminal pending-map mutex poisoned")
                .insert(terminal_session_id, change.clone());
            !matches!(
                subscriber.wake.try_send(()),
                Err(flume::TrySendError::Disconnected(_))
            )
        });
}

pub fn run_resident_core(endpoint: CoreEndpoint) -> Result<(), String> {
    let auth_secret = load_auth_secret()?;
    let Some((listener, _endpoint_guard)) = create_listener(&endpoint, &auth_secret)? else {
        return Ok(());
    };
    listener
        .set_nonblocking(ListenerNonblockingMode::Accept)
        .map_err(|error| format!("configure Resident Core listener: {error}"))?;
    let core = Arc::new(ResidentCore::start(&endpoint)?);
    let stopping = Arc::new(AtomicBool::new(false));

    while !stopping.load(Ordering::Acquire) {
        match listener.accept() {
            Ok(stream) => {
                // BSD-derived Unix sockets may inherit O_NONBLOCK from the
                // listener even when the cross-platform listener requests
                // blocking streams. Client handlers use blocking framed I/O.
                stream
                    .set_nonblocking(false)
                    .map_err(|error| format!("configure UI Client stream: {error}"))?;
                if !same_user(&stream)? {
                    continue;
                }
                let core = Arc::clone(&core);
                let endpoint = endpoint.clone();
                let stopping = Arc::clone(&stopping);
                thread::Builder::new()
                    .name("resident-core-client".into())
                    .spawn(
                        move || match handle_client(stream, &core, &endpoint, &auth_secret) {
                            Ok(ClientOutcome::Disconnected) => {}
                            Ok(ClientOutcome::StopResidentCore) => {
                                stopping.store(true, Ordering::Release);
                            }
                            Err(error) if is_disconnect(&error) => {}
                            Err(error) => eprintln!("UI Client connection failed: {error}"),
                        },
                    )
                    .map_err(|error| format!("spawn UI Client handler: {error}"))?;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(format!("accept UI Client: {error}")),
        }
    }
    Ok(())
}

pub fn stop_resident_core_after_parent(endpoint: CoreEndpoint) -> Result<(), String> {
    let mut parent_lifetime = std::io::stdin().lock();
    let mut buffer = [0_u8; 256];
    loop {
        match parent_lifetime.read(&mut buffer) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(format!("wait for Desktop Shell exit: {error}")),
        }
    }

    let mut core = CoreClient::connect(&endpoint, Duration::from_secs(10))?;
    core.stop_resident_core()
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
    let reader = stream.try_clone()?;
    let writer = Arc::new(Mutex::new(stream));
    let mut connection = BufReader::new(reader);
    let Some(Request::Hello { version, nonce }) = wire::read_request(&mut connection)? else {
        write_shared_response(
            &writer,
            &Response::Error("UI Client must begin with a protocol handshake".into()),
        )?;
        return Ok(ClientOutcome::Disconnected);
    };
    if version != PROTOCOL_VERSION {
        write_shared_response(
            &writer,
            &Response::Error(format!(
                "Resident Core protocol mismatch: client {version}, core {PROTOCOL_VERSION}"
            )),
        )?;
        return Ok(ClientOutcome::Disconnected);
    }
    let registration = core.attach_client();
    let client_id = registration.id;
    let (semantic_events, _semantic_subscription, semantic_overflowed, semantic_sequence) =
        core.subscribe_semantic();
    let (snapshot, leases) = match core.hierarchy_snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            write_shared_response(&writer, &Response::Error(error))?;
            return Ok(ClientOutcome::Disconnected);
        }
    };
    write_shared_response(
        &writer,
        &Response::Ready {
            version: PROTOCOL_VERSION,
            proof: server_proof(auth_secret, endpoint, &nonce),
            client_id,
            snapshot,
            leases,
            semantic_sequence,
        },
    )?;
    let TerminalChangeSubscription {
        wakes: change_wakes,
        pending: pending_changes,
        registration: _terminal_subscription,
    } = core.subscribe();
    let change_writer = Arc::clone(&writer);
    thread::Builder::new()
        .name("resident-core-client-events".into())
        .spawn(move || {
            while change_wakes.recv().is_ok() {
                let mut changes = pending_changes
                    .lock()
                    .expect("Resident Core terminal pending-map mutex poisoned")
                    .drain()
                    .map(|(_, change)| change)
                    .collect::<Vec<_>>();
                changes.sort_unstable_by_key(|change| change.sequence);
                for change in changes {
                    if write_shared_response(&change_writer, &Response::TerminalChanged(change))
                        .is_err()
                    {
                        return;
                    }
                }
            }
        })?;
    let semantic_writer = Arc::clone(&writer);
    thread::Builder::new()
        .name("resident-core-client-semantic-events".into())
        .spawn(move || {
            while let Ok(event) = semantic_events.recv() {
                if write_shared_response(&semantic_writer, &Response::SemanticEvent(event)).is_err()
                {
                    return;
                }
            }
            if semantic_overflowed.load(Ordering::Acquire) {
                let _ = write_shared_response(&semantic_writer, &Response::ResnapshotRequired);
            }
        })?;

    loop {
        let Some(request) = wire::read_request(&mut connection)? else {
            return Ok(ClientOutcome::Disconnected);
        };
        let (response, outcome) = match request {
            Request::Hello { .. } => (
                Response::Error("UI Client already completed its handshake".into()),
                None,
            ),
            Request::Input { bytes, .. } if bytes.len() > 1024 * 1024 => (
                Response::Error("terminal input command exceeds 1 MiB".into()),
                None,
            ),
            Request::Paste { bytes, .. } if bytes.len() > 1024 * 1024 => (
                Response::Error("terminal paste command exceeds 1 MiB".into()),
                None,
            ),
            Request::Input {
                terminal_session_id,
                lease_generation,
                bytes,
            } => (
                core.controlled_call(
                    client_id,
                    terminal_session_id,
                    lease_generation,
                    WorkerRequest::Input {
                        terminal_session_id,
                        bytes,
                    },
                ),
                None,
            ),
            Request::Paste {
                terminal_session_id,
                lease_generation,
                bytes,
            } => (
                core.controlled_call(
                    client_id,
                    terminal_session_id,
                    lease_generation,
                    WorkerRequest::Paste {
                        terminal_session_id,
                        bytes,
                    },
                ),
                None,
            ),
            Request::Resize {
                terminal_session_id,
                lease_generation,
                size,
            } => (
                core.controlled_call(
                    client_id,
                    terminal_session_id,
                    lease_generation,
                    WorkerRequest::Resize {
                        terminal_session_id,
                        size,
                    },
                ),
                None,
            ),
            Request::Snapshot {
                terminal_session_id,
                since,
            } => (
                worker_response(core.call(WorkerRequest::Snapshot {
                    terminal_session_id,
                    since,
                })),
                None,
            ),
            Request::CoreSnapshot => match core.hierarchy_snapshot() {
                Ok((snapshot, _)) => (Response::CoreSnapshot(snapshot), None),
                Err(error) => (Response::Error(error), None),
            },
            Request::ApplyCoreCommand {
                expected_revision,
                command,
            } => (
                core.apply_core_command(client_id, expected_revision, command),
                None,
            ),
            Request::ControlLease {
                terminal_session_id,
            } => (
                core.control_lease(terminal_session_id)
                    .map(Response::ControlLease)
                    .unwrap_or_else(|| {
                        Response::Error(format!(
                            "Terminal Session {} does not exist",
                            terminal_session_id.as_u64()
                        ))
                    }),
                None,
            ),
            Request::TransferControl {
                terminal_session_id,
                lease_generation,
                target,
            } => (
                core.transfer_control(client_id, terminal_session_id, lease_generation, target),
                None,
            ),
            Request::AcquireControl {
                terminal_session_id,
                lease_generation,
            } => (
                core.acquire_control(client_id, terminal_session_id, lease_generation),
                None,
            ),
            Request::Detach => (Response::Ack, Some(ClientOutcome::Disconnected)),
            Request::StopResidentCore => (
                worker_response(core.call(WorkerRequest::Stop)),
                Some(ClientOutcome::StopResidentCore),
            ),
        };
        write_shared_response(&writer, &response)?;
        if let Some(outcome) = outcome {
            return Ok(outcome);
        }
    }
}

fn write_shared_response(
    writer: &Arc<Mutex<LocalSocketStream>>,
    response: &Response,
) -> io::Result<()> {
    let mut writer = writer
        .lock()
        .map_err(|_| io::Error::other("Resident Core response writer mutex poisoned"))?;
    wire::write_response(&mut *writer, response)
}

fn worker_response(response: Result<WorkerResponse, String>) -> Response {
    match response {
        Ok(WorkerResponse::Ack) => Response::Ack,
        Ok(WorkerResponse::Snapshot(snapshot)) => Response::Snapshot(snapshot),
        Ok(WorkerResponse::CoreSnapshot(snapshot)) => Response::CoreSnapshot(snapshot),
        Ok(WorkerResponse::CoreCommandAccepted(outcome)) => Response::CoreCommandAccepted(outcome),
        Ok(WorkerResponse::CoreCommandRejected(error)) => Response::CoreCommandRejected(error),
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
    command.creation_flags(WINDOWS_RESIDENT_CORE_CREATION_FLAGS);
}

#[cfg(windows)]
const WINDOWS_RESIDENT_CORE_CREATION_FLAGS: u32 =
    windows_sys::Win32::System::Threading::DETACHED_PROCESS
        | windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

#[cfg(test)]
mod tests {
    use super::{
        CoreEndpoint, PROTOCOL_VERSION, ResidentCore, TerminalCell, TerminalLifecycle,
        TerminalSnapshot, TerminalUpdate, WorkerRequest, WorkerResponse, protected_endpoint_name,
        read_response_until, server_proof, verify_server_proof,
    };
    use crate::CoreCommand;

    struct TransientWouldBlock {
        bytes: std::io::Cursor<Vec<u8>>,
        would_block_next: bool,
    }

    #[test]
    fn development_launches_are_isolated_from_default_and_each_other() {
        let default = CoreEndpoint::for_current_user().expect("default endpoint");
        let first = CoreEndpoint::for_development_launch().expect("first development endpoint");
        let second = CoreEndpoint::for_development_launch().expect("second development endpoint");

        assert_ne!(first, default);
        assert_ne!(first, second);
        assert!(first.argument().contains("-development-"));
    }

    #[test]
    fn removing_layout_persistence_preserves_running_v8_core_compatibility() {
        assert_eq!(
            PROTOCOL_VERSION, 8,
            "a Desktop Shell upgrade must still attach to an already-running v8 Resident Core"
        );
    }

    #[cfg(windows)]
    #[test]
    fn resident_core_spawn_does_not_disable_ctrl_c_for_terminal_descendants() {
        use windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;

        assert_eq!(
            super::WINDOWS_RESIDENT_CORE_CREATION_FLAGS & CREATE_NEW_PROCESS_GROUP,
            0,
            "the detached Resident Core must not make every ConPTY descendant ignore Ctrl+C"
        );
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
            client_id: super::UiClientId(1),
            snapshot: crate::CoreSnapshot {
                revision: 0,
                spaces: Vec::new(),
                terminal_sessions: Vec::new(),
            },
            leases: vec![super::ControlLease {
                terminal_session_id: crate::TerminalSessionId::from_u64(1),
                generation: 1,
                controller: Some(super::UiClientId(1)),
            }],
            semantic_sequence: 1,
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

    #[test]
    fn resident_core_cold_restart_starts_with_a_fresh_hierarchy() {
        let endpoint = CoreEndpoint::for_profile("cold-restart-fresh").expect("test endpoint");
        let first = ResidentCore::start(&endpoint).expect("start first Resident Core");
        let registration = first.attach_client();
        let (initial, _) = first.hierarchy_snapshot().expect("initial hierarchy");
        let accepted = first.apply_core_command(
            registration.id,
            initial.revision,
            CoreCommand::CreateSpace {
                name: "Restored Space".into(),
                directory: std::env::current_dir().expect("current directory"),
            },
        );
        assert!(matches!(accepted, super::Response::CoreCommandAccepted(_)));
        let (before_restart, _) = first.hierarchy_snapshot().expect("mutated hierarchy");
        assert_eq!(before_restart.spaces.len(), 2);
        assert!(matches!(
            first.call(WorkerRequest::Stop),
            Ok(WorkerResponse::Ack)
        ));
        drop(registration);
        drop(first);

        let second = ResidentCore::start(&endpoint).expect("restart Resident Core");
        let (after_restart, _) = second.hierarchy_snapshot().expect("fresh hierarchy");

        assert_eq!(after_restart.spaces.len(), 1);
        assert_eq!(after_restart.spaces[0].tabs.len(), 1);
        assert_eq!(after_restart.terminal_sessions.len(), 1);
        let old_terminal_ids = before_restart
            .terminal_sessions
            .iter()
            .map(|terminal| terminal.id)
            .collect::<std::collections::HashSet<_>>();
        let new_terminal_ids = after_restart
            .terminal_sessions
            .iter()
            .map(|terminal| terminal.id)
            .collect::<std::collections::HashSet<_>>();
        assert!(
            old_terminal_ids.is_disjoint(&new_terminal_ids),
            "a cold Core must not reuse Terminal Session identities from its predecessor"
        );
        let old_initial_pane = match &before_restart.spaces[0].tabs[0].layout {
            crate::PaneLayout::Pane(pane) => pane.id,
            crate::PaneLayout::Split(_) => panic!("initial Tab must contain one Pane"),
        };
        let new_initial_pane = match &after_restart.spaces[0].tabs[0].layout {
            crate::PaneLayout::Pane(pane) => pane.id,
            crate::PaneLayout::Split(_) => panic!("fresh Tab must contain one Pane"),
        };
        assert_ne!(
            old_initial_pane, new_initial_pane,
            "a cold Core must not reuse Pane identities from its predecessor"
        );
        assert!(
            after_restart
                .spaces
                .iter()
                .all(|space| space.name != "Restored Space")
        );
        assert!(matches!(
            second.call(WorkerRequest::Stop),
            Ok(WorkerResponse::Ack)
        ));
        drop(second);
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
