# Space, Tab, Pane, and Terminal Session model

This document resolves the structural decisions tracked in issue #9. `CONTEXT.md` remains the canonical glossary; this document records behavior and invariants.

## Identity and ownership

- Every Space, Tab, Pane, and Terminal Session has an opaque, immutable ID that is never reused.
- The Resident Core is the sole owner of Spaces and live Terminal Sessions. A UI Client holds projections and references by ID.
- One authoritative Core model owns the complete Space/Tab/Pane hierarchy. Callers submit typed semantic commands and observe revisioned immutable snapshots rather than receiving mutable entity references.
- A separate terminal runtime registry owns live Terminal Session objects. The Core model stores their stable IDs and durable launch intent, never PTY/ConPTY handles or libghostty-vt state.
- A Space owns an ordered list of Tabs and an initial directory. Git repository and worktree information are optional metadata, not identity.
- A Tab owns one split tree. Every leaf names a Pane; a Pane references exactly one Terminal Session.
- A Terminal Session appears in at most one Pane. Its identity and lifetime do not change when its Pane moves between Tabs or Spaces.
- A recognized agent decorates a Terminal Session. It never replaces the terminal or Pane model, and failed integration leaves an ordinary interactive Terminal Session.

## Lifecycle and commands

- Creating a Space also creates its initial Tab, Pane, and Terminal Session unless the caller explicitly supplies a restored structure.
- Closing a native window or disconnecting a UI Client is Detach. It changes no Space, Tab, Pane, or Terminal Session state.
- Closing a Pane is an explicit destructive command. The UI confirms when work may be active; after acknowledgement the Resident Core stops the referenced Terminal Session and removes the Pane atomically.
- Moving a Pane atomically removes and reinserts the same Pane placement and Terminal Session reference. Launch-time environment hints may become stale and never confer identity or authority.
- A live Terminal Session is `Running`, `Stopping`, `Exited`, or `Failed`. Structural cold restoration creates a new Terminal Session identity and must not claim live-process continuity.
- Restore Disposition is durable intent independent of that live lifecycle. `Relaunch` creates a new Running Terminal Session during cold restore; `RemainEnded` creates a new ended representation without launching a process. Natural exit/failure changes the disposition to `RemainEnded`; Core shutdown, logout, or update cleanup does not change it merely because the Core stops the child.
- Tabs and Panes have stable explicit ordering. Reordering does not change IDs. Focus, selected Tab, window geometry, sidebar state, and transient selection belong to each UI Client rather than the Resident Core.
- A UI Client applies acknowledged hierarchy revisions in order. It may update ephemeral focus, animation, selection, and drag-preview state locally, but a structural change is authoritative only after the Resident Core accepts it.

## Persistence

The Resident Core persists Space identity and directory, Tab identity/name/order, split trees, Pane identity, Terminal Session launch intent and working directory, Restore Disposition, and optional agent-native resume references. It does not persist PTY/ConPTY handles, live process identity, UI focus, window state, or implicit repository identity. Terminal history is a separate opt-in store because it may contain secrets.

Persistence is generation-based. A quiet debounce may coalesce mutations, but a non-resetting hard deadline starts a checkpoint no later than five seconds after the first unsaved mutation. Clean Core shutdown waits for the newest generation to publish. Natural Terminal Session exit/failure is a persistent mutation; shutdown-induced process exit is not.

Snapshot publication guarantees that an application crash exposes either the prior complete snapshot or the new complete snapshot, never a partially serialized target. Machine-crash and sudden-power-loss durability are best effort through platform-specific flush and replacement adapters. A newer-schema or corrupt snapshot is a write-blocking load outcome rather than an empty profile; the original remains untouched until upgrade, validated recovery, or explicit reset.
