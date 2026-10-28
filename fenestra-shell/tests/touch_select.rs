//! Touch text selection: a long press selects a word in selectable text or
//! a field, with the selection handles and the edit menu a phone shows; an
//! app's own long-press (right-click) menu wins; mouse input never starts
//! one. Driven through the shell's touch translator by the harness.

use fenestra_core::{App, EditAction, Element, Haptic, Theme, by, col, div, raw_input, text};
use fenestra_shell::Harness;

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Set(String),
    Menu,
}

struct Page {
    text: Option<String>,
    field: String,
    menu_row: bool,
}

impl Default for Page {
    fn default() -> Self {
        Self {
            text: Some("hello wide world".to_owned()),
            field: "foo bar".to_owned(),
            menu_row: false,
        }
    }
}

impl App for Page {
    type Msg = Msg;
    fn update(&mut self, msg: Msg) {
        if let Msg::Set(s) = msg {
            self.field = s;
        }
    }
    fn view(&self) -> Element<Msg> {
        let mut children = Vec::new();
        if let Some(t) = &self.text {
            let line = text(t.clone()).size_px(20.0).selectable();
            children.push(if self.menu_row {
                div()
                    .label("menu row")
                    .on_right_click(Msg::Menu)
                    .child(line)
            } else {
                line
            });
        }
        children.push(
            raw_input(&self.field, "")
                .on_input(|s| Msg::Set(s.to_owned()))
                .label("field")
                .w(240.0)
                .h(40.0),
        );
        children.push(div().label("blank").w(240.0).h(80.0));
        col().p(20.0).gap(20.0).children(children)
    }
}

fn harness(page: Page) -> Harness<Page> {
    Harness::new(page, Theme::light(), (400, 400))
}

/// A point inside the field near its right edge: past "foo bar", so the
/// word there is "bar".
fn field_end(h: &Harness<Page>) -> (f32, f32) {
    let r = h.get(&by::label("field")).rect;
    #[expect(clippy::cast_possible_truncation)]
    ((r.x1 - 12.0) as f32, r.center().y as f32)
}

/// A point on "wide" in "hello wide world" at 20px (the text node spans
/// the column, so its centre is past the text).
fn on_wide(h: &Harness<Page>) -> (f32, f32) {
    let r = h.get(&by::label("hello wide world")).rect;
    #[expect(clippy::cast_possible_truncation)]
    ((r.x0 + 75.0) as f32, r.center().y as f32)
}

/// Holds a finger still at `at` for a long press.
fn long_press_at(h: &mut Harness<Page>, at: (f32, f32)) {
    h.touch_down_at(at);
    h.pump(500.0);
    h.touch_up();
}

fn actions(h: &Harness<Page>) -> Vec<EditAction> {
    h.touch_selection()
        .expect("a touch selection")
        .menu
        .iter()
        .map(|(a, _)| *a)
        .collect()
}

#[test]
fn long_press_selects_a_word_in_selectable_text() {
    let mut h = harness(Page::default());
    let at = on_wide(&h);
    long_press_at(&mut h, at);
    let sel = h.touch_selection().expect("a touch selection");
    assert_eq!(sel.range, (6, 10));
    assert!(sel.start_handle.is_some() && sel.end_handle.is_some());
    assert_eq!(actions(&h), vec![EditAction::Copy, EditAction::SelectAll]);
    assert_eq!(h.haptics(), &[Haptic::LongPress]);
}

#[test]
fn long_press_selects_a_word_in_an_input() {
    let mut h = harness(Page::default());
    let at = field_end(&h);
    h.touch_down_at(at);
    h.pump(500.0);
    h.touch_up();
    assert_eq!(
        h.touch_selection().expect("a touch selection").range,
        (4, 7)
    );
    // An empty clipboard offers no Paste.
    assert_eq!(
        actions(&h),
        vec![EditAction::Cut, EditAction::Copy, EditAction::SelectAll]
    );

    let mut h = harness(Page::default());
    h.clipboard().set("pasted".to_owned());
    h.touch_down_at(at);
    h.pump(500.0);
    h.touch_up();
    assert_eq!(
        actions(&h),
        vec![
            EditAction::Cut,
            EditAction::Copy,
            EditAction::Paste,
            EditAction::SelectAll
        ]
    );
}

#[test]
fn a_right_click_ancestor_wins_over_selection() {
    let mut h = harness(Page {
        menu_row: true,
        ..Page::default()
    });
    h.long_press(&by::label("hello wide world"));
    assert_eq!(h.take_messages(), vec![Msg::Menu]);
    assert!(h.touch_selection().is_none());
}

#[test]
fn a_tap_elsewhere_clears_the_selection() {
    let mut h = harness(Page::default());
    h.long_press(&by::label("hello wide world"));
    assert!(h.touch_selection().is_some());
    h.touch_tap(&by::label("blank"));
    assert!(h.touch_selection().is_none());
}

#[test]
fn double_tap_in_an_input_selects_a_word() {
    let mut h = harness(Page::default());
    let at = field_end(&h);
    h.touch_tap_at(at);
    h.touch_tap_at(at);
    assert_eq!(
        h.touch_selection().expect("a touch selection").range,
        (4, 7)
    );
}

#[test]
fn a_tap_on_the_caret_opens_paste_and_select_all() {
    let mut h = harness(Page::default());
    h.clipboard().set("pasted".to_owned());
    let at = field_end(&h);
    h.touch_tap_at(at);
    assert!(
        h.touch_selection().is_none(),
        "the first tap only places the caret"
    );
    h.pump(1000.0);
    h.touch_tap_at(at);
    let sel = h.touch_selection().expect("a touch selection");
    assert_eq!(sel.range.0, sel.range.1);
    assert_eq!(actions(&h), vec![EditAction::Paste, EditAction::SelectAll]);
}

#[test]
fn mouse_input_never_starts_a_touch_selection() {
    let mut h = harness(Page::default());
    h.right_click(&by::label("hello wide world"));
    assert!(h.touch_selection().is_none());
    h.double_click(&by::label("field"));
    assert!(h.touch_selection().is_none());
    h.click(&by::label("field"));
    h.pump(1000.0);
    h.click(&by::label("field"));
    assert!(h.touch_selection().is_none());
}

#[test]
fn selection_survives_text_shrinking_and_removal() {
    let mut h = harness(Page::default());
    h.long_press(&by::label("hello wide world"));
    h.app_mut().text = Some("hello".to_owned());
    h.rebuild();
    assert!(
        h.touch_selection().is_none(),
        "a range past the new text is dropped"
    );

    let mut h = harness(Page::default());
    h.long_press(&by::label("hello wide world"));
    h.app_mut().text = None;
    h.rebuild();
    assert!(h.touch_selection().is_none());
    let _ = h.render();
}
