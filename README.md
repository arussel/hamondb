# Hamon

**Write the logic. Hamon keeps the state correct.**

Every app that keeps live state in memory ends up writing the same plumbing:
indexes to keep in sync, derived values to recompute, change notifications to wire up.
It drifts out of sync, and before long it outweighs the code you actually care about.

Hamon handles all of it. You declare your data as plain Rust structs, write each
derived value as a function, and Hamon tracks what depends on what, recomputing and
notifying whenever anything changes. You write only your core logic, so there is
far less code to maintain.

```rust
// --- Data: plain structs ---
#[derive(Clone, Table)]
struct Price {
    #[primary_key] symbol: u32,
    mark: i64,
}

#[derive(Clone, Table)]
struct Position {
    #[primary_key] id: u64,
    symbol: u32,
    size: i64,
    entry: i64,
    #[computed] pnl: i64,
}

// --- Logic: one function per derived value, one listener per reaction ---
impl Position {
    fn pnl(row: &Row<Self>, ctx: &Ctx) -> i64 {
        let entry = row.get(Self::ENTRY);
        let mark = ctx.lookup::<Price>(&row.get(Self::SYMBOL))
            .map(|p| p.get(Price::MARK))
            .unwrap_or(entry);
        (mark - entry) * row.get(Self::SIZE)
    }
}

let mut builder = HamonBuilder::new();
builder.register_table::<Price>();
builder.register_table::<Position>()
    .register_listener::<Position>(|id, _old, new| {
        if let Some(p) = new { if p.pnl < -MAX_LOSS { liquidate(*id) } }
    });
let (db, _shutdown) = builder.build();

// --- Feed: write data in, everything downstream happens automatically ---
db.insert(Price { symbol: SOL, mark: 14_000 })?;
db.insert(Position { id: 1, symbol: SOL, size: 100, entry: 14_100, pnl: 0 })?;

// Your price feed only writes prices. For each tick, every SOL position's
// PnL is recomputed. Any that cross the limit trigger the listener.
db.update::<Price>(&SOL, (Price::MARK, 14_250))?;
db.update::<Price>(&SOL, (Price::MARK, 13_800))?;
db.update::<Price>(&SOL, (Price::MARK, 13_000))?; // → liquidate(1) runs
```

| You write | Hamon does |
|---|---|
| Structs | Storage, primary keys, secondary indexes |
| One function per derived value | Dependency tracking, recomputation, cross-table cascades |
| What to do when something changes | Change detection and notification |

## Install

```toml
[dependencies]
hamondb = { git = "https://github.com/arussel/hamondb" }
```

Not yet published to crates.io.

## Learn more

- [`hamondb/examples/liquidation.rs`](hamondb/examples/liquidation.rs) — the
  snippet above as a runnable example (`cargo run --example liquidation -p hamondb`).
- [`docs/guide.md`](docs/guide.md) — reference for every concept: tables,
  indexes, computed columns, slots, resources, listeners, feeders, shutdown,
  the manual (no-derive) API, and the concurrency model.

## License

MIT. See [`LICENSE`](LICENSE).
