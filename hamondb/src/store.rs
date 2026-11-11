use crate::column::{AnyColumn, CellId, Column, ColumnId};
use crate::index::{Index, IndexMaintainer};
use crate::hamon_core::{BucketKey, ComputeContext, Ctx, DepGraph, HamonError, Row, RowInner};
use crate::table::{IntoPrimaryKey, Table};
use crate::update::Update;
use dashmap::DashMap;
use parking_lot::{RawRwLock, RwLock, RwLockWriteGuard};
use std::any::{Any, TypeId};
use std::collections::HashSet;
use std::hash::RandomState;
use std::sync::Arc;

pub struct ComputedColumn<T: Table + Send + Sync, V: Send + Sync> {
    pub column: Column<T, V>,
    pub closure: Box<dyn Fn(&Row<T>, &Ctx) -> V + Send + Sync>,
    pub computed_column_dependencies: HashSet<ColumnId>,
}

pub trait AnyComputedColumn<T: Table + Send + Sync>: Send + Sync {
    fn apply<'a>(&self, ctx: &Ctx, row_id: u64, row: &mut T, cc: ComputeContext);
    fn column_id(&self) -> ColumnId;
    fn copy_from(&self, src: &T, dst: &mut T);
    fn differs(&self, old: &T, new: &T) -> bool;
    fn get_computed_column_dependencies(&self) -> &HashSet<ColumnId>;
    fn id(&self) -> ColumnId;
    fn name(&self) -> &'static str;
}

impl<T: Table + Send + Sync, V: PartialEq + Send + Sync + 'static> AnyComputedColumn<T>
    for ComputedColumn<T, V>
{
    fn apply<'a>(&self, ctx: &Ctx, row_id: u64, row: &mut T, cc: ComputeContext) {
        let value = {
            let wrapped_row = Row::new_for_compute(RowInner::Ref(row), row_id, ctx, cc);
            (self.closure)(&wrapped_row, ctx)
        };
        (self.column.set)(row, value);
    }

    fn column_id(&self) -> ColumnId {
        self.column.id
    }

    fn copy_from(&self, src: &T, dst: &mut T) {
        let value = (self.column.get)(src);
        (self.column.set)(dst, value);
    }

    fn differs(&self, old: &T, new: &T) -> bool {
        (self.column.get)(old) != (self.column.get)(new)
    }

    fn get_computed_column_dependencies(&self) -> &HashSet<ColumnId> {
        &self.computed_column_dependencies
    }

    fn id(&self) -> ColumnId {
        self.column.id
    }

    fn name(&self) -> &'static str {
        self.column.name
    }
}

pub(super) struct Listener<T: Table + Send + Sync> {
    pub(super) closure: Box<dyn Fn(&T::PrimaryKey, Option<&T>, Option<&T>) + Send + Sync>,
}

pub(super) trait AnyListener<T: Table + Send + Sync>: Send + Sync {
    fn on_change(&self, key: &T::PrimaryKey, old: Option<&T>, new: Option<&T>);
}

impl<T: Table + Send + Sync> AnyListener<T> for Listener<T> {
    fn on_change(&self, key: &T::PrimaryKey, old: Option<&T>, new: Option<&T>) {
        (self.closure)(key, old, new)
    }
}

pub(super) enum ChangedColumnFilter {
    RegularOnly,
    ComputedOnly,
    All,
}

pub(super) trait AnyTableStore: Send + Sync + Any {
    fn recompute_row(
        &self,
        row_id: u64,
        columns: HashSet<ColumnId>,
        ctx: &Ctx,
        rwlock_dep_graph: &RwLock<DepGraph>,
    ) -> HashSet<CellId>;

    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

pub(super) enum InsertResult<T: Table + Send + Sync> {
    Inserted(
        parking_lot::ArcRwLockWriteGuard<RawRwLock, T>,
        Vec<BucketKey>,
    ),
    Duplicate(T, T::PrimaryKey, Arc<RwLock<T>>),
}

pub(super) struct TableStore<T: Table + Send + Sync> {
    pub(super) rows: DashMap<T::PrimaryKey, Arc<RwLock<T>>>,
    pub(super) pk_to_row_id: DashMap<T::PrimaryKey, u64>,
    pub(super) row_id_to_pk: DashMap<u64, T::PrimaryKey>,
    pub(super) pk_column_ids: Vec<ColumnId>,
    pk_extract: Box<dyn Fn(&T) -> T::PrimaryKey + Send + Sync>,
    columns: Vec<Box<dyn AnyColumn<T>>>,
    indexes: RwLock<Vec<Box<dyn IndexMaintainer<T>>>>,
    pub(super) listeners: Vec<Box<dyn AnyListener<T>>>,
    pub(super) computed_columns: Vec<Box<dyn AnyComputedColumn<T>>>,
    computed_column_ids: HashSet<ColumnId>,
}

pub(super) struct ReplaceOutcome<T: Table> {
    pub old_row: T,
    pub new_row: T,
    pub bucket_moves: Vec<BucketMove>, // one per index whose V changed
}

pub(super) struct BucketMove {
    pub from: BucketKey,
    pub to: BucketKey,
}

impl<T: Table> AnyTableStore for TableStore<T> {
    fn recompute_row(
        &self,
        row_id: u64,
        columns: HashSet<ColumnId>,
        ctx: &Ctx,
        dep_graph: &RwLock<DepGraph>,
    ) -> HashSet<CellId> {
        let Some(pk) = self.row_id_to_pk.get(&row_id).map(|p| p.clone()) else {
            return HashSet::default();
        };
        let Some(arc) = self.get_row_from_pk(&pk) else {
            return HashSet::default();
        };
        let guard = arc.write();
        let mut new = guard.clone();
        let mut dep_graph = dep_graph.write();
        let cross_row =
            self.fixpoint_compute(row_id, columns, &mut new, &guard, ctx, &mut *dep_graph);
        drop(dep_graph);
        let (old, new) = self.replace(guard, new);
        self.fire_lister(&Some(old), &Some(new));
        cross_row
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl<T: Table> TableStore<T> {
    pub(super) fn new(
        pk: impl IntoPrimaryKey<T> + Send + Sync + 'static,
        regular_columns: Vec<Box<dyn AnyColumn<T>>>,
        indexes: RwLock<Vec<Box<dyn IndexMaintainer<T>>>>,
        listeners: Vec<Box<dyn AnyListener<T>>>,
        computed_columns: Vec<Box<dyn AnyComputedColumn<T>>>,
    ) -> Self {
        let mut columns = pk.columns();
        let pk_column_ids = columns.iter().map(|c| c.id()).collect();
        columns.extend(regular_columns);
        let pk_extract: Box<dyn Fn(&T) -> T::PrimaryKey + Sync + Send> =
            Box::new(move |row| pk.extract(row));
        let computed_column_ids = computed_columns.iter().map(|c| c.column_id()).collect();
        Self {
            rows: DashMap::new(),
            pk_to_row_id: DashMap::new(),
            row_id_to_pk: DashMap::new(),
            pk_column_ids,
            pk_extract,
            columns,
            indexes,
            listeners,
            computed_columns,
            computed_column_ids,
        }
    }

    pub(super) fn fixpoint_compute(
        &self,
        row_id: u64,
        initial_columns: HashSet<ColumnId>,
        row: &mut T,
        old_row: &T,
        ctx: &Ctx,
        dep_graph: &mut DepGraph,
    ) -> HashSet<CellId> {
        Self::fixpoint_compute_inner(
            &self.computed_columns,
            &self.computed_column_ids,
            row_id,
            initial_columns,
            row,
            old_row,
            ctx,
            dep_graph,
        )
    }

    fn fixpoint_compute_inner(
        computed_columns: &Vec<Box<dyn AnyComputedColumn<T>>>,
        computed_column_ids: &HashSet<ColumnId>,
        row_id: u64,
        initial_columns: HashSet<ColumnId>,
        row: &mut T,
        old_row: &T,
        ctx: &Ctx,
        dep_graph: &mut DepGraph,
    ) -> HashSet<CellId> {
        let mut cross_row: HashSet<CellId> = HashSet::new();
        let mut to_recompute = initial_columns;
        let mut reconcile_against: T = old_row.clone();

        loop {
            if to_recompute.is_empty() {
                break;
            }
            let deps = Self::compute_columns_inner(
                computed_columns,
                computed_column_ids,
                row,
                ctx,
                row_id,
                Some(&to_recompute),
            );
            for (c, reads) in deps {
                dep_graph.register(c, reads);
            }
            let changed = Self::reconcile_computed(
                computed_columns,
                row_id,
                &reconcile_against,
                row,
                &to_recompute,
            );
            reconcile_against = row.clone();
            let next_dependents: HashSet<CellId> = changed
                .iter()
                .flat_map(|c| dep_graph.dependents(c).into_iter().flatten())
                .cloned()
                .collect();
            let (same_row, other_rows): (HashSet<CellId>, HashSet<CellId>) = next_dependents
                .iter()
                .partition(|c| c.table == TypeId::of::<T>() && c.row == row_id);
            cross_row.extend(other_rows.iter().cloned());
            to_recompute = same_row.iter().map(|c| c.column).collect();
        }
        cross_row
    }

    pub(super) fn any_computed(&self, column_ids: &[ColumnId]) -> bool {
        column_ids
            .iter()
            .any(|id| self.computed_column_ids.contains(id))
    }

    pub(super) fn any_primary_key(&self, colum_ids: &[ColumnId]) -> bool {
        colum_ids.iter().any(|id| self.pk_column_ids.contains(id))
    }

    pub(super) fn cell_ids_for_row(&self, row_id: u64) -> Vec<CellId> {
        self.columns
            .iter()
            .map(|c| c.id())
            .chain(self.computed_columns.iter().map(|c| c.id()))
            .map(|col_id| CellId {
                table: TypeId::of::<T>(),
                row: row_id,
                column: col_id,
            })
            .collect()
    }

    pub(super) fn fire_lister(&self, old: &Option<T>, new: &Option<T>) {
        let pk = match old.as_ref().or(new.as_ref()) {
            Some(row) => (self.pk_extract)(row),
            None => return,
        };
        for listener in &self.listeners {
            if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                listener.on_change(&pk, old.as_ref(), new.as_ref())
            })) {
                eprintln!("Listener panicked {:?}", e);
            }
        }
    }

    pub(super) fn apply_update(&self, row: &mut T, update: Update<T>) {
        for col_update in update.updates {
            col_update.apply(row);
        }
    }

    pub(super) fn get_row_from_pk(&self, key: &T::PrimaryKey) -> Option<Arc<RwLock<T>>> {
        self.rows.get(key).map(|arc| arc.clone())
    }

    pub(super) fn get_row_from_row(&self, row: &T) -> Option<(T::PrimaryKey, Arc<RwLock<T>>)> {
        let key = (self.pk_extract)(row);
        self.rows.get(&key).map(|arc| (key, arc.clone()))
    }

    fn reconcile_computed(
        computed_columns: &Vec<Box<dyn AnyComputedColumn<T>>>,
        row_id: u64,
        old_row: &T,
        new_row: &mut T,
        recomputed: &HashSet<ColumnId>,
    ) -> HashSet<CellId> {
        let mut result: HashSet<CellId> = HashSet::new();
        for column in computed_columns {
            if (recomputed.contains(&column.column_id()) && (column.differs(old_row, new_row))) {
                result.insert(CellId {
                    table: TypeId::of::<T>(),
                    row: row_id,
                    column: column.column_id(),
                });
                // if it does not differ, we do keep the result in new row, but it will not be re-evaluated
            } else if (!recomputed.contains(&column.column_id())) {
                //it did not need to be recomputed, the old row holds the value, we copy it into the new row
                // as the caller might use any value in the new row
                column.copy_from(old_row, new_row);
            }
        }
        result
    }

    pub(super) fn changed_columns(
        &self,
        old: &T,
        new: &T,
        column_filter: ChangedColumnFilter,
    ) -> Vec<ColumnId> {
        self.columns
            .iter()
            .filter(|col| match column_filter {
                ChangedColumnFilter::RegularOnly => !self.computed_column_ids.contains(&col.id()),
                ChangedColumnFilter::ComputedOnly => self.computed_column_ids.contains(&col.id()),
                ChangedColumnFilter::All => true,
            })
            .filter(|col| col.differs(old, new))
            .map(|col| col.id())
            .collect()
    }

    pub(super) fn insert(&self, row: T, row_id: u64, hasher: &RandomState) -> InsertResult<T> {
        let pk = (self.pk_extract)(&row);
        let mut bucket_keys: Vec<BucketKey> = Vec::new();
        match self.rows.entry(pk.clone()) {
            dashmap::mapref::entry::Entry::Occupied(entry) => {
                InsertResult::Duplicate(row, pk, entry.get().clone())
            } // duplicate
            dashmap::mapref::entry::Entry::Vacant(e) => {
                let arc = Arc::new(RwLock::new(row.clone()));
                let guard = arc.write_arc();
                let arc_for_map = arc.clone();
                self.row_id_to_pk.insert(row_id, pk.clone());
                self.pk_to_row_id.insert(pk.clone(), row_id);
                e.insert(arc_for_map);
                // shard lock released here (entry dropped)

                let mut indexes = self.indexes.write();
                for index in indexes.iter_mut() {
                    let bucket_key = index.on_insert(&pk, &row, hasher);
                    bucket_keys.push(bucket_key);
                }
                drop(indexes);
                InsertResult::Inserted(guard, bucket_keys)
            }
        }
    }

    pub(super) fn replace(
        &self,
        mut guard: RwLockWriteGuard<T>,
        row: T,
        hasher: &RandomState,
    ) -> ReplaceOutcome<T> {
        let pk = (self.pk_extract)(&row);
        let old_row = guard.clone();
        let mut bucket_moves: Vec<BucketMove> = Vec::new();
        let mut indexes = self.indexes.write();
        let changed_columns = self.changed_columns(&old_row, &row, ChangedColumnFilter::All);
        for index in indexes.iter_mut() {
            if changed_columns
                .iter()
                .any(|c| index.depends_on().contains(c))
            {
                if let Some((from, to)) = index.on_update(&pk, &old_row, &row, hasher) {
                    bucket_moves.push(BucketMove { from, to });
                }
            }
        }
        drop(indexes);
        *guard = row;
        ReplaceOutcome {
            old_row,
            new_row: guard.clone(),
            bucket_moves,
        }
    }

    pub(super) fn delete(
        &self,
        guard: RwLockWriteGuard<T>,
        hasher: &RandomState,
    ) -> (T, Vec<BucketKey>) {
        let old_row = guard.clone();
        let mut bucket_keys: Vec<BucketKey> = Vec::new();
        let pk = (self.pk_extract)(&old_row);
        let mut indexes = self.indexes.write();
        for index in indexes.iter_mut() {
            let bucket_key = index.on_remove(&pk, &old_row, hasher);
            bucket_keys.push(bucket_key);
        }
        drop(indexes);
        let (_, row_id) = self
            .pk_to_row_id
            .remove(&pk)
            .expect("pk_to_row_id out of sync: PK not found during delete");
        self.row_id_to_pk
            .remove(&row_id)
            .expect("row_id_to_pk out of sync: RowId not found during delete");
        self.rows
            .remove(&pk)
            .expect("rows.remove out of sync: PK not found during delete");
        drop(guard);
        (old_row, bucket_keys)
    }

    pub(super) fn len(&self) -> usize {
        self.rows.len()
    }

    pub(super) fn lookup_index<V: 'static>(
        &self,
        index: &Index<T, V>,
        value: &V,
    ) -> Vec<(Arc<RwLock<T>>, u64)> {
        let guard = self.indexes.read();
        let pks: Vec<T::PrimaryKey> = {
            let index_maintainer = guard
                .iter()
                .find(|i| i.id() == index.id)
                .expect("index not declared");
            index_maintainer.lookup(value as &dyn Any)
        };
        drop(guard);
        pks.into_iter()
            .flat_map(|pk| {
                let arc = Arc::clone(self.rows.get(&pk)?.value());
                let row_id = *self.pk_to_row_id.get(&pk)?;
                Some((arc, row_id))
            })
            .collect()
    }

    pub(super) fn for_each(&self, mut f: impl FnMut(&T)) {
        for entry in self.rows.iter() {
            f(&entry.value().read())
        }
    }

    pub(super) fn compute_columns(
        &self,
        row: &mut T,
        ctx: &Ctx,
        row_id: u64,
        only: Option<&HashSet<ColumnId>>,
    ) -> (
        Vec<(CellId, HashSet<CellId>)>,
        Vec<(CellId, HashSet<BucketKey>)>,
    ) {
        Self::compute_columns_inner(
            &self.computed_columns,
            &self.computed_column_ids.clone(),
            row,
            ctx,
            row_id,
            only,
        )
    }

    fn compute_columns_inner(
        computed_columns: &[Box<dyn AnyComputedColumn<T>>],
        computed_column_ids: &HashSet<ColumnId>,
        row: &mut T,
        ctx: &Ctx,
        row_id: u64,
        only: Option<&HashSet<ColumnId>>,
    ) -> (
        Vec<(CellId, HashSet<CellId>)>,
        Vec<(CellId, HashSet<BucketKey>)>,
    ) {
        let mut all_deps = Vec::new();
        let mut all_keys = Vec::new();
        for computed in computed_columns {
            let cc = ComputeContext {
                table: TypeId::of::<T>(),
                current_column: computed.column_id(),
                current_column_name: computed.name(),
                declared_deps: computed.get_computed_column_dependencies(),
                all_computed_ids: computed_column_ids,
            };
            if let Some(filter) = only {
                if !filter.contains(&computed.column_id()) {
                    continue;
                }
            }
            ctx.tracker.borrow_mut().clear();
            ctx.bucket_tracker.borrow_mut().clear();
            let computed_cell = CellId {
                table: TypeId::of::<T>(),
                row: row_id,
                column: computed.column_id(),
            };
            computed.apply(ctx, row_id, row, cc);
            let read_cells = ctx.tracker.borrow().clone();
            let read_index = ctx.bucket_tracker.borrow().clone();
            all_deps.push((computed_cell, read_cells));
            all_keys.push((computed_cell, read_index));
        }
        (all_deps, all_keys)
    }

    pub fn add_computed<V: PartialEq + Send + Sync + 'static>(
        &mut self,
        column: Column<T, V>,
        f: impl Fn(&Row<T>, &Ctx) -> V + Send + Sync + 'static,
        computed_column_dependencies: HashSet<ColumnId>,
    ) {
        self.computed_columns.push(Box::new(ComputedColumn {
            column,
            closure: Box::new(f),
            computed_column_dependencies,
        }));
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test_support::schema::Product;
    use crate::test_support::{make_cc, make_cc_with_used_dependencies};
    use std::collections::HashMap;

    #[derive(Clone)]
    struct T;

    impl Table for T {
        type PrimaryKey = u64;
    }

    #[test]
    fn compute_columns_inner_empty() {
        let mut row = T {};
        let tables: HashMap<TypeId, Box<dyn AnyTableStore>> = HashMap::new();
        let resources: HashMap<TypeId, Box<dyn Any + Send + Sync>> = HashMap::new();
        let ctx = Ctx::new(&tables, &resources);
        let result =
            TableStore::compute_columns_inner(&vec![], &HashSet::new(), &mut row, &ctx, 0, None);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn compute_columns_inner_one() {
        let mut row = T {};
        let tables: HashMap<TypeId, Box<dyn AnyTableStore>> = HashMap::new();
        let resources: HashMap<TypeId, Box<dyn Any + Send + Sync>> = HashMap::new();
        let ctx = Ctx::new(&tables, &resources);
        let cc1 = make_cc::<T>(0, &vec![]);
        let result =
            TableStore::compute_columns_inner(&vec![cc1], &HashSet::new(), &mut row, &ctx, 0, None);
        assert_eq!(result.len(), 1);
        let deps = &result.first().unwrap().1;
        assert!(deps.is_empty());
    }

    #[test]
    fn compute_columns_inner_with_dependencies() {
        let mut row = T {};
        let tables: HashMap<TypeId, Box<dyn AnyTableStore>> = HashMap::new();
        let resources: HashMap<TypeId, Box<dyn Any + Send + Sync>> = HashMap::new();
        let ctx = Ctx::new(&tables, &resources);
        let cc1 = make_cc_with_used_dependencies::<T>(4, &vec![1, 2]);
        let cc2 = make_cc_with_used_dependencies::<T>(5, &vec![2, 4]);
        let cc3 = make_cc_with_used_dependencies::<T>(6, &vec![1, 2, 5]);
        let result = TableStore::compute_columns_inner(
            &vec![cc1, cc2, cc3],
            &HashSet::new(),
            &mut row,
            &ctx,
            0,
            None,
        );
        assert_eq!(result.len(), 3);
        let cell = |col: ColumnId| CellId {
            table: TypeId::of::<T>(),
            row: 0,
            column: col,
        };
        let expected = vec![
            (cell(4), HashSet::from([cell(1), cell(2)])),
            (cell(5), HashSet::from([cell(2), cell(4)])),
            (cell(6), HashSet::from([cell(1), cell(2), cell(5)])),
        ];
        assert_eq!(result, expected);
    }

    #[test]
    fn fixpoint_compute_inner_empty() {
        let original = T {};
        let mut updated = original.clone();
        let tables: HashMap<TypeId, Box<dyn AnyTableStore>> = HashMap::new();
        let resources: HashMap<TypeId, Box<dyn Any + Send + Sync>> = HashMap::new();
        let ctx = Ctx::new(&tables, &resources);
        let mut dep_graph = DepGraph::default();
        let result = TableStore::fixpoint_compute_inner(
            &Vec::new(),
            &HashSet::new(),
            0,
            HashSet::new(),
            &mut updated,
            &original,
            &ctx,
            &mut dep_graph,
        );
        assert_eq!(result.len(), 0);
    }

    fn register_rows<T: Table>(dep_graph: &mut DepGraph, values: Vec<(ColumnId, Vec<ColumnId>)>) {
        for (c_id, ids) in values {
            dep_graph.register(
                CellId {
                    table: TypeId::of::<T>(),
                    row: 0,
                    column: c_id,
                },
                HashSet::from_iter(
                    ids.iter()
                        .map(|id| CellId {
                            table: TypeId::of::<T>(),
                            row: 0,
                            column: id.clone(),
                        })
                        .collect::<Vec<_>>(),
                ),
            )
        }
    }

    #[test]
    fn fixpoint_compute_inner_with_dependents() {
        use crate::test_support::schema::Product;
        use crate::TableExtractor;

        let original = Product {
            sku: 42,
            category_id: 1,
            price_cents: 100,
            quantity: 50,
            effective_price_cents: 100, // no category in empty tables map → markup=0 → price*1.0
            needs_restock: false,       // 50 < 10 == false
            restock_urgency: 0,         // needs_restock=false → return 0
        };

        let mut new = Product {
            sku: 42,
            category_id: 1,
            price_cents: 100,
            quantity: 8, // ← the only field the user touched
            effective_price_cents: 100,
            needs_restock: false, // stale
            restock_urgency: 0,   // stale
        };
        let initial_columns = HashSet::from([5u16 as ColumnId]); // needs_restock — dependent of QUANTITY
        let mut dep_graph = DepGraph::default();

        let tables: HashMap<TypeId, Box<dyn AnyTableStore>> = HashMap::new();
        let resources: HashMap<TypeId, Box<dyn Any + Send + Sync>> = HashMap::new();
        let ctx = Ctx::new(&tables, &resources);
        register_rows::<Product>(
            &mut dep_graph,
            vec![
                (4, vec![1, 2]), // effective_price_cents reads category_id + price_cents
                (5, vec![3]),    // needs_restock reads quantity
                (6, vec![5]), // restock_urgency reads only needs_restock (short-circuited at insert because needs_restock was false, so the QUANTITY branch never ran)
            ],
        );
        let ccs = Product::extract_computed_columns();
        let computed_ids: HashSet<ColumnId> = ccs.iter().map(|c| c.id()).collect();
        let result = TableStore::<Product>::fixpoint_compute_inner(
            &ccs,
            &ccs.iter().map(|cc| cc.id()).collect(),
            0,
            initial_columns,
            &mut new,
            &original,
            &ctx,
            &mut dep_graph,
        );

        assert!(
            result.is_empty(),
            "no cross-row deps expected, got {:?}",
            result
        );
        assert_eq!(new.quantity, 8); // untouched by compute
        assert_eq!(new.needs_restock, true); // recomputed: 8 < 10
        assert_eq!(new.restock_urgency, 1); // recomputed: nr=true, q=8 → branch "else { 1 }"
        assert_eq!(new.effective_price_cents, 100); // not recomputed; no dep changed // --- 3. dep graph: restock_urgency's deps should have expanded ---
        let cell = |col: ColumnId| CellId {
            table: TypeId::of::<Product>(),
            row: 0,
            column: col,
        };

        assert_eq!(
            dep_graph.uses.get(&cell(5)),
            Some(&HashSet::from([cell(3)])),
            "needs_restock (5) still uses only quantity (3)"
        );

        assert_eq!(
            dep_graph.uses.get(&cell(6)),
            Some(&HashSet::from([cell(3), cell(5)])),
            "restock_urgency (6) must now track BOTH needs_restock (5) and quantity (3) — \
       the branch that reads quantity actually ran this time"
        );

        assert_eq!(
            dep_graph.uses.get(&cell(4)),
            Some(&HashSet::from([cell(1), cell(2)])),
            "effective_price_cents (4) unchanged — not recomputed, deps preserved"
        );
    }
}
