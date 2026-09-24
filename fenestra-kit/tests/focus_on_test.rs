//! `.focus_on(token)`: focus moves once per request token — the programmatic
//! counterpart of `.autofocus()` — and stays wherever the user takes it.

use fenestra_core::{App, Element, Theme, by, col};
use fenestra_kit::text_input;
use fenestra_shell::Harness;

#[derive(Default)]
struct Two {
    a: String,
    b: String,
    /// 0 = never asked; each request bumps it.
    focus_b: u64,
}

#[derive(Clone)]
enum Msg {
    A(String),
    B(String),
    FocusB,
}

impl App for Two {
    type Msg = Msg;

    fn update(&mut self, msg: Msg) {
        match msg {
            Msg::A(s) => self.a = s,
            Msg::B(s) => self.b = s,
            Msg::FocusB => self.focus_b += 1,
        }
    }

    fn view(&self) -> Element<Msg> {
        let b: Element<Msg> = text_input(&self.b).on_input(Msg::B).id("b").into();
        let b = if self.focus_b > 0 {
            b.focus_on(self.focus_b)
        } else {
            b
        };
        col().w(400.0).children([
            Element::from(text_input(&self.a).on_input(Msg::A).id("a")),
            b,
        ])
    }
}

/// A request focuses the target; the user can leave; the next request brings
/// focus back, and the caret stays where the text ends.
#[test]
fn a_request_token_moves_focus_once_and_again_on_the_next_token() {
    let mut h = Harness::new(Two::default(), Theme::light(), (400, 200));
    h.type_text("x");
    assert_eq!(h.app().b, "", "no token yet: nothing is focused");

    h.update(Msg::FocusB);
    h.rebuild();
    h.type_text("one");
    assert_eq!(h.app().b, "one", "the first token focuses the target");

    // The user clicks into the other field; the unchanged token must not
    // pull focus back on the next frame.
    h.click(&by::id("a"));
    h.rebuild();
    h.type_text("elsewhere");
    assert_eq!(h.app().a, "elsewhere");
    assert_eq!(h.app().b, "one", "an unchanged token never steals focus");

    h.update(Msg::FocusB);
    h.rebuild();
    h.type_text(" two");
    assert_eq!(h.app().b, "one two", "the next token refocuses, caret kept at the end");
}
