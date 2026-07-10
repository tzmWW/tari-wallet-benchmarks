use std::{fs, path::Path};

use assert_cmd::Command;
use predicates::prelude::*;
use wallet_bench::{
    config::Config,
    env_capture,
    modes::ModeName,
    result_profile::{RESULT_SCHEMA_VERSION, ResultProfile, empty_mode_profile},
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
        serde_json::from_str(&fs::read_to_string(&schema_path).unwrap()).unwrap();
    assert_eq!(
        json["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert_eq!(
        json["properties"]["schema_version"]["const"],
        RESULT_SCHEMA_VERSION
    );
    assert_eq!(
        json["$defs"]["verified_transaction"]["properties"]["status_value"]["const"],
        6
    );
    assert!(json["$defs"]["transaction_observation"].is_object());
    assert!(
        json["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|key| key == "funding")
    );
    assert!(
        json["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|key| key == "computed_deltas")
    );
    assert_eq!(
        fs::read(&schema_path).unwrap(),
        fs::read("RESULT_PROFILE_SCHEMA.json").unwrap()
    );
}

#[test]
fn validate_and_summarize_profile_commands_use_schema_v4() {
    let tempdir = tempfile::tempdir().unwrap();
    let profile_path = tempdir.path().join("checkpoint.json");
    let summary_path = tempdir.path().join("summary.md");
    let mut profile = ResultProfile::new(&Config::default(), env_capture::capture());
    for mode in ModeName::ALL {
        profile.modes.insert(
            mode.as_str().to_string(),
            empty_mode_profile(mode, Some(format!("{mode:?}-address"))),
        );
    }
    profile.refresh_computed_deltas();
    profile.write_atomic(&profile_path).unwrap();

    Command::cargo_bin("wallet-bench")
        .unwrap()
        .args(["validate-profile", "--profile"])
        .arg(&profile_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("profile PASS"));

    Command::cargo_bin("wallet-bench")
        .unwrap()
        .args(["summarize-profile", "--profile"])
        .arg(&profile_path)
        .arg("--out")
        .arg(&summary_path)
        .assert()
        .success();

    let first = fs::read_to_string(&summary_path).unwrap();
    Command::cargo_bin("wallet-bench")
        .unwrap()
        .args(["summarize-profile", "--profile"])
        .arg(&profile_path)
        .arg("--out")
        .arg(&summary_path)
        .assert()
        .success();
    assert_eq!(first, fs::read_to_string(summary_path).unwrap());
    assert!(first.contains("| old_wallet | S1 |"));
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
