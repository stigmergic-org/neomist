use std::sync::mpsc::Sender;
use std::time::Duration;

use alloy_primitives::U256;
use eyre::{Result, WrapErr};
use serde_json::json;
use tokio::time::interval;
use tracing::{info, warn};

use crate::state::AppState;

pub async fn poll_gas_price(
    state: AppState,
    tx: Sender<String>,
) {
    let mut ticker = interval(Duration::from_secs(15));
    info!("Gas price polling started");

    loop {
        ticker.tick().await;

        let execution_rpcs = {
            let config = state.config.read().await;
            config.execution_rpcs.clone()
        };

        let mut success = false;
        for rpc in &execution_rpcs {
            match fetch_gas_price(&state.http_client, rpc).await {
                Ok(price) => {
                    let label = match u128::try_from(price) {
                        Ok(wei) => {
                            let gwei = wei as f64 / 1_000_000_000f64;
                            if gwei < 1.0 {
                                let mwei = wei as f64 / 1_000_000f64;
                                format!("{:.0} Mwei", mwei)
                            } else {
                                format!("{:.1} Gwei", gwei)
                            }
                        }
                        Err(_) => format!("{} Gwei", price / U256::from(1_000_000_000u64)),
                    };
                    let _ = tx.send(label);
                    success = true;
                    break;
                }
                Err(err) => {
                    warn!("Failed to fetch gas price from {rpc}: {err}");
                }
            }
        }

        if !success {
            warn!("Failed to fetch gas price from all available RPCs");
        }
    }
}

async fn fetch_gas_price(http_client: &reqwest::Client, rpc_url: &str) -> Result<U256> {
    match fetch_base_fee(http_client, rpc_url).await {
        Ok(value) => Ok(value),
        Err(_) => {
            let payload = json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "eth_gasPrice",
                "params": []
            });

            let response: serde_json::Value = http_client
                .post(rpc_url)
                .json(&payload)
                .send()
                .await
                .wrap_err("Failed to call eth_gasPrice")?
                .json()
                .await
                .wrap_err("Failed to decode eth_gasPrice response")?;

            let gas_price = response
                .get("result")
                .and_then(|value| value.as_str())
                .ok_or_else(|| eyre::eyre!("eth_gasPrice missing result"))?;

            parse_hex_u256(gas_price)
        }
    }
}

async fn fetch_base_fee(http_client: &reqwest::Client, rpc_url: &str) -> Result<U256> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_feeHistory",
        "params": ["0x1", "latest", []]
    });

    let response: serde_json::Value = http_client
        .post(rpc_url)
        .json(&payload)
        .send()
        .await
        .wrap_err("Failed to call eth_feeHistory")?
        .json()
        .await
        .wrap_err("Failed to decode eth_feeHistory response")?;

    let base_fee = response
        .get("result")
        .and_then(|result| result.get("baseFeePerGas"))
        .and_then(|fees| fees.get(0))
        .and_then(|fee| fee.as_str())
        .ok_or_else(|| eyre::eyre!("eth_feeHistory missing baseFeePerGas"))?;

    parse_hex_u256(base_fee)
}

fn parse_hex_u256(value: &str) -> Result<U256> {
    let trimmed = value.trim_start_matches("0x");
    U256::from_str_radix(trimmed, 16).wrap_err("Failed to parse hex U256")
}
