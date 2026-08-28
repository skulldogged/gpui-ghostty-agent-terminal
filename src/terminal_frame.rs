use crate::{
    TerminalCursorShape, TerminalSnapshot, terminal_grid::cell_offset,
    terminal_selection::SelectionRow,
};
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TerminalFrame {
    pub rows: Vec<FrameRow>,
    // Default-background cells reveal the pane surface. These runs contain only
    // explicit terminal backgrounds and block cursor fills, which must stay opaque
    // when the pane surface becomes translucent.
    pub opaque_backgrounds: Vec<BackgroundRun>,
    pub cursor_cell: Option<(u16, u16)>,
    pub cursor_overlay: Option<CursorOverlay>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FrameRow {
    pub text: String,
    pub runs: Vec<ForegroundRun>,
    pub glyph_cells: Vec<GlyphCell>,
}

impl FrameRow {
    pub(crate) fn glyph_cell_index(&self, byte_index: usize) -> Option<usize> {
        self.glyph_cells
            .binary_search_by(|cell| {
                if cell.byte_range.end <= byte_index {
                    std::cmp::Ordering::Less
                } else if cell.byte_range.start > byte_index {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GlyphCell {
    // Maps every shaped glyph in this UTF-8 range back to its Ghostty cell anchor.
    pub x: u16,
    pub width: u8,
    pub byte_range: Range<usize>,
    pub color: [u8; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ForegroundRun {
    pub len: usize,
    pub color: [u8; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BackgroundRun {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub color: [u8; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CursorOverlay {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub shape: TerminalCursorShape,
    pub color: [u8; 3],
}

impl TerminalFrame {
    #[cfg(test)]
    pub(crate) fn from_snapshot(snapshot: &TerminalSnapshot) -> Self {
        Self::from_snapshot_with_cursor(snapshot, true)
    }

    pub(crate) fn from_snapshot_with_cursor(
        snapshot: &TerminalSnapshot,
        cursor_visible: bool,
    ) -> Self {
        let cursor = snapshot.cursor.map(|(x, y)| {
            if snapshot.cursor_wide_tail {
                (x.saturating_sub(1), y)
            } else {
                (x, y)
            }
        });
        let mut cells = vec![None; usize::from(snapshot.cols) * usize::from(snapshot.rows)];
        for cell in &snapshot.cells {
            if let Some(offset) = offset(snapshot.cols, snapshot.rows, cell.x, cell.y) {
                cells[offset] = Some(cell);
            }
        }

        let mut rows = Vec::with_capacity(usize::from(snapshot.rows));
        let mut opaque_backgrounds = Vec::new();
        let mut cursor_overlay = None;
        for y in 0..snapshot.rows {
            let mut text = String::new();
            let mut runs = Vec::<ForegroundRun>::new();
            let mut glyph_cells = Vec::with_capacity(usize::from(snapshot.cols));
            for x in 0..snapshot.cols {
                let cell = cells[offset(snapshot.cols, snapshot.rows, x, y)
                    .expect("in-bounds terminal coordinate")];
                let width = cell.map(|cell| cell.width).unwrap_or(1);
                if width == 0 {
                    continue;
                }

                let mut foreground = cell.map(|cell| cell.fg).unwrap_or(snapshot.default_fg);
                let cursor_here = cursor_visible && cursor == Some((x, y));
                let mut background = cell.map(|cell| cell.bg).unwrap_or(snapshot.default_bg);
                let block_cursor_here =
                    cursor_here && snapshot.cursor_shape == TerminalCursorShape::Block;
                if block_cursor_here {
                    background = foreground;
                    foreground = snapshot.default_bg;
                } else if cursor_here {
                    cursor_overlay = Some(CursorOverlay {
                        x,
                        y,
                        width: u16::from(width),
                        shape: snapshot.cursor_shape,
                        color: foreground,
                    });
                }
                if block_cursor_here || cell.is_some_and(|cell| cell.has_explicit_bg) {
                    push_background(
                        &mut opaque_backgrounds,
                        BackgroundRun {
                            x,
                            y,
                            width: u16::from(width),
                            color: background,
                        },
                    );
                }

                match cell.map(|cell| cell.text.as_str()) {
                    Some(value) if !value.is_empty() => {
                        push_text_cell(&mut text, &mut glyph_cells, x, width, value, foreground);
                        push_foreground(&mut runs, value.len(), foreground);
                    }
                    _ => {
                        for cell_offset in 0..u16::from(width) {
                            push_text_cell(
                                &mut text,
                                &mut glyph_cells,
                                x.saturating_add(cell_offset),
                                1,
                                " ",
                                foreground,
                            );
                            push_foreground(&mut runs, 1, foreground);
                        }
                    }
                }
            }
            rows.push(FrameRow {
                text,
                runs,
                glyph_cells,
            });
        }

        Self {
            rows,
            opaque_backgrounds,
            cursor_cell: cursor_visible.then_some(cursor).flatten(),
            cursor_overlay,
        }
    }

    pub(crate) fn dimmed_toward(mut self, background: [u8; 3], contrast: f32) -> Self {
        for row in &mut self.rows {
            for run in &mut row.runs {
                run.color = blend_toward(run.color, background, contrast);
            }
            for cell in &mut row.glyph_cells {
                cell.color = blend_toward(cell.color, background, contrast);
            }
        }
        for run in &mut self.opaque_backgrounds {
            run.color = blend_toward(run.color, background, contrast);
        }
        if let Some(cursor) = &mut self.cursor_overlay {
            cursor.color = blend_toward(cursor.color, background, contrast);
        }
        self
    }

    pub(crate) fn apply_selection_foreground(
        &mut self,
        selection_rows: &[SelectionRow],
        foreground: [u8; 3],
    ) {
        for selection in selection_rows {
            let Some(row) = self.rows.get_mut(usize::from(selection.y)) else {
                continue;
            };
            for cell in &mut row.glyph_cells {
                let end_x = cell
                    .x
                    .saturating_add(u16::from(cell.width).saturating_sub(1));
                if cell.x <= selection.end_x && end_x >= selection.start_x {
                    cell.color = foreground;
                }
            }

            row.runs.clear();
            for cell in &row.glyph_cells {
                push_foreground(&mut row.runs, cell.byte_range.len(), cell.color);
            }
        }
    }
}

fn blend_toward(color: [u8; 3], background: [u8; 3], contrast: f32) -> [u8; 3] {
    let contrast = contrast.clamp(0., 1.);
    std::array::from_fn(|index| {
        let background = f32::from(background[index]);
        (background + (f32::from(color[index]) - background) * contrast).round() as u8
    })
}

fn push_text_cell(
    text: &mut String,
    glyph_cells: &mut Vec<GlyphCell>,
    x: u16,
    width: u8,
    value: &str,
    color: [u8; 3],
) {
    let start = text.len();
    text.push_str(value);
    glyph_cells.push(GlyphCell {
        x,
        width,
        byte_range: start..text.len(),
        color,
    });
}

fn push_foreground(runs: &mut Vec<ForegroundRun>, len: usize, color: [u8; 3]) {
    if let Some(run) = runs.last_mut().filter(|run| run.color == color) {
        run.len += len;
    } else {
        runs.push(ForegroundRun { len, color });
    }
}

fn offset(cols: u16, rows: u16, x: u16, y: u16) -> Option<usize> {
    (y < rows).then(|| cell_offset(cols, x, y)).flatten()
}

fn push_background(backgrounds: &mut Vec<BackgroundRun>, next: BackgroundRun) {
    if let Some(previous) = backgrounds.last_mut()
        && previous.y == next.y
        && previous.color == next.color
        && previous.x.saturating_add(previous.width) == next.x
    {
        previous.width = previous.width.saturating_add(next.width);
    } else {
        backgrounds.push(next);
    }
}

#[cfg(test)]
mod tests {
    use super::{BackgroundRun, CursorOverlay, GlyphCell, TerminalFrame};
    use crate::{TerminalCell, TerminalCursorShape, TerminalLifecycle, TerminalSnapshot};

    #[test]
    fn wide_tails_are_not_rendered_as_independent_glyphs() {
        let snapshot = snapshot(
            None,
            vec![
                cell(0, 0, 2, "界", [0xaa, 0xbb, 0xcc]),
                cell(1, 0, 0, "", [0xaa, 0xbb, 0xcc]),
                cell(2, 0, 1, "x", [0xaa, 0xbb, 0xcc]),
            ],
        );

        let frame = TerminalFrame::from_snapshot(&snapshot);

        assert_eq!(frame.rows[0].text, "界x ");
        assert_eq!(
            frame.rows[0].glyph_cells,
            vec![
                GlyphCell {
                    x: 0,
                    width: 2,
                    byte_range: 0..3,
                    color: [0xaa, 0xbb, 0xcc],
                },
                GlyphCell {
                    x: 2,
                    width: 1,
                    byte_range: 3..4,
                    color: [0xaa, 0xbb, 0xcc],
                },
                GlyphCell {
                    x: 3,
                    width: 1,
                    byte_range: 4..5,
                    color: [0xdd; 3],
                },
            ]
        );
        assert_eq!(
            frame.rows[0].runs.iter().map(|run| run.len).sum::<usize>(),
            5
        );
        assert!(frame.opaque_backgrounds.is_empty());
    }

    #[test]
    fn only_explicit_and_cursor_backgrounds_become_opaque_runs() {
        let mut explicit = cell(1, 0, 1, "b", [0xbb; 3]);
        explicit.bg = [0x22; 3];
        explicit.has_explicit_bg = true;
        let mut explicit_default_color = cell(2, 0, 1, "c", [0xcc; 3]);
        explicit_default_color.has_explicit_bg = true;
        let frame = TerminalFrame::from_snapshot(&snapshot(
            Some((3, 0)),
            vec![
                cell(0, 0, 1, "a", [0xaa; 3]),
                explicit,
                explicit_default_color,
                cell(3, 0, 1, "d", [0xdd; 3]),
            ],
        ));

        assert_eq!(
            frame.opaque_backgrounds,
            vec![
                BackgroundRun {
                    x: 1,
                    y: 0,
                    width: 1,
                    color: [0x22; 3],
                },
                BackgroundRun {
                    x: 2,
                    y: 0,
                    width: 1,
                    color: [0x11; 3],
                },
                BackgroundRun {
                    x: 3,
                    y: 0,
                    width: 1,
                    color: [0xdd; 3],
                },
            ]
        );
    }

    #[test]
    fn a_block_cursor_on_a_wide_tail_inverts_the_leading_wide_cell() {
        let snapshot = snapshot_with_cursor_state(
            Some((1, 0)),
            TerminalCursorShape::Block,
            true,
            vec![
                cell(0, 0, 2, "界", [0xaa, 0xbb, 0xcc]),
                cell(1, 0, 0, "", [0xaa, 0xbb, 0xcc]),
            ],
        );

        let frame = TerminalFrame::from_snapshot(&snapshot);

        assert_eq!(frame.cursor_cell, Some((0, 0)));
        assert!(frame.cursor_overlay.is_none());
        assert_eq!(frame.opaque_backgrounds[0].x, 0);
        assert_eq!(frame.opaque_backgrounds[0].width, 2);
    }

    #[test]
    fn a_thin_cursor_on_a_wide_tail_spans_from_the_leading_cell() {
        let frame = TerminalFrame::from_snapshot(&snapshot_with_cursor_state(
            Some((1, 0)),
            TerminalCursorShape::Underline,
            true,
            vec![
                cell(0, 0, 2, "界", [0xaa, 0xbb, 0xcc]),
                cell(1, 0, 0, "", [0xaa, 0xbb, 0xcc]),
            ],
        ));

        let cursor = frame.cursor_overlay.expect("wide cursor overlay");
        assert_eq!(cursor.x, 0);
        assert_eq!(cursor.width, 2);
    }

    #[test]
    fn non_block_cursors_overlay_without_inverting_cell_content() {
        for shape in [
            TerminalCursorShape::Bar,
            TerminalCursorShape::Underline,
            TerminalCursorShape::BlockHollow,
        ] {
            let frame = TerminalFrame::from_snapshot(&snapshot_with_cursor_shape(
                Some((0, 0)),
                shape,
                vec![cell(0, 0, 1, "x", [0xaa, 0xbb, 0xcc])],
            ));

            assert!(frame.opaque_backgrounds.is_empty());
            assert_eq!(frame.rows[0].glyph_cells[0].color, [0xaa, 0xbb, 0xcc]);
            assert_eq!(
                frame.cursor_overlay,
                Some(CursorOverlay {
                    x: 0,
                    y: 0,
                    width: 1,
                    shape,
                    color: [0xaa, 0xbb, 0xcc],
                })
            );
        }
    }

    #[test]
    fn an_underline_cursor_spans_the_full_wide_glyph() {
        let frame = TerminalFrame::from_snapshot(&snapshot_with_cursor_shape(
            Some((0, 0)),
            TerminalCursorShape::Underline,
            vec![
                cell(0, 0, 2, "界", [0xaa, 0xbb, 0xcc]),
                cell(1, 0, 0, "", [0xaa, 0xbb, 0xcc]),
            ],
        ));

        assert_eq!(frame.cursor_overlay.expect("cursor overlay").width, 2);
    }

    #[test]
    fn an_inactive_pane_frame_omits_its_cursor() {
        let snapshot = snapshot(
            Some((1, 0)),
            vec![
                cell(0, 0, 2, "界", [0xaa, 0xbb, 0xcc]),
                cell(1, 0, 0, "", [0xaa, 0xbb, 0xcc]),
            ],
        );

        let frame = TerminalFrame::from_snapshot_with_cursor(&snapshot, false);

        assert!(frame.cursor_overlay.is_none());
        assert!(frame.opaque_backgrounds.is_empty());
    }

    #[test]
    fn an_inactive_pane_frame_reduces_color_contrast() {
        let frame = TerminalFrame::from_snapshot_with_cursor(
            &snapshot(None, vec![cell(0, 0, 1, "x", [0xaa, 0xbb, 0xcc])]),
            false,
        )
        .dimmed_toward([0; 3], 0.5);

        assert_eq!(frame.rows[0].runs[0].color, [0x55, 0x5e, 0x66]);
        assert_eq!(frame.rows[0].glyph_cells[0].color, [0x55, 0x5e, 0x66]);
    }

    #[test]
    fn every_glyph_in_a_combining_cluster_maps_to_one_terminal_cell() {
        let frame = TerminalFrame::from_snapshot(&snapshot(
            None,
            vec![
                cell(0, 0, 1, "e\u{301}", [0xaa, 0xbb, 0xcc]),
                cell(1, 0, 1, "x", [0xaa, 0xbb, 0xcc]),
            ],
        ));

        assert_eq!(frame.rows[0].glyph_cell_index(0), Some(0));
        assert_eq!(frame.rows[0].glyph_cell_index(1), Some(0));
        assert_eq!(frame.rows[0].glyph_cell_index(2), Some(0));
        assert_eq!(frame.rows[0].glyph_cell_index(3), Some(1));
    }

    fn snapshot(cursor: Option<(u16, u16)>, cells: Vec<TerminalCell>) -> TerminalSnapshot {
        snapshot_with_cursor_shape(cursor, TerminalCursorShape::Block, cells)
    }

    fn snapshot_with_cursor_shape(
        cursor: Option<(u16, u16)>,
        cursor_shape: TerminalCursorShape,
        cells: Vec<TerminalCell>,
    ) -> TerminalSnapshot {
        snapshot_with_cursor_state(cursor, cursor_shape, false, cells)
    }

    fn snapshot_with_cursor_state(
        cursor: Option<(u16, u16)>,
        cursor_shape: TerminalCursorShape,
        cursor_wide_tail: bool,
        cells: Vec<TerminalCell>,
    ) -> TerminalSnapshot {
        TerminalSnapshot {
            revision: 1,
            lifecycle: TerminalLifecycle::Running,
            active_work: false,
            title: None,
            agent: None,
            cols: 4,
            rows: 1,
            cursor,
            cursor_shape,
            cursor_blinking: false,
            cursor_wide_tail,
            default_fg: [0xdd; 3],
            default_bg: [0x11; 3],
            selection_text: None,
            cells,
        }
    }

    fn cell(x: u16, y: u16, width: u8, text: &str, fg: [u8; 3]) -> TerminalCell {
        TerminalCell {
            x,
            y,
            width,
            text: text.into(),
            fg,
            bg: [0x11; 3],
            has_explicit_bg: false,
            soft_wrapped: false,
            selected: false,
        }
    }
}
