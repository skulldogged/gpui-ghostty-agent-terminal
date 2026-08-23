# Agent Terminal Multiplexer

This context describes a graphical terminal multiplexer that organizes persistent terminal work and adds optional, progressively richer integrations with agent programs.

## Language

**Space**:
A long-lived, directory-rooted work context containing an ordered collection of Tabs. Repository and worktree metadata are optional enrichment rather than identity. It survives UI Client detach while its Resident Core is running; cold layout restoration is not currently supported.
_Avoid_: Workspace, Session

**Tab**:
A named member of a Space that owns one split layout of Panes.
_Avoid_: Window

**Pane**:
A leaf placement in a Tab's split layout that references one Terminal Session. Panes contain terminals in the initial product scope and may move without changing the referenced Terminal Session's identity.
_Avoid_: Panel, Agent Pane

**Terminal Session**:
A live shell or CLI execution stream with an immutable identity independent of Pane placement. It appears in at most one Pane and has at most one controlling UI Client at a time.
_Avoid_: Agent Session

**Resident Core**:
The long-lived local process that owns Spaces, Terminal Sessions, and agent-integration state independently of any native application window.
_Avoid_: Hidden Window, GUI Process

**UI Client**:
An attachable GPUI front end whose windows can present any Space without owning that Space or its terminal lifetimes.
_Avoid_: Main Process, Space Window

**Desktop Shell**:
An independently restartable presentation host for one OS login and application profile. It may have zero windows and never owns Spaces or Terminal Sessions.
_Avoid_: Tray Process, Hidden Window

**Detach**:
Closing a UI view while its Space, Panes, and Terminal Sessions continue running in the Resident Core. Detach never means closing a Pane.
_Avoid_: Quit, Stop

**Close Pane**:
An explicit destructive action that immediately removes a Pane and stops its referenced Terminal Session. A shell that exits naturally has already ended its own work and closes the corresponding Pane through the same hierarchy mutation.
_Avoid_: Detach, Close Window

**Control Lease**:
The exclusive relationship granting one UI Client authority to send input and set the canonical size of a Terminal Session. Other UI Clients cannot control that Terminal Session while the lease is held.
_Avoid_: Focus, Ownership

**Agent Integration**:
Optional capabilities that augment a Terminal Session when an agent program is recognized or cooperates with the application.
_Avoid_: Agent Pane, required agent protocol
