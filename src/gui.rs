use crate::{
    CoreClient, CoreCommand, CoreSnapshot, CreatedResource, PaneId, PaneLayout, SpaceId, SplitAxis,
    SplitPlacement, SplitRatio, TabId, TerminalLifecycle, TerminalSessionId, TerminalSize,
    TerminalSnapshot,
    core_driver::{CoreDriver, CoreProjection, DriverUpdate},
    terminal_frame::{FrameRow, TerminalFrame},
    terminal_grid::{CellMetrics, GridDimensions, fixed_cell_glyph_x, measured_cell_height},
};
use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, Context, FocusHandle, IntoElement, KeyDownEvent,
    Keystroke, Pixels, Point, Render, ShapedLine, SharedString, Task, TextRun, Window,
    WindowBounds, WindowOptions, canvas, div, fill, font, point, prelude::*, px, rgb, size,
};
use std::collections::{HashMap, HashSet};
#[cfg(windows)]
use std::time::Duration;

const TERMINAL_PADDING_PX: f32 = 10.0;
const SIDEBAR_WIDTH_PX: f32 = 188.0;
const TAB_BAR_HEIGHT_PX: f32 = 38.0;
const SPLIT_GAP_PX: f32 = 1.0;
const DEFAULT_FONT_SIZE_PX: f32 = 14.0;

pub(crate) fn open_terminal_window(cx: &mut App) -> Result<AnyWindowHandle, String> {
    let terminal_font = TerminalFont::resolve(cx)?;
    let core = CoreClient::connect_or_spawn()?;
    let (driver, projection) = CoreDriver::start(core)?;
    let selection = UiSelection::initial(&projection.hierarchy);
    let terminal_errors = projection
        .terminals
        .iter()
        .filter_map(|(&terminal_session_id, snapshot)| {
            lifecycle_message(&snapshot.lifecycle).map(|message| (terminal_session_id, message))
        })
        .collect();
    let bounds = Bounds::centered(None, size(px(1080.), px(680.)), cx);
    let window = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            move |window, cx| {
                let focus = cx.focus_handle();
                focus.focus(window, cx);
                let view = cx.new(|_| MultiplexerView {
                    driver,
                    hierarchy: projection.hierarchy,
                    terminals: projection.terminals,
                    selection,
                    focus,
                    refresh_task: Task::ready(()),
                    terminal_errors,
                    global_error: None,
                    terminal_font,
                    requested_sizes: HashMap::new(),
                    pending_close: None,
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

struct MultiplexerView {
    driver: CoreDriver,
    hierarchy: CoreSnapshot,
    terminals: HashMap<TerminalSessionId, TerminalSnapshot>,
    selection: UiSelection,
    focus: FocusHandle,
    refresh_task: Task<()>,
    terminal_errors: HashMap<TerminalSessionId, String>,
    global_error: Option<String>,
    terminal_font: TerminalFont,
    requested_sizes: HashMap<TerminalSessionId, TerminalSize>,
    pending_close: Option<PaneId>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct UiSelection {
    space_id: Option<SpaceId>,
    tab_id: Option<TabId>,
    pane_id: Option<PaneId>,
}

impl UiSelection {
    fn initial(hierarchy: &CoreSnapshot) -> Self {
        Self::default().normalized(hierarchy)
    }

    fn normalized(mut self, hierarchy: &CoreSnapshot) -> Self {
        let space = self
            .space_id
            .and_then(|space_id| hierarchy.spaces.iter().find(|space| space.id == space_id))
            .or_else(|| hierarchy.spaces.first());
        self.space_id = space.map(|space| space.id);
        let tab = space.and_then(|space| {
            self.tab_id
                .and_then(|tab_id| space.tabs.iter().find(|tab| tab.id == tab_id))
                .or_else(|| space.tabs.first())
        });
        self.tab_id = tab.map(|tab| tab.id);
        self.pane_id = tab.and_then(|tab| {
            self.pane_id
                .filter(|pane_id| layout_contains_pane(&tab.layout, *pane_id))
                .or_else(|| first_pane_id(&tab.layout))
        });
        self
    }
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

impl MultiplexerView {
    #[cfg(windows)]
    fn start_windows_probe(&mut self, cx: &mut Context<Self>) {
        let timer = cx.background_executor().timer(Duration::from_millis(750));
        cx.spawn(async move |this, cx| {
            timer.await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |view, _cx| {
                    if let Some(terminal_session_id) = view.focused_terminal_session_id()
                        && let Err(error) = view
                            .driver
                            .input_to(terminal_session_id, b"echo WINDOWS_CONPTY_LIVE\r".to_vec())
                    {
                        view.global_error = Some(error);
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
                    view.accept_driver_update(update);
                    cx.notify();
                });
            }
        });
    }

    fn accept_driver_update(&mut self, update: DriverUpdate) {
        match update {
            DriverUpdate::Projection(projection) => self.replace_projection(projection),
            DriverUpdate::Hierarchy(hierarchy) => {
                self.hierarchy = hierarchy;
                self.retain_live_terminals();
                self.selection = self.selection.normalized(&self.hierarchy);
                if self.pending_close.is_some_and(|pane_id| {
                    !self
                        .hierarchy
                        .spaces
                        .iter()
                        .flat_map(|space| &space.tabs)
                        .any(|tab| layout_contains_pane(&tab.layout, pane_id))
                }) {
                    self.pending_close = None;
                }
            }
            DriverUpdate::CommandAccepted(outcome) => {
                self.selection =
                    selection_for_created(self.selection, &outcome.created, &self.hierarchy);
                self.global_error = None;
            }
            DriverUpdate::Terminal {
                terminal_session_id,
                snapshot,
            } => accept_terminal_snapshot(
                &self.hierarchy,
                &mut self.terminals,
                &mut self.terminal_errors,
                terminal_session_id,
                snapshot,
            ),
            DriverUpdate::Error(error) => self.global_error = Some(error),
        }
    }

    fn replace_projection(&mut self, projection: CoreProjection) {
        self.hierarchy = projection.hierarchy;
        self.terminals = projection.terminals;
        self.terminal_errors = self
            .terminals
            .iter()
            .filter_map(|(&terminal_session_id, snapshot)| {
                lifecycle_message(&snapshot.lifecycle).map(|message| (terminal_session_id, message))
            })
            .collect();
        self.requested_sizes.clear();
        self.selection = self.selection.normalized(&self.hierarchy);
        self.pending_close = None;
    }

    fn retain_live_terminals(&mut self) {
        let terminal_ids = self
            .hierarchy
            .terminal_sessions
            .iter()
            .map(|session| session.id)
            .collect::<HashSet<_>>();
        self.terminals
            .retain(|terminal_session_id, _| terminal_ids.contains(terminal_session_id));
        self.terminal_errors
            .retain(|terminal_session_id, _| terminal_ids.contains(terminal_session_id));
        self.requested_sizes
            .retain(|terminal_session_id, _| terminal_ids.contains(terminal_session_id));
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(terminal_session_id) = self.focused_terminal_session_id() else {
            return;
        };
        if let Some(bytes) = terminal_input_bytes(&event.keystroke) {
            if let Err(error) = self.driver.input_to(terminal_session_id, bytes) {
                self.global_error = Some(error);
                cx.notify();
            }
            cx.stop_propagation();
        }
    }

    fn focused_terminal_session_id(&self) -> Option<TerminalSessionId> {
        let pane_id = self.selection.pane_id?;
        self.selected_tab()
            .and_then(|tab| terminal_for_pane(&tab.layout, pane_id))
    }

    fn selected_space(&self) -> Option<&crate::SpaceSnapshot> {
        let space_id = self.selection.space_id?;
        self.hierarchy
            .spaces
            .iter()
            .find(|space| space.id == space_id)
    }

    fn selected_tab(&self) -> Option<&crate::TabSnapshot> {
        let tab_id = self.selection.tab_id?;
        self.selected_space()?
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
    }

    fn select_space(&mut self, space_id: SpaceId, window: &mut Window, cx: &mut Context<Self>) {
        self.selection.space_id = Some(space_id);
        self.selection.tab_id = None;
        self.selection.pane_id = None;
        self.selection = self.selection.normalized(&self.hierarchy);
        self.pending_close = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn select_tab(&mut self, tab_id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        self.selection.tab_id = Some(tab_id);
        self.selection.pane_id = None;
        self.selection = self.selection.normalized(&self.hierarchy);
        self.pending_close = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn focus_pane(&mut self, pane_id: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        self.selection.pane_id = Some(pane_id);
        self.pending_close = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn submit_core_command(&mut self, command: CoreCommand, cx: &mut Context<Self>) {
        match self.driver.apply_core_command(command) {
            Ok(()) => self.global_error = None,
            Err(error) => self.global_error = Some(error),
        }
        cx.notify();
    }

    fn create_space(&mut self, cx: &mut Context<Self>) {
        match std::env::current_dir() {
            Ok(directory) => self.submit_core_command(
                CoreCommand::CreateSpace {
                    name: format!("Space {}", self.hierarchy.spaces.len() + 1),
                    directory,
                },
                cx,
            ),
            Err(error) => {
                self.global_error = Some(format!("locate current directory: {error}"));
                cx.notify();
            }
        }
    }

    fn create_tab(&mut self, cx: &mut Context<Self>) {
        let Some(space) = self.selected_space() else {
            return;
        };
        self.submit_core_command(
            CoreCommand::CreateTab {
                space_id: space.id,
                name: format!("Terminal {}", space.tabs.len() + 1),
            },
            cx,
        );
    }

    fn split_focused_pane(&mut self, axis: SplitAxis, cx: &mut Context<Self>) {
        let Some(pane_id) = self.selection.pane_id else {
            return;
        };
        self.submit_core_command(
            CoreCommand::SplitPane {
                pane_id,
                axis,
                placement: SplitPlacement::After,
                ratio: SplitRatio::EQUAL,
            },
            cx,
        );
    }

    fn request_close_focused_pane(&mut self, cx: &mut Context<Self>) {
        let Some(pane_id) = self.selection.pane_id else {
            return;
        };
        if self.pending_close == Some(pane_id) {
            self.pending_close = None;
            self.submit_core_command(CoreCommand::ClosePane { pane_id }, cx);
        } else {
            self.pending_close = Some(pane_id);
            cx.notify();
        }
    }

    fn resize_visible_terminals(&mut self, viewport: gpui::Size<Pixels>) {
        let Some(layout) = self.selected_tab().map(|tab| tab.layout.clone()) else {
            return;
        };
        let width = (f32::from(viewport.width) - SIDEBAR_WIDTH_PX).max(1.0);
        let height = (f32::from(viewport.height) - TAB_BAR_HEIGHT_PX).max(1.0);
        let mut panes = Vec::new();
        pane_extents(&layout, width, height, &mut panes);
        for (terminal_session_id, width, height) in panes {
            let dimensions =
                GridDimensions::fit(width, height, TERMINAL_PADDING_PX, self.terminal_font.cells);
            let size = TerminalSize::new(
                dimensions.cols,
                dimensions.rows,
                self.terminal_font.cells.width_px,
                self.terminal_font.cells.height_px,
            );
            if self.requested_sizes.get(&terminal_session_id) != Some(&size) {
                match self.driver.resize_terminal(terminal_session_id, size) {
                    Ok(()) => {
                        self.requested_sizes.insert(terminal_session_id, size);
                    }
                    Err(error) => self.global_error = Some(error),
                }
            }
        }
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut sidebar = div()
            .flex()
            .flex_col()
            .w(px(SIDEBAR_WIDTH_PX))
            .h_full()
            .flex_none()
            .bg(rgb(0x11151d))
            .border_r_1()
            .border_color(rgb(0x2b3240))
            .p_2()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_2()
                    .py_1()
                    .text_size(px(12.))
                    .text_color(rgb(0x8993a4))
                    .child("SPACES")
                    .child(
                        div()
                            .id("create-space")
                            .cursor_pointer()
                            .rounded_md()
                            .px_2()
                            .py_1()
                            .bg(rgb(0x273247))
                            .text_color(rgb(0xdbe6f5))
                            .on_click(
                                cx.listener(|view, _event, _window, cx| view.create_space(cx)),
                            )
                            .child("+"),
                    ),
            );
        for space in &self.hierarchy.spaces {
            let space_id = space.id;
            let selected = self.selection.space_id == Some(space_id);
            sidebar = sidebar.child(
                div()
                    .id(("space", space_id.as_u64()))
                    .cursor_pointer()
                    .rounded_md()
                    .px_2()
                    .py_2()
                    .text_size(px(13.))
                    .text_color(if selected {
                        rgb(0xf4f7fb)
                    } else {
                        rgb(0xb4bdca)
                    })
                    .when(selected, |this| this.bg(rgb(0x273247)))
                    .on_click(cx.listener(move |view, _event, window, cx| {
                        view.select_space(space_id, window, cx)
                    }))
                    .child(space.name.clone()),
            );
        }
        sidebar.into_any_element()
    }

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut tabs = div()
            .flex()
            .flex_row()
            .h(px(TAB_BAR_HEIGHT_PX))
            .w_full()
            .flex_none()
            .items_center()
            .bg(rgb(0x151a23))
            .border_b_1()
            .border_color(rgb(0x2b3240))
            .px_2()
            .gap_1();
        if let Some(space) = self.selected_space() {
            for tab in &space.tabs {
                let tab_id = tab.id;
                let selected = self.selection.tab_id == Some(tab_id);
                tabs = tabs.child(
                    div()
                        .id(("tab", tab_id.as_u64()))
                        .cursor_pointer()
                        .px_3()
                        .py_2()
                        .rounded_t_md()
                        .text_size(px(13.))
                        .text_color(if selected {
                            rgb(0xf4f7fb)
                        } else {
                            rgb(0x929cac)
                        })
                        .when(selected, |this| this.bg(rgb(0x0b0e13)))
                        .on_click(cx.listener(move |view, _event, window, cx| {
                            view.select_tab(tab_id, window, cx)
                        }))
                        .child(tab.name.clone()),
                );
            }
            tabs = tabs
                .child(
                    div()
                        .id("create-tab")
                        .cursor_pointer()
                        .rounded_md()
                        .px_2()
                        .py_1()
                        .text_color(rgb(0xb4bdca))
                        .on_click(cx.listener(|view, _event, _window, cx| view.create_tab(cx)))
                        .child("+"),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .id("split-horizontal")
                        .cursor_pointer()
                        .rounded_md()
                        .px_2()
                        .py_1()
                        .text_size(px(12.))
                        .text_color(rgb(0xb4bdca))
                        .on_click(cx.listener(|view, _event, _window, cx| {
                            view.split_focused_pane(SplitAxis::Horizontal, cx)
                        }))
                        .child("Split H"),
                )
                .child(
                    div()
                        .id("split-vertical")
                        .cursor_pointer()
                        .rounded_md()
                        .px_2()
                        .py_1()
                        .text_size(px(12.))
                        .text_color(rgb(0xb4bdca))
                        .on_click(cx.listener(|view, _event, _window, cx| {
                            view.split_focused_pane(SplitAxis::Vertical, cx)
                        }))
                        .child("Split V"),
                )
                .child(
                    div()
                        .id("close-pane")
                        .cursor_pointer()
                        .rounded_md()
                        .px_2()
                        .py_1()
                        .text_size(px(12.))
                        .text_color(if self.pending_close == self.selection.pane_id {
                            rgb(0xffb4b4)
                        } else {
                            rgb(0xb4bdca)
                        })
                        .on_click(cx.listener(|view, _event, _window, cx| {
                            view.request_close_focused_pane(cx)
                        }))
                        .child(if self.pending_close == self.selection.pane_id {
                            "Confirm close"
                        } else {
                            "Close"
                        }),
                );
        }
        tabs.into_any_element()
    }

    fn render_selected_layout(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.selected_tab() {
            Some(tab) => self.render_pane_layout(&tab.layout, cx),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgb(0x0b0e13))
                .text_color(rgb(0x8993a4))
                .child("No Spaces yet")
                .into_any_element(),
        }
    }

    fn render_pane_layout(&self, layout: &PaneLayout, cx: &mut Context<Self>) -> AnyElement {
        match layout {
            PaneLayout::Pane(pane) => self.render_terminal_pane(pane, cx),
            PaneLayout::Split(split) => {
                let first = self.render_pane_layout(&split.first, cx);
                let second = self.render_pane_layout(&split.second, cx);
                let first_grow = f32::from(split.ratio.parts_per_thousand());
                let second_grow = 1000.0 - first_grow;
                let first = match split.axis {
                    SplitAxis::Horizontal => div()
                        .h_full()
                        .min_w_0()
                        .flex_basis(px(0.))
                        .flex_grow(first_grow)
                        .child(first),
                    SplitAxis::Vertical => div()
                        .w_full()
                        .min_h_0()
                        .flex_basis(px(0.))
                        .flex_grow(first_grow)
                        .child(first),
                };
                div()
                    .flex()
                    .when(split.axis == SplitAxis::Horizontal, |this| this.flex_row())
                    .when(split.axis == SplitAxis::Vertical, |this| this.flex_col())
                    .size_full()
                    .gap(px(SPLIT_GAP_PX))
                    .bg(rgb(0x343c4a))
                    .child(first)
                    .child(
                        div()
                            .flex_basis(px(0.))
                            .flex_grow(second_grow)
                            .min_w_0()
                            .min_h_0()
                            .child(second),
                    )
                    .into_any_element()
            }
        }
    }

    fn render_terminal_pane(
        &self,
        pane: &crate::PaneSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pane_id = pane.id;
        let terminal_session_id = pane.terminal_session_id;
        let focused = self.selection.pane_id == Some(pane_id);
        let Some(snapshot) = self.terminals.get(&terminal_session_id) else {
            return div()
                .id(("pane", pane_id.as_u64()))
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgb(0x0b0e13))
                .text_color(rgb(0x8993a4))
                .child("Starting terminal…")
                .into_any_element();
        };
        let frame = TerminalFrame::from_snapshot(snapshot);
        let default_bg = color(snapshot.default_bg);
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
            .id(("pane", pane_id.as_u64()))
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .bg(default_bg)
            .border_1()
            .border_color(if focused { rgb(0x5b8def) } else { default_bg })
            .p(px(TERMINAL_PADDING_PX))
            .font_family(self.terminal_font.family.clone())
            .text_size(self.terminal_font.size)
            .on_click(
                cx.listener(move |view, _event, window, cx| view.focus_pane(pane_id, window, cx)),
            )
            .child(terminal_canvas)
            .when_some(
                self.terminal_errors.get(&terminal_session_id).cloned(),
                |this, error| {
                    this.child(
                        div()
                            .absolute()
                            .bottom(px(8.))
                            .left(px(12.))
                            .text_color(rgb(0xff6b6b))
                            .child(error),
                    )
                },
            )
            .into_any_element()
    }
}

fn accept_terminal_snapshot(
    hierarchy: &CoreSnapshot,
    terminals: &mut HashMap<TerminalSessionId, TerminalSnapshot>,
    terminal_errors: &mut HashMap<TerminalSessionId, String>,
    terminal_session_id: TerminalSessionId,
    snapshot: TerminalSnapshot,
) {
    if !hierarchy
        .terminal_sessions
        .iter()
        .any(|session| session.id == terminal_session_id)
    {
        return;
    }
    if let Some(message) = lifecycle_message(&snapshot.lifecycle) {
        terminal_errors.insert(terminal_session_id, message);
    } else {
        terminal_errors.remove(&terminal_session_id);
    }
    terminals.insert(terminal_session_id, snapshot);
}

impl Render for MultiplexerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.resize_visible_terminals(window.viewport_size());
        div()
            .id("multiplexer")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|view, event, _window, cx| view.on_key_down(event, cx)))
            .flex()
            .flex_row()
            .size_full()
            .overflow_hidden()
            .bg(rgb(0x0b0e13))
            .child(self.render_sidebar(cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.render_tab_bar(cx))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .child(self.render_selected_layout(cx)),
                    ),
            )
            .when_some(self.global_error.clone(), |this, error| {
                this.child(
                    div()
                        .absolute()
                        .right(px(12.))
                        .bottom(px(10.))
                        .rounded_md()
                        .bg(rgb(0x55262a))
                        .px_2()
                        .py_1()
                        .text_color(rgb(0xffb4b4))
                        .child(error),
                )
            })
    }
}

fn selection_for_created(
    current: UiSelection,
    created: &CreatedResource,
    hierarchy: &CoreSnapshot,
) -> UiSelection {
    let selected = match created {
        CreatedResource::None => current,
        CreatedResource::Space {
            space_id,
            tab_id,
            pane_id,
            ..
        } => UiSelection {
            space_id: Some(*space_id),
            tab_id: Some(*tab_id),
            pane_id: Some(*pane_id),
        },
        CreatedResource::Tab {
            tab_id, pane_id, ..
        } => {
            let space_id = hierarchy
                .spaces
                .iter()
                .find(|space| space.tabs.iter().any(|tab| tab.id == *tab_id))
                .map(|space| space.id);
            UiSelection {
                space_id,
                tab_id: Some(*tab_id),
                pane_id: Some(*pane_id),
            }
        }
        CreatedResource::Pane { pane_id, .. } => {
            let location = hierarchy.spaces.iter().find_map(|space| {
                space
                    .tabs
                    .iter()
                    .find(|tab| layout_contains_pane(&tab.layout, *pane_id))
                    .map(|tab| (space.id, tab.id))
            });
            UiSelection {
                space_id: location.map(|(space_id, _)| space_id),
                tab_id: location.map(|(_, tab_id)| tab_id),
                pane_id: Some(*pane_id),
            }
        }
    };
    selected.normalized(hierarchy)
}

fn pane_extents(
    layout: &PaneLayout,
    width: f32,
    height: f32,
    output: &mut Vec<(TerminalSessionId, f32, f32)>,
) {
    match layout {
        PaneLayout::Pane(pane) => output.push((pane.terminal_session_id, width, height)),
        PaneLayout::Split(split) => {
            let ratio = f32::from(split.ratio.parts_per_thousand()) / 1000.0;
            match split.axis {
                SplitAxis::Horizontal => {
                    let available = (width - SPLIT_GAP_PX).max(2.0);
                    pane_extents(&split.first, available * ratio, height, output);
                    pane_extents(&split.second, available * (1.0 - ratio), height, output);
                }
                SplitAxis::Vertical => {
                    let available = (height - SPLIT_GAP_PX).max(2.0);
                    pane_extents(&split.first, width, available * ratio, output);
                    pane_extents(&split.second, width, available * (1.0 - ratio), output);
                }
            }
        }
    }
}

fn first_pane_id(layout: &PaneLayout) -> Option<PaneId> {
    match layout {
        PaneLayout::Pane(pane) => Some(pane.id),
        PaneLayout::Split(split) => {
            first_pane_id(&split.first).or_else(|| first_pane_id(&split.second))
        }
    }
}

fn layout_contains_pane(layout: &PaneLayout, pane_id: PaneId) -> bool {
    terminal_for_pane(layout, pane_id).is_some()
}

fn terminal_for_pane(layout: &PaneLayout, pane_id: PaneId) -> Option<TerminalSessionId> {
    match layout {
        PaneLayout::Pane(pane) => (pane.id == pane_id).then_some(pane.terminal_session_id),
        PaneLayout::Split(split) => terminal_for_pane(&split.first, pane_id)
            .or_else(|| terminal_for_pane(&split.second, pane_id)),
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

fn paint_fixed_cell_line(
    row: &FrameRow,
    line: &ShapedLine,
    origin: Point<Pixels>,
    line_height: Pixels,
    cells: CellMetrics,
    window: &mut Window,
) -> gpui::Result<()> {
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
    use super::{
        UiSelection, accept_terminal_snapshot, pane_extents, selection_for_created,
        terminal_input_bytes,
    };
    use crate::{
        CoreCommand, CoreModel, CreatedResource, PaneLayout, SplitAxis, SplitPlacement, SplitRatio,
        TerminalLifecycle, TerminalSnapshot,
    };
    use gpui::Keystroke;
    use std::collections::HashMap;

    #[test]
    fn named_space_key_maps_to_ascii_space_without_a_key_char() {
        let key = Keystroke {
            key: "space".into(),
            key_char: None,
            ..Default::default()
        };

        assert_eq!(terminal_input_bytes(&key), Some(vec![b' ']));
    }

    #[test]
    fn selection_falls_back_to_valid_space_tab_and_pane_ids() {
        let directory = std::env::current_dir().expect("current directory");
        let mut model = CoreModel::new();
        let first = model
            .apply(
                0,
                CoreCommand::CreateSpace {
                    name: "First".into(),
                    directory: directory.clone(),
                },
            )
            .expect("create first Space");
        let second = model
            .apply(
                first.revision,
                CoreCommand::CreateSpace {
                    name: "Second".into(),
                    directory,
                },
            )
            .expect("create second Space");
        let selected = UiSelection::initial(&second.snapshot);

        assert_eq!(selected.space_id, Some(second.snapshot.spaces[0].id));
        assert_eq!(selected.tab_id, Some(second.snapshot.spaces[0].tabs[0].id));
        assert!(selected.pane_id.is_some());
    }

    #[test]
    fn recursive_split_extents_follow_the_authoritative_ratios() {
        let directory = std::env::current_dir().expect("current directory");
        let mut model = CoreModel::new();
        let initial = model
            .apply(
                0,
                CoreCommand::CreateSpace {
                    name: "Space".into(),
                    directory,
                },
            )
            .expect("create Space");
        let first_pane = match &initial.snapshot.spaces[0].tabs[0].layout {
            PaneLayout::Pane(pane) => pane.id,
            PaneLayout::Split(_) => panic!("initial Tab must contain one Pane"),
        };
        let split = model
            .apply(
                initial.revision,
                CoreCommand::SplitPane {
                    pane_id: first_pane,
                    axis: SplitAxis::Horizontal,
                    placement: SplitPlacement::After,
                    ratio: SplitRatio::new(600).expect("valid ratio"),
                },
            )
            .expect("split Pane");
        let mut extents = Vec::new();
        pane_extents(
            &split.snapshot.spaces[0].tabs[0].layout,
            1000.0,
            500.0,
            &mut extents,
        );

        assert_eq!(extents.len(), 2);
        assert!((extents[0].1 - 599.4).abs() < 0.1);
        assert!((extents[1].1 - 399.6).abs() < 0.1);
        assert_eq!(extents[0].2, 500.0);
        assert_eq!(extents[1].2, 500.0);
    }

    #[test]
    fn late_terminal_updates_cannot_restore_a_closed_terminal_projection() {
        let directory = std::env::current_dir().expect("current directory");
        let mut model = CoreModel::new();
        let initial = model
            .apply(
                0,
                CoreCommand::CreateSpace {
                    name: "Space".into(),
                    directory,
                },
            )
            .expect("create Space");
        let pane_id = match &initial.snapshot.spaces[0].tabs[0].layout {
            PaneLayout::Pane(pane) => pane.id,
            PaneLayout::Split(_) => panic!("initial Tab must contain one Pane"),
        };
        let terminal_session_id = initial.snapshot.terminal_sessions[0].id;
        let closed = model
            .apply(initial.revision, CoreCommand::ClosePane { pane_id })
            .expect("close Pane");
        let mut terminals = HashMap::new();
        let mut terminal_errors = HashMap::new();

        accept_terminal_snapshot(
            &closed.snapshot,
            &mut terminals,
            &mut terminal_errors,
            terminal_session_id,
            TerminalSnapshot {
                revision: 1,
                lifecycle: TerminalLifecycle::Running,
                cols: 1,
                rows: 1,
                cursor: None,
                default_fg: [0xdd; 3],
                default_bg: [0x11; 3],
                cells: Vec::new(),
            },
        );

        assert!(!terminals.contains_key(&terminal_session_id));
    }

    #[test]
    fn created_resources_become_the_client_local_selection() {
        let directory = std::env::current_dir().expect("current directory");
        let mut model = CoreModel::new();
        let initial = model
            .apply(
                0,
                CoreCommand::CreateSpace {
                    name: "Space".into(),
                    directory,
                },
            )
            .expect("create Space");
        let first_pane = match &initial.snapshot.spaces[0].tabs[0].layout {
            PaneLayout::Pane(pane) => pane.id,
            PaneLayout::Split(_) => panic!("initial Tab must contain one Pane"),
        };
        let split = model
            .apply(
                initial.revision,
                CoreCommand::SplitPane {
                    pane_id: first_pane,
                    axis: SplitAxis::Horizontal,
                    placement: SplitPlacement::After,
                    ratio: SplitRatio::EQUAL,
                },
            )
            .expect("split Pane");
        let CreatedResource::Pane { pane_id, .. } = split.created else {
            panic!("split must identify the new Pane");
        };

        let selection =
            selection_for_created(UiSelection::default(), &split.created, &split.snapshot);

        assert_eq!(selection.space_id, Some(split.snapshot.spaces[0].id));
        assert_eq!(selection.tab_id, Some(split.snapshot.spaces[0].tabs[0].id));
        assert_eq!(selection.pane_id, Some(pane_id));
    }
}
