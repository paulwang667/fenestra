//! Dragging a touch selection: the handles move its ends (and can cross),
//! a drag after a long press moves the caret, a handle held at a
//! scroller's edge scrolls, and the handles ride along with scrolling.

use fenestra_core::{App, Element, Theme, WidgetId, by, col, raw_input, text};
use fenestra_shell::Harness;
use kurbo::Rect;

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Set(String),
}

struct Page {
    line: String,
    width: f32,
    field: String,
    filler: usize,
}

impl Default for Page {
    fn default() -> Self {
        Self {
            line: "hello wide world".to_owned(),
            width: 360.0,
            field: "foo bar".to_owned(),
            filler: 0,
        }
    }
}

impl App for Page {
    type Msg = Msg;
    fn update(&mut self, msg: Msg) {
        let Msg::Set(s) = msg;
        self.field = s;
    }
    fn view(&self) -> Element<Msg> {
        let mut rows = vec![
            text(self.line.clone())
                .size_px(20.0)
                .w(self.width)
                .selectable(),
            raw_input(&self.field, "")
                .on_input(|s| Msg::Set(s.to_owned()))
                .label("field")
                .w(240.0)
                .h(40.0),
        ];
        for i in 0..self.filler {
            rows.push(
                text(format!("filler line {i}"))
                    .size_px(20.0)
                    .h(40.0)
                    .shrink0(),
            );
        }
        col()
            .id("list")
            .p(20.0)
            .gap(20.0)
            .w_full()
            .h_full()
            .scroll_y()
            .children(rows)
    }
}

fn harness(page: Page) -> Harness<Page> {
    Harness::new(page, Theme::light(), (400, 300))
}

fn line_rect(h: &Harness<Page>) -> Rect {
    let line = h.app().line.clone();
    h.get(&by::label(line.as_str())).rect
}

#[expect(clippy::cast_possible_truncation)]
fn pt(x: f64, y: f64) -> (f32, f32) {
    (x as f32, y as f32)
}

/// Long-presses at `at` and lifts.
fn long_press_at(h: &mut Harness<Page>, at: (f32, f32)) {
    h.touch_down_at(at);
    h.pump(500.0);
    h.touch_up();
}

/// Long-presses "wide".
fn select_wide(h: &mut Harness<Page>) {
    let r = line_rect(h);
    long_press_at(h, pt(r.x0 + 75.0, r.center().y));
    assert_eq!(h.touch_selection().expect("selected").range, (6, 10));
}

fn list(h: &Harness<Page>) -> WidgetId {
    h.get(&by::id("list")).id
}

#[test]
fn dragging_the_end_handle_extends_the_selection() {
    let mut h = harness(Page::default());
    select_wide(&mut h);
    let handle = h
        .touch_selection()
        .unwrap()
        .end_handle
        .expect("end handle")
        .center();
    let r = line_rect(&h);
    h.touch_down_at(pt(handle.x, handle.y));
    h.touch_move_to(pt(r.x0 + 200.0, handle.y));
    let sel = h.touch_selection().unwrap();
    assert!(sel.menu.is_empty(), "the menu hides while dragging");
    assert!(
        sel.magnifier.is_some(),
        "the magnifier shows while dragging"
    );
    h.touch_up();
    let sel = h.touch_selection().unwrap();
    assert_eq!(sel.range, (6, 16));
    assert!(!sel.menu.is_empty(), "the menu is back");
    assert!(sel.magnifier.is_none());
}

#[test]
fn handles_cross_and_swap_ends() {
    let mut h = harness(Page::default());
    select_wide(&mut h);
    let handle = h
        .touch_selection()
        .unwrap()
        .start_handle
        .expect("start handle")
        .center();
    let r = line_rect(&h);
    h.touch_down_at(pt(handle.x, handle.y));
    h.touch_move_to(pt(r.x0 + 200.0, handle.y));
    h.touch_up();
    assert_eq!(h.touch_selection().unwrap().range, (10, 16));
}

#[test]
fn a_handle_drag_does_not_scroll() {
    let mut h = harness(Page {
        filler: 20,
        ..Page::default()
    });
    select_wide(&mut h);
    let handle = h.touch_selection().unwrap().end_handle.unwrap().center();
    h.touch_down_at(pt(handle.x, handle.y));
    h.touch_move_to(pt(handle.x - 30.0, handle.y + 60.0));
    h.touch_up();
    assert_eq!(h.frame().scroll_offset(list(&h)), 0.0);
}

#[test]
fn a_drag_on_text_without_a_selection_scrolls() {
    let mut h = harness(Page {
        filler: 20,
        ..Page::default()
    });
    let field = h.get(&by::label("field")).rect.center();
    h.touch_down_at(pt(field.x, field.y));
    h.touch_move_to(pt(field.x, field.y - 30.0));
    h.touch_move_to(pt(field.x, field.y - 80.0));
    h.touch_up();
    assert!(h.frame().scroll_offset(list(&h)) > 0.0, "it scrolled");
    assert!(h.touch_selection().is_none());
}

#[test]
fn long_press_then_drag_moves_the_caret() {
    let mut h = harness(Page::default());
    let field = h.get(&by::label("field")).rect;
    h.touch_down_at(pt(field.x1 - 12.0, field.center().y));
    h.pump(500.0);
    assert_eq!(h.touch_selection().unwrap().range, (4, 7));
    h.touch_move_to(pt(field.x0 - 20.0, field.center().y));
    let sel = h.touch_selection().unwrap();
    assert_eq!(sel.range, (0, 0), "the caret follows the finger");
    assert!(sel.magnifier.is_some());
    h.touch_up();
    assert!(h.touch_selection().unwrap().magnifier.is_none());
}

#[test]
fn dragging_into_the_edge_band_autoscrolls() {
    // Wrapped text taller than the 300px window.
    let words: Vec<String> = (0..60).map(|i| format!("word{i}")).collect();
    let mut h = harness(Page {
        line: words.join(" "),
        width: 200.0,
        ..Page::default()
    });
    let r = line_rect(&h);
    long_press_at(&mut h, pt(r.x0 + 10.0, r.y0 + 10.0));
    let handle = h
        .touch_selection()
        .expect("selected")
        .end_handle
        .unwrap()
        .center();
    let viewport = h.get(&by::id("list")).rect;
    h.touch_down_at(pt(handle.x, handle.y));
    h.touch_move_to(pt(handle.x, viewport.y1 - 4.0));
    let before = h.touch_selection().unwrap().range;
    h.pump(300.0);
    h.pump(16.0);
    assert!(h.frame().scroll_offset(list(&h)) > 0.0, "the list scrolled");
    let after = h.touch_selection().unwrap().range;
    assert!(
        after.1 > before.1,
        "the selection end followed the content: {before:?} -> {after:?}"
    );
    h.touch_up();
}

#[test]
fn handles_follow_scroll_and_hide_off_screen() {
    let mut h = harness(Page {
        filler: 20,
        ..Page::default()
    });
    select_wide(&mut h);
    let y0 = h.touch_selection().unwrap().end_handle.unwrap().center().y;
    h.wheel(&by::id("list"), -40.0);
    let y1 = h
        .touch_selection()
        .unwrap()
        .end_handle
        .expect("still visible")
        .center()
        .y;
    assert!(
        (y0 - y1 - 40.0).abs() < 1.0,
        "moved with the content: {y0} -> {y1}"
    );
    h.wheel(&by::id("list"), -400.0);
    assert!(
        h.touch_selection().unwrap().end_handle.is_none(),
        "scrolled out of view"
    );
    h.wheel(&by::id("list"), 1000.0);
    assert!(
        h.touch_selection().unwrap().end_handle.is_some(),
        "back in view"
    );
}

#[test]
fn handle_drag_snaps_to_grapheme_clusters() {
    let line = "中文👍🏽结尾 中文👍🏽结尾";
    let mut h = harness(Page {
        line: line.to_owned(),
        width: 120.0,
        ..Page::default()
    });
    let r = line_rect(&h);
    long_press_at(&mut h, pt(r.x0 + 5.0, r.y0 + 10.0));
    let handle = h
        .touch_selection()
        .expect("selected")
        .end_handle
        .unwrap()
        .center();
    let skin_tone = "中文👍".len();
    h.touch_down_at(pt(handle.x, handle.y));
    for step in 0..40 {
        let x = r.x0 + f64::from(step) * 3.0;
        h.touch_move_to(pt(x, handle.y));
        let (a, b) = h.touch_selection().unwrap().range;
        for end in [a, b] {
            assert!(line.is_char_boundary(end), "{end} splits a character");
            assert_ne!(end, skin_tone, "{end} splits 👍🏽");
        }
    }
    h.touch_up();
}
