//! The two-pass renderer reads back only the part of the backdrop the glass
//! uses (`backdrop_region`), rendered from the scene shifted to that part's
//! origin. These pin that it is the same picture as rendering the whole frame:
//! the rasterizer is translation-invariant at whole-pixel offsets, and the
//! final image matches the whole-frame two-pass render.

use fenestra_core::{
    AdaptiveTint, Element, Fonts, FrameState, Material, R_LG, SP3, ShadowToken, Sheen,
    SpecularEdge, TextSize, Theme, Weight, build_frame, col, effects, image_rgba8, row, stack,
    text,
};
use fenestra_shell::vello::{Scene, kurbo::Affine};
use fenestra_shell::{
    GlassCache, process_specs, process_specs_in, region_scene, with_fonts, with_headless,
};

const LOGICAL: (f32, f32) = (480.0, 300.0);
const SCALE: f64 = 2.0;

fn physical() -> (u32, u32) {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "test sizes"
    )]
    let size = (
        (f64::from(LOGICAL.0) * SCALE) as u32,
        (f64::from(LOGICAL.1) * SCALE) as u32,
    );
    size
}

fn pane(t: &Theme, label: &str) -> Element<()> {
    col()
        .absolute()
        .p(SP3)
        .rounded(R_LG)
        .bg(Material::new(0.55, 14.0, 1.5).tint(t.surface_raised))
        .backdrop_blur(14.0)
        .backdrop_saturate(1.5)
        .specular_edge(SpecularEdge::glass())
        .sheen(Sheen::glass())
        .adaptive_tint(AdaptiveTint::glass())
        .shadow(ShadowToken::Lg)
        .overflow_hidden()
        .child(text(label.to_owned()).size(TextSize::Sm).color(t.text))
}

/// Text, an image, rounded fills and shadows behind three glass panes — one
/// of them hanging off the right edge.
fn view(t: &Theme) -> Element<()> {
    let pts = [
        (0.0, 0.0, t.accent),
        (1.0, 0.0, t.danger.solid),
        (0.0, 1.0, t.warning.solid),
        (1.0, 1.0, t.success.solid),
    ]
    .map(|(x, y, color)| effects::MeshPoint { x, y, color });
    let stripes = row().w_full().h(120.0).children(
        (0..24_usize)
            .map(|i| col().w(20.0).h_full().bg(t.accents.step(2 + i % 9)))
            .collect::<Vec<_>>(),
    );
    let body = col().w_full().h_full().gap(SP3).children([
        stripes,
        row().gap(SP3).children([
            image_rgba8(160, 90, effects::mesh(160, 90, &pts))
                .w(160.0)
                .h(90.0)
                .rounded(R_LG),
            col().gap(4.0).children([
                text("Frosted region read-back")
                    .size(TextSize::Lg)
                    .weight(Weight::Semibold)
                    .color(t.text),
                text("the quick brown fox jumps over the lazy dog").color(t.text_muted),
            ]),
        ]),
    ]);
    stack().w_full().h_full().bg(t.bg).children([
        body,
        pane(t, "left").top(40.0).left(30.0).w(150.0).h(80.0),
        pane(t, "middle").top(150.0).left(200.0).w(160.0).h(90.0),
        pane(t, "off the edge")
            .top(20.0)
            .left(400.0)
            .w(140.0)
            .h(70.0),
    ])
}

fn at_scale(scene: &Scene) -> Scene {
    let mut s = Scene::new();
    s.append(scene, Some(Affine::scale(SCALE)));
    s
}

fn frame(theme: &Theme, fonts: &mut Fonts, state: &mut FrameState) -> fenestra_core::Frame {
    state.reduced_motion = true;
    build_frame(&view(theme), theme, fonts, state, LOGICAL, SCALE)
}

/// **Shifting is not resampling.** The backdrop rendered whole and cropped to
/// the glass region, and rendered shifted into a target that size, are the
/// same bytes — whole-pixel translation does not move a single sample.
#[test]
fn a_shifted_region_render_is_the_whole_render_cropped() {
    for theme in [Theme::light(), Theme::dark()] {
        let (pw, ph) = physical();
        let (whole, region) = with_fonts(|fonts| {
            let mut state = FrameState::new();
            let frame = frame(&theme, fonts, &mut state);
            let (backdrop, specs) = frame.paint_backdrop(fonts, &mut state);
            let backdrop = at_scale(&backdrop);
            let (rect, shifted) = region_scene(&backdrop, &specs, SCALE, (pw, ph));
            assert!(
                rect.2 < pw || rect.3 < ph,
                "the region is the whole frame: {rect:?}"
            );
            with_headless(|h| {
                let whole = h.render(&backdrop, pw, ph, theme.bg).expect("whole render");
                let part = h
                    .render(&shifted, rect.2, rect.3, theme.bg)
                    .expect("region render");
                (
                    image::imageops::crop_imm(&whole, rect.0, rect.1, rect.2, rect.3).to_image(),
                    part,
                )
            })
            .expect("a GPU adapter")
        });
        assert_eq!(whole.dimensions(), region.dimensions());
        let differing = whole
            .pixels()
            .zip(region.pixels())
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(
            differing, 0,
            "{differing} pixels moved under a whole-pixel shift"
        );
    }
}

/// **The picture is the whole-frame picture.** `render_plan` (region path)
/// against the two-pass render done the old way, over the whole frame.
///
/// Not byte-for-byte: the GPU rasterizer is not bit-stable run to run — the
/// whole-frame path compared with itself differs in a handful of pixels by a
/// level — so this holds the two to that jitter, and no further.
#[test]
fn the_region_two_pass_render_matches_the_whole_frame_one() {
    for theme in [Theme::light(), Theme::dark()] {
        let (pw, ph) = physical();
        let (new, old) = with_fonts(|fonts| {
            let mut state = FrameState::new();
            let frame = frame(&theme, fonts, &mut state);
            let new = with_headless(|h| h.render_plan(&frame, fonts, &mut state, pw, ph, theme.bg))
                .expect("a GPU adapter")
                .expect("region render");
            let mut state = FrameState::new();
            let frame = self::frame(&theme, fonts, &mut state);
            let (backdrop, specs) = frame.paint_backdrop(fonts, &mut state);
            let old = with_headless(|h| {
                let whole = h
                    .render(&at_scale(&backdrop), pw, ph, theme.bg)
                    .expect("backdrop");
                let injected = process_specs(
                    &whole,
                    &specs,
                    SCALE,
                    &|_, _, _| None,
                    &mut Default::default(),
                );
                let final_scene = at_scale(&frame.paint_final(fonts, &mut state, &injected));
                h.render(&final_scene, pw, ph, theme.bg).expect("final")
            })
            .expect("a GPU adapter");
            (new, old)
        });
        let mut differing = 0;
        let mut worst = 0;
        for (a, b) in new.pixels().zip(old.pixels()) {
            let d = (0..4).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
            differing += usize::from(d > 0);
            worst = worst.max(d);
        }
        let budget = (pw * ph) as usize / 2000; // 0.05%
        assert!(
            worst <= 2 && differing <= budget,
            "{differing} pixels differ (budget {budget}), worst by {worst}"
        );
    }
}

/// Stripes and glass only — no image anywhere in the backdrop.
fn imageless(t: &Theme) -> Element<()> {
    stack().w_full().h_full().bg(t.bg).children([
        row().w_full().h_full().children(
            (0..24_usize)
                .map(|i| col().w(20.0).h_full().bg(t.accents.step(2 + i % 9)))
                .collect::<Vec<_>>(),
        ),
        pane(t, "glass").top(60.0).left(120.0).w(200.0).h(100.0),
    ])
}

/// **The second frame has its glass too.** A live window keeps its frame state
/// and renderer from frame to frame, so a still pane's baked body comes back
/// from the cache. vello 0.9 swaps its persistent image atlas for a fresh
/// texture whenever a render draws no image — which the glass-free backdrop
/// pass is — while still counting the body as uploaded; a body reused under
/// its old image id drew from an empty atlas and the pane vanished.
#[test]
fn a_still_pane_survives_the_next_frame() {
    let theme = Theme::dark();
    let (pw, ph) = physical();
    // The windowed shell's per-frame sequence, with what it keeps between
    // frames kept here too: frame state, glass cache and the one renderer.
    let (first, second) = with_fonts(|fonts| {
        let mut state = FrameState::new();
        state.reduced_motion = true;
        let mut glass = GlassCache::default();
        let mut render = |state: &mut FrameState, glass: &mut GlassCache| {
            let frame = build_frame(&imageless(&theme), &theme, fonts, state, LOGICAL, SCALE);
            let (backdrop, specs) = frame.paint_backdrop(fonts, state);
            let (rect, shifted) = region_scene(&at_scale(&backdrop), &specs, SCALE, (pw, ph));
            with_headless(|h| {
                let part = h
                    .render(&shifted, rect.2, rect.3, theme.bg)
                    .expect("backdrop");
                let injected = process_specs_in(
                    &part,
                    (rect.0, rect.1),
                    (pw, ph),
                    &specs,
                    SCALE,
                    &|_, _, _| None,
                    &mut Default::default(),
                    glass,
                );
                let final_scene = at_scale(&frame.paint_final(fonts, state, &injected));
                h.render(&final_scene, pw, ph, theme.bg).expect("final")
            })
            .expect("a GPU adapter")
        };
        let first = render(&mut state, &mut glass);
        let second = render(&mut state, &mut glass);
        (first, second)
    });
    let differing = first
        .pixels()
        .zip(second.pixels())
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        differing < 50,
        "{differing} pixels changed on a still second frame"
    );
}
