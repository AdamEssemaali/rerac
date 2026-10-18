// rc-metal's 2D quads: Bevy's sprite (a textured, tinted rectangle in a 2D camera's world, one unit per pixel) and the
// UI node draws (images, flat colours), each a rectangle given in clip space by the renderer (crate::render::frame).

struct Quad {
    // Clip-space rectangle: (x0, y0, x1, y1) at the corners uv (0, 0) and (1, 1).
    rect: vec4<f32>,
    // Texture sub-rectangle (u0, v0, u1, v1).
    uv_rect: vec4<f32>,
    // Linear tint (multiplied with the texture; the UI's flat colours use a white texture).
    color: vec4<f32>,
}

@group(1) @binding(0) var<uniform> quad: Quad;
@group(2) @binding(0) var tex: texture_2d<f32>;
@group(2) @binding(1) var tex_sampler: sampler;

struct QuadOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vertex(@builtin(vertex_index) i: u32) -> QuadOut {
    let c = vec2<f32>(f32(i & 1u), f32((i >> 1u) & 1u));
    let p = mix(quad.rect.xy, quad.rect.zw, c);
    let uv = mix(quad.uv_rect.xy, quad.uv_rect.zw, c);
    return QuadOut(vec4<f32>(p, 0.0, 1.0), uv);
}

@fragment
fn fragment(in: QuadOut) -> @location(0) vec4<f32> {
    return textureSample(tex, tex_sampler, in.uv) * quad.color;
}
