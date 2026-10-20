//! A drag in a multiline editor selects across its lines.
//!
//! **The bug this locks.** The pointer was mapped into the editor with its y thrown away: every
//! press and every drag landed on whichever line sat at half the box's height, so a drag from the
//! first line to the last selected along one line only, and a click on any line put the caret on
//! the middle one.

use fenestra_core::{Element, Fonts, FrameState, InputEvent, Key, KeyInput, Theme, build_frame, col, dispatch, raw_text_area};

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Changed(String),
}

fn area(value: &str) -> Element<Msg> {
    col().children([raw_text_area(value.to_owned(), "type here")
        .w(200.0)
        .h(90.0)
        .on_input(|v| Msg::Changed(v.to_owned()))])
}

/// Presses at `from`, drags to `to`, lets go, and deletes what is selected; returns the value
/// the app is handed.
fn delete_dragged(value: &str, from: (f32, f32), to: (f32, f32)) -> Option<String> {
    let theme = Theme::light();
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();
    let v = area(value);
    let mut changed = None;
    for event in [
        InputEvent::PointerMove { x: from.0, y: from.1 },
        InputEvent::PointerDown,
        InputEvent::PointerMove { x: to.0, y: to.1 },
        InputEvent::PointerUp,
        InputEvent::Key(KeyInput::plain(Key::Backspace)),
    ] {
        let frame = build_frame(&v, &theme, &mut fonts, &mut state, (240.0, 120.0), 1.0);
        let out = dispatch(&v, &frame, &mut state, &mut fonts, event);
        for m in out.msgs {
            let Msg::Changed(s) = m;
            changed = Some(s);
        }
    }
    changed
}

#[test]
fn a_drag_from_the_first_line_to_the_last_selects_all_three() {
    let left = delete_dragged("one\ntwo\nthree", (3.0, 6.0), (199.0, 88.0));
    assert_eq!(left.as_deref(), Some(""), "the drag did not select across the lines");
}

#[test]
fn a_press_on_the_first_line_puts_the_caret_there() {
    // A press at the start of the first line and a drag to its end selects "one".
    let left = delete_dragged("one\ntwo\nthree", (3.0, 6.0), (199.0, 6.0));
    assert_eq!(left.as_deref(), Some("\ntwo\nthree"), "the press did not land on the first line");
}
