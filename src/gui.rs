use crate::{
    CoreClient, TerminalLifecycle, TerminalSize, TerminalSnapshot,
    terminal_grid::{CellMetrics, GridDimensions, cell_offset, measured_cell_height},
};
use gpui::{
    App, Application, Bounds, Context, FocusHandle, IntoElement, KeyDownEvent, Pixels, Render,
    SharedString, Task, Window, WindowBounds, WindowOptions, div, font, prelude::*, px, rgb, size,
};
use std::time::Duration;

const TERMINAL_PADDING_PX: f32 = 12.0;
const DEFAULT_FONT_SIZE_PX: f32 = 14.0;

pub fn run() {
    Application::new().run(|cx: &mut App| {
        let terminal_font =
            TerminalFont::resolve(cx).expect("resolve an installed fixed-pitch terminal font");
        let mut core = CoreClient::connect_or_spawn().expect("attach to Resident Core");
        let snapshot = core.snapshot().expect("snapshot Terminal Session");
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
                    focus.focus(window);
                    let view = cx.new(|_| TerminalView {
                        core,
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
            .expect("open GPUI window");

        window
            .update(cx, |_view, _window, cx| cx.activate(true))
            .expect("activate GPUI window");
    });
}

struct TerminalView {
    core: CoreClient,
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
                    if let Err(error) = view.core.input(b"echo WINDOWS_CONPTY_LIVE\r") {
                        view.terminal_error = Some(error);
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    fn start_refresh_task(&mut self, cx: &mut Context<Self>) {
        let executor = cx.background_executor().clone();
        self.refresh_task = cx.spawn(async move |this, cx| {
            loop {
                executor.timer(Duration::from_millis(16)).await;
                let Some(this) = this.upgrade() else {
                    break;
                };
                if this
                    .update(cx, |view, cx| {
                        match view.core.snapshot_since(view.snapshot.revision) {
                            Ok(Some(snapshot)) => {
                                view.terminal_error = lifecycle_message(&snapshot.lifecycle);
                                view.snapshot = snapshot;
                                cx.notify();
                            }
                            Ok(None) => {}
                            Err(error) => {
                                view.terminal_error = Some(error);
                                cx.notify();
                            }
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        let bytes = if key.modifiers.control && key.key.len() == 1 {
            let byte = key.key.as_bytes()[0].to_ascii_uppercase();
            (b'@'..=b'_').contains(&byte).then(|| vec![byte - b'@'])
        } else if key.modifiers.platform || key.modifiers.alt {
            None
        } else {
            match key.key.as_str() {
                "enter" => Some(vec![b'\r']),
                "backspace" => Some(vec![0x7f]),
                "tab" => Some(vec![b'\t']),
                "escape" => Some(vec![0x1b]),
                "up" => Some(b"\x1b[A".to_vec()),
                "down" => Some(b"\x1b[B".to_vec()),
                "right" => Some(b"\x1b[C".to_vec()),
                "left" => Some(b"\x1b[D".to_vec()),
                _ => key.key_char.as_ref().map(|text| text.as_bytes().to_vec()),
            }
        };

        if let Some(bytes) = bytes {
            if let Err(error) = self.core.input(&bytes) {
                self.terminal_error = Some(error);
                cx.notify();
            }
            cx.stop_propagation();
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
            match self.core.resize(desired_size) {
                Ok(()) => self.requested_size = Some(desired_size),
                Err(error) => self.terminal_error = Some(error),
            }
        }

        let snapshot = &self.snapshot;
        let cursor = snapshot.cursor;
        let default_bg = color(snapshot.default_bg);
        let mut cells_by_offset =
            vec![None; usize::from(snapshot.cols) * usize::from(snapshot.rows)];
        for cell in &snapshot.cells {
            if let Some(offset) = cell_offset(snapshot.cols, cell.x, cell.y)
                && let Some(slot) = cells_by_offset.get_mut(offset)
            {
                *slot = Some(cell);
            }
        }

        let rows = (0..snapshot.rows).map(|y| {
            let cells = (0..snapshot.cols).map(|x| {
                let cell = cell_offset(snapshot.cols, x, y)
                    .and_then(|offset| cells_by_offset.get(offset))
                    .copied()
                    .flatten();
                let text = cell
                    .map(|cell| cell.text.as_str())
                    .filter(|text| !text.is_empty())
                    .unwrap_or(" ")
                    .to_owned();
                let foreground = cell.map(|cell| cell.fg).unwrap_or(snapshot.default_fg);
                let background = if cursor == Some((x, y)) {
                    foreground
                } else {
                    cell.map(|cell| cell.bg).unwrap_or(snapshot.default_bg)
                };
                let foreground = if cursor == Some((x, y)) {
                    snapshot.default_bg
                } else {
                    foreground
                };
                let width = cell.map(|cell| cell.width).unwrap_or(1);

                div()
                    .w(px(
                        f32::from(self.terminal_font.cells.width_px) * f32::from(width)
                    ))
                    .h(px(f32::from(self.terminal_font.cells.height_px)))
                    .flex_none()
                    .overflow_hidden()
                    .bg(color(background))
                    .text_color(color(foreground))
                    .text_size(self.terminal_font.size)
                    .line_height(px(f32::from(self.terminal_font.cells.height_px)))
                    .font_family(self.terminal_font.family.clone())
                    .child(text)
            });
            div()
                .h(px(f32::from(self.terminal_font.cells.height_px)))
                .flex()
                .flex_row()
                .children(cells)
        });

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
            .children(rows)
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

fn color(rgb_bytes: [u8; 3]) -> gpui::Rgba {
    rgb((u32::from(rgb_bytes[0]) << 16) | (u32::from(rgb_bytes[1]) << 8) | u32::from(rgb_bytes[2]))
}
