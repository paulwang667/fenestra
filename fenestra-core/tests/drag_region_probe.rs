//! Minimal drag-region probe: builder flag -> frame -> query.

use fenestra_core::*;
use fenestra_core::{FrameState, Fonts, build_frame};

#[test]
fn frame_query_finds_full_window_region() {
    // Single flagged element covering the whole window: any point must hit.
    let view: Element<()> = col().w_full().h_full().drag_region();
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();
    let frame = build_frame(&view, &Theme::dark(), &mut fonts, &mut state, (800.0, 600.0), 1.0);
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
    let frame = build_frame(&view, &Theme::dark(), &mut fonts, &mut state, (800.0, 600.0), 1.0);
    assert!(
        frame.drag_region_at(kurbo::Point::new(400.0, 18.0)),
        "titled row region not found"
    );
}
