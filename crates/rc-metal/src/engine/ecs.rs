//! The ECS rc-engine's systems run on, without Bevy: a `hecs` world for entities and components, a type map of
//! resources borrowed at run time (Bevy's `Res` / `ResMut`), a deferred command queue (Bevy's `Commands`), and the
//! transform / visibility propagation Bevy runs in `PostUpdate`.
//!
//! A system is a plain function over a [`Ctx`]: it queries `ctx.world` (hecs checks component borrows at run time,
//! like Bevy's queries), borrows resources with `ctx.res::<T>()` / `ctx.res_mut::<T>()`, and queues spawns and
//! despawns on `ctx.commands()`, applied right after the system ([`Ctx::run`]), as Bevy applies them before the next
//! system that could see them.

use super::scene::{ChildOf, GlobalTransform, InheritedVisibility, Transform, ViewVisibility, Visibility};
use std::any::{Any, TypeId};
use std::cell::{Ref, RefCell, RefMut};
use std::collections::HashMap;

pub use hecs::{Entity, World};

/// Bevy's `Entity` order (rc-engine breaks equal sort keys by entity: the transparent phase, the binned phases, the UI
/// stack): generation first, then the index **descending**, because Bevy 0.19 stores the index as a `NonMaxU32` whose
/// bits are `!index` and compares entities by their bits. hecs allocates like Bevy (a freed index is reused last-in
/// first-out with the next generation), so the same spawn sequence gives the same order.
pub fn bevy_order(e: Entity) -> (u32, u32) {
    let bits = e.to_bits().get();
    ((bits >> 32) as u32, !(bits as u32))
}

/// Resources by type.
#[derive(Default)]
pub struct Resources {
    map: HashMap<TypeId, RefCell<Box<dyn Any>>>,
}

impl Resources {
    pub fn insert<T: 'static>(&mut self, v: T) { self.map.insert(TypeId::of::<T>(), RefCell::new(Box::new(v))); }

    pub fn remove<T: 'static>(&mut self) -> Option<T> {
        self.map.remove(&TypeId::of::<T>()).and_then(|c| c.into_inner().downcast().ok()).map(|b| *b)
    }

    pub fn contains<T: 'static>(&self) -> bool { self.map.contains_key(&TypeId::of::<T>()) }

    /// Panics when absent (Bevy's `Res<T>`), or when mutably borrowed elsewhere.
    pub fn get<T: 'static>(&self) -> Ref<'_, T> {
        self.try_get().unwrap_or_else(|| panic!("resource {} is missing", std::any::type_name::<T>()))
    }

    pub fn get_mut<T: 'static>(&self) -> RefMut<'_, T> {
        self.try_get_mut().unwrap_or_else(|| panic!("resource {} is missing", std::any::type_name::<T>()))
    }

    /// Bevy's `Option<Res<T>>`.
    pub fn try_get<T: 'static>(&self) -> Option<Ref<'_, T>> {
        let c = self.map.get(&TypeId::of::<T>())?;
        let r = c.try_borrow().unwrap_or_else(|_| panic!("resource {} is already borrowed mutably", std::any::type_name::<T>()));
        Some(Ref::map(r, |b| b.downcast_ref::<T>().unwrap()))
    }

    pub fn try_get_mut<T: 'static>(&self) -> Option<RefMut<'_, T>> {
        let c = self.map.get(&TypeId::of::<T>())?;
        let r = c.try_borrow_mut().unwrap_or_else(|_| panic!("resource {} is already borrowed", std::any::type_name::<T>()));
        Some(RefMut::map(r, |b| b.downcast_mut::<T>().unwrap()))
    }

    /// Bevy's `init_resource`.
    pub fn init<T: Default + 'static>(&mut self) {
        if !self.contains::<T>() { self.insert(T::default()); }
    }
}

type Command = Box<dyn FnOnce(&mut World, &mut Resources)>;

/// Deferred world changes (Bevy's `Commands`).
#[derive(Default)]
pub struct Commands {
    queue: Vec<Command>,
    /// Entities reserved by `spawn` before the queue is applied.
    reserved: Vec<Entity>,
}

impl Commands {
    pub fn add(&mut self, f: impl FnOnce(&mut World, &mut Resources) + 'static) { self.queue.push(Box::new(f)); }

    pub fn insert_resource<T: 'static>(&mut self, v: T) { self.add(move |_, r| r.insert(v)); }

    pub fn remove_resource<T: 'static>(&mut self) { self.add(|_, r| { r.remove::<T>(); }); }

    pub fn despawn(&mut self, e: Entity) { self.add(move |w, _| { despawn_recursive(w, e); }); }

    pub fn is_empty(&self) -> bool { self.queue.is_empty() }

    pub fn apply(&mut self, world: &mut World, res: &mut Resources) {
        // Commands queued by commands run after them, in order.
        while !self.queue.is_empty() {
            for c in std::mem::take(&mut self.queue) { c(world, res); }
        }
        self.reserved.clear();
    }
}

/// Despawns `e` and its descendants (Bevy despawns children with their parent).
pub fn despawn_recursive(world: &mut World, e: Entity) {
    let children: Vec<Entity> = world.query::<(Entity, &ChildOf)>().iter().filter(|(_, c)| c.0 == e).map(|(c, _)| c).collect();
    for c in children { despawn_recursive(world, c); }
    let _ = world.despawn(e);
}

/// What a system runs against.
pub struct Ctx {
    pub world: World,
    pub res: Resources,
    commands: RefCell<Commands>,
}

impl Default for Ctx {
    fn default() -> Self { Ctx { world: World::new(), res: Resources::default(), commands: RefCell::new(Commands::default()) } }
}

impl Ctx {
    pub fn res<T: 'static>(&self) -> Ref<'_, T> { self.res.get() }
    pub fn res_mut<T: 'static>(&self) -> RefMut<'_, T> { self.res.get_mut() }
    pub fn try_res<T: 'static>(&self) -> Option<Ref<'_, T>> { self.res.try_get() }
    pub fn try_res_mut<T: 'static>(&self) -> Option<RefMut<'_, T>> { self.res.try_get_mut() }
    pub fn has<T: 'static>(&self) -> bool { self.res.contains::<T>() }

    pub fn commands(&self) -> RefMut<'_, Commands> { self.commands.borrow_mut() }

    /// Spawns now (an entity id is needed at once, as Bevy's `commands.spawn(..).id()`): the world is not borrowed by
    /// the caller while it spawns. Prefer this to queuing for spawns.
    pub fn spawn(&mut self, components: impl hecs::DynamicBundle) -> Entity { self.world.spawn(components) }

    /// Applies the queued commands.
    pub fn flush(&mut self) {
        let mut c = std::mem::take(&mut *self.commands.borrow_mut());
        c.apply(&mut self.world, &mut self.res);
    }

    /// Runs one system and applies its commands.
    pub fn run(&mut self, system: fn(&mut Ctx)) {
        system(self);
        self.flush();
    }
}

/// Bevy's `PostUpdate` propagation: `GlobalTransform` from `Transform` and the parent chain, `InheritedVisibility` /
/// `ViewVisibility` from `Visibility` and the parents. Entities with a `Transform` get the derived components on first
/// sight, as Bevy's required components do.
pub fn propagate(world: &mut World) {
    // Required components.
    let missing: Vec<Entity> = world
        .query::<(Entity, &Transform)>()
        .without::<&GlobalTransform>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    for e in missing { let _ = world.insert_one(e, GlobalTransform::IDENTITY); }
    let missing: Vec<Entity> = world.query::<(Entity, &Visibility)>().without::<&InheritedVisibility>().iter().map(|(e, _)| e).collect();
    for e in missing { let _ = world.insert(e, (InheritedVisibility(true), ViewVisibility(true))); }
    let missing: Vec<Entity> = world
        .query::<(Entity, &Transform)>()
        .without::<&Visibility>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    for e in missing { let _ = world.insert(e, (Visibility::Inherited, InheritedVisibility(true), ViewVisibility(true))); }

    // Roots (no parent: nearly every entity) in one pass each.
    for (t, g) in world.query_mut::<(&Transform, &mut GlobalTransform)>().without::<&ChildOf>() { *g = GlobalTransform::from(*t); }
    for (v, iv, vv) in world.query_mut::<(&Visibility, &mut InheritedVisibility, &mut ViewVisibility)>().without::<&ChildOf>() {
        let visible = *v != Visibility::Hidden;
        iv.0 = visible;
        vv.0 = visible;
    }
    // Children (the few hierarchies: Ratchet's LOD groups, the UI) in depth order, parents before children.
    let mut parent_of: HashMap<Entity, Entity> = HashMap::new();
    for (e, c) in world.query::<(Entity, &ChildOf)>().iter() { parent_of.insert(e, c.0); }
    if parent_of.is_empty() { return; }
    let depth = |mut e: Entity| {
        let mut d = 0;
        while let Some(&p) = parent_of.get(&e) {
            e = p;
            d += 1;
            if d > 64 { break; }
        }
        d
    };
    let mut order: Vec<(usize, Entity)> = parent_of.keys().map(|&e| (depth(e), e)).collect();
    order.sort_by_key(|(d, e)| (*d, e.to_bits()));
    for (_, e) in order {
        let p = parent_of[&e];
        if let Ok(local) = world.get::<&Transform>(e).map(|t| *t) {
            let g = match world.get::<&GlobalTransform>(p).ok().map(|g| *g) {
                Some(pg) => pg.mul_transform(local),
                None => GlobalTransform::from(local),
            };
            if let Ok(mut gt) = world.get::<&mut GlobalTransform>(e) { *gt = g; }
        }
        if let Ok(v) = world.get::<&Visibility>(e).map(|v| *v) {
            let parent_visible = world.get::<&InheritedVisibility>(p).map_or(true, |i| i.0);
            let visible = match v {
                Visibility::Hidden => false,
                Visibility::Visible => true,
                Visibility::Inherited => parent_visible,
            };
            if let Ok(mut iv) = world.get::<&mut InheritedVisibility>(e) { iv.0 = visible; }
            if let Ok(mut vv) = world.get::<&mut ViewVisibility>(e) { vv.0 = visible; }
        }
    }
}

/// A resource insertion deferred until the borrows of a system end.
pub type ResInsert = Box<dyn FnOnce(&mut Resources)>;

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn children_inherit_transform_and_visibility() {
        let mut w = World::new();
        let p = w.spawn((Transform::from_xyz(1.0, 0.0, 0.0), Visibility::Hidden));
        let c = w.spawn((Transform::from_xyz(0.0, 2.0, 0.0), Visibility::Inherited, ChildOf(p)));
        propagate(&mut w);
        assert_eq!(w.get::<&GlobalTransform>(c).unwrap().translation(), Vec3::new(1.0, 2.0, 0.0));
        assert!(!w.get::<&InheritedVisibility>(c).unwrap().0);
        *w.get::<&mut Visibility>(p).unwrap() = Visibility::Inherited;
        propagate(&mut w);
        assert!(w.get::<&InheritedVisibility>(c).unwrap().0);
    }

    #[test]
    fn resources_borrow_at_run_time() {
        let mut r = Resources::default();
        r.insert(3u32);
        r.insert(String::from("a"));
        {
            let mut n = r.get_mut::<u32>();
            let s = r.get::<String>();
            *n += s.len() as u32;
        }
        assert_eq!(*r.get::<u32>(), 4);
        assert!(r.try_get::<f32>().is_none());
    }
}
