use crate::harness::fire_log::FireLog;
use std::path::PathBuf;
use std::sync::Arc;
use hamondb::test_support::schema::{Category, Order, Product, CURRENCY_LABEL, TOTAL_PRODUCT_COUNT};

mod fire_log;
pub mod harness;
pub mod schema;
mod replay;
mod canonical;
mod final_state;

pub fn run_scenario(name: &str) {
    let dir: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "scenarios", name].iter().collect();

    let fire_log = Arc::new(FireLog::new());
    let (hamon, _) = build_test_hamon(fire_log.clone());
    let events = replay::load_events(&dir.join("events.json"));
    let expected_fires = canonical::load_expected_fires(&dir.join("expected_listener_log.json"));
    let expected_state = canonical::load_expected_state(&dir.join("expected_final_state.json"));

    replay::replay_events(&hamon, &fire_log, &events);
    fire_log.wait_for(expected_fires.len());

    let observed_fires = fire_log.snapshot_canonical();
    let observed_state = final_state::collect(&hamon);

    canonical::assert_or_bless(&dir.join("expected_listener_log.json"), &expected_fires, &observed_fires);
    canonical::assert_or_bless(&dir.join("expected_final_state.json"), &expected_state, &observed_state);
}

pub fn build_test_hamon(fire_log: Arc<FireLog>) -> (hamondb::Hamon, hamondb::HamonShutdown) {
    let mut builder = hamondb::HamonBuilder::new();
    builder
        .register_table::<Category>()
        .register_listener(fire_log.recorder::<Category>("category"));
    builder
        .register_table::<Product>()
        .register_listener(fire_log.recorder::<Product>("product"));
    builder
        .register_table::<Order>()
        .register_listener(fire_log.recorder::<Order>("order"));
    builder.register_slot(TOTAL_PRODUCT_COUNT, 0);
    builder.register_resource(CURRENCY_LABEL, Arc::<str>::from(""));
    builder.build()
}
