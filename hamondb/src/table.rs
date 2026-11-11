use crate::column::{AnyColumn, Column, ColumnId};
use std::hash::Hash;

pub trait Table: Clone + Send + Sync + 'static {
    type PrimaryKey: Eq + Hash + Clone + Send + Sync;
}

pub(super) trait IntoPrimaryKey<T: Table> {
    fn extract(&self, row: &T) -> T::PrimaryKey;
    fn columns(&self) -> Vec<Box<dyn AnyColumn<T>>>;
}

impl<T: Table + 'static> IntoPrimaryKey<T> for Column<T, T::PrimaryKey> {
    fn extract(&self, row: &T) -> T::PrimaryKey {
        (self.get)(row)
    }

    fn columns(&self) -> Vec<Box<dyn AnyColumn<T>>> {
        vec![Box::new(*self)]
    }
}

impl<T: Table<PrimaryKey = ()> + 'static> IntoPrimaryKey<T> for () {
    fn extract(&self, _row: &T) -> T::PrimaryKey {
        ()
    }

    fn columns(&self) -> Vec<Box<dyn AnyColumn<T>>> {
        vec![]
    }
}

impl<T, A, B> IntoPrimaryKey<T> for (Column<T, A>, Column<T, B>)
where
    T: Table<PrimaryKey = (A, B)> + 'static,
    A: Eq + Hash + Clone + 'static,
    B: Eq + Hash + Clone + 'static,
{
    fn extract(&self, row: &T) -> T::PrimaryKey {
        ((self.0.get)(row), (self.1.get)(row))
    }

    fn columns(&self) -> Vec<Box<dyn AnyColumn<T>>> {
        vec![Box::new(self.0), Box::new(self.1)]
    }
}
