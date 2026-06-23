#![cfg(feature = "live-minotari")]

use std::{
    fs,
    path::{Path, PathBuf},
    str::FromStr,
    time::{Duration, Instant},
};

use anyhow::Context;
use minotari::{
    ScanMode, Scanner,
    db::{self, get_latest_scanned_tip_block_by_account},
    get_accounts, get_balance, init_db,
    models::PendingTransactionStatus,
    transactions::{manager::TransactionSender, one_sided_transaction::Recipient},
    utils::init_wallet::init_with_seed_words,
};
use tari_common::configuration::Network;
use tari_common_types::seeds::cipher_seed::CipherSeed;
use tari_common_types::tari_address::TariAddress;
use tari_transaction_components::{
    MicroMinotari,
    consensus::ConsensusConstantsBuilder,
    offline_signing::{models::SignedOneSidedTransactionResult, sign_locked_transaction},
    rpc::models::{TxSubmissionRejectionReason, TxSubmissionResponse},
};
use tari_utilities::ByteArray;

use crate::{
    config::{Config, parse_amount},
    modes::ScenarioName,
    result_profile::{CellStatus, Repetition, ResultProfile, ScenarioCell},
    seeds::{
        AddressBook, WalletRole, current_birthday, seed_from_words, seed_from_words_with_birthday,
    },
};

pub async fn annotate_profile_with_library_smoke(
    config: &Config,
    book: &AddressBook,
    profile: &mut ResultProfile,
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
                });
            }
        }
    }

    if config.benchmark.mode2_send_smoke {
        annotate_mode2_send_smoke(config, book, profile).await?;
    } else if let Some(mode) = profile.modes.get_mut("new_wallet")
        && let Some(cell) = mode.scenarios.get_mut("S1")
    {
        cell.notes.push(
            "Mode 2 send smoke disabled; set benchmark.mode2_send_smoke=true to run the opt-in tiny spend"
                .to_string(),
        );
    }

    if config.benchmark.live_fresh_scan_cells {
        annotate_fresh_scan_cells(config, book, profile).await?;
    } else {
        annotate_fresh_scan_cells_disabled(profile);
    }
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

async fn annotate_fresh_scan_cells(
    config: &Config,
    book: &AddressBook,
    profile: &mut ResultProfile,
) -> anyhow::Result<()> {
    let birthday = current_birthday();

    if let Some(new_wallet) = book.addresses.get(WalletRole::NewWallet.label()) {
        annotate_mode_scan_cells(
            config,
            profile,
            "new_wallet",
            Some(&new_wallet.seed_words),
            birthday,
        )
        .await?;
    }

    if let Some(pp_wallet) = book.addresses.get(WalletRole::PaymentProcessor.label()) {
        annotate_mode_scan_cells(
            config,
            profile,
            "payment_processor",
            Some(&pp_wallet.seed_words),
            birthday,
        )
        .await?;
    }

    Ok(())
}

fn annotate_fresh_scan_cells_disabled(profile: &mut ResultProfile) {
    for mode in ["new_wallet", "payment_processor"] {
        let Some(mode_profile) = profile.modes.get_mut(mode) else {
            continue;
        };
        for scenario in [
            ScenarioName::B0,
            ScenarioName::S2,
            ScenarioName::S3,
            ScenarioName::S6,
            ScenarioName::S7,
        ] {
            if let Some(cell) = mode_profile.scenarios.get_mut(scenario.as_str()) {
                cell.notes.push(
                    "fresh live scan cell disabled for this run; set benchmark.live_fresh_scan_cells=true for the long baseline pass"
                        .to_string(),
                );
            }
        }
    }
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
                });
            }
        }
    }

    Ok(())
}

async fn annotate_mode_scan_cells(
    config: &Config,
    profile: &mut ResultProfile,
    mode: &str,
    funded_seed_words: Option<&str>,
    birthday: u16,
) -> anyhow::Result<()> {
    let Some(mode_profile) = profile.modes.get_mut(mode) else {
        return Ok(());
    };

    let scan_specs = [
        FreshScanSpec {
            scenario: ScenarioName::B0,
            wallet_state: FreshScanWalletState::EmptyGenesis,
        },
        FreshScanSpec {
            scenario: ScenarioName::S2,
            wallet_state: FreshScanWalletState::FundedGenesis,
        },
        FreshScanSpec {
            scenario: ScenarioName::S3,
            wallet_state: FreshScanWalletState::FundedBirthday { birthday },
        },
    ];

    for spec in scan_specs {
        let Some(cell) = mode_profile.scenarios.get_mut(spec.scenario.as_str()) else {
            continue;
        };
        run_fresh_scan_cell(config, mode, funded_seed_words, spec, cell).await?;
    }

    for scenario in [ScenarioName::S6, ScenarioName::S7] {
        if let Some(cell) = mode_profile.scenarios.get_mut(scenario.as_str()) {
            cell.notes.push(
                "requires post-S5 checkpoint; left ready until send-side scenarios run".to_string(),
            );
        }
    }

    Ok(())
}

async fn run_fresh_scan_cell(
    config: &Config,
    mode: &str,
    funded_seed_words: Option<&str>,
    spec: FreshScanSpec,
    cell: &mut ScenarioCell,
) -> anyhow::Result<()> {
    for run in 1..=config.benchmark.repetitions {
        println!(
            "live scan {mode}/{} run {run}/{} birthday={} starting",
            spec.scenario.as_str(),
            config.benchmark.repetitions,
            spec.birthday()
        );
        let scan = run_fresh_scan(config, mode, spec, run, funded_seed_words).await;
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
                });
            }
        }
    }

    Ok(())
}

async fn run_fresh_scan(
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

    let wall_ms = scan_to_tip(
        &db_path,
        &password,
        &config.network.base_node_http_url,
        config.benchmark.scan_batch_size,
        config.benchmark.c_min,
    )
    .await?;
    let account = account_snapshot(&db_path)?;

    Ok(ScanMeasurement {
        wall_ms,
        birthday: spec.birthday(),
        max_height: account.max_height,
        available_microtari: account.available_microtari,
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

pub struct OneSidedSendOutcome {
    pub tx_id: String,
    pub fee_microtari: u64,
    pub accepted: bool,
    pub is_synced: bool,
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
    let signed = sign_locked_transaction(&key_manager, constants, Network::Esmeralda, unsigned)?;
    finalize_transaction_and_broadcast_without_retry(&sender, signed, request).await
}

async fn finalize_transaction_and_broadcast_without_retry(
    sender: &TransactionSender,
    signed: SignedOneSidedTransactionResult,
    request: OneSidedSendRequest<'_>,
) -> anyhow::Result<OneSidedSendOutcome> {
    persist_signed_transaction(sender, &signed)?;
    let tx_id = signed.signed_transaction.tx_id;
    let fee_microtari = signed.request.info.fee.0;
    let submission = submit_transaction_without_retry(
        request.base_node_url,
        signed.signed_transaction.transaction,
        request.request_timeout,
    )
    .await;

    let conn = sender.db_pool.get()?;
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
                sender.processed_transactions.id(),
                PendingTransactionStatus::Expired,
            )?;
            db::unlock_outputs_for_pending_transaction(&conn, sender.processed_transactions.id())?;
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
                sender.processed_transactions.id(),
                PendingTransactionStatus::Expired,
            )?;
            db::unlock_outputs_for_pending_transaction(&conn, sender.processed_transactions.id())?;
            Err(error)
        }
    }
}

fn persist_signed_transaction(
    sender: &TransactionSender,
    signed: &SignedOneSidedTransactionResult,
) -> anyhow::Result<()> {
    let conn = sender.db_pool.get()?;
    let pending_tx_id = sender.processed_transactions.id();
    if pending_tx_id.is_empty() {
        anyhow::bail!("pending transaction id missing before broadcast");
    }

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
        sender.account.id,
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
}

#[derive(Debug, Clone, Copy)]
enum FreshScanWalletState {
    EmptyGenesis,
    FundedGenesis,
    FundedBirthday { birthday: u16 },
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
}

impl ScanMeasurement {
    fn note(&self) -> String {
        format!(
            "fresh scan birthday={} max_height={} available_microtari={}",
            self.birthday, self.max_height, self.available_microtari
        )
    }
}
