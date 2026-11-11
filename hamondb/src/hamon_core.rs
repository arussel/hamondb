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
#[derive(Default)]
pub(super) struct DepGraph {
    pub(super) uses: HashMap<CellId, HashSet<CellId>>,
    pub(super) used_by: HashMap<CellId, HashSet<CellId>>,
    pub(super) used_by_index: HashMap<BucketKey, HashSet<CellId>>,
    pub(super) uses_index: HashMap<CellId, HashSet<BucketKey>>,
}

impl<V: Eq + Hash + Clone> DepGraph {
    pub(super) fn index_register<T: Table>(
        &mut self,
        cell_id: CellId,
        bucket_keys: HashSet<BucketKey>,
    ) {
        // clear stale outgoing edges for `computed`
        if let Some(old_reads) = self.uses_index.remove(&cell_id) {
            for old in &old_reads {
                if let Some(set) = self.used_by_index.get_mut(old) {
                    set.remove(&cell_id);
                }
            }
        }
        for read in &bucket_keys {
            self.used_by_index.entry(*read).or_default().insert(cell_id);
        }
        self.uses_index.insert(cell_id, bucket_keys);
    }

    pub(super) fn index_clear_cells(&mut self, cell: CellId) {
        if let Some(buckets) = self.uses_index.remove(&cell) {
            for b in &buckets {
                if let Some(set) = self.used_by_index.get_mut(b) {
                    set.remove(&cell);
                    if set.is_empty() {
                        self.used_by_index.remove(b);
                    }
                }
            }
        }
    }
    pub(super) fn register(&mut self, computed: CellId, reads: HashSet<CellId>) {
        // clear stale outgoing edges for `computed`
        if let Some(old_reads) = self.uses.remove(&computed) {
            for old in &old_reads {
                if let Some(set) = self.used_by.get_mut(old) {
                    set.remove(&computed);
                }
            }
        }
        for read in &reads {
            self.used_by.entry(*read).or_default().insert(computed);
        }
        self.uses.insert(computed, reads);
    }

    fn remove(&mut self, cell: &CellId) -> HashSet<CellId> {
        // clear what C reads, they will possibly be reinserted if recompute (not a delete)
        if let Some(reads) = self.uses.remove(cell) {
            for read in &reads {
                if let Some(set) = self.used_by.get_mut(read) {
                    set.remove(cell);
                }
            }
        }

        // clear who reads C, return for recomputation
        if let Some(dependents) = self.used_by.remove(cell) {
            for dep in &dependents {
                if let Some(set) = self.uses.get_mut(dep) {
                    set.remove(cell);
                }
            }
            dependents
        } else {
            HashSet::new()
        }
    }

    pub(super) fn dependents(&self, cell: &CellId) -> Option<&HashSet<CellId>> {
        self.used_by.get(cell)
    }
}

pub enum InsertOutcome<T: Table + Send + Sync> {
    Inserted,
    Duplicate(T, T::PrimaryKey, Arc<RwLock<T>>),
}

pub struct TableRegistry {
    pub(super) tables: HashMap<TypeId, Box<dyn AnyTableStore>>,
    pub(super) deps: RwLock<DepGraph>,
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
        let ctx = Ctx::new(&self.register.tables, &self.register.resources);
        let mut new = row.clone();
        let (deps, bucket_deps) = table.compute_columns(&mut new, &ctx, row_id, None);
        match table.insert(new, row_id) {
            InsertResult::Inserted(guard) => {
                let mut dep_graph = self.register.deps.write();
                for (computed_cell, read_cells) in deps {
                    dep_graph.register(computed_cell, read_cells)
                }
                for (computed_cell, read_bucket) in bucket_deps {
                    dep_graph.index_register()
                }
                let new_copy = guard.clone();
                drop(dep_graph);
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
            let mut dep_graph: RwLockWriteGuard<RawRwLock, DepGraph> = self.register.deps.write();
            let dependent_cells: HashSet<CellId> = changed_regular_cells
                .iter()
                .flat_map(|cell| dep_graph.dependents(cell).into_iter().flatten())
                .cloned()
                .collect();
            let (mut same_row, mut other_rows): (HashSet<CellId>, HashSet<CellId>) =
                dependent_cells
                    .iter()
                    .partition(|cell| cell.table == TypeId::of::<T>() && cell.row == row_id);

            let initial = same_row.iter().map(|c| c.column).collect::<HashSet<_>>();
            let ctx = Ctx::new(&self.register.tables, &self.register.resources);
            let cross_row_additions =
                table.fixpoint_compute(row_id, initial, &mut row, &*guard, &ctx, &mut *dep_graph);
            other_rows.extend(cross_row_additions);
            let ReplaceOutcome{old_row, new_row, bucket_moves} = table.replace(guard, row, &self.register.hasher);
            drop(dep_graph);
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
        table.lookup_index(index, value)
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
                    let mut dep_graph = self.register.deps.write();
                    for key in &bucket_keys {
                        if let Some(cells) = dep_graph.used_by_index.get(key) {
                            for cell in cells {
                                let _ = self.queue.enqueue(*cell);
                            }
                        }
                    }
                    for cell in table.cell_ids_for_row(row_id) {
                        let to_recompute = dep_graph.remove(&cell);
                        for cell in to_recompute {
                            let _ = self.queue.enqueue(cell);
                        }
                    }
                    drop(dep_graph);
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
    hasher: &'a RandomState,
}

impl<'a> Ctx<'a> {
    pub fn lookup<T: Table + 'static>(&self, key: &T::PrimaryKey) -> Option<Row<T>> {
        let table = self.get_table::<T>();
        let arc = table.get_row_from_pk(key)?;
        let guard_for_row = arc.read_arc(); // first read lock — goes into Row
        let guard_for_hold = arc.read_arc(); // second read lock — goes into held_locks
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
                let guard_for_hold = guard.read_arc();
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
    ) -> Self {
        Ctx {
            tables,
            tracker: RefCell::new(HashSet::new()),
            held_locks: RefCell::new(Vec::new()),
            resources,
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
