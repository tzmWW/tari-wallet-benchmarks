use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use rusqlite::Connection;

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

pub async fn preflight(
    config: &Config,
    check_funds: bool,
    mode1_db: Option<PathBuf>,
    mode2_db: Option<PathBuf>,
    payment_receiver_db: Option<PathBuf>,
) -> anyhow::Result<()> {
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
    if check_funds {
        check_live_funds(config, mode1_db, mode2_db, payment_receiver_db)?;
    }
    println!("preflight PASS: config and seed material are Esmeralda-scoped");
    Ok(())
}

pub async fn run_profile(
    config: &Config,
    profile_path: &Path,
    fresh_data_dir: bool,
    yes: bool,
) -> anyhow::Result<()> {
    if fresh_data_dir {
        reset_enabled_mode_dirs(config, yes)?;
    }

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

    let mut profile = ResultProfile::new(
        config,
        env_capture::capture_for_base_node(&config.network.base_node_http_url),
    );
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
        crate::live_minotari::annotate_profile_with_library_smoke(
            config,
            &book,
            &mut profile,
            Some(profile_path),
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
    }

    profile.write_atomic(profile_path)?;
    println!("wrote {}", profile_path.display());
    Ok(())
}

fn reset_enabled_mode_dirs(config: &Config, yes: bool) -> anyhow::Result<()> {
    if !yes {
        bail!("--fresh-data-dir deletes enabled mode data dirs; pass --yes to confirm");
    }
    let mut dirs = Vec::new();
    if config.benchmark.mode1_live_topology {
        dirs.push(config.paths.data_dir.join("old-wallet-console"));
    }
    if (config.benchmark.mode2_live_scenarios || config.benchmark.mode2_send_smoke)
        && let Some(parent) = config.modes.new_wallet_database.parent()
    {
        dirs.push(parent.to_path_buf());
    }
    if config.benchmark.mode3_live_topology {
        dirs.push(config.paths.data_dir.join("payment-processor"));
        dirs.push(config.paths.data_dir.join("payment-receiver"));
        dirs.push(
            config
                .paths
                .data_dir
                .join("payment-processor-console-wallet"),
        );
    }
    for dir in dirs {
        if dir.exists() {
            println!("removing fresh data dir {}", dir.display());
            fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
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
    invalid_rows: Vec<String>,
    unknown_rows: Vec<String>,
}

fn check_live_funds(
    config: &Config,
    mode1_db: Option<PathBuf>,
    mode2_db: Option<PathBuf>,
    payment_receiver_db: Option<PathBuf>,
) -> anyhow::Result<()> {
    let checks = [
        (
            "old_wallet",
            mode1_db.unwrap_or_else(|| {
                config
                    .paths
                    .data_dir
                    .join("old-wallet-console/esmeralda/data/wallet/db/console_wallet.db")
            }),
            mode1_required_unspent_outputs(config),
        ),
        (
            "new_wallet",
            mode2_db.unwrap_or_else(|| config.modes.new_wallet_database.clone()),
            mode2_required_unspent_outputs(config),
        ),
        (
            "payment_processor",
            payment_receiver_db
                .unwrap_or_else(|| config.paths.data_dir.join("payment-receiver/wallet.db")),
            mode3_required_unspent_outputs(config),
        ),
    ];

    let mut errors = Vec::new();
    for (label, db_path, required_unspent) in checks {
        if !db_path.exists() {
            errors.push(format!(
                "{label}: wallet DB missing at {}; fund/scan this wallet before live run",
                db_path.display()
            ));
            continue;
        }
        let summary = output_status_summary(&db_path)
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
        if totals.spendable_count < required_unspent {
            errors.push(format!(
                "{label}: only {} spendable outputs, require at least {required_unspent} for configured live shape",
                totals.spendable_count
            ));
        }
        if !totals.pending_rows.is_empty() {
            errors.push(format!(
                "{label}: pending/encumbered outputs present ({}); unlock/rescan before final live run",
                totals.pending_rows.join(",")
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

fn output_status_summary(db_path: &Path) -> anyhow::Result<Vec<OutputStatusSummary>> {
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT CAST(status AS TEXT), count(*), coalesce(sum(value), 0) FROM outputs GROUP BY status ORDER BY CAST(status AS TEXT)",
    )?;
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

fn fund_status_totals(summary: &[OutputStatusSummary]) -> FundStatusTotals {
    let mut totals = FundStatusTotals::default();
    for row in summary {
        match classify_output_status(&row.status) {
            OutputStatusClass::Spendable => {
                totals.spendable_count = totals.spendable_count.saturating_add(row.count);
                totals.spendable_value = totals.spendable_value.saturating_add(row.value);
            }
            OutputStatusClass::Pending => totals.pending_rows.push(format_status_row(row)),
            OutputStatusClass::Spent => {}
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

fn mode1_required_unspent_outputs(_config: &Config) -> u64 {
    1
}

fn mode2_required_unspent_outputs(config: &Config) -> u64 {
    if !config.benchmark.mode2_live_scenarios {
        return 1;
    }
    if config.benchmark.mode2_live_max_s1_txs == 0
        && config.benchmark.mode2_live_max_s4_batch == 0
        && config.benchmark.mode2_live_max_s5_txs == 0
    {
        return u64::from(
            config
                .benchmark
                .volume_target
                .saturating_div(config.benchmark.fanout_outputs_per_tx.max(1)),
        )
        .max(1);
    }
    u64::from(
        config
            .benchmark
            .mode2_live_max_s1_txs
            .max(config.benchmark.mode2_live_max_s4_batch)
            .max(config.benchmark.mode2_live_max_s5_txs)
            .max(1),
    )
}

fn mode3_required_unspent_outputs(config: &Config) -> u64 {
    if !config.benchmark.mode3_live_topology {
        return 1;
    }
    if config.benchmark.mode3_live_max_s1_batches == 0
        && config.benchmark.mode3_live_max_s4_batch == 0
        && config.benchmark.mode3_live_max_s5_items == 0
    {
        return 150;
    }
    u64::from(
        config
            .benchmark
            .mode3_live_max_s1_batches
            .max(config.benchmark.mode3_live_max_s4_batch)
            .max(config.benchmark.mode3_live_max_s5_items)
            .max(1),
    )
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
