//! Binary websocket frames for terminals. What a shell prints isn't valid UTF-8 at every
//! chunk boundary, and escaping it into JSON would cost time on every keystroke's echo.
//!
//! A frame is its kind, the length of the terminal's ID, the ID, then the payload.

use crate::TerminalId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    /// Daemon to client: what the terminal printed, to write after what came before.
    Output = 1,
    /// Daemon to client: start over. The payload is the columns and rows as two `u16`s, then
    /// what recreates everything the terminal shows. It comes first after attaching, after a
    /// resize, and instead of output a client fell too far behind on.
    Reset = 2,
    /// Client to daemon: what was typed or pasted.
    Input = 3,
    /// Client to daemon: how many payload bytes of output and resets the client has drawn,
    /// as a `u32`. A client that doesn't acknowledge is sent nothing more until it does.
    Ack = 4,
}

impl Kind {
    fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            1 => Self::Output,
            2 => Self::Reset,
            3 => Self::Input,
            4 => Self::Ack,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame<'a> {
    pub kind: Kind,
    pub terminal_id: &'a str,
    pub payload: &'a [u8],
}

pub fn encode(kind: Kind, terminal_id: &TerminalId, payload: &[u8]) -> Vec<u8> {
    let id = terminal_id.0.as_bytes();
    // IDs are generated and short, so this never truncates one.
    let id_len = u8::try_from(id.len()).expect("terminal IDs are short");
    let mut frame = Vec::with_capacity(2 + id.len() + payload.len());
    frame.push(kind as u8);
    frame.push(id_len);
    frame.extend_from_slice(id);
    frame.extend_from_slice(payload);
    frame
}

/// `None` for a frame this build doesn't understand, which is skipped like an unknown message.
pub fn decode(frame: &[u8]) -> Option<Frame<'_>> {
    let (&kind, rest) = frame.split_first()?;
    let (&id_len, rest) = rest.split_first()?;
    let id_len = usize::from(id_len);
    if rest.len() < id_len {
        return None;
    }
    let (id, payload) = rest.split_at(id_len);
    Some(Frame {
        kind: Kind::from_byte(kind)?,
        terminal_id: std::str::from_utf8(id).ok()?,
        payload,
    })
}

pub fn reset_payload(cols: u16, rows: u16, screen: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(4 + screen.len());
    payload.extend_from_slice(&cols.to_le_bytes());
    payload.extend_from_slice(&rows.to_le_bytes());
    payload.extend_from_slice(screen);
    payload
}

/// The columns, rows and screen of a reset's payload.
pub fn read_reset(payload: &[u8]) -> Option<(u16, u16, &[u8])> {
    let cols = u16::from_le_bytes(payload.get(0..2)?.try_into().ok()?);
    let rows = u16::from_le_bytes(payload.get(2..4)?.try_into().ok()?);
    Some((cols, rows, &payload[4..]))
}

pub fn ack_payload(bytes: u32) -> [u8; 4] {
    bytes.to_le_bytes()
}

pub fn read_ack(payload: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(payload.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_with_any_bytes() {
        let id = TerminalId::from("0123abcd");
        let payload = [0xff, 0x1b, b'[', 0x00, 0xe2];
        let encoded = encode(Kind::Output, &id, &payload);
        assert_eq!(
            decode(&encoded),
            Some(Frame {
                kind: Kind::Output,
                terminal_id: "0123abcd",
                payload: &payload,
            })
        );
    }

    #[test]
    fn a_short_or_unknown_frame_is_skipped() {
        assert_eq!(decode(&[]), None);
        assert_eq!(decode(&[1]), None);
        assert_eq!(decode(&[1, 9, b'a']), None);
        assert_eq!(decode(&[99, 1, b'a']), None);
    }

    #[test]
    fn a_reset_carries_its_size_and_an_ack_its_count() {
        let payload = reset_payload(120, 40, b"screen");
        assert_eq!(read_reset(&payload), Some((120, 40, &b"screen"[..])));
        assert_eq!(read_reset(&[1, 2, 3]), None);
        assert_eq!(read_ack(&ack_payload(70_000)), Some(70_000));
        assert_eq!(read_ack(&[1, 2]), None);
    }
}
