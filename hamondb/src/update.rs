use crate::column::{Column, ColumnId};
use crate::table::Table;

pub(super) trait ColumnUpdate<T> {
    fn apply(self: Box<Self>, row: &mut T);
    fn column_id(&self) -> ColumnId;
}

pub(super) struct TypedColumnUpdate<T: Send + Sync, V> {
    pub(super) column: Column<T, V>,
    pub(super) value: V,
}

impl<T: Send + Sync, V> ColumnUpdate<T> for TypedColumnUpdate<T, V> {
    fn apply(self: Box<Self>, row: &mut T) {
        (self.column.set)(row, self.value);
    }

    fn column_id(&self) -> ColumnId {
        self.column.id
    }
}

pub struct Update<T: Table> {
    pub(super) updates: Vec<Box<dyn ColumnUpdate<T>>>,
}

impl<T: Table + 'static> Update<T> {
    fn new() -> Self {
        Self {
            updates: Vec::new(),
        }
    }
    fn set<V: 'static>(mut self, column: Column<T, V>, value: V) -> Self {
        self.updates
            .push(Box::new(TypedColumnUpdate { column, value }));
        self
    }

    pub(super) fn column_ids(&self) -> Vec<ColumnId> {
        self.updates.iter().map(|c| c.column_id()).collect()
    }
}

pub trait IntoUpdate<T: Table> {
    fn into_update(self) -> Update<T>;
}

impl<T: Table + 'static, V: 'static> IntoUpdate<T> for (Column<T, V>, V) {
    fn into_update(self) -> Update<T> {
        Update::new().set(self.0, self.1)
    }
}

impl<T: Table + 'static> IntoUpdate<T> for Update<T> {
    fn into_update(self) -> Update<T> {
        self
    }
}
