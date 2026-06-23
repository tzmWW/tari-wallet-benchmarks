use anyhow::bail;

use crate::config::Config;

const MAINNET_HOST_DENYLIST: &[&str] = &["mainnet", "rpc.tari.com"];

pub fn enforce_esmeralda(config: &Config) -> anyhow::Result<()> {
    if config.network.name.to_lowercase() != "esmeralda" {
        bail!("refusing to run: only Esmeralda is allowed");
    }

    let base = config.network.base_node_http_url.to_lowercase();
    if MAINNET_HOST_DENYLIST
        .iter()
        .any(|needle| base.contains(needle))
    {
        bail!(
            "refusing to run: base node URL '{}' looks like mainnet",
            config.network.base_node_http_url
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::config::Config;

    use super::enforce_esmeralda;

    #[test]
    fn rejects_mainnet_like_endpoint() {
        let mut cfg = Config::default();
        cfg.network.base_node_http_url = "https://rpc.tari.com".to_string();
        assert!(enforce_esmeralda(&cfg).is_err());
    }
}
