use super::*;
use anyhow::{Context, bail};

pub(super) async fn annotate_mode1_console_wallet(
    config: &Config,
    book: &AddressBook,
    profile: &mut ResultProfile,
) -> anyhow::Result<()> {
    let Some(old_seed) = book.addresses.get(WalletRole::OldWallet.label()) else {
        return Ok(());
    };
    let start = Instant::now();
    let topology = start_mode1_console_wallet(config, old_seed).await;
    match topology {
        Ok(mut context) => {
            let spendable_count = mode1_unspent_count(&mut context.client).await.ok();
            let s0_ok = record_mode1_s0(
                config,
                profile,
                &context,
                start.elapsed().as_millis(),
                spendable_count,
            );
            if !s0_ok {
                record_blocked_prerequisite_cells(
                    profile,
                    "old_wallet",
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
                context.process.shutdown().await?;
                return Ok(());
            }
            run_mode1_send_cells(config, profile, old_seed, &mut context).await?;
            context.process.shutdown().await?;
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
    start_mode1_console_wallet_with_recovery(config, old_seed, false, config.a_fund()?.0).await
}

pub(super) async fn start_mode1_console_wallet_with_recovery(
    config: &Config,
    old_seed: &crate::seeds::SeedMaterial,
    recovery: bool,
    min_available: u64,
) -> anyhow::Result<Mode1ConsoleContext> {
    let password = wallet_password(&config.seeds.wallet_password_env)?;
    let base_path = old_wallet_base_path(config);
    let config_path = base_path.join("config/config.toml");
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    write_mode1_runtime_config(config, &config_path)?;
    let grpc_bind = grpc_bind_multiaddr(&config.modes.old_wallet_grpc_address)?;
    let birthday = config
        .funding
        .old_wallet
        .as_ref()
        .and_then(|funding| funding.birthday)
        .unwrap_or_else(|| mode1_wallet_birthday(old_seed));
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
        .arg(&grpc_bind);
    if recovery {
        command.arg("--recovery");
    }
    let mut process = ManagedProcess::spawn(
        "mode1-console-wallet",
        command,
        &config.paths.data_dir.join("logs"),
    )?;
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
    let balance = wait_for_mode1_balance(config, &mut context, min_available).await?;
    context.balance = Some(balance);
    Ok(context)
}

pub(super) fn write_mode1_runtime_config(config: &Config, path: &Path) -> anyhow::Result<()> {
    let peers = config
        .network
        .mode1_base_node_service_peer
        .iter()
        .map(|peer| format!("\"{}\"", peer.replace('"', "\\\"")))
        .collect::<Vec<_>>()
        .join(", ");
    let contents = format!(
        "[wallet]\nbase_node_service_peers = [{peers}]\nhttp_server_url = \"{}\"\nfallback_http_server_url = \"{}\"\nfee_per_gram = {}\nnum_required_confirmations = {}\n",
        config.network.base_node_http_url.replace('"', "\\\""),
        config.network.base_node_http_url.replace('"', "\\\""),
        config.fee_rate()?.0,
        config.benchmark.c_min
    );
    fs::write(path, contents)
        .with_context(|| format!("writing Mode 1 runtime config {}", path.display()))
}

pub(super) async fn send_mode1_operator_one_sided(
    config: &Config,
    old_seed: &crate::seeds::SeedMaterial,
    recipient: &str,
    amount: MicroMinotari,
) -> anyhow::Result<()> {
    let mut context =
        start_mode1_console_wallet_with_recovery(config, old_seed, false, amount.0).await?;
    let actual = context
        .client
        .get_address(grpc::Empty {})
        .await
        .context("querying Mode 1 wallet address before sweep")?
        .into_inner()
        .one_sided_address;
    if !mode1_address_matches_seed(&actual, old_seed)? {
        bail!("Mode 1 sweep refused: console-wallet address does not match configured seed");
    }
    let outcome = submit_mode1_transfer(
        &mut context.client,
        ScenarioName::S0,
        1,
        1,
        true,
        recipient,
        amount,
        config.fee_rate()?.0,
    )
    .await?;
    if outcome.failure_count > 0 || outcome.tx_ids.is_empty() {
        bail!(
            "Mode 1 sweep submission failed: {}",
            outcome.errors.join("; ")
        );
    }
    println!(
        "sweep-mode1 submitted amount_microtari={} tx_ids={} fee_microtari={}",
        amount.0,
        outcome.tx_ids.join(","),
        outcome.fee_microtari
    );
    Ok(())
}

pub(super) fn old_wallet_base_path(config: &Config) -> PathBuf {
    config.paths.data_dir.join("old-wallet-console")
}

pub(super) fn mode1_wallet_birthday(seed: &crate::seeds::SeedMaterial) -> u16 {
    if seed.birthday == 0 {
        current_birthday()
    } else {
        seed.birthday
    }
}

pub(super) fn grpc_bind_multiaddr(address: &str) -> anyhow::Result<String> {
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

pub(super) fn mode1_scan_grpc_address(
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

pub(super) async fn wait_for_mode1_grpc_address(
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

pub(super) async fn wait_for_mode1_scan_to_tip(
    process: &mut Mode1ConsoleProcess,
    client: &mut WalletGrpcClient<tonic::transport::Channel>,
    target_tip: u64,
    timeout: Duration,
    no_progress_timeout: Duration,
) -> anyhow::Result<u64> {
    let start = Instant::now();
    let mut last_progress = Instant::now();
    let mut interval = time::interval(Duration::from_secs(5));
    let mut last_scanned_height = None;
    loop {
        interval.tick().await;
        if let Some(status) = process.try_wait()? {
            bail!(
                "minotari_console_wallet exited during fresh scan with status {status}; stdout_log={} stderr_log={}",
                process.stdout_path.display(),
                process.stderr_path.display()
            );
        }
        if start.elapsed() > timeout {
            bail!(
                "console wallet fresh scan did not reach target tip {:?} within {:?}; scanned_height={:?}; stdout_log={} stderr_log={}",
                target_tip,
                timeout,
                last_scanned_height,
                process.stdout_path.display(),
                process.stderr_path.display()
            );
        }
        let remaining = timeout.saturating_sub(start.elapsed());
        let call_timeout = Duration::from_secs(10).min(remaining);
        let state = match time::timeout(call_timeout, client.get_state(grpc::GetStateRequest {}))
            .await
        {
            Ok(Ok(response)) => response.into_inner(),
            Ok(Err(error)) => {
                if start.elapsed() > timeout {
                    bail!(
                        "console wallet fresh scan state query failed after {:?}: {error}; scanned_height={:?}; stdout_log={} stderr_log={}",
                        timeout,
                        last_scanned_height,
                        process.stdout_path.display(),
                        process.stderr_path.display()
                    );
                }
                println!("mode1 fresh scan state query failed: {error}");
                continue;
            }
            Err(_) => {
                if start.elapsed() > timeout {
                    bail!(
                        "console wallet fresh scan state query timed out after {:?}; scanned_height={:?}; stdout_log={} stderr_log={}",
                        timeout,
                        last_scanned_height,
                        process.stdout_path.display(),
                        process.stderr_path.display()
                    );
                }
                println!("mode1 fresh scan state query timed out after {call_timeout:?}");
                continue;
            }
        };
        let scanned_height = state.scanned_height;
        if last_scanned_height.is_none_or(|previous| scanned_height > previous) {
            last_progress = Instant::now();
        }
        last_scanned_height = Some(scanned_height);
        if scanned_height >= target_tip {
            return Ok(scanned_height);
        }
        if last_progress.elapsed() > no_progress_timeout {
            bail!(
                "console wallet fresh scan made no height progress for {:?}; target_tip={:?}; scanned_height={scanned_height}; stdout_log={} stderr_log={}",
                no_progress_timeout,
                target_tip,
                process.stdout_path.display(),
                process.stderr_path.display()
            );
        }
        if start.elapsed() > timeout {
            bail!(
                "console wallet fresh scan did not reach target tip {target_tip} within {:?}; scanned_height={scanned_height}",
                timeout
            );
        }
        println!("mode1 fresh scan wait: scanned_height={scanned_height} target={target_tip}");
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
    spendable_count: Option<u64>,
) -> bool {
    let Some(mode) = profile.modes.get_mut("old_wallet") else {
        return false;
    };
    let Some(cell) = mode.scenarios.get_mut("S0") else {
        return false;
    };
    let balance = context.balance.as_ref();
    let available = balance.map(|b| b.available_balance).unwrap_or_default();
    let expected = config.a_fund().map(|amount| amount.0).unwrap_or_default();
    let (status, success_count, failure_count, error, mut metrics) =
        strict_s0_status(expected, available, spendable_count);
    let ok = status == CellStatus::Ok;
    add_s0_funding_observation(
        &mut metrics,
        config.funding.old_wallet.as_ref(),
        Some(context.birthday),
    );
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
        "Mode 1 topology started real minotari_console_wallet gRPC version {}; grpc_address={} grpc_bind={} birthday={} balance_available={} pending_in={} pending_out={}",
        context.version.as_deref().unwrap_or("unknown"),
        config.modes.old_wallet_grpc_address,
        context.grpc_bind,
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
    ok
}

fn record_mode1_startup_failure(profile: &mut ResultProfile, wall_ms: u128, error: anyhow::Error) {
    let Some(mode) = profile.modes.get_mut("old_wallet") else {
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
                "Mode 1 topology failed before final wallet balance could be observed",
            )),
        });
        cell.notes
            .push("Mode 1 console-wallet startup failed before scenario dispatch".to_string());
    }
}

async fn run_mode1_send_cells(
    config: &Config,
    profile: &mut ResultProfile,
    old_seed: &crate::seeds::SeedMaterial,
    context: &mut Mode1ConsoleContext,
) -> anyhow::Result<()> {
    let amount = parse_amount(&config.benchmark.mode1_payment_amount)?;
    let fee_rate = config.fee_rate()?.0;
    let s1_components_before = mode1_balance_components(&mut context.client).await.ok();
    let mut s1 = run_mode1_s1(config, &mut context.client, fee_rate).await;
    s1.tip_end_height = base_node_tip_height(&config.network.base_node_http_url)
        .await
        .ok();
    let s1_components_after = mode1_balance_components(&mut context.client).await.ok();
    add_balance_component_metrics(
        &mut s1.extra_metrics,
        s1_components_before,
        s1_components_after,
    );
    record_mode1_transfer_summary(
        profile,
        ScenarioName::S1,
        &s1,
        vec![format!(
            "Mode 1 S1 drove native gRPC CoinSplit rounds with requested_splits=target_outputs-1 so wallet change completes the exact output count; attempted_batches={} cap={}",
            s1.attempted_batches, config.benchmark.mode1_live_max_s1_txs
        )],
    );
    if !mode1_s1_complete(&s1) {
        record_blocked_prerequisite_cells(
            profile,
            "old_wallet",
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

    let s4_recipients = derive_distinct_recipient_pool(128)?;
    let s4_components_before = mode1_balance_components(&mut context.client).await.ok();
    let s4_balance_before = mode1_available_balance(&mut context.client).await.ok();
    let mut s4 = run_mode1_s4_batches(
        config,
        &mut context.client,
        &s4_recipients,
        amount,
        fee_rate,
    )
    .await;
    s4.tip_end_height = base_node_tip_height(&config.network.base_node_http_url)
        .await
        .ok();
    let s4_components_after = mode1_balance_components(&mut context.client).await.ok();
    let s4_balance_after = mode1_available_balance(&mut context.client).await.ok();
    let s4_success_payments =
        u32::try_from(s4.tx_infos.iter().filter(|tx| tx.confirmed).count()).unwrap_or(u32::MAX);
    add_balance_reconciliation_metrics(
        &mut s4.extra_metrics,
        s4_balance_before,
        s4_balance_after,
        u64::from(s4_success_payments).saturating_mul(amount.0),
        s4.fee_microtari,
    );
    add_balance_component_metrics(
        &mut s4.extra_metrics,
        s4_components_before,
        s4_components_after,
    );
    s4.extra_metrics.insert(
        "unspent_after".to_string(),
        serde_json::json!(mode1_unspent_count(&mut context.client).await.ok()),
    );
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
    let s5_components_before = mode1_balance_components(&mut context.client).await.ok();
    let s5_balance_before = mode1_available_balance(&mut context.client).await.ok();
    let s5_unspent_before = mode1_unspent_count(&mut context.client).await.ok();
    let mut s5 = run_mode1_s5(
        config,
        &mut context.client,
        &s5_recipients,
        amount,
        fee_rate,
    )
    .await;
    s5.tip_end_height = base_node_tip_height(&config.network.base_node_http_url)
        .await
        .ok();
    let s5_components_after = mode1_balance_components(&mut context.client).await.ok();
    s5.extra_metrics.insert(
        "recipient_set".to_string(),
        serde_json::json!(s5_recipients),
    );
    let s5_balance_after = mode1_available_balance(&mut context.client).await.ok();
    let s5_success_payments =
        u32::try_from(s5.tx_infos.iter().filter(|tx| tx.confirmed).count()).unwrap_or(u32::MAX);
    add_balance_reconciliation_metrics(
        &mut s5.extra_metrics,
        s5_balance_before,
        s5_balance_after,
        u64::from(s5_success_payments).saturating_mul(amount.0),
        s5.fee_microtari,
    );
    add_balance_component_metrics(
        &mut s5.extra_metrics,
        s5_components_before,
        s5_components_after,
    );
    s5.extra_metrics.insert(
        "unspent_before".to_string(),
        serde_json::json!(s5_unspent_before),
    );
    s5.extra_metrics.insert(
        "unspent_after".to_string(),
        serde_json::json!(mode1_unspent_count(&mut context.client).await.ok()),
    );
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
    fee_rate: u64,
) -> Mode1TransferSummary {
    let tip_start_height = base_node_tip_height(&config.network.base_node_http_url)
        .await
        .ok();
    let mut total = Mode1TransferSummary {
        tip_start_height,
        ..Mode1TransferSummary::default()
    };
    let start = Instant::now();
    let rounds = s1_round_plan(config, config.benchmark.mode1_live_max_s1_txs);
    let balance_before = mode1_available_balance(client).await.ok();
    for round in rounds {
        let round_start = Instant::now();
        let round_balance_before = mode1_available_balance(client).await.ok();
        let mut spendable_amounts = match mode1_unspent_amounts(client).await {
            Ok(amounts) => amounts,
            Err(error) => {
                total.failure_count = total.failure_count.saturating_add(1);
                total.errors.push(format!(
                    "mode1 S1 round {} could not read spendable amounts: {error:#}",
                    round.round_index
                ));
                break;
            }
        };
        spendable_amounts.sort_unstable_by(|a, b| b.cmp(a));
        if spendable_amounts.len() != round.tx_count as usize {
            total.failure_count = total.failure_count.saturating_add(1);
            total.errors.push(format!(
                "mode1 S1 round {} expected {} spendable inputs before dispatch, observed {}; refusing noncanonical state",
                round.round_index,
                round.tx_count,
                spendable_amounts.len()
            ));
            break;
        }
        let mut round_summary = Mode1TransferSummary {
            attempted_batches: round.tx_count,
            attempted_payments: round.tx_count.saturating_mul(round.outputs_per_tx),
            tip_start_height,
            ..Mode1TransferSummary::default()
        };
        for tx_index in 1..=round.tx_count {
            let input = spendable_amounts[(tx_index - 1) as usize];
            let plan = match mode1_coin_split_plan(input, round.outputs_per_tx, fee_rate) {
                Ok(plan) => plan,
                Err(error) => {
                    round_summary.failure_count = round_summary.failure_count.saturating_add(1);
                    round_summary.errors.push(format!(
                        "mode1 S1 round {} tx {tx_index} exact planner failed: {error:#}",
                        round.round_index
                    ));
                    break;
                }
            };
            println!(
                "old_wallet/S1 round {} tx {}/{} native coin-split outputs={} requested_splits={} input={} planned_fee={}",
                round.round_index,
                tx_index,
                round.tx_count,
                round.outputs_per_tx,
                plan.split_count,
                input,
                plan.fee_microtari
            );
            let submit_offset_ms = round_start.elapsed().as_millis();
            let result =
                submit_mode1_coin_split(client, plan.amount_per_split, plan.split_count, fee_rate)
                    .await;
            let completed_offset_ms = round_start.elapsed().as_millis();
            round_summary.record_batch(
                tx_index,
                round.outputs_per_tx,
                submit_offset_ms,
                completed_offset_ms,
                Vec::new(),
                result,
            );
            round_summary
                .construction_complete_ms
                .push(completed_offset_ms);
        }
        wait_for_mode1_summary_verification(
            client,
            &config.network.base_node_http_url,
            &mut round_summary,
            ScenarioName::S1,
            round_start.elapsed().as_millis(),
            config.timeout(config.timeouts.confirmation_secs),
            config.benchmark.c_min,
        )
        .await;
        round_summary.wall_ms = round_start.elapsed().as_millis();
        let observed_utxos = mode1_unspent_count(client).await.ok();
        let balance_after = mode1_available_balance(client).await.ok();
        let balance_delta_matches_fees =
            round_balance_before
                .zip(balance_after)
                .is_some_and(|(before, after)| {
                    before.saturating_sub(after) == round_summary.fee_microtari
                });
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
                "total_fee_microtari": round_summary.fee_microtari,
                "success_count": round_summary.success_count,
                "failure_count": round_summary.failure_count,
                "fee_only_balance_delta_ok": balance_delta_matches_fees,
                "wall_ms": round_summary.wall_ms
            }),
        );
        let round_complete = mode1_s1_complete(&round_summary)
            && observed_utxos == Some(u64::from(round.target_utxos_after))
            && balance_delta_matches_fees;
        if !round_complete {
            if round_summary.failure_count == 0 {
                round_summary.failure_count = round_summary.failure_count.saturating_add(1);
            }
            round_summary.errors.push(format!(
                "mode1 S1 round {} failed exact UTXO/fee/C_min invariants; stopping subsequent S1 rounds",
                round.round_index
            ));
        }
        total.add_batch(round.round_index, round_summary);
        if !round_complete || total.failure_count > 0 {
            break;
        }
    }
    total.wall_ms = start.elapsed().as_millis();
    let balance_after = mode1_available_balance(client).await.ok();
    add_balance_reconciliation_metrics(
        &mut total.extra_metrics,
        balance_before,
        balance_after,
        0,
        total.fee_microtari,
    );
    total.extra_metrics.insert(
        "unspent_after".to_string(),
        serde_json::json!(mode1_unspent_count(client).await.ok()),
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
    let individual_recipients = selected
        .into_iter()
        .map(|recipient| vec![recipient])
        .collect::<Vec<_>>();
    let mut total = run_mode1_recipient_batches_sequential(
        "old_wallet/S5 individual",
        client,
        ScenarioName::S5,
        individual_recipients,
        false,
        amount,
        fee_rate,
        &config.network.base_node_http_url,
    )
    .await;
    wait_for_mode1_summary_verification(
        client,
        &config.network.base_node_http_url,
        &mut total,
        ScenarioName::S5,
        start.elapsed().as_millis(),
        config.timeout(config.timeouts.confirmation_secs),
        config.benchmark.c_min,
    )
    .await;
    total.wall_ms = start.elapsed().as_millis();
    total.batch_summaries.push(Mode1BatchSummary {
        configured_batch: 1,
        attempted_batches: total.attempted_batches,
        attempted_payments: total.attempted_payments,
        success_count: total.success_count,
        failure_count: total.failure_count,
        wall_ms: total.wall_ms,
        fee_microtari: total.fee_microtari,
        tx_ids: total.tx_ids.clone(),
    });
    total.extra_metrics.insert(
        "s5_protocol".to_string(),
        serde_json::json!({
            "recipient_count": s5_items,
            "batch_size": 1,
            "complete": mode1_send_complete(&total),
            "unavailable_reason": if mode1_send_complete(&total) { None } else { Some("one or more individual transactions did not reach C_min before timeout") }
        }),
    );
    total
}

async fn run_mode1_s4_batches(
    config: &Config,
    client: &mut WalletGrpcClient<tonic::transport::Channel>,
    recipients: &[String],
    amount: MicroMinotari,
    fee_rate: u64,
) -> Mode1TransferSummary {
    let mut total = Mode1TransferSummary::default();
    let start = Instant::now();
    for configured_batch in &config.benchmark.concurrent_batches {
        let attempts = capped_attempts(*configured_batch, config.benchmark.mode1_live_max_s4_batch);
        let selected = recipients.iter().take(attempts as usize).cloned().collect();
        let batch = run_mode1_transfers_concurrent(
            &format!("old_wallet/S4 batch {configured_batch}"),
            client,
            ScenarioName::S4,
            selected,
            amount,
            fee_rate,
            config.timeout(config.benchmark.s4_t_budget_secs),
            config.benchmark.c_min,
            &config.network.base_node_http_url,
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
    recipients: Vec<String>,
    amount: MicroMinotari,
    fee_rate: u64,
    budget: Duration,
    required_depth: u64,
    base_node_url: &str,
) -> Mode1TransferSummary {
    let batch_count = u32::try_from(recipients.len()).unwrap_or(u32::MAX);
    let mut summary = Mode1TransferSummary {
        attempted_batches: batch_count,
        attempted_payments: batch_count,
        tip_start_height: base_node_tip_height(base_node_url).await.ok(),
        ..Mode1TransferSummary::default()
    };
    let start = Instant::now();
    let deadline = time::Instant::now() + budget;
    let mut join_set = JoinSet::new();
    let mut pending = BTreeMap::new();
    for (index, recipient) in recipients.into_iter().enumerate() {
        let batch_index = u32::try_from(index + 1).unwrap_or(u32::MAX);
        println!("{label} batch {batch_index}/{batch_count} dispatching");
        let mut client = client.clone();
        let recipient_for_metrics = recipient.clone();
        let submit_offset_ms = start.elapsed().as_millis();
        pending.insert(batch_index, (submit_offset_ms, recipient.clone()));
        let arm_start = start;
        join_set.spawn(async move {
            let mut transfer = submit_mode1_transfer(
                &mut client,
                scenario,
                batch_index,
                1,
                false,
                &recipient,
                amount,
                fee_rate,
            )
            .await;
            if let Ok(outcome) = &mut transfer {
                for timing in &mut outcome.tx_timings {
                    if let Some(map) = timing.as_object_mut() {
                        map.insert(
                            "recipient".to_string(),
                            serde_json::json!(&recipient_for_metrics),
                        );
                    }
                }
            }
            (
                batch_index,
                submit_offset_ms,
                arm_start.elapsed().as_millis(),
                recipient_for_metrics,
                transfer,
            )
        });
    }
    while !join_set.is_empty() {
        let Ok(Some(result)) = time::timeout_at(deadline, join_set.join_next()).await else {
            join_set.abort_all();
            break;
        };
        match result {
            Ok((batch_index, submit_offset_ms, completed_ms, recipient, transfer)) => {
                pending.remove(&batch_index);
                summary.construction_complete_ms.push(completed_ms);
                summary.record_batch(
                    batch_index,
                    1,
                    submit_offset_ms,
                    completed_ms,
                    vec![recipient],
                    transfer,
                )
            }
            Err(error) => summary.errors.push(format!("task join error: {error}")),
        }
    }
    let timed_out_at = start.elapsed().as_millis();
    for (batch_index, (submit_offset_ms, recipient)) in pending {
        summary.record_batch(
            batch_index,
            1,
            submit_offset_ms,
            timed_out_at,
            vec![recipient],
            Err(anyhow::anyhow!(
                "{label} absolute deadline expired before dispatch task completed"
            )),
        );
    }
    let remaining = deadline.saturating_duration_since(time::Instant::now());
    if !remaining.is_zero() && !summary.tx_ids.is_empty() {
        let mut verifier = client.clone();
        wait_for_mode1_summary_verification(
            &mut verifier,
            base_node_url,
            &mut summary,
            ScenarioName::S4,
            start.elapsed().as_millis(),
            remaining,
            required_depth,
        )
        .await;
    }
    if !summary.tx_ids.is_empty() && !mode1_summary_verification_complete(&summary) {
        summary.errors.push(format!(
            "{label} reached its absolute deadline before every submitted transaction was C_min-deep"
        ));
    }
    summary.wall_ms = start.elapsed().as_millis();
    summary
}

#[allow(clippy::too_many_arguments)]
async fn run_mode1_recipient_batches_sequential(
    label: &str,
    client: &mut WalletGrpcClient<tonic::transport::Channel>,
    scenario: ScenarioName,
    recipient_batches: Vec<Vec<String>>,
    single_tx: bool,
    amount: MicroMinotari,
    fee_rate: u64,
    base_node_url: &str,
) -> Mode1TransferSummary {
    let mut summary = Mode1TransferSummary {
        attempted_batches: recipient_batches.len().try_into().unwrap_or(u32::MAX),
        attempted_payments: recipient_batches
            .iter()
            .map(|batch| u32::try_from(batch.len()).unwrap_or(u32::MAX))
            .fold(0u32, u32::saturating_add),
        tip_start_height: base_node_tip_height(base_node_url).await.ok(),
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
        let submit_offset_ms = start.elapsed().as_millis();
        let recipient_identities = recipients.clone();
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
        let completed_offset_ms = start.elapsed().as_millis();
        summary.construction_complete_ms.push(completed_offset_ms);
        summary.record_batch(
            batch_index,
            items_per_batch,
            submit_offset_ms,
            completed_offset_ms,
            recipient_identities,
            result,
        );
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
    let submit_start = Instant::now();
    let response = client
        .transfer(grpc::TransferRequest {
            recipients,
            single_tx,
        })
        .await;
    let elapsed = submit_start.elapsed().as_millis();
    match response {
        Ok(response) => Ok(Mode1TransferOutcome::from_response(response.into_inner())
            .with_rpc_timing(batch_index, elapsed)),
        Err(status) => {
            let Some(tx_id) = mode1_not_found_tx_id(&status) else {
                return Err(status.into());
            };
            let error = status.to_string();
            Ok(Mode1TransferOutcome {
                success_count: 0,
                failure_count: 1,
                fee_microtari: 0,
                tx_ids: vec![tx_id.to_string()],
                errors: vec![error.clone()],
                tx_timings: vec![serde_json::json!({
                    "batch_index": batch_index,
                    "tx_id": tx_id.to_string(),
                    "construction_complete_ms": elapsed,
                    "grpc_round_trip_ms": elapsed,
                    "construction_timing_reason": "console-wallet gRPC does not expose internal construction completion",
                    "submission_timing_origin": "console_wallet_grpc_round_trip",
                    "api_accepted": false,
                    "api_error": error,
                    "grpc_code": "NotFound",
                    "chain_reconciliation_candidate": true,
                    "broadcast_to_mempool_ms": null,
                    "broadcast_to_mempool_unavailable_reason": "console_wallet_grpc_transfer_response_does_not_expose_mempool_timestamp"
                })],
            })
        }
    }
}

fn mode1_not_found_tx_id(status: &tonic::Status) -> Option<u64> {
    if status.code() != tonic::Code::NotFound {
        return None;
    }
    let value = status
        .message()
        .strip_prefix("Transaction ")?
        .strip_suffix(" not found within timeout")?;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

async fn submit_mode1_coin_split(
    client: &mut WalletGrpcClient<tonic::transport::Channel>,
    amount_per_split: u64,
    split_count: u32,
    fee_rate: u64,
) -> anyhow::Result<Mode1TransferOutcome> {
    let submit_start = Instant::now();
    let response = client
        .coin_split(grpc::CoinSplitRequest {
            amount_per_split,
            split_count: u64::from(split_count),
            fee_per_gram: fee_rate,
            lock_height: 0,
            payment_id: format!("wallet-bench-S1-{split_count}-{amount_per_split}").into_bytes(),
        })
        .await?
        .into_inner();
    Ok(Mode1TransferOutcome {
        success_count: 1,
        failure_count: 0,
        fee_microtari: 0,
        tx_ids: vec![response.tx_id.to_string()],
        errors: Vec::new(),
        tx_timings: Vec::new(),
    }
    .with_rpc_timing(1, submit_start.elapsed().as_millis()))
}

pub(super) async fn mode1_unspent_count(
    client: &mut WalletGrpcClient<tonic::transport::Channel>,
) -> anyhow::Result<u64> {
    let response = client
        .get_unspent_amounts(grpc::Empty {})
        .await?
        .into_inner();
    Ok(response.amount.len().try_into().unwrap_or(u64::MAX))
}

async fn mode1_unspent_amounts(
    client: &mut WalletGrpcClient<tonic::transport::Channel>,
) -> anyhow::Result<Vec<u64>> {
    Ok(client
        .get_unspent_amounts(grpc::Empty {})
        .await?
        .into_inner()
        .amount)
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

async fn mode1_balance_components(
    client: &mut WalletGrpcClient<tonic::transport::Channel>,
) -> anyhow::Result<serde_json::Value> {
    let response = client
        .get_balance(grpc::GetBalanceRequest { payment_id: None })
        .await?
        .into_inner();
    Ok(serde_json::json!({
        "available": response.available_balance,
        "pending_incoming": response.pending_incoming_balance,
        "pending_outgoing": response.pending_outgoing_balance,
        "timelocked": response.timelocked_balance
    }))
}

/// Pinned transaction weights: one kernel (10), one input (8), and 53 per output.
/// A stealth output with default features/script/covenant and an empty memo rounds to
/// four feature/script grams, for 57 grams total per output.
pub(super) const STEALTH_OUTPUT_GRAMS: u64 = 57;

/// Console-wallet `send_one_sided_multi_recipient_transaction` adds the sender address
/// to every memo. The pinned MemoField is padded to 130 bytes, making each output's
/// rounded feature/script contribution 12 grams and its total weight 65 grams.
pub(super) const CONSOLE_SELF_OUTPUT_GRAMS: u64 = 65;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Mode1CoinSplitPlan {
    fee_microtari: u64,
    amount_per_split: u64,
    split_count: u32,
}

/// The console wallet's native coin split adds an ordinary change output. Request
/// `desired_outputs - 1` explicit splits so the on-chain total is exactly the S1
/// target. A near-even amount keeps every child usable by the next round.
fn mode1_coin_split_plan(
    input_microtari: u64,
    desired_outputs: u32,
    fee_per_gram: u64,
) -> anyhow::Result<Mode1CoinSplitPlan> {
    if desired_outputs < 2 {
        bail!("Mode 1 coin split requires at least two total outputs");
    }
    let weight = 18u64
        .checked_add(
            CONSOLE_SELF_OUTPUT_GRAMS
                .checked_mul(u64::from(desired_outputs))
                .context("Mode 1 coin-split output weight overflow")?,
        )
        .context("Mode 1 coin-split weight overflow")?;
    let fee_microtari = weight
        .checked_mul(fee_per_gram)
        .context("Mode 1 coin-split fee overflow")?;
    let available = input_microtari
        .checked_sub(fee_microtari)
        .context("Mode 1 coin-split input does not cover fee")?;
    let amount_per_split = available / u64::from(desired_outputs);
    if amount_per_split <= fee_microtari {
        bail!("Mode 1 coin-split child is too small for the next round");
    }
    Ok(Mode1CoinSplitPlan {
        fee_microtari,
        amount_per_split,
        split_count: desired_outputs - 1,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExactSplitPlan {
    pub(super) input_microtari: u64,
    pub(super) fee_microtari: u64,
    pub(super) child_amounts: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PpSplitPlan {
    pub(super) input_microtari: u64,
    pub(super) fee_microtari: u64,
    /// PP creates these explicit self-payment outputs. Its builder creates the
    /// final balanced child as the ordinary change output.
    pub(super) payment_amounts: Vec<u64>,
    pub(super) change_microtari: u64,
}

impl ExactSplitPlan {
    pub(super) fn total_children(&self) -> u64 {
        self.child_amounts.iter().copied().sum()
    }
}

pub(super) fn exact_no_change_split_with_fee(
    input_microtari: u64,
    child_count: u32,
    fee_microtari: u64,
) -> anyhow::Result<ExactSplitPlan> {
    if child_count < 2 {
        bail!("exact split requires at least two child outputs");
    }
    let child_count_u64 = u64::from(child_count);
    let available = input_microtari
        .checked_sub(fee_microtari)
        .context("exact split input does not cover fee")?;
    let base_child = available / child_count_u64;
    if base_child == 0 {
        bail!("exact split would create a zero-value child output");
    }
    let remainder = available % child_count_u64;
    let mut child_amounts = vec![base_child; child_count as usize];
    let last = child_amounts
        .last_mut()
        .context("exact split unexpectedly has no child outputs")?;
    *last = last
        .checked_add(remainder)
        .context("exact split final child overflow")?;
    let plan = ExactSplitPlan {
        input_microtari,
        fee_microtari,
        child_amounts,
    };
    if plan.total_children().checked_add(plan.fee_microtari) != Some(plan.input_microtari) {
        bail!("exact split conservation invariant failed");
    }
    Ok(plan)
}

pub(super) fn exact_pp_split_with_change(
    input_microtari: u64,
    child_count: u32,
    fee_per_gram: u64,
) -> anyhow::Result<PpSplitPlan> {
    const PP_LOCK_FEE_BUFFER: u64 = 200_000;
    if child_count < 2 {
        bail!("PP exact split requires at least two child outputs");
    }
    let payment_count = u64::from(child_count - 1);
    // Pinned PP f0572c9 unsigned_tx_creator: one kernel/input, explicit empty-memo
    // stealth outputs (57g each), and one padded change output (65g).
    let weight = 18u64
        .checked_add(
            STEALTH_OUTPUT_GRAMS
                .checked_mul(payment_count)
                .context("PP recipient weight overflow")?,
        )
        .and_then(|weight| weight.checked_add(CONSOLE_SELF_OUTPUT_GRAMS))
        .context("PP exact split weight overflow")?;
    let fee_microtari = weight
        .checked_mul(fee_per_gram)
        .context("PP exact split fee overflow")?;
    let available = input_microtari
        .checked_sub(fee_microtari)
        .context("PP exact split input does not cover fee")?;
    let base_child = available / u64::from(child_count);
    if base_child <= fee_microtari {
        bail!("PP exact split child is too small for a later split fee");
    }
    let payment_amounts = vec![base_child; payment_count as usize];
    let explicit_total = base_child
        .checked_mul(payment_count)
        .context("PP payment total overflow")?;
    if explicit_total
        .checked_add(PP_LOCK_FEE_BUFFER)
        .is_none_or(|required| required > input_microtari)
    {
        bail!("PP exact split cannot satisfy the pinned 200000 µT lock buffer");
    }
    let change_microtari = input_microtari
        .checked_sub(explicit_total)
        .and_then(|remaining| remaining.checked_sub(fee_microtari))
        .context("PP exact split change underflow")?;
    Ok(PpSplitPlan {
        input_microtari,
        fee_microtari,
        payment_amounts,
        change_microtari,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_coin_split_requests_one_fewer_split_than_total_outputs() {
        let doubling = mode1_coin_split_plan(10_000_000_000, 2, 5).unwrap();
        assert_eq!(doubling.split_count, 1);
        assert_eq!(doubling.fee_microtari, 740);
        assert!(doubling.amount_per_split > doubling.fee_microtari);

        let fanout = mode1_coin_split_plan(1_000_000_000, 8, 5).unwrap();
        assert_eq!(fanout.split_count, 7);
        assert_eq!(fanout.fee_microtari, 2_690);
        assert!(fanout.amount_per_split > fanout.fee_microtari);
    }

    #[test]
    fn native_coin_split_rejects_non_splitting_or_dust_inputs() {
        assert!(mode1_coin_split_plan(10_000, 1, 5).is_err());
        assert!(mode1_coin_split_plan(1_000, 8, 5).is_err());
    }

    #[test]
    fn exact_split_conserves_value_without_change_for_all_s1_targets() {
        let mut amounts = vec![10_000_000_000u64];
        for target in [2usize, 4, 8, 16, 32, 64] {
            amounts = amounts
                .into_iter()
                .flat_map(|input| {
                    let fee = (18 + CONSOLE_SELF_OUTPUT_GRAMS * 2) * 5;
                    exact_no_change_split_with_fee(input, 2, fee)
                        .unwrap()
                        .child_amounts
                })
                .collect();
            assert_eq!(amounts.len(), target);
        }
        amounts = amounts
            .into_iter()
            .flat_map(|input| {
                let fee = (18 + CONSOLE_SELF_OUTPUT_GRAMS * 8) * 5;
                exact_no_change_split_with_fee(input, 8, fee)
                    .unwrap()
                    .child_amounts
            })
            .collect();
        assert_eq!(amounts.len(), 512);
    }

    #[test]
    fn parses_only_pinned_mode1_not_found_status() {
        let status =
            tonic::Status::not_found("Transaction 18446744073709551615 not found within timeout");
        assert_eq!(mode1_not_found_tx_id(&status), Some(u64::MAX));
        assert_eq!(
            mode1_not_found_tx_id(&tonic::Status::internal(
                "Transaction 42 not found within timeout"
            )),
            None
        );
        assert_eq!(
            mode1_not_found_tx_id(&tonic::Status::not_found(
                "Transaction nope not found within timeout"
            )),
            None
        );
        assert_eq!(
            mode1_not_found_tx_id(&tonic::Status::not_found("unrelated 42")),
            None
        );
    }

    #[test]
    fn runtime_config_pins_primary_and_fallback_http_endpoints() {
        let mut config = Config::default();
        config.network.base_node_http_url = "http://127.0.0.1:18142".to_string();
        config.network.mode1_base_node_service_peer = Some("/ip4/127.0.0.1/tcp/18189".to_string());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wallet.toml");

        write_mode1_runtime_config(&config, &path).unwrap();
        let contents = std::fs::read_to_string(path).unwrap();
        assert!(contents.contains("http_server_url = \"http://127.0.0.1:18142\""));
        assert!(contents.contains("fallback_http_server_url = \"http://127.0.0.1:18142\""));
        assert!(!contents.contains("rpc.esmeralda.tari.com"));
    }

    #[test]
    fn final_child_absorbs_division_remainder() {
        let plan = exact_no_change_split_with_fee(2_499_999_505, 2, 740).unwrap();
        assert_eq!(plan.child_amounts[1], plan.child_amounts[0] + 1);
        assert_eq!(
            plan.total_children() + plan.fee_microtari,
            plan.input_microtari
        );
    }

    #[test]
    fn pp_split_uses_balanced_change_to_reach_all_s1_targets() {
        let mut amounts = vec![10_000_000_000u64];
        for target in [2usize, 4, 8, 16, 32, 64] {
            amounts = amounts
                .into_iter()
                .flat_map(|input| {
                    let plan = exact_pp_split_with_change(input, 2, 5).unwrap();
                    plan.payment_amounts
                        .into_iter()
                        .chain([plan.change_microtari])
                })
                .collect();
            assert_eq!(amounts.len(), target);
        }
        amounts = amounts
            .into_iter()
            .flat_map(|input| {
                let plan = exact_pp_split_with_change(input, 8, 5).unwrap();
                plan.payment_amounts
                    .into_iter()
                    .chain([plan.change_microtari])
            })
            .collect();
        assert_eq!(amounts.len(), 512);
    }
}
