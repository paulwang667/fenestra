// fenestra-raster shaders. Colors are premultiplied, in sRGB-encoded values
// (the same space vello blends in); every pipeline blends One /
// OneMinusSrcAlpha.

// The device-space rectangle the current pass renders: the whole frame for
// the root, a layer's bounds for an offscreen layer.
struct Globals { origin: vec2f, size: vec2f };
@group(0) @binding(0) var<uniform> g: Globals;

fn to_clip(p: vec2f) -> vec4f {
    let q = (p - g.origin) / g.size;
    return vec4f(q.x * 2.0 - 1.0, 1.0 - q.y * 2.0, 0.0, 1.0);
}

// ---------------------------------------------------------------- paint

struct PaintIn {
    @location(0) pos: vec2f,
    @location(1) local: vec2f,
    @location(2) color: vec4f,
    @location(3) grad: vec4f,
    @location(4) grad2: vec4f,
};

struct PaintOut {
    @builtin(position) pos: vec4f,
    @location(0) local: vec2f,
    @location(1) color: vec4f,
    @location(2) @interpolate(flat) grad: vec4f,
    @location(3) @interpolate(flat) grad2: vec4f,
};

@vertex fn vs_paint(v: PaintIn) -> PaintOut {
    return PaintOut(to_clip(v.pos), v.local, v.color, v.grad, v.grad2);
}

@fragment fn fs_paint(i: PaintOut) -> @location(0) vec4f {
    return i.color;
}
