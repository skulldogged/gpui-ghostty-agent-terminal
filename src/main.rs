fn main() {
    #[cfg(feature = "gui")]
    {
        let mut arguments = std::env::args().skip(1);
        let command = arguments.next();
        if command.as_deref() == Some("--development") {
            if let Err(error) = agent_terminal::run_development_gui() {
                eprintln!("Development launch failed: {error}");
                std::process::exit(1);
            }
            return;
        }

        if let Err(error) = agent_terminal::run_gui() {
            eprintln!("Launch failed: {error}");
            std::process::exit(1);
        }
    }

    #[cfg(not(feature = "gui"))]
    agent_terminal::headless_smoke();
}
