//! Swiper navigation: swipe flicks, arrow keys, clamping, and the page
//! label — driven through the real dispatch pipeline.

use fenestra_core::{
    Element, Fonts, FrameState, InputEvent, Key, KeyInput, Theme, build_frame, col, dispatch, text,
};
use fenestra_kit::swiper;

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Page(usize),
}

const SIZE: (f32, f32) = (320.0, 160.0);

fn view(current: usize) -> Element<Msg> {
    col().p(16.0).child(
        swiper(current)
            .page(col().w(280.0).h(100.0).p(24.0).child(text("First")))
            .page(col().w(280.0).h(100.0).p(24.0).child(text("Second")))
            .page(col().w(280.0).h(100.0).p(24.0).child(text("Third")))
            .on_change(Msg::Page)
            .id("sw"),
    )
}

/// Drives events through freshly built frames (state persists across steps,
/// so focus/press survive), collecting emitted messages.
fn drive(steps: &[(Element<Msg>, InputEvent)]) -> Vec<Msg> {
    let theme = Theme::light();
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();
    state.reduced_motion = true;
    let mut msgs = Vec::new();
    for (v, ev) in steps {
        let frame = build_frame(v, &theme, &mut fonts, &mut state, SIZE, 1.0);
        msgs.extend(dispatch(v, &frame, &mut state, &mut fonts, ev.clone()).msgs);
    }
    msgs
}

fn flick(from_x: f32, to_x: f32, y: f32) -> [InputEvent; 4] {
    [
        InputEvent::PointerMove { x: from_x, y },
        InputEvent::PointerDown,
        InputEvent::PointerMove { x: to_x, y },
        InputEvent::PointerUp,
    ]
}

/// A left flick (finger travels −x) advances; a right flick goes back.
#[test]
fn swipe_flicks_navigate() {
    let msgs = drive(&[
        (view(0), flick(200.0, 80.0, 60.0)[0].clone()),
        (view(0), flick(200.0, 80.0, 60.0)[1].clone()),
        (view(0), flick(200.0, 80.0, 60.0)[2].clone()),
        (view(0), flick(200.0, 80.0, 60.0)[3].clone()),
    ]);
    assert_eq!(msgs, vec![Msg::Page(1)]);

    // From page 0, a right flick has nowhere to go: no message.
    let msgs = drive(&[
        (view(0), flick(80.0, 200.0, 60.0)[0].clone()),
        (view(0), flick(80.0, 200.0, 60.0)[1].clone()),
        (view(0), flick(80.0, 200.0, 60.0)[2].clone()),
        (view(0), flick(80.0, 200.0, 60.0)[3].clone()),
    ]);
    assert!(msgs.is_empty());
}

/// Arrow keys step (clamped at the ends), Home/End jump, and the container
/// announces "Page N of M".
#[test]
fn arrows_step_and_label_announces() {
    let key = |current: usize, k: Key| (view(current), InputEvent::Key(KeyInput::plain(k)));
    let tap = |current: usize| (view(current), flick(160.0, 159.0, 60.0)[3].clone());
    // The tap focuses the container (a 1px press is below the swipe floor
    // and the container has no click, so it emits nothing). The drive does
    // not echo state back, so each step pins the view it asserts against.
    let msgs = drive(&[
        (view(1), InputEvent::PointerMove { x: 160.0, y: 60.0 }),
        (view(1), InputEvent::PointerDown),
        tap(1),
        key(1, Key::ArrowRight), // 1 → 2
        key(2, Key::ArrowRight), // clamped at the last page
        key(2, Key::Home),       // → 0
        key(2, Key::End),        // → 2
        key(2, Key::ArrowLeft),  // → 1
    ]);
    assert_eq!(
        msgs,
        vec![Msg::Page(2), Msg::Page(0), Msg::Page(2), Msg::Page(1)],
        "right; clamp; home; end; left"
    );
    let theme = Theme::light();
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();
    let frame = build_frame(&view(1), &theme, &mut fonts, &mut state, SIZE, 1.0);
    let yaml = frame.access_yaml();
    assert!(yaml.contains("Page 2 of 3"), "{yaml}");
}
