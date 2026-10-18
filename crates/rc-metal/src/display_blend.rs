//! PS2-style blending of the effects: additive and translucent effects blend on the frame's **display bytes**, as
//! the GS does, not in linear light (user decision 2026-09-27; docs/plan/hardware_fidelity_layers.md "Result-level
//! reproductions"). One mechanism for every effect: the particles (crate::particle_render), the effect mobys
//! (`MobyBlend::Translucent` / `Additive` → `GsPass::EffectTested` + `EffectLowAlpha` / `GsPass::AdditiveNoZ`) and the draw-callback
//! effects (crate::fx_draw; the fire fields of crate::water_render). Each effect entity carries [`DisplayEffect`].
//!
//! **Why.** The GS equations are `Cd + (Cs·As >> 7)` (ALPHA 0x48) and `Cd + ((Cs − Cd)·As >> 7)` (0x44) on the
//! frame buffer's display-encoded bytes, clamped to 0..255. The port's frame is an sRGB target whose bytes are those
//! display bytes (every world shader outputs `srgb_to_linear(GS byte)`), but the GPU blends it in linear light: a dim
//! additive glow adds next to nothing over a lit scene (+25 display levels of a type-23 puff become +3 over grass),
//! stacked glows never reach the white the GS clamps them to, and a translucent shell looks lighter over dark ground.
//!
//! (rc-metal: the render-world half, the split of the effect items and the effect pass, is crate::render::frame.)
//!
//! **How (a render target choice).** After Bevy's transparent pass (the world, water and every other blended draw,
//! in linear light as before), the effects are drawn into a display-encoded copy of the frame and copied back:
//! 1. the effect items ([`DisplayEffect`] entities) are taken out of the view's `Transparent3d` phase before the
//!    transparent pass ([`split_effects`]; they keep their sorted order: back to front within the bands the materials
//!    give them, effect mobys, then the list-1 callbacks, the particles, the list-2 callbacks);
//! 2. [`effect_pass`]: the frame is converted into an `Rgba8Unorm` texture that stores the display bytes themselves
//!    (`effect_blit.wgsl` `to_display`, per pixel: the sRGB decode and re-encode round-trips every byte), the effect
//!    items are drawn into it with ordinary hardware blending (`One, OneMinusSrcAlpha` on the premultiplied GS terms,
//!    [`PREMULTIPLIED`]) against the view's depth buffer (their Z tests and writes as before), and the result is
//!    converted back into the view's target (`to_linear`, exact per byte);
//! 3. the items are put back ([`restore_effects`]) so Bevy's retained phase is unchanged for the next frame.
//!
//! Blending on a non-sRGB target is the GS's display-byte arithmetic, so overlapping effects stack and saturate as
//! on the PS2, in draw order, and nothing is read back per fragment. The effect materials' pipelines target
//! `Rgba8Unorm` ([`specialize`]); their shaders output display-encoded colour (`display_blend.wgsl`). Remaining
//! differences: the hardware rounds `Cs·As/128` where the GS truncates (±1 level), As > 0x80 clamps to 1 in the 0x44
//! destination factor, and the effects are drawn after all the world's translucent draws (in the GS the effect mobys
//! come before the list-1 water strips).
//!
//! **Cost.** When a view has effect items: two full-frame conversion passes (and the MSAA resolve of the effect
//! target); none otherwise. Measured in docs/plan/particles.md "Display-space blending".

use crate::prelude::*;

pub use crate::render::frame::DisplayEffect;

/// The display-encoded effect target's format (its bytes are the GS frame buffer's).
pub const EFFECT_FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;

/// `One, OneMinusSrcAlpha`: the blend of every effect draw; the shaders output `(Cs·As, As)` for 0x44 and
/// `(Cs·As, 0)` for 0x48, in display units (`display_blend.wgsl` `gs_mix` / `gs_add`).
pub const PREMULTIPLIED: Blend = Blend {
    color: BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::OneMinusSrcAlpha, operation: BlendOperation::Add },
    alpha: BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::OneMinusSrcAlpha, operation: BlendOperation::Add },
};

/// Makes a material pipeline an effect pipeline: the display-encoded target and [`PREMULTIPLIED`]. The entity drawn
/// with it must carry [`DisplayEffect`] (it is drawn only by the effect pass).
pub fn specialize(desc: &mut PipelineDesc) {
    desc.color_format = EFFECT_FORMAT;
    desc.blend = Some(PREMULTIPLIED);
}

/// An effect pipeline's blend made subtractive (after [`specialize`]): `Cd − Cs·As` on the display bytes (the shader
/// outputs `(Cs·As, 0)` as for 0x48), alpha kept; the GS's `(0 − Cs)·FIX + Cd` (ALPHA 0x62), clamped at 0.
pub fn specialize_subtract(desc: &mut PipelineDesc) {
    desc.blend = Some(Blend {
        color: BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::One, operation: BlendOperation::ReverseSubtract },
        alpha: BlendComponent { src_factor: BlendFactor::Zero, dst_factor: BlendFactor::One, operation: BlendOperation::Add },
    });
}
