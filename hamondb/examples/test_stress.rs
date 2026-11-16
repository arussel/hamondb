use hamondb::hamon_core::{Ctx, Row};
use hamondb::{HamonBuilder, Table};

const SOL: u32 = 1;

#[derive(Clone, hamondb::Table)]
struct Price { #[primary_key] symbol: u32, mark: i64 }

#[derive(Clone, hamondb::Table)]
struct Position {
    #[primary_key] id: u64,
    symbol: u32, size: i64, entry: i64,
    #[computed] pnl: i64,
}

impl Position {
    fn pnl(row: &Row<Self>, ctx: &Ctx) -> i64 {
        let entry = row.get(Self::ENTRY);
        let mark = ctx.lookup::<Price>(&row.get(Self::SYMBOL))
            .map(|p| p.get(Price::MARK)).unwrap_or(entry);
        (mark - entry) * row.get(Self::SIZE)
    }
}

fn main() {
    let mut builder = HamonBuilder::new();
    builder.register_table::<Price>();
    builder.register_table::<Position>();
    let (db, _shutdown) = builder.build();
    db.insert(Price { symbol: SOL, mark: 14_000 }).unwrap();
    db.insert(Position { id: 1, symbol: SOL, size: 100, entry: 14_100, pnl: 0 }).unwrap();
    eprintln!("[{}] starting loop", std::process::id());
    for i in 0..50 {
        for mark in [14_250i64, 13_800, 13_000] {
            db.update::<Price>(&SOL, (Price::MARK, mark)).unwrap();
        }
        if i % 10 == 0 { eprintln!("  iter {}", i); }
    }
    println!("done all");
}
