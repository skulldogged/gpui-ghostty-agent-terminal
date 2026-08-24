use crate::application::ApplicationIntent;
use flume::Sender;

pub(crate) struct DesktopPresence {
    _platform: platform::DesktopPresence,
}

impl DesktopPresence {
    pub(crate) fn start(sender: Sender<ApplicationIntent>) -> Result<Self, String> {
        platform::DesktopPresence::start(sender).map(|platform| Self {
            _platform: platform,
        })
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use ksni::{
        Tray,
        blocking::{Handle, TrayMethods},
        menu::{MenuItem, StandardItem},
    };

    struct LinuxStatusNotifier {
        sender: Sender<ApplicationIntent>,
    }

    impl LinuxStatusNotifier {
        fn send(&self, intent: ApplicationIntent) {
            let _ = self.sender.send(intent);
        }
    }

    impl Tray for LinuxStatusNotifier {
        fn id(&self) -> String {
            "agent-terminal".into()
        }

        fn title(&self) -> String {
            "Agent Terminal".into()
        }

        fn icon_name(&self) -> String {
            "utilities-terminal".into()
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            self.send(ApplicationIntent::OpenOrFocus);
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            vec![
                StandardItem {
                    label: "Open Terminal".into(),
                    activate: Box::new(|tray: &mut Self| {
                        tray.send(ApplicationIntent::OpenOrFocus);
                    }),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                StandardItem {
                    label: "Quit".into(),
                    activate: Box::new(|tray: &mut Self| {
                        tray.send(ApplicationIntent::Quit);
                    }),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    pub(super) struct DesktopPresence {
        handle: Handle<LinuxStatusNotifier>,
    }

    impl DesktopPresence {
        pub(super) fn start(sender: Sender<ApplicationIntent>) -> Result<Self, String> {
            LinuxStatusNotifier { sender }
                .spawn()
                .map(|handle| Self { handle })
                .map_err(|error| format!("start Linux status notifier: {error}"))
        }
    }

    impl Drop for DesktopPresence {
        fn drop(&mut self) {
            self.handle.shutdown().wait();
        }
    }
}

#[cfg(any(target_os = "macos", windows))]
mod platform {
    use super::*;
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
        menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    };

    const TRAY_ID: &str = "agent-terminal";
    const OPEN_ID: &str = "agent-terminal.open";
    const QUIT_ID: &str = "agent-terminal.quit";

    pub(super) struct DesktopPresence {
        _tray_icon: TrayIcon,
    }

    impl DesktopPresence {
        pub(super) fn start(sender: Sender<ApplicationIntent>) -> Result<Self, String> {
            let open = MenuItem::with_id(OPEN_ID, "Open Terminal", true, None);
            let separator = PredefinedMenuItem::separator();
            let quit = MenuItem::with_id(QUIT_ID, "Quit", true, None);
            let menu = Menu::with_items(&[&open, &separator, &quit])
                .map_err(|error| format!("create desktop tray menu: {error}"))?;

            let menu_sender = sender.clone();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| match event.id.as_ref() {
                OPEN_ID => {
                    let _ = menu_sender.send(ApplicationIntent::OpenOrFocus);
                }
                QUIT_ID => {
                    let _ = menu_sender.send(ApplicationIntent::Quit);
                }
                _ => {}
            }));

            let click_sender = sender;
            TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
                if let TrayIconEvent::Click {
                    id,
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                    && id == TRAY_ID
                {
                    let _ = click_sender.send(ApplicationIntent::OpenOrFocus);
                }
            }));

            let icon = Icon::from_rgba(terminal_icon_rgba(), 32, 32)
                .map_err(|error| format!("create desktop tray icon: {error}"))?;
            let tray_icon = TrayIconBuilder::new()
                .with_id(TRAY_ID)
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(false)
                .with_tooltip("Agent Terminal")
                .with_icon(icon)
                .build()
                .map_err(|error| format!("create desktop tray item: {error}"))?;

            Ok(Self {
                _tray_icon: tray_icon,
            })
        }
    }

    fn terminal_icon_rgba() -> Vec<u8> {
        const SIDE: usize = 32;
        let mut rgba = vec![0; SIDE * SIDE * 4];

        for y in 3..29 {
            for x in 2..30 {
                let pixel = (y * SIDE + x) * 4;
                rgba[pixel..pixel + 4].copy_from_slice(&[30, 41, 59, 255]);
            }
        }

        for &(x, y) in &[
            (8, 10),
            (9, 11),
            (10, 12),
            (11, 13),
            (10, 14),
            (9, 15),
            (8, 16),
        ] {
            paint_block(&mut rgba, SIDE, x, y, [125, 211, 252, 255]);
        }
        for x in 15..24 {
            paint_block(&mut rgba, SIDE, x, 18, [226, 232, 240, 255]);
        }

        rgba
    }

    fn paint_block(rgba: &mut [u8], side: usize, x: usize, y: usize, color: [u8; 4]) {
        for offset_y in 0..2 {
            for offset_x in 0..2 {
                let pixel = ((y + offset_y) * side + x + offset_x) * 4;
                rgba[pixel..pixel + 4].copy_from_slice(&color);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::terminal_icon_rgba;

        #[test]
        fn terminal_icon_has_the_expected_rgba_shape_and_transparency() {
            let rgba = terminal_icon_rgba();
            assert_eq!(rgba.len(), 32 * 32 * 4);
            assert_eq!(&rgba[0..4], &[0, 0, 0, 0]);
            assert!(rgba.as_chunks::<4>().0.iter().any(|pixel| pixel[3] == 255));
        }
    }
}
