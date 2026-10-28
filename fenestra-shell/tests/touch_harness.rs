//! The harness drives touch through the shell's own touch translator, so a
//! gesture in a test is the gesture on a phone: a held finger long-presses
//! (the right click it becomes, with the haptic it calls for), and core is
//! told when input comes from a finger and when from the mouse.

use fenestra_core::{App, Element, Haptic, PointerSource, Theme, by, col, div, text};
use fenestra_shell::Harness;

#[derive(Default)]
struct Row;

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Menu,
    Tap,
}

impl App for Row {
    type Msg = Msg;
    fn update(&mut self, _msg: Msg) {}
    fn view(&self) -> Element<Msg> {
        col().p(20.0).children([div()
            .w(200.0)
            .h(60.0)
            .label("the row")
            .on_click(Msg::Tap)
            .on_right_click(Msg::Menu)
            .child(text("row"))])
    }
}

#[test]
fn a_long_press_opens_a_right_click_menu() {
    let mut h = Harness::new(Row, Theme::light(), (300, 200));
    h.long_press(&by::label("the row"));
    assert_eq!(h.take_messages(), vec![Msg::Menu]);
    assert_eq!(h.haptics(), &[Haptic::LongPress]);
}

#[test]
fn touch_input_reports_touch_and_mouse_resets_it() {
    let mut h = Harness::new(Row, Theme::light(), (300, 200));
    assert_eq!(h.pointer_source(), PointerSource::Mouse);
    h.touch_tap(&by::label("the row"));
    assert_eq!(h.pointer_source(), PointerSource::Touch);
    assert_eq!(h.take_messages(), vec![Msg::Tap]);
    h.click(&by::label("the row"));
    assert_eq!(h.pointer_source(), PointerSource::Mouse);
}

#[test]
fn a_short_touch_hold_does_not_long_press() {
    let mut h = Harness::new(Row, Theme::light(), (300, 200));
    h.touch_down_at((120.0, 50.0));
    h.pump(300.0);
    h.touch_up();
    assert_eq!(h.take_messages(), vec![Msg::Tap]);
    assert!(h.haptics().is_empty());
}
