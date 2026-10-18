# Native Metal renderer (`crates/rc-metal`)

Status: **done**. The whole game (front end, title, every level, gameplay, menus, movies, audio, saves, level switching)
runs in `rerac-metal` on Metal with no Bevy and no wgpu, frame for frame like `rerac`, on macOS and on iOS (below). Decided by the user on
2026-10-18 (docs/plan/decisions.md). Porting guide for later rc-engine changes: `docs/plan/metal_porting.md`.

## Why

rc-engine renders through Bevy 0.19 → wgpu 29 → Metal. The layers cost time on the render thread (wgpu's
`CommandEncoder::finish` alone is 7.7 ms per frame in the Novalis profile, docs/plan/performance.md) and hide the
GPU behind Bevy's phase and pipeline machinery (pipeline variants, G-REN-037).

## Shape

- **Own executable** `rerac-metal` (`cargo run -p rc-metal --release`), same arguments, environment variables, exit
  codes and `--version-json` as `rerac`: winit 0.30 window, `CAMetalLayer` through `raw-window-metal`, Metal through
  `objc2-metal` 0.3, input through winit + gilrs (iOS: GameController and touches), ECS through hecs 0.11.
- **Every rc-engine module is ported under its own name** (`crates/rc-metal/src/<module>.rs`), logic, constants,
  comments and tests verbatim; only the Bevy API changed (table in `metal_porting.md`). rc-game, rc-data, rc-formats,
  rc-video and the audio code are shared unchanged.
- **The runtime that replaces Bevy** (`src/engine/`): `App` with Bevy's schedules (Startup … Last, FixedUpdate with
  Bevy's fixed-step rules, the level schedules), `Time` with Bevy's real / virtual / fixed clocks (the first update has
  zero delta and no fixed step, as Bevy's), assets with handle refcounting, transform propagation, required components.
  The system order in `main.rs::schedule` is rc-engine's executable order, dumped from Bevy (`--features bevy/debug`).
- **Shaders are rc-engine's WGSL**, embedded at build time and translated to MSL by naga 29 at start-up
  (`src/shader.rs`): a naga_oil subset (imports, `#ifdef` / `#else ifdef`, `#{DEF}`), Bevy's shader modules replaced by
  stand-ins in `shaders/shim/`, and wgpu-hal's compile options (MSL 3.2, `preserveInvariance`, restricted bounds
  checks), so the generated code matches what wgpu runs. One shader source for both renderers.
- **The renderer** (`src/render/`) does what rc-engine asks of Bevy's: materials with `specialize` (`Material`,
  `Material2d`, `UiMaterial`), the Opaque / AlphaMask phases binned in Bevy's order (pipeline, material, mesh, entity),
  the Transparent phase sorted by mesh-AABB distance + depth bias with ties in Bevy's entity order, render layers,
  sub-camera views, per-target shared depth, MSAA, full-screen passes before and after the UI, the UI (absolute layout),
  sprites, the display-byte effect pass, pre-shadow ordering, and hooks for the shadow volumes and the menu snapshot.
- **Performance:** instanced batches (one draw per run of equal pipeline / material / mesh), bind groups cached by
  asset id, two frames in flight, a fresh GPU buffer per changed storage buffer, a fast path for root transforms.

## Results (Apple M5 Pro, 2026-10-18)

Frame dumps of both renderers at `RC_RESOLUTION=416`, compared with `tools/metal/compare.sh`:

| Run | Result |
|---|---|
| Novalis boot, frames 4–240 | every frame ≥ 99.92 % of pixels within 2 levels; ~97 % byte-identical on average |
| All 19 levels, frame 90 | 98.25–100 % within 2 levels; no crash |
| Gameplay with scripted input, pause menu | ≥ 98.9 % within 2 levels (except frame 484, below) |
| Front end, title screen | same picture; their timing is asynchronous, so not frame-exact |
| Level switch (`RC_METAL_SWITCH=60:2`) | works |
| Audio (10 s WAV dump) | byte-identical |

Speed (release, no vsync, Novalis): ~3.6 ms CPU per frame (update ~1.1 ms, encode ~2.5 ms), GPU wait 0; the
display caps it at 120 fps. `rerac` on wgpu: 159 fps.

## iOS

`rerac-metal` also runs on iPhone and iPad (iOS 18+, the MSL 3.2 that wgpu-hal's options ask for):
`tools/package/ios.sh` builds `dist/ios/sim/ReRAC.app` for the Simulator, or with a signing identity and a
provisioning profile `dist/ios/device/ReRAC.app` and `dist/ios/ReRAC.ipa` (usage in the script's header). The code is
the macOS code; what differs:

- **Window:** winit on a UIKit view, the whole screen (winit's inner size on iOS is only the safe area), landscape,
  always synced to the display.
- **Input:** controllers and the hardware keyboard through Apple's GameController framework
  (`engine/game_controller.rs`; gilrs has no iOS backend), and an on-screen PS2 pad (`engine/touch.rs`, drawn by
  `touch_controls.rs` through the renderer's `Hook::Window`). The pad is the device in use from the start and again
  after any touch; a controller takes over once it is used (an idle one, like the Simulator's virtual controller,
  does not).
- **Data:** not in the bundle. Default data folder `Documents/data` in the app's container (the Finder and the Files
  app show it; `UIFileSharingEnabled`): copy the launcher's `games/rac1/data` there. `RC_DATA_DIR` still wins
  (Simulator: `SIMCTL_CHILD_RC_DATA_DIR`). Saves and settings go to the container's `Library/Application Support`.

Checked 2026-10-18 in the iOS 26.5 Simulator (iPhone 17 Pro): front end, movies, title, gameplay with HUD at 60 fps.
Not yet run on a physical device (no signing identity on the build machine).

## Known differences

- **Frame 484 of the scripted gameplay run:** Bevy's UI hides the pause menu one frame later than rc-metal when it
  closes (Bevy's UI extraction runs a frame behind the change). One frame, then identical again.
- **Foliage edges:** a few pixels at overlapping alpha-tested shrub edges differ where Bevy's internal bin order for
  equal keys is not reproducible from outside.
- **Main camera in sway / cull:** rc-metal names the main camera explicitly (`game_camera::first_camera3d`) where
  rc-engine takes the first `Camera3d` in archetype order. Same camera in every case tested.
- Not ported because the renderer does none of it: tonemapping, debanding, HDR, clustered lights, indirect drawing
  (rc-engine disables or does not use them).

## Launcher

macOS packages (`tools/package/package.sh`) ship `rerac-metal` next to `rerac` and name it with the optional
manifest key `"runtime_metal"` (docs/plan/launcher_contract.md). The launcher offers it under Settings → While
playing → **Native Metal renderer**; the default stays `rerac`.

## Switches

All of rc-engine's (`RC_*`, `--data-dir`). rc-metal's own: `RC_METAL_PROFILE=1` (mean update / encode / GPU-wait
times every 5 s, and per-system times), `RC_METAL_ITEMS=1` (per-camera draw lists), `RC_METAL_SWITCH=<frame>:<level>`
(switch level at a frame, for testing).

## Keeping it in step with rc-engine

A change to an rc-engine module is repeated in the rc-metal module of the same name (`metal_porting.md`); a change
to a WGSL shader needs nothing (both compile the same file); a new system goes into `main.rs::schedule` at the place
Bevy's dumped order puts it. Then `tools/metal/compare.sh` on the affected scene.
