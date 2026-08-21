# Cross-platform terminal foundations: GPUI + Ghostty

Research date: 2026-08-19

## Executive conclusion

Building the application for macOS and Linux immediately is viable. Building it for Windows at the same time is also technically viable, but only if “libghostty” means the supported, headless `libghostty-vt` terminal-state library and the application supplies its own GPUI rendering, PTY/process, clipboard, font, input/IME, and platform integration.

It is **not** currently sound to base a three-platform product on Ghostty's full internal embedder (`include/ghostty.h`) or to expect to place Ghostty's existing Metal/OpenGL terminal surface inside a GPUI view. Upstream explicitly says that internal API is tailored to the macOS app, is not for external use, and directs external embedders to `libghostty-vt`. Only the VT library is supported and tested on Windows. [Ghostty internal embedder header](https://github.com/ghostty-org/ghostty/blob/4c6215bb8ee186b5c829457a9a9a9c936f2337bf/include/ghostty.h#L1-L15), [maintainer clarification](https://github.com/ghostty-org/ghostty/discussions/11610#discussioncomment-16213878)

The recommended foundation is therefore:

```text
GPUI application and terminal view
        |
        +-- app-owned TerminalBackend interface
        |       |
        |       +-- pinned libghostty-vt adapter (parser, grid, render state,
        |       |   key/mouse encoding, snapshots)
        |       |
        |       +-- optional fallback/test backend
        |
        +-- app-owned GPUI renderer and input/IME adapter
        |
        +-- app-owned ProcessTransport interface
                +-- Unix PTY (macOS/Linux)
                +-- ConPTY (native Windows)
                +-- WSL transport later, if desired
```

This is an inference from the sources, not an upstream-prescribed architecture. It follows the supported boundary and matches the pattern used by working cross-platform consumers.

## Support matrix

| Layer | macOS | Linux | Windows | Confidence and qualification |
| --- | --- | --- | --- | --- |
| GPUI application/windowing | Supported | Supported (Wayland and/or X11) | Supported (Win32) | High. GPUI documents all three, Zed ships on all three. |
| GPUI renderer | Metal | `wgpu` through Wayland/X11 backends | DirectX 11 | High, from GPUI source and shipped Zed builds. |
| Official Ghostty desktop app | Supported | Supported (GTK) | Not supported | High. Official feature docs still say Windows is future work. |
| `libghostty-vt` | Supported | Supported | Supported and CI-built | High, but API signatures are unstable. |
| Full `libghostty-internal` embedder and Ghostty renderer | macOS app's private integration | Not a supported external API | Not a supported external API | High. Do not use as the product boundary. |
| Day-one GPUI + Ghostty VT product | Viable | Viable | Viable, higher integration/QA cost | Architectural assessment based on the verified layers and existing consumers. |

## GPUI platform status

GPUI's current README describes a cross-platform `gpui_platform::application()` entry point and gives platform-specific setup for all requested systems:

- macOS rendering uses Metal.
- Linux/FreeBSD requires Wayland, X11, or both.
- Windows needs no GPUI feature flag; it uses Win32 and DirectWrite.

GPUI remains pre-1.0, is developed primarily as part of Zed, and warns that breaking changes are expected. [GPUI README](https://github.com/zed-industries/zed/blob/45ae0572c422e5e8dcf73157730d747152d0be52/crates/gpui/README.md#L5-L42)

The source layout backs up that documentation. `gpui_platform` selects `gpui_macos`, `gpui_windows`, or `gpui_linux` by target OS; its Linux feature flags select Wayland and X11. [gpui_platform manifest](https://github.com/zed-industries/zed/blob/45ae0572c422e5e8dcf73157730d747152d0be52/crates/gpui_platform/Cargo.toml#L13-L38) The Windows implementation contains Win32 windowing, DirectWrite, and DirectX renderer modules. [Windows platform module](https://github.com/zed-industries/zed/blob/45ae0572c422e5e8dcf73157730d747152d0be52/crates/gpui_windows/src/gpui_windows.rs) The Linux crate defaults to both Wayland and X11 and uses `gpui_wgpu` for either backend. [Linux platform manifest](https://github.com/zed-industries/zed/blob/45ae0572c422e5e8dcf73157730d747152d0be52/crates/gpui_linux/Cargo.toml#L13-L58)

This is not merely compile-time aspiration: Zed presently distributes macOS, Linux, and Windows builds, and its Windows documentation requires a DirectX 11-capable GPU. [Zed README](https://github.com/zed-industries/zed/blob/45ae0572c422e5e8dcf73157730d747152d0be52/README.md#installation), [Zed on Windows](https://github.com/zed-industries/zed/blob/45ae0572c422e5e8dcf73157730d747152d0be52/docs/src/windows.md)

GPUI is also proven as a terminal UI host. Zed's `terminal_view` renders backend-neutral terminal snapshots in GPUI and deliberately isolates its current Alacritty backend behind a boundary intended to make backend experiments reviewable. It documents separate keyboard, GPUI action, IME, and paste paths—useful evidence that a production terminal view needs more than drawing a cell grid. [Zed terminal-view design notes](https://github.com/zed-industries/zed/blob/45ae0572c422e5e8dcf73157730d747152d0be52/crates/terminal_view/README.md)

### GPUI implications

Facts:

- All three requested desktop targets are real GPUI targets today.
- GPUI provides custom canvas/painting facilities, so an app-owned terminal renderer can stay inside the GPUI compositor rather than embedding a foreign native child surface. [GPUI painting example](https://github.com/zed-industries/zed/blob/45ae0572c422e5e8dcf73157730d747152d0be52/crates/gpui/examples/painting.rs)
- Its public API is unstable and tied closely to Zed's development cadence.

Inference:

- Pin GPUI to an exact revision, isolate it behind app UI components, and expect periodic migration work.
- A GPUI-native cell renderer is the lowest-risk cross-platform composition model. Trying to share Ghostty's Metal/OpenGL renderer with GPUI's Metal/`wgpu`/DirectX compositors would create platform-specific GPU ownership, synchronization, and texture/surface integration work with no supported upstream embedder contract.

## What “libghostty” currently means

Ghostty now uses “libghostty” as an umbrella for separately scoped libraries. The available supported foundation is `libghostty-vt`, not the older full application embedder.

`libghostty-vt` provides parsing, terminal state, scrollback, reflow, input encoding, render-state traversal, formatting, selection-related APIs, and terminal snapshots. It has no drawing/windowing layer. The top-level header labels the API incomplete and work-in-progress and explicitly warns that breaking changes are expected. [VT API header](https://github.com/ghostty-org/ghostty/blob/4c6215bb8ee186b5c829457a9a9a9c936f2337bf/include/ghostty/vt.h#L1-L48)

Ghostty's README says the VT implementation works on macOS, Linux, Windows, and WebAssembly. It calls the underlying behavior mature but says the API signatures remain in flux and no libghostty version has yet been tagged. [Ghostty README](https://github.com/ghostty-org/ghostty/blob/4c6215bb8ee186b5c829457a9a9a9c936f2337bf/README.md#cross-platform-libghostty-for-embeddable-terminals)

Ghostling, the official minimal consumer, makes the boundary concrete: it uses `libghostty-vt` for terminal state and its render-state API, while Raylib and application code provide the renderer, windowing, fonts/layout, and the rest of the terminal product. Ghostling has not tested/implemented its own Windows app even though its dependency supports Windows. [Ghostling README](https://github.com/ghostty-org/ghostling/blob/63842bf8e5e481160f81d348da9ff6fd27986798/README.md#what-is-libghostty)

The current build system produces shared and static VT libraries on Windows (`ghostty-vt.dll`, import library, and `ghostty-vt-static.lib`) as well as the usual macOS and Linux forms. Windows static consumers must link additional Windows libraries; disabling SIMD removes the extra SIMD dependency complication. [Ghostty CMake integration](https://github.com/ghostty-org/ghostty/blob/4c6215bb8ee186b5c829457a9a9a9c936f2337bf/CMakeLists.txt) Ghostty's required checks include dedicated Windows `libghostty-vt` and Windows CMake-example jobs. [Ghostty test workflow run showing required jobs](https://github.com/ghostty-org/ghostty/actions/runs/24828009269/workflow)

By contrast, `include/ghostty.h` calls itself `libghostty-internal`, says the macOS app is its only consumer, and says it is tailored to that app rather than external use. The exposed platform enum only names macOS and iOS. [Internal embedder header](https://github.com/ghostty-org/ghostty/blob/4c6215bb8ee186b5c829457a9a9a9c936f2337bf/include/ghostty.h#L1-L31)

### Ghostty desktop on Windows

Official Ghostty runs on macOS and Linux. Its current feature documentation says Windows support is planned for the future, while the 1.3.0 release notes say there is no current Windows plan/timeline and that libghostty work is the path toward it. [Ghostty features](https://ghostty.org/docs/features), [Ghostty 1.3.0 release notes](https://ghostty.org/docs/install/release-notes/1-3-0)

The upstream Windows discussion makes the desired eventual direction clear but unfinished: a native Windows frontend, a Direct3D renderer comparable to the Metal/OpenGL backends, Windows 10/11 at most, and minimal new C++ maintenance. A maintainer explicitly rejects shipping GTK or Qt as the Windows application runtime. [Official Windows support discussion](https://github.com/ghostty-org/ghostty/discussions/2563#discussioncomment-16422365)

Therefore, “official Ghostty supports Windows” has two different answers:

- **Desktop terminal application:** no.
- **Supported terminal-state library (`libghostty-vt`):** yes.

## Windows projects and how they work

These are proof points, not equivalent upstream support. The official project created a separate discussion for unofficial alternatives and custom builds. [Official alternatives thread](https://github.com/ghostty-org/ghostty/discussions/12371)

### Closest technical-stack proof: mightty

Mightty uses almost exactly the proposed foundation: Rust, GPUI, platform PTYs, and `libghostty-vt`. Windows uses ConPTY; Unix uses a `forkpty` bridge. The project renders the terminal through GPUI and owns the terminal widget, cell renderer, Kitty graphics cache, key translation, search, and PTY worker. [Mightty README](https://github.com/frixaco/mightty/blob/f6f974f897907c1b761e0b8867c6becc57edf729/README.md)

Its Ghostty boundary is particularly useful as a reference. It pins Ghostty as a source submodule, builds a static `libghostty-vt`, generates private C bindings, exposes only a project-owned safe Rust layer, records a public-header fingerprint, checks Rust/C ABI layouts, and does not permit raw C types to escape the FFI module. [Mightty Ghostty integration](https://github.com/frixaco/mightty/blob/f6f974f897907c1b761e0b8867c6becc57edf729/docs/ghostty-integration.md)

This is strong design evidence, not maturity evidence: the repository labels itself a highly experimental WIP/PoC and currently has no published GitHub release. Its techniques are worth borrowing, but its existence does not replace our own three-platform spike and compatibility suite.

### Closest architectural proof: Paneflow

Paneflow is a GPUI application for supervising coding agents—the closest public proof point to this project. It ships GPUI builds for Linux, macOS, and Windows; uses `libghostty-vt` by default on Linux and Windows x64 MSVC; maintains Alacritty as a rollback backend; and intentionally still uses Alacritty on macOS. Its author describes safe Rust wrappers, reproducible static Ghostty archives, Unix PTY and Windows ConPTY lifecycles, differential tests, fuzzing, and native CI. [Paneflow README](https://github.com/arthjean/paneflow/blob/f53f982291f75a9daf565827b3167d0e96925d0a/README.md#openai-build-week)

What it proves:

- GPUI + `libghostty-vt` + native Windows ConPTY is workable and shippable.
- A backend-neutral session model and fallback backend are valuable while the VT integration matures.
- Windows ARM64 remains a separate risk; Paneflow defers it pending GPUI DirectX reliability.

What it does not prove:

- A single libghostty-backed implementation already ships across all three systems; Paneflow's macOS release deliberately stays on Alacritty.

### Clean `libghostty-vt` consumers with custom platform/render layers

- **Mite:** imports Ghostty's Zig `ghostty-vt` module. Its Windows target uses a separate native entry point, a ConPTY read thread, Direct3D 11, and DirectWrite; Linux uses direct X11/Wayland paths. It is a compact demonstration of “VT core plus app-owned platform.” [Mite README](https://github.com/marler8997/mite/blob/ae64e7cec6c34631da012d94e67ec84564fb108e/README.md), [Mite build](https://github.com/marler8997/mite/blob/ae64e7cec6c34631da012d94e67ec84564fb108e/build.zig)
- **Liney-win:** uses `libghostty-vt` for terminal state, but implements a C++20 Win32/Direct2D UI, font handling, IME, selection, and related terminal UX itself; it builds with MSVC and Zig. [Liney-win README](https://github.com/everettjf/liney-win/blob/0684b69f975fd53da780dead8ce510501282d69f/README.md)
- **Hollow:** uses Ghostty's VT core with its own Zig/Lua runtime and Sokol-based UI. Windows and WSL are primary; it exposes native Windows shells and offers a WSL PTY-bypass helper with automatic ConPTY fallback. Its WSL helper starts through `wsl.exe` and streams PTY bytes using its own framed transport rather than treating WSL as the GUI runtime. [Hollow README](https://github.com/sudo-tee/hollow/blob/ddd91967cb1b1c3d90b77d017122100ad37c5565/README.md), [Hollow WSL transport](https://github.com/sudo-tee/hollow/blob/ddd91967cb1b1c3d90b77d017122100ad37c5565/src/wsl_bypass.zig)
- **WispTerm:** uses `libghostty-vt` for parsing/state and supplies font discovery (DirectWrite/CoreText/fontconfig), FreeType rendering, and platform GPU/UI layers. It ships Windows and macOS; its Linux AppImage is experimental. [WispTerm README](https://github.com/xuzhougeng/wispterm/blob/0e804636932779fd9e80e06297a77c8a411ead1d/README.md)
- **TildaZ:** uses a shared Zig session/config core with explicit OS seams. Windows supplies ConPTY, Direct3D 11, and DirectWrite; macOS supplies POSIX PTY, Metal, and CoreText; Linux supplies POSIX PTY, Wayland, and its renderer. Its current build matrix publishes artifacts for all three OS families. [TildaZ architecture](https://github.com/ensky0/tildaz/blob/181af82f13e74bb8cf148ab12b0c996b1fbb1e15/ARCHITECTURE.md), [TildaZ README](https://github.com/ensky0/tildaz/blob/181af82f13e74bb8cf148ab12b0c996b1fbb1e15/README.md)

These support the inference that the portable asset is Ghostty's VT state machine, while renderer, process transport, and native integration belong to the host application.

### Full Ghostty forks/ports

- **winghostty:** forks the full Ghostty tree, retains its terminal/font/renderer/input/config/termio internals, removes the macOS and GTK runtimes, and adds a Win32 runtime plus D3D11/DirectComposition chrome and Windows packaging. This is a port/fork, not consumption of a stable native `libghostty` surface API. [winghostty README](https://github.com/amanthanvi/winghostty/blob/6a8353f4ced7124a37993ee2ad08277afa539ae6/README.md#relationship-to-ghostty)
- **GhostInTheWSL:** also forks Ghostty for a Windows UI, but its main target is WSL. It avoids ConPTY for WSL by running a small Linux PTY bridge in the WSL 2 guest and communicating over Hyper-V sockets/VSOCK. The project says its Windows UI foundation comes from another unofficial Ghostty fork until upstream has Windows support. [GhostInTheWSL README](https://github.com/Codavo/ghostinthewsl/blob/215c9673cf183950f4b4941b7dce523167a6aac9/README.md#ghostinthewsl)

These projects show that much of Ghostty proper can be ported, but they do not create a stable dependency seam for a GPUI application. Following that route means carrying a large Ghostty fork and reconciling it with upstream indefinitely.

### Unsupported full-internal embedding

Some Windows prototypes compile Ghostty's full internal DLL and use the embedded application-runtime pattern. One reported WinUI 3/DirectX prototype had basic input, IME, tabs, clipboard, and mouse working but still listed ConPTY exit detection, tab latency, and IME stability problems. [Prototype description in upstream discussion](https://github.com/ghostty-org/ghostty/discussions/2563#discussioncomment-16511237)

Upstream's response to another such consumer is decisive: the full internals are not cross-platform to Windows; only `libghostty-vt` is tested and supported there. [Maintainer answer](https://github.com/ghostty-org/ghostty/discussions/11610#discussioncomment-16213878) This path should be treated as experimental porting work, not an application dependency.

## Architectural consequences for this project

### Recommended decisions

1. **Commit to macOS and Linux as first-class initial platforms.** GPUI and `libghostty-vt` support both without qualification at the layer we need.
2. **Keep Windows in the initial architecture and CI from the first vertical slice.** Native x64 Windows is feasible now. Do not make it depend on upstream Ghostty desktop Windows support.
3. **Define “based on libghostty” precisely as `libghostty-vt` behind an app-owned Rust adapter.** Avoid `include/ghostty.h`, Ghostty renderer internals, and direct Ghostty application-runtime dependencies.
4. **Render the terminal as a GPUI-native component.** Consume libghostty's render state and paint cell backgrounds, glyph runs, decorations, cursor, selections, and eventually images through GPUI. This keeps terminal panes composable with sidebars, overlays, attention indicators, diffs, and drag/drop across all three GPUI backends.
5. **Separate terminal emulation from process transport.** A VT engine should accept byte streams and produce encoded input; it should not know whether those bytes come from Unix PTY, ConPTY, WSL, SSH, a persistent-session daemon, or replayed test data.
6. **Keep a backend-neutral terminal/session model.** A temporary Alacritty fallback is optional, but the seam itself is valuable for differential tests, upgrades, and avoiding UI-wide coupling to libghostty's changing C structs.
7. **Pin exact GPUI and Ghostty revisions.** Wrap every unsafe C call in a narrow crate, own the Rust domain types returned to the rest of the app, and upgrade intentionally with conformance tests.

### Initial release wording

A defensible platform promise would be:

- macOS and Linux: first-class initial targets.
- Windows x64: first-class target developed and tested from the beginning, with release readiness gated by an explicit terminal compatibility checklist rather than by upstream Ghostty's desktop roadmap.
- Windows ARM64: architecturally supported but not promised until GPUI/DirectX, Ghostty static-library, packaging, and test coverage are proven on hardware.

This is more realistic than either “macOS only” or an unconditional promise that every architecture on all three OS families ships simultaneously.

## Required spike before locking the foundation

The evidence is strong enough to choose the architecture, but a small three-OS spike should retire integration risk before the broader product design builds on it:

1. Link a pinned `libghostty-vt` static library into a minimal Rust workspace on macOS, Linux, and Windows x64 MSVC.
2. Feed a PTY/ConPTY byte stream into the VT engine and render its incremental state in one GPUI element.
3. Verify resize/reflow, Unicode graphemes and wide cells, bold/italic/underline, cursor shapes/blink, selection, clipboard, mouse modes, Kitty keyboard input, bracketed paste, and IME composition.
4. Run a full-screen TUI and one target coding agent on each OS.
5. Capture a golden/differential corpus and test that terminal snapshots remain app-owned data across backend upgrades.
6. Confirm static-link packaging and licenses for release artifacts.

The Windows console on `win11` and the macOS console on `canis` are appropriate for this later executable spike. They were not needed for this source-level support investigation.

## Risks to track

- **Two unstable upstreams:** GPUI is pre-1.0; libghostty-vt says its API is unfinished and will break.
- **Renderer completeness:** the VT library deliberately does not provide font shaping/rasterization or final GPU drawing. Correct ligatures, emoji, fallback fonts, combining marks, selection, and Kitty images remain host work.
- **Input/IME correctness:** Zed's terminal notes and multiple Windows prototypes show that terminal keyboard translation and IME are distinct, nontrivial paths.
- **Windows process semantics:** ConPTY launch, resize, exit detection, job/process-tree cleanup, and shell discovery need their own adapter and integration suite. WSL is a separate transport, not a substitute for native Windows support.
- **Platform QA:** Linux means at least Wayland and X11; Windows means DirectX/DirectWrite plus hardware and DPI variation; macOS means Metal and input-method behavior.
- **Upstream pin/build complexity:** current Ghostty releases require a specific Zig version. The build and generated bindings must be reproducible per target.
- **Licensing review:** GPUI crates declare Apache-2.0 and Ghostty is MIT, but the complete resolved Rust/Zig dependency graph and bundled assets still require an automated release-time audit. This report makes no legal conclusion.

## Bottom line

The three-platform vision does not require waiting for official Ghostty on Windows. GPUI already supplies the three native GUI backends, and `libghostty-vt` already supplies the cross-platform terminal semantics. The price of that choice is that this project owns the terminal presentation and host integration—exactly the layer where an agent-focused workspace needs control anyway.

The architecture should therefore target macOS, Linux, and Windows x64 from day one, but should be explicit that it is a **GPUI terminal frontend powered by `libghostty-vt`**, not an embedding of the full Ghostty application or renderer.
