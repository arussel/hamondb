# Locking and concurrency

Rules describing lock granularity, lock scope per operation, and the
guarantees callers can rely on in a multi-threaded setting. Hamon aims for
row-level concurrency: different rows never contend, different cells on
different rows in the dep graph rarely contend.

### HMN-LOCK-001 — Per-row RwLock
Every row of every table is wrapped in its own `parking_lot::RwLock<T>`.
There is no table-level lock protecting the data of all rows.

### HMN-LOCK-002 — Writers on distinct rows do not serialize
`insert`, `update`, `replace`, and `delete` on different primary keys
acquire distinct row locks and proceed concurrently.

### HMN-LOCK-003 — Writers on the same row serialize
Two concurrent `update` (or `replace` or `delete`) calls on the same key
serialize on that row's write lock. There is no ordering guarantee between
contending writers (parking_lot's writer-biased fairness decides).

### HMN-LOCK-004 — `get` releases its lock before returning
`db.get` takes a read lock on the matching row, clones the row, drops the
lock, and returns the clone. The returned value may be stale by the time
the caller consumes it.

### HMN-LOCK-005 — `for_each` holds the lock during the closure
`db.for_each` iterates every row, holding the read lock on the current row
while the user's closure runs for that row, then releasing before moving
on. Long-running work in the closure extends the lock hold time.

### HMN-LOCK-006 — Write lock scope of a direct mutation
A direct mutation (`insert` / `replace` / `update` / `delete`) holds the
row's write lock from acquisition until *after* the same-row fixpoint has
run, index maintainers have updated, and dep-graph edges have been
registered. The lock is released *before* the listener fires and before
cross-row recomputes are enqueued.

### HMN-LOCK-007 — Dependency graph is lock-free at the outer level
The `DepGraph` (uses / used_by / uses_index / used_by_index) is backed by
`DashMap`. All methods take `&self`; there is no outer `RwLock`. Shard-
level locks inside DashMap provide thread safety, so disjoint cells never
contend.

### HMN-LOCK-008 — `register` self-heals stale back-edges
`DepGraph::register(cell, new_reads)` overwrites the forward edge
atomically and diffs against the previous set so only changed back-edges
are touched. The bidirectional invariant
`D ∈ uses[C] ⇔ C ∈ used_by[D]` is maintained eventually; momentary
inconsistency with a concurrent `remove` is cleaned up on the next
`register`.

### HMN-LOCK-009 — Recursive read lock inside `Ctx::lookup`
`Ctx::lookup` and `Ctx::lookup_index` acquire two read locks on the looked-
up row: one owned by the returned `Row`, one stashed in `Ctx::held_locks`.
The second acquisition is recursive (`read_arc_recursive`) so a writer
queued between the two calls does not deadlock the compute under
parking_lot's writer-biased fairness.

### HMN-LOCK-010 — Compute functions hold a write lock on their own row
A computed column is evaluated under the write lock of the row it belongs
to, acquired by the enclosing `insert` / `replace` / `update` / `delete` or
by the recompute worker.

### HMN-LOCK-011 — No global lock ordering across tables
Hamon does *not* impose a cross-table lock order. A compute function that
holds a write lock on `Row<A>` and reads `Row<B>` via `ctx.lookup` can, in
principle, deadlock with another thread that writes `Row<B>` and reads
`Row<A>`.

> OPEN(HMN-LOCK-011): A cross-table write / lookup cycle between two
> distinct tables is not exercised by any current test. Is this a gap
> (potential deadlock that users must avoid) or not applicable because
> computes are read-only on other rows and the recursive-read fix
> (HMN-LOCK-009) covers every realistic scenario?

### HMN-LOCK-012 — Writes see the row atomically
A reader that acquires a row's read lock after a writer releases it sees
the *full* post-write state: all regular columns set, all same-row
computed columns evaluated, and all indexes updated. There is no observable
intermediate state.

### HMN-LOCK-013 — Hamon is `Send + Sync`
`Hamon` is cheap to clone through an `Arc` and may be shared across any
number of threads. Every method on `Hamon` takes `&self`.

### HMN-LOCK-014 — Recompute enqueueing is lock-free
`RecomputeQueue::enqueue` uses a DashMap keyed by `(TypeId, row_id)` for
deduplication plus a crossbeam unbounded channel for worker dispatch.
Enqueueing from multiple threads does not block.

### HMN-LOCK-015 — Insert write-lock acquisition is race-free against concurrent inserts of the same key
`TableStore::insert` uses a DashMap entry-level occupancy check: the first
concurrent insert on a given key observes `Vacant` and succeeds, every
other observes `Occupied` and returns `Duplicate`. No inserter ever observes
a partially-initialized row.
