use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tari_transaction_components::MicroMinotari;

use crate::versions::{MINOTARI_CLI_REV, PAYMENT_PROCESSOR_REV, TARI_CONSOLE_WALLET_REV};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub network: NetworkConfig,
    pub benchmark: BenchmarkConfig,
    pub paths: PathConfig,
    pub seeds: SeedConfig,
    pub modes: ModeConfig,
    pub versions: VersionConfig,
    #[serde(default)]
    pub funding: FundingConfig,
    #[serde(default)]
    pub timeouts: TimeoutConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub name: String,
    pub base_node_http_url: String,
    #[serde(default = "default_authority_http_url")]
    pub authority_http_url: String,
    #[serde(default)]
    pub mode1_base_node_service_peer: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkConfig {
    pub a_fund: String,
    pub c_min: u64,
    pub volume_target: u32,
    pub doubling_rounds: u32,
    pub fanout_outputs_per_tx: u32,
    pub concurrent_batches: Vec<u32>,
    pub s4_t_budget_secs: u64,
    pub s5_m: u32,
    pub s5_k: u32,
    pub fee_rate: String,
    #[serde(default = "default_scan_repetitions")]
    pub scan_repetitions: u32,
    #[serde(default = "default_scan_batch_size")]
    pub scan_batch_size: u64,
    #[serde(default)]
    pub mode1_live_topology: bool,
    #[serde(
        default = "default_mode1_payment_amount",
        alias = "mode1_scenario_amount"
    )]
    pub mode1_payment_amount: String,
    #[serde(default)]
    pub mode1_live_max_s1_txs: u32,
    #[serde(default)]
    pub mode1_live_max_s4_batch: u32,
    #[serde(default)]
    pub mode1_live_max_s5_items: u32,
    #[serde(default)]
    pub live_fresh_scan_cells: bool,
    #[serde(default)]
    pub mode2_live_scenarios: bool,
    #[serde(
        default = "default_mode2_payment_amount",
        alias = "mode2_scenario_amount"
    )]
    pub mode2_payment_amount: String,
    #[serde(default)]
    pub mode2_live_max_s1_txs: u32,
    #[serde(default)]
    pub mode2_live_max_s4_batch: u32,
    #[serde(default)]
    pub mode2_live_max_s5_txs: u32,
    #[serde(default)]
    pub mode3_live_topology: bool,
    #[serde(
        default = "default_mode3_payment_amount",
        alias = "mode3_scenario_amount"
    )]
    pub mode3_payment_amount: String,
    #[serde(default)]
    pub mode3_live_max_s1_batches: u32,
    #[serde(default)]
    pub mode3_live_max_s4_batch: u32,
    #[serde(default)]
    pub mode3_live_max_s5_items: u32,
    #[serde(default = "default_mode3_worker_sleep_secs")]
    pub mode3_worker_sleep_secs: u64,
}

impl BenchmarkConfig {
    pub fn mode1_scenario_amount(&self) -> &str {
        &self.mode1_payment_amount
    }

    pub fn mode2_scenario_amount(&self) -> &str {
        &self.mode2_payment_amount
    }

    pub fn mode3_scenario_amount(&self) -> &str {
        &self.mode3_payment_amount
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathConfig {
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub minotari_console_wallet: PathBuf,
    pub minotari_binary: PathBuf,
    pub minotari_node: PathBuf,
    pub payment_processor_binary: PathBuf,
    #[serde(default = "default_build_manifest")]
    pub build_manifest: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedConfig {
    pub old_wallet_env: String,
    pub new_wallet_env: String,
    pub payment_processor_env: String,
    pub wallet_password_env: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeConfig {
    pub old_wallet_grpc_address: String,
    pub new_wallet_database: PathBuf,
    pub payment_processor_listen: String,
    pub payment_receiver_listen: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionConfig {
    pub minotari_cli_rev: String,
    pub tari_console_wallet_rev: String,
    pub payment_processor_rev: String,
    #[serde(default = "default_base_node_rev")]
    pub base_node_rev: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FundingConfig {
    #[serde(default)]
    pub old_wallet: Option<FundingRecord>,
    #[serde(default)]
    pub new_wallet: Option<FundingRecord>,
    #[serde(default)]
    pub payment_processor: Option<FundingRecord>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FundingRecord {
    pub amount: String,
    pub tx_id: String,
    pub height: u64,
    #[serde(default)]
    pub birthday: Option<u16>,
    #[serde(default)]
    pub birthday_start_height: Option<u64>,
    #[serde(default)]
    pub construction_ms: Option<u128>,
    #[serde(default)]
    pub broadcast_to_mempool_ms: Option<u128>,
    #[serde(default)]
    pub broadcast_to_confirmed_at_c_min_ms: Option<u128>,
    #[serde(default)]
    pub tip_height_at_broadcast: Option<u64>,
    #[serde(default)]
    pub tip_height_at_confirmation: Option<u64>,
    #[serde(default)]
    pub shared_funding_fee_microtari: Option<u64>,
    #[serde(default)]
    pub funding_fee_attribution: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutConfig {
    pub startup_secs: u64,
    pub confirmation_secs: u64,
    pub scan_batch_secs: u64,
    pub transaction_lock_secs: u64,
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            startup_secs: 1_800,
            confirmation_secs: 1_200,
            scan_batch_secs: 300,
            transaction_lock_secs: 60,
        }
    }
}

impl Config {
    pub fn protocol_fingerprint(&self) -> anyhow::Result<String> {
        let runtime_sha256 = |path: &Path| {
            std::fs::read(path)
                .ok()
                .map(|bytes| hex::encode(Sha256::digest(bytes)))
                .unwrap_or_else(|| "missing".to_string())
        };
        let protocol = serde_json::json!({
            "network": self.network,
            "benchmark": self.benchmark,
            "paths": self.paths,
            "seeds": self.seeds,
            "modes": self.modes,
            "versions": self.versions,
            "timeouts": self.timeouts,
            "runtime_sha256": {
                "minotari": runtime_sha256(&self.paths.minotari_binary),
                "minotari_node": runtime_sha256(&self.paths.minotari_node),
                "minotari_console_wallet": runtime_sha256(&self.paths.minotari_console_wallet),
                "minotari_payment_processor": runtime_sha256(&self.paths.payment_processor_binary),
                "build_manifest": runtime_sha256(&self.paths.build_manifest),
            }
        });
        let bytes = serde_json::to_vec(&protocol).context("serializing protocol fingerprint")?;
        Ok(hex::encode(Sha256::digest(bytes)))
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        Self::load_with_validation(path, true)
    }

    pub fn load_prefunding_b0(path: &Path) -> anyhow::Result<Self> {
        Self::load_with_validation(path, false)
    }

    fn load_with_validation(path: &Path, require_live_funding: bool) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let mut config: Self = toml::from_str(&text)?;
        config.resolve_runtime_paths(&std::env::current_dir()?)?;
        config.validate_with_live_funding(require_live_funding)?;
        Ok(config)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        self.validate_with_live_funding(true)
    }

    pub fn validate_prefunding_b0(&self) -> anyhow::Result<()> {
        self.validate_with_live_funding(false)
    }

    fn validate_with_live_funding(&self, require_live_funding: bool) -> anyhow::Result<()> {
        if self.network.name.to_lowercase() != "esmeralda" {
            bail!("network.name must be esmeralda");
        }
        let scan_url = url::Url::parse(&self.network.base_node_http_url)
            .context("network.base_node_http_url")?;
        let authority_url = url::Url::parse(&self.network.authority_http_url)
            .context("network.authority_http_url")?;
        let canonical_live_shape = self.benchmark.mode1_live_topology
            && self.benchmark.mode2_live_scenarios
            && self.benchmark.mode3_live_topology
            && self.benchmark.live_fresh_scan_cells;
        if canonical_live_shape {
            if !matches!(scan_url.host_str(), Some("localhost" | "127.0.0.1" | "::1")) {
                bail!("canonical live runs require a local archival base_node_http_url");
            }
            if matches!(
                authority_url.host_str(),
                Some("localhost" | "127.0.0.1" | "::1")
            ) || self.network.authority_http_url == self.network.base_node_http_url
            {
                bail!("canonical live runs require a distinct remote authority_http_url");
            }
            if parse_amount(&self.benchmark.a_fund)?.0 != parse_amount("10000 T")?.0
                || self.benchmark.c_min != 3
                || self.benchmark.volume_target != 512
                || self.benchmark.doubling_rounds != 6
                || self.benchmark.fanout_outputs_per_tx != 8
                || self.benchmark.concurrent_batches != [8, 16, 32, 64, 128]
                || self.benchmark.s4_t_budget_secs != 900
                || self.benchmark.s5_m != 100
                || self.benchmark.s5_k != 10
                || self.fee_rate()?.0 != 5
                || self.benchmark.mode1_live_max_s1_txs != 0
                || self.benchmark.mode1_live_max_s4_batch != 0
                || self.benchmark.mode1_live_max_s5_items != 0
                || self.benchmark.mode2_live_max_s1_txs != 0
                || self.benchmark.mode2_live_max_s4_batch != 0
                || self.benchmark.mode2_live_max_s5_txs != 0
                || self.benchmark.mode3_live_max_s1_batches != 0
                || self.benchmark.mode3_live_max_s4_batch != 0
                || self.benchmark.mode3_live_max_s5_items != 0
            {
                bail!("canonical live runs require the uncapped reference benchmark parameters");
            }
        }
        if matches!(scan_url.host_str(), Some("localhost" | "127.0.0.1" | "::1"))
            && self.benchmark.mode1_live_topology
            && self.network.mode1_base_node_service_peer.is_none()
        {
            bail!(
                "network.mode1_base_node_service_peer is required when Mode 1 uses a local base node"
            );
        }
        if self.benchmark.c_min == 0 {
            bail!("benchmark.c_min must be greater than 0");
        }
        if self.benchmark.scan_repetitions == 0 {
            bail!("benchmark.scan_repetitions must be greater than 0");
        }
        if self.benchmark.scan_batch_size == 0 {
            bail!("benchmark.scan_batch_size must be greater than 0");
        }
        parse_amount(&self.benchmark.mode1_payment_amount)
            .context("benchmark.mode1_payment_amount")?;
        parse_amount(&self.benchmark.mode2_payment_amount)
            .context("benchmark.mode2_payment_amount")?;
        parse_amount(&self.benchmark.mode3_payment_amount)
            .context("benchmark.mode3_payment_amount")?;
        if self.benchmark.mode3_worker_sleep_secs == 0 {
            bail!("benchmark.mode3_worker_sleep_secs must be greater than 0");
        }
        if self.benchmark.s5_k == 0 || !self.benchmark.s5_m.is_multiple_of(self.benchmark.s5_k) {
            bail!("benchmark.s5_m must be a positive multiple of benchmark.s5_k");
        }
        if self.benchmark.volume_target == 0 {
            bail!("benchmark.volume_target must be greater than 0");
        }
        if self.benchmark.concurrent_batches.is_empty() {
            bail!("benchmark.concurrent_batches must not be empty");
        }
        if self.benchmark.concurrent_batches.contains(&0) {
            bail!("benchmark.concurrent_batches entries must be greater than 0");
        }
        if require_live_funding
            && self.benchmark.mode1_live_topology
            && self.funding.old_wallet.is_none()
        {
            bail!("funding.old_wallet must be set when benchmark.mode1_live_topology=true");
        }
        if require_live_funding
            && self.benchmark.mode2_live_scenarios
            && self.funding.new_wallet.is_none()
        {
            bail!("funding.new_wallet must be set when Mode 2 live sends are enabled");
        }
        if require_live_funding
            && self.benchmark.mode3_live_topology
            && self.funding.payment_processor.is_none()
        {
            bail!("funding.payment_processor must be set when benchmark.mode3_live_topology=true");
        }
        self.a_fund()?;
        self.fee_rate()?;
        self.funding.validate()?;
        if require_live_funding {
            self.funding.validate_live_birthdays(&self.benchmark)?;
        }
        Ok(())
    }

    /// Resolves runtime filesystem inputs once, without changing the persisted
    /// TOML representation. Existing paths are canonicalized; missing paths are
    /// still made absolute so subprocesses never inherit cwd-dependent values.
    pub fn resolve_runtime_paths(&mut self, base: &Path) -> anyhow::Result<()> {
        self.paths.data_dir = resolve_runtime_path(base, &self.paths.data_dir)?;
        self.paths.cache_dir = resolve_runtime_path(base, &self.paths.cache_dir)?;
        self.paths.minotari_console_wallet =
            resolve_runtime_path(base, &self.paths.minotari_console_wallet)?;
        self.paths.minotari_binary = resolve_runtime_path(base, &self.paths.minotari_binary)?;
        self.paths.minotari_node = resolve_runtime_path(base, &self.paths.minotari_node)?;
        self.paths.payment_processor_binary =
            resolve_runtime_path(base, &self.paths.payment_processor_binary)?;
        self.paths.build_manifest = resolve_runtime_path(base, &self.paths.build_manifest)?;
        self.modes.new_wallet_database =
            resolve_runtime_path(base, &self.modes.new_wallet_database)?;
        Ok(())
    }

    pub fn a_fund(&self) -> anyhow::Result<MicroMinotari> {
        parse_amount(&self.benchmark.a_fund).context("benchmark.a_fund")
    }

    pub fn fee_rate(&self) -> anyhow::Result<MicroMinotari> {
        parse_amount(&self.benchmark.fee_rate).context("benchmark.fee_rate")
    }

    pub fn scenario_defaults(&self) -> BTreeMap<String, serde_json::Value> {
        let mut defaults = BTreeMap::from([
            (
                "A_fund".to_string(),
                serde_json::json!(self.benchmark.a_fund),
            ),
            ("C_min".to_string(), serde_json::json!(self.benchmark.c_min)),
            (
                "volume_target".to_string(),
                serde_json::json!(self.benchmark.volume_target),
            ),
            (
                "doubling_rounds".to_string(),
                serde_json::json!(self.benchmark.doubling_rounds),
            ),
            (
                "fanout_outputs_per_tx".to_string(),
                serde_json::json!(self.benchmark.fanout_outputs_per_tx),
            ),
            (
                "concurrent_batches".to_string(),
                serde_json::json!(self.benchmark.concurrent_batches),
            ),
            (
                "S4_T_budget_secs".to_string(),
                serde_json::json!(self.benchmark.s4_t_budget_secs),
            ),
            ("S5_M".to_string(), serde_json::json!(self.benchmark.s5_m)),
            ("S5_K".to_string(), serde_json::json!(self.benchmark.s5_k)),
            (
                "fee_rate".to_string(),
                serde_json::json!(self.benchmark.fee_rate),
            ),
            (
                "scan_repetitions".to_string(),
                serde_json::json!(self.benchmark.scan_repetitions),
            ),
            (
                "scan_batch_size".to_string(),
                serde_json::json!(self.benchmark.scan_batch_size),
            ),
            (
                "mode1_live_topology".to_string(),
                serde_json::json!(self.benchmark.mode1_live_topology),
            ),
            (
                "base_node_http_url".to_string(),
                serde_json::json!(self.network.base_node_http_url),
            ),
            (
                "authority_http_url".to_string(),
                serde_json::json!(self.network.authority_http_url),
            ),
            (
                "mode1_base_node_service_peer".to_string(),
                serde_json::json!(self.network.mode1_base_node_service_peer),
            ),
            (
                "base_node_rev".to_string(),
                serde_json::json!(self.versions.base_node_rev),
            ),
            (
                "mode1_payment_amount".to_string(),
                serde_json::json!(self.benchmark.mode1_payment_amount),
            ),
            (
                "mode1_live_max_s1_txs".to_string(),
                serde_json::json!(self.benchmark.mode1_live_max_s1_txs),
            ),
            (
                "mode1_live_max_s4_batch".to_string(),
                serde_json::json!(self.benchmark.mode1_live_max_s4_batch),
            ),
            (
                "mode1_live_max_s5_items".to_string(),
                serde_json::json!(self.benchmark.mode1_live_max_s5_items),
            ),
            (
                "live_fresh_scan_cells".to_string(),
                serde_json::json!(self.benchmark.live_fresh_scan_cells),
            ),
            (
                "mode2_live_scenarios".to_string(),
                serde_json::json!(self.benchmark.mode2_live_scenarios),
            ),
            (
                "mode2_payment_amount".to_string(),
                serde_json::json!(self.benchmark.mode2_payment_amount),
            ),
            (
                "mode2_live_max_s1_txs".to_string(),
                serde_json::json!(self.benchmark.mode2_live_max_s1_txs),
            ),
            (
                "mode2_live_max_s4_batch".to_string(),
                serde_json::json!(self.benchmark.mode2_live_max_s4_batch),
            ),
            (
                "mode2_live_max_s5_txs".to_string(),
                serde_json::json!(self.benchmark.mode2_live_max_s5_txs),
            ),
            (
                "mode3_live_topology".to_string(),
                serde_json::json!(self.benchmark.mode3_live_topology),
            ),
            (
                "mode3_payment_amount".to_string(),
                serde_json::json!(self.benchmark.mode3_payment_amount),
            ),
            (
                "mode3_live_max_s1_batches".to_string(),
                serde_json::json!(self.benchmark.mode3_live_max_s1_batches),
            ),
            (
                "mode3_live_max_s4_batch".to_string(),
                serde_json::json!(self.benchmark.mode3_live_max_s4_batch),
            ),
            (
                "mode3_live_max_s5_items".to_string(),
                serde_json::json!(self.benchmark.mode3_live_max_s5_items),
            ),
            (
                "mode3_worker_sleep_secs".to_string(),
                serde_json::json!(self.benchmark.mode3_worker_sleep_secs),
            ),
            (
                "funding".to_string(),
                serde_json::json!(self.funding.as_map()),
            ),
            ("timeouts".to_string(), serde_json::json!(self.timeouts)),
        ]);
        defaults.insert(
            "protocol_fingerprint".to_string(),
            serde_json::json!(
                self.protocol_fingerprint()
                    .unwrap_or_else(|_| "unavailable".to_string())
            ),
        );
        defaults
    }

    pub fn timeout(&self, secs: u64) -> Duration {
        Duration::from_secs(secs)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            network: NetworkConfig {
                name: "esmeralda".to_string(),
                base_node_http_url: "https://rpc.esmeralda.tari.com".to_string(),
                authority_http_url: default_authority_http_url(),
                mode1_base_node_service_peer: None,
            },
            benchmark: BenchmarkConfig {
                a_fund: "10000 T".to_string(),
                c_min: 3,
                volume_target: 512,
                doubling_rounds: 6,
                fanout_outputs_per_tx: 8,
                concurrent_batches: vec![8, 16, 32, 64, 128],
                s4_t_budget_secs: 900,
                s5_m: 100,
                s5_k: 10,
                fee_rate: "5 uT".to_string(),
                scan_repetitions: default_scan_repetitions(),
                scan_batch_size: default_scan_batch_size(),
                mode1_live_topology: false,
                mode1_payment_amount: default_mode1_payment_amount(),
                mode1_live_max_s1_txs: 0,
                mode1_live_max_s4_batch: 0,
                mode1_live_max_s5_items: 0,
                live_fresh_scan_cells: false,
                mode2_live_scenarios: false,
                mode2_payment_amount: default_mode2_payment_amount(),
                mode2_live_max_s1_txs: 0,
                mode2_live_max_s4_batch: 0,
                mode2_live_max_s5_txs: 0,
                mode3_live_topology: false,
                mode3_payment_amount: default_mode3_payment_amount(),
                mode3_live_max_s1_batches: 0,
                mode3_live_max_s4_batch: 0,
                mode3_live_max_s5_items: 0,
                mode3_worker_sleep_secs: default_mode3_worker_sleep_secs(),
            },
            paths: PathConfig {
                data_dir: PathBuf::from(".bench-data"),
                cache_dir: PathBuf::from(".bench-cache"),
                minotari_console_wallet: PathBuf::from("tools/minotari_console_wallet"),
                minotari_binary: PathBuf::from("tools/minotari"),
                minotari_node: PathBuf::from("tools/minotari_node"),
                payment_processor_binary: PathBuf::from("tools/minotari_payment_processor"),
                build_manifest: default_build_manifest(),
            },
            seeds: SeedConfig {
                old_wallet_env: "HARNESS_SEED_OLD".to_string(),
                new_wallet_env: "HARNESS_SEED_NEW".to_string(),
                payment_processor_env: "HARNESS_SEED_PP".to_string(),
                wallet_password_env: "HARNESS_WALLET_PW".to_string(),
            },
            modes: ModeConfig {
                old_wallet_grpc_address: "http://127.0.0.1:18143".to_string(),
                new_wallet_database: PathBuf::from(".bench-data/new-wallet/wallet.db"),
                payment_processor_listen: "127.0.0.1:9145".to_string(),
                payment_receiver_listen: "127.0.0.1:9146".to_string(),
            },
            versions: VersionConfig {
                minotari_cli_rev: MINOTARI_CLI_REV.to_string(),
                tari_console_wallet_rev: TARI_CONSOLE_WALLET_REV.to_string(),
                payment_processor_rev: PAYMENT_PROCESSOR_REV.to_string(),
                base_node_rev: default_base_node_rev(),
            },
            funding: FundingConfig::default(),
            timeouts: TimeoutConfig::default(),
        }
    }
}

fn default_scan_batch_size() -> u64 {
    1_000
}

fn default_scan_repetitions() -> u32 {
    1
}

fn default_mode1_payment_amount() -> String {
    "1 T".to_string()
}

fn default_mode2_payment_amount() -> String {
    "1 T".to_string()
}

fn default_mode3_payment_amount() -> String {
    "1 T".to_string()
}

fn default_mode3_worker_sleep_secs() -> u64 {
    10
}

fn default_base_node_rev() -> String {
    "v5.4.0".to_string()
}

fn default_authority_http_url() -> String {
    "https://rpc.esmeralda.tari.com".to_string()
}

fn default_build_manifest() -> PathBuf {
    PathBuf::from("tools/build-manifest.json")
}

fn resolve_runtime_path(base: &Path, path: &Path) -> anyhow::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    if absolute.exists() {
        return absolute
            .canonicalize()
            .with_context(|| format!("canonicalizing runtime path {}", absolute.display()));
    }
    Ok(std::path::absolute(&absolute)?)
}

impl FundingConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        for (role, record) in self.records() {
            let Some(record) = record else {
                continue;
            };
            parse_amount(&record.amount).with_context(|| format!("funding.{role}.amount"))?;
            if record.tx_id.trim().is_empty() {
                bail!("funding.{role}.tx_id must not be empty");
            }
            if record.height == 0 {
                bail!("funding.{role}.height must be greater than 0");
            }
            if let Some(start_height) = record.birthday_start_height
                && start_height > record.height
            {
                bail!("funding.{role}.birthday_start_height must not exceed the funding height");
            }
        }
        Ok(())
    }

    fn validate_live_birthdays(&self, benchmark: &BenchmarkConfig) -> anyhow::Result<()> {
        let requirements = [
            (
                "old_wallet",
                benchmark.mode1_live_topology,
                self.old_wallet.as_ref(),
            ),
            (
                "new_wallet",
                benchmark.mode2_live_scenarios,
                self.new_wallet.as_ref(),
            ),
            (
                "payment_processor",
                benchmark.mode3_live_topology,
                self.payment_processor.as_ref(),
            ),
        ];
        for (role, live, record) in requirements {
            if !live {
                continue;
            }
            let record = record.with_context(|| format!("funding.{role} must be set"))?;
            if parse_amount(&record.amount)? != parse_amount(&benchmark.a_fund)? {
                bail!("funding.{role}.amount must equal benchmark.a_fund for a pristine live run");
            }
            if record.birthday.is_none() {
                bail!("funding.{role}.birthday must be set for live runs");
            }
            if record.birthday_start_height.is_none() {
                bail!("funding.{role}.birthday_start_height must be set for live runs");
            }
        }
        Ok(())
    }

    pub fn records(&self) -> [(&'static str, Option<&FundingRecord>); 3] {
        [
            ("old_wallet", self.old_wallet.as_ref()),
            ("new_wallet", self.new_wallet.as_ref()),
            ("payment_processor", self.payment_processor.as_ref()),
        ]
    }

    pub fn as_map(&self) -> BTreeMap<String, FundingRecord> {
        self.records()
            .into_iter()
            .filter_map(|(role, record)| record.cloned().map(|record| (role.to_string(), record)))
            .collect()
    }
}

pub fn parse_amount(input: &str) -> anyhow::Result<MicroMinotari> {
    let normalized = input.replace("uT", "µT");
    MicroMinotari::from_str(normalized.trim()).map_err(|e| anyhow::anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_validates() {
        Config::default().validate().unwrap();
    }

    #[test]
    fn amount_parser_uses_tari_type() {
        assert_eq!(parse_amount("5 uT").unwrap(), MicroMinotari(5));
        assert_eq!(parse_amount("1 T").unwrap(), MicroMinotari(1_000_000));
    }

    #[test]
    fn s5_requires_even_batches() {
        let mut cfg = Config::default();
        cfg.benchmark.s5_m = 101;
        let error = cfg.validate().unwrap_err().to_string();
        assert!(error.contains("s5_m"));
    }

    #[test]
    fn funding_records_validate_amounts_and_heights() {
        let mut cfg = Config::default();
        cfg.funding.new_wallet = Some(FundingRecord {
            amount: "10000 T".to_string(),
            tx_id: "synthetic-funding-tx".to_string(),
            height: 100,
            birthday: None,
            birthday_start_height: None,
            ..FundingRecord::default()
        });
        cfg.validate().unwrap();
        assert_eq!(cfg.funding.as_map()["new_wallet"].height, 100);

        cfg.funding.new_wallet.as_mut().unwrap().height = 0;
        let error = cfg.validate().unwrap_err().to_string();
        assert!(error.contains("funding.new_wallet.height"));
    }

    #[test]
    fn scan_batch_size_must_be_positive() {
        let mut cfg = Config::default();
        cfg.benchmark.scan_batch_size = 0;
        let error = cfg.validate().unwrap_err().to_string();
        assert!(error.contains("scan_batch_size"));
    }

    #[test]
    fn mode1_live_scenario_amount_must_parse() {
        let mut cfg = Config::default();
        cfg.benchmark.mode1_payment_amount = "not money".to_string();
        let error = cfg.validate().unwrap_err().to_string();
        assert!(error.contains("mode1_payment_amount"));
    }

    #[test]
    fn mode2_live_scenario_amount_must_parse() {
        let mut cfg = Config::default();
        cfg.benchmark.mode2_payment_amount = "not money".to_string();
        let error = cfg.validate().unwrap_err().to_string();
        assert!(error.contains("mode2_payment_amount"));
    }

    #[test]
    fn mode3_live_scenario_amount_must_parse() {
        let mut cfg = Config::default();
        cfg.benchmark.mode3_payment_amount = "not money".to_string();
        let error = cfg.validate().unwrap_err().to_string();
        assert!(error.contains("mode3_payment_amount"));
    }

    #[test]
    fn mode3_worker_sleep_must_be_positive() {
        let mut cfg = Config::default();
        cfg.benchmark.mode3_worker_sleep_secs = 0;
        let error = cfg.validate().unwrap_err().to_string();
        assert!(error.contains("mode3_worker_sleep_secs"));
    }

    #[test]
    fn concurrent_batches_must_be_positive() {
        let mut cfg = Config::default();
        cfg.benchmark.concurrent_batches = vec![8, 0, 16];
        let error = cfg.validate().unwrap_err().to_string();
        assert!(error.contains("concurrent_batches"));
    }

    #[test]
    fn legacy_scenario_amount_names_deserialize_as_payment_amounts() {
        let legacy = toml::to_string(&Config::default())
            .unwrap()
            .replace("mode1_payment_amount", "mode1_scenario_amount")
            .replace("mode2_payment_amount", "mode2_scenario_amount")
            .replace("mode3_payment_amount", "mode3_scenario_amount");
        let parsed: Config = toml::from_str(&legacy).unwrap();
        assert_eq!(parsed.benchmark.mode1_payment_amount, "1 T");
        assert_eq!(parsed.benchmark.mode2_payment_amount, "1 T");
        assert_eq!(parsed.benchmark.mode3_payment_amount, "1 T");
    }

    #[test]
    fn live_funding_requires_birthday_and_resolved_start_height() {
        let mut cfg = Config::default();
        cfg.benchmark.mode2_live_scenarios = true;
        cfg.funding.new_wallet = Some(FundingRecord {
            amount: "10000 T".to_string(),
            tx_id: "123".to_string(),
            height: 700_000,
            birthday: None,
            birthday_start_height: None,
            ..FundingRecord::default()
        });

        let error = cfg.validate().unwrap_err().to_string();
        assert!(error.contains("funding.new_wallet.birthday"));

        let record = cfg.funding.new_wallet.as_mut().unwrap();
        record.birthday = Some(1_600);
        record.birthday_start_height = Some(690_000);
        cfg.validate().unwrap();
    }

    #[test]
    fn local_mode1_endpoint_requires_an_explicit_service_peer() {
        let mut cfg = Config::default();
        cfg.network.base_node_http_url = "http://127.0.0.1:18142".to_string();
        cfg.benchmark.mode1_live_topology = true;
        assert!(
            cfg.validate_prefunding_b0()
                .unwrap_err()
                .to_string()
                .contains("mode1_base_node_service_peer")
        );
        cfg.network.mode1_base_node_service_peer =
            Some("abc::/ip4/127.0.0.1/tcp/18189".to_string());
        cfg.validate_prefunding_b0().unwrap();
    }

    #[test]
    fn prefunding_b0_validation_allows_live_shape_without_funding() {
        let mut cfg = Config::default();
        cfg.benchmark.mode1_live_topology = true;
        cfg.benchmark.mode2_live_scenarios = true;
        cfg.benchmark.mode3_live_topology = true;
        cfg.benchmark.live_fresh_scan_cells = true;
        cfg.network.base_node_http_url = "http://127.0.0.1:18142".to_string();
        cfg.network.mode1_base_node_service_peer =
            Some("abc::/ip4/127.0.0.1/tcp/18189".to_string());

        assert!(cfg.validate_prefunding_b0().is_ok());
        assert!(
            cfg.validate()
                .unwrap_err()
                .to_string()
                .contains("funding.old_wallet")
        );
    }

    #[test]
    fn canonical_live_shape_rejects_capped_reference_parameters_early() {
        let mut cfg = Config::default();
        cfg.benchmark.mode1_live_topology = true;
        cfg.benchmark.mode2_live_scenarios = true;
        cfg.benchmark.mode3_live_topology = true;
        cfg.benchmark.live_fresh_scan_cells = true;
        cfg.benchmark.mode1_live_max_s1_txs = 1;
        cfg.network.base_node_http_url = "http://127.0.0.1:18142".to_string();
        cfg.network.mode1_base_node_service_peer =
            Some("abc::/ip4/127.0.0.1/tcp/18189".to_string());

        assert!(
            cfg.validate_prefunding_b0()
                .unwrap_err()
                .to_string()
                .contains("uncapped reference benchmark parameters")
        );
    }

    #[test]
    fn runtime_paths_are_absolute_without_persisting_changes() {
        let base = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.resolve_runtime_paths(base.path()).unwrap();
        assert!(cfg.paths.data_dir.is_absolute());
        assert!(cfg.paths.minotari_binary.is_absolute());
        assert!(cfg.modes.new_wallet_database.is_absolute());
    }
}
