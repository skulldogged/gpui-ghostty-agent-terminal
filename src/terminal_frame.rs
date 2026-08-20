use crate::{TerminalSnapshot, terminal_grid::cell_offset};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TerminalFrame {
    pub rows: Vec<FrameRow>,
    pub backgrounds: Vec<BackgroundRun>,
    pub cursor_overlay: Option<BackgroundRun>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FrameRow {
    pub text: String,
    pub runs: Vec<ForegroundRun>,
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

impl TerminalFrame {
    pub(crate) fn from_snapshot(snapshot: &TerminalSnapshot) -> Self {
        let mut cells = vec![None; usize::from(snapshot.cols) * usize::from(snapshot.rows)];
        for cell in &snapshot.cells {
            if let Some(offset) = offset(snapshot.cols, snapshot.rows, cell.x, cell.y) {
                cells[offset] = Some(cell);
            }
        }

        let mut rows = Vec::with_capacity(usize::from(snapshot.rows));
        let mut backgrounds = Vec::new();
        let mut cursor_overlay = None;
        for y in 0..snapshot.rows {
            let mut text = String::new();
            let mut runs = Vec::<ForegroundRun>::new();
            for x in 0..snapshot.cols {
                let cell = cells[offset(snapshot.cols, snapshot.rows, x, y)
                    .expect("in-bounds terminal coordinate")];
                let width = cell.map(|cell| cell.width).unwrap_or(1);
                if width == 0 {
                    if snapshot.cursor == Some((x, y)) {
                        cursor_overlay = Some(BackgroundRun {
                            x,
                            y,
                            width: 1,
                            color: cell.map(|cell| cell.fg).unwrap_or(snapshot.default_fg),
                        });
                    }
                    continue;
                }

                let mut foreground = cell.map(|cell| cell.fg).unwrap_or(snapshot.default_fg);
                let mut background = cell.map(|cell| cell.bg).unwrap_or(snapshot.default_bg);
                if snapshot.cursor == Some((x, y)) {
                    background = foreground;
                    foreground = snapshot.default_bg;
                }
                push_background(
                    &mut backgrounds,
                    BackgroundRun {
                        x,
                        y,
                        width: u16::from(width),
                        color: background,
                    },
                );

                let start = text.len();
                match cell.map(|cell| cell.text.as_str()) {
                    Some(value) if !value.is_empty() => text.push_str(value),
                    _ => {
                        for _ in 0..width {
                            text.push(' ');
                        }
                    }
                }
                let len = text.len() - start;
                if let Some(run) = runs.last_mut().filter(|run| run.color == foreground) {
                    run.len += len;
                } else {
                    runs.push(ForegroundRun {
                        len,
                        color: foreground,
                    });
                }
            }
            rows.push(FrameRow { text, runs });
        }

        Self {
            rows,
            backgrounds,
            cursor_overlay,
        }
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
    use super::{BackgroundRun, TerminalFrame};
    use crate::{TerminalCell, TerminalLifecycle, TerminalSnapshot};

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
            frame.rows[0].runs.iter().map(|run| run.len).sum::<usize>(),
            5
        );
        assert_eq!(frame.backgrounds.len(), 1);
        assert_eq!(frame.backgrounds[0].width, 4);
    }

    #[test]
    fn a_cursor_on_a_wide_tail_becomes_a_cell_overlay() {
        let snapshot = snapshot(
            Some((1, 0)),
            vec![
                cell(0, 0, 2, "界", [0xaa, 0xbb, 0xcc]),
                cell(1, 0, 0, "", [0xaa, 0xbb, 0xcc]),
            ],
        );

        let frame = TerminalFrame::from_snapshot(&snapshot);

        assert_eq!(
            frame.cursor_overlay,
            Some(BackgroundRun {
                x: 1,
                y: 0,
                width: 1,
                color: [0xaa, 0xbb, 0xcc],
            })
        );
    }

    fn snapshot(cursor: Option<(u16, u16)>, cells: Vec<TerminalCell>) -> TerminalSnapshot {
        TerminalSnapshot {
            revision: 1,
            lifecycle: TerminalLifecycle::Running,
            cols: 4,
            rows: 1,
            cursor,
            default_fg: [0xdd; 3],
            default_bg: [0x11; 3],
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
        }
    }
}
