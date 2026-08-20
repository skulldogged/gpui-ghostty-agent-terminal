# Resident Core and UI Client protocol

This document resolves the protocol choice tracked in issue #2 and defines the target contract that issue #4 exercises. The implementation includes real local IPC, protocol negotiation, concurrent controller/observer attachments, a generation-checked Control Lease, commands, pushed terminal invalidations, revision-gated terminal snapshots with lifecycle state, detach, and reconnect.

## Process and transport seam

The Resident Core is a separately restartable, unelevated per-user process. It owns all domain state, PTY/ConPTY transports, libghostty-vt state, process lifecycle, persistence, and agent-integration authority. GPUI runs only in a UI Client process.

`CoreClient` is the UI-facing interface. Its adapter uses a Unix-domain local socket on macOS/Linux and a named pipe on Windows through a cross-platform local-socket implementation. The Resident Core keeps Unix sockets in an owner-verified private runtime directory and restricts peers by effective user ID; the Windows duplex pipe requires creator-authorized write access and derives its otherwise opaque name from the protected per-user secret, preventing another local user from predictably claiming the endpoint first. That 256-bit secret authenticates every server handshake with a fresh HMAC challenge before the client sends any commands. Platform transport types, raw terminal bytes, and libghostty-vt types do not cross the interface.

Protocol version 4 uses one little-endian 32-bit length prefix followed by an
explicit binary payload. Requests, acknowledgements, lifecycle state, and
rendered cell data each have stable field encodings; Rust memory layouts are
never copied onto the wire. A complete frame is assembled before it is written,
which avoids turning each terminal-cell field into a separate socket write.
Frames are rejected before allocation when their declared payload exceeds 16
MiB. The same framed stream may carry coalescible terminal invalidations between
command responses; the client distinguishes them before matching the next
response. Raw PTY bytes remain inside the Resident Core and are consumed
unchanged by libghostty-vt; the binary IPC payload represents commands and
rendered state, not a second terminal byte stream.

## Versioning and attachment

- Every connection begins with a protocol-version and authenticated challenge-response handshake. An incompatible major version or invalid server proof fails closed with a useful error. Connect timeouts cover endpoint discovery and this handshake only; established Unix connections clear that temporary socket timeout before normal commands begin.
- Attachment returns a complete `CoreSnapshot` containing the aggregate revision plus stable resource IDs and per-resource revisions.
- Reconnect is routine: attach, inspect or explicitly acquire the Control Lease, request a fresh snapshot, then subscribe from the snapshot's event sequence. A client must resnapshot after an unknown sequence gap or explicit stale response.
- Multiple UI Clients may attach concurrently. The first attachment acquires a vacant Control Lease; later attachments are observers that may read snapshots and terminal invalidations but cannot input or resize. The controller can explicitly transfer the lease to a connected observer. Controller disconnect advances and vacates the lease; an observer must explicitly acquire it rather than being promoted implicitly.

## Commands, events, and pressure

- Commands carry a request ID and receive exactly one acknowledgement or structured error. Destructive commands are explicit and idempotent where retry is possible.
- Input and resize carry the caller's current lease generation. Authorization remains serialized until the terminal worker acknowledges the command, so transfer cannot overtake accepted input or resize. Observer, vacant-lease, stale-generation, and unavailable-transfer-target failures are structured responses rather than strings. Resize updates libghostty-vt and PTY/ConPTY through the ordered barrier before its acknowledgement.
- Semantic lifecycle events are reliable and ordered by a monotonically increasing sequence. They include resource creation/removal, process exit/failure, lease changes, agent state, and command results.
- Terminal visual updates are a separate pressure class. They carry only invalidation/revision information and may be coalesced; the client requests the latest terminal snapshot. Raw PTY bytes never cross the protocol.
- The tracer bullet exposes terminal revision and Running/Exited/Failed state in its snapshots. A conditional snapshot request returns no payload when that revision is unchanged, so an idle UI neither rebuilds Ghostty snapshots nor rerenders. When it has changed, the Resident Core consumes libghostty-vt's dirty-row state and sends an ordered update based on the client's last revision. The UI Client replaces only the named rows in its local snapshot. Attach, resize, an unknown base revision, or a detected sequence gap forces a complete recovery snapshot.
- Protocol version 3 adds a server-pushed `TerminalChange` wake hint carrying an event sequence and terminal revision. Each client subscription has a capacity-one nonblocking queue: a pending visual wake may stand in for newer revisions because the subsequent snapshot is authoritative, and a stalled socket writer never blocks the terminal worker. Disconnect removes the subscription immediately. Reliable semantic event delivery and switching GPUI off polling remain later #4 slices.
- Protocol version 4 gives every connection a stable `UiClientId` and the current `ControlLease` during attachment. Lease inspection, acquisition, and transfer use explicit protocol commands; every successful transition advances its generation. Observer attachment and terminal subscriptions share no process or terminal ownership with the UI Client. Reliable lease-change event delivery and switching GPUI off polling remain later #4 slices.
- A slow client never blocks PTY consumption. Coalescible terminal notifications may be replaced by a newer revision. If the bounded reliable queue fills, the Core disconnects that client and requires a resnapshot rather than dropping semantic events.

## Failure behavior

- UI Client disconnect, crash, or normal quit releases its lease while the Resident Core and terminals continue.
- After final PTY output reaches libghostty-vt, the Resident Core reaps an exited child while keeping its terminal snapshot available for reconnecting clients.
- Resident Core failure ends live-process continuity. Restart restores only persisted structure and explicitly supported agent resumes.
- A stale endpoint is never overwritten merely from a PID file. On Unix, the core holds an exclusive per-endpoint startup lock for its lifetime; only that lock owner may reclaim a same-user socket after a live connection probe fails. Windows named-pipe lifetime is managed by the kernel.
- Stopping the Resident Core is a separate acknowledged destructive command; closing a window, Pane presentation, or Desktop Shell never aliases it.
