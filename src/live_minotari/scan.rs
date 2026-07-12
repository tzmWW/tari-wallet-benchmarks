use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use sysinfo::{Pid, ProcessesToUpdate, System};
use tokio::time;

use super::*;

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct ResourcePeaks {
    pub(super) peak_rss_bytes: Option<u64>,
    pub(super) peak_cpu_percent: Option<f32>,
}

pub(super) async fn with_resource_sampling<F, T>(pid: Option<u32>, future: F) -> (T, ResourcePeaks)
where
    F: Future<Output = T>,
{
    let Some(pid) = pid else {
        return (future.await, ResourcePeaks::default());
    };
    let running = Arc::new(AtomicBool::new(true));
    let sampler = tokio::spawn(sample_process_resources(pid, running.clone()));
    let output = future.await;
    running.store(false, Ordering::Relaxed);
    let peaks = sampler.await.unwrap_or_default();
    (output, peaks)
}

async fn sample_process_resources(pid: u32, running: Arc<AtomicBool>) -> ResourcePeaks {
    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    let mut peaks = ResourcePeaks::default();
    let mut interval = time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;
        system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        if let Some(process) = system.process(pid) {
            peaks.peak_rss_bytes = Some(
                peaks
                    .peak_rss_bytes
                    .unwrap_or_default()
                    .max(process.memory()),
            );
            peaks.peak_cpu_percent = Some(
                peaks
                    .peak_cpu_percent
                    .unwrap_or_default()
                    .max(process.cpu_usage()),
            );
        }
        if !running.load(Ordering::Relaxed) {
            break;
        }
    }
    peaks
}

pub(super) async fn run_library_checkpoint_scan_cells(
    config: &Config,
    profile: &mut ResultProfile,
    mode: &str,
    funded_seed_words: Option<&str>,
    scenarios: &[ScenarioName],
    checkpoint: ScanCheckpoint,
) -> anyhow::Result<()> {
    let birthday = match mode {
        "new_wallet" => config.funding.new_wallet.as_ref(),
        "payment_processor" => config.funding.payment_processor.as_ref(),
        _ => None,
    }
    .and_then(|funding| funding.birthday)
    .with_context(|| format!("funding birthday missing for {mode}"))?;
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

pub(super) async fn run_b0_fresh_scan_for_mode(
    config: &Config,
    profile: &mut ResultProfile,
    book: &AddressBook,
    mode: &str,
) -> anyhow::Result<()> {
    let spec = FreshScanSpec {
        scenario: ScenarioName::B0,
        wallet_state: FreshScanWalletState::EmptyGenesis,
        checkpoint: ScanCheckpoint::Empty,
    };
    match mode {
        "old_wallet" => {
            if let Some(seed) = book.addresses.get(WalletRole::OldWallet.label()) {
                run_mode1_fresh_scan_for_cell(config, profile, seed, spec).await?;
            }
        }
        "new_wallet" => {
            let seed = book
                .addresses
                .get(WalletRole::NewWallet.label())
                .map(|seed| seed.seed_words.as_str());
            run_library_fresh_scan_for_cell(config, profile, mode, seed, spec).await?;
        }
        "payment_processor" => {
            let seed = book
                .addresses
                .get(WalletRole::PaymentProcessor.label())
                .map(|seed| seed.seed_words.as_str());
            run_library_fresh_scan_for_cell(config, profile, mode, seed, spec).await?;
        }
        _ => bail!("unsupported B0 mode {mode}"),
    }
    Ok(())
}

pub(super) async fn run_library_fresh_scan_for_cell(
    config: &Config,
    profile: &mut ResultProfile,
    mode: &str,
    funded_seed_words: Option<&str>,
    spec: FreshScanSpec,
) -> anyhow::Result<()> {
    let expectations = scan_expectations_from_profile(profile, mode, spec, config);
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
    run_library_fresh_scan_cell(config, mode, funded_seed_words, spec, expectations, cell).await
}

pub(super) async fn run_mode1_checkpoint_scan_cells(
    config: &Config,
    profile: &mut ResultProfile,
    old_seed: &crate::seeds::SeedMaterial,
    scenarios: &[ScenarioName],
    checkpoint: ScanCheckpoint,
) -> anyhow::Result<()> {
    let birthday = config
        .funding
        .old_wallet
        .as_ref()
        .and_then(|funding| funding.birthday)
        .context("funding.old_wallet.birthday missing")?;
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

pub(super) async fn run_mode1_fresh_scan_for_cell(
    config: &Config,
    profile: &mut ResultProfile,
    old_seed: &crate::seeds::SeedMaterial,
    spec: FreshScanSpec,
) -> anyhow::Result<()> {
    let expectations = scan_expectations_from_profile(profile, "old_wallet", spec, config);
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
    run_mode1_fresh_scan_cell(config, old_seed, spec, expectations, cell).await
}

pub(super) fn record_blocked_checkpoint_scan(cell: &mut ScenarioCell, spec: FreshScanSpec) {
    let note = spec.checkpoint.blocked_note(spec.scenario);
    cell.notes.push(note.clone());
    cell.record_repetition(Repetition {
        run: 1,
        status: CellStatus::Failed,
        wall_ms: Some(0),
        success_count: 0,
        failure_count: 1,
        fee_microtari: Some(0),
        error: Some(note),
        metrics: Some(serde_json::json!({
            "blocked_prerequisite": true,
            "scan_checkpoint": spec.checkpoint.label(),
            "balance_reconciliation_unavailable_reason": "scenario did not execute because its checkpoint prerequisite failed"
        })),
    });
}

pub(super) fn record_blocked_prerequisite_cells(
    profile: &mut ResultProfile,
    mode: &str,
    scenarios: &[ScenarioName],
    prerequisite: &str,
) {
    for scenario in scenarios {
        let Some(cell) = profile
            .modes
            .get_mut(mode)
            .and_then(|mode_profile| mode_profile.scenarios.get_mut(scenario.as_str()))
        else {
            continue;
        };
        record_blocked_prerequisite_cell(cell, *scenario, prerequisite);
    }
}

pub(super) fn record_blocked_prerequisite_cell(
    cell: &mut ScenarioCell,
    scenario: ScenarioName,
    prerequisite: &str,
) {
    let note = format!(
        "{} not run: prerequisite {prerequisite} did not complete",
        scenario.as_str()
    );
    cell.notes.push(note.clone());
    cell.record_repetition(Repetition {
        run: 1,
        status: CellStatus::Failed,
        wall_ms: Some(0),
        success_count: 0,
        failure_count: 1,
        fee_microtari: Some(0),
        error: Some(note),
        metrics: Some(serde_json::json!({
            "blocked_prerequisite": true,
            "prerequisite": prerequisite,
            "blocked_scenario": scenario.as_str(),
            "balance_reconciliation_unavailable_reason": "scenario did not execute because its prerequisite failed"
        })),
    });
}

async fn run_library_fresh_scan_cell(
    config: &Config,
    mode: &str,
    funded_seed_words: Option<&str>,
    spec: FreshScanSpec,
    expectations: ScanExpectations,
    cell: &mut ScenarioCell,
) -> anyhow::Result<()> {
    for run in 1..=config.benchmark.scan_repetitions {
        let run_start = Instant::now();
        println!(
            "live scan {mode}/{} run {run}/{} birthday={} starting",
            spec.scenario.as_str(),
            config.benchmark.scan_repetitions,
            spec.birthday()
        );
        let scan =
            run_library_fresh_scan(config, mode, spec, run, funded_seed_words, expectations).await;
        match scan {
            Ok(measurement) => {
                println!(
                    "live scan {mode}/{} run {run} ok: wall_ms={} max_height={} available_microtari={}",
                    spec.scenario.as_str(),
                    measurement.wall_ms,
                    measurement.max_height,
                    measurement.available_microtari
                );
                let verification_ok = measurement.scan_verification_ok();
                cell.record_repetition(Repetition {
                    run,
                    status: if verification_ok {
                        CellStatus::Ok
                    } else {
                        CellStatus::Failed
                    },
                    wall_ms: Some(measurement.wall_ms),
                    success_count: if verification_ok { 1 } else { 0 },
                    failure_count: if verification_ok { 0 } else { 1 },
                    fee_microtari: Some(0),
                    error: (!verification_ok).then(|| measurement.scan_verification_error()),
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
                    wall_ms: Some(run_start.elapsed().as_millis()),
                    success_count: 0,
                    failure_count: 1,
                    fee_microtari: Some(0),
                    error: Some(format!("{error:#}")),
                    metrics: Some(unavailable_balance_metrics(
                        spec.scenario,
                        "scan failed before final wallet balance could be observed",
                    )),
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
    expectations: ScanExpectations,
    cell: &mut ScenarioCell,
) -> anyhow::Result<()> {
    for run in 1..=config.benchmark.scan_repetitions {
        let run_start = Instant::now();
        println!(
            "live scan old_wallet/{} run {run}/{} birthday={} starting",
            spec.scenario.as_str(),
            config.benchmark.scan_repetitions,
            spec.birthday()
        );
        let scan = run_mode1_fresh_scan(config, old_seed, spec, run, expectations).await;
        match scan {
            Ok(measurement) => {
                println!(
                    "live scan old_wallet/{} run {run} ok: wall_ms={} max_height={} available_microtari={}",
                    spec.scenario.as_str(),
                    measurement.wall_ms,
                    measurement.max_height,
                    measurement.available_microtari
                );
                let verification_ok = measurement.scan_verification_ok();
                cell.record_repetition(Repetition {
                    run,
                    status: if verification_ok {
                        CellStatus::Ok
                    } else {
                        CellStatus::Failed
                    },
                    wall_ms: Some(measurement.wall_ms),
                    success_count: if verification_ok { 1 } else { 0 },
                    failure_count: if verification_ok { 0 } else { 1 },
                    fee_microtari: Some(0),
                    error: (!verification_ok).then(|| measurement.scan_verification_error()),
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
                    wall_ms: Some(run_start.elapsed().as_millis()),
                    success_count: 0,
                    failure_count: 1,
                    fee_microtari: Some(0),
                    error: Some(format!("{error:#}")),
                    metrics: Some(unavailable_balance_metrics(
                        spec.scenario,
                        "console-wallet recovery failed before final balance could be observed",
                    )),
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
    expectations: ScanExpectations,
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
    let tip_tolerance = 0;
    let (scan_result, resource_peaks) = with_resource_sampling(
        Some(std::process::id()),
        scan_to_tip(
            &db_path,
            &password,
            &config.network.base_node_http_url,
            config.benchmark.scan_batch_size,
            tip_tolerance,
            config.timeout(config.timeouts.scan_batch_secs),
        ),
    )
    .await;
    let scan_report = scan_result?;
    let tip_end = Some(scan_report.target_tip);
    let account = account_snapshot(&db_path)?;
    let detected_outputs = detected_output_count(&db_path).unwrap_or_default();
    let history_transactions = history_transaction_count(&db_path).unwrap_or_default();
    let spendable_outputs = spendable_output_count(&db_path).unwrap_or_default();

    Ok(ScanMeasurement {
        wall_ms: scan_report.wall_ms,
        birthday: spec.birthday(),
        birthday_start_height: resolved_birthday_start_height(config, mode, spec),
        max_height: account.max_height,
        available_microtari: account.available_microtari,
        tip_start,
        tip_end,
        detected_outputs,
        history_transactions,
        spendable_outputs,
        resource_peaks,
        expectations,
        tip_lag_tolerance_blocks: tip_tolerance,
        scan_no_progress_attempts: scan_report.no_progress_attempts,
        scan_stopped_without_progress: scan_report.stopped_without_progress,
        scan_last_more_blocks: scan_report.last_more_blocks,
    })
}

async fn run_mode1_fresh_scan(
    config: &Config,
    old_seed: &crate::seeds::SeedMaterial,
    spec: FreshScanSpec,
    run: u32,
    expectations: ScanExpectations,
) -> anyhow::Result<ScanMeasurement> {
    let base_path = fresh_console_base_path(config, spec, run);
    reset_dir(&base_path)?;
    let grpc_address = mode1_scan_grpc_address(&config.modes.old_wallet_grpc_address, spec, run)?;
    let grpc_bind = grpc_bind_multiaddr(&grpc_address)?;
    let password = wallet_password(&config.seeds.wallet_password_env)?;
    let seed_words = match spec.wallet_state {
        FreshScanWalletState::EmptyGenesis => seed_words_with_birthday(&old_seed.seed_words, 0)?,
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
    let scan_pid = process.child.id();
    let mut client = wait_for_mode1_grpc_address(config, &mut process, &grpc_address).await?;
    let (scan_result, resource_peaks) = with_resource_sampling(
        scan_pid,
        wait_for_mode1_scan_to_tip(
            &mut process,
            &mut client,
            tip_start,
            Some(&config.network.base_node_http_url),
            config.timeout(config.timeouts.startup_secs),
            config.timeout(config.timeouts.scan_batch_secs),
        ),
    )
    .await;
    let max_height = scan_result?;
    let balance = client
        .get_balance(grpc::GetBalanceRequest { payment_id: None })
        .await?
        .into_inner();
    let detected_outputs = mode1_unspent_count(&mut client).await.unwrap_or_default();
    let history_transactions =
        history_transaction_count(&base_path.join("esmeralda/data/wallet/db/console_wallet.db"))
            .unwrap_or_default();
    let spendable_outputs = detected_outputs;
    let tip_end = Some(max_height);

    Ok(ScanMeasurement {
        wall_ms: start.elapsed().as_millis(),
        birthday: spec.birthday(),
        birthday_start_height: resolved_birthday_start_height(config, "old_wallet", spec),
        max_height,
        available_microtari: balance.available_balance,
        tip_start,
        tip_end,
        detected_outputs,
        history_transactions,
        spendable_outputs,
        resource_peaks,
        expectations,
        tip_lag_tolerance_blocks: 0,
        scan_no_progress_attempts: 0,
        scan_stopped_without_progress: false,
        scan_last_more_blocks: None,
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

fn history_transaction_count(db_path: &Path) -> anyhow::Result<u64> {
    let conn = Connection::open(db_path)?;
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM completed_transactions", [], |row| {
        row.get(0)
    })?;
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

pub(super) fn account_snapshot(db_path: &Path) -> anyhow::Result<AccountSnapshot> {
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

pub(super) fn amount_field_as_microtari(balance: &serde_json::Value, key: &str) -> Option<u64> {
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

pub(super) fn spendable_output_count(db_path: &Path) -> anyhow::Result<u64> {
    let conn = Connection::open(db_path)?;
    let active = active_output_predicate(&conn)?;
    let sql = format!(
        "SELECT COUNT(*) FROM outputs WHERE {active} confirmed_height IS NOT NULL AND CAST(status AS TEXT) IN ('UNSPENT', '0')"
    );
    let count: i64 = conn.query_row(&sql, [], |row| row.get(0))?;
    Ok(u64::try_from(count).unwrap_or_default())
}

pub(super) fn spendable_output_amounts(db_path: &Path) -> anyhow::Result<Vec<u64>> {
    let conn = Connection::open(db_path)?;
    let active = active_output_predicate(&conn)?;
    let sql = format!(
        "SELECT value FROM outputs WHERE {active} confirmed_height IS NOT NULL AND CAST(status AS TEXT) IN ('UNSPENT', '0') ORDER BY value DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    stmt.query_map([], |row| row.get::<_, i64>(0))?
        .map(|value| {
            let value = value?;
            u64::try_from(value).context("spendable output has negative value")
        })
        .collect()
}

fn active_output_predicate(conn: &Connection) -> anyhow::Result<&'static str> {
    let mut stmt = conn.prepare("PRAGMA table_info(outputs)")?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if columns.iter().any(|column| column == "deleted_at") {
        Ok("deleted_at IS NULL AND")
    } else if columns
        .iter()
        .any(|column| column == "marked_deleted_at_height")
    {
        Ok("marked_deleted_at_height IS NULL AND")
    } else {
        Ok("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spendable_queries_exclude_unconfirmed_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("wallet.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE outputs (
                id INTEGER PRIMARY KEY,
                value INTEGER NOT NULL,
                status TEXT NOT NULL,
                confirmed_height INTEGER,
                deleted_at TIMESTAMP
            );
            INSERT INTO outputs (value, status, confirmed_height) VALUES
                (100, 'UNSPENT', 10),
                (200, 'UNSPENT', NULL),
                (300, 'SPENT', 10);",
        )
        .unwrap();
        drop(conn);

        assert_eq!(spendable_output_count(&db_path).unwrap(), 1);
        assert_eq!(spendable_output_amounts(&db_path).unwrap(), vec![100]);
    }
}
