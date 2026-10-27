//! Touch scrolling: a lifted finger flings, a touch catches, and the edges
//! stretch and spring back — `Pan` / `PanEnd` / `PanStop` from the shell's
//! touch handling. The wheel is unchanged.

use fenestra_core::{
    Element, Fonts, Frame, FrameState, InputEvent, Semantics, Theme, WidgetId, build_frame, by,
    col, dispatch, div, row, scroll_moving_at,
};
use kurbo::Point;

/// The viewport: 400 tall over 40 rows of 48 (max offset 1520).
const VIEW_H: f32 = 400.0;
const MAX: f32 = 40.0 * 48.0 - VIEW_H;
const TAU: f32 = 0.325;
const DT: f64 = 1.0 / 60.0;

fn list() -> Element<()> {
    col().w(400.0).h(500.0).child(
        col()
            .id("list")
            .w(200.0)
            .h(VIEW_H)
            .scroll_y()
            .hide_scrollbar()
            .children((0..40).map(|i| {
                div()
                    .id(&format!("row{i}"))
                    .semantics(Semantics::Label)
                    .h(48.0)
                    .shrink0()
            })),
    )
}

/// A horizontal strip (rows 600 wide in 200) inside a vertical list.
fn nested() -> Element<()> {
    col().w(400.0).h(500.0).child(
        col()
            .id("outer")
            .w(200.0)
            .h(VIEW_H)
            .scroll_y()
            .hide_scrollbar()
            .child(
                row()
                    .id("strip")
                    .w(200.0)
                    .h(100.0)
                    .shrink0()
                    .scroll_x()
                    .hide_scrollbar()
                    .children((0..6).map(|_| div().w(100.0).h(100.0).shrink0())),
            )
            .children((0..40).map(|_| div().h(48.0).shrink0())),
    )
}

/// A view, its frame state and clock.
struct Touchy {
    view: Element<()>,
    fonts: Fonts,
    state: FrameState,
    frame: Frame,
    now: f64,
}

impl Touchy {
    fn new(view: Element<()>) -> Self {
        let mut fonts = Fonts::embedded();
        let mut state = FrameState::new();
        let frame = build_frame(&view, &Theme::light(), &mut fonts, &mut state, (400.0, 500.0), 1.0);
        let mut t = Self {
            view,
            fonts,
            state,
            frame,
            now: 0.0,
        };
        t.send(InputEvent::PointerMove { x: 50.0, y: 200.0 });
        t
    }

    /// Swaps the view (the next frame builds it).
    fn set_view(&mut self, view: Element<()>) {
        self.view = view;
        self.build();
    }

    fn send(&mut self, event: InputEvent) {
        let _ = dispatch(&self.view, &self.frame, &mut self.state, &mut self.fonts, event);
        self.build();
    }

    fn build(&mut self) {
        self.state.tick(self.now);
        self.frame = build_frame(
            &self.view,
            &Theme::light(),
            &mut self.fonts,
            &mut self.state,
            (400.0, 500.0),
            1.0,
        );
    }

    /// Runs frames at 60 Hz for `secs`.
    fn run(&mut self, secs: f64) {
        let end = self.now + secs;
        while self.now < end - 1e-9 {
            self.now += DT;
            self.build();
        }
    }

    fn id(&self, at: Point) -> WidgetId {
        self.frame.scrollable_at(at).expect("a scroller")
    }

    fn list(&self) -> WidgetId {
        self.id(Point::new(50.0, 200.0))
    }

    fn offset(&self) -> f32 {
        self.state.scroll_offset(self.list())
    }

    fn stretch(&self) -> f32 {
        self.state.scroll_stretch(self.list()).1
    }

    fn pan(&mut self, dy: f32) {
        self.send(InputEvent::Pan { dx: 0.0, dy });
    }

    fn end(&mut self, vy: f32) {
        self.send(InputEvent::PanEnd { vx: 0.0, vy });
    }
}

/// The rubber band for `x` px dragged past the edge of a `d` viewport.
fn band(x: f32, d: f32) -> f32 {
    (1.0 - 1.0 / (x * 0.55 / d + 1.0)) * d
}

#[test]
fn a_fling_glides_and_slows_to_a_stop() {
    let mut t = Touchy::new(list());
    t.pan(-100.0);
    assert_eq!(t.offset(), 100.0, "content follows the finger");
    t.end(-2000.0);
    t.run(0.1);
    let early = t.offset();
    assert!(early > 100.0, "it glides on: {early}");
    t.run(3.0);
    let rest = t.offset();
    let glide = rest - 100.0;
    let expected = 2000.0 * TAU;
    assert!(
        (glide - expected).abs() < expected * 0.1,
        "glides {glide}, about {expected}"
    );
    assert!(!t.frame.animating, "stopped: no more frames");
    t.run(0.2);
    assert_eq!(t.offset(), rest);
}

#[test]
fn a_slow_release_does_not_glide() {
    let mut t = Touchy::new(list());
    t.pan(-100.0);
    t.end(-40.0);
    t.run(0.5);
    assert_eq!(t.offset(), 100.0);
    assert!(!t.frame.animating);
}

#[test]
fn dragging_past_the_top_follows_the_rubber_band() {
    let mut t = Touchy::new(list());
    t.pan(100.0);
    assert_eq!(t.offset(), 0.0, "the offset stays in range");
    let want = -band(100.0, VIEW_H);
    assert!(
        (t.stretch() - want).abs() < 0.5,
        "stretch {} vs {want}",
        t.stretch()
    );
}

#[test]
fn a_release_past_the_edge_springs_back() {
    let mut t = Touchy::new(list());
    t.pan(100.0);
    t.end(0.0);
    t.run(0.1);
    assert!(t.stretch() < -0.5, "still coming back: {}", t.stretch());
    t.run(0.6);
    assert!(t.stretch().abs() < 0.5, "back: {}", t.stretch());
    assert_eq!(t.offset(), 0.0);
    assert!(!t.frame.animating);
}

#[test]
fn a_fling_into_the_end_overshoots_then_settles() {
    let mut t = Touchy::new(list());
    let list = t.list();
    t.state.scroll_to(list, MAX - 50.0);
    t.build();
    t.pan(-1.0);
    t.end(-4000.0);
    let mut deepest = 0.0_f32;
    for _ in 0..120 {
        t.run(DT);
        deepest = deepest.max(t.stretch());
    }
    assert!(deepest > 1.0, "it overshoots: {deepest}");
    assert!(deepest <= VIEW_H / 4.0 + 0.5, "by at most a quarter: {deepest}");
    assert_eq!(t.offset(), MAX);
    assert!(t.stretch().abs() < 0.5);
    assert!(!t.frame.animating);
}

#[test]
fn pan_stop_halts_a_fling() {
    let mut t = Touchy::new(list());
    t.pan(-100.0);
    t.end(-2000.0);
    t.run(0.1);
    t.send(InputEvent::PanStop);
    let held = t.offset();
    t.run(0.5);
    assert_eq!(t.offset(), held);
}

#[test]
fn catching_a_bounce_holds_it_and_drag_continues() {
    let mut t = Touchy::new(list());
    t.pan(100.0);
    t.end(0.0);
    t.run(2.0 * DT);
    t.send(InputEvent::PanStop);
    let held = t.stretch();
    assert!(held < -1.0, "caught mid-bounce: {held}");
    t.run(0.2);
    assert_eq!(t.stretch(), held, "held while the finger is down");
    t.pan(10.0);
    let step = held - t.stretch();
    assert!(
        step > 0.0 && step < 5.5,
        "the band goes on from where it was: {held} → {}",
        t.stretch()
    );
}

#[test]
fn reduced_motion_never_overscrolls() {
    let mut t = Touchy::new(list());
    t.state.reduced_motion = true;
    t.pan(100.0);
    assert_eq!(t.stretch(), 0.0);
    t.end(0.0);
    t.pan(-100.0);
    t.end(-2000.0);
    t.run(0.1);
    assert!(t.offset() > 100.0, "the fling stays");
    let list = t.list();
    t.state.scroll_to(list, MAX - 20.0);
    t.build();
    t.pan(-1.0);
    t.end(-4000.0);
    for _ in 0..60 {
        t.run(DT);
        assert_eq!(t.stretch(), 0.0);
    }
}

#[test]
fn the_wheel_is_unchanged() {
    let mut t = Touchy::new(list());
    t.send(InputEvent::Wheel { dx: 0.0, dy: 100.0 });
    assert_eq!(t.offset(), 0.0);
    assert_eq!(t.stretch(), 0.0);
    t.run(0.1);
    assert_eq!(t.offset(), 0.0);
    assert!(!t.frame.animating);
}

#[test]
fn each_axis_flings_its_own_scroller() {
    let mut t = Touchy::new(nested());
    // The pointer sits on the strip (top 100 of the outer list).
    let strip = t.frame.scrollable_x_at(Point::new(50.0, 50.0)).expect("strip");
    let outer = t.frame.scrollable_y_at(Point::new(50.0, 50.0)).expect("outer");
    assert_ne!(strip, outer);
    t.send(InputEvent::PointerMove { x: 50.0, y: 50.0 });
    t.send(InputEvent::Pan { dx: -20.0, dy: -20.0 });
    t.send(InputEvent::PanEnd {
        vx: -1500.0,
        vy: -1500.0,
    });
    t.run(0.2);
    assert!(t.state.scroll_offset_x(strip) > 20.0, "the strip glides");
    assert!(t.state.scroll_offset(outer) > 20.0, "the list glides");
}

#[test]
fn scroll_moving_at_sees_only_the_moving_scroller() {
    let mut t = Touchy::new(list());
    t.pan(-100.0);
    t.end(-2000.0);
    t.run(0.1);
    assert!(scroll_moving_at(&t.frame, &t.state, Point::new(50.0, 200.0)));
    assert!(!scroll_moving_at(
        &t.frame,
        &t.state,
        Point::new(300.0, 200.0)
    ));
    t.run(3.0);
    assert!(!scroll_moving_at(&t.frame, &t.state, Point::new(50.0, 200.0)));
}

/// A sticky log of `rows` rows of 48 in a 400 viewport.
fn feed(rows: usize) -> Element<()> {
    col().w(400.0).h(500.0).child(
        col()
            .id("feed")
            .w(200.0)
            .h(VIEW_H)
            .scroll_y()
            .stick_to_bottom()
            .hide_scrollbar()
            .children((0..rows).map(|_| div().h(48.0).shrink0())),
    )
}

#[test]
fn the_stretch_moves_what_is_painted_and_hit() {
    let mut t = Touchy::new(list());
    let still = t.frame.get(&by::id("row0")).rect;
    t.pan(100.0);
    let pulled = t.frame.get(&by::id("row0")).rect;
    let stretch = -t.stretch();
    assert!(stretch > 40.0);
    assert!(
        (pulled.y0 - still.y0 - f64::from(stretch)).abs() < 0.5,
        "row 0 moved {} for a stretch of {stretch}",
        pulled.y0 - still.y0
    );
    let row0 = t.frame.get(&by::id("row0")).id;
    let inside = kurbo::Point::new(50.0, pulled.y0 + 2.0);
    assert!(t.frame.hit_chain(inside).contains(&row0), "hit where it is painted");
}

#[test]
fn a_sticky_feed_stays_pinned_after_a_bounce() {
    let mut t = Touchy::new(feed(40));
    let feed_id = t.list();
    assert_eq!(t.state.scroll_offset(feed_id), MAX, "starts at the bottom");
    t.pan(-80.0);
    assert!(t.state.scroll_stretch(feed_id).1 > 10.0, "pulled past the bottom");
    t.end(0.0);
    t.run(1.0);
    assert_eq!(t.state.scroll_stretch(feed_id).1, 0.0);
    assert_eq!(t.state.scroll_offset(feed_id), MAX);
    t.set_view(feed(42));
    assert_eq!(
        t.state.scroll_offset(feed_id),
        MAX + 96.0,
        "still follows new rows"
    );
}

#[test]
fn a_vanished_scroller_stops_animating() {
    let mut t = Touchy::new(list());
    t.pan(-100.0);
    t.end(-3000.0);
    t.run(0.1);
    assert!(t.frame.animating);
    t.set_view(col().w(400.0).h(500.0));
    t.run(DT);
    assert!(!t.frame.animating);
    t.run(DT);
    assert!(!t.frame.animating);
}

#[test]
fn a_still_frame_does_not_animate() {
    let mut t = Touchy::new(list());
    t.run(0.5);
    assert!(!t.frame.animating);
    assert_eq!(t.stretch(), 0.0);
}
