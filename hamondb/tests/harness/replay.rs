use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::slice::from_raw_parts_mut;
use std::sync::Arc;
use hamondb::test_support::schema::{Category, Order, Product, CURRENCY_LABEL, TOTAL_PRODUCT_COUNT};

#[derive(Deserialize, Debug, strum::Display)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Insert {
        table: String,
        row: Value,
    },
    Upsert {
        table: String,
        row: Value,
    },
    Update {
        table: String,
        pk: Value,
        set: Value,
    },
    Replace {
        table: String,
        row: Value,
    },
    Delete {
        table: String,
        pk: Value,
    },
    SetSlot {
        slot: String,
        value: Value,
    },
    SetResource {
        resource: String,
        value: Value,
    },
    WaitForFires {
        count: usize,
    },
    AssertAbsent {
        table: String,
        pk: Value,
    },
    AssertPresent {
        table: String,
        pk: Value,
    },
    AssertSlot {
        slot: String,
        value: Value,
    },
    AssertResource {
        resource: String,
        value: Value,
    },
}
impl Op {
    /// Returns the table name for ops that reference a table.
    /// Unreachable for table-less ops (filtered out before this is called).
    fn table_name(&self) -> &str {
        match self {
            Op::Insert { table, .. }
            | Op::Upsert { table, .. }
            | Op::Update { table, .. }
            | Op::Replace { table, .. }
            | Op::Delete { table, .. }
            | Op::AssertAbsent { table, .. }
            | Op::AssertPresent { table, .. } => table,
            _ => unreachable!("non-table op reached table_name()"),
        }
    }
}

/// Reads events.json, strips comment entries, deserializes into Op.
pub fn load_events(path: &Path) -> Vec<Op> {
    let raw: Vec<Value> = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    raw.into_iter()
        .filter(|v| !is_comment(v))
        .map(|v| serde_json::from_value(v).unwrap())
        .collect()
}

/// `{"_comment": "..."}` entries are no-ops. Any object whose keys all start with
/// `_comment` is skipped — lets you temporarily disable an op by renaming its key.
fn is_comment(v: &Value) -> bool {
    v.as_object()
        .map(|m| m.keys().all(|k| k.starts_with("_comment")))
        .unwrap_or(false)
}

pub fn replay_events(hamon: &hamondb::Hamon, fire_log: &super::fire_log::FireLog, events: &[Op]) {
    for op in events {
        dispatch(hamon, fire_log, op);
    }
}

fn dispatch(hamon: &hamondb::Hamon, fire_log: &super::fire_log::FireLog, op: &Op) {
    // Table-less ops first — handle and return.
    match op {
        Op::WaitForFires { count } => return fire_log.wait_for(*count),
        Op::SetSlot { .. }
        | Op::SetResource { .. }
        | Op::AssertSlot { .. }
        | Op::AssertResource { .. } => return dispatch_global(hamon, op),
        _ => {}
    }

    use super::schema::*;
    match op.table_name() {
        "category" => dispatch_typed::<Category>(hamon, op),
        "product" => dispatch_typed::<Product>(hamon, op),
        "order" => dispatch_typed::<Order>(hamon, op),
        other => panic!("unknown table {other}"),
    }
}

/// Trait implemented per table in schema.rs. Puts the column-name-string →
/// column-handle match in exactly one place per table. See schema.rs for impls.
pub trait UpdateFromJson: hamondb::Table {
    fn apply_update_from_json(
        hamon: &hamondb::Hamon,
        pk: &Self::PrimaryKey,
        column: &str,
        value: &Value,
    );
}

fn dispatch_typed<T>(hamon: &hamondb::Hamon, op: &Op)
where
    T: hamondb::Table + serde::de::DeserializeOwned + UpdateFromJson + 'static,
    T::PrimaryKey: serde::de::DeserializeOwned,
{
    match op {
        Op::Insert { row, .. } => {
            hamon.insert::<T>(from_json(row)).unwrap();
        }
        Op::Upsert { row, .. } => {
            hamon.upsert::<T>(from_json(row)).unwrap();
        }
        Op::Replace { row, .. } => {
            hamon.replace::<T>(from_json(row)).unwrap();
        }
        Op::Delete { pk, .. } => {
            hamon.delete::<T>(&from_json::<T::PrimaryKey>(pk)).unwrap();
        }
        Op::Update { pk, set, .. } => {
            // Hamon::update accepts a single (Column<T, V>, V) per call. For patches with
            // multiple fields, we issue one call per field. Semantically different from an
            // atomic multi-column update, but hamon has no public multi-column builder today,
            // and the per-field cascade behaviour is what the scenarios assert against.
            let obj = set.as_object().expect("update.set must be an object");
            let pk_typed: T::PrimaryKey = from_json(pk);
            for (col_name, value) in obj {
                T::apply_update_from_json(hamon, &pk_typed, col_name, value);
            }
        }
        Op::AssertAbsent { pk, .. } => assert!(hamon.get::<T>(&from_json(pk)).is_err()),
        Op::AssertPresent { pk, .. } => assert!(hamon.get::<T>(&from_json(pk)).is_ok()),
        _ => unreachable!("non-table op reached dispatch_typed"),
    }
}

fn dispatch_global(hamon: &hamondb::Hamon, op: &Op) {
    match op {
        Op::SetSlot { slot, value } => match slot.as_str() {
            "TOTAL_PRODUCT_COUNT" => hamon
                .set_slot(TOTAL_PRODUCT_COUNT, from_json::<u64>(value))
                .unwrap(),
            _ => panic!("unknown slot {slot}"),
        },
        Op::SetResource { resource, value } => match resource.as_str() {
            "CURRENCY_LABEL" => {
                hamon.set_resource(CURRENCY_LABEL, Arc::<str>::from(from_json::<String>(value)))
            }
            _ => panic!("unknown resource {resource}"),
        },
        Op::AssertSlot { slot, value } => match slot.as_str() {
            "TOTAL_PRODUCT_COUNT" => {
                assert_eq!(from_json::<u64>(value), hamon.slot(TOTAL_PRODUCT_COUNT))
            }
            _ => panic!("unknown slot {slot}"),
        },
        Op::AssertResource { resource, value } => match resource.as_str() {
            "CCURRENCY_LABEL" => assert_eq!(
                from_json::<String>(value),
                hamon.resource(CURRENCY_LABEL).to_string()
            ),
            _ => panic!("unknown resource {resource}"),
        },
        _ => unreachable!("non-table op reached dispatch_global: {op}"),
    }
}

fn from_json<T: serde::de::DeserializeOwned>(v: &Value) -> T {
    serde_json::from_value(v.clone()).unwrap()
}
