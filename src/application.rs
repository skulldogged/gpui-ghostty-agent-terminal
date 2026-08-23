use crate::{ApplicationCore, desktop_presence::DesktopPresence, gui, ui_shell::ShellAssets};
use gpui::{App, Global, QuitMode, Task};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ApplicationIntent {
    OpenOrFocus,
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ApplicationAction {
    OpenWindow,
    FocusWindow,
    Quit,
}

struct ApplicationRuntime {
    core: ApplicationCore,
    _presence: Option<DesktopPresence>,
    _intent_task: Task<()>,
}

impl Global for ApplicationRuntime {}

pub(crate) fn run(development: bool) -> Result<(), String> {
    let core = ApplicationCore::start()?;
    let launch_core = core.clone();
    let application = gpui_platform::application()
        .with_assets(ShellAssets)
        .with_quit_mode(QuitMode::Explicit);
    application.on_reopen(|cx| handle_intent(ApplicationIntent::OpenOrFocus, cx));
    application.run(move |cx| {
        if let Err(error) = install_desktop_presence(cx, launch_core, development) {
            eprintln!("Could not start application: {error}");
            cx.quit();
            return;
        }
        handle_intent(ApplicationIntent::OpenOrFocus, cx);
    });
    drop(core);
    Ok(())
}

fn install_desktop_presence(
    cx: &mut App,
    core: ApplicationCore,
    development: bool,
) -> Result<(), String> {
    let (intent_sender, intent_receiver) = flume::unbounded();
    if development {
        let interrupt_sender = intent_sender.clone();
        ctrlc::set_handler(move || {
            let _ = interrupt_sender.send(ApplicationIntent::Quit);
        })
        .map_err(|error| format!("install development interrupt handler: {error}"))?;
    }
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
    cx.set_global(ApplicationRuntime {
        core,
        _presence: presence,
        _intent_task: intent_task,
    });
    Ok(())
}

pub(crate) fn handle_intent(intent: ApplicationIntent, cx: &mut App) {
    let core = cx.global::<ApplicationRuntime>().core.clone();
    match action_for(intent, !cx.windows().is_empty()) {
        ApplicationAction::OpenWindow => {
            gui::open_terminal_window(cx, core).expect("open GPUI terminal window");
        }
        ApplicationAction::FocusWindow => {
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
        ApplicationAction::Quit => cx.quit(),
    }
}

fn action_for(intent: ApplicationIntent, has_window: bool) -> ApplicationAction {
    match (intent, has_window) {
        (ApplicationIntent::OpenOrFocus, true) => ApplicationAction::FocusWindow,
        (ApplicationIntent::OpenOrFocus, false) => ApplicationAction::OpenWindow,
        (ApplicationIntent::Quit, _) => ApplicationAction::Quit,
    }
}

#[cfg(test)]
mod tests {
    use super::{ApplicationAction, ApplicationIntent, action_for};

    #[test]
    fn open_or_focus_reuses_an_existing_window() {
        assert_eq!(
            action_for(ApplicationIntent::OpenOrFocus, true),
            ApplicationAction::FocusWindow
        );
        assert_eq!(
            action_for(ApplicationIntent::OpenOrFocus, false),
            ApplicationAction::OpenWindow
        );
    }

    #[test]
    fn quit_is_independent_of_window_state() {
        assert_eq!(
            action_for(ApplicationIntent::Quit, true),
            ApplicationAction::Quit
        );
        assert_eq!(
            action_for(ApplicationIntent::Quit, false),
            ApplicationAction::Quit
        );
    }
}
