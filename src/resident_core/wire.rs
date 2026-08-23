use super::{
    ControlLease, ControlLeaseDenial, CoreCommandOutcome, MAX_MESSAGE_BYTES, Request, Response,
    SemanticEvent, SemanticEventKind, TerminalCell, TerminalChange, TerminalLifecycle,
    TerminalUpdate, UiClientId,
};
use crate::{
    CoreCommand, CoreModelError, CoreSnapshot, CreatedResource, PaneId, PaneLayout, PaneSnapshot,
    ResourceKind, SpaceId, SpaceSnapshot, SplitAxis, SplitId, SplitPlacement, SplitRatio,
    SplitSnapshot, TabId, TabSnapshot, TerminalLaunch, TerminalSessionId, TerminalSessionSnapshot,
};
use std::io::{self, Read, Write};

const REQUEST_HELLO: u8 = 1;
const REQUEST_INPUT: u8 = 2;
const REQUEST_RESIZE: u8 = 3;
const REQUEST_SNAPSHOT: u8 = 4;
const REQUEST_STOP_RESIDENT_CORE: u8 = 5;
const REQUEST_CONTROL_LEASE: u8 = 6;
const REQUEST_TRANSFER_CONTROL: u8 = 7;
const REQUEST_ACQUIRE_CONTROL: u8 = 8;
const REQUEST_DETACH: u8 = 9;
const REQUEST_CORE_SNAPSHOT: u8 = 10;
const REQUEST_APPLY_CORE_COMMAND: u8 = 11;
const REQUEST_PASTE: u8 = 12;
const RESPONSE_READY: u8 = 1;
const RESPONSE_ACK: u8 = 2;
const RESPONSE_SNAPSHOT: u8 = 3;
const RESPONSE_ERROR: u8 = 4;
const RESPONSE_TERMINAL_CHANGED: u8 = 5;
const RESPONSE_CONTROL_LEASE: u8 = 6;
const RESPONSE_CONTROL_LEASE_DENIED: u8 = 7;
const RESPONSE_SEMANTIC_EVENT: u8 = 8;
const RESPONSE_RESNAPSHOT_REQUIRED: u8 = 9;
const RESPONSE_CORE_SNAPSHOT: u8 = 10;
const RESPONSE_CORE_COMMAND_ACCEPTED: u8 = 11;
const RESPONSE_CORE_COMMAND_REJECTED: u8 = 12;
const MAX_COLLECTION_ITEMS: usize = 16_384;
const MAX_LAYOUT_DEPTH: usize = 64;

pub(super) fn encode_request(request: &Request) -> io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    match request {
        Request::Hello { version, nonce } => {
            payload.push(REQUEST_HELLO);
            payload.extend_from_slice(&version.to_le_bytes());
            payload.extend_from_slice(nonce);
        }
        Request::Input {
            terminal_session_id,
            lease_generation,
            bytes,
        } => {
            payload.push(REQUEST_INPUT);
            put_terminal_session_id(&mut payload, *terminal_session_id);
            payload.extend_from_slice(&lease_generation.to_le_bytes());
            put_bytes(&mut payload, bytes)?;
        }
        Request::Paste {
            terminal_session_id,
            lease_generation,
            bytes,
        } => {
            payload.push(REQUEST_PASTE);
            put_terminal_session_id(&mut payload, *terminal_session_id);
            payload.extend_from_slice(&lease_generation.to_le_bytes());
            put_bytes(&mut payload, bytes)?;
        }
        Request::Resize {
            terminal_session_id,
            lease_generation,
            size,
        } => {
            payload.push(REQUEST_RESIZE);
            put_terminal_session_id(&mut payload, *terminal_session_id);
            payload.extend_from_slice(&lease_generation.to_le_bytes());
            for value in [
                size.cols,
                size.rows,
                size.cell_width_px,
                size.cell_height_px,
            ] {
                payload.extend_from_slice(&value.to_le_bytes());
            }
        }
        Request::Snapshot {
            terminal_session_id,
            since,
        } => {
            payload.push(REQUEST_SNAPSHOT);
            put_terminal_session_id(&mut payload, *terminal_session_id);
            match since {
                Some(revision) => {
                    payload.push(1);
                    payload.extend_from_slice(&revision.to_le_bytes());
                }
                None => payload.push(0),
            }
        }
        Request::CoreSnapshot => payload.push(REQUEST_CORE_SNAPSHOT),
        Request::ApplyCoreCommand {
            expected_revision,
            command,
        } => {
            payload.push(REQUEST_APPLY_CORE_COMMAND);
            payload.extend_from_slice(&expected_revision.to_le_bytes());
            put_core_command(&mut payload, command)?;
        }
        Request::ControlLease {
            terminal_session_id,
        } => {
            payload.push(REQUEST_CONTROL_LEASE);
            put_terminal_session_id(&mut payload, *terminal_session_id);
        }
        Request::TransferControl {
            terminal_session_id,
            lease_generation,
            target,
        } => {
            payload.push(REQUEST_TRANSFER_CONTROL);
            put_terminal_session_id(&mut payload, *terminal_session_id);
            payload.extend_from_slice(&lease_generation.to_le_bytes());
            payload.extend_from_slice(&target.0.to_le_bytes());
        }
        Request::AcquireControl {
            terminal_session_id,
            lease_generation,
        } => {
            payload.push(REQUEST_ACQUIRE_CONTROL);
            put_terminal_session_id(&mut payload, *terminal_session_id);
            payload.extend_from_slice(&lease_generation.to_le_bytes());
        }
        Request::Detach => payload.push(REQUEST_DETACH),
        Request::StopResidentCore => payload.push(REQUEST_STOP_RESIDENT_CORE),
    }
    frame(payload)
}

pub(super) fn decode_request(frame: &[u8]) -> io::Result<Request> {
    let mut decoder = Decoder::new(payload(frame)?);
    let request = match decoder.u8()? {
        REQUEST_HELLO => Request::Hello {
            version: decoder.u16()?,
            nonce: decoder.array()?,
        },
        REQUEST_INPUT => Request::Input {
            terminal_session_id: decode_terminal_session_id(&mut decoder)?,
            lease_generation: decoder.u64()?,
            bytes: decoder.bytes()?.to_vec(),
        },
        REQUEST_PASTE => Request::Paste {
            terminal_session_id: decode_terminal_session_id(&mut decoder)?,
            lease_generation: decoder.u64()?,
            bytes: decoder.bytes()?.to_vec(),
        },
        REQUEST_RESIZE => Request::Resize {
            terminal_session_id: decode_terminal_session_id(&mut decoder)?,
            lease_generation: decoder.u64()?,
            size: crate::terminal_session::TerminalSize::new(
                decoder.u16()?,
                decoder.u16()?,
                decoder.u16()?,
                decoder.u16()?,
            ),
        },
        REQUEST_SNAPSHOT => Request::Snapshot {
            terminal_session_id: decode_terminal_session_id(&mut decoder)?,
            since: match decoder.u8()? {
                0 => None,
                1 => Some(decoder.u64()?),
                _ => return Err(invalid("invalid Snapshot revision flag")),
            },
        },
        REQUEST_CORE_SNAPSHOT => Request::CoreSnapshot,
        REQUEST_APPLY_CORE_COMMAND => Request::ApplyCoreCommand {
            expected_revision: decoder.u64()?,
            command: decode_core_command(&mut decoder)?,
        },
        REQUEST_CONTROL_LEASE => Request::ControlLease {
            terminal_session_id: decode_terminal_session_id(&mut decoder)?,
        },
        REQUEST_TRANSFER_CONTROL => Request::TransferControl {
            terminal_session_id: decode_terminal_session_id(&mut decoder)?,
            lease_generation: decoder.u64()?,
            target: UiClientId(decoder.u64()?),
        },
        REQUEST_ACQUIRE_CONTROL => Request::AcquireControl {
            terminal_session_id: decode_terminal_session_id(&mut decoder)?,
            lease_generation: decoder.u64()?,
        },
        REQUEST_DETACH => Request::Detach,
        REQUEST_STOP_RESIDENT_CORE => Request::StopResidentCore,
        _ => return Err(invalid("unknown Resident Core request kind")),
    };
    decoder.finish()?;
    Ok(request)
}

pub(super) fn encode_response(response: &Response) -> io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    match response {
        Response::Ready {
            version,
            proof,
            client_id,
            snapshot,
            leases,
            semantic_sequence,
        } => {
            payload.push(RESPONSE_READY);
            payload.extend_from_slice(&version.to_le_bytes());
            payload.extend_from_slice(proof);
            payload.extend_from_slice(&client_id.0.to_le_bytes());
            put_core_snapshot(&mut payload, snapshot)?;
            put_count(&mut payload, leases.len(), "too many Control Leases")?;
            for lease in leases {
                put_control_lease(&mut payload, lease);
            }
            payload.extend_from_slice(&semantic_sequence.to_le_bytes());
        }
        Response::Ack => payload.push(RESPONSE_ACK),
        Response::Snapshot(snapshot) => {
            payload.push(RESPONSE_SNAPSHOT);
            match snapshot {
                Some(snapshot) => {
                    payload.push(1);
                    put_snapshot(&mut payload, snapshot)?;
                }
                None => payload.push(0),
            }
        }
        Response::TerminalChanged(change) => {
            payload.push(RESPONSE_TERMINAL_CHANGED);
            payload.extend_from_slice(&change.sequence.to_le_bytes());
            put_terminal_session_id(&mut payload, change.terminal_session_id);
            payload.extend_from_slice(&change.terminal_revision.to_le_bytes());
        }
        Response::CoreSnapshot(snapshot) => {
            payload.push(RESPONSE_CORE_SNAPSHOT);
            put_core_snapshot(&mut payload, snapshot)?;
        }
        Response::CoreCommandAccepted(outcome) => {
            payload.push(RESPONSE_CORE_COMMAND_ACCEPTED);
            put_core_command_outcome(&mut payload, outcome)?;
        }
        Response::CoreCommandRejected(error) => {
            payload.push(RESPONSE_CORE_COMMAND_REJECTED);
            put_core_model_error(&mut payload, error)?;
        }
        Response::ControlLease(lease) => {
            payload.push(RESPONSE_CONTROL_LEASE);
            put_control_lease(&mut payload, lease);
        }
        Response::ControlLeaseDenied { reason, lease } => {
            payload.push(RESPONSE_CONTROL_LEASE_DENIED);
            payload.push(match reason {
                ControlLeaseDenial::HeldByOther => 0,
                ControlLeaseDenial::StaleGeneration => 1,
                ControlLeaseDenial::TargetUnavailable => 2,
                ControlLeaseDenial::NoController => 3,
            });
            put_control_lease(&mut payload, lease);
        }
        Response::SemanticEvent(event) => {
            payload.push(RESPONSE_SEMANTIC_EVENT);
            payload.extend_from_slice(&event.sequence.to_le_bytes());
            match &event.kind {
                SemanticEventKind::ControlLeaseChanged { lease } => {
                    payload.push(0);
                    put_control_lease(&mut payload, lease);
                }
                SemanticEventKind::TerminalLifecycleChanged {
                    terminal_session_id,
                    lifecycle,
                    terminal_revision,
                } => {
                    payload.push(1);
                    put_terminal_session_id(&mut payload, *terminal_session_id);
                    put_terminal_lifecycle(&mut payload, lifecycle)?;
                    payload.extend_from_slice(&terminal_revision.to_le_bytes());
                }
                SemanticEventKind::HierarchyChanged { revision } => {
                    payload.push(2);
                    payload.extend_from_slice(&revision.to_le_bytes());
                }
            }
        }
        Response::ResnapshotRequired => payload.push(RESPONSE_RESNAPSHOT_REQUIRED),
        Response::Error(error) => {
            payload.push(RESPONSE_ERROR);
            put_string(&mut payload, error)?;
        }
    }
    frame(payload)
}

pub(super) fn decode_response(frame: &[u8]) -> io::Result<Response> {
    let mut decoder = Decoder::new(payload(frame)?);
    let response = match decoder.u8()? {
        RESPONSE_READY => Response::Ready {
            version: decoder.u16()?,
            proof: decoder.array()?,
            client_id: UiClientId(decoder.u64()?),
            snapshot: decode_core_snapshot(&mut decoder)?,
            leases: decode_counted(&mut decoder, decode_control_lease)?,
            semantic_sequence: decoder.u64()?,
        },
        RESPONSE_ACK => Response::Ack,
        RESPONSE_SNAPSHOT => Response::Snapshot(match decoder.u8()? {
            0 => None,
            1 => Some(decode_snapshot(&mut decoder)?),
            _ => return Err(invalid("invalid Snapshot presence flag")),
        }),
        RESPONSE_ERROR => Response::Error(decoder.string()?.to_owned()),
        RESPONSE_TERMINAL_CHANGED => Response::TerminalChanged(TerminalChange {
            sequence: decoder.u64()?,
            terminal_session_id: decode_terminal_session_id(&mut decoder)?,
            terminal_revision: decoder.u64()?,
        }),
        RESPONSE_CORE_SNAPSHOT => Response::CoreSnapshot(decode_core_snapshot(&mut decoder)?),
        RESPONSE_CORE_COMMAND_ACCEPTED => {
            Response::CoreCommandAccepted(decode_core_command_outcome(&mut decoder)?)
        }
        RESPONSE_CORE_COMMAND_REJECTED => {
            Response::CoreCommandRejected(decode_core_model_error(&mut decoder)?)
        }
        RESPONSE_CONTROL_LEASE => Response::ControlLease(decode_control_lease(&mut decoder)?),
        RESPONSE_CONTROL_LEASE_DENIED => Response::ControlLeaseDenied {
            reason: match decoder.u8()? {
                0 => ControlLeaseDenial::HeldByOther,
                1 => ControlLeaseDenial::StaleGeneration,
                2 => ControlLeaseDenial::TargetUnavailable,
                3 => ControlLeaseDenial::NoController,
                _ => return Err(invalid("invalid Control Lease denial reason")),
            },
            lease: decode_control_lease(&mut decoder)?,
        },
        RESPONSE_SEMANTIC_EVENT => Response::SemanticEvent(SemanticEvent {
            sequence: decoder.u64()?,
            kind: match decoder.u8()? {
                0 => SemanticEventKind::ControlLeaseChanged {
                    lease: decode_control_lease(&mut decoder)?,
                },
                1 => SemanticEventKind::TerminalLifecycleChanged {
                    terminal_session_id: decode_terminal_session_id(&mut decoder)?,
                    lifecycle: decode_terminal_lifecycle(&mut decoder)?,
                    terminal_revision: decoder.u64()?,
                },
                2 => SemanticEventKind::HierarchyChanged {
                    revision: decoder.u64()?,
                },
                _ => return Err(invalid("invalid semantic event kind")),
            },
        }),
        RESPONSE_RESNAPSHOT_REQUIRED => Response::ResnapshotRequired,
        _ => return Err(invalid("unknown Resident Core response kind")),
    };
    decoder.finish()?;
    Ok(response)
}

pub(super) fn write_request<W: Write>(writer: &mut W, request: &Request) -> io::Result<()> {
    write_frame(writer, encode_request(request)?)
}

pub(super) fn read_request<R: Read>(reader: &mut R) -> io::Result<Option<Request>> {
    read_frame(reader)?
        .map(|frame| decode_request(&frame))
        .transpose()
}

pub(super) fn write_response<W: Write>(writer: &mut W, response: &Response) -> io::Result<()> {
    write_frame(writer, encode_response(response)?)
}

pub(super) fn read_response<R: Read>(reader: &mut R) -> io::Result<Option<Response>> {
    read_frame(reader)?
        .map(|frame| decode_response(&frame))
        .transpose()
}

pub(super) fn expected_frame_len(prefix: [u8; 4]) -> io::Result<usize> {
    let payload_len = u32::from_le_bytes(prefix) as usize;
    if payload_len as u64 > MAX_MESSAGE_BYTES {
        return Err(invalid("protocol frame exceeds 16 MiB"));
    }
    Ok(4 + payload_len)
}

fn write_frame<W: Write>(writer: &mut W, frame: Vec<u8>) -> io::Result<()> {
    writer.write_all(&frame)?;
    writer.flush()
}

fn read_frame<R: Read>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut prefix = [0_u8; 4];
    let first = reader.read(&mut prefix[..1])?;
    if first == 0 {
        return Ok(None);
    }
    reader.read_exact(&mut prefix[1..])?;
    let frame_len = expected_frame_len(prefix)?;
    let mut frame = Vec::with_capacity(frame_len);
    frame.extend_from_slice(&prefix);
    frame.resize(frame_len, 0);
    reader.read_exact(&mut frame[4..])?;
    Ok(Some(frame))
}

fn put_control_lease(output: &mut Vec<u8>, lease: &ControlLease) {
    put_terminal_session_id(output, lease.terminal_session_id);
    output.extend_from_slice(&lease.generation.to_le_bytes());
    match lease.controller {
        Some(controller) => {
            output.push(1);
            output.extend_from_slice(&controller.0.to_le_bytes());
        }
        None => output.push(0),
    }
}

fn decode_control_lease(decoder: &mut Decoder<'_>) -> io::Result<ControlLease> {
    let terminal_session_id = decode_terminal_session_id(decoder)?;
    let generation = decoder.u64()?;
    let controller = match decoder.u8()? {
        0 => None,
        1 => Some(UiClientId(decoder.u64()?)),
        _ => return Err(invalid("invalid Control Lease controller flag")),
    };
    Ok(ControlLease {
        terminal_session_id,
        generation,
        controller,
    })
}

fn put_terminal_session_id(output: &mut Vec<u8>, id: TerminalSessionId) {
    output.extend_from_slice(&id.as_u64().to_le_bytes());
}

fn decode_terminal_session_id(decoder: &mut Decoder<'_>) -> io::Result<TerminalSessionId> {
    Ok(TerminalSessionId::from_u64(decode_nonzero_id(decoder)?))
}

fn decode_nonzero_id(decoder: &mut Decoder<'_>) -> io::Result<u64> {
    let id = decoder.u64()?;
    if id == 0 {
        return Err(invalid("Core resource ID cannot be zero"));
    }
    Ok(id)
}

fn put_core_snapshot(output: &mut Vec<u8>, snapshot: &CoreSnapshot) -> io::Result<()> {
    output.extend_from_slice(&snapshot.revision.to_le_bytes());
    put_count(output, snapshot.spaces.len(), "too many Spaces")?;
    for space in &snapshot.spaces {
        output.extend_from_slice(&space.id.as_u64().to_le_bytes());
        put_string(output, &space.name)?;
        put_path(output, &space.directory)?;
        put_count(output, space.tabs.len(), "too many Tabs")?;
        for tab in &space.tabs {
            output.extend_from_slice(&tab.id.as_u64().to_le_bytes());
            put_string(output, &tab.name)?;
            put_pane_layout(output, &tab.layout, 0)?;
        }
    }
    put_count(
        output,
        snapshot.terminal_sessions.len(),
        "too many Terminal Sessions",
    )?;
    for terminal in &snapshot.terminal_sessions {
        put_terminal_session_id(output, terminal.id);
        put_terminal_launch(output, &terminal.launch)?;
    }
    Ok(())
}

fn decode_core_snapshot(decoder: &mut Decoder<'_>) -> io::Result<CoreSnapshot> {
    let revision = decoder.u64()?;
    let spaces = decode_counted(decoder, |decoder| {
        let id = SpaceId::from_u64(decode_nonzero_id(decoder)?);
        let name = decoder.string()?.to_owned();
        let directory = decode_path(decoder)?;
        let tabs = decode_counted(decoder, |decoder| {
            Ok(TabSnapshot {
                id: TabId::from_u64(decode_nonzero_id(decoder)?),
                name: decoder.string()?.to_owned(),
                layout: decode_pane_layout(decoder, 0)?,
            })
        })?;
        Ok(SpaceSnapshot {
            id,
            name,
            directory,
            tabs,
        })
    })?;
    let terminal_sessions = decode_counted(decoder, |decoder| {
        Ok(TerminalSessionSnapshot {
            id: decode_terminal_session_id(decoder)?,
            launch: decode_terminal_launch(decoder)?,
        })
    })?;
    Ok(CoreSnapshot {
        revision,
        spaces,
        terminal_sessions,
    })
}

fn put_pane_layout(output: &mut Vec<u8>, layout: &PaneLayout, depth: usize) -> io::Result<()> {
    if depth >= MAX_LAYOUT_DEPTH {
        return Err(invalid("Pane layout exceeds maximum depth"));
    }
    match layout {
        PaneLayout::Pane(pane) => {
            output.push(0);
            output.extend_from_slice(&pane.id.as_u64().to_le_bytes());
            put_terminal_session_id(output, pane.terminal_session_id);
        }
        PaneLayout::Split(split) => {
            output.push(1);
            output.extend_from_slice(&split.id.as_u64().to_le_bytes());
            put_split_axis(output, split.axis);
            output.extend_from_slice(&split.ratio.parts_per_thousand().to_le_bytes());
            put_pane_layout(output, &split.first, depth + 1)?;
            put_pane_layout(output, &split.second, depth + 1)?;
        }
    }
    Ok(())
}

fn decode_pane_layout(decoder: &mut Decoder<'_>, depth: usize) -> io::Result<PaneLayout> {
    if depth >= MAX_LAYOUT_DEPTH {
        return Err(invalid("Pane layout exceeds maximum depth"));
    }
    match decoder.u8()? {
        0 => Ok(PaneLayout::Pane(PaneSnapshot {
            id: PaneId::from_u64(decode_nonzero_id(decoder)?),
            terminal_session_id: decode_terminal_session_id(decoder)?,
        })),
        1 => Ok(PaneLayout::Split(SplitSnapshot {
            id: SplitId::from_u64(decode_nonzero_id(decoder)?),
            axis: decode_split_axis(decoder)?,
            ratio: decode_split_ratio(decoder)?,
            first: Box::new(decode_pane_layout(decoder, depth + 1)?),
            second: Box::new(decode_pane_layout(decoder, depth + 1)?),
        })),
        _ => Err(invalid("invalid Pane layout kind")),
    }
}

fn put_terminal_launch(output: &mut Vec<u8>, launch: &TerminalLaunch) -> io::Result<()> {
    put_path(output, &launch.working_directory)?;
    // Protocol v8 included Restore Disposition here. Preserve the field so a
    // new Desktop Shell can exchange snapshots with an already-running v8
    // Resident Core, but always advertise the removed relaunch/default value.
    output.push(0);
    Ok(())
}

fn decode_terminal_launch(decoder: &mut Decoder<'_>) -> io::Result<TerminalLaunch> {
    let working_directory = decode_path(decoder)?;
    match decoder.u8()? {
        // Both former Restore Disposition values now have the same meaning:
        // they are ignored because cold layout restoration no longer exists.
        0 | 1 => Ok(TerminalLaunch { working_directory }),
        _ => Err(invalid("invalid removed Restore Disposition")),
    }
}

fn put_core_command(output: &mut Vec<u8>, command: &CoreCommand) -> io::Result<()> {
    match command {
        CoreCommand::CreateSpace { name, directory } => {
            output.push(0);
            put_string(output, name)?;
            put_path(output, directory)?;
        }
        CoreCommand::RenameSpace { space_id, name } => {
            output.push(1);
            output.extend_from_slice(&space_id.as_u64().to_le_bytes());
            put_string(output, name)?;
        }
        CoreCommand::CreateTab { space_id, name } => {
            output.push(2);
            output.extend_from_slice(&space_id.as_u64().to_le_bytes());
            put_string(output, name)?;
        }
        CoreCommand::RenameTab { tab_id, name } => {
            output.push(3);
            output.extend_from_slice(&tab_id.as_u64().to_le_bytes());
            put_string(output, name)?;
        }
        CoreCommand::ReorderTab { tab_id, index } => {
            output.push(4);
            output.extend_from_slice(&tab_id.as_u64().to_le_bytes());
            put_index(output, *index)?;
        }
        CoreCommand::SplitPane {
            pane_id,
            axis,
            placement,
            ratio,
        } => {
            output.push(5);
            output.extend_from_slice(&pane_id.as_u64().to_le_bytes());
            put_split_axis(output, *axis);
            put_split_placement(output, *placement);
            output.extend_from_slice(&ratio.parts_per_thousand().to_le_bytes());
        }
        CoreCommand::MovePane {
            pane_id,
            target_pane_id,
            axis,
            placement,
            ratio,
        } => {
            output.push(6);
            output.extend_from_slice(&pane_id.as_u64().to_le_bytes());
            output.extend_from_slice(&target_pane_id.as_u64().to_le_bytes());
            put_split_axis(output, *axis);
            put_split_placement(output, *placement);
            output.extend_from_slice(&ratio.parts_per_thousand().to_le_bytes());
        }
        CoreCommand::ResizeSplit { split_id, ratio } => {
            output.push(7);
            output.extend_from_slice(&split_id.as_u64().to_le_bytes());
            output.extend_from_slice(&ratio.parts_per_thousand().to_le_bytes());
        }
        CoreCommand::ClosePane { pane_id } => {
            output.push(8);
            output.extend_from_slice(&pane_id.as_u64().to_le_bytes());
        }
        CoreCommand::CloseTab { tab_id } => {
            output.push(9);
            output.extend_from_slice(&tab_id.as_u64().to_le_bytes());
        }
        CoreCommand::CloseSpace { space_id } => {
            output.push(10);
            output.extend_from_slice(&space_id.as_u64().to_le_bytes());
        }
    }
    Ok(())
}

fn decode_core_command(decoder: &mut Decoder<'_>) -> io::Result<CoreCommand> {
    match decoder.u8()? {
        0 => Ok(CoreCommand::CreateSpace {
            name: decoder.string()?.to_owned(),
            directory: decode_path(decoder)?,
        }),
        1 => Ok(CoreCommand::RenameSpace {
            space_id: SpaceId::from_u64(decode_nonzero_id(decoder)?),
            name: decoder.string()?.to_owned(),
        }),
        2 => Ok(CoreCommand::CreateTab {
            space_id: SpaceId::from_u64(decode_nonzero_id(decoder)?),
            name: decoder.string()?.to_owned(),
        }),
        3 => Ok(CoreCommand::RenameTab {
            tab_id: TabId::from_u64(decode_nonzero_id(decoder)?),
            name: decoder.string()?.to_owned(),
        }),
        4 => Ok(CoreCommand::ReorderTab {
            tab_id: TabId::from_u64(decode_nonzero_id(decoder)?),
            index: decoder.u32()? as usize,
        }),
        5 => Ok(CoreCommand::SplitPane {
            pane_id: PaneId::from_u64(decode_nonzero_id(decoder)?),
            axis: decode_split_axis(decoder)?,
            placement: decode_split_placement(decoder)?,
            ratio: decode_split_ratio(decoder)?,
        }),
        6 => Ok(CoreCommand::MovePane {
            pane_id: PaneId::from_u64(decode_nonzero_id(decoder)?),
            target_pane_id: PaneId::from_u64(decode_nonzero_id(decoder)?),
            axis: decode_split_axis(decoder)?,
            placement: decode_split_placement(decoder)?,
            ratio: decode_split_ratio(decoder)?,
        }),
        7 => Ok(CoreCommand::ResizeSplit {
            split_id: SplitId::from_u64(decode_nonzero_id(decoder)?),
            ratio: decode_split_ratio(decoder)?,
        }),
        8 => Ok(CoreCommand::ClosePane {
            pane_id: PaneId::from_u64(decode_nonzero_id(decoder)?),
        }),
        9 => Ok(CoreCommand::CloseTab {
            tab_id: TabId::from_u64(decode_nonzero_id(decoder)?),
        }),
        10 => Ok(CoreCommand::CloseSpace {
            space_id: SpaceId::from_u64(decode_nonzero_id(decoder)?),
        }),
        _ => Err(invalid("invalid Core command kind")),
    }
}

fn put_core_command_outcome(output: &mut Vec<u8>, outcome: &CoreCommandOutcome) -> io::Result<()> {
    output.extend_from_slice(&outcome.revision.to_le_bytes());
    put_core_snapshot(output, &outcome.snapshot)?;
    put_created_resource(output, &outcome.created);
    put_count(
        output,
        outcome.control_leases.len(),
        "too many Control Leases",
    )?;
    for lease in &outcome.control_leases {
        put_control_lease(output, lease);
    }
    Ok(())
}

fn decode_core_command_outcome(decoder: &mut Decoder<'_>) -> io::Result<CoreCommandOutcome> {
    Ok(CoreCommandOutcome {
        revision: decoder.u64()?,
        snapshot: decode_core_snapshot(decoder)?,
        created: decode_created_resource(decoder)?,
        control_leases: decode_counted(decoder, decode_control_lease)?,
    })
}

fn put_created_resource(output: &mut Vec<u8>, created: &CreatedResource) {
    match created {
        CreatedResource::None => output.push(0),
        CreatedResource::Space {
            space_id,
            tab_id,
            pane_id,
            terminal_session_id,
        } => {
            output.push(1);
            output.extend_from_slice(&space_id.as_u64().to_le_bytes());
            output.extend_from_slice(&tab_id.as_u64().to_le_bytes());
            output.extend_from_slice(&pane_id.as_u64().to_le_bytes());
            put_terminal_session_id(output, *terminal_session_id);
        }
        CreatedResource::Tab {
            tab_id,
            pane_id,
            terminal_session_id,
        } => {
            output.push(2);
            output.extend_from_slice(&tab_id.as_u64().to_le_bytes());
            output.extend_from_slice(&pane_id.as_u64().to_le_bytes());
            put_terminal_session_id(output, *terminal_session_id);
        }
        CreatedResource::Pane {
            pane_id,
            split_id,
            terminal_session_id,
        } => {
            output.push(3);
            output.extend_from_slice(&pane_id.as_u64().to_le_bytes());
            output.extend_from_slice(&split_id.as_u64().to_le_bytes());
            put_terminal_session_id(output, *terminal_session_id);
        }
    }
}

fn decode_created_resource(decoder: &mut Decoder<'_>) -> io::Result<CreatedResource> {
    match decoder.u8()? {
        0 => Ok(CreatedResource::None),
        1 => Ok(CreatedResource::Space {
            space_id: SpaceId::from_u64(decode_nonzero_id(decoder)?),
            tab_id: TabId::from_u64(decode_nonzero_id(decoder)?),
            pane_id: PaneId::from_u64(decode_nonzero_id(decoder)?),
            terminal_session_id: decode_terminal_session_id(decoder)?,
        }),
        2 => Ok(CreatedResource::Tab {
            tab_id: TabId::from_u64(decode_nonzero_id(decoder)?),
            pane_id: PaneId::from_u64(decode_nonzero_id(decoder)?),
            terminal_session_id: decode_terminal_session_id(decoder)?,
        }),
        3 => Ok(CreatedResource::Pane {
            pane_id: PaneId::from_u64(decode_nonzero_id(decoder)?),
            split_id: SplitId::from_u64(decode_nonzero_id(decoder)?),
            terminal_session_id: decode_terminal_session_id(decoder)?,
        }),
        _ => Err(invalid("invalid created resource kind")),
    }
}

fn put_core_model_error(output: &mut Vec<u8>, error: &CoreModelError) -> io::Result<()> {
    match error {
        CoreModelError::StaleRevision { expected, actual } => {
            output.push(0);
            output.extend_from_slice(&expected.to_le_bytes());
            output.extend_from_slice(&actual.to_le_bytes());
        }
        CoreModelError::NotFound { kind, id } => {
            output.push(1);
            output.push(put_resource_kind(*kind));
            output.extend_from_slice(&id.to_le_bytes());
        }
        CoreModelError::InvalidName => output.push(2),
        CoreModelError::InvalidDirectory => output.push(3),
        CoreModelError::InvalidSplitRatio(ratio) => {
            output.push(4);
            output.extend_from_slice(&ratio.to_le_bytes());
        }
        CoreModelError::TabIndexOutOfBounds { index, tab_count } => {
            output.push(5);
            put_index(output, *index)?;
            put_index(output, *tab_count)?;
        }
        CoreModelError::CannotMovePaneOntoItself => output.push(6),
    }
    Ok(())
}

fn decode_core_model_error(decoder: &mut Decoder<'_>) -> io::Result<CoreModelError> {
    match decoder.u8()? {
        0 => Ok(CoreModelError::StaleRevision {
            expected: decoder.u64()?,
            actual: decoder.u64()?,
        }),
        1 => Ok(CoreModelError::NotFound {
            kind: decode_resource_kind(decoder.u8()?)?,
            id: decoder.u64()?,
        }),
        2 => Ok(CoreModelError::InvalidName),
        3 => Ok(CoreModelError::InvalidDirectory),
        4 => Ok(CoreModelError::InvalidSplitRatio(decoder.u16()?)),
        5 => Ok(CoreModelError::TabIndexOutOfBounds {
            index: decoder.u32()? as usize,
            tab_count: decoder.u32()? as usize,
        }),
        6 => Ok(CoreModelError::CannotMovePaneOntoItself),
        _ => Err(invalid("invalid Core model error kind")),
    }
}

fn put_split_axis(output: &mut Vec<u8>, axis: SplitAxis) {
    output.push(match axis {
        SplitAxis::Horizontal => 0,
        SplitAxis::Vertical => 1,
    });
}

fn decode_split_axis(decoder: &mut Decoder<'_>) -> io::Result<SplitAxis> {
    match decoder.u8()? {
        0 => Ok(SplitAxis::Horizontal),
        1 => Ok(SplitAxis::Vertical),
        _ => Err(invalid("invalid split axis")),
    }
}

fn put_split_placement(output: &mut Vec<u8>, placement: SplitPlacement) {
    output.push(match placement {
        SplitPlacement::Before => 0,
        SplitPlacement::After => 1,
    });
}

fn decode_split_placement(decoder: &mut Decoder<'_>) -> io::Result<SplitPlacement> {
    match decoder.u8()? {
        0 => Ok(SplitPlacement::Before),
        1 => Ok(SplitPlacement::After),
        _ => Err(invalid("invalid split placement")),
    }
}

fn decode_split_ratio(decoder: &mut Decoder<'_>) -> io::Result<SplitRatio> {
    SplitRatio::new(decoder.u16()?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn put_resource_kind(kind: ResourceKind) -> u8 {
    match kind {
        ResourceKind::Space => 0,
        ResourceKind::Tab => 1,
        ResourceKind::Pane => 2,
        ResourceKind::Split => 3,
        ResourceKind::TerminalSession => 4,
    }
}

fn decode_resource_kind(kind: u8) -> io::Result<ResourceKind> {
    match kind {
        0 => Ok(ResourceKind::Space),
        1 => Ok(ResourceKind::Tab),
        2 => Ok(ResourceKind::Pane),
        3 => Ok(ResourceKind::Split),
        4 => Ok(ResourceKind::TerminalSession),
        _ => Err(invalid("invalid Core resource kind")),
    }
}

fn put_index(output: &mut Vec<u8>, index: usize) -> io::Result<()> {
    let index = u32::try_from(index).map_err(|_| invalid("Core index is too large"))?;
    output.extend_from_slice(&index.to_le_bytes());
    Ok(())
}

fn put_count(output: &mut Vec<u8>, count: usize, message: &'static str) -> io::Result<()> {
    if count > MAX_COLLECTION_ITEMS {
        return Err(invalid(message));
    }
    put_index(output, count)
}

fn decode_counted<T>(
    decoder: &mut Decoder<'_>,
    mut decode: impl FnMut(&mut Decoder<'_>) -> io::Result<T>,
) -> io::Result<Vec<T>> {
    let count = decoder.u32()? as usize;
    if count > MAX_COLLECTION_ITEMS {
        return Err(invalid("protocol collection exceeds item limit"));
    }
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        values.push(decode(decoder)?);
    }
    Ok(values)
}

#[cfg(unix)]
fn put_path(output: &mut Vec<u8>, path: &std::path::Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    put_bytes(output, path.as_os_str().as_bytes())
}

#[cfg(unix)]
fn decode_path(decoder: &mut Decoder<'_>) -> io::Result<std::path::PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    Ok(std::ffi::OsString::from_vec(decoder.bytes()?.to_vec()).into())
}

#[cfg(windows)]
fn put_path(output: &mut Vec<u8>, path: &std::path::Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let units = path.as_os_str().encode_wide().collect::<Vec<_>>();
    let length = u32::try_from(units.len()).map_err(|_| invalid("path is too long"))?;
    output.extend_from_slice(&length.to_le_bytes());
    for unit in units {
        output.extend_from_slice(&unit.to_le_bytes());
    }
    Ok(())
}

#[cfg(windows)]
fn decode_path(decoder: &mut Decoder<'_>) -> io::Result<std::path::PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let count = decoder.u32()? as usize;
    if count > MAX_MESSAGE_BYTES as usize / 2 {
        return Err(invalid("path is too long"));
    }
    let mut units = Vec::with_capacity(count);
    for _ in 0..count {
        units.push(decoder.u16()?);
    }
    Ok(std::ffi::OsString::from_wide(&units).into())
}

fn put_terminal_lifecycle(output: &mut Vec<u8>, lifecycle: &TerminalLifecycle) -> io::Result<()> {
    match lifecycle {
        TerminalLifecycle::Running => output.push(0),
        TerminalLifecycle::Exited => output.push(1),
        TerminalLifecycle::Failed(error) => {
            output.push(2);
            put_string(output, error)?;
        }
    }
    Ok(())
}

fn decode_terminal_lifecycle(decoder: &mut Decoder<'_>) -> io::Result<TerminalLifecycle> {
    match decoder.u8()? {
        0 => Ok(TerminalLifecycle::Running),
        1 => Ok(TerminalLifecycle::Exited),
        2 => Ok(TerminalLifecycle::Failed(decoder.string()?.to_owned())),
        _ => Err(invalid("invalid Terminal Session lifecycle")),
    }
}

fn put_snapshot(output: &mut Vec<u8>, snapshot: &TerminalUpdate) -> io::Result<()> {
    snapshot
        .validate()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    match snapshot.base_revision {
        Some(revision) => {
            output.push(1);
            output.extend_from_slice(&revision.to_le_bytes());
        }
        None => output.push(0),
    }
    output.extend_from_slice(&snapshot.revision.to_le_bytes());
    put_terminal_lifecycle(output, &snapshot.lifecycle)?;
    output.extend_from_slice(&snapshot.cols.to_le_bytes());
    output.extend_from_slice(&snapshot.rows.to_le_bytes());
    match snapshot.cursor {
        Some((x, y)) => {
            output.push(1);
            output.extend_from_slice(&x.to_le_bytes());
            output.extend_from_slice(&y.to_le_bytes());
        }
        None => output.push(0),
    }
    output.extend_from_slice(&snapshot.default_fg);
    output.extend_from_slice(&snapshot.default_bg);
    let dirty_row_count = u16::try_from(snapshot.dirty_rows.len())
        .map_err(|_| invalid("snapshot contains too many dirty rows"))?;
    output.extend_from_slice(&dirty_row_count.to_le_bytes());
    for row in &snapshot.dirty_rows {
        output.extend_from_slice(&row.to_le_bytes());
    }
    let cell_count = u32::try_from(snapshot.cells.len())
        .map_err(|_| invalid("snapshot contains too many cells"))?;
    output.extend_from_slice(&cell_count.to_le_bytes());
    for cell in &snapshot.cells {
        output.extend_from_slice(&cell.x.to_le_bytes());
        output.extend_from_slice(&cell.y.to_le_bytes());
        output.push(cell.width);
        put_string(output, &cell.text)?;
        output.extend_from_slice(&cell.fg);
        output.extend_from_slice(&cell.bg);
        output.push(u8::from(cell.has_explicit_bg));
    }
    Ok(())
}

fn decode_snapshot(decoder: &mut Decoder<'_>) -> io::Result<TerminalUpdate> {
    let base_revision = match decoder.u8()? {
        0 => None,
        1 => Some(decoder.u64()?),
        _ => return Err(invalid("invalid base revision presence flag")),
    };
    let revision = decoder.u64()?;
    let lifecycle = decode_terminal_lifecycle(decoder)?;
    let cols = decoder.u16()?;
    let rows = decoder.u16()?;
    let cursor = match decoder.u8()? {
        0 => None,
        1 => Some((decoder.u16()?, decoder.u16()?)),
        _ => return Err(invalid("invalid cursor presence flag")),
    };
    let default_fg = decoder.array()?;
    let default_bg = decoder.array()?;
    let dirty_row_count = usize::from(decoder.u16()?);
    if dirty_row_count > usize::from(rows) {
        return Err(invalid("snapshot dirty row count exceeds grid height"));
    }
    let mut dirty_rows = Vec::with_capacity(dirty_row_count);
    for _ in 0..dirty_row_count {
        dirty_rows.push(decoder.u16()?);
    }
    let cell_count = decoder.u32()? as usize;
    if cell_count > crate::ghostty::SNAPSHOT_CELL_CAPACITY {
        return Err(invalid("snapshot cell count exceeds capacity"));
    }
    let mut cells = Vec::with_capacity(cell_count);
    for _ in 0..cell_count {
        let cell = TerminalCell {
            x: decoder.u16()?,
            y: decoder.u16()?,
            width: decoder.u8()?,
            text: decoder.string()?.to_owned(),
            fg: decoder.array()?,
            bg: decoder.array()?,
            has_explicit_bg: match decoder.u8()? {
                0 => false,
                1 => true,
                _ => return Err(invalid("invalid explicit-background flag")),
            },
        };
        if cell.x >= cols || cell.y >= rows {
            return Err(invalid("snapshot cell is outside the terminal grid"));
        }
        cells.push(cell);
    }
    if let Some((x, y)) = cursor
        && (x >= cols || y >= rows)
    {
        return Err(invalid("snapshot cursor is outside the terminal grid"));
    }
    let update = TerminalUpdate {
        base_revision,
        revision,
        lifecycle,
        cols,
        rows,
        cursor,
        default_fg,
        default_bg,
        dirty_rows,
        cells,
    };
    update
        .validate()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(update)
}

fn frame(payload: Vec<u8>) -> io::Result<Vec<u8>> {
    if payload.len() as u64 > MAX_MESSAGE_BYTES {
        return Err(invalid("protocol frame exceeds 16 MiB"));
    }
    let payload_len =
        u32::try_from(payload.len()).map_err(|_| invalid("protocol frame is too large"))?;
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&payload_len.to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

fn payload(frame: &[u8]) -> io::Result<&[u8]> {
    let length = frame
        .get(..4)
        .ok_or_else(|| invalid("protocol frame is missing its length prefix"))?;
    let length = u32::from_le_bytes(length.try_into().expect("four-byte prefix")) as usize;
    if length as u64 > MAX_MESSAGE_BYTES {
        return Err(invalid("protocol frame exceeds 16 MiB"));
    }
    if frame.len() != 4 + length {
        return Err(invalid("protocol frame length does not match its payload"));
    }
    Ok(&frame[4..])
}

fn put_bytes(output: &mut Vec<u8>, bytes: &[u8]) -> io::Result<()> {
    let length = u32::try_from(bytes.len()).map_err(|_| invalid("byte field is too large"))?;
    output.extend_from_slice(&length.to_le_bytes());
    output.extend_from_slice(bytes);
    Ok(())
}

fn put_string(output: &mut Vec<u8>, value: &str) -> io::Result<()> {
    put_bytes(output, value.as_bytes())
}

struct Decoder<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Decoder<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, length: usize) -> io::Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| invalid("protocol field length overflowed"))?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| invalid("protocol frame ended inside a field"))?;
        self.offset = end;
        Ok(bytes)
    }

    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().expect("two-byte field"),
        ))
    }

    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("four-byte field"),
        ))
    }

    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("eight-byte field"),
        ))
    }

    fn array<const N: usize>(&mut self) -> io::Result<[u8; N]> {
        Ok(self.take(N)?.try_into().expect("fixed-size field"))
    }

    fn bytes(&mut self) -> io::Result<&'a [u8]> {
        let length = self.u32()? as usize;
        self.take(length)
    }

    fn string(&mut self) -> io::Result<&'a str> {
        std::str::from_utf8(self.bytes()?).map_err(|_| invalid("protocol string is not UTF-8"))
    }

    fn finish(self) -> io::Result<()> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(invalid("protocol frame contains trailing bytes"))
        }
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::{
        Decoder, decode_core_command, decode_request, decode_response, decode_terminal_launch,
        encode_request, encode_response, put_path, put_terminal_launch, read_request,
        write_response,
    };
    use crate::resident_core::{
        ControlLease, ControlLeaseDenial, CoreCommandOutcome, Request, Response, SemanticEvent,
        SemanticEventKind, TerminalCell, TerminalChange, TerminalLifecycle, TerminalUpdate,
        UiClientId,
    };
    use crate::{
        CoreCommand, CoreModelError, CoreSnapshot, CreatedResource, PaneId, SpaceId, SplitAxis,
        SplitId, SplitPlacement, SplitRatio, TabId, TerminalLaunch, TerminalSessionId,
        terminal_session::TerminalSize,
    };

    #[test]
    fn terminal_launch_retains_the_v8_compatibility_byte() {
        let launch = TerminalLaunch::shell(std::env::current_dir().expect("current directory"));
        let mut path = Vec::new();
        put_path(&mut path, &launch.working_directory).expect("encode launch path");
        let mut encoded = Vec::new();
        put_terminal_launch(&mut encoded, &launch).expect("encode Terminal launch");

        assert_eq!(encoded.len(), path.len() + 1);
        assert_eq!(encoded.last(), Some(&0));

        let mut decoder = Decoder::new(&encoded);
        assert_eq!(
            decode_terminal_launch(&mut decoder).expect("decode v8 Terminal launch"),
            launch
        );
        decoder.finish().expect("consume compatibility byte");
    }

    #[test]
    fn hello_is_one_length_prefixed_binary_frame() {
        let request = Request::Hello {
            version: 0x0201,
            nonce: [0x5a; 32],
        };

        let encoded = encode_request(&request).expect("encode Hello");

        let mut expected = vec![35, 0, 0, 0, 1, 1, 2];
        expected.extend_from_slice(&[0x5a; 32]);
        assert_eq!(encoded, expected);
    }

    #[test]
    fn every_request_round_trips_arbitrary_terminal_bytes() {
        let terminal_session_id = TerminalSessionId::from_u64(16);
        let requests = [
            Request::Hello {
                version: 2,
                nonce: [7; 32],
            },
            Request::Input {
                terminal_session_id,
                lease_generation: 11,
                bytes: vec![0, 0xff, b'\n', b'{', b'}'],
            },
            Request::Paste {
                terminal_session_id,
                lease_generation: 12,
                bytes: "Unicode: 雪\nnext line".as_bytes().to_vec(),
            },
            Request::Resize {
                terminal_session_id,
                lease_generation: 13,
                size: TerminalSize::new(132, 43, 9, 18),
            },
            Request::Snapshot {
                terminal_session_id,
                since: None,
            },
            Request::Snapshot {
                terminal_session_id,
                since: Some(0x0102_0304_0506_0708),
            },
            Request::CoreSnapshot,
            Request::ControlLease {
                terminal_session_id,
            },
            Request::TransferControl {
                terminal_session_id,
                lease_generation: 13,
                target: UiClientId(14),
            },
            Request::AcquireControl {
                terminal_session_id,
                lease_generation: 15,
            },
            Request::Detach,
            Request::StopResidentCore,
        ];

        for request in requests {
            let encoded = encode_request(&request).expect("encode request");
            let decoded = decode_request(&encoded).expect("decode request");
            assert_eq!(decoded, request);
        }
    }

    #[test]
    fn every_core_command_round_trips_without_rust_layout_encoding() {
        let commands = [
            CoreCommand::CreateSpace {
                name: "Workspace λ".into(),
                directory: std::env::current_dir().expect("current directory"),
            },
            CoreCommand::RenameSpace {
                space_id: SpaceId::from_u64(1),
                name: "Renamed".into(),
            },
            CoreCommand::CreateTab {
                space_id: SpaceId::from_u64(1),
                name: "Tab".into(),
            },
            CoreCommand::RenameTab {
                tab_id: TabId::from_u64(2),
                name: "Renamed Tab".into(),
            },
            CoreCommand::ReorderTab {
                tab_id: TabId::from_u64(2),
                index: 3,
            },
            CoreCommand::SplitPane {
                pane_id: PaneId::from_u64(3),
                axis: SplitAxis::Horizontal,
                placement: SplitPlacement::After,
                ratio: SplitRatio::new(450).expect("valid ratio"),
            },
            CoreCommand::MovePane {
                pane_id: PaneId::from_u64(3),
                target_pane_id: PaneId::from_u64(4),
                axis: SplitAxis::Vertical,
                placement: SplitPlacement::Before,
                ratio: SplitRatio::new(600).expect("valid ratio"),
            },
            CoreCommand::ResizeSplit {
                split_id: SplitId::from_u64(5),
                ratio: SplitRatio::EQUAL,
            },
            CoreCommand::ClosePane {
                pane_id: PaneId::from_u64(3),
            },
            CoreCommand::CloseTab {
                tab_id: TabId::from_u64(2),
            },
            CoreCommand::CloseSpace {
                space_id: SpaceId::from_u64(1),
            },
        ];

        for command in commands {
            let request = Request::ApplyCoreCommand {
                expected_revision: 42,
                command,
            };
            assert_eq!(
                decode_request(&encode_request(&request).expect("encode Core command"))
                    .expect("decode Core command"),
                request
            );
        }
    }

    #[test]
    fn resource_close_commands_reject_missing_and_zero_ids() {
        assert!(decode_core_command(&mut Decoder::new(&[9])).is_err());
        assert!(decode_core_command(&mut Decoder::new(&[10])).is_err());

        let mut zero_id = vec![9];
        zero_id.extend_from_slice(&0_u64.to_le_bytes());
        let error = decode_core_command(&mut Decoder::new(&zero_id))
            .expect_err("CloseTab must reject a zero Tab ID");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);

        zero_id[0] = 10;
        let error = decode_core_command(&mut Decoder::new(&zero_id))
            .expect_err("CloseSpace must reject a zero Space ID");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn every_response_round_trips_a_compact_unicode_snapshot() {
        let terminal_session_id = TerminalSessionId::from_u64(16);
        let core_snapshot = CoreSnapshot {
            revision: 7,
            spaces: Vec::new(),
            terminal_sessions: Vec::new(),
        };
        let snapshot = TerminalUpdate {
            base_revision: None,
            revision: 0x0102_0304_0506_0708,
            lifecycle: TerminalLifecycle::Running,
            cols: 80,
            rows: 24,
            cursor: Some((3, 4)),
            default_fg: [0xdd, 0xdd, 0xdd],
            default_bg: [0x11, 0x11, 0x11],
            dirty_rows: (0..24).collect(),
            cells: (0..24)
                .flat_map(|y| {
                    (0..80).map(move |x| TerminalCell {
                        x,
                        y,
                        width: 1,
                        text: if (x, y) == (0, 0) {
                            "λ界".into()
                        } else {
                            String::new()
                        },
                        fg: [0xaa, 0xbb, 0xcc],
                        bg: [0x11, 0x11, 0x11],
                        has_explicit_bg: false,
                    })
                })
                .collect(),
        };
        let responses = [
            Response::Ready {
                version: 2,
                proof: [9; 32],
                client_id: UiClientId(17),
                snapshot: core_snapshot.clone(),
                leases: vec![ControlLease {
                    terminal_session_id,
                    generation: 18,
                    controller: Some(UiClientId(17)),
                }],
                semantic_sequence: 19,
            },
            Response::Ack,
            Response::Snapshot(None),
            Response::Snapshot(Some(snapshot)),
            Response::CoreSnapshot(core_snapshot.clone()),
            Response::CoreCommandAccepted(CoreCommandOutcome {
                revision: core_snapshot.revision,
                snapshot: core_snapshot,
                created: CreatedResource::None,
                control_leases: vec![ControlLease {
                    terminal_session_id,
                    generation: 18,
                    controller: Some(UiClientId(17)),
                }],
            }),
            Response::CoreCommandRejected(CoreModelError::StaleRevision {
                expected: 6,
                actual: 7,
            }),
            Response::TerminalChanged(TerminalChange {
                sequence: 23,
                terminal_session_id,
                terminal_revision: 42,
            }),
            Response::ControlLease(ControlLease {
                terminal_session_id,
                generation: 24,
                controller: Some(UiClientId(25)),
            }),
            Response::ControlLeaseDenied {
                reason: ControlLeaseDenial::HeldByOther,
                lease: ControlLease {
                    terminal_session_id,
                    generation: 26,
                    controller: Some(UiClientId(27)),
                },
            },
            Response::SemanticEvent(SemanticEvent {
                sequence: 28,
                kind: SemanticEventKind::TerminalLifecycleChanged {
                    terminal_session_id,
                    lifecycle: TerminalLifecycle::Exited,
                    terminal_revision: 29,
                },
            }),
            Response::ResnapshotRequired,
            Response::Error("failed: λ".into()),
        ];

        for response in responses {
            let encoded = encode_response(&response).expect("encode response");
            if matches!(response, Response::Snapshot(Some(_))) {
                assert!(
                    encoded.len() < 40_000,
                    "an 80x24 frame should be compact, got {} bytes",
                    encoded.len()
                );
            }
            let decoded = decode_response(&encoded).expect("decode response");
            assert_eq!(decoded, response);
        }
    }

    #[test]
    fn oversized_and_malformed_frames_fail_closed() {
        let oversized_prefix = ((16 * 1024 * 1024_u32) + 1).to_le_bytes();
        let error = read_request(&mut oversized_prefix.as_slice())
            .expect_err("reject oversized frame before reading its payload");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);

        let truncated = [2, 0, 0, 0, 1];
        assert!(decode_request(&truncated).is_err());

        let unknown_kind = [1, 0, 0, 0, 0xff];
        assert!(decode_request(&unknown_kind).is_err());

        let trailing_byte = [2, 0, 0, 0, 5, 0];
        assert!(decode_request(&trailing_byte).is_err());
    }

    #[test]
    fn a_response_is_written_as_one_complete_frame() {
        #[derive(Default)]
        struct CountingWriter {
            bytes: Vec<u8>,
            writes: usize,
        }

        impl std::io::Write for CountingWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.writes += 1;
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let mut writer = CountingWriter::default();
        write_response(&mut writer, &Response::Error("one frame".into())).expect("write response");

        assert_eq!(writer.writes, 1);
        assert_eq!(
            decode_response(&writer.bytes).expect("decode written response"),
            Response::Error("one frame".into())
        );
    }

    #[test]
    fn a_single_dirty_row_is_a_small_wire_update() {
        let update = TerminalUpdate {
            base_revision: Some(41),
            revision: 42,
            lifecycle: TerminalLifecycle::Running,
            cols: 80,
            rows: 24,
            cursor: Some((4, 7)),
            default_fg: [0xdd; 3],
            default_bg: [0x11; 3],
            dirty_rows: vec![7],
            cells: (0..80)
                .map(|x| TerminalCell {
                    x,
                    y: 7,
                    width: 1,
                    text: if x == 4 { "λ".into() } else { String::new() },
                    fg: [0xdd; 3],
                    bg: [0x11; 3],
                    has_explicit_bg: false,
                })
                .collect(),
        };

        let encoded = encode_response(&Response::Snapshot(Some(update.clone())))
            .expect("encode dirty-row update");

        assert!(
            encoded.len() < 1_500,
            "one 80-cell row should remain compact, got {} bytes",
            encoded.len()
        );
        assert_eq!(
            decode_response(&encoded).expect("decode dirty-row update"),
            Response::Snapshot(Some(update))
        );
    }
}
