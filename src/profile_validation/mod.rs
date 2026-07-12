use std::{collections::BTreeSet, fs, path::Path};

use anyhow::{Context, bail};
use serde_json::{Value, json};

use super::{ProfileKind, REFERENCE_BASE_NODE_REVISION, RESULT_SCHEMA_VERSION, ResultProfile};
use crate::{
    modes::{ModeName, ScenarioName},
    versions::TX_MINED_CONFIRMED_STATUS,
};

const REQUIRED_STAGES: [&str; 4] = [
    "old_wallet",
    "new_wallet",
    "payment_processor",
    "fresh_scans",
];
const REFERENCE_S4_RAMP: [u64; 5] = [8, 16, 32, 64, 128];

pub fn schema_document() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://github.com/tzmWW/tari-wallet-benchmarks/blob/main/RESULT_PROFILE_SCHEMA.json",
        "title": "Tari wallet benchmark result profile",
        "type": "object",
        "additionalProperties": false,
        "required": [
            "schema_version", "run_id", "profile_kind", "run_complete",
            "harness_git_commit", "completed_stages", "generated_at", "network",
            "base_node", "environment", "versions", "config", "funding", "modes",
            "computed_deltas", "findings", "chain_verification"
        ],
        "properties": {
            "schema_version": {"const": RESULT_SCHEMA_VERSION},
            "run_id": {"type": "string", "minLength": 1},
            "profile_kind": {"enum": ["checkpoint", "final"]},
            "run_complete": {"type": "boolean"},
            "harness_git_commit": {"type": "string", "minLength": 1},
            "completed_stages": {
                "type": "array", "items": {"type": "string", "minLength": 1},
                "uniqueItems": true
            },
            "generated_at": {"type": "string", "format": "date-time"},
            "network": {"type": "string", "minLength": 1},
            "base_node": {"$ref": "#/$defs/base_node"},
            "environment": {"$ref": "#/$defs/environment"},
            "versions": {
                "type": "object", "additionalProperties": {"type": "string"}
            },
            "config": {"$ref": "#/$defs/config"},
            "funding": {"$ref": "#/$defs/funding"},
            "modes": {"$ref": "#/$defs/modes"},
            "computed_deltas": {"$ref": "#/$defs/computed_deltas"},
            "findings": {"type": "array", "items": {"$ref": "#/$defs/finding"}},
            "chain_verification": {"$ref": "#/$defs/chain_verification"}
        },
        "$defs": {
            "nullable_string": {"type": ["string", "null"]},
            "nullable_integer": {"type": ["integer", "null"], "minimum": 0},
            "base_node": {
                "type": "object", "additionalProperties": false,
                "required": [
                    "endpoint", "configured_revision", "observed_version", "version_observable",
                    "tip_start_height", "tip_start_hash", "tip_end_height", "tip_end_hash",
                    "pruning_horizon", "is_synced"
                ],
                "properties": {
                    "endpoint": {"type": "string", "minLength": 1},
                    "configured_revision": {"type": "string", "minLength": 1},
                    "observed_version": {"$ref": "#/$defs/nullable_string"},
                    "version_observable": {"type": "boolean"},
                    "tip_start_height": {"$ref": "#/$defs/nullable_integer"},
                    "tip_start_hash": {"$ref": "#/$defs/nullable_string"},
                    "tip_end_height": {"$ref": "#/$defs/nullable_integer"},
                    "tip_end_hash": {"$ref": "#/$defs/nullable_string"},
                    "pruning_horizon": {"$ref": "#/$defs/nullable_integer"},
                    "is_synced": {"type": ["boolean", "null"]}
                }
            },
            "environment": {
                "type": "object", "additionalProperties": false,
                "required": ["os", "cpu_brand", "physical_cores", "total_memory_bytes", "base_node_network_path"],
                "properties": {
                    "os": {"type": "string"},
                    "cpu_brand": {"type": "string"},
                    "physical_cores": {"$ref": "#/$defs/nullable_integer"},
                    "total_memory_bytes": {"type": "integer", "minimum": 0},
                    "disk_kind": {"type": "string"},
                    "disk_name": {"type": "string"},
                    "base_node_host": {"type": "string"},
                    "base_node_network_path": {"enum": ["local", "remote", "unknown"]}
                }
            },
            "config": {
                "type": "object",
                "required": [
                    "A_fund", "C_min", "volume_target", "doubling_rounds",
                    "fanout_outputs_per_tx", "concurrent_batches", "S4_T_budget_secs",
                    "S5_M", "S5_K", "fee_rate", "repetitions", "scan_repetitions"
                ],
                "properties": {
                    "A_fund": {"type": "string"},
                    "C_min": {"type": "integer", "minimum": 1},
                    "volume_target": {"type": "integer", "minimum": 1},
                    "doubling_rounds": {"type": "integer", "minimum": 1},
                    "fanout_outputs_per_tx": {"type": "integer", "minimum": 1},
                    "concurrent_batches": {
                        "type": "array", "items": {"type": "integer", "minimum": 1},
                        "minItems": 1, "uniqueItems": true
                    },
                    "S4_T_budget_secs": {"type": "integer", "minimum": 1},
                    "S5_M": {"type": "integer", "minimum": 1},
                    "S5_K": {"type": "integer", "minimum": 1},
                    "fee_rate": {"type": "string"},
                    "repetitions": {"type": "integer", "minimum": 1},
                    "scan_repetitions": {"type": "integer", "minimum": 1}
                },
                "additionalProperties": true
            },
            "funding_record": {
                "type": "object", "additionalProperties": false,
                "required": ["amount", "tx_id", "height"],
                "properties": {
                    "amount": {"type": "string"},
                    "tx_id": {"type": "string", "minLength": 1},
                    "height": {"type": "integer", "minimum": 0},
                    "birthday": {"$ref": "#/$defs/nullable_integer"},
                    "birthday_start_height": {"$ref": "#/$defs/nullable_integer"},
                    "construction_ms": {"$ref": "#/$defs/nullable_integer"},
                    "broadcast_to_mempool_ms": {"$ref": "#/$defs/nullable_integer"},
                    "broadcast_to_confirmed_at_c_min_ms": {"$ref": "#/$defs/nullable_integer"},
                    "tip_height_at_confirmation": {"$ref": "#/$defs/nullable_integer"}
                }
            },
            "funding": {
                "type": "object", "additionalProperties": false,
                "properties": {
                    "old_wallet": {"$ref": "#/$defs/funding_record"},
                    "new_wallet": {"$ref": "#/$defs/funding_record"},
                    "payment_processor": {"$ref": "#/$defs/funding_record"}
                }
            },
            "execution_status": {
                "enum": ["completed", "blocked_prerequisite", "harness_error", "not_applicable"]
            },
            "outcome_status": {"enum": ["success", "partial", "failure", "unavailable"]},
            "repetition": {
                "type": "object", "additionalProperties": false,
                "required": [
                    "run", "execution_status", "outcome_status", "wall_ms", "success_count",
                    "failure_count", "fee_microtari", "error"
                ],
                "properties": {
                    "run": {"type": "integer", "minimum": 1},
                    "execution_status": {"$ref": "#/$defs/execution_status"},
                    "outcome_status": {"$ref": "#/$defs/outcome_status"},
                    "wall_ms": {"$ref": "#/$defs/nullable_integer"},
                    "success_count": {"type": "integer", "minimum": 0},
                    "failure_count": {"type": "integer", "minimum": 0},
                    "fee_microtari": {"$ref": "#/$defs/nullable_integer"},
                    "error": {"$ref": "#/$defs/nullable_string"},
                    "metrics": {
                        "type": "object", "additionalProperties": true,
                        "properties": {
                            "blocked_prerequisite": {"type": "boolean"},
                            "unspent_after": {"$ref": "#/$defs/nullable_integer"},
                            "tx_timings": {"type": "array", "items": {"type": "object", "additionalProperties": true}},
                            "transaction_observations": {
                                "type": "array", "items": {"$ref": "#/$defs/transaction_observation"}
                            },
                            "s5_arms": {"type": "object", "additionalProperties": true}
                        }
                    }
                }
            },
            "transaction_observation": {
                "type": "object", "additionalProperties": false,
                "required": [
                    "transaction_id", "construction_ms", "submission_ms", "mempool_available", "mempool_reason",
                    "confirmation_ms", "fee_microtari", "terminal_outcome", "error",
                    "mined_height", "tip_start_height", "tip_end_height"
                ],
                "properties": {
                    "transaction_id": {"$ref": "#/$defs/nullable_string"},
                    "construction_ms": {"$ref": "#/$defs/nullable_integer"},
                    "submission_ms": {"$ref": "#/$defs/nullable_integer"},
                    "mempool_available": {"type": ["boolean", "null"]},
                    "mempool_reason": {"$ref": "#/$defs/nullable_string"},
                    "confirmation_ms": {"$ref": "#/$defs/nullable_integer"},
                    "fee_microtari": {"$ref": "#/$defs/nullable_integer"},
                    "terminal_outcome": {"enum": ["confirmed", "rejected", "timed_out", "unavailable"]},
                    "error": {"$ref": "#/$defs/nullable_string"},
                    "mined_height": {"$ref": "#/$defs/nullable_integer"},
                    "tip_start_height": {"$ref": "#/$defs/nullable_integer"},
                    "tip_end_height": {"$ref": "#/$defs/nullable_integer"}
                }
            },
            "scenario": {
                "type": "object", "additionalProperties": false,
                "required": [
                    "scenario", "surface", "execution_status", "outcome_status", "repetitions",
                    "median_wall_ms", "spread_wall_ms", "notes"
                ],
                "properties": {
                    "scenario": {"enum": ["b0", "s0", "s1", "s2", "s3", "s4", "s5", "s6", "s7"]},
                    "surface": {"type": "string", "minLength": 1},
                    "execution_status": {"$ref": "#/$defs/execution_status"},
                    "outcome_status": {"$ref": "#/$defs/outcome_status"},
                    "repetitions": {"type": "array", "items": {"$ref": "#/$defs/repetition"}},
                    "median_wall_ms": {"$ref": "#/$defs/nullable_integer"},
                    "spread_wall_ms": {"$ref": "#/$defs/nullable_integer"},
                    "notes": {"type": "array", "items": {"type": "string"}}
                }
            },
            "scenario_map": {
                "type": "object", "additionalProperties": false,
                "required": ["B0", "S0", "S1", "S2", "S3", "S4", "S5", "S6", "S7"],
                "properties": {
                    "B0": {"$ref": "#/$defs/scenario"}, "S0": {"$ref": "#/$defs/scenario"},
                    "S1": {"$ref": "#/$defs/scenario"}, "S2": {"$ref": "#/$defs/scenario"},
                    "S3": {"$ref": "#/$defs/scenario"}, "S4": {"$ref": "#/$defs/scenario"},
                    "S5": {"$ref": "#/$defs/scenario"}, "S6": {"$ref": "#/$defs/scenario"},
                    "S7": {"$ref": "#/$defs/scenario"}
                }
            },
            "mode": {
                "type": "object", "additionalProperties": false,
                "required": ["mode", "address", "scenarios"],
                "properties": {
                    "mode": {"enum": ["old_wallet", "new_wallet", "payment_processor"]},
                    "address": {"$ref": "#/$defs/nullable_string"},
                    "scenarios": {"$ref": "#/$defs/scenario_map"}
                }
            },
            "modes": {
                "type": "object", "additionalProperties": false,
                "required": ["old_wallet", "new_wallet", "payment_processor"],
                "properties": {
                    "old_wallet": {"$ref": "#/$defs/mode"},
                    "new_wallet": {"$ref": "#/$defs/mode"},
                    "payment_processor": {"$ref": "#/$defs/mode"}
                }
            },
            "computed_deltas": {
                "type": "object", "additionalProperties": false,
                "required": ["scan_deltas", "s5_throughput"],
                "properties": {
                    "scan_deltas": {"type": "object", "additionalProperties": true},
                    "s5_throughput": {
                        "type": "object", "additionalProperties": false,
                        "required": ["arms", "comparisons", "comparison_unavailable_reasons", "source"],
                        "properties": {
                            "arms": {"type": "object", "additionalProperties": true},
                            "comparisons": {"type": "object", "additionalProperties": {"type": ["number", "null"]}},
                            "comparison_unavailable_reasons": {"type": "object", "additionalProperties": {"type": ["string", "null"]}},
                            "source": {"type": "string"}
                        }
                    }
                }
            },
            "finding": {
                "type": "object", "additionalProperties": false,
                "required": ["id", "title", "status", "recommendation"],
                "properties": {
                    "id": {"type": "string"}, "title": {"type": "string"},
                    "status": {"type": "string"}, "recommendation": {"type": "string"}
                }
            },
            "verified_transaction": {
                "type": "object", "additionalProperties": false,
                "required": [
                    "tx_id", "status_value", "mode", "scenario", "mined_height", "confirmations",
                    "min_confirmations", "tip_height", "confirmed"
                ],
                "properties": {
                    "tx_id": {"type": "string", "minLength": 1},
                    "status_value": {"const": TX_MINED_CONFIRMED_STATUS},
                    "mode": {"enum": ["old_wallet", "new_wallet", "payment_processor"]},
                    "scenario": {"enum": ["S1", "S4", "S5"]},
                    "amount_microtari": {"type": "integer", "minimum": 0},
                    "fee_microtari": {"type": "integer", "minimum": 0},
                    "mined_height": {"type": "integer", "minimum": 1},
                    "confirmations": {"type": "integer", "minimum": 1},
                    "min_confirmations": {"type": "integer", "minimum": 1},
                    "tip_height": {"type": "integer", "minimum": 1},
                    "confirmed": {"const": true}
                }
            },
            "chain_verification": {
                "type": "object", "additionalProperties": false,
                "required": ["tx_mined_confirmed_status_value", "verified_transactions"],
                "properties": {
                    "tx_mined_confirmed_status_value": {"const": TX_MINED_CONFIRMED_STATUS},
                    "verified_transactions": {"type": "array", "items": {"$ref": "#/$defs/verified_transaction"}}
                }
            }
        }
    })
}

pub fn validate_path(path: &Path, submission: bool) -> anyhow::Result<ResultProfile> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let document: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing {} as JSON", path.display()))?;
    validate_document(&document, submission)
}

pub fn validate_document(document: &Value, submission: bool) -> anyhow::Result<ResultProfile> {
    validate_schema(document)?;
    let profile: ResultProfile = serde_json::from_value(document.clone())
        .context("deserializing schema-v4 result profile")?;
    validate_profile(&profile, submission)?;
    if profile.profile_kind == ProfileKind::Final || submission {
        validate_final_document(&profile, document, submission)?;
    }
    Ok(profile)
}

pub fn validate_profile(profile: &ResultProfile, _submission: bool) -> anyhow::Result<()> {
    let document = serde_json::to_value(profile).context("serializing result profile")?;
    validate_schema(&document)?;
    validate_identity(profile)?;
    validate_transactions(profile)?;
    validate_status_pairs(&document)?;
    if profile.profile_kind == ProfileKind::Checkpoint && profile.run_complete {
        bail!("checkpoint profile must have run_complete=false");
    }
    Ok(())
}

pub fn validate_final(profile: &ResultProfile, submission: bool) -> anyhow::Result<()> {
    let document = serde_json::to_value(profile).context("serializing result profile")?;
    validate_final_document(profile, &document, submission)
}

fn validate_schema(document: &Value) -> anyhow::Result<()> {
    let schema = schema_document();
    let validator = jsonschema::draft202012::options()
        .should_validate_formats(true)
        .build(&schema)
        .context("compiling result profile JSON Schema")?;
    let errors = validator
        .iter_errors(document)
        .map(|error| format!("{}: {error}", error.instance_path()))
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        bail!("profile does not match schema v4:\n{}", errors.join("\n"));
    }
    Ok(())
}

fn validate_identity(profile: &ResultProfile) -> anyhow::Result<()> {
    if profile.schema_version != RESULT_SCHEMA_VERSION {
        bail!("schema_version must be {RESULT_SCHEMA_VERSION}");
    }
    for mode in ModeName::ALL {
        let key = mode.as_str();
        let entry = profile
            .modes
            .get(key)
            .with_context(|| format!("missing mode {key}"))?;
        if entry.mode != mode {
            bail!("mode map key {key} does not match embedded mode");
        }
        for scenario in ScenarioName::ALL {
            let scenario_key = scenario.as_str();
            let cell = entry
                .scenarios
                .get(scenario_key)
                .with_context(|| format!("missing {key}/{scenario_key}"))?;
            if cell.scenario != scenario {
                bail!("scenario map key {key}/{scenario_key} does not match embedded scenario");
            }
        }
    }
    Ok(())
}

fn validate_transactions(profile: &ResultProfile) -> anyhow::Result<()> {
    if profile.chain_verification.tx_mined_confirmed_status_value != TX_MINED_CONFIRMED_STATUS {
        bail!("chain verification status constant must be {TX_MINED_CONFIRMED_STATUS}");
    }
    let mut seen = BTreeSet::new();
    for tx in &profile.chain_verification.verified_transactions {
        if tx.status_value != TX_MINED_CONFIRMED_STATUS || !tx.confirmed {
            bail!(
                "top-level transaction {} is not independently confirmed",
                tx.tx_id
            );
        }
        if tx.mined_height.is_none() {
            bail!("top-level transaction {} has no mined_height", tx.tx_id);
        }
        let confirmations = tx
            .confirmations
            .context("top-level transaction has no confirmations")?;
        let min_confirmations = tx
            .min_confirmations
            .context("top-level transaction has no min_confirmations")?;
        let tip_height = tx
            .tip_height
            .context("top-level transaction has no tip_height")?;
        let mined_height = tx.mined_height.expect("checked above");
        if confirmations < min_confirmations
            || tip_height < mined_height
            || tip_height.saturating_sub(mined_height) < min_confirmations
        {
            bail!(
                "top-level transaction {} has insufficient independent C_min depth proof",
                tx.tx_id
            );
        }
        if !seen.insert(&tx.tx_id) {
            bail!("duplicate top-level transaction id {}", tx.tx_id);
        }
    }
    Ok(())
}

fn validate_status_pairs(document: &Value) -> anyhow::Result<()> {
    for (mode, scenarios) in mode_scenarios(document)? {
        for (scenario, cell) in scenarios {
            validate_status_pair(cell, &format!("{mode}/{scenario}"))?;
            if let Some(repetitions) = cell.get("repetitions").and_then(Value::as_array) {
                for repetition in repetitions {
                    validate_status_pair(repetition, &format!("{mode}/{scenario} repetition"))?;
                }
            }
        }
    }
    Ok(())
}

fn validate_status_pair(value: &Value, label: &str) -> anyhow::Result<()> {
    let execution = value["execution_status"].as_str().unwrap_or_default();
    let outcome = value["outcome_status"].as_str().unwrap_or_default();
    let coherent = match execution {
        "completed" => matches!(outcome, "success" | "partial" | "failure"),
        "blocked_prerequisite" | "harness_error" | "not_applicable" => outcome == "unavailable",
        _ => false,
    };
    if !coherent {
        bail!("incoherent execution/outcome status for {label}: {execution}/{outcome}");
    }
    Ok(())
}

fn validate_final_document(
    profile: &ResultProfile,
    document: &Value,
    submission: bool,
) -> anyhow::Result<()> {
    if profile.profile_kind != ProfileKind::Final || !profile.run_complete {
        bail!("final profile must have profile_kind=final and run_complete=true");
    }
    for (mode, scenarios) in mode_scenarios(document)? {
        for (scenario, cell) in scenarios {
            let execution = cell["execution_status"].as_str().unwrap_or_default();
            let repetitions = cell["repetitions"].as_array().expect("schema checked");
            if repetitions.is_empty() && execution != "not_applicable" {
                bail!("final profile contains unexecuted cell {mode}/{scenario}");
            }
        }
    }
    validate_s1_exact_outputs(document)?;
    validate_s5_comparisons(document)?;
    if submission {
        validate_reference_configuration(profile, document)?;
    }
    Ok(())
}

fn validate_reference_configuration(
    profile: &ResultProfile,
    document: &Value,
) -> anyhow::Result<()> {
    let config = &document["config"];
    let exact = [
        ("A_fund", json!("10000 T")),
        ("C_min", json!(3)),
        ("volume_target", json!(512)),
        ("doubling_rounds", json!(6)),
        ("fanout_outputs_per_tx", json!(8)),
        ("S4_T_budget_secs", json!(900)),
        ("S5_M", json!(100)),
        ("S5_K", json!(10)),
        ("repetitions", json!(1)),
    ];
    for (key, expected) in exact {
        if config[key] != expected {
            bail!(
                "submission config {key} must be {expected}, got {}",
                config[key]
            );
        }
    }
    if config["concurrent_batches"] != json!(REFERENCE_S4_RAMP) {
        bail!("submission config must contain the full S4 concurrency ramp");
    }
    if config["scenario_order"] != json!(["B0", "S0", "S1", "S2", "S3", "S4", "S5", "S6", "S7"]) {
        bail!("submission config must record canonical B0,S0-S7 scenario order");
    }
    validate_cross_cutting_scenario_metrics(document)?;
    validate_scenario_specific_metrics(document)?;
    for (mode, scenarios) in mode_scenarios(document)? {
        if scenarios["S1"]["outcome_status"] != "success" {
            bail!("submission requires canonical successful S1 for {mode}");
        }
        for (scenario, cell) in scenarios {
            if cell["execution_status"] == "not_applicable" {
                bail!(
                    "submission requires all 27 benchmark cells; {mode}/{scenario} is not applicable"
                );
            }
            if cell["execution_status"] == "harness_error" {
                bail!("submission contains harness error at {mode}/{scenario}");
            }
            if cell["repetitions"].as_array().is_some_and(|runs| {
                runs.iter()
                    .any(|run| run["execution_status"] == "harness_error")
            }) {
                bail!("submission contains harness error repetition at {mode}/{scenario}");
            }
        }
    }
    for flag in [
        "mode1_live_topology",
        "mode2_live_scenarios",
        "mode3_live_topology",
        "live_fresh_scan_cells",
    ] {
        if config[flag] != Value::Bool(true) {
            bail!("submission config {flag} must be true");
        }
    }
    for cap in [
        "mode1_live_max_s1_txs",
        "mode1_live_max_s4_batch",
        "mode1_live_max_s5_items",
        "mode2_live_max_s1_txs",
        "mode2_live_max_s4_batch",
        "mode2_live_max_s5_txs",
        "mode3_live_max_s1_batches",
        "mode3_live_max_s4_batch",
        "mode3_live_max_s5_items",
    ] {
        if config[cap] != json!(0) {
            bail!("submission config {cap} must be zero (uncapped)");
        }
    }
    let completed = profile
        .completed_stages
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if REQUIRED_STAGES
        .iter()
        .any(|stage| !completed.contains(stage))
    {
        bail!("submission profile is missing one or more completed stages");
    }
    if profile.harness_git_commit == "unknown" {
        bail!("submission profile must identify the harness git commit");
    }
    if profile.base_node.configured_revision != REFERENCE_BASE_NODE_REVISION {
        bail!(
            "submission base-node compatibility reference must be {REFERENCE_BASE_NODE_REVISION}"
        );
    }
    if !profile.base_node.version_observable && profile.base_node.observed_version.is_some() {
        bail!("unobservable base-node version must not claim an observed version");
    }
    if profile.base_node.tip_start_height.is_none()
        || profile
            .base_node
            .tip_start_hash
            .as_deref()
            .is_none_or(str::is_empty)
        || profile.base_node.tip_end_height.is_none()
        || profile
            .base_node
            .tip_end_hash
            .as_deref()
            .is_none_or(str::is_empty)
    {
        bail!("submission profile must include base-node tip height/hash start/end anchors");
    }
    if profile.base_node.is_synced != Some(true) || profile.base_node.pruning_horizon.is_none() {
        bail!("submission profile must record synchronized base-node and pruning state");
    }
    if profile
        .chain_verification
        .verified_transactions
        .iter()
        .any(|tx| tx.fee_microtari.is_none())
    {
        bail!("submission top-level transactions must include observed fees");
    }
    for mode in ModeName::ALL {
        if profile.modes[mode.as_str()].address.is_none() {
            bail!(
                "submission mode {} is missing its public address",
                mode.as_str()
            );
        }
        let Some(funding) = profile.funding.get(mode.as_str()) else {
            bail!("submission funding is missing {}", mode.as_str());
        };
        if funding.birthday.is_none() || funding.birthday_start_height.is_none() {
            bail!(
                "submission funding for {} is missing birthday resolution",
                mode.as_str()
            );
        }
        if funding.construction_ms.is_none()
            || funding.broadcast_to_mempool_ms.is_none()
            || funding.broadcast_to_confirmed_at_c_min_ms.is_none()
            || funding.tip_height_at_confirmation.is_none()
        {
            bail!(
                "submission funding for {} is missing measured S0 timing/tip evidence",
                mode.as_str()
            );
        }
        if !profile
            .chain_verification
            .verified_transactions
            .iter()
            .any(|tx| tx.mode == mode.as_str() && tx.scenario == "S1")
        {
            bail!(
                "successful submission S1 for {} has no independently verified transaction",
                mode.as_str()
            );
        }
    }
    Ok(())
}

fn validate_cross_cutting_scenario_metrics(document: &Value) -> anyhow::Result<()> {
    for (mode, scenarios) in mode_scenarios(document)? {
        for (scenario, cell) in scenarios {
            for (index, run) in cell["repetitions"]
                .as_array()
                .expect("schema checked")
                .iter()
                .enumerate()
            {
                if run["execution_status"] != "completed" {
                    continue;
                }
                let label = format!("{mode}/{scenario} repetition {}", index + 1);
                if run["wall_ms"].as_u64().is_none() {
                    bail!("submission {label} must record wall_ms");
                }
                if run["fee_microtari"].as_u64().is_none() {
                    bail!("submission {label} must record explicit fees, including zero");
                }
                let Some(metrics) = run["metrics"].as_object() else {
                    bail!("submission {label} must record scenario metrics");
                };
                let has_balance = metrics.contains_key("balance_reconciliation")
                    || metrics
                        .get("balance_delta_microtari")
                        .is_some_and(|value| !value.is_null())
                    || metrics
                        .get("extra")
                        .and_then(Value::as_object)
                        .is_some_and(|extra| extra.contains_key("balance_reconciliation"));
                let has_unavailable_reason = metrics
                    .get("balance_reconciliation_unavailable_reason")
                    .and_then(Value::as_str)
                    .is_some_and(|reason| !reason.is_empty());
                if !has_balance && !has_unavailable_reason {
                    bail!(
                        "submission {label} must record final balance reconciliation or an explicit unavailable reason"
                    );
                }
            }
        }
    }
    Ok(())
}

fn validate_scenario_specific_metrics(document: &Value) -> anyhow::Result<()> {
    let expected_rounds = [
        (1, 1, 2, 2),
        (2, 2, 2, 4),
        (3, 4, 2, 8),
        (4, 8, 2, 16),
        (5, 16, 2, 32),
        (6, 32, 2, 64),
        (7, 64, 8, 512),
    ];
    let mut recipient_set: Option<Vec<Value>> = None;
    for (mode, scenarios) in mode_scenarios(document)? {
        let b0 = &scenarios["B0"]["repetitions"][0]["metrics"];
        if b0["birthday"] != 0
            || b0["detected_outputs"] != 0
            || b0["spendable_outputs"] != 0
            || b0["available_microtari"] != 0
            || b0["history_transactions"] != 0
            || b0["max_height"] != b0["H_tip_end"]
            || b0["tip_lag_tolerance_blocks"] != 0
            || b0["scan_reached_tip"] != true
        {
            bail!("submission B0 exact empty-tip contract failed for {mode}");
        }
        for key in [
            "T_scan_ms",
            "blocks_per_sec",
            "H_tip_start",
            "H_tip_end",
            "peak_rss_bytes",
            "peak_cpu_percent",
        ] {
            if !b0[key].is_number() {
                bail!("submission B0 metric {key} missing for {mode}");
            }
        }

        let s1_metrics = &scenarios["S1"]["repetitions"][0]["metrics"];
        for (round_index, tx_count, outputs_per_tx, target) in expected_rounds {
            let key = format!("round_{round_index}");
            let direct = s1_metrics.get(&key).filter(|value| !value.is_null());
            let nested = s1_metrics["extra"]
                .get(&key)
                .filter(|value| !value.is_null());
            let array = s1_metrics["rounds"].as_array().and_then(|rounds| {
                rounds
                    .iter()
                    .find(|round| round["round_index"] == round_index)
            });
            let plan = s1_metrics["extra"]["rounds"].as_array().and_then(|rounds| {
                rounds
                    .iter()
                    .find(|round| round["round_index"] == round_index)
            });
            let observed = direct.or(nested).or(array).with_context(|| {
                format!("submission {mode}/S1 missing round {round_index} metrics")
            })?;
            let shape = array
                .or(plan)
                .or(direct)
                .or(nested)
                .expect("observed above");
            if shape["tx_count"] != tx_count
                || shape["outputs_per_tx"] != outputs_per_tx
                || shape["target_utxos_after"] != target
                || observed["wall_ms"].as_u64().is_none()
                || observed["failure_count"] != 0
                || observed["fee_only_balance_delta_ok"] != true
            {
                bail!("submission {mode}/S1 round {round_index} contract failed");
            }
        }
        validate_transaction_observations(mode, "S1", s1_metrics)?;

        let s4 = &scenarios["S4"]["repetitions"][0]["metrics"];
        if s4["batch_summaries"].as_array().is_none()
            || s4["max_serialization_gap_ms"].as_u64().is_none()
            || s4["double_selection_rejections"].as_u64().is_none()
        {
            bail!("submission {mode}/S4 is missing concurrency batch metrics");
        }
        validate_transaction_observations(mode, "S4", s4)?;

        let s5 = &scenarios["S5"]["repetitions"][0]["metrics"];
        let set = s5
            .get("recipient_set")
            .or_else(|| s5["extra"].get("recipient_set"))
            .and_then(Value::as_array)
            .with_context(|| format!("submission {mode}/S5 missing recipient_set"))?;
        if set.len() != 100
            || recipient_set
                .as_ref()
                .is_some_and(|expected| expected != set)
        {
            bail!("submission S5 recipient set is not the same 100 addresses for every mode");
        }
        recipient_set = Some(set.clone());
        if s5
            .get("unspent_before")
            .or_else(|| s5["extra"].get("unspent_before"))
            .is_none()
            || s5
                .get("balance_before_microtari")
                .or_else(|| s5["extra"].get("balance_before_microtari"))
                .is_none()
        {
            bail!("submission {mode}/S5 missing disclosed post-S4 starting state");
        }
        validate_transaction_observations(mode, "S5", s5)?;

        for scenario in ["S2", "S3", "S6", "S7"] {
            let cell = &scenarios[scenario];
            if cell["execution_status"] != "completed" {
                continue;
            }
            for run in cell["repetitions"].as_array().expect("schema checked") {
                let metrics = &run["metrics"];
                if metrics["birthday"].as_u64()
                    != Some(if matches!(scenario, "S2" | "S6") {
                        0
                    } else {
                        profile_funding_birthday(document, mode)?
                    })
                    || metrics["max_height"] != metrics["H_tip_end"]
                    || metrics["tip_lag_tolerance_blocks"] != 0
                    || metrics["history_matches_expected"] != true
                {
                    bail!("submission {mode}/{scenario} scan contract failed");
                }
            }
        }
    }
    Ok(())
}

fn profile_funding_birthday(document: &Value, mode: &str) -> anyhow::Result<u64> {
    document["funding"][mode]["birthday"]
        .as_u64()
        .with_context(|| format!("funding birthday missing for {mode}"))
}

fn validate_transaction_observations(
    mode: &str,
    scenario: &str,
    metrics: &Value,
) -> anyhow::Result<()> {
    let observations = metrics["transaction_observations"]
        .as_array()
        .with_context(|| {
            format!("submission {mode}/{scenario} missing transaction observations")
        })?;
    if observations.is_empty() {
        bail!("submission {mode}/{scenario} has no transaction observations");
    }
    for observation in observations {
        let outcome = observation["terminal_outcome"].as_str().unwrap_or_default();
        if observation["transaction_id"].is_string()
            && observation["construction_ms"].as_u64().is_none()
        {
            bail!("submission {mode}/{scenario} transaction is missing construction time");
        }
        if outcome == "confirmed"
            && (observation["confirmation_ms"].as_u64().is_none()
                || observation["fee_microtari"].as_u64().is_none()
                || observation["mined_height"].as_u64().is_none()
                || observation["tip_end_height"].as_u64().is_none())
        {
            bail!(
                "submission {mode}/{scenario} confirmed transaction is missing timing/fee/tip evidence"
            );
        }
        if outcome != "confirmed" && observation["error"].as_str().unwrap_or_default().is_empty() {
            bail!("submission {mode}/{scenario} failed transaction is missing an error reason");
        }
    }
    Ok(())
}

fn validate_s1_exact_outputs(document: &Value) -> anyhow::Result<()> {
    for (mode, scenarios) in mode_scenarios(document)? {
        let s1 = &scenarios["S1"];
        if s1["outcome_status"] != "success" {
            continue;
        }
        let exact = s1["repetitions"]
            .as_array()
            .and_then(|runs| runs.last())
            .and_then(|run| run.get("metrics"))
            .and_then(|metrics| metrics.get("unspent_after"))
            .and_then(Value::as_u64);
        if exact != Some(512) {
            bail!("successful {mode}/S1 must prove unspent_after=512");
        }
    }
    Ok(())
}

fn validate_s5_comparisons(document: &Value) -> anyhow::Result<()> {
    let s5 = &document["computed_deltas"]["s5_throughput"];
    let arms = &s5["arms"];
    let comparisons = &s5["comparisons"];
    let cases = [
        (
            "new_wallet_individual_over_payment_processor_batch",
            ("new_wallet", "individual"),
            ("payment_processor", "batch"),
        ),
        (
            "old_wallet_individual_over_payment_processor_batch",
            ("old_wallet", "individual"),
            ("payment_processor", "batch"),
        ),
    ];
    for (name, left, right) in cases {
        if (!arm_complete(arms, left.0, left.1) || !arm_complete(arms, right.0, right.1))
            && !comparisons[name].is_null()
        {
            bail!("S5 comparison {name} must be null when a source arm is incomplete");
        }
    }
    Ok(())
}

fn arm_complete(arms: &Value, mode: &str, arm: &str) -> bool {
    let arm = &arms[mode][arm];
    let recipients = arm["recipient_count"].as_u64();
    recipients.is_some_and(|count| count > 0)
        && arm["success_count"].as_u64() == recipients
        && arm["failure_count"].as_u64() == Some(0)
}

fn mode_scenarios(
    document: &Value,
) -> anyhow::Result<Vec<(&str, &serde_json::Map<String, Value>)>> {
    document["modes"]
        .as_object()
        .context("modes must be an object")?
        .iter()
        .map(|(mode, value)| {
            value["scenarios"]
                .as_object()
                .map(|scenarios| (mode.as_str(), scenarios))
                .with_context(|| format!("{mode}.scenarios must be an object"))
        })
        .collect()
}

pub fn write_summary(profile_path: &Path, output_path: &Path) -> anyhow::Result<()> {
    let bytes =
        fs::read(profile_path).with_context(|| format!("reading {}", profile_path.display()))?;
    let document: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing {} as JSON", profile_path.display()))?;
    let markdown = render_summary(&document)?;
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output_path, markdown)
        .with_context(|| format!("writing {}", output_path.display()))?;
    Ok(())
}

pub fn render_summary(document: &Value) -> anyhow::Result<String> {
    validate_document(document, false)?;
    let mut output = String::new();
    output.push_str("# Tari Wallet Benchmark Result\n\n");
    output.push_str(&format!(
        "- Run ID: `{}`\n",
        markdown_text(&document["run_id"])
    ));
    output.push_str(&format!(
        "- Profile: `{}`\n",
        markdown_text(&document["profile_kind"])
    ));
    output.push_str(&format!("- Complete: `{}`\n", document["run_complete"]));
    output.push_str(&format!(
        "- Network: `{}`\n",
        markdown_text(&document["network"])
    ));
    output.push_str(&format!(
        "- Harness commit: `{}`\n",
        markdown_text(&document["harness_git_commit"])
    ));
    output.push_str(&format!(
        "- Base node: `{}` (`{}`)\n\n",
        markdown_text(&document["base_node"]["endpoint"]),
        markdown_text(&document["base_node"]["configured_revision"])
    ));
    output
        .push_str("| Mode | Scenario | Execution | Outcome | Median ms | Successes | Failures |\n");
    output.push_str("|---|---:|---|---|---:|---:|---:|\n");
    for mode in ModeName::ALL {
        for scenario in ScenarioName::ALL {
            let cell = &document["modes"][mode.as_str()]["scenarios"][scenario.as_str()];
            let (successes, failures) = cell["repetitions"]
                .as_array()
                .and_then(|runs| runs.last())
                .map(|run| (&run["success_count"], &run["failure_count"]))
                .unwrap_or((&Value::Null, &Value::Null));
            output.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} |\n",
                mode.as_str(),
                scenario.as_str(),
                markdown_text(&cell["execution_status"]),
                markdown_text(&cell["outcome_status"]),
                display_value(&cell["median_wall_ms"]),
                display_value(successes),
                display_value(failures)
            ));
        }
    }
    output.push_str(&format!(
        "\nConfirmed top-level transactions: **{}**\n",
        document["chain_verification"]["verified_transactions"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default()
    ));
    Ok(output)
}

fn markdown_text(value: &Value) -> String {
    value
        .as_str()
        .unwrap_or_default()
        .replace('|', "\\|")
        .replace('`', "'")
}

fn display_value(value: &Value) -> String {
    if value.is_null() {
        "—".to_string()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, env_capture, result_profile::empty_mode_profile};

    fn profile_document() -> Value {
        let config = Config::default();
        let mut profile = ResultProfile::new(&config, env_capture::capture());
        for mode in ModeName::ALL {
            profile.modes.insert(
                mode.as_str().to_string(),
                empty_mode_profile(mode, Some(format!("{mode:?}-address"))),
            );
        }
        profile.refresh_computed_deltas();
        serde_json::to_value(profile).unwrap()
    }

    #[test]
    fn valid_v4_checkpoint_round_trips() {
        let document = profile_document();
        let profile = validate_document(&document, false).unwrap();
        assert_eq!(profile.schema_version, RESULT_SCHEMA_VERSION);
    }

    #[test]
    fn completed_repetitions_require_fee_and_balance_evidence() {
        let document = serde_json::json!({
            "modes": {
                "old_wallet": {
                    "scenarios": {
                        "S0": {
                            "repetitions": [{
                                "execution_status": "completed",
                                "wall_ms": 1,
                                "fee_microtari": 0,
                                "metrics": {
                                    "balance_reconciliation_unavailable_reason": "pre-run timing is unavailable"
                                }
                            }]
                        }
                    }
                }
            }
        });
        validate_cross_cutting_scenario_metrics(&document).unwrap();

        let mut missing_fee = document.clone();
        missing_fee["modes"]["old_wallet"]["scenarios"]["S0"]["repetitions"][0]["fee_microtari"] =
            Value::Null;
        assert!(
            validate_cross_cutting_scenario_metrics(&missing_fee)
                .unwrap_err()
                .to_string()
                .contains("explicit fees")
        );

        let mut missing_balance = document;
        missing_balance["modes"]["old_wallet"]["scenarios"]["S0"]["repetitions"][0]["metrics"] =
            serde_json::json!({});
        assert!(
            validate_cross_cutting_scenario_metrics(&missing_balance)
                .unwrap_err()
                .to_string()
                .contains("final balance reconciliation")
        );
    }

    #[test]
    fn malformed_profile_missing_key_fails_schema() {
        let mut document = profile_document();
        document.as_object_mut().unwrap().remove("run_id");
        assert!(validate_document(&document, false).is_err());
    }

    #[test]
    fn invalid_confirmation_row_fails_schema() {
        let mut document = profile_document();
        document["chain_verification"]["verified_transactions"] = json!([{
            "tx_id": "42", "status_value": 2, "mode": "old_wallet", "scenario": "S1",
            "mined_height": 100, "confirmations": 3, "min_confirmations": 3,
            "tip_height": 103, "confirmed": false
        }]);
        assert!(validate_document(&document, false).is_err());
    }

    #[test]
    fn incomplete_final_is_rejected() {
        let mut document = profile_document();
        document["profile_kind"] = json!("final");
        document["run_complete"] = json!(true);
        assert!(validate_document(&document, false).is_err());
    }

    #[test]
    fn summary_is_deterministic() {
        let document = profile_document();
        assert_eq!(
            render_summary(&document).unwrap(),
            render_summary(&document).unwrap()
        );
    }

    #[test]
    fn incomplete_s5_arms_never_create_comparison() {
        let config = Config::default();
        let mut profile = ResultProfile::new(&config, env_capture::capture());
        profile.refresh_computed_deltas();
        let comparisons = &profile.computed_deltas["s5_throughput"]["comparisons"];
        assert!(
            comparisons
                .as_object()
                .unwrap()
                .values()
                .all(Value::is_null)
        );
    }

    #[test]
    fn outcome_status_enum_remains_stable() {
        assert_eq!(
            serde_json::to_value(crate::result_profile::OutcomeStatus::Success).unwrap(),
            json!("success")
        );
    }
}
