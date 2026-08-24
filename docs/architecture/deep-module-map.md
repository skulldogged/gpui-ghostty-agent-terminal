# Deep module map

This document records the internal boundaries for the first usable multiplexer. They are code-organization boundaries inside one application process, not services or protocols.

## Packaging

- Ship one executable, one OS process, and one Cargo package.
- Keep subsystems private and exchange project-owned value types.
- Extract a crate or process only after a concrete requirement demonstrates that the simpler boundary cannot work.

## Ownership and interfaces

| Module | Owns and hides | Narrow interface |
| --- | --- | --- |
| `core_model` | Space/Tab/Pane structure, stable IDs, placement invariants, aggregate revision | typed commands and immutable snapshots |
| `application_core` | live Terminal Session registry, ordered PTY/ConPTY work, model/runtime coordination, terminal invalidations | in-process commands, snapshots, and events |
| `terminal_session` | one live shell, transport, libghostty-vt state, resize barrier, cleanup | input, paste, resize, render update, lifecycle |
| `core_driver` | background delivery and coalescing for a native window | window commands and projection updates |
| `gui` | GPUI entities, fixed-cell rendering, focus, selection, and drag state | user actions and project-owned snapshots |
| `application` | application lifetime, windows, tray/status item, activation, quit | open/focus window and explicit quit |
| `agent_integration` | optional capability detection and adapters | attach by Terminal Session ID and degrade to an ordinary terminal |

The authoritative state flow is:

```text
application
├── application_core ──> core_model
│                    └── terminal_session ──> PTY/ConPTY + libghostty-vt
└── gui ──> core_driver ──> application_core
```

`core_model` remains deterministic and never owns process handles or terminal parser state. `application_core` applies structural changes and their runtime effects on one worker thread, then publishes revisioned snapshots. The worker exists to keep terminal work off GPUI's event thread; it has no address, authentication, wire format, version handshake, or restart contract.

Closing every native window leaves `application` and `application_core` alive through the tray. Choosing Quit drops the application runtime, stops Terminal Sessions, and exits. A new process always starts a fresh default hierarchy.

PTY/ConPTY bytes reach libghostty-vt unchanged and in order. The UI receives rendered cells and lifecycle facts, never raw handles or Ghostty FFI types. Agent integration remains optional: absence, incompatibility, or failure must leave the normal interactive terminal unchanged.

## Verification

- Model tests cover identity, hierarchy invariants, revision ordering, and command rejection.
- Application-core tests cover structural/runtime coordination, terminal updates, exit, and cleanup.
- Terminal tests cover byte ordering, resize barriers, wide cells, transport handles, and process-tree cleanup on every platform.
- UI tests cover selection, rendering, and interaction using project-owned snapshots.
