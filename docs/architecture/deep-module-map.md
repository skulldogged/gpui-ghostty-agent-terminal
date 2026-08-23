# Deep module map

This document resolves the boundaries tracked in issue #5. It describes the target shape for the first usable multiplexer; it does not require a mechanical rewrite before Spaces, Tabs, and Panes can be implemented.

## Packaging decisions

- Ship one versioned executable. It selects Desktop Shell/UI Client or Resident Core behavior at startup, while those roles continue to run as separate OS processes.
- Keep one Cargo package for the first usable release. Use private modules and project-owned data types as the compile-time boundaries.
- Extract a crate only for demonstrated independent linkage, dependency exclusion, external reuse/versioning, or a dependency rule that module privacy repeatedly fails to enforce.
- Do not create pass-through crates whose public API merely mirrors a neighboring module.

One executable does not mean one process, and one package does not mean shared ownership. Runtime authority remains divided exactly as `CONTEXT.md` and the existing ADRs specify.

## Ownership and interfaces

| Deep module | Owns and hides | Narrow interface | Must not own or expose |
| --- | --- | --- | --- |
| `core_model` | Space/Tab/Pane structure, stable IDs, placement invariants, aggregate revision | typed semantic commands, revisioned outcomes, immutable snapshots, effect intents | process handles, sockets, filesystem I/O, GPUI types, libghostty-vt types |
| `terminal_runtime` | registry of live Terminal Sessions, ordered PTY/ConPTY I/O, libghostty-vt state, resize barriers, process lifecycle | session commands, lifecycle facts, rendered terminal snapshots and invalidations | Pane placement, UI state, agent-specific protocols |
| `core_service` | single serialized command path and coordination of model transitions with runtime and integrations | handle semantic Core commands, attach clients, publish acknowledged revisions and events | platform transport details, wire encoding, GPUI rendering |
| `core_ipc` | authenticated local connection, application codec, recovery-v1 codec, pressure classes, reconnect/resnapshot behavior | concrete `CoreClient` and a private server adapter around `core_service` | domain mutation outside `core_service`, raw PTY bytes, Rust memory layouts |
| `agent_integration` | capability detection, negotiated sidecars, adapter health, agent-derived activity and attention evidence | attach/detach by Terminal Session ID, capability snapshots, normalized agent events and commands | terminal or Pane lifetime, mandatory launch behavior, authority when an adapter fails |
| `ui` | GPUI entities, client-side acknowledged projection, fixed-cell rendering, ephemeral focus/selection/drag state | user intents to `CoreClient`; revisioned projections and terminal frames from it | authoritative Spaces, Terminal Sessions, protocol structs, filesystem I/O |
| `desktop_shell` | windows, tray/status item, activation, profile selection, presentation quit behavior | open/focus/detach UI views and explicitly request destructive Core recovery/stop flows | Spaces, Terminal Sessions, implicit Core shutdown |
| `app` | mode selection, dependency construction, startup and shutdown order | executable entry points | domain rules or subsystem implementation logic |

The table names responsibilities, not mandatory filenames. Existing proven seams such as `TerminalSession`, `CoreClient`, `CoreDriver`, and `DesktopShell` can move toward this map incrementally while their behavior remains covered by tests.

## Authoritative state flow

`core_model` is a deterministic aggregate, not a container of independently locked entities. A command such as creating a Space, splitting a Pane, moving a Pane, or closing a Pane is validated against one revision and produces one semantic outcome. Multi-resource invariants are therefore checked and committed together:

- every Pane belongs to exactly one Tab split tree;
- every Pane references exactly one Terminal Session identity;
- one Terminal Session appears in at most one Pane;
- moving a Pane preserves both identities;
- only the Resident Core advances authoritative revisions.

Callers never receive mutable Space, Tab, or Pane references. They submit typed commands and receive an acknowledgement containing the accepted revision or a structured rejection. Successful commits publish ordered semantic events and an immutable snapshot or delta suitable for client projection.

Live runtime objects do not enter `core_model`. The model stores stable Terminal Session IDs and launch parameters; `terminal_runtime` maps those IDs to live `TerminalSession` instances. When a semantic command requires an external effect, `core_service` coordinates it and feeds its typed outcome back through the model. Process launch, exit, and failure remain explicit rather than being inferred from whether a runtime object happens to exist.

## Runtime lifetime

The Resident Core keeps the authoritative hierarchy and Terminal Sessions alive across UI Client detach. It does not serialize the hierarchy or launch parameters. A new Resident Core begins with a fresh default hierarchy, which keeps the current product behavior predictable while the interaction model is still changing quickly.

## Core Client and UI flow

`CoreClient` is a concrete facade, not a trait or a collection of remote entity proxies. It owns connection and protocol behavior internally and exposes semantic commands, revisioned snapshots, terminal invalidations, reliable events, and structured errors. Raw application protocol enums and wire fields remain private to `core_ipc`.

The UI maintains an acknowledged, read-only projection. It applies snapshots or deltas only in revision order and resnapshots after an unknown gap. Optimistic authoritative hierarchy changes are deferred until a real latency problem justifies rollback machinery. Focus, animation, selection, window geometry, and drag previews may update locally because they are presentation state; completing a drop still requires an acknowledged Core command.

Terminal visual updates remain a separate pressure class from semantic hierarchy events. The Resident Core may coalesce visual invalidations because a newer terminal snapshot is authoritative. It may not coalesce or silently drop accepted hierarchy, lifecycle, lease, or agent-attention events.

## Terminal and rendering boundary

`terminal_runtime` keeps the proven `TerminalSession` seam. PTY/ConPTY bytes reach libghostty-vt unchanged and in order; libghostty-vt remains the terminal-state authority. Platform transports, FFI layouts, reader barriers, child containment, and process handles remain internal. The runtime exports project-owned rendered cell snapshots and lifecycle facts, never libghostty-vt or OS handle types.

The `ui` module converts those cell snapshots into GPUI-specific fixed-cell frames. It owns font measurement, shaping-to-cell placement, viewport geometry, cursor presentation, and drawing. Wide-cell trailing positions are not independent glyphs. A resize originates from verified UI cell metrics, but the accepted Core command updates libghostty-vt and the platform PTY/ConPTY through the ordered `TerminalSession` barrier before acknowledgement.

## Optional agent boundary

Agent integrations attach to an existing Terminal Session ID through capability-negotiated adapters. The base Terminal Session and Pane model contain no required agent protocol and no special agent Pane variant. An adapter can contribute normalized activity, attention, resume, approval, and command capabilities, but `core_service` remains the authority that evaluates whether a requested operation is allowed.

Adapter absence, incompatibility, crash, malformed output, or authentication failure degrades to the unchanged interactive terminal. Agent-specific protocol structs stay inside the corresponding adapter. The detailed normalized capability and authority contract remains the subject of issue #10.

## Dependency rules

The intended dependency direction is:

```text
app
├── desktop_shell ──> ui ──> core_ipc::CoreClient
└── core_ipc::server ──> core_service
                         ├── core_model
                         ├── terminal_runtime
                         └── agent_integration
```

Additional rules keep those arrows meaningful:

- `core_model` depends only on project-owned value types and deterministic helpers.
- `terminal_runtime` and `agent_integration` do not call one another; `core_service` coordinates them.
- `core_ipc` translates between private wire messages and service commands at one boundary.
- GPUI dependencies stop at `ui` and `desktop_shell`.
- libghostty-vt FFI and PTY/ConPTY dependencies stop at `terminal_runtime`.
- Platform lifecycle APIs stop at `desktop_shell`; platform process APIs stop at `terminal_runtime`.
- Application composition constructs modules but does not accumulate business rules.

## Verification strategy

- Pure model tests exercise command rejection, identity preservation, split-tree invariants, revision ordering, and snapshot determinism without a process or filesystem.
- Core service tests use controllable runtime and agent adapters to cover side-effect success, failure, and event ordering.
- Terminal runtime conformance runs the same behavioral cases through Unix PTY and Windows ConPTY implementations and verifies byte ordering, resize barriers, exit, cleanup, and process-tree containment.
- IPC tests cover authentication, version negotiation, framing limits, reconnect, sequence gaps, and slow clients without importing wire types into UI tests.
- UI tests consume project-owned snapshots and verify fixed-cell, wide-glyph, cursor, projection, and drag-preview behavior without owning a terminal process.
- Agent adapter contract tests prove that adapter failure leaves the ordinary terminal path usable.

## First multiplexer slice

This decision intentionally avoids a preparatory crate migration. The next vertical slice can add the `core_model` command/snapshot types and implement one visible path end to end:

1. create and list Spaces;
2. create/select Tabs inside a Space;
3. split, focus, move, and close Panes while preserving Terminal Session identity;
4. render the acknowledged hierarchy in the GPUI shell; and
5. detach and reconnect without ending its Terminal Sessions.

Agent-specific UI is not a prerequisite for this slice. Issue #10 can define the optional capability contract without delaying the basic Herdr-like multiplexer experience.
