mod core_model;
mod ghostty;

#[cfg(feature = "gui")]
mod pty;
#[cfg(feature = "gui")]
mod resident_core;
#[cfg(feature = "gui")]
mod terminal_frame;
#[cfg(feature = "gui")]
mod terminal_grid;
#[cfg(feature = "gui")]
mod terminal_session;
#[cfg(feature = "gui")]
mod ui_shell;
#[cfg(all(feature = "gui", windows))]
mod windows_pty;

#[cfg(feature = "gui")]
mod core_driver;
#[cfg(feature = "gui")]
mod desktop_presence;
#[cfg(feature = "gui")]
mod desktop_shell;
#[cfg(feature = "gui")]
mod gui;

pub use core_model::{
    CoreCommand, CoreCommit, CoreEffect, CoreModel, CoreModelError, CoreSnapshot, CreatedResource,
    PaneId, PaneLayout, PaneSnapshot, ResourceKind, SpaceId, SpaceSnapshot, SplitAxis, SplitId,
    SplitPlacement, SplitRatio, SplitSnapshot, TabId, TabSnapshot, TerminalLaunch,
    TerminalSessionId, TerminalSessionSnapshot,
};
#[cfg(feature = "gui")]
pub use resident_core::{
    ControlLease, ControlLeaseDenial, CoreClient, CoreCommandError, CoreCommandOutcome,
    CoreEndpoint, SemanticEvent, SemanticEventKind, TerminalCell, TerminalChange,
    TerminalLifecycle, TerminalSnapshot, UiClientId, run_resident_core,
    stop_resident_core_after_parent,
};
#[cfg(feature = "gui")]
pub use terminal_session::TerminalSize;

#[cfg(feature = "gui")]
pub fn run_gui() {
    let endpoint = CoreEndpoint::for_current_user().expect("resolve default Resident Core profile");
    desktop_shell::run(endpoint, false).expect("run Desktop Shell");
}

#[cfg(feature = "gui")]
pub fn run_development_gui() -> Result<(), String> {
    // This is the process entry point for `--development`, before GPUI or any
    // worker threads start. Remove only the host's development-runner color
    // preference so the app and the child Core inherit a normal terminal
    // environment; regular launches continue to honor an explicit NO_COLOR.
    unsafe { std::env::remove_var("NO_COLOR") };
    let endpoint = CoreEndpoint::for_development_launch()?;
    desktop_shell::run(endpoint, true)
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
