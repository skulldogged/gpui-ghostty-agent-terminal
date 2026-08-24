# Separate the Resident Core from UI Clients over local IPC

**Status: Superseded by ADR 0004.**

The Resident Core is a separately restartable per-user process, reached through a versioned app-owned local IPC protocol and a small `CoreClient` interface. It alone owns Terminal Sessions; GPUI is an attachable UI Client that may disconnect without ending them. We rejected an in-process registry because it would survive a window close but not a UI Client crash, quit, or update, and would leave presentation code responsible for process lifetime.

The protocol permits multiple attached UI Clients but exactly one Control Lease per Terminal Session. The lease names its controlling UI Client and carries a generation that every input and resize command must match; this keeps queued commands from a former controller from crossing a transfer. Other clients attach as observers, can read snapshots and terminal-change events, and receive structured lease denials for control commands. The controller may transfer the lease to a connected observer; disconnecting the controller vacates and advances the lease rather than auto-promoting an observer, which must explicitly acquire it. The Resident Core and Terminal Sessions remain live throughout.
