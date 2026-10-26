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
        var v = p - i.grad.xy;
        // The seam at angle 0 is a jump in the ramp: vello evaluates exactly
        // on it (y = 0) where an interpolated coordinate lands a hair off,
        // which would flip a whole row to the ramp's other end. A row a real
        // fraction of a pixel off it stays off it, however far it reaches.
        v.y = select(v.y, 0.0, abs(v.y) < 1e-3);
        var phi = atan2(v.y, v.x) / TAU;
        phi = select(phi, phi + 1.0, phi < 0.0);
        t = (phi - i.grad.z) / (i.grad.w - i.grad.z);
    }
    t = extend(t, u32(i.grad2.z + 0.5));
    let x = i32(round(t * 511.0));
    return textureLoad(ramps, vec2(x, i32(i.grad2.y + 0.5)), 0) * i.color.a;
}

// Writes only the stencil (clip shapes).
@fragment fn fs_stencil(i: PaintOut) -> @location(0) vec4f {
    return vec4f(0.0);
}

// ---------------------------------------------------------------- textured quads

@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct ImageIn {
    @location(0) pos: vec2f,
    @location(1) uv: vec2f,
    @location(2) tint: vec4f,
};

struct ImageOut {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f,
    @location(1) tint: vec4f,
};

@vertex fn vs_image(v: ImageIn) -> ImageOut {
    return ImageOut(to_clip(v.pos), v.uv, v.tint);
}

// Textures hold premultiplied color.
@fragment fn fs_image(i: ImageOut) -> @location(0) vec4f {
    return textureSample(tex, samp, i.uv) * i.tint;
}

// Glyph coverage (an R8 atlas page) tinted by the run's color.
@fragment fn fs_mask(i: ImageOut) -> @location(0) vec4f {
    return textureSample(tex, samp, i.uv).r * i.tint;
}

// ---------------------------------------------------------------- shadows

struct ShadowIn {
    @location(0) pos: vec2f,
    @location(1) local: vec2f,
    @location(2) size: vec2f,
    @location(3) params: vec2f,
    @location(4) color: vec4f,
};

struct ShadowOut {
    @builtin(position) pos: vec4f,
    @location(0) local: vec2f,
    @location(1) @interpolate(flat) size: vec2f,
    @location(2) @interpolate(flat) params: vec2f,
    @location(3) @interpolate(flat) color: vec4f,
};

@vertex fn vs_shadow(v: ShadowIn) -> ShadowOut {
    return ShadowOut(to_clip(v.pos), v.local, v.size, v.params, v.color);
}

// vello_shaders 0.9.0 fine.wgsl (MIT OR Apache-2.0).
fn erf7(x: f32) -> f32 {
    let y = clamp(x * 1.1283791671, -100.0, 100.0);
    let yy = y * y;
    let z = y + (0.24295 + (0.03395 + 0.0104 * yy) * yy) * (y * yy);
    return z / sqrt(1.0 + z * z);
}

fn hypot(a: f32, b: f32) -> f32 {
    return sqrt(a * a + b * b);
}

// vello's CMD_BLUR_RECT: Raph Levien's closed-form approximation of a
// gaussian-blurred rounded rect, evaluated in the rect's centered space.
@fragment fn fs_shadow(i: ShadowOut) -> @location(0) vec4f {
    let std_dev = max(i.params.y, 1e-5);
    let inv_std_dev = 1.0 / std_dev;
    let radius = i.params.x;
    let min_edge = min(i.size.x, i.size.y);
    let radius_max = 0.5 * min_edge;
    let r0 = min(hypot(radius, std_dev * 1.15), radius_max);
    let r1 = min(hypot(radius, std_dev * 2.0), radius_max);
    let exponent = 2.0 * r1 / r0;
    let inv_exponent = 1.0 / exponent;
    let delta = 1.25 * std_dev * (exp(-pow(0.5 * inv_std_dev * i.size.x, 2.0)) - exp(-pow(0.5 * inv_std_dev * i.size.y, 2.0)));
    let width = i.size.x + min(delta, 0.0);
    let height = i.size.y - max(delta, 0.0);
    let scale = 0.5 * erf7(inv_std_dev * 0.5 * (max(width, height) - 0.5 * radius));
    let y0 = abs(i.local.y) - (height * 0.5 - r1);
    let y1 = max(y0, 0.0);
    let x0 = abs(i.local.x) - (width * 0.5 - r1);
    let x1 = max(x0, 0.0);
    let d_pos = pow(pow(x1, exponent) + pow(y1, exponent), inv_exponent);
    let d_neg = min(max(x0, y0), 0.0);
    let d = d_pos + d_neg - r1;
    let alpha = scale * (erf7(inv_std_dev * (min_edge + d)) - erf7(inv_std_dev * d));
    return i.color * alpha;
}
