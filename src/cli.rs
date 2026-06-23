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
    },
    Run {
        #[arg(long, default_value = "harness.toml")]
        config: PathBuf,
        #[arg(long, default_value = "baselines/esmeralda_baseline.json")]
        profile: PathBuf,
    },
    Schema {
        #[arg(long, default_value = "RESULT_PROFILE_SCHEMA.json")]
        out: PathBuf,
    },
}
