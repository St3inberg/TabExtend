//! The TabExtend wire protocol, spoken inside TLS.
//!
//! Every message is `[length: u32 BE][tag: u8][payload]`, where `length`
//! counts the tag and payload. Integers are big-endian; strings are a `u16`
//! byte length followed by UTF-8.
//!
//! Everything a peer sends is untrusted. Each tag has a maximum size, checked
//! from the length prefix before the payload is read, so a peer cannot make
//! the other side allocate more than that. Parsing never panics.
//!
//! The Android client has a matching implementation in
//! `android/app/src/main/java/org/tabextend/client/Protocol.kt`.

use std::fmt;

pub const PROTOCOL_VERSION: u16 = 1;

/// Largest video frame accepted, in bytes. A 2560×1600 keyframe at a high
/// bitrate is around 1 MiB.
pub const MAX_VIDEO_FRAME: usize = 16 * 1024 * 1024;
/// Largest audio chunk accepted, in bytes (about 340 ms of the format below).
pub const MAX_AUDIO_CHUNK: usize = 64 * 1024;
/// Audio is sent as raw PCM: 48 kHz, 2 channels, signed 16-bit little-endian,
/// interleaved. 1.5 Mbit/s is small next to the video and adds no codec delay.
pub const AUDIO_RATE: u32 = 48_000;
pub const AUDIO_CHANNELS: u32 = 2;
/// Largest control message accepted, in bytes.
pub const MAX_CONTROL_MESSAGE: usize = 4096;
/// Longest device name accepted, in bytes.
pub const MAX_NAME: usize = 64;
/// Length of a pairing token, in bytes.
pub const PAIRING_TOKEN_LEN: usize = 16;
/// Length of a pairing nonce and of a commitment to one, in bytes.
pub const NONCE_LEN: usize = 32;

/// What a tablet's screen can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayInfo {
    pub width: u32,
    pub height: u32,
    pub density_dpi: u32,
    /// Refresh rate in millihertz (90 Hz = 90 000).
    pub refresh_millihz: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Codec {
    H264 = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RejectReason {
    UnsupportedVersion = 1,
    NotPaired = 2,
    BadPairingToken = 3,
    Busy = 4,
    CaptureFailed = 5,
    /// The user declined the pairing on the computer, or it timed out.
    PairingDenied = 6,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TouchPhase {
    Down = 0,
    Move = 1,
    Up = 2,
    /// The touch was taken over (e.g. by palm rejection); end it.
    Cancel = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MouseButton {
    Left = 1,
    Middle = 2,
    Right = 3,
    Back = 4,
    Forward = 5,
}

/// Input from the tablet. Positions are fractions of the stream, 0.0–1.0,
/// so the tablet needs no knowledge of the host's pixel sizes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputEvent {
    /// One finger. Slots identify fingers while they are down.
    Touch {
        slot: u8,
        phase: TouchPhase,
        x: f32,
        y: f32,
    },
    /// Mouse, trackpad or a hovering/touching pen.
    PointerMove {
        x: f32,
        y: f32,
    },
    Button {
        button: MouseButton,
        pressed: bool,
    },
    /// Wheel steps. Positive is down and right, as in libinput.
    Scroll {
        horizontal: i16,
        vertical: i16,
    },
    /// A physical key, as a Linux evdev key code (what Android reports as
    /// the scan code).
    Key {
        code: u16,
        pressed: bool,
    },
}

/// How a tablet asks to be trusted, if it is not paired yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingRequest {
    /// Already paired: the certificate is enough.
    None,
    /// The single-use token from the host's QR code.
    Token([u8; PAIRING_TOKEN_LEN]),
    /// Found by scanning: both sides show a code and the user confirms on
    /// the computer. Carries the commitment to the tablet's nonce; see
    /// `pairing::confirmation_code`.
    Confirm { commitment: [u8; NONCE_LEN] },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// Client → host, first message on every connection.
    Hello {
        version: u16,
        pairing: PairingRequest,
        device_name: String,
        display: DisplayInfo,
    },
    /// Host → client: accepted; video follows at this size.
    Welcome {
        version: u16,
        codec: Codec,
        width: u32,
        height: u32,
    },
    /// Host → client: refused; the host closes the connection after this.
    Reject {
        reason: RejectReason,
        detail: String,
    },
    /// Host → client: one encoded access unit (Annex B).
    Video {
        pts_us: u64,
        keyframe: bool,
        data: Vec<u8>,
    },
    /// Either side; the other answers with `Pong` carrying the same values.
    Ping {
        id: u32,
        sent_us: u64,
    },
    Pong {
        id: u32,
        sent_us: u64,
    },
    /// Client → host: the decoder lost its place; send a keyframe.
    RequestKeyframe,
    /// Host → client, during confirm pairing: the host's random nonce.
    PairChallenge {
        nonce: [u8; NONCE_LEN],
    },
    /// Client → host, after the challenge: the nonce it committed to.
    PairReveal {
        nonce: [u8; NONCE_LEN],
    },
    /// Client → host: touch, pointer, button, scroll or key.
    Input(InputEvent),
    /// Host → client: a chunk of the laptop's sound, in the format above.
    Audio {
        pts_us: u64,
        pcm: Vec<u8>,
    },
}

mod tag {
    pub const HELLO: u8 = 1;
    pub const WELCOME: u8 = 2;
    pub const REJECT: u8 = 3;
    pub const VIDEO: u8 = 4;
    pub const PING: u8 = 5;
    pub const PONG: u8 = 6;
    pub const REQUEST_KEYFRAME: u8 = 7;
    pub const PAIR_CHALLENGE: u8 = 8;
    pub const PAIR_REVEAL: u8 = 9;
    pub const TOUCH: u8 = 10;
    pub const POINTER_MOVE: u8 = 11;
    pub const BUTTON: u8 = 12;
    pub const SCROLL: u8 = 13;
    pub const KEY: u8 = 14;
    pub const AUDIO: u8 = 15;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    /// The length prefix is zero or over the limit for this tag.
    BadLength {
        tag: u8,
        length: usize,
    },
    UnknownTag(u8),
    /// The payload ended before a field did.
    Truncated,
    /// Bytes left over after the last field.
    TrailingBytes,
    InvalidUtf8,
    StringTooLong,
    UnknownValue {
        field: &'static str,
        value: u8,
    },
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadLength { tag, length } => {
                write!(f, "message tag {tag} has invalid length {length}")
            }
            Self::UnknownTag(tag) => write!(f, "unknown message tag {tag}"),
            Self::Truncated => write!(f, "message ended early"),
            Self::TrailingBytes => write!(f, "message has trailing bytes"),
            Self::InvalidUtf8 => write!(f, "string is not UTF-8"),
            Self::StringTooLong => write!(f, "string is too long"),
            Self::UnknownValue { field, value } => write!(f, "unknown {field} value {value}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

/// Bytes in the length prefix.
pub const HEADER_LEN: usize = 4;

/// The largest `length` allowed for a tag. Checked before reading the payload.
pub fn max_length(tag: u8) -> Result<usize, ProtocolError> {
    match tag {
        tag::VIDEO => Ok(1 + MAX_VIDEO_FRAME + 9),
        tag::AUDIO => Ok(1 + 8 + MAX_AUDIO_CHUNK),
        tag::HELLO
        | tag::WELCOME
        | tag::REJECT
        | tag::PING
        | tag::PONG
        | tag::REQUEST_KEYFRAME
        | tag::PAIR_CHALLENGE
        | tag::PAIR_REVEAL
        | tag::TOUCH
        | tag::POINTER_MOVE
        | tag::BUTTON
        | tag::SCROLL
        | tag::KEY => Ok(MAX_CONTROL_MESSAGE),
        other => Err(ProtocolError::UnknownTag(other)),
    }
}

/// Validate a message's length prefix against the tag that follows it.
///
/// Readers call this with the 4-byte prefix and the first payload byte (the
/// tag) before reading the rest, so an oversized message is refused before
/// anything is allocated for it.
pub fn check_length(length: u32, tag: u8) -> Result<usize, ProtocolError> {
    let length = length as usize;
    if length == 0 || length > max_length(tag)? {
        return Err(ProtocolError::BadLength { tag, length });
    }
    Ok(length)
}

impl Message {
    /// Encode with its length prefix, ready to write.
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Writer::default();
        match self {
            Message::Hello {
                version,
                pairing,
                device_name,
                display,
            } => {
                body.u8(tag::HELLO);
                body.u16(*version);
                match pairing {
                    PairingRequest::None => body.u8(0),
                    PairingRequest::Token(token) => {
                        body.u8(1);
                        body.bytes(token);
                    }
                    PairingRequest::Confirm { commitment } => {
                        body.u8(2);
                        body.bytes(commitment);
                    }
                }
                body.string(device_name);
                body.u32(display.width);
                body.u32(display.height);
                body.u32(display.density_dpi);
                body.u32(display.refresh_millihz);
            }
            Message::Welcome {
                version,
                codec,
                width,
                height,
            } => {
                body.u8(tag::WELCOME);
                body.u16(*version);
                body.u8(*codec as u8);
                body.u32(*width);
                body.u32(*height);
            }
            Message::Reject { reason, detail } => {
                body.u8(tag::REJECT);
                body.u8(*reason as u8);
                body.string(detail);
            }
            Message::Video {
                pts_us,
                keyframe,
                data,
            } => {
                body.u8(tag::VIDEO);
                body.u64(*pts_us);
                body.u8(u8::from(*keyframe));
                body.bytes(data);
            }
            Message::Ping { id, sent_us } => {
                body.u8(tag::PING);
                body.u32(*id);
                body.u64(*sent_us);
            }
            Message::Pong { id, sent_us } => {
                body.u8(tag::PONG);
                body.u32(*id);
                body.u64(*sent_us);
            }
            Message::RequestKeyframe => body.u8(tag::REQUEST_KEYFRAME),
            Message::PairChallenge { nonce } => {
                body.u8(tag::PAIR_CHALLENGE);
                body.bytes(nonce);
            }
            Message::PairReveal { nonce } => {
                body.u8(tag::PAIR_REVEAL);
                body.bytes(nonce);
            }
            Message::Input(InputEvent::Touch { slot, phase, x, y }) => {
                body.u8(tag::TOUCH);
                body.u8(*slot);
                body.u8(*phase as u8);
                body.f32(*x);
                body.f32(*y);
            }
            Message::Input(InputEvent::PointerMove { x, y }) => {
                body.u8(tag::POINTER_MOVE);
                body.f32(*x);
                body.f32(*y);
            }
            Message::Input(InputEvent::Button { button, pressed }) => {
                body.u8(tag::BUTTON);
                body.u8(*button as u8);
                body.u8(u8::from(*pressed));
            }
            Message::Input(InputEvent::Scroll {
                horizontal,
                vertical,
            }) => {
                body.u8(tag::SCROLL);
                body.u16(*horizontal as u16);
                body.u16(*vertical as u16);
            }
            Message::Input(InputEvent::Key { code, pressed }) => {
                body.u8(tag::KEY);
                body.u16(*code);
                body.u8(u8::from(*pressed));
            }
            Message::Audio { pts_us, pcm } => {
                body.u8(tag::AUDIO);
                body.u64(*pts_us);
                body.bytes(pcm);
            }
        }
        let body = body.0;
        let mut out = Vec::with_capacity(HEADER_LEN + body.len());
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
        out
    }

    /// Decode one message body: the bytes after the length prefix, tag first.
    pub fn decode(body: &[u8]) -> Result<Message, ProtocolError> {
        let mut r = Reader(body);
        let tag = r.u8()?;
        check_length(body.len() as u32, tag)?;
        let message = match tag {
            tag::HELLO => {
                let version = r.u16()?;
                let pairing = match r.u8()? {
                    0 => PairingRequest::None,
                    1 => PairingRequest::Token(r.array()?),
                    2 => PairingRequest::Confirm {
                        commitment: r.array()?,
                    },
                    value => {
                        return Err(ProtocolError::UnknownValue {
                            field: "pairing mode",
                            value,
                        });
                    }
                };
                let device_name = r.string(MAX_NAME)?;
                let display = DisplayInfo {
                    width: r.u32()?,
                    height: r.u32()?,
                    density_dpi: r.u32()?,
                    refresh_millihz: r.u32()?,
                };
                Message::Hello {
                    version,
                    pairing,
                    device_name,
                    display,
                }
            }
            tag::WELCOME => Message::Welcome {
                version: r.u16()?,
                codec: match r.u8()? {
                    1 => Codec::H264,
                    value => {
                        return Err(ProtocolError::UnknownValue {
                            field: "codec",
                            value,
                        });
                    }
                },
                width: r.u32()?,
                height: r.u32()?,
            },
            tag::REJECT => Message::Reject {
                reason: match r.u8()? {
                    1 => RejectReason::UnsupportedVersion,
                    2 => RejectReason::NotPaired,
                    3 => RejectReason::BadPairingToken,
                    4 => RejectReason::Busy,
                    5 => RejectReason::CaptureFailed,
                    6 => RejectReason::PairingDenied,
                    value => {
                        return Err(ProtocolError::UnknownValue {
                            field: "reject reason",
                            value,
                        });
                    }
                },
                detail: r.string(MAX_CONTROL_MESSAGE)?,
            },
            tag::VIDEO => Message::Video {
                pts_us: r.u64()?,
                keyframe: r.bool("keyframe flag")?,
                data: r.rest().to_vec(),
            },
            tag::PING => Message::Ping {
                id: r.u32()?,
                sent_us: r.u64()?,
            },
            tag::PONG => Message::Pong {
                id: r.u32()?,
                sent_us: r.u64()?,
            },
            tag::REQUEST_KEYFRAME => Message::RequestKeyframe,
            tag::PAIR_CHALLENGE => Message::PairChallenge { nonce: r.array()? },
            tag::PAIR_REVEAL => Message::PairReveal { nonce: r.array()? },
            tag::TOUCH => Message::Input(InputEvent::Touch {
                slot: r.u8()?,
                phase: match r.u8()? {
                    0 => TouchPhase::Down,
                    1 => TouchPhase::Move,
                    2 => TouchPhase::Up,
                    3 => TouchPhase::Cancel,
                    value => {
                        return Err(ProtocolError::UnknownValue {
                            field: "touch phase",
                            value,
                        });
                    }
                },
                x: r.f32()?,
                y: r.f32()?,
            }),
            tag::POINTER_MOVE => Message::Input(InputEvent::PointerMove {
                x: r.f32()?,
                y: r.f32()?,
            }),
            tag::BUTTON => Message::Input(InputEvent::Button {
                button: match r.u8()? {
                    1 => MouseButton::Left,
                    2 => MouseButton::Middle,
                    3 => MouseButton::Right,
                    4 => MouseButton::Back,
                    5 => MouseButton::Forward,
                    value => {
                        return Err(ProtocolError::UnknownValue {
                            field: "mouse button",
                            value,
                        });
                    }
                },
                pressed: r.bool("button state")?,
            }),
            tag::SCROLL => Message::Input(InputEvent::Scroll {
                horizontal: r.u16()? as i16,
                vertical: r.u16()? as i16,
            }),
            tag::AUDIO => Message::Audio {
                pts_us: r.u64()?,
                pcm: r.rest().to_vec(),
            },
            tag::KEY => Message::Input(InputEvent::Key {
                code: r.u16()?,
                pressed: r.bool("key state")?,
            }),
            other => return Err(ProtocolError::UnknownTag(other)),
        };
        if !r.0.is_empty() {
            return Err(ProtocolError::TrailingBytes);
        }
        Ok(message)
    }
}

#[derive(Default)]
struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }
    fn bytes(&mut self, v: &[u8]) {
        self.0.extend_from_slice(v);
    }
    /// Callers keep strings within their limits; longer ones are cut at a
    /// character boundary rather than producing a message the peer rejects.
    fn string(&mut self, s: &str) {
        let mut end = s.len().min(u16::MAX as usize);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        self.u16(end as u16);
        self.bytes(&s.as_bytes()[..end]);
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ProtocolError> {
        if self.0.len() < n {
            return Err(ProtocolError::Truncated);
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], ProtocolError> {
        Ok(self.take(N)?.try_into().expect("take returned N bytes"))
    }
    fn u8(&mut self) -> Result<u8, ProtocolError> {
        Ok(self.take(1)?[0])
    }
    fn bool(&mut self, field: &'static str) -> Result<bool, ProtocolError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            value => Err(ProtocolError::UnknownValue { field, value }),
        }
    }
    fn u16(&mut self) -> Result<u16, ProtocolError> {
        Ok(u16::from_be_bytes(self.array()?))
    }
    fn u32(&mut self) -> Result<u32, ProtocolError> {
        Ok(u32::from_be_bytes(self.array()?))
    }
    fn u64(&mut self) -> Result<u64, ProtocolError> {
        Ok(u64::from_be_bytes(self.array()?))
    }
    /// Any bit pattern, NaN included; the input mapper rejects non-finite values.
    fn f32(&mut self) -> Result<f32, ProtocolError> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn string(&mut self, max: usize) -> Result<String, ProtocolError> {
        let len = self.u16()? as usize;
        if len > max {
            return Err(ProtocolError::StringTooLong);
        }
        String::from_utf8(self.take(len)?.to_vec()).map_err(|_| ProtocolError::InvalidUtf8)
    }
    fn rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display() -> DisplayInfo {
        DisplayInfo {
            width: 2560,
            height: 1600,
            density_dpi: 240,
            refresh_millihz: 90_000,
        }
    }

    fn all_messages() -> Vec<Message> {
        vec![
            Message::Hello {
                version: PROTOCOL_VERSION,
                pairing: PairingRequest::Token([7; PAIRING_TOKEN_LEN]),
                device_name: "Idea Tab ✓".into(),
                display: display(),
            },
            Message::Hello {
                version: PROTOCOL_VERSION,
                pairing: PairingRequest::None,
                device_name: String::new(),
                display: display(),
            },
            Message::Welcome {
                version: PROTOCOL_VERSION,
                codec: Codec::H264,
                width: 1280,
                height: 800,
            },
            Message::Reject {
                reason: RejectReason::NotPaired,
                detail: "pair first".into(),
            },
            Message::Video {
                pts_us: u64::MAX,
                keyframe: true,
                data: vec![0, 0, 0, 1, 0x67, 0x42],
            },
            Message::Video {
                pts_us: 0,
                keyframe: false,
                data: Vec::new(),
            },
            Message::Ping {
                id: 9,
                sent_us: 123,
            },
            Message::Pong {
                id: 9,
                sent_us: 123,
            },
            Message::RequestKeyframe,
            Message::Hello {
                version: PROTOCOL_VERSION,
                pairing: PairingRequest::Confirm {
                    commitment: [9; NONCE_LEN],
                },
                device_name: "tab".into(),
                display: display(),
            },
            Message::PairChallenge {
                nonce: [1; NONCE_LEN],
            },
            Message::PairReveal {
                nonce: [2; NONCE_LEN],
            },
            Message::Reject {
                reason: RejectReason::PairingDenied,
                detail: String::new(),
            },
            Message::Input(InputEvent::Touch {
                slot: 3,
                phase: TouchPhase::Move,
                x: 0.25,
                y: 1.0,
            }),
            Message::Input(InputEvent::PointerMove { x: 0.5, y: 0.0 }),
            Message::Input(InputEvent::Button {
                button: MouseButton::Right,
                pressed: true,
            }),
            Message::Input(InputEvent::Scroll {
                horizontal: -2,
                vertical: 3,
            }),
            Message::Input(InputEvent::Key {
                code: 30,
                pressed: false,
            }),
            Message::Audio {
                pts_us: 7,
                pcm: vec![1, 2, 3, 4],
            },
        ]
    }

    #[test]
    fn oversized_audio_is_refused_from_the_header() {
        let max = max_length(15).unwrap() as u32;
        assert!(check_length(max, 15).is_ok());
        assert!(check_length(max + 1, 15).is_err());
    }

    #[test]
    fn known_bytes_for_touch() {
        // Pins the wire format the Kotlin client also implements.
        let encoded = Message::Input(InputEvent::Touch {
            slot: 1,
            phase: TouchPhase::Down,
            x: 0.5,
            y: 1.0,
        })
        .encode();
        assert_eq!(
            encoded,
            [0, 0, 0, 11, 10, 1, 0, 0x3f, 0, 0, 0, 0x3f, 0x80, 0, 0]
        );
    }

    fn split(encoded: &[u8]) -> (u32, &[u8]) {
        let (head, body) = encoded.split_at(HEADER_LEN);
        (u32::from_be_bytes(head.try_into().unwrap()), body)
    }

    #[test]
    fn every_message_round_trips() {
        for message in all_messages() {
            let encoded = message.encode();
            let (length, body) = split(&encoded);
            assert_eq!(length as usize, body.len());
            assert_eq!(check_length(length, body[0]).unwrap(), body.len());
            assert_eq!(Message::decode(body).unwrap(), message);
        }
    }

    #[test]
    fn known_bytes_for_ping() {
        // Pins the wire format the Kotlin client also implements.
        let encoded = Message::Ping { id: 1, sent_us: 2 }.encode();
        assert_eq!(
            encoded,
            [0, 0, 0, 13, 5, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2]
        );
    }

    #[test]
    fn every_truncation_is_an_error_not_a_panic() {
        for message in all_messages() {
            let encoded = message.encode();
            let (_, body) = split(&encoded);
            for cut in 0..body.len() {
                let result = Message::decode(&body[..cut]);
                // Video and audio cut after their header are still valid,
                // just shorter (10 and 9 header bytes).
                let header = match message {
                    Message::Video { .. } => 10,
                    Message::Audio { .. } => 9,
                    _ => usize::MAX,
                };
                if cut < header {
                    assert!(result.is_err(), "{message:?} cut at {cut} decoded");
                }
            }
        }
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut encoded = Message::RequestKeyframe.encode();
        encoded.push(0);
        assert_eq!(
            Message::decode(&encoded[HEADER_LEN..]),
            Err(ProtocolError::TrailingBytes)
        );
    }

    #[test]
    fn oversized_control_messages_are_refused_from_the_header() {
        let tag = 1; // Hello
        assert!(check_length(MAX_CONTROL_MESSAGE as u32, tag).is_ok());
        assert!(matches!(
            check_length(MAX_CONTROL_MESSAGE as u32 + 1, tag),
            Err(ProtocolError::BadLength { .. })
        ));
    }

    #[test]
    fn oversized_video_is_refused_from_the_header() {
        let max = max_length(4).unwrap() as u32;
        assert!(check_length(max, 4).is_ok());
        assert!(check_length(max + 1, 4).is_err());
        assert!(check_length(u32::MAX, 4).is_err());
    }

    #[test]
    fn zero_length_and_unknown_tags_are_refused() {
        assert!(check_length(0, 5).is_err());
        assert_eq!(check_length(10, 99), Err(ProtocolError::UnknownTag(99)));
        assert_eq!(Message::decode(&[99]), Err(ProtocolError::UnknownTag(99)));
        assert_eq!(Message::decode(&[]), Err(ProtocolError::Truncated));
    }

    #[test]
    fn long_device_names_are_refused() {
        let mut body = vec![1, 0, 1, 0];
        body.extend_from_slice(&((MAX_NAME + 1) as u16).to_be_bytes());
        body.extend(std::iter::repeat_n(b'a', MAX_NAME + 1));
        body.extend_from_slice(&[0; 16]);
        assert_eq!(Message::decode(&body), Err(ProtocolError::StringTooLong));
    }

    #[test]
    fn invalid_utf8_is_refused() {
        let mut body = vec![1, 0, 1, 0, 0, 2, 0xff, 0xfe];
        body.extend_from_slice(&[0; 16]);
        assert_eq!(Message::decode(&body), Err(ProtocolError::InvalidUtf8));
    }

    #[test]
    fn unknown_enum_values_are_refused() {
        // Welcome with codec 9.
        let body = [2, 0, 1, 9, 0, 0, 0, 1, 0, 0, 0, 1];
        assert!(matches!(
            Message::decode(&body),
            Err(ProtocolError::UnknownValue { field: "codec", .. })
        ));
        // Video with keyframe flag 2.
        let body = [4, 0, 0, 0, 0, 0, 0, 0, 0, 2];
        assert!(matches!(
            Message::decode(&body),
            Err(ProtocolError::UnknownValue { .. })
        ));
    }

    #[test]
    fn writer_cuts_long_strings_at_a_character_boundary() {
        let name: String = "é".repeat(40_000); // 80 000 bytes
        let encoded = Message::Reject {
            reason: RejectReason::Busy,
            detail: name,
        }
        .encode();
        let len = u16::from_be_bytes([encoded[6], encoded[7]]) as usize;
        assert!(len <= u16::MAX as usize && len.is_multiple_of(2));
    }

    #[test]
    fn arbitrary_bytes_never_panic() {
        // A small deterministic fuzz pass; cargo-fuzz comes later.
        let mut seed: u32 = 0x1234_5678;
        for _ in 0..20_000 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let len = (seed % 64) as usize;
            let bytes: Vec<u8> = (0..len)
                .map(|i| (seed.rotate_left(i as u32) & 0xff) as u8)
                .collect();
            let _ = Message::decode(&bytes);
        }
    }
}
