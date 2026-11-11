use hamondb::test_support::schema::{Category, Order, OrderStatus, Product};
use super::replay::UpdateFromJson;

impl UpdateFromJson for Category {
    fn apply_update_from_json(
        hamon: &hamondb::Hamon,
        pk: &u64,
        column: &str,
        value: &serde_json::Value,
    ) {
        match column {
            "markup_bps" => {
                hamon
                    .update::<Self>(pk, (Self::MARKUP_BPS, from_json::<u32>(value)))
                    .unwrap();
            }
            other => panic!("Category has no updatable column named `{other}`"),
        }
    }
}

impl UpdateFromJson for Product {
    fn apply_update_from_json(
        hamon: &hamondb::Hamon,
        pk: &u64,
        column: &str,
        value: &serde_json::Value,
    ) {
        match column {
            "category_id" => {
                hamon
                    .update::<Self>(pk, (Self::CATEGORY_ID, from_json::<u64>(value)))
                    .unwrap();
            }
            "price_cents" => {
                hamon
                    .update::<Self>(pk, (Self::PRICE_CENTS, from_json::<u64>(value)))
                    .unwrap();
            }
            "quantity" => {
                hamon
                    .update::<Self>(pk, (Self::QUANTITY, from_json::<u32>(value)))
                    .unwrap();
            }
            other => panic!("Product has no updatable column named `{other}`"),
        }
    }
}

impl UpdateFromJson for Order {
    fn apply_update_from_json(
        hamon: &hamondb::Hamon,
        pk: &u64,
        column: &str,
        value: &serde_json::Value,
    ) {
        match column {
            "product_sku" => {
                hamon
                    .update::<Self>(pk, (Self::PRODUCT_SKU, from_json::<u64>(value)))
                    .unwrap();
            }
            "qty" => {
                hamon
                    .update::<Self>(pk, (Self::QTY, from_json::<u32>(value)))
                    .unwrap();
            }
            "status" => {
                hamon
                    .update::<Self>(pk, (Self::STATUS, from_json::<OrderStatus>(value)))
                    .unwrap();
            }
            other => panic!("Order has no updatable column named `{other}`"),
        }
    }
}

fn from_json<T: serde::de::DeserializeOwned>(v: &serde_json::Value) -> T {
    serde_json::from_value(v.clone()).unwrap()
}
