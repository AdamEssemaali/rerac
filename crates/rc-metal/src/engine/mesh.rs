//! Meshes: the part of Bevy's `Mesh` API rc-engine uses, with Bevy's GPU layout (every attribute interleaved in one
//! vertex buffer, ordered by attribute id), so a material's attribute → shader-location list maps the same way.

use super::assets::AssetId;

/// Kept for source compatibility with rc-engine's constructors (every rc-engine asset is render-world only).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderAssetUsages;

impl RenderAssetUsages {
    pub const RENDER_WORLD: RenderAssetUsages = RenderAssetUsages;
    pub const MAIN_WORLD: RenderAssetUsages = RenderAssetUsages;
    pub fn all() -> Self { RenderAssetUsages }
}

impl std::ops::BitOr for RenderAssetUsages {
    type Output = Self;
    fn bitor(self, _: Self) -> Self { self }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PrimitiveTopology {
    PointList,
    LineList,
    LineStrip,
    TriangleList,
    TriangleStrip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum VertexFormat {
    Float32,
    Float32x2,
    Float32x3,
    Float32x4,
    Uint32,
    Uint32x2,
    Uint32x3,
    Uint32x4,
    Sint32,
    Unorm8x4,
}

impl VertexFormat {
    pub fn size(self) -> u64 {
        match self {
            VertexFormat::Float32 | VertexFormat::Uint32 | VertexFormat::Sint32 | VertexFormat::Unorm8x4 => 4,
            VertexFormat::Float32x2 | VertexFormat::Uint32x2 => 8,
            VertexFormat::Float32x3 | VertexFormat::Uint32x3 => 12,
            VertexFormat::Float32x4 | VertexFormat::Uint32x4 => 16,
        }
    }
}

/// A vertex attribute: Bevy's name, id (orders the interleaved layout) and format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MeshVertexAttribute {
    pub name: &'static str,
    pub id: u64,
    pub format: VertexFormat,
}

impl MeshVertexAttribute {
    pub const fn new(name: &'static str, id: u64, format: VertexFormat) -> Self { MeshVertexAttribute { name, id, format } }

    pub const fn at_shader_location(self, location: u32) -> VertexAttributeDescriptor { VertexAttributeDescriptor { attribute: self, location } }
}

/// An attribute bound to a shader location (Bevy's `VertexAttributeDescriptor`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VertexAttributeDescriptor {
    pub attribute: MeshVertexAttribute,
    pub location: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum VertexAttributeValues {
    Float32(Vec<f32>),
    Float32x2(Vec<[f32; 2]>),
    Float32x3(Vec<[f32; 3]>),
    Float32x4(Vec<[f32; 4]>),
    Uint32(Vec<u32>),
    Uint32x2(Vec<[u32; 2]>),
    Uint32x3(Vec<[u32; 3]>),
    Uint32x4(Vec<[u32; 4]>),
    Sint32(Vec<i32>),
    Unorm8x4(Vec<[u8; 4]>),
}

impl VertexAttributeValues {
    pub fn len(&self) -> usize {
        match self {
            Self::Float32(v) => v.len(),
            Self::Float32x2(v) => v.len(),
            Self::Float32x3(v) => v.len(),
            Self::Float32x4(v) => v.len(),
            Self::Uint32(v) => v.len(),
            Self::Uint32x2(v) => v.len(),
            Self::Uint32x3(v) => v.len(),
            Self::Uint32x4(v) => v.len(),
            Self::Sint32(v) => v.len(),
            Self::Unorm8x4(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool { self.len() == 0 }

    pub fn format(&self) -> VertexFormat {
        match self {
            Self::Float32(_) => VertexFormat::Float32,
            Self::Float32x2(_) => VertexFormat::Float32x2,
            Self::Float32x3(_) => VertexFormat::Float32x3,
            Self::Float32x4(_) => VertexFormat::Float32x4,
            Self::Uint32(_) => VertexFormat::Uint32,
            Self::Uint32x2(_) => VertexFormat::Uint32x2,
            Self::Uint32x3(_) => VertexFormat::Uint32x3,
            Self::Uint32x4(_) => VertexFormat::Uint32x4,
            Self::Sint32(_) => VertexFormat::Sint32,
            Self::Unorm8x4(_) => VertexFormat::Unorm8x4,
        }
    }

    /// Vertex `i`'s bytes (little-endian, as the GPU reads them).
    fn write(&self, i: usize, out: &mut Vec<u8>) {
        fn f(out: &mut Vec<u8>, v: &[f32]) { for x in v { out.extend_from_slice(&x.to_le_bytes()); } }
        fn u(out: &mut Vec<u8>, v: &[u32]) { for x in v { out.extend_from_slice(&x.to_le_bytes()); } }
        match self {
            Self::Float32(v) => f(out, &[v[i]]),
            Self::Float32x2(v) => f(out, &v[i]),
            Self::Float32x3(v) => f(out, &v[i]),
            Self::Float32x4(v) => f(out, &v[i]),
            Self::Uint32(v) => u(out, &[v[i]]),
            Self::Uint32x2(v) => u(out, &v[i]),
            Self::Uint32x3(v) => u(out, &v[i]),
            Self::Uint32x4(v) => u(out, &v[i]),
            Self::Sint32(v) => out.extend_from_slice(&v[i].to_le_bytes()),
            Self::Unorm8x4(v) => out.extend_from_slice(&v[i]),
        }
    }
}

macro_rules! from_values {
    ($($t:ty => $v:ident),* $(,)?) => {
        $(impl From<Vec<$t>> for VertexAttributeValues {
            fn from(v: Vec<$t>) -> Self { VertexAttributeValues::$v(v) }
        })*
    };
}
from_values!(f32 => Float32, [f32; 2] => Float32x2, [f32; 3] => Float32x3, [f32; 4] => Float32x4, u32 => Uint32,
    [u32; 2] => Uint32x2, [u32; 3] => Uint32x3, [u32; 4] => Uint32x4, i32 => Sint32);

impl From<Vec<glam::Vec3>> for VertexAttributeValues {
    fn from(v: Vec<glam::Vec3>) -> Self { VertexAttributeValues::Float32x3(v.into_iter().map(|p| p.to_array()).collect()) }
}
impl From<Vec<glam::Vec2>> for VertexAttributeValues {
    fn from(v: Vec<glam::Vec2>) -> Self { VertexAttributeValues::Float32x2(v.into_iter().map(|p| p.to_array()).collect()) }
}
impl From<Vec<glam::Vec4>> for VertexAttributeValues {
    fn from(v: Vec<glam::Vec4>) -> Self { VertexAttributeValues::Float32x4(v.into_iter().map(|p| p.to_array()).collect()) }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Indices {
    U16(Vec<u16>),
    U32(Vec<u32>),
}

impl Indices {
    pub fn len(&self) -> usize { match self { Indices::U16(v) => v.len(), Indices::U32(v) => v.len() } }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
}

#[derive(Clone, Debug)]
pub struct Mesh {
    pub topology: PrimitiveTopology,
    /// By attribute id (Bevy's layout order).
    attributes: std::collections::BTreeMap<u64, (MeshVertexAttribute, VertexAttributeValues)>,
    pub indices: Option<Indices>,
}

impl Mesh {
    pub const ATTRIBUTE_POSITION: MeshVertexAttribute = MeshVertexAttribute::new("Vertex_Position", 0, VertexFormat::Float32x3);
    pub const ATTRIBUTE_NORMAL: MeshVertexAttribute = MeshVertexAttribute::new("Vertex_Normal", 1, VertexFormat::Float32x3);
    pub const ATTRIBUTE_UV_0: MeshVertexAttribute = MeshVertexAttribute::new("Vertex_Uv", 2, VertexFormat::Float32x2);
    pub const ATTRIBUTE_UV_1: MeshVertexAttribute = MeshVertexAttribute::new("Vertex_Uv_1", 3, VertexFormat::Float32x2);
    pub const ATTRIBUTE_TANGENT: MeshVertexAttribute = MeshVertexAttribute::new("Vertex_Tangent", 4, VertexFormat::Float32x4);
    pub const ATTRIBUTE_COLOR: MeshVertexAttribute = MeshVertexAttribute::new("Vertex_Color", 5, VertexFormat::Float32x4);

    pub fn new(topology: PrimitiveTopology, _usage: RenderAssetUsages) -> Self { Mesh { topology, attributes: Default::default(), indices: None } }

    pub fn insert_attribute(&mut self, attribute: MeshVertexAttribute, values: impl Into<VertexAttributeValues>) {
        let values = values.into();
        assert_eq!(values.format(), attribute.format, "attribute {} has format {:?}, values are {:?}", attribute.name, attribute.format, values.format());
        self.attributes.insert(attribute.id, (attribute, values));
    }

    pub fn with_inserted_attribute(mut self, attribute: MeshVertexAttribute, values: impl Into<VertexAttributeValues>) -> Self {
        self.insert_attribute(attribute, values);
        self
    }

    pub fn insert_indices(&mut self, indices: Indices) { self.indices = Some(indices); }

    pub fn with_inserted_indices(mut self, indices: Indices) -> Self {
        self.indices = Some(indices);
        self
    }

    pub fn attribute(&self, attribute: MeshVertexAttribute) -> Option<&VertexAttributeValues> { self.attributes.get(&attribute.id).map(|(_, v)| v) }

    pub fn attribute_mut(&mut self, attribute: MeshVertexAttribute) -> Option<&mut VertexAttributeValues> { self.attributes.get_mut(&attribute.id).map(|(_, v)| v) }

    pub fn contains_attribute(&self, attribute: MeshVertexAttribute) -> bool { self.attributes.contains_key(&attribute.id) }

    pub fn count_vertices(&self) -> usize { self.attributes.values().map(|(_, v)| v.len()).min().unwrap_or(0) }

    pub fn primitive_topology(&self) -> PrimitiveTopology { self.topology }

    /// The interleaved layout: (attribute, byte offset) in id order, and the stride.
    pub fn layout(&self) -> MeshLayout {
        let mut off = 0;
        let attributes = self
            .attributes
            .values()
            .map(|(a, _)| {
                let o = off;
                off += a.format.size();
                (*a, o)
            })
            .collect();
        MeshLayout { attributes, stride: off }
    }

    /// The interleaved vertex bytes (Bevy's `create_packed_vertex_buffer_data`).
    pub fn vertex_bytes(&self) -> Vec<u8> {
        let n = self.count_vertices();
        let stride = self.layout().stride as usize;
        let mut out = Vec::with_capacity(n * stride);
        for i in 0..n {
            for (_, v) in self.attributes.values() { v.write(i, &mut out); }
        }
        out
    }
}

/// A mesh's interleaved vertex layout.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MeshLayout {
    pub attributes: Vec<(MeshVertexAttribute, u64)>,
    pub stride: u64,
}

impl MeshLayout {
    /// The requested attributes at their shader locations: (location, format, offset). Missing ones are an error, as in
    /// Bevy's `get_layout`.
    pub fn get_layout(&self, wanted: &[VertexAttributeDescriptor]) -> Result<Vec<(u32, VertexFormat, u64)>, String> {
        wanted
            .iter()
            .map(|w| {
                self.attributes
                    .iter()
                    .find(|(a, _)| a.id == w.attribute.id)
                    .map(|(a, o)| (w.location, a.format, *o))
                    .ok_or_else(|| format!("mesh is missing vertex attribute {}", w.attribute.name))
            })
            .collect()
    }

    pub fn contains(&self, attribute: MeshVertexAttribute) -> bool { self.attributes.iter().any(|(a, _)| a.id == attribute.id) }
}

/// Bevy's `Mesh3d` / `Mesh2d` components: the mesh an entity draws.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Mesh3d(pub super::assets::Handle<Mesh>);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Mesh2d(pub super::assets::Handle<Mesh>);

impl Mesh3d {
    pub fn id(&self) -> AssetId { self.0.id() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleaves_by_attribute_id() {
        const REF: MeshVertexAttribute = MeshVertexAttribute::new("Ref", 0x5446_5246, VertexFormat::Uint32x2);
        let m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
            .with_inserted_attribute(REF, vec![[7u32, 8]])
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![[1.0f32, 2.0, 3.0]]);
        let l = m.layout();
        assert_eq!(l.stride, 20);
        assert_eq!(l.get_layout(&[REF.at_shader_location(1)]).unwrap(), vec![(1, VertexFormat::Uint32x2, 12)]);
        assert_eq!(m.vertex_bytes().len(), 20);
        assert_eq!(&m.vertex_bytes()[12..16], &7u32.to_le_bytes());
    }
}
