# Use one tray-resident application process

The application owns Spaces, Tabs, Panes, Terminal Sessions, native windows, and desktop presence in one OS process. Closing the last window leaves that same process running in the tray. Opening from the tray creates or focuses a window over the existing state. Choosing Quit stops the terminal runtime and exits the process.

Terminal work may remain on a dedicated worker thread so PTY/ConPTY I/O and libghostty-vt processing never block GPUI. That is an implementation boundary, not a second service: calls use in-process channels and project-owned values without IPC, authentication, protocol versions, endpoints, reconnect logic, or control leases.

The application does not promise survival across process quit, crash, update, or restart. It does not persist the live hierarchy or terminal launches. This deliberately trades cross-process survival for a smaller, easier-to-maintain Herdr-like application model.

This decision supersedes ADR 0001 and ADR 0002.
