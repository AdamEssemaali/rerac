//! Asset stores with reference-counted handles (the part of Bevy's `Assets<T>` / `Handle<T>` rc-engine uses).
//!
//! A [`Handle`] is a shared id: cloning it shares the asset, and an asset whose handles are all dropped is freed at the
//! next [`Assets::collect_garbage`] (once per frame, as Bevy frees unreferenced assets). [`Assets::get_mut`] marks the
//! asset changed, so the renderer re-uploads it (Bevy's `AssetEvent::Modified`), exactly when rc-engine relied on it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// An asset's id: unique across every store for the process lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssetId(pub u64);

/// A strong reference to an asset in an [`Assets<T>`].
pub struct Handle<T> {
    id: Arc<AssetId>,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Handle<T> {
    pub fn id(&self) -> AssetId { *self.id }
}

impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self { Handle { id: self.id.clone(), _marker: PhantomData } }
}

impl<T> PartialEq for Handle<T> {
    fn eq(&self, o: &Self) -> bool { *self.id == *o.id }
}
impl<T> Eq for Handle<T> {}

impl<T> Hash for Handle<T> {
    fn hash<H: Hasher>(&self, h: &mut H) { self.id.hash(h) }
}

impl<T> PartialOrd for Handle<T> {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> { Some(self.cmp(o)) }
}
impl<T> Ord for Handle<T> {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering { self.id.cmp(&o.id) }
}

impl<T> fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "Handle<{}>({})", std::any::type_name::<T>(), self.id.0) }
}

impl<T> From<&Handle<T>> for AssetId {
    fn from(h: &Handle<T>) -> Self { h.id() }
}

struct Entry<T> {
    value: T,
    /// The store's own reference: the asset is unreferenced when it is the only one left.
    handle: Arc<AssetId>,
}

/// A store of assets of one type.
pub struct Assets<T> {
    entries: BTreeMap<AssetId, Entry<T>>,
    /// Added or changed since the renderer last took them ([`Assets::take_changed`]).
    changed: BTreeSet<AssetId>,
    /// Freed since the renderer last took them.
    removed: Vec<AssetId>,
}

impl<T> Default for Assets<T> {
    fn default() -> Self { Assets { entries: BTreeMap::new(), changed: BTreeSet::new(), removed: Vec::new() } }
}

impl<T> Assets<T> {
    pub fn add(&mut self, value: impl Into<T>) -> Handle<T> {
        let id = Arc::new(AssetId(NEXT_ID.fetch_add(1, Ordering::Relaxed)));
        self.entries.insert(*id, Entry { value: value.into(), handle: id.clone() });
        self.changed.insert(*id);
        Handle { id, _marker: PhantomData }
    }

    pub fn get(&self, id: impl Into<AssetId>) -> Option<&T> { self.entries.get(&id.into()).map(|e| &e.value) }

    /// The asset, marked changed (the renderer re-uploads it).
    pub fn get_mut(&mut self, id: impl Into<AssetId>) -> Option<&mut T> {
        let id = id.into();
        let e = self.entries.get_mut(&id)?;
        self.changed.insert(id);
        Some(&mut e.value)
    }

    /// The asset without marking it changed (for reads that need `&mut` access to nothing GPU-visible).
    pub fn get_mut_untracked(&mut self, id: impl Into<AssetId>) -> Option<&mut T> { self.entries.get_mut(&id.into()).map(|e| &mut e.value) }

    pub fn contains(&self, id: impl Into<AssetId>) -> bool { self.entries.contains_key(&id.into()) }

    pub fn remove(&mut self, id: impl Into<AssetId>) -> Option<T> {
        let id = id.into();
        let e = self.entries.remove(&id)?;
        self.changed.remove(&id);
        self.removed.push(id);
        Some(e.value)
    }

    pub fn iter(&self) -> impl Iterator<Item = (AssetId, &T)> { self.entries.iter().map(|(k, e)| (*k, &e.value)) }

    /// Every asset, each marked changed.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (AssetId, &mut T)> {
        self.changed.extend(self.entries.keys().copied());
        self.entries.iter_mut().map(|(k, e)| (*k, &mut e.value))
    }

    pub fn len(&self) -> usize { self.entries.len() }

    pub fn is_empty(&self) -> bool { self.entries.is_empty() }

    /// Frees every asset no handle refers to any more.
    pub fn collect_garbage(&mut self) {
        let dead: Vec<AssetId> = self.entries.iter().filter(|(_, e)| Arc::strong_count(&e.handle) == 1).map(|(k, _)| *k).collect();
        for id in dead { self.remove(id); }
    }

    /// The ids added or changed, and those freed, since the last call (the renderer's sync).
    pub fn take_changed(&mut self) -> (Vec<AssetId>, Vec<AssetId>) {
        (std::mem::take(&mut self.changed).into_iter().collect(), std::mem::take(&mut self.removed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_keep_assets_alive() {
        let mut a: Assets<u32> = Assets::default();
        let h = a.add(5u32);
        let h2 = h.clone();
        a.collect_garbage();
        assert_eq!(a.get(&h), Some(&5));
        drop(h);
        a.collect_garbage();
        assert_eq!(a.get(&h2), Some(&5));
        let id = h2.id();
        drop(h2);
        a.collect_garbage();
        assert!(a.get(id).is_none());
        let (_, removed) = a.take_changed();
        assert_eq!(removed, vec![id]);
    }

    #[test]
    fn get_mut_marks_changed() {
        let mut a: Assets<u32> = Assets::default();
        let h = a.add(1u32);
        let _ = a.take_changed();
        assert!(a.get(&h).is_some());
        assert_eq!(a.take_changed().0, Vec::<AssetId>::new());
        *a.get_mut(&h).unwrap() = 2;
        assert_eq!(a.take_changed().0, vec![h.id()]);
    }
}
