use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{
    config::Config,
    env_capture::Environment,
    modes::{ModeName, ScenarioName},
    versions::{
        MINOTARI_CLI_REV, PAYMENT_PROCESSOR_REV, TARI_CONSOLE_WALLET_REV, TX_MINED_CONFIRMED_STATUS,
    },
};

#[path = "profile_validation/mod.rs"]
pub mod profile_validation;

pub const RESULT_SCHEMA_VERSION: u32 = 4;
pub const REFERENCE_BASE_NODE_REVISION: &str = "v5.4.0";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    Checkpoint,
    Final,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Completed,
    BlockedPrerequisite,
    HarnessError,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeStatus {
    Success,
    Partial,
    Failure,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultProfile {
    pub schema_version: u32,
    pub run_id: String,
    pub profile_kind: ProfileKind,
    pub run_complete: bool,
    pub harness_git_commit: String,
    pub completed_stages: Vec<String>,
    pub generated_at: chrono::DateTime<chrono::Utc>,
    pub network: String,
    pub base_node: BaseNodeMetadata,
    pub environment: Environment,
    pub versions: BTreeMap<String, String>,
    pub config: BTreeMap<String, serde_json::Value>,
    pub funding: BTreeMap<String, crate::config::FundingRecord>,
    pub modes: BTreeMap<String, ModeProfile>,
    #[serde(default)]
    pub computed_deltas: BTreeMap<String, serde_json::Value>,
    pub findings: Vec<Finding>,
    pub chain_verification: ChainVerification,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaseNodeMetadata {
    pub endpoint: String,
    pub configured_revision: String,
    pub observed_version: Option<String>,
    pub version_observable: bool,
    pub tip_start_height: Option<u64>,
    pub tip_start_hash: Option<String>,
    pub tip_end_height: Option<u64>,
    pub tip_end_hash: Option<String>,
    pub pruning_horizon: Option<u64>,
    pub is_synced: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeProfile {
    pub mode: ModeName,
    pub address: Option<String>,
    pub scenarios: BTreeMap<String, ScenarioCell>,
}

#[derive(Debug, Clone)]
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
    HarnessError,
    Partial,
}

#[derive(Debug, Clone)]
pub struct Repetition {
    pub run: u32,
    pub status: CellStatus,
    pub wall_ms: Option<u128>,
    pub success_count: u32,
    pub failure_count: u32,
    pub fee_microtari: Option<u64>,
    pub error: Option<String>,
    pub metrics: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct ScenarioCellRef<'a> {
    scenario: ScenarioName,
    surface: &'a str,
    execution_status: ExecutionStatus,
    outcome_status: OutcomeStatus,
    repetitions: &'a [Repetition],
    median_wall_ms: Option<u128>,
    spread_wall_ms: Option<u128>,
    notes: &'a [String],
}

#[derive(Deserialize)]
struct ScenarioCellOwned {
    scenario: ScenarioName,
    surface: String,
    execution_status: ExecutionStatus,
    outcome_status: OutcomeStatus,
    repetitions: Vec<Repetition>,
    median_wall_ms: Option<u128>,
    spread_wall_ms: Option<u128>,
    notes: Vec<String>,
}

impl Serialize for ScenarioCell {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let blocked = self.repetitions.iter().any(repetition_is_blocked);
        let partial = self
            .repetitions
            .iter()
            .any(|run| run.status == CellStatus::Ok)
            && self
                .repetitions
                .iter()
                .any(|run| run.status != CellStatus::Ok);
        let (execution_status, mut outcome_status) = status_pair(&self.status, blocked);
        if partial {
            outcome_status = OutcomeStatus::Partial;
        }
        ScenarioCellRef {
            scenario: self.scenario,
            surface: &self.surface,
            execution_status,
            outcome_status,
            repetitions: &self.repetitions,
            median_wall_ms: self.median_wall_ms,
            spread_wall_ms: self.spread_wall_ms,
            notes: &self.notes,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ScenarioCell {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ScenarioCellOwned::deserialize(deserializer)?;
        Ok(Self {
            scenario: wire.scenario,
            surface: wire.surface,
            status: status_from_pair(wire.execution_status, wire.outcome_status),
            repetitions: wire.repetitions,
            median_wall_ms: wire.median_wall_ms,
            spread_wall_ms: wire.spread_wall_ms,
            notes: wire.notes,
        })
    }
}

#[derive(Serialize)]
struct RepetitionRef<'a> {
    run: u32,
    execution_status: ExecutionStatus,
    outcome_status: OutcomeStatus,
    wall_ms: Option<u128>,
    success_count: u32,
    failure_count: u32,
    fee_microtari: Option<u64>,
    error: &'a Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metrics: &'a Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct RepetitionOwned {
    run: u32,
    execution_status: ExecutionStatus,
    outcome_status: OutcomeStatus,
    wall_ms: Option<u128>,
    success_count: u32,
    failure_count: u32,
    fee_microtari: Option<u64>,
    error: Option<String>,
    #[serde(default)]
    metrics: Option<serde_json::Value>,
}

impl Serialize for Repetition {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let (execution_status, outcome_status) =
            status_pair(&self.status, repetition_is_blocked(self));
        RepetitionRef {
            run: self.run,
            execution_status,
            outcome_status,
            wall_ms: self.wall_ms,
            success_count: self.success_count,
            failure_count: self.failure_count,
            fee_microtari: self.fee_microtari,
            error: &self.error,
            metrics: &self.metrics,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Repetition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = RepetitionOwned::deserialize(deserializer)?;
        Ok(Self {
            run: wire.run,
            status: status_from_pair(wire.execution_status, wire.outcome_status),
            wall_ms: wire.wall_ms,
            success_count: wire.success_count,
            failure_count: wire.failure_count,
            fee_microtari: wire.fee_microtari,
            error: wire.error,
            metrics: wire.metrics,
        })
    }
}

fn repetition_is_blocked(repetition: &Repetition) -> bool {
    repetition
        .metrics
        .as_ref()
        .and_then(|metrics| metrics.get("blocked_prerequisite"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn status_pair(status: &CellStatus, blocked: bool) -> (ExecutionStatus, OutcomeStatus) {
    if blocked {
        return (
            ExecutionStatus::BlockedPrerequisite,
            OutcomeStatus::Unavailable,
        );
    }
    match status {
        CellStatus::PendingFunding | CellStatus::ReadyForLiveRun => (
            ExecutionStatus::BlockedPrerequisite,
            OutcomeStatus::Unavailable,
        ),
        CellStatus::Ok => (ExecutionStatus::Completed, OutcomeStatus::Success),
        CellStatus::Failed | CellStatus::BlockedUpstream => {
            (ExecutionStatus::Completed, OutcomeStatus::Failure)
        }
        CellStatus::NotApplicable => (ExecutionStatus::NotApplicable, OutcomeStatus::Unavailable),
        CellStatus::HarnessError => (ExecutionStatus::HarnessError, OutcomeStatus::Unavailable),
        CellStatus::Partial => (ExecutionStatus::Completed, OutcomeStatus::Partial),
    }
}

fn status_from_pair(execution: ExecutionStatus, outcome: OutcomeStatus) -> CellStatus {
    match (execution, outcome) {
        (ExecutionStatus::Completed, OutcomeStatus::Success) => CellStatus::Ok,
        (ExecutionStatus::Completed, OutcomeStatus::Partial) => CellStatus::Partial,
        (ExecutionStatus::Completed, _) => CellStatus::Failed,
        (ExecutionStatus::BlockedPrerequisite, _) => CellStatus::BlockedUpstream,
        (ExecutionStatus::HarnessError, _) => CellStatus::HarnessError,
        (ExecutionStatus::NotApplicable, _) => CellStatus::NotApplicable,
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmations: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_confirmations: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tip_height: Option<u64>,
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
                "base_node_rev".to_string(),
                config.versions.base_node_rev.clone(),
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
        let mut scenario_config = config.scenario_defaults();
        scenario_config.insert(
            "scenario_order".to_string(),
            serde_json::json!(ScenarioName::ALL.map(ScenarioName::as_str)),
        );

        Self {
            schema_version: RESULT_SCHEMA_VERSION,
            run_id: new_run_id(),
            profile_kind: ProfileKind::Checkpoint,
            run_complete: false,
            harness_git_commit: harness_git_commit(),
            completed_stages: Vec::new(),
            generated_at: chrono::Utc::now(),
            network: config.network.name.clone(),
            base_node: BaseNodeMetadata {
                endpoint: config.network.base_node_http_url.clone(),
                configured_revision: config.versions.base_node_rev.clone(),
                observed_version: None,
                version_observable: false,
                tip_start_height: None,
                tip_start_hash: None,
                tip_end_height: None,
                tip_end_hash: None,
                pruning_horizon: None,
                is_synced: None,
            },
            environment,
            versions,
            config: scenario_config,
            funding: config.funding.as_map(),
            modes: BTreeMap::new(),
            computed_deltas: BTreeMap::new(),
            findings: default_findings(),
            chain_verification: ChainVerification {
                tx_mined_confirmed_status_value: TX_MINED_CONFIRMED_STATUS,
                verified_transactions: Vec::new(),
            },
        }
    }

    pub fn refresh_computed_deltas(&mut self) {
        self.computed_deltas = computed_deltas(self);
    }

    pub fn mark_checkpoint_stage(&mut self, stage: impl Into<String>) {
        let stage = stage.into();
        if !self.completed_stages.contains(&stage) {
            self.completed_stages.push(stage);
        }
        self.profile_kind = ProfileKind::Checkpoint;
        self.run_complete = false;
    }

    pub fn mark_final(&mut self) {
        self.profile_kind = ProfileKind::Final;
        self.run_complete = true;
    }

    pub fn set_tip_start(&mut self, height: u64, hash: Option<String>) {
        self.base_node.tip_start_height = Some(height);
        self.base_node.tip_start_hash = hash;
    }

    pub fn set_tip_end(&mut self, height: u64, hash: Option<String>) {
        self.base_node.tip_end_height = Some(height);
        self.base_node.tip_end_hash = hash;
    }

    pub fn validate_checkpoint(&self) -> anyhow::Result<()> {
        profile_validation::validate_profile(self, false)
    }

    pub fn validate_final(&self) -> anyhow::Result<()> {
        profile_validation::validate_profile(self, false)?;
        profile_validation::validate_final(self, false)
    }

    pub fn validate_submission(&self) -> anyhow::Result<()> {
        profile_validation::validate_profile(self, true)?;
        profile_validation::validate_final(self, true)
    }

    pub fn write_validated_atomic(&self, path: &Path, submission: bool) -> anyhow::Result<()> {
        match (self.profile_kind, submission) {
            (ProfileKind::Checkpoint, false) => self.validate_checkpoint()?,
            (ProfileKind::Checkpoint, true) => {
                anyhow::bail!("a checkpoint cannot be written as a submission profile")
            }
            (ProfileKind::Final, false) => self.validate_final()?,
            (ProfileKind::Final, true) => self.validate_submission()?,
        }
        self.write_atomic(path)
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

fn new_run_id() -> String {
    format!(
        "run-{}-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
        std::process::id()
    )
}

fn harness_git_commit() -> String {
    option_env!("GIT_COMMIT")
        .map(ToString::to_string)
        .or_else(|| {
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|commit| commit.trim().to_string())
        })
        .filter(|commit| !commit.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn computed_deltas(profile: &ResultProfile) -> BTreeMap<String, serde_json::Value> {
    let mut deltas = BTreeMap::new();
    deltas.insert("scan_deltas".to_string(), computed_scan_deltas(profile));
    deltas.insert("s5_throughput".to_string(), computed_s5_throughput(profile));
    deltas
}

fn computed_scan_deltas(profile: &ResultProfile) -> serde_json::Value {
    let modes = ModeName::ALL
        .into_iter()
        .map(|mode| {
            let mode_key = mode.as_str();
            let b0 = scenario_wall_ms(profile, mode_key, "B0");
            let s2 = scenario_wall_ms(profile, mode_key, "S2");
            let s6 = scenario_wall_ms(profile, mode_key, "S6");
            (
                mode_key.to_string(),
                serde_json::json!({
                    "t_scan_s2_minus_b0_ms": option_sub_u128(s2, b0),
                    "t_scan_s6_minus_s2_ms": option_sub_u128(s6, s2),
                    "t_scan_s6_over_b0": option_ratio_u128(s6, b0),
                    "source": "scenario_median_wall_ms"
                }),
            )
        })
        .collect::<BTreeMap<_, _>>();
    serde_json::Value::Object(modes.into_iter().collect())
}

fn computed_s5_throughput(profile: &ResultProfile) -> serde_json::Value {
    let arms = ModeName::ALL
        .into_iter()
        .filter_map(|mode| {
            let mode_key = mode.as_str();
            s5_arm_metrics(profile, mode_key).map(|value| (mode_key.to_string(), value))
        })
        .collect::<BTreeMap<_, _>>();

    let old_individual_ms = complete_arm_wall_ms(&arms, "old_wallet", "individual");
    let new_individual_ms = complete_arm_wall_ms(&arms, "new_wallet", "individual");
    let pp_batch_ms = complete_arm_wall_ms(&arms, "payment_processor", "batch");

    let new_pp_comparison = option_ratio_u128(new_individual_ms, pp_batch_ms);
    let old_pp_comparison = option_ratio_u128(old_individual_ms, pp_batch_ms);

    serde_json::json!({
        "arms": arms,
        "comparisons": {
            "new_wallet_individual_over_payment_processor_batch": new_pp_comparison,
            "old_wallet_individual_over_payment_processor_batch": old_pp_comparison
        },
        "comparison_unavailable_reasons": {
            "new_wallet_individual_over_payment_processor_batch": new_pp_comparison.is_none().then_some("one_or_both_source_arms_incomplete"),
            "old_wallet_individual_over_payment_processor_batch": old_pp_comparison.is_none().then_some("one_or_both_source_arms_incomplete")
        },
        "source": "S5 repetition metrics.s5_arms"
    })
}

fn scenario_wall_ms(profile: &ResultProfile, mode: &str, scenario: &str) -> Option<u128> {
    profile
        .modes
        .get(mode)?
        .scenarios
        .get(scenario)?
        .median_wall_ms
}

fn s5_arm_metrics(profile: &ResultProfile, mode: &str) -> Option<serde_json::Value> {
    profile
        .modes
        .get(mode)?
        .scenarios
        .get("S5")?
        .repetitions
        .iter()
        .find_map(|run| run.metrics.as_ref())?
        .get("s5_arms")
        .cloned()
}

fn complete_arm_wall_ms(
    arms: &BTreeMap<String, serde_json::Value>,
    mode: &str,
    arm: &str,
) -> Option<u128> {
    let arm = arms.get(mode)?.get(arm)?;
    let recipients = arm.get("recipient_count")?.as_u64()?;
    let successes = arm.get("success_count")?.as_u64()?;
    let failures = arm.get("failure_count")?.as_u64()?;
    if recipients == 0 || successes != recipients || failures != 0 {
        return None;
    }
    arm.get("wall_ms")?.as_u64().map(u128::from)
}

fn option_sub_u128(left: Option<u128>, right: Option<u128>) -> Option<i128> {
    Some(left? as i128 - right? as i128)
}

fn option_ratio_u128(numerator: Option<u128>, denominator: Option<u128>) -> Option<f64> {
    let denominator = denominator?;
    if denominator == 0 {
        return None;
    }
    Some(numerator? as f64 / denominator as f64)
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
            CellStatus::Partial
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
    let schema = profile_validation::schema_document();
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
            birthday: None,
            birthday_start_height: None,
        });
        let profile = ResultProfile::new(&config, env_capture::capture());
        let json = serde_json::to_string(&profile).unwrap();
        let decoded: ResultProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.schema_version, RESULT_SCHEMA_VERSION);
        assert_eq!(decoded.funding["new_wallet"].height, 707741);
        assert_eq!(
            decoded.config["scenario_order"],
            serde_json::json!(["B0", "S0", "S1", "S2", "S3", "S4", "S5", "S6", "S7"])
        );
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

    #[test]
    fn computed_deltas_include_scan_and_s5_ratios() {
        let config = Config::default();
        let mut profile = ResultProfile::new(&config, crate::env_capture::capture());
        profile.modes.insert(
            "old_wallet".to_string(),
            empty_mode_profile(crate::modes::ModeName::OldWallet, None),
        );
        profile.modes.insert(
            "payment_processor".to_string(),
            empty_mode_profile(crate::modes::ModeName::PaymentProcessor, None),
        );

        let old = profile.modes.get_mut("old_wallet").unwrap();
        old.scenarios
            .get_mut("B0")
            .unwrap()
            .record_repetition(Repetition {
                run: 1,
                status: CellStatus::Ok,
                wall_ms: Some(100),
                success_count: 1,
                failure_count: 0,
                fee_microtari: None,
                error: None,
                metrics: None,
            });
        old.scenarios
            .get_mut("S2")
            .unwrap()
            .record_repetition(Repetition {
                run: 1,
                status: CellStatus::Ok,
                wall_ms: Some(175),
                success_count: 1,
                failure_count: 0,
                fee_microtari: None,
                error: None,
                metrics: None,
            });
        old.scenarios
            .get_mut("S5")
            .unwrap()
            .record_repetition(Repetition {
                run: 1,
                status: CellStatus::Ok,
                wall_ms: Some(300),
                success_count: 1,
                failure_count: 0,
                fee_microtari: None,
                error: None,
                metrics: Some(serde_json::json!({
                    "s5_arms": {
                        "individual": {"wall_ms": 300, "recipient_count": 100, "success_count": 100, "failure_count": 0},
                        "batch": {"wall_ms": 150, "recipient_count": 100, "success_count": 100, "failure_count": 0}
                    }
                })),
            });
        let pp = profile.modes.get_mut("payment_processor").unwrap();
        pp.scenarios
            .get_mut("S5")
            .unwrap()
            .record_repetition(Repetition {
                run: 1,
                status: CellStatus::Ok,
                wall_ms: Some(120),
                success_count: 1,
                failure_count: 0,
                fee_microtari: None,
                error: None,
                metrics: Some(serde_json::json!({
                    "s5_arms": {"batch": {"wall_ms": 120, "recipient_count": 100, "success_count": 100, "failure_count": 0}}
                })),
            });

        profile.refresh_computed_deltas();

        assert_eq!(
            profile.computed_deltas["scan_deltas"]["old_wallet"]["t_scan_s2_minus_b0_ms"],
            serde_json::json!(75)
        );
        assert_eq!(
            profile.computed_deltas["s5_throughput"]["comparisons"]["old_wallet_individual_over_payment_processor_batch"],
            serde_json::json!(2.5)
        );
    }
}
