use agent_terminal::{CoreClient, CoreEndpoint, TerminalLifecycle};
use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[test]
fn terminal_session_survives_ui_client_disconnect_and_reconnect() {
    let endpoint = isolated_endpoint("detach-reconnect");
    let mut core = spawn_core(&endpoint);

    let mut first =
        CoreClient::connect(&endpoint, Duration::from_secs(10)).expect("attach first UI Client");
    first
        .input(b"echo BEFORE_UI_DETACH\r")
        .expect("write through first UI Client");
    wait_for_text(&mut first, "BEFORE_UI_DETACH");
    drop(first);

    assert!(
        core.0.try_wait().expect("inspect Resident Core").is_none(),
        "Resident Core exited when its UI Client disconnected"
    );

    let mut second =
        CoreClient::connect(&endpoint, Duration::from_secs(10)).expect("reattach UI Client");
    second
        .input(b"echo AFTER_UI_REATTACH\r")
        .expect("write through reattached UI Client");
    wait_for_text(&mut second, "AFTER_UI_REATTACH");
    second.stop_resident_core().expect("stop Resident Core");

    wait_for_core_exit(&mut core);
}

#[test]
fn snapshots_report_revisions_and_terminal_exit() {
    let endpoint = isolated_endpoint("revision-lifecycle");
    let mut core = spawn_core(&endpoint);
    let mut client =
        CoreClient::connect(&endpoint, Duration::from_secs(10)).expect("attach UI Client");

    let initial = client.snapshot().expect("take initial snapshot");
    assert_eq!(initial.lifecycle, TerminalLifecycle::Running);
    assert!(
        client
            .snapshot_since(initial.revision)
            .expect("check unchanged Terminal Session")
            .is_none(),
        "an idle Terminal Session should not rebuild an unchanged snapshot"
    );

    client.input(b"exit\r").expect("exit terminal shell");
    let exited = wait_for_snapshot_after(&mut client, initial.revision);
    assert_eq!(exited.lifecycle, TerminalLifecycle::Exited);
    client.stop_resident_core().expect("stop Resident Core");
    wait_for_core_exit(&mut core);
}

#[cfg(unix)]
#[test]
fn resident_core_reclaims_a_verified_stale_socket() {
    let endpoint = isolated_endpoint("stale-socket");
    let mut crashed = spawn_core(&endpoint);
    let client =
        CoreClient::connect(&endpoint, Duration::from_secs(10)).expect("attach first UI Client");
    drop(client);
    crashed.0.kill().expect("kill first Resident Core");
    crashed.0.wait().expect("reap first Resident Core");

    let mut replacement = spawn_core(&endpoint);
    let mut client = CoreClient::connect(&endpoint, Duration::from_secs(10))
        .expect("attach replacement Resident Core");
    client
        .stop_resident_core()
        .expect("stop replacement Resident Core");
    wait_for_core_exit(&mut replacement);
}

fn isolated_endpoint(scenario: &str) -> CoreEndpoint {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after Unix epoch")
        .as_nanos();
    CoreEndpoint::for_profile(&format!(
        "resident-core-{scenario}-{}-{nonce}",
        std::process::id()
    ))
    .expect("create isolated Resident Core endpoint")
}

fn spawn_core(endpoint: &CoreEndpoint) -> ChildGuard {
    ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_agent-terminal"))
            .arg("--resident-core")
            .arg(endpoint.argument())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn Resident Core process"),
    )
}

fn wait_for_core_exit(core: &mut ChildGuard) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if core.0.try_wait().expect("wait for Resident Core").is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("Resident Core did not stop after an explicit stop command");
}

fn wait_for_snapshot_after(
    client: &mut CoreClient,
    mut revision: u64,
) -> agent_terminal::TerminalSnapshot {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        match client
            .snapshot_since(revision)
            .expect("request changed Terminal Session snapshot")
        {
            Some(snapshot) if snapshot.lifecycle != TerminalLifecycle::Running => return snapshot,
            Some(snapshot) => {
                revision = snapshot.revision;
            }
            None => {}
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("Terminal Session did not report a lifecycle change before timeout");
}

fn wait_for_text(client: &mut CoreClient, marker: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let snapshot = client.snapshot().expect("snapshot Terminal Session");
        if snapshot.text().contains(marker) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("Terminal Session did not contain {marker:?} before timeout");
}
