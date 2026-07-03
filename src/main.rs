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
        Command::Preflight {
            config,
            check_funds,
            mode1_db,
            mode2_db,
            payment_receiver_db,
        } => {
            let config =
                Config::load(&config).with_context(|| format!("loading {}", config.display()))?;
            enforce_esmeralda(&config)?;
            preflight(
                &config,
                check_funds,
                mode1_db,
                mode2_db,
                payment_receiver_db,
            )
            .await?;
        }
        Command::Run {
            config,
            profile,
            fresh_data_dir,
            yes,
        } => {
            let config =
                Config::load(&config).with_context(|| format!("loading {}", config.display()))?;
            enforce_esmeralda(&config)?;
            run_profile(&config, &profile, fresh_data_dir, yes).await?;
        }
        Command::FundOneSided {
            config,
            source_db,
            recipient,
            amount,
            outputs,
            batch_size,
        } => {
            let config =
                Config::load(&config).with_context(|| format!("loading {}", config.display()))?;
            enforce_esmeralda(&config)?;
            #[cfg(feature = "live-minotari")]
            {
                wallet_bench::live_minotari::fund_one_sided_outputs(
                    &config, &source_db, &recipient, &amount, outputs, batch_size,
                )
                .await?;
            }
            #[cfg(not(feature = "live-minotari"))]
            {
                let _ = (&source_db, &recipient, &amount, outputs, batch_size);
                anyhow::bail!("fund-one-sided requires --features live-minotari");
            }
        }
        Command::ScanWallet {
            config,
            db,
            seed_env,
        } => {
            let config =
                Config::load(&config).with_context(|| format!("loading {}", config.display()))?;
            enforce_esmeralda(&config)?;
            #[cfg(feature = "live-minotari")]
            {
                wallet_bench::live_minotari::scan_wallet_db(&config, &db, seed_env.as_deref())
                    .await?;
            }
            #[cfg(not(feature = "live-minotari"))]
            {
                let _ = (db, seed_env);
                anyhow::bail!("scan-wallet requires --features live-minotari");
            }
        }
        Command::RecoverMode1Wallet { config } => {
            let config =
                Config::load(&config).with_context(|| format!("loading {}", config.display()))?;
            enforce_esmeralda(&config)?;
            #[cfg(feature = "live-minotari")]
            {
                wallet_bench::live_minotari::recover_mode1_console_wallet(&config).await?;
            }
            #[cfg(not(feature = "live-minotari"))]
            {
                anyhow::bail!("recover-mode1-wallet requires --features live-minotari");
            }
        }
        Command::QueryTx { config, db, tx_id } => {
            let config =
                Config::load(&config).with_context(|| format!("loading {}", config.display()))?;
            enforce_esmeralda(&config)?;
            #[cfg(feature = "live-minotari")]
            {
                wallet_bench::live_minotari::query_wallet_transaction(&config, &db, tx_id).await?;
            }
            #[cfg(not(feature = "live-minotari"))]
            {
                let _ = (db, tx_id);
                anyhow::bail!("query-tx requires --features live-minotari");
            }
        }
        Command::Schema { out } => write_schema(&out)?,
    }

    Ok(())
}
