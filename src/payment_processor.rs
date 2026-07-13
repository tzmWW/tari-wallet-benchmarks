use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::ExitStatus,
    time::{Duration, Instant},
};

use anyhow::{Context, bail};
use rusqlite::{Connection, OpenFlags, params_from_iter};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tari_transaction_components::offline_signing::models::SignedOneSidedTransactionResult;
use tari_utilities::ByteArray;
use tokio::{process::Command, time};

use crate::{config::Config, seeds::SeedMaterial};

pub use crate::managed_process::ManagedProcess;

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
    vars.insert(
        "FEE_PER_GRAM".to_string(),
        config
            .fee_rate()
            .expect("validated benchmark fee rate")
            .0
            .to_string(),
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
        bail!(
            "payment-processor signer DB already exists at {}; canonical runs require pristine signer state",
            console_wallet_db_path(&base_path).display()
        );
    }
    fs::create_dir_all(&base_path)?;
    let mut command = Command::new(&config.paths.minotari_console_wallet);
    command
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
        .arg("get-balance");
    let mut process = ManagedProcess::spawn(
        "mode3-signer-init",
        command,
        &config.paths.data_dir.join("logs"),
    )?;
    let status = process
        .wait(config.timeout(config.timeouts.startup_secs))
        .await
        .context("initializing payment-processor console wallet signer base path")?;

    if !status.success() {
        bail!(
            "console wallet signer initialization failed: status={} stderr_log={} stdout_log={}",
            status,
            process.stderr_path.display(),
            process.stdout_path.display()
        );
    }
    Ok(())
}

fn console_wallet_db_path(base_path: &Path) -> PathBuf {
    base_path
        .join("esmeralda")
        .join("data/wallet/db/console_wallet.db")
}

pub fn payment_processor_signer_db_path(config: &Config) -> PathBuf {
    console_wallet_db_path(&console_wallet_base_path(config))
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
        // The companion wallet must refresh after each confirmed PP batch so
        // the bounty's per-round UTXO and balance invariants observe current
        // state. This is wallet-state refresh, not transaction retry/backoff.
        .arg(config.benchmark.mode3_worker_sleep_secs.to_string())
        .arg("--api-port")
        .arg(api_port.to_string());
    ManagedProcess::spawn(
        "mode3-payment-receiver",
        command,
        &config.paths.data_dir.join("logs"),
    )
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
    ManagedProcess::spawn(
        "mode3-payment-processor",
        command,
        &config.paths.data_dir.join("logs"),
    )
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
    /// A payment-processor batch UUID is an API observation identifier, not a chain transaction id.
    /// These fields are populated only by decoding the signed transaction persisted by PP.
    pub chain_tx_id: Option<String>,
    pub fee_microtari: Option<u64>,
    pub kernel_excess_sig_nonce: Option<Vec<u8>>,
    pub kernel_excess_sig: Option<Vec<u8>>,
    pub input_count: Option<u32>,
    pub total_output_count: Option<u32>,
    pub output_commitments: Option<Vec<String>>,
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
            mined_height,
            signed_tx_json
        FROM payment_batches
        WHERE id IN ({placeholders})
        "#
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(ids.iter()), |row| {
        let signed_tx_json = row.get::<_, Option<String>>(7)?;
        let chain = signed_tx_json
            .as_deref()
            .and_then(|json| payment_batch_chain_fields(json).ok());
        Ok(PaymentBatchSnapshot {
            id: row.get(0)?,
            status: row.get(1)?,
            retry_count: row.get(2)?,
            error_message: row.get(3)?,
            has_unsigned_tx: row.get::<_, i64>(4)? != 0,
            has_signed_tx: row.get::<_, i64>(5)? != 0,
            mined_height: row.get(6)?,
            chain_tx_id: chain.as_ref().map(|fields| fields.chain_tx_id.clone()),
            fee_microtari: chain.as_ref().map(|fields| fields.fee_microtari),
            kernel_excess_sig_nonce: chain
                .as_ref()
                .map(|fields| fields.kernel_excess_sig_nonce.clone()),
            input_count: chain.as_ref().map(|fields| fields.input_count),
            total_output_count: chain.as_ref().map(|fields| fields.total_output_count),
            output_commitments: chain
                .as_ref()
                .map(|fields| fields.output_commitments.clone()),
            kernel_excess_sig: chain.map(|fields| fields.kernel_excess_sig),
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

#[derive(Debug)]
struct PaymentBatchChainFields {
    chain_tx_id: String,
    fee_microtari: u64,
    kernel_excess_sig_nonce: Vec<u8>,
    kernel_excess_sig: Vec<u8>,
    input_count: u32,
    total_output_count: u32,
    output_commitments: Vec<String>,
}

fn payment_batch_chain_fields(json: &str) -> anyhow::Result<PaymentBatchChainFields> {
    let value: serde_json::Value =
        serde_json::from_str(json).context("decoding PP signed batch payload JSON")?;
    let signed = find_signed_transaction(&value)
        .context("PP signed batch payload contains no signed transaction")?;
    let kernel = signed
        .signed_transaction
        .transaction
        .body()
        .kernels()
        .first()
        .context("PP signed transaction has no kernel")?;
    let kernel_excess_sig_nonce = kernel
        .excess_sig
        .get_compressed_public_nonce()
        .as_bytes()
        .to_vec();
    let kernel_excess_sig = kernel.excess_sig.get_signature().as_bytes().to_vec();
    let input_count = u32::try_from(signed.signed_transaction.transaction.body().inputs().len())
        .context("PP transaction input count exceeds u32")?;
    let total_output_count =
        u32::try_from(signed.signed_transaction.transaction.body().outputs().len())
            .context("PP transaction output count exceeds u32")?;
    let output_commitments = signed
        .signed_transaction
        .transaction
        .body()
        .outputs()
        .iter()
        .map(|output| hex::encode(output.commitment().as_bytes()))
        .collect();
    let chain_tx_id = format!(
        "{}:{}",
        hex::encode(&kernel_excess_sig_nonce),
        hex::encode(&kernel_excess_sig)
    );
    Ok(PaymentBatchChainFields {
        chain_tx_id,
        fee_microtari: kernel.fee.0,
        kernel_excess_sig_nonce,
        kernel_excess_sig,
        input_count,
        total_output_count,
        output_commitments,
    })
}

fn find_signed_transaction(value: &serde_json::Value) -> Option<SignedOneSidedTransactionResult> {
    if value.get("signed_transaction").is_some()
        && let Ok(signed) = serde_json::from_value(value.clone())
    {
        return Some(signed);
    }
    match value {
        serde_json::Value::String(json) => serde_json::from_str::<serde_json::Value>(json)
            .ok()
            .and_then(|nested| find_signed_transaction(&nested)),
        serde_json::Value::Array(values) => values.iter().find_map(find_signed_transaction),
        serde_json::Value::Object(values) => values.values().find_map(find_signed_transaction),
        _ => None,
    }
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
        response_json(
            self.client
                .get(format!("{}/health/version", self.base_url))
                .send()
                .await?,
        )
        .await
    }

    pub async fn create_payment(
        &self,
        request: &PaymentRequest,
    ) -> anyhow::Result<serde_json::Value> {
        response_json(
            self.client
                .post(format!("{}/v1/payments", self.base_url))
                .json(request)
                .send()
                .await?,
        )
        .await
    }

    pub async fn create_payment_batch(
        &self,
        request: &BulkPaymentRequest,
    ) -> anyhow::Result<serde_json::Value> {
        response_json(
            self.client
                .post(format!("{}/v1/payment-batches", self.base_url))
                .json(request)
                .send()
                .await?,
        )
        .await
    }

    pub async fn get_payment(&self, payment_id: &str) -> anyhow::Result<serde_json::Value> {
        response_json(
            self.client
                .get(format!("{}/v1/payments/{}", self.base_url, payment_id))
                .send()
                .await?,
        )
        .await
    }

    pub async fn events(&self, limit: u32) -> anyhow::Result<serde_json::Value> {
        response_json(
            self.client
                .get(format!("{}/v1/events", self.base_url))
                .query(&[("limit", limit)])
                .send()
                .await?,
        )
        .await
    }
}

async fn response_json<T: DeserializeOwned>(response: reqwest::Response) -> anyhow::Result<T> {
    let status = response.status();
    let url = response.url().to_string();
    let body = response
        .text()
        .await
        .unwrap_or_else(|error| format!("<failed to read response body: {error}>"));
    if !status.is_success() {
        bail!(
            "{}",
            payment_processor_http_error_message(status, &url, &body)
        );
    }
    serde_json::from_str(&body)
        .with_context(|| format!("decoding payment processor JSON response from {url}: {body}"))
}

fn payment_processor_http_error_message(
    status: reqwest::StatusCode,
    url: &str,
    body: &str,
) -> String {
    format!("payment processor HTTP {status} for {url}: {body}")
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

    use super::{
        build_env, inspect_payment_processor_db, payment_processor_db_path,
        payment_processor_http_error_message,
    };

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
        assert_eq!(env.vars.get("FEE_PER_GRAM").map(String::as_str), Some("5"));
    }

    #[test]
    fn pp_http_error_message_includes_response_body() {
        let error = payment_processor_http_error_message(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            "http://127.0.0.1:9000/v1/payment-batches",
            "{\"error\":\"funds pending\"}",
        );

        assert!(error.contains("500 Internal Server Error"));
        assert!(error.contains("/v1/payment-batches"));
        assert!(error.contains("funds pending"));
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
