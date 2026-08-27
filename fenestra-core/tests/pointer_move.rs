//! `on_pointer_move`: following the pointer without a button held.
//!
//! `on_hover` says the pointer is somewhere over an element and
//! `on_drag_event` needs a press, so before this nothing could answer "where
//! is the pointer right now" — which is what anything highlighting whatever is
//! under the cursor has to know.

use fenestra_core::*;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    At(i32, i32),
}

fn view() -> Element<Msg> {
    col().w(200.0).h(200.0).p(40.0).child(
        row()
            .w(100.0)
            .h(100.0)
            .id("target")
            .on_pointer_move(|x, y| Some(Msg::At(x as i32, y as i32))),
    )
}

#[test]
fn the_pointer_is_reported_in_the_element_s_own_pixels() {
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();
    let el = view();
    let frame = build_frame(
        &el,
        &Theme::light(),
        &mut fonts,
        &mut state,
        (200.0, 200.0),
        1.0,
    );

    // Over the target, forty in from the padding: local (20, 20).
    let out = dispatch(
        &el,
        &frame,
        &mut state,
        &mut fonts,
        InputEvent::PointerMove { x: 60.0, y: 60.0 },
    );
    assert_eq!(
        out.msgs,
        vec![Msg::At(20, 20)],
        "the pointer was not reported in the element's own space"
    );

    // Outside it: the element is not under the pointer, so it hears nothing.
    let out = dispatch(
        &el,
        &frame,
        &mut state,
        &mut fonts,
        InputEvent::PointerMove { x: 5.0, y: 5.0 },
    );
    assert!(
        out.msgs.is_empty(),
        "an element heard about a pointer that was not over it: {:?}",
        out.msgs
    );
}
