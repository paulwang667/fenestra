//! The vello backend's GPU glass, headless: off by default (the vello
//! goldens keep the CPU glass, bit-identical across platforms), and with
//! `set_gpu_glass(true)` the same path the windows take — the raster
//! backend's GPU glass on vello's backdrop texture, handed to vello as image
//! overrides, nothing read back.

use fenestra_core::{
    Element, ElementFilter, Fonts, FrameState, Material, R_LG, SP3, TextSize, Theme, build_frame,
    col, effects, image_rgba8, row, stack, text,
};
use fenestra_shell::{Backend, Headless};
use image::RgbaImage;

const LOGICAL: (f32, f32) = (360.0, 240.0);

fn scene(t: &Theme, panes: Vec<Element<()>>) -> Element<()> {
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
            text("Frosted glass, vello backend")
                .size(TextSize::Lg)
                .color(t.text),
        ]),
    ]);
    let mut children = vec![body];
    children.extend(panes);
    stack().w_full().h_full().bg(t.bg).children(children)
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

fn vello() -> Headless {
    Headless::with_backend(Backend::Vello).expect("a GPU adapter")
}

/// Renders `view` through `h`; returns the image and how many backdrop
/// regions it read back.
fn render_with(h: &mut Headless, view: &Element<()>) -> (RgbaImage, u32) {
    let theme = Theme::light();
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();
    state.reduced_motion = true;
    let frame = build_frame(view, &theme, &mut fonts, &mut state, LOGICAL, 1.0);
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

/// A fresh vello `Headless` with GPU glass on or off.
fn render(view: &Element<()>, gpu_glass: bool) -> (RgbaImage, u32) {
    let mut h = vello();
    h.set_gpu_glass(gpu_glass);
    render_with(&mut h, view)
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

#[track_caller]
fn assert_like_cpu(gpu: &RgbaImage, cpu: &RgbaImage) {
    let frac = diff_frac(gpu, cpu, 8);
    assert!(
        frac <= 0.02,
        "{:.2}% of pixels differ from the CPU glass by more than 8/255",
        frac * 100.0
    );
}

fn one_pane() -> Element<()> {
    scene(
        &Theme::light(),
        vec![glass_pane("pane", 40.0, 20.0, 200.0, 60.0)],
    )
}

#[test]
fn vello_headless_keeps_cpu_glass_by_default() {
    let (_, reads) = render_with(&mut vello(), &one_pane());
    assert_eq!(reads, 1, "the vello goldens keep the CPU glass");
}

#[test]
fn vello_gpu_glass_reads_nothing_back() {
    let (gpu, reads) = render(&one_pane(), true);
    assert_eq!(reads, 0, "GPU glass reads no backdrop back");
    let (cpu, _) = render(&one_pane(), false);
    assert_like_cpu(&gpu, &cpu);
}

#[test]
fn overlapping_panes_each_blur_the_glassless_backdrop() {
    let t = Theme::light();
    let v = stack().w_full().h_full().bg(t.bg).children([
        bands(&t, 10.0),
        glass_pane("a", 30.0, 30.0, 160.0, 90.0),
        glass_pane("b", 120.0, 80.0, 160.0, 90.0),
    ]);
    let (gpu, reads) = render(&v, true);
    assert_eq!(reads, 0);
    let (cpu, _) = render(&v, false);
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
    let (gpu, reads) = render(&v, true);
    assert_eq!(reads, 1, "only the filter's region is read back");
    let (cpu, _) = render(&v, false);
    assert_like_cpu(&gpu, &cpu);
}

#[test]
fn a_pane_partly_off_screen_is_not_stretched() {
    let t = Theme::light();
    // Top 40% above the frame, no tint or sheen, a slight blur: the bands in
    // its visible part sit where they sit in the backdrop.
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
    let (image, _) = render(&glass, true);
    let (sharp, _) = render(
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
    // The blur softens each band's edges; a stretched slice drifts out of
    // phase with them.
    assert!(
        corr > 0.8,
        "bands in the pane line up with the backdrop's (correlation {corr:.2})"
    );
}

#[test]
fn a_pane_under_a_moved_region_lines_up() {
    let t = Theme::light();
    let at = |left| {
        stack()
            .w_full()
            .h_full()
            .bg(t.bg)
            .children([bands(&t, 10.0), glass_pane("pane", left, 30.0, 140.0, 80.0)])
    };
    let mut h = vello();
    h.set_gpu_glass(true);
    render_with(&mut h, &at(20.0));
    // The region the backdrop renders moves with the pane: the second frame
    // must use its own origin, not the first's.
    let (moved, reads) = render_with(&mut h, &at(180.0));
    assert_eq!(reads, 0);
    let (cpu, _) = render(&at(180.0), false);
    assert_like_cpu(&moved, &cpu);
}

#[test]
fn a_frame_without_glass_is_unchanged() {
    let v = scene(&Theme::light(), vec![]);
    let (on, reads) = render(&v, true);
    assert_eq!(reads, 0);
    let (off, _) = render(&v, false);
    // vello itself is not bit-stable from one renderer to the next on every
    // GPU (two CPU-glass renders of this frame differ now and then by one
    // level on one pixel), so "unchanged" is: the same, give or take that.
    let max = on
        .as_raw()
        .iter()
        .zip(off.as_raw())
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    let pixels = on
        .pixels()
        .zip(off.pixels())
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        max <= 1 && pixels <= 4,
        "unchanged with GPU glass on or off: {pixels} pixels differ, by up to {max}/255"
    );
}

#[test]
fn a_vanished_pane_is_unregistered_from_vello() {
    let mut h = vello();
    h.set_gpu_glass(true);
    render_with(&mut h, &one_pane());
    assert_eq!(h.vello_glass_registered(), 1);
    render_with(&mut h, &scene(&Theme::light(), vec![]));
    assert_eq!(
        h.vello_glass_registered(),
        0,
        "the gone pane's override is dropped"
    );
}

/// Sum of horizontal neighbour differences inside `rect`: high for
/// stripes, low once they are blurred.
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

/// A pane the GPU glass cannot take (wider than the device's largest
/// texture) still frosts: it falls back to the CPU, as before.
#[test]
fn a_pane_too_wide_for_the_gpu_still_frosts_on_the_cpu() {
    let t = Theme::light();
    // Blur only (no tint to dim the stripes on its own).
    let wide = col()
        .id("wide")
        .absolute()
        .left(40.0)
        .top(20.0)
        .w(20_000.0)
        .h(60.0)
        .backdrop_blur(10.0);
    let (glass, _) = render(&scene(&t, vec![wide]), true);
    let (sharp, _) = render(&scene(&t, vec![]), true);
    let under = (48, 28, 300, 44);
    assert!(
        energy(&glass, under) * 2 < energy(&sharp, under),
        "the stripes under the pane are blurred"
    );
}
