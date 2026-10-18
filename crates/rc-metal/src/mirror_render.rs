//! The mirror cheat 0x15edb4 ("Levels are mirrored", `rc_game::cheats`) on the screen: the game mirrors its camera's left
//! row (`CameraUpdate` 0x20eca8 for the follow camera, `0x2ac8d8` for the scene camera, `0x2a4080` for the ship), so the
//! world is drawn as its mirror image while the 2D layer (the HUD, the menus, the subtitles, the movies) is not. The
//! port's cameras ignore the left row (a transform has no reflection), so the world's frame is mirrored left to
//! right by a full-screen pass after the 3D passes and before the UI pass ([`MirrorFlip`],
//! `assets/shaders/mirror.wgsl`). The pad's left / right swap (`ProcessPadInput`), the follow camera's rows
//! (`rc_game::follow_camera`, the sound's listener) and the move classification of the cheat entry read the byte in the
//! game code; this module only draws.
//!
//! The pass mirrors the whole main target about its centre: the game frame's letterboxed viewport is centred, so the
//! picture inside it is mirrored about its own centre [L: an off-centre viewport would shift].

use crate::fly_cam::FlyCam;
use crate::gameplay::Persistent;
use crate::prelude::*;

/// The mirror pass on the main camera (present only while the cheat is on).
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct MirrorFlip {
    pub on: u32,
}

impl MirrorFlip {
    /// The `MirrorFlip` uniform (`assets/shaders/mirror.wgsl`).
    pub fn bytes(&self) -> Vec<u8> {
        let mut s = Std140::default();
        s.u32(self.on);
        s.finish()
    }
}

/// After the 3D passes, before the UI pass (the HUD and the menus stay as they are).
pub fn mirror_fullscreen_pass() -> FullscreenPass {
    FullscreenPass {
        shader: "mirror",
        entry: "fragment",
        defs: Default::default(),
        uniform: |w, e| w.get::<&MirrorFlip>(e).ok().map(|c| c.bytes()),
        blend: None,
        after_ui: false,
    }
}

pub fn plugin(_app: &mut crate::engine::app::App, _r: &mut crate::render::frame::Renderer) {}

/// The pass on the main camera while 0x15edb4 is set.
pub fn follow_cheat(ctx: &mut Ctx) {
    let on = ctx.try_res::<Persistent>().is_some_and(|s| s.0.global.cheats_active[rc_game::cheats::slot::MIRROR] != 0);
    let cams: Vec<(Entity, bool)> = ctx.world.query::<(Entity, &FlyCam, Option<&MirrorFlip>)>().iter().map(|(e, _, m)| (e, m.is_some())).collect();
    for (e, have) in cams {
        match (on, have) {
            (true, false) => { let _ = ctx.world.insert_one(e, MirrorFlip { on: 1 }); }
            (false, true) => { let _ = ctx.world.remove_one::<MirrorFlip>(e); }
            _ => {}
        }
    }
}
