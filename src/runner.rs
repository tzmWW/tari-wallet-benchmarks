use std::{env, path::Path};

use anyhow::{Context, bail};

use crate::{
    config::Config,
    env_capture,
    modes::ModeName,
    payment_processor,
    result_profile::{ResultProfile, empty_mode_profile},
    seeds::{AddressBook, WalletRole},
};

pub fn generate_addresses(config: &Config, out: &Path) -> anyhow::Result<()> {
    let book = AddressBook::from_config_or_generate(config)?;
    book.write_env_file(out)?;
    println!("{}", serde_json::to_string_pretty(&book.public_summary())?);
    println!("wrote seed env file to {}", out.display());
    Ok(())
}

pub async fn preflight(config: &Config) -> anyhow::Result<()> {
    let book = AddressBook::from_config_or_generate(config)?;
    require_env(&config.seeds.wallet_password_env)?;
    let mut missing = Vec::new();
    if !config.paths.minotari_console_wallet.exists() || !config.paths.minotari_binary.exists() {
        println!(
            "minotari binaries missing; fetch/build with: scripts/fetch-minotari-cli.sh {} tools",
            config.paths.cache_dir.display()
        );
    }
    if let Err(error) = check_binary(
        &config.paths.minotari_console_wallet,
        "minotari_console_wallet",
    ) {
        missing.push(error.to_string());
    }
    if let Err(error) = check_binary(&config.paths.minotari_binary, "minotari") {
        missing.push(error.to_string());
    }
    if !config.paths.payment_processor_binary.exists() {
        println!(
            "payment processor binary missing: {}\nfetch/build with: {}",
            config.paths.payment_processor_binary.display(),
            payment_processor::build_fetch_command(&config.paths.cache_dir)
        );
        missing.push(format!(
            "payment processor binary not found at {}",
            config.paths.payment_processor_binary.display()
        ));
    }
    if !missing.is_empty() {
        bail!("preflight failed:\n{}", missing.join("\n"));
    }

    for (role, material) in &book.addresses {
        println!("{role}: {}", material.address);
    }
    println!("preflight PASS: config and seed material are Esmeralda-scoped");
    Ok(())
}

pub async fn run_profile(config: &Config, profile_path: &Path) -> anyhow::Result<()> {
    let book = AddressBook::from_config_or_generate(config)?;
    if book
        .addresses
        .values()
        .any(|seed| env::var(&seed.env_var).is_err())
    {
        bail!(
            "seed env vars are not all set; run addresses and source the generated .secrets/seeds.env first"
        );
    }
    require_env(&config.seeds.wallet_password_env)?;

    let mut profile = ResultProfile::new(config, env_capture::capture());
    for mode in ModeName::ALL {
        let address = match mode {
            ModeName::OldWallet => book.addresses.get(WalletRole::OldWallet.label()),
            ModeName::NewWallet => book.addresses.get(WalletRole::NewWallet.label()),
            ModeName::PaymentProcessor => book.addresses.get(WalletRole::PaymentProcessor.label()),
        }
        .map(|seed| seed.address.clone());
        profile
            .modes
            .insert(mode.as_str().to_string(), empty_mode_profile(mode, address));
    }

    if let Some(pp_seed) = book.addresses.get(WalletRole::PaymentProcessor.label()) {
        let pp_env = payment_processor::build_env(config, pp_seed);
        profile.config.insert(
            "mode3_env_template".to_string(),
            serde_json::to_value(pp_env.vars.keys().collect::<Vec<_>>())?,
        );
    }

    #[cfg(feature = "live-minotari")]
    {
        crate::live_minotari::annotate_profile_with_library_smoke(config, &book, &mut profile)
            .await?;
    }

    #[cfg(not(feature = "live-minotari"))]
    {
        for mode in profile.modes.values_mut() {
            for cell in mode.scenarios.values_mut() {
                cell.notes.push(
                    "built without live-minotari feature; this profile is a pre-live scaffold"
                        .to_string(),
                );
            }
        }
    }

    profile.write_atomic(profile_path)?;
    println!("wrote {}", profile_path.display());
    Ok(())
}

fn require_env(name: &str) -> anyhow::Result<String> {
    env::var(name).with_context(|| format!("${name} must be set"))
}

fn check_binary(path: &Path, label: &str) -> anyhow::Result<()> {
    if !path.exists() {
        bail!("{label} binary not found at {}", path.display());
    }
    Ok(())
}
