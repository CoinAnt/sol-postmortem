use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use solana_transaction_status::EncodedConfirmedTransactionWithStatusMeta;
use std::time::Duration;

#[derive(Serialize)]
struct RpcRequest<'a> {
    jsonrpc: &'a str,
    id: u32,
    method: &'a str,
    params: Value,
}

#[derive(Deserialize)]
struct RpcEnvelope {
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<RpcError>,
}

#[derive(Deserialize, Debug)]
struct RpcError {
    code: i64,
    message: String,
}

pub fn fetch_transaction(
    rpc_url: &str,
    signature_str: &str,
) -> Result<EncodedConfirmedTransactionWithStatusMeta> {
    // Validate signature shape (base58 of 64 bytes) before paying for the round-trip.
    let sig_bytes = bs58::decode(signature_str)
        .into_vec()
        .with_context(|| format!("invalid base58 signature: {signature_str}"))?;
    if sig_bytes.len() != 64 {
        return Err(anyhow!(
            "signature must be 64 bytes after base58-decode, got {}",
            sig_bytes.len()
        ));
    }

    let body = RpcRequest {
        jsonrpc: "2.0",
        id: 1,
        method: "getTransaction",
        params: json!([
            signature_str,
            {
                "encoding": "json",
                "commitment": "confirmed",
                "maxSupportedTransactionVersion": 0
            }
        ]),
    };

    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .build();

    let response = agent
        .post(rpc_url)
        .set("content-type", "application/json")
        .send_json(serde_json::to_value(&body)?)
        .with_context(|| format!("RPC POST failed: {rpc_url}"))?;

    let envelope: RpcEnvelope = response
        .into_json()
        .context("RPC response was not valid JSON")?;

    if let Some(err) = envelope.error {
        return Err(anyhow!("RPC error {}: {}", err.code, err.message));
    }
    let result = envelope
        .result
        .ok_or_else(|| anyhow!("RPC returned neither result nor error"))?;
    if result.is_null() {
        return Err(anyhow!(
            "transaction not found — check signature, commitment, and that your RPC has the slot"
        ));
    }

    let tx: EncodedConfirmedTransactionWithStatusMeta = serde_json::from_value(result)
        .context("failed to deserialize getTransaction response into solana types")?;

    if tx.transaction.meta.is_none() {
        return Err(anyhow!("transaction has no meta — likely not yet confirmed"));
    }

    Ok(tx)
}
