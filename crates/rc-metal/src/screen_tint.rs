//! **Full-screen tints from class draw callbacks** (G-REN-030): `rc_game::moby_update::classes::draw_callbacks::
//! screen_tints` (Hoven's turret 1267's red screen, Quartu / the Fleet's alarm 408, Giant Clank's beam flash) as one
//! full-screen GS-blend pass on the main camera ([`crate::gs_post`]): each tint in order, `Cv = ((A − B)·As >> 7) + D` on
//! the display bytes with A / B / D from the low byte of its ALPHA_1 (0x44: Cs, Cd, Cd; 0x42: 0, Cs, Cd),
//! `assets/shaders/screen_tint.wgsl`.
//!
//! [L] The game draws them inside the world pass (the list-2 callbacks after the ties, the beam's sprite after the
//! shrubs), so the mobys, particles and effects drawn later are not tinted; the pass runs after the 3-D passes and before
//! the HUD, so they are.

use crate::prelude::*;

use crate::fly_cam::FlyCam;

/// The frame's tints (at most four; GS RGBA bytes, A = 0x80 → 1.0) and their ALPHA_1 low bytes.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct ScreenTint {
    pub rgba: [UVec4; 4],
    pub alpha: UVec4,
    pub count: UVec4,
}

impl ScreenTint {
    /// The `ScreenTint` uniform (`assets/shaders/screen_tint.wgsl`).
    pub fn bytes(&self) -> Vec<u8> {
        let mut s = Std140::default();
        for v in self.rgba { s.uvec4(v); }
        s.uvec4(self.alpha).uvec4(self.count);
        s.finish()
    }
}

/// After the 3-D passes, before the HUD (main.rs registers it before the AA blit).
pub fn screen_tint_fullscreen_pass() -> FullscreenPass {
    FullscreenPass {
        shader: "screen_tint",
        entry: "fragment",
        defs: Default::default(),
        uniform: |w, e| w.get::<&ScreenTint>(e).ok().map(|c| c.bytes()),
        blend: None,
        after_ui: false,
    }
}

pub fn plugin(_app: &mut crate::engine::app::App, _r: &mut crate::render::frame::Renderer) {}

pub fn update(ctx: &mut Ctx) {
    let Some((entity, have)) = ctx.world.query::<(Entity, &FlyCam, Option<&ScreenTint>)>().iter().map(|(e, _, t)| (e, t.copied())).next() else { return };
    let tints = ctx.try_res::<crate::gameplay::Play>().map(|p| rc_game::moby_update::classes::draw_callbacks::screen_tints(&p.game.mobys, &p.svc, p.game.counter)).unwrap_or_default();
    let want = (!tints.is_empty()).then(|| {
        let mut t = ScreenTint::default();
        for (k, s) in tints.iter().take(4).enumerate() {
            t.rgba[k] = UVec4::new(s.rgba & 0xff, (s.rgba >> 8) & 0xff, (s.rgba >> 16) & 0xff, s.rgba >> 24);
            t.alpha[k] = s.alpha as u32;
        }
        t.count.x = tints.len().min(4) as u32;
        t
    });
    match (want, have) {
        (Some(w), Some(h)) if h == w => {}
        (Some(w), _) => { let _ = ctx.world.insert_one(entity, w); }
        (None, Some(_)) => { let _ = ctx.world.remove_one::<ScreenTint>(entity); }
        (None, None) => {}
    }
}
