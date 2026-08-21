use crate::{desktop_presence::DesktopPresence, gui};
use gpui::{App, Global, QuitMode, Task};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

struct DesktopShellRuntime {
    _presence: Option<DesktopPresence>,
    _intent_task: Task<()>,
}

impl Global for DesktopShellRuntime {}

pub(crate) fn run() {
    let application = gpui_platform::application().with_quit_mode(QuitMode::Explicit);
    application.on_reopen(|cx| handle_intent(DesktopIntent::OpenOrFocus, cx));
    application.run(|cx| {
        install_desktop_presence(cx);
        handle_intent(DesktopIntent::OpenOrFocus, cx);
    });
}

fn install_desktop_presence(cx: &mut App) {
    let (intent_sender, intent_receiver) = flume::unbounded();
    let presence = match DesktopPresence::start(intent_sender) {
        Ok(presence) => {
            cx.set_quit_mode(QuitMode::Explicit);
            Some(presence)
        }
        Err(error) => {
            eprintln!("Desktop presence unavailable: {error}");
            cx.set_quit_mode(QuitMode::LastWindowClosed);
            None
        }
    };
    let intent_task = cx.spawn(async move |cx| {
        while let Ok(intent) = intent_receiver.recv_async().await {
            cx.update(|cx| handle_intent(intent, cx));
        }
    });
    cx.set_global(DesktopShellRuntime {
        _presence: presence,
        _intent_task: intent_task,
    });
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
