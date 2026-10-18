//! The Metal renderer.

// Bevy's API surface as rc-engine's code uses it, kept whole for later ports (docs/plan/metal_porting.md): not every
// method or variant has a caller yet.
#![allow(dead_code)]

pub mod frame;
pub mod gfx;
pub mod material;
pub mod standard;
pub mod ui;
