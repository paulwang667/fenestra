// The last pass of a frame that may be translucent: premultiplied color to
// straight, as vello's fine stage writes it (rgb / max(a, 1e-6)).

@group(0) @binding(0) var src: texture_2d<f32>;

@vertex fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    // One triangle covering the target.
    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4f(p * 2.0 - 1.0, 0.0, 1.0);
}

@fragment fn fs_unpremultiply(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let c = textureLoad(src, vec2i(pos.xy), 0);
    return vec4f(c.rgb / max(c.a, 1e-6), c.a);
}
