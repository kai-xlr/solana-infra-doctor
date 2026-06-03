//! WebSocket readiness diagnostics: connect, `slotSubscribe`, measure
//! time-to-first-slot-notification, unsubscribe, and close.

use crate::{cli::WsArgs, error::AppError, redact, rpc::RpcEndpoint, verdict::Verdict};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;
use tokio::time::{timeout, Instant};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use url::Url;

const SLOT_SUBSCRIBE_METHOD: &str = "slotSubscribe";
const SLOT_SUBSCRIBE_REQUEST: &str = r#"{"jsonrpc":"2.0","id":1,"method":"slotSubscribe"}"#;
const GOOD_NOTIFY_MS: u128 = 2_000;
const UNSUBSCRIBE_ACK_MS: u64 = 1_000;

#[derive(Debug, Clone, Serialize)]
pub struct WsReport {
    pub verdict: Verdict,
    pub rpc_url: String,
    pub ws_url: String,
    pub connected: bool,
    pub connect_latency_ms: Option<u128>,
    pub subscription_method: &'static str,
    pub subscribed: bool,
    pub time_to_first_notification_ms: Option<u128>,
    pub first_slot: Option<u64>,
    pub unsubscribed: bool,
    pub closed_cleanly: bool,
    pub summary: String,
    pub notes: Vec<String>,
}

impl WsReport {
    fn new(rpc_url: String, ws_url: String) -> Self {
        Self {
            verdict: Verdict::Unknown,
            rpc_url,
            ws_url,
            connected: false,
            connect_latency_ms: None,
            subscription_method: SLOT_SUBSCRIBE_METHOD,
            subscribed: false,
            time_to_first_notification_ms: None,
            first_slot: None,
            unsubscribed: false,
            closed_cleanly: false,
            summary: String::new(),
            notes: Vec::new(),
        }
    }

    fn finish(mut self) -> Self {
        let (verdict, summary, notes) = classify(&self);
        self.verdict = verdict;
        self.summary = summary;
        self.notes = notes;
        self
    }
}

pub fn classify(report: &WsReport) -> (Verdict, String, Vec<String>) {
    if !report.connected {
        return (
            Verdict::Bad,
            "websocket connection failed".to_string(),
            Vec::new(),
        );
    }
    if !report.subscribed {
        return (
            Verdict::Bad,
            "slotSubscribe did not succeed".to_string(),
            Vec::new(),
        );
    }
    match report.time_to_first_notification_ms {
        None => (
            Verdict::Bad,
            "no slot notification received before timeout".to_string(),
            Vec::new(),
        ),
        Some(ms) if ms <= GOOD_NOTIFY_MS && report.unsubscribed && report.closed_cleanly => (
            Verdict::Good,
            "websocket realtime checks succeeded".to_string(),
            Vec::new(),
        ),
        Some(ms) => {
            let mut notes = Vec::new();
            if ms > GOOD_NOTIFY_MS {
                notes.push(format!("First slot notification was slow at {ms}ms."));
            }
            if !report.unsubscribed {
                notes.push("Unsubscribe did not complete cleanly.".to_string());
            }
            if !report.closed_cleanly {
                notes.push("Connection did not close cleanly.".to_string());
            }
            (
                Verdict::Warning,
                "websocket is reachable but realtime behavior is degraded".to_string(),
                notes,
            )
        }
    }
}

/// Derive a `ws`/`wss` URL from the HTTP RPC URL, or validate an explicit override.
pub fn derive_ws_url(rpc: &Url, override_ws: Option<&str>) -> Result<Url, AppError> {
    if let Some(raw) = override_ws {
        let parsed = Url::parse(raw).map_err(|error| AppError::InvalidRpcUrl {
            reason: error.to_string(),
        })?;
        return match parsed.scheme() {
            "ws" | "wss" => Ok(parsed),
            other => Err(AppError::InvalidRpcUrl {
                reason: format!("unsupported WebSocket scheme '{other}', expected ws or wss"),
            }),
        };
    }

    let mut ws = rpc.clone();
    let scheme = match rpc.scheme() {
        "https" => "wss",
        "http" => "ws",
        other => {
            return Err(AppError::InvalidRpcUrl {
                reason: format!("cannot derive WebSocket URL from scheme '{other}'"),
            })
        }
    };
    ws.set_scheme(scheme)
        .map_err(|()| AppError::InvalidRpcUrl {
            reason: "cannot derive WebSocket URL".to_string(),
        })?;
    Ok(ws)
}

pub async fn run_ws(args: WsArgs) -> Result<WsReport, AppError> {
    let endpoint = match RpcEndpoint::parse(&args.rpc) {
        Ok(endpoint) => endpoint,
        Err(AppError::InvalidRpcUrl { reason }) => {
            let mut report = WsReport::new("<invalid>".to_string(), "<invalid>".to_string());
            report.summary = format!("invalid RPC URL: {}", redact::redact_text(&reason));
            report.verdict = Verdict::Bad;
            return Ok(report);
        }
        Err(error) => return Err(error),
    };

    let rpc_redacted = endpoint.redacted();
    let ws_url = match derive_ws_url(endpoint.as_url(), args.ws.as_deref()) {
        Ok(url) => url,
        Err(AppError::InvalidRpcUrl { reason }) => {
            let mut report = WsReport::new(rpc_redacted, "<invalid>".to_string());
            report.summary = format!("invalid WebSocket URL: {}", redact::redact_text(&reason));
            report.verdict = Verdict::Bad;
            return Ok(report);
        }
        Err(error) => return Err(error),
    };

    let ws_redacted = redact::redact_url(&ws_url);
    let mut report = WsReport::new(rpc_redacted, ws_redacted);
    let duration = Duration::from_millis(args.timeout_ms);

    let started = Instant::now();
    let mut stream = match timeout(duration, connect_async(ws_url.as_str())).await {
        Ok(Ok((stream, _response))) => {
            report.connected = true;
            report.connect_latency_ms = Some(started.elapsed().as_millis());
            stream
        }
        Ok(Err(_)) | Err(_) => return Ok(report.finish()),
    };

    let subscribe_started = Instant::now();
    let deadline = subscribe_started + duration;
    let mut sub_id = None;
    let mut got_notification = false;

    if stream
        .send(Message::text(SLOT_SUBSCRIBE_REQUEST))
        .await
        .is_ok()
    {
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match timeout(remaining, stream.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => match serde_json::from_str::<Value>(&text) {
                    Ok(value) => {
                        if value.get("method").and_then(Value::as_str) == Some("slotNotification") {
                            report.first_slot = value
                                .get("params")
                                .and_then(|params| params.get("result"))
                                .and_then(|result| result.get("slot"))
                                .and_then(Value::as_u64);
                            report.time_to_first_notification_ms =
                                Some(subscribe_started.elapsed().as_millis());
                            got_notification = true;
                            break;
                        }
                        // A confirmation `{"result":<subId>,"id":1}` confirms the
                        // subscription and gives the id used to unsubscribe.
                        if value.get("id").and_then(Value::as_u64) == Some(1)
                            && value.get("result").is_some()
                        {
                            report.subscribed = true;
                            sub_id = value.get("result").and_then(Value::as_u64);
                        }
                    }
                    Err(_) => break, // malformed frame
                },
                Ok(Some(Ok(Message::Close(_)))) | Ok(None) => break, // server closed
                Ok(Some(Ok(_))) => {}                                // ping/pong/binary
                Ok(Some(Err(_))) => break,                           // protocol error
                Err(_) => break,                                     // timed out
            }
        }
    }

    if got_notification {
        report.subscribed = true;
        report.unsubscribed = unsubscribe(&mut stream, sub_id).await;
        report.closed_cleanly = stream.close(None).await.is_ok();
    } else {
        let _ = stream.close(None).await;
    }

    Ok(report.finish())
}

async fn unsubscribe(
    stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    sub_id: Option<u64>,
) -> bool {
    let Some(id) = sub_id else {
        return false;
    };
    let request =
        format!(r#"{{"jsonrpc":"2.0","id":2,"method":"slotUnsubscribe","params":[{id}]}}"#);
    if stream.send(Message::text(request)).await.is_err() {
        return false;
    }
    // Best-effort: wait briefly for the acknowledgement, but treat a sent
    // unsubscribe as success even if the ack does not arrive in time.
    let _ = timeout(Duration::from_millis(UNSUBSCRIBE_ACK_MS), stream.next()).await;
    true
}

pub fn render_human(report: &WsReport) -> String {
    let mut output = String::new();
    output.push_str("Solana Infra Doctor — WebSocket\n");
    output.push_str("===============================\n");
    output.push_str(&format!("RPC URL: {}\n", report.rpc_url));
    output.push_str(&format!("WS URL:  {}\n", report.ws_url));
    output.push_str(&format!("Verdict: {}\n", report.verdict));
    output.push_str(&format!("Summary: {}\n\n", report.summary));

    output.push_str(&format!(
        "Connect:      {}\n",
        format_step(report.connected, report.connect_latency_ms.map(format_ms)),
    ));
    output.push_str(&format!(
        "Subscribe:    {}\n",
        format_step(
            report.subscribed,
            Some(format!("{} (id 1)", report.subscription_method))
        ),
    ));
    output.push_str(&format!(
        "First slot:   {}\n",
        format_step(
            report.first_slot.is_some(),
            report
                .time_to_first_notification_ms
                .map(|ms| match report.first_slot {
                    Some(slot) => format!("{ms}ms (slot {slot})"),
                    None => format_ms(ms),
                })
        ),
    ));
    output.push_str(&format!(
        "Unsubscribe:  {}\n",
        format_step(report.unsubscribed, None)
    ));
    output.push_str(&format!(
        "Close:        {}\n",
        format_step(report.closed_cleanly, None)
    ));

    if !report.notes.is_empty() {
        output.push_str("\nNotes:\n");
        for note in &report.notes {
            output.push_str(&format!("- {note}\n"));
        }
    }
    output
}

pub fn render_json(report: &WsReport) -> Result<String, AppError> {
    serde_json::to_string_pretty(report).map_err(AppError::SerializeReport)
}

fn format_step(ok: bool, detail: Option<String>) -> String {
    let status = if ok { "OK" } else { "FAIL" };
    match detail {
        Some(detail) => format!("{status:<5} {detail}"),
        None => status.to_string(),
    }
}

fn format_ms(ms: u128) -> String {
    format!("{ms}ms")
}
