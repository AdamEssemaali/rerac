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
//! while the camera carries the pass's component (its `uniform` gives the component's bytes).

