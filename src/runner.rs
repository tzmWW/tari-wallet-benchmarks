use std::{
    env, fs,
    fs::OpenOptions,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
#[cfg(feature = "live-minotari")]
use sha2::{Digest, Sha256};
use sysinfo::Disks;
#[cfg(feature = "live-minotari")]
use sysinfo::{Pid, System};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct S0FundingEvidence {
    pub schema_version: u32,
    pub b0_run_id: String,
    pub protocol_fingerprint: String,
    pub status: String,
    pub addresses: std::collections::BTreeMap<String, String>,
    pub birthday: u16,
    pub birthday_start_height: u64,
    pub submission: crate::result_profile::S0FundingSubmissionEvidence,
    pub transaction: Option<crate::result_profile::S0FundingTransactionEvidence>,
}

use crate::{
    config::Config,
    env_capture,
    modes::{ModeName, ScenarioName},
    payment_processor,
    result_profile::{
        CellStatus, ProfileKind, ResultProfile, empty_mode_profile, profile_validation,
    },
    seeds::{AddressBook, WalletRole},
};

pub fn generate_addresses(config: &Config, out: &Path) -> anyhow::Result<()> {
    let book = AddressBook::generate_fresh(config)?;
    book.write_env_file(out)?;
    println!("{}", serde_json::to_string_pretty(&book.public_summary())?);
    println!("wrote seed env file to {}", out.display());
    Ok(())
}

pub async fn preflight(
    config: &Config,
    check_funds: bool,
    mode1_db: Option<PathBuf>,
    mode2_db: Option<PathBuf>,
    payment_receiver_db: Option<PathBuf>,
) -> anyhow::Result<()> {
    let book = AddressBook::load_required(config)?;
    preflight_checks(
        config,
        &book,
        check_funds,
        mode1_db.clone(),
        mode2_db.clone(),
        payment_receiver_db.clone(),
        true,
    )?;
    if check_funds {
        if mode1_db.is_none() {
            verify_mode1_wallet_identity(config, &book).await?;
        } else {
            println!(
                "old_wallet: custom --mode1-db audit skips gRPC identity proof; use the configured DB for strict live-run readiness"
            );
        }
        if mode2_db.is_none() && payment_receiver_db.is_none() {
            refresh_library_wallets_before_selected_chain_check(config).await?;
        }
        let paths = live_wallet_paths(config, mode1_db, mode2_db, payment_receiver_db);
        check_selected_chain_readiness(config, &paths).await?;
    }

    for (role, material) in &book.addresses {
        println!("{role}: {}", material.address);
    }
    println!("preflight PASS: config and seed material are Esmeralda-scoped");
    Ok(())
}

fn preflight_checks(
    config: &Config,
    book: &AddressBook,
    check_funds: bool,
    mode1_db: Option<PathBuf>,
    mode2_db: Option<PathBuf>,
    payment_receiver_db: Option<PathBuf>,
    verify_manifest: bool,
) -> anyhow::Result<()> {
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
    if let Err(error) = check_binary(&config.paths.minotari_node, "minotari_node") {
        missing.push(error.to_string());
    }
    if let Err(error) = check_binary(
        &config.paths.payment_processor_binary,
        "minotari_payment_processor",
    ) {
        println!(
            "payment processor binary missing: {}\nfetch/build with: {}",
            config.paths.payment_processor_binary.display(),
            payment_processor::build_fetch_command(&config.paths.cache_dir)
        );
        missing.push(error.to_string());
    }
    if !missing.is_empty() {
        bail!("preflight failed:\n{}", missing.join("\n"));
    }
    if verify_manifest {
        crate::build_manifest::verify(config)?;
    }

    if check_funds {
        let paths = live_wallet_paths(config, mode1_db, mode2_db, payment_receiver_db);
        check_live_funds_at_paths(config, &paths)?;
        check_live_wallet_identities(config, book, &paths)?;
        check_payment_processor_pristine(config)?;
    }
    Ok(())
}

/// Runs the fail-closed, non-spending readiness gate used by `run` before a
/// result profile or any live topology is created.
pub async fn preflight_for_live_run(config: &Config, book: &AddressBook) -> anyhow::Result<()> {
    preflight_for_live_run_inner(config, book, true).await
}

async fn preflight_for_live_run_inner(
    config: &Config,
    book: &AddressBook,
    run_launch_checks: bool,
) -> anyhow::Result<()> {
    ensure_runtime_paths_are_absolute(config)?;
    #[cfg(feature = "live-minotari")]
    recover_prepared_transactions(config).await?;
    check_harness_worktree_clean()?;
    if run_launch_checks {
        check_disk_space(config)?;
    }
    check_listen_ports_available(config)?;
    preflight_checks(config, book, true, None, None, None, run_launch_checks)
        .context("strict live-run preflight")?;
    verify_mode1_wallet_identity(config, book).await?;
    refresh_library_wallets_before_selected_chain_check(config).await?;
    let paths = live_wallet_paths(config, None, None, None);
    check_selected_chain_readiness(config, &paths)
        .await
        .context("selected-chain live-run preflight")
}

#[cfg(feature = "live-minotari")]
async fn recover_prepared_transactions(config: &Config) -> anyhow::Result<()> {
    let paths = live_wallet_paths(config, None, None, None);
    for path in [
        &paths.old_wallet,
        &paths.new_wallet,
        &paths.payment_processor,
    ] {
        crate::live_minotari::recover_prepared_transaction_checkpoint(
            path,
            &config.network.base_node_http_url,
            config.timeout(config.timeouts.startup_secs),
            config.benchmark.c_min,
        )
        .await?;
    }
    Ok(())
}

#[cfg(feature = "live-minotari")]
async fn refresh_library_wallets_before_selected_chain_check(
    config: &Config,
) -> anyhow::Result<()> {
    crate::live_minotari::refresh_library_wallets_to_tip(config).await
}

#[cfg(not(feature = "live-minotari"))]
async fn refresh_library_wallets_before_selected_chain_check(_: &Config) -> anyhow::Result<()> {
    bail!("strict wallet cursor refresh requires --features live-minotari")
}

#[cfg(feature = "live-minotari")]
async fn verify_mode1_wallet_identity(config: &Config, book: &AddressBook) -> anyhow::Result<()> {
    let seed = book
        .addresses
        .get(WalletRole::OldWallet.label())
        .context("old_wallet seed material is missing")?;
    crate::live_minotari::verify_mode1_wallet_identity(config, seed).await
}

#[cfg(not(feature = "live-minotari"))]
async fn verify_mode1_wallet_identity(_: &Config, _: &AddressBook) -> anyhow::Result<()> {
    bail!("strict Mode 1 identity preflight requires --features live-minotari")
}

pub async fn run_profile(
    config: &Config,
    profile_path: &Path,
    b0_profile_path: &Path,
    s0_evidence_path: &Path,
) -> anyhow::Result<()> {
    run_profile_inner(
        config,
        profile_path,
        b0_profile_path,
        s0_evidence_path,
        true,
    )
    .await
}

async fn run_profile_inner(
    config: &Config,
    profile_path: &Path,
    b0_profile_path: &Path,
    s0_evidence_path: &Path,
    run_launch_checks: bool,
) -> anyhow::Result<()> {
    ensure_runtime_paths_are_absolute(config)?;
    let config = config_with_s0_evidence(config, s0_evidence_path)?;
    let config = &config;
    let _namespace_lock = RunNamespaceLock::acquire(&config.paths.data_dir)?;
    let book = AddressBook::load_required(config)?;
    preflight_for_live_run_inner(config, &book, run_launch_checks).await?;

    let mut profile = ResultProfile::new(
        config,
        env_capture::capture_for_network_with_data_dir(
            &config.network.base_node_http_url,
            &config.network.authority_http_url,
            config.network.mode1_base_node_service_peer.as_deref(),
            Some(&config.paths.data_dir),
        ),
    );
    #[cfg(feature = "live-minotari")]
    {
        check_endpoint_authority(config).await?;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        let tip = fetch_chain_tip(&client, &config.network.base_node_http_url).await?;
        profile.set_tip_start(tip.height, Some(hex::encode(&tip.hash)));
        let authority = fetch_chain_tip(&client, &config.network.authority_http_url).await?;
        profile.base_node.authority_tip_start_height = Some(authority.height);
        profile.base_node.authority_tip_start_hash = Some(hex::encode(authority.hash));
        profile.base_node.pruning_horizon = Some(tip.pruning_horizon);
        profile.base_node.is_synced = Some(tip.is_synced);
    }
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
    record_seed_fingerprints(&mut profile, &book);

    if let Some(pp_seed) = book.addresses.get(WalletRole::PaymentProcessor.label()) {
        let pp_env = payment_processor::build_env(config, pp_seed);
        profile.config.insert(
            "mode3_env_template".to_string(),
            serde_json::to_value(pp_env.vars.keys().collect::<Vec<_>>())?,
        );
    }

    import_prefunding_b0(config, &mut profile, b0_profile_path)?;
    validate_s0_funding_evidence(config, &profile, s0_evidence_path)?;

    #[cfg(feature = "live-minotari")]
    {
        crate::live_minotari::annotate_profile_with_library_smoke(
            config,
            &book,
            &mut profile,
            Some(profile_path),
            true,
        )
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
        profile.mark_checkpoint_stage("scaffold");
    }

    #[cfg(feature = "live-minotari")]
    {
        check_endpoint_authority(config).await?;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        let tip = fetch_chain_tip(&client, &config.network.base_node_http_url).await?;
        profile.set_tip_end(tip.height, Some(hex::encode(&tip.hash)));
        let authority = fetch_chain_tip(&client, &config.network.authority_http_url).await?;
        profile.base_node.authority_tip_end_height = Some(authority.height);
        profile.base_node.authority_tip_end_hash = Some(hex::encode(authority.hash));
        profile.base_node.pruning_horizon = Some(tip.pruning_horizon);
        profile.base_node.is_synced = Some(tip.is_synced);
        profile.mark_final();
    }
    profile.refresh_computed_deltas();
    profile.write_validated_atomic(profile_path, true)?;
    println!("wrote {}", profile_path.display());
    Ok(())
}

#[cfg(feature = "live-minotari")]
pub async fn prepare_b0_profile(config: &Config, profile_path: &Path) -> anyhow::Result<()> {
    prepare_b0_profile_inner(config, profile_path, true).await
}

#[cfg(feature = "live-minotari")]
async fn prepare_b0_profile_inner(
    config: &Config,
    profile_path: &Path,
    run_launch_checks: bool,
) -> anyhow::Result<()> {
    config.validate_prefunding_b0()?;
    let _namespace_lock = RunNamespaceLock::acquire(&config.paths.data_dir)?;
    check_harness_worktree_clean()?;
    let book = AddressBook::load_required(config)?;
    ensure_runtime_paths_are_absolute(config)?;
    if run_launch_checks {
        check_disk_space(config)?;
    }
    check_listen_ports_available(config)?;
    preflight_checks(config, &book, false, None, None, None, run_launch_checks)?;
    check_endpoint_authority(config).await?;
    let mut profile = ResultProfile::new(
        config,
        env_capture::capture_for_network_with_data_dir(
            &config.network.base_node_http_url,
            &config.network.authority_http_url,
            config.network.mode1_base_node_service_peer.as_deref(),
            Some(&config.paths.data_dir),
        ),
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let tip = fetch_chain_tip(&client, &config.network.base_node_http_url).await?;
    if !tip.is_synced {
        bail!("prepare-b0 requires a base node synchronized to tip");
    }
    profile.set_tip_start(tip.height, Some(hex::encode(&tip.hash)));
    let authority = fetch_chain_tip(&client, &config.network.authority_http_url).await?;
    profile.base_node.authority_tip_start_height = Some(authority.height);
    profile.base_node.authority_tip_start_hash = Some(hex::encode(authority.hash));
    profile.base_node.pruning_horizon = Some(tip.pruning_horizon);
    profile.base_node.is_synced = Some(tip.is_synced);
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
    record_seed_fingerprints(&mut profile, &book);
    crate::live_minotari::annotate_prefunding_b0(config, &book, &mut profile).await?;
    check_endpoint_authority(config).await?;
    let tip = fetch_chain_tip(&client, &config.network.base_node_http_url).await?;
    profile.set_tip_end(tip.height, Some(hex::encode(&tip.hash)));
    let authority = fetch_chain_tip(&client, &config.network.authority_http_url).await?;
    profile.base_node.authority_tip_end_height = Some(authority.height);
    profile.base_node.authority_tip_end_hash = Some(hex::encode(authority.hash));
    profile.base_node.is_synced = Some(tip.is_synced);
    profile.refresh_computed_deltas();
    validate_prefunding_b0_metrics(&profile)?;
    profile.write_validated_atomic(profile_path, false)?;
    println!("wrote pre-funding B0 checkpoint {}", profile_path.display());
    Ok(())
}

#[cfg(feature = "live-minotari")]
pub async fn fund_s0_from_checkpoint(
    config: &Config,
    source_db: &Path,
    b0_profile_path: &Path,
    evidence_path: &Path,
) -> anyhow::Result<()> {
    fund_s0_from_checkpoint_inner(config, source_db, b0_profile_path, evidence_path, true).await
}

#[cfg(feature = "live-minotari")]
async fn fund_s0_from_checkpoint_inner(
    config: &Config,
    source_db: &Path,
    b0_profile_path: &Path,
    evidence_path: &Path,
    run_launch_checks: bool,
) -> anyhow::Result<()> {
    config.validate_prefunding_b0()?;
    let _namespace_lock = RunNamespaceLock::acquire(&config.paths.data_dir)?;
    crate::live_minotari::recover_prepared_transaction_checkpoint(
        source_db,
        &config.network.base_node_http_url,
        config.timeout(config.timeouts.startup_secs),
        config.benchmark.c_min,
    )
    .await
    .context("recovering prepared source-wallet transaction before S0 funding")?;
    check_harness_worktree_clean()?;
    let checkpoint = profile_validation::validate_path(b0_profile_path, false)?;
    validate_prefunding_b0_metrics(&checkpoint)?;
    let book = AddressBook::load_required(config)?;
    let mut current = ResultProfile::new(
        config,
        env_capture::capture_for_network_with_data_dir(
            &config.network.base_node_http_url,
            &config.network.authority_http_url,
            config.network.mode1_base_node_service_peer.as_deref(),
            Some(&config.paths.data_dir),
        ),
    );
    record_seed_fingerprints(&mut current, &book);
    if checkpoint.provenance.export_commit != current.provenance.export_commit {
        bail!("fund-s0 B0 checkpoint harness commit does not match this binary");
    }
    if checkpoint.config.get("protocol_fingerprint") != current.config.get("protocol_fingerprint")
        || serde_json::to_value(&checkpoint.environment)?
            != serde_json::to_value(&current.environment)?
        || checkpoint.base_node.endpoint != current.base_node.endpoint
        || checkpoint.base_node.configured_revision != current.base_node.configured_revision
        || checkpoint.config.get("seed_fingerprints") != current.config.get("seed_fingerprints")
    {
        bail!("fund-s0 runtime fingerprint does not match the B0 checkpoint");
    }
    if checkpoint.profile_kind != ProfileKind::Checkpoint
        || !checkpoint
            .completed_stages
            .iter()
            .any(|stage| stage == "prefunding_b0")
    {
        bail!("fund-s0 requires a completed pre-funding B0 checkpoint");
    }
    ensure_runtime_paths_are_absolute(config)?;
    if run_launch_checks {
        check_disk_space(config)?;
    }
    check_listen_ports_available(config)?;
    preflight_checks(config, &book, false, None, None, None, run_launch_checks)?;
    check_endpoint_authority(config).await?;
    let mut addresses = std::collections::BTreeMap::new();
    let recipients = [
        (ModeName::OldWallet, WalletRole::OldWallet),
        (ModeName::NewWallet, WalletRole::NewWallet),
        (ModeName::PaymentProcessor, WalletRole::PaymentProcessor),
    ]
    .into_iter()
    .map(|(mode, role)| {
        let address = book
            .addresses
            .get(role.label())
            .with_context(|| format!("missing {} seed", role.label()))?
            .address
            .clone();
        if checkpoint.modes[mode.as_str()].address.as_deref() != Some(address.as_str()) {
            bail!("B0 checkpoint address mismatch for {}", mode.as_str());
        }
        addresses.insert(mode.as_str().to_string(), address.clone());
        Ok(address)
    })
    .collect::<anyhow::Result<Vec<_>>>()?;
    let protocol_fingerprint = config.protocol_fingerprint()?;
    let mut evidence = if evidence_path.exists() {
        let existing: S0FundingEvidence = serde_json::from_slice(&fs::read(evidence_path)?)?;
        if existing.schema_version != 2
            || existing.b0_run_id != checkpoint.run_id
            || existing.protocol_fingerprint != protocol_fingerprint
            || existing.addresses != addresses
        {
            bail!("existing S0 evidence does not match this B0 continuation");
        }
        existing
    } else {
        let (birthday, birthday_start_height) =
            crate::live_minotari::initialize_s0_wallets(config, &book).await?;
        let mut broadcast_evidence = None;
        let b0_run_id = checkpoint.run_id.clone();
        let evidence_addresses = addresses.clone();
        let evidence_protocol_fingerprint = protocol_fingerprint.clone();
        crate::live_minotari::submit_s0_outputs(config, source_db, &recipients, |submission| {
            let evidence = S0FundingEvidence {
                schema_version: 2,
                b0_run_id,
                protocol_fingerprint: evidence_protocol_fingerprint,
                status: "broadcast".to_string(),
                addresses: evidence_addresses,
                birthday,
                birthday_start_height,
                submission: submission.clone(),
                transaction: None,
            };
            write_json_atomic(evidence_path, &evidence)?;
            broadcast_evidence = Some(evidence);
            Ok(())
        })
        .await?;
        broadcast_evidence.context("S0 broadcast callback did not persist evidence")?
    };
    if evidence.status != "confirmed" || evidence.transaction.is_none() {
        let transaction =
            crate::live_minotari::observe_s0_funding(config, source_db, &evidence.submission)
                .await?;
        check_endpoint_authority(config).await?;
        evidence.status = "confirmed".to_string();
        evidence.transaction = Some(transaction);
        write_json_atomic(evidence_path, &evidence)?;
        println!("wrote S0 funding evidence {}", evidence_path.display());
    } else {
        println!(
            "S0 funding evidence is already confirmed at {}; verifying recipient readiness",
            evidence_path.display()
        );
    }
    let transaction = evidence
        .transaction
        .as_ref()
        .context("confirmed S0 evidence has no transaction")?;
    let funded_config = config_with_s0_evidence(config, evidence_path)?;
    crate::live_minotari::synchronize_s0_recipients(
        &funded_config,
        &book,
        evidence.birthday,
        transaction.mined_height,
    )
    .await?;
    preflight_for_live_run_inner(&funded_config, &book, false)
        .await
        .context("post-funding recipient readiness")?;
    println!("S0 recipient readiness PASS");
    Ok(())
}

#[cfg(feature = "live-minotari")]
pub async fn run_baseline_workflow(
    config: &Config,
    source_db: &Path,
    b0_profile_path: &Path,
    evidence_path: &Path,
    profile_path: &Path,
    summary_path: &Path,
) -> anyhow::Result<()> {
    config.validate_prefunding_b0()?;
    ensure_runtime_paths_are_absolute(config)?;
    check_harness_worktree_clean()?;
    let book = AddressBook::load_required(config)?;
    check_disk_space(config)?;
    preflight_checks(config, &book, false, None, None, None, true)?;
    println!("baseline workflow launch preflight PASS");

    prepare_b0_profile_inner(config, b0_profile_path, false).await?;
    fund_s0_from_checkpoint_inner(config, source_db, b0_profile_path, evidence_path, false).await?;
    run_profile_inner(config, profile_path, b0_profile_path, evidence_path, false).await?;
    profile_validation::validate_path(profile_path, true)?;
    println!("profile PASS: {}", profile_path.display());
    profile_validation::write_summary(profile_path, summary_path)?;
    println!("wrote {}", summary_path.display());
    Ok(())
}

#[cfg(feature = "live-minotari")]
fn write_json_atomic(path: &Path, value: &impl serde::Serialize) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, path)?;
    Ok(())
}

struct RunNamespaceLock {
    path: PathBuf,
}

impl RunNamespaceLock {
    fn acquire(data_dir: &Path) -> anyhow::Result<Self> {
        fs::create_dir_all(data_dir)?;
        let path = data_dir.join(".wallet-bench.lock");
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| {
                format!(
                    "candidate namespace is already locked at {}; use a new namespace if a prior process did not exit cleanly",
                    path.display()
                )
            })?;
        Ok(Self { path })
    }
}

impl Drop for RunNamespaceLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn import_prefunding_b0(
    config: &Config,
    profile: &mut ResultProfile,
    checkpoint_path: &Path,
) -> anyhow::Result<()> {
    let checkpoint =
        profile_validation::validate_path(checkpoint_path, false).with_context(|| {
            format!(
                "validating pre-funding B0 checkpoint {}",
                checkpoint_path.display()
            )
        })?;
    validate_prefunding_b0_metrics(&checkpoint)?;
    if checkpoint.profile_kind != ProfileKind::Checkpoint
        || !checkpoint
            .completed_stages
            .iter()
            .any(|stage| stage == "prefunding_b0")
        || checkpoint.config.get("prefunding_b0_checkpoint") != Some(&serde_json::json!(true))
    {
        bail!("B0 checkpoint is missing prefunding_b0 provenance");
    }
    if checkpoint.provenance.export_commit != profile.provenance.export_commit {
        bail!("B0 checkpoint harness commit does not match the funded continuation");
    }
    if checkpoint.config.get("protocol_fingerprint") != profile.config.get("protocol_fingerprint") {
        bail!("B0 checkpoint protocol fingerprint does not match the funded continuation");
    }
    if checkpoint.config.get("harness_executable_sha256")
        != profile.config.get("harness_executable_sha256")
    {
        bail!("B0 checkpoint harness executable does not match the funded continuation");
    }
    if serde_json::to_value(&checkpoint.environment)? != serde_json::to_value(&profile.environment)?
    {
        bail!("B0 checkpoint environment does not match the funded continuation");
    }
    if checkpoint.base_node.endpoint != profile.base_node.endpoint
        || checkpoint.base_node.configured_revision != profile.base_node.configured_revision
    {
        bail!("B0 checkpoint base-node identity does not match the funded continuation");
    }
    if checkpoint.network != profile.network {
        bail!("B0 checkpoint network does not match the funded continuation");
    }
    if checkpoint.config.get("seed_fingerprints") != profile.config.get("seed_fingerprints") {
        bail!("B0 checkpoint seed fingerprints do not match the funded continuation");
    }
    let b0_tip_end = checkpoint
        .base_node
        .tip_end_height
        .context("B0 checkpoint is missing H_tip_end")?;
    for (role, funding) in config.funding.records() {
        let funding = funding.with_context(|| format!("funding.{role} must be set"))?;
        if funding.height <= b0_tip_end {
            bail!(
                "funding.{role} height {} must be after pre-funding B0 H_tip_end {b0_tip_end}",
                funding.height
            );
        }
    }
    for mode in ModeName::ALL {
        let mode_name = mode.as_str();
        let source = checkpoint
            .modes
            .get(mode_name)
            .with_context(|| format!("B0 checkpoint missing mode {mode_name}"))?;
        let target = profile
            .modes
            .get_mut(mode_name)
            .with_context(|| format!("continuation missing mode {mode_name}"))?;
        if source.address != target.address {
            bail!("B0 checkpoint address mismatch for {mode_name}");
        }
        target.scenarios.insert(
            ScenarioName::B0.as_str().to_string(),
            source.scenarios[ScenarioName::B0.as_str()].clone(),
        );
    }
    profile.base_node.tip_start_height = checkpoint.base_node.tip_start_height;
    profile.base_node.tip_start_hash = checkpoint.base_node.tip_start_hash;
    profile.base_node.authority_tip_start_height = checkpoint.base_node.authority_tip_start_height;
    profile.base_node.authority_tip_start_hash = checkpoint.base_node.authority_tip_start_hash;
    profile.mark_checkpoint_stage("prefunding_b0");
    profile.config.insert(
        "prefunding_b0_checkpoint".to_string(),
        serde_json::json!(true),
    );
    profile.config.insert(
        "prefunding_b0_run_id".to_string(),
        serde_json::json!(checkpoint.run_id),
    );
    Ok(())
}

fn record_seed_fingerprints(profile: &mut ResultProfile, book: &AddressBook) {
    let fingerprints = book
        .addresses
        .iter()
        .map(|(role, material)| (role.clone(), material.wallet_fingerprint_hex.clone()))
        .collect::<std::collections::BTreeMap<_, _>>();
    profile.config.insert(
        "seed_fingerprints".to_string(),
        serde_json::json!(fingerprints),
    );
}

fn validate_s0_funding_evidence(
    config: &Config,
    profile: &ResultProfile,
    evidence_path: &Path,
) -> anyhow::Result<()> {
    let bytes = fs::read(evidence_path)
        .with_context(|| format!("reading S0 evidence {}", evidence_path.display()))?;
    let evidence: S0FundingEvidence = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing S0 evidence {}", evidence_path.display()))?;
    if evidence.schema_version != 2
        || evidence.status != "confirmed"
        || profile.config.get("prefunding_b0_run_id")
            != Some(&serde_json::json!(evidence.b0_run_id))
    {
        bail!("S0 evidence does not continue the imported B0 checkpoint");
    }
    let transaction = evidence
        .transaction
        .as_ref()
        .context("S0 evidence has no confirmed transaction")?;
    if transaction
        .tip_height_at_confirmation
        .saturating_sub(transaction.mined_height)
        < config.benchmark.c_min
    {
        bail!("S0 evidence transaction did not reach C_min");
    }
    for mode in ModeName::ALL {
        let label = mode.as_str();
        if profile.modes[label].address.as_ref() != evidence.addresses.get(label) {
            bail!("S0 evidence address mismatch for {label}");
        }
        let funding = profile
            .funding
            .get(label)
            .with_context(|| format!("funding record missing for {label}"))?;
        if funding.tx_id != transaction.tx_id
            || funding.height != transaction.mined_height
            || funding.birthday != Some(evidence.birthday)
            || funding.birthday_start_height != Some(evidence.birthday_start_height)
            || funding.construction_ms != Some(transaction.construction_ms)
            || funding.broadcast_to_mempool_ms != Some(transaction.broadcast_to_mempool_ms)
            || funding.broadcast_to_confirmed_at_c_min_ms
                != Some(transaction.broadcast_to_confirmed_at_c_min_ms)
            || funding.tip_height_at_broadcast != transaction.tip_height_at_broadcast
            || funding.tip_height_at_confirmation != Some(transaction.tip_height_at_confirmation)
            || funding.shared_funding_fee_microtari != Some(transaction.fee_microtari)
            || funding.funding_fee_attribution.as_deref()
                != Some("external_source_shared_not_deducted_from_mode_balance")
        {
            bail!("funding record does not match measured S0 evidence for {label}");
        }
    }
    Ok(())
}

fn config_with_s0_evidence(config: &Config, evidence_path: &Path) -> anyhow::Result<Config> {
    let evidence: S0FundingEvidence = serde_json::from_slice(
        &fs::read(evidence_path)
            .with_context(|| format!("reading S0 evidence {}", evidence_path.display()))?,
    )?;
    if evidence.schema_version != 2 || evidence.status != "confirmed" {
        bail!("S0 evidence must be a confirmed schema-v2 record");
    }
    if evidence.protocol_fingerprint != config.protocol_fingerprint()? {
        bail!("S0 evidence protocol fingerprint does not match the run configuration");
    }
    let transaction = evidence
        .transaction
        .as_ref()
        .context("S0 evidence has no confirmed transaction")?;
    let record = crate::config::FundingRecord {
        amount: config.benchmark.a_fund.clone(),
        tx_id: transaction.tx_id.clone(),
        height: transaction.mined_height,
        birthday: Some(evidence.birthday),
        birthday_start_height: Some(evidence.birthday_start_height),
        construction_ms: Some(transaction.construction_ms),
        broadcast_to_mempool_ms: Some(transaction.broadcast_to_mempool_ms),
        broadcast_to_confirmed_at_c_min_ms: Some(transaction.broadcast_to_confirmed_at_c_min_ms),
        tip_height_at_broadcast: transaction.tip_height_at_broadcast,
        tip_height_at_confirmation: Some(transaction.tip_height_at_confirmation),
        shared_funding_fee_microtari: Some(transaction.fee_microtari),
        funding_fee_attribution: Some(
            "external_source_shared_not_deducted_from_mode_balance".to_string(),
        ),
    };
    let mut resolved = config.clone();
    resolved.funding.old_wallet = Some(record.clone());
    resolved.funding.new_wallet = Some(record.clone());
    resolved.funding.payment_processor = Some(record);
    resolved.validate()?;
    Ok(resolved)
}

fn validate_prefunding_b0_metrics(profile: &ResultProfile) -> anyhow::Result<()> {
    if !profile.funding.is_empty() {
        bail!("pre-funding B0 checkpoint must not contain funding records");
    }
    let mut shared_target: Option<(u64, &str)> = None;
    for mode in ModeName::ALL {
        let label = mode.as_str();
        let cell = &profile.modes[label].scenarios[ScenarioName::B0.as_str()];
        if cell.status != CellStatus::Ok || cell.repetitions.is_empty() {
            bail!("pre-funding B0 must succeed for {label}");
        }
        for repetition in &cell.repetitions {
            let metrics = repetition
                .metrics
                .as_ref()
                .with_context(|| format!("pre-funding B0 metrics missing for {label}"))?;
            let tip_end = metrics["H_tip_end"]
                .as_u64()
                .with_context(|| format!("pre-funding B0 H_tip_end missing for {label}"))?;
            let target_hash = metrics["H_tip_target_hash"]
                .as_str()
                .with_context(|| format!("pre-funding B0 target hash missing for {label}"))?;
            if let Some(expected) = shared_target {
                if expected != (tip_end, target_hash) {
                    bail!(
                        "pre-funding B0 modes do not share one scan target: expected {}:{}, {label} has {tip_end}:{target_hash}",
                        expected.0,
                        expected.1
                    );
                }
            } else {
                shared_target = Some((tip_end, target_hash));
            }
            if metrics["birthday"] != 0
                || metrics["detected_outputs"] != 0
                || metrics["spendable_outputs"] != 0
                || metrics["available_microtari"] != 0
                || metrics["history_transactions"] != 0
                || metrics["max_height"].as_u64() != Some(tip_end)
                || metrics["scan_reached_tip"] != true
                || metrics["tip_lag_blocks"] != 0
                || metrics["tip_lag_tolerance_blocks"] != 0
                || metrics["H_scan_cursor_hash"].as_str() != Some(target_hash)
                || metrics["H_tip_completion"].as_u64().is_none()
                || metrics["H_tip_completion_hash"].as_str().is_none()
            {
                bail!("pre-funding B0 exact empty-tip verification failed for {label}");
            }
            for metric in [
                "T_scan_ms",
                "blocks_per_sec",
                "H_tip_start",
                "H_tip_end",
                "peak_rss_bytes",
                "peak_cpu_percent",
                "scan_invocations",
            ] {
                if !metrics[metric].is_number() {
                    bail!("pre-funding B0 metric {metric} missing for {label}");
                }
            }
            if repetition.wall_ms != metrics["T_scan_ms"].as_u64().map(u128::from)
                || repetition.fee_microtari != Some(0)
                || repetition.success_count != 1
                || repetition.failure_count != 0
            {
                bail!("pre-funding B0 repetition accounting failed for {label}");
            }
        }
    }
    Ok(())
}

#[derive(Debug)]
struct OutputStatusSummary {
    status: String,
    count: u64,
    value: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputStatusClass {
    Spendable,
    Pending,
    Spent,
    Invalid,
    Unknown,
}

#[derive(Debug, Default)]
struct FundStatusTotals {
    spendable_count: u64,
    spendable_value: u64,
    pending_rows: Vec<String>,
    spent_rows: Vec<String>,
    invalid_rows: Vec<String>,
    unknown_rows: Vec<String>,
}

#[cfg(test)]
fn check_live_funds(
    config: &Config,
    mode1_db: Option<PathBuf>,
    mode2_db: Option<PathBuf>,
    payment_receiver_db: Option<PathBuf>,
) -> anyhow::Result<()> {
    let paths = live_wallet_paths(config, mode1_db, mode2_db, payment_receiver_db);
    check_live_funds_at_paths(config, &paths)
}

#[derive(Debug)]
struct LiveWalletPaths {
    old_wallet: PathBuf,
    new_wallet: PathBuf,
    payment_processor: PathBuf,
}

fn live_wallet_paths(
    config: &Config,
    mode1_db: Option<PathBuf>,
    mode2_db: Option<PathBuf>,
    payment_receiver_db: Option<PathBuf>,
) -> LiveWalletPaths {
    LiveWalletPaths {
        old_wallet: mode1_db.unwrap_or_else(|| {
            config
                .paths
                .data_dir
                .join("old-wallet-console/esmeralda/data/wallet/db/console_wallet.db")
        }),
        new_wallet: mode2_db.unwrap_or_else(|| config.modes.new_wallet_database.clone()),
        payment_processor: payment_receiver_db
            .unwrap_or_else(|| config.paths.data_dir.join("payment-receiver/wallet.db")),
    }
}

fn check_live_funds_at_paths(config: &Config, paths: &LiveWalletPaths) -> anyhow::Result<()> {
    let checks = [
        (
            "old_wallet",
            paths.old_wallet.as_path(),
            1,
            config.a_fund()?.0,
        ),
        (
            "new_wallet",
            paths.new_wallet.as_path(),
            1,
            config.a_fund()?.0,
        ),
        (
            "payment_processor",
            paths.payment_processor.as_path(),
            1,
            config.a_fund()?.0,
        ),
    ];

    let mut errors = Vec::new();
    for (label, db_path, required_unspent, required_value) in checks {
        if !db_path.exists() {
            errors.push(format!(
                "{label}: wallet DB missing at {}; fund/scan this wallet before live run",
                db_path.display()
            ));
            continue;
        }
        let summary = output_status_summary(db_path)
            .with_context(|| format!("reading {label} outputs from {}", db_path.display()))?;
        let totals = fund_status_totals(&summary);
        println!(
            "{label}: db={} spendable_count={} spendable_microtari={} required_spendable_count={required_unspent} statuses={}",
            db_path.display(),
            totals.spendable_count,
            totals.spendable_value,
            summary
                .iter()
                .map(format_status_row)
                .collect::<Vec<_>>()
                .join(",")
        );
        if totals.spendable_count != required_unspent {
            errors.push(format!(
                "{label}: observed {} spendable outputs, require exactly {required_unspent} for the final benchmark starting state",
                totals.spendable_count
            ));
        }
        if totals.spendable_value != required_value {
            errors.push(format!(
                "{label}: observed {} spendable µT, require exactly {required_value} µT for configured A_fund",
                totals.spendable_value
            ));
        }
        if !totals.pending_rows.is_empty() {
            errors.push(format!(
                "{label}: pending/encumbered outputs present ({}); unlock/rescan before final live run",
                totals.pending_rows.join(",")
            ));
        }
        if !totals.spent_rows.is_empty() {
            errors.push(format!(
                "{label}: spent outputs from prior activity are present ({}); use a pristine funded wallet for the final live run",
                totals.spent_rows.join(",")
            ));
        }
        if !totals.invalid_rows.is_empty() {
            errors.push(format!(
                "{label}: invalid/cancelled/not-stored outputs present ({})",
                totals.invalid_rows.join(",")
            ));
        }
        if !totals.unknown_rows.is_empty() {
            errors.push(format!(
                "{label}: unknown output statuses present ({})",
                totals.unknown_rows.join(",")
            ));
        }
    }
    if !errors.is_empty() {
        bail!("fund preflight failed:\n{}", errors.join("\n"));
    }
    Ok(())
}

fn check_live_wallet_identities(
    config: &Config,
    book: &AddressBook,
    paths: &LiveWalletPaths,
) -> anyhow::Result<()> {
    let checks = [
        (
            "new_wallet",
            paths.new_wallet.as_path(),
            WalletRole::NewWallet,
            config.funding.new_wallet.as_ref(),
        ),
        (
            "payment_processor",
            paths.payment_processor.as_path(),
            WalletRole::PaymentProcessor,
            config.funding.payment_processor.as_ref(),
        ),
    ];
    let mut errors = Vec::new();
    for (label, db_path, role, funding) in checks {
        let Some(expected) = book.addresses.get(role.label()) else {
            errors.push(format!("{label}: configured seed material is missing"));
            continue;
        };
        if let Err(error) =
            check_minotari_wallet_identity(db_path, &expected.wallet_fingerprint_hex)
        {
            errors.push(format!("{label}: {error:#}"));
            continue;
        }
        if let Some(funding) = funding
            && let Err(error) = check_local_scan_height(db_path, funding.height)
        {
            errors.push(format!("{label}: {error:#}"));
        }
    }
    if !errors.is_empty() {
        bail!("wallet identity preflight failed:\n{}", errors.join("\n"));
    }
    Ok(())
}

fn check_payment_processor_pristine(config: &Config) -> anyhow::Result<()> {
    let signer = payment_processor::payment_processor_signer_db_path(config);
    if signer.exists() {
        bail!(
            "payment-processor signer state already exists at {}; use a new candidate namespace",
            signer.display()
        );
    }
    let database = payment_processor::payment_processor_db_path(config);
    if !database.exists() {
        return Ok(());
    }
    let connection = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    for table in ["payment_batches", "payments", "events"] {
        if table_exists(&connection, table)? {
            let count: i64 =
                connection.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })?;
            if count != 0 {
                bail!("payment-processor operational table {table} contains {count} stale row(s)");
            }
        }
    }
    Ok(())
}

fn check_minotari_wallet_identity(
    db_path: &Path,
    expected_fingerprint_hex: &str,
) -> anyhow::Result<()> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let fingerprint = conn
        .query_row(
            "SELECT fingerprint FROM accounts WHERE friendly_name = 'default'",
            [],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?
        .context("default account is missing")?;
    let expected =
        hex::decode(expected_fingerprint_hex).context("decoding expected fingerprint")?;
    if fingerprint != expected {
        bail!("default account fingerprint does not match the configured seed");
    }
    Ok(())
}

fn check_local_scan_height(db_path: &Path, funding_height: u64) -> anyhow::Result<()> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let scanned_height = conn
        .query_row(
            "SELECT max(scanned_tip_blocks.height) \
             FROM scanned_tip_blocks \
             JOIN accounts ON accounts.id = scanned_tip_blocks.account_id \
             WHERE accounts.friendly_name = 'default'",
            [],
            |row| row.get::<_, Option<i64>>(0),
        )?
        .unwrap_or(0)
        .max(0) as u64;
    if scanned_height < funding_height {
        bail!(
            "scanner height {scanned_height} is behind funding height {funding_height}; scan to the selected chain tip before running"
        );
    }
    Ok(())
}

const MIN_FREE_DISK_BYTES: u64 = 20 * 1024 * 1024 * 1024;

#[derive(Debug)]
struct ChainTip {
    height: u64,
    hash: Vec<u8>,
    pruning_horizon: u64,
    is_synced: bool,
}

#[derive(Debug)]
struct FundingOutputProof {
    output_hash: Vec<u8>,
    block_hash: Vec<u8>,
    height: u64,
}

#[cfg(feature = "live-minotari")]
async fn check_endpoint_authority(config: &Config) -> anyhow::Result<()> {
    check_local_node_process_identity(config)?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let selected = fetch_chain_tip(&client, &config.network.base_node_http_url).await?;
    let authority = fetch_chain_tip(&client, &config.network.authority_http_url).await?;
    if !selected.is_synced || !authority.is_synced {
        bail!("selected and authority endpoints must both report is_synced=true");
    }
    if selected.pruning_horizon != 0 {
        bail!("selected scan endpoint must be archival (pruning_horizon=0)");
    }
    if selected.height.abs_diff(authority.height) > config.benchmark.c_min {
        bail!("selected scan endpoint is stale relative to the authority endpoint");
    }
    let finalized_height = selected
        .height
        .min(authority.height)
        .saturating_sub(config.benchmark.c_min);
    let selected_hash = fetch_header_hash(
        &client,
        &config.network.base_node_http_url,
        finalized_height,
    )
    .await?;
    let authority_hash = fetch_header_hash(
        &client,
        &config.network.authority_http_url,
        finalized_height,
    )
    .await?;
    if selected_hash != authority_hash {
        bail!("selected and authority endpoints disagree at finalized height {finalized_height}");
    }
    Ok(())
}

#[cfg(feature = "live-minotari")]
fn check_local_node_process_identity(config: &Config) -> anyhow::Result<()> {
    let endpoint = url::Url::parse(&config.network.base_node_http_url)?;
    let port = endpoint
        .port_or_known_default()
        .context("local base-node endpoint has no port")?;
    let output = std::process::Command::new("lsof")
        .args(["-nP", "-t", &format!("-iTCP:{port}"), "-sTCP:LISTEN"])
        .output()
        .context("querying local base-node listener with lsof")?;
    if !output.status.success() {
        bail!("no process is listening on the configured local base-node port {port}");
    }
    let pid = String::from_utf8(output.stdout)?
        .lines()
        .next()
        .context("lsof returned no local base-node listener PID")?
        .parse::<u32>()?;
    let mut system = System::new_all();
    system.refresh_all();
    let executable = system
        .process(Pid::from_u32(pid))
        .and_then(|process| process.exe())
        .context("could not resolve the local base-node listener executable")?;
    if sha256_file(executable)? != sha256_file(&config.paths.minotari_node)? {
        bail!(
            "process listening on local base-node port {port} is not the build-manifest minotari_node"
        );
    }
    Ok(())
}

async fn check_selected_chain_readiness(
    config: &Config,
    paths: &LiveWalletPaths,
) -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("building strict-preflight HTTP client")?;
    let tip = fetch_chain_tip(&client, &config.network.base_node_http_url).await?;
    if !tip.is_synced {
        bail!("selected base node reports is_synced=false");
    }
    if tip.pruning_horizon != 0 {
        bail!(
            "selected base node is pruned (pruning_horizon={}); the submission run requires an archival endpoint",
            tip.pruning_horizon
        );
    }
    if tip.hash.len() != 32 {
        bail!(
            "selected base-node tip hash has {} bytes, expected 32",
            tip.hash.len()
        );
    }
    let authority_tip = fetch_chain_tip(&client, &config.network.authority_http_url)
        .await
        .context("querying independent Esmeralda authority tip")?;
    if !authority_tip.is_synced {
        bail!("independent Esmeralda authority reports is_synced=false");
    }
    if tip.height.abs_diff(authority_tip.height) > config.benchmark.c_min {
        bail!(
            "selected base-node tip {} differs from authority tip {} by more than C_min={}",
            tip.height,
            authority_tip.height,
            config.benchmark.c_min
        );
    }
    let finalized_height = tip
        .height
        .min(authority_tip.height)
        .saturating_sub(config.benchmark.c_min);
    let selected_finalized_hash = fetch_header_hash(
        &client,
        &config.network.base_node_http_url,
        finalized_height,
    )
    .await
    .context("querying selected base-node finalized header")?;
    let authority_finalized_hash = fetch_header_hash(
        &client,
        &config.network.authority_http_url,
        finalized_height,
    )
    .await
    .context("querying authority finalized header")?;
    if selected_finalized_hash != authority_finalized_hash {
        bail!("selected base node and authority disagree at finalized height {finalized_height}");
    }

    let checks = [
        (
            "old_wallet",
            paths.old_wallet.as_path(),
            config.funding.old_wallet.as_ref(),
        ),
        (
            "new_wallet",
            paths.new_wallet.as_path(),
            config.funding.new_wallet.as_ref(),
        ),
        (
            "payment_processor",
            paths.payment_processor.as_path(),
            config.funding.payment_processor.as_ref(),
        ),
    ];
    let mut errors = Vec::new();
    for (label, db_path, funding) in checks {
        let Some(funding) = funding else {
            errors.push(format!("{label}: funding record is missing"));
            continue;
        };
        let proof = match read_funding_output_proof(db_path) {
            Ok(proof) => proof,
            Err(error) => {
                errors.push(format!("{label}: {error:#}"));
                continue;
            }
        };
        if proof.height != funding.height {
            errors.push(format!(
                "{label}: wallet funding output height {} does not match configured height {}",
                proof.height, funding.height
            ));
            continue;
        }
        if tip.height < funding.height.saturating_add(config.benchmark.c_min) {
            errors.push(format!(
                "{label}: funding output is not C_min={} deep at tip {}",
                config.benchmark.c_min, tip.height
            ));
            continue;
        }
        match fetch_header_hash(&client, &config.network.base_node_http_url, funding.height).await {
            Ok(header_hash) if header_hash == proof.block_hash => {}
            Ok(header_hash) => {
                errors.push(format!(
                    "{label}: wallet funding block {} is not on the selected chain (wallet={}, chain={})",
                    funding.height,
                    hex::encode(proof.block_hash),
                    hex::encode(header_hash)
                ));
                continue;
            }
            Err(error) => {
                errors.push(format!("{label}: header proof failed: {error:#}"));
                continue;
            }
        }
        if let Err(error) = prove_output_unspent(
            &client,
            &config.network.base_node_http_url,
            &proof.block_hash,
            &proof.output_hash,
        )
        .await
        {
            errors.push(format!("{label}: {error:#}"));
        }
        match read_scanned_height(db_path) {
            Ok(scanned_height)
                if scanned_height.saturating_add(config.benchmark.c_min) >= tip.height => {}
            Ok(scanned_height) => errors.push(format!(
                "{label}: scanner height {scanned_height} is stale relative to selected-chain tip {} (allowed lag C_min={})",
                tip.height,
                config.benchmark.c_min
            )),
            Err(error) => errors.push(format!("{label}: scanner-height proof failed: {error:#}")),
        }
    }
    if !errors.is_empty() {
        bail!("selected-chain preflight failed:\n{}", errors.join("\n"));
    }
    println!(
        "selected-chain proof PASS: tip={} hash={} pruning_horizon={} authority_tip={} finalized_height={} finalized_hash={} is_synced=true",
        tip.height,
        hex::encode(tip.hash),
        tip.pruning_horizon,
        authority_tip.height,
        finalized_height,
        hex::encode(selected_finalized_hash)
    );
    Ok(())
}

async fn fetch_chain_tip(client: &reqwest::Client, base_url: &str) -> anyhow::Result<ChainTip> {
    let url = url::Url::parse(base_url)?.join("/get_tip_info")?;
    let value: serde_json::Value = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(ChainTip {
        height: value
            .pointer("/metadata/best_block_height")
            .and_then(serde_json::Value::as_u64)
            .context("tip response is missing best_block_height")?,
        hash: json_byte_array(&value["metadata"]["best_block_hash"])
            .context("tip response is missing best_block_hash")?,
        pruning_horizon: value
            .pointer("/metadata/pruning_horizon")
            .and_then(serde_json::Value::as_u64)
            .context("tip response is missing pruning_horizon")?,
        is_synced: value["is_synced"]
            .as_bool()
            .context("tip response is missing is_synced")?,
    })
}

async fn fetch_header_hash(
    client: &reqwest::Client,
    base_url: &str,
    height: u64,
) -> anyhow::Result<Vec<u8>> {
    let mut url = url::Url::parse(base_url)?.join("/get_header_by_height")?;
    url.query_pairs_mut()
        .append_pair("height", &height.to_string());
    let value: serde_json::Value = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    json_byte_array(&value["hash"]).context("header response is missing hash")
}

async fn prove_output_unspent(
    client: &reqwest::Client,
    base_url: &str,
    block_hash: &[u8],
    output_hash: &[u8],
) -> anyhow::Result<()> {
    let mut url = url::Url::parse(base_url)?.join("/sync_utxos_by_block")?;
    url.query_pairs_mut()
        .append_pair("start_header_hash", &hex::encode(block_hash))
        .append_pair("limit", "1")
        .append_pair("page", "0")
        .append_pair("exclude_spent", "true")
        .append_pair("exclude_inputs", "false")
        .append_pair("version", "1");
    let value: serde_json::Value = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let found = value["blocks"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|block| block["outputs"].as_array().into_iter().flatten())
        .filter_map(|output| output["output_hash"].as_str())
        .filter_map(|encoded| BASE64_STANDARD.decode(encoded).ok())
        .any(|hash| hash == output_hash);
    if !found {
        bail!(
            "funding output {} is absent from the selected chain's unspent set at its mining block",
            hex::encode(output_hash)
        );
    }
    Ok(())
}

fn read_funding_output_proof(db_path: &Path) -> anyhow::Result<FundingOutputProof> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let columns = table_columns(&conn, "outputs")?;
    let (output_hash, block_hash, height) = if columns.contains("output_hash") {
        (
            "output_hash",
            "mined_in_block_hash",
            "mined_in_block_height",
        )
    } else {
        ("hash", "mined_in_block", "mined_height")
    };
    let active_filter = active_output_filter(&conn)?;
    let conjunction = if active_filter.is_empty() {
        "WHERE"
    } else {
        "AND"
    };
    let sql = format!(
        "SELECT {output_hash}, {block_hash}, {height} FROM outputs {active_filter} {conjunction} upper(CAST(status AS TEXT)) IN ('UNSPENT', '0')"
    );
    conn.query_row(&sql, [], |row| {
        Ok(FundingOutputProof {
            output_hash: row.get(0)?,
            block_hash: row.get(1)?,
            height: row.get::<_, i64>(2)?.max(0) as u64,
        })
    })
    .context("reading the single spendable funding output")
}

fn read_scanned_height(db_path: &Path) -> anyhow::Result<u64> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let table = if table_exists(&conn, "scanned_tip_blocks")? {
        "scanned_tip_blocks"
    } else if table_exists(&conn, "scanned_blocks")? {
        "scanned_blocks"
    } else {
        bail!("wallet DB has no supported scanner-height table");
    };
    Ok(conn
        .query_row(&format!("SELECT max(height) FROM {table}"), [], |row| {
            row.get::<_, Option<i64>>(0)
        })?
        .unwrap_or(0)
        .max(0) as u64)
}

fn table_columns(
    conn: &Connection,
    table: &str,
) -> anyhow::Result<std::collections::BTreeSet<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    Ok(stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<_, _>>()?)
}

fn table_exists(conn: &Connection, table: &str) -> anyhow::Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn json_byte_array(value: &serde_json::Value) -> Option<Vec<u8>> {
    value
        .as_array()?
        .iter()
        .map(|byte| byte.as_u64().and_then(|byte| u8::try_from(byte).ok()))
        .collect()
}

fn check_disk_space(config: &Config) -> anyhow::Result<()> {
    let disks = Disks::new_with_refreshed_list();
    let disk = disks
        .list()
        .iter()
        .filter(|disk| config.paths.data_dir.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().as_os_str().len())
        .context("could not determine the benchmark data disk")?;
    if disk.available_space() < MIN_FREE_DISK_BYTES {
        bail!(
            "only {} bytes are free on {}; require at least {} bytes",
            disk.available_space(),
            disk.mount_point().display(),
            MIN_FREE_DISK_BYTES
        );
    }
    println!(
        "disk preflight PASS: {} bytes free on {}",
        disk.available_space(),
        disk.mount_point().display()
    );
    Ok(())
}

fn ensure_runtime_paths_are_absolute(config: &Config) -> anyhow::Result<()> {
    let paths = [
        ("paths.data_dir", config.paths.data_dir.as_path()),
        ("paths.cache_dir", config.paths.cache_dir.as_path()),
        (
            "paths.minotari_console_wallet",
            config.paths.minotari_console_wallet.as_path(),
        ),
        (
            "paths.minotari_binary",
            config.paths.minotari_binary.as_path(),
        ),
        ("paths.minotari_node", config.paths.minotari_node.as_path()),
        (
            "paths.payment_processor_binary",
            config.paths.payment_processor_binary.as_path(),
        ),
        (
            "paths.build_manifest",
            config.paths.build_manifest.as_path(),
        ),
        (
            "modes.new_wallet_database",
            config.modes.new_wallet_database.as_path(),
        ),
    ];
    let relative = paths
        .into_iter()
        .filter_map(|(label, path)| (!path.is_absolute()).then_some(label))
        .collect::<Vec<_>>();
    if !relative.is_empty() {
        bail!(
            "runtime paths must be absolute before launching subprocesses: {}",
            relative.join(", ")
        );
    }
    Ok(())
}

fn check_listen_ports_available(config: &Config) -> anyhow::Result<()> {
    let mut addresses = Vec::new();
    if config.benchmark.mode1_live_topology {
        let url = url::Url::parse(&config.modes.old_wallet_grpc_address)
            .context("parsing modes.old_wallet_grpc_address")?;
        let host = url
            .host_str()
            .context("modes.old_wallet_grpc_address has no host")?;
        let port = url
            .port_or_known_default()
            .context("modes.old_wallet_grpc_address has no port")?;
        addresses.push(format!("{host}:{port}"));
    }
    if config.benchmark.mode3_live_topology {
        addresses.push(config.modes.payment_processor_listen.clone());
        addresses.push(config.modes.payment_receiver_listen.clone());
    }
    let mut listeners = Vec::new();
    for address in addresses {
        let listener = std::net::TcpListener::bind(&address)
            .with_context(|| format!("required listen address {address} is unavailable"))?;
        listeners.push(listener);
    }
    Ok(())
}

fn output_status_summary(db_path: &Path) -> anyhow::Result<Vec<OutputStatusSummary>> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let active_filter = active_output_filter(&conn)?;
    let sql = format!(
        "SELECT CAST(status AS TEXT), count(*), coalesce(sum(value), 0) FROM outputs {active_filter} GROUP BY status ORDER BY CAST(status AS TEXT)"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(OutputStatusSummary {
                status: row.get(0)?,
                count: row.get::<_, i64>(1)?.max(0) as u64,
                value: row.get::<_, i64>(2)?.max(0) as u64,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn active_output_filter(conn: &Connection) -> anyhow::Result<&'static str> {
    let mut stmt = conn.prepare("PRAGMA table_info(outputs)")?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if columns.iter().any(|column| column == "deleted_at") {
        Ok("WHERE deleted_at IS NULL")
    } else if columns
        .iter()
        .any(|column| column == "marked_deleted_at_height")
    {
        Ok("WHERE marked_deleted_at_height IS NULL")
    } else {
        Ok("")
    }
}

fn fund_status_totals(summary: &[OutputStatusSummary]) -> FundStatusTotals {
    let mut totals = FundStatusTotals::default();
    for row in summary {
        match classify_output_status(&row.status) {
            OutputStatusClass::Spendable => {
                totals.spendable_count = totals.spendable_count.saturating_add(row.count);
                totals.spendable_value = totals.spendable_value.saturating_add(row.value);
            }
            OutputStatusClass::Pending => totals.pending_rows.push(format_status_row(row)),
            OutputStatusClass::Spent => totals.spent_rows.push(format_status_row(row)),
            OutputStatusClass::Invalid => totals.invalid_rows.push(format_status_row(row)),
            OutputStatusClass::Unknown => totals.unknown_rows.push(format_status_row(row)),
        }
    }
    totals
}

fn format_status_row(row: &OutputStatusSummary) -> String {
    format!(
        "{}({}):{}:{}",
        row.status,
        output_status_label(&row.status),
        row.count,
        row.value
    )
}

fn classify_output_status(status: &str) -> OutputStatusClass {
    match normalized_output_status(status).as_str() {
        "UNSPENT" | "0" => OutputStatusClass::Spendable,
        "LOCKED" | "2" | "3" | "6" | "7" | "8" => OutputStatusClass::Pending,
        "SPENT" | "1" | "9" => OutputStatusClass::Spent,
        "4" | "5" | "10" => OutputStatusClass::Invalid,
        _ => OutputStatusClass::Unknown,
    }
}

fn output_status_label(status: &str) -> &'static str {
    match normalized_output_status(status).as_str() {
        "UNSPENT" => "Unspent",
        "LOCKED" => "Locked",
        "SPENT" => "Spent",
        "0" => "Unspent",
        "1" => "Spent",
        "2" => "EncumberedToBeReceived",
        "3" => "EncumberedToBeSpent",
        "4" => "Invalid",
        "5" => "CancelledInbound",
        "6" => "UnspentMinedUnconfirmed",
        "7" => "ShortTermEncumberedToBeReceived",
        "8" => "ShortTermEncumberedToBeSpent",
        "9" => "SpentMinedUnconfirmed",
        "10" => "NotStored",
        _ => "Unknown",
    }
}

fn normalized_output_status(status: &str) -> String {
    status.trim().to_ascii_uppercase()
}

fn require_env(name: &str) -> anyhow::Result<String> {
    env::var(name).with_context(|| format!("${name} must be set"))
}

fn check_binary(path: &Path, label: &str) -> anyhow::Result<()> {
    let metadata = path
        .metadata()
        .with_context(|| format!("{label} binary not found at {}", path.display()))?;
    if !metadata.is_file() {
        bail!("{label} binary path is not a file: {}", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            bail!("{label} binary is not executable: {}", path.display());
        }
    }
    if !path.is_absolute() {
        bail!("{label} binary path must be absolute: {}", path.display());
    }
    Ok(())
}

#[cfg(feature = "live-minotari")]
fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let bytes =
        fs::read(path).with_context(|| format!("reading {} for SHA-256", path.display()))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn check_harness_worktree_clean() -> anyhow::Result<()> {
    let output = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .context("checking harness git worktree")?;
    if !output.status.success() {
        bail!("could not verify the harness git worktree state");
    }
    if !output.stdout.is_empty() {
        bail!("canonical candidate commands require a clean git worktree");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_fund_summary_classifies_minotari_text_statuses() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("wallet.db");
        write_outputs_db(
            &db_path,
            "TEXT",
            &[
                ("UNSPENT", 1, 1_000_000),
                ("LOCKED", 2, 20_000_000),
                ("SPENT", 3, 30_000_000),
            ],
        );

        let summary = output_status_summary(&db_path).unwrap();
        let totals = fund_status_totals(&summary);

        assert_eq!(totals.spendable_count, 1);
        assert_eq!(totals.spendable_value, 1_000_000);
        assert_eq!(totals.pending_rows, vec!["LOCKED(Locked):2:40000000"]);
        assert_eq!(totals.spent_rows, vec!["SPENT(Spent):3:90000000"]);
        assert!(totals.invalid_rows.is_empty());
        assert!(totals.unknown_rows.is_empty());
    }

    #[test]
    fn sqlite_fund_summary_classifies_console_numeric_statuses() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("console_wallet.db");
        write_outputs_db(
            &db_path,
            "INTEGER",
            &[
                ("0", 1, 1_000_000),
                ("1", 1, 1_000_000),
                ("2", 1, 1_000_000),
                ("3", 1, 1_000_000),
                ("4", 1, 1_000_000),
                ("5", 1, 1_000_000),
                ("6", 1, 1_000_000),
                ("7", 1, 1_000_000),
                ("8", 1, 1_000_000),
                ("9", 1, 1_000_000),
                ("10", 1, 1_000_000),
            ],
        );

        let summary = output_status_summary(&db_path).unwrap();
        let totals = fund_status_totals(&summary);

        assert_eq!(totals.spendable_count, 1);
        assert_eq!(totals.spendable_value, 1_000_000);
        assert_eq!(
            totals.pending_rows,
            vec![
                "2(EncumberedToBeReceived):1:1000000",
                "3(EncumberedToBeSpent):1:1000000",
                "6(UnspentMinedUnconfirmed):1:1000000",
                "7(ShortTermEncumberedToBeReceived):1:1000000",
                "8(ShortTermEncumberedToBeSpent):1:1000000"
            ]
        );
        assert_eq!(
            totals.invalid_rows,
            vec![
                "10(NotStored):1:1000000",
                "4(Invalid):1:1000000",
                "5(CancelledInbound):1:1000000"
            ]
        );
        assert!(totals.unknown_rows.is_empty());
    }

    #[test]
    fn sqlite_fund_summary_ignores_deleted_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let minotari_db = dir.path().join("minotari-wallet.db");
        let conn = Connection::open(&minotari_db).unwrap();
        conn.execute(
            "CREATE TABLE outputs (status TEXT, value INTEGER, deleted_at INTEGER)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO outputs (status, value, deleted_at) VALUES ('UNSPENT', 100, NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO outputs (status, value, deleted_at) VALUES ('LOCKED', 200, 123)",
            [],
        )
        .unwrap();
        drop(conn);

        let console_db = dir.path().join("console-wallet.db");
        let conn = Connection::open(&console_db).unwrap();
        conn.execute(
            "CREATE TABLE outputs (status INTEGER, value INTEGER, marked_deleted_at_height INTEGER)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO outputs (status, value, marked_deleted_at_height) VALUES (0, 300, NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO outputs (status, value, marked_deleted_at_height) VALUES (2, 400, 456)",
            [],
        )
        .unwrap();
        drop(conn);

        let minotari_totals = fund_status_totals(&output_status_summary(&minotari_db).unwrap());
        assert_eq!(minotari_totals.spendable_count, 1);
        assert_eq!(minotari_totals.spendable_value, 100);
        assert!(minotari_totals.pending_rows.is_empty());

        let console_totals = fund_status_totals(&output_status_summary(&console_db).unwrap());
        assert_eq!(console_totals.spendable_count, 1);
        assert_eq!(console_totals.spendable_value, 300);
        assert!(console_totals.pending_rows.is_empty());
    }

    #[test]
    fn fund_preflight_rejects_non_exact_final_starting_value() {
        let dir = tempfile::tempdir().unwrap();
        let old_db = dir.path().join("old-wallet.db");
        let new_db = dir.path().join("new-wallet.db");
        let pp_db = dir.path().join("payment-receiver.db");
        write_outputs_db(&old_db, "TEXT", &[("UNSPENT", 1, 1_000_000)]);
        write_outputs_db(&new_db, "TEXT", &[("UNSPENT", 1, 10_000_000)]);
        write_outputs_db(&pp_db, "TEXT", &[("UNSPENT", 1, 9_000_000)]);

        let mut config = Config::default();
        config.benchmark.a_fund = "10 T".to_string();

        let error = check_live_funds(&config, Some(old_db), Some(new_db), Some(pp_db))
            .expect_err("non-exact spendable value must fail fund preflight");
        assert!(
            format!("{error:#}")
                .contains("old_wallet: observed 1000000 spendable µT, require exactly 10000000 µT")
        );
        assert!(format!("{error:#}").contains(
            "payment_processor: observed 9000000 spendable µT, require exactly 10000000 µT"
        ));
    }

    #[test]
    fn fund_status_classifier_reports_unknown_statuses() {
        let row = OutputStatusSummary {
            status: "MYSTERY".to_string(),
            count: 1,
            value: 42,
        };
        let totals = fund_status_totals(&[row]);

        assert_eq!(totals.spendable_count, 0);
        assert_eq!(totals.unknown_rows, vec!["MYSTERY(Unknown):1:42"]);
    }

    #[test]
    fn minotari_identity_mismatch_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("wallet.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "CREATE TABLE accounts (friendly_name TEXT NOT NULL, fingerprint BLOB NOT NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO accounts (friendly_name, fingerprint) VALUES ('default', x'010203')",
            [],
        )
        .unwrap();
        drop(conn);

        let error = check_minotari_wallet_identity(&db_path, "aabbcc")
            .expect_err("mismatched seed identity must fail")
            .to_string();
        assert!(error.contains("does not match"));
    }

    #[test]
    fn payment_processor_preflight_rejects_stale_operational_rows() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.paths.data_dir = dir.path().to_path_buf();
        let database = payment_processor::payment_processor_db_path(&config);
        fs::create_dir_all(database.parent().unwrap()).unwrap();
        let connection = Connection::open(database).unwrap();
        connection
            .execute("CREATE TABLE payment_batches (id TEXT)", [])
            .unwrap();
        connection
            .execute("INSERT INTO payment_batches (id) VALUES ('stale')", [])
            .unwrap();
        drop(connection);

        let error = check_payment_processor_pristine(&config)
            .unwrap_err()
            .to_string();
        assert!(error.contains("stale row"));
    }

    #[tokio::test]
    async fn run_profile_invokes_strict_preflight_before_writing() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.seeds.old_wallet_env = "WALLET_BENCH_RUN_GATE_OLD".to_string();
        config.seeds.new_wallet_env = "WALLET_BENCH_RUN_GATE_NEW".to_string();
        config.seeds.payment_processor_env = "WALLET_BENCH_RUN_GATE_PP".to_string();
        config.seeds.wallet_password_env = "WALLET_BENCH_RUN_GATE_PASSWORD".to_string();
        let generated = AddressBook::generate_fresh(&config).unwrap();
        for material in generated.addresses.values() {
            // SAFETY: these test-specific names are not read by other code.
            unsafe { env::set_var(&material.env_var, &material.seed_words) };
        }
        // SAFETY: this test-specific name is not read by other code.
        unsafe { env::set_var(&config.seeds.wallet_password_env, "test-only") };

        let profile = dir.path().join("must-not-exist.json");
        let error = run_profile(
            &config,
            &profile,
            Path::new("missing-b0.json"),
            Path::new("missing-s0.json"),
        )
        .await
        .expect_err("relative runtime paths must fail the strict run gate");

        for material in generated.addresses.values() {
            // SAFETY: restore the test-specific process environment immediately.
            unsafe { env::remove_var(&material.env_var) };
        }
        // SAFETY: restore the test-specific process environment immediately.
        unsafe { env::remove_var(&config.seeds.wallet_password_env) };
        assert!(format!("{error:#}").contains("runtime paths must be absolute"));
        assert!(!profile.exists());
    }

    fn write_outputs_db(db_path: &Path, status_column_type: &str, rows: &[(&str, u64, u64)]) {
        let conn = Connection::open(db_path).unwrap();
        conn.execute(
            &format!("CREATE TABLE outputs (status {status_column_type}, value INTEGER)"),
            [],
        )
        .unwrap();
        for (status, count, value) in rows {
            for _ in 0..*count {
                conn.execute(
                    "INSERT INTO outputs (status, value) VALUES (?1, ?2)",
                    (*status, *value as i64),
                )
                .unwrap();
            }
        }
    }
}
