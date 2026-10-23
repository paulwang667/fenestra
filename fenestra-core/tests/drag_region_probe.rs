//! Minimal drag-region probe: builder flag -> frame -> query.

use fenestra_core::*;
use fenestra_core::{Fonts, FrameState, build_frame};

#[test]
fn frame_query_finds_full_window_region() {
    // Single flagged element covering the whole window: any point must hit.
    let view: Element<()> = col().w_full().h_full().drag_region();
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();
    let frame = build_frame(
        &view,
        &Theme::dark(),
        &mut fonts,
        &mut state,
        (800.0, 600.0),
        1.0,
    );
    assert!(
        frame.drag_region_at(kurbo::Point::new(400.0, 300.0)),
        "full-window drag region not found at center"
    );
}

#[test]
fn frame_query_finds_titled_row() {
    let view: Element<()> = col().children([
        row().h(36.0).w_full().drag_region(),
        col().grow().child(text("body")),
    ]);
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();
    let frame = build_frame(
        &view,
        &Theme::dark(),
        &mut fonts,
        &mut state,
        (800.0, 600.0),
        1.0,
    );
    assert!(
        frame.drag_region_at(kurbo::Point::new(400.0, 18.0)),
        "titled row region not found"
    );
}

/// **A control on a title bar is a control, not a handle.** The region's own
/// surface and its plain text drag the window; a button on it, a clickable
/// box nested a level deeper, and a text field must take the press instead —
/// a press on the theme toggle in a borderless title bar dragged the window.
#[test]
fn controls_inside_a_drag_region_take_the_press() {
    #[derive(Clone)]
    enum Msg {
        Clicked,
        Typed,
    }
    let view: Element<Msg> = col().w_full().h_full().children([
        row().id("bar").h(36.0).w_full().drag_region().children([
            text("Title").w(100.0),
            div()
                .id("button")
                .w(24.0)
                .h(24.0)
                .focusable(true)
                .on_click(Msg::Clicked),
            row()
                .w(80.0)
                .h(36.0)
                .child(div().id("nested").w(24.0).h(24.0).on_click(Msg::Clicked)),
            div()
                .id("close")
                .w(12.0)
                .h(12.0)
                .window_control(WindowControl::Close),
            raw_input("", "")
                .id("field")
                .w(120.0)
                .on_input(|_| Msg::Typed),
        ]),
        col().grow().child(text("body")),
    ]);
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();
    let frame = build_frame(
        &view,
        &Theme::dark(),
        &mut fonts,
        &mut state,
        (800.0, 600.0),
        1.0,
    );
    let center = |key: &str| frame.get(&by::id(key)).rect.center();

    assert!(
        frame.drag_region_at(kurbo::Point::new(20.0, 18.0)),
        "the title text drags"
    );
    assert!(
        frame.drag_region_at(kurbo::Point::new(700.0, 18.0)),
        "empty bar drags"
    );
    assert!(
        !frame.drag_region_at(center("button")),
        "a button takes its press"
    );
    assert!(
        !frame.drag_region_at(center("nested")),
        "a nested clickable takes its press"
    );
    assert!(
        !frame.drag_region_at(center("field")),
        "a text field takes its press"
    );
    assert!(
        !frame.drag_region_at(center("close")),
        "a window control takes its press"
    );
    assert_eq!(
        frame.window_control_at(center("close")),
        Some(WindowControl::Close)
    );
    assert!(
        !frame.drag_region_at(kurbo::Point::new(400.0, 300.0)),
        "the body is not a region"
    );
}
