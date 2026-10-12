# Moby skinning and lighting (EE + VU0)

Where the moby vertex work happens before VU1 program 13859 (which only transforms, culls and clips). Addresses are boot ELF `SCUS_971.99` unless marked "L01" (level01.elf overlay). Evidence comes from the Ghidra disassembly of the functions named below (the decompiler output for these hand-written asm routines is unusable), the VU0 disassembly `work/vu/104691.txt` (instruction indices = MSCAL/CMSAR0 units, byte address = index × 8), and a scan of every moby class blob in `extracted/levels/*/core/moby_class/`.

## Summary

1. **Skinning** is done by **VU0 micro program 104691 at entry 0**, driven one vertex at a time by the EE routine **`fun_001ee650`** (unnamed; suggested name `MobySkinLight`). The EE unpacks each 16-byte vertex into VU0 registers, VU0 blends up to 3 joint matrices held in VU0 data memory (Wrench's "VU0 matrix slots" are real), transforms the position and normal, and hands back an integer position and a lit colour through a register handshake. Joint matrices come from **`fun_0020e0e0`** (animation evaluation, suggested name `MobyAnimEval`) in scratchpad. Both run from `moby_anim_proc` (0x211728) in `draw_mobys_clean_up`, i.e. *after* `MobyProc` has built the VIF1 chain. The outputs are DMA'd into holes left in that chain. Confidence: verified in disassembly.
2. **Lighting** is in the same VU0 pass: per-vertex Lambert against **2 directional lights** (one entry of the level's `dir_lights` bank, optionally cross-faded with a second entry) plus **1 virtual light made by merging the point lights in range**, plus a per-moby ambient RGB. Back faces get `|K|·|N·L|` (K is the w of the light colour). Colours are in units where **128 = 1.0** (the 65536.0 float bias trick). The result is multiplied per vertex by a baked RGBA byte (0x80 = 1.0, and it is 0x80 on every disc class) and saturated at 255. Confidence: verified (formulas), inferred (a few field names).
3. **VU0 load order.** 28259 is uploaded once by `init_once` (0x201650, chain 0x10e4c0) and only its 0xC80–0xF6F part is uploaded. The 0x0000–0x002F stub (a 6-instruction `nop :e`, chain 0x10e7c0) has **no code references**, so it is never uploaded. 104691 (0x0000–0x0A3F) is re-uploaded every frame by `draw_mobys_setup`. 436083 (0x0000–0x057F) is uploaded later in the same frame by `draw_debug_profiler` (= DrawWorld). Neither reaches 0xC80, so the overlap does not matter. Confidence: verified (DMA tags read from memory + xrefs).
4. **104691** has 4 entry points: **0x000** is a software-pipelined streaming loop (2-way, then 3-way, then single-matrix vertices, then a 2-vertex drain) that does skinning, normal transform, normalisation and 3-light shading. **0x111 / 0x121 / 0x12D** are single-shot 3-, 2- and 1-matrix versions (called with `vcallmsr` via CMSAR0) used for **metal/chrome ("shine") packets**, which also output a sphere-map ST. Confidence: verified.
5. The **VU1 constant block** has two parts. The per-frame 6 qw at **0x18cd00** are written by **`update_view_context` (0x1f2d98)**; MobyProc uploads them at chain start to VU1 qw 0x123–0x128 (buffer A) and 0x287–0x28c (buffer B). The **per-moby matrix** (vf17–vf20) is built by **`MobyProc` = `fun_00211808`** (L01 `MobyProc` 0x26a7a0) and unpacked to 0x129 / 0x28d with the first two packets of each moby. vf27 is not uploaded: VU1 builds it itself (output buffers 0x2c8/0x348). Confidence: verified.

## 1. Frame flow

`draw_debug_profiler` (0x1f39d0) → `draw_mobys` (0x20d460):
- `draw_mobys_setup` (0x20d278): `vu1_add_data_ref(0x10faa0)` (VU1 program 13859), resident id 6, `start_vif1_dma_transfer(0x100080)` (= `VU0_loadMicroProgram`, uploads 104691), GS TEST_1 = 0x5360b.
- `fun_00211808` **MobyProc**: culls, picks the LOD, builds the VIF1 chain and one **job record** per visible moby (§2).
- `draw_mobys_clean_up` (0x20d3b0) → `dma_moby_textures` → `process_moby_anim_data` (0x20d1a8): copies the 256-entry (cos, sin) table 0x165500 → SPR 0x3800, then `moby_anim_proc(job_list)`. `moby_anim_proc` loops `fun_0020e0e0(job)` then `fun_001ee650(job, vertex_cache)` until the job's first word is 0. The job pointer is kept in FPU reg f0.

The chain is kicked next frame (`VU1_sendChain`), so filling it after it is built is safe.

## 2. MobyProc `fun_00211808` (Lombyte: assembly/textbin/fun_00211808; L01 `MobyProc` 0x26a7a0)

Walks moby instances (0x100 bytes each). At entry it DMAs the light bank **0x19bdc0 → SPR 0x2000** (0x50 qw): 16 directional entries × 0x40 plus 8 point lights × 0x20 at SPR 0x2400. `fun_001e9b10` fills the bank from the gameplay `dir_lights` block (≤ 12 entries), whose layout matches Wrench's `DirLight {col_a, dir_a, col_b, dir_b}`.

**Moby instance fields used** (runtime struct; defaults from `init_moby_instance` 0x20c5f0):

| off | use |
|---|---|
| 0x00 | bounding sphere, xyz = centre × 1024, w = radius (cull, point lights) |
| 0x10 | position (world units) |
| 0x23 | alpha byte (default 0x80) |
| 0x24 | class pointer |
| 0x2c | f32 scale (copied from class +0x24 at init) |
| 0x34 | u16 mode bits (0x10 = glow list, 0x400/0x800 = deferred/shadow path, 0x81 = skip) |
| 0x38, 0x39, 0x3a | directional light bank index 0, index 1, cross-fade b (0..255) |
| 0x3c..0x3e | ambient RGB (default 0x40, 0x40, 0x40) |
| 0x54, 0x60–0x6c | animation state/frame pointers, copied into the job |
| 0x72 | LOD switch distance (class `lod_trans`) |
| 0x73 | shine/metal alpha (0x18 if class has metal packets) |
| 0xc0, 0xd0, 0xe0 | rotation rows r0, r1, r2 (unit, no scale) |

Confidence: offsets verified by use; meanings inferred.

**LOD.** If depth > moby[0x72]·1024, use the low LOD: packets start after `high_lod_count`, count = class[5], joint count = **class[9]**. Otherwise count = class[4] and joint count = class[8]. Class byte 9 (`unknown_9` in moby_rac1.md) is the low-LOD joint count. Confidence: verified.

**Per-moby VU1 matrix.** Let V = the 4×4 at 0x186f80 (`update_view_context` builds it as projection 0x18cdc0 × view 0x186f40), cam = 0x187080, s = moby[0x2c], p = moby[0x10]. Using the VU convention `M·v = M0·v.x + M1·v.y + M2·v.z + M3·v.w`:

    M_vu1 = V · [ s·r0 ; s·r1 ; s·r2 ; (1024·(p − cam), 1) ]      // vf15..vf18 at 0x211db8

So VU1 maps skinned packed-model integers to clip space, with the camera origin pulled out (world units × 1024). Confidence: verified (math); the identity of V and cam is inferred from the call chain.

**Light constants.** `rec_i = bank[moby[0x38+i]]`. If b = moby[0x3a] ≠ 0: `E = rec0·(1 − b/256) + rec1·(b/256)` for all four qw, and both directions are renormalised with rsqrt. Otherwise E = rec0. Light k's model-space "to-light" vector is `L_k[i] = −(r_i · dir_k)`, which is `−Rᵀ·dir` (0x211d88).

**Point lights** (0x212150): for each active bank point light j (prepared at entry: pos·1024, R_j·1024, colour.w = 1/(1024·R_j)), with c = moby sphere centre and δ_j = |x_j − c| < R_j:

    a_j = 1 − δ_j/R_j,   ℓ_j = −Rᵀ·(c − x_j)/δ_j
    C_2 = Σ a_j·col_j,    L_2 = normalize(Σ a_j·ℓ_j),    K_2 = 0

With a single light in range, that light is used as is. The test uses the moby centre, not the vertices. Confidence: verified.

**Job record** (built in SPR 0x2c00/0x3000, copied to main memory with DMA ch.8 SPR_FROM):

| off | contents |
|---|---|
| 0x00 | u16 packet count (0 word ends the list) |
| 0x04–0x0c | moby[0x54], [0x60], [0x64] (animation inputs) |
| 0x10 | s16 joint count; 0x12 = 0x400 flag (anim, not reversed) |
| 0x14 | metal word: lo16 = shine alpha (≤ 0x80, distance faded), hi16 = metal record qwc (0 = no metal pass) |
| 0x18 / 0x1c | class `common_trans` / class `skeleton` pointers |
| 0x20–0x3c | two animation frame descriptors (from moby[0x68]/[0x6c]) |
| 0x40/0x50/0x60 | light rows: lane x = L_0, lane y = L_1, lane z = L_2, lane w = 0 |
| 0x70/0x80/0x90 | C_0 = col_a, C_1 = col_b, C_2 = point colour (w = 0) |
| 0xa0 | K = (col_a.w, col_b.w, 0, 0) |
| 0xb0 | ambient: `0x47800000 \| byte` per lane = 65536 + {R, G, B, α}/128; α = moby[0x23]·fade >> 7 |
| 0xc0.. | per packet: `u32 (vertex_table_addr << 4) \| vertex_data_size`, `u32 chain destination` |
| after | optional metal record (§5) |

**VIF1 chain per packet** (templates at gp−0x64b0.., gp = 0x166c00):
1. First two packets of the moby only: CNT 4 qw, `STCYCL 4,4`, `UNPACK V4_32 → 0x129` or `0x28d` (absolute; picked by a sign flip that tracks the VU1 double buffer), then M_vu1.
2. REF (qwc = entry.`unknown_d` = ⌈6n/16⌉) with `UNPACK V3_16 signed, TOPS+0x00`, num n → positions.
3. NEXT (qwc = entry.`unknown_e` = ⌈4n/16⌉) with `UNPACK V4_8 unsigned, TOPS+0x61` → colours, stored inline, followed by the position block. **This data area is where `fun_001ee650` writes.**
4. REF to the class VIF list (ST at 0xc2, indices at 0x12d, ad-gifs).
5. CNT with `MSCAL 0x0e` when the bounding sphere passes the guard-band test (13859 L61, no clipper) or `MSCAL 0x0a` otherwise (13859 L2, which has the clip test and clipper). Sign convention inferred.

Chain preamble: `MSCAL 0` (13859 init), REF 4 qw at 0x1de380 (`STCYCL 4,4`, `STMASK 0x20202020`, `STCOL 0x1000×4`, so ST's q lane = 1.0; `BASE 0`, `OFFSET 0x164`), then the two 6-qw constant uploads (§7). Per-moby GS state (ALPHA_1 0x8000000044, TEST_1 0x5360b, fog variant) comes from 0x1dedc0.

## 3. `fun_0020e0e0` (Lombyte: assembly/textbin/fun_0020e0e0) — joint palette

DMAs the class skeleton (4·joints qw from job+0x1c) and the animation frame data into SPR. It decodes keyframes (quaternions via `vitof15`, optional per-joint scale, translation, parent chain) into pose matrices P_j at SPR 0x70000000 + 0x40·j. It then forms the **skinning palette in place** (0x20ed18):

    F_j = P_j · S_j          // S_j = class skeleton matrix j, used as-is (it acts as the inverse bind)

With joint count 0 it just writes the identity at SPR 0. The ≤ 0x6f-joint limit is exactly SPR 0x0000–0x1bff (the job constants start at 0x1c00). Confidence: final multiply and identity case verified; keyframe decode **not reversed**.

## 4. `fun_001ee650` — EE driver for VU0 104691 (Lombyte: assembly/textbin/fun_001ee650)

SPR map: 0x0000 joint palette, 0x1c00 job constants (8 qw), 0x1c80 packet list, 0x2000/0x2800 vertex table (double-buffered, D9 SPR_TO), 0x3000/0x3400 output (double-buffered, D8 SPR_FROM to the chain address), 0x3800 trig table.

Setup per moby: `lqc2` vf25–27 = light rows, vf28–30 = colours, vf31 = ambient. VU0 data memory gets `[0xfb] = −|K|` (x,y,z; w = 1), `[0xfc..0xfe]` = light rows, and `[0xff]` = a −1 sentinel. Registers: vi10 = 0xfe.

Per packet (header = RAC1 vertex table header): the pre-loop matrix transfers are `vsqi`'d into VU0 memory (`VU0[dst..dst+3] = F[joint]`). Then `ctc2` vi3 = 2-way, vi4 = 3-way and vi5 = main counts, and `vcallms 0`. Per vertex the EE supplies:

| VU0 reg | contents |
|---|---|
| vf1 | halfwords 4–7 sign-extended = (az\|el, x, y, z) |
| vf2 | (cos a, sin a, cos e, sin e), from SPR table entries a = byte 8, e = byte 9 |
| vf3 / vf4 | bytes 0–3 / 4–7, one per lane |
| vf5–vf8 | F[low_halfword >> 9] (the scheduled transfer; stored to the 0xf4 sink when unused) |

**Handshake.** VU0 loads the sentinel into the output colour register, writes the real colour when done, and then waits for its `vi1`/`vi2` flag to be cleared. The EE polls `qmfc2` until the value is positive, reads vf11/vf12 (or the alternate set vf9/vf10), loads the next inputs and `ctc2 zero, vi1|vi2`. Outputs lag inputs by 2 vertices.

Packing: `ppach(vf12)` gives s16 x,y,z (6 bytes, **no saturation**). The colour is `ppach(vf11)` (low 16 bits of 65536 + c/128 = ⌊128c⌋), then `pmulth` by the per-vertex multiplier bytes, `pmfhl.sh` (**saturates to 32767**), `psrah 7` and `ppacb`. That gives `rgba = min(255, (⌊128c⌋·m) >> 7)`.

The multiplier m comes from the RAC1 vertex-table **`unknown_e` blob: 4 bytes (RGBA) per transfer vertex**, align16(4·transfer_vertex_count) bytes (checked on all 1680 level-01 packets). It is **0x80 in every class on the disc** (8.8 M bytes, levels 00–18).

Every result is also written to the **vertex cache** at `a1 + 16·id` ({s16 x,y,z,pad; u32 rgba}), using the 9-bit ID carried 7 vertices later and then the epilogue ID lists. The **duplicates** are copied from that cache (`id = dupe >> 7`) into the output after the loop, so duplicates are *not* re-lit. Output layout: colours n×4 (align 16), then positions n×6 (align 16); the chain destination is packet entry data + 0x20.

## 5. VU0 program 104691

Memory: matrix slots at qw 0..0xf3 (4 qw each; row 3 = translation); **0xf4 is a write-only sink** (0xf4–0xf7 are never loaded); 0xfb–0xff are the constants above.

**Entry 0x000** (`vcallms 0`). Loops L7/L9 (2-way, vi03), L11/L14 (3-way, vi04), L1/L4 (single, vi05), then L18/L21 (drain, vi11 = 2), ending at L23 `:e`. For each vertex:

    transfer:   VU0[vf3.w | vf4.z(2-way)] ← vf5..vf8                // store before load
    2-way:      M = (w1·S[b2] + w2·S[b3]) / 256;  S[b7] ← M          // itof12(w)·16.0
    3-way:      M = (w1·S[b2] + w2·S[b3] + w3·S[b1 & 0xFE]) / 256;  S[b7] ← M
    single:     M = S[b2]
    pos  = trunc(M0·x + M1·y + M2·z + M3)                            // ftoi0 → s16
    n    = (cos a·cos e, sin a·cos e, sin e),  a,e in 2π/256 steps
    n'   = M0·n.x + M1·n.y + M2·n.z                                   // not normalised
    d_k  = L_k · n'                   (k = 0..2; the w lane gives 0)
    f_k  = max(d_k, −|K_k|·d_k)
    c    = A + (Σ_k C_k · f_k) · rsqrt(|n'|²)                          // A, C in 1/128 units

The weights are **/256** (every 2- and 3-way vertex on the disc sums to exactly 256). Confidence: verified.

**Entries 0x111 / 0x121 / 0x12D** (3 / 2 / 1 matrix, `vcallmsr`, CMSAR0 = index). Inputs: vf07 = position, vf08 = trig, vf09 = weights ×16, and matrices vf10–13 / vf14–17 / vf18–21 taken **directly from the SPR palette** (no slot cache). Outputs:

    vf01 = ftoi0(M·p);  vf02 = A' + Σ C'_k·max(L_k·n', 0);  vf03 = ftoi12((E·n').xy + 1)

The EE shifts vf03 right by 1 to get ST = (E·n' + 1)/2 in 4.12 fixed point. There is no normalisation and no back-light term. Confidence: verified.

## 6. Metal / shine pass (moby[0x73] ≠ 0 and the class has metal packets)

- MobyProc (0x212998) builds E = 3 rows (camera rotation × moby rotation, re-based on the view direction to the moby) and adds **(0, 0, 1000, 0)** to M_vu1's translation row as a depth bias. It chains, per metal packet: REF V3_16 positions, REF V4_8 colours, NEXT V2_16 (mask) ST inline at 0xc2, the class VIF list and `MSCAL 0x0a`.
- `fun_001ee650` tail (0x1ef65c) scales vf28–30 by 0.25 and sets the ambient to 0x70 (w = 0x80). Vertex RGB = 0x70 + 32·Σ C_k·max(L_k·n', 0), no saturation; α = shine alpha.
- Metal vertex (16 bytes) — this resolves moby_rac1.md §2.11: `s16 x,y,z; u8 az, el; u8 joint[3]; u8 count; u8 weight[3]; u8 pad`. count ≤ 1 means a single joint from byte 8; the weights sum to 256. The metal header's `unknown_4/8/c` are the output offsets of positions and colours and the total output bytes (ST at 0). Confidence: verified.

## 7. VU1 constant block for 13859

Uploaded from 0x18cd00 (filled by `update_view_context` 0x1f2d98 from the view context 0x18cf00..). In the table, cf00/cf04 = half the screen size, cf08/cf0c = the ×4 guard band, cf10 = the perspective numerator, cf14 = the fog offset, cf2c = the fog floor, and `fz` = −8388080.0 (a z constant):

| VU1 qw (A / B) | reg | contents |
|---|---|---|
| 0x123 / 0x287 | vf24 | x = cf2c (`maxx.w` fog/w floor); yzw = A+D GIF tag template (NREG 1, REGS 0xE) |
| 0x124 / 0x288 | vf23 | x = cf10 (`div Q = vf23.x / w`); rest = strip GIF tag: PRE, PRIM 0x7c (tristrip, IIP, TME, FGE, ABE), NREG 3, REGS 0x412 (ST, RGBAQ, XYZF2); w = 0x303ec000 (same tag word with the PRIM low bit set = fan, presumably for clipped polygons) |
| 0x125 / 0x289 | vf28 | (1/cf08, 1/cf0c, 1/fz, 1/cf10), clip-test scale |
| 0x126 / 0x28a | vf21 | (cf08/cf00, cf0c/cf04, 1, 1), guard-band scale |
| 0x127 / 0x28b | — | (cf08, cf0c, fz, cf10); not loaded by 13859 via vi13 |
| 0x128 / 0x28c | vf22 | (2048, 2048, 8388096.0, cf14), post-divide offset |
| 0x129–0x12c / 0x28d–0x290 | vf17–vf20 | M_vu1 (per moby, §2) |
| — | vf27 | set by 13859 init (`mfir` 0x2c8 / 0x348 output buffers) |

Per-vertex input (relative to TOPS, index i 1-based): position at 0x00+(i−1), colour at 0x61+(i−1), ST at 0xc2+(i−1), indices at 0x12d. Confidence: verified.

## 8. Corrections to docs/formats/moby_rac1.md

- Normal decode: the game uses **x = cos a·cos e, y = sin a·cos e** (table 0x165500 = (cos, sin)). Wrench and §2.9 have x and y swapped.
- Blend weights are /256, not /255.
- The RAC1 `unknown_e` blob holds per-vertex RGBA multipliers (0x80 = 1.0).
- Header byte 9 is the low-LOD joint count.
- Metal vertex and header layout: see §6.

## Next steps for the Rust moby renderer

1. **Loader** (`rc-formats`) must expose: raw vertex bytes 0–7 plus normal az/el; the 9-bit IDs *after* the i−7 shift and the epilogue IDs; the transfer list; the duplicate list; the multiplier blob; the metal vertex fields; class bytes 4/5/8/9/0xe; `scale`; and the skeleton matrices.
2. **Resolve the VU0 slot machine at load time** (as Wrench does) into per-vertex `{joint[3], weight[3]/256}`, carrying slot state across packets. This is exact because every slot only ever holds F_j or a blend of F's (blends of blends are never formed; Wrench checks this). Store the result as GPU vertex attributes.
3. **Skin in the vertex shader** (≤ 3 bones; no compute pass needed). Upload the palette F_j = P_j·S_j per moby per frame (≤ 111 mat4). Until `fun_0020e0e0` is reversed, identity F reproduces the stored pose. For exactness, truncate the skinned position toward zero before M_vu1 (ftoi0).
4. **Per-moby uniforms:** M_vu1 (or model = [s·R | p] with camera-relative 1024 scaling), L_0..2 (model space), C_0..2, K_0..1, ambient RGB, alpha. Compute the bank cross-fade and the point-light merge on the CPU once per moby per frame.
5. **Vertex lighting** exactly as §5: unnormalised n' and the |n'| normalisation after the colour sum; `floor(128·c)`; multiplier; `min(255, ·)`; then GS MODULATE (0x80 = 1.0) as for tfrags. Duplicates copy their source vertex's colour.
6. **Metal pass:** a second draw with the sphere-map ST, colour rule, alpha and depth bias from §6.
7. Still to reverse: the keyframe decode in `fun_0020e0e0`; the deferred/shadow (0x400/0x800) path in MobyProc (the glow path: §10); the exact cull sign conventions; and whether anything writes the multiplier blob at runtime.

## 9. LOD, culls, fade and the metal pass, pinned (2026-09-27)

Boot addresses (MobyProc `fun_00211808`; L01 `MobyProc` 0x26a7a0 is byte-identical). Port: `crates/rc-engine/src/moby_lod.rs`
(the per-moby decisions), `moby_render.rs` (entities, metal meshes), `assets/shaders/moby_metal.wgsl`.

**Sphere.** moby+0x00 is built by `fun_0020def8` (0x20df48..0x20dfa0): the current sequence's header sphere (seq A =
moby+0x52; while A ≠ B the lerp `(B·t + A) − A·t` with t = moby+0x54, 0x20e038; A = 0xff reads a snapshot sphere from
0x1b2c00), times moby+0x2c on all four lanes, rotated by the rows and added to 1024·position. Integer units throughout.
MobyProc turns it into camera space v (vf25..27 = view rows 0x186f40, camera ×1024 = vf24). Confidence: verified.

**Culls, in order** (after the dead byte +0x20 and the occlusion bits +0x36/+0x37; mode & 0x81 skips):
- draw distance: dd = `pminw`(moby+0x32 (s16), *(0x15ff30)) with 0x15ff30 = **500** (set by `fun_001e9b10` and
  `fun_00230f60`, no other writer); culled when `(itof(dd << 10) − r) − (v.z − r) < 0` (0x211bb0/0x211be8/0x211c30), i.e.
  the **centre's** view depth beyond dd units — the radius cancels;
- near: culled when `n − (v.z + r) ≥ 0` (n = 0x18cda0 = 32; 0x211c38);
- side planes: `vf20.xy·v.z − (|v.xy| − r·vf22.xy) < 0` (0x18cdb0 tangents, 0x18cee0; 0x211c50/0x211c58), as for ties.
A culled moby clears +0x31 and gets no job (0x212508).

**Fade** (0x211cd4..0x211d60): `fade = min(ftoi0((dd·1024 − r) − (v.z − r)) >> 7, 0x80)` (last 16 units of dd),
ambient α lane = `(fade · moby+0x23) >> 7` (the vertex alpha, ×multiplier >> 7), and t6 bit 3 = fade < 0x80. With t6 ≠ 0
the first packet is preceded by one A+D (template 0x1dedc0 copied to SPR 0x3f00): **TEST_1 = 0x5360b + (t6 << 4) − 0x600
= 0x5308b (AREF 0x08)** for bit 3 alone, or **ALPHA_1 = 0x48 (Cs·As + Cd, additive) for mode bit 0x200** (the `ori a0,
zero, 0x48` in the branch delay slot keeps the register address 0x42 in the upper 64 bits); the original word is re-sent
after the last packet (0x212450). Neither 0x200 nor 0x8 occurs on Novalis.

**LOD** (0x211e18..0x211ea0): `d = clamp(ftoi0(v.z − r), 0, 0x30000)`; low iff `d − (moby+0x72 << 10) > 0`; moby+0x72 =
class byte 0xe (`InitMobyInstance` 0x20c5f0; 0xff without a class). No hysteresis, no cross-fade. Low: packets
[class[4], class[4] + class[5]), **job joint count = class[9]** (`MobyAnimEval` then fills palette slots 0..class[9]−1,
the identity at slot 0 for 0). Measured on the disc (669 classes with a low LOD, all levels): every low-LOD vertex skins
to a slot < max(class[9], 1), so the low LOD uses the first class[9] slots of the same palette; class[9] = 0 means the
low LOD is drawn in the bind pose (identity). lod_trans ≥ 192 can never switch (clamp); every Novalis class without a
low LOD has 0xff. A class with class[5] = 0 would draw nothing beyond lod_trans (none on the disc with lod_trans < 192).
The per-class counters at SPR 0x2600 + 2·moby+0x22 (high, low) and the nearest-depth table at SPR 0x3a00 are stats.

**Shine gate** (0x212968): moby+0x73 = 0x18 when class byte 6 (metal count) ≠ 0 (`InitMobyInstance`). With
`m = (moby+0x73 << 10) − ftoi0(v.z − r)` (not clamped): m ≤ 0 → no metal pass, else shine alpha = min(m >> 7, 0x80) —
full within 8 units, 0 at 24. Job +0x14 = shine alpha | metal record qwc << 16 (`(metal_count + 10) >> 1`, 0x2129d4).

**Metal record and chain** (0x212998..0x212c0c, after the moby's regular packets, same chain):
- E: d = view-rotated unit vector camera → sphere centre; m_i = view-rotated moby row i;
  `A(m) = (m.x, d.z·m.y − d.y·m.z, d.y·m.y + d.z·m.z)`, `B(u) = (d.z·u.x − d.x·u.z, u.y, d.x·u.x + d.z·u.z)`,
  E_i = B(A(m_i)). Both steps use the same unrenormalised d, so this is only approximately a rotation that takes the view
  direction to +z (exact for a moby straight ahead). 4 qw (E rows + (0,0,0,1)) head the metal record → SPR 0x1c00 → VU0
  vf22..24 (0x1ef730).
- M_vu1 translation row += (0, 0, 1000, 0) (template gp−0x63f0 = SPR 0x3ee0; 0x212b08), re-uploaded with the first two
  metal packets: GS Z += 1000·Q = 1000·n/z (Q = cf10/w = n/z), a pull toward the camera.
- Per metal packet: REF positions (V3_16, qwc = entry +0xd), REF colours (V4_8, +0xe), ST (V2_16, +0xe) — the EE's output
  block is ST at 0, positions at +16·e, colours after (the metal header's unknown_4/8/c) — then the class VIF list (indices,
  ad-gifs) and `MSCAL 0x0a` (13859 L2, clipping version) always. **No GS register change**: ALPHA_1 0x8000000044 and
  TEST_1 0x5360b of the moby (a fade's 0x5308b was restored at 0x212450 before the metal packets). PRIM 0x7c
  (tristrip, IIP, TME, FGE, ABE) from the VU1 constants.

**Metal vertex** (`fun_001ee650` tail 0x1ef65c, VU0 0x111/0x121/0x12d): count byte 0xb: 3 → 0x111, 2 → 0x121, else
0x12d (joint = byte 8); matrices straight from the SPR palette (0x70000000 + 64·joint); weights `itof12(w << 4)` = w/256.
Position ftoi0 + `ppach`. n = (cos a·cos e, sin a·cos e, sin e) from the SPR trig table, n' = M0·n.x + M1·n.y + M2·n.z.
Colour: vf28..30 = C_k·0.25 (0x3e800000), vf31 = (65536 + 0x70/128)×3, w 65536 + 1; `vf02 = vf31 + Σ C'_k·max(L_k·n', 0)`
one truncating add per light → RGB = low byte of `0x70 + Σ_k ⌊32·C_k·max(L_k·n', 0)⌋` (`ppach` + `ppacb`, no
saturation, no rsqrt, no back-light term, no multiplier blob); α byte overwritten with the shine alpha (`sb sp, 3(a1)`).
ST: `vf03 = ftoi12((E·n').xy + 1)`, `psraw 1`, `ppach` → S, T in 4.12 = ⌊trunc(4096·(e + 1)) / 2⌋, so
`st = (e.xy + 1)/2` with e = E_0·n'.x + E_1·n'.y + E_2·n'.z (camera-space x right, y down: t grows downward).

**Texture** (level load, L01 0x258128 → `fun_00203338` → `fun_00202d78`): TEX0 data_lo −2 / −3 are replaced by the words
at 0x19e6c0 / 0x19e6d8 (L01 0x182c40 / 0x182c58): chrome `TBP0 = (gs_base + core[0x90]) >> 8 | 0x1d308000 | CBP =
(gs_base + core[0x94]) >> 8 << 37 | 0x5c0000000 | CLD 4` = **128×128 PSMT8, TBW 2, TCC 1, MODULATE**; glass the same
with 0x19304000 / 0x580000000 = **64×64** from core[0x98] / core[0x9c]; MIPTBP1 0x40000400004000; TEX1 = `data_hi << 6 |
0x20 | data_lo << 32` (MXL 0, MMAG linear; every metal ad-gif on Novalis has data_hi 4 → linear); CLAMP_1 = 5 (clamp).
core[0x90..0x9c] are byte offsets into the gs_ram lump (Novalis: chrome 0x400 / pal 0x0, glass 0x4800 / pal 0x4400); both
maps decode to blurry environment photographs with alpha 13..128. **Chrome and glass differ only in the map** (same VU0
path, same GS state). (0x182c70, from core[0xc0]/[0xc4], is the **gold** 128×128 map: `select_world_object_resource_tables` writes it
into the first metal ad-gif of a gold weapon's class, `ExtraMobys::set_gold`.)

**Novalis classes with metal packets:** placed: 13 (281 instances, the bolt pickups (threaded shaft + head, seen in a close shot); 1 high packet + 2 metal packets, all glass; jc 1),
1456–1465 (one instance each, 3–7 chrome packets, jc 1, far east of the start), 1818 (1 instance, 4 chrome packets, jc
49). Also on the level: Clank 601 (4 chrome packets, drawn on Ratchet's back by moby_attach), 10, 14–16, 173, 195, 203,
479, 608, 609, 1143, 1289, 1290, 1900; the wrench (gadget 71) has chrome packets too. No metal class has a low LOD.

Confidence: verified in the disassembly except where marked; the port's camera is Bevy's, so thresholds agree with the
game to float noise, not bit for bit.

## 10. The glow list (mode 0x10), pinned (2026-09-28)

Level01 addresses (MobyProc 0x26a7a0; the boot copy is byte-identical). Port: `crates/rc-engine/src/moby_lod.rs`
(`glow_word`), `moby_render.rs` (`SKIN_GLOW`, `glow_from`, `MobyLook`), `assets/shaders/moby.wgsl`.

* **Set-up.** `InitMobyInstance` (0x263488): class header +0x40 ≠ 0 → mode |= 0x10 and moby+0x90 = that word. Updates
  rewrite +0x90: the vendor 11 `(sin t·48 + 96)·0x010101 | 0x80000000`, the floor switch 830 (grey pulse, green
  0x80208020 when pressed), Clank's moby (0x2278c0), the ship 531 (0x2a1c40), amoeboids, the bomb.
* **MobyProc** (0x26b2a0 `bne t5, zero` with t5 = moby+0x34 & 0x10, 0x26aef4): after the moby's job, at 0x26b890..0x26b8f8,
  n = class byte `0xa + lod` (0x26b8b0; lod = 0 high, 1 low, the upper half of t6 set at 0x26adfc / 0x26ae34); when the
  LOD list has more than n packets, a 16-byte record is appended at SPR 0x3400..0x3800 (64 records): word 0 = +0x90 with
  byte 3 = the job's ambient α byte (`lbu a3, −4(t8)` = job +0xbc: the vertex alpha `fade·+0x23 >> 7`), word 1 = the
  packet entries from n on, word 2 = the job's chain slots of those packets (job +0xc0 + 8n), word 3 = their count.
  At the end (0x26b4c8) the records are copied to 0x1ac680 and their byte length to 0x15fff8 (gp −0x6c08).
* **Draw** (`DrawMobysCleanUp` 0x264d68 → `fun_002116b8` 0x26a650, after `ProcessMobyAnimData` has skinned and lit):
  for every record and packet, the first `transfer_vertex_count` (packet byte 0xf) colour words of the packet's
  output are overwritten with word 0. So a glow packet is drawn **unlit, in the moby's glow RGB, at the moby's vertex
  alpha** (no multiplier, no saturation), with the moby's own GS state (ALPHA 0x44, TEST 0x5360b, the texture of its
  ad-gifs). Confidence: verified in the disassembly.
* **The disc** (all 19 levels): 547 (level, class) pairs with a glow word, 175 distinct classes, 2568 placed instances;
  532 pairs / 163 classes / 2387 instances have glow packets in their high LOD (the vendor's balls, lamps, eyes,
  Ratchet's class 0 packet 62, Clank's 601 packet 21, the logo 1143's ring text). No class sets mode 0x10 through
  +0x44 without a glow word.

**In the port.** Mesh build: the vertices of packets ≥ class byte 0xa (0xb for the low LOD) carry `SKIN_GLOW` (bit 24
of the skin word) and are their own parts. Per frame every moby's `MobyLod.misc.w` holds `GLOW_ON | RGB` when its mode
has 0x10 (statics: `MobyOcclusion::look` from the moby table, class default for undriven ones; dynamic slots:
`SlotLook.glow`; extras: the class default from `ExtraMobys::spawn`). moby.wgsl replaces a flagged vertex's colour
with (RGB, vertex alpha) / 128 after lighting. A glow part's TEST_1 fail half (its soft edge, As < 0x60) is drawn as
`GsPass::EffectLowAlpha`: the same blend on display bytes (crate::display_blend), like every other glow. No class is
special-cased. `RC_MOBY_GLOW=0` turns the list off.

**One overwrite per batch.** Each `MobyProc` call restarts the list (SPR 0x3400, 0x26a8ac) and at its end rewrites
0x1ac680 and the length 0x15fff8 (the store is in a delay slot: 0 when it has no records); `DrawMobysCleanUp` runs the
overwrite once. So in a `DrawMobyList` batch (mode 5's vendor screens `DrawWorld_Mode5` 0x2b4020, `PageMenuDraw`
0x28d080) only the **last** list's glow packets are recoloured. In the vendor's mode 5 the last list is the salesman
(class 12) or the popup (0x471), neither with glow packets: the item model (e.g. the Pyrocitor's lens), the hologram
item and the vendor itself are drawn lit. Port: `ExtraMobys::clear_glow` for the vendor's screen slots; the vendor's
own static instance still glows in mode 5 (not modelled). The page menus' widget mobys (Weapons / Gadgets: the 3D
Ratchet, his items, the preview) are off the list for another reason: the widgets set their +0x34 to 0 (0x297ad0,
0x291c38) or 4 (`LoadHandGadget` 0x297d70) after creating them; `crate::menu_models` clears their glow too.

**Not the halos.** The soft blue halos around the vendor's antenna balls are not this list: they are the glow quads of
the vendor's draw callback (docs/plan/interaction.md §9.1, `crate::fx_draw` "glow quad").
