use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use sysinfo::Disks;

use crate::{
    config::Config,
    env_capture,
    modes::ModeName,
    payment_processor,
    result_profile::{ResultProfile, empty_mode_profile},
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
    )?;
    if check_funds {
        if mode1_db.is_none() {
            verify_mode1_wallet_identity(config, &book).await?;
        } else {
            println!(
                "old_wallet: custom --mode1-db audit skips gRPC identity proof; use the configured DB for strict live-run readiness"
            );
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
    println!(
        "binary paths are executable; embedded source revisions are not observable, configured pins: minotari_cli={} console_wallet={} payment_processor={}",
        config.versions.minotari_cli_rev,
        config.versions.tari_console_wallet_rev,
        config.versions.payment_processor_rev
    );

    if check_funds {
        let paths = live_wallet_paths(config, mode1_db, mode2_db, payment_receiver_db);
        check_live_funds_at_paths(config, &paths)?;
        check_live_wallet_identities(config, book, &paths)?;
    }
    Ok(())
}

/// Runs the fail-closed, non-spending readiness gate used by `run` before a
/// result profile or any live topology is created.
pub async fn preflight_for_live_run(config: &Config, book: &AddressBook) -> anyhow::Result<()> {
    ensure_runtime_paths_are_absolute(config)?;
    check_disk_space(config)?;
    check_listen_ports_available(config)?;
    preflight_checks(config, book, true, None, None, None).context("strict live-run preflight")?;
    verify_mode1_wallet_identity(config, book).await?;
    let paths = live_wallet_paths(config, None, None, None);
    check_selected_chain_readiness(config, &paths)
        .await
        .context("selected-chain live-run preflight")
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
    fresh_data_dir: bool,
    yes: bool,
) -> anyhow::Result<()> {
    let book = AddressBook::load_required(config)?;
    preflight_for_live_run(config, &book).await?;

    if fresh_data_dir {
        reset_enabled_mode_dirs(config, yes)?;
    }

    let mut profile = ResultProfile::new(
        config,
        env_capture::capture_for_base_node(&config.network.base_node_http_url),
    );
    #[cfg(feature = "live-minotari")]
    {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        let tip = fetch_chain_tip(&client, &config.network.base_node_http_url).await?;
        profile.set_tip_start(tip.height, Some(hex::encode(&tip.hash)));
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
        profile.mark_checkpoint_stage("scaffold");
    }

    #[cfg(feature = "live-minotari")]
    {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        let tip = fetch_chain_tip(&client, &config.network.base_node_http_url).await?;
        profile.set_tip_end(tip.height, Some(hex::encode(&tip.hash)));
        profile.base_node.pruning_horizon = Some(tip.pruning_horizon);
        profile.base_node.is_synced = Some(tip.is_synced);
        profile.mark_final();
    }
    profile.refresh_computed_deltas();
    profile.write_validated_atomic(profile_path, false)?;
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
                if scanned_height.saturating_add(config.settle_wait_blocks()) >= tip.height => {}
            Ok(scanned_height) => errors.push(format!(
                "{label}: scanner height {scanned_height} is stale relative to selected-chain tip {} (allowed lag settle_wait_blocks={})",
                tip.height,
                config.settle_wait_blocks()
            )),
            Err(error) => errors.push(format!("{label}: scanner-height proof failed: {error:#}")),
        }
    }
    if !errors.is_empty() {
        bail!("selected-chain preflight failed:\n{}", errors.join("\n"));
    }
    println!(
        "selected-chain proof PASS: tip={} hash={} pruning_horizon={} is_synced=true",
        tip.height,
        hex::encode(tip.hash),
        tip.pruning_horizon
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
        (
            "paths.payment_processor_binary",
            config.paths.payment_processor_binary.as_path(),
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
        let error = run_profile(&config, &profile, false, false)
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
