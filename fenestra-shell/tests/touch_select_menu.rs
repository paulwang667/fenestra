//! The touch edit menu: its actions do what the keyboard shortcuts do,
//! the field's app hears about the edit, the input method's mirror
//! follows, and the menu stays on the canvas.

use fenestra_core::{
    App, EditAction, Element, ImeTextState, InputEvent, Theme, by, col, raw_input, text,
};
use fenestra_shell::Harness;
use kurbo::Insets;

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Set(String),
}

struct Page {
    field: String,
    masked: bool,
}

impl Default for Page {
    fn default() -> Self {
        Self {
            field: "foo bar".to_owned(),
            masked: false,
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
        col().p(20.0).gap(20.0).children([
            text("hello wide world").size_px(20.0).selectable(),
            raw_input(&self.field, "")
                .masked(self.masked)
                .on_input(|s| Msg::Set(s.to_owned()))
                .label("field")
                .w(150.0)
                .h(40.0),
        ])
    }
}

fn harness(page: Page, size: (u32, u32)) -> Harness<Page> {
    Harness::new(page, Theme::light(), size)
}

#[expect(clippy::cast_possible_truncation)]
fn pt(x: f64, y: f64) -> (f32, f32) {
    (x as f32, y as f32)
}

fn long_press_at(h: &mut Harness<Page>, at: (f32, f32)) {
    h.touch_down_at(at);
    h.pump(500.0);
    h.touch_up();
}

fn select_wide(h: &mut Harness<Page>) {
    let r = h.get(&by::label("hello wide world")).rect;
    long_press_at(h, pt(r.x0 + 75.0, r.center().y));
}

fn select_bar(h: &mut Harness<Page>) {
    let r = h.get(&by::label("field")).rect;
    long_press_at(h, pt(r.x1 - 12.0, r.center().y));
    assert_eq!(h.touch_selection().expect("selected").range, (4, 7));
}

/// Taps the menu item for `action`.
fn tap(h: &mut Harness<Page>, action: EditAction) {
    let rect = h
        .touch_selection()
        .expect("a touch selection")
        .menu
        .iter()
        .find(|(a, _)| *a == action)
        .unwrap_or_else(|| panic!("no {action:?} in the menu"))
        .1;
    let c = rect.center();
    h.touch_tap_at(pt(c.x, c.y));
}

#[test]
fn copy_from_the_menu_puts_the_selection_on_the_clipboard() {
    let mut h = harness(Page::default(), (400, 300));
    select_wide(&mut h);
    tap(&mut h, EditAction::Copy);
    assert_eq!(h.clipboard().get().as_deref(), Some("wide"));
    assert!(h.touch_selection().is_none());
}

#[test]
fn cut_and_paste_in_an_input() {
    let mut h = harness(Page::default(), (400, 300));
    select_bar(&mut h);
    tap(&mut h, EditAction::Cut);
    assert_eq!(h.app().field, "foo ");
    assert_eq!(h.clipboard().get().as_deref(), Some("bar"));
    assert!(h.touch_selection().is_none());
    // A tap on the caret, then Paste.
    let r = h.get(&by::label("field")).rect;
    h.pump(1000.0);
    h.touch_tap_at(pt(r.x1 - 12.0, r.center().y));
    tap(&mut h, EditAction::Paste);
    assert_eq!(h.app().field, "foo bar");
    let ime = h.ime_text_state().expect("a focused field");
    assert_eq!(ime.text, "foo bar");
    assert_eq!(ime.selection, (7, 7));
}

#[test]
fn select_all_from_the_menu() {
    let mut h = harness(Page::default(), (400, 300));
    select_wide(&mut h);
    tap(&mut h, EditAction::SelectAll);
    let sel = h.touch_selection().expect("still selected");
    assert_eq!(sel.range, (0, 16));
    assert!(!sel.menu.is_empty(), "the menu stays");

    let mut h = harness(Page::default(), (400, 300));
    select_bar(&mut h);
    tap(&mut h, EditAction::SelectAll);
    assert_eq!(h.touch_selection().expect("still selected").range, (0, 7));
}

#[test]
fn a_masked_input_offers_no_cut_or_copy() {
    let mut h = harness(
        Page {
            masked: true,
            ..Page::default()
        },
        (400, 300),
    );
    let r = h.get(&by::label("field")).rect;
    long_press_at(&mut h, pt(r.x1 - 12.0, r.center().y));
    let actions: Vec<_> = h
        .touch_selection()
        .expect("selected")
        .menu
        .iter()
        .map(|(a, _)| *a)
        .collect();
    assert_eq!(actions, vec![EditAction::SelectAll]);
}

#[test]
fn paste_while_composing_commits_first() {
    let mut h = harness(Page::default(), (400, 300));
    h.clipboard().set("X".to_owned());
    let r = h.get(&by::label("field")).rect;
    h.touch_tap_at(pt(r.x1 - 12.0, r.center().y));
    // The input method is composing "ni" at the end.
    let field = h.get(&by::label("field")).id;
    h.input(InputEvent::ImeState(ImeTextState {
        text: "foo barni".to_owned(),
        selection: (9, 9),
        compose: Some((7, 9)),
        field: Some(field),
        ..ImeTextState::default()
    }));
    h.pump(1000.0);
    h.touch_tap_at(pt(r.x1 - 12.0, r.center().y));
    tap(&mut h, EditAction::Paste);
    let ime = h.ime_text_state().expect("a focused field");
    assert_eq!(ime.compose, None);
    assert!(
        !ime.text.contains("ni"),
        "no leftover pinyin: {:?}",
        ime.text
    );
    assert!(ime.text.contains('X'));
}

#[test]
fn menu_flips_below_near_top_and_clamps_horizontally() {
    let mut h = harness(Page::default(), (200, 300));
    h.set_safe_area(Insets::new(0.0, 10.0, 0.0, 0.0));
    let r = h.get(&by::label("hello wide world")).rect;
    long_press_at(&mut h, pt(r.x0 + 75.0, r.center().y));
    let sel = h.touch_selection().expect("selected");
    let bottom = sel.end_handle.expect("handle").y0;
    for (_, item) in &sel.menu {
        assert!(item.y0 > bottom, "below the selection: {item:?}");
        assert!(
            item.x0 >= 0.0 && item.x1 <= 200.0,
            "on the canvas: {item:?}"
        );
    }
}
