//! Scene components: transforms, visibility, render layers, names, colours and cameras (the part of Bevy's
//! `Transform` / `Visibility` / `Camera` / `Color` API rc-engine uses). Propagation (parents, inherited visibility) is
//! in crate::engine::app.

use glam::{Affine3A, Mat3, Mat4, Quat, UVec2, Vec3};

/// Bevy's `Transform`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self { Transform::IDENTITY }
}

impl Transform {
    pub const IDENTITY: Transform = Transform { translation: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE };

    pub fn from_xyz(x: f32, y: f32, z: f32) -> Self { Self::from_translation(Vec3::new(x, y, z)) }
    pub fn from_translation(translation: Vec3) -> Self { Transform { translation, ..Self::IDENTITY } }
    pub fn from_rotation(rotation: Quat) -> Self { Transform { rotation, ..Self::IDENTITY } }
    pub fn from_scale(scale: Vec3) -> Self { Transform { scale, ..Self::IDENTITY } }

    pub fn from_matrix(m: Mat4) -> Self {
        let (scale, rotation, translation) = m.to_scale_rotation_translation();
        Transform { translation, rotation, scale }
    }

    pub fn with_translation(mut self, t: Vec3) -> Self { self.translation = t; self }
    pub fn with_rotation(mut self, r: Quat) -> Self { self.rotation = r; self }
    pub fn with_scale(mut self, s: Vec3) -> Self { self.scale = s; self }

    pub fn to_matrix(&self) -> Mat4 { Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation) }
    pub fn compute_matrix(&self) -> Mat4 { self.to_matrix() }
    pub fn compute_affine(&self) -> Affine3A { Affine3A::from_scale_rotation_translation(self.scale, self.rotation, self.translation) }

    /// Bevy's `looking_at`: −Z toward `target`, `up` as close to +Y of the result as possible.
    pub fn looking_at(mut self, target: Vec3, up: Vec3) -> Self {
        self.look_at(target, up);
        self
    }

    pub fn look_at(&mut self, target: Vec3, up: Vec3) { self.look_to(target - self.translation, up) }

    pub fn looking_to(mut self, direction: Vec3, up: Vec3) -> Self {
        self.look_to(direction, up);
        self
    }

    /// Bevy 0.19's `look_to` (fallbacks for a zero or parallel direction / up as Bevy's).
    pub fn look_to(&mut self, direction: Vec3, up: Vec3) {
        let back = -direction.try_normalize().unwrap_or(Vec3::NEG_Z);
        let up = up.try_normalize().unwrap_or(Vec3::Y);
        let right = up.cross(back).try_normalize().unwrap_or_else(|| up.any_orthonormal_vector());
        let up = back.cross(right);
        self.rotation = Quat::from_mat3(&Mat3::from_cols(right, up, back));
    }

    pub fn forward(&self) -> Vec3 { self.rotation * Vec3::NEG_Z }
    pub fn back(&self) -> Vec3 { self.rotation * Vec3::Z }
    pub fn left(&self) -> Vec3 { self.rotation * Vec3::NEG_X }
    pub fn right(&self) -> Vec3 { self.rotation * Vec3::X }
    pub fn up(&self) -> Vec3 { self.rotation * Vec3::Y }
    pub fn down(&self) -> Vec3 { self.rotation * Vec3::NEG_Y }
    pub fn local_x(&self) -> Vec3 { self.right() }
    pub fn local_y(&self) -> Vec3 { self.up() }
    pub fn local_z(&self) -> Vec3 { self.back() }

    pub fn rotate_y(&mut self, angle: f32) { self.rotation = Quat::from_rotation_y(angle) * self.rotation; }
    pub fn rotate_x(&mut self, angle: f32) { self.rotation = Quat::from_rotation_x(angle) * self.rotation; }
    pub fn rotate_local_x(&mut self, angle: f32) { self.rotation *= Quat::from_rotation_x(angle); }
    pub fn transform_point(&self, p: Vec3) -> Vec3 { self.translation + self.rotation * (self.scale * p) }

    pub fn mul_transform(&self, child: Transform) -> Transform {
        Transform {
            translation: self.transform_point(child.translation),
            rotation: self.rotation * child.rotation,
            scale: self.scale * child.scale,
        }
    }
}

/// Bevy's `GlobalTransform`: the world transform after propagation.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct GlobalTransform(pub Affine3A);

impl GlobalTransform {
    pub const IDENTITY: GlobalTransform = GlobalTransform(Affine3A::IDENTITY);
    pub fn translation(&self) -> Vec3 { self.0.translation.into() }
    pub fn affine(&self) -> Affine3A { self.0 }
    pub fn to_matrix(&self) -> Mat4 { Mat4::from(self.0) }
    pub fn compute_matrix(&self) -> Mat4 { self.to_matrix() }
    pub fn compute_transform(&self) -> Transform { Transform::from_matrix(self.to_matrix()) }
    pub fn forward(&self) -> Vec3 { self.0.matrix3.mul_vec3(Vec3::NEG_Z).normalize_or_zero() }
    pub fn mul_transform(&self, t: Transform) -> GlobalTransform { GlobalTransform(self.0 * t.compute_affine()) }
}

impl From<Transform> for GlobalTransform {
    fn from(t: Transform) -> Self { GlobalTransform(t.compute_affine()) }
}

/// Bevy's `Visibility`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Visibility {
    #[default]
    Inherited,
    Hidden,
    Visible,
}

impl Visibility {
    pub fn toggle_inherited_hidden(&mut self) {
        *self = if *self == Visibility::Hidden { Visibility::Inherited } else { Visibility::Hidden };
    }
}

/// Bevy's `InheritedVisibility`: visible after the parents' visibility (crate::engine::app propagates it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct InheritedVisibility(pub bool);

impl InheritedVisibility {
    pub fn get(&self) -> bool { self.0 }
}

/// Bevy's `ViewVisibility` (rc-engine reads it for "drawn last frame"; here: inherited visibility, the renderer does no
/// frustum culling of its own).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ViewVisibility(pub bool);

impl ViewVisibility {
    pub fn get(&self) -> bool { self.0 }
}

/// Bevy's `ChildOf`: the entity's parent (transform and visibility inherit from it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChildOf(pub hecs::Entity);

impl ChildOf {
    pub fn parent(&self) -> hecs::Entity { self.0 }
}

/// Bevy's `Name`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct Name(pub String);

impl Name {
    pub fn new(s: impl Into<String>) -> Self { Name(s.into()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) }
}

/// Bevy's `RenderLayers`: a set of layer bits (an entity without the component is on layer 0).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RenderLayers(pub u64);

impl Default for RenderLayers {
    fn default() -> Self { RenderLayers(1) }
}

impl RenderLayers {
    pub const fn layer(n: usize) -> Self { RenderLayers(1 << n) }
    pub const fn none() -> Self { RenderLayers(0) }
    pub fn with(mut self, n: usize) -> Self { self.0 |= 1 << n; self }
    pub fn intersects(&self, o: &RenderLayers) -> bool { self.0 & o.0 != 0 }
}

/// Bevy's `NoFrustumCulling` (the renderer culls nothing on its own anyway).
#[derive(Clone, Copy, Debug, Default)]
pub struct NoFrustumCulling;

/// Bevy's `Color`, reduced to what rc-engine builds: sRGB-encoded components (alpha linear).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    /// sRGB-encoded red, green, blue (0..1) and alpha.
    pub srgba: [f32; 4],
}

impl Color {
    pub const BLACK: Color = Color { srgba: [0.0, 0.0, 0.0, 1.0] };
    pub const WHITE: Color = Color { srgba: [1.0, 1.0, 1.0, 1.0] };
    pub const NONE: Color = Color { srgba: [0.0, 0.0, 0.0, 0.0] };

    pub fn srgb(r: f32, g: f32, b: f32) -> Self { Color { srgba: [r, g, b, 1.0] } }
    pub fn srgba(r: f32, g: f32, b: f32, a: f32) -> Self { Color { srgba: [r, g, b, a] } }
    pub fn srgb_u8(r: u8, g: u8, b: u8) -> Self { Self::srgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0) }
    pub fn srgba_u8(r: u8, g: u8, b: u8, a: u8) -> Self { Self::srgba(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a as f32 / 255.0) }

    /// Linear RGBA, as a clear of an sRGB target needs it.
    pub fn to_linear(&self) -> [f32; 4] {
        let l = |c: f32| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        [l(self.srgba[0]), l(self.srgba[1]), l(self.srgba[2]), self.srgba[3]]
    }

    pub fn alpha(&self) -> f32 { self.srgba[3] }
}

/// Bevy's `ClearColorConfig`.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum ClearColorConfig {
    /// The `ClearColor` resource.
    #[default]
    Default,
    Custom(Color),
    None,
}

/// Bevy's `ClearColor` resource.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClearColor(pub Color);

/// Bevy's `Msaa` (per camera).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Msaa {
    #[default]
    Off,
    Sample2,
    Sample4,
    Sample8,
}

impl Msaa {
    pub fn samples(self) -> u32 { match self { Msaa::Off => 1, Msaa::Sample2 => 2, Msaa::Sample4 => 4, Msaa::Sample8 => 8 } }
    pub fn from_samples(n: u32) -> Self { match n { 2 => Msaa::Sample2, 4 => Msaa::Sample4, 8 => Msaa::Sample8, _ => Msaa::Off } }
}

/// Where a camera renders: the window (here: the presentation) or an image.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum RenderTarget {
    #[default]
    Window,
    Image(super::assets::Handle<super::image::Image>),
}

/// A sub-rectangle of the target (physical pixels).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Viewport {
    pub physical_position: UVec2,
    pub physical_size: UVec2,
}

/// Bevy's `Camera` (the fields rc-engine sets).
#[derive(Clone, Debug, PartialEq)]
pub struct Camera {
    pub order: isize,
    pub is_active: bool,
    pub clear_color: ClearColorConfig,
    pub target: RenderTarget,
    pub viewport: Option<Viewport>,
}

impl Default for Camera {
    fn default() -> Self { Camera { order: 0, is_active: true, clear_color: ClearColorConfig::Default, target: RenderTarget::Window, viewport: None } }
}

/// Bevy's `Camera3d` marker.
#[derive(Clone, Copy, Debug, Default)]
pub struct Camera3d;

/// Bevy's `Camera2d` marker.
#[derive(Clone, Copy, Debug, Default)]
pub struct Camera2d;

/// rc-engine's cameras use a custom projection (crate::game_camera::GameProjection) or Bevy's defaults.
pub trait CameraProjection: Send + Sync + std::fmt::Debug {
    fn get_clip_from_view(&self) -> Mat4;
    fn far(&self) -> f32;
    fn as_any(&self) -> &dyn std::any::Any;
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
    fn clone_box(&self) -> Box<dyn CameraProjection>;
}

/// Bevy's `Projection`.
#[derive(Debug)]
pub enum Projection {
    /// Bevy's default perspective (vertical fov π/4, near 0.1, infinite reverse Z), for the targets' aspect.
    Perspective { fov: f32, near: f32 },
    /// Bevy's default 2D orthographic projection: one unit per pixel, origin at the centre.
    Orthographic,
    Custom(CustomProjection),
}

impl Clone for Projection {
    fn clone(&self) -> Self {
        match self {
            Projection::Perspective { fov, near } => Projection::Perspective { fov: *fov, near: *near },
            Projection::Orthographic => Projection::Orthographic,
            Projection::Custom(c) => Projection::Custom(CustomProjection(c.0.clone_box())),
        }
    }
}

impl Default for Projection {
    fn default() -> Self { Projection::Perspective { fov: std::f32::consts::FRAC_PI_4, near: 0.1 } }
}

impl Projection {
    pub fn custom(p: impl CameraProjection + 'static) -> Self { Projection::Custom(CustomProjection(Box::new(p))) }

    /// Clip from view for a target of `size` pixels.
    pub fn clip_from_view(&self, size: UVec2) -> Mat4 {
        let aspect = size.x.max(1) as f32 / size.y.max(1) as f32;
        match self {
            Projection::Perspective { fov, near } => Mat4::perspective_infinite_reverse_rh(*fov, aspect, *near),
            Projection::Orthographic => {
                let (w, h) = (size.x.max(1) as f32 * 0.5, size.y.max(1) as f32 * 0.5);
                // Bevy's 2D camera: near −1000, far 1000, reverse Z.
                Mat4::orthographic_rh(-w, w, -h, h, 1000.0, -1000.0)
            }
            Projection::Custom(c) => c.0.get_clip_from_view(),
        }
    }
}

/// A custom projection (Bevy's `CustomProjection`): `get::<T>()` downcasts.
#[derive(Debug)]
pub struct CustomProjection(pub Box<dyn CameraProjection>);

impl CustomProjection {
    pub fn get<T: 'static>(&self) -> Option<&T> { self.0.as_any().downcast_ref() }
    pub fn get_mut<T: 'static>(&mut self) -> Option<&mut T> { self.0.as_any_mut().downcast_mut() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looking_at_points_forward() {
        let t = Transform::from_xyz(0.0, 0.0, 0.0).looking_at(Vec3::new(1.0, 0.0, 0.0), Vec3::Y);
        assert!((t.forward() - Vec3::X).length() < 1e-5);
        assert!((t.up() - Vec3::Y).length() < 1e-5);
        assert!((t.left() - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-5);
    }
}
