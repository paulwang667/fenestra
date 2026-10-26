//! Images and blurred rounded rects (shadows), compared with vello.

mod support;

use fenestra_core::draw::DrawTarget;
use fenestra_raster::RasterRenderer;
use kurbo::{Affine, Rect};
use peniko::{Blob, Color, ImageAlphaType, ImageData, ImageFormat};
use support::{
    Budget, EDGES, FILLS, FORMAT, Paint, assert_like_vello_all, assert_like_vello_with, gpu,
    render_raster, renderer,
};

/// A `w`×`h` test pattern: red rising across, green down, alpha in steps.
fn pattern(
    w: u32,
    h: u32,
    format: ImageFormat,
    alpha_type: ImageAlphaType,
    translucent: bool,
) -> ImageData {
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = ((x * 255 / w) as u8, (y * 255 / h) as u8, 140u8);
            let a = if translucent {
                [255u8, 200, 120, 60][((x / 8 + y / 8) % 4) as usize]
            } else {
                255
            };
            let (r, g, b) = match alpha_type {
                ImageAlphaType::AlphaPremultiplied => {
                    let m = |c: u8| (u32::from(c) * u32::from(a) / 255) as u8;
                    (m(r), m(g), m(b))
                }
                _ => (r, g, b),
            };
            data.extend(match format {
                ImageFormat::Bgra8 => [b, g, r, a],
                _ => [r, g, b, a],
            });
        }
    }
    ImageData {
        data: Blob::from(data),
        format,
        alpha_type,
        width: w,
        height: h,
    }
}

struct Picture(ImageData, Affine);
impl Paint for Picture {
    fn paint<T: DrawTarget>(&self, t: &mut T) {
        t.draw_image(&self.0, self.1);
    }
}

fn scaled_into(image: &ImageData, dest: Rect) -> Affine {
    Affine::translate((dest.x0, dest.y0))
        * Affine::scale_non_uniform(
            dest.width() / f64::from(image.width),
            dest.height() / f64::from(image.height),
        )
}

#[test]
fn image_matches_vello() {
    let image = pattern(64, 48, ImageFormat::Rgba8, ImageAlphaType::Alpha, false);
    let t = scaled_into(&image, Rect::new(20.0, 5.0, 220.0, 155.0));
    assert_like_vello_all("image", &Picture(image, t), (240, 160), &[EDGES, FILLS]);
}

#[test]
fn bgra_and_unpremultiplied_images_match_vello() {
    for (format, alpha) in [
        (ImageFormat::Bgra8, ImageAlphaType::Alpha),
        (ImageFormat::Rgba8, ImageAlphaType::AlphaPremultiplied),
        (ImageFormat::Bgra8, ImageAlphaType::AlphaPremultiplied),
    ] {
        let image = pattern(64, 48, format, alpha, true);
        let t = scaled_into(&image, Rect::new(20.0, 5.0, 220.0, 155.0));
        assert_like_vello_all(
            &format!("image_{format:?}_{alpha:?}"),
            &Picture(image, t),
            (240, 160),
            &[EDGES, FILLS],
        );
    }
}

#[test]
fn rotated_image_matches_vello() {
    let image = pattern(64, 48, ImageFormat::Rgba8, ImageAlphaType::Alpha, false);
    let about = Affine::translate((120.0, 80.0));
    let t = about
        * Affine::rotate(25f64.to_radians())
        * about.inverse()
        * scaled_into(&image, Rect::new(40.0, 20.0, 200.0, 140.0));
    assert_like_vello_all(
        "image_rotated",
        &Picture(image, t),
        (240, 160),
        &[EDGES, FILLS],
    );
}

struct Shadow(Affine);
impl Paint for Shadow {
    fn paint<T: DrawTarget>(&self, t: &mut T) {
        let color = Color::from_rgba8(0, 0, 0, 128);
        t.draw_blurred_rounded_rect(
            self.0,
            Rect::new(50.0, 40.0, 190.0, 120.0),
            color,
            12.0,
            8.0,
        );
    }
}

#[test]
fn shadow_matches_vello() {
    assert_like_vello_all(
        "shadow",
        &Shadow(Affine::IDENTITY),
        (240, 160),
        &[EDGES, FILLS],
    );
}

#[test]
fn rotated_shadow_matches_vello() {
    let about = Affine::translate((120.0, 80.0));
    let t = about * Affine::rotate(30f64.to_radians()) * about.inverse();
    assert_like_vello_all("shadow_rotated", &Shadow(t), (240, 160), &[EDGES, FILLS]);
}

/// A phone GPU may cap textures at 4096 px; an image past the cap is
/// downscaled on upload, not rejected by wgpu.
#[test]
fn an_image_larger_than_the_texture_limit_is_downscaled() {
    let mut r = RasterRenderer::with_max_texture_dimension(&gpu().device, FORMAT, 256);
    let image = pattern(512, 256, ImageFormat::Rgba8, ImageAlphaType::Alpha, false);
    let t = scaled_into(&image, Rect::new(0.0, 16.0, 256.0, 144.0));
    assert_like_vello_with(
        &mut r,
        "image_downscaled",
        &Picture(image, t),
        (256, 160),
        Budget {
            channel: 8,
            max_frac: 0.05,
        },
    );
}

struct Nothing;
impl Paint for Nothing {
    fn paint<T: DrawTarget>(&self, _: &mut T) {}
}

#[test]
fn image_cache_reuses_and_evicts() {
    let mut r = renderer();
    let image = pattern(16, 16, ImageFormat::Rgba8, ImageAlphaType::Alpha, false);
    let scene = Picture(image, Affine::IDENTITY);
    let (_, first) = render_raster(&mut r, &scene, (32, 32));
    let (_, second) = render_raster(&mut r, &scene, (32, 32));
    assert_eq!(first.images_uploaded, 1);
    assert_eq!(second.images_uploaded, 0);
    assert_eq!(r.cache_sizes().images, 1);
    for _ in 0..121 {
        render_raster(&mut r, &Nothing, (32, 32));
    }
    assert_eq!(r.cache_sizes().images, 0);
}
