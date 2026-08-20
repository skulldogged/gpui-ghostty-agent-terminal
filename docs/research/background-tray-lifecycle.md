# Background and tray lifecycle for the Resident Core

Research date: 2026-08-19

This report answers [research ticket #3](https://github.com/skulldogged/gpui-ghostty-agent-terminal/issues/3). It uses only current first-party documentation, specifications, and source repositories. Source observations are labelled **Fact**; proposed decisions and compatibility judgments are labelled **Inference**.

## Executive conclusion

Use two long-lived, per-user processes:

```text
Resident Core                               Desktop Shell (one per OS login/profile)
---------------------------                 ----------------------------------------
owns Spaces                                 owns GPUI Application and native windows
owns Terminal Sessions       local IPC      owns tray/status item and its menu
owns PTY/ConPTY + VT state  <------------>  projects Core state into desktop UI
owns agent state                            may legitimately have zero windows
persists structural state                   can crash or quit without killing terminals
```

**Inference — recommended boundary.** The Resident Core must not link GPUI, AppKit, GTK, a Linux status-notifier host, or Windows notification-area code. The Desktop Shell is a UI Client and owns a small app-defined `DesktopPresence` adapter. Closing the last GPUI window detaches; it does not terminate either process. Quitting the Desktop Shell removes the tray and windows but still does not stop the Resident Core. Stopping the Resident Core is a separate, explicit destructive command.

Implement `DesktopPresence` with:

- macOS: `tray-icon` 0.24.x over `NSStatusItem`, created in GPUI's post-launch callback on the main thread; retain direct `objc2-app-kit` as the fallback if the integration spike exposes run-loop conflicts.
- Windows: `tray-icon` 0.24.x over `Shell_NotifyIcon`, created on GPUI's Win32 event-loop thread.
- Linux: `ksni` 0.3.x, a pure D-Bus implementation of StatusNotifierItem and DBusMenu, with no GTK or XEmbed fallback in the first release.

These are implementation dependencies, not domain interfaces. Pin exact versions/revisions and keep their types inside platform adapters.

The premise of the ticket needs one current correction. **GPUI now supports a zero-window application lifetime** through `QuitMode::Explicit`; it still has **no merged tray/status-item API**. Therefore we do not need a hidden GPUI window or an external event loop merely to keep the Desktop Shell alive, but we do need platform integration for the tray itself.

## Facts that constrain the design

### GPUI can remain alive with zero windows, but does not own desktop presence

**Fact.** At Zed commit [`fa00dccc`](https://github.com/zed-industries/zed/tree/fa00dccc42311f8dc71c533105488b0dbd518138), GPUI 0.2.2 exposes `Application::with_quit_mode` and `QuitMode::Explicit`, documented as quitting only when `App::quit` is requested. Its window-removal path does not quit when that mode is selected, even when the window collection becomes empty. [GPUI application API](https://github.com/zed-industries/zed/blob/fa00dccc42311f8dc71c533105488b0dbd518138/crates/gpui/src/app.rs#L217-L282), [quit-mode definition and close behavior](https://github.com/zed-industries/zed/blob/fa00dccc42311f8dc71c533105488b0dbd518138/crates/gpui/src/app.rs#L315-L325), [last-window decision](https://github.com/zed-industries/zed/blob/fa00dccc42311f8dc71c533105488b0dbd518138/crates/gpui/src/app.rs#L1862-L1875)

**Fact.** GPUI's default remains platform-conventional: explicit quit on macOS and last-window quit elsewhere. The Desktop Shell must opt into `QuitMode::Explicit`; relying on the default would still exit after the last Windows or Linux window closes. [GPUI `QuitMode`](https://github.com/zed-industries/zed/blob/fa00dccc42311f8dc71c533105488b0dbd518138/crates/gpui/src/app.rs#L315-L325)

**Fact.** GPUI's current public `Platform` surface includes application menus, Dock/jump-list menus, notifications, reopen registration, and wake handling, but no tray icon, `NSStatusItem`, `Shell_NotifyIcon`, AppIndicator, or StatusNotifierItem abstraction. Desktop lifecycle callbacks explicitly describe mobile lifecycle as separate and say desktop platforms never invoke it. [GPUI `Platform` trait](https://github.com/zed-industries/zed/blob/fa00dccc42311f8dc71c533105488b0dbd518138/crates/gpui/src/platform.rs#L124-L260) An attempted Linux tray API was closed unmerged because Zed had no direct use for it. [Zed PR #13098](https://github.com/zed-industries/zed/pull/13098)

**Fact.** `App::set_menus` is the normal application menu surface; it is not a tray/status item. Treating it as one would conflate the macOS application menu on the left of the menu bar with an `NSStatusItem` on the right. [GPUI menu methods](https://github.com/zed-industries/zed/blob/fa00dccc42311f8dc71c533105488b0dbd518138/crates/gpui/src/platform.rs#L230-L247), [Apple `NSStatusBar`](https://developer.apple.com/documentation/appkit/nsstatusbar)

**Fact.** `Application::on_reopen` documents the already-running-app/Dock behavior specifically for macOS. The macOS backend calls it when AppKit asks the application to reopen and there are no visible windows. Although the Windows and Linux backends store a callback, current source contains no corresponding invocation path. [GPUI reopen API](https://github.com/zed-industries/zed/blob/fa00dccc42311f8dc71c533105488b0dbd518138/crates/gpui/src/app.rs#L270-L282), [macOS reopen handler](https://github.com/zed-industries/zed/blob/fa00dccc42311f8dc71c533105488b0dbd518138/crates/gpui_macos/src/platform.rs#L1326-L1335)

**Inference.** GPUI now provides the correct lifetime primitive for a windowless Desktop Shell, but it does not provide process uniqueness, Windows/Linux second-launch forwarding, tray availability, or recovery. Those remain application responsibilities.

### macOS status item

**Fact.** AppKit's supported API is `NSStatusBar.system.statusItem(withLength:)`, which creates an `NSStatusItem` and adds it to the system menu bar. A status item provides a button and can own an `NSMenu`. Apple warns that menu-bar space is limited, status items are not guaranteed to remain available, and applications should not make them their only control surface. [Apple `NSStatusItem`](https://developer.apple.com/documentation/appkit/nsstatusitem), [Apple `NSStatusBar`](https://developer.apple.com/documentation/appkit/nsstatusbar), [status-item menu](https://developer.apple.com/documentation/appkit/nsstatusitem/menu)

**Fact.** AppKit requires UI work on the application's main thread. [Apple UIKit and AppKit overview](https://developer.apple.com/documentation/technologyoverviews/uikit-appkit)

**Fact.** `tray-icon` 0.24.2 is current and actively maintained by Tauri. Its official documentation supports macOS, Windows, and GTK-based Linux. It requires macOS tray creation on the main thread after the event loop is running; creation from another thread returns `NotMainThread`. Its macOS backend exposes the underlying `NSStatusItem` when platform-specific behavior is needed. [tray-icon platform notes](https://github.com/tauri-apps/tray-icon/blob/1c23131c96ebce1703d5dee17c483cfdc892999b/src/lib.rs#L7-L34), [macOS native handle](https://github.com/tauri-apps/tray-icon/blob/1c23131c96ebce1703d5dee17c483cfdc892999b/src/lib.rs#L527-L533), [0.24.2 manifest](https://github.com/tauri-apps/tray-icon/blob/1c23131c96ebce1703d5dee17c483cfdc892999b/Cargo.toml#L1-L20)

**Fact.** GPUI owns and runs `NSApplication`. It invokes the application's launch callback from `applicationDidFinishLaunching` on the AppKit thread, after setting regular activation policy. [GPUI macOS run loop](https://github.com/zed-industries/zed/blob/fa00dccc42311f8dc71c533105488b0dbd518138/crates/gpui_macos/src/platform.rs#L491-L517), [GPUI did-finish-launching](https://github.com/zed-industries/zed/blob/fa00dccc42311f8dc71c533105488b0dbd518138/crates/gpui_macos/src/platform.rs#L1274-L1308)

**Inference.** Creating `tray-icon` inside GPUI's launch callback is the likely compatible seam: it is on the main thread and inside the active AppKit lifecycle. This has not been promised by either upstream as a supported GPUI pairing, so it needs a macOS spike covering creation, click/menu delivery, closing every window, fullscreen transitions, sleep/wake, and recreation after a Desktop Shell restart.

**Inference.** Keep the application activation policy `Regular` initially. This is a terminal multiplexer with normal windows, so its Dock icon and standard reopen path are valuable. `Accessory` suppresses the Dock and application menu; `Prohibited` may not create windows at all. A future user-facing “menu-bar only” mode would need a deliberate activation-policy spike, not a default architectural assumption. [Apple activation policies](https://developer.apple.com/documentation/appkit/nsapplication/activationpolicy-swift.enum)

### Windows notification area

**Fact.** The native API is `Shell_NotifyIcon`. An icon is registered with a `NOTIFYICONDATA` record; the Shell delivers mouse and keyboard interaction through an application-defined message to the supplied `HWND`. `NIM_SETVERSION` must be called whenever an icon is added. [Microsoft `Shell_NotifyIcon`](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shell_notifyiconw), [Microsoft `NOTIFYICONDATA`](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/ns-shellapi-notifyicondataw)

**Fact.** The owning thread therefore needs a Win32 message pump. GPUI's Windows backend runs `GetMessageW` with no `HWND` filter and dispatches the resulting messages, so it can dispatch messages for another window created on that same thread. [GPUI Windows loop](https://github.com/zed-industries/zed/blob/fa00dccc42311f8dc71c533105488b0dbd518138/crates/gpui_windows/src/platform.rs#L413-L430)

**Fact.** `tray-icon` creates a non-activating hidden tool window, routes notification-area events through its window procedure, and calls `Shell_NotifyIcon`. The current implementation listens for the registered `TaskbarCreated` message, keeps its message window alive if Explorer is not ready at startup, and re-registers after Explorer/taskbar recreation. [tray-icon Windows backend](https://github.com/tauri-apps/tray-icon/blob/1c23131c96ebce1703d5dee17c483cfdc892999b/src/platform_impl/windows/mod.rs#L40-L166), [current Windows fixes](https://github.com/tauri-apps/tray-icon/blob/1c23131c96ebce1703d5dee17c483cfdc892999b/CHANGELOG.md#L3-L19) Microsoft likewise says taskbar applications must re-add their icons after the `TaskbarCreated` broadcast. [Microsoft taskbar creation notification](https://learn.microsoft.com/en-us/windows/win32/shell/taskbar#taskbar-creation-notification)

**Fact.** There is one current accessibility caveat to validate before adopting the crate unchanged. Microsoft says `NIM_SETVERSION` must follow every `NIM_ADD` and defines keyboard selections such as `NIN_KEYSELECT`. The pinned `tray-icon` backend adds the icon but does not call `NIM_SETVERSION`, and its callback branch recognizes legacy mouse messages rather than the documented keyboard/context-menu notifications. [Microsoft `Shell_NotifyIcon`](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shell_notifyiconw), [tray-icon add path](https://github.com/tauri-apps/tray-icon/blob/1c23131c96ebce1703d5dee17c483cfdc892999b/src/platform_impl/windows/mod.rs#L570-L608), [tray-icon callback path](https://github.com/tauri-apps/tray-icon/blob/1c23131c96ebce1703d5dee17c483cfdc892999b/src/platform_impl/windows/mod.rs#L380-L499)

**Inference.** Creating `tray-icon` from GPUI's Windows launch callback should allow GPUI's existing message pump to service the crate's hidden `HWND`. This is strong source-level compatibility evidence, not a contractual guarantee. Test close-all-windows, Explorer restart, startup-before-Explorer, DPI changes, menu focus/keyboard activation, suspend/resume, and Desktop Shell restart. If keyboard access fails, upstream/fix the crate or replace only the Windows adapter with Microsoft's maintained `windows`/`windows-sys` bindings; do not waive notification-area accessibility. [Rust for Windows crate guide](https://github.com/microsoft/windows-rs/blob/master/docs/crates/windows.md)

**Fact.** A Windows service is the wrong place for desktop presence. Microsoft says services cannot interact directly with users on current Windows and recommends a separate GUI process in the interactive user session when a service needs UI. [Microsoft interactive services guidance](https://learn.microsoft.com/en-us/windows/win32/services/interactive-services)

**Inference.** The Resident Core should be an ordinary unelevated per-user background process, not an SCM service. Core and Desktop Shell should run at the same integrity level so IPC and activation behavior remain predictable.

### Linux: StatusNotifierItem is not the X11 system tray

There are three commonly conflated mechanisms:

1. **Legacy X11 system tray / XEmbed.** The freedesktop system-tray protocol locates a tray manager through an X selection and embeds a client X window through XEmbed. It is intrinsically an X11 window protocol, not a native Wayland protocol. [freedesktop system-tray specification](https://specifications.freedesktop.org/systemtray/latest-single/), [XEmbed specification](https://specifications.freedesktop.org/xembed-spec/latest/)
2. **StatusNotifierItem (SNI) + DBusMenu.** An application exports icon/status/menu objects over the user D-Bus; a `StatusNotifierWatcher` and `StatusNotifierHost` supplied by the desktop discover and render them. The current freedesktop catalog still labels SNI version 0.1 a draft. [freedesktop specifications catalog](https://specifications.freedesktop.org/), [SNI item specification](https://specifications.freedesktop.org/status-notifier-item/latest/status-notifier-item.html)
3. **AppIndicator/Ayatana AppIndicator.** Ayatana describes Application Indicators as a GNOME implementation of the SNI specification. It is a library/toolkit route to the same broad desktop-host model, not a Wayland tray protocol owned by the compositor. [Ayatana Indicators](https://ayatanaindicators.github.io/)

**Fact.** SNI travels over D-Bus and does not embed a Wayland or X11 client window. Consequently an SNI item can work under either display protocol, but only when the desktop session provides a compatible watcher/host. Display protocol and desktop support are independent axes.

**Fact.** KDE implements SNI directly; its current framework describes `KStatusNotifierItem` as a D-Bus status-notifier implementation whose actual icon and menu representation belongs to the system tray. [KDE `KStatusNotifierItem`](https://api.kde.org/kstatusnotifieritem.html) GNOME Shell requires an extension for this behavior; Ubuntu's maintained AppIndicator/KStatusNotifierItem extension explicitly adds icon, menu, activate, and secondary-activate support, and documents separate X11-shell-restart and Wayland-logout activation procedures. Tooltips are intentionally not implemented there. [Ubuntu AppIndicator/KStatusNotifierItem extension](https://github.com/ubuntu/gnome-shell-extension-appindicator)

**Inference.** Never report “Linux tray supported” merely because the process is on X11 or Wayland. Runtime capability is `StatusNotifierWatcher/Host is present`, and the UI should be functional if it is absent. A GNOME user without the extension must still be able to reopen from the launcher/task switcher and manage the Resident Core from a window or CLI.

#### Rust implementation choice

**Fact.** `tray-icon`'s Linux backend is GTK-only and uses `libappindicator` or `libayatana-appindicator`; its official example for a non-GTK event loop starts a separate thread, initializes GTK there, creates the tray on that thread, and runs `gtk::main`. [tray-icon Linux notes](https://github.com/tauri-apps/tray-icon/blob/1c23131c96ebce1703d5dee17c483cfdc892999b/src/lib.rs#L13-L34), [tray-icon non-GTK example](https://github.com/tauri-apps/tray-icon/blob/1c23131c96ebce1703d5dee17c483cfdc892999b/examples/winit.rs#L105-L142)

**Fact.** `ksni` 0.3.6 is an active Rust implementation of SNI and DBusMenu. It supports Tokio, a runtime-agnostic `async-io` mode, and a blocking API. It models an unavailable watcher/host explicitly and can remain alive to observe a watcher appearing later. [ksni README at current commit](https://github.com/iovxw/ksni/blob/08c8e0ab8b6adf61e5c0b6c8b5ccb94b3250a53c/README.md#L1-L183), [ksni availability handling](https://github.com/iovxw/ksni/blob/08c8e0ab8b6adf61e5c0b6c8b5ccb94b3250a53c/src/lib.rs#L254-L296), [ksni late-watcher option](https://github.com/iovxw/ksni/blob/08c8e0ab8b6adf61e5c0b6c8b5ccb94b3250a53c/src/lib.rs#L383-L399)

**Inference.** Prefer `ksni` on Linux. It avoids introducing a second GUI toolkit, GTK event loop, and native AppIndicator packaging dependency into a GPUI process. Use its watcher-online/offline behavior to expose `Available`, `Unavailable`, and `Recovering` presence states. Do not implement legacy XEmbed initially: it adds a second protocol, only helps X11, and still cannot make a tray a reliable application control surface.

## Recommended application-owned boundary

`DesktopPresence` should be small, event-based, and incapable of owning domain state:

```rust
trait DesktopPresence {
    fn start(&mut self, initial: PresenceView) -> Result<PresenceCapability>;
    fn update(&mut self, view: PresenceView) -> Result<()>;
    fn set_visible(&mut self, visible: bool) -> Result<()>;
    fn shutdown(&mut self);
}

enum PresenceEvent {
    OpenOrFocus,
    NewSpace,
    OpenSpace(SpaceId),
    ShowAttention,
    QuitDesktopShell,
    RequestStopResidentCore,
}
```

**Inference.** `PresenceView` is a read-only projection produced from a Resident Core snapshot plus Desktop Shell state. Platform callbacks emit intent; they do not mutate Spaces or Terminal Sessions directly. The Desktop Shell sends commands over the normal IPC command path and updates the menu only from acknowledged state. `RequestStopResidentCore` must never be aliased to “Quit,” “Exit,” closing a window, or dropping the tray handle.

**Inference.** Keep notification delivery separate from tray presence even if a platform crate exposes both. GPUI already has system-notification APIs; a missing tray host must not suppress agent-attention notifications, and notification permission failure must not remove the tray.

## Process-topology decision

| Topology | Terminal survival | Event-loop fit | Cost | Decision |
| --- | --- | --- | --- | --- |
| One process: Core + GPUI + tray | Closing windows can be handled, but a GUI/tray crash or app update kills terminals | Simplest native-loop integration | Lowest initially, highest lifecycle coupling | Reject for product architecture |
| Two processes: Resident Core + Desktop Shell containing GPUI and tray | Independent of windows and Desktop Shell failures | One native desktop loop; Linux SNI can use async D-Bus | One local protocol and process supervisor/launcher | **Adopt** |
| Three processes: Core + GPUI windows + dedicated tray host | Maximum desktop isolation | Each UI runtime is isolated | Extra instance coordination, macOS activation complexity, stale menu paths | Defer unless integration testing proves necessary |
| OS system service owns Core | Can be supervised by OS | Wrong interactive-session boundary on Windows; adds permissions/packaging complexity | High | Reject as cross-platform baseline |

The two-process choice is an architectural inference from the platform facts. It preserves the defining Resident Core property without prematurely adding a third desktop process.

## Startup, uniqueness, and reopen

### Normal launch

**Inference.** Every entry point—application icon, CLI, file/URL activation, tray menu, and login startup—should use one launcher protocol:

1. Resolve the current OS user and application profile.
2. Connect to that profile's Resident Core endpoint.
3. If absent, race safely to become/spawn the Core owner; losers connect to the winner.
4. Authenticate the endpoint as same-user and perform a versioned handshake.
5. Connect to the Desktop Shell endpoint.
6. If a shell already exists, forward `OpenOrFocus`/activation payload and exit. Otherwise become the Desktop Shell, attach to the Core, and start GPUI with `QuitMode::Explicit`.

The listening IPC endpoint plus an OS lock/mutex should establish authority; a PID file alone is insufficient because it becomes stale and PIDs are reused. Profile identity belongs in both endpoint names so intentional isolated profiles do not collide.

### Reopen with zero windows

**Fact.** On macOS, GPUI's `on_reopen` is wired to AppKit's reopen callback when no windows are visible. [GPUI macOS reopen](https://github.com/zed-industries/zed/blob/fa00dccc42311f8dc71c533105488b0dbd518138/crates/gpui_macos/src/platform.rs#L1326-L1335)

**Inference.** On macOS, register that callback and make it send the same internal `OpenOrFocus` intent as a status-item click. On Windows and Linux, second-launch forwarding through the Desktop Shell endpoint is authoritative; do not depend on GPUI's stored but currently uninvoked reopen callbacks. A tray click follows the same path on all platforms.

If the Desktop Shell is absent but the Resident Core exists, a normal application launch starts a new Desktop Shell and attaches. If both are absent, it starts both. No path restores process continuity after the Core itself died; it can only restore structural state and explicitly resumable agents.

### Start at login

**Fact.** macOS 13+ provides `SMAppService` for user-approved LoginItems and LaunchAgents inside an application bundle. Registration can start the helper immediately and at later logins; user denial is an explicit result. [Apple `SMAppService`](https://developer.apple.com/documentation/servicemanagement/smappservice), [registration behavior](https://developer.apple.com/documentation/servicemanagement/smappservice/register%28%29)

**Fact.** Packaged Windows desktop applications can declare a startup task, which appears in Task Manager and can be disabled by the user. [Microsoft desktop startup tasks](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/desktop-to-uwp-extensions#start-an-executable-file-when-users-log-into-windows)

**Fact.** Linux desktops standardize per-user login startup through an application `.desktop` file in the XDG autostart directories. [freedesktop autostart specification](https://specifications.freedesktop.org/autostart/0.5/)

**Inference.** “Start at login” should start the Desktop Shell in tray/background mode, and the normal launcher protocol should ensure the Resident Core exists. Do not install a privileged daemon. Preserve the user's platform control: show denied/disabled state rather than repeatedly repairing their choice. A future Linux systemd-user integration may supervise the Core where available, but it cannot replace the XDG desktop-session path as the portable baseline.

## Failure model and required behavior

| Failure or transition | Required behavior |
| --- | --- |
| Last GPUI window closes | Desktop Shell remains in `QuitMode::Explicit`; Resident Core and Terminal Sessions remain live. |
| Desktop Shell/tray crashes or user chooses “Quit Desktop Shell” | Resident Core remains live. Relaunch attaches and resnapshots. |
| User explicitly stops Resident Core | Warn about affected Terminal Sessions, send an acknowledged stop command, persist structural state, then terminate Core. Desktop Shell may remain disconnected or quit by separate choice. |
| Resident Core crashes | UI/tray changes to disconnected; disable state-changing menu items; retry with backoff. Live process continuity is lost unless later architecture implements PTY handoff. Cold restoration must not claim otherwise. |
| Core and Shell start concurrently from multiple activations | Exactly one Core and one Desktop Shell per user/profile win; losing launchers forward their activation payload and exit. |
| IPC endpoint is stale | Verify owner/handshake, remove/recover only while holding the singleton lock, then bind atomically. Never trust a PID file alone. |
| macOS hides/reorders status item or menu-bar space is exhausted | Dock, application menu, launcher, and keyboard command remain valid reopen/control paths, matching Apple's warning that status items are not guaranteed visible. |
| Windows Explorer restarts or was not ready | Re-register the icon on `TaskbarCreated`; current `tray-icon` contains this behavior, but retain an integration test. |
| Linux has no SNI watcher/host | Record `Unavailable`, continue normally without a tray, expose a diagnostic explaining desktop/extension requirements, and watch for later availability. |
| Linux watcher disappears/reappears | Keep Core unaffected; transition presence offline/online and republish current `PresenceView`. |
| Linux runs under Wayland rather than X11 | Use the same D-Bus SNI adapter. Do not start XWayland solely for a legacy tray icon. |
| Sleep/wake or desktop session restart | Treat desktop presence as recreatable; reconnect D-Bus/re-register platform icon as needed and resnapshot from Core. |
| OS logout, reboot, power loss, or update | Persist structural state continuously. All per-user processes may be terminated; on next login cold-restore honestly rather than promising live Terminal Session survival. |
| Tray menu is open while Core state changes | Menu events carry stable IDs, commands are idempotent, and the next projection corrects stale state. Avoid blocking Core IPC from native menu callbacks. |
| Tray dependency panics on its required thread | The Desktop Shell may die, which is acceptable for terminal survival. Capture diagnostics and let normal relaunch reconstruct it. Do not catch-unwind across AppKit/Win32/GTK FFI. |

## Validation needed before locking crate choices

The architecture does not depend on a particular tray crate, but the recommended adapters require a small three-platform spike:

1. Start a GPUI application with `QuitMode::Explicit` and no initial window; create a window later through a foreground task.
2. macOS: create `tray-icon` from GPUI's launch callback; verify click/menu delivery, standard application menus, Dock reopen, close-all, fullscreen transitions, sleep/wake, and clean destruction under Instruments/Address Sanitizer where practical.
3. Windows: create `tray-icon` on GPUI's message-loop thread; verify keyboard activation, popup focus, close-all, Explorer kill/restart, startup-before-Explorer, DPI change, suspend/resume, and clean Desktop Shell restart.
4. Linux: run `ksni` under Plasma Wayland/X11 and GNOME Wayland/X11 with and without the AppIndicator extension. Verify no-watcher startup, watcher appearing/disappearing, menu mutation, activate/secondary-activate, and clean shutdown. Add at least one other supported desktop to the release matrix when selected.
5. Kill and restart only the Desktop Shell while a Core-owned Terminal Session continues producing output; reconnect, resnapshot, and confirm no lost semantic control state.
6. Trigger simultaneous launches and stale-endpoint recovery on all three platforms.

If `tray-icon` conflicts with GPUI on macOS or Windows, keep the same `DesktopPresence` boundary and replace only that platform adapter with direct `objc2-app-kit` or `windows` APIs. If Linux SNI host variance proves unacceptable, the correct fallback is “no tray plus clear diagnostics,” not coupling terminal survival to GTK or an invisible window.

## Decision statement

**Inference — proposed architecture decision.** A per-user Resident Core owns all live Terminal Sessions and agent state. A separately restartable Desktop Shell is the singleton graphical UI Client for an OS login/profile, owns GPUI and desktop presence, opts into GPUI `QuitMode::Explicit`, and is allowed to have zero windows. Desktop presence is optional, capability-reported, and implemented behind an app-owned adapter (`tray-icon` on macOS/Windows; `ksni` on Linux initially). Closing or quitting presentation never implies stopping the Resident Core. Startup and reopen always converge through versioned same-user IPC and singleton activation forwarding.

This design makes terminal survival depend only on the Resident Core, while treating tray/status mechanisms for what the platform sources show they are: useful, native, and inherently less reliable than the application's primary launcher/window surface.
