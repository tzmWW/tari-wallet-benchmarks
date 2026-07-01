use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context, bail};
use rusqlite::{Connection, OpenFlags, params, params_from_iter};
use serde::{Deserialize, Serialize};
use tokio::{process::Command, time};

use crate::{config::Config, seeds::SeedMaterial};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentProcessorEnv {
    pub vars: BTreeMap<String, String>,
}

pub fn build_env(config: &Config, pp_seed: &SeedMaterial) -> PaymentProcessorEnv {
    let mut vars = BTreeMap::new();
    vars.insert("TARI_NETWORK".to_string(), "Esmeralda".to_string());
    vars.insert(
        "DATABASE_URL".to_string(),
        sqlite_url(&payment_processor_db_path(config)),
    );
    vars.insert(
        "PAYMENT_RECEIVER".to_string(),
        format!("http://{}", config.modes.payment_receiver_listen),
    );
    vars.insert(
        "BASE_NODE".to_string(),
        config.network.base_node_http_url.clone(),
    );
    vars.insert(
        "CONSOLE_WALLET_PATH".to_string(),
        absolute_path(&config.paths.minotari_console_wallet)
            .display()
            .to_string(),
    );
    vars.insert(
        "CONSOLE_WALLET_BASE_PATH".to_string(),
        absolute_path(&console_wallet_base_path(config))
            .display()
            .to_string(),
    );
    vars.insert(
        "CONSOLE_WALLET_PASSWORD".to_string(),
        std::env::var(&config.seeds.wallet_password_env)
            .unwrap_or_else(|_| format!("${}", config.seeds.wallet_password_env)),
    );
    vars.insert(
        "LISTEN_IP".to_string(),
        config
            .modes
            .payment_processor_listen
            .split(':')
            .next()
            .unwrap_or("127.0.0.1")
            .to_string(),
    );
    vars.insert(
        "LISTEN_PORT".to_string(),
        config
            .modes
            .payment_processor_listen
            .rsplit(':')
            .next()
            .unwrap_or("9145")
            .to_string(),
    );
    vars.insert(
        "BATCH_CREATOR_SLEEP_SECS".to_string(),
        config.benchmark.mode3_worker_sleep_secs.to_string(),
    );
    vars.insert(
        "UNSIGNED_TX_CREATOR_SLEEP_SECS".to_string(),
        config.benchmark.mode3_worker_sleep_secs.to_string(),
    );
    vars.insert(
        "TRANSACTION_SIGNER_SLEEP_SECS".to_string(),
        config.benchmark.mode3_worker_sleep_secs.to_string(),
    );
    vars.insert(
        "BROADCASTER_SLEEP_SECS".to_string(),
        config.benchmark.mode3_worker_sleep_secs.to_string(),
    );
    vars.insert(
        "CONFIRMATION_CHECKER_SLEEP_SECS".to_string(),
        config.benchmark.mode3_worker_sleep_secs.to_string(),
    );
    vars.insert(
        "CONFIRMATION_CHECKER_REQUIRED_CONFIRMATIONS".to_string(),
        config.benchmark.c_min.to_string(),
    );
    vars.insert("ACCOUNTS__DEFAULT__NAME".to_string(), "default".to_string());
    vars.insert(
        "ACCOUNTS__DEFAULT__VIEW_KEY".to_string(),
        pp_seed.private_view_key_hex.clone(),
    );
    vars.insert(
        "ACCOUNTS__DEFAULT__PUBLIC_SPEND_KEY".to_string(),
        pp_seed.public_spend_key_hex.clone(),
    );
    PaymentProcessorEnv { vars }
}

pub fn payment_receiver_db_path(config: &Config) -> PathBuf {
    config.paths.data_dir.join("payment-receiver/wallet.db")
}

pub fn payment_processor_db_path(config: &Config) -> PathBuf {
    config.paths.data_dir.join("payment-processor/payments.db")
}

pub fn unlock_stale_payment_receiver_locks(config: &Config) -> anyhow::Result<usize> {
    let db_path = payment_receiver_db_path(config);
    if !db_path.exists() {
        return Ok(0);
    }
    let mut conn = Connection::open(&db_path)
        .with_context(|| format!("opening payment receiver database {}", db_path.display()))?;
    let locked_request_ids = {
        let mut stmt = conn.prepare(
            r#"
            SELECT DISTINCT locked_by_request_id
            FROM outputs
            WHERE status = 'LOCKED' AND locked_by_request_id IS NOT NULL
            "#,
        )?;
        stmt.query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    if locked_request_ids.is_empty() {
        return Ok(0);
    }
    let tx = conn.transaction()?;
    for request_id in &locked_request_ids {
        tx.execute(
            "UPDATE pending_transactions SET status = 'EXPIRED' WHERE id = ?1 AND status = 'PENDING'",
            params![request_id],
        )?;
        tx.execute(
            r#"
            UPDATE outputs
            SET status = 'UNSPENT', locked_at = NULL, locked_by_request_id = NULL
            WHERE locked_by_request_id = ?1 AND status = 'LOCKED'
            "#,
            params![request_id],
        )?;
    }
    tx.commit()?;
    Ok(locked_request_ids.len())
}

pub fn console_wallet_base_path(config: &Config) -> PathBuf {
    config
        .paths
        .data_dir
        .join("payment-processor-console-wallet")
}

pub async fn ensure_console_wallet_base(
    config: &Config,
    pp_seed: &SeedMaterial,
    password: &str,
) -> anyhow::Result<()> {
    let base_path = console_wallet_base_path(config);
    if console_wallet_db_path(&base_path).exists() {
        return Ok(());
    }
    fs::create_dir_all(&base_path)?;
    let output = Command::new(&config.paths.minotari_console_wallet)
        .env("MINOTARI_WALLET_SEED_WORDS", &pp_seed.seed_words)
        .env("MINOTARI_WALLET_PASSWORD", password)
        .arg("--base-path")
        .arg(&base_path)
        .arg("--network")
        .arg("Esmeralda")
        .arg("--non-interactive-mode")
        .arg("--command-mode-auto-exit")
        .arg("--skip-recovery")
        .arg("--command")
        .arg("get-balance")
        .output()
        .await
        .context("initializing payment-processor console wallet signer base path")?;

    if !output.status.success() {
        bail!(
            "console wallet signer initialization failed: status={} stderr={} stdout={}",
            output.status,
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
    }
    Ok(())
}

fn console_wallet_db_path(base_path: &Path) -> PathBuf {
    base_path
        .join("esmeralda")
        .join("data/wallet/db/console_wallet.db")
}

pub async fn start_payment_receiver(
    config: &Config,
    password: &str,
) -> anyhow::Result<ManagedProcess> {
    let db_path = payment_receiver_db_path(config);
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let api_port = listen_port(&config.modes.payment_receiver_listen, 9146);
    let mut command = Command::new(&config.paths.minotari_binary);
    command
        .arg("--network")
        .arg("esmeralda")
        .arg("daemon")
        .arg("--password")
        .arg(password)
        .arg("--base-url")
        .arg(&config.network.base_node_http_url)
        .arg("--batch-size")
        .arg(config.benchmark.scan_batch_size.to_string())
        .arg("--database-path")
        .arg(db_path)
        .arg("--scan-interval-secs")
        .arg(config.timeouts.scan_batch_secs.to_string())
        .arg("--api-port")
        .arg(api_port.to_string());
    spawn_logged_process("mode3-payment-receiver", command)
}

pub async fn start_payment_processor(
    config: &Config,
    env: &PaymentProcessorEnv,
) -> anyhow::Result<ManagedProcess> {
    let db_path = payment_processor_db_path(config);
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&db_path)
        .with_context(|| format!("creating PP database file {}", db_path.display()))?;
    let mut command = Command::new(&config.paths.payment_processor_binary);
    command.envs(&env.vars);
    spawn_logged_process("mode3-payment-processor", command)
}

fn sqlite_url(path: &Path) -> String {
    format!("sqlite://{}", absolute_path(path).display())
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn spawn_logged_process(label: &str, mut command: Command) -> anyhow::Result<ManagedProcess> {
    fs::create_dir_all("logs")?;
    let stdout_path = PathBuf::from("logs").join(format!("{label}.stdout.log"));
    let stderr_path = PathBuf::from("logs").join(format!("{label}.stderr.log"));
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stdout_path)?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stderr_path)?;
    let child = command
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .with_context(|| format!("spawning {label}"))?;
    Ok(ManagedProcess {
        label: label.to_string(),
        child,
        stdout_path,
        stderr_path,
    })
}

pub struct ManagedProcess {
    label: String,
    child: tokio::process::Child,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
}

impl ManagedProcess {
    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn try_wait(&mut self) -> anyhow::Result<Option<ExitStatus>> {
        self.child
            .try_wait()
            .with_context(|| format!("checking {} process status", self.label))
    }
}

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

pub async fn wait_for_payment_receiver(
    config: &Config,
    process: &mut ManagedProcess,
) -> anyhow::Result<serde_json::Value> {
    let base_url = format!("http://{}", config.modes.payment_receiver_listen);
    wait_for_json(
        format!("{base_url}/accounts/default/balance"),
        config.timeout(config.timeouts.startup_secs),
        process,
    )
    .await
}

pub async fn wait_for_payment_receiver_balance(
    config: &Config,
    process: &mut ManagedProcess,
    min_available: u64,
) -> anyhow::Result<serde_json::Value> {
    let url = format!(
        "http://{}/accounts/default/balance",
        config.modes.payment_receiver_listen
    );
    let timeout = config.timeout(config.timeouts.startup_secs);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let start = Instant::now();
    let mut last_report = Instant::now();
    let mut interval = time::interval(Duration::from_secs(5));
    loop {
        interval.tick().await;
        if let Some(status) = process.try_wait()? {
            bail!("{}", process_exit_message(process, status));
        }
        let balance = match client.get(&url).send().await {
            Ok(response) => match response.error_for_status() {
                Ok(response) => response.json::<serde_json::Value>().await?,
                Err(error) => {
                    if start.elapsed() > timeout {
                        bail!(
                            "payment receiver balance endpoint did not recover within {:?}: {}",
                            timeout,
                            error
                        );
                    }
                    continue;
                }
            },
            Err(error) => {
                if start.elapsed() > timeout {
                    bail!(
                        "payment receiver balance endpoint did not respond within {:?}: {}",
                        timeout,
                        error
                    );
                }
                continue;
            }
        };
        let available = balance_amount_field(&balance, "available").unwrap_or_default();
        if available >= min_available {
            return Ok(balance);
        }
        if last_report.elapsed() >= Duration::from_secs(30) {
            println!(
                "mode3 payment receiver scan wait: available={} required={} balance={}",
                available, min_available, balance
            );
            last_report = Instant::now();
        }
        if start.elapsed() > timeout {
            bail!(
                "payment receiver did not reach required available balance {} within {:?}; last_balance={}",
                min_available,
                timeout,
                balance
            );
        }
    }
}

pub async fn wait_for_payment_processor(
    config: &Config,
    process: &mut ManagedProcess,
) -> anyhow::Result<ServiceVersion> {
    let client =
        PaymentProcessorClient::new(format!("http://{}", config.modes.payment_processor_listen));
    let start = Instant::now();
    let timeout = config.timeout(config.timeouts.startup_secs);
    let mut interval = time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;
        if let Some(status) = process.try_wait()? {
            bail!("{}", process_exit_message(process, status));
        }
        let attempt_error = match client.health_version().await {
            Ok(version) => return Ok(version),
            Err(error) => error.to_string(),
        };
        if start.elapsed() > timeout {
            bail!(
                "payment processor did not become healthy within {:?}: {}",
                timeout,
                attempt_error
            );
        }
    }
}

async fn wait_for_json(
    url: String,
    timeout: Duration,
    process: &mut ManagedProcess,
) -> anyhow::Result<serde_json::Value> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let start = Instant::now();
    let mut interval = time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;
        if let Some(status) = process.try_wait()? {
            bail!("{}", process_exit_message(process, status));
        }
        let attempt_error = match client.get(&url).send().await {
            Ok(response) => match response.error_for_status() {
                Ok(response) => return Ok(response.json().await?),
                Err(error) => error.to_string(),
            },
            Err(error) => error.to_string(),
        };
        if start.elapsed() > timeout {
            bail!(
                "{url} did not return JSON within {:?}: {}",
                timeout,
                attempt_error
            );
        }
    }
}

fn process_exit_message(process: &ManagedProcess, status: ExitStatus) -> String {
    format!(
        "{} exited during startup with status {status}; stdout_log={} stderr_log={}",
        process.label(),
        process.stdout_path.display(),
        process.stderr_path.display()
    )
}

fn balance_amount_field(value: &serde_json::Value, key: &str) -> Option<u64> {
    value
        .get(key)
        .and_then(|field| field.as_u64().or_else(|| field.as_str()?.parse().ok()))
}

fn listen_port(listen: &str, default: u16) -> u16 {
    listen
        .rsplit(':')
        .next()
        .and_then(|port| port.parse().ok())
        .unwrap_or(default)
}

#[derive(Debug, Clone)]
pub struct PaymentProcessorClient {
    client: reqwest::Client,
    base_url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaymentRequest {
    pub client_id: String,
    pub account_name: String,
    pub recipient_address: String,
    pub amount: i64,
    pub payment_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BulkPaymentItem {
    pub client_id: String,
    pub recipient_address: String,
    pub amount: i64,
    pub payment_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BulkPaymentRequest {
    pub account_name: String,
    pub items: Vec<BulkPaymentItem>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServiceVersion {
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentBatchSnapshot {
    pub id: String,
    pub status: String,
    pub retry_count: i64,
    pub error_message: Option<String>,
    pub has_unsigned_tx: bool,
    pub has_signed_tx: bool,
    pub mined_height: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentSnapshot {
    pub id: String,
    pub status: String,
    pub payment_batch_id: Option<String>,
    pub failure_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentProcessorDbSnapshot {
    pub batches: Vec<PaymentBatchSnapshot>,
    pub payments: Vec<PaymentSnapshot>,
}

impl PaymentProcessorDbSnapshot {
    pub fn status_summary(&self) -> String {
        let batches = self
            .batches
            .iter()
            .map(|batch| {
                format!(
                    "batch:{} status:{} retries:{} unsigned:{} signed:{} error:{}",
                    batch.id,
                    batch.status,
                    batch.retry_count,
                    batch.has_unsigned_tx,
                    batch.has_signed_tx,
                    batch.error_message.as_deref().unwrap_or("none")
                )
            })
            .collect::<Vec<_>>()
            .join(" | ");
        let payments = self
            .payments
            .iter()
            .map(|payment| {
                format!(
                    "payment:{} status:{} batch:{} failure:{}",
                    payment.id,
                    payment.status,
                    payment.payment_batch_id.as_deref().unwrap_or("none"),
                    payment.failure_reason.as_deref().unwrap_or("none")
                )
            })
            .collect::<Vec<_>>()
            .join(" | ");
        format!("batches=[{batches}] payments=[{payments}]")
    }

    pub fn has_upstream_signing_or_broadcast_error(&self) -> bool {
        self.batches.iter().any(|batch| {
            batch.error_message.as_ref().is_some_and(|error| {
                let error = error.to_lowercase();
                error.contains("sign")
                    || error.contains("deserialize")
                    || error.contains("broadcast")
                    || error.contains("submit")
                    || error.contains("cli exited")
            })
        })
    }
}

pub fn inspect_payment_processor_db(
    config: &Config,
    batch_ids: &[String],
    payment_ids: &[String],
) -> anyhow::Result<PaymentProcessorDbSnapshot> {
    let db_path = payment_processor_db_path(config);
    let conn = Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening PP database {}", db_path.display()))?;
    conn.busy_timeout(Duration::from_millis(100))
        .with_context(|| format!("setting PP database busy timeout {}", db_path.display()))?;

    let batches = inspect_batches(&conn, batch_ids)?;
    let payments = inspect_payments(&conn, payment_ids)?;
    Ok(PaymentProcessorDbSnapshot { batches, payments })
}

fn inspect_batches(conn: &Connection, ids: &[String]) -> anyhow::Result<Vec<PaymentBatchSnapshot>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = sql_placeholders(ids.len());
    let sql = format!(
        r#"
        SELECT
            id,
            status,
            retry_count,
            error_message,
            unsigned_tx_json IS NOT NULL,
            signed_tx_json IS NOT NULL,
            mined_height
        FROM payment_batches
        WHERE id IN ({placeholders})
        "#
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(ids.iter()), |row| {
        Ok(PaymentBatchSnapshot {
            id: row.get(0)?,
            status: row.get(1)?,
            retry_count: row.get(2)?,
            error_message: row.get(3)?,
            has_unsigned_tx: row.get::<_, i64>(4)? != 0,
            has_signed_tx: row.get::<_, i64>(5)? != 0,
            mined_height: row.get(6)?,
        })
    })?;
    let mut by_id = rows
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(|batch| (batch.id.clone(), batch))
        .collect::<BTreeMap<_, _>>();
    ids.iter()
        .map(|id| {
            by_id
                .remove(id)
                .with_context(|| format!("reading PP batch {id}"))
        })
        .collect()
}

fn inspect_payments(conn: &Connection, ids: &[String]) -> anyhow::Result<Vec<PaymentSnapshot>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = sql_placeholders(ids.len());
    let sql = format!(
        r#"
        SELECT id, status, payment_batch_id, failure_reason
        FROM payments
        WHERE id IN ({placeholders})
        "#
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(ids.iter()), |row| {
        Ok(PaymentSnapshot {
            id: row.get(0)?,
            status: row.get(1)?,
            payment_batch_id: row.get(2)?,
            failure_reason: row.get(3)?,
        })
    })?;
    let mut by_id = rows
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(|payment| (payment.id.clone(), payment))
        .collect::<BTreeMap<_, _>>();
    ids.iter()
        .map(|id| {
            by_id
                .remove(id)
                .with_context(|| format!("reading PP payment {id}"))
        })
        .collect()
}

fn sql_placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(",")
}

impl PaymentProcessorClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("valid reqwest client"),
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    pub async fn health_version(&self) -> anyhow::Result<ServiceVersion> {
        Ok(self
            .client
            .get(format!("{}/health/version", self.base_url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    pub async fn create_payment(
        &self,
        request: &PaymentRequest,
    ) -> anyhow::Result<serde_json::Value> {
        Ok(self
            .client
            .post(format!("{}/v1/payments", self.base_url))
            .json(request)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    pub async fn create_payment_batch(
        &self,
        request: &BulkPaymentRequest,
    ) -> anyhow::Result<serde_json::Value> {
        Ok(self
            .client
            .post(format!("{}/v1/payment-batches", self.base_url))
            .json(request)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    pub async fn get_payment(&self, payment_id: &str) -> anyhow::Result<serde_json::Value> {
        Ok(self
            .client
            .get(format!("{}/v1/payments/{}", self.base_url, payment_id))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    pub async fn events(&self, limit: u32) -> anyhow::Result<serde_json::Value> {
        Ok(self
            .client
            .get(format!("{}/v1/events", self.base_url))
            .query(&[("limit", limit)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
}

pub fn build_fetch_command(cache_dir: &Path) -> String {
    format!("scripts/fetch-payment-processor.sh {}", cache_dir.display(),)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::{
        config::Config,
        seeds::{WalletRole, material_from_seed},
    };
    use rusqlite::{Connection, params};
    use tari_common_types::seeds::cipher_seed::CipherSeed;

    use super::{build_env, inspect_payment_processor_db, payment_processor_db_path};

    #[test]
    fn pp_env_uses_private_view_key() {
        let cfg = Config::default();
        let seed = material_from_seed(
            WalletRole::PaymentProcessor,
            "HARNESS_SEED_PP".to_string(),
            CipherSeed::random(),
        )
        .unwrap();
        let env = build_env(&cfg, &seed);
        assert_eq!(
            env.vars.get("ACCOUNTS__DEFAULT__VIEW_KEY"),
            Some(&seed.private_view_key_hex)
        );
        assert!(env.vars.contains_key("CONSOLE_WALLET_PASSWORD"));
    }

    #[test]
    fn inspect_pp_db_reads_requested_rows_in_input_order() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.paths.data_dir = dir.path().to_path_buf();
        let db_path = payment_processor_db_path(&cfg);
        fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let conn = Connection::open(&db_path).unwrap();
        create_pp_snapshot_tables(&conn);
        insert_batch(&conn, "batch-2", "CONFIRMED");
        insert_batch(&conn, "batch-1", "FAILED");
        insert_payment(&conn, "payment-2", "CONFIRMED", "batch-2");
        insert_payment(&conn, "payment-1", "FAILED", "batch-1");

        let snapshot = inspect_payment_processor_db(
            &cfg,
            &["batch-1".to_string(), "batch-2".to_string()],
            &["payment-1".to_string(), "payment-2".to_string()],
        )
        .unwrap();

        assert_eq!(
            snapshot
                .batches
                .iter()
                .map(|batch| batch.id.as_str())
                .collect::<Vec<_>>(),
            vec!["batch-1", "batch-2"]
        );
        assert_eq!(
            snapshot
                .payments
                .iter()
                .map(|payment| payment.id.as_str())
                .collect::<Vec<_>>(),
            vec!["payment-1", "payment-2"]
        );
    }

    #[test]
    fn inspect_pp_db_reports_missing_requested_rows() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.paths.data_dir = dir.path().to_path_buf();
        let db_path = payment_processor_db_path(&cfg);
        fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let conn = Connection::open(&db_path).unwrap();
        create_pp_snapshot_tables(&conn);

        let error = inspect_payment_processor_db(&cfg, &["missing-batch".to_string()], &[])
            .expect_err("missing batch id must fail");

        assert!(format!("{error:#}").contains("reading PP batch missing-batch"));
    }

    fn create_pp_snapshot_tables(conn: &Connection) {
        conn.execute_batch(
            r#"
            CREATE TABLE payment_batches (
                id TEXT PRIMARY KEY NOT NULL,
                status TEXT NOT NULL,
                retry_count INTEGER NOT NULL DEFAULT 0,
                error_message TEXT,
                unsigned_tx_json TEXT,
                signed_tx_json TEXT,
                mined_height BIGINT
            );
            CREATE TABLE payments (
                id TEXT PRIMARY KEY NOT NULL,
                status TEXT NOT NULL,
                payment_batch_id TEXT,
                failure_reason TEXT
            );
            "#,
        )
        .unwrap();
    }

    fn insert_batch(conn: &Connection, id: &str, status: &str) {
        conn.execute(
            r#"
            INSERT INTO payment_batches (
                id, status, retry_count, error_message, unsigned_tx_json, signed_tx_json, mined_height
            )
            VALUES (?1, ?2, 0, NULL, '{}', '{}', 42)
            "#,
            params![id, status],
        )
        .unwrap();
    }

    fn insert_payment(conn: &Connection, id: &str, status: &str, batch_id: &str) {
        conn.execute(
            r#"
            INSERT INTO payments (id, status, payment_batch_id, failure_reason)
            VALUES (?1, ?2, ?3, NULL)
            "#,
            params![id, status, batch_id],
        )
        .unwrap();
    }
}
