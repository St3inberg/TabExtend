//! Turning the tablet's input into remote-desktop calls.
//!
//! The tablet is untrusted, so this is where its input is made safe: every
//! position is checked and clipped to the virtual monitor (input can never
//! land on the laptop's own screen), finger and key codes are limited to
//! real ones, and duplicate presses or releases are ignored. The mapper
//! remembers what is held down, so a disconnect can release everything
//! instead of leaving a key or button stuck on the laptop.

use std::collections::BTreeSet;

use crate::protocol::{InputEvent, MouseButton, TouchPhase};

/// Most fingers tracked at once.
pub const MAX_TOUCH_SLOTS: u8 = 10;
/// Highest Linux evdev key code (`KEY_MAX`).
const KEY_MAX: u16 = 0x2ff;
/// Largest scroll accepted in one message, in wheel steps.
const MAX_SCROLL_STEPS: i16 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollAxis {
    Vertical,
    Horizontal,
}

/// One call on the remote-desktop session. Positions are in the stream's
/// logical pixels; buttons and keys are Linux evdev codes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputCall {
    TouchDown { slot: u32, x: f64, y: f64 },
    TouchMotion { slot: u32, x: f64, y: f64 },
    TouchUp { slot: u32 },
    PointerMotion { x: f64, y: f64 },
    PointerButton { code: i32, pressed: bool },
    Scroll { axis: ScrollAxis, steps: i32 },
    Key { code: i32, pressed: bool },
}

/// evdev button codes (`linux/input-event-codes.h`).
fn button_code(button: MouseButton) -> u16 {
    match button {
        MouseButton::Left => 0x110,
        MouseButton::Right => 0x111,
        MouseButton::Middle => 0x112,
        MouseButton::Back => 0x113,
        MouseButton::Forward => 0x114,
    }
}

impl InputCall {
    pub fn is_touch(&self) -> bool {
        matches!(
            self,
            Self::TouchDown { .. } | Self::TouchMotion { .. } | Self::TouchUp { .. }
        )
    }
}

/// Where the desktop's picture sits inside the stream, as fractions of the
/// stream. Usually all of it; when a desktop makes its screen a different
/// shape than the tablet (KDE always uses 1920×1080), the picture is fitted
/// with black bars and taps on the bars are clipped to the edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContentArea {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl ContentArea {
    pub const FULL: Self = Self {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    };

    /// The screen scaled to fit the stream, centred, aspect ratio kept:
    /// what `videoscale add-borders=true` draws.
    pub fn letterbox(stream: (f64, f64), screen: (f64, f64)) -> Self {
        if stream.0 <= 0.0 || stream.1 <= 0.0 || screen.0 <= 0.0 || screen.1 <= 0.0 {
            return Self::FULL;
        }
        let scale = (stream.0 / screen.0).min(stream.1 / screen.1);
        let width = screen.0 * scale / stream.0;
        let height = screen.1 * scale / stream.1;
        Self {
            x: (1.0 - width) / 2.0,
            y: (1.0 - height) / 2.0,
            width,
            height,
        }
    }
}

pub struct InputMapper {
    width: f64,
    height: f64,
    area: ContentArea,
    touches: BTreeSet<u8>,
    /// When the desktop refuses touch input, the first finger drives the
    /// pointer instead: down presses the left button, up releases it.
    emulating_touch: bool,
    /// The finger driving the pointer while emulating.
    primary: Option<u8>,
    buttons: BTreeSet<u16>,
    keys: BTreeSet<u16>,
}

impl InputMapper {
    /// `width` × `height` is the stream's size in logical pixels.
    pub fn new(width: f64, height: f64) -> Self {
        Self {
            width,
            height,
            area: ContentArea::FULL,
            touches: BTreeSet::new(),
            emulating_touch: false,
            primary: None,
            buttons: BTreeSet::new(),
            keys: BTreeSet::new(),
        }
    }

    /// Positions arrive as fractions of the stream; `area` says where the
    /// desktop's picture is inside it.
    pub fn with_content_area(mut self, area: ContentArea) -> Self {
        self.area = area;
        self
    }

    /// A fraction of the stream to a pixel on the desktop's screen, or None
    /// for NaN and infinities. Clipped to the last pixel so the position
    /// stays on the virtual monitor.
    fn position(&self, x: f32, y: f32) -> Option<(f64, f64)> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let clip = |v: f32, start: f64, extent: f64, size: f64| {
            ((f64::from(v) - start) / extent).clamp(0.0, 1.0) * size
        };
        let clip_last = |v: f64, size: f64| v.min(size - 1.0).max(0.0);
        Some((
            clip_last(
                clip(x, self.area.x, self.area.width, self.width),
                self.width,
            ),
            clip_last(
                clip(y, self.area.y, self.area.height, self.height),
                self.height,
            ),
        ))
    }

    pub fn map(&mut self, event: InputEvent) -> Vec<InputCall> {
        match event {
            InputEvent::Touch { slot, phase, x, y } => self.touch(slot, phase, x, y),
            InputEvent::PointerMove { x, y } => self
                .position(x, y)
                .map(|(x, y)| InputCall::PointerMotion { x, y })
                .into_iter()
                .collect(),
            InputEvent::Button { button, pressed } => {
                let code = button_code(button);
                if toggle(&mut self.buttons, code, pressed) {
                    vec![InputCall::PointerButton {
                        code: code.into(),
                        pressed,
                    }]
                } else {
                    Vec::new()
                }
            }
            InputEvent::Scroll {
                horizontal,
                vertical,
            } => {
                let clamp = |s: i16| i32::from(s.clamp(-MAX_SCROLL_STEPS, MAX_SCROLL_STEPS));
                let mut calls = Vec::new();
                if vertical != 0 {
                    calls.push(InputCall::Scroll {
                        axis: ScrollAxis::Vertical,
                        steps: clamp(vertical),
                    });
                }
                if horizontal != 0 {
                    calls.push(InputCall::Scroll {
                        axis: ScrollAxis::Horizontal,
                        steps: clamp(horizontal),
                    });
                }
                calls
            }
            InputEvent::Key { code, pressed } => {
                if (1..=KEY_MAX).contains(&code) && toggle(&mut self.keys, code, pressed) {
                    vec![InputCall::Key {
                        code: code.into(),
                        pressed,
                    }]
                } else {
                    Vec::new()
                }
            }
        }
    }

    /// Switch to driving the pointer with the first finger, for desktops
    /// that refuse touch input. Fingers that were down are forgotten (the
    /// desktop never accepted them).
    pub fn emulate_touch_with_pointer(&mut self) {
        self.emulating_touch = true;
        self.touches.clear();
    }

    pub fn is_emulating_touch(&self) -> bool {
        self.emulating_touch
    }

    fn emulated_touch(&mut self, slot: u8, phase: TouchPhase, x: f32, y: f32) -> Vec<InputCall> {
        let left = button_code(MouseButton::Left);
        match phase {
            TouchPhase::Down | TouchPhase::Move => {
                if self.primary.is_some_and(|p| p != slot) {
                    return Vec::new(); // only one finger can drive a pointer
                }
                let Some((x, y)) = self.position(x, y) else {
                    return Vec::new();
                };
                let mut calls = vec![InputCall::PointerMotion { x, y }];
                if self.primary.is_none() {
                    if phase == TouchPhase::Move {
                        return Vec::new(); // a move without a down
                    }
                    self.primary = Some(slot);
                    if toggle(&mut self.buttons, left, true) {
                        calls.push(InputCall::PointerButton {
                            code: left.into(),
                            pressed: true,
                        });
                    }
                }
                calls
            }
            TouchPhase::Up | TouchPhase::Cancel if self.primary == Some(slot) => {
                self.primary = None;
                if toggle(&mut self.buttons, left, false) {
                    vec![InputCall::PointerButton {
                        code: left.into(),
                        pressed: false,
                    }]
                } else {
                    Vec::new()
                }
            }
            TouchPhase::Up | TouchPhase::Cancel => Vec::new(),
        }
    }

    fn touch(&mut self, slot: u8, phase: TouchPhase, x: f32, y: f32) -> Vec<InputCall> {
        if slot >= MAX_TOUCH_SLOTS {
            return Vec::new();
        }
        if self.emulating_touch {
            return self.emulated_touch(slot, phase, x, y);
        }
        let down = self.touches.contains(&slot);
        let call = match phase {
            TouchPhase::Down | TouchPhase::Move => {
                let Some((x, y)) = self.position(x, y) else {
                    return Vec::new();
                };
                let slot32 = u32::from(slot);
                match (phase, down) {
                    // A repeated down is a move; a move without a down is ignored.
                    (TouchPhase::Down, false) => {
                        self.touches.insert(slot);
                        InputCall::TouchDown { slot: slot32, x, y }
                    }
                    (_, true) => InputCall::TouchMotion { slot: slot32, x, y },
                    (TouchPhase::Move, false) => return Vec::new(),
                    _ => unreachable!("phase is Down or Move"),
                }
            }
            TouchPhase::Up | TouchPhase::Cancel if down => {
                self.touches.remove(&slot);
                InputCall::TouchUp {
                    slot: u32::from(slot),
                }
            }
            TouchPhase::Up | TouchPhase::Cancel => return Vec::new(),
        };
        vec![call]
    }

    /// Release every finger, button and key still held, e.g. when the
    /// tablet disconnects mid-drag.
    pub fn release_all(&mut self) -> Vec<InputCall> {
        self.primary = None;
        let touches =
            std::mem::take(&mut self.touches)
                .into_iter()
                .map(|slot| InputCall::TouchUp {
                    slot: u32::from(slot),
                });
        let buttons =
            std::mem::take(&mut self.buttons)
                .into_iter()
                .map(|code| InputCall::PointerButton {
                    code: code.into(),
                    pressed: false,
                });
        let keys = std::mem::take(&mut self.keys)
            .into_iter()
            .map(|code| InputCall::Key {
                code: code.into(),
                pressed: false,
            });
        touches.chain(buttons).chain(keys).collect()
    }
}

/// Record a press or release; false if it changes nothing (a duplicate).
fn toggle(held: &mut BTreeSet<u16>, code: u16, pressed: bool) -> bool {
    if pressed {
        held.insert(code)
    } else {
        held.remove(&code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapper() -> InputMapper {
        InputMapper::new(1808.0, 1130.0)
    }

    fn touch(slot: u8, phase: TouchPhase, x: f32, y: f32) -> InputEvent {
        InputEvent::Touch { slot, phase, x, y }
    }

    #[test]
    fn touch_down_move_up() {
        let mut m = mapper();
        assert_eq!(
            m.map(touch(0, TouchPhase::Down, 0.5, 0.5)),
            [InputCall::TouchDown {
                slot: 0,
                x: 904.0,
                y: 565.0
            }]
        );
        assert_eq!(
            m.map(touch(0, TouchPhase::Move, 0.25, 0.0)),
            [InputCall::TouchMotion {
                slot: 0,
                x: 452.0,
                y: 0.0
            }]
        );
        assert_eq!(
            m.map(touch(0, TouchPhase::Up, 0.0, 0.0)),
            [InputCall::TouchUp { slot: 0 }]
        );
    }

    #[test]
    fn letterbox_fits_a_wide_screen_into_a_taller_stream() {
        // KDE's 1920×1080 screen on a 16:10 tablet stream: bars above and
        // below, none at the sides.
        let area = ContentArea::letterbox((1808.0, 1130.0), (1920.0, 1080.0));
        assert!((area.x).abs() < 1e-9 && (area.width - 1.0).abs() < 1e-9);
        let bar = (1130.0 - 1808.0 * 1080.0 / 1920.0) / 2.0 / 1130.0;
        assert!((area.y - bar).abs() < 1e-9, "{area:?}");
        assert!((area.y * 2.0 + area.height - 1.0).abs() < 1e-9);
        // Same shape: the whole stream.
        assert_eq!(
            ContentArea::letterbox((1808.0, 1130.0), (1808.0, 1130.0)),
            ContentArea::FULL
        );
        assert_eq!(
            ContentArea::letterbox((0.0, 0.0), (1.0, 1.0)),
            ContentArea::FULL
        );
    }

    #[test]
    fn taps_map_through_the_bars_onto_the_screen() {
        let area = ContentArea::letterbox((1808.0, 1130.0), (1920.0, 1080.0));
        let mut m = InputMapper::new(1920.0, 1080.0).with_content_area(area);
        // The middle of the tablet is the middle of the screen.
        assert_eq!(
            m.map(InputEvent::PointerMove { x: 0.5, y: 0.5 }),
            [InputCall::PointerMotion { x: 960.0, y: 540.0 }]
        );
        // The top of the picture (just below the bar) is the screen's top;
        // a tap on the bar itself is clipped to that edge.
        for y in [area.y as f32, 0.0] {
            let calls = m.map(InputEvent::PointerMove { x: 0.0, y });
            let [InputCall::PointerMotion { y, .. }] = calls[..] else {
                panic!("{calls:?}")
            };
            assert!(y.abs() < 0.01, "{y}");
        }
    }

    #[test]
    fn positions_are_clipped_to_the_virtual_monitor() {
        let mut m = mapper();
        let calls = m.map(touch(1, TouchPhase::Down, 7.0, -3.0));
        assert_eq!(
            calls,
            [InputCall::TouchDown {
                slot: 1,
                x: 1807.0,
                y: 0.0
            }]
        );
        let calls = m.map(InputEvent::PointerMove { x: 1.0, y: 1.0 });
        assert_eq!(
            calls,
            [InputCall::PointerMotion {
                x: 1807.0,
                y: 1129.0
            }]
        );
    }

    #[test]
    fn non_finite_positions_are_dropped() {
        let mut m = mapper();
        for (x, y) in [
            (f32::NAN, 0.5),
            (0.5, f32::INFINITY),
            (f32::NEG_INFINITY, 0.0),
        ] {
            assert!(m.map(touch(0, TouchPhase::Down, x, y)).is_empty());
            assert!(m.map(InputEvent::PointerMove { x, y }).is_empty());
        }
        assert!(m.release_all().is_empty(), "nothing was pressed");
    }

    #[test]
    fn slots_beyond_the_limit_are_ignored() {
        let mut m = mapper();
        assert!(
            m.map(touch(MAX_TOUCH_SLOTS, TouchPhase::Down, 0.5, 0.5))
                .is_empty()
        );
        assert!(m.map(touch(255, TouchPhase::Down, 0.5, 0.5)).is_empty());
        assert_eq!(m.map(touch(9, TouchPhase::Down, 0.5, 0.5)).len(), 1);
    }

    #[test]
    fn inconsistent_touch_sequences_are_repaired() {
        let mut m = mapper();
        // Move and up for a finger that is not down: ignored.
        assert!(m.map(touch(2, TouchPhase::Move, 0.5, 0.5)).is_empty());
        assert!(m.map(touch(2, TouchPhase::Up, 0.5, 0.5)).is_empty());
        // A second down becomes a move instead of a second press.
        m.map(touch(2, TouchPhase::Down, 0.5, 0.5));
        assert!(matches!(
            m.map(touch(2, TouchPhase::Down, 0.6, 0.5))[..],
            [InputCall::TouchMotion { slot: 2, .. }]
        ));
        // Cancel ends the touch like up.
        assert_eq!(
            m.map(touch(2, TouchPhase::Cancel, 0.0, 0.0)),
            [InputCall::TouchUp { slot: 2 }]
        );
    }

    #[test]
    fn buttons_map_to_evdev_codes_and_ignore_duplicates() {
        let mut m = mapper();
        let press = |button| InputEvent::Button {
            button,
            pressed: true,
        };
        assert_eq!(
            m.map(press(MouseButton::Left)),
            [InputCall::PointerButton {
                code: 0x110,
                pressed: true
            }]
        );
        assert!(m.map(press(MouseButton::Left)).is_empty());
        assert_eq!(
            m.map(press(MouseButton::Right)),
            [InputCall::PointerButton {
                code: 0x111,
                pressed: true
            }]
        );
        assert!(
            m.map(InputEvent::Button {
                button: MouseButton::Middle,
                pressed: false
            })
            .is_empty()
        );
    }

    #[test]
    fn keys_must_be_real_evdev_codes() {
        let mut m = mapper();
        assert!(
            m.map(InputEvent::Key {
                code: 0,
                pressed: true
            })
            .is_empty()
        );
        assert!(
            m.map(InputEvent::Key {
                code: 0x300,
                pressed: true
            })
            .is_empty()
        );
        assert_eq!(
            m.map(InputEvent::Key {
                code: 30,
                pressed: true
            }),
            [InputCall::Key {
                code: 30,
                pressed: true
            }]
        );
        assert!(
            m.map(InputEvent::Key {
                code: 30,
                pressed: true
            })
            .is_empty()
        );
        assert_eq!(
            m.map(InputEvent::Key {
                code: 30,
                pressed: false
            }),
            [InputCall::Key {
                code: 30,
                pressed: false
            }]
        );
    }

    #[test]
    fn scroll_is_split_by_axis_and_bounded() {
        let mut m = mapper();
        assert_eq!(
            m.map(InputEvent::Scroll {
                horizontal: -1,
                vertical: 30_000
            }),
            [
                InputCall::Scroll {
                    axis: ScrollAxis::Vertical,
                    steps: 10
                },
                InputCall::Scroll {
                    axis: ScrollAxis::Horizontal,
                    steps: -1
                },
            ]
        );
        assert!(
            m.map(InputEvent::Scroll {
                horizontal: 0,
                vertical: 0
            })
            .is_empty()
        );
    }

    #[test]
    fn emulation_drives_the_pointer_with_the_first_finger() {
        let mut m = mapper();
        m.emulate_touch_with_pointer();
        assert_eq!(
            m.map(touch(3, TouchPhase::Down, 0.5, 0.5)),
            [
                InputCall::PointerMotion { x: 904.0, y: 565.0 },
                InputCall::PointerButton {
                    code: 0x110,
                    pressed: true
                },
            ]
        );
        // A second finger is ignored, the first keeps dragging.
        assert!(m.map(touch(4, TouchPhase::Down, 0.1, 0.1)).is_empty());
        assert_eq!(
            m.map(touch(3, TouchPhase::Move, 0.0, 0.0)),
            [InputCall::PointerMotion { x: 0.0, y: 0.0 }]
        );
        assert!(m.map(touch(4, TouchPhase::Up, 0.1, 0.1)).is_empty());
        assert_eq!(
            m.map(touch(3, TouchPhase::Up, 0.0, 0.0)),
            [InputCall::PointerButton {
                code: 0x110,
                pressed: false
            }]
        );
        // The next finger can take over.
        assert_eq!(m.map(touch(4, TouchPhase::Down, 0.1, 0.1)).len(), 2);
    }

    #[test]
    fn switching_to_emulation_forgets_refused_touches() {
        let mut m = mapper();
        m.map(touch(0, TouchPhase::Down, 0.5, 0.5));
        m.emulate_touch_with_pointer();
        assert!(m.is_emulating_touch());
        // The refused down is replayed as a pointer press.
        assert_eq!(m.map(touch(0, TouchPhase::Down, 0.5, 0.5)).len(), 2);
        // Disconnect releases the emulated button, not a touch.
        assert_eq!(
            m.release_all(),
            [InputCall::PointerButton {
                code: 0x110,
                pressed: false
            }]
        );
    }

    #[test]
    fn disconnect_releases_everything_held() {
        let mut m = mapper();
        m.map(touch(0, TouchPhase::Down, 0.1, 0.1));
        m.map(touch(4, TouchPhase::Down, 0.2, 0.2));
        m.map(InputEvent::Button {
            button: MouseButton::Left,
            pressed: true,
        });
        m.map(InputEvent::Key {
            code: 42,
            pressed: true,
        }); // shift
        let released = m.release_all();
        assert_eq!(
            released,
            [
                InputCall::TouchUp { slot: 0 },
                InputCall::TouchUp { slot: 4 },
                InputCall::PointerButton {
                    code: 0x110,
                    pressed: false
                },
                InputCall::Key {
                    code: 42,
                    pressed: false
                },
            ]
        );
        assert!(m.release_all().is_empty());
    }
}
