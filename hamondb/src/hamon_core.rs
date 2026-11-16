use crate::column::{CellId, Column, ColumnId};
use crate::index::Index;
use crate::recompute_queue::RecomputeQueue;
use crate::resource::Resource;
use crate::slot::SlotRow;
use crate::store::{AnyTableStore, ChangedColumnFilter, InsertResult, ReplaceOutcome, BucketMove, TableStore};
use crate::table::Table;
use crate::update::IntoUpdate;
use crate::Slot;
use arc_swap::ArcSwap;
use lock_api::{ArcRwLockReadGuard, RwLockWriteGuard};
use parking_lot::{Condvar, Mutex, RawRwLock, RwLock};
use std::any::{type_name, Any, TypeId};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hash, Hasher, RandomState};
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug, thiserror::Error, Eq, PartialEq)]
pub enum HamonError {
    #[error("duplicate key")]
    DuplicateKey,
    #[error("key not found")]
    KeyNotFound,
    #[error("computed column update")]
    ComputedColumnUpdate,
    #[error("pk column update")]
    PKColumnUpdate,
    #[error("table not registered")]
    TableNotRegistered,
    #[error(" computed column dependency error")]
    ComputedColumnDependencyError,
    #[error("concurrent modification")]
    ConcurrentModification,
    #[error("Dependency cycle of computed column")]
    DependencyCycle(Vec<ColumnId>),
}

#[derive(Clone)]
pub struct ShutdownToken {
    inner: Arc<ShutdownInner>,
}

pub struct ShutdownInner {
    triggered: AtomicBool,
    lock: Mutex<()>,
    cvar: Condvar,
}

impl ShutdownToken {
    pub(super) fn new() -> Self {
        Self {
            inner: Arc::new(ShutdownInner {
                triggered: AtomicBool::new(false),
                lock: Mutex::new(()),
                cvar: Condvar::new(),
            }),
        }
    }

    pub(super) fn trigger(&self) {
        self.inner.triggered.store(true, Ordering::SeqCst);
        let _g = self.inner.lock.lock();
        self.inner.cvar.notify_all();
    }

    pub fn is_triggered(&self) -> bool {
        self.inner.triggered.load(Ordering::SeqCst)
    }

    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        let mut guard = self.inner.lock.lock();
        if self.is_triggered() {
            return true;
        }
        let res = self.inner.cvar.wait_for(&mut guard, timeout);
        self.is_triggered()
    }
}

#[derive(Clone, Eq, PartialEq, Hash, Copy)]
pub(super) struct BucketKey {
    pub(super) table: TypeId,
    pub(super) index: usize,
    pub(super) bucket: u64,
}

impl BucketKey {
    pub(super) fn new<T: Table, V: Hash>(
        index: &Index<T, V>,
        value: &V,
        hasher: &RandomState,
    ) -> Self {
        let mut h = hasher.build_hasher();
        value.hash(&mut h);
        Self {
            table: TypeId::of::<T>(),
            index: index.id,
            bucket: h.finish(),
        }
    }
}
/// Dependency graph between cells (data edges) and between cells and index
/// buckets (bucket-watcher edges).
///
/// All methods take `&self`. Internally backed by `DashMap`, so operations are
/// safely concurrent without an outer lock. The maps are kept pairwise
/// consistent *eventually*; see module notes on `register` vs `remove` races.
///
/// Invariant (steady-state, no in-flight mutations): `D ∈ uses[C] ⇔ C ∈ used_by[D]`.
#[derive(Default)]
pub(super) struct DepGraph {
    uses: dashmap::DashMap<CellId, HashSet<CellId>>,
    used_by: dashmap::DashMap<CellId, HashSet<CellId>>,
    used_by_index: dashmap::DashMap<BucketKey, HashSet<CellId>>,
    uses_index: dashmap::DashMap<CellId, HashSet<BucketKey>>,
}

impl DepGraph {
    pub(super) fn index_register(&self, cell_id: CellId, bucket_keys: HashSet<BucketKey>) {
        // Swap in the new forward-edge set for `cell_id`, then diff against the
        // old set so back-edges on `used_by_index` are only touched where they
        // changed. Reduces shard-lock contention compared to blind re-insert.
        let old = self
            .uses_index
            .insert(cell_id, bucket_keys.clone())
            .unwrap_or_default();
        for stale in old.difference(&bucket_keys) {
            if let Some(mut set) = self.used_by_index.get_mut(stale) {
                set.remove(&cell_id);
            }
        }
        for fresh in bucket_keys.difference(&old) {
            self.used_by_index
                .entry(*fresh)
                .or_default()
                .insert(cell_id);
        }
    }

    #[allow(dead_code)]
    pub(super) fn index_clear_cells(&self, cell: CellId) {
        if let Some((_, buckets)) = self.uses_index.remove(&cell) {
            for b in &buckets {
                let drop_entry = self
                    .used_by_index
                    .get_mut(b)
                    .map(|mut s| {
                        s.remove(&cell);
                        s.is_empty()
                    })
                    .unwrap_or(false);
                if drop_entry {
                    // best-effort cleanup; benign if another thread repopulated the bucket
                    self.used_by_index
                        .remove_if(b, |_, s| s.is_empty());
                }
            }
        }
    }

    pub(super) fn register(&self, computed: CellId, reads: HashSet<CellId>) {
        let old = self.uses.insert(computed, reads.clone()).unwrap_or_default();
        for stale in old.difference(&reads) {
            if let Some(mut set) = self.used_by.get_mut(stale) {
                set.remove(&computed);
            }
        }
        for fresh in reads.difference(&old) {
            self.used_by.entry(*fresh).or_default().insert(computed);
        }
    }

    /// Remove `cell` from both directions. Returns its dependents (cells that
    /// read `cell`) so the caller can enqueue them for recomputation.
    ///
    /// Not atomic across the two maps. A concurrent `register(D, {cell})` may
    /// observe this half-removed state; the leftover edge gets cleaned on
    /// D's next `register` call via the forward-edge diff.
    pub(super) fn remove(&self, cell: &CellId) -> HashSet<CellId> {
        if let Some((_, reads)) = self.uses.remove(cell) {
            for read in &reads {
                if let Some(mut set) = self.used_by.get_mut(read) {
                    set.remove(cell);
                }
            }
        }
        if let Some((_, dependents)) = self.used_by.remove(cell) {
            for dep in &dependents {
                if let Some(mut set) = self.uses.get_mut(dep) {
                    set.remove(cell);
                }
            }
            dependents
        } else {
            HashSet::new()
        }
    }

    pub(super) fn dependents(&self, cell: &CellId) -> HashSet<CellId> {
        self.used_by
            .get(cell)
            .map(|r| r.clone())
            .unwrap_or_default()
    }

    pub(super) fn watchers_of_bucket(&self, key: &BucketKey) -> HashSet<CellId> {
        self.used_by_index
            .get(key)
            .map(|r| r.clone())
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(super) fn uses_of(&self, cell: &CellId) -> Option<HashSet<CellId>> {
        self.uses.get(cell).map(|r| r.clone())
    }
}

pub enum InsertOutcome<T: Table + Send + Sync> {
    Inserted,
    Duplicate(T, T::PrimaryKey, Arc<RwLock<T>>),
}

pub struct TableRegistry {
    pub(super) tables: HashMap<TypeId, Box<dyn AnyTableStore>>,
    pub(super) deps: DepGraph,
    pub(super) join_handles: Mutex<Vec<std::thread::JoinHandle<()>>>,
    pub(super) shutdown_token: ShutdownToken,
    pub(super) resources: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    pub(super) hasher: RandomState,
}

pub struct Hamon(pub(super) Arc<HamonInner>);
pub struct HamonShutdown(pub(super) Arc<HamonInner>);

impl Drop for HamonShutdown {
    fn drop(&mut self) {
        self.0.register.shutdown_token.trigger();
        let mut handles = self.0.register.join_handles.lock();
        for h in handles.drain(..) {
            let _ = h.join();
        }
    }
}

impl Hamon {
    pub fn insert<T: Table + 'static>(&self, row: T) -> Result<(), HamonError> {
        tracing::debug!("op:insert, table:{}", type_name::<T>());
        self.0.insert(row)
    }
    pub fn upsert<T: Table + 'static>(&self, row: T) -> Result<(), HamonError> {
        tracing::debug!("op:upsert, table:{}", type_name::<T>());
        self.0.upsert(row)
    }
    pub fn replace<T: Table + 'static>(&self, mut row: T) -> Result<(), HamonError> {
        tracing::debug!("op:replace, table:{}", type_name::<T>());
        self.0.replace(row)
    }
    pub fn update<T: Table + 'static>(
        &self,
        key: &T::PrimaryKey,
        update: impl IntoUpdate<T>,
    ) -> Result<(), HamonError> {
        tracing::debug!("op:update, table:{}", type_name::<T>());
        self.0.update(key, update)
    }
    pub fn slot<M, V>(&self, slot: Slot<M, V>) -> V
    where
        M: Send + Sync + 'static,
        V: Clone + PartialEq + Send + Sync + 'static,
    {
        self.0.get_slot(slot)
    }
    pub fn set_slot<M, V>(&self, slot: Slot<M, V>, value: V) -> Result<(), HamonError>
    where
        M: Send + Sync + 'static,
        V: Clone + PartialEq + Send + Sync + 'static,
    {
        tracing::debug!("op:set_slot, table:{}", type_name::<SlotRow<M, V>>());
        self.0.set_slot(slot, value)
    }
    pub fn lookup_index<T: Table + 'static, V: 'static>(
        &self,
        index: Index<T, V>,
        value: &V,
    ) -> Vec<T> {
        self.0.lookup_index(index, value)
    }
    pub fn len<T: Table + 'static>(&self) -> usize {
        self.0.len::<T>()
    }
    pub fn for_each<T: Table + 'static>(&self, f: impl FnMut(&T)) {
        tracing::debug!("op:for_each, table:{}", type_name::<T>());
        self.0.for_each(f)
    }
    pub fn get<T: Table + 'static>(&self, key: &T::PrimaryKey) -> Result<T, HamonError> {
        self.0.get(key)
    }
    pub fn delete<T: Table + 'static>(&self, pk: &T::PrimaryKey) -> Result<(), HamonError> {
        tracing::debug!("op:delete, table:{}", type_name::<T>());
        self.0.delete::<T>(pk)
    }

    pub fn resource<M, V>(&self, _resource: Resource<M, V>) -> Arc<V>
    where
        M: Send + Sync + 'static,
        V: Send + Sync + 'static,
    {
        self.0.resource(_resource)
    }

    pub fn set_resource<M, V>(&self, _resource: Resource<M, V>, value: V)
    where
        M: Send + Sync + 'static,
        V: Send + Sync + 'static,
    {
        self.0.set_resource(_resource, value)
    }
}

pub(super) struct HamonInner {
    pub(super) register: Arc<TableRegistry>,
    pub(super) next_row_id: AtomicU64,
    pub(super) queue: Arc<RecomputeQueue>,
}

impl HamonInner {
    pub fn shutdown(&self) {
        self.register.shutdown_token.trigger();
        let mut handles = self.register.join_handles.lock();
        for h in handles.drain(..) {
            let _ = h.join();
        }
    }

    fn insert_impl<T: Table + 'static>(&self, row: T) -> InsertOutcome<T> {
        let row_id = self.next_row_id.fetch_add(1, Ordering::Relaxed);
        let table = self.table::<T>();
        let ctx = Ctx::new(
            &self.register.tables,
            &self.register.resources,
            &self.register.hasher,
        );
        let mut new = row.clone();
        let (deps, bucket_deps) = table.compute_columns(&mut new, &ctx, row_id, None);
        match table.insert(new, row_id, &self.register.hasher) {
            InsertResult::Inserted(guard, bucket_keys) => {
                let dep_graph = &self.register.deps;
                for (computed_cell, read_cells) in deps {
                    dep_graph.register(computed_cell, read_cells)
                }
                for (computed_cell, read_bucket) in bucket_deps {
                    dep_graph.index_register(computed_cell, read_bucket)
                }
                for key in &bucket_keys {
                    for cell in dep_graph.watchers_of_bucket(key) {
                        let _ = self.queue.enqueue(cell);
                    }
                }
                let new_copy = guard.clone();
                drop(guard);
                table.fire_lister(&None, &Some(new_copy));
                InsertOutcome::Inserted
            }
            InsertResult::Duplicate(row, key, arc) => InsertOutcome::Duplicate(row, key, arc),
        }
    }

    pub fn insert<T: Table + 'static>(&self, row: T) -> Result<(), HamonError> {
        match self.insert_impl(row) {
            InsertOutcome::Inserted => Ok(()),
            InsertOutcome::Duplicate(_, _, _) => Err(HamonError::DuplicateKey),
        }
    }

    pub fn upsert<T: Table + 'static>(&self, row: T) -> Result<(), HamonError> {
        let table = self.table::<T>();
        match table.get_row_from_row(&row) {
            None => match self.insert_impl(row) {
                InsertOutcome::Inserted => Ok(()),
                InsertOutcome::Duplicate(row, key, arc) => {
                    let guard = arc.write();
                    self.replace_impl(table, &key, guard, row)
                }
            },
            Some((key, arc)) => {
                let guard = arc.write();
                match self.replace_impl(table, &key, guard, row) {
                    // they were a row, we tried to update, but it got deleted in the meantime
                    // let the caller manage it
                    Err(HamonError::KeyNotFound) => Err(HamonError::ConcurrentModification),
                    other => other,
                }
            }
        }
    }

    pub fn replace<T: Table + 'static>(&self, mut row: T) -> Result<(), HamonError> {
        let table = self.table::<T>();

        match table.get_row_from_row(&row) {
            Some((key, arc)) => {
                let guard = arc.write();
                self.replace_impl(table, &key, guard, row)
            }
            None => Err(HamonError::KeyNotFound),
        }
    }

    fn replace_impl<T: Table + 'static>(
        &self,
        table: &TableStore<T>,
        key: &T::PrimaryKey,
        guard: RwLockWriteGuard<RawRwLock, T>,
        mut row: T,
    ) -> Result<(), HamonError> {
        {
            let row_id = *table
                .pk_to_row_id
                .get(key)
                .expect("pk_to_row_id out of sync");
            let changed_regular_cells: Vec<CellId> = table
                .changed_columns(&guard, &row, ChangedColumnFilter::RegularOnly)
                .iter()
                .map(|colId| CellId {
                    table: TypeId::of::<T>(),
                    row: row_id,
                    column: *colId,
                })
                .collect();
            if changed_regular_cells.is_empty() {
                return Ok(());
            }
            // Partition dependents into same-row (feed back into this fixpoint)
            // vs cross-row (enqueue after replace).
            let dependent_cells: HashSet<CellId> = changed_regular_cells
                .iter()
                .flat_map(|cell| self.register.deps.dependents(cell))
                .collect();
            let (same_row, mut other_rows): (HashSet<CellId>, HashSet<CellId>) = dependent_cells
                .iter()
                .partition(|cell| cell.table == TypeId::of::<T>() && cell.row == row_id);
            let initial: HashSet<ColumnId> = same_row.iter().map(|c| c.column).collect();
            let ctx = Ctx::new(
                &self.register.tables,
                &self.register.resources,
                &self.register.hasher,
            );
            let cross_row_additions =
                table.fixpoint_compute(row_id, initial, &mut row, &*guard, &ctx, &self.register.deps);
            other_rows.extend(cross_row_additions);
            let ReplaceOutcome { old_row, new_row, bucket_moves } =
                table.replace(guard, row, &self.register.hasher);
            for mv in &bucket_moves {
                for cell in self.register.deps.watchers_of_bucket(&mv.from) {
                    let _ = self.queue.enqueue(cell);
                }
                for cell in self.register.deps.watchers_of_bucket(&mv.to) {
                    let _ = self.queue.enqueue(cell);
                }
            }
            for cell in other_rows.iter() {
                let _ = self.queue.enqueue(*cell);
            }
            table.fire_lister(&Some(old_row), &Some(new_row));
            Ok(())
        }
    }

    pub fn update<T: Table + 'static>(
        &self,
        key: &T::PrimaryKey,
        update: impl IntoUpdate<T>,
    ) -> Result<(), HamonError> {
        let update = update.into_update();
        let table = self.table::<T>();
        let ids = update.column_ids();
        if table.any_computed(&ids) {
            return Err(HamonError::ComputedColumnUpdate);
        }
        if table.any_primary_key(&ids) {
            return Err(HamonError::PKColumnUpdate);
        }
        match table.get_row_from_pk(key) {
            None => Err(HamonError::KeyNotFound),
            Some(arc) => {
                let guard = arc.write();
                let mut draft = guard.clone();
                table.apply_update(&mut draft, update);
                self.replace_impl(table, key, guard, draft)
            }
        }
    }

    pub fn get_slot<M, V>(&self, slot: Slot<M, V>) -> V
    where
        M: Send + Sync + 'static,
        V: Clone + PartialEq + Send + Sync + 'static,
    {
        self.table::<SlotRow<M, V>>()
            .get_row_from_pk(&())
            .unwrap_or_else(|| panic!("slot {} row missing", slot.name))
            .read()
            .value
            .clone()
    }

    pub fn set_slot<M, V>(&self, slot: Slot<M, V>, value: V) -> Result<(), HamonError>
    where
        M: Send + Sync + 'static,
        V: Clone + PartialEq + Send + Sync + 'static,
    {
        self.update::<SlotRow<M, V>>(&(), (slot.column, value))
    }
    pub fn get<T: Table + 'static>(&self, key: &T::PrimaryKey) -> Result<T, HamonError> {
        let table = self.table::<T>();
        match table.rows.get(&key) {
            None => Err(HamonError::KeyNotFound),
            Some(cell) => Ok(cell.read().clone()),
        }
    }

    pub fn resource<M, V>(&self, _resource: Resource<M, V>) -> Arc<V>
    where
        M: Send + Sync + 'static,
        V: Send + Sync + 'static,
    {
        let any = self
            .register
            .resources
            .get(&TypeId::of::<M>())
            .expect("Resource not registered");
        any.downcast_ref::<ArcSwap<V>>()
            .expect("Resource type mismatch")
            .load_full()
    }

    pub fn set_resource<M, V>(&self, resource: Resource<M, V>, value: V)
    where
        M: Send + Sync + 'static,
        V: Send + Sync + 'static,
    {
        let any = self
            .register
            .resources
            .get(&TypeId::of::<M>())
            .unwrap_or_else(|| panic!("Resource not registered: {}", resource.name));
        let swap = any
            .downcast_ref::<ArcSwap<V>>()
            .unwrap_or_else(|| panic!("Resource type mismatch {}", resource.name));
        swap.store(Arc::new(value));
    }

    pub fn lookup_index<T: Table + 'static, V: 'static>(
        &self,
        index: Index<T, V>,
        value: &V,
    ) -> Vec<T> {
        let table = self.table::<T>();
        table
            .lookup_index(&index, value)
            .into_iter()
            .map(|(arc, _row_id)| arc.read().clone())
            .collect()
    }

    pub fn len<T: Table + 'static>(&self) -> usize {
        let table = self.table::<T>();
        table.len()
    }

    pub fn for_each<T: Table + 'static>(&self, f: impl FnMut(&T)) {
        let table = self.table::<T>();
        table.for_each(f);
    }

    pub fn delete<T: Table + 'static>(&self, pk: &T::PrimaryKey) -> Result<(), HamonError> {
        {
            let table = self.table::<T>();
            match table.get_row_from_pk(pk) {
                Some(arc) => {
                    let row_id = *table
                        .pk_to_row_id
                        .get(pk)
                        .expect("pk_to_row_id out of sync");
                    let guard = arc.write();
                    let row = guard.clone();
                    let (_, bucket_keys) = table.delete(guard, &self.register.hasher);
                    let dep_graph = &self.register.deps;
                    for key in &bucket_keys {
                        for cell in dep_graph.watchers_of_bucket(key) {
                            let _ = self.queue.enqueue(cell);
                        }
                    }
                    for cell in table.cell_ids_for_row(row_id) {
                        for to_recompute in dep_graph.remove(&cell) {
                            let _ = self.queue.enqueue(to_recompute);
                        }
                    }
                    table.fire_lister(&Some(row), &None);
                    Ok(())
                }
                None => Err(HamonError::KeyNotFound),
            }
        }
    }

    fn table<T: Table + 'static>(&self) -> &TableStore<T> {
        self.register
            .tables
            .get(&TypeId::of::<T>())
            .expect("Table not registerd")
            .as_any()
            .downcast_ref::<TableStore<T>>()
            .expect("Table type mismatch")
    }
}

pub(super) struct ComputeContext<'a> {
    pub(super) table: TypeId,
    pub(super) current_column: ColumnId,
    pub(super) current_column_name: &'static str,
    pub(super) declared_deps: &'a HashSet<ColumnId>,
    pub(super) all_computed_ids: &'a HashSet<ColumnId>,
}

pub struct Ctx<'a> {
    tables: &'a HashMap<TypeId, Box<dyn AnyTableStore>>,
    resources: &'a HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    pub(super) tracker: RefCell<HashSet<CellId>>,
    pub(super) bucket_tracker: RefCell<HashSet<BucketKey>>,
    held_locks: RefCell<Vec<Box<dyn Any>>>,
    pub(super) hasher: &'a RandomState,
}

impl<'a> Ctx<'a> {
    pub fn lookup<T: Table + 'static>(&self, key: &T::PrimaryKey) -> Option<Row<T>> {
        let table = self.get_table::<T>();
        let arc = table.get_row_from_pk(key)?;
        let guard_for_row = arc.read_arc(); // first read lock — goes into Row
        // Second read lock from the same thread: must be recursive so a writer
        // queued between the two acquisitions does not deadlock us under
        // parking_lot's writer-biased fairness.
        let guard_for_hold = arc.read_arc_recursive();
        self.held_locks.borrow_mut().push(Box::new(guard_for_hold));
        table
            .pk_to_row_id
            .get(key)
            .map(|row_id| Row::new_for_lookup(RowInner::Guard(guard_for_row), self, *row_id))
    }

    fn get_table<T: Table + 'static>(&self) -> &TableStore<T> {
        self.tables
            .get(&TypeId::of::<T>())
            .expect("table not registered")
            .as_any()
            .downcast_ref::<TableStore<T>>()
            .expect("table type mismatch")
    }

    pub fn lookup_index<T: Table + 'static, V: Hash + 'static>(
        &self,
        index: &Index<T, V>,
        value: &V,
    ) -> Vec<Row<T>> {
        let table = self.get_table::<T>();
        let result = table.lookup_index(index, value);
        self.bucket_tracker
            .borrow_mut()
            .insert(BucketKey::new(index, value, self.hasher));
        result
            .iter()
            .map(|(guard, row_id)| {
                let guard_for_row = guard.read_arc();
                // Recursive to avoid the same self-deadlock documented in `lookup`.
                let guard_for_hold = guard.read_arc_recursive();
                self.held_locks.borrow_mut().push(Box::new(guard_for_hold));
                Row::new_for_lookup(RowInner::Guard(guard_for_row), self, *row_id)
            })
            .collect()
    }

    pub fn resource<M, V>(&self, resource: Resource<M, V>) -> Arc<V>
    where
        M: Send + Sync + 'static,
        V: Send + Sync + 'static,
    {
        let any = self
            .resources
            .get(&TypeId::of::<M>())
            .unwrap_or_else(|| panic!("resource not registered: {}", resource.name));
        any.downcast_ref::<ArcSwap<V>>()
            .unwrap_or_else(|| panic!("resource type mismatch: {}", resource.name))
            .load_full()
    }

    pub fn slot<M, V>(&self, _slot: Slot<M, V>) -> V
    where
        M: Send + Sync + 'static,
        V: Clone + PartialEq + Send + Sync + 'static,
    {
        self.tracker.borrow_mut().insert(CellId {
            table: TypeId::of::<SlotRow<M, V>>(),
            row: 0,
            column: _slot.column.id,
        });

        self.tables
            .get(&TypeId::of::<SlotRow<M, V>>())
            .expect("slot not registered")
            .as_any()
            .downcast_ref::<TableStore<SlotRow<M, V>>>()
            .expect("slot store type mismatch")
            .get_row_from_pk(&())
            .expect("slot row missing")
            .read()
            .value
            .clone()
    }

    pub(super) fn new(
        tables: &'a HashMap<TypeId, Box<dyn AnyTableStore>>,
        resources: &'a HashMap<TypeId, Box<dyn Any + Send + Sync>>,
        hasher: &'a RandomState,
    ) -> Self {
        Ctx {
            tables,
            tracker: RefCell::new(HashSet::new()),
            bucket_tracker: RefCell::new(HashSet::new()),
            held_locks: RefCell::new(Vec::new()),
            resources,
            hasher,
        }
    }
}

pub(super) enum RowInner<'a, T> {
    Ref(&'a T),
    Guard(ArcRwLockReadGuard<RawRwLock, T>),
}

impl<'a, T> Deref for RowInner<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        match self {
            RowInner::Ref(r) => r,
            RowInner::Guard(g) => &*g,
        }
    }
}

pub struct Row<'a, T: Table + Send + Sync + 'static> {
    inner: RowInner<'a, T>,
    row_id: u64,
    ctx: &'a Ctx<'a>,
    compute_context: Option<ComputeContext<'a>>,
}

impl<'a, T: Table + Send + Sync> Row<'a, T> {
    pub(super) fn new_for_compute(
        inner: RowInner<'a, T>,
        row_id: u64,
        ctx: &'a Ctx,
        cc: ComputeContext<'a>,
    ) -> Self {
        Self {
            inner,
            row_id,
            ctx,
            compute_context: Some(cc),
        }
    }
    pub(super) fn new_for_lookup(inner: RowInner<'a, T>, ctx: &'a Ctx, row_id: u64) -> Self {
        Self {
            inner,
            row_id,
            ctx,
            compute_context: None,
        }
    }

    pub fn get<V: 'static>(&self, col: Column<T, V>) -> V {
        self.ctx.tracker.borrow_mut().insert(CellId {
            table: TypeId::of::<T>(),
            row: self.row_id,
            column: col.id,
        });
        if let Some(cc) = &self.compute_context {
            if cc.all_computed_ids.contains(&col.id) && !cc.declared_deps.contains(&col.id) {
                if cc.current_column == col.id {
                    panic!(
                        "computed column `{}` cannot read itself",
                        cc.current_column_name
                    );
                } else {
                    panic!(
                        "computed column `{}` read undeclared computed column `{}` — add `{}` to the dependencies of `{}`",
                        cc.current_column_name, col.name, col.name, cc.current_column_name
                    );
                }
            }
        }
        (col.get)(&self.inner)
    }
}

#[cfg(test)]
mod test {
    use crate::builder::HamonBuilder;
    use crate::column::Column;
    use crate::table::Table;

    #[derive(Clone)]
    struct SimpleTable {
        id: u64,
        val: String,
    }

    impl Table for SimpleTable {
        type PrimaryKey = u64;
    }

    const COL_ID: Column<SimpleTable, u64> = Column {
        id: 0,
        name: "SimpleTable::COL_ID",
        get: |r| r.id,
        set: |r, v| r.id = v,
    };
    const COL_VAL: Column<SimpleTable, String> = Column {
        id: 1,
        name: "SimpleTable::COL_VAL",
        get: |r| r.val.clone(),
        set: |r, v| r.val = v,
    };
    #[test]
    fn test() {
        let mut builder = HamonBuilder::new();
        builder
            .register::<SimpleTable, Column<SimpleTable, u64>>(COL_ID)
            .done();
        let (db, shutdown) = builder.build();
        let result = db.insert(SimpleTable {
            id: 1,
            val: "foo".to_string(),
        });
        assert!(result.is_ok());
    }
}

// Concurrency stress tests for DepGraph.
//
// These exercise the lock-free (DashMap-backed) implementation under
// contention — scenarios the single-RwLock version could never trigger.
// The goal is to confirm:
//   1. No corruption (panics / bad unwraps / lost dependents).
//   2. Self-healing: transient dangling back-edges from register/remove races
//      get cleaned on the next `register` call via forward-edge diffing.
//   3. Convergence of `dependents()` under concurrent writers to a hot cell.
#[cfg(test)]
mod dep_graph_stress {
    use super::{BucketKey, DepGraph};
    use crate::column::{CellId, ColumnId};
    use std::any::TypeId;
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::thread;

    struct Marker;
    fn cell(row: u64, col: ColumnId) -> CellId {
        CellId {
            table: TypeId::of::<Marker>(),
            row,
            column: col,
        }
    }
    fn bucket(idx: usize, b: u64) -> BucketKey {
        BucketKey {
            table: TypeId::of::<Marker>(),
            index: idx,
            bucket: b,
        }
    }

    #[test]
    fn many_cells_register_same_hot_read() {
        // N cells all register as dependents of one hot cell X.
        // After join, `dependents(X)` must contain exactly all N cells.
        // Exercises the DashMap entry-lock serialization on `used_by[X]`.
        const N: u32 = 500;
        let graph = Arc::new(DepGraph::default());
        let x = cell(0, 0);
        let handles: Vec<_> = (0..N)
            .map(|i| {
                let g = graph.clone();
                thread::spawn(move || {
                    let c = cell(1, (i + 1) as ColumnId);
                    g.register(c, HashSet::from([x]));
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let deps = graph.dependents(&x);
        assert_eq!(deps.len(), N as usize);
        for i in 0..N {
            assert!(deps.contains(&cell(1, (i + 1) as ColumnId)));
        }
    }

    #[test]
    fn concurrent_register_updates_same_cell_converge() {
        // One cell C re-registers its reads from many threads using the same
        // target set; the final state is well-defined even if intermediate
        // interleavings are racy. All threads converge on `uses[C] = {X}`.
        const N: u32 = 300;
        let graph = Arc::new(DepGraph::default());
        let c = cell(5, 100);
        let x = cell(0, 0);
        let handles: Vec<_> = (0..N)
            .map(|_| {
                let g = graph.clone();
                thread::spawn(move || {
                    g.register(c, HashSet::from([x]));
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(graph.uses_of(&c), Some(HashSet::from([x])));
        assert!(graph.dependents(&x).contains(&c));
    }

    #[test]
    fn register_vs_remove_self_heals_on_next_register() {
        // Simulates the critical race: cell D is registered to read X,
        // then X is removed by a concurrent delete. The forward edge
        // `uses[D]={X}` may linger. A later re-register of D with a fresh
        // reads set must clean the stale `used_by[X]` entry via the
        // forward-edge diff.
        let graph = DepGraph::default();
        let d = cell(1, 10);
        let x = cell(0, 0);
        let y = cell(0, 1);
        graph.register(d, HashSet::from([x]));
        // X is removed (e.g. its row was deleted)
        let dependents_of_x = graph.remove(&x);
        assert!(dependents_of_x.contains(&d));
        // Now D re-registers with {Y} — this should drop the stale x edge
        graph.register(d, HashSet::from([y]));
        assert_eq!(graph.uses_of(&d), Some(HashSet::from([y])));
        assert!(!graph.dependents(&x).contains(&d), "stale back-edge not cleaned");
        assert!(graph.dependents(&y).contains(&d));
    }

    #[test]
    fn concurrent_register_and_remove_no_panic() {
        // Hammer two threads doing register(D, {X}) and remove(X) in a tight
        // loop. If the maps got out of sync in a way that panics (e.g. a bad
        // unwrap), this would catch it. We don't assert on final state because
        // the race is the whole point — only correctness of each operation.
        const ITERS: u32 = 2_000;
        let graph = Arc::new(DepGraph::default());
        let x = cell(0, 0);
        let g1 = graph.clone();
        let t1 = thread::spawn(move || {
            for i in 0..ITERS {
                let d = cell(1, (i % 50) as ColumnId);
                g1.register(d, HashSet::from([x]));
            }
        });
        let g2 = graph.clone();
        let t2 = thread::spawn(move || {
            for _ in 0..ITERS {
                let _ = g2.remove(&x);
            }
        });
        t1.join().unwrap();
        t2.join().unwrap();
        // Regardless of interleaving, `dependents(X)` must be a valid HashSet
        // (no panic) and `uses_of(D)` for each D is either {X} or absent.
        for i in 0..50u32 {
            let d = cell(1, i as ColumnId);
            if let Some(u) = graph.uses_of(&d) {
                assert!(u.is_subset(&HashSet::from([x])));
            }
        }
    }

    #[test]
    fn index_register_concurrent_hot_bucket() {
        // N cells all watch one hot bucket key — end state has all watchers
        // present. Mirrors the "1000 positions watching PRICE(SOL)" fanout.
        const N: u32 = 400;
        let graph = Arc::new(DepGraph::default());
        let hot = bucket(0, 42);
        let handles: Vec<_> = (0..N)
            .map(|i| {
                let g = graph.clone();
                thread::spawn(move || {
                    let c = cell((i + 1) as u64, 0);
                    g.index_register(c, HashSet::from([hot]));
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let watchers = graph.watchers_of_bucket(&hot);
        assert_eq!(watchers.len(), N as usize);
    }

    #[test]
    fn index_register_replacement_drops_old_edges() {
        // index_register with a new set must sever stale back-edges on the
        // previous bucket keys (forward-edge diffing).
        let graph = DepGraph::default();
        let c = cell(1, 0);
        let b_old = bucket(0, 1);
        let b_new = bucket(0, 2);
        graph.index_register(c, HashSet::from([b_old]));
        graph.index_register(c, HashSet::from([b_new]));
        assert!(!graph.watchers_of_bucket(&b_old).contains(&c));
        assert!(graph.watchers_of_bucket(&b_new).contains(&c));
    }

    #[test]
    fn mixed_register_remove_hammer() {
        // Three thread classes:
        //   - registers cells that depend on a shared pool of read targets
        //   - removes random read targets
        //   - reads `dependents` (the query path used by replace/delete)
        // Goal: no panics, no deadlocks. Timeout is the test timeout.
        const READS: u32 = 20;
        const ITERS: u32 = 1_000;
        let graph = Arc::new(DepGraph::default());
        let targets: Vec<CellId> = (0..READS).map(|i| cell(0, i as ColumnId)).collect();

        let g1 = graph.clone();
        let t1_targets = targets.clone();
        let t1 = thread::spawn(move || {
            for i in 0..ITERS {
                let d = cell(1, (i % 100) as ColumnId);
                let read_count = (i as usize % 4) + 1;
                let reads: HashSet<CellId> = t1_targets
                    .iter()
                    .cycle()
                    .skip(i as usize % READS as usize)
                    .take(read_count)
                    .copied()
                    .collect();
                g1.register(d, reads);
            }
        });

        let g2 = graph.clone();
        let t2_targets = targets.clone();
        let t2 = thread::spawn(move || {
            for i in 0..ITERS {
                let t = t2_targets[(i as usize) % t2_targets.len()];
                let _ = g2.remove(&t);
            }
        });

        let g3 = graph.clone();
        let t3_targets = targets.clone();
        let t3 = thread::spawn(move || {
            for i in 0..ITERS {
                let t = t3_targets[(i as usize) % t3_targets.len()];
                let _ = g3.dependents(&t);
            }
        });

        t1.join().unwrap();
        t2.join().unwrap();
        t3.join().unwrap();
    }
}
