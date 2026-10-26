//! What happens to an app's keyboard while an input method is composing.
//!
//! **Nothing had ever exercised this.** For a writer of Chinese, Japanese,
//! Korean or Vietnamese, every character arrives through a composition, so
//! this is not an edge: it is the ordinary path, and it was broken two ways
//! at once.

use fenestra_core::{
    Element, Fonts, FrameState, InputEvent, Key, KeyInput, Theme, build_frame, by, col, dispatch,
    raw_text_area,
};

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Input(String),
    Sent,
    Pressed,
}

fn view(value: &str) -> Element<Msg> {
    col().children([
        raw_text_area(value.to_owned(), "type here")
            .id("field")
            // As a chat composer is: the editor leaves Enter to the app
            // instead of inserting a newline, which is what puts the key in
            // reach of the binding below.
            .submit_on_enter(true)
            .w(200.0)
            .h(80.0)
            .on_input(|v| Msg::Input(v.to_owned()))
            // What a chat composer binds, and what accepting a candidate
            // presses.
            .on_key(|k| match k.key {
                Key::Enter if !k.shift => Some(Msg::Sent),
                _ => None,
            }),
        // Somewhere else to click, as a composer's send button is.
        col()
            .id("elsewhere")
            .w(200.0)
            .h(32.0)
            .focusable(true)
            .on_click(Msg::Pressed),
    ])
}

struct Fixture {
    theme: Theme,
    fonts: Fonts,
    state: FrameState,
}

impl Fixture {
    fn focused() -> Self {
        let mut f = Self {
            theme: Theme::light(),
            fonts: Fonts::embedded(),
            state: FrameState::new(),
        };
        f.send("", InputEvent::Tab);
        f
    }

    fn send(&mut self, value: &str, event: InputEvent) -> Vec<Msg> {
        let v = view(value);
        let frame = build_frame(
            &v,
            &self.theme,
            &mut self.fonts,
            &mut self.state,
            (240.0, 120.0),
            1.0,
        );
        dispatch(&v, &frame, &mut self.state, &mut self.fonts, event).msgs
    }

    /// One redraw, which is what a runner does after any event.
    fn draw(&mut self, value: &str) {
        let v = view(value);
        build_frame(
            &v,
            &self.theme,
            &mut self.fonts,
            &mut self.state,
            (240.0, 120.0),
            1.0,
        );
    }

    fn compose(&mut self, value: &str) -> Vec<Msg> {
        self.send(
            value,
            InputEvent::ImePreedit {
                text: "nihao".to_owned(),
                cursor: Some((5, 5)),
            },
        )
    }

    /// A full click at the centre of the element with this id.
    fn click(&mut self, value: &str, id: &str) -> Vec<Msg> {
        let v = view(value);
        let frame = build_frame(
            &v,
            &self.theme,
            &mut self.fonts,
            &mut self.state,
            (240.0, 120.0),
            1.0,
        );
        let at = frame.get(&by::id(id)).rect.center();
        let mut msgs = self.send(
            value,
            InputEvent::PointerMove {
                x: at.x as f32,
                y: at.y as f32,
            },
        );
        msgs.extend(self.send(value, InputEvent::PointerDown));
        msgs.extend(self.send(value, InputEvent::PointerUp));
        msgs
    }

    fn composing(&self) -> bool {
        self.state
            .composing_in(self.state.focused().expect("focused"))
    }
}

/// **A composition has to survive being drawn.**
///
/// A preedit raises no `on_input` — half-typed pinyin is not the field's
/// value — so the app's value stays behind the editor's text and the two can
/// never match while composing. `sync` compared them anyway and called
/// `set_text`, which drops the composition: it did not survive one redraw,
/// and a redraw is exactly what follows the preedit that started it.
#[test]
fn a_composition_survives_the_redraw_that_follows_it() {
    let mut f = Fixture::focused();
    let echoed = f.compose("");
    assert!(f.composing(), "the preedit did not start a composition");
    assert_eq!(echoed, vec![], "a preedit is not the field's value");

    f.draw("");

    assert!(
        f.composing(),
        "one redraw and the composition was gone; every character a CJK \
         writer types would vanish as it was typed"
    );
}

/// **And the keyboard is the input method's while it lasts.**
///
/// Enter accepts a candidate. An app that binds Enter — every chat composer
/// does — would send on that key, taking the half-composed word with it.
#[test]
fn an_apps_key_binding_does_not_fire_while_composing() {
    let mut f = Fixture::focused();
    f.compose("明天见");
    f.draw("明天见");

    let msgs = f.send("明天见", InputEvent::Key(KeyInput::plain(Key::Enter)));

    assert_eq!(
        msgs,
        vec![],
        "Enter reached the app while a candidate was being accepted"
    );
}

/// It comes back when the composition ends, or the binding is gone for good.
#[test]
fn the_binding_comes_back_when_the_composition_ends() {
    let mut f = Fixture::focused();
    f.compose("明天见");
    f.draw("明天见");
    f.send(
        "明天见",
        InputEvent::ImePreedit {
            text: String::new(),
            cursor: None,
        },
    );
    f.draw("明天见");

    let msgs = f.send("明天见", InputEvent::Key(KeyInput::plain(Key::Enter)));

    assert_eq!(msgs, vec![Msg::Sent], "Enter never came back");
}

/// **A composition does not outlive the focus it was typed into.**
///
/// A click elsewhere mid-composition moves focus inside the window, which the
/// platform never hears about, so the preedit that would end the composition
/// goes to whatever has focus next. The editor left behind kept its preedit,
/// and with the keyboard the input method's while it composes, a click back
/// found a field where Backspace, the arrows and Tab did nothing.
#[test]
fn clicking_away_mid_composition_drops_it() {
    let mut f = Fixture::focused();
    f.compose("明天见");
    f.draw("明天见");

    f.click("明天见", "elsewhere");
    f.click("明天见", "field");

    assert!(!f.composing(), "the composition outlived its focus");
    assert_eq!(
        f.send("明天见", InputEvent::Key(KeyInput::plain(Key::Backspace))),
        vec![Msg::Input("明天".to_owned())],
        "Backspace did nothing, or deleted into a stale preedit"
    );
}
