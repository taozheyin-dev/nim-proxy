---
type: Decision
title: Same-model request racing and explicit policy replacement
description: Race identical request attempts, keep originals eligible after delayed hedges, and reject legacy cross-model policies instead of silently migrating them.
tags: [racing, routing, configuration, compatibility]
timestamp: 2026-10-12T00:00:00Z
---

# Same-model request racing

## Context

The owner clarified that racing means sending several copies to the same
client-selected model and returning the first effective response. The earlier
unreleased implementation replaced the model, prohibited repeated IDs and
cancelled its primary before launching fallbacks. Those semantics cannot be
reinterpreted as request copies without an explicit policy change.

## Options

1. Allow duplicate model IDs in the old policies. This still permits model
   substitution and hides both the number of attempts and launch semantics.
2. Infer an attempt count from legacy model lists. This silently changes
   durable operator intent and is rejected.
3. Introduce an explicit same-model policy and require replacement of any old
   policy. Reuse admission, owned futures and the settings transaction.

## Choice

Choose option 3. `same_model` carries `attempts` (1–5) and `hedge_delay_ms`
(0–300000). Zero launches all copies. A positive delay launches the first now
and each next after that interval from the preceding launch. If every launched
attempt fails, start the next immediately. Originals remain contenders until
they fail or another response wins. Delay begins before admission; the
[governor](../architecture/governor.md) and RPM queue may serialize sends.

Every attempt preserves the original model and request body, except the
already documented [usage injection](usage-injection-auto-fallback.md).
The first attempt retains [conversation affinity](sticky-affinity-with-spillover.md).
At each subsequent grant, prefer a key not already granted to this race if one
is ready; otherwise reuse ready capacity. Record key identities rather than
lane positions so a pool rebuild cannot confuse the group. Diversity never
reserves extra capacity, bypasses FIFO, or bypasses model admission.

Buffered winners require HTTP success and complete JSON with no top-level
error, nonempty choices, terminal finish reasons and meaningful assistant
content, reasoning, complete tool/function call or refusal output. Buffered
function calls carry a nonempty name and argument string; tool calls also
carry a nonempty call ID and function type. Streaming winners
use the existing bounded complete-event gate for content, reasoning or tool
data. The winning stream is relayed without generation splicing; later errors
and truncated endings remain failed observations.

The optional config-v1 `racing` field, routes and response envelopes keep their
names. Legacy `full_parallel`, `smart`, and `ttft_racing` policies are parsed
only to return an actionable validation error: enabling them fails and durable
stores refuse startup without rewriting their bytes. Operators replace the
field with a same-model policy or remove it to disable racing. Unknown or mixed
policy fields fail parsing. Absence remains disabled.

Metric names and model labels stay intact. New requests use strategy label
`same_model` and cancellation reason `winner_found`. The historical strategy
and smart-primary counters are no longer emitted by the retired policies;
old history is not relabelled. Cancelled counts cover launched pending
contenders, not unlaunched copies or RPM refunds.

## Consequences

- This is an intentional breaking change to an unreleased opt-in policy,
  recorded in [release notes](../../CHANGELOG.md), generated OpenAPI and
  [operator instructions](../../docs/features/model-racing.md).
- A one-key pool can race several governed requests on that key. Distinct
  keys do not imply independent NIM workers or guaranteed lower latency.
- All sends consume RPM, including retries and losers. Cancellation releases
  HTTP/model ownership without refunding sent reservations.
- The [component](../architecture/model-racing.md) explains executable
  behavior; the [active plan](../../docs/plans/model-racing-implementation.md)
  owns proof and delivery state, following the [test strategy](../testing/test-strategy.md).
