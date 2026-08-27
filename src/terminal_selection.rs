use crate::TerminalSnapshot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TerminalPoint {
    pub(crate) y: u16,
    pub(crate) x: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SelectionRow {
    pub(crate) y: u16,
    pub(crate) start_x: u16,
    pub(crate) end_x: u16,
}

pub(crate) fn point_for_position(
    x: f32,
    y: f32,
    cell_width: f32,
    cell_height: f32,
    snapshot: &TerminalSnapshot,
) -> Option<TerminalPoint> {
    if snapshot.cols == 0
        || snapshot.rows == 0
        || !x.is_finite()
        || !y.is_finite()
        || !cell_width.is_finite()
        || !cell_height.is_finite()
        || cell_width <= 0.0
        || cell_height <= 0.0
    {
        return None;
    }
    Some(TerminalPoint {
        x: (x / cell_width)
            .floor()
            .clamp(0.0, f32::from(snapshot.cols - 1)) as u16,
        y: (y / cell_height)
            .floor()
            .clamp(0.0, f32::from(snapshot.rows - 1)) as u16,
    })
}

pub(crate) fn selection_rows(snapshot: &TerminalSnapshot) -> Vec<SelectionRow> {
    let mut rows = Vec::new();
    let mut cell_index = 0;
    for y in 0..snapshot.rows {
        let mut start_x = None;
        for x in 0..snapshot.cols {
            while snapshot
                .cells
                .get(cell_index)
                .is_some_and(|cell| (cell.y, cell.x) < (y, x))
            {
                cell_index += 1;
            }
            let selected = snapshot
                .cells
                .get(cell_index)
                .is_some_and(|cell| (cell.y, cell.x) == (y, x) && cell.selected);
            match (start_x, selected) {
                (None, true) => start_x = Some(x),
                (Some(start), false) => {
                    rows.push(SelectionRow {
                        y,
                        start_x: start,
                        end_x: x - 1,
                    });
                    start_x = None;
                }
                _ => {}
            }
        }
        if let Some(start_x) = start_x {
            rows.push(SelectionRow {
                y,
                start_x,
                end_x: snapshot.cols - 1,
            });
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::{SelectionRow, TerminalPoint, point_for_position, selection_rows};
    use crate::{TerminalCell, TerminalCursorShape, TerminalLifecycle, TerminalSnapshot};

    #[test]
    fn pointer_positions_clamp_to_the_visible_grid() {
        let snapshot = snapshot(4, 2, &[]);
        assert_eq!(
            point_for_position(500.0, -5.0, 10.0, 20.0, &snapshot),
            Some(TerminalPoint { x: 3, y: 0 })
        );
    }

    #[test]
    fn native_selected_cells_become_visible_row_ranges() {
        let snapshot = snapshot(5, 2, &[(1, 0), (2, 0), (0, 1)]);
        assert_eq!(
            selection_rows(&snapshot),
            vec![
                SelectionRow {
                    y: 0,
                    start_x: 1,
                    end_x: 2,
                },
                SelectionRow {
                    y: 1,
                    start_x: 0,
                    end_x: 0,
                },
            ]
        );
    }

    fn snapshot(cols: u16, rows: u16, selected: &[(u16, u16)]) -> TerminalSnapshot {
        let mut cells = Vec::new();
        for y in 0..rows {
            for x in 0..cols {
                cells.push(TerminalCell {
                    x,
                    y,
                    width: 1,
                    text: String::new(),
                    fg: [0xdd; 3],
                    bg: [0x11; 3],
                    has_explicit_bg: false,
                    soft_wrapped: false,
                    selected: selected.contains(&(x, y)),
                });
            }
        }
        TerminalSnapshot {
            revision: 1,
            lifecycle: TerminalLifecycle::Running,
            active_work: false,
            title: None,
            agent: None,
            cols,
            rows,
            cursor: None,
            cursor_shape: TerminalCursorShape::Block,
            cursor_blinking: false,
            cursor_wide_tail: false,
            default_fg: [0xdd; 3],
            default_bg: [0x11; 3],
            selection_text: None,
            cells,
        }
    }
}
