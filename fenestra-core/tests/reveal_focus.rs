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
