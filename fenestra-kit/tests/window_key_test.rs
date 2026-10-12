//! `App::window_key`: a key press that nothing in the window took reaches the
//! app, whatever has focus. A key a focused element took never does.

use fenestra_core::{App, Element, Key, KeyInput, Theme, by, col, row, text};
use fenestra_kit::text_input;
use fenestra_shell::Harness;

#[derive(Default)]
struct Shell {
    /// Messages in the order they were applied.
    log: Vec<&'static str>,
    text: String,
}

#[derive(Clone)]
enum Msg {
    Log(&'static str),
    Typed(String),
}

impl App for Shell {
    type Msg = Msg;

    fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Log(s) => self.log.push(s),
            Msg::Typed(s) => self.text = s,
        }
    }

    fn window_key(&self, key: &KeyInput) -> Option<Msg> {
        match key.key {
            Key::Space => Some(Msg::Log("hook:space")),
            Key::Char('+') => Some(Msg::Log("hook:plus")),
            Key::ArrowRight => Some(Msg::Log("hook:right")),
            _ => None,
        }
    }

    fn view(&self) -> Element<Msg> {
        // A lane that answers Space itself, so the hook must not see Space
        // while it holds focus.
        let lane = col()
            .id("lane")
            .focusable(true)
            .on_click(Msg::Log("lane:click"))
            .on_key(|k| match k.key {
                Key::Space => Some(Msg::Log("lane:space")),
                _ => None,
            })
            .child(text("lane"));
        let button = row()
            .id("button")
            .focusable(true)
            .on_click(Msg::Log("button:click"))
            .child(text("button"));
        let field = text_input(&self.text).on_input(Msg::Typed).id("field");
        col()
            .w(400.0)
            .children([lane, button, Element::from(field)])
    }
}

#[test]
fn space_with_nothing_focused_reaches_the_window_hook() {
    let mut h = Harness::new(Shell::default(), Theme::dark(), (400, 300));
    h.type_text(" ");
    assert_eq!(h.app().log, ["hook:space"]);
}

#[test]
fn plus_with_nothing_focused_reaches_the_window_hook() {
    let mut h = Harness::new(Shell::default(), Theme::dark(), (400, 300));
    h.type_text("+");
    assert_eq!(h.app().log, ["hook:plus"]);
}

#[test]
fn an_arrow_with_nothing_focused_reaches_the_window_hook() {
    let mut h = Harness::new(Shell::default(), Theme::dark(), (400, 300));
    h.key(KeyInput::plain(Key::ArrowRight));
    assert_eq!(h.app().log, ["hook:right"]);
}

#[test]
fn a_focused_lane_that_takes_space_keeps_it_from_the_hook() {
    let mut h = Harness::new(Shell::default(), Theme::dark(), (400, 300));
    h.click(&by::id("lane"));
    h.type_text(" ");
    assert_eq!(h.app().log, ["lane:click", "lane:space"]);
}

#[test]
fn a_focused_button_keeps_space_from_the_hook() {
    let mut h = Harness::new(Shell::default(), Theme::dark(), (400, 300));
    h.click(&by::id("button"));
    h.type_text(" ");
    assert_eq!(h.app().log, ["button:click", "button:click"]);
}

#[test]
fn a_focused_text_field_takes_its_characters_and_the_hook_stays_silent() {
    let mut h = Harness::new(Shell::default(), Theme::dark(), (400, 300));
    h.click(&by::id("field"));
    h.type_text("+ ");
    assert_eq!(h.app().text, "+ ");
    assert!(h.app().log.is_empty(), "got {:?}", h.app().log);
}

#[test]
fn a_key_the_hook_does_not_map_does_nothing() {
    let mut h = Harness::new(Shell::default(), Theme::dark(), (400, 300));
    h.type_text("q");
    assert!(h.app().log.is_empty());
}
