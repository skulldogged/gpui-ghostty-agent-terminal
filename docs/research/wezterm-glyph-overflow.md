# WezTerm glyph-overflow research

Primary sources examined: WezTerm's official configuration documentation and
[`wezterm/wezterm` at `6fecdf5ae1a362b6f2e5b2c8df95123b5bd48967`](https://github.com/wezterm/wezterm/tree/6fecdf5ae1a362b6f2e5b2c8df95123b5bd48967).

## Conclusion

The recollection is substantially correct, with an important qualification:
**WezTerm deliberately allows square or wide symbol glyphs to overflow their
nominal horizontal cell allocation, but it does so conditionally by default.**
Its default policy is `WhenFollowedBySpace`; `Always` and `Never` are also
available. The official documentation explicitly describes this as deliberate
cell-width overflow and says the default changed from strict fitting to
`WhenFollowedBySpace` ([configuration reference](https://wezterm.org/config/lua/config/allow_square_glyphs_to_overflow_width.html),
[source documentation](https://github.com/wezterm/wezterm/blob/6fecdf5ae1a362b6f2e5b2c8df95123b5bd48967/docs/config/lua/config/allow_square_glyphs_to_overflow_width.md#L6-L25)).

That is not the same as giving every fallback glyph unrestricted overflow.
WezTerm retains the primary font's fixed grid, classifies a fallback face as
square/wide, considers whether the following shaped glyph is a space, and only
then decides whether an oversized bitmap should remain at scale `1.0` or be
scaled down to its allocated cell width.

## What the current implementation does

### The primary font owns the grid

WezTerm documents that the first resolved font defines the display-grid cell
metrics and that those metrics continue to place glyphs from fallback fonts
([cell-width reference](https://wezterm.org/config/lua/config/cell_width.html),
[source documentation](https://github.com/wezterm/wezterm/blob/6fecdf5ae1a362b6f2e5b2c8df95123b5bd48967/docs/config/lua/config/cell_width.md#L21-L27)).
Changing the cell width does not resize or center glyph ink; sufficiently tight
spacing can therefore make glyphs paint over one another
([source documentation](https://github.com/wezterm/wezterm/blob/6fecdf5ae1a362b6f2e5b2c8df95123b5bd48967/docs/config/lua/config/cell_width.md#L33-L44)).

This separation is important: letting a glyph overflow does not change terminal
column widths, cursor placement, or PTY dimensions.

### Width overflow is a collision-aware policy

The current enum defaults to `WhenFollowedBySpace`
([configuration source](https://github.com/wezterm/wezterm/blob/6fecdf5ae1a362b6f2e5b2c8df95123b5bd48967/config/src/font.rs#L652-L663)).
During shaping-to-raster conversion, WezTerm passes whether the next shaped
glyph is a space into the glyph cache
([render source](https://github.com/wezterm/wezterm/blob/6fecdf5ae1a362b6f2e5b2c8df95123b5bd48967/wezterm-gui/src/termwindow/render/mod.rs#L762-L774)).

The glyph cache then:

1. derives a square/wide classification from the selected face's cell aspect
   ratio;
2. combines that with `Never`, `Always`, or `WhenFollowedBySpace`;
3. computes the nominal width from the **base font** cell width and Unicode cell
   count, with a `0.25`-cell tolerance;
4. leaves a scalable fallback bitmap at `1.0` scale when overflow is allowed,
   otherwise scales an oversized bitmap down to fit that width.

The classification and policy are implemented here
([glyph-cache source](https://github.com/wezterm/wezterm/blob/6fecdf5ae1a362b6f2e5b2c8df95123b5bd48967/wezterm-gui/src/glyphcache.rs#L722-L747));
the distinct scalable-fallback branches are visible here
([glyph-cache source](https://github.com/wezterm/wezterm/blob/6fecdf5ae1a362b6f2e5b2c8df95123b5bd48967/wezterm-gui/src/glyphcache.rs#L756-L785)).

The source currently uses a face cell-aspect threshold of `0.7`, despite the
documentation's older prose saying an aspect ratio larger than `0.9`. The code
comment explains that the lower threshold intentionally includes symbols that
look square enough to benefit from overflow
([glyph-cache source](https://github.com/wezterm/wezterm/blob/6fecdf5ae1a362b6f2e5b2c8df95123b5bd48967/wezterm-gui/src/glyphcache.rs#L722-L727)).

### It does not generally clip ink to each terminal cell

The screen-line renderer builds its draw range from the complete rasterized
glyph texture. It subdivides that range for cursor and selection coloring, but
does not intersect it with the glyph's nominal cell span; the source even marks
per-pixel clipping as a TODO
([screen-line renderer](https://github.com/wezterm/wezterm/blob/6fecdf5ae1a362b6f2e5b2c8df95123b5bd48967/wezterm-gui/src/termwindow/render/screen_line.rs#L513-L596)).
This is direct source evidence that glyph ink which survived the glyph-cache
scaling decision is painted beyond cell boundaries rather than clipped back to
them.

The width-overflow setting itself is specifically horizontal. For scalable
fallback fonts, the fitting branch compares bitmap width against the allowed
pixel width and does not run an equivalent cell-height fitting calculation
([glyph-cache source](https://github.com/wezterm/wezterm/blob/6fecdf5ae1a362b6f2e5b2c8df95123b5bd48967/wezterm-gui/src/glyphcache.rs#L776-L785)).
The renderer positions the complete texture using baseline, bearing, and shaping
offsets, with no per-cell vertical clip in this path
([screen-line renderer](https://github.com/wezterm/wezterm/blob/6fecdf5ae1a362b6f2e5b2c8df95123b5bd48967/wezterm-gui/src/termwindow/render/screen_line.rs#L500-L527)).

### Fallback faces are not cap-height-normalized by default

WezTerm has an optional `use_cap_height_to_scale_fallback_fonts` setting, but it
defaults to `false`
([configuration reference](https://wezterm.org/config/lua/config/use_cap_height_to_scale_fallback_fonts.html),
[source documentation](https://github.com/wezterm/wezterm/blob/6fecdf5ae1a362b6f2e5b2c8df95123b5bd48967/docs/config/lua/config/use_cap_height_to_scale_fallback_fonts.md#L5-L13)).
When enabled for a secondary style, the font loader explicitly derives a scale
from the default and selected faces' cap heights
([font-loader source](https://github.com/wezterm/wezterm/blob/6fecdf5ae1a362b6f2e5b2c8df95123b5bd48967/wezterm-font/src/lib.rs#L861-L900)).
Its default being off is consistent with preserving each fallback face's design
size instead of automatically shrinking it to resemble the primary face.

## Implication for Agent Terminal

The present behavior described by the operator—fitting every fallback cluster
inside both the primary cell width and height—is stricter than WezTerm and can
make Nerd Font symbols noticeably too small.

A WezTerm-shaped policy for Agent Terminal would be:

- keep primary-font cell metrics authoritative for the grid, cursor, and PTY;
- rasterize scalable symbol fallbacks at the requested terminal point size;
- do not vertically fit or clip ordinary fallback ink to the primary cell;
- allow square/wide symbol ink to overflow horizontally when the following
  terminal cell is blank;
- fit horizontally when the following cell is occupied, preventing a symbol
  from obscuring adjacent text;
- retain grapheme-cluster transforms as a unit so combining sequences are not
  pulled apart.

This preserves the visual size users expect from Nerd Font symbols without
making terminal geometry depend on fallback-font metrics. A simpler initial
policy of always allowing bundled Nerd Font symbols to overflow would be closer
to WezTerm's `Always` option, but not to its collision-aware default.
