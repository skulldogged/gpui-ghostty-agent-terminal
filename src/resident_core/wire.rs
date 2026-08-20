use super::{
    MAX_MESSAGE_BYTES, Request, Response, TerminalCell, TerminalChange, TerminalLifecycle,
    TerminalUpdate,
};
use std::io::{self, Read, Write};

const REQUEST_HELLO: u8 = 1;
const REQUEST_INPUT: u8 = 2;
const REQUEST_RESIZE: u8 = 3;
const REQUEST_SNAPSHOT: u8 = 4;
const REQUEST_STOP_RESIDENT_CORE: u8 = 5;
const RESPONSE_READY: u8 = 1;
const RESPONSE_ACK: u8 = 2;
const RESPONSE_SNAPSHOT: u8 = 3;
const RESPONSE_ERROR: u8 = 4;
const RESPONSE_TERMINAL_CHANGED: u8 = 5;

pub(super) fn encode_request(request: &Request) -> io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    match request {
        Request::Hello { version, nonce } => {
            payload.push(REQUEST_HELLO);
            payload.extend_from_slice(&version.to_le_bytes());
            payload.extend_from_slice(nonce);
        }
        Request::Input { bytes } => {
            payload.push(REQUEST_INPUT);
            put_bytes(&mut payload, bytes)?;
        }
        Request::Resize { size } => {
            payload.push(REQUEST_RESIZE);
            for value in [
                size.cols,
                size.rows,
                size.cell_width_px,
                size.cell_height_px,
            ] {
                payload.extend_from_slice(&value.to_le_bytes());
            }
        }
        Request::Snapshot { since } => {
            payload.push(REQUEST_SNAPSHOT);
            match since {
                Some(revision) => {
                    payload.push(1);
                    payload.extend_from_slice(&revision.to_le_bytes());
                }
                None => payload.push(0),
            }
        }
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
            bytes: decoder.bytes()?.to_vec(),
        },
        REQUEST_RESIZE => Request::Resize {
            size: crate::terminal_session::TerminalSize::new(
                decoder.u16()?,
                decoder.u16()?,
                decoder.u16()?,
                decoder.u16()?,
            ),
        },
        REQUEST_SNAPSHOT => Request::Snapshot {
            since: match decoder.u8()? {
                0 => None,
                1 => Some(decoder.u64()?),
                _ => return Err(invalid("invalid Snapshot revision flag")),
            },
        },
        REQUEST_STOP_RESIDENT_CORE => Request::StopResidentCore,
        _ => return Err(invalid("unknown Resident Core request kind")),
    };
    decoder.finish()?;
    Ok(request)
}

pub(super) fn encode_response(response: &Response) -> io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    match response {
        Response::Ready { version, proof } => {
            payload.push(RESPONSE_READY);
            payload.extend_from_slice(&version.to_le_bytes());
            payload.extend_from_slice(proof);
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
            payload.extend_from_slice(&change.terminal_revision.to_le_bytes());
        }
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
            terminal_revision: decoder.u64()?,
        }),
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
    match &snapshot.lifecycle {
        TerminalLifecycle::Running => output.push(0),
        TerminalLifecycle::Exited => output.push(1),
        TerminalLifecycle::Failed(error) => {
            output.push(2);
            put_string(output, error)?;
        }
    }
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
    let lifecycle = match decoder.u8()? {
        0 => TerminalLifecycle::Running,
        1 => TerminalLifecycle::Exited,
        2 => TerminalLifecycle::Failed(decoder.string()?.to_owned()),
        _ => return Err(invalid("invalid Terminal Session lifecycle")),
    };
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
        decode_request, decode_response, encode_request, encode_response, read_request,
        write_response,
    };
    use crate::resident_core::{
        Request, Response, TerminalCell, TerminalChange, TerminalLifecycle, TerminalUpdate,
    };
    use crate::terminal_session::TerminalSize;

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
        let requests = [
            Request::Hello {
                version: 2,
                nonce: [7; 32],
            },
            Request::Input {
                bytes: vec![0, 0xff, b'\n', b'{', b'}'],
            },
            Request::Resize {
                size: TerminalSize::new(132, 43, 9, 18),
            },
            Request::Snapshot { since: None },
            Request::Snapshot {
                since: Some(0x0102_0304_0506_0708),
            },
            Request::StopResidentCore,
        ];

        for request in requests {
            let encoded = encode_request(&request).expect("encode request");
            let decoded = decode_request(&encoded).expect("decode request");
            assert_eq!(decoded, request);
        }
    }

    #[test]
    fn every_response_round_trips_a_compact_unicode_snapshot() {
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
            },
            Response::Ack,
            Response::Snapshot(None),
            Response::Snapshot(Some(snapshot)),
            Response::TerminalChanged(TerminalChange {
                sequence: 23,
                terminal_revision: 42,
            }),
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
