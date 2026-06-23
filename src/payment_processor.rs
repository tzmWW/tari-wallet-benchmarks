use std::{collections::BTreeMap, path::Path};

use serde::{Deserialize, Serialize};

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
        format!(
            "sqlite://{}",
            config
                .paths
                .data_dir
                .join("payment-processor/payments.db")
                .display()
        ),
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
        config.paths.minotari_console_wallet.display().to_string(),
    );
    vars.insert(
        "CONSOLE_WALLET_BASE_PATH".to_string(),
        config
            .paths
            .data_dir
            .join("payment-processor-console-wallet")
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

impl PaymentProcessorClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
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
    use crate::{
        config::Config,
        seeds::{WalletRole, material_from_seed},
    };
    use tari_common_types::seeds::cipher_seed::CipherSeed;

    use super::build_env;

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
}
