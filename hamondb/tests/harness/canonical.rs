use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fs;
use std::path::Path;

pub fn load_expected_fires(path: &Path) -> Vec<super::fire_log::FireRecord> {
    load_json(path).unwrap_or_default()
}

pub fn load_expected_state(path: &Path) -> super::final_state::FinalState {
    load_json(path).unwrap_or_else(|| panic!("missing {}", path.display()))
}

pub fn load_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    fs::read_to_string(path)
        .ok()
        .map(|s| serde_json::from_str(&s).unwrap())
}

pub fn assert_or_bless<T>(path: &Path, expected: &T, actual: &T)
where
    T: Serialize + PartialEq + std::fmt::Debug,
{
    if bless_mode() {
        fs::write(path, serde_json::to_string_pretty(expected).unwrap()).unwrap();
        return;
    }
    assert_eq!(
        serde_json::to_string_pretty(expected).unwrap(),
        serde_json::to_string_pretty(actual).unwrap()
    );
}

fn bless_mode() -> bool {
    std::env::var("BLESS").as_deref() == Ok("1")
}
