//! A transparent, always-on-top, borderless window — the shape a screen-region
//! selector needs, and the one thing about it that **cannot** be tested
//! headlessly.
//!
//! The headless renderer never touches a swapchain, so nothing in the test
//! suite can say whether a transparent window is actually see-through on this
//! machine. That answer lives in one place: whether the adapter advertises a
//! compositing `CompositeAlphaMode` for the surface. This example asks it out
//! loud.
//!
//! ```sh
//! cargo run --example overlay
//! ```
//!
//! Press the button, then **look at what is inside the frame**:
//!
//! - **Your desktop shows through** — transparency works. This is the window a
//!   selection overlay is built on.
//! - **A black rectangle** — the swapchain fell back to an opaque alpha mode.
//!   The window is real and on top, but nothing behind it can be seen, so a
//!   selector would have to freeze the screen and paint the capture itself.
//!
//! Deliberately *not* fullscreen. Covering the screen would leave nothing to
//! compare against, and on macOS `Fullscreen::Borderless` opens a Space, which
//! is not what an overlay wants. A window placed over the desktop with the
//! desktop still visible around it answers the question with no ambiguity.
//!
//! Click the overlay, or press Escape, to dismiss it. The window that opened
//! it stays put — which is the point of a secondary window, and also the way
//! out if the overlay paints nothing you can see.

use fenestra::prelude::*;

#[derive(Default)]
struct Overlay {
    showing: bool,
}

#[derive(Clone)]
enum Msg {
    Show,
    Hide,
}

/// Big enough to judge, small enough to leave desktop visible around it.
const SIZE: (f64, f64) = (720.0, 460.0);

impl App for Overlay {
    type Msg = Msg;

    fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Show => self.showing = true,
            Msg::Hide => self.showing = false,
        }
    }

    fn windows(&self) -> Vec<WindowDesc<Msg>> {
        if !self.showing {
            return Vec::new();
        }
        vec![
            WindowDesc::new("overlay", "Overlay", SIZE, Msg::Hide)
                .borderless()
                .transparent()
                .always_on_top()
                .at(220.0, 180.0),
        ]
    }

    fn view_for(&self, key: &str) -> Element<Msg> {
        match key {
            "overlay" => self.overlay(),
            _ => self.launcher(),
        }
    }

    fn view(&self) -> Element<Msg> {
        self.launcher()
    }

    fn theme(&self) -> Theme {
        Theme::dark()
    }
}

impl Overlay {
    fn launcher(&self) -> Element<Msg> {
        col()
            .p(SP6)
            .gap(SP4)
            .items_center()
            .justify_center()
            .children([
                text("Transparent overlay check")
                    .size(TextSize::Xl)
                    .weight(Weight::Semibold),
                text("Open it, then look inside the frame: desktop means transparency works, black means the swapchain is opaque.")
                    .size(TextSize::Sm)
                    .themed(|t: &Theme, s| s.color(t.text_muted)),
                button("Show overlay").on_click(Msg::Show).into(),
            ])
    }

    /// The overlay paints a frame, a label, and **nothing else** — no
    /// background anywhere, so every unpainted pixel is the transparent clear.
    /// A scrim would be the honest thing for a real selector and the wrong
    /// thing here: dimmed-desktop and black are hard to tell apart, and telling
    /// them apart is the whole job.
    fn overlay(&self) -> Element<Msg> {
        col()
            .w_full()
            .h_full()
            .p(SP4)
            .items_center()
            .justify_center()
            .on_click(Msg::Hide)
            .on_key(|k| match k.key {
                Key::Escape => Some(Msg::Hide),
                _ => None,
            })
            .child(
                col()
                    .w_full()
                    .h_full()
                    .items_center()
                    .justify_center()
                    .gap(SP3)
                    .rounded(18.0)
                    .themed(|t: &Theme, s| s.border(2.0, t.accent))
                    .children([
                        text("Can you see your desktop in here?")
                            .size(TextSize::Lg)
                            .weight(Weight::Semibold)
                            .themed(|t: &Theme, s| s.color(t.accent)),
                        text("Yes — transparency works.  Black — the surface is opaque.")
                            .size(TextSize::Sm)
                            .themed(|t: &Theme, s| s.color(t.accent)),
                        text("Click anywhere, or press Escape, to close.")
                            .size(TextSize::Xs)
                            .themed(|t: &Theme, s| s.color(t.accent)),
                    ]),
            )
    }
}

fn main() {
    fenestra::run(
        Overlay::default(),
        WindowOptions::titled("Overlay check").with_size(560.0, 320.0),
    );
}
