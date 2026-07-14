use super::mode2::capped_attempts;
use super::*;
use crate::payment_processor::PaymentProcessorDbSnapshot;

pub(super) fn pp_observation_complete(
    accepted_batch_ids: &[String],
    snapshot: &PaymentProcessorDbSnapshot,
    proofs: &BTreeMap<String, PpChainProof>,
) -> bool {
    !accepted_batch_ids.is_empty()
        && accepted_batch_ids.iter().all(|id| {
            snapshot
                .batches
                .iter()
                .find(|batch| &batch.id == id)
                .is_some_and(|batch| match batch.status.as_str() {
                    "CONFIRMED" => proofs.contains_key(id),
                    "FAILED" | "CANCELLED" => true,
                    _ => false,
                })
        })
}

pub(super) async fn annotate_mode3_payment_processor(
    config: &Config,
    book: &AddressBook,
    profile: &mut ResultProfile,
) -> anyhow::Result<()> {
    let Some(pp_seed) = book.addresses.get(WalletRole::PaymentProcessor.label()) else {
        return Ok(());
    };
    let start = Instant::now();
    let topology = start_mode3_topology(config, pp_seed).await;
    match topology {
        Ok(mut context) => {
            let s0_ok = record_mode3_s0(config, profile, &context, start.elapsed().as_millis());
            if !s0_ok {
                record_blocked_prerequisite_cells(
                    profile,
                    "payment_processor",
                    &[
                        ScenarioName::S1,
                        ScenarioName::S2,
                        ScenarioName::S3,
                        ScenarioName::S4,
                        ScenarioName::S5,
                        ScenarioName::S6,
                        ScenarioName::S7,
                    ],
                    "S0",
                );
                context._payment_processor.shutdown().await?;
                context._payment_receiver.shutdown().await?;
                return Ok(());
            }
            run_mode3_send_cells(config, profile, pp_seed, &context).await?;
            context._payment_processor.shutdown().await?;
            context._payment_receiver.shutdown().await?;
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
) -> bool {
    let available =
        amount_field_as_microtari(&context.receiver_balance, "available").unwrap_or_default();
    let expected = config.a_fund().map(|amount| amount.0).unwrap_or_default();
    let spendable_count =
        spendable_output_count(&payment_processor::payment_receiver_db_path(config)).ok();
    let (status, success_count, failure_count, error, mut metrics) =
        strict_s0_status(expected, available, spendable_count);
    let ok = status == CellStatus::Ok;
    add_s0_funding_observation(
        &mut metrics,
        config.funding.payment_processor.as_ref(),
        Some(context.receiver_birthday),
    );

    let Some(mode) = profile.modes.get_mut("payment_processor") else {
        return false;
    };
    let Some(cell) = mode.scenarios.get_mut("S0") else {
        return false;
    };
    cell.record_repetition(Repetition {
        run: 1,
        status,
        wall_ms: Some(wall_ms),
        success_count,
        failure_count,
        fee_microtari: Some(0),
        error,
        metrics: Some(metrics),
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
    ok
}

fn record_mode3_startup_failure(profile: &mut ResultProfile, wall_ms: u128, error: anyhow::Error) {
    let Some(mode) = profile.modes.get_mut("payment_processor") else {
        return;
    };
    for scenario in [
        ScenarioName::S0,
        ScenarioName::S1,
        ScenarioName::S2,
        ScenarioName::S3,
        ScenarioName::S4,
        ScenarioName::S5,
        ScenarioName::S6,
        ScenarioName::S7,
    ] {
        let Some(cell) = mode.scenarios.get_mut(scenario.as_str()) else {
            continue;
        };
        cell.record_repetition(Repetition {
            run: 1,
            status: CellStatus::HarnessError,
            wall_ms: Some(wall_ms),
            success_count: 0,
            failure_count: 1,
            fee_microtari: Some(0),
            error: Some(format!("{error:#}")),
            metrics: Some(unavailable_balance_metrics(
                scenario,
                "Mode 3 topology failed before final wallet balance could be observed",
            )),
        });
        cell.notes
            .push("Mode 3 topology startup failed before scenario dispatch".to_string());
    }
}

async fn run_mode3_send_cells(
    config: &Config,
    profile: &mut ResultProfile,
    pp_seed: &crate::seeds::SeedMaterial,
    context: &Mode3TopologyContext,
) -> anyhow::Result<()> {
    let amount = parse_amount(&config.benchmark.mode3_payment_amount)?;
    let s1_rounds = s1_round_plan(config, config.benchmark.mode3_live_max_s1_batches);
    let pp_db_path = payment_processor::payment_receiver_db_path(config);
    let s1_components_before = account_balance(&pp_db_path).ok();
    let s1_balance_before = account_snapshot(&pp_db_path)
        .ok()
        .map(|snapshot| snapshot.available_microtari);
    let s1 = run_pp_s1_rounds(config, context, &pp_seed.address, &s1_rounds).await;
    let mut s1_extra = serde_json::Map::new();
    s1_extra.insert("rounds".to_string(), s1_round_metrics(&s1_rounds));
    let mut s1 = s1.with_extra_metrics(s1_extra);
    s1.tip_end_height = base_node_tip_height(&config.network.base_node_http_url)
        .await
        .ok();
    let s1_components_after = account_balance(&pp_db_path).ok();
    let s1_balance_after = account_snapshot(&pp_db_path)
        .ok()
        .map(|snapshot| snapshot.available_microtari);
    add_balance_reconciliation_metrics(
        &mut s1.extra_metrics,
        s1_balance_before,
        s1_balance_after,
        0,
        s1.chain_proofs
            .values()
            .map(|proof| proof.fee_microtari)
            .fold(0, u64::saturating_add),
    );
    add_balance_component_metrics(
        &mut s1.extra_metrics,
        s1_components_before,
        s1_components_after,
    );
    s1.extra_metrics.insert(
        "unspent_after".to_string(),
        serde_json::json!(spendable_output_count(&pp_db_path).ok()),
    );
    record_pp_summary(
        profile,
        ScenarioName::S1,
        &s1,
        vec![format!(
            "Mode 3 S1 drove exact balanced self-payment/change rounds through /v1/payment-batches; attempted_batches={} attempted_payments={} cap={}",
            s1.attempted_batches, s1.attempted_payments, config.benchmark.mode3_live_max_s1_batches
        )],
    );
    if !pp_summary_complete(&s1) {
        record_blocked_prerequisite_cells(
            profile,
            "payment_processor",
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

    let s4_components_before = account_balance(&pp_db_path).ok();
    let s4_balance_before = account_snapshot(&pp_db_path)
        .ok()
        .map(|snapshot| snapshot.available_microtari);
    let s4_recipients = derive_distinct_recipient_pool(128)?;
    let mut s4 = run_pp_s4_batches(config, context, &s4_recipients, amount).await;
    s4.tip_end_height = base_node_tip_height(&config.network.base_node_http_url)
        .await
        .ok();
    let s4_fee_microtari = s4.verified_fee_total();
    let s4_confirmed_payments = s4.independently_confirmed_payments();
    let s4_expected_balance = s4_balance_before.and_then(|before| {
        before.checked_sub(
            u64::from(s4_confirmed_payments)
                .saturating_mul(amount.0)
                .saturating_add(s4_fee_microtari),
        )
    });
    if let Some(expected) = s4_expected_balance
        && let Err(error) = wait_for_pp_receiver_balance(config, &pp_db_path, expected).await
    {
        let error = format!("PP S4 receiver state did not converge: {error:#}");
        s4.errors.push(error.clone());
        s4.state_observation_error = Some(error);
    }
    let s4_components_after = account_balance(&pp_db_path).ok();
    let s4_balance_after = account_snapshot(&pp_db_path)
        .ok()
        .map(|snapshot| snapshot.available_microtari);
    add_balance_reconciliation_metrics(
        &mut s4.extra_metrics,
        s4_balance_before,
        s4_balance_after,
        u64::from(s4_confirmed_payments).saturating_mul(amount.0),
        s4_fee_microtari,
    );
    add_balance_component_metrics(
        &mut s4.extra_metrics,
        s4_components_before,
        s4_components_after,
    );
    s4.extra_metrics.insert(
        "unspent_after".to_string(),
        serde_json::json!(spendable_output_count(&pp_db_path).ok()),
    );
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
    let s5_recipient_set = s5_recipients.clone();
    let s5_components_before = account_balance(&pp_db_path).ok();
    let s5_balance_before = account_snapshot(&pp_db_path)
        .ok()
        .map(|snapshot| snapshot.available_microtari);
    let s5_unspent_before = spendable_output_count(&pp_db_path).ok();
    let mut s5 = run_pp_recipient_batches_sequential(
        config,
        context,
        "payment_processor/S5",
        ScenarioName::S5,
        recipient_batches(s5_recipients, config.benchmark.s5_k),
        amount,
    )
    .await;
    s5.tip_end_height = base_node_tip_height(&config.network.base_node_http_url)
        .await
        .ok();
    s5.extra_metrics.insert(
        "recipient_set".to_string(),
        serde_json::json!(s5_recipient_set),
    );
    s5.extra_metrics.insert(
        "s5_batch_size".to_string(),
        serde_json::json!(config.benchmark.s5_k),
    );
    let s5_fee_microtari = s5.verified_fee_total();
    let s5_confirmed_payments = s5.independently_confirmed_payments();
    let s5_expected_balance = s5_balance_before.and_then(|before| {
        before.checked_sub(
            u64::from(s5_confirmed_payments)
                .saturating_mul(amount.0)
                .saturating_add(s5_fee_microtari),
        )
    });
    if let Some(expected) = s5_expected_balance
        && let Err(error) = wait_for_pp_receiver_balance(config, &pp_db_path, expected).await
    {
        let error = format!("PP S5 receiver state did not converge: {error:#}");
        s5.errors.push(error.clone());
        s5.state_observation_error = Some(error);
    }
    let s5_components_after = account_balance(&pp_db_path).ok();
    let s5_balance_after = account_snapshot(&pp_db_path)
        .ok()
        .map(|snapshot| snapshot.available_microtari);
    add_balance_reconciliation_metrics(
        &mut s5.extra_metrics,
        s5_balance_before,
        s5_balance_after,
        u64::from(s5_confirmed_payments).saturating_mul(amount.0),
        s5_fee_microtari,
    );
    add_balance_component_metrics(
        &mut s5.extra_metrics,
        s5_components_before,
        s5_components_after,
    );
    s5.extra_metrics.insert(
        "s5_complete".to_string(),
        serde_json::json!(pp_summary_complete(&s5)),
    );
    s5.extra_metrics.insert(
        "s5_unavailable_reason".to_string(),
        serde_json::json!(
            (!pp_summary_complete(&s5)).then_some(
                "one or more PP batches or receiver-state observations did not complete"
            )
        ),
    );
    s5.extra_metrics.insert(
        "unspent_before".to_string(),
        serde_json::json!(s5_unspent_before),
    );
    s5.extra_metrics.insert(
        "unspent_after".to_string(),
        serde_json::json!(spendable_output_count(&pp_db_path).ok()),
    );
    record_pp_summary(
        profile,
        ScenarioName::S5,
        &s5,
        vec![format!(
            "Mode 3 S5 payment-batch arm used {} sequential /v1/payment-batches requests with total_items={} of configured S5_M={} and S5_K={}; cap={}",
            s5.batch_ids.len(),
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

async fn run_pp_s1_rounds(
    config: &Config,
    context: &Mode3TopologyContext,
    self_address: &str,
    rounds: &[S1RoundPlan],
) -> PpScenarioSummary {
    let db_path = payment_processor::payment_receiver_db_path(config);
    let start = Instant::now();
    let tip_start_height = base_node_tip_height(&config.network.base_node_http_url)
        .await
        .ok();
    let mut total = PpScenarioSummary {
        tip_start_height,
        ..PpScenarioSummary::default()
    };
    for round in rounds {
        let round_start = Instant::now();
        let round_balance_before = account_snapshot(&db_path)
            .ok()
            .map(|snapshot| snapshot.available_microtari);
        let mut spendable_amounts = match spendable_output_amounts(&db_path) {
            Ok(amounts) => amounts,
            Err(error) => {
                total.failed_batches = total.failed_batches.saturating_add(1);
                total.errors.push(format!(
                    "PP S1 round {} could not read spendable amounts: {error:#}",
                    round.round_index
                ));
                break;
            }
        };
        spendable_amounts.sort_unstable_by(|a, b| b.cmp(a));
        if spendable_amounts.len() != round.tx_count as usize {
            total.failed_batches = total.failed_batches.saturating_add(1);
            total.errors.push(format!(
                "PP S1 round {} expected {} spendable inputs before dispatch, observed {}; refusing noncanonical state",
                round.round_index,
                round.tx_count,
                spendable_amounts.len()
            ));
            break;
        }
        let mut round_summary = PpScenarioSummary {
            attempted_batches: round.tx_count,
            attempted_payments: round
                .tx_count
                .saturating_mul(round.outputs_per_tx.saturating_sub(1)),
            tip_start_height,
            ..PpScenarioSummary::default()
        };
        for tx_index in 1..=round.tx_count {
            let input = spendable_amounts[(tx_index - 1) as usize];
            let submit_offset_ms = round_start.elapsed().as_millis();
            let result = exact_pp_split_with_change(input, round.outputs_per_tx).map(|plan| {
                plan.payment_amounts
                    .into_iter()
                    .map(|amount| (self_address.to_string(), amount))
                    .collect()
            });
            let result = match result {
                Ok(recipients) => {
                    submit_pp_batch_to_recipient_amounts(
                        &context.client,
                        ScenarioName::S1,
                        tx_index,
                        recipients,
                        false,
                    )
                    .await
                }
                Err(error) => Err(error),
            };
            let completed_offset_ms = round_start.elapsed().as_millis();
            round_summary
                .construction_complete_ms
                .push(completed_offset_ms);
            round_summary.record_batch(
                tx_index,
                submit_offset_ms,
                completed_offset_ms,
                vec![self_address.to_string(); round.outputs_per_tx.saturating_sub(1) as usize],
                result,
            );
            if round_summary.failed_batches > 0 {
                break;
            }
        }
        round_summary
            .observe_db(config, pp_observation_timeout(config, ScenarioName::S1))
            .await;
        round_summary.wall_ms = round_start.elapsed().as_millis();
        let observed_fees = round_summary
            .chain_proofs
            .values()
            .map(|proof| proof.fee_microtari)
            .fold(0, u64::saturating_add);
        if pp_summary_complete(&round_summary) {
            let expected_balance =
                round_balance_before.map(|before| before.saturating_sub(observed_fees));
            if let Err(error) = wait_for_pp_receiver_round_state(
                config,
                &db_path,
                u64::from(round.target_utxos_after),
                expected_balance,
            )
            .await
            {
                round_summary.failed_batches = round_summary.failed_batches.saturating_add(1);
                round_summary.errors.push(format!(
                    "PP S1 round {} companion-wallet refresh failed: {error:#}",
                    round.round_index
                ));
            }
        }
        let observed_utxos = spendable_output_count(&db_path).ok();
        let round_balance_after = account_snapshot(&db_path)
            .ok()
            .map(|snapshot| snapshot.available_microtari);
        let fee_only_balance_delta_ok = round_balance_before
            .zip(round_balance_after)
            .is_some_and(|(before, after)| before.saturating_sub(after) == observed_fees);
        round_summary.extra_metrics.insert(
            format!("round_{}", round.round_index),
            serde_json::json!({
                "target_utxos_after": round.target_utxos_after,
                "observed_unspent_count": observed_utxos,
                "observed_fee_microtari": observed_fees,
                "success_count": round_summary.accepted_batches,
                "failure_count": round_summary.failed_batches,
                "fee_only_balance_delta_ok": fee_only_balance_delta_ok,
                "wall_ms": round_summary.wall_ms
            }),
        );
        if observed_utxos != Some(u64::from(round.target_utxos_after))
            || !fee_only_balance_delta_ok
            || !pp_summary_complete(&round_summary)
        {
            round_summary.failed_batches = round_summary.failed_batches.saturating_add(1);
            round_summary.errors.push(format!(
                "PP S1 round {} failed exact UTXO/fee/C_min invariants",
                round.round_index
            ));
        }
        let failed = round_summary.failed_batches > 0;
        total.add_batch(round.round_index, round_summary);
        if failed {
            break;
        }
    }
    total.wall_ms = start.elapsed().as_millis();
    total
}

async fn wait_for_pp_receiver_round_state(
    config: &Config,
    db_path: &Path,
    expected_outputs: u64,
    expected_balance: Option<u64>,
) -> anyhow::Result<()> {
    let start = Instant::now();
    let timeout = config.timeout(config.timeouts.confirmation_secs);
    let mut interval = time::interval(Duration::from_secs(5));
    loop {
        interval.tick().await;
        let outputs = spendable_output_count(db_path).ok();
        let balance = account_snapshot(db_path)
            .ok()
            .map(|snapshot| snapshot.available_microtari);
        if pp_receiver_state_ready(outputs, balance, expected_outputs, expected_balance) {
            return Ok(());
        }
        if start.elapsed() >= timeout {
            bail!(
                "payment receiver did not converge within {timeout:?}: expected_outputs={expected_outputs} observed_outputs={outputs:?} expected_balance={expected_balance:?} observed_balance={balance:?}"
            );
        }
    }
}

async fn wait_for_pp_receiver_balance(
    config: &Config,
    db_path: &Path,
    expected_balance: u64,
) -> anyhow::Result<()> {
    let start = Instant::now();
    let timeout = config.timeout(config.timeouts.confirmation_secs);
    let mut interval = time::interval(Duration::from_secs(5));
    loop {
        interval.tick().await;
        let observed = account_snapshot(db_path)
            .ok()
            .map(|snapshot| snapshot.available_microtari);
        if observed == Some(expected_balance) {
            return Ok(());
        }
        if start.elapsed() >= timeout {
            bail!(
                "payment receiver balance did not converge within {timeout:?}: expected={expected_balance} observed={observed:?}"
            );
        }
    }
}

pub(super) fn pp_receiver_state_ready(
    observed_outputs: Option<u64>,
    observed_balance: Option<u64>,
    expected_outputs: u64,
    expected_balance: Option<u64>,
) -> bool {
    observed_outputs == Some(expected_outputs)
        && expected_balance.is_none_or(|expected| observed_balance == Some(expected))
}

async fn run_pp_s4_batches(
    config: &Config,
    context: &Mode3TopologyContext,
    recipients: &[String],
    amount: MicroMinotari,
) -> PpScenarioSummary {
    let start = Instant::now();
    let mut total = PpScenarioSummary::default();
    for configured_batch in &config.benchmark.concurrent_batches {
        let attempts = capped_attempts(*configured_batch, config.benchmark.mode3_live_max_s4_batch);
        let selected = recipients.iter().take(attempts as usize).cloned().collect();
        let batch = run_pp_batches_concurrent(
            config,
            context,
            &format!("payment_processor/S4 batch {configured_batch}"),
            ScenarioName::S4,
            selected,
            amount,
        )
        .await;
        total.add_batch(*configured_batch, batch);
    }
    total.wall_ms = start.elapsed().as_millis();
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
        tip_start_height: base_node_tip_height(&config.network.base_node_http_url)
            .await
            .ok(),
        ..PpScenarioSummary::default()
    };
    let start = Instant::now();
    for (index, recipients) in recipient_batches.into_iter().enumerate() {
        let batch_index = u32::try_from(index + 1).unwrap_or(u32::MAX);
        println!("{label} batch {batch_index}/{attempted_batches} dispatching");
        let submit_offset_ms = start.elapsed().as_millis();
        let expected_recipients = recipients.clone();
        let result = submit_pp_batch_to_recipients(
            &context.client,
            scenario,
            batch_index,
            recipients,
            amount,
        )
        .await;
        let completed_offset_ms = start.elapsed().as_millis();
        summary.construction_complete_ms.push(completed_offset_ms);
        summary.record_batch(
            batch_index,
            submit_offset_ms,
            completed_offset_ms,
            expected_recipients,
            result,
        );
    }
    summary
        .observe_db(config, pp_observation_timeout(config, scenario))
        .await;
    summary.wall_ms = start.elapsed().as_millis();
    summary
}

#[allow(clippy::too_many_arguments)]
async fn run_pp_batches_concurrent(
    config: &Config,
    context: &Mode3TopologyContext,
    label: &str,
    scenario: ScenarioName,
    recipients: Vec<String>,
    amount: MicroMinotari,
) -> PpScenarioSummary {
    let batch_count = u32::try_from(recipients.len()).unwrap_or(u32::MAX);
    let mut summary = PpScenarioSummary {
        attempted_batches: batch_count,
        attempted_payments: batch_count,
        tip_start_height: base_node_tip_height(&config.network.base_node_http_url)
            .await
            .ok(),
        ..PpScenarioSummary::default()
    };
    let start = Instant::now();
    let mut join_set = JoinSet::new();
    let mut pending = BTreeMap::new();
    let budget = pp_observation_timeout(config, scenario);
    let deadline = time::Instant::now() + budget;
    for (index, recipient) in recipients.into_iter().enumerate() {
        let batch_index = u32::try_from(index + 1).unwrap_or(u32::MAX);
        println!("{label} batch {batch_index}/{batch_count} dispatching");
        let context = context.clone_for_task();
        let submit_offset_ms = start.elapsed().as_millis();
        pending.insert(batch_index, (submit_offset_ms, recipient.clone()));
        let arm_start = start;
        join_set.spawn(async move {
            let result = submit_pp_batch(
                &context.client,
                scenario,
                batch_index,
                1,
                &recipient,
                amount,
            )
            .await;
            (
                batch_index,
                submit_offset_ms,
                arm_start.elapsed().as_millis(),
                recipient,
                result,
            )
        });
    }
    while !join_set.is_empty() {
        let Ok(Some(result)) = time::timeout_at(deadline, join_set.join_next()).await else {
            join_set.abort_all();
            break;
        };
        match result {
            Ok((batch_index, submit_offset_ms, completed_ms, recipient, send)) => {
                pending.remove(&batch_index);
                summary.construction_complete_ms.push(completed_ms);
                summary.record_batch(
                    batch_index,
                    submit_offset_ms,
                    completed_ms,
                    vec![recipient],
                    send,
                );
            }
            Err(error) => summary.errors.push(format!("task join error: {error}")),
        }
    }
    let timed_out_at = start.elapsed().as_millis();
    for (batch_index, (submit_offset_ms, recipient)) in pending {
        summary.record_batch(
            batch_index,
            submit_offset_ms,
            timed_out_at,
            vec![recipient],
            Err(anyhow::anyhow!(
                "{label} absolute deadline expired before dispatch task completed"
            )),
        );
    }
    summary
        .observe_db(
            config,
            deadline.saturating_duration_since(time::Instant::now()),
        )
        .await;
    summary.wall_ms = start.elapsed().as_millis();
    summary
}

pub(super) fn pp_observation_timeout(config: &Config, scenario: ScenarioName) -> Duration {
    if scenario == ScenarioName::S4 {
        config.timeout(config.benchmark.s4_t_budget_secs)
    } else {
        Duration::from_secs(config.timeouts.confirmation_secs.max(30))
    }
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
    let recipients = recipients
        .into_iter()
        .map(|recipient| (recipient, amount.0))
        .collect();
    submit_pp_batch_to_recipient_amounts(client, scenario, batch_index, recipients, true).await
}

async fn submit_pp_batch_to_recipient_amounts(
    client: &PaymentProcessorClient,
    scenario: ScenarioName,
    batch_index: u32,
    recipients: Vec<(String, u64)>,
    include_payment_id: bool,
) -> anyhow::Result<PpBatchSubmission> {
    let items = recipients
        .into_iter()
        .enumerate()
        .map(|(item_index, (recipient_address, amount))| {
            let payment_index = item_index + 1;
            Ok(BulkPaymentItem {
                client_id: format!(
                    "bench-{}-{}-{}-{}",
                    scenario.as_str().to_lowercase(),
                    chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
                    batch_index,
                    payment_index
                ),
                recipient_address,
                amount: i64::try_from(amount).context("mode3 payment amount exceeds i64")?,
                payment_id: include_payment_id.then(|| {
                    format!(
                        "wallet-bench-{}-{batch_index}-{payment_index}",
                        scenario.as_str()
                    )
                }),
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let api_start = Instant::now();
    let response = client
        .create_payment_batch(&BulkPaymentRequest {
            account_name: "default".to_string(),
            items,
        })
        .await?;
    let api_accept_ms = api_start.elapsed().as_millis();
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
        api_accept_ms,
    })
}

pub(super) fn recipient_batches(recipients: Vec<String>, batch_size: u32) -> Vec<Vec<String>> {
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

pub(super) fn record_pp_summary(
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
    let confirmed_batches = u32::try_from(confirmed_batch_count).unwrap_or(u32::MAX);
    let terminal_failures = summary.attempted_batches.saturating_sub(confirmed_batches);
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
    } else if terminal_failures == 0
        && observation_complete
        && all_verified_ok
        && summary.state_observation_error.is_none()
    {
        CellStatus::Ok
    } else {
        CellStatus::Failed
    };
    cell.record_repetition(Repetition {
        run: 1,
        status,
        wall_ms: Some(summary.wall_ms),
        success_count: confirmed_batches,
        failure_count: terminal_failures,
        fee_microtari: Some(summary.verified_fee_total()),
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
