//! The glyph atlas: each glyph rasterized once (by vello_cpu, which also
//! draws COLR and bitmap color glyphs) at its device size and a
//! quarter-pixel position, kept in shelf-packed pages — coverage in `R8`
//! pages, color glyphs in `RGBA8` ones.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use etagere::{AllocId, AtlasAllocator, size2};
use fenestra_core::draw::{Glyph, GlyphRun};
use kurbo::{Affine, Point};
use skrifa::instance::{LocationRef, Size};
use skrifa::raw::types::F2Dot14;
use skrifa::{FontRef, GlyphId, MetadataProvider};

/// Side of an atlas page, in pixels.
pub(crate) const PAGE: u32 = 1024;
/// Pages kept once a frame no longer needs more.
const KEEP_PAGES: usize = 4;
/// The largest cell side rasterized at full size; bigger glyphs are
/// rasterized smaller and drawn scaled up.
const MAX_CELL: u32 = 480;
/// Empty pixels around a cell, so bilinear sampling never reads a neighbor.
const PAD: u32 = 1;
/// Pixels of margin around a glyph's bounds inside its cell.
const MARGIN: f64 = 2.0;
/// Subpixel positions per pixel, on each axis.
const BINS: f64 = 4.0;

/// What a page holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Coverage, tinted by the run's color.
    Mask,
    /// Premultiplied color (emoji), faded by the run's alpha.
    Color,
}

/// Everything that changes a glyph's pixels.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    font: u64,
    index: u32,
    glyph: u32,
    /// Device size in 1/16 px.
    size: u32,
    coords: u64,
    hint: bool,
    skew: [u64; 4],
    bin: (u8, u8),
}

struct Cell {
    page: usize,
    alloc: AllocId,
    /// The cell's pixels within the page (padding excluded).
    rect: (u32, u32, u32, u32),
    /// The glyph origin's whole-pixel position within the cell.
    origin: (f64, f64),
    /// Device pixels per cell pixel (above 1 for oversized glyphs).
    scale: f64,
    last_used: u64,
}

struct Page {
    kind: Kind,
    alloc: AtlasAllocator,
    texture: Option<(wgpu::Texture, wgpu::TextureView)>,
    last_used: u64,
}

/// A glyph placed for drawing: which page, where its quad's corners land
/// (top-left, top-right, bottom-right, bottom-left), and its texture
/// rectangle `[u0, v0, u1, v1]`.
pub(crate) struct Placed {
    pub page: usize,
    pub kind: Kind,
    pub corners: [Point; 4],
    pub uv: [f32; 4],
}

/// One cell's pixels waiting to reach its page.
struct Upload {
    page: usize,
    /// The allocated rectangle, padding included.
    rect: (u32, u32, u32, u32),
    pixels: Vec<u8>,
}

/// The atlas pages and the glyphs in them.
#[derive(Default)]
pub(crate) struct Atlas {
    /// Freed pages leave `None`, so cells' page numbers stay valid.
    pages: Vec<Option<Page>>,
    cells: HashMap<Key, Cell>,
    uploads: Vec<Upload>,
    frame: u64,
    /// Glyphs rasterized this frame.
    pub rasterized: u32,
    resources: Option<vello_cpu::Resources>,
}

fn hash_of(coords: &[i16]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    coords.hash(&mut h);
    h.finish()
}

/// `t` without its translation.
fn linear(t: Affine) -> Affine {
    let [a, b, c, d, _, _] = t.as_coeffs();
    Affine::new([a, b, c, d, 0.0, 0.0])
}

impl Atlas {
    /// Starts a frame.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        self.rasterized = 0;
    }

    /// Pages held.
    pub fn pages(&self) -> usize {
        self.pages.iter().flatten().count()
    }

    /// Places `glyph` of `run`, whose glyph space maps to the device by
    /// `transform` (the current transform times the run's), rasterizing it
    /// first if the atlas does not have it. `None` draws nothing (a
    /// degenerate transform or size).
    pub fn place(&mut self, run: &GlyphRun<'_>, transform: Affine, glyph: Glyph) -> Option<Placed> {
        let device = transform * Point::new(f64::from(glyph.x), f64::from(glyph.y));
        let scale = transform.determinant().abs().sqrt();
        if !(scale.is_finite() && scale > 0.0 && device.is_finite()) {
            return None;
        }
        // The rotation/skew left once the scale is folded into the size.
        let residual = linear(transform) * Affine::scale(1.0 / scale);
        let [a, b, c, d, _, _] = residual.as_coeffs();
        let aligned =
            (a - 1.0).abs() < 1e-4 && b.abs() < 1e-4 && c.abs() < 1e-4 && (d - 1.0).abs() < 1e-4;
        let size = f64::from(run.font_size) * scale;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "font sizes are small and positive"
        )]
        let size16 = (size * 16.0).round() as u32;
        if size16 == 0 {
            return None;
        }
        // Whole pixel and quarter-pixel bin of the origin, on each axis.
        let snap = |v: f64| {
            let mut whole = v.floor();
            let mut bin = ((v - whole) * BINS).round();
            if bin >= BINS {
                whole += 1.0;
                bin = 0.0;
            }
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "0..BINS"
            )]
            (whole, bin as u8)
        };
        let ((wx, bx), (wy, by)) = if aligned {
            (snap(device.x), snap(device.y))
        } else {
            ((device.x, 0), (device.y, 0))
        };
        let skew = run.glyph_transform.map_or([0; 4], |g| {
            let [a, b, c, d, _, _] = g.as_coeffs();
            [a, b, c, d].map(f64::to_bits)
        });
        let key = Key {
            font: run.font.data.id(),
            index: run.font.index,
            glyph: glyph.id,
            size: size16,
            coords: hash_of(run.normalized_coords),
            // Hinting (like vello's) needs an axis-aligned glyph with no
            // glyph transform: glifo drops the transform of a hinted glyph.
            hint: run.hint && aligned && run.glyph_transform.is_none(),
            skew,
            bin: (bx, by),
        };
        let frame = self.frame;
        if !self.cells.contains_key(&key) {
            let cell = self.rasterize(run, &key, f64::from(size16) / 16.0)?;
            self.cells.insert(key.clone(), cell);
        }
        let cell = self.cells.get_mut(&key).expect("present");
        cell.last_used = frame;
        let page = self.pages[cell.page]
            .as_mut()
            .expect("a cell's page is live");
        page.last_used = frame;
        let (x, y, w, h) = cell.rect;
        #[expect(
            clippy::cast_precision_loss,
            reason = "page coordinates are exact in f32"
        )]
        let uv = [
            x as f32 / PAGE as f32,
            y as f32 / PAGE as f32,
            (x + w) as f32 / PAGE as f32,
            (y + h) as f32 / PAGE as f32,
        ];
        let (w, h) = (f64::from(w), f64::from(h));
        let (ox, oy) = cell.origin;
        let corners = if aligned && cell.scale == 1.0 {
            let (x0, y0) = (wx - ox, wy - oy);
            [
                Point::new(x0, y0),
                Point::new(x0 + w, y0),
                Point::new(x0 + w, y0 + h),
                Point::new(x0, y0 + h),
            ]
        } else {
            let k = cell.scale;
            let at = |cx: f64, cy: f64| {
                device + (residual * Point::new((cx - ox) * k, (cy - oy) * k)).to_vec2()
            };
            [at(0.0, 0.0), at(w, 0.0), at(w, h), at(0.0, h)]
        };
        Some(Placed {
            page: cell.page,
            kind: page.kind,
            corners,
            uv,
        })
    }

    /// Rasterizes the glyph `key` names at `size` device pixels into a new
    /// cell.
    fn rasterize(&mut self, run: &GlyphRun<'_>, key: &Key, size: f64) -> Option<Cell> {
        let font = FontRef::from_index(run.font.data.data(), run.font.index).ok()?;
        let gid = GlyphId::new(key.glyph);
        let coords: Vec<F2Dot14> = run
            .normalized_coords
            .iter()
            .map(|c| F2Dot14::from_bits(*c))
            .collect();
        #[expect(clippy::cast_possible_truncation, reason = "font sizes fit in f32")]
        let size_f32 = size as f32;
        let color = font.color_glyphs().get(gid).is_some()
            || font
                .bitmap_strikes()
                .glyph_for_size(Size::new(size_f32), gid)
                .is_some();
        let kind = if color { Kind::Color } else { Kind::Mask };
        // Bounds in y-down pixels; a glyph without outline bounds (a bitmap)
        // gets a generous box.
        let metrics = font.glyph_metrics(Size::new(size_f32), LocationRef::new(&coords));
        let (mut x0, mut y0, mut x1, mut y1) = match metrics.bounds(gid) {
            Some(b) => (
                f64::from(b.x_min),
                -f64::from(b.y_max),
                f64::from(b.x_max),
                -f64::from(b.y_min),
            ),
            None => (-0.1 * size, -1.2 * size, 1.3 * size, 0.35 * size),
        };
        if let Some(g) = run.glyph_transform {
            // Either y convention: the union covers both.
            let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
            let flip = Affine::scale_non_uniform(1.0, -1.0);
            let mapped: Vec<Point> = corners
                .iter()
                .flat_map(|&(x, y)| [g * Point::new(x, y), flip * (g * (flip * Point::new(x, y)))])
                .collect();
            x0 = mapped.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
            y0 = mapped.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
            x1 = mapped.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max);
            y1 = mapped.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max);
        }
        let left = (x0 - MARGIN).floor();
        let top = (y0 - MARGIN).floor();
        // One more pixel for the subpixel shift (up to 3/4 px).
        let w = (x1 + MARGIN).ceil() - left + 1.0;
        let h = (y1 + MARGIN).ceil() - top + 1.0;
        if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
            return None;
        }
        // Oversized: rasterize smaller, draw scaled up.
        let k = (w.max(h) / f64::from(MAX_CELL)).ceil().max(1.0);
        if k > 1.0 {
            let mut cell = self.rasterize(
                run,
                &Key {
                    size: 0,
                    ..key.clone()
                },
                size / k,
            )?;
            cell.scale = k;
            return Some(cell);
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "cells are at most MAX_CELL"
        )]
        let (wu, hu) = (w as u32, h as u32);
        let origin = (-left, -top);
        let shift = (f64::from(key.bin.0) / BINS, f64::from(key.bin.1) / BINS);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "cells are at most MAX_CELL"
        )]
        let mut ctx = vello_cpu::RenderContext::new(wu as u16, hu as u16);
        let resources = self.resources.get_or_insert_with(vello_cpu::Resources::new);
        ctx.set_paint(peniko::Color::WHITE);
        ctx.set_transform(Affine::translate((origin.0 + shift.0, origin.1 + shift.1)));
        ctx.glyph_run(resources, run.font)
            .font_size(size_f32)
            .hint(key.hint)
            .normalized_coords(run.normalized_coords)
            // vello applies a glyph transform in the font's y-up space,
            // glifo in y-down: conjugate by the flip so a synthetic italic
            // leans right in both.
            .glyph_transform(run.glyph_transform.map_or(Affine::IDENTITY, |g| {
                let flip = Affine::scale_non_uniform(1.0, -1.0);
                flip * g * flip
            }))
            .fill_glyphs(std::iter::once(vello_cpu::Glyph {
                id: key.glyph,
                x: 0.0,
                y: 0.0,
            }));
        ctx.flush();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "cells are at most MAX_CELL"
        )]
        let mut pixmap = vello_cpu::Pixmap::new(wu as u16, hu as u16);
        ctx.render(&mut pixmap, resources);
        self.rasterized += 1;

        let (page, alloc, (ax, ay)) = self.allocate(kind, wu + 2 * PAD, hu + 2 * PAD)?;
        let bpp = if kind == Kind::Mask { 1 } else { 4 };
        let (pw, ph) = (wu + 2 * PAD, hu + 2 * PAD);
        let mut pixels = vec![0u8; (pw * ph * bpp) as usize];
        for (i, p) in pixmap.data().iter().enumerate() {
            #[expect(clippy::cast_possible_truncation, reason = "cells are small")]
            let (x, y) = (i as u32 % wu + PAD, i as u32 / wu + PAD);
            let at = ((y * pw + x) * bpp) as usize;
            match kind {
                Kind::Mask => pixels[at] = p.a,
                Kind::Color => pixels[at..at + 4].copy_from_slice(&[p.r, p.g, p.b, p.a]),
            }
        }
        self.uploads.push(Upload {
            page,
            rect: (ax, ay, pw, ph),
            pixels,
        });
        Some(Cell {
            page,
            alloc,
            rect: (ax + PAD, ay + PAD, wu, hu),
            origin,
            scale: 1.0,
            last_used: self.frame,
        })
    }

    /// Room for a `w`×`h` cell in a `kind` page: an existing page, then one
    /// freed of glyphs this frame does not use, then a new page.
    fn allocate(&mut self, kind: Kind, w: u32, h: u32) -> Option<(usize, AllocId, (u32, u32))> {
        #[expect(clippy::cast_possible_wrap, reason = "cells are small")]
        let size = size2(w as i32, h as i32);
        let try_pages = |pages: &mut Vec<Option<Page>>| {
            pages.iter_mut().enumerate().find_map(|(i, p)| {
                let p = p.as_mut().filter(|p| p.kind == kind)?;
                p.alloc.allocate(size).map(|a| (i, a))
            })
        };
        let found = try_pages(&mut self.pages).or_else(|| {
            // Evict this kind's glyphs the frame has not used.
            let frame = self.frame;
            let pages = &mut self.pages;
            self.cells.retain(|_, c| {
                let page = pages[c.page].as_mut().expect("live");
                if page.kind == kind && c.last_used < frame {
                    page.alloc.deallocate(c.alloc);
                    false
                } else {
                    true
                }
            });
            try_pages(&mut self.pages)
        });
        let (page, a) = match found {
            Some(f) => f,
            None => {
                #[expect(clippy::cast_possible_wrap, reason = "the page side fits in i32")]
                let mut alloc = AtlasAllocator::new(size2(PAGE as i32, PAGE as i32));
                let a = alloc.allocate(size)?;
                let page = Page {
                    kind,
                    alloc,
                    texture: None,
                    last_used: self.frame,
                };
                let slot = match self.pages.iter().position(Option::is_none) {
                    Some(i) => {
                        self.pages[i] = Some(page);
                        i
                    }
                    None => {
                        self.pages.push(Some(page));
                        self.pages.len() - 1
                    }
                };
                (slot, a)
            }
        };
        #[expect(clippy::cast_sign_loss, reason = "allocations lie inside the page")]
        let origin = (a.rectangle.min.x as u32, a.rectangle.min.y as u32);
        Some((page, a.id, origin))
    }

    /// Creates page textures and writes this frame's new cells into them.
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        for page in self.pages.iter_mut().flatten() {
            if page.texture.is_none() {
                let format = match page.kind {
                    Kind::Mask => wgpu::TextureFormat::R8Unorm,
                    Kind::Color => wgpu::TextureFormat::Rgba8Unorm,
                };
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("fenestra-raster glyph atlas"),
                    size: wgpu::Extent3d {
                        width: PAGE,
                        height: PAGE,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                page.texture = Some((texture, view));
            }
        }
        for up in self.uploads.drain(..) {
            let Some(page) = self.pages[up.page].as_ref() else {
                continue;
            };
            let Some((texture, _)) = &page.texture else {
                continue;
            };
            let bpp = if page.kind == Kind::Mask { 1 } else { 4 };
            let (x, y, w, h) = up.rect;
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                &up.pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * bpp),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    /// The texture of `page`.
    pub fn view(&self, page: usize) -> Option<&wgpu::TextureView> {
        self.pages
            .get(page)?
            .as_ref()?
            .texture
            .as_ref()
            .map(|(_, v)| v)
    }

    /// After a frame: beyond [`KEEP_PAGES`], pages it did not use go, with
    /// their glyphs.
    pub fn trim(&mut self) {
        let frame = self.frame;
        while self.pages() > KEEP_PAGES {
            let Some(i) = self
                .pages
                .iter()
                .position(|p| p.as_ref().is_some_and(|p| p.last_used < frame))
            else {
                break;
            };
            self.pages[i] = None;
            self.cells.retain(|_, c| c.page != i);
        }
    }
}
