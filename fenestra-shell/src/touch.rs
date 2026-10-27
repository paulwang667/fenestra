//! A touch as input: the finger drives the pointer, and on touch screens
//! (Android, iOS) a drag scrolls and a held finger long-presses.

use fenestra_core::InputEvent;
use kurbo::Point;
use winit::event::TouchPhase;

/// How far a finger may travel (logical px) and still count as a tap.
pub(crate) const TAP_SLOP: f64 = 10.0;

/// How long a finger held still takes to long-press (seconds).
pub(crate) const LONG_PRESS: f64 = 0.5;

/// Whether touches scroll and long-press: on the touch platforms. Elsewhere
/// (desktop touch screens, the web, whose browser has its own gestures) a
/// touch stays only the pointer.
pub(crate) const GESTURES: bool = cfg!(any(target_os = "android", target_os = "ios"));

/// What the driving finger is doing.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    /// Down, within the slop: still a tap or a long press.
    Pending,
    /// Past the slop: its travel scrolls.
    Scrolling,
    /// Held long enough: it right-clicked, and is spent.
    LongPressed,
    /// Down on an element with its own drag (or without gestures): only
    /// the pointer.
    Pointer,
}

#[derive(Clone, Copy, Debug)]
struct Finger {
    id: u64,
    down: Point,
    down_at: f64,
    last: Point,
    mode: Mode,
}

/// The touch that drives the pointer: the first finger to land, until it
/// lifts; others are ignored (the single-pointer model has no multi-touch).
///
/// A finger has no hover, so a lift also leaves; a cancel (the OS took the
/// gesture, as a system swipe does) leaves *before* releasing, so the
/// release lands on nothing and clicks nothing. With gestures on, a finger
/// that travels past [`TAP_SLOP`] scrolls instead (the press let go the same
/// way, then [`InputEvent::Wheel`] deltas at the point it landed), and one
/// held still for [`LONG_PRESS`] right-clicks where it is.
#[derive(Debug)]
pub(crate) struct Touch {
    gestures: bool,
    finger: Option<Finger>,
}

#[expect(clippy::cast_possible_truncation, reason = "positions fit in f32")]
fn pointer_at(p: Point) -> InputEvent {
    InputEvent::PointerMove {
        x: p.x as f32,
        y: p.y as f32,
    }
}

impl Touch {
    /// `gestures` turns on drag-to-scroll and long press; without them a
    /// touch is only the pointer.
    pub(crate) fn new(gestures: bool) -> Self {
        Self {
            gestures,
            finger: None,
        }
    }

    /// One touch event at `at` (logical px) and `now` (seconds on the
    /// runner's clock); `owns_drag` (read on `Started`) says the element
    /// under the finger drags by itself. Returns the events to dispatch, in
    /// order, or `None` for a finger that does not drive the pointer.
    pub(crate) fn event(
        &mut self,
        id: u64,
        phase: TouchPhase,
        at: Point,
        now: f64,
        owns_drag: bool,
    ) -> Option<Vec<InputEvent>> {
        if phase == TouchPhase::Started && self.finger.is_none() {
            let mode = if self.gestures && !owns_drag {
                Mode::Pending
            } else {
                Mode::Pointer
            };
            self.finger = Some(Finger {
                id,
                down: at,
                down_at: now,
                last: at,
                mode,
            });
            return Some(vec![pointer_at(at), InputEvent::PointerDown]);
        }
        if self.finger.is_none_or(|f| f.id != id) {
            return None;
        }
        // A long press that came due before this event happens first.
        let mut events = self.tick(now);
        let finger = self.finger.as_mut().expect("checked above");
        match phase {
            TouchPhase::Started | TouchPhase::Moved => match finger.mode {
                Mode::Pending if finger.down.distance(at) > TAP_SLOP => {
                    finger.mode = Mode::Scrolling;
                    events.extend([
                        InputEvent::PointerLeave,
                        InputEvent::PointerUp,
                        pointer_at(finger.down),
                        wheel(finger.last, at),
                    ]);
                }
                Mode::Pending | Mode::Pointer => events.push(pointer_at(at)),
                Mode::Scrolling => events.push(wheel(finger.last, at)),
                Mode::LongPressed => {}
            },
            TouchPhase::Ended => {
                events.extend(match finger.mode {
                    Mode::Pending | Mode::Pointer => {
                        vec![
                            pointer_at(at),
                            InputEvent::PointerUp,
                            InputEvent::PointerLeave,
                        ]
                    }
                    Mode::Scrolling => vec![InputEvent::PointerLeave],
                    Mode::LongPressed => vec![InputEvent::PointerLeave, InputEvent::PointerUp],
                });
                self.finger = None;
                return Some(events);
            }
            TouchPhase::Cancelled => {
                events.extend(match finger.mode {
                    Mode::Pending | Mode::Pointer => {
                        vec![InputEvent::PointerLeave, InputEvent::PointerUp]
                    }
                    Mode::Scrolling | Mode::LongPressed => vec![InputEvent::PointerLeave],
                });
                self.finger = None;
                return Some(events);
            }
        }
        finger.last = at;
        Some(events)
    }

    /// Fires a long press that has come due by `now` (a right click where
    /// the finger is); nothing otherwise.
    pub(crate) fn tick(&mut self, now: f64) -> Vec<InputEvent> {
        match &mut self.finger {
            Some(f) if f.mode == Mode::Pending && now - f.down_at >= LONG_PRESS => {
                f.mode = Mode::LongPressed;
                vec![InputEvent::RightDown, InputEvent::RightUp]
            }
            _ => Vec::new(),
        }
    }

    /// When a pending long press comes due (seconds), for the event loop to
    /// wake then; `None` when no finger is waiting for one.
    pub(crate) fn deadline(&self) -> Option<f64> {
        self.finger
            .filter(|f| f.mode == Mode::Pending)
            .map(|f| f.down_at + LONG_PRESS)
    }
}

/// The scroll for a finger moving `from` → `to`: its travel, so content
/// follows the finger (the core scrolls by the negated delta).
#[expect(clippy::cast_possible_truncation, reason = "deltas fit in f32")]
fn wheel(from: Point, to: Point) -> InputEvent {
    InputEvent::Wheel {
        dx: (to.x - from.x) as f32,
        dy: (to.y - from.y) as f32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenestra_core::InputEvent;
    use kurbo::Point;
    use winit::event::TouchPhase::{Cancelled, Ended, Moved, Started};

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    #[expect(clippy::cast_possible_truncation, reason = "test positions are small")]
    fn mv(x: f64, y: f64) -> InputEvent {
        InputEvent::PointerMove {
            x: x as f32,
            y: y as f32,
        }
    }

    /// A finger that moves a little (within the slop) still taps.
    #[test]
    fn a_tap_that_wiggles_within_the_slop_still_clicks() {
        let mut t = Touch::new(true);
        assert_eq!(
            t.event(0, Started, p(100., 100.), 0.0, false).unwrap(),
            vec![mv(100., 100.), InputEvent::PointerDown]
        );
        assert_eq!(
            t.event(0, Moved, p(106., 104.), 0.1, false).unwrap(),
            vec![mv(106., 104.)]
        );
        assert_eq!(
            t.event(0, Ended, p(106., 104.), 0.2, false).unwrap(),
            vec![
                mv(106., 104.),
                InputEvent::PointerUp,
                InputEvent::PointerLeave
            ]
        );
        assert_eq!(t.deadline(), None);
    }

    /// Past the slop the touch scrolls: the press is let go without a click
    /// (the pointer leaves first), and the finger's travel becomes wheel
    /// deltas at the point it landed.
    #[test]
    fn a_drag_that_starts_on_a_button_scrolls_without_clicking() {
        let mut t = Touch::new(true);
        t.event(0, Started, p(100., 300.), 0.0, false);
        assert_eq!(
            t.event(0, Moved, p(100., 280.), 0.05, false).unwrap(),
            vec![
                InputEvent::PointerLeave,
                InputEvent::PointerUp,
                mv(100., 300.),
                InputEvent::Wheel { dx: 0.0, dy: -20.0 }
            ]
        );
        assert_eq!(
            t.event(0, Moved, p(100., 250.), 0.1, false).unwrap(),
            vec![InputEvent::Wheel { dx: 0.0, dy: -30.0 }]
        );
        // No long press once scrolling, however long the finger stays.
        assert!(t.tick(2.0).is_empty());
        assert_eq!(
            t.event(0, Ended, p(100., 250.), 2.1, false).unwrap(),
            vec![InputEvent::PointerLeave]
        );
    }

    /// Held still for half a second, the finger right-clicks where it is,
    /// once.
    #[test]
    fn a_held_finger_long_presses_once_at_half_a_second() {
        let mut t = Touch::new(true);
        t.event(0, Started, p(50., 60.), 1.0, false);
        assert_eq!(t.deadline(), Some(1.5));
        assert!(t.tick(1.49).is_empty());
        assert_eq!(
            t.tick(1.5),
            vec![InputEvent::RightDown, InputEvent::RightUp]
        );
        assert!(t.tick(1.6).is_empty());
        assert_eq!(t.deadline(), None);
    }

    /// After a long press the finger is spent: moving does nothing, and the
    /// lift leaves before it releases, so it clicks nothing.
    #[test]
    fn after_a_long_press_the_lift_clicks_nothing() {
        let mut t = Touch::new(true);
        t.event(0, Started, p(50., 60.), 0.0, false);
        t.tick(0.6);
        assert!(
            t.event(0, Moved, p(80., 90.), 0.7, false)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            t.event(0, Ended, p(80., 90.), 0.8, false).unwrap(),
            vec![InputEvent::PointerLeave, InputEvent::PointerUp]
        );
    }

    /// A slider, splitter or text field under the finger keeps its own drag.
    #[test]
    fn a_drag_on_an_element_that_owns_it_is_not_a_scroll() {
        let mut t = Touch::new(true);
        t.event(0, Started, p(10., 10.), 0.0, true);
        assert_eq!(t.deadline(), None);
        assert_eq!(
            t.event(0, Moved, p(60., 10.), 0.1, false).unwrap(),
            vec![mv(60., 10.)]
        );
        assert!(t.tick(1.0).is_empty());
    }

    /// One finger drives; a cancel (the OS took the gesture) ends it cleanly.
    #[test]
    fn a_second_finger_is_ignored_and_a_cancel_resets() {
        let mut t = Touch::new(true);
        t.event(0, Started, p(10., 10.), 0.0, false);
        t.event(0, Moved, p(10., 40.), 0.1, false); // scrolling
        assert_eq!(t.event(1, Started, p(200., 200.), 0.2, false), None);
        assert_eq!(
            t.event(0, Cancelled, p(10., 40.), 0.3, false).unwrap(),
            vec![InputEvent::PointerLeave]
        );
        // The next touch starts fresh.
        assert_eq!(
            t.event(1, Started, p(5., 5.), 0.4, false).unwrap(),
            vec![mv(5., 5.), InputEvent::PointerDown]
        );
    }

    /// Without gestures (desktop touch screens, the web) a touch is only the
    /// pointer: a drag moves it, holding does nothing.
    #[test]
    fn without_gestures_a_touch_is_only_the_pointer() {
        let mut t = Touch::new(false);
        t.event(0, Started, p(10., 10.), 0.0, false);
        assert_eq!(t.deadline(), None);
        assert_eq!(
            t.event(0, Moved, p(10., 80.), 0.1, false).unwrap(),
            vec![mv(10., 80.)]
        );
        assert!(t.tick(5.0).is_empty());
        assert_eq!(
            t.event(0, Ended, p(10., 80.), 5.1, false).unwrap(),
            vec![
                mv(10., 80.),
                InputEvent::PointerUp,
                InputEvent::PointerLeave
            ]
        );
    }

    /// A touch cancelled while still undecided releases without clicking.
    #[test]
    fn a_cancelled_touch_releases_without_clicking() {
        for gestures in [false, true] {
            let mut t = Touch::new(gestures);
            t.event(0, Started, p(10., 10.), 0.0, false);
            assert_eq!(
                t.event(0, Cancelled, p(10., 10.), 0.1, false).unwrap(),
                vec![InputEvent::PointerLeave, InputEvent::PointerUp]
            );
            assert_eq!(t.deadline(), None);
        }
    }
}
