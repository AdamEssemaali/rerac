// Stand-in for `bevy_pbr::forward_io::Vertex` with Bevy's attribute locations (position 0, uv 2, colour 5).
#define_import_path bevy_pbr::forward_io

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
#ifdef VERTEX_UVS_A
    @location(2) uv: vec2<f32>,
#endif
#ifdef VERTEX_COLORS
    @location(5) color: vec4<f32>,
#endif
}
