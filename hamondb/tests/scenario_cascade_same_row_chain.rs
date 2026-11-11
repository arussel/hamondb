#[path = "harness/mod.rs"]
mod harness;

#[test]
fn cascade_same_row_chain() {
    harness::run_scenario("cascade_same_row_chain");
}
