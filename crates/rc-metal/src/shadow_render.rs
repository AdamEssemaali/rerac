//! The moby shadows in the engine (docs/plan/shadows.md): the game's shadow rules each tick, the casters' volumes
//! each frame, and a native screen-space shadow-volume pass. One general system: any class with a shadow block
//! (`rc_formats::moby_shadow`) whose update fills its slab casts, with no per-class code here.
//!
//! * **Tick** (`FixedUpdate`, after each game tick, [`shadow_tick`]): Ratchet's gate, pitch and four probes
//!   (`rc_game::shadows::update_hero_shadow`, the game's `HeroUpdateAlt` call) and direction 0
//!   (`update_shadow_dir`, the end of `GameStateUpdate`), on the tick's collision (world mesh and mobys, flags 0x22).
//!   The other casters' slabs come from their class updates (`rc_game::shadows::probe_down` / `set_ground`).
//! * **Frame** (`Last`, [`collect`]): `MobyProc`'s deferral in the game's order (the moby array): a moby with mode
//!   0x400 (not 0x800), drawn this frame (+0x31), not hidden, with a slab (`hi > 0`) and a class shadow block; its
//!   bounding sphere moved along its direction to the slab's middle and grown by half the slab must pass the moby
//!   culls with the shadow range +0x7f as the draw distance (`moby_lod::moby_proc`); the size word by distance
//!   (`volume::shadow_size`); the 8,064-byte budget in list order (`volume::fitting`). Each caster is posed on its
//!   current skeleton pose (`moby_anim::evaluate_chains`, the pose the renderer draws) and turned into closed
//!   prisms (`rc_game::shadows::volume`), in Bevy axes, into [`ShadowVolumes`].
//! * **GPU** (the renderer's `Hook::AfterOpaque`, between the opaque and the transparent main passes, [`shadow_hook`]): the prisms
//!   into an `R16Float` count target (cleared to 0, same size and samples as the view), depth-tested
//!   `GreaterEqual` against the scene depth (read only), additive `+1` front / `−1` back faces; then a full-screen
//!   triangle multiplies the colour target by [`SHADOW_FACTOR`] where the count is positive. The 3D depth
//!   format has no stencil (rc-engine's Bevy one had none), hence the count target. The casters themselves are drawn after this pass
//!   (crate::moby_render `ShadowCaster`: their draws move to the late phase), so they are never darkened.
//!
//! Not reproduced (hardware): the GS guard-band face drop, the destination-alpha counter's byte arithmetic, the
//! NTSC / PAL register blocks. The darkening is a result-level reproduction (hardware_fidelity_layers.md).
//!
//! Environment: `RC_SHADOWS=0|1` overrides the Port Options setting at start (default on, as the game);
//! `RC_SHADOW_DEBUG=1` draws the count target instead (red inside, blue negative); `RC_SHADOW_TRACE=1` prints the
//! casters of each frame; `RC_SHADOW_PERF=1` the collection's mean CPU time.

use crate::fly_cam::FlyCam;
use crate::gameplay::Play;
use crate::moby_lod::{self, ProcInput};
use crate::tfrag_render::game_to_bevy;
use crate::prelude::*;
use crate::render::gfx::{self, Binding, PipelineKey, TopologyClass};
use objc2::rc::Retained;
use objc2_metal::{MTLCullMode, MTLWinding, 
    MTLClearColor, MTLColorWriteMask, MTLCommandBuffer, MTLCommandEncoder, MTLLoadAction, MTLPixelFormat, MTLPrimitiveType,
    MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLStoreAction, MTLTexture, MTLViewport,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use rc_formats::moby_anim::{self, SNAPSHOT_SEQ};
use rc_formats::moby_shadow::ShadowBlock;
use rc_game::collision_query::{coll_line_m, QueryFlags};
use rc_game::moby_runtime::mode;
use rc_game::shadows::{self, volume, HeroShadowIn, ShadowDirs};
use std::sync::Arc;

/// The darkening as one linear-light multiply on the sRGB target: the GS byte `Cd − ⌈Cd/4⌉` (0.75·Cd) is met
/// within ±2 on 244 of the 256 display bytes and −3 on 12 dark ones (12..40); 0.53 minimises that error
/// (`tests::factor_matches_the_gs_bytes`). Mirrored in `shadow_resolve.wgsl`.
#[cfg_attr(not(test), allow(dead_code))]
pub const SHADOW_FACTOR: f32 = 0.53;
/// The count target's format (blendable; counts the small integers exactly).
pub const COUNT_FORMAT: TextureFormat = TextureFormat::R16Float;

/// The shadows' setting (Port Options "Shadows", on by default like the game).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShadowSettings {
    pub enabled: bool,
    /// `RC_SHADOW_DEBUG=1`: show the count target.
    pub debug: bool,
}

impl ShadowSettings {
    /// `RC_SHADOWS=0|1`, else the port settings file's `shadows = on|off`, else on.
    pub fn startup() -> (Self, &'static str) {
        let debug = std::env::var("RC_SHADOW_DEBUG").is_ok_and(|v| v.trim() == "1");
        if let Ok(v) = std::env::var("RC_SHADOWS") { return (ShadowSettings { enabled: v.trim() != "0", debug }, "RC_SHADOWS"); }
        match crate::render_settings::load_key("shadows") {
            Some(v) => (ShadowSettings { enabled: v.trim() != "off", debug }, "settings file"),
            None => (ShadowSettings { enabled: true, debug }, "default"),
        }
    }

    /// Writes the setting into the port settings file.
    pub fn save(&self) { crate::render_settings::save_key("shadows", if self.enabled { "on" } else { "off" }); }
}

/// The game-side shadow state the engine keeps: the direction table 0x1af000, Ratchet's pitch 0x140070, the light
/// sets' directions (for `fun_0020d510`) and the class shadow blocks.
#[derive(Default)]
pub struct ShadowGame {
    pub dirs: ShadowDirs,
    pub pitch: f32,
    /// The game tick the state was last updated for.
    counter: Option<u64>,
    dir_a: Vec<[f32; 4]>,
    blocks: HashMap<i16, Arc<ShadowBlock>>,
    /// o_class → index into `LevelMobys::classes` / `anim`.
    class_index: HashMap<i16, usize>,
}

/// The frame's shadow volumes: triangles (3 points each) in Bevy world axes.
#[derive(Clone, Default)]
pub struct ShadowVolumes {
    pub tris: Arc<Vec<[f32; 3]>>,
    pub debug: bool,
    /// The mobys `MobyProc` deferred as casters this frame (their draws after the shadow pass: crate::moby_render
    /// `CasterTwin`; read the next frame [L]).
    pub deferred: std::collections::HashSet<usize>,
}

/// rc-engine's render-world set of the shadow pass (crate::pre_shadow drew before it); here [`shadow_hook`] is the
/// renderer's `Hook::AfterOpaque`, after the pre-shadow draws. Kept as a name only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShadowPassSet;

/// The camera whose view gets the shadow pass (the main world camera).
#[derive(Clone, Copy, Debug, Default)]
pub struct ShadowCamera;

/// rc-engine's `ShadowPlugin`: [`ShadowSettings`] and [`ShadowVolumes`]. Register [`shadow_hook`] as
/// `Hook::AfterOpaque`. Systems (crate::main's schedule): LevelUnload `level_switch::reset::<ShadowVolumes>`,
/// `remove::<ShadowGame>`; LevelStartup [`setup`]; Update [`tag_camera`]; Last [`collect`]; FixedUpdate
/// [`shadow_tick`] (after `gameplay::GameTick` and `scene_render::actor_mobys`, only when `gameplay::enabled()`).
pub fn plugin(app: &mut crate::engine::app::App, _r: &mut crate::render::frame::Renderer) {
    let (settings, source) = ShadowSettings::startup();
    println!("shadows: {} ({source}){}", if settings.enabled { "on" } else { "off" }, if settings.debug { ", RC_SHADOW_DEBUG: count target shown" } else { "" });
    app.ctx.res.insert(settings);
    app.ctx.res.init::<ShadowVolumes>();
    app.ctx.res.init::<CollectLocal>();
}

pub fn setup(ctx: &mut Ctx) {
    let level = ctx.res.get::<crate::Level>();
    let lv = &level.0;
    let (root, index) = (crate::level_load::extracted_root(), crate::level_load::level_index());
    let mut game = ShadowGame {
        dir_a: rc_formats::tfrag_light::parse_light_bank(&lv.gameplay).map(|b| b.sets.iter().map(|s| s.dir_a).collect()).unwrap_or_default(),
        class_index: lv.mobys.classes.iter().enumerate().map(|(i, c)| (c.o_class as i16, i)).collect(),
        ..Default::default()
    };
    let blocks = (|| -> anyhow::Result<_> {
        let data = rc_data::level_core_data(&root, index)?;
        let core = rc_formats::level::parse_level_core(&crate::disc_source::level_file(&root, index, "core_index.bin")?, data.len())?;
        Ok(rc_formats::moby_shadow::parse_level(&core, &data)?)
    })();
    match blocks {
        Ok(b) => {
            println!("shadows: {} classes with a shadow block", b.len());
            game.blocks = b.into_iter().map(|(oc, _, blk)| (oc as i16, Arc::new(blk))).collect();
        }
        Err(e) => eprintln!("shadows: shadow blocks not read ({e:#}): no shadows"),
    }
    drop(level);
    ctx.res.insert(game);
}

/// Tags the main world camera (FlyCam + Camera3d) with [`ShadowCamera`].
pub fn tag_camera(ctx: &mut Ctx) {
    let cams: Vec<Entity> = ctx.world.query::<Entity>().with::<&FlyCam>().with::<&Camera3d>().without::<&ShadowCamera>().iter().collect();
    for e in cams { let _ = ctx.world.insert_one(e, ShadowCamera); }
}

/// After each game tick: Ratchet's shadow (gate, pitch, direction 1, slab) and direction 0. While a scene runs, every
/// frame of it (the game's scene update, also during its blocking waits): the actors' slabs, on the pose they are
/// drawn with this frame (crate::scene_render::actor_mobys wrote it into their table mobys just before).
pub fn shadow_tick(ctx: &mut Ctx) {
    let (Some(mut play), Some(mut sg)) = (ctx.res.try_get_mut::<Play>(), ctx.res.try_get_mut::<ShadowGame>()) else { return };
    let level = ctx.res.get::<crate::Level>();
    let active = ctx.res.try_get::<crate::scene_render::ActiveScene>();
    let counter = play.game.counter;
    let ticked = sg.counter != Some(counter);
    let actors = active.as_ref().filter(|s| s.running).map_or(&[][..], |s| &s.actors[..]);
    // Game mode 6's take-off / landing actors (crate::travel_render): the same probe in `GameStateUpdate`.
    let space = active.as_ref().map_or(&[][..], |s| &s.space_actors[..]);
    if !ticked && actors.is_empty() && space.is_empty() { return; }
    sg.counter = Some(counter);
    let Some(coll) = level.0.collision.as_ref() else { return };
    let sg = &mut *sg;
    let p = &mut *play;
    let hero_id = p.game.hero_moby;
    let Some(hero_moby) = p.game.mobys.mobys.get(hero_id) else { return };
    let light = shadows::light_dir(&sg.dir_a, hero_moby.light);
    let scene = p.svc.hero_scene(&p.game.mobys, p.classes.clone(), None);
    let sc = scene.scene();
    let line = |a: [f32; 3], b: [f32; 3]| coll_line_m(coll, Some(&sc), a, b, QueryFlags(shadows::PROBE_FLAGS), None).map(|o| o.point[2]);
    if ticked {
        let h = &p.game.hero;
        let hin = HeroShadowIn {
            group: h.group,
            state: h.state,
            air_ticks: h.air_ticks,
            ground_z: h.ground_z.to_f32(),
            water_z: h.water_level.to_f32(),
            liquid_z: h.surf.liquid,
        };
        // `0x22a260` runs on the hero moby 0x1413d0 (a body's while one is in: rc_game::hero::bodies).
        let hm = p.game.hero.hero_moby(hero_id);
        shadows::update_hero_shadow(&mut p.game.mobys.mobys[hm], &mut sg.pitch, &mut sg.dirs, &hin, light, line);
    }
    // The scene actors (`CutsceneModeUpdate`, right after each actor's `MobyBuildMatrix`: +0x7f ≠ 0 →
    // ShadowProbeAlongDir 0x26f0e0 with direction 0), before the direction update at the end of the frame.
    for (id, _) in actors.iter().chain(space) {
        if let Some(m) = p.game.mobys.mobys.get_mut(*id).filter(|m| m.b7f != 0) { shadows::probe_along_dir(m, sg.dirs.of(0), line); }
    }
    if ticked { shadows::update_shadow_dir(&mut sg.dirs, light); }
}

/// A caster that passed `MobyProc`'s shadow tests this frame.
struct Deferred {
    id: usize,
    size: u16,
    bytes: usize,
}

/// [`collect`]'s locals: the frame counter of `RC_SHADOW_TRACE` and the `RC_SHADOW_PERF` sums.
#[derive(Default)]
pub struct CollectLocal {
    trace_frame: u64,
    perf: (std::time::Duration, u32, usize),
}

/// The frame's casters and their volumes (module docs).
pub fn collect(ctx: &mut Ctx) {
    let settings = *ctx.res.get::<ShadowSettings>();
    let play = ctx.res.try_get::<Play>();
    let level = ctx.res.get::<crate::Level>();
    let game = ctx.res.try_get::<ShadowGame>();
    let cam_view: Option<(Transform, Option<Projection>)> =
        ctx.world.query::<(&Transform, Option<&Projection>)>().with::<&ShadowCamera>().iter().next().map(|(t, p)| (*t, p.cloned()));
    let scene = ctx.res.try_get::<crate::scene_render::ActiveScene>();
    let vendor_hidden_any = ctx.world.query::<&crate::interact_render::VendorHidden>().iter().next().is_some();
    let mut out = ctx.res.get_mut::<ShadowVolumes>();
    let mut local = ctx.res.get_mut::<CollectLocal>();
    let CollectLocal { trace_frame, perf } = &mut *local;
    *trace_frame += 1;
    let t0 = std::time::Instant::now();
    let clear = |out: &mut ShadowVolumes| {
        if !out.tris.is_empty() { out.tris = Arc::new(Vec::new()); }
        out.deferred.clear();
    };
    let (Some(play), Some(sg), Some((cam, proj))) = (play, game, cam_view.as_ref()) else { return clear(&mut out) };
    // MobyProc's frustum planes from the view (16:9 widens them, crate::display).
    let tans = crate::game_camera::projection_tans(proj.as_ref());
    if !settings.enabled { return clear(&mut out); }
    let (eye, rows) = (crate::game_camera::game_eye(cam), crate::game_camera::game_rows(cam));
    let cam_rows = moby_lod::camera_rows(rows[0], rows[1], rows[2]);
    let table = &play.game.mobys;
    // Ratchet's moby is hidden by his drawn entities, not by mode bits, while a scene runs (`FUN_002486c0`'s mode |= 1)
    // and in the vendor (crate::interact_render::hide_hero): MobyProc then skips him, shadow included.
    let hero_hidden = scene.as_ref().is_some_and(|s| s.running || s.space_hero_hidden) || vendor_hidden_any;
    // MobyProc's deferral: the shadow sphere, its cull and size, in the moby array's order.
    // The scene actors: drawn by crate::scene_render (their table mobys carry the port-only hidden bit), posed on the
    // streamed sequence.
    let actor = |id: usize| scene.as_ref().and_then(|s| s.actors.iter().chain(&s.space_actors).find(|a| a.0 == id).map(|a| a.1.clone()));
    let mut list = Vec::new();
    for (id, m) in table.mobys.iter().enumerate() {
        let is_actor = actor(id).is_some();
        let hidden_bits = if is_actor { 0x80 } else { mode::HIDDEN | 0x80 };
        if m.is_deleted() || m.mode & mode::CLASS_F == 0 || m.mode & 0x800 != 0 || m.mode & hidden_bits != 0 { continue; }
        if (m.visible == 0 && !is_actor) || m.shadow_hi <= 0.0 || m.shadow_hi.is_nan() || (hero_hidden && id == play.game.hero_moby) { continue; }
        let Some(block) = sg.blocks.get(&m.o_class) else { continue };
        let d = sg.dirs.of(m.bbd);
        let (lo, hi) = (m.shadow_lo * 1024.0, m.shadow_hi * 1024.0);
        let half = (hi - lo) * 0.5;
        let r = m.bsphere[3] + half;
        if d[2] == 0.0 { continue; }
        let q = ((lo + half) - m.bsphere[2]) / d[2];
        let c = [d[0] * q + m.bsphere[0], d[1] * q + m.bsphere[1], d[2] * q + m.bsphere[2], r];
        let v = moby_lod::view_centre(c, eye, &cam_rows);
        let inp = ProcInput { position: [m.position[0], m.position[1], m.position[2]], rows: [[0.0; 3]; 3], scale: m.scale, draw_distance: m.b7f as i32, lod_trans: 0xff, shine_distance: 0, alpha: 0x80 };
        if moby_lod::moby_proc_view(v, r, &inp, tans).is_err() { continue; }
        let size = volume::shadow_size(m.b7f, moby_lod::sphere_depth(v, r));
        list.push(Deferred { id, size, bytes: volume::caster_bytes(block) });
    }
    let n = volume::fitting(list.iter().map(|c| c.bytes));
    out.deferred.clear();
    out.deferred.extend(list[..n].iter().map(|c| c.id));
    let mut tris = Vec::new();
    for c in &list[..n] {
        let m = &table.mobys[c.id];
        let (Some(block), Some(&ci)) = (sg.blocks.get(&m.o_class), sg.class_index.get(&m.o_class)) else { continue };
        let streamed = actor(c.id);
        let anim = streamed.as_deref().unwrap_or(&level.0.mobys.anim[ci]);
        let snap = if m.anim.seq_a != SNAPSHOT_SEQ {
            None
        } else if c.id == play.game.hero_moby {
            play.ratchet.snapshot.as_ref()
        } else {
            play.svc.snapshots.get(c.id).and_then(Option::as_ref)
        };
        // The posed joints the records read (0x267fc0 poses the class's joints; the evaluator's chains).
        let top = block.prims.iter().flat_map(|p| p.joints()).max().unwrap_or(0) as usize + 1;
        let chains: Vec<Vec<u8>> = (0..top.min(anim.joint_count).max(1)).map(|j| (0..=j as u8).collect()).collect();
        let refs: Vec<&[u8]> = chains.iter().map(Vec::as_slice).collect();
        // With the runtime lists as the game poses them: the joint modifiers (+0x64; Ratchet's head look, lean and
        // eyelids) and, for Ratchet, his weapon arm (+0x60).
        let layers = if c.id == play.game.hero_moby { rc_game::hero::anim::pose_layers_with(&play.game.hero.weapons.layers, &play.ratchet.layer_joints, &play.ratchet.hold) } else { Vec::new() };
        let joints = moby_anim::evaluate_chains_posed(anim, &m.anim, snap, &refs, &layers, &m.joint_mods);
        let r3 = |k: usize| [m.rows[k][0], m.rows[k][1], m.rows[k][2]];
        let pose = volume::CasterPose { joints: &joints, rows: [r3(0), r3(1), r3(2)], position: [m.position[0], m.position[1], m.position[2]], scale: m.scale, size: c.size };
        let prims = volume::pose_prims(block, &pose);
        let before = tris.len();
        volume::caster_volume(&prims, sg.dirs.of(m.bbd), m.shadow_lo, m.shadow_hi, &mut tris);
        if std::env::var("RC_SHADOW_TRACE").is_ok_and(|v| v.trim() == "1") {
            println!(
                "shadows: frame {} moby {} class {} size {:#x} slab [{:.3}, {:.3}] dir {} {} triangles",
                *trace_frame, c.id, m.o_class, c.size, m.shadow_lo, m.shadow_hi, m.bbd, (tris.len() - before) / 3
            );
        }
    }
    for p in &mut tris { *p = game_to_bevy(*p).to_array(); }
    // RC_SHADOW_PERF=1: the mean CPU time of this system (casters, poses, volumes) every 600 frames.
    if std::env::var("RC_SHADOW_PERF").is_ok_and(|v| v.trim() == "1") {
        *perf = (perf.0 + t0.elapsed(), perf.1 + 1, perf.2 + tris.len() / 3);
        if perf.1 == 600 {
            println!("shadows: collect {:.1} µs per frame, {} triangles per frame (mean of 600)", perf.0.as_secs_f64() * 1e6 / 600.0, perf.2 / 600);
            *perf = Default::default();
        }
    }
    out.tris = Arc::new(tris);
    out.debug = settings.debug;
}

// ---------------------------------------------------------------------------------------------------
// GPU: the renderer's hook (rc-engine's render world: `ShadowGpu`, `ShadowPipelines`, `prepare`, `shadow_pass`)

/// Pipeline kinds.
const VOLUME: u8 = 0;
const DARKEN: u8 = 1;
const DEBUG: u8 = 2;

thread_local! {
    /// The count target, kept while the view's size and samples stay (rc-engine: Bevy's texture cache).
    static COUNT_TARGET: RefCell<Option<((usize, usize, u32), Retained<gfx::Texture>)>> = const { RefCell::new(None) };
}

/// The pipeline of `kind` for the view's samples and colour format.
fn pipeline(g: &mut gfx::Gfx, kind: u8, samples: u32, format: TextureFormat) -> anyhow::Result<Rc<gfx::PipelineEntry>> {
    let key = if kind == VOLUME {
        let add = BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::One, operation: BlendOperation::Add };
        // Both faces (front +1, back −1); the scene depth read only, reverse Z (GS GEQUAL): the depth state below.
        PipelineKey {
            shader: "shadow_volume",
            vertex_entry: "vertex",
            vertex_shader: None,
            fragment_entry: Some("fragment"),
            defs: Default::default(),
            vertex_attributes: vec![(0, VertexFormat::Float32x3, 0)],
            vertex_stride: 12,
            color_formats: vec![COUNT_FORMAT],
            blend: Some(Blend { color: add, alpha: add }),
            write_mask: MTLColorWriteMask::Red,
            depth: true,
            samples,
            topology_class: TopologyClass::Triangle,
        }
    } else {
        // Darken: dst·K (Zero / Src), alpha kept; debug: replace.
        let blend = (kind == DARKEN).then_some(Blend {
            color: BlendComponent { src_factor: BlendFactor::Zero, dst_factor: BlendFactor::Src, operation: BlendOperation::Add },
            alpha: BlendComponent { src_factor: BlendFactor::Zero, dst_factor: BlendFactor::One, operation: BlendOperation::Add },
        });
        PipelineKey {
            shader: "shadow_resolve",
            vertex_entry: "fullscreen_vertex_shader",
            vertex_shader: Some("fullscreen_vertex_shader"),
            fragment_entry: Some(if kind == DARKEN { "darken" } else { "debug" }),
            defs: if samples > 1 { crate::shader::defs([("MULTISAMPLED", None)]) } else { Default::default() },
            vertex_attributes: vec![],
            vertex_stride: 0,
            color_formats: vec![format],
            blend,
            write_mask: MTLColorWriteMask::All,
            depth: false,
            samples,
            topology_class: TopologyClass::Triangle,
        }
    };
    g.pipeline(&key)
}

/// The shadow pass of the shadow view (module docs): the frame's triangles into the count target, then the resolve.
/// The renderer's `Hook::AfterOpaque`, called for every 3D camera; only the [`ShadowCamera`] view gets it.
pub fn shadow_hook(hc: &mut HookCtx) -> anyhow::Result<()> {
    if hc.ctx.world.get::<&ShadowCamera>(hc.camera).is_err() { return Ok(()); }
    let Some(volumes) = hc.ctx.try_res::<ShadowVolumes>() else { return Ok(()) };
    if volumes.tris.is_empty() { return Ok(()); }
    let (tris, debug) = (volumes.tris.clone(), volumes.debug);
    drop(volumes);
    let samples = hc.samples;
    let vol_pipe = pipeline(hc.gfx, VOLUME, samples, hc.format)?;
    let res_pipe = pipeline(hc.gfx, if debug { DEBUG } else { DARKEN }, samples, hc.format)?;
    let depth_state = hc.gfx.depth_state(CompareFunction::GreaterEqual, false)?;
    let bytes: Vec<u8> = tris.iter().flat_map(|p| p.iter().flat_map(|x| x.to_le_bytes())).collect();
    let buffer = gfx::new_buffer(&hc.gfx.device, &bytes, "shadow volumes")?;
    // The view's clip_from_world: the first matrix of the view uniform (shaders/shim/mesh_view_bindings.wgsl).
    let view = Binding::Bytes(Rc::new(hc.view[..64].to_vec()));
    let (w, h) = (hc.depth.width(), hc.depth.height());
    let key = (w, h, samples);
    let count = COUNT_TARGET.with(|c| -> anyhow::Result<Retained<gfx::Texture>> {
        let mut c = c.borrow_mut();
        if let Some((k, t)) = c.as_ref() {
            if *k == key { return Ok(t.clone()); }
        }
        let t = hc.gfx.target(MTLPixelFormat::R16Float, w, h, samples, true)?;
        *c = Some((key, t.clone()));
        Ok(t)
    })?;
    let (vx, vy, vw, vh) = hc.viewport;
    let viewport = MTLViewport { originX: vx, originY: vy, width: vw, height: vh, znear: 0.0, zfar: 1.0 };
    // Volumes.
    {
        let rpd = MTLRenderPassDescriptor::renderPassDescriptor();
        // SAFETY: attachment 0.
        let ca = unsafe { rpd.colorAttachments().objectAtIndexedSubscript(0) };
        ca.setTexture(Some(&count));
        ca.setLoadAction(MTLLoadAction::Clear);
        ca.setClearColor(MTLClearColor { red: 0.0, green: 0.0, blue: 0.0, alpha: 0.0 });
        ca.setStoreAction(MTLStoreAction::Store);
        let da = rpd.depthAttachment();
        da.setTexture(Some(hc.depth));
        da.setLoadAction(MTLLoadAction::Load);
        da.setStoreAction(MTLStoreAction::Store);
        let enc = hc.cb.renderCommandEncoderWithDescriptor(&rpd).ok_or_else(|| anyhow::anyhow!("shadow volumes: render encoder"))?;
        enc.setViewport(viewport);
        enc.setRenderPipelineState(&vol_pipe.state);
        enc.setDepthStencilState(Some(&depth_state));
        // wgpu's front face (counter-clockwise; Metal's default is clockwise): `front_facing` counts +1 on the outside.
        enc.setFrontFacingWinding(MTLWinding::CounterClockwise);
        enc.setCullMode(MTLCullMode::None);
        gfx::bind(&enc, &vol_pipe.vertex, true, &[(0, 0, &view)]);
        // SAFETY: the frame's triangles, three float3 per triangle.
        unsafe {
            enc.setVertexBuffer_offset_atIndex(Some(&buffer), 0, crate::shader::VERTEX_BUFFER_BASE as usize);
            enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, tris.len());
        }
        enc.endEncoding();
    }
    // Resolve: the colour target darkened where the count is positive (the view's next pass resolves MSAA).
    let rpd = MTLRenderPassDescriptor::renderPassDescriptor();
    // SAFETY: attachment 0.
    let ca = unsafe { rpd.colorAttachments().objectAtIndexedSubscript(0) };
    ca.setTexture(Some(hc.color));
    ca.setLoadAction(MTLLoadAction::Load);
    ca.setStoreAction(MTLStoreAction::Store);
    let enc = hc.cb.renderCommandEncoderWithDescriptor(&rpd).ok_or_else(|| anyhow::anyhow!("shadow resolve: render encoder"))?;
    enc.setViewport(viewport);
    enc.setRenderPipelineState(&res_pipe.state);
    let count_b = Binding::Texture(count);
    if let Some(f) = &res_pipe.fragment { gfx::bind(&enc, f, false, &[(0, 0, &count_b)]); }
    // SAFETY: the fullscreen triangle.
    unsafe { enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3) };
    enc.endEncoding();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s2l(c: f32) -> f32 { if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) } }
    fn l2s(c: f32) -> f32 { if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 } }

    /// The resolve's linear multiply against the GS byte `Cd − ⌈Cd/4⌉` (ALPHA_1 0x2000000064: `(0 − Cd)·0x20 >> 7
    /// + Cd`, arithmetic shift), on every display byte.
    #[test]
    fn factor_matches_the_gs_bytes() {
        let (mut worst, mut over2) = (0i32, 0);
        for d in 0..=255i32 {
            let gs = d + ((-d * 0x20) >> 7);
            let port = (l2s(SHADOW_FACTOR * s2l(d as f32 / 255.0)) * 255.0).round() as i32;
            let e = port - gs;
            worst = worst.max(e.abs());
            if e.abs() > 2 {
                over2 += 1;
                assert!((12..=40).contains(&d) && e == -3, "byte {d}: {port} vs {gs}");
            }
        }
        assert_eq!((worst, over2), (3, 12));
        assert_eq!(255 + ((-255 * 0x20) >> 7), 191, "white → 191");
    }

    #[test]
    fn shader_uses_the_same_factor() {
        let wgsl = include_str!("../../rc-engine/assets/shaders/shadow_resolve.wgsl");
        assert!(wgsl.contains(&format!("const K: f32 = {SHADOW_FACTOR};")), "shadow_resolve.wgsl K");
    }
}
