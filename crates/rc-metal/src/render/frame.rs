//! One frame: every active camera, in Bevy's camera order, renders its view into its target with Bevy 0.19's Core3d /
//! Core2d structure and rc-engine's additions (docs/plan/metal_renderer.md, "Frame").
//!
//! Per 3D camera:
//! 1. **Main pass A**: clear (per `ClearColorConfig`, depth per [`Camera3dDepth`]), then the Opaque3d items, then the
//!    AlphaMask3d items (Bevy's binned phases, ordered by pipeline creation, material, mesh, entity), then the world's
//!    colour-only items in the game's order (rc-engine `pre_shadow`: [`BeforeShadows`] entities, by [`WorldDrawOrder`]).
//! 2. The hooks registered for [`Hook::AfterOpaque`] (rc-engine's shadow-volume pass).
//! 3. **Main pass B**: the remaining Transparent3d items, back to front (distance of the mesh AABB centre in view space
//!    plus the material's depth bias, ties by entity: rc-engine `determinism::stable_transparent_order`), without the
//!    [`DisplayEffect`] items.
//! 4. **Effect pass** (rc-engine `display_blend`): when the view has effect items, the frame is converted to display bytes
//!    (`effect_blit.wgsl` `to_display`), the effect items are drawn there in their sorted order against the scene depth,
//!    and the result converted back (`to_linear`).
//! 5. The camera's full-screen passes ([`FullscreenPasses`], in their registered order) and the UI nodes that target it.
//!
//! Depth textures are shared by cameras rendering to the same target (Bevy's `prepare_core_3d_depth_textures`), which
//! is how the sky camera and the main camera share one. Cameras render straight into their target: Bevy's intermediate
//! main texture holds the same 8-bit sRGB values and is copied out unchanged.

use super::gfx::{self, bind, Binding, CommandBuffer, Encoder, Gfx, PipelineKey, Texture, TopologyClass};
use super::material::{AlphaMode, BindGroup, CachedPipeline, Face, Material, MeshMaterial2d, MeshMaterial3d, MeshTag, Phase, PipelineDesc, Std140};
use crate::engine::assets::{AssetId, Assets, Handle};
use crate::engine::ecs::{Ctx, Entity, World};
use crate::engine::image::{Image, TextureFormat};
use crate::engine::mesh::{Mesh, Mesh2d, Mesh3d};
use crate::engine::scene::*;
use anyhow::{Context, Result};
use glam::{Mat4, UVec2, Vec2, Vec3, Vec4};
use objc2::rc::Retained;
use objc2::Message;
use objc2::runtime::ProtocolObject;
use objc2_metal::*;
use std::any::TypeId;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::rc::Rc;

/// rc-engine `display_blend::DisplayEffect`: an entity drawn in the effect pass.
#[derive(Clone, Copy, Debug, Default)]
pub struct DisplayEffect;

/// rc-engine `pre_shadow::BeforeShadows`.
#[derive(Clone, Copy, Debug, Default)]
pub struct BeforeShadows;

/// rc-engine `pre_shadow::WorldDrawOrder`: the game's draw order of the world's colour-only draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorldDrawOrder(pub [u32; 5]);

impl WorldDrawOrder {
    pub const TFRAG: u32 = 0;
    pub const TIE: u32 = 1;
    pub const SHRUB: u32 = 2;
    pub const BILLBOARD: u32 = 3;
    pub const NONE: Self = Self([u32::MAX; 5]);
}

/// Bevy's `Camera3d` depth load op (`Camera3dDepthLoadOp`): clear to a value (Bevy's default: 0, the far end of
/// reverse Z) or keep.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Camera3dDepth {
    Clear(f32),
    Load,
}

impl Default for Camera3dDepth {
    fn default() -> Self { Camera3dDepth::Clear(0.0) }
}

/// Bevy's `CameraOutputMode::Skip` (the view is rendered but not copied to its target: here, the target is the view).
#[derive(Clone, Copy, Debug, Default)]
pub struct OutputSkip;

/// Bevy's `IsDefaultUiCamera`.
#[derive(Clone, Copy, Debug, Default)]
pub struct IsDefaultUiCamera;

/// The view uniform (`shaders/shim/mesh_view_bindings.wgsl`).
fn view_uniform(clip_from_view: Mat4, world_from_view: Mat4, viewport: Vec4) -> Vec<u8> {
    let view_from_world = world_from_view.inverse();
    let mut s = Std140::default();
    s.mat4(clip_from_view * view_from_world).mat4(clip_from_view).mat4(view_from_world).mat4(world_from_view);
    s.vec3(world_from_view.w_axis.truncate()).vec4(viewport);
    s.finish()
}

/// One draw in a 3D view.
struct Item {
    pipeline: Rc<CachedPipeline>,
    mesh: AssetId,
    instance: u32,
    bindings: Vec<(u32, Binding)>,
    phase: Phase,
    /// Transparent3d sort key (view z of the mesh AABB centre + depth bias).
    distance: f32,
    entity: Entity,
    material: AssetId,
    effect: bool,
    before_shadows: Option<WorldDrawOrder>,
}

/// What a material type contributes to a 3D view.
trait MaterialKind3d {
    fn sync(&mut self, ctx: &Ctx);
    fn collect(&mut self, ctx: &Ctx, gfx: &mut Gfx, view: &ViewInfo, serial: &mut u64, out: &mut Vec<Item>, instances: &mut Vec<u8>);
}

struct Kind3d<M: Material> {
    cache: HashMap<(AssetId, u64, TextureFormat, u32), Rc<CachedPipeline>>,
    _m: PhantomData<M>,
}

/// The view a material collects for.
struct ViewInfo {
    layers: RenderLayers,
    format: TextureFormat,
    samples: u32,
    view_from_world: Mat4,
}

fn instance_record(m: Mat4, tag: u32, out: &mut Vec<u8>) {
    for x in m.to_cols_array() { out.extend_from_slice(&x.to_le_bytes()); }
    out.extend_from_slice(&tag.to_le_bytes());
    out.extend_from_slice(&[0u8; 12]);
}

/// The centre of a mesh's position AABB (Bevy's `RenderMesh::aabb_center`).
fn aabb_center(m: &Mesh) -> Vec3 {
    match m.attribute(Mesh::ATTRIBUTE_POSITION) {
        Some(crate::engine::mesh::VertexAttributeValues::Float32x3(p)) if !p.is_empty() => {
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for v in p {
                let v = Vec3::from_array(*v);
                lo = lo.min(v);
                hi = hi.max(v);
            }
            (lo + hi) * 0.5
        }
        _ => Vec3::ZERO,
    }
}

impl<M: Material> MaterialKind3d for Kind3d<M> {
    fn sync(&mut self, ctx: &Ctx) {
        let Some(mut mats) = ctx.try_res_mut::<Assets<M>>() else { return };
        let (changed, removed) = mats.take_changed();
        if changed.is_empty() && removed.is_empty() { return; }
        self.cache.retain(|k, _| !changed.contains(&k.0) && !removed.contains(&k.0));
    }

    fn collect(&mut self, ctx: &Ctx, gfx: &mut Gfx, view: &ViewInfo, serial: &mut u64, out: &mut Vec<Item>, instances: &mut Vec<u8>) {
        let Some(mats) = ctx.try_res::<Assets<M>>() else { return };
        let meshes = ctx.res::<Assets<Mesh>>();
        let mut q = ctx.world.query::<(
            Entity,
            &Mesh3d,
            &MeshMaterial3d<M>,
            &GlobalTransform,
            Option<&InheritedVisibility>,
            Option<&RenderLayers>,
            Option<&MeshTag>,
            Option<&DisplayEffect>,
            Option<&BeforeShadows>,
            Option<&WorldDrawOrder>,
        )>();
        for (e, mesh, mat, gt, vis, layers, tag, effect, before, order) in q.iter() {
            if vis.is_some_and(|v| !v.0) { continue; }
            if !layers.cloned().unwrap_or_default().intersects(&view.layers) { continue; }
            let Some(m) = mats.get(&mat.0) else { continue };
            let Some(gm) = gfx.meshes.get(&mesh.0.id()) else { continue };
            let layout_hash = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                gm.layout.hash(&mut h);
                gm.topology.hash(&mut h);
                m.pipeline_key().hash(&mut h);
                h.finish()
            };
            let key = (mat.0.id(), layout_hash, view.format, view.samples);
            let pipeline = match self.cache.get(&key) {
                Some(p) => p.clone(),
                None => {
                    let layout = gm.layout.clone();
                    let topology = gm.topology;
                    let mut desc = PipelineDesc::mesh_defaults(&layout, m.alpha_mode(), view.format);
                    if let Err(err) = m.specialize(&mut desc, &layout) {
                        eprintln!("metal: {} pipeline: {err}", std::any::type_name::<M>());
                        continue;
                    }
                    let attrs = match layout.get_layout(&desc.vertex_attributes) {
                        Ok(a) => a,
                        Err(err) => {
                            eprintln!("metal: {}: {err}", std::any::type_name::<M>());
                            continue;
                        }
                    };
                    let pk = PipelineKey {
                        shader: M::shader(),
                        vertex_entry: desc.vertex_entry,
                        vertex_shader: None,
                        fragment_entry: Some(desc.fragment_entry),
                        defs: desc.defs.clone(),
                        vertex_attributes: attrs,
                        vertex_stride: layout.stride,
                        color_formats: vec![desc.color_format],
                        blend: desc.blend,
                        write_mask: desc.write_mask,
                        depth: true,
                        samples: view.samples,
                        topology_class: TopologyClass::from(topology),
                    };
                    let entry = match gfx.pipeline(&pk) {
                        Ok(p) => p,
                        Err(err) => {
                            eprintln!("metal: {err:#}");
                            continue;
                        }
                    };
                    let depth = match gfx.depth_state(desc.depth_compare, desc.depth_write) {
                        Ok(d) => d,
                        Err(_) => continue,
                    };
                    *serial += 1;
                    let p = Rc::new(CachedPipeline { entry, depth, cull: desc.cull, format: desc.color_format, serial: *serial });
                    self.cache.insert(key, p.clone());
                    p
                }
            };
            let mut bg = BindGroup::new(gfx);
            m.bindings(&mut bg);
            let Some(bindings) = bg.into_bindings() else { continue };
            let phase = m.alpha_mode().phase();
            let distance = if phase == Phase::Transparent {
                let centre = meshes.get(&mesh.0).map(aabb_center).unwrap_or(Vec3::ZERO);
                let world = gt.0.transform_point3(centre);
                view.view_from_world.row(2).dot(world.extend(1.0)) + m.depth_bias()
            } else {
                0.0
            };
            let instance = (instances.len() / 80) as u32;
            instance_record(gt.to_matrix(), tag.map_or(0, |t| t.0), instances);
            out.push(Item {
                pipeline,
                mesh: mesh.0.id(),
                instance,
                bindings,
                phase,
                distance,
                entity: e,
                material: mat.0.id(),
                effect: effect.is_some(),
                before_shadows: before.map(|_| order.copied().unwrap_or(WorldDrawOrder::NONE)),
            });
        }
    }
}

/// Bevy's `Material2d` (rc-engine's HUD primitives).
pub trait Material2d: 'static {
    fn shader() -> &'static str;
    fn alpha_mode(&self) -> AlphaMode { AlphaMode::Blend }
    fn specialize(&self, _desc: &mut PipelineDesc, _layout: &crate::engine::mesh::MeshLayout) -> Result<(), String> { Ok(()) }
    fn bindings(&self, b: &mut BindGroup);
}

struct Item2d {
    pipeline: Rc<CachedPipeline>,
    mesh: AssetId,
    instance: u32,
    bindings: Vec<(u32, Binding)>,
    z: f32,
    entity: Entity,
}

trait MaterialKind2d {
    fn collect(&mut self, ctx: &Ctx, gfx: &mut Gfx, view: &ViewInfo, serial: &mut u64, out: &mut Vec<Item2d>, instances: &mut Vec<u8>);
}

struct Kind2d<M: Material2d> {
    cache: HashMap<(AssetId, u64, TextureFormat), Rc<CachedPipeline>>,
    _m: PhantomData<M>,
}

impl<M: Material2d> MaterialKind2d for Kind2d<M> {
    fn collect(&mut self, ctx: &Ctx, gfx: &mut Gfx, view: &ViewInfo, serial: &mut u64, out: &mut Vec<Item2d>, instances: &mut Vec<u8>) {
        let Some(mut mats) = ctx.try_res_mut::<Assets<M>>() else { return };
        let (changed, _) = mats.take_changed();
        if !changed.is_empty() { self.cache.retain(|k, _| !changed.contains(&k.0)); }
        let mut q = ctx.world.query::<(Entity, &Mesh2d, &MeshMaterial2d<M>, &GlobalTransform, Option<&InheritedVisibility>, Option<&RenderLayers>)>();
        for (e, mesh, mat, gt, vis, layers) in q.iter() {
            if vis.is_some_and(|v| !v.0) { continue; }
            if !layers.cloned().unwrap_or_default().intersects(&view.layers) { continue; }
            let Some(m) = mats.get(&mat.0) else { continue };
            let Some(gm) = gfx.meshes.get(&mesh.0.id()) else { continue };
            let layout_hash = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                gm.layout.hash(&mut h);
                h.finish()
            };
            let key = (mat.0.id(), layout_hash, view.format);
            let pipeline = match self.cache.get(&key) {
                Some(p) => p.clone(),
                None => {
                    let layout = gm.layout.clone();
                    let topology = gm.topology;
                    // Bevy's Mesh2dPipeline: no depth test or write here (rc-engine's 2D cameras have no depth target),
                    // alpha blending for Blend, no culling.
                    let mut desc = PipelineDesc::mesh_defaults(&layout, m.alpha_mode(), view.format);
                    desc.cull = None;
                    if let Err(err) = m.specialize(&mut desc, &layout) {
                        eprintln!("metal: {} pipeline: {err}", std::any::type_name::<M>());
                        continue;
                    }
                    let Ok(attrs) = layout.get_layout(&desc.vertex_attributes) else { continue };
                    let pk = PipelineKey {
                        shader: M::shader(),
                        vertex_entry: desc.vertex_entry,
                        vertex_shader: None,
                        fragment_entry: Some(desc.fragment_entry),
                        defs: desc.defs.clone(),
                        vertex_attributes: attrs,
                        vertex_stride: layout.stride,
                        color_formats: vec![desc.color_format],
                        blend: desc.blend,
                        write_mask: desc.write_mask,
                        depth: false,
                        samples: 1,
                        topology_class: TopologyClass::from(topology),
                    };
                    let entry = match gfx.pipeline(&pk) {
                        Ok(p) => p,
                        Err(err) => {
                            eprintln!("metal: {err:#}");
                            continue;
                        }
                    };
                    let Ok(depth) = gfx.depth_state(gfx::CompareFunction::Always, false) else { continue };
                    *serial += 1;
                    let p = Rc::new(CachedPipeline { entry, depth, cull: None, format: desc.color_format, serial: *serial });
                    self.cache.insert(key, p.clone());
                    p
                }
            };
            let mut bg = BindGroup::new(gfx);
            m.bindings(&mut bg);
            let Some(bindings) = bg.into_bindings() else { continue };
            let instance = (instances.len() / 80) as u32;
            instance_record(gt.to_matrix(), 0, instances);
            out.push(Item2d { pipeline, mesh: mesh.0.id(), instance, bindings, z: gt.translation().z, entity: e });
        }
    }
}

/// Bevy's `Sprite` (rc-engine draws the game frame into the window with one).
#[derive(Clone, Debug)]
pub struct Sprite {
    pub image: Handle<Image>,
    pub custom_size: Option<Vec2>,
    pub color: Color,
}

impl Sprite {
    pub fn from_image(image: Handle<Image>) -> Self { Sprite { image, custom_size: None, color: Color::WHITE } }
}

/// Where in a view the hooks run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Hook {
    /// After the opaque, alpha-mask and pre-shadow draws, before the transparent pass (rc-engine `ShadowPassSet`).
    AfterOpaque,
    /// After every camera (Bevy's `RenderGraphSystems::Finish`; rc-engine's menu snapshot copy). `color` / `depth` are
    /// unused there: the hook reads the images it needs from `gfx`.
    Finish,
}

/// What a hook gets: the view's targets, a fresh render pass is begun by the hook itself.
pub struct HookCtx<'a> {
    pub gfx: &'a mut Gfx,
    pub ctx: &'a Ctx,
    pub cb: &'a CommandBuffer,
    pub color: &'a Texture,
    pub depth: &'a Texture,
    pub view: &'a [u8],
    pub viewport: (f64, f64, f64, f64),
    pub camera: Entity,
    pub samples: u32,
    pub format: TextureFormat,
}

pub type HookFn = fn(&mut HookCtx) -> Result<()>;

/// A full-screen pass on a camera (rc-engine `gs_post` / Bevy's `FullscreenMaterial`): present while the camera carries
/// the component the pass checks; the pass reads the frame (`@group(0) @binding(0)` texture, `binding(1)` sampler,
/// `binding(2)` the uniform the component gives) and writes it back.
pub struct FullscreenPass {
    pub shader: &'static str,
    pub entry: &'static str,
    pub defs: crate::shader::Defs,
    /// The uniform bytes when the camera carries the pass's component (None: the pass is off for the camera).
    pub uniform: fn(&World, Entity) -> Option<Vec<u8>>,
    pub blend: Option<gfx::Blend>,
    /// Drawn after the UI pass (rc-engine: the missile view, the underwater tint, the scene fade); else before it.
    pub after_ui: bool,
}

/// The registered full-screen passes, in drawing order.
#[derive(Default)]
pub struct FullscreenPasses(pub Vec<FullscreenPass>);

/// rc-engine's UI (crate::render::ui) draws through this.
pub type UiDrawFn = fn(&mut UiCtx) -> Result<()>;

pub struct UiCtx<'a> {
    pub gfx: &'a mut Gfx,
    pub ctx: &'a Ctx,
    pub enc: &'a Encoder,
    pub camera: Entity,
    pub default_camera: bool,
    pub target_size: UVec2,
    pub format: TextureFormat,
    pub quad: &'a mut QuadDrawer,
}

/// Draws textured quads with `sprite.wgsl` (sprites, UI images and flat colours).
pub struct QuadDrawer {
    white: Option<Handle<Image>>,
}

impl QuadDrawer {
    /// Draws `image` (None: white) over clip rectangle `rect` with `uv` and linear `color`, blended.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, gfx: &mut Gfx, enc: &Encoder, format: TextureFormat, image: Option<AssetId>, rect: Vec4, uv: Vec4, color: [f32; 4], blend: Option<gfx::Blend>) -> Result<()> {
        let key = PipelineKey {
            shader: "sprite",
            vertex_entry: "vertex",
            vertex_shader: None,
            fragment_entry: Some("fragment"),
            defs: Default::default(),
            vertex_attributes: vec![],
            vertex_stride: 0,
            color_formats: vec![format],
            blend,
            write_mask: MTLColorWriteMask::All,
            depth: false,
            samples: 1,
            topology_class: TopologyClass::Triangle,
        };
        let p = gfx.pipeline(&key)?;
        let img = match image.or_else(|| self.white.as_ref().map(|h| h.id())) {
            Some(id) => id,
            None => return Ok(()),
        };
        let Some(gi) = gfx.images.get(&img) else { return Ok(()) };
        let mut s = Std140::default();
        s.vec4(rect).vec4(uv).vec4(Vec4::from_array(color));
        let q = Binding::Bytes(Rc::new(s.finish()));
        let t = Binding::Texture(gi.texture.clone());
        let smp = Binding::Sampler(gi.sampler.clone());
        enc.setRenderPipelineState(&p.state);
        let res = [(1, 0, &q), (2, 0, &t), (2, 1, &smp)];
        bind(enc, &p.vertex, true, &res);
        if let Some(f) = &p.fragment { bind(enc, f, false, &res); }
        // SAFETY: a four-vertex strip, no buffers.
        unsafe { enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::TriangleStrip, 0, 4) };
        Ok(())
    }
}

pub struct Renderer {
    pub gfx: Gfx,
    kinds3d: Vec<(TypeId, Box<dyn MaterialKind3d>)>,
    kinds2d: Vec<(TypeId, Box<dyn MaterialKind2d>)>,
    serial: u64,
    /// Depth per (target, size, samples), shared by the cameras of one target.
    depth: HashMap<(Option<AssetId>, u32, u32, u32), Retained<Texture>>,
    msaa: HashMap<(Option<AssetId>, u32, u32, u32, TextureFormat), Retained<Texture>>,
    scratch: HashMap<(u32, u32, TextureFormat), Retained<Texture>>,
    hooks: Vec<(Hook, HookFn)>,
    pub fullscreen: FullscreenPasses,
    pub ui: Option<UiDrawFn>,
    quad: QuadDrawer,
    /// Instance and view buffers reused frame to frame.
    instance_buffers: Vec<Retained<gfx::Buffer>>,
}

impl Renderer {
    pub fn new() -> Result<Self> {
        Ok(Renderer {
            gfx: Gfx::new()?,
            kinds3d: Vec::new(),
            kinds2d: Vec::new(),
            serial: 0,
            depth: HashMap::new(),
            msaa: HashMap::new(),
            scratch: HashMap::new(),
            hooks: Vec::new(),
            fullscreen: FullscreenPasses::default(),
            ui: None,
            quad: QuadDrawer { white: None },
            instance_buffers: Vec::new(),
        })
    }

    /// Bevy's `MaterialPlugin::<M>`.
    pub fn add_material<M: Material>(&mut self, ctx: &mut Ctx) {
        ctx.res.init::<Assets<M>>();
        if self.kinds3d.iter().any(|(t, _)| *t == TypeId::of::<M>()) { return; }
        self.kinds3d.push((TypeId::of::<M>(), Box::new(Kind3d::<M> { cache: HashMap::new(), _m: PhantomData })));
    }

    /// Bevy's `Material2dPlugin::<M>`.
    pub fn add_material_2d<M: Material2d>(&mut self, ctx: &mut Ctx) {
        ctx.res.init::<Assets<M>>();
        if self.kinds2d.iter().any(|(t, _)| *t == TypeId::of::<M>()) { return; }
        self.kinds2d.push((TypeId::of::<M>(), Box::new(Kind2d::<M> { cache: HashMap::new(), _m: PhantomData })));
    }

    pub fn add_hook(&mut self, hook: Hook, f: HookFn) { self.hooks.push((hook, f)); }

    /// Renders every camera. `window`: the drawable's texture (cameras targeting the window), if presenting.
    pub fn render(&mut self, ctx: &mut Ctx, window: Option<&Texture>, drawable: Option<&ProtocolObject<dyn MTLDrawable>>) -> Result<Retained<CommandBuffer>> {
        self.gfx.wait_idle();
        if self.quad.white.is_none() {
            let h = ctx.res_mut::<Assets<Image>>().add(Image::default());
            self.quad.white = Some(h);
        }
        {
            let mut meshes = ctx.res_mut::<Assets<Mesh>>();
            let mut images = ctx.res_mut::<Assets<Image>>();
            let mut buffers = ctx.res_mut::<Assets<gfx::ShaderBuffer>>();
            self.gfx.sync_assets(&mut meshes, &mut images, &mut buffers)?;
        }
        for (_, k) in &mut self.kinds3d { k.sync(ctx); }

        // Cameras in Bevy's order (order, then entity).
        let mut cams: Vec<(isize, Entity)> = ctx
            .world
            .query::<(Entity, &Camera)>()
            .iter()
            .filter(|(_, c)| c.is_active)
            .map(|(e, c)| (c.order, e))
            .collect();
        cams.sort_by_key(|(o, e)| (*o, e.to_bits()));

        let cb = self.gfx.queue.commandBuffer().context("Metal command buffer")?;
        let mut used_buffers = 0;
        for (_, cam) in cams {
            let r = self.render_camera(ctx, &cb, cam, window, &mut used_buffers);
            if let Err(e) = r { eprintln!("metal: camera {cam:?}: {e:#}"); }
        }
        for (h, f) in self.hooks.clone() {
            if h != Hook::Finish { continue; }
            let Some(dummy) = self.gfx.images.values().next().map(|i| i.texture.clone()) else { break };
            let mut hc = HookCtx {
                gfx: &mut self.gfx,
                ctx,
                cb: &cb,
                color: &dummy,
                depth: &dummy,
                view: &[],
                viewport: (0.0, 0.0, 0.0, 0.0),
                camera: Entity::DANGLING,
                samples: 1,
                format: TextureFormat::Rgba8Unorm,
            };
            if let Err(e) = f(&mut hc) { eprintln!("metal: finish hook: {e:#}"); }
        }
        if let Some(d) = drawable { cb.presentDrawable(d); }
        cb.commit();
        self.gfx.in_flight = Some(cb.clone());
        Ok(cb)
    }

    fn buffer_for(&mut self, idx: &mut usize, bytes: &[u8]) -> Result<Retained<gfx::Buffer>> {
        if *idx >= self.instance_buffers.len() { self.instance_buffers.push(gfx::new_buffer(&self.gfx.device, bytes, "mesh instances")?); } else {
            let mut b = self.instance_buffers[*idx].clone();
            gfx::write_buffer(&self.gfx.device, &mut b, bytes, "mesh instances")?;
            self.instance_buffers[*idx] = b;
        }
        *idx += 1;
        Ok(self.instance_buffers[*idx - 1].clone())
    }

    fn render_camera(&mut self, ctx: &Ctx, cb: &CommandBuffer, cam: Entity, window: Option<&Texture>, used: &mut usize) -> Result<()> {
        let (camera, gt, proj, layers, msaa, is3d, is2d, depth_op, default_ui) = {
            let w = &ctx.world;
            let camera = w.get::<&Camera>(cam)?.clone();
            let gt = w.get::<&GlobalTransform>(cam).map(|g| *g).unwrap_or_default();
            let proj = w.get::<&Projection>(cam).ok().map(|p| (*p).clone());
            let layers = w.get::<&RenderLayers>(cam).ok().map(|l| (*l).clone()).unwrap_or_default();
            let msaa = w.get::<&Msaa>(cam).ok().map(|m| *m).unwrap_or(Msaa::Sample4);
            let is3d = w.get::<&Camera3d>(cam).is_ok();
            let is2d = w.get::<&Camera2d>(cam).is_ok();
            let depth_op = w.get::<&Camera3dDepth>(cam).ok().map(|d| *d).unwrap_or_default();
            let default_ui = w.get::<&IsDefaultUiCamera>(cam).is_ok();
            (camera, gt, proj, layers, msaa, is3d, is2d, depth_op, default_ui)
        };
        let (target_tex, target_id, format): (Retained<Texture>, Option<AssetId>, TextureFormat) = match &camera.target {
            RenderTarget::Image(h) => {
                let Some(gi) = self.gfx.images.get(&h.id()) else { return Ok(()) };
                (gi.texture.clone(), Some(h.id()), gi.format)
            }
            RenderTarget::Window => match window {
                Some(t) => (t.retain(), None, TextureFormat::Bgra8UnormSrgb),
                None => return Ok(()),
            },
        };
        let (tw, th) = (target_tex.width() as u32, target_tex.height() as u32);
        let (vx, vy, vw, vh) = match camera.viewport {
            Some(v) => (v.physical_position.x, v.physical_position.y, v.physical_size.x.max(1), v.physical_size.y.max(1)),
            None => (0, 0, tw, th),
        };
        let size = UVec2::new(vw, vh);
        let samples = if is3d { msaa.samples() } else { 1 };
        let clear = match camera.clear_color {
            ClearColorConfig::Default => Some(ctx.try_res::<ClearColor>().map_or(Color::srgb(0.43, 0.43, 0.43), |c| c.0)),
            ClearColorConfig::Custom(c) => Some(c),
            ClearColorConfig::None => None,
        };
        let projection = proj.unwrap_or(if is2d { Projection::Orthographic } else { Projection::default() });
        let clip_from_view = projection.clip_from_view(size);
        let world_from_view = gt.to_matrix();
        let view_bytes = view_uniform(clip_from_view, world_from_view, Vec4::new(vx as f32, vy as f32, vw as f32, vh as f32));
        let view_info = ViewInfo { layers: layers.clone(), format, samples, view_from_world: world_from_view.inverse() };
        let viewport = MTLViewport { originX: vx as f64, originY: vy as f64, width: vw as f64, height: vh as f64, znear: 0.0, zfar: 1.0 };

        // MSAA colour (resolved into the target at the end of every pass).
        let msaa_tex = if samples > 1 {
            let key = (target_id, tw, th, samples, format);
            if !self.msaa.contains_key(&key) {
                let t = self.gfx.target(gfx::pixel_format(format), tw as usize, th as usize, samples, false)?;
                self.msaa.insert(key, t);
            }
            Some(self.msaa[&key].clone())
        } else {
            None
        };
        let color_pass = |rpd: &MTLRenderPassDescriptor, load: Option<Option<Color>>| {
            // SAFETY: attachment 0.
            let ca = unsafe { rpd.colorAttachments().objectAtIndexedSubscript(0) };
            match &msaa_tex {
                Some(m) => {
                    ca.setTexture(Some(m));
                    ca.setResolveTexture(Some(&target_tex));
                    ca.setStoreAction(MTLStoreAction::StoreAndMultisampleResolve);
                }
                None => {
                    ca.setTexture(Some(&target_tex));
                    ca.setStoreAction(MTLStoreAction::Store);
                }
            }
            match load {
                Some(Some(c)) => {
                    let l = c.to_linear();
                    ca.setLoadAction(MTLLoadAction::Clear);
                    ca.setClearColor(MTLClearColor { red: l[0] as f64, green: l[1] as f64, blue: l[2] as f64, alpha: l[3] as f64 });
                }
                _ => ca.setLoadAction(MTLLoadAction::Load),
            }
        };

        if is3d {
            let dkey = (target_id, tw, th, samples);
            if !self.depth.contains_key(&dkey) {
                let t = self.gfx.target(gfx::DEPTH_FORMAT, tw as usize, th as usize, samples, false)?;
                self.depth.insert(dkey, t);
            }
            let depth_tex = self.depth[&dkey].clone();

            let mut items = Vec::new();
            let mut instances = Vec::new();
            let serial = &mut self.serial;
            for (_, k) in &mut self.kinds3d { k.collect(ctx, &mut self.gfx, &view_info, serial, &mut items, &mut instances); }
            let inst_buf = self.buffer_for(used, &instances)?;

            // Phases.
            let mut opaque: Vec<&Item> = items.iter().filter(|i| i.phase == Phase::Opaque).collect();
            let mut mask: Vec<&Item> = items.iter().filter(|i| i.phase == Phase::AlphaMask).collect();
            let bin = |i: &&Item| (i.pipeline.serial, i.material, i.mesh, i.entity.to_bits());
            opaque.sort_by_key(bin);
            mask.sort_by_key(bin);
            let mut transparent: Vec<&Item> = items.iter().filter(|i| i.phase == Phase::Transparent).collect();
            transparent.sort_by(|a, b| a.distance.total_cmp(&b.distance).then_with(|| a.entity.to_bits().cmp(&b.entity.to_bits())));
            // pre_shadow: the tagged items first in the game's order (stable for the rest), then split off.
            let mut pre: Vec<&Item> = transparent.iter().copied().filter(|i| i.before_shadows.is_some()).collect();
            pre.sort_by_key(|i| i.before_shadows.unwrap());
            let rest: Vec<&Item> = transparent.iter().copied().filter(|i| i.before_shadows.is_none()).collect();
            let main_b: Vec<&Item> = rest.iter().copied().filter(|i| !i.effect).collect();
            let effects: Vec<&Item> = rest.iter().copied().filter(|i| i.effect).collect();

            // Pass A.
            let rpd = MTLRenderPassDescriptor::renderPassDescriptor();
            color_pass(&rpd, Some(clear));
            let da = rpd.depthAttachment();
            da.setTexture(Some(&depth_tex));
            match depth_op {
                Camera3dDepth::Clear(v) => {
                    da.setLoadAction(MTLLoadAction::Clear);
                    da.setClearDepth(v as f64);
                }
                Camera3dDepth::Load => da.setLoadAction(MTLLoadAction::Load),
            }
            da.setStoreAction(MTLStoreAction::Store);
            let enc = cb.renderCommandEncoderWithDescriptor(&rpd).context("render encoder")?;
            enc.setViewport(viewport);
            self.draw_items(&enc, &view_bytes, &inst_buf, opaque.iter().chain(mask.iter()).chain(pre.iter()).copied());
            enc.endEncoding();

            for (h, f) in self.hooks.clone() {
                if h != Hook::AfterOpaque { continue; }
                let color: &Texture = msaa_tex.as_deref().unwrap_or(&target_tex);
                let mut hc = HookCtx {
                    gfx: &mut self.gfx,
                    ctx,
                    cb,
                    color,
                    depth: &depth_tex,
                    view: &view_bytes,
                    viewport: (vx as f64, vy as f64, vw as f64, vh as f64),
                    camera: cam,
                    samples,
                    format,
                };
                if let Err(e) = f(&mut hc) { eprintln!("metal: hook: {e:#}"); }
            }

            // Pass B.
            let rpd = MTLRenderPassDescriptor::renderPassDescriptor();
            color_pass(&rpd, None);
            let da = rpd.depthAttachment();
            da.setTexture(Some(&depth_tex));
            da.setLoadAction(MTLLoadAction::Load);
            da.setStoreAction(MTLStoreAction::Store);
            let enc = cb.renderCommandEncoderWithDescriptor(&rpd).context("render encoder")?;
            enc.setViewport(viewport);
            self.draw_items(&enc, &view_bytes, &inst_buf, main_b.iter().copied());
            enc.endEncoding();

            if !effects.is_empty() {
                self.effect_pass(cb, &target_tex, &depth_tex, samples, format, viewport, &view_bytes, &inst_buf, &effects)?;
            }
        } else {
            // Core2d: Transparent2d (Mesh2d materials) by z then entity, then the sprites.
            let mut items = Vec::new();
            let mut instances = Vec::new();
            let serial = &mut self.serial;
            for (_, k) in &mut self.kinds2d { k.collect(ctx, &mut self.gfx, &view_info, serial, &mut items, &mut instances); }
            items.sort_by(|a, b| a.z.total_cmp(&b.z).then_with(|| a.entity.to_bits().cmp(&b.entity.to_bits())));
            let inst_buf = self.buffer_for(used, &instances)?;
            let rpd = MTLRenderPassDescriptor::renderPassDescriptor();
            color_pass(&rpd, Some(clear));
            let enc = cb.renderCommandEncoderWithDescriptor(&rpd).context("render encoder")?;
            enc.setViewport(viewport);
            let view_b = Binding::Bytes(Rc::new(view_bytes.clone()));
            let inst_b = Binding::Buffer(inst_buf.clone());
            for it in &items {
                let Some(gm) = self.gfx.meshes.get(&it.mesh) else { continue };
                enc.setRenderPipelineState(&it.pipeline.entry.state);
                let mut res: Vec<(u32, u32, &Binding)> = vec![(0, 0, &view_b), (1, 0, &inst_b)];
                res.extend(it.bindings.iter().map(|(n, b)| (2, *n, b)));
                bind(&enc, &it.pipeline.entry.vertex, true, &res);
                if let Some(f) = &it.pipeline.entry.fragment { bind(&enc, f, false, &res); }
                draw_mesh(&enc, gm, it.instance);
            }
            // Sprites (a 2D camera's world: one unit per pixel, origin at the centre).
            let sprites: Vec<(Handle<Image>, Vec2, Color, GlobalTransform, Entity)> = ctx
                .world
                .query::<(Entity, &Sprite, &GlobalTransform, Option<&InheritedVisibility>, Option<&RenderLayers>)>()
                .iter()
                .filter(|(_, _, _, v, l)| v.is_none_or(|v| v.0) && l.cloned().unwrap_or_default().intersects(&layers))
                .map(|(e, s, g, _, _)| {
                    let size = s.custom_size.unwrap_or_else(|| self.gfx.images.get(&s.image.id()).map_or(Vec2::ONE, |i| i.size.as_vec2()));
                    (s.image.clone(), size, s.color, *g, e)
                })
                .collect();
            for (img, sz, color, g, _) in sprites {
                let c = g.translation();
                let clip = clip_from_view * world_from_view.inverse();
                let p0 = clip * Vec4::new(c.x - sz.x * 0.5, c.y + sz.y * 0.5, 0.0, 1.0);
                let p1 = clip * Vec4::new(c.x + sz.x * 0.5, c.y - sz.y * 0.5, 0.0, 1.0);
                let lin = color.to_linear();
                self.quad.draw(&mut self.gfx, &enc, format, Some(img.id()), Vec4::new(p0.x, p0.y, p1.x, p1.y), Vec4::new(0.0, 0.0, 1.0, 1.0), lin, Some(gfx::Blend::ALPHA_BLENDING))?;
            }
            enc.endEncoding();
        }

        // Full-screen passes before the UI (rc-engine gs_post order), the UI, the passes after it.
        if is3d {
            for i in 0..self.fullscreen.0.len() {
                if self.fullscreen.0[i].after_ui { continue; }
                let Some(uniform) = (self.fullscreen.0[i].uniform)(&ctx.world, cam) else { continue };
                self.fullscreen_pass(cb, i, &target_tex, format, uniform, viewport)?;
            }
        }
        self.ui_pass(ctx, cb, cam, default_ui, &target_tex, format)?;
        if is3d {
            for i in 0..self.fullscreen.0.len() {
                if !self.fullscreen.0[i].after_ui { continue; }
                let Some(uniform) = (self.fullscreen.0[i].uniform)(&ctx.world, cam) else { continue };
                self.fullscreen_pass(cb, i, &target_tex, format, uniform, viewport)?;
            }
        }
        Ok(())
    }

    fn ui_pass(&mut self, ctx: &Ctx, cb: &CommandBuffer, cam: Entity, default_ui: bool, target_tex: &Texture, format: TextureFormat) -> Result<()> {
        let (tw, th) = (target_tex.width() as u32, target_tex.height() as u32);
        if let Some(ui) = self.ui {
            let rpd = MTLRenderPassDescriptor::renderPassDescriptor();
            // SAFETY: attachment 0.
            let ca = unsafe { rpd.colorAttachments().objectAtIndexedSubscript(0) };
            ca.setTexture(Some(target_tex));
            ca.setLoadAction(MTLLoadAction::Load);
            ca.setStoreAction(MTLStoreAction::Store);
            let enc = cb.renderCommandEncoderWithDescriptor(&rpd).context("render encoder")?;
            let mut uc = UiCtx { gfx: &mut self.gfx, ctx, enc: &enc, camera: cam, default_camera: default_ui, target_size: UVec2::new(tw, th), format, quad: &mut self.quad };
            let r = ui(&mut uc);
            enc.endEncoding();
            r?;
        }
        Ok(())
    }

    fn draw_items<'a>(&self, enc: &Encoder, view: &[u8], inst: &Retained<gfx::Buffer>, items: impl Iterator<Item = &'a Item>) {
        let view_b = Binding::Bytes(Rc::new(view.to_vec()));
        let inst_b = Binding::Buffer(inst.clone());
        let mut last_pipeline: *const CachedPipeline = std::ptr::null();
        for it in items {
            let Some(gm) = self.gfx.meshes.get(&it.mesh) else { continue };
            let p = &*it.pipeline;
            if !std::ptr::eq(last_pipeline, p) {
                enc.setRenderPipelineState(&p.entry.state);
                enc.setDepthStencilState(Some(&p.depth));
                enc.setFrontFacingWinding(MTLWinding::CounterClockwise);
                enc.setCullMode(match p.cull {
                    None => MTLCullMode::None,
                    Some(Face::Back) => MTLCullMode::Back,
                    Some(Face::Front) => MTLCullMode::Front,
                });
                last_pipeline = p;
            }
            let mut res: Vec<(u32, u32, &Binding)> = vec![(0, 0, &view_b), (1, 0, &inst_b)];
            res.extend(it.bindings.iter().map(|(n, b)| (2, *n, b)));
            bind(enc, &p.entry.vertex, true, &res);
            if let Some(f) = &p.entry.fragment { bind(enc, f, false, &res); }
            draw_mesh(enc, gm, it.instance);
        }
    }

    fn scratch(&mut self, w: u32, h: u32, format: TextureFormat) -> Result<Retained<Texture>> {
        let key = (w, h, format);
        if !self.scratch.contains_key(&key) {
            let t = self.gfx.target(gfx::pixel_format(format), w as usize, h as usize, 1, true)?;
            self.scratch.insert(key, t);
        }
        Ok(self.scratch[&key].clone())
    }

    /// Copies `src` into `dst` (same size and format).
    fn copy(cb: &CommandBuffer, src: &Texture, dst: &Texture) -> Result<()> {
        let blit = cb.blitCommandEncoder().context("blit encoder")?;
        // SAFETY: same-size textures.
        unsafe { blit.copyFromTexture_toTexture(src, dst) };
        blit.endEncoding();
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn effect_pass(
        &mut self,
        cb: &CommandBuffer,
        target: &Texture,
        depth: &Texture,
        samples: u32,
        format: TextureFormat,
        viewport: MTLViewport,
        view: &[u8],
        inst: &Retained<gfx::Buffer>,
        effects: &[&Item],
    ) -> Result<()> {
        let (w, h) = (target.width() as u32, target.height() as u32);
        let eff = self.scratch(w, h, TextureFormat::Rgba8Unorm)?;
        let eff_ms = if samples > 1 { Some(self.gfx.target(MTLPixelFormat::RGBA8Unorm, w as usize, h as usize, samples, false)?) } else { None };
        // 1. The frame as display bytes.
        self.blit_pass(cb, "effect_blit", "to_display", target, &eff, eff_ms.as_deref(), TextureFormat::Rgba8Unorm, samples)?;
        // 2. The effects on those bytes, against the scene depth.
        let rpd = MTLRenderPassDescriptor::renderPassDescriptor();
        // SAFETY: attachment 0.
        let ca = unsafe { rpd.colorAttachments().objectAtIndexedSubscript(0) };
        match &eff_ms {
            Some(m) => {
                ca.setTexture(Some(m));
                ca.setResolveTexture(Some(&eff));
                ca.setStoreAction(MTLStoreAction::StoreAndMultisampleResolve);
            }
            None => {
                ca.setTexture(Some(&eff));
                ca.setStoreAction(MTLStoreAction::Store);
            }
        }
        ca.setLoadAction(MTLLoadAction::Load);
        let da = rpd.depthAttachment();
        da.setTexture(Some(depth));
        da.setLoadAction(MTLLoadAction::Load);
        da.setStoreAction(MTLStoreAction::Store);
        let enc = cb.renderCommandEncoderWithDescriptor(&rpd).context("render encoder")?;
        enc.setViewport(viewport);
        self.draw_items(&enc, view, inst, effects.iter().copied());
        enc.endEncoding();
        // 3. Back into the frame.
        let msaa_target = if samples > 1 { self.msaa.iter().find(|(k, _)| k.1 == w && k.2 == h && k.3 == samples && k.4 == format).map(|(_, t)| t.clone()) } else { None };
        self.blit_pass(cb, "effect_blit", "to_linear", &eff, target, msaa_target.as_deref(), format, samples)?;
        Ok(())
    }

    /// A full-screen draw of `shader:entry` reading `src` (binding 0) into `dst` (through `ms` when multisampled).
    #[allow(clippy::too_many_arguments)]
    fn blit_pass(&mut self, cb: &CommandBuffer, shader: &'static str, entry: &'static str, src: &Texture, dst: &Texture, ms: Option<&Texture>, format: TextureFormat, samples: u32) -> Result<()> {
        let key = PipelineKey {
            shader,
            vertex_entry: "fullscreen_vertex_shader",
            vertex_shader: Some("fullscreen_vertex_shader"),
            fragment_entry: Some(entry),
            defs: if samples > 1 { crate::shader::defs([("MULTISAMPLED", None)]) } else { Default::default() },
            vertex_attributes: vec![],
            vertex_stride: 0,
            color_formats: vec![format],
            blend: None,
            write_mask: MTLColorWriteMask::All,
            depth: false,
            samples,
            topology_class: TopologyClass::Triangle,
        };
        let p = self.gfx.pipeline(&key)?;
        let rpd = MTLRenderPassDescriptor::renderPassDescriptor();
        // SAFETY: attachment 0.
        let ca = unsafe { rpd.colorAttachments().objectAtIndexedSubscript(0) };
        match ms {
            Some(m) => {
                ca.setTexture(Some(m));
                ca.setResolveTexture(Some(dst));
                ca.setStoreAction(MTLStoreAction::StoreAndMultisampleResolve);
            }
            None => {
                ca.setTexture(Some(dst));
                ca.setStoreAction(MTLStoreAction::Store);
            }
        }
        ca.setLoadAction(MTLLoadAction::Load);
        let enc = cb.renderCommandEncoderWithDescriptor(&rpd).context("render encoder")?;
        enc.setRenderPipelineState(&p.state);
        let t = Binding::Texture(src.retain());
        let res = [(0, 0, &t)];
        if let Some(f) = &p.fragment { bind(&enc, f, false, &res); }
        // SAFETY: the fullscreen triangle.
        unsafe { enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3) };
        enc.endEncoding();
        Ok(())
    }

    fn fullscreen_pass(&mut self, cb: &CommandBuffer, i: usize, target: &Texture, format: TextureFormat, uniform: Vec<u8>, viewport: MTLViewport) -> Result<()> {
        let (w, h) = (target.width() as u32, target.height() as u32);
        let src = self.scratch(w, h, format)?;
        Self::copy(cb, target, &src)?;
        let pass = &self.fullscreen.0[i];
        let key = PipelineKey {
            shader: pass.shader,
            vertex_entry: "fullscreen_vertex_shader",
            vertex_shader: Some("fullscreen_vertex_shader"),
            fragment_entry: Some(pass.entry),
            defs: pass.defs.clone(),
            vertex_attributes: vec![],
            vertex_stride: 0,
            color_formats: vec![format],
            blend: pass.blend,
            write_mask: MTLColorWriteMask::All,
            depth: false,
            samples: 1,
            topology_class: TopologyClass::Triangle,
        };
        let p = self.gfx.pipeline(&key)?;
        // rc-engine gs_post's sampler: wgpu's default descriptor (nearest, clamp).
        let sampler = self.gfx.sampler(crate::engine::image::ImageSamplerDescriptor::default())?;
        let rpd = MTLRenderPassDescriptor::renderPassDescriptor();
        // SAFETY: attachment 0.
        let ca = unsafe { rpd.colorAttachments().objectAtIndexedSubscript(0) };
        ca.setTexture(Some(target));
        ca.setLoadAction(MTLLoadAction::Load);
        ca.setStoreAction(MTLStoreAction::Store);
        let enc = cb.renderCommandEncoderWithDescriptor(&rpd).context("render encoder")?;
        enc.setViewport(viewport);
        enc.setRenderPipelineState(&p.state);
        let t = Binding::Texture(src.clone());
        let s = Binding::Sampler(sampler);
        let u = Binding::Bytes(Rc::new(uniform));
        let res = [(0, 0, &t), (0, 1, &s), (0, 2, &u)];
        if let Some(f) = &p.fragment { bind(&enc, f, false, &res); }
        // SAFETY: the fullscreen triangle.
        unsafe { enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3) };
        enc.endEncoding();
        Ok(())
    }

    /// The texels of image `h` (a render target) as RGBA8 rows: for screenshots.
    pub fn read_image(&mut self, h: AssetId) -> Result<(u32, u32, Vec<u8>)> {
        self.gfx.wait_idle();
        let gi = self.gfx.images.get(&h).context("image not on the GPU")?;
        let (w, hh) = (gi.size.x as usize, gi.size.y as usize);
        let bgra = matches!(gi.format, TextureFormat::Bgra8Unorm | TextureFormat::Bgra8UnormSrgb);
        let buf = self.gfx.device.newBufferWithLength_options(w * hh * 4, MTLResourceOptions::StorageModeShared).context("read-back buffer")?;
        let cb = self.gfx.queue.commandBuffer().context("command buffer")?;
        let blit = cb.blitCommandEncoder().context("blit")?;
        // SAFETY: level 0 into a buffer of exactly its size.
        unsafe {
            blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                &gi.texture, 0, 0, MTLOrigin { x: 0, y: 0, z: 0 }, MTLSize { width: w, height: hh, depth: 1 }, &buf, 0, w * 4, w * hh * 4,
            );
        }
        blit.endEncoding();
        cb.commit();
        cb.waitUntilCompleted();
        // SAFETY: the copy has completed.
        let px = unsafe { std::slice::from_raw_parts(buf.contents().as_ptr() as *const u8, w * hh * 4) };
        let rgba = if bgra { px.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect() } else { px.to_vec() };
        Ok((w as u32, hh as u32, rgba))
    }
}

fn draw_mesh(enc: &Encoder, gm: &gfx::GpuMesh, instance: u32) {
    let prim = gfx::primitive(gm.topology);
    // SAFETY: the mesh's own buffers; one instance at `instance` (the shader reads its record through instance_index).
    unsafe {
        enc.setVertexBuffer_offset_atIndex(Some(&gm.vertices), 0, crate::shader::VERTEX_BUFFER_BASE as usize);
        match &gm.indices {
            Some((buf, ty, n)) => enc.drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferOffset_instanceCount_baseVertex_baseInstance(
                prim, *n, *ty, buf, 0, 1, 0, instance as usize,
            ),
            None => enc.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(prim, 0, gm.vertex_count, 1, instance as usize),
        }
    }
}
