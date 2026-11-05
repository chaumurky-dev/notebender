use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use notebender::claude::client::{Client, ClientError, SendOptions};
use notebender::claude::{ContentBlock, Message, MessagesRequest};
use notebender::models::Tier;

type Reply = Arc<dyn Fn(usize) -> Response + Send + Sync>;

#[derive(Clone)]
struct St {
    count: Arc<AtomicUsize>,
    seen: Arc<Mutex<Vec<(HeaderMap, String)>>>,
    reply: Reply,
}

async fn handler(State(st): State<St>, headers: HeaderMap, body: String) -> Response {
    let n = st.count.fetch_add(1, Ordering::SeqCst);
    st.seen.lock().unwrap().push((headers, body));
    (st.reply)(n)
}

async fn serve(reply: Reply) -> (String, St) {
    let st = St {
        count: Arc::new(AtomicUsize::new(0)),
        seen: Default::default(),
        reply,
    };
    let app = Router::new()
        .route("/v1/messages", post(handler))
        .with_state(st.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (url, st)
}

fn sse(stop: &str, extra_delta: &str) -> String {
    let mut s = String::new();
    s += "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude-x\",\"usage\":{\"input_tokens\":7,\"cache_creation_input_tokens\":2,\"output_tokens\":1}}}\n\n";
    for (i, t) in ["Hello ", "world", "!"].iter().enumerate() {
        let idx = i / 2;
        if i % 2 == 0 {
            s += &format!("event: content_block_start\ndata: {{\"type\":\"content_block_start\",\"index\":{idx},\"content_block\":{{\"type\":\"text\",\"text\":\"\"}}}}\n\n");
        }
        s += &format!("event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":{idx},\"delta\":{{\"type\":\"text_delta\",\"text\":\"{t}\"}}}}\n\n");
    }
    s += &format!("event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"{stop}\"{extra_delta}}},\"usage\":{{\"output_tokens\":9}}}}\n\n");
    s += "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
    s
}

fn ok(body: String) -> Response {
    Response::builder()
        .status(200)
        .header("content-type", "text/event-stream")
        .body(Body::from(body))
        .unwrap()
}

fn err(status: u16, kind: &str, msg: &str) -> Response {
    Response::builder()
        .status(StatusCode::from_u16(status).unwrap())
        .body(Body::from(format!(
            "{{\"type\":\"error\",\"error\":{{\"type\":\"{kind}\",\"message\":\"{msg}\"}}}}"
        )))
        .unwrap()
}

fn req() -> MessagesRequest {
    MessagesRequest {
        model: "claude-x".into(),
        max_tokens: 100,
        system: vec![],
        messages: vec![Message::user(vec![ContentBlock::text("hi")])],
        thinking: None,
        output_config: None,
        fallbacks: None,
        temperature: None,
        stream: None,
    }
}

fn client(url: &str, retries: u32, fallbacks: bool) -> Client {
    Client::from_parts("sk-test", url, retries, Duration::from_secs(10), fallbacks)
}

#[tokio::test]
async fn happy_path_and_fallbacks() {
    let (url, st) = serve(Arc::new(|_| ok(sse("end_turn", "")))).await;
    let c = client(&url, 2, true);
    assert!(!c.base_url().ends_with('/'));
    let got = Arc::new(Mutex::new(Vec::<String>::new()));
    let g2 = got.clone();
    let opts = SendOptions {
        tier: Some(Tier::Opus),
        on_text: Some(Arc::new(move |t| g2.lock().unwrap().push(t.to_string()))),
        extra_betas: vec!["foo-beta".into()],
    };
    let r = c.send(&req(), &opts).await.unwrap();
    assert_eq!(r.content.len(), 2);
    assert_eq!(r.text(), "Hello world!");
    assert_eq!(r.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(r.usage.input_tokens, 7);
    assert_eq!(r.usage.cache_creation_input_tokens, 2);
    assert_eq!(r.usage.output_tokens, 9);
    assert_eq!(*got.lock().unwrap(), vec!["Hello ", "world", "!"]);
    {
        let seen = st.seen.lock().unwrap();
        let (h, body) = &seen[0];
        assert!(body.contains("\"stream\":true"));
        assert!(body.contains("\"fallbacks\":\"default\""));
        assert_eq!(h["x-api-key"], "sk-test");
        assert_eq!(h["anthropic-version"], "2023-06-01");
        assert_eq!(h["anthropic-beta"], "foo-beta,server-side-fallback-2026-07-01");
    }
    // Haiku: no fallbacks.
    let opts = SendOptions {
        tier: Some(Tier::Haiku),
        ..Default::default()
    };
    c.send(&req(), &opts).await.unwrap();
    let seen = st.seen.lock().unwrap();
    let (h, body) = &seen[1];
    assert!(!body.contains("fallbacks"));
    assert!(h.get("anthropic-beta").is_none());
}

#[tokio::test]
async fn retries_overloaded_then_succeeds() {
    let (url, st) = serve(Arc::new(|n| {
        if n == 0 {
            err(529, "overloaded_error", "busy")
        } else {
            ok(sse("end_turn", ""))
        }
    }))
    .await;
    let r = client(&url, 3, false)
        .send(&req(), &SendOptions::default())
        .await
        .unwrap();
    assert_eq!(r.text(), "Hello world!");
    assert_eq!(st.count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn bad_request_is_not_retried() {
    let (url, st) = serve(Arc::new(|_| err(400, "invalid_request_error", "bad field"))).await;
    let e = client(&url, 3, false)
        .send(&req(), &SendOptions::default())
        .await
        .unwrap_err();
    match e {
        ClientError::Api { status, kind, message } => {
            assert_eq!(status, 400);
            assert_eq!(kind, "invalid_request_error");
            assert_eq!(message, "bad field");
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(st.count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn refusal_maps_to_error() {
    let (url, _) = serve(Arc::new(|_| {
        ok(sse(
            "refusal",
            ",\"stop_details\":{\"type\":\"refusal\",\"category\":\"cyber\",\"explanation\":\"nope\"}",
        ))
    }))
    .await;
    let e = client(&url, 0, false)
        .send(&req(), &SendOptions::default())
        .await
        .unwrap_err();
    match e {
        ClientError::Refusal { category, explanation } => {
            assert_eq!(category.as_deref(), Some("cyber"));
            assert_eq!(explanation.as_deref(), Some("nope"));
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn chunked_sse_parses() {
    let (url, _) = serve(Arc::new(|_| {
        let bytes = sse("end_turn", "").into_bytes();
        let chunks: Vec<Result<Vec<u8>, std::io::Error>> = bytes.chunks(5).map(|c| Ok(c.to_vec())).collect();
        Response::builder()
            .status(200)
            .body(Body::from_stream(futures_util::stream::iter(chunks)))
            .unwrap()
    }))
    .await;
    let r = client(&url, 0, false)
        .send(&req(), &SendOptions::default())
        .await
        .unwrap();
    assert_eq!(r.text(), "Hello world!");
    assert_eq!(r.usage.output_tokens, 9);
}
