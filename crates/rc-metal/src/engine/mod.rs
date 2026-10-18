//! The runtime rc-engine's code runs on in place of Bevy: ECS (hecs), assets, meshes, images, scene components, input
//! and clocks.

// Bevy's API surface as rc-engine's code uses it, kept whole for later ports (docs/plan/metal_porting.md): not every
// method or variant has a caller yet.
#![allow(dead_code)]

pub mod app;
pub mod assets;
pub mod ecs;
pub mod image;
pub mod input;
pub mod mesh;
pub mod scene;
pub mod time;
