//! The Tesla Claw's beam: the draw callback `0x2d05d8` its update registers every tick it fires
//! (`rc_game::hero::tesla`, `Tesla::drawn`): the main chain's and the second chain's two strips each, the four arcs and
//! the claw's glow ([`rc_game::hero::tesla::beam_quads`], `FastDrawQuadReal` quads computed against the camera, FX
//! textures 14 / 16 / 0xb, ALPHA 0x48 additive, Z tested and not written), through the shared draw-callback material of
//! crate::fx_draw, in the list-1 band after the reticles. A module of its own (the beam is the Tesla Claw's alone).

use crate::fx_draw::{FxAssets, FxGroup, FxPrimMaterial, FxSlots, PrimBuf, LIST1_BIAS};
use crate::game_camera::{GameFog, TfragFog};
use crate::prelude::*;

/// After the reticles' slots.
const BAND: f32 = LIST1_BIAS + 64.0;

/// Bevy's plugin: the state. Systems (central schedule): LevelUnload `crate::level_switch::reset::<TeslaDraw>`,
/// PostUpdate [`draw`] (before Bevy's asset events: any point before the render here).
pub fn plugin(app: &mut crate::engine::app::App, _r: &mut crate::render::frame::Renderer) { app.ctx.res.init::<TeslaDraw>(); }

#[derive(Default)]
pub struct TeslaDraw {
    slots: FxSlots,
    drawn: Option<u64>,
}

pub fn draw(ctx: &mut Ctx) {
    let res = &ctx.res;
    let mut state = res.get_mut::<TeslaDraw>();
    let play = res.try_get::<crate::gameplay::Play>();
    let fog = res.try_get::<GameFog>();
    let level = res.get::<crate::Level>();
    let counter = play.as_deref().map(|p| p.game.counter);
    if counter == state.drawn && counter.is_some() { return; }
    state.drawn = counter;
    let mut groups: Vec<FxGroup> = Vec::new();
    if let Some(p) = play.as_deref().filter(|p| p.svc.game_mode != 2) {
        let t = &p.game.hero.weapons.tesla;
        if t.drawn == Some(p.game.counter.wrapping_sub(1)) && p.game.hero.items.slot.id == rc_game::hero::tesla::TESLA {
            let eye = p.game.camera.out.pos_f32();
            let g = rc_game::hero::physics::to_f32x3(p.game.hero.gravity_dir);
            for q in rc_game::hero::tesla::beam_quads(t, eye, g) {
                // One group per texture, in the callback's order.
                if groups.last().is_none_or(|l| l.fx != q.fx) { groups.push(FxGroup { fx: q.fx, additive: true, subtract: false, prims: PrimBuf::default() }); }
                groups.last_mut().unwrap().prims.quad(q.corners, q.st, q.rgba);
            }
        }
    }
    let fog = fog.map(|f| f.uniform).unwrap_or_else(|| TfragFog::new(&level.0.fog));
    let tex = level.0.particles.textures.as_ref().map(|t| t.fx_textures.as_slice());
    let (mut meshes, mut images, mut materials) = (res.get_mut::<Assets<Mesh>>(), res.get_mut::<Assets<Image>>(), res.get_mut::<Assets<FxPrimMaterial>>());
    let mut a = FxAssets { meshes: &mut meshes, images: &mut images, materials: &mut materials, fx: tex, fog };
    state.slots.show(&mut ctx.world, &mut a, groups, BAND, "tesla");
}
