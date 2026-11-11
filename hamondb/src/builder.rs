use crate::column::AnyColumn;
use crate::index::{Index, IndexImpl, IndexMaintainer};
use crate::hamon_core::{
    Ctx, DepGraph, Hamon, HamonError, HamonInner, HamonShutdown, ShutdownToken, TableRegistry,
};
use crate::recompute_queue::RecomputeQueue;
use crate::resource::Resource;
use crate::slot::SlotRow;
use crate::store::{
    AnyComputedColumn, AnyListener, AnyTableStore, InsertResult, Listener, TableStore,
};
use crate::table::{IntoPrimaryKey, Table};
use crate::{Column, ColumnId, ComputedColumn, Slot};
use arc_swap::ArcSwap;
use parking_lot::{Mutex, RwLock};
use std::any::{type_name, Any, TypeId};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::marker::PhantomData;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Weak};

pub trait TableExtractor<T: Table> {
    type PrimaryKeyExtractor: IntoPrimaryKey<T> + Send + Sync + 'static;
    fn extract_primary_key() -> Self::PrimaryKeyExtractor;
    fn extract_indexes() -> Vec<Box<dyn IndexMaintainer<T>>>;
    fn extract_computed_columns() -> Vec<Box<dyn AnyComputedColumn<T>>>;
    fn extract_regular_columns() -> Vec<Box<dyn AnyColumn<T>>>;
}

pub struct TableBuilder<'a, T: Table, PK: IntoPrimaryKey<T> + Send + Sync + 'static> {
    hamon_builder: &'a mut HamonBuilder,
    indexes: Vec<Box<dyn IndexMaintainer<T>>>,
    computed_columns: Vec<Box<dyn AnyComputedColumn<T>>>,
    regular_columns: Vec<Box<dyn AnyColumn<T>>>,
    listeners: Vec<Box<dyn AnyListener<T>>>,
    _phantom_data: PhantomData<T>,
    pk: PK,
}

impl<'a, T: Table + 'static, PK: IntoPrimaryKey<T> + Sync + Send> TableBuilder<'a, T, PK> {
    pub fn new(
        hamon_builder: &'a mut HamonBuilder,
        pk: PK,
        regular_columns: Vec<Box<dyn AnyColumn<T>>>,
        indexes: Vec<Box<dyn IndexMaintainer<T>>>,
        listeners: Vec<Box<dyn AnyListener<T>>>,
        computed_columns: Vec<Box<dyn AnyComputedColumn<T>>>,
    ) -> Self {
        Self {
            hamon_builder,
            indexes,
            regular_columns,
            computed_columns,
            _phantom_data: PhantomData,
            listeners,
            pk,
        }
    }

    pub fn register_index<V: Eq + Hash + Send + Sync + 'static>(
        &mut self,
        index: Index<T, V>,
        extract: fn(&T) -> V,
    ) -> &mut Self {
        let im = IndexImpl::new(index.columns, extract, index.id);
        self.indexes.push(Box::new(im));
        self
    }

    pub fn register_regular_column<V: PartialEq + 'static>(
        &mut self,
        regular_column: Column<T, V>,
    ) -> &mut Self {
        self.regular_columns.push(Box::new(regular_column));
        self
    }

    pub fn register_computed_column<V: PartialEq + Send + Sync + 'static>(
        &mut self,
        computed_column: ComputedColumn<T, V>,
    ) -> &mut Self {
        self.computed_columns.push(Box::new(computed_column));
        self
    }

    pub fn done(mut self) -> &'a mut HamonBuilder {
        match order_computed_columns(&self.computed_columns) {
            Ok(computed_columns) => {
                let position: HashMap<ColumnId, usize> = computed_columns
                    .into_iter()
                    .enumerate()
                    .map(|(i, id)| (id, i))
                    .collect();
                self.computed_columns.sort_by_key(|c| position[&c.id()]);
                self.hamon_builder.tables.insert(
                    TypeId::of::<T>(),
                    Box::new(TableStore::new(
                        self.pk,
                        self.regular_columns,
                        RwLock::new(self.indexes),
                        self.listeners,
                        self.computed_columns,
                    )),
                );
                self.hamon_builder
            }
            Err(HamonError::DependencyCycle(cycle)) => {
                let mut names: Vec<&str> = Vec::new();
                for column_id in cycle {
                    names.push(
                        &self
                            .computed_columns
                            .iter()
                            .find(|cc| cc.id() == column_id)
                            .unwrap()
                            .name(),
                    )
                }
                panic!("computed column cycle detected: {:?}", names)
            }
            Err(err) => panic!("{:?}", err),
        }
    }
}

fn order_computed_columns<T: Table + 'static>(
    computed_columns: &Vec<Box<dyn AnyComputedColumn<T>>>,
) -> Result<Vec<ColumnId>, HamonError> {
    let mut in_degree: HashMap<ColumnId, usize> = HashMap::new();
    let mut dependents: HashMap<ColumnId, Vec<ColumnId>> = HashMap::new();
    //set up
    let computed_ids: HashSet<ColumnId> = computed_columns.iter().map(|c| c.id()).collect();
    for c in computed_columns {
        let computed_column_dependency_count = c
            .get_computed_column_dependencies()
            .iter()
            .filter(|d| computed_ids.contains(d))
            .count();
        in_degree.insert(c.id(), computed_column_dependency_count);
        for d in c.get_computed_column_dependencies() {
            if computed_ids.contains(&d) {
                dependents.entry(*d).or_default().push(c.id());
            }
        }
    }
    let mut topo_order: Vec<ColumnId> = Vec::new();
    let (mut ready, mut in_degree): (HashMap<ColumnId, usize>, HashMap<ColumnId, usize>) =
        in_degree.into_iter().partition(|(_, v)| *v == 0);
    let mut ready = ready.into_keys().collect::<Vec<_>>();
    while !ready.is_empty() {
        process_next_node(&mut ready, &mut in_degree, &mut topo_order, &dependents);
    }
    if topo_order.len() < computed_columns.len() {
        return Err(HamonError::DependencyCycle(
            in_degree.keys().copied().collect(),
        ));
    }
    Ok(topo_order)
}

fn process_next_node(
    ready: &mut Vec<ColumnId>,
    in_degree: &mut HashMap<ColumnId, usize>,
    topo_order: &mut Vec<ColumnId>,
    dependents: &HashMap<ColumnId, Vec<ColumnId>>,
) {
    let poped = ready.pop().unwrap();
    topo_order.push(poped);
    for d in dependents.get(&poped).unwrap_or(&vec![]) {
        let v = in_degree
            .get_mut(d)
            .expect("dependent missing from in_degree");
        *v -= 1;
        if *v == 0 {
            ready.push(*d);
        }
    }
}

pub struct HamonBuilder {
    tables: HashMap<TypeId, Box<dyn AnyTableStore>>,
    feeders: HashMap<String, Box<dyn Fn(&Hamon, &ShutdownToken) + Send + 'static>>,
    resources: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl HamonBuilder {
    pub fn new() -> Self {
        Self {
            tables: HashMap::new(),
            feeders: HashMap::new(),
            resources: HashMap::new(),
        }
    }

    pub fn register<T: Table + 'static, PK: IntoPrimaryKey<T> + Send + Sync + 'static>(
        &mut self,
        column: PK,
    ) -> TableBuilder<T, PK> {
        TableBuilder::new(self, column, Vec::new(), Vec::new(), Vec::new(), Vec::new())
    }

    pub fn register_table<T: Table + TableExtractor<T> + Send + Sync + 'static>(
        &mut self,
    ) -> &mut Self {
        let pk = T::extract_primary_key();
        let indexes = T::extract_indexes();
        let computed_columns = T::extract_computed_columns();
        let regular_columns = T::extract_regular_columns();
        TableBuilder::new(
            self,
            pk,
            regular_columns,
            indexes,
            Vec::new(),
            computed_columns,
        )
        .done()
    }

    pub fn register_resource<M, V>(&mut self, resource: Resource<M, V>, init: V) -> &mut Self
    where
        M: Send + Sync + 'static,
        V: Send + Sync + 'static,
    {
        match self.resources.entry(TypeId::of::<M>()) {
            Entry::Occupied(mut e) => {
                panic!("resource already exists: {}", resource.name);
            }
            Entry::Vacant(e) => {
                e.insert(Box::new(ArcSwap::from_pointee(init)));
            }
        }
        self
    }

    pub fn register_slot<M, V>(&mut self, slot: Slot<M, V>, init: V) -> &mut Self
    where
        M: Send + Sync + 'static,
        V: Clone + PartialEq + Send + Sync + 'static,
    {
        let store: TableStore<SlotRow<M, V>> = TableStore::new(
            (),
            vec![Box::new(slot.column)],
            RwLock::new(vec![]),
            vec![],
            vec![],
        );
        let result = store.insert(
            SlotRow {
                value: init,
                _marker: PhantomData,
            },
            0,
        );
        assert!(
            matches!(result, InsertResult::Inserted(_)),
            "slot store was not empty at registration",
        );
        match self.tables.entry(TypeId::of::<SlotRow<M, V>>()) {
            Entry::Vacant(entry) => {
                entry.insert(Box::new(store));
            }
            Entry::Occupied(entry) => {
                panic!("slot already exists: {}", slot.name);
            }
        }
        self
    }

    pub fn register_feeder(
        &mut self,
        name: String,
        feeder: impl Fn(&Hamon, &ShutdownToken) + Send + 'static,
    ) -> &mut Self {
        match self.feeders.entry(name) {
            Entry::Vacant(entry) => {
                entry.insert(Box::new(feeder));
            }
            Entry::Occupied(entry) => {
                panic!("feeder already exists: {}", entry.key());
            }
        }
        self
    }

    pub fn register_listener<T: Table + 'static>(
        &mut self,
        f: impl Fn(&T::PrimaryKey, Option<&T>, Option<&T>) + Send + Sync + 'static,
    ) -> &mut Self {
        let store = self
            .tables
            .get_mut(&TypeId::of::<T>())
            .unwrap_or_else(|| panic!("table not registered: {}", type_name::<T>()))
            .as_any_mut()
            .downcast_mut::<TableStore<T>>()
            .expect("table type mismatch");
        store.listeners.push(Box::new(Listener {
            closure: Box::new(f),
        }));
        self
    }

    pub fn build(self) -> (Hamon, HamonShutdown) {
        // let hamon = HamonInner::new(self.tables, self.feeders);
        let shutdown = ShutdownToken::new();
        let deps = RwLock::new(DepGraph::default());
        let join_handles = Mutex::new(vec![]);
        let inner = Arc::new(TableRegistry {
            tables: self.tables,
            deps,
            join_handles,
            shutdown_token: shutdown.clone(),
            resources: self.resources,
        });
        let inner_clone = inner.clone();
        let queue = Arc::new_cyclic(|weak_queue: &Weak<RecomputeQueue>| {
            let weak = weak_queue.clone();
            let callback = Box::new(move |table, row_id, columns| {
                let table = inner_clone.tables.get(&table).unwrap();
                let ctx = Ctx::new(&inner_clone.tables, &inner_clone.resources);
                let cross_row_dependents =
                    { table.recompute_row(row_id, columns, &ctx, &inner_clone.deps) };
                if let Some(queue) = weak.upgrade() {
                    for cell in cross_row_dependents {
                        let _ = queue.enqueue(cell);
                    }
                }
            });
            RecomputeQueue::new(Box::new(callback), 4)
        });

        let me = Arc::new(HamonInner {
            register: inner,
            next_row_id: AtomicU64::new(0),
            queue,
        });

        for (name, feeder) in self.feeders {
            tracing::debug!("registering feeder: {}", name);
            let name_for_err = name.clone();
            let me_for_thread = Arc::clone(&me);
            let token_for_thread = shutdown.clone();
            let handle = std::thread::Builder::new()
                .name(format!("hamondb-feeder-{name}"))
                .spawn(move || {
                    use std::panic::{catch_unwind, AssertUnwindSafe};
                    let hamon = Hamon(me_for_thread);
                    let result = catch_unwind(AssertUnwindSafe(|| {
                        feeder(&hamon, &token_for_thread);
                    }));
                    if let Err(payload) = result {
                        let msg = payload
                            .downcast_ref::<&'static str>()
                            .map(|s| (*s).to_string())
                            .or_else(|| payload.downcast_ref::<String>().cloned())
                            .unwrap_or_else(|| "<non-string panic payload>".into());
                        tracing::error!("feeder {name} panicked: {msg}");
                        std::process::abort();
                    }
                })
                .unwrap_or_else(|e| {
                    panic!("failed to spawn feeder thread for {name_for_err}: {e}")
                });
            me.register.join_handles.lock().push(handle);
        }

        (Hamon(me.clone()), HamonShutdown(me))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test_support::make_cc;

    #[derive(Clone)]
    struct T;

    impl Table for T {
        type PrimaryKey = u64;
    }

    #[test]
    fn test_order_computed_columns_empty() {
        let columns: Vec<Box<dyn AnyComputedColumn<T>>> = vec![];
        let result = order_computed_columns(&columns);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Vec::new());
    }

    #[test]
    fn test_order_computed_columns() {
        let columns = vec![
            make_cc::<T>(4, &vec![5, 2]),
            make_cc::<T>(6, &vec![4]),
            make_cc::<T>(7, &vec![6, 4]),
        ];
        let result = order_computed_columns(&columns);
        assert!(result.is_ok());
    }

    #[test]
    fn test_order_computed_columns_cycle() {
        let columns = vec![
            make_cc::<T>(4, &vec![5, 2, 7]),
            make_cc::<T>(6, &vec![4]),
            make_cc::<T>(7, &vec![6, 4]),
        ];
        let result = order_computed_columns(&columns);
        assert!(result.is_err());
    }
}
