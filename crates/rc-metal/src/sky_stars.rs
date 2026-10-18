//! Sky stars: the star step of the level sky dispatch (`rc_game::sky_stars`) and `SkySpriteProc`'s sprites
//! (level00 0x28bd58, boot 0x22bba0), drawn in the sky pass between the shells the dispatch names
//! (docs/plan/sky_render_notes.md "Stars in the port").
//!
//! `RC_SKY_STARS=0` skips the star step (for comparisons); `RC_STAR_STATS=1` prints the records' hash each second.
//!
//! **When.** The game generates and updates the records inside the frame render, i.e. after the frame's game
//! tick (main loop 0x259c40: pad → gameplay tick → render). Here: one star frame per 60 Hz `FixedUpdate` tick,
//! after the gameplay tick (and the `RC_PLAY=0` particle tick), drawing from the game's one `rand` stream
//! (`Game::rng` of crate::gameplay, after the tick's counter increment; with `RC_PLAY=0` the particle
//! simulation's stand-alone stream; both seeded `srand(1234)` at level init; the star code never reseeds). The first tick generates, then updates in
//! the same frame, as the dispatch does. At 60 fps the game renders once per tick, so this is its schedule;
//! at 30 fps its catch-up tick would skip a star frame (not modelled).
//!
//! **Draw** (`SkySpriteProc`, per record in index order, no sort):
//! * camera space c = R·pos with R the rotation-only view (level00 0x166c80; x right, y down, z forward): the
//!   stars sit on a radius-50 shell around the eye and never translate. They do not use SkyM (the shells'
//!   rotation), so they do not turn with any shell.
//! * texture byte < 0 → skipped; cull unless `tan·c.z − (|c| − size·sec) ≥ 0` in x and y (vf26 = (tan_x,
//!   tan_y) 0x16caf0, vf27 = (sec_x, sec_y) 0x16cc20); no near/far test, no fade.
//! * half-diagonal Q = size·832/c.z frame-buffer pixels: `vmulw.x vf10, vf10, vf25` takes w of 0x16cc40 =
//!   (W/2, H/2, 1024, 832) = 832 (4·H/2). (`PartProc` uses 256/(0.63·210000) per 1/210000 unit instead.)
//! * the particle VU1 program's kind-0 sprite (same constants block, `(1, 1.0625)` y factor): corners
//!   S ± (Q cos θ, 1.0625 Q sin θ), S ± (−Q sin θ, 1.0625 Q cos θ), θ = the f32 at +8 (VU0 sin/cos), ST
//!   (0,0) (1,0) (0,1) (1,1); RGBA = +4 as is; TEX0 = sky texture byte 2 (header +0x10 table, the loader's
//!   TEX0 template 0x8000000401300000: PSMT8, TCC 1, MODULATE), bilinear, clamp.
//! * GS state: ALPHA_1 = byte 3 = 0x48 (`Cs·As + Cd`, additive) on every star; TEST_1/ZBUF_1 as the previous
//!   shell left them (0x3180b, ZMSK 1: ZTST ALWAYS, alpha test with AFAIL FB_ONLY, no Z write), so no depth
//!   test, no depth write, colour always written; FGE 0 (no fog). After the step the dispatch restores
//!   ALPHA_1 = 0x8000000044 for the next shell (whose own A+D block sets it anyway).
//!
//! Bevy: one draw per sky texture used (additive blending makes the draw order within the step irrelevant:
//! non-negative adds clamp to the same sum in any order), each a persistent quad mesh of one quad per record
//! and a sprite storage buffer rewritten every frame (`particle_render::SpriteBuffer`, no mesh re-insert),
//! `depth_bias` = the reserved slot
//! [`crate::sky_render::SkyStarOrder`] so they sort between the right shells; the shader adds the pixel offsets
//! in clip space like `particle.wgsl` and sets depth 0 (unused: compare Always, no write).

use crate::game_camera::game_rows;
use crate::sky_render::{SkyCamera, SkyStarOrder, ORDER_SPACING, SKY_LAYER};
use crate::tfrag_render::game_to_bevy;
use anyhow::{bail, Context, Result};
use crate::particle_render::{quad_mesh, SpriteBuffer, ATTRIBUTE_SPRITE_CORNER};
use crate::prelude::*;
use rc_formats::moby_light::vu0_sin_cos;
use rc_game::rng::{Rng, LEVEL_SEED};
use rc_game::sky_stars::{level_stars, OverlayTables, SkyStars, Star, RECORD};

/// `shaders/sky_stars.wgsl`.
const SHADER: &str = "sky_stars";

/// 0x16cc4c: 4·H/2 = 832 (the w lane `SkySpriteProc` scales the size by).
const SIZE_TO_PIXELS: f32 = 832.0;

/// `(vaddr, bytes)` from a level overlay (`overlay.bin`, Insomniac's "ratchet executable": `[dest, size, type,
/// entry]` + data per section until the entry changes; docs/formats/wad_layouts_rac1.md §4).
pub fn overlay_read(overlay: &[u8], vaddr: u32, len: usize) -> Option<Vec<u8>> {
    let word = |o: usize| overlay.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let (mut pos, mut entry) = (0usize, None);
    while pos + 16 <= overlay.len() {
        let (dest, size, ty, ep) = (word(pos)?, word(pos + 4)? as usize, word(pos + 8)?, word(pos + 12)?);
        if *entry.get_or_insert(ep) != ep { break; }
        let data = overlay.get(pos + 16..pos + 16 + size)?;
        if ty == 1 && vaddr >= dest && (vaddr - dest) as usize + len <= size {
            let o = (vaddr - dest) as usize;
            return Some(data[o..o + len].to_vec());
        }
        pos += 16 + size;
    }
    None
}

/// The level-06/17 tables from the level's overlay (zeros on the levels that do not read them).
fn load_tables(level: u32) -> Result<OverlayTables> {
    if !matches!(level, 6 | 17) { return Ok(OverlayTables::default()); }
    let ov = crate::disc_source::level_file(&crate::level_load::extracted_root(), level, "overlay.bin").context("reading overlay.bin")?;
    let t = OverlayTables::read(|a, n| overlay_read(&ov, a, n));
    match t {
        Some(t) => Ok(t),
        None => bail!("level {level:02}: star tables outside the overlay's data sections"),
    }
}

pub(crate) struct StarSim {
    stars: SkyStars,
    /// Own stream when there is no particle simulation (it is the game's shared stream otherwise).
    fallback_rng: Rng,
    frames: u64,
    /// Per group (entity): its sky texture index, sprites (tag = 1 when ALPHA_1 is additive 0x48) and buffer.
    groups: Vec<(u8, SpriteBuffer, Handle<ShaderBuffer>)>,
    drawn: usize,
    stats: bool,
}

struct StarDraw(usize);

/// Bindings: 0 texture, 1 sampler, 2 sprites (storage).
#[derive(Debug, Clone)]
pub struct SkyStarMaterial {
    /// The sky texture with raw GS alpha (0..0x80), as `sky_render::sky_image` builds it.
    pub texture: Handle<Image>,
    /// This group's sprites (`particle_render::SpriteBuffer`; the centre is the star's direction).
    pub sprites: Handle<ShaderBuffer>,
    /// Sort key: the reserved sky slot plus a sub-slot per group.
    pub bias: f32,
}

/// `One, OneMinusSrcAlpha` on premultiplied output: alpha As (0x44) or 0 (0x48) picks the equation.
const PREMULTIPLIED: Blend = Blend {
    color: BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::OneMinusSrcAlpha, operation: BlendOperation::Add },
    alpha: BlendComponent { src_factor: BlendFactor::Zero, dst_factor: BlendFactor::One, operation: BlendOperation::Add },
};

impl Material for SkyStarMaterial {
    fn shader() -> &'static str { SHADER }
    fn alpha_mode(&self) -> AlphaMode { AlphaMode::Blend }
    fn depth_bias(&self) -> f32 { self.bias }

    fn specialize(&self, desc: &mut PipelineDesc, _layout: &MeshLayout) -> Result<(), String> {
        desc.cull = None;
        // ZTST ALWAYS, ZMSK 1 (the state the previous shell left).
        desc.depth_write = false;
        desc.depth_compare = CompareFunction::Always;
        desc.blend = Some(PREMULTIPLIED);
        desc.vertex_attributes = vec![ATTRIBUTE_SPRITE_CORNER.at_shader_location(0)];
        Ok(())
    }
    fn bindings(&self, b: &mut BindGroup) {
        b.texture_sampler(0, &self.texture);
        b.storage(2, &self.sprites);
    }
}

/// Schedule (central, crate::main): LevelPostStartup `setup`; LevelUnload `unload`; FixedUpdate `star_frame` after
/// `crate::particle_render::tick` and `crate::gameplay::tick` (GameTick); PostUpdate `build_meshes`.
pub fn plugin(app: &mut crate::engine::app::App, r: &mut crate::render::frame::Renderer) { r.add_material::<SkyStarMaterial>(&mut app.ctx); }

/// LevelUnload: rc-engine's `level_switch::remove::<StarSim>`.
pub fn unload(ctx: &mut Ctx) { ctx.res.remove::<StarSim>(); }

pub fn setup(ctx: &mut Ctx) {
    let sim = {
        let level = ctx.res.get::<crate::Level>();
        let slot = ctx.res.try_get::<SkyStarOrder>().map(|s| s.0);
        let (Some(ls), Some(slot)) = (level.0.sky.as_ref(), slot) else { return };
        let (mut meshes, mut images, mut materials, mut buffers) =
            (ctx.res.get_mut::<Assets<Mesh>>(), ctx.res.get_mut::<Assets<Image>>(), ctx.res.get_mut::<Assets<SkyStarMaterial>>(), ctx.res.get_mut::<Assets<ShaderBuffer>>());
        spawn_stars(&mut ctx.world, ls, slot, RenderLayers::layer(SKY_LAYER), &mut meshes, &mut images, &mut materials, &mut buffers)
    };
    if let Some(sim) = sim { ctx.res.insert(sim); }
}

/// The star step of sky `ls` (its level's generator, [`level_stars`]) drawn in sky order slot `slot` on render layer
/// `layer`; the simulation the caller inserts (one at a time: the level's, or the title world's while it is shown,
/// crate::title_world). None: no star step, or `RC_SKY_STARS=0`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_stars(
    world: &mut World,
    ls: &crate::sky_render::LevelSky,
    slot: u32,
    layer: RenderLayers,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<SkyStarMaterial>,
    buffers: &mut Assets<ShaderBuffer>,
) -> Option<StarSim> {
    let info = level_stars(ls.level)?;
    if std::env::var("RC_SKY_STARS").is_ok_and(|v| v.trim() == "0") {
        println!("sky stars: RC_SKY_STARS=0, not generated or drawn (the rand stream then differs from the game's)");
        return None;
    }
    let tables = if ls.level == rc_game::sky_stars::TITLE_LEVEL {
        rc_game::sky_stars::OverlayTables::default()
    } else {
        match load_tables(ls.level) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("sky stars: {e:#}; stars disabled");
                return None;
            }
        }
    };
    let initial: Vec<Star> = ls.sprite_scratch.as_chunks::<RECORD>().0.iter().map(|c| Star(*c)).collect();
    let nonzero = ls.sprite_scratch.iter().filter(|&&b| b != 0).count();
    // Texture groups: every sky texture index a generator can write (twinkle 0/1 or 2/3, fixed 2, moving 1/3).
    let mut texs: Vec<u8> = match info.variant {
        rc_game::sky_stars::Variant::L02 => vec![2, 3],
        rc_game::sky_stars::Variant::L06 => vec![0, 1, 2],
        _ => vec![0, 1],
    };
    texs.retain(|&t| ls.textures.iter().any(|x| x.index == t as usize));
    let mut groups = Vec::new();
    // One quad per record: every record can land in any group.
    let capacity = info.count as u32;
    let mesh = meshes.add(quad_mesh(capacity));
    for (g, &t) in texs.iter().enumerate() {
        let tex = ls.textures.iter().find(|x| x.index == t as usize).unwrap();
        let mut sprites = SpriteBuffer::new(capacity);
        let buffer = buffers.add(sprites.asset());
        let mat = materials.add(SkyStarMaterial {
            texture: images.add(crate::sky_render::sky_image(tex)),
            sprites: buffer.clone(),
            bias: slot as f32 * ORDER_SPACING + g as f32,
        });
        world.spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(mat),
            Transform::IDENTITY,
            layer.clone(),
            NoFrustumCulling,
            Visibility::Hidden,
            StarDraw(groups.len()),
            Name::new(format!("sky stars tex {t}")),
        ));
        groups.push((t, sprites, buffer));
    }
    println!(
        "sky stars: level {:02}, {} records ({} twinkle, {} fixed, {} moving), drawn before shell {} (sky order slot {}), \
         textures {:?}, scratch {} bytes ({} non-zero){}",
        ls.level, info.count, info.twinkle, info.fixed, info.count - info.twinkle - info.fixed, info.before_shell, slot, texs,
        ls.sprite_scratch.len(), nonzero,
        if matches!(ls.level, 6 | 17) { format!(", overlay tables {tables:x?}") } else { String::new() }
    );
    let mut fallback_rng = Rng::new();
    fallback_rng.srand(LEVEL_SEED);
    let stats = std::env::var("RC_STAR_STATS").is_ok_and(|v| v.trim() == "1");
    Some(StarSim { stars: SkyStars::new(ls.level, tables, initial), fallback_rng, frames: 0, groups, drawn: 0, stats })
}

/// One star frame per game tick, after the particle tick (same `rand` stream).
pub fn star_frame(ctx: &mut Ctx) {
    let Some(mut sim) = ctx.try_res_mut::<StarSim>() else { return };
    let mut play = ctx.try_res_mut::<crate::gameplay::Play>();
    let mut part = ctx.try_res_mut::<crate::particle_render::ParticleSim>();
    let sim = &mut *sim;
    let rng = match (play.as_deref_mut(), part.as_deref_mut()) {
        (Some(p), _) => &mut p.game.rng,
        (None, Some(p)) => &mut p.rng,
        (None, None) => &mut sim.fallback_rng,
    };
    let before = rng.state;
    sim.stars.frame(rng);
    let after = rng.state;
    sim.frames += 1;
    if sim.frames == 1 || (sim.stats && sim.frames % 60 == 0) {
        let s = &sim.stars.stars;
        let h = s.iter().fold(0u64, |h, r| r.0.iter().fold(h, |h, &b| h.wrapping_mul(0x100_0000_01b3).wrapping_add(b as u64)));
        println!(
            "sky stars: frame {} rng {before:#010x} -> {after:#010x}, records hash {h:016x}, star 0 rgba {:08x}, drawn {} (last frame)",
            sim.frames, s.first().map_or(0, |r| r.rgba()), sim.drawn
        );
    }
}

/// `SkySpriteProc` on the CPU: cull and write this frame's sprites per texture. (rc-metal: the main camera, `FlyCam`;
/// rc-engine took the first non-sky `Camera3d`, which the menu and canvas cameras shadow with the main transform.)
pub fn build_meshes(ctx: &mut Ctx) {
    let Some(mut sim) = ctx.try_res_mut::<StarSim>() else { return };
    let cam = ctx
        .world
        .query::<(&Transform, Option<&Projection>)>()
        .with::<&crate::fly_cam::FlyCam>()
        .without::<&SkyCamera>()
        .iter()
        .next()
        .map(|(t, p)| (*t, p.cloned()));
    let Some((t, proj)) = cam else { return };
    let mut buffers = ctx.res_mut::<Assets<ShaderBuffer>>();
    let [fwd, left, up] = game_rows(&t);
    // The view's tan_x / tan_y (0x16d0a0 / 0x16cf70 as `UpdateViewContext` sets them): the camera's real projection.
    let (tx, ty) = crate::particle_render::view_tans(proj.as_ref());
    let (sec_x, sec_y) = ((1.0 + tx * tx).sqrt(), (1.0 + ty * ty).sqrt());

    let StarSim { stars, groups, drawn: drawn_out, .. } = &mut *sim;
    for g in groups.iter_mut() { g.1.clear(); }
    let mut drawn = 0;
    for r in &stars.stars {
        if r.texture() < 0 { continue; }
        let Some(g) = groups.iter().position(|(t, _, _)| *t as i8 == r.texture()) else { continue };
        let p = Vec3::from_array(r.position_f32());
        let c = Vec3::new(-left.dot(p), -up.dot(p), fwd.dot(p));
        let size = r.size_f32();
        let vx = tx * c.z - (c.x.abs() - size * sec_x);
        let vy = ty * c.z - (c.y.abs() - size * sec_y);
        // `bltz` on both lanes: the sign bit (−0.0 counts as negative).
        if vx.is_sign_negative() || vy.is_sign_negative() { continue; }
        drawn += 1;
        let q = size * SIZE_TO_PIXELS / c.z;
        let (sin, cos) = vu0_sin_cos(r.rotation());
        let (s, co) = (f32::from_bits(sin) * q, f32::from_bits(cos) * q);
        let a = [co, 1.0625 * s];
        let b = [-s, 1.0625 * co];
        let dir = game_to_bevy(r.position_f32()).to_array();
        let tag = (r.alpha_reg() == 0x48) as u32;
        // Corners S+a, S+b, S−b, S−a (quad_mesh).
        groups[g].1.push(dir, a, b, r.rgba(), tag);
    }
    *drawn_out = drawn;
    for (v, d) in ctx.world.query::<(&mut Visibility, &StarDraw)>().iter() {
        // A draw of another star simulation (the level's under the title world's, crate::title_world) past this one's
        // groups is left alone (it is on a layer no camera shows meanwhile).
        let Some(g) = groups.get(d.0) else { continue };
        let vis = if g.1.len() == 0 { Visibility::Hidden } else { Visibility::Inherited };
        if *v != vis { *v = vis; }
    }
    for (_, sprites, h) in groups.iter_mut() {
        if sprites.len() == 0 { continue; }
        crate::asset_write::set_buffer(&mut buffers, &*h, &sprites.contents());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two sections; the lookup honours section bounds, the type and the entry-point termination rule.
    #[test]
    fn overlay_sections() {
        let mut ov = Vec::new();
        for (dest, data, ty, ep) in [(0x1000u32, vec![1u8, 2, 3, 4], 1u32, 0x9000u32), (0x2000, vec![5, 6, 7, 8], 8, 0x9000), (0x3000, vec![9; 4], 1, 0x1234)] {
            for w in [dest, data.len() as u32, ty, ep] { ov.extend(w.to_le_bytes()); }
            ov.extend(data);
        }
        assert_eq!(overlay_read(&ov, 0x1001, 2), Some(vec![2, 3]));
        assert_eq!(overlay_read(&ov, 0x1002, 4), None);
        assert_eq!(overlay_read(&ov, 0x2000, 4), None, "NOBITS section");
        assert_eq!(overlay_read(&ov, 0x3000, 4), None, "past the entry-point change");
    }
}
