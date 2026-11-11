use std::marker::PhantomData;

pub struct Resource<M, V> {
    pub name: &'static str,
    pub _marker: PhantomData<fn(M) -> V>,
}

impl<M, V> Copy for Resource<M, V> {}
impl<M, V> Clone for Resource<M, V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M, V> Resource<M, V> {
    #[doc(hidden)]
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            _marker: PhantomData,
        }
    }
}
