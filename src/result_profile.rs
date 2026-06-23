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
    versions::{MINOTARI_CLI_REV, PAYMENT_PROCESSOR_REV, TX_MINED_CONFIRMED_STATUS},
};

pub const RESULT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultProfile {
    pub schema_version: u32,
    pub generated_at: chrono::DateTime<chrono::Utc>,
    pub network: String,
    pub environment: Environment,
    pub versions: BTreeMap<String, String>,
    pub config: BTreeMap<String, serde_json::Value>,
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
}

impl ResultProfile {
    pub fn new(config: &Config, environment: Environment) -> Self {
        let versions = BTreeMap::from([
            (
                "minotari_cli_rev".to_string(),
                config.versions.minotari_cli_rev.clone(),
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
    ]
}

#[cfg(test)]
mod tests {
    use crate::{config::Config, env_capture};

    use super::*;

    #[test]
    fn profile_round_trips() {
        let profile = ResultProfile::new(&Config::default(), env_capture::capture());
        let json = serde_json::to_string(&profile).unwrap();
        let decoded: ResultProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.schema_version, RESULT_SCHEMA_VERSION);
    }
}
