//! Feasibility proof: a 3D component rendered natively inside a fenestra UI,
//! via Approach B (off-screen 3D → readback → `image_rgba8`).
//!
//! A CPU software rasterizer (no GPU dependency) renders a rotating, flat-shaded
//! cube to an RGBA8 buffer on a worker thread (`Cmd::task`). The result is
//! cached as an `ImageData` (Arc'd blob — identity-compared across rebuilds, so
//! vello skips re-upload when the frame hasn't changed). A `Sub::every` ticker
//! advances the rotation angle ~60 Hz.
//!
//! The surrounding UI — title, stats overlay, controls — is pure fenestra 2D.
//! The 3D image participates in flexbox layout like any other element.
//!
//! `cargo run --example scene3d`               windowed (Cmd::task async + Sub::every)
//! `cargo run --example scene3d -- --shot`     headless PNG → gallery/scene3d.png
//! `cargo run --example scene3d -- --shot-wide` wider layout, 3D fills via responsive()

use std::time::Duration;

use fenestra::prelude::*;
use fenestra::shell::{WindowOptions, render_element};

// ───────────────────────── 3D math (minimal, no deps) ─────────────────────

type Vec3 = [f32; 3];


fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(a: Vec3) -> Vec3 {
    let len = dot(a, a).sqrt().max(1e-10);
    [a[0] / len, a[1] / len, a[2] / len]
}

/// Rotation matrix from Euler angles (radians), row-major.
fn rot_matrix(rx: f32, ry: f32) -> [[f32; 3]; 3] {
    let (cx, sx) = (rx.cos(), rx.sin());
    let (cy, sy) = (ry.cos(), ry.sin());
    // R_y * R_x  (yaw then pitch)
    [
        [cy, 0.0, sy],
        [sx * sy, cx, -sx * cy],
        [-cx * sy, sx, cx * cy],
    ]
}

fn transform(m: [[f32; 3]; 3], v: Vec3) -> Vec3 {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

// ───────────────────────── cube geometry ───────────────────────────────────

/// 8 corners of a unit cube centered at origin, edge length 1.0.
const CUBE_VERTS: [Vec3; 8] = [
    [-0.5, -0.5, -0.5],
    [ 0.5, -0.5, -0.5],
    [ 0.5,  0.5, -0.5],
    [-0.5,  0.5, -0.5],
    [-0.5, -0.5,  0.5],
    [ 0.5, -0.5,  0.5],
    [ 0.5,  0.5,  0.5],
    [-0.5,  0.5,  0.5],
];

/// 12 triangles (2 per face), indices into CUBE_VERTS, plus a base color.
struct Tri {
    vi: [usize; 3],
    color: [u8; 3],
}

const CUBE_TRIS: [Tri; 12] = [
    // -Z (back, dark blue)
    Tri { vi: [0, 2, 1], color: [40, 70, 120] },
    Tri { vi: [0, 3, 2], color: [40, 70, 120] },
    // +Z (front, light blue)
    Tri { vi: [4, 5, 6], color: [80, 130, 200] },
    Tri { vi: [4, 6, 7], color: [80, 130, 200] },
    // -X (left, dark green)
    Tri { vi: [0, 4, 7], color: [40, 100, 60] },
    Tri { vi: [0, 7, 3], color: [40, 100, 60] },
    // +X (right, light green)
    Tri { vi: [1, 2, 6], color: [70, 160, 90] },
    Tri { vi: [1, 6, 5], color: [70, 160, 90] },
    // -Y (bottom, dark red)
    Tri { vi: [0, 1, 5], color: [100, 40, 40] },
    Tri { vi: [0, 5, 4], color: [100, 40, 40] },
    // +Y (top, light red — the "lit" face)
    Tri { vi: [3, 7, 6], color: [200, 80, 80] },
    Tri { vi: [3, 6, 2], color: [200, 80, 80] },
];

// ───────────────────────── software rasterizer ─────────────────────────────

/// Renders the rotating cube to a straight-alpha RGBA8 buffer.
///
/// Pipeline: rotate vertices → perspective project → back-face cull →
/// painter-sort by mean depth → scanline fill with flat shading.
fn render_cube(angle_x: f32, angle_y: f32, w: u32, h: u32) -> Vec<u8> {
    let w = w as usize;
    let h = h as usize;
    let mut buf = vec![0u8; w * h * 4];

    // Camera: looking down -Z, cube at distance 3.0.
    let cam_z = 3.0f32;
    let fov = 1.4f32; // radians (~80°)
    let focal = 0.5 * (w.min(h)) as f32 / (0.5 * fov).tan();

    let rm = rot_matrix(angle_x, angle_y);
    let light = normalize([-0.4, -0.8, -1.0]);

    // Transform vertices.
    let projected: Vec<(Vec3, Vec3)> = CUBE_VERTS
        .iter()
        .map(|&v| {
            let world = transform(rm, v);
            // Perspective: screen_x = focal * x / (cam_z - z), etc.
            let z = cam_z - world[2];
            let sx = focal * world[0] / z + w as f32 * 0.5;
            let sy = -focal * world[1] / z + h as f32 * 0.5;
            ([sx, sy, world[2]], world)
        })
        .collect();

    // Collect visible triangles (back-face cull).
    let mut tris: Vec<([Vec3; 3], Vec3, [u8; 3], f32)> = CUBE_TRIS
        .iter()
        .filter_map(|t| {
            let a = projected[t.vi[0]].0;
            let b = projected[t.vi[1]].0;
            let c = projected[t.vi[2]].0;
            // Screen-space cross product (CCW = front-facing).
            let edge = cross(
                [b[0] - a[0], b[1] - a[1], 0.0],
                [c[0] - a[0], c[1] - a[1], 0.0],
            );
            if edge[2] <= 0.0 {
                return None; // back-facing
            }
            let wa = projected[t.vi[0]].1;
            let wb = projected[t.vi[1]].1;
            let wc = projected[t.vi[2]].1;
            let normal = normalize(cross(sub(wb, wa), sub(wc, wa)));
            let mean_z = (a[2] + b[2] + c[2]) / 3.0;
            Some(([a, b, c], normal, t.color, mean_z))
        })
        .collect();

    // Painter's algorithm: far-to-near (larger z = farther).
    tris.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal));

    // Rasterize each triangle.
    for (pts, normal, base, _) in &tris {
        // Flat shading: lambert + ambient.
        let lambert = dot(*normal, light).max(0.0);
        let shade = 0.25 + 0.75 * lambert;
        let r = (base[0] as f32 * shade).clamp(0.0, 255.0) as u8;
        let g = (base[1] as f32 * shade).clamp(0.0, 255.0) as u8;
        let b = (base[2] as f32 * shade).clamp(0.0, 255.0) as u8;

        fill_triangle(&mut buf, w, h, pts[0], pts[1], pts[2], r, g, b);
    }

    // Wireframe edges on top for clarity.
    for pts in &tris {
        let [a, b, c] = pts.0;
        draw_line(&mut buf, w, h, a, b, 200, 200, 210);
        draw_line(&mut buf, w, h, b, c, 200, 200, 210);
        draw_line(&mut buf, w, h, c, a, 200, 200, 210);
    }

    buf
}

/// Barycentric scanline triangle fill with simple bounds clipping.
fn fill_triangle(
    buf: &mut [u8], w: usize, h: usize,
    a: Vec3, b: Vec3, c: Vec3,
    r: u8, g: u8, bl: u8,
) {
    let min_x = a[0].min(b[0]).min(c[0]).floor() as i32;
    let max_x = a[0].max(b[0]).max(c[0]).ceil() as i32;
    let min_y = a[1].min(b[1]).min(c[1]).floor() as i32;
    let max_y = a[1].max(b[1]).max(c[1]).ceil() as i32;

    let min_x = min_x.max(0) as usize;
    let max_x = (max_x as usize).min(w.saturating_sub(1));
    let min_y = min_y.max(0) as usize;
    let max_y = (max_y as usize).min(h.saturating_sub(1));

    let area = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
    if area.abs() < 1e-6 {
        return; // degenerate
    }
    let inv_area = 1.0 / area;

    for y in min_y..=max_y {
        let py = y as f32 + 0.5;
        for x in min_x..=max_x {
            let px = x as f32 + 0.5;
            let w0 = ((b[0] - px) * (c[1] - py) - (c[0] - px) * (b[1] - py)) * inv_area;
            let w1 = ((c[0] - px) * (a[1] - py) - (a[0] - px) * (c[1] - py)) * inv_area;
            let w2 = 1.0 - w0 - w1;
            if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                let idx = (y * w + x) * 4;
                buf[idx] = r;
                buf[idx + 1] = g;
                buf[idx + 2] = bl;
                buf[idx + 3] = 255;
            }
        }
    }
}

/// Bresenham line draw.
fn draw_line(
    buf: &mut [u8], w: usize, h: usize,
    a: Vec3, b: Vec3,
    r: u8, g: u8, bl: u8,
) {
    let (mut x0, mut y0) = (a[0] as i32, a[1] as i32);
    let (x1, y1) = (b[0] as i32, b[1] as i32);
    let dx = (x1 - x0).abs();
    let dy = (y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx - dy;
    loop {
        if x0 >= 0 && x0 < w as i32 && y0 >= 0 && y0 < h as i32 {
            let idx = (y0 as usize * w + x0 as usize) * 4;
            buf[idx] = r;
            buf[idx + 1] = g;
            buf[idx + 2] = bl;
            buf[idx + 3] = 255;
        }
        if x0 == x1 && y0 == y1 { break; }
        let e2 = 2 * err;
        if e2 > -dy { err -= dy; x0 += sx; }
        if e2 < dx { err += dx; y0 += sy; }
    }
}

// ───────────────────────── fenestra App ────────────────────────────────────

/// Render resolution (physical px). The element is styled to this logical size
/// at scale 1.0, so pixels map 1:1 in the headless shot. In a real app you'd
/// multiply by the window's `scale_factor` for retina.
const RW: u32 = 400;
const RH: u32 = 300;

#[derive(Clone)]
enum Msg {
    /// Tick from the `Sub::every` rotation clock.
    Tick,
    /// 3D render completed on a worker thread.
    Rendered(ImageData),
}

struct Scene3d {
    angle_x: f32,
    angle_y: f32,
    /// Cached render: identity-compared (Arc blob) across view rebuilds,
    /// so vello skips texture re-upload when this hasn't changed.
    cached: Option<ImageData>,
    /// Tier 2: when true, the canvas fills its container via `responsive()`
    /// instead of a fixed size. The closure receives the container's
    /// measured size from the previous frame (one-frame deferred).
    responsive: bool,
}

impl App for Scene3d {
    type Msg = Msg;

    fn update(&mut self, _: Msg) {}

    fn update_with(&mut self, msg: Msg) -> Cmd<Msg> {
        match msg {
            Msg::Tick => {
                self.angle_y += 0.03;
                self.angle_x += 0.011;
                // Render on a worker thread — blocking compute is fine here.
                let (ax, ay) = (self.angle_x, self.angle_y);
                Cmd::task(move || {
                    let pixels = render_cube(ax, ay, RW, RH);
                    Msg::Rendered(image_payload(RW, RH, pixels))
                })
            }
            Msg::Rendered(data) => {
                self.cached = Some(data);
                Cmd::none()
            }
        }
    }

    /// Continuous rotation while the app runs.
    fn subscriptions(&self) -> Vec<Sub<Msg>> {
        vec![Sub::every("spin", Duration::from_millis(16), || Msg::Tick)]
    }

    fn view(&self) -> Element<Msg> {
        let border = Color::from_rgba8(60, 60, 70, 255);
        let placeholder = Color::from_rgba8(20, 22, 28, 255);
        let angle_deg = self.angle_y.to_degrees() % 360.0;

        // Tier 1: fixed-size canvas. Tier 2: `responsive()` wrapper that
        // receives the container's measured size from the previous frame
        // and stretches the 3D image to fill it (one-frame deferred: first
        // frame uses the hint, then converges to the real size).
        let canvas: Element<Msg> = if self.responsive {
            let cached = self.cached.clone();
            responsive_hinted(
                (RW as f32, RH as f32),
                move |(w, h)| match &cached {
                    Some(data) => image_from_data(data.clone())
                        .w(w)
                        .h(h)
                        .rounded(8.0)
                        .border(1.0, border),
                    None => div().w(w).h(h).bg(placeholder).rounded(8.0),
                },
            )
        } else {
            match &self.cached {
                Some(data) => image_from_data(data.clone())
                    .rounded(8.0)
                    .border(1.0, border),
                None => div().w(RW as f32).h(RH as f32).bg(placeholder),
            }
        };

        let mode_label = if self.responsive { "responsive" } else { "fixed" };
        col()
            .p(SP6)
            .gap(SP4)
            .items_center()
            .children((
                text("3D in fenestra")
                    .size(TextSize::Xl)
                    .weight(Weight::Semibold),
                canvas,
                text(format!(
                    "yaw: {angle_deg:.0}°  ·  {mode_label}  ·  CPU rasterizer → image_rgba8"
                ))
                .size(TextSize::Sm)
                .color(Color::from_rgba8(140, 140, 150, 255)),
            ))
    }
}

/// Counts cube-face pixels and reports the bounding box, for shot verification.
fn report_pixels(img: &image::RgbaImage, label: &str) {
    let (w, h) = img.dimensions();
    let opaque = img.chunks_exact(4).filter(|p| p[3] > 0).count();
    let (mut blue, mut green, mut red) = (0u32, 0u32, 0u32);
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (w, 0u32, h, 0u32);
    for y in 0..h {
        for x in 0..w {
            let p = img.get_pixel(x, y);
            let (r, g, b) = (p[0], p[1], p[2]);
            let hit = if b > r + 10 && b > g + 10 {
                blue += 1; true
            } else if g > r + 10 && g > b + 10 {
                green += 1; true
            } else if r > g + 15 && r > b + 15 && r > 50 {
                red += 1; true
            } else {
                false
            };
            if hit {
                min_x = min_x.min(x); max_x = max_x.max(x);
                min_y = min_y.min(y); max_y = max_y.max(y);
            }
        }
    }
    let total = blue + green + red;
    println!("{label}: {w}x{h}, {opaque} opaque, cube={total} (b={blue} g={green} r={red})");
    if total > 0 {
        println!("  bbox: ({min_x},{min_y})-({max_x},{max_y}) {}x{}",
            max_x - min_x + 1, max_y - min_y + 1);
    }
    let cx = w / 2;
    let cy = h / 2;
    let p = img.get_pixel(cx, cy);
    println!("  center ({cx},{cy}): RGBA({},{},{},{})", p[0], p[1], p[2], p[3]);
}

// ───────────────────────── Tier 3: PassKind::Custom (windowed) ────────────
//
// Same rotating cube, but instead of rendering on a worker thread and
// pushing an `ImageData` through `Cmd::task` (Tier 2), the app registers
// a custom-render closure via `App::custom_render`. The shell calls it
// during the two-pass render path when it encounters an element styled
// with `.custom_render(key)`. The returned `ImageData` replaces the
// element's subtree in the final compositing pass.

/// Render key for the cube. The shell looks this up in the custom-render
/// registry; any element styled with `.custom_render(CUBE_KEY)` is
/// replaced by the cube render.
const CUBE_KEY: u64 = 1;

struct Scene3dCustom {
    /// Shared between the app and the `'static` custom-render closure
    /// (which can't borrow `self`). The closure reads the current angles
    /// from here every frame.
    angles: std::sync::Arc<std::sync::Mutex<(f32, f32)>>,
}

#[derive(Clone)]
enum MsgCustom {
    Tick,
}

impl App for Scene3dCustom {
    type Msg = MsgCustom;

    fn update(&mut self, _: MsgCustom) {}

    fn update_with(&mut self, msg: MsgCustom) -> Cmd<MsgCustom> {
        match msg {
            MsgCustom::Tick => {
                let angles = std::sync::Arc::clone(&self.angles);
                let mut guard = angles.lock().expect("angles mutex");
                guard.0 += 0.011;
                guard.1 += 0.03;
                Cmd::none()
            }
        }
    }

    fn subscriptions(&self) -> Vec<Sub<MsgCustom>> {
        vec![Sub::every("spin-custom", Duration::from_millis(16), || MsgCustom::Tick)]
    }

    /// Registers the cube renderer. The shell calls this closure (on a
    /// worker thread, during the two-pass render) for every element
    /// styled with `.custom_render(CUBE_KEY)`. We render the rotating
    /// cube synchronously to a fresh `ImageData`.
    fn custom_render(
        &self,
    ) -> Option<std::sync::Arc<dyn Fn(u64, u32, u32) -> Option<ImageData> + Send + Sync>>
    {
        let angles = std::sync::Arc::clone(&self.angles);
        Some(std::sync::Arc::new(move |_key, w, h| {
            let (ax, ay) = *angles.lock().expect("angles mutex");
            let pixels = render_cube(ax, ay, w, h);
            Some(image_payload(w, h, pixels))
        }))
    }

    fn view(&self) -> Element<MsgCustom> {
        let border = Color::from_rgba8(60, 60, 70, 255);
        let placeholder = Color::from_rgba8(20, 22, 28, 255);
        let (_ax, ay) = *self.angles.lock().expect("angles mutex");
        let angle_deg = ay.to_degrees() % 360.0;

        // The element styled with `.custom_render(CUBE_KEY)` is replaced
        // by the cube's GPU-rendered image in the final pass. The
        // placeholder div provides the layout box.
        let cube = div()
            .w(RW as f32)
            .h(RH as f32)
            .bg(placeholder)
            .rounded(8.0)
            .border(1.0, border)
            .custom_render(CUBE_KEY);

        col()
            .p(SP6)
            .gap(SP4)
            .items_center()
            .children((
                text("3D in fenestra (Tier 3: PassKind::Custom)")
                    .size(TextSize::Xl)
                    .weight(Weight::Semibold),
                cube,
                text(format!(
                    "yaw: {angle_deg:.0}°  ·  PassKind::Custom  ·  shell calls App::custom_render"
                ))
                .size(TextSize::Sm)
                .color(Color::from_rgba8(140, 140, 150, 255)),
            ))
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--custom") {
        // Tier 3: PassKind::Custom in windowed mode. The shell calls
        // `App::custom_render` during the two-pass render path; the
        // returned image replaces the cube element's subtree.
        fenestra::run(
            Scene3dCustom {
                angles: std::sync::Arc::new(std::sync::Mutex::new((0.4, 0.7))),
            },
            WindowOptions::titled("fenestra 3D — Custom pass").with_size(480.0, 420.0),
        );
    } else if args.iter().any(|a| a == "--shot-wide") {
        // Tier 2 headless: wider window, 3D fills via responsive().
        let rw = 640u32;
        let rh = 300u32;
        let mut app = Scene3d {
            angle_x: 0.4,
            angle_y: 0.7,
            cached: None,
            responsive: true,
        };
        let pixels = render_cube(app.angle_x, app.angle_y, rw, rh);
        app.cached = Some(image_payload(rw, rh, pixels));
        let view = app.view();
        let theme = Theme::dark();
        let img = render_element(view, &theme, (720, 420));
        let path = std::path::Path::new("gallery/scene3d_wide.png");
        println!("saved {} (responsive, 720x420)", path.display());
        report_pixels(&img, "responsive 720x420");
    } else if args.iter().any(|a| a == "--shot") {
        // Tier 1 headless: fixed-size 3D element.
        let mut app = Scene3d {
            angle_x: 0.4,
            angle_y: 0.7,
            cached: None,
            responsive: false,
        };
        let pixels = render_cube(app.angle_x, app.angle_y, RW, RH);
        app.cached = Some(image_payload(RW, RH, pixels));
        let view = app.view();
        let theme = Theme::dark();
        let img = render_element(view, &theme, (480, 420));
        let path = std::path::Path::new("gallery/scene3d.png");
        println!("saved {} (fixed, 480x420)", path.display());
        report_pixels(&img, "fixed 480x420");
    } else {
        // Tier 2 windowed (default): Cmd::task + image_rgba8.
        fenestra::run(
            Scene3d {
                angle_x: 0.4,
                angle_y: 0.7,
                cached: None,
                responsive: false,
            },
            WindowOptions::titled("fenestra 3D").with_size(480.0, 420.0),
        );
    }
}
