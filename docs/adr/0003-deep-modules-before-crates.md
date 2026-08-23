# Prefer deep modules before physical crate splits

The first usable release remains one executable, one process, and one Cargo package. Inside the package, deep modules own domain state, terminal runtime, agent integration, GPUI presentation, desktop lifecycle, and application composition behind narrow project-owned interfaces.

We rejected a crate per subsystem for the initial release. Crates would make some dependency rules compiler-visible, but most proposed crates currently have one caller and would add public types, manifests, feature plumbing, and coordinated versioning without hiding more complexity. Module privacy and compile-time dependency checks are sufficient while the first multiplexer vertical slice establishes the real seams.

The Application is coordinated through one authoritative model rather than independently mutable Space, Tab, and Pane objects. Callers submit typed semantic commands and observe revisioned immutable snapshots and events. Live Terminal Session objects remain in a worker-owned runtime registry so PTY work does not block GPUI. Agent integrations are optional adapters attached by Terminal Session ID and never become a prerequisite for an interactive terminal.

A module becomes a crate only when a concrete pressure justifies the physical boundary: a second executable links it independently, platform or FFI dependencies must be excluded from another build, a separately versioned reusable library is required, or repeated accidental dependencies cannot be prevented with module visibility and tests. Extraction must preserve the existing project-owned interface; creating a crate is not permission to expose its implementation types.
