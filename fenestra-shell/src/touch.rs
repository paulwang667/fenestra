//! A touch as input: the finger drives the pointer, and on touch screens
//! (Android, iOS) a drag scrolls and a held finger long-presses.

use fenestra_core::InputEvent;
use kurbo::Point;
use winit::event::TouchPhase;

/// How far a finger may travel (logical px) and still count as a tap.
pub(crate) const TAP_SLOP: f64 = 10.0;

/// How long a finger held still takes to long-press (milliseconds).
pub(crate) const LONG_PRESS_MS: u32 = 500;

/// [`LONG_PRESS_MS`] in seconds.
pub(crate) const LONG_PRESS: f64 = LONG_PRESS_MS as f64 / 1000.0;

/// The stretch of travel a lift's velocity is measured over (seconds).
const VELOCITY_WINDOW: f64 = 0.1;

/// A finger last moved longer ago than this (seconds) lifts still.
const STILL_AFTER: f64 = 0.05;

/// The fastest lift (logical px/s).
const MAX_VELOCITY: f64 = 8000.0;

/// How many recent positions a finger keeps for its velocity.
const SAMPLES: usize = 8;

/// A drag this many times longer on one axis than the other, when it
/// leaves the tap slop, keeps to that axis until the lift.
const AXIS_LOCK: f64 = 2.0;

/// Which axes a scrolling finger moves.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Lock {
    Free,
    Vertical,
    Horizontal,
}

impl Lock {
    /// The lock for a drag that has travelled `travel` past the slop.
    fn of(travel: kurbo::Vec2) -> Self {
        let (x, y) = (travel.x.abs(), travel.y.abs());
        if y > AXIS_LOCK * x {
            Self::Vertical
        } else if x > AXIS_LOCK * y {
            Self::Horizontal
        } else {
            Self::Free
        }
    }

    /// `(x, y)` with the locked-out axis zeroed.
    fn keep(self, x: f64, y: f64) -> (f64, f64) {
        match self {
            Self::Free => (x, y),
            Self::Vertical => (0.0, y),
            Self::Horizontal => (x, 0.0),
        }
    }
}

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
    /// Down on a gliding or springing scroller: it stopped it, and is no
    /// press (a lift lets the scroller go; a drag scrolls).
    Caught,
}

#[derive(Clone, Copy, Debug)]
struct Finger {
    id: u64,
    down: Point,
    down_at: f64,
    last: Point,
    mode: Mode,
    /// The axes a scroll moves, set when it starts.
    lock: Lock,
    /// Recent `(time, position)` samples, oldest first, for the velocity
    /// at the lift.
    samples: [(f64, Point); SAMPLES],
    len: usize,
}

impl Finger {
    fn sample(&mut self, now: f64, at: Point) {
        if self.len == SAMPLES {
            self.samples.copy_within(1.., 0);
            self.len -= 1;
        }
        self.samples[self.len] = (now, at);
        self.len += 1;
    }

    /// The finger's velocity at `now` (logical px/s) over its last
    /// [`VELOCITY_WINDOW`]; zero when it had stopped moving or barely moved.
    fn velocity(&self, now: f64) -> (f32, f32) {
        let recent = &self.samples[..self.len];
        let Some(&(last_t, last_p)) = recent.last() else {
            return (0.0, 0.0);
        };
        if now - last_t > STILL_AFTER {
            return (0.0, 0.0);
        }
        let Some(&(first_t, first_p)) = recent.iter().find(|(t, _)| last_t - t <= VELOCITY_WINDOW)
        else {
            return (0.0, 0.0);
        };
        let dt = last_t - first_t;
        if dt < 1e-3 {
            return (0.0, 0.0);
        }
        let travel = last_p - first_p;
        let (x, y) = self.lock.keep(travel.x, travel.y);
        let v = kurbo::Vec2::new(x, y) / dt;
        let speed = v.hypot();
        let v = if speed > MAX_VELOCITY {
            v * (MAX_VELOCITY / speed)
        } else {
            v
        };
        #[expect(clippy::cast_possible_truncation, reason = "velocities fit in f32")]
        (v.x as f32, v.y as f32)
    }
}

/// The touch that drives the pointer: the first finger to land, until it
/// lifts; others are ignored (the single-pointer model has no multi-touch).
///
/// A finger has no hover, so a lift also leaves; a cancel (the OS took the
/// gesture, as a system swipe does) leaves *before* releasing, so the
/// release lands on nothing and clicks nothing. With gestures on, a finger
/// that travels past [`TAP_SLOP`] scrolls instead (the press let go the same
/// way, then [`InputEvent::Pan`] steps at the point it landed, and on the
/// lift an [`InputEvent::PanEnd`] with its velocity), and one
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
    /// under the finger drags by itself, `moving` (read on `Started`) that
    /// a scroller under it is gliding or springing back, which the touch
    /// catches instead of pressing. Returns the events to dispatch, in
    /// order, or `None` for a finger that does not drive the pointer.
    pub(crate) fn event(
        &mut self,
        id: u64,
        phase: TouchPhase,
        at: Point,
        now: f64,
        owns_drag: bool,
        moving: bool,
    ) -> Option<Vec<InputEvent>> {
        if phase == TouchPhase::Started && self.finger.is_none() {
            // A moving scroller is caught first, even under a field or a
            // slider: the list moving under the finger is what it stops.
            let mode = if !self.gestures {
                Mode::Pointer
            } else if moving {
                Mode::Caught
            } else if owns_drag {
                Mode::Pointer
            } else {
                Mode::Pending
            };
            let mut finger = Finger {
                id,
                down: at,
                down_at: now,
                last: at,
                mode,
                lock: Lock::Free,
                samples: [(0.0, Point::ZERO); SAMPLES],
                len: 0,
            };
            finger.sample(now, at);
            self.finger = Some(finger);
            return Some(if mode == Mode::Caught {
                vec![
                    pointer_at(at),
                    InputEvent::PanStop,
                    InputEvent::PointerLeave,
                ]
            } else {
                vec![pointer_at(at), InputEvent::PointerDown]
            });
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
                    finger.lock = Lock::of(at - finger.down);
                    events.extend([InputEvent::PointerLeave, InputEvent::PointerUp]);
                    events.extend(scroll_step(finger.down, finger.last, at, finger.lock));
                }
                // A catch never pressed: past the slop it just scrolls.
                Mode::Caught if finger.down.distance(at) > TAP_SLOP => {
                    finger.mode = Mode::Scrolling;
                    finger.lock = Lock::of(at - finger.down);
                    events.extend(scroll_step(finger.down, finger.last, at, finger.lock));
                }
                // Undecided, the pointer stays where the finger landed: a
                // wiggle must not drag a text selection or a hover along.
                Mode::Pending | Mode::Caught => {}
                // After a long press the finger drives the pointer (a touch
                // selection's caret or end follows it).
                Mode::Pointer | Mode::LongPressed => events.push(pointer_at(at)),
                Mode::Scrolling => {
                    events.extend(scroll_step(finger.down, finger.last, at, finger.lock));
                }
            },
            TouchPhase::Ended => {
                events.extend(match finger.mode {
                    // A tap releases where it landed.
                    Mode::Pending => vec![InputEvent::PointerUp, InputEvent::PointerLeave],
                    Mode::Pointer => {
                        vec![
                            pointer_at(at),
                            InputEvent::PointerUp,
                            InputEvent::PointerLeave,
                        ]
                    }
                    // The scroller it drove (or caught) glides on at the
                    // finger's speed, or springs back.
                    Mode::Scrolling | Mode::Caught => {
                        let (vx, vy) = finger.velocity(now);
                        pan_end(finger.down, vx, vy).to_vec()
                    }
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
                    // Let go where it is: a stretch springs back.
                    Mode::Scrolling | Mode::Caught => pan_end(finger.down, 0.0, 0.0).to_vec(),
                    Mode::LongPressed => vec![InputEvent::PointerLeave],
                });
                self.finger = None;
                return Some(events);
            }
        }
        if at != finger.last {
            finger.sample(now, at);
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

/// The haptic an event a touch produced calls for, once dispatched with
/// `consumed` (whether it produced messages): a `long_press` — the right
/// click a held finger becomes — that did something. Nothing else asks for
/// one.
pub(crate) fn touch_haptic(long_press: bool, consumed: bool) -> Option<fenestra_core::Haptic> {
    (consumed && long_press).then_some(fenestra_core::Haptic::LongPress)
}

/// One scroll step of a finger that landed at `down`: the pointer goes to
/// the landing point only for the pan (so the same container scrolls) and
/// leaves again before any frame paints a hover there.
fn scroll_step(down: Point, from: Point, to: Point, lock: Lock) -> [InputEvent; 3] {
    [
        pointer_at(down),
        pan(from, to, lock),
        InputEvent::PointerLeave,
    ]
}

/// The lift of a finger that landed at `down` and was driving a scroller,
/// moving at `(vx, vy)`: at the landing point like each step.
fn pan_end(down: Point, vx: f32, vy: f32) -> [InputEvent; 3] {
    [
        pointer_at(down),
        InputEvent::PanEnd { vx, vy },
        InputEvent::PointerLeave,
    ]
}

/// The pan for a finger moving `from` → `to`: its travel on the axes
/// `lock` keeps, so content follows the finger (the core scrolls by the
/// negated delta).
#[expect(clippy::cast_possible_truncation, reason = "deltas fit in f32")]
fn pan(from: Point, to: Point, lock: Lock) -> InputEvent {
    let (dx, dy) = lock.keep(to.x - from.x, to.y - from.y);
    InputEvent::Pan {
        dx: dx as f32,
        dy: dy as f32,
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

    /// A long press that did something (its right click produced messages)
    /// asks for the long-press haptic; one that did nothing, and any other
    /// event, asks for none.
    #[test]
    fn a_long_press_that_acts_asks_for_a_haptic() {
        use fenestra_core::Haptic;
        assert_eq!(touch_haptic(true, true), Some(Haptic::LongPress));
        assert_eq!(touch_haptic(true, false), None);
        assert_eq!(touch_haptic(false, true), None);
    }

    /// A lift with no speed: a stretch springs back, nothing glides.
    fn still() -> InputEvent {
        InputEvent::PanEnd { vx: 0.0, vy: 0.0 }
    }

    /// Drags a finger from `(x, y0)` by `step` px every 10 ms, `n` times,
    /// starting at `t0`; returns the time of the last move.
    fn swipe(t: &mut Touch, x: f64, y0: f64, step: f64, n: usize, t0: f64) -> f64 {
        t.event(0, Started, p(x, y0), t0, false, false);
        let mut now = t0;
        for i in 1..=n {
            now = t0 + 0.01 * i as f64;
            t.event(0, Moved, p(x, y0 + step * i as f64), now, false, false);
        }
        now
    }

    fn pan_end(events: &[InputEvent]) -> (f32, f32) {
        events
            .iter()
            .find_map(|e| match e {
                InputEvent::PanEnd { vx, vy } => Some((*vx, *vy)),
                _ => None,
            })
            .expect("a PanEnd")
    }

    /// A mostly vertical drag is locked vertical: its sideways drift
    /// neither scrolls nor flings a horizontal scroller under it.
    #[test]
    fn a_mostly_vertical_drag_is_locked_vertical() {
        let mut t = Touch::new(true);
        t.event(0, Started, p(100., 500.), 0.0, false, false);
        let first = t
            .event(0, Moved, p(104., 470.), 0.01, false, false)
            .unwrap();
        assert!(
            first.contains(&InputEvent::Pan { dx: 0.0, dy: -30.0 }),
            "{first:?}"
        );
        let next = t
            .event(0, Moved, p(112., 440.), 0.02, false, false)
            .unwrap();
        assert!(
            next.contains(&InputEvent::Pan { dx: 0.0, dy: -30.0 }),
            "{next:?}"
        );
        let end = t
            .event(0, Ended, p(112., 440.), 0.02, false, false)
            .unwrap();
        let (vx, vy) = pan_end(&end);
        assert_eq!(vx, 0.0);
        assert!(vy < -1000.0);
    }

    /// A diagonal drag stays free on both axes.
    #[test]
    fn a_diagonal_drag_moves_both_axes() {
        let mut t = Touch::new(true);
        t.event(0, Started, p(100., 500.), 0.0, false, false);
        let first = t
            .event(0, Moved, p(120., 480.), 0.01, false, false)
            .unwrap();
        assert!(
            first.contains(&InputEvent::Pan {
                dx: 20.0,
                dy: -20.0
            }),
            "{first:?}"
        );
    }

    #[test]
    fn a_quick_swipe_ends_with_its_velocity() {
        let mut t = Touch::new(true);
        let last = swipe(&mut t, 100., 500., -20., 5, 0.0);
        let events = t
            .event(0, Ended, p(100., 400.), last, false, false)
            .unwrap();
        let (vx, vy) = pan_end(&events);
        assert_eq!(vx, 0.0);
        assert!((vy + 2000.0).abs() < 200.0, "about -2000 px/s: {vy}");
        assert_eq!(events.last(), Some(&InputEvent::PointerLeave));
    }

    #[test]
    fn a_held_finger_releases_without_velocity() {
        let mut t = Touch::new(true);
        let last = swipe(&mut t, 100., 500., -20., 5, 0.0);
        let events = t
            .event(0, Ended, p(100., 400.), last + 0.08, false, false)
            .unwrap();
        assert_eq!(pan_end(&events), (0.0, 0.0));
    }

    #[test]
    fn the_velocity_is_capped() {
        let mut t = Touch::new(true);
        let last = swipe(&mut t, 100., 2000., -300., 5, 0.0);
        let events = t
            .event(0, Ended, p(100., 500.), last, false, false)
            .unwrap();
        let (_, vy) = pan_end(&events);
        assert!((vy + 8000.0).abs() < 1.0, "capped: {vy}");
    }

    #[test]
    fn a_touch_on_a_moving_scroller_catches_it() {
        let mut t = Touch::new(true);
        assert_eq!(
            t.event(0, Started, p(50., 60.), 0.0, false, true).unwrap(),
            vec![mv(50., 60.), InputEvent::PanStop, InputEvent::PointerLeave]
        );
        assert_eq!(t.deadline(), None, "no long press");
        assert!(t.tick(1.0).is_empty());
        assert_eq!(
            t.event(0, Ended, p(50., 60.), 1.1, false, false).unwrap(),
            vec![mv(50., 60.), still(), InputEvent::PointerLeave]
        );
    }

    /// A finger landing on a gliding list catches it even over a field or a
    /// slider: the list moving under it is what the finger stops.
    #[test]
    fn a_touch_on_a_moving_scroller_catches_even_over_a_field() {
        let mut t = Touch::new(true);
        assert_eq!(
            t.event(0, Started, p(50., 60.), 0.0, true, true).unwrap(),
            vec![mv(50., 60.), InputEvent::PanStop, InputEvent::PointerLeave]
        );
    }

    #[test]
    fn a_catch_that_drags_scrolls() {
        let mut t = Touch::new(true);
        t.event(0, Started, p(50., 300.), 0.0, false, true);
        assert_eq!(
            t.event(0, Moved, p(50., 270.), 0.05, false, false).unwrap(),
            vec![
                mv(50., 300.),
                InputEvent::Pan { dx: 0.0, dy: -30.0 },
                InputEvent::PointerLeave,
            ]
        );
    }

    #[test]
    fn a_touch_outside_the_moving_scroller_still_clicks() {
        let mut t = Touch::new(true);
        assert_eq!(
            t.event(0, Started, p(50., 60.), 0.0, false, false).unwrap(),
            vec![mv(50., 60.), InputEvent::PointerDown]
        );
        assert_eq!(
            t.event(0, Ended, p(50., 60.), 0.1, false, false).unwrap(),
            vec![InputEvent::PointerUp, InputEvent::PointerLeave]
        );
    }

    #[test]
    fn a_tap_sends_no_pan_end() {
        let mut t = Touch::new(true);
        t.event(0, Started, p(50., 60.), 0.0, false, false);
        let events = t.event(0, Ended, p(52., 61.), 0.1, false, false).unwrap();
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, InputEvent::PanEnd { .. }))
        );
    }

    #[expect(clippy::cast_possible_truncation, reason = "test positions are small")]
    fn mv(x: f64, y: f64) -> InputEvent {
        InputEvent::PointerMove {
            x: x as f32,
            y: y as f32,
        }
    }

    /// A finger that moves a little (within the slop) still taps, where it
    /// landed: the wiggle never moves the pointer, so it cannot start a text
    /// selection either.
    #[test]
    fn a_tap_that_wiggles_within_the_slop_still_clicks() {
        let mut t = Touch::new(true);
        assert_eq!(
            t.event(0, Started, p(100., 100.), 0.0, false, false)
                .unwrap(),
            vec![mv(100., 100.), InputEvent::PointerDown]
        );
        assert!(
            t.event(0, Moved, p(106., 104.), 0.1, false, false)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            t.event(0, Ended, p(106., 104.), 0.2, false, false).unwrap(),
            vec![InputEvent::PointerUp, InputEvent::PointerLeave]
        );
        assert_eq!(t.deadline(), None);
    }

    /// Past the slop the touch scrolls: the press is let go without a click
    /// (the pointer leaves first), and the finger's travel becomes wheel
    /// deltas at the point it landed — the pointer there only for the step,
    /// so no hover tint follows the content.
    #[test]
    fn a_drag_that_starts_on_a_button_scrolls_without_clicking() {
        let mut t = Touch::new(true);
        t.event(0, Started, p(100., 300.), 0.0, false, false);
        assert_eq!(
            t.event(0, Moved, p(100., 280.), 0.05, false, false)
                .unwrap(),
            vec![
                InputEvent::PointerLeave,
                InputEvent::PointerUp,
                mv(100., 300.),
                InputEvent::Pan { dx: 0.0, dy: -20.0 },
                InputEvent::PointerLeave,
            ]
        );
        assert_eq!(
            t.event(0, Moved, p(100., 250.), 0.1, false, false).unwrap(),
            vec![
                mv(100., 300.),
                InputEvent::Pan { dx: 0.0, dy: -30.0 },
                InputEvent::PointerLeave,
            ]
        );
        // No long press once scrolling, however long the finger stays.
        assert!(t.tick(2.0).is_empty());
        // Held still that long, it lifts without a glide.
        assert_eq!(
            t.event(0, Ended, p(100., 250.), 2.1, false, false).unwrap(),
            vec![mv(100., 300.), still(), InputEvent::PointerLeave]
        );
    }

    /// Held still for half a second, the finger right-clicks where it is,
    /// once.
    #[test]
    fn a_held_finger_long_presses_once_at_half_a_second() {
        let mut t = Touch::new(true);
        t.event(0, Started, p(50., 60.), 1.0, false, false);
        assert_eq!(t.deadline(), Some(1.5));
        assert!(t.tick(1.49).is_empty());
        assert_eq!(
            t.tick(1.5),
            vec![InputEvent::RightDown, InputEvent::RightUp]
        );
        assert!(t.tick(1.6).is_empty());
        assert_eq!(t.deadline(), None);
    }

    /// After a long press the finger moves the pointer (a touch selection's
    /// caret or end follows it), and the lift leaves before it releases,
    /// so it clicks nothing.
    #[test]
    fn after_a_long_press_the_lift_clicks_nothing() {
        let mut t = Touch::new(true);
        t.event(0, Started, p(50., 60.), 0.0, false, false);
        t.tick(0.6);
        assert_eq!(
            t.event(0, Moved, p(80., 90.), 0.7, false, false).unwrap(),
            vec![InputEvent::PointerMove { x: 80.0, y: 90.0 }]
        );
        assert_eq!(
            t.event(0, Ended, p(80., 90.), 0.8, false, false).unwrap(),
            vec![InputEvent::PointerLeave, InputEvent::PointerUp]
        );
    }

    /// A slider, splitter or text field under the finger keeps its own drag.
    #[test]
    fn a_drag_on_an_element_that_owns_it_is_not_a_scroll() {
        let mut t = Touch::new(true);
        t.event(0, Started, p(10., 10.), 0.0, true, false);
        assert_eq!(t.deadline(), None);
        assert_eq!(
            t.event(0, Moved, p(60., 10.), 0.1, false, false).unwrap(),
            vec![mv(60., 10.)]
        );
        assert!(t.tick(1.0).is_empty());
    }

    /// One finger drives; a cancel (the OS took the gesture) ends it cleanly.
    #[test]
    fn a_second_finger_is_ignored_and_a_cancel_resets() {
        let mut t = Touch::new(true);
        t.event(0, Started, p(10., 10.), 0.0, false, false);
        t.event(0, Moved, p(10., 40.), 0.1, false, false); // scrolling
        assert_eq!(t.event(1, Started, p(200., 200.), 0.2, false, false), None);
        assert_eq!(
            t.event(0, Cancelled, p(10., 40.), 0.3, false, false)
                .unwrap(),
            vec![mv(10., 10.), still(), InputEvent::PointerLeave]
        );
        // The next touch starts fresh.
        assert_eq!(
            t.event(1, Started, p(5., 5.), 0.4, false, false).unwrap(),
            vec![mv(5., 5.), InputEvent::PointerDown]
        );
    }

    /// Without gestures (desktop touch screens, the web) a touch is only the
    /// pointer: a drag moves it, holding does nothing.
    #[test]
    fn without_gestures_a_touch_is_only_the_pointer() {
        let mut t = Touch::new(false);
        t.event(0, Started, p(10., 10.), 0.0, false, false);
        assert_eq!(t.deadline(), None);
        assert_eq!(
            t.event(0, Moved, p(10., 80.), 0.1, false, false).unwrap(),
            vec![mv(10., 80.)]
        );
        assert!(t.tick(5.0).is_empty());
        assert_eq!(
            t.event(0, Ended, p(10., 80.), 5.1, false, false).unwrap(),
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
            t.event(0, Started, p(10., 10.), 0.0, false, false);
            assert_eq!(
                t.event(0, Cancelled, p(10., 10.), 0.1, false, false)
                    .unwrap(),
                vec![InputEvent::PointerLeave, InputEvent::PointerUp]
            );
            assert_eq!(t.deadline(), None);
        }
    }
}
