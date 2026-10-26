//! Glyph runs through the atlas, compared with vello.

mod support;

use fenestra_core::draw::{DrawTarget, Glyph, GlyphRun};
use kurbo::Affine;
use peniko::{Blob, Color, FontData};
use skrifa::{FontRef, GlyphId, MetadataProvider, instance::Size};
use support::{Budget, Paint, assert_like_vello, render_raster, renderer};

/// Text: glyph edges come from two different rasterizers (vello's GPU
/// fill vs vello_cpu into the atlas, at quarter-pixel positions), so AA
/// differs on every edge; this budget catches misplaced, missing, or
/// mis-sized glyphs, not AA.
const TEXT: Budget = Budget {
    channel: 24,
    max_frac: 0.03,
};

fn inter() -> FontData {
    let bytes = include_bytes!("../../fenestra-core/assets/inter/Inter-Regular.otf");
    FontData::new(Blob::from(bytes.to_vec()), 0)
}

/// The first system font fontique falls back to for `key`, if any.
fn system_font(key: impl Into<fontique::FallbackKey>) -> Option<FontData> {
    let mut collection = fontique::Collection::new(fontique::CollectionOptions {
        shared: false,
        system_fonts: true,
    });
    let family = collection.fallback_families(key).next()?;
    let font = collection.family(family)?.fonts().first()?.clone();
    Some(FontData::new(font.load(None)?, font.index()))
}

fn emoji_font() -> Option<FontData> {
    let mut collection = fontique::Collection::new(fontique::CollectionOptions {
        shared: false,
        system_fonts: true,
    });
    let family = collection
        .generic_families(fontique::GenericFamily::Emoji)
        .next()?;
    let font = collection.family(family)?.fonts().first()?.clone();
    Some(FontData::new(font.load(None)?, font.index()))
}

fn han() -> Option<FontData> {
    system_font(fontique::Script::from_bytes(*b"Hani"))
}

/// Glyphs for `text` from the cmap, advanced by their widths (no shaping).
fn layout(font: &FontData, size: f32, text: &str, (x, y): (f32, f32)) -> Vec<Glyph> {
    let f = FontRef::from_index(font.data.data(), font.index).expect("a font");
    let metrics = f.glyph_metrics(Size::new(size), skrifa::instance::LocationRef::default());
    let mut pen = x;
    text.chars()
        .map(|c| {
            let id = f.charmap().map(c).unwrap_or(GlyphId::NOTDEF);
            let g = Glyph {
                id: id.to_u32(),
                x: pen,
                y,
            };
            pen += metrics.advance_width(id).unwrap_or(size * 0.5);
            g
        })
        .collect()
}

struct Text {
    font: FontData,
    size: f32,
    lines: Vec<(String, (f32, f32))>,
    transform: Affine,
    glyph_transform: Option<Affine>,
    color: Color,
}

impl Text {
    fn new(font: FontData, size: f32, lines: &[(&str, (f32, f32))]) -> Self {
        Self {
            font,
            size,
            lines: lines.iter().map(|(s, p)| (s.to_string(), *p)).collect(),
            transform: Affine::IDENTITY,
            glyph_transform: None,
            color: Color::from_rgb8(20, 20, 30),
        }
    }
}

impl Paint for Text {
    fn paint<T: DrawTarget>(&self, t: &mut T) {
        let run = GlyphRun {
            font: &self.font,
            font_size: self.size,
            hint: true,
            normalized_coords: &[],
            transform: self.transform,
            glyph_transform: self.glyph_transform,
            color: self.color,
        };
        for (line, origin) in &self.lines {
            t.draw_glyphs(
                &run,
                layout(&self.font, self.size, line, *origin).into_iter(),
            );
        }
    }
}

#[test]
fn latin_text_matches_vello() {
    let text = Text::new(
        inter(),
        16.0,
        &[
            ("The quick brown fox jumps", (10.0, 30.0)),
            ("over 1,234 lazy dogs.", (10.5, 58.25)),
        ],
    );
    assert_like_vello("latin", &text, (260, 80), TEXT);
}

#[test]
fn cjk_text_matches_vello() {
    let Some(font) = han() else {
        eprintln!("skipped: no system Han font");
        return;
    };
    let text = Text::new(font, 18.0, &[("中文混排的段落，界面框架。", (10.0, 32.0))]);
    assert_like_vello("cjk", &text, (260, 50), TEXT);
}

#[test]
fn emoji_is_drawn_in_color() {
    let Some(font) = emoji_font() else {
        eprintln!("skipped: no system emoji font");
        return;
    };
    let text = Text::new(font, 32.0, &[("👍", (10.0, 40.0))]);
    let (image, _) = render_raster(&mut renderer(), &text, (60, 60));
    let saturated = image.pixels.as_chunks::<4>().0.iter().any(|p| {
        let (max, min) = (p[..3].iter().max().unwrap(), p[..3].iter().min().unwrap());
        *max > 0 && f32::from(max - min) / f32::from(*max) > 0.3
    });
    assert!(saturated, "the emoji keeps its colors");
}

#[test]
fn synthetic_italic_is_skewed() {
    let mut text = Text::new(inter(), 20.0, &[("italic text", (12.0, 34.0))]);
    let upright = render_raster(&mut renderer(), &text, (160, 50)).0;
    text.glyph_transform = Some(Affine::skew(14f64.to_radians().tan(), 0.0));
    assert_like_vello("italic", &text, (160, 50), TEXT);
    let skewed = render_raster(&mut renderer(), &text, (160, 50)).0;
    assert!(
        support::diff_frac(&upright, &skewed, 24) > 0.01,
        "the skew shows"
    );
}

#[test]
fn scaled_text_is_rasterized_at_the_scaled_size() {
    let mut text = Text::new(inter(), 16.0, &[("Scaled twice", (6.0, 22.0))]);
    text.transform = Affine::scale(2.0);
    assert_like_vello("scaled", &text, (240, 60), TEXT);
}

#[test]
fn a_second_frame_rasterizes_nothing() {
    let text = Text::new(inter(), 16.0, &[("Cached glyphs", (10.0, 30.0))]);
    let mut r = renderer();
    let (_, first) = render_raster(&mut r, &text, (200, 50));
    let (_, second) = render_raster(&mut r, &text, (200, 50));
    assert!(first.glyphs_rasterized > 0);
    assert_eq!(second.glyphs_rasterized, 0);
}

/// 3000 distinct 48px ideographs need several atlas pages in one frame;
/// none may go missing, and the extra pages go once they are unused.
#[test]
fn a_frame_larger_than_one_page_draws_every_glyph() {
    let Some(font) = han() else {
        eprintln!("skipped: no system Han font");
        return;
    };
    const N: u32 = 3000;
    const COLS: u32 = 60;
    let chars: String = (0..N)
        .map(|i| char::from_u32(0x4E00 + i).unwrap())
        .collect();
    let lines: Vec<(String, (f32, f32))> = chars
        .chars()
        .collect::<Vec<_>>()
        .chunks(COLS as usize)
        .enumerate()
        .map(|(row, cs)| {
            (
                cs.iter().collect::<String>(),
                (0.0, 50.0 * row as f32 + 42.0),
            )
        })
        .collect();
    let line_refs: Vec<(&str, (f32, f32))> = lines.iter().map(|(s, p)| (s.as_str(), *p)).collect();
    let mut text = Text::new(font.clone(), 48.0, &line_refs);
    // Fixed 50px cells, whatever the advances.
    text.lines = lines;
    struct Grid(Text);
    impl Paint for Grid {
        fn paint<T: DrawTarget>(&self, t: &mut T) {
            let run = GlyphRun {
                font: &self.0.font,
                font_size: 48.0,
                hint: true,
                normalized_coords: &[],
                transform: Affine::IDENTITY,
                glyph_transform: None,
                color: Color::BLACK,
            };
            for (row, (line, _)) in self.0.lines.iter().enumerate() {
                let glyphs = layout(&self.0.font, 48.0, line, (0.0, 0.0));
                t.draw_glyphs(
                    &run,
                    glyphs.into_iter().enumerate().map(|(col, g)| Glyph {
                        id: g.id,
                        x: 50.0 * col as f32 + 1.0,
                        y: 50.0 * row as f32 + 42.0,
                    }),
                );
            }
        }
    }
    let size = (COLS * 50, (N / COLS) * 50);
    let mut r = renderer();
    let (image, _) = render_raster(&mut r, &Grid(text), size);
    for i in 0..N {
        let (cx, cy) = ((i % COLS) * 50, (i / COLS) * 50);
        let inked = (cy..cy + 50).any(|y| (cx..cx + 50).any(|x| image.pixel(x, y)[0] < 128));
        assert!(inked, "glyph {i} (U+{:04X}) is missing", 0x4E00 + i);
    }
    let few = Text::new(font, 48.0, &[("一二三四五六七八九十", (0.0, 42.0))]);
    render_raster(&mut r, &few, (600, 60));
    assert!(r.cache_sizes().atlas_pages <= 4, "{:?}", r.cache_sizes());
}

/// Darkness-weighted ink: total and centroid.
fn ink(image: &support::Image) -> (f64, f64, f64) {
    let (mut mass, mut sx, mut sy) = (0.0, 0.0, 0.0);
    for y in 0..image.height {
        for x in 0..image.width {
            let dark = 255.0 - f64::from(image.pixel(x, y)[0]);
            mass += dark;
            sx += dark * (f64::from(x) + 0.5);
            sy += dark * (f64::from(y) + 0.5);
        }
    }
    (mass, sx / mass, sy / mass)
}

/// An `H` whose cap height is `cap` device pixels, at `origin`.
fn big_h(cap: f64, origin: (f32, f32)) -> Text {
    let font = inter();
    let f = FontRef::from_index(font.data.data(), font.index).expect("a font");
    let id = f.charmap().map('H').expect("an H");
    let per_em = f
        .glyph_metrics(Size::new(1.0), skrifa::instance::LocationRef::default())
        .bounds(id)
        .expect("bounds")
        .y_max;
    #[expect(clippy::cast_possible_truncation, reason = "a font size")]
    let size = (cap / f64::from(per_em)) as f32;
    Text::new(font, size, &[("H", origin)])
}

/// The raster glyph's ink matches vello's in amount and position.
fn assert_ink_like_vello(text: &Text, size: (u32, u32)) {
    let (raster, _) = render_raster(&mut renderer(), text, size);
    let vello = support::render_vello(text, size);
    let (rm, rx, ry) = ink(&raster);
    let (vm, vx, vy) = ink(&vello);
    assert!((rm / vm - 1.0).abs() < 0.03, "ink {rm} vs vello {vm}");
    assert!(
        (rx - vx).abs() < 0.3 && (ry - vy).abs() < 0.3,
        "centroid ({rx:.2}, {ry:.2}) vs vello ({vx:.2}, {vy:.2})"
    );
}

/// Just under twice the cell limit: the half-size cell is itself still
/// oversized, so the glyph is shrunk twice and must be scaled up by both.
#[test]
fn a_glyph_shrunk_twice_draws_at_full_size() {
    assert_ink_like_vello(&big_h(961.0, (40.0, 1000.0)), (1100, 1040));
}

/// An oversized glyph at a fractional position lands where vello puts it.
#[test]
fn an_oversized_glyph_keeps_its_subpixel_position() {
    assert_ink_like_vello(&big_h(600.0, (40.75, 660.75)), (700, 700));
}
