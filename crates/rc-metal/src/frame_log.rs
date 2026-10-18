//! `RC_FRAME_LOG=1`: a line for every slow frame (over 25 ms), to find where a stall comes from: the frame time,
//! the gameplay ticks it ran, Ratchet's position, and how many entities, meshes, moby materials and images exist and
//! how many appeared since the last frame. A stall with many new entities or materials points at first-show spawns
//! and pipeline compiles; one with none points elsewhere (the tick, the renderer). Off by default.

use crate::prelude::*;

/// Frames slower than this are printed.
const SLOW_MS: f32 = 25.0;

/// `RC_FRAME_LOG=1` (rc-engine's `FrameLogPlugin` adds [`log`] only then).
pub fn enabled() -> bool { std::env::var("RC_FRAME_LOG").is_ok_and(|v| v.trim() == "1") }

pub fn plugin(app: &mut crate::engine::app::App, _r: &mut crate::render::frame::Renderer) {
    if enabled() {
        println!("frame log: frames over {SLOW_MS} ms are printed (RC_FRAME_LOG=1)");
        app.ctx.res.insert(LogLocal::default());
    }
}

#[derive(Default)]
struct Last {
    counter: Option<u64>,
    counts: [usize; 4],
}

/// The system's locals (rc-engine: `Local<Option<Instant>>`, `Local<Last>`).
#[derive(Default)]
struct LogLocal {
    clock: Option<std::time::Instant>,
    last: Last,
}

/// Last.
pub fn log(ctx: &mut Ctx) {
    let Some(mut local) = ctx.try_res_mut::<LogLocal>() else { return };
    let LogLocal { clock, last } = &mut *local;
    // The wall clock: the clocks advance by whole game ticks (crate::frame_pace).
    let now = std::time::Instant::now();
    let ms = clock.replace(now).map_or(0.0, |t| (now - t).as_secs_f32() * 1000.0);
    let play = ctx.try_res::<crate::gameplay::Play>();
    let counter = play.as_ref().map(|p| p.game.counter);
    let counts = [
        ctx.world.len() as usize,
        ctx.res::<Assets<Mesh>>().len(),
        ctx.try_res::<Assets<crate::moby_render::MobyMaterial>>().map_or(0, |m| m.len()),
        ctx.res::<Assets<Image>>().len(),
    ];
    if ms > SLOW_MS {
        let ticks = counter.zip(last.counter).map_or(0, |(a, b)| a.saturating_sub(b));
        let at = play.as_ref().map(|p| p.game.hero.position()).unwrap_or_default();
        let d = |k: usize| counts[k] as i64 - last.counts[k] as i64;
        println!(
            "frame log: {ms:.1} ms, {ticks} ticks (tick {}), Ratchet at {:.1}, {:.1}, {:.1}; entities {} ({:+}), meshes {} ({:+}), moby materials {} ({:+}), images {} ({:+})",
            counter.unwrap_or(0), at[0], at[1], at[2], counts[0], d(0), counts[1], d(1), counts[2], d(2), counts[3], d(3)
        );
    }
    last.counter = counter;
    last.counts = counts;
}
