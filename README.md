# Agent Terminal

Cross-platform graphical terminal-multiplexer foundation built with GPUI and
the supported `libghostty-vt` C interface.

The cross-platform proof is tracked by
[#8](https://github.com/skulldogged/gpui-ghostty-agent-terminal/issues/8).
The production-oriented Terminal Session seam is tracked by
[#11](https://github.com/skulldogged/gpui-ghostty-agent-terminal/issues/11).

## Terminal Session interface

GPUI interacts with one deep `TerminalSession` module. That module owns the
live shell, the platform process transport, Ghostty terminal state, input,
resize, snapshots, events, and cleanup. Neither PTY bytes nor raw Ghostty C
types escape to the view.

The current transport implementation uses Unix PTYs on macOS/Linux and ConPTY
on Windows through `portable-pty`. That dependency remains an internal adapter
so Windows can move to a dedicated implementation without changing callers.

## Pinned inputs

- Ghostty `4c725242b7dbe8c77c6e227ef1f9540c5ef17921`
- GPUI `0.2` (resolved exactly in `Cargo.lock` once generated)
- Zig `0.16.0`

## Commands

```bash
# Linux development shell
nix develop

# ABI/parser/render-state smoke test without GPUI
cargo run --no-default-features

# Native GPUI terminal
cargo run
```

## Verdict

The foundation is viable, with one important qualification: lock GPUI and
`libghostty-vt`, but keep the process transport behind an interface rather than
locking `portable-pty` as the production Windows implementation.

| Platform | GPUI window | `libghostty-vt` link/render | Process transport | Result |
| --- | --- | --- | --- | --- |
| Linux x64 | Native Vulkan/X11 window | ANSI color, Unicode, resize, and cell snapshots passed | Native Unix PTY input/output passed | Green |
| macOS arm64 | Native Metal window | Same pinned library and GPUI renderer passed | Native Unix PTY input/output passed | Green |
| Windows x64 | Native GPUI window | MSVC build plus ANSI/Unicode/resize executable smoke passed | ConPTY process starts, but this spike did not receive its byte stream in the scheduled desktop launch | Yellow |

The Windows result does not block the architecture. It reinforces the planned
seam: a resident core owns terminals and exposes a narrow byte-stream/resize/
lifecycle protocol; platform adapters own Unix PTY or ConPTY details. T3 Code
uses the same broad split (`libghostty-vt` semantics plus an independently
owned PTY layer), while mightty is the closest native Rust/GPUI precedent.

## Remaining vertical-slice work

- The GPUI renderer remains the correctness probe from #8. Fixed-cell font
  metrics, live grid geometry, and more efficient cell lookup are tracked by
  [#12](https://github.com/skulldogged/gpui-ghostty-agent-terminal/issues/12).
- Keyboard input covers text, control keys, and arrows. Full IME support needs
  a GPUI `InputHandler`, not only key-down events.
- Resize is proven at the Ghostty ABI and PTY interface levels, but the spike
  does not yet derive rows and columns from live window geometry.
- Agent TUIs remain ordinary PTY clients. Rich agent integration belongs in a
  capability-negotiated sidecar, outside the terminal parser and renderer.
- GPUI and the supported `libghostty-vt` C API are both pre-1.0/unstable, so the
  workspace must pin revisions and isolate each behind a deep adapter.
