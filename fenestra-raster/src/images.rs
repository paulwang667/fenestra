//! Image textures, cached across frames by the image's identity.

use std::collections::HashMap;

use peniko::{ImageAlphaType, ImageData, ImageFormat};

/// Frames an image may go undrawn before its texture is dropped.
const KEEP_FRAMES: u64 = 120;

/// What identifies an image's pixels: blob id, size, format, alpha type.
pub(crate) type Key = (u64, u32, u32, u8, u8);

fn key(image: &ImageData) -> Key {
    (
        image.data.id(),
        image.width,
        image.height,
        image.format as u8,
        image.alpha_type as u8,
    )
}

struct Entry {
    view: wgpu::TextureView,
    last_used: u64,
}

/// Image textures, and the images drawn this frame that are not uploaded yet.
#[derive(Default)]
pub(crate) struct ImageCache {
    entries: HashMap<Key, Entry>,
    /// Drawn this frame, waiting for `upload`.
    pending: Vec<(Key, ImageData)>,
    frame: u64,
}

impl ImageCache {
    /// Starts a frame, dropping textures unused for [`KEEP_FRAMES`] frames.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        let frame = self.frame;
        self.entries
            .retain(|_, e| frame - e.last_used <= KEEP_FRAMES);
        self.pending.clear();
    }

    /// Cached textures.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Notes that `image` is drawn this frame; returns its key.
    pub fn use_image(&mut self, image: &ImageData) -> Key {
        let k = key(image);
        if let Some(e) = self.entries.get_mut(&k) {
            e.last_used = self.frame;
        } else if !self.pending.iter().any(|(p, _)| *p == k) {
            self.pending.push((k, image.clone()));
        }
        k
    }

    /// Uploads the images first drawn this frame; returns how many.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        max_dimension: u32,
    ) -> u32 {
        let mut uploaded = 0;
        for (k, image) in self.pending.drain(..) {
            let (width, height, pixels) = rgba_premultiplied(&image, max_dimension);
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("fenestra-raster image"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                texture.as_image_copy(),
                &pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: None,
                },
                texture.size(),
            );
            self.entries.insert(
                k,
                Entry {
                    view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
                    last_used: self.frame,
                },
            );
            uploaded += 1;
        }
        uploaded
    }

    /// The texture for `k` (uploaded this frame or before).
    pub fn view(&self, k: &Key) -> Option<&wgpu::TextureView> {
        self.entries.get(k).map(|e| &e.view)
    }
}

/// `image` as premultiplied RGBA8, box-downscaled to fit `max_dimension`
/// on both sides (keeping its aspect ratio).
fn rgba_premultiplied(image: &ImageData, max_dimension: u32) -> (u32, u32, Vec<u8>) {
    let (w, h) = (image.width.max(1), image.height.max(1));
    let data = image.data.data();
    let texel = |x: u32, y: u32| -> [u32; 4] {
        let i = ((y * w + x) * 4) as usize;
        let Some(p) = data.get(i..i + 4) else {
            return [0; 4];
        };
        let [r, g, b, a] = match image.format {
            ImageFormat::Bgra8 => [p[2], p[1], p[0], p[3]],
            _ => [p[0], p[1], p[2], p[3]],
        }
        .map(u32::from);
        match image.alpha_type {
            ImageAlphaType::Alpha => [
                (r * a + 127) / 255,
                (g * a + 127) / 255,
                (b * a + 127) / 255,
                a,
            ],
            _ => [r, g, b, a],
        }
    };
    let factor = w.max(h).div_ceil(max_dimension.max(1)).max(1);
    let (tw, th) = (w.div_ceil(factor), h.div_ceil(factor));
    let mut out = Vec::with_capacity((tw * th * 4) as usize);
    for ty in 0..th {
        for tx in 0..tw {
            let mut sum = [0u32; 4];
            let mut n = 0;
            for y in ty * factor..((ty + 1) * factor).min(h) {
                for x in tx * factor..((tx + 1) * factor).min(w) {
                    for (s, c) in sum.iter_mut().zip(texel(x, y)) {
                        *s += c;
                    }
                    n += 1;
                }
            }
            #[expect(
                clippy::cast_possible_truncation,
                reason = "an average of bytes is a byte"
            )]
            out.extend(sum.map(|s| ((s + n / 2) / n.max(1)) as u8));
        }
    }
    (tw, th, out)
}
