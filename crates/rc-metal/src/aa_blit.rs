//! **Anti-aliasing Original** (crate::graphics): the game's own softening of the picture, two full-screen bilinear
//! copies, drawn at the frame's resolution in game-pixel units (`assets/shaders/aa_blit.wgsl`).
//!
//! The game (level01 addresses, level07's in brackets; `SetupFS_AA_buffer__Fiiiiii` 0x2224f8 [0x228450] builds both
//! packets, NTSC: draw 512×416, display 512×448):
//! * **A, the AA blit** (`DrawWorld` 0x21a1b8 [0x2203a0] → `PutAABlitPacket_A` 0x223200 [0x229158], packet 0x151900,
//!   38 qwords; its toggle 0x16a0b8 is always 1: [0x21b420] sets every render toggle): after the particles, before the HUD, the draw buffer is copied onto itself as 16
//!   sprites 32 pixels wide (PRIM 0x116: sprite, textured, UV; no blending; TEST_1 0x30000; CLAMP_1 5 = clamp;
//!   TEX1_1 0x100000261 = bilinear, level 0; TEX0 = the draw buffer, DECAL). X runs 0..512 with U = X (texel units):
//!   pixel x reads texels x − 1 and x half each. Y runs 0..415 with V 0..416: row y reads at V = y·416/415, and row 415
//!   is not drawn.
//! * **B, the display copy** (the frame loop [0x26d280] → `PutAABlitPacket_B` 0x223260 [0x2291b8] between `PutDrawBufferSmall` and `PutDrawBufferLarge`,
//!   packet 0x151b60, 41 qwords): the finished draw buffer, HUD included, copied to the 512×448 display buffer the same
//!   way: U = X again, and Y −0.5..447.5 with V 0..416 (the 416 lines stretched to 448, centred).
//!
//! On a TV both soften the picture: about half a pixel across, twice, and the vertical resampling. Here each copy is
//! drawn at the frame's resolution: every frame pixel reads the box of one game pixel a GS bilinear tap is, at the place
//! the copy maps it to (`assets/shaders/aa_blit.wgsl`), so at 416 lines the vertical is the GS's result exactly. B's
//! vertical stretch is the frame's own scale (416 game lines → the frame's height): its box is centred. Each copy is
//! separable: one draw across, one down, A's then B's ([`BlitAcross`], [`BlitDown`], [`CopyAcross`], [`CopyDown`]), after the 3D passes and the screen tints and before
//! the UI pass (rc-metal: full-screen passes registered in that order by main.rs, [`blit_across_fullscreen_pass`] …). The HUD, drawn between the two on the PS2, gets B only (crate::hud_render, the HUD option).
//!
//! [L] The blit reads the buffer it writes, a strip at a time; whether a strip's left texel comes from the GS texture
//! cache before or after the strip to its left was written is not modelled (the unfiltered frame is read throughout).
//! The interlace offsets (XYOFFSET) are not part of it: on a progressive screen they would only show as jitter.

use crate::prelude::*;

use crate::fly_cam::FlyCam;
use crate::graphics::{AntiAliasing, GraphicsSettings};

macro_rules! aa_copy {
    ($(#[$doc:meta])* $name:ident { k: $k:expr, a: $a:expr, last: $last:expr, down: $down:expr }, $pass:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Debug)]
        pub struct $name {
            /// x = k, y = a, z = last (the shader's names).
            pub params: Vec4,
            /// x = 0 across, 1 down.
            pub axis: UVec4,
        }

        impl Default for $name {
            fn default() -> Self { $name { params: Vec4::new($k, $a, $last, 0.0), axis: UVec4::new($down as u32, 0, 0, 0) } }
        }

        impl $name {
            /// The `AaCopy` uniform (`assets/shaders/aa_blit.wgsl`).
            pub fn bytes(&self) -> Vec<u8> {
                let mut s = Std140::default();
                s.vec4(self.params).uvec4(self.axis);
                s.finish()
            }
        }

        /// The pass while the main camera carries the component (before the UI pass, in A across → A down → B across →
        /// B down order: main.rs registers them so, after the mirror and the screen tints).
        pub fn $pass() -> FullscreenPass {
            FullscreenPass {
                shader: "aa_blit",
                entry: "fragment",
                defs: Default::default(),
                uniform: |w, e| w.get::<&$name>(e).ok().map(|c| c.bytes()),
                blend: None,
                after_ui: false,
            }
        }
    };
}

// Each draw: frame pixel centre t (game units) reads the box of one game pixel at s = (t − a)·k; pixels at t ≥ last
// are not drawn.
aa_copy!(
    /// A across: U = X, pixel x reads at x (its left edge).
    BlitAcross { k: 1.0, a: 0.5, last: f32::MAX, down: false },
    blit_across_fullscreen_pass
);
aa_copy!(
    /// A down: V 0..416 over rows 0..415, row y reads at y·416/415; row 415 is not drawn.
    BlitDown { k: 416.0 / 415.0, a: 0.5, last: 415.0, down: true },
    blit_down_fullscreen_pass
);
aa_copy!(
    /// B across: U = X again.
    CopyAcross { k: 1.0, a: 0.5, last: f32::MAX, down: false },
    copy_across_fullscreen_pass
);
aa_copy!(
    /// B down: the 416 lines over the frame's height, centred.
    CopyDown { k: 1.0, a: 0.0, last: f32::MAX, down: true },
    copy_down_fullscreen_pass
);

/// Nothing to insert: the four passes are registered by main.rs ([`blit_across_fullscreen_pass`] …).
pub fn plugin(_app: &mut crate::engine::app::App, _r: &mut crate::render::frame::Renderer) {}

/// The four draws on the main camera while the option is Original.
pub fn follow_option(ctx: &mut Ctx) {
    let on = ctx.res::<GraphicsSettings>().aa == AntiAliasing::Original;
    let cams: Vec<(Entity, bool)> = ctx
        .world
        .query::<(Entity, &FlyCam, Option<&BlitAcross>)>()
        .iter()
        .map(|(e, _, b)| (e, b.is_some()))
        .collect();
    for (e, have) in cams {
        match (on, have) {
            (true, false) => { let _ = ctx.world.insert(e, (BlitAcross::default(), BlitDown::default(), CopyAcross::default(), CopyDown::default())); }
            (false, true) => { let _ = ctx.world.remove::<(BlitAcross, BlitDown, CopyAcross, CopyDown)>(e); }
            _ => {}
        }
    }
}
