use crate::{
    cli::CheckArgs,
    error::AppError,
    latency::{average_latency_ms, Latency},
    rpc::{
        BlockhashValidResponse, JsonRpcRequest, JsonRpcResponse, LatestBlockhashResponse,
        PerformanceSample, RpcClient, RpcEndpoint, VersionInfo,
    },
    verdict::Verdict,
};
use serde::Serialize;
use serde_json::Value;
use std::time::{Duration, Instant};

const GOOD_AVERAGE_LATENCY_MS: u128 = 500;
const WARNING_AVERAGE_LATENCY_MS: u128 = 1_500;

#[derive(Debug, Clone, Serialize)]
pub struct CheckReport {
    pub verdict: Verdict,
    pub rpc_url: String,
    pub summary: String,
    pub average_latency_ms: Option<u128>,
    pub fail_on_warning: bool,
    pub checks: Vec<RpcCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RpcCheck {
    pub category: CheckCategory,
    pub method: &'static str,
    pub status: CheckStatus,
    pub latency_ms: Option<u128>,
    pub detail: String,
    pub error_kind: Option<ErrorKind>,
    pub critical: bool,
}

impl RpcCheck {
    fn success(
        category: CheckCategory,
        method: &'static str,
        latency: Latency,
        detail: String,
    ) -> Self {
        Self {
            category,
            method,
            status: CheckStatus::Success,
            latency_ms: Some(latency.millis),
            detail,
            error_kind: None,
            critical: category.is_critical(),
        }
    }

    fn failed(
        category: CheckCategory,
        method: &'static str,
        latency: Option<Latency>,
        detail: String,
        error_kind: ErrorKind,
    ) -> Self {
        Self {
            category,
            method,
            status: CheckStatus::Failed,
            latency_ms: latency.map(|value| value.millis),
            detail,
            error_kind: Some(error_kind),
            critical: category.is_critical(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckCategory {
    Core,
    Blockhash,
    Performance,
}

impl CheckCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::Core => "Core",
            Self::Blockhash => "Blockhash",
            Self::Performance => "Performance",
        }
    }

    fn is_critical(self) -> bool {
        matches!(self, Self::Core | Self::Blockhash)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Success,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    InvalidUrl,
    Timeout,
    HttpError,
    RpcError,
    MalformedResponse,
    UnknownError,
}

impl ErrorKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::InvalidUrl => "invalid_url",
            Self::Timeout => "timeout",
            Self::HttpError => "http_error",
            Self::RpcError => "rpc_error",
            Self::MalformedResponse => "malformed_response",
            Self::UnknownError => "unknown_error",
        }
    }
}

pub async fn run_check(args: CheckArgs) -> Result<CheckReport, AppError> {
    let endpoint = match RpcEndpoint::parse(&args.rpc) {
        Ok(endpoint) => endpoint,
        Err(AppError::InvalidRpcUrl { reason }) => {
            return Ok(CheckReport {
                verdict: Verdict::Bad,
                rpc_url: "<invalid>".to_string(),
                summary: format!("invalid RPC URL: {reason}"),
                average_latency_ms: None,
                fail_on_warning: args.fail_on_warning,
                checks: vec![RpcCheck::failed(
                    CheckCategory::Core,
                    "url_validation",
                    None,
                    reason,
                    ErrorKind::InvalidUrl,
                )],
            });
        }
        Err(error) => return Err(error),
    };
    let redacted_rpc_url = endpoint.redacted();
    let client = RpcClient::new(endpoint, Duration::from_millis(args.timeout_ms))?;

    let mut checks = vec![
        check_health(&client).await,
        check_version(&client).await,
        check_genesis_hash(&client).await,
        check_slot(&client).await,
    ];

    let latest_blockhash = check_latest_blockhash(&client).await;
    let blockhash = latest_blockhash
        .status
        .eq(&CheckStatus::Success)
        .then(|| latest_blockhash.detail.clone());
    checks.push(latest_blockhash);
    checks.push(check_blockhash_valid(&client, blockhash.as_deref()).await);
    checks.push(check_performance_samples(&client).await);

    let average_latency_ms = average_latency_ms(
        checks
            .iter()
            .filter_map(|check| check.latency_ms.map(|millis| Latency { millis })),
    );
    let verdict = calculate_verdict(&checks, average_latency_ms);
    let summary = summarize(verdict, &checks, average_latency_ms, args.fail_on_warning);

    Ok(CheckReport {
        verdict,
        rpc_url: redacted_rpc_url,
        summary,
        average_latency_ms,
        fail_on_warning: args.fail_on_warning,
        checks,
    })
}

async fn check_health(client: &RpcClient) -> RpcCheck {
    match call_rpc::<String>(client, 1, "getHealth", Vec::new()).await {
        Ok((response, latency)) => match response.result {
            Some(value) if value == "ok" => RpcCheck::success(
                CheckCategory::Core,
                "getHealth",
                latency,
                "health is ok".to_string(),
            ),
            Some(value) => RpcCheck::failed(
                CheckCategory::Core,
                "getHealth",
                Some(latency),
                format!("unexpected health response: {value}"),
                ErrorKind::MalformedResponse,
            ),
            None => {
                failed_from_response(CheckCategory::Core, "getHealth", Some(latency), &response)
            }
        },
        Err(error) => failed_from_error(CheckCategory::Core, "getHealth", error),
    }
}

async fn check_version(client: &RpcClient) -> RpcCheck {
    match call_rpc::<VersionInfo>(client, 2, "getVersion", Vec::new()).await {
        Ok((response, latency)) => match response.result {
            Some(version) => RpcCheck::success(
                CheckCategory::Core,
                "getVersion",
                latency,
                format!("solana-core {}", version.solana_core),
            ),
            None => {
                failed_from_response(CheckCategory::Core, "getVersion", Some(latency), &response)
            }
        },
        Err(error) => failed_from_error(CheckCategory::Core, "getVersion", error),
    }
}

async fn check_genesis_hash(client: &RpcClient) -> RpcCheck {
    match call_rpc::<String>(client, 3, "getGenesisHash", Vec::new()).await {
        Ok((response, latency)) => match response.result {
            Some(hash) if !hash.trim().is_empty() => {
                RpcCheck::success(CheckCategory::Core, "getGenesisHash", latency, hash)
            }
            Some(_) => RpcCheck::failed(
                CheckCategory::Core,
                "getGenesisHash",
                Some(latency),
                "empty genesis hash".to_string(),
                ErrorKind::MalformedResponse,
            ),
            None => failed_from_response(
                CheckCategory::Core,
                "getGenesisHash",
                Some(latency),
                &response,
            ),
        },
        Err(error) => failed_from_error(CheckCategory::Core, "getGenesisHash", error),
    }
}

async fn check_slot(client: &RpcClient) -> RpcCheck {
    match call_rpc::<u64>(client, 4, "getSlot", Vec::new()).await {
        Ok((response, latency)) => match response.result {
            Some(slot) => RpcCheck::success(
                CheckCategory::Core,
                "getSlot",
                latency,
                format!("slot {slot}"),
            ),
            None => failed_from_response(CheckCategory::Core, "getSlot", Some(latency), &response),
        },
        Err(error) => failed_from_error(CheckCategory::Core, "getSlot", error),
    }
}

async fn check_latest_blockhash(client: &RpcClient) -> RpcCheck {
    match call_rpc::<LatestBlockhashResponse>(client, 5, "getLatestBlockhash", Vec::new()).await {
        Ok((response, latency)) => match response.result {
            Some(blockhash) if !blockhash.value.blockhash.trim().is_empty() => RpcCheck::success(
                CheckCategory::Blockhash,
                "getLatestBlockhash",
                latency,
                blockhash.value.blockhash,
            ),
            Some(_) => RpcCheck::failed(
                CheckCategory::Blockhash,
                "getLatestBlockhash",
                Some(latency),
                "empty latest blockhash".to_string(),
                ErrorKind::MalformedResponse,
            ),
            None => failed_from_response(
                CheckCategory::Blockhash,
                "getLatestBlockhash",
                Some(latency),
                &response,
            ),
        },
        Err(error) => failed_from_error(CheckCategory::Blockhash, "getLatestBlockhash", error),
    }
}

async fn check_blockhash_valid(client: &RpcClient, blockhash: Option<&str>) -> RpcCheck {
    let Some(blockhash) = blockhash else {
        return RpcCheck::failed(
            CheckCategory::Blockhash,
            "isBlockhashValid",
            None,
            "latest blockhash unavailable".to_string(),
            ErrorKind::MalformedResponse,
        );
    };

    match call_rpc::<BlockhashValidResponse>(
        client,
        6,
        "isBlockhashValid",
        vec![Value::String(blockhash.to_string())],
    )
    .await
    {
        Ok((response, latency)) => match response.result {
            Some(validity) if validity.value => RpcCheck::success(
                CheckCategory::Blockhash,
                "isBlockhashValid",
                latency,
                "latest blockhash is valid".to_string(),
            ),
            Some(_) => RpcCheck::failed(
                CheckCategory::Blockhash,
                "isBlockhashValid",
                Some(latency),
                "latest blockhash is not valid".to_string(),
                ErrorKind::RpcError,
            ),
            None => failed_from_response(
                CheckCategory::Blockhash,
                "isBlockhashValid",
                Some(latency),
                &response,
            ),
        },
        Err(error) => failed_from_error(CheckCategory::Blockhash, "isBlockhashValid", error),
    }
}

async fn check_performance_samples(client: &RpcClient) -> RpcCheck {
    match call_rpc::<Vec<PerformanceSample>>(client, 7, "getRecentPerformanceSamples", Vec::new())
        .await
    {
        Ok((response, latency)) => match response.result {
            Some(samples) if !samples.is_empty() => {
                let sample = &samples[0];
                RpcCheck::success(
                    CheckCategory::Performance,
                    "getRecentPerformanceSamples",
                    latency,
                    format!(
                        "{} transactions across {} slots in {}s",
                        sample.num_transactions, sample.num_slots, sample.sample_period_secs
                    ),
                )
            }
            Some(_) => RpcCheck::failed(
                CheckCategory::Performance,
                "getRecentPerformanceSamples",
                Some(latency),
                "no recent performance samples returned".to_string(),
                ErrorKind::MalformedResponse,
            ),
            None => failed_from_response(
                CheckCategory::Performance,
                "getRecentPerformanceSamples",
                Some(latency),
                &response,
            ),
        },
        Err(error) => failed_from_error(
            CheckCategory::Performance,
            "getRecentPerformanceSamples",
            error,
        ),
    }
}

async fn call_rpc<T>(
    client: &RpcClient,
    id: u64,
    method: &'static str,
    params: Vec<Value>,
) -> Result<(JsonRpcResponse<T>, Latency), AppError>
where
    T: serde::de::DeserializeOwned,
{
    let request = if params.is_empty() {
        JsonRpcRequest::new(id, method)
    } else {
        JsonRpcRequest::with_params(id, method, params)
    };
    let started = Instant::now();
    let response = client
        .call::<T>(&request)
        .await
        .map_err(|source| AppError::RpcRequest { method, source })?;
    if response.jsonrpc != "2.0" {
        return Err(AppError::UnexpectedRpcResponse {
            method,
            reason: format!("expected JSON-RPC 2.0, got {}", response.jsonrpc),
        });
    }
    if response.id != id {
        return Err(AppError::UnexpectedRpcResponse {
            method,
            reason: format!("expected response id {id}, got {}", response.id),
        });
    }
    let latency = Latency::from_duration(started.elapsed());

    Ok((response, latency))
}

fn failed_from_response<T>(
    category: CheckCategory,
    method: &'static str,
    latency: Option<Latency>,
    response: &JsonRpcResponse<T>,
) -> RpcCheck {
    if let Some(error) = &response.error {
        RpcCheck::failed(
            category,
            method,
            latency,
            format!("RPC error {}: {}", error.code, error.message),
            ErrorKind::RpcError,
        )
    } else {
        RpcCheck::failed(
            category,
            method,
            latency,
            "missing result".to_string(),
            ErrorKind::MalformedResponse,
        )
    }
}

fn failed_from_error(category: CheckCategory, method: &'static str, error: AppError) -> RpcCheck {
    let error_kind = classify_error(&error);
    RpcCheck::failed(category, method, None, error.to_string(), error_kind)
}

fn classify_error(error: &AppError) -> ErrorKind {
    match error {
        AppError::InvalidRpcUrl { .. } => ErrorKind::InvalidUrl,
        AppError::RpcRequest { source, .. } if source.is_timeout() => ErrorKind::Timeout,
        AppError::RpcRequest { source, .. } if source.is_status() => ErrorKind::HttpError,
        AppError::RpcRequest { source, .. } if source.is_decode() => ErrorKind::MalformedResponse,
        AppError::UnexpectedRpcResponse { .. } => ErrorKind::MalformedResponse,
        AppError::RpcRequest { .. } | AppError::HttpClient(_) | AppError::SerializeReport(_) => {
            ErrorKind::UnknownError
        }
    }
}

pub fn calculate_verdict(checks: &[RpcCheck], average_latency_ms: Option<u128>) -> Verdict {
    if checks.is_empty() {
        return Verdict::Unknown;
    }

    let failed_count = checks
        .iter()
        .filter(|check| check.status == CheckStatus::Failed)
        .count();
    let critical_failed = checks
        .iter()
        .any(|check| check.critical && check.status == CheckStatus::Failed);
    let timeout_failures = checks
        .iter()
        .filter(|check| check.error_kind == Some(ErrorKind::Timeout))
        .count();
    let invalid_url = checks
        .iter()
        .any(|check| check.error_kind == Some(ErrorKind::InvalidUrl));

    if invalid_url
        || critical_failed
        || failed_count >= 2
        || timeout_failures >= 2
        || average_latency_ms.is_some_and(|latency| latency > WARNING_AVERAGE_LATENCY_MS)
    {
        return Verdict::Bad;
    }

    if failed_count == 1
        || average_latency_ms.is_some_and(|latency| {
            latency > GOOD_AVERAGE_LATENCY_MS && latency <= WARNING_AVERAGE_LATENCY_MS
        })
    {
        return Verdict::Warning;
    }

    if failed_count == 0 && average_latency_ms.is_some() {
        Verdict::Good
    } else {
        Verdict::Unknown
    }
}

fn summarize(
    verdict: Verdict,
    checks: &[RpcCheck],
    average_latency_ms: Option<u128>,
    fail_on_warning: bool,
) -> String {
    let failed_count = checks
        .iter()
        .filter(|check| check.status == CheckStatus::Failed)
        .count();

    match verdict {
        Verdict::Good => "all RPC readiness checks succeeded".to_string(),
        Verdict::Warning => {
            let base = if failed_count > 0 {
                format!("RPC is reachable, but {failed_count} non-critical check failed")
            } else {
                let latency = average_latency_ms.unwrap_or_default();
                format!("RPC checks succeeded, but average latency is elevated at {latency}ms")
            };
            if fail_on_warning {
                format!(
                    "{base}; --fail-on-warning is enabled, so CI should treat this as a failure"
                )
            } else {
                base
            }
        }
        Verdict::Bad => {
            if failed_count > 0 {
                format!("{failed_count} RPC readiness checks failed")
            } else {
                let latency = average_latency_ms.unwrap_or_default();
                format!("average latency is too high at {latency}ms")
            }
        }
        Verdict::Unknown => "not enough data to produce a reliable verdict".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn success(category: CheckCategory, method: &'static str, latency_ms: u128) -> RpcCheck {
        RpcCheck {
            category,
            method,
            status: CheckStatus::Success,
            latency_ms: Some(latency_ms),
            detail: "ok".to_string(),
            error_kind: None,
            critical: category.is_critical(),
        }
    }

    fn failed(category: CheckCategory, method: &'static str, error_kind: ErrorKind) -> RpcCheck {
        RpcCheck {
            category,
            method,
            status: CheckStatus::Failed,
            latency_ms: None,
            detail: "request failed".to_string(),
            error_kind: Some(error_kind),
            critical: category.is_critical(),
        }
    }

    #[test]
    fn verdict_good_when_all_new_checks_pass_quickly() {
        let checks = vec![
            success(CheckCategory::Core, "getHealth", 100),
            success(CheckCategory::Core, "getVersion", 120),
            success(CheckCategory::Core, "getGenesisHash", 110),
            success(CheckCategory::Core, "getSlot", 90),
            success(CheckCategory::Blockhash, "getLatestBlockhash", 100),
            success(CheckCategory::Blockhash, "isBlockhashValid", 100),
            success(
                CheckCategory::Performance,
                "getRecentPerformanceSamples",
                100,
            ),
        ];
        assert_eq!(calculate_verdict(&checks, Some(103)), Verdict::Good);
    }

    #[test]
    fn verdict_warning_for_one_non_critical_failed_check() {
        let checks = vec![
            success(CheckCategory::Core, "getHealth", 100),
            success(CheckCategory::Blockhash, "getLatestBlockhash", 110),
            success(CheckCategory::Blockhash, "isBlockhashValid", 90),
            failed(
                CheckCategory::Performance,
                "getRecentPerformanceSamples",
                ErrorKind::RpcError,
            ),
        ];
        assert_eq!(calculate_verdict(&checks, Some(100)), Verdict::Warning);
    }

    #[test]
    fn verdict_bad_for_critical_blockhash_failure() {
        let checks = vec![
            success(CheckCategory::Core, "getHealth", 100),
            failed(
                CheckCategory::Blockhash,
                "isBlockhashValid",
                ErrorKind::RpcError,
            ),
        ];
        assert_eq!(calculate_verdict(&checks, Some(100)), Verdict::Bad);
    }

    #[test]
    fn verdict_bad_for_invalid_url() {
        let checks = vec![failed(
            CheckCategory::Core,
            "url_validation",
            ErrorKind::InvalidUrl,
        )];
        assert_eq!(calculate_verdict(&checks, None), Verdict::Bad);
    }

    #[test]
    fn verdict_warning_for_elevated_latency() {
        let checks = vec![success(CheckCategory::Core, "getHealth", 700)];
        assert_eq!(calculate_verdict(&checks, Some(700)), Verdict::Warning);
    }

    #[test]
    fn verdict_bad_for_repeated_timeouts() {
        let checks = vec![
            failed(CheckCategory::Performance, "a", ErrorKind::Timeout),
            failed(CheckCategory::Performance, "b", ErrorKind::Timeout),
        ];
        assert_eq!(calculate_verdict(&checks, None), Verdict::Bad);
    }

    #[test]
    fn verdict_bad_for_high_latency() {
        let checks = vec![success(CheckCategory::Core, "getHealth", 2_000)];
        assert_eq!(calculate_verdict(&checks, Some(2_000)), Verdict::Bad);
    }

    #[test]
    fn verdict_unknown_without_checks() {
        assert_eq!(calculate_verdict(&[], None), Verdict::Unknown);
    }
}
