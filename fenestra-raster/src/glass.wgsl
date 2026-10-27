// Frosted glass on the GPU: the shell's CPU path (fenestra-shell blur.rs and
// fenestra-core glass.rs) in as few render passes as it will go — a mobile
// tiler pays a fixed cost per pass on top of the work. A frame takes three:
// blur across (reading the backdrop in linear light), blur down (back to
// sRGB, vibrancy applied), then upsample, refraction and the body at once.
// Downsampled blurs average their blocks in a pass of their own first;
// tinted panes add their 1×1 adapted tints before the last. Every pane of
// the frame shares these passes: each draws into its own region of the
// stage's atlas (the viewport), and `o` places it there.

struct Params {
    a: vec4<i32>,
    b: vec4<i32>,
    f: vec4<f32>,
    g: vec4<f32>,
    // Where this pane sits: its output region's origin (the pane's (0, 0)),
    // and its low field's origin in the low atlases.
    o: vec4<i32>,
    // The blur kernel's weights for offsets 0, 1, … (symmetric).
    w: array<vec4<f32>, 64>,
}

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var src: texture_2d<f32>;
// The last pass's second input: the 1×1 adapted tint.
@group(0) @binding(2) var tint_tex: texture_2d<f32>;
// Bilinear, for averaging the backdrop's sRGB view in linear light.
@group(0) @binding(3) var smp: sampler;

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

fn weight(t: i32) -> f32 {
    let i = abs(t);
    return p.w[i / 4][i % 4];
}

// The mean of the backdrop's f×f block (bx, by) of the crop, in linear light
// (`src` is the backdrop's sRGB view, so both its loads and its filtering
// decode first). A whole block of an even f is (f/2)² bilinear taps on the
// texel corners; a partial one at the crop's right or bottom edge averages
// only what it covers, as the CPU does.
fn block(bx: i32, by: i32) -> vec4<f32> {
    let f = p.b.x;
    let x0 = p.a.x + bx * f;
    let y0 = p.a.y + by * f;
    let x1 = min(x0 + f, p.a.x + p.a.z);
    let y1 = min(y0 + f, p.a.y + p.a.w);
    var sum = vec4<f32>(0.0);
    if f > 1 && x1 - x0 == f && y1 - y0 == f {
        let inv = vec2<f32>(1.0) / vec2<f32>(textureDimensions(src));
        for (var j = 1; j < f; j += 2) {
            for (var i = 1; i < f; i += 2) {
                sum += textureSampleLevel(src, smp, vec2<f32>(f32(x0 + i), f32(y0 + j)) * inv, 0.0);
            }
        }
        return sum / f32((f / 2) * (f / 2));
    }
    for (var y = y0; y < y1; y++) {
        for (var x = x0; x < x1; x++) {
            sum += textureLoad(src, vec2<i32>(x, y), 0);
        }
    }
    return sum / f32((x1 - x0) * (y1 - y0));
}

// 0. A downsampled blur first averages each block once (every tap of the
//    blur across would otherwise re-average its block). a = crop (x0, y0,
//    w, h); b.x = f.
@fragment
fn fs_downsample(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let l = vec2<i32>(pos.xy) - p.o.xy;
    return block(l.x, l.y);
}

// The kernel along one axis of a texture: `at` is the texel the kernel is
// centred on, `step` the axis, `lo..=hi` the texels it may read (it clamps
// there). Two neighbouring taps inside that range are one bilinear sample
// between them, weighted by their sum — half the reads; a tap past the
// range reads the edge texel on its own.
fn convolve(at: vec2<i32>, step: vec2<i32>, lo: i32, hi: i32, along: i32) -> vec4<f32> {
    let r = p.b.w;
    let inv = vec2<f32>(1.0) / vec2<f32>(textureDimensions(src));
    let fstep = vec2<f32>(step);
    var sum = vec4<f32>(0.0);
    var t = -r;
    loop {
        if t > r {
            break;
        }
        let i = along + t;
        if t < r && i >= lo && i + 1 <= hi {
            let w0 = weight(t);
            let w1 = weight(t + 1);
            let w = w0 + w1;
            let c = (vec2<f32>(at + step * t) + vec2<f32>(0.5) + fstep * (w1 / max(w, 1e-30))) * inv;
            sum += w * textureSampleLevel(src, smp, c, 0.0);
            t += 2;
        } else {
            let e = clamp(i, lo, hi) - along;
            sum += weight(t) * textureLoad(src, at + step * e, 0);
            t += 1;
        }
    }
    return sum;
}

// 1. Across: each low texel is the kernel (the three box passes, convolved)
//    over the row, edge-clamped — of the backdrop's pixels at f = 1, of the
//    block means `src` holds otherwise. a = crop (x0, y0, w, h);
//    b = (f, low w, low h, kernel radius).
@fragment
fn fs_blur_h(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let l = vec2<i32>(pos.xy) - p.o.xy;
    let base = select(p.o.zw, p.a.xy, p.b.x == 1);
    return convolve(base + l, vec2<i32>(1, 0), 0, p.b.y - 1, l.x);
}

// 2. Down, then back to sRGB with vibrancy (both on the small field, as on
//    the CPU). a.y = low h; b.w = kernel radius; f.x = saturation.
@fragment
fn fs_blur_v(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let l = vec2<i32>(pos.xy) - p.o.xy;
    let sum = convolve(p.o.zw + l, vec2<i32>(0, 1), 0, p.a.y - 1, l.y);
    let v = vibrance(sum.rgb, p.f.x);
    return vec4<f32>(linear_to_srgb(v.r), linear_to_srgb(v.g), linear_to_srgb(v.b), sum.a);
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


// 3. The tint, adapted (`adapt_fill` / `theme::oklch`): the mean Rec. 709
//    luma of the frosted field under the pane (on its sRGB values, as
//    `adapt_fill` reads bytes; sampled from the low field, every step-th
//    texel) moves the tint's OKLCH lightness by gain·(pivot − luma); a result
//    outside sRGB bisects its chroma down to the gamut. Each pane writes
//    its own texel of the row. a = the low texels under the pane (x0, y0,
//    x1, y1), exclusive; b = (adaptive, step);
//    f = the tint (sRGB, alpha); g = (pivot, gain).
@fragment
fn fs_tint(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let tint = p.f;
    if p.b.x == 0 {
        return tint;
    }
    var sum = 0.0;
    var n = 0.0;
    for (var y = p.a.y; y < p.a.w; y += p.b.y) {
        for (var x = p.a.x; x < p.a.z; x += p.b.y) {
            let c = textureLoad(src, p.o.zw + vec2<i32>(x, y), 0);
            sum += dot(c.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
            n += 1.0;
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

// The frosted field at fractional pane pixel q (pixel centres at integers,
// clamped to the pane): bilinear from the low field, whose texel centres sit
// at (c + 0.5) / f − 0.5 in crop px.
fn field(q: vec2<f32>) -> vec4<f32> {
    let c = clamp(q, vec2<f32>(0.0), vec2<f32>(p.a.xy - vec2<i32>(1))) + vec2<f32>(p.a.zw);
    let f = f32(p.b.x);
    let last = vec2<i32>(p.b.y - 1, p.b.z - 1);
    let u = clamp((c + vec2<f32>(0.5)) / f - vec2<f32>(0.5), vec2<f32>(0.0), vec2<f32>(last));
    // u stays within the pane's low texels, so the filter never reaches a
    // neighbour's.
    let inv = vec2<f32>(1.0) / vec2<f32>(textureDimensions(src));
    return textureSampleLevel(src, smp, (vec2<f32>(p.o.zw) + u + vec2<f32>(0.5)) * inv, 0.0);
}

// 4. The pane: the frosted field, its rim refracted (blur.rs
//    `refract_edges`: within the bevel band along the rounded-rect outline,
//    resample from further inside along the inward normal, the three
//    channels at slightly different depths), then the body (`bake_body`: the
//    tint source-over, the sheen's white and black washes); quantized once
//    with dither, premultiplied. The dither is keyed by pane pixel with a
//    body (as the final pass bakes it) and by crop pixel without (as the blur
//    quantizes). a = (pane w, pane h, origin in crop px); b = (f, low w,
//    low h, flags: 1 lensed, 2 body, 4 tint, 8 sheen); f = (corner radius,
//    band, sheen origin, sheen step x); g = (sheen step y, top, bottom, tint
//    texel). The region is the pane with a one-pixel border of its edge
//    pixels, so a filtered draw of it never reaches a neighbour.
@fragment
fn fs_compose(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let at = clamp(vec2<i32>(pos.xy) - p.o.xy, vec2<i32>(0), p.a.xy - vec2<i32>(1));
    let flags = p.b.w;
    var v = field(vec2<f32>(at));
    if (flags & 1) != 0 {
        let half = vec2<f32>(p.a.xy) * 0.5;
        let r = clamp(p.f.x, 0.0, min(half.x, half.y));
        let band = min(max(p.f.y, 1.0), min(half.x, half.y));
        let e = half - vec2<f32>(r);
        let rel = vec2<f32>(at) + vec2<f32>(0.5) - half;
        let q = abs(rel) - e;
        let d = -(length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r);
        if d > 0.0 && d < band {
            let n = sdf_normal(rel, e);
            let t = d / band;
            let disp = band * 0.55 * (1.0 - t) * (1.0 - t);
            let base = vec2<f32>(at);
            let rp = field(base - n * disp * 0.94);
            let gp = field(base - n * disp);
            let bp = field(base - n * disp * 1.06);
            v = vec4<f32>(rp.r, gp.g, bp.b, gp.a);
        }
    }
    var rgb = v.rgb;
    var key = at + p.a.zw;
    if (flags & 2) != 0 {
        key = at;
        if (flags & 4) != 0 {
            let tc = textureLoad(tint_tex, vec2<i32>(i32(p.g.w), 0), 0);
            let ta = clamp(tc.a, 0.0, 1.0);
            rgb = rgb * (1.0 - ta) + tc.rgb * ta;
        }
        if (flags & 8) != 0 {
            let t = clamp(p.f.z + f32(at.x) * p.f.w + f32(at.y) * p.g.x, 0.0, 1.0);
            var white = 0.0;
            var black = 0.0;
            if t < 0.55 {
                white = p.g.y * (1.0 - t / 0.55);
            } else {
                black = p.g.z * (t - 0.55) / 0.45;
            }
            rgb = rgb * (1.0 - white - black) + vec3<f32>(white);
        }
    }
    let n = dither_noise(u32(key.x), u32(key.y));
    let a = dither_to_unit(v.a, 0.0);
    let q = vec3<f32>(dither_to_unit(rgb.r, n), dither_to_unit(rgb.g, n), dither_to_unit(rgb.b, n));
    return vec4<f32>(q * a, a);
}
