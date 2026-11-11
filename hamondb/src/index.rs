use crate::column::{CellId, ColumnId};
use crate::table::Table;
use hamondb::hamon_core::BucketKey;
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, RandomState};
use std::marker::PhantomData;

pub trait IndexMaintainer<T: Table>: Send + Sync {
    fn on_insert(&mut self, key: &T::PrimaryKey, row: &T, hasher: &RandomState) -> BucketKey;
    fn on_remove(&mut self, key: &T::PrimaryKey, row: &T, hasher: &RandomState) -> BucketKey;
    fn on_update(&mut self, key: &T::PrimaryKey, old: &T, new: &T, hasher: &RandomState) -> Option<(BucketKey, BucketKey)>;
    fn depends_on(&self) -> &[ColumnId];
    fn lookup(&self, value: &dyn Any) -> Vec<T::PrimaryKey>;
    fn id(&self) -> usize;
}

impl<T: Table, V: Eq + Hash + Send + Sync> IndexImpl<T, V> {
    pub fn new(columns: &'static [ColumnId], extract: fn(&T) -> V, id: usize) -> Self {
        Self {
            map: HashMap::new(),
            extract,
            handle: Index {
                id,
                columns,
                _phantom: PhantomData,
            },
        }
    }
}

impl<T: Table, V: Send + Sync> IndexMaintainer<T> for IndexImpl<T, V>
where
    V: Eq + Hash + 'static,
{
    fn on_insert(&mut self, key: &T::PrimaryKey, new: &T, hasher: &RandomState) -> BucketKey {
        let value = (self.extract)(new);
        let bucket_key = BucketKey::new(&self.handle, &value, hasher)
        self.map.entry(value).or_default().push(key.clone());
        bucket_key
    }

    fn on_remove(&mut self, key: &T::PrimaryKey, old: &T, hasher: &RandomState) -> BucketKey {
        let value = (self.extract)(old);
        let bucket_key = BucketKey::new(&self.handle, &value, hasher);
        let found = self.map.get_mut(&value);
        debug_assert!(found.is_some(), "on_remove: value not found in index");
        if let Some(keys) = found {
            keys.retain(|k| k != key);
            if keys.is_empty() {
                self.map.remove(&value);
            }
        }
        bucket_key
    }

    fn on_update(&mut self, key: &T::PrimaryKey, old_row: &T, new_row: &T, hasher: &RandomState) -> Option<(BucketKey, BucketKey)> {
        let old_v = (self.extract)(old_row);
        let old_bucket_key = BucketKey::new(&self.handle, &old_v, hasher);
        let new_v = (self.extract)(new_row);
        let new_bucket_key = BucketKey::new(&self.handle, &new_v, hasher);
        if old_v != new_v {
            if let Some(keys) = self.map.get_mut(&old_v) {
                keys.retain(|k| k != key);
                if keys.is_empty() {
                    self.map.remove(&old_v);
                }
            }
            self.map.entry(new_v).or_default().push(key.clone());
            Some((old_bucket_key, new_bucket_key))
        } else {
            None
        }
    }

    fn depends_on(&self) -> &[ColumnId] {
        &self.handle.columns
    }

    fn lookup(&self, value: &dyn Any) -> Vec<T::PrimaryKey> {
        let v = value.downcast_ref::<V>().expect("type mismatch");
        self.map.get(v).cloned().unwrap_or_default()
    }
    fn id(&self) -> usize {
        self.handle.id
    }
}

#[derive(Debug, Copy, Clone)]
pub struct Index<T: Table, V> {
    pub(super) id: usize,
    pub(super) columns: &'static [ColumnId],
    _phantom: PhantomData<(T, V)>,
}

impl<T: Table, V> Index<T, V> {
    pub const fn new(columns: &'static [ColumnId], id: usize) -> Index<T, V> {
        Index {
            id,
            columns,
            _phantom: PhantomData,
        }
    }
}

pub struct IndexImpl<T: Table, V: Eq + Hash> {
    map: HashMap<V, Vec<T::PrimaryKey>>,
    extract: fn(&T) -> V,
    handle: Index<T, V>,
}
