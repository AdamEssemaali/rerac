// rc-metal's stand-in for Bevy's `bevy_pbr::mesh_view_bindings::view`: the fields rc-engine's shaders read, filled per
// view by the renderer (crate::frame::ViewUniform). Group 0 is the view.
#define_import_path bevy_pbr::mesh_view_bindings

struct View {
    clip_from_world: mat4x4<f32>,
    clip_from_view: mat4x4<f32>,
    view_from_world: mat4x4<f32>,
    world_from_view: mat4x4<f32>,
    world_position: vec3<f32>,
    // Physical viewport (x, y, width, height).
    viewport: vec4<f32>,
}

@group(0) @binding(0) var<uniform> view: View;
