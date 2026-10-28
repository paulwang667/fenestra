//! The iOS safe area: the part of the screen clear of the status bar, the
//! Dynamic Island and the home indicator.
//!
//! The surface covers the whole screen; the shell wraps the main window's
//! view in a root padded by the safe area ([`inset_root`]) and painted with
//! the view's own background, so the screen reads as one surface while
//! nothing interactive sits under the system's chrome. Layout, paint, glass
//! and input all keep working in whole-screen coordinates.

use fenestra_core::{Color, Element, col};

/// Safe-area insets in logical pixels; zero everywhere but iOS.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Insets {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Insets {
    /// `(width, height)` with the insets taken off.
    pub fn inside(self, (w, h): (f32, f32)) -> (f32, f32) {
        (
            (w - self.left - self.right).max(0.0),
            (h - self.top - self.bottom).max(0.0),
        )
    }
}

impl From<Insets> for kurbo::Insets {
    fn from(i: Insets) -> Self {
        Self::new(
            f64::from(i.left),
            f64::from(i.top),
            f64::from(i.right),
            f64::from(i.bottom),
        )
    }
}

/// The insets of a safe area at physical `inner_pos` sized `inner`, inside a
/// physical `outer` screen, in logical pixels at `scale`.
#[expect(clippy::cast_possible_truncation, reason = "screen sizes fit in f32")]
#[cfg_attr(
    not(any(target_os = "ios", test)),
    expect(dead_code, reason = "only iOS has a safe area")
)]
pub(crate) fn insets_from(
    outer: (u32, u32),
    inner_pos: (i32, i32),
    inner: (u32, u32),
    scale: f64,
) -> Insets {
    let logical = |px: f64| (px / scale).max(0.0) as f32;
    let (left, top) = (f64::from(inner_pos.0), f64::from(inner_pos.1));
    Insets {
        top: logical(top),
        left: logical(left),
        right: logical(f64::from(outer.0) - left - f64::from(inner.0)),
        bottom: logical(f64::from(outer.1) - top - f64::from(inner.1)),
    }
}

/// The window's safe-area insets. winit on iOS reports the safe area as the
/// inner position and size, and the whole screen as the outer size.
#[cfg(target_os = "ios")]
pub(crate) fn window_insets(window: &winit::window::Window) -> Insets {
    let outer = window.outer_size();
    let inner = window.inner_size();
    let pos = window.inner_position().unwrap_or_default();
    insets_from(
        (outer.width, outer.height),
        (pos.x, pos.y),
        (inner.width, inner.height),
        window.scale_factor(),
    )
}

/// The window's safe-area insets: none outside iOS.
#[cfg(not(target_os = "ios"))]
pub(crate) fn window_insets(_window: &winit::window::Window) -> Insets {
    Insets::default()
}

/// The size a window's surface takes: the whole screen on iOS (where
/// `inner_size` is only the safe area), the inner size elsewhere.
#[cfg_attr(
    target_arch = "wasm32",
    expect(dead_code, reason = "the web sizes its surface when setup lands")
)]
pub(crate) fn surface_size(window: &winit::window::Window) -> winit::dpi::PhysicalSize<u32> {
    if cfg!(target_os = "ios") {
        window.outer_size()
    } else {
        window.inner_size()
    }
}

/// `view` padded by `insets`, on a root painted with the view's own
/// background (`fallback_bg` when it has none) so the bars' area matches.
/// Zero insets return `view` untouched.
pub(crate) fn inset_root<M>(view: Element<M>, insets: Insets, fallback_bg: Color) -> Element<M> {
    if insets == Insets::default() {
        return view;
    }
    let bg = view
        .style()
        .fill
        .clone()
        .unwrap_or_else(|| fallback_bg.into());
    col()
        .w_full()
        .h_full()
        .bg(bg)
        .pt(insets.top)
        .pr(insets.right)
        .pb(insets.bottom)
        .pl(insets.left)
        .child(view)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Harness;
    use fenestra_core::{App, Color, Element, Theme, by, col, text};

    const RED: Color = Color::new([1.0, 0.0, 0.0, 1.0]);
    const BLUE: Color = Color::new([0.0, 0.0, 1.0, 1.0]);
    const NOTCHED: Insets = Insets {
        top: 62.0,
        right: 0.0,
        bottom: 34.0,
        left: 0.0,
    };

    struct Screen {
        insets: Insets,
        painted: bool,
    }

    impl App for Screen {
        type Msg = ();
        fn update(&mut self, (): ()) {}
        fn view(&self) -> Element<()> {
            let page = col().w_full().h_full().child(text("hi"));
            let page = if self.painted { page.bg(RED) } else { page };
            inset_root(page, self.insets, BLUE)
        }
    }

    fn harness(insets: Insets, painted: bool) -> Harness<Screen> {
        Harness::new(Screen { insets, painted }, Theme::light(), (402, 874))
    }

    fn pixel(img: &image::RgbaImage, x: u32, y: u32) -> [u8; 4] {
        img.get_pixel(x, y).0
    }

    #[test]
    fn insets_carry_over_to_the_frame_state() {
        assert_eq!(
            kurbo::Insets::from(NOTCHED),
            kurbo::Insets::new(0.0, 62.0, 0.0, 34.0)
        );
    }

    #[test]
    fn no_safe_area_is_zero_insets() {
        assert_eq!(
            insets_from((1206, 2622), (0, 0), (1206, 2622), 3.0),
            Insets::default()
        );
    }

    #[test]
    fn a_notched_phone_insets_top_and_bottom() {
        // iPhone 17: 62pt on top, 34pt at the bottom, @3x.
        let i = insets_from((1206, 2622), (0, 186), (1206, 2622 - 186 - 102), 3.0);
        assert_eq!(i, NOTCHED);
    }

    #[test]
    fn the_view_sits_inside_the_insets_and_the_root_fills_the_screen() {
        let mut h = harness(NOTCHED, true);
        assert_eq!(h.get(&by::label("hi")).rect.y0, 62.0);
        let img = h.render();
        assert_eq!(pixel(&img, 200, 10), [255, 0, 0, 255], "status bar area");
        assert_eq!(
            pixel(&img, 200, 870),
            [255, 0, 0, 255],
            "home indicator area"
        );
    }

    #[test]
    fn a_view_without_background_falls_back_to_the_theme() {
        let img = harness(NOTCHED, false).render();
        assert_eq!(pixel(&img, 200, 10), [0, 0, 255, 255]);
    }

    #[test]
    fn zero_insets_leave_the_view_alone() {
        let mut h = harness(Insets::default(), false);
        assert_eq!(h.get(&by::label("hi")).rect.y0, 0.0);
        let img = h.render();
        assert_ne!(pixel(&img, 200, 10), [0, 0, 255, 255], "no fallback fill");
    }
}
