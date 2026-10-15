---
type: Component
title: Opt-in same-model request racing
description: Identical request copies with governed admission, delayed hedges, key diversity, effective-response selection and cancellation.
tags: [racing, routing, cancellation, streaming]
timestamp: 2026-10-12T00:00:00Z
---

# Opt-in same-model request racing

The [decision](../decisions/same-model-request-racing.md) supersedes the old
multi-model interpretation. An optional config-v1 `racing` policy is validated
at startup and settings writes, persisted through `settings::commit`, and
captured in each request's immutable snapshot. Absence disables it; enabled
racing conflicts with strict passthrough. Management APIs remain admin-only.
Legacy policies fail validation instead of being silently reinterpreted.

`proxy::RaceRequest` owns the original body and candidate I/O. Attempt identity
is an ordinal, never a replacement model. The original body passes through,
subject only to normal usage injection and its rejection fallback.
`racing::select` uses owned futures: immediate copies launch together; delayed
copies join pending originals. Failure of all current contenders accelerates
the next launch. A winner drops pending contenders and prevents future launches.

Each send takes a shared [model permit](governor.md) and a
[FIFO dispatcher](dispatcher.md) grant. Racing waiters share a request-local
set of granted key identities. The dispatcher chooses an unused ready key when
possible and records the key before granting another waiter. If all unused keys are busy it uses
ready existing capacity. The [pool](key-pool.md) retains its normal rolling
window, cooldown, pacing and rebuild semantics. Original affinity is preserved
for the first copy. Key diversity does not bypass the shared model limit.

Buffered selection requires successful HTTP and a complete valid JSON
completion with nonempty choices, terminal finish reasons and meaningful
assistant output, including complete callable tool/function data. Streamed
selection retains the bounded SSE gate for complete
content, reasoning or tool deltas. Comments and empty deltas cannot win. The
winner's bytes are relayed unchanged; a later error never triggers splicing
in another generation. The winner retains its permit throughout stream relay.

The single streaming task owns in-flight accounting, observation finalization,
and disconnect/[absolute deadline](../decisions/explicit-request-deadline.md)
cancellation. Buffered deadlines bound otherwise-unobservable disconnect work.
Losers release HTTP/model ownership without refunding already-sent RPM.

Observations record definitive failures and completed winners, excluding
cancelled work. The SSE classifier follows the winner so errors and incomplete
EOF cannot become successful samples. Buffered latency is not TTFT; missing
measurements are excluded from rankings. Statistics are process-local, bounded
and biased toward winners. No live latency improvement is established.

See the [operator guide](../../docs/features/model-racing.md) for schemas,
migration and metrics; [test strategy](../testing/test-strategy.md) and
[active plan](../../docs/plans/model-racing-implementation.md) for proof and
delivery state.
