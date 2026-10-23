//! Stream sizing and flow control.

use crate::protocol::DisplayInfo;

/// Smallest stream side; GNOME refuses tiny monitors and encoders need some room.
const MIN_SIDE: u32 = 320;

/// Pick the virtual monitor size for a tablet.
///
/// Starts from the tablet's own resolution so text is sharp, then scales
/// down to `max_width` if the host asked for that (to save bandwidth or
/// decoder load). Keeps the aspect ratio and rounds to even numbers, which
/// H.264 with 4:2:0 chroma needs.
pub fn stream_size(display: &DisplayInfo, max_width: Option<u32>) -> Option<(u32, u32)> {
    let (w, h) = (display.width, display.height);
    if w < MIN_SIDE || h < MIN_SIDE {
        return None;
    }
    let scale = match max_width {
        Some(max) if max < w => max as f64 / w as f64,
        _ => 1.0,
    };
    let even = |v: f64| ((v as u32) & !1).max(MIN_SIDE & !1);
    Some((even(w as f64 * scale), even(h as f64 * scale)))
}

/// A starting bitrate in bits per second for a stream size and frame rate.
///
/// About 0.1 bits per pixel per frame: enough for sharp text with H.264 on
/// mostly static desktop content, bounded so Wi-Fi is not saturated.
pub fn default_bitrate(width: u32, height: u32, fps: u32) -> u32 {
    let bits = width as u64 * height as u64 * fps as u64 / 10;
    bits.clamp(2_000_000, 40_000_000) as u32
}

/// What to do with a newly encoded frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDecision {
    Send,
    /// Skip it; the next frame should also be skipped unless it is a keyframe.
    Drop,
    /// Skip it and ask the encoder for a keyframe so the stream can recover.
    DropAndRequestKeyframe,
}

/// Decides which frames to send when the network cannot keep up.
///
/// Sending old frames only adds latency, so when the send queue is full the
/// frame is dropped. But H.264 delta frames depend on the frames before
/// them: after one drop, every following delta frame would decode as garbage.
/// So after a drop the gate keeps dropping until a keyframe, and asks the
/// encoder for one exactly once.
#[derive(Debug, Default)]
pub struct FrameGate {
    awaiting_keyframe: bool,
    keyframe_requested: bool,
}

impl FrameGate {
    /// A new gate waits for the first keyframe: a decoder cannot start on a
    /// delta frame.
    pub fn new() -> Self {
        Self {
            awaiting_keyframe: true,
            keyframe_requested: false,
        }
    }

    pub fn decide(&mut self, keyframe: bool, queue_has_room: bool) -> FrameDecision {
        if keyframe && queue_has_room {
            self.awaiting_keyframe = false;
            self.keyframe_requested = false;
            return FrameDecision::Send;
        }
        if !self.awaiting_keyframe && queue_has_room {
            return FrameDecision::Send;
        }
        self.awaiting_keyframe = true;
        if self.keyframe_requested {
            FrameDecision::Drop
        } else {
            self.keyframe_requested = true;
            FrameDecision::DropAndRequestKeyframe
        }
    }

    /// The client asked for a keyframe (its decoder lost its place).
    pub fn reset(&mut self) {
        self.awaiting_keyframe = true;
        self.keyframe_requested = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(width: u32, height: u32) -> DisplayInfo {
        DisplayInfo {
            width,
            height,
            density_dpi: 240,
            refresh_millihz: 60_000,
        }
    }

    #[test]
    fn native_size_by_default() {
        assert_eq!(stream_size(&display(2560, 1600), None), Some((2560, 1600)));
    }

    #[test]
    fn scales_down_keeping_aspect_ratio() {
        assert_eq!(
            stream_size(&display(2560, 1600), Some(1280)),
            Some((1280, 800))
        );
    }

    #[test]
    fn never_scales_up() {
        assert_eq!(
            stream_size(&display(1280, 800), Some(4000)),
            Some((1280, 800))
        );
    }

    #[test]
    fn rounds_to_even() {
        let (w, h) = stream_size(&display(1281, 801), None).unwrap();
        assert_eq!((w % 2, h % 2), (0, 0));
        let (w, h) = stream_size(&display(2560, 1600), Some(1001)).unwrap();
        assert_eq!((w % 2, h % 2), (0, 0));
    }

    #[test]
    fn portrait_tablets_work() {
        assert_eq!(stream_size(&display(1600, 2560), None), Some((1600, 2560)));
    }

    #[test]
    fn absurd_displays_are_refused() {
        assert_eq!(stream_size(&display(0, 0), None), None);
        assert_eq!(stream_size(&display(100, 2000), None), None);
    }

    #[test]
    fn bitrate_is_bounded() {
        assert_eq!(default_bitrate(320, 320, 30), 2_000_000);
        assert_eq!(default_bitrate(7680, 4320, 120), 40_000_000);
        let b = default_bitrate(2560, 1600, 60);
        assert!((2_000_000..=40_000_000).contains(&b));
    }

    #[test]
    fn waits_for_the_first_keyframe() {
        let mut gate = FrameGate::new();
        assert_eq!(
            gate.decide(false, true),
            FrameDecision::DropAndRequestKeyframe
        );
        assert_eq!(gate.decide(false, true), FrameDecision::Drop);
        assert_eq!(gate.decide(true, true), FrameDecision::Send);
        assert_eq!(gate.decide(false, true), FrameDecision::Send);
    }

    #[test]
    fn congestion_drops_until_the_next_keyframe() {
        let mut gate = FrameGate::new();
        assert_eq!(gate.decide(true, true), FrameDecision::Send);
        assert_eq!(
            gate.decide(false, false),
            FrameDecision::DropAndRequestKeyframe
        );
        // Room again, but a delta frame now would reference a dropped frame.
        assert_eq!(gate.decide(false, true), FrameDecision::Drop);
        assert_eq!(gate.decide(true, true), FrameDecision::Send);
        assert_eq!(gate.decide(false, true), FrameDecision::Send);
    }

    #[test]
    fn keyframe_is_requested_once_per_outage() {
        let mut gate = FrameGate::new();
        gate.decide(true, true);
        let decisions: Vec<_> = (0..5).map(|_| gate.decide(false, false)).collect();
        assert_eq!(
            decisions
                .iter()
                .filter(|d| **d == FrameDecision::DropAndRequestKeyframe)
                .count(),
            1
        );
    }

    #[test]
    fn keyframe_with_full_queue_is_dropped_too() {
        let mut gate = FrameGate::new();
        gate.decide(true, true);
        assert_eq!(
            gate.decide(true, false),
            FrameDecision::DropAndRequestKeyframe
        );
        assert_eq!(gate.decide(true, true), FrameDecision::Send);
    }

    #[test]
    fn client_reset_waits_for_a_keyframe() {
        let mut gate = FrameGate::new();
        gate.decide(true, true);
        gate.reset();
        assert_eq!(
            gate.decide(false, true),
            FrameDecision::DropAndRequestKeyframe
        );
        assert_eq!(gate.decide(true, true), FrameDecision::Send);
    }
}
