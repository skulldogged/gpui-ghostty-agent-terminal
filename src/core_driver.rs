use crate::{CoreClient, TerminalChange, TerminalSize, TerminalSnapshot};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

const COMMAND_CAPACITY: usize = 256;

pub(crate) struct CoreDriver {
    commands: flume::Sender<Command>,
    updates: DriverUpdates,
    counters: Arc<DriverCounters>,
}

pub(crate) enum DriverUpdate {
    Snapshot(TerminalSnapshot),
    Error(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct DriverStats {
    pushed_changes: u64,
    snapshot_requests: u64,
    snapshots_published: u64,
}

#[derive(Default)]
struct DriverCounters {
    pushed_changes: AtomicU64,
    snapshot_requests: AtomicU64,
    snapshots_published: AtomicU64,
}

#[derive(Clone)]
pub(crate) struct DriverUpdates {
    receiver: flume::Receiver<DriverUpdate>,
}

impl DriverUpdates {
    pub(crate) async fn next(&self) -> Option<DriverUpdate> {
        self.receiver.recv_async().await.ok()
    }

    #[cfg(test)]
    fn recv_timeout(&self, timeout: std::time::Duration) -> Option<DriverUpdate> {
        self.receiver.recv_timeout(timeout).ok()
    }
}

enum Command {
    Input(Vec<u8>),
    Resize(TerminalSize),
}

enum DriverEvent {
    Command(Result<Command, flume::RecvError>),
    TerminalChanged(Result<TerminalChange, flume::RecvError>),
}

impl CoreDriver {
    pub(crate) fn start(core: CoreClient, revision: u64) -> Result<Self, String> {
        let (commands_tx, commands_rx) = flume::bounded(COMMAND_CAPACITY);
        let (updates_tx, updates_rx) = flume::bounded(1);
        let stale_updates = updates_rx.clone();
        let counters = Arc::new(DriverCounters::default());
        let worker_counters = Arc::clone(&counters);
        std::thread::Builder::new()
            .name("ui-core-driver".into())
            .spawn(move || {
                run_driver(
                    core,
                    revision,
                    commands_rx,
                    updates_tx,
                    stale_updates,
                    worker_counters,
                )
            })
            .map_err(|error| format!("spawn UI Core driver: {error}"))?;
        Ok(Self {
            commands: commands_tx,
            updates: DriverUpdates {
                receiver: updates_rx,
            },
            counters,
        })
    }

    pub(crate) fn input(&self, bytes: Vec<u8>) -> Result<(), String> {
        self.send(Command::Input(bytes))
    }

    pub(crate) fn resize(&self, size: TerminalSize) -> Result<(), String> {
        self.send(Command::Resize(size))
    }

    pub(crate) fn updates(&self) -> DriverUpdates {
        self.updates.clone()
    }

    fn stats(&self) -> DriverStats {
        DriverStats {
            pushed_changes: self.counters.pushed_changes.load(Ordering::Relaxed),
            snapshot_requests: self.counters.snapshot_requests.load(Ordering::Relaxed),
            snapshots_published: self.counters.snapshots_published.load(Ordering::Relaxed),
        }
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

impl Drop for CoreDriver {
    fn drop(&mut self) {
        if std::env::var_os("AGENT_TERMINAL_DRIVER_STATS").is_some() {
            let stats = self.stats();
            eprintln!(
                "UI Core driver: {} pushed changes, {} snapshot requests, {} snapshots published",
                stats.pushed_changes, stats.snapshot_requests, stats.snapshots_published
            );
        }
    }
}

fn run_driver(
    mut core: CoreClient,
    mut revision: u64,
    commands: flume::Receiver<Command>,
    updates: flume::Sender<DriverUpdate>,
    stale_updates: flume::Receiver<DriverUpdate>,
    counters: Arc<DriverCounters>,
) {
    let mut terminal_changes = core.terminal_changes();
    loop {
        let event = flume::Selector::new()
            .recv(&commands, DriverEvent::Command)
            .recv(&terminal_changes, DriverEvent::TerminalChanged)
            .wait();
        let (result, reconnect) = match event {
            DriverEvent::Command(Err(_)) => break,
            DriverEvent::Command(Ok(Command::Input(bytes))) => {
                let result = core
                    .input(&bytes)
                    .map(|()| None)
                    .map_err(|error| error.to_string());
                let reconnect = result
                    .as_ref()
                    .is_err_and(|error| is_connection_failure(error));
                (result, reconnect)
            }
            DriverEvent::Command(Ok(Command::Resize(size))) => {
                let result = core
                    .resize(size)
                    .map(|()| None)
                    .map_err(|error| error.to_string());
                let reconnect = result
                    .as_ref()
                    .is_err_and(|error| is_connection_failure(error));
                (result, reconnect)
            }
            DriverEvent::TerminalChanged(Ok(change)) => {
                counters.pushed_changes.fetch_add(1, Ordering::Relaxed);
                if change.terminal_revision <= revision {
                    continue;
                }
                counters.snapshot_requests.fetch_add(1, Ordering::Relaxed);
                let result = core.snapshot_since(revision).map(|snapshot| {
                    if let Some(snapshot) = &snapshot {
                        revision = snapshot.revision;
                    }
                    snapshot.map(DriverUpdate::Snapshot)
                });
                let reconnect = result
                    .as_ref()
                    .is_err_and(|error| is_connection_failure(error));
                (result, reconnect)
            }
            DriverEvent::TerminalChanged(Err(_)) => (
                Err("Resident Core disconnected while waiting for a terminal change".into()),
                true,
            ),
        };
        match result {
            Ok(Some(update)) => {
                counters.snapshots_published.fetch_add(1, Ordering::Relaxed);
                publish_latest(&updates, &stale_updates, update);
            }
            Ok(None) => {}
            Err(error) => {
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
                        terminal_changes = replacement.terminal_changes();
                        core = replacement;
                        revision = snapshot.revision;
                        counters.snapshots_published.fetch_add(1, Ordering::Relaxed);
                        publish_latest(&updates, &stale_updates, DriverUpdate::Snapshot(snapshot));
                    }
                    Err(error) => {
                        publish_latest(
                            &updates,
                            &stale_updates,
                            DriverUpdate::Error(format!("Resident Core reconnect failed: {error}")),
                        );
                        break;
                    }
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
    use super::{CoreDriver, DriverUpdate, is_connection_failure, publish_latest};
    use crate::{CoreClient, CoreEndpoint, run_resident_core};
    use std::{
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    struct TestCore {
        endpoint: CoreEndpoint,
        thread: Option<thread::JoinHandle<Result<(), String>>>,
    }

    impl TestCore {
        fn start(scenario: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock after Unix epoch")
                .as_nanos();
            let endpoint = CoreEndpoint::for_profile(&format!(
                "core-driver-{scenario}-{}-{nonce}",
                std::process::id()
            ))
            .expect("create isolated Resident Core endpoint");
            let server_endpoint = endpoint.clone();
            let thread = thread::spawn(move || run_resident_core(server_endpoint));
            Self {
                endpoint,
                thread: Some(thread),
            }
        }
    }

    impl Drop for TestCore {
        fn drop(&mut self) {
            if let Ok(mut client) = CoreClient::connect(&self.endpoint, Duration::from_secs(10)) {
                let _ = client.stop_resident_core();
            }
            if let Some(thread) = self.thread.take() {
                assert_eq!(thread.join().expect("join Resident Core thread"), Ok(()));
            }
        }
    }

    #[test]
    fn terminal_output_reaches_the_view_without_snapshot_polling() {
        let core = TestCore::start("pushed-update");
        let mut client =
            CoreClient::connect(&core.endpoint, Duration::from_secs(10)).expect("attach UI Client");
        let mut initial = client.snapshot().expect("take initial snapshot");
        while client
            .wait_for_terminal_change(Duration::from_millis(100))
            .expect("wait for a quiet terminal baseline")
            .is_some()
        {
            if let Some(snapshot) = client
                .snapshot_since(initial.revision)
                .expect("refresh terminal baseline")
            {
                initial = snapshot;
            }
        }
        let driver = CoreDriver::start(client, initial.revision).expect("start UI Core driver");
        let updates = driver.updates();

        assert!(
            updates.recv_timeout(Duration::from_millis(100)).is_none(),
            "an idle UI Core driver must not publish polling updates"
        );
        let idle = driver.stats();
        assert_eq!(idle.snapshot_requests, 0);
        assert_eq!(idle.snapshots_published, 0);

        driver
            .input(b"echo DRIVER_PUSHED_UPDATE\r".to_vec())
            .expect("send terminal input");

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match updates.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Some(DriverUpdate::Snapshot(snapshot))
                    if snapshot.text().contains("DRIVER_PUSHED_UPDATE") =>
                {
                    break;
                }
                Some(DriverUpdate::Error(error)) => panic!("UI Core driver failed: {error}"),
                _ if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                _ => panic!("UI Core driver did not publish pushed terminal output"),
            }
        }
        let active = driver.stats();
        assert!(active.pushed_changes > 0);
        assert!(active.snapshot_requests > 0);
        assert!(active.snapshots_published > 0);
    }

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
