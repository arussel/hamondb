use hamondb::hamon_core::{Ctx, Row};
use hamondb::{HamonBuilder, Table};
use hamondb_macro::{resource, slot};
use std::rc::Rc;

#[derive(Clone, Table)]
struct SimpleTable {
    #[primary_key]
    id: u64,
    #[index(VAL_IDX)]
    val: String,
    #[skip]
    ignored_field: u64,
    #[computed]
    three_val: String,
    #[computed(THREE_VAL)]
    dep_three_val: String,
    #[computed(DEP_THREE_VAL, THREE_VAL)]
    dep_2_level: String,
}

impl SimpleTable {
    fn three_val(row: &Row<Self>, _ctx: &Ctx) -> String {
        row.get(SimpleTable::VAL) + "foobar"
    }
    fn dep_three_val(row: &Row<Self>, _ctx: &Ctx) -> String {
        row.get(SimpleTable::THREE_VAL) + "level three"
    }
    fn dep_2_level(row: &Row<Self>, _ctx: &Ctx) -> String {
        row.get(SimpleTable::THREE_VAL) + "_" + &row.get(SimpleTable::DEP_THREE_VAL) + "dep_2_level"
    }
}

slot!(pub MYSLOT: u64);

resource!(pub MY_RESOURCE: String);

#[test]
fn test() {
    let mut builder = HamonBuilder::new();
    builder.register_table::<SimpleTable>();
    builder.register_slot(MYSLOT, 0);
    builder.register_resource(MY_RESOURCE, "foo".to_string());
    let (db, shutdown) = builder.build();
    let result = db.insert(SimpleTable {
        id: 1,
        val: "foo".to_string(),
        ignored_field: 1,
        three_val: "bar".to_string(),
        dep_three_val: "bar".to_string(),
        dep_2_level: "bar".to_string(),
    });
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), ());

    let length = db.len::<SimpleTable>();
    assert_eq!(length, 1);

    let mut values: Vec<String> = vec![];
    db.for_each::<SimpleTable>(|row| {
        values.push(row.val.clone());
    });
    assert_eq!(values, vec!["foo"]);
    let rows = db.lookup_index::<SimpleTable, String>(SimpleTable::VAL_IDX, &"foo".to_string());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, 1);
    assert_eq!(rows[0].val, "foo");
    assert_eq!(rows[0].ignored_field, 1);
    assert_eq!(rows[0].three_val, "foofoobar");
    assert_eq!(rows[0].dep_three_val, "foofoobarlevel three");
    assert_eq!(
        rows[0].dep_2_level,
        "foofoobar_foofoobarlevel threedep_2_level"
    );
    let v = db.slot(MYSLOT);
    assert_eq!(v, 0);
    let v = db.set_slot(MYSLOT, 1u64).unwrap();
    assert_eq!(v, ());
    let v = db.slot(MYSLOT);
    assert_eq!(v, 1);

    let r = db.resource(MY_RESOURCE);
    assert_eq!(r.to_string(), "foo".to_string());
    db.set_resource(MY_RESOURCE, "bar".to_string());
    let r = db.resource(MY_RESOURCE);
    assert_eq!(r.to_string(), "bar".to_string());
}
