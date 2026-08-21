# T3 Code terminal architecture

Research date: 2026-08-19

Primary source examined: current [`pingdotgg/t3code` at `f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3`](https://github.com/pingdotgg/t3code/tree/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3), plus the [exact official source revision `fee10def1afb63cecd9a626aeba2bb063828cf22`](https://github.com/pingdotgg/t3code/tree/fee10def1afb63cecd9a626aeba2bb063828cf22) named by the locally installed T3 server (`0.0.39-mainfee10def.pi.8c06b768`). The installed package depends on `node-pty` 1.1.0 and its client bundle contains a 630,932-byte `ghostty-vt` WASM asset plus the 112-byte PTY-callback trampoline described by the repository. Both official revisions implement the same terminal architecture; claims below cite current source unless a dependency source is more direct.

## Executive answer

T3 Code's current desktop terminal is **not xterm.js, Ghostty's application, or Ghostty's renderer**. It is a split system:

1. A server process owns each live PTY and shell process.
2. PTY output travels to the client as raw terminal data over WebSocket RPC; input and resize travel back over the same terminal contracts.
3. The web/Electron renderer loads an official `libghostty-vt` build as WebAssembly. It uses the C ABI for parsing, terminal/grid state, graphemes, scrollback, selection, keyboard/paste/mouse encoding, and OSC 8 metadata.
4. T3's own TypeScript adapter converts Ghostty render state into snapshots and paints them with Canvas 2D. Browser code—not Ghostty—owns font shaping, the hidden IME textarea, clipboard/DOM events, device-pixel-ratio handling, cursor animation, and the actual drawing.

T3 documents this boundary directly: PTYs remain server-owned, renderer choice never crosses the wire, and the web client reads `libghostty-vt` render state into Canvas 2D ([terminal renderer architecture, lines 1-27](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/docs/architecture/terminal-renderers.md#L1-L27)). Its adapter README explicitly says it is not an xterm compatibility layer and assigns the WASM runtime, terminal core, Canvas renderer, and browser surface to separate modules ([web terminal README, lines 1-19](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/web/src/terminal/ghostty/README.md#L1-L19)).

This is highly relevant to a GPUI foundation spike: T3 proves the official `libghostty-vt` ABI is usable as a headless terminal engine without adopting Ghostty's windowing or renderer. Our equivalent should link the native library and translate its render state into GPUI draw primitives instead of reproducing T3's WASM/Canvas bridge.

## Stack by responsibility

| Responsibility | T3 Code implementation | Where it runs |
| --- | --- | --- |
| Shell process and pseudo-terminal | `PtyAdapter`; normally `node-pty` 1.1.0, alternatively Bun's terminal API on non-Windows | Server/backend process |
| Live terminal-session ownership | `TerminalManager`, keyed by thread ID and client-chosen terminal ID | Server/backend process |
| Wire protocol | WebSocket RPC snapshot/event stream plus write/resize/open/restart/close calls | Between server and client |
| VT parsing and terminal semantics | Official `libghostty-vt` C ABI, compiled to `wasm32-freestanding` | Browser/Electron renderer |
| Text shaping and drawing | T3 Canvas 2D renderer and browser font APIs | Browser/Electron renderer |
| Keyboard, IME, clipboard, pointer, selection UI | T3 browser surface; semantic encoding delegated to `libghostty-vt` where available | Browser/Electron renderer |
| React | Lifecycle/component integration only; deliberately excluded from terminal frames | Browser/Electron renderer |

The server-side abstraction is deliberately narrow: spawn returns a process with `pid`, `write`, `resize`, `kill`, `onData`, and `onExit` ([`PtyAdapter.ts`, lines 32-65](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/server/src/terminal/PtyAdapter.ts#L32-L65)). This keeps terminal-session orchestration independent of `node-pty` or Bun.

## `libghostty-vt`: exactly what T3 uses

### Provenance and build

T3 pins Ghostty source to commit [`9f62873bf195e4d8a762d768a1405a5f2f7b1697`](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/native/libghostty-vt/VERSION#L1). Its build script checks out exactly that revision and invokes Zig with `-Demit-lib-vt`, `-Dtarget=wasm32-freestanding`, `ReleaseSmall`, stripping, and revision-bearing build metadata ([build script, lines 68-121](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/web/scripts/build-libghostty-wasm.sh#L68-L121)). The macOS/Linux cases earlier in that script select a Zig **build host**; they are not distinct runtime terminal implementations ([lines 29-51](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/web/scripts/build-libghostty-wasm.sh#L29-L51)). The resulting platform-neutral WASM runs in Chromium on macOS, Linux, and Windows.

T3 treats the pin as a single source of truth and tests the WASM-reported build revision, artifact budget, and repeated create/write/free behavior ([architecture doc, lines 29-42](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/docs/architecture/terminal-renderers.md#L29-L42)).

### Runtime and object ownership

One `GhosttyRuntime` is lazily shared per browser tab. It owns the WASM instance, memory, ABI type layouts, and PTY callback registry; failed initialization clears the singleton so a later attempt may retry ([`runtime.ts`, lines 22-65 and 213-220](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/web/src/terminal/ghostty/runtime.ts#L22-L65)). Each visible terminal separately allocates a Ghostty terminal, render state, row/cell iterators, key encoder/event, mouse encoder/event, and scratch structures ([`core.ts`, lines 168-190 and 218-290](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/web/src/terminal/ghostty/core.ts#L168-L190)).

Incoming PTY data is encoded to bytes and passed to `ghostty_terminal_vt_write`; resize calls `ghostty_terminal_resize` ([`core.ts`, lines 293-337](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/web/src/terminal/ghostty/core.ts#L293-L337)). Key events first copy terminal modes into Ghostty's key encoder, then populate Ghostty key/modifier/text fields and call `ghostty_key_encoder_encode`; paste checks bracketed-paste mode and calls `ghostty_paste_encode` ([`core.ts`, lines 450-537](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/web/src/terminal/ghostty/core.ts#L450-L537)). Mouse encoding, word/line selection, scrollback, and hyperlinks similarly go through the ABI rather than a parallel JS terminal model.

`libghostty-vt` may generate replies to the hosted process—for example terminal device-query responses. WASM cannot directly call an arbitrary JS closure, so T3 loads a tiny second WASM module and installs its function into Ghostty's indirect-call table. The runtime includes a WebKit-specific grow-then-set workaround ([`runtime.ts`, lines 184-210](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/web/src/terminal/ghostty/runtime.ts#L184-L210)). This trampoline is a web embedding detail, not something a native Rust/GPUI client should need.

### Renderer boundary

T3 asks Ghostty for render state and materializes row/cell snapshots; it does not call a Ghostty GPU renderer. Its Canvas renderer measures the browser font, computes the grid, redraws only dirty rows when possible, batches background spans and text runs, and draws decorations and cursors itself ([`renderer.ts`, lines 63-109 and 125-218](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/web/src/terminal/ghostty/renderer.ts#L63-L109)). A `GhosttyTerminalSurface` owns the canvas, invisible textarea, resize observer, font metrics, selection, scrollbar, and animation state ([`surface.ts`, lines 480-584](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/web/src/terminal/ghostty/surface.ts#L480-L584)). It coalesces drawing with `requestAnimationFrame`, reads a fresh Ghostty snapshot, and sends that snapshot to the Canvas renderer ([`surface.ts`, lines 1582-1645](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/web/src/terminal/ghostty/surface.ts#L1582-L1645)).

The hidden textarea and explicit composition handling are important: terminal-core keyboard encoding does not eliminate platform text-input work. T3 suppresses Safari/IME double delivery and separately handles composition commits, clipboard paste, and ordinary input ([`surface.ts`, lines 980-1041 and 1087-1129](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/web/src/terminal/ghostty/surface.ts#L980-L1041)). GPUI will need an equivalent IME/clipboard/accessibility surface even with `libghostty-vt` underneath.

### Not xterm.js

The current adapter explicitly says it is not an xterm compatibility layer ([README, lines 1-4](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/web/src/terminal/ghostty/README.md#L1-L4)), and the current web package has no xterm dependency ([`apps/web/package.json`, lines 14-50](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/web/package.json#L14-L50)). A source comment notes that this Ghostty renderer replaced an earlier xterm.js renderer ([`core.ts`, lines 340-345](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/web/src/terminal/ghostty/core.ts#L340-L345)). `TERM=xterm-256color` is merely the child-process compatibility declaration; it does not mean xterm is the emulator.

## PTYs and platform adaptations

### macOS and Linux

Under Node, T3 imports `node-pty` and wraps it behind `PtyAdapter`; before spawning on Unix it finds the native `spawn-helper` and best-effort marks it executable ([`NodePtyAdapter.ts`, lines 25-68](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/server/src/terminal/NodePtyAdapter.ts#L25-L68)). T3 is locked to `node-pty` 1.1.0 ([lockfile, lines 479-481](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/pnpm-lock.yaml#L479-L481)). That dependency implements Unix PTYs with `forkpty(3)` ([node-pty 1.1.0 README, lines 1-13](https://github.com/microsoft/node-pty/blob/1def5774632305246fe21f0f69e23a664d6c5910/README.md#L1-L13); [`forkpty` call](https://github.com/microsoft/node-pty/blob/1def5774632305246fe21f0f69e23a664d6c5910/src/unix/pty.cc#L390-L416)).

If the server itself runs under Bun, T3 selects `BunPtyAdapter`, which uses `Bun.spawn(..., { terminal: ... })` and maps its write/resize/data/exit surface into the same interface ([`BunPtyAdapter.ts`, lines 120-153](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/server/src/terminal/BunPtyAdapter.ts#L120-L153)). Server startup dynamically selects Bun or Node based on the runtime ([`server.ts`, lines 133-143](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/server/src/server.ts#L133-L143)).

The Unix shell preference is `$SHELL`, then `/bin/zsh`, `/bin/bash`, `/bin/sh`, then PATH-resolved equivalents. Zsh receives `-o nopromptsp` ([`Manager.ts`, lines 448-503 and 542-573](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/server/src/terminal/Manager.ts#L448-L503)). Linux AppImage-specific environment entries are scrubbed before shell spawn so the packaged application's loader paths do not contaminate terminal children ([`Manager.ts`, lines 1060-1100](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/server/src/terminal/Manager.ts#L1060-L1100)).

### Native Windows

Windows uses the Node adapter; T3 explicitly rejects Bun PTYs on `win32` and tells the user to run the Node version ([`BunPtyAdapter.ts`, lines 10-18 and 120-124](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/server/src/terminal/BunPtyAdapter.ts#L10-L18)). `node-pty` supports Windows using ConPTY on supported Windows versions, with its legacy WinPTY path retained for older systems ([node-pty 1.1.0 README, lines 5-14](https://github.com/microsoft/node-pty/blob/1def5774632305246fe21f0f69e23a664d6c5910/README.md#L5-L14); [selection in `WindowsPtyAgent`](https://github.com/microsoft/node-pty/blob/1def5774632305246fe21f0f69e23a664d6c5910/src/windowsPtyAgent.ts#L59-L74)). T3's own Node adapter explicitly calls this the ConPTY path and injects `TERM=xterm-256color` because node-pty's Windows path does not derive `TERM` from the `name` option ([`NodePtyAdapter.ts`, lines 141-167](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/server/src/terminal/NodePtyAdapter.ts#L141-L167)).

T3 tries `pwsh.exe`, Windows PowerShell, `ComSpec`, and explicit `cmd.exe` locations in order, adding `-NoLogo` for PowerShell ([`Manager.ts`, lines 491-521 and 542-561](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/server/src/terminal/Manager.ts#L491-L521)). It also uses PowerShell/CIM instead of POSIX `ps` to inspect the terminal's child process tree and derive activity labels ([`Manager.ts`, lines 690-785](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/server/src/terminal/Manager.ts#L690-L785)).

### Windows Subsystem for Linux

T3 can additionally launch a Linux backend inside a selected WSL distribution. The Electron main process starts `wsl.exe --exec ... node <server>`, and the renderer talks to that backend over HTTP/WebSocket at either the WSL address or loopback ([`DesktopBackendConfiguration.ts`, lines 502-515 and 594-608](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/desktop/src/backend/DesktopBackendConfiguration.ts#L502-L515)). Packaged Windows builds ship a Linux `node-pty` prebuild for this backend; the source explicitly avoids compiling it on first launch ([lines 470-494](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/desktop/src/backend/DesktopBackendConfiguration.ts#L470-L494)). The WSL backend omits the Windows-side T3 home so Windows and Linux do not share a database or environment identity ([lines 448-466 and 526-550](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/desktop/src/backend/DesktopBackendConfiguration.ts#L448-L466)).

Thus WSL is not a special PTY bridged into a Windows server. It is another server environment running Linux PTYs; the same local Electron renderer attaches over the normal protocol.

## Process, IPC, detach, and history model

The desktop shell is Electron ([desktop package, lines 1-28](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/desktop/package.json#L1-L28)). Electron's main process spawns the T3 server as a separate Node-mode child and waits for its HTTP readiness endpoint ([`DesktopBackendConfiguration.ts`, lines 366-414](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/desktop/src/backend/DesktopBackendConfiguration.ts#L366-L414); [`DesktopBackendManager.ts`, lines 435-499](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/desktop/src/backend/DesktopBackendManager.ts#L435-L499)). The BrowserWindow loads the application URL served/proxied from that backend ([`DesktopWindow.ts`, lines 619-624 and 747-750](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/desktop/src/window/DesktopWindow.ts#L619-L624)).

`TerminalManager` owns an in-memory session map. A session contains the cwd, status, history, dimensions, PTY process handle, data/exit subscriptions, and runtime environment ([`Manager.ts`, lines 241-266 and 297-300](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/server/src/terminal/Manager.ts#L241-L266)). PTY data is serialized into ordered session events, appended to capped history, persisted, and fanned out to listeners ([`Manager.ts`, lines 1630-1726](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/server/src/terminal/Manager.ts#L1630-L1726)).

The terminal wire contract is snapshot plus events. `terminal.attach` subscribes first, obtains/creates the session, delivers a snapshot, replays events buffered during that race, and then switches to live delivery ([`Manager.ts`, lines 2351-2405](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/server/src/terminal/Manager.ts#L2351-L2405)). WebSocket handlers expose attach as a stream and write/resize/open/restart/close as RPC methods ([`ws.ts`, lines 2042-2075](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/server/src/ws.ts#L2042-L2075)). Resize commands are client-coalesced with latest-wins scheduling ([client terminal state, lines 64-73](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/packages/client-runtime/src/state/terminal.ts#L64-L73)).

**Detach fact:** ending an attach subscription only removes the listener; it does not close the session or PTY. The process therefore survives a renderer reload or client disconnect while the backend stays alive. T3 explicitly relies on this after an Electron renderer crash: it reloads the renderer, which rehydrates from the unaffected backend while agents keep running ([`DesktopWindow.ts`, lines 691-724](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/desktop/src/window/DesktopWindow.ts#L691-L724)).

**Server-restart fact:** T3 does not hand PTY handles across backend replacement. The terminal manager's finalizer kills all live PTY processes, while output history is persisted as files and read when a new in-memory session is opened ([`Manager.ts`, lines 2116-2143 and 2145-2190](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/server/src/terminal/Manager.ts#L2116-L2143)). This is history restoration, not process continuity.

Replay has a subtle safety mechanism worth copying. The server strips stored device-query sequences that would produce new replies when replayed, and the client temporarily detaches Ghostty's PTY-reply callback while it resets and replays history ([server sanitizer rationale, lines 799-818](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/server/src/terminal/Manager.ts#L799-L818); [`core.resetAndWrite`, lines 303-323](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/apps/web/src/terminal/ghostty/core.ts#L303-L323)). Otherwise historical queries could inject fresh bytes into the current shell.

## Reusable lessons for the GPUI + `libghostty-vt` foundation spike

The items in this section are recommendations inferred from T3's implementation, not claims that T3 prescribes our architecture.

1. **Use `libghostty-vt` as a deep native module, not as the entire terminal app.** Give one Rust module ownership of Ghostty handles, byte ingestion, resize, mode queries, key/paste/mouse encoding, selections, render-state iteration, and deterministic cleanup. Expose project-owned snapshots/damage records to GPUI. Do not let Ghostty ABI calls spread through views.

2. **Keep rendering native to GPUI.** T3 shows that the ABI's render state is sufficient to build a renderer. Our first spike should prove: feed recorded VT bytes, extract cells/damage/cursor, shape text, draw backgrounds/glyph runs/decorations/cursor, and handle wide/grapheme cells. Do not attempt to embed Ghostty's Metal/OpenGL application renderer.

3. **Separate one shared library/runtime from per-terminal state.** T3 shares the compiled module but gives each visible terminal its own terminal/render/iterator/input handles. The native equivalent is a process-wide library adapter plus strictly per-terminal RAII state, with destruction tests and thread-affinity documented.

4. **Put PTY ownership behind an interface independent of VT state.** T3's six-operation PTY seam is a good minimum. For our resident core, define the interface in terms of byte streams, resize, child lifecycle, and process-tree observation. Implement Unix PTYs and Windows ConPTY behind it; do not make GPUI or `libghostty-vt` aware of the platform backend.

5. **Use one renderer architecture on all desktop platforms.** T3's WASM/Canvas path is the same on macOS, Linux, and Windows; platform differences live under the PTY, shell resolver, process inspection, and host input integration. We should aim for the same split with native `libghostty-vt` + GPUI on all three.

6. **Design the resident-core protocol around attach snapshots plus ordered deltas.** T3 subscribes before snapshotting to close the snapshot/live race. Our version should add an explicit monotonic sequence/revision and reconnect cursor or required resnapshot behavior. Terminal output/damage should have bounded queues and coalescing distinct from reliable lifecycle/control messages.

7. **Decide where authoritative VT state lives before supporting multiple clients.** T3's PTY is server-owned but every client parses its own byte history into its own Ghostty state. That is simple and makes renderer choice local, but it duplicates parsing and leaves resize/input arbitration implicit. A multiplexer with multiple windows or remote observers should likely keep authoritative terminal state in the resident core and send damage/snapshots, or else define one size/input owner and robust replay for every parser.

8. **Treat history replay as active input, not inert text.** Copy T3's query stripping/callback detachment principle. Better yet, distinguish live byte stream, sanitized replay log, and a serialized cold-start snapshot. Never allow replay to answer the current PTY.

9. **Preserve raw bytes across our process boundary.** T3's public contracts model terminal `data` as strings ([terminal contract, lines 152-168](https://github.com/pingdotgg/t3code/blob/f2d5fc91e3030e5c3956fdadc13e1eaa25bcabe3/packages/contracts/src/terminal.ts#L152-L168)). Our Rust core and native ABI are naturally byte-oriented; a binary or byte-string protocol avoids accidental Unicode transcoding and makes chunk-boundary behavior explicit.

10. **Budget for input-system work in the spike.** `libghostty-vt` supplies terminal-aware encoders, not OS text services. Acceptance criteria should include dead keys/IME, non-US layouts, bracketed paste, Kitty keyboard reports, mouse tracking with a selection bypass modifier, clipboard, focus reporting, DPR/scaling, and accessibility behavior—not only ANSI color output.

11. **Pin and test the Ghostty ABI.** Follow T3's single revision file, recorded upstream license, build-info check, ABI-layout smoke test, artifact-size budget, and repeated allocate/write/free test. Add golden VT recordings shared across macOS, Linux, and Windows so renderer or ABI updates can be compared deterministically.

## Bottom line

T3 Code is strong evidence for the proposed foundation: official `libghostty-vt` can be the cross-platform semantic core while a host-native UI owns rendering and interaction. Its most reusable seams are `PTY process -> raw bytes -> terminal core -> host render snapshot`, a narrow PTY adapter, snapshot-before-live attach, and strict per-terminal handle ownership. Its least reusable pieces are the WASM callback trampoline, Canvas renderer, and client-local authority model. For a GUI multiplexer, keep those seams but move process and likely terminal-state authority into the resident Rust core, with GPUI as a reconnectable client.
