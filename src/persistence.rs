use crate::core_model::{
    PaneId, PersistedCoreLayout, PersistedPane, PersistedPaneLayout, PersistedSpace,
    PersistedSplit, PersistedTab, RestoreDisposition, SpaceId, SplitAxis, SplitId, SplitRatio,
    TabId, TerminalLaunch,
};
use ring::digest::{SHA256, digest};
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const MAGIC: &[u8; 8] = b"ATSTATE1";
const SCHEMA_VERSION: u32 = 1;
const MAX_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;
const MAX_ITEMS: usize = 16_384;
const MAX_VALUE_BYTES: usize = 1024 * 1024;
const MAX_LAYOUT_DEPTH: usize = 256;
const SLOT_A: &str = "snapshot-a.bin";
const SLOT_B: &str = "snapshot-b.bin";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const QUIET_SAVE_DELAY: Duration = Duration::from_millis(500);
const MAX_SAVE_DELAY: Duration = Duration::from_secs(5);
const SAVE_RETRY_DELAY: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SnapshotRecord {
    pub generation: u64,
    pub layout: PersistedCoreLayout,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SnapshotLoad {
    Ready(SnapshotRecord),
    Absent,
    IncompatibleNewer { schema_version: u32 },
    Corrupt { reason: String },
}

pub(crate) struct SnapshotStore {
    directory: PathBuf,
}

pub(crate) struct SnapshotSaver {
    commands: flume::Sender<SaverCommand>,
    latest: Arc<Mutex<Option<SnapshotRecord>>>,
    thread: Option<thread::JoinHandle<()>>,
}

enum SaverCommand {
    Dirty,
    Flush(flume::Sender<Result<(), String>>),
    Stop(flume::Sender<Result<(), String>>),
}

struct SaveSchedule {
    durable_generation: u64,
    pending: Option<SnapshotRecord>,
    first_dirty_at: Option<Instant>,
    last_mutation_at: Option<Instant>,
    retry_not_before: Option<Instant>,
}

impl SnapshotSaver {
    pub(crate) fn start(store: SnapshotStore, durable_generation: u64) -> Result<Self, String> {
        let (commands_tx, commands_rx) = flume::bounded(1);
        let latest = Arc::new(Mutex::new(None));
        let worker_latest = Arc::clone(&latest);
        let thread = thread::Builder::new()
            .name("resident-core-snapshot".into())
            .spawn(move || run_saver(store, commands_rx, worker_latest, durable_generation))
            .map_err(|error| format!("spawn snapshot saver: {error}"))?;
        Ok(Self {
            commands: commands_tx,
            latest,
            thread: Some(thread),
        })
    }

    pub(crate) fn mark_dirty(&self, record: SnapshotRecord) -> Result<(), String> {
        {
            let mut latest = self
                .latest
                .lock()
                .expect("snapshot saver latest-record mutex poisoned");
            if latest
                .as_ref()
                .is_none_or(|pending| record.generation >= pending.generation)
            {
                *latest = Some(record);
            }
        }
        match self.commands.try_send(SaverCommand::Dirty) {
            Ok(()) | Err(flume::TrySendError::Full(_)) => Ok(()),
            Err(flume::TrySendError::Disconnected(_)) => Err("snapshot saver stopped".into()),
        }
    }

    pub(crate) fn flush(&self) -> Result<(), String> {
        let (response_tx, response_rx) = flume::bounded(1);
        self.commands
            .send(SaverCommand::Flush(response_tx))
            .map_err(|_| "snapshot saver stopped".to_string())?;
        response_rx
            .recv()
            .map_err(|_| "snapshot saver stopped before flushing".to_string())?
    }
}

impl Drop for SnapshotSaver {
    fn drop(&mut self) {
        let (response_tx, response_rx) = flume::bounded(1);
        let _ = self.commands.send(SaverCommand::Stop(response_tx));
        let _ = response_rx.recv();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_saver(
    store: SnapshotStore,
    commands: flume::Receiver<SaverCommand>,
    latest: Arc<Mutex<Option<SnapshotRecord>>>,
    durable_generation: u64,
) {
    let mut schedule = SaveSchedule {
        durable_generation,
        pending: None,
        first_dirty_at: None,
        last_mutation_at: None,
        retry_not_before: None,
    };
    loop {
        let command = match schedule.next_deadline() {
            Some(deadline) => commands.recv_deadline(deadline),
            None => commands
                .recv()
                .map_err(|_| flume::RecvTimeoutError::Disconnected),
        };
        match command {
            Ok(SaverCommand::Dirty) => drain_latest(&latest, &mut schedule),
            Ok(SaverCommand::Flush(response)) => {
                drain_latest(&latest, &mut schedule);
                let _ = response.send(schedule.save(&store));
            }
            Ok(SaverCommand::Stop(response)) => {
                drain_latest(&latest, &mut schedule);
                let _ = response.send(schedule.save(&store));
                break;
            }
            Err(flume::RecvTimeoutError::Timeout) => {
                if let Err(error) = schedule.save(&store) {
                    eprintln!("Resident Core snapshot save failed: {error}");
                }
            }
            Err(flume::RecvTimeoutError::Disconnected) => {
                drain_latest(&latest, &mut schedule);
                let _ = schedule.save(&store);
                break;
            }
        }
    }
}

fn drain_latest(latest: &Mutex<Option<SnapshotRecord>>, schedule: &mut SaveSchedule) {
    if let Some(record) = latest
        .lock()
        .expect("snapshot saver latest-record mutex poisoned")
        .take()
    {
        schedule.mark_dirty(record, Instant::now());
    }
}

impl SaveSchedule {
    fn mark_dirty(&mut self, record: SnapshotRecord, now: Instant) {
        if record.generation <= self.durable_generation {
            return;
        }
        if self.pending.is_none() {
            self.first_dirty_at = Some(now);
        }
        if self
            .pending
            .as_ref()
            .is_none_or(|pending| record.generation >= pending.generation)
        {
            self.pending = Some(record);
        }
        self.last_mutation_at = Some(now);
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.pending.as_ref()?;
        let quiet = self.last_mutation_at? + QUIET_SAVE_DELAY;
        let hard = self.first_dirty_at? + MAX_SAVE_DELAY;
        let deadline = quiet.min(hard);
        Some(
            self.retry_not_before
                .map_or(deadline, |retry| deadline.max(retry)),
        )
    }

    fn save(&mut self, store: &SnapshotStore) -> Result<(), String> {
        let Some(record) = self.pending.clone() else {
            return Ok(());
        };
        match store.publish(&record) {
            Ok(()) => {
                self.durable_generation = self.durable_generation.max(record.generation);
                if self
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.generation <= self.durable_generation)
                {
                    self.pending = None;
                    self.first_dirty_at = None;
                    self.last_mutation_at = None;
                }
                self.retry_not_before = None;
                Ok(())
            }
            Err(error) => {
                self.retry_not_before = Some(Instant::now() + SAVE_RETRY_DELAY);
                Err(error)
            }
        }
    }
}

impl SnapshotStore {
    pub(crate) fn for_profile(profile: &str) -> Result<Self, String> {
        if profile.is_empty()
            || !profile
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err("snapshot profile contains invalid path characters".into());
        }
        let root = persistent_data_root()?;
        Ok(Self {
            directory: root.join("profiles").join(profile),
        })
    }

    #[cfg(test)]
    pub(crate) fn in_directory(directory: PathBuf) -> Self {
        Self { directory }
    }

    pub(crate) fn load(&self) -> SnapshotLoad {
        let slots = self.inspect_slots();
        let mut ready = Vec::new();
        let mut newer = Vec::new();
        let mut corrupt = Vec::new();
        for (name, slot) in [(SLOT_A, &slots[0]), (SLOT_B, &slots[1])] {
            match slot {
                Slot::Ready(record) => ready.push(record.clone()),
                Slot::Absent => {}
                Slot::IncompatibleNewer { schema_version } => newer.push(*schema_version),
                Slot::Corrupt(reason) => corrupt.push(format!("{name}: {reason}")),
            }
        }
        if let Some(schema_version) = newer.into_iter().max() {
            return SnapshotLoad::IncompatibleNewer { schema_version };
        }
        if let Some(record) = ready.into_iter().max_by_key(|record| record.generation) {
            return SnapshotLoad::Ready(record);
        }
        if corrupt.is_empty() {
            SnapshotLoad::Absent
        } else {
            SnapshotLoad::Corrupt {
                reason: corrupt.join("; "),
            }
        }
    }

    pub(crate) fn publish(&self, record: &SnapshotRecord) -> Result<(), String> {
        if record.generation == 0 || record.generation == u64::MAX {
            return Err("snapshot generation is outside the supported range".into());
        }
        fs::create_dir_all(&self.directory)
            .map_err(|error| format!("create snapshot directory: {error}"))?;
        secure_directory(&self.directory)
            .map_err(|error| format!("secure snapshot directory: {error}"))?;
        let slots = self.inspect_slots();
        if let Some(schema_version) = slots.iter().find_map(|slot| match slot {
            Slot::IncompatibleNewer { schema_version } => Some(*schema_version),
            _ => None,
        }) {
            return Err(format!(
                "snapshot schema {schema_version} is newer than supported schema {SCHEMA_VERSION}"
            ));
        }
        let target = choose_target(&slots);
        let target_path = self.directory.join(target);
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp_path = self
            .directory
            .join(format!(".{target}.tmp-{}-{sequence}", std::process::id()));
        let bytes = encode_snapshot(record)?;
        let publication = publish_file(&temp_path, &target_path, &bytes);
        if let Err(error) = publication {
            let _ = fs::remove_file(&temp_path);
            return Err(format!("publish snapshot: {error}"));
        }
        sync_directory(&self.directory)
            .map_err(|error| format!("synchronize snapshot directory: {error}"))
    }

    fn inspect_slots(&self) -> [Slot; 2] {
        [
            inspect_slot(&self.directory.join(SLOT_A)),
            inspect_slot(&self.directory.join(SLOT_B)),
        ]
    }
}

#[derive(Clone)]
enum Slot {
    Ready(SnapshotRecord),
    Absent,
    IncompatibleNewer { schema_version: u32 },
    Corrupt(String),
}

fn choose_target(slots: &[Slot; 2]) -> &'static str {
    match (&slots[0], &slots[1]) {
        (Slot::Ready(first), Slot::Ready(second)) if first.generation > second.generation => SLOT_B,
        (Slot::Ready(_), Slot::Ready(_)) => SLOT_A,
        (Slot::Ready(_), _) => SLOT_B,
        (_, Slot::Ready(_)) => SLOT_A,
        _ => SLOT_A,
    }
}

fn inspect_slot(path: &Path) -> Slot {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Slot::Absent,
        Err(error) => return Slot::Corrupt(format!("open failed: {error}")),
    };
    let length = match file.metadata() {
        Ok(metadata) => metadata.len(),
        Err(error) => return Slot::Corrupt(format!("metadata failed: {error}")),
    };
    if length > MAX_SNAPSHOT_BYTES as u64 {
        return Slot::Corrupt(format!("file exceeds {MAX_SNAPSHOT_BYTES} bytes"));
    }
    let mut bytes = Vec::with_capacity(length as usize);
    if let Err(error) = file.read_to_end(&mut bytes) {
        return Slot::Corrupt(format!("read failed: {error}"));
    }
    match decode_snapshot(&bytes) {
        Ok(record) => Slot::Ready(record),
        Err(DecodeError::IncompatibleNewer { schema_version }) => {
            Slot::IncompatibleNewer { schema_version }
        }
        Err(DecodeError::Corrupt(reason)) => Slot::Corrupt(reason),
    }
}

fn publish_file(temp_path: &Path, target_path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(temp_path)?;
    file.write_all(bytes)?;
    file.flush()?;
    sync_snapshot_file(&file)?;
    drop(file);

    if target_path.exists() {
        fs::remove_file(target_path)?;
    }
    fs::rename(temp_path, target_path)
}

#[cfg(target_os = "macos")]
fn sync_snapshot_file(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    file.sync_all()?;
    let result = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_FULLFSYNC) };
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
fn sync_snapshot_file(file: &File) -> io::Result<()> {
    file.sync_all()
}

#[cfg(unix)]
fn secure_directory(directory: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn secure_directory(_directory: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> io::Result<()> {
    File::open(directory)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) -> io::Result<()> {
    Ok(())
}

fn persistent_data_root() -> Result<PathBuf, String> {
    if let Some(override_root) = std::env::var_os("AGENT_TERMINAL_STATE_DIR")
        && !override_root.is_empty()
    {
        return Ok(PathBuf::from(override_root));
    }

    let base = directories::BaseDirs::new()
        .ok_or_else(|| "resolve the per-user snapshot data directory".to_string())?;

    #[cfg(target_os = "linux")]
    return Ok(base.data_dir().join("agent-terminal"));

    #[cfg(target_os = "macos")]
    return Ok(base.data_dir().join("com.skulldogged.agent-terminal"));

    #[cfg(windows)]
    return Ok(base.data_local_dir().join("Skulldogged/AgentTerminal"));

    #[allow(unreachable_code)]
    Err("snapshot persistence is unsupported on this platform".into())
}

fn encode_snapshot(record: &SnapshotRecord) -> Result<Vec<u8>, String> {
    let mut payload = Writer::default();
    encode_layout(&mut payload, &record.layout)?;
    let payload = payload.finish();
    let payload_length = u32::try_from(payload.len())
        .map_err(|_| "snapshot payload exceeds the format limit".to_string())?;
    let checksum = digest(&SHA256, &payload);
    let mut output = Vec::with_capacity(56 + payload.len());
    output.extend_from_slice(MAGIC);
    output.extend_from_slice(&SCHEMA_VERSION.to_le_bytes());
    output.extend_from_slice(&record.generation.to_le_bytes());
    output.extend_from_slice(&payload_length.to_le_bytes());
    output.extend_from_slice(checksum.as_ref());
    output.extend_from_slice(&payload);
    if output.len() > MAX_SNAPSHOT_BYTES {
        return Err(format!("snapshot exceeds {MAX_SNAPSHOT_BYTES} bytes"));
    }
    Ok(output)
}

enum DecodeError {
    IncompatibleNewer { schema_version: u32 },
    Corrupt(String),
}

fn decode_snapshot(bytes: &[u8]) -> Result<SnapshotRecord, DecodeError> {
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return Err(DecodeError::Corrupt("snapshot is oversized".into()));
    }
    let mut reader = Reader::new(bytes);
    if reader.take(MAGIC.len())? != MAGIC {
        return Err(DecodeError::Corrupt("invalid snapshot magic".into()));
    }
    let schema_version = reader.u32()?;
    if schema_version > SCHEMA_VERSION {
        return Err(DecodeError::IncompatibleNewer { schema_version });
    }
    if schema_version != SCHEMA_VERSION {
        return Err(DecodeError::Corrupt(format!(
            "unsupported older snapshot schema {schema_version}"
        )));
    }
    let generation = reader.u64()?;
    if generation == 0 || generation == u64::MAX {
        return Err(DecodeError::Corrupt(
            "snapshot generation is outside the supported range".into(),
        ));
    }
    let payload_length = reader.u32()? as usize;
    let checksum = reader.take(32)?;
    let payload = reader.take(payload_length)?;
    reader.finish()?;
    if digest(&SHA256, payload).as_ref() != checksum {
        return Err(DecodeError::Corrupt("snapshot checksum mismatch".into()));
    }
    let mut payload_reader = Reader::new(payload);
    let layout = decode_layout(&mut payload_reader)?;
    payload_reader.finish()?;
    Ok(SnapshotRecord { generation, layout })
}

fn encode_layout(writer: &mut Writer, layout: &PersistedCoreLayout) -> Result<(), String> {
    writer.u64(layout.next_id);
    writer.count(layout.spaces.len())?;
    for space in &layout.spaces {
        writer.u64(space.id.as_u64());
        writer.string(&space.name)?;
        writer.path(&space.directory)?;
        writer.count(space.tabs.len())?;
        for tab in &space.tabs {
            writer.u64(tab.id.as_u64());
            writer.string(&tab.name)?;
            encode_pane_layout(writer, &tab.layout, 0)?;
        }
    }
    Ok(())
}

fn encode_pane_layout(
    writer: &mut Writer,
    layout: &PersistedPaneLayout,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_LAYOUT_DEPTH {
        return Err("snapshot Pane layout is too deeply nested".into());
    }
    match layout {
        PersistedPaneLayout::Pane(pane) => {
            writer.u8(0);
            writer.u64(pane.id.as_u64());
            writer.path(&pane.launch.working_directory)?;
            writer.u8(match pane.launch.restore_disposition {
                RestoreDisposition::Relaunch => 0,
                RestoreDisposition::RemainEnded => 1,
            });
        }
        PersistedPaneLayout::Split(split) => {
            writer.u8(1);
            writer.u64(split.id.as_u64());
            writer.u8(match split.axis {
                SplitAxis::Horizontal => 0,
                SplitAxis::Vertical => 1,
            });
            writer.u16(split.ratio.parts_per_thousand());
            encode_pane_layout(writer, &split.first, depth + 1)?;
            encode_pane_layout(writer, &split.second, depth + 1)?;
        }
    }
    Ok(())
}

fn decode_layout(reader: &mut Reader<'_>) -> Result<PersistedCoreLayout, DecodeError> {
    let next_id = reader.u64()?;
    let space_count = reader.count()?;
    let mut spaces = Vec::with_capacity(space_count);
    for _ in 0..space_count {
        let id = SpaceId::from_u64(reader.u64()?);
        let name = reader.string()?;
        let directory = reader.path()?;
        let tab_count = reader.count()?;
        let mut tabs = Vec::with_capacity(tab_count);
        for _ in 0..tab_count {
            tabs.push(PersistedTab {
                id: TabId::from_u64(reader.u64()?),
                name: reader.string()?,
                layout: decode_pane_layout(reader, 0)?,
            });
        }
        spaces.push(PersistedSpace {
            id,
            name,
            directory,
            tabs,
        });
    }
    Ok(PersistedCoreLayout { next_id, spaces })
}

fn decode_pane_layout(
    reader: &mut Reader<'_>,
    depth: usize,
) -> Result<PersistedPaneLayout, DecodeError> {
    if depth > MAX_LAYOUT_DEPTH {
        return Err(DecodeError::Corrupt(
            "snapshot Pane layout is too deeply nested".into(),
        ));
    }
    match reader.u8()? {
        0 => {
            let id = PaneId::from_u64(reader.u64()?);
            let working_directory = reader.path()?;
            let restore_disposition = match reader.u8()? {
                0 => RestoreDisposition::Relaunch,
                1 => RestoreDisposition::RemainEnded,
                value => {
                    return Err(DecodeError::Corrupt(format!(
                        "invalid Restore Disposition tag {value}"
                    )));
                }
            };
            Ok(PersistedPaneLayout::Pane(PersistedPane {
                id,
                launch: TerminalLaunch {
                    working_directory,
                    restore_disposition,
                },
            }))
        }
        1 => {
            let id = SplitId::from_u64(reader.u64()?);
            let axis = match reader.u8()? {
                0 => SplitAxis::Horizontal,
                1 => SplitAxis::Vertical,
                value => {
                    return Err(DecodeError::Corrupt(format!(
                        "invalid split axis tag {value}"
                    )));
                }
            };
            let ratio = SplitRatio::new(reader.u16()?)
                .map_err(|error| DecodeError::Corrupt(error.to_string()))?;
            let first = Box::new(decode_pane_layout(reader, depth + 1)?);
            let second = Box::new(decode_pane_layout(reader, depth + 1)?);
            Ok(PersistedPaneLayout::Split(PersistedSplit {
                id,
                axis,
                ratio,
                first,
                second,
            }))
        }
        value => Err(DecodeError::Corrupt(format!(
            "invalid Pane layout tag {value}"
        ))),
    }
}

#[derive(Default)]
struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    fn finish(self) -> Vec<u8> {
        self.bytes
    }

    fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    fn u16(&mut self, value: u16) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn count(&mut self, count: usize) -> Result<(), String> {
        if count > MAX_ITEMS {
            return Err(format!("snapshot collection exceeds {MAX_ITEMS} items"));
        }
        let count = u32::try_from(count).map_err(|_| "snapshot collection is too large")?;
        self.u32(count);
        Ok(())
    }

    fn string(&mut self, value: &str) -> Result<(), String> {
        self.bytes(value.as_bytes())
    }

    fn path(&mut self, value: &Path) -> Result<(), String> {
        self.bytes(&encode_os_string(value.as_os_str()))
    }

    fn bytes(&mut self, value: &[u8]) -> Result<(), String> {
        if value.len() > MAX_VALUE_BYTES {
            return Err(format!("snapshot value exceeds {MAX_VALUE_BYTES} bytes"));
        }
        let length = u32::try_from(value.len()).map_err(|_| "snapshot value is too large")?;
        self.u32(length);
        self.bytes.extend_from_slice(value);
        Ok(())
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn finish(&self) -> Result<(), DecodeError> {
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err(DecodeError::Corrupt("snapshot has trailing bytes".into()))
        }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], DecodeError> {
        let end = self
            .position
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| DecodeError::Corrupt("snapshot is truncated".into()))?;
        let value = &self.bytes[self.position..end];
        self.position = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().expect("two-byte slice"),
        ))
    }

    fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("four-byte slice"),
        ))
    }

    fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("eight-byte slice"),
        ))
    }

    fn count(&mut self) -> Result<usize, DecodeError> {
        let count = self.u32()? as usize;
        if count > MAX_ITEMS {
            return Err(DecodeError::Corrupt(format!(
                "snapshot collection exceeds {MAX_ITEMS} items"
            )));
        }
        Ok(count)
    }

    fn string(&mut self) -> Result<String, DecodeError> {
        String::from_utf8(self.bytes()?.to_vec())
            .map_err(|_| DecodeError::Corrupt("snapshot string is not UTF-8".into()))
    }

    fn path(&mut self) -> Result<PathBuf, DecodeError> {
        decode_os_string(self.bytes()?).map(PathBuf::from)
    }

    fn bytes(&mut self) -> Result<&'a [u8], DecodeError> {
        let length = self.u32()? as usize;
        if length > MAX_VALUE_BYTES {
            return Err(DecodeError::Corrupt(format!(
                "snapshot value exceeds {MAX_VALUE_BYTES} bytes"
            )));
        }
        self.take(length)
    }
}

#[cfg(unix)]
fn encode_os_string(value: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    value.as_bytes().to_vec()
}

#[cfg(unix)]
fn decode_os_string(bytes: &[u8]) -> Result<OsString, DecodeError> {
    use std::os::unix::ffi::OsStringExt;
    Ok(OsString::from_vec(bytes.to_vec()))
}

#[cfg(windows)]
fn encode_os_string(value: &OsStr) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    value
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>()
}

#[cfg(windows)]
fn decode_os_string(bytes: &[u8]) -> Result<OsString, DecodeError> {
    use std::os::windows::ffi::OsStringExt;
    if !bytes.len().is_multiple_of(2) {
        return Err(DecodeError::Corrupt(
            "snapshot Windows path has an odd byte length".into(),
        ));
    }
    let (pairs, remainder) = bytes.as_chunks::<2>();
    debug_assert!(remainder.is_empty());
    let wide = pairs
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect::<Vec<_>>();
    Ok(OsString::from_wide(&wide))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CoreCommand, CoreModel, CreatedResource, SplitPlacement};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_directory(scenario: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "agent-terminal-persistence-{scenario}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn populated_layout() -> PersistedCoreLayout {
        let directory = std::env::current_dir().expect("current directory");
        let mut model = CoreModel::new();
        let created = model
            .apply(
                0,
                CoreCommand::CreateSpace {
                    name: "Persistent Space".into(),
                    directory,
                },
            )
            .expect("create Space");
        let CreatedResource::Space { pane_id, .. } = created.created else {
            panic!("Space creation must identify its Pane");
        };
        model
            .apply(
                created.revision,
                CoreCommand::SplitPane {
                    pane_id,
                    axis: SplitAxis::Vertical,
                    placement: SplitPlacement::After,
                    ratio: SplitRatio::new(650).expect("valid ratio"),
                },
            )
            .expect("split Pane");
        model.persisted_layout().expect("capture persisted layout")
    }

    #[test]
    fn binary_snapshot_round_trips_the_complete_layout() {
        let record = SnapshotRecord {
            generation: 7,
            layout: populated_layout(),
        };
        let bytes = encode_snapshot(&record).expect("encode snapshot");

        assert_eq!(decode_snapshot(&bytes).ok(), Some(record));
    }

    #[test]
    fn checksum_corruption_is_rejected() {
        let record = SnapshotRecord {
            generation: 1,
            layout: populated_layout(),
        };
        let mut bytes = encode_snapshot(&record).expect("encode snapshot");
        let last = bytes.last_mut().expect("non-empty snapshot");
        *last ^= 0xff;

        assert!(matches!(
            decode_snapshot(&bytes),
            Err(DecodeError::Corrupt(reason)) if reason.contains("checksum")
        ));
    }

    #[test]
    fn ab_store_retains_the_previous_valid_generation() {
        let directory = temporary_directory("ab");
        let store = SnapshotStore::in_directory(directory.clone());
        let first = SnapshotRecord {
            generation: 1,
            layout: populated_layout(),
        };
        let second = SnapshotRecord {
            generation: 2,
            layout: first.layout.clone(),
        };
        store.publish(&first).expect("publish first generation");
        store.publish(&second).expect("publish second generation");
        fs::write(directory.join(SLOT_B), b"broken").expect("corrupt newest slot");

        assert_eq!(store.load(), SnapshotLoad::Ready(first));
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn newer_schema_blocks_loading_and_publication() {
        let directory = temporary_directory("newer");
        fs::create_dir_all(&directory).expect("create test directory");
        let record = SnapshotRecord {
            generation: 1,
            layout: populated_layout(),
        };
        let mut bytes = encode_snapshot(&record).expect("encode snapshot");
        bytes[8..12].copy_from_slice(&(SCHEMA_VERSION + 1).to_le_bytes());
        fs::write(directory.join(SLOT_A), bytes).expect("write newer snapshot");
        let store = SnapshotStore::in_directory(directory.clone());

        assert_eq!(
            store.load(),
            SnapshotLoad::IncompatibleNewer {
                schema_version: SCHEMA_VERSION + 1
            }
        );
        assert!(store.publish(&record).is_err());
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn continuous_mutations_do_not_move_the_hard_save_deadline() {
        let start = Instant::now();
        let mut schedule = SaveSchedule {
            durable_generation: 0,
            pending: None,
            first_dirty_at: None,
            last_mutation_at: None,
            retry_not_before: None,
        };
        schedule.mark_dirty(
            SnapshotRecord {
                generation: 1,
                layout: populated_layout(),
            },
            start,
        );
        schedule.mark_dirty(
            SnapshotRecord {
                generation: 2,
                layout: populated_layout(),
            },
            start + MAX_SAVE_DELAY - Duration::from_millis(10),
        );

        assert_eq!(schedule.next_deadline(), Some(start + MAX_SAVE_DELAY));
    }

    #[test]
    fn clean_flush_publishes_the_latest_queued_generation() {
        let directory = temporary_directory("flush");
        let saver = SnapshotSaver::start(SnapshotStore::in_directory(directory.clone()), 0)
            .expect("start saver");
        let layout = populated_layout();
        saver
            .mark_dirty(SnapshotRecord {
                generation: 1,
                layout: layout.clone(),
            })
            .expect("queue first generation");
        saver
            .mark_dirty(SnapshotRecord {
                generation: 2,
                layout: layout.clone(),
            })
            .expect("queue second generation");
        saver.flush().expect("flush latest generation");
        drop(saver);

        assert_eq!(
            SnapshotStore::in_directory(directory.clone()).load(),
            SnapshotLoad::Ready(SnapshotRecord {
                generation: 2,
                layout,
            })
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn dirty_notifications_are_bounded_before_the_saver_can_run() {
        let (commands, queued) = flume::bounded(1);
        let latest = Arc::new(Mutex::new(None));
        let saver = std::mem::ManuallyDrop::new(SnapshotSaver {
            commands,
            latest: Arc::clone(&latest),
            thread: None,
        });
        let layout = populated_layout();

        for generation in 1..=8 {
            saver
                .mark_dirty(SnapshotRecord {
                    generation,
                    layout: layout.clone(),
                })
                .expect("queue dirty generation");
        }

        assert_eq!(queued.len(), 1, "only one saver wake may remain queued");
        assert_eq!(
            latest
                .lock()
                .expect("latest snapshot mutex")
                .as_ref()
                .map(|record| record.generation),
            Some(8)
        );
    }
}
