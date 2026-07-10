use super::verification::verify_mode2_transactions_with_client;
use super::*;
use crate::versions::TX_MINED_CONFIRMED_STATUS;

pub(super) async fn annotate_mode2_send_smoke(
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

pub(super) async fn annotate_mode2_live_scenarios(
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
        amount: parse_amount(&config.benchmark.mode2_payment_amount)?,
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
                config.benchmark.mode2_payment_amount,
                sender_seed.address,
                s1_round_plan(config, 0).len(),
                config.benchmark.mode2_live_max_s1_txs
            ),
            "Mode 2 S1 uses the minotari multi-recipient one-sided builder directly so the measured wallet builds the output set without shelling out or pre-partitioning UTXOs."
                .to_string(),
        ],
    );
    if !mode2_summary_complete(&s1) {
        record_blocked_prerequisite_cells(
            profile,
            "new_wallet",
            &[
                ScenarioName::S2,
                ScenarioName::S3,
                ScenarioName::S4,
                ScenarioName::S5,
                ScenarioName::S6,
                ScenarioName::S7,
            ],
            "S1",
        );
        return Ok(());
    }
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
    let s4_balance_before = account_snapshot(&config.modes.new_wallet_database)
        .ok()
        .map(|snapshot| snapshot.available_microtari);
    let s4_recipients = derive_distinct_recipient_pool(128)?;
    let mut s4 = run_s4_batches(config, request.clone(), &s4_recipients).await?;
    let s4_balance_after = account_snapshot(&config.modes.new_wallet_database)
        .ok()
        .map(|snapshot| snapshot.available_microtari);
    add_balance_reconciliation_metrics(
        &mut s4.extra_metrics,
        s4_balance_before,
        s4_balance_after,
        u64::from(s4.success_count).saturating_mul(request.amount.0),
        s4.fee_microtari,
    );
    s4.extra_metrics.insert(
        "unspent_after".to_string(),
        serde_json::json!(spendable_output_count(&config.modes.new_wallet_database).ok()),
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
    let s5_balance_before = account_snapshot(&config.modes.new_wallet_database)
        .ok()
        .map(|snapshot| snapshot.available_microtari);
    let s5_amount_microtari = request.amount.0;
    let s5_start = Instant::now();
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
    s5.wall_ms = s5_start.elapsed().as_millis();
    let s5_balance_after = account_snapshot(&config.modes.new_wallet_database)
        .ok()
        .map(|snapshot| snapshot.available_microtari);
    add_balance_reconciliation_metrics(
        &mut s5.extra_metrics,
        s5_balance_before,
        s5_balance_after,
        u64::from(s5.success_count).saturating_mul(s5_amount_microtari),
        s5.fee_microtari,
    );
    s5.extra_metrics.insert(
        "unspent_after".to_string(),
        serde_json::json!(spendable_output_count(&config.modes.new_wallet_database).ok()),
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

pub(super) async fn verify_mode2_transactions_until_confirmed(
    config: &Config,
    db_path: &Path,
    tx_ids: &[String],
    scenario: ScenarioName,
) -> anyhow::Result<(Mode2VerificationResult, u32, u128)> {
    verify_mode2_transactions_until_confirmed_with_timeout(
        config,
        db_path,
        tx_ids,
        scenario,
        config.timeout(config.timeouts.confirmation_secs),
    )
    .await
}

async fn verify_mode2_transactions_until_confirmed_with_timeout(
    config: &Config,
    db_path: &Path,
    tx_ids: &[String],
    scenario: ScenarioName,
    timeout: Duration,
) -> anyhow::Result<(Mode2VerificationResult, u32, u128)> {
    if tx_ids.is_empty() {
        return Ok((Mode2VerificationResult::default(), 0, 0));
    }

    let start = Instant::now();
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

pub(super) fn mode2_verification_confirmed(
    verification: &Mode2VerificationResult,
    tx_ids: &[String],
) -> bool {
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
    let mut total_scan_no_progress_attempts = 0u64;
    let mut stopped_without_progress = false;

    loop {
        attempts = attempts.saturating_add(1);
        let scan_report = scan_to_tip(
            db_path,
            password,
            &config.network.base_node_http_url,
            config.benchmark.scan_batch_size,
            config.benchmark.c_min,
            config.timeout(config.timeouts.scan_batch_secs),
        )
        .await
        .with_context(|| format!("mode2 settle gate {label} scan failed"))?;
        total_scan_wall_ms = total_scan_wall_ms.saturating_add(scan_report.wall_ms);
        total_scan_no_progress_attempts =
            total_scan_no_progress_attempts.saturating_add(scan_report.no_progress_attempts);
        stopped_without_progress |= scan_report.stopped_without_progress;
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
            "mode2 settle gate {label}: scanned_height={last_height} tip_height={tip_height} target_tip={target_tip_height} scan_no_progress_attempts={} scan_stopped_without_progress={}",
            total_scan_no_progress_attempts, stopped_without_progress
        );

        if mode2_settle_gate_ready(last_height, tip_height, target_tip_height) {
            return Ok(format!(
                "Mode 2 settle gate {label}: scanned_height {initial_scan_height}->{last_height} tip_height {initial_tip_height}->{tip_height} target_tip={target_tip_height} attempts={attempts} scan_wall_ms={total_scan_wall_ms} scan_no_progress_attempts={} scan_stopped_without_progress={}",
                total_scan_no_progress_attempts, stopped_without_progress
            ));
        }
        if start.elapsed() >= timeout {
            bail!(
                "mode2 settle gate {label} timed out after {}s waiting for both wallet scan and base-node tip to reach target {target_tip_height}; scanned_height={last_height} tip_height={tip_height} scan_no_progress_attempts={} scan_stopped_without_progress={}",
                timeout.as_secs(),
                total_scan_no_progress_attempts,
                stopped_without_progress
            );
        }

        let remaining = timeout.saturating_sub(start.elapsed());
        let sleep_for = Duration::from_secs(10).min(remaining);
        if !sleep_for.is_zero() {
            settle_gate_pause(sleep_for).await;
        }
    }
}

pub(super) fn mode2_settle_gate_ready(
    scanned_height: u64,
    tip_height: u64,
    target_tip_height: u64,
) -> bool {
    scanned_height >= target_tip_height && tip_height >= target_tip_height
}

pub(super) async fn settle_gate_pause(duration: Duration) {
    wait_one_interval(duration).await;
}

async fn wait_one_interval(duration: Duration) {
    if duration.is_zero() {
        return;
    }
    let mut interval = time::interval(duration);
    interval.tick().await;
    interval.tick().await;
}

pub(super) async fn base_node_tip_height(base_node_url: &str) -> anyhow::Result<u64> {
    let client = base_node_http_client()?;
    base_node_tip_height_with_client(&client, base_node_url).await
}

pub(super) async fn base_node_tip_height_with_client(
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

pub(super) fn base_node_http_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .context("building base-node HTTP client")
}

pub(super) fn base_node_endpoint_url(base_node_url: &str, path: &str) -> anyhow::Result<url::Url> {
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
    let balance_before = account_snapshot(&request.db_path)
        .ok()
        .map(|snapshot| snapshot.available_microtari);
    let rounds = s1_round_plan(config, config.benchmark.mode2_live_max_s1_txs);
    let mut round_metrics = Vec::new();

    for round in rounds {
        let mut round_summary = ScenarioSendSummary {
            attempted: round.tx_count,
            ..ScenarioSendSummary::default()
        };
        let round_start = Instant::now();
        let round_balance_before = account_snapshot(&request.db_path)
            .ok()
            .map(|snapshot| snapshot.available_microtari);
        let mut spendable_amounts = match spendable_output_amounts(&request.db_path) {
            Ok(amounts) => amounts,
            Err(error) => {
                round_summary.failure_count = round_summary.failure_count.saturating_add(1);
                round_summary.errors.push(format!(
                    "mode2 S1 round {} could not read spendable amounts: {error:#}",
                    round.round_index
                ));
                total.add_batch(round.round_index, round_summary);
                break;
            }
        };
        spendable_amounts.sort_unstable_by(|a, b| b.cmp(a));
        if spendable_amounts.len() != round.tx_count as usize {
            round_summary.failure_count = round_summary.failure_count.saturating_add(1);
            round_summary.errors.push(format!(
                "mode2 S1 round {} expected {} spendable inputs before dispatch, observed {}; refusing noncanonical state",
                round.round_index,
                round.tx_count,
                spendable_amounts.len()
            ));
            total.add_batch(round.round_index, round_summary);
            break;
        }
        for tx_index in 1..=round.tx_count {
            println!(
                "new_wallet/S1 round {} tx {}/{} outputs={}",
                round.round_index, tx_index, round.tx_count, round.outputs_per_tx
            );
            let input = spendable_amounts[(tx_index - 1) as usize];
            let result = exact_no_change_split(
                input,
                round.outputs_per_tx,
                request.fee_rate.0,
                STEALTH_OUTPUT_GRAMS,
            )
            .map(|plan| {
                plan.child_amounts
                    .into_iter()
                    .map(|amount| (request.recipient.clone(), amount))
                    .collect()
            });
            let result = match result {
                Ok(recipients) => {
                    construct_sign_broadcast_one_sided_recipient_amounts_owned(
                        request.clone(),
                        recipients,
                    )
                    .await
                }
                Err(error) => Err(error),
            };
            round_summary
                .construction_complete_ms
                .push(round_start.elapsed().as_millis());
            round_summary.record_attempt(tx_index, result);
        }
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
            match verify_mode2_transactions_until_confirmed(
                config,
                &request.db_path,
                &round_summary.tx_ids,
                ScenarioName::S1,
            )
            .await
            {
                Ok((verification, attempts, wall_ms)) => {
                    round_summary.apply_mode2_verification(verification);
                    record_mode2_verification_loop_metrics(&mut round_summary, attempts, wall_ms);
                }
                Err(error) => {
                    round_summary.failure_count = round_summary.failure_count.saturating_add(1);
                    round_summary.errors.push(format!(
                        "mode2 S1 round {} independent C_min verification failed: {error:#}",
                        round.round_index
                    ));
                }
            }
        }
        let observed_utxos = spendable_output_count(&request.db_path).ok();
        let round_balance_after = account_snapshot(&request.db_path)
            .ok()
            .map(|snapshot| snapshot.available_microtari);
        let fee_only_balance_delta_ok =
            round_balance_before
                .zip(round_balance_after)
                .is_some_and(|(before, after)| {
                    before.saturating_sub(after) == round_summary.fee_microtari
                });
        let independently_confirmed = mode2_summary_complete(&round_summary);
        if observed_utxos != Some(u64::from(round.target_utxos_after))
            || !fee_only_balance_delta_ok
            || !independently_confirmed
        {
            round_summary.failure_count = round_summary.failure_count.saturating_add(1);
            round_summary.errors.push(format!(
                "mode2 S1 round {} failed exact post-round invariant: observed_utxos={observed_utxos:?} target={} fee_only_balance_delta_ok={fee_only_balance_delta_ok} independently_confirmed={independently_confirmed}",
                round.round_index, round.target_utxos_after
            ));
        }
        round_summary.wall_ms = round_start.elapsed().as_millis();

        round_metrics.push(serde_json::json!({
            "round_index": round.round_index,
            "fanout": round.fanout,
            "tx_count": round.tx_count,
            "outputs_per_tx": round.outputs_per_tx,
            "target_utxos_after": round.target_utxos_after,
            "success_count": round_summary.success_count,
            "failure_count": round_summary.failure_count,
            "settle_note": settle_note,
            "observed_unspent_count": observed_utxos,
            "fee_only_balance_delta_ok": fee_only_balance_delta_ok,
            "independently_confirmed": independently_confirmed,
            "wall_ms": round_summary.wall_ms
        }));
        let has_failure = round_summary.failure_count > 0;
        total.add_batch(round.round_index, round_summary);
        if has_failure {
            break;
        }
    }

    total.wall_ms = start.elapsed().as_millis();
    let balance_after = account_snapshot(&request.db_path)
        .ok()
        .map(|snapshot| snapshot.available_microtari);
    add_balance_reconciliation_metrics(
        &mut total.extra_metrics,
        balance_before,
        balance_after,
        0,
        total.fee_microtari,
    );
    total.extra_metrics.insert(
        "unspent_after".to_string(),
        serde_json::json!(spendable_output_count(&request.db_path).ok()),
    );
    total
        .extra_metrics
        .insert("rounds".to_string(), serde_json::json!(round_metrics));
    total
}

pub(super) fn repeated_recipient(recipient: &str, count: usize) -> Vec<String> {
    let mut recipients = Vec::with_capacity(count);
    for _ in 0..count {
        recipients.push(recipient.to_string());
    }
    recipients
}

async fn run_s4_batches(
    config: &Config,
    request: OwnedOneSidedSendRequest,
    recipients: &[String],
) -> anyhow::Result<ScenarioSendSummary> {
    let mut total = ScenarioSendSummary::default();
    let start = Instant::now();
    for configured_batch in &config.benchmark.concurrent_batches {
        let attempts = capped_attempts(*configured_batch, config.benchmark.mode2_live_max_s4_batch);
        let arm_start = Instant::now();
        let deadline = time::Instant::now() + config.timeout(config.benchmark.s4_t_budget_secs);
        let selected = recipients.iter().take(attempts as usize).cloned().collect();
        let mut batch = run_send_attempts_concurrent(
            &format!("new_wallet/S4 batch {configured_batch}"),
            selected,
            request.clone(),
            deadline,
        )
        .await;
        let remaining = deadline.saturating_duration_since(time::Instant::now());
        if !batch.tx_ids.is_empty() && !remaining.is_zero() {
            let (verification, verification_attempts, verification_wall_ms) =
                verify_mode2_transactions_until_confirmed_with_timeout(
                    config,
                    &request.db_path,
                    &batch.tx_ids,
                    ScenarioName::S4,
                    remaining,
                )
                .await?;
            batch.apply_mode2_verification(verification);
            record_mode2_verification_loop_metrics(
                &mut batch,
                verification_attempts,
                verification_wall_ms,
            );
        }
        batch.wall_ms = arm_start.elapsed().as_millis();
        if !batch.tx_ids.is_empty() && !mode2_summary_complete(&batch) {
            batch.errors.push(format!(
                "new_wallet/S4 batch {configured_batch} reached its absolute deadline before every submitted transaction was C_min-deep"
            ));
        }
        total.add_batch(*configured_batch, batch);
    }
    total.wall_ms = start.elapsed().as_millis();
    Ok(total)
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
    recipients: Vec<String>,
    request: OwnedOneSidedSendRequest,
    deadline: time::Instant,
) -> ScenarioSendSummary {
    let attempts = u32::try_from(recipients.len()).unwrap_or(u32::MAX);
    let mut summary = ScenarioSendSummary {
        attempted: attempts,
        ..ScenarioSendSummary::default()
    };
    let start = Instant::now();
    let mut join_set = JoinSet::new();
    for (index, recipient) in recipients.into_iter().enumerate() {
        let attempt = u32::try_from(index + 1).unwrap_or(u32::MAX);
        println!("{label} attempt {attempt}/{attempts} dispatching");
        let mut request = request.clone();
        request.recipient = recipient;
        let attempt_start = Instant::now();
        join_set.spawn(async move {
            let result = construct_sign_broadcast_one_sided_owned(request).await;
            (attempt, attempt_start.elapsed().as_millis(), result)
        });
    }
    while !join_set.is_empty() {
        let Ok(Some(result)) = time::timeout_at(deadline, join_set.join_next()).await else {
            let unfinished = u32::try_from(join_set.len()).unwrap_or(u32::MAX);
            join_set.abort_all();
            summary.failure_count = summary.failure_count.saturating_add(unfinished);
            summary.errors.push(format!(
                "{label} absolute deadline expired with {unfinished} dispatch task(s) unfinished"
            ));
            break;
        };
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

pub(super) fn record_mode2_send_summary(
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
pub(super) fn refresh_recorded_mode2_send_summary(
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

pub(super) fn capped_attempts(planned: u32, cap: u32) -> u32 {
    if cap == 0 { planned } else { planned.min(cap) }
}

pub(super) fn mode2_completed_transaction_status(status: &str) -> (u32, bool) {
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
