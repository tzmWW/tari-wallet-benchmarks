use std::{fs, path::Path};

use assert_cmd::Command;
use predicates::prelude::*;
use sha2::{Digest, Sha256};
use wallet_bench::{
    build_manifest::BUILD_MANIFEST_SCHEMA_VERSION,
    config::Config,
    env_capture,
    modes::ModeName,
    result_profile::{RESULT_SCHEMA_VERSION, ResultProfile, empty_mode_profile},
    seeds::{WalletRole, material_from_seed},
};

#[test]
fn prefunding_template_loads_without_manual_funding_records() {
    let config = Config::load_prefunding_b0(Path::new("harness-prefunding.toml")).unwrap();
    assert!(config.funding.as_map().is_empty());
    assert!(config.benchmark.live_fresh_scan_cells);
    assert!(config.benchmark.mode1_live_topology);
    assert!(config.benchmark.mode2_live_scenarios);
    assert!(config.benchmark.mode3_live_topology);
}

#[test]
fn canonical_pins_are_consistent_across_build_inputs() {
    let config = Config::load_prefunding_b0(Path::new("harness-prefunding.toml")).unwrap();
    assert_eq!(
        config.versions.minotari_cli_rev,
        wallet_bench::versions::MINOTARI_CLI_REV
    );
    assert_eq!(
        config.versions.tari_console_wallet_rev,
        wallet_bench::versions::TARI_CONSOLE_WALLET_REV
    );
    assert_eq!(
        config.versions.payment_processor_rev,
        wallet_bench::versions::PAYMENT_PROCESSOR_REV
    );
    for path in [
        "Cargo.toml",
        "scripts/fetch-minotari-cli.sh",
        "scripts/fetch-payment-processor.sh",
    ] {
        let contents = fs::read_to_string(path).unwrap();
        assert!(
            contents.contains(wallet_bench::versions::MINOTARI_CLI_REV),
            "{path}"
        );
    }
}

#[test]
fn source_provenance_inputs_are_immutable_and_verifiable() {
    assert_eq!(BUILD_MANIFEST_SCHEMA_VERSION, 2);
    let patches = [
        (
            "patches/minotari-fixed-range-scan.patch",
            "8efbed4f8cfbd87f5ad83080fd9ad70fdf9b8841b48b13279c9863b38fda807d",
        ),
        (
            "patches/minotari-wallet-password-env.patch",
            "fa49b2d0fa25ae31e2fdc9e17f85ca67a9a0206b9a62192d1b632d14b67888a6",
        ),
        (
            "patches/payment-processor-fee-rate.patch",
            "69c3001b4474d478822651810dc5f25cae5c8bfede2f9bc756de6ded37dc89fe",
        ),
    ];
    for (path, expected) in patches {
        assert_eq!(
            hex::encode(Sha256::digest(fs::read(path).unwrap())),
            expected
        );
    }
    assert!(
        fs::read_to_string("patches/minotari-wallet-password-env.patch")
            .unwrap()
            .contains("hide_env_values = true")
    );

    for script in [
        "scripts/fetch-minotari-cli.sh",
        "scripts/fetch-payment-processor.sh",
    ] {
        let contents = fs::read_to_string(script).unwrap();
        assert!(contents.contains("--verify-only"), "{script}");
        assert!(
            contents.contains("diff --cached --full-index --binary"),
            "{script}"
        );
        assert!(contents.contains("write-tree"), "{script}");
        assert!(
            contents.contains("minotari-fixed-range-scan.patch"),
            "{script}"
        );
        assert!(
            !contents.contains("minotari-exact-output-locking.patch"),
            "{script}"
        );
        assert!(
            contents.contains("codesign --force --sign -"),
            "{script} must normalize copied macOS artifact signatures"
        );
    }
    let license = fs::read_to_string("LICENSE").unwrap();
    assert!(license.contains("BSD 3-Clause License"));
    assert!(license.contains("Tari Wallet Benchmarks contributors"));
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

    Command::cargo_bin("wallet-bench")
        .unwrap()
        .current_dir(tempdir.path())
        .args(["schema", "--out", "relative-schema.json"])
        .assert()
        .success();
    assert!(tempdir.path().join("relative-schema.json").is_file());
}

#[test]
fn validate_and_summarize_profile_commands_use_current_schema() {
    let tempdir = tempfile::tempdir().unwrap();
    let profile_path = tempdir.path().join("checkpoint.json");
    let summary_path = tempdir.path().join("summary.md");
    let mut profile = ResultProfile::new(&Config::default(), env_capture::capture());
    profile.provenance.measurement_build_manifest = serde_json::Value::Null;
    profile.provenance.export_build_manifest = serde_json::Value::Null;
    profile
        .config
        .insert("build_manifest".to_string(), serde_json::Value::Null);
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
