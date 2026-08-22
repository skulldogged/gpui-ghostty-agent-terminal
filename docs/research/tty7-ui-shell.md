# tty7 UI-shell research

Research date: 2026-08-21

Primary source examined: [`l0ng-ai/tty7` at `74bb98697d8621b7b243d2d9aa19edbeab26c29e`](https://github.com/l0ng-ai/tty7/tree/74bb98697d8621b7b243d2d9aa19edbeab26c29e), its locked [`gpui-component` fork at `070d1a28cec5130bf7c4c7895683595d488155b8`](https://github.com/l0ng-ai/gpui-component/tree/070d1a28cec5130bf7c4c7895683595d488155b8), and its locked [GPUI fork at `d99a40a5f79a173465feec00363d480f75881a81`](https://github.com/l0ng-ai/zed/tree/d99a40a5f79a173465feec00363d480f75881a81). Comparisons to this project use [`gpui-ghostty-agent-terminal` at `62e3cfa97a16bce5604257ef2dbda0e700e141a2`](https://github.com/skulldogged/gpui-ghostty-agent-terminal/tree/62e3cfa97a16bce5604257ef2dbda0e700e141a2) and its pinned [upstream GPUI revision `fa00dccc42311f8dc71c533105488b0dbd518138`](https://github.com/zed-industries/zed/tree/fa00dccc42311f8dc71c533105488b0dbd518138).

## Executive recommendation

Adopt tty7's **native desktop-shell language**, not its information architecture or terminal implementation. For this product, the central hierarchy should be:

```text
┌─ Space controls ─────────┬─ Tabs for the selected Space ──────┬─ OS controls ─┐
│                          │                                     │               │
│  Search Spaces           ├─────────────────────────────────────┴───────────────┤
│                          │                                                     │
│  ● Space A               │                                                     │
│    path · agent summary  │              selected Tab's panes                   │
│                          │                                                     │
│  ○ Space B               │                                                     │
│    path · attention      │                                                     │
└──────────────────────────┴─────────────────────────────────────────────────────┘
```

The left rail should list **Spaces**, because a Space is the project's long-lived working context; the integrated top-center chrome should list the selected Space's **Tabs**, because Tabs are the immediate views within it. This matches the repository's authoritative model: a Space owns an ordered list of Tabs, a Tab owns a split tree, and sidebar state plus selected Tab are UI-client state rather than Resident Core ownership ([domain model, lines 7-25](https://github.com/skulldogged/gpui-ghostty-agent-terminal/blob/62e3cfa97a16bce5604257ef2dbda0e700e141a2/docs/architecture/domain-model.md#L7-L25)).

tty7 demonstrates that this can feel native in GPUI: it opens a transparent custom titlebar, requests client-side decorations on Linux, puts interactive content into a titlebar-height row, reserves platform-control space, and paints the whole workspace as one coherent themed surface ([tty7 window options, lines 664-720](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/src/ui/windows.rs#L664-L720), [titlebar assembly, lines 7061-7082](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/src/ui/app.rs#L7061-L7082), [workspace layout, lines 7191-7279](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/src/ui/app.rs#L7191-L7279)).

## What tty7 actually implements

### Integrated window chrome

tty7's `WindowOptions` sets a custom `TitlebarOptions`, requests `WindowDecorations::Client` so Wayland does not add a second titlebar, and sets `window_background` from the active material/theme resolver. It explicitly notes that macOS and Windows ignore the Linux decoration request while X11 can fall back to its window-manager frame when client decorations are unavailable ([window options, lines 703-720](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/src/ui/windows.rs#L703-L720)).

The titlebar helper tty7 consumes comes from its `gpui-component` fork. It sets `appears_transparent: true`, positions macOS traffic lights, draws minimize/maximize/close controls on Windows and Linux, and maps Windows control bounds to GPUI's native `WindowControlArea` hit-test roles ([titlebar options and control roles, lines 30-47 and 62-117](https://github.com/l0ng-ai/gpui-component/blob/070d1a28cec5130bf7c4c7895683595d488155b8/crates/ui/src/title_bar.rs#L30-L117), [control rendering, lines 151-226](https://github.com/l0ng-ai/gpui-component/blob/070d1a28cec5130bf7c4c7895683595d488155b8/crates/ui/src/title_bar.rs#L151-L226)). Its titlebar marks empty chrome as a drag region, starts a native window move after pointer motion, uses platform-appropriate double-click behavior, and exposes the Linux system window menu on right-click ([titlebar rendering, lines 252-332](https://github.com/l0ng-ai/gpui-component/blob/070d1a28cec5130bf7c4c7895683595d488155b8/crates/ui/src/title_bar.rs#L252-L332)).

tty7 further separates drag regions from interactive tiles: its reusable move gesture arms only on a left press, cancels when the pointer leaves or releases, then calls `start_window_move` on motion ([window-move gesture, lines 340-398](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/src/ui/app.rs#L340-L398)). Tab chips stop pointer propagation so clicking or dragging a tab does not move the window underneath it; double-clicking a chip delegates to titlebar zoom behavior ([tab-chip interaction, lines 1708-1722](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/src/ui/tab_strip.rs#L1708-L1722)).

The app's chrome is 40 px high. It reserves 80 px at the leading edge on macOS for traffic lights, no trailing control width there, and 102 px at the trailing edge on Windows/Linux for its three custom controls ([titlebar metrics, lines 218-291](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/src/ui/app.rs#L218-L291)). The horizontal tab strip uses 30 px rounded chips inside that 40 px band, keeps a flexible blank drag area in the center, and appends trailing app/panel chrome ([tab chips, lines 1660-1691](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/src/ui/tab_strip.rs#L1660-L1691), [strip assembly, lines 1833-1906](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/src/ui/tab_strip.rs#L1833-L1906)).

### Sidebar hierarchy and interaction

tty7's present sidebar lists Tabs, which is the piece this product should **not** copy. Its useful shell mechanics are independent of that choice: the rail defaults to 220 px, has a 180 px minimum, clamps against space required by other panels, and persists drag-resized width ([default width, lines 1158-1164](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/crates/tty7-core/src/core/config.rs#L1158-L1164), [width constraints, lines 27-58 and 98-132](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/src/ui/tab_sidebar.rs#L27-L132), [resize handle and persistence, lines 1047-1133](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/src/ui/tab_sidebar.rs#L1047-L1133)).

The rail has a titlebar-height control row, a workspace selector, a 44 px search row, a scrollable grouped list, and a one-pixel divider. The top control band itself is a window drag region, while buttons are occluding interactive children ([sidebar header/search/layout, lines 961-1045 and 1135-1170](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/src/ui/tab_sidebar.rs#L961-L1045)).

Rows use an avatar/status mark, a primary title, optional branch/diff or cwd metadata, a rounded selected surface, hover fill, and controls revealed on hover. Text is measured with the actual font and width budget before path-aware elision rather than relying only on generic CSS truncation ([row measurement, lines 278-351](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/src/ui/tab_sidebar.rs#L278-L351), [row composition, lines 611-785](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/src/ui/tab_sidebar.rs#L611-L785)). Group headings are 11 px semibold uppercase labels with counts, and rows can be reordered within groups while the groups themselves can also be reordered ([group composition, lines 848-944](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/src/ui/tab_sidebar.rs#L848-L944)).

### Tokens, typography, and icons

tty7 does not style each widget from unrelated literals. A theme supplies background, foreground, accent, caret, selection, optional opacity/blur/image, and an ANSI palette; from those it derives neutral surfaces, semantic colors, and consistent hover/selected/pressed/cursor states with contrast targets ([theme model and state ladder, lines 9-113](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/src/ui/presets.rs#L9-L113), [derived neutrals and surfaces, lines 146-303](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/src/ui/presets.rs#L146-L303)). This is why its sidebar, titlebar, terminal, overlays, and menus read as one system rather than separate dark rectangles.

Interface scaling and terminal metrics are deliberately separate. tty7 sets a root `rem` from `ui_font_size` for chrome text and spacing, while its terminal grid remains sized from a separate absolute terminal font size; the defaults are a 16 px UI root and 15 px Hack terminal font ([UI/terminal configuration, lines 113-142 and 563-583](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/crates/tty7-core/src/core/config.rs#L113-L142), [UI size constants, lines 1140-1159](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/crates/tty7-core/src/core/config.rs#L1140-L1159), [root scaling, lines 6962-6971](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/src/ui/app.rs#L6962-L6971)).

Its icons are SVG assets served through a GPUI `AssetSource`: tty7 overrides product-specific glyphs and delegates the rest to `gpui-component-assets`, including dedicated agent marks for Claude, Codex, Gemini, OpenCode, Pi, and others ([asset source and icon mapping, lines 1-67](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/src/ui/assets.rs#L1-L67)).

### Blur, transparency, Mica, and platform fallbacks

tty7 resolves `WindowBackdrop::{Auto, Blur, Mica, MicaAlt, Acrylic, Off}` into GPUI window appearances by Windows build number. Mica/Mica Alt/native Acrylic require Windows 11 22H2 (build 22621); requests fall back to classic blurred Acrylic on 1809+ (17763), then plain transparency on older builds. The settings list hides distinct material choices on systems where they would render identically ([resolver and fallback table, lines 282-370](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/src/ui/theme.rs#L282-L370)).

That complete Acrylic path is **not stock GPUI** in tty7: Cargo.lock resolves GPUI to tty7's Zed fork, and the manifest documents its `[patch]` replacement ([GPUI dependency and patch rationale, lines 276-295 and 369-402](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/Cargo.toml#L276-L295)). The fork adds `AcrylicBackdrop` and makes runtime transitions clear the previous composition attribute and refresh the non-client frame ([fork background enum, lines 1724-1750](https://github.com/l0ng-ai/zed/blob/d99a40a5f79a173465feec00363d480f75881a81/crates/gpui/src/platform.rs#L1724-L1750), [Windows application, lines 831-870](https://github.com/l0ng-ai/zed/blob/d99a40a5f79a173465feec00363d480f75881a81/crates/gpui_windows/src/window.rs#L831-L870)).

This project does not need that fork to start. Its pinned stock GPUI already exposes `Opaque`, `Transparent`, `Blurred`, `MicaBackdrop`, and `MicaAltBackdrop`; its Windows backend maps those to composition/DWM calls ([stock background enum, lines 2099-2122](https://github.com/zed-industries/zed/blob/fa00dccc42311f8dc71c533105488b0dbd518138/crates/gpui/src/platform.rs#L2099-L2122), [stock Windows implementation, lines 880-904](https://github.com/zed-industries/zed/blob/fa00dccc42311f8dc71c533105488b0dbd518138/crates/gpui_windows/src/window.rs#L880-L904)). The same revision implements `Blurred` on macOS with a view behind the content and on KDE Wayland through the compositor blur protocol; X11 only updates alpha transparency, so blur quality is necessarily compositor/platform-dependent ([macOS blurred view, lines 1651-1687](https://github.com/zed-industries/zed/blob/fa00dccc42311f8dc71c533105488b0dbd518138/crates/gpui_macos/src/window.rs#L1651-L1687), [Wayland blur protocol, lines 1971-2012](https://github.com/zed-industries/zed/blob/fa00dccc42311f8dc71c533105488b0dbd518138/crates/gpui_linux/src/linux/wayland/window.rs#L1971-L2012), [X11 transparency update, lines 1584-1589](https://github.com/zed-industries/zed/blob/fa00dccc42311f8dc71c533105488b0dbd518138/crates/gpui_linux/src/linux/x11/window.rs#L1584-L1589)).

tty7 only makes a material visible when the app surface also has alpha. Its material default is 0.82 opacity, large side surfaces use a low-alpha overlay so they do not cover the backdrop twice, and full-window settings/doc overlays deliberately return to opaque surfaces for readability ([material opacity and surface composition, lines 452-489](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/src/ui/theme.rs#L452-L489), [workspace versus overlay fills, lines 187-239](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/src/ui/theme.rs#L187-L239)).

## The terminal-renderer prerequisite

Before enabling translucency, this project must distinguish the terminal's **default background** from an **explicit ANSI cell background** all the way through painting.

The data is already available: `TerminalCell` stores `has_explicit_bg` from libghostty ([terminal cell, lines 1507-1528](https://github.com/skulldogged/gpui-ghostty-agent-terminal/blob/62e3cfa97a16bce5604257ef2dbda0e700e141a2/src/resident_core.rs#L1507-L1528)). The current frame conversion discards that distinction, creates an RGB `BackgroundRun` for every visible cell, and coalesces equal runs ([frame conversion, lines 56-135 and 166-175](https://github.com/skulldogged/gpui-ghostty-agent-terminal/blob/62e3cfa97a16bce5604257ef2dbda0e700e141a2/src/terminal_frame.rs#L56-L135)). The GPUI pane then paints every run opaque and also sets the whole pane to `default_bg`, so any Mica/blur layer behind it would be hidden ([terminal painting, lines 856-928 and 966-981](https://github.com/skulldogged/gpui-ghostty-agent-terminal/blob/62e3cfa97a16bce5604257ef2dbda0e700e141a2/src/gui.rs#L856-L928)).

Required behavior:

- The terminal's default background is the only terminal layer whose alpha follows the window/theme opacity.
- Cells with `has_explicit_bg`, inverse-video backgrounds, selections, and cursor fills remain visually opaque unless terminal semantics explicitly say otherwise.
- Explicit backgrounds still coalesce into runs; default cells may be omitted from the background-run list and reveal one pane-level translucent default surface.
- A regression fixture must mix default cells, SGR background cells, reverse video, wide-cell tails, and a cursor, and verify both the run classification and the rendered/material result.

tty7 follows the same semantic rule: it records whether a cell background is default, sets `draw_bg` only for inverse or non-default backgrounds, and skips default cells during background painting so the workspace material remains visible through them ([cell classification, lines 157-193](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/src/terminal/element.rs#L157-L193), [background painting, lines 440-461](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/src/terminal/element.rs#L440-L461)).

## Reusable concepts versus project-specific code

| Area | Reuse directly or conceptually | Do not transplant blindly |
| --- | --- | --- |
| Window shell | Transparent titlebar, Linux client decorations, native control hit regions, explicit drag gaps, macOS traffic-light reserve | tty7's full `gpui-component` titlebar; this project can implement a smaller internal chrome module on its existing GPUI pin |
| Navigation | Resizable 220 px rail, search, rounded selected rows, primary + muted metadata, hover actions | tty7's Tab-in-sidebar hierarchy; this product needs Spaces in the rail and Tabs in the titlebar |
| Theme | One token source for surfaces, interaction states, semantic colors, borders, radii, spacing, and typography | tty7's theme file format and broad settings UI before this product needs them |
| Icons | GPUI `AssetSource`, coherent SVG family, agent-specific marks | Copying tty7's product artwork or binding the UI to its forked asset crate |
| Materials | GPUI `WindowBackgroundAppearance`, alpha-aware root surfaces, platform capability/fallback policy | Assuming Acrylic is in this project's GPUI revision, or promising blur on Linux compositors that do not implement it |
| Terminal | Default-background transparency with explicit ANSI/cursor fills preserved | tty7's Alacritty grid/renderer; libghostty-vt remains this project's terminal-state authority |

### Reuse and attribution

tty7 is Apache-2.0 licensed. Its license permits redistribution and derivative works, but copied or modified source must ship with the license, modified files must be marked, relevant copyright/attribution notices must be retained, and any upstream `NOTICE` content must be carried forward ([tty7 license, redistribution conditions, lines 89-128](https://github.com/l0ng-ai/tty7/blob/74bb98697d8621b7b243d2d9aa19edbeab26c29e/LICENSE#L89-L128)). Therefore a close **design baseline implemented independently** is straightforward; if implementation work copies tty7 Rust, SVGs, or other assets, record the source commit and file in third-party notices and preserve the applicable Apache header/attribution. Do not copy tty7's logo or product identity; use an original icon family and branding. This is an engineering compliance recommendation, not legal advice.

## Proposed UI-shell specification

These are recommendations inferred from the implementation above, not claims about tty7.

### Information architecture

- **Left rail: Spaces only.** Each row should show the Space name, abbreviated initial/current directory, a compact count of Tabs, and one agent-attention summary. Agent status decorates the Space summary but does not replace its identity.
- **Top center: Tabs for the selected Space.** Use compact rounded chips with optional terminal/agent mark, title, attention dot, and hover close button. Keep `+` beside the chips and reserve the flexible remainder for window dragging.
- **Pane surface: terminal first.** Pane borders should normally be hairlines; focus can use a restrained accent edge rather than the current bright full rectangle. Split controls belong on hover or in a pane menu so the terminal remains visually dominant.
- **Search/switching:** the rail search filters Spaces. A later command palette can search Spaces, Tabs, panes, and actions without forcing all those levels into the sidebar.

### Starting metrics and visual tokens

- 40 px integrated titlebar; 30 px Tab chips; 32 px chrome hit targets with approximately 13 px glyphs.
- 220 px default Space rail, 180 px minimum, user-resizable, with a one-pixel surface divider.
- Four-pixel spacing base; 8/12 px row insets; approximately 8 px radii for selected rows and Tab chips.
- Separate proportional UI typography from the verified monospace terminal font. Start with a 16 px root scale, approximately 14 px primary labels, 12 px metadata, and 11 px section labels.
- Derive window, sidebar, hover, selected, pressed, border, muted text, accent, danger, warning, and success colors from one theme object. Avoid adding more raw RGB constants to `gui.rs`.
- Ship one coherent SVG outline family and treat agent logos as semantic/status assets, not general-purpose navigation icons.

### Platform behavior

- **Windows:** make the titlebar transparent, reserve the rightmost native-control band, mark its three buttons as `Min`/`Max`/`Close`, and mark only blank chrome as `Drag`. Start with stock GPUI Mica/Mica Alt plus a build-aware fallback; evaluate tty7's small Acrylic GPUI patch separately rather than taking its entire fork.
- **macOS:** reserve the traffic-light area at the far left of the sidebar header, set the traffic-light position explicitly, omit custom OS-control buttons, and preserve native titlebar double-click behavior.
- **Linux:** request client-side decorations and render app-owned window controls. Use `Blurred` where the compositor supports it, but make opaque or plain-translucent rendering a first-class tested fallback; right-clicking a drag region should expose the system window menu.
- **All platforms:** interactive Space rows, Tab chips, add/collapse buttons, search, and menus must occlude the drag region. Empty titlebar space remains draggable, and double-click behavior follows the platform.

The relevant primitives already exist in this project's GPUI pin: `TitlebarOptions::appears_transparent` is defined for macOS/Windows, `WindowDecorations` selects Linux client/server decorations, and `WindowControlArea` provides Drag/Close/Max/Min hit roles ([titlebar options, lines 2026-2038](https://github.com/zed-industries/zed/blob/fa00dccc42311f8dc71c533105488b0dbd518138/crates/gpui/src/platform.rs#L2026-L2038), [Linux decoration option, lines 1878-1895](https://github.com/zed-industries/zed/blob/fa00dccc42311f8dc71c533105488b0dbd518138/crates/gpui/src/platform.rs#L1878-L1895), [control roles, lines 721-732](https://github.com/zed-industries/zed/blob/fa00dccc42311f8dc71c533105488b0dbd518138/crates/gpui/src/window.rs#L721-L732)).

## Recommended focused stack

1. **Preserve terminal background semantics.** Carry `has_explicit_bg` into `TerminalFrame`, omit default-cell runs, make the pane default surface alpha-aware, and add mixed-background renderer tests. This can merge independently and is required before material evidence is meaningful.
2. **Build the native UI shell.** Introduce internal theme/metric/icon modules, transparent custom titlebar and platform controls, a resizable Space rail, and Tabs in the integrated top-center chrome. Keep the first pass opaque by default so chrome correctness can be reviewed separately from compositor behavior.
3. **Enable materials and prove fallbacks.** Add Windows Mica/Mica Alt with build-aware fallback, macOS/Wayland blur, opaque/plain-translucent Linux fallback, an opacity preference, and graphical evidence on Windows, macOS, Wayland, and X11.

The milestone is complete when the same hierarchy works without native titlebar duplication on all three operating systems, every interactive titlebar child is click/drag safe, terminal explicit backgrounds remain correct over a material, and unsupported blur paths degrade to a deliberate readable surface rather than a broken transparent window.
