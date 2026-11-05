//! Streaming HTTP client for `POST /v1/messages` with retries and SSE reassembly.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde_json::Value;
use thiserror::Error;

use super::types::*;
use crate::config::Config;
use crate::models::Tier;

pub const ANTHROPIC_VERSION: &str = "2023-06-01";
pub const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("{0}")]
    NoApiKey(String),
    #[error("API error {status} ({kind}): {message}")]
    Api { status: u16, kind: String, message: String },
    #[error("request refused by safety classifier{}", refusal_suffix(.category, .explanation))]
    Refusal {
        category: Option<String>,
        explanation: Option<String>,
    },
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("malformed stream: {0}")]
    Stream(String),
    #[error("gave up after {attempts} attempts: {last}")]
    Exhausted { attempts: u32, last: String },
}

fn refusal_suffix(category: &Option<String>, explanation: &Option<String>) -> String {
    let mut s = String::new();
    if let Some(c) = category {
        s.push_str(&format!(" (category: {c})"));
    }
    if let Some(e) = explanation {
        s.push_str(&format!(": {e}"));
    }
    s
}

/// Callback invoked with each streamed text delta.
pub type TextCallback = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone, Default)]
pub struct SendOptions {
    /// Used to decide whether to add server-side fallbacks.
    pub tier: Option<Tier>,
    /// Called with each text delta (progress).
    pub on_text: Option<TextCallback>,
    /// Additional `anthropic-beta` values.
    pub extra_betas: Vec<String>,
}

pub struct Client {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    max_retries: u32,
    fallbacks: bool,
    #[allow(dead_code)]
    timeout: Duration,
}

/// Outcome of a single attempt that did not succeed.
enum AttemptError {
    Retry { msg: String, retry_after: Option<Duration> },
    Fatal(ClientError),
}

impl Client {
    pub fn new(config: &Config) -> Result<Client, ClientError> {
        let key = config.api_key().map_err(|e| ClientError::NoApiKey(e.to_string()))?;
        Ok(Client::from_parts(
            key,
            config.api.base_url.clone(),
            config.api.max_retries,
            Duration::from_secs(config.api.timeout_secs),
            config.api.fallbacks,
        ))
    }

    pub fn from_parts(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        max_retries: u32,
        timeout: Duration,
        fallbacks: bool,
    ) -> Client {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(30))
            .build()
            .expect("failed to build reqwest client");
        Client {
            http,
            api_key: api_key.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            max_retries,
            fallbacks,
            timeout,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Send a request (forces `stream=true`) and return the reassembled response.
    pub async fn send(&self, req: &MessagesRequest, opts: &SendOptions) -> Result<MessagesResponse, ClientError> {
        let mut req = req.clone();
        req.stream = Some(true);
        let mut betas: Vec<String> = opts.extra_betas.clone();
        if self.fallbacks && opts.tier.is_some_and(|t| t.supports_fallbacks()) && req.fallbacks.is_none() {
            req.fallbacks = Some(Value::String("default".into()));
            if !betas.iter().any(|b| b == FALLBACK_BETA) {
                betas.push(FALLBACK_BETA.to_string());
            }
        }
        let body =
            serde_json::to_vec(&req).map_err(|e| ClientError::Stream(format!("cannot serialize request: {e}")))?;
        let beta_header = betas.join(",");
        let url = format!("{}/v1/messages", self.base_url);

        let nblocks: usize = req.messages.iter().map(|m| m.content.len()).sum();
        tracing::debug!(model = %req.model, max_tokens = req.max_tokens, blocks = nblocks,
            betas = %beta_header, "claude request start");
        let started = Instant::now();

        let mut attempts = 0u32;
        loop {
            attempts += 1;
            match self.attempt(&url, &body, &beta_header, opts).await {
                Ok(resp) => {
                    tracing::debug!(usage = ?resp.usage, stop_reason = ?resp.stop_reason,
                        elapsed_ms = started.elapsed().as_millis() as u64, attempts,
                        "claude request finished");
                    if resp.is_refusal() {
                        let d = resp.stop_details.clone().unwrap_or_default();
                        return Err(ClientError::Refusal {
                            category: d.category,
                            explanation: d.explanation,
                        });
                    }
                    return Ok(resp);
                }
                Err(AttemptError::Fatal(e)) => return Err(e),
                Err(AttemptError::Retry { msg, retry_after }) => {
                    if attempts > self.max_retries {
                        return Err(ClientError::Exhausted { attempts, last: msg });
                    }
                    let delay = retry_after.unwrap_or_else(|| backoff(attempts));
                    tracing::debug!(attempt = attempts, ?delay, error = %msg, "retrying claude request");
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    async fn attempt(
        &self,
        url: &str,
        body: &[u8],
        beta_header: &str,
        opts: &SendOptions,
    ) -> Result<MessagesResponse, AttemptError> {
        let mut rb = self
            .http
            .post(url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .body(body.to_vec());
        if !beta_header.is_empty() {
            rb = rb.header("anthropic-beta", beta_header);
        }
        let resp = match rb.send().await {
            Ok(r) => r,
            Err(e) => return Err(transport_error(e)),
        };
        let status = resp.status();
        if !status.is_success() {
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse::<f64>().ok())
                .filter(|s| *s >= 0.0)
                .map(|s| Duration::from_secs_f64(s.min(120.0)));
            let text = resp.text().await.unwrap_or_default();
            let (kind, mut message) = match serde_json::from_str::<ApiErrorBody>(&text) {
                Ok(b) if !b.error.message.is_empty() || !b.error.kind.is_empty() => (b.error.kind, b.error.message),
                _ => ("unknown".to_string(), text),
            };
            let code = status.as_u16();
            if matches!(code, 401 | 403) {
                message =
                    format!("{message} (check that your Anthropic API key is set, valid and permitted for this model)");
            }
            let retryable = matches!(code, 408 | 409 | 429) || code >= 500 || kind == "overloaded_error";
            return if retryable {
                Err(AttemptError::Retry {
                    msg: format!("HTTP {code} ({kind}): {message}"),
                    retry_after,
                })
            } else {
                Err(AttemptError::Fatal(ClientError::Api {
                    status: code,
                    kind,
                    message,
                }))
            };
        }

        let mut parser = SseParser::default();
        let mut asm = Assembler::new(opts.on_text.clone());
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(transport_error)?;
            for ev in parser.feed(&chunk) {
                asm.handle(&ev)?;
            }
            if asm.done {
                break;
            }
        }
        for ev in parser.finish() {
            asm.handle(&ev)?;
        }
        if !asm.done {
            return Err(AttemptError::Retry {
                msg: "stream ended before message_stop".into(),
                retry_after: None,
            });
        }
        Ok(asm.resp)
    }
}

fn transport_error(e: reqwest::Error) -> AttemptError {
    if e.is_timeout() || e.is_connect() || e.is_request() || e.is_body() || e.is_decode() {
        AttemptError::Retry {
            msg: e.to_string(),
            retry_after: None,
        }
    } else {
        AttemptError::Fatal(ClientError::Transport(e))
    }
}

fn backoff(attempt: u32) -> Duration {
    let base = 0.5f64 * 2f64.powi(attempt.saturating_sub(1).min(10) as i32);
    let base = base.min(30.0);
    let jitter = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0) as f64
        / 1e9)
        * 0.25
        * base;
    Duration::from_secs_f64(base + jitter)
}

/// One parsed server-sent event.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

/// Incremental SSE parser: buffer bytes, split on blank lines (LF or CRLF).
#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
}

impl SseParser {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some((end, skip)) = find_boundary(&self.buf) {
            let block: Vec<u8> = self.buf.drain(..end + skip).take(end).collect();
            if let Some(ev) = parse_block(&block) {
                out.push(ev);
            }
        }
        out
    }

    /// Flush a trailing event with no final blank line.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let rest = std::mem::take(&mut self.buf);
        parse_block(&rest).into_iter().collect()
    }
}

/// Returns (index of block end, length of the separator).
fn find_boundary(buf: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0;
    while i < buf.len() {
        if buf[i] == b'\n' {
            if buf.get(i + 1) == Some(&b'\n') {
                return Some((i, 2));
            }
            if buf.get(i + 1) == Some(&b'\r') && buf.get(i + 2) == Some(&b'\n') {
                return Some((i, 3));
            }
        }
        i += 1;
    }
    None
}

fn parse_block(block: &[u8]) -> Option<SseEvent> {
    let text = String::from_utf8_lossy(block);
    let mut ev = SseEvent::default();
    let mut has_data = false;
    for line in text.lines() {
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "event" => ev.event = Some(value.to_string()),
            "data" => {
                if has_data {
                    ev.data.push('\n');
                }
                ev.data.push_str(value);
                has_data = true;
            }
            _ => {}
        }
    }
    if has_data || ev.event.is_some() {
        Some(ev)
    } else {
        None
    }
}

struct Assembler {
    resp: MessagesResponse,
    on_text: Option<TextCallback>,
    done: bool,
}

impl Assembler {
    fn new(on_text: Option<TextCallback>) -> Self {
        Assembler {
            resp: MessagesResponse::default(),
            on_text,
            done: false,
        }
    }

    fn block(&mut self, idx: usize) -> &mut ContentBlock {
        while self.resp.content.len() <= idx {
            self.resp.content.push(ContentBlock::Other);
        }
        &mut self.resp.content[idx]
    }

    fn merge_usage(&mut self, u: &Value) {
        let get = |k: &str| u.get(k).and_then(Value::as_u64);
        let us = &mut self.resp.usage;
        if let Some(v) = get("input_tokens") {
            us.input_tokens = v;
        }
        if let Some(v) = get("output_tokens") {
            us.output_tokens = v;
        }
        if let Some(v) = get("cache_creation_input_tokens") {
            us.cache_creation_input_tokens = v;
        }
        if let Some(v) = get("cache_read_input_tokens") {
            us.cache_read_input_tokens = v;
        }
    }

    fn handle(&mut self, ev: &SseEvent) -> Result<(), AttemptError> {
        let name = ev.event.as_deref().unwrap_or("");
        if ev.data.trim().is_empty() {
            return Ok(());
        }
        let v: Value = match serde_json::from_str(&ev.data) {
            Ok(v) => v,
            Err(e) => {
                if name == "ping" {
                    return Ok(());
                }
                return Err(AttemptError::Fatal(ClientError::Stream(format!(
                    "bad JSON in {name} event: {e}"
                ))));
            }
        };
        let name = if name.is_empty() {
            v.get("type").and_then(Value::as_str).unwrap_or("")
        } else {
            name
        };
        let idx = v.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        match name {
            "message_start" => {
                if let Some(m) = v.get("message") {
                    if let Some(id) = m.get("id").and_then(Value::as_str) {
                        self.resp.id = id.to_string();
                    }
                    if let Some(model) = m.get("model").and_then(Value::as_str) {
                        self.resp.model = model.to_string();
                    }
                    if let Some(u) = m.get("usage") {
                        self.merge_usage(u);
                    }
                }
            }
            "content_block_start" => {
                let cb = v.get("content_block").cloned().unwrap_or(Value::Null);
                let block = serde_json::from_value::<ContentBlock>(cb).unwrap_or(ContentBlock::Other);
                *self.block(idx) = block;
            }
            "content_block_delta" => {
                let d = v.get("delta").cloned().unwrap_or(Value::Null);
                match d.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        let t = d.get("text").and_then(Value::as_str).unwrap_or("");
                        match self.block(idx) {
                            ContentBlock::Text { text, .. } => text.push_str(t),
                            b => {
                                *b = ContentBlock::Text {
                                    text: t.to_string(),
                                    cache_control: None,
                                }
                            }
                        }
                        if let Some(cb) = &self.on_text {
                            cb(t);
                        }
                    }
                    Some("thinking_delta") => {
                        let t = d.get("thinking").and_then(Value::as_str).unwrap_or("");
                        match self.block(idx) {
                            ContentBlock::Thinking { thinking, .. } => thinking.push_str(t),
                            b => {
                                *b = ContentBlock::Thinking {
                                    thinking: t.to_string(),
                                    signature: None,
                                }
                            }
                        }
                    }
                    Some("signature_delta") => {
                        let s = d.get("signature").and_then(Value::as_str).unwrap_or("");
                        if let ContentBlock::Thinking { signature, .. } = self.block(idx) {
                            *signature = Some(s.to_string());
                        }
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(d) = v.get("delta") {
                    if let Some(sr) = d.get("stop_reason").and_then(Value::as_str) {
                        self.resp.stop_reason = Some(sr.to_string());
                    }
                    if let Some(sd) = d.get("stop_details").filter(|x| !x.is_null()) {
                        if let Ok(sd) = serde_json::from_value::<StopDetails>(sd.clone()) {
                            self.resp.stop_details = Some(sd);
                        }
                    }
                }
                if let Some(u) = v.get("usage") {
                    self.merge_usage(u);
                }
            }
            "message_stop" => self.done = true,
            "error" => {
                let e = v.get("error").unwrap_or(&v);
                let kind = e.get("type").and_then(Value::as_str).unwrap_or("error").to_string();
                let message = e.get("message").and_then(Value::as_str).unwrap_or("").to_string();
                return Err(if kind == "invalid_request_error" {
                    AttemptError::Fatal(ClientError::Api {
                        status: 400,
                        kind,
                        message,
                    })
                } else {
                    AttemptError::Retry {
                        msg: format!("stream error {kind}: {message}"),
                        retry_after: None,
                    }
                });
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRANSCRIPT: &str = "event: message_start\r\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"m\",\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":5,\"output_tokens\":1}}}\r\n\r\n\
: comment\n\nevent: ping\ndata: {\"type\":\"ping\"}\n\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"hmm\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig\"}}\n\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n\
event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n\
event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":42}}\n\n\
event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

    #[test]
    fn parser_splits_events() {
        let mut p = SseParser::default();
        let evs = p.feed(TRANSCRIPT.as_bytes());
        assert_eq!(evs.len(), 11);
        assert_eq!(evs[0].event.as_deref(), Some("message_start"));
        assert_eq!(evs[1].event.as_deref(), Some("ping"));
        assert_eq!(evs.last().unwrap().event.as_deref(), Some("message_stop"));
    }

    #[test]
    fn parser_handles_arbitrary_chunking() {
        let mut p = SseParser::default();
        let mut n = 0;
        for chunk in TRANSCRIPT.as_bytes().chunks(3) {
            n += p.feed(chunk).len();
        }
        assert_eq!(n, 11);
    }

    #[test]
    fn assembler_builds_response() {
        let mut p = SseParser::default();
        let mut a = Assembler::new(None);
        for ev in p.feed(TRANSCRIPT.as_bytes()) {
            assert!(a.handle(&ev).is_ok());
        }
        assert!(a.done);
        let r = a.resp;
        assert_eq!(r.id, "msg_1");
        assert_eq!(r.text(), "Hello");
        assert_eq!(r.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(r.usage.input_tokens, 10);
        assert_eq!(r.usage.cache_read_input_tokens, 5);
        assert_eq!(r.usage.output_tokens, 42);
        assert_eq!(
            r.content[0],
            ContentBlock::Thinking {
                thinking: "hmm".into(),
                signature: Some("sig".into())
            }
        );
    }

    #[test]
    fn trailing_event_without_blank_line_is_flushed() {
        let mut p = SseParser::default();
        assert!(p.feed(b"event: message_stop\ndata: {}").is_empty());
        assert_eq!(p.finish().len(), 1);
    }
}
