use std::any::{Any, TypeId};

pub type ColumnId = usize;

#[derive(Debug)]
pub struct Column<T: Send + Sync, V> {
    pub id: ColumnId,
    pub name: &'static str,
    pub get: fn(&T) -> V,
    pub set: fn(&mut T, V),
}

impl<T: Send + Sync, V> Clone for Column<T, V> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            name: self.name,
            get: self.get,
            set: self.set,
        }
    }
}

impl<T: Send + Sync, V> Copy for Column<T, V> {}

pub trait AnyColumn<T: Send + Sync>: Send + Sync {
    fn extract_any(&self, row: &T) -> Box<dyn Any>;
    fn id(&self) -> ColumnId;
    fn differs(&self, old: &T, new: &T) -> bool;
}

impl<T: Send + Sync, V: PartialEq + 'static> AnyColumn<T> for Column<T, V> {
    fn extract_any(&self, row: &T) -> Box<dyn Any> {
        Box::new((self.get)(row))
    }
    fn id(&self) -> ColumnId {
        self.id
    }
    fn differs(&self, old: &T, new: &T) -> bool {
        (self.get)(old) != (self.get)(new)
    }
}

#[derive(Clone, Copy, Hash, Eq, PartialEq, Debug)]
pub(super) struct CellId {
    pub(super) table: TypeId,
    pub(super) row: u64,
    pub(super) column: ColumnId,
}
