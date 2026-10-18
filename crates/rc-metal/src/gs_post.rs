//! Full-screen GS-blend passes on the main camera (a read of the frame, integer blending on the display bytes, the
//! result written back): the port's own copy of Bevy's `FullscreenMaterialPlugin`, keyed by the pass type.
//!
//! **Why a copy.** Bevy 0.19's plugin keeps each view's pipeline in one component shared by every material type
//! (`FullscreenMaterialPipelineId`, not generic): the prepare system of a type the camera does not carry removes it,
//! so with two types registered only the one whose prepare runs last draws. The scene fade (`SceneFade`) stays on
//! Bevy's plugin (its only user); the passes here each keep their own pipeline ([`PassPipeline<T>`]). The pass types
//! still implement Bevy's [`FullscreenMaterial`] (the shader and the ordering).
//!
//! Users: the Visibomb's missile view overlay (crate::visibomb_view), the underwater tint (crate::fog_state).
//!
//! rc-metal: a pass is a [`FullscreenPass`] registered with the renderer (crate::render::frame), present on a camera
//! while the camera carries the pass's component; [`uniform`] gives a component's bytes.

use crate::prelude::*;

/// A pass component: its uniform bytes (WGSL uniform layout, binding 2 of the pass).
pub trait PostUniform: Send + Sync + 'static {
    fn bytes(&self) -> Vec<u8>;
}

/// The uniform of pass component `T` on camera `e`, when the camera carries it (a [`FullscreenPass::uniform`]).
pub fn uniform<T: PostUniform>(world: &World, e: Entity) -> Option<Vec<u8>> { world.get::<&T>(e).ok().map(|t| t.bytes()) }
