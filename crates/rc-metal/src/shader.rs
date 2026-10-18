//! rc-engine's WGSL shaders, translated to MSL with naga: one shader source for both renderers.
//!
//! The WGSL files stay in `crates/rc-engine/assets/shaders` (embedded here at build time). They are written for Bevy's
//! shader preprocessor (naga_oil); [`preprocess`] implements the subset they use:
//! * `#import path::{a, b::c}` / `#import path::item`: the module defining that path (`#define_import_path`) is inlined
//!   once, before the importing source. Bevy's modules are replaced by rc-metal's stand-ins (`shaders/shim/`), which
//!   define exactly what the shaders read; qualified references (`mesh_functions::get_tag`) lose their qualifier
//!   (WGSL itself has no `::`, so every `ident::` outside a directive is a module path);
//! * `#ifdef` / `#ifndef` / `#else` / `#else ifdef` / `#endif`, and `#{NAME}` replaced by a definition's value.
//!
//! [`compile`] parses and validates the result with naga and writes MSL with an explicit binding map (the same for
//! every entry point of a module): per stage, buffers (uniform and storage) take Metal buffer slots 0, 1, … in
//! declaration order, textures and samplers likewise their own slots; a module with runtime-sized storage arrays gets
//! naga's sizes buffer in the next free buffer slot. Vertex attributes are `[[stage_in]]` at their WGSL locations; the
//! vertex buffers themselves bind from [`VERTEX_BUFFER_BASE`] up (crate::gfx).

use anyhow::{anyhow, bail, Context, Result};
use std::collections::{BTreeMap, HashSet};

/// First Metal buffer slot of the vertex buffers (resources stay below it).
pub const VERTEX_BUFFER_BASE: u64 = 16;

macro_rules! sources {
    ($dir:literal: $($name:literal),* $(,)?) => {
        &[$(($name, include_str!(concat!($dir, $name, ".wgsl")))),*]
    };
}

/// rc-engine's shaders by file stem.
const ENGINE: &[(&str, &str)] = sources!("../../rc-engine/assets/shaders/":
    "aa_blit", "display_blend", "effect_blit", "fade", "fx_prim", "hud", "menu_snapshot", "mirror", "missile_view",
    "moby", "moby_metal", "particle", "screen_tint", "shadow_resolve", "shadow_volume", "shrub", "shrub_billboard",
    "sky", "sky_stars", "tfrag", "tie", "underwater_tint", "water", "world_lights",
);

/// rc-metal's stand-ins for the Bevy shader modules (and its own shaders).
const SHIMS: &[(&str, &str)] = sources!("../shaders/shim/":
    "forward_io", "fullscreen_vertex_shader", "mesh_functions", "mesh_view_bindings", "sprite", "standard_unlit", "ui_vertex_output",
    "view_transformations",
);

/// A shader definition: a flag (`#ifdef`) or a value (`#{NAME}`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Def {
    Flag,
    UInt(u32),
    Int(i32),
}

pub type Defs = BTreeMap<String, Def>;

/// Defs from `(name, value)` pairs; a `None` value is a flag.
pub fn defs<'a>(items: impl IntoIterator<Item = (&'a str, Option<u32>)>) -> Defs {
    items.into_iter().map(|(k, v)| (k.to_string(), v.map_or(Def::Flag, Def::UInt))).collect()
}

fn source(name: &str) -> Result<&'static str> {
    ENGINE.iter().chain(SHIMS).find(|(n, _)| *n == name).map(|(_, s)| *s).ok_or_else(|| anyhow!("no shader named {name:?}"))
}

/// The module defining each `#define_import_path`.
fn module_paths() -> BTreeMap<String, &'static str> {
    let mut out = BTreeMap::new();
    for (_, src) in ENGINE.iter().chain(SHIMS) {
        if let Some(p) = src.lines().find_map(|l| l.trim().strip_prefix("#define_import_path ")) {
            out.insert(p.trim().to_string(), *src);
        }
    }
    out
}

/// The full paths of an import list: `a::{b::c, d}` → `a::b::c`, `a::d`.
fn expand_import(spec: &str) -> Vec<String> {
    let spec: String = spec.split_whitespace().collect();
    match spec.find('{') {
        None => vec![spec],
        Some(open) => {
            let prefix = &spec[..open];
            let inner = spec[open + 1..].strip_suffix('}').unwrap_or(&spec[open + 1..]);
            // Split on top-level commas.
            let (mut parts, mut depth, mut start) = (Vec::new(), 0, 0);
            for (i, c) in inner.char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    ',' if depth == 0 => { parts.push(&inner[start..i]); start = i + 1; }
                    _ => {}
                }
            }
            parts.push(&inner[start..]);
            parts.into_iter().filter(|s| !s.is_empty()).flat_map(|s| expand_import(&format!("{prefix}{s}"))).collect()
        }
    }
}

/// Applies `#ifdef` / `#ifndef` / `#else` / `#endif` and `#{NAME}`; returns the kept lines and the import specs.
fn conditionals(src: &str, defs: &Defs) -> Result<(Vec<String>, Vec<String>)> {
    let mut out = Vec::new();
    let mut imports = Vec::new();
    // Per open block: (this branch is kept, an earlier branch was kept, the enclosing context is kept).
    let mut stack: Vec<(bool, bool, bool)> = Vec::new();
    let mut lines = src.lines();
    while let Some(line) = lines.next() {
        let t = line.trim();
        let active = stack.last().is_none_or(|&(on, _, outer)| on && outer);
        if let Some(name) = t.strip_prefix("#ifdef ") {
            let on = defs.contains_key(name.trim());
            stack.push((on, on, active));
        } else if let Some(name) = t.strip_prefix("#ifndef ") {
            let on = !defs.contains_key(name.trim());
            stack.push((on, on, active));
        } else if let Some(name) = t.strip_prefix("#else ifdef ") {
            let top = stack.last_mut().ok_or_else(|| anyhow!("#else without #ifdef"))?;
            top.0 = !top.1 && defs.contains_key(name.trim());
            top.1 |= top.0;
        } else if t == "#else" {
            let top = stack.last_mut().ok_or_else(|| anyhow!("#else without #ifdef"))?;
            top.0 = !top.1;
            top.1 = true;
        } else if t == "#endif" {
            stack.pop().ok_or_else(|| anyhow!("#endif without #ifdef"))?;
        } else if !active || t.starts_with("#define_import_path") {
            continue;
        } else if let Some(rest) = t.strip_prefix("#import ") {
            // An import list may span lines until its braces close.
            let mut spec = rest.to_string();
            while spec.matches('{').count() > spec.matches('}').count() {
                spec.push_str(lines.next().ok_or_else(|| anyhow!("unterminated #import"))?);
            }
            imports.push(spec);
        } else if t.starts_with('#') && !t.starts_with("#{") {
            bail!("unsupported directive {t:?}");
        } else {
            out.push(substitute(line, defs)?);
        }
    }
    if !stack.is_empty() { bail!("unterminated #ifdef"); }
    Ok((out, imports))
}

/// `#{NAME}` → the definition's value; `ident::` (a module qualifier) → nothing.
fn substitute(line: &str, defs: &Defs) -> Result<String> {
    let mut s = line.to_string();
    while let Some(i) = s.find("#{") {
        let j = s[i..].find('}').ok_or_else(|| anyhow!("unterminated #{{ in {line:?}"))? + i;
        let name = &s[i + 2..j];
        let v = match defs.get(name) {
            Some(Def::UInt(v)) => format!("{v}u"),
            Some(Def::Int(v)) => format!("{v}"),
            Some(Def::Flag) => bail!("#{{{name}}} needs a value"),
            None if name == "MATERIAL_BIND_GROUP" => "2".to_string(),
            None => bail!("#{{{name}}} is not defined"),
        };
        s.replace_range(i..=j, &v);
    }
    // Comments may say `a::b`; dropping qualifiers there is harmless.
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut k = 0;
    while k < b.len() {
        if b[k].is_ascii_alphabetic() || b[k] == b'_' {
            let start = k;
            while k < b.len() && (b[k].is_ascii_alphanumeric() || b[k] == b'_') { k += 1; }
            if s[k..].starts_with("::") { k += 2; continue; }
            out.push_str(&s[start..k]);
        } else {
            out.push(b[k] as char);
            k += 1;
        }
    }
    Ok(out)
}

/// The WGSL of shader `name` with `defs`, its imports inlined (each module once, dependencies first).
pub fn preprocess(name: &str, defs: &Defs) -> Result<String> {
    let paths = module_paths();
    let mut done = HashSet::new();
    let mut out = Vec::new();
    fn visit(
        src: &'static str,
        defs: &Defs,
        paths: &BTreeMap<String, &'static str>,
        done: &mut HashSet<*const u8>,
        out: &mut Vec<String>,
    ) -> Result<()> {
        if !done.insert(src.as_ptr()) { return Ok(()); }
        let (lines, imports) = conditionals(src, defs)?;
        for spec in imports {
            for full in expand_import(&spec) {
                // The longest module path that `full` names or lies inside.
                let module = paths
                    .iter()
                    .filter(|(p, _)| full == **p || full.starts_with(&format!("{p}::")))
                    .max_by_key(|(p, _)| p.len())
                    .map(|(_, s)| *s)
                    .ok_or_else(|| anyhow!("#import {full}: no module defines it"))?;
                visit(module, defs, paths, done, out)?;
            }
        }
        out.extend(lines);
        Ok(())
    }
    visit(source(name)?, defs, &paths, &mut done, &mut out).with_context(|| format!("preprocessing {name}.wgsl"))?;
    Ok(out.join("\n"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    /// A uniform or storage buffer.
    Buffer,
    Texture,
    Sampler,
}

/// Where a WGSL `@group(g) @binding(b)` resource lives in Metal (the same slot in every stage that uses it).
#[derive(Clone, Copy, Debug)]
pub struct Resource {
    pub group: u32,
    pub binding: u32,
    pub kind: ResourceKind,
    pub slot: u64,
}

/// A translated module: its MSL and the binding map (shared by its entry points).
pub struct Compiled {
    pub msl: String,
    /// Entry point name in WGSL → name in the MSL.
    pub entry_points: BTreeMap<String, String>,
    pub resources: Vec<Resource>,
    /// naga's sizes buffer slot and the buffers it describes, in order (byte sizes are filled per draw).
    pub sizes: Option<(u64, Vec<(u32, u32)>)>,
}

impl Compiled {
    pub fn slot(&self, group: u32, binding: u32) -> Option<&Resource> {
        self.resources.iter().find(|r| r.group == group && r.binding == binding)
    }
}

/// Parses, validates and translates WGSL to MSL (see the module doc for the binding map).
pub fn compile(wgsl: &str, label: &str) -> Result<Compiled> {
    use naga::back::msl;
    let module = naga::front::wgsl::parse_str(wgsl).map_err(|e| anyhow!("{label}: {}", e.emit_to_string(wgsl)))?;
    let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), msl::supported_capabilities())
        .validate(&module)
        .map_err(|e| anyhow!("{label}: {}", e.emit_to_string(wgsl)))?;

    let mut resources = Vec::new();
    let mut map = msl::BindingMap::default();
    let (mut nb, mut nt, mut ns) = (0u8, 0u8, 0u8);
    let mut sized = Vec::new();
    for (_, var) in module.global_variables.iter() {
        let Some(rb) = var.binding else { continue };
        let ty = &module.types[var.ty].inner;
        let mut target = msl::BindTarget::default();
        let kind = match var.space {
            naga::AddressSpace::Uniform | naga::AddressSpace::Storage { .. } => {
                if let naga::AddressSpace::Storage { access } = var.space {
                    target.mutable = access.contains(naga::StorageAccess::STORE);
                }
                // A runtime-sized array (directly or as the last struct member) needs its length from the sizes buffer.
                let runtime = match ty {
                    naga::TypeInner::Array { size: naga::ArraySize::Dynamic, .. } => true,
                    naga::TypeInner::Struct { members, .. } => members.last().is_some_and(|m| {
                        matches!(module.types[m.ty].inner, naga::TypeInner::Array { size: naga::ArraySize::Dynamic, .. })
                    }),
                    _ => false,
                };
                if runtime { sized.push((rb.group, rb.binding)); }
                target.buffer = Some(nb);
                nb += 1;
                ResourceKind::Buffer
            }
            naga::AddressSpace::Handle => match ty {
                naga::TypeInner::Sampler { .. } => {
                    target.sampler = Some(msl::BindSamplerTarget::Resource(ns));
                    ns += 1;
                    ResourceKind::Sampler
                }
                naga::TypeInner::Image { .. } => {
                    target.texture = Some(nt);
                    nt += 1;
                    ResourceKind::Texture
                }
                other => bail!("{label}: unsupported handle type {other:?}"),
            },
            other => bail!("{label}: unsupported address space {other:?}"),
        };
        let slot = match kind { ResourceKind::Buffer => nb - 1, ResourceKind::Texture => nt - 1, ResourceKind::Sampler => ns - 1 } as u64;
        resources.push(Resource { group: rb.group, binding: rb.binding, kind, slot });
        map.insert(rb, target);
    }
    if nb as u64 + 1 > VERTEX_BUFFER_BASE { bail!("{label}: {nb} buffers collide with the vertex buffer slots"); }
    let sizes_slot = (!sized.is_empty()).then_some(nb);
    let ep_res = msl::EntryPointResources { resources: map, immediates_buffer: None, sizes_buffer: sizes_slot };
    let options = msl::Options {
        lang_version: (2, 4),
        per_entry_point_map: module.entry_points.iter().map(|e| (e.name.clone(), ep_res.clone())).collect(),
        fake_missing_bindings: false,
        // The WGSL relies on WebGPU's bounds rules no more than wgpu's Metal backend gives it (which restricts too).
        bounds_check_policies: naga::proc::BoundsCheckPolicies {
            index: naga::proc::BoundsCheckPolicy::Restrict,
            buffer: naga::proc::BoundsCheckPolicy::Restrict,
            image_load: naga::proc::BoundsCheckPolicy::ReadZeroSkipWrite,
            binding_array: naga::proc::BoundsCheckPolicy::Restrict,
        },
        ..Default::default()
    };
    let pipeline = msl::PipelineOptions { vertex_pulling_transform: false, ..Default::default() };
    let (msl_src, tinfo) = msl::write_string(&module, &info, &options, &pipeline).map_err(|e| anyhow!("{label}: MSL: {e}"))?;
    let mut entry_points = BTreeMap::new();
    for (e, name) in module.entry_points.iter().zip(tinfo.entry_point_names) {
        entry_points.insert(e.name.clone(), name.map_err(|err| anyhow!("{label}: entry point {}: {err}", e.name))?);
    }
    Ok(Compiled { msl: msl_src, entry_points, resources, sizes: sizes_slot.map(|s| (s as u64, sized)) })
}

/// Preprocesses and translates shader `name` with `defs`.
pub fn load(name: &str, defs: &Defs) -> Result<Compiled> {
    let wgsl = preprocess(name, defs)?;
    let label = format!("{name}.wgsl {:?}", defs.keys().collect::<Vec<_>>());
    compile(&wgsl, &label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_lists_expand() {
        assert_eq!(expand_import("a::{b::c, d}"), vec!["a::b::c", "a::d"]);
        assert_eq!(expand_import("a::b::c"), vec!["a::b::c"]);
        assert_eq!(expand_import("bevy_pbr::{\n mesh_functions,\n view_transformations::{x, y},\n}"), vec![
            "bevy_pbr::mesh_functions",
            "bevy_pbr::view_transformations::x",
            "bevy_pbr::view_transformations::y"
        ]);
    }

    #[test]
    fn conditionals_and_values() {
        let d = defs([("A", None), ("N", Some(7))]);
        let (lines, _) = conditionals("#ifdef A\na #{N}\n#else\nb\n#endif\n#ifndef A\nc\n#endif\nm::f()", &d).unwrap();
        assert_eq!(lines, vec!["a 7u", "f()"]);
        let (lines, _) = conditionals("#ifdef X\nx\n#else ifdef A\na\n#else\ne\n#endif", &d).unwrap();
        assert_eq!(lines, vec!["a"]);
    }

    /// Every rc-engine shader translates with the definitions its pipelines use (the GPU compile is checked at start-up).
    #[test]
    fn every_engine_shader_translates() {
        let gs = |extra: &[(&'static str, Option<u32>)]| {
            let mut d = defs([("GS_AREF", Some(0x60))]);
            d.extend(defs(extra.iter().copied()));
            d
        };
        let cases: Vec<(&str, Defs)> = vec![
            ("tfrag", gs(&[])),
            ("tfrag", gs(&[("GS_ATEST_PASS", None)])),
            ("tie", gs(&[("GS_ATEST_FAIL", None)])),
            ("shrub", gs(&[])),
            ("shrub_billboard", gs(&[])),
            ("moby", gs(&[])),
            ("moby", gs(&[("DISPLAY_BLEND_MIX", None)])),
            ("moby_metal", gs(&[])),
            ("sky", gs(&[("VERTEX_COLORS", None)])),
            ("sky", gs(&[("VERTEX_UVS_A", None), ("VERTEX_COLORS", None), ("SKY_TEXTURED", None)])),
            ("sky_stars", gs(&[])),
            ("particle", gs(&[])),
            ("water", gs(&[])),
            ("fx_prim", gs(&[])),
            ("hud", gs(&[("HUD_PRIMS", None)])),
            ("hud", gs(&[("HUD_COMPOSITE", None)])),
            ("aa_blit", gs(&[])),
            ("effect_blit", gs(&[])),
            ("fade", gs(&[])),
            ("menu_snapshot", { let mut d = gs(&[]); d.insert("DARKEN".into(), Def::Int(0x30)); d }),
            ("mirror", gs(&[])),
            ("missile_view", gs(&[])),
            ("screen_tint", gs(&[])),
            ("shadow_resolve", gs(&[])),
            ("shadow_volume", gs(&[])),
            ("underwater_tint", gs(&[])),
        ];
        let mut failures = Vec::new();
        #[cfg(target_os = "macos")]
        let device = objc2_metal::MTLCreateSystemDefaultDevice();
        for (name, d) in cases {
            match load(name, &d) {
                Err(e) => failures.push(format!("{e:#}")),
                // The Metal compiler accepts naga's output (when this machine has a Metal device).
                #[cfg(target_os = "macos")]
                Ok(c) => {
                    use objc2_metal::MTLDevice;
                    if let Some(dev) = device.as_ref() {
                        if let Err(e) = dev.newLibraryWithSource_options_error(&objc2_foundation::NSString::from_str(&c.msl), None) {
                            failures.push(format!("{name} {:?}: Metal: {}", d.keys().collect::<Vec<_>>(), e.localizedDescription()));
                        }
                    }
                }
                #[cfg(not(target_os = "macos"))]
                Ok(_) => {}
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    }
}
