use gpui::{
    App, Bounds, Pixels, Rgba, TitlebarOptions, WindowBackgroundAppearance, WindowBounds,
    WindowDecorations, WindowOptions, div, prelude::*, px, rgb,
};
use std::borrow::Cow;

use crate::settings::ThemePreset;

#[derive(Clone, Copy, Debug)]
pub(crate) struct WorkspaceShell {
    appearance: WindowBackgroundAppearance,
    opacity: f32,
    theme: ThemePreset,
}

#[derive(Clone, Copy)]
pub(crate) enum ShellColor {
    Window,
    Chrome,
    Sidebar,
    Border,
    Text,
    MutedText,
    FaintText,
    Hover,
    Selected,
    SelectedBorder,
    Accent,
    AccentMuted,
    Danger,
    DangerHover,
}

#[derive(Clone, Copy)]
pub(crate) enum ShellIcon {
    AppMark,
    Plus,
    SplitHorizontal,
    SplitVertical,
    Move,
    Settings,
    SidebarClose,
    SidebarOpen,
    Close,
    ChevronRight,
    ChevronDown,
}

#[derive(Clone, Copy)]
struct ThemePalette {
    window: u32,
    chrome: u32,
    sidebar: u32,
    border: u32,
    text: u32,
    muted_text: u32,
    faint_text: u32,
    hover: u32,
    selected: u32,
    selected_border: u32,
    accent: u32,
    accent_muted: u32,
    danger: u32,
    danger_hover: u32,
}

impl WorkspaceShell {
    const DEFAULT_OPACITY: f32 = 0.65;
    pub(crate) const TITLE_BAR_HEIGHT: f32 = 40.;
    pub(crate) const SIDEBAR_WIDTH: f32 = 220.;
    pub(crate) const SIDEBAR_MIN_WIDTH: f32 = 180.;
    pub(crate) const CHROME_TILE_SIZE: f32 = 32.;
    pub(crate) const CHROME_ICON_SIZE: f32 = 13.;
    pub(crate) const TAB_HEIGHT: f32 = 30.;
    pub(crate) const WINDOW_CONTROLS_WIDTH: f32 = if cfg!(target_os = "macos") { 0. } else { 138. };

    pub(crate) fn from_preferences(theme: ThemePreset, opacity: f32) -> Self {
        Self::resolve(platform_appearance(), theme, opacity)
    }

    pub(crate) fn appearance(&self) -> WindowBackgroundAppearance {
        self.appearance
    }

    pub(crate) fn window_options(&self, bounds: Bounds<Pixels>) -> WindowOptions {
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Agent Terminal".into()),
                appears_transparent: true,
                traffic_light_position: Some(gpui::point(px(14.), px(12.))),
            }),
            window_background: self.appearance,
            window_decorations: Some(WindowDecorations::Client),
            window_min_size: Some(gpui::size(px(720.), px(440.))),
            ..Default::default()
        }
    }

    pub(crate) fn color(&self, role: ShellColor) -> Rgba {
        let color = self.base_color(role);
        if !self.is_translucent() {
            return color;
        }

        color.alpha(match role {
            ShellColor::Window | ShellColor::Chrome | ShellColor::Sidebar => self.opacity,
            ShellColor::Hover => 0.45,
            ShellColor::Selected => 0.28,
            ShellColor::AccentMuted => 0.32,
            ShellColor::DangerHover => 0.3,
            ShellColor::Border => 0.5,
            ShellColor::SelectedBorder => 0.58,
            ShellColor::Text
            | ShellColor::MutedText
            | ShellColor::FaintText
            | ShellColor::Accent
            | ShellColor::Danger => 1.,
        })
    }

    pub(crate) fn opaque_color(&self, role: ShellColor) -> Rgba {
        self.base_color(role)
    }

    pub(crate) fn control_hover(&self) -> Rgba {
        self.base_color(ShellColor::Text).alpha(0.16)
    }

    pub(crate) fn icon(&self, icon: ShellIcon, color: Rgba, size: f32) -> gpui::Div {
        div()
            .flex()
            .items_center()
            .justify_center()
            .size(px(size))
            .font_family("lucide")
            .text_size(px(size))
            .text_color(color)
            .child(char::from(icon.lucide()).to_string())
    }

    pub(crate) fn terminal_background(&self, color: Rgba) -> Rgba {
        if self.is_translucent() {
            color.alpha(self.opacity)
        } else {
            color
        }
    }

    pub(crate) fn root_color(&self) -> Rgba {
        if self.is_translucent() {
            self.base_color(ShellColor::Window).alpha(0.)
        } else {
            self.color(ShellColor::Window)
        }
    }

    fn resolve(
        appearance: WindowBackgroundAppearance,
        theme: ThemePreset,
        requested_opacity: f32,
    ) -> Self {
        let mut opacity = if requested_opacity.is_finite() {
            requested_opacity.clamp(0.45, 1.)
        } else {
            Self::DEFAULT_OPACITY
        };
        let appearance = if opacity >= 1. {
            WindowBackgroundAppearance::Opaque
        } else {
            appearance
        };
        if appearance == WindowBackgroundAppearance::Opaque {
            opacity = 1.;
        }
        Self {
            appearance,
            opacity,
            theme,
        }
    }

    fn is_translucent(&self) -> bool {
        self.appearance != WindowBackgroundAppearance::Opaque && self.opacity < 1.
    }

    fn base_color(&self, role: ShellColor) -> Rgba {
        let palette = theme_palette(self.theme);
        rgb(match role {
            ShellColor::Window => palette.window,
            ShellColor::Chrome => palette.chrome,
            ShellColor::Sidebar => palette.sidebar,
            ShellColor::Border => palette.border,
            ShellColor::Text => palette.text,
            ShellColor::MutedText => palette.muted_text,
            ShellColor::FaintText => palette.faint_text,
            ShellColor::Hover => palette.hover,
            ShellColor::Selected => palette.selected,
            ShellColor::SelectedBorder => palette.selected_border,
            ShellColor::Accent => palette.accent,
            ShellColor::AccentMuted => palette.accent_muted,
            ShellColor::Danger => palette.danger,
            ShellColor::DangerHover => palette.danger_hover,
        })
    }
}

fn theme_palette(theme: ThemePreset) -> ThemePalette {
    let terminal = theme.terminal_theme();
    let background = packed(terminal.background);
    let foreground = packed(terminal.foreground);
    let accent = packed(terminal.palette[4]);
    let danger = packed(terminal.palette[1]);
    ThemePalette {
        window: background,
        chrome: mix(background, foreground, 0.025),
        sidebar: mix(background, foreground, 0.04),
        border: mix(background, foreground, 0.12),
        text: foreground,
        muted_text: mix(foreground, background, 0.25),
        faint_text: mix(foreground, background, 0.5),
        hover: mix(background, foreground, 0.14),
        selected: mix(background, accent, 0.13),
        selected_border: mix(background, accent, 0.35),
        accent,
        accent_muted: mix(background, accent, 0.16),
        danger,
        danger_hover: mix(background, danger, 0.16),
    }
}

fn packed(color: [u8; 3]) -> u32 {
    (u32::from(color[0]) << 16) | (u32::from(color[1]) << 8) | u32::from(color[2])
}

fn mix(first: u32, second: u32, second_weight: f32) -> u32 {
    let channel = |shift: u32| {
        let first = ((first >> shift) & 0xff_u32) as f32;
        let second = ((second >> shift) & 0xff_u32) as f32;
        (first + (second - first) * second_weight).round() as u32
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

#[cfg(windows)]
fn platform_appearance() -> WindowBackgroundAppearance {
    windows_appearance_for_build(windows_version::OsVersion::current().build)
}

#[cfg(target_os = "macos")]
fn platform_appearance() -> WindowBackgroundAppearance {
    WindowBackgroundAppearance::Blurred
}

#[cfg(target_os = "linux")]
fn platform_appearance() -> WindowBackgroundAppearance {
    linux_appearance(std::env::var_os("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty()))
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn platform_appearance() -> WindowBackgroundAppearance {
    WindowBackgroundAppearance::Opaque
}

#[cfg(any(windows, test))]
fn windows_appearance_for_build(build: u32) -> WindowBackgroundAppearance {
    match build {
        17763.. => WindowBackgroundAppearance::Blurred,
        _ => WindowBackgroundAppearance::Opaque,
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_appearance(has_wayland_display: bool) -> WindowBackgroundAppearance {
    if has_wayland_display {
        WindowBackgroundAppearance::Blurred
    } else {
        WindowBackgroundAppearance::Opaque
    }
}

impl ShellIcon {
    fn lucide(self) -> lucide_icons::Icon {
        match self {
            Self::AppMark => lucide_icons::Icon::SquareTerminal,
            Self::Plus => lucide_icons::Icon::Plus,
            Self::SplitHorizontal => lucide_icons::Icon::Columns2,
            Self::SplitVertical => lucide_icons::Icon::Rows2,
            Self::Move => lucide_icons::Icon::Move,
            Self::Settings => lucide_icons::Icon::Settings2,
            Self::SidebarClose => lucide_icons::Icon::PanelLeftClose,
            Self::SidebarOpen => lucide_icons::Icon::PanelLeftOpen,
            Self::Close => lucide_icons::Icon::X,
            Self::ChevronRight => lucide_icons::Icon::ChevronRight,
            Self::ChevronDown => lucide_icons::Icon::ChevronDown,
        }
    }
}

pub(crate) fn install_icon_font(cx: &App) -> Result<(), String> {
    cx.text_system()
        .add_fonts(vec![Cow::Borrowed(lucide_icons::LUCIDE_FONT_BYTES)])
        .map_err(|error| format!("register Lucide icon font: {error}"))
}

#[cfg(test)]
mod tests {
    use gpui::{WindowBackgroundAppearance, rgb};

    use crate::settings::{ThemePreset, parse_opacity};

    use super::{
        ShellColor, ShellIcon, WorkspaceShell, linux_appearance, windows_appearance_for_build,
    };

    #[test]
    fn semantic_colors_and_icons_resolve_without_call_site_literals() {
        let shell = WorkspaceShell::resolve(
            WindowBackgroundAppearance::Opaque,
            ThemePreset::TokyoNight,
            1.,
        );
        let _ = shell.color(ShellColor::Selected);
        let _ = shell.icon(ShellIcon::AppMark, shell.color(ShellColor::Accent), 13.);
    }

    #[test]
    fn shell_colors_derive_from_the_official_catppuccin_terminal_values() {
        for (theme, base, ansi_blue) in [
            (ThemePreset::CatppuccinLatte, 0xeff1f5, 0x1e66f5),
            (ThemePreset::CatppuccinFrappe, 0x303446, 0x8caaee),
            (ThemePreset::CatppuccinMacchiato, 0x24273a, 0x8aadf4),
            (ThemePreset::CatppuccinMocha, 0x1e1e2e, 0x89b4fa),
        ] {
            let shell = WorkspaceShell::resolve(WindowBackgroundAppearance::Opaque, theme, 1.);
            assert_eq!(shell.color(ShellColor::Window), rgb(base));
            assert_eq!(shell.color(ShellColor::Accent), rgb(ansi_blue));
        }
    }

    #[test]
    fn opacity_preference_is_bounded_and_rejects_non_numbers() {
        assert_eq!(parse_opacity("0.82"), Some(0.82));
        assert_eq!(parse_opacity("0"), Some(0.45));
        assert_eq!(parse_opacity("4"), Some(1.));
        assert_eq!(parse_opacity("NaN"), None);
        assert_eq!(parse_opacity("not-a-number"), None);
        assert_eq!(
            WorkspaceShell::resolve(
                WindowBackgroundAppearance::Blurred,
                ThemePreset::TokyoNight,
                f32::NAN,
            )
            .opacity,
            0.65
        );
    }

    #[test]
    fn platform_fallbacks_never_request_unsupported_materials() {
        assert_eq!(
            windows_appearance_for_build(22621),
            WindowBackgroundAppearance::Blurred
        );
        assert_eq!(
            windows_appearance_for_build(17763),
            WindowBackgroundAppearance::Blurred
        );
        assert_eq!(
            windows_appearance_for_build(17134),
            WindowBackgroundAppearance::Opaque
        );
        assert_eq!(linux_appearance(true), WindowBackgroundAppearance::Blurred);
        assert_eq!(linux_appearance(false), WindowBackgroundAppearance::Opaque);
    }

    #[test]
    fn opaque_fallback_does_not_leave_translucent_content() {
        let shell = WorkspaceShell::resolve(
            WindowBackgroundAppearance::Opaque,
            ThemePreset::TokyoNight,
            0.7,
        );
        assert_eq!(shell.opacity, 1.);
        assert_eq!(shell.root_color().a, 1.);
    }

    #[test]
    fn only_surface_colors_receive_translucency() {
        let shell = WorkspaceShell::resolve(
            WindowBackgroundAppearance::Blurred,
            ThemePreset::TokyoNight,
            0.82,
        );
        assert_eq!(shell.terminal_background(rgb(0x101010)).a, 0.82);
        assert_eq!(shell.color(ShellColor::Text).a, 1.);
        assert_eq!(shell.root_color().a, 0.);
        assert_eq!(shell.appearance, WindowBackgroundAppearance::Blurred,);
        assert!(shell.color(ShellColor::Selected).a < shell.color(ShellColor::Hover).a);
        assert!(shell.color(ShellColor::Hover).a < shell.color(ShellColor::Chrome).a);
    }
}
