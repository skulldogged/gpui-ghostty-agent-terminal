use crate::{
    CoreClient, CoreCommand, CoreSnapshot, CreatedResource, PaneId, PaneLayout, SpaceId, SplitAxis,
    SplitId, SplitPlacement, SplitRatio, TabId, TerminalLifecycle, TerminalSessionId, TerminalSize,
    TerminalSnapshot,
    core_driver::{CoreDriver, CoreProjection, DriverUpdate},
    terminal_frame::{FrameRow, TerminalFrame},
    terminal_grid::{CellMetrics, GridDimensions, fixed_cell_glyph_x, measured_cell_height},
    ui_shell::{ShellColor, ShellIcon, WorkspaceShell},
};
use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, Context, Decorations, FocusHandle, IntoElement,
    KeyDownEvent, Keystroke, MouseButton, MouseMoveEvent, Pixels, Point, Render, ShapedLine,
    SharedString, Task, TextRun, Window, WindowControlArea, canvas, div, fill, font, point,
    prelude::*, px, rgb, size,
};
use std::collections::{HashMap, HashSet};
#[cfg(windows)]
use std::time::Duration;

const TERMINAL_PADDING_PX: f32 = 10.0;
const SPLIT_DIVIDER_PX: f32 = 5.0;
const DEFAULT_FONT_SIZE_PX: f32 = 14.0;

pub(crate) fn open_terminal_window(
    cx: &mut App,
    endpoint: &crate::CoreEndpoint,
) -> Result<AnyWindowHandle, String> {
    let terminal_font = TerminalFont::resolve(cx)?;
    let core = CoreClient::connect_or_spawn_at(endpoint)?;
    let (driver, projection) = CoreDriver::start(core)?;
    let shell = WorkspaceShell::from_environment();
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
        .open_window(shell.window_options(bounds), move |window, cx| {
            let focus = cx.focus_handle();
            focus.focus(window, cx);
            let view = cx.new(|_| MultiplexerView {
                shell,
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
                move_source: None,
                selections_after_commands: HashMap::new(),
                sidebar_width: WorkspaceShell::SIDEBAR_WIDTH,
                sidebar_dragging: false,
                split_geometries: HashMap::new(),
                split_dragging: None,
                preview_split_ratios: HashMap::new(),
                pending_split_resizes: HashMap::new(),
                titlebar_drag_armed: false,
            });
            view.update(cx, |view, cx| {
                view.start_refresh_task(cx);
                #[cfg(windows)]
                view.start_windows_probe(cx);
            });
            view
        })
        .map_err(|error| format!("open GPUI window: {error}"))?;

    window
        .update(cx, |_view, _window, cx| cx.activate(true))
        .map_err(|error| format!("activate GPUI window: {error}"))?;
    Ok(window.into())
}

struct MultiplexerView {
    shell: WorkspaceShell,
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
    move_source: Option<PaneId>,
    selections_after_commands: HashMap<u64, PaneId>,
    sidebar_width: f32,
    sidebar_dragging: bool,
    split_geometries: HashMap<SplitId, SplitGeometry>,
    split_dragging: Option<SplitId>,
    preview_split_ratios: HashMap<SplitId, SplitRatio>,
    pending_split_resizes: HashMap<u64, SplitId>,
    titlebar_drag_armed: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct UiSelection {
    space_id: Option<SpaceId>,
    tab_id: Option<TabId>,
    pane_id: Option<PaneId>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SplitGeometry {
    axis: SplitAxis,
    start: f32,
    length: f32,
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
                if self.move_source.is_some_and(|pane_id| {
                    !self
                        .hierarchy
                        .spaces
                        .iter()
                        .flat_map(|space| &space.tabs)
                        .any(|tab| layout_contains_pane(&tab.layout, pane_id))
                }) {
                    self.move_source = None;
                }
            }
            DriverUpdate::CommandAccepted {
                command_id,
                outcome,
            } => {
                if let Some(split_id) = self.pending_split_resizes.remove(&command_id) {
                    self.preview_split_ratios.remove(&split_id);
                }
                self.selection = self
                    .selections_after_commands
                    .remove(&command_id)
                    .map(|pane_id| selection_for_pane(pane_id, &self.hierarchy))
                    .unwrap_or_else(|| {
                        selection_for_created(self.selection, &outcome.created, &self.hierarchy)
                    });
                self.global_error = None;
            }
            DriverUpdate::CommandRejected { command_id, error } => {
                self.selections_after_commands.remove(&command_id);
                if let Some(split_id) = self.pending_split_resizes.remove(&command_id) {
                    self.preview_split_ratios.remove(&split_id);
                }
                self.global_error = Some(error);
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
            DriverUpdate::Error(error) => {
                self.selections_after_commands.clear();
                self.pending_split_resizes.clear();
                self.preview_split_ratios.clear();
                self.split_dragging = None;
                self.global_error = Some(error);
            }
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
        self.move_source = None;
        self.selections_after_commands.clear();
        self.split_geometries.clear();
        self.split_dragging = None;
        self.preview_split_ratios.clear();
        self.pending_split_resizes.clear();
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
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn select_tab(&mut self, tab_id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        self.selection.tab_id = Some(tab_id);
        self.selection.pane_id = None;
        self.selection = self.selection.normalized(&self.hierarchy);
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn focus_pane(&mut self, pane_id: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(source_pane_id) = self.move_source
            && source_pane_id != pane_id
        {
            self.move_source = None;
            self.selection.pane_id = Some(pane_id);
            if let Some(command_id) = self.submit_core_command(
                CoreCommand::MovePane {
                    pane_id: source_pane_id,
                    target_pane_id: pane_id,
                    axis: SplitAxis::Horizontal,
                    placement: SplitPlacement::After,
                    ratio: SplitRatio::EQUAL,
                },
                cx,
            ) {
                self.selections_after_commands
                    .insert(command_id, source_pane_id);
            }
            self.focus.focus(window, cx);
            return;
        }
        self.selection.pane_id = Some(pane_id);
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn submit_core_command(&mut self, command: CoreCommand, cx: &mut Context<Self>) -> Option<u64> {
        match self.driver.apply_core_command(command) {
            Ok(command_id) => {
                self.global_error = None;
                cx.notify();
                Some(command_id)
            }
            Err(error) => {
                self.global_error = Some(error);
                cx.notify();
                None
            }
        }
    }

    fn create_space(&mut self, cx: &mut Context<Self>) {
        self.cancel_move();
        match std::env::current_dir() {
            Ok(directory) => {
                self.submit_core_command(
                    CoreCommand::CreateSpace {
                        name: format!("Space {}", self.hierarchy.spaces.len() + 1),
                        directory,
                    },
                    cx,
                );
            }
            Err(error) => {
                self.global_error = Some(format!("locate current directory: {error}"));
                cx.notify();
            }
        }
    }

    fn create_tab(&mut self, cx: &mut Context<Self>) {
        self.cancel_move();
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
        self.cancel_move();
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

    fn toggle_move_focused_pane(&mut self, cx: &mut Context<Self>) {
        let Some(pane_id) = self.selection.pane_id else {
            return;
        };
        self.move_source = (self.move_source != Some(pane_id)).then_some(pane_id);
        cx.notify();
    }

    fn cancel_move(&mut self) {
        self.move_source = None;
    }

    fn resize_visible_terminals(&mut self, viewport: gpui::Size<Pixels>) {
        let Some(layout) = self.selected_tab().map(|tab| tab.layout.clone()) else {
            return;
        };
        let width = (f32::from(viewport.width) - self.sidebar_width).max(1.0);
        let height = (f32::from(viewport.height) - WorkspaceShell::TITLE_BAR_HEIGHT).max(1.0);
        let mut panes = Vec::new();
        let mut split_geometries = HashMap::new();
        collect_layout_metrics(
            &layout,
            LayoutRect {
                x: self.sidebar_width,
                y: WorkspaceShell::TITLE_BAR_HEIGHT,
                width,
                height,
            },
            &self.preview_split_ratios,
            &mut panes,
            &mut split_geometries,
        );
        self.split_geometries = split_geometries;
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

    fn update_split_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(split_id) = self.split_dragging else {
            return;
        };
        let Some(geometry) = self.split_geometries.get(&split_id).copied() else {
            self.split_dragging = None;
            return;
        };
        let pointer = match geometry.axis {
            SplitAxis::Horizontal => position.x.as_f32(),
            SplitAxis::Vertical => position.y.as_f32(),
        };
        let ratio = split_ratio_at(geometry, pointer);
        if self.preview_split_ratios.get(&split_id) != Some(&ratio) {
            self.preview_split_ratios.insert(split_id, ratio);
            cx.notify();
        }
    }

    fn finish_split_drag(&mut self, cx: &mut Context<Self>) {
        let Some(split_id) = self.split_dragging.take() else {
            return;
        };
        let Some(ratio) = self.preview_split_ratios.get(&split_id).copied() else {
            return;
        };
        let authoritative_ratio = self
            .selected_tab()
            .and_then(|tab| find_split_ratio(&tab.layout, split_id));
        if authoritative_ratio == Some(ratio) {
            self.preview_split_ratios.remove(&split_id);
            cx.notify();
            return;
        }
        match self.submit_core_command(CoreCommand::ResizeSplit { split_id, ratio }, cx) {
            Some(command_id) => {
                self.pending_split_resizes.insert(command_id, split_id);
            }
            None => {
                self.preview_split_ratios.remove(&split_id);
            }
        }
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut sidebar = div()
            .flex()
            .flex_col()
            .relative()
            .w(px(self.sidebar_width))
            .h_full()
            .flex_none()
            .bg(self.shell.color(ShellColor::Sidebar))
            .border_r_1()
            .border_color(self.shell.color(ShellColor::Border))
            .px_1()
            .pt_2()
            .gap(px(2.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(28.))
                    .px_3()
                    .text_size(px(11.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(self.shell.color(ShellColor::FaintText))
                    .child("SPACES")
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(self.shell.color(ShellColor::FaintText))
                            .child(self.hierarchy.spaces.len().to_string()),
                    ),
            );
        for space in &self.hierarchy.spaces {
            let space_id = space.id;
            let selected = self.selection.space_id == Some(space_id);
            let initial = space
                .name
                .chars()
                .next()
                .map(|character| character.to_uppercase().to_string())
                .unwrap_or_else(|| "·".into());
            let tab_count = space.tabs.len();
            let tab_label = if tab_count == 1 { "tab" } else { "tabs" };
            let metadata = format!(
                "{tab_count} {tab_label}  ·  {}",
                space.directory.to_string_lossy()
            );
            sidebar = sidebar.child(
                div()
                    .id(("space", space_id.as_u64()))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_h(px(52.))
                    .mx_1()
                    .px_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(if selected {
                        self.shell.color(ShellColor::SelectedBorder)
                    } else {
                        self.shell.color(ShellColor::Sidebar)
                    })
                    .when(selected, |this| {
                        this.bg(self.shell.color(ShellColor::Selected))
                    })
                    .hover(|this| this.bg(self.shell.color(ShellColor::Hover)))
                    .on_click(cx.listener(move |view, _event, window, cx| {
                        view.select_space(space_id, window, cx)
                    }))
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(24.))
                            .rounded_full()
                            .bg(if selected {
                                self.shell.color(ShellColor::AccentMuted)
                            } else {
                                self.shell.color(ShellColor::Hover)
                            })
                            .text_size(px(11.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(if selected {
                                self.shell.color(ShellColor::Accent)
                            } else {
                                self.shell.color(ShellColor::MutedText)
                            })
                            .child(initial),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(13.))
                                    .font_weight(if selected {
                                        gpui::FontWeight::SEMIBOLD
                                    } else {
                                        gpui::FontWeight::NORMAL
                                    })
                                    .text_color(self.shell.color(ShellColor::Text))
                                    .child(space.name.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(11.))
                                    .text_color(self.shell.color(ShellColor::FaintText))
                                    .child(metadata),
                            ),
                    ),
            );
        }
        sidebar
            .child(
                div()
                    .id("sidebar-resize")
                    .absolute()
                    .top_0()
                    .right(px(-3.))
                    .w(px(6.))
                    .h_full()
                    .cursor_col_resize()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|view, _event, _window, cx| {
                            view.sidebar_dragging = true;
                            cx.stop_propagation();
                        }),
                    ),
            )
            .into_any_element()
    }

    fn chrome_tile(
        &self,
        id: &'static str,
        icon: ShellIcon,
        active: bool,
        danger: bool,
    ) -> gpui::Stateful<gpui::Div> {
        let color = if danger {
            self.shell.color(ShellColor::Danger)
        } else if active {
            self.shell.color(ShellColor::Accent)
        } else {
            self.shell.color(ShellColor::MutedText)
        };
        div()
            .id(id)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .size(px(WorkspaceShell::CHROME_TILE_SIZE))
            .cursor_pointer()
            .rounded_lg()
            .when(active, |this| {
                this.bg(self.shell.color(ShellColor::AccentMuted))
            })
            .hover(move |this| {
                this.bg(self.shell.color(if danger {
                    ShellColor::DangerHover
                } else {
                    ShellColor::Hover
                }))
            })
            .child(self.shell.icon(icon, color))
    }

    fn render_titlebar_drag_region(&self, id: &'static str, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id(id)
            .flex_1()
            .h_full()
            .min_w(px(24.))
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _event, _window, _cx| {
                    view.titlebar_drag_armed = true;
                }),
            )
            .on_mouse_down_out(cx.listener(|view, _event, _window, _cx| {
                view.titlebar_drag_armed = false;
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|view, _event, _window, _cx| {
                    view.titlebar_drag_armed = false;
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|view, _event, _window, _cx| {
                    view.titlebar_drag_armed = false;
                }),
            )
            .on_mouse_move(cx.listener(|view, _event, window, _cx| {
                if view.titlebar_drag_armed {
                    view.titlebar_drag_armed = false;
                    window.start_window_move();
                }
            }))
            .on_click(|event, window, _cx| {
                if event.click_count() == 2 {
                    if cfg!(target_os = "linux") {
                        window.zoom_window();
                    } else {
                        window.titlebar_double_click();
                    }
                }
            })
            .into_any_element()
    }

    fn render_window_controls(&self, window: &Window) -> Option<AnyElement> {
        if cfg!(target_os = "macos")
            || (cfg!(target_os = "linux")
                && !matches!(window.window_decorations(), Decorations::Client { .. }))
        {
            return None;
        }

        let supported = window.window_controls();
        let windows_caption_font = windows_caption_font();
        let caption_button =
            |id: &'static str, glyph: &'static str, area: WindowControlArea, danger: bool| {
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(46.))
                    .h_full()
                    .text_size(px(if cfg!(windows) { 10. } else { 13. }))
                    .text_color(self.shell.color(ShellColor::MutedText))
                    .when(cfg!(windows), |this| this.font_family(windows_caption_font))
                    .window_control_area(area)
                    .hover(move |this| {
                        this.bg(self.shell.color(if danger {
                            ShellColor::DangerHover
                        } else {
                            ShellColor::Hover
                        }))
                        .text_color(self.shell.color(if danger {
                            ShellColor::Danger
                        } else {
                            ShellColor::Text
                        }))
                    })
                    .child(glyph)
            };
        let minimize_glyph = if cfg!(windows) { "\u{e921}" } else { "−" };
        let maximize_glyph = if cfg!(windows) {
            if window.is_maximized() {
                "\u{e923}"
            } else {
                "\u{e922}"
            }
        } else if window.is_maximized() {
            "▣"
        } else {
            "□"
        };
        let close_glyph = if cfg!(windows) { "\u{e8bb}" } else { "×" };

        let mut controls = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .w(px(WorkspaceShell::WINDOW_CONTROLS_WIDTH))
            .h_full()
            .flex_none();
        if supported.minimize {
            controls = controls.child(
                caption_button(
                    "window-minimize",
                    minimize_glyph,
                    WindowControlArea::Min,
                    false,
                )
                .when(cfg!(target_os = "linux"), |this| {
                    this.on_click(|_event, window, cx| {
                        cx.stop_propagation();
                        window.minimize_window();
                    })
                }),
            );
        }
        if supported.maximize {
            controls = controls.child(
                caption_button(
                    "window-maximize",
                    maximize_glyph,
                    WindowControlArea::Max,
                    false,
                )
                .when(cfg!(target_os = "linux"), |this| {
                    this.on_click(|_event, window, cx| {
                        cx.stop_propagation();
                        window.zoom_window();
                    })
                }),
            );
        }
        Some(
            controls
                .child(
                    caption_button("window-close", close_glyph, WindowControlArea::Close, true)
                        .when(cfg!(target_os = "linux"), |this| {
                            this.on_click(|_event, window, cx| {
                                cx.stop_propagation();
                                window.remove_window();
                            })
                        }),
                )
                .into_any_element(),
        )
    }

    fn render_title_bar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let sidebar_chrome = div()
            .flex()
            .items_center()
            .w(px(self.sidebar_width))
            .h_full()
            .flex_none()
            .border_r_1()
            .border_color(self.shell.color(ShellColor::Border))
            .px_2()
            .when(cfg!(target_os = "macos"), |this| this.pl(px(78.)))
            .when(!cfg!(target_os = "macos"), |this| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(WorkspaceShell::CHROME_TILE_SIZE))
                        .child(
                            self.shell
                                .icon(ShellIcon::AppMark, self.shell.color(ShellColor::Accent)),
                        ),
                )
            })
            .child(self.render_titlebar_drag_region("sidebar-titlebar-drag-region", cx))
            .child(
                self.chrome_tile("create-space", ShellIcon::Plus, false, false)
                    .on_click(cx.listener(|view, _event, _window, cx| view.create_space(cx))),
            );

        let mut tabs = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .h_full()
            .flex_shrink(1.)
            .min_w_0()
            .overflow_hidden()
            .pl_2();
        if let Some(space) = self.selected_space() {
            for tab in &space.tabs {
                let tab_id = tab.id;
                let selected = self.selection.tab_id == Some(tab_id);
                tabs = tabs.child(
                    div()
                        .id(("tab", tab_id.as_u64()))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .gap_2()
                        .h(px(WorkspaceShell::TAB_HEIGHT))
                        .max_w(px(180.))
                        .px_3()
                        .rounded_lg()
                        .border_1()
                        .border_color(if selected {
                            self.shell.color(ShellColor::SelectedBorder)
                        } else {
                            self.shell.color(ShellColor::Chrome)
                        })
                        .text_size(px(13.))
                        .text_color(if selected {
                            self.shell.color(ShellColor::Text)
                        } else {
                            self.shell.color(ShellColor::MutedText)
                        })
                        .when(selected, |this| {
                            this.bg(self.shell.color(ShellColor::Selected))
                        })
                        .hover(|this| this.bg(self.shell.color(ShellColor::Hover)))
                        .on_click(cx.listener(move |view, _event, window, cx| {
                            view.select_tab(tab_id, window, cx)
                        }))
                        .child(
                            self.shell
                                .icon(
                                    ShellIcon::AppMark,
                                    self.shell.color(if selected {
                                        ShellColor::Accent
                                    } else {
                                        ShellColor::FaintText
                                    }),
                                )
                                .size(px(11.)),
                        )
                        .child(div().truncate().child(tab.name.clone())),
                );
            }
            tabs = tabs.child(
                self.chrome_tile("create-tab", ShellIcon::Plus, false, false)
                    .on_click(cx.listener(|view, _event, _window, cx| view.create_tab(cx))),
            );
        }

        let pane_controls = div()
            .flex()
            .items_center()
            .h_full()
            .flex_none()
            .child(
                self.chrome_tile("split-horizontal", ShellIcon::SplitHorizontal, false, false)
                    .on_click(cx.listener(|view, _event, _window, cx| {
                        view.split_focused_pane(SplitAxis::Horizontal, cx)
                    })),
            )
            .child(
                self.chrome_tile("split-vertical", ShellIcon::SplitVertical, false, false)
                    .on_click(cx.listener(|view, _event, _window, cx| {
                        view.split_focused_pane(SplitAxis::Vertical, cx)
                    })),
            )
            .child(
                self.chrome_tile(
                    "move-pane",
                    ShellIcon::Move,
                    self.move_source.is_some(),
                    false,
                )
                .on_click(
                    cx.listener(|view, _event, _window, cx| view.toggle_move_focused_pane(cx)),
                ),
            );

        div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(WorkspaceShell::TITLE_BAR_HEIGHT))
            .w_full()
            .flex_none()
            .bg(self.shell.color(ShellColor::Chrome))
            .border_b_1()
            .border_color(self.shell.color(ShellColor::Border))
            .child(sidebar_chrome)
            .child(tabs)
            .child(self.render_titlebar_drag_region("main-titlebar-drag-region", cx))
            .child(pane_controls)
            .children(self.render_window_controls(window))
            .into_any_element()
    }

    fn render_selected_layout(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.selected_tab() {
            Some(tab) => self.render_pane_layout(&tab.layout, cx),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(self.shell.color(ShellColor::Window))
                .text_color(self.shell.color(ShellColor::MutedText))
                .child("No Spaces yet")
                .into_any_element(),
        }
    }

    fn render_split_divider(
        &self,
        split_id: SplitId,
        axis: SplitAxis,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self.split_dragging == Some(split_id);
        div()
            .id(("split-divider", split_id.as_u64()))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .when(axis == SplitAxis::Horizontal, |this| {
                this.w(px(SPLIT_DIVIDER_PX)).h_full().cursor_col_resize()
            })
            .when(axis == SplitAxis::Vertical, |this| {
                this.h(px(SPLIT_DIVIDER_PX)).w_full().cursor_row_resize()
            })
            .when(active, |this| {
                this.bg(self.shell.color(ShellColor::AccentMuted))
            })
            .hover(|this| this.bg(self.shell.color(ShellColor::Hover)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, _event, _window, cx| {
                    view.split_dragging = Some(split_id);
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .when(axis == SplitAxis::Horizontal, |this| {
                        this.w(px(1.)).h_full()
                    })
                    .when(axis == SplitAxis::Vertical, |this| this.h(px(1.)).w_full())
                    .bg(self.shell.color(if active {
                        ShellColor::Accent
                    } else {
                        ShellColor::Border
                    })),
            )
            .into_any_element()
    }

    fn render_pane_layout(&self, layout: &PaneLayout, cx: &mut Context<Self>) -> AnyElement {
        match layout {
            PaneLayout::Pane(pane) => self.render_terminal_pane(pane, cx),
            PaneLayout::Split(split) => {
                let first = self.render_pane_layout(&split.first, cx);
                let second = self.render_pane_layout(&split.second, cx);
                let ratio = self
                    .preview_split_ratios
                    .get(&split.id)
                    .copied()
                    .unwrap_or(split.ratio);
                let first_grow = f32::from(ratio.parts_per_thousand());
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
                    .child(first)
                    .child(self.render_split_divider(split.id, split.axis, cx))
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
                .bg(self.shell.terminal_background(rgb(0x0b0e13)))
                .text_color(rgb(0x8993a4))
                .child("Starting terminal…")
                .into_any_element();
        };
        let frame = TerminalFrame::from_snapshot(snapshot);
        let default_bg = self.shell.terminal_background(color(snapshot.default_bg));
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
                for background in &frame.opaque_backgrounds {
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
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .bg(default_bg)
            .border_1()
            .border_color(if focused {
                self.shell.color(ShellColor::Accent)
            } else {
                default_bg
            })
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
                            .text_color(self.shell.color(ShellColor::Danger))
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
            .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, window, cx| {
                if view.sidebar_dragging {
                    let viewport = window.viewport_size().width.as_f32();
                    let max_width = (viewport - 500.)
                        .max(WorkspaceShell::SIDEBAR_MIN_WIDTH)
                        .min(viewport * 0.5);
                    view.sidebar_width = event
                        .position
                        .x
                        .as_f32()
                        .clamp(WorkspaceShell::SIDEBAR_MIN_WIDTH, max_width);
                    cx.notify();
                }
                view.update_split_drag(event.position, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|view, _event, _window, cx| {
                    if view.sidebar_dragging {
                        view.sidebar_dragging = false;
                        cx.notify();
                    }
                    view.finish_split_drag(cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|view, _event, _window, cx| {
                    if view.sidebar_dragging {
                        view.sidebar_dragging = false;
                        cx.notify();
                    }
                    view.finish_split_drag(cx);
                }),
            )
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .bg(self.shell.root_color())
            .child(self.render_title_bar(window, cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(self.render_sidebar(cx))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
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
                        .rounded_lg()
                        .bg(self.shell.color(ShellColor::DangerHover))
                        .px_3()
                        .py_2()
                        .text_color(self.shell.color(ShellColor::Danger))
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

fn selection_for_pane(pane_id: PaneId, hierarchy: &CoreSnapshot) -> UiSelection {
    let location = hierarchy.spaces.iter().find_map(|space| {
        space
            .tabs
            .iter()
            .find(|tab| layout_contains_pane(&tab.layout, pane_id))
            .map(|tab| (space.id, tab.id))
    });
    UiSelection {
        space_id: location.map(|(space_id, _)| space_id),
        tab_id: location.map(|(_, tab_id)| tab_id),
        pane_id: location.map(|_| pane_id),
    }
    .normalized(hierarchy)
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct LayoutRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

#[cfg(test)]
fn pane_extents(
    layout: &PaneLayout,
    width: f32,
    height: f32,
    output: &mut Vec<(TerminalSessionId, f32, f32)>,
) {
    collect_layout_metrics(
        layout,
        LayoutRect {
            x: 0.,
            y: 0.,
            width,
            height,
        },
        &HashMap::new(),
        output,
        &mut HashMap::new(),
    );
}

fn collect_layout_metrics(
    layout: &PaneLayout,
    rect: LayoutRect,
    preview_ratios: &HashMap<SplitId, SplitRatio>,
    panes: &mut Vec<(TerminalSessionId, f32, f32)>,
    splits: &mut HashMap<SplitId, SplitGeometry>,
) {
    match layout {
        PaneLayout::Pane(pane) => panes.push((pane.terminal_session_id, rect.width, rect.height)),
        PaneLayout::Split(split) => {
            let ratio = preview_ratios
                .get(&split.id)
                .copied()
                .unwrap_or(split.ratio);
            let ratio = f32::from(ratio.parts_per_thousand()) / 1000.0;
            match split.axis {
                SplitAxis::Horizontal => {
                    let available = (rect.width - SPLIT_DIVIDER_PX).max(2.0);
                    let first_width = available * ratio;
                    splits.insert(
                        split.id,
                        SplitGeometry {
                            axis: split.axis,
                            start: rect.x,
                            length: available,
                        },
                    );
                    collect_layout_metrics(
                        &split.first,
                        LayoutRect {
                            width: first_width,
                            ..rect
                        },
                        preview_ratios,
                        panes,
                        splits,
                    );
                    collect_layout_metrics(
                        &split.second,
                        LayoutRect {
                            x: rect.x + first_width + SPLIT_DIVIDER_PX,
                            width: available - first_width,
                            ..rect
                        },
                        preview_ratios,
                        panes,
                        splits,
                    );
                }
                SplitAxis::Vertical => {
                    let available = (rect.height - SPLIT_DIVIDER_PX).max(2.0);
                    let first_height = available * ratio;
                    splits.insert(
                        split.id,
                        SplitGeometry {
                            axis: split.axis,
                            start: rect.y,
                            length: available,
                        },
                    );
                    collect_layout_metrics(
                        &split.first,
                        LayoutRect {
                            height: first_height,
                            ..rect
                        },
                        preview_ratios,
                        panes,
                        splits,
                    );
                    collect_layout_metrics(
                        &split.second,
                        LayoutRect {
                            y: rect.y + first_height + SPLIT_DIVIDER_PX,
                            height: available - first_height,
                            ..rect
                        },
                        preview_ratios,
                        panes,
                        splits,
                    );
                }
            }
        }
    }
}

fn split_ratio_at(geometry: SplitGeometry, pointer: f32) -> SplitRatio {
    let fraction = ((pointer - geometry.start) / geometry.length).clamp(0., 1.);
    let parts = (fraction * 1000.).round() as u16;
    SplitRatio::new(parts.clamp(SplitRatio::MIN_PARTS, SplitRatio::MAX_PARTS))
        .expect("clamped pointer ratio must be valid")
}

fn find_split_ratio(layout: &PaneLayout, split_id: SplitId) -> Option<SplitRatio> {
    match layout {
        PaneLayout::Pane(_) => None,
        PaneLayout::Split(split) if split.id == split_id => Some(split.ratio),
        PaneLayout::Split(split) => find_split_ratio(&split.first, split_id)
            .or_else(|| find_split_ratio(&split.second, split_id)),
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

fn windows_caption_font_for_build(build: u32) -> &'static str {
    if build >= 22_000 {
        "Segoe Fluent Icons"
    } else {
        "Segoe MDL2 Assets"
    }
}

#[cfg(windows)]
fn windows_caption_font() -> &'static str {
    windows_caption_font_for_build(windows_version::OsVersion::current().build)
}

#[cfg(not(windows))]
fn windows_caption_font() -> &'static str {
    windows_caption_font_for_build(22_000)
}

#[cfg(test)]
mod tests {
    use super::{
        SplitGeometry, UiSelection, accept_terminal_snapshot, first_pane_id, pane_extents,
        selection_for_created, selection_for_pane, split_ratio_at, terminal_input_bytes,
        windows_caption_font_for_build,
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
    fn windows_caption_font_follows_the_windows_11_build_boundary() {
        assert_eq!(windows_caption_font_for_build(21_999), "Segoe MDL2 Assets");
        assert_eq!(windows_caption_font_for_build(22_000), "Segoe Fluent Icons");
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
        assert!((extents[0].1 - 597.0).abs() < 0.1);
        assert!((extents[1].1 - 398.0).abs() < 0.1);
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

    #[test]
    fn dragging_a_split_seam_maps_pointer_position_to_a_bounded_ratio() {
        let geometry = SplitGeometry {
            axis: SplitAxis::Horizontal,
            start: 220.,
            length: 800.,
        };

        assert_eq!(split_ratio_at(geometry, 780.).parts_per_thousand(), 700);
        assert_eq!(split_ratio_at(geometry, 0.).parts_per_thousand(), 100);
        assert_eq!(split_ratio_at(geometry, 2_000.).parts_per_thousand(), 900);
    }

    #[test]
    fn moved_pane_selection_follows_it_across_spaces() {
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
        let source_pane =
            first_pane_id(&first.snapshot.spaces[0].tabs[0].layout).expect("source Pane");
        let second = model
            .apply(
                first.revision,
                CoreCommand::CreateSpace {
                    name: "Second".into(),
                    directory,
                },
            )
            .expect("create second Space");
        let CreatedResource::Space {
            space_id: target_space,
            pane_id: target_pane,
            ..
        } = second.created
        else {
            panic!("Space creation must identify its Pane");
        };
        let moved = model
            .apply(
                second.revision,
                CoreCommand::MovePane {
                    pane_id: source_pane,
                    target_pane_id: target_pane,
                    axis: SplitAxis::Horizontal,
                    placement: SplitPlacement::After,
                    ratio: SplitRatio::EQUAL,
                },
            )
            .expect("move Pane across Spaces");

        let selection = selection_for_pane(source_pane, &moved.snapshot);

        assert_eq!(selection.space_id, Some(target_space));
        assert_eq!(selection.pane_id, Some(source_pane));
    }
}
