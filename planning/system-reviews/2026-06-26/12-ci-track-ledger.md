# Make-It-Real Ledger — Phase 4: the Actions/CI subsystem track (E2.1–E2.5)

Date: 2026-06-29. Status: PLAN. The spine + the sandbox-independent Git daily driver (GT-001..005) are done.
This decomposes **Actions/CI — "the long pole"** (roadmap priority #2), grounded in the MR-002 census + the
shortcut-inventory CI CRITICALs (SI-016/017) + MR-006 RESHAPE-001 + roadmap E2.1–E2.5. Per the master plan:
**no rush; get the sandbox right before trusting it with your supply chain.** CI runs your own build +
dependency code, so a weak sandbox is a supply-chain hole.

**Execution reconciliation (2026-07-24):** this file remains the decomposition; the live source of
truth is release ledger 14 §R4.2. CT-004 now includes the opt-in coordinated production runner and the
durable CI→Git check projection. Attempt numbers are allocated immutably per run/context in the same
reserve commit as the run, queued outbox rows, and trigger dedup; PipelineStarter consumes rather than
reallocates them. CT-004 remains in progress until a real founder push produces, settles, and surfaces
its exact-head check. CT-005 is now **IN PROGRESS**: CT-005a mounts production durable run-list and
run-detail reads at Edge, prefiltered through the parent Git repository's Pull visibility and backed by
an opaque scope-bound keyset cursor plus a ready-at-boot index. CT-005b adds byte-exact bounded archived
log reads over sealed `log_segment` rows and the production content-addressed BlobStore. CT-005c adds
the authenticated durable web list/detail/archive surface and binds its dev Edge to the same
request/response vectors as the production Rust integration through the permanent contract-coverage
gate. CT-005d adds the thin authenticated CLI list/view/archive client over those same Edge routes
and executes the shared response vectors through the compiled binary. Its request-bound success
decoder rejects malformed list/detail/log bodies before either human or JSON rendering while
preserving the production beyond-end empty-range contract. It deliberately does not call the
process-local `LiveTail` an SSE transport. CT-005e projects CI's two implemented durable reads from
their shared `ToolDef`s into MCP and routes them through the same permission-checked Edge/CI adapter,
under a per-run token re-verified at the final read boundary. Cross-service resumable live tail
now has its CT-005f consumer/resume half: production Edge tails the durable T3 segment sequence
through repository-authorized SSE with strict `Last-Event-ID`, retention-gap refusal, and bounded
pointer-only frames. CT-005f2 now supplies the producer half: both production sandbox backends emit
bounded byte-exact frames while the command is still running, and the runner acknowledges each frame
only after durable segment persistence. Authenticated web/CLI live consumption, the production-path
CI-D11 sever/resume drill, and the founder acceptance pass remain open.
CT-007 is still unopened; GitHub Actions must not be
removed before the founder acceptance pass and the complete CT-005 surface are genuinely usable.

## Environment (confirmed — this track is testable here)
Firecracker v1.16.0 + gVisor (`runsc`) on PATH; `/dev/kvm` present. So the production microVM boot + the
AG-D4 escape corpus run for real here (set `MYELIN_REQUIRE_KVM=1` so a real guest must boot — no skip).

## The census CRITICALs this track closes
- **RESHAPE-001 (MR-006):** `SandboxHandle{guest_id}` + `launch()->Result<SandboxHandle>` can't carry a
  command's exit/stdout/stderr/usage; the runner does launch→kill with `TerminalReport` passed IN. The
  result/lifecycle seam must be redrawn → CT-001.
- **SI-016** sandbox `launch()` is a no-op in prod (Firecracker `init=/bin/true`; gVisor only probes
  `runsc --version`; `spec.command` never runs) → CT-002.
- **SI-017** the AG-D4 escape corpus runs through separate drill harnesses, NOT the production `launch()` —
  so "0 escapes" certifies a path real jobs never take → CT-003.

## Conventions
Same as the spine/Git ledgers: `CT-NNN` ids; anti-duplication grep + ledger-vs-commits cross-check opens every
prompt (reuse `hardening.rs::HardeningProfile`, `firecracker.rs`, `gvisor.rs`, `escape_corpus.rs`, the runner,
+ the MR-014/015 edge + MR-019 shell + MR-020/021 CLI/MCP — extend, never fork); orchestrator runs the FULL
gate; **independent SECURITY verification** on every prompt — and for CT-002/003 the verifier runs the escape
corpus THROUGH the production path itself (a sandbox escape = supply-chain compromise; the verifier tries to
escape). Commit per prompt. **No green without a real microVM boot** (`MYELIN_REQUIRE_KVM=1`).

## The CI-track prompt set

| ID | Epic | Title | Deps | Size |
|---|---|---|---|---|
| CT-001 | (RESHAPE-001) | **Redraw the sandbox launch/result/lifecycle seam:** `launch → run(spec.command) → wait → SandboxResult{exit, stdout, stderr, usage, timed_out}` (+ settle-once); the seam both Firecracker + gVisor backends and the runner implement. Sandbox-INDEPENDENT (a Rust type/API redraw; no exec yet). | — | mid |
| CT-002a | E2.1 (P-544) | **Firecracker PRODUCTION exec** (the DEFAULT backend; CI-P2 = the one through the escape drill FIRST): a real `spec.command` runs in a real microVM (NOT `init=/bin/true`) — reuse the proven `drill_config_json` recipe (2nd read-only virtio drive + `init=/bin/bash /dev/vdb`) with a command-runner init script; capture exit/stdout/stderr (bounded by `SANDBOX_CAPTURE_BOUND`) + usage from the serial console + `VmmChild::wait`; `spec.limits.timeout_secs` kills the whole guest (`timed_out=true`); settle once. Real boots (`MYELIN_REQUIRE_KVM=1`). | CT-001 | high |
| CT-002b | E2.1 (P-544) | **gVisor PRODUCTION exec** (the NAMED SECOND backend, CI-P28): real `runsc run --bundle` of the OCI bundle built from the spec (`OciConfig::from_spec` + a rootfs) — NOT a `--version` probe; same `SandboxResult` capture convention + timeout→whole-guest-kill as CT-002a. Real `runsc run`. | CT-002a | high |
| CT-003 | E2.2 (P-545) | **Production-path escape verification:** re-run the **AG-D4 escape corpus through the PRODUCTION `launch()`** on both backends — **0 escapes** — with a guard that fails RED if a case is routed to the harness shortcut instead of the prod path. The supply-chain-safety proof. | CT-002 | high |
| CT-004 | E2.3 | **CI backend HARDEN + RECONCILE** (ci-controlplane / dispatch / sandbox): durable pipeline/run/step state, the scheduler/lease/metering, the log pipeline — on the durable substrate (MR-022). | CT-002 | high |
| CT-005 | E2.4 | **CI API + UI + CLI/MCP:** pipelines / runs / live log-tail (SSE) through the edge (MR-014/015) + the web UI (MR-019) + the CLI/MCP (MR-020/021); reuse the CI ViewSpec. | CT-004 | high; may split (API/UI vs CLI/MCP) |
| CT-006 | (GT-006) | **The Git smart-transport WIRE** (`upload-pack`/`receive-pack` = real `git clone`/`push`/`fetch`) via the now-hardened sandboxed git + the production `WireExecutor` + the git server binary/listener + the external-oracle test (real `git clone`/`push` + `git fsck`). UNBLOCKED by CT-002. **DONE** — split into: **CT-006a** sandbox git-wire capability (RO repo mount + quarantine + stdin/stdout, confined) `15b01a3`; **CT-006b** production WireExecutor + GitCore wiring, clone/fetch through the seam `2ce5742`; **CT-006c** wire stdout streaming + HTTP upload-pack server + external-oracle CLONE/FETCH (real `git clone` works) `1fe630d`; **CT-006d** PUSH path (rootless quarantine via ingest-not-receive-pack + in-process fsck/policy + one-tx ref-CAS/outbox + external-oracle PUSH) `fd0eb19`. All security-verified. Follow-ups: per-repo authz seam (task #60, platform-wide), orphan-on-reject (task #61, bounded). | CT-002, GT-001 | high |
| CT-007 | E2.5 | **Cut over from GitHub Actions** — move CI off GitHub Actions onto the hardened sandbox. ONLY after CT-003 (0 escapes through the prod path) is green. The reward AFTER the work. | CT-003, CT-004, CT-005 | mid |

## Waves
- **W1:** CT-001 (seam redraw — sandbox-independent)
- **W2:** CT-002a (Firecracker prod exec — real boots) → CT-002b (gVisor prod exec — real `runsc run`)
- **W3:** CT-003 (escape verification, 0 escapes through prod) · CT-004 (CI backend harden) · CT-006 (the git wire, unblocked)
- **W4:** CT-005 (CI API+UI+CLI/MCP)
- **W5:** CT-007 (cut over — only after the sandbox is genuinely hardened)

## CT-005 execution increments

- **CT-005a — durable run reads:** authenticated repository-authorized run list/detail, opaque
  scope-bound keyset cursor, repeatable-read detail snapshot, and exact boot index readiness.
- **CT-005b — bounded archived logs:** authenticated
  `GET /v1/ci/runs/{run}/jobs/{job}/log?start=<byte>&limit=<bytes>` resolves the exact run parent,
  inherits Git Pull authorization, selects only overlapping sealed segment refs in one tenant-scoped
  repeatable-read transaction, then reads each content address through BlobStore's metadata bound and
  re-hash-on-read integrity gate. The response is base64 so arbitrary byte ranges never corrupt UTF-8.
  Missing, malformed, corrupt, oversized, gapped, overlapping, or over-fragmented archives fail
  generically; absent jobs and denied parents remain the same 404. The default range is 64 KiB and the
  hard response cap is 256 KiB.
- **CT-005c — durable web reads and executable mock parity:** the authenticated SolidStart surface
  lists and filters authorized runs, pages only through Edge-issued opaque cursors, renders exact
  run/job/step materialization, and reads archived output in bounded byte ranges through server-only
  gateway queries. Empty, unavailable, absent-or-denied, malformed, and visibility-stale states are
  distinct and leak-free. A committed golden artifact is executed by the production Rust Edge
  integration and the dev Edge contract test; `contract-coverage.toml` binds both to the browser
  suite. Its visible-repository vector includes unsorted ASCII, composed/decomposed Unicode, emoji,
  and a duplicate; both implementations must produce the same UTF-8 byte order/deduplication, and a
  real membership addition invalidates the prior cursor. Six Chromium flows cover login refusal,
  list/filter/detail/archive reads, next-page and
  Back, stale 409 recovery, failure postures, mobile layout, and accessibility. The UI states that
  live updates are unavailable; no polling or SSE capability is implied.
- **CT-005d — durable CLI reads:** `myelin ci list [--status] [--limit] [--cursor]`,
  `myelin ci view <run>` (with `show` as the ordinary read alias), and
  `myelin ci logs <run> --job <job> [--start <byte>] [--limit <bytes>]` reuse a total
  CI-owned grammar and call only the authenticated CT-005a/b Edge routes. Human output always pairs
  a glyph with a state word, emits parser-round-trippable next-page/archive commands only for
  canonical opaque cursors and UUIDs, and renders arbitrary archived bytes without terminal-control
  injection; `--json` preserves the exact Edge envelope. The compiled CLI executes list/detail/log
  responses from the same committed golden artifact as Rust Edge and dev Edge and asserts its exact
  request targets. `ci watch` fails locally and names the missing cross-service resume authority.
  The frozen architecture's branch/actor filters and line/step ranges are not fabricated: the
  current durable API has only the indexed state filter plus job byte ranges.
  Every CI 2xx response is decoded against the exact originating call before rendering: list
  filter/limit and canonical cursor frame, detail run/DAG/step integrity and bounded enums/times,
  and log run/job/range/base64/byte length must agree. The production-valid empty response for a
  start beyond `total_end` remains accepted and never invents a continuation.
- **CT-005e — durable MCP reads:** `ci.read_run` and `ci.read_log` are the only CI ToolDefs marked
  `exposed_over_mcp`; MCP projects their exact subsystem-owned schemas, `run.view` requirement,
  read effect kind, non-side-effecting posture, and non-HITL default. Reads lazily mint and consult
  the session run token, bind the exact declared capability, bypass mutation idempotency and
  `EffectApi`, then re-verify the signed token, subject, tenant/region scope, durable S7 liveness,
  and capability at the concrete adapter immediately before the shared durable Edge/CI read.
  Parent visibility remains Git Pull; denied and absent objects remain indistinguishable; archived
  logs retain the same bounded content-addressed integrity path. Git's older catalogue stays behind
  an explicit compatibility adapter, while every unimplemented CI tool remains absent.
- **CT-005f1 — durable live-log consumer/resume transport:** production Edge mounts
  `GET /v1/ci/runs/{run}/jobs/{job}/log/live` under `run.view` plus the parent repository's exact
  Pull decision. The durable T3 `segment_seq + 1` is the SSE cursor; an omitted
  `Last-Event-ID` emits an ID-bearing current-head checkpoint without historical output, while an
  explicit cursor replays strictly after it.
  Each bounded frame carries only run/job and byte-range coordinates; bytes remain behind CT-005b's
  content-addressed integrity reader. Edge rechecks repository authorization on every poll,
  returns the denied/absent 404 posture at open, conservatively refuses every explicit cursor over
  an empty retained set and every old retention cursor with 409, and fails closed on internal
  sequence or byte-coordinate discontinuity. Head/floor, predecessor, and next-64 reads are
  index-oriented and work-bounded per poll. The one-connection producer is bounded; lag disconnects
  so the client must resume rather than silently lose frames. A live
  PostgreSQL CDC registered on contracts 3.5 and 11.8 proves initial backfill, current-head
  checkpoint, post-subscribe append across the service boundary, live Pull revocation, terminal
  completion, reconnect, hidden/absent equivalence, partial/full retention staleness, and
  within-/cross-batch gap refusal through the actual second poll.
- **CT-005f2 — bounded incremental production persistence:** `SandboxBackend` now accepts one
  output sink and cancellation signal. gVisor drains stdout/stderr into byte-exact 64 KiB frames;
  Firecracker emits nonce-qualified base64 serial frames so payload bytes cannot forge the control
  stream. Both real backends prove a durable frame arrives before command exit, preserve exact
  binary bytes, keep only the bounded capture head in memory, and cancel the complete runtime group
  when persistence fails. The runner uses an eight-frame synchronous channel and one serial durable
  consumer: each frame reaches `LogPipelineSink` before acknowledgement, so a fast producer cannot
  outrun durable storage. PostgreSQL append positions serialize on the exact stream generation and
  replay immutably without overlapping offsets or anchors.
  A durability failure after launch is an infrastructure retryable attempt, never an ordinary job
  verdict. Measured usage accrues in fixed-size durable queue state and is folded into the eventual
  terminal settlement exactly once. Supersession and retry serialize on Flow authority; claim and
  generic reap require active Flow/CI owners, so cancelled work cannot resurrect. An expired
  cancelled launch is instead reconciled under its exact tenant and queue generation using the
  immutable manifest resource ceiling. Cancelled recovery is a rotating keyset page of at most 64
  candidates, isolates poison rows, and reports accumulated failures only after later candidates
  have been attempted.

**Named CT-005b floor:** `LiveTail`/Firehose is process-local while runners and Edge are separate
services. It is not an honest production SSE resume source. SSE remains open until a real
cross-service bounded resume transport exists; CT-005b/CT-005c claim archived cold-path reads only.
The live founder push→settled check→surfaced archived/live log pass and CT-007 also remain open.
CT-005e closes the prior MCP floor without inventing live output: its two reads are exact durable
run/detail and archived-log operations, and the complete content-addressed Agent tool transcript
remains an Agent-runtime trace artifact rather than a claim made by the stdio transport.
CT-005f1 and CT-005f2 close the durable consumer/resume and producer-persistence halves without
overstating the remaining surface. The real gVisor and Firecracker paths now persist bounded output
while commands execute. The web and CLI still need authenticated live-tail UX over the existing Edge
route, and CI-D11 must sever and resume the composed production path across service boundaries. The
live founder push→settled check→surfaced archived/live log pass and CT-007 also remain open.

The danger concentrates in CT-002/003 (untrusted execution + escape verification). Those get a security
verifier that actively tries to escape the production sandbox; "0 escapes" is only credible THROUGH the prod
path (CT-003's guard enforces that). CT-007 (the GitHub-Actions bill killer) is the reward, gated on CT-003.
