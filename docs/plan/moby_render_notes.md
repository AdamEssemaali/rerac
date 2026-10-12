# Moby renderer, first pass (bind pose)

Code: `crates/rc-formats/src/gameplay.rs` (instances), `crates/rc-formats/src/moby_light.rs` (rotation
and lighting, bit-exact), `crates/rc-engine/src/moby_light.rs`, `crates/rc-engine/src/moby_render.rs`,
`crates/rc-engine/assets/shaders/moby.wgsl`. Background: docs/plan/moby_skinning_lighting.md.

## 1. Instance record (gameplay pointer 0x44, 0x78 bytes)

Section: `s32 static_count; s32 spawnable_count; s32 pad[2]`, then the records. The level loader
(level01 `FUN_00255958`, the loop after `piVar27[0x11]`) walks `static_count` records, stepping by each
record's `size`, and turns each into a 0x100-byte runtime moby (`InitMobyInstance` 0x20c5f0 first):

| off | field | loader use (moby+X) |
|---|---|---|
| 0x00 | size (0x78) | record stride |
| 0x04 | unknown_4 | +0xb0 (byte); index into 0x15fc88 in the spawn test |
| 0x08 | spawn flags | spawn test (below); 0x10 → +0xb1 from `FUN_0029ab50` |
| 0x0c | spawn id | +0xb2 (s16); bit index for the save-state tests |
| 0x10 / 0x14 | unknown | +0xb4 / +0xb6 (s16; halved by some spawn tests) |
| 0x18 | o_class | `InitMobyInstance(moby, o_class)` |
| 0x1c | f32 scale | +0x2c = class scale (class 0x24) × scale |
| 0x20 | draw distance | +0x32 (s16). **An integer** (64 on Novalis), not the f32 Wrench declares |
| 0x24 | update distance | +0x30 (byte) |
| 0x28 / 0x2c | 32 / 64 | unused |
| 0x30 | f32 position[3] | +0x10 |
| 0x3c | f32 rotation[3] | +0x40 (Euler, radians) |
| 0x48 | group | +0x21 (byte) |
| 0x4c / 0x50 | is_rooted, rooted distance | z = ground (`FUN_0026e618`) + distance when set (0 instances on Novalis; 311 on 13 levels; ported 2026-10-12: `rc_game::moby_update::scheduler::root_instances` at the level load) |
| 0x58 | pvar index | +0x78 |
| 0x5c | occlusion | 0 → +0x36 = 0, else 0x7f80 |
| 0x60 | mode bits | OR'd into +0x34 and into class header +0x44 |
| 0x64 | s32 r, g, b | `pack_render_command_fields(moby, b<<16 \| g<<8 \| r, 0,0,0)` → +0x3c..0x3e = **ambient** |
| 0x70 | light | stored as an s32 at +0x38: set 0, set 1, cross-fade byte |
| 0x74 | unknown_74 | ≠ −1 → `FUN_0025e7b0(moby)` |

`fun_0020def8(moby)` then builds the rotation rows and the bounding sphere (+0x00).

Novalis: 983 records; ambient is never the `InitMobyInstance` default 0x40 (41,41,41 on 526, 43,47,52 on
181, …); light sets 0–4, and 10 instances with light 0xf0f (set 15, which is empty: ambient only).

**Spawn conditions.** Flags 0x08 ≠ 0 make the spawn depend on game state (bits in the save at
0x14c190, a table at 0x1ba950, a per-uid table 0x1bbb04, 0x15fc88). Ported: the loader's test
(0x256890..0x256a58) is `rc_formats::moby_spawn::spawn_test` / `loader_spawns`; the engine hides the entities of
the instances it rejects (Novalis first arrival: 929 of 983 created, the 54 flag-2 577s not) and drives the renderer
through the moby → instance map. Details: `trace_results_novalis.md` "Fixes applied" #1.

## 2. Rotation convention (verified in the disassembly)

`fun_0020def8` does `lqc2 vf1, 0x40(moby); vcallms 0x1a3` (VU0 28259 at 0xd18), then stores vf20..vf22 at
moby+0xc0/0xd0/0xe0 (negating vf21.xyz when mode bit 0x8000 is set; mode 0x100 keeps the old rows). The
microprogram starts with the identity rows and, for each angle whose `add` sets no MAC zero flag, applies:

- x: rows 1, 2 ← (0, c, s), (0, −s, c)
- y: rows ← Y0·row.x + Y1·row.y + Y2·row.z, Y0 = (c, 0, −s), Y1 = (0, 1, 0), Y2 = (s, 0, c)
- z: same with Z0 = (c, s, 0), Z1 = (−s, c, 0), Z2 = (0, 0, 1)

Row i is the image of model axis i (MobyProc multiplies `M·v = r0·v.x + r1·v.y + r2·v.z`), so
**R = Rz·Ry·Rx** (x applied first), right-handed, angles counter-clockwise. This agrees with Wrench's
`T·S·Rz·Ry·Rx`. sin/cos: range-fold `max(min(a, π−a), −π−a)` then `x − x³/6 + …` to x⁹ with the constants
at 0xed0–0xf10 (cos = sin(a + π/2)); valid for |a| ≤ 3π/2 (Novalis angles lie in −5.18..π; −5.18 folds
correctly). Ported exactly in `moby_light::rotation_rows` on the PS2 float model.

World placement (MobyProc per-moby VU1 matrix): world = s/1024 · R · packed + p, s = moby+0x2c.

## 3. Lighting (verified in the disassembly)

`moby_light::moby_lights` = MobyProc 0x211d88..0x212140: L_k = −Rᵀ·dir_k per component
(`(−r_i.x·d.x + −r_i.y·d.y) + −r_i.z·d.z`), job rows (L0[j], L1[j], 0, 0), colours with w cleared,
K = colour w, ambient `0x47800000 | byte`. The cross-fade path (byte 2 of the light word) is ported too
(renormalised with rsqrt after the rotation); unused on Novalis. `light_vertex` = VU0 104691 single-matrix
loop: n = (ca·ce, sa·ce, se), n′ = M·n, d = rows·n′, f = max(d, −|K|·d), c = (Σ C_k f_k)·rsqrt(|n′|²) + A,
then the EE pack (low halfword × multiplier, s16 saturation, >> 7). Duplicates take the colour cached for
their 9-bit id (carried across the packets of the LOD list), as the game does.

Unit tests cover the convention and values; there is no PCSX2 dump of moby vertex colours yet, so
bit-exactness on real data is **inferred** from the op-by-op port, not measured.

## 4. Renderer choices

- High LOD always (`lod_trans` switch not ported). Bind pose: identity joint palette.
- One mesh per distinct (class, rotation rows, light word, ambient) and texture; one entity per instance
  and texture with the instance transform (`game_to_bevy` conjugated).
- Textures: moby table, `Rgba8Unorm` with the raw GS alpha, **repeat** (every Novalis moby ad-gif has
  CLAMP_1 = 0, used raw since `patch_moby_gifs` only patches TEX0/MIPTBP1), bilinear, no mips (TEX1 words
  like 0xff92/4 are not raw registers; their rewrite is unknown).
- Shader: copy of the tfrag one (MODULATE, fog, As). The moby VU1 program's fog lanes are assumed to hold
  the same view-context terms as the tfrag block (`maxx.w`/`mini.w` clamps) — inferred, not traced.
- Both faces drawn (no culling), like the GS.

## 5. Not done

(The 2026-09-26 list. All of it is done since — §7–§9, moby_skinning_lighting.md, `rc_game::moby_update` — except
the leftovers filed in gaps.md G-REN-018 (render) and G-CLS-018 (spawn conditions).)

- Animation: keyframe decode (`fun_0020e0e0`), palette upload, GPU skinning.
- Metal/chrome pass (−2) and glass (−3) triangles: skipped (none in Novalis high-LOD packets).
- Point-light merge (third light), distance fade/cull against the draw distance (+0x32), alpha < 0x80
  paths, glow (mode 0x10; done 2026-09-28, §9), deferred/shadow (0x400/0x800), low LOD, `is_rooted` ground snap, spawn
  conditions, `unknown_74` hook, moby update code (mobys never move).
- Mip maps / TEX1 filtering.

## 6. Novalis numbers (2026-09-26)

983 instances; 918 placed, 65 whose class has no geometry blob; 169 classes parsed, 65 used; 527,361
high-LOD triangles drawn; 0 chrome / 0 glass triangles in high-LOD packets (they live in the metal
packets), 144 triangles with texture −1 or an unused slot skipped; 380 distinct lit colour sets →
826 meshes, 1712 entities, 139 images. Parse 5.5 ms, lighting 22 ms, mesh build 26 ms; ~57 fps
(M4, dev build, together with the tfrag pass). All instance positions lie inside the tfrag bounds.

## 7. LOD, draw distance, fade, texture −1 and the metal pass (2026-09-27)

Spec: docs/plan/moby_skinning_lighting.md §9. Code: `crates/rc-engine/src/moby_lod.rs` (MobyProc replay, unit tests and
two disc tests), `moby_render.rs`, `assets/shaders/moby.wgsl`, `assets/shaders/moby_metal.wgsl`, `gs_state.rs`
(`GsPass::LateTested`).

- **Per frame** (`update_moby_occlusion`, PostUpdate before visibility propagation and the asset-event flush), per placed
  instance: occlusion bits, then `moby_lod::moby_proc` on the animated sequence sphere (`seq_sphere` + `world_sphere`):
  draw-distance (min(+0x32, 500) on the centre depth) / near / side-plane culls, LOD, fade alpha, shine alpha. Results go to
  a `MobyLod` storage buffer (binding 7 of `MobyMaterial`, binding 6 of `MobyMetalMaterial`; 64 bytes per instance:
  vertex alpha, tint flag, shine alpha, E rows), written only when it changes.
- **Entities.** Per instance four groups — high, high fading, low, low fading — and the metal entities; the pick makes one
  group visible (`Visibility` toggled only on change), so the number of draws is that of one LOD. The fading groups use
  the TEST_1 0x5308b split (AREF 0x08, `GsPass::OpaqueTested/ColorOnlyLowAlpha { aref: 8 }`). A low LOD whose class byte 9
  is 0 is built with joint count 0 and moby.wgsl draws it with the identity (the game's job joint count 0).
  `AnimInstance::visible` = drawn (the game's +0x31), so culled mobys are not evaluated.
- **Texture −1** (docs/plan/moby_untextured.md): drawn as its own batch with a 1×1 (0x80, 0x80, 0x80, 0x80) image, clamp
  (GS block 0x3ffb's 8×8 0x80808080 texture): colour = lit vertex colour, alpha = vertex alpha. High and low LOD.
- **Metal pass.** One mesh per (class, −2/−3) from the metal packets (position + skin word), one entity per GS pass;
  `moby_metal.wgsl` skins like the base pass (same palette, ≤ 3 joints), computes the metal colour and the sphere-map ST
  (§9 formulas) and adds the Z bias `1000·n/(1024·2^24)` to clip z (exact in `GameProjection`). Textures: the level's
  chrome / glass maps decoded from gs_ram (`LevelMobys::env_maps`), clamp, bilinear. GS passes: the TEST_1 0x5360b split
  with the Z-writing half as `LateTested` (Transparent3d with Z write), so it lands on the moby's own AlphaMask3d pixels
  as in the game's per-moby packet order. Metal materials get the frame fog from `push_metal_fog`.
- **Extras** (moby_attach: wrench, Clank, back pack; gameplay.rs: Ratchet): drawn at high LOD with α 0x80 as before; their
  metal pass (wrench, Clank) gets shine alpha and E from `update_extra_metal` (Last schedule) using the record's model
  matrix and the **class header sphere** (their sequence state is private to moby_attach: an approximation of the sphere
  centre, which only moves E slightly).
- **Environment.** `RC_MOBY_LOD=0`: no LOD, culls or fade (every occlusion-passing instance high, α 0x80; the old path).
  `RC_MOBY_LOD_TINT=1`: low-LOD instances red. `RC_MOBY_METAL=0`: no metal pass. `RC_MOBY_CPU_LIGHT=1` implies no LOD.
- **Novalis at start-up**: 532,519 high-LOD triangles per full draw (144 texture −1), 121,579 low-LOD, 20,158 metal
  (5,546 chrome + 14,612 glass); 8,478 entities (1,963 / 3,460 / 961 / 1,510 in the four groups, 584 metal), 269 meshes.
  At the Ratchet camera (`RC_PLAY=0`): 847 culled (62 draw distance, 528 near/behind, 257 frustum), 36 high (+14 fading),
  10 low (+12 fading), 9 metal. Wrench and Clank show the chrome map.
- **Not modelled:** the snapshot sphere (seq A = 0xff: key B's sphere is used), the guard-band MSCAL 0x0e/0x0a choice
  (the GPU clips), the moby Z offset 8388096 vs the tfrag 8388112 (16 Z units), the extras' culls and distance fade.
  Since modelled: mode 0x200 and bit 3 (§8, `MobyBlend::pick`), the deferred/shadow path per moby (2026-10-05,
  `CasterTwin`), Ratchet's LOD pick (2026-10-05; the attachments' classes have no low LOD).

## 8. Translucent and additive mobys, and the point-light merge (2026-09-28)

**Root cause of the opaque explosion.** The game draws every moby with a vertex alpha of `(fade · moby+0x23) >> 7`
(MobyProc 0x26a7a0, the ambient α lane; §7 / moby_skinning_lighting.md §9) and switches its blend per moby from the
mode bits (+0x34: 0x200 additive, 8 "fading"). The explosion flashes (class 1192 from the Bomb Glove, `0x309a68`; class
0x70 from crates and creatures, `FlashSpawn` 0x2c20e0) are spawned with +0x23 = 0x20..0x40 and fade it to 0 in their
second half (`FlashUpdate` 0x2c22a8); in the game their pixels (As < 0x60) blend over the scene without writing Z. The
port drew the scheduler-created (dynamic) mobys with a fixed alpha 0x80 and no fade, and the driven statics with 0x80
too, so a flash rendered as a solid, lava-textured ball hiding the scene and the fire inside it.

**The fix (general, no class special-cased; `moby_render.rs` `MobyBlend`).** Each moby's blend is chosen from its own
data, with native render modes:

| moby data | port draw |
|---|---|
| vertex alpha 0x80, no flags | the regular moby draws (unchanged) |
| distance fade (or mode bit 8) | the existing fade draws (unchanged) |
| +0x23 below 0x80 | **alpha blend, depth tested, no depth write, sorted back to front** (`GsPass::BlendNoZ`) |
| mode bit 0x200 | **additive** (`src·α + dst`), depth tested, no depth write, sorted (`GsPass::AdditiveNoZ`) |

Each (instance, LOD, blend) is one entity group: the high/low plain and fading groups are spawned at load as before;
a translucent or additive group is spawned the first time an instance needs it (statics: `update_moby_occlusion`;
dynamic slots and other extras: `ExtraMobys::show_slot`, which gameplay.rs's `upload_dynamic` now calls). The vertex
alpha goes to the `MobyLod` record of every drawn moby (statics from `MobyOcclusion::look`, fed by `drive_statics`;
dynamic mobys from MobyProc's fade × +0x23, which they did not get before). The blended phase sorts by the entities'
translation, so dynamic and driven entities now carry the moby's position (they sat at the origin); ties are broken by
entity (determinism.rs). Z is tested against the scene, so a translucent shell is cut by the opaque geometry in front
of it and shows everything behind it.

**Not a GS model.** The game's per-moby TEST/ALPHA words are the reference for *which* result to produce; the port does
not add register words or a two-draw split for these modes (in the game a translucent moby's pixels above As 0x60
would write Z; none of the effect mobys reaches that).

**Point-light merge** (MobyProc, moby_skinning_lighting.md §2 "Point lights"; `point_light_merge` / `write_point_light`).
The explosion light (class 639) owns one of 8 point-light slots (`rc_game::point_lights`, `WritePointLight_B` 0x252750);
after each tick gameplay.rs publishes the bank (`PointLightFrame`), and every moby whose sphere centre lies within a
light's radius gets it as its third light (`C_2 = Σ a_j·colour_j`, `a_j = 1 − δ_j/R_j`, `L_2` = the weighted model-space
direction, K_2 = 0) in its `MobyInst` record: statics in `update_moby_occlusion`, dynamic mobys and Ratchet in
`upload`. The GPU lighting was already three-light, so nothing else changed. Not done: the attachments (Clank, packs,
the hand item: crate::moby_attach), and the world (tfrag / tie / shrub relight of the instances in range,
`LightTfrags` / `LightTies` / `LightShrubs` point-light pass; tfrag_lighting.md): the explosion lights mobys only.

**Checked on screen** (`RC_SCENE=0 RC_GIVE_ITEMS=10 RC_HERO_AT=143.42,125.81,57.0,0 RC_PLAY_SCRIPT='20-21:press CIRCLE'`):
before, frames 50 and 65 show an opaque orange ball; after, translucent nested shells (the outer size-4 flash, the
inner 3.5 / 3 ones) with the fire and the world visible through them, the type-4 fire trails arcing off, and the nearby
mushrooms lit orange by the light. With only the blend fix (before the particle types), frame 40 (before the explosion) was
byte-identical to the old renderer; two runs of frame 50 give identical PNGs. The flyers' altitude fade (+0x23 between 0 and
0x80) now also shows as translucent (a few pixels in the flyer-trail view change; the additive trail blend is unchanged).

## 9. The glow list, the metal pass of dynamic mobys, and the shine inputs (2026-09-28)

* **Glow.** MobyProc's glow list (mode 0x10) is ported for every moby: the packets from class byte 0xa (high) / 0xb
  (low) on are drawn in the moby's +0x90 RGB at its vertex alpha (docs/plan/moby_skinning_lighting.md §10).
* **Metal on dynamic mobys.** `ExtraMobys::show_slot` (the scheduler's dynamic slots) drew no metal pass, so every
  runtime-created moby with metal packets was drawn flat: the bolts a crate drops (13–16, glass), weapons'
  and effects' metal classes (19 metal classes appear on some level only as runtime mobys). `upload_dynamic` now runs
  the shine gate on the moby's own +0x73 (`moby_lod::moby_proc`) and the basis E, and the slot shows the class's metal
  entities (spawned on first use) while the shine alpha is above 0. Statics take their shine distance from +0x73 too
  (`MobyLook.shine_distance`: the bolts' update sets 0x14, `InitMobyInstance` 0x18).
* **The logo's flat ball ("wrong inputs").** The Gadgetron logo 1143 (7 chrome packets, +0x73 = 32) is drawn by
  crate::vendor_render through `ExtraMobys::spawn`; `update_extra_metal` rebuilt the moby rows from the record by
  dividing by the **class** scale, but the logo is drawn at 2.5× its class scale (vendor 0x2bb128), so the rows were
  2.5 long, `st = (E·n' + 1)/2` ran far outside [0, 1] and the clamped map gave one flat beige colour. The rows and
  scale now come from the record itself (`record_placement`): the ball is the dark, glossy chrome of the original.
