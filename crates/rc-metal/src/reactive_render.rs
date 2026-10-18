//! The Suck Cannon's vortex, the Taunter's sound-wave rings and the Morph-o-Ray's beam: their draw callbacks
//! (`0x306158`, draw list 2; `0x2cd1d0`, list 1; `0x2d38b8`, list 2 (`RegisterDrawCallback2`)) as `FastDrawQuadReal`
//! quads through the shared draw-callback material of crate::fx_draw (additive, ALPHA 0x48). The quads come from rc-game
//! (`rc_game::hero::suck_vortex::Vortex::quads`, `rc_game::hero::taunter::Rings::quads`,
//! `rc_game::hero::morph_ray::Beam::quads`: one group per FX texture, in the callback's order); they are drawn on the
//! ticks their updates registered the callback.

use crate::fx_draw::{FxAssets, FxGroup, FxPrimMaterial, FxSlots, PrimBuf, LIST1_BIAS, LIST2_BIAS};
use crate::game_camera::{GameFog, TfragFog};
use crate::prelude::*;

/// Bevy's plugin: the state. Systems (central schedule): LevelUnload `crate::level_switch::reset::<ReactiveDraw>`,
/// PostUpdate [`draw`] (before Bevy's asset events: any point before the render here).
pub fn plugin(app: &mut crate::engine::app::App, _r: &mut crate::render::frame::Renderer) { app.ctx.res.init::<ReactiveDraw>(); }

#[derive(Default)]
pub struct ReactiveDraw {
    vortex: FxSlots,
    rings: FxSlots,
    morph: FxSlots,
    drawn: Option<u64>,
}

pub fn draw(ctx: &mut Ctx) {
    let res = &ctx.res;
    let mut state = res.get_mut::<ReactiveDraw>();
    let play = res.try_get::<crate::gameplay::Play>();
    let fog = res.try_get::<GameFog>();
    let level = res.get::<crate::Level>();
    let counter = play.as_deref().map(|p| p.game.counter);
    if counter == state.drawn && counter.is_some() { return; }
    state.drawn = counter;
    let mut vortex: Vec<FxGroup> = Vec::new();
    let mut rings: Vec<FxGroup> = Vec::new();
    let mut morph: Vec<FxGroup> = Vec::new();
    if let Some(p) = play.as_deref().filter(|p| p.svc.game_mode != 2) {
        let last = p.game.counter.wrapping_sub(1);
        let r = &p.game.hero.weapons.reactive;
        let id = p.game.hero.items.slot.id;
        if id == rc_game::hero::suck_cannon::SUCK_CANNON && r.suck.vortex.drawn == Some(last) {
            let mut g = FxGroup { fx: rc_game::hero::suck_vortex::FX, additive: true, subtract: false, prims: PrimBuf::default() };
            for q in r.suck.vortex.quads(p.game.camera.out.pos_f32()) { g.prims.quad(q.corners, q.st, q.rgba); }
            vortex.push(g);
        }
        if id == rc_game::hero::taunter::TAUNTER && r.taunter.rings.drawn == Some(last) {
            let mut g = FxGroup { fx: rc_game::hero::taunter::RING_FX, additive: true, subtract: false, prims: PrimBuf::default() };
            for (c, st, rgba) in r.taunter.rings.quads() { g.prims.quad(c, st, rgba); }
            rings.push(g);
        }
        if id == rc_game::hero::morph_ray::MORPH && r.morph.beam.drawn == Some(last) {
            for (fx, c, st, rgba) in r.morph.beam.quads(p.game.camera.out.pos_f32()) {
                if morph.last().is_none_or(|g| g.fx != fx) { morph.push(FxGroup { fx, additive: true, subtract: false, prims: PrimBuf::default() }); }
                morph.last_mut().unwrap().prims.quad(c, st, rgba);
            }
        }
    }
    let fog = fog.map(|f| f.uniform).unwrap_or_else(|| TfragFog::new(&level.0.fog));
    let tex = level.0.particles.textures.as_ref().map(|t| t.fx_textures.as_slice());
    let (mut meshes, mut images, mut materials) = (res.get_mut::<Assets<Mesh>>(), res.get_mut::<Assets<Image>>(), res.get_mut::<Assets<FxPrimMaterial>>());
    let mut a = FxAssets { meshes: &mut meshes, images: &mut images, materials: &mut materials, fx: tex, fog };
    state.vortex.show(&mut ctx.world, &mut a, vortex, LIST2_BIAS + 64.0, "suck vortex");
    state.rings.show(&mut ctx.world, &mut a, rings, LIST1_BIAS + 128.0, "taunter rings");
    state.morph.show(&mut ctx.world, &mut a, morph, LIST2_BIAS + 96.0, "morph beam");
}
