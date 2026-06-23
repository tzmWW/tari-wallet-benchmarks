use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use tari_transaction_components::MicroMinotari;

use crate::versions::{MINOTARI_CLI_REV, PAYMENT_PROCESSOR_REV};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub network: NetworkConfig,
    pub benchmark: BenchmarkConfig,
    pub paths: PathConfig,
    pub seeds: SeedConfig,
    pub modes: ModeConfig,
    pub versions: VersionConfig,
    #[serde(default)]
    pub timeouts: TimeoutConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub name: String,
    pub base_node_http_url: String,
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
    pub repetitions: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathConfig {
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub minotari_console_wallet: PathBuf,
    pub minotari_binary: PathBuf,
    pub payment_processor_binary: PathBuf,
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
    pub payment_processor_rev: String,
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
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&text)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.network.name.to_lowercase() != "esmeralda" {
            bail!("network.name must be esmeralda");
        }
        if self.benchmark.c_min == 0 {
            bail!("benchmark.c_min must be greater than 0");
        }
        if self.benchmark.repetitions == 0 {
            bail!("benchmark.repetitions must be greater than 0");
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
        self.a_fund()?;
        self.fee_rate()?;
        Ok(())
    }

    pub fn a_fund(&self) -> anyhow::Result<MicroMinotari> {
        parse_amount(&self.benchmark.a_fund).context("benchmark.a_fund")
    }

    pub fn fee_rate(&self) -> anyhow::Result<MicroMinotari> {
        parse_amount(&self.benchmark.fee_rate).context("benchmark.fee_rate")
    }

    pub fn scenario_defaults(&self) -> BTreeMap<String, serde_json::Value> {
        BTreeMap::from([
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
                "repetitions".to_string(),
                serde_json::json!(self.benchmark.repetitions),
            ),
        ])
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
                repetitions: 3,
            },
            paths: PathConfig {
                data_dir: PathBuf::from(".bench-data"),
                cache_dir: PathBuf::from(".bench-cache"),
                minotari_console_wallet: PathBuf::from("tools/minotari_console_wallet"),
                minotari_binary: PathBuf::from("tools/minotari"),
                payment_processor_binary: PathBuf::from(
                    ".bench-cache/minotari_payment_processor/target/release/minotari_payment_processor",
                ),
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
                payment_processor_rev: PAYMENT_PROCESSOR_REV.to_string(),
            },
            timeouts: TimeoutConfig::default(),
        }
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
}
