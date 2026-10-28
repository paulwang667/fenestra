//! What a text field tells the keyboard it is, and what its enter key does:
//! the key `Default` resolves to, an explicit key over `submit_on_enter`,
//! Next moving focus, and an explicit Done / Search / Go letting go of it.

use fenestra_core::{
    Element, EnterKey, Fonts, FrameState, InputEvent, Key, KeyInput, Keyboard, Theme, WidgetId,
    build_frame, by, col, dispatch, raw_input, raw_text_area, resolve_enter_key,
};

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Input(String),
    Key,
}

/// A view builder: the fields, first one focused by the first Tab.
type View = fn(&str) -> Element<Msg>;

struct Fixture {
    view: View,
    value: String,
    theme: Theme,
    fonts: Fonts,
    state: FrameState,
}

impl Fixture {
    fn new(view: View) -> Self {
        let mut f = Self {
            view,
            value: String::new(),
            theme: Theme::light(),
            fonts: Fonts::embedded(),
            state: FrameState::new(),
        };
        f.send(InputEvent::Tab);
        f
    }

    fn frame(&mut self) -> (Element<Msg>, fenestra_core::Frame) {
        let v = (self.view)(&self.value);
        let frame = build_frame(
            &v,
            &self.theme,
            &mut self.fonts,
            &mut self.state,
            (300.0, 300.0),
            1.0,
        );
        (v, frame)
    }

    fn send(&mut self, event: InputEvent) -> Vec<Msg> {
        let (v, frame) = self.frame();
        let msgs = dispatch(&v, &frame, &mut self.state, &mut self.fonts, event).msgs;
        for m in &msgs {
            if let Msg::Input(s) = m {
                self.value.clone_from(s);
            }
        }
        msgs
    }

    fn enter(&mut self) -> Vec<Msg> {
        self.send(InputEvent::Key(KeyInput::plain(Key::Enter)))
    }

    fn id(&mut self, name: &str) -> WidgetId {
        self.frame().1.get(&by::id(name)).id
    }

    fn focused(&self) -> Option<WidgetId> {
        self.state.focused()
    }
}

fn field(id: &str, value: &str) -> Element<Msg> {
    raw_input(value.to_owned(), "")
        .id(id)
        .w(200.0)
        .h(32.0)
        .on_input(|v| Msg::Input(v.to_owned()))
}

#[test]
fn resolve_defaults_to_todays_behaviour() {
    assert_eq!(
        resolve_enter_key(EnterKey::Default, false, false),
        EnterKey::Done
    );
    assert_eq!(
        resolve_enter_key(EnterKey::Default, true, false),
        EnterKey::Newline
    );
    assert_eq!(
        resolve_enter_key(EnterKey::Default, true, true),
        EnterKey::Done
    );
    assert_eq!(
        resolve_enter_key(EnterKey::Newline, false, false),
        EnterKey::Done
    );
    assert_eq!(
        resolve_enter_key(EnterKey::Send, true, false),
        EnterKey::Send
    );
}

#[test]
fn ime_state_carries_the_field_kind() {
    let mut f = Fixture::new(|v| {
        col().children([field("a", v)
            .keyboard(Keyboard::Email)
            .enter_key(EnterKey::Next)])
    });
    let ime = f.state.ime_text_state().expect("a focused field");
    assert_eq!(ime.keyboard, Keyboard::Email);
    assert_eq!(ime.enter_key, EnterKey::Next);
    assert!(!ime.multiline);
    let _ = f.id("a");
}

#[test]
fn an_explicit_enter_key_overrides_submit_on_enter() {
    // A composer that asked for a newline key gets newlines.
    let mut f = Fixture::new(|v| {
        col().children([raw_text_area(v.to_owned(), "")
            .id("a")
            .w(200.0)
            .h(80.0)
            .submit_on_enter(true)
            .enter_key(EnterKey::Newline)
            .on_input(|v| Msg::Input(v.to_owned()))
            .on_key(|_| Some(Msg::Key))])
    });
    let msgs = f.enter();
    assert_eq!(msgs, vec![Msg::Input("\n".to_owned())], "a newline");

    // A text area with a Send key leaves Enter to the app.
    let mut f = Fixture::new(|v| {
        col().children([raw_text_area(v.to_owned(), "")
            .id("a")
            .w(200.0)
            .h(80.0)
            .enter_key(EnterKey::Send)
            .on_input(|v| Msg::Input(v.to_owned()))
            .on_key(|k| (k.key == Key::Enter).then_some(Msg::Key))])
    });
    assert_eq!(f.enter(), vec![Msg::Key], "the app's Enter, no newline");
}

fn two_fields(next: EnterKey) -> fn(&str) -> Element<Msg> {
    match next {
        EnterKey::Next => {
            |v| col().children([field("a", v).enter_key(EnterKey::Next), field("b", "")])
        }
        _ => |v| col().children([field("a", v), field("b", "")]),
    }
}

#[test]
fn next_moves_focus_to_the_next_field() {
    let mut f = Fixture::new(two_fields(EnterKey::Next));
    let (a, b) = (f.id("a"), f.id("b"));
    assert_eq!(f.focused(), Some(a));
    f.enter();
    assert_eq!(f.focused(), Some(b));
}

#[test]
fn on_key_takes_enter_before_next() {
    let mut f = Fixture::new(|v| {
        col().children([
            field("a", v)
                .enter_key(EnterKey::Next)
                .on_key(|k| (k.key == Key::Enter).then_some(Msg::Key)),
            field("b", ""),
        ])
    });
    let a = f.id("a");
    assert_eq!(f.enter(), vec![Msg::Key]);
    assert_eq!(f.focused(), Some(a), "the app took it");
}

#[test]
fn next_on_the_last_field_wraps_like_tab() {
    let mut f =
        Fixture::new(|v| col().children([field("a", v), field("b", "").enter_key(EnterKey::Next)]));
    let (a, b) = (f.id("a"), f.id("b"));
    f.send(InputEvent::Tab);
    assert_eq!(f.focused(), Some(b));
    f.enter();
    assert_eq!(f.focused(), Some(a));
}

#[test]
fn an_explicit_search_blurs_when_unhandled() {
    let mut f = Fixture::new(|v| col().children([field("a", v).enter_key(EnterKey::Search)]));
    f.enter();
    assert_eq!(f.focused(), None, "an explicit Search lets go");

    let mut f = Fixture::new(|v| {
        col().children([field("a", v)
            .enter_key(EnterKey::Search)
            .on_key(|k| (k.key == Key::Enter).then_some(Msg::Key))])
    });
    let a = f.id("a");
    f.enter();
    assert_eq!(f.focused(), Some(a), "handled: focus stays");

    let mut f = Fixture::new(|v| col().children([field("a", v)]));
    let a = f.id("a");
    f.enter();
    assert_eq!(f.focused(), Some(a), "a default field keeps focus");

    let mut f = Fixture::new(|v| col().children([field("a", v).enter_key(EnterKey::Send)]));
    let a = f.id("a");
    f.enter();
    assert_eq!(f.focused(), Some(a), "Send keeps focus");
}

#[test]
fn enter_while_composing_neither_moves_nor_blurs() {
    for key in [EnterKey::Next, EnterKey::Search] {
        let mut f = Fixture::new(match key {
            EnterKey::Next => {
                |v| col().children([field("a", v).enter_key(EnterKey::Next), field("b", "")])
            }
            _ => |v| col().children([field("a", v).enter_key(EnterKey::Search), field("b", "")]),
        });
        let a = f.id("a");
        f.send(InputEvent::ImePreedit {
            text: "nihao".to_owned(),
            cursor: Some((5, 5)),
        });
        f.enter();
        assert_eq!(f.focused(), Some(a), "{key:?}: composing");
    }
}
