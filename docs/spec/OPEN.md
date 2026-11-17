# Open spec questions

A checklist of unresolved spec questions. Each entry is a spec requirement
whose behaviour the current code exhibits but whose *intent* is not
settled — or an orphan code path that the spec cannot yet commit to.
Resolve one at a time; either tighten the spec, delete the dead code, or
change the behaviour and the spec together.

Each entry links to the requirement whose paragraph has the inline
`OPEN(…)` marker.

## Schema

- [ ] **OPEN(HMN-SCHEMA-005)** — Column `id` is the field's positional
  index. Is this a public contract, or an implementation detail that could
  later swap to a stable hash of the field name?
  *Code:* `hamondb-macro/src/table.rs:97-98`.
- [ ] **OPEN(HMN-SCHEMA-018)** — `set_slot` is implemented as `update` on a
  one-row table. Is the layering (slot writes go through the full fixpoint
  / listener machinery) intentional, or an incidental consequence of
  reuse?
  *Code:* `hamondb/src/hamon_core.rs:512`.

## Computed columns

- [ ] **OPEN(HMN-COMP-009)** — Recompute worker count is hard-coded at 4.
  Is this part of the public contract, or should it be configurable?
  *Code:* `hamondb/src/builder.rs:338`.
- [ ] **OPEN(HMN-COMP-016)** — `ctx.resource` reads are not tracked, so a
  computed column that reads a resource will not re-fire when
  `set_resource` runs. Is this intended (resources are "pre-aggregate
  state, shouldn't flow through the dep graph"), or an omission?
  *Code:* `hamondb/src/hamon_core.rs:686-699`. *Test coverage:* none.

## Reactivity

- [ ] **OPEN(HMN-REACT-005)** — The async recompute path unconditionally
  fires listeners, even when `fixpoint_compute` produces an identical row.
  Direct mutations short-circuit in the same situation (HMN-REACT-003).
  Should recompute align with direct mutation, or is the extra fire
  wanted?
  *Code:* `hamondb/src/store.rs:150`. *Test coverage:* none for
  "recompute produces identical row → listener fires".
- [ ] **OPEN(HMN-REACT-015)** — Panic policies differ across paths:
  listeners *catch-and-continue*, feeders *abort*, recompute workers
  *abort*. Is this three-way split intentional?
  *Code:* `hamondb/src/builder.rs:367`, `hamondb/src/store.rs:294`,
  `hamondb/src/recompute_queue.rs:50`.
- [ ] **OPEN(HMN-REACT-016)** — `HamonShutdown::drop` joins feeders but
  not recompute workers. Pending recomputes may be dropped on shutdown.
  Add a drain API, or document the trade-off?

## Locking

- [ ] **OPEN(HMN-LOCK-011)** — Cross-table deadlock risk if two threads
  take write locks on different tables while cross-looking-up. No current
  test covers this. Is it a real risk, or ruled out because computes read
  with recursive read locks and user writes are atomic per row?

## Errors

- [ ] **OPEN(HMN-ERR-009)** / **OPEN(HMN-ERR-010)** —
  `TableNotRegistered` and `ComputedColumnDependencyError` are declared
  on `HamonError` but never returned by the public API. Remove them,
  repurpose them, or route the current panics through them?
