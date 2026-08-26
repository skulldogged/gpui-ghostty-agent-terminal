# Bundled terminal fonts

`SymbolsNerdFontMono-Regular.ttf` comes from the Nerd Fonts 3.5.0
`NerdFontsSymbolsOnly` release. The bundled copy adds one compatibility cmap
entry mapping U+006D (`m`) to the font's blank `.null` glyph. GPUI currently
rejects fallback-only faces on Linux unless they map `m`; normal terminal text
continues to come from the selected primary font, so this entry is never used
for rendering.

- Upstream: https://github.com/ryanoasis/nerd-fonts/tree/v3.5.0/patched-fonts/NerdFontsSymbolsOnly
- Upstream SHA-256: `2dc316f2505a0cbfbcf6060a1b4ba85b0a2974189e30c0037cdedc436a25a4ff`
- Bundled SHA-256: `0341c975459317fea711fd2a1eb4adf08b740111b9f1900ac60adbb48fd34ba8`
- License: [Nerd Fonts licensing](./NERD-FONTS-LICENSE.txt)
