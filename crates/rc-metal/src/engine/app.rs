//! The frame loop: Bevy's schedule structure with rc-engine's systems in an explicit order (docs/plan/metal_renderer.md,
//! "Schedule"), the fixed-step loop, and the custom level schedules.
//!
//! Per frame: First → PreUpdate → (FixedUpdate → FixedPostUpdate) × n → AfterFixedMainLoop → Update → PostUpdate →
//! Last, then the render. `n` is set by the frame pace in First (rc-engine `frame_pace::steps`, Bevy's
//! `TimeUpdateStrategy::FixedTimesteps(n)`); [`Time::begin_frame`] advances the clocks after First's pace system, as
//! Bevy's `TimeSystems` run after it. Transform and visibility propagation is the [`PROPAGATE`] entry of PostUpdate.

use super::assets::Assets;
use super::ecs::{propagate, Ctx};
use super::image::Image;
use super::input::InputState;
use super::mesh::Mesh;
use super::time::Time;
use crate::render::gfx::ShaderBuffer;
use std::collections::HashMap;
use std::rc::Rc;

pub type SystemFn = fn(&mut Ctx);

/// A schedule's label (Bevy's built-in and rc-engine's custom ones).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    Startup,
    PostStartup,
    First,
    PreUpdate,
    FixedUpdate,
    FixedPostUpdate,
    AfterFixedMainLoop,
    Update,
    PostUpdate,
    Last,
    LevelStartup,
    LevelPostStartup,
    LevelUnload,
}

/// Bevy's `TransformSystems::Propagate` + `VisibilityPropagate`, as an entry of PostUpdate. Drawable entities and
/// cameras spawned without a `Transform` get one first (Bevy's required components).
pub fn propagate_system(ctx: &mut Ctx) {
    use super::scene::{Camera, Transform};
    let w = &mut ctx.world;
    let mut need: Vec<hecs::Entity> = Vec::new();
    need.extend(w.query::<(hecs::Entity, &super::mesh::Mesh3d)>().without::<&Transform>().iter().map(|(e, _)| e));
    need.extend(w.query::<(hecs::Entity, &super::mesh::Mesh2d)>().without::<&Transform>().iter().map(|(e, _)| e));
    need.extend(w.query::<(hecs::Entity, &Camera)>().without::<&Transform>().iter().map(|(e, _)| e));
    need.extend(w.query::<(hecs::Entity, &crate::render::frame::Sprite)>().without::<&Transform>().iter().map(|(e, _)| e));
    for e in need { let _ = w.insert_one(e, Transform::IDENTITY); }
    propagate(w);
}
pub const PROPAGATE: (&str, SystemFn) = ("propagate", propagate_system);

/// The systems of every schedule, in order.
#[derive(Default, Clone)]
pub struct Schedules {
    pub map: HashMap<Stage, Vec<(&'static str, SystemFn)>>,
}

impl Schedules {
    pub fn add(&mut self, stage: Stage, name: &'static str, f: SystemFn) { self.map.entry(stage).or_default().push((name, f)); }

    pub fn list(&self, stage: Stage) -> &[(&'static str, SystemFn)] { self.map.get(&stage).map_or(&[], |v| v.as_slice()) }
}

/// Runs schedule `stage` (an exclusive system's `world.run_schedule`).
pub fn run_schedule(ctx: &mut Ctx, stage: Stage) {
    let s = ctx.res::<Rc<Schedules>>().clone();
    for (_, f) in s.list(stage) { ctx.run(*f); }
}

/// Bevy's `AppExit`: set to end the run with that status.
#[derive(Clone, Copy, Debug, Default)]
pub struct AppExit(pub Option<i32>);

/// The window as the systems see and change it (applied to winit after each frame by crate::app).
#[derive(Clone, Debug)]
pub struct Window {
    pub title: String,
    /// Physical size of the drawable.
    pub physical_size: glam::UVec2,
    pub scale_factor: f64,
    pub fullscreen: bool,
    pub vsync: bool,
    pub cursor_grabbed: bool,
    pub cursor_visible: bool,
    /// The monitor's refresh rate in millihertz, when known (rc-engine `frame_pace` paces from it).
    pub refresh_millihertz: Option<u32>,
    pub focused: bool,
}

impl Default for Window {
    fn default() -> Self {
        Window {
            title: "ReRAC".into(),
            physical_size: glam::UVec2::new(1024, 768),
            scale_factor: 1.0,
            fullscreen: false,
            vsync: true,
            cursor_grabbed: false,
            cursor_visible: true,
            refresh_millihertz: None,
            focused: true,
        }
    }
}

impl Window {
    pub fn width(&self) -> f32 { self.physical_size.x as f32 / self.scale_factor as f32 }
    pub fn height(&self) -> f32 { self.physical_size.y as f32 / self.scale_factor as f32 }
    pub fn physical_width(&self) -> u32 { self.physical_size.x }
    pub fn physical_height(&self) -> u32 { self.physical_size.y }
}

/// Frames since start (1 = the first update), the engine's own counter.
#[derive(Clone, Copy, Debug, Default)]
pub struct UpdateCount(pub u64);

pub struct App {
    pub ctx: Ctx,
    pub schedules: Schedules,
    started: bool,
}

impl Default for App {
    fn default() -> Self { Self::new() }
}

impl App {
    pub fn new() -> Self {
        let mut ctx = Ctx::default();
        ctx.res.insert(Assets::<Mesh>::default());
        ctx.res.insert(Assets::<Image>::default());
        ctx.res.insert(Assets::<ShaderBuffer>::default());
        ctx.res.insert(Time::default());
        ctx.res.insert(InputState::default());
        ctx.res.insert(Window::default());
        ctx.res.insert(AppExit::default());
        ctx.res.insert(UpdateCount::default());
        App { ctx, schedules: Schedules::default(), started: false }
    }

    pub fn add(&mut self, stage: Stage, name: &'static str, f: SystemFn) -> &mut Self {
        self.schedules.add(stage, name, f);
        self
    }

    fn run(&mut self, stage: Stage) {
        let list = self.schedules.list(stage).to_vec();
        let prof = profiling();
        for (name, f) in list {
            if prof {
                let t = std::time::Instant::now();
                self.ctx.run(f);
                record(name, t.elapsed());
            } else {
                self.ctx.run(f);
            }
        }
    }

    /// One frame of the main world (the render follows, by the caller).
    pub fn update(&mut self) {
        if !self.started {
            self.started = true;
            self.ctx.res.insert(Rc::new(self.schedules.clone()));
            self.run(Stage::Startup);
            self.run(Stage::PostStartup);
        }
        self.ctx.res_mut::<UpdateCount>().0 += 1;
        self.ctx.res_mut::<InputState>().poll_gamepads();
        self.run(Stage::First);
        self.run(Stage::PreUpdate);
        let n = self.ctx.res::<Time>().steps;
        for _ in 0..n {
            self.ctx.res_mut::<Time>().begin_fixed_step();
            self.run(Stage::FixedUpdate);
            self.run(Stage::FixedPostUpdate);
        }
        self.ctx.res_mut::<Time>().end_fixed_loop();
        self.run(Stage::AfterFixedMainLoop);
        self.run(Stage::Update);
        self.run(Stage::PostUpdate);
        self.run(Stage::Last);
        // Bevy frees unreferenced assets after the frame.
        self.ctx.res_mut::<Assets<Mesh>>().collect_garbage();
        self.ctx.res_mut::<Assets<Image>>().collect_garbage();
        self.ctx.res_mut::<Assets<ShaderBuffer>>().collect_garbage();
    }

    /// Clears the per-frame input (after the frame; the next frame's events accumulate from here).
    pub fn end_frame_input(&mut self) { self.ctx.res_mut::<InputState>().begin_frame(); }

    pub fn exit_status(&self) -> Option<i32> { self.ctx.res::<AppExit>().0 }
}

/// `RC_METAL_PROFILE=1`: per-system times, the slowest printed every 5 s.
fn profiling() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("RC_METAL_PROFILE").is_ok_and(|v| v.trim() == "1"))
}

fn record(name: &'static str, d: std::time::Duration) {
    use std::sync::Mutex;
    static ACC: Mutex<Option<(std::time::Instant, HashMap<&'static str, f64>)>> = Mutex::new(None);
    let mut a = ACC.lock().unwrap();
    let e = a.get_or_insert_with(|| (std::time::Instant::now(), HashMap::new()));
    *e.1.entry(name).or_default() += d.as_secs_f64();
    if e.0.elapsed().as_secs_f32() >= 5.0 {
        let secs = e.0.elapsed().as_secs_f64();
        let mut v: Vec<_> = e.1.iter().map(|(k, t)| (*k, *t)).collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1));
        let top: Vec<String> = v.iter().take(12).map(|(k, t)| format!("{k} {:.2}%", 100.0 * t / secs)).collect();
        println!("profile systems (share of wall time): {}", top.join(", "));
        *e = (std::time::Instant::now(), HashMap::new());
    }
}

/// Bevy's `Time::begin_frame` from the pace's step count: call from First after the pace system (crate::frame_pace).
pub fn time_system(ctx: &mut Ctx) {
    let n = ctx.try_res::<crate::engine::time::Steps>().map_or(1, |s| s.0);
    ctx.res_mut::<Time>().begin_frame(n);
}
