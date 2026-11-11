use std::sync::Arc;
use serde::{Deserialize, Serialize};
use hamondb_macro::{resource, slot, Table};
use crate::hamon_core::{Ctx, Row};

#[derive(Table, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Category {
    #[primary_key]
    pub id: u64,
    pub markup_bps: u32,
}

#[derive(Table, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Product {
    #[primary_key]
    pub sku: u64, //0

    #[index(CAT_ID_IDX)]
    pub category_id: u64,//1

    pub price_cents: u64,//2
    pub quantity: u32,//3

    #[computed(PRICE_CENTS, CATEGORY_ID)]
    #[serde(default)]
    pub effective_price_cents: u64,//4

    #[computed(QUANTITY)]
    #[serde(default)]
    pub needs_restock: bool,//5

    #[computed(NEEDS_RESTOCK, QUANTITY)]
    #[serde(default)]
    pub restock_urgency: u8,//6
}

impl Product {
    fn effective_price_cents(row: &Row<Self>, ctx: &Ctx) -> u64 {
        let price = row.get(Self::PRICE_CENTS);
        let cat_id = row.get(Self::CATEGORY_ID);
        let markup = ctx
            .lookup::<Category>(&cat_id)
            .map(|c| c.get(Category::MARKUP_BPS))
            .unwrap_or(0);
        price * (10_000 + markup as u64) / 10_000
    }

    fn needs_restock(row: &Row<Self>, _ctx: &Ctx) -> bool {
        row.get(Self::QUANTITY) < 10
    }

    fn restock_urgency(row: &Row<Self>, _ctx: &Ctx) -> u8 {
        if !row.get(Self::NEEDS_RESTOCK) {
            return 0;
        }
        let q = row.get(Self::QUANTITY);
        if q == 0 {
            10
        } else if q < 5 {
            5
        } else {
            1
        }
    }
}

#[derive(Table, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Order {
    #[primary_key]
    id: u64,

    #[index(PRODUCT_ID_IDX)]
    product_sku: u64,

    qty: u32,
    status: OrderStatus,

    #[computed]
    total_cents: u64,
}

impl Order {
    fn total_cents(row: &Row<Self>, ctx: &Ctx) -> u64 {
        let qty = row.get(Self::QTY) as u64;
        let sku = row.get(Self::PRODUCT_SKU);
        let unit_price = ctx
            .lookup::<Product>(&sku)
            .map(|p| p.get(Product::EFFECTIVE_PRICE_CENTS))
            .unwrap_or(0);
        qty * unit_price
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum OrderStatus {
    Pending,
    Fulfilled,
    Cancelled,
}

slot!(pub TOTAL_PRODUCT_COUNT: u64);
resource!(pub CURRENCY_LABEL: Arc<str>);
