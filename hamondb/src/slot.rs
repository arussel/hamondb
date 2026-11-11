use crate::{Column, Resource, Table};
use std::marker::PhantomData;

pub struct SlotRow<M: 'static, V: Clone> {
    pub value: V,
    pub _marker: PhantomData<M>,
}

impl<M, V: Clone> Clone for SlotRow<M, V> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            _marker: PhantomData,
        }
    }
}

impl<M: Send + Sync + 'static, V: Send + Sync + Clone + 'static> Table for SlotRow<M, V> {
    type PrimaryKey = ();
}

pub struct Slot<M: Send + Sync + 'static, V: Clone + PartialEq + Send + Sync> {
    pub(super) name: &'static str,
    _maker: PhantomData<(fn() -> M, fn() -> V)>,
    pub(super) column: Column<SlotRow<M, V>, V>,
}

impl<M, V> Copy for Slot<M, V>
where
    M: Send + Sync + 'static,
    V: Clone + PartialEq + Send + Sync,
{
}

impl<M, V> Clone for Slot<M, V>
where
    M: Send + Sync + 'static,
    V: Clone + PartialEq + Send + Sync,
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: Send + Sync + 'static, V: Clone + PartialEq + Send + Sync> Slot<M, V> {
    #[doc(hidden)]
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            _maker: PhantomData,
            column: Column {
                id: 0,
                name: name,
                get: |r| r.value.clone(),
                set: |r, v| r.value = v,
            },
        }
    }
}
