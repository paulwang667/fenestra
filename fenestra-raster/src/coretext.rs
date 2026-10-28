//! Glyphs only the platform can draw. iOS's Apple Color Emoji stores its
//! `sbix` bitmaps as `emjc`, Apple's private compressed format, not PNG;
//! vello_cpu draws nothing for them, so CoreText draws them instead.
//!
//! The workspace's one `unsafe` module: these are FFI calls.
#![allow(unsafe_code)]

use std::ptr::NonNull;

use objc2_core_foundation::{CFString, CGPoint};
use objc2_core_graphics::{CGBitmapContextCreate, CGColorSpace, CGImageAlphaInfo};
use objc2_core_text::CTFont;
use skrifa::instance::Size;
use skrifa::raw::TableProvider;
use skrifa::string::StringId;
use skrifa::{FontRef, GlyphId, MetadataProvider};

/// Whether `gid` is an `sbix` bitmap vello_cpu has no image for at `size`.
pub(crate) fn draws(font: &FontRef<'_>, gid: GlyphId, size: Size) -> bool {
    let Ok(sbix) = font.sbix() else {
        return false;
    };
    let has_bitmap = sbix
        .strikes()
        .iter()
        .flatten()
        .any(|strike| matches!(strike.glyph_data(gid), Ok(Some(_))));
    has_bitmap && font.bitmap_strikes().glyph_for_size(size, gid).is_none()
}

/// Glyph `gid` drawn at `size` pixels into a `w`×`h` premultiplied RGBA
/// image, its origin at `pen` (y-down pixels). The font is found by its
/// PostScript name — `emjc` fonts are system fonts — and `None` if that
/// finds a different font.
pub(crate) fn draw(
    font: &FontRef<'_>,
    gid: GlyphId,
    size: f64,
    (w, h): (u32, u32),
    pen: (f64, f64),
) -> Option<Vec<[u8; 4]>> {
    let name = font
        .localized_strings(StringId::POSTSCRIPT_NAME)
        .english_or_first()?
        .to_string();
    // SAFETY: a null matrix means identity.
    let ct = unsafe { CTFont::with_name(&CFString::from_str(&name), size, std::ptr::null()) };
    let glyphs = font.maxp().ok()?.num_glyphs();
    if unsafe { ct.glyph_count() } != isize::try_from(glyphs).ok()? {
        return None;
    }
    let mut glyph = u16::try_from(gid.to_u32()).ok()?;
    let mut pixels = vec![[0u8; 4]; (w * h) as usize];
    let space = CGColorSpace::new_device_rgb()?;
    // SAFETY: `pixels` holds `h` rows of `w * 4` bytes and outlives `cx`.
    let cx = unsafe {
        CGBitmapContextCreate(
            pixels.as_mut_ptr().cast(),
            w as usize,
            h as usize,
            8,
            w as usize * 4,
            Some(&space),
            CGImageAlphaInfo::PremultipliedLast.0,
        )
    }?;
    // CoreGraphics is y-up from the bottom row; memory starts at the top.
    let mut at = CGPoint::new(pen.0, f64::from(h) - pen.1);
    // SAFETY: one glyph, one position, both live for the call.
    unsafe { ct.draw_glyphs(NonNull::from(&mut glyph), NonNull::from(&mut at), 1, &cx) };
    drop(cx);
    Some(pixels)
}
