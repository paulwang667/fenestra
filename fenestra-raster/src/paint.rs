//! Brushes to vertex parameters, and the gradient ramp texture.
//!
//! A gradient is evaluated per fragment in the shape's own space (the
//! vertex's `local`): `grad` holds its geometry, `grad2` its kind, ramp row,
//! and extend mode. Ramps are built exactly as vello builds them, so the
//! colors match its output.

use peniko::color::{HueDirection, Srgb};
use peniko::{BrushRef, ColorStop, Extend, Gradient, GradientKind};

use crate::geometry::PaintParams;

/// Samples per ramp row (vello's `GRADIENT_WIDTH`).
pub(crate) const RAMP_WIDTH: u32 = 512;
/// Rows kept across frames before rows the last frame did not use go.
const RETAINED_ROWS: usize = 64;

/// `grad2.x`: how the fragment shader paints.
const KIND_LINEAR: f32 = 1.0;
const KIND_RADIAL: f32 = 2.0;
const KIND_SWEEP: f32 = 3.0;

/// A ramp's 512 premultiplied RGBA8 samples.
type Ramp = Vec<[u8; 4]>;

struct Row {
    /// What the ramp is built from: [`stops_key`].
    key: Vec<u32>,
    ramp: Ramp,
    last_used: u64,
}

/// The gradient ramps of recent frames, one texture row each.
#[derive(Default)]
pub(crate) struct Ramps {
    rows: Vec<Row>,
    frame: u64,
    texture: Option<(wgpu::Texture, wgpu::TextureView)>,
    /// Rows changed since the last upload.
    dirty: bool,
    /// Ramps built this frame.
    pub built: u32,
}

/// vello_encoding 0.9.0 `ramp_cache.rs` `make_ramp` (MIT OR Apache-2.0):
/// sRGB stops blended with `color`'s `lerp` (which, like CSS, interpolates
/// premultiplied), each sample stored premultiplied.
fn make_ramp(stops: &[ColorStop]) -> Ramp {
    let mut last_u = 0.0;
    let mut last_c = stops[0].color.to_alpha_color::<Srgb>();
    let mut this_u = last_u;
    let mut this_c = last_c;
    let mut j = 0;
    (0..RAMP_WIDTH)
        .map(|i| {
            #[expect(clippy::cast_precision_loss, reason = "ramp indices are exact in f32")]
            let u = (i as f32) / (RAMP_WIDTH - 1) as f32;
            while u > this_u {
                last_u = this_u;
                last_c = this_c;
                if let Some(s) = stops.get(j + 1) {
                    this_u = s.offset;
                    this_c = s.color.to_alpha_color::<Srgb>();
                    j += 1;
                } else {
                    break;
                }
            }
            let du = this_u - last_u;
            let c = if du < 1e-9 {
                this_c
            } else {
                last_c.lerp(this_c, (u - last_u) / du, HueDirection::default())
            };
            let p = c.premultiply().to_rgba8();
            [p.r, p.g, p.b, p.a]
        })
        .collect()
}

/// `stops` as bits (offsets and sRGB components), to find a ramp built
/// before without building it again.
fn stops_key(stops: &[ColorStop]) -> Vec<u32> {
    stops
        .iter()
        .flat_map(|s| {
            let c = s.color.to_alpha_color::<Srgb>().components;
            [s.offset, c[0], c[1], c[2], c[3]].map(f32::to_bits)
        })
        .collect()
}

fn extend_code(extend: Extend) -> f32 {
    match extend {
        Extend::Pad => 0.0,
        Extend::Repeat => 1.0,
        Extend::Reflect => 2.0,
    }
}

fn premultiply(c: peniko::Color) -> [f32; 4] {
    let [r, g, b, a] = c.components;
    [r * a, g * a, b * a, a]
}

/// A solid brush of `c`.
pub(crate) fn solid(c: peniko::Color) -> PaintParams {
    PaintParams {
        color: premultiply(c),
        ..PaintParams::default()
    }
}

impl Ramps {
    /// Starts a frame: past the retained count, rows the last frame did not
    /// use are dropped (row numbers are only meaningful within a frame).
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        self.built = 0;
        if self.rows.len() > RETAINED_ROWS {
            let last = self.frame - 1;
            self.rows.retain(|r| r.last_used == last);
            self.dirty = true;
        }
    }

    /// Rows held.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// The row holding the ramp of `stops`, building it if new.
    fn row(&mut self, stops: &[ColorStop]) -> usize {
        let frame = self.frame;
        let key = stops_key(stops);
        if let Some(i) = self.rows.iter().position(|r| r.key == key) {
            self.rows[i].last_used = frame;
            return i;
        }
        self.built += 1;
        self.rows.push(Row {
            key,
            ramp: make_ramp(stops),
            last_used: frame,
        });
        self.dirty = true;
        self.rows.len() - 1
    }

    /// The vertex parameters for `brush`; `None` paints nothing.
    pub fn params(&mut self, brush: BrushRef<'_>) -> Option<PaintParams> {
        match brush {
            BrushRef::Solid(c) => Some(solid(c)),
            BrushRef::Gradient(g) => self.gradient(g),
            BrushRef::Image(_) => {
                debug_assert!(false, "fenestra never fills with an image brush");
                None
            }
        }
    }

    fn gradient(&mut self, g: &Gradient) -> Option<PaintParams> {
        match g.stops.len() {
            0 => return None,
            1 => return Some(solid(g.stops[0].color.to_alpha_color::<Srgb>())),
            _ => {}
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "gradient geometry fits in f32"
        )]
        let (kind, grad) = match g.kind {
            GradientKind::Linear(p) => (
                KIND_LINEAR,
                [
                    p.start.x as f32,
                    p.start.y as f32,
                    p.end.x as f32,
                    p.end.y as f32,
                ],
            ),
            GradientKind::Radial(p) if p.start_center == p.end_center && p.start_radius == 0.0 => (
                KIND_RADIAL,
                [
                    p.end_center.x as f32,
                    p.end_center.y as f32,
                    p.end_radius,
                    0.0,
                ],
            ),
            GradientKind::Sweep(p) => {
                use core::f32::consts::TAU;
                (
                    KIND_SWEEP,
                    [
                        p.center.x as f32,
                        p.center.y as f32,
                        p.start_angle / TAU,
                        p.end_angle / TAU,
                    ],
                )
            }
            GradientKind::Radial(_) => {
                debug_assert!(false, "fenestra only builds single-circle radial gradients");
                return Some(solid(g.stops[0].color.to_alpha_color::<Srgb>()));
            }
        };
        let row = self.row(&g.stops);
        #[expect(clippy::cast_precision_loss, reason = "row numbers are small")]
        Some(PaintParams {
            color: [1.0; 4],
            grad,
            grad2: [kind, row as f32, extend_code(g.extend), 0.0],
        })
    }

    /// Uploads the rows (when they changed) and returns the ramp texture.
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> &wgpu::TextureView {
        #[expect(clippy::cast_possible_truncation, reason = "row counts are small")]
        let needed = (self.rows.len().max(1) as u32).next_power_of_two();
        let capacity = self.texture.as_ref().map(|(t, _)| t.height());
        if capacity.is_none_or(|c| c < needed) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("fenestra-raster ramps"),
                size: wgpu::Extent3d {
                    width: RAMP_WIDTH,
                    height: needed,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.texture = Some((texture, view));
            self.dirty = true;
        }
        let (texture, view) = self.texture.as_ref().expect("just ensured");
        if self.dirty && !self.rows.is_empty() {
            let data: Vec<u8> = self
                .rows
                .iter()
                .flat_map(|r| r.ramp.iter().flatten().copied())
                .collect();
            #[expect(clippy::cast_possible_truncation, reason = "row counts are small")]
            let height = self.rows.len() as u32;
            queue.write_texture(
                texture.as_image_copy(),
                &data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(RAMP_WIDTH * 4),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: RAMP_WIDTH,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }
        self.dirty = false;
        view
    }
}
