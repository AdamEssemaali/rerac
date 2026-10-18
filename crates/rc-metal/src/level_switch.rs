//! **The runtime level change** (one path for every way the game changes level): the old level ends and the next one
//! is loaded in this process, exactly as the boot loads the first one. Spec: docs/plan/progression.md `## travel`.
//!
//! **API** (for the other lanes):
//! * `crate::travel_render::request_leave(dest)`: the game's `0x15f5c0 = dest; 0x15f570 = 1` (the level's main loop
//!   ends; `DoSpaceTransition` plays its cards / movies / flight and loads `dest` through this module; −1 = the front
//!   end). New Game (after the state reset to the template, level 0) and Load (after the restore) call it with the
//!   level to go to, Quit Game with −1 (`crate::media_render::request_level_exit` feeds it too).
//! * [`LevelChange::start_load`] / [`LevelChange::loaded`] / [`LevelChange::swap`]: the loader the transition drives
//!   (`read_file_entry_with_retry(level)` / `FUN_00257dc8` / the new overlay's `entry`). Every change goes through the
//!   transition (crate::travel_render); there is no second path.
//!
//! **What a change does** ([`apply_switch`], an exclusive system at the end of the frame the transition asks for it):
//! 1. The new level was loaded on a worker thread ([`load_bundle`]: the level data, the loader's moby spawn pass with the
//!    ship of `0x13e056`, the fog zones), as the game's async loader runs behind the flight or a card.
//! 2. The old level's entities are despawned: every mesh, camera and UI root that is not [`KeepAcrossLevels`] (the main
//!    camera, the movie picture). The old level's state is dropped by each module's [`LevelUnload`] systems (the play
//!    state, the renderer's per-level resources: [`remove`] / [`reset`]).
//! 3. The level-wide resources are replaced: `Level`, `MobySpawn`, `FogState`, `GameFog`, `ClearColor`, the audio
//!    system's level data (`crate::audio_out::AudioOut::swap_level`); the current level ([`crate::level_load::level_index`])
//!    and the lumps the store keeps for the old one ([`rc_data::Lumps::forget_level`]).
//! 4. [`LevelGeneration`] + 1, then the level start runs again: [`LevelStartup`] / [`LevelPostStartup`] (the systems that
//!    ran at the boot's `Startup` / `PostStartup` for the level: the tfrags, ties, shrubs, mobys, sky, water,
//!    particles, HUD, shadows, occlusion), and on the next frame the lazy set-ups (`PreUpdate`: the gameplay set-up with
//!    the moby table, the load pass and `Game::new`, the menus, the attachments, the vendor and menu models) whose
//!    guards follow the generation.
//!
//! What the game keeps across a level change is the boot memory: the saved game (`Persistent`, the global chunks in
//! core memory and every level's chunks), the session (`Session`: re-initialised by the level start's hero init), the
//! ship index 0x13e056 ([`crate::level_load::set_ship`]), the options; this module keeps those and the app's own state
//! (the window, the settings, the menu mode, the movie player, the audio output).

use crate::prelude::*;
use std::path::PathBuf;
use std::sync::Mutex;
use std::thread::JoinHandle;

// The level schedules are `Stage::LevelStartup` (run at the boot after `Startup`, and after each level change),
// `Stage::LevelPostStartup` (after it: the boot's `PostStartup` systems of the level) and `Stage::LevelUnload` (the old
// level's state dropped before the change: [`remove`] / [`reset`]), listed in the central schedule (crate::main).

/// An entity a level change keeps (the main camera, the movie UI).
#[derive(Clone, Copy, Debug, Default)]
pub struct KeepAcrossLevels;

/// The level generation: 0 at the boot, +1 per change. The lazy per-level set-ups re-run when it changes
/// (`Res<LevelGeneration>::is_changed`).
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelGeneration(pub u64);

/// A level's data as the loader leaves it (crate::main at the boot, [`LevelChange`] at a change).
pub struct LevelBundle {
    pub index: u32,
    pub level: crate::level_load::LoadedLevel,
    pub spawn: crate::moby_spawn::MobySpawn,
    pub fog: crate::fog_state::FogState,
}

/// Loads level `index` (its data, the loader's moby spawn pass, the fog zones): the one load path of the boot and of
/// every change.
pub fn load_bundle(root: &std::path::Path, index: u32, save: &rc_formats::moby_spawn::SpawnSave) -> anyhow::Result<LevelBundle> {
    let mut level = crate::level_load::load_level(root, index)?;
    let spawn = crate::moby_spawn::apply_load_pass(root, index, &mut level, save)?;
    let fog = crate::fog_state::FogState::new(crate::fog_state::load(root, index)?, &level.fog);
    Ok(LevelBundle { index, level, spawn, fog })
}

/// The loader of the next level.
#[derive(Default)]
pub struct LevelChange {
    load: Option<Load>,
    /// The swap was asked for: done at the end of the frame once the load is ready.
    swap: bool,
}

struct Load {
    index: u32,
    thread: Mutex<Option<JoinHandle<anyhow::Result<LevelBundle>>>>,
    result: Option<anyhow::Result<LevelBundle>>,
}

impl LevelChange {
    /// `read_file_entry_with_retry(level)` and the async loader: level `index` loads on a worker thread.
    pub fn start_load(&mut self, index: u32, save: rc_formats::moby_spawn::SpawnSave) {
        if self.load.as_ref().is_some_and(|l| l.index == index) { return; }
        let root: PathBuf = crate::level_load::extracted_root();
        let t = std::thread::Builder::new().name(format!("level {index:02} load")).spawn(move || load_bundle(&root, index, &save));
        let (thread, result) = match t {
            Ok(h) => (Some(h), None),
            Err(e) => (None, Some(Err(anyhow::anyhow!("cannot start the level loader thread: {e}")))),
        };
        println!("level switch: loading level {index:02} in the background");
        self.load = Some(Load { index, thread: Mutex::new(thread), result });
        self.swap = false;
    }

    /// `FUN_00257dc8`: the load is done (or failed: the failure is reported at the swap).
    pub fn loaded(&mut self) -> bool {
        let Some(l) = self.load.as_mut() else { return false };
        if l.result.is_some() { return true; }
        let mut g = l.thread.lock().unwrap_or_else(|e| e.into_inner());
        if g.as_ref().is_some_and(|h| h.is_finished()) {
            let h = g.take().unwrap();
            drop(g);
            l.result = Some(h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("the level loader thread panicked"))));
            return true;
        }
        false
    }

    /// The new overlay's `entry`: swap at the end of this frame (the load must be done).
    pub fn swap(&mut self) { self.swap = true; }
}

/// [`LevelUnload`]: remove resource `T` (the lazy set-up re-creates it).
pub fn remove<T: 'static>(ctx: &mut Ctx) { ctx.res.remove::<T>(); }

/// [`LevelUnload`]: `T` back to its default (a per-frame or per-level cache built at the plugin's `build`).
pub fn reset<T: Default + 'static>(ctx: &mut Ctx) { ctx.res.insert(T::default()); }

/// rc-engine's `LevelSwitchPlugin` (its systems are in the central schedule).
pub fn plugin(app: &mut crate::engine::app::App) {
    app.ctx.res.init::<LevelGeneration>();
    app.ctx.res.init::<LevelChange>();
}

/// PostStartup: the boot's level start: `LevelStartup` then `LevelPostStartup` (after the app's `Startup`: the main
/// camera).
pub fn run_level_startup(ctx: &mut Ctx) {
    run_schedule(ctx, Stage::LevelStartup);
    run_schedule(ctx, Stage::LevelPostStartup);
}

/// Last: the swap (module docs).
pub fn apply_switch(ctx: &mut Ctx) {
    let due = ctx.try_res::<LevelChange>().is_some_and(|c| c.swap && c.load.as_ref().is_some_and(|l| l.result.is_some()));
    if !due { return; }
    let load = ctx.res_mut::<LevelChange>().load.take().unwrap();
    ctx.res_mut::<LevelChange>().swap = false;
    let bundle = match load.result.unwrap() {
        Ok(b) => b,
        Err(e) => {
            // The game has no failure path (the disc read retries forever): the port stays on the old level.
            eprintln!("level switch: level {:02} not loaded ({e:#}); staying on level {:02}", load.index, crate::level_load::level_index());
            return;
        }
    };
    let t0 = std::time::Instant::now();
    let old = crate::level_load::level_index();
    run_schedule(ctx, Stage::LevelUnload);
    despawn_level_entities(&mut ctx.world);
    let LevelBundle { index, mut level, spawn, fog } = bundle;
    crate::level_load::set_level_index(index);
    rc_data::global().forget_level(&crate::level_load::extracted_root(), old);
    if let Some(data) = level.audio.take() {
        if let Some(mut out) = ctx.try_res_mut::<crate::audio_out::AudioOut>() { out.swap_level(data); }
    }
    let bg = level.background;
    ctx.res.insert(crate::game_camera::GameFog::new(level.fog, rc_game::fog_zones::PARTICLE_FAR12));
    ctx.res.insert(ClearColor(Color::srgb_u8(bg[0], bg[1], bg[2])));
    ctx.res.insert(fog);
    ctx.res.insert(spawn);
    ctx.res.insert(crate::Level(level));
    ctx.res_mut::<LevelGeneration>().0 += 1;
    run_schedule(ctx, Stage::LevelStartup);
    run_schedule(ctx, Stage::LevelPostStartup);
    println!("level switch: level {old:02} → {index:02} in {:.1} ms (generation {})", t0.elapsed().as_secs_f64() * 1e3, ctx.res::<LevelGeneration>().0);
}

/// Despawns every mesh, camera and UI root of the old level (module docs): entities with `Mesh3d`, `Mesh2d`, a
/// `Camera` or a UI `Node` that are not [`KeepAcrossLevels`] and have no kept ancestor.
fn despawn_level_entities(world: &mut World) {
    let mut candidates: Vec<Entity> = Vec::new();
    candidates.extend(world.query::<(Entity, &Mesh3d)>().without::<&KeepAcrossLevels>().iter().map(|(e, _)| e));
    candidates.extend(world.query::<(Entity, &Mesh2d)>().without::<&KeepAcrossLevels>().iter().map(|(e, _)| e));
    candidates.extend(world.query::<(Entity, &Camera)>().without::<&KeepAcrossLevels>().iter().map(|(e, _)| e));
    candidates.extend(world.query::<(Entity, &Node)>().without::<&KeepAcrossLevels>().iter().map(|(e, _)| e));
    candidates.sort_by_key(|e| e.to_bits());
    candidates.dedup();
    let mut gone = 0usize;
    for e in candidates {
        // A child of a kept entity stays with it.
        let mut cur = e;
        let mut kept = false;
        while let Some(p) = world.get::<&ChildOf>(cur).ok().map(|c| c.parent()) {
            if world.get::<&KeepAcrossLevels>(p).is_ok() {
                kept = true;
                break;
            }
            cur = p;
        }
        if kept { continue; }
        // Bevy's despawn takes the entity's descendants with it.
        if world.contains(e) {
            despawn_recursive(world, e);
            gone += 1;
        }
    }
    println!("level switch: {gone} entities of the old level despawned");
}
