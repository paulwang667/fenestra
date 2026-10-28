//! The text input's keyboard, enter key and submit: `on_submit` fires on
//! Enter (an on-screen keyboard's action key arrives as one).

use fenestra_core::{App, Element, EnterKey, Key, KeyInput, Keyboard, SP4, Semantics, by, col};
use fenestra_kit::text_input;
use fenestra_shell::Harness;

#[derive(Default)]
struct Search {
    query: String,
    submitted: Vec<String>,
    /// Without `on_submit`: an explicit Search lets go of focus.
    no_submit: bool,
}

#[derive(Clone)]
enum Msg {
    Edit(String),
    Submit,
}

impl App for Search {
    type Msg = Msg;

    fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Edit(v) => self.query = v,
            Msg::Submit => self.submitted.push(self.query.clone()),
        }
    }

    fn view(&self) -> Element<Msg> {
        let field = text_input(&self.query)
            .placeholder("Search")
            .keyboard(Keyboard::Email)
            .enter_key(EnterKey::Search)
            .on_input(Msg::Edit);
        let field = if self.no_submit {
            field
        } else {
            field.on_submit(Msg::Submit)
        };
        col().p(SP4).items_start().children([field])
    }
}

#[test]
fn on_submit_fires_on_enter() {
    let mut h = Harness::new(Search::default(), fenestra_core::Theme::light(), (320, 120));
    h.click(&by::role(Semantics::TextInput { multiline: false }));
    h.key(KeyInput::plain(Key::Char('a')));
    h.key(KeyInput::plain(Key::Enter));
    assert_eq!(h.app().submitted, vec!["a".to_owned()]);
    // on_submit took the Enter, so the field keeps focus.
    h.key(KeyInput::plain(Key::Char('b')));
    assert_eq!(h.app().query, "ab");
}

#[test]
fn an_explicit_search_without_submit_lets_go() {
    let app = Search {
        no_submit: true,
        ..Search::default()
    };
    let mut h = Harness::new(app, fenestra_core::Theme::light(), (320, 120));
    h.click(&by::role(Semantics::TextInput { multiline: false }));
    h.key(KeyInput::plain(Key::Char('a')));
    h.key(KeyInput::plain(Key::Enter));
    h.key(KeyInput::plain(Key::Char('b')));
    assert_eq!(h.app().query, "a", "focus left with the Search key");
}
