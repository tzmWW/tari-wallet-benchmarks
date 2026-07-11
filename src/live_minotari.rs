#![cfg(feature = "live-minotari")]

use std::{
    collections::BTreeMap,
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

mod mode1;
mod mode2;
mod mode3;
mod scan;
mod verification;

use mode1::{
    STEALTH_OUTPUT_GRAMS, annotate_mode1_console_wallet, exact_no_change_split,
    exact_pp_split_with_change, grpc_bind_multiaddr, mode1_scan_grpc_address, mode1_unspent_count,
    mode1_wallet_birthday, old_wallet_base_path, start_mode1_console_wallet_with_recovery,
    wait_for_mode1_grpc_address, wait_for_mode1_scan_to_tip,
};
use mode2::{
    annotate_mode2_live_scenarios, annotate_mode2_send_smoke, base_node_endpoint_url,
    base_node_http_client, base_node_tip_height, base_node_tip_height_with_client, capped_attempts,
    mode2_completed_transaction_status, repeated_recipient, settle_gate_pause,
};
#[cfg(test)]
use mode2::{
    mode2_settle_gate_ready, mode2_verification_confirmed, record_mode2_send_summary,
    refresh_recorded_mode2_send_summary, verify_mode2_transactions_until_confirmed,
};
use mode3::{annotate_mode3_payment_processor, pp_snapshot_is_terminal_for_summary};
#[cfg(test)]
use mode3::{pp_observation_timeout, recipient_batches, record_pp_summary};
use scan::{
    ResourcePeaks, account_balance, account_snapshot, amount_field_as_microtari,
    record_blocked_prerequisite_cells, run_library_checkpoint_scan_cells,
    run_library_fresh_scan_for_cell, run_mode1_checkpoint_scan_cells,
    run_mode1_fresh_scan_for_cell, spendable_output_amounts, spendable_output_count,
};
#[cfg(test)]
use scan::{record_blocked_checkpoint_scan, record_blocked_prerequisite_cell};
#[cfg(test)]
use verification::mode2_transaction_query_url;
use verification::{
    mode2_completed_transaction_row, mode2_kernel_query_from_serialized_transaction,
    mode2_transaction_query_status, query_mode2_transaction, wait_for_mode1_summary_verification,
};

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

pub async fn scan_wallet_db(
    config: &Config,
    db_path: &Path,
    seed_env: Option<&str>,
    birthday: Option<u16>,
) -> anyhow::Result<()> {
    if let Some(seed_env) = seed_env {
        let seed_words = std::env::var(seed_env)
            .with_context(|| format!("reading seed words from env var {seed_env}"))?;
        let seed_words = match birthday {
            Some(birthday) => seed_words_with_birthday(&seed_words, birthday)?,
            None => seed_words,
        };
        ensure_signing_wallet(db_path, &seed_words, &config.seeds.wallet_password_env)?;
    } else if !db_path.exists() {
        bail!(
            "wallet DB not found at {}; pass --seed-env to initialize it before scanning",
            db_path.display()
        );
    }
    let scan_report = scan_to_tip(
        db_path,
        &wallet_password(&config.seeds.wallet_password_env)?,
        &config.network.base_node_http_url,
        config.benchmark.scan_batch_size,
        config.benchmark.c_min,
        config.timeout(config.timeouts.scan_batch_secs),
    )
    .await?;
    let balance = account_balance(db_path)?;
    println!(
        "scan-wallet db={} wall_ms={} no_progress_attempts={} stopped_without_progress={} balance={}",
        db_path.display(),
        scan_report.wall_ms,
        scan_report.no_progress_attempts,
        scan_report.stopped_without_progress,
        balance
    );
    Ok(())
}

pub async fn recover_mode1_console_wallet(config: &Config) -> anyhow::Result<()> {
    let book = AddressBook::load_required(config)?;
    let old_seed = book
        .addresses
        .get(WalletRole::OldWallet.label())
        .context("old wallet seed material missing")?;
    let mut context = start_mode1_console_wallet_with_recovery(config, old_seed, true).await?;
    let spendable_count = mode1_unspent_count(&mut context.client).await.ok();
    let balance = context
        .balance
        .as_ref()
        .context("Mode 1 balance missing after recovery")?;
    println!(
        "recover-mode1-wallet db={} birthday={} available={} pending_in={} pending_out={} spendable_count={:?}",
        old_wallet_base_path(config)
            .join("esmeralda/data/wallet/db/console_wallet.db")
            .display(),
        context.birthday,
        balance.available_balance,
        balance.pending_incoming_balance,
        balance.pending_outgoing_balance,
        spendable_count
    );
    Ok(())
}

/// Proves that the configured console-wallet database opens as the address
/// derived from the loaded benchmark seed. This intentionally uses the same
/// gRPC surface as Mode 1 rather than reading or repairing encrypted wallet
/// state directly.
pub async fn verify_mode1_wallet_identity(
    config: &Config,
    seed: &crate::seeds::SeedMaterial,
) -> anyhow::Result<()> {
    let mut context = start_mode1_console_wallet_with_recovery(config, seed, false).await?;
    let actual = context
        .client
        .get_address(grpc::Empty {})
        .await
        .context("querying Mode 1 wallet address over gRPC")?
        .into_inner()
        .one_sided_address;
    if !mode1_address_matches_seed(&actual, seed)? {
        bail!("Mode 1 console-wallet address does not match the configured old_wallet seed");
    }
    println!("old_wallet: console-wallet gRPC address matches the configured seed");
    Ok(())
}

fn mode1_address_matches_seed(
    actual_one_sided_address: &[u8],
    seed: &crate::seeds::SeedMaterial,
) -> anyhow::Result<bool> {
    let expected = TariAddress::from_str(&seed.address)
        .context("decoding configured Mode 1 seed address")?
        .to_vec();
    Ok(actual_one_sided_address == expected)
}

pub async fn query_wallet_transaction(
    config: &Config,
    db_path: &Path,
    tx_id: u64,
) -> anyhow::Result<()> {
    let conn = Connection::open(db_path)
        .with_context(|| format!("opening wallet DB {}", db_path.display()))?;
    let row = mode2_completed_transaction_row(&conn, tx_id as i64)?.with_context(|| {
        format!(
            "completed transaction {tx_id} not found in {}",
            db_path.display()
        )
    })?;
    let kernel_query = mode2_kernel_query_from_serialized_transaction(&row.serialized_transaction)?;
    let client = base_node_http_client()?;
    let response =
        query_mode2_transaction(&client, &config.network.base_node_http_url, &kernel_query).await?;
    let tip_height = base_node_tip_height_with_client(&client, &config.network.base_node_http_url)
        .await
        .ok();
    let (_, confirmed) =
        mode2_transaction_query_status(&response, tip_height, config.benchmark.c_min);

    let output = serde_json::json!({
        "tx_id": tx_id.to_string(),
        "wallet_db_status": row.status,
        "wallet_db_mined_height": row.mined_height,
        "wallet_db_confirmation_height": row.confirmation_height,
        "base_node_query_location": format!("{:?}", response.location),
        "base_node_query_mined_height": response.mined_height,
        "base_node_tip_height": tip_height,
        "confirmed": confirmed,
        "fee_microtari": kernel_query.fee_microtari,
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

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
        config.timeout(config.timeouts.scan_batch_secs),
    )
    .await
    .and_then(|scan_report| {
        let balance = account_balance(db_path)?;
        let available = amount_field_as_microtari(&balance, "available")
            .with_context(|| format!("available balance missing from {balance}"))?;
        let expected = config.a_fund()?.0;
        let spendable_count = spendable_output_count(db_path).ok();
        let (status, success_count, failure_count, error, mut metrics) =
            strict_s0_status(expected, available, spendable_count);
        if let serde_json::Value::Object(map) = &mut metrics {
            scan_report.insert_metrics(map);
        }
        Ok((
            scan_report.wall_ms,
            available,
            balance,
            status,
            success_count,
            failure_count,
            error,
            metrics,
        ))
    });

    if let Some(mode) = profile.modes.get_mut("new_wallet")
        && let Some(cell) = mode.scenarios.get_mut("S0")
    {
        match scan {
            Ok((
                wall_ms,
                available,
                balance,
                status,
                success_count,
                failure_count,
                error,
                metrics,
            )) => {
                cell.record_repetition(Repetition {
                    run: 1,
                    status,
                    wall_ms: Some(wall_ms),
                    success_count,
                    failure_count,
                    fee_microtari: None,
                    error,
                    metrics: Some(metrics),
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
    profile: &mut ResultProfile,
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
    profile.mark_checkpoint_stage(label);
    profile.refresh_computed_deltas();
    profile.write_validated_atomic(&checkpoint_path, false)?;
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
    no_progress_timeout: Duration,
) -> anyhow::Result<ScanToTipReport> {
    let start = std::time::Instant::now();
    let target_tip = base_node_tip_height(base_url).await?;
    let configured_chunk_size = batch_size.clamp(1, 100);
    let mut chunk_size = configured_chunk_size;
    let mut used_single_block_fallback = false;
    let mut fallback_blocks_remaining = 0u64;
    let mut last_height = account_snapshot(db_path)
        .map(|snapshot| snapshot.max_height)
        .unwrap_or_default();
    let mut no_progress_since = None;
    let mut no_progress_attempts = 0u64;
    let mut stopped_without_progress = false;
    let mut last_more_blocks = None;

    // Full scans can report completion before all downloaded batches are processed;
    // bounded partial scans force progress to be committed before each continuation.
    loop {
        // Stateful wallets must process the fixed target tip itself so mined
        // change is promoted to spendable. Confirmation-lag tolerance belongs
        // in measurement validation, not in the controller catch-up loop.
        if last_height >= target_tip {
            break;
        }

        let (_, more_blocks) = Scanner::new(
            password,
            base_url,
            db_path.to_path_buf(),
            chunk_size,
            required_confirmations,
        )
        .account("default")
        .mode(ScanMode::Partial {
            max_blocks: chunk_size,
        })
        .run()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        last_more_blocks = Some(more_blocks);

        let current_height = account_snapshot(db_path)?.max_height;
        if current_height <= last_height {
            no_progress_attempts = no_progress_attempts.saturating_add(1);
            let first_no_progress = no_progress_since.get_or_insert_with(Instant::now);
            let elapsed_without_progress = first_no_progress.elapsed();
            println!(
                "wallet scan made no progress below tip: max_height={current_height} target_tip={target_tip} more_blocks={more_blocks} no_progress_attempts={no_progress_attempts} elapsed_without_progress_ms={}",
                elapsed_without_progress.as_millis()
            );
            if chunk_size > 1 {
                chunk_size = 1;
                fallback_blocks_remaining = 16;
                used_single_block_fallback = true;
                println!(
                    "wallet scan switching from configured chunk {} to one-block progress fallback",
                    configured_chunk_size
                );
            }
            if elapsed_without_progress >= no_progress_timeout {
                stopped_without_progress = true;
                break;
            }
            let remaining = no_progress_timeout.saturating_sub(elapsed_without_progress);
            let sleep_for = Duration::from_secs(10).min(remaining);
            if !sleep_for.is_zero() {
                settle_gate_pause(sleep_for).await;
            }
            continue;
        }
        no_progress_since = None;
        last_height = current_height;
        // Stay in boundary-breaking mode for a short successful window before
        // probing the configured chunk again. Immediate probing repeated the
        // same failed large request at every two-block boundary on Esmeralda.
        if chunk_size == 1 && configured_chunk_size > 1 {
            fallback_blocks_remaining = fallback_blocks_remaining.saturating_sub(1);
            if fallback_blocks_remaining == 0 {
                chunk_size = configured_chunk_size;
            }
        }
    }
    Ok(ScanToTipReport {
        wall_ms: start.elapsed().as_millis(),
        target_tip,
        max_height: last_height,
        no_progress_attempts,
        stopped_without_progress,
        last_more_blocks,
        used_single_block_fallback,
    })
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
            cell.status = CellStatus::NotApplicable;
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
            cell.status = CellStatus::NotApplicable;
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
            cell.status = CellStatus::NotApplicable;
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
            cell.status = CellStatus::NotApplicable;
            cell.notes.push(
                "Mode 1 console-wallet topology disabled; set benchmark.mode1_live_topology=true to spawn minotari_console_wallet with gRPC"
                    .to_string(),
            );
        }
    }
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
        .extend(summary.tx_infos.iter().filter(|tx| tx.confirmed).cloned());
    let Some(mode) = profile.modes.get_mut("old_wallet") else {
        return;
    };
    let Some(cell) = mode.scenarios.get_mut(scenario.as_str()) else {
        return;
    };
    let verification_complete =
        summary.tx_ids.is_empty() || summary.tx_infos.len() >= summary.tx_ids.len();
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

fn strict_s0_status(
    expected: u64,
    available: u64,
    spendable_count: Option<u64>,
) -> (CellStatus, u32, u32, Option<String>, serde_json::Value) {
    let balance_ok = available == expected;
    let count_ok = spendable_count == Some(1);
    let ok = balance_ok && count_ok;
    let error = (!ok).then(|| {
        format!(
            "S0 verification failed: expected exactly 1 spendable UTXO and available balance {expected} µT; observed_spendable_count={spendable_count:?} observed_available={available} µT"
        )
    });
    (
        if ok {
            CellStatus::Ok
        } else {
            CellStatus::Failed
        },
        if ok { 1 } else { 0 },
        if ok { 0 } else { 1 },
        error,
        serde_json::json!({
            "verification_source": "wallet_state_observed",
            "expected_spendable_count": 1,
            "observed_spendable_count": spendable_count,
            "expected_available_microtari": expected,
            "available_microtari": available,
            "balance_matches_expected": balance_ok,
            "spendable_count_matches_expected": count_ok
        }),
    )
}

fn add_balance_reconciliation_metrics(
    metrics: &mut serde_json::Map<String, serde_json::Value>,
    balance_before: Option<u64>,
    balance_after: Option<u64>,
    outgoing_microtari: u64,
    fee_microtari: u64,
) {
    metrics.insert(
        "balance_before_microtari".to_string(),
        serde_json::json!(balance_before),
    );
    metrics.insert(
        "balance_after_microtari".to_string(),
        serde_json::json!(balance_after),
    );
    metrics.insert(
        "outgoing_microtari".to_string(),
        serde_json::json!(outgoing_microtari),
    );
    if let (Some(before), Some(after)) = (balance_before, balance_after) {
        let debit = outgoing_microtari.saturating_add(fee_microtari);
        let expected = before.saturating_sub(debit);
        let delta = expected as i128 - after as i128;
        metrics.insert(
            "balance_reconciliation".to_string(),
            serde_json::json!({
                "expected_balance_microtari": expected,
                "observed_balance_microtari": after,
                "delta_microtari": delta,
                "flagged": delta != 0,
                "assumption": "expected = balance_before - outgoing_microtari - fee_microtari"
            }),
        );
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
    pub rejection_reason: Option<String>,
    pub construction_ms: u128,
    pub broadcast_to_mempool_ms: Option<u128>,
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
    tx_timings: Vec<serde_json::Value>,
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
    tx_timings: Vec<serde_json::Value>,
    tx_infos: Vec<VerifiedTransaction>,
    construction_complete_ms: Vec<u128>,
    extra_metrics: serde_json::Map<String, serde_json::Value>,
}

struct Mode1BatchSummary {
    configured_batch: u32,
    attempted_batches: u32,
    attempted_payments: u32,
    success_count: u32,
    failure_count: u32,
    wall_ms: u128,
    fee_microtari: u64,
    tx_ids: Vec<String>,
}

struct Mode1TransferOutcome {
    success_count: u32,
    failure_count: u32,
    fee_microtari: u64,
    tx_ids: Vec<String>,
    errors: Vec<String>,
    tx_timings: Vec<serde_json::Value>,
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
    accepted_payments: u32,
    failed_batches: u32,
    wall_ms: u128,
    batch_ids: Vec<String>,
    payment_ids: Vec<String>,
    errors: Vec<String>,
    batch_summaries: Vec<PpBatchSummary>,
    db_snapshot: Option<PaymentProcessorDbSnapshot>,
    events: Vec<serde_json::Value>,
    tx_timings: Vec<serde_json::Value>,
    blocked_upstream: bool,
    construction_complete_ms: Vec<u128>,
    extra_metrics: serde_json::Map<String, serde_json::Value>,
    chain_proofs: BTreeMap<String, PpChainProof>,
}

#[derive(Debug, Clone)]
struct PpChainProof {
    chain_tx_id: String,
    fee_microtari: u64,
    mined_height: u64,
    tip_height: u64,
    confirmations: u64,
    min_confirmations: u64,
}

struct PpBatchSummary {
    configured_batch: u32,
    attempted_batches: u32,
    accepted_batches: u32,
    accepted_payments: u32,
    failed_batches: u32,
    wall_ms: u128,
}

struct PpBatchSubmission {
    batch_id: String,
    payment_ids: Vec<String>,
    raw_response: serde_json::Value,
    api_accept_ms: u128,
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

fn fee_per_recipient(fee_microtari: Option<u64>, recipients: u32) -> Option<f64> {
    let fee = fee_microtari?;
    if recipients == 0 {
        return None;
    }
    Some(fee as f64 / f64::from(recipients))
}

fn blocks_consumed_for_tx_ids(tx_infos: &[VerifiedTransaction], tx_ids: &[String]) -> Option<u64> {
    let mut heights = tx_infos
        .iter()
        .filter(|tx| tx_ids.iter().any(|id| id == &tx.tx_id))
        .filter_map(|tx| tx.mined_height)
        .collect::<Vec<_>>();
    if heights.is_empty() {
        return None;
    }
    heights.sort_unstable();
    Some(
        heights
            .last()
            .copied()
            .unwrap_or_default()
            .saturating_sub(heights.first().copied().unwrap_or_default())
            .saturating_add(1),
    )
}

fn blocks_consumed_from_heights(heights: impl Iterator<Item = u64>) -> Option<u64> {
    let mut heights = heights.collect::<Vec<_>>();
    if heights.is_empty() {
        return None;
    }
    heights.sort_unstable();
    Some(
        heights
            .last()
            .copied()
            .unwrap_or_default()
            .saturating_sub(heights.first().copied().unwrap_or_default())
            .saturating_add(1),
    )
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
    matches!(status_value, TX_MINED_CONFIRMED_STATUS | 9 | 13)
}

fn mode1_summary_verification_complete(summary: &Mode1TransferSummary) -> bool {
    !summary.tx_ids.is_empty()
        && summary.tx_infos.len() >= summary.tx_ids.len()
        && summary.tx_infos.iter().all(|tx| tx.confirmed)
}

fn mode1_s1_complete(summary: &Mode1TransferSummary) -> bool {
    summary.attempted_batches > 0
        && summary.failure_count == 0
        && summary.success_count == summary.attempted_batches
        && mode1_summary_verification_complete(summary)
}

fn mode1_send_complete(summary: &Mode1TransferSummary) -> bool {
    summary.attempted_payments > 0
        && summary.failure_count == 0
        && summary.success_count == summary.attempted_payments
        && mode1_summary_verification_complete(summary)
}

fn mode2_summary_complete(summary: &ScenarioSendSummary) -> bool {
    summary.attempted > 0
        && summary.failure_count == 0
        && summary.success_count == summary.attempted
        && !summary.tx_ids.is_empty()
        && summary.tx_infos.len() >= summary.tx_ids.len()
        && summary.tx_infos.iter().all(|tx| tx.confirmed)
}

fn pp_summary_complete(summary: &PpScenarioSummary) -> bool {
    let accepted_batch_count = usize::try_from(summary.accepted_batches).unwrap_or(usize::MAX);
    summary.attempted_batches > 0
        && summary.failed_batches == 0
        && summary.accepted_batches == summary.attempted_batches
        && !summary.blocked_upstream
        && summary.db_snapshot.as_ref().is_some_and(|snapshot| {
            snapshot
                .batches
                .iter()
                .filter(|batch| batch.status == "CONFIRMED")
                .count()
                >= accepted_batch_count
        })
        && summary.chain_proofs.len() >= accepted_batch_count
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
                self.accepted_payments = self.accepted_payments.saturating_add(
                    u32::try_from(submission.payment_ids.len()).unwrap_or(u32::MAX),
                );
                let batch_id = submission.batch_id.clone();
                self.tx_timings.push(serde_json::json!({
                    "batch_index": batch_index,
                    "batch_id": batch_id,
                    "payment_count": submission.payment_ids.len(),
                    "api_accept_ms": submission.api_accept_ms,
                    "broadcast_to_mempool_ms": null,
                    "broadcast_to_mempool_unavailable_reason": "payment_processor_api_acceptance_precedes_worker_broadcast_and_exposes_no_per_batch_mempool_timestamp"
                }));
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
        let batch_extra_metrics = batch.extra_metrics.clone();
        self.attempted_batches = self
            .attempted_batches
            .saturating_add(batch.attempted_batches);
        self.attempted_payments = self
            .attempted_payments
            .saturating_add(batch.attempted_payments);
        self.accepted_batches = self.accepted_batches.saturating_add(batch.accepted_batches);
        self.accepted_payments = self
            .accepted_payments
            .saturating_add(batch.accepted_payments);
        self.failed_batches = self.failed_batches.saturating_add(batch.failed_batches);
        self.batch_ids.extend(batch.batch_ids);
        self.payment_ids.extend(batch.payment_ids);
        self.errors.extend(batch.errors);
        self.events.extend(batch.events);
        self.tx_timings.extend(batch.tx_timings);
        self.construction_complete_ms
            .extend(batch.construction_complete_ms);
        self.extra_metrics.extend(batch.extra_metrics);
        self.chain_proofs.extend(batch.chain_proofs);
        self.extra_metrics.insert(
            format!("configured_batch_{configured_batch}_observation"),
            serde_json::Value::Object(batch_extra_metrics),
        );
        self.blocked_upstream |= batch.blocked_upstream;
        if let Some(snapshot) = batch.db_snapshot {
            merge_pp_snapshot(&mut self.db_snapshot, snapshot);
        }
        self.batch_summaries.push(PpBatchSummary {
            configured_batch,
            attempted_batches: batch.attempted_batches,
            accepted_batches: batch.accepted_batches,
            accepted_payments: batch.accepted_payments,
            failed_batches: batch.failed_batches,
            wall_ms: batch.wall_ms,
        });
    }

    async fn observe_db(&mut self, config: &Config, timeout: Duration) {
        if self.batch_ids.is_empty() && self.payment_ids.is_empty() {
            return;
        }
        let start = Instant::now();
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        let mut latest = None;
        let mut attempts = 0u64;
        let mut timed_out = false;
        loop {
            interval.tick().await;
            if start.elapsed() > timeout {
                timed_out = true;
                break;
            }
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
                timed_out = true;
                break;
            }
        }
        if let Some(snapshot) = latest {
            self.blocked_upstream = snapshot.has_upstream_signing_or_broadcast_error();
            match verify_pp_snapshot_chain(config, &snapshot).await {
                Ok(proofs) => self.chain_proofs.extend(proofs),
                Err(error) => self.errors.push(format!(
                    "PP independent chain verification failed: {error:#}"
                )),
            }
            self.db_snapshot = Some(snapshot);
        }
        let pending_no_progress = timed_out
            && self
                .db_snapshot
                .as_ref()
                .is_some_and(|snapshot| !pp_snapshot_has_progress_or_error(snapshot));
        self.extra_metrics.insert(
            "db_observation_attempts".to_string(),
            serde_json::json!(attempts),
        );
        self.extra_metrics.insert(
            "db_observation_timeout_secs".to_string(),
            serde_json::json!(timeout.as_secs()),
        );
        self.extra_metrics.insert(
            "db_observation_wall_ms".to_string(),
            serde_json::json!(start.elapsed().as_millis()),
        );
        self.extra_metrics.insert(
            "db_observation_timed_out".to_string(),
            serde_json::json!(timed_out),
        );
        self.extra_metrics.insert(
            "db_observation_pending_no_progress".to_string(),
            serde_json::json!(pending_no_progress),
        );
        self.extra_metrics.insert(
            "db_observation_stop_reason".to_string(),
            serde_json::json!(if pending_no_progress {
                Some("pending_no_progress")
            } else if timed_out {
                Some("timeout")
            } else {
                Some("terminal_snapshot")
            }),
        );
    }

    fn note(&self, scenario: ScenarioName) -> String {
        let mut parts = vec![
            format!(
                "{} PP summary: attempted_batches={} attempted_payments={} accepted_batches={} accepted_payments={} failed_batches={} wall_ms={}",
                scenario.as_str(),
                self.attempted_batches,
                self.attempted_payments,
                self.accepted_batches,
                self.accepted_payments,
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
                        "configured_batch:{} attempted:{} accepted:{} accepted_payments:{} failed:{} wall_ms:{}",
                        batch.configured_batch,
                        batch.attempted_batches,
                        batch.accepted_batches,
                        batch.accepted_payments,
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
        let mut metrics = serde_json::json!({
            "scenario": scenario.as_str(),
            "verification_source": "payment_processor_db_observed",
            "attempted_batches": self.attempted_batches,
            "attempted_payments": self.attempted_payments,
            "accepted_batches": self.accepted_batches,
            "accepted_payments": self.accepted_payments,
            "failed_batches": self.failed_batches,
            "batch_ids": self.batch_ids,
            "payment_ids": self.payment_ids,
            "tx_timings": self.tx_timings,
            "transaction_observations": self.transaction_observations(),
            "max_serialization_gap_ms": max_serialization_gap_ms(self.construction_complete_ms.clone()),
            "double_selection_rejections": double_selection_rejections(&self.errors),
            "db_status_summary": self.db_snapshot.as_ref().map(PaymentProcessorDbSnapshot::status_summary),
            "responses": self.events,
            "extra": self.extra_metrics,
        });
        if scenario == ScenarioName::S5
            && let serde_json::Value::Object(map) = &mut metrics
        {
            map.insert("s5_arms".to_string(), self.s5_arms_metrics());
        }
        metrics
    }

    fn transaction_observations(&self) -> Vec<serde_json::Value> {
        let mut errors = self.errors.iter();
        let mut observations = self
            .tx_timings
            .iter()
            .map(|timing| {
                let batch_id = timing.get("batch_id").and_then(serde_json::Value::as_str);
                let proof = batch_id.and_then(|id| self.chain_proofs.get(id));
                let terminal_outcome = if proof.is_some() {
                    "confirmed"
                } else if batch_id.and_then(|id| {
                    self.db_snapshot.as_ref().and_then(|snapshot| {
                        snapshot
                            .batches
                            .iter()
                            .find(|batch| batch.id == id)
                            .map(|batch| batch.status.as_str())
                    })
                }) == Some("FAILED") {
                    "rejected"
                } else {
                    "timed_out"
                };
                transaction_observation(
                    batch_id,
                    timing_u128(timing, "api_accept_ms"),
                    timing_u128(timing, "api_accept_ms"),
                    None,
                    Some(
                        "payment-processor acceptance precedes worker broadcast and exposes no per-batch mempool timestamp",
                    ),
                    Some(self.wall_ms),
                    proof.map(|proof| proof.fee_microtari),
                    terminal_outcome,
                    (terminal_outcome != "confirmed")
                        .then(|| errors.next().cloned())
                        .flatten(),
                    proof.map(|proof| proof.mined_height),
                    None,
                    proof.map(|proof| proof.tip_height),
                )
            })
            .collect::<Vec<_>>();
        observations.extend(errors.map(|error| {
            transaction_observation(
                None,
                None,
                None,
                None,
                Some("payment-processor batch creation failed before an observable broadcast"),
                None,
                None,
                "rejected",
                Some(error.clone()),
                None,
                None,
                None,
            )
        }));
        observations
    }

    fn s5_arms_metrics(&self) -> serde_json::Value {
        let blocks_consumed = self.db_snapshot.as_ref().and_then(|snapshot| {
            blocks_consumed_from_heights(
                snapshot
                    .batches
                    .iter()
                    .filter(|batch| batch.status == "CONFIRMED")
                    .filter_map(|batch| {
                        batch
                            .mined_height
                            .and_then(|height| u64::try_from(height).ok())
                    }),
            )
        });
        let fee_microtari = self
            .chain_proofs
            .values()
            .map(|proof| proof.fee_microtari)
            .fold(0u64, u64::saturating_add);
        let complete = self.attempted_batches > 0
            && self.failed_batches == 0
            && self.chain_proofs.len() == self.attempted_batches as usize;
        serde_json::json!({
            "batch": {
                "mode": "payment_processor",
                "arm": "batch",
                "batch_size": self.extra_metrics.get("s5_batch_size"),
                "recipient_count": self.attempted_payments,
                "wall_ms": self.wall_ms,
                "success_count": self.accepted_batches,
                "failure_count": self.failed_batches,
                "complete": complete,
                "unavailable_reason": (!complete).then_some("one or more PP batches lack independent C_min-deep proof"),
                "fee_microtari": complete.then_some(fee_microtari),
                "fee_per_recipient_microtari": complete.then(|| fee_per_recipient(Some(fee_microtari), self.attempted_payments)).flatten(),
                "blocks_consumed": blocks_consumed,
                "mempool_timing_surface": "unavailable_through_payment_processor_api"
            }
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
                    .filter_map(|batch| {
                        let proof = self.chain_proofs.get(&batch.id)?;
                        Some(VerifiedTransaction {
                            tx_id: proof.chain_tx_id.clone(),
                            status_value: TX_MINED_CONFIRMED_STATUS,
                            mode: "payment_processor".to_string(),
                            scenario: scenario.as_str().to_string(),
                            amount_microtari: None,
                            fee_microtari: Some(proof.fee_microtari),
                            mined_height: Some(proof.mined_height),
                            confirmations: Some(proof.confirmations),
                            min_confirmations: Some(proof.min_confirmations),
                            tip_height: Some(proof.tip_height),
                            confirmed: true,
                        })
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

async fn verify_pp_snapshot_chain(
    config: &Config,
    snapshot: &PaymentProcessorDbSnapshot,
) -> anyhow::Result<BTreeMap<String, PpChainProof>> {
    let client = base_node_http_client()?;
    let tip_height = base_node_tip_height_with_client(&client, &config.network.base_node_http_url)
        .await
        .context("reading tip for PP independent transaction verification")?;
    let mut proofs = BTreeMap::new();
    for batch in snapshot
        .batches
        .iter()
        .filter(|batch| batch.status == "CONFIRMED")
    {
        let (Some(chain_tx_id), Some(fee_microtari), Some(excess_sig_nonce), Some(excess_sig)) = (
            batch.chain_tx_id.clone(),
            batch.fee_microtari,
            batch.kernel_excess_sig_nonce.clone(),
            batch.kernel_excess_sig.clone(),
        ) else {
            continue;
        };
        let query = Mode2KernelQuery {
            excess_sig_nonce,
            excess_sig,
            fee_microtari: Some(fee_microtari),
        };
        let response = query_mode2_transaction(&client, &config.network.base_node_http_url, &query)
            .await
            .with_context(|| format!("querying PP batch {} by real kernel", batch.id))?;
        let (_, confirmed) =
            mode2_transaction_query_status(&response, Some(tip_height), config.benchmark.c_min);
        let Some(mined_height) = response.mined_height.filter(|_| confirmed) else {
            continue;
        };
        proofs.insert(
            batch.id.clone(),
            PpChainProof {
                chain_tx_id,
                fee_microtari,
                mined_height,
                tip_height,
                confirmations: tip_height.saturating_sub(mined_height),
                min_confirmations: config.benchmark.c_min,
            },
        );
    }
    Ok(proofs)
}

fn merge_pp_snapshot(
    target: &mut Option<PaymentProcessorDbSnapshot>,
    mut source: PaymentProcessorDbSnapshot,
) {
    match target {
        Some(existing) => {
            existing.batches.append(&mut source.batches);
            existing.payments.append(&mut source.payments);
        }
        None => *target = Some(source),
    }
}

fn pp_snapshot_has_progress_or_error(snapshot: &PaymentProcessorDbSnapshot) -> bool {
    snapshot.has_upstream_signing_or_broadcast_error()
        || snapshot.batches.iter().any(|batch| {
            matches!(
                batch.status.as_str(),
                "AWAITING_SIGNATURE"
                    | "SIGNING_IN_PROGRESS"
                    | "AWAITING_BROADCAST"
                    | "BROADCASTING"
                    | "AWAITING_CONFIRMATION"
                    | "CONFIRMED"
                    | "FAILED"
                    | "CANCELLED"
            ) || batch.has_unsigned_tx
                || batch.has_signed_tx
                || batch.mined_height.is_some()
                || batch.error_message.is_some()
        })
}

impl Mode1TransferOutcome {
    fn with_rpc_timing(mut self, batch_index: u32, submit_response_ms: u128) -> Self {
        if self.tx_ids.is_empty() {
            self.tx_timings.push(serde_json::json!({
                "batch_index": batch_index,
                "construction_complete_ms": submit_response_ms,
                "broadcast_to_mempool_ms": null,
                "broadcast_to_mempool_unavailable_reason": "console_wallet_grpc_transfer_response_does_not_expose_mempool_timestamp"
            }));
        } else {
            self.tx_timings.extend(self.tx_ids.iter().map(|tx_id| {
                serde_json::json!({
                    "batch_index": batch_index,
                    "tx_id": tx_id,
                    "construction_complete_ms": submit_response_ms,
                    "broadcast_to_mempool_ms": null,
                    "broadcast_to_mempool_unavailable_reason": "console_wallet_grpc_transfer_response_does_not_expose_mempool_timestamp"
                })
            }));
        }
        self
    }

    fn from_response(response: grpc::TransferResponse) -> Self {
        let mut outcome = Self {
            success_count: 0,
            failure_count: 0,
            fee_microtari: 0,
            tx_ids: Vec::new(),
            errors: Vec::new(),
            tx_timings: Vec::new(),
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
                self.tx_timings.extend(outcome.tx_timings);
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
        let batch_tx_ids = batch.tx_ids.clone();
        self.tx_ids.extend(batch.tx_ids);
        self.errors.extend(batch.errors);
        self.tx_timings.extend(batch.tx_timings);
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
            fee_microtari: batch.fee_microtari,
            tx_ids: batch_tx_ids,
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
        metrics.insert("tx_timings".to_string(), serde_json::json!(self.tx_timings));
        metrics.insert(
            "transaction_observations".to_string(),
            serde_json::Value::Array(self.transaction_observations()),
        );
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
        metrics.insert("scenario".to_string(), serde_json::json!(scenario.as_str()));
        if scenario == ScenarioName::S5 {
            metrics.insert("s5_arms".to_string(), self.s5_arms_metrics());
        }
        metrics.extend(self.extra_metrics.clone());
        serde_json::Value::Object(metrics)
    }

    fn transaction_observations(&self) -> Vec<serde_json::Value> {
        let mut errors = self.errors.iter();
        let mut observations = self
            .tx_timings
            .iter()
            .map(|timing| {
                let tx_id = timing.get("tx_id").and_then(serde_json::Value::as_str);
                let verified = tx_id.and_then(|id| self.tx_infos.iter().find(|tx| tx.tx_id == id));
                let confirmed = verified.is_some_and(|tx| tx.confirmed);
                transaction_observation(
                    tx_id,
                    timing_u128(timing, "construction_complete_ms"),
                    timing_u128(timing, "construction_complete_ms"),
                    None,
                    Some("console-wallet gRPC does not expose a per-transaction mempool timestamp"),
                    Some(self.wall_ms),
                    verified.and_then(|tx| tx.fee_microtari),
                    if confirmed { "confirmed" } else { "timed_out" },
                    (!confirmed).then(|| errors.next().cloned()).flatten(),
                    verified.and_then(|tx| tx.mined_height),
                    None,
                    verified.and_then(|tx| tx.tip_height),
                )
            })
            .collect::<Vec<_>>();
        observations.extend(errors.map(|error| {
            transaction_observation(
                None,
                None,
                None,
                None,
                Some("console-wallet gRPC submission failed before a mempool observation"),
                None,
                None,
                "rejected",
                Some(error.clone()),
                None,
                None,
                None,
            )
        }));
        observations
    }

    fn s5_arms_metrics(&self) -> serde_json::Value {
        let mut arms = serde_json::Map::new();
        for batch in &self.batch_summaries {
            let arm_name = if batch.configured_batch == 1 {
                "individual"
            } else {
                "batch"
            };
            arms.insert(
                arm_name.to_string(),
                {
                    let complete = batch.failure_count == 0
                        && batch.success_count == batch.attempted_payments
                        && !batch.tx_ids.is_empty()
                        && batch.tx_ids.iter().all(|tx_id| {
                            self.tx_infos
                                .iter()
                                .any(|tx| &tx.tx_id == tx_id && tx.confirmed)
                        });
                    serde_json::json!({
                    "mode": "old_wallet",
                    "arm": arm_name,
                    "batch_size": batch.configured_batch,
                    "recipient_count": batch.attempted_payments,
                    "wall_ms": batch.wall_ms,
                    "success_count": batch.success_count,
                    "failure_count": batch.failure_count,
                    "complete": complete,
                    "unavailable_reason": (!complete).then_some("one or more individual transactions lack C_min-deep proof"),
                    "fee_microtari": complete.then_some(batch.fee_microtari),
                    "fee_per_recipient_microtari": complete.then(|| fee_per_recipient(Some(batch.fee_microtari), batch.attempted_payments)).flatten(),
                    "blocks_consumed": blocks_consumed_for_tx_ids(&self.tx_infos, &batch.tx_ids),
                    "mempool_timing_surface": "console_wallet_grpc_unavailable"
                })
                },
            );
        }
        serde_json::Value::Object(arms)
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
                let tx_id = outcome.tx_id.clone();
                self.tx_timings.push(serde_json::json!({
                    "attempt": attempt,
                    "tx_id": tx_id,
                    "construction_ms": outcome.construction_ms,
                    "broadcast_to_mempool_ms": outcome.broadcast_to_mempool_ms,
                    "accepted": outcome.accepted,
                    "is_synced": outcome.is_synced,
                    "rejection_reason": outcome.rejection_reason
                }));
                if outcome.accepted {
                    self.success_count += 1;
                    self.fee_microtari = self.fee_microtari.saturating_add(outcome.fee_microtari);
                    self.tx_ids.push(outcome.tx_id);
                } else {
                    self.failure_count += 1;
                    self.errors.push(format!(
                        "tx_id={tx_id} rejected: {}",
                        outcome
                            .rejection_reason
                            .as_deref()
                            .unwrap_or("base node did not accept the transaction")
                    ));
                }
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
        self.tx_timings.extend(batch.tx_timings);
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
        let verified_fee_total = verification
            .observed_transactions
            .iter()
            .filter(|tx| tx.confirmed)
            .filter_map(|tx| tx.fee_microtari)
            .sum::<u64>();
        self.fee_microtari = self.fee_microtari.max(verified_fee_total);
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
        metrics.insert("tx_timings".to_string(), serde_json::json!(self.tx_timings));
        metrics.insert(
            "transaction_observations".to_string(),
            serde_json::Value::Array(self.transaction_observations()),
        );
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
        if scenario == ScenarioName::S5 {
            metrics.insert("s5_arms".to_string(), self.s5_arms_metrics());
        }
        metrics.extend(self.extra_metrics.clone());
        serde_json::Value::Object(metrics)
    }

    fn transaction_observations(&self) -> Vec<serde_json::Value> {
        let mut errors = self
            .errors
            .iter()
            .filter(|error| !error.starts_with("tx_id="));
        let mut observations = self
            .tx_timings
            .iter()
            .map(|timing| {
                let tx_id = timing.get("tx_id").and_then(serde_json::Value::as_str);
                let verified = tx_id.and_then(|id| self.tx_infos.iter().find(|tx| tx.tx_id == id));
                let accepted = timing
                    .get("accepted")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let rejection = timing
                    .get("rejection_reason")
                    .and_then(serde_json::Value::as_str)
                    .map(ToString::to_string);
                let confirmed = verified.is_some_and(|tx| tx.confirmed);
                let terminal_outcome = if confirmed {
                    "confirmed"
                } else if !accepted {
                    "rejected"
                } else {
                    "timed_out"
                };
                transaction_observation(
                    tx_id,
                    timing_u128(timing, "construction_ms"),
                    timing_u128(timing, "broadcast_to_mempool_ms"),
                    None,
                    Some(
                        "base-node submission exposes acceptance but not a per-transaction mempool timestamp",
                    ),
                    Some(self.wall_ms),
                    verified
                        .and_then(|tx| tx.fee_microtari)
                        .or_else(|| timing.get("fee_microtari").and_then(serde_json::Value::as_u64)),
                    terminal_outcome,
                    rejection.or_else(|| (!confirmed).then(|| errors.next().cloned()).flatten()),
                    verified.and_then(|tx| tx.mined_height),
                    None,
                    verified.and_then(|tx| tx.tip_height),
                )
            })
            .collect::<Vec<_>>();
        observations.extend(errors.map(|error| {
            transaction_observation(
                None,
                None,
                None,
                None,
                Some("transaction construction or submission failed before base-node acceptance"),
                None,
                None,
                "rejected",
                Some(error.clone()),
                None,
                None,
                None,
            )
        }));
        observations
    }

    fn s5_arms_metrics(&self) -> serde_json::Value {
        let complete = mode2_summary_complete(self);
        serde_json::json!({
            "individual": {
                "mode": "new_wallet",
                "arm": "individual",
                "batch_size": 1,
                "recipient_count": self.attempted,
                "wall_ms": self.wall_ms,
                "success_count": self.success_count,
                "failure_count": self.failure_count,
                "complete": complete,
                "unavailable_reason": (!complete).then_some("one or more individual transactions lack independent C_min-deep proof"),
                "fee_microtari": complete.then_some(self.fee_microtari),
                "fee_per_recipient_microtari": complete.then(|| fee_per_recipient(Some(self.fee_microtari), self.attempted)).flatten(),
                "blocks_consumed": blocks_consumed_for_tx_ids(&self.tx_infos, &self.tx_ids),
                "mempool_timing_surface": "base_node_transaction_query"
            }
        })
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

#[allow(clippy::too_many_arguments)]
fn transaction_observation(
    transaction_id: Option<&str>,
    construction_ms: Option<u128>,
    submission_ms: Option<u128>,
    mempool_available: Option<bool>,
    mempool_reason: Option<&str>,
    confirmation_ms: Option<u128>,
    fee_microtari: Option<u64>,
    terminal_outcome: &str,
    error: Option<String>,
    mined_height: Option<u64>,
    tip_start_height: Option<u64>,
    tip_end_height: Option<u64>,
) -> serde_json::Value {
    serde_json::json!({
        "transaction_id": transaction_id,
        "construction_ms": construction_ms,
        "submission_ms": submission_ms,
        "mempool_available": mempool_available,
        "mempool_reason": mempool_reason,
        "confirmation_ms": confirmation_ms,
        "fee_microtari": fee_microtari,
        "terminal_outcome": terminal_outcome,
        "error": error,
        "mined_height": mined_height,
        "tip_start_height": tip_start_height,
        "tip_end_height": tip_end_height,
    })
}

fn timing_u128(timing: &serde_json::Value, field: &str) -> Option<u128> {
    timing.get(field)?.as_u64().map(u128::from)
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

async fn construct_sign_broadcast_one_sided_recipient_amounts_owned(
    request: OwnedOneSidedSendRequest,
    recipients: Vec<(String, u64)>,
) -> anyhow::Result<OneSidedSendOutcome> {
    construct_sign_broadcast_one_sided_recipient_amounts(request.as_borrowed(), &recipients).await
}

pub async fn construct_sign_broadcast_one_sided(
    request: OneSidedSendRequest<'_>,
) -> anyhow::Result<OneSidedSendOutcome> {
    let construction_start = Instant::now();
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
            Err(error) => anyhow::bail!("signing locked transaction failed: {error}"),
        };
    let construction_ms = construction_start.elapsed().as_millis();
    finalize_transaction_and_broadcast_without_retry(&sender, signed, request, construction_ms)
        .await
}

pub async fn construct_sign_broadcast_one_sided_multi_recipient(
    request: OneSidedSendRequest<'_>,
    recipients: &[String],
) -> anyhow::Result<OneSidedSendOutcome> {
    let recipients = recipients
        .iter()
        .cloned()
        .map(|recipient| (recipient, request.amount.0))
        .collect::<Vec<_>>();
    construct_sign_broadcast_one_sided_recipient_amounts(request, &recipients).await
}

async fn construct_sign_broadcast_one_sided_recipient_amounts(
    request: OneSidedSendRequest<'_>,
    recipients: &[(String, u64)],
) -> anyhow::Result<OneSidedSendOutcome> {
    let construction_start = Instant::now();
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
        .map(|(recipient, amount)| {
            Ok(Recipient {
                address: TariAddress::from_str(recipient)?,
                amount: MicroMinotari(*amount),
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
            anyhow::bail!("creating multi-recipient unsigned transaction failed: {error}")
        }
    };
    let key_manager = match account.get_key_manager(request.password) {
        Ok(key_manager) => key_manager,
        Err(error) => anyhow::bail!("opening key manager failed: {error}"),
    };
    let constants = ConsensusConstantsBuilder::new(Network::Esmeralda).build();
    let signed =
        match sign_locked_transaction(&key_manager, constants, Network::Esmeralda, unsigned) {
            Ok(signed) => signed,
            Err(error) => {
                anyhow::bail!("signing multi-recipient locked transaction failed: {error}")
            }
        };
    let construction_ms = construction_start.elapsed().as_millis();
    finalize_signed_transaction_and_broadcast_without_retry(
        &pool,
        account.id,
        &pending_tx_id,
        signed,
        request,
        construction_ms,
    )
    .await
}

pub async fn fund_one_sided_outputs(
    config: &Config,
    source_db: &Path,
    recipients: &[String],
    amount: &str,
    outputs: u32,
    batch_size: u32,
) -> anyhow::Result<()> {
    if recipients.is_empty() {
        bail!("at least one --recipient is required");
    }
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
        recipient: recipients[0].clone(),
        amount,
        fee_rate: config.fee_rate()?,
        seconds_to_lock: config.timeouts.transaction_lock_secs,
        confirmation_window: config.benchmark.c_min,
        request_timeout: Duration::from_secs(30),
    };

    if recipients.len() > 1 {
        if outputs != 1 || batch_size != 1 {
            bail!(
                "multiple --recipient values create one output per recipient; leave --outputs and --batch-size at 1"
            );
        }
        println!(
            "fund-one-sided batch 1: outputs={} amount_microtari={}",
            recipients.len(),
            amount.0
        );
        let outcome =
            construct_sign_broadcast_one_sided_multi_recipient_owned(request, recipients.to_vec())
                .await?;
        println!(
            "fund-one-sided batch 1 accepted={} synced={} tx_id={} fee_microtari={} rejection_reason={}",
            outcome.accepted,
            outcome.is_synced,
            outcome.tx_id,
            outcome.fee_microtari,
            outcome.rejection_reason.as_deref().unwrap_or("None")
        );
        println!(
            "fund-one-sided submitted 1 tx for {} outputs: {}",
            recipients.len(),
            outcome.tx_id
        );
        return Ok(());
    }

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
        let recipients = repeated_recipient(&recipients[0], batch_outputs as usize);
        let outcome =
            construct_sign_broadcast_one_sided_multi_recipient_owned(request.clone(), recipients)
                .await?;
        println!(
            "fund-one-sided batch {batch_index} accepted={} synced={} tx_id={} fee_microtari={} rejection_reason={}",
            outcome.accepted,
            outcome.is_synced,
            outcome.tx_id,
            outcome.fee_microtari,
            outcome.rejection_reason.as_deref().unwrap_or("None")
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
    construction_ms: u128,
) -> anyhow::Result<OneSidedSendOutcome> {
    finalize_signed_transaction_and_broadcast_without_retry(
        &sender.db_pool,
        sender.account.id,
        sender.processed_transactions.id(),
        signed,
        request,
        construction_ms,
    )
    .await
}

async fn finalize_signed_transaction_and_broadcast_without_retry(
    db_pool: &SqlitePool,
    account_id: i64,
    pending_tx_id: &str,
    signed: SignedOneSidedTransactionResult,
    request: OneSidedSendRequest<'_>,
    construction_ms: u128,
) -> anyhow::Result<OneSidedSendOutcome> {
    persist_signed_transaction(db_pool, account_id, pending_tx_id, &signed)?;
    let tx_id = signed.signed_transaction.tx_id;
    let fee_microtari = signed.request.info.fee.0;
    let broadcast_start = Instant::now();
    let submission = submit_transaction_without_retry(
        request.base_node_url,
        signed.signed_transaction.transaction,
        request.request_timeout,
    )
    .await;
    let broadcast_to_mempool_ms = broadcast_start.elapsed().as_millis();

    let conn = db_pool.get()?;
    match submission {
        Ok(response) if response.accepted => {
            db::mark_completed_transaction_as_broadcasted(&conn, tx_id, 1)?;
            Ok(OneSidedSendOutcome {
                tx_id: tx_id.to_string(),
                fee_microtari,
                accepted: response.accepted,
                is_synced: response.is_synced,
                rejection_reason: None,
                construction_ms,
                broadcast_to_mempool_ms: Some(broadcast_to_mempool_ms),
            })
        }
        Ok(response) if response.rejection_reason == TxSubmissionRejectionReason::AlreadyMined => {
            Ok(OneSidedSendOutcome {
                tx_id: tx_id.to_string(),
                fee_microtari,
                accepted: response.accepted,
                is_synced: response.is_synced,
                rejection_reason: Some(response.rejection_reason.to_string()),
                construction_ms,
                broadcast_to_mempool_ms: None,
            })
        }
        Ok(response) => {
            db::mark_completed_transaction_as_rejected(
                &conn,
                tx_id,
                &response.rejection_reason.to_string(),
            )?;
            anyhow::bail!(
                "transaction was not accepted by the network: {}",
                response.rejection_reason
            );
        }
        Err(error) => Err(error),
    }
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

fn resolved_birthday_start_height(config: &Config, mode: &str, spec: FreshScanSpec) -> u64 {
    if spec.birthday() == 0 {
        return 0;
    }
    match mode {
        "old_wallet" => config.funding.old_wallet.as_ref(),
        "new_wallet" => config.funding.new_wallet.as_ref(),
        "payment_processor" => config.funding.payment_processor.as_ref(),
        _ => None,
    }
    .map(|funding| funding.height)
    .unwrap_or_default()
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
    let complete = match complete_checkpoint {
        ScanCheckpoint::PostS1 => mode1_s1_complete(summary),
        _ => mode1_send_complete(summary),
    };
    if complete {
        return complete_checkpoint;
    }
    if summary.success_count == 0 {
        return match complete_checkpoint {
            ScanCheckpoint::PostS1 => ScanCheckpoint::PostS1Blocked,
            _ => ScanCheckpoint::PostS5Blocked,
        };
    }
    match complete_checkpoint {
        ScanCheckpoint::PostS1 => ScanCheckpoint::PostS1Partial,
        _ => ScanCheckpoint::PostS5Partial,
    }
}

fn checkpoint_from_mode2_summary(
    summary: &ScenarioSendSummary,
    complete_checkpoint: ScanCheckpoint,
) -> ScanCheckpoint {
    if mode2_summary_complete(summary) {
        return complete_checkpoint;
    }
    if summary.success_count == 0 {
        return match complete_checkpoint {
            ScanCheckpoint::PostS1 => ScanCheckpoint::PostS1Blocked,
            _ => ScanCheckpoint::PostS5Blocked,
        };
    }
    match complete_checkpoint {
        ScanCheckpoint::PostS1 => ScanCheckpoint::PostS1Partial,
        _ => ScanCheckpoint::PostS5Partial,
    }
}

fn checkpoint_from_pp_summary(
    summary: &PpScenarioSummary,
    complete_checkpoint: ScanCheckpoint,
) -> ScanCheckpoint {
    if pp_summary_complete(summary) {
        return complete_checkpoint;
    }
    if summary.accepted_batches == 0 {
        return match complete_checkpoint {
            ScanCheckpoint::PostS1 => ScanCheckpoint::PostS1Blocked,
            _ => ScanCheckpoint::PostS5Blocked,
        };
    }
    match complete_checkpoint {
        ScanCheckpoint::PostS1 => ScanCheckpoint::PostS1Partial,
        _ => ScanCheckpoint::PostS5Partial,
    }
}

fn scan_expectations_from_profile(
    profile: &ResultProfile,
    mode: &str,
    spec: FreshScanSpec,
    config: &Config,
) -> ScanExpectations {
    match spec.checkpoint {
        ScanCheckpoint::Empty => ScanExpectations {
            expected_outputs: Some(0),
            expected_available_microtari: Some(0),
        },
        ScanCheckpoint::PostS1 => scenario_scan_expectations(profile, mode, ScenarioName::S1)
            .with_fallback_outputs(Some(u64::from(config.benchmark.volume_target))),
        ScanCheckpoint::PostS1Partial => {
            scenario_scan_expectations(profile, mode, ScenarioName::S1)
        }
        ScanCheckpoint::PostS5Complete | ScanCheckpoint::PostS5Partial => {
            scenario_scan_expectations(profile, mode, ScenarioName::S5)
        }
        ScanCheckpoint::PostS1Blocked | ScanCheckpoint::PostS5Blocked => {
            ScanExpectations::default()
        }
    }
}

fn scenario_scan_expectations(
    profile: &ResultProfile,
    mode: &str,
    scenario: ScenarioName,
) -> ScanExpectations {
    let Some(metrics) = scenario_metrics(profile, mode, scenario) else {
        return ScanExpectations::default();
    };
    ScanExpectations {
        expected_outputs: metrics
            .get("unspent_after")
            .and_then(serde_json::Value::as_u64),
        expected_available_microtari: metrics
            .get("balance_after_microtari")
            .and_then(serde_json::Value::as_u64),
    }
}

fn scenario_metrics<'a>(
    profile: &'a ResultProfile,
    mode: &str,
    scenario: ScenarioName,
) -> Option<&'a serde_json::Value> {
    profile
        .modes
        .get(mode)?
        .scenarios
        .get(scenario.as_str())?
        .repetitions
        .iter()
        .find_map(|run| run.metrics.as_ref())
}

impl ScanExpectations {
    fn with_fallback_outputs(mut self, fallback: Option<u64>) -> Self {
        if self.expected_outputs.is_none() {
            self.expected_outputs = fallback;
        }
        self
    }
}

struct AccountSnapshot {
    max_height: u64,
    available_microtari: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct ScanToTipReport {
    pub wall_ms: u128,
    pub target_tip: u64,
    pub max_height: u64,
    pub no_progress_attempts: u64,
    pub stopped_without_progress: bool,
    pub last_more_blocks: Option<bool>,
    pub used_single_block_fallback: bool,
}

impl ScanToTipReport {
    fn insert_metrics(&self, metrics: &mut serde_json::Map<String, serde_json::Value>) {
        metrics.insert(
            "scan_target_tip".to_string(),
            serde_json::json!(self.target_tip),
        );
        metrics.insert(
            "scan_used_single_block_fallback".to_string(),
            serde_json::json!(self.used_single_block_fallback),
        );
        metrics.insert(
            "scan_max_height".to_string(),
            serde_json::json!(self.max_height),
        );
        metrics.insert(
            "scan_no_progress_attempts".to_string(),
            serde_json::json!(self.no_progress_attempts),
        );
        metrics.insert(
            "scan_stopped_without_progress".to_string(),
            serde_json::json!(self.stopped_without_progress),
        );
        metrics.insert(
            "scan_last_more_blocks".to_string(),
            serde_json::json!(self.last_more_blocks),
        );
        metrics.insert(
            "scan_stop_reason".to_string(),
            serde_json::json!(if self.stopped_without_progress {
                Some("no_progress_timeout")
            } else {
                None
            }),
        );
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct ScanExpectations {
    expected_outputs: Option<u64>,
    expected_available_microtari: Option<u64>,
}

struct ScanMeasurement {
    wall_ms: u128,
    birthday: u16,
    birthday_start_height: u64,
    max_height: u64,
    available_microtari: u64,
    tip_start: Option<u64>,
    tip_end: Option<u64>,
    detected_outputs: u64,
    spendable_outputs: u64,
    resource_peaks: ResourcePeaks,
    expectations: ScanExpectations,
    tip_lag_tolerance_blocks: u64,
    scan_no_progress_attempts: u64,
    scan_stopped_without_progress: bool,
    scan_last_more_blocks: Option<bool>,
}

impl ScanMeasurement {
    fn note(&self) -> String {
        format!(
            "fresh scan checkpoint data: birthday={} max_height={} available_microtari={} detected_outputs={} spendable_outputs={} tip_start={:?} tip_end={:?}",
            self.birthday,
            self.max_height,
            self.available_microtari,
            self.detected_outputs,
            self.spendable_outputs,
            self.tip_start,
            self.tip_end
        )
    }

    fn metrics(&self, mode: &str, spec: FreshScanSpec) -> serde_json::Value {
        let blocks_scanned = Some(self.max_height.saturating_sub(self.birthday_start_height));
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
            "expected_outputs": self.expectations.expected_outputs,
            "outputs_match_expected": self.expectations.expected_outputs.map(|expected| expected == self.spendable_outputs),
            "expected_available_microtari": self.expectations.expected_available_microtari,
            "balance_matches_expected": self.expectations.expected_available_microtari.map(|expected| expected == self.available_microtari),
            "birthday": self.birthday,
            "birthday_start_height": self.birthday_start_height,
            "tip_start": self.tip_start,
            "tip_end": self.tip_end,
            "tip_lag_blocks": self.tip_lag_blocks(),
            "tip_lag_tolerance_blocks": self.tip_lag_tolerance_blocks,
            "scan_reached_tip": self.scan_reached_tip(),
            "blocks_scanned": blocks_scanned,
            "blocks_per_sec": blocks_per_sec,
            "detected_outputs": self.detected_outputs,
            "spendable_outputs": self.spendable_outputs,
            "available_microtari": self.available_microtari,
            "max_height": self.max_height,
            "scan_no_progress_attempts": self.scan_no_progress_attempts,
            "scan_stopped_without_progress": self.scan_stopped_without_progress,
            "scan_last_more_blocks": self.scan_last_more_blocks,
            "scan_stop_reason": if self.scan_stopped_without_progress { Some("no_progress_timeout") } else { None },
            "peak_rss_bytes": self.resource_peaks.peak_rss_bytes,
            "peak_cpu_percent": self.resource_peaks.peak_cpu_percent
        })
    }

    fn scan_verification_ok(&self) -> bool {
        self.scan_reached_tip()
            && self
                .expectations
                .expected_outputs
                .is_none_or(|expected| expected == self.spendable_outputs)
            && self
                .expectations
                .expected_available_microtari
                .is_none_or(|expected| expected == self.available_microtari)
    }

    fn scan_verification_error(&self) -> String {
        format!(
            "scan verification mismatch: max_height={} tip_end={:?} tip_lag_blocks={:?} tip_lag_tolerance_blocks={} expected_outputs={:?} spendable_outputs={} detected_outputs={} expected_available_microtari={:?} available_microtari={}",
            self.max_height,
            self.tip_end,
            self.tip_lag_blocks(),
            self.tip_lag_tolerance_blocks,
            self.expectations.expected_outputs,
            self.spendable_outputs,
            self.detected_outputs,
            self.expectations.expected_available_microtari,
            self.available_microtari
        )
    }

    fn tip_lag_blocks(&self) -> Option<u64> {
        self.tip_end.map(|tip| tip.saturating_sub(self.max_height))
    }

    fn scan_reached_tip(&self) -> bool {
        self.tip_end.is_none_or(|tip| {
            self.max_height
                .saturating_add(self.tip_lag_tolerance_blocks)
                >= tip
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
                confirmations: None,
                min_confirmations: None,
                tip_height: None,
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
                confirmations: None,
                min_confirmations: None,
                tip_height: None,
                confirmed: true,
            }],
            ..Mode1TransferSummary::default()
        };

        summary.backfill_verified_fee_total();

        assert_eq!(summary.fee_microtari, 1_000);
    }

    #[test]
    fn terminal_ok_status_matches_bounty_status_set() {
        for status in [6, 9, 13] {
            assert!(terminal_ok_status(status));
        }
        for status in [1, 2, 7, 11, 14] {
            assert!(!terminal_ok_status(status));
        }
    }

    #[test]
    fn mode1_s1_does_not_complete_on_mined_unconfirmed_status() {
        let pending = Mode1TransferSummary {
            attempted_batches: 1,
            attempted_payments: 2,
            success_count: 1,
            tx_ids: vec!["42".to_string()],
            tx_infos: vec![VerifiedTransaction {
                tx_id: "42".to_string(),
                status_value: 2,
                mode: "old_wallet".to_string(),
                scenario: ScenarioName::S1.as_str().to_string(),
                amount_microtari: Some(2_000_000),
                fee_microtari: Some(945),
                mined_height: Some(710_357),
                confirmations: None,
                min_confirmations: None,
                tip_height: None,
                confirmed: terminal_ok_status(2),
            }],
            ..Mode1TransferSummary::default()
        };
        assert!(!mode1_s1_complete(&pending));

        let confirmed = Mode1TransferSummary {
            tx_infos: vec![VerifiedTransaction {
                tx_id: "42".to_string(),
                status_value: TX_MINED_CONFIRMED_STATUS,
                mode: "old_wallet".to_string(),
                scenario: ScenarioName::S1.as_str().to_string(),
                amount_microtari: Some(2_000_000),
                fee_microtari: Some(945),
                mined_height: Some(710_357),
                confirmations: None,
                min_confirmations: None,
                tip_height: None,
                confirmed: terminal_ok_status(TX_MINED_CONFIRMED_STATUS),
            }],
            ..pending
        };
        assert!(mode1_s1_complete(&confirmed));
    }

    #[test]
    fn mode1_summary_keeps_mined_unconfirmed_out_of_chain_rows() {
        let config = Config::default();
        let mut profile = ResultProfile::new(&config, crate::env_capture::capture());
        profile.modes.insert(
            ModeName::OldWallet.as_str().to_string(),
            empty_mode_profile(ModeName::OldWallet, None),
        );
        let summary = Mode1TransferSummary {
            attempted_batches: 1,
            attempted_payments: 2,
            success_count: 1,
            tx_ids: vec!["42".to_string()],
            tx_infos: vec![VerifiedTransaction {
                tx_id: "42".to_string(),
                status_value: 2,
                mode: "old_wallet".to_string(),
                scenario: ScenarioName::S1.as_str().to_string(),
                amount_microtari: Some(2_000_000),
                fee_microtari: Some(945),
                mined_height: Some(710_357),
                confirmations: None,
                min_confirmations: None,
                tip_height: None,
                confirmed: false,
            }],
            ..Mode1TransferSummary::default()
        };

        record_mode1_transfer_summary(&mut profile, ScenarioName::S1, &summary, Vec::new());

        let cell = &profile
            .modes
            .get(ModeName::OldWallet.as_str())
            .unwrap()
            .scenarios[ScenarioName::S1.as_str()];
        assert_eq!(cell.status, CellStatus::Failed);
        assert!(profile.chain_verification.verified_transactions.is_empty());
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
                confirmations: None,
                min_confirmations: None,
                tip_height: None,
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
                    confirmations: None,
                    min_confirmations: None,
                    tip_height: None,
                    confirmed: true,
                },
            ],
            observations: Vec::new(),
            used_base_node_query: true,
        };
        assert!(mode2_verification_confirmed(&all_confirmed, &tx_ids));
    }

    #[test]
    fn mode2_settle_gate_requires_scan_and_tip_to_reach_target() {
        assert!(!mode2_settle_gate_ready(99, 101, 100));
        assert!(!mode2_settle_gate_ready(101, 99, 100));
        assert!(mode2_settle_gate_ready(100, 100, 100));
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
    fn blocked_prerequisite_records_failed_repetition() {
        let mut cell = ScenarioCell {
            scenario: ScenarioName::S5,
            surface: "minotari_library".to_string(),
            status: CellStatus::ReadyForLiveRun,
            repetitions: Vec::new(),
            median_wall_ms: None,
            spread_wall_ms: None,
            notes: Vec::new(),
        };

        record_blocked_prerequisite_cell(&mut cell, ScenarioName::S5, "S4");

        assert_eq!(cell.status, CellStatus::Failed);
        assert_eq!(cell.repetitions.len(), 1);
        assert_eq!(cell.repetitions[0].status, CellStatus::Failed);
        assert_eq!(cell.repetitions[0].success_count, 0);
        assert_eq!(cell.repetitions[0].failure_count, 1);
        assert!(
            cell.repetitions[0]
                .error
                .as_deref()
                .unwrap()
                .contains("prerequisite S4 did not complete")
        );
        let metrics = cell.repetitions[0].metrics.as_ref().unwrap();
        assert_eq!(metrics["blocked_prerequisite"], serde_json::json!(true));
        assert_eq!(metrics["prerequisite"], serde_json::json!("S4"));
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
        assert_eq!(new_b0.status, CellStatus::NotApplicable);
        let old_b0 = &profile
            .modes
            .get(ModeName::OldWallet.as_str())
            .unwrap()
            .scenarios[ScenarioName::B0.as_str()];
        assert_eq!(old_b0.status, CellStatus::NotApplicable);
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
            birthday_start_height: 700_000,
            max_height: 711_305,
            wall_ms: 10_000,
            available_microtari: 1,
            tip_start: Some(711_300),
            tip_end: Some(711_305),
            detected_outputs: 1,
            spendable_outputs: 1,
            resource_peaks: ResourcePeaks::default(),
            expectations: ScanExpectations::default(),
            tip_lag_tolerance_blocks: 3,
            scan_no_progress_attempts: 0,
            scan_stopped_without_progress: false,
            scan_last_more_blocks: None,
        };
        let spec = FreshScanSpec {
            scenario: ScenarioName::S3,
            wallet_state: FreshScanWalletState::FundedBirthday { birthday: 1_635 },
            checkpoint: ScanCheckpoint::PostS1,
        };
        let metrics = measurement.metrics("new_wallet", spec);

        assert_eq!(metrics["blocks_scanned"], serde_json::json!(11_305));
        assert!(metrics["blocks_per_sec"].as_f64().unwrap() > 0.0);
    }

    #[test]
    fn scan_verification_fails_when_scan_stops_far_below_tip() {
        let measurement = ScanMeasurement {
            birthday: 0,
            birthday_start_height: 0,
            max_height: 627_100,
            wall_ms: 10_000,
            available_microtari: 0,
            tip_start: Some(726_900),
            tip_end: Some(726_905),
            detected_outputs: 0,
            spendable_outputs: 0,
            resource_peaks: ResourcePeaks::default(),
            expectations: ScanExpectations {
                expected_outputs: Some(0),
                expected_available_microtari: Some(0),
            },
            tip_lag_tolerance_blocks: 3,
            scan_no_progress_attempts: 2,
            scan_stopped_without_progress: true,
            scan_last_more_blocks: Some(true),
        };

        assert!(!measurement.scan_verification_ok());
        assert_eq!(measurement.tip_lag_blocks(), Some(99_805));
        assert!(
            measurement
                .scan_verification_error()
                .contains("max_height=627100")
        );
    }

    #[test]
    fn scan_verification_allows_confirmation_window_lag() {
        let measurement = ScanMeasurement {
            birthday: 0,
            birthday_start_height: 0,
            max_height: 726_902,
            wall_ms: 10_000,
            available_microtari: 0,
            tip_start: Some(726_900),
            tip_end: Some(726_905),
            detected_outputs: 0,
            spendable_outputs: 0,
            resource_peaks: ResourcePeaks::default(),
            expectations: ScanExpectations {
                expected_outputs: Some(0),
                expected_available_microtari: Some(0),
            },
            tip_lag_tolerance_blocks: 3,
            scan_no_progress_attempts: 0,
            scan_stopped_without_progress: false,
            scan_last_more_blocks: None,
        };

        assert!(measurement.scan_verification_ok());
        assert_eq!(measurement.tip_lag_blocks(), Some(3));
        let metrics = measurement.metrics(
            "new_wallet",
            FreshScanSpec {
                scenario: ScenarioName::B0,
                wallet_state: FreshScanWalletState::EmptyGenesis,
                checkpoint: ScanCheckpoint::Empty,
            },
        );
        assert_eq!(metrics["scan_reached_tip"], serde_json::json!(true));
        assert_eq!(metrics["tip_lag_blocks"], serde_json::json!(3));
    }

    #[test]
    fn pp_s4_observation_uses_s4_budget() {
        let mut config = Config::default();
        config.benchmark.s4_t_budget_secs = 17;
        config.timeouts.confirmation_secs = 999;

        assert_eq!(
            pp_observation_timeout(&config, ScenarioName::S4),
            Duration::from_secs(17)
        );
        assert_eq!(
            pp_observation_timeout(&config, ScenarioName::S1),
            Duration::from_secs(999)
        );
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
                chain_tx_id: None,
                fee_microtari: None,
                kernel_excess_sig_nonce: None,
                kernel_excess_sig: None,
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
                chain_tx_id: None,
                fee_microtari: None,
                kernel_excess_sig_nonce: None,
                kernel_excess_sig: None,
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
                chain_tx_id: None,
                fee_microtari: None,
                kernel_excess_sig_nonce: None,
                kernel_excess_sig: None,
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
                chain_tx_id: None,
                fee_microtari: None,
                kernel_excess_sig_nonce: None,
                kernel_excess_sig: None,
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
                confirmations: None,
                min_confirmations: None,
                tip_height: None,
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
                confirmations: None,
                min_confirmations: None,
                tip_height: None,
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
            confirmations: None,
            min_confirmations: None,
            tip_height: None,
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
                confirmations: None,
                min_confirmations: None,
                tip_height: None,
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
    fn mode2_verification_backfills_confirmed_fee_before_reconciliation() {
        let mut summary = ScenarioSendSummary::default();
        summary.apply_mode2_verification(Mode2VerificationResult {
            observed_transactions: vec![VerifiedTransaction {
                tx_id: "42".to_string(),
                status_value: TX_MINED_CONFIRMED_STATUS,
                mode: "new_wallet".to_string(),
                scenario: ScenarioName::S1.as_str().to_string(),
                amount_microtari: None,
                fee_microtari: Some(660),
                mined_height: Some(100),
                confirmations: Some(3),
                min_confirmations: Some(3),
                tip_height: Some(102),
                confirmed: true,
            }],
            observations: Vec::new(),
            used_base_node_query: true,
        });
        assert_eq!(summary.fee_microtari, 660);
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
                    chain_tx_id: None,
                    fee_microtari: None,
                    kernel_excess_sig_nonce: None,
                    kernel_excess_sig: None,
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
            chain_proofs: BTreeMap::from([(
                "confirmed".to_string(),
                PpChainProof {
                    chain_tx_id: "kernel-confirmed".to_string(),
                    fee_microtari: 700,
                    mined_height: 42,
                    tip_height: 45,
                    confirmations: 3,
                    min_confirmations: 3,
                },
            )]),
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
                        chain_tx_id: None,
                        fee_microtari: None,
                        kernel_excess_sig_nonce: None,
                        kernel_excess_sig: None,
                    },
                    payment_processor::PaymentBatchSnapshot {
                        id: "pending".to_string(),
                        status: "PENDING_BATCHING".to_string(),
                        retry_count: 0,
                        error_message: None,
                        has_unsigned_tx: false,
                        has_signed_tx: false,
                        mined_height: None,
                        chain_tx_id: None,
                        fee_microtari: None,
                        kernel_excess_sig_nonce: None,
                        kernel_excess_sig: None,
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
            "kernel-confirmed"
        );
        let metrics = cell.repetitions[0].metrics.as_ref().unwrap();
        assert_eq!(
            metrics["verification_source"],
            serde_json::json!("payment_processor_db_observed")
        );
    }

    #[test]
    fn mode2_rejected_submission_is_a_failure_with_a_structured_observation() {
        let mut summary = ScenarioSendSummary {
            attempted: 1,
            ..ScenarioSendSummary::default()
        };
        summary.record_attempt(
            1,
            Ok(OneSidedSendOutcome {
                tx_id: "rejected-tx".to_string(),
                fee_microtari: 500,
                accepted: false,
                is_synced: true,
                rejection_reason: Some("AlreadyMined".to_string()),
                construction_ms: 12,
                broadcast_to_mempool_ms: None,
            }),
        );

        assert_eq!(summary.success_count, 0);
        assert_eq!(summary.failure_count, 1);
        assert!(summary.tx_ids.is_empty());
        let observations = summary.transaction_observations();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0]["terminal_outcome"], "rejected");
        assert_eq!(observations[0]["error"], "AlreadyMined");
        assert!(observations[0].get("submission_ms").is_some());
    }

    #[test]
    fn confirmed_transaction_observation_records_scenario_terminal_duration() {
        let mut summary = ScenarioSendSummary {
            attempted: 1,
            wall_ms: 321,
            ..ScenarioSendSummary::default()
        };
        summary.record_attempt(
            1,
            Ok(OneSidedSendOutcome {
                tx_id: "confirmed-tx".to_string(),
                fee_microtari: 500,
                accepted: true,
                is_synced: true,
                rejection_reason: None,
                construction_ms: 12,
                broadcast_to_mempool_ms: Some(4),
            }),
        );
        summary.tx_infos.push(VerifiedTransaction {
            tx_id: "confirmed-tx".to_string(),
            status_value: TX_MINED_CONFIRMED_STATUS,
            mode: "new_wallet".to_string(),
            scenario: ScenarioName::S5.as_str().to_string(),
            amount_microtari: None,
            fee_microtari: Some(500),
            mined_height: Some(100),
            confirmations: Some(3),
            min_confirmations: Some(3),
            tip_height: Some(103),
            confirmed: true,
        });

        let observations = summary.transaction_observations();
        assert_eq!(observations[0]["terminal_outcome"], "confirmed");
        assert_eq!(observations[0]["confirmation_ms"], 321);
    }

    #[test]
    fn mode1_grpc_address_identity_matches_only_the_configured_seed() {
        let material = crate::seeds::material_from_seed(
            WalletRole::OldWallet,
            "WALLET_BENCH_TEST_MODE1".to_string(),
            CipherSeed::random(),
        )
        .unwrap();
        let expected = TariAddress::from_str(&material.address).unwrap().to_vec();

        assert!(mode1_address_matches_seed(&expected, &material).unwrap());
        let mut mismatched = expected;
        mismatched[0] ^= 0x01;
        assert!(!mode1_address_matches_seed(&mismatched, &material).unwrap());
    }
}
