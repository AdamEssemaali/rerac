//! The Metal side: device, pipelines, the level's buffers and textures, and one frame's encoding into a colour target
//! (the window's drawable or an offscreen texture for screenshots).
//!
//! Frame layout as rc-engine's: the 3D view fills a centred 4:3 box of the target (the PS2's 512×416 at the TV's 4:3,
//! rc-engine `display.rs`), black around it; inside it the level background (the GS clear colour) where no tfrag
//! draws. Colour is `BGRA8Unorm_sRGB` (rc-engine's `Bgra8UnormSrgb` frame), depth `Depth32Float` with reverse Z (clear
//! 0, test GEQUAL, as the GS's ZTST GEQUAL). Draw order: every `Opaque` draw, then `OpaqueTested`, then
//! `ColorOnlyLowAlpha` (Bevy's Opaque3d → AlphaMask3d → Transparent3d).

use crate::camera::{clip_from_view, FlyCam, LodBlock, TfragLod};
use crate::level::Level;
use crate::tfrag::{self, GsPass, SamplerKey, TfragScene, AREF_WORLD};
use anyhow::{anyhow, Context, Result};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::*;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;

pub type Device = ProtocolObject<dyn MTLDevice>;
pub type Texture = ProtocolObject<dyn MTLTexture>;
type Buffer = ProtocolObject<dyn MTLBuffer>;

pub const COLOR_FORMAT: MTLPixelFormat = MTLPixelFormat::BGRA8Unorm_sRGB;
const DEPTH_FORMAT: MTLPixelFormat = MTLPixelFormat::Depth32Float;
/// The frame's aspect (rc-engine `display::Aspect::Original`).
const ASPECT: f64 = 4.0 / 3.0;

/// `View` in tfrag.metal.
#[repr(C)]
struct ViewBlock {
    clip_from_world: [f32; 16],
    view_from_world: [f32; 16],
}

/// `DrawState` in tfrag.metal.
#[repr(C)]
struct DrawState {
    atest: u32,
    aref: u32,
}

pub struct Renderer {
    pub device: Retained<Device>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    /// The background fill of the 4:3 box (`clear_box`).
    clear_pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    clear_depth: Retained<ProtocolObject<dyn MTLDepthStencilState>>,
    /// Blend off / on.
    pipelines: [Retained<ProtocolObject<dyn MTLRenderPipelineState>>; 2],
    /// Z write off / on (both GEQUAL).
    depth_states: [Retained<ProtocolObject<dyn MTLDepthStencilState>>; 2],
    samplers: HashMap<SamplerKey, Retained<ProtocolObject<dyn MTLSamplerState>>>,
    textures: Vec<Retained<Texture>>,
    slots: Retained<Buffer>,
    vinfos: Retained<Buffer>,
    refs: Retained<Buffer>,
    indices: Retained<Buffer>,
    /// The per-frame draw modes, one buffer per frame in flight.
    modes: Vec<Retained<Buffer>>,
    frame: usize,
    /// The command buffer that last read each `modes` buffer.
    in_flight: Vec<Option<Retained<ProtocolObject<dyn MTLCommandBuffer>>>>,
    scene: TfragScene,
    lod: TfragLod,
    lod_block: LodBlock,
    fog: [f32; 12],
    background: [u8; 3],
    depth: Option<Retained<Texture>>,
    mode_scratch: Vec<u32>,
}

const FRAMES_IN_FLIGHT: usize = 3;

fn ns(s: &str) -> Retained<NSString> { NSString::from_str(s) }

fn new_buffer(device: &Device, bytes: &[u8], label: &str) -> Result<Retained<Buffer>> {
    // Metal rejects zero-length buffers.
    let len = bytes.len().max(4);
    let buf = if bytes.is_empty() {
        device.newBufferWithLength_options(len, MTLResourceOptions::StorageModeShared)
    } else {
        // SAFETY: `bytes` is valid for `len` bytes and Metal copies it before returning.
        unsafe {
            device.newBufferWithBytes_length_options(
                NonNull::new(bytes.as_ptr() as *mut c_void).unwrap(),
                len,
                MTLResourceOptions::StorageModeShared,
            )
        }
    }
    .ok_or_else(|| anyhow!("Metal could not allocate the {label} buffer ({len} bytes)"))?;
    buf.setLabel(Some(&ns(label)));
    Ok(buf)
}

fn as_bytes<T: Copy>(v: &[T]) -> &[u8] {
    // SAFETY: plain-old-data slices (u32 arrays and #[repr(C)] f32 blocks) viewed as bytes.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

impl Renderer {
    pub fn new(device: Retained<Device>, level: &Level) -> Result<Self> {
        let queue = device.newCommandQueue().context("Metal command queue")?;
        let source = ns(include_str!("../shaders/tfrag.metal"));
        let library = device
            .newLibraryWithSource_options_error(&source, None)
            .map_err(|e| anyhow!("compiling tfrag.metal: {}", e.localizedDescription()))?;
        let vf = library.newFunctionWithName(&ns("tfrag_vertex")).context("tfrag_vertex")?;
        let ff = library.newFunctionWithName(&ns("tfrag_fragment")).context("tfrag_fragment")?;
        let pipeline = |blend: bool| -> Result<_> {
            let d = MTLRenderPipelineDescriptor::new();
            d.setLabel(Some(&ns(if blend { "tfrag blended" } else { "tfrag opaque" })));
            d.setVertexFunction(Some(&vf));
            d.setFragmentFunction(Some(&ff));
            d.setDepthAttachmentPixelFormat(DEPTH_FORMAT);
            // SAFETY: index 0 is in range of the colour attachment array.
            let ca = unsafe { d.colorAttachments().objectAtIndexedSubscript(0) };
            ca.setPixelFormat(COLOR_FORMAT);
            if blend {
                // The GS equation `((Cs − Cd)·As >> 7) + Cd` with the fragment alpha As/128 (rc-engine `GS_BLEND`).
                ca.setBlendingEnabled(true);
                ca.setRgbBlendOperation(MTLBlendOperation::Add);
                ca.setSourceRGBBlendFactor(MTLBlendFactor::SourceAlpha);
                ca.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
                ca.setAlphaBlendOperation(MTLBlendOperation::Add);
                ca.setSourceAlphaBlendFactor(MTLBlendFactor::One);
                ca.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
            }
            device
                .newRenderPipelineStateWithDescriptor_error(&d)
                .map_err(|e| anyhow!("tfrag pipeline: {}", e.localizedDescription()))
        };
        let pipelines = [pipeline(false)?, pipeline(true)?];
        let clear_pipeline = {
            let cv = library.newFunctionWithName(&ns("clear_vertex")).context("clear_vertex")?;
            let cf = library.newFunctionWithName(&ns("clear_fragment")).context("clear_fragment")?;
            let d = MTLRenderPipelineDescriptor::new();
            d.setLabel(Some(&ns("background")));
            d.setVertexFunction(Some(&cv));
            d.setFragmentFunction(Some(&cf));
            d.setDepthAttachmentPixelFormat(DEPTH_FORMAT);
            // SAFETY: index 0 is in range of the colour attachment array.
            unsafe { d.colorAttachments().objectAtIndexedSubscript(0) }.setPixelFormat(COLOR_FORMAT);
            device
                .newRenderPipelineStateWithDescriptor_error(&d)
                .map_err(|e| anyhow!("background pipeline: {}", e.localizedDescription()))?
        };
        let depth_state = |write: bool| -> Result<_> {
            let d = MTLDepthStencilDescriptor::new();
            d.setDepthCompareFunction(MTLCompareFunction::GreaterEqual);
            d.setDepthWriteEnabled(write);
            device.newDepthStencilStateWithDescriptor(&d).context("Metal depth state")
        };
        let depth_states = [depth_state(false)?, depth_state(true)?];
        let clear_depth = {
            let d = MTLDepthStencilDescriptor::new();
            d.setDepthCompareFunction(MTLCompareFunction::Always);
            d.setDepthWriteEnabled(false);
            device.newDepthStencilStateWithDescriptor(&d).context("Metal depth state")?
        };

        let t0 = std::time::Instant::now();
        let scene = tfrag::build(level);
        let build = t0.elapsed();
        let t0 = std::time::Instant::now();
        let mut samplers = HashMap::new();
        for d in &scene.draws {
            if samplers.contains_key(&d.sampler) { continue; }
            samplers.insert(d.sampler, new_sampler(&device, d.sampler)?);
        }
        let textures = scene.images.iter().map(|img| new_texture(&device, img)).collect::<Result<Vec<_>>>()?;
        let slots = new_buffer(&device, &scene.slots, "tfrag slots")?;
        let vinfos = new_buffer(&device, &scene.vinfos, "tfrag vinfos")?;
        let refs = new_buffer(&device, as_bytes(&scene.refs), "tfrag refs")?;
        let indices = new_buffer(&device, as_bytes(&scene.indices), "tfrag indices")?;
        let n = level.tfrags.len().max(1);
        let modes = (0..FRAMES_IN_FLIGHT).map(|_| new_buffer(&device, &vec![0u8; n * 4], "tfrag modes")).collect::<Result<Vec<_>>>()?;
        println!(
            "metal: {} on {}: {} tfrags, {} LOD-0 triangles, {} vertices, {} draws, {} textures (built in {:.0?}, uploaded in {:.0?})",
            level.index,
            device.name(),
            level.tfrags.len(),
            scene.triangles,
            scene.refs.len(),
            scene.draws.len(),
            textures.len(),
            build,
            t0.elapsed()
        );
        Ok(Renderer {
            queue,
            clear_pipeline,
            clear_depth,
            pipelines,
            depth_states,
            samplers,
            textures,
            slots,
            vinfos,
            refs,
            indices,
            modes,
            frame: 0,
            in_flight: vec![None; FRAMES_IN_FLIGHT],
            lod: TfragLod::new(level),
            lod_block: LodBlock::new(&level.block, &level.fog),
            fog: level.fog.shader_block(),
            background: level.background,
            scene,
            depth: None,
            mode_scratch: Vec::new(),
            device,
        })
    }

    /// The 4:3 box of a `w × h` target, centred (x, y, width, height in pixels).
    fn view_box(w: usize, h: usize) -> (f64, f64, f64, f64) {
        let (w, h) = (w as f64, h as f64);
        let (bw, bh) = if w / h > ASPECT { ((h * ASPECT).round(), h) } else { (w, (w / ASPECT).round()) };
        (((w - bw) / 2.0).floor(), ((h - bh) / 2.0).floor(), bw, bh)
    }

    /// Encodes one frame into `target` and commits it; presents `drawable` when given. Returns the command buffer so
    /// a caller can wait on it (screenshots).
    pub fn render(
        &mut self,
        cam: &FlyCam,
        target: &Texture,
        drawable: Option<&ProtocolObject<dyn MTLDrawable>>,
    ) -> Result<Retained<ProtocolObject<dyn MTLCommandBuffer>>> {
        let (w, h) = (target.width(), target.height());
        if self.depth.as_ref().is_none_or(|d| d.width() != w || d.height() != h) {
            // SAFETY: plain texture descriptor; the depth texture lives only on the GPU.
            let d = unsafe { MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(DEPTH_FORMAT, w, h, false) };
            d.setUsage(MTLTextureUsage::RenderTarget);
            d.setStorageMode(MTLStorageMode::Private);
            self.depth = Some(self.device.newTextureWithDescriptor(&d).context("Metal depth texture")?);
        }

        // This frame's draw modes (`TfragProc` on the CPU), into the next buffer in the ring.
        self.lod.modes(cam, &mut self.mode_scratch);
        let slot = self.frame % FRAMES_IN_FLIGHT;
        self.frame += 1;
        if let Some(cb) = self.in_flight[slot].take() { cb.waitUntilCompleted(); }
        let modes = &self.modes[slot];
        let bytes = as_bytes(&self.mode_scratch);
        // SAFETY: shared-storage buffer sized for one u32 per tfrag; the ring keeps the GPU off the one written here
        // (the command buffer that last read this one has completed: `in_flight`).
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), modes.contents().as_ptr() as *mut u8, bytes.len().min(modes.length())) };

        let view_from_world = cam.view_from_world();
        let view = ViewBlock {
            clip_from_world: (clip_from_view() * view_from_world).to_cols_array(),
            view_from_world: view_from_world.to_cols_array(),
        };

        let cb = self.queue.commandBuffer().context("Metal command buffer")?;
        // The bars: clear the whole target black, then the 4:3 box to the level background.
        let pass = MTLRenderPassDescriptor::renderPassDescriptor();
        // SAFETY: index 0 is in range of the colour attachment array.
        let ca = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        ca.setTexture(Some(target));
        ca.setLoadAction(MTLLoadAction::Clear);
        ca.setClearColor(MTLClearColor { red: 0.0, green: 0.0, blue: 0.0, alpha: 1.0 });
        ca.setStoreAction(MTLStoreAction::Store);
        let da = pass.depthAttachment();
        da.setTexture(self.depth.as_deref());
        da.setLoadAction(MTLLoadAction::Clear);
        // Reverse Z: 0 is the farthest (the GS clear sprite's Z = 0).
        da.setClearDepth(0.0);
        da.setStoreAction(MTLStoreAction::DontCare);
        let enc = cb.renderCommandEncoderWithDescriptor(&pass).context("Metal render encoder")?;
        enc.setLabel(Some(&ns("tfrag")));
        let (bx, by, bw, bh) = Self::view_box(w, h);
        enc.setViewport(MTLViewport { originX: bx, originY: by, width: bw, height: bh, znear: 0.0, zfar: 1.0 });
        enc.setScissorRect(MTLScissorRect { x: bx as usize, y: by as usize, width: bw as usize, height: bh as usize });
        self.clear_box(&enc);

        enc.setCullMode(MTLCullMode::None);
        // SAFETY: the byte blocks match the MSL structs (View, TfragFog) and outlive the calls (Metal copies them).
        unsafe {
            enc.setVertexBytes_length_atIndex(NonNull::from(&view).cast(), std::mem::size_of::<ViewBlock>(), 0);
            enc.setVertexBytes_length_atIndex(NonNull::from(&self.fog).cast(), std::mem::size_of_val(&self.fog), 1);
            enc.setFragmentBytes_length_atIndex(NonNull::from(&self.fog).cast(), std::mem::size_of_val(&self.fog), 1);
            enc.setVertexBuffer_offset_atIndex(Some(&self.slots), 0, 3);
            enc.setVertexBuffer_offset_atIndex(Some(&self.vinfos), 0, 4);
            enc.setVertexBuffer_offset_atIndex(Some(modes), 0, 5);
            enc.setVertexBuffer_offset_atIndex(Some(&self.refs), 0, 6);
        }
        let mut bound: Option<(bool, bool, usize, SamplerKey, u32)> = None;
        for d in &self.scene.draws {
            let (blend, zwrite, state) = match d.pass {
                GsPass::Opaque => (false, true, DrawState { atest: 0, aref: 0 }),
                GsPass::OpaqueTested => (true, true, DrawState { atest: 1, aref: AREF_WORLD as u32 }),
                GsPass::ColorOnlyLowAlpha => (true, false, DrawState { atest: 2, aref: AREF_WORLD as u32 }),
            };
            let key = (blend, zwrite, d.image, d.sampler, d.mxl);
            if bound != Some(key) {
                enc.setRenderPipelineState(&self.pipelines[blend as usize]);
                enc.setDepthStencilState(Some(&self.depth_states[zwrite as usize]));
                let mut lod = self.lod_block;
                lod.tex[1] = d.mxl as f32;
                // SAFETY: as above (TfragLod, DrawState), and the texture / sampler indices match the shader.
                unsafe {
                    enc.setVertexBytes_length_atIndex(NonNull::from(&lod).cast(), std::mem::size_of::<LodBlock>(), 2);
                    enc.setFragmentBytes_length_atIndex(NonNull::from(&lod).cast(), std::mem::size_of::<LodBlock>(), 2);
                    enc.setFragmentBytes_length_atIndex(NonNull::from(&state).cast(), std::mem::size_of::<DrawState>(), 7);
                    enc.setFragmentTexture_atIndex(Some(&self.textures[d.image]), 0);
                    enc.setFragmentSamplerState_atIndex(Some(&self.samplers[&d.sampler]), 0);
                }
                bound = Some(key);
            }
            // SAFETY: the range lies in the index buffer (built with it in tfrag::build).
            unsafe {
                enc.drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferOffset(
                    MTLPrimitiveType::Triangle,
                    d.index_count as usize,
                    MTLIndexType::UInt32,
                    &self.indices,
                    d.first_index as usize * 4,
                );
            }
        }
        enc.endEncoding();
        if let Some(d) = drawable { cb.presentDrawable(d); }
        cb.commit();
        self.in_flight[slot] = Some(cb.clone());
        Ok(cb)
    }

    /// Fills the bound 4:3 box with the level background: one full-viewport triangle through the clear pipeline.
    /// (A render pass clears the whole attachment, and the bars must stay black.)
    fn clear_box(&self, enc: &ProtocolObject<dyn MTLRenderCommandEncoder>) {
        let c = self.background.map(|b| b as f32 / 255.0);
        let color = [c[0], c[1], c[2], 1.0f32];
        enc.setRenderPipelineState(&self.clear_pipeline);
        enc.setDepthStencilState(Some(&self.clear_depth));
        // SAFETY: a float4, as clear_fragment reads it.
        unsafe {
            enc.setFragmentBytes_length_atIndex(NonNull::from(&color).cast(), 16, 0);
            enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3);
        }
    }
}

fn new_sampler(device: &Device, key: SamplerKey) -> Result<Retained<ProtocolObject<dyn MTLSamplerState>>> {
    // TEX1_1 MMAG = LINEAR, MMIN = LINEAR_MIPMAP_NEAREST; the shader picks the level (an integer level reads that level
    // alone through the linear mip filter); anisotropy 16 as rc-engine's sampler.
    let wrap = |clamp: bool| if clamp { MTLSamplerAddressMode::ClampToEdge } else { MTLSamplerAddressMode::Repeat };
    let d = MTLSamplerDescriptor::new();
    d.setMinFilter(MTLSamplerMinMagFilter::Linear);
    d.setMagFilter(MTLSamplerMinMagFilter::Linear);
    d.setMipFilter(MTLSamplerMipFilter::Linear);
    d.setMaxAnisotropy(16);
    d.setSAddressMode(wrap(key.clamp_s));
    d.setTAddressMode(wrap(key.clamp_t));
    device.newSamplerStateWithDescriptor(&d).context("Metal sampler")
}

fn new_texture(device: &Device, img: &tfrag::TfragImage) -> Result<Retained<Texture>> {
    let (w, h, _) = img.levels[0];
    // Keep the levels that halve as Metal's mip chain expects (all of them on the disc).
    let n = img.levels.iter().enumerate().take_while(|(l, (lw, lh, _))| *lw == (w >> l).max(1) && *lh == (h >> l).max(1)).count();
    // SAFETY: plain texture descriptor.
    let d = unsafe { MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(MTLPixelFormat::RGBA8Unorm, w as usize, h as usize, n > 1) };
    // SAFETY: `n` ≥ 1 and within the chain of a w × h texture.
    unsafe { d.setMipmapLevelCount(n) };
    d.setUsage(MTLTextureUsage::ShaderRead);
    let tex = device.newTextureWithDescriptor(&d).context("Metal texture")?;
    for (l, (lw, lh, rgba)) in img.levels.iter().take(n).enumerate() {
        let region = MTLRegion { origin: MTLOrigin { x: 0, y: 0, z: 0 }, size: MTLSize { width: *lw as usize, height: *lh as usize, depth: 1 } };
        // SAFETY: `rgba` holds lw × lh RGBA8 texels.
        unsafe {
            tex.replaceRegion_mipmapLevel_withBytes_bytesPerRow(region, l, NonNull::new(rgba.as_ptr() as *mut c_void).unwrap(), *lw as usize * 4);
        }
    }
    Ok(tex)
}

/// An offscreen colour target of `w × h` that can be read back ([`read_back`]).
pub fn offscreen_target(device: &Device, w: usize, h: usize) -> Result<Retained<Texture>> {
    // SAFETY: plain texture descriptor.
    let d = unsafe { MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(COLOR_FORMAT, w, h, false) };
    d.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
    d.setStorageMode(MTLStorageMode::Private);
    device.newTextureWithDescriptor(&d).context("Metal offscreen target")
}

/// The texels of a finished `target` as RGBA8 rows (the stored sRGB bytes, as a PNG wants them).
pub fn read_back(device: &Device, queue_owner: &Renderer, target: &Texture) -> Result<Vec<u8>> {
    let (w, h) = (target.width(), target.height());
    let buf = device.newBufferWithLength_options(w * h * 4, MTLResourceOptions::StorageModeShared).context("Metal read-back buffer")?;
    let cb = queue_owner.queue.commandBuffer().context("Metal command buffer")?;
    let blit = cb.blitCommandEncoder().context("Metal blit encoder")?;
    // SAFETY: the copy covers the whole level-0 slice into a buffer of exactly that size.
    unsafe {
        blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
            target,
            0,
            0,
            MTLOrigin { x: 0, y: 0, z: 0 },
            MTLSize { width: w, height: h, depth: 1 },
            &buf,
            0,
            w * 4,
            w * h * 4,
        );
    }
    blit.endEncoding();
    cb.commit();
    cb.waitUntilCompleted();
    // SAFETY: the GPU has finished writing the shared buffer.
    let bgra = unsafe { std::slice::from_raw_parts(buf.contents().as_ptr() as *const u8, w * h * 4) };
    Ok(bgra.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], 255]).collect())
}
