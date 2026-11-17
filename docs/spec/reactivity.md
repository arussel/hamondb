# Reactivity

Rules governing listener firing, index maintenance, and the observable
ordering of change notifications. For the mechanics of what causes a
*recompute*, see `computed.md`; this document is about what reaches the
outside world.

### HMN-REACT-001 — Listener fires on successful insert
A listener registered for table `T` fires exactly once per successful
`db.insert::<T>` call, with `(pk, None, Some(&new))`.

### HMN-REACT-002 — Listener fires on direct mutation when regular columns changed
`db.update` or `db.replace` that modifies at least one regular (non-computed)
column fires the listener once with `(pk, Some(&old), Some(&new))` after the
row's write lock is released.

### HMN-REACT-003 — Listener does not fire on no-op direct mutation
A `db.update` or `db.replace` call that does not change any regular column
returns `Ok(())` without firing the listener. Computed-only differences do
not count.

### HMN-REACT-004 — Listener fires on delete
A listener fires exactly once per successful `db.delete`, with
`(pk, Some(&old), None)`.

### HMN-REACT-005 — Listener fires on async recompute
When a row is modified by a background recompute (its dependencies changed
on another row), the listener registered for its table fires with
`(pk, Some(&old), Some(&new))` on the worker thread, after the per-row
write lock is released.

> OPEN(HMN-REACT-005): The async recompute path calls `fire_lister`
> unconditionally in `store.rs:150`, even when `fixpoint_compute` produces
> an identical row. Should it short-circuit the way direct-mutation does
> (HMN-REACT-003)? No test covers "recompute produces identical row →
> listener fires".

### HMN-REACT-006 — Listener invocation order per write
When a single write produces multiple listener firings (on the same table),
they run in the order the listeners were registered on the builder.

### HMN-REACT-007 — Direct writes fire listeners after unlock
For direct mutations, the row's write lock is released before the listener
runs. Listeners may therefore issue further write operations against the
same row without self-deadlocking.

### HMN-REACT-008 — Listener panics are isolated
A panic inside a listener is caught by `catch_unwind`; the error is written
to stderr. The surrounding write still completes successfully and
subsequent listeners still run.

### HMN-REACT-009 — Index maintained on write
Secondary indexes are updated in-place during `insert`, `replace`, `update`,
and `delete`, while the row's write lock is held and before any listener
fires. `lookup_index` after a successful write observes the new value
immediately (under the normal happens-before guarantees of the row lock).

### HMN-REACT-010 — Index bucket watchers enqueue on bucket entry/exit
When a row enters or leaves an index bucket that a computed column reads
via `ctx.lookup_index`, the dependent computed cell is enqueued on the
recompute queue. Rows staying in a bucket on an unrelated column change do
not cause bucket-watcher enqueueing.

### HMN-REACT-011 — `lookup_index` returns cloned rows
`db.lookup_index::<T, V>(idx, &value)` returns `Vec<T>` whose elements are
clones taken under per-row read locks. The order of the vector is
unspecified.

### HMN-REACT-012 — `for_each` is a snapshot per row, not across rows
`db.for_each` iterates all rows of a table, holding the read lock on the
current row while the closure runs for that row, then moving on. Rows read
earlier may have been mutated by the time rows read later are visited.

### HMN-REACT-013 — Computed-column listener fires are deduplicated per row across a batch
Enqueued recomputes are coalesced per `(TypeId::<T>, row_id)` by the queue
before being processed. If multiple cross-row cascades land on the same row
before the worker picks it up, the listener fires once per processed batch,
not once per enqueued column.

### HMN-REACT-014 — Feeder threads run `register_feeder` closures
Each feeder registered via `register_feeder` is spawned on its own thread
at `build()` time. The feeder receives `&Hamon` and a `ShutdownToken`.
Feeders are joined when `HamonShutdown` is dropped.

### HMN-REACT-015 — Feeder panics abort the process
If a feeder closure panics, the worker catches the panic, logs it via
`tracing::error!`, and calls `std::process::abort()`.

> OPEN(HMN-REACT-015): Feeder-panic policy is `abort`, but listener-panic
> policy (HMN-REACT-008) is `catch and continue`, and recompute-worker
> panic policy is also `abort`. The three paths are inconsistent — is this
> intentional? Code: `hamondb/src/builder.rs:367`,
> `hamondb/src/store.rs:294`, `hamondb/src/recompute_queue.rs:50`.

### HMN-REACT-016 — `HamonShutdown::drop` joins feeders only
Dropping `HamonShutdown` triggers the shared `ShutdownToken` and joins the
feeder threads. The recompute queue workers are not explicitly joined;
they exit when the final `Arc<RecomputeQueue>` is dropped.

> OPEN(HMN-REACT-016): If `HamonShutdown` is dropped while cross-row
> recomputes are still queued, some may not run before worker threads
> exit. No API currently provides "drain pending recomputes". Is this
> acceptable or a gap?
