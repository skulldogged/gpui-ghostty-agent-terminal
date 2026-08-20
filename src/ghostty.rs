use std::ffi::c_void;
use std::ptr::NonNull;

#[repr(C)]
#[derive(Default)]
struct RawCell {
    x: u16,
    y: u16,
    text: [u8; 32],
    text_len: u8,
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
        snapshot: *mut RawSnapshot,
        cells: *mut RawCell,
        capacity: usize,
    ) -> i32;
}

#[cfg(not(feature = "gui"))]
pub const SOURCE_REVISION: &str = env!("GHOSTTY_SOURCE_REVISION");

pub struct Terminal {
    raw: NonNull<c_void>,
}

pub struct Snapshot {
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
        })
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        unsafe { spike_terminal_write(self.raw.as_ptr(), bytes.as_ptr(), bytes.len()) }
    }

    #[allow(dead_code)] // Used by the fixed-cell renderer in the next stacked PR.
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

    pub fn snapshot(&mut self) -> Result<Snapshot, String> {
        let mut raw_snapshot = RawSnapshot::default();
        let mut raw_cells: Vec<RawCell> = (0..65_536).map(|_| RawCell::default()).collect();
        let result = unsafe {
            spike_terminal_snapshot(
                self.raw.as_ptr(),
                &mut raw_snapshot,
                raw_cells.as_mut_ptr(),
                raw_cells.len(),
            )
        };
        result_ok(result, "snapshot")?;
        if raw_snapshot.cell_count > raw_cells.len() {
            return Err(format!(
                "snapshot needs {} cells, prototype buffer has {}",
                raw_snapshot.cell_count,
                raw_cells.len()
            ));
        }
        raw_cells.truncate(raw_snapshot.cell_count);

        let cells = raw_cells
            .into_iter()
            .map(|raw| Cell {
                x: raw.x,
                y: raw.y,
                text: String::from_utf8_lossy(&raw.text[..usize::from(raw.text_len)]).into_owned(),
                fg: [raw.fg_r, raw.fg_g, raw.fg_b],
                bg: [raw.bg_r, raw.bg_g, raw.bg_b],
                has_explicit_bg: raw.has_explicit_bg,
            })
            .collect();

        Ok(Snapshot {
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
