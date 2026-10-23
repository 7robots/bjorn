//! The Craft Connect API: one HTTP request at a time, kept under its limits.
//!
//! Craft allows 100 requests a minute per space and 50 every ten seconds per
//! address, shared with anything else using the same link (the MCP server,
//! scripts). Bjorn keeps well under both, and its background work (filling
//! the body cache, walking folders) stops at a lower ceiling still, so a note
//! opened by hand never queues behind it.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use serde_json::Value;

use crate::bear::{BearError, Result};

/// One API request. `query` pairs go on the URL; `body` is sent as JSON.
pub trait Transport: Send + Sync {
    fn request<'a>(
        &'a self,
        method: &'a str,
        path: &'a str,
        query: &'a [(&'a str, String)],
        body: Option<&'a Value>,
    ) -> BoxFuture<'a, Result<Value>>;
}

/// (window, requests allowed in it) for work someone is waiting on.
const FOREGROUND: [(Duration, usize); 2] =
    [(Duration::from_secs(60), 85), (Duration::from_secs(10), 40)];
/// The same for background work, leaving room for the foreground.
const BACKGROUND: [(Duration, usize); 2] =
    [(Duration::from_secs(60), 50), (Duration::from_secs(10), 20)];
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const RETRIES: u32 = 5;

/// Requests sent in the last minute, shared by both priorities.
#[derive(Default)]
pub struct Throttle {
    sent: std::sync::Mutex<VecDeque<Instant>>,
}

impl Throttle {
    /// Wait until a request fits under `limits`, and count it.
    pub async fn admit(&self, background: bool) {
        let limits = if background { &BACKGROUND } else { &FOREGROUND };
        loop {
            let wait = {
                let mut sent = self.sent.lock().unwrap();
                let now = Instant::now();
                while sent
                    .front()
                    .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60))
                {
                    sent.pop_front();
                }
                let mut wait = Duration::ZERO;
                for (span, cap) in limits {
                    let recent: Vec<&Instant> = sent
                        .iter()
                        .filter(|t| now.duration_since(**t) < *span)
                        .collect();
                    if recent.len() >= *cap {
                        let oldest = *recent[recent.len() - cap];
                        wait = wait
                            .max(*span - now.duration_since(oldest) + Duration::from_millis(50));
                    }
                }
                if wait.is_zero() {
                    sent.push_back(now);
                }
                wait
            };
            if wait.is_zero() {
                return;
            }
            tokio::time::sleep(wait).await;
        }
    }
}

/// The real API over HTTPS.
pub struct Http {
    base: String,
    key: String,
    client: reqwest::Client,
    throttle: Throttle,
    /// Set by the store while it runs background work on this transport.
    background: tokio::sync::Mutex<()>,
}

impl Http {
    pub fn new(base: &str, key: &str) -> Result<Http> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            // Cloudflare in front of the API turns away anonymous clients.
            .user_agent(concat!("bjorn/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| BearError::new(format!("Craft: could not start HTTP: {e}")))?;
        Ok(Http {
            base: base.trim().trim_end_matches('/').to_string(),
            key: key.trim().to_string(),
            client,
            throttle: Throttle::default(),
            background: tokio::sync::Mutex::new(()),
        })
    }

    async fn send(
        &self,
        method: &str,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
        background: bool,
    ) -> Result<Value> {
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|_| BearError::new(format!("Craft: bad method {method}")))?;
        let idempotent = method != reqwest::Method::POST;
        let url = format!("{}{}", self.base, path);
        let mut attempt = 0;
        loop {
            self.throttle.admit(background).await;
            let mut request = self
                .client
                .request(method.clone(), &url)
                .header("Accept", "application/json")
                .query(query);
            if !self.key.is_empty() {
                request = request.bearer_auth(&self.key);
            }
            if let Some(body) = body {
                request = request.json(body);
            }
            let outcome = request.send().await;
            let retry_after = |resp: &reqwest::Response| {
                resp.headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .map(Duration::from_secs)
            };
            let backoff =
                Duration::from_millis(500 * 2u64.pow(attempt)).min(Duration::from_secs(30));
            match outcome {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        let bytes = resp.bytes().await.map_err(|e| {
                            BearError::new(format!("Craft: reading the answer failed: {e}"))
                        })?;
                        if bytes.iter().all(u8::is_ascii_whitespace) {
                            return Ok(Value::Null);
                        }
                        return serde_json::from_slice(&bytes).map_err(|e| {
                            BearError::new(format!("Craft returned invalid JSON: {e}"))
                        });
                    }
                    // A 429 was never processed, so anything may try again; a
                    // 5xx on a POST may have been applied, so only reads and
                    // idempotent writes do.
                    let retryable = status.as_u16() == 429
                        || (idempotent && matches!(status.as_u16(), 500 | 502 | 503 | 504));
                    if retryable && attempt < RETRIES {
                        let wait = retry_after(&resp).unwrap_or(backoff);
                        attempt += 1;
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    let text = resp.text().await.unwrap_or_default();
                    return Err(api_error(status.as_u16(), path, &text));
                }
                Err(err) => {
                    if idempotent && attempt < RETRIES && (err.is_timeout() || err.is_connect()) {
                        attempt += 1;
                        tokio::time::sleep(backoff).await;
                        continue;
                    }
                    return Err(BearError::with_code(
                        format!("Craft could not be reached: {}", without_url(&err)),
                        "offline",
                    ));
                }
            }
        }
    }

    /// The same request, at background priority.
    pub async fn background(
        &self,
        method: &str,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Value> {
        let _one_at_a_time = self.background.lock().await;
        self.send(method, path, query, body, true).await
    }
}

/// reqwest's errors carry the URL, and the URL of a Craft link is a secret.
fn without_url(err: &reqwest::Error) -> String {
    let mut text = err.to_string();
    if let Some(url) = err.url() {
        text = text.replace(url.as_str(), "<craft>");
    }
    text
}

fn api_error(status: u16, path: &str, text: &str) -> BearError {
    let detail = serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| {
            v.get("error").or_else(|| v.get("message")).map(|e| {
                e.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| e.to_string())
            })
        })
        .unwrap_or_else(|| text.chars().take(200).collect());
    let code = match status {
        404 => "not_found",
        429 => "rate_limited",
        401 | 403 => "unauthorized",
        _ => "",
    };
    BearError::with_code(
        format!("Craft {path}: HTTP {status}: {}", detail.trim()),
        code,
    )
}

impl Transport for Http {
    fn request<'a>(
        &'a self,
        method: &'a str,
        path: &'a str,
        query: &'a [(&'a str, String)],
        body: Option<&'a Value>,
    ) -> BoxFuture<'a, Result<Value>> {
        Box::pin(self.send(method, path, query, body, false))
    }
}

/// A transport that runs background work at the lower ceiling; the store
/// holds one of these beside the foreground transport.
pub struct Background(pub std::sync::Arc<Http>);

impl Transport for Background {
    fn request<'a>(
        &'a self,
        method: &'a str,
        path: &'a str,
        query: &'a [(&'a str, String)],
        body: Option<&'a Value>,
    ) -> BoxFuture<'a, Result<Value>> {
        Box::pin(self.0.background(method, path, query, body))
    }
}

/// Resolve a config value that may be a 1Password reference (`op://…`), read
/// with `op read`. Anything else is taken as it is.
pub fn resolve_secret(value: &str, op_account: &str) -> std::result::Result<String, String> {
    let value = value.trim();
    if !value.starts_with("op://") {
        return Ok(value.to_string());
    }
    let mut cmd = std::process::Command::new("op");
    cmd.arg("read");
    if !op_account.trim().is_empty() {
        cmd.args(["--account", op_account.trim()]);
    }
    let out = cmd
        .arg(value)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("could not run op to read {value}: {e}"))?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        let why = why.lines().next().unwrap_or("").trim();
        return Err(if why.contains("authorization timeout") {
            format!("1Password is locked; unlock it and start again ({value})")
        } else {
            format!("op read {value} failed: {why}")
        });
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() {
        return Err(format!("op read {value} returned nothing"));
    }
    Ok(text)
}
