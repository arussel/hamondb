#[path = "harness/mod.rs"]
mod harness;

#[test]
fn basic_crud() {
    harness::run_scenario("basic_crud");
}
