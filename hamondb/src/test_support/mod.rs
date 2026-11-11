pub mod schema;

use crate::{AnyComputedColumn, Column, ColumnId, ComputedColumn, Table};

pub(super) fn make_cc<T: Table>(id: ColumnId, deps: &[ColumnId]) -> Box<dyn AnyComputedColumn<T>> {
    Box::new(ComputedColumn::<T, u64> {
        column: Column {
            id,
            name: "",
            get: |_| 0,
            set: |_, _| {},
        },
        closure: Box::new(|_, _| 0),
        computed_column_dependencies: deps.iter().copied().collect(),
    })
}

pub(super) fn make_cc_with_used_dependencies<'a, T: Table>(
    id: ColumnId,
    deps: &[ColumnId],
) -> Box<dyn AnyComputedColumn<T>> {
    let deps_owned: Vec<ColumnId> = deps.to_vec();
    Box::new(ComputedColumn::<T, u64> {
        column: Column {
            id,
            name: "",
            get: |_| 0,
            set: |_, _| {},
        },
        closure: Box::new(move |row, ctx| {
            for column_id in &deps_owned {
                row.get(Column {
                    id: column_id.clone(),
                    name: "",
                    get: |_| (),
                    set: |_, _| {},
                })
            }
            0
        }),
        computed_column_dependencies: deps.iter().copied().collect(),
    })
}
