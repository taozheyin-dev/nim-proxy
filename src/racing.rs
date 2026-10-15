//! Selection policy only. The proxy owns admission, retries and response I/O.
//! Dropping a race drops pending attempts instead of detaching upstream tasks.
pub use crate::racing_config::RaceStrategy;
use futures_util::{stream::FuturesUnordered, StreamExt};
use metrics::counter;
use std::future::Future;
use std::time::Duration;

pub(crate) async fn select<T, F, Fut>(strategy: RaceStrategy, mut attempt: F) -> Result<T, ()>
where
    F: FnMut(usize) -> Fut,
    Fut: Future<Output = Result<T, ()>>,
{
    let RaceStrategy::SameModel {
        attempts,
        hedge_delay_ms,
    } = strategy
    else {
        return Err(());
    };
    counter!("nimproxy_race_requests_total", "strategy" => "same_model").increment(1);
    let delay = Duration::from_millis(hedge_delay_ms);
    let mut pending = FuturesUnordered::new();
    let mut launched = 0;
    let mut next_launch = tokio::time::Instant::now();
    loop {
        // Start immediately when no viable attempt remains. Otherwise each
        // delayed copy joins the originals without taking away their chance.
        if launched < attempts && (pending.is_empty() || delay.is_zero()) {
            pending.push(attempt(launched));
            launched += 1;
            next_launch = tokio::time::Instant::now() + delay;
            if delay.is_zero() && launched < attempts {
                continue;
            }
        }
        if pending.is_empty() {
            return Err(());
        }
        tokio::select! {
            // A response already ready at the launch boundary wins before
            // another copy is started.
            biased;
            result = pending.next() => {
                if let Some(Ok(value)) = result {
                    counter!("nimproxy_race_cancelled_total", "reason" => "winner_found")
                        .increment(pending.len() as u64);
                    return Ok(value);
                }
            }
            _ = tokio::time::sleep_until(next_launch), if launched < attempts => {
                pending.push(attempt(launched));
                launched += 1;
                next_launch = tokio::time::Instant::now() + delay;
            }
        }
    }
}

/// Buffered candidates must be complete assistant output, not merely HTTP 200.
pub(crate) fn effective_completion(bytes: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return false;
    };
    value.get("error").is_none()
        && value
            .get("choices")
            .and_then(|v| v.as_array())
            .is_some_and(|choices| {
                !choices.is_empty()
                    && choices.iter().all(|choice| {
                        choice
                            .get("finish_reason")
                            .and_then(|v| v.as_str())
                            .is_some_and(|s| !s.is_empty())
                            && choice.get("message").is_some_and(|message| {
                                message.get("role").and_then(|v| v.as_str()) == Some("assistant")
                                    && complete_message(message)
                            })
                    })
            })
}

fn complete_message(message: &serde_json::Value) -> bool {
    let nonempty = |value: Option<&serde_json::Value>| {
        value
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty())
    };
    let function = |value: &serde_json::Value| {
        nonempty(value.get("name"))
            && value
                .get("arguments")
                .is_some_and(serde_json::Value::is_string)
    };
    // Text accompanying a malformed call cannot make the complete response
    // usable. Null/absent call fields remain ordinary optional fields.
    let has_function = match message.get("function_call").filter(|v| !v.is_null()) {
        Some(call) if function(call) => true,
        Some(_) => return false,
        None => false,
    };
    let has_tools = match message.get("tool_calls").filter(|v| !v.is_null()) {
        Some(value) => {
            let Some(calls) = value.as_array() else {
                return false;
            };
            if !calls.iter().all(|call| {
                nonempty(call.get("id"))
                    && call.get("type").and_then(|v| v.as_str()) == Some("function")
                    && call.get("function").is_some_and(function)
            }) {
                return false;
            }
            !calls.is_empty()
        }
        None => false,
    };
    nonempty(message.get("content"))
        || nonempty(message.get("reasoning_content"))
        || nonempty(message.get("refusal"))
        || has_function
        || has_tools
}

/// Bounded complete-event classifier; the relay preserves the original bytes.
#[derive(Default)]
pub(crate) struct SseGate {
    line: Vec<u8>,
    data: Vec<u8>,
    after_cr: bool,
    prefix_bytes: usize,
    finished: bool,
    pub ready: bool,
    pub failed: bool,
    pub done: bool,
}
pub(crate) const PREFIX_LIMIT: usize = 64 * 1024;

impl SseGate {
    pub fn is_complete(&self) -> bool {
        !self.failed && (self.done || self.finished) && self.line.is_empty() && self.data.is_empty()
    }

    pub fn push(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if self.failed || self.done {
                return;
            }
            if !self.ready {
                self.prefix_bytes += 1;
                if self.prefix_bytes > PREFIX_LIMIT {
                    self.failed = true;
                    return;
                }
            }
            // SSE accepts LF, CRLF and bare CR, even when CRLF is split
            // between reads. The byte limit ends at the winning event.
            if self.after_cr && byte == b'\n' {
                self.after_cr = false;
                continue;
            }
            self.after_cr = byte == b'\r';
            if matches!(byte, b'\r' | b'\n') {
                self.finish_line();
            } else if self.line.len() + self.data.len() >= PREFIX_LIMIT {
                self.failed = true;
                self.line.clear();
                self.data.clear();
            } else {
                self.line.push(byte);
            }
        }
    }

    fn finish_line(&mut self) {
        if self.line.is_empty() {
            if self.data == b"[DONE]" {
                self.done = true;
            } else if !self.data.is_empty() {
                match serde_json::from_slice::<serde_json::Value>(&self.data) {
                    Ok(value) if value.get("error").is_some() => self.failed = true,
                    Ok(value) => {
                        self.ready |= value.get("choices").and_then(|v| v.as_array()).is_some_and(
                            |choices| {
                                self.finished |= !choices.is_empty()
                                    && choices.iter().all(|choice| {
                                        choice
                                            .get("finish_reason")
                                            .and_then(|v| v.as_str())
                                            .is_some_and(|reason| !reason.is_empty())
                                    });
                                choices
                                    .iter()
                                    .any(|choice| choice.get("delta").is_some_and(meaningful_delta))
                            },
                        );
                    }
                    Err(_) => self.failed = true,
                }
            }
            self.data.clear();
        } else if let Some(data) = self.line.strip_prefix(b"data:") {
            let data = data.strip_prefix(b" ").unwrap_or(data);
            if !self.data.is_empty() {
                self.data.push(b'\n');
            }
            self.data.extend_from_slice(data);
        } else if let Some(event) = self.line.strip_prefix(b"event:") {
            self.failed |= event.strip_prefix(b" ").unwrap_or(event) == b"error";
        }
        self.line.clear();
    }
}

fn meaningful_delta(delta: &serde_json::Value) -> bool {
    let nonempty = |value: Option<&serde_json::Value>| {
        value
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty())
    };
    nonempty(delta.get("content"))
        || nonempty(delta.get("reasoning_content"))
        || delta
            .get("tool_calls")
            .and_then(|v| v.as_array())
            .is_some_and(|calls| {
                calls.iter().any(|call| {
                    nonempty(call.get("id"))
                        || call.get("function").is_some_and(|function| {
                            nonempty(function.get("name")) || nonempty(function.get("arguments"))
                        })
                })
            })
        || delta.get("function_call").is_some_and(|function| {
            nonempty(function.get("name")) || nonempty(function.get("arguments"))
        })
}
