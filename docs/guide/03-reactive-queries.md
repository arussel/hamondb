# Reactive queries

Hamon gives you three ways to observe data:

- **`get` / `for_each`** — one-shot reads.
- **Listeners** — a callback fires every time a row changes.
- **Indexes** — fast lookups by a secondary column, maintained automatically.

## Listeners: react to changes

Register a listener before `build()`. Its signature is
`fn(&PrimaryKey, Option<&T>, Option<&T>)`: the first option is the previous
row (`None` on insert), the second is the new row (`None` on delete).

```rust
use hamondb::{HamonBuilder, Table};
use std::sync::{Arc, Mutex};

#[derive(Clone, Table)]
struct Order {
    #[primary_key]
    id: u64,
    status: u32,
}

let seen: Arc<Mutex<Vec<(u64, Option<u32>, Option<u32>)>>> = Arc::default();
let seen_cb = seen.clone();

let mut builder = HamonBuilder::new();
builder
    .register_table::<Order>()
    .register_listener::<Order>(move |id, old, new| {
        seen_cb.lock().unwrap().push((
            *id,
            old.map(|r| r.status),
            new.map(|r| r.status),
        ));
    });
let (db, _shutdown) = builder.build();

db.insert(Order { id: 1, status: 10 }).unwrap();
db.update::<Order>(&1, (Order::STATUS, 20)).unwrap();
db.delete::<Order>(&1).unwrap();

let log = seen.lock().unwrap().clone();
assert_eq!(log, vec![
    (1, None,     Some(10)),
    (1, Some(10), Some(20)),
    (1, Some(20), None),
]);
```

### When listeners fire

- **`insert`**: fires with `(None, Some(new))`.
- **`replace`** / **`update`**: fires with `(Some(old), Some(new))`, but only
  when at least one regular (non-computed) column changed. A `replace` with
  the same values is silently ignored — the listener does *not* fire.
- **`delete`**: fires with `(Some(old), None)`.
- **Cross-row recompute**: when a computed column on row X fires as a result
  of changes to another row Y, the listener registered for X's table fires
  with `(Some(old), Some(new))`, from the background worker thread.

Listeners run synchronously on the writer thread for direct mutations, and
on a background worker thread for cross-row recomputes. Any panic in a
listener is caught and logged, but the write itself still succeeds.

> Direct writes fire listeners **after** the row's write lock has been
> released. From inside a listener you can call `db.update`, `db.insert`,
> etc. on other rows without self-deadlocking.

## Indexes: look up by a non-primary field

Add `#[index]` to a non-primary field. The derive generates an index handle
as `<FIELD>_IDX` by default; pass a name to `#[index(NAME)]` to override it.

```rust
use hamondb::{HamonBuilder, Table};

#[derive(Clone, Table)]
struct Order {
    #[primary_key]
    id: u64,
    #[index(BY_STATUS)]
    status: u32,
}

# let mut builder = HamonBuilder::new();
# builder.register_table::<Order>();
# let (db, _shutdown) = builder.build();
# db.insert(Order { id: 1, status: 10 }).unwrap();
# db.insert(Order { id: 2, status: 10 }).unwrap();
# db.insert(Order { id: 3, status: 20 }).unwrap();
let mut rows = db.lookup_index::<Order, u32>(Order::BY_STATUS, &10);
rows.sort_by_key(|r| r.id);
let ids: Vec<u64> = rows.iter().map(|r| r.id).collect();
assert_eq!(ids, vec![1, 2]);
```

The index stays in sync automatically: `insert`, `update`, `replace`, and
`delete` all maintain it. Order within the returned `Vec` is unspecified.

### Index dependencies in compute functions

A computed column can read through an index via `ctx.lookup_index::<T, V>(idx, &val)`.
The compute re-fires when a row *enters* or *leaves* the bucket you read
from (e.g. its indexed column was updated). It does *not* re-fire on changes
to rows that stayed in a bucket you didn't read.

## Full scan: `for_each`

For bulk reads that don't fit any index, `for_each` walks every row in a
table, holding the per-row read lock during the closure for that row:

```rust
# use hamondb::{HamonBuilder, Table};
# #[derive(Clone, Table)]
# struct Order { #[primary_key] id: u64, status: u32 }
# let mut builder = HamonBuilder::new();
# builder.register_table::<Order>();
# let (db, _shutdown) = builder.build();
# db.insert(Order { id: 1, status: 10 }).unwrap();
# db.insert(Order { id: 2, status: 10 }).unwrap();
let mut count = 0;
db.for_each::<Order>(|_row| { count += 1; });
assert_eq!(count, 2);
```

`for_each` is a last resort. If you find yourself calling it from a hot path,
the behavior you want is probably an index or a computed column.

## Resources and slots

For shared state that isn't a table, Hamon has two sibling primitives:

- **`resource!(NAME: Type)`** — mutable singleton accessed via
  `db.resource(...)` and `db.set_resource(...)`. Reads are **not** tracked by
  computed columns.
- **`slot!(NAME: Type)`** — a single-value cell that *is* tracked by computed
  columns. Think of it as a one-row table with a trivial primary key.

```rust
use hamondb::HamonBuilder;
use hamondb::{slot, resource};

slot!(pub TICK: u64);
resource!(pub CONFIG: String);

let mut builder = HamonBuilder::new();
builder.register_slot(TICK, 0);
builder.register_resource(CONFIG, "default".to_string());
let (db, _shutdown) = builder.build();

db.set_slot(TICK, 42).unwrap();
assert_eq!(db.slot(TICK), 42);

db.set_resource(CONFIG, "live".to_string());
assert_eq!(&*db.resource(CONFIG), "live");
```

Use a slot when you want a computed column to re-fire on change. Use a
resource when you don't.