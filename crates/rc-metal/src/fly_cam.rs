//! Debug fly camera: WASD + mouse look, Shift for speed. It only moves the view; the projection,
//! depth mapping and fog are the game's (`game_camera.rs`).

use crate::prelude::*;

use crate::game_camera::CameraSource;

/// [`grab_cursor`]'s Tab toggle (rc-engine: `Local<bool>`).
#[derive(Default)]
pub struct GrabToggle(bool);

pub fn plugin(app: &mut crate::engine::app::App) { app.ctx.res.insert(GrabToggle::default()); }

pub struct FlyCam {
    /// Game units per second.
    pub speed: f32,
    /// Multiplier while Shift is held.
    pub fast: f32,
    /// Radians per pixel of mouse motion.
    pub sensitivity: f32,
    pub yaw: f32,
    pub pitch: f32,
}

impl FlyCam {
    /// Takes yaw/pitch from an existing orientation so a `looking_at` transform is kept.
    pub fn from_transform(t: &Transform, speed: f32) -> Self {
        let (yaw, pitch, _) = t.rotation.to_euler(EulerRot::YXZ);
        FlyCam { speed, fast: 5.0, sensitivity: 0.003, yaw, pitch }
    }
}

pub const CONTROLS: &str = "\
controls: hold right mouse (or press Tab to toggle) to look | WASD move | Space/E up, Ctrl/Q down
          Shift fast | mouse wheel changes speed | Esc releases the cursor | P prints the game camera (RC_CAM + VU matrix)";

/// Right mouse held or Tab toggles a locked, hidden cursor; Esc releases it. With gameplay on
/// (`CameraSource` present, crate::gameplay) Tab switches cameras instead and only the right button grabs.
pub fn grab_cursor(ctx: &mut Ctx) {
    let input = ctx.res::<InputState>();
    let (mouse, keys) = (&input.mouse, &input.keys);
    let source = ctx.try_res::<CameraSource>();
    let mut toggled = ctx.res_mut::<GrabToggle>();
    if keys.just_pressed(KeyCode::Tab) && source.is_none() { toggled.0 = !toggled.0; }
    if keys.just_pressed(KeyCode::Escape) { toggled.0 = false; }
    let grab = toggled.0 || mouse.pressed(MouseButton::Right);
    let mut window = ctx.res_mut::<Window>();
    if window.cursor_grabbed != grab {
        window.cursor_grabbed = grab;
        window.cursor_visible = !grab;
    }
}

pub fn fly_cam(ctx: &mut Ctx) {
    let time = ctx.res::<Time>();
    let input = ctx.res::<InputState>();
    let (keys, motion, scroll) = (&input.keys, &input.motion, &input.scroll);
    // Frozen while the game camera drives the view (crate::play_camera).
    if ctx.try_res::<CameraSource>().is_some_and(|s| *s == CameraSource::Play) { return; }
    let looking = ctx.res::<Window>().cursor_grabbed;
    for (tf, cam) in ctx.world.query::<(&mut Transform, &mut FlyCam)>().iter() {
        if scroll.delta.y != 0.0 { cam.speed = (cam.speed * 1.2f32.powf(scroll.delta.y.signum())).clamp(0.5, 5000.0); }
        if looking && motion.delta != Vec2::ZERO {
            cam.yaw -= motion.delta.x * cam.sensitivity;
            cam.pitch = (cam.pitch - motion.delta.y * cam.sensitivity).clamp(-1.54, 1.54);
            tf.rotation = Quat::from_euler(EulerRot::YXZ, cam.yaw, cam.pitch, 0.0);
        }
        let mut dir = Vec3::ZERO;
        let (fwd, right) = (tf.forward().as_vec3(), tf.right().as_vec3());
        if keys.pressed(KeyCode::KeyW) { dir += fwd; }
        if keys.pressed(KeyCode::KeyS) { dir -= fwd; }
        if keys.pressed(KeyCode::KeyD) { dir += right; }
        if keys.pressed(KeyCode::KeyA) { dir -= right; }
        if keys.any_pressed([KeyCode::Space, KeyCode::KeyE]) { dir += Vec3::Y; }
        if keys.any_pressed([KeyCode::ControlLeft, KeyCode::KeyQ]) { dir -= Vec3::Y; }
        if dir != Vec3::ZERO {
            let fast = if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) { cam.fast } else { 1.0 };
            tf.translation += dir.normalize() * cam.speed * fast * time.default.delta_secs();
        }
    }
}
