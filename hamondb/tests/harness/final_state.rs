use hamondb::{Hamon, Table};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use hamondb::test_support::schema::{Category, Order, Product, CURRENCY_LABEL, TOTAL_PRODUCT_COUNT};

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct FinalState {
    pub tables: BTreeMap<String, Vec<Value>>,
    pub slots: BTreeMap<String, Value>,
    pub resources: BTreeMap<String, Value>,
}

pub fn collect(hamon: &Hamon) -> FinalState {
    FinalState {
        tables: BTreeMap::from([
            (
                "category".to_string(),
                collect_table::<Category>(hamon, |v| v["id"].as_u64().unwrap()),
            ),
            (
                "product".to_string(),
                collect_table::<Product>(hamon, |v| v["id"].as_u64().unwrap()),
            ),
            (
                "order".to_string(),
                collect_table::<Order>(hamon, |v| v["id"].as_u64().unwrap()),
            ),
        ]),
        slots: BTreeMap::from([(
            "TOTAL_PRODUCT_COUNT".to_string(),
            serde_json::to_value(hamon.slot(TOTAL_PRODUCT_COUNT)).unwrap(),
        )]),
        resources: BTreeMap::from([(
            "CURRENCY_LABEL".to_string(),
            serde_json::to_value(&**hamon.resource(CURRENCY_LABEL)).unwrap(),
        )]),
    }
}

fn collect_table<T>(hamon: &Hamon, sort_key: impl Fn(&Value) -> u64) -> Vec<Value>
where
    T: Table + Serialize,
{
    let mut rows = Vec::new();
    hamon.for_each::<T>(|row| rows.push(serde_json::to_value(&row).unwrap()));
    rows.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
    rows
}
