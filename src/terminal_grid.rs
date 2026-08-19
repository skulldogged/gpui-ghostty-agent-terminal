#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellMetrics {
    pub width_px: u16,
    pub height_px: u16,
}

impl CellMetrics {
    pub const fn new(width_px: u16, height_px: u16) -> Self {
        Self {
            width_px,
            height_px,
        }
    }
}

pub fn measured_cell_height(font_size_px: f32, ascent_px: f32, descent_px: f32) -> u16 {
    font_size_px
        .max(ascent_px.max(0.0) + descent_px.max(0.0))
        .ceil()
        .clamp(1.0, f32::from(u16::MAX)) as u16
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridDimensions {
    pub cols: u16,
    pub rows: u16,
}

impl GridDimensions {
    pub fn fit(
        view_width_px: f32,
        view_height_px: f32,
        padding_px: f32,
        cells: CellMetrics,
    ) -> Self {
        let content_width = (view_width_px - padding_px * 2.0).max(f32::from(cells.width_px));
        let content_height = (view_height_px - padding_px * 2.0).max(f32::from(cells.height_px));
        Self {
            cols: cell_count(content_width, cells.width_px),
            rows: cell_count(content_height, cells.height_px),
        }
    }
}

fn cell_count(available_px: f32, cell_px: u16) -> u16 {
    (available_px / f32::from(cell_px))
        .floor()
        .clamp(1.0, f32::from(u16::MAX)) as u16
}

pub fn cell_offset(cols: u16, x: u16, y: u16) -> Option<usize> {
    (x < cols).then(|| usize::from(y) * usize::from(cols) + usize::from(x))
}

#[cfg(test)]
mod tests {
    use super::{CellMetrics, GridDimensions, cell_offset, measured_cell_height};

    #[test]
    fn grid_dimensions_fit_whole_cells_inside_the_viewport() {
        let dimensions = GridDimensions::fit(900.0, 560.0, 12.0, CellMetrics::new(9, 20));
        assert_eq!(dimensions.cols, 97);
        assert_eq!(dimensions.rows, 26);
    }

    #[test]
    fn grid_dimensions_never_collapse_to_zero() {
        let dimensions = GridDimensions::fit(1.0, 1.0, 12.0, CellMetrics::new(9, 20));
        assert_eq!(dimensions.cols, 1);
        assert_eq!(dimensions.rows, 1);
    }

    #[test]
    fn cell_offsets_are_constant_time_row_major_lookups() {
        assert_eq!(cell_offset(80, 0, 0), Some(0));
        assert_eq!(cell_offset(80, 79, 23), Some(1_919));
        assert_eq!(cell_offset(80, 80, 0), None);
    }

    #[test]
    fn cell_height_contains_the_selected_fonts_vertical_metrics() {
        assert_eq!(measured_cell_height(14.0, 16.25, 5.25), 22);
        assert_eq!(measured_cell_height(14.0, 9.0, 3.0), 14);
    }
}
