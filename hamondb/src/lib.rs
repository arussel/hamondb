extern crate self as hamondb;

mod builder;
mod column;
mod index;
pub mod hamon_core;
mod recompute_queue;
mod resource;
mod slot;
mod store;
mod table;
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub mod test_support;
mod update;

pub use builder::{HamonBuilder, TableExtractor};
pub use column::{AnyColumn, Column, ColumnId};
pub use index::{Index, IndexImpl, IndexMaintainer};
pub use hamon_core::{Hamon, HamonShutdown};
pub use hamondb_macro::{resource, slot, Table};
pub use resource::Resource;
pub use slot::Slot;
pub use store::{AnyComputedColumn, ComputedColumn};
pub use table::Table;

/// The user guide, included here so every example compiles as a doctest.
/// Only built under `--test`; invisible in the normal API.
#[cfg(doctest)]
mod _guide_doctests {
    #[doc = include_str!("../../docs/guide/01-getting-started.md")]
    pub mod _01_getting_started {}
    #[doc = include_str!("../../docs/guide/02-computed-columns.md")]
    pub mod _02_computed_columns {}
    #[doc = include_str!("../../docs/guide/03-reactive-queries.md")]
    pub mod _03_reactive_queries {}
    #[doc = include_str!("../../docs/guide/04-typed-columns.md")]
    pub mod _04_typed_columns {}
    #[doc = include_str!("../../docs/guide/05-concurrency.md")]
    pub mod _05_concurrency {}
    #[doc = include_str!("../../docs/guide/06-errors.md")]
    pub mod _06_errors {}
}
