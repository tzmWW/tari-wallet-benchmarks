use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    config::Config,
    env_capture::Environment,
    modes::{ModeName, ScenarioName},
    versions::{
        MINOTARI_CLI_REV, PAYMENT_PROCESSOR_REV, TARI_CONSOLE_WALLET_REV, TX_MINED_CONFIRMED_STATUS,
    },
};

pub const RESULT_SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultProfile {
    pub schema_version: u32,
    pub generated_at: chrono::DateTime<chrono::Utc>,
    pub network: String,
    pub environment: Environment,
    pub versions: BTreeMap<String, String>,
    pub config: BTreeMap<String, serde_json::Value>,
    pub funding: BTreeMap<String, crate::config::FundingRecord>,
    pub modes: BTreeMap<String, ModeProfile>,
    pub findings: Vec<Finding>,
    pub chain_verification: ChainVerification,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeProfile {
    pub mode: ModeName,
    pub address: Option<String>,
    pub scenarios: BTreeMap<String, ScenarioCell>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioCell {
    pub scenario: ScenarioName,
    pub surface: String,
    pub status: CellStatus,
    pub repetitions: Vec<Repetition>,
    pub median_wall_ms: Option<u128>,
    pub spread_wall_ms: Option<u128>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CellStatus {
    PendingFunding,
    ReadyForLiveRun,
    Ok,
    Failed,
    BlockedUpstream,
    NotApplicable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repetition {
    pub run: u32,
    pub status: CellStatus,
    pub wall_ms: Option<u128>,
    pub success_count: u32,
    pub failure_count: u32,
    pub fee_microtari: Option<u64>,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    pub title: String,
    pub status: String,
    pub recommendation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainVerification {
    pub tx_mined_confirmed_status_value: u32,
    pub verified_transactions: Vec<VerifiedTransaction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifiedTransaction {
    pub tx_id: String,
    pub status_value: u32,
    pub mode: String,
    pub scenario: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount_microtari: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fee_microtari: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mined_height: Option<u64>,
    #[serde(default)]
    pub confirmed: bool,
}

impl ResultProfile {
    pub fn new(config: &Config, environment: Environment) -> Self {
        let versions = BTreeMap::from([
            (
                "minotari_cli_rev".to_string(),
                config.versions.minotari_cli_rev.clone(),
            ),
            (
                "tari_console_wallet_rev".to_string(),
                config.versions.tari_console_wallet_rev.clone(),
            ),
            (
                "minotari_payment_processor_rev".to_string(),
                config.versions.payment_processor_rev.clone(),
            ),
            (
                "harness_minotari_cli_pin".to_string(),
                MINOTARI_CLI_REV.to_string(),
            ),
            (
                "harness_tari_console_wallet_pin".to_string(),
                TARI_CONSOLE_WALLET_REV.to_string(),
            ),
            (
                "harness_payment_processor_pin".to_string(),
                PAYMENT_PROCESSOR_REV.to_string(),
            ),
        ]);

        Self {
            schema_version: RESULT_SCHEMA_VERSION,
            generated_at: chrono::Utc::now(),
            network: config.network.name.clone(),
            environment,
            versions,
            config: config.scenario_defaults(),
            funding: config.funding.as_map(),
            modes: BTreeMap::new(),
            findings: default_findings(),
            chain_verification: ChainVerification {
                tx_mined_confirmed_status_value: TX_MINED_CONFIRMED_STATUS,
                verified_transactions: Vec::new(),
            },
        }
    }

    pub fn write_atomic(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(&mut tmp, self)?;
        writeln!(tmp)?;
        tmp.persist(path)?;
        Ok(())
    }
}

impl ScenarioCell {
    pub fn record_repetition(&mut self, repetition: Repetition) {
        self.repetitions.push(repetition);
        self.refresh_summary();
    }

    pub fn refresh_summary(&mut self) {
        let mut walls = self
            .repetitions
            .iter()
            .filter_map(|run| {
                if run.status == CellStatus::Ok {
                    run.wall_ms
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        walls.sort_unstable();

        self.median_wall_ms = median(&walls);
        self.spread_wall_ms = match (walls.first(), walls.last()) {
            (Some(min), Some(max)) => Some(max - min),
            _ => None,
        };

        self.status = if self.repetitions.is_empty() {
            self.status.clone()
        } else if self
            .repetitions
            .iter()
            .all(|run| run.status == CellStatus::Ok)
        {
            CellStatus::Ok
        } else if self
            .repetitions
            .iter()
            .any(|run| run.status == CellStatus::Ok)
        {
            CellStatus::Failed
        } else {
            self.repetitions
                .last()
                .map(|run| run.status.clone())
                .unwrap_or_else(|| self.status.clone())
        };
    }
}

pub fn empty_mode_profile(mode: ModeName, address: Option<String>) -> ModeProfile {
    let scenarios = ScenarioName::ALL
        .into_iter()
        .map(|scenario| {
            (
                scenario.as_str().to_string(),
                ScenarioCell {
                    scenario,
                    surface: scenario.measurement_surface(mode).to_string(),
                    status: CellStatus::ReadyForLiveRun,
                    repetitions: Vec::new(),
                    median_wall_ms: None,
                    spread_wall_ms: None,
                    notes: Vec::new(),
                },
            )
        })
        .collect();
    ModeProfile {
        mode,
        address,
        scenarios,
    }
}

fn median(sorted: &[u128]) -> Option<u128> {
    if sorted.is_empty() {
        return None;
    }
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        Some((sorted[mid - 1] + sorted[mid]) / 2)
    } else {
        Some(sorted[mid])
    }
}

pub fn write_schema(path: &PathBuf) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let schema = serde_json::json!({
        "schema_version": RESULT_SCHEMA_VERSION,
        "required_top_level_keys": [
            "schema_version",
            "generated_at",
            "network",
            "environment",
            "versions",
            "config",
            "funding",
            "modes",
            "findings",
            "chain_verification"
        ],
        "cell_status_values": [
            "pending_funding",
            "ready_for_live_run",
            "ok",
            "failed",
            "blocked_upstream",
            "not_applicable"
        ],
        "tx_mined_confirmed_status_value": TX_MINED_CONFIRMED_STATUS
        ,
        "repetition_optional_metrics": {
            "description": "scenario-specific structured metrics; fields are optional and cells only emit values they observed",
            "common_keys": [
                "verification_source",
                "verification_observations",
                "observed_transactions",
                "verification_loop",
                "blocked_prerequisite",
                "scan_checkpoint",
                "birthday",
                "tip_start",
                "tip_end",
                "blocks_scanned",
                "blocks_per_sec",
                "detected_outputs",
                "available_microtari"
            ],
            "verification_source_values": [
                "base_node_transaction_query",
                "wallet_db_observed",
                "payment_processor_db_observed",
                "wallet_scan_observed"
            ]
        },
        "verified_transaction_optional_keys": [
            "amount_microtari",
            "fee_microtari",
            "mined_height",
            "confirmed"
        ],
        "environment_fields": [
            "os",
            "cpu_brand",
            "physical_cores",
            "total_memory_bytes",
            "disk_kind",
            "disk_name",
            "base_node_host",
            "base_node_network_path"
        ]
    });
    fs::write(path, serde_json::to_string_pretty(&schema)? + "\n")?;
    Ok(())
}

fn default_findings() -> Vec<Finding> {
    vec![
        Finding {
            id: "pp-real-app".to_string(),
            title: "Mode 3 uses the real minotari_payment_processor".to_string(),
            status: "implemented_in_topology".to_string(),
            recommendation: "Keep PP failures visible as benchmark output rather than bypassing the service."
                .to_string(),
        },
        Finding {
            id: "birthday-seeds".to_string(),
            title: "Genesis and birthday scans require birthday-encoded seeds".to_string(),
            status: "harness_generates_seed_material".to_string(),
            recommendation: "Use generated seeds and rewrite birthday for scan setup instead of RescanWallet(0)."
                .to_string(),
        },
        Finding {
            id: "chain-verification".to_string(),
            title: "Mempool acceptance is not benchmark success".to_string(),
            status: "schema_requires_chain_verification".to_string(),
            recommendation: "Verify claimed successful transactions with status value 6 before publishing throughput."
                .to_string(),
        },
        Finding {
            id: "funds-pending-hidden-state".to_string(),
            title: "FundsPending can live outside reported balances".to_string(),
            status: "observed_in_live_runs".to_string(),
            recommendation: "Use chain-advance settlement gates between planned spend rounds; do not retry failed sends."
                .to_string(),
        },
        Finding {
            id: "mode2-multi-recipient-s1-builder".to_string(),
            title: "Mode 2 S1 uses the multi-recipient one-sided builder".to_string(),
            status: "implemented_for_s1_round_shape".to_string(),
            recommendation: "Keep S1 on the lower-level multi-recipient builder; S4/S5 remain single-recipient by scenario shape."
                .to_string(),
        },
        Finding {
            id: "pp-single-utxo-lock-stalls-batches".to_string(),
            title: "Payment processor batches can stall behind one locked UTXO".to_string(),
            status: "observed_in_real_topology".to_string(),
            recommendation: "Preserve PENDING_BATCHING/insufficient-funds states as benchmark signal."
                .to_string(),
        },
        Finding {
            id: "console-mnemonic-birthday".to_string(),
            title: "Console wallet recovery uses mnemonic birthday over the birthday flag".to_string(),
            status: "implemented_in_mode1_startup".to_string(),
            recommendation: "Rewrite only the mnemonic birthday for birthday scans while preserving wallet keys and address."
                .to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use crate::{config::Config, env_capture};

    use super::*;

    #[test]
    fn profile_round_trips() {
        let mut config = Config::default();
        config.funding.new_wallet = Some(crate::config::FundingRecord {
            amount: "50000 T".to_string(),
            tx_id: "7676530785144502866".to_string(),
            height: 707741,
        });
        let profile = ResultProfile::new(&config, env_capture::capture());
        let json = serde_json::to_string(&profile).unwrap();
        let decoded: ResultProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.schema_version, RESULT_SCHEMA_VERSION);
        assert_eq!(decoded.funding["new_wallet"].height, 707741);
    }

    #[test]
    fn cell_summary_uses_ok_repetition_walls() {
        let mut cell = ScenarioCell {
            scenario: crate::modes::ScenarioName::S2,
            surface: "minotari_library".to_string(),
            status: CellStatus::ReadyForLiveRun,
            repetitions: Vec::new(),
            median_wall_ms: None,
            spread_wall_ms: None,
            notes: Vec::new(),
        };
        cell.record_repetition(Repetition {
            run: 1,
            status: CellStatus::Ok,
            wall_ms: Some(30),
            success_count: 1,
            failure_count: 0,
            fee_microtari: None,
            error: None,
            metrics: None,
        });
        cell.record_repetition(Repetition {
            run: 2,
            status: CellStatus::Ok,
            wall_ms: Some(10),
            success_count: 1,
            failure_count: 0,
            fee_microtari: None,
            error: None,
            metrics: None,
        });
        cell.record_repetition(Repetition {
            run: 3,
            status: CellStatus::Ok,
            wall_ms: Some(20),
            success_count: 1,
            failure_count: 0,
            fee_microtari: None,
            error: None,
            metrics: Some(serde_json::json!({"sample": true})),
        });

        assert_eq!(cell.status, CellStatus::Ok);
        assert_eq!(cell.median_wall_ms, Some(20));
        assert_eq!(cell.spread_wall_ms, Some(20));
    }
}
