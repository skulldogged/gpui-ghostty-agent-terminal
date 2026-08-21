fn main() {
    #[cfg(feature = "gui")]
    {
        let mut arguments = std::env::args().skip(1);
        let command = arguments.next();
        if command.as_deref() == Some("--resident-core") {
            let endpoint = arguments
                .next()
                .ok_or_else(|| "--resident-core requires an endpoint argument".to_string())
                .and_then(agent_terminal::CoreEndpoint::from_argument)
                .unwrap_or_else(|error| {
                    eprintln!("{error}");
                    std::process::exit(2);
                });
            if let Err(error) = agent_terminal::run_resident_core(endpoint) {
                eprintln!("Resident Core failed: {error}");
                std::process::exit(1);
            }
            return;
        }
        if command.as_deref() == Some("--stop-resident-core-after-parent") {
            let endpoint = arguments
                .next()
                .ok_or_else(|| {
                    "--stop-resident-core-after-parent requires an endpoint argument".to_string()
                })
                .and_then(agent_terminal::CoreEndpoint::from_argument)
                .unwrap_or_else(|error| {
                    eprintln!("{error}");
                    std::process::exit(2);
                });
            if let Err(error) = agent_terminal::stop_resident_core_after_parent(endpoint) {
                eprintln!("Could not stop Resident Core after Desktop Shell exit: {error}");
                std::process::exit(1);
            }
            return;
        }

        agent_terminal::run_gui();
    }

    #[cfg(not(feature = "gui"))]
    agent_terminal::headless_smoke();
}
