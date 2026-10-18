//! `rerac-metal`: ReRAC's native Metal renderer, without Bevy (docs/plan/metal_renderer.md).
//!
//! Phase 1 draws one level's terrain (tfrags: LOD, morph, GS lighting colours, fog, the GS alpha-test split) with a
//! fly camera, in a winit window on a `CAMetalLayer`. Nothing of the game runs: no tick, no mobys, ties, shrubs, sky,
//! HUD or audio yet.
//!
//! Controls: right mouse button + move to look, W/A/S/D to fly, Q/E down/up, Shift faster, P prints the camera as an
//! `RC_CAM` value, Esc quits.
//!
//! Environment (the names rc-engine uses where they mean the same): `RC_DATA_DIR` / `RC_EXTRACTED` / `--data-dir`
//! (the game data folder), `RC_LEVEL`, `RC_CAM`, `RC_FOG=0`, `RC_NO_LIGHT=1`, `RC_LOD=0`, `RC_LOD_TINT=1`, `RC_NOVSYNC=1`.
//! `RC_SCREENSHOT=<file.png>` renders one frame offscreen without opening a window (`RC_SCREENSHOT_SIZE=WxH`, default
//! 1024x768), writes it and exits.

#[cfg(target_os = "macos")]
mod app;
mod camera;
mod engine;
#[cfg(target_os = "macos")]
mod render;
#[cfg(target_os = "macos")]
mod gpu;
mod level;
mod shader;
mod tfrag;

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let mut data_dir = None;
    while let Some(a) = args.next() {
        let s = a.to_string_lossy();
        if let Some(v) = s.strip_prefix("--data-dir=") {
            data_dir = Some(v.into());
        } else if s == "--data-dir" {
            data_dir = Some(args.next().ok_or_else(|| anyhow::anyhow!("--data-dir needs a folder"))?.into());
        } else {
            eprintln!("warning: ignoring argument {s:?}");
        }
    }
    let root = level::data_root(data_dir);
    let index = level::level_index()?;
    let t0 = std::time::Instant::now();
    let level = level::load(&root, index).map_err(|e| e.context(format!("loading level {index:02} from {}", root.display())))?;
    println!("level {index:02}: loaded from {} in {:.0?}", root.display(), t0.elapsed());
    match std::env::var_os("RC_SCREENSHOT").filter(|v| !v.is_empty()) {
        Some(path) => app::screenshot(&level, std::path::Path::new(&path)),
        None => app::run(level),
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("rerac-metal runs on macOS only (Metal); use rerac (rc-engine) elsewhere");
    std::process::exit(1);
}
