use agent_terminal::{CoreClient, CoreEndpoint};
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
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after Unix epoch")
        .as_nanos();
    let endpoint = CoreEndpoint::for_profile(&format!(
        "resident-core-integration-{}-{nonce}",
        std::process::id()
    ))
    .expect("create isolated Resident Core endpoint");
    let child = Command::new(env!("CARGO_BIN_EXE_agent-terminal"))
        .arg("--resident-core")
        .arg(endpoint.argument())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn Resident Core process");
    let mut core = ChildGuard(child);

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

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if core.0.try_wait().expect("wait for Resident Core").is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("Resident Core did not stop after an explicit stop command");
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
