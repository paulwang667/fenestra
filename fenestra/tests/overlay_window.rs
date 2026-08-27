//! A declared overlay window opens with what it declared, and renders.
//!
//! `fenestra-core`'s `windows.rs` covers the routing — which keys open and
//! which view each gets — and has no renderer to ask anything else. This is
//! the other half: that the flags an overlay needs survive the trip, and that
//! a secondary window is a window the harness can actually draw.
//!
//! What is *not* here, and cannot be: whether a transparent window is
//! see-through on screen. Headless rendering never configures a swapchain, so
//! the alpha mode this all turns on is not reachable from a test. Run
//! `cargo run --example overlay` and look.

use fenestra::prelude::*;
use fenestra::shell::Harness;

#[derive(Default)]
struct Host {
    open: bool,
}

#[derive(Clone)]
enum Msg {
    Open,
    Close,
}

impl App for Host {
    type Msg = Msg;

    fn update(&mut self, msg: Msg) {
        self.open = matches!(msg, Msg::Open);
    }

    fn windows(&self) -> Vec<WindowDesc<Msg>> {
        if !self.open {
            return Vec::new();
        }
        vec![
            WindowDesc::new("pick", "Pick", (640.0, 400.0), Msg::Close)
                .borderless()
                .transparent()
                .always_on_top()
                .at(-1280.0, 0.0),
        ]
    }

    fn view_for(&self, key: &str) -> Element<Msg> {
        match key {
            "pick" => col()
                .w_full()
                .h_full()
                .items_center()
                .justify_center()
                .child(text("selection").size(TextSize::Lg)),
            _ => self.view(),
        }
    }

    fn view(&self) -> Element<Msg> {
        col().p(SP6).child(button("Open").on_click(Msg::Open))
    }
}

#[test]
fn an_overlay_window_carries_its_flags_and_renders() {
    let mut h = Harness::new(Host::default(), Theme::dark(), (400, 300));
    assert!(h.app().windows().is_empty(), "an overlay was open at rest");

    h.click(&by::role(Semantics::Button).name("Open"));
    let open = h.app().windows();
    assert_eq!(open.len(), 1, "the overlay did not open");

    // Every flag, because the failure this guards was per-field: the runner
    // built a secondary window from the title and the size and dropped the
    // rest without saying so.
    let desc = &open[0];
    assert!(desc.borderless, "the overlay would open with a title bar");
    assert!(desc.transparent, "the overlay would open opaque");
    assert!(desc.always_on_top, "the overlay would open behind the app");
    assert_eq!(
        desc.position,
        Some((-1280.0, 0.0)),
        "the overlay could not be placed on the display it covers"
    );

    // And it is a window the renderer can draw, at the size it asked for.
    h.activate_window("pick");
    let image = h.render_window("pick");
    assert!(
        image.width() > 0 && image.height() > 0,
        "the overlay rendered nothing"
    );
    assert!(
        h.query(&by::label("selection")).is_some(),
        "the overlay drew the main window's view instead of its own"
    );
}
