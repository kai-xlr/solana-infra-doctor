//! JSON-RPC request/response wire types and Solana RPC result models.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: &'static str,
    pub params: Vec<Value>,
}

impl JsonRpcRequest {
    pub fn new(id: u64, method: &'static str) -> Self {
        Self::with_params(id, method, Vec::new())
    }

    pub fn with_params(id: u64, method: &'static str, params: Vec<Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method,
            params,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcResponse<T> {
    pub jsonrpc: String,
    pub id: u64,
    pub result: Option<T>,
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VersionInfo {
    #[serde(rename = "solana-core")]
    pub solana_core: String,
    #[serde(default)]
    pub feature_set: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LatestBlockhashResponse {
    pub value: LatestBlockhashValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LatestBlockhashValue {
    pub blockhash: String,
    #[serde(rename = "lastValidBlockHeight")]
    pub last_valid_block_height: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BlockhashValidResponse {
    pub value: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PerformanceSample {
    pub slot: u64,
    #[serde(rename = "numSlots")]
    pub num_slots: u64,
    #[serde(rename = "numTransactions")]
    pub num_transactions: u64,
    #[serde(rename = "samplePeriodSecs")]
    pub sample_period_secs: u64,
    #[serde(rename = "numNonVoteTransactions", default)]
    pub num_non_vote_transactions: Option<u64>,
}
