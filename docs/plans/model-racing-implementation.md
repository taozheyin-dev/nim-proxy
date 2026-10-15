# Plan — Model Racing Implementation

**Status:** Two-channel new-api deployment verified, 2026-10-15 (Asia/Shanghai).
The owner has resumed PR delivery to `taozheyin-dev/nim-proxy`; the original
upstream and `main` are not publication targets for this work item.
The earlier multi-model repair remains historical implementation evidence,
not acceptance of the corrected requirement.

## new-api integration — 2026-10-14

The owner cancelled further PR work and requested a usefulness check followed
by replacing the old NVIDIA channels in the local new-api instance with this
proxy if useful. Draft PR #1 was closed. This operational work uses isolated
branch `codex/new-api-integration` from the current integration branch; no PR
will be created for it.

- **Outcome:** prove real inference through nim-proxy, connect it to new-api,
  verify the complete new-api → nim-proxy → NIM path, and remove precisely the
  superseded NVIDIA channels after replacement validation.
- **Proof:** bounded synthetic-prompt live requests with absolute deadlines;
  record first effective/visible output, completion, failures and request cost
  without storing credentials or prompt/completion content in repository files.
  Back up relevant configuration privately, verify replacement channel routing,
  then re-read new-api's channel state and run end-to-end inference.
- **Constraint:** maintain authentication and limiter ownership, preserve
  non-NVIDIA channels and their routing, never infer speed gains from a single
  sample. Prefer native management APIs. Do not print credentials. Use the
  existing 2-attempt/1000 ms policy unless measured evidence warrants a change.
  No broad upstream key sweep; start with models already used by the old channel.
- **Ponytail Rung:** 2/3/4 — reuse existing APIs, Python standard library,
  Docker connectivity and the deployed binary; no production code change.
- **Act:** inspect old channels, prepare a restorable backup, perform bounded
  live usefulness checks, then replace and verify the authorized channels.

Scope delta surfaced before action: authorized writes include the local
new-api NVIDIA channel replacement and a dedicated proxy client credential
if required. All other channels remain outside scope. The new-api container
can reach `http://host.docker.internal:8001/health` successfully.

The owner then requested two channels: `英伟达` without racing and
`英伟达测试` with racing. Use two proxy instances with disjoint keys: 470
for ordinary traffic on port 8001, eight for testing on port 8002. Retire the
old benchmark process, which duplicated the production key set. Preserve its
container/data for recovery. Put test routing in a separate `NVIDIA_TEST`
new-api group; preserve the existing ordinary `default,NVIDIA` groups and all
80 model ids. Add the test group with the same ratio as NVIDIA and an explicit
test display label. New channels replace the old direct channel atomically.
This is an operational split; no per-request policy or wire contract is added.

### Resumed cutover — 2026-10-15

The previous invocation stopped before changing either channel or partitioning
the keys. Fresh inspection found the ordinary proxy still has 478 enabled keys
and racing enabled. The old benchmark is stopped. The owner has now authorized
PR work again under `taozheyin-dev`, and authenticated that account; its new
fork is `https://github.com/taozheyin-dev/nim-proxy`.

Scope delta shown before action: the still-running `nvidia-api-gateway` owns
137 of the same keys (exact comparison after in-memory decryption; no key
values emitted). Preserve that service and exclude those keys from both new
pools. Ordinary traffic therefore gets 333 keys and testing gets eight,
with 145 original entries retained disabled in the ordinary store. This
supersedes the proposed 470/eight allocation above.

- **Outcome:** replace the old direct channel with the two approved channels,
  preserve its 80 model ids and ordinary groups, and publish review to the
  owner's new fork.
- **Proof:** fresh consistent SQLite backup and integrity check; replacement
  transaction dry-run/rollback; exact unrelated-row comparisons; stopped-service
  cutover; key-set and authentication checks; bounded real inference through
  both ordinary and NVIDIA_TEST group routing. Review and validate documentation
  with the agent-guide selftest/current-link check and `git diff --check`.
- **Constraint:** never publish a test config before successful key exclusion.
  Stop new-api and drain nim-proxy, then wait at least 61 seconds after shutdown
  before starting either pool. Keep original data/containers for recovery.
  Use a short-lived, model-limited probe token for the test group and revoke it
  after verification. Client-side absolute deadlines do not assume new-api
  forwards arbitrary headers. Do not claim measured speedup or global ownership
  of keys outside the inspected services.
- **Ponytail Rung:** 2/3/4 — repair existing operational scripts, use Python's
  standard library and native Docker/GitHub operations; no proxy code change.
- **Act:** correct the independently reviewed script gaps, repeat dry-run and
  failure controls, execute cutover, verify, then ingest the operational runbook
  and publish the reviewed branch against `feature/model-racing` on the new fork.

Independent reviewer `integration_review` identified an unbounded stream-read
loop, a rollback that left a launchable overlapping test config, and a pinned
channel probe that did not exercise group routing. All were corrected before
execution. The reviewer then independently reproduced a finite Content-Length
EOF bug on Python 3.12 and verified the correction.

### Verified cutover evidence — 2026-10-15

- A fresh consistent backup passed SQLite integrity checking. The replacement
  transaction passed dry-run/rollback; a changed-channel-name control fired
  `Channel configuration changed since review` and preserved channel/token rows.
- new-api and nim-proxy stopped with exit code zero. The final stopped-service
  database backup has SHA256
  `7b61e8e76665ec07d85d568499bcb6625f4164e380c33780dd6c3e84fcb0e779`.
  It is private, alongside original channel/group rows and proxy config.
- Channel 1 (`NVIDIA`, direct upstream) was replaced by channel 47 (`英伟达`,
  `default,NVIDIA`, port 8001) and channel 48 (`英伟达测试`, `NVIDIA_TEST`,
  port 8002). Each retains all 80 model ids. Ordinary routing has 160 ability
  rows and testing has 80; unrelated historical ability rows compare equal.
  No other current channel existed. Only the two group options changed.
- Exact active-key comparisons prove ordinary/test counts 333/eight, no overlap,
  and exclusion of all 137 keys owned by the preserved gateway. All 478 original
  entries remain in the ordinary store, with 145 disabled. Ordinary racing is
  off; test racing is two same-model attempts with a 1000 ms delay.
- Both proxies started more than 61 seconds after the recorded stop: 132.235
  and 133.400 seconds. Both use the previously verified image
  `sha256:26cb95cbe871d76bb24aa4cd12207b301187ad45576a84417a56df0e7d54b467`.
  The two proxy containers and new-api are running and healthy. Each proxy
  returns health 200 and anonymous client/operator API 401.
- Both real group-routed streaming requests completed successfully through
  new-api and NIM. Completion logs identify channel 47 / `default` and channel
  48 / `NVIDIA_TEST`. Each granted one upstream attempt and observed no 429
  cooldown; racing counters increased by zero and one respectively. These are
  connectivity/policy samples, not performance comparisons. A fast first
  effective response can prevent the delayed copy from being sent.
- The model-limited, expiring probe token was revoked and its credential erased
  from the token row. new-api restarted to drop cached authorization; the old
  probe token now receives HTTP 401. Existing user tokens were preserved.
- The observer passed fragmented-SSE positive/negative controls, finite-length
  completion, and an absolute-timeout control under continuous heartbeats.
  The client deadline is 45 seconds, independent of custom-header forwarding.
- Source, tests and examples were hash-compared with the verification container:
  120 files match. A leftover transport-probe example was preserved outside the
  build tree after its private-module compile error. The next full run in the
  reused environment had two route-test failures; focused route checks passed
  four tests, then the full run with a fresh temporary directory passed 397
  tests (228 library, 119 E2E, 10 OpenAPI, 32 racing, eight configuration).
  The normally ignored history restart/idle test was not rerun this session.
  The earlier failures' cause is unproven; see the investigation record below.
- Clippy with denied warnings, `cargo fmt --check`, all 12 guide failure
  controls, current documentation links and `git diff --check` passed.
  Independent final review found no technical blocker in the operational
  scripts, runbook or sanitized reports, and independently verified EOF handling
  and the fresh Rust counts. It did not rerun production requests.
- The durable [new-api runbook](../../knowledge/ops/new-api-integration.md)
  records ownership, cutover, verification and safe rollback. Secret-free local
  reports are in `D:/Desktop/2api/nim-proxy-work/new-api-integration-20261014`;
  its sibling `-private` directory contains restricted recovery data.
  Browser matrix, release CI and human layout acceptance remain outside this
  operational closure. No measured latency gain is claimed.
- Published [draft PR #1](https://github.com/taozheyin-dev/nim-proxy/pull/1)
  from `codex/new-api-integration` to the new fork's `feature/model-racing`
  baseline (`41d77d5`). The PR is attached to this continuation chat. The
  original upstream and main refs were not pushed. GitHub Actions currently
  lists no registered workflows or runs on the new fork despite the workflow
  files being present; attempts to enable CI and CodeQL by filename returned
  404. Remote CI success is not claimed. Local operational/review evidence is
  complete, while the draft retains those remote acceptance limits.

### Pending verification-isolation investigation

- **Title:** Route contract checks failed in reused verifier, then passed.
- **What happened?** The first complete runnable check returned 200 instead of
  pre-setup 503 in `route_contract_client_auth_matrix`, and observed one upstream
  call during a static-asset row in `route_contract_behavior_matrix`. Production
  auth smoke passed. The cause of the two test failures is not established.
- **Steps to reproduce:** the observed command was `cargo test --locked --offline
  -- --test-threads=4` in the long-lived `/tmp/nim-delivery` verification tree.
  Four focused serialized route checks and a complete four-thread rerun with a
  new `TMPDIR` passed. A deterministic reproducer remains pending.
- **Auth mode:** synthetic configured/fresh stores, no production credentials.
- **How are you running it?** Docker Rust verifier on a Windows host.
- **Version or commit:** source at `37667e5`, unchanged by this operational item.
- **Client / harness:** Rust `tests/e2e.rs` route-contract checks.
- **Relevant logs:** `cargo-test-reused-env.log`, `route-recheck.log` and
  `cargo-test.log` in the secret-free local report directory.
- **Before submitting:** diagnosis/duplicate search pending; no credentials or
  user request contents are included. The new fork also has issues disabled,
  so the issue record remains here. This record does not assert a code cause.

## Delivery closure — 2026-10-14

The owner requested completion and explicitly selected upgrading the existing
loopback service on port 8001 after acceptance. This supersedes the previous
invocation's no-commit/no-deployment boundary. Integration remains
`feature/model-racing`; isolated work item `codex/nim-delivery` continues from
`7f3c259`, with all 15 inherited modified/untracked files copied intact from
the earlier worktree. That worktree and the original untracked reports remain
untouched. Main remains unchanged.

- **Outcome:** deliver reviewed, tested same-model racing, a local Docker image,
  and a verified upgrade of the existing service with rollback evidence.
- **Proof:** reproduce OpenAPI drift, regenerate it, run full Rust tests,
  Clippy, formatting, locale/source/link guards, strict synthetic load, and
  container health/auth/configuration smoke checks per the test strategy.
  Preserve secret-free logs and image/source identifiers.
- **Constraint:** no live benchmark is required for correctness acceptance and
  no speedup is claimed. Preserve credentials and operator configuration; do
  not silently enable racing or migrate legacy policies. Before upgrade inspect
  policy compatibility without printing secrets, stop gracefully, back up data,
  and wait out the 61-second rate window before starting the replacement.
  Retain the old container and image for rollback. Do not publish a release.
- **Ponytail Rung:** 2/4/5 — reuse the test container/cache, strict mock harness,
  native Docker lifecycle, and existing image build recipe.
- **Act:** close schema/doc gaps, independently review, verify and commit the
  work item, prepare the image, then perform the authorized local upgrade.

Scope delta surfaced before implementation: current delivery includes source
integration and the authorized port-8001 upgrade. Historic live-speed research
and localization visual acceptance remain separate; current correctness tests
must not be presented as performance or human layout approval.

### Delivery evidence

- The owner selected two attempts with a 1000 ms hedge delay for port 8001.
  Pre-upgrade config-v1 had no racing field, used installed `zh-CN`, and
  retained keyed client authentication. The upgrade added only the approved
  policy and preserved all other configuration values.
- OpenAPI drift was freshly reproduced: 9 passed, 1 failed at
  `committed_spec_matches_the_code`. Regeneration passed all 10 OpenAPI tests;
  the generated diff adds only four named-variant property caps.
- Guide selftest (12 controls), current links, locale selftest (26 cases),
  both production catalogs, i18n selftest (121 cases), 452 referenced ids,
  pseudolocale generation, script syntax and local asset guards passed.
- Independent agent `delivery_review` reviewed the same-model data plane,
  configuration, admission, schema fix and regression assertions and found no
  blocking defect. Its required log ingest is now recorded. The reviewer did
  not execute tests or approve deployment/performance claims.
- GitHub direct access failed to connect; the available local proxy returned
  HTTP 501 for CONNECT. A second existing proxy on loopback port 7897 then
  succeeded. Publish the task branch against the original `41d77d5` integration
  baseline on the fork so the already integrated local delivery remains
  reviewable; do not alter main.
- Published [draft PR #1](https://github.com/ytzzjx/nim-proxy/pull/1) from
  `codex/nim-delivery` to the fork's `feature/model-racing` baseline. It is
  attached to this delivery chat. The fork has issues disabled, so the browser
  bug-form record below remains pending; no upstream issue was filed.
- Fresh full Rust proof passed 397 tests: 228 library, 119 E2E, 10 OpenAPI,
  32 racing and 8 configuration. The one normally ignored
  `release_restart_and_idle_history` check was then explicitly run and passed.
  Clippy with warnings denied and `cargo fmt --check` passed.
- Strict synthetic load passed 100 clients x 3 requests: 300 completions,
  596 upstream sends (198/199/199 across three 40-RPM keys), zero client
  failures, zero worker exhaustion and zero upstream rate violations.
  The wrapper observed exactly 300 same-model races and reaped its children.
  This is optimized test-profile rate-safety evidence, not latency evidence.
- Release compilation passed offline with the cached Rust 1.97.1 musl
  toolchain. The standard Dockerfile's offline build lacked registry cache;
  its runtime stage was reused verbatim apart from local COPY inputs to
  package that release binary as `nim-proxy:same-model-20261014`.
  Image smoke proved health, pre-setup closure, admin configuration and a
  successful two-attempt/1000 ms buffered race with zero mock rate violations.
  Its first script assumed the mock emitted a model response field; that
  invalid test assumption was removed, and the rerun checked the actual
  completion and both per-key send increments. Production code was unchanged.
- Commit `ba64078` was independently reviewed and fast-forwarded into the local
  integration branch. The primary checkout now contains the completed source;
  its six original untracked files and the main ref remain unchanged. Task
  branches/worktrees and all prior regression commits are retained.
- Production upgrade completed on port 8001 using image
  `sha256:26cb95cbe871d76bb24aa4cd12207b301187ad45576a84417a56df0e7d54b467`.
  All 38 source/Cargo build inputs match the committed worktree. Binary SHA256:
  `8b9be40ff4ec953312edc375fba3ee8e92e736356587adabbca631e997b96a45`.
  The old service exited cleanly; three data files were copied and hash-checked
  into the restricted local rollback directory. More than 61 seconds elapsed
  before the replacement started. The only config change is the approved
  same-model policy (two attempts, 1000 ms). All previous values compare equal.
- A health probe issued immediately after start raced startup and failed;
  subsequent built-in and HTTP probes passed and Docker reports healthy.
  `/health` returns 200; anonymous `/api/racing/config` and `/v1/models`
  return 401. The real login page renders Chinese through the in-app browser
  with no captured warning/error logs. This is page smoke evidence, not the
  full interaction matrix or human visual approval.
- Rollback retains container `nim-proxy-before-same-model-20261014`, image tag
  `nim-proxy:before-same-model-20261014`, and the restricted directory
  `D:/Desktop/2api/nim-proxy-work/rollback-20261014-nim-delivery`.
  Secret-free reports, build logs and rollback instructions are in
  `D:/Desktop/2api/nim-proxy-work/delivery-20261014`.
- The extra browser matrix hung after Chromium exited; a serialized 240-second
  retry also did not complete. Neither run passes. Independent review found
  unbounded pending CDP calls after socket closure; the crash cause is unknown.
  Owned test proxies/mock and smoke containers were stopped. See pending bug
  below. Coverage, MSRV, security/release CI, visual-matrix human approval and
  live NIM latency benchmarking were not run and are not claimed.

### Pending browser-harness issue (fork issues disabled)

- **Title:** Browser render gate hangs indefinitely after Chromium exits.
- **What happened?** `CDP.send()` in `scripts/render_check.js` has neither a
  request deadline nor rejection of outstanding calls on WebSocket close. The
  child browser exited while Node and its test proxy remained alive with an
  empty log; the full matrix cannot be counted as passed.
- **Steps to reproduce:** run `node scripts/render_check.js --all-states` in
  the verifier. Browser exit was observed during this run; a deterministic
  forced-exit regression remains future work.
- **Auth mode:** synthetic configured store; no production credentials used.
- **How are you running it?** Docker, Linux Chromium, Windows host.
- **Version or commit:** `ba64078`.
- **Client / harness:** `scripts/render_check.js --all-states`.
- **Relevant logs:** Chromium children became zombies while the Node driver
  remained alive; container `oom` and `oom_kill` counters were zero. Browser
  crash cause is not established. The bounded rerun terminated without success.
- **Before submitting:** duplicate search could not run because fork issues
  are disabled; no credentials
  or production request contents are included.

## Historical completion and live-performance scope — 2026-10-14

The owner requested immediate completion with parallel subagents and real speed
measurements. Continue the existing isolated `codex/same-model-hedging-fix`
work item; preserve inherited edits, the primary checkout, main, and the running
production service. No commit, push, PR publication or production deployment is
part of this invocation.

- **Outcome:** close the OpenAPI regression, independently review the completed
  same-model implementation, and measure actual same-model latency versus racing
  disabled without presenting historical estimates as observed improvements.
- **Proof:** reproduce the generated-schema mismatch without update mode, then
  regenerate and run the OpenAPI, racing and configuration suites, the full Rust
  suite, Clippy, format and repository guards per the test strategy. Run the
  strict synthetic load proof. Validate the benchmark parser with positive and
  negative controls, then retain secret-free per-request timings and summaries.
- **Constraint:** use one isolated test proxy and its shared rate state for all
  live arms; credentials may only be consumed privately from an existing
  dedicated benchmark source. Do not inspect the production data directory.
  Use synthetic prompts, fixed model/body parameters, interleaved arm order,
  per-request deadlines and a bounded experiment (at most 240 upstream sends,
  at most two concurrent upstream attempts). Stop on rate-limit or credential
  errors; do not disable safety gates or substitute another model inside a race.
  Report first effective output, first visible content and completion latency
  separately, together with errors and upstream request consumption.
- **Ponytail Rung:** 2/3/5 — reuse the existing Rust verification container,
  cached dependencies, mock/load harness, settings API and Python standard
  library; add only the bounded measurement proof missing from the repository.
- **Act:** parallelize schema closure, benchmark preparation and independent
  review; serialize builds and live measurements to avoid resource interference.
  Test baseline, two immediate copies and two delayed copies on selected live
  models using one release binary. No measured benefit is promised in advance.

### Completion evidence

This invocation ended before fresh execution. The delivery closure above
supersedes its work state; live-speed measurement remains unperformed.

## Execution contract — 2026-10-12

The owner authorized implementing this corrected plan. The work item is
`codex/same-model-hedging-fix`, based on the integration branch
`feature/model-racing` through the unmerged repair and contract dependencies
`c169089` and `d338974`. Main and the primary checkout remain untouched.

- **Outcome:** preserve the client-selected model and request semantics in
  every contender, choose the first effective response, and cancel all losers.
- **Proof:** per the [test strategy](../../knowledge/testing/test-strategy.md),
  commit reviewed failing HTTP/config regressions, then run `cargo test --test
  racing --test racing_config`, the complete Rust suite, Clippy with warnings
  denied, `cargo fmt --check`, generated OpenAPI/route/field-order guards,
  agent-guide selftest/current links and `git diff --check`. Run the strict
  enforcing mock load harness with racing enabled. All keys and traffic are
  synthetic; no live NIM or production service mutation is required.
- **Constraint:** each send, including retries, uses the shared model governor
  and FIFO RPM dispatcher; sent reservations are never refunded. Disabled
  routing, authentication, deadlines, stream ownership and machine identifiers
  retain their contracts. No new dependencies, UI, deployment or quality ranking.
- **Ponytail Rung:** 2/5 — reuse owned futures, serde, the existing settings
  transaction, pool/dispatcher, bounded SSE gate and HTTP mock harness.
- **Act:** implement and verify small red-to-green slices; obtain independent
  review before each substantive commit and publish a reviewable work-item PR
  against `feature/model-racing` after local verification.

Scope decisions shown to the owner before implementation:

1. New explicit policy: `{"type":"same_model","attempts":2,
   "hedge_delay_ms":0}`. Attempts must be 1–5; delay must be 0–300000 ms.
   Zero starts all attempts; a positive delay starts the first immediately and
   each subsequent copy after the interval from the preceding launch. If all
   launched contenders fail, launch the next immediately. Pending originals
   remain eligible. A winner prevents any unlaunched copies from starting.
2. First attempt retains conversation affinity; additional copies prefer
   different currently available keys. Diversity is best effort: one-key or
   saturated pools may reuse a ready key without blocking FIFO for diversity.
   Key diversity does not imply independent NIM workers or measured speedup.
3. Buffered success requires HTTP success, valid completion JSON with no error,
   nonempty choices, assistant messages carrying content/reasoning/tool/refusal
   output, and nonempty terminal finish reasons. Streaming keeps the existing
   bounded first-meaningful-content/reasoning/tool-event predicate. Once winner
   bytes are emitted, another generation is never spliced in.
4. This intentionally retires the old multi-model policy types. Old policies
   remain parseable for an actionable validation error, but cannot be enabled
   through the API or loaded at startup. Operators explicitly replace or remove
   the optional `racing` field; no store is silently migrated or rewritten.
   Existing route names, envelopes, config version and metric names stay fixed;
   the new strategy label is `same_model`, with cancellation `winner_found`.
   The compatibility decision will be reflected in OpenAPI and release notes.

Verification environment: Windows host has no Cargo. Use the existing isolated
Rust verification container with a dedicated copy of this worktree and cached
build dependencies. Container test processes use synthetic temporary stores.

### Implementation proof log

- Fresh baseline `cargo test --locked --offline --test racing_config --
  --test-threads=4`: 6 passed. Key-diversity red check `cargo test --locked
  --offline --lib racing_ -- --test-threads=4`: 7 ran, 5 failed and 2 passed.
  Actual failures prove affinity reused `key0`, ignored key identity after
  rebuild, and did not record granted group keys. Root independently reviewed
  the delegate's tests/stubs before the red commit; no selection semantics
  were implemented in those stubs.
- Configuration red check `cargo test --locked --offline --test racing_config
  -- --test-threads=4`: 8 ran, 6 failed and 2 passed. The old build accepted
  legacy/mixed policies, started with a legacy store, and did not recognize
  `same_model`. Root independently reviewed the delegated HTTP assertions and
  retained exact save-failure boundary checks before the regression commit.
- Continuation recovered the original HTTP red output: 30 ran, 29 failed at
  the missing `same_model` configuration and one helper passed. Independent
  review approved the HTTP regressions before commit `243da65`. Those failures
  prove the missing policy contract, not every later data-plane failure path.
- First integrated green check: 30 racing and 8 configuration tests passed.
  The focused pool/dispatcher `--lib racing_` check passed all 7 tests.
  Independent source review approved config validation and key allocation;
  commit `4512bfd` records those green slices.
- Review found the buffered gate accepted partial streaming tool data as a
  terminal completion. Expanded HTTP proof ran 32 tests: 31 passed, and the
  invalid-completion test failed because the ID-only tool response won before
  the valid spare could launch. Additional passing checks prove rich streaming
  bytes, exact injection-rejection retry bytes and RPM cost, and a racing
  request sharing the governor with an ordinary stream. Root reviewed these
  delegated assertions before committing this next red regression slice.
- The complete-tool fix first passed 32 racing and 8 configuration tests.
  Independent re-review then found text/reasoning could bypass validation of
  accompanying malformed calls. A focused two-test run reproduced one failure
  and one positive-control pass. Reviewed red rows also preserve null optional
  call fields; empty call lists may accompany content but are not output alone.
- The complete Rust suite and Clippy passed after the data-plane repair.
  Strict synthetic load passed: 100 clients x 3 requests, 300 completions,
  598 upstream sends (199/199/200 across three 40-RPM keys), zero client
  failures and zero rate violations over 247.145 seconds. The wrapper verified
  the enabled two-copy policy and exactly 300 `same_model` race observations,
  then reaped every process it started. This used the optimized test profile,
  not a release build; it proves synthetic rate safety, not live NIM speedup.
- Final schema review found utoipa did not carry the enum's unknown-field
  restriction into named variant schemas. A reviewed OpenAPI regression failed
  at missing `maxProperties` (null versus 3). Since all variant properties are
  required, supported property-count caps close those schemas without custom
  schema machinery or runtime changes. Final verification follows this fix.

## Requirement audit — 2026-10-12 (before implementation)

The owner clarified the intended behavior on 2026-10-12:
“同一个模型发起多个请求，采用最先有效响应的结果”. Preserve the
client-selected model and race request attempts for that model. Cross-model
replacement, model tournaments and quality ranking do not satisfy this goal.

Integration branch remains `feature/model-racing`. The audit work item is
`codex/same-model-hedging-contract`, in an isolated managed worktree. It
continues from the unmerged repair dependency `c169089`, a descendant of the
integration branch. Both `codex/model-racing-repair` and `codex/model-hedging`
pointed at that same commit during this audit; the latter name introduces no
behavioral change. This documentation item does not integrate that dependency.

- **Outcome:** record the corrected requirement, the implementation mismatch,
  reusable mechanisms and pending acceptance criteria before behavioral work.
- **Proof:** follow the [test strategy](../../knowledge/testing/test-strategy.md).
  For this documentation edit, run `python -X utf8 scripts/check_agent_guide.py
  --selftest`, `python -X utf8 scripts/check_agent_guide.py`, explicitly apply
  its `local_link_problems` helper to this plan, run `git diff --check`, and
  obtain independent review. Rerun the existing racing suites only to establish
  the present implementation; they cannot prove the corrected requirement.
- **Constraint:** preserve historical evidence and current executable behavior.
  Every future attempt must keep the requested model and request semantics,
  obey per-key RPM and the shared per-model governor, and release losers without
  refunding already-sent requests. Existing API, store and metric contracts
  require an explicit compatibility decision before change.
- **Ponytail Rung:** 2/5: reuse the dispatcher, governor, owned futures, bounded
  SSE gate, deadlines, settings transaction and local mock harness.
- **Act:** publish this scope delta locally, validate the documentation and
  record independent review. Behavioral implementation remains future work.

### Findings from the current implementation

- `RaceRequest::attempt_inner` in [proxy.rs](../../src/proxy.rs) overwrites
  `request["model"]` with each configured candidate. Enabled racing therefore
  does not preserve the client-selected model.
- [racing_config.rs](../../src/racing_config.rs) requires unique model IDs;
  [configuration tests](../../tests/racing_config.rs) explicitly reject repeated
  IDs and a primary repeated as fallback. Repeating one model in the current
  configuration is not a workaround.
- `Smart` in [racing.rs](../../src/racing.rs) cancels the primary before starting
  fallbacks. If delayed hedging is chosen, the original pending attempt must
  remain eligible while another copy starts. The launch policy is not yet chosen.
- Conversation affinity currently gives identical model/messages the same
  preferred key. Removing uniqueness alone would not guarantee separate keys.
  Lane diversity needs an explicit policy compatible with FIFO admission and
  [affinity](../../knowledge/decisions/sticky-affinity-with-spillover.md).
- The streamed gate already waits for meaningful content, reasoning or tool
  data. Buffered candidates currently require HTTP success and a fully read
  body, without validating a successful completion JSON payload. Define
  “effective response” explicitly for both transports before implementation.

### Acceptance work identified at audit time (historical)

- [ ] Define attempt count, immediate versus delayed launch, and whether spare
  attempts require distinct available keys. Do not assume separate keys mean
  independent NIM model workers or guaranteed lower latency.
- [ ] Keep the original model, messages, tools and sampling parameters in every
  attempt, subject only to already-defined proxy transformations. Use attempt
  identity rather than model identity to distinguish participants.
- [ ] Specify the buffered success predicate and streamed first-effective-event
  predicate, including reasoning/tool deltas and malformed/error responses.
  After winner bytes are emitted, do not splice in a different generation.
- [ ] Commit red regressions proving identical model/payload across attempts,
  correct winner selection, loser cancellation, shared governor/RPM admission,
  deadline/disconnect cleanup and disabled-path preservation; then make them
  green with the minimum change. If delayed launch is selected, prove the
  original can still win after the extra attempt starts.
- [ ] Decide compatibility for the existing racing API, durable policies,
  OpenAPI and metrics; document any intentional breaking change with regression
  evidence and a release note. Do not silently reinterpret a multi-model store.
- [ ] Independently review and run the relevant tests and strict mock load proof
  from the test strategy. Live NIM speedup remains unmeasured.

### Audit evidence and delivery state

Fresh execution on 2026-10-12 against `c169089` in the existing isolated Rust
verification container: `cargo test --locked --offline --test racing --test
racing_config -- --test-threads=4` passed 23 racing tests and 6 configuration
tests, with no failures or ignored tests in those suites. The duplicate-model
rejection remains a passing assertion under the old contract.

The primary checkout still points at `feature/model-racing` / `41d77d5`;
the repaired code lives in `.worktrees/model-racing-repair` at `c169089`.
Fresh `cargo check --locked --offline` in the primary checkout failed with
8 compile errors and 4 warnings. The compiler reports type mismatches, missing
HTTP client API support, unsupported `Instant` serialization/deserialization
and a non-`Debug` stream. This confirms the primary checkout is not the repaired
implementation validated by the racing suites above.
The repair worktree was clean at audit time. The fork PR list returned no PRs.
No deployment or production request was performed by this audit. The full Rust
suite, Clippy, release CI and strict load test were not rerun for this
documentation-only correction; earlier records below retain their original scope.

Documentation proof passed: all 12 guide selftest fixtures demonstrated their
named failure, the guide/current-link check passed, this plan's local links
passed the existing helper, and `git diff --check` passed. An independent agent
reviewed all five changed Markdown files and approved the requirement/current-
behavior distinction and the explicitly pending implementation decisions.

## Historical repair contract — 2026-10-10

The owner requested repair after the handoff audit reproduced eight compile
errors and found placeholder upstream functions, disconnected racing, ordered
winner selection, detached loser tasks, and unvalidated volatile configuration.
Integration branch: `feature/model-racing`. Work item:
`codex/model-racing-repair`, isolated in `.worktrees/model-racing-repair`.

- **Outcome:** deliver the existing opt-in full-parallel, smart fallback and
  streaming TTFT strategies through `POST /v1/chat/completions`, with durable
  admin-managed configuration and truthful race observations.
- **Proof:** preserve the compiler failure as the build regression; commit
  behavioral tests before implementation and observe red then green using
  `cargo test --test racing` and `cargo test --test racing_config`. Then run
  `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`,
  generated OpenAPI and route-contract tests, documentation/link checks and
  `git diff --check`, following the test strategy. All HTTP tests use synthetic
  stores and a local mock; no production key or NIM request is needed.
- **Constraint:** disabled requests keep their existing route and wire behavior;
  each participant uses the existing dispatcher and model governor; cancellation
  cannot refund an already-sent RPM slot. Preserve absolute deadline and stream
  disconnect ownership. Keep main, the running service, the sibling localization
  branch, stash and untracked reports unchanged.
- **Ponytail Rung:** 2/5: reuse settings::commit, reqwest, FuturesUnordered,
  dispatcher/governor and the existing test harness, without new dependencies.
- **Act:** compile-only repairs first, behavioral red checks next, implementation
  and independent review before final commit. Root owns integration and proof;
  delegates own control-plane, reusable foundation and independent E2E tests.

Scope delta shown to the owner before implementation: complete the disconnected
feature through the existing chat endpoint and its existing racing management
API. No new `/v1/race` route, per-request race override, Dashboard controls,
quality-based model selection, live benchmarking, deployment or localization
merge is part of this repair. Configuration is disabled by default, limited to
five unique models, and serialized as an optional store field (absent when off).
For streaming, all enabled strategies select the first valid content, reasoning
or tool-call delta; comments, empty deltas and terminal errors cannot win.
Smart fallback cancels the timed-out primary before starting fallback contenders.

### Fresh proof record

- Audit at `41d77d5`: `cargo test --locked --offline` failed compilation with
  eight errors and four warnings; `cargo fmt --check` failed. No Rust test ran.
  Locale, i18n, source syntax/assets and agent-guide guards passed. Historical
  reports of speed gains are not performance acceptance evidence.
- Regression commits `bd76fa0` and `df21970` preserve the original behavioral
  failures. Continuation initially passed 221 library tests, 15 racing HTTP
  tests, 6 racing configuration tests and 119 existing E2E tests (one existing
  release-only test ignored).
- Independent review identified SSE framing and post-selection observation
  defects. The expanded HTTP suite reproduced seven failing tests (16 passed):
  CR-only framing, no-space `event:error`, successful classification after a
  winner's error, the pre-token byte bound counting post-token bytes, and three
  checks preserving the historical `ttft_winner` cancellation label. Commit
  `945f256` preserves those regression cases. All seven failures are repaired.
  This stays within the
  original truthful-observation and streaming contracts.
- Independent source review: foundation/client/measurement repair approved by
  the foundation reviewer (8 performance and 3 client tests passed). The same
  independent reviewer approved the config validation, serialized publication,
  authorization ordering and typed API contract. Final data-plane re-review
  approved the SSE fixes and found no remaining blockers.
- Final exact working-tree proof in the isolated Rust container:
  `cargo test --locked --offline -- --test-threads=4` passed **378 tests**:
  221 library, 119 existing E2E, 9 OpenAPI, 23 racing, 6 racing-config.
  The existing `release_restart_and_idle_history` release-only test remains
  ignored (one skipped). No test failed.
- `cargo clippy --locked --offline --all-targets -- -D warnings` and
  `cargo fmt --check` passed. Agent-guide selftest passed all 12 negative
  controls; current links, both locale catalogs, 452 i18n references and
  `git diff --check` passed. Windows locale checks ran with Python's `-X utf8`
  because the host's default GBK decoder cannot read the UTF-8 catalogs.
- The resulting 21-operation OpenAPI file matches code generation. Operator
  documentation and architecture memory explain opt-in model replacement,
  increased RPM use, cancellation, sample limitations and configuration.
  Historical draft speedup/HTTP protocol claims are explicitly withdrawn as
  acceptance evidence.

### Completed scope and delivery boundaries

- [x] Compile and connect all three strategies to chat POSTs.
- [x] Preserve default routing, limiter/governor, cancellation and deadlines.
- [x] Persist/admin-gate/validate configuration and reject strict conflicts.
- [x] Count actual TTFT samples; reject erroneous/incomplete stream successes.
- [x] Independently review and pass the complete local proof set above.
- [x] Update generated API contract, usage docs, knowledge and release notes.

Delivery remains on `codex/model-racing-repair` for review against
`feature/model-racing`. No production deployment, NIM performance benchmark,
localization merge, remote push or PR publication was performed. Release CI
coverage/MSRV/security jobs were not run locally; they remain publication
gates. The running service, main branch, sibling localization work, stash and
original untracked reports are outside this repair.

## Archived initial design (superseded by the repair contract above)

The following draft preserves design provenance. Its proposed endpoint,
deferred streaming scope, and unchecked draft tasks are not current work.

### Authority and Scope

Implementation of model racing/tournament functionality to allow concurrent requests to multiple models with first-response-wins semantics. This addresses the use case mentioned in Rambler's model tournament where comparative model performance evaluation is needed.

## Background

The zh-CN handoff plan noted: "Model-racing disposition: defer implementation. The inherited research reports additional per-key request consumption and a model-routing contract change; it supplies no measured benefit for this deployment."

This branch will explore the design and implementation of model racing as a separate, opt-in feature.

## Use Cases

1. **Model Tournament**: Send the same prompt to multiple models simultaneously, return the first successful response
2. **Performance Benchmarking**: Compare response times and quality across models
3. **Fallback Strategy**: Primary model with automatic fallback to alternatives on timeout
4. **Quality Comparison**: A/B testing for model selection decisions

## Design Considerations

### Key Questions to Answer

1. **Request Routing**: How to fan out one client request to multiple upstream models?
   - New `/v1/race` endpoint vs extending existing `/v1/chat/completions`?
   - Client-specified model list vs server-configured tournaments?

2. **Response Strategy**: Which response to return?
   - First successful response (speed)
   - Best quality (requires evaluation criteria)
   - All responses (for comparison)

3. **Resource Management**: How to handle multiple concurrent requests?
   - Impact on rate limits (multiplies key consumption)
   - Governor behavior with racing requests
   - Cancellation of slower responses after winner

4. **Metrics and Observability**: What to track?
   - Per-model latency in race
   - Win/loss statistics
   - Resource consumption
   - Quality metrics if applicable

5. **Streaming Support**: How to handle streaming responses?
   - First token wins?
   - Full response comparison?
   - Mixed streaming/buffered?

### Architecture Options

#### Option A: Separate Race Endpoint
```
POST /v1/race
{
  "models": ["model-a", "model-b", "model-c"],
  "strategy": "first-wins",
  "messages": [...]
}
```

**Pros:**
- Clean separation of concerns
- Explicit opt-in
- Can have race-specific configuration

**Cons:**
- Not OpenAI-compatible
- Clients need special support

#### Option B: Header-Based Racing
```
POST /v1/chat/completions
X-Nim-Proxy-Race: model-a,model-b,model-c
X-Nim-Proxy-Race-Strategy: first-wins
{
  "model": "model-a",  // primary/fallback
  "messages": [...]
}
```

**Pros:**
- Backward compatible
- Minimal client changes
- Works with existing tools

**Cons:**
- Mixing concerns in existing endpoint
- Header parsing complexity

#### Option C: Internal Tournament Mode
Server-side configuration for automatic racing without client awareness.

**Pros:**
- Zero client changes
- Centralized policy

**Cons:**
- Hidden behavior
- Resource consumption not visible to client

### Cost and Trade-offs

**Resource Consumption:**
- Racing multiplies upstream requests (3 models = 3x requests)
- Rate limit consumption increases proportionally
- More key slots occupied during race

**Benefits:**
- Reduced perceived latency (first wins)
- Fallback resilience
- Performance data for model selection
- Quality comparison data

## Proposed Initial Design

### Phase 1: Foundation (this branch)

1. **New Race Endpoint**
   - `POST /v1/race` with OpenAI-compatible body + race config
   - Support buffered responses initially (streaming in Phase 2)
   - Strategy: `first-wins` (return first successful response)

2. **Request Fan-out**
   - Spawn parallel upstream requests
   - Use existing dispatcher/governor for each
   - Cancel remaining requests when first succeeds

3. **Metrics**
   - `nimproxy_race_requests_total{strategy}`
   - `nimproxy_race_model_wins_total{model}`
   - `nimproxy_race_latency_seconds{model,position}` (winner, 2nd, 3rd, etc.)
   - `nimproxy_race_cancelled_total{model,reason}`

4. **Configuration**
   - Opt-in feature flag
   - Maximum models per race (default 5)
   - Timeout for race (use existing deadline mechanism)

### Phase 2: Advanced Features (future)

- Streaming support
- Quality-based selection (requires response evaluation)
- Server-side tournament configuration
- Statistical winner selection (e.g., best 2 of 3)

## Open Questions

1. **Request Body**: Should all models see identical request bodies, or can we vary sampling params?
2. **Error Handling**: If all models fail, which error to return?
3. **Observability**: How detailed should race metrics be? Store all responses?
4. **Cost Control**: Should there be per-user race limits?
5. **Upstream Compatibility**: Any NIM-specific considerations?

## Success Criteria

- [ ] Race endpoint accepts request and fans out to N models
- [ ] First successful response returned to client
- [ ] Slower responses cancelled properly
- [ ] Metrics track wins/losses/latencies
- [ ] Resource consumption (keys, slots) properly managed
- [ ] E2E tests cover racing scenarios
- [ ] Documentation explains feature and trade-offs

## Deferred / Out of Scope

- Streaming race responses
- Quality-based selection
- Automatic tournament mode
- Response caching/deduplication
- Cost optimization strategies

## Related Decisions

- [explicit-request-deadline](../../knowledge/decisions/explicit-request-deadline.md) - Can reuse deadline mechanism
- [governor](../../knowledge/architecture/governor.md) - Need to understand interaction with model limits
- [key-pool](../../knowledge/architecture/key-pool.md) - Race multiplies key consumption

## Implementation Tasks

- [ ] Design API contract (request/response schemas)
- [ ] Implement race handler with fan-out logic
- [ ] Add cancellation for slower responses
- [ ] Implement metrics collection
- [ ] Add E2E tests
- [ ] Document feature in README and API docs
- [ ] Consider OpenAPI schema updates
