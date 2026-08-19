use crate::{ghostty, pty::PtySession};
use gpui::{
    App, Application, Bounds, Context, FocusHandle, IntoElement, KeyDownEvent, Render, Task,
    Window, WindowBounds, WindowOptions, div, prelude::*, px, rgb, size,
};
#[cfg(windows)]
use std::time::Duration;

const COLS: u16 = 80;
const ROWS: u16 = 24;
#[cfg(not(windows))]
const STARTUP_BANNER: &[u8] = b"\x1b[1;32mGPUI + libghostty-vt\x1b[0m\r\nUnix PTY live\r\n";
#[cfg(windows)]
const STARTUP_BANNER: &[u8] = b"\x1b[1;32mGPUI + libghostty-vt\x1b[0m\r\nWindows ConPTY probe starting...\r\n";

pub fn run() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(900.), px(560.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| {
                    let (session, output) = PtySession::spawn().expect("spawn cross-platform PTY");
                    let mut terminal =
                        ghostty::Terminal::new(COLS, ROWS).expect("create Ghostty VT");
                    terminal.feed(STARTUP_BANNER);
                    let focus = cx.focus_handle();
                    focus.focus(window);
                    let view = cx.new(|_| TerminalView {
                        terminal,
                        session,
                        focus,
                        output_task: Task::ready(()),
                    });
                    view.update(cx, |view, cx| {
                        view.start_output_task(output, cx);
                        #[cfg(windows)]
                        view.start_windows_probe(cx);
                    });
                    view
                },
            )
            .expect("open GPUI window");

        window
            .update(cx, |_view, _window, cx| cx.activate(true))
            .expect("activate GPUI window");
    });
}

struct TerminalView {
    terminal: ghostty::Terminal,
    session: PtySession,
    focus: FocusHandle,
    output_task: Task<()>,
}

impl TerminalView {
    #[cfg(windows)]
    fn start_windows_probe(&mut self, cx: &mut Context<Self>) {
        let timer = cx.background_executor().timer(Duration::from_millis(750));
        cx.spawn(async move |this, cx| {
            timer.await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |view, _cx| {
                    view.session.write(b"echo WINDOWS_CONPTY_LIVE\r");
                })
                .ok();
            }
        })
        .detach();
    }

    fn start_output_task(
        &mut self,
        output: flume::Receiver<Vec<u8>>,
        cx: &mut Context<Self>,
    ) {
        self.output_task = cx.spawn(async move |this, cx| {
            while let Ok(first) = output.recv_async().await {
                let mut chunks = vec![first];
                while let Ok(next) = output.try_recv() {
                    chunks.push(next);
                }

                let Some(this) = this.upgrade() else {
                    break;
                };
                if this
                    .update(cx, |view, cx| {
                        for chunk in chunks {
                            view.terminal.feed(&chunk);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        let bytes = if key.modifiers.control && key.key.len() == 1 {
            let byte = key.key.as_bytes()[0].to_ascii_uppercase();
            (b'@'..=b'_').contains(&byte).then(|| vec![byte - b'@'])
        } else if key.modifiers.platform || key.modifiers.alt {
            None
        } else {
            match key.key.as_str() {
                "enter" => Some(vec![b'\r']),
                "backspace" => Some(vec![0x7f]),
                "tab" => Some(vec![b'\t']),
                "escape" => Some(vec![0x1b]),
                "up" => Some(b"\x1b[A".to_vec()),
                "down" => Some(b"\x1b[B".to_vec()),
                "right" => Some(b"\x1b[C".to_vec()),
                "left" => Some(b"\x1b[D".to_vec()),
                _ => key.key_char.as_ref().map(|text| text.as_bytes().to_vec()),
            }
        };

        if let Some(bytes) = bytes {
            self.session.write(&bytes);
            cx.stop_propagation();
        }
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.terminal.snapshot().expect("snapshot Ghostty terminal");
        let cursor = snapshot.cursor;
        let default_bg = color(snapshot.default_bg);

        let rows = (0..snapshot.rows).map(|y| {
            let cells = (0..snapshot.cols).map(|x| {
                let cell = snapshot.cells.iter().find(|cell| cell.x == x && cell.y == y);
                let text = cell
                    .map(|cell| cell.text.as_str())
                    .filter(|text| !text.is_empty())
                    .unwrap_or(" ")
                    .to_owned();
                let foreground = cell.map(|cell| cell.fg).unwrap_or(snapshot.default_fg);
                let background = if cursor == Some((x, y)) {
                    foreground
                } else {
                    cell.map(|cell| cell.bg).unwrap_or(snapshot.default_bg)
                };
                let foreground = if cursor == Some((x, y)) {
                    snapshot.default_bg
                } else {
                    foreground
                };

                div()
                    .w(px(10.))
                    .h(px(20.))
                    .flex_none()
                    .overflow_hidden()
                    .bg(color(background))
                    .text_color(color(foreground))
                    .text_size(px(14.))
                    .font_family("monospace")
                    .child(text)
            });
            div().h(px(20.)).flex().flex_row().children(cells)
        });

        div()
            .id("terminal")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|view, event, _window, cx| {
                view.on_key_down(event, cx)
            }))
            .size_full()
            .overflow_hidden()
            .bg(default_bg)
            .p(px(12.))
            .children(rows)
    }
}

fn color(rgb_bytes: [u8; 3]) -> gpui::Rgba {
    rgb(
        (u32::from(rgb_bytes[0]) << 16)
            | (u32::from(rgb_bytes[1]) << 8)
            | u32::from(rgb_bytes[2]),
    )
}
