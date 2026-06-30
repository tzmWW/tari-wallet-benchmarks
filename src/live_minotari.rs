#![cfg(feature = "live-minotari")]

use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::Stdio,
    str::FromStr,
    time::{Duration, Instant},
};

use anyhow::{Context, bail};
use minotari::{
    ScanMode, Scanner,
    db::{self, SqlitePool, get_latest_scanned_tip_block_by_account},
    get_accounts, get_balance, init_db,
    models::PendingTransactionStatus,
    transactions::{
        fund_locker::FundLocker,
        manager::TransactionSender,
        one_sided_transaction::{OneSidedTransaction, Recipient},
    },
    utils::init_wallet::{init_with_seed_words, init_with_view_key},
};
use minotari_wallet_grpc_client::{WalletGrpcClient, grpc};
use rusqlite::Connection;
use tari_common::configuration::Network;
use tari_common_types::seeds::cipher_seed::CipherSeed;
use tari_common_types::tari_address::TariAddress;
use tari_transaction_components::{
    MicroMinotari,
    consensus::ConsensusConstantsBuilder,
    offline_signing::{models::SignedOneSidedTransactionResult, sign_locked_transaction},
    rpc::models::{TxLocation, TxQueryResponse, TxSubmissionRejectionReason, TxSubmissionResponse},
    transaction_components::Transaction,
};
use tari_utilities::ByteArray;
use tokio::{process::Command, task::JoinSet, time};

use crate::{
    config::{Config, parse_amount},
    modes::ScenarioName,
    payment_processor::{
        self, BulkPaymentItem, BulkPaymentRequest, PaymentProcessorClient,
        PaymentProcessorDbSnapshot,
    },
    result_profile::{CellStatus, Repetition, ResultProfile, ScenarioCell, VerifiedTransaction},
    seeds::{
        AddressBook, WalletRole, current_birthday, derive_distinct_recipient_pool, seed_from_words,
        seed_from_words_with_birthday, seed_words_with_birthday,
    },
    versions::TX_MINED_CONFIRMED_STATUS,
};

const MODE1_DB_LOCK_RETRY_ATTEMPTS: u32 = 8;

pub async fn annotate_profile_with_library_smoke(
    config: &Config,
    book: &AddressBook,
    profile: &mut ResultProfile,
    partial_profile_path: Option<&Path>,
) -> anyhow::Result<()> {
    let Some(seed) = book.addresses.get(WalletRole::NewWallet.label()) else {
        return Ok(());
    };
    let db_path = &config.modes.new_wallet_database;
    ensure_signing_wallet(db_path, &seed.seed_words, &config.seeds.wallet_password_env)?;

    let scan = scan_to_tip(
        db_path,
        &wallet_password(&config.seeds.wallet_password_env)?,
        &config.network.base_node_http_url,
        config.benchmark.scan_batch_size,
        config.benchmark.c_min,
    )
    .await
    .and_then(|wall_ms| {
        let balance = account_balance(db_path)?;
        let available = amount_field_as_microtari(&balance, "available")
            .with_context(|| format!("available balance missing from {balance}"))?;
        let expected = config.a_fund()?.0;
        if available < expected {
            anyhow::bail!(
                "available balance {available} µT is below configured A_fund {expected} µT; balance={balance}"
            );
        }
        Ok((wall_ms, available, balance))
    });

    if let Some(mode) = profile.modes.get_mut("new_wallet")
        && let Some(cell) = mode.scenarios.get_mut("S0")
    {
        match scan {
            Ok((wall_ms, available, balance)) => {
                cell.record_repetition(Repetition {
                    run: 1,
                    status: CellStatus::Ok,
                    wall_ms: Some(wall_ms),
                    success_count: 1,
                    failure_count: 0,
                    fee_microtari: None,
                    error: None,
                    metrics: None,
                });
                cell.notes.push(format!(
                    "live-minotari funded scan smoke detected available_microtari={available}; balance={balance}"
                ));
                if let Some(funding) = &config.funding.new_wallet {
                    cell.notes.push(format!(
                        "funding tx_id={} height={} amount={}",
                        funding.tx_id, funding.height, funding.amount
                    ));
                }
            }
            Err(error) => {
                cell.record_repetition(Repetition {
                    run: 1,
                    status: CellStatus::Failed,
                    wall_ms: None,
                    success_count: 0,
                    failure_count: 1,
                    fee_microtari: None,
                    error: Some(format!("{error:#}")),
                    metrics: None,
                });
            }
        }
    }

    if config.benchmark.mode1_live_topology {
        annotate_mode1_console_wallet(config, book, profile).await?;
    } else {
        annotate_mode1_disabled(profile);
    }
    write_profile_checkpoint(profile, partial_profile_path, "old_wallet")?;

    if config.benchmark.mode2_live_scenarios {
        annotate_mode2_live_scenarios(config, book, profile).await?;
    } else if config.benchmark.mode2_send_smoke {
        annotate_mode2_send_smoke(config, book, profile).await?;
    } else if let Some(mode) = profile.modes.get_mut("new_wallet")
        && let Some(cell) = mode.scenarios.get_mut("S1")
    {
        cell.notes.push(
            "Mode 2 send smoke disabled; set benchmark.mode2_send_smoke=true to run the opt-in tiny spend"
                .to_string(),
        );
        if let Some(cell) = mode.scenarios.get_mut("S4") {
            cell.notes.push(
                "Mode 2 live scenarios disabled; set benchmark.mode2_live_scenarios=true to run concurrent send batches"
                    .to_string(),
            );
        }
        if let Some(cell) = mode.scenarios.get_mut("S5") {
            cell.notes.push(
                "Mode 2 live scenarios disabled; set benchmark.mode2_live_scenarios=true to run the individual-send arm"
                    .to_string(),
            );
        }
    }
    write_profile_checkpoint(profile, partial_profile_path, "new_wallet")?;

    if config.benchmark.live_fresh_scan_cells {
        annotate_fresh_scan_b0_cells(config, book, profile).await?;
    } else {
        annotate_fresh_scan_cells_disabled(profile);
    }
    write_profile_checkpoint(profile, partial_profile_path, "fresh_scans")?;

    if config.benchmark.mode3_live_topology {
        annotate_mode3_payment_processor(config, book, profile).await?;
    } else {
        annotate_mode3_disabled(profile);
    }
    write_profile_checkpoint(profile, partial_profile_path, "payment_processor")?;
    Ok(())
}

fn write_profile_checkpoint(
    profile: &ResultProfile,
    profile_path: Option<&Path>,
    label: &str,
) -> anyhow::Result<()> {
    let Some(profile_path) = profile_path else {
        return Ok(());
    };
    let parent = profile_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = profile_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("profile");
    let ext = profile_path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("json");
    let checkpoint_path = parent.join(format!("{stem}.{label}.{ext}"));
    profile.write_atomic(&checkpoint_path)?;
    println!("wrote checkpoint {}", checkpoint_path.display());
    Ok(())
}

pub fn ensure_signing_wallet(
    db_path: &Path,
    seed_words: &str,
    password_env: &str,
) -> anyhow::Result<()> {
    if db_path.exists() {
        return Ok(());
    }
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let seed = seed_from_words(seed_words)?;
    init_with_seed_words(
        seed,
        &wallet_password(password_env)?,
        db_path,
        Some("default"),
    )
    .context("initializing minotari signing wallet")
}

pub async fn scan_to_tip(
    db_path: &Path,
    password: &str,
    base_url: &str,
    batch_size: u64,
    required_confirmations: u64,
) -> anyhow::Result<u128> {
    let start = std::time::Instant::now();
    Scanner::new(
        password,
        base_url,
        db_path.to_path_buf(),
        batch_size,
        required_confirmations,
    )
    .account("default")
    .mode(ScanMode::Full)
    .run()
    .await
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(start.elapsed().as_millis())
}

async fn annotate_fresh_scan_b0_cells(
    config: &Config,
    book: &AddressBook,
    profile: &mut ResultProfile,
) -> anyhow::Result<()> {
    let spec = FreshScanSpec {
        scenario: ScenarioName::B0,
        wallet_state: FreshScanWalletState::EmptyGenesis,
        checkpoint: ScanCheckpoint::Empty,
    };

    if let Some(old_wallet) = book.addresses.get(WalletRole::OldWallet.label()) {
        run_mode1_fresh_scan_for_cell(config, profile, old_wallet, spec).await?;
    }

    if let Some(new_wallet) = book.addresses.get(WalletRole::NewWallet.label()) {
        run_library_fresh_scan_for_cell(
            config,
            profile,
            "new_wallet",
            Some(&new_wallet.seed_words),
            spec,
        )
        .await?;
    }

    if let Some(pp_wallet) = book.addresses.get(WalletRole::PaymentProcessor.label()) {
        run_library_fresh_scan_for_cell(
            config,
            profile,
            "payment_processor",
            Some(&pp_wallet.seed_words),
            spec,
        )
        .await?;
    }

    Ok(())
}

fn annotate_fresh_scan_cells_disabled(profile: &mut ResultProfile) {
    for scenario in [
        ScenarioName::B0,
        ScenarioName::S2,
        ScenarioName::S3,
        ScenarioName::S6,
        ScenarioName::S7,
    ] {
        if let Some(cell) = profile
            .modes
            .get_mut("old_wallet")
            .and_then(|mode| mode.scenarios.get_mut(scenario.as_str()))
        {
            cell.notes.push(
                "fresh live scan cell disabled for this run; set benchmark.live_fresh_scan_cells=true for the long baseline pass"
                    .to_string(),
            );
        }

        if let Some(cell) = profile
            .modes
            .get_mut("new_wallet")
            .and_then(|mode| mode.scenarios.get_mut(scenario.as_str()))
        {
            cell.notes.push(
                "fresh live scan cell disabled for this run; set benchmark.live_fresh_scan_cells=true for the long baseline pass"
                    .to_string(),
            );
        }

        if let Some(cell) = profile
            .modes
            .get_mut("payment_processor")
            .and_then(|mode| mode.scenarios.get_mut(scenario.as_str()))
        {
            cell.status = CellStatus::NotApplicable;
            cell.notes.push(
                "PP has no direct scan API; companion-wallet scan cells run only when benchmark.live_fresh_scan_cells=true"
                    .to_string(),
            );
        }
    }
}

fn annotate_mode3_disabled(profile: &mut ResultProfile) {
    let Some(mode) = profile.modes.get_mut("payment_processor") else {
        return;
    };
    for scenario in [
        ScenarioName::S0,
        ScenarioName::S1,
        ScenarioName::S4,
        ScenarioName::S5,
    ] {
        if let Some(cell) = mode.scenarios.get_mut(scenario.as_str()) {
            cell.notes.push(
                "Mode 3 real payment-processor topology disabled; set benchmark.mode3_live_topology=true to spawn minotari PR daemon plus minotari_payment_processor"
                    .to_string(),
            );
        }
    }
}

fn annotate_mode1_disabled(profile: &mut ResultProfile) {
    let Some(mode) = profile.modes.get_mut("old_wallet") else {
        return;
    };
    for scenario in [
        ScenarioName::S0,
        ScenarioName::S1,
        ScenarioName::S4,
        ScenarioName::S5,
    ] {
        if let Some(cell) = mode.scenarios.get_mut(scenario.as_str()) {
            cell.notes.push(
                "Mode 1 console-wallet topology disabled; set benchmark.mode1_live_topology=true to spawn minotari_console_wallet with gRPC"
                    .to_string(),
            );
        }
    }
}

async fn annotate_mode1_console_wallet(
    config: &Config,
    book: &AddressBook,
    profile: &mut ResultProfile,
) -> anyhow::Result<()> {
    let Some(old_seed) = book.addresses.get(WalletRole::OldWallet.label()) else {
        return Ok(());
    };
    let Some(recipient_seed) = book.addresses.get(WalletRole::NewWallet.label()) else {
        return Ok(());
    };

    let start = Instant::now();
    let topology = start_mode1_console_wallet(config, old_seed).await;
    match topology {
        Ok(mut context) => {
            record_mode1_s0(config, profile, &context, start.elapsed().as_millis());
            run_mode1_send_cells(
                config,
                profile,
                old_seed,
                recipient_seed.address.clone(),
                &mut context,
            )
            .await?;
        }
        Err(error) => {
            record_mode1_startup_failure(profile, start.elapsed().as_millis(), error);
        }
    }
    Ok(())
}

async fn start_mode1_console_wallet(
    config: &Config,
    old_seed: &crate::seeds::SeedMaterial,
) -> anyhow::Result<Mode1ConsoleContext> {
    let password = wallet_password(&config.seeds.wallet_password_env)?;
    let base_path = old_wallet_base_path(config);
    let config_path = base_path.join("config/config.toml");
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir_all("logs")?;
    let stdout_path = PathBuf::from("logs/mode1-console-wallet.stdout.log");
    let stderr_path = PathBuf::from("logs/mode1-console-wallet.stderr.log");
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stdout_path)?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stderr_path)?;
    let grpc_bind = grpc_bind_multiaddr(&config.modes.old_wallet_grpc_address)?;
    let birthday = mode1_wallet_birthday(old_seed);
    // Console-wallet seed recovery reads the birthday embedded in the mnemonic.
    let recovery_seed_words = seed_words_with_birthday(&old_seed.seed_words, birthday)
        .context("encoding Mode 1 console-wallet recovery seed birthday")?;

    let mut command = Command::new(&config.paths.minotari_console_wallet);
    command
        .env("MINOTARI_WALLET_SEED_WORDS", recovery_seed_words)
        .env("MINOTARI_WALLET_PASSWORD", &password)
        .arg("--base-path")
        .arg(&base_path)
        .arg("--config")
        .arg(&config_path)
        .arg("--network")
        .arg("Esmeralda")
        .arg("--non-interactive-mode")
        .arg("--grpc-enabled")
        .arg("--grpc-address")
        .arg(&grpc_bind)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));

    let mut process = Mode1ConsoleProcess {
        child: command
            .spawn()
            .context("spawning minotari_console_wallet")?,
        stdout_path,
        stderr_path,
    };
    let client = wait_for_mode1_grpc(config, &mut process).await?;
    let mut context = Mode1ConsoleContext {
        process,
        client,
        balance: None,
        birthday,
        grpc_bind,
        version: None,
    };
    let version = context
        .client
        .get_version(grpc::GetVersionRequest {})
        .await?
        .into_inner()
        .version;
    context.version = Some(version);
    let required_balance = config.a_fund()?.0;
    let balance = wait_for_mode1_balance(config, &mut context, required_balance).await?;
    context.balance = Some(balance);
    Ok(context)
}

fn old_wallet_base_path(config: &Config) -> PathBuf {
    config.paths.data_dir.join("old-wallet-console")
}

fn mode1_wallet_birthday(seed: &crate::seeds::SeedMaterial) -> u16 {
    if seed.birthday == 0 {
        current_birthday()
    } else {
        seed.birthday
    }
}

fn grpc_bind_multiaddr(address: &str) -> anyhow::Result<String> {
    if address.starts_with('/') {
        return Ok(address.to_string());
    }
    let trimmed = address
        .strip_prefix("http://")
        .or_else(|| address.strip_prefix("https://"))
        .unwrap_or(address);
    let (host, port) = trimmed
        .rsplit_once(':')
        .with_context(|| format!("invalid gRPC address {address}"))?;
    Ok(format!("/ip4/{host}/tcp/{port}"))
}

fn mode1_scan_grpc_address(
    base_address: &str,
    spec: FreshScanSpec,
    run: u32,
) -> anyhow::Result<String> {
    let trimmed = base_address
        .strip_prefix("http://")
        .or_else(|| base_address.strip_prefix("https://"))
        .unwrap_or(base_address);
    let (host, port) = trimmed
        .rsplit_once(':')
        .with_context(|| format!("invalid gRPC address {base_address}"))?;
    let port = port.parse::<u16>()?;
    let offset = spec.port_offset(run);
    let scheme = if base_address.starts_with("https://") {
        "https"
    } else {
        "http"
    };
    Ok(format!(
        "{scheme}://{host}:{}",
        port.checked_add(offset)
            .with_context(|| format!("scan gRPC port overflow for {base_address}"))?
    ))
}

async fn wait_for_mode1_grpc(
    config: &Config,
    process: &mut Mode1ConsoleProcess,
) -> anyhow::Result<WalletGrpcClient<tonic::transport::Channel>> {
    wait_for_mode1_grpc_address(config, process, &config.modes.old_wallet_grpc_address).await
}

async fn wait_for_mode1_grpc_address(
    config: &Config,
    process: &mut Mode1ConsoleProcess,
    grpc_address: &str,
) -> anyhow::Result<WalletGrpcClient<tonic::transport::Channel>> {
    let start = Instant::now();
    let timeout = config.timeout(config.timeouts.startup_secs);
    let mut interval = time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;
        if let Some(status) = process.try_wait()? {
            bail!(
                "minotari_console_wallet exited during gRPC startup with status {status}; stdout_log={} stderr_log={}",
                process.stdout_path.display(),
                process.stderr_path.display()
            );
        }
        match time::timeout(
            Duration::from_secs(5),
            WalletGrpcClient::connect(grpc_address),
        )
        .await
        {
            Ok(Ok(client)) => return Ok(client),
            Ok(Err(error)) => {
                if start.elapsed() > timeout {
                    bail!("console wallet gRPC did not become ready within {timeout:?}: {error}");
                }
            }
            Err(_) => {
                if start.elapsed() > timeout {
                    bail!("console wallet gRPC connect timed out for {timeout:?}");
                }
            }
        }
    }
}

async fn wait_for_mode1_scan_to_tip(
    process: &mut Mode1ConsoleProcess,
    client: &mut WalletGrpcClient<tonic::transport::Channel>,
    target_tip: Option<u64>,
    timeout: Duration,
) -> anyhow::Result<u64> {
    let start = Instant::now();
    let mut interval = time::interval(Duration::from_secs(5));
    loop {
        interval.tick().await;
        if let Some(status) = process.try_wait()? {
            bail!(
                "minotari_console_wallet exited during fresh scan with status {status}; stdout_log={} stderr_log={}",
                process.stdout_path.display(),
                process.stderr_path.display()
            );
        }
        let state = client
            .get_state(grpc::GetStateRequest {})
            .await?
            .into_inner();
        let scanned_height = state.scanned_height;
        let target = target_tip.unwrap_or_else(|| scanned_height.saturating_add(1));
        if scanned_height >= target {
            return Ok(scanned_height);
        }
        if start.elapsed() > timeout {
            bail!(
                "console wallet fresh scan did not reach target tip {target} within {:?}; scanned_height={scanned_height}",
                timeout
            );
        }
        println!("mode1 fresh scan wait: scanned_height={scanned_height} target={target}");
    }
}

async fn wait_for_mode1_balance(
    config: &Config,
    context: &mut Mode1ConsoleContext,
    min_available: u64,
) -> anyhow::Result<grpc::GetBalanceResponse> {
    let start = Instant::now();
    let timeout = config.timeout(config.timeouts.startup_secs);
    let mut last_report = Instant::now();
    let mut interval = time::interval(Duration::from_secs(5));
    loop {
        interval.tick().await;
        if let Some(status) = context.process.try_wait()? {
            bail!(
                "minotari_console_wallet exited during startup with status {status}; stdout_log={} stderr_log={}",
                context.process.stdout_path.display(),
                context.process.stderr_path.display()
            );
        }
        let balance = context
            .client
            .get_balance(grpc::GetBalanceRequest { payment_id: None })
            .await?
            .into_inner();
        if balance.available_balance >= min_available {
            return Ok(balance);
        }
        if last_report.elapsed() >= Duration::from_secs(30) {
            println!(
                "mode1 console wallet balance wait: available={} pending_in={} pending_out={} required={}",
                balance.available_balance,
                balance.pending_incoming_balance,
                balance.pending_outgoing_balance,
                min_available
            );
            last_report = Instant::now();
        }
        if start.elapsed() > timeout {
            bail!(
                "console wallet did not reach required available balance {} within {:?}; available={} pending_in={} pending_out={}",
                min_available,
                timeout,
                balance.available_balance,
                balance.pending_incoming_balance,
                balance.pending_outgoing_balance
            );
        }
    }
}

fn record_mode1_s0(
    config: &Config,
    profile: &mut ResultProfile,
    context: &Mode1ConsoleContext,
    wall_ms: u128,
) {
    let Some(mode) = profile.modes.get_mut("old_wallet") else {
        return;
    };
    let Some(cell) = mode.scenarios.get_mut("S0") else {
        return;
    };
    let balance = context.balance.as_ref();
    cell.record_repetition(Repetition {
        run: 1,
        status: CellStatus::Ok,
        wall_ms: Some(wall_ms),
        success_count: 1,
        failure_count: 0,
        fee_microtari: None,
        error: None,
        metrics: None,
    });
    cell.notes.push(format!(
        "Mode 1 topology started real minotari_console_wallet gRPC version {}; grpc_address={} grpc_bind={} base_path={} birthday={} balance_available={} pending_in={} pending_out={}",
        context.version.as_deref().unwrap_or("unknown"),
        config.modes.old_wallet_grpc_address,
        context.grpc_bind,
        old_wallet_base_path(config).display(),
        context.birthday,
        balance.map(|b| b.available_balance).unwrap_or_default(),
        balance.map(|b| b.pending_incoming_balance).unwrap_or_default(),
        balance.map(|b| b.pending_outgoing_balance).unwrap_or_default()
    ));
    if let Some(funding) = &config.funding.old_wallet {
        cell.notes.push(format!(
            "funding tx_id={} height={} amount={}",
            funding.tx_id, funding.height, funding.amount
        ));
    }
}

fn record_mode1_startup_failure(profile: &mut ResultProfile, wall_ms: u128, error: anyhow::Error) {
    let Some(mode) = profile.modes.get_mut("old_wallet") else {
        return;
    };
    for scenario in [
        ScenarioName::S0,
        ScenarioName::S1,
        ScenarioName::S4,
        ScenarioName::S5,
    ] {
        let Some(cell) = mode.scenarios.get_mut(scenario.as_str()) else {
            continue;
        };
        cell.record_repetition(Repetition {
            run: 1,
            status: CellStatus::Failed,
            wall_ms: Some(wall_ms),
            success_count: 0,
            failure_count: 1,
            fee_microtari: None,
            error: Some(format!("{error:#}")),
            metrics: None,
        });
        cell.notes
            .push("Mode 1 console-wallet startup failed before scenario dispatch".to_string());
    }
}

async fn run_mode1_send_cells(
    config: &Config,
    profile: &mut ResultProfile,
    old_seed: &crate::seeds::SeedMaterial,
    recipient: String,
    context: &mut Mode1ConsoleContext,
) -> anyhow::Result<()> {
    let amount = parse_amount(&config.benchmark.mode1_scenario_amount)?;
    let fee_rate = config.fee_rate()?.0;
    let s1 = run_mode1_s1(config, &mut context.client, amount, fee_rate).await;
    record_mode1_transfer_summary(
        profile,
        ScenarioName::S1,
        &s1,
        vec![format!(
            "Mode 1 S1 drove console-wallet gRPC CoinSplit rounds; attempted_batches={} amount_per_output={} cap={}",
            s1.attempted_batches,
            config.benchmark.mode1_scenario_amount,
            config.benchmark.mode1_live_max_s1_txs
        )],
    );
    if config.benchmark.live_fresh_scan_cells {
        let checkpoint = checkpoint_from_mode1_summary(&s1, ScanCheckpoint::PostS1);
        run_mode1_checkpoint_scan_cells(
            config,
            profile,
            old_seed,
            &[ScenarioName::S2, ScenarioName::S3],
            checkpoint,
        )
        .await?;
    }

    let mut s4 = run_mode1_s4_batches(config, &context.client, &recipient, amount, fee_rate).await;
    wait_for_mode1_scan_advance(
        &mut context.client,
        config.settle_wait_blocks(),
        config.timeout(config.timeouts.confirmation_secs),
    )
    .await;
    wait_for_mode1_summary_verification(
        &mut context.client,
        &mut s4,
        ScenarioName::S4,
        config.timeout(config.timeouts.confirmation_secs),
    )
    .await;
    record_mode1_transfer_summary(
        profile,
        ScenarioName::S4,
        &s4,
        vec![format!(
            "Mode 1 S4 dispatched configured concurrent_batches={:?} through console-wallet gRPC Transfer; per-batch cap={}",
            config.benchmark.concurrent_batches, config.benchmark.mode1_live_max_s4_batch
        )],
    );

    let s5_recipients = derive_distinct_recipient_pool(config.benchmark.s5_m)?;
    let s5 = run_mode1_s5(
        config,
        &mut context.client,
        &s5_recipients,
        amount,
        fee_rate,
    )
    .await;
    record_mode1_transfer_summary(
        profile,
        ScenarioName::S5,
        &s5,
        vec![format!(
            "Mode 1 S5 used deterministic distinct recipients; attempted_payments={} of configured S5_M={} and S5_K={}; cap={}",
            s5.attempted_payments,
            config.benchmark.s5_m,
            config.benchmark.s5_k,
            config.benchmark.mode1_live_max_s5_items
        )],
    );
    if config.benchmark.live_fresh_scan_cells {
        let checkpoint = checkpoint_from_mode1_summary(&s5, ScanCheckpoint::PostS5Complete);
        run_mode1_checkpoint_scan_cells(
            config,
            profile,
            old_seed,
            &[ScenarioName::S6, ScenarioName::S7],
            checkpoint,
        )
        .await?;
    }
    Ok(())
}

async fn run_mode1_s1(
    config: &Config,
    client: &mut WalletGrpcClient<tonic::transport::Channel>,
    amount: MicroMinotari,
    fee_rate: u64,
) -> Mode1TransferSummary {
    let mut total = Mode1TransferSummary::default();
    let start = Instant::now();
    let rounds = s1_round_plan(config, config.benchmark.mode1_live_max_s1_txs);
    let balance_before = mode1_available_balance(client).await.ok();
    for round in rounds {
        let round_start = Instant::now();
        let mut round_summary = Mode1TransferSummary {
            attempted_batches: round.tx_count,
            attempted_payments: round.tx_count.saturating_mul(round.outputs_per_tx),
            ..Mode1TransferSummary::default()
        };
        for tx_index in 1..=round.tx_count {
            println!(
                "old_wallet/S1 round {} tx {}/{} coin_split outputs={}",
                round.round_index, tx_index, round.tx_count, round.outputs_per_tx
            );
            let result = submit_mode1_coin_split(
                client,
                amount,
                round.outputs_per_tx,
                fee_rate,
                format!("wallet-bench-S1-r{}-{tx_index}", round.round_index).into_bytes(),
            )
            .await;
            round_summary.record_batch(tx_index, round.outputs_per_tx, result);
            round_summary
                .construction_complete_ms
                .push(round_start.elapsed().as_millis());
        }
        round_summary.wall_ms = round_start.elapsed().as_millis();
        wait_for_mode1_scan_advance(
            client,
            config.settle_wait_blocks(),
            config.timeout(config.timeouts.confirmation_secs),
        )
        .await;
        wait_for_mode1_summary_verification(
            client,
            &mut round_summary,
            ScenarioName::S1,
            config.timeout(config.timeouts.confirmation_secs),
        )
        .await;
        let observed_utxos = mode1_unspent_count(client).await.ok();
        let balance_after = mode1_available_balance(client).await.ok();
        round_summary.extra_metrics.insert(
            format!("round_{}", round.round_index),
            serde_json::json!({
                "round_index": round.round_index,
                "fanout": round.fanout,
                "tx_count": round.tx_count,
                "outputs_per_tx": round.outputs_per_tx,
                "target_utxos_after": round.target_utxos_after,
                "observed_unspent_count": observed_utxos,
                "balance_after_microtari": balance_after,
                "verified_count": round_summary.tx_infos.iter().filter(|tx| tx.confirmed).count(),
                "wall_ms": round_summary.wall_ms
            }),
        );
        total.add_batch(round.round_index, round_summary);
        if total.failure_count > 0 {
            break;
        }
    }
    total.wall_ms = start.elapsed().as_millis();
    total.extra_metrics.insert(
        "balance_before_microtari".to_string(),
        serde_json::json!(balance_before),
    );
    total.extra_metrics.insert(
        "balance_after_microtari".to_string(),
        serde_json::json!(mode1_available_balance(client).await.ok()),
    );
    total
}

async fn run_mode1_s5(
    config: &Config,
    client: &mut WalletGrpcClient<tonic::transport::Channel>,
    recipients: &[String],
    amount: MicroMinotari,
    fee_rate: u64,
) -> Mode1TransferSummary {
    let s5_items = capped_attempts(
        config.benchmark.s5_m,
        config.benchmark.mode1_live_max_s5_items,
    );
    let selected = recipients
        .iter()
        .take(s5_items as usize)
        .cloned()
        .collect::<Vec<_>>();
    let start = Instant::now();
    let mut total = Mode1TransferSummary::default();
    let mut batch_recipients = Vec::new();
    let mut current_batch = Vec::new();
    for recipient in &selected {
        current_batch.push(recipient.clone());
        if current_batch.len() == config.benchmark.s5_k as usize {
            batch_recipients.push(std::mem::take(&mut current_batch));
        }
    }
    if !current_batch.is_empty() {
        batch_recipients.push(current_batch);
    }
    let batch_arm = run_mode1_recipient_batches_sequential(
        "old_wallet/S5 batch",
        client,
        ScenarioName::S5,
        batch_recipients,
        true,
        amount,
        fee_rate,
    )
    .await;
    wait_for_mode1_scan_advance(
        client,
        config.settle_wait_blocks(),
        config.timeout(config.timeouts.confirmation_secs),
    )
    .await;
    // Settle gate between the two S5 arms; dispatch inside each arm stays immediate.
    settle_gate_pause(Duration::from_secs(config.benchmark.settle_cooldown_secs)).await;
    let individual_recipients = selected
        .into_iter()
        .map(|recipient| vec![recipient])
        .collect::<Vec<_>>();
    let individual_arm = run_mode1_recipient_batches_sequential(
        "old_wallet/S5 individual",
        client,
        ScenarioName::S5,
        individual_recipients,
        false,
        amount,
        fee_rate,
    )
    .await;
    total.add_batch(config.benchmark.s5_k, batch_arm);
    total.add_batch(1, individual_arm);
    wait_for_mode1_scan_advance(
        client,
        config.settle_wait_blocks(),
        config.timeout(config.timeouts.confirmation_secs),
    )
    .await;
    wait_for_mode1_summary_verification(
        client,
        &mut total,
        ScenarioName::S5,
        config.timeout(config.timeouts.confirmation_secs),
    )
    .await;
    total.wall_ms = start.elapsed().as_millis();
    total.extra_metrics.insert(
        "s5_arms".to_string(),
        serde_json::json!({
            "recipient_count": s5_items,
            "batch_size": config.benchmark.s5_k,
            "settle_cooldown_secs": config.benchmark.settle_cooldown_secs
        }),
    );
    total
}

async fn run_mode1_s4_batches(
    config: &Config,
    client: &WalletGrpcClient<tonic::transport::Channel>,
    recipient: &str,
    amount: MicroMinotari,
    fee_rate: u64,
) -> Mode1TransferSummary {
    let mut total = Mode1TransferSummary::default();
    let start = Instant::now();
    for configured_batch in &config.benchmark.concurrent_batches {
        let attempts = capped_attempts(*configured_batch, config.benchmark.mode1_live_max_s4_batch);
        let batch = run_mode1_transfers_concurrent(
            &format!("old_wallet/S4 batch {configured_batch}"),
            client,
            ScenarioName::S4,
            attempts,
            1,
            false,
            recipient,
            amount,
            fee_rate,
        )
        .await;
        total.add_batch(*configured_batch, batch);
    }
    total.wall_ms = start.elapsed().as_millis();
    total
}

#[allow(clippy::too_many_arguments)]
async fn run_mode1_transfers_concurrent(
    label: &str,
    client: &WalletGrpcClient<tonic::transport::Channel>,
    scenario: ScenarioName,
    batch_count: u32,
    items_per_batch: u32,
    single_tx: bool,
    recipient: &str,
    amount: MicroMinotari,
    fee_rate: u64,
) -> Mode1TransferSummary {
    let mut summary = Mode1TransferSummary {
        attempted_batches: batch_count,
        attempted_payments: batch_count.saturating_mul(items_per_batch),
        ..Mode1TransferSummary::default()
    };
    let start = Instant::now();
    let mut join_set = JoinSet::new();
    for batch_index in 1..=batch_count {
        println!("{label} batch {batch_index}/{batch_count} dispatching");
        let mut client = client.clone();
        let recipient = recipient.to_string();
        join_set.spawn(async move {
            (
                batch_index,
                submit_mode1_transfer(
                    &mut client,
                    scenario,
                    batch_index,
                    items_per_batch,
                    single_tx,
                    &recipient,
                    amount,
                    fee_rate,
                )
                .await,
            )
        });
    }
    while let Some(result) = join_set.join_next().await {
        match result {
            Ok((batch_index, transfer)) => {
                summary.record_batch(batch_index, items_per_batch, transfer)
            }
            Err(error) => summary.record_join_error(error.to_string()),
        }
    }
    summary.wall_ms = start.elapsed().as_millis();
    summary
}

async fn run_mode1_recipient_batches_sequential(
    label: &str,
    client: &mut WalletGrpcClient<tonic::transport::Channel>,
    scenario: ScenarioName,
    recipient_batches: Vec<Vec<String>>,
    single_tx: bool,
    amount: MicroMinotari,
    fee_rate: u64,
) -> Mode1TransferSummary {
    let mut summary = Mode1TransferSummary {
        attempted_batches: recipient_batches.len().try_into().unwrap_or(u32::MAX),
        attempted_payments: recipient_batches
            .iter()
            .map(|batch| u32::try_from(batch.len()).unwrap_or(u32::MAX))
            .fold(0u32, u32::saturating_add),
        ..Mode1TransferSummary::default()
    };
    let start = Instant::now();
    for (index, recipients) in recipient_batches.into_iter().enumerate() {
        let batch_index = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let items_per_batch = u32::try_from(recipients.len()).unwrap_or(u32::MAX);
        println!(
            "{label} batch {}/{} dispatching recipients={}",
            batch_index,
            summary.attempted_batches,
            recipients.len()
        );
        let result = submit_mode1_transfer_to_recipients(
            client,
            scenario,
            batch_index,
            recipients,
            single_tx,
            amount,
            fee_rate,
        )
        .await;
        summary
            .construction_complete_ms
            .push(start.elapsed().as_millis());
        summary.record_batch(batch_index, items_per_batch, result);
    }
    summary.wall_ms = start.elapsed().as_millis();
    summary
}

#[allow(clippy::too_many_arguments)]
async fn submit_mode1_transfer(
    client: &mut WalletGrpcClient<tonic::transport::Channel>,
    scenario: ScenarioName,
    batch_index: u32,
    items_per_batch: u32,
    single_tx: bool,
    recipient: &str,
    amount: MicroMinotari,
    fee_rate: u64,
) -> anyhow::Result<Mode1TransferOutcome> {
    let recipients = (1..=items_per_batch)
        .map(|_| recipient.to_string())
        .collect::<Vec<_>>();
    submit_mode1_transfer_to_recipients(
        client,
        scenario,
        batch_index,
        recipients,
        single_tx,
        amount,
        fee_rate,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn submit_mode1_transfer_to_recipients(
    client: &mut WalletGrpcClient<tonic::transport::Channel>,
    scenario: ScenarioName,
    batch_index: u32,
    recipients: Vec<String>,
    single_tx: bool,
    amount: MicroMinotari,
    fee_rate: u64,
) -> anyhow::Result<Mode1TransferOutcome> {
    let recipients = recipients
        .into_iter()
        .enumerate()
        .map(|(index, address)| grpc::PaymentRecipient {
            address,
            amount: amount.0,
            fee_per_gram: fee_rate,
            payment_type: 1,
            raw_payment_id: format!(
                "wallet-bench-{}-{batch_index}-{}",
                scenario.as_str(),
                index + 1
            )
            .into_bytes(),
            user_payment_id: None,
        })
        .collect::<Vec<_>>();
    let mut db_lock_retries = 0u32;
    let response = loop {
        match client
            .transfer(grpc::TransferRequest {
                recipients: recipients.clone(),
                single_tx,
            })
            .await
        {
            Ok(response) => break response,
            Err(status)
                if mode1_status_is_database_locked(&status)
                    && db_lock_retries < MODE1_DB_LOCK_RETRY_ATTEMPTS =>
            {
                db_lock_retries = db_lock_retries.saturating_add(1);
                wait_after_mode1_database_lock(
                    &format!("mode1 {} batch {batch_index} transfer", scenario.as_str()),
                    db_lock_retries,
                )
                .await;
            }
            Err(status) => return Err(status.into()),
        }
    };
    Ok(Mode1TransferOutcome::from_response(response.into_inner()).with_retries(db_lock_retries))
}

async fn submit_mode1_coin_split(
    client: &mut WalletGrpcClient<tonic::transport::Channel>,
    amount: MicroMinotari,
    split_count: u32,
    fee_rate: u64,
    payment_id: Vec<u8>,
) -> anyhow::Result<Mode1TransferOutcome> {
    let request = grpc::CoinSplitRequest {
        amount_per_split: amount.0,
        split_count: split_count.into(),
        fee_per_gram: fee_rate,
        lock_height: 0,
        payment_id,
    };
    let mut db_lock_retries = 0u32;
    let response = loop {
        match client.coin_split(request.clone()).await {
            Ok(response) => break response,
            Err(status)
                if mode1_status_is_database_locked(&status)
                    && db_lock_retries < MODE1_DB_LOCK_RETRY_ATTEMPTS =>
            {
                db_lock_retries = db_lock_retries.saturating_add(1);
                wait_after_mode1_database_lock(
                    &format!("mode1 S1 coin_split outputs={split_count}"),
                    db_lock_retries,
                )
                .await;
            }
            Err(status) => return Err(status.into()),
        }
    };
    let response = response.into_inner();
    Ok(Mode1TransferOutcome {
        success_count: 1,
        failure_count: 0,
        fee_microtari: 0,
        tx_ids: vec![response.tx_id.to_string()],
        errors: Vec::new(),
        db_lock_retries,
    })
}

fn mode1_status_is_database_locked(status: &tonic::Status) -> bool {
    let message = status.message().to_ascii_lowercase();
    message.contains("database is locked")
}

async fn wait_after_mode1_database_lock(label: &str, retry: u32) {
    let backoff = Duration::from_millis(250 * u64::from(retry));
    println!(
        "{label} hit wallet database lock; retry {retry}/{MODE1_DB_LOCK_RETRY_ATTEMPTS} after {}ms",
        backoff.as_millis()
    );
    time::sleep_until(time::Instant::now() + backoff).await;
}

async fn mode1_unspent_count(
    client: &mut WalletGrpcClient<tonic::transport::Channel>,
) -> anyhow::Result<u64> {
    let response = client
        .get_unspent_amounts(grpc::Empty {})
        .await?
        .into_inner();
    Ok(response.amount.len().try_into().unwrap_or(u64::MAX))
}

async fn mode1_available_balance(
    client: &mut WalletGrpcClient<tonic::transport::Channel>,
) -> anyhow::Result<u64> {
    let response = client
        .get_balance(grpc::GetBalanceRequest { payment_id: None })
        .await?
        .into_inner();
    Ok(response.available_balance)
}

async fn wait_for_mode1_scan_advance(
    client: &mut WalletGrpcClient<tonic::transport::Channel>,
    blocks: u64,
    timeout: Duration,
) {
    let Ok(start_state) = client
        .get_state(grpc::GetStateRequest {})
        .await
        .map(|r| r.into_inner())
    else {
        return;
    };
    let target = start_state.scanned_height.saturating_add(blocks);
    let start = Instant::now();
    let mut interval = time::interval(Duration::from_secs(10));
    while start.elapsed() < timeout {
        interval.tick().await;
        let Ok(state) = client
            .get_state(grpc::GetStateRequest {})
            .await
            .map(|r| r.into_inner())
        else {
            continue;
        };
        if state.scanned_height >= target {
            return;
        }
        println!(
            "mode1 settle wait: scanned_height={} target={}",
            state.scanned_height, target
        );
    }
}

async fn verify_mode1_transactions(
    client: &mut WalletGrpcClient<tonic::transport::Channel>,
    tx_ids: &[String],
    scenario: ScenarioName,
) -> anyhow::Result<Vec<VerifiedTransaction>> {
    let ids = tx_ids
        .iter()
        .filter_map(|tx_id| tx_id.parse::<u64>().ok())
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let response = client
        .get_transaction_info(grpc::GetTransactionInfoRequest {
            transaction_ids: ids,
        })
        .await?
        .into_inner();
    Ok(response
        .transactions
        .into_iter()
        .map(|info| {
            let status_value = u32::try_from(info.status).unwrap_or_default();
            VerifiedTransaction {
                tx_id: info.tx_id.to_string(),
                status_value,
                mode: "old_wallet".to_string(),
                scenario: scenario.as_str().to_string(),
                amount_microtari: Some(info.amount),
                fee_microtari: Some(info.fee),
                mined_height: (info.mined_in_block_height > 0)
                    .then_some(info.mined_in_block_height),
                confirmed: status_value == TX_MINED_CONFIRMED_STATUS
                    || terminal_ok_status(status_value),
            }
        })
        .collect())
}

async fn wait_for_mode1_summary_verification(
    client: &mut WalletGrpcClient<tonic::transport::Channel>,
    summary: &mut Mode1TransferSummary,
    scenario: ScenarioName,
    timeout: Duration,
) {
    if summary.tx_ids.is_empty() {
        return;
    }
    let start = Instant::now();
    let mut interval = time::interval(Duration::from_secs(10));
    let mut latest = Vec::new();
    loop {
        let remaining = timeout.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            break;
        }
        let call_timeout = remaining.min(Duration::from_secs(30));
        match time::timeout(
            call_timeout,
            verify_mode1_transactions(client, &summary.tx_ids, scenario),
        )
        .await
        {
            Ok(Ok(verified)) => {
                let all_terminal = !verified.is_empty()
                    && verified.len() >= summary.tx_ids.len()
                    && verified.iter().all(|tx| tx.confirmed);
                println!(
                    "mode1 verification wait: scenario={} confirmed={}/{} statuses={}",
                    scenario.as_str(),
                    verified.iter().filter(|tx| tx.confirmed).count(),
                    summary.tx_ids.len(),
                    verified
                        .iter()
                        .map(|tx| format!("{}:{}", tx.tx_id, tx.status_value))
                        .collect::<Vec<_>>()
                        .join(",")
                );
                latest = verified;
                if all_terminal {
                    break;
                }
            }
            Ok(Err(error)) => {
                summary
                    .errors
                    .push(format!("mode1 chain verification failed: {error:#}"));
                break;
            }
            Err(_) => {
                summary.errors.push(format!(
                    "mode1 chain verification timed out after {}s for {} tx ids",
                    call_timeout.as_secs(),
                    summary.tx_ids.len()
                ));
                break;
            }
        }
        interval.tick().await;
    }
    summary.tx_infos.extend(latest);
    summary.backfill_verified_fee_total();
}

async fn verify_mode2_transactions_with_client(
    config: &Config,
    db_path: &Path,
    tx_ids: &[String],
    scenario: ScenarioName,
    client: &reqwest::Client,
) -> anyhow::Result<Mode2VerificationResult> {
    if tx_ids.is_empty() || !db_path.exists() {
        return Ok(Mode2VerificationResult::default());
    }
    let conn = Connection::open(db_path)?;
    let tip_height = base_node_tip_height_with_client(client, &config.network.base_node_http_url)
        .await
        .ok();
    let mut result = Mode2VerificationResult::default();
    for tx_id in tx_ids {
        let Ok(parsed) = tx_id.parse::<u64>() else {
            continue;
        };
        let row = mode2_completed_transaction_row(&conn, parsed as i64)?;
        let (status_value, confirmed, mined_height, source, query_observation) = match row.as_ref()
        {
            Some(row) => {
                let kernel_query =
                    mode2_kernel_query_from_serialized_transaction(&row.serialized_transaction);
                match kernel_query {
                    Ok(kernel_query) => {
                        let query = query_mode2_transaction(
                            client,
                            &config.network.base_node_http_url,
                            &kernel_query,
                        )
                        .await;
                        match query {
                            Ok(response) => {
                                let (status_value, confirmed) = mode2_transaction_query_status(
                                    &response,
                                    tip_height,
                                    config.benchmark.c_min,
                                );
                                let mined_height = response.mined_height.or(row
                                    .confirmation_height
                                    .or(row.mined_height)
                                    .and_then(|height| u64::try_from(height).ok()));
                                (
                                    status_value,
                                    confirmed,
                                    mined_height,
                                    "base_node_transaction_query",
                                    mode2_query_observation(
                                        tx_id,
                                        row,
                                        Some(&kernel_query),
                                        Some(&response),
                                        tip_height,
                                        confirmed,
                                        None,
                                    ),
                                )
                            }
                            Err(error) => {
                                let (status_value, db_confirmed) =
                                    mode2_completed_transaction_status(&row.status);
                                (
                                    status_value,
                                    false,
                                    row.confirmation_height
                                        .or(row.mined_height)
                                        .and_then(|height| u64::try_from(height).ok()),
                                    "wallet_db_observed",
                                    mode2_query_observation(
                                        tx_id,
                                        row,
                                        Some(&kernel_query),
                                        None,
                                        tip_height,
                                        db_confirmed,
                                        Some(format!("{error:#}")),
                                    ),
                                )
                            }
                        }
                    }
                    Err(error) => {
                        let (status_value, db_confirmed) =
                            mode2_completed_transaction_status(&row.status);
                        (
                            status_value,
                            false,
                            row.confirmation_height
                                .or(row.mined_height)
                                .and_then(|height| u64::try_from(height).ok()),
                            "wallet_db_observed",
                            mode2_query_observation(
                                tx_id,
                                row,
                                None,
                                None,
                                tip_height,
                                db_confirmed,
                                Some(format!("{error:#}")),
                            ),
                        )
                    }
                }
            }
            None => (
                0,
                false,
                None,
                "wallet_db_observed",
                serde_json::json!({
                    "tx_id": tx_id,
                    "verification_source": "wallet_db_observed",
                    "wallet_db_status": "not_found",
                    "confirmed": false
                }),
            ),
        };

        result.observed_transactions.push(VerifiedTransaction {
            tx_id: tx_id.clone(),
            status_value,
            mode: "new_wallet".to_string(),
            scenario: scenario.as_str().to_string(),
            amount_microtari: None,
            fee_microtari: row
                .as_ref()
                .and_then(|row| {
                    mode2_kernel_query_from_serialized_transaction(&row.serialized_transaction).ok()
                })
                .and_then(|query| query.fee_microtari),
            mined_height,
            confirmed,
        });
        result.observations.push(query_observation);
        if source == "base_node_transaction_query" {
            result.used_base_node_query = true;
        }
    }
    Ok(result)
}

fn mode2_completed_transaction_status(status: &str) -> (u32, bool) {
    match status {
        "mined_confirmed" => (TX_MINED_CONFIRMED_STATUS, true),
        "mined_unconfirmed" => (2, false),
        "broadcast" => (1, false),
        "completed" => (0, false),
        "rejected" => (7, false),
        "canceled" => (14, false),
        _ => (0, false),
    }
}

fn mode2_completed_transaction_row(
    conn: &Connection,
    tx_id: i64,
) -> anyhow::Result<Option<Mode2CompletedTransactionRow>> {
    let row = conn.query_row(
        r#"
        SELECT pending_tx_id, status, mined_height, confirmation_height, sent_payref, serialized_transaction
        FROM completed_transactions
        WHERE id = ?1
        "#,
        [tx_id],
        |row| {
            Ok(Mode2CompletedTransactionRow {
                pending_tx_id: row.get::<_, String>(0)?,
                status: row.get::<_, String>(1)?,
                mined_height: row.get::<_, Option<i64>>(2)?,
                confirmation_height: row.get::<_, Option<i64>>(3)?,
                sent_payref: row.get::<_, Option<String>>(4)?,
                serialized_transaction: row.get::<_, Vec<u8>>(5)?,
            })
        },
    );
    match row {
        Ok(row) => Ok(Some(row)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn mode2_kernel_query_from_serialized_transaction(
    serialized_transaction: &[u8],
) -> anyhow::Result<Mode2KernelQuery> {
    let transaction: Transaction = serde_json::from_slice(serialized_transaction)
        .context("deserializing Mode 2 transaction")?;
    let kernel = transaction
        .body()
        .kernels()
        .first()
        .context("Mode 2 transaction has no kernel")?;
    Ok(Mode2KernelQuery {
        excess_sig_nonce: kernel
            .excess_sig
            .get_compressed_public_nonce()
            .as_bytes()
            .to_vec(),
        excess_sig: kernel.excess_sig.get_signature().as_bytes().to_vec(),
        fee_microtari: Some(kernel.fee.0),
    })
}

async fn query_mode2_transaction(
    client: &reqwest::Client,
    base_node_url: &str,
    query: &Mode2KernelQuery,
) -> anyhow::Result<TxQueryResponse> {
    let url = mode2_transaction_query_url(base_node_url, query)?;
    let response = client
        .get(url)
        .send()
        .await
        .context("requesting base-node transaction query")?
        .error_for_status()
        .context("base-node transaction query HTTP status")?
        .json::<TxQueryResponse>()
        .await
        .context("decoding base-node transaction query")?;
    Ok(response)
}

fn mode2_transaction_query_url(
    base_node_url: &str,
    query: &Mode2KernelQuery,
) -> anyhow::Result<url::Url> {
    let mut url = base_node_endpoint_url(base_node_url, "/transactions")?;
    url.query_pairs_mut()
        .append_pair("excess_sig_nonce", &hex::encode(&query.excess_sig_nonce))
        .append_pair("excess_sig_sig", &hex::encode(&query.excess_sig));
    Ok(url)
}

fn mode2_transaction_query_status(
    response: &TxQueryResponse,
    tip_height: Option<u64>,
    required_depth: u64,
) -> (u32, bool) {
    match response.location {
        TxLocation::Mined => {
            let confirmed = response
                .mined_height
                .zip(tip_height)
                .is_some_and(|(mined, tip)| tip >= mined.saturating_add(required_depth));
            if confirmed {
                (TX_MINED_CONFIRMED_STATUS, true)
            } else {
                (2, false)
            }
        }
        TxLocation::InMempool => (1, false),
        TxLocation::NotStored | TxLocation::None => (0, false),
    }
}

fn mode2_query_observation(
    tx_id: &str,
    row: &Mode2CompletedTransactionRow,
    kernel_query: Option<&Mode2KernelQuery>,
    response: Option<&TxQueryResponse>,
    tip_height: Option<u64>,
    confirmed: bool,
    query_error: Option<String>,
) -> serde_json::Value {
    let (db_status_value, db_confirmed) = mode2_completed_transaction_status(&row.status);
    serde_json::json!({
        "tx_id": tx_id,
        "pending_tx_id": row.pending_tx_id,
        "sent_payref": row.sent_payref,
        "verification_source": if response.is_some() { "base_node_transaction_query" } else { "wallet_db_observed" },
        "wallet_db_status": row.status,
        "wallet_db_status_value": db_status_value,
        "wallet_db_confirmed": db_confirmed,
        "wallet_db_mined_height": row.mined_height,
        "wallet_db_confirmation_height": row.confirmation_height,
        "base_node_query_location": response.map(|response| format!("{:?}", response.location)),
        "base_node_query_mined_height": response.and_then(|response| response.mined_height),
        "base_node_tip_height": tip_height,
        "fee_microtari": kernel_query.and_then(|query| query.fee_microtari),
        "confirmed": confirmed,
        "query_error": query_error
    })
}

fn record_mode1_transfer_summary(
    profile: &mut ResultProfile,
    scenario: ScenarioName,
    summary: &Mode1TransferSummary,
    mut notes: Vec<String>,
) {
    profile
        .chain_verification
        .verified_transactions
        .extend(summary.tx_infos.clone());
    let Some(mode) = profile.modes.get_mut("old_wallet") else {
        return;
    };
    let Some(cell) = mode.scenarios.get_mut(scenario.as_str()) else {
        return;
    };
    let verification_complete = summary.tx_ids.is_empty() || !summary.tx_infos.is_empty();
    let all_verified_ok = summary.tx_infos.iter().all(|tx| tx.confirmed);
    let status = if summary.failure_count == 0 && verification_complete && all_verified_ok {
        CellStatus::Ok
    } else {
        CellStatus::Failed
    };
    cell.record_repetition(Repetition {
        run: 1,
        status,
        wall_ms: Some(summary.wall_ms),
        success_count: summary.success_count,
        failure_count: summary.failure_count,
        fee_microtari: Some(summary.fee_microtari),
        error: summary.error_note().or_else(|| {
            (!all_verified_ok)
                .then_some("one or more tx_ids did not verify as terminal-ok".to_string())
                .or_else(|| {
                    (!verification_complete).then_some(
                        "tx_ids were produced but chain verification returned no rows".to_string(),
                    )
                })
        }),
        metrics: Some(summary.metrics(scenario)),
    });
    notes.push(summary.note(scenario));
    cell.notes.extend(notes);
}

async fn annotate_mode2_send_smoke(
    config: &Config,
    book: &AddressBook,
    profile: &mut ResultProfile,
) -> anyhow::Result<()> {
    let Some(sender_seed) = book.addresses.get(WalletRole::NewWallet.label()) else {
        return Ok(());
    };
    let Some(recipient_seed) = book.addresses.get(WalletRole::OldWallet.label()) else {
        return Ok(());
    };

    let password = wallet_password(&config.seeds.wallet_password_env)?;
    let amount = parse_amount(&config.benchmark.mode2_send_smoke_amount)?;
    ensure_signing_wallet(
        &config.modes.new_wallet_database,
        &sender_seed.seed_words,
        &config.seeds.wallet_password_env,
    )?;
    let start = Instant::now();
    let send = construct_sign_broadcast_one_sided(OneSidedSendRequest {
        db_path: &config.modes.new_wallet_database,
        password: &password,
        base_node_url: &config.network.base_node_http_url,
        recipient: &recipient_seed.address,
        amount,
        fee_rate: config.fee_rate()?,
        seconds_to_lock: config.timeouts.transaction_lock_secs,
        confirmation_window: config.benchmark.c_min,
        request_timeout: Duration::from_secs(30),
    })
    .await;
    let wall_ms = start.elapsed().as_millis();

    if let Some(mode) = profile.modes.get_mut("new_wallet")
        && let Some(cell) = mode.scenarios.get_mut("S1")
    {
        match send {
            Ok(outcome) => {
                cell.record_repetition(Repetition {
                    run: 1,
                    status: CellStatus::Ok,
                    wall_ms: Some(wall_ms),
                    success_count: 1,
                    failure_count: 0,
                    fee_microtari: Some(outcome.fee_microtari),
                    error: None,
                    metrics: None,
                });
                cell.notes.push(format!(
                    "Mode 2 compatibility smoke only: constructed, signed, persisted, and submitted one one-sided tx without retry middleware; tx_id={} amount={} recipient={} accepted={} is_synced={}",
                    outcome.tx_id,
                    config.benchmark.mode2_send_smoke_amount,
                    recipient_seed.address,
                    outcome.accepted,
                    outcome.is_synced
                ));
            }
            Err(error) => {
                cell.record_repetition(Repetition {
                    run: 1,
                    status: CellStatus::Failed,
                    wall_ms: Some(wall_ms),
                    success_count: 0,
                    failure_count: 1,
                    fee_microtari: None,
                    error: Some(format!("{error:#}")),
                    metrics: None,
                });
            }
        }
    }

    Ok(())
}

async fn annotate_mode2_live_scenarios(
    config: &Config,
    book: &AddressBook,
    profile: &mut ResultProfile,
) -> anyhow::Result<()> {
    let Some(sender_seed) = book.addresses.get(WalletRole::NewWallet.label()) else {
        return Ok(());
    };
    let Some(recipient_seed) = book.addresses.get(WalletRole::OldWallet.label()) else {
        return Ok(());
    };

    ensure_signing_wallet(
        &config.modes.new_wallet_database,
        &sender_seed.seed_words,
        &config.seeds.wallet_password_env,
    )?;

    let password = wallet_password(&config.seeds.wallet_password_env)?;
    let request = OwnedOneSidedSendRequest {
        db_path: config.modes.new_wallet_database.clone(),
        password: password.clone(),
        base_node_url: config.network.base_node_http_url.clone(),
        recipient: recipient_seed.address.clone(),
        amount: parse_amount(&config.benchmark.mode2_scenario_amount)?,
        fee_rate: config.fee_rate()?,
        seconds_to_lock: config.timeouts.transaction_lock_secs,
        confirmation_window: config.benchmark.c_min,
        request_timeout: Duration::from_secs(30),
    };

    let mut s1_request = request.clone();
    s1_request.recipient = sender_seed.address.clone();
    let mut s1 = run_mode2_s1_rounds(config, s1_request).await;
    let (s1_verification, s1_verification_attempts, s1_verification_wall_ms) =
        verify_mode2_transactions_until_confirmed(
            config,
            &config.modes.new_wallet_database,
            &s1.tx_ids,
            ScenarioName::S1,
        )
        .await?;
    s1.apply_mode2_verification(s1_verification);
    record_mode2_verification_loop_metrics(
        &mut s1,
        s1_verification_attempts,
        s1_verification_wall_ms,
    );
    record_mode2_send_summary(
        profile,
        ScenarioName::S1,
        &s1,
        vec![
            format!(
                "Mode 2 S1 live scenario: attempted {} self-directed multi-recipient one-sided txs of {} per output to {}; planned_rounds={} cap={}",
                s1.attempted,
                config.benchmark.mode2_scenario_amount,
                sender_seed.address,
                s1_round_plan(config, 0).len(),
                config.benchmark.mode2_live_max_s1_txs
            ),
            "Mode 2 S1 uses the minotari multi-recipient one-sided builder directly so the measured wallet builds the output set without shelling out or pre-partitioning UTXOs."
                .to_string(),
        ],
    );
    if config.benchmark.live_fresh_scan_cells {
        let checkpoint = checkpoint_from_mode2_summary(&s1, ScanCheckpoint::PostS1);
        run_library_checkpoint_scan_cells(
            config,
            profile,
            "new_wallet",
            Some(&sender_seed.seed_words),
            &[ScenarioName::S2, ScenarioName::S3],
            checkpoint,
        )
        .await?;
    }
    let mut s4 = run_s4_batches(config, request.clone()).await;
    let (s4_verification, s4_verification_attempts, s4_verification_wall_ms) =
        verify_mode2_transactions_until_confirmed(
            config,
            &config.modes.new_wallet_database,
            &s4.tx_ids,
            ScenarioName::S4,
        )
        .await?;
    s4.apply_mode2_verification(s4_verification);
    record_mode2_verification_loop_metrics(
        &mut s4,
        s4_verification_attempts,
        s4_verification_wall_ms,
    );
    record_mode2_send_summary(
        profile,
        ScenarioName::S4,
        &s4,
        vec![
            format!(
                "Mode 2 S4 live scenario: configured concurrent_batches={:?}, per-batch cap={}, S4_T_budget_secs={}",
                config.benchmark.concurrent_batches,
                config.benchmark.mode2_live_max_s4_batch,
                config.benchmark.s4_t_budget_secs
            ),
            "Each S4 batch is dispatched concurrently against the same wallet database; UTXO lock contention and send failures are benchmark signal."
                .to_string(),
        ],
    );
    if !s4.tx_ids.is_empty() {
        let note = wait_for_mode2_scan_advance(
            config,
            &config.modes.new_wallet_database,
            &password,
            "S4->S5",
        )
        .await?;
        append_mode2_note(profile, ScenarioName::S4, note);
    }

    let s5_attempts = capped_attempts(
        config.benchmark.s5_m,
        config.benchmark.mode2_live_max_s5_txs,
    );
    let s5_recipients = derive_distinct_recipient_pool(config.benchmark.s5_m)?
        .into_iter()
        .take(s5_attempts as usize)
        .collect::<Vec<_>>();
    let mut s5 =
        run_send_attempts_to_recipients_sequential("new_wallet/S5", s5_recipients, request).await;
    let s5_settle_note = if s5.tx_ids.is_empty() {
        None
    } else {
        Some(
            wait_for_mode2_scan_advance(
                config,
                &config.modes.new_wallet_database,
                &password,
                "S5 final",
            )
            .await?,
        )
    };
    let (s5_verification, s5_verification_attempts, s5_verification_wall_ms) =
        verify_mode2_transactions_until_confirmed(
            config,
            &config.modes.new_wallet_database,
            &s5.tx_ids,
            ScenarioName::S5,
        )
        .await?;
    s5.apply_mode2_verification(s5_verification);
    record_mode2_verification_loop_metrics(
        &mut s5,
        s5_verification_attempts,
        s5_verification_wall_ms,
    );
    record_mode2_send_summary(
        profile,
        ScenarioName::S5,
        &s5,
        vec![
            format!(
                "Mode 2 S5 individual-send arm: attempted {} of configured S5_M={} sends with S5_K={}; cap={}",
                s5.attempted,
                config.benchmark.s5_m,
                config.benchmark.s5_k,
                config.benchmark.mode2_live_max_s5_txs
            ),
            "Mode 2 has no batch endpoint at this layer; PP Mode 3 is responsible for the payment-batch arm."
                .to_string(),
        ],
    );
    if let Some(note) = s5_settle_note {
        append_mode2_note(profile, ScenarioName::S5, note);
    }
    if config.benchmark.live_fresh_scan_cells {
        let checkpoint = checkpoint_from_mode2_summary(&s5, ScanCheckpoint::PostS5Complete);
        run_library_checkpoint_scan_cells(
            config,
            profile,
            "new_wallet",
            Some(&sender_seed.seed_words),
            &[ScenarioName::S6, ScenarioName::S7],
            checkpoint,
        )
        .await?;
    }

    Ok(())
}

async fn verify_mode2_transactions_until_confirmed(
    config: &Config,
    db_path: &Path,
    tx_ids: &[String],
    scenario: ScenarioName,
) -> anyhow::Result<(Mode2VerificationResult, u32, u128)> {
    if tx_ids.is_empty() {
        return Ok((Mode2VerificationResult::default(), 0, 0));
    }

    let start = Instant::now();
    let timeout = config.timeout(config.timeouts.confirmation_secs);
    let mut attempts = 0u32;
    let client = base_node_http_client()?;

    loop {
        attempts = attempts.saturating_add(1);
        let verification =
            verify_mode2_transactions_with_client(config, db_path, tx_ids, scenario, &client)
                .await?;
        if mode2_verification_confirmed(&verification, tx_ids) || start.elapsed() >= timeout {
            return Ok((verification, attempts, start.elapsed().as_millis()));
        }

        let remaining = timeout.saturating_sub(start.elapsed());
        let sleep_for = Duration::from_secs(10).min(remaining);
        if sleep_for.is_zero() {
            return Ok((verification, attempts, start.elapsed().as_millis()));
        }
        settle_gate_pause(sleep_for).await;
    }
}

fn record_mode2_verification_loop_metrics(
    summary: &mut ScenarioSendSummary,
    attempts: u32,
    wall_ms: u128,
) {
    summary.extra_metrics.insert(
        "verification_loop".to_string(),
        serde_json::json!({
            "attempts": attempts,
            "wall_ms": wall_ms
        }),
    );
}

fn mode2_verification_confirmed(verification: &Mode2VerificationResult, tx_ids: &[String]) -> bool {
    !tx_ids.is_empty()
        && verification.observed_transactions.len() == tx_ids.len()
        && verification
            .observed_transactions
            .iter()
            .all(|tx| tx.confirmed)
}

async fn wait_for_mode2_scan_advance(
    config: &Config,
    db_path: &Path,
    password: &str,
    label: &str,
) -> anyhow::Result<String> {
    let client = base_node_http_client()?;
    let initial_scan_height = account_snapshot(db_path)
        .with_context(|| format!("mode2 settle gate {label} could not read wallet scan height"))?
        .max_height;
    let initial_tip_height =
        base_node_tip_height_with_client(&client, &config.network.base_node_http_url)
            .await
            .with_context(|| format!("mode2 settle gate {label} could not read base-node tip"))?;
    let target_tip_height = initial_tip_height.saturating_add(config.settle_wait_blocks());
    let timeout = config.timeout(config.timeouts.confirmation_secs);
    let start = Instant::now();
    let mut attempts = 0u32;
    let mut total_scan_wall_ms = 0u128;

    loop {
        attempts = attempts.saturating_add(1);
        let scan_wall_ms = scan_to_tip(
            db_path,
            password,
            &config.network.base_node_http_url,
            config.benchmark.scan_batch_size,
            config.benchmark.c_min,
        )
        .await
        .with_context(|| format!("mode2 settle gate {label} scan failed"))?;
        total_scan_wall_ms = total_scan_wall_ms.saturating_add(scan_wall_ms);
        let last_height = account_snapshot(db_path)
            .with_context(|| {
                format!("mode2 settle gate {label} could not read wallet scan height")
            })?
            .max_height;
        let tip_height =
            base_node_tip_height_with_client(&client, &config.network.base_node_http_url)
                .await
                .with_context(|| {
                    format!("mode2 settle gate {label} could not read base-node tip")
                })?;
        println!(
            "mode2 settle gate {label}: scanned_height={last_height} tip_height={tip_height} target_tip={target_tip_height}"
        );

        if tip_height >= target_tip_height {
            return Ok(format!(
                "Mode 2 settle gate {label}: scanned_height {initial_scan_height}->{last_height} tip_height {initial_tip_height}->{tip_height} target_tip={target_tip_height} attempts={attempts} scan_wall_ms={total_scan_wall_ms}"
            ));
        }
        if start.elapsed() >= timeout {
            bail!(
                "mode2 settle gate {label} timed out after {}s waiting for tip_height {tip_height} to reach target {target_tip_height}; scanned_height={last_height}",
                timeout.as_secs()
            );
        }

        let remaining = timeout.saturating_sub(start.elapsed());
        let sleep_for = Duration::from_secs(10).min(remaining);
        if !sleep_for.is_zero() {
            settle_gate_pause(sleep_for).await;
        }
    }
}

async fn settle_gate_pause(duration: Duration) {
    time::sleep_until(time::Instant::now() + duration).await;
}

async fn base_node_tip_height(base_node_url: &str) -> anyhow::Result<u64> {
    let client = base_node_http_client()?;
    base_node_tip_height_with_client(&client, base_node_url).await
}

async fn base_node_tip_height_with_client(
    client: &reqwest::Client,
    base_node_url: &str,
) -> anyhow::Result<u64> {
    let url = base_node_endpoint_url(base_node_url, "/get_tip_info")?;
    let response = client
        .get(url)
        .send()
        .await
        .context("requesting base-node tip info")?
        .error_for_status()
        .context("base-node tip info HTTP status")?
        .json::<serde_json::Value>()
        .await
        .context("decoding base-node tip info")?;
    response
        .pointer("/metadata/best_block_height")
        .and_then(serde_json::Value::as_u64)
        .with_context(|| format!("base-node tip info missing best_block_height: {response}"))
}

fn base_node_http_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .context("building base-node HTTP client")
}

fn base_node_endpoint_url(base_node_url: &str, path: &str) -> anyhow::Result<url::Url> {
    Ok(url::Url::parse(base_node_url)?.join(path)?)
}

fn append_mode2_note(profile: &mut ResultProfile, scenario: ScenarioName, note: String) {
    if let Some(cell) = profile
        .modes
        .get_mut("new_wallet")
        .and_then(|mode| mode.scenarios.get_mut(scenario.as_str()))
    {
        cell.notes.push(note);
    }
}

async fn run_mode2_s1_rounds(
    config: &Config,
    request: OwnedOneSidedSendRequest,
) -> ScenarioSendSummary {
    let mut total = ScenarioSendSummary::default();
    let start = Instant::now();
    let rounds = s1_round_plan(config, config.benchmark.mode2_live_max_s1_txs);
    let mut round_metrics = Vec::new();

    for round in rounds {
        let mut round_summary = ScenarioSendSummary {
            attempted: round.tx_count,
            ..ScenarioSendSummary::default()
        };
        let round_start = Instant::now();
        for tx_index in 1..=round.tx_count {
            println!(
                "new_wallet/S1 round {} tx {}/{} outputs={}",
                round.round_index, tx_index, round.tx_count, round.outputs_per_tx
            );
            let recipients = repeated_recipient(&request.recipient, round.outputs_per_tx as usize);
            let result = construct_sign_broadcast_one_sided_multi_recipient_owned(
                request.clone(),
                recipients,
            )
            .await;
            round_summary
                .construction_complete_ms
                .push(round_start.elapsed().as_millis());
            round_summary.record_attempt(tx_index, result);
        }
        round_summary.wall_ms = round_start.elapsed().as_millis();

        let mut settle_note = None;
        if !round_summary.tx_ids.is_empty() {
            match wait_for_mode2_scan_advance(
                config,
                &request.db_path,
                &request.password,
                &format!("S1 round {}", round.round_index),
            )
            .await
            {
                Ok(note) => settle_note = Some(note),
                Err(error) => {
                    round_summary.failure_count = round_summary.failure_count.saturating_add(1);
                    round_summary
                        .errors
                        .push(format!("mode2 S1 settle gate failed: {error:#}"));
                }
            }
        }

        round_metrics.push(serde_json::json!({
            "round_index": round.round_index,
            "fanout": round.fanout,
            "tx_count": round.tx_count,
            "outputs_per_tx": round.outputs_per_tx,
            "target_utxos_after": round.target_utxos_after,
            "success_count": round_summary.success_count,
            "failure_count": round_summary.failure_count,
            "settle_note": settle_note,
            "wall_ms": round_summary.wall_ms
        }));
        let has_failure = round_summary.failure_count > 0;
        total.add_batch(round.round_index, round_summary);
        if has_failure {
            break;
        }
    }

    total.wall_ms = start.elapsed().as_millis();
    total
        .extra_metrics
        .insert("rounds".to_string(), serde_json::json!(round_metrics));
    total
}

fn repeated_recipient(recipient: &str, count: usize) -> Vec<String> {
    let mut recipients = Vec::with_capacity(count);
    for _ in 0..count {
        recipients.push(recipient.to_string());
    }
    recipients
}

async fn run_s4_batches(config: &Config, request: OwnedOneSidedSendRequest) -> ScenarioSendSummary {
    let mut total = ScenarioSendSummary::default();
    let start = Instant::now();
    for configured_batch in &config.benchmark.concurrent_batches {
        let attempts = capped_attempts(*configured_batch, config.benchmark.mode2_live_max_s4_batch);
        let batch = run_send_attempts_concurrent(
            &format!("new_wallet/S4 batch {configured_batch}"),
            attempts,
            request.clone(),
        )
        .await;
        total.add_batch(*configured_batch, batch);
    }
    total.wall_ms = start.elapsed().as_millis();
    total
}

async fn run_send_attempts_to_recipients_sequential(
    label: &str,
    recipients: Vec<String>,
    request: OwnedOneSidedSendRequest,
) -> ScenarioSendSummary {
    let attempts = u32::try_from(recipients.len()).unwrap_or(u32::MAX);
    let mut summary = ScenarioSendSummary {
        attempted: attempts,
        ..ScenarioSendSummary::default()
    };
    let start = Instant::now();
    for (index, recipient) in recipients.into_iter().enumerate() {
        let attempt = u32::try_from(index + 1).unwrap_or(u32::MAX);
        println!("{label} attempt {attempt}/{attempts} dispatching");
        let mut request = request.clone();
        request.recipient = recipient;
        let result = construct_sign_broadcast_one_sided_owned(request).await;
        summary
            .construction_complete_ms
            .push(start.elapsed().as_millis());
        summary.record_attempt(attempt, result);
    }
    summary.wall_ms = start.elapsed().as_millis();
    summary
}

async fn run_send_attempts_concurrent(
    label: &str,
    attempts: u32,
    request: OwnedOneSidedSendRequest,
) -> ScenarioSendSummary {
    let mut summary = ScenarioSendSummary {
        attempted: attempts,
        ..ScenarioSendSummary::default()
    };
    let start = Instant::now();
    let mut join_set = JoinSet::new();
    for attempt in 1..=attempts {
        println!("{label} attempt {attempt}/{attempts} dispatching");
        let request = request.clone();
        let attempt_start = Instant::now();
        join_set.spawn(async move {
            let result = construct_sign_broadcast_one_sided_owned(request).await;
            (attempt, attempt_start.elapsed().as_millis(), result)
        });
    }
    while let Some(result) = join_set.join_next().await {
        match result {
            Ok((attempt, completed_ms, send)) => {
                summary.construction_complete_ms.push(completed_ms);
                summary.record_attempt(attempt, send);
            }
            Err(error) => summary.record_join_error(error.to_string()),
        }
    }
    summary.wall_ms = start.elapsed().as_millis();
    summary
}

fn record_mode2_send_summary(
    profile: &mut ResultProfile,
    scenario: ScenarioName,
    summary: &ScenarioSendSummary,
    mut notes: Vec<String>,
) {
    profile
        .chain_verification
        .verified_transactions
        .extend(summary.verified_transactions());

    let Some(mode) = profile.modes.get_mut("new_wallet") else {
        return;
    };
    let Some(cell) = mode.scenarios.get_mut(scenario.as_str()) else {
        return;
    };

    cell.record_repetition(mode2_send_repetition(summary, scenario));
    notes.push(summary.note(scenario));
    cell.notes.extend(notes);
}

#[cfg(test)]
fn refresh_recorded_mode2_send_summary(
    profile: &mut ResultProfile,
    scenario: ScenarioName,
    summary: &ScenarioSendSummary,
    note: String,
) {
    profile
        .chain_verification
        .verified_transactions
        .retain(|tx| !(tx.mode == "new_wallet" && tx.scenario == scenario.as_str()));
    profile
        .chain_verification
        .verified_transactions
        .extend(summary.verified_transactions());

    let Some(mode) = profile.modes.get_mut("new_wallet") else {
        return;
    };
    let Some(cell) = mode.scenarios.get_mut(scenario.as_str()) else {
        return;
    };

    let repetition = mode2_send_repetition(summary, scenario);
    if let Some(existing) = cell.repetitions.last_mut() {
        *existing = repetition;
        cell.refresh_summary();
    } else {
        cell.record_repetition(repetition);
    }
    cell.notes.push(note);
}

fn mode2_send_repetition(summary: &ScenarioSendSummary, scenario: ScenarioName) -> Repetition {
    let verified = summary.verified_transactions();
    let verification_complete = summary.tx_ids.is_empty() || !verified.is_empty();
    let all_verified_ok = verified.iter().all(|tx| tx.confirmed);

    let status = if summary.failure_count == 0 && verification_complete && all_verified_ok {
        CellStatus::Ok
    } else {
        CellStatus::Failed
    };

    Repetition {
        run: 1,
        status,
        wall_ms: Some(summary.wall_ms),
        success_count: summary.success_count,
        failure_count: summary.failure_count,
        fee_microtari: Some(summary.fee_microtari),
        error: summary.error_note().or_else(|| {
            (!all_verified_ok)
                .then_some("one or more tx_ids did not verify as terminal-ok".to_string())
                .or_else(|| {
                    (!verification_complete).then_some(
                        "tx_ids were produced but chain verification returned no rows".to_string(),
                    )
                })
        }),
        metrics: Some(summary.metrics(scenario)),
    }
}

fn capped_attempts(planned: u32, cap: u32) -> u32 {
    if cap == 0 { planned } else { planned.min(cap) }
}

async fn annotate_mode3_payment_processor(
    config: &Config,
    book: &AddressBook,
    profile: &mut ResultProfile,
) -> anyhow::Result<()> {
    let Some(pp_seed) = book.addresses.get(WalletRole::PaymentProcessor.label()) else {
        return Ok(());
    };
    let Some(recipient_seed) = book.addresses.get(WalletRole::OldWallet.label()) else {
        return Ok(());
    };

    let start = Instant::now();
    let topology = start_mode3_topology(config, pp_seed).await;
    match topology {
        Ok(context) => {
            record_mode3_s0(config, profile, &context, start.elapsed().as_millis());
            run_mode3_send_cells(
                config,
                profile,
                pp_seed,
                recipient_seed.address.clone(),
                &context,
            )
            .await?;
        }
        Err(error) => {
            record_mode3_startup_failure(profile, start.elapsed().as_millis(), error);
        }
    }
    Ok(())
}

async fn start_mode3_topology(
    config: &Config,
    pp_seed: &crate::seeds::SeedMaterial,
) -> anyhow::Result<Mode3TopologyContext> {
    let password = wallet_password(&config.seeds.wallet_password_env)?;
    ensure_payment_receiver_wallet(config, pp_seed, &password)?;
    payment_processor::ensure_console_wallet_base(config, pp_seed, &password).await?;
    let unlocked = payment_processor::unlock_stale_payment_receiver_locks(config)?;
    if unlocked > 0 {
        println!(
            "mode3 payment receiver startup cleanup unlocked {unlocked} stale lock request(s)"
        );
    }

    let mut payment_receiver = payment_processor::start_payment_receiver(config, &password).await?;
    payment_processor::wait_for_payment_receiver(config, &mut payment_receiver).await?;
    let required_balance = config.a_fund()?.0;
    let receiver_balance = payment_processor::wait_for_payment_receiver_balance(
        config,
        &mut payment_receiver,
        required_balance,
    )
    .await?;

    let env = payment_processor::build_env(config, pp_seed);
    let mut payment_processor = payment_processor::start_payment_processor(config, &env).await?;
    let version =
        payment_processor::wait_for_payment_processor(config, &mut payment_processor).await?;

    Ok(Mode3TopologyContext {
        _payment_receiver: payment_receiver,
        _payment_processor: payment_processor,
        client: PaymentProcessorClient::new(format!(
            "http://{}",
            config.modes.payment_processor_listen
        )),
        receiver_balance,
        processor_version: version.version,
        worker_sleep_secs: config.benchmark.mode3_worker_sleep_secs,
        receiver_birthday: mode3_receiver_birthday(pp_seed),
    })
}

fn ensure_payment_receiver_wallet(
    config: &Config,
    pp_seed: &crate::seeds::SeedMaterial,
    password: &str,
) -> anyhow::Result<()> {
    let db_path = payment_processor::payment_receiver_db_path(config);
    if db_path.exists() {
        return Ok(());
    }
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let birthday = mode3_receiver_birthday(pp_seed);
    init_with_view_key(
        &pp_seed.private_view_key_hex,
        &pp_seed.public_spend_key_hex,
        password,
        &db_path,
        birthday,
        Some("default"),
    )
    .context("initializing Mode 3 payment receiver view wallet")
}

fn mode3_receiver_birthday(pp_seed: &crate::seeds::SeedMaterial) -> u16 {
    if pp_seed.birthday == 0 {
        current_birthday()
    } else {
        pp_seed.birthday
    }
}

fn record_mode3_s0(
    config: &Config,
    profile: &mut ResultProfile,
    context: &Mode3TopologyContext,
    wall_ms: u128,
) {
    let available =
        amount_field_as_microtari(&context.receiver_balance, "available").unwrap_or_default();
    let expected = config.a_fund().map(|amount| amount.0).unwrap_or_default();
    let (status, success_count, failure_count, error) = if available >= expected {
        (CellStatus::Ok, 1, 0, None)
    } else {
        (
            CellStatus::Failed,
            0,
            1,
            Some(format!(
                "payment receiver available balance {available} µT is below configured A_fund {expected} µT"
            )),
        )
    };

    let Some(mode) = profile.modes.get_mut("payment_processor") else {
        return;
    };
    let Some(cell) = mode.scenarios.get_mut("S0") else {
        return;
    };
    cell.record_repetition(Repetition {
        run: 1,
        status,
        wall_ms: Some(wall_ms),
        success_count,
        failure_count,
        fee_microtari: None,
        error,
        metrics: None,
    });
    cell.notes.push(format!(
        "Mode 3 topology started real minotari payment receiver plus minotari_payment_processor version {}; receiver_balance={}; receiver_birthday={}; worker_sleep_secs={}",
        context.processor_version,
        context.receiver_balance,
        context.receiver_birthday,
        context.worker_sleep_secs
    ));
    cell.notes.push(format!(
        "payment_receiver_db={} payment_processor_db={} console_wallet_base={}",
        payment_processor::payment_receiver_db_path(config).display(),
        payment_processor::payment_processor_db_path(config).display(),
        payment_processor::console_wallet_base_path(config).display()
    ));
    if let Some(funding) = &config.funding.payment_processor {
        cell.notes.push(format!(
            "funding tx_id={} height={} amount={}",
            funding.tx_id, funding.height, funding.amount
        ));
    }
}

fn record_mode3_startup_failure(profile: &mut ResultProfile, wall_ms: u128, error: anyhow::Error) {
    let Some(mode) = profile.modes.get_mut("payment_processor") else {
        return;
    };
    for scenario in [
        ScenarioName::S0,
        ScenarioName::S1,
        ScenarioName::S4,
        ScenarioName::S5,
    ] {
        let Some(cell) = mode.scenarios.get_mut(scenario.as_str()) else {
            continue;
        };
        cell.record_repetition(Repetition {
            run: 1,
            status: CellStatus::Failed,
            wall_ms: Some(wall_ms),
            success_count: 0,
            failure_count: 1,
            fee_microtari: None,
            error: Some(format!("{error:#}")),
            metrics: None,
        });
        cell.notes
            .push("Mode 3 topology startup failed before scenario dispatch".to_string());
    }
}

async fn run_mode3_send_cells(
    config: &Config,
    profile: &mut ResultProfile,
    pp_seed: &crate::seeds::SeedMaterial,
    recipient: String,
    context: &Mode3TopologyContext,
) -> anyhow::Result<()> {
    let amount = parse_amount(&config.benchmark.mode3_scenario_amount)?;
    let s1_rounds = s1_round_plan(config, config.benchmark.mode3_live_max_s1_batches);
    let s1 = run_pp_recipient_batches_sequential(
        config,
        context,
        "payment_processor/S1",
        ScenarioName::S1,
        s1_pp_recipient_batches(&s1_rounds, &recipient),
        amount,
    )
    .await;
    let mut s1_extra = serde_json::Map::new();
    s1_extra.insert("rounds".to_string(), s1_round_metrics(&s1_rounds));
    let s1 = s1.with_extra_metrics(s1_extra);
    record_pp_summary(
        profile,
        ScenarioName::S1,
        &s1,
        vec![format!(
            "Mode 3 S1 drove /v1/payment-batches through real PP topology as a batch-shape analogue to doubling/fanout rounds; attempted_batches={} attempted_payments={} amount={} cap={}",
            s1.attempted_batches,
            s1.attempted_payments,
            config.benchmark.mode3_scenario_amount,
            config.benchmark.mode3_live_max_s1_batches
        )],
    );
    if config.benchmark.live_fresh_scan_cells {
        let checkpoint = checkpoint_from_pp_summary(&s1, ScanCheckpoint::PostS1);
        run_library_checkpoint_scan_cells(
            config,
            profile,
            "payment_processor",
            Some(&pp_seed.seed_words),
            &[ScenarioName::S2, ScenarioName::S3],
            checkpoint,
        )
        .await?;
    }

    let s4 = run_pp_s4_batches(config, context, &recipient, amount).await;
    record_pp_summary(
        profile,
        ScenarioName::S4,
        &s4,
        vec![format!(
            "Mode 3 S4 dispatched configured concurrent_batches={:?} through real PP /v1/payment-batches; per-batch cap={}",
            config.benchmark.concurrent_batches, config.benchmark.mode3_live_max_s4_batch
        )],
    );

    let s5_items = capped_attempts(
        config.benchmark.s5_m,
        config.benchmark.mode3_live_max_s5_items,
    );
    let s5_recipients = derive_distinct_recipient_pool(config.benchmark.s5_m)?
        .into_iter()
        .take(s5_items as usize)
        .collect::<Vec<_>>();
    let s5 = run_pp_recipient_batches_sequential(
        config,
        context,
        "payment_processor/S5",
        ScenarioName::S5,
        recipient_batches(s5_recipients, config.benchmark.s5_k),
        amount,
    )
    .await;
    record_pp_summary(
        profile,
        ScenarioName::S5,
        &s5,
        vec![format!(
            "Mode 3 S5 payment-batch arm used one /v1/payment-batches request with items={} of configured S5_M={} and S5_K={}; cap={}",
            s5_items,
            config.benchmark.s5_m,
            config.benchmark.s5_k,
            config.benchmark.mode3_live_max_s5_items
        )],
    );
    if config.benchmark.live_fresh_scan_cells {
        let checkpoint = checkpoint_from_pp_summary(&s5, ScanCheckpoint::PostS5Complete);
        run_library_checkpoint_scan_cells(
            config,
            profile,
            "payment_processor",
            Some(&pp_seed.seed_words),
            &[ScenarioName::S6, ScenarioName::S7],
            checkpoint,
        )
        .await?;
    }

    Ok(())
}

async fn run_pp_s4_batches(
    config: &Config,
    context: &Mode3TopologyContext,
    recipient: &str,
    amount: MicroMinotari,
) -> PpScenarioSummary {
    let start = Instant::now();
    let mut total = PpScenarioSummary::default();
    for configured_batch in &config.benchmark.concurrent_batches {
        let attempts = capped_attempts(*configured_batch, config.benchmark.mode3_live_max_s4_batch);
        let batch = run_pp_batches_concurrent(
            config,
            context,
            &format!("payment_processor/S4 batch {configured_batch}"),
            ScenarioName::S4,
            attempts,
            1,
            recipient,
            amount,
        )
        .await;
        total.add_batch(*configured_batch, batch);
    }
    total.wall_ms = start.elapsed().as_millis();
    total.observe_db(config).await;
    total
}

async fn run_pp_recipient_batches_sequential(
    config: &Config,
    context: &Mode3TopologyContext,
    label: &str,
    scenario: ScenarioName,
    recipient_batches: Vec<Vec<String>>,
    amount: MicroMinotari,
) -> PpScenarioSummary {
    let attempted_batches = u32::try_from(recipient_batches.len()).unwrap_or(u32::MAX);
    let attempted_payments = recipient_batches
        .iter()
        .map(|batch| u32::try_from(batch.len()).unwrap_or(u32::MAX))
        .fold(0u32, u32::saturating_add);
    let mut summary = PpScenarioSummary {
        attempted_batches,
        attempted_payments,
        ..PpScenarioSummary::default()
    };
    let start = Instant::now();
    for (index, recipients) in recipient_batches.into_iter().enumerate() {
        let batch_index = u32::try_from(index + 1).unwrap_or(u32::MAX);
        println!("{label} batch {batch_index}/{attempted_batches} dispatching");
        let result = submit_pp_batch_to_recipients(
            &context.client,
            scenario,
            batch_index,
            recipients,
            amount,
        )
        .await;
        summary
            .construction_complete_ms
            .push(start.elapsed().as_millis());
        summary.record_batch(batch_index, result);
    }
    summary.wall_ms = start.elapsed().as_millis();
    summary.observe_db(config).await;
    summary
}

#[allow(clippy::too_many_arguments)]
async fn run_pp_batches_concurrent(
    config: &Config,
    context: &Mode3TopologyContext,
    label: &str,
    scenario: ScenarioName,
    batch_count: u32,
    items_per_batch: u32,
    recipient: &str,
    amount: MicroMinotari,
) -> PpScenarioSummary {
    let mut summary = PpScenarioSummary {
        attempted_batches: batch_count,
        attempted_payments: batch_count.saturating_mul(items_per_batch),
        ..PpScenarioSummary::default()
    };
    let start = Instant::now();
    let mut join_set = JoinSet::new();
    for batch_index in 1..=batch_count {
        println!("{label} batch {batch_index}/{batch_count} dispatching");
        let context = context.clone_for_task();
        let recipient = recipient.to_string();
        let batch_start = Instant::now();
        join_set.spawn(async move {
            let result = submit_pp_batch(
                &context.client,
                scenario,
                batch_index,
                items_per_batch,
                &recipient,
                amount,
            )
            .await;
            (batch_index, batch_start.elapsed().as_millis(), result)
        });
    }
    while let Some(result) = join_set.join_next().await {
        match result {
            Ok((batch_index, completed_ms, send)) => {
                summary.construction_complete_ms.push(completed_ms);
                summary.record_batch(batch_index, send);
            }
            Err(error) => summary.record_join_error(error.to_string()),
        }
    }
    summary.wall_ms = start.elapsed().as_millis();
    summary.observe_db(config).await;
    summary
}

async fn submit_pp_batch(
    client: &PaymentProcessorClient,
    scenario: ScenarioName,
    batch_index: u32,
    items_per_batch: u32,
    recipient: &str,
    amount: MicroMinotari,
) -> anyhow::Result<PpBatchSubmission> {
    let items = (1..=items_per_batch)
        .map(|_| recipient.to_string())
        .collect::<Vec<_>>();
    submit_pp_batch_to_recipients(client, scenario, batch_index, items, amount).await
}

async fn submit_pp_batch_to_recipients(
    client: &PaymentProcessorClient,
    scenario: ScenarioName,
    batch_index: u32,
    recipients: Vec<String>,
    amount: MicroMinotari,
) -> anyhow::Result<PpBatchSubmission> {
    let amount = i64::try_from(amount.0).context("mode3 payment amount exceeds i64")?;
    let items = recipients
        .into_iter()
        .enumerate()
        .map(|(item_index, recipient_address)| {
            let payment_index = item_index + 1;
            BulkPaymentItem {
                client_id: format!(
                    "bench-{}-{}-{}-{}",
                    scenario.as_str().to_lowercase(),
                    chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
                    batch_index,
                    payment_index
                ),
                recipient_address,
                amount,
                payment_id: Some(format!(
                    "wallet-bench-{}-{batch_index}-{payment_index}",
                    scenario.as_str()
                )),
            }
        })
        .collect::<Vec<_>>();
    let response = client
        .create_payment_batch(&BulkPaymentRequest {
            account_name: "default".to_string(),
            items,
        })
        .await?;
    let batch_id = response
        .get("batch_id")
        .and_then(|value| value.as_str())
        .context("PP batch response missing batch_id")?
        .to_string();
    let payment_ids = response
        .get("payments")
        .and_then(|value| value.as_array())
        .context("PP batch response missing payments")?
        .iter()
        .filter_map(|payment| {
            payment
                .get("payment_id")
                .and_then(|value| value.as_str())
                .map(ToString::to_string)
        })
        .collect::<Vec<_>>();
    Ok(PpBatchSubmission {
        batch_id,
        payment_ids,
        raw_response: response,
    })
}

fn recipient_batches(recipients: Vec<String>, batch_size: u32) -> Vec<Vec<String>> {
    let target = usize::try_from(batch_size.max(1)).unwrap_or(1);
    let mut batches = Vec::new();
    let mut current = Vec::new();
    for recipient in recipients {
        current.push(recipient);
        if current.len() == target {
            batches.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

fn s1_pp_recipient_batches(rounds: &[S1RoundPlan], recipient: &str) -> Vec<Vec<String>> {
    let mut batches = Vec::new();
    for round in rounds {
        for _ in 0..round.tx_count {
            batches.push(repeated_recipient(recipient, round.outputs_per_tx as usize));
        }
    }
    batches
}

fn s1_round_metrics(rounds: &[S1RoundPlan]) -> serde_json::Value {
    serde_json::Value::Array(
        rounds
            .iter()
            .map(|round| {
                serde_json::json!({
                    "round_index": round.round_index,
                    "fanout": round.fanout,
                    "tx_count": round.tx_count,
                    "outputs_per_tx": round.outputs_per_tx,
                    "target_utxos_after": round.target_utxos_after,
                })
            })
            .collect(),
    )
}

fn record_pp_summary(
    profile: &mut ResultProfile,
    scenario: ScenarioName,
    summary: &PpScenarioSummary,
    mut notes: Vec<String>,
) {
    let verified = summary.verified_transactions(scenario);
    let confirmed_batch_count = verified.len();
    let accepted_batch_count = usize::try_from(summary.accepted_batches).unwrap_or(usize::MAX);
    let observation_complete =
        summary.accepted_batches == 0 || confirmed_batch_count >= accepted_batch_count;
    let all_verified_ok = verified.iter().all(|tx| tx.confirmed);
    profile
        .chain_verification
        .verified_transactions
        .extend(verified);
    let Some(mode) = profile.modes.get_mut("payment_processor") else {
        return;
    };
    let Some(cell) = mode.scenarios.get_mut(scenario.as_str()) else {
        return;
    };
    let status = if summary.blocked_upstream {
        CellStatus::BlockedUpstream
    } else if summary.failed_batches == 0 && observation_complete && all_verified_ok {
        CellStatus::Ok
    } else {
        CellStatus::Failed
    };
    cell.record_repetition(Repetition {
        run: 1,
        status,
        wall_ms: Some(summary.wall_ms),
        success_count: summary.accepted_batches,
        failure_count: summary.failed_batches,
        fee_microtari: None,
        error: summary.error_note().or_else(|| {
            (!all_verified_ok)
                .then_some("one or more PP batches did not verify as terminal-ok".to_string())
                .or_else(|| {
                    (!observation_complete).then_some(
                        "PP batches were accepted but payment_processor_db_observed returned no confirmed rows"
                            .to_string(),
                    )
                })
        }),
        metrics: Some(summary.metrics(scenario)),
    });
    notes.push(
        "verification_source=payment_processor_db_observed; pending PP batches stay in metrics/notes and are not emitted as confirmed chain-verification rows"
            .to_string(),
    );
    notes.push(summary.note(scenario));
    cell.notes.extend(notes);
}

async fn run_library_checkpoint_scan_cells(
    config: &Config,
    profile: &mut ResultProfile,
    mode: &str,
    funded_seed_words: Option<&str>,
    scenarios: &[ScenarioName],
    checkpoint: ScanCheckpoint,
) -> anyhow::Result<()> {
    let birthday = current_birthday();
    for scenario in scenarios {
        let spec = FreshScanSpec {
            scenario: *scenario,
            wallet_state: fresh_scan_wallet_state(*scenario, birthday),
            checkpoint,
        };
        run_library_fresh_scan_for_cell(config, profile, mode, funded_seed_words, spec).await?;
    }
    Ok(())
}

async fn run_library_fresh_scan_for_cell(
    config: &Config,
    profile: &mut ResultProfile,
    mode: &str,
    funded_seed_words: Option<&str>,
    spec: FreshScanSpec,
) -> anyhow::Result<()> {
    let Some(cell) = profile
        .modes
        .get_mut(mode)
        .and_then(|mode_profile| mode_profile.scenarios.get_mut(spec.scenario.as_str()))
    else {
        return Ok(());
    };
    if !spec.checkpoint.runnable() {
        record_blocked_checkpoint_scan(cell, spec);
        return Ok(());
    }
    run_library_fresh_scan_cell(config, mode, funded_seed_words, spec, cell).await
}

async fn run_mode1_checkpoint_scan_cells(
    config: &Config,
    profile: &mut ResultProfile,
    old_seed: &crate::seeds::SeedMaterial,
    scenarios: &[ScenarioName],
    checkpoint: ScanCheckpoint,
) -> anyhow::Result<()> {
    let birthday = mode1_wallet_birthday(old_seed);
    for scenario in scenarios {
        let spec = FreshScanSpec {
            scenario: *scenario,
            wallet_state: fresh_scan_wallet_state(*scenario, birthday),
            checkpoint,
        };
        run_mode1_fresh_scan_for_cell(config, profile, old_seed, spec).await?;
    }
    Ok(())
}

async fn run_mode1_fresh_scan_for_cell(
    config: &Config,
    profile: &mut ResultProfile,
    old_seed: &crate::seeds::SeedMaterial,
    spec: FreshScanSpec,
) -> anyhow::Result<()> {
    let Some(cell) = profile
        .modes
        .get_mut("old_wallet")
        .and_then(|mode_profile| mode_profile.scenarios.get_mut(spec.scenario.as_str()))
    else {
        return Ok(());
    };
    if !spec.checkpoint.runnable() {
        record_blocked_checkpoint_scan(cell, spec);
        return Ok(());
    }
    run_mode1_fresh_scan_cell(config, old_seed, spec, cell).await
}

fn record_blocked_checkpoint_scan(cell: &mut ScenarioCell, spec: FreshScanSpec) {
    let note = spec.checkpoint.blocked_note(spec.scenario);
    cell.notes.push(note.clone());
    cell.record_repetition(Repetition {
        run: 1,
        status: CellStatus::Failed,
        wall_ms: None,
        success_count: 0,
        failure_count: 1,
        fee_microtari: None,
        error: Some(note),
        metrics: Some(serde_json::json!({
            "blocked_prerequisite": true,
            "scan_checkpoint": spec.checkpoint.label()
        })),
    });
}

async fn run_library_fresh_scan_cell(
    config: &Config,
    mode: &str,
    funded_seed_words: Option<&str>,
    spec: FreshScanSpec,
    cell: &mut ScenarioCell,
) -> anyhow::Result<()> {
    for run in 1..=config.benchmark.scan_repetitions {
        println!(
            "live scan {mode}/{} run {run}/{} birthday={} starting",
            spec.scenario.as_str(),
            config.benchmark.scan_repetitions,
            spec.birthday()
        );
        let scan = run_library_fresh_scan(config, mode, spec, run, funded_seed_words).await;
        match scan {
            Ok(measurement) => {
                println!(
                    "live scan {mode}/{} run {run} ok: wall_ms={} max_height={} available_microtari={}",
                    spec.scenario.as_str(),
                    measurement.wall_ms,
                    measurement.max_height,
                    measurement.available_microtari
                );
                cell.record_repetition(Repetition {
                    run,
                    status: CellStatus::Ok,
                    wall_ms: Some(measurement.wall_ms),
                    success_count: 1,
                    failure_count: 0,
                    fee_microtari: None,
                    error: None,
                    metrics: Some(measurement.metrics(mode, spec)),
                });
                cell.notes.push(measurement.note());
            }
            Err(error) => {
                println!(
                    "live scan {mode}/{} run {run} failed: {error:#}",
                    spec.scenario.as_str()
                );
                cell.record_repetition(Repetition {
                    run,
                    status: CellStatus::Failed,
                    wall_ms: None,
                    success_count: 0,
                    failure_count: 1,
                    fee_microtari: None,
                    error: Some(format!("{error:#}")),
                    metrics: None,
                });
            }
        }
    }

    Ok(())
}

async fn run_mode1_fresh_scan_cell(
    config: &Config,
    old_seed: &crate::seeds::SeedMaterial,
    spec: FreshScanSpec,
    cell: &mut ScenarioCell,
) -> anyhow::Result<()> {
    for run in 1..=config.benchmark.scan_repetitions {
        println!(
            "live scan old_wallet/{} run {run}/{} birthday={} starting",
            spec.scenario.as_str(),
            config.benchmark.scan_repetitions,
            spec.birthday()
        );
        let scan = run_mode1_fresh_scan(config, old_seed, spec, run).await;
        match scan {
            Ok(measurement) => {
                println!(
                    "live scan old_wallet/{} run {run} ok: wall_ms={} max_height={} available_microtari={}",
                    spec.scenario.as_str(),
                    measurement.wall_ms,
                    measurement.max_height,
                    measurement.available_microtari
                );
                cell.record_repetition(Repetition {
                    run,
                    status: CellStatus::Ok,
                    wall_ms: Some(measurement.wall_ms),
                    success_count: 1,
                    failure_count: 0,
                    fee_microtari: None,
                    error: None,
                    metrics: Some(measurement.metrics("old_wallet", spec)),
                });
                cell.notes.push(measurement.note());
            }
            Err(error) => {
                println!(
                    "live scan old_wallet/{} run {run} failed: {error:#}",
                    spec.scenario.as_str()
                );
                cell.record_repetition(Repetition {
                    run,
                    status: CellStatus::Failed,
                    wall_ms: None,
                    success_count: 0,
                    failure_count: 1,
                    fee_microtari: None,
                    error: Some(format!("{error:#}")),
                    metrics: None,
                });
            }
        }
    }

    Ok(())
}

async fn run_library_fresh_scan(
    config: &Config,
    mode: &str,
    spec: FreshScanSpec,
    run: u32,
    funded_seed_words: Option<&str>,
) -> anyhow::Result<ScanMeasurement> {
    let db_path = fresh_scan_db_path(config, mode, spec, run);
    reset_sqlite_files(&db_path)?;

    let password = wallet_password(&config.seeds.wallet_password_env)?;
    let seed = spec.seed(funded_seed_words)?;
    init_with_seed_words(seed, &password, &db_path, Some("default"))
        .context("initializing fresh scan wallet")?;

    let tip_start = base_node_tip_height(&config.network.base_node_http_url)
        .await
        .ok();
    let wall_ms = scan_to_tip(
        &db_path,
        &password,
        &config.network.base_node_http_url,
        config.benchmark.scan_batch_size,
        config.benchmark.c_min,
    )
    .await?;
    let tip_end = base_node_tip_height(&config.network.base_node_http_url)
        .await
        .ok();
    let account = account_snapshot(&db_path)?;
    let detected_outputs = detected_output_count(&db_path).unwrap_or_default();

    Ok(ScanMeasurement {
        wall_ms,
        birthday: spec.birthday(),
        max_height: account.max_height,
        available_microtari: account.available_microtari,
        tip_start,
        tip_end,
        detected_outputs,
    })
}

async fn run_mode1_fresh_scan(
    config: &Config,
    old_seed: &crate::seeds::SeedMaterial,
    spec: FreshScanSpec,
    run: u32,
) -> anyhow::Result<ScanMeasurement> {
    let base_path = fresh_console_base_path(config, spec, run);
    reset_dir(&base_path)?;
    let grpc_address = mode1_scan_grpc_address(&config.modes.old_wallet_grpc_address, spec, run)?;
    let grpc_bind = grpc_bind_multiaddr(&grpc_address)?;
    let password = wallet_password(&config.seeds.wallet_password_env)?;
    let seed_words = match spec.wallet_state {
        FreshScanWalletState::EmptyGenesis => {
            let mut seed = CipherSeed::random();
            seed.change_birthday(0);
            crate::seeds::seed_words_from_seed(&seed)?
        }
        FreshScanWalletState::FundedGenesis | FreshScanWalletState::FundedBirthday { .. } => {
            seed_words_with_birthday(&old_seed.seed_words, spec.birthday())?
        }
    };
    let config_path = base_path.join("config/config.toml");
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir_all("logs")?;
    let log_stem = format!(
        "mode1-scan-{}-run{}-birthday{}",
        spec.scenario.as_str().to_lowercase(),
        run,
        spec.birthday()
    );
    let stdout_path = PathBuf::from(format!("logs/{log_stem}.stdout.log"));
    let stderr_path = PathBuf::from(format!("logs/{log_stem}.stderr.log"));
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stdout_path)?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stderr_path)?;

    let tip_start = base_node_tip_height(&config.network.base_node_http_url)
        .await
        .ok();
    let start = Instant::now();
    let mut command = Command::new(&config.paths.minotari_console_wallet);
    command
        .env("MINOTARI_WALLET_SEED_WORDS", seed_words)
        .env("MINOTARI_WALLET_PASSWORD", &password)
        .arg("--base-path")
        .arg(&base_path)
        .arg("--config")
        .arg(&config_path)
        .arg("--network")
        .arg("Esmeralda")
        .arg("--non-interactive-mode")
        .arg("--recovery")
        .arg("--grpc-enabled")
        .arg("--grpc-address")
        .arg(&grpc_bind)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    let mut process = Mode1ConsoleProcess {
        child: command.spawn().context("spawning scan console wallet")?,
        stdout_path,
        stderr_path,
    };
    let mut client = wait_for_mode1_grpc_address(config, &mut process, &grpc_address).await?;
    let max_height = wait_for_mode1_scan_to_tip(
        &mut process,
        &mut client,
        tip_start,
        config.timeout(config.timeouts.startup_secs),
    )
    .await?;
    let balance = client
        .get_balance(grpc::GetBalanceRequest { payment_id: None })
        .await?
        .into_inner();
    let detected_outputs = mode1_unspent_count(&mut client).await.unwrap_or_default();
    let tip_end = base_node_tip_height(&config.network.base_node_http_url)
        .await
        .ok();

    Ok(ScanMeasurement {
        wall_ms: start.elapsed().as_millis(),
        birthday: spec.birthday(),
        max_height,
        available_microtari: balance.available_balance,
        tip_start,
        tip_end,
        detected_outputs,
    })
}

fn fresh_scan_db_path(config: &Config, mode: &str, spec: FreshScanSpec, run: u32) -> PathBuf {
    config.paths.data_dir.join("fresh-scans").join(format!(
        "{}-{}-run{}-birthday{}.db",
        mode,
        spec.scenario.as_str().to_lowercase(),
        run,
        spec.birthday()
    ))
}

fn fresh_console_base_path(config: &Config, spec: FreshScanSpec, run: u32) -> PathBuf {
    config.paths.data_dir.join("fresh-scans").join(format!(
        "old-wallet-{}-run{}-birthday{}",
        spec.scenario.as_str().to_lowercase(),
        run,
        spec.birthday()
    ))
}

fn reset_dir(path: &Path) -> anyhow::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    fs::create_dir_all(path)?;
    Ok(())
}

fn reset_sqlite_files(db_path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)?;
    }
    for path in [
        db_path.to_path_buf(),
        PathBuf::from(format!("{}-wal", db_path.display())),
        PathBuf::from(format!("{}-shm", db_path.display())),
    ] {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn detected_output_count(db_path: &Path) -> anyhow::Result<u64> {
    let conn = Connection::open(db_path)?;
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM outputs", [], |row| row.get(0))?;
    Ok(u64::try_from(count).unwrap_or_default())
}

pub fn account_balance(db_path: &Path) -> anyhow::Result<serde_json::Value> {
    let pool = init_db(db_path.to_path_buf())?;
    let conn = pool.get()?;
    let account = get_accounts(&conn, None)?
        .into_iter()
        .next()
        .context("no account")?;
    let balance = get_balance(&conn, account.id)?;
    Ok(serde_json::to_value(balance)?)
}

fn account_snapshot(db_path: &Path) -> anyhow::Result<AccountSnapshot> {
    let pool = init_db(db_path.to_path_buf())?;
    let conn = pool.get()?;
    let account = get_accounts(&conn, None)?
        .into_iter()
        .next()
        .context("no account")?;
    let balance = get_balance(&conn, account.id)?;
    let max_height = get_latest_scanned_tip_block_by_account(&conn, account.id)?
        .map(|tip| tip.height)
        .unwrap_or_default();
    let balance = serde_json::to_value(balance)?;
    let available_microtari = amount_field_as_microtari(&balance, "available").unwrap_or_default();

    Ok(AccountSnapshot {
        max_height,
        available_microtari,
    })
}

fn amount_field_as_microtari(balance: &serde_json::Value, key: &str) -> Option<u64> {
    match balance.get(key)? {
        serde_json::Value::Number(number) => number.as_u64(),
        serde_json::Value::String(value) => {
            if let Ok(raw) = value.parse::<u64>() {
                return Some(raw);
            }
            parse_amount(value).ok().map(|amount| amount.0)
        }
        serde_json::Value::Object(map) => map
            .get("value")
            .and_then(|value| value.as_u64())
            .or_else(|| map.get("microtari").and_then(|value| value.as_u64())),
        _ => None,
    }
}

pub struct OneSidedSendRequest<'a> {
    pub db_path: &'a Path,
    pub password: &'a str,
    pub base_node_url: &'a str,
    pub recipient: &'a str,
    pub amount: MicroMinotari,
    pub fee_rate: MicroMinotari,
    pub seconds_to_lock: u64,
    pub confirmation_window: u64,
    pub request_timeout: Duration,
}

#[derive(Clone)]
struct OwnedOneSidedSendRequest {
    db_path: PathBuf,
    password: String,
    base_node_url: String,
    recipient: String,
    amount: MicroMinotari,
    fee_rate: MicroMinotari,
    seconds_to_lock: u64,
    confirmation_window: u64,
    request_timeout: Duration,
}

impl OwnedOneSidedSendRequest {
    fn as_borrowed(&self) -> OneSidedSendRequest<'_> {
        OneSidedSendRequest {
            db_path: &self.db_path,
            password: &self.password,
            base_node_url: &self.base_node_url,
            recipient: &self.recipient,
            amount: self.amount,
            fee_rate: self.fee_rate,
            seconds_to_lock: self.seconds_to_lock,
            confirmation_window: self.confirmation_window,
            request_timeout: self.request_timeout,
        }
    }
}

pub struct OneSidedSendOutcome {
    pub tx_id: String,
    pub fee_microtari: u64,
    pub accepted: bool,
    pub is_synced: bool,
}

#[derive(Default)]
struct ScenarioSendSummary {
    attempted: u32,
    success_count: u32,
    failure_count: u32,
    wall_ms: u128,
    fee_microtari: u64,
    tx_ids: Vec<String>,
    errors: Vec<String>,
    batch_summaries: Vec<BatchSendSummary>,
    construction_complete_ms: Vec<u128>,
    tx_infos: Vec<VerifiedTransaction>,
    extra_metrics: serde_json::Map<String, serde_json::Value>,
}

#[derive(Default)]
struct Mode2VerificationResult {
    observed_transactions: Vec<VerifiedTransaction>,
    observations: Vec<serde_json::Value>,
    used_base_node_query: bool,
}

struct Mode2CompletedTransactionRow {
    pending_tx_id: String,
    status: String,
    mined_height: Option<i64>,
    confirmation_height: Option<i64>,
    sent_payref: Option<String>,
    serialized_transaction: Vec<u8>,
}

#[derive(Debug)]
struct Mode2KernelQuery {
    excess_sig_nonce: Vec<u8>,
    excess_sig: Vec<u8>,
    fee_microtari: Option<u64>,
}

struct BatchSendSummary {
    configured_batch: u32,
    attempted: u32,
    success_count: u32,
    failure_count: u32,
    wall_ms: u128,
}

struct Mode1ConsoleContext {
    process: Mode1ConsoleProcess,
    client: WalletGrpcClient<tonic::transport::Channel>,
    balance: Option<grpc::GetBalanceResponse>,
    birthday: u16,
    grpc_bind: String,
    version: Option<String>,
}

struct Mode1ConsoleProcess {
    child: tokio::process::Child,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

impl Mode1ConsoleProcess {
    fn try_wait(&mut self) -> anyhow::Result<Option<std::process::ExitStatus>> {
        self.child
            .try_wait()
            .context("checking minotari_console_wallet process status")
    }
}

impl Drop for Mode1ConsoleProcess {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

#[derive(Default)]
struct Mode1TransferSummary {
    attempted_batches: u32,
    attempted_payments: u32,
    success_count: u32,
    failure_count: u32,
    wall_ms: u128,
    fee_microtari: u64,
    tx_ids: Vec<String>,
    errors: Vec<String>,
    batch_summaries: Vec<Mode1BatchSummary>,
    tx_infos: Vec<VerifiedTransaction>,
    construction_complete_ms: Vec<u128>,
    extra_metrics: serde_json::Map<String, serde_json::Value>,
    db_lock_retries: u32,
}

struct Mode1BatchSummary {
    configured_batch: u32,
    attempted_batches: u32,
    attempted_payments: u32,
    success_count: u32,
    failure_count: u32,
    wall_ms: u128,
}

struct Mode1TransferOutcome {
    success_count: u32,
    failure_count: u32,
    fee_microtari: u64,
    tx_ids: Vec<String>,
    errors: Vec<String>,
    db_lock_retries: u32,
}

struct Mode3TopologyContext {
    _payment_receiver: payment_processor::ManagedProcess,
    _payment_processor: payment_processor::ManagedProcess,
    client: PaymentProcessorClient,
    receiver_balance: serde_json::Value,
    processor_version: String,
    worker_sleep_secs: u64,
    receiver_birthday: u16,
}

#[derive(Clone)]
struct Mode3TopologyTaskContext {
    client: PaymentProcessorClient,
}

impl Mode3TopologyContext {
    fn clone_for_task(&self) -> Mode3TopologyTaskContext {
        Mode3TopologyTaskContext {
            client: self.client.clone(),
        }
    }
}

#[derive(Default)]
struct PpScenarioSummary {
    attempted_batches: u32,
    attempted_payments: u32,
    accepted_batches: u32,
    failed_batches: u32,
    wall_ms: u128,
    batch_ids: Vec<String>,
    payment_ids: Vec<String>,
    errors: Vec<String>,
    batch_summaries: Vec<PpBatchSummary>,
    db_snapshot: Option<PaymentProcessorDbSnapshot>,
    events: Vec<serde_json::Value>,
    blocked_upstream: bool,
    construction_complete_ms: Vec<u128>,
    extra_metrics: serde_json::Map<String, serde_json::Value>,
}

struct PpBatchSummary {
    configured_batch: u32,
    attempted_batches: u32,
    accepted_batches: u32,
    failed_batches: u32,
    wall_ms: u128,
}

struct PpBatchSubmission {
    batch_id: String,
    payment_ids: Vec<String>,
    raw_response: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct S1RoundPlan {
    round_index: u32,
    tx_count: u32,
    outputs_per_tx: u32,
    target_utxos_after: u32,
    fanout: bool,
}

fn s1_round_plan(config: &Config, cap: u32) -> Vec<S1RoundPlan> {
    let mut remaining = if cap == 0 { u32::MAX } else { cap };
    let mut plans = Vec::new();
    let mut utxos = 1u32;
    for round in 1..=config.benchmark.doubling_rounds {
        if remaining == 0 {
            break;
        }
        let planned = utxos;
        let tx_count = planned.min(remaining);
        utxos = utxos.saturating_add(tx_count);
        plans.push(S1RoundPlan {
            round_index: round,
            tx_count,
            outputs_per_tx: 2,
            target_utxos_after: utxos,
            fanout: false,
        });
        remaining = remaining.saturating_sub(tx_count);
    }
    if remaining > 0 {
        let planned = utxos;
        let tx_count = planned.min(remaining);
        let net_new_outputs = config
            .benchmark
            .fanout_outputs_per_tx
            .saturating_sub(1)
            .saturating_mul(tx_count);
        utxos = utxos.saturating_add(net_new_outputs);
        plans.push(S1RoundPlan {
            round_index: config.benchmark.doubling_rounds.saturating_add(1),
            tx_count,
            outputs_per_tx: config.benchmark.fanout_outputs_per_tx,
            target_utxos_after: utxos.min(config.benchmark.volume_target),
            fanout: true,
        });
    }
    plans
}

fn max_serialization_gap_ms(mut completions: Vec<u128>) -> Option<u128> {
    if completions.len() < 2 {
        return None;
    }
    completions.sort_unstable();
    completions
        .windows(2)
        .map(|pair| pair[1].saturating_sub(pair[0]))
        .max()
}

fn double_selection_rejections(errors: &[String]) -> u32 {
    errors
        .iter()
        .filter(|error| {
            let lower = error.to_lowercase();
            lower.contains("doublespend")
                || lower.contains("double spend")
                || lower.contains("duplicate input")
                || lower.contains("already locked")
                || lower.contains("funds are pending")
        })
        .count()
        .try_into()
        .unwrap_or(u32::MAX)
}

fn terminal_ok_status(status_value: u32) -> bool {
    matches!(status_value, 2 | 6 | 9 | 13)
}

impl PpScenarioSummary {
    fn record_batch(&mut self, batch_index: u32, result: anyhow::Result<PpBatchSubmission>) {
        match result {
            Ok(submission) => {
                println!(
                    "mode3 PP batch {batch_index} accepted: batch_id={} payments={}",
                    submission.batch_id,
                    submission.payment_ids.len()
                );
                self.accepted_batches += 1;
                self.batch_ids.push(submission.batch_id);
                self.payment_ids.extend(submission.payment_ids);
                self.events.push(submission.raw_response);
            }
            Err(error) => {
                println!("mode3 PP batch {batch_index} failed: {error:#}");
                self.failed_batches += 1;
                self.errors.push(format!("{error:#}"));
            }
        }
    }

    fn record_join_error(&mut self, error: String) {
        println!("mode3 PP concurrent task failed: {error}");
        self.failed_batches += 1;
        self.errors.push(format!("task join error: {error}"));
    }

    fn add_batch(&mut self, configured_batch: u32, batch: Self) {
        self.attempted_batches = self
            .attempted_batches
            .saturating_add(batch.attempted_batches);
        self.attempted_payments = self
            .attempted_payments
            .saturating_add(batch.attempted_payments);
        self.accepted_batches = self.accepted_batches.saturating_add(batch.accepted_batches);
        self.failed_batches = self.failed_batches.saturating_add(batch.failed_batches);
        self.batch_ids.extend(batch.batch_ids);
        self.payment_ids.extend(batch.payment_ids);
        self.errors.extend(batch.errors);
        self.events.extend(batch.events);
        self.construction_complete_ms
            .extend(batch.construction_complete_ms);
        self.extra_metrics.extend(batch.extra_metrics);
        self.blocked_upstream |= batch.blocked_upstream;
        self.batch_summaries.push(PpBatchSummary {
            configured_batch,
            attempted_batches: batch.attempted_batches,
            accepted_batches: batch.accepted_batches,
            failed_batches: batch.failed_batches,
            wall_ms: batch.wall_ms,
        });
    }

    async fn observe_db(&mut self, config: &Config) {
        if self.batch_ids.is_empty() && self.payment_ids.is_empty() {
            return;
        }
        let timeout = Duration::from_secs(config.timeouts.confirmation_secs.max(30));
        let start = Instant::now();
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        let mut latest = None;
        let mut attempts = 0u64;
        loop {
            interval.tick().await;
            attempts = attempts.saturating_add(1);
            match payment_processor::inspect_payment_processor_db(
                config,
                &self.batch_ids,
                &self.payment_ids,
            ) {
                Ok(snapshot) => {
                    let done =
                        pp_snapshot_is_terminal_for_summary(&snapshot, self.accepted_batches);
                    latest = Some(snapshot);
                    if done {
                        break;
                    }
                }
                Err(error) => self
                    .errors
                    .push(format!("PP DB inspection failed: {error:#}")),
            }
            if start.elapsed() > timeout {
                break;
            }
        }
        if let Some(snapshot) = latest {
            self.blocked_upstream = snapshot.has_upstream_signing_or_broadcast_error();
            self.db_snapshot = Some(snapshot);
        }
        self.extra_metrics.insert(
            "db_observation_attempts".to_string(),
            serde_json::json!(attempts),
        );
        self.extra_metrics.insert(
            "db_observation_wall_ms".to_string(),
            serde_json::json!(start.elapsed().as_millis()),
        );
    }

    fn note(&self, scenario: ScenarioName) -> String {
        let mut parts = vec![
            format!(
                "{} PP summary: attempted_batches={} attempted_payments={} accepted_batches={} failed_batches={} wall_ms={}",
                scenario.as_str(),
                self.attempted_batches,
                self.attempted_payments,
                self.accepted_batches,
                self.failed_batches,
                self.wall_ms
            ),
            format!("batch_ids={}", limited_list(&self.batch_ids)),
            format!("payment_ids={}", limited_list(&self.payment_ids)),
        ];
        if let Some(snapshot) = &self.db_snapshot {
            parts.push(format!("db_snapshot={}", snapshot.status_summary()));
        }
        if !self.events.is_empty() {
            parts.push(format!(
                "pp_responses={}",
                compact_json(&serde_json::Value::Array(self.events.clone()), 512)
            ));
        }
        if !self.errors.is_empty() {
            parts.push(format!("errors={}", limited_list(&self.errors)));
        }
        if !self.batch_summaries.is_empty() {
            let batches = self
                .batch_summaries
                .iter()
                .map(|batch| {
                    format!(
                        "configured_batch:{} attempted:{} accepted:{} failed:{} wall_ms:{}",
                        batch.configured_batch,
                        batch.attempted_batches,
                        batch.accepted_batches,
                        batch.failed_batches,
                        batch.wall_ms
                    )
                })
                .collect::<Vec<_>>();
            parts.push(format!("batches={}", batches.join("; ")));
        }
        parts.join("; ")
    }

    fn error_note(&self) -> Option<String> {
        if self.blocked_upstream {
            return Some(
                self.db_snapshot
                    .as_ref()
                    .map(PaymentProcessorDbSnapshot::status_summary)
                    .unwrap_or_else(|| "PP upstream signing/broadcast error".to_string()),
            );
        }
        if self.failed_batches > 0 {
            return Some(limited_list(&self.errors));
        }
        None
    }

    fn metrics(&self, scenario: ScenarioName) -> serde_json::Value {
        serde_json::json!({
            "scenario": scenario.as_str(),
            "verification_source": "payment_processor_db_observed",
            "attempted_batches": self.attempted_batches,
            "attempted_payments": self.attempted_payments,
            "accepted_batches": self.accepted_batches,
            "failed_batches": self.failed_batches,
            "batch_ids": self.batch_ids,
            "payment_ids": self.payment_ids,
            "max_serialization_gap_ms": max_serialization_gap_ms(self.construction_complete_ms.clone()),
            "double_selection_rejections": double_selection_rejections(&self.errors),
            "db_status_summary": self.db_snapshot.as_ref().map(PaymentProcessorDbSnapshot::status_summary),
            "responses": self.events,
            "extra": self.extra_metrics,
        })
    }

    fn verified_transactions(&self, scenario: ScenarioName) -> Vec<VerifiedTransaction> {
        self.db_snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .batches
                    .iter()
                    .filter(|batch| batch.status == "CONFIRMED")
                    .map(|batch| VerifiedTransaction {
                        tx_id: batch.id.clone(),
                        status_value: TX_MINED_CONFIRMED_STATUS,
                        mode: "payment_processor".to_string(),
                        scenario: scenario.as_str().to_string(),
                        amount_microtari: None,
                        fee_microtari: None,
                        mined_height: batch
                            .mined_height
                            .and_then(|height| u64::try_from(height).ok()),
                        confirmed: true,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn with_extra_metrics(mut self, metrics: serde_json::Map<String, serde_json::Value>) -> Self {
        self.extra_metrics.extend(metrics);
        self
    }
}

#[cfg(test)]
fn pp_snapshot_has_progress_or_error(snapshot: &PaymentProcessorDbSnapshot) -> bool {
    snapshot.has_upstream_signing_or_broadcast_error()
        || snapshot.batches.iter().all(|batch| {
            matches!(
                batch.status.as_str(),
                "AWAITING_CONFIRMATION" | "CONFIRMED" | "FAILED" | "CANCELLED"
            ) || batch.has_signed_tx
        })
}

fn pp_snapshot_is_terminal_for_summary(
    snapshot: &PaymentProcessorDbSnapshot,
    accepted_batches: u32,
) -> bool {
    if snapshot.has_upstream_signing_or_broadcast_error() {
        return true;
    }
    let accepted_batches = usize::try_from(accepted_batches).unwrap_or(usize::MAX);
    snapshot.batches.len() >= accepted_batches
        && snapshot
            .batches
            .iter()
            .all(|batch| matches!(batch.status.as_str(), "CONFIRMED" | "FAILED" | "CANCELLED"))
}

impl Mode1TransferOutcome {
    fn with_retries(mut self, db_lock_retries: u32) -> Self {
        self.db_lock_retries = db_lock_retries;
        self
    }

    fn from_response(response: grpc::TransferResponse) -> Self {
        let mut outcome = Self {
            success_count: 0,
            failure_count: 0,
            fee_microtari: 0,
            tx_ids: Vec::new(),
            errors: Vec::new(),
            db_lock_retries: 0,
        };
        for result in response.results {
            if result.is_success {
                outcome.success_count += 1;
                outcome.tx_ids.push(result.transaction_id.to_string());
                if let Some(info) = result.transaction_info {
                    outcome.fee_microtari = outcome.fee_microtari.saturating_add(info.fee);
                }
            } else {
                outcome.failure_count += 1;
                outcome.errors.push(format!(
                    "address={} failure={}",
                    result.address, result.failure_message
                ));
            }
        }
        outcome
    }
}

impl Mode1TransferSummary {
    fn backfill_verified_fee_total(&mut self) {
        let verified_fee_total = self
            .tx_infos
            .iter()
            .filter(|tx| tx.confirmed)
            .filter_map(|tx| tx.fee_microtari)
            .fold(0u64, u64::saturating_add);
        if verified_fee_total > self.fee_microtari {
            self.fee_microtari = verified_fee_total;
        }
    }

    fn record_batch(
        &mut self,
        batch_index: u32,
        items_per_batch: u32,
        result: anyhow::Result<Mode1TransferOutcome>,
    ) {
        match result {
            Ok(outcome) => {
                println!(
                    "mode1 gRPC batch {batch_index} completed: successes={} failures={} tx_ids={}",
                    outcome.success_count,
                    outcome.failure_count,
                    limited_list(&outcome.tx_ids)
                );
                self.success_count = self.success_count.saturating_add(outcome.success_count);
                self.failure_count = self.failure_count.saturating_add(outcome.failure_count);
                self.fee_microtari = self.fee_microtari.saturating_add(outcome.fee_microtari);
                self.db_lock_retries = self.db_lock_retries.saturating_add(outcome.db_lock_retries);
                self.tx_ids.extend(outcome.tx_ids);
                self.errors.extend(outcome.errors);
            }
            Err(error) => {
                println!("mode1 gRPC batch {batch_index} failed: {error:#}");
                self.failure_count = self.failure_count.saturating_add(items_per_batch);
                self.errors.push(format!("{error:#}"));
            }
        }
    }

    fn record_join_error(&mut self, error: String) {
        println!("mode1 concurrent gRPC transfer task failed: {error}");
        self.failure_count += 1;
        self.errors.push(format!("task join error: {error}"));
    }

    fn add_batch(&mut self, configured_batch: u32, batch: Self) {
        self.attempted_batches = self
            .attempted_batches
            .saturating_add(batch.attempted_batches);
        self.attempted_payments = self
            .attempted_payments
            .saturating_add(batch.attempted_payments);
        self.success_count = self.success_count.saturating_add(batch.success_count);
        self.failure_count = self.failure_count.saturating_add(batch.failure_count);
        self.fee_microtari = self.fee_microtari.saturating_add(batch.fee_microtari);
        self.db_lock_retries = self.db_lock_retries.saturating_add(batch.db_lock_retries);
        self.tx_ids.extend(batch.tx_ids);
        self.errors.extend(batch.errors);
        self.tx_infos.extend(batch.tx_infos);
        self.construction_complete_ms
            .extend(batch.construction_complete_ms);
        self.extra_metrics.extend(batch.extra_metrics);
        self.batch_summaries.push(Mode1BatchSummary {
            configured_batch,
            attempted_batches: batch.attempted_batches,
            attempted_payments: batch.attempted_payments,
            success_count: batch.success_count,
            failure_count: batch.failure_count,
            wall_ms: batch.wall_ms,
        });
    }

    fn note(&self, scenario: ScenarioName) -> String {
        let mut parts = vec![
            format!(
                "{} console gRPC summary: attempted_batches={} attempted_payments={} successes={} failures={} wall_ms={} fee_microtari={}",
                scenario.as_str(),
                self.attempted_batches,
                self.attempted_payments,
                self.success_count,
                self.failure_count,
                self.wall_ms,
                self.fee_microtari
            ),
            format!("tx_ids={}", limited_list(&self.tx_ids)),
        ];
        if self.db_lock_retries > 0 {
            parts.push(format!("db_lock_retries={}", self.db_lock_retries));
        }
        if !self.errors.is_empty() {
            parts.push(format!("errors={}", limited_list(&self.errors)));
        }
        if !self.batch_summaries.is_empty() {
            let batches = self
                .batch_summaries
                .iter()
                .map(|batch| {
                    format!(
                        "configured_batch:{} attempted_batches:{} attempted_payments:{} successes:{} failures:{} wall_ms:{}",
                        batch.configured_batch,
                        batch.attempted_batches,
                        batch.attempted_payments,
                        batch.success_count,
                        batch.failure_count,
                        batch.wall_ms
                    )
                })
                .collect::<Vec<_>>();
            parts.push(format!("batches={}", batches.join("; ")));
        }
        parts.join("; ")
    }

    fn metrics(&self, scenario: ScenarioName) -> serde_json::Value {
        let mut metrics = serde_json::Map::new();
        metrics.insert(
            "attempted_batches".to_string(),
            serde_json::json!(self.attempted_batches),
        );
        metrics.insert(
            "attempted_payments".to_string(),
            serde_json::json!(self.attempted_payments),
        );
        metrics.insert("tx_ids".to_string(), serde_json::json!(self.tx_ids));
        metrics.insert(
            "verified_transactions".to_string(),
            serde_json::json!(self.tx_infos),
        );
        metrics.insert(
            "max_serialization_gap_ms".to_string(),
            serde_json::json!(max_serialization_gap_ms(
                self.construction_complete_ms.clone()
            )),
        );
        metrics.insert(
            "double_selection_rejections".to_string(),
            serde_json::json!(double_selection_rejections(&self.errors)),
        );
        metrics.insert(
            "db_lock_retries".to_string(),
            serde_json::json!(self.db_lock_retries),
        );
        metrics.insert("scenario".to_string(), serde_json::json!(scenario.as_str()));
        metrics.extend(self.extra_metrics.clone());
        serde_json::Value::Object(metrics)
    }

    fn error_note(&self) -> Option<String> {
        if self.failure_count == 0 {
            None
        } else {
            Some(limited_list(&self.errors))
        }
    }
}

impl ScenarioSendSummary {
    fn record_attempt(&mut self, attempt: u32, result: anyhow::Result<OneSidedSendOutcome>) {
        match result {
            Ok(outcome) => {
                println!(
                    "mode2 send attempt {attempt} ok: tx_id={} accepted={} is_synced={}",
                    outcome.tx_id, outcome.accepted, outcome.is_synced
                );
                self.success_count += 1;
                self.fee_microtari = self.fee_microtari.saturating_add(outcome.fee_microtari);
                self.tx_ids.push(outcome.tx_id);
            }
            Err(error) => {
                println!("mode2 send attempt {attempt} failed: {error:#}");
                self.failure_count += 1;
                self.errors.push(format!("{error:#}"));
            }
        }
    }

    fn record_join_error(&mut self, error: String) {
        println!("mode2 concurrent send task failed: {error}");
        self.failure_count += 1;
        self.errors.push(format!("task join error: {error}"));
    }

    fn add_batch(&mut self, configured_batch: u32, batch: Self) {
        self.attempted = self.attempted.saturating_add(batch.attempted);
        self.success_count = self.success_count.saturating_add(batch.success_count);
        self.failure_count = self.failure_count.saturating_add(batch.failure_count);
        self.fee_microtari = self.fee_microtari.saturating_add(batch.fee_microtari);
        self.tx_ids.extend(batch.tx_ids);
        self.errors.extend(batch.errors);
        self.construction_complete_ms
            .extend(batch.construction_complete_ms);
        self.tx_infos.extend(batch.tx_infos);
        self.extra_metrics.extend(batch.extra_metrics);
        self.batch_summaries.push(BatchSendSummary {
            configured_batch,
            attempted: batch.attempted,
            success_count: batch.success_count,
            failure_count: batch.failure_count,
            wall_ms: batch.wall_ms,
        });
    }

    fn apply_mode2_verification(&mut self, verification: Mode2VerificationResult) {
        self.tx_infos = verification.observed_transactions;
        self.extra_metrics.insert(
            "verification_source".to_string(),
            serde_json::json!(if verification.used_base_node_query {
                "base_node_transaction_query"
            } else {
                "wallet_db_observed"
            }),
        );
        self.extra_metrics.insert(
            "verification_observations".to_string(),
            serde_json::Value::Array(verification.observations),
        );
    }

    fn note(&self, scenario: ScenarioName) -> String {
        let mut parts = vec![
            format!(
                "{} summary: attempted={} successes={} failures={} wall_ms={} fee_microtari={}",
                scenario.as_str(),
                self.attempted,
                self.success_count,
                self.failure_count,
                self.wall_ms,
                self.fee_microtari
            ),
            format!("tx_ids={}", limited_list(&self.tx_ids)),
        ];
        if !self.errors.is_empty() {
            parts.push(format!("errors={}", limited_list(&self.errors)));
        }
        if !self.batch_summaries.is_empty() {
            let batches = self
                .batch_summaries
                .iter()
                .map(|batch| {
                    format!(
                        "configured_batch:{} attempted:{} successes:{} failures:{} wall_ms:{}",
                        batch.configured_batch,
                        batch.attempted,
                        batch.success_count,
                        batch.failure_count,
                        batch.wall_ms
                    )
                })
                .collect::<Vec<_>>();
            parts.push(format!("batches={}", batches.join("; ")));
        }
        parts.join("; ")
    }

    fn error_note(&self) -> Option<String> {
        if self.failure_count == 0 {
            None
        } else {
            Some(limited_list(&self.errors))
        }
    }

    fn metrics(&self, scenario: ScenarioName) -> serde_json::Value {
        let mut metrics = serde_json::Map::new();
        metrics.insert("scenario".to_string(), serde_json::json!(scenario.as_str()));
        metrics.insert(
            "verification_source".to_string(),
            serde_json::json!("wallet_db_observed"),
        );
        metrics.insert("attempted".to_string(), serde_json::json!(self.attempted));
        metrics.insert("tx_ids".to_string(), serde_json::json!(self.tx_ids));
        metrics.insert(
            "verified_transactions".to_string(),
            serde_json::json!(self.verified_transactions()),
        );
        metrics.insert(
            "observed_transactions".to_string(),
            serde_json::json!(self.tx_infos),
        );
        metrics.insert(
            "max_serialization_gap_ms".to_string(),
            serde_json::json!(max_serialization_gap_ms(
                self.construction_complete_ms.clone()
            )),
        );
        metrics.insert(
            "double_selection_rejections".to_string(),
            serde_json::json!(double_selection_rejections(&self.errors)),
        );
        metrics.extend(self.extra_metrics.clone());
        serde_json::Value::Object(metrics)
    }

    fn verified_transactions(&self) -> Vec<VerifiedTransaction> {
        self.tx_infos
            .iter()
            .filter(|tx| tx.confirmed)
            .cloned()
            .collect()
    }
}

fn limited_list(values: &[String]) -> String {
    const LIMIT: usize = 12;
    let mut visible = values.iter().take(LIMIT).cloned().collect::<Vec<_>>();
    if values.len() > LIMIT {
        visible.push(format!("... {} more", values.len() - LIMIT));
    }
    format!("[{}]", visible.join(", "))
}

fn compact_json(value: &serde_json::Value, limit: usize) -> String {
    let rendered = value.to_string();
    if rendered.len() <= limit {
        return rendered;
    }
    format!("{}...<truncated>", &rendered[..limit])
}

async fn construct_sign_broadcast_one_sided_owned(
    request: OwnedOneSidedSendRequest,
) -> anyhow::Result<OneSidedSendOutcome> {
    construct_sign_broadcast_one_sided(request.as_borrowed()).await
}

async fn construct_sign_broadcast_one_sided_multi_recipient_owned(
    request: OwnedOneSidedSendRequest,
    recipients: Vec<String>,
) -> anyhow::Result<OneSidedSendOutcome> {
    construct_sign_broadcast_one_sided_multi_recipient(request.as_borrowed(), &recipients).await
}

pub async fn construct_sign_broadcast_one_sided(
    request: OneSidedSendRequest<'_>,
) -> anyhow::Result<OneSidedSendOutcome> {
    let pool = init_db(request.db_path.to_path_buf())?;
    let mut sender = TransactionSender::new(
        pool,
        "default".to_string(),
        request.password.to_string(),
        Network::Esmeralda,
        request.confirmation_window,
    )?;
    sender.fee_per_gram = request.fee_rate;
    let recipient = Recipient {
        address: TariAddress::from_str(request.recipient)?,
        amount: request.amount,
        payment_id: None,
    };
    let unsigned = sender.start_new_transaction(
        uuid_like_idempotency(),
        recipient,
        request.seconds_to_lock,
    )?;
    let key_manager = sender.account.get_key_manager(request.password)?;
    let constants = ConsensusConstantsBuilder::new(Network::Esmeralda).build();
    let signed =
        match sign_locked_transaction(&key_manager, constants, Network::Esmeralda, unsigned) {
            Ok(signed) => signed,
            Err(error) => {
                if let Err(cleanup) = expire_and_unlock_processed_transaction(&sender) {
                    anyhow::bail!(
                        "signing locked transaction failed: {error}; cleanup failed: {cleanup:#}"
                    );
                }
                anyhow::bail!("signing locked transaction failed: {error}");
            }
        };
    finalize_transaction_and_broadcast_without_retry(&sender, signed, request).await
}

pub async fn construct_sign_broadcast_one_sided_multi_recipient(
    request: OneSidedSendRequest<'_>,
    recipients: &[String],
) -> anyhow::Result<OneSidedSendOutcome> {
    if recipients.is_empty() {
        bail!("multi-recipient one-sided transaction requires at least one recipient");
    }
    let pool = init_db(request.db_path.to_path_buf())?;
    let account = {
        let conn = pool.get()?;
        db::get_account_by_name(&conn, "default")?.context("Account not found: default")?
    };
    let recipients = recipients
        .iter()
        .map(|recipient| {
            Ok(Recipient {
                address: TariAddress::from_str(recipient)?,
                amount: request.amount,
                payment_id: None,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let amount = recipients.iter().map(|recipient| recipient.amount).sum();
    let idempotency_key = uuid_like_idempotency();
    let locked_funds = FundLocker::new(pool.clone()).lock(
        account.id,
        amount,
        recipients.len(),
        request.fee_rate,
        None,
        Some(idempotency_key.clone()),
        request.seconds_to_lock,
        request.confirmation_window,
    )?;
    let pending_tx_id = {
        let conn = pool.get()?;
        db::find_pending_transaction_by_idempotency_key(&conn, &idempotency_key, account.id)?
            .map(|pending| pending.id.to_string())
            .with_context(|| {
                format!("pending transaction missing for idempotency key {idempotency_key}")
            })?
    };
    let one_sided_tx = OneSidedTransaction::new(
        pool.clone(),
        Network::Esmeralda,
        request.password.to_string(),
    );
    let unsigned = match one_sided_tx.create_unsigned_transaction(
        &account,
        locked_funds,
        recipients,
        request.fee_rate,
    ) {
        Ok(unsigned) => unsigned,
        Err(error) => {
            if let Err(cleanup) = expire_and_unlock_pending_transaction_id(&pool, &pending_tx_id) {
                anyhow::bail!(
                    "creating multi-recipient unsigned transaction failed: {error}; cleanup failed: {cleanup:#}"
                );
            }
            anyhow::bail!("creating multi-recipient unsigned transaction failed: {error}");
        }
    };
    let key_manager = match account.get_key_manager(request.password) {
        Ok(key_manager) => key_manager,
        Err(error) => {
            if let Err(cleanup) = expire_and_unlock_pending_transaction_id(&pool, &pending_tx_id) {
                anyhow::bail!("opening key manager failed: {error}; cleanup failed: {cleanup:#}");
            }
            anyhow::bail!("opening key manager failed: {error}");
        }
    };
    let constants = ConsensusConstantsBuilder::new(Network::Esmeralda).build();
    let signed = match sign_locked_transaction(
        &key_manager,
        constants,
        Network::Esmeralda,
        unsigned,
    ) {
        Ok(signed) => signed,
        Err(error) => {
            if let Err(cleanup) = expire_and_unlock_pending_transaction_id(&pool, &pending_tx_id) {
                anyhow::bail!(
                    "signing multi-recipient locked transaction failed: {error}; cleanup failed: {cleanup:#}"
                );
            }
            anyhow::bail!("signing multi-recipient locked transaction failed: {error}");
        }
    };
    finalize_signed_transaction_and_broadcast_without_retry(
        &pool,
        account.id,
        &pending_tx_id,
        signed,
        request,
    )
    .await
}

pub async fn fund_one_sided_outputs(
    config: &Config,
    source_db: &Path,
    recipient: &str,
    amount: &str,
    outputs: u32,
    batch_size: u32,
) -> anyhow::Result<()> {
    if outputs == 0 {
        bail!("--outputs must be greater than 0");
    }
    if batch_size == 0 {
        bail!("--batch-size must be greater than 0");
    }
    if !source_db.exists() {
        bail!("source DB not found at {}", source_db.display());
    }
    let amount = parse_amount(amount)?;
    let request = OwnedOneSidedSendRequest {
        db_path: source_db.to_path_buf(),
        password: wallet_password(&config.seeds.wallet_password_env)?,
        base_node_url: config.network.base_node_http_url.clone(),
        recipient: recipient.to_string(),
        amount,
        fee_rate: config.fee_rate()?,
        seconds_to_lock: config.timeouts.transaction_lock_secs,
        confirmation_window: config.benchmark.c_min,
        request_timeout: Duration::from_secs(30),
    };

    let mut remaining = outputs;
    let mut batch_index = 0u32;
    let mut tx_ids = Vec::new();
    while remaining > 0 {
        batch_index = batch_index.saturating_add(1);
        let batch_outputs = remaining.min(batch_size);
        println!(
            "fund-one-sided batch {batch_index}: outputs={batch_outputs} amount_microtari={}",
            amount.0
        );
        let recipients = repeated_recipient(recipient, batch_outputs as usize);
        let outcome =
            construct_sign_broadcast_one_sided_multi_recipient_owned(request.clone(), recipients)
                .await?;
        println!(
            "fund-one-sided batch {batch_index} accepted={} synced={} tx_id={} fee_microtari={}",
            outcome.accepted, outcome.is_synced, outcome.tx_id, outcome.fee_microtari
        );
        tx_ids.push(outcome.tx_id);
        remaining -= batch_outputs;
    }

    println!(
        "fund-one-sided submitted {} txs for {} outputs: {}",
        tx_ids.len(),
        outputs,
        tx_ids.join(",")
    );
    Ok(())
}

async fn finalize_transaction_and_broadcast_without_retry(
    sender: &TransactionSender,
    signed: SignedOneSidedTransactionResult,
    request: OneSidedSendRequest<'_>,
) -> anyhow::Result<OneSidedSendOutcome> {
    finalize_signed_transaction_and_broadcast_without_retry(
        &sender.db_pool,
        sender.account.id,
        sender.processed_transactions.id(),
        signed,
        request,
    )
    .await
}

async fn finalize_signed_transaction_and_broadcast_without_retry(
    db_pool: &SqlitePool,
    account_id: i64,
    pending_tx_id: &str,
    signed: SignedOneSidedTransactionResult,
    request: OneSidedSendRequest<'_>,
) -> anyhow::Result<OneSidedSendOutcome> {
    if let Err(error) = persist_signed_transaction(db_pool, account_id, pending_tx_id, &signed) {
        if let Err(cleanup) = expire_and_unlock_pending_transaction_id(db_pool, pending_tx_id) {
            anyhow::bail!(
                "persisting signed transaction failed: {error:#}; cleanup failed: {cleanup:#}"
            );
        }
        return Err(error);
    }
    let tx_id = signed.signed_transaction.tx_id;
    let fee_microtari = signed.request.info.fee.0;
    let submission = submit_transaction_without_retry(
        request.base_node_url,
        signed.signed_transaction.transaction,
        request.request_timeout,
    )
    .await;

    let conn = db_pool.get()?;
    match submission {
        Ok(response) if response.accepted => {
            db::mark_completed_transaction_as_broadcasted(&conn, tx_id, 1)?;
            Ok(OneSidedSendOutcome {
                tx_id: tx_id.to_string(),
                fee_microtari,
                accepted: response.accepted,
                is_synced: response.is_synced,
            })
        }
        Ok(response) if response.rejection_reason == TxSubmissionRejectionReason::AlreadyMined => {
            Ok(OneSidedSendOutcome {
                tx_id: tx_id.to_string(),
                fee_microtari,
                accepted: response.accepted,
                is_synced: response.is_synced,
            })
        }
        Ok(response) => {
            db::mark_completed_transaction_as_rejected(
                &conn,
                tx_id,
                &response.rejection_reason.to_string(),
            )?;
            db::update_pending_transaction_status(
                &conn,
                pending_tx_id,
                PendingTransactionStatus::Expired,
            )?;
            db::unlock_outputs_for_pending_transaction(&conn, pending_tx_id)?;
            anyhow::bail!(
                "transaction was not accepted by the network: {}",
                response.rejection_reason
            );
        }
        Err(error) => {
            db::mark_completed_transaction_as_rejected(
                &conn,
                tx_id,
                &format!("Transaction submission failed: {error}"),
            )?;
            db::update_pending_transaction_status(
                &conn,
                pending_tx_id,
                PendingTransactionStatus::Expired,
            )?;
            db::unlock_outputs_for_pending_transaction(&conn, pending_tx_id)?;
            Err(error)
        }
    }
}

fn expire_and_unlock_processed_transaction(sender: &TransactionSender) -> anyhow::Result<()> {
    let pending_tx_id = sender.processed_transactions.id();
    expire_and_unlock_pending_transaction_id(&sender.db_pool, pending_tx_id)
}

fn expire_and_unlock_pending_transaction_id(
    db_pool: &SqlitePool,
    pending_tx_id: &str,
) -> anyhow::Result<()> {
    if pending_tx_id.is_empty() {
        return Ok(());
    }
    let conn = db_pool.get()?;
    db::update_pending_transaction_status(&conn, pending_tx_id, PendingTransactionStatus::Expired)?;
    db::unlock_outputs_for_pending_transaction(&conn, pending_tx_id)?;
    Ok(())
}

fn persist_signed_transaction(
    db_pool: &SqlitePool,
    account_id: i64,
    pending_tx_id: &str,
    signed: &SignedOneSidedTransactionResult,
) -> anyhow::Result<()> {
    if pending_tx_id.is_empty() {
        anyhow::bail!("pending transaction id missing before broadcast");
    }
    let conn = db_pool.get()?;

    let tx_id = signed.signed_transaction.tx_id;
    let kernel_excess = signed
        .signed_transaction
        .transaction
        .body()
        .kernels()
        .first()
        .map(|kernel| kernel.excess.as_bytes().to_vec())
        .unwrap_or_default();
    let serialized_transaction = serde_json::to_vec(&signed.signed_transaction.transaction)
        .context("serializing signed transaction")?;
    let sent_output_hash = signed
        .signed_transaction
        .sent_hashes
        .first()
        .map(hex::encode);

    db::update_pending_transaction_status(
        &conn,
        pending_tx_id,
        PendingTransactionStatus::Completed,
    )?;
    db::create_completed_transaction(
        &conn,
        account_id,
        pending_tx_id,
        &kernel_excess,
        &serialized_transaction,
        sent_output_hash,
        tx_id,
    )?;
    Ok(())
}

async fn submit_transaction_without_retry(
    base_node_url: &str,
    transaction: tari_transaction_components::transaction_components::Transaction,
    timeout: Duration,
) -> anyhow::Result<TxSubmissionResponse> {
    let url = json_rpc_url(base_node_url)?;
    let client = reqwest::Client::builder().timeout(timeout).build()?;
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "1",
        "method": "submit_transaction",
        "params": { "transaction": transaction }
    });

    let response = client.post(url).json(&request).send().await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("submit_transaction HTTP {status}: {body}");
    }
    let envelope: JsonRpcEnvelope<TxSubmissionResponse> = serde_json::from_str(&body)?;
    if let Some(result) = envelope.result {
        return Ok(result);
    }
    anyhow::bail!(
        "submit_transaction JSON-RPC error: {}",
        envelope
            .error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "missing result".to_string())
    )
}

fn json_rpc_url(base_node_url: &str) -> anyhow::Result<url::Url> {
    Ok(url::Url::parse(base_node_url)?.join("/json_rpc")?)
}

#[derive(serde::Deserialize)]
struct JsonRpcEnvelope<T> {
    result: Option<T>,
    error: Option<serde_json::Value>,
}

fn wallet_password(env_var: &str) -> anyhow::Result<String> {
    std::env::var(env_var).with_context(|| format!("${env_var} must be set"))
}

fn uuid_like_idempotency() -> String {
    format!(
        "bench-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    )
}

#[derive(Debug, Clone, Copy)]
struct FreshScanSpec {
    scenario: ScenarioName,
    wallet_state: FreshScanWalletState,
    checkpoint: ScanCheckpoint,
}

impl FreshScanSpec {
    fn seed(self, funded_seed_words: Option<&str>) -> anyhow::Result<CipherSeed> {
        match self.wallet_state {
            FreshScanWalletState::EmptyGenesis => {
                let mut seed = CipherSeed::random();
                seed.change_birthday(0);
                Ok(seed)
            }
            FreshScanWalletState::FundedGenesis => {
                let words = funded_seed_words.context("funded seed words missing")?;
                seed_from_words_with_birthday(words, 0)
            }
            FreshScanWalletState::FundedBirthday { birthday } => {
                let words = funded_seed_words.context("funded seed words missing")?;
                seed_from_words_with_birthday(words, birthday)
            }
        }
    }

    fn birthday(self) -> u16 {
        match self.wallet_state {
            FreshScanWalletState::EmptyGenesis | FreshScanWalletState::FundedGenesis => 0,
            FreshScanWalletState::FundedBirthday { birthday } => birthday,
        }
    }

    fn port_offset(self, run: u32) -> u16 {
        let scenario_offset = match self.scenario {
            ScenarioName::B0 => 100,
            ScenarioName::S2 => 200,
            ScenarioName::S3 => 300,
            ScenarioName::S6 => 400,
            ScenarioName::S7 => 500,
            _ => 900,
        };
        scenario_offset + u16::try_from(run).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy)]
enum FreshScanWalletState {
    EmptyGenesis,
    FundedGenesis,
    FundedBirthday { birthday: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanCheckpoint {
    Empty,
    PostS1,
    PostS1Partial,
    PostS1Blocked,
    PostS5Complete,
    PostS5Partial,
    PostS5Blocked,
}

impl ScanCheckpoint {
    fn label(self) -> &'static str {
        match self {
            Self::Empty => "empty_genesis",
            Self::PostS1 => "post_s1",
            Self::PostS1Partial => "post_s1_partial",
            Self::PostS1Blocked => "post_s1_blocked",
            Self::PostS5Complete => "post_s5_complete",
            Self::PostS5Partial => "post_s5_partial",
            Self::PostS5Blocked => "post_s5_blocked",
        }
    }

    fn runnable(self) -> bool {
        !matches!(self, Self::PostS1Blocked | Self::PostS5Blocked)
    }

    fn blocked_note(self, scenario: ScenarioName) -> String {
        format!(
            "{} scan not run: prerequisite checkpoint is {}",
            scenario.as_str(),
            self.label()
        )
    }
}

fn fresh_scan_wallet_state(scenario: ScenarioName, birthday: u16) -> FreshScanWalletState {
    match scenario {
        ScenarioName::B0 => FreshScanWalletState::EmptyGenesis,
        ScenarioName::S2 | ScenarioName::S6 => FreshScanWalletState::FundedGenesis,
        ScenarioName::S3 | ScenarioName::S7 => FreshScanWalletState::FundedBirthday { birthday },
        _ => FreshScanWalletState::FundedBirthday { birthday },
    }
}

fn checkpoint_from_mode1_summary(
    summary: &Mode1TransferSummary,
    complete_checkpoint: ScanCheckpoint,
) -> ScanCheckpoint {
    if summary.success_count == 0 {
        return match complete_checkpoint {
            ScanCheckpoint::PostS1 => ScanCheckpoint::PostS1Blocked,
            _ => ScanCheckpoint::PostS5Blocked,
        };
    }
    if summary.failure_count > 0 {
        return match complete_checkpoint {
            ScanCheckpoint::PostS1 => ScanCheckpoint::PostS1Partial,
            _ => ScanCheckpoint::PostS5Partial,
        };
    }
    complete_checkpoint
}

fn checkpoint_from_mode2_summary(
    summary: &ScenarioSendSummary,
    complete_checkpoint: ScanCheckpoint,
) -> ScanCheckpoint {
    if summary.success_count == 0 {
        return match complete_checkpoint {
            ScanCheckpoint::PostS1 => ScanCheckpoint::PostS1Blocked,
            _ => ScanCheckpoint::PostS5Blocked,
        };
    }
    if summary.failure_count > 0 {
        return match complete_checkpoint {
            ScanCheckpoint::PostS1 => ScanCheckpoint::PostS1Partial,
            _ => ScanCheckpoint::PostS5Partial,
        };
    }
    complete_checkpoint
}

fn checkpoint_from_pp_summary(
    summary: &PpScenarioSummary,
    complete_checkpoint: ScanCheckpoint,
) -> ScanCheckpoint {
    if summary.accepted_batches == 0 {
        return match complete_checkpoint {
            ScanCheckpoint::PostS1 => ScanCheckpoint::PostS1Blocked,
            _ => ScanCheckpoint::PostS5Blocked,
        };
    }
    if summary.failed_batches > 0 || summary.blocked_upstream {
        return match complete_checkpoint {
            ScanCheckpoint::PostS1 => ScanCheckpoint::PostS1Partial,
            _ => ScanCheckpoint::PostS5Partial,
        };
    }
    complete_checkpoint
}

struct AccountSnapshot {
    max_height: u64,
    available_microtari: u64,
}

struct ScanMeasurement {
    wall_ms: u128,
    birthday: u16,
    max_height: u64,
    available_microtari: u64,
    tip_start: Option<u64>,
    tip_end: Option<u64>,
    detected_outputs: u64,
}

impl ScanMeasurement {
    fn note(&self) -> String {
        format!(
            "fresh scan checkpoint data: birthday={} max_height={} available_microtari={} detected_outputs={} tip_start={:?} tip_end={:?}",
            self.birthday,
            self.max_height,
            self.available_microtari,
            self.detected_outputs,
            self.tip_start,
            self.tip_end
        )
    }

    fn metrics(&self, mode: &str, spec: FreshScanSpec) -> serde_json::Value {
        let blocks_scanned = if self.birthday == 0 {
            Some(self.max_height)
        } else {
            self.tip_end
                .map(|tip| tip.saturating_sub(u64::from(self.birthday)))
        };
        let blocks_per_sec = blocks_scanned.and_then(|blocks| {
            if self.wall_ms == 0 {
                None
            } else {
                Some((blocks as f64) / (self.wall_ms as f64 / 1000.0))
            }
        });
        serde_json::json!({
            "mode": mode,
            "scenario": spec.scenario.as_str(),
            "verification_source": "wallet_scan_observed",
            "scan_checkpoint": spec.checkpoint.label(),
            "birthday": self.birthday,
            "tip_start": self.tip_start,
            "tip_end": self.tip_end,
            "blocks_scanned": blocks_scanned,
            "blocks_per_sec": blocks_per_sec,
            "detected_outputs": self.detected_outputs,
            "available_microtari": self.available_microtari,
            "max_height": self.max_height
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Config,
        modes::ModeName,
        result_profile::{ResultProfile, empty_mode_profile},
    };

    #[test]
    fn s1_round_plan_reaches_512_without_cap() {
        let config = Config::default();
        let rounds = s1_round_plan(&config, 0);
        assert_eq!(rounds.len(), 7);
        assert_eq!(rounds[0].tx_count, 1);
        assert_eq!(rounds[5].target_utxos_after, 64);
        assert_eq!(rounds[6].tx_count, 64);
        assert_eq!(rounds[6].outputs_per_tx, 8);
        assert_eq!(rounds[6].target_utxos_after, 512);
    }

    #[test]
    fn s1_round_plan_honors_cap_without_inventing_rounds() {
        let config = Config::default();
        let rounds = s1_round_plan(&config, 3);
        assert_eq!(rounds.len(), 2);
        assert_eq!(rounds[0].tx_count, 1);
        assert_eq!(rounds[1].tx_count, 2);
        assert!(!rounds[1].fanout);
    }

    #[test]
    fn serialization_gap_uses_sorted_completion_times() {
        assert_eq!(max_serialization_gap_ms(vec![30, 10, 50, 35]), Some(20));
        assert_eq!(max_serialization_gap_ms(vec![10]), None);
    }

    #[test]
    fn double_selection_rejections_classifies_wallet_lock_errors() {
        let errors = vec![
            "Funds are pending. Available: 0".to_string(),
            "duplicate input detected".to_string(),
            "plain network timeout".to_string(),
        ];
        assert_eq!(double_selection_rejections(&errors), 2);
    }

    #[test]
    fn mode1_database_lock_detection_matches_console_wallet_error() {
        let status = tonic::Status::unknown(
            r#"OutputManagerError(OutputManagerStorageError(DieselError(DatabaseError(Unknown, "database is locked"))))"#,
        );
        assert!(mode1_status_is_database_locked(&status));
        assert!(!mode1_status_is_database_locked(&tonic::Status::unknown(
            "Funds are pending"
        )));
    }

    #[test]
    fn mode1_summary_records_db_lock_retry_metrics() {
        let mut summary = Mode1TransferSummary::default();
        summary.record_batch(
            1,
            1,
            Ok(Mode1TransferOutcome {
                success_count: 1,
                failure_count: 0,
                fee_microtari: 0,
                tx_ids: vec!["42".to_string()],
                errors: Vec::new(),
                db_lock_retries: 2,
            }),
        );

        let metrics = summary.metrics(ScenarioName::S1);
        assert_eq!(metrics["db_lock_retries"], serde_json::json!(2));
        assert!(summary.note(ScenarioName::S1).contains("db_lock_retries=2"));
    }

    #[test]
    fn mode1_summary_backfills_missing_verified_fee_total() {
        let mut summary = Mode1TransferSummary {
            fee_microtari: 0,
            tx_infos: vec![VerifiedTransaction {
                tx_id: "tx".to_string(),
                status_value: TX_MINED_CONFIRMED_STATUS,
                mode: "old_wallet".to_string(),
                scenario: ScenarioName::S1.as_str().to_string(),
                amount_microtari: Some(2_000_000),
                fee_microtari: Some(945),
                mined_height: Some(710_357),
                confirmed: true,
            }],
            ..Mode1TransferSummary::default()
        };

        summary.backfill_verified_fee_total();

        assert_eq!(summary.fee_microtari, 945);
    }

    #[test]
    fn mode1_summary_keeps_larger_response_fee_total() {
        let mut summary = Mode1TransferSummary {
            fee_microtari: 1_000,
            tx_infos: vec![VerifiedTransaction {
                tx_id: "tx".to_string(),
                status_value: TX_MINED_CONFIRMED_STATUS,
                mode: "old_wallet".to_string(),
                scenario: ScenarioName::S4.as_str().to_string(),
                amount_microtari: Some(1_000_000),
                fee_microtari: Some(945),
                mined_height: Some(710_357),
                confirmed: true,
            }],
            ..Mode1TransferSummary::default()
        };

        summary.backfill_verified_fee_total();

        assert_eq!(summary.fee_microtari, 1_000);
    }

    #[test]
    fn terminal_ok_status_matches_bounty_status_set() {
        for status in [2, 6, 9, 13] {
            assert!(terminal_ok_status(status));
        }
        for status in [1, 7, 11, 14] {
            assert!(!terminal_ok_status(status));
        }
    }

    #[test]
    fn base_node_endpoint_url_uses_http_surface() {
        assert_eq!(
            base_node_endpoint_url("https://rpc.esmeralda.tari.com", "/get_tip_info")
                .unwrap()
                .as_str(),
            "https://rpc.esmeralda.tari.com/get_tip_info"
        );
    }

    #[test]
    fn mode2_transaction_query_url_uses_kernel_signature_params() {
        let query = Mode2KernelQuery {
            excess_sig_nonce: vec![0xab, 0xcd],
            excess_sig: vec![0x12, 0x34],
            fee_microtari: Some(42),
        };
        let url = mode2_transaction_query_url("https://rpc.esmeralda.tari.com", &query).unwrap();

        assert_eq!(
            url.as_str(),
            "https://rpc.esmeralda.tari.com/transactions?excess_sig_nonce=abcd&excess_sig_sig=1234"
        );
    }

    #[test]
    fn mode2_transaction_query_status_requires_depth() {
        let mined = TxQueryResponse {
            location: TxLocation::Mined,
            mined_height: Some(100),
            mined_header_hash: None,
            mined_timestamp: None,
        };
        assert_eq!(
            mode2_transaction_query_status(&mined, Some(102), 3),
            (2, false)
        );
        assert_eq!(
            mode2_transaction_query_status(&mined, Some(103), 3),
            (TX_MINED_CONFIRMED_STATUS, true)
        );

        let mempool = TxQueryResponse {
            location: TxLocation::InMempool,
            mined_height: None,
            mined_header_hash: None,
            mined_timestamp: None,
        };
        assert_eq!(
            mode2_transaction_query_status(&mempool, Some(103), 3),
            (1, false)
        );

        let not_stored = TxQueryResponse {
            location: TxLocation::NotStored,
            mined_height: None,
            mined_header_hash: None,
            mined_timestamp: None,
        };
        assert_eq!(
            mode2_transaction_query_status(&not_stored, Some(103), 3),
            (0, false)
        );

        let none = TxQueryResponse {
            location: TxLocation::None,
            mined_height: None,
            mined_header_hash: None,
            mined_timestamp: None,
        };
        assert_eq!(
            mode2_transaction_query_status(&none, Some(103), 3),
            (0, false)
        );
    }

    #[test]
    fn mode2_kernel_query_rejects_invalid_serialized_transaction() {
        let error = mode2_kernel_query_from_serialized_transaction(b"not-json")
            .expect_err("invalid transaction must fail");
        assert!(format!("{error:#}").contains("deserializing Mode 2 transaction"));
    }

    #[test]
    fn mode2_verification_confirmed_requires_every_tx_confirmed() {
        let tx_ids = vec!["1".to_string(), "2".to_string()];
        let one_confirmed = Mode2VerificationResult {
            observed_transactions: vec![VerifiedTransaction {
                tx_id: "1".to_string(),
                status_value: TX_MINED_CONFIRMED_STATUS,
                mode: "new_wallet".to_string(),
                scenario: ScenarioName::S1.as_str().to_string(),
                amount_microtari: None,
                fee_microtari: Some(10),
                mined_height: Some(100),
                confirmed: true,
            }],
            observations: Vec::new(),
            used_base_node_query: true,
        };
        assert!(!mode2_verification_confirmed(&one_confirmed, &tx_ids));

        let all_confirmed = Mode2VerificationResult {
            observed_transactions: vec![
                one_confirmed.observed_transactions[0].clone(),
                VerifiedTransaction {
                    tx_id: "2".to_string(),
                    status_value: TX_MINED_CONFIRMED_STATUS,
                    mode: "new_wallet".to_string(),
                    scenario: ScenarioName::S1.as_str().to_string(),
                    amount_microtari: None,
                    fee_microtari: Some(10),
                    mined_height: Some(101),
                    confirmed: true,
                },
            ],
            observations: Vec::new(),
            used_base_node_query: true,
        };
        assert!(mode2_verification_confirmed(&all_confirmed, &tx_ids));
    }

    #[tokio::test]
    async fn mode2_verification_returns_immediately_without_tx_ids() {
        let config = Config::default();
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("unused-wallet.db");

        let (verification, attempts, wall_ms) =
            verify_mode2_transactions_until_confirmed(&config, &db_path, &[], ScenarioName::S4)
                .await
                .unwrap();

        assert_eq!(attempts, 0);
        assert_eq!(wall_ms, 0);
        assert!(verification.observed_transactions.is_empty());
        assert!(verification.observations.is_empty());
        assert!(!verification.used_base_node_query);
    }

    #[test]
    fn scan_checkpoint_gates_missing_prerequisites() {
        assert!(!ScanCheckpoint::PostS1Blocked.runnable());
        assert!(!ScanCheckpoint::PostS5Blocked.runnable());
        assert!(ScanCheckpoint::PostS1Partial.runnable());
        assert!(
            ScanCheckpoint::PostS1Blocked
                .blocked_note(ScenarioName::S2)
                .contains("post_s1_blocked")
        );
    }

    #[test]
    fn blocked_checkpoint_scan_records_failed_repetition() {
        let mut cell = ScenarioCell {
            scenario: ScenarioName::S6,
            surface: "minotari_library".to_string(),
            status: CellStatus::ReadyForLiveRun,
            repetitions: Vec::new(),
            median_wall_ms: None,
            spread_wall_ms: None,
            notes: Vec::new(),
        };
        let spec = FreshScanSpec {
            scenario: ScenarioName::S6,
            wallet_state: FreshScanWalletState::FundedGenesis,
            checkpoint: ScanCheckpoint::PostS5Blocked,
        };

        record_blocked_checkpoint_scan(&mut cell, spec);

        assert_eq!(cell.status, CellStatus::Failed);
        assert_eq!(cell.median_wall_ms, None);
        assert_eq!(cell.spread_wall_ms, None);
        assert_eq!(cell.repetitions.len(), 1);
        assert_eq!(cell.repetitions[0].status, CellStatus::Failed);
        assert_eq!(cell.repetitions[0].wall_ms, None);
        assert_eq!(cell.repetitions[0].success_count, 0);
        assert_eq!(cell.repetitions[0].failure_count, 1);
        assert!(
            cell.repetitions[0]
                .error
                .as_deref()
                .unwrap()
                .contains("post_s5_blocked")
        );
        let metrics = cell.repetitions[0].metrics.as_ref().unwrap();
        assert_eq!(metrics["blocked_prerequisite"], serde_json::json!(true));
        assert_eq!(
            metrics["scan_checkpoint"],
            serde_json::json!("post_s5_blocked")
        );
    }

    #[test]
    fn mode1_scan_grpc_address_offsets_port() {
        let spec = FreshScanSpec {
            scenario: ScenarioName::S3,
            wallet_state: FreshScanWalletState::FundedBirthday { birthday: 123 },
            checkpoint: ScanCheckpoint::PostS1,
        };
        assert_eq!(
            mode1_scan_grpc_address("http://127.0.0.1:18143", spec, 2).unwrap(),
            "http://127.0.0.1:18445"
        );
    }

    #[test]
    fn recipient_batches_preserve_order_without_chunks() {
        let recipients = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(
            recipient_batches(recipients, 2),
            vec![
                vec!["a".to_string(), "b".to_string()],
                vec!["c".to_string()]
            ]
        );
    }

    #[test]
    fn pp_s1_batches_follow_round_output_shape() {
        let config = Config::default();
        let rounds = s1_round_plan(&config, 65);
        let batches = s1_pp_recipient_batches(&rounds, "addr");

        assert_eq!(batches.len(), 65);
        assert_eq!(batches[0], vec!["addr".to_string(), "addr".to_string()]);
        assert_eq!(batches[63].len(), 8);
        assert_eq!(batches[64].len(), 8);
    }

    #[test]
    fn pp_scan_cells_are_not_applicable_when_companion_scans_are_disabled() {
        let config = Config::default();
        let mut profile = ResultProfile::new(&config, crate::env_capture::capture());
        profile.modes.insert(
            ModeName::OldWallet.as_str().to_string(),
            empty_mode_profile(ModeName::OldWallet, None),
        );
        profile.modes.insert(
            ModeName::NewWallet.as_str().to_string(),
            empty_mode_profile(ModeName::NewWallet, None),
        );
        profile.modes.insert(
            ModeName::PaymentProcessor.as_str().to_string(),
            empty_mode_profile(ModeName::PaymentProcessor, None),
        );

        annotate_fresh_scan_cells_disabled(&mut profile);

        let pp_b0 = &profile
            .modes
            .get(ModeName::PaymentProcessor.as_str())
            .unwrap()
            .scenarios[ScenarioName::B0.as_str()];
        assert_eq!(pp_b0.status, CellStatus::NotApplicable);
        assert!(
            pp_b0
                .notes
                .iter()
                .any(|note| note.contains("PP has no direct scan API"))
        );

        let new_b0 = &profile
            .modes
            .get(ModeName::NewWallet.as_str())
            .unwrap()
            .scenarios[ScenarioName::B0.as_str()];
        assert_eq!(new_b0.status, CellStatus::ReadyForLiveRun);
        let old_b0 = &profile
            .modes
            .get(ModeName::OldWallet.as_str())
            .unwrap()
            .scenarios[ScenarioName::B0.as_str()];
        assert!(
            old_b0
                .notes
                .iter()
                .any(|note| note.contains("fresh live scan cell disabled"))
        );
    }

    #[test]
    fn birthday_scan_metrics_include_blocks_per_second() {
        let measurement = ScanMeasurement {
            birthday: 1_635,
            max_height: 711_305,
            wall_ms: 10_000,
            available_microtari: 1,
            tip_start: Some(711_300),
            tip_end: Some(711_305),
            detected_outputs: 1,
        };
        let spec = FreshScanSpec {
            scenario: ScenarioName::S3,
            wallet_state: FreshScanWalletState::FundedBirthday { birthday: 1_635 },
            checkpoint: ScanCheckpoint::PostS1,
        };
        let metrics = measurement.metrics("new_wallet", spec);

        assert_eq!(metrics["blocks_scanned"], serde_json::json!(709_670));
        assert!(metrics["blocks_per_sec"].as_f64().unwrap() > 0.0);
    }

    #[test]
    fn pp_retry_count_is_not_terminal_progress() {
        let snapshot = PaymentProcessorDbSnapshot {
            batches: vec![payment_processor::PaymentBatchSnapshot {
                id: "batch".to_string(),
                status: "PENDING_BATCHING".to_string(),
                retry_count: 3,
                error_message: None,
                has_unsigned_tx: false,
                has_signed_tx: false,
                mined_height: None,
            }],
            payments: Vec::new(),
        };
        assert!(!pp_snapshot_has_progress_or_error(&snapshot));
    }

    #[test]
    fn pp_terminal_wait_requires_confirmed_or_failed_batches() {
        let awaiting = PaymentProcessorDbSnapshot {
            batches: vec![payment_processor::PaymentBatchSnapshot {
                id: "batch".to_string(),
                status: "AWAITING_CONFIRMATION".to_string(),
                retry_count: 0,
                error_message: None,
                has_unsigned_tx: true,
                has_signed_tx: true,
                mined_height: None,
            }],
            payments: Vec::new(),
        };
        assert!(!pp_snapshot_is_terminal_for_summary(&awaiting, 1));

        let confirmed = PaymentProcessorDbSnapshot {
            batches: vec![payment_processor::PaymentBatchSnapshot {
                id: "batch".to_string(),
                status: "CONFIRMED".to_string(),
                retry_count: 0,
                error_message: None,
                has_unsigned_tx: true,
                has_signed_tx: true,
                mined_height: Some(42),
            }],
            payments: Vec::new(),
        };
        assert!(pp_snapshot_is_terminal_for_summary(&confirmed, 1));
    }

    #[test]
    fn pp_terminal_wait_stops_on_upstream_error() {
        let snapshot = PaymentProcessorDbSnapshot {
            batches: vec![payment_processor::PaymentBatchSnapshot {
                id: "batch".to_string(),
                status: "PENDING_BATCHING".to_string(),
                retry_count: 1,
                error_message: Some("signing failed".to_string()),
                has_unsigned_tx: false,
                has_signed_tx: false,
                mined_height: None,
            }],
            payments: Vec::new(),
        };
        assert!(pp_snapshot_is_terminal_for_summary(&snapshot, 1));
    }

    #[test]
    fn mode2_summary_requires_terminal_verification_for_ok_status() {
        let config = Config::default();
        let mut profile = ResultProfile::new(&config, crate::env_capture::capture());
        profile.modes.insert(
            ModeName::NewWallet.as_str().to_string(),
            empty_mode_profile(ModeName::NewWallet, None),
        );
        let summary = ScenarioSendSummary {
            attempted: 1,
            success_count: 1,
            tx_ids: vec!["1".to_string()],
            tx_infos: vec![VerifiedTransaction {
                tx_id: "1".to_string(),
                status_value: 1,
                mode: "new_wallet".to_string(),
                scenario: ScenarioName::S1.as_str().to_string(),
                amount_microtari: None,
                fee_microtari: None,
                mined_height: None,
                confirmed: false,
            }],
            ..ScenarioSendSummary::default()
        };

        record_mode2_send_summary(&mut profile, ScenarioName::S1, &summary, Vec::new());

        let cell = &profile
            .modes
            .get(ModeName::NewWallet.as_str())
            .unwrap()
            .scenarios[ScenarioName::S1.as_str()];
        assert_eq!(cell.status, CellStatus::Failed);
        assert_eq!(profile.chain_verification.verified_transactions.len(), 0);
        let metrics = cell.repetitions[0].metrics.as_ref().unwrap();
        assert_eq!(
            metrics["observed_transactions"].as_array().unwrap().len(),
            1
        );
        assert_eq!(
            metrics["verified_transactions"].as_array().unwrap().len(),
            0
        );
    }

    #[test]
    fn mode2_summary_does_not_emit_unverified_placeholder_rows() {
        let config = Config::default();
        let mut profile = ResultProfile::new(&config, crate::env_capture::capture());
        profile.modes.insert(
            ModeName::NewWallet.as_str().to_string(),
            empty_mode_profile(ModeName::NewWallet, None),
        );
        let summary = ScenarioSendSummary {
            attempted: 1,
            success_count: 1,
            tx_ids: vec!["1".to_string()],
            tx_infos: Vec::new(),
            ..ScenarioSendSummary::default()
        };

        record_mode2_send_summary(&mut profile, ScenarioName::S1, &summary, Vec::new());

        let cell = &profile
            .modes
            .get(ModeName::NewWallet.as_str())
            .unwrap()
            .scenarios[ScenarioName::S1.as_str()];
        assert_eq!(cell.status, CellStatus::Failed);
        assert_eq!(profile.chain_verification.verified_transactions.len(), 0);
        assert_eq!(
            cell.repetitions[0].error.as_deref(),
            Some("tx_ids were produced but chain verification returned no rows")
        );
    }

    #[test]
    fn mode2_refresh_replaces_pending_repetition_and_confirmed_row() {
        let config = Config::default();
        let mut profile = ResultProfile::new(&config, crate::env_capture::capture());
        profile.modes.insert(
            ModeName::NewWallet.as_str().to_string(),
            empty_mode_profile(ModeName::NewWallet, None),
        );
        let mut summary = ScenarioSendSummary {
            attempted: 1,
            success_count: 1,
            tx_ids: vec!["42".to_string()],
            tx_infos: vec![VerifiedTransaction {
                tx_id: "42".to_string(),
                status_value: 1,
                mode: "new_wallet".to_string(),
                scenario: ScenarioName::S1.as_str().to_string(),
                amount_microtari: None,
                fee_microtari: Some(10),
                mined_height: None,
                confirmed: false,
            }],
            ..ScenarioSendSummary::default()
        };

        record_mode2_send_summary(&mut profile, ScenarioName::S1, &summary, Vec::new());

        let cell = &profile
            .modes
            .get(ModeName::NewWallet.as_str())
            .unwrap()
            .scenarios[ScenarioName::S1.as_str()];
        assert_eq!(cell.status, CellStatus::Failed);
        assert_eq!(cell.repetitions.len(), 1);
        assert_eq!(profile.chain_verification.verified_transactions.len(), 0);

        summary.tx_infos = vec![VerifiedTransaction {
            tx_id: "42".to_string(),
            status_value: TX_MINED_CONFIRMED_STATUS,
            mode: "new_wallet".to_string(),
            scenario: ScenarioName::S1.as_str().to_string(),
            amount_microtari: None,
            fee_microtari: Some(10),
            mined_height: Some(100),
            confirmed: true,
        }];
        summary.extra_metrics.insert(
            "verification_source".to_string(),
            serde_json::json!("base_node_transaction_query"),
        );

        refresh_recorded_mode2_send_summary(
            &mut profile,
            ScenarioName::S1,
            &summary,
            "post-S5 refresh".to_string(),
        );
        refresh_recorded_mode2_send_summary(
            &mut profile,
            ScenarioName::S1,
            &summary,
            "post-S5 refresh repeat".to_string(),
        );

        let cell = &profile
            .modes
            .get(ModeName::NewWallet.as_str())
            .unwrap()
            .scenarios[ScenarioName::S1.as_str()];
        assert_eq!(cell.status, CellStatus::Ok);
        assert_eq!(cell.repetitions.len(), 1);
        assert_eq!(cell.repetitions[0].status, CellStatus::Ok);
        assert_eq!(cell.repetitions[0].error, None);
        assert_eq!(profile.chain_verification.verified_transactions.len(), 1);
        assert_eq!(
            profile.chain_verification.verified_transactions[0].tx_id,
            "42"
        );
    }

    #[test]
    fn mode2_db_observed_fallback_stays_out_of_confirmed_rows() {
        let mut summary = ScenarioSendSummary {
            attempted: 1,
            success_count: 1,
            tx_ids: vec!["42".to_string()],
            ..ScenarioSendSummary::default()
        };
        summary.apply_mode2_verification(Mode2VerificationResult {
            observed_transactions: vec![VerifiedTransaction {
                tx_id: "42".to_string(),
                status_value: 1,
                mode: "new_wallet".to_string(),
                scenario: ScenarioName::S1.as_str().to_string(),
                amount_microtari: None,
                fee_microtari: Some(10),
                mined_height: None,
                confirmed: false,
            }],
            observations: vec![serde_json::json!({
                "tx_id": "42",
                "verification_source": "wallet_db_observed",
                "wallet_db_status": "broadcast",
                "confirmed": false
            })],
            used_base_node_query: false,
        });

        let metrics = summary.metrics(ScenarioName::S1);
        assert_eq!(
            metrics["verification_source"],
            serde_json::json!("wallet_db_observed")
        );
        assert_eq!(
            metrics["verification_observations"][0]["wallet_db_status"],
            serde_json::json!("broadcast")
        );
        assert!(summary.verified_transactions().is_empty());
    }

    #[test]
    fn mode2_completed_transaction_status_matches_pinned_minotari_strings() {
        assert_eq!(
            mode2_completed_transaction_status("mined_confirmed"),
            (TX_MINED_CONFIRMED_STATUS, true)
        );
        assert_eq!(
            mode2_completed_transaction_status("mined_unconfirmed"),
            (2, false)
        );
        assert_eq!(mode2_completed_transaction_status("broadcast"), (1, false));
        assert_eq!(mode2_completed_transaction_status("completed"), (0, false));
        assert_eq!(mode2_completed_transaction_status("rejected"), (7, false));
        assert_eq!(mode2_completed_transaction_status("canceled"), (14, false));
    }

    #[test]
    fn pp_summary_requires_terminal_verification_for_ok_status() {
        let config = Config::default();
        let mut profile = ResultProfile::new(&config, crate::env_capture::capture());
        profile.modes.insert(
            ModeName::PaymentProcessor.as_str().to_string(),
            empty_mode_profile(ModeName::PaymentProcessor, None),
        );
        let summary = PpScenarioSummary {
            attempted_batches: 1,
            attempted_payments: 1,
            accepted_batches: 1,
            batch_ids: vec!["batch".to_string()],
            db_snapshot: Some(PaymentProcessorDbSnapshot {
                batches: vec![payment_processor::PaymentBatchSnapshot {
                    id: "batch".to_string(),
                    status: "AWAITING_CONFIRMATION".to_string(),
                    retry_count: 0,
                    error_message: None,
                    has_unsigned_tx: true,
                    has_signed_tx: true,
                    mined_height: None,
                }],
                payments: Vec::new(),
            }),
            ..PpScenarioSummary::default()
        };

        record_pp_summary(&mut profile, ScenarioName::S5, &summary, Vec::new());

        let cell = &profile
            .modes
            .get(ModeName::PaymentProcessor.as_str())
            .unwrap()
            .scenarios[ScenarioName::S5.as_str()];
        assert_eq!(cell.status, CellStatus::Failed);
        assert_eq!(profile.chain_verification.verified_transactions.len(), 0);
        assert_eq!(
            cell.repetitions[0].error.as_deref(),
            Some(
                "PP batches were accepted but payment_processor_db_observed returned no confirmed rows"
            )
        );
    }

    #[test]
    fn pp_summary_emits_only_confirmed_observed_batches() {
        let config = Config::default();
        let mut profile = ResultProfile::new(&config, crate::env_capture::capture());
        profile.modes.insert(
            ModeName::PaymentProcessor.as_str().to_string(),
            empty_mode_profile(ModeName::PaymentProcessor, None),
        );
        let summary = PpScenarioSummary {
            attempted_batches: 2,
            attempted_payments: 2,
            accepted_batches: 2,
            batch_ids: vec!["confirmed".to_string(), "pending".to_string()],
            db_snapshot: Some(PaymentProcessorDbSnapshot {
                batches: vec![
                    payment_processor::PaymentBatchSnapshot {
                        id: "confirmed".to_string(),
                        status: "CONFIRMED".to_string(),
                        retry_count: 0,
                        error_message: None,
                        has_unsigned_tx: true,
                        has_signed_tx: true,
                        mined_height: Some(42),
                    },
                    payment_processor::PaymentBatchSnapshot {
                        id: "pending".to_string(),
                        status: "PENDING_BATCHING".to_string(),
                        retry_count: 0,
                        error_message: None,
                        has_unsigned_tx: false,
                        has_signed_tx: false,
                        mined_height: None,
                    },
                ],
                payments: Vec::new(),
            }),
            ..PpScenarioSummary::default()
        };

        record_pp_summary(&mut profile, ScenarioName::S1, &summary, Vec::new());

        let cell = &profile
            .modes
            .get(ModeName::PaymentProcessor.as_str())
            .unwrap()
            .scenarios[ScenarioName::S1.as_str()];
        assert_eq!(cell.status, CellStatus::Failed);
        assert_eq!(profile.chain_verification.verified_transactions.len(), 1);
        assert_eq!(
            profile.chain_verification.verified_transactions[0].tx_id,
            "confirmed"
        );
        let metrics = cell.repetitions[0].metrics.as_ref().unwrap();
        assert_eq!(
            metrics["verification_source"],
            serde_json::json!("payment_processor_db_observed")
        );
    }
}
