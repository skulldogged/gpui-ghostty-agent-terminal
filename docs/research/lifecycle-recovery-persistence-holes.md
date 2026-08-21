# Lifecycle recovery and persistence holes

This report is intentionally divided into primary-source facts and project
recommendations. It covers recovery and termination when a Resident Core endpoint
accepts a connection but the normal authentication or versioned protocol handshake
cannot complete, cross-platform snapshot publication and durability, profile data
directories, and bounded save scheduling. Restore disposition is covered by the
separate lifecycle/domain-model decision.

## Recovery and termination

### Repository facts

- `CoreClient::connect` sends the versioned `Hello` request, requires a matching `Ready`, and only then returns a client ([source](../../src/resident_core.rs#L402-L490)). A protocol mismatch therefore prevents construction of the `CoreClient` needed for all later commands.
- `stop_resident_core` is a normal post-handshake request ([source](../../src/resident_core.rs#L663-L669)). The Desktop Shell's stop helper first calls `CoreClient::connect`, then sends that request ([source](../../src/resident_core.rs#L1840-L1854)). It consequently cannot recover from the handshake failure that caused the UI to offer recovery.
- The server rejects a mismatched version before it registers the client or enters the command loop ([source](../../src/resident_core.rs#L1973-L2010)). `StopResidentCore` is handled only inside that later loop ([source](../../src/resident_core.rs#L2080-L2092)).
- The spawned Core is detached on both Unix and Windows, while the spawning UI retains only a `Child` used by an in-process reaper thread ([source](../../src/resident_core.rs#L2140-L2183)). A later UI process therefore does not inherit a durable OS process handle from the original spawn.

These facts make the hole concrete: retrying the existing acknowledged stop command is not an independent recovery path.

### Primary-source platform facts

#### Common authentication primitive

HMAC is a message-authentication mechanism based on a shared secret; its purpose is to let parties sharing that key validate transmitted information. RFC 2104 also requires randomly chosen keys and protection of those keys ([RFC 2104, sections 1 and 3](https://www.rfc-editor.org/rfc/rfc2104)). This supports reuse of the existing protected per-user secret for a narrowly domain-separated recovery exchange, but the RFC does not itself define the exchange, freshness rules, or authorization policy.

#### Linux

- A numeric PID is not a stable process reference. The Linux `pidfd_send_signal(2)` documentation explicitly identifies the race in PID-based signaling: the intended process can exit, its PID can be recycled, and a traditional `kill(2)` can signal the replacement. A pidfd is instead a stable reference, and signaling it fails with `ESRCH` after the referenced process is gone ([Linux `pidfd_send_signal(2)`](https://man7.org/linux/man-pages/man2/pidfd_send_signal.2.html)).
- `pidfd_open(2)` is the preferred way to obtain a pidfd for an existing process; a pidfd can be polled for exit and passed to `pidfd_send_signal` ([Linux `pidfd_open(2)`](https://man7.org/linux/man-pages/man2/pidfd_open.2.html)). This stabilizes the process selected by the lookup, but it does not prove that a stale recorded PID had not already been recycled *before* `pidfd_open` ran.
- `/proc/<pid>/stat` field 22 is the process start time measured from system boot, and `/proc/<pid>/exe` refers to the executed image ([Linux `proc_pid_stat(5)`](https://man7.org/linux/man-pages/man5/proc_pid_stat.5.html), [Linux `proc_pid_exe(5)`](https://man7.org/linux/man-pages/man5/proc_pid_exe.5.html)). They are useful identity evidence to compare with a record made by the Core, subject to `/proc` availability and access checks.
- `_exit` terminates all threads through the C-library wrapper on modern Linux, does not run `atexit` handlers, and does not flush stdio, although closing descriptors can still delay on pending I/O ([Linux `_exit(2)`](https://man7.org/linux/man-pages/man2/_exit.2.html)).

#### macOS

- Apple's documented `kill(2)` accepts a PID, and `kill(pid, 0)` performs only validity and permission checks ([Apple `kill(2)`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/kill.2.html)). It provides no stable process handle in the call.
- `EVFILT_PROC` can attach a kqueue event to a PID and report `NOTE_EXIT`, `NOTE_EXEC`, and related events, but it is a monitoring interface; it does not provide a handle-based signaling operation ([Apple `kqueue(2)`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/kqueue.2.html)).
- Apple's public `_exit(2)` terminates the process, closes its descriptors, and never returns ([Apple `_exit(2)`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/_exit.2.html)).

The reviewed public macOS interfaces therefore do not supply an equivalent to Linux's `pidfd_send_signal` or Windows' handle-targeted `TerminateProcess`. Checking a PID's attributes and later calling `kill(pid, ...)` still leaves a check-to-signal PID-reuse window. This is an inference from the documented interfaces, not an Apple guarantee that no other suitable API exists.

#### Windows

- A process ID is valid only from process creation through termination. By contrast, a process handle remains valid until it is closed, even after the represented process terminates ([Microsoft, Process Handles and Identifiers](https://learn.microsoft.com/en-us/windows/win32/procthread/process-handles-and-identifiers)).
- `OpenProcess` resolves a PID to a handle and checks requested rights against the process security descriptor. The returned handle can be used by process APIs until closed ([Microsoft, `OpenProcess`](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-openprocess)). As with `pidfd_open`, this does not prove that a stale PID had not already been reused before the call.
- `GetProcessTimes` returns the creation time for the process represented by a handle, while `QueryFullProcessImageNameW` returns that handle's executable image path. Both accept `PROCESS_QUERY_LIMITED_INFORMATION` ([Microsoft, `GetProcessTimes`](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getprocesstimes), [Microsoft, `QueryFullProcessImageNameW`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-queryfullprocessimagenamew)).
- `TerminateProcess` targets a process handle and requires `PROCESS_TERMINATE`. It is unconditional and asynchronous for an external caller; Microsoft directs callers that require completion to wait on the process handle. The kernel process object remains until all open handles are released ([Microsoft, `TerminateProcess`](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-terminateprocess)). `SYNCHRONIZE` is the right required to wait for process termination ([Microsoft, Process Security and Access Rights](https://learn.microsoft.com/en-us/windows/win32/procthread/process-security-and-access-rights)).

### Recommendation: a stable authenticated recovery control plane

Add a deliberately small recovery protocol that does **not** depend on `PROTOCOL_VERSION` or construct a `CoreClient`.

1. Bind a separate recovery endpoint per profile alongside the normal endpoint. Service it on a dedicated thread whose request path does not call the terminal worker merely to authenticate, report identity, or arm termination. Keeping it separate prevents a decoder or handshake incompatibility on the normal endpoint from consuming the recovery request.
2. Freeze a `recovery-v1` binary envelope with fixed magic, bounded frame length, fixed-width fields, and only two operations: `Probe` and `Stop`. Future application protocols must leave this v1 endpoint and meaning intact for the supported upgrade window; an incompatible recovery design gets a new endpoint rather than changing v1 in place.
3. Authenticate both directions with the existing protected secret, but use recovery-specific domain separators. A suitable exchange is:

   - client sends `Probe(client_nonce)`;
   - Core returns `instance_id`, PID, `server_nonce`, and `HMAC(secret, "recovery-v1/server" || profile || instance_id || PID || client_nonce || server_nonce)`;
   - client verifies the Core proof, then sends `Stop` with `HMAC(secret, "recovery-v1/client-stop" || the same bound fields)`;
   - Core verifies the proof in constant time and consumes that nonce pair once.

   Binding the profile, random per-Core `instance_id`, both fresh nonces, PID, and operation prevents a captured proof from authorizing a different profile, Core instance, or command. These field choices and replay rules are project recommendations built on HMAC, not requirements stated by RFC 2104.
4. Make the destructive UI name unambiguous: this path implements **Stop Sessions and Quit**, never **Quit Desktop Shell (Keep Sessions)**. Before sending `Stop`, show that live sessions will end and cold restoration cannot preserve their processes.
5. Give the recovery listener two termination phases:

   - request the normal worker's graceful stop and allow a short, bounded period for persistence and transport cleanup;
   - if that deadline expires, perform emergency *self-termination* from the already authenticated Core process (`_exit` on Unix/macOS; `TerminateProcess(GetCurrentProcess(), recovery_exit_code)` on Windows).

   Self-termination avoids choosing a target from a possibly stale external PID. It is intentionally destructive: no Rust destructors, terminal cleanup, or final persistence may be assumed in the emergency phase. `std::process::exit` is a poor hard-stop primitive because Rust documents that it runs `atexit` and platform exit handlers, which may themselves depend on wedged locks ([Rust `std::process::exit`](https://doc.rust-lang.org/std/process/fn.exit.html)).
6. Treat the response as **accepted**, not **completed**. After receiving it, the recovery client must wait for recovery-endpoint closure and then confirm that reconnect/spawn succeeds. If the endpoint remains live past a deadline, report that automatic recovery failed rather than silently spawning a second Core.

This control plane resolves the stated protocol/authentication mismatch on all three target operating systems without using an unsafe PID kill. It also handles many worker deadlocks because the dedicated listener can escalate to self-termination. It cannot recover from whole-process starvation, a kernel wait that prevents the listener from running, or failure before the recovery endpoint binds.

### Process-tree containment is a prerequisite

Emergency self-termination proves that the old **Core** exits; it does not by itself
prove the issue record's stronger claim that every hosted process is dead. The current
transport cleanup lives in Rust `Drop` implementations, which `_exit` and
`TerminateProcess(GetCurrentProcess(), ...)` deliberately bypass
([Unix PTY drop](../../src/pty.rs#L217-L224),
[Windows ConPTY drop](../../src/windows_pty.rs#L274-L285)).

The platform contracts are weaker than process-tree termination:

- POSIX says that the last close of a pseudoterminal master sends `SIGHUP` to the
  controlling process, if any; a process may catch or ignore that signal
  ([POSIX `close`](https://pubs.opengroup.org/onlinepubs/009604499/functions/close.html)).
- Windows says that `ClosePseudoConsole` sends `CTRL_CLOSE_EVENT` to connected client
  applications and that they may continue writing while disconnecting; it does not
  promise an unconditional tree kill
  ([Microsoft `ClosePseudoConsole`](https://learn.microsoft.com/en-us/windows/console/closepseudoconsole)).
- A Windows Job Object configured with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` does
  terminate all associated processes when its last handle closes
  ([Microsoft, Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)).

Before the product promises that Core crash or emergency recovery kills every hosted
process, the Terminal Session transport must add crash-surviving containment:

- Windows should assign each hosted process tree to an owned Job Object with
  kill-on-close, accounting for existing/nested jobs and refusing an uncontained
  launch when the invariant cannot be established.
- Unix needs an independently surviving guardian/lifeline design that owns the
  Terminal Session process group and kills it when the Core's private lifeline reaches
  EOF. Ordinary Core-side `Drop`, a PID file, or PTY close alone is insufficient for
  arbitrary programs that daemonize or ignore `SIGHUP`.

If that containment is deferred, correct the event matrix to “live continuity is
lost; surviving processes are unmanaged” instead of “processes are dead.” Acceptance
coverage should crash the Core around a shell with a descendant that ignores terminal
hangup/close and verify either containment-driven termination or the weaker documented
behavior.

#### Concise Rust shape

Keep the seam independent of `CoreClient`, `Request`, `Response`, and `PROTOCOL_VERSION`:

```rust
// resident_core/recovery.rs
struct RecoveryServer {
    endpoint: RecoveryEndpoint,
    secret: RecoverySecret,
    instance: CoreInstanceIdentity,
    graceful_stop: flume::Sender<GracefulStop>,
    stopped: flume::Receiver<()>,
}

enum RecoveryRequest {
    Probe { client_nonce: [u8; 32] },
    Stop { transcript: StopTranscript, proof: [u8; 32] },
}

enum RecoveryResponse {
    Ready { transcript: ProbeTranscript, proof: [u8; 32] },
    Accepted,
    Denied,
}
```

`RecoveryServer::run` owns its listener thread and a small `recovery_v1` codec with a hard frame cap. Startup is considered failed unless both normal and recovery endpoints bind; the server starts before terminal workers accept commands. After a verified `Stop`, it writes and flushes `Accepted`, sends `GracefulStop`, and waits on `stopped` for the fixed grace period. Timeout calls a private `emergency_exit(code) -> !` implemented with `libc::_exit` under `cfg(unix)` and `TerminateProcess(GetCurrentProcess(), code)` under `cfg(windows)`. Tests substitute an `EmergencyExit` function pointer or trait so escalation can be asserted without terminating the test process. No emergency path takes a Resident Core mutex or waits for a destructor.

### Optional last-resort external termination

If the product later requires automatic recovery even when the dedicated recovery listener cannot run, keep that code behind an additional explicit confirmation and fail closed on any identity uncertainty.

#### Linux fallback

Have each Core atomically publish an authenticated runtime record containing at least profile, PID, random `instance_id`, `/proc/<pid>/stat` start time, and executable identity. Recovery should:

1. verify the record and its profile using the protected secret;
2. call `pidfd_open(record.pid, 0)`;
3. while holding that pidfd, compare the live start time and executable identity with the record;
4. refuse on any missing field or mismatch;
5. send `SIGTERM` through `pidfd_send_signal`, poll the same pidfd, then send `SIGKILL` through that same pidfd only after a deadline.

The identity comparison detects a PID that was already recycled before `pidfd_open`; the pidfd prevents the later check-to-signal race. Never fall back from a pidfd error to `kill(record.pid, ...)`.

#### Windows fallback

Publish the equivalent authenticated record with PID, random `instance_id`, process creation `FILETIME`, and executable identity. Recovery should make one `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | SYNCHRONIZE, FALSE, pid)` call, keep that handle, compare `GetProcessTimes` creation time and `QueryFullProcessImageNameW` output with the record, refuse on uncertainty, call `TerminateProcess` on that same handle, wait on it, and close it. Do not verify through one handle and reopen by PID for termination.

#### macOS fallback

Do not automate external `kill(pid, ...)` based only on a PID file or a pre-kill attribute check. The reviewed public APIs do not close the PID-reuse race between checking identity and signaling. If the authenticated recovery endpoint cannot self-terminate, report the exact Core PID and executable path as diagnostic evidence and require the human to terminate it through an OS process-management UI or an explicitly entered command. A future automatic macOS fallback should be added only if it can retain or acquire a documented stable, handle-like reference that is also the target of termination.

### Implementation acceptance tests

- A current UI can `Probe` and stop an older Core whose normal protocol version differs; an older recovery-v1 fixture can do the reverse.
- Invalid magic, oversized frames, invalid HMACs, wrong profile/instance bindings, replayed nonce pairs, and unauthenticated stop requests fail without changing Core state.
- Stalling the normal handshake handler and separately deadlocking the terminal worker do not prevent the recovery listener from accepting a stop and reaching its emergency self-termination deadline.
- **Quit Desktop Shell (Keep Sessions)** never touches the recovery endpoint; only a separately confirmed **Stop Sessions and Quit** does.
- The recovery client never spawns a replacement until the old endpoint is gone, and it reports a bounded failure if the Core does not exit.
- Linux fallback tests substitute a live unrelated process at the recorded PID/start-time boundary and verify refusal; all signals use one validated pidfd.
- Windows fallback tests substitute a creation-time or image mismatch and verify refusal; query, terminate, and wait all use one handle.
- macOS exposes no automatic external PID-kill fallback.

## Snapshot publication and crash durability

### Primary-source facts

Atomic visibility and durable storage are different guarantees:

- On POSIX systems, replacing an existing non-directory pathname with `rename` is
  atomic: another process does not observe a moment when the destination name is
  absent ([POSIX `rename`](https://pubs.opengroup.org/onlinepubs/9799919799/functions/rename.html),
  [Linux `rename(2)`](https://man7.org/linux/man-pages/man2/rename.2.html)). That says
  nothing by itself about which directory entry survives a crash.
- POSIX's own rationale describes the durable update sequence as synchronizing the
  new file, renaming it over the old file, and synchronizing the containing directory
  when the application must ensure that the new name is durable. A rename touching
  two directories can require synchronizing both
  ([POSIX.1-2024 rationale, directory operations](https://pubs.opengroup.org/onlinepubs/9799919799/xrat/V4_xbd_chap01.html)).
- Rust's `File::sync_all` attempts to synchronize both file content and metadata and
  can surface errors that dropping a `File` would discard
  ([Rust `File::sync_all`](https://doc.rust-lang.org/std/fs/struct.File.html#method.sync_all)).
  Rust's `fs::rename` currently maps to POSIX `rename` on Unix and to `MoveFileExW`
  with a `SetFileInformationByHandle` fallback on Windows; the API documents
  platform differences but makes no crash-durability promise
  ([Rust `fs::rename`](https://doc.rust-lang.org/std/fs/fn.rename.html)).
- Apple documents that ordinary `fsync` can leave data in a drive's volatile cache;
  `F_FULLFSYNC` additionally requests that the drive flush its buffered data to
  permanent storage. Even that is a best-effort storage guarantee, not immunity from
  every sudden-power-loss failure
  ([Apple `fsync(2)`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fsync.2.html),
  [Apple `fcntl(2)`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fcntl.2.html)).
- On Windows, `FlushFileBuffers` sends the buffered data for an open writable file to
  the device ([Microsoft `FlushFileBuffers`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers)).
  `ReplaceFileW` replaces one file with another, requires all participants to be on
  the same volume, and documents several partial failure arrangements that callers
  must handle. Its `REPLACEFILE_WRITE_THROUGH` flag is explicitly unsupported
  ([Microsoft `ReplaceFileW`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew)).
  `MoveFileExW` supports replacement; its `MOVEFILE_WRITE_THROUGH` guarantee is
  specifically described for a move performed as copy-and-delete
  ([Microsoft `MoveFileExW`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw)).

The practical consequence is that “pretty JSON plus temp file plus rename” is a
useful publication pattern, but it does not alone prove the issue comment's stronger
claim that the disk always contains a valid previous snapshot after a process,
kernel, or power failure.

### What Herdr actually establishes

At the pinned research revision, Herdr does use pretty JSON, a sibling `.tmp` path,
and `std::fs::rename` over the target
([Herdr `persist/io.rs`, lines 41-56](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/src/persist/io.rs#L41-L56)).
It does **not** call `sync_all`, `fsync`, `F_FULLFSYNC`, or `FlushFileBuffers` for
that update. Herdr therefore demonstrates a simple, readable implementation and
atomic pathname replacement where the underlying rename supplies it; it is not
primary evidence for crash-durable publication.

Herdr also uses one fixed temporary name. That is safe under its single-writer
assumption, but it is not a general multi-writer protocol. The Resident Core should
make the single-writer invariant explicit rather than inheriting it accidentally.

### Recommendation: define and implement two durability tiers

Name the guarantees precisely:

1. **Required application-crash guarantee:** after a Core process crash, startup
   reads either the last successfully published snapshot or the newly published
   snapshot, never a partially serialized target.
2. **Best-effort machine-crash guarantee:** after kernel failure or sudden power
   loss, use the strongest documented user-space flush sequence available without
   claiming that commodity hardware makes loss impossible.

Implement one `SnapshotStore` owned exclusively by the Resident Core. A save should:

1. capture immutable state at model generation `G`;
2. serialize and validate the complete versioned envelope in memory;
3. create a uniquely named temporary file in the destination directory with
   create-new semantics and user-only permissions;
4. write all bytes, flush language-level buffers, then call `File::sync_all`;
5. publish within the same directory/volume;
6. on Unix, synchronize the containing directory after `rename` (and after deleting
   the empty-state snapshot); on macOS, put any stronger `F_FULLFSYNC` policy behind
   a small platform adapter;
7. on Windows, use an explicit adapter rather than assuming `std::fs::rename` has
   Unix guarantees: use `ReplaceFileW` with a sibling backup when a target exists
   and an appropriate same-volume rename for first publication, inspect its
   documented partial-failure states, retain whichever target or backup still
   validates, and retry/report sharing violations without deleting the known-good
   copy; and
8. delete an abandoned temp file only after recording the publication failure; a
   cleanup failure must not mask the original error.

The Windows API documentation does not establish the same directory-flush contract
as POSIX. If “one valid snapshot survives sudden power loss” is a hard product
requirement rather than a best effort, prefer an A/B store over a single replaced
file: alternate between `snapshot-a.json` and `snapshot-b.json`, include a monotonic
generation plus a checksum over the payload, fully flush the inactive slot, and on
startup choose the highest-generation slot that passes schema and checksum
validation. Never overwrite the only valid slot. This recommendation derives from
the documented gaps above; it is not a guarantee supplied by Win32 or Rust.

Useful Rust seams are:

```rust
trait SnapshotPublisher {
    fn publish(&self, bytes: &[u8]) -> std::io::Result<()>;
    fn remove(&self) -> std::io::Result<()>;
}

struct SnapshotEnvelope<T> {
    schema_version: u32,
    generation: u64,
    payload: T,
    // Required for an A/B store, computed over a canonical payload encoding.
    checksum: Option<String>,
}
```

Keep serialization/model capture above this platform seam. Keep the file replacement,
flush, directory synchronization, and Windows error handling below it. This makes
fault-injection tests possible without embedding platform filesystem details in the
domain model.

### Persistence acceptance tests

- Crash/fault injection at every step (create, partial write, file sync, replace,
  directory sync, cleanup) leaves the previous snapshot loadable or the new snapshot
  fully loadable; startup ignores temp files.
- A newer schema warns and leaves the file untouched. A malformed envelope, failed
  checksum, or impossible generation is rejected without partial restore.
- Only one writer exists per profile; tests attempting a second writer fail before
  either can publish.
- Windows replacement tests hold the destination open with and without delete sharing
  and verify that failure retains the prior snapshot and dirty state.
- Empty-state removal is subjected to the same directory-durability and error-reporting
  rules as publication.

## Profile-scoped persistent directories

### Primary-source facts

- The XDG Base Directory Specification applies to Linux and similar environments,
  not to “Unix” as a universal platform rule. It defines `XDG_DATA_HOME` for
  user-specific data and `XDG_STATE_HOME` for state retained between application
  restarts, explicitly giving current layout/open-file state as examples. Their
  defaults are `$HOME/.local/share` and `$HOME/.local/state`, respectively. It also
  says `XDG_RUNTIME_DIR` is session-bound and must not survive logout or reboot
  ([XDG Base Directory Specification 0.8](https://specifications.freedesktop.org/basedir/)).
- Apple directs macOS applications to locate the per-user Application Support
  directory through the system API and append an application/bundle-identifier
  subdirectory. It is intended for app-managed state and autosave files
  ([Apple, Accessing Files and Directories](https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/AccessingFilesandDirectories/AccessingFilesandDirectories.html),
  [macOS Library Directory Details](https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/MacOSXDirectories/MacOSXDirectories.html)).
- Windows defines `FOLDERID_LocalAppData` as a per-user known folder whose default is
  `%USERPROFILE%\AppData\Local`; `SHGetKnownFolderPath` resolves the actual location,
  including redirection, for the current user
  ([Microsoft `KNOWNFOLDERID`](https://learn.microsoft.com/en-us/windows/win32/shell/knownfolderid),
  [Microsoft `SHGetKnownFolderPath`](https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/nf-shlobj_core-shgetknownfolderpath)).

### Recommendation

Correct the issue decision from “`XDG_DATA_HOME` on Unix” to an explicit platform
table:

| Platform | Snapshot root |
| --- | --- |
| Linux | Preserve the approved decision with `$XDG_DATA_HOME/<app-id>` (default `$HOME/.local/share/<app-id>`), or explicitly amend it to `$XDG_STATE_HOME/<app-id>` if restartable layout is classified as state. Do not switch silently. |
| macOS | Per-user `Library/Application Support/<bundle-id>` resolved through the platform directory API. |
| Windows | `FOLDERID_LocalAppData\<publisher>\<app-id>` resolved through the Known Folder API. |

The Rust `directories` crate's `ProjectDirs::data_local_dir` implements the approved
three-platform data-location mapping—XDG data on Linux, Application Support on macOS,
and LocalAppData on Windows
([crate API](https://docs.rs/directories/latest/directories/struct.ProjectDirs.html#method.data_local_dir)).
Using it is reasonable, but persist the chosen application identifiers as constants
and test the resolved suffixes; a future product rename must not silently orphan
existing snapshots.

## Debounce and maximum staleness

### Herdr is a trailing debounce, not a bounded checkpoint interval

Herdr defines `SESSION_SAVE_DEBOUNCE` as five seconds
([Herdr `app/mod.rs`, lines 42-47](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/src/app/mod.rs#L42-L47)).
Every dirty synchronization resets `session_save_deadline` to `Instant::now() +
SESSION_SAVE_DEBOUNCE`
([Herdr `app/session.rs`, lines 13-24](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/src/app/session.rs#L13-L24)).
This is a true trailing-edge debounce: mutations arriving more frequently than every
five seconds can postpone the write indefinitely. Herdr's implementation therefore
does not prove “a crash loses at most one five-second window.”

### Recommendation: quiet debounce plus hard checkpoint deadline

Track both the first unsaved mutation and the most recent mutation with monotonic
time. For a quiet debounce `D` and maximum start delay `M`, schedule:

```text
quiet_deadline = last_mutation_at + D
hard_deadline  = first_dirty_at + M
next_save_at   = min(quiet_deadline, hard_deadline)
```

For the approved five-second window, use `M = 5s`; `D` may also be five seconds to
preserve the behavior for isolated edits, or shorter if quicker quiet-state saves are
worth the extra writes. Repeated mutations update `last_mutation_at` but never
`first_dirty_at`, so continuous activity cannot move the hard deadline.

Model scheduling by generation, not one boolean:

```rust
struct SaveSchedule {
    current_generation: u64,
    durable_generation: u64,
    first_dirty_at: Option<std::time::Instant>,
    last_mutation_at: Option<std::time::Instant>,
    in_flight_generation: Option<u64>,
}
```

When a write starts, capture generation `G`. Mutations during that write advance the
current generation and remain dirty. A successful publication advances
`durable_generation` only to `G`; it clears dirty timing only when no newer generation
exists. A failed save advances nothing, retains dirty state, reports the error, and
retries with bounded backoff. A clean-exit flush waits for the current generation to
be durably published and propagates failure rather than treating process exit as a
successful flush.

No scheduler can honestly guarantee completion within five wall-clock seconds when
the OS stalls I/O or storage returns errors. Specify the healthy-system invariant as
“start a checkpoint no later than five seconds after the first unsaved mutation,”
measure completion latency separately, and only claim “at most five seconds lost” if
tests and telemetry establish a completed-durable-checkpoint interval within that
bound.

### Scheduling acceptance tests

Use a fake monotonic clock and injectable publisher to prove:

- one mutation saves at the quiet deadline;
- mutations arriving continuously just before the quiet deadline still start a save
  at the original hard deadline;
- a mutation arriving during an in-flight save schedules a later save and is never
  cleared by the older generation's success;
- publication failure retains dirty state and retries without a busy loop;
- a successful clean-exit flush joins any in-flight save and publishes the newest
  generation; and
- wall-clock jumps do not affect deadlines.

## Restore disposition and schema safety

### Separate durable intent from live lifecycle

The issue decision says that only Panes whose Terminal Session was `Running` at the
last write relaunch after cold restore, while its dirty scope names only structural
and metadata mutations. Those rules do not compose safely. A natural process exit
can leave a persisted `Running` value indefinitely, and a later cold restore can
re-execute something that had already ended. Conversely, if graceful Core shutdown
kills children before the final snapshot, shutdown-induced `Exited` values can
incorrectly suppress the approved relaunch behavior.

Persist a **Restore Disposition** independent of live `TerminalLifecycle`:

```rust
enum RestoreDisposition {
    Relaunch,
    RemainEnded,
}
```

- Starting or explicitly restarting a Terminal Session sets `Relaunch`.
- A natural exit or failure sets `RemainEnded` and advances the persistence
  generation.
- Graceful Core stop, OS logout, and update shutdown do not change the disposition
  merely because the Core terminates their child processes.
- Closing the Pane removes the persisted Pane and its disposition.
- Cold restore never reuses a Terminal Session ID. `Relaunch` creates a new Running
  Terminal Session; `RemainEnded` creates a new ended representation for the restored
  Pane without claiming process continuity.

This preserves the already approved product behavior while making the write trigger
and shutdown ordering explicit. The bounded checkpoint policy limits—but cannot
eliminate—the crash window between a natural exit and publication of
`RemainEnded`.

### Do not collapse incompatible or corrupt state into “absent”

The snapshot loader should return a typed result such as:

```rust
enum SnapshotLoad<T> {
    Ready(T),
    Absent,
    IncompatibleNewer { schema_version: u32 },
    Corrupt { reason: String },
}
```

`IncompatibleNewer` and `Corrupt` must preserve the original files and disable
automatic publication for that profile until the user upgrades, selects a validated
backup, or explicitly resets persistence. Treating either result as `Absent` would
allow an older or damaged installation to start empty and later overwrite the only
recoverable snapshot. Older schemas should pass through explicit, tested migrations;
lenient defaults are appropriate only for fields whose absence has defined semantics.

### Restore and schema acceptance tests

- a natural exit advances the durable generation and restores as ended;
- shutdown-induced child exits do not change a `Relaunch` disposition;
- a Running Pane restored from `Relaunch` receives a new Terminal Session ID;
- an ended Pane never launches a process during restore;
- a newer-schema or corrupt primary file is never overwritten by background save or
  empty-state cleanup; and
- an older-schema migration either produces a fully valid current envelope or leaves
  the source untouched.

## Required corrections to the issue #7 decision record

1. Replace “reuse the acknowledged Stop command” with the version-independent,
   authenticated recovery endpoint and self-termination design above.
2. Replace “temp-file-plus-rename” as a complete durability claim with the explicit
   publication/flush contract and platform adapter above; choose A/B snapshots if
   sudden-power-loss survival must be a hard cross-platform invariant.
3. Replace “five-second debounced save” as the basis for bounded loss with a trailing
   debounce plus non-resetting maximum checkpoint deadline.
4. Replace “`XDG_DATA_HOME` on Unix” with the explicit Linux, macOS, and Windows path
   table, and decide openly whether Linux restartable layout is data or XDG state.
5. Replace persisted “last Running state” with a Restore Disposition independent of
   live process lifecycle, and include natural exit/failure in the dirty generation.
6. Treat newer-schema and corrupt snapshots as write-blocking load outcomes rather
   than as an empty profile.
7. Either add crash-surviving platform process-tree containment or weaken the claim
   that every hosted process is dead after Core crash/emergency termination.
