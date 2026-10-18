//! Tfrag (terrain) geometry for `tfrag.metal`, the same layout as rc-engine `tfrag_render.rs` (its module doc has the
//! LOD, morph and GS-state derivations): all three strip lists of every tfrag, batched per GS texture state, a vertex
//! being only its (global vertex-info index, tfrag | list << 16 | TEX1 K << 18) reference into two static storage
//! buffers (`slots`, `vinfos`). Here every batch shares one reference buffer and one index buffer (absolute indices).

use crate::camera::game_to_world;
use crate::level::Level;
use rc_formats::texture::Texture;
use rc_formats::tfrag::{tfrag_triangles, TfragAdGifs, TfragMorphTier};
use std::collections::{BTreeMap, HashMap};

/// GS AREF of the tfrag pass (TEST_1 = 0x5360b; rc-engine `gs_state::AREF_WORLD`).
pub const AREF_WORLD: u8 = 0x60;

/// One draw's GS state (the tfrag subset of rc-engine `gs_state::GsPass`). The derived order is the draw order:
/// Bevy's Opaque3d, then AlphaMask3d, then Transparent3d.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GsPass {
    /// As = 0x80 everywhere: Cs and Z.
    Opaque,
    /// As ≥ AREF: blended, Z written.
    OpaqueTested,
    /// As < AREF (AFAIL = RGB_ONLY): blended, no Z.
    ColorOnlyLowAlpha,
}

/// Range of GS alpha values (0x80 = 1.0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AlphaRange {
    pub min: u8,
    pub max: u8,
}

impl AlphaRange {
    pub const OPAQUE: AlphaRange = AlphaRange { min: 0x80, max: 0x80 };

    pub fn of(values: impl IntoIterator<Item = u8>) -> Self {
        values
            .into_iter()
            .fold(None, |r: Option<AlphaRange>, a| {
                Some(r.map_or(AlphaRange { min: a, max: a }, |r| AlphaRange { min: r.min.min(a), max: r.max.max(a) }))
            })
            .unwrap_or(Self::OPAQUE)
    }

    pub fn union(self, o: AlphaRange) -> Self { AlphaRange { min: self.min.min(o.min), max: self.max.max(o.max) } }
}

/// The draws that reproduce a batch under the Z-writing alpha test (rc-engine `gs_state::draws`).
pub fn draws(aref: u8, texel: AlphaRange, vertex: AlphaRange) -> Vec<GsPass> {
    if texel == AlphaRange::OPAQUE && vertex == AlphaRange::OPAQUE { return vec![GsPass::Opaque]; }
    let lo = ((texel.min as u32 * vertex.min as u32) >> 7).min(0xff);
    let hi = ((texel.max as u32 * vertex.max as u32) >> 7).min(0xff);
    let mut out = Vec::with_capacity(2);
    if hi >= aref as u32 { out.push(GsPass::OpaqueTested); }
    if lo < aref as u32 { out.push(GsPass::ColorOnlyLowAlpha); }
    out
}

/// The texture wrap state from the ad-gif's CLAMP_1 (WMS in `data_lo`, WMT in `data_hi`; 1 = CLAMP).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SamplerKey {
    pub clamp_s: bool,
    pub clamp_t: bool,
}

impl SamplerKey {
    fn of(ad: &TfragAdGifs) -> Self { SamplerKey { clamp_s: ad.clamp.data_lo & 1 != 0, clamp_t: ad.clamp.data_hi & 1 != 0 } }
}

/// A texture as the shader reads it: Rgba8Unorm (display-encoded bytes), alpha back to the raw GS At (0..0x80).
pub struct TfragImage {
    /// Per mip level, level 0 first: (width, height, RGBA texels).
    pub levels: Vec<(u32, u32, Vec<u8>)>,
    pub texel_alpha: AlphaRange,
}

impl TfragImage {
    fn from_mips(mips: &[Texture]) -> Self {
        let levels: Vec<(u32, u32, Vec<u8>)> = mips
            .iter()
            .map(|l| {
                let mut rgba = l.rgba.clone();
                // `texture::scale_alpha` maps a < 0x80 to 2a and a ≥ 0x80 to 0xff: exact back for a ≤ 0x80.
                for a in rgba.iter_mut().skip(3).step_by(4) { *a = if *a == 0xff { 0x80 } else { *a / 2 }; }
                (l.width, l.height, rgba)
            })
            .collect();
        let texel_alpha = AlphaRange::of(levels.iter().flat_map(|(_, _, l)| l.iter().skip(3).step_by(4).copied()));
        TfragImage { levels, texel_alpha }
    }

    /// The magenta 1×1 stand-in for a texture index without pixels (alpha is the raw GS At, 0x80 = opaque).
    fn fallback() -> Self { TfragImage { levels: vec![(1, 1, vec![255, 0, 255, 0x80])], texel_alpha: AlphaRange::OPAQUE } }
}

/// One draw: a range of the shared index buffer with its texture, sampler state and GS pass.
pub struct Draw {
    pub first_index: u32,
    pub index_count: u32,
    /// Index into [`TfragScene::images`].
    pub image: usize,
    pub sampler: SamplerKey,
    /// MXL = levels − 1.
    pub mxl: u32,
    pub pass: GsPass,
}

/// Everything the GPU side uploads once per level.
pub struct TfragScene {
    /// `Slot { pos: packed_float3, color, p1, p2, tier, normal }`, 32 bytes per (tfrag, position).
    pub slots: Vec<u8>,
    /// `VInfo { uv: float2, slot, tier, p1, p2 }`, 24 bytes per (tfrag, vertex-info entry).
    pub vinfos: Vec<u8>,
    /// Per vertex: `uint2` reference.
    pub refs: Vec<[u32; 2]>,
    pub indices: Vec<u32>,
    pub images: Vec<TfragImage>,
    /// In draw order (by pass, then batch).
    pub draws: Vec<Draw>,
    /// LOD-0 triangles.
    pub triangles: usize,
}

#[derive(Default)]
struct Bytes(Vec<u8>);

impl Bytes {
    fn f(&mut self, v: f32) -> &mut Self { self.0.extend_from_slice(&v.to_le_bytes()); self }
    fn u(&mut self, v: u32) -> &mut Self { self.0.extend_from_slice(&v.to_le_bytes()); self }
}

const TIER_NONE: u32 = 0;
const TIER_LOD01: u32 = 1;
const TIER_LOD0: u32 = 2;

fn tier_code(t: TfragMorphTier) -> u32 { match t { TfragMorphTier::Lod01 => TIER_LOD01, TfragMorphTier::Lod0 => TIER_LOD0 } }

/// The static `slots` and `vinfos` buffers over all tfrags and each tfrag's first vertex-info index
/// (rc-engine `tfrag_render::build_storage`).
fn build_storage(level: &Level) -> (Vec<u8>, Vec<u8>, Vec<u32>) {
    let (mut slots, mut vinfos) = (Bytes::default(), Bytes::default());
    let mut vinfo_base = Vec::with_capacity(level.tfrags.len());
    let (mut ns, mut nv) = (0u32, 0u32);
    for t in &level.tfrags {
        vinfo_base.push(nv);
        let lit = (t.header.vert_count as usize).min(t.lights.len());
        let mut owner = vec![(TIER_NONE, 0u32, 0u32); t.positions.len()];
        for v in 0..t.vertex_info.len() {
            if let Some(l) = t.lod_link(v).filter(|l| l.morphs) {
                owner[l.own_position] = (tier_code(l.tier), ns + l.parent1_position as u32, ns + l.parent2_position as u32);
            }
        }
        for (p, pos) in t.positions.iter().enumerate() {
            let w = [
                t.origin[0].wrapping_add(pos.x as i32) as f32 / 1024.0,
                t.origin[1].wrapping_add(pos.y as i32) as f32 / 1024.0,
                t.origin[2].wrapping_add(pos.z as i32) as f32 / 1024.0,
            ];
            let b = game_to_world(w);
            // GS MODULATE: Cv = Ct * Cf >> 7, so 0x80 = 1.0.
            let c = t.rgba.get(p).map_or([0x80; 4], |c| [c.r, c.g, c.b, c.a]);
            let (tier, p1, p2) = owner[p];
            let normal = if p < lit {
                let l = rc_formats::tfrag_light::VertexLight::from_record(&t.lights[p]);
                l.azimuth as u32 | (l.elevation as u32) << 8 | 1 << 16
            } else { 0 };
            slots.f(b.x).f(b.y).f(b.z).u(u32::from_le_bytes(c)).u(p1).u(p2).u(tier).u(normal);
        }
        for (v, e) in t.vertex_info.iter().enumerate() {
            vinfos.f(e.s as f32 / 4096.0).f(e.t as f32 / 4096.0).u(ns + t.position_index(v) as u32);
            match t.lod_link(v) {
                Some(l) => vinfos.u(tier_code(l.tier)).u(nv + l.parent1_vinfo as u32).u(ns + l.parent2_position as u32),
                None => vinfos.u(TIER_NONE).u(0).u(0),
            };
        }
        ns += t.positions.len() as u32;
        nv += t.vertex_info.len() as u32;
    }
    (slots.0, vinfos.0, vinfo_base)
}

#[derive(Default)]
struct Batch {
    refs: Vec<[u32; 2]>,
    indices: Vec<u32>,
    vertex_alpha: Option<AlphaRange>,
    /// (tfrag, list, vertex-info index, K) → vertex, so shared strip vertices stay shared.
    remap: HashMap<(u32, u8, u16, u32), u32>,
}

/// The batched geometry, the storage buffers and the textures of a level (rc-engine `tfrag_render::spawn_tfrags_on`).
pub fn build(level: &Level) -> TfragScene {
    let (slots, vinfos, vinfo_base) = build_storage(level);
    // Batch key: texture index and wrap state (TEX1 K travels per vertex).
    let mut batches: BTreeMap<(i32, SamplerKey), Batch> = BTreeMap::new();
    let mut triangles = 0;
    for (ti, t) in level.tfrags.iter().enumerate() {
        let alpha = AlphaRange::of(t.rgba.iter().take(t.positions.len()).map(|c| c.a));
        for list in 0..3 {
            let tris = tfrag_triangles(t, list).expect("tfrag strips were validated by the golden test");
            if list == 0 { triangles += tris.len(); }
            for tri in &tris {
                let ad = &t.ad_gifs[tri.ad_gif as usize];
                let key = (t.texture_index(tri.ad_gif as usize), SamplerKey::of(ad));
                let k12 = ad.lod_k_raw() as u32 & 0xfff;
                let b = batches.entry(key).or_default();
                b.vertex_alpha = Some(b.vertex_alpha.map_or(alpha, |r| r.union(alpha)));
                for vi in [tri.a, tri.b, tri.c] {
                    let k = (ti as u32, list as u8, vi, k12);
                    let v = match b.remap.get(&k) {
                        Some(&v) => v,
                        None => {
                            b.refs.push([vinfo_base[ti] + vi as u32, ti as u32 | (list as u32) << 16 | k12 << 18]);
                            let v = (b.refs.len() - 1) as u32;
                            b.remap.insert(k, v);
                            v
                        }
                    };
                    b.indices.push(v);
                }
            }
        }
    }

    let mut images: Vec<TfragImage> = Vec::new();
    let mut image_of: HashMap<i32, usize> = HashMap::new();
    let (mut refs, mut indices, mut draws_out) = (Vec::new(), Vec::new(), Vec::new());
    for ((tex_index, sampler), b) in batches {
        let image = *image_of.entry(tex_index).or_insert_with(|| {
            let mips = usize::try_from(tex_index).ok().and_then(|i| level.mips.get(i)).and_then(|m| m.as_ref());
            images.push(match mips {
                Some(levels) => TfragImage::from_mips(levels),
                None => {
                    eprintln!("tfrag texture index {tex_index} has no decoded texture; drawing magenta");
                    TfragImage::fallback()
                }
            });
            images.len() - 1
        });
        let base = refs.len() as u32;
        let first_index = indices.len() as u32;
        refs.extend_from_slice(&b.refs);
        indices.extend(b.indices.iter().map(|i| i + base));
        let img = &images[image];
        let passes = draws(AREF_WORLD, img.texel_alpha, b.vertex_alpha.unwrap_or(AlphaRange::OPAQUE));
        for pass in passes {
            draws_out.push(Draw {
                first_index,
                index_count: b.indices.len() as u32,
                image,
                sampler,
                mxl: img.levels.len() as u32 - 1,
                pass,
            });
        }
    }
    // Stable: batches keep their order within a pass.
    draws_out.sort_by_key(|d| d.pass);
    TfragScene { slots, vinfos, refs, indices, images, draws: draws_out, triangles }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// rc-engine `gs_state::tests::draw_split`, tfrag cases.
    #[test]
    fn draw_split() {
        let o = AlphaRange::OPAQUE;
        assert_eq!(draws(0x60, o, o), vec![GsPass::Opaque]);
        let cut = AlphaRange { min: 0, max: 0x80 };
        assert_eq!(draws(0x60, cut, o), vec![GsPass::OpaqueTested, GsPass::ColorOnlyLowAlpha]);
        assert_eq!(draws(0x60, AlphaRange { min: 0x60, max: 0x80 }, o), vec![GsPass::OpaqueTested]);
        assert_eq!(draws(0x60, AlphaRange { min: 0, max: 0x40 }, o), vec![GsPass::ColorOnlyLowAlpha]);
    }
}
