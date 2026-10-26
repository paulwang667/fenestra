//! A masked field (a password): the app still owns the value and still
//! receives every edit, but the value never reaches the pixels and never
//! reaches the accessibility tree.

use fenestra_core::{
    Element, Fonts, FrameState, InputEvent, Theme, build_frame, by, dispatch, raw_input,
};

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Value(String),
}

/// A masked field showing `value`.
fn masked(value: &str) -> Element<Msg> {
    raw_input(value, "")
        .masked(true)
        .id("pw")
        .w(200.0)
        .on_input(|s| Msg::Value(s.to_owned()))
}

/// An ordinary field showing `value` — the control the masked one is
/// measured against.
fn plain(value: &str) -> Element<Msg> {
    raw_input(value, "")
        .id("pw")
        .w(200.0)
        .on_input(|s| Msg::Value(s.to_owned()))
}

/// The byte range the field's editor holds selected (collapsed = the caret).
fn caret_of(frame: &fenestra_core::Frame) -> Option<(usize, usize)> {
    frame.get(&by::id("pw")).selection
}

/// Typing into a masked field hands the app the real value: the bullets on
/// screen are a mask, not the field's contents.
#[test]
fn typing_emits_the_real_value() {
    let theme = Theme::light();
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();
    let mut value = String::new();
    let steps = [
        InputEvent::Tab, // focus the field
        InputEvent::Text("h".into()),
        InputEvent::Text("u".into()),
        InputEvent::Text("n".into()),
        InputEvent::Text("t".into()),
    ];
    for ev in steps {
        let v = masked(&value);
        let frame = build_frame(&v, &theme, &mut fonts, &mut state, (240.0, 60.0), 1.0);
        let out = dispatch(&v, &frame, &mut state, &mut fonts, ev);
        for msg in out.msgs {
            let Msg::Value(v) = msg;
            value = v;
        }
    }
    assert_eq!(value, "hunt", "the app owns the value, not the bullets");
}

/// A masked field carries no value in the accessibility tree; an ordinary
/// one does. The tree is read by screen readers and written to snapshots,
/// so a password in it is a password on disk.
#[test]
fn a_masked_field_exposes_no_value() {
    let theme = Theme::light();
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();

    let v = masked("hunter2");
    let frame = build_frame(&v, &theme, &mut fonts, &mut state, (240.0, 60.0), 1.0);
    let node = frame.get(&by::id("pw"));
    assert!(node.value.is_none(), "a masked field exposes no value");

    let v = plain("hunter2");
    let frame = build_frame(&v, &theme, &mut fonts, &mut state, (240.0, 60.0), 1.0);
    let node = frame.get(&by::id("pw"));
    assert_eq!(node.value.as_deref(), Some("hunter2"));
}

/// A press in a masked field lands on a character of the *value*, not on a
/// byte of the bullet text. The bullets are one per character, so the
/// caret's byte offset is the value's own — the accent below is two bytes
/// and the bullet in its place is three.
#[test]
fn a_press_lands_on_a_character_of_the_value() {
    let theme = Theme::light();
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();
    // "héllo" — five characters, six bytes.
    let v = masked("héllo");
    let frame = build_frame(&v, &theme, &mut fonts, &mut state, (240.0, 60.0), 1.0);
    let rect = frame
        .rect_of(frame.get(&by::id("pw")).id)
        .expect("field rect");

    let mut press = |state: &mut FrameState, x: f32| {
        dispatch(
            &v,
            &frame,
            state,
            &mut fonts,
            InputEvent::PointerMove {
                x,
                y: rect.y0 as f32 + 2.0,
            },
        );
        dispatch(&v, &frame, state, &mut fonts, InputEvent::PointerDown);
        let frame = build_frame(&v, &theme, &mut fonts, state, (240.0, 60.0), 1.0);
        caret_of(&frame)
    };

    // Past the last bullet, but still inside the field.
    assert_eq!(
        press(&mut state, rect.x1 as f32 - 1.0),
        Some((6, 6)),
        "the caret lands after the last character — the value's own bytes, \
         not the bullets'"
    );

    // Past the double-click window, so the next press is a press and not
    // the second click of a pair.
    let now = state.now();
    state.tick(now + 1.0);

    // Before the first bullet.
    assert_eq!(
        press(&mut state, rect.x0 as f32 + 0.5),
        Some((0, 0)),
        "the caret lands before the first character"
    );
}
