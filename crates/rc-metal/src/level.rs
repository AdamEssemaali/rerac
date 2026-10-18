//! The part of a level the Metal renderer draws so far, read from the game data folder.
//!
//! The same call chain as rc-engine's `level_load::load_level`, cut down to the terrain: core index and data, the tfrags
//! relit by the game's `LightTfrags` pass (rc-engine `tfrag_light.rs`), the tfrag block header and mip chains
//! (rc-engine `tfrag_lod::load`), and the level-settings fog and background colour (rc-engine `game_camera.rs`). Those
//! rc-engine modules depend on Bevy, so the few Bevy-free lines this needs are repeated here; the readers themselves
//! are rc-formats'.

use anyhow::{Context, Result};
use rc_formats::level::{self, LevelCore};
use rc_formats::texture::{self, Texture};
use rc_formats::tfrag::{self, Tfrag, TfragBlockHeader};
use rc_formats::tfrag_light::{self, NormalTable, BOOT_NORMAL_TABLE_VADDR};
use std::path::{Path, PathBuf};

/// Novalis, the level the boot loads (rc-engine `level_load::DEFAULT_LEVEL`).
pub const DEFAULT_LEVEL: u32 = 1;

pub struct Level {
    pub index: u32,
    pub tfrags: Vec<Tfrag>,
    /// Tfrags block header (`unknown_8` = the LOD base distance L).
    pub block: TfragBlockHeader,
    /// Full mip chain per `LevelCore::tfrag_textures` index (`None` where the entry has no pixels).
    pub mips: Vec<Option<Vec<Texture>>>,
    pub fog: LevelFog,
    /// The GS frame clear colour (display-encoded bytes).
    pub background: [u8; 3],
}

/// The game data folder: `--data-dir <dir>`, `RC_DATA_DIR`, `RC_EXTRACTED`, else the development `extracted/`
/// (rc-engine `disc_source`, without its `extract-info.json` checks).
pub fn data_root(arg: Option<PathBuf>) -> PathBuf {
    arg.or_else(|| std::env::var_os("RC_DATA_DIR").filter(|v| !v.is_empty()).map(PathBuf::from))
        .or_else(|| std::env::var_os("RC_EXTRACTED").filter(|v| !v.is_empty()).map(PathBuf::from))
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extracted"))
}

/// `RC_LEVEL` (decimal level index), else Novalis.
pub fn level_index() -> Result<u32> {
    match std::env::var("RC_LEVEL") {
        Ok(s) => s.trim().parse().with_context(|| format!("RC_LEVEL must be a level index, got {s:?}")),
        Err(_) => Ok(DEFAULT_LEVEL),
    }
}

pub fn load(root: &Path, index: u32) -> Result<Level> {
    let read = |name: &str| {
        let p = root.join(format!("levels/{index:02}/{name}"));
        std::fs::read(&p).with_context(|| format!("reading {}", p.display()))
    };
    let core_index = read("core_index.bin")?;
    let gs_ram = read("gs_ram.bin")?;
    let core_data = rc_data::level_core_data(root, index).context("decompressing core_data")?;
    let core = level::parse_level_core(&core_index, core_data.len()).context("parsing core index")?;
    let gameplay = rc_data::level_gameplay(root, index).context("decompressing gameplay_ntsc")?;

    let mut tfrags = tfrag::parse_level_tfrags(&core, &core_data).context("parsing tfrags")?;
    light_tfrags(root, &gameplay, &mut tfrags)?;
    let block = tfrag::parse_tfrag_block_header(tfrag::tfrag_block(&core, &core_data)?)?;
    let mips = tfrag_mips(&core, &core_data, &gs_ram)?;
    let fog = LevelFog::parse(&gameplay).context("parsing level settings")?;
    let background = level_background(&gameplay).context("parsing level settings")?;
    Ok(Level { index, tfrags, block, mips, fog, background })
}

/// The game's load-time `LightTfrags` pass over every tfrag (rc-engine `tfrag_light::light_level_tfrags`);
/// `RC_NO_LIGHT=1` keeps the stored RGBA.
fn light_tfrags(root: &Path, gameplay: &[u8], tfrags: &mut [Tfrag]) -> Result<()> {
    if std::env::var("RC_NO_LIGHT").is_ok_and(|v| v != "0") { return Ok(()); }
    let bank = tfrag_light::parse_light_bank(gameplay).context("parsing directional lights")?;
    let elf = root.join("boot/SCUS_971.99");
    let elf = std::fs::read(&elf).with_context(|| format!("reading {}", elf.display()))?;
    let table = NormalTable::from_elf(&elf, BOOT_NORMAL_TABLE_VADDR).context("reading the normal table from the boot ELF")?;
    for t in tfrags.iter_mut() {
        t.rgba = tfrag_light::light_tfrag(t, &bank, &table, None);
    }
    Ok(())
}

/// Every tfrag texture's mip chain (rc-engine `tfrag_lod::load`).
fn tfrag_mips(core: &LevelCore, core_data: &[u8], gs_ram: &[u8]) -> Result<Vec<Option<Vec<Texture>>>> {
    core.tfrag_textures
        .iter()
        .map(|e| {
            if e.width <= 0 || e.height <= 0 || e.data_offset < 0 || e.palette < 0 { return Ok(None); }
            texture::decode_tfrag_mip_levels(core, core_data, gs_ram, e).map(Some)
        })
        .collect::<rc_formats::buf::Result<Vec<_>>>()
        .context("decoding tfrag mip levels")
}

/// The u32 at byte `off` of the level-settings section (gameplay file pointer 0).
fn level_settings_word(gameplay: &[u8], off: usize) -> Result<u32> {
    let u32_at = |o: usize| -> Result<u32> {
        let b = gameplay.get(o..o + 4).context("gameplay file too short for level settings")?;
        Ok(u32::from_le_bytes(b.try_into().unwrap()))
    };
    let base = u32_at(0)? as usize;
    anyhow::ensure!(base != 0, "gameplay file has no level-settings section");
    u32_at(base + off)
}

/// Level settings +0x00/+0x04/+0x08 as `set_background_color` packs them (rc-engine `game_camera::level_background`).
fn level_background(gameplay: &[u8]) -> Result<[u8; 3]> {
    let w = |o: usize| level_settings_word(gameplay, o);
    let rgbaq = w(0x00)? | w(0x04)? << 8 | w(0x08)? << 16 | 0x8000_0000;
    Ok([rgbaq as u8, (rgbaq >> 8) as u8, (rgbaq >> 16) as u8])
}

/// The level-settings fog fields (rc-engine `game_camera::LevelFog`; derivation in docs/plan/game_camera_fog.md).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelFog {
    /// FOGCOL.
    pub color: [u8; 3],
    /// Integer units (depth along the view axis).
    pub near_dist: f32,
    pub far_dist: f32,
    /// GS fog coefficient F at those depths (255 = no fog).
    pub near_intensity: f32,
    pub far_intensity: f32,
}

impl LevelFog {
    pub fn parse(gameplay: &[u8]) -> Result<Self> {
        let w = |o: usize| level_settings_word(gameplay, o);
        let f = |o: usize| w(o).map(f32::from_bits);
        Ok(LevelFog {
            color: [w(0x0c)? as u8, w(0x10)? as u8, w(0x14)? as u8],
            near_dist: f(0x18)?,
            far_dist: f(0x1c)?,
            near_intensity: f(0x20)?,
            far_intensity: f(0x24)?,
        })
    }

    /// `UpdateViewContext`'s fog terms in f32 like the EE: (qw656.x = Q numerator, w-lane slope, qw661.w).
    pub fn vu_terms(&self) -> (f32, f32, f32) {
        let range = self.far_dist - self.near_dist;
        let q_num = ((self.far_intensity - self.near_intensity) * crate::camera::NEAR) / range;
        let slope = (1.0 / crate::camera::NEAR) * q_num;
        let offset = (self.near_intensity * self.far_dist - self.far_intensity * self.near_dist) / range;
        (q_num, slope, offset)
    }

    /// The world shaders' fog block (rc-engine `TfragFog` + the graphics word): FOGCOL (w = 1 enabled, `RC_FOG=0`
    /// disables), (slope, offset, lower clamp, upper clamp), and the texture option (0 = the GS level rule).
    pub fn shader_block(&self) -> [f32; 12] {
        let enabled = std::env::var("RC_FOG").map(|v| v.trim() != "0").unwrap_or(true);
        let (_, slope, offset) = self.vu_terms();
        let [r, g, b] = self.color.map(|c| c as f32 / 255.0);
        [r, g, b, if enabled { 1.0 } else { 0.0 }, slope, offset, self.far_intensity, self.near_intensity, 0.0, 1.0, 0.0, 0.0]
    }
}
