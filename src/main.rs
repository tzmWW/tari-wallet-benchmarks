use anyhow::Context;
use clap::Parser;
use wallet_bench::{
    cli::{Cli, Command},
    config::Config,
    guards::enforce_esmeralda,
    result_profile::write_schema,
    runner::{generate_addresses, preflight, run_profile},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Addresses { config, out } => {
            let config =
                Config::load(&config).with_context(|| format!("loading {}", config.display()))?;
            enforce_esmeralda(&config)?;
            generate_addresses(&config, &out)?;
        }
        Command::Preflight { config } => {
            let config =
                Config::load(&config).with_context(|| format!("loading {}", config.display()))?;
            enforce_esmeralda(&config)?;
            preflight(&config).await?;
        }
        Command::Run { config, profile } => {
            let config =
                Config::load(&config).with_context(|| format!("loading {}", config.display()))?;
            enforce_esmeralda(&config)?;
            run_profile(&config, &profile).await?;
        }
        Command::Schema { out } => write_schema(&out)?,
    }

    Ok(())
}
