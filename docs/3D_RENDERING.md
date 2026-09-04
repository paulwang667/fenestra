# 3D rendering in fenestra

fenestra's rendering backend is vello — a 2D GPU renderer with no depth
buffer, no rasterizer, no custom shaders. A `vello::Scene` is a flat list of
paint commands (paths, images, text) rasterized by vello's own pipeline.
There is no place to put a 3D scene inside it.

That does not mean fenestra cannot display 3D. The integration model is
**off-screen rendering → pixel readback → `image_rgba8`**. Anything you can
rasterize to RGBA8 — CPU software rasterizer, wgpu 3D pipeline, external
renderer — becomes a fenestra element indistinguishable from a `<img>`.

This document covers the three tiers, from zero-framework-change to a
proper GPU pipeline extension.

---

## Tier 1: `image_rgba8` (zero framework changes)

The fastest path. Render your 3D scene to an RGBA8 buffer, wrap it in an
`ImageData`, display it as a static image element.

```rust
use fenestra::prelude::*;

// Your 3D renderer produces a Vec<u8> of w * h * 4 RGBA8 bytes.
fn render_3d_scene(angle: f32, w: u32, h: u32) -> Vec<u8> {
    // ... perspective projection, rasterization, shading ...
    vec![0u8; (w * h * 4) as usize]
}

let pixels = render_3d_scene(0.7, 400, 300);
let el = image_rgba8(400, 300, pixels)
    .rounded(8.0)
    .border(1.0, Color::from_rgba8(60, 60, 70, 255));
```

`image_rgba8` is equivalent to `image_from_data(image_payload(...))` — it
wraps the pixel buffer as an `ImageData` and builds a `Kind::Image` element.
The element participates in flexbox layout, receives all normal styling
(padding, margin, rounded corners, border, shadow, transitions), and
paints through vello's `draw_image` path.

**When to use:** static 3D thumbnails, pre-rendered diagrams, one-shot
visualizations where the 3D content doesn't change at runtime.

---

## Tier 2: `Cmd::task` + `responsive()` (zero framework changes)

Animate the 3D content at ~60 Hz by rendering on a worker thread and
feeding the result back through the Elm message loop. Stretch the canvas
to fill its container with `responsive()`.

```rust
use std::time::Duration;
use fenestra::prelude::*;

#[derive(Clone)]
enum Msg {
    Tick,
    Rendered(ImageData),
}

struct App3d {
    angle: f32,
    cached: Option<ImageData>,
}

impl App for App3d {
    type Msg = Msg;

    fn update_with(&mut self, msg: Msg) -> Cmd<Msg> {
        match msg {
            Msg::Tick => {
                self.angle += 0.03;
                let (ax, w, h) = (self.angle, 400u32, 300u32);
                Cmd::task(move || {
                    let pixels = render_3d_scene(ax, w, h);
                    Msg::Rendered(image_payload(w, h, pixels))
                })
            }
            Msg::Rendered(data) => {
                self.cached = Some(data);
                Cmd::none()
            }
        }
    }

    fn subscriptions(&self) -> Vec<Sub<Msg>> {
        vec![Sub::every("rotate", Duration::from_millis(16), || Msg::Tick)]
    }

    fn view(&self) -> Element<Msg> {
        let border = Color::from_rgba8(60, 60, 70, 255);
        match &self.cached {
            Some(data) => image_from_data(data.clone())
                .rounded(8.0)
                .border(1.0, border),
            None => div().w(400.0).h(300.0).bg(Color::from_rgba8(20, 22, 28, 255)),
        }
    }
}
```

`Cmd::task` moves the closure to a background thread — blocking compute is
safe there and never stalls the UI. The `Proxy` delivers the result back
as a message, triggering the next view rebuild.

### Container-adaptive: `responsive()`

For a canvas that fills its container at runtime, wrap the element in
`responsive()`. The closure receives the container's measured size from
the previous frame:

```rust
responsive_hinted(
    (400.0, 300.0), // hint for the first frame
    move |(w, h)| match &self.cached {
        Some(data) => image_from_data(data.clone()).w(w).h(h).rounded(8.0),
        None => div().w(w).h(h).bg(placeholder).rounded(8.0),
    },
)
```

The 3D render resolution must match the target size. Since `responsive()`
delivers the container size one frame late, the first frame uses the
hint and subsequent frames converge to the measured size. The 3D content
scales to fill whatever size the container provides.

**When to use:** animated 3D views (rotating models, live simulations)
where pixel-perfect resolution matching matters.

**Trade-off:** one-frame latency on size changes; 3D pixels are CPU-side
readback (wgpu memory copy), not zero-copy texture references.

---

## Tier 3: `PassKind::Custom` (3 files changed)

For GPU-accelerated 3D rendering that participates in fenestra's multi-pass
pipeline. The element's subtree is replaced by a GPU-rendered texture —
the same compositing path as `BackdropBlur` and `ElementFilter`.

### What changed

| File | Change |
|---|---|
| `fenestra-core/src/paint_plan.rs` | `PassKind::Custom { render_key: u64, cache_key: u64 }` — pure data, two u64s |
| `fenestra-shell/src/multi_pass.rs` | `process_specs` gains `custom: &dyn Fn(u64, u32, u32) -> Option<peniko::ImageData>` |
| `fenestra-shell/src/headless.rs` | `render_plan` passes `&\|_, _, _\| None` as the default closure |

### How it works

```
                        ┌─────────────────────────────────────────┐
                        │  Frame::paint_backdrop()               │
                        │  (PaintMode::Backdrop)                  │
                        │                                         │
                        │  For each Custom spec:                  │
                        │    → emit MultiPassSpec {               │
                        │        id, rect,                       │
                        │        kind: Custom {                  │
                        │            render_key, cache_key       │
                        │        }                               │
                        │    }                                   │
                        │    → skip subtree painting             │
                        └────────────────┬────────────────────────┘
                                         │
                                         ▼
                        ┌─────────────────────────────────────────┐
                        │  shell: render backdrop scene           │
                        │  → read back pixels (wgpu copy)         │
                        └────────────────┬────────────────────────┘
                                         │
                                         ▼
                        ┌─────────────────────────────────────────┐
                        │  multi_pass::process_specs(backdrop,     │
                        │    specs, scale, custom_closure)         │
                        │                                         │
                        │  For each PassKind::Custom spec:         │
                        │    → call custom(render_key, w, h)      │
                        │    → closure looks up the render fn     │
                        │    → closure returns Some(ImageData)    │
                        │      or None (paint normally)           │
                        └────────────────┬────────────────────────┘
                                         │
                                         ▼
                        ┌─────────────────────────────────────────┐
                        │  Frame::paint_final(injected_images)    │
                        │  (PaintMode::Final)                     │
                        │  → composite injected image over the    │
                        │    backdrop at the element's rect       │
                        └─────────────────────────────────────────┘
```

The shell supplies the `custom` closure with device/queue access. Core
stays wgpu-free: `PassKind::Custom` is just two u64s.

### Registering a render function

```rust
// The shell's custom render closure (pseudocode — your app wires this up):
let custom_render = |render_key: u64, w: u32, h: u32| -> Option<peniko::ImageData> {
    match render_key {
        1 => render_glass_cube(device, queue, w, h),   // wgpu 3D pipeline
        2 => render_heatmap(device, queue, w, h),       // wgpu compute → texture
        _ => None,                                       // no renderer → paint normally
    }
};
```

### Cache invalidation

`cache_key` controls re-rendering. When the shell sees the same `cache_key`
as the previous frame, it skips calling the render function and reuses the
cached image. Change the key whenever the render output should change:

```rust
// Recompute cache_key each frame — only call GPU when camera moves or mesh changes.
let cache_key = hash(self.camera.position, self.camera.rotation, self.mesh.id);
```

---

## The example: `cargo run --example scene3d`

A complete working example combining Tier 1 + Tier 2:

```
fenestra/examples/scene3d.rs
├── render_cube(angle_x, angle_y, w, h) → Vec<u8>
│   ├── perspective projection (camera at z=3.0, FOV ~80°)
│   ├── back-face cull (screen-space cross product)
│   ├── painter's algorithm (sort by mean depth, far-to-near)
│   ├── flat shading (Lambert + ambient, 25% base)
│   ├── barycentric scanline triangle fill
│   └── Bresenham wireframe overlay (color 200,200,210)
├── Scene3d struct
│   ├── angle_x, angle_y: f32
│   ├── cached: Option<ImageData> (Arc'd, identity-compared)
│   └── responsive: bool (Tier 2 flag)
├── impl App for Scene3d
│   ├── update_with: Cmd::task → render_cube on worker thread
│   ├── subscriptions: Sub::every(16ms) → Msg::Tick
│   └── view: div() + text() + image_from_data() (flexbox layout)
└── main()
    ├── --shot → headless render at 480×420, save PNG
    ├── --shot-wide → headless render at 720×420, responsive()
    └── default → windowed app (Cmd::task async + Sub::every)
```

Run:

```bash
cargo run --example scene3d                # windowed, animated
cargo run --example scene3d -- --shot      # headless 480×420, fixed
cargo run --example scene3d -- --shot-wide # headless 720×420, responsive
```

---

## Limitations

| Limitation | Detail |
|---|---|
| No GPU texture sharing | Tier 1/2 readback copies pixels from GPU to CPU memory each frame. |
| No depth buffer / occlusion | Software rasterizer uses painter's algorithm. For real depth, use wgpu. |
| No hit-test pick from 3D | 3D interaction requires a parallel CPU-side ray-triangle intersection. |
| Windowed runner uses single-pass | `PassKind::Custom` works in headless mode (two-pass `render_plan`). Windowed integration requires making `present()` switch to two-pass when Custom specs exist — same architectural change needed for backdrop blur in windowed mode. |
| One-frame latency | `responsive()` and `Cmd::task` both deliver results one frame behind. |

---

## Choosing a tier

| Need | Tier |
|---|---|
| Static 3D thumbnail, diagram | **Tier 1** — `image_rgba8`, zero cost |
| Animated 3D, CPU rasterizer, fixed size | **Tier 2** — `Cmd::task` + `image_from_data` |
| Animated 3D, container-adaptive | **Tier 2** — `Cmd::task` + `responsive()` |
| GPU-accelerated 3D, headless/golden | **Tier 3** — `PassKind::Custom` + wgpu |
| GPU-accelerated 3D, windowed interactive | **Tier 3 +** — custom render registry + windowed two-pass wiring |
