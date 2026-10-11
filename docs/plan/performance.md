# Performance investigation (dev build, M4, 2026-09-26)

Read-only profiling of `cargo dev` (Bevy 0.19, dynamic linking, `[profile.dev]` opt-level 1 for our crates,
3 for dependencies) on Novalis. Goal: hold 60 fps. Nothing under `crates/` was changed.

## 1. Method

- **Runs.** `cargo dev` with `RC_SCENE=0 RC_NOVSYNC=1 MTL_HUD_ENABLED=1 MTL_HUD_LOG_ENABLED=1`, one engine
  process at a time. Two cameras: the spawn play camera (default) and the far view
  `RC_PLAY=0 RC_CAM=160,40,95,180,380,60`. fps comes from the engine's own `fps:` line (5 s windows, the first
  one dropped). Frame time and **GPU time** come from the Metal HUD log (`/usr/bin/log show`, lines
  `metal-HUD: n, _, _, (frame ms, GPU ms)…`; medians over the steady window).
- **Paired A/B.** Every toggle run came right after its own base run (the base is re-measured for each pair),
  3 rounds, median of the per-pair deltas. The build changed under the runs (other agents), so only paired
  deltas mean anything.
- **CPU.** `sample <pid> 5` at t = 12 s (the call tree on each thread, parsed by scratch scripts) plus
  `xctrace` Metal System Trace (encoder list only: attach mode records no GPU intervals). 1 sample ≈ 2.7 ms
  of one thread; at about 54 fps, **samples/100 ≈ ms per frame**.
- **Noise (read before trusting a number).** Load averages were 6.5–11 during the runs (rustc and Ghidra/Java
  from other agents, other agents' engine runs). Thermal state was "Fair". The same base config at the
  spawn camera measured **42–78 fps (median 62, 14.7 ms frame, 12.3 ms GPU, n = 18)**. When the window is not
  composited (occluded, display asleep), presentation drops to about 60 fps and the HUD stops logging.
  Those 9 of 42 toggle runs are discarded. An engine left over from an earlier run, plus another agent's
  capture running alongside, brought the base down to 33–36 fps. The user's 30–35 fps is consistent with
  concurrent engine instances or builds, the ~25 s arrival cutscene (it now starts on load) and heat.
  Earlier runs (before the cutscene landed) gave 50–73 fps at the spawn camera.

## 2. Per-layer cost (paired deltas: toggle minus its own base; negative = the layer costs this much)

| toggle | spawn Δframe | spawn ΔGPU | far Δframe | far ΔGPU | valid pairs | reading |
|---|---|---|---|---|---|---|
| base (absolute) | 14.7 ms (62 fps) | 12.3 ms | 11.9 ms (81 fps) | 10.3 ms | 18 / 10 | GPU ≈ frame at best; CPU spikes above |
| `RC_NO_TIES=1` | −5.0 | **−2.4** | −3.1 | **−1.9** | 2 / 2 | largest single layer, consistent |
| `RC_NO_SHRUBS=1` | −2.5 | −1.8 | −0.9 | −0.6 | 3 / 3 | second |
| `RC_WATER=0` | (+5.8, noise) | −1.3 | – | – | 2 | GPU cost is real; frame delta not credible |
| `RC_SKY_STARS=0` | −1.0 | ~0 | – | – | 2 | CPU only (mesh rebuild, §4 F5) |
| `RC_ANIM=0` | −1.7 | ~0 | – | – | 2 | main-world CPU (palette, §4 F6) |
| `RC_MOBY_LOD=0` | **+5.3** | ~0 | – | – | 3 | all mobys drawn high: per-draw CPU cost, 3/3 consistent |
| `RC_OCCL=0` | +4.5 (mixed) | +0.8 | +0.2 | 0.0 | 2 / 3 | occlusion pays off near spawn, not at far view |
| `RC_INDIRECT=1` | +5.9 | −1.9 | – | – | 1 | no win; keep indirect off |
| floor: no ties, shrubs, stars, water, HUD, audio | −7.5 (116 fps) | −5.3 | −5.5 (148 fps) | −2.8 | 1 / 2 | what is left: sky, tfrag, mobys, particles, MSAA |
| `RC_HUD=0`, `RC_AUDIO=0` | invalid runs | | | | 0 | audio mix is 0.3 ms/frame in the sample |

## 3. CPU vs GPU split (sample at the spawn camera, ~54 fps, 18.5 ms frame)

Bevy runs **pipelined**: main world (our systems) ‖ render world (extract → prepare → encode → submit) ‖ GPU.
- **The render thread is the critical path: 17.3 ms/frame busy (93%).** The main thread spends 43% of its time
  blocked in `renderer_extract` waiting for it. GPU 11–12 ms. With the CPU fixes below, the GPU becomes the limit.
- Render thread, per frame: **wgpu `CommandEncoder::finish` 7.7 ms** (Metal encoding of the recorded passes:
  `set_bind_group` → `set_buffer` 3.4 ms, `draw_indexed` 0.8 ms). Of this, **5.0 ms is `objc2::msg_send_check`**
  (3.3 ms in `verify_method_signature`): objc2 checks every Objective-C message's type encoding when
  `debug_assertions` is on, which `[profile.dev]` enables for **all** dependencies. `queue.submit` + wgpu
  `maintain`: 1.7 ms. Waiting for parallel render-world systems: 6.5 ms, including prepare assets (below),
  pass recording (1.2 ms) and extract (1.1 ms).
- xctrace encoder list (per displayed frame): `main_opaque_pass_3d` + `main_transparent_pass_3d` 4.4 ms
  driver-side encode, **`msaa_writeback` ×2** (MSAA is on), 5 Bevy light-clustering compute passes per camera
  (no Bevy lights are used), 2.7 upscaling blits.
- Main world (4 compute threads, 7.3 ms of CPU per frame self time in the `rc-engine` binary, i.e. our three
  crates plus Bevy generics instantiated for our types): `moby_anim::upload_palette` 3.1 ms,
  `check_entities_needing_specialization` (×10 material types) 2.5 ms, gameplay tick 1.9 ms (statics 0.7,
  particles 0.45), visibility and bounds 0.8 ms, audio 0.3 ms, particle sort and build 0.1 ms,
  tfrag/tie/moby LOD replays under 0.3 ms together, occlusion negligible. **Our PS2-emulation code is cheap.** The
  cost is in Bevy/wgpu overhead that we trigger (asset churn, draw count) and in byte shuffling.
- GPU (HUD): 10.3–12.3 ms. Ties 2 ms (their culling happens only in the vertex shader, see F3), shrubs
  0.6–1.8 ms, water ~1.3 ms, and a ~7 ms floor. **The main and sky cameras use Bevy's default
  `Msaa::Sample4`** (only the HUD camera sets `Msaa::Off`), on a Retina window: 1024×832 logical = **2048×1664
  physical**, 16× the GS's 512×416. The main camera loads colour and depth (`ClearColorConfig::None`,
  `DepthLoadOp::Load`), so each MSAA pass loads and stores 4-sample colour and depth (~54 MB each) plus a
  writeback and a resolve. This is estimated at 2–4 ms of the floor (bandwidth arithmetic, not measured:
  no switch exists).

## 4. Hotspots and fixes (ranked by gain ÷ effort)

| # | where | cost now | cause | fix (exactness kept) | expected gain | effort |
|---|---|---|---|---|---|---|
| F1 | dependency profile (`Cargo.toml`) | **5.0 ms render thread** | objc2 debug-assertion message verification in wgpu-hal Metal calls | `[profile.dev.package.objc2] debug-assertions = false` (matches objc2 0.5 and 0.6). **User decision: one-time rebuild of ~78 dependents incl. wgpu, bevy_render, bevy_dylib (a few minutes)**; release builds never had it | render thread 17.3 → ~12.3 ms (+~25–35% fps when CPU-bound) | trivial, needs approval |
| F2 | `main.rs` setup, `sky_render.rs` sky camera | est. 2–4 ms GPU | default 4× MSAA on both world cameras | `Msaa::Off` on every camera sharing the window. More faithful (the GS has no MSAA); removes writeback and resolve. Add an `RC_MSAA` A/B switch and screenshot-diff | GPU −2..4 ms (to verify) | small |
| F3 | `tie_lod.rs` (`update_tie_lods`), `tie_render.rs` | ~2 ms GPU + draws | culled and occluded ties (1158 of 1508 at spawn) stay `Visible`; the VS discards them, and runs all 3 LODs' vertices for every instance | set `Visibility::Hidden` on an instance's part entities when TieProc culls or occludes it, changed only on transitions (as `update_moby_occlusion` does). Optional: per-LOD meshes chosen by visibility, so the VS processes one LOD | GPU −1.5..2 ms, fewer instances per batch | medium |
| F4 | `water_render.rs` (`draw`, `set_params`) | 1.6 ms render-world critical path | every `WaterMaterial` is `get_mut`'d each tick (scroll/wobble/z; the ripple patches' no-op closure too) → Bevy re-prepares each one: new uniform buffer + bind group | keep materials immutable; put the per-tick params in one `ShaderBuffer` indexed per draw (`MeshTag` or a params index); drop the no-op `set_params` | −1.5 ms render thread, fewer buffer allocs | medium |
| F5 | `particle_render.rs` (`build_sprites`), `sky_stars.rs` (`build_meshes`), `water_render.rs` (ripple `patch_mesh`), `hud_render.rs` (`tick_and_build`) | 1.2 ms render (`allocate_and_free_meshes`) + 2.5 ms main (`check_entities_needing_specialization` over ~24k mesh entities × 10 materials, via `AssetChanged<Mesh3d>`) + 0.3 ms | `meshes.insert` of new `Mesh` assets every frame | fixed-capacity meshes (quad index/corner only) with per-sprite data in a `ShaderBuffer`; or at least skip `insert` when nothing changed. Storage-buffer writes do not touch the mesh allocator or specialization | −1.2 ms render, −2.5 ms main CPU | medium |
| F6 | `moby_anim.rs` (`upload_palette`, `evaluate_all`, `identity_palette`, `rows_bytes`) | 3.1 ms main per tick | pose evaluation is already memoised (11 poses, 0.3 ms). 90% is building 543 KB byte by byte with `flat_map` iterators at opt-level 1 (identity palette rebuilt every tick) | keep `Vec<[f32; 16]>` and `bytemuck::cast_slice` (memcpy); cache the identity bytes; overwrite only visible instances' slots; skip the upload when no pose changed | −2.5..3 ms main CPU | small |
| F7 | draw count (all renderers) | `RC_MOBY_LOD=0` +5.3 ms for a few hundred extra draws | each draw is several bind-group and buffer sets, each an objc2-checked `msg_send`; the Transparent3d half of every GS alpha split (231 shrub, 14 tie materials) cannot batch | after F1, re-measure. Then fewer materials and bind-group switches (shared texture arrays), and merged transparent halves where the GS order allows | CPU, size unknown until F1 | large |
| F8 | our crates' opt-level | see §5 | our code and Bevy generics for our types run at opt-level 1 | per-package override, see §5 | main world −2..3 ms | trivial, needs approval |
| F9 | `main.rs` `WindowPlugin` | ~¾ of fill cost | Retina scale 2: 2048×1664 physical | `WindowResolution` with `scale_factor_override(1.0)`: the documented "2× GS" 1024×832 in real pixels | GPU roughly halves | trivial; **presentation choice for the user** |
| F10 | Bevy light clustering | small GPU and CPU | 5 compute passes per camera for lights we do not use | disable clustered forward for our cameras if Bevy 0.19 allows it; otherwise ignore | < 0.5 ms | small |

Not worth doing now: tfrag/tie/moby LOD replays, occlusion, particle sort, audio mixing (each ≤ 0.3 ms).
`RC_INDIRECT=1` (GPU preprocessing) was slower in its one valid pair. Leave it off (it also breaks determinism).

**Projected.** Render thread 17.3 → ~9.5 ms (F1, F4, F5); GPU 12.3 → ~7–8 ms (F2, F3), and ~4–5 ms with F9;
main world ~10 → ~5 ms (F5, F6, F8). About 100 fps at the spawn camera on a quiet machine, which holds 60
with margin under build load.

## 5. Per-crate opt-level (`[profile.dev.package.rc-engine] opt-level = 2|3`): analysis only, not applied

- **Safe for the Bevy cache: verified.** A scratch workspace with the same `[profile.dev]` layout
  (`opt-level = 1`, `package."*"` = 3) and a registry dependency: after adding
  `[profile.dev.package.app] opt-level = 3`, `cargo build -v` reports every dependency **`Fresh`** and only
  `app` compiling. The member gets a new metadata hash, so both variants stay cached, and removing the
  override again is `Fresh` too. Cargo keeps each unit's profile in that unit's own fingerprint and
  metadata; dependents are rebuilt, dependencies are not. The converse was also checked: an override on a
  **dependency** (`debug-assertions` on `bytemuck`) rebuilds it and everything above it. So F1 does
  invalidate Bevy and F8 does not.
- **Scope.** `package."*"` covers only non-members. The hot code lives in all three members: `rc-engine`
  (systems, Bevy generics instantiated for our types such as `check_entities_needing_specialization` and
  material prepare), `rc-game` (tick, particles, audio) and `rc-formats` (`moby_anim`, the `ps2` float
  model). Override all three:
  `[profile.dev.package.rc-engine]`, `[profile.dev.package.rc-game]`, `[profile.dev.package.rc-formats]`
  with `opt-level = 3` (2 performs about the same and compiles no faster: see below).
- **Build cost.** A scratch copy of rc-formats and rc-game, `-j 4`: clean build 10.3 s at opt 1, 13.8 s at
  opt 2, 13.4 s at opt 3 (+30%). Incremental rebuild after a code edit: 2.0–3.1 s at opt 1 against 2.5–2.7 s
  at opt 3 (no measurable difference). rc-engine could not be timed without touching the shared target
  (it needs Bevy). Expect a similar +30% on its clean compile and a few seconds more per edit. One-time cost
  of switching: those 3 crates only, no Bevy rebuild.
- **Gain.** Only main-world CPU gets faster (7.3 ms/frame of self time in the `rc-engine` binary, spread over
  4 threads; typical opt-1→3 speed-up on iterator-heavy code is 2–3×): an estimated 2–3 ms off the main
  world. **Today that is 0–5% fps**, because the render thread (Bevy/wgpu, already opt-3) is the critical
  path. After F1, F4 and F5 the main world is next, so this is worth taking together with F1 in a single
  approved `Cargo.toml` change.

## 6. Brief outline for the fix agents (file ownership, so they can run in parallel)

| agent | files (exclusive) | work | verify |
|---|---|---|---|
| A: profile (after user approval) | `Cargo.toml` `[profile.*]` only | F1 + F8 in one edit, so there is only one Bevy rebuild; report the rebuild time | render-thread sample: `msg_send_check` gone |
| B: cameras | `crates/rc-engine/src/main.rs` (camera spawn, `WindowPlugin`), `sky_render.rs` (camera spawn only) | F2 with an `RC_MSAA=4` switch for A/B; F9 only if the user approves | HUD GPU ms A/B; frame-exact screenshot diff (edges only) |
| C: ties | `tie_lod.rs`, `tie_render.rs`, `assets/shaders/tie.wgsl` | F3 | HUD GPU ms at both cameras; `RC_TIE_LOD_TINT` screenshots unchanged |
| D: water | `water_render.rs`, `assets/shaders/water.wgsl` | F4, and F5 for the ripple patch meshes | sample: no `WaterMaterial` in `prepare_asset`; water screenshots identical |
| E: sprites and HUD meshes | `particle_render.rs`, `assets/shaders/particle.wgsl`, `sky_stars.rs`, `assets/shaders/sky_stars.wgsl`, `hud_render.rs` | F5 | sample: `allocate_and_free_meshes` and `check_entities_needing_specialization` near 0 |
| F: moby palette | `moby_anim.rs` | F6 (keep the memo and the pose rules) | `moby anim: evaluation … ms per tick` line; `RC_ANIM` screenshots identical |
| later | many | F7, F10: after re-measuring with A–F landed | |

B–F touch disjoint files and can run in parallel. A changes the build and should land when no agent is
mid-build. Every verification: `RC_SCENE=0 RC_NOVSYNC=1 MTL_HUD_ENABLED=1 MTL_HUD_LOG_ENABLED=1`, one engine
at a time, window visible, paired base/toggle runs, HUD `metal-HUD:` medians and `sample <pid> 5`.

## 7. Applied (2026-09-26): F2 (MSAA), F3 (tie CPU culling), F5 (particles, stars, water ripples), F4, F6

Not applied: F1/F8 (`Cargo.toml`, user decision), F5 for the HUD mesh (applied 2026-10-07, §8), F7, F9, F10.

| fix | change | files |
|---|---|---|
| F2 | `render_settings.rs`: `RenderSettings { msaa: Msaa }` resource (default `Msaa::Off`, start value `RC_MSAA=0\|2\|4\|8`), applied by `render_settings::apply` to every `WorldCamera` (main + sky, together: the main pass loads the sky pass's depth) when the resource changes or a camera spawns. A menu option only needs `ResMut<RenderSettings>`. `RC_MSAA_SWITCH=<frame>:<samples>` exercises the run-time path. | `render_settings.rs`, `main.rs`, `sky_render.rs` |
| F3 | `update_tie_lods` sets `Visibility::Hidden` on the part entities of instances `TieProc` culls or occludes (only on state changes), ordered before `VisibilityPropagate` so it applies the same frame. Rules unchanged; the VS `LOD_CULLED` test stays. `RC_TIE_CPU_CULL=0` restores the old path. | `tie_lod.rs`, `tie_render.rs` |
| F5 | Particles and sky stars: one persistent quad mesh (`particle_render::quad_mesh`, vertex = sprite << 2 \| corner) + a `SpriteBuffer` storage buffer per group rewritten each frame (count, centre, corner vectors a/b, RGBA, tag); quads past the count are dropped in the VS. Water: `WaterMaterial` is no longer mutated per tick (fog only on change); per-slot w/scroll/wobble in a `frame` storage buffer, ripple patches read a `ripple` storage buffer (mask, 17×17 vertices, 46 water UVs) through one static mesh of all 16 sub-blocks. | `particle_render.rs`, `sky_stars.rs`, `water_render.rs`, `particle.wgsl`, `sky_stars.wgsl`, `water.wgsl` |
| F6 | `upload_palette`: persistent palette bytes, cached identity matrix, per-pose bytes built once, `copy_from_slice` per instance, only changed bytes touched (stale instances reset to identity first), upload skipped when nothing changed. | `moby_anim.rs` |

**Measured** (spawn camera, `RC_NOVSYNC=1`, Metal HUD medians of steady frames; paired back-to-back runs of a
frozen pre-change binary and the new build; load 4.9–5.9; round 3's base run was not composited and is dropped):

| build | frame ms (per round) | GPU ms (per round) | fps windows (median) |
|---|---|---|---|
| before (MSAA 4) | 13.46, 13.22 | 11.85, 11.34 | ~76 |
| after, `RC_MSAA=4` (F3, F5, F6 only) | 12.09, 12.09, 12.15 | 10.56, 10.69, 10.87 | ~82 |
| after, default (MSAA off) | 10.49, 10.72, 10.71 | 6.49, 6.57, 6.58 | ~94 |

F3/F5/F6 together: −1.2 ms frame, −0.9 ms GPU. MSAA off: a further −1.4 ms frame and **−4.1 ms GPU** (more than
the 2–4 ms estimate). Total ≈ 13.3 → 10.7 ms, 76 → 94 fps; the GPU is now at 6.6 ms, so the render thread
(F1) is the limit again. Per-fix CPU splits were not sampled.

**Pixels** (frame-exact captures, `RC_MSAA=4` against the pre-change build): spawn f120, far view f120, level
06 stars f120 and the pause menu f90 byte-identical. Water surfaces differ in a few pixels (water view
`RC_PLAY=0 RC_CAM=160,82,72,140,110,62`: 185 px, mostly ±1, max 14; particle view
`RC_CAM=141,160,61,150.5,170,57.5`: 234 px ±1, all on the water, particles identical). The water shader's
per-vertex inputs are bit-identical (a one-frame-lag experiment gave 121k px, ruling out timing), so this is
Metal fast-math codegen of the rewritten vertex shader (positions/fog at ULP level). MSAA off vs on: 102,067 px
(3.0 %) at spawn, 55,514 px in the menu, all on polygon edges (38 % differ by ≤ 2 levels). A run switched from
MSAA 4 to off at frame 60 is byte-identical to a run started with MSAA off (run-time switching and
determinism). The menu snapshot, the frame-exact offscreen capture, the HUD composite work in both modes.

## 8. Review of 2026-10-06 (4K and stutter reports): open issues

**Not good enough yet (user, 2026-10-07).** The dev build runs Metropolis (Kerwan) at about 90–100 fps and uses
about 1 GB of RAM. That is playable, but the original ran the same game at 60 fps on a PS2: 32 MB of main RAM and
4 MB of video memory, with a level in roughly 20 MB. A modern machine should do far better on both. Frame time is
covered below. Memory has not been profiled yet: find out where the gigabyte goes (Bevy's per-entity overhead on
~25k entities, CPU-side copies of mesh and texture data kept after upload, decoded atlases, audio, the dev build's
debug info) before deciding what to cut.

A read-only pass after testers reported stutters (Veldin, Novalis, a Windows build). On this Mac the dev build held
65–76 fps at 2160p on Novalis, so pixel count is not the limit. The cost is Bevy overhead the port triggers, not
the ported game code: the tick, the moby loop, the collision kernels and the LOD/occlusion replays stay well under
1 ms per frame, and the moby palette evaluation is 0.1 ms per tick. None of the fixes below changes behaviour or
pixels; they only stop repeated work.

| # | issue | where | cost | fix | status |
|---|---|---|---|---|---|
| P1 | **HUD meshes re-inserted every frame.** Each inserted `Mesh` defeats the short-circuit of `AssetChanged<Mesh3d>`, so every material type's specialization check walks all mesh entities (~25,800 in a level) every frame | `hud_render.rs` `tick_and_build` (main mesh + 3 static passes) | ~2.5 ms main CPU per frame (the Sep 26 `check_entities_needing_specialization`), plus mesh allocator churn | `Built`: a mesh is re-inserted only when its inputs (prims, fine corners, additive list, streamed-image rects, side margin) change | **fixed 2026-10-07** |
| P1b | The same pattern in the effect primitives, whenever they are shown: `meshes.get_mut` per frame | `sea_render.rs` (`g.prims.write`), `fx_draw.rs` (draws, reactive, tesla, walloper), `water_render.rs:925`, `moby_attach.rs` (rope mesh) | as P1, while one is on screen | skip the write when the primitives did not change, or move to a persistent mesh + storage buffer as F5 did for particles | **fixed 2026-10-10** for the draw-callback, fire-field and sea meshes (d24fef8, `fx_draw::PrimBuf::update`); water and the rope mesh open |
| P2 | **Entity and draw count.** Shrubs 9,649 entities (5,624 in the fading list) over 231 blended materials, ties 5,820, mobys 9,215. Every GS alpha split's second half lands in the sorted transparent phase and cannot batch | all world renderers | the largest remaining CPU lever (F7) | fewer materials and bind-group switches (shared texture arrays), merged transparent halves where the GS order allows | open (large); the colour-only halves now draw in the game's order, each class part's instances together (ea4b61c, `pre_shadow::order`) |
| P3 | **First-sight stalls.** A moby class's draw groups are spawned, and their pipelines compiled, the first time it passes the culls. No pipeline pre-warm at level load | `moby_render.rs` spawn path, Bevy pipeline cache | 130–170 ms frames right after load; 28–34 ms frames as new classes appear | spawn every class's draw groups (hidden) at level load so pipelines compile during loading, or pre-specialize their pipeline keys | open; the most likely cause of the reported stutters; now part of gaps.md G-REN-037 (pipeline preparation: the cold-cache missing world / sky too) |
| P4 | **Dev-profile numbers.** Debug assertions in the dependencies (objc2 message checks in wgpu's Metal backend, ~5 ms per frame) and opt-level 1 for our crates | `Cargo.toml` (F1, F8) | dev builds only; release builds have neither | none needed for players. Measure a **current** release build before sizing P2–P5 (the Sep 29 release binary fails shader validation against today's assets: rebuild it) | caveat |
| P5a | Bevy's light-clustering compute passes run for both 3D cameras with no lights in use | camera spawn (`main.rs`, `sky_render.rs`) | small GPU + CPU | Bevy 0.19's cluster config can turn them off (F10) | **fixed 2026-10-10** (cf19080, `no_clusters.rs`; the sky camera's output is skipped too) |
| P5b | tfrag and tie LOD buffers rebuilt and re-uploaded every frame even when nothing changed | `tfrag_*`, `tie_lod.rs` | cheap (in-place uploads), churn at high fps | upload only on change | **2026-10-10**: `asset_write::set_buffer` compares before writing (d24fef8); the LOD records still change every frame |
| P5c | Dynamic moby palettes built byte by byte per tick | `gameplay.rs` (the `DynMobys` palette, ~2309) | small per tick | the F6 treatment: persistent bytes, `copy_from_slice` | open |

Fragment cost at 4K is reasonable: each world shader does one pow-based sRGB conversion per fragment, and the alpha
split batches rasterize twice by design. There are no full-resolution readbacks or extra world passes beyond the
shadow count target and a few full-screen blends. Measurement caveat: `RC_NOVSYNC` was not honoured in fullscreen,
and the windowed run sat at exactly 60.0 fps, so the window-resolution numbers are not a headroom measurement.

**2026-10-10 pass (release build, Novalis, 60 fps cap).** Frame pacing: one game tick per drawn frame, at most 60
frames a second (d9c8b94; at 120 Hz the port drew every refresh: ~145% → ~87% CPU, and no frame carries 0 or 2 ticks).
Three task-pool threads by default (55b2a34, `RC_THREADS`): 28.3 → 21.1 s of CPU over 1,500 frames at 416p; **reverted
2026-10-11** to Bevy's default (`RC_THREADS=3` keeps it as an option): with v0.3.1 a Windows PC on Vulkan (16 cores, NVIDIA
RTX 4080 SUPER, two 4K screens) showed a black window while the game ran; `RC_THREADS=0` or DX12 showed the picture. A clean
`xctrace` Time Profiler run (no GPU capture, no debug layers) then measured ~55–59% of one core: gameplay tick 4.7%,
Bevy frustum culling 3.2%, moby animation and buffers ~3%, Bevy bookkeeping ~2.5%, render thread 12.5%. The
`pre_shadow` split removed one item at a time (O(n²), 2.5 s of CPU per run): one split since ae9e571. Diagnostics:
`RC_DRAW_STATS=1` (draws per material type, cameras, uploads), `RC_RENDER_DIAG=1` (Bevy's per-pass timings). Still
open: P3 (first-sight stalls), memory, P2.

**Open after 2026-10-11:** pipeline preparation (gaps.md G-REN-037, includes P3) and the thread cap's return (gaps.md
G-TOOL-021, after G-REN-037).
