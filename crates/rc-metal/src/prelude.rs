//! What rc-engine's modules got from `bevy::prelude::*`, from the Bevy-free runtime (crate::engine, crate::render).

pub use crate::engine::app::{run_schedule, AppExit, Stage, UpdateCount, Window};
pub use crate::engine::assets::{AssetId, Assets, Handle};
pub use crate::engine::ecs::{despawn_recursive, Commands, Ctx, Entity, Resources, World};
pub use crate::engine::image::{
    Extent3d, Image, ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor, TextureDimension, TextureFormat, TextureUsages,
};
pub use crate::engine::input::{AccumulatedMouseMotion, AccumulatedMouseScroll, ButtonInput, Gamepad, GamepadButton, InputState, KeyCode, MouseButton};
pub use crate::engine::mesh::{
    Indices, Mesh, Mesh2d, Mesh3d, MeshLayout, MeshVertexAttribute, PrimitiveTopology, RenderAssetUsages, VertexAttributeValues, VertexFormat,
};
pub use crate::engine::scene::{
    Camera, Camera2d, Camera3d, CameraProjection, ChildOf, ClearColor, ClearColorConfig, Color, CustomProjection, GlobalTransform,
    InheritedVisibility, Msaa, Name, NoFrustumCulling, Projection, RenderLayers, RenderTarget, SubCameraView, Transform, ViewVisibility, Viewport,
    Visibility,
};
pub use crate::engine::time::{Clock, Steps, Time};
pub use crate::render::frame::{
    BeforeShadows, Camera3dDepth, DisplayEffect, FullscreenPass, FullscreenPasses, Hook, HookCtx, IsDefaultUiCamera, Material2d, OutputSkip,
    Sprite, WorldDrawOrder,
};
pub use crate::render::gfx::{Blend, BlendComponent, BlendFactor, BlendOperation, CompareFunction, ShaderBuffer};
pub use crate::render::material::{AlphaMode, BindGroup, Face, Material, MeshMaterial2d, MeshMaterial3d, MeshTag, Phase, PipelineDesc, Std140};
pub use crate::render::ui::{
    BackgroundColor, GlobalZIndex, ImageNode, MaterialNode, Node, PositionType, Rect, UiMaterial, UiMaterials, UiTargetCamera, Val, ZIndex,
};
pub use glam::{Affine3A, IVec2, Mat3, Mat4, Quat, UVec2, UVec3, UVec4, Vec2, Vec3, Vec3A, Vec4, Vec4Swizzles};

/// Bevy's `default()`.
pub fn default<T: Default>() -> T { T::default() }

/// Bevy's `FloatOrd`.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct FloatOrd(pub f32);

impl Eq for FloatOrd {}

impl Ord for FloatOrd {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering { self.0.total_cmp(&o.0) }
}
pub use crate::render::standard::StandardMaterial;

pub use glam::EulerRot;

/// Bevy's `Dir3::as_vec3` (rc-metal's directions are plain `Vec3`s).
pub trait AsVec3 {
    fn as_vec3(self) -> Vec3;
}

impl AsVec3 for Vec3 {
    fn as_vec3(self) -> Vec3 { self }
}
