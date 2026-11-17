# Requirement coverage

Each spec requirement listed with the test(s) that exercise it today, or
`none` if no test is known. Test paths are relative to the workspace root.
Test names are the Rust `#[test]` function names.

Legend:
- `L` = unit test inside `hamondb/src/*` (`cargo test -p hamondb --lib`)
- `I` = integration test under `hamondb/tests/`
- `M` = macro-expansion test in `hamondb/tests/hamon_macro_tests.rs`
- `T` = trybuild compile-fail test under `hamondb-macro/tests/compile_fail/`
- `S` = scenario test under `hamondb/tests/scenario_*`
- `E` = example in `hamondb/examples/` (not a test, but exercises the path)

## Schema

| ID              | Status | Tests                                                                |
|-----------------|--------|----------------------------------------------------------------------|
| HMN-SCHEMA-001  | partial | `T hamondb-macro/tests/compile_fail/` — covers only `#[skip]` combos |
| HMN-SCHEMA-002  | partial | `M hamon_macro_tests::test` (u64 PK)                                 |
| HMN-SCHEMA-003  | none   | No test asserts "two `#[primary_key]` is a compile error"            |
| HMN-SCHEMA-004  | covered | `M hamon_macro_tests::test` (VAL_IDX, THREE_VAL, etc. constants used) |
| HMN-SCHEMA-005  | none   | No test asserts column id stability                                  |
| HMN-SCHEMA-006  | covered | `M hamon_macro_tests::test` (`#[skip] ignored_field` present on row, no column handle used) |
| HMN-SCHEMA-007  | covered | `T compile_fail/skip_and_primary_key.rs`, `skip_and_index.rs`, `skip_and_computed.rs` |
| HMN-SCHEMA-008  | covered | `M hamon_macro_tests::test` (`VAL_IDX` + `lookup_index`)             |
| HMN-SCHEMA-009  | none   | No test asserts "wrong value type at `lookup_index` is a compile error" |
| HMN-SCHEMA-010  | covered | `M hamon_macro_tests::test` (compute methods)                        |
| HMN-SCHEMA-011  | covered | `M hamon_macro_tests::test` (initial `three_val = "bar"`, read back `"foofoobar"`) |
| HMN-SCHEMA-012  | partial | `M hamon_macro_tests::test` exercises `#[computed(DEP)]` positive path; no negative test for undeclared-dep panic |
| HMN-SCHEMA-013  | covered | `L builder::test::test_order_computed_columns_cycle`                 |
| HMN-SCHEMA-014  | covered | `M hamon_macro_tests::test` (`slot!(pub MYSLOT: u64);` + set/get)    |
| HMN-SCHEMA-015  | covered | `M hamon_macro_tests::test` (`resource!(pub MY_RESOURCE: String);`)  |
| HMN-SCHEMA-016  | none   | No test asserts "double register panics"                             |
| HMN-SCHEMA-017  | none   | No test asserts "register_listener before register_table panics"     |
| HMN-SCHEMA-018  | partial | Slot writes work (`M`), but no test exercises listener-on-SlotRow    |

## Computed columns

| ID            | Status | Tests                                                                 |
|---------------|--------|-----------------------------------------------------------------------|
| HMN-COMP-001  | covered | `M hamon_macro_tests::test`, `S scenario_basic_crud::basic_crud`     |
| HMN-COMP-002  | covered | `S scenario_cascade_same_row_chain::cascade_same_row_chain`          |
| HMN-COMP-003  | none   | No test asserts "replace that changes nothing fires no listener"     |
| HMN-COMP-004  | covered | `L builder::test::test_order_computed_columns`                        |
| HMN-COMP-005  | covered | `L builder::test::test_order_computed_columns_cycle`                  |
| HMN-COMP-006  | covered | `S scenario_cascade_same_row_chain::cascade_same_row_chain` (3 levels) |
| HMN-COMP-007  | covered | `E examples/liquidation.rs`; `I concurrent_stress::concurrent_position_updates_share_hot_read` |
| HMN-COMP-008  | partial | `I concurrent_stress::concurrent_register_remove_churn` exercises the queue; no targeted dedup test |
| HMN-COMP-009  | none   | Worker count not asserted by any test                                 |
| HMN-COMP-010  | covered | `M hamon_macro_tests::test`                                           |
| HMN-COMP-011  | covered | `I concurrent_stress::*` (Position reads Price via `ctx.lookup`); `E liquidation.rs` |
| HMN-COMP-012  | partial | `M hamon_macro_tests::test` sets up indexes; no test asserts bucket-watcher recompute |
| HMN-COMP-013  | covered | `I concurrent_stress::concurrent_register_remove_churn`              |
| HMN-COMP-014  | none   | No test asserts self-read panics                                      |
| HMN-COMP-015  | none   | No test asserts undeclared-computed-dep-read panics                   |
| HMN-COMP-016  | none   | No test asserts resource writes do not trigger compute               |
| HMN-COMP-017  | partial | `M hamon_macro_tests::test` reads slots but doesn't verify a slot-driven recompute |
| HMN-COMP-018  | covered | `I concurrent_stress::ctx_lookup_no_double_read_deadlock`            |
| HMN-COMP-019  | covered | `M hamon_macro_tests::test` (initial `three_val = "bar"`, computed overwrites) |
| HMN-COMP-020  | none   | No test asserts `ComputedColumnUpdate`                               |

## Reactivity

| ID             | Status | Tests                                                                |
|----------------|--------|----------------------------------------------------------------------|
| HMN-REACT-001  | covered | `S scenario_basic_crud::basic_crud`, `E liquidation.rs`             |
| HMN-REACT-002  | covered | `S scenario_cascade_same_row_chain::cascade_same_row_chain`          |
| HMN-REACT-003  | none   | No test asserts the no-op short-circuit                              |
| HMN-REACT-004  | covered | `S scenario_basic_crud::basic_crud` (delete fires `(Some, None)`)   |
| HMN-REACT-005  | covered | `S scenario_cascade_same_row_chain::cascade_same_row_chain`          |
| HMN-REACT-006  | partial | Scenarios verify listener logs, but no explicit multi-listener ordering test |
| HMN-REACT-007  | none   | No test writes from inside a listener                                 |
| HMN-REACT-008  | none   | No test asserts "listener panic does not propagate"                   |
| HMN-REACT-009  | covered | `M hamon_macro_tests::test` (`lookup_index` after insert)            |
| HMN-REACT-010  | partial | Scenarios exercise cross-table recomputes; no bucket-entry/exit targeted test |
| HMN-REACT-011  | covered | `M hamon_macro_tests::test`                                           |
| HMN-REACT-012  | covered | `M hamon_macro_tests::test`                                           |
| HMN-REACT-013  | partial | `I concurrent_stress::concurrent_position_updates_share_hot_read` touches this path; no dedicated dedup assertion |
| HMN-REACT-014  | none   | No test registers a feeder                                            |
| HMN-REACT-015  | none   | No test triggers a feeder panic                                       |
| HMN-REACT-016  | none   | No test asserts "HamonShutdown joins feeders"                        |

## Locking

| ID             | Status | Tests                                                                |
|----------------|--------|----------------------------------------------------------------------|
| HMN-LOCK-001   | implicit | All concurrent stress tests rely on it                             |
| HMN-LOCK-002   | covered | `I concurrent_stress::concurrent_position_updates_share_hot_read`   |
| HMN-LOCK-003   | implicit | Covered by DashMap + per-row RwLock semantics; no targeted test     |
| HMN-LOCK-004   | covered | `M hamon_macro_tests::test` (`get` after `insert`)                  |
| HMN-LOCK-005   | covered | `M hamon_macro_tests::test` (`for_each`)                            |
| HMN-LOCK-006   | implicit | Scenarios rely on it                                                |
| HMN-LOCK-007   | covered | `I concurrent_stress::concurrent_position_updates_share_hot_read`   |
| HMN-LOCK-008   | covered | `I concurrent_stress::concurrent_register_remove_churn`              |
| HMN-LOCK-009   | covered | `I concurrent_stress::ctx_lookup_no_double_read_deadlock`            |
| HMN-LOCK-010   | implicit | All compute scenarios rely on it                                     |
| HMN-LOCK-011   | none   | No test exercises cross-table deadlock scenarios                     |
| HMN-LOCK-012   | implicit | Scenarios rely on it                                                 |
| HMN-LOCK-013   | covered | `I concurrent_stress::*` (Arc<Hamon> across threads)                 |
| HMN-LOCK-014   | covered | `I concurrent_stress::concurrent_register_remove_churn`              |
| HMN-LOCK-015   | covered | `I concurrent_stress::concurrent_register_remove_churn`              |

## Errors

| ID            | Status | Tests                                                                 |
|---------------|--------|-----------------------------------------------------------------------|
| HMN-ERR-001   | implicit | Used via `assert!(result.is_ok())` in most tests                    |
| HMN-ERR-002   | none   | No test asserts `DuplicateKey`                                       |
| HMN-ERR-003   | none   | No test asserts `KeyNotFound` from `get`                             |
| HMN-ERR-004   | none   | No test asserts `KeyNotFound` from `replace`/`update`/`delete`       |
| HMN-ERR-005   | none   | No test asserts `ComputedColumnUpdate`                               |
| HMN-ERR-006   | none   | No test asserts `PKColumnUpdate`                                     |
| HMN-ERR-007   | none   | No test exercises `upsert` race → `ConcurrentModification`           |
| HMN-ERR-008   | covered | `L builder::test::test_order_computed_columns_cycle`                 |
| HMN-ERR-009   | n/a    | Dead variant                                                         |
| HMN-ERR-010   | n/a    | Dead variant                                                         |
| HMN-ERR-011   | none   | No test asserts unregistered-slot panic                              |
| HMN-ERR-012   | partial | Exercised at unit level (`L`); no integration test                   |
| HMN-ERR-013   | none   | No test exercises double-registration panics                         |
| HMN-ERR-014   | none   | No test exercises `register_listener` before `register_table`        |
| HMN-ERR-015   | none   | No test exercises self-read panic                                    |
| HMN-ERR-016   | none   | No test exercises undeclared-dep panic                               |
| HMN-ERR-017   | none   | No test exercises listener-panic isolation                           |
| HMN-ERR-018   | none   | No test exercises feeder-panic abort                                  |
| HMN-ERR-019   | none   | No test exercises worker-panic abort                                  |

## Summary

- Total requirements: **75**
  (Schema 18, Computed 20, Reactivity 16, Locking 15, Errors 19 — minus 2 `n/a`)
- Covered or implicitly covered: **44**
- Partial: **8**
- None: **23**
