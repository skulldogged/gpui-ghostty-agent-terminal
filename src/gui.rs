use crate::{
    AgentProgram, AgentSnapshot, AgentState, ApplicationCore, CoreCommand, CoreSnapshot,
    CreatedResource, PaneId, PaneLayout, SpaceId, SplitAxis, SplitId, SplitPlacement, SplitRatio,
    TabId, TerminalCursorShape, TerminalLifecycle, TerminalSessionId, TerminalSize,
    TerminalSnapshot,
    core_driver::{CoreDriver, DriverUpdate},
    core_model::default_space_name,
    ghostty,
    settings::{
        AppSettings, KeybindAction, Shortcut, TerminalCursorStyle, TerminalGlyphOverflow,
        ThemePreset, adjust_font_size,
    },
    terminal_frame::{FrameRow, TerminalFrame},
    terminal_grid::{
        CellMetrics, GridDimensions, fixed_cell_glyph_x, font_points_to_pixels,
        measured_cell_height, measured_cell_width,
    },
    terminal_selection::{SelectionRow, point_for_position, selection_rows},
    ui_shell::{ShellColor, ShellIcon, WorkspaceShell},
};
use gpui::{
    Animation, AnimationExt as _, AnyElement, AnyWindowHandle, App, Bounds, ClipboardItem, Context,
    Decorations, FocusHandle, Font, FontFallbacks, FontId, IntoElement, KeyDownEvent, Keystroke,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, PromptButton,
    PromptLevel, Render, ScrollDelta, ScrollWheelEvent, ShapedLine, SharedString, Task, TextRun,
    TouchPhase, Transformation, Window, WindowControlArea, canvas, div, ease_out_quint, fill, font,
    percentage, point, prelude::*, px, rgb, size, svg,
};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TERMINAL_PADDING_PX: f32 = 10.0;
const CURSOR_STROKE_PX: f32 = 2.0;
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(500);
const SYMBOL_OVERFLOW_TOLERANCE_CELLS: f32 = 0.25;
const SPLIT_DIVIDER_PX: f32 = 5.0;
const INACTIVE_PANE_CONTRAST: f32 = 0.62;
const WINDOWS_PAGE_SCROLL_SENTINEL_MIN: f32 = u32::MAX as f32 / 2.0;
const OPENAI_AGENT_ICON: &[u8] = include_bytes!("../assets/agent-icons/openai.svg");
const CLAUDE_AGENT_ICON: &[u8] = include_bytes!("../assets/agent-icons/claude.svg");
const GEMINI_AGENT_ICON: &[u8] = include_bytes!("../assets/agent-icons/gemini.svg");
const CHEVRON_RIGHT_ICON: &[u8] = include_bytes!("../assets/lucide/chevron-right.svg");
const TERMINAL_FONT_FALLBACK_CANDIDATES: [&str; 2] =
    ["Symbols Nerd Font Mono", "Symbols Nerd Font"];

pub(crate) fn open_terminal_window(
    cx: &mut App,
    core: ApplicationCore,
) -> Result<AnyWindowHandle, String> {
    let (settings, settings_warning) = AppSettings::load();
    let terminal_font = TerminalFont::resolve(&settings, cx)?;
    let available_fonts = installed_monospace_fonts(terminal_font.size, cx);
    core.set_terminal_theme(settings.theme.terminal_theme())?;
    core.set_terminal_cursor_shape(settings.cursor_style.into())?;
    let (driver, projection) = CoreDriver::start(core)?;
    let shell = WorkspaceShell::from_preferences(settings.theme, settings.effective_opacity());
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
                cursor_blink_task: Task::ready(()),
                cursor_blink: CursorBlinkState::new(Instant::now()),
                terminal_errors,
                global_error: settings_warning,
                terminal_font,
                settings,
                settings_open: false,
                settings_section: SettingsSection::Appearance,
                recording_binding: None,
                available_fonts,
                requested_sizes: HashMap::new(),
                terminal_scroll_remainders: HashMap::new(),
                pending_terminal_scrolls: HashMap::new(),
                terminal_scroll_flush_scheduled: false,
                terminal_render_cache: HashMap::new(),
                terminal_bounds: Arc::new(Mutex::new(HashMap::new())),
                terminal_selection_drag: None,
                move_source: None,
                selections_after_commands: HashMap::new(),
                sidebar_width: WorkspaceShell::SIDEBAR_WIDTH,
                sidebar_dragging: false,
                split_geometries: HashMap::new(),
                split_dragging: None,
                preview_split_ratios: HashMap::new(),
                pending_split_resizes: HashMap::new(),
                titlebar_drag_armed: false,
                sidebar_collapsed: false,
                expanded_agent_spaces: HashSet::new(),
                agent_layout_transitions: HashMap::new(),
            });
            view.update(cx, |view, cx| {
                view.start_refresh_task(cx);
                view.start_cursor_blink_task(cx);
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
    cursor_blink_task: Task<()>,
    cursor_blink: CursorBlinkState,
    terminal_errors: HashMap<TerminalSessionId, String>,
    global_error: Option<String>,
    terminal_font: TerminalFont,
    settings: AppSettings,
    settings_open: bool,
    settings_section: SettingsSection,
    recording_binding: Option<KeybindAction>,
    available_fonts: Vec<String>,
    requested_sizes: HashMap<TerminalSessionId, TerminalSize>,
    terminal_scroll_remainders: HashMap<TerminalSessionId, ScrollRemainder>,
    pending_terminal_scrolls: HashMap<TerminalSessionId, Vec<ghostty::ScrollInput>>,
    terminal_scroll_flush_scheduled: bool,
    terminal_render_cache: HashMap<TerminalSessionId, CachedTerminalRender>,
    terminal_bounds: Arc<Mutex<HashMap<PaneId, Bounds<Pixels>>>>,
    terminal_selection_drag: Option<TerminalSelectionDrag>,
    move_source: Option<PaneId>,
    selections_after_commands: HashMap<u64, PaneId>,
    sidebar_width: f32,
    sidebar_dragging: bool,
    split_geometries: HashMap<SplitId, SplitGeometry>,
    split_dragging: Option<SplitId>,
    preview_split_ratios: HashMap<SplitId, SplitRatio>,
    pending_split_resizes: HashMap<u64, SplitId>,
    titlebar_drag_armed: bool,
    sidebar_collapsed: bool,
    expanded_agent_spaces: HashSet<SpaceId>,
    agent_layout_transitions: HashMap<SpaceId, AgentLayoutTransition>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct UiSelection {
    space_id: Option<SpaceId>,
    tab_id: Option<TabId>,
    pane_id: Option<PaneId>,
}

#[derive(Clone, Copy, Debug)]
struct TerminalSelectionDrag {
    pane_id: PaneId,
    terminal_session_id: TerminalSessionId,
    input: ghostty::SelectionInput,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SplitGeometry {
    axis: SplitAxis,
    start: f32,
    length: f32,
    first_extent: f32,
    cell_step: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct ScrollRemainder {
    x: f32,
    y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TerminalRenderKey {
    revision: u64,
    inactive: bool,
    cursor_visible: bool,
    selection_foreground: [u8; 3],
}

struct CachedTerminalRender {
    key: TerminalRenderKey,
    frame: Arc<TerminalFrame>,
    shaped_rows: Arc<Vec<ShapedLine>>,
    selection_rows: Arc<Vec<SelectionRow>>,
}

#[derive(Clone, Copy, Debug)]
struct CursorBlinkState {
    visible: bool,
    deadline: Instant,
}

impl CursorBlinkState {
    fn new(now: Instant) -> Self {
        Self {
            visible: true,
            deadline: now + CURSOR_BLINK_INTERVAL,
        }
    }

    fn reset(&mut self, now: Instant) {
        self.visible = true;
        self.deadline = now + CURSOR_BLINK_INTERVAL;
    }

    fn update_at(&mut self, now: Instant, blinking: bool) -> bool {
        if now < self.deadline {
            return false;
        }
        self.deadline = now + CURSOR_BLINK_INTERVAL;
        if blinking {
            self.visible = !self.visible;
            true
        } else {
            let changed = !self.visible;
            self.visible = true;
            changed
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CloseTarget {
    Space(SpaceId),
    Tab(TabId),
    Pane(PaneId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AgentLayoutTransition {
    Expanding,
    Collapsing,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum SettingsSection {
    #[default]
    Appearance,
    Terminal,
    Keybindings,
}

impl SettingsSection {
    const ALL: [Self; 3] = [Self::Appearance, Self::Terminal, Self::Keybindings];

    fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Terminal => "Terminal",
            Self::Keybindings => "Keybindings",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Appearance => "Theme and window material",
            Self::Terminal => "Font family and sizing",
            Self::Keybindings => "Keyboard shortcuts",
        }
    }
}

impl CloseTarget {
    fn command(self) -> CoreCommand {
        match self {
            Self::Space(space_id) => CoreCommand::CloseSpace { space_id },
            Self::Tab(tab_id) => CoreCommand::CloseTab { tab_id },
            Self::Pane(pane_id) => CoreCommand::ClosePane { pane_id },
        }
    }
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
    font: Font,
    fallbacks: Vec<ResolvedTerminalFallback>,
    primary_font_id: FontId,
    #[cfg(any(windows, target_os = "macos"))]
    primary_font: Font,
    #[cfg(any(windows, target_os = "macos"))]
    primary_face_id: FontId,
    primary_baseline_px: f32,
    size: Pixels,
    cells: CellMetrics,
    #[cfg(any(windows, target_os = "macos"))]
    fallback_cache: Arc<Mutex<HashMap<char, Option<usize>>>>,
}

#[derive(Clone)]
struct ResolvedTerminalFallback {
    font_id: FontId,
    #[cfg(any(windows, target_os = "macos"))]
    font: Font,
}

impl TerminalFont {
    fn resolve(settings: &AppSettings, cx: &App) -> Result<Self, String> {
        let font_size_points = settings.effective_font_size();
        let size = px(font_points_to_pixels(font_size_points));
        let requested = settings.effective_font_family();
        let available = cx.text_system().all_font_names();
        let family: SharedString = requested
            .filter(|candidate| {
                font_is_available(candidate, &available)
                    && font_is_terminal_candidate(candidate, size, cx)
            })
            .or_else(|| {
                terminal_font_candidates()
                    .iter()
                    .copied()
                    .find(|candidate| {
                        font_is_available(candidate, &available)
                            && font_is_terminal_candidate(candidate, size, cx)
                    })
                    .map(str::to_owned)
            })
            .or_else(|| {
                available
                    .iter()
                    .find(|candidate| font_is_terminal_candidate(candidate, size, cx))
                    .cloned()
            })
            .ok_or_else(|| {
                "no installed fixed-pitch font is available; set AGENT_TERMINAL_FONT to an installed monospace family"
                    .to_owned()
            })?
            .into();
        let fallback_families = installed_terminal_font_fallbacks(&family, &available);
        let primary_font = terminal_font_with_fallbacks(family.clone(), &fallback_families);
        let font_id = cx.text_system().resolve_font(&primary_font);
        #[cfg(any(windows, target_os = "macos"))]
        let (primary_face, primary_face_id) = {
            let primary_face = font(family.clone());
            let primary_face_id = cx.text_system().resolve_font(&primary_face);
            (primary_face, primary_face_id)
        };
        let advance = cx
            .text_system()
            .advance(font_id, size, '0')
            .map(|advance| f32::from(advance.width))
            .unwrap_or_else(|_| f32::from(size) * 0.6);
        let ascent = f32::from(cx.text_system().ascent(font_id, size));
        let descent = f32::from(cx.text_system().descent(font_id, size)).abs();
        let cell_height = measured_cell_height(f32::from(size), ascent, descent);
        let primary_baseline_px = (f32::from(cell_height) - ascent - descent).max(0.) / 2. + ascent;
        let fallbacks = fallback_families
            .into_iter()
            .map(|family| {
                let font = font(family);
                let font_id = cx.text_system().resolve_font(&font);
                ResolvedTerminalFallback {
                    font_id,
                    #[cfg(any(windows, target_os = "macos"))]
                    font,
                }
            })
            .collect();

        Ok(Self {
            family,
            font: primary_font,
            fallbacks,
            primary_font_id: font_id,
            #[cfg(any(windows, target_os = "macos"))]
            primary_font: primary_face,
            #[cfg(any(windows, target_os = "macos"))]
            primary_face_id,
            primary_baseline_px,
            size,
            cells: CellMetrics::new(measured_cell_width(advance), cell_height),
            #[cfg(any(windows, target_os = "macos"))]
            fallback_cache: Arc::default(),
        })
    }
}

fn terminal_font_with_fallbacks(family: impl Into<SharedString>, fallbacks: &[String]) -> Font {
    let mut terminal_font = font(family);
    if !fallbacks.is_empty() {
        terminal_font.fallbacks = Some(FontFallbacks::from_fonts(fallbacks.to_vec()));
    }
    terminal_font
}

fn terminal_text_runs(
    row: &FrameRow,
    terminal_font: &TerminalFont,
    window: &Window,
) -> Vec<TextRun> {
    let mut runs = Vec::<TextRun>::new();

    for glyph_cell in &row.glyph_cells {
        let text = &row.text[glyph_cell.byte_range.clone()];
        let font = terminal_font.font_for_text(text, window);
        let color = color(glyph_cell.color).into();

        if let Some(previous) = runs
            .last_mut()
            .filter(|run| run.font == *font && run.color == color)
        {
            previous.len += glyph_cell.byte_range.len();
        } else {
            runs.push(TextRun {
                len: glyph_cell.byte_range.len(),
                font: font.clone(),
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            });
        }
    }

    runs
}

fn shape_terminal_rows(
    rows: &[FrameRow],
    previous: Option<&CachedTerminalRender>,
    terminal_font: &TerminalFont,
    window: &Window,
) -> Vec<ShapedLine> {
    rows.iter()
        .enumerate()
        .map(|(row_index, row)| {
            if let Some(previous) = previous
                && let Some(previous_index) =
                    reusable_shaped_row_index(&previous.frame.rows, row, row_index)
                && let Some(shaped) = previous.shaped_rows.get(previous_index)
            {
                return shaped.clone();
            }

            let runs = terminal_text_runs(row, terminal_font, window);
            window.text_system().shape_line(
                row.text.clone().into(),
                terminal_font.size,
                &runs,
                None,
            )
        })
        .collect()
}

fn reusable_shaped_row_index(
    previous_rows: &[FrameRow],
    row: &FrameRow,
    preferred_index: usize,
) -> Option<usize> {
    previous_rows
        .get(preferred_index)
        .filter(|previous| *previous == row)
        .map(|_| preferred_index)
        .or_else(|| previous_rows.iter().position(|previous| previous == row))
}

impl TerminalFont {
    fn is_symbol_fallback(&self, font_id: FontId) -> bool {
        self.fallbacks
            .iter()
            .any(|fallback| fallback.font_id == font_id)
    }

    fn font_for_text<'a>(&'a self, text: &str, window: &Window) -> &'a Font {
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let _ = (text, window);
            &self.font
        }
        #[cfg(any(windows, target_os = "macos"))]
        {
            // GPUI keeps bundled faces outside the platform collections consulted by named
            // fallback on Windows and macOS. Probe the actual shaped face once per character and
            // select the bundled face explicitly when the primary did not supply it.
            let Some(character) = text.chars().next() else {
                return &self.font;
            };
            if character.is_ascii() {
                return &self.font;
            }

            if let Some(fallback) = self
                .fallback_cache
                .lock()
                .expect("terminal fallback cache lock poisoned")
                .get(&character)
                .copied()
            {
                return fallback
                    .and_then(|index| self.fallbacks.get(index))
                    .map(|fallback| &fallback.font)
                    .unwrap_or(&self.font);
            }

            let fallback = if font_face_contains_character(
                &self.primary_font,
                self.primary_face_id,
                self.size,
                character,
                window,
            ) {
                None
            } else {
                self.fallbacks.iter().position(|fallback| {
                    font_face_contains_character(
                        &fallback.font,
                        fallback.font_id,
                        self.size,
                        character,
                        window,
                    )
                })
            };
            self.fallback_cache
                .lock()
                .expect("terminal fallback cache lock poisoned")
                .insert(character, fallback);

            fallback
                .and_then(|index| self.fallbacks.get(index))
                .map(|fallback| &fallback.font)
                .unwrap_or(&self.font)
        }
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn font_face_contains_character(
    font: &Font,
    font_id: FontId,
    size: Pixels,
    character: char,
    window: &Window,
) -> bool {
    let mut encoded = [0; 4];
    let text = character.encode_utf8(&mut encoded);
    let run = TextRun {
        len: text.len(),
        font: font.clone(),
        color: color([0; 3]).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let shaped = window
        .text_system()
        .shape_line(text.to_owned().into(), size, &[run], None);

    shaped.runs.iter().any(|run| {
        run.font_id == font_id
            && run
                .glyphs
                .iter()
                .any(|glyph| glyph.index == 0 && glyph.id.0 != 0)
    })
}

fn installed_terminal_font_fallbacks(primary: &str, available: &[String]) -> Vec<String> {
    installed_terminal_font_fallbacks_from(primary, available, &TERMINAL_FONT_FALLBACK_CANDIDATES)
}

fn installed_terminal_font_fallbacks_from(
    primary: &str,
    available: &[String],
    candidates: &[&str],
) -> Vec<String> {
    candidates
        .iter()
        .filter_map(|candidate| {
            available
                .iter()
                .find(|family| family.eq_ignore_ascii_case(candidate))
        })
        .filter(|family| !family.eq_ignore_ascii_case(primary))
        .fold(Vec::new(), |mut fallbacks, family| {
            if !fallbacks
                .iter()
                .any(|existing: &String| existing.eq_ignore_ascii_case(family))
            {
                fallbacks.push(family.clone());
            }
            fallbacks
        })
}

fn installed_monospace_fonts(size: Pixels, cx: &App) -> Vec<String> {
    let mut fonts = cx
        .text_system()
        .all_font_names()
        .into_iter()
        .filter(|candidate| font_is_terminal_candidate(candidate, size, cx))
        .collect::<Vec<_>>();
    fonts.sort_by_key(|family| family.to_ascii_lowercase());
    fonts.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    fonts
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

fn font_is_terminal_candidate(candidate: &str, size: Pixels, cx: &App) -> bool {
    !candidate.eq_ignore_ascii_case("lucide")
        && !font_is_terminal_fallback_only(candidate)
        && font_is_fixed_pitch(candidate, size, cx)
}

fn font_is_terminal_fallback_only(candidate: &str) -> bool {
    TERMINAL_FONT_FALLBACK_CANDIDATES
        .iter()
        .any(|family| candidate.eq_ignore_ascii_case(family))
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

    fn start_cursor_blink_task(&mut self, cx: &mut Context<Self>) {
        self.cursor_blink_task = cx.spawn(async move |this, cx| {
            loop {
                let Ok(deadline) = this.update(cx, |view, _cx| view.cursor_blink.deadline) else {
                    break;
                };
                cx.background_executor()
                    .timer(deadline.saturating_duration_since(Instant::now()))
                    .await;
                let Ok(()) = this.update(cx, |view, cx| {
                    let focused_cursor_blinks = view
                        .focused_terminal_session_id()
                        .and_then(|terminal_session_id| view.terminals.get(&terminal_session_id))
                        .is_some_and(|snapshot| {
                            snapshot.cursor.is_some() && snapshot.cursor_blinking
                        });
                    if view
                        .cursor_blink
                        .update_at(Instant::now(), focused_cursor_blinks)
                    {
                        cx.notify();
                    }
                }) else {
                    break;
                };
            }
        });
    }

    fn accept_driver_update(&mut self, update: DriverUpdate) {
        let focused_before = self.focused_terminal_session_id();
        let reset_for_terminal = match &update {
            DriverUpdate::Terminal {
                terminal_session_id,
                ..
            } => cursor_blink_resets_for_terminal(focused_before, *terminal_session_id),
            _ => false,
        };
        match update {
            DriverUpdate::Hierarchy(hierarchy) => {
                self.hierarchy = hierarchy;
                self.retain_live_terminals();
                self.expanded_agent_spaces.retain(|space_id| {
                    self.hierarchy
                        .spaces
                        .iter()
                        .any(|space| space.id == *space_id)
                });
                self.agent_layout_transitions.retain(|space_id, _| {
                    self.hierarchy
                        .spaces
                        .iter()
                        .any(|space| space.id == *space_id)
                });
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
                self.requested_sizes.get(&terminal_session_id).copied(),
                snapshot,
            ),
            DriverUpdate::Error(error) => {
                self.selections_after_commands.clear();
                self.pending_split_resizes.clear();
                self.preview_split_ratios.clear();
                self.split_dragging = None;
                // A resize is recorded as soon as it enters the asynchronous driver queue.
                // If the driver later rejects that batch, retaining the recorded target would
                // suppress old-geometry snapshots forever and prevent the render loop from
                // retrying the resize.
                self.requested_sizes.clear();
                self.global_error = Some(error);
            }
        }
        if reset_for_terminal || self.focused_terminal_session_id() != focused_before {
            self.cursor_blink.reset(Instant::now());
        }
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
        self.terminal_scroll_remainders
            .retain(|terminal_session_id, _| terminal_ids.contains(terminal_session_id));
        self.pending_terminal_scrolls
            .retain(|terminal_session_id, _| terminal_ids.contains(terminal_session_id));
        self.terminal_render_cache
            .retain(|terminal_session_id, _| terminal_ids.contains(terminal_session_id));
    }

    fn on_terminal_scroll(
        &mut self,
        pane_id: PaneId,
        terminal_session_id: TerminalSessionId,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let remainder = self
            .terminal_scroll_remainders
            .entry(terminal_session_id)
            .or_default();
        match event.touch_phase {
            TouchPhase::Started => {
                *remainder = ScrollRemainder::default();
            }
            TouchPhase::Ended | TouchPhase::Cancelled => return,
            TouchPhase::Moved => {}
        }

        let (columns, rows) = match event.delta {
            ScrollDelta::Lines(delta) => (delta.x, delta.y),
            ScrollDelta::Pixels(delta) => (
                delta.x / px(self.terminal_font.cells.width_px()),
                delta.y / px(self.terminal_font.cells.height_px()),
            ),
        };
        let (page_columns, page_rows) = self
            .terminals
            .get(&terminal_session_id)
            .map_or((1, 1), |snapshot| (snapshot.cols, snapshot.rows));
        let columns = normalize_page_scroll_delta(columns, page_columns);
        let rows = normalize_page_scroll_delta(rows, page_rows);
        let accumulated_x = remainder.x + columns;
        let accumulated_y = remainder.y + rows;
        let whole_columns = accumulated_x.trunc() as isize;
        let whole_rows = accumulated_y.trunc() as isize;
        remainder.x = accumulated_x.fract();
        remainder.y = accumulated_y.fract();
        if whole_rows == 0 && whole_columns == 0 {
            return;
        }

        let bounds = self
            .terminal_bounds
            .lock()
            .expect("terminal bounds mutex poisoned")
            .get(&pane_id)
            .copied();
        let (pointer_x, pointer_y, viewport_width, viewport_height) =
            bounds.map_or((0.0, 0.0, 1, 1), |bounds| {
                let width = f32::from(bounds.size.width).max(1.0);
                let height = f32::from(bounds.size.height).max(1.0);
                (
                    f32::from(event.position.x - bounds.left()).clamp(0.0, width - 1.0),
                    f32::from(event.position.y - bounds.top()).clamp(0.0, height - 1.0),
                    width.ceil() as u32,
                    height.ceil() as u32,
                )
            });
        let modifiers = u16::from(event.modifiers.shift)
            | (u16::from(event.modifiers.control) << 1)
            | (u16::from(event.modifiers.alt) << 2)
            | (u16::from(event.modifiers.platform) << 3);
        // GPUI reports positive Y when the wheel moves content toward the
        // viewport bottom; Ghostty's viewport API uses negative rows for up.
        let input = ghostty::ScrollInput {
            delta_rows: -whole_rows,
            delta_columns: -whole_columns,
            pointer_x,
            pointer_y,
            viewport_width,
            viewport_height,
            cell_width: self.terminal_font.cells.width_px().round().max(1.0) as u32,
            cell_height: self.terminal_font.cells.height_px().round().max(1.0) as u32,
            modifiers,
        };
        coalesce_scroll_input(
            self.pending_terminal_scrolls
                .entry(terminal_session_id)
                .or_default(),
            input,
        );
        if !self.terminal_scroll_flush_scheduled {
            self.terminal_scroll_flush_scheduled = true;
            cx.on_next_frame(window, |view, _window, cx| {
                view.flush_terminal_scrolls(cx);
            });
        }
        cx.stop_propagation();
    }

    fn flush_terminal_scrolls(&mut self, cx: &mut Context<Self>) -> bool {
        self.terminal_scroll_flush_scheduled = false;
        let mut pending = std::mem::take(&mut self.pending_terminal_scrolls)
            .into_iter()
            .collect::<Vec<_>>();
        pending.sort_unstable_by_key(|(terminal_session_id, _)| *terminal_session_id);
        for (terminal_session_id, inputs) in pending {
            for input in inputs {
                if let Err(error) = self.driver.scroll_terminal(terminal_session_id, input) {
                    self.global_error = Some(error);
                    cx.notify();
                    return false;
                }
            }
        }
        true
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(action) = self.recording_binding {
            if event.keystroke.key.eq_ignore_ascii_case("escape") {
                self.recording_binding = None;
                cx.notify();
            } else if event.keystroke.key.eq_ignore_ascii_case("backspace") {
                self.reset_keybinding(action, cx);
            } else if let Some(shortcut) = shortcut_from_keystroke(&event.keystroke) {
                if let Some(conflict) = self.settings.keybindings.conflict_for(action, &shortcut) {
                    self.global_error = Some(format!(
                        "{} is already assigned to {}",
                        shortcut.display(),
                        conflict.label()
                    ));
                    cx.notify();
                } else {
                    self.settings.keybindings.set(action, Some(shortcut));
                    self.recording_binding = None;
                    self.save_settings(cx);
                }
            }
            cx.stop_propagation();
            return;
        }
        if self.settings_open && event.keystroke.key.eq_ignore_ascii_case("escape") {
            self.settings_open = false;
            self.focus.focus(window, cx);
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if !self.flush_terminal_scrolls(cx) {
            cx.stop_propagation();
            return;
        }
        if let Some(action) = KeybindAction::ALL.into_iter().find(|action| {
            shortcut_matches(&self.settings.keybindings.get(*action), &event.keystroke)
        }) {
            if action == KeybindAction::OpenSettings {
                self.toggle_settings(window, cx);
            } else if !self.settings_open {
                self.perform_keybind_action(action, window, cx);
            }
            cx.stop_propagation();
            return;
        }
        if self.settings_open {
            return;
        }
        let Some(terminal_session_id) = self.focused_terminal_session_id() else {
            return;
        };
        if terminal_copy_shortcut(&event.keystroke)
            && let Some(text) = self.selected_terminal_text()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            cx.stop_propagation();
            return;
        }
        if terminal_paste_shortcut(&event.keystroke) {
            self.clear_terminal_selection();
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                self.cursor_blink.reset(Instant::now());
                cx.notify();
                if let Err(error) = self.driver.paste_to(terminal_session_id, text.into_bytes()) {
                    self.global_error = Some(error);
                    cx.notify();
                }
            }
            cx.stop_propagation();
            return;
        }
        if let Some(input) = terminal_key_input(&event.keystroke) {
            self.clear_terminal_selection();
            self.cursor_blink.reset(Instant::now());
            cx.notify();
            if let Err(error) = self.driver.key_to(terminal_session_id, input) {
                self.global_error = Some(error);
                cx.notify();
            }
            cx.stop_propagation();
        }
    }

    fn reset_keybinding(&mut self, action: KeybindAction, cx: &mut Context<Self>) {
        let default = crate::settings::default_shortcut(action);
        if let Some(conflict) = self.settings.keybindings.conflict_for(action, &default) {
            self.global_error = Some(format!(
                "{} is already assigned to {}; change that shortcut before resetting {}",
                default.display(),
                conflict.label(),
                action.label()
            ));
            cx.notify();
            return;
        }
        self.settings.keybindings.set(action, None);
        self.recording_binding = None;
        self.save_settings(cx);
    }

    fn perform_keybind_action(
        &mut self,
        action: KeybindAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            KeybindAction::OpenSettings => unreachable!("settings is handled before dispatch"),
            KeybindAction::CreateSpace => self.create_space(cx),
            KeybindAction::CreateTab => self.create_tab(cx),
            KeybindAction::ClosePane => {
                if let Some(pane_id) = self.selection.pane_id {
                    self.close_target(CloseTarget::Pane(pane_id), window, cx);
                }
            }
            KeybindAction::SplitHorizontal => self.split_focused_pane(SplitAxis::Horizontal, cx),
            KeybindAction::SplitVertical => self.split_focused_pane(SplitAxis::Vertical, cx),
        }
    }

    fn save_settings(&mut self, cx: &mut Context<Self>) {
        match self.settings.save() {
            Ok(()) => self.global_error = None,
            Err(error) => self.global_error = Some(error),
        }
        cx.notify();
    }

    fn toggle_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_open = !self.settings_open;
        self.recording_binding = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn apply_shell_preferences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shell = WorkspaceShell::from_preferences(
            self.settings.theme,
            self.settings.effective_opacity(),
        );
        window.set_background_appearance(self.shell.appearance());
        self.save_settings(cx);
    }

    fn set_theme(&mut self, theme: ThemePreset, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self.driver.set_terminal_theme(theme.terminal_theme()) {
            self.global_error = Some(error);
            cx.notify();
            return;
        }
        self.settings.theme = theme;
        self.apply_shell_preferences(window, cx);
    }

    fn set_opacity(&mut self, opacity: f32, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.background_opacity = opacity;
        self.settings.sanitize();
        self.apply_shell_preferences(window, cx);
    }

    fn update_terminal_font(&mut self, family: Option<String>, size: f32, cx: &mut Context<Self>) {
        let previous_family = self.settings.font_family.clone();
        let previous_size = self.settings.font_size;
        self.settings.font_family = family;
        self.settings.font_size = size;
        self.settings.sanitize();
        match TerminalFont::resolve(&self.settings, cx) {
            Ok(font) => {
                self.terminal_font = font;
                self.terminal_render_cache.clear();
                self.requested_sizes.clear();
                self.save_settings(cx);
            }
            Err(error) => {
                self.settings.font_family = previous_family;
                self.settings.font_size = previous_size;
                self.global_error = Some(error);
                cx.notify();
            }
        }
    }

    fn set_terminal_glyph_overflow(
        &mut self,
        terminal_glyph_overflow: TerminalGlyphOverflow,
        cx: &mut Context<Self>,
    ) {
        self.settings.terminal_glyph_overflow = terminal_glyph_overflow;
        self.save_settings(cx);
    }

    fn set_terminal_cursor_style(
        &mut self,
        cursor_style: TerminalCursorStyle,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = self.driver.set_terminal_cursor_shape(cursor_style.into()) {
            self.global_error = Some(error);
            cx.notify();
            return;
        }
        self.settings.cursor_style = cursor_style;
        self.save_settings(cx);
    }

    fn focused_terminal_session_id(&self) -> Option<TerminalSessionId> {
        let pane_id = self.selection.pane_id?;
        self.selected_tab()
            .and_then(|tab| terminal_for_pane(&tab.layout, pane_id))
    }

    fn terminal_selection_input_at(
        &self,
        pane_id: PaneId,
        position: Point<Pixels>,
        event_type: ghostty::SelectionEventType,
        click_count: usize,
    ) -> Option<(TerminalSessionId, ghostty::SelectionInput, bool)> {
        let terminal_session_id = self
            .selected_tab()
            .and_then(|tab| terminal_for_pane(&tab.layout, pane_id))?;
        let snapshot = self.terminals.get(&terminal_session_id)?;
        let bounds = self
            .terminal_bounds
            .lock()
            .expect("terminal bounds mutex poisoned")
            .get(&pane_id)
            .copied()?;
        let pointer_x = f32::from(position.x - bounds.left());
        let pointer_y = f32::from(position.y - bounds.top());
        let point = point_for_position(
            pointer_x,
            pointer_y,
            self.terminal_font.cells.width_px(),
            self.terminal_font.cells.height_px(),
            snapshot,
        )?;
        let screen_height = f32::from(bounds.size.height).max(1.0);
        let autoscroll = pointer_y < 0.0 || pointer_y >= screen_height;
        Some((
            terminal_session_id,
            ghostty::SelectionInput {
                event_type,
                click_count: click_count.min(usize::from(u8::MAX)) as u8,
                x: point.x,
                y: point.y,
                pointer_x,
                pointer_y,
                columns: u32::from(snapshot.cols),
                cell_width: self.terminal_font.cells.width_px().round().max(1.0) as u32,
                padding_left: 0,
                screen_height: screen_height.ceil() as u32,
            },
            autoscroll,
        ))
    }

    fn start_terminal_selection(
        &mut self,
        pane_id: PaneId,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let moving_pane = self.move_source.is_some();
        self.focus_pane(pane_id, window, cx);
        if moving_pane {
            self.terminal_selection_drag = None;
            return;
        }
        if !self.flush_terminal_scrolls(cx) {
            return;
        }
        let Some((terminal_session_id, input, _)) = self.terminal_selection_input_at(
            pane_id,
            event.position,
            ghostty::SelectionEventType::Press,
            event.click_count,
        ) else {
            return;
        };
        if let Err(error) = self.driver.selection_event(terminal_session_id, input) {
            self.global_error = Some(error);
            cx.notify();
            return;
        }
        self.terminal_selection_drag = Some(TerminalSelectionDrag {
            pane_id,
            terminal_session_id,
            input,
        });
    }

    fn update_terminal_selection_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.flush_terminal_scrolls(cx) {
            return;
        }
        let Some(drag) = self.terminal_selection_drag else {
            return;
        };
        let Some((terminal_session_id, input, autoscroll)) = self.terminal_selection_input_at(
            drag.pane_id,
            position,
            ghostty::SelectionEventType::Drag,
            usize::from(drag.input.click_count),
        ) else {
            return;
        };
        if let Err(error) = self.driver.selection_event(terminal_session_id, input) {
            self.global_error = Some(error);
            self.terminal_selection_drag = None;
            cx.notify();
            return;
        }
        if autoscroll {
            let mut autoscroll_input = input;
            autoscroll_input.event_type = ghostty::SelectionEventType::Autoscroll;
            if let Err(error) = self
                .driver
                .selection_event(terminal_session_id, autoscroll_input)
            {
                self.global_error = Some(error);
                self.terminal_selection_drag = None;
                cx.notify();
                return;
            }
        }
        self.terminal_selection_drag = Some(TerminalSelectionDrag {
            pane_id: drag.pane_id,
            terminal_session_id,
            input,
        });
    }

    fn finish_terminal_selection(&mut self, _position: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.flush_terminal_scrolls(cx) {
            return;
        }
        let Some(drag) = self.terminal_selection_drag.take() else {
            return;
        };
        let mut input = drag.input;
        input.event_type = ghostty::SelectionEventType::Release;
        if let Err(error) = self.driver.selection_event(drag.terminal_session_id, input) {
            self.global_error = Some(error);
            cx.notify();
        }
    }

    fn selected_terminal_text(&self) -> Option<String> {
        self.terminals
            .get(&self.focused_terminal_session_id()?)?
            .selection_text
            .clone()
    }

    fn clear_terminal_selection(&mut self) {
        let terminal_session_id = self
            .terminal_selection_drag
            .take()
            .map(|drag| drag.terminal_session_id)
            .or_else(|| {
                let terminal_session_id = self.focused_terminal_session_id()?;
                self.terminals
                    .get(&terminal_session_id)?
                    .selection_text
                    .as_ref()?;
                Some(terminal_session_id)
            });
        if let Some(terminal_session_id) = terminal_session_id
            && let Err(error) = self
                .driver
                .selection_event(terminal_session_id, ghostty::SelectionInput::clear())
        {
            self.global_error = Some(error);
        }
    }

    fn selected_terminal_background(&self) -> gpui::Rgba {
        self.focused_terminal_session_id()
            .and_then(|terminal_session_id| self.terminals.get(&terminal_session_id))
            .map(|snapshot| self.shell.terminal_background(color(snapshot.default_bg)))
            .unwrap_or_else(|| self.shell.terminal_background(rgb(0x0b0e13)))
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

    fn tab_display_name(&self, tab: &crate::TabSnapshot) -> String {
        if tab.name_is_custom {
            return tab.name.clone();
        }
        let pane_id = (self.selection.tab_id == Some(tab.id))
            .then_some(self.selection.pane_id)
            .flatten()
            .filter(|pane_id| layout_contains_pane(&tab.layout, *pane_id))
            .or_else(|| first_pane_id(&tab.layout));
        let terminal_session_id =
            pane_id.and_then(|pane_id| terminal_for_pane(&tab.layout, pane_id));
        if let Some(title) = terminal_session_id
            .and_then(|terminal_session_id| self.terminals.get(&terminal_session_id))
            .and_then(|snapshot| snapshot.title.as_deref())
            .map(str::trim)
            .filter(|title| !title.is_empty())
        {
            return title.to_owned();
        }
        if let Some(directory_name) = terminal_session_id
            .and_then(|terminal_session_id| {
                self.hierarchy
                    .terminal_sessions
                    .iter()
                    .find(|session| session.id == terminal_session_id)
            })
            .and_then(|session| session.launch.working_directory.file_name())
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
        {
            return directory_name.to_owned();
        }
        tab.name.clone()
    }

    fn select_space(&mut self, space_id: SpaceId, window: &mut Window, cx: &mut Context<Self>) {
        self.cursor_blink.reset(Instant::now());
        self.clear_terminal_selection();
        self.selection.space_id = Some(space_id);
        self.selection.tab_id = None;
        self.selection.pane_id = None;
        self.selection = self.selection.normalized(&self.hierarchy);
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn select_tab(&mut self, tab_id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        self.cursor_blink.reset(Instant::now());
        self.clear_terminal_selection();
        self.selection.tab_id = Some(tab_id);
        self.selection.pane_id = None;
        self.selection = self.selection.normalized(&self.hierarchy);
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn focus_pane(&mut self, pane_id: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        self.cursor_blink.reset(Instant::now());
        if self.selection.pane_id != Some(pane_id) {
            self.clear_terminal_selection();
        }
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
                        name: default_space_name(&directory),
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
                name: "Terminal".into(),
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

    fn close_target(&mut self, target: CloseTarget, window: &mut Window, cx: &mut Context<Self>) {
        if !self.flush_terminal_scrolls(cx) {
            return;
        }
        if !self.close_target_exists(target) {
            return;
        }
        let active_sessions = self.active_session_count(target);
        if active_sessions > 0 {
            let noun = if active_sessions == 1 {
                "session"
            } else {
                "sessions"
            };
            let answer = window.prompt(
                PromptLevel::Warning,
                "Close active terminal work?",
                Some(&format!(
                    "{active_sessions} terminal {noun} appear to be busy. Closing will stop the running work."
                )),
                &[
                    PromptButton::ok("Close anyway"),
                    PromptButton::cancel("Keep open"),
                ],
                cx,
            );
            cx.spawn(async move |this, cx| {
                if answer.await.ok() == Some(0)
                    && let Some(this) = this.upgrade()
                {
                    this.update(cx, move |view, cx| {
                        if view.close_target_exists(target) {
                            view.cancel_move();
                            view.submit_core_command(target.command(), cx);
                        }
                    });
                }
            })
            .detach();
            return;
        }
        self.cancel_move();
        self.submit_core_command(target.command(), cx);
    }

    fn active_session_count(&self, target: CloseTarget) -> usize {
        terminal_sessions_for_target(&self.hierarchy, target)
            .into_iter()
            .filter(|terminal_session_id| {
                self.terminals
                    .get(terminal_session_id)
                    .is_none_or(|snapshot| snapshot.active_work)
            })
            .count()
    }

    fn close_target_exists(&self, target: CloseTarget) -> bool {
        match target {
            CloseTarget::Space(space_id) => self
                .hierarchy
                .spaces
                .iter()
                .any(|space| space.id == space_id),
            CloseTarget::Tab(tab_id) => self
                .hierarchy
                .spaces
                .iter()
                .any(|space| space.tabs.iter().any(|tab| tab.id == tab_id)),
            CloseTarget::Pane(pane_id) => self
                .hierarchy
                .spaces
                .iter()
                .flat_map(|space| &space.tabs)
                .any(|tab| layout_contains_pane(&tab.layout, pane_id)),
        }
    }

    fn resize_visible_terminals(&mut self, viewport: gpui::Size<Pixels>) {
        let Some(layout) = self.selected_tab().map(|tab| tab.layout.clone()) else {
            return;
        };
        let sidebar_width = if self.sidebar_collapsed {
            0.
        } else {
            self.sidebar_width
        };
        let width = (f32::from(viewport.width) - sidebar_width).max(1.0);
        let height = (f32::from(viewport.height) - WorkspaceShell::TITLE_BAR_HEIGHT).max(1.0);
        let mut panes = Vec::new();
        let mut split_geometries = HashMap::new();
        collect_layout_metrics(
            &layout,
            LayoutRect {
                x: sidebar_width,
                y: WorkspaceShell::TITLE_BAR_HEIGHT,
                width,
                height,
            },
            &self.preview_split_ratios,
            self.terminal_font.cells,
            &mut panes,
            &mut split_geometries,
        );
        self.split_geometries = split_geometries;
        for (terminal_session_id, width, height) in panes {
            let dimensions =
                GridDimensions::fit(width, height, TERMINAL_PADDING_PX, self.terminal_font.cells);
            let size = self.terminal_font.cells.terminal_size(dimensions);
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
            .bg(self.shell.opaque_color(ShellColor::Sidebar))
            .border_r_1()
            .border_color(self.shell.opaque_color(ShellColor::Border))
            .px_1()
            .pt_2()
            .gap(px(2.));
        for space in &self.hierarchy.spaces {
            let space_id = space.id;
            let selected = self.selection.space_id == Some(space_id);
            let hover_group: SharedString = format!("space-hover-{}", space_id.as_u64()).into();
            let agents = agent_summary_for_space(space, &self.terminals);
            let has_agents = agents.is_some();
            let agents_expanded = self.expanded_agent_spaces.contains(&space_id);
            let agent_layout_transition = self.agent_layout_transitions.get(&space_id).copied();
            let terminal_summary = (!has_agents).then(|| {
                let tab = self
                    .selection
                    .tab_id
                    .filter(|_| selected)
                    .and_then(|tab_id| space.tabs.iter().find(|tab| tab.id == tab_id))
                    .or_else(|| space.tabs.first());
                tab.map(|tab| {
                    (
                        self.tab_display_name(tab),
                        space.tabs.len().saturating_sub(1),
                    )
                })
            });
            let close_button = div()
                .id(("close-space", space_id.as_u64()))
                .absolute()
                .top(px(10.))
                .right(px(7.))
                .flex()
                .items_center()
                .justify_center()
                .size(px(18.))
                .rounded_md()
                .cursor_pointer()
                .opacity(if selected { 1. } else { 0. })
                .when(!selected, |this| {
                    this.group_hover(hover_group.clone(), |this| this.opacity(1.))
                })
                .hover(|this| this.bg(self.shell.control_hover()))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_view, _event, _window, cx| cx.stop_propagation()),
                )
                .on_click(cx.listener(move |view, _event, _window, cx| {
                    cx.stop_propagation();
                    view.close_target(CloseTarget::Space(space_id), _window, cx);
                }))
                .child(self.shell.icon(
                    ShellIcon::Close,
                    self.shell.color(ShellColor::MutedText),
                    10.,
                ));
            sidebar =
                sidebar.child(
                    div()
                        .id(("space", space_id.as_u64()))
                        .group(hover_group.clone())
                        .relative()
                        .cursor_pointer()
                        .flex()
                        .items_start()
                        .gap_2()
                        .mx_1()
                        .px_2()
                        .py_2()
                        .rounded_lg()
                        .when(selected, |this| {
                            this.bg(self.shell.opaque_color(ShellColor::Selected))
                        })
                        .hover(move |this| {
                            this.bg(self.shell.opaque_color(if selected {
                                ShellColor::Selected
                            } else {
                                ShellColor::Hover
                            }))
                        })
                        .on_click(cx.listener(move |view, _event, window, cx| {
                            view.select_space(space_id, window, cx)
                        }))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .h(px(22.))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .text_size(px(13.))
                                                .font_weight(gpui::FontWeight::NORMAL)
                                                .text_color(self.shell.color(ShellColor::Text))
                                                .child(space.name.clone()),
                                        )
                                        .child(
                                            div()
                                                .flex_none()
                                                .w(px(if selected { 24. } else { 0. }))
                                                .when(!selected, |this| {
                                                    this.group_hover(hover_group.clone(), |this| {
                                                        this.w(px(24.))
                                                    })
                                                }),
                                        ),
                                )
                                .when_some(agents, |this, agents| {
                                    this.child(self.render_agent_summary(
                                        space_id,
                                        agents,
                                        agents_expanded,
                                        agent_layout_transition,
                                        cx,
                                    ))
                                })
                                .when_some(
                                    terminal_summary.flatten(),
                                    |this, (title, additional_tabs)| {
                                        this.child(self.render_compact_terminal_summary(
                                            title,
                                            additional_tabs,
                                        ))
                                    },
                                ),
                        )
                        .child(close_button),
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

    fn render_agent_summary(
        &self,
        space_id: SpaceId,
        summary: SpaceAgentSummary,
        expanded: bool,
        transition: Option<AgentLayoutTransition>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let transitioning = transition.is_some();
        let visible_count = summary.visible.len();
        let mut icons = div().flex().items_center().h(px(26.));
        if !expanded {
            for (index, entry) in summary.visible.iter().enumerate() {
                icons = icons.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(index > 0, |this| this.ml(px(-2.)))
                        .child(transitioning_agent_icon(entry, index, false, transitioning)),
                );
            }
        }
        let noun = if summary.count == 1 {
            "agent"
        } else {
            "agents"
        };
        let count_label = transitioning_agent_count_label(
            format!("{} {noun}", summary.count),
            space_id,
            visible_count,
            expanded,
            transitioning,
        );
        let toggle = div()
            .id(("toggle-space-agents", space_id.as_u64()))
            .flex()
            .flex_none()
            .items_center()
            .w_full()
            .h(px(28.))
            .mt(px(4.))
            .pl(px(3.))
            .pr(px(2.))
            .gap(px(7.))
            .rounded_md()
            .cursor_pointer()
            .text_size(px(12.))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(self.shell.color(ShellColor::MutedText))
            .hover(|this| this.bg(self.shell.control_hover()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_view, _event, _window, cx| cx.stop_propagation()),
            )
            .on_click(cx.listener(move |view, _event, _window, cx| {
                cx.stop_propagation();
                let transition = if view.expanded_agent_spaces.insert(space_id) {
                    AgentLayoutTransition::Expanding
                } else {
                    view.expanded_agent_spaces.remove(&space_id);
                    AgentLayoutTransition::Collapsing
                };
                view.agent_layout_transitions.insert(space_id, transition);
                let timer = cx.background_executor().timer(Duration::from_millis(220));
                cx.spawn(async move |this, cx| {
                    timer.await;
                    let Some(this) = this.upgrade() else {
                        return;
                    };
                    this.update(cx, move |view, cx| {
                        if view.agent_layout_transitions.get(&space_id) == Some(&transition) {
                            view.agent_layout_transitions.remove(&space_id);
                            cx.notify();
                        }
                    });
                })
                .detach();
                cx.notify();
            }))
            .when(!expanded, |this| this.child(icons))
            .child(count_label)
            .child(div().flex_1())
            .child(transitioning_agent_chevron(
                space_id,
                expanded,
                transition,
                self.shell.color(ShellColor::FaintText),
            ));

        let mut section = div().flex().flex_col().child(toggle);
        if expanded {
            let mut rows = div().flex().flex_col().flex_none().mt(px(3.)).gap(px(2.));
            for (index, entry) in summary.visible.into_iter().enumerate() {
                let tab_id = entry.tab_id;
                let pane_id = entry.pane_id;
                let active = self.selection.space_id == Some(space_id)
                    && self.selection.tab_id == Some(tab_id)
                    && self.selection.pane_id == Some(pane_id);
                let text = div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(1.))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.))
                            .line_height(px(19.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(self.shell.color(ShellColor::Text))
                            .child(format!(
                                "{} · {}",
                                entry.agent.program.label(),
                                agent_state_label(entry.agent.state)
                            )),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(11.))
                            .line_height(px(18.))
                            .text_color(self.shell.color(ShellColor::FaintText))
                            .child(agent_status_detail(entry.agent.state)),
                    );
                let text: AnyElement = if transition == Some(AgentLayoutTransition::Expanding) {
                    text.with_animation(
                        ("expand-agent-text", entry.terminal_session_id.as_u64()),
                        Animation::new(Duration::from_millis(160)).with_easing(ease_out_quint()),
                        |this, delta| this.relative().left(px(6. * (1. - delta))).opacity(delta),
                    )
                    .into_any_element()
                } else {
                    text.into_any_element()
                };
                rows = rows.child(
                    div()
                        .id(("space-agent", entry.terminal_session_id.as_u64()))
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(8.))
                        .px_1()
                        .py(px(5.))
                        .rounded_md()
                        .cursor_pointer()
                        .when(active, |this| {
                            this.bg(self.shell.color(ShellColor::Selected))
                        })
                        .hover(|this| this.bg(self.shell.control_hover()))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_view, _event, _window, cx| cx.stop_propagation()),
                        )
                        .on_click(cx.listener(move |view, _event, window, cx| {
                            cx.stop_propagation();
                            view.selection = UiSelection {
                                space_id: Some(space_id),
                                tab_id: Some(tab_id),
                                pane_id: Some(pane_id),
                            }
                            .normalized(&view.hierarchy);
                            view.focus.focus(window, cx);
                            cx.notify();
                        }))
                        .child(transitioning_agent_icon(
                            &entry,
                            index,
                            true,
                            transition == Some(AgentLayoutTransition::Expanding),
                        ))
                        .child(text),
                );
            }
            section = section.child(rows);
        }
        transitioning_agent_section(section, space_id, visible_count, expanded, transition)
    }

    fn render_compact_terminal_summary(&self, title: String, additional_tabs: usize) -> AnyElement {
        div()
            .flex()
            .items_center()
            .h(px(20.))
            .gap(px(6.))
            .text_size(px(12.))
            .text_color(self.shell.color(ShellColor::MutedText))
            .child(div().min_w_0().truncate().child(title))
            .when(additional_tabs > 0, |this| {
                this.child(
                    div()
                        .flex_none()
                        .text_color(self.shell.color(ShellColor::FaintText))
                        .child(format!("+{additional_tabs}")),
                )
            })
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
            .child(
                self.shell
                    .icon(icon, color, WorkspaceShell::CHROME_ICON_SIZE),
            )
    }

    fn sidebar_toggle(&self, cx: &mut Context<Self>) -> AnyElement {
        let hover_group: SharedString = "sidebar-toggle-hover".into();
        let icon_size = WorkspaceShell::CHROME_ICON_SIZE;
        let icon_inset = (WorkspaceShell::CHROME_TILE_SIZE - icon_size) / 2.;
        let toggle_icon = if self.sidebar_collapsed {
            ShellIcon::SidebarOpen
        } else {
            ShellIcon::SidebarClose
        };

        div()
            .id("toggle-sidebar")
            .group(hover_group.clone())
            .relative()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .size(px(WorkspaceShell::CHROME_TILE_SIZE))
            .rounded_lg()
            .cursor_pointer()
            .hover(|this| this.bg(self.shell.color(ShellColor::Hover)))
            .on_click(cx.listener(|view, _event, _window, cx| {
                view.sidebar_collapsed = !view.sidebar_collapsed;
                view.sidebar_dragging = false;
                view.requested_sizes.clear();
                cx.notify();
            }))
            .child(
                self.shell
                    .icon(
                        ShellIcon::AppMark,
                        self.shell.color(ShellColor::Accent),
                        icon_size,
                    )
                    .group_hover(hover_group.clone(), |this| this.opacity(0.)),
            )
            .child(
                self.shell
                    .icon(toggle_icon, self.shell.color(ShellColor::Text), icon_size)
                    .absolute()
                    .top(px(icon_inset))
                    .left(px(icon_inset))
                    .opacity(0.)
                    .group_hover(hover_group, |this| this.opacity(1.)),
            )
            .into_any_element()
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
        let sidebar_chrome_width = if self.sidebar_collapsed {
            WorkspaceShell::TITLE_BAR_HEIGHT + if cfg!(target_os = "macos") { 78. } else { 0. }
        } else {
            self.sidebar_width
        };
        let sidebar_chrome = div()
            .flex()
            .items_center()
            .w(px(sidebar_chrome_width))
            .h_full()
            .flex_none()
            .when(!self.sidebar_collapsed, |this| {
                this.border_r_1()
                    .border_color(self.shell.opaque_color(ShellColor::Border))
                    .bg(self.shell.opaque_color(ShellColor::Sidebar))
            })
            .px_2()
            .when(cfg!(target_os = "macos"), |this| this.pl(px(78.)))
            .child(self.sidebar_toggle(cx))
            .when(!self.sidebar_collapsed, |this| {
                this.child(self.render_titlebar_drag_region("sidebar-titlebar-drag-region", cx))
                    .child(
                        self.chrome_tile("create-space", ShellIcon::Plus, false, false)
                            .on_click(
                                cx.listener(|view, _event, _window, cx| view.create_space(cx)),
                            ),
                    )
            });

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
                let tab_name = self.tab_display_name(tab);
                let selected = self.selection.tab_id == Some(tab_id);
                let hover_group: SharedString = format!("tab-hover-{}", tab_id.as_u64()).into();
                let close_button = div()
                    .id(("close-tab", tab_id.as_u64()))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(18.))
                    .rounded_md()
                    .cursor_pointer()
                    .opacity(0.)
                    .group_hover(hover_group.clone(), |this| this.opacity(1.))
                    .hover(|this| this.bg(self.shell.color(ShellColor::DangerHover)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_view, _event, _window, cx| cx.stop_propagation()),
                    )
                    .on_click(cx.listener(move |view, _event, _window, cx| {
                        cx.stop_propagation();
                        view.close_target(CloseTarget::Tab(tab_id), _window, cx);
                    }))
                    .child(self.shell.icon(
                        ShellIcon::Close,
                        self.shell.color(ShellColor::MutedText),
                        10.,
                    ));
                tabs = tabs.child(
                    div()
                        .id(("tab", tab_id.as_u64()))
                        .group(hover_group)
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .gap_1()
                        .h(px(WorkspaceShell::TAB_HEIGHT))
                        .max_w(px(180.))
                        .px_2()
                        .rounded_lg()
                        .border_1()
                        .border_color(if selected {
                            self.shell.color(ShellColor::SelectedBorder)
                        } else {
                            self.shell.color(ShellColor::Border).alpha(0.)
                        })
                        .when(selected, |this| {
                            this.bg(self.shell.color(ShellColor::Selected))
                        })
                        .text_size(px(13.))
                        .text_color(if selected {
                            self.shell.color(ShellColor::Text)
                        } else {
                            self.shell.color(ShellColor::MutedText)
                        })
                        .hover(|this| {
                            this.bg(self.shell.color(if selected {
                                ShellColor::Selected
                            } else {
                                ShellColor::Hover
                            }))
                        })
                        .on_click(cx.listener(move |view, _event, window, cx| {
                            view.select_tab(tab_id, window, cx)
                        }))
                        .child(self.shell.icon(
                            ShellIcon::AppMark,
                            self.shell.color(if selected {
                                ShellColor::Accent
                            } else {
                                ShellColor::FaintText
                            }),
                            11.,
                        ))
                        .child(div().flex_1().min_w_0().truncate().child(tab_name))
                        .child(close_button),
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
            )
            .child(
                self.chrome_tile("open-settings", ShellIcon::Settings, false, false)
                    .on_click(
                        cx.listener(|view, _event, window, cx| view.toggle_settings(window, cx)),
                    ),
            );

        div()
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .h(px(WorkspaceShell::TITLE_BAR_HEIGHT))
            .w_full()
            .flex_none()
            .bg(self.selected_terminal_background())
            .child(sidebar_chrome)
            .child(tabs)
            .child(self.render_titlebar_drag_region("main-titlebar-drag-region", cx))
            .child(pane_controls)
            .children(self.render_window_controls(window))
            .into_any_element()
    }

    fn render_settings_title_bar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let sidebar_chrome = div()
            .flex()
            .items_center()
            .w(px(WorkspaceShell::SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .border_r_1()
            .border_color(self.shell.color(ShellColor::Border))
            .bg(self.shell.opaque_color(ShellColor::Sidebar))
            .px_2()
            .when(cfg!(target_os = "macos"), |this| this.pl(px(78.)))
            .when(!cfg!(target_os = "macos"), |this| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(WorkspaceShell::CHROME_TILE_SIZE))
                        .child(self.shell.icon(
                            ShellIcon::Settings,
                            self.shell.color(ShellColor::Accent),
                            WorkspaceShell::CHROME_ICON_SIZE,
                        )),
                )
            })
            .child(
                div()
                    .pl_1()
                    .text_size(px(13.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(self.shell.color(ShellColor::Text))
                    .child("Settings"),
            )
            .child(self.render_titlebar_drag_region("settings-sidebar-titlebar-drag", cx));

        let done = div()
            .id("settings-done")
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .h(px(28.))
            .px_3()
            .mr_1()
            .rounded_lg()
            .cursor_pointer()
            .text_size(px(12.))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(self.shell.color(ShellColor::Accent))
            .bg(self.shell.color(ShellColor::AccentMuted))
            .hover(|this| this.bg(self.shell.color(ShellColor::Selected)))
            .on_click(cx.listener(|view, _event, window, cx| view.toggle_settings(window, cx)))
            .child("Done");

        div()
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .h(px(WorkspaceShell::TITLE_BAR_HEIGHT))
            .w_full()
            .flex_none()
            .bg(self.shell.color(ShellColor::Chrome))
            .child(sidebar_chrome)
            .child(self.render_titlebar_drag_region("settings-main-titlebar-drag", cx))
            .child(done)
            .children(self.render_window_controls(window))
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .w_full()
                    .h(px(1.))
                    .bg(self.shell.color(ShellColor::Border)),
            )
            .into_any_element()
    }

    fn render_settings_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut navigation = div()
            .flex()
            .flex_col()
            .w(px(WorkspaceShell::SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .gap_1()
            .px_2()
            .py_3()
            .bg(self.shell.opaque_color(ShellColor::Sidebar))
            .border_r_1()
            .border_color(self.shell.opaque_color(ShellColor::Border))
            .child(
                div()
                    .px_2()
                    .pb_2()
                    .text_size(px(11.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(self.shell.opaque_color(ShellColor::FaintText))
                    .child("PREFERENCES"),
            );
        for (index, section) in SettingsSection::ALL.into_iter().enumerate() {
            let selected = self.settings_section == section;
            navigation = navigation.child(
                div()
                    .id(("settings-section", index))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .cursor_pointer()
                    .border_1()
                    .border_color(if selected {
                        self.shell.opaque_color(ShellColor::SelectedBorder)
                    } else {
                        self.shell.opaque_color(ShellColor::Sidebar)
                    })
                    .when(selected, |this| {
                        this.bg(self.shell.opaque_color(ShellColor::Selected))
                    })
                    .hover(|this| this.bg(self.shell.opaque_color(ShellColor::Hover)))
                    .on_click(cx.listener(move |view, _event, _window, cx| {
                        view.settings_section = section;
                        view.recording_binding = None;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(if selected {
                                gpui::FontWeight::SEMIBOLD
                            } else {
                                gpui::FontWeight::NORMAL
                            })
                            .text_color(self.shell.opaque_color(if selected {
                                ShellColor::Text
                            } else {
                                ShellColor::MutedText
                            }))
                            .child(section.label()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(self.shell.opaque_color(ShellColor::FaintText))
                            .child(section.description()),
                    ),
            );
        }

        div()
            .flex()
            .flex_row()
            .size_full()
            .min_h_0()
            .bg(self.shell.opaque_color(ShellColor::Window))
            .child(navigation)
            .child(
                div()
                    .id("settings-content-scroll")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .w_full()
                            .max_w(px(820.))
                            .mx_auto()
                            .px(px(36.))
                            .py(px(30.))
                            .child(match self.settings_section {
                                SettingsSection::Appearance => self.render_appearance_settings(cx),
                                SettingsSection::Terminal => self.render_terminal_settings(cx),
                                SettingsSection::Keybindings => self.render_keybinding_settings(cx),
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_settings_heading(
        &self,
        title: &'static str,
        description: &'static str,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .pb_4()
            .child(
                div()
                    .text_size(px(24.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(self.shell.opaque_color(ShellColor::Text))
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(self.shell.opaque_color(ShellColor::MutedText))
                    .child(description),
            )
            .into_any_element()
    }

    fn render_appearance_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut themes = div().flex().flex_row().flex_wrap().gap_2();
        for (index, theme) in ThemePreset::ALL.into_iter().enumerate() {
            let selected = self.settings.theme == theme;
            let preview = WorkspaceShell::from_preferences(theme, 1.);
            themes = themes.child(
                div()
                    .id(("theme-preset", index))
                    .flex()
                    .flex_col()
                    .w(px(220.))
                    .min_h(px(116.))
                    .p_3()
                    .gap_2()
                    .rounded_lg()
                    .cursor_pointer()
                    .border_1()
                    .border_color(self.shell.opaque_color(if selected {
                        ShellColor::Accent
                    } else {
                        ShellColor::Border
                    }))
                    .bg(self.shell.opaque_color(if selected {
                        ShellColor::Selected
                    } else {
                        ShellColor::Chrome
                    }))
                    .hover(|this| this.bg(self.shell.opaque_color(ShellColor::Hover)))
                    .on_click(cx.listener(move |view, _event, window, cx| {
                        view.set_theme(theme, window, cx)
                    }))
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(
                                div()
                                    .size(px(18.))
                                    .rounded_md()
                                    .bg(preview.opaque_color(ShellColor::Window)),
                            )
                            .child(
                                div()
                                    .size(px(18.))
                                    .rounded_md()
                                    .bg(preview.opaque_color(ShellColor::Sidebar)),
                            )
                            .child(
                                div()
                                    .size(px(18.))
                                    .rounded_md()
                                    .bg(preview.opaque_color(ShellColor::Accent)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(self.shell.opaque_color(ShellColor::Text))
                            .child(theme.label()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(self.shell.opaque_color(ShellColor::FaintText))
                            .child(theme.description()),
                    ),
            );
        }

        let mut opacity_choices = div().flex().flex_row().flex_wrap().gap_2();
        for (index, opacity) in [0.55_f32, 0.65, 0.75, 0.85, 1.].into_iter().enumerate() {
            let selected = (self.settings.background_opacity - opacity).abs() < 0.001;
            opacity_choices = opacity_choices.child(
                div()
                    .id(("opacity-choice", index))
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(34.))
                    .min_w(px(60.))
                    .px_3()
                    .rounded_lg()
                    .cursor_pointer()
                    .border_1()
                    .border_color(self.shell.opaque_color(if selected {
                        ShellColor::Accent
                    } else {
                        ShellColor::Border
                    }))
                    .bg(self.shell.opaque_color(if selected {
                        ShellColor::AccentMuted
                    } else {
                        ShellColor::Chrome
                    }))
                    .text_size(px(12.))
                    .font_weight(if selected {
                        gpui::FontWeight::SEMIBOLD
                    } else {
                        gpui::FontWeight::NORMAL
                    })
                    .text_color(self.shell.opaque_color(if selected {
                        ShellColor::Accent
                    } else {
                        ShellColor::MutedText
                    }))
                    .hover(|this| this.bg(self.shell.opaque_color(ShellColor::Hover)))
                    .on_click(cx.listener(move |view, _event, window, cx| {
                        view.set_opacity(opacity, window, cx)
                    }))
                    .child(format!("{}%", (opacity * 100.).round() as u32)),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(self.render_settings_heading(
                "Appearance",
                "Choose the atmosphere of the shell without changing terminal colors.",
            ))
            .child(self.settings_group("Theme", "Applied immediately across the window", themes))
            .child(self.settings_group(
                "Background material",
                "Lower values show more of the desktop material through the shell",
                opacity_choices,
            ))
            .into_any_element()
    }

    fn render_terminal_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let current_family = self.terminal_font.family.to_string();
        let font_size = self.settings.font_size;
        let size_control =
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(self.shell.opaque_color(ShellColor::Text))
                                .child("Font size"),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(self.shell.opaque_color(ShellColor::FaintText))
                                .child("Terminal cells resize with the rendered viewport"),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(self.settings_step_button("font-size-down", "−").on_click(
                            cx.listener(|view, _event, _window, cx| {
                                let size = adjust_font_size(view.settings.font_size, -1.);
                                view.update_terminal_font(
                                    view.settings.font_family.clone(),
                                    size,
                                    cx,
                                );
                            }),
                        ))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(54.))
                                .h(px(32.))
                                .rounded_lg()
                                .bg(self.shell.opaque_color(ShellColor::Selected))
                                .text_size(px(12.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(self.shell.opaque_color(ShellColor::Text))
                                .child(format!("{font_size:.1} pt")),
                        )
                        .child(self.settings_step_button("font-size-up", "+").on_click(
                            cx.listener(|view, _event, _window, cx| {
                                let size = adjust_font_size(view.settings.font_size, 1.);
                                view.update_terminal_font(
                                    view.settings.font_family.clone(),
                                    size,
                                    cx,
                                );
                            }),
                        )),
                );

        let mut font_choices = div().flex().flex_row().flex_wrap().gap_2();
        for (index, family) in self.available_fonts.iter().cloned().enumerate() {
            let selected = family.eq_ignore_ascii_case(&current_family);
            let displayed_family = family.clone();
            font_choices = font_choices.child(
                div()
                    .id(("terminal-font", index))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .w(px(220.))
                    .h(px(46.))
                    .px_3()
                    .rounded_lg()
                    .cursor_pointer()
                    .border_1()
                    .border_color(self.shell.opaque_color(if selected {
                        ShellColor::Accent
                    } else {
                        ShellColor::Border
                    }))
                    .bg(self.shell.opaque_color(if selected {
                        ShellColor::Selected
                    } else {
                        ShellColor::Chrome
                    }))
                    .text_color(self.shell.opaque_color(if selected {
                        ShellColor::Text
                    } else {
                        ShellColor::MutedText
                    }))
                    .hover(|this| this.bg(self.shell.opaque_color(ShellColor::Hover)))
                    .on_click(cx.listener(move |view, _event, _window, cx| {
                        view.update_terminal_font(
                            Some(displayed_family.clone()),
                            view.settings.font_size,
                            cx,
                        );
                    }))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .child(family.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(family)
                            .text_size(px(13.))
                            .child("Aa 01"),
                    ),
            );
        }

        let mut overflow_choices = div().flex().flex_row().flex_wrap().gap_2();
        for (index, policy) in TerminalGlyphOverflow::ALL.into_iter().enumerate() {
            let selected = self.settings.terminal_glyph_overflow == policy;
            overflow_choices = overflow_choices.child(
                div()
                    .id(("terminal-glyph-overflow", index))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .w(px(220.))
                    .min_h(px(66.))
                    .p_3()
                    .rounded_lg()
                    .cursor_pointer()
                    .border_1()
                    .border_color(self.shell.opaque_color(if selected {
                        ShellColor::Accent
                    } else {
                        ShellColor::Border
                    }))
                    .bg(self.shell.opaque_color(if selected {
                        ShellColor::Selected
                    } else {
                        ShellColor::Chrome
                    }))
                    .hover(|this| this.bg(self.shell.opaque_color(ShellColor::Hover)))
                    .on_click(cx.listener(move |view, _event, _window, cx| {
                        view.set_terminal_glyph_overflow(policy, cx);
                    }))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(self.shell.opaque_color(ShellColor::Text))
                            .child(policy.label()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(self.shell.opaque_color(ShellColor::FaintText))
                            .child(policy.description()),
                    ),
            );
        }

        let mut cursor_choices = div().flex().flex_row().flex_wrap().gap_2();
        for (index, cursor_style) in TerminalCursorStyle::ALL.into_iter().enumerate() {
            let selected = self.settings.cursor_style == cursor_style;
            cursor_choices = cursor_choices.child(
                div()
                    .id(("terminal-cursor-style", index))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .w(px(220.))
                    .min_h(px(66.))
                    .p_3()
                    .rounded_lg()
                    .cursor_pointer()
                    .border_1()
                    .border_color(self.shell.opaque_color(if selected {
                        ShellColor::Accent
                    } else {
                        ShellColor::Border
                    }))
                    .bg(self.shell.opaque_color(if selected {
                        ShellColor::Selected
                    } else {
                        ShellColor::Chrome
                    }))
                    .hover(|this| this.bg(self.shell.opaque_color(ShellColor::Hover)))
                    .on_click(cx.listener(move |view, _event, _window, cx| {
                        view.set_terminal_cursor_style(cursor_style, cx);
                    }))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(self.shell.opaque_color(ShellColor::Text))
                            .child(cursor_style.label()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(self.shell.opaque_color(ShellColor::FaintText))
                            .child(cursor_style.description()),
                    ),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(self.render_settings_heading(
                "Terminal",
                "Use any installed fixed-pitch font; terminal content remains the source of its own colors.",
            ))
            .child(self.settings_group("Text", "Changes apply to every visible pane", size_control))
            .child(self.settings_group(
                "Font family",
                "Only fonts that measure as fixed-pitch are shown",
                font_choices,
            ))
            .child(self.settings_group(
                "Cursor",
                "The default shape; terminal programs may override it",
                cursor_choices,
            ))
            .child(self.settings_group(
                "Symbol overflow",
                "Controls how fallback symbols may extend beyond their fixed-cell allocation",
                overflow_choices,
            ))
            .into_any_element()
    }

    fn render_keybinding_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut rows = div()
            .flex()
            .flex_col()
            .rounded_lg()
            .border_1()
            .border_color(self.shell.opaque_color(ShellColor::Border))
            .overflow_hidden();
        for (index, action) in KeybindAction::ALL.into_iter().enumerate() {
            let recording = self.recording_binding == Some(action);
            let custom = self.settings.keybindings.custom(action).is_some();
            let shortcut = self.settings.keybindings.get(action);
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .min_h(px(64.))
                    .px_4()
                    .py_2()
                    .bg(self.shell.opaque_color(ShellColor::Chrome))
                    .when(index > 0, |this| {
                        this.border_t_1()
                            .border_color(self.shell.opaque_color(ShellColor::Border))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(self.shell.opaque_color(ShellColor::Text))
                                    .child(action.label()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(self.shell.opaque_color(ShellColor::FaintText))
                                    .child(action.description()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .when(custom, |this| {
                                this.child(
                                    div()
                                        .id(("reset-keybind", index))
                                        .cursor_pointer()
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .text_size(px(11.))
                                        .text_color(self.shell.opaque_color(ShellColor::FaintText))
                                        .hover(|this| {
                                            this.bg(self.shell.opaque_color(ShellColor::Hover))
                                                .text_color(
                                                    self.shell.opaque_color(ShellColor::Text),
                                                )
                                        })
                                        .on_click(cx.listener(move |view, _event, _window, cx| {
                                            view.reset_keybinding(action, cx);
                                        }))
                                        .child("Reset"),
                                )
                            })
                            .child(
                                div()
                                    .id(("record-keybind", index))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .min_w(px(126.))
                                    .h(px(32.))
                                    .px_3()
                                    .rounded_lg()
                                    .cursor_pointer()
                                    .border_1()
                                    .border_color(self.shell.opaque_color(if recording {
                                        ShellColor::Accent
                                    } else {
                                        ShellColor::SelectedBorder
                                    }))
                                    .bg(self.shell.opaque_color(if recording {
                                        ShellColor::AccentMuted
                                    } else {
                                        ShellColor::Selected
                                    }))
                                    .text_size(px(11.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(self.shell.opaque_color(if recording {
                                        ShellColor::Accent
                                    } else {
                                        ShellColor::Text
                                    }))
                                    .hover(|this| {
                                        this.bg(self.shell.opaque_color(ShellColor::Hover))
                                    })
                                    .on_click(cx.listener(move |view, _event, _window, cx| {
                                        view.recording_binding = Some(action);
                                        view.global_error = None;
                                        cx.notify();
                                    }))
                                    .child(if recording {
                                        "Press shortcut…".to_owned()
                                    } else {
                                        shortcut.display()
                                    }),
                            ),
                    ),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(self.render_settings_heading(
                "Keybindings",
                "Click a shortcut, then press the replacement chord.",
            ))
            .child(rows)
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(self.shell.opaque_color(ShellColor::FaintText))
                    .child("Escape cancels recording. Backspace restores the default shortcut."),
            )
            .into_any_element()
    }

    fn settings_group(
        &self,
        title: &'static str,
        description: &'static str,
        content: impl IntoElement,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(self.shell.opaque_color(ShellColor::Border))
            .bg(self.shell.opaque_color(ShellColor::Chrome))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(self.shell.opaque_color(ShellColor::Text))
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(self.shell.opaque_color(ShellColor::FaintText))
                            .child(description),
                    ),
            )
            .child(content)
            .into_any_element()
    }

    fn settings_step_button(
        &self,
        id: &'static str,
        label: &'static str,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .size(px(32.))
            .rounded_lg()
            .cursor_pointer()
            .border_1()
            .border_color(self.shell.opaque_color(ShellColor::Border))
            .bg(self.shell.opaque_color(ShellColor::Chrome))
            .text_size(px(16.))
            .text_color(self.shell.opaque_color(ShellColor::MutedText))
            .hover(|this| {
                this.bg(self.shell.opaque_color(ShellColor::Hover))
                    .text_color(self.shell.opaque_color(ShellColor::Text))
            })
            .child(label)
    }

    fn render_selected_layout(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let layout = self.selected_tab().map(|tab| tab.layout.clone());
        match layout.as_ref() {
            Some(layout) => self.render_pane_layout(layout, window, cx),
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
            .bg(self.selected_terminal_background())
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

    fn render_pane_layout(
        &mut self,
        layout: &PaneLayout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match layout {
            PaneLayout::Pane(pane) => self.render_terminal_pane(pane, window, cx),
            PaneLayout::Split(split) => {
                let first = self.render_pane_layout(&split.first, window, cx);
                let second = self.render_pane_layout(&split.second, window, cx);
                let projected_extent = self
                    .split_geometries
                    .get(&split.id)
                    .map(|geometry| geometry.first_extent);
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
                        .when_some(projected_extent, |this, extent| {
                            this.w(px(extent)).flex_none()
                        })
                        .when(projected_extent.is_none(), |this| {
                            this.flex_grow(first_grow)
                        })
                        .child(first),
                    SplitAxis::Vertical => div()
                        .w_full()
                        .min_h_0()
                        .flex_basis(px(0.))
                        .when_some(projected_extent, |this, extent| {
                            this.h(px(extent)).flex_none()
                        })
                        .when(projected_extent.is_none(), |this| {
                            this.flex_grow(first_grow)
                        })
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
                            .flex_grow(if projected_extent.is_some() {
                                1.0
                            } else {
                                second_grow
                            })
                            .min_w_0()
                            .min_h_0()
                            .child(second),
                    )
                    .into_any_element()
            }
        }
    }

    fn render_terminal_pane(
        &mut self,
        pane: &crate::PaneSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pane_id = pane.id;
        let terminal_session_id = pane.terminal_session_id;
        let tab_is_split = self
            .selected_tab()
            .is_some_and(|tab| matches!(&tab.layout, PaneLayout::Split(_)));
        let inactive = tab_is_split && self.selection.pane_id != Some(pane_id);
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
        let cursor_visible = cursor_should_be_visible(
            inactive,
            snapshot.cursor_blinking,
            self.cursor_blink.visible,
        );
        let terminal_theme = self.settings.theme.terminal_theme();
        let selection_background = color(terminal_theme.selection_background);
        let default_bg = self.shell.terminal_background(color(snapshot.default_bg));
        let render_key = TerminalRenderKey {
            revision: snapshot.revision,
            inactive,
            cursor_visible,
            selection_foreground: terminal_theme.selection_foreground,
        };
        let next_render = (self
            .terminal_render_cache
            .get(&terminal_session_id)
            .is_none_or(|cached| cached.key != render_key))
        .then(|| {
            let mut frame = if inactive {
                TerminalFrame::from_snapshot_with_cursor(snapshot, cursor_visible)
                    .dimmed_toward(snapshot.default_bg, INACTIVE_PANE_CONTRAST)
            } else {
                TerminalFrame::from_snapshot_with_cursor(snapshot, cursor_visible)
            };
            let selection_rows = selection_rows(snapshot);
            frame.apply_selection_foreground(&selection_rows, terminal_theme.selection_foreground);
            (frame, selection_rows)
        });

        if let Some((frame, selection_rows)) = next_render {
            let previous = self.terminal_render_cache.remove(&terminal_session_id);
            let shaped_rows =
                shape_terminal_rows(&frame.rows, previous.as_ref(), &self.terminal_font, window);
            self.terminal_render_cache.insert(
                terminal_session_id,
                CachedTerminalRender {
                    key: render_key,
                    frame: Arc::new(frame),
                    shaped_rows: Arc::new(shaped_rows),
                    selection_rows: Arc::new(selection_rows),
                },
            );
        }
        let cached = self
            .terminal_render_cache
            .get(&terminal_session_id)
            .expect("terminal render cache is populated before painting");
        let frame = Arc::clone(&cached.frame);
        let shaped_rows = Arc::clone(&cached.shaped_rows);
        let selection_rows = Arc::clone(&cached.selection_rows);
        let terminal_font = self.terminal_font.clone();
        let paint_font = terminal_font.clone();
        let terminal_bounds = Arc::clone(&self.terminal_bounds);
        let terminal_glyph_overflow = self.settings.terminal_glyph_overflow;
        let terminal_canvas = canvas(
            move |bounds, _window, _cx| {
                terminal_bounds
                    .lock()
                    .expect("terminal bounds mutex poisoned")
                    .insert(pane_id, bounds);
                shaped_rows.as_ref().clone()
            },
            move |bounds, lines, window, _cx| {
                for background in &frame.opaque_backgrounds {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(
                                bounds.left()
                                    + px(f32::from(background.x) * paint_font.cells.width_px()),
                                bounds.top()
                                    + px(f32::from(background.y) * paint_font.cells.height_px()),
                            ),
                            size(
                                px(f32::from(background.width) * paint_font.cells.width_px()),
                                px(paint_font.cells.height_px()),
                            ),
                        ),
                        color(background.color),
                    ));
                }
                for selection in selection_rows.iter() {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(
                                bounds.left()
                                    + px(f32::from(selection.start_x)
                                        * paint_font.cells.width_px()),
                                bounds.top()
                                    + px(f32::from(selection.y)
                                        * paint_font.cells.height_px()),
                            ),
                            size(
                                px(f32::from(selection.end_x - selection.start_x + 1)
                                    * paint_font.cells.width_px()),
                                px(paint_font.cells.height_px()),
                            ),
                        ),
                        selection_background,
                    ));
                }
                for (y, line) in lines.iter().enumerate() {
                    let cursor_x = frame
                        .cursor_cell
                        .filter(|(_, cursor_y)| usize::from(*cursor_y) == y)
                        .map(|(cursor_x, _)| cursor_x);
                    let _ = paint_fixed_cell_line(
                        &frame.rows[y],
                        line,
                        point(
                            bounds.left(),
                            bounds.top() + px(y as f32 * paint_font.cells.height_px()),
                        ),
                        &paint_font,
                        terminal_glyph_overflow,
                        cursor_x,
                        window,
                    );
                }
                if let Some(cursor) = frame.cursor_overlay {
                    let cell_width = paint_font.cells.width_px();
                    let cell_height = paint_font.cells.height_px();
                    let cursor_width = cell_width * f32::from(cursor.width);
                    let cursor_origin = point(
                        bounds.left() + px(f32::from(cursor.x) * cell_width),
                        bounds.top() + px(f32::from(cursor.y) * cell_height),
                    );
                    for rect in cursor_rects(cursor.shape, cursor_width, cell_height) {
                        window.paint_quad(fill(
                            Bounds::new(
                                point(cursor_origin.x + px(rect.x), cursor_origin.y + px(rect.y)),
                                size(px(rect.width), px(rect.height)),
                            ),
                            color(cursor.color),
                        ));
                    }
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
            .p(px(TERMINAL_PADDING_PX))
            .font_family(self.terminal_font.family.clone())
            .text_size(self.terminal_font.size)
            .cursor_text()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    view.start_terminal_selection(pane_id, event, window, cx)
                }),
            )
            .on_scroll_wheel(cx.listener(move |view, event, window, cx| {
                view.on_terminal_scroll(pane_id, terminal_session_id, event, window, cx)
            }))
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

#[derive(Clone, Copy, Debug, PartialEq)]
struct CursorRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

fn cursor_rects(shape: TerminalCursorShape, cell_width: f32, cell_height: f32) -> Vec<CursorRect> {
    let stroke = CURSOR_STROKE_PX.min(cell_width).min(cell_height);
    match shape {
        TerminalCursorShape::Block => vec![CursorRect {
            x: 0.,
            y: 0.,
            width: cell_width,
            height: cell_height,
        }],
        TerminalCursorShape::Bar => vec![CursorRect {
            x: 0.,
            y: 0.,
            width: stroke,
            height: cell_height,
        }],
        TerminalCursorShape::Underline => vec![CursorRect {
            x: 0.,
            y: cell_height - stroke,
            width: cell_width,
            height: stroke,
        }],
        TerminalCursorShape::BlockHollow => vec![
            CursorRect {
                x: 0.,
                y: 0.,
                width: cell_width,
                height: stroke,
            },
            CursorRect {
                x: 0.,
                y: cell_height - stroke,
                width: cell_width,
                height: stroke,
            },
            CursorRect {
                x: 0.,
                y: stroke,
                width: stroke,
                height: (cell_height - stroke * 2.).max(0.),
            },
            CursorRect {
                x: cell_width - stroke,
                y: stroke,
                width: stroke,
                height: (cell_height - stroke * 2.).max(0.),
            },
        ],
    }
}

fn cursor_should_be_visible(inactive: bool, blinking: bool, blink_phase_visible: bool) -> bool {
    !inactive && (!blinking || blink_phase_visible)
}

fn cursor_blink_resets_for_terminal(
    focused: Option<TerminalSessionId>,
    updated: TerminalSessionId,
) -> bool {
    focused == Some(updated)
}

fn accept_terminal_snapshot(
    hierarchy: &CoreSnapshot,
    terminals: &mut HashMap<TerminalSessionId, TerminalSnapshot>,
    terminal_errors: &mut HashMap<TerminalSessionId, String>,
    terminal_session_id: TerminalSessionId,
    requested_size: Option<TerminalSize>,
    snapshot: TerminalSnapshot,
) {
    if !hierarchy
        .terminal_sessions
        .iter()
        .any(|session| session.id == terminal_session_id)
    {
        return;
    }
    if requested_size.is_some_and(|size| (snapshot.cols, snapshot.rows) != (size.cols, size.rows)) {
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
        if !self.settings_open {
            self.resize_visible_terminals(window.viewport_size());
        }
        div()
            .id("multiplexer")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|view, event, window, cx| view.on_key_down(event, window, cx)))
            .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, window, cx| {
                view.update_terminal_selection_drag(event.position, cx);
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
                cx.listener(|view, event: &MouseUpEvent, _window, cx| {
                    view.finish_terminal_selection(event.position, cx);
                    if view.sidebar_dragging {
                        view.sidebar_dragging = false;
                        cx.notify();
                    }
                    view.finish_split_drag(cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|view, event: &MouseUpEvent, _window, cx| {
                    view.finish_terminal_selection(event.position, cx);
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
            .child(if self.settings_open {
                self.render_settings_title_bar(window, cx)
            } else {
                self.render_title_bar(window, cx)
            })
            .child(if self.settings_open {
                self.render_settings_page(cx)
            } else {
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .when(!self.sidebar_collapsed, |this| {
                        this.child(self.render_sidebar(cx))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .w_full()
                            .child(self.render_selected_layout(window, cx)),
                    )
                    .into_any_element()
            })
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
        CellMetrics::new(9, 20),
        output,
        &mut HashMap::new(),
    );
}

fn collect_layout_metrics(
    layout: &PaneLayout,
    rect: LayoutRect,
    preview_ratios: &HashMap<SplitId, SplitRatio>,
    cells: CellMetrics,
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
                    let cell_step = cells.width_px();
                    let first_width = projected_split_extent(available, ratio, cell_step);
                    splits.insert(
                        split.id,
                        SplitGeometry {
                            axis: split.axis,
                            start: rect.x,
                            length: available,
                            first_extent: first_width,
                            cell_step,
                        },
                    );
                    collect_layout_metrics(
                        &split.first,
                        LayoutRect {
                            width: first_width,
                            ..rect
                        },
                        preview_ratios,
                        cells,
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
                        cells,
                        panes,
                        splits,
                    );
                }
                SplitAxis::Vertical => {
                    let available = (rect.height - SPLIT_DIVIDER_PX).max(2.0);
                    let cell_step = cells.height_px();
                    let first_height = projected_split_extent(available, ratio, cell_step);
                    splits.insert(
                        split.id,
                        SplitGeometry {
                            axis: split.axis,
                            start: rect.y,
                            length: available,
                            first_extent: first_height,
                            cell_step,
                        },
                    );
                    collect_layout_metrics(
                        &split.first,
                        LayoutRect {
                            height: first_height,
                            ..rect
                        },
                        preview_ratios,
                        cells,
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
                        cells,
                        panes,
                        splits,
                    );
                }
            }
        }
    }
}

fn projected_split_extent(length: f32, ratio: f32, cell_step: f32) -> f32 {
    let raw_extent = length * ratio;
    let min_extent = length * (f32::from(SplitRatio::MIN_PARTS) / 1000.0);
    let max_extent = length * (f32::from(SplitRatio::MAX_PARTS) / 1000.0);
    if !cell_step.is_finite() || cell_step <= 0.0 {
        return raw_extent.clamp(min_extent, max_extent);
    }

    let terminal_inset = TERMINAL_PADDING_PX * 2.0;
    let minimum_cell = ((min_extent - terminal_inset) / cell_step).ceil();
    let maximum_cell = ((max_extent - terminal_inset) / cell_step).floor();
    if minimum_cell > maximum_cell {
        return raw_extent.clamp(min_extent, max_extent);
    }

    let cell = ((raw_extent - terminal_inset) / cell_step)
        .round()
        .clamp(minimum_cell, maximum_cell);
    terminal_inset + cell * cell_step
}

fn split_ratio_at(geometry: SplitGeometry, pointer: f32) -> SplitRatio {
    let raw_extent = pointer - geometry.start;
    let extent = projected_split_extent(
        geometry.length,
        raw_extent / geometry.length,
        geometry.cell_step,
    );
    let fraction = extent / geometry.length;
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

#[derive(Clone, Copy)]
#[cfg_attr(not(test), allow(dead_code))]
enum PasteShortcutPlatform {
    MacOs,
    Linux,
    Windows,
}

fn terminal_paste_shortcut(key: &Keystroke) -> bool {
    #[cfg(target_os = "macos")]
    let platform = PasteShortcutPlatform::MacOs;
    #[cfg(target_os = "linux")]
    let platform = PasteShortcutPlatform::Linux;
    #[cfg(target_os = "windows")]
    let platform = PasteShortcutPlatform::Windows;

    terminal_paste_shortcut_for(key, platform)
}

fn terminal_copy_shortcut(key: &Keystroke) -> bool {
    #[cfg(target_os = "macos")]
    let platform = PasteShortcutPlatform::MacOs;
    #[cfg(target_os = "linux")]
    let platform = PasteShortcutPlatform::Linux;
    #[cfg(target_os = "windows")]
    let platform = PasteShortcutPlatform::Windows;

    terminal_copy_shortcut_for(key, platform)
}

fn terminal_copy_shortcut_for(key: &Keystroke, platform: PasteShortcutPlatform) -> bool {
    if !key.key.eq_ignore_ascii_case("c") || key.modifiers.alt || key.modifiers.function {
        return false;
    }

    match platform {
        PasteShortcutPlatform::MacOs => {
            key.modifiers.platform && !key.modifiers.control && !key.modifiers.shift
        }
        PasteShortcutPlatform::Linux => {
            key.modifiers.control && key.modifiers.shift && !key.modifiers.platform
        }
        PasteShortcutPlatform::Windows => {
            key.modifiers.control && key.modifiers.shift && !key.modifiers.platform
        }
    }
}

fn terminal_paste_shortcut_for(key: &Keystroke, platform: PasteShortcutPlatform) -> bool {
    if !key.key.eq_ignore_ascii_case("v")
        || key.modifiers.alt
        || key.modifiers.function
        || key.modifiers.platform && !matches!(platform, PasteShortcutPlatform::MacOs)
    {
        return false;
    }

    match platform {
        PasteShortcutPlatform::MacOs => {
            key.modifiers.platform && !key.modifiers.control && !key.modifiers.shift
        }
        PasteShortcutPlatform::Linux => {
            key.modifiers.control && key.modifiers.shift && !key.modifiers.platform
        }
        PasteShortcutPlatform::Windows => key.modifiers.control && !key.modifiers.platform,
    }
}

#[cfg(test)]
fn pane_close_shortcut_for(key: &Keystroke, platform: PasteShortcutPlatform) -> bool {
    if !key.key.eq_ignore_ascii_case("w") || key.modifiers.alt || key.modifiers.function {
        return false;
    }

    match platform {
        PasteShortcutPlatform::MacOs => {
            key.modifiers.platform && key.modifiers.shift && !key.modifiers.control
        }
        PasteShortcutPlatform::Linux | PasteShortcutPlatform::Windows => {
            key.modifiers.control && key.modifiers.shift && !key.modifiers.platform
        }
    }
}

fn shortcut_matches(shortcut: &Shortcut, key: &Keystroke) -> bool {
    shortcut.key.eq_ignore_ascii_case(&key.key)
        && shortcut.control == key.modifiers.control
        && shortcut.alt == key.modifiers.alt
        && shortcut.shift == key.modifiers.shift
        && shortcut.platform == key.modifiers.platform
        && !key.modifiers.function
}

fn shortcut_from_keystroke(key: &Keystroke) -> Option<Shortcut> {
    if key.modifiers.function {
        return None;
    }
    let shortcut = Shortcut {
        key: key.key.to_string(),
        control: key.modifiers.control,
        alt: key.modifiers.alt,
        shift: key.modifiers.shift,
        platform: key.modifiers.platform,
    };
    shortcut.is_usable().then_some(shortcut)
}

fn terminal_sessions_for_target(
    hierarchy: &CoreSnapshot,
    target: CloseTarget,
) -> Vec<TerminalSessionId> {
    let layouts = hierarchy.spaces.iter().flat_map(|space| {
        space
            .tabs
            .iter()
            .filter(move |tab| match target {
                CloseTarget::Space(space_id) => space.id == space_id,
                CloseTarget::Tab(tab_id) => tab.id == tab_id,
                CloseTarget::Pane(_) => true,
            })
            .map(|tab| &tab.layout)
    });
    match target {
        CloseTarget::Pane(pane_id) => layouts
            .filter_map(|layout| terminal_for_pane(layout, pane_id))
            .collect(),
        CloseTarget::Space(_) | CloseTarget::Tab(_) => {
            let mut terminal_sessions = Vec::new();
            for layout in layouts {
                collect_terminal_sessions(layout, &mut terminal_sessions);
            }
            terminal_sessions
        }
    }
}

fn collect_terminal_sessions(layout: &PaneLayout, output: &mut Vec<TerminalSessionId>) {
    match layout {
        PaneLayout::Pane(pane) => output.push(pane.terminal_session_id),
        PaneLayout::Split(split) => {
            collect_terminal_sessions(&split.first, output);
            collect_terminal_sessions(&split.second, output);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SpaceAgentSummary {
    count: usize,
    visible: Vec<SpaceAgentEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SpaceAgentEntry {
    agent: AgentSnapshot,
    tab_id: TabId,
    pane_id: PaneId,
    terminal_session_id: TerminalSessionId,
}

fn agent_summary_for_space(
    space: &crate::SpaceSnapshot,
    terminals: &HashMap<TerminalSessionId, TerminalSnapshot>,
) -> Option<SpaceAgentSummary> {
    let mut agents = Vec::new();
    for tab in &space.tabs {
        let mut panes = Vec::new();
        collect_pane_terminals(&tab.layout, &mut panes);
        agents.extend(
            panes
                .into_iter()
                .filter_map(|(pane_id, terminal_session_id)| {
                    let agent = terminals.get(&terminal_session_id)?.agent?;
                    Some(SpaceAgentEntry {
                        agent,
                        tab_id: tab.id,
                        pane_id,
                        terminal_session_id,
                    })
                }),
        );
    }
    prioritized_agent_summary(agents)
}

fn prioritized_agent_summary(agents: Vec<SpaceAgentEntry>) -> Option<SpaceAgentSummary> {
    let count = agents.len();
    let mut agents = agents.into_iter().enumerate().collect::<Vec<_>>();
    agents.sort_by_key(|(position, entry)| (std::cmp::Reverse(entry.agent.priority()), *position));
    (count > 0).then(|| SpaceAgentSummary {
        count,
        visible: agents.into_iter().take(3).map(|(_, entry)| entry).collect(),
    })
}

fn collect_pane_terminals(layout: &PaneLayout, output: &mut Vec<(PaneId, TerminalSessionId)>) {
    match layout {
        PaneLayout::Pane(pane) => output.push((pane.id, pane.terminal_session_id)),
        PaneLayout::Split(split) => {
            collect_pane_terminals(&split.first, output);
            collect_pane_terminals(&split.second, output);
        }
    }
}

fn agent_state_label(state: AgentState) -> &'static str {
    match state {
        AgentState::Idle => "Waiting",
        AgentState::Working => "Working",
        AgentState::Blocked => "Needs attention",
        AgentState::Unknown => "Detected",
    }
}

fn agent_status_detail(state: AgentState) -> &'static str {
    match state {
        AgentState::Idle => "Ready for input",
        AgentState::Working => "Agent is active",
        AgentState::Blocked => "Waiting for your input",
        AgentState::Unknown => "Status unavailable",
    }
}

fn transitioning_agent_count_label(
    label: String,
    space_id: SpaceId,
    visible_count: usize,
    expanded: bool,
    animate: bool,
) -> AnyElement {
    let label = div().flex_none().child(label);
    if !animate {
        return label.into_any_element();
    }
    let icons_width = if visible_count == 0 {
        0.
    } else {
        22. + visible_count.saturating_sub(1) as f32 * 20.
    };
    let travel = icons_width + 7.;
    let animation_id = if expanded {
        "expand-agent-count"
    } else {
        "collapse-agent-count"
    };
    label
        .with_animation(
            (animation_id, space_id.as_u64()),
            Animation::new(Duration::from_millis(190)).with_easing(ease_out_quint()),
            move |this, delta| {
                let start = if expanded { travel } else { -travel };
                this.relative().left(px(start * (1. - delta)))
            },
        )
        .into_any_element()
}

fn transitioning_agent_chevron(
    space_id: SpaceId,
    expanded: bool,
    transition: Option<AgentLayoutTransition>,
    color: gpui::Rgba,
) -> AnyElement {
    let chevron = svg()
        .data(CHEVRON_RIGHT_ICON)
        .size(px(12.))
        .text_color(color);
    let Some(transition) = transition else {
        let rotation = if expanded { 0.25 } else { 0. };
        return chevron
            .with_transformation(Transformation::rotate(percentage(rotation)))
            .into_any_element();
    };
    let animation_id = match transition {
        AgentLayoutTransition::Expanding => "expand-agent-chevron",
        AgentLayoutTransition::Collapsing => "collapse-agent-chevron",
    };
    chevron
        .with_animation(
            (animation_id, space_id.as_u64()),
            Animation::new(Duration::from_millis(200)).with_easing(ease_out_quint()),
            move |chevron, delta| {
                let rotation = match transition {
                    AgentLayoutTransition::Expanding => delta * 0.25,
                    AgentLayoutTransition::Collapsing => (1. - delta) * 0.25,
                };
                chevron.with_transformation(Transformation::rotate(percentage(rotation)))
            },
        )
        .into_any_element()
}

fn transitioning_agent_section(
    section: gpui::Div,
    space_id: SpaceId,
    visible_count: usize,
    expanded: bool,
    transition: Option<AgentLayoutTransition>,
) -> AnyElement {
    let collapsed_height = 32.;
    let expanded_height = 33. + visible_count as f32 * 50.;
    let Some(transition) = transition else {
        return if expanded {
            section.into_any_element()
        } else {
            section
                .overflow_hidden()
                .h(px(collapsed_height))
                .into_any_element()
        };
    };
    let section = section.overflow_hidden();
    let (start, end, animation_id) = match transition {
        AgentLayoutTransition::Expanding => {
            (collapsed_height, expanded_height, "expand-agent-section")
        }
        AgentLayoutTransition::Collapsing => {
            (expanded_height, collapsed_height, "collapse-agent-section")
        }
    };
    section
        .with_animation(
            (animation_id, space_id.as_u64()),
            Animation::new(Duration::from_millis(200)).with_easing(ease_out_quint()),
            move |this, delta| this.h(px(start + (end - start) * delta)),
        )
        .into_any_element()
}

fn transitioning_agent_icon(
    entry: &SpaceAgentEntry,
    index: usize,
    expanding: bool,
    animate: bool,
) -> AnyElement {
    let program = entry.agent.program;
    let (resting_diameter, resting_top) = agent_icon_resting_geometry(expanding);
    let icon = div()
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .top(px(resting_top))
        .size(px(resting_diameter));
    if !animate {
        return icon
            .child(agent_icon(program, resting_diameter))
            .into_any_element();
    }
    let animation_id = if expanding {
        "expand-agent-icon"
    } else {
        "collapse-agent-icon"
    };
    icon.with_animation(
        (animation_id, entry.terminal_session_id.as_u64()),
        Animation::new(Duration::from_millis(190)).with_easing(ease_out_quint()),
        move |this, delta| {
            let (left, top, diameter) = agent_icon_transition_geometry(index, expanding, delta);
            this.left(px(left))
                .top(px(top))
                .child(agent_icon(program, diameter))
        },
    )
    .into_any_element()
}

fn agent_icon_resting_geometry(expanded: bool) -> (f32, f32) {
    if expanded { (30., 0.) } else { (22., 0.) }
}

fn agent_icon_transition_geometry(index: usize, expanding: bool, delta: f32) -> (f32, f32, f32) {
    let horizontal_travel = -1. + index as f32 * 20.;
    let vertical_travel = 33. + index as f32 * 50.;
    let inverse = 1. - delta;
    if expanding {
        (
            horizontal_travel * inverse,
            -vertical_travel * inverse,
            22. + 8. * delta,
        )
    } else {
        (
            -horizontal_travel * inverse,
            vertical_travel * inverse,
            30. - 8. * delta,
        )
    }
}

fn agent_icon(program: AgentProgram, diameter: f32) -> AnyElement {
    let background = match program {
        AgentProgram::Codex => rgb(0x101014),
        AgentProgram::Claude => rgb(0xd97757),
        AgentProgram::Gemini => rgb(0x4285f4),
    };

    div()
        .flex()
        .items_center()
        .justify_center()
        .size(px(diameter))
        .rounded_full()
        .bg(background)
        .child(
            svg()
                .data(agent_icon_data(program))
                .size_full()
                .text_color(rgb(0xffffff)),
        )
        .into_any_element()
}

fn agent_icon_data(program: AgentProgram) -> &'static [u8] {
    match program {
        AgentProgram::Codex => OPENAI_AGENT_ICON,
        AgentProgram::Claude => CLAUDE_AGENT_ICON,
        AgentProgram::Gemini => GEMINI_AGENT_ICON,
    }
}

fn terminal_key_input(key: &Keystroke) -> Option<ghostty::KeyInput> {
    if key.key.is_empty() {
        return None;
    }
    let text = key.key_char.as_ref().filter(|text| {
        !text.chars().any(|character| {
            character.is_control() || ('\u{f700}'..='\u{f8ff}').contains(&character)
        })
    });
    Some(ghostty::KeyInput {
        key: key.key.clone(),
        text: text.cloned(),
        modifiers: u16::from(key.modifiers.shift)
            | (u16::from(key.modifiers.control) << 1)
            | (u16::from(key.modifiers.alt) << 2)
            | (u16::from(key.modifiers.platform) << 3),
    })
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
    terminal_font: &TerminalFont,
    terminal_glyph_overflow: TerminalGlyphOverflow,
    cursor_x: Option<u16>,
    window: &mut Window,
) -> gpui::Result<()> {
    let mut cell_layouts = vec![CellGlyphLayout::default(); row.glyph_cells.len()];
    let mut positioned_glyphs = Vec::with_capacity(row.glyph_cells.len());
    for run in &line.runs {
        for glyph in &run.glyphs {
            if let Some(cell_index) = row.glyph_cell_index(glyph.index) {
                let layout = &mut cell_layouts[cell_index];
                layout.natural_x.get_or_insert(f32::from(glyph.position.x));
                match layout.font_id {
                    Some(font_id) if font_id != run.font_id => {
                        layout.mixed_fonts = true;
                    }
                    None => layout.font_id = Some(run.font_id),
                    _ => {}
                }
                layout.symbol_fallback |= terminal_font.is_symbol_fallback(run.font_id);
                positioned_glyphs.push((cell_index, run.font_id, glyph));
            }
        }
    }

    for (cell_index, font_id, glyph) in &positioned_glyphs {
        let layout = &mut cell_layouts[*cell_index];
        // Only the known Symbols Nerd Font faces use this source-scalar width policy. Other
        // fallback faces may substitute several source characters into unrelated glyph IDs;
        // keeping their native shaped geometry avoids remeasuring the wrong glyph.
        if !layout.symbol_fallback
            || *font_id == terminal_font.primary_font_id
            || layout.mixed_fonts
            || layout.invalid_bounds
        {
            continue;
        }
        let Some(character) = row
            .text
            .get(glyph.index..)
            .and_then(|text| text.chars().next())
        else {
            layout.invalid_bounds = true;
            layout.bounds = None;
            continue;
        };
        let (Ok(bounds), Ok(advance)) = (
            window
                .text_system()
                .typographic_bounds(*font_id, line.font_size, character),
            window
                .text_system()
                .advance(*font_id, line.font_size, character),
        ) else {
            layout.invalid_bounds = true;
            layout.bounds = None;
            continue;
        };
        let natural_cell_x = layout.natural_x.expect("positioned glyph has an origin");
        let relative_x = f32::from(glyph.position.x) - natural_cell_x;
        let bounds_left = f32::from(bounds.origin.x);
        let next = FallbackClusterBounds {
            left: relative_x + bounds_left.min(0.),
            right: relative_x
                + f32::from(advance.width).max(bounds_left + f32::from(bounds.size.width)),
        };
        layout.bounds = Some(
            layout
                .bounds
                .map(|current| current.union(next))
                .unwrap_or(next),
        );
    }
    for (cell_index, layout) in cell_layouts.iter_mut().enumerate() {
        if let Some(bounds) = layout.bounds {
            let allocated_width_px =
                terminal_font.cells.width_px() * f32::from(row.glyph_cells[cell_index].width);
            layout.fitted = Some(layout_symbol_fallback_cluster(
                f32::from(line.font_size),
                terminal_font.primary_baseline_px,
                terminal_font.cells.width_px(),
                allocated_width_px,
                row_cell_is_followed_by_space(row, cell_index, cursor_x),
                terminal_glyph_overflow,
                bounds,
            ));
        }
    }

    for run in &line.runs {
        for glyph in &run.glyphs {
            let Some(cell_index) = row.glyph_cell_index(glyph.index) else {
                continue;
            };
            let glyph_cell = &row.glyph_cells[cell_index];
            let cell_layout = &cell_layouts[cell_index];
            let Some(natural_cell_x) = cell_layout.natural_x else {
                continue;
            };
            let relative_x = f32::from(glyph.position.x) - natural_cell_x;
            let relative_y = f32::from(glyph.position.y);
            let (glyph_x, glyph_y, glyph_size) = if let Some(geometry) = cell_layout.fitted {
                let (glyph_x, glyph_y) = fitted_cluster_glyph_position(
                    f32::from(glyph_cell.x) * terminal_font.cells.width_px(),
                    relative_x,
                    relative_y,
                    geometry,
                );
                (glyph_x, glyph_y, px(geometry.font_size_px))
            } else {
                (
                    fixed_cell_glyph_x(
                        glyph_cell.x,
                        terminal_font.cells.width_px(),
                        f32::from(glyph.position.x),
                        natural_cell_x,
                    ),
                    terminal_font.primary_baseline_px + relative_y,
                    line.font_size,
                )
            };
            let glyph_origin = point(origin.x + px(glyph_x), origin.y + px(glyph_y));
            if glyph.is_emoji {
                window.paint_emoji(glyph_origin, run.font_id, glyph.id, glyph_size)?;
            } else {
                window.paint_glyph(
                    glyph_origin,
                    run.font_id,
                    glyph.id,
                    glyph_size,
                    color(glyph_cell.color).into(),
                )?;
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct CellGlyphLayout {
    natural_x: Option<f32>,
    font_id: Option<FontId>,
    mixed_fonts: bool,
    symbol_fallback: bool,
    invalid_bounds: bool,
    bounds: Option<FallbackClusterBounds>,
    fitted: Option<FittedFallbackCluster>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct FittedFallbackCluster {
    font_size_px: f32,
    scale: f32,
    x_offset_px: f32,
    baseline_px: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct FallbackClusterBounds {
    left: f32,
    right: f32,
}

impl FallbackClusterBounds {
    fn union(self, other: Self) -> Self {
        Self {
            left: self.left.min(other.left),
            right: self.right.max(other.right),
        }
    }
}

fn fitted_cluster_glyph_position(
    cell_anchor_x: f32,
    relative_x: f32,
    relative_y: f32,
    geometry: FittedFallbackCluster,
) -> (f32, f32) {
    (
        cell_anchor_x + geometry.x_offset_px + relative_x * geometry.scale,
        geometry.baseline_px + relative_y * geometry.scale,
    )
}

fn row_cell_is_followed_by_space(row: &FrameRow, cell_index: usize, cursor_x: Option<u16>) -> bool {
    let Some(cell) = row.glyph_cells.get(cell_index) else {
        return false;
    };
    let next_x = cell.x.saturating_add(u16::from(cell.width));
    row.glyph_cells
        .get(cell_index + 1)
        .filter(|next| next.x == next_x)
        .filter(|next| cursor_x != Some(next.x))
        .and_then(|next| row.text.get(next.byte_range.clone()))
        .is_some_and(|text| !text.is_empty() && text.chars().all(|character| character == ' '))
}

fn layout_symbol_fallback_cluster(
    font_size_px: f32,
    primary_baseline_px: f32,
    cell_width_px: f32,
    allocated_width_px: f32,
    followed_by_space: bool,
    overflow: TerminalGlyphOverflow,
    bounds: FallbackClusterBounds,
) -> FittedFallbackCluster {
    let footprint_width = (bounds.right - bounds.left).max(1.);
    let maximum_width_px = allocated_width_px + cell_width_px * SYMBOL_OVERFLOW_TOLERANCE_CELLS;
    let scale = if overflow.allows(followed_by_space) {
        1.
    } else {
        1_f32.min(maximum_width_px / footprint_width)
    };

    FittedFallbackCluster {
        font_size_px: font_size_px * scale,
        scale,
        x_offset_px: if scale < 1. { -bounds.left * scale } else { 0. },
        baseline_px: primary_baseline_px,
    }
}

fn color(rgb_bytes: [u8; 3]) -> gpui::Rgba {
    rgb((u32::from(rgb_bytes[0]) << 16) | (u32::from(rgb_bytes[1]) << 8) | u32::from(rgb_bytes[2]))
}

fn coalesce_scroll_input(pending: &mut Vec<ghostty::ScrollInput>, input: ghostty::ScrollInput) {
    if let Some(previous) = pending.last_mut()
        && scroll_inputs_are_compatible(*previous, input)
    {
        previous.delta_rows = previous.delta_rows.saturating_add(input.delta_rows);
        previous.delta_columns = previous.delta_columns.saturating_add(input.delta_columns);
        return;
    }
    pending.push(input);
}

fn scroll_inputs_are_compatible(
    previous: ghostty::ScrollInput,
    next: ghostty::ScrollInput,
) -> bool {
    same_scroll_direction(previous.delta_rows, next.delta_rows)
        && same_scroll_direction(previous.delta_columns, next.delta_columns)
        && previous.pointer_x.to_bits() == next.pointer_x.to_bits()
        && previous.pointer_y.to_bits() == next.pointer_y.to_bits()
        && previous.viewport_width == next.viewport_width
        && previous.viewport_height == next.viewport_height
        && previous.cell_width == next.cell_width
        && previous.cell_height == next.cell_height
        && previous.modifiers == next.modifiers
}

fn same_scroll_direction(previous: isize, next: isize) -> bool {
    previous == 0 || next == 0 || previous.signum() == next.signum()
}

fn normalize_page_scroll_delta(delta: f32, page_cells: u16) -> f32 {
    if delta.abs() >= WINDOWS_PAGE_SCROLL_SENTINEL_MIN {
        delta.signum() * f32::from(page_cells.max(1))
    } else {
        delta
    }
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
        CLAUDE_AGENT_ICON, FallbackClusterBounds, GEMINI_AGENT_ICON, OPENAI_AGENT_ICON,
        PasteShortcutPlatform, SpaceAgentEntry, SplitGeometry, TERMINAL_PADDING_PX,
        TerminalGlyphOverflow, UiSelection, accept_terminal_snapshot, agent_icon_data,
        agent_icon_resting_geometry, agent_icon_transition_geometry, cursor_rects, first_pane_id,
        fitted_cluster_glyph_position, font_is_terminal_fallback_only,
        installed_terminal_font_fallbacks_from, layout_symbol_fallback_cluster,
        normalize_page_scroll_delta, pane_close_shortcut_for, pane_extents,
        prioritized_agent_summary, projected_split_extent, row_cell_is_followed_by_space,
        selection_for_created, selection_for_pane, split_ratio_at, terminal_copy_shortcut_for,
        terminal_font_with_fallbacks, terminal_key_input, terminal_paste_shortcut_for,
        windows_caption_font_for_build,
    };
    use crate::{
        AgentProgram, AgentSnapshot, AgentState, CoreCommand, CoreModel, CreatedResource, PaneId,
        PaneLayout, SplitAxis, SplitPlacement, SplitRatio, TabId, TerminalCursorShape,
        TerminalLifecycle, TerminalSessionId, TerminalSize, TerminalSnapshot,
        ghostty::ScrollInput,
        terminal_frame::{FrameRow, GlyphCell},
    };
    use gpui::{Keystroke, Modifiers};
    use std::collections::HashMap;

    #[test]
    fn windows_page_scroll_sentinel_becomes_one_visible_page() {
        assert_eq!(normalize_page_scroll_delta(u32::MAX as f32, 24), 24.0);
        assert_eq!(normalize_page_scroll_delta(-(u32::MAX as f32), 80), -80.0);
        assert_eq!(normalize_page_scroll_delta(120.0, 24), 120.0);
    }

    #[test]
    fn precision_scroll_burst_coalesces_to_one_terminal_operation() {
        let mut pending = Vec::new();
        for _ in 0..128 {
            super::coalesce_scroll_input(&mut pending, scroll_input(-1, 0, 0));
        }

        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].delta_rows, -128);
    }

    #[test]
    fn scroll_direction_and_modifier_changes_remain_ordering_boundaries() {
        let mut pending = Vec::new();
        super::coalesce_scroll_input(&mut pending, scroll_input(-2, 0, 0));
        super::coalesce_scroll_input(&mut pending, scroll_input(1, 0, 0));
        super::coalesce_scroll_input(&mut pending, scroll_input(1, 0, 1));

        assert_eq!(pending.len(), 3);
        assert_eq!(pending[0].delta_rows, -2);
        assert_eq!(pending[1].delta_rows, 1);
        assert_eq!(pending[2].modifiers, 1);
    }

    #[test]
    fn shaped_rows_are_reused_when_scrollback_moves_them() {
        let first = FrameRow {
            text: "first".into(),
            runs: Vec::new(),
            glyph_cells: Vec::new(),
        };
        let second = FrameRow {
            text: "second".into(),
            runs: Vec::new(),
            glyph_cells: Vec::new(),
        };
        let previous = vec![first, second.clone()];

        assert_eq!(
            super::reusable_shaped_row_index(&previous, &second, 0),
            Some(1)
        );
        assert_eq!(
            super::reusable_shaped_row_index(
                &previous,
                &FrameRow {
                    text: "new".into(),
                    runs: Vec::new(),
                    glyph_cells: Vec::new(),
                },
                0,
            ),
            None
        );
    }

    fn scroll_input(delta_rows: isize, delta_columns: isize, modifiers: u16) -> ScrollInput {
        ScrollInput {
            delta_rows,
            delta_columns,
            pointer_x: 12.,
            pointer_y: 18.,
            viewport_width: 800,
            viewport_height: 480,
            cell_width: 10,
            cell_height: 20,
            modifiers,
        }
    }

    #[test]
    fn cursor_shapes_use_fixed_cell_geometry() {
        assert_eq!(
            cursor_rects(TerminalCursorShape::Bar, 10., 20.),
            vec![super::CursorRect {
                x: 0.,
                y: 0.,
                width: 2.,
                height: 20.,
            }]
        );
        assert_eq!(
            cursor_rects(TerminalCursorShape::Underline, 10., 20.),
            vec![super::CursorRect {
                x: 0.,
                y: 18.,
                width: 10.,
                height: 2.,
            }]
        );
        assert_eq!(
            cursor_rects(TerminalCursorShape::BlockHollow, 10., 20.).len(),
            4
        );
    }

    #[test]
    fn cursor_visibility_respects_focus_and_terminal_blink_mode() {
        assert!(super::cursor_should_be_visible(false, false, false));
        assert!(super::cursor_should_be_visible(false, true, true));
        assert!(!super::cursor_should_be_visible(false, true, false));
        assert!(!super::cursor_should_be_visible(true, false, true));
    }

    #[test]
    fn cursor_blink_reset_is_isolated_to_the_focused_terminal() {
        let focused = TerminalSessionId::from_u64(7);
        let background = TerminalSessionId::from_u64(8);

        assert!(super::cursor_blink_resets_for_terminal(
            Some(focused),
            focused
        ));
        assert!(!super::cursor_blink_resets_for_terminal(
            Some(focused),
            background
        ));
    }

    #[test]
    fn cursor_blink_reset_restarts_the_full_visible_interval() {
        let start = std::time::Instant::now();
        let mut blink = super::CursorBlinkState::new(start);
        blink.reset(start + std::time::Duration::from_millis(450));

        assert!(!blink.update_at(start + std::time::Duration::from_millis(500), true));
        assert!(blink.visible);
        assert!(blink.update_at(start + std::time::Duration::from_millis(950), true));
        assert!(!blink.visible);
    }

    #[test]
    fn terminal_font_fallbacks_use_available_nerd_families_in_priority_order() {
        let available = vec![
            "Symbols Nerd Font".to_owned(),
            "Victor Mono".to_owned(),
            "SYMBOLS NERD FONT MONO".to_owned(),
        ];

        assert_eq!(
            installed_terminal_font_fallbacks_from(
                "Victor Mono",
                &available,
                &["Symbols Nerd Font Mono", "Symbols Nerd Font"],
            ),
            vec![
                "SYMBOLS NERD FONT MONO".to_owned(),
                "Symbols Nerd Font".to_owned()
            ]
        );
        assert_eq!(
            installed_terminal_font_fallbacks_from(
                "Symbols Nerd Font Mono",
                &available,
                &["Symbols Nerd Font Mono", "Symbols Nerd Font"],
            ),
            vec!["Symbols Nerd Font".to_owned()]
        );
    }

    #[test]
    fn terminal_symbol_fallbacks_are_not_primary_font_choices() {
        assert!(font_is_terminal_fallback_only("Symbols Nerd Font Mono"));
        assert!(font_is_terminal_fallback_only("symbols nerd font"));
        assert!(!font_is_terminal_fallback_only("Victor Mono"));
    }

    #[test]
    fn terminal_font_preserves_the_primary_and_attaches_ordered_fallbacks() {
        let fallbacks = vec![
            "Symbols Nerd Font Mono".to_owned(),
            "Symbols Nerd Font".to_owned(),
        ];
        let terminal_font = terminal_font_with_fallbacks("Victor Mono", &fallbacks);

        assert_eq!(terminal_font.family.as_ref(), "Victor Mono");
        assert_eq!(
            terminal_font
                .fallbacks
                .expect("terminal fallback chain should be present")
                .fallback_list(),
            fallbacks
        );
    }

    #[test]
    fn every_glyph_in_a_fallback_cluster_uses_one_shared_transform() {
        let fitted = layout_symbol_fallback_cluster(
            14.,
            15.,
            8.,
            8.,
            false,
            TerminalGlyphOverflow::Never,
            FallbackClusterBounds {
                left: 0.,
                right: 16.,
            },
        );
        let first = fitted_cluster_glyph_position(24., 1., -2., fitted);
        let second = fitted_cluster_glyph_position(24., 5., 2., fitted);

        assert_eq!(second.0 - first.0, 2.5);
        assert_eq!(second.1 - first.1, 2.5);
    }

    #[test]
    fn symbol_fallbacks_keep_their_natural_size_when_followed_by_space() {
        let layout = layout_symbol_fallback_cluster(
            14.,
            15.,
            8.,
            8.,
            true,
            TerminalGlyphOverflow::WhenFollowedBySpace,
            FallbackClusterBounds {
                left: -1.,
                right: 15.,
            },
        );

        assert_eq!(layout.font_size_px, 14.);
        assert_eq!(layout.scale, 1.);
        assert_eq!(layout.x_offset_px, 0.);
        assert_eq!(layout.baseline_px, 15.);
    }

    #[test]
    fn symbol_fallbacks_fit_width_but_not_height_when_the_next_cell_is_occupied() {
        let layout = layout_symbol_fallback_cluster(
            14.,
            15.,
            8.,
            8.,
            false,
            TerminalGlyphOverflow::WhenFollowedBySpace,
            FallbackClusterBounds {
                left: -1.,
                right: 15.,
            },
        );

        assert!((layout.scale - 0.625).abs() < 0.001);
        assert!((layout.font_size_px - 8.75).abs() < 0.001);
        assert!((layout.x_offset_px - 0.625).abs() < 0.001);
        assert_eq!(layout.baseline_px, 15.);
    }

    #[test]
    fn symbol_overflow_policy_distinguishes_always_and_never() {
        let bounds = FallbackClusterBounds {
            left: 0.,
            right: 16.,
        };
        let always = layout_symbol_fallback_cluster(
            14.,
            15.,
            8.,
            8.,
            false,
            TerminalGlyphOverflow::Always,
            bounds,
        );
        let never = layout_symbol_fallback_cluster(
            14.,
            15.,
            8.,
            8.,
            true,
            TerminalGlyphOverflow::Never,
            bounds,
        );

        assert_eq!(always.scale, 1.);
        assert!((never.scale - 0.625).abs() < 0.001);
    }

    #[test]
    fn overflow_safety_uses_the_ghostty_cell_after_the_allocated_span() {
        let row = FrameRow {
            text: "\u{f120} A".to_owned(),
            runs: Vec::new(),
            glyph_cells: vec![
                GlyphCell {
                    x: 0,
                    width: 1,
                    byte_range: 0..3,
                    color: [0; 3],
                },
                GlyphCell {
                    x: 1,
                    width: 1,
                    byte_range: 3..4,
                    color: [0; 3],
                },
                GlyphCell {
                    x: 2,
                    width: 1,
                    byte_range: 4..5,
                    color: [0; 3],
                },
            ],
        };

        assert!(row_cell_is_followed_by_space(&row, 0, None));
        assert!(!row_cell_is_followed_by_space(&row, 0, Some(1)));
        assert!(!row_cell_is_followed_by_space(&row, 1, None));
        assert!(!row_cell_is_followed_by_space(&row, 2, None));
    }

    #[test]
    fn expanded_agent_icons_rest_on_the_row_centerline() {
        assert_eq!(agent_icon_resting_geometry(true), (30., 0.));
        assert_eq!(agent_icon_transition_geometry(0, true, 1.), (0., 0., 30.));
        assert_eq!(agent_icon_transition_geometry(1, false, 1.), (0., 0., 22.));
    }

    #[test]
    fn agent_marks_use_one_shared_vector_canvas() {
        for (program, data) in [
            (AgentProgram::Codex, OPENAI_AGENT_ICON),
            (AgentProgram::Claude, CLAUDE_AGENT_ICON),
            (AgentProgram::Gemini, GEMINI_AGENT_ICON),
        ] {
            let svg = std::str::from_utf8(data).expect("agent mark is UTF-8 SVG");
            assert!(svg.contains("viewBox=\"0 0 64 64\""));
            assert!(svg.contains("<path"));
            assert!(!svg.contains("<circle"));
            assert_eq!(agent_icon_data(program), data);
        }
    }

    #[test]
    fn compact_agent_summary_keeps_count_and_shows_top_three_by_priority() {
        let agent = |id, program, state| SpaceAgentEntry {
            agent: AgentSnapshot { program, state },
            tab_id: TabId::from_u64(id),
            pane_id: PaneId::from_u64(id),
            terminal_session_id: TerminalSessionId::from_u64(id),
        };
        let summary = prioritized_agent_summary(vec![
            agent(1, AgentProgram::Gemini, AgentState::Unknown),
            agent(2, AgentProgram::Claude, AgentState::Idle),
            agent(3, AgentProgram::Codex, AgentState::Blocked),
            agent(4, AgentProgram::Gemini, AgentState::Working),
        ])
        .expect("agents produce a summary");

        assert_eq!(summary.count, 4);
        assert_eq!(summary.visible.len(), 3);
        assert_eq!(summary.visible[0].agent.state, AgentState::Blocked);
        assert_eq!(summary.visible[1].agent.state, AgentState::Working);
        assert_eq!(summary.visible[2].agent.state, AgentState::Idle);
    }

    #[test]
    fn named_space_key_is_forwarded_to_ghostty() {
        let key = Keystroke {
            key: "space".into(),
            key_char: None,
            ..Default::default()
        };

        assert_eq!(
            terminal_key_input(&key),
            Some(crate::ghostty::KeyInput {
                key: "space".into(),
                text: None,
                modifiers: 0,
            })
        );
    }

    #[test]
    fn paste_shortcuts_follow_each_desktop_convention() {
        let key = |modifiers| Keystroke {
            key: "v".into(),
            key_char: None,
            modifiers,
        };
        let command_v = key(Modifiers {
            platform: true,
            ..Default::default()
        });
        let control_v = key(Modifiers {
            control: true,
            ..Default::default()
        });
        let control_shift_v = key(Modifiers {
            control: true,
            shift: true,
            ..Default::default()
        });

        assert!(terminal_paste_shortcut_for(
            &command_v,
            PasteShortcutPlatform::MacOs
        ));
        assert!(terminal_paste_shortcut_for(
            &control_shift_v,
            PasteShortcutPlatform::Linux
        ));
        assert!(terminal_paste_shortcut_for(
            &control_v,
            PasteShortcutPlatform::Windows
        ));
        assert!(terminal_paste_shortcut_for(
            &control_shift_v,
            PasteShortcutPlatform::Windows
        ));
        assert!(!terminal_paste_shortcut_for(
            &control_v,
            PasteShortcutPlatform::Linux
        ));
    }

    #[test]
    fn copy_shortcuts_follow_each_desktop_convention() {
        let key = |modifiers| Keystroke {
            key: "c".into(),
            key_char: None,
            modifiers,
        };
        let command_c = key(Modifiers {
            platform: true,
            ..Default::default()
        });
        let control_c = key(Modifiers {
            control: true,
            ..Default::default()
        });
        let control_shift_c = key(Modifiers {
            control: true,
            shift: true,
            ..Default::default()
        });

        assert!(terminal_copy_shortcut_for(
            &command_c,
            PasteShortcutPlatform::MacOs
        ));
        assert!(terminal_copy_shortcut_for(
            &control_shift_c,
            PasteShortcutPlatform::Linux
        ));
        assert!(terminal_copy_shortcut_for(
            &control_shift_c,
            PasteShortcutPlatform::Windows
        ));
        assert!(!terminal_copy_shortcut_for(
            &control_c,
            PasteShortcutPlatform::Linux
        ));
        assert!(!terminal_copy_shortcut_for(
            &control_c,
            PasteShortcutPlatform::Windows
        ));
    }

    #[test]
    fn pane_close_shortcut_is_deliberate_on_every_desktop() {
        let key = |modifiers| Keystroke {
            key: "w".into(),
            key_char: None,
            modifiers,
        };
        let command_shift_w = key(Modifiers {
            platform: true,
            shift: true,
            ..Default::default()
        });
        let control_shift_w = key(Modifiers {
            control: true,
            shift: true,
            ..Default::default()
        });
        let control_w = key(Modifiers {
            control: true,
            ..Default::default()
        });

        assert!(pane_close_shortcut_for(
            &command_shift_w,
            PasteShortcutPlatform::MacOs
        ));
        assert!(pane_close_shortcut_for(
            &control_shift_w,
            PasteShortcutPlatform::Linux
        ));
        assert!(pane_close_shortcut_for(
            &control_shift_w,
            PasteShortcutPlatform::Windows
        ));
        assert!(!pane_close_shortcut_for(
            &control_w,
            PasteShortcutPlatform::Windows
        ));
    }

    #[test]
    fn control_c_remains_terminal_input() {
        let key = Keystroke {
            key: "c".into(),
            key_char: None,
            modifiers: Modifiers {
                control: true,
                ..Default::default()
            },
        };

        assert_eq!(
            terminal_key_input(&key),
            Some(crate::ghostty::KeyInput {
                key: "c".into(),
                text: None,
                modifiers: 1 << 1,
            })
        );
    }

    #[test]
    fn alt_word_controls_are_forwarded_to_ghostty() {
        for name in ["left", "backspace"] {
            let key = Keystroke {
                key: name.into(),
                key_char: None,
                modifiers: Modifiers {
                    alt: true,
                    ..Default::default()
                },
            };

            assert_eq!(
                terminal_key_input(&key),
                Some(crate::ghostty::KeyInput {
                    key: name.into(),
                    text: None,
                    modifiers: 1 << 2,
                })
            );
        }
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
    fn recursive_split_extents_project_authoritative_ratios_to_cell_boundaries() {
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
        assert!((extents[0].1 - 596.0).abs() < 0.1);
        assert!((extents[1].1 - 399.0).abs() < 0.1);
        assert_eq!((extents[0].1 - TERMINAL_PADDING_PX * 2.0) % 9.0, 0.0);
        assert_eq!(extents[0].2, 500.0);
        assert_eq!(extents[1].2, 500.0);
    }

    #[test]
    fn bounded_split_extents_remain_on_the_terminal_cell_lattice() {
        let length = 800.0;
        let cell_step = 9.0;
        let terminal_inset = TERMINAL_PADDING_PX * 2.0;

        for ratio in [0.0, 0.8975, 0.9, 1.0] {
            let extent = projected_split_extent(length, ratio, cell_step);
            assert!(extent >= length * 0.1);
            assert!(extent <= length * 0.9);
            assert_eq!((extent - terminal_inset) % cell_step, 0.0);
        }

        assert_eq!(projected_split_extent(length, 0.8975, cell_step), 713.0);
        assert_eq!(projected_split_extent(length, 0.9, cell_step), 713.0);
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
            None,
            TerminalSnapshot {
                revision: 1,
                lifecycle: TerminalLifecycle::Running,
                active_work: false,
                title: None,
                agent: None,
                cols: 1,
                rows: 1,
                cursor: None,
                cursor_shape: TerminalCursorShape::Block,
                cursor_blinking: false,
                cursor_wide_tail: false,
                default_fg: [0xdd; 3],
                default_bg: [0x11; 3],
                selection_text: None,
                cells: Vec::new(),
            },
        );

        assert!(!terminals.contains_key(&terminal_session_id));
    }

    #[test]
    fn superseded_terminal_geometry_cannot_replace_the_displayed_projection() {
        let directory = std::env::current_dir().expect("current directory");
        let mut model = CoreModel::new();
        let hierarchy = model
            .apply(
                0,
                CoreCommand::CreateSpace {
                    name: "Space".into(),
                    directory,
                },
            )
            .expect("create Space")
            .snapshot;
        let terminal_session_id = hierarchy.terminal_sessions[0].id;
        let snapshot = |revision, cols| TerminalSnapshot {
            revision,
            lifecycle: TerminalLifecycle::Running,
            active_work: false,
            title: None,
            agent: None,
            cols,
            rows: 69,
            cursor: Some((0, 28)),
            cursor_shape: TerminalCursorShape::Block,
            cursor_blinking: false,
            cursor_wide_tail: false,
            default_fg: [0xdd; 3],
            default_bg: [0x11; 3],
            selection_text: None,
            cells: Vec::new(),
        };
        let mut terminals = HashMap::from([(terminal_session_id, snapshot(20, 143))]);
        let mut terminal_errors = HashMap::new();

        accept_terminal_snapshot(
            &hierarchy,
            &mut terminals,
            &mut terminal_errors,
            terminal_session_id,
            Some(TerminalSize::new(145, 69, 9, 20)),
            snapshot(21, 144),
        );

        assert_eq!(terminals[&terminal_session_id].cols, 143);

        accept_terminal_snapshot(
            &hierarchy,
            &mut terminals,
            &mut terminal_errors,
            terminal_session_id,
            Some(TerminalSize::new(145, 69, 9, 20)),
            snapshot(22, 145),
        );

        assert_eq!(terminals[&terminal_session_id].cols, 145);
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
            first_extent: 560.,
            cell_step: 9.,
        };

        assert_eq!(split_ratio_at(geometry, 780.).parts_per_thousand(), 700);
        assert_eq!(split_ratio_at(geometry, 0.).parts_per_thousand(), 104);
        assert_eq!(split_ratio_at(geometry, 2_000.).parts_per_thousand(), 891);
    }

    #[test]
    fn dragging_a_split_seam_only_moves_on_terminal_cell_boundaries() {
        let geometry = SplitGeometry {
            axis: SplitAxis::Horizontal,
            start: 220.,
            length: 800.,
            first_extent: 560.,
            cell_step: 9.,
        };

        let ratios = (780..785)
            .map(|pointer| split_ratio_at(geometry, pointer as f32))
            .collect::<Vec<_>>();

        assert!(
            ratios.windows(2).all(|pair| pair[0] == pair[1]),
            "sub-cell pointer movement must not move the divider independently of the terminal grid"
        );
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
