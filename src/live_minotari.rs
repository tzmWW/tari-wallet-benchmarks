#![cfg(feature = "live-minotari")]

use std::{path::Path, str::FromStr};

use anyhow::Context;
use minotari::{
    ScanMode, Scanner, get_accounts, get_balance, init_db,
    transactions::{manager::TransactionSender, one_sided_transaction::Recipient},
    utils::init_wallet::init_with_seed_words,
};
use tari_common::configuration::Network;
use tari_common_types::tari_address::TariAddress;
use tari_transaction_components::{
    MicroMinotari, consensus::ConsensusConstantsBuilder, offline_signing::sign_locked_transaction,
};

use crate::{
    config::Config,
    result_profile::{CellStatus, Repetition, ResultProfile},
    seeds::{AddressBook, WalletRole, seed_from_words},
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
        config.benchmark.c_min,
    )
    .await;

    if let Some(mode) = profile.modes.get_mut("new_wallet")
        && let Some(cell) = mode.scenarios.get_mut("B0")
    {
        match scan {
            Ok(wall_ms) => {
                cell.status = CellStatus::Ok;
                cell.repetitions.push(Repetition {
                    run: 1,
                    status: CellStatus::Ok,
                    wall_ms: Some(wall_ms),
                    success_count: 1,
                    failure_count: 0,
                    fee_microtari: None,
                    error: None,
                });
                cell.median_wall_ms = Some(wall_ms);
                cell.spread_wall_ms = Some(0);
            }
            Err(error) => {
                cell.status = CellStatus::Failed;
                cell.repetitions.push(Repetition {
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
    required_confirmations: u64,
) -> anyhow::Result<u128> {
    let start = std::time::Instant::now();
    Scanner::new(
        password,
        base_url,
        db_path.to_path_buf(),
        100,
        required_confirmations,
    )
    .account("default")
    .mode(ScanMode::Full)
    .run()
    .await
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(start.elapsed().as_millis())
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

pub struct OneSidedSendRequest<'a> {
    pub db_path: &'a Path,
    pub password: &'a str,
    pub base_node_url: &'a str,
    pub recipient: &'a str,
    pub amount: MicroMinotari,
    pub fee_rate: MicroMinotari,
    pub seconds_to_lock: u64,
    pub confirmation_window: u64,
}

pub async fn construct_sign_broadcast_one_sided(
    request: OneSidedSendRequest<'_>,
) -> anyhow::Result<String> {
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
    let tx_id = signed.signed_transaction.tx_id.to_string();
    sender
        .finalize_transaction_and_broadcast(signed, request.base_node_url.to_string())
        .await?;
    Ok(tx_id)
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
