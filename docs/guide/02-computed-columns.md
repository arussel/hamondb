# Computed columns

A computed column is a field whose value is a function of the row (and
optionally of other rows in the database). Hamon tracks what each computed
column reads and recomputes it whenever anything it read has changed.

You never call the function yourself. You declare the dependency, write the
function once, and trust the recompute loop.

## Declaring one

Mark the field with `#[computed]` and define a static method on the struct
with the same name as the field. The method takes `&Row<Self>` and `&Ctx` and
returns the field's type:

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
        row.get(Self::QTY) * row.get(Self::PRICE_CENTS)
    }
}
```

The function **must** read its inputs through `row.get(Self::COLUMN)`. That
read is how Hamon learns that `total_cents` depends on `qty` and `price_cents`
— there is no separate dependency list for same-row reads.

The initial value you pass at insert time is ignored: `total_cents` is
recomputed from scratch as part of the insert.

```rust
# use hamondb::{HamonBuilder, Table};
# use hamondb::hamon_core::{Ctx, Row};
# #[derive(Clone, Table)]
# struct Order {
#     #[primary_key] id: u64, qty: i64, price_cents: i64,
#     #[computed] total_cents: i64,
# }
# impl Order {
#     fn total_cents(row: &Row<Self>, _ctx: &Ctx) -> i64 {
#         row.get(Self::QTY) * row.get(Self::PRICE_CENTS)
#     }
# }
let mut builder = HamonBuilder::new();
builder.register_table::<Order>();
let (db, _shutdown) = builder.build();

db.insert(Order {
    id: 1, qty: 10, price_cents: 500,
    total_cents: 0, // ignored; recomputed on insert
}).unwrap();
assert_eq!(db.get::<Order>(&1).unwrap().total_cents, 5_000);
```

When a regular column changes, the computed column recomputes on the same
write path:

```rust
# use hamondb::{HamonBuilder, Table};
# use hamondb::hamon_core::{Ctx, Row};
# #[derive(Clone, Table)]
# struct Order {
#     #[primary_key] id: u64, qty: i64, price_cents: i64,
#     #[computed] total_cents: i64,
# }
# impl Order {
#     fn total_cents(row: &Row<Self>, _ctx: &Ctx) -> i64 {
#         row.get(Self::QTY) * row.get(Self::PRICE_CENTS)
#     }
# }
# let mut builder = HamonBuilder::new();
# builder.register_table::<Order>();
# let (db, _shutdown) = builder.build();
# db.insert(Order { id: 1, qty: 10, price_cents: 500, total_cents: 0 }).unwrap();
db.update::<Order>(&1, (Order::QTY, 3)).unwrap();
assert_eq!(db.get::<Order>(&1).unwrap().total_cents, 1_500);
```

You cannot `update` a computed column directly — it's derived, not stored
input:

```rust
# use hamondb::{HamonBuilder, Table};
# use hamondb::hamon_core::{Ctx, Row, HamonError};
# #[derive(Clone, Table)]
# struct Order {
#     #[primary_key] id: u64, qty: i64, price_cents: i64,
#     #[computed] total_cents: i64,
# }
# impl Order {
#     fn total_cents(row: &Row<Self>, _ctx: &Ctx) -> i64 {
#         row.get(Self::QTY) * row.get(Self::PRICE_CENTS)
#     }
# }
# let mut builder = HamonBuilder::new();
# builder.register_table::<Order>();
# let (db, _shutdown) = builder.build();
# db.insert(Order { id: 1, qty: 10, price_cents: 500, total_cents: 0 }).unwrap();
let err = db.update::<Order>(&1, (Order::TOTAL_CENTS, 999));
assert_eq!(err, Err(HamonError::ComputedColumnUpdate));
```

## Computed columns that depend on other computed columns

When a computed column reads another computed column, the macro cannot see
that from the body alone — you have to declare it. Pass the dependency column
names to `#[computed(...)]`:

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
    #[computed(TOTAL_CENTS)]
    tax_cents: i64,
}

impl Order {
    fn total_cents(row: &Row<Self>, _ctx: &Ctx) -> i64 {
        row.get(Self::QTY) * row.get(Self::PRICE_CENTS)
    }
    fn tax_cents(row: &Row<Self>, _ctx: &Ctx) -> i64 {
        row.get(Self::TOTAL_CENTS) / 10
    }
}

# let mut builder = HamonBuilder::new();
# builder.register_table::<Order>();
# let (db, _shutdown) = builder.build();
# db.insert(Order { id: 1, qty: 10, price_cents: 500, total_cents: 0, tax_cents: 0 }).unwrap();
# assert_eq!(db.get::<Order>(&1).unwrap().tax_cents, 500);
```

If a computed column reads another computed column at runtime without
declaring it in `#[computed(...)]`, the compute function panics with
`computed column `X` read undeclared computed column `Y``. The declaration
is not just documentation — Hamon uses it to order the recomputes and to
detect cycles at build time.

Cycles are rejected: constructing a `HamonBuilder` with `A` depending on `B`
and `B` depending on `A` panics on `build()` with
`computed column cycle detected`.

## Reading from another table

The `&Ctx` second argument is how you look up rows of other tables inside a
compute function. Reads through `ctx.lookup::<OtherTable>(&key)` are tracked
the same way as same-row reads: the computed column is recorded as watching
that cell, so a later write to that other table's row re-fires this compute.

```rust
use hamondb::{HamonBuilder, Table};
use hamondb::hamon_core::{Ctx, Row};
use std::thread;
use std::time::Duration;

#[derive(Clone, Table)]
struct Price {
    #[primary_key]
    symbol: u32,
    mark: i64,
}

#[derive(Clone, Table)]
struct Position {
    #[primary_key]
    id: u64,
    symbol: u32,
    entry: i64,
    size: i64,
    #[computed]
    pnl: i64,
}

impl Position {
    fn pnl(row: &Row<Self>, ctx: &Ctx) -> i64 {
        let entry = row.get(Self::ENTRY);
        let mark = ctx
            .lookup::<Price>(&row.get(Self::SYMBOL))
            .map(|p| p.get(Price::MARK))
            .unwrap_or(entry);
        (mark - entry) * row.get(Self::SIZE)
    }
}

let mut builder = HamonBuilder::new();
builder.register_table::<Price>();
builder.register_table::<Position>();
let (db, _shutdown) = builder.build();

db.insert(Price { symbol: 1, mark: 100 }).unwrap();
db.insert(Position { id: 1, symbol: 1, entry: 90, size: 10, pnl: 0 }).unwrap();
assert_eq!(db.get::<Position>(&1).unwrap().pnl, 100); // (100 - 90) * 10

// Updating the Price re-fires Position.pnl — but on a background worker.
db.update::<Price>(&1, (Price::MARK, 120)).unwrap();
thread::sleep(Duration::from_millis(50));
assert_eq!(db.get::<Position>(&1).unwrap().pnl, 300);
```

Two things are worth calling out here:

1. **Cross-table recomputes run asynchronously.** A write that triggers only
   same-row recomputes finishes them before returning; a write that triggers
   recomputes on other rows (same table or not) enqueues them for background
   workers. The sleep above is a doctest convenience, not a required pattern
   — see [Reactive queries](./03-reactive-queries.md) for the proper
   subscribe-to-changes pattern.
2. **Dependency tracking is per-compute-call.** Every time `pnl` recomputes,
   `ctx.lookup::<Price>(&…)` re-records the symbol it read. If your lookup
   key depends on data that changes, the tracked dependencies move with it —
   you don't have to re-declare them.

## Not available in computed functions

- `ctx.resource::<M, V>(&r)` returns the current value of a shared resource
  but **reads are not tracked** — a computed column reading a resource will
  not re-fire when the resource is updated via `set_resource`.
- There is no synchronous "recompute now" primitive. Cross-row cascades run
  on the pool created by `build`.
