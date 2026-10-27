//! Headless rendering through the raster backend: the same frames as vello,
//! glass included (blurred and composited on the GPU, with no readback; the
//! old read-back-and-blur-on-the-CPU path stays behind `set_gpu_glass(false)`
//! as the reference).

use fenestra_core::{
    Element, ElementFilter, Fonts, FrameState, Material, R_LG, SP3, TextSize, Theme, build_frame,
    by, col, effects, image_rgba8, row, stack, text,
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

/// Renders `view` through the raster backend with GPU glass on or off;
/// returns the image and how many backdrop regions were read back.
fn raster(view: &Element<()>, gpu_glass: bool) -> (RgbaImage, u32) {
    let theme = Theme::light();
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();
    state.reduced_motion = true;
    let frame = build_frame(view, &theme, &mut fonts, &mut state, LOGICAL, 1.0);
    let mut h = Headless::with_backend(Backend::Raster).expect("a GPU adapter");
    h.set_gpu_glass(gpu_glass);
    let before = h.backdrop_reads();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "test sizes"
    )]
    let image = h
        .render_plan(
            &frame,
            &mut fonts,
            &mut state,
            LOGICAL.0 as u32,
            LOGICAL.1 as u32,
            theme.bg,
        )
        .expect("render");
    (image, h.backdrop_reads() - before)
}

/// Full-width horizontal bands, `period` px each, dark and light.
fn bands(t: &Theme, period: f32) -> Element<()> {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "test sizes"
    )]
    let n = (LOGICAL.1 / period) as usize;
    col().w_full().h_full().children(
        (0..n)
            .map(|i| {
                row()
                    .w_full()
                    .h(period)
                    .bg(if i % 2 == 0 { t.text } else { t.bg })
            })
            .collect::<Vec<_>>(),
    )
}

fn glass_pane(id: &str, left: f32, top: f32, w: f32, h: f32) -> Element<()> {
    let t = Theme::light();
    col()
        .id(id)
        .absolute()
        .left(left)
        .top(top)
        .w(w)
        .h(h)
        .rounded(R_LG)
        .bg(Material::new(0.55, 14.0, 1.5).tint(t.surface_raised))
        .backdrop_blur(10.0)
}

fn assert_like_cpu(gpu: &RgbaImage, cpu: &RgbaImage) {
    let frac = diff_frac(gpu, cpu, 8);
    assert!(
        frac <= 0.02,
        "{:.2}% of pixels differ from the CPU path by more than 8/255",
        frac * 100.0
    );
}

#[test]
fn raster_glass_reads_nothing_back() {
    let v = view(&Theme::light(), true);
    let (gpu, reads) = raster(&v, true);
    assert_eq!(reads, 0, "GPU glass reads no backdrop back");
    let (cpu, cpu_reads) = raster(&v, false);
    assert_eq!(cpu_reads, 1, "the CPU path reads the backdrop once");
    assert_like_cpu(&gpu, &cpu);
}

#[test]
fn a_pane_partly_off_screen_is_not_stretched() {
    let t = Theme::light();
    // A pane with its top 40% above the frame, no tint or sheen, a slight
    // blur: the bands inside its visible part must sit where they sit in the
    // backdrop. (The CPU path stretches the visible slice over the pane.)
    let pane = col()
        .id("pane")
        .absolute()
        .left(40.0)
        .top(-40.0)
        .w(200.0)
        .h(100.0)
        .backdrop_blur(1.0);
    let glass = stack()
        .w_full()
        .h_full()
        .bg(t.bg)
        .children([bands(&t, 8.0), pane]);
    let (image, _) = raster(&glass, true);
    let (sharp, _) = raster(
        &stack().w_full().h_full().bg(t.bg).child(bands(&t, 8.0)),
        true,
    );
    let x = 140;
    let rows = 2..58_u32;
    let profile = |img: &RgbaImage| -> Vec<f64> {
        rows.clone()
            .map(|y| f64::from(img.get_pixel(x, y).0[0]))
            .collect()
    };
    let (a, b) = (profile(&image), profile(&sharp));
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let (ma, mb) = (mean(&a), mean(&b));
    let cov: f64 = a.iter().zip(&b).map(|(p, q)| (p - ma) * (q - mb)).sum();
    let var = |v: &[f64], m: f64| v.iter().map(|p| (p - m) * (p - m)).sum::<f64>();
    let corr = cov / (var(&a, ma) * var(&b, mb)).sqrt();
    // The blur softens each band's edges (≈0.88 here); a stretched slice
    // drifts out of phase with them (≈0.02).
    assert!(
        corr > 0.8,
        "bands in the pane line up with the backdrop's (correlation {corr:.2})"
    );
}

#[test]
fn overlapping_panes_each_blur_the_glassless_backdrop() {
    let t = Theme::light();
    let v = stack().w_full().h_full().bg(t.bg).children([
        bands(&t, 10.0),
        glass_pane("a", 30.0, 30.0, 160.0, 90.0),
        glass_pane("b", 120.0, 80.0, 160.0, 90.0),
    ]);
    let (gpu, reads) = raster(&v, true);
    assert_eq!(reads, 0);
    let (cpu, _) = raster(&v, false);
    assert_like_cpu(&gpu, &cpu);
}

#[test]
fn glass_and_an_element_filter_read_back_only_the_filter() {
    let t = Theme::light();
    let v = stack().w_full().h_full().bg(t.bg).children([
        bands(&t, 10.0),
        glass_pane("a", 30.0, 30.0, 160.0, 90.0),
        text("Blurred words")
            .size(TextSize::Lg)
            .color(t.text)
            .absolute()
            .left(40.0)
            .top(160.0)
            .element_filter(ElementFilter::Blur(2.0)),
    ]);
    let (gpu, reads) = raster(&v, true);
    assert_eq!(reads, 1, "only the filter's region is read back");
    let (cpu, _) = raster(&v, false);
    assert_like_cpu(&gpu, &cpu);
}

#[test]
fn a_frame_without_glass_is_unchanged() {
    let v = view(&Theme::light(), false);
    let (on, reads) = raster(&v, true);
    assert_eq!(reads, 0);
    let (off, _) = raster(&v, false);
    assert_eq!(
        on.as_raw(),
        off.as_raw(),
        "byte-identical with GPU glass on or off"
    );
}
