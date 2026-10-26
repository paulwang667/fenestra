// fenestra-raster shaders. Colors are premultiplied, in sRGB-encoded values
// (the same space vello blends in); every pipeline blends One /
// OneMinusSrcAlpha.

// The device-space rectangle the current pass renders: the whole frame for
// the root, a layer's bounds for an offscreen layer.
struct Globals { origin: vec2f, size: vec2f };
@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var ramps: texture_2d<f32>;

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

const TAU: f32 = 6.28318530718;

// vello's extend_mode_normalized (fine.wgsl).
fn extend(t: f32, mode: u32) -> f32 {
    switch mode {
        case 0u: { return clamp(t, 0.0, 1.0); }
        case 1u: { return fract(t); }
        default: { return abs(t - 2.0 * round(0.5 * t)); }
    }
}

@fragment fn fs_paint(i: PaintOut) -> @location(0) vec4f {
    let kind = u32(i.grad2.x + 0.5);
    if kind == 0u {
        return i.color;
    }
    let p = i.local;
    var t = 0.0;
    if kind == 1u {
        let d = i.grad.zw - i.grad.xy;
        let len2 = dot(d, d);
        t = select(dot(p - i.grad.xy, d) / len2, 0.0, len2 == 0.0);
    } else if kind == 2u {
        t = length(p - i.grad.xy) / i.grad.z;
    } else {
        // vello's unit angle: clockwise from +x in y-down space, in turns.
        let v = p - i.grad.xy;
        var phi = atan2(v.y, v.x) / TAU;
        phi = select(phi, phi + 1.0, phi < 0.0);
        t = (phi - i.grad.z) / (i.grad.w - i.grad.z);
    }
    t = extend(t, u32(i.grad2.z + 0.5));
    let x = i32(round(t * 511.0));
    return textureLoad(ramps, vec2(x, i32(i.grad2.y + 0.5)), 0) * i.color.a;
}
