# Agent Terminal

Cross-platform graphical terminal-multiplexer foundation built with GPUI and
the supported `libghostty-vt` C interface.

The cross-platform proof is tracked by
[#8](https://github.com/skulldogged/gpui-ghostty-agent-terminal/issues/8).
The production-oriented Terminal Session seam is tracked by
[#11](https://github.com/skulldogged/gpui-ghostty-agent-terminal/issues/11).
The fixed-cell GPUI renderer is tracked by
[#12](https://github.com/skulldogged/gpui-ghostty-agent-terminal/issues/12).

## Terminal Session interface

GPUI interacts with one deep `TerminalSession` module. That module owns the
live shell, the platform process transport, Ghostty terminal state, input,
resize, snapshots, events, and cleanup. Neither PTY bytes nor raw Ghostty C
types escape to the view.

The current transport implementation uses `portable-pty` only for Unix PTYs on
macOS/Linux. Windows uses a project-owned ConPTY adapter over the supported
Windows API. Both remain internal adapters behind the same Terminal Session
interface.

## Pinned inputs

- Ghostty `4c725242b7dbe8c77c6e227ef1f9540c5ef17921`
- GPUI `fa00dccc42311f8dc71c533105488b0dbd518138`
- Zig `0.16.0`

## Commands

```bash
# Linux development shell
nix develop

# ABI/parser/render-state smoke test without GPUI
cargo run --no-default-features

# Native GPUI terminal development launch
cargo run -- --development

# Optimized development launch
cargo run --release -- --development
```

The renderer selects and verifies a native fixed-pitch family on each platform:
Menlo/SF Mono on macOS, Cascadia Mono/Consolas on Windows, and DejaVu Sans Mono
or Liberation Mono on Linux. `AGENT_TERMINAL_FONT` can request another installed
fixed-pitch family, and `AGENT_TERMINAL_FONT_SIZE` accepts sizes from 8 through
48 pixels.

The native workspace shell uses platform window materials when they are
available: Acrylic blur on supported Windows, native blur on macOS, and
compositor blur on Wayland. X11 and unknown platforms fall back to an opaque
window. Shell and default terminal surfaces use 65% opacity by default; set
`AGENT_TERMINAL_BACKGROUND_OPACITY` between `0.45` and `1.0` to override it. A
value of `1.0` forces opaque mode. Explicit terminal cell backgrounds and
cursor fills remain opaque regardless of this setting.

Closing the last window keeps the Application available from its native tray
item on macOS and Windows, or from a StatusNotifierItem host on Linux. Linux
desktops without a status-notifier host instead quit with the last window so the
application never becomes an invisible background process. The tray offers one
explicit **Quit** action, which ends the application and all Terminal Sessions.

The `--development` launch uses the same single-process architecture as a normal
launch. It additionally treats `Ctrl+C` in the launching terminal as an explicit
application quit, which makes local rebuild/test loops predictable.

## Verdict

The foundation is viable, with one important qualification: lock GPUI and
`libghostty-vt`, and keep each process transport behind the project-owned
Terminal Session interface.

| Platform | GPUI window | `libghostty-vt` link/render | Process transport | Result |
| --- | --- | --- | --- | --- |
| Linux x64 | Native Vulkan/X11 window | ANSI color, Unicode, resize, and cell snapshots passed | Native Unix PTY input/output passed | Green |
| macOS arm64 | Native Metal window | Same pinned library and GPUI renderer passed | Native Unix PTY input/output passed | Green |
| Windows x64 | Native GPUI window | MSVC build plus ANSI/Unicode/resize executable smoke passed | Project-owned ConPTY input/output, resize, and live shell round trip passed | Green |

The application owns terminals in-process while platform adapters keep Unix PTY
and ConPTY details behind the project-owned Terminal Session interface. T3 Code
uses the same broad terminal split (`libghostty-vt` semantics plus an
independently owned PTY layer), while mightty is the closest native Rust/GPUI
precedent.

## Remaining vertical-slice work

- The GPUI renderer uses measured fixed-cell geometry, constant-time cell
  lookup, one shaped draw per row, merged background runs, and live PTY
  resizing. A bounded background driver coalesces visual refreshes so terminal
  work never blocks GPUI's event thread.
- Keyboard input covers text, control keys, and arrows. Full IME support needs
  a GPUI `InputHandler`, not only key-down events.
- Agent TUIs remain ordinary PTY clients. Rich agent integration belongs in a
  capability-negotiated sidecar, outside the terminal parser and renderer.
- GPUI and the supported `libghostty-vt` C API are both pre-1.0/unstable, so the
  workspace must pin revisions and isolate each behind a deep adapter.
