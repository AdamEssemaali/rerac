// Stand-in for `bevy_pbr::view_transformations` (the two functions rc-engine's shaders use).
#define_import_path bevy_pbr::view_transformations
#import bevy_pbr::mesh_view_bindings::view

fn position_world_to_clip(p: vec3<f32>) -> vec4<f32> { return view.clip_from_world * vec4<f32>(p, 1.0); }

fn position_world_to_view(p: vec3<f32>) -> vec3<f32> { return (view.view_from_world * vec4<f32>(p, 1.0)).xyz; }
