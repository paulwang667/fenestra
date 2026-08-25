//! OTP auto-advance: a capped input inside a roving scope hands focus to
//! the next candidate when a commit fills the cap, and rejects further
//! edits at the cap.

use fenestra_core::{
    Element, Fonts, FrameState, InputEvent, Key, KeyInput, RovingAxis, Theme, build_frame, col,
    dispatch, raw_input,
};

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Code(String),
}

/// Three capped single-character boxes in a horizontal roving scope.
fn view(code: &str) -> Element<Msg> {
    col().children([col()
        .focusable(true)
        .roving(RovingAxis::Horizontal)
        .children(
            (0..3)
                .map(|i| {
                    let ch = code.chars().nth(i);
                    let code = code.to_string();
                    let mut b = raw_input(ch.map(|c| c.to_string()).unwrap_or_default(), "")
                        .max_chars(Some(1))
                        .w(44.0)
                        .on_input(move |s| {
                            let mut next: Vec<char> = code.chars().collect();
                            match s.chars().next() {
                                Some(c) if i < next.len() => next[i] = c,
                                Some(c) => next.push(c),
                                None => next.truncate(i),
                            }
                            Msg::Code(next.into_iter().collect())
                        });
                    if ch.is_some() {
                        b = b.read_only(false);
                    }
                    b
                })
                .collect::<Vec<_>>(),
        )])
}

/// Typing fills box after box: each saturated commit advances focus to the
/// next candidate and emits the reassembled code. A saturated box rejects
/// further characters (no message).
#[test]
fn fill_advances_and_cap_rejects() {
    let theme = Theme::light();
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();
    let mut code = String::new();
    let type_char = |c: char| InputEvent::Text(c.to_string());
    let steps = [
        InputEvent::Tab,                                   // → scope container
        InputEvent::Key(KeyInput::plain(Key::ArrowRight)), // → box 0
        type_char('4'),
        type_char('2'),
        type_char('7'),
        type_char('9'),
    ];
    for ev in steps {
        let v = view(&code);
        let frame = build_frame(&v, &theme, &mut fonts, &mut state, (240.0, 120.0), 1.0);
        let out = dispatch(&v, &frame, &mut state, &mut fonts, ev);
        for msg in out.msgs {
            let Msg::Code(c) = msg;
            code = c;
        }
        assert!(out.redraw);
    }
    assert_eq!(
        code, "427",
        "the fourth character must be rejected at the cap"
    );
}

/// Overwriting a filled box replaces in place and still advances.
#[test]
fn overwrite_replaces_and_advances() {
    let theme = Theme::light();
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();
    let mut code = String::from("4");
    let steps = [
        InputEvent::Tab,                                   // → scope container
        InputEvent::Key(KeyInput::plain(Key::ArrowRight)), // → box 0
        InputEvent::Text("9".into()),
    ];
    for ev in steps {
        let v = view(&code);
        let frame = build_frame(&v, &theme, &mut fonts, &mut state, (240.0, 120.0), 1.0);
        let out = dispatch(&v, &frame, &mut state, &mut fonts, ev);
        for msg in out.msgs {
            let Msg::Code(c) = msg;
            code = c;
        }
    }
    assert_eq!(code, "9");
}
