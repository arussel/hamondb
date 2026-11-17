# Concurrency

Hamon is designed to be shared freely across threads. A `Hamon` is cheap to
clone (it's an `Arc` inside) and every public write method takes `&self`.

```rust
# use hamondb::{HamonBuilder, Table};
# #[derive(Clone, Table)]
# struct Order { #[primary_key] id: u64, qty: i64 }
use std::sync::Arc;
use std::thread;

let mut builder = HamonBuilder::new();
builder.register_table::<Order>();
let (db, _shutdown) = builder.build();
let db = Arc::new(db);

let mut handles = vec![];
for id in 0..4u64 {
    let db = db.clone();
    handles.push(thread::spawn(move || {
        db.insert(Order { id, qty: id as i64 }).unwrap();
    }));
}
for h in handles { h.join().unwrap(); }

assert_eq!(db.len::<Order>(), 4);
```

## The locking model

Each row in a table is protected by its own read/write lock. There is no
table-wide write lock.

- **Writes on different rows run in parallel.** Two `update` calls on
  different primary keys never contend.
- **Writes on the same row serialize.** Two `update` calls on the same key
  take turns on that row's write lock.
- **`get` releases its lock before returning.** The value you receive is a
  clone, not a borrow — it may be stale by the time you read it.
- **`for_each` holds the per-row read lock for the duration of your closure
  on that row.** Don't block inside it.

The dependency graph that tracks computed-column reads is also internally
concurrent (DashMap-backed), so registering dependencies on different cells
proceeds in parallel.

## The write-time fixpoint

When you write a row, Hamon locks that row for writes, recomputes every
computed column whose input changed on *the same row*, releases the lock,
then schedules cross-row recomputes on the background worker pool.

```text
db.update                       └─ write lock on row ─┘
    ├─ apply change
    ├─ same-row fixpoint (compute columns that read what changed)
    ├─ release write lock
    ├─ fire listener
    └─ enqueue cross-row dependents on worker pool
```

So: after `db.update(...)` returns, every *same-row* computed column on the
written row has its final value. Computed columns on *other* rows that
depended on the write will finish updating "soon" — but not synchronously.
If you need to observe a cross-row cascade, register a listener on the
dependent table (see [Reactive queries](./03-reactive-queries.md)).

## Reading from inside a computed column

Inside `fn pnl(row, ctx)`, calls like `ctx.lookup::<Price>(&…)` acquire a
read lock on the other row for as long as the compute runs. The compute
engine uses a recursive read lock internally so that a pending writer on the
same row cannot deadlock the compute.

You should still write compute functions as pure, cheap, and short. Blocking
I/O or long-running work inside a compute will hold up a worker thread.

## Shutdown

`build()` returns `(Hamon, HamonShutdown)`. Hold the `HamonShutdown` for as
long as you want the database to exist. Dropping it:

1. Signals the shared shutdown token.
2. Joins every feeder thread registered on the builder.

Background recompute workers are not explicitly joined; they wind down when
the final `Hamon` reference drops. If you drop `HamonShutdown` while cross-
row recomputes are still queued, some of them may not run. For graceful
shutdown that drains pending work, finish your writes before dropping
`HamonShutdown`.

## Multithread checklist

- Clone the `Hamon` via `Arc` before moving into threads.
- Use `update` with a single `(COLUMN, value)` tuple when you want the
  smallest write-lock scope; `replace` locks the row for the full copy.
- Don't call back into the same table's write methods from `for_each` —
  you'd be trying to upgrade a read guard you still hold.
- Panics in listeners are caught and the write continues; panics in a
  recompute worker call `std::process::abort()`.
