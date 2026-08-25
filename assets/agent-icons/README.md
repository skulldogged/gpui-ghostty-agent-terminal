# Agent icons

These normalized SVG marks share a 64 by 64 canvas and are rendered directly by GPUI at their
final display size. GPUI treats SVG elements as monochrome masks, so the shared badge component
provides the circular brand background and colors each mark white.

The logo paths are derived from the SVGL sources in `../svgl`; only the badge canvas, background,
and monochrome presentation are application-owned composition.
