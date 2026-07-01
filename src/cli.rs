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
        #[arg(long, default_value = "baselines/esmeralda_baseline.json")]
        profile: PathBuf,
        #[arg(long)]
        fresh_data_dir: bool,
        #[arg(long)]
        yes: bool,
    },
    FundOneSided {
        #[arg(long, default_value = "harness.toml")]
        config: PathBuf,
        #[arg(long)]
        source_db: PathBuf,
        #[arg(long)]
        recipient: String,
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
    },
    Schema {
        #[arg(long, default_value = "RESULT_PROFILE_SCHEMA.json")]
        out: PathBuf,
    },
}
