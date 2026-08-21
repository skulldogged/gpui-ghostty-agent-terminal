# Herdr architecture: lessons for a resident terminal core

Research basis: first-party Herdr documentation and source at commit
[`a5c69be`](https://github.com/herdrdev/herdr/tree/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f).
This report distinguishes Herdr's documented behavior from conclusions inferred from
its implementation.

## Model and hierarchy

Herdr describes itself as a terminal workspace manager: real terminal processes are
the foundation and the product adds organization and agent awareness around them. Its
documented hierarchy is:

```text
Session (persistent server namespace)
└── Workspace (project or work context)
    └── Tab (one terminal layout)
        └── Pane (a real terminal location)
            └── Agent (a recognized process currently occupying the pane, if any)
```

A named **Session** is a separate server namespace with its own workspaces, tabs,
panes, sockets, and persisted runtime state. Herdr recommends using workspaces for
normal organization and named sessions only when complete runtime isolation is
needed. A **Workspace** is the top-level project container, a **Tab** is a layout
inside it, and a **Pane** is a real terminal. An **Agent** is not a parallel layout
primitive: it is a process Herdr recognizes inside an existing pane
([concepts](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/docs/next/website/src/content/docs/concepts.mdx#L6-L24),
[sessions](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/docs/next/website/src/content/docs/concepts.mdx#L51-L71),
[automation model](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/docs/next/website/src/content/docs/agent-automation.mdx#L8-L18)).

This separation lets shells, tests, servers, and unsupported agents remain ordinary
terminal occupants. Unsupported agents still run normally; they simply lack rich
semantic state until an integration or socket report supplies it
([agent support](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/docs/next/website/src/content/docs/agents.mdx#L10-L49)).

## Workspace semantics

**Documented behavior.** A workspace is a “top-level project or work context.” It is
created with an optional working directory and automatically receives an initial tab
and root pane. Git worktrees are normal workspaces augmented with checkout provenance
and optional grouping
([CLI reference](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/docs/next/website/src/content/docs/cli-reference.mdx#L115-L146)).

**Source-based conclusion.** A workspace is directory-rooted, but it is not required
to represent a Git repository. Creation resolves a cwd, while Git discovery returns
optional metadata and falls back to ordinary directory-derived identity when no repo
exists
([create parameters](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/src/api/schema/workspaces.rs#L7-L17),
[creation path](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/src/app/api/workspaces.rs#L39-L70),
[optional Git identity](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/src/workspace.rs#L63-L75)).
Herdr's separate “space” concept is therefore best understood as optional repo/worktree
grouping in the sidebar, not as the universal owner of arbitrary terminal layouts.

For this project, a Space/Workspace should likewise be a user-defined work context
with an initial directory, not a synonym for repository. Repo and worktree metadata
should be optional capabilities attached to that context.

## Ownership, detach, and restart

Herdr's background server owns panes and process state; one or more clients provide
the attached terminal UI. Detaching or closing a client leaves the server, PTYs,
shells, agents, tests, and servers running. Stopping and restarting the server is a
different event: original processes are gone, but Herdr restores workspace, tab,
pane, cwd, layout, and focus from a snapshot. Panes become fresh shells unless a
supported agent can resume from a native session reference. Screen history is an
independent, opt-in replay mechanism rather than process continuity
([client/server ownership](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/docs/next/website/src/content/docs/concepts.mdx#L65-L77),
[survival matrix](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/docs/next/website/src/content/docs/session-state.mdx#L8-L46),
[agent restore](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/docs/next/website/src/content/docs/session-state.mdx#L48-L89)).

The structural snapshot confirms this distinction. It stores workspace identity,
cwd, optional worktree provenance, public pane/tab numbering, BSP layouts, focus,
pane labels, launch commands, and native agent session references. It does not store
live process or PTY handles. Restore explicitly creates fresh shells and remaps
internal pane IDs
([snapshot schema](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/src/persist/snapshot.rs#L14-L136),
[restore entry point](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/src/persist/restore.rs#L64-L92)).

Herdr also has experimental live handoff for transferring PTYs during server
replacement. Even successful handoff may interrupt client sockets, subscriptions,
waits, and in-flight API calls, so clients must reconnect and retry
([handoff semantics](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/docs/next/website/src/content/docs/session-state.mdx#L91-L108)).

## Pane placement, terminal identity, and moves

The implementation separates placement from runtime. A Workspace stores Tabs; each
Tab stores its BSP layout and `PaneState` map. Live PTYs, parsers, detector tasks, and
channels live in a server-owned `TerminalRuntimeRegistry`, keyed by `TerminalId` and
kept outside pure application state
([Workspace](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/src/workspace.rs#L177-L208),
[Tab](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/src/workspace/tab.rs#L38-L52),
[runtime registry](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/src/terminal/runtime_registry.rs#L5-L34)).

A pane belongs to one tab layout at a time. Moving it removes the same `(PaneId,
PaneState)` from one tab and inserts it into another tab, a new tab, or a new
workspace without replacing the attached terminal runtime
([move implementation](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/src/workspace/tab.rs#L464-L528),
[move destinations](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/src/api/schema/panes.rs#L72-L103)).

Herdr's public pane IDs are workspace-qualified. Consequently, a cross-workspace move
changes the public pane ID even though the terminal remains live. The launch-time
`HERDR_PANE_ID`, `HERDR_TAB_ID`, and `HERDR_WORKSPACE_ID` environment values cannot be
rewritten in the running process, so Herdr retains the old pane ID as an alias
([documented move behavior](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/docs/next/website/src/content/docs/cli-reference.mdx#L180-L199)).

That is a caution, not a pattern to copy. Herdr issue
[#2012](https://github.com/herdrdev/herdr/issues/2012) documents how inherited or
detached processes can carry a stale `HERDR_PANE_ID` and resolve unrelated context.
Our intrinsic `TerminalSessionId` should therefore be immutable and independent of
containment. A pane should be a mutable placement relationship pointing to a terminal
session. Workspace-qualified display addresses may exist, but must not be process
identity or authority; environment variables should be treated as hints, not proof.

## Protocol seams

Herdr's headless server listens on two local seams: a JSON control/event socket and a
private binary render/input client socket. It initializes application state and PTYs,
renders virtually, streams frames, routes client input, and continues after clients
disconnect
([headless server](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/src/server/headless.rs#L1-L15)).

The public API offers a schema, request/response commands, subscriptions, and a
normalized `session.snapshot`. A client bootstraps from the snapshot, applies resource
events to its cache, and requests another snapshot after reconnect or suspected
staleness
([socket API](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/docs/next/website/src/content/docs/socket-api.mdx#L6-L34),
[snapshot/event workflow](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/docs/next/website/src/content/docs/socket-api.mdx#L93-L132),
[snapshot resources](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/src/api/schema/session.rs#L8-L23)).
Its private client transport also separates reliable control messages from droppable,
coalescible render updates and prioritizes control under backpressure
([client transport](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/src/server/client_transport.rs#L650-L689)).

## Recommendations for Resident Core + GPUI Client

1. **Let the Resident Core be the sole runtime owner.** It should own PTY/ConPTY
   handles, libghostty terminal state, child lifecycle, terminal sessions, agent
   evidence and authority, persistence, and semantic events. GPUI should own windows,
   visual layout, selection, modals, colors, animation, and other presentation state.
2. **Model placement separately from execution.** Use a hierarchy such as `Session ->
   Workspace -> Tab -> PanePlacement`, where each placement references an independent
   `TerminalSession`. An `AgentOccupant` or `AgentObservation` decorates the terminal
   and may appear, exit, or be replaced.
3. **Use snapshot plus revisioned deltas.** Include protocol version, aggregate and
   per-resource revisions, ordered event sequence numbers, and an explicit resnapshot
   path. Reconnect should be routine, not exceptional.
4. **Separate transport pressure classes.** Semantic commands, acknowledgements, and
   lifecycle events must be reliable. Terminal damage/grid deltas may be coalesced or
   dropped in favor of a newer snapshot. GPUI should paint terminal state locally,
   rather than receiving server-rendered application frames.
5. **Make input and resize ownership explicit.** Permit many observers, but grant a
   clear lease to the client controlling input and canonical terminal dimensions;
   takeover and loss behavior must be specified.
6. **Name continuity honestly.** Distinguish live attachment, cold-restored shell,
   agent resume in progress, history replay, and unavailable process. Replayed pixels
   are not a running process.
7. **Keep presentation persistence client-side.** Herdr currently persists sidebar
   width and collapsed UI rows in its structural session snapshot even though its own
   architecture guardrail says presentation belongs to the client
   ([snapshot fields](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/src/persist/snapshot.rs#L14-L29),
   [boundary guardrail](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/AGENTS.md#L66-L81)).
   We should avoid that coupling from the outset.
8. **Store terminal history separately and conservatively.** Herdr keeps pane history
   opt-in and separate because it can contain credentials, prompts, and command output
   ([history warning](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/docs/next/website/src/content/docs/session-state.mdx#L35-L46)).
9. **Give each agent state one authority.** Cooperative lifecycle integrations should
   be authoritative when complete; heuristic screen/process detection should be a
   fallback, not a competing writer
   ([authority model](https://github.com/herdrdev/herdr/blob/a5c69beabfc82d9c3f9563eb821139b2e0f3e14f/docs/next/website/src/content/docs/agents.mdx#L39-L49)).

The central architectural lesson is simple: the GUI is an attached view and control
surface, while terminal sessions are resident resources. Spaces, tabs, panes, and
agent affordances should organize and observe those resources without becoming their
lifetime owners.
