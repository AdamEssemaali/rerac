// Bevy's StandardMaterial with `unlit: true`, as rc-engine uses it (the Swingshot rope): base colour × base-colour
// texture × vertex colour, no lighting, no fog; the alpha mode's blend is the pipeline's.
#import bevy_pbr::{forward_io::Vertex, mesh_functions, view_transformations::position_world_to_clip}

struct StandardUnlit {
    base_color: vec4<f32>,
    // x = 1 when there is a base-colour texture.
    flags: vec4<f32>,
}

@group(2) @binding(0) var<uniform> material: StandardUnlit;
@group(2) @binding(1) var base_color_texture: texture_2d<f32>;
@group(2) @binding(2) var base_color_sampler: sampler;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
}

@vertex
fn vertex(v: Vertex) -> Out {
    var o: Out;
    let world = mesh_functions::get_world_from_local(v.instance_index) * vec4<f32>(v.position, 1.0);
    o.position = position_world_to_clip(world.xyz);
#ifdef VERTEX_UVS_A
    o.uv = v.uv;
#else
    o.uv = vec2<f32>(0.0);
#endif
#ifdef VERTEX_COLORS
    o.color = v.color;
#else
    o.color = vec4<f32>(1.0);
#endif
    return o;
}

@fragment
fn fragment(in: Out) -> @location(0) vec4<f32> {
    var c = material.base_color * in.color;
    if (material.flags.x > 0.5) { c = c * textureSample(base_color_texture, base_color_sampler, in.uv); }
    return c;
}
