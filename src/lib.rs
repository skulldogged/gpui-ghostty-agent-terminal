mod core_model;
mod ghostty;
mod terminal_link;

#[cfg(feature = "gui")]
mod agent_integration;
#[cfg(feature = "gui")]
mod application_core;
#[cfg(feature = "gui")]
mod pty;
#[cfg(feature = "gui")]
mod settings;
#[cfg(feature = "gui")]
mod terminal_frame;
#[cfg(feature = "gui")]
mod terminal_grid;
#[cfg(feature = "gui")]
mod terminal_selection;
#[cfg(feature = "gui")]
mod terminal_session;
#[cfg(feature = "gui")]
mod terminal_theme;
#[cfg(feature = "gui")]
mod ui_shell;
#[cfg(all(feature = "gui", windows))]
mod windows_pty;

#[cfg(feature = "gui")]
mod activation;
#[cfg(feature = "gui")]
mod application;
#[cfg(feature = "gui")]
mod core_driver;
#[cfg(feature = "gui")]
mod desktop_presence;
#[cfg(feature = "gui")]
mod gui;

#[cfg(feature = "gui")]
pub use agent_integration::{AgentProgram, AgentSnapshot, AgentState};
#[cfg(feature = "gui")]
pub use application_core::{
    ApplicationCore, CoreCommandOutcome, SemanticEvent, SemanticEventKind, TerminalCell,
    TerminalChange, TerminalCursorShape, TerminalLifecycle, TerminalSnapshot,
};
pub use core_model::{
    CoreCommand, CoreCommit, CoreEffect, CoreModel, CoreModelError, CoreSnapshot, CreatedResource,
    PaneId, PaneLayout, PaneSnapshot, ResourceKind, SpaceId, SpaceSnapshot, SplitAxis, SplitId,
    SplitPlacement, SplitRatio, SplitSnapshot, TabId, TabSnapshot, TerminalLaunch,
    TerminalSessionId, TerminalSessionSnapshot,
};
#[cfg(feature = "gui")]
pub use terminal_session::TerminalSize;

#[cfg(feature = "gui")]
pub fn run_gui() {
    application::run(false).expect("run application");
}

#[cfg(feature = "gui")]
pub fn run_development_gui() -> Result<(), String> {
    application::run(true)
}

#[cfg(not(feature = "gui"))]
pub fn headless_smoke() {
    let mut terminal = ghostty::Terminal::new(48, 8).expect("create terminal");
    terminal.feed(
        b"\x1b[2J\x1b[H\x1b[1;31mRED\x1b[0m green?\r\nUnicode: \xe7\x8c\xab \xf0\x9f\x90\x88 e\xcc\x81\r\n\x1b[44mbackground\x1b[0m\r\n",
    ).expect("process headless smoke output");
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
