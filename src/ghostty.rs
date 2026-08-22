use std::ffi::c_void;
use std::ptr::NonNull;

pub(crate) const SNAPSHOT_CELL_CAPACITY: usize = 65_536;

#[repr(C)]
#[derive(Default)]
struct RawCell {
    x: u16,
    y: u16,
    text: [u8; 32],
    text_len: u8,
    width: u8,
    fg_r: u8,
    fg_g: u8,
    fg_b: u8,
    bg_r: u8,
    bg_g: u8,
    bg_b: u8,
    has_explicit_bg: bool,
}

#[repr(C)]
#[derive(Default)]
struct RawSnapshot {
    cols: u16,
    rows: u16,
    cursor_x: u16,
    cursor_y: u16,
    cursor_visible: bool,
    default_fg_r: u8,
    default_fg_g: u8,
    default_fg_b: u8,
    default_bg_r: u8,
    default_bg_g: u8,
    default_bg_b: u8,
    full: bool,
    cell_count: usize,
}

unsafe extern "C" {
    fn spike_terminal_new(cols: u16, rows: u16, scrollback: usize) -> *mut c_void;
    fn spike_terminal_free(terminal: *mut c_void);
    fn spike_terminal_write(terminal: *mut c_void, data: *const u8, len: usize);
    fn spike_terminal_resize(
        terminal: *mut c_void,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> i32;
    fn spike_terminal_snapshot(
        terminal: *mut c_void,
        force_full: bool,
        snapshot: *mut RawSnapshot,
        cells: *mut RawCell,
        capacity: usize,
    ) -> i32;
}

#[cfg(not(feature = "gui"))]
pub const SOURCE_REVISION: &str = env!("GHOSTTY_SOURCE_REVISION");

pub struct Terminal {
    raw: NonNull<c_void>,
    raw_cells: Box<[RawCell]>,
}

pub struct Snapshot {
    pub full: bool,
    pub dirty_rows: Vec<u16>,
    pub cols: u16,
    pub rows: u16,
    pub cursor: Option<(u16, u16)>,
    pub default_fg: [u8; 3],
    pub default_bg: [u8; 3],
    pub cells: Vec<Cell>,
}

#[derive(Clone)]
pub struct Cell {
    pub x: u16,
    pub y: u16,
    pub text: String,
    pub width: u8,
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    #[cfg_attr(feature = "gui", allow(dead_code))]
    pub has_explicit_bg: bool,
}

impl Terminal {
    pub fn new(cols: u16, rows: u16) -> Result<Self, String> {
        let raw = unsafe { spike_terminal_new(cols, rows, 10_000) };
        Ok(Self {
            raw: NonNull::new(raw).ok_or("libghostty-vt terminal allocation failed")?,
            raw_cells: (0..SNAPSHOT_CELL_CAPACITY)
                .map(|_| RawCell::default())
                .collect(),
        })
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        unsafe { spike_terminal_write(self.raw.as_ptr(), bytes.as_ptr(), bytes.len()) }
    }

    pub fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<(), String> {
        let result = unsafe {
            spike_terminal_resize(self.raw.as_ptr(), cols, rows, cell_width_px, cell_height_px)
        };
        result_ok(result, "resize")
    }

    #[cfg_attr(feature = "gui", allow(dead_code))]
    pub fn snapshot(&mut self) -> Result<Snapshot, String> {
        self.render_update(true)
    }

    pub(crate) fn render_update(&mut self, force_full: bool) -> Result<Snapshot, String> {
        let mut raw_snapshot = RawSnapshot::default();
        let result = unsafe {
            spike_terminal_snapshot(
                self.raw.as_ptr(),
                force_full,
                &mut raw_snapshot,
                self.raw_cells.as_mut_ptr(),
                self.raw_cells.len(),
            )
        };
        result_ok(result, "snapshot")?;
        if raw_snapshot.cell_count > self.raw_cells.len() {
            return Err(format!(
                "snapshot needs {} cells, prototype buffer has {}",
                raw_snapshot.cell_count,
                self.raw_cells.len()
            ));
        }

        let cells: Vec<Cell> = self.raw_cells[..raw_snapshot.cell_count]
            .iter()
            .map(|raw| Cell {
                x: raw.x,
                y: raw.y,
                text: String::from_utf8_lossy(&raw.text[..usize::from(raw.text_len)]).into_owned(),
                width: raw.width,
                fg: [raw.fg_r, raw.fg_g, raw.fg_b],
                bg: [raw.bg_r, raw.bg_g, raw.bg_b],
                has_explicit_bg: raw.has_explicit_bg,
            })
            .collect();
        let mut dirty_rows = cells.iter().map(|cell| cell.y).collect::<Vec<_>>();
        dirty_rows.dedup();

        Ok(Snapshot {
            full: raw_snapshot.full,
            dirty_rows,
            cols: raw_snapshot.cols,
            rows: raw_snapshot.rows,
            cursor: raw_snapshot
                .cursor_visible
                .then_some((raw_snapshot.cursor_x, raw_snapshot.cursor_y)),
            default_fg: [
                raw_snapshot.default_fg_r,
                raw_snapshot.default_fg_g,
                raw_snapshot.default_fg_b,
            ],
            default_bg: [
                raw_snapshot.default_bg_r,
                raw_snapshot.default_bg_g,
                raw_snapshot.default_bg_b,
            ],
            cells,
        })
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe { spike_terminal_free(self.raw.as_ptr()) }
    }
}

fn result_ok(result: i32, operation: &str) -> Result<(), String> {
    if result == 0 {
        Ok(())
    } else {
        Err(format!("libghostty-vt {operation} failed with {result}"))
    }
}

#[cfg(any(test, not(feature = "gui")))]
pub fn snapshot_text(snapshot: &Snapshot) -> String {
    let mut output = String::new();
    for y in 0..snapshot.rows {
        for x in 0..snapshot.cols {
            let cell = snapshot
                .cells
                .iter()
                .find(|cell| cell.x == x && cell.y == y);
            match cell {
                Some(cell) if !cell.text.is_empty() => output.push_str(&cell.text),
                _ => output.push(' '),
            }
        }
        output.push('\n');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::Terminal;

    #[test]
    fn render_updates_consume_only_rows_marked_dirty_by_ghostty() {
        let mut terminal = Terminal::new(8, 3).expect("create terminal");

        let initial = terminal.render_update(false).expect("initial update");
        assert!(initial.full);
        assert_eq!(initial.dirty_rows, vec![0, 1, 2]);
        assert_eq!(initial.cells.len(), 24);

        let clean = terminal.render_update(false).expect("clean update");
        assert!(!clean.full);
        assert!(clean.dirty_rows.is_empty());
        assert!(clean.cells.is_empty());

        terminal.feed(b"x");
        let changed = terminal.render_update(false).expect("changed update");
        assert!(!changed.full);
        assert_eq!(changed.dirty_rows, vec![0]);
        assert_eq!(changed.cells.len(), 8);

        let forced = terminal.render_update(true).expect("forced full update");
        assert!(forced.full);
        assert_eq!(forced.dirty_rows, vec![0, 1, 2]);
        assert_eq!(forced.cells.len(), 24);
    }

    #[test]
    fn reverse_video_is_reported_as_an_explicit_background() {
        let mut terminal = Terminal::new(4, 1).expect("create terminal");
        terminal.feed(b"\x1b[7mX\x1b[0mY");

        let snapshot = terminal.snapshot().expect("snapshot terminal");
        let reversed = snapshot
            .cells
            .iter()
            .find(|cell| cell.x == 0)
            .expect("reversed cell");
        let plain = snapshot
            .cells
            .iter()
            .find(|cell| cell.x == 1)
            .expect("plain cell");

        assert!(reversed.has_explicit_bg);
        assert_eq!(reversed.bg, snapshot.default_fg);
        assert!(!plain.has_explicit_bg);
        assert_eq!(plain.bg, snapshot.default_bg);
    }
}
