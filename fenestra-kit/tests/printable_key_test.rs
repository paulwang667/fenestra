//! Printable keys reach a focused non-editor's `on_key` as the key they
//! stand for. The runner sends Space, `+` and `-` as `Text`, so before this a
//! focusable `on_key` never saw them, while arrows (non-printable) did.

use fenestra_core::{App, Element, Key, Theme, by, col, row, text};
use fenestra_kit::text_input;
use fenestra_shell::Harness;

#[derive(Default)]
struct Probe {
    keys: Vec<&'static str>,
    text: String,
    clicks: usize,
}

#[derive(Clone)]
enum Msg {
    Key(&'static str),
    Clicked,
    Typed(String),
}

impl App for Probe {
    type Msg = Msg;

    fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Key(k) => self.keys.push(k),
            Msg::Clicked => self.clicks += 1,
            Msg::Typed(s) => self.text = s,
        }
    }

    fn view(&self) -> Element<Msg> {
        // A lane-like element: takes the press (so it can hold focus) and
        // answers keys on its own.
        let lane = col()
            .id("lane")
            .focusable(true)
            .on_click(Msg::Clicked)
            .on_key(|k| match k.key {
                Key::Space => Some(Msg::Key("space")),
                Key::Char('+') => Some(Msg::Key("plus")),
                Key::Char('-') => Some(Msg::Key("minus")),
                Key::ArrowRight => Some(Msg::Key("right")),
                _ => None,
            })
            .child(text("lane"));
        // A button: Space activates it, and it has no `on_key` of its own.
        let button = row()
            .id("button")
            .focusable(true)
            .on_click(Msg::Clicked)
            .child(text("button"));
        let field = text_input(&self.text).on_input(Msg::Typed).id("field");
        col()
            .w(400.0)
            .children([lane, button, Element::from(field)])
    }
}

#[test]
fn space_and_plus_typed_on_a_focused_lane_reach_its_on_key() {
    let mut h = Harness::new(Probe::default(), Theme::dark(), (400, 300));
    h.click(&by::id("lane"));
    h.type_text(" ");
    h.type_text("+");
    h.type_text("-");
    assert_eq!(h.app().keys, ["space", "plus", "minus"]);
}

#[test]
fn an_arrow_still_reaches_the_lane_as_a_key_press() {
    let mut h = Harness::new(Probe::default(), Theme::dark(), (400, 300));
    h.click(&by::id("lane"));
    h.key(fenestra_core::KeyInput::plain(Key::ArrowRight));
    assert_eq!(h.app().keys, ["right"]);
}

#[test]
fn an_unhandled_character_on_a_focused_lane_does_nothing() {
    let mut h = Harness::new(Probe::default(), Theme::dark(), (400, 300));
    h.click(&by::id("lane"));
    h.type_text("q");
    assert!(h.app().keys.is_empty());
}

#[test]
fn space_on_a_focused_button_still_activates_it() {
    let mut h = Harness::new(Probe::default(), Theme::dark(), (400, 300));
    h.click(&by::id("button"));
    let before = h.app().clicks;
    h.type_text(" ");
    assert_eq!(h.app().clicks, before + 1);
    assert!(h.app().keys.is_empty());
}

#[test]
fn typing_into_a_text_input_is_not_a_key_press() {
    let mut h = Harness::new(Probe::default(), Theme::dark(), (400, 300));
    h.click(&by::id("field"));
    h.type_text("a b+c");
    assert_eq!(h.app().text, "a b+c", "the field takes every character");
    assert!(h.app().keys.is_empty(), "no lane key fired from the field");
}
