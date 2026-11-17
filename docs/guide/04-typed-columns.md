# Typed columns

Every field of a `#[derive(Table)]` struct gets a `pub const` associated with
its SHOUTY_SNAKE_CASE name. These constants are the *only* way to refer to a
column in `update`, `(col, value)` tuples, or inside a computed function.

## What the handle encodes

A column handle is a typed [`Column<T, V>`] pairing the table (`T`) with the
column's value type (`V`). This lets the compiler enforce several things that
would otherwise be runtime failures:

```rust
use hamondb::{HamonBuilder, Table};

#[derive(Clone, Table)]
struct Order {
    #[primary_key]
    id: u64,
    qty: i64,
    note: String,
}
```

For this struct the derive produces:

```text
Order::ID    : Column<Order, u64>
Order::QTY   : Column<Order, i64>
Order::NOTE  : Column<Order, String>
```

### Wrong value type → compile error

`Order::QTY` expects an `i64`. Passing a `&str` does not type-check:

```compile_fail
# use hamondb::{HamonBuilder, Table};
# #[derive(Clone, Table)]
# struct Order { #[primary_key] id: u64, qty: i64, note: String }
# let mut builder = HamonBuilder::new();
# builder.register_table::<Order>();
# let (db, _shutdown) = builder.build();
# db.insert(Order { id: 1, qty: 1, note: String::new() }).unwrap();
db.update::<Order>(&1, (Order::QTY, "not an int")).unwrap();
```

### Wrong table → compile error

Column constants are tied to their table. You cannot mix them:

```compile_fail
# use hamondb::{HamonBuilder, Table};
# #[derive(Clone, Table)]
# struct Order { #[primary_key] id: u64, qty: i64 }
# #[derive(Clone, Table)]
# struct Other { #[primary_key] id: u64, flag: i64 }
# let mut builder = HamonBuilder::new();
# builder.register_table::<Order>();
# builder.register_table::<Other>();
# let (db, _shutdown) = builder.build();
# db.insert(Order { id: 1, qty: 1 }).unwrap();
db.update::<Order>(&1, (Other::FLAG, 1)).unwrap();
```

### Updating a computed or primary-key column → runtime error

The compile-time handle doesn't remember *which* columns are writable — that
check happens at runtime and returns `Err(ComputedColumnUpdate)` or
`Err(PKColumnUpdate)`. See [Errors](./06-errors.md).

## Reading a column inside a compute

Inside a `#[computed]` function, use `row.get(Self::FIELD)` rather than the
raw field access. The `get` call is how dependency tracking happens: if you
read `row.foo` directly, Hamon does not learn that you depend on `foo`, and
your computed value will not re-fire when `foo` changes.

```rust
use hamondb::{HamonBuilder, Table};
use hamondb::hamon_core::{Ctx, Row};

#[derive(Clone, Table)]
struct Order {
    #[primary_key]
    id: u64,
    qty: i64,
    price_cents: i64,
    #[computed]
    total_cents: i64,
}

impl Order {
    fn total_cents(row: &Row<Self>, _ctx: &Ctx) -> i64 {
        // CORRECT: tracked.
        row.get(Self::QTY) * row.get(Self::PRICE_CENTS)
    }
}
```

## Field types

Each `row.get(Self::FIELD)` returns the field's value by clone (the derived
getter is `|r| r.field.clone()`). For `Copy` types like `u64` or `i64`, this
is free. For `String`, every read allocates. If you have string columns that
are read often by computes, prefer `Arc<str>` — `Arc::clone` is a refcount
bump, no allocation:

```rust
# use hamondb::{HamonBuilder, Table};
use std::sync::Arc;

#[derive(Clone, Table)]
struct User {
    #[primary_key]
    id: u64,
    name: Arc<str>,
}
```

## `#[skip]`

Fields marked `#[skip]` are stored but not exposed as a column handle. You
can read them from a row obtained via `get` / `for_each`, but you can't use
them from a computed function or an index.

`#[skip]` cannot be combined with `#[primary_key]`, `#[index]`, or
`#[computed]` — the macro rejects those combinations at compile time.
