use crate::{CoreClient, TerminalSize, TerminalSnapshot};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const COMMAND_CAPACITY: usize = 256;

pub(crate) struct CoreDriver {
    commands: flume::Sender<Command>,
    updates: flume::Receiver<DriverUpdate>,
    poll_pending: Arc<AtomicBool>,
}

pub(crate) enum DriverUpdate {
    Snapshot(TerminalSnapshot),
    Error(String),
}

enum Command {
    Input(Vec<u8>),
    Resize(TerminalSize),
    Poll,
}

impl CoreDriver {
    pub(crate) fn start(core: CoreClient, revision: u64) -> Result<Self, String> {
        let (commands_tx, commands_rx) = flume::bounded(COMMAND_CAPACITY);
        let (updates_tx, updates_rx) = flume::bounded(1);
        let stale_updates = updates_rx.clone();
        let poll_pending = Arc::new(AtomicBool::new(false));
        let worker_poll_pending = poll_pending.clone();
        std::thread::Builder::new()
            .name("ui-core-driver".into())
            .spawn(move || {
                run_driver(
                    core,
                    revision,
                    commands_rx,
                    updates_tx,
                    stale_updates,
                    worker_poll_pending,
                )
            })
            .map_err(|error| format!("spawn UI Core driver: {error}"))?;
        Ok(Self {
            commands: commands_tx,
            updates: updates_rx,
            poll_pending,
        })
    }

    pub(crate) fn input(&self, bytes: Vec<u8>) -> Result<(), String> {
        self.send(Command::Input(bytes))
    }

    pub(crate) fn resize(&self, size: TerminalSize) -> Result<(), String> {
        self.send(Command::Resize(size))
    }

    pub(crate) fn request_snapshot(&self) {
        if self
            .poll_pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            && self.commands.try_send(Command::Poll).is_err()
        {
            self.poll_pending.store(false, Ordering::Release);
        }
    }

    pub(crate) fn latest_update(&self) -> Option<DriverUpdate> {
        let mut latest = None;
        while let Ok(update) = self.updates.try_recv() {
            latest = Some(update);
        }
        latest
    }

    fn send(&self, command: Command) -> Result<(), String> {
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                flume::TrySendError::Full(_) => "UI Core driver command queue is busy".into(),
                flume::TrySendError::Disconnected(_) => "UI Core driver stopped".into(),
            })
    }
}

fn run_driver(
    mut core: CoreClient,
    mut revision: u64,
    commands: flume::Receiver<Command>,
    updates: flume::Sender<DriverUpdate>,
    stale_updates: flume::Receiver<DriverUpdate>,
    poll_pending: Arc<AtomicBool>,
) {
    while let Ok(command) = commands.recv() {
        let is_poll = matches!(command, Command::Poll);
        let result = match command {
            Command::Input(bytes) => core.input(&bytes).map(|()| None),
            Command::Resize(size) => core.resize(size).map(|()| None),
            Command::Poll => core.snapshot_since(revision).map(|snapshot| {
                if let Some(snapshot) = &snapshot {
                    revision = snapshot.revision;
                }
                snapshot.map(DriverUpdate::Snapshot)
            }),
        };
        if is_poll {
            poll_pending.store(false, Ordering::Release);
        }
        match result {
            Ok(Some(update)) => publish_latest(&updates, &stale_updates, update),
            Ok(None) => {}
            Err(error) => {
                let reconnect = is_connection_failure(&error);
                publish_latest(&updates, &stale_updates, DriverUpdate::Error(error));
                if !reconnect {
                    continue;
                }
                let recovered = CoreClient::connect_or_spawn().and_then(|mut replacement| {
                    let snapshot = replacement.snapshot()?;
                    Ok((replacement, snapshot))
                });
                match recovered {
                    Ok((replacement, snapshot)) => {
                        core = replacement;
                        revision = snapshot.revision;
                        publish_latest(&updates, &stale_updates, DriverUpdate::Snapshot(snapshot));
                    }
                    Err(error) => publish_latest(
                        &updates,
                        &stale_updates,
                        DriverUpdate::Error(format!("Resident Core reconnect failed: {error}")),
                    ),
                }
            }
        }
    }
}

fn is_connection_failure(error: &str) -> bool {
    error.starts_with("send Resident Core command:")
        || error.starts_with("receive Resident Core response:")
        || error.starts_with("Resident Core disconnected")
}

fn publish_latest<T>(updates: &flume::Sender<T>, stale_updates: &flume::Receiver<T>, update: T) {
    match updates.try_send(update) {
        Ok(()) => {}
        Err(flume::TrySendError::Full(update)) => {
            let _ = stale_updates.try_recv();
            let _ = updates.try_send(update);
        }
        Err(flume::TrySendError::Disconnected(_)) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{is_connection_failure, publish_latest};

    #[test]
    fn slow_views_receive_only_the_latest_coalescible_update() {
        let (sender, receiver) = flume::bounded(1);
        publish_latest(&sender, &receiver, 1);
        publish_latest(&sender, &receiver, 2);
        publish_latest(&sender, &receiver, 3);

        assert_eq!(receiver.recv().unwrap(), 3);
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn semantic_command_errors_do_not_open_a_second_control_connection() {
        assert!(!is_connection_failure("terminal grid exceeds capacity"));
        assert!(is_connection_failure(
            "receive Resident Core response: connection reset"
        ));
    }
}
