//! What the server reports while it runs.
//!
//! The command line prints these; the desktop app shows them in its window.

use std::fmt;
use std::sync::Arc;

use tabextend_core::pairing::Fingerprint;

/// Live numbers for the tablet being streamed to, every few seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamStats {
    pub captured_fps: f64,
    pub sent_fps: f64,
    pub mbps: f64,
    pub round_trip_ms: f64,
    pub dropped: u64,
    /// Frames go from GNOME to the encoder without copies.
    pub zero_copy: bool,
}

#[derive(Debug, Clone)]
pub enum ServerEvent {
    /// A tablet was paired (and is now streaming).
    Paired {
        name: String,
        fingerprint: Fingerprint,
    },
    Streaming {
        name: String,
        width: u32,
        height: u32,
        encoder: String,
    },
    Stats(StreamStats),
    Disconnected {
        name: String,
    },
    /// A connection was refused or failed before streaming.
    Refused {
        peer: String,
        reason: String,
    },
    /// Worth telling the user, not an error (a fallback was taken, say).
    Note(String),
}

impl fmt::Display for ServerEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Paired { name, fingerprint } => {
                write!(f, "Paired with {name}, fingerprint {fingerprint}")
            }
            Self::Streaming {
                name,
                width,
                height,
                encoder,
            } => write!(f, "Streaming to {name}: {width}×{height} via {encoder}"),
            Self::Stats(s) => write!(
                f,
                "  captured {:.0} fps · sent {:.0} fps · {:.1} Mbit/s · round trip {:.1} ms · dropped {}{}",
                s.captured_fps,
                s.sent_fps,
                s.mbps,
                s.round_trip_ms,
                s.dropped,
                if s.zero_copy { " · zero-copy" } else { "" }
            ),
            Self::Disconnected { name } => write!(f, "{name} disconnected"),
            Self::Refused { peer, reason } => write!(f, "{peer}: {reason}"),
            Self::Note(note) => write!(f, "  {note}"),
        }
    }
}

pub type Events = Arc<dyn Fn(ServerEvent) + Send + Sync>;

/// Print every event on standard error, as the command line does.
pub fn print_events() -> Events {
    Arc::new(|event| eprintln!("{event}"))
}
