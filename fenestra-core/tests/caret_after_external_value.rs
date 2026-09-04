//! An app that sets a multiline editor's value gets the caret left after the
//! text, so the next Down is handed back to it.
//!
//! **The bug this locks.** `set_text` puts the caret at position 0. Down from
//! the start of a one-line value *moves*, so the editor reported MOVED and
//! kept the key — and a composer that recalls what it sent could walk up and
//! never come back, taking the half-typed draft with it. Nothing caught it:
//! the recall's own tests call the list directly and never press a key.
//!
//! Single-line editors keep the old landing on purpose, and `otp.rs` is
//! both the reason and the guard: it overwrites in place by having the
//! committed character inserted at the front. There is no assertion about
//! them here, because a single-line editor has no ArrowDown arm at all — it
//! hands the key back whatever the caret is doing, so a Down test would say
//! nothing about the caret.

use fenestra_core::{
    Element, Fonts, FrameState, InputEvent, Key, KeyInput, Theme, build_frame, col, dispatch,
    raw_text_area,
};

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    WentDown,
}

fn area(value: &str) -> Element<Msg> {
    col().children([raw_text_area(value.to_owned(), "type here")
        .w(200.0)
        .h(80.0)
        .on_key(|k| match k.key {
            Key::ArrowDown if !k.shift => Some(Msg::WentDown),
            _ => None,
        })])
}

/// Sets a value from outside, then presses Down. The app must see it.
fn down_reaches_the_app(view: fn(&str) -> Element<Msg>) -> bool {
    let theme = Theme::light();
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();

    // A frame with an empty box, focused.
    let v = view("");
    let frame = build_frame(&v, &theme, &mut fonts, &mut state, (240.0, 120.0), 1.0);
    dispatch(&v, &frame, &mut state, &mut fonts, InputEvent::Tab);

    // The app puts a value in it — a recalled message, a taken suggestion.
    let v = view("已经发过的一句");
    let frame = build_frame(&v, &theme, &mut fonts, &mut state, (240.0, 120.0), 1.0);
    let out = dispatch(
        &v,
        &frame,
        &mut state,
        &mut fonts,
        InputEvent::Key(KeyInput::plain(Key::ArrowDown)),
    );
    out.msgs.contains(&Msg::WentDown)
}

#[test]
fn a_multiline_editor_hands_down_back_after_the_app_set_its_value() {
    assert!(
        down_reaches_the_app(area),
        "the caret was left before the text, so the editor kept Down"
    );
}
