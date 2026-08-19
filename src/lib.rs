mod ghostty;

#[cfg(feature = "gui")]
mod pty;
#[cfg(feature = "gui")]
mod resident_core;
#[cfg(feature = "gui")]
mod terminal_grid;
#[cfg(feature = "gui")]
mod terminal_session;
#[cfg(all(feature = "gui", windows))]
mod windows_pty;

#[cfg(feature = "gui")]
mod gui;

#[cfg(feature = "gui")]
pub use resident_core::{
    CoreClient, CoreEndpoint, TerminalCell, TerminalLifecycle, TerminalSnapshot, run_resident_core,
};
#[cfg(feature = "gui")]
pub use terminal_session::TerminalSize;

#[cfg(feature = "gui")]
pub fn run_gui() {
    gui::run();
}

#[cfg(not(feature = "gui"))]
pub fn headless_smoke() {
    let mut terminal = ghostty::Terminal::new(48, 8).expect("create terminal");
    terminal.feed(
        b"\x1b[2J\x1b[H\x1b[1;31mRED\x1b[0m green?\r\nUnicode: \xe7\x8c\xab \xf0\x9f\x90\x88 e\xcc\x81\r\n\x1b[44mbackground\x1b[0m\r\n",
    );
    terminal.resize(52, 9, 10, 20).expect("resize terminal");
    let snapshot = terminal.snapshot().expect("snapshot terminal");
    let text = ghostty::snapshot_text(&snapshot);
    assert!(text.contains("RED"));
    assert!(text.contains("Unicode"));
    assert!(snapshot.cells.iter().any(|cell| cell.has_explicit_bg));

    println!("Ghostty revision: {}", ghostty::SOURCE_REVISION);
    println!(
        "Snapshot: {}x{}, cells={}, cursor={:?}, fg={:?}, bg={:?}",
        snapshot.cols,
        snapshot.rows,
        snapshot.cells.len(),
        snapshot.cursor,
        snapshot.default_fg,
        snapshot.default_bg,
    );
    print!("{text}");
}
