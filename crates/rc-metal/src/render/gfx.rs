//! The Metal layer: device and queue, GPU mirrors of the asset stores (meshes, images, shader buffers), samplers, and
//! render pipelines built from rc-engine's WGSL (crate::shader) and cached by their full state.
//!
//! Up to [`FRAMES_IN_FLIGHT`] frames are in flight. The CPU never writes a buffer the GPU may still read: a changed
//! asset gets a fresh GPU object (Metal keeps the old one alive until the frames using it complete), and bind groups name
//! assets by id, resolved when they are bound, so they stay valid across those swaps.

use crate::engine::assets::{AssetId, Assets};
use crate::engine::image::{Image, ImageAddressMode, ImageFilterMode, ImageSamplerDescriptor, TextureDimension, TextureFormat};
use crate::engine::mesh::{Indices, Mesh, PrimitiveTopology, VertexFormat};
use crate::shader::{self, Compiled, Defs, ResourceKind};
use anyhow::{anyhow, Context, Result};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::*;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;

pub type Device = ProtocolObject<dyn MTLDevice>;
pub type Texture = ProtocolObject<dyn MTLTexture>;
pub type Buffer = ProtocolObject<dyn MTLBuffer>;
pub type Sampler = ProtocolObject<dyn MTLSamplerState>;
pub type Pipeline = ProtocolObject<dyn MTLRenderPipelineState>;
pub type DepthState = ProtocolObject<dyn MTLDepthStencilState>;
pub type Encoder = ProtocolObject<dyn MTLRenderCommandEncoder>;
pub type CommandBuffer = ProtocolObject<dyn MTLCommandBuffer>;

pub const DEPTH_FORMAT: MTLPixelFormat = MTLPixelFormat::Depth32Float;

/// Frames the CPU may encode ahead of the GPU.
pub const FRAMES_IN_FLIGHT: usize = 2;

pub fn ns(s: &str) -> Retained<NSString> { NSString::from_str(s) }

pub fn pixel_format(f: TextureFormat) -> MTLPixelFormat {
    match f {
        TextureFormat::Rgba8Unorm => MTLPixelFormat::RGBA8Unorm,
        TextureFormat::Rgba8UnormSrgb => MTLPixelFormat::RGBA8Unorm_sRGB,
        TextureFormat::Bgra8Unorm => MTLPixelFormat::BGRA8Unorm,
        TextureFormat::Bgra8UnormSrgb => MTLPixelFormat::BGRA8Unorm_sRGB,
        TextureFormat::Rgba16Float => MTLPixelFormat::RGBA16Float,
        TextureFormat::R16Float => MTLPixelFormat::R16Float,
        TextureFormat::R8Unorm => MTLPixelFormat::R8Unorm,
        TextureFormat::Depth32Float => MTLPixelFormat::Depth32Float,
    }
}

fn vertex_format(f: VertexFormat) -> MTLVertexFormat {
    match f {
        VertexFormat::Float32 => MTLVertexFormat::Float,
        VertexFormat::Float32x2 => MTLVertexFormat::Float2,
        VertexFormat::Float32x3 => MTLVertexFormat::Float3,
        VertexFormat::Float32x4 => MTLVertexFormat::Float4,
        VertexFormat::Uint32 => MTLVertexFormat::UInt,
        VertexFormat::Uint32x2 => MTLVertexFormat::UInt2,
        VertexFormat::Uint32x3 => MTLVertexFormat::UInt3,
        VertexFormat::Uint32x4 => MTLVertexFormat::UInt4,
        VertexFormat::Sint32 => MTLVertexFormat::Int,
        VertexFormat::Unorm8x4 => MTLVertexFormat::UChar4Normalized,
    }
}

pub fn primitive(t: PrimitiveTopology) -> MTLPrimitiveType {
    match t {
        PrimitiveTopology::PointList => MTLPrimitiveType::Point,
        PrimitiveTopology::LineList => MTLPrimitiveType::Line,
        PrimitiveTopology::LineStrip => MTLPrimitiveType::LineStrip,
        PrimitiveTopology::TriangleList => MTLPrimitiveType::Triangle,
        PrimitiveTopology::TriangleStrip => MTLPrimitiveType::TriangleStrip,
    }
}

/// A buffer with `bytes` (at least 16 bytes: Metal rejects empty buffers and naga's sizes math wants a real length).
pub fn new_buffer(device: &Device, bytes: &[u8], label: &str) -> Result<Retained<Buffer>> {
    let len = bytes.len().max(16);
    let buf = device.newBufferWithLength_options(len, MTLResourceOptions::StorageModeShared).ok_or_else(|| anyhow!("Metal could not allocate {label} ({len} bytes)"))?;
    // SAFETY: a fresh shared buffer of `len` ≥ bytes.len() bytes.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf.contents().as_ptr() as *mut u8, bytes.len()) };
    buf.setLabel(Some(&ns(label)));
    Ok(buf)
}

/// Rewrites a shared buffer in place when it is big enough (the previous frame has completed, module doc), else
/// returns a new one.
pub fn write_buffer(device: &Device, buf: &mut Retained<Buffer>, bytes: &[u8], label: &str) -> Result<()> {
    if buf.length() >= bytes.len().max(16) {
        // SAFETY: shared storage, length checked, no frame in flight.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf.contents().as_ptr() as *mut u8, bytes.len()) };
        Ok(())
    } else {
        *buf = new_buffer(device, bytes, label)?;
        Ok(())
    }
}

/// A mesh on the GPU.
pub struct GpuMesh {
    pub vertices: Retained<Buffer>,
    pub layout: crate::engine::mesh::MeshLayout,
    /// A hash of the layout and topology (a pipeline cache key).
    pub layout_hash: u64,
    pub indices: Option<(Retained<Buffer>, MTLIndexType, usize)>,
    pub vertex_count: usize,
    pub topology: PrimitiveTopology,
}

/// An image on the GPU (its sampler follows the image's descriptor).
pub struct GpuImage {
    pub texture: Retained<Texture>,
    pub sampler: Rc<Retained<Sampler>>,
    pub format: TextureFormat,
    pub size: glam::UVec2,
}

/// Bevy's `ShaderBuffer` (a storage buffer asset).
#[derive(Clone, Debug, Default)]
pub struct ShaderBuffer {
    pub data: Option<Vec<u8>>,
}

impl ShaderBuffer {
    pub fn new(data: &[u8], _usage: crate::engine::mesh::RenderAssetUsages) -> Self { ShaderBuffer { data: Some(data.to_vec()) } }
}

/// Everything a pipeline is built from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PipelineKey {
    pub shader: &'static str,
    pub vertex_entry: &'static str,
    /// Another shader module for the vertex stage (e.g. the fullscreen triangle); `None` = `shader`.
    pub vertex_shader: Option<&'static str>,
    pub fragment_entry: Option<&'static str>,
    pub defs: Defs,
    /// (location, format, offset) and the stride of the one vertex buffer; empty = no vertex buffer.
    pub vertex_attributes: Vec<(u32, VertexFormat, u64)>,
    pub vertex_stride: u64,
    pub color_formats: Vec<TextureFormat>,
    pub blend: Option<Blend>,
    pub write_mask: MTLColorWriteMask,
    pub depth: bool,
    pub samples: u32,
    pub topology_class: TopologyClass,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TopologyClass {
    Point,
    Line,
    Triangle,
}

impl From<PrimitiveTopology> for TopologyClass {
    fn from(t: PrimitiveTopology) -> Self {
        match t {
            PrimitiveTopology::PointList => TopologyClass::Point,
            PrimitiveTopology::LineList | PrimitiveTopology::LineStrip => TopologyClass::Line,
            _ => TopologyClass::Triangle,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlendFactor {
    Zero,
    One,
    Src,
    OneMinusSrc,
    SrcAlpha,
    OneMinusSrcAlpha,
    Dst,
    OneMinusDst,
    DstAlpha,
    OneMinusDstAlpha,
    Constant,
    OneMinusConstant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlendOperation {
    Add,
    Subtract,
    ReverseSubtract,
    Min,
    Max,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlendComponent {
    pub src_factor: BlendFactor,
    pub dst_factor: BlendFactor,
    pub operation: BlendOperation,
}

impl BlendComponent {
    pub const REPLACE: BlendComponent = BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::Zero, operation: BlendOperation::Add };
    pub const OVER: BlendComponent = BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::OneMinusSrcAlpha, operation: BlendOperation::Add };
}

/// wgpu's `BlendState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Blend {
    pub color: BlendComponent,
    pub alpha: BlendComponent,
}

impl Blend {
    pub const ALPHA_BLENDING: Blend = Blend {
        color: BlendComponent { src_factor: BlendFactor::SrcAlpha, dst_factor: BlendFactor::OneMinusSrcAlpha, operation: BlendOperation::Add },
        alpha: BlendComponent::OVER,
    };
    pub const PREMULTIPLIED_ALPHA_BLENDING: Blend = Blend { color: BlendComponent::OVER, alpha: BlendComponent::OVER };
}

fn blend_factor(f: BlendFactor) -> MTLBlendFactor {
    match f {
        BlendFactor::Zero => MTLBlendFactor::Zero,
        BlendFactor::One => MTLBlendFactor::One,
        BlendFactor::Src => MTLBlendFactor::SourceColor,
        BlendFactor::OneMinusSrc => MTLBlendFactor::OneMinusSourceColor,
        BlendFactor::SrcAlpha => MTLBlendFactor::SourceAlpha,
        BlendFactor::OneMinusSrcAlpha => MTLBlendFactor::OneMinusSourceAlpha,
        BlendFactor::Dst => MTLBlendFactor::DestinationColor,
        BlendFactor::OneMinusDst => MTLBlendFactor::OneMinusDestinationColor,
        BlendFactor::DstAlpha => MTLBlendFactor::DestinationAlpha,
        BlendFactor::OneMinusDstAlpha => MTLBlendFactor::OneMinusDestinationAlpha,
        BlendFactor::Constant => MTLBlendFactor::BlendColor,
        BlendFactor::OneMinusConstant => MTLBlendFactor::OneMinusBlendColor,
    }
}

fn blend_op(o: BlendOperation) -> MTLBlendOperation {
    match o {
        BlendOperation::Add => MTLBlendOperation::Add,
        BlendOperation::Subtract => MTLBlendOperation::Subtract,
        BlendOperation::ReverseSubtract => MTLBlendOperation::ReverseSubtract,
        BlendOperation::Min => MTLBlendOperation::Min,
        BlendOperation::Max => MTLBlendOperation::Max,
    }
}

/// A built pipeline and the binding map of its shader module(s).
pub struct PipelineEntry {
    pub state: Retained<Pipeline>,
    pub vertex: Rc<Compiled>,
    pub fragment: Option<Rc<Compiled>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompareFunction {
    Never,
    Less,
    Equal,
    LessEqual,
    Greater,
    NotEqual,
    GreaterEqual,
    Always,
}

pub struct Gfx {
    pub device: Retained<Device>,
    pub queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    pub meshes: HashMap<AssetId, GpuMesh>,
    pub images: HashMap<AssetId, GpuImage>,
    pub buffers: HashMap<AssetId, Retained<Buffer>>,
    samplers: HashMap<ImageSamplerDescriptor, Rc<Retained<Sampler>>>,
    modules: HashMap<(&'static str, Defs), Rc<Compiled>>,
    libraries: HashMap<(&'static str, Defs), Retained<ProtocolObject<dyn MTLLibrary>>>,
    pipelines: HashMap<PipelineKey, Rc<PipelineEntry>>,
    depth_states: HashMap<(CompareFunction, bool), Retained<DepthState>>,
    /// The command buffers of the frames in flight, oldest first.
    pub in_flight: std::collections::VecDeque<Retained<CommandBuffer>>,
    /// Bumped whenever an image or buffer asset appears on or leaves the GPU (cached bind groups know which they name).
    pub generation: u64,
}

impl Gfx {
    pub fn new() -> Result<Self> {
        let device = MTLCreateSystemDefaultDevice().context("no Metal device")?;
        let queue = device.newCommandQueue().context("Metal command queue")?;
        Ok(Gfx {
            device,
            queue,
            meshes: HashMap::new(),
            images: HashMap::new(),
            buffers: HashMap::new(),
            samplers: HashMap::new(),
            modules: HashMap::new(),
            libraries: HashMap::new(),
            pipelines: HashMap::new(),
            depth_states: HashMap::new(),
            in_flight: std::collections::VecDeque::new(),
            generation: 0,
        })
    }

    /// Waits until fewer than [`FRAMES_IN_FLIGHT`] frames are in flight (before encoding the next).
    pub fn wait_slot(&mut self) {
        while self.in_flight.len() >= FRAMES_IN_FLIGHT {
            let cb = self.in_flight.pop_front().unwrap();
            cb.waitUntilCompleted();
            if let Some(e) = cb.error() { eprintln!("metal: frame failed: {}", e.localizedDescription()); }
        }
    }

    /// Waits for every frame in flight (read-backs).
    pub fn wait_idle(&mut self) {
        while let Some(cb) = self.in_flight.pop_front() {
            cb.waitUntilCompleted();
            if let Some(e) = cb.error() { eprintln!("metal: frame failed: {}", e.localizedDescription()); }
        }
    }

    pub fn sampler(&mut self, d: ImageSamplerDescriptor) -> Result<Rc<Retained<Sampler>>> {
        if let Some(s) = self.samplers.get(&d) { return Ok(s.clone()); }
        let addr = |m: ImageAddressMode| match m {
            ImageAddressMode::ClampToEdge => MTLSamplerAddressMode::ClampToEdge,
            ImageAddressMode::Repeat => MTLSamplerAddressMode::Repeat,
            ImageAddressMode::MirrorRepeat => MTLSamplerAddressMode::MirrorRepeat,
        };
        let filt = |m: ImageFilterMode| match m {
            ImageFilterMode::Nearest => MTLSamplerMinMagFilter::Nearest,
            ImageFilterMode::Linear => MTLSamplerMinMagFilter::Linear,
        };
        let s = MTLSamplerDescriptor::new();
        s.setSAddressMode(addr(d.address_mode_u));
        s.setTAddressMode(addr(d.address_mode_v));
        s.setRAddressMode(addr(d.address_mode_w));
        s.setMagFilter(filt(d.mag_filter));
        s.setMinFilter(filt(d.min_filter));
        s.setMipFilter(match d.mipmap_filter { ImageFilterMode::Nearest => MTLSamplerMipFilter::Nearest, ImageFilterMode::Linear => MTLSamplerMipFilter::Linear });
        s.setMaxAnisotropy(d.anisotropy_clamp.max(1) as usize);
        let st = Rc::new(self.device.newSamplerStateWithDescriptor(&s).context("Metal sampler")?);
        self.samplers.insert(d, st.clone());
        Ok(st)
    }

    /// Brings the GPU copies of every changed mesh, image and shader buffer up to date (Bevy's render-asset prepare).
    pub fn sync_assets(&mut self, meshes: &mut Assets<Mesh>, images: &mut Assets<Image>, buffers: &mut Assets<ShaderBuffer>) -> Result<()> {
        let (changed, removed) = meshes.take_changed();
        for id in removed { self.meshes.remove(&id); }
        for id in changed {
            let Some(m) = meshes.get(id) else { continue };
            let gm = self.upload_mesh(m)?;
            self.meshes.insert(id, gm);
        }
        let (changed, removed) = images.take_changed();
        if !removed.is_empty() { self.generation += 1; }
        for id in removed { self.images.remove(&id); }
        let known = self.images.len();
        for id in changed {
            let Some(img) = images.get(id) else { continue };
            // A render target keeps its texture while its size and format stay (its contents are the GPU's).
            if img.data.is_none() {
                if let Some(g) = self.images.get(&id) {
                    if g.size == img.size() && g.format == img.texture_descriptor.format { continue; }
                }
            }
            let gi = self.upload_image(img)?;
            self.images.insert(id, gi);
        }
        if self.images.len() != known { self.generation += 1; }
        let (changed, removed) = buffers.take_changed();
        if !removed.is_empty() { self.generation += 1; }
        for id in removed { self.buffers.remove(&id); }
        for id in changed {
            let Some(b) = buffers.get(id) else { continue };
            let bytes = b.data.as_deref().unwrap_or(&[]);
            // A fresh buffer: the frames in flight may still read the old one (module doc).
            let buf = new_buffer(&self.device, bytes, "shader buffer")?;
            if self.buffers.insert(id, buf).is_none() { self.generation += 1; }
        }
        Ok(())
    }

    fn upload_mesh(&self, m: &Mesh) -> Result<GpuMesh> {
        let vertices = new_buffer(&self.device, &m.vertex_bytes(), "mesh vertices")?;
        let indices = match &m.indices {
            Some(Indices::U32(v)) => {
                let b: Vec<u8> = v.iter().flat_map(|i| i.to_le_bytes()).collect();
                Some((new_buffer(&self.device, &b, "mesh indices")?, MTLIndexType::UInt32, v.len()))
            }
            Some(Indices::U16(v)) => {
                let b: Vec<u8> = v.iter().flat_map(|i| i.to_le_bytes()).collect();
                Some((new_buffer(&self.device, &b, "mesh indices")?, MTLIndexType::UInt16, v.len()))
            }
            None => None,
        };
        let layout = m.layout();
        let layout_hash = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            layout.hash(&mut h);
            m.topology.hash(&mut h);
            h.finish()
        };
        Ok(GpuMesh { vertices, layout, layout_hash, indices, vertex_count: m.count_vertices(), topology: m.topology })
    }

    fn upload_image(&mut self, img: &Image) -> Result<GpuImage> {
        let td = &img.texture_descriptor;
        let (w, h) = (td.size.width.max(1) as usize, td.size.height.max(1) as usize);
        let layers = td.size.depth_or_array_layers.max(1) as usize;
        let levels = td.mip_level_count.max(1) as usize;
        let d = MTLTextureDescriptor::new();
        d.setPixelFormat(pixel_format(td.format));
        // SAFETY: sizes within Metal's limits (the game's textures are at most 1024 wide; targets the window's size).
        unsafe {
            d.setWidth(w);
            d.setHeight(h);
        }
        if td.dimension == TextureDimension::D2 && layers > 1 {
            d.setTextureType(MTLTextureType::Type2DArray);
            // SAFETY: layer count within Metal's limit for these small arrays.
            unsafe { d.setArrayLength(layers) };
        }
        // SAFETY: `levels` is the image's own mip count.
        unsafe { d.setMipmapLevelCount(levels) };
        let mut usage = MTLTextureUsage::ShaderRead;
        if img.is_render_target() { usage |= MTLTextureUsage::RenderTarget; }
        if !td.view_formats.is_empty() { usage |= MTLTextureUsage::PixelFormatView; }
        d.setUsage(usage);
        d.setStorageMode(if img.is_render_target() { MTLStorageMode::Private } else { MTLStorageMode::Shared });
        let texture = self.device.newTextureWithDescriptor(&d).context("Metal texture")?;
        if let Some(data) = &img.data {
            // Bevy's layout: layer-major, then mip level, each tightly packed.
            let px = td.format.pixel_size();
            let mut off = 0usize;
            for layer in 0..layers {
                for l in 0..levels {
                    let (lw, lh) = ((w >> l).max(1), (h >> l).max(1));
                    let n = lw * lh * px;
                    if off + n > data.len() { break; }
                    let region = MTLRegion { origin: MTLOrigin { x: 0, y: 0, z: 0 }, size: MTLSize { width: lw, height: lh, depth: 1 } };
                    // SAFETY: `n` bytes of `data` from `off` hold this level's texels.
                    unsafe {
                        texture.replaceRegion_mipmapLevel_slice_withBytes_bytesPerRow_bytesPerImage(
                            region,
                            l,
                            layer,
                            NonNull::new(data[off..].as_ptr() as *mut c_void).unwrap(),
                            lw * px,
                            n,
                        );
                    }
                    off += n;
                }
            }
        }
        let sampler = self.sampler(img.sampler.resolve())?;
        Ok(GpuImage { texture, sampler, format: td.format, size: img.size() })
    }

    /// Shader module `name` with `defs`, translated once.
    pub fn module(&mut self, name: &'static str, defs: &Defs) -> Result<Rc<Compiled>> {
        let key = (name, defs.clone());
        if let Some(m) = self.modules.get(&key) { return Ok(m.clone()); }
        let c = Rc::new(shader::load(name, defs)?);
        self.modules.insert(key, c.clone());
        Ok(c)
    }

    fn library(&mut self, name: &'static str, defs: &Defs) -> Result<Retained<ProtocolObject<dyn MTLLibrary>>> {
        let key = (name, defs.clone());
        if let Some(l) = self.libraries.get(&key) { return Ok(l.clone()); }
        let m = self.module(name, defs)?;
        // wgpu-hal's compile options: the MSL version naga wrote for, invariant positions preserved (the default fast
        // math stays on, as there).
        let options = MTLCompileOptions::new();
        options.setLanguageVersion(MTLLanguageVersion::Version3_2);
        options.setPreserveInvariance(true);
        let lib = self
            .device
            .newLibraryWithSource_options_error(&ns(&m.msl), Some(&options))
            .map_err(|e| anyhow!("{name}.wgsl: Metal: {}", e.localizedDescription()))?;
        self.libraries.insert(key, lib.clone());
        Ok(lib)
    }

    fn function(&mut self, name: &'static str, defs: &Defs, entry: &str) -> Result<Retained<ProtocolObject<dyn MTLFunction>>> {
        let m = self.module(name, defs)?;
        let msl_name = m.entry_points.get(entry).ok_or_else(|| anyhow!("{name}.wgsl has no entry point {entry}"))?.clone();
        let lib = self.library(name, defs)?;
        lib.newFunctionWithName(&ns(&msl_name)).ok_or_else(|| anyhow!("{name}.wgsl: no MSL function {msl_name}"))
    }

    pub fn pipeline(&mut self, key: &PipelineKey) -> Result<Rc<PipelineEntry>> {
        if let Some(p) = self.pipelines.get(key) { return Ok(p.clone()); }
        let vshader = key.vertex_shader.unwrap_or(key.shader);
        let vf = self.function(vshader, &key.defs, key.vertex_entry)?;
        let d = MTLRenderPipelineDescriptor::new();
        d.setLabel(Some(&ns(&format!("{}:{}", key.shader, key.fragment_entry.unwrap_or("-")))));
        d.setVertexFunction(Some(&vf));
        let fragment = match key.fragment_entry {
            Some(e) => {
                let ff = self.function(key.shader, &key.defs, e)?;
                d.setFragmentFunction(Some(&ff));
                Some(self.module(key.shader, &key.defs)?)
            }
            None => None,
        };
        if !key.vertex_attributes.is_empty() {
            let vd = MTLVertexDescriptor::new();
            for &(loc, fmt, off) in &key.vertex_attributes {
                // SAFETY: attribute indices are WGSL locations (< 31).
                let a = unsafe { vd.attributes().objectAtIndexedSubscript(loc as usize) };
                a.setFormat(vertex_format(fmt));
                // SAFETY: offsets lie inside the stride.
                unsafe {
                    a.setOffset(off as usize);
                    a.setBufferIndex(shader::VERTEX_BUFFER_BASE as usize);
                }
            }
            // SAFETY: the one vertex buffer slot.
            let l = unsafe { vd.layouts().objectAtIndexedSubscript(shader::VERTEX_BUFFER_BASE as usize) };
            // SAFETY: a non-zero stride (Metal requires a multiple of 4; every format is).
            unsafe { l.setStride(key.vertex_stride as usize) };
            l.setStepFunction(MTLVertexStepFunction::PerVertex);
            d.setVertexDescriptor(Some(&vd));
        }
        for (i, f) in key.color_formats.iter().enumerate() {
            // SAFETY: attachment index < 8.
            let ca = unsafe { d.colorAttachments().objectAtIndexedSubscript(i) };
            ca.setPixelFormat(pixel_format(*f));
            ca.setWriteMask(key.write_mask);
            if let Some(b) = key.blend {
                ca.setBlendingEnabled(true);
                ca.setRgbBlendOperation(blend_op(b.color.operation));
                ca.setSourceRGBBlendFactor(blend_factor(b.color.src_factor));
                ca.setDestinationRGBBlendFactor(blend_factor(b.color.dst_factor));
                ca.setAlphaBlendOperation(blend_op(b.alpha.operation));
                ca.setSourceAlphaBlendFactor(blend_factor(b.alpha.src_factor));
                ca.setDestinationAlphaBlendFactor(blend_factor(b.alpha.dst_factor));
            }
        }
        if key.depth { d.setDepthAttachmentPixelFormat(DEPTH_FORMAT); }
        // 1, 2, 4 or 8 samples (the device supports them; crate::render::frame clamps).
        d.setRasterSampleCount(key.samples.max(1) as usize);
        // SAFETY: the class matches every draw made with this pipeline (part of the key).
        unsafe {
            d.setInputPrimitiveTopology(match key.topology_class {
                TopologyClass::Point => MTLPrimitiveTopologyClass::Point,
                TopologyClass::Line => MTLPrimitiveTopologyClass::Line,
                TopologyClass::Triangle => MTLPrimitiveTopologyClass::Triangle,
            })
        };
        let state = self
            .device
            .newRenderPipelineStateWithDescriptor_error(&d)
            .map_err(|e| anyhow!("pipeline {}: {}", key.shader, e.localizedDescription()))?;
        let vertex = self.module(vshader, &key.defs)?;
        let entry = Rc::new(PipelineEntry { state, vertex, fragment });
        self.pipelines.insert(key.clone(), entry.clone());
        Ok(entry)
    }

    pub fn depth_state(&mut self, compare: CompareFunction, write: bool) -> Result<Retained<DepthState>> {
        if let Some(d) = self.depth_states.get(&(compare, write)) { return Ok(d.clone()); }
        let d = MTLDepthStencilDescriptor::new();
        d.setDepthCompareFunction(match compare {
            CompareFunction::Never => MTLCompareFunction::Never,
            CompareFunction::Less => MTLCompareFunction::Less,
            CompareFunction::Equal => MTLCompareFunction::Equal,
            CompareFunction::LessEqual => MTLCompareFunction::LessEqual,
            CompareFunction::Greater => MTLCompareFunction::Greater,
            CompareFunction::NotEqual => MTLCompareFunction::NotEqual,
            CompareFunction::GreaterEqual => MTLCompareFunction::GreaterEqual,
            CompareFunction::Always => MTLCompareFunction::Always,
        });
        d.setDepthWriteEnabled(write);
        let s = self.device.newDepthStencilStateWithDescriptor(&d).context("Metal depth state")?;
        self.depth_states.insert((compare, write), s.clone());
        Ok(s)
    }

    /// A private render target texture (depth, MSAA colour, effect targets…).
    pub fn target(&self, format: MTLPixelFormat, w: usize, h: usize, samples: u32, sampled: bool) -> Result<Retained<Texture>> {
        let d = MTLTextureDescriptor::new();
        d.setPixelFormat(format);
        // SAFETY: as in upload_image.
        unsafe {
            d.setWidth(w.max(1));
            d.setHeight(h.max(1));
        }
        if samples > 1 {
            d.setTextureType(MTLTextureType::Type2DMultisample);
            // SAFETY: a supported sample count.
            unsafe { d.setSampleCount(samples as usize) };
        }
        let mut usage = MTLTextureUsage::RenderTarget;
        if sampled { usage |= MTLTextureUsage::ShaderRead; }
        d.setUsage(usage);
        d.setStorageMode(MTLStorageMode::Private);
        self.device.newTextureWithDescriptor(&d).context("Metal render target")
    }
}

/// One resource to bind: what a WGSL `@group(g) @binding(b)` receives.
#[derive(Clone)]
pub enum Binding {
    /// Bytes copied into the command stream (uniforms; up to 4 KB).
    Bytes(Rc<Vec<u8>>),
    Buffer(Retained<Buffer>),
    Texture(Retained<Texture>),
    Sampler(Rc<Retained<Sampler>>),
    /// A shader-buffer asset's current GPU buffer (resolved when bound; [`bind_assets`]).
    AssetBuffer(AssetId),
    /// An image asset's current texture / its sampler (resolved when bound).
    AssetTexture(AssetId),
    AssetSampler(AssetId),
}

/// [`bind_assets`] without asset bindings.
pub fn bind(enc: &Encoder, module: &Compiled, vertex_stage: bool, resources: &[(u32, u32, &Binding)]) { bind_assets(enc, module, vertex_stage, resources, None) }

/// Binds `resources` (group, binding → resource) for `module` on the stage(s) that read them, and fills naga's sizes
/// buffer; asset bindings resolve through `gfx`. Missing bindings the shader declares are skipped (Metal then reads
/// zero).
pub fn bind_assets(enc: &Encoder, module: &Compiled, vertex_stage: bool, resources: &[(u32, u32, &Binding)], gfx: Option<&Gfx>) {
    let resolved: Vec<(u32, u32, Binding)> = resources
        .iter()
        .filter_map(|(g, n, b)| {
            let r = match b {
                Binding::AssetBuffer(id) => Binding::Buffer(gfx?.buffers.get(id)?.clone()),
                Binding::AssetTexture(id) => Binding::Texture(gfx?.images.get(id)?.texture.clone()),
                Binding::AssetSampler(id) => Binding::Sampler(gfx?.images.get(id)?.sampler.clone()),
                other => (*other).clone(),
            };
            Some((*g, *n, r))
        })
        .collect();
    let resources: Vec<(u32, u32, &Binding)> = resolved.iter().map(|(g, n, b)| (*g, *n, b)).collect();
    let resources = resources.as_slice();
    let mut sizes: Vec<u32> = Vec::new();
    for r in &module.resources {
        let Some((_, _, b)) = resources.iter().find(|(g, n, _)| *g == r.group && *n == r.binding) else {
            if r.kind == ResourceKind::Buffer && module.sizes.as_ref().is_some_and(|(_, l)| l.contains(&(r.group, r.binding))) { sizes.push(0); }
            continue;
        };
        let slot = r.slot as usize;
        // SAFETY: slots come from the module's own binding map; bytes outlive the call (Metal copies them).
        unsafe {
            match (r.kind, b) {
                (ResourceKind::Buffer, Binding::Bytes(bytes)) => {
                    let p = NonNull::new(bytes.as_ptr() as *mut c_void).unwrap();
                    let n = bytes.len().max(16);
                    let mut padded;
                    let p = if bytes.len() < 16 {
                        padded = bytes.to_vec();
                        padded.resize(16, 0);
                        NonNull::new(padded.as_mut_ptr() as *mut c_void).unwrap()
                    } else {
                        p
                    };
                    if vertex_stage { enc.setVertexBytes_length_atIndex(p, n, slot) } else { enc.setFragmentBytes_length_atIndex(p, n, slot) }
                }
                (ResourceKind::Buffer, Binding::Buffer(buf)) => {
                    if vertex_stage { enc.setVertexBuffer_offset_atIndex(Some(buf), 0, slot) } else { enc.setFragmentBuffer_offset_atIndex(Some(buf), 0, slot) }
                }
                (ResourceKind::Texture, Binding::Texture(t)) => {
                    if vertex_stage { enc.setVertexTexture_atIndex(Some(t), slot) } else { enc.setFragmentTexture_atIndex(Some(t), slot) }
                }
                (ResourceKind::Sampler, Binding::Sampler(s)) => {
                    if vertex_stage { enc.setVertexSamplerState_atIndex(Some(s), slot) } else { enc.setFragmentSamplerState_atIndex(Some(s), slot) }
                }
                _ => eprintln!("metal: binding ({}, {}) has the wrong kind for {:?}", r.group, r.binding, r.kind),
            }
        }
        if let Some((_, list)) = &module.sizes {
            if list.contains(&(r.group, r.binding)) {
                let len = match b { Binding::Bytes(v) => v.len(), Binding::Buffer(buf) => buf.length(), _ => 0 };
                sizes.push(len as u32);
            }
        }
    }
    if let Some((slot, list)) = &module.sizes {
        sizes.resize(list.len().max(1), 0);
        // SAFETY: naga's sizes struct: one u32 per runtime-sized buffer, in declaration order.
        unsafe {
            let p = NonNull::new(sizes.as_mut_ptr() as *mut c_void).unwrap();
            let n = (sizes.len() * 4).max(16);
            let mut padded = sizes.clone();
            padded.resize(n / 4, 0);
            let p = if sizes.len() * 4 < 16 { NonNull::new(padded.as_mut_ptr() as *mut c_void).unwrap() } else { p };
            if vertex_stage { enc.setVertexBytes_length_atIndex(p, n, *slot as usize) } else { enc.setFragmentBytes_length_atIndex(p, n, *slot as usize) }
        }
    }
}
