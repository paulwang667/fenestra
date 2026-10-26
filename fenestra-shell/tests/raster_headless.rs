//! Headless rendering through the raster backend: the same frames as vello,
//! glass included (its backdrop rendered and read back by the raster
//! backend, then blurred on the CPU as before).

use fenestra_core::{
    Element, Fonts, FrameState, Material, R_LG, SP3, TextSize, Theme, build_frame, by, col,
    effects, image_rgba8, row, stack, text,
};
use fenestra_shell::{Backend, Headless};
use image::RgbaImage;

const LOGICAL: (f32, f32) = (360.0, 240.0);

fn view(t: &Theme, glass: bool) -> Element<()> {
    let pts = [
        (0.0, 0.0, t.accent),
        (1.0, 0.0, t.danger.solid),
        (0.0, 1.0, t.warning.solid),
        (1.0, 1.0, t.success.solid),
    ]
    .map(|(x, y, color)| effects::MeshPoint { x, y, color });
    let stripes = row().w_full().h(100.0).children(
        (0..36_usize)
            .map(|i| {
                col()
                    .w(10.0)
                    .h_full()
                    .bg(if i % 2 == 0 { t.text } else { t.bg })
            })
            .collect::<Vec<_>>(),
    );
    let body = col().w_full().h_full().gap(SP3).children([
        stripes,
        row().gap(SP3).children([
            image_rgba8(120, 80, effects::mesh(120, 80, &pts))
                .w(120.0)
                .h(80.0)
                .rounded(R_LG),
            text("Frosted glass, raster backend")
                .size(TextSize::Lg)
                .color(t.text),
        ]),
    ]);
    let mut pane = col()
        .id("pane")
        .absolute()
        .top(20.0)
        .left(40.0)
        .w(200.0)
        .h(60.0)
        .rounded(R_LG);
    if glass {
        pane = pane
            .bg(Material::new(0.55, 14.0, 1.5).tint(t.surface_raised))
            .backdrop_blur(10.0);
    }
    stack().w_full().h_full().bg(t.bg).children([body, pane])
}

fn render(backend: Backend, glass: bool, scale: f64) -> (RgbaImage, (u32, u32, u32, u32)) {
    let theme = Theme::light();
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();
    state.reduced_motion = true;
    let frame = build_frame(
        &view(&theme, glass),
        &theme,
        &mut fonts,
        &mut state,
        LOGICAL,
        scale,
    );
    let r = frame.get(&by::id("pane")).rect;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "test sizes"
    )]
    let (pw, ph) = (
        (f64::from(LOGICAL.0) * scale) as u32,
        (f64::from(LOGICAL.1) * scale) as u32,
    );
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "test sizes"
    )]
    let pane = (
        (r.x0 * scale) as u32 + 8,
        (r.y0 * scale) as u32 + 8,
        (r.width() * scale) as u32 - 16,
        (r.height() * scale) as u32 - 16,
    );
    let mut h = Headless::with_backend(backend).expect("a GPU adapter");
    assert_eq!(h.backend(), backend);
    let image = h
        .render_plan(&frame, &mut fonts, &mut state, pw, ph, theme.bg)
        .expect("render");
    (image, pane)
}

/// The fraction of pixels whose largest channel difference exceeds `channel`.
fn diff_frac(a: &RgbaImage, b: &RgbaImage, channel: u8) -> f64 {
    let over = a
        .pixels()
        .zip(b.pixels())
        .filter(|(p, q)| {
            p.0.iter()
                .zip(q.0.iter())
                .any(|(x, y)| x.abs_diff(*y) > channel)
        })
        .count();
    over as f64 / f64::from(a.width() * a.height())
}

/// Sum of horizontal neighbor differences inside `rect`: high for stripes,
/// low once they are blurred.
fn energy(image: &RgbaImage, (x, y, w, h): (u32, u32, u32, u32)) -> u64 {
    let mut e = 0;
    for yy in y..y + h {
        for xx in x..x + w - 1 {
            let (a, b) = (image.get_pixel(xx, yy).0, image.get_pixel(xx + 1, yy).0);
            e += a
                .iter()
                .zip(b.iter())
                .map(|(p, q)| u64::from(p.abs_diff(*q)))
                .sum::<u64>();
        }
    }
    e
}

#[test]
fn glass_renders_through_raster() {
    let (vello, _) = render(Backend::Vello, true, 1.0);
    let (raster, pane) = render(Backend::Raster, true, 1.0);
    let frac = diff_frac(&vello, &raster, 24);
    assert!(
        frac <= 0.03,
        "{:.2}% of pixels differ by more than 24/255",
        frac * 100.0
    );
    let (sharp, _) = render(Backend::Raster, false, 1.0);
    assert!(
        energy(&raster, pane) * 2 < energy(&sharp, pane),
        "the stripes under the pane are blurred"
    );
}

#[test]
fn hidpi_raster_matches_vello() {
    let (vello, _) = render(Backend::Vello, true, 2.0);
    let (raster, _) = render(Backend::Raster, true, 2.0);
    let frac = diff_frac(&vello, &raster, 24);
    assert!(
        frac <= 0.03,
        "{:.2}% of pixels differ by more than 24/255",
        frac * 100.0
    );
}
