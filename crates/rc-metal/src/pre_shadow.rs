//! The world's colour-only draws before the shadow pass (docs/plan/shadows.md §7 risk 5): the halves of the
//! world's alpha tests that write colour only (`GsPass::ColorOnlyLowAlpha`: tfrags, ties, shrubs) and the shrub
//! billboards' pass 2 (`GsPass::BlendNoZ`). The game draws them with the world, before the shadow volumes darken the
//! frame, so a shadow darkens them too; Bevy would draw them in the transparent pass, after crate::shadow_render's
//! darkening. Their items are taken out of the view's `Transparent3d` phase (in sorted order) and drawn right after
//! the opaque pass, against the scene depth, then put back for Bevy's retained phase (as crate::display_blend does
//! for the effects).
//!
//! Which entities: [`BeforeShadows`], added when a world entity first gets a material whose GS pass is one of the
//! two ([`tag`], one system per world material type). The split, the ordering and the draw are crate::render::frame's.
//!
//! Order: the game's, not Bevy's back-to-front sort ([`order`], between Bevy's sort and its batching). The GS frame
//! runs sky → tfrag → tie → shrub → billboards (docs/plan/shadows.md §3.1), and inside each:
//! * tfrags: the port's texture batches in their build order (the game draws per tfrag; a batch merges many);
//! * ties: `TieProc` (boot 0x235be8) per class, then per packet across the class's visible instances in list
//!   order: the port's (class, part, instance);
//! * shrubs: `ShrubProc` (boot 0x228be8) draws the opaque list class by class, then sends the fading test block
//!   and draws the fading list class by class: (list, class, part, instance); the game sends all of an instance's
//!   packets together, the port each part across the class's instances (as in the opaque phase);
//! * billboards: pass 1 per class in class order, then pass 2 per class.
//!
//! Every renderer puts a [`WorldDrawOrder`] on its entities. Besides following the game, the order puts each
//! class part's instances next to each other, so Bevy draws them as one instanced draw instead of one draw per
//! instance interleaved by distance (and resets the pipeline only between groups).

use crate::gs_state::GsPass;
use crate::prelude::*;

pub use crate::render::frame::{BeforeShadows, WorldDrawOrder};

/// An entity [`tag`] has looked at (rc-engine's `Added<MeshMaterial3d<M>>` filter: each entity is judged once, on the
/// first frame it has its material).
struct PreShadowSeen;

/// PostUpdate: tags the new entities of material `M` whose GS pass writes colour only.
pub fn tag<M: Material>(ctx: &mut Ctx)
where
    for<'a> GsPass: From<&'a M>,
{
    let Some(mats) = ctx.try_res::<Assets<M>>() else { return };
    let mut seen = Vec::new();
    let mut tagged = Vec::new();
    for (e, m) in ctx.world.query::<(Entity, &MeshMaterial3d<M>)>().without::<&PreShadowSeen>().iter() {
        seen.push(e);
        let Some(mat) = mats.get(&m.0) else { continue };
        if matches!(GsPass::from(mat), GsPass::ColorOnlyLowAlpha { .. } | GsPass::BlendNoZ) { tagged.push(e); }
    }
    drop(mats);
    for e in seen { let _ = ctx.world.insert_one(e, PreShadowSeen); }
    for e in tagged { let _ = ctx.world.insert_one(e, BeforeShadows); }
}
