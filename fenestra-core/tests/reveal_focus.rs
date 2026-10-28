//! The focused element scrolls into view: when focus moves to it, and when
//! the window changes size while it holds focus (an on-screen keyboard
//! shrinking the window) — once, leaving later scrolling to the user.

use fenestra_core::{
    Element, Fonts, Frame, FrameState, InputEvent, Overlay, OverlayMode, Semantics, Theme,
    WidgetId, build_frame, by, col, div, raw_input, raw_text_area, stack,
};
use kurbo::{Point, Rect};

const MARGIN: f64 = 8.0;

fn fields(n: usize) -> Vec<Element<()>> {
    (0..n)
        .map(|i| {
            raw_input(String::new(), "")
                .id(&format!("f{i}"))
                .h(48.0)
                .shrink0()
        })
        .collect()
}

/// A 200-tall list of 20 fields of 48 in the window.
fn list(_: ()) -> Element<()> {
    col().w(400.0).h_full().child(
        col()
            .id("list")
            .w(300.0)
            .h(200.0)
            .scroll_y()
            .hide_scrollbar()
            .children(fields(20)),
    )
}

struct Fixture {
    view: fn(()) -> Element<()>,
    size: (f32, f32),
    fonts: Fonts,
    state: FrameState,
    frame: Frame,
}

impl Fixture {
    fn new(view: fn(()) -> Element<()>) -> Self {
        let mut fonts = Fonts::embedded();
        let mut state = FrameState::new();
        let size = (400.0, 300.0);
        let frame = build_frame(
            &view(()),
            &Theme::light(),
            &mut fonts,
            &mut state,
            size,
            1.0,
        );
        Self {
            view,
            size,
            fonts,
            state,
            frame,
        }
    }

    fn build(&mut self) {
        self.frame = build_frame(
            &(self.view)(()),
            &Theme::light(),
            &mut self.fonts,
            &mut self.state,
            self.size,
            1.0,
        );
    }

    /// Focuses `name` and builds twice (the reveal's offset lands on the
    /// next frame).
    fn focus(&mut self, name: &str) {
        let id = self.id(name);
        self.state.set_focus(Some(id));
        self.build();
        self.build();
    }

    fn id(&self, name: &str) -> WidgetId {
        self.frame.get(&by::id(name)).id
    }

    fn rect(&self, name: &str) -> Rect {
        self.frame.get(&by::id(name)).rect
    }

    fn scroller(&self, at: Point) -> (WidgetId, Rect) {
        let id = self.frame.scrollable_at(at).expect("a scroller");
        (id, self.frame.rect_of(id).expect("its rect"))
    }
}

#[track_caller]
fn assert_inside(r: Rect, view: Rect, margin: f64) {
    assert!(
        r.y0 >= view.y0 + margin - 0.5 && r.y1 <= view.y1 - margin + 0.5,
        "{r:?} not inside {view:?} (margin {margin})"
    );
}

#[test]
fn tab_into_a_field_below_the_fold_reveals_it() {
    let mut f = Fixture::new(list);
    let (_, view) = f.scroller(Point::new(50.0, 50.0));
    assert!(f.rect("f9").y0 > view.y1, "starts out of view");
    for _ in 0..10 {
        let _ = fenestra_core::dispatch(
            &list(()),
            &f.frame,
            &mut f.state,
            &mut f.fonts,
            InputEvent::Tab,
        );
        f.build();
    }
    f.build();
    assert_eq!(f.state.focused(), Some(f.id("f9")));
    assert_inside(f.rect("f9"), view, MARGIN);
}

#[test]
fn a_visible_field_does_not_scroll() {
    let mut f = Fixture::new(list);
    let (list_id, _) = f.scroller(Point::new(50.0, 50.0));
    f.focus("f1");
    assert_eq!(f.state.scroll_offset(list_id), 0.0);
}

#[test]
fn a_shorter_window_reveals_the_focused_field() {
    // The list fills the window: a shorter window is a shorter list.
    fn full(_: ()) -> Element<()> {
        col().w(400.0).h_full().child(
            col()
                .id("list")
                .w(300.0)
                .h_full()
                .scroll_y()
                .hide_scrollbar()
                .children(fields(20)),
        )
    }
    let mut f = Fixture::new(full);
    f.focus("f5");
    let (list_id, _) = f.scroller(Point::new(50.0, 50.0));
    assert_eq!(f.state.scroll_offset(list_id), 0.0, "visible in 300");
    // A keyboard takes the bottom half.
    f.size = (400.0, 150.0);
    f.build();
    f.build();
    let (_, view) = f.scroller(Point::new(50.0, 50.0));
    assert_inside(f.rect("f5"), view, MARGIN);
}

#[test]
fn a_scrolled_away_field_is_left_alone() {
    let mut f = Fixture::new(list);
    f.focus("f1");
    let (list_id, _) = f.scroller(Point::new(50.0, 50.0));
    f.state.scroll_to(list_id, 500.0);
    f.build();
    f.build();
    f.build();
    assert_eq!(f.state.scroll_offset(list_id), 500.0, "not pulled back");
}

#[test]
fn nested_scrollers_both_reveal() {
    fn nested(_: ()) -> Element<()> {
        col().w(400.0).h_full().child(
            col()
                .id("outer")
                .w(300.0)
                .h(250.0)
                .scroll_y()
                .hide_scrollbar()
                .children([
                    div().h(300.0).shrink0(),
                    col()
                        .id("inner")
                        .w(280.0)
                        .h(150.0)
                        .shrink0()
                        .scroll_y()
                        .hide_scrollbar()
                        .children(fields(20)),
                    div().h(300.0).shrink0(),
                ]),
        )
    }
    let mut f = Fixture::new(nested);
    f.focus("f15");
    let outer = f
        .frame
        .rect_of(f.frame.scrollable_at(Point::new(10.0, 10.0)).unwrap())
        .unwrap();
    let field = f.rect("f15");
    assert_inside(field, outer, 0.0);
    let inner_id = f
        .frame
        .scrollable_at(field.center())
        .expect("inner under the field");
    let inner = f.frame.rect_of(inner_id).unwrap();
    assert_inside(field, inner, MARGIN);
}

#[test]
fn a_field_taller_than_the_view_aligns_its_top() {
    fn tall(_: ()) -> Element<()> {
        col().w(400.0).h_full().child(
            col()
                .id("list")
                .w(300.0)
                .h(200.0)
                .scroll_y()
                .hide_scrollbar()
                .children([
                    div().h(400.0).shrink0(),
                    raw_text_area(String::new(), "").id("ta").h(300.0).shrink0(),
                    div().h(400.0).shrink0(),
                ]),
        )
    }
    let mut f = Fixture::new(tall);
    f.focus("ta");
    let (_, view) = f.scroller(Point::new(50.0, 50.0));
    let ta = f.rect("ta");
    assert!(
        (ta.y0 - (view.y0 + MARGIN)).abs() < 0.5,
        "top aligned: {ta:?} in {view:?}"
    );
}

#[test]
fn padding_counts_as_hidden() {
    // 60 px of top padding, as a scaffold leaves under a floating bar.
    fn padded(_: ()) -> Element<()> {
        col().w(400.0).h_full().child(
            col()
                .id("list")
                .w(300.0)
                .h(250.0)
                .pt(60.0)
                .scroll_y()
                .hide_scrollbar()
                .children(fields(20)),
        )
    }
    let mut f = Fixture::new(padded);
    let (list_id, view) = f.scroller(Point::new(50.0, 100.0));
    f.state.scroll_to(list_id, 5.0 * 48.0 + 30.0);
    f.build();
    f.focus("f5");
    let content = Rect::new(view.x0, view.y0 + 60.0, view.x1, view.y1);
    assert_inside(f.rect("f5"), content, MARGIN);
}

#[test]
fn a_focused_menu_item_is_revealed_in_its_overlay() {
    fn menu(_: ()) -> Element<()> {
        stack().w(400.0).h_full().children([
            col().w_full().h_full(),
            col()
                .id("menu")
                .w(200.0)
                .h(150.0)
                .scroll_y()
                .hide_scrollbar()
                .overlay(Overlay {
                    mode: OverlayMode::Open,
                    ..Overlay::menu()
                })
                .children((0..20).map(|i| {
                    div()
                        .id(&format!("item{i}"))
                        .semantics(Semantics::Button)
                        .focusable(true)
                        .h(32.0)
                        .shrink0()
                })),
        ])
    }
    let mut f = Fixture::new(menu);
    f.focus("item15");
    let item = f.rect("item15");
    let menu_id = f
        .frame
        .scrollable_at(item.center())
        .expect("the menu scrolls");
    let view = f.frame.rect_of(menu_id).unwrap();
    assert_inside(item, view, MARGIN);
}

/// A 200-tall list of focusable rows, as kit buttons and list rows are.
fn rows(_: ()) -> Element<()> {
    col().w(400.0).h_full().child(
        col()
            .id("list")
            .w(300.0)
            .h(200.0)
            .scroll_y()
            .hide_scrollbar()
            .children((0..20).map(|i| {
                div()
                    .id(&format!("r{i}"))
                    .semantics(Semantics::Button)
                    .focusable(true)
                    .h(48.0)
                    .shrink0()
            })),
    )
}

/// A press focuses a row partly out of view (the finger is starting a
/// scroll): the list does not jump under it.
#[test]
fn a_press_on_a_partly_hidden_row_does_not_scroll() {
    let mut f = Fixture::new(rows);
    let (list_id, _) = f.scroller(Point::new(50.0, 50.0));
    f.state.scroll_to(list_id, 30.0);
    f.build();
    // Row 0 shows only its bottom 18 px; press there.
    for event in [
        InputEvent::PointerMove { x: 50.0, y: 10.0 },
        InputEvent::PointerDown,
    ] {
        let _ = fenestra_core::dispatch(&rows(()), &f.frame, &mut f.state, &mut f.fonts, event);
    }
    assert_eq!(f.state.focused(), Some(f.id("r0")), "the press focused it");
    f.build();
    f.build();
    assert_eq!(f.state.scroll_offset(list_id), 30.0, "no jump");
}

/// A field focused by a tap, then a keyboard shrinking the window: the field
/// still comes into view (the size changed while it showed).
#[test]
fn a_tapped_field_is_revealed_when_the_window_shrinks() {
    fn full(_: ()) -> Element<()> {
        col().w(400.0).h_full().child(
            col()
                .id("list")
                .w(300.0)
                .h_full()
                .scroll_y()
                .hide_scrollbar()
                .children(fields(20)),
        )
    }
    let mut f = Fixture::new(full);
    let at = f.rect("f5").center();
    for event in [
        InputEvent::PointerMove {
            x: at.x as f32,
            y: at.y as f32,
        },
        InputEvent::PointerDown,
        InputEvent::PointerUp,
    ] {
        let _ = fenestra_core::dispatch(&full(()), &f.frame, &mut f.state, &mut f.fonts, event);
    }
    f.build();
    assert_eq!(f.state.focused(), Some(f.id("f5")));
    f.size = (400.0, 150.0);
    f.build();
    f.build();
    let (_, view) = f.scroller(Point::new(50.0, 50.0));
    assert_inside(f.rect("f5"), view, MARGIN);
}

/// Scrolled away from the focused field, a resize (a window edge dragged, a
/// keyboard closed) leaves the list where the user put it.
#[test]
fn a_resize_leaves_a_scrolled_away_field_alone() {
    let mut f = Fixture::new(list);
    f.focus("f1");
    let (list_id, _) = f.scroller(Point::new(50.0, 50.0));
    f.state.scroll_to(list_id, 500.0);
    f.build();
    f.size = (400.0, 280.0);
    f.build();
    f.size = (400.0, 260.0);
    f.build();
    f.build();
    assert_eq!(f.state.scroll_offset(list_id), 500.0);
}

/// A field scrolled into view while the list is stretched past its end
/// lands where it would without the stretch: the stretch is part of where
/// it is painted, and it goes as the list scrolls.
#[test]
fn a_reveal_during_a_bounce_lands_fully_in_view() {
    let mut f = Fixture::new(list);
    let (list_id, view) = f.scroller(Point::new(50.0, 50.0));
    f.state.scroll_to(list_id, 1.0e6);
    f.build();
    let max = f.state.scroll_offset(list_id);
    // A finger pulls the bottom up past the end, and holds.
    for event in [
        InputEvent::PointerMove { x: 50.0, y: 100.0 },
        InputEvent::Pan { dx: 0.0, dy: -80.0 },
    ] {
        let _ = fenestra_core::dispatch(&list(()), &f.frame, &mut f.state, &mut f.fonts, event);
        f.build();
    }
    assert!(f.state.scroll_stretch(list_id).1 > 10.0, "stretched");
    assert_eq!(f.state.scroll_offset(list_id), max);
    f.focus("f3");
    let top = f.rect("f3").y0;
    assert!(
        (top - (view.y0 + MARGIN)).abs() < 0.5,
        "field top at {top}, view top + margin {}",
        view.y0 + MARGIN
    );
}

#[test]
fn a_focused_element_that_appears_later_is_revealed() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SHOW: AtomicBool = AtomicBool::new(true);
    fn late(_: ()) -> Element<()> {
        let mut kids = fields(20);
        if !SHOW.load(Ordering::Relaxed) {
            kids.pop();
        }
        col().w(400.0).h_full().child(
            col()
                .id("list")
                .w(300.0)
                .h(200.0)
                .scroll_y()
                .hide_scrollbar()
                .children(kids),
        )
    }
    let mut f = Fixture::new(late);
    let last = f.id("f19");
    SHOW.store(false, Ordering::Relaxed);
    f.build();
    f.state.set_focus(Some(last));
    f.build();
    f.build();
    SHOW.store(true, Ordering::Relaxed);
    f.build();
    f.build();
    let (_, view) = f.scroller(Point::new(50.0, 50.0));
    // The last row: the list ends at it, so no margin is left below.
    assert_inside(f.rect("f19"), view, 0.0);
}

#[test]
fn a_hidden_focused_element_does_not_scroll() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static HIDE: AtomicBool = AtomicBool::new(false);
    fn view(_: ()) -> Element<()> {
        let hide = HIDE.load(Ordering::Relaxed);
        let section = col()
            .id("section")
            .themed(move |_, mut s| {
                if hide {
                    s.display = fenestra_core::Display::None;
                }
                s
            })
            .child(raw_input(String::new(), "").id("ghost").h(48.0).shrink0());
        col().w(400.0).h_full().child(
            col()
                .id("list")
                .w(300.0)
                .h(200.0)
                .scroll_y()
                .hide_scrollbar()
                .children(fields(20))
                .child(section),
        )
    }
    let mut f = Fixture::new(view);
    let ghost = f.id("ghost");
    HIDE.store(true, Ordering::Relaxed);
    let (list_id, _) = f.scroller(Point::new(50.0, 50.0));
    f.state.scroll_to(list_id, 300.0);
    f.build();
    f.state.set_focus(Some(ghost));
    f.build();
    f.build();
    assert_eq!(
        f.state.scroll_offset(list_id),
        300.0,
        "no jump to a hidden field"
    );
}

#[test]
fn a_field_in_a_horizontal_scroller_is_revealed() {
    fn strip(_: ()) -> Element<()> {
        col().w(400.0).h_full().child(
            fenestra_core::row()
                .id("strip")
                .w(300.0)
                .h(60.0)
                .scroll_x()
                .hide_scrollbar()
                .children((0..12).map(|i| {
                    raw_input(String::new(), "")
                        .id(&format!("h{i}"))
                        .w(100.0)
                        .h(48.0)
                        .shrink0()
                })),
        )
    }
    let mut f = Fixture::new(strip);
    f.focus("h8");
    let (_, view) = f.scroller(Point::new(50.0, 30.0));
    let field = f.rect("h8");
    assert!(
        field.x0 >= view.x0 + MARGIN - 0.5 && field.x1 <= view.x1 - MARGIN + 0.5,
        "{field:?} in {view:?}"
    );
}

/// A field revealed at the bottom of a stick-to-bottom feed leaves it
/// pinned: new rows still bring it along.
#[test]
fn revealing_in_a_sticky_feed_keeps_it_pinned_at_the_bottom() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static ROWS: AtomicUsize = AtomicUsize::new(20);
    fn feed(_: ()) -> Element<()> {
        col().w(400.0).h_full().child(
            col()
                .id("feed")
                .w(300.0)
                .h(200.0)
                .scroll_y()
                .stick_to_bottom()
                .hide_scrollbar()
                .children(fields(ROWS.load(Ordering::Relaxed))),
        )
    }
    let mut f = Fixture::new(feed);
    let (feed_id, _) = f.scroller(Point::new(50.0, 50.0));
    let bottom = f.state.scroll_offset(feed_id);
    f.focus("f19");
    assert_eq!(f.state.scroll_offset(feed_id), bottom, "already in view");
    ROWS.store(22, Ordering::Relaxed);
    f.build();
    assert_eq!(
        f.state.scroll_offset(feed_id),
        bottom + 96.0,
        "still follows new rows"
    );
}
