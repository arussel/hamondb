use hamondb::hamon_core::{Ctx, Row};
use hamondb::{Column, ColumnId, ComputedColumn, Index, HamonBuilder, Table};
use std::clone::Clone;
use std::collections::{HashMap, HashSet};

#[derive(Clone)]
struct SimpleTable {
    id: u64,
    val: String,
    three_val: String,
}

impl Table for SimpleTable {
    type PrimaryKey = u64;
}

const COL_ID: Column<SimpleTable, u64> = Column {
    id: 0,
    name: "SimpleTable::COL_ID",
    get: |r| r.id,
    set: |r, v| r.id = v,
};
const COL_VAL: Column<SimpleTable, String> = Column {
    id: 1,
    name: "SimpleTable::COL_VAL",
    get: |r| r.val.clone(),
    set: |r, v| r.val = v,
};

const COL_THREE_VAL: Column<SimpleTable, String> = Column {
    id: 2,
    name: "SimpleTable::COL_THREE_VAL",
    get: |r| r.three_val.clone(),
    set: |r, v| r.three_val = v,
};

impl SimpleTable {
    fn three_val(row: &Row<Self>, _ctx: &Ctx) -> String {
        row.get(COL_VAL) + "foobar"
    }
}

pub struct __slot_marker_marker_MYSLOT;
pub const MYSLOT: ::hamondb::Slot<__slot_marker_marker_MYSLOT, u64> =
    ::hamondb::Slot::new(concat!(module_path!(), "::CLOCK"));

#[test]
fn test() {
    let mut builder = HamonBuilder::new();
    let mut table_builder = builder.register::<SimpleTable, Column<SimpleTable, u64>>(COL_ID);
    // const INDEX_COLS: &[ColumnId] = ;
    const INDX: Index<SimpleTable, String> = Index::new(&[COL_VAL.id], 0);
    table_builder.register_index(INDX, |row| row.val.clone());
    table_builder.register_computed_column(ComputedColumn {
        column: COL_THREE_VAL,
        closure: Box::new(SimpleTable::three_val),
        computed_column_dependencies: HashSet::new(),
    });
    table_builder.done();
    builder.register_slot(MYSLOT, 0);
    let (db, shutdown) = builder.build();
    let v = db.slot(MYSLOT);
    assert_eq!(v, 0);
    let v = db.set_slot(MYSLOT, 1u64).unwrap();
    assert_eq!(v, ());
    let v = db.slot(MYSLOT);
    assert_eq!(v, 1);
    let result = db.insert(SimpleTable {
        id: 1,
        val: "foo".to_string(),
        three_val: "bar".to_string(),
    });
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), ());
    let rows = db.lookup_index::<SimpleTable, String>(INDX, &"foo".to_string());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, 1);
    assert_eq!(rows[0].three_val, "foofoobar");
}
