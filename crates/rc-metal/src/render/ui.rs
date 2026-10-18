//! The UI rc-engine composites with (Bevy UI's subset it uses): absolutely positioned nodes sized in pixels or percent
//! of their parent, flat colours (`BackgroundColor`), images (`ImageNode`, optionally a texel sub-rectangle) and UI
//! materials (`MaterialNode<M>`), stacked by `GlobalZIndex` / `ZIndex` as Bevy stacks them, drawn on their target
//! camera (`UiTargetCamera`, else the camera marked `IsDefaultUiCamera`) after its 3D passes.
//!
//! Stacking (Bevy's `ui_stack_system`): the roots (nodes without a parent, and nodes with a `GlobalZIndex`, which leave
//! their parent's context) sorted by `GlobalZIndex`, then `ZIndex`, then entity; each node is followed by its children
//! sorted by `ZIndex` then entity. Per node: the background colour, then the image or material.

use super::frame::{IsDefaultUiCamera, UiCtx};
use super::gfx::{Binding, Blend, PipelineKey, TopologyClass};
use super::material::{BindGroup, Std140};
use crate::engine::assets::{Assets, Handle};
use crate::engine::ecs::{Entity, World};
use crate::engine::image::Image;
use crate::engine::scene::{ChildOf, Color, InheritedVisibility};
use anyhow::Result;
use glam::{Vec2, Vec4};
use objc2_metal::{MTLColorWriteMask, MTLPrimitiveType, MTLRenderCommandEncoder};
use std::any::TypeId;
use std::collections::HashMap;
use std::rc::Rc;

/// Bevy's `Val` (the units rc-engine uses).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Val {
    #[default]
    Auto,
    Px(f32),
    Percent(f32),
}

impl Val {
    fn resolve(self, parent: f32, auto: f32) -> f32 {
        match self {
            Val::Auto => auto,
            Val::Px(v) => v,
            Val::Percent(p) => parent * p / 100.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum PositionType {
    #[default]
    Relative,
    Absolute,
}

/// Bevy's `Node` (the layout fields rc-engine sets; every node is placed by left / top / width / height in its parent).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Node {
    pub position_type: PositionType,
    pub left: Val,
    pub top: Val,
    pub width: Val,
    pub height: Val,
}

/// Bevy's `BackgroundColor`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BackgroundColor(pub Color);

/// A texel rectangle (Bevy's `Rect`).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect {
    pub min: Vec2,
    pub max: Vec2,
}

impl Rect {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self { Rect { min: Vec2::new(x0, y0), max: Vec2::new(x1, y1) } }
    pub fn width(&self) -> f32 { self.max.x - self.min.x }
    pub fn height(&self) -> f32 { self.max.y - self.min.y }
}

/// Bevy's `ImageNode`.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageNode {
    pub image: Handle<Image>,
    pub color: Color,
    pub rect: Option<Rect>,
}

impl ImageNode {
    pub fn new(image: Handle<Image>) -> Self { ImageNode { image, color: Color::WHITE, rect: None } }
}

/// Bevy's `GlobalZIndex` / `ZIndex`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct GlobalZIndex(pub i32);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct ZIndex(pub i32);

/// Bevy's `UiTargetCamera`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiTargetCamera(pub Entity);

/// Bevy's `UiMaterial`: a material for a node's rectangle (group 1; the vertex shader is the stand-in `ui_vertex`).
pub trait UiMaterial: 'static {
    fn shader() -> &'static str;
    fn fragment_entry(&self) -> &'static str { "fragment" }
    fn defs(&self) -> crate::shader::Defs { Default::default() }
    /// Bevy's UI material pipeline blends with `ALPHA_BLENDING` unless the material says otherwise.
    fn blend(&self) -> Option<Blend> { Some(Blend::ALPHA_BLENDING) }
    fn bindings(&self, b: &mut BindGroup);
}

/// Bevy's `MaterialNode<M>`.
pub struct MaterialNode<M>(pub Handle<M>);

impl<M> Clone for MaterialNode<M> {
    fn clone(&self) -> Self { MaterialNode(self.0.clone()) }
}

type DrawMaterial = fn(&mut UiCtx, Entity, Vec4) -> Result<()>;

/// The UI material types registered with the renderer.
#[derive(Default)]
pub struct UiMaterials(pub Vec<(TypeId, DrawMaterial)>);

fn draw_material<M: UiMaterial>(uc: &mut UiCtx, e: Entity, rect: Vec4) -> Result<()> {
    let Ok(node) = uc.ctx.world.get::<&MaterialNode<M>>(e) else { return Ok(()) };
    let mats = uc.ctx.res::<Assets<M>>();
    let Some(m) = mats.get(&node.0) else { return Ok(()) };
    let key = PipelineKey {
        shader: M::shader(),
        vertex_entry: "ui_vertex",
        vertex_shader: None,
        fragment_entry: Some(m.fragment_entry()),
        defs: m.defs(),
        vertex_attributes: vec![],
        vertex_stride: 0,
        color_formats: vec![uc.format],
        blend: m.blend(),
        write_mask: MTLColorWriteMask::All,
        depth: false,
        samples: 1,
        topology_class: TopologyClass::Triangle,
    };
    let p = uc.gfx.pipeline(&key)?;
    let mut bg = BindGroup::new(uc.gfx);
    m.bindings(&mut bg);
    let Some(bindings) = bg.into_bindings() else { return Ok(()) };
    let mut s = Std140::default();
    s.vec4(rect).vec4(Vec4::new(uc.target_size.x as f32, uc.target_size.y as f32, 0.0, 0.0));
    let quad = Binding::Bytes(Rc::new(s.finish()));
    let mut res: Vec<(u32, u32, &Binding)> = vec![(3, 0, &quad)];
    res.extend(bindings.iter().map(|(n, b)| (1, *n, b)));
    uc.enc.setRenderPipelineState(&p.state);
    crate::render::gfx::bind_assets(uc.enc, &p.vertex, true, &res, Some(uc.gfx));
    if let Some(f) = &p.fragment { crate::render::gfx::bind_assets(uc.enc, f, false, &res, Some(uc.gfx)); }
    // SAFETY: four vertices of the stand-in's quad.
    unsafe { uc.enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::TriangleStrip, 0, 4) };
    Ok(())
}

impl UiMaterials {
    pub fn add<M: UiMaterial>(&mut self, ctx: &mut crate::engine::ecs::Ctx) {
        ctx.res.init::<Assets<M>>();
        if !self.0.iter().any(|(t, _)| *t == TypeId::of::<M>()) { self.0.push((TypeId::of::<M>(), draw_material::<M>)); }
    }
}

/// Bevy's `UiMaterialPlugin::<M>`.
pub fn add_ui_material<M: UiMaterial>(ctx: &mut crate::engine::ecs::Ctx) {
    let mut m = ctx.res.remove::<UiMaterials>().unwrap_or_default();
    m.add::<M>(ctx);
    ctx.res.insert(m);
}

/// The nodes of `camera` in stacking order with their pixel rectangles (x, y, w, h).
fn stack(world: &World, camera: Entity, default_camera: bool, target: Vec2) -> Vec<(Entity, Vec4)> {
    let mut children: HashMap<Entity, Vec<Entity>> = HashMap::new();
    let mut nodes: Vec<Entity> = Vec::new();
    for (e, _, parent) in world.query::<(Entity, &Node, Option<&ChildOf>)>().iter() {
        nodes.push(e);
        if let Some(p) = parent { children.entry(p.0).or_default().push(e); }
    }
    let z = |e: Entity| world.get::<&ZIndex>(e).map_or(0, |z| z.0);
    let gz = |e: Entity| world.get::<&GlobalZIndex>(e).ok().map(|z| z.0);
    let is_node = |e: Entity| world.get::<&Node>(e).is_ok();
    // A node's camera: its own UiTargetCamera, else its root's, else the default UI camera.
    let target_of = |mut e: Entity| loop {
        if let Ok(t) = world.get::<&UiTargetCamera>(e) { return Some(t.0); }
        match world.get::<&ChildOf>(e) {
            Ok(p) if is_node(p.0) => e = p.0,
            _ => return None,
        }
    };
    let mut roots: Vec<Entity> = nodes
        .iter()
        .copied()
        .filter(|&e| gz(e).is_some() || world.get::<&ChildOf>(e).map_or(true, |p| !is_node(p.0)))
        .filter(|&e| match target_of(e) { Some(c) => c == camera, None => default_camera })
        .collect();
    roots.sort_by_key(|&e| (gz(e).unwrap_or(0), z(e), crate::engine::ecs::bevy_order(e)));
    // Rectangles, parents before children.
    let mut rects: HashMap<Entity, Vec4> = HashMap::new();
    fn place(world: &World, e: Entity, parent: Vec4, rects: &mut HashMap<Entity, Vec4>, children: &HashMap<Entity, Vec<Entity>>) {
        let Ok(n) = world.get::<&Node>(e) else { return };
        let x = parent.x + n.left.resolve(parent.z, 0.0);
        let y = parent.y + n.top.resolve(parent.w, 0.0);
        let w = n.width.resolve(parent.z, parent.z);
        let h = n.height.resolve(parent.w, parent.w);
        let r = Vec4::new(x, y, w, h);
        rects.insert(e, r);
        drop(n);
        for &c in children.get(&e).map(|v| v.as_slice()).unwrap_or(&[]) { place(world, c, r, rects, children); }
    }
    let full = Vec4::new(0.0, 0.0, target.x, target.y);
    for &r in &roots {
        let parent = world.get::<&ChildOf>(r).ok().and_then(|p| rects.get(&p.0).copied()).unwrap_or(full);
        place(world, r, parent, &mut rects, &children);
    }
    let mut out = Vec::new();
    fn visit(e: Entity, out: &mut Vec<(Entity, Vec4)>, rects: &HashMap<Entity, Vec4>, children: &HashMap<Entity, Vec<Entity>>, z: &dyn Fn(Entity) -> i32, gz: &dyn Fn(Entity) -> Option<i32>) {
        if let Some(r) = rects.get(&e) { out.push((e, *r)); }
        let mut cs: Vec<Entity> = children.get(&e).cloned().unwrap_or_default().into_iter().filter(|&c| gz(c).is_none()).collect();
        cs.sort_by_key(|&c| (z(c), crate::engine::ecs::bevy_order(c)));
        for c in cs { visit(c, out, rects, children, z, gz); }
    }
    for r in roots { visit(r, &mut out, &rects, &children, &z, &gz); }
    out
}

/// The renderer's UI step (crate::render::frame::Renderer::ui).
pub fn draw(uc: &mut UiCtx) -> Result<()> {
    let target = uc.target_size.as_vec2();
    let nodes = stack(&uc.ctx.world, uc.camera, uc.default_camera, target);
    if nodes.is_empty() { return Ok(()); }
    let mats: Vec<(TypeId, DrawMaterial)> = uc.ctx.try_res::<UiMaterials>().map(|m| m.0.clone()).unwrap_or_default();
    let clip = |r: Vec4| Vec4::new(r.x / target.x * 2.0 - 1.0, 1.0 - r.y / target.y * 2.0, (r.x + r.z) / target.x * 2.0 - 1.0, 1.0 - (r.y + r.w) / target.y * 2.0);
    for (e, r) in nodes {
        let visible = uc.ctx.world.get::<&InheritedVisibility>(e).map_or(true, |v| v.0);
        if !visible { continue; }
        let bg = uc.ctx.world.get::<&BackgroundColor>(e).ok().map(|b| b.0);
        if let Some(c) = bg {
            if c.alpha() > 0.0 {
                uc.quad.draw(uc.gfx, uc.enc, uc.format, None, clip(r), Vec4::new(0.0, 0.0, 1.0, 1.0), c.to_linear(), Some(Blend::ALPHA_BLENDING))?;
            }
        }
        let img = uc.ctx.world.get::<&ImageNode>(e).ok().map(|i| (*i).clone());
        if let Some(i) = img {
            let size = uc.gfx.images.get(&i.image.id()).map(|g| g.size.as_vec2()).unwrap_or(Vec2::ONE);
            let uv = match i.rect {
                Some(t) => Vec4::new(t.min.x / size.x, t.min.y / size.y, t.max.x / size.x, t.max.y / size.y),
                None => Vec4::new(0.0, 0.0, 1.0, 1.0),
            };
            uc.quad.draw(uc.gfx, uc.enc, uc.format, Some(i.image.id()), clip(r), uv, i.color.to_linear(), Some(Blend::ALPHA_BLENDING))?;
        }
        for (_, f) in &mats { f(uc, e, r)?; }
    }
    Ok(())
}

/// Bevy's default UI camera lookup for a node with no target: the camera marked [`IsDefaultUiCamera`].
pub fn default_ui_camera(world: &World) -> Option<Entity> { world.query::<(Entity, &IsDefaultUiCamera)>().iter().map(|(e, _)| e).next() }
