---
type: Runbook
title: Ordinary and racing channels in new-api
description: Route two new-api groups through independently configured proxies without sharing active NIM keys.
tags: [new-api, deployment, racing, rate-limiting]
timestamp: 2026-10-15T00:00:00Z
---

# Ordinary and racing channels in new-api

Use two nim-proxy instances when ordinary requests and optional same-model
racing must coexist. The [racing policy](../architecture/model-racing.md) is
instance-wide, captured per request. There is no per-request policy override.

## Configuration and ownership

Each instance needs its own data directory and client credential. Retain keyed
authentication and an existing operator account; do not expose an unfinished
setup wizard. Bind host ports to loopback. A Docker-hosted new-api can reach
them using `host.docker.internal` on Docker Desktop.

| Channel | Base URL in new-api | Groups | Racing |
|---|---|---|---|
| 英伟达 | `http://host.docker.internal:8001` | Existing ordinary groups | Disabled |
| 英伟达测试 | `http://host.docker.internal:8002` | `NVIDIA_TEST` | Two attempts, 1000 ms delay |

These URLs are new-api channel base URLs; do not append `/v1` there. Configure
the API with the channel's dedicated `npk_` client credential, rather than a
NIM upstream key. Preserve the original model IDs, mappings, channel type,
weights, priority and ordinary group membership. Add the test group's ratio
and display label without changing existing groups. To use the test channel,
create a new-api token assigned to `NVIDIA_TEST`; an ordinary token continues
using its ordinary group. An administrator's pinned-channel token is useful
for diagnosis but does not prove group routing.

The [limiter](../architecture/key-pool.md) is process-local. Active key sets
must be disjoint across both instances and any other service using those NIM
keys. Compare the actual key values privately, including encrypted stores
when authorized; comparing ciphertext with plaintext proves nothing. Emit
only aggregate results. Retain excluded keys disabled if needed for recovery.
Never start a preserved benchmark or rollback container while its keys are
active elsewhere. This check covers the inspected services, not unknown
external consumers of the same account's credentials.

## Cutover

1. Read the current channels and group settings, then take a consistent private
   database backup. Validate integrity and capture the exact old channel and
   route rows. Refuse stale configuration instead of overwriting it.
2. Prepare and review the replacement before stopping traffic. Prefer supported
   management APIs. If no operator session is available and the owner authorized
   local database maintenance, stop new-api before writing SQLite and restart it
   afterward to refresh cached channels/options.
3. Exclude shared keys and the test subset from the ordinary instance using its
   Settings API. Disable ordinary racing. Publish a launchable test config only
   after exclusion and authentication checks succeed. On failure, quarantine the
   test config and keep affected data planes stopped for recovery.
4. Drain and stop the ordinary proxy. Wait at least 61 seconds after its stop
   and the old direct caller's stop before starting either replacement pool.
   A restart loses the in-memory rate window; retrying an upstream 429 does not
   make an overlapping restart safe.
5. Replace the old channel and abilities in one database transaction. Choose
   new IDs beyond all channel, ability and historical-log references. Preserve
   unrelated channels, routes and options with exact before/after comparisons.
6. Start both proxies, require `/health` 200 and anonymous client/operator API
   401 responses, then start new-api. Confirm the policies and key counts via
   authenticated APIs.

## Verification and recovery

Follow the [test strategy](../testing/test-strategy.md). Exercise the database
transaction on a backup and roll it back. A drift control must identify its
specific rejection and leave the channel/token rows unchanged.

Run bounded synthetic-prompt requests through both real groups. Require visible
SSE output, a finish reason and `[DONE]`, with no error event; check the actual
channel and group in new-api's completion logs. Observe ordinary and racing
metrics separately. A fast response may finish before the hedge is sent;
entering the racing path does not imply two upstream sends or a speedup.

Use an absolute client deadline and a byte bound; socket inactivity timeouts
alone do not stop continuous heartbeats. Do not assume new-api forwards the
proxy's custom deadline header. A finite `Content-Length` response can close
Python's response reader after the final nonempty read, so check EOF before
accessing its socket. Prove both complete finite SSE and a perpetual-heartbeat
timeout. Revoke any temporary model-limited test token, refresh new-api's cache,
and verify that it returns 401 afterward.

Keep the original database, config and containers as private recovery evidence.
Do not blindly restore the old active key set: it may include keys now owned
by another service. To retire testing, stop its traffic and proxy, drain the
ordinary proxy, disable the test channel, and reassign only the test subset
after the rate window expires. Continue excluding keys owned by other services.
Keep all data planes stopped if ownership or durable configuration is uncertain.

The [active plan](../../docs/plans/model-racing-implementation.md) records the
local deployment, independent review, exact verification and publication state.
