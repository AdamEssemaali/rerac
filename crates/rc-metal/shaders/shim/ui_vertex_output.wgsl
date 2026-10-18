// Stand-in for `bevy_ui::ui_vertex_output::UiVertexOutput` (rc-engine's UI materials read `uv` only) and the quad
// vertex shader that draws a UI node: a rectangle in target pixels (crate::frame::UiQuad, group 3: Bevy UI materials use 1).
#define_import_path bevy_ui::ui_vertex_output

struct UiVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

struct UiQuad {
    // (x, y, width, height) in target pixels, and the target size.
    rect: vec4<f32>,
    target_size: vec4<f32>,
}

@group(3) @binding(0) var<uniform> ui_quad: UiQuad;

@vertex
fn ui_vertex(@builtin(vertex_index) i: u32) -> UiVertexOutput {
    let uv = vec2<f32>(f32(i & 1u), f32((i >> 1u) & 1u));
    let px = ui_quad.rect.xy + uv * ui_quad.rect.zw;
    let ndc = vec2<f32>(px.x / ui_quad.target_size.x * 2.0 - 1.0, 1.0 - px.y / ui_quad.target_size.y * 2.0);
    return UiVertexOutput(vec4<f32>(ndc, 0.0, 1.0), uv);
}
