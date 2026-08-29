use std::ffi::c_void;
use std::ptr::NonNull;

pub(crate) const SNAPSHOT_CELL_CAPACITY: usize = 65_536;
const MAX_SCROLL_OUTPUT_STEPS: usize = 16_384;

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
    soft_wrapped: bool,
    selected: bool,
}

#[repr(C)]
struct RawSnapshot {
    cols: u16,
    rows: u16,
    cursor_x: u16,
    cursor_y: u16,
    cursor_style: i32,
    title: [u8; 512],
    title_len: u16,
    cursor_visible: bool,
    cursor_blinking: bool,
    cursor_wide_tail: bool,
    bracketed_paste: bool,
    alternate_screen: bool,
    selection_active: bool,
    default_fg_r: u8,
    default_fg_g: u8,
    default_fg_b: u8,
    default_bg_r: u8,
    default_bg_g: u8,
    default_bg_b: u8,
    full: bool,
    cell_count: usize,
}

#[repr(C)]
struct RawSelectionInput {
    event_type: u8,
    click_count: u8,
    x: u16,
    y: u16,
    pointer_x: f32,
    pointer_y: f32,
    columns: u32,
    cell_width: u32,
    padding_left: u32,
    screen_height: u32,
}

#[repr(C)]
struct RawKeyInput {
    key: *const u8,
    key_len: usize,
    text: *const u8,
    text_len: usize,
    modifiers: u16,
}

impl Default for RawSnapshot {
    fn default() -> Self {
        Self {
            cols: 0,
            rows: 0,
            cursor_x: 0,
            cursor_y: 0,
            cursor_style: 1,
            title: [0; 512],
            title_len: 0,
            cursor_visible: false,
            cursor_blinking: false,
            cursor_wide_tail: false,
            bracketed_paste: false,
            alternate_screen: false,
            selection_active: false,
            default_fg_r: 0,
            default_fg_g: 0,
            default_fg_b: 0,
            default_bg_r: 0,
            default_bg_g: 0,
            default_bg_b: 0,
            full: false,
            cell_count: 0,
        }
    }
}

unsafe extern "C" {
    fn spike_terminal_new(cols: u16, rows: u16, scrollback: usize) -> *mut c_void;
    fn spike_terminal_free(terminal: *mut c_void);
    fn spike_terminal_write(
        terminal: *mut c_void,
        data: *const u8,
        len: usize,
        response: *mut *const u8,
        response_len: *mut usize,
    ) -> i32;
    fn spike_terminal_resize(
        terminal: *mut c_void,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
        response: *mut *const u8,
        response_len: *mut usize,
    ) -> i32;
    fn spike_terminal_set_theme(
        terminal: *mut c_void,
        foreground: *const u8,
        background: *const u8,
        cursor: *const u8,
        ansi_palette: *const u8,
    ) -> i32;
    fn spike_terminal_set_default_cursor_style(terminal: *mut c_void, cursor_style: i32) -> i32;
    fn spike_terminal_set_default_cursor_blink(terminal: *mut c_void, cursor_blink: bool) -> i32;
    fn spike_terminal_encode_paste(
        terminal: *mut c_void,
        data: *mut u8,
        data_len: usize,
        output: *mut u8,
        output_len: usize,
        output_written: *mut usize,
    ) -> i32;
    fn spike_terminal_encode_key(
        terminal: *mut c_void,
        input: *const RawKeyInput,
        output: *mut u8,
        output_len: usize,
        output_written: *mut usize,
    ) -> i32;
    fn spike_terminal_scroll(
        terminal: *mut c_void,
        delta_rows: isize,
        delta_columns: isize,
        pointer_x: f32,
        pointer_y: f32,
        viewport_width: u32,
        viewport_height: u32,
        cell_width: u32,
        cell_height: u32,
        modifiers: u16,
        output: *mut u8,
        output_len: usize,
        output_written: *mut usize,
        viewport_changed: *mut bool,
    ) -> i32;
    fn spike_terminal_scroll_to_bottom(terminal: *mut c_void, changed: *mut bool) -> i32;
    fn spike_terminal_selection_event(
        terminal: *mut c_void,
        input: *const RawSelectionInput,
        viewport_changed: *mut bool,
    ) -> i32;
    fn spike_terminal_link_context(
        terminal: *mut c_void,
        x: u16,
        y: u16,
        output: *mut u8,
        output_len: usize,
        output_written: *mut usize,
        click_offset: *mut usize,
    ) -> i32;
    fn spike_terminal_select_link(
        terminal: *mut c_void,
        x: u16,
        y: u16,
        match_start: usize,
        match_end: usize,
        selected: *mut bool,
    ) -> i32;
    fn spike_terminal_selection_text(
        terminal: *mut c_void,
        output: *mut u8,
        output_len: usize,
        output_written: *mut usize,
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
    selection_text_cache: Option<String>,
    selection_dragging: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ScrollInput {
    pub delta_rows: isize,
    pub delta_columns: isize,
    pub pointer_x: f32,
    pub pointer_y: f32,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub modifiers: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KeyInput {
    pub key: String,
    pub text: Option<String>,
    pub modifiers: u16,
}

struct LinkContext {
    text: String,
    click_offset: usize,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ScrollResult {
    pub viewport_changed: bool,
    pub input: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectionEventType {
    Press,
    Drag,
    Release,
    Autoscroll,
    Clear,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SelectionInput {
    pub event_type: SelectionEventType,
    pub click_count: u8,
    pub x: u16,
    pub y: u16,
    pub pointer_x: f32,
    pub pointer_y: f32,
    pub columns: u32,
    pub cell_width: u32,
    pub padding_left: u32,
    pub screen_height: u32,
}

impl SelectionInput {
    pub(crate) fn clear() -> Self {
        Self {
            event_type: SelectionEventType::Clear,
            click_count: 0,
            x: 0,
            y: 0,
            pointer_x: 0.0,
            pointer_y: 0.0,
            columns: 0,
            cell_width: 0,
            padding_left: 0,
            screen_height: 0,
        }
    }
}

pub struct Snapshot {
    pub full: bool,
    pub dirty_rows: Vec<u16>,
    pub cols: u16,
    pub rows: u16,
    pub title: Option<String>,
    pub cursor: Option<(u16, u16)>,
    pub cursor_shape: CursorShape,
    pub cursor_blinking: bool,
    pub cursor_wide_tail: bool,
    pub bracketed_paste: bool,
    pub alternate_screen: bool,
    pub default_fg: [u8; 3],
    pub default_bg: [u8; 3],
    pub selection_text: Option<String>,
    pub cells: Vec<Cell>,
}

#[repr(i32)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CursorShape {
    Bar,
    #[default]
    Block,
    Underline,
    BlockHollow,
}

impl TryFrom<i32> for CursorShape {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Bar),
            1 => Ok(Self::Block),
            2 => Ok(Self::Underline),
            3 => Ok(Self::BlockHollow),
            value => Err(format!("unknown libghostty-vt cursor style {value}")),
        }
    }
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
    pub soft_wrapped: bool,
    pub selected: bool,
}

impl Terminal {
    pub fn new(cols: u16, rows: u16) -> Result<Self, String> {
        let raw = unsafe { spike_terminal_new(cols, rows, 10_000) };
        Ok(Self {
            raw: NonNull::new(raw).ok_or("libghostty-vt terminal allocation failed")?,
            raw_cells: (0..SNAPSHOT_CELL_CAPACITY)
                .map(|_| RawCell::default())
                .collect(),
            selection_text_cache: None,
            selection_dragging: false,
        })
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<u8>, String> {
        let mut response = std::ptr::null();
        let mut response_len = 0;
        let result = unsafe {
            spike_terminal_write(
                self.raw.as_ptr(),
                bytes.as_ptr(),
                bytes.len(),
                &mut response,
                &mut response_len,
            )
        };
        self.selection_text_cache = None;
        result_ok(result, "process terminal output")?;
        copy_pty_response(response, response_len)
    }

    pub fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<Vec<u8>, String> {
        let mut response = std::ptr::null();
        let mut response_len = 0;
        let result = unsafe {
            spike_terminal_resize(
                self.raw.as_ptr(),
                cols,
                rows,
                cell_width_px,
                cell_height_px,
                &mut response,
                &mut response_len,
            )
        };
        result_ok(result, "resize")?;
        copy_pty_response(response, response_len)
    }

    #[cfg(feature = "gui")]
    pub(crate) fn set_theme(
        &mut self,
        theme: crate::terminal_theme::TerminalTheme,
    ) -> Result<(), String> {
        let result = unsafe {
            spike_terminal_set_theme(
                self.raw.as_ptr(),
                theme.foreground.as_ptr(),
                theme.background.as_ptr(),
                theme.cursor.as_ptr(),
                theme.palette.as_ptr().cast(),
            )
        };
        result_ok(result, "set color theme")
    }

    #[cfg(feature = "gui")]
    pub(crate) fn set_default_cursor_shape(&mut self, shape: CursorShape) -> Result<(), String> {
        let result =
            unsafe { spike_terminal_set_default_cursor_style(self.raw.as_ptr(), shape as i32) };
        result_ok(result, "set default cursor style")
    }

    #[cfg(feature = "gui")]
    pub(crate) fn set_default_cursor_blink(&mut self, blink: bool) -> Result<(), String> {
        let result = unsafe { spike_terminal_set_default_cursor_blink(self.raw.as_ptr(), blink) };
        result_ok(result, "set default cursor blink")
    }

    pub fn encode_paste(&mut self, bytes: &[u8]) -> Result<Vec<u8>, String> {
        const BRACKETED_PASTE_OVERHEAD: usize = 12;
        let capacity = bytes
            .len()
            .checked_add(BRACKETED_PASTE_OVERHEAD)
            .ok_or_else(|| "terminal paste is too large".to_string())?;
        let mut input = bytes.to_vec();
        let mut output = vec![0; capacity];
        let mut written = 0;
        let result = unsafe {
            spike_terminal_encode_paste(
                self.raw.as_ptr(),
                input.as_mut_ptr(),
                input.len(),
                output.as_mut_ptr(),
                output.len(),
                &mut written,
            )
        };
        result_ok(result, "encode paste")?;
        if written > output.len() {
            return Err(format!(
                "libghostty-vt paste needs {written} bytes, buffer has {}",
                output.len()
            ));
        }
        output.truncate(written);
        Ok(output)
    }

    pub(crate) fn encode_key(&mut self, input: &KeyInput) -> Result<Vec<u8>, String> {
        const MAX_KEY_OUTPUT_BYTES: usize = 1024 * 1024;
        let text = input.text.as_deref().unwrap_or_default();
        let raw = RawKeyInput {
            key: input.key.as_ptr(),
            key_len: input.key.len(),
            text: text.as_ptr(),
            text_len: text.len(),
            modifiers: input.modifiers,
        };
        let mut output = vec![0; text.len().saturating_mul(2).max(128)];
        loop {
            let mut written = 0;
            let result = unsafe {
                spike_terminal_encode_key(
                    self.raw.as_ptr(),
                    &raw,
                    output.as_mut_ptr(),
                    output.len(),
                    &mut written,
                )
            };
            if result == -3 {
                if written <= output.len() || written > MAX_KEY_OUTPUT_BYTES {
                    return Err("libghostty-vt key encoding is too large".into());
                }
                output.resize(written, 0);
                continue;
            }
            result_ok(result, "encode key")?;
            if written > output.len() {
                return Err(format!(
                    "libghostty-vt key encoding needs {written} bytes, buffer has {}",
                    output.len()
                ));
            }
            output.truncate(written);
            return Ok(output);
        }
    }

    pub(crate) fn scroll(&mut self, input: ScrollInput) -> Result<ScrollResult, String> {
        if input.delta_rows == 0 && input.delta_columns == 0 {
            return Ok(ScrollResult::default());
        }
        let output_steps = input
            .delta_rows
            .unsigned_abs()
            .checked_add(input.delta_columns.unsigned_abs())
            .ok_or_else(|| "terminal scroll output is too large".to_string())?;
        if output_steps > MAX_SCROLL_OUTPUT_STEPS {
            return Err("terminal scroll output is too large".into());
        }
        let output_capacity = output_steps
            .checked_mul(128)
            .ok_or_else(|| "terminal scroll output is too large".to_string())?;
        let mut output = vec![0; output_capacity];
        let mut output_written = 0;
        let mut viewport_changed = false;
        let result = unsafe {
            spike_terminal_scroll(
                self.raw.as_ptr(),
                input.delta_rows,
                input.delta_columns,
                input.pointer_x,
                input.pointer_y,
                input.viewport_width,
                input.viewport_height,
                input.cell_width,
                input.cell_height,
                input.modifiers,
                output.as_mut_ptr(),
                output.len(),
                &mut output_written,
                &mut viewport_changed,
            )
        };
        result_ok(result, "route terminal scroll")?;
        if output_written > output.len() {
            return Err(format!(
                "libghostty-vt scroll needs {output_written} bytes, buffer has {}",
                output.len()
            ));
        }
        output.truncate(output_written);
        Ok(ScrollResult {
            viewport_changed,
            input: output,
        })
    }

    pub(crate) fn scroll_to_bottom(&mut self) -> Result<bool, String> {
        let mut changed = false;
        let result = unsafe { spike_terminal_scroll_to_bottom(self.raw.as_ptr(), &mut changed) };
        result_ok(result, "scroll terminal to bottom")?;
        Ok(changed)
    }

    pub(crate) fn selection_event(&mut self, input: SelectionInput) -> Result<bool, String> {
        let raw = RawSelectionInput {
            event_type: match input.event_type {
                SelectionEventType::Press => 0,
                SelectionEventType::Drag => 1,
                SelectionEventType::Release => 2,
                SelectionEventType::Autoscroll => 3,
                SelectionEventType::Clear => 4,
            },
            click_count: input.click_count,
            x: input.x,
            y: input.y,
            pointer_x: input.pointer_x,
            pointer_y: input.pointer_y,
            columns: input.columns,
            cell_width: input.cell_width,
            padding_left: input.padding_left,
            screen_height: input.screen_height,
        };
        let mut viewport_changed = false;
        let result = unsafe {
            spike_terminal_selection_event(self.raw.as_ptr(), &raw, &mut viewport_changed)
        };
        result_ok(result, "update selection")?;
        if input.event_type == SelectionEventType::Press && input.click_count == 2 {
            let _ = self.select_link_at(input.x, input.y);
        }
        match input.event_type {
            SelectionEventType::Press
            | SelectionEventType::Drag
            | SelectionEventType::Autoscroll => {
                self.selection_dragging = true;
                self.selection_text_cache = None;
            }
            SelectionEventType::Release => {
                self.selection_dragging = false;
                self.selection_text_cache = self.selection_text()?;
            }
            SelectionEventType::Clear => {
                self.selection_dragging = false;
                self.selection_text_cache = None;
            }
        }
        Ok(viewport_changed)
    }

    fn select_link_at(&mut self, x: u16, y: u16) -> Result<bool, String> {
        let Some(context) = self.link_context(x, y)? else {
            return Ok(false);
        };
        let Some(range) = crate::terminal_link::match_at(&context.text, context.click_offset)
            .map_err(|error| format!("match terminal link: {error}"))?
        else {
            return Ok(false);
        };
        self.select_link(x, y, range.start, range.end)
    }

    fn link_context(&mut self, x: u16, y: u16) -> Result<Option<LinkContext>, String> {
        const MAX_LINK_LINE_BYTES: usize = 1024 * 1024;
        let mut required = 0;
        let mut click_offset = 0;
        let result = unsafe {
            spike_terminal_link_context(
                self.raw.as_ptr(),
                x,
                y,
                std::ptr::null_mut(),
                0,
                &mut required,
                &mut click_offset,
            )
        };
        if result == -4 {
            return Ok(None);
        }
        if result != -3 {
            result_ok(result, "measure terminal link context")?;
        }
        if required == 0 {
            return Ok(None);
        }
        if required > MAX_LINK_LINE_BYTES {
            return Err("terminal link line is too large".into());
        }

        let mut output = vec![0; required];
        let mut written = 0;
        let result = unsafe {
            spike_terminal_link_context(
                self.raw.as_ptr(),
                x,
                y,
                output.as_mut_ptr(),
                output.len(),
                &mut written,
                &mut click_offset,
            )
        };
        result_ok(result, "read terminal link context")?;
        if written > output.len() || click_offset >= written {
            return Err("libghostty-vt returned an invalid link context".into());
        }
        output.truncate(written);
        let text = String::from_utf8(output)
            .map_err(|error| format!("terminal link context is not UTF-8: {error}"))?;
        Ok(Some(LinkContext { text, click_offset }))
    }

    fn select_link(
        &mut self,
        x: u16,
        y: u16,
        match_start: usize,
        match_end: usize,
    ) -> Result<bool, String> {
        let mut selected = false;
        let result = unsafe {
            spike_terminal_select_link(
                self.raw.as_ptr(),
                x,
                y,
                match_start,
                match_end,
                &mut selected,
            )
        };
        result_ok(result, "select terminal link")?;
        Ok(selected)
    }

    fn selection_text(&mut self) -> Result<Option<String>, String> {
        let mut required = 0;
        let result = unsafe {
            spike_terminal_selection_text(self.raw.as_ptr(), std::ptr::null_mut(), 0, &mut required)
        };
        if result == -4 {
            return Ok(None);
        }
        if result != -3 {
            result_ok(result, "measure selection text")?;
        }
        let mut output = vec![0; required];
        let mut written = 0;
        let result = unsafe {
            spike_terminal_selection_text(
                self.raw.as_ptr(),
                output.as_mut_ptr(),
                output.len(),
                &mut written,
            )
        };
        result_ok(result, "format selection text")?;
        if written > output.len() {
            return Err(format!(
                "libghostty-vt selection needs {written} bytes, buffer has {}",
                output.len()
            ));
        }
        output.truncate(written);
        String::from_utf8(output)
            .map(Some)
            .map_err(|error| format!("libghostty-vt selection is not UTF-8: {error}"))
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
                soft_wrapped: raw.soft_wrapped,
                selected: raw.selected,
            })
            .collect();
        let mut dirty_rows = cells.iter().map(|cell| cell.y).collect::<Vec<_>>();
        dirty_rows.dedup();
        let title =
            String::from_utf8_lossy(&raw_snapshot.title[..usize::from(raw_snapshot.title_len)])
                .chars()
                .map(|character| {
                    if character.is_control() {
                        ' '
                    } else {
                        character
                    }
                })
                .collect::<String>();
        let title = (!title.trim().is_empty()).then(|| title.trim().to_owned());
        if !raw_snapshot.selection_active {
            self.selection_text_cache = None;
        } else if !self.selection_dragging && self.selection_text_cache.is_none() {
            self.selection_text_cache = self.selection_text()?;
        }
        let selection_text = self.selection_text_cache.clone();

        Ok(Snapshot {
            full: raw_snapshot.full,
            dirty_rows,
            cols: raw_snapshot.cols,
            rows: raw_snapshot.rows,
            title,
            cursor: raw_snapshot
                .cursor_visible
                .then_some((raw_snapshot.cursor_x, raw_snapshot.cursor_y)),
            cursor_shape: CursorShape::try_from(raw_snapshot.cursor_style)?,
            cursor_blinking: raw_snapshot.cursor_blinking,
            cursor_wide_tail: raw_snapshot.cursor_wide_tail,
            bracketed_paste: raw_snapshot.bracketed_paste,
            alternate_screen: raw_snapshot.alternate_screen,
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
            selection_text,
            cells,
        })
    }
}

fn copy_pty_response(response: *const u8, response_len: usize) -> Result<Vec<u8>, String> {
    if response_len == 0 {
        return Ok(Vec::new());
    }
    if response.is_null() {
        return Err("libghostty-vt returned a null PTY response".into());
    }
    Ok(unsafe { std::slice::from_raw_parts(response, response_len) }.to_vec())
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
    use super::{CursorShape, KeyInput, ScrollInput, SelectionEventType, SelectionInput, Terminal};
    #[cfg(feature = "gui")]
    use crate::terminal_theme::ThemePreset;

    #[test]
    fn terminal_query_responses_are_returned_in_order_and_cleared_after_feed() {
        let mut terminal = Terminal::new(8, 3).expect("create terminal");

        assert_eq!(
            terminal
                .feed(b"\x1b[0c\x1b[0c")
                .expect("process device attribute queries"),
            b"\x1b[?62;22c\x1b[?62;22c"
        );
        assert!(
            terminal
                .feed(b"ordinary output")
                .expect("process ordinary output")
                .is_empty()
        );
    }

    #[cfg(feature = "gui")]
    #[test]
    fn terminal_theme_updates_ghostty_defaults_and_ansi_palette() {
        let mut terminal = Terminal::new(8, 3).expect("create terminal");
        let theme = ThemePreset::CatppuccinMocha.terminal_theme();
        terminal.set_theme(theme).expect("apply terminal theme");
        terminal
            .feed(b"\x1b[31mred\x1b[0m")
            .expect("feed styled output");

        let snapshot = terminal.snapshot().expect("snapshot terminal");
        assert_eq!(snapshot.default_fg, theme.foreground);
        assert_eq!(snapshot.default_bg, theme.background);
        assert!(
            snapshot
                .cells
                .iter()
                .any(|cell| cell.fg == theme.palette[1])
        );
    }

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

        terminal.feed(b"x").expect("feed changed output");
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
    fn viewport_scrolling_uses_ghostty_scrollback() {
        let mut terminal = Terminal::new(8, 3).expect("create terminal");
        terminal
            .feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfive")
            .expect("feed scrollback output");
        let bottom = terminal.snapshot().expect("snapshot bottom");
        assert!(super::snapshot_text(&bottom).contains("five"));

        let scroll = terminal
            .scroll(ScrollInput {
                delta_rows: -2,
                delta_columns: 0,
                pointer_x: 0.0,
                pointer_y: 0.0,
                viewport_width: 80,
                viewport_height: 60,
                cell_width: 10,
                cell_height: 20,
                modifiers: 0,
            })
            .expect("scroll up");
        assert!(scroll.viewport_changed);
        assert!(scroll.input.is_empty());
        let history = terminal.snapshot().expect("snapshot history");
        let history_text = super::snapshot_text(&history);
        assert!(history_text.contains("two"));
        assert!(!history_text.contains("five"));

        assert!(terminal.scroll_to_bottom().expect("scroll bottom"));
        let bottom = terminal.snapshot().expect("snapshot restored bottom");
        assert!(super::snapshot_text(&bottom).contains("five"));
    }

    #[test]
    fn alternate_screen_scrolls_as_cursor_keys() {
        let mut terminal = Terminal::new(8, 3).expect("create terminal");
        terminal
            .feed(b"\x1b[?1049h")
            .expect("enter alternate screen");

        let scroll = terminal
            .scroll(ScrollInput {
                delta_rows: -2,
                delta_columns: 0,
                pointer_x: 0.0,
                pointer_y: 0.0,
                viewport_width: 80,
                viewport_height: 60,
                cell_width: 10,
                cell_height: 20,
                modifiers: 0,
            })
            .expect("scroll alternate screen");

        assert!(!scroll.viewport_changed);
        assert_eq!(scroll.input, b"\x1b[A\x1b[A");
    }

    #[test]
    fn mouse_tracking_scrolls_as_mouse_reports() {
        let mut terminal = Terminal::new(8, 3).expect("create terminal");
        terminal
            .feed(b"\x1b[?1000h\x1b[?1006h")
            .expect("enable mouse reporting");

        let scroll = terminal
            .scroll(ScrollInput {
                delta_rows: -1,
                delta_columns: -1,
                pointer_x: 25.0,
                pointer_y: 30.0,
                viewport_width: 80,
                viewport_height: 60,
                cell_width: 10,
                cell_height: 20,
                modifiers: 0,
            })
            .expect("scroll mouse-aware terminal");

        assert!(!scroll.viewport_changed);
        assert_eq!(scroll.input, b"\x1b[<64;3;2M\x1b[<66;3;2M");
    }

    #[test]
    fn unbounded_scroll_output_is_rejected_before_allocation() {
        let mut terminal = Terminal::new(8, 3).expect("create terminal");

        let error = terminal
            .scroll(ScrollInput {
                delta_rows: u32::MAX as isize,
                delta_columns: 0,
                pointer_x: 0.0,
                pointer_y: 0.0,
                viewport_width: 80,
                viewport_height: 60,
                cell_width: 10,
                cell_height: 20,
                modifiers: 0,
            })
            .expect_err("reject an unbounded scroll event");

        assert_eq!(error, "terminal scroll output is too large");
    }

    #[test]
    fn snapshots_report_terminal_titles_from_ghostty() {
        let mut terminal = Terminal::new(8, 3).expect("create terminal");
        terminal
            .feed(b"\x1b]0;Codex Settings\x07")
            .expect("set terminal title");

        let snapshot = terminal.snapshot().expect("snapshot terminal title");
        assert_eq!(snapshot.title.as_deref(), Some("Codex Settings"));
    }

    #[test]
    fn snapshots_report_decscusr_cursor_shapes() {
        let mut terminal = Terminal::new(8, 3).expect("create terminal");

        assert_eq!(
            terminal
                .snapshot()
                .expect("snapshot default cursor")
                .cursor_shape,
            CursorShape::Block
        );

        for (sequence, expected_shape, expected_blinking) in [
            (b"\x1b[1 q".as_slice(), CursorShape::Block, true),
            (b"\x1b[2 q".as_slice(), CursorShape::Block, false),
            (b"\x1b[3 q".as_slice(), CursorShape::Underline, true),
            (b"\x1b[4 q".as_slice(), CursorShape::Underline, false),
            (b"\x1b[5 q".as_slice(), CursorShape::Bar, true),
            (b"\x1b[6 q".as_slice(), CursorShape::Bar, false),
        ] {
            terminal.feed(sequence).expect("set cursor shape");
            let snapshot = terminal.snapshot().expect("snapshot cursor shape");
            assert_eq!(snapshot.cursor_shape, expected_shape);
            assert_eq!(snapshot.cursor_blinking, expected_blinking);
        }
    }

    #[test]
    fn snapshots_report_when_the_cursor_is_on_a_wide_glyph_tail() {
        let mut terminal = Terminal::new(8, 3).expect("create terminal");
        terminal
            .feed("界\x1b[D".as_bytes())
            .expect("position cursor on wide tail");

        let snapshot = terminal.snapshot().expect("snapshot wide-tail cursor");
        assert_eq!(snapshot.cursor, Some((1, 0)));
        assert!(snapshot.cursor_wide_tail);
    }

    #[cfg(feature = "gui")]
    #[test]
    fn configured_cursor_shape_is_the_default_without_overriding_decscusr() {
        let mut terminal = Terminal::new(8, 3).expect("create terminal");
        terminal
            .set_default_cursor_shape(CursorShape::Bar)
            .expect("set beam default");
        terminal
            .set_default_cursor_blink(true)
            .expect("enable default cursor blink");
        let default_snapshot = terminal.snapshot().expect("snapshot beam default");
        assert_eq!(default_snapshot.cursor_shape, CursorShape::Bar);
        assert!(default_snapshot.cursor_blinking);

        terminal.feed(b"\x1b[2 q").expect("set explicit block");
        terminal
            .set_default_cursor_shape(CursorShape::Underline)
            .expect("change configured default");
        let explicit_snapshot = terminal.snapshot().expect("snapshot explicit cursor");
        assert_eq!(explicit_snapshot.cursor_shape, CursorShape::Block);
        assert!(!explicit_snapshot.cursor_blinking);

        terminal.feed(b"\x1b[0 q").expect("reset cursor style");
        let reset_snapshot = terminal.snapshot().expect("snapshot reset cursor");
        assert_eq!(reset_snapshot.cursor_shape, CursorShape::Underline);
        assert!(reset_snapshot.cursor_blinking);
    }

    #[test]
    fn reverse_video_is_reported_as_an_explicit_background() {
        let mut terminal = Terminal::new(4, 1).expect("create terminal");
        terminal
            .feed(b"\x1b[7mX\x1b[0mY")
            .expect("feed inverse output");

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

    #[test]
    fn paste_encoding_uses_ghostty_terminal_modes() {
        let mut terminal = Terminal::new(8, 3).expect("create terminal");

        assert_eq!(
            terminal
                .encode_paste("plain\nUnicode: 雪".as_bytes())
                .expect("encode ordinary paste"),
            "plain\rUnicode: 雪".as_bytes()
        );

        terminal
            .feed(b"\x1b[?2004h")
            .expect("enable bracketed paste");
        assert_eq!(
            terminal
                .encode_paste("line one\nline two".as_bytes())
                .expect("encode bracketed paste"),
            b"\x1b[200~line one\nline two\x1b[201~"
        );
    }

    #[test]
    fn key_encoding_uses_ghostty_terminal_modes() {
        let mut terminal = Terminal::new(80, 24).expect("create terminal");
        let left = KeyInput {
            key: "left".into(),
            text: None,
            modifiers: 0,
        };

        assert_eq!(terminal.encode_key(&left).expect("encode left"), b"\x1b[D");

        terminal
            .feed(b"\x1b[?1h")
            .expect("enable application cursor keys");
        assert_eq!(
            terminal.encode_key(&left).expect("encode application left"),
            b"\x1bOD"
        );
    }

    #[test]
    fn key_encoding_preserves_alt_word_controls() {
        let mut terminal = Terminal::new(80, 24).expect("create terminal");

        for (key, expected) in [
            ("left", b"\x1b[1;3D".as_slice()),
            ("right", b"\x1b[1;3C".as_slice()),
            ("backspace", b"\x1b\x7f".as_slice()),
        ] {
            assert_eq!(
                terminal
                    .encode_key(&KeyInput {
                        key: key.into(),
                        text: None,
                        modifiers: 1 << 2,
                    })
                    .expect("encode Alt key"),
                expected
            );
        }
    }

    #[test]
    fn double_clicking_a_url_selects_only_the_matched_range() {
        let mut terminal = Terminal::new(80, 24).expect("create terminal");
        terminal
            .feed(b"visit https://google.com: now")
            .expect("feed URL");

        for event_type in [SelectionEventType::Press, SelectionEventType::Release] {
            terminal
                .selection_event(SelectionInput {
                    event_type,
                    click_count: 2,
                    x: 16,
                    y: 0,
                    pointer_x: 165.0,
                    pointer_y: 10.0,
                    columns: 80,
                    cell_width: 10,
                    padding_left: 0,
                    screen_height: 480,
                })
                .expect("select URL");
        }

        assert_eq!(
            terminal
                .snapshot()
                .expect("snapshot selection")
                .selection_text,
            Some("https://google.com".into())
        );
    }

    #[test]
    fn double_clicking_a_soft_wrapped_url_selects_the_logical_link() {
        let mut terminal = Terminal::new(12, 4).expect("create terminal");
        terminal
            .feed(b"go https://google.com: now")
            .expect("feed wrapped URL");

        for event_type in [SelectionEventType::Press, SelectionEventType::Release] {
            terminal
                .selection_event(SelectionInput {
                    event_type,
                    click_count: 2,
                    x: 3,
                    y: 1,
                    pointer_x: 35.0,
                    pointer_y: 30.0,
                    columns: 12,
                    cell_width: 10,
                    padding_left: 0,
                    screen_height: 80,
                })
                .expect("select wrapped URL");
        }

        assert_eq!(
            terminal
                .snapshot()
                .expect("snapshot wrapped selection")
                .selection_text,
            Some("https://google.com".into())
        );
    }

    #[test]
    fn snapshots_report_interactive_and_full_screen_modes() {
        let mut terminal = Terminal::new(8, 3).expect("create terminal");

        let initial = terminal.snapshot().expect("snapshot initial modes");
        assert!(!initial.bracketed_paste);
        assert!(!initial.alternate_screen);

        terminal
            .feed(b"\x1b[?2004h\x1b[?1049h")
            .expect("enable terminal modes");
        let active = terminal.snapshot().expect("snapshot enabled modes");
        assert!(active.bracketed_paste);
        assert!(active.alternate_screen);

        terminal
            .feed(b"\x1b[?2004l\x1b[?1049l")
            .expect("disable terminal modes");
        let inactive = terminal.snapshot().expect("snapshot disabled modes");
        assert!(!inactive.bracketed_paste);
        assert!(!inactive.alternate_screen);
    }

    #[test]
    fn paste_encoding_sanitizes_control_bytes_through_ghostty() {
        let mut terminal = Terminal::new(8, 3).expect("create terminal");
        terminal
            .feed(b"\x1b[?2004h")
            .expect("enable bracketed paste");

        assert_eq!(
            terminal
                .encode_paste(b"before\x1b[201~after\0")
                .expect("encode unsafe paste"),
            b"\x1b[200~before [201~after \x1b[201~"
        );
    }
}
