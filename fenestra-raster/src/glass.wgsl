// Frosted glass on the GPU: the same steps, in the same order and with the
// same arithmetic, as the shell's CPU path (fenestra-shell blur.rs and
// fenestra-core glass.rs). Every pass is a full-target triangle whose
// fragment reads its source with `textureLoad` (exact texels, edge-clamped).

struct Params {
    a: vec4<i32>,
    b: vec4<i32>,
    f: vec4<f32>,
}

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var src: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

fn linear_to_srgb(v: f32) -> f32 {
    let c = clamp(v, 0.0, 1.0);
    if c <= 0.0031308 {
        return c * 12.92;
    }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

// Vibrancy in linear light about Rec. 709 luminance (blur.rs `vibrance`).
fn vibrance(c: vec3<f32>, s: f32) -> vec3<f32> {
    if abs(s - 1.0) <= 1.1920929e-7 {
        return c;
    }
    let l = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    return max(vec3<f32>(l) + s * (c - vec3<f32>(l)), vec3<f32>(0.0));
}

// glass.rs `hash` / `dither_noise`: a triangle on (-1, 1), in levels.
fn hash(v0: u32) -> u32 {
    var v = v0;
    v = v ^ (v >> 16u);
    v = v * 0x7FEB352Du;
    v = v ^ (v >> 15u);
    v = v * 0x846CA68Bu;
    return v ^ (v >> 16u);
}

fn unit(v: u32) -> f32 {
    return f32(v >> 8u) / 16777216.0;
}

fn dither_noise(x: u32, y: u32) -> f32 {
    let a = hash((x * 0x9E3779B9u) ^ (y * 0x85EBCA6Bu));
    let b = hash(a ^ 0xC2B2AE35u);
    return unit(a) + unit(b) - 1.0;
}

fn dither_to_unit(v: f32, n: f32) -> f32 {
    return clamp(round(v * 255.0 + n), 0.0, 255.0) / 255.0;
}

// 1. Box-average each f×f block of the crop (partial at its right/bottom
//    edge) in linear light. a = crop (x0, y0, w, h); b.x = f.
@fragment
fn fs_linearize(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let f = p.b.x;
    let l = vec2<i32>(pos.xy);
    let x0 = p.a.x + l.x * f;
    let y0 = p.a.y + l.y * f;
    let x1 = min(x0 + f, p.a.x + p.a.z);
    let y1 = min(y0 + f, p.a.y + p.a.w);
    var sum = vec4<f32>(0.0);
    for (var y = y0; y < y1; y++) {
        for (var x = x0; x < x1; x++) {
            let c = textureLoad(src, vec2<i32>(x, y), 0);
            sum += vec4<f32>(srgb_to_linear(c.r), srgb_to_linear(c.g), srgb_to_linear(c.b), c.a);
        }
    }
    return sum / f32((x1 - x0) * (y1 - y0));
}

// 2. One box pass, edge-clamped. a = (radius, horizontal, width, height).
@fragment
fn fs_box(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let r = p.a.x;
    let at = vec2<i32>(pos.xy);
    let dir = select(vec2<i32>(0, 1), vec2<i32>(1, 0), p.a.y == 1);
    let hi = vec2<i32>(p.a.z - 1, p.a.w - 1);
    var sum = vec4<f32>(0.0);
    for (var k = -r; k <= r; k++) {
        sum += textureLoad(src, clamp(at + dir * k, vec2<i32>(0), hi), 0);
    }
    return sum / f32(2 * r + 1);
}

// A low-resolution texel back in sRGB, vibrancy applied (both run on the
// small field, as on the CPU).
fn low_srgb(at: vec2<i32>, s: f32) -> vec4<f32> {
    let c = textureLoad(src, at, 0);
    let v = vibrance(c.rgb, s);
    return vec4<f32>(linear_to_srgb(v.r), linear_to_srgb(v.g), linear_to_srgb(v.b), c.a);
}

// 3. The pane's window of the blurred field: bilinear from the low field,
//    pixel centres at (c + 0.5) / f − 0.5, clamped at its edges; sRGB, not
//    yet quantized. a = (pane origin in crop px, f, 0); b = (low w, low h);
//    f.x = saturation.
@fragment
fn fs_upsample(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let f = f32(p.a.z);
    let c = vec2<f32>(vec2<i32>(pos.xy) + p.a.xy);
    let last = vec2<f32>(f32(p.b.x - 1), f32(p.b.y - 1));
    let u = clamp((c + vec2<f32>(0.5)) / f - vec2<f32>(0.5), vec2<f32>(0.0), last);
    let i0 = vec2<i32>(floor(u));
    let i1 = min(i0 + vec2<i32>(1), vec2<i32>(p.b.x - 1, p.b.y - 1));
    let t = u - floor(u);
    let s = p.f.x;
    let top = mix(low_srgb(i0, s), low_srgb(vec2<i32>(i1.x, i0.y), s), t.x);
    let bot = mix(low_srgb(vec2<i32>(i0.x, i1.y), s), low_srgb(i1, s), t.x);
    return mix(top, bot, t.y);
}

// 4. Quantize once with dither, premultiplied. a.xy = the pane origin in crop
//    px (the dither is keyed by crop coordinates, as on the CPU).
@fragment
fn fs_quantize(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let at = vec2<i32>(pos.xy);
    let v = textureLoad(src, at, 0);
    let c = at + p.a.xy;
    let n = dither_noise(u32(c.x), u32(c.y));
    let a = dither_to_unit(v.a, 0.0);
    let rgb = vec3<f32>(dither_to_unit(v.r, n), dither_to_unit(v.g, n), dither_to_unit(v.b, n));
    return vec4<f32>(rgb * a, a);
}
