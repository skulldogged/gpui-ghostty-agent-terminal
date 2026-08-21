use crate::gui;
use gpui::{App, QuitMode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "the platform presence adapter constructs the quit intent in the next stacked slice"
)]
pub(crate) enum DesktopIntent {
    OpenOrFocus,
    QuitDesktopShell,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DesktopAction {
    OpenWindow,
    FocusWindow,
    Quit,
}

pub(crate) fn run() {
    let application = gpui_platform::application().with_quit_mode(QuitMode::Explicit);
    application.on_reopen(|cx| handle_intent(DesktopIntent::OpenOrFocus, cx));
    application.run(|cx| handle_intent(DesktopIntent::OpenOrFocus, cx));
}

pub(crate) fn handle_intent(intent: DesktopIntent, cx: &mut App) {
    match action_for(intent, !cx.windows().is_empty()) {
        DesktopAction::OpenWindow => {
            gui::open_terminal_window(cx).expect("open GPUI terminal window");
        }
        DesktopAction::FocusWindow => {
            let window = cx
                .window_stack()
                .and_then(|windows| windows.first().copied())
                .or_else(|| cx.windows().first().copied());
            if let Some(window) = window {
                let _ = window.update(cx, |_view, window, cx| {
                    window.activate_window();
                    cx.activate(true);
                });
            }
        }
        DesktopAction::Quit => cx.quit(),
    }
}

fn action_for(intent: DesktopIntent, has_window: bool) -> DesktopAction {
    match (intent, has_window) {
        (DesktopIntent::OpenOrFocus, true) => DesktopAction::FocusWindow,
        (DesktopIntent::OpenOrFocus, false) => DesktopAction::OpenWindow,
        (DesktopIntent::QuitDesktopShell, _) => DesktopAction::Quit,
    }
}

#[cfg(test)]
mod tests {
    use super::{DesktopAction, DesktopIntent, action_for};

    #[test]
    fn open_or_focus_reuses_an_existing_window() {
        assert_eq!(
            action_for(DesktopIntent::OpenOrFocus, true),
            DesktopAction::FocusWindow
        );
        assert_eq!(
            action_for(DesktopIntent::OpenOrFocus, false),
            DesktopAction::OpenWindow
        );
    }

    #[test]
    fn quitting_the_desktop_shell_never_depends_on_window_state() {
        assert_eq!(
            action_for(DesktopIntent::QuitDesktopShell, true),
            DesktopAction::Quit
        );
        assert_eq!(
            action_for(DesktopIntent::QuitDesktopShell, false),
            DesktopAction::Quit
        );
    }
}
