use crate::{ghostty::SNAPSHOT_CELL_CAPACITY, terminal_session::TerminalSize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellMetrics {
    width_px: u16,
    height_px: u16,
}

impl CellMetrics {
    pub const fn new(width_px: u16, height_px: u16) -> Self {
        Self {
            width_px,
            height_px,
        }
    }

    pub fn width_px(self) -> f32 {
        f32::from(self.width_px)
    }

    pub fn height_px(self) -> f32 {
        f32::from(self.height_px)
    }

    #[cfg(test)]
    pub fn grid_width_px(self, cols: u16) -> f32 {
        self.width_px() * f32::from(cols)
    }

    pub fn terminal_size(self, dimensions: GridDimensions) -> TerminalSize {
        TerminalSize::new(
            dimensions.cols,
            dimensions.rows,
            self.width_px,
            self.height_px,
        )
    }
}

const PLATFORM_FONT_DPI: f32 = if cfg!(target_os = "macos") { 72. } else { 96. };

pub fn font_points_to_pixels(font_size_points: f32) -> f32 {
    font_points_to_pixels_at_dpi(font_size_points, PLATFORM_FONT_DPI)
}

pub fn font_pixels_to_points(font_size_px: f32) -> f32 {
    font_size_px * 72. / PLATFORM_FONT_DPI
}

fn font_points_to_pixels_at_dpi(font_size_points: f32, dpi: f32) -> f32 {
    font_size_points * dpi / 72.
}

pub fn measured_cell_width(advance_px: f32) -> u16 {
    if advance_px.is_finite() {
        advance_px.round().clamp(1., f32::from(u16::MAX)) as u16
    } else {
        1
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
        let content_width = (view_width_px - padding_px * 2.0).max(cells.width_px());
        let content_height = (view_height_px - padding_px * 2.0).max(cells.height_px());
        fit_within_capacity(Self {
            cols: cell_count(content_width, cells.width_px()),
            rows: cell_count(content_height, cells.height_px()),
        })
    }
}

fn fit_within_capacity(dimensions: GridDimensions) -> GridDimensions {
    let cell_count = u64::from(dimensions.cols) * u64::from(dimensions.rows);
    if cell_count <= SNAPSHOT_CELL_CAPACITY as u64 {
        return dimensions;
    }

    let scale = (SNAPSHOT_CELL_CAPACITY as f64 / cell_count as f64).sqrt();
    let mut cols = (f64::from(dimensions.cols) * scale).floor().max(1.0) as u16;
    let mut rows = (f64::from(dimensions.rows) * scale).floor().max(1.0) as u16;
    while usize::from(cols) * usize::from(rows) > SNAPSHOT_CELL_CAPACITY {
        if cols >= rows {
            cols -= 1;
        } else {
            rows -= 1;
        }
    }
    GridDimensions { cols, rows }
}

fn cell_count(available_px: f32, cell_px: f32) -> u16 {
    (available_px / cell_px)
        .floor()
        .clamp(1.0, f32::from(u16::MAX)) as u16
}

pub fn cell_offset(cols: u16, x: u16, y: u16) -> Option<usize> {
    (x < cols).then(|| usize::from(y) * usize::from(cols) + usize::from(x))
}

pub(crate) fn fixed_cell_glyph_x(
    cell_x: u16,
    cell_width_px: f32,
    natural_glyph_x: f32,
    natural_cell_x: f32,
) -> f32 {
    f32::from(cell_x) * cell_width_px + natural_glyph_x - natural_cell_x
}

#[cfg(test)]
mod tests {
    use super::{
        CellMetrics, GridDimensions, cell_offset, fixed_cell_glyph_x, font_pixels_to_points,
        font_points_to_pixels, font_points_to_pixels_at_dpi, measured_cell_height,
        measured_cell_width,
    };

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
    fn viewport_grid_stays_within_the_snapshot_capacity() {
        let dimensions = GridDimensions::fit(3_840.0, 2_160.0, 0.0, CellMetrics::new(8, 8));
        assert!(usize::from(dimensions.cols) * usize::from(dimensions.rows) <= 65_536);
        assert!(
            dimensions.cols > dimensions.rows,
            "preserve viewport aspect ratio"
        );
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

    #[test]
    fn repeated_glyphs_stay_anchored_to_the_cursor_grid() {
        let natural_advance = 8.25;
        let cell_width = 9.;

        for cell_x in 0..64 {
            let natural_x = f32::from(cell_x) * natural_advance;
            let painted_x = fixed_cell_glyph_x(cell_x, cell_width, natural_x, natural_x);
            assert_eq!(painted_x, f32::from(cell_x) * cell_width);
        }
    }

    #[test]
    fn point_sized_font_uses_ghostty_style_nearest_pixel_cells() {
        let measured_advance_at_fourteen_pixels = 7.636_364;
        let font_size_px = font_points_to_pixels_at_dpi(10., 96.);
        let natural_advance = measured_advance_at_fourteen_pixels * font_size_px / 14.;
        let cols = 54;
        let cells = CellMetrics::new(measured_cell_width(natural_advance), 20);
        let size = cells.terminal_size(GridDimensions { cols, rows: 24 });

        assert!((font_size_px - 13.333_333).abs() < 0.001);
        assert!((natural_advance - 7.272_727).abs() < 0.001);
        assert_eq!(cells.grid_width_px(cols), 378.);
        assert_eq!(size.cell_width_px, 7);
        assert_eq!(cols * size.cell_width_px, 378);
    }

    #[test]
    fn fractional_cell_advances_round_to_the_nearest_pixel() {
        assert_eq!(measured_cell_width(7.49), 7);
        assert_eq!(measured_cell_width(7.5), 8);
    }

    #[test]
    fn adjacent_point_sizes_remain_distinct_render_sizes() {
        let ten_points = font_points_to_pixels(10.);
        let eleven_points = font_points_to_pixels(11.);
        let expected_step = if cfg!(target_os = "macos") {
            1.
        } else {
            1.333_333
        };

        assert!(eleven_points > ten_points);
        assert!((eleven_points - ten_points - expected_step).abs() < 0.001);
    }

    #[test]
    fn platform_point_conversion_round_trips_legacy_pixels() {
        let font_size_points = font_pixels_to_points(14.);

        assert!((font_points_to_pixels(font_size_points) - 14.).abs() < 0.001);
    }

    #[test]
    fn glyph_offsets_inside_a_combining_cluster_are_preserved() {
        assert_eq!(fixed_cell_glyph_x(7, 9., 11.5, 10.75), 63.75);
    }
}
