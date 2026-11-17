# Errors and panics

Rules for each `HamonError` variant and each panic path exposed by the
public API. Panics are listed alongside errors because some
programmer-error conditions are reported as panics rather than `Err(…)`.

### HMN-ERR-001 — `HamonError` is `Clone + Debug + Eq + PartialEq + thiserror::Error`
`HamonError` derives these traits. Callers may compare values with `==`
and format them with `{:?}`.

### HMN-ERR-002 — `DuplicateKey`
Returned by `db.insert::<T>(row)` when the row's primary key is already
present in `T`'s store. The existing row is unchanged.

### HMN-ERR-003 — `KeyNotFound` from `get`
Returned by `db.get::<T>(&pk)` when no row with that primary key exists.

### HMN-ERR-004 — `KeyNotFound` from `replace` / `update` / `delete`
Returned by `db.replace`, `db.update`, or `db.delete` when no row with
that primary key exists.

### HMN-ERR-005 — `ComputedColumnUpdate`
Returned by `db.update::<T>(&pk, (COL, v))` when `COL` is a `#[computed]`
column. The row is not modified.

### HMN-ERR-006 — `PKColumnUpdate`
Returned by `db.update::<T>(&pk, (COL, v))` when `COL` is the primary-key
column. The row is not modified.

### HMN-ERR-007 — `ConcurrentModification` from `upsert`
Returned by `db.upsert::<T>(row)` when the row was present at the initial
lookup but was deleted by another thread before the replace fired.
`upsert` does not retry internally.

### HMN-ERR-008 — `DependencyCycle` is thrown only internally
`HamonError::DependencyCycle(Vec<ColumnId>)` is returned from
`order_computed_columns` but it is not surfaced to callers — `build()`
converts it into a panic (HMN-ERR-012). Users will not observe this
variant through the public API today.

### HMN-ERR-009 — `TableNotRegistered` is unused by the public API
The public API panics (not errors) when a table is not registered. The
`TableNotRegistered` variant exists on the enum but is not returned by
any public method as of this spec version.

> OPEN(HMN-ERR-009): `TableNotRegistered` and `ComputedColumnDependencyError`
> are declared on `HamonError` but not returned by any public code path
> I surveyed. Should they be removed, or repurposed?

### HMN-ERR-010 — `ComputedColumnDependencyError` is unused by the public API
Same status as HMN-ERR-009 — declared but not returned by any public
method.

### HMN-ERR-011 — Panic: slot / resource not registered
`db.slot(&s)` / `db.set_slot(&s, …)` / `db.resource(&r)` /
`db.set_resource(&r, …)` panic if the corresponding slot or resource was
not registered on the builder. The panic message includes the slot /
resource name.

### HMN-ERR-012 — Panic: computed-column cycle at build
`HamonBuilder::build()` panics with
`computed column cycle detected: [...]` when the `#[computed(...)]`
declarations across the registered tables form a cycle.

### HMN-ERR-013 — Panic: double registration
`register_table`, `register_slot`, `register_resource`, and
`register_feeder` panic if the same `T` (resp. slot / resource / feeder
name) is registered twice.

### HMN-ERR-014 — Panic: register_listener before register_table
`register_listener::<T>` panics with `table not registered` when `T` has
not been registered yet.

### HMN-ERR-015 — Panic: computed function self-read
A compute function that calls `row.get(Self::<SELF>)` for the column
being computed panics with `computed column X cannot read itself`.

### HMN-ERR-016 — Panic: computed function reads undeclared computed dep
A compute function that reads another `#[computed]` column without listing
it in `#[computed(...)]` panics with
`computed column X read undeclared computed column Y`.

### HMN-ERR-017 — Listener panics do not propagate
A panic inside a listener is caught and logged; the write that triggered
it still succeeds.

### HMN-ERR-018 — Feeder panics abort
A panic inside a feeder closure is caught, logged, and followed by
`std::process::abort()` — the process terminates immediately.

### HMN-ERR-019 — Recompute worker panics abort
A panic inside a recompute callback is caught, logged to stderr, and
followed by `std::process::abort()`.
