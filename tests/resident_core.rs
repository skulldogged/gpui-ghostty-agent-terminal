use agent_terminal::{
    ControlLeaseDenial, CoreClient, CoreCommand, CoreCommandError, CoreEndpoint, CoreModelError,
    CreatedResource, PaneLayout, SemanticEventKind, TerminalLifecycle, TerminalSessionId,
    TerminalSize,
};
use std::{
    process::{Child, ChildStdin, Command, Stdio},
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
fn hierarchy_commands_run_multiple_terminals_and_reconnect_by_stable_id() {
    let endpoint = isolated_endpoint("multiplexer-hierarchy");
    let mut core = spawn_core(&endpoint);
    let mut controller = CoreClient::connect(&endpoint, Duration::from_secs(10))
        .expect("attach controlling UI Client");
    let first_terminal = controller
        .active_terminal_session_id()
        .expect("initial Terminal Session");
    let initial_revision = controller.core_snapshot().revision;

    let created = controller
        .apply_core_command(CoreCommand::CreateSpace {
            name: "Second Space".into(),
            directory: std::env::current_dir().expect("current directory"),
        })
        .expect("create a second Space through Resident Core IPC");
    let CreatedResource::Space {
        terminal_session_id: second_terminal,
        ..
    } = created.created
    else {
        panic!("Space creation must identify its Terminal Session");
    };
    assert!(created.revision > initial_revision);
    assert_eq!(created.snapshot.spaces.len(), 2);
    assert_eq!(created.snapshot.terminal_sessions.len(), 2);
    assert_eq!(
        controller
            .control_lease_for(second_terminal)
            .expect("new Terminal Session has a Control Lease")
            .controller,
        Some(controller.client_id())
    );

    controller
        .input_to(first_terminal, b"echo FIRST_MULTIPLEXED_TERMINAL\r")
        .expect("write first Terminal Session");
    controller
        .input_to(second_terminal, b"echo SECOND_MULTIPLEXED_TERMINAL\r")
        .expect("write second Terminal Session");
    wait_for_terminal_text(
        &mut controller,
        first_terminal,
        "FIRST_MULTIPLEXED_TERMINAL",
    );
    wait_for_terminal_text(
        &mut controller,
        second_terminal,
        "SECOND_MULTIPLEXED_TERMINAL",
    );

    let mut stale_observer = CoreClient::connect(&endpoint, Duration::from_secs(10))
        .expect("attach observer to complete hierarchy");
    assert_eq!(stale_observer.core_snapshot(), &created.snapshot);
    let second_lease = controller
        .transfer_terminal_control(second_terminal, stale_observer.client_id())
        .expect("transfer only the second Terminal Session lease");
    assert_eq!(second_lease.controller, Some(stale_observer.client_id()));
    assert_eq!(
        controller
            .control_lease_for(first_terminal)
            .expect("first Terminal Session lease remains available")
            .controller,
        Some(controller.client_id())
    );
    stale_observer
        .refresh_control_lease_for(second_terminal)
        .expect("observer refreshes second Terminal Session lease");
    stale_observer
        .input_to(second_terminal, b"echo INDEPENDENT_SECOND_LEASE\r")
        .expect("observer controls only the transferred Terminal Session");
    assert!(matches!(
        stale_observer.input_to(first_terminal, b"echo MUST_NOT_REACH_FIRST\r"),
        Err(CoreCommandError::ControlLeaseDenied {
            reason: ControlLeaseDenial::HeldByOther,
            ..
        })
    ));
    assert!(matches!(
        stale_observer.paste_to(first_terminal, b"MUST_NOT_PASTE_TO_FIRST"),
        Err(CoreCommandError::ControlLeaseDenied {
            reason: ControlLeaseDenial::HeldByOther,
            ..
        })
    ));
    wait_for_terminal_text(
        &mut stale_observer,
        second_terminal,
        "INDEPENDENT_SECOND_LEASE",
    );
    controller
        .apply_core_command(CoreCommand::RenameSpace {
            space_id: created.snapshot.spaces[1].id,
            name: "Renamed Space".into(),
        })
        .expect("rename Space");
    assert!(matches!(
        stale_observer.apply_core_command(CoreCommand::RenameSpace {
            space_id: created.snapshot.spaces[1].id,
            name: "Stale Rename".into(),
        }),
        Err(CoreCommandError::Rejected(
            CoreModelError::StaleRevision { .. }
        ))
    ));
    stale_observer
        .refresh_core_snapshot()
        .expect("observer resnapshots after structured stale rejection");
    drop(stale_observer);
    drop(controller);

    assert!(
        core.0.try_wait().expect("inspect Resident Core").is_none(),
        "Resident Core exited when every UI Client detached"
    );
    let mut reattached = CoreClient::connect(&endpoint, Duration::from_secs(10))
        .expect("reattach to multiplexed Resident Core");
    assert_eq!(reattached.core_snapshot().spaces.len(), 2);
    wait_for_terminal_text(
        &mut reattached,
        first_terminal,
        "FIRST_MULTIPLEXED_TERMINAL",
    );
    wait_for_terminal_text(
        &mut reattached,
        second_terminal,
        "SECOND_MULTIPLEXED_TERMINAL",
    );
    reattached.stop_resident_core().expect("stop Resident Core");
    wait_for_core_exit(&mut core);
}

#[test]
fn an_empty_hierarchy_can_reconnect_and_create_a_new_space() {
    let endpoint = isolated_endpoint("empty-hierarchy");
    let mut core = spawn_core(&endpoint);
    let mut client =
        CoreClient::connect(&endpoint, Duration::from_secs(10)).expect("attach UI Client");
    let pane_id = match &client.core_snapshot().spaces[0].tabs[0].layout {
        PaneLayout::Pane(pane) => pane.id,
        PaneLayout::Split(_) => panic!("initial Tab must contain one Pane"),
    };
    client
        .apply_core_command(CoreCommand::ClosePane { pane_id })
        .expect("close the final Pane");
    assert!(client.core_snapshot().spaces.is_empty());
    assert_eq!(client.active_terminal_session_id(), None);
    drop(client);

    let mut reattached = CoreClient::connect(&endpoint, Duration::from_secs(10))
        .expect("reattach to an empty hierarchy");
    assert!(reattached.core_snapshot().spaces.is_empty());
    let created = reattached
        .apply_core_command(CoreCommand::CreateSpace {
            name: "Recreated".into(),
            directory: std::env::current_dir().expect("current directory"),
        })
        .expect("create a Space from the empty state");
    let CreatedResource::Space {
        terminal_session_id,
        ..
    } = created.created
    else {
        panic!("Space creation must identify its Terminal Session");
    };
    assert_eq!(
        reattached.active_terminal_session_id(),
        Some(terminal_session_id)
    );
    reattached
        .input(b"echo RECREATED_AFTER_EMPTY\r")
        .expect("new Terminal Session is controllable");
    wait_for_text(&mut reattached, "RECREATED_AFTER_EMPTY");
    reattached.stop_resident_core().expect("stop Resident Core");
    wait_for_core_exit(&mut core);
}

#[test]
fn full_exit_stopper_waits_for_the_desktop_shell_before_stopping_the_core() {
    let endpoint = isolated_endpoint("full-exit-stopper");
    let mut core = spawn_core(&endpoint);
    let client = CoreClient::connect(&endpoint, Duration::from_secs(10)).expect("attach UI Client");
    drop(client);

    let (mut stopper, parent_lifetime) = spawn_full_exit_stopper(&endpoint);
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        stopper
            .0
            .try_wait()
            .expect("inspect full-exit stopper")
            .is_none(),
        "full-exit stopper did not wait for the Desktop Shell"
    );
    assert!(
        core.0.try_wait().expect("inspect Resident Core").is_none(),
        "full-exit stopper ended the Resident Core before the Desktop Shell exited"
    );

    drop(parent_lifetime);
    wait_for_process_exit(&mut stopper, "full-exit stopper");
    wait_for_core_exit(&mut core);
}

#[test]
fn terminal_changes_wake_the_ui_client_without_snapshot_polling() {
    let endpoint = isolated_endpoint("pushed-terminal-change");
    let mut core = spawn_core(&endpoint);
    let mut client =
        CoreClient::connect(&endpoint, Duration::from_secs(10)).expect("attach UI Client");
    let initial = client.snapshot().expect("take initial terminal snapshot");

    client
        .input(b"echo PUSHED_TERMINAL_CHANGE\r")
        .expect("write terminal input");

    let deadline = Instant::now() + Duration::from_secs(10);
    let change = loop {
        let change = client
            .wait_for_terminal_change(Duration::from_millis(250))
            .expect("wait for pushed terminal invalidation");
        if let Some(change) = change
            && change.terminal_revision > initial.revision
        {
            break change;
        }
        assert!(
            Instant::now() < deadline,
            "Resident Core did not push a terminal invalidation"
        );
    };

    let snapshot = client
        .snapshot_since(initial.revision)
        .expect("fetch changed terminal snapshot")
        .expect("terminal changed after pushed invalidation");
    assert_eq!(
        Some(change.terminal_session_id),
        client.active_terminal_session_id()
    );
    assert!(snapshot.revision >= change.terminal_revision);
    wait_for_text(&mut client, "PUSHED_TERMINAL_CHANGE");
    client.stop_resident_core().expect("stop Resident Core");
    wait_for_core_exit(&mut core);
}

#[test]
fn observer_can_attach_and_receive_an_explicit_control_lease_transfer() {
    let endpoint = isolated_endpoint("control-lease-transfer");
    let mut core = spawn_core(&endpoint);
    let mut controller = CoreClient::connect(&endpoint, Duration::from_secs(10))
        .expect("attach controlling UI Client");
    let mut observer =
        CoreClient::connect(&endpoint, Duration::from_secs(10)).expect("attach observer UI Client");

    let initial_lease = controller
        .control_lease()
        .expect("active Control Lease")
        .clone();
    let initial_generation = initial_lease.generation;
    assert_eq!(initial_lease.controller, Some(controller.client_id()));
    assert_eq!(observer.control_lease(), Some(&initial_lease));
    observer
        .snapshot()
        .expect("observer reads terminal snapshot");

    assert_eq!(
        observer
            .input(b"echo OBSERVER_MUST_NOT_CONTROL\r")
            .expect_err("observer input must be rejected"),
        CoreCommandError::ControlLeaseDenied {
            reason: ControlLeaseDenial::HeldByOther,
            lease: initial_lease.clone(),
        }
    );

    let transferred = controller
        .transfer_control(observer.client_id())
        .expect("transfer Control Lease to observer");
    assert_eq!(transferred.controller, Some(observer.client_id()));
    assert!(transferred.generation > initial_generation);
    assert_eq!(
        observer
            .refresh_control_lease()
            .expect("observer refreshes transferred Control Lease"),
        transferred
    );

    assert_eq!(
        controller
            .input(b"echo PREVIOUS_CONTROLLER_MUST_NOT_CONTROL\r")
            .expect_err("previous controller input must be rejected"),
        CoreCommandError::ControlLeaseDenied {
            reason: ControlLeaseDenial::HeldByOther,
            lease: transferred.clone(),
        }
    );
    observer
        .input(b"echo TRANSFERRED_CONTROL_LIVE\r")
        .expect("new controller writes terminal input");
    wait_for_text(&mut observer, "TRANSFERRED_CONTROL_LIVE");

    observer.stop_resident_core().expect("stop Resident Core");
    wait_for_core_exit(&mut core);
}

#[test]
fn control_lease_changes_are_delivered_as_ordered_semantic_events() {
    let endpoint = isolated_endpoint("semantic-control-lease");
    let mut core = spawn_core(&endpoint);
    let mut controller = CoreClient::connect(&endpoint, Duration::from_secs(10))
        .expect("attach controlling UI Client");
    let mut observer =
        CoreClient::connect(&endpoint, Duration::from_secs(10)).expect("attach observer UI Client");

    let transferred = controller
        .transfer_control(observer.client_id())
        .expect("transfer Control Lease to observer");
    let first = observer
        .wait_for_semantic_event(Duration::from_secs(10))
        .expect("wait for first semantic event")
        .expect("Control Lease transfer must publish a semantic event");
    assert_eq!(
        first.kind,
        SemanticEventKind::ControlLeaseChanged {
            lease: transferred.clone(),
        }
    );
    assert_eq!(observer.control_lease(), Some(&transferred));
    let controller_first = controller
        .wait_for_semantic_event(Duration::from_secs(10))
        .expect("wait for controller's first semantic event")
        .expect("controller must observe the same Control Lease transfer");
    assert_eq!(controller_first, first);

    let returned = observer
        .transfer_control(controller.client_id())
        .expect("transfer Control Lease back to original controller");
    let second = controller
        .wait_for_semantic_event(Duration::from_secs(10))
        .expect("wait for second semantic event")
        .expect("second Control Lease transfer must publish a semantic event");
    assert_eq!(second.sequence, first.sequence + 1);
    assert_eq!(
        second.kind,
        SemanticEventKind::ControlLeaseChanged { lease: returned }
    );

    controller.stop_resident_core().expect("stop Resident Core");
    wait_for_core_exit(&mut core);
}

#[test]
fn observer_can_acquire_the_control_lease_after_controller_detaches() {
    let endpoint = isolated_endpoint("control-lease-reacquire");
    let mut core = spawn_core(&endpoint);
    let controller = CoreClient::connect(&endpoint, Duration::from_secs(10))
        .expect("attach controlling UI Client");
    let mut observer =
        CoreClient::connect(&endpoint, Duration::from_secs(10)).expect("attach observer UI Client");

    assert_eq!(
        observer
            .acquire_control()
            .expect_err("held Control Lease must not be stolen"),
        CoreCommandError::ControlLeaseDenied {
            reason: ControlLeaseDenial::HeldByOther,
            lease: controller
                .control_lease()
                .expect("active Control Lease")
                .clone(),
        }
    );
    drop(controller);

    let vacancy = observer
        .wait_for_semantic_event(Duration::from_secs(10))
        .expect("wait for controller detach event")
        .expect("controller detach must publish a Control Lease event");
    let vacant = match vacancy.kind {
        SemanticEventKind::ControlLeaseChanged { lease } if lease.controller.is_none() => lease,
        event => panic!("expected vacant Control Lease event, got {event:?}"),
    };
    assert_eq!(observer.control_lease(), Some(&vacant));

    assert_eq!(
        observer
            .input(b"echo VACANT_LEASE_MUST_NOT_CONTROL\r")
            .expect_err("input without a controller must be rejected"),
        CoreCommandError::ControlLeaseDenied {
            reason: ControlLeaseDenial::NoController,
            lease: vacant.clone(),
        }
    );

    let acquired = observer
        .acquire_control()
        .expect("observer acquires vacant Control Lease");
    assert_eq!(acquired.controller, Some(observer.client_id()));
    assert!(acquired.generation > vacant.generation);
    observer
        .input(b"echo REACQUIRED_CONTROL_LIVE\r")
        .expect("new controller writes after detach");
    wait_for_text(&mut observer, "REACQUIRED_CONTROL_LIVE");

    observer.stop_resident_core().expect("stop Resident Core");
    wait_for_core_exit(&mut core);
}

#[test]
fn a_slow_semantic_observer_recovers_without_blocking_the_controller_or_terminal() {
    let endpoint = isolated_endpoint("semantic-pressure");
    let mut core = spawn_core(&endpoint);
    let mut first = CoreClient::connect(&endpoint, Duration::from_secs(10))
        .expect("attach first controlling UI Client");
    let mut second =
        CoreClient::connect(&endpoint, Duration::from_secs(10)).expect("attach second UI Client");
    let mut slow_observer =
        CoreClient::connect(&endpoint, Duration::from_secs(10)).expect("attach slow observer");

    for transfer in 0..80 {
        let (source, target) = if transfer % 2 == 0 {
            (&mut first, &mut second)
        } else {
            (&mut second, &mut first)
        };
        let lease = source
            .transfer_control(target.client_id())
            .expect("transfer Control Lease under sustained event pressure");
        for client in [source, target] {
            let event = client
                .wait_for_semantic_event(Duration::from_secs(10))
                .expect("active UI Client semantic stream remains connected")
                .expect("active UI Client receives every Control Lease event");
            assert_eq!(
                event.kind,
                SemanticEventKind::ControlLeaseChanged {
                    lease: lease.clone(),
                }
            );
        }
    }

    let overflow_deadline = Instant::now() + Duration::from_secs(10);
    let overflow = loop {
        match slow_observer.snapshot() {
            Err(error) => break error,
            Ok(_) if Instant::now() < overflow_deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(_) => panic!("a slow reliable-event consumer did not fail closed"),
        }
    };
    assert!(
        overflow.contains("reconnect and resnapshot required"),
        "unexpected slow-observer recovery error: {overflow}"
    );

    let mut recovered = CoreClient::connect(&endpoint, Duration::from_secs(10))
        .expect("reattach slow observer after overflow");
    recovered
        .snapshot()
        .expect("reattached observer takes an authoritative snapshot");

    first
        .input(b"echo CONTROLLER_SURVIVED_OBSERVER_PRESSURE\r")
        .expect("controller writes after observer overflow");
    wait_for_text(&mut first, "CONTROLLER_SURVIVED_OBSERVER_PRESSURE");

    first.stop_resident_core().expect("stop Resident Core");
    wait_for_core_exit(&mut core);
}

#[test]
fn snapshots_report_revisions_and_terminal_exit() {
    let endpoint = isolated_endpoint("revision-lifecycle");
    let mut core = spawn_core(&endpoint);
    let mut client =
        CoreClient::connect(&endpoint, Duration::from_secs(10)).expect("attach UI Client");

    client
        .input(shell_revision_hold_command())
        .expect("put the shell into a quiet revision hold");
    wait_for_text(&mut client, "RESIDENT_CORE_RESIZE_HOLD");

    let initial = client.snapshot().expect("take initial snapshot");
    let initial = wait_for_idle_snapshot(&mut client, initial);
    assert_eq!(initial.lifecycle, TerminalLifecycle::Running);

    client
        .resize(TerminalSize::default())
        .expect("repeat the current terminal size");
    assert!(
        client
            .snapshot_since(initial.revision)
            .expect("check unchanged resize revision")
            .is_none(),
        "an unchanged resize must not advance the terminal revision"
    );
    assert!(
        client.resize(TerminalSize::new(400, 200, 10, 20)).is_err(),
        "oversized grids must be rejected before reaching libghostty-vt"
    );

    client
        .input(b"\rexit\r")
        .expect("release shell revision hold and exit");
    let lifecycle_event = client
        .wait_for_semantic_event(Duration::from_secs(10))
        .expect("wait for Terminal Session lifecycle event")
        .expect("terminal exit must publish a semantic event");
    let lifecycle_revision = match lifecycle_event.kind {
        SemanticEventKind::TerminalLifecycleChanged {
            lifecycle: TerminalLifecycle::Exited,
            terminal_revision,
            ..
        } => terminal_revision,
        event => panic!("expected Terminal Session exit event, got {event:?}"),
    };
    let exited = wait_for_snapshot_after(&mut client, initial.revision);
    assert_eq!(exited.lifecycle, TerminalLifecycle::Exited);
    assert!(exited.revision >= lifecycle_revision);
    client.stop_resident_core().expect("stop Resident Core");
    wait_for_core_exit(&mut core);
}

#[cfg(windows)]
fn shell_revision_hold_command() -> &'static [u8] {
    b"Write-Output ('RESIDENT_CORE_'+'RESIZE_HOLD'); Read-Host\r"
}

#[cfg(unix)]
fn shell_revision_hold_command() -> &'static [u8] {
    b"printf '%s%s\\n' RESIDENT_CORE_ RESIZE_HOLD; read _\r"
}

fn wait_for_idle_snapshot(
    client: &mut CoreClient,
    mut snapshot: agent_terminal::TerminalSnapshot,
) -> agent_terminal::TerminalSnapshot {
    let deadline = Instant::now() + Duration::from_secs(10);
    let quiet_period = Duration::from_millis(100);
    let mut quiet_since = Instant::now();
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        match client
            .snapshot_since(snapshot.revision)
            .expect("check Terminal Session revision")
        {
            Some(changed) => {
                snapshot = changed;
                quiet_since = Instant::now();
            }
            None if quiet_since.elapsed() >= quiet_period => return snapshot,
            None => {}
        }
    }
    panic!("Terminal Session did not reach an unchanged revision before timeout");
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

fn spawn_full_exit_stopper(endpoint: &CoreEndpoint) -> (ChildGuard, ChildStdin) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agent-terminal"))
        .arg("--stop-resident-core-after-parent")
        .arg(endpoint.argument())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn full-exit stopper");
    let parent_lifetime = child.stdin.take().expect("open parent-lifetime pipe");
    (ChildGuard(child), parent_lifetime)
}

fn wait_for_core_exit(core: &mut ChildGuard) {
    wait_for_process_exit(core, "Resident Core");
}

fn wait_for_process_exit(process: &mut ChildGuard, process_name: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if process
            .0
            .try_wait()
            .unwrap_or_else(|error| panic!("wait for {process_name}: {error}"))
            .is_some()
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("{process_name} did not exit before timeout");
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
    let terminal_session_id = client
        .active_terminal_session_id()
        .expect("active Terminal Session");
    wait_for_terminal_text(client, terminal_session_id, marker);
}

fn wait_for_terminal_text(
    client: &mut CoreClient,
    terminal_session_id: TerminalSessionId,
    marker: &str,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let snapshot = client
            .terminal_snapshot(terminal_session_id)
            .expect("snapshot Terminal Session");
        let visual_rows = snapshot.text();
        let unwrapped = visual_rows.lines().collect::<String>();
        if unwrapped.contains(marker) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("Terminal Session did not contain {marker:?} before timeout");
}
