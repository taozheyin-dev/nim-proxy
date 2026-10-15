# Same-model request racing

Racing is disabled by default. When an administrator enables it, each
`POST /v1/chat/completions` races several copies of the client's request using
exactly the requested model. Messages, tools and sampling parameters are
preserved. Other endpoints keep their normal routing.

## Configure

All four operations require an authenticated admin or superuser session:

| Operation | Purpose |
|---|---|
| `GET /api/racing/config` | Read `{enabled, strategy}` |
| `POST /api/racing/config` | Validate, persist and publish a policy |
| `GET /api/racing/stats` | Read observed model statistics |
| `GET /api/racing/ranked` | Read models with measured TTFT samples |

For two immediate copies, send this JSON to `POST /api/racing/config`:

```json
{
  "enabled": true,
  "strategy": {
    "type": "same_model",
    "attempts": 2,
    "hedge_delay_ms": 0
  }
}
```

`attempts` is 1–5. `hedge_delay_ms` is 0–300000: zero launches all copies;
a positive value launches the first immediately and the next copy after that
interval from the preceding launch. If all launched copies fail, the next
starts immediately. The original remains eligible to win after another copy
starts. A winner prevents any remaining copies from launching. Admission can
still serialize sends when RPM capacity or model permits are unavailable.

To disable, send `{"enabled":false}`. Saves return `{"ok":true}` and survive
restart. Rejections preserve runtime and durable settings. Racing conflicts
with `strict_passthrough`; both settings writers enforce this. There are no
Dashboard controls or per-request override headers. See [OpenAPI](../../openapi.json).

### Replacing an old policy

The unreleased `full_parallel`, `smart` and `ttft_racing` policies selected
other models and have been retired. Enabling them is rejected with a migration message.
A store containing one refuses startup without rewriting the file. Before
upgrading, disable racing using the old build, or stop the proxy and back up
`DATA_DIR/config.json`, then replace its optional `racing` object with the
`same_model` inner strategy object above (include neither the `enabled` nor the
`strategy` wrapper). Removing
`racing` disables it. Never map a model list to an attempt count automatically.
Unknown fields and mixed old/new policy fields are rejected.

## Effective responses and cancellation

Buffered requests select the first fully read successful HTTP response whose
JSON contains no top-level error and has nonempty completion choices. Every
choice must have a nonempty finish reason and an assistant message containing
content, reasoning, complete tool/function-call or refusal output. Buffered
function calls require a function name and argument string; tool calls also
require a call ID and function type. Empty, malformed,
error and unfinished payloads cannot win. The winning bytes are relayed intact.

Streaming selection waits for a complete SSE event carrying nonempty content,
reasoning content or tool/function-call data. Comments, role-only/empty deltas
and errors before any meaningful event cannot win. Prefixes are bounded to
64 KiB through the winning event; later bytes in that read do not count toward
the prefix limit. LF, CRLF and CR framing are supported. The winning prefix
and subsequent stream are relayed intact, without switching generations after
selection. Later errors and incomplete endings remain failed observations.
If every copy fails, buffered requests receive HTTP 502 `race_failed`; streams
receive that code in a terminal SSE error after HTTP 200.

Each send, including retries and usage-injection fallback, goes through the
shared model governor and FIFO per-key RPM dispatcher. The first copy retains
conversation affinity. Subsequent copies prefer unused ready keys within this
race; if none are ready they reuse available keys, including a single-key pool.
Key identities survive lane reorder during settings changes. Distinct keys do
not imply independent NIM workers. Losing HTTP futures and permits are dropped;
already-sent requests keep their RPM cost.

`X-Nim-Proxy-Deadline-Ms` bounds admission, selection and complete winner relay.
Streaming disconnect cancels work immediately. Buffered clients should provide
a deadline for a wall-clock bound; detecting their disconnect before a response
is available remains a transport limitation. Normal request/stream-idle timeouts
and usage injection with one-shot rejection fallback still apply.

## Observations and limits

Metric names remain unchanged. Requests use
`nimproxy_race_requests_total{strategy="same_model"}`; pending launched losers
use `nimproxy_race_cancelled_total{reason="winner_found"}`. Unlaunched copies
are not cancellations. Wins and TTFT use `nimproxy_race_model_wins_total` and
`nimproxy_race_ttft_seconds`, labelled with the client's model. Retired strategy
labels and smart-primary counters stop receiving new samples; history is not
rewritten.

Statistics are bounded to 256 model identities and reset on restart. Completed
winners and definitive failures contribute; cancelled work is excluded.
Buffered latency is not TTFT. Rankings require three measured successful TTFT
samples. Winner observations are biased samples,
not a controlled benchmark. No production speedup is claimed.

See the [compatibility decision](../../knowledge/decisions/same-model-request-racing.md)
and [implementation plan](../plans/model-racing-implementation.md) for rationale
and verification records.
