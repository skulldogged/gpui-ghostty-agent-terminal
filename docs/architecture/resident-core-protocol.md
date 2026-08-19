# Resident Core and UI Client protocol

This document resolves the protocol choice tracked in issue #2 and defines the target contract that issue #4 will exercise. The initial implementation in this stack is a tracer bullet: real local IPC, protocol negotiation, an exclusive connection-scoped Control Lease, commands, snapshots, detach, and reconnect.

## Process and transport seam

The Resident Core is a separately restartable, unelevated per-user process. It owns all domain state, PTY/ConPTY transports, libghostty-vt state, process lifecycle, persistence, and agent-integration authority. GPUI runs only in a UI Client process.

`CoreClient` is the UI-facing interface. Its adapter uses a Unix-domain local socket on macOS/Linux and a named pipe on Windows through a cross-platform local-socket implementation. The Resident Core restricts Unix peers by effective user ID and socket mode; the Windows duplex pipe requires creator-authorized write access. Platform transport types, raw terminal bytes, and libghostty-vt types do not cross the interface.

## Versioning and attachment

- Every connection begins with a protocol-version and capability handshake. An incompatible major version fails closed with a useful error.
- Attachment returns a complete `CoreSnapshot` containing the aggregate revision plus stable resource IDs and per-resource revisions.
- Reconnect is routine: acquire a new Control Lease, request a fresh snapshot, then subscribe from the snapshot's event sequence. A client must resnapshot after an unknown sequence gap or explicit stale response.
- The initial product permits one controlling UI Client connection. Its connection-scoped Control Lease grants input and canonical resize authority; disconnect releases it without changing terminal lifetime. Later observer connections may read snapshots/events but cannot input or resize.

## Commands, events, and pressure

- Commands carry a request ID and receive exactly one acknowledgement or structured error. Destructive commands are explicit and idempotent where retry is possible.
- Input and resize require the current lease generation. Resize updates libghostty-vt and PTY/ConPTY through the ordered barrier before its acknowledgement.
- Semantic lifecycle events are reliable and ordered by a monotonically increasing sequence. They include resource creation/removal, process exit/failure, lease changes, agent state, and command results.
- Terminal visual updates are a separate pressure class. They carry only invalidation/revision information and may be coalesced; the client requests the latest terminal snapshot. Raw PTY bytes never cross the protocol.
- A slow client never blocks PTY consumption. Coalescible terminal notifications may be replaced by a newer revision. If the bounded reliable queue fills, the Core disconnects that client and requires a resnapshot rather than dropping semantic events.

## Failure behavior

- UI Client disconnect, crash, or normal quit releases its lease while the Resident Core and terminals continue.
- Resident Core failure ends live-process continuity. Restart restores only persisted structure and explicitly supported agent resumes.
- A stale endpoint is never overwritten merely from a PID file. Startup first attempts a versioned handshake and requires singleton ownership before reclaiming an unreachable endpoint.
- Stopping the Resident Core is a separate acknowledged destructive command; closing a window, Pane presentation, or Desktop Shell never aliases it.
