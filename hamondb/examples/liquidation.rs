//! Liquidation example: a sketch of a trading bot that uses a cross-table
//! computed column and a listener to react to price updates.
//!
//! Run with:
//!
//!     cargo run --example liquidation -p hamondb

use hamondb::hamon_core::{Ctx, Row};
use hamondb::{HamonBuilder, Table};

const SOL: u32 = 1;
const MAX_LOSS: i64 = 100_000;

fn liquidate(id: u64) {
    println!("liquidating position {id}");
}

#[derive(Clone, Table)]
struct Price {
    #[primary_key]
    symbol: u32,
    mark: i64,
}

#[derive(Clone, Table)]
struct Position {
    #[primary_key]
    id: u64,
    symbol: u32,
    size: i64,
    entry: i64,
    #[computed]
    pnl: i64,
}

impl Position {
    fn pnl(row: &Row<Self>, ctx: &Ctx) -> i64 {
        let entry = row.get(Self::ENTRY);
        let mark = ctx
            .lookup::<Price>(&row.get(Self::SYMBOL))
            .map(|p| p.get(Price::MARK))
            .unwrap_or(entry);
        (mark - entry) * row.get(Self::SIZE)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = HamonBuilder::new();
    builder.register_table::<Price>();
    builder
        .register_table::<Position>()
        .register_listener::<Position>(|id, _old, new| {
            if let Some(p) = new {
                if p.pnl < -MAX_LOSS {
                    liquidate(*id);
                }
            }
        });
    let (db, _shutdown) = builder.build();

    db.insert(Price { symbol: SOL, mark: 14_000 })?;
    db.insert(Position {
        id: 1,
        symbol: SOL,
        size: 100,
        entry: 14_100,
        pnl: 0,
    })?;

    // Your price feed only writes prices. For each tick, every SOL position's
    // PnL is recomputed. Any that cross the limit trigger the listener.
    db.update::<Price>(&SOL, (Price::MARK, 14_250))?;
    db.update::<Price>(&SOL, (Price::MARK, 13_800))?;
    db.update::<Price>(&SOL, (Price::MARK, 13_000))?; // → liquidate(1) runs

    Ok(())
}
