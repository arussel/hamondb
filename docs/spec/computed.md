# Computed columns

Rules governing when a computed column is evaluated, what it may read, how
its dependencies are tracked, and what the engine guarantees about the order
and completeness of recomputation.

### HMN-COMP-001 — Computed columns are evaluated on insert
On a successful `insert`, every computed column on the new row is evaluated
before the row is made visible, and the returned value is written into the
row.

### HMN-COMP-002 — Computed columns are evaluated on same-row write
On `replace` or `update` where at least one regular column changes, the
engine evaluates all computed columns whose declared or tracked dependencies
intersect the changed cells, iterated to fixpoint on the same row before the
new row becomes visible.

### HMN-COMP-003 — No-op write short-circuit
On `replace` / `update` where *no* regular column differs from the current
row, no computed column is re-evaluated, the write lock is released, and no
listener fires.

### HMN-COMP-004 — Topological order among computed columns
At `build()` time, computed columns are topologically sorted by their
declared `#[computed(...)]` dependency graph. Fixpoint computation evaluates
them in this order within each iteration.

### HMN-COMP-005 — Cycles are rejected at build time
If the declared `#[computed(...)]` dependencies form a cycle, `build()`
panics (`computed column cycle detected`). Cycles are never caught at
runtime; the build-time check is the only one.

### HMN-COMP-006 — Same-row fixpoint terminates
Same-row recompute iterates until no computed column whose value changed
has a dependent that still needs re-evaluation. Because dependencies are a
DAG (HMN-COMP-005), the fixpoint terminates in at most `height(DAG)`
iterations.

### HMN-COMP-007 — Cross-row recomputes run asynchronously
When a write changes a cell that is read by a computed column on another
row (same or different table), the dependent cells are enqueued on a shared
background queue and processed by a worker pool. The originating write
returns before these recomputes run.

### HMN-COMP-008 — Recompute dedup per `(table, row)`
The recompute queue coalesces enqueued recomputes by `(TypeId::<T>, row_id)`
key; if multiple columns of the same row are enqueued before the row is
processed, the worker evaluates them together under a single write lock.

### HMN-COMP-009 — Worker pool size
The recompute pool is created with 4 workers in `builder::build()`.

> OPEN(HMN-COMP-009): Is the worker count (`4` at `hamondb/src/builder.rs:338`)
> a part of the public contract, or an implementation detail? Not configurable
> via builder API today.

### HMN-COMP-010 — Compute may read same-row cells
Inside a compute function, `row.get(Self::COL)` returns the value of column
`COL` on the row being computed. The read is recorded as a dependency so
future writes to `COL` trigger re-evaluation.

### HMN-COMP-011 — Compute may read other rows via `ctx.lookup`
`ctx.lookup::<U>(&key)` returns `Option<Row<U>>` for a point lookup on
table `U`. Every call records the looked-up row's cells (that are then read
via `row.get`) as dependencies of the computing cell.

### HMN-COMP-012 — Compute may read through an index via `ctx.lookup_index`
`ctx.lookup_index::<U, V>(idx, &value)` returns the matching rows of `U`
and records the index bucket (identified by `(TypeId::<U>, index_id, hash(value))`)
as a bucket-watched dependency of the computing cell. The computed cell
re-fires when a row enters or leaves that bucket.

### HMN-COMP-013 — Dependency tracking is per-call
The dependency set of a computed cell is replaced on every evaluation. If a
compute call stops reading a cell it read previously, the stale edge is
removed; if it starts reading a new cell, the new edge is added.

### HMN-COMP-014 — Self-read panics
A compute function that calls `row.get(Self::SELF_COLUMN)` for the column
it is currently computing panics with
`computed column X cannot read itself`.

### HMN-COMP-015 — Undeclared computed-column read panics
A compute function that reads a *different* computed column without listing
that column in its `#[computed(...)]` attribute panics with `computed
column X read undeclared computed column Y`. This is required to make the
topological sort (HMN-COMP-004) correct.

### HMN-COMP-016 — `ctx.resource` reads are not tracked
`ctx.resource(…)` returns the current resource value. The read is not
registered as a dependency. A computed column that reads a resource will
not automatically re-fire when `set_resource` is called.

> OPEN(HMN-COMP-016): This is observable behaviour, but is it an intended
> design decision (resources are "pre-aggregate state, shouldn't flow
> through the dep graph") or an omission? Code:
> `hamondb/src/hamon_core.rs:686-699`. No test covers "resource write
> triggers computed recompute".

### HMN-COMP-017 — `ctx.slot` reads *are* tracked
`ctx.slot(…)` records the slot's cell as a dependency of the computing
column. Calling `set_slot` triggers the dependent computed columns.

### HMN-COMP-018 — Compute runs under the row's write lock
During evaluation, the row being computed is write-locked. Looked-up rows
(via `ctx.lookup` / `ctx.lookup_index`) are held under recursive read locks
for the duration of the compute.

### HMN-COMP-019 — Computed values set at insert-time are ignored
The initial value placed in a `#[computed]` field on `insert` is overwritten
by the compute's return value before the row becomes visible.

### HMN-COMP-020 — `update` on a computed column is rejected
`db.update::<T>(&key, (Self::COMPUTED_COL, v))` returns
`Err(HamonError::ComputedColumnUpdate)`. Computed columns can only change
by their function being re-evaluated.
