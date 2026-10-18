//! Frame-exact runs: a game tick counter that never reads the wall clock in deterministic mode, and a
//! screenshot taken at an exact frame number.
//!
//! Environment:
//! - `RC_SCREENSHOT_FRAME=N` capture the frame rendered by update N (1-based) to `RC_SCREENSHOT`, then exit.
//!   Implies `RC_DETERMINISTIC=1`. `main.rs` then does not schedule its wall-clock capture (`RC_SCREENSHOT_DELAY`).
//! - `RC_DETERMINISTIC=1` every update advances exactly one 60 Hz game tick: `Time<Real>`, `Time<Virtual>`
//!   and `Time` advance by exactly 1/60 s and `FixedUpdate` runs exactly once per update
//!   (`TimeUpdateStrategy::FixedTimesteps(1)` with a 60 Hz fixed clock), so everything driven by `Time`
//!   or `FixedUpdate` is a function of the frame number, not of how fast the machine renders.
//! - `RC_DUMP_FRAMES=<start>..<end>` (dev only, not a Port Option) capture every frame from update `start` to update
//!   `end` (inclusive) as `<RC_DUMP_DIR>/frame_NNNNN.png` (default folder `frames`), then exit. Implies
//!   `RC_DETERMINISTIC=1` and the offscreen target below; `RC_SCREENSHOT_FRAME` wins when both are set. A frame
//!   reached while pipelines still compile is captured anyway, with a warning naming it. Leave `RC_SCREENSHOT` unset
//!   (`main.rs` would schedule its wall-clock capture and end the run).
//! - `RC_DUMP_REALTIME=1` (dev only) keeps `RC_DUMP_FRAMES` in real-time mode (ticks from the wall clock, as in
//!   play; the PNG saving slows the frames, so updates carry 0..n ticks).
//! - `RC_DUMP_TICKS=n,m,…` (dev only) in frame-exact mode, update k advances the n-th entry's ticks (cycled) instead of
//!   one: `1,0` is a 120 Hz display (every other frame has no game tick), `2` a 30 Hz one. `GameTicks` counts ticks,
//!   not updates.
//!
//! [`GameTicks`] is the game's 60 Hz logic tick count since level start: one per update in deterministic
//! mode; otherwise the catch-up count `floor(60·t_virtual)`, like the game's main loop at 30 fps.
//!
//! Always (independent of the mode above), for run-to-run identical frames:
//! - every `Camera3d` gets `NoIndirectDrawing` as it is spawned (`RC_INDIRECT=1` keeps Bevy's GPU indirect
//!   mode): indirect mode orders the instances of each instanced draw with GPU atomics, see
//!   [`no_indirect_drawing`];
//! - the blended (Transparent3d) phase breaks equal-distance ties by entity, see [`stable_transparent_order`].
//!
//! (rc-metal compiles every pipeline before the frame that needs it, so no capture waits for pipelines.)
//!
//! Capture robustness (docs/plan/level_sweep.md H1/H2):
//! - **Offscreen target.** On macOS a window that is fully covered, on another Space or behind a
//!   full-screen app is "occluded": wgpu-hal's Metal surface then refuses to hand out a drawable
//!   (`SurfaceError::Occluded`), Bevy renders nothing to the window, and a window screenshot comes back all
//!   zeros for as long as the window stays hidden (2,000+ frames seen). Every game camera renders into the
//!   game frame (crate::display: an offscreen image in the swapchain format `Bgra8UnormSrgb`), and the capture
//!   is of that image: frame N does not depend on the window being visible. `RC_CAPTURE_WINDOW=1` captures the
//!   window (the frame as presented, on black).
//! - **Bounded retry.** A capture that still comes back blank is retried on later frames for at most
//!   [`MAX_RETRY_FRAMES`] frames past N; then whatever came back is saved, a warning names the frame, and the
//!   process exits with status 3 (the image is not frame N). A later-frame success also exits 3.
//! - **Monitor blips.** Bevy links each window to its monitor (`OnMonitor` → `HasWindows`, `linked_spawn`),
//!   so when winit briefly stops listing the monitor, `create_monitors` despawns the monitor entity and the
//!   window with it, and `exit_on_all_closed` ends the run ("Monitor removed" → "No windows are open,
//!   exiting", exit 0, no PNG). The link is dropped as soon as it is made (nothing here uses it), in every
//!   mode.
//! - **Exit before capture.** If the app exits before the capture was saved, the process exits with
//!   status 2 instead of 0.

use crate::prelude::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// Frames past `RC_SCREENSHOT_FRAME` a blank capture is retried for before it is saved anyway.
pub const MAX_RETRY_FRAMES: u64 = 30;

/// Game logic ticks per second (NTSC).
pub const TICK_HZ: f64 = 60.0;

/// 60 Hz game ticks since start (see the module docs); updated in `First`.
#[derive(Default, Clone, Copy, Debug)]
pub struct GameTicks(pub u64);

/// Updates run so far, 1 in the first `Update`; updated in `First`.
#[derive(Default, Clone, Copy, Debug)]
pub struct FrameNumber(pub u64);

/// Whether this run is frame-exact (`RC_DETERMINISTIC=1` or `RC_SCREENSHOT_FRAME` set).
#[derive(Clone, Copy, Debug)]
pub struct Deterministic(pub bool);

fn env(name: &str) -> Option<String> { std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) }

pub fn screenshot_frame() -> Option<u64> { env("RC_SCREENSHOT_FRAME").and_then(|v| v.parse().ok()) }

/// `RC_DUMP_FRAMES=<start>..<end>` (module docs): the inclusive frame range, when `RC_SCREENSHOT_FRAME` is not set.
pub fn dump_frames() -> Option<(u64, u64)> {
    if screenshot_frame().is_some() { return None; }
    let v = env("RC_DUMP_FRAMES")?;
    let (a, b) = v.split_once("..")?;
    let (a, b): (u64, u64) = (a.trim().parse().ok()?, b.trim().trim_start_matches('=').parse().ok()?);
    (a >= 1 && b >= a).then_some((a, b))
}

pub fn deterministic() -> bool {
    screenshot_frame().is_some() || (dump_frames().is_some() && !dump_realtime()) || env("RC_DETERMINISTIC").as_deref() == Some("1")
}

/// `RC_DUMP_REALTIME=1` (dev only): `RC_DUMP_FRAMES` without frame-exact mode (wall-clock ticks, as in play).
pub fn dump_realtime() -> bool { env("RC_DUMP_REALTIME").as_deref() == Some("1") }

/// What a capture does with the frame's RGBA pixels (width, height, pixels).
pub type CaptureDone = Box<dyn FnOnce(u32, u32, &[u8], &mut crate::engine::ecs::Resources)>;

/// A capture of this frame's render (Bevy's `Screenshot` entity and its `ScreenshotCaptured` observer): the image (None:
/// the window's drawable), and what to do with the RGBA pixels. Taken by the app after the frame is rendered.
pub struct CaptureRequest {
    pub target: Option<Handle<Image>>,
    pub done: CaptureDone,
}

/// The captures requested this frame.
#[derive(Default)]
pub struct Captures(pub Vec<CaptureRequest>);

struct FrameShot {
    path: PathBuf,
    frame: u64,
    /// Set by the capture when the image came back blank.
    retry: Arc<AtomicBool>,
    /// Set by the capture once it has saved (or given up on) the image.
    saved: Arc<AtomicBool>,
    /// The offscreen image the cameras render to (None with `RC_CAPTURE_WINDOW=1`).
    target: Option<Handle<Image>>,
    /// rc-engine: `Local<bool>` of [`frame_screenshot`].
    done: bool,
}

/// `RC_DUMP_FRAMES`: the range, the folder and the number of frames saved so far.
struct FrameDump {
    dir: PathBuf,
    range: (u64, u64),
    target: Option<Handle<Image>>,
    saved: Arc<AtomicU64>,
}

/// PreUpdate, frame-exact mode: clears this frame's keyboard, mouse and gamepad state right after the input update, so no
/// live device input reaches the game, the dev keys or the fly camera.
pub fn no_live_input(ctx: &mut Ctx) {
    if !ctx.res::<Deterministic>().0 { return; }
    let mut input = ctx.res_mut::<InputState>();
    input.keys.reset_all();
    input.mouse.reset_all();
    input.motion.delta = Vec2::ZERO;
    input.scroll.delta = Vec2::ZERO;
    input.gamepads.clear();
}

/// rc-engine's `DeterminismPlugin`.
pub fn plugin(app: &mut crate::engine::app::App) {
    let det = deterministic();
    app.ctx.res.insert(Deterministic(det));
    app.ctx.res.insert(GameTicks::default());
    app.ctx.res.insert(FrameNumber::default());
    app.ctx.res.insert(Captures::default());
    if det {
        app.ctx.res.insert(Steps(1));
        match tick_pattern() {
            Some(p) => println!("determinism: frame-exact mode, RC_DUMP_TICKS {p:?} ticks of 1/60 s per update (cycled)"),
            None => println!("determinism: frame-exact mode (1 tick of 1/60 s per update)"),
        }
    }
    let dump = dump_frames();
    let frame = screenshot_frame();
    if frame.is_none() && dump.is_none() { return; }
    let path = std::env::var_os("RC_SCREENSHOT");
    if frame.is_some() && path.is_none() {
        eprintln!("RC_SCREENSHOT_FRAME is set without RC_SCREENSHOT: no capture");
        return;
    }
    // The game frame (crate::display: every game camera renders into it, at its size); RC_CAPTURE_WINDOW=1 captures
    // the window (the frame presented on black).
    let target = (env("RC_CAPTURE_WINDOW").as_deref() != Some("1")).then(|| app.ctx.res::<crate::display::GameFrame>().image.clone());
    if target.is_some() { println!("determinism: capturing the game frame (RC_CAPTURE_WINDOW=1 captures the window)"); }
    if let Some(range) = dump {
        let dir = PathBuf::from(std::env::var_os("RC_DUMP_DIR").unwrap_or_else(|| "frames".into()));
        if let Err(e) = std::fs::create_dir_all(&dir) { eprintln!("RC_DUMP_FRAMES: cannot create {}: {e}", dir.display()); }
        println!("determinism: RC_DUMP_FRAMES: frames {}..={} into {}", range.0, range.1, dir.display());
        app.ctx.res.insert(FrameDump { dir, range, target, saved: Arc::default() });
        return;
    }
    let (Some(frame), Some(path)) = (frame, path) else { return };
    app.ctx.res.insert(FrameShot { path: path.into(), frame: frame.max(1), retry: Arc::default(), saved: Arc::default(), target, done: false });
}

/// First, after the clocks advance.
pub fn advance_ticks(ctx: &mut Ctx) {
    let det = ctx.res::<Deterministic>().0;
    let elapsed = ctx.res::<Time>().virt.elapsed;
    let mut frame = ctx.res_mut::<FrameNumber>();
    let mut ticks = ctx.res_mut::<GameTicks>();
    frame.0 += 1;
    ticks.0 = if det {
        match tick_pattern() {
            Some(p) => ticks.0 + p[((frame.0 - 1) % p.len() as u64) as usize] as u64,
            None => frame.0,
        }
    } else {
        (elapsed.as_nanos() * TICK_HZ as u128 / 1_000_000_000) as u64
    };
}

/// `RC_DUMP_TICKS=n,m,…` (dev only; module docs): the game ticks of each update in frame-exact mode, cycled.
pub fn tick_pattern() -> Option<&'static [u32]> {
    static P: std::sync::OnceLock<Option<Vec<u32>>> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        let v: Vec<u32> = env("RC_DUMP_TICKS")?.split(',').map(|t| t.trim().parse().ok()).collect::<Option<_>>()?;
        (!v.is_empty() && v.iter().any(|&n| n > 0)).then_some(v)
    })
    .as_deref()
}

/// First, before the clocks advance, frame-exact mode with `RC_DUMP_TICKS`: this update's fixed steps (the time advances
/// by as many ticks).
pub fn pattern_steps(ctx: &mut Ctx) {
    if !ctx.res::<Deterministic>().0 { return; }
    let Some(p) = tick_pattern() else { return };
    let frame = ctx.res::<FrameNumber>().0;
    *ctx.res_mut::<Steps>() = Steps(p[(frame % p.len() as u64) as usize]);
}

/// Last: the run ends (window closed) before the capture was saved: exit status 2.
pub fn exit_before_capture(ctx: &mut Ctx) {
    let Some(shot) = ctx.try_res::<FrameShot>() else { return };
    if ctx.res::<AppExit>().0.is_some() && !shot.saved.load(Ordering::Acquire) {
        eprintln!("determinism: the app is exiting at frame {} before the frame-{} capture was saved; exit status 2", ctx.res::<FrameNumber>().0, shot.frame);
        std::process::exit(2);
    }
}

pub fn save_rgb(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    let rgb: Vec<u8> = rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().and_then(|mut wr| wr.write_image_data(&rgb)).map_err(|e| e.to_string())
}

/// Update, `RC_DUMP_FRAMES`: one capture per frame of the range, saved as `frame_NNNNN.png`; exit once the last is saved.
pub fn dump_frame(ctx: &mut Ctx) {
    let Some(dump) = ctx.try_res::<FrameDump>() else { return };
    let (a, b) = dump.range;
    let n = ctx.res::<FrameNumber>().0;
    if n < a || n > b { return; }
    let (path, saved, count) = (dump.dir.join(format!("frame_{n:05}.png")), dump.saved.clone(), b - a + 1);
    let target = dump.target.clone();
    ctx.res_mut::<Captures>().0.push(CaptureRequest {
        target,
        done: Box::new(move |w, h, rgba, res| {
            if let Err(e) = save_rgb(&path, w, h, rgba) { eprintln!("cannot save {}: {e}", path.display()); }
            if saved.fetch_add(1, Ordering::AcqRel) + 1 == count {
                println!("determinism: RC_DUMP_FRAMES: {count} frames saved; exiting");
                res.get_mut::<AppExit>().0 = Some(0);
            }
        }),
    });
}

/// Update, `RC_SCREENSHOT_FRAME`.
pub fn frame_screenshot(ctx: &mut Ctx) {
    let Some(mut shot) = ctx.try_res_mut::<FrameShot>() else { return };
    if shot.retry.swap(false, Ordering::AcqRel) { shot.done = false; }
    let n = ctx.res::<FrameNumber>().0;
    if shot.done || n < shot.frame { return; }
    shot.done = true;
    let (path, want, retry, saved) = (shot.path.clone(), shot.frame, shot.retry.clone(), shot.saved.clone());
    let give_up = n >= want + MAX_RETRY_FRAMES;
    let target = shot.target.clone();
    drop(shot);
    ctx.res_mut::<Captures>().0.push(CaptureRequest {
        target,
        done: Box::new(move |w, h, rgba, res| {
            let mut status = 0;
            // An all-zero image is a frame that was never drawn (an occluded window: see the module docs); nothing the
            // game draws is pure black edge to edge. Capture a later frame instead, for at most MAX_RETRY_FRAMES frames.
            if rgba.as_chunks::<4>().0.iter().all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0) {
                if !give_up {
                    eprintln!("determinism: frame {n} came back blank (window not presented?); capturing a later frame");
                    retry.store(true, Ordering::Release);
                    return;
                }
                eprintln!("determinism: WARNING frame {n} came back blank and {MAX_RETRY_FRAMES} frames of retries are used up; saving it anyway");
            }
            match save_rgb(&path, w, h, rgba) {
                Ok(()) => println!("screenshot of frame {n} saved to {}", path.display()),
                Err(e) => {
                    eprintln!("cannot save screenshot to {}: {e}", path.display());
                    status = 1;
                }
            }
            if n != want {
                eprintln!("determinism: WARNING the saved screenshot is frame {n}, not frame {want}; exit status 3");
                status = status.max(3);
            }
            saved.store(true, Ordering::Release);
            if status != 0 { std::process::exit(status); }
            res.get_mut::<AppExit>().0 = Some(0);
        }),
    });
}
