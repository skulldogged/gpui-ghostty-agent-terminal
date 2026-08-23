# Agent Terminal Multiplexer

This context describes a graphical terminal multiplexer that organizes persistent terminal work and adds optional, progressively richer integrations with agent programs.

## Language

**Space**:
A directory-rooted work context containing an ordered collection of Tabs. Repository and worktree metadata are optional enrichment rather than identity. It survives native window closure while the application remains running in the tray; cold layout restoration is not currently supported.
_Avoid_: Workspace, Session

**Tab**:
A named member of a Space that owns one split layout of Panes.
_Avoid_: Window

**Pane**:
A leaf placement in a Tab's split layout that references one Terminal Session. Panes contain terminals in the initial product scope and may move without changing the referenced Terminal Session's identity.
_Avoid_: Panel, Agent Pane

**Terminal Session**:
A live shell or CLI execution stream with an immutable identity independent of Pane placement. It appears in at most one Pane.
_Avoid_: Agent Session

**Application**:
The single tray-resident process that owns Spaces, Terminal Sessions, agent-integration state, and zero or more native windows. Closing every window does not stop the application; choosing Quit does.
_Avoid_: Resident Core, UI Client, Desktop Shell

**Close Window**:
Closing a native presentation window while the Application, Spaces, Panes, and Terminal Sessions continue running. Closing a window never means closing a Pane.
_Avoid_: Quit, Close Pane

**Close Pane**:
An explicit action that removes a Pane and stops its referenced Terminal Session. The UI asks for confirmation only when that session has active work worth protecting. A shell that exits naturally has already ended its own work and closes the corresponding Pane through the same hierarchy mutation without confirmation.
_Avoid_: Close Window

**Agent Integration**:
Optional capabilities that augment a Terminal Session when an agent program is recognized or cooperates with the application.
_Avoid_: Agent Pane, required agent protocol
