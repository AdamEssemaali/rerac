// Stand-in for `bevy_pbr::mesh_functions`: one record per drawn mesh instance (crate::frame::MeshInstance), indexed by
// the draw's instance index (base instance = the record), as Bevy's mesh uniform array. Group 1 is the mesh.
#define_import_path bevy_pbr::mesh_functions

struct MeshInstance {
    world_from_local: mat4x4<f32>,
    // Bevy's `MeshTag`.
    tag: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

@group(1) @binding(0) var<storage, read> mesh_instances: array<MeshInstance>;

fn get_world_from_local(instance_index: u32) -> mat4x4<f32> { return mesh_instances[instance_index].world_from_local; }

fn get_tag(instance_index: u32) -> u32 { return mesh_instances[instance_index].tag; }
