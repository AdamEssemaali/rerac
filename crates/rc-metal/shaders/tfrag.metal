// Tfrag base pass in MSL: a port of rc-engine's assets/shaders/tfrag.wgsl (its header has the GS derivations: TFX =
// MODULATE, the ALPHA_1 / TEST_1 state, fog, the VU1 LOD morph / collapse passes and the GS mip rule). Differences:
// * no point lights yet (the WGSL's `tfrag_lit`; the Metal renderer has no game tick to light from), so `modes` holds
//   only the draw modes;
// * Bevy's view bindings are the `View` block (buffer 0), the material bindings the buffers / texture / sampler below;
// * the alpha-test half of the split is a per-draw value (`DrawState::atest`) instead of the GS_ATEST_* shader defs.

#include <metal_stdlib>
using namespace metal;

struct View {
    float4x4 clip_from_world;
    float4x4 view_from_world;
};

struct TfragFog {
    // rgb = FOGCOL (display-encoded 0..1), w = 1 when fog is enabled.
    float4 color;
    // (slope per integer unit of depth, offset qw661.w, lower clamp qw656.y, upper clamp qw656.z).
    float4 params;
    // x: the texture option (0 = the GS level rule, 1 = smooth, 2 = the GPU's own level).
    float4 tex_mode;
};

struct TfragLod {
    float4 k666;
    float4 k667;
    float4 k668;
    float4 k669;
    // x = VU w per raw unit of depth, y = 1: tint by LOD, z = near (32).
    float4 misc;
    // y = MXL.
    float4 tex;
};

struct Slot {
    packed_float3 pos;
    uint color;
    uint p1;
    uint p2;
    // 0 none, 1 = written by a LOD-01 primary, 2 = by a LOD-0 primary.
    uint tier;
    uint normal;
};

struct VInfo {
    float2 uv;
    uint slot;
    // 0 common, 1 LOD-01, 2 LOD-0 (collapse candidates).
    uint tier;
    uint p1;
    uint p2;
};

struct DrawState {
    // 0: no alpha test, 1: keep As >= AREF (GS_ATEST_PASS), 2: keep As < AREF (GS_ATEST_FAIL).
    uint atest;
    uint aref;
};

struct VertexOut {
    float4 position [[position]];
    float2 uv;
    // Gouraud RGBA / 128, screen-space linear like the GS.
    float4 color [[center_no_perspective]];
    // GS F / 255, screen-space linear like the GS.
    float fog [[center_no_perspective]];
    // Camera depth in raw units, perspective-correct (= n / Q per pixel).
    float depth;
    // Debug tint: 0 none, 1 LOD 1, 2 LOD 2, 3 clipping path.
    uint tint [[flat]];
    // TEX1 K of the triangle's ad-gif, in mip levels.
    float k [[flat]];
};

struct Vtx {
    float3 pos;
    float4 col;
};

struct Ctx {
    constant View* view;
    constant TfragLod* lod;
    device const Slot* slots;
};

static float depth_raw(constant View& view, float3 p) {
    // Camera-space depth in the game's integer units (view space looks down -z, game units).
    return -(view.view_from_world * float4(p, 1.0)).z * 1024.0;
}

static float w_of(Ctx c, float3 p) { return depth_raw(*c.view, p) * c.lod->misc.x; }

static Vtx own(Ctx c, uint s) {
    Slot sl = c.slots[s];
    return Vtx{ float3(sl.pos), float4(as_type<uchar4>(sl.color)) };
}

static Vtx blend(Vtx c, Vtx p1, Vtx p2, float2 t) {
    return Vtx{ (p1.pos + p2.pos) * t.x + c.pos * t.y, floor((p1.col + p2.col) * t.x + c.col * t.y) };
}

// A common or LOD-01 slot after L18 / L26 in mode m.
static Vtx value01(Ctx c, uint s, uint m) {
    Slot sl = c.slots[s];
    Vtx v = own(c, s);
    if (sl.tier != 1u || !(m == 8u || m == 10u || m == 14u)) { return v; }
    Vtx p1 = own(c, sl.p1);
    Vtx p2 = own(c, sl.p2);
    if (m == 8u && w_of(c, p1.pos) < c.lod->k666.w && w_of(c, p2.pos) < c.lod->k666.w) { return v; }
    float2 t = clamp(w_of(c, v.pos) * c.lod->k666.xy + c.lod->k668.xy, float2(0.0), float2(0.5, 1.0));
    return blend(v, p1, p2, t);
}

// Any slot after all morph passes of mode m.
static Vtx value(Ctx c, uint s, uint m) {
    Slot sl = c.slots[s];
    if (sl.tier != 2u) { return value01(c, s, m); }
    Vtx v = own(c, s);
    if (!(m == 14u || m == 16u)) { return v; }
    Vtx p1 = value01(c, sl.p1, m);
    Vtx p2 = value01(c, sl.p2, m);
    if (m == 14u && w_of(c, p1.pos) < c.lod->k667.w && w_of(c, p2.pos) < c.lod->k667.w) { return v; }
    float2 t = clamp(w_of(c, v.pos) * c.lod->k667.xy + c.lod->k669.xy, float2(0.0), float2(0.5, 1.0));
    return blend(v, p1, p2, t);
}

vertex VertexOut tfrag_vertex(
    uint vid [[vertex_id]],
    constant View& view [[buffer(0)]],
    constant TfragFog& fog [[buffer(1)]],
    constant TfragLod& lod [[buffer(2)]],
    device const Slot* slots [[buffer(3)]],
    device const VInfo* vinfos [[buffer(4)]],
    device const uint* modes [[buffer(5)]],
    // (global vertex-info index, tfrag index | strip list << 16 | TEX1 K (s12, 1/16 units) << 18)
    device const uint2* refs [[buffer(6)]]
) {
    VertexOut out;
    Ctx c = Ctx{ &view, &lod, slots };
    uint2 tref = refs[vid];
    uint tf = tref.y & 0xffffu;
    uint list = (tref.y >> 16u) & 3u;
    uint m = modes[tf];
    // Strip list the mode draws: LOD 2 for MSCAL 6, LOD 1 for 8 / 0xa, LOD 0 otherwise (incl. the clip path).
    uint mlist = (m == 6u) ? 2u : ((m == 8u || m == 10u) ? 1u : 0u);
    if (m == 0u || mlist != list) {
        // Not drawn this frame: all three vertices of the triangle land on one point outside the clip volume.
        out.position = float4(2.0, 2.0, 2.0, 1.0);
        out.uv = float2(0.0);
        out.color = float4(0.0);
        out.fog = 0.0;
        out.depth = 1.0;
        out.tint = 0u;
        out.k = 0.0;
        return out;
    }
    VInfo e = vinfos[tref.x];
    if ((m == 8u && e.tier == 1u) || (m == 14u && e.tier == 2u)) {
        float thr = (m == 8u) ? lod.k666.w : lod.k667.w;
        VInfo pe = vinfos[e.p1];
        if (w_of(c, value(c, pe.slot, m).pos) < thr && w_of(c, value(c, e.p2, m).pos) < thr) { e = pe; }
    }
    Vtx r = value(c, e.slot, m);
    out.position = view.clip_from_world * float4(r.pos, 1.0);
    out.uv = e.uv;
    out.color = r.col / 128.0;
    float depth = depth_raw(view, r.pos);
    out.depth = depth;
    // VU: w = depth * slope (the w row of the matrix), += qw661.w, min qw656.z, max qw656.y, ftoi4.
    float f = max(min(depth * fog.params.x + fog.params.y, fog.params.w), fog.params.z);
    out.fog = trunc(f) / 255.0;
    out.k = float(as_type<int>(tref.y << 2u) >> 20) / 16.0;
    out.tint = (lod.misc.y > 0.5) ? ((m == 2u) ? 3u : list) : 0u;
    return out;
}

static float3 srgb_to_linear(float3 c) {
    float3 lo = c / 12.92;
    float3 hi = pow((c + 0.055) / 1.055, float3(2.4));
    return select(hi, lo, c <= float3(0.04045));
}

fragment float4 tfrag_fragment(
    VertexOut in [[stage_in]],
    constant TfragFog& fog [[buffer(1)]],
    constant TfragLod& lod [[buffer(2)]],
    constant DrawState& draw [[buffer(7)]],
    texture2d<float> tex [[texture(0)]],
    sampler tex_sampler [[sampler(0)]]
) {
    // GS mip level: round(log2(z / n) + K), clamped to 0..MXL (or the texture option's level).
    float l = log2(in.depth / lod.misc.z) + in.k;
    float mxl = lod.tex.y;
    float4 t;
    if (fog.tex_mode.x > 1.5) {
        t = tex.sample(tex_sampler, in.uv, gradient2d(dfdx(in.uv), dfdy(in.uv)));
    } else {
        float lvl = (fog.tex_mode.x > 0.5) ? clamp(l, 0.0, mxl) : clamp(floor(l + 0.5), 0.0, mxl);
        t = tex.sample(tex_sampler, in.uv, level(lvl));
    }
    float3 rgb = min(t.rgb * in.color.rgb, float3(1.0));
    if (fog.color.w > 0.5) {
        rgb = mix(fog.color.rgb, rgb, in.fog);
    }
    if (in.tint == 1u) { rgb = mix(rgb, float3(1.0, 0.0, 0.0), 0.5); }
    if (in.tint == 2u) { rgb = mix(rgb, float3(0.0, 0.2, 1.0), 0.5); }
    if (in.tint == 3u) { rgb = mix(rgb, float3(0.0, 1.0, 0.0), 0.5); }
    // As = (At * Af) >> 7 in GS units (At raw 0..0x80, Af 0x80 = 1.0), clamped to 0xff; FGE leaves it alone.
    float a_s = min(floor(round(t.a * 255.0) * round(in.color.a * 128.0) / 128.0), 255.0);
    // GS TEST_1 alpha test (ATST GEQUAL AREF, AFAIL RGB_ONLY): which half of the split this draw is.
    if (draw.atest == 1u && a_s < float(draw.aref)) { discard_fragment(); }
    if (draw.atest == 2u && a_s >= float(draw.aref)) { discard_fragment(); }
    return float4(srgb_to_linear(rgb), a_s / 128.0);
}

// The GS clear sprite (rc-engine `game_camera::level_background`): one triangle over the viewport (the frame's 4:3 box)
// in the level background colour, linear for the sRGB target; Z untouched (the render pass clears it to 0).
vertex float4 clear_vertex(uint vid [[vertex_id]]) {
    float2 p = float2(float((vid << 1u) & 2u), float(vid & 2u));
    return float4(p * 2.0 - 1.0, 0.0, 1.0);
}

fragment float4 clear_fragment(constant float4& color [[buffer(0)]]) {
    return float4(srgb_to_linear(color.rgb), 1.0);
}
