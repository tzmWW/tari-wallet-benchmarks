use std::{fs, path::Path};

use assert_cmd::Command;
use predicates::prelude::*;
use wallet_bench::{
    config::Config,
    result_profile::RESULT_SCHEMA_VERSION,
    seeds::{WalletRole, material_from_seed},
};

#[test]
fn example_config_loads() {
    let config = Config::load(Path::new("harness.toml.example")).unwrap();
    assert_eq!(config.network.name, "esmeralda");
    assert_eq!(
        config.benchmark.concurrent_batches,
        vec![8, 16, 32, 64, 128]
    );
}

#[test]
fn schema_command_writes_json() {
    let tempdir = tempfile::tempdir().unwrap();
    let schema_path = tempdir.path().join("schema.json");

    Command::cargo_bin("wallet-bench")
        .unwrap()
        .args(["schema", "--out"])
        .arg(&schema_path)
        .assert()
        .success();

    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(schema_path).unwrap()).unwrap();
    assert_eq!(json["schema_version"], RESULT_SCHEMA_VERSION);
    assert_eq!(json["tx_mined_confirmed_status_value"], 6);
    assert!(
        json["required_top_level_keys"]
            .as_array()
            .unwrap()
            .iter()
            .any(|key| key == "funding")
    );
    assert!(
        json["required_top_level_keys"]
            .as_array()
            .unwrap()
            .iter()
            .any(|key| key == "computed_deltas")
    );
}

#[test]
fn seed_material_json_omits_seed_words() {
    let material = material_from_seed(
        WalletRole::OldWallet,
        "HARNESS_SEED_OLD".to_string(),
        tari_common_types::seeds::cipher_seed::CipherSeed::random(),
    )
    .unwrap();
    let json = serde_json::to_string(&material).unwrap();
    assert!(!json.contains(&material.seed_words));
    assert!(predicate::str::contains("address").eval(&json));
}
