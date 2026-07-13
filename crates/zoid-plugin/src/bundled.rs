//! Bundled (first-party) manifests shipped inside zoid, embedded at compile time.

use crate::manifest::{parse_manifest, PluginManifest};

const SUPERPOWERS_TOML: &str = include_str!("../manifests/superpowers.toml");

pub fn bundled_ids() -> &'static [&'static str] {
    &["superpowers"]
}

pub fn bundled_manifest(id: &str) -> Option<PluginManifest> {
    match id {
        "superpowers" => Some(parse_manifest(SUPERPOWERS_TOML).expect("bundled superpowers.toml parses")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn superpowers_is_bundled_and_valid() {
        let m = bundled_manifest("superpowers").unwrap();
        m.validate().unwrap();
        assert_eq!(m.source.as_ref().unwrap().ref_, "d884ae04edebef577e82ff7c4e143debd0bbec99");
    }

    #[test]
    fn unknown_id_is_none() {
        assert!(bundled_manifest("nope").is_none());
    }

    #[test]
    fn every_bundled_id_resolves_to_a_manifest() {
        for id in bundled_ids() {
            assert!(bundled_manifest(id).is_some(), "bundled id {id} has no manifest");
        }
    }

    #[test]
    fn bundled_superpowers_reproduces_the_golden_body() {
        let m = crate::bundled::bundled_manifest("superpowers").unwrap();
        let plan = crate::plan::build_plan(&m, &crate::plan::tests::scan()).unwrap();
        let golden = include_str!("../tests/superpowers_body_golden.txt");
        assert_eq!(plan.mapping.mode_body, golden,
            "bundled superpowers.toml body drifted from the golden");
    }
}
