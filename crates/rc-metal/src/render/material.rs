//! Materials: Bevy's `Material` / `Material2d` contract for rc-engine's material types, without Bevy.
//!
//! A material names its WGSL file (crate::shader), its phase ([`AlphaMode`]), its sort bias, and its bindings in the
//! material group (`@group(2)`, Bevy's `#{MATERIAL_BIND_GROUP}`), and adjusts the pipeline in [`Material::specialize`]
//! exactly where rc-engine's `specialize` adjusts Bevy's. [`PipelineDesc::mesh_defaults`] is Bevy 0.19's
//! `MeshPipeline::specialize` (vertex attributes and shader defs from the mesh, blend and depth write from the alpha
//! mode, back-face culling, GEQUAL), so a material that changes nothing gets Bevy's pipeline.

use super::gfx::{Binding, Blend, CompareFunction, Gfx, ShaderBuffer};
use crate::engine::assets::Handle;
use crate::engine::image::{Image, TextureFormat};
use crate::engine::mesh::{Mesh, MeshLayout, VertexAttributeDescriptor};
use crate::shader::{Def, Defs};
use objc2_metal::MTLColorWriteMask;
use std::rc::Rc;

/// Bevy's `AlphaMode`: which phase a draw goes to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AlphaMode {
    Opaque,
    Mask(f32),
    Blend,
    Premultiplied,
    Add,
    Multiply,
}

impl AlphaMode {
    pub fn phase(self) -> Phase {
        match self {
            AlphaMode::Opaque => Phase::Opaque,
            AlphaMode::Mask(_) => Phase::AlphaMask,
            _ => Phase::Transparent,
        }
    }
}

/// Bevy's 3D phases, in drawing order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Phase {
    Opaque,
    AlphaMask,
    Transparent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Face {
    Front,
    Back,
}

/// The part of a render pipeline descriptor rc-engine's `specialize` functions touch.
#[derive(Clone, Debug)]
pub struct PipelineDesc {
    pub vertex_entry: &'static str,
    pub fragment_entry: &'static str,
    pub defs: Defs,
    pub vertex_attributes: Vec<VertexAttributeDescriptor>,
    pub color_format: TextureFormat,
    pub blend: Option<Blend>,
    pub write_mask: MTLColorWriteMask,
    pub depth_write: bool,
    pub depth_compare: CompareFunction,
    pub cull: Option<Face>,
}

impl PipelineDesc {
    /// Bevy's mesh pipeline for `layout`, `alpha` and the view's target format.
    pub fn mesh_defaults(layout: &MeshLayout, alpha: AlphaMode, color_format: TextureFormat) -> Self {
        let mut defs = Defs::new();
        let mut attrs = Vec::new();
        defs.insert("MESH_PIPELINE".into(), Def::Flag);
        defs.insert("VERTEX_OUTPUT_INSTANCE_INDEX".into(), Def::Flag);
        let mut add = |a: crate::engine::mesh::MeshVertexAttribute, loc: u32, names: &[&str]| {
            if layout.contains(a) {
                for n in names { defs.insert((*n).into(), Def::Flag); }
                attrs.push(a.at_shader_location(loc));
            }
        };
        add(Mesh::ATTRIBUTE_POSITION, 0, &["VERTEX_POSITIONS"]);
        add(Mesh::ATTRIBUTE_NORMAL, 1, &["VERTEX_NORMALS"]);
        add(Mesh::ATTRIBUTE_UV_0, 2, &["VERTEX_UVS", "VERTEX_UVS_A"]);
        add(Mesh::ATTRIBUTE_UV_1, 3, &["VERTEX_UVS", "VERTEX_UVS_B"]);
        add(Mesh::ATTRIBUTE_TANGENT, 4, &["VERTEX_TANGENTS"]);
        add(Mesh::ATTRIBUTE_COLOR, 5, &["VERTEX_COLORS"]);
        let (blend, depth_write) = match alpha {
            AlphaMode::Opaque | AlphaMode::Mask(_) => (None, true),
            AlphaMode::Blend => (Some(Blend::ALPHA_BLENDING), false),
            AlphaMode::Premultiplied | AlphaMode::Add => (Some(Blend::PREMULTIPLIED_ALPHA_BLENDING), false),
            AlphaMode::Multiply => (
                Some(Blend {
                    color: super::gfx::BlendComponent {
                        src_factor: super::gfx::BlendFactor::Dst,
                        dst_factor: super::gfx::BlendFactor::OneMinusSrcAlpha,
                        operation: super::gfx::BlendOperation::Add,
                    },
                    alpha: super::gfx::BlendComponent::OVER,
                }),
                false,
            ),
        };
        PipelineDesc {
            vertex_entry: "vertex",
            fragment_entry: "fragment",
            defs,
            vertex_attributes: attrs,
            color_format,
            blend,
            write_mask: MTLColorWriteMask::All,
            depth_write,
            depth_compare: CompareFunction::GreaterEqual,
            cull: Some(Face::Back),
        }
    }

    pub fn def(&mut self, name: &str) { self.defs.insert(name.into(), Def::Flag); }
    pub fn def_u(&mut self, name: &str, v: u32) { self.defs.insert(name.into(), Def::UInt(v)); }
    pub fn def_i(&mut self, name: &str, v: i32) { self.defs.insert(name.into(), Def::Int(v)); }
}

/// The material group's bindings, filled by [`Material::bindings`] (Bevy's `AsBindGroup` attributes). A missing GPU
/// asset (not uploaded yet, or freed) makes the draw skip this frame, as Bevy skips a material whose bind group cannot
/// be prepared.
pub struct BindGroup<'a> {
    pub(crate) gfx: &'a Gfx,
    pub(crate) out: Vec<(u32, Binding)>,
    pub(crate) missing: bool,
}

impl<'a> BindGroup<'a> {
    pub fn new(gfx: &'a Gfx) -> Self { BindGroup { gfx, out: Vec::new(), missing: false } }

    /// `#[texture(n)]`.
    pub fn texture(&mut self, n: u32, h: &Handle<Image>) {
        match self.gfx.images.get(&h.id()) {
            Some(i) => self.out.push((n, Binding::Texture(i.texture.clone()))),
            None => self.missing = true,
        }
    }

    /// `#[sampler(n)]`: the image's own sampler.
    pub fn sampler(&mut self, n: u32, h: &Handle<Image>) {
        match self.gfx.images.get(&h.id()) {
            Some(i) => self.out.push((n, Binding::Sampler(i.sampler.clone()))),
            None => self.missing = true,
        }
    }

    /// `#[texture(n)] #[sampler(n + 1)]`, the common pair.
    pub fn texture_sampler(&mut self, n: u32, h: &Handle<Image>) {
        self.texture(n, h);
        self.sampler(n + 1, h);
    }

    /// `#[storage(n)]`.
    pub fn storage(&mut self, n: u32, h: &Handle<ShaderBuffer>) {
        match self.gfx.buffers.get(&h.id()) {
            Some(b) => self.out.push((n, Binding::Buffer(b.clone()))),
            None => self.missing = true,
        }
    }

    /// `#[uniform(n)]`: the value's bytes in WGSL uniform layout (crate::render::material::Std140 helps).
    pub fn uniform(&mut self, n: u32, bytes: Vec<u8>) { self.out.push((n, Binding::Bytes(Rc::new(bytes)))); }

    pub fn into_bindings(self) -> Option<Vec<(u32, Binding)>> { (!self.missing).then_some(self.out) }
}

/// Bevy's `Material` for rc-engine's material types.
pub trait Material: 'static {
    /// The WGSL file stem in crate::shader.
    fn shader() -> &'static str;

    fn alpha_mode(&self) -> AlphaMode { AlphaMode::Opaque }

    /// Added to the view depth of a transparent draw (Bevy's `depth_bias`).
    fn depth_bias(&self) -> f32 { 0.0 }

    /// Bevy's `specialize`, applied to [`PipelineDesc::mesh_defaults`].
    fn specialize(&self, _desc: &mut PipelineDesc, _layout: &MeshLayout) -> Result<(), String> { Ok(()) }

    fn bindings(&self, b: &mut BindGroup);

    /// A key of everything [`Material::specialize`] depends on besides the mesh layout (Bevy's `bind_group_data`); the
    /// pipeline is cached per (key, layout, target).
    fn pipeline_key(&self) -> u64 { 0 }
}

/// Bevy's `MeshMaterial3d<M>`.
pub struct MeshMaterial3d<M>(pub Handle<M>);

impl<M> Clone for MeshMaterial3d<M> {
    fn clone(&self) -> Self { MeshMaterial3d(self.0.clone()) }
}

impl<M> PartialEq for MeshMaterial3d<M> {
    fn eq(&self, o: &Self) -> bool { self.0 == o.0 }
}

/// Bevy's `MeshMaterial2d<M>`.
pub struct MeshMaterial2d<M>(pub Handle<M>);

impl<M> Clone for MeshMaterial2d<M> {
    fn clone(&self) -> Self { MeshMaterial2d(self.0.clone()) }
}

/// Bevy's `MeshTag`: per-entity data the shader reads through `mesh_functions::get_tag`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct MeshTag(pub u32);

/// Writes values in WGSL's uniform (std140-like) layout: scalars and vectors aligned to their size (vec3 to 16),
/// structs padded to 16.
#[derive(Default, Clone, Debug)]
pub struct Std140(pub Vec<u8>);

impl Std140 {
    fn align(&mut self, a: usize) { while self.0.len() % a != 0 { self.0.push(0); } }
    pub fn f32(&mut self, v: f32) -> &mut Self { self.align(4); self.0.extend_from_slice(&v.to_le_bytes()); self }
    pub fn u32(&mut self, v: u32) -> &mut Self { self.align(4); self.0.extend_from_slice(&v.to_le_bytes()); self }
    pub fn i32(&mut self, v: i32) -> &mut Self { self.align(4); self.0.extend_from_slice(&v.to_le_bytes()); self }
    pub fn vec2(&mut self, v: glam::Vec2) -> &mut Self { self.align(8); for x in v.to_array() { self.0.extend_from_slice(&x.to_le_bytes()); } self }
    pub fn vec3(&mut self, v: glam::Vec3) -> &mut Self { self.align(16); for x in v.to_array() { self.0.extend_from_slice(&x.to_le_bytes()); } self }
    pub fn vec4(&mut self, v: glam::Vec4) -> &mut Self { self.align(16); for x in v.to_array() { self.0.extend_from_slice(&x.to_le_bytes()); } self }
    pub fn uvec4(&mut self, v: glam::UVec4) -> &mut Self { self.align(16); for x in v.to_array() { self.0.extend_from_slice(&x.to_le_bytes()); } self }
    pub fn mat4(&mut self, m: glam::Mat4) -> &mut Self { self.align(16); for x in m.to_cols_array() { self.0.extend_from_slice(&x.to_le_bytes()); } self }
    /// Ends a struct (size rounded up to 16).
    pub fn finish(mut self) -> Vec<u8> {
        self.align(16);
        if self.0.is_empty() { self.0.resize(16, 0); }
        self.0
    }
}

/// A material type's pipeline cache entry and the GPU state it implies.
pub(crate) struct CachedPipeline {
    pub entry: Rc<super::gfx::PipelineEntry>,
    pub depth: objc2::rc::Retained<super::gfx::DepthState>,
    pub cull: Option<Face>,
    pub format: TextureFormat,
    /// Creation order (Bevy's `CachedRenderPipelineId`, which orders the binned phases).
    pub serial: u64,
}
