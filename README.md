# GPUI + libghostty-vt foundation spike

> THROWAWAY PROTOTYPE for the architecture decision tracked by
> [Prove the GPUI and libghostty-vt foundation on all three platforms](https://github.com/skulldogged/gpui-ghostty-agent-terminal/issues/8).

This branch answers one question: can a pinned Rust application consume the
supported `libghostty-vt` C API, render its state through GPUI, and attach the
same terminal model to Unix PTY and Windows ConPTY transports?

It is deliberately not the production application. The useful output is the
compatibility verdict and the seams exposed by the experiment.

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

# Native GPUI + PTY prototype
cargo run
```
