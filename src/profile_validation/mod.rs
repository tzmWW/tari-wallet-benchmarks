use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use anyhow::{Context, bail};
use serde_json::{Value, json};

use super::{
    CellStatus, ProfileKind, REFERENCE_BASE_NODE_REVISION, RESULT_SCHEMA_VERSION, ResultProfile,
};
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
            "provenance", "completed_stages", "generated_at", "network",
            "base_node", "environment", "versions", "config", "funding", "modes",
            "computed_deltas", "findings", "chain_verification"
        ],
        "properties": {
            "schema_version": {"const": RESULT_SCHEMA_VERSION},
            "run_id": {"type": "string", "minLength": 1},
            "profile_kind": {"enum": ["checkpoint", "final"]},
            "run_complete": {"type": "boolean"},
            "provenance": {"$ref": "#/$defs/provenance"},
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
            "provenance": {
                "type": "object", "additionalProperties": false,
                "required": ["measurement_commit", "export_commit", "measurement_build_manifest", "export_build_manifest"],
                "properties": {
                    "measurement_commit": {"type": "string", "minLength": 1},
                    "export_commit": {"type": "string", "minLength": 1},
                    "measurement_build_manifest": {"$ref": "#/$defs/build_manifest"},
                    "export_build_manifest": {"$ref": "#/$defs/build_manifest"},
                    "correction": {"$ref": "#/$defs/profile_correction"}
                }
            },
            "profile_correction": {
                "type": "object", "additionalProperties": false,
                "required": ["manifest_schema_version", "manifest_path", "tool", "tool_version", "corrected_at", "raw_profile_sha256", "raw_profile_size", "transformations"],
                "properties": {
                    "manifest_schema_version": {"type": "integer", "minimum": 1},
                    "manifest_path": {"type": "string", "minLength": 1},
                    "tool": {"type": "string", "minLength": 1},
                    "tool_version": {"type": "string", "minLength": 1},
                    "corrected_at": {"type": "string", "format": "date-time"},
                    "raw_profile_sha256": {"type": "string", "pattern": "^[0-9a-fA-F]{64}$"},
                    "raw_profile_size": {"type": "integer", "minimum": 1},
                    "transformations": {"type": "array", "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["pointer", "value"],
                        "properties": {
                            "pointer": {"type": "string", "minLength": 1, "pattern": "^/"},
                            "value": {}
                        }
                    }}
                }
            },
            "build_manifest": {
                "type": "object", "additionalProperties": false,
                "required": ["schema_version", "artifacts"],
                "properties": {
                    "schema_version": {"type": "integer", "minimum": 1},
                    "sources": {"type": "object"},
                    "artifacts": {"type": "object", "minProperties": 1, "additionalProperties": {"$ref": "#/$defs/build_artifact"}},
                    "payment_processor_patch_sha256": {"type": "string", "minLength": 1}
                }
            },
            "build_artifact": {
                "type": "object", "additionalProperties": true,
                "required": ["source_revision", "sha256"],
                "properties": {
                    "source_revision": {"type": "string", "minLength": 1}, "sha256": {"type": "string", "minLength": 1}
                }
            },
            "base_node": {
                "type": "object", "additionalProperties": false,
                "required": [
                    "endpoint", "authority_endpoint", "configured_revision", "observed_version", "version_observable",
                    "tip_start_height", "tip_start_hash", "tip_end_height", "tip_end_hash",
                    "pruning_horizon", "is_synced", "authority_tip_start_height",
                    "authority_tip_start_hash", "authority_tip_end_height", "authority_tip_end_hash"
                ],
                "properties": {
                    "endpoint": {"type": "string", "minLength": 1},
                    "authority_endpoint": {"type": "string", "minLength": 1},
                    "configured_revision": {"type": "string", "minLength": 1},
                    "observed_version": {"$ref": "#/$defs/nullable_string"},
                    "version_observable": {"type": "boolean"},
                    "tip_start_height": {"$ref": "#/$defs/nullable_integer"},
                    "tip_start_hash": {"$ref": "#/$defs/nullable_string"},
                    "tip_end_height": {"$ref": "#/$defs/nullable_integer"},
                    "tip_end_hash": {"$ref": "#/$defs/nullable_string"},
                    "pruning_horizon": {"$ref": "#/$defs/nullable_integer"},
                    "is_synced": {"type": ["boolean", "null"]},
                    "authority_tip_start_height": {"$ref": "#/$defs/nullable_integer"},
                    "authority_tip_start_hash": {"$ref": "#/$defs/nullable_string"},
                    "authority_tip_end_height": {"$ref": "#/$defs/nullable_integer"},
                    "authority_tip_end_hash": {"$ref": "#/$defs/nullable_string"}
                }
            },
            "environment": {
                "type": "object", "additionalProperties": false,
                "required": ["os", "cpu_brand", "physical_cores", "total_memory_bytes", "base_node_network_path", "authority_network_path"],
                "properties": {
                    "os": {"type": "string"},
                    "cpu_brand": {"type": "string"},
                    "physical_cores": {"$ref": "#/$defs/nullable_integer"},
                    "total_memory_bytes": {"type": "integer", "minimum": 0},
                    "disk_kind": {"type": "string"},
                    "disk_name": {"type": "string"},
                    "base_node_host": {"type": "string"},
                    "base_node_network_path": {"enum": ["local", "remote", "unknown"]},
                    "authority_host": {"type": "string"},
                    "authority_network_path": {"enum": ["local", "remote", "unknown"]},
                    "mode1_base_node_service_peer": {"type": "string"}
                }
            },
            "config": {
                "type": "object",
                "required": [
                    "A_fund", "C_min", "volume_target", "doubling_rounds",
                    "fanout_outputs_per_tx", "concurrent_batches", "S4_T_budget_secs",
                    "S5_M", "S5_K", "fee_rate", "scan_repetitions",
                    "protocol_fingerprint"
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
                    "scan_repetitions": {"type": "integer", "minimum": 1},
                    "protocol_fingerprint": {"type": "string", "minLength": 64}
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
                    "tip_height_at_broadcast": {"$ref": "#/$defs/nullable_integer"},
                    "tip_height_at_confirmation": {"$ref": "#/$defs/nullable_integer"},
                    "shared_funding_fee_microtari": {"$ref": "#/$defs/nullable_integer"},
                    "funding_fee_attribution": {"$ref": "#/$defs/nullable_string"}
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
                            "s4_arms": {"type": "array", "items": {"$ref": "#/$defs/s4_arm"}},
                            "s5_arms": {"type": "object", "additionalProperties": true}
                        }
                    }
                }
            },
            "transaction_observation": {
                "type": "object", "additionalProperties": false,
                "required": [
                    "transaction_id", "attempt_index", "batch_index", "submit_offset_ms",
                    "construction_complete_offset_ms", "construction_ms", "submission_ms",
                    "mempool_available", "mempool_reason", "confirmation_ms",
                    "confirmation_timing_reason", "fee_microtari", "terminal_outcome", "error",
                    "mined_height", "tip_start_height", "tip_end_height", "input_count",
                    "total_output_count", "payment_output_count", "change_output_count",
                    "output_commitments", "configured_batch"
                ],
                "properties": {
                    "transaction_id": {"$ref": "#/$defs/nullable_string"},
                    "attempt_index": {"$ref": "#/$defs/nullable_integer"},
                    "batch_index": {"$ref": "#/$defs/nullable_integer"},
                    "submit_offset_ms": {"$ref": "#/$defs/nullable_integer"},
                    "construction_complete_offset_ms": {"$ref": "#/$defs/nullable_integer"},
                    "broadcast_start_offset_ms": {"$ref": "#/$defs/nullable_integer"},
                    "construction_ms": {"$ref": "#/$defs/nullable_integer"},
                    "construction_timing_origin": {"$ref": "#/$defs/nullable_string"},
                    "construction_timing_reason": {"$ref": "#/$defs/nullable_string"},
                    "submission_ms": {"$ref": "#/$defs/nullable_integer"},
                    "submission_timing_origin": {"$ref": "#/$defs/nullable_string"},
                    "mempool_available": {"type": ["boolean", "null"]},
                    "mempool_reason": {"$ref": "#/$defs/nullable_string"},
                    "confirmation_ms": {"$ref": "#/$defs/nullable_integer"},
                    "confirmation_timing_origin": {"$ref": "#/$defs/nullable_string"},
                    "confirmation_timing_reason": {"$ref": "#/$defs/nullable_string"},
                    "fee_microtari": {"$ref": "#/$defs/nullable_integer"},
                    "fee_unavailable_reason": {"$ref": "#/$defs/nullable_string"},
                    "fee_disposition": {"enum": ["confirmed_paid", "proposed_unresolved", "rejected", "unavailable"]},
                    "recipient": {"$ref": "#/$defs/nullable_string"},
                    "recipients": {"type": "array", "items": {"type": "string"}},
                    "api_accepted": {"type": ["boolean", "null"]},
                    "api_error": {"$ref": "#/$defs/nullable_string"},
                    "http_status": {"type": ["integer", "null"], "minimum": 100, "maximum": 599},
                    "failure_class": {"enum": ["http_response", "request_timeout", "arm_deadline", "transport", "response_decode", "response_shape", "database", "process", "chain_target"]},
                    "terminal_outcome": {"enum": ["confirmed", "rejected", "stalled", "timed_out", "unavailable"]},
                    "error": {"$ref": "#/$defs/nullable_string"},
                    "mined_height": {"$ref": "#/$defs/nullable_integer"},
                    "tip_start_height": {"$ref": "#/$defs/nullable_integer"},
                    "tip_end_height": {"$ref": "#/$defs/nullable_integer"},
                    "input_count": {"$ref": "#/$defs/nullable_integer"},
                    "total_output_count": {"$ref": "#/$defs/nullable_integer"},
                    "payment_output_count": {"$ref": "#/$defs/nullable_integer"},
                    "change_output_count": {"$ref": "#/$defs/nullable_integer"},
                    "output_commitments": {"type": "array", "items": {"type": "string"}},
                    "configured_batch": {"$ref": "#/$defs/nullable_integer"}
                }
            },
            "s4_arm": {
                "type": "object", "additionalProperties": false,
                "required": ["configured_batch", "wall_ms", "attempted", "accepted", "confirmed", "rejected", "stalled", "timed_out", "confirmed_success_rate", "api_acceptance_rate", "max_serialization_gap_ms", "serialization_timing_reason", "recipients", "distinct_recipient_count", "recipients_are_distinct"],
                "properties": {
                    "configured_batch": {"type": "integer", "minimum": 1},
                    "wall_ms": {"type": "integer", "minimum": 0},
                    "attempted": {"type": "integer", "minimum": 0},
                    "accepted": {"type": "integer", "minimum": 0},
                    "confirmed": {"type": "integer", "minimum": 0},
                    "rejected": {"type": "integer", "minimum": 0},
                    "stalled": {"type": "integer", "minimum": 0},
                    "timed_out": {"type": "integer", "minimum": 0},
                    "confirmed_success_rate": {"type": ["number", "null"], "minimum": 0, "maximum": 1},
                    "api_acceptance_rate": {"type": ["number", "null"], "minimum": 0, "maximum": 1},
                    "max_serialization_gap_ms": {"$ref": "#/$defs/nullable_integer"},
                    "serialization_timing_reason": {"$ref": "#/$defs/nullable_string"},
                    "recipients": {"type": "array", "items": {"type": "string"}},
                    "distinct_recipient_count": {"type": "integer", "minimum": 0},
                    "recipients_are_distinct": {"type": "boolean"}
                }
            },
            "scenario": {
                "type": "object", "additionalProperties": false,
                "required": [
                    "scenario", "surface", "execution_status", "outcome_status", "repetitions",
                    "median_wall_ms", "spread_wall_ms", "all_runs_median_wall_ms", "notes"
                ],
                "properties": {
                    "scenario": {"enum": ["b0", "s0", "s1", "s2", "s3", "s4", "s5", "s6", "s7"]},
                    "surface": {"type": "string", "minLength": 1},
                    "execution_status": {"$ref": "#/$defs/execution_status"},
                    "outcome_status": {"$ref": "#/$defs/outcome_status"},
                    "repetitions": {"type": "array", "items": {"$ref": "#/$defs/repetition"}},
                    "median_wall_ms": {"$ref": "#/$defs/nullable_integer"},
                    "spread_wall_ms": {"$ref": "#/$defs/nullable_integer"},
                    "all_runs_median_wall_ms": {"$ref": "#/$defs/nullable_integer"},
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

pub fn validate_legacy_v5_path(path: &Path) -> anyhow::Result<()> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let document: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing {} as JSON", path.display()))?;
    validate_legacy_v5_document(&document)
}

fn validate_legacy_v5_document(document: &Value) -> anyhow::Result<()> {
    if document["schema_version"] != json!(5) {
        bail!("legacy validation requires schema_version 5");
    }
    if document["profile_kind"] != json!("final") || document["run_complete"] != json!(true) {
        bail!("legacy schema-v5 profile must be a complete final profile");
    }
    for key in [
        "run_id",
        "harness_git_commit",
        "generated_at",
        "network",
        "base_node",
        "environment",
        "versions",
        "config",
        "funding",
        "modes",
        "computed_deltas",
        "findings",
        "chain_verification",
    ] {
        if document.get(key).is_none() {
            bail!("legacy schema-v5 profile is missing {key}");
        }
    }
    let modes = document["modes"]
        .as_object()
        .context("legacy schema-v5 modes must be an object")?;
    for mode in ModeName::ALL {
        let scenarios = modes
            .get(mode.as_str())
            .and_then(|value| value["scenarios"].as_object())
            .with_context(|| format!("legacy schema-v5 missing {}/scenarios", mode.as_str()))?;
        for scenario in ScenarioName::ALL {
            if !scenarios.contains_key(scenario.as_str()) {
                bail!(
                    "legacy schema-v5 missing {}/{}",
                    mode.as_str(),
                    scenario.as_str()
                );
            }
        }
    }
    Ok(())
}

pub fn validate_document(document: &Value, submission: bool) -> anyhow::Result<ResultProfile> {
    validate_schema(document)?;
    let profile: ResultProfile = serde_json::from_value(document.clone())
        .context("deserializing schema-v6 result profile")?;
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
    if profile.computed_deltas != super::computed_deltas(profile) {
        bail!("computed_deltas do not match recomputed source scenario metrics");
    }
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
        bail!("profile does not match schema v6:\n{}", errors.join("\n"));
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
                    if repetition["execution_status"] == "blocked_prerequisite"
                        && (!repetition["wall_ms"].is_null()
                            || repetition["success_count"] != 0
                            || repetition["failure_count"] != 0
                            || !repetition["fee_microtari"].is_null()
                            || !repetition["error"].is_null())
                    {
                        bail!(
                            "blocked repetition {mode}/{scenario} must not contain measured wall, fee, outcome counts, or error"
                        );
                    }
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
        ("fee_rate", json!("5 uT")),
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
        for required in ["B0", "S0"] {
            if scenarios[required]["execution_status"] != "completed"
                || scenarios[required]["outcome_status"] != "success"
            {
                bail!("submission requires successful {required} for {mode}");
            }
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
    if profile.provenance.measurement_commit == "unknown"
        || profile.provenance.export_commit == "unknown"
    {
        bail!("submission profile must identify measurement and export commits");
    }
    if !is_complete_build_manifest(&profile.provenance.measurement_build_manifest)
        || !is_complete_build_manifest(&profile.provenance.export_build_manifest)
        || !is_complete_build_manifest(&profile.config["build_manifest"])
        || !profile.config["seed_fingerprints"].is_object()
    {
        bail!("submission profile must record build-manifest and seed fingerprints");
    }
    if profile.environment.authority_network_path != "remote" {
        bail!("submission requires an independent remote Esmeralda authority endpoint");
    }
    if profile.environment.base_node_network_path != "local"
        || profile.environment.mode1_base_node_service_peer.is_none()
    {
        bail!("submission requires an archival local scan endpoint and Mode 1 P2P service peer");
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
        || profile.base_node.authority_tip_start_height.is_none()
        || profile
            .base_node
            .authority_tip_start_hash
            .as_deref()
            .is_none_or(str::is_empty)
        || profile.base_node.authority_tip_end_height.is_none()
        || profile
            .base_node
            .authority_tip_end_hash
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
    validate_confirmed_observation_bindings(document, profile)?;
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
            || funding.tip_height_at_broadcast.is_none()
            || funding.tip_height_at_confirmation.is_none()
        {
            bail!(
                "submission funding for {} is missing measured S0 timing/tip evidence",
                mode.as_str()
            );
        }
        if funding.shared_funding_fee_microtari.is_none()
            || funding.funding_fee_attribution.as_deref()
                != Some("external_source_shared_not_deducted_from_mode_balance")
        {
            bail!(
                "submission funding for {} is missing shared fee attribution",
                mode.as_str()
            );
        }
        let s0 = &profile.modes[mode.as_str()].scenarios["S0"];
        if s0.status == CellStatus::Ok {
            for (repetition, run) in
                document["modes"][mode.as_str()]["scenarios"]["S0"]["repetitions"]
                    .as_array()
                    .expect("schema checked")
                    .iter()
                    .enumerate()
            {
                if run["execution_status"] != "completed" {
                    continue;
                }
                let metrics = run["metrics"].as_object().with_context(|| {
                    format!(
                        "submission S0 repetition {} metrics missing for {}",
                        repetition + 1,
                        mode.as_str()
                    )
                })?;
                if metrics["verification_source"] != json!("wallet_state_observed")
                    || metrics["expected_spendable_count"] != json!(1)
                    || metrics["observed_spendable_count"] != json!(1)
                    || metrics["expected_available_microtari"]
                        != json!(crate::config::parse_amount("10000 T")?.0)
                    || metrics["available_microtari"]
                        != json!(crate::config::parse_amount("10000 T")?.0)
                    || metrics["spendable_count_matches_expected"] != json!(true)
                    || metrics["balance_matches_expected"] != json!(true)
                    || metrics["wallet_state_complete"] != json!(true)
                    || metrics["pending_outputs"] != json!(0)
                    || metrics["locked_outputs"] != json!(0)
                    || metrics["invalid_outputs"] != json!(0)
                    || metrics["unknown_outputs"] != json!(0)
                {
                    bail!(
                        "submission S0 repetition {} for {} does not prove exact one-UTXO A_fund state",
                        repetition + 1,
                        mode.as_str()
                    );
                }
                let funding_observation =
                    metrics.get("funding_observation").with_context(|| {
                        format!(
                            "successful submission S0 repetition {} is missing funding observation",
                            repetition + 1
                        )
                    })?;
                if funding_observation["tx_id"] != json!(funding.tx_id)
                    || funding_observation["mined_height"] != json!(funding.height)
                    || funding_observation["shared_funding_fee_microtari"]
                        != json!(funding.shared_funding_fee_microtari)
                {
                    bail!(
                        "submission S0 repetition {} funding observation does not match funding record for {}",
                        repetition + 1,
                        mode.as_str()
                    );
                }
            }
        }
        if profile.modes[mode.as_str()].scenarios["S1"].status == CellStatus::Ok
            && !profile
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
    let funding_records = ModeName::ALL
        .into_iter()
        .map(|mode| {
            profile
                .funding
                .get(mode.as_str())
                .with_context(|| format!("submission funding is missing {}", mode.as_str()))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let first = funding_records.first().expect("three benchmark modes");
    let expected_amount = crate::config::parse_amount("10000 T")?.0;
    if funding_records.iter().any(|funding| {
        funding.tx_id != first.tx_id
            || funding.height != first.height
            || funding.amount != "10000 T"
            || crate::config::parse_amount(&funding.amount)
                .map(|amount| amount.0 != expected_amount)
                .unwrap_or(true)
            || funding.shared_funding_fee_microtari != first.shared_funding_fee_microtari
            || funding.tip_height_at_broadcast != first.tip_height_at_broadcast
            || funding.construction_ms != first.construction_ms
            || funding.broadcast_to_mempool_ms != first.broadcast_to_mempool_ms
            || funding.broadcast_to_confirmed_at_c_min_ms
                != first.broadcast_to_confirmed_at_c_min_ms
            || funding.tip_height_at_confirmation != first.tip_height_at_confirmation
    }) {
        bail!("submission funding records must describe one shared exact A_fund transaction");
    }
    Ok(())
}

fn validate_confirmed_observation_bindings(
    document: &Value,
    profile: &ResultProfile,
) -> anyhow::Result<()> {
    let mut observed = BTreeMap::<(String, String, String), usize>::new();
    for mode in ModeName::ALL {
        for scenario in ScenarioName::ALL {
            for (repetition, run) in document["modes"][mode.as_str()]["scenarios"]
                [scenario.as_str()]["repetitions"]
                .as_array()
                .expect("schema checked")
                .iter()
                .enumerate()
            {
                let Some(observations) = run["metrics"]
                    .get("transaction_observations")
                    .and_then(Value::as_array)
                else {
                    continue;
                };
                for observation in observations
                    .iter()
                    .filter(|value| value["terminal_outcome"] == "confirmed")
                {
                    let tx_id = observation["transaction_id"].as_str().with_context(|| {
                        format!(
                            "confirmed {}/{} repetition {} observation lacks tx id",
                            mode.as_str(),
                            scenario.as_str(),
                            repetition + 1
                        )
                    })?;
                    let key = (
                        mode.as_str().to_string(),
                        scenario.as_str().to_string(),
                        tx_id.to_string(),
                    );
                    *observed.entry(key).or_default() += 1;
                }
            }
        }
    }
    let mut chain = BTreeMap::<(String, String, String), usize>::new();
    for transaction in &profile.chain_verification.verified_transactions {
        if !transaction.confirmed {
            bail!("top-level chain verification contains an unconfirmed transaction");
        }
        let key = (
            transaction.mode.clone(),
            transaction.scenario.clone(),
            transaction.tx_id.clone(),
        );
        *chain.entry(key).or_default() += 1;
    }
    if observed != chain {
        bail!("confirmed transaction observations and top-level chain rows are not one-to-one");
    }
    Ok(())
}

fn is_complete_build_manifest(value: &Value) -> bool {
    value
        .get("schema_version")
        .and_then(Value::as_u64)
        .is_some_and(|version| version >= 1)
        && value
            .get("artifacts")
            .and_then(Value::as_object)
            .is_some_and(|artifacts| {
                !artifacts.is_empty()
                    && artifacts.values().all(|artifact| {
                        artifact
                            .get("source_revision")
                            .and_then(Value::as_str)
                            .is_some_and(|value| !value.is_empty())
                            && artifact
                                .get("sha256")
                                .and_then(Value::as_str)
                                .is_some_and(|value| !value.is_empty())
                    })
            })
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
                let confirmed_fee_total = metrics
                    .get("transaction_observations")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|observation| observation["terminal_outcome"] == "confirmed")
                    .map(|observation| {
                        observation["fee_microtari"].as_u64().with_context(|| {
                            format!("submission {label} confirmed observation lacks fee")
                        })
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?
                    .into_iter()
                    .try_fold(0u64, |total, fee| total.checked_add(fee))
                    .with_context(|| format!("submission {label} confirmed fee total overflows"))?;
                if run["fee_microtari"] != json!(confirmed_fee_total) {
                    bail!(
                        "submission {label} fee_microtari does not equal confirmed observation fees"
                    );
                }
                validate_balance_reconciliation(metrics, &label, run["fee_microtari"].as_u64())?;
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

fn validate_balance_reconciliation(
    metrics: &serde_json::Map<String, Value>,
    label: &str,
    fee_microtari: Option<u64>,
) -> anyhow::Result<()> {
    let Some(reconciliation) = metrics.get("balance_reconciliation") else {
        return Ok(());
    };
    let fee = fee_microtari.context("balance reconciliation requires a recorded fee")?;
    let outgoing = metrics
        .get("outgoing_microtari")
        .and_then(Value::as_u64)
        .context("balance reconciliation requires outgoing_microtari")?;
    let (before, after, domain) = if let (Some(before), Some(after)) = (
        metrics
            .get("balance_before_microtari")
            .and_then(Value::as_u64),
        metrics
            .get("balance_after_microtari")
            .and_then(Value::as_u64),
    ) {
        (before, after, "available")
    } else {
        let before = metrics
            .get("balance_before")
            .and_then(|value| value.get("total"))
            .and_then(Value::as_u64)
            .context("balance reconciliation requires balance_before.total")?;
        let after = metrics
            .get("balance_after")
            .and_then(|value| value.get("total"))
            .and_then(Value::as_u64)
            .context("balance reconciliation requires balance_after.total")?;
        (before, after, "total")
    };
    let deduction = outgoing
        .checked_add(fee)
        .context("balance reconciliation outgoing amount and fee overflow")?;
    let expected = before
        .checked_sub(deduction)
        .with_context(|| format!("{label} balance reconciliation underflows {domain} balance"))?;
    let delta = i128::from(expected) - i128::from(after);
    if reconciliation["expected_balance_microtari"] != json!(expected)
        || reconciliation["observed_balance_microtari"] != json!(after)
        || reconciliation["delta_microtari"] != json!(delta)
        || reconciliation["flagged"] != json!(delta != 0)
        || reconciliation["balance_domain"].as_str() != Some(domain)
    {
        bail!("{label} balance reconciliation arithmetic is inconsistent");
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
    let mut b0_target: Option<(u64, &str)> = None;
    for (mode, scenarios) in mode_scenarios(document)? {
        let b0 = &scenarios["B0"]["repetitions"][0]["metrics"];
        let target_height = b0["H_tip_end"]
            .as_u64()
            .with_context(|| format!("submission B0 target height missing for {mode}"))?;
        let target_hash = b0["H_tip_target_hash"]
            .as_str()
            .with_context(|| format!("submission B0 target hash missing for {mode}"))?;
        if let Some(expected) = b0_target {
            if expected != (target_height, target_hash) {
                bail!("submission B0 modes do not share one target height and hash");
            }
        } else {
            b0_target = Some((target_height, target_hash));
        }
        if b0["birthday"] != 0
            || b0["detected_outputs"] != 0
            || b0["spendable_outputs"] != 0
            || b0["available_microtari"] != 0
            || b0["history_transactions"] != 0
            || b0["max_height"] != b0["H_tip_end"]
            || b0["tip_lag_tolerance_blocks"] != 0
            || b0["scan_reached_tip"] != true
            || b0["H_scan_cursor_hash"].as_str() != Some(target_hash)
            || b0["H_tip_completion"].as_u64().is_none()
            || b0["H_tip_completion_hash"].as_str().is_none()
        {
            bail!("submission B0 exact empty-tip contract failed for {mode}");
        }
        let funding = &document["funding"][mode];
        if funding["height"]
            .as_u64()
            .is_none_or(|height| height <= target_height)
            || funding["tip_height_at_broadcast"]
                .as_u64()
                .is_none_or(|height| height <= target_height)
        {
            bail!("submission funding for {mode} did not occur after the shared B0 target");
        }
        for key in [
            "T_scan_ms",
            "blocks_per_sec",
            "H_tip_start",
            "H_tip_end",
            "peak_rss_bytes",
            "peak_cpu_percent",
            "scan_invocations",
        ] {
            if !b0[key].is_number() {
                bail!("submission B0 metric {key} missing for {mode}");
            }
        }
        for scenario in ["S1", "S4", "S5"] {
            if scenarios[scenario]["execution_status"] != "completed" {
                continue;
            }
            let metrics = &scenarios[scenario]["repetitions"][0]["metrics"];
            let start_tip = metrics["scenario_tip_start_height"].as_u64();
            let end_tip = metrics["scenario_tip_end_height"].as_u64();
            if start_tip.is_none() || end_tip.is_none() || end_tip < start_tip {
                bail!("submission {mode}/{scenario} is missing coherent scenario tip anchors");
            }
            let before = metrics
                .get("balance_before")
                .or_else(|| metrics["extra"].get("balance_before"));
            let after = metrics
                .get("balance_after")
                .or_else(|| metrics["extra"].get("balance_after"));
            if before.is_none_or(Value::is_null) || after.is_none_or(Value::is_null) {
                bail!("submission {mode}/{scenario} is missing balance components");
            }
            let required_balance_keys: &[&str] = if mode == "old_wallet" {
                &[
                    "available",
                    "pending_incoming",
                    "pending_outgoing",
                    "timelocked",
                ]
            } else {
                &["total", "available", "locked", "unconfirmed", "immature"]
            };
            for balance in [
                before.expect("checked above"),
                after.expect("checked above"),
            ] {
                if required_balance_keys.iter().any(|key| {
                    balance
                        .get(*key)
                        .is_none_or(|value| !is_amount_value(value))
                }) {
                    bail!("submission {mode}/{scenario} balance components are incomplete");
                }
            }
        }

        let s1_metrics = &scenarios["S1"]["repetitions"][0]["metrics"];
        if scenarios["S1"]["outcome_status"] == "success" {
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
            if s1_metrics["transaction_observations"]
                .as_array()
                .is_none_or(|observations| observations.len() != 127)
            {
                bail!(
                    "submission {mode}/S1 must contain one observation for each of 127 transactions"
                );
            }
            let s1_observations = s1_metrics["transaction_observations"]
                .as_array()
                .expect("checked above");
            let two_output = s1_observations
                .iter()
                .filter(|observation| {
                    observation["input_count"] == 1 && observation["total_output_count"] == 2
                })
                .count();
            let eight_output = s1_observations
                .iter()
                .filter(|observation| {
                    observation["input_count"] == 1 && observation["total_output_count"] == 8
                })
                .count();
            if two_output != 63 || eight_output != 64 {
                bail!(
                    "submission {mode}/S1 does not prove 63 one-to-two and 64 one-to-eight transactions"
                );
            }
        } else {
            validate_transaction_observations(mode, "S1", s1_metrics)?;
        }

        if scenarios["S4"]["execution_status"] == "completed" {
            let s4 = &scenarios["S4"]["repetitions"][0]["metrics"];
            let batch_summaries = s4["batch_summaries"].as_array();
            let arms = s4["s4_arms"].as_array();
            if batch_summaries.is_none()
                || arms.is_none()
                || s4["double_selection_rejections"].as_u64().is_none()
            {
                bail!("submission {mode}/S4 is missing concurrency batch metrics");
            }
            let batch_summaries = batch_summaries.expect("checked above");
            if batch_summaries.len() != REFERENCE_S4_RAMP.len()
                || batch_summaries
                    .iter()
                    .zip(REFERENCE_S4_RAMP)
                    .any(|(summary, expected)| {
                        summary["configured_batch"].as_u64() != Some(expected)
                            || summary
                                .get("attempted")
                                .or_else(|| summary.get("attempted_batches"))
                                .and_then(Value::as_u64)
                                != Some(expected)
                            || summary["wall_ms"].as_u64().is_none()
                    })
            {
                bail!("submission {mode}/S4 does not contain the exact 8,16,32,64,128 arms");
            }
            validate_transaction_observations(mode, "S4", s4)?;
            if s4["transaction_observations"]
                .as_array()
                .is_none_or(|observations| observations.len() != 248)
            {
                bail!("submission {mode}/S4 must contain one observation for each of 248 attempts");
            }
            let observations = s4["transaction_observations"]
                .as_array()
                .expect("checked above");
            for configured_batch in REFERENCE_S4_RAMP {
                if observations
                    .iter()
                    .filter(|observation| {
                        observation["configured_batch"].as_u64() == Some(configured_batch)
                    })
                    .count()
                    != configured_batch as usize
                {
                    bail!(
                        "submission {mode}/S4 observations are not bound to configured arm {configured_batch}"
                    );
                }
            }
            let arms = arms.expect("checked above");
            if arms.len() != REFERENCE_S4_RAMP.len() {
                bail!("submission {mode}/S4 is missing per-arm metrics");
            }
            for (arm, configured_batch) in arms.iter().zip(REFERENCE_S4_RAMP) {
                let attempted = arm["attempted"].as_u64().unwrap_or_default();
                let accepted = arm["accepted"].as_u64().unwrap_or_default();
                let confirmed = arm["confirmed"].as_u64().unwrap_or_default();
                let terminal = confirmed
                    + arm["rejected"].as_u64().unwrap_or_default()
                    + arm["stalled"].as_u64().unwrap_or_default()
                    + arm["timed_out"].as_u64().unwrap_or_default();
                if arm["configured_batch"] != configured_batch
                    || attempted != configured_batch
                    || terminal != attempted
                    || accepted > attempted
                    || arm["recipients_are_distinct"] != true
                    || arm["distinct_recipient_count"] != configured_batch
                {
                    bail!(
                        "submission {mode}/S4 arm {configured_batch} has inconsistent counts or recipients"
                    );
                }
                let expected_success = confirmed as f64 / attempted as f64;
                let expected_accept = accepted as f64 / attempted as f64;
                if (arm["confirmed_success_rate"].as_f64().unwrap_or(-1.0) - expected_success).abs()
                    > f64::EPSILON
                    || (arm["api_acceptance_rate"].as_f64().unwrap_or(-1.0) - expected_accept).abs()
                        > f64::EPSILON
                {
                    bail!("submission {mode}/S4 arm {configured_batch} has inconsistent rates");
                }
                if mode == "new_wallet"
                    && accepted >= 2
                    && arm["max_serialization_gap_ms"].as_u64().is_none()
                {
                    bail!(
                        "submission new_wallet/S4 arm {configured_batch} lacks serialization evidence"
                    );
                }
                if mode != "new_wallet"
                    && arm["serialization_timing_reason"]
                        .as_str()
                        .unwrap_or_default()
                        .is_empty()
                {
                    bail!(
                        "submission {mode}/S4 arm {configured_batch} lacks unavailable timing reason"
                    );
                }
            }
        }

        if scenarios["S5"]["execution_status"] == "completed" {
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
            let unique = set
                .iter()
                .filter_map(Value::as_str)
                .collect::<BTreeSet<_>>();
            if unique.len() != 100 {
                bail!("submission S5 recipient set must contain 100 unique addresses");
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
            for (repetition, run) in scenarios["S5"]["repetitions"]
                .as_array()
                .expect("schema checked")
                .iter()
                .enumerate()
            {
                let metrics = &run["metrics"];
                let Some(observations) = metrics["transaction_observations"].as_array() else {
                    continue;
                };
                let expected = set
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<BTreeSet<_>>();
                let mut observed = BTreeSet::new();
                for observation in observations
                    .iter()
                    .filter(|observation| observation["terminal_outcome"] == "confirmed")
                {
                    let recipients = observation["recipients"]
                        .as_array()
                        .with_context(|| format!("submission {mode}/S5 repetition {} confirmed observation lacks recipients", repetition + 1))?;
                    if recipients.is_empty()
                        || recipients.iter().any(|recipient| {
                            recipient
                                .as_str()
                                .is_none_or(|recipient| !expected.contains(recipient))
                        })
                    {
                        bail!(
                            "submission {mode}/S5 repetition {} contains an unbound recipient",
                            repetition + 1
                        );
                    }
                    observed.extend(recipients.iter().filter_map(Value::as_str));
                }
                if scenarios["S5"]["outcome_status"] == "success" && observed != expected {
                    bail!(
                        "submission {mode}/S5 repetition {} does not cover exactly the declared recipient set",
                        repetition + 1
                    );
                }
            }
            let expected_s5_transactions = if mode == "payment_processor" { 10 } else { 100 };
            if s5["transaction_observations"]
                .as_array()
                .is_none_or(|observations| observations.len() != expected_s5_transactions)
            {
                bail!("submission {mode}/S5 has the wrong transaction observation count");
            }
            let expected_payment_outputs = if mode == "payment_processor" { 10 } else { 1 };
            if s5["transaction_observations"]
                .as_array()
                .expect("checked above")
                .iter()
                .filter(|observation| observation["terminal_outcome"] == "confirmed")
                .any(|observation| {
                    let total = observation["total_output_count"].as_u64();
                    let payments = observation["payment_output_count"].as_u64();
                    let change = observation["change_output_count"].as_u64();
                    observation["input_count"] != 1
                        || observation["payment_output_count"] != expected_payment_outputs
                        || total.is_none()
                        || change.is_none()
                        || change.is_some_and(|count| count > 1)
                        || total
                            != payments
                                .zip(change)
                                .map(|(payments, change)| payments + change)
                })
            {
                bail!(
                    "submission {mode}/S5 confirmed transaction has the wrong input/payment-output shape"
                );
            }
        }

        for scenario in ["S2", "S3", "S6", "S7"] {
            let cell = &scenarios[scenario];
            if cell["execution_status"] != "completed" {
                continue;
            }
            for run in cell["repetitions"].as_array().expect("schema checked") {
                let metrics = &run["metrics"];
                if run["outcome_status"] != "success" {
                    if metrics["H_tip_end"].as_u64().is_none()
                        || metrics["H_tip_target_hash"].as_str().is_none()
                        || metrics["max_height"].as_u64().is_none()
                        || metrics["peak_rss_bytes"].as_u64().is_none()
                        || metrics["peak_cpu_percent"].as_f64().is_none()
                    {
                        bail!(
                            "submission {mode}/{scenario} failed scan is missing target, final progress, or resource evidence"
                        );
                    }
                    continue;
                }
                if metrics["birthday"].as_u64()
                    != Some(if matches!(scenario, "S2" | "S6") {
                        0
                    } else {
                        profile_funding_birthday(document, mode)?
                    })
                    || metrics["max_height"] != metrics["H_tip_end"]
                    || metrics["tip_lag_tolerance_blocks"] != 0
                    || metrics["history_matches_expected"] != true
                    || metrics["history_identities_match_expected"] != true
                    || metrics["H_tip_target_hash"].as_str().is_none()
                    || metrics["H_scan_cursor_hash"] != metrics["H_tip_target_hash"]
                    || metrics["scan_invocations"].as_u64().is_none()
                    || metrics["H_tip_completion"].as_u64().is_none()
                    || metrics["H_tip_completion_hash"].as_str().is_none()
                    || metrics["expected_history_tx_ids"].as_array().is_none()
                    || metrics["expected_output_commitments"].as_object().is_none()
                    || metrics["missing_history_tx_ids"] != json!([])
                    || metrics["missing_history_output_tx_ids"] != json!([])
                    || metrics["resource_sampling_window"] != "scan_wall_window"
                    || metrics["resource_sampling_process"].as_str().is_none()
                {
                    bail!("submission {mode}/{scenario} scan contract failed");
                }
                if (mode == "old_wallet"
                    && metrics["expected_history_tx_ids"]
                        .as_array()
                        .is_none_or(Vec::is_empty))
                    || (mode != "old_wallet"
                        && metrics["expected_output_commitments"]
                            .as_object()
                            .is_none_or(serde_json::Map::is_empty))
                {
                    bail!("submission {mode}/{scenario} scan history expectation is empty");
                }
                validate_scan_history_evidence(document, mode, scenario, metrics)?;
            }
        }
    }
    Ok(())
}

fn validate_scan_history_evidence(
    document: &Value,
    mode: &str,
    scan_scenario: &str,
    metrics: &Value,
) -> anyhow::Result<()> {
    let source_scenarios: &[&str] = if matches!(scan_scenario, "S2" | "S3") {
        &["S1"]
    } else {
        &["S1", "S4", "S5"]
    };
    if mode == "old_wallet" {
        let expected = document["chain_verification"]["verified_transactions"]
            .as_array()
            .expect("schema checked")
            .iter()
            .filter(|tx| {
                tx["mode"] == mode
                    && tx["confirmed"] == true
                    && tx["scenario"]
                        .as_str()
                        .is_some_and(|scenario| source_scenarios.contains(&scenario))
            })
            .filter_map(|tx| tx["tx_id"].as_str().map(ToString::to_string))
            .collect::<BTreeSet<_>>();
        let reported = json_string_set(&metrics["expected_history_tx_ids"])?;
        let recovered = json_string_set(&metrics["recovered_history_tx_ids"])?;
        let missing = expected
            .difference(&recovered)
            .cloned()
            .collect::<BTreeSet<_>>();
        if reported != expected || json_string_set(&metrics["missing_history_tx_ids"])? != missing {
            bail!("submission {mode}/{scan_scenario} history transaction evidence is inconsistent");
        }
    } else {
        let expected = expected_observation_commitments(document, mode, source_scenarios)?;
        let reported = json_commitment_map(&metrics["expected_output_commitments"])?;
        let recovered = json_string_set(&metrics["recovered_output_commitments"])?;
        let missing = expected
            .iter()
            .filter(|(_, commitments)| commitments.is_disjoint(&recovered))
            .map(|(tx_id, _)| tx_id.clone())
            .collect::<BTreeSet<_>>();
        if reported != expected
            || json_string_set(&metrics["missing_history_output_tx_ids"])? != missing
        {
            bail!("submission {mode}/{scan_scenario} history commitment evidence is inconsistent");
        }
    }
    Ok(())
}

fn expected_observation_commitments(
    document: &Value,
    mode: &str,
    scenarios: &[&str],
) -> anyhow::Result<BTreeMap<String, BTreeSet<String>>> {
    let mut expected = BTreeMap::<String, BTreeSet<String>>::new();
    for scenario in scenarios {
        let observations = document["modes"][mode]["scenarios"][scenario]["repetitions"][0]
            ["metrics"]["transaction_observations"]
            .as_array()
            .with_context(|| format!("submission {mode}/{scenario} observations missing"))?;
        for observation in observations
            .iter()
            .filter(|observation| observation["terminal_outcome"] == "confirmed")
        {
            let tx_id = observation["transaction_id"]
                .as_str()
                .with_context(|| format!("submission {mode}/{scenario} confirmed tx id missing"))?;
            expected
                .entry(tx_id.to_string())
                .or_default()
                .extend(json_string_set(&observation["output_commitments"])?);
        }
    }
    Ok(expected)
}

fn json_string_set(value: &Value) -> anyhow::Result<BTreeSet<String>> {
    value
        .as_array()
        .context("expected an array of strings")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToString::to_string)
                .context("expected a string")
        })
        .collect()
}

fn json_commitment_map(value: &Value) -> anyhow::Result<BTreeMap<String, BTreeSet<String>>> {
    value
        .as_object()
        .context("expected a transaction commitment map")?
        .iter()
        .map(|(tx_id, commitments)| Ok((tx_id.clone(), json_string_set(commitments)?)))
        .collect()
}

fn is_amount_value(value: &Value) -> bool {
    value.as_u64().is_some()
        || value.as_str().is_some_and(|value| {
            value.parse::<u64>().is_ok() || crate::config::parse_amount(value).is_ok()
        })
        || value.as_object().is_some_and(|value| {
            value.get("value").and_then(Value::as_u64).is_some()
                || value.get("microtari").and_then(Value::as_u64).is_some()
        })
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
    if mode == "payment_processor" {
        let confirmed = observations
            .iter()
            .filter(|observation| observation["terminal_outcome"] == "confirmed")
            .collect::<Vec<_>>();
        if !confirmed.is_empty() {
            let identities = metrics["batch_chain_identities"]
                .as_array()
                .with_context(|| {
                    format!("submission {mode}/{scenario} lacks batch-to-chain identities")
                })?;
            for observation in confirmed {
                let tx_id = observation["transaction_id"].as_str().unwrap_or_default();
                if tx_id.is_empty()
                    || !identities
                        .iter()
                        .any(|identity| identity["transaction_id"] == tx_id)
                {
                    bail!(
                        "submission {mode}/{scenario} confirmed transaction lacks UUID-to-chain mapping"
                    );
                }
            }
        }
    }
    for observation in observations {
        let outcome = observation["terminal_outcome"].as_str().unwrap_or_default();
        let api_accepted = observation["api_accepted"].as_bool();
        let has_identity =
            observation["transaction_id"].is_string() || observation["batch_index"].is_number();
        if api_accepted == Some(false) && outcome != "rejected" {
            bail!("submission {mode}/{scenario} immediate API failure must be rejected");
        }
        if outcome == "stalled" && (api_accepted != Some(true) || !has_identity) {
            bail!(
                "submission {mode}/{scenario} stalled observation lacks accepted operation identity"
            );
        }
        if mode == "new_wallet" && observation["transaction_id"].is_string() {
            if observation["construction_ms"].as_u64().is_none()
                || observation["construction_timing_origin"] != "library_build_and_sign"
            {
                bail!("submission {mode}/{scenario} transaction is missing real construction time");
            }
        } else if mode != "new_wallet"
            && (!observation["construction_ms"].is_null()
                || observation["construction_timing_reason"]
                    .as_str()
                    .unwrap_or_default()
                    .is_empty())
        {
            bail!(
                "submission {mode}/{scenario} must disclose unavailable internal construction time"
            );
        }
        if scenario == "S4"
            && (observation["submit_offset_ms"].as_u64().is_none()
                || observation["construction_complete_offset_ms"]
                    .as_u64()
                    .is_none())
        {
            bail!("submission {mode}/{scenario} attempt is missing common-clock offsets");
        }
        if scenario == "S4"
            && observation["recipients"]
                .as_array()
                .is_none_or(Vec::is_empty)
        {
            bail!("submission {mode}/{scenario} attempt is missing recipient identity");
        }
        if mode == "new_wallet" && outcome == "confirmed" {
            if observation["broadcast_start_offset_ms"].as_u64().is_none()
                || observation["confirmation_ms"].as_u64().is_none()
                || observation["confirmation_timing_origin"]
                    != "wallet_broadcast_start_to_independent_c_min"
            {
                bail!(
                    "submission {mode}/{scenario} confirmed transaction has invalid timing origins"
                );
            }
        } else if mode != "new_wallet" && outcome == "confirmed" {
            let origin = observation["confirmation_timing_origin"]
                .as_str()
                .unwrap_or_default();
            if observation["confirmation_ms"].as_u64().is_none()
                || !matches!(
                    origin,
                    "grpc_dispatch_to_independent_c_min" | "pp_api_acceptance_to_independent_c_min"
                )
            {
                bail!(
                    "submission {mode}/{scenario} confirmed transaction lacks honest dispatch/API-to-C-min timing"
                );
            }
        }
        if outcome == "confirmed"
            && (observation["fee_microtari"].as_u64().is_none()
                || observation["mined_height"].as_u64().is_none()
                || observation["tip_end_height"].as_u64().is_none()
                || observation["output_commitments"]
                    .as_array()
                    .is_none_or(Vec::is_empty))
        {
            bail!(
                "submission {mode}/{scenario} confirmed transaction is missing timing/fee/tip evidence"
            );
        }
        if outcome != "confirmed" && observation["error"].as_str().unwrap_or_default().is_empty() {
            bail!("submission {mode}/{scenario} failed transaction is missing an error reason");
        }
        if observation["fee_microtari"].is_null()
            && observation["fee_unavailable_reason"]
                .as_str()
                .unwrap_or_default()
                .is_empty()
        {
            bail!("submission {mode}/{scenario} transaction with unknown fee lacks a reason");
        }
        let fee_disposition = observation["fee_disposition"].as_str().unwrap_or_default();
        if observation.get("fee_disposition").is_some()
            && outcome == "confirmed"
            && fee_disposition != "confirmed_paid"
        {
            bail!("submission {mode}/{scenario} confirmed transaction fee is not marked paid");
        }
        if observation.get("fee_disposition").is_some()
            && outcome == "rejected"
            && fee_disposition != "rejected"
        {
            bail!("submission {mode}/{scenario} rejected transaction has invalid fee disposition");
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
            .and_then(|metrics| {
                metrics
                    .get("unspent_after")
                    .or_else(|| metrics.get("extra")?.get("unspent_after"))
            })
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
        (
            "new_wallet_individual_fee_per_recipient_over_payment_processor_batch",
            ("new_wallet", "individual"),
            ("payment_processor", "batch"),
        ),
        (
            "old_wallet_individual_fee_per_recipient_over_payment_processor_batch",
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
    let fee_cases = [
        (
            "new_wallet_individual_fee_per_recipient_over_payment_processor_batch",
            ("new_wallet", "individual"),
            ("payment_processor", "batch"),
        ),
        (
            "old_wallet_individual_fee_per_recipient_over_payment_processor_batch",
            ("old_wallet", "individual"),
            ("payment_processor", "batch"),
        ),
    ];
    for (name, left, right) in fee_cases {
        let available = arm_complete(arms, left.0, left.1)
            && arm_complete(arms, right.0, right.1)
            && arms[left.0][left.1]["fee_per_recipient_microtari"]
                .as_f64()
                .is_some()
            && arms[right.0][right.1]["fee_per_recipient_microtari"]
                .as_f64()
                .is_some();
        if available == comparisons[name].is_null() {
            bail!("S5 fee comparison {name} does not match source fee availability");
        }
    }
    for mode in ["old_wallet", "new_wallet", "payment_processor"] {
        if let Some(mode_arms) = arms[mode].as_object() {
            if mode_arms.is_empty() {
                continue;
            }
            let self_send = mode_arms
                .get("self_send")
                .with_context(|| format!("submission {mode}/S5 missing self_send scope arm"))?;
            if self_send["complete"] != false
                || self_send["unavailable_reason"]
                    .as_str()
                    .is_none_or(str::is_empty)
            {
                bail!("submission {mode}/S5 self_send must be disclosed as unavailable");
            }
        }
    }
    Ok(())
}

fn arm_complete(arms: &Value, mode: &str, arm: &str) -> bool {
    let arm = &arms[mode][arm];
    arm["recipient_count"]
        .as_u64()
        .is_some_and(|count| count > 0)
        && arm["complete"] == true
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

fn summary_outcome_counts(cell: &Value) -> (usize, usize, usize, usize, usize) {
    let mut api_accepted = 0;
    let mut confirmed = 0;
    let mut rejected = 0;
    let mut stalled = 0;
    let mut timed_out = 0;
    for observation in cell["repetitions"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|run| run["metrics"]["transaction_observations"].as_array())
        .flatten()
    {
        if observation["api_accepted"] == true {
            api_accepted += 1;
        }
        match observation["terminal_outcome"].as_str() {
            Some("confirmed") => confirmed += 1,
            Some("rejected") => rejected += 1,
            Some("stalled") => stalled += 1,
            Some("timed_out") => timed_out += 1,
            _ => {}
        }
    }
    (api_accepted, confirmed, rejected, stalled, timed_out)
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

pub fn write_legacy_v5_summary(profile_path: &Path, output_path: &Path) -> anyhow::Result<()> {
    let bytes =
        fs::read(profile_path).with_context(|| format!("reading {}", profile_path.display()))?;
    let document: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing {} as JSON", profile_path.display()))?;
    validate_legacy_v5_document(&document)?;
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
        "- Selected scan node: `{}` (`{}`; `{}`)\n",
        markdown_text(&document["base_node"]["endpoint"]),
        markdown_text(&document["base_node"]["configured_revision"]),
        markdown_text(&document["environment"]["base_node_network_path"])
    ));
    output.push_str(&format!(
        "- Independent authority: `{}` (`{}`)\n\n",
        markdown_text(&document["base_node"]["authority_endpoint"]),
        markdown_text(&document["environment"]["authority_network_path"])
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
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output_path, output).with_context(|| format!("writing {}", output_path.display()))?;
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
        "- Measurement commit: `{}`\n",
        markdown_text(&document["provenance"]["measurement_commit"])
    ));
    output.push_str(&format!(
        "- Export commit: `{}`\n",
        markdown_text(&document["provenance"]["export_commit"])
    ));
    output.push_str(&format!(
        "- Selected scan node: `{}` (`{}`; `{}`)\n",
        markdown_text(&document["base_node"]["endpoint"]),
        markdown_text(&document["base_node"]["configured_revision"]),
        markdown_text(&document["environment"]["base_node_network_path"])
    ));
    output.push_str(&format!(
        "- Independent authority: `{}` (`{}`)\n\n",
        markdown_text(&document["base_node"]["authority_endpoint"]),
        markdown_text(&document["environment"]["authority_network_path"])
    ));
    output.push_str("| Mode | Scenario | Execution | Outcome | Median ms (ok) | Median ms (all) | API accepted | Chain confirmed | Rejected | Stalled | Timed out | Successes | Failures |\n");
    output.push_str("|---|---:|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for mode in ModeName::ALL {
        for scenario in ScenarioName::ALL {
            let cell = &document["modes"][mode.as_str()]["scenarios"][scenario.as_str()];
            let (successes, failures) = cell["repetitions"]
                .as_array()
                .and_then(|runs| runs.last())
                .map(|run| (&run["success_count"], &run["failure_count"]))
                .unwrap_or((&Value::Null, &Value::Null));
            let (api_accepted, confirmed, rejected, stalled, timed_out) =
                summary_outcome_counts(cell);
            output.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                mode.as_str(),
                scenario.as_str(),
                markdown_text(&cell["execution_status"]),
                markdown_text(&cell["outcome_status"]),
                display_value(&cell["median_wall_ms"]),
                display_value(&cell["all_runs_median_wall_ms"]),
                api_accepted,
                confirmed,
                rejected,
                stalled,
                timed_out,
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
    use crate::{
        config::Config,
        env_capture,
        result_profile::{VerifiedTransaction, empty_mode_profile},
    };

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
        let mut document = serde_json::to_value(profile).unwrap();
        let manifest = json!({
            "schema_version": 2,
            "sources": {"harness": {
                "repository": "test",
                "upstream": {"revision": "test", "commit": "test", "tree": "test"},
                "patches": [], "complete_diff_sha256": "test", "result_tree": "test"
            }},
            "artifacts": {"harness": {
                "source": "harness", "source_revision": "test", "source_tree": "test", "sha256": "test"
            }}
        });
        document["provenance"]["measurement_build_manifest"] = manifest.clone();
        document["provenance"]["export_build_manifest"] = manifest.clone();
        document["config"]["build_manifest"] = manifest;
        document
    }

    #[test]
    fn valid_v5_checkpoint_round_trips() {
        let document = profile_document();
        let profile = validate_document(&document, false).unwrap();
        assert_eq!(profile.schema_version, RESULT_SCHEMA_VERSION);
    }

    #[test]
    fn legacy_v5_validation_is_explicit_and_does_not_accept_v6() {
        let mut legacy = profile_document();
        legacy["schema_version"] = json!(5);
        legacy["profile_kind"] = json!("final");
        legacy["run_complete"] = json!(true);
        legacy["harness_git_commit"] = json!("historical");
        legacy.as_object_mut().unwrap().remove("provenance");
        validate_legacy_v5_document(&legacy).unwrap();

        legacy["schema_version"] = json!(6);
        assert!(validate_legacy_v5_document(&legacy).is_err());
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
    fn balance_reconciliation_rejects_arithmetic_mutation() {
        let metrics = json!({
            "balance_before_microtari": 1000,
            "balance_after_microtari": 300,
            "outgoing_microtari": 600,
            "balance_reconciliation": {
                "expected_balance_microtari": 300,
                "observed_balance_microtari": 300,
                "delta_microtari": 0,
                "flagged": false,
                "balance_domain": "available"
            }
        });
        validate_balance_reconciliation(metrics.as_object().unwrap(), "test", Some(100)).unwrap();
        let mut mutated = metrics.clone();
        mutated["balance_reconciliation"]["delta_microtari"] = json!(1);
        assert!(
            validate_balance_reconciliation(mutated.as_object().unwrap(), "test", Some(100))
                .is_err()
        );
        let mut underflow = metrics.clone();
        underflow["balance_before_microtari"] = json!(50);
        assert!(
            validate_balance_reconciliation(underflow.as_object().unwrap(), "test", Some(100))
                .is_err()
        );
        let mut missing_domain = metrics;
        missing_domain["balance_reconciliation"]["balance_domain"] = Value::Null;
        assert!(
            validate_balance_reconciliation(missing_domain.as_object().unwrap(), "test", Some(100))
                .is_err()
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
    fn confirmed_binding_covers_later_repetitions() {
        let observation = json!({"transaction_id": "42", "terminal_outcome": "confirmed"});
        let mut document = profile_document();
        document["modes"]["old_wallet"]["scenarios"]["S1"]["repetitions"] = json!([
            {"metrics": {"transaction_observations": [observation.clone()]}},
            {"metrics": {"transaction_observations": [observation]}}
        ]);
        let mut profile = ResultProfile::new(&Config::default(), env_capture::capture());
        profile
            .chain_verification
            .verified_transactions
            .push(VerifiedTransaction {
                tx_id: "42".to_string(),
                status_value: TX_MINED_CONFIRMED_STATUS,
                mode: "old_wallet".to_string(),
                scenario: "S1".to_string(),
                amount_microtari: None,
                fee_microtari: None,
                mined_height: Some(1),
                confirmations: Some(3),
                min_confirmations: Some(3),
                tip_height: Some(4),
                confirmed: true,
            });
        assert!(validate_confirmed_observation_bindings(&document, &profile).is_err());
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
        let first = render_summary(&document).unwrap();
        assert_eq!(first, render_summary(&document).unwrap());
        assert!(first.contains("Measurement commit: `"));
        assert!(first.contains("Export commit: `"));
        assert!(first.contains("Median ms (all)"));
        for column in [
            "API accepted",
            "Chain confirmed",
            "Rejected",
            "Stalled",
            "Timed out",
        ] {
            assert!(first.contains(column), "missing summary column {column}");
        }
    }

    #[test]
    fn summary_outcome_counts_come_from_structured_observations() {
        let cell = json!({
            "repetitions": [{"metrics": {"transaction_observations": [
                {"api_accepted": true, "terminal_outcome": "confirmed"},
                {"api_accepted": true, "terminal_outcome": "stalled"},
                {"api_accepted": false, "terminal_outcome": "rejected"},
                {"api_accepted": true, "terminal_outcome": "timed_out"}
            ]}}]
        });
        assert_eq!(summary_outcome_counts(&cell), (3, 1, 1, 1, 1));
    }

    #[test]
    fn incomplete_provenance_manifests_are_rejected() {
        let mut document = profile_document();
        document["provenance"]["measurement_build_manifest"] = Value::Null;
        assert!(validate_document(&document, false).is_err());

        document["provenance"]["measurement_build_manifest"] = json!({
            "schema_version": 2,
            "artifacts": {"harness": {"source_revision": "", "sha256": "test"}}
        });
        assert!(validate_document(&document, false).is_err());
    }

    #[test]
    fn scan_history_validation_recomputes_commitment_evidence() {
        let document = json!({
            "modes": {
                "new_wallet": {
                    "scenarios": {
                        "S1": {"repetitions": [{"metrics": {"transaction_observations": [{
                            "transaction_id": "tx-1",
                            "terminal_outcome": "confirmed",
                            "output_commitments": ["commitment-1", "commitment-2"]
                        }]}}]}
                    }
                }
            }
        });
        let metrics = json!({
            "expected_output_commitments": {
                "tx-1": ["commitment-1", "commitment-2"]
            },
            "recovered_output_commitments": ["commitment-2"],
            "missing_history_output_tx_ids": []
        });
        validate_scan_history_evidence(&document, "new_wallet", "S2", &metrics).unwrap();

        let mut tampered = metrics;
        tampered["expected_output_commitments"] = json!({"tx-2": ["commitment-2"]});
        assert!(
            validate_scan_history_evidence(&document, "new_wallet", "S2", &tampered)
                .unwrap_err()
                .to_string()
                .contains("inconsistent")
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
    fn s5_self_send_arms_are_explicitly_unavailable() {
        let arms = json!({
            "old_wallet": {"self_send": {"complete": false, "unavailable_reason": "unsupported"}},
            "new_wallet": {"self_send": {"complete": false, "unavailable_reason": "unsupported"}},
            "payment_processor": {"self_send": {"complete": false, "unavailable_reason": "unsupported"}}
        });
        let document = json!({
            "computed_deltas": {
                "s5_throughput": {
                    "arms": arms,
                    "comparisons": {
                        "new_wallet_individual_over_payment_processor_batch": null,
                        "old_wallet_individual_over_payment_processor_batch": null,
                        "new_wallet_individual_fee_per_recipient_over_payment_processor_batch": null,
                        "old_wallet_individual_fee_per_recipient_over_payment_processor_batch": null
                    }
                }
            }
        });
        validate_s5_comparisons(&document).unwrap();
    }

    #[test]
    fn outcome_status_enum_remains_stable() {
        assert_eq!(
            serde_json::to_value(crate::result_profile::OutcomeStatus::Success).unwrap(),
            json!("success")
        );
    }

    #[test]
    fn schema_accepts_realistic_s1_s4_s5_transaction_observations() {
        let mut document = profile_document();
        let observation = json!({
            "transaction_id": "tx-1",
            "attempt_index": 1,
            "batch_index": null,
            "submit_offset_ms": 2,
            "construction_complete_offset_ms": 7,
            "construction_ms": 5,
            "submission_ms": 3,
            "mempool_available": true,
            "mempool_reason": null,
            "confirmation_ms": 40,
            "confirmation_timing_reason": null,
            "fee_microtari": 700,
            "terminal_outcome": "confirmed",
            "error": null,
            "mined_height": 100,
            "tip_start_height": 99,
            "tip_end_height": 103,
            "input_count": 1,
            "total_output_count": 2,
            "payment_output_count": 1,
            "change_output_count": 1,
            "output_commitments": ["commitment"],
            "configured_batch": 8
        });
        for mode in ["old_wallet", "new_wallet", "payment_processor"] {
            for scenario in ["S1", "S4", "S5"] {
                document["modes"][mode]["scenarios"][scenario]["repetitions"] = json!([{
                    "run": 1,
                    "execution_status": "completed",
                    "outcome_status": "success",
                    "wall_ms": 50,
                    "success_count": 1,
                    "failure_count": 0,
                    "fee_microtari": 700,
                    "error": null,
                    "metrics": {"transaction_observations": [observation.clone()]}
                }]);
            }
        }
        validate_schema(&document).unwrap();
    }

    #[test]
    fn validation_recomputes_and_rejects_tampered_deltas() {
        let mut document = profile_document();
        document["computed_deltas"]["scan_deltas"]["old_wallet"]["t_scan_s6_over_b0"] =
            json!(999.0);
        let error = validate_document(&document, false).unwrap_err().to_string();
        assert!(error.contains("computed_deltas"));
    }

    #[test]
    fn realistic_submission_accepts_honest_wallet_failure_and_blocked_dependents() {
        let mut config = Config::default();
        config.network.base_node_http_url = "http://127.0.0.1:18142".to_string();
        config.network.authority_http_url = "https://rpc.esmeralda.tari.com".to_string();
        config.network.mode1_base_node_service_peer =
            Some("abc::/ip4/127.0.0.1/tcp/18189".to_string());
        config.benchmark.mode1_live_topology = true;
        config.benchmark.mode2_live_scenarios = true;
        config.benchmark.mode3_live_topology = true;
        config.benchmark.live_fresh_scan_cells = true;
        let funding = crate::config::FundingRecord {
            amount: "10000 T".to_string(),
            tx_id: "funding-tx".to_string(),
            height: 101,
            birthday: Some(1),
            birthday_start_height: Some(90),
            construction_ms: Some(1),
            broadcast_to_mempool_ms: Some(2),
            broadcast_to_confirmed_at_c_min_ms: Some(3),
            tip_height_at_broadcast: Some(101),
            tip_height_at_confirmation: Some(104),
            shared_funding_fee_microtari: Some(700),
            funding_fee_attribution: Some(
                "external_source_shared_not_deducted_from_mode_balance".to_string(),
            ),
        };
        config.funding.old_wallet = Some(funding.clone());
        config.funding.new_wallet = Some(funding.clone());
        config.funding.payment_processor = Some(funding);
        let mut profile = ResultProfile::new(
            &config,
            env_capture::capture_for_network(
                &config.network.base_node_http_url,
                &config.network.authority_http_url,
                config.network.mode1_base_node_service_peer.as_deref(),
            ),
        );
        let manifest = json!({
            "schema_version": 2,
            "sources": {"harness": {
                "repository": "test",
                "upstream": {"revision": "test", "commit": "test", "tree": "test"},
                "patches": [], "complete_diff_sha256": "test", "result_tree": "test"
            }},
            "artifacts": {"harness": {
                "source": "harness", "source_revision": "test", "source_tree": "test", "sha256": "test"
            }}
        });
        profile
            .config
            .insert("build_manifest".to_string(), manifest.clone());
        profile.provenance.measurement_build_manifest = manifest.clone();
        profile.provenance.export_build_manifest = manifest;
        profile.config.insert(
            "seed_fingerprints".to_string(),
            json!({"old_wallet": "a", "new_wallet": "b", "payment_processor": "c"}),
        );
        profile.completed_stages = REQUIRED_STAGES.iter().map(ToString::to_string).collect();
        profile.base_node.tip_start_height = Some(100);
        profile.base_node.tip_start_hash = Some("aa".repeat(32));
        profile.base_node.tip_end_height = Some(103);
        profile.base_node.tip_end_hash = Some("bb".repeat(32));
        profile.base_node.authority_tip_start_height = Some(100);
        profile.base_node.authority_tip_start_hash = Some("aa".repeat(32));
        profile.base_node.authority_tip_end_height = Some(103);
        profile.base_node.authority_tip_end_hash = Some("bb".repeat(32));
        profile.base_node.pruning_horizon = Some(0);
        profile.base_node.is_synced = Some(true);

        for mode in ModeName::ALL {
            let mut mode_profile = empty_mode_profile(mode, Some(format!("{mode:?}-address")));
            mode_profile
                .scenarios
                .get_mut("B0")
                .unwrap()
                .record_repetition(crate::result_profile::Repetition {
                    run: 1,
                    status: CellStatus::Ok,
                    wall_ms: Some(10),
                    success_count: 1,
                    failure_count: 0,
                    fee_microtari: Some(0),
                    error: None,
                    metrics: Some(json!({
                        "T_scan_ms": 10, "blocks_per_sec": 10.0,
                        "H_tip_start": 100, "H_tip_end": 100,
                        "H_tip_target_hash": "aa", "H_scan_cursor_hash": "aa",
                        "H_tip_completion": 101,
                        "H_tip_completion_hash": "bb", "birthday": 0,
                        "detected_outputs": 0, "spendable_outputs": 0,
                        "available_microtari": 0, "history_transactions": 0,
                        "max_height": 100, "tip_lag_blocks": 0,
                        "tip_lag_tolerance_blocks": 0, "scan_reached_tip": true,
                        "scan_invocations": 1,
                        "peak_rss_bytes": 1, "peak_cpu_percent": 1.0,
                        "balance_delta_microtari": 0
                    })),
                });
            mode_profile
                .scenarios
                .get_mut("S0")
                .unwrap()
                .record_repetition(crate::result_profile::Repetition {
                    run: 1,
                    status: CellStatus::Ok,
                    wall_ms: Some(1),
                    success_count: 1,
                    failure_count: 0,
                    fee_microtari: Some(0),
                    error: None,
                    metrics: Some(json!({
                        "verification_source": "wallet_state_observed",
                        "balance_delta_microtari": 0,
                        "expected_spendable_count": 1,
                        "observed_spendable_count": 1,
                        "expected_available_microtari": 10000000000u64,
                        "available_microtari": 10000000000u64,
                        "pending_outputs": 0,
                        "locked_outputs": 0,
                        "invalid_outputs": 0,
                        "unknown_outputs": 0,
                        "wallet_state_complete": true,
                        "spendable_count_matches_expected": true,
                        "balance_matches_expected": true,
                        "funding_observation": {
                            "tx_id": "funding-tx",
                            "mined_height": 101,
                            "shared_funding_fee_microtari": 700
                        }
                    })),
                });
            mode_profile
                .scenarios
                .get_mut("S1")
                .unwrap()
                .record_repetition(crate::result_profile::Repetition {
                    run: 1,
                    status: CellStatus::Failed,
                    wall_ms: Some(5),
                    success_count: 0,
                    failure_count: 1,
                    fee_microtari: Some(0),
                    error: Some("wallet rejected construction".to_string()),
                    metrics: Some(json!({
                        "balance_reconciliation_unavailable_reason": "wallet rejected construction",
                        "scenario_tip_start_height": 100,
                        "scenario_tip_end_height": 100,
                        "balance_before": {"total": 1, "available": 1, "locked": 0, "unconfirmed": 0, "immature": 0, "pending_incoming": 0, "pending_outgoing": 0, "timelocked": 0},
                        "balance_after": {"total": 1, "available": 1, "locked": 0, "unconfirmed": 0, "immature": 0, "pending_incoming": 0, "pending_outgoing": 0, "timelocked": 0},
                        "transaction_observations": [{
                            "transaction_id": null, "attempt_index": 1, "batch_index": null,
                            "submit_offset_ms": 0, "construction_complete_offset_ms": 1,
                            "broadcast_start_offset_ms": null,
                            "construction_ms": null, "submission_ms": null,
                            "construction_timing_origin": null,
                            "construction_timing_reason": "construction failed",
                            "submission_timing_origin": null,
                            "mempool_available": null, "mempool_reason": "not submitted",
                            "confirmation_ms": null, "confirmation_timing_origin": null,
                            "confirmation_timing_reason": "not submitted",
                            "fee_microtari": null, "fee_unavailable_reason": "not constructed",
                            "recipient": null, "recipients": [], "api_accepted": false,
                            "api_error": "wallet rejected construction", "terminal_outcome": "rejected",
                            "error": "wallet rejected construction", "mined_height": null,
                            "tip_start_height": 100, "tip_end_height": null,
                            "input_count": null, "total_output_count": null,
                            "payment_output_count": null, "change_output_count": null,
                            "output_commitments": [], "configured_batch": null
                        }]
                    })),
                });
            for scenario in ["S2", "S3", "S4", "S5", "S6", "S7"] {
                mode_profile
                    .scenarios
                    .get_mut(scenario)
                    .unwrap()
                    .record_repetition(crate::result_profile::Repetition {
                        run: 1,
                        status: CellStatus::BlockedPrerequisite,
                        wall_ms: None,
                        success_count: 0,
                        failure_count: 0,
                        fee_microtari: None,
                        error: None,
                        metrics: Some(json!({
                            "blocked_prerequisite": true,
                            "balance_reconciliation_unavailable_reason": "blocked by S1"
                        })),
                    });
            }
            profile
                .modes
                .insert(mode.as_str().to_string(), mode_profile);
        }
        profile.mark_final();
        profile.refresh_computed_deltas();
        profile.validate_submission().unwrap();

        let mut repeated = serde_json::to_value(&profile).unwrap();
        let second_s0 =
            repeated["modes"]["old_wallet"]["scenarios"]["S0"]["repetitions"][0].clone();
        repeated["modes"]["old_wallet"]["scenarios"]["S0"]["repetitions"]
            .as_array_mut()
            .unwrap()
            .push(second_s0);
        repeated["modes"]["old_wallet"]["scenarios"]["S0"]["repetitions"][1]["metrics"]["available_microtari"] =
            json!(999);
        assert!(validate_document(&repeated, true).is_err());

        for field in [
            "pending_outputs",
            "locked_outputs",
            "invalid_outputs",
            "unknown_outputs",
        ] {
            let mut mutated = serde_json::to_value(&profile).unwrap();
            mutated["modes"]["old_wallet"]["scenarios"]["S0"]["repetitions"][0]["metrics"][field] =
                json!(1);
            assert!(validate_document(&mutated, true).is_err(), "{field}");
        }

        let mut tampered_fee = serde_json::to_value(&profile).unwrap();
        tampered_fee["modes"]["old_wallet"]["scenarios"]["S1"]["repetitions"][0]["fee_microtari"] =
            json!(1);
        assert!(validate_document(&tampered_fee, true).is_err());

        let mut mismatched = serde_json::to_value(&profile).unwrap();
        mismatched["modes"]["new_wallet"]["scenarios"]["B0"]["repetitions"][0]["metrics"]["H_tip_target_hash"] =
            json!("different");
        mismatched["modes"]["new_wallet"]["scenarios"]["B0"]["repetitions"][0]["metrics"]["H_scan_cursor_hash"] =
            json!("different");
        let error = validate_document(&mismatched, true)
            .unwrap_err()
            .to_string();
        assert!(error.contains("do not share one target"));
    }
}
