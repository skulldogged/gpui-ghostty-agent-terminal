use gpui::{
    AssetSource, Bounds, Pixels, Rgba, SharedString, Svg, TitlebarOptions,
    WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowOptions, prelude::*, px,
    rgb, svg,
};
use std::borrow::Cow;

#[derive(Clone, Copy, Debug)]
pub(crate) struct WorkspaceShell {
    appearance: WindowBackgroundAppearance,
    opacity: f32,
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

    pub(crate) fn from_environment() -> Self {
        let requested_opacity = std::env::var("AGENT_TERMINAL_BACKGROUND_OPACITY").ok();
        Self::resolve(platform_appearance(), requested_opacity.as_deref())
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
        let color = Self::base_color(role);
        if !self.is_translucent() {
            return color;
        }

        color.alpha(match role {
            ShellColor::Window | ShellColor::Chrome | ShellColor::Sidebar => self.opacity,
            ShellColor::Hover => 0.48,
            ShellColor::Selected | ShellColor::AccentMuted | ShellColor::DangerHover => 0.72,
            ShellColor::Border | ShellColor::SelectedBorder => 0.8,
            ShellColor::Text
            | ShellColor::MutedText
            | ShellColor::FaintText
            | ShellColor::Accent
            | ShellColor::Danger => 1.,
        })
    }

    pub(crate) fn icon(&self, icon: ShellIcon, color: Rgba) -> Svg {
        svg()
            .path(icon.path())
            .size(px(Self::CHROME_ICON_SIZE))
            .text_color(color)
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
            Self::base_color(ShellColor::Window).alpha(0.)
        } else {
            self.color(ShellColor::Window)
        }
    }

    fn resolve(appearance: WindowBackgroundAppearance, requested_opacity: Option<&str>) -> Self {
        let mut opacity = requested_opacity
            .and_then(Self::parse_opacity)
            .unwrap_or(Self::DEFAULT_OPACITY);
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
        }
    }

    fn is_translucent(&self) -> bool {
        self.appearance != WindowBackgroundAppearance::Opaque && self.opacity < 1.
    }

    fn parse_opacity(value: &str) -> Option<f32> {
        let opacity = value.trim().parse::<f32>().ok()?;
        opacity.is_finite().then(|| opacity.clamp(0.45, 1.))
    }

    fn base_color(role: ShellColor) -> Rgba {
        match role {
            ShellColor::Window => rgb(0x0f0f17),
            ShellColor::Chrome => rgb(0x15151f),
            ShellColor::Sidebar => rgb(0x171721),
            ShellColor::Border => rgb(0x2a2937),
            ShellColor::Text => rgb(0xe8e7f0),
            ShellColor::MutedText => rgb(0xa09eaf),
            ShellColor::FaintText => rgb(0x6f6d7d),
            ShellColor::Hover => rgb(0x23222f),
            ShellColor::Selected => rgb(0x302e3d),
            ShellColor::SelectedBorder => rgb(0x444152),
            ShellColor::Accent => rgb(0x58d68d),
            ShellColor::AccentMuted => rgb(0x264c39),
            ShellColor::Danger => rgb(0xe88c8c),
            ShellColor::DangerHover => rgb(0x4a292f),
        }
    }
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
    fn path(self) -> &'static str {
        match self {
            Self::AppMark => "icons/app-mark.svg",
            Self::Plus => "icons/plus.svg",
            Self::SplitHorizontal => "icons/split-horizontal.svg",
            Self::SplitVertical => "icons/split-vertical.svg",
            Self::Move => "icons/move.svg",
        }
    }
}

pub(crate) struct ShellAssets;

impl AssetSource for ShellAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let bytes: Option<&'static [u8]> = match path {
            "icons/app-mark.svg" => Some(include_bytes!("../assets/icons/app-mark.svg")),
            "icons/plus.svg" => Some(include_bytes!("../assets/icons/plus.svg")),
            "icons/split-horizontal.svg" => {
                Some(include_bytes!("../assets/icons/split-horizontal.svg"))
            }
            "icons/split-vertical.svg" => {
                Some(include_bytes!("../assets/icons/split-vertical.svg"))
            }
            "icons/move.svg" => Some(include_bytes!("../assets/icons/move.svg")),
            _ => None,
        };
        Ok(bytes.map(Cow::Borrowed))
    }

    fn list(&self, _path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use gpui::{WindowBackgroundAppearance, rgb};

    use super::{
        ShellColor, ShellIcon, WorkspaceShell, linux_appearance, windows_appearance_for_build,
    };

    #[test]
    fn semantic_colors_and_icons_resolve_without_call_site_literals() {
        let shell = WorkspaceShell::resolve(WindowBackgroundAppearance::Opaque, Some("1"));
        let _ = shell.color(ShellColor::Selected);
        let _ = shell.icon(ShellIcon::AppMark, shell.color(ShellColor::Accent));
    }

    #[test]
    fn opacity_preference_is_bounded_and_rejects_non_numbers() {
        assert_eq!(WorkspaceShell::parse_opacity("0.82"), Some(0.82));
        assert_eq!(WorkspaceShell::parse_opacity("0"), Some(0.45));
        assert_eq!(WorkspaceShell::parse_opacity("4"), Some(1.));
        assert_eq!(WorkspaceShell::parse_opacity("NaN"), None);
        assert_eq!(WorkspaceShell::parse_opacity("not-a-number"), None);
        assert_eq!(
            WorkspaceShell::resolve(WindowBackgroundAppearance::Blurred, None).opacity,
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
        let shell = WorkspaceShell::resolve(WindowBackgroundAppearance::Opaque, Some("0.7"));
        assert_eq!(shell.opacity, 1.);
        assert_eq!(shell.root_color().a, 1.);
    }

    #[test]
    fn only_surface_colors_receive_translucency() {
        let shell = WorkspaceShell::resolve(WindowBackgroundAppearance::Blurred, Some("0.82"));
        assert_eq!(shell.terminal_background(rgb(0x101010)).a, 0.82);
        assert_eq!(shell.color(ShellColor::Text).a, 1.);
        assert_eq!(shell.root_color().a, 0.);
        assert_eq!(shell.appearance, WindowBackgroundAppearance::Blurred,);
    }
}
