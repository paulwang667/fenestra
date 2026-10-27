//! Surface noise renders as fine monochrome grain on both backends: the
//! pixels vary about the surface colour, and the mean holds.

use fenestra_core::{Fonts, FrameState, Theme, build_frame, col, stack};
use fenestra_shell::{Backend, Headless};
use image::RgbaImage;

fn render(backend: Backend, noise: f32) -> RgbaImage {
    let theme = Theme::dark();
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();
    let surface: fenestra_core::Element<()> = col()
        .absolute()
        .left(20.0)
        .top(20.0)
        .w(160.0)
        .h(80.0)
        .bg(theme.surface_raised)
        .surface_noise(noise);
    let view = stack().w_full().h_full().bg(theme.bg).child(surface);
    let frame = build_frame(&view, &theme, &mut fonts, &mut state, (200.0, 120.0), 1.0);
    let mut h = Headless::with_backend(backend).expect("a GPU adapter");
    h.render_plan(&frame, &mut fonts, &mut state, 200, 120, theme.bg)
        .expect("render")
}

/// The surface's luma samples (inside it, clear of its edges).
fn samples(image: &RgbaImage) -> Vec<f64> {
    let mut out = Vec::new();
    for y in 30..90 {
        for x in 30..170 {
            let p = image.get_pixel(x, y).0;
            out.push(
                0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2]),
            );
        }
    }
    out
}

fn mean_and_spread(v: &[f64]) -> (f64, f64) {
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    let var = v.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / v.len() as f64;
    (mean, var.sqrt())
}

#[test]
fn surface_noise_is_grain_that_keeps_the_mean_on_both_backends() {
    for backend in [Backend::Vello, Backend::Raster] {
        let (plain_mean, plain_spread) = mean_and_spread(&samples(&render(backend, 0.0)));
        let (grain_mean, grain_spread) = mean_and_spread(&samples(&render(backend, 0.04)));
        assert!(
            plain_spread < 0.5,
            "{backend:?}: a flat surface ({plain_spread:.2})"
        );
        assert!(
            grain_spread > 2.0,
            "{backend:?}: grain shows ({grain_spread:.2})"
        );
        assert!(
            (grain_mean - plain_mean).abs() < 4.0,
            "{backend:?}: the mean holds ({plain_mean:.1} → {grain_mean:.1})"
        );
    }
}
