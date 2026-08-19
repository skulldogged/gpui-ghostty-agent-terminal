# Separate the Resident Core from UI Clients over local IPC

The Resident Core is a separately restartable per-user process, reached through a versioned app-owned local IPC protocol and a small `CoreClient` interface. It alone owns Terminal Sessions; GPUI is an attachable UI Client that may disconnect without ending them. We rejected an in-process registry because it would survive a window close but not a UI Client crash, quit, or update, and would leave presentation code responsible for process lifetime.

The initial protocol permits one controlling UI Client at a time. Its connection is the Control Lease: disconnecting releases input and resize authority, while the Resident Core and Terminal Sessions remain live for a later client to attach and resnapshot.
