use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "wallet-bench")]
#[command(about = "Tari wallet benchmark harness for Esmeralda")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Addresses {
        #[arg(long, default_value = "harness.toml")]
        config: PathBuf,
        #[arg(long, default_value = ".secrets/seeds.env")]
        out: PathBuf,
    },
    Preflight {
        #[arg(long, default_value = "harness.toml")]
        config: PathBuf,
        #[arg(long)]
        check_funds: bool,
        #[arg(long)]
        mode1_db: Option<PathBuf>,
        #[arg(long)]
        mode2_db: Option<PathBuf>,
        #[arg(long)]
        payment_receiver_db: Option<PathBuf>,
    },
    Run {
        #[arg(long, default_value = "harness.toml")]
        config: PathBuf,
        #[arg(long, default_value = "candidates/esmeralda-baseline.json")]
        profile: PathBuf,
        /// Immutable checkpoint produced before any benchmark address is
        /// funded. Required for the funded B0->S0 continuation.
        #[arg(long)]
        b0_profile: PathBuf,
        #[arg(long)]
        s0_evidence: PathBuf,
    },
    /// Run B0, S0 funding, the uncapped benchmark, validation, and summary in
    /// one process. Launch-invariant disk and build checks execute once.
    BaselineWorkflow {
        #[arg(long, default_value = "harness.toml")]
        config: PathBuf,
        #[arg(long)]
        source_db: PathBuf,
        #[arg(long)]
        b0_profile: PathBuf,
        #[arg(long)]
        s0_evidence: PathBuf,
        #[arg(long)]
        profile: PathBuf,
        #[arg(long)]
        summary: PathBuf,
    },
    PrepareB0 {
        #[arg(long, default_value = "harness.toml")]
        config: PathBuf,
        #[arg(long)]
        profile: PathBuf,
    },
    FundS0 {
        #[arg(long, default_value = "harness.toml")]
        config: PathBuf,
        #[arg(long)]
        source_db: PathBuf,
        #[arg(long)]
        b0_profile: PathBuf,
        #[arg(long)]
        evidence_out: PathBuf,
    },
    FundOneSided {
        #[arg(long, default_value = "harness.toml")]
        config: PathBuf,
        #[arg(long)]
        source_db: PathBuf,
        #[arg(long, required = true)]
        recipient: Vec<String>,
        #[arg(long)]
        amount: String,
        #[arg(long, default_value_t = 1)]
        outputs: u32,
        #[arg(long, default_value_t = 1)]
        batch_size: u32,
    },
    ScanWallet {
        #[arg(long, default_value = "harness.toml")]
        config: PathBuf,
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        seed_env: Option<String>,
        /// Rewrite the seed birthday when initializing a new signing DB. The
        /// address is unchanged; this avoids unnecessary genesis recovery.
        #[arg(long)]
        birthday: Option<u16>,
        /// Scan to this exact height instead of capturing the current tip.
        /// Requires --target-hash.
        #[arg(long, requires = "target_hash")]
        target_height: Option<u64>,
        /// Hex block hash for --target-height.
        #[arg(long, requires = "target_height")]
        target_hash: Option<String>,
    },
    RecoverMode1Wallet {
        #[arg(long, default_value = "harness.toml")]
        config: PathBuf,
    },
    /// Submit one operator-controlled one-sided transfer from the configured
    /// Mode 1 console-wallet database. Intended for post-run fund recovery.
    SweepMode1 {
        #[arg(long, default_value = "harness.toml")]
        config: PathBuf,
        #[arg(long)]
        recipient: String,
        #[arg(long)]
        amount: String,
    },
    QueryTx {
        #[arg(long, default_value = "harness.toml")]
        config: PathBuf,
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        tx_id: u64,
    },
    Schema {
        #[arg(long, default_value = "RESULT_PROFILE_SCHEMA.json")]
        out: PathBuf,
    },
    ValidateProfile {
        #[arg(long)]
        profile: PathBuf,
        #[arg(long)]
        submission: bool,
        /// Validate the committed historical schema-v5 profile.
        #[arg(long)]
        legacy_v5: bool,
    },
    SummarizeProfile {
        #[arg(long)]
        profile: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// Summarize the committed historical schema-v5 profile.
        #[arg(long)]
        legacy_v5: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_scan_target_requires_height_and_hash() {
        assert!(
            Cli::try_parse_from([
                "wallet-bench",
                "scan-wallet",
                "--db",
                "wallet.db",
                "--target-height",
                "100",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "wallet-bench",
                "scan-wallet",
                "--db",
                "wallet.db",
                "--target-height",
                "100",
                "--target-hash",
                &"aa".repeat(32),
            ])
            .is_ok()
        );
    }
}
