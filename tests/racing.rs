//! Real-proxy regression coverage for opt-in same-model request racing.
//! Attempt-specific gates make completion order explicit; dropped body guards
//! prove cancellation, rather than inferring it from a quick client response.

#[allow(dead_code)]
mod support;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::sync::{Notify, Semaphore};
use tokio::task::JoinHandle;

use support::{chat_body, login, metrics, start_proxy_with, Proxy, StoreOpts};

const ORIGINAL: &str = "mock/model-a";
const SLOW: &str = "first-attempt";
const FAST: &str = "second-attempt";
const THIRD: &str = "third-attempt";

#[derive(Clone)]
struct AttemptReply {
    status: StatusCode,
    content_type: &'static str,
    before: Vec<Bytes>,
    gate: Option<Arc<Semaphore>>,
    after: Vec<Bytes>,
    hang: bool,
}

impl AttemptReply {
    fn json(attempt: &str) -> Self {
        Self {
            status: StatusCode::OK,
            content_type: "application/json",
            before: Vec::new(),
            gate: None,
            after: vec![Bytes::from(
                json!({
                    "id": format!("reply-{attempt}"), "model": ORIGINAL,
                    "object": "chat.completion",
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": attempt}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
                })
                .to_string(),
            )],
            hang: false,
        }
    }

    fn gated(attempt: &str) -> (Self, Arc<Semaphore>) {
        let gate = Arc::new(Semaphore::new(0));
        let mut reply = Self::json(attempt);
        reply.gate = Some(gate.clone());
        (reply, gate)
    }

    fn failure() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            content_type: "application/json",
            before: Vec::new(),
            gate: None,
            after: vec![Bytes::from_static(
                b"{\"error\":{\"message\":\"scripted non-retryable failure\"}}",
            )],
            hang: false,
        }
    }

    fn raw_json(body: impl Into<Bytes>) -> Self {
        let mut reply = Self::json("raw-json");
        reply.after = vec![body.into()];
        reply
    }

    fn sse(body: impl Into<Bytes>) -> Self {
        Self {
            status: StatusCode::OK,
            content_type: "text/event-stream",
            before: Vec::new(),
            gate: None,
            after: vec![body.into()],
            hang: false,
        }
    }
}

#[derive(Default)]
struct Observations {
    // Attempt labels belong to the mock script, never the upstream model.
    calls: Vec<(String, String)>,
    bodies: Vec<Value>,
    raw_bodies: Vec<Bytes>,
    started: Vec<tokio::time::Instant>,
    emitted: HashMap<String, usize>,
    cancelled: HashMap<String, usize>,
}

struct MockState {
    replies: Vec<(String, AttemptReply)>,
    observations: Mutex<Observations>,
    changed: Notify,
}

struct AttemptMock {
    url: String,
    state: Arc<MockState>,
    server: JoinHandle<()>,
}

impl Drop for AttemptMock {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl AttemptMock {
    async fn start(replies: Vec<(&str, AttemptReply)>) -> Self {
        let state = Arc::new(MockState {
            replies: replies
                .into_iter()
                .map(|(name, reply)| (name.to_owned(), reply))
                .collect(),
            observations: Mutex::new(Observations::default()),
            changed: Notify::new(),
        });
        let app = Router::new()
            .route("/v1/chat/completions", post(mock_chat))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { url, state, server }
    }

    async fn wait_for(&self, label: &str, predicate: impl Fn(&Observations) -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
        loop {
            let changed = self.state.changed.notified();
            if predicate(&self.state.observations.lock().unwrap()) {
                return;
            }
            assert!(
                tokio::time::timeout_at(deadline, changed).await.is_ok(),
                "mock did not observe {label}; calls: {:?}",
                self.state.observations.lock().unwrap().calls
            );
        }
    }

    async fn called(&self, attempts: &[&str]) {
        self.wait_for("all expected attempt calls", |seen| {
            attempts.iter().all(|attempt| {
                seen.calls
                    .iter()
                    .any(|(called, _)| called.as_str() == *attempt)
            })
        })
        .await;
    }

    async fn cancelled(&self, attempt: &str) {
        self.wait_for("cancelled upstream body", |seen| {
            seen.cancelled.get(attempt).copied().unwrap_or_default() > 0
        })
        .await;
    }

    fn calls(&self) -> Vec<String> {
        let seen = self.state.observations.lock().unwrap();
        for body in &seen.bodies {
            assert_eq!(body["model"], ORIGINAL, "attempt changed requested model");
        }
        seen.calls
            .iter()
            .map(|(attempt, _)| attempt.clone())
            .collect()
    }
}

struct BodyGuard {
    state: Arc<MockState>,
    attempt: String,
    complete: bool,
}

impl Drop for BodyGuard {
    fn drop(&mut self) {
        if !self.complete {
            *self
                .state
                .observations
                .lock()
                .unwrap()
                .cancelled
                .entry(self.attempt.clone())
                .or_default() += 1;
            self.state.changed.notify_one();
        }
    }
}

struct ReplyBody {
    guard: BodyGuard,
    before: VecDeque<Bytes>,
    gate: Option<Arc<Semaphore>>,
    after: VecDeque<Bytes>,
    hang: bool,
}

async fn mock_chat(
    State(state): State<Arc<MockState>>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Response {
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let key = headers
        .get(header::AUTHORIZATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let (attempt, reply) = {
        let mut seen = state.observations.lock().unwrap();
        let (attempt, reply) = state
            .replies
            .get(seen.calls.len())
            .cloned()
            .unwrap_or_else(|| ("unexpected-attempt".to_owned(), AttemptReply::failure()));
        seen.calls.push((attempt.clone(), key));
        seen.bodies.push(body);
        seen.raw_bodies.push(bytes);
        seen.started.push(tokio::time::Instant::now());
        (attempt, reply)
    };
    state.changed.notify_one();
    let response_body = ReplyBody {
        guard: BodyGuard {
            state,
            attempt,
            complete: false,
        },
        before: reply.before.into(),
        gate: reply.gate,
        after: reply.after.into(),
        hang: reply.hang,
    };
    let stream = futures_util::stream::unfold(response_body, |mut body| async move {
        let next = if let Some(bytes) = body.before.pop_front() {
            Some(bytes)
        } else {
            if let Some(gate) = body.gate.take() {
                gate.acquire_owned().await.unwrap().forget();
            }
            body.after.pop_front()
        };
        if let Some(bytes) = next {
            *body
                .guard
                .state
                .observations
                .lock()
                .unwrap()
                .emitted
                .entry(body.guard.attempt.clone())
                .or_default() += 1;
            body.guard.state.changed.notify_one();
            Some((Ok::<_, std::io::Error>(bytes), body))
        } else if body.hang {
            std::future::pending().await
        } else {
            body.guard.complete = true;
            drop(body);
            None
        }
    });
    Response::builder()
        .status(reply.status)
        .header(header::CONTENT_TYPE, reply.content_type)
        .body(Body::from_stream(stream))
        .unwrap()
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .unwrap()
}

async fn configure(proxy: &Proxy, strategy: Option<Value>) {
    let cookie = login(proxy).await;
    let response = client()
        .post(proxy.url("/api/racing/config"))
        .header(header::COOKIE, cookie)
        .json(&json!({"enabled": strategy.is_some(), "strategy": strategy}))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(status, StatusCode::OK, "racing configuration: {body}");
}

fn same_model(attempts: usize, hedge_delay_ms: u64) -> Value {
    json!({"type": "same_model", "attempts": attempts, "hedge_delay_ms": hedge_delay_ms})
}

fn request(
    proxy: &Proxy,
    stream: bool,
    deadline_ms: Option<u64>,
) -> JoinHandle<(StatusCode, String)> {
    let url = proxy.url("/v1/chat/completions");
    tokio::spawn(async move {
        let mut request = client()
            .post(url)
            .json(&chat_body("racing regression", stream));
        if let Some(deadline) = deadline_ms {
            request = request.header("x-nim-proxy-deadline-ms", deadline.to_string());
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        (status, response.text().await.unwrap())
    })
}

fn winner(status: StatusCode, body: &str, attempt: &str) {
    assert_eq!(status, StatusCode::OK, "winner response: {body}");
    let body: Value = serde_json::from_str(body).unwrap();
    assert_eq!(body["model"], ORIGINAL);
    assert_eq!(body["choices"][0]["message"]["content"], attempt);
}

async fn permits_released(proxy: &Proxy) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    loop {
        let text = metrics(proxy).await;
        if text.contains(&format!(
            "nimproxy_model_inflight{{model=\"{ORIGINAL}\"}} 0"
        )) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "model permits were not released: {text}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn disabled_racing_preserves_requested_model_and_body() {
    let mock = AttemptMock::start(vec![
        (ORIGINAL, AttemptReply::json(ORIGINAL)),
        (FAST, AttemptReply::json(FAST)),
    ])
    .await;
    let proxy = start_proxy_with(&mock.url, StoreOpts::default(), &[]).await;
    configure(&proxy, Some(same_model(1, 0))).await;
    configure(&proxy, None).await;
    let (status, body) = request(&proxy, false, None).await.unwrap();
    winner(status, &body, ORIGINAL);
    assert_eq!(mock.calls(), [ORIGINAL]);
    assert_eq!(
        mock.state.observations.lock().unwrap().bodies,
        [chat_body("racing regression", false)]
    );
}

#[tokio::test]
async fn fastest_success_wins_and_drops_the_slow_upstream() {
    let (slow, _slow_gate) = AttemptReply::gated(SLOW);
    let (fast, fast_gate) = AttemptReply::gated(FAST);
    let mock = AttemptMock::start(vec![(SLOW, slow), (FAST, fast)]).await;
    let proxy = start_proxy_with(&mock.url, StoreOpts::default(), &[]).await;
    configure(&proxy, Some(same_model(2, 0))).await;
    let response = request(&proxy, false, None);
    mock.called(&[SLOW, FAST]).await;
    fast_gate.add_permits(1);
    let (status, body) = response.await.unwrap();
    winner(status, &body, FAST);
    mock.cancelled(SLOW).await;
    permits_released(&proxy).await;
}

#[tokio::test]
async fn racing_preserves_rich_request_bytes_and_uses_distinct_ready_keys() {
    let (first, _first_gate) = AttemptReply::gated(SLOW);
    let (second, _second_gate) = AttemptReply::gated(FAST);
    let (third, third_gate) = AttemptReply::gated(THIRD);
    let mock = AttemptMock::start(vec![(SLOW, first), (FAST, second), (THIRD, third)]).await;
    let proxy = start_proxy_with(&mock.url, StoreOpts::default(), &[]).await;
    configure(&proxy, Some(same_model(3, 0))).await;
    let payload = json!({
        "model": ORIGINAL,
        "messages": [
            {"role": "system", "content": "Keep all request semantics."},
            {"role": "user", "content": [{"type": "text", "text": "weather?"}]},
            {"role": "assistant", "content": null, "tool_calls": [{"id": "previous", "type": "function", "function": {"name": "weather", "arguments": "{\"city\":\"Paris\"}"}}]},
            {"role": "tool", "tool_call_id": "previous", "content": "sunny"}
        ],
        "tools": [{"type": "function", "function": {"name": "weather", "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}}}],
        "tool_choice": "auto", "parallel_tool_calls": true,
        "temperature": 0.37, "top_p": 0.83, "seed": 17, "max_tokens": 41,
        "stop": ["END"], "presence_penalty": 0.2, "frequency_penalty": 0.4,
        "stream": false
    });
    let wire = format!("  {}\n", serde_json::to_string_pretty(&payload).unwrap());
    let request_wire = wire.clone();
    let url = proxy.url("/v1/chat/completions");
    let response = tokio::spawn(async move {
        client()
            .post(url)
            .header(header::CONTENT_TYPE, "application/json")
            .body(request_wire)
            .send()
            .await
            .unwrap()
    });
    mock.called(&[SLOW, FAST, THIRD]).await;
    {
        let seen = mock.state.observations.lock().unwrap();
        assert_eq!(seen.bodies, vec![payload; 3]);
        assert!(seen
            .raw_bodies
            .iter()
            .all(|body| body.as_ref() == wire.as_bytes()));
        let mut keys: Vec<_> = seen.calls.iter().map(|(_, key)| key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(
            keys.len(),
            3,
            "spare attempts must prefer unused ready keys"
        );
    }
    third_gate.add_permits(1);
    let response = response.await.unwrap();
    winner(response.status(), &response.text().await.unwrap(), THIRD);
    mock.cancelled(SLOW).await;
    mock.cancelled(FAST).await;
    permits_released(&proxy).await;
}

#[tokio::test]
async fn racing_preserves_rich_streaming_request_bytes_for_every_attempt() {
    let winner_wire = "data: {\"choices\":[{\"delta\":{\"content\":\"streaming winner\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    let mut first = AttemptReply::sse(winner_wire);
    first.gate = Some(Arc::new(Semaphore::new(0)));
    let gate = Arc::new(Semaphore::new(0));
    let mut second = AttemptReply::sse(winner_wire);
    second.gate = Some(gate.clone());
    let mock = AttemptMock::start(vec![(SLOW, first), (FAST, second)]).await;
    let proxy = start_proxy_with(&mock.url, StoreOpts::default(), &[]).await;
    configure(&proxy, Some(same_model(2, 0))).await;
    let payload = json!({
        "model": ORIGINAL,
        "messages": [
            {"role": "system", "content": "Keep the streaming request intact."},
            {"role": "user", "content": [{"type": "text", "text": "weather?"}]},
            {"role": "assistant", "content": null, "tool_calls": [{"id": "previous", "type": "function", "function": {"name": "weather", "arguments": "{\"city\":\"Paris\"}"}}]},
            {"role": "tool", "tool_call_id": "previous", "content": "sunny"}
        ],
        "tools": [{"type": "function", "function": {"name": "weather", "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}}}],
        "tool_choice": "auto", "parallel_tool_calls": true,
        "temperature": 0.37, "top_p": 0.83, "seed": 17, "max_tokens": 41,
        "stop": ["END"], "presence_penalty": 0.2, "frequency_penalty": 0.4,
        "stream": true,
        // Existing stream_options must prevent injection or reserialization.
        "stream_options": {"include_usage": true}
    });
    let wire = format!("  {}\n", serde_json::to_string_pretty(&payload).unwrap());
    let response = client()
        .post(proxy.url("/v1/chat/completions"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(wire.clone())
        .send()
        .await
        .unwrap();
    mock.called(&[SLOW, FAST]).await;
    {
        let seen = mock.state.observations.lock().unwrap();
        assert_eq!(seen.bodies, vec![payload; 2]);
        assert!(seen
            .raw_bodies
            .iter()
            .all(|body| body.as_ref() == wire.as_bytes()));
        assert_ne!(
            seen.calls[0].1, seen.calls[1].1,
            "streaming copies should prefer different ready keys"
        );
    }
    gate.add_permits(1);
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(
        body.contains(winner_wire),
        "winning stream bytes moved: {body}"
    );
    mock.cancelled(SLOW).await;
    permits_released(&proxy).await;
}

#[tokio::test]
async fn racing_usage_rejection_retries_the_original_bytes_and_consumes_another_rpm_slot() {
    let winner_wire = "data: {\"choices\":[{\"delta\":{\"content\":\"untouched retry\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    let mock = AttemptMock::start(vec![
        (SLOW, AttemptReply::failure()),
        (FAST, AttemptReply::sse(winner_wire)),
    ])
    .await;
    let proxy = start_proxy_with(
        &mock.url,
        StoreOpts {
            nim_keys: vec![("usage-retry-key".into(), 2)],
            ..StoreOpts::default()
        },
        &[],
    )
    .await;
    // One configured contender must still perform its injection retry. A
    // second contender cannot hide a missing retry, and both sends cost RPM.
    configure(&proxy, Some(same_model(1, 0))).await;
    let payload = json!({
        "model": ORIGINAL, "stream": true,
        "messages": [
            {"role": "system", "content": "Preserve tool and sampling semantics."},
            {"role": "user", "content": "weather?"}
        ],
        "tools": [{"type": "function", "function": {"name": "weather", "parameters": {"type": "object"}}}],
        "tool_choice": "auto", "temperature": 0.37, "top_p": 0.83,
        "seed": 17, "max_tokens": 41, "stop": ["END"]
    });
    let wire = format!("  {}\n", serde_json::to_string_pretty(&payload).unwrap());
    let response = client()
        .post(proxy.url("/v1/chat/completions"))
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-nim-proxy-deadline-ms", "2000")
        .body(wire.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(
        body.contains(winner_wire),
        "injection fallback did not succeed: {body}"
    );
    assert!(!body.contains("race_failed"));
    {
        let seen = mock.state.observations.lock().unwrap();
        let mut injected = payload.clone();
        injected["stream_options"] = json!({"include_usage": true});
        assert_eq!(seen.bodies, vec![injected, payload]);
        assert_eq!(
            seen.raw_bodies[1].as_ref(),
            wire.as_bytes(),
            "the 400 fallback must use the untouched client body"
        );
        assert!(seen
            .calls
            .iter()
            .all(|(_, key)| key == "Bearer usage-retry-key"));
    }
    permits_released(&proxy).await;
    configure(&proxy, None).await;
    let (status, body) = request(&proxy, false, Some(150)).await.unwrap();
    assert_eq!(
        status,
        StatusCode::GATEWAY_TIMEOUT,
        "both injection and fallback sends consume RPM: {body}"
    );
    assert_eq!(
        mock.calls(),
        [SLOW, FAST],
        "the retry must not bypass or refund an RPM reservation"
    );
    permits_released(&proxy).await;
}

#[tokio::test]
async fn single_ready_key_can_send_multiple_rpm_admitted_attempts() {
    let (first, _first_gate) = AttemptReply::gated(SLOW);
    let (second, second_gate) = AttemptReply::gated(FAST);
    let mock = AttemptMock::start(vec![(SLOW, first), (FAST, second)]).await;
    let proxy = start_proxy_with(
        &mock.url,
        StoreOpts {
            nim_keys: vec![("single-hedge-key".into(), 2)],
            ..StoreOpts::default()
        },
        &[],
    )
    .await;
    configure(&proxy, Some(same_model(2, 0))).await;
    let response = request(&proxy, false, Some(2000));
    mock.called(&[SLOW, FAST]).await;
    assert!(mock
        .state
        .observations
        .lock()
        .unwrap()
        .calls
        .iter()
        .all(|(_, key)| key == "Bearer single-hedge-key"));
    second_gate.add_permits(1);
    let (status, body) = response.await.unwrap();
    winner(status, &body, FAST);
    mock.cancelled(SLOW).await;
    configure(&proxy, None).await;
    let (status, body) = request(&proxy, false, Some(150)).await.unwrap();
    assert_eq!(
        status,
        StatusCode::GATEWAY_TIMEOUT,
        "two sends exhaust the two-RPM key: {body}"
    );
    assert_eq!(
        mock.calls().len(),
        2,
        "cancelling the first attempt cannot refund its sent slot"
    );
    permits_released(&proxy).await;
}

#[tokio::test]
async fn failed_candidate_does_not_beat_a_later_success() {
    let (fast, fast_gate) = AttemptReply::gated(FAST);
    let mock = AttemptMock::start(vec![(SLOW, AttemptReply::failure()), (FAST, fast)]).await;
    let proxy = start_proxy_with(&mock.url, StoreOpts::default(), &[]).await;
    configure(&proxy, Some(same_model(2, 0))).await;
    let response = request(&proxy, false, None);
    mock.called(&[SLOW, FAST]).await;
    fast_gate.add_permits(1);
    let (status, body) = response.await.unwrap();
    winner(status, &body, FAST);
    assert_eq!(
        mock.calls().len(),
        2,
        "non-retryable failure is not retried"
    );
}

#[tokio::test]
async fn timely_primary_does_not_start_spare_attempts() {
    let (primary, gate) = AttemptReply::gated(SLOW);
    let mock = AttemptMock::start(vec![(SLOW, primary), (FAST, AttemptReply::json(FAST))]).await;
    let proxy = start_proxy_with(&mock.url, StoreOpts::default(), &[]).await;
    configure(&proxy, Some(same_model(2, 300))).await;
    let response = request(&proxy, false, None);
    mock.called(&[SLOW]).await;
    gate.add_permits(1);
    let (status, body) = response.await.unwrap();
    winner(status, &body, SLOW);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(mock.calls(), [SLOW]);
}

#[tokio::test]
async fn delayed_hedge_wins_and_drops_pending_primary() {
    let (primary, _gate) = AttemptReply::gated(SLOW);
    let mock = AttemptMock::start(vec![(SLOW, primary), (FAST, AttemptReply::json(FAST))]).await;
    let proxy = start_proxy_with(&mock.url, StoreOpts::default(), &[]).await;
    configure(&proxy, Some(same_model(2, 150))).await;
    let response = request(&proxy, false, None);
    mock.called(&[SLOW, FAST]).await;
    let (status, body) = response.await.unwrap();
    winner(status, &body, FAST);
    mock.cancelled(SLOW).await;
    permits_released(&proxy).await;
}

#[tokio::test]
async fn original_attempt_can_still_win_after_delayed_hedge_starts() {
    let (primary, primary_gate) = AttemptReply::gated(SLOW);
    let (hedge, _hedge_gate) = AttemptReply::gated(FAST);
    let mock = AttemptMock::start(vec![(SLOW, primary), (FAST, hedge)]).await;
    let proxy = start_proxy_with(&mock.url, StoreOpts::default(), &[]).await;
    configure(&proxy, Some(same_model(2, 150))).await;
    let response = request(&proxy, false, None);
    mock.called(&[SLOW, FAST]).await;
    assert_eq!(
        mock.state.observations.lock().unwrap().cancelled.get(SLOW),
        None,
        "the original must remain eligible after launching the hedge"
    );
    primary_gate.add_permits(1);
    let (status, body) = response.await.unwrap();
    winner(status, &body, SLOW);
    mock.cancelled(FAST).await;
    permits_released(&proxy).await;
}

#[tokio::test]
async fn delayed_attempts_launch_one_at_a_time_while_previous_attempts_remain_pending() {
    let (first, first_gate) = AttemptReply::gated(SLOW);
    let (second, _second_gate) = AttemptReply::gated(FAST);
    let (third, _third_gate) = AttemptReply::gated(THIRD);
    let mock = AttemptMock::start(vec![(SLOW, first), (FAST, second), (THIRD, third)]).await;
    let proxy = start_proxy_with(&mock.url, StoreOpts::default(), &[]).await;
    configure(&proxy, Some(same_model(3, 300))).await;
    let response = request(&proxy, false, None);
    mock.called(&[SLOW, FAST, THIRD]).await;
    {
        let seen = mock.state.observations.lock().unwrap();
        assert!(
            seen.cancelled.is_empty(),
            "pending originals must remain competitors"
        );
        for pair in seen.started.windows(2) {
            assert!(
                pair[1].duration_since(pair[0]) >= Duration::from_millis(200),
                "a delayed launch must not fan out all remaining attempts at once"
            );
        }
    }
    first_gate.add_permits(1);
    let (status, body) = response.await.unwrap();
    winner(status, &body, SLOW);
    mock.cancelled(FAST).await;
    mock.cancelled(THIRD).await;
    permits_released(&proxy).await;
}

#[tokio::test]
async fn primary_failure_starts_spare_before_hedge_delay() {
    let mock = AttemptMock::start(vec![
        (SLOW, AttemptReply::failure()),
        (FAST, AttemptReply::json(FAST)),
    ])
    .await;
    let proxy = start_proxy_with(&mock.url, StoreOpts::default(), &[]).await;
    configure(&proxy, Some(same_model(2, 4000))).await;
    let (status, body) = tokio::time::timeout(Duration::from_secs(2), request(&proxy, false, None))
        .await
        .expect("a failed primary must not wait for its four-second hedge delay")
        .unwrap();
    winner(status, &body, FAST);
    assert_eq!(mock.calls(), [SLOW, FAST]);
}

#[tokio::test]
async fn buffered_invalid_or_ineffective_json_cannot_beat_a_valid_completion() {
    for invalid in [
        "not json".to_owned(),
        "[]".to_owned(),
        json!({"error": {"message": "private provider failure"}}).to_string(),
        json!({"choices": []}).to_string(),
        json!({"choices": [{"message": {"role": "assistant", "content": "unfinished"}}]}).to_string(),
        json!({"choices": [{"message": {"role": "assistant", "content": "unfinished"}, "finish_reason": null}]}).to_string(),
        json!({"choices": [{"message": {"role": "assistant", "content": "unfinished"}, "finish_reason": ""}]}).to_string(),
        json!({"choices": [{"message": {"role": "assistant", "content": ""}, "finish_reason": "stop"}]}).to_string(),
        json!({"choices": [{"message": {"role": "user", "content": "wrong role"}, "finish_reason": "stop"}]}).to_string(),
        json!({"choices": [{"message": {"role": "assistant", "tool_calls": [{"id": "call-1"}]}, "finish_reason": "tool_calls"}]}).to_string(),
        json!({"choices": [{"message": {"role": "assistant", "function_call": {"name": "weather"}}, "finish_reason": "function_call"}]}).to_string(),
        json!({"choices": [{"message": {"role": "assistant", "content": "Calling weather now", "tool_calls": [{"id": "call-1"}]}, "finish_reason": "tool_calls"}]}).to_string(),
        json!({"choices": [{"message": {"role": "assistant", "reasoning_content": "Check weather", "function_call": {"name": "weather"}}, "finish_reason": "function_call"}]}).to_string(),
        json!({"error": {"message": "private provider failure"}, "choices": [{"message": {"role": "assistant", "content": "error cannot win"}, "finish_reason": "stop"}]}).to_string(),
    ] {
        let (valid, gate) = AttemptReply::gated(FAST);
        let mock = AttemptMock::start(vec![(SLOW, AttemptReply::raw_json(invalid.clone())), (FAST, valid)]).await;
        let proxy = start_proxy_with(&mock.url, StoreOpts::default(), &[]).await;
        configure(&proxy, Some(same_model(2, 4000))).await;
        let response = request(&proxy, false, Some(2000));
        mock.called(&[SLOW, FAST]).await;
        gate.add_permits(1);
        let (status, body) = response.await.unwrap();
        winner(status, &body, FAST);
        assert!(!body.contains("private provider failure"), "loser error escaped for {invalid}: {body}");
        permits_released(&proxy).await;
    }
}

#[tokio::test]
async fn buffered_completed_content_reasoning_tools_and_refusal_are_valid() {
    for message in [
        json!({"role": "assistant", "content": "answer"}),
        json!({"role": "assistant", "content": "answer", "tool_calls": null, "function_call": null}),
        json!({"role": "assistant", "content": "answer", "tool_calls": []}),
        json!({"role": "assistant", "content": null, "reasoning_content": "reasoning"}),
        json!({"role": "assistant", "content": null, "tool_calls": [{"id": "call-1", "type": "function", "function": {"name": "weather", "arguments": "{}"}}]}),
        json!({"role": "assistant", "content": null, "function_call": {"name": "weather", "arguments": "{}"}}),
        json!({"role": "assistant", "content": null, "refusal": "cannot fulfill this request"}),
    ] {
        let wire =
            json!({"model": ORIGINAL, "choices": [{"message": message, "finish_reason": "stop"}]})
                .to_string();
        let mock = AttemptMock::start(vec![(SLOW, AttemptReply::raw_json(wire.clone()))]).await;
        let proxy = start_proxy_with(&mock.url, StoreOpts::default(), &[]).await;
        configure(&proxy, Some(same_model(1, 0))).await;
        let (status, body) = request(&proxy, false, None).await.unwrap();
        assert_eq!(
            status,
            StatusCode::OK,
            "completed assistant message rejected: {wire}"
        );
        assert_eq!(body, wire, "winner response bytes must be preserved");
        permits_released(&proxy).await;
    }
}

async fn semantic_stream_winner(delta: Value, strategy: Value) {
    let loser_initial = ": loser-preface\n\ndata: {\"id\":\"loser-role\",\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n";
    let winner_initial = ": winner-preface\n\ndata: {\"id\":\"winner-role\",\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n";
    let token = format!(
        "data: {}\n\n",
        json!({"id": "winner-token", "choices": [{"index": 0, "delta": delta}]})
    );
    let tail = "data: {\"id\":\"winner-tail\",\"choices\":[{\"delta\":{\"content\":\"after winner\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    let gate = Arc::new(Semaphore::new(0));
    let slow = AttemptReply {
        status: StatusCode::OK,
        content_type: "text/event-stream",
        before: vec![Bytes::from(loser_initial)],
        gate: None,
        after: Vec::new(),
        hang: true,
    };
    // Split the winning JSON across transport chunks as well as preserving
    // the earlier role-only event. A network chunk is not an SSE event/token.
    let split = token.len() / 2;
    let fast = AttemptReply {
        status: StatusCode::OK,
        content_type: "text/event-stream",
        before: vec![Bytes::from(winner_initial)],
        gate: Some(gate.clone()),
        after: vec![
            Bytes::copy_from_slice(&token.as_bytes()[..split]),
            Bytes::copy_from_slice(&token.as_bytes()[split..]),
            Bytes::from(tail),
        ],
        hang: false,
    };
    let mock = AttemptMock::start(vec![(SLOW, slow), (FAST, fast)]).await;
    let proxy = start_proxy_with(&mock.url, StoreOpts::default(), &[]).await;
    configure(&proxy, Some(strategy)).await;
    let response = request(&proxy, true, None);
    mock.wait_for("both role-only prefixes", |seen| {
        [SLOW, FAST]
            .iter()
            .all(|attempt| seen.emitted.get(*attempt).copied().unwrap_or_default() >= 1)
    })
    .await;
    gate.add_permits(1);
    let (status, body) = response.await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(&format!("{winner_initial}{token}{tail}")),
        "the complete winning stream must survive selection byte-for-byte: {body}"
    );
    assert!(!body.contains("loser-"), "loser bytes escaped: {body}");
    mock.cancelled(SLOW).await;
    permits_released(&proxy).await;
    let text = metrics(&proxy).await;
    assert!(
        text.contains("nimproxy_race_cancelled_total{reason=\"winner_found\"} 1"),
        "same-model selection must count exactly one cancelled contender: {text}"
    );
    assert!(
        text.contains("nimproxy_race_requests_total{strategy=\"same_model\"} 1"),
        "same-model racing must identify its strategy: {text}"
    );
}

#[tokio::test]
async fn stream_ignores_comments_and_empty_roles_until_content() {
    semantic_stream_winner(json!({"content": "answer"}), same_model(2, 0)).await;
}

#[tokio::test]
async fn stream_accepts_reasoning_as_first_meaningful_delta() {
    semantic_stream_winner(json!({"reasoning_content": "thinking"}), same_model(2, 0)).await;
}

#[tokio::test]
async fn stream_accepts_tool_call_as_first_meaningful_delta() {
    semantic_stream_winner(
        json!({"tool_calls": [{"index": 0, "id": "call-1", "type": "function", "function": {"name": "weather", "arguments": "{"}}]}),
        same_model(2, 0),
    )
    .await;
}

#[tokio::test]
async fn delayed_streaming_also_waits_for_meaningful_content() {
    semantic_stream_winner(json!({"content": "answer"}), same_model(2, 150)).await;
}

#[tokio::test]
async fn stream_terminal_or_invalid_prefix_cannot_beat_valid_content() {
    for invalid in [
        "data: {\"error\":{\"message\":\"private provider failure\"}}\n\n",
        "event: error\ndata: {\"message\":\"private provider failure\"}\n\n",
        "data: [DONE]\n\n",
        "data: {not valid json}\n\n",
        "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
    ] {
        let gate = Arc::new(Semaphore::new(0));
        let mut slow = AttemptReply::sse(invalid);
        slow.hang = true;
        let mut fast = AttemptReply::sse(
            "data: {\"choices\":[{\"delta\":{\"content\":\"valid winner\"}}]}\n\ndata: [DONE]\n\n",
        );
        fast.gate = Some(gate.clone());
        let mock = AttemptMock::start(vec![(SLOW, slow), (FAST, fast)]).await;
        let proxy = start_proxy_with(&mock.url, StoreOpts::default(), &[]).await;
        configure(
            &proxy,
            Some(same_model(2, 0)),
        )
        .await;
        let response = request(&proxy, true, None);
        mock.called(&[SLOW, FAST]).await;
        // Cancellation before opening the successful body proves rejection
        // happens at the terminal event, not only after another attempt wins.
        mock.cancelled(SLOW).await;
        gate.add_permits(1);
        let (status, body) = response.await.unwrap();
        assert_eq!(status, StatusCode::OK, "candidate prefix: {invalid}");
        assert!(body.contains("valid winner"), "candidate prefix: {invalid}; response: {body}");
        assert!(!body.contains("private provider failure"), "loser error escaped: {body}");
        permits_released(&proxy).await;
    }
}

#[tokio::test]
async fn all_failed_streams_return_one_terminal_race_error() {
    let mock = AttemptMock::start(vec![
        (
            SLOW,
            AttemptReply::sse("data: {\"error\":{\"message\":\"provider failure\"}}\n\n"),
        ),
        (FAST, AttemptReply::sse("data: [DONE]\n\n")),
    ])
    .await;
    let proxy = start_proxy_with(&mock.url, StoreOpts::default(), &[]).await;
    configure(&proxy, Some(same_model(2, 0))).await;
    let (status, body) = request(&proxy, true, None).await.unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "stream has already committed: {body}"
    );
    assert_eq!(body.matches("race_failed").count(), 1, "all failed: {body}");
    assert!(
        !body.contains("provider failure"),
        "loser error escaped: {body}"
    );
    permits_released(&proxy).await;
}

#[tokio::test]
async fn stream_rejects_no_space_error_event_with_content_shaped_data() {
    let mock = AttemptMock::start(vec![(
        SLOW,
        AttemptReply::sse(
            "event:error\ndata: {\"choices\":[{\"delta\":{\"content\":\"must not win\"}}]}\n\n",
        ),
    )])
    .await;
    let proxy = start_proxy_with(&mock.url, StoreOpts::default(), &[]).await;
    configure(&proxy, Some(same_model(1, 0))).await;
    let (status, body) = request(&proxy, true, None).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("race_failed"),
        "error events cannot win: {body}"
    );
    assert!(
        !body.contains("must not win"),
        "error payload escaped: {body}"
    );
}

#[tokio::test]
async fn stream_accepts_cr_only_event_framing() {
    let wire = "data: {\"choices\":[{\"delta\":{\"content\":\"CR winner\"}}]}\r\rdata: [DONE]\r\r";
    let mock = AttemptMock::start(vec![(FAST, AttemptReply::sse(wire))]).await;
    let proxy = start_proxy_with(&mock.url, StoreOpts::default(), &[]).await;
    configure(&proxy, Some(same_model(1, 0))).await;
    let (status, body) = request(&proxy, true, None).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(wire), "CR is a valid SSE line ending: {body}");
    assert!(!body.contains("race_failed"));
}

#[tokio::test]
async fn stream_prefix_bound_does_not_reject_bytes_after_winning_event() {
    let prefix = format!(": {}\n\n", "p".repeat(63 * 1024));
    let token = "data: {\"choices\":[{\"delta\":{\"content\":\"bounded winner\"}}]}\n\n";
    let tail = format!(": {}\n\ndata: [DONE]\n\n", "t".repeat(4096));
    let gate = Arc::new(Semaphore::new(0));
    let mut fast = AttemptReply::sse(format!("{token}{tail}"));
    fast.before = vec![Bytes::from(prefix.clone())];
    fast.gate = Some(gate.clone());
    let mock = AttemptMock::start(vec![(FAST, fast)]).await;
    let proxy = start_proxy_with(&mock.url, StoreOpts::default(), &[]).await;
    configure(&proxy, Some(same_model(1, 0))).await;
    let response = request(&proxy, true, None);
    mock.wait_for("preselection prefix", |seen| {
        seen.emitted.get(FAST) == Some(&1)
    })
    .await;
    gate.add_permits(1);
    let (status, body) = response.await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(&format!("{prefix}{token}{tail}")),
        "postwinner bytes must not count against prefix bound; response length: {}",
        body.len()
    );
}

#[tokio::test]
async fn all_failed_candidates_return_one_race_error() {
    let mock = AttemptMock::start(vec![
        (SLOW, AttemptReply::failure()),
        (FAST, AttemptReply::failure()),
    ])
    .await;
    let proxy = start_proxy_with(&mock.url, StoreOpts::default(), &[]).await;
    configure(&proxy, Some(same_model(2, 0))).await;
    let (status, body) = request(&proxy, false, None).await.unwrap();
    assert_eq!(status, StatusCode::BAD_GATEWAY, "all failed: {body}");
    let body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["error"]["code"], "race_failed");
    let mut calls = mock.calls();
    calls.sort();
    assert_eq!(calls, [SLOW, FAST]);
    permits_released(&proxy).await;
}

#[tokio::test]
async fn deadline_cancels_inflight_and_queued_candidates_without_bypassing_rpm() {
    let (slow, _slow_gate) = AttemptReply::gated(SLOW);
    let (fast, _fast_gate) = AttemptReply::gated(FAST);
    let (third, _third_gate) = AttemptReply::gated(THIRD);
    let mock = AttemptMock::start(vec![(SLOW, slow), (FAST, fast), (THIRD, third)]).await;
    let proxy = start_proxy_with(
        &mock.url,
        StoreOpts {
            nim_keys: vec![("one-rpm-test-key".into(), 1)],
            max_inflight: 1,
            ..StoreOpts::default()
        },
        &[],
    )
    .await;
    configure(&proxy, Some(same_model(3, 0))).await;
    let (status, body) = request(&proxy, false, Some(300)).await.unwrap();
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT, "deadline: {body}");
    let body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["error"]["code"], "deadline_exceeded");
    let calls = mock.calls();
    assert_eq!(calls.len(), 1, "one RPM permits exactly one upstream send");
    mock.cancelled(&calls[0]).await;
    assert_eq!(
        mock.state.observations.lock().unwrap().calls[0].1,
        "Bearer one-rpm-test-key"
    );
    // A cancelled generation must not refund an already sent RPM slot. A
    // following request is admitted at the client cap, then expires in queue.
    configure(&proxy, None).await;
    let (status, body) = request(&proxy, false, Some(150)).await.unwrap();
    assert_eq!(
        status,
        StatusCode::GATEWAY_TIMEOUT,
        "after deadline: {body}"
    );
    assert_eq!(mock.calls().len(), 1, "sent RPM cannot be double spent");
    permits_released(&proxy).await;
}

#[tokio::test]
async fn disconnect_releases_race_inflight_and_governor_permits() {
    let (slow, _slow_gate) = AttemptReply::gated(SLOW);
    let mut fast = AttemptReply::json(FAST);
    fast.content_type = "text/event-stream";
    fast.after = vec![Bytes::from_static(
        b"data: {\"choices\":[{\"delta\":{\"content\":\"selected winner\"}}]}\n\n",
    )];
    fast.hang = true;
    let mock = AttemptMock::start(vec![
        (SLOW, slow),
        (FAST, fast),
        (ORIGINAL, AttemptReply::json(ORIGINAL)),
    ])
    .await;
    let proxy = start_proxy_with(
        &mock.url,
        StoreOpts {
            max_inflight: 1,
            stream_idle_secs: 0,
            ..StoreOpts::default()
        },
        &[],
    )
    .await;
    let cookie = login(&proxy).await;
    let set_governor = client()
        .post(proxy.url("/api/settings/governor"))
        .header(header::COOKIE, &cookie)
        .json(&json!({"enabled": true, "set_override": {"model": ORIGINAL, "cap": 2}}))
        .send()
        .await
        .unwrap();
    assert_eq!(set_governor.status(), StatusCode::OK);
    configure(&proxy, Some(same_model(2, 0))).await;
    let response = client()
        .post(proxy.url("/v1/chat/completions"))
        .json(&chat_body("disconnect winner", true))
        .send()
        .await
        .unwrap();
    let mut stream = response.bytes_stream();
    let mut body = String::new();
    tokio::time::timeout(Duration::from_secs(4), async {
        while !body.contains("selected winner") {
            body.push_str(&String::from_utf8_lossy(
                &stream.next().await.unwrap().unwrap(),
            ));
        }
    })
    .await
    .expect("winner should be selected while loser is blocked");
    mock.cancelled(SLOW).await;
    let (status, _) = request(&proxy, false, None).await.unwrap();
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "open winner holds the one client inflight permit"
    );
    drop(stream);
    mock.cancelled(FAST).await;
    permits_released(&proxy).await;
    configure(&proxy, None).await;
    let (status, body) = request(&proxy, false, Some(1000)).await.unwrap();
    winner(status, &body, ORIGINAL);
}

#[tokio::test]
async fn streaming_deadline_cancels_before_and_after_winner_selection() {
    for has_winner in [false, true] {
        let mut slow = AttemptReply::sse(": still waiting\n\n");
        slow.hang = true;
        let mut fast = AttemptReply::sse(if has_winner {
            "data: {\"choices\":[{\"delta\":{\"content\":\"selected winner\"}}]}\n\n"
        } else {
            ": also waiting\n\n"
        });
        fast.hang = true;
        let mock = AttemptMock::start(vec![(SLOW, slow), (FAST, fast)]).await;
        let proxy = start_proxy_with(
            &mock.url,
            StoreOpts {
                stream_idle_secs: 0,
                ..StoreOpts::default()
            },
            &[],
        )
        .await;
        configure(&proxy, Some(same_model(2, 0))).await;
        let (status, body) = request(&proxy, true, Some(500)).await.unwrap();
        assert_eq!(
            status,
            StatusCode::OK,
            "stream has already committed: {body}"
        );
        assert_eq!(
            body.matches("deadline_exceeded").count(),
            1,
            "deadline: {body}"
        );
        assert_eq!(body.contains("selected winner"), has_winner);
        mock.cancelled(SLOW).await;
        mock.cancelled(FAST).await;
        permits_released(&proxy).await;
    }
}

#[tokio::test]
async fn disconnect_before_selection_cancels_every_candidate() {
    let (slow, _slow_gate) = AttemptReply::gated(SLOW);
    let (fast, _fast_gate) = AttemptReply::gated(FAST);
    let mock = AttemptMock::start(vec![
        (SLOW, slow),
        (FAST, fast),
        (ORIGINAL, AttemptReply::json(ORIGINAL)),
    ])
    .await;
    let proxy = start_proxy_with(
        &mock.url,
        StoreOpts {
            max_inflight: 1,
            stream_idle_secs: 0,
            ..StoreOpts::default()
        },
        &[],
    )
    .await;
    configure(&proxy, Some(same_model(2, 0))).await;
    let response = client()
        .post(proxy.url("/v1/chat/completions"))
        .json(&chat_body("disconnect before winner", true))
        .send()
        .await
        .unwrap();
    mock.called(&[SLOW, FAST]).await;
    drop(response);
    mock.cancelled(SLOW).await;
    mock.cancelled(FAST).await;
    permits_released(&proxy).await;
    configure(&proxy, None).await;
    let (status, body) = request(&proxy, false, Some(1000)).await.unwrap();
    winner(status, &body, ORIGINAL);
}

#[tokio::test]
async fn racing_contender_waits_for_existing_model_governor_permit() {
    let mut fast = AttemptReply::sse(
        "data: {\"choices\":[{\"delta\":{\"content\":\"holds model permit\"}}]}\n\n",
    );
    fast.hang = true;
    let mock = AttemptMock::start(vec![(FAST, fast)]).await;
    let proxy = start_proxy_with(
        &mock.url,
        StoreOpts {
            max_inflight: 2,
            stream_idle_secs: 0,
            ..StoreOpts::default()
        },
        &[],
    )
    .await;
    let cookie = login(&proxy).await;
    let set_governor = client()
        .post(proxy.url("/api/settings/governor"))
        .header(header::COOKIE, &cookie)
        .json(&json!({"enabled": true, "set_override": {"model": ORIGINAL, "cap": 1}}))
        .send()
        .await
        .unwrap();
    assert_eq!(set_governor.status(), StatusCode::OK);
    let response = client()
        .post(proxy.url("/v1/chat/completions"))
        .json(&chat_body("holds model permit", true))
        .send()
        .await
        .unwrap();
    let mut stream = response.bytes_stream();
    let mut prefix = String::new();
    tokio::time::timeout(Duration::from_secs(4), async {
        while !prefix.contains("holds model permit") {
            prefix.push_str(&String::from_utf8_lossy(
                &stream.next().await.unwrap().unwrap(),
            ));
        }
    })
    .await
    .expect("ordinary stream owns the model permit");
    configure(&proxy, Some(same_model(1, 0))).await;
    let (status, body) = request(&proxy, false, Some(300)).await.unwrap();
    assert_eq!(
        status,
        StatusCode::GATEWAY_TIMEOUT,
        "contender must wait: {body}"
    );
    assert_eq!(
        mock.calls(),
        [FAST],
        "model cap must govern every contender"
    );
    drop(stream);
    mock.cancelled(FAST).await;
    permits_released(&proxy).await;
}

#[tokio::test]
async fn concurrent_attempts_share_the_original_model_governor() {
    let (first, _first_gate) = AttemptReply::gated(SLOW);
    let mock = AttemptMock::start(vec![
        (SLOW, first),
        (FAST, AttemptReply::json(FAST)),
        (THIRD, AttemptReply::json(THIRD)),
    ])
    .await;
    let proxy = start_proxy_with(&mock.url, StoreOpts::default(), &[]).await;
    let cookie = login(&proxy).await;
    let response = client()
        .post(proxy.url("/api/settings/governor"))
        .header(header::COOKIE, cookie)
        .json(&json!({"enabled": true, "set_override": {"model": ORIGINAL, "cap": 1}}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    configure(&proxy, Some(same_model(3, 0))).await;
    let (status, body) = request(&proxy, false, Some(500)).await.unwrap();
    assert_eq!(
        status,
        StatusCode::GATEWAY_TIMEOUT,
        "shared governor deadline: {body}"
    );
    assert_eq!(
        mock.calls(),
        [SLOW],
        "three ready keys cannot bypass the one-per-model cap"
    );
    mock.cancelled(SLOW).await;
    permits_released(&proxy).await;
}

#[tokio::test]
async fn postselection_errors_and_unterminated_streams_are_not_successes() {
    for tail in [
        "event: error\ndata: {\"error\":{\"message\":\"late failure\"}}\n\n",
        "",
    ] {
        let gate = Arc::new(Semaphore::new(0));
        let prefix = "data: {\"choices\":[{\"delta\":{\"content\":\"partial answer\"}}]}\n\n";
        let mut fast = AttemptReply::sse(tail);
        fast.before = vec![Bytes::from_static(prefix.as_bytes())];
        fast.gate = Some(gate.clone());
        let mock = AttemptMock::start(vec![(FAST, fast); 3]).await;
        let proxy = start_proxy_with(&mock.url, StoreOpts::default(), &[]).await;
        configure(&proxy, Some(same_model(1, 0))).await;
        let cookie = login(&proxy).await;
        // Three failed selected streams would wrongly qualify this model for
        // ranked results if EOF alone were counted as successful completion.
        for _ in 0..3 {
            let response = client()
                .post(proxy.url("/v1/chat/completions"))
                .json(&chat_body("late failure", true))
                .send()
                .await
                .unwrap();
            let mut stream = response.bytes_stream();
            let mut body = String::new();
            tokio::time::timeout(Duration::from_secs(4), async {
                while !body.contains("partial answer") {
                    body.push_str(&String::from_utf8_lossy(
                        &stream.next().await.unwrap().unwrap(),
                    ));
                }
            })
            .await
            .expect("meaningful prefix must select winner before the tail");
            gate.add_permits(1);
            while let Some(chunk) = stream.next().await {
                body.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
            }
            assert!(
                body.contains(&format!("{prefix}{tail}")),
                "winner bytes moved: {body}"
            );
        }
        let stats: Value = client()
            .get(proxy.url("/api/racing/stats"))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let model = &stats["models"][0];
        assert_eq!(model["name"], ORIGINAL);
        assert_eq!(model["total_requests"], 3);
        assert_eq!(
            model["successful_requests"], 0,
            "late failure counted as success: {stats}"
        );
        assert_eq!(model["success_rate"], 0.0);
        let ranked: Value = client()
            .get(proxy.url("/api/racing/ranked"))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            ranked["models"],
            json!([]),
            "failed streams must not earn a rank"
        );
    }
}
