//! The game's projection, a fly camera, and the per-frame tfrag LOD decision.
//!
//! World space is rc-engine's (right-handed, Y up: `game_to_world` is its `game_to_bevy`), so the shader and the
//! storage layouts stay those of `tfrag.wgsl`. The projection is rc-engine `game_camera::GameProjection` (reverse Z,
//! depth = GS Z / 2^24); the LOD is rc-engine `tfrag_lod` (`TfragProc`, `SetTfragDists`), without the occlusion test.

use crate::level::{Level, LevelFog};
use glam::{Mat4, Vec3, Vec4};
use rc_formats::tfrag::{Tfrag, TfragBlockHeader};

/// Position units per game unit.
pub const UNITS: f32 = 1024.0;
/// `0x18cda0` near and `0x18cda4` far (integer units).
pub const NEAR: f32 = 32.0;
pub const FAR: f32 = 745472.0;
/// `0x18cdb0`: tangent of the horizontal half field of view; NTSC vertical = horizontal × 0.775.
pub const TAN_X: f32 = 0.63;
pub const TAN_Y: f32 = TAN_X * 0.775;
const Z_SCALE: f32 = -8388080.0;
const Z_OFFSET: f32 = 8388112.0;

/// Game (Z up) → world (Y up): `(x, y, z) → (x, z, −y)`, a rotation, so winding is kept.
pub fn game_to_world(p: [f32; 3]) -> Vec3 { Vec3::new(p[0], p[2], -p[1]) }

/// The game's perspective as a reverse-Z clip matrix (rc-engine `GameProjection::get_clip_from_view`).
pub fn clip_from_view() -> Mat4 {
    let (n, f, zs) = (NEAR as f64, FAR as f64, Z_SCALE as f64);
    let a_m = (f + n) / (n * (f - n)) * zs;
    let c_m = (n * -2.0 * f) / (n * (f - n)) * zs;
    let (a, b) = (n * a_m + Z_OFFSET as f64, n * c_m / UNITS as f64);
    let s = 1.0 / 16_777_216.0;
    Mat4::from_cols(
        Vec4::new(1.0 / TAN_X, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 1.0 / TAN_Y, 0.0, 0.0),
        Vec4::new(0.0, 0.0, (-a * s) as f32, -1.0),
        Vec4::new(0.0, 0.0, (b * s) as f32, 0.0),
    )
}

/// A free camera: eye in world units, yaw about +Y and pitch.
pub struct FlyCam {
    pub eye: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub speed: f32,
}

impl FlyCam {
    /// `RC_CAM` (`ex,ey,ez,tx,ty,tz`, game units), else framed on the tfrag bounding spheres as rc-engine's
    /// `setup_camera` does: up and back from their centroid, looking at it.
    pub fn new(level: &Level) -> Self {
        let spheres: Vec<[f32; 4]> = level.tfrags.iter().map(|t| t.header.bsphere.map(|v| v / UNITS)).collect();
        let n = spheres.len().max(1) as f32;
        let c = spheres.iter().fold(Vec3::ZERO, |a, s| a + Vec3::new(s[0], s[1], s[2])) / n;
        let extent = spheres.iter().map(|s| (Vec3::new(s[0], s[1], s[2]) - c).length() + s[3]).fold(0.0f32, f32::max);
        let (eye, target) = match std::env::var("RC_CAM").ok().and_then(|v| parse_cam(&v)) {
            Some((e, t)) => (game_to_world(e), game_to_world(t)),
            None => (game_to_world((c + Vec3::new(0.0, -0.35 * extent, 0.25 * extent)).to_array()), game_to_world(c.to_array())),
        };
        let d = (target - eye).normalize_or(Vec3::NEG_Z);
        FlyCam { eye, yaw: d.x.atan2(-d.z), pitch: d.y.clamp(-1.0, 1.0).asin(), speed: (extent / 20.0).clamp(5.0, 200.0) }
    }

    pub fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(sy * cp, sp, -cy * cp)
    }

    pub fn right(&self) -> Vec3 { self.forward().cross(Vec3::Y).normalize() }

    pub fn view_from_world(&self) -> Mat4 { Mat4::look_to_rh(self.eye, self.forward(), Vec3::Y) }

    /// Moves by `dir` (camera-relative: x right, y up, z forward) for `dt` seconds.
    pub fn fly(&mut self, dir: Vec3, fast: bool, dt: f32) {
        let v = self.right() * dir.x + Vec3::Y * dir.y + self.forward() * dir.z;
        self.eye += v * self.speed * if fast { 4.0 } else { 1.0 } * dt;
    }

    pub fn look(&mut self, dx: f32, dy: f32) {
        self.yaw += dx * 0.003;
        self.pitch = (self.pitch - dy * 0.003).clamp(-1.55, 1.55);
    }

    /// The game's camera position (raw units) and view rows (x = −left, y = −up, z = forward), game axes.
    fn game_view(&self) -> (Vec3, [Vec3; 3]) {
        let to_game = |v: Vec3| Vec3::new(v.x, -v.z, v.y);
        let (f, r) = (self.forward(), self.right());
        let up = r.cross(f);
        (to_game(self.eye) * UNITS, [to_game(r), to_game(-up), to_game(f)])
    }
}

/// `ex,ey,ez,tx,ty,tz` in game coordinates.
fn parse_cam(s: &str) -> Option<([f32; 3], [f32; 3])> {
    let v: Vec<f32> = s.split(',').map(|x| x.trim().parse().ok()).collect::<Option<_>>()?;
    (v.len() == 6).then(|| ([v[0], v[1], v[2]], [v[3], v[4], v[5]]))
}

pub const MODE_CULLED: u32 = 0;
pub const MODE_CLIP: u32 = 2;

/// VU1 constants qw666..669 (`SetTfragDists`), the w slope, and the per-batch MXL: the `TfragLod` block of
/// `tfrag.metal` (rc-engine `TfragLodUniform`).
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct LodBlock {
    pub k666: [f32; 4],
    pub k667: [f32; 4],
    pub k668: [f32; 4],
    pub k669: [f32; 4],
    /// x = VU w per raw unit of depth, y = 1 to tint by LOD (`RC_LOD_TINT=1`), z = near.
    pub misc: [f32; 4],
    /// y = MXL.
    pub tex: [f32; 4],
}

impl LodBlock {
    pub fn new(block: &TfragBlockHeader, fog: &LevelFog) -> Self {
        let [d0, d1, d2] = block.lod_distances();
        let cf20 = (fog.far_intensity - fog.near_intensity) / ((fog.far_dist - fog.near_dist) * 0.0009765625);
        let (f0, f1, f2) = (d0 * cf20, d1 * cf20, d2 * cf20);
        let a = 1.0 / (f0 - f1);
        let b = 1.0 / (f1 - f2);
        let tint = std::env::var("RC_LOD_TINT").is_ok_and(|v| v.trim() == "1");
        LodBlock {
            k666: [a * 0.5, -a, 0.0, f0],
            k667: [b * 0.5, -b, 0.0, f1],
            k668: [f1 * a * -0.5, f0 * a, 0.0, 0.0],
            k669: [f2 * b * -0.5, f1 * b, 0.0, 0.0],
            misc: [fog.vu_terms().1, if tint { 1.0 } else { 0.0 }, NEAR, 0.0],
            tex: [0.0; 4],
        }
    }
}

/// What `TfragProc` reads per tfrag: bounding sphere and clip-box corners (raw units) and the header.
pub struct TfragCull {
    sphere: [f32; 4],
    corners: [Vec3; 8],
    header_only: Tfrag,
}

pub struct TfragLod {
    tfrags: Vec<TfragCull>,
    thresholds: [i32; 3],
    force_lod0: bool,
}

impl TfragLod {
    pub fn new(level: &Level) -> Self {
        let tfrags = level
            .tfrags
            .iter()
            .map(|t| TfragCull {
                sphere: t.header.bsphere,
                corners: t.cube.map(|c| Vec3::new(c[0] as f32 * 64.0, c[1] as f32 * 64.0, c[2] as f32 * 64.0)),
                header_only: Tfrag { header: t.header, ..Default::default() },
            })
            .collect();
        TfragLod { tfrags, thresholds: level.block.lod_thresholds_raw(), force_lod0: std::env::var("RC_LOD").is_ok_and(|v| v.trim() == "0") }
    }

    /// This frame's draw mode per tfrag (0 culled, 2 clipping path, else the VU1 MSCAL entry).
    pub fn modes(&self, cam: &FlyCam, out: &mut Vec<u32>) {
        let (eye, rows) = cam.game_view();
        out.clear();
        out.extend(self.tfrags.iter().map(|t| {
            let m = tfrag_proc(t, eye, rows, self.thresholds, FAR);
            if self.force_lod0 && m != MODE_CULLED { 0x14 } else { m }
        }));
    }
}

/// VU CLIP judgement of one vertex against |w| (bits: +x, −x, +y, −y, +z, −z).
fn clip_flags(c: Vec4) -> u32 {
    let w = c.w.abs();
    (c.x > w) as u32 | ((c.x < -w) as u32) << 1 | ((c.y > w) as u32) << 2 | ((c.y < -w) as u32) << 3
        | ((c.z > w) as u32) << 4 | ((c.z < -w) as u32) << 5
}

/// `TfragProc`'s decision for one tfrag (rc-engine `tfrag_lod::tfrag_proc`, which documents each test).
fn tfrag_proc(info: &TfragCull, cam: Vec3, rows: [Vec3; 3], thresholds: [i32; 3], f: f32) -> u32 {
    let (tx, ty) = (TAN_X, TAN_Y);
    let (kx, ky) = ((1.0 + tx * tx).sqrt(), (1.0 + ty * ty).sqrt());
    let [sx, sy, sz, r] = info.sphere;
    let d = Vec3::new(sx, sy, sz) - cam;
    let p = Vec3::new(rows[0].dot(d), rows[1].dot(d), rows[2].dot(d));
    let (far, near) = (r + p.z, -r + p.z);
    if 512000.0 - near < 0.0 || NEAR - far >= 0.0 { return MODE_CULLED; }
    let (ax, ay) = (p.x.abs(), p.y.abs());
    if tx * p.z - (ax - r * kx) < 0.0 || ty * p.z - (ay - r * ky) < 0.0 { return MODE_CULLED; }
    let inside = tx * p.z - (ax + r * kx) >= 0.0 && ty * p.z - (ay + r * ky) >= 0.0 && far - f < 0.0 && near - NEAR >= 0.0;
    if !inside {
        let (mut all_out, mut any_guard) = (0x3fu32, 0u32);
        let (a, b) = ((f + NEAR) / (NEAR * (f - NEAR)), (NEAR * -2.0 * f) / (NEAR * (f - NEAR)));
        for c in &info.corners {
            let d = *c - cam;
            let v = Vec3::new(rows[0].dot(d), rows[1].dot(d), rows[2].dot(d));
            let clip = Vec4::new(v.x * (1.0 / (tx * NEAR)), v.y * (1.0 / (ty * NEAR)), v.z * a + b, v.z * (1.0 / NEAR));
            all_out &= clip_flags(clip);
            any_guard |= clip_flags(Vec4::new(clip.x * 0.25, clip.y * 0.25, clip.z, clip.w));
        }
        if all_out != 0 { return MODE_CULLED; }
        if any_guard != 0 { return MODE_CLIP; }
    }
    info.header_only.draw_mode(p.z, thresholds).mscal() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same cases as rc-engine's `tfrag_lod::tests::tfrag_proc_culls_and_selects`.
    #[test]
    fn tfrag_proc_culls_and_selects() {
        let th = TfragBlockHeader { table_offset: 0, tfrag_count: 1, unknown_8: 15.0, unknown_c: 0 }.lod_thresholds_raw();
        let rows = [Vec3::new(0.0, -1.0, 0.0), Vec3::new(0.0, 0.0, -1.0), Vec3::X];
        let m = |x: f32, y: f32, r: f32| {
            let c = Vec3::new(x, y, 0.0) * 1024.0;
            let corners = std::array::from_fn(|i| {
                c + Vec3::new(if i & 1 == 0 { -r } else { r }, if i & 2 == 0 { -r } else { r }, if i & 4 == 0 { -r } else { r }) * 1024.0
            });
            let mut t = Tfrag::default();
            t.header.bsphere = [c.x, c.y, c.z, r * 1024.0];
            tfrag_proc(&TfragCull { sphere: t.header.bsphere, corners, header_only: t }, Vec3::ZERO, rows, th, FAR)
        };
        assert_eq!(m(20.0, 0.0, 4.0), 0x14);
        assert_eq!(m(40.0, 0.0, 4.0), 0x10);
        assert_eq!(m(58.0, 0.0, 4.0), 0xe);
        assert_eq!(m(70.0, 0.0, 4.0), 0xa);
        assert_eq!(m(88.0, 0.0, 4.0), 8);
        assert_eq!(m(120.0, 0.0, 4.0), 6);
        assert_eq!(m(-40.0, 0.0, 4.0), MODE_CULLED);
        assert_eq!(m(600.0, 0.0, 4.0), MODE_CULLED);
        assert_eq!(m(40.0, 200.0, 4.0), MODE_CULLED);
        assert_eq!(m(2.0, 0.0, 4.0), MODE_CLIP);
    }

    /// The camera rows match rc-engine's `game_rows` for a camera looking along game +X.
    #[test]
    fn game_view_rows() {
        let cam = FlyCam { eye: Vec3::ZERO, yaw: std::f32::consts::FRAC_PI_2, pitch: 0.0, speed: 1.0 };
        let (_, [x, y, z]) = cam.game_view();
        assert!((z - Vec3::X).length() < 1e-5, "forward {z}");
        assert!((x - Vec3::new(0.0, -1.0, 0.0)).length() < 1e-5, "x {x}");
        assert!((y - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-5, "y {y}");
    }
}
