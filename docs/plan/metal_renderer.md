# Native Metal renderer (`crates/rc-metal`)

Status: **phase 1 done** (terrain). Decided by the user on 2026-10-18 (docs/plan/decisions.md): a native Metal renderer
on macOS, **without Bevy**, built next to rc-engine's Bevy/wgpu renderer and compared against it frame by frame.

## Why

rc-engine renders through Bevy 0.19 → wgpu 29 → Metal. The layers cost time on the render thread (wgpu's
`CommandEncoder::finish` alone is 7.7 ms per frame in the Novalis profile, docs/plan/performance.md) and hide the
GPU behind Bevy's phase and pipeline machinery (pipeline variants, G-REN-037). The port draws everything with its own
shaders and its own GS-state model (`gs_state.rs`) anyway; a direct Metal renderer keeps those and drops the layers.

## Shape

- **Own executable** `rerac-metal` (`cargo run -p rc-metal --release`): winit 0.30 window, `CAMetalLayer` through
  `raw-window-metal`, Metal through `objc2-metal` 0.3 (the versions wgpu-hal already locks). No Bevy, no wgpu.
- **Inputs are rc-formats' and rc-data's.** rc-engine's loaders that the renderer needs live in Bevy-dependent modules
  (`tfrag_light`, `tfrag_lod::load`, `game_camera::LevelFog`); `rc-metal/src/level.rs` repeats the few Bevy-free lines.
  Once more than the terrain moves over, those pieces should move into a shared Bevy-free crate both renderers use.
- **Shaders are MSL ports of the WGSL**, one file per WGSL source (`crates/rc-metal/shaders/*.metal`, compiled at
  start-up by the device: no offline Metal toolchain needed). Each keeps the WGSL's derivation comments by reference
  and lists its differences in its header.
- **World space, storage layouts and GS state are rc-engine's** (`game_to_bevy`, the `Slot` / `VInfo` layouts,
  `GsPass`), so a shader port is a translation, not a redesign, and the two renderers can be diffed.
- **Frame:** a centred 4:3 box (rc-engine `display.rs`, `Aspect::Original`) on black, `BGRA8Unorm_sRGB` colour,
  `Depth32Float` reverse Z (clear 0, GEQUAL). Draw order = Bevy's phase order: Opaque, then the alpha-tested half,
  then the colour-only half (`gs_state.rs` explains why that differs from the GS's packet order).

## Phases

| Phase | Content | State |
|---|---|---|
| 1 | Window, fly camera, level load, tfrag terrain: LOD modes (`TfragProc`, CPU), VU1 morph / collapse (vertex shader), GS MODULATE, fog, GS mip rule, alpha-test split; headless screenshot | **done** |
| 2 | Ties, shrubs (+ billboards), mobys (skinning, VU0 lighting, metal pass) | – |
| 3 | Sky, water / sea, particles, FX draw callbacks, point lights (`world_lights`) | – |
| 4 | HUD and text (GS 2D primitives), menus, movies (rc-video frames as a texture) | – |
| 5 | Post passes (AA blit, tints, fades, mirror, visibomb), shadow volumes, display-byte effect blending, occlusion | – |
| 6 | The game itself: rc-game tick, input, audio (today rc-engine's Bevy glue: `gameplay.rs`, `input_map.rs`, `audio_out.rs`) | – |

Phase 6 is what "without Bevy" ultimately costs: rc-engine's ~6k lines of non-render glue (gameplay, level switching,
saves, audio through `bevy::audio`, input through Bevy's `ButtonInput` / gilrs) need Bevy-free equivalents before
`rerac-metal` can replace `rerac`. rc-game itself has no Bevy dependency and emits plain draw lists
(`hud::Draw`, `menus::MenuDraw`, `cinematic::EngineRequest`), which is the seam to build on.

## Phase 1: what is drawn and what is not

Drawn: every tfrag of the level, all three strip lists, with this frame's `TfragProc` decision per tfrag (culled,
clipping path, MSCAL 6 / 8 / 0xa / 0xe / 0x10 / 0x14) and the VU1 morph / collapse in the vertex shader; the
load-time `LightTfrags` colours; GS MODULATE; the level fog; the GS mip-level rule; the level background colour.

Not yet (each a phase above): the occlusion test before `TfragProc` (rc-engine `occlusion.rs`: every tfrag passes it
here), point lights, the sky (the background colour shows instead), the AA blit (rc-engine's default
`RC_GFX_ANTI_ALIASING=original`), every other object type, the game tick.

Numbers (Novalis, Apple M5 Pro): 1004 tfrags, 61,919 LOD-0 triangles, 99,146 vertices, 87 draws (86 texture batches,
texture 35 split by the alpha test), 86 textures; geometry built in ~30 ms, uploaded in ~5 ms.

## Comparing with rc-engine

```sh
# Metal, offscreen, the default framing (or RC_CAM=...)
RC_SCREENSHOT=work/metal.png cargo run -q -p rc-metal
# wgpu, the same view with only the terrain (sky and mobys still draw)
RC_LEVEL=1 RC_FRONTEND=0 RC_PLAY=0 RC_HUD=0 RC_NO_TIES=1 RC_NO_SHRUBS=1 RC_AUDIO=0 \
  RC_GFX_ANTI_ALIASING=off RC_SCREENSHOT_FRAME=30 RC_SCREENSHOT=work/wgpu.png cargo dev
```

rc-engine's screenshot has the window's physical size (2048×1536 on a Retina Mac at the default 1024×768 window); give
rc-metal the same with `RC_SCREENSHOT_SIZE=2048x1536`. Both use the same starting camera (rc-engine `setup_camera`'s
framing, or `RC_CAM`) and the same projection.

Result on 2026-10-18 (Novalis, default camera, 2048×1536): of the 2,779,149 pixels rc-metal draws terrain on,
**92.93 % are byte-identical** to rc-engine's frame. Every differing pixel lies under something only rc-engine draws
yet: the river water (`water_render`), the ships, the bridge and the other mobys; the terrain itself shows no
difference.

## Controls and switches

Right mouse + move to look, W/A/S/D fly, Q/E down/up, Shift faster, wheel changes speed, P prints an `RC_CAM` value,
Esc quits. rc-engine's switches with the same meaning: `RC_DATA_DIR` / `RC_EXTRACTED` / `--data-dir`, `RC_LEVEL`,
`RC_CAM`, `RC_FOG=0`, `RC_NO_LIGHT=1`, `RC_LOD=0`, `RC_LOD_TINT=1`, `RC_NOVSYNC=1`, plus `RC_SCREENSHOT` /
`RC_SCREENSHOT_SIZE` (headless here: no window opens).
