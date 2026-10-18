//! What rc-engine's modules got from `bevy::prelude::*`, from the Bevy-free runtime (crate::engine, crate::render).

pub use crate::engine::app::{run_schedule, AppExit, Stage, Window};
pub use crate::engine::assets::{AssetId, Assets, Handle};
pub use crate::engine::ecs::{despawn_recursive, Commands, Ctx, Entity, Resources, World};
pub use crate::engine::image::{
    Extent3d, Image, ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor, TextureDimension, TextureFormat, TextureUsages,
};
pub use crate::engine::input::{AccumulatedMouseMotion, ButtonInput, Gamepad, GamepadButton, InputState, KeyCode, MouseButton};
pub use crate::engine::mesh::{
    Indices, Mesh, Mesh2d, Mesh3d, MeshLayout, MeshVertexAttribute, PrimitiveTopology, RenderAssetUsages, VertexAttributeValues, VertexFormat,
};
pub use crate::engine::scene::{
    Camera, Camera2d, Camera3d, CameraProjection, ChildOf, ClearColor, ClearColorConfig, Color,
    InheritedVisibility, Msaa, Name, NoFrustumCulling, Projection, RenderLayers, RenderTarget, SubCameraView, Transform, Viewport,
    Visibility,
};
pub use crate::engine::time::{Steps, Time};
pub use crate::render::frame::{
    Camera3dDepth, DisplayEffect, FullscreenPass, Hook, HookCtx, IsDefaultUiCamera, Material2d, OutputSkip,
    Sprite, WorldDrawOrder,
};
pub use crate::render::gfx::{Blend, BlendComponent, BlendFactor, BlendOperation, CompareFunction, ShaderBuffer};
pub use crate::render::material::{AlphaMode, BindGroup, Material, MeshMaterial2d, MeshMaterial3d, MeshTag, PipelineDesc, Std140};
pub use crate::render::ui::{
    BackgroundColor, GlobalZIndex, ImageNode, MaterialNode, Node, PositionType, Rect, UiMaterial, UiTargetCamera, Val, ZIndex,
};
pub use glam::{Mat3, Mat4, Quat, UVec2, UVec4, Vec2, Vec3, Vec4};

/// Bevy's `default()`.
pub fn default<T: Default>() -> T { T::default() }

/// Bevy's `FloatOrd`.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FloatOrd(pub f32);

impl PartialOrd for FloatOrd {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> { Some(self.cmp(o)) }
}

impl Eq for FloatOrd {}

impl Ord for FloatOrd {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering { self.0.total_cmp(&o.0) }
}
pub use crate::render::standard::StandardMaterial;

pub use glam::EulerRot;

/// Bevy's `Dir3::as_vec3` (rc-metal's directions are plain `Vec3`s).
#[allow(clippy::wrong_self_convention)] // Bevy's name
pub trait AsVec3 {
    fn as_vec3(self) -> Vec3;
}

impl AsVec3 for Vec3 {
    fn as_vec3(self) -> Vec3 { self }
}
