use crate::{
    CoreClient, TerminalLifecycle, TerminalSessionId, TerminalSize, TerminalSnapshot,
    core_driver::{CoreDriver, DriverUpdate},
    terminal_frame::{FrameRow, TerminalFrame},
    terminal_grid::{CellMetrics, GridDimensions, fixed_cell_glyph_x, measured_cell_height},
};
use gpui::{
    AnyWindowHandle, App, Bounds, Context, FocusHandle, IntoElement, KeyDownEvent, Keystroke,
    Pixels, Point, Render, ShapedLine, SharedString, Task, TextRun, Window, WindowBounds,
    WindowOptions, canvas, div, fill, font, point, prelude::*, px, rgb, size,
};
#[cfg(windows)]
use std::time::Duration;

const TERMINAL_PADDING_PX: f32 = 12.0;
const DEFAULT_FONT_SIZE_PX: f32 = 14.0;

pub(crate) fn open_terminal_window(cx: &mut App) -> Result<AnyWindowHandle, String> {
    let terminal_font = TerminalFont::resolve(cx)?;
    let core = CoreClient::connect_or_spawn()?;
    let (driver, mut projection) = CoreDriver::start(core)?;
    let terminal_session_id = projection
        .hierarchy
        .terminal_sessions
        .first()
        .map(|session| session.id)
        .ok_or_else(|| "the initial hierarchy has no Terminal Session".to_string())?;
    let snapshot = projection
        .terminals
        .remove(&terminal_session_id)
        .ok_or_else(|| "the initial Terminal Session has no rendered snapshot".to_string())?;
    let terminal_error = lifecycle_message(&snapshot.lifecycle);
    let bounds = Bounds::centered(None, size(px(900.), px(560.)), cx);
    let window = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            move |window, cx| {
                let focus = cx.focus_handle();
                focus.focus(window, cx);
                let view = cx.new(|_| TerminalView {
                    driver,
                    terminal_session_id,
                    snapshot,
                    focus,
                    refresh_task: Task::ready(()),
                    terminal_error,
                    terminal_font,
                    requested_size: None,
                });
                view.update(cx, |view, cx| {
                    view.start_refresh_task(cx);
                    #[cfg(windows)]
                    view.start_windows_probe(cx);
                });
                view
            },
        )
        .map_err(|error| format!("open GPUI window: {error}"))?;

    window
        .update(cx, |_view, _window, cx| cx.activate(true))
        .map_err(|error| format!("activate GPUI window: {error}"))?;
    Ok(window.into())
}

struct TerminalView {
    driver: CoreDriver,
    terminal_session_id: TerminalSessionId,
    snapshot: TerminalSnapshot,
    focus: FocusHandle,
    refresh_task: Task<()>,
    terminal_error: Option<String>,
    terminal_font: TerminalFont,
    requested_size: Option<TerminalSize>,
}

#[derive(Clone)]
struct TerminalFont {
    family: SharedString,
    size: Pixels,
    cells: CellMetrics,
}

impl TerminalFont {
    fn resolve(cx: &App) -> Result<Self, String> {
        let font_size = std::env::var("AGENT_TERMINAL_FONT_SIZE")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|size| (8.0..=48.0).contains(size))
            .unwrap_or(DEFAULT_FONT_SIZE_PX);
        let size = px(font_size);
        let requested = std::env::var("AGENT_TERMINAL_FONT")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let available = cx.text_system().all_font_names();
        let family: SharedString = requested
            .filter(|candidate| {
                font_is_available(candidate, &available) && font_is_fixed_pitch(candidate, size, cx)
            })
            .or_else(|| {
                terminal_font_candidates()
                    .iter()
                    .copied()
                    .find(|candidate| {
                        font_is_available(candidate, &available)
                            && font_is_fixed_pitch(candidate, size, cx)
                    })
                    .map(str::to_owned)
            })
            .or_else(|| {
                available
                    .iter()
                    .find(|candidate| font_is_fixed_pitch(candidate, size, cx))
                    .cloned()
            })
            .ok_or_else(|| {
                "no installed fixed-pitch font is available; set AGENT_TERMINAL_FONT to an installed monospace family"
                    .to_owned()
            })?
            .into();
        let font_id = cx.text_system().resolve_font(&font(family.clone()));
        let advance = cx
            .text_system()
            .advance(font_id, size, '0')
            .map(|advance| f32::from(advance.width))
            .unwrap_or(font_size * 0.6);
        let cell_width = advance.ceil().max(1.0) as u16;
        let ascent = f32::from(cx.text_system().ascent(font_id, size));
        let descent = f32::from(cx.text_system().descent(font_id, size));
        let cell_height = measured_cell_height(font_size, ascent, descent);

        Ok(Self {
            family,
            size,
            cells: CellMetrics::new(cell_width, cell_height),
        })
    }
}

fn font_is_available(candidate: &str, available: &[String]) -> bool {
    available
        .iter()
        .any(|font| font.eq_ignore_ascii_case(candidate))
}

fn font_is_fixed_pitch(candidate: &str, size: Pixels, cx: &App) -> bool {
    let font_id = cx.text_system().resolve_font(&font(candidate.to_owned()));
    let advances = ['i', 'W', '0'].map(|character| {
        cx.text_system()
            .advance(font_id, size, character)
            .map(|advance| f32::from(advance.width))
    });
    match advances {
        [Ok(first), Ok(second), Ok(third)] => {
            (first - second).abs() < 0.01 && (first - third).abs() < 0.01
        }
        _ => false,
    }
}

#[cfg(target_os = "macos")]
fn terminal_font_candidates() -> &'static [&'static str] {
    &["Menlo", "SF Mono", "Monaco"]
}

#[cfg(windows)]
fn terminal_font_candidates() -> &'static [&'static str] {
    &["Cascadia Mono", "Consolas", "Courier New"]
}

#[cfg(not(any(target_os = "macos", windows)))]
fn terminal_font_candidates() -> &'static [&'static str] {
    &["DejaVu Sans Mono", "Liberation Mono", "Noto Sans Mono"]
}

impl TerminalView {
    #[cfg(windows)]
    fn start_windows_probe(&mut self, cx: &mut Context<Self>) {
        let timer = cx.background_executor().timer(Duration::from_millis(750));
        cx.spawn(async move |this, cx| {
            timer.await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |view, _cx| {
                    if let Err(error) = view.driver.input_to(
                        view.terminal_session_id,
                        b"echo WINDOWS_CONPTY_LIVE\r".to_vec(),
                    ) {
                        view.terminal_error = Some(error);
                    }
                });
            }
        })
        .detach();
    }

    fn start_refresh_task(&mut self, cx: &mut Context<Self>) {
        let updates = self.driver.updates();
        self.refresh_task = cx.spawn(async move |this, cx| {
            while let Some(update) = updates.next().await {
                let Some(this) = this.upgrade() else {
                    break;
                };
                this.update(cx, move |view, cx| {
                    match update {
                        DriverUpdate::Terminal {
                            terminal_session_id,
                            snapshot,
                        } if terminal_session_id == view.terminal_session_id => {
                            view.terminal_error = lifecycle_message(&snapshot.lifecycle);
                            view.snapshot = snapshot;
                        }
                        DriverUpdate::Projection(mut projection) => {
                            if let Some(terminal_session_id) = projection
                                .hierarchy
                                .terminal_sessions
                                .first()
                                .map(|session| session.id)
                                && let Some(snapshot) =
                                    projection.terminals.remove(&terminal_session_id)
                            {
                                view.terminal_session_id = terminal_session_id;
                                view.terminal_error = lifecycle_message(&snapshot.lifecycle);
                                view.snapshot = snapshot;
                                view.requested_size = None;
                            }
                        }
                        DriverUpdate::Terminal { .. } | DriverUpdate::Hierarchy(_) => {}
                        DriverUpdate::Error(error) => view.terminal_error = Some(error),
                    }
                    cx.notify();
                });
            }
        });
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if let Some(bytes) = terminal_input_bytes(&event.keystroke) {
            if let Err(error) = self.driver.input_to(self.terminal_session_id, bytes) {
                self.terminal_error = Some(error);
                cx.notify();
            }
            cx.stop_propagation();
        }
    }
}

fn terminal_input_bytes(key: &Keystroke) -> Option<Vec<u8>> {
    if key.modifiers.control && key.key.len() == 1 {
        let byte = key.key.as_bytes()[0].to_ascii_uppercase();
        (b'@'..=b'_').contains(&byte).then(|| vec![byte - b'@'])
    } else if key.modifiers.platform || key.modifiers.alt {
        None
    } else {
        match key.key.as_str() {
            "enter" => Some(vec![b'\r']),
            "space" => Some(vec![b' ']),
            "backspace" => Some(vec![0x7f]),
            "tab" => Some(vec![b'\t']),
            "escape" => Some(vec![0x1b]),
            "up" => Some(b"\x1b[A".to_vec()),
            "down" => Some(b"\x1b[B".to_vec()),
            "right" => Some(b"\x1b[C".to_vec()),
            "left" => Some(b"\x1b[D".to_vec()),
            _ => key.key_char.as_ref().map(|text| text.as_bytes().to_vec()),
        }
    }
}

fn lifecycle_message(lifecycle: &TerminalLifecycle) -> Option<String> {
    match lifecycle {
        TerminalLifecycle::Running => None,
        TerminalLifecycle::Exited => Some("Terminal process exited".into()),
        TerminalLifecycle::Failed(error) => Some(format!("Terminal process failed: {error}")),
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let dimensions = GridDimensions::fit(
            f32::from(viewport.width),
            f32::from(viewport.height),
            TERMINAL_PADDING_PX,
            self.terminal_font.cells,
        );
        let desired_size = TerminalSize::new(
            dimensions.cols,
            dimensions.rows,
            self.terminal_font.cells.width_px,
            self.terminal_font.cells.height_px,
        );
        if self.requested_size != Some(desired_size) {
            match self
                .driver
                .resize_terminal(self.terminal_session_id, desired_size)
            {
                Ok(()) => self.requested_size = Some(desired_size),
                Err(error) => self.terminal_error = Some(error),
            }
        }

        let frame = TerminalFrame::from_snapshot(&self.snapshot);
        let default_bg = color(self.snapshot.default_bg);
        let terminal_font = self.terminal_font.clone();
        let paint_font = terminal_font.clone();
        let shape_frame = frame.clone();
        let terminal_canvas = canvas(
            move |_bounds, window, _cx| {
                let font = font(terminal_font.family.clone());
                shape_frame
                    .rows
                    .iter()
                    .map(|row| {
                        let runs = row
                            .runs
                            .iter()
                            .map(|run| TextRun {
                                len: run.len,
                                font: font.clone(),
                                color: color(run.color).into(),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            })
                            .collect::<Vec<_>>();
                        window.text_system().shape_line(
                            row.text.clone().into(),
                            terminal_font.size,
                            &runs,
                            None,
                        )
                    })
                    .collect::<Vec<_>>()
            },
            move |bounds, lines, window, _cx| {
                for background in &frame.backgrounds {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(
                                bounds.left()
                                    + px(f32::from(background.x)
                                        * f32::from(paint_font.cells.width_px)),
                                bounds.top()
                                    + px(f32::from(background.y)
                                        * f32::from(paint_font.cells.height_px)),
                            ),
                            size(
                                px(f32::from(background.width)
                                    * f32::from(paint_font.cells.width_px)),
                                px(f32::from(paint_font.cells.height_px)),
                            ),
                        ),
                        color(background.color),
                    ));
                }
                for (y, line) in lines.iter().enumerate() {
                    let _ = paint_fixed_cell_line(
                        &frame.rows[y],
                        line,
                        point(
                            bounds.left(),
                            bounds.top() + px(y as f32 * f32::from(paint_font.cells.height_px)),
                        ),
                        px(f32::from(paint_font.cells.height_px)),
                        paint_font.cells,
                        window,
                    );
                }
                if let Some(cursor) = frame.cursor_overlay {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(
                                bounds.left()
                                    + px(f32::from(cursor.x)
                                        * f32::from(paint_font.cells.width_px)),
                                bounds.top()
                                    + px(f32::from(cursor.y)
                                        * f32::from(paint_font.cells.height_px)),
                            ),
                            size(
                                px(f32::from(paint_font.cells.width_px)),
                                px(f32::from(paint_font.cells.height_px)),
                            ),
                        ),
                        color(cursor.color),
                    ));
                }
            },
        )
        .size_full();

        div()
            .id("terminal")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|view, event, _window, cx| view.on_key_down(event, cx)))
            .size_full()
            .overflow_hidden()
            .bg(default_bg)
            .p(px(TERMINAL_PADDING_PX))
            .font_family(self.terminal_font.family.clone())
            .text_size(self.terminal_font.size)
            .child(terminal_canvas)
            .when_some(self.terminal_error.clone(), |this, error| {
                this.child(
                    div()
                        .absolute()
                        .bottom(px(8.))
                        .left(px(12.))
                        .text_color(rgb(0xff6b6b))
                        .child(error),
                )
            })
    }
}

fn paint_fixed_cell_line(
    row: &FrameRow,
    line: &ShapedLine,
    origin: Point<Pixels>,
    line_height: Pixels,
    cells: CellMetrics,
    window: &mut Window,
) -> gpui::Result<()> {
    // Shape the row once, but anchor each cluster to Ghostty's cell grid. Keeping the
    // cluster's internal offsets preserves combining marks and wide/fallback glyphs.
    let mut natural_cell_x = vec![None; row.glyph_cells.len()];
    for run in &line.runs {
        for glyph in &run.glyphs {
            if let Some(cell_index) = row.glyph_cell_index(glyph.index) {
                natural_cell_x[cell_index].get_or_insert(f32::from(glyph.position.x));
            }
        }
    }

    let padding_top = (line_height - line.ascent - line.descent) / 2.;
    let baseline_y = origin.y + padding_top + line.ascent;
    for run in &line.runs {
        for glyph in &run.glyphs {
            let Some(cell_index) = row.glyph_cell_index(glyph.index) else {
                continue;
            };
            let glyph_cell = &row.glyph_cells[cell_index];
            let Some(natural_cell_x) = natural_cell_x[cell_index] else {
                continue;
            };
            let glyph_x = fixed_cell_glyph_x(
                glyph_cell.x,
                cells.width_px,
                f32::from(glyph.position.x),
                natural_cell_x,
            );
            let glyph_origin = point(origin.x + px(glyph_x), baseline_y);
            if glyph.is_emoji {
                window.paint_emoji(glyph_origin, run.font_id, glyph.id, line.font_size)?;
            } else {
                window.paint_glyph(
                    glyph_origin,
                    run.font_id,
                    glyph.id,
                    line.font_size,
                    color(glyph_cell.color).into(),
                )?;
            }
        }
    }
    Ok(())
}

fn color(rgb_bytes: [u8; 3]) -> gpui::Rgba {
    rgb((u32::from(rgb_bytes[0]) << 16) | (u32::from(rgb_bytes[1]) << 8) | u32::from(rgb_bytes[2]))
}

#[cfg(test)]
mod tests {
    use super::terminal_input_bytes;
    use gpui::Keystroke;

    #[test]
    fn named_space_key_maps_to_ascii_space_without_a_key_char() {
        let key = Keystroke {
            key: "space".into(),
            key_char: None,
            ..Default::default()
        };

        assert_eq!(terminal_input_bytes(&key), Some(vec![b' ']));
    }
}
