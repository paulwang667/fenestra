// Frosted glass on the GPU: the same steps, in the same order and with the
// same arithmetic, as the shell's CPU path (fenestra-shell blur.rs and
// fenestra-core glass.rs). Every pass is a full-target triangle whose
// fragment reads its source with `textureLoad` (exact texels, edge-clamped).

struct Params {
    a: vec4<i32>,
    b: vec4<i32>,
    f: vec4<f32>,
    g: vec4<f32>,
}

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var src: texture_2d<f32>;
// The bake's second input: the 1×1 adapted tint.
@group(0) @binding(2) var tint_tex: texture_2d<f32>;

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

// Edge-clamped bilinear sample at fractional pixel-centre coordinates of a
// w×h field (blur.rs `bilinear`).
fn bilinear(s: vec2<f32>, size: vec2<i32>) -> vec4<f32> {
    let last = vec2<f32>(size - vec2<i32>(1));
    let fxy = clamp(s - vec2<f32>(0.5), vec2<f32>(0.0), last);
    let i0 = vec2<i32>(floor(fxy));
    let t = fxy - floor(fxy);
    let i1 = min(i0 + vec2<i32>(1), size - vec2<i32>(1));
    let top = mix(textureLoad(src, i0, 0), textureLoad(src, vec2<i32>(i1.x, i0.y), 0), t.x);
    let bot = mix(textureLoad(src, vec2<i32>(i0.x, i1.y), 0), textureLoad(src, i1, 0), t.x);
    return mix(top, bot, t.y);
}

fn sign_of(v: f32) -> f32 {
    return select(1.0, -1.0, v < 0.0);
}

// The outward unit normal of the rounded-box SDF (blur.rs `sdf_normal`).
fn sdf_normal(r: vec2<f32>, e: vec2<f32>) -> vec2<f32> {
    let s = vec2<f32>(sign_of(r.x), sign_of(r.y));
    let d = abs(r) - e;
    if d.x > 0.0 && d.y > 0.0 {
        let len = max(length(d), 1e-6);
        return s * d / len;
    }
    if d.x >= d.y {
        return vec2<f32>(s.x, 0.0);
    }
    return vec2<f32>(0.0, s.y);
}

// 5. Edge refraction (blur.rs `refract_edges`): within the bevel band along
//    the rounded-rect outline, resample from further inside along the inward
//    normal, the three channels at slightly different depths (dispersion).
//    a.xy = pane size; f = (corner radius, band) in px.
@fragment
fn fs_refract(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let size = p.a.xy;
    let at = vec2<i32>(pos.xy);
    let whf = vec2<f32>(size);
    let half = whf * 0.5;
    let r = clamp(p.f.x, 0.0, min(half.x, half.y));
    let band = min(max(p.f.y, 1.0), min(half.x, half.y));
    let max_disp = band * 0.55;
    let e = half - vec2<f32>(r);
    let px = vec2<f32>(at) + vec2<f32>(0.5);
    let rel = px - half;
    let q = abs(rel) - e;
    let outside = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
    let d = -outside;
    if !(d > 0.0 && d < band) {
        return textureLoad(src, at, 0);
    }
    let n = sdf_normal(rel, e);
    let t = d / band;
    let disp = max_disp * (1.0 - t) * (1.0 - t);
    let rp = bilinear(px - n * disp * 0.94, size);
    let gp = bilinear(px - n * disp, size);
    let bp = bilinear(px - n * disp * 1.06, size);
    return vec4<f32>(rp.r, gp.g, bp.b, gp.a);
}

// 6. Mean luma, first level: each texel of a ≤16×16 grid sums Rec. 709 luma
//    (on the sRGB values, as `adapt_fill` reads bytes) over every fourth
//    pixel of its block of the pane. a = (pane w, pane h, grid w, grid h).
//    Writes (sum, count).
@fragment
fn fs_luma(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let g = vec2<i32>(pos.xy);
    let size = p.a.xy;
    let grid = p.a.zw;
    let lo = g * size / grid;
    let hi = (g + vec2<i32>(1)) * size / grid;
    var sum = 0.0;
    var n = 0.0;
    for (var y = lo.y + ((4 - lo.y % 4) % 4); y < hi.y; y += 4) {
        for (var x = lo.x + ((4 - lo.x % 4) % 4); x < hi.x; x += 4) {
            let c = textureLoad(src, vec2<i32>(x, y), 0);
            sum += dot(c.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
            n += 1.0;
        }
    }
    return vec4<f32>(sum, n, 0.0, 0.0);
}

fn cbrt(x: f32) -> f32 {
    return sign(x) * pow(abs(x), 1.0 / 3.0);
}

// Linear sRGB ↔ OKLab (Björn Ottosson's matrices).
fn linear_to_oklab(c: vec3<f32>) -> vec3<f32> {
    let l = cbrt(0.4122214708 * c.r + 0.5363325363 * c.g + 0.0514459929 * c.b);
    let m = cbrt(0.2119034982 * c.r + 0.6806995451 * c.g + 0.1073969566 * c.b);
    let s = cbrt(0.0883024619 * c.r + 0.2817188376 * c.g + 0.6299787005 * c.b);
    return vec3<f32>(
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    );
}

fn oklab_to_linear(lab: vec3<f32>) -> vec3<f32> {
    let l0 = lab.x + 0.3963377774 * lab.y + 0.2158037573 * lab.z;
    let m0 = lab.x - 0.1055613458 * lab.y - 0.0638541728 * lab.z;
    let s0 = lab.x - 0.0894841775 * lab.y - 1.2914855480 * lab.z;
    let l = l0 * l0 * l0;
    let m = m0 * m0 * m0;
    let s = s0 * s0 * s0;
    return vec3<f32>(
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    );
}

// sRGB's transfer, extended past [0, 1] by symmetry (as `color` converts).
fn srgb_ext(x: f32) -> f32 {
    let a = abs(x);
    let v = select(1.055 * pow(a, 1.0 / 2.4) - 0.055, a * 12.92, a <= 0.0031308);
    return sign(x) * v;
}

fn oklch_to_srgb(l: f32, c: f32, h: f32) -> vec3<f32> {
    let lin = oklab_to_linear(vec3<f32>(l, c * cos(h), c * sin(h)));
    return vec3<f32>(srgb_ext(lin.r), srgb_ext(lin.g), srgb_ext(lin.b));
}

fn in_gamut(c: vec3<f32>) -> bool {
    return all(c >= vec3<f32>(-1e-4)) && all(c <= vec3<f32>(1.0 + 1e-4));
}

// 7. The tint, adapted (`adapt_fill` / `theme::oklch`): the mean luma moves
//    the tint's OKLCH lightness by gain·(pivot − luma); a result outside sRGB
//    bisects its chroma down to the gamut. a = (grid w, grid h, adaptive, 0);
//    f = the tint (sRGB, alpha); g = (pivot, gain).
@fragment
fn fs_tint(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let tint = p.f;
    if p.a.z == 0 {
        return tint;
    }
    var sum = 0.0;
    var n = 0.0;
    for (var y = 0; y < p.a.y; y++) {
        for (var x = 0; x < p.a.x; x++) {
            let v = textureLoad(src, vec2<i32>(x, y), 0);
            sum += v.x;
            n += v.y;
        }
    }
    let luma = sum / max(n, 1.0);
    let lab = linear_to_oklab(vec3<f32>(srgb_to_linear(tint.r), srgb_to_linear(tint.g), srgb_to_linear(tint.b)));
    let c = length(lab.yz);
    let h = select(atan2(lab.z, lab.y), 0.0, c < 1e-7);
    let l = clamp(lab.x + p.g.y * (p.g.x - luma), 0.0, 1.0);
    var rgb = oklch_to_srgb(l, c, h);
    if !in_gamut(rgb) {
        var lo = 0.0;
        var hi = c;
        for (var i = 0; i < 24; i++) {
            let mid = 0.5 * (lo + hi);
            if in_gamut(oklch_to_srgb(l, mid, h)) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        rgb = oklch_to_srgb(l, lo, h);
    }
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), tint.a);
}

// 8. The body (`bake_body`): over the frosted sRGB field, the tint
//    source-over, then the sheen's white and black washes; quantized once
//    with dither (keyed by pane pixel), premultiplied. a = (has tint,
//    has sheen); f = (sheen origin, step x, step y, top); g.x = bottom.
@fragment
fn fs_bake(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let at = vec2<i32>(pos.xy);
    let v = textureLoad(src, at, 0);
    var rgb = v.rgb;
    if p.a.x == 1 {
        let t = textureLoad(tint_tex, vec2<i32>(0, 0), 0);
        let ta = clamp(t.a, 0.0, 1.0);
        rgb = rgb * (1.0 - ta) + t.rgb * ta;
    }
    if p.a.y == 1 {
        let t = clamp(p.f.x + f32(at.x) * p.f.y + f32(at.y) * p.f.z, 0.0, 1.0);
        var white = 0.0;
        var black = 0.0;
        if t < 0.55 {
            white = p.f.w * (1.0 - t / 0.55);
        } else {
            black = p.g.x * (t - 0.55) / 0.45;
        }
        rgb = rgb * (1.0 - white - black) + vec3<f32>(white);
    }
    let n = dither_noise(u32(at.x), u32(at.y));
    let a = dither_to_unit(v.a, 0.0);
    let q = vec3<f32>(dither_to_unit(rgb.r, n), dither_to_unit(rgb.g, n), dither_to_unit(rgb.b, n));
    return vec4<f32>(q * a, a);
}
