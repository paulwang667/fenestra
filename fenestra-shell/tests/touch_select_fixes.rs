//! Review fixes: what a real finger does that the first tests missed — it
//! jitters while held, scrolls by dragging, presses slowly — and what the
//! mouse must still never see.

use fenestra_core::{
    App, Color, EditAction, Element, ImeTextState, InputEvent, Key, KeyInput, Overlay, Theme, by,
    col, div, raw_input, text,
};
use fenestra_shell::Harness;

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Set(String),
    Menu,
}

struct Page {
    line: String,
    width: f32,
    field: String,
    filler: usize,
    menu_row: bool,
    modal: bool,
    token: u64,
}

impl Default for Page {
    fn default() -> Self {
        Self {
            line: "hello wide world".to_owned(),
            width: 360.0,
            field: "foo bar".to_owned(),
            filler: 0,
            menu_row: false,
            modal: false,
            token: 0,
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
        let mut line = text(self.line.clone())
            .size_px(20.0)
            .w(self.width)
            .selectable();
        if self.token > 0 {
            line = line.select_all_on(self.token);
        }
        let line = if self.menu_row {
            div()
                .label("menu row")
                .on_right_click(Msg::Menu)
                .child(line)
        } else {
            line
        };
        let field = raw_input(&self.field, "")
            .on_input(|s| Msg::Set(s.to_owned()))
            .label("field")
            .w(240.0)
            .h(40.0);
        let mut rows = vec![line];
        if self.modal {
            rows.push(
                col()
                    .w(300.0)
                    .h(120.0)
                    .p(20.0)
                    .bg(Color::WHITE)
                    .overlay(Overlay::modal())
                    .children([field]),
            );
        } else {
            rows.push(field);
        }
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
            .bg(Color::from_rgb8(255, 0, 0))
            .scroll_y()
            .children(rows)
    }
}

fn harness(page: Page) -> Harness<Page> {
    Harness::new(page, Theme::light(), (400, 300))
}

#[expect(clippy::cast_possible_truncation)]
fn pt(x: f64, y: f64) -> (f32, f32) {
    (x as f32, y as f32)
}

fn wide(h: &Harness<Page>) -> (f32, f32) {
    let r = h.get(&by::label("hello wide world")).rect;
    pt(r.x0 + 75.0, r.center().y)
}

fn long_press_at(h: &mut Harness<Page>, at: (f32, f32)) {
    h.touch_down_at(at);
    h.pump(500.0);
    h.touch_up();
}

fn range(h: &Harness<Page>) -> (usize, usize) {
    h.touch_selection().expect("a touch selection").range
}

fn tap_menu(h: &mut Harness<Page>, action: EditAction) {
    let rect = h
        .touch_selection()
        .unwrap()
        .menu
        .iter()
        .find(|(a, _)| *a == action)
        .unwrap()
        .1;
    let c = rect.center();
    h.touch_tap_at(pt(c.x, c.y));
}

/// Critical 1: a finger held still (Android keeps reporting moves) or
/// jittering after a long press keeps the word.
#[test]
fn a_still_finger_after_a_long_press_keeps_the_word() {
    let mut h = harness(Page::default());
    let (x, y) = wide(&h);
    h.touch_down_at((x, y));
    h.pump(500.0);
    h.touch_move_to((x, y));
    h.touch_move_to((x + 3.0, y + 2.0));
    h.touch_up();
    assert_eq!(range(&h), (6, 10));

    let r = h.get(&by::label("field")).rect;
    let (x, y) = pt(r.x1 - 12.0, r.center().y);
    h.touch_down_at((x, y));
    h.pump(500.0);
    h.touch_move_to((x - 2.0, y + 1.0));
    h.touch_up();
    assert_eq!(range(&h), (4, 7));
}

/// Critical 1: dragging after a long press in text keeps the pressed word
/// inside the range, either way.
#[test]
fn a_drag_after_a_long_press_keeps_the_word_in_the_range() {
    let mut h = harness(Page::default());
    let (x, y) = wide(&h);
    let r = h.get(&by::label("hello wide world")).rect;
    h.touch_down_at((x, y));
    h.pump(500.0);
    #[expect(clippy::cast_possible_truncation)]
    h.touch_move_to(((r.x0 + 2.0) as f32, y));
    let (a, b) = range(&h);
    h.touch_up();
    assert_eq!((a, b), (0, 10), "extended back to the start, the word kept");
}

/// Critical 2: a finger scroll keeps the selection (and its handles move).
#[test]
fn a_finger_scroll_keeps_the_selection() {
    let mut h = harness(Page {
        filler: 20,
        ..Page::default()
    });
    let at = wide(&h);
    long_press_at(&mut h, at);
    let y0 = h.touch_selection().unwrap().end_handle.unwrap().center().y;
    h.touch_down_at((300.0, 250.0));
    h.touch_move_to((300.0, 230.0));
    h.touch_move_to((300.0, 210.0));
    h.touch_up();
    let sel = h.touch_selection().expect("still selected after a scroll");
    assert_eq!(sel.range, (6, 10));
    let y1 = sel.end_handle.expect("still on screen").center().y;
    assert!(y1 < y0, "the handle moved with the content: {y0} -> {y1}");
}

/// Critical 2: a tap elsewhere still clears it, and still taps.
#[test]
fn a_tap_elsewhere_still_clears_and_reaches_the_field() {
    let mut h = harness(Page::default());
    let at = wide(&h);
    long_press_at(&mut h, at);
    let r = h.get(&by::label("field")).rect;
    h.touch_tap_at(pt(r.center().x, r.center().y));
    assert!(h.touch_selection().is_none());
    assert!(h.ime_text_state().is_some(), "the tap focused the field");
}

/// Important 3: under the mouse, select_all_on selects but shows no touch
/// handles or menu (Cmd/Ctrl+C copies it).
#[test]
fn select_all_on_under_the_mouse_shows_no_touch_controls() {
    let mut h = harness(Page::default());
    h.click(&by::label("field"));
    h.app_mut().token = 1;
    h.rebuild();
    assert!(h.touch_selection().is_none());
    h.key(KeyInput {
        ctrl: true,
        ..KeyInput::plain(Key::Char('c'))
    });
    assert_eq!(h.clipboard().get().as_deref(), Some("hello wide world"));
}

/// Important 4: Paste during a composition commits it (Gboard keeps the
/// current word composing) and keeps the user's text.
#[test]
fn paste_while_composing_keeps_the_composed_text() {
    let mut h = harness(Page::default());
    h.clipboard().set("X".to_owned());
    let r = h.get(&by::label("field")).rect;
    h.touch_tap_at(pt(r.x1 - 12.0, r.center().y));
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
    tap_menu(&mut h, EditAction::Paste);
    let ime = h.ime_text_state().unwrap();
    assert_eq!(ime.compose, None);
    assert_eq!(ime.text, "foo barniX");
}

/// Important 5: a slow press on a menu item is not a long press.
#[test]
fn a_slow_press_on_a_menu_item_still_acts() {
    let mut h = harness(Page::default());
    let at = wide(&h);
    long_press_at(&mut h, at);
    let rect = h
        .touch_selection()
        .unwrap()
        .menu
        .iter()
        .find(|(a, _)| *a == EditAction::Copy)
        .unwrap()
        .1;
    let c = rect.center();
    h.touch_down_at(pt(c.x, c.y));
    h.pump(800.0);
    h.touch_up();
    assert_eq!(h.clipboard().get().as_deref(), Some("wide"));
    assert_eq!(h.haptics().len(), 1, "only the first long press");
}

/// Important 6: a selection taller than the view keeps its menu on screen.
#[test]
fn the_menu_stays_on_screen_for_a_tall_selection() {
    let words: Vec<String> = (0..120).map(|i| format!("word{i}")).collect();
    let mut h = harness(Page {
        line: words.join(" "),
        width: 200.0,
        ..Page::default()
    });
    let r = h.get(&by::id("list")).rect;
    long_press_at(&mut h, pt(r.x0 + 30.0, r.y0 + 30.0));
    tap_menu(&mut h, EditAction::SelectAll);
    let sel = h.touch_selection().unwrap();
    assert!(!sel.menu.is_empty());
    for (_, item) in &sel.menu {
        assert!(item.y0 >= 0.0 && item.y1 <= 300.0, "on screen: {item:?}");
    }
}

/// Important 7: the magnifier shows what is on screen, overlays included.
#[test]
fn the_magnifier_shows_a_field_in_an_overlay() {
    let mut h = harness(Page {
        modal: true,
        ..Page::default()
    });
    let r = h.get(&by::label("field")).rect;
    let (x, y) = pt(r.x1 - 12.0, r.center().y);
    h.touch_down_at((x, y));
    h.pump(500.0);
    h.touch_move_to((x - 40.0, y));
    let mag = h.touch_selection().unwrap().magnifier.expect("magnifying");
    let img = h.render();
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let p = img
        .get_pixel((mag.x0 + 8.0) as u32, mag.center().y as u32)
        .0;
    assert!(
        !(p[0] > 200 && p[1] < 60 && p[2] < 60),
        "not the red page behind the modal: {p:?}"
    );
    h.touch_up();
}

/// Important 8: moving after a long press that opened an app menu drags
/// no selection under it.
#[test]
fn moving_after_an_app_menu_long_press_selects_nothing() {
    let mut h = harness(Page {
        menu_row: true,
        ..Page::default()
    });
    let (x, y) = wide(&h);
    h.touch_down_at((x, y));
    h.pump(500.0);
    assert_eq!(h.take_messages(), vec![Msg::Menu]);
    h.touch_move_to((x + 60.0, y));
    h.touch_up();
    h.key(KeyInput {
        ctrl: true,
        ..KeyInput::plain(Key::Char('c'))
    });
    assert_eq!(h.clipboard().get(), None, "no text was selected");
}
