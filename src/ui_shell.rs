use gpui::{
    AssetSource, Bounds, Pixels, Rgba, SharedString, Svg, TitlebarOptions,
    WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowOptions, prelude::*, px,
    rgb, svg,
};
use std::borrow::Cow;

pub(crate) struct WorkspaceShell;

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
    Shrink,
    Grow,
    Close,
}

impl WorkspaceShell {
    pub(crate) const TITLE_BAR_HEIGHT: f32 = 40.;
    pub(crate) const SIDEBAR_WIDTH: f32 = 220.;
    pub(crate) const SIDEBAR_MIN_WIDTH: f32 = 180.;
    pub(crate) const CHROME_TILE_SIZE: f32 = 32.;
    pub(crate) const CHROME_ICON_SIZE: f32 = 13.;
    pub(crate) const TAB_HEIGHT: f32 = 30.;
    pub(crate) const WINDOW_CONTROLS_WIDTH: f32 = if cfg!(target_os = "macos") { 0. } else { 138. };

    pub(crate) fn window_options(bounds: Bounds<Pixels>) -> WindowOptions {
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Agent Terminal".into()),
                appears_transparent: true,
                traffic_light_position: Some(gpui::point(px(14.), px(12.))),
            }),
            window_background: WindowBackgroundAppearance::Opaque,
            window_decorations: Some(WindowDecorations::Client),
            window_min_size: Some(gpui::size(px(720.), px(440.))),
            ..Default::default()
        }
    }

    pub(crate) fn color(role: ShellColor) -> Rgba {
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

    pub(crate) fn icon(icon: ShellIcon, color: Rgba) -> Svg {
        svg()
            .path(icon.path())
            .size(px(Self::CHROME_ICON_SIZE))
            .text_color(color)
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
            Self::Shrink => "icons/shrink.svg",
            Self::Grow => "icons/grow.svg",
            Self::Close => "icons/close.svg",
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
            "icons/shrink.svg" => Some(include_bytes!("../assets/icons/shrink.svg")),
            "icons/grow.svg" => Some(include_bytes!("../assets/icons/grow.svg")),
            "icons/close.svg" => Some(include_bytes!("../assets/icons/close.svg")),
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
    use super::{ShellColor, ShellIcon, WorkspaceShell};

    #[test]
    fn semantic_colors_and_icons_resolve_without_call_site_literals() {
        let _ = WorkspaceShell::color(ShellColor::Selected);
        let _ = WorkspaceShell::icon(
            ShellIcon::AppMark,
            WorkspaceShell::color(ShellColor::Accent),
        );
    }
}
