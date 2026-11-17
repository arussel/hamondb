# Errors

Hamon's single error type is [`HamonError`](../../hamondb/src/hamon_core.rs).
Every fallible method on `Hamon` returns `Result<_, HamonError>`.

```rust
use hamondb::hamon_core::HamonError;
```

## Variants and when you'll see them

| Variant                              | Thrown by                                                                 |
|--------------------------------------|---------------------------------------------------------------------------|
| `DuplicateKey`                       | `insert` on a key that already exists                                     |
| `KeyNotFound`                        | `get`, `update`, `replace`, `delete` on a missing key                     |
| `ComputedColumnUpdate`               | `update` targeting a `#[computed]` column                                 |
| `PKColumnUpdate`                     | `update` targeting the `#[primary_key]` column                            |
| `ConcurrentModification`             | `upsert` that found a row initially but it was deleted before the replace |
| `TableNotRegistered`                 | *(internal: the public API panics instead of returning this)*             |
| `ComputedColumnDependencyError`      | *(internal, not surfaced through the public API today)*                   |
| `DependencyCycle(Vec<ColumnId>)`     | *(returned only from the internal order-computed-columns pass; `build()` panics on cycles)* |

## Insert / replace / update examples

```rust
use hamondb::{HamonBuilder, Table};
use hamondb::hamon_core::{Ctx, Row, HamonError};

#[derive(Clone, Table)]
struct Order {
    #[primary_key] id: u64,
    qty: i64,
    #[computed] doubled: i64,
}
impl Order {
    fn doubled(row: &Row<Self>, _ctx: &Ctx) -> i64 {
        row.get(Self::QTY) * 2
    }
}

let mut builder = HamonBuilder::new();
builder.register_table::<Order>();
let (db, _shutdown) = builder.build();

db.insert(Order { id: 1, qty: 10, doubled: 0 }).unwrap();

// Duplicate insert
assert_eq!(
    db.insert(Order { id: 1, qty: 99, doubled: 0 }),
    Err(HamonError::DuplicateKey),
);

// Replace without a prior row
assert_eq!(
    db.replace(Order { id: 999, qty: 1, doubled: 0 }),
    Err(HamonError::KeyNotFound),
);

// Update on missing key
assert_eq!(
    db.update::<Order>(&999, (Order::QTY, 5)),
    Err(HamonError::KeyNotFound),
);

// Update on computed column
assert_eq!(
    db.update::<Order>(&1, (Order::DOUBLED, 42)),
    Err(HamonError::ComputedColumnUpdate),
);

// Update on primary key
assert_eq!(
    db.update::<Order>(&1, (Order::ID, 123)),
    Err(HamonError::PKColumnUpdate),
);
```

## `upsert` and `ConcurrentModification`

`upsert` is "insert if absent, else replace". There is a narrow race where
the row exists when `upsert` checks for it, but is deleted by another
thread before the replace happens. `upsert` does not retry — it returns
`ConcurrentModification` and lets you decide:

```text
// Pseudocode for the race window:
// Thread A                Thread B
// upsert(...) sees row    —
// —                       delete(same key)
// upsert tries replace    replace errors: KeyNotFound
// upsert → ConcurrentModification
```

If you hit this, the typical response is to call `upsert` again.

## Panics (not errors)

A handful of conditions panic rather than return `HamonError`:

- **`build()`** panics with `computed column cycle detected` if the
  `#[computed]` declarations form a cycle.
- **`register_slot` / `register_resource`** panic if the same slot or
  resource is registered twice.
- **`db.slot` / `db.set_slot` / `db.resource` / `db.set_resource`** panic if
  the slot or resource type was never registered.
- **Reading a column from a computed function that itself depends on that
  column but did not declare it** panics the compute worker with
  `computed column X read undeclared computed column Y`.
- **A computed function that reads its own column** panics with
  `computed column X cannot read itself`.

These are programmer errors — they indicate a misconfigured schema or a
compute function that doesn't match its `#[computed(...)]` declaration — not
runtime data issues, so they're not represented in `HamonError`.
