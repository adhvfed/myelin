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
reallocates them. CT-004's live founder handoff is now closed by CT-005f8c below: a real Myelin
source-snapshot push produced, settled, and surfaced its exact-head trusted check. CT-005 is now
**DONE**: CT-005a mounts production durable run-list and
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
only after durable segment persistence. CT-005f3 adds the authenticated web consumer: a same-origin
session-only proxy performs one bounded refresh/retry, the browser snapshots the durable archive before
acknowledging SSE cursors, and stale retention cursors reload the archive before a fresh subscription.
The dev Edge's actual live state machine and production Rust Edge execute the same committed golden
vectors, including fresh, replay, terminal, ahead-of-head, and pruned-cursor branches. Authenticated CLI
live consumption is now closed by CT-005f4: the compiled thin client snapshots archived bytes, consumes
the pointer-only stream, fetches every appended range before acknowledging its cursor, and catches up
the archive before clearing a retention-stale cursor. CT-005f5 now closes the production-path CI-D11
committed-prefix sever/resume drill. CT-005f6 adds the bounded, no-overwrite founder verifier that
re-reads the terminal run and complete archive through Edge and emits a checksum-bearing receipt only
when the independently captured live output and archive contain the exact marker once. CT-005f7 checks
in the V2 one-job founder pipeline, executes its real file through production Dispatch planning into
the decoded CAS snapshot, and refuses runner activation unless the staged personal-cell rootfs matches
the authored digest. CT-005f8 then proves the composed push-to-terminal path with an authorized
disposable repository and closes the production-only elected-publisher, subject, immutable-read,
activation, log-route, terminal-job, and SSR-output defects that the rehearsal exposed. CT-005f8c
records the founder's real Myelin source-snapshot push, exact-head trusted check, live/archive marker
receipt, browser pass, and the dogfood defects repaired before this surface was called usable.
CT-007 is still unopened; GitHub Actions must not be removed until its pre-registered workload-parity
gate below is satisfied.

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
  live updates are unavailable; no polling or SSE capability is implied. That statement records the
  CT-005c boundary; CT-005f3 subsequently replaces it with the authenticated durable SSE consumer.
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
- **CT-005f3 — authenticated web live-log consumer and executable resume parity:** the run-detail
  surface now keeps bounded recent live output beside the existing archived-range reader. A
  same-origin server route obtains credentials only from the encrypted session, forwards a
  canonical `Last-Event-ID`, and permits exactly one access-token refresh plus one retry before
  clearing a failed session. The browser uses an explicit fetch stream so it can distinguish 409:
  it snapshots and validates the durable archive before acknowledging a cursor, reads through every
  pointer range serially before advancing, and on a stale cursor reloads the archive before opening
  a fresh subscription. Initial acquisition and transient disconnects retry with bounded backoff;
  malformed, oversized, discontinuous, cross-scope, or truncated SSE fails closed. The recent
  in-memory window is capped at 256 KiB; the complete log remains in the durable archive.
  The dev Edge HTTP server and its contract test call the same `ciLiveOpen` state machine. Shared
  golden vectors execute fresh current-head, explicit replay, terminal completion, ahead-of-head
  400, and pruned-cursor 409 through both that state machine and the production Rust Gateway.
  Eight Chromium CI flows include the previously missing tree next-page click and stale list-cursor
  reload, plus forced live reconnect 401→one refresh, append, stale live-cursor 409→archive reload,
  resume, and completion.
- **CT-005f4 — authenticated CLI live-log consumer:** the CI-owned total grammar now admits
  `ci watch <run> --job <job>` and maps it to the exact job-scoped durable Edge stream. The CLI
  snapshots and validates the archive before subscribing, reads every pointer range through the
  bounded integrity-checked archive route before acknowledging its event id, resumes from only the
  last acknowledged canonical cursor, and catches the archive up before clearing a retention-stale
  cursor. SSE frames, error bodies, reconnects, archive chunks, and recent transport state are
  bounded; malformed scope, ranges, cursor transitions, media types, success statuses, and fresh
  409s fail closed. Human bytes and every error field are terminal-safe; `--json` emits validated
  archive envelopes as NDJSON. The compiled binary executes the shared terminal live vector plus
  abrupt-body disconnect/resume and stale-cursor recovery without duplicate bytes.
- **CT-005f5 — composed production CI-D11:** a live PostgreSQL + S3/RustFS drill writes the first
  frame through the production `LogPipelineSink`/`DurableLogPersist`, observes pointer 1 through the
  authenticated production Edge, then destroys both sink and Gateway. A reconstructed sink recovers
  the durable sequence/byte head and appends the second frame; a reconstructed Gateway resumes from
  `Last-Event-ID: 1` and emits only pointer 2. The ordinary archive route returns the exact
  concatenated bytes and PostgreSQL independently contains exactly two contiguous segments. This
  proves the committed-prefix sever boundary; it does not claim a kill inside CAS/PG commit,
  commit-unknown replay, or HTTP-wire serialization.
- **CT-005f6 — mechanical founder-acceptance receipt:** `dogfood.sh verify-ci` accepts only the real
  pipe-delimited capability shape over verified HTTPS or exact numeric-loopback HTTP, disables
  inherited curl tracing and proxies, and bounds every response. It requires the exact succeeded,
  cost-settled run/job, walks at most 256 full non-final 256 KiB pages under stable totals and exact
  continuation coordinates, canonical-decodes each Base64 body, and caps the archive at 64 MiB.
  Existing, linked, partial, malformed, or cross-scope evidence cannot produce the checksum-bearing
  receipt. Seven compiled Bash/curl/socket contracts cover the green two-page path plus transport,
  paging, encoding, and no-overwrite refusals with zero credential disclosure.
- **CT-005f7 — armed founder pipeline:** the real repository now carries a V2 `on = "push"`
  `linux-small-v1` pipeline with one `build` job that emits the acceptance marker exactly once. The
  checked-in command first emits a non-marker readiness line and holds a bounded 120-second
  observation window so the required CLI and browser consumers can attach before terminal output. Its
  compiled contract reads the checked-in file, drives a canonical `git.ref.updated` through
  `plan_dispatch`, requires one queued CI `build` context, then reads and decodes the emitted CAS
  snapshot and verifies the exact profile, image, and command. Before intake, `dogfood.sh ci`
  canonical-hashes the staged personal-cell rootfs and refuses activation unless it matches the
  authored image pin. The local founder repository's `refs/heads/main` ruleset has been set through
  the authenticated production API to require `ci/build`; the operator runbook preserves that
  context while reducing solo-founder approvals to zero, so `verify-check ... build` cannot be made
  vacuous by an empty required set. A focused required-runsc production test also confirms this host
  executes and captures an ordinary shell command through the real gVisor backend. The immutable
  release-grade image/provenance mechanism remains explicitly sequenced in P0; this one-cell binding
  is the honest R4 founder-dogfood bridge.
- **CT-005f8 — composed production push rehearsal:** an authorized disposable repository push runs
  through the real Git wire, elected outbox publisher, Dispatch, coordinated gVisor runner,
  Controlplane, PostgreSQL/S3 log store, Git projection consumer, CLI, and authenticated browser.
  The terminal counted run/job are `e5e79ca1-c8fb-4a0f-ce02-c686e7eba714` /
  `7d6b3d1c-4a9e-86d9-9e9a-1e3bb1906860`; the acceptance verifier observes one identical marker in
  the independently captured live and 54-byte archived streams. Failed rehearsals remain negative
  evidence and forced repairs to the elected publisher composition, canonical event identities,
  runtime-role-compatible immutable reads, source-pinned workflow activation, CI-run log routing,
  terminal public job state, and direct-SSR archived output. This is readiness evidence, not the
  named Myelin-repository founder acceptance act.
- **CT-005f8a — exact publisher database capability under isolated migrations:** the production
  publisher's startup validator detected an excess grant left by applying the original unqualified
  publisher-grant migration in a disposable-schema test. Immutable migration 0006 now removes
  table- and column-level publisher authority from every non-public outbox pair and restores only
  the exact public relay grants. A live PostgreSQL regression applies the full foundation set under
  an isolated `search_path`, proves the runtime login cannot SELECT/INSERT/UPDATE/DELETE either
  isolated table, and proves the ordinary production provider still accepts its public capability.
  The strict startup validator was not weakened. The restarted composed path drained both queued
  full-source snapshot triggers: runs `1e7f3510-7651-6ffb-a30b-d3267ab68254` and
  `da50c464-28bd-0dc8-3926-5e17ace266ef` settled succeeded/cost-settled with trusted attempt-1
  `ci/build` projections and zero publish retries. The old `5db61d81-…` pre-fix rehearsal remains
  explicit negative evidence, not a green acceptance record.
- **CT-005f8b — retained-snapshot mutation closure:** the mandatory-core mutation gate exposed eight
  surviving mutations in the bounded outbox snapshot path: empty snapshots and incorrect inclusive
  row/byte ceilings were not distinguished. Public-path tests now require exact and slack ceilings
  to preserve the retained row and require either ceiling at one under the row/envelope size to
  refuse. The complete configured gate accounts for all 464 mutants: 360 caught, 104 compile-time
  unviable, zero missed, and zero timed out. Mechanical payoff: missed mandatory-core mutants 8→0.
- **CT-005f8c — founder Myelin acceptance and surfaced-state repair:** the founder pushed
  `r4-1-founder-source-snapshot` at exact OID
  `1550ab91492c79357f78af4160f8aba106f73669` through the production wire and opened hosted PR #1.
  Run `00152490-3234-593f-ed63-4bb2ac0291ec`, job
  `e1694e63-8c35-8504-b826-0d05822e7e9d`, settled succeeded/cost-settled and projected trusted
  attempt-1 `ci/build` for that exact head. The no-overwrite verifier independently reread 76 bytes,
  found marker `MYELIN-CI-a005e32fc1bb0c2b64e7d40ac1a01236` exactly once in both the earlier live
  capture and archive, and emitted matching SHA-256
  `93f9208811ed63047e9a41f5b8d2bb7ab343d1dc0354efd246817227649ae250`. The exact-head check receipt
  records `gate_admitted=true`.

  The live pass exposed four ordinary dogfood seams and one browser capacity posture. CLI writes
  now require and transmit an explicit retry-stable `--idempotency-key`; Git's typed check context
  renders `ci/build` consistently in policy, projection, API, and the verifier; final launch moves
  the durable queue and public `ci_job` to `running` in one statement and refuses if the public row
  is absent; and an oversized PR diff is rendered as an HTTP-200 capacity state on direct SSR
  instead of a route 500. The latter 413 envelope is a shared golden vector executed by the Rust
  Edge and dev Edge, registered with both browser proofs in `contract-coverage.toml`. The full
  85-flow Chromium suite passes, including tree next-page, stale-tree 409→reload, and the new
  oversized-diff direct load.

**Historical CT-005b floor (closed by CT-005f1–f5):** `LiveTail`/Firehose was process-local while
runners and Edge were separate services, so CT-005b/CT-005c correctly claimed archived cold-path
reads only. The durable pointer transport, archive-before-ack consumers, and composed sever drill
closed that floor; CT-005f8c now closes the live founder push→settled check→surfaced archived/live
log pass. CT-007 remains separately gated on workload parity.
CT-005e closes the prior MCP floor without inventing live output: its two reads are exact durable
run/detail and archived-log operations, and the complete content-addressed Agent tool transcript
remains an Agent-runtime trace artifact rather than a claim made by the stdio transport.
CT-005f1 and CT-005f2 close the durable consumer/resume and producer-persistence halves without
overstating the remaining surface. The real gVisor and Firecracker paths now persist bounded output
while commands execute. CT-005f3 closes the authenticated web consumer and makes mock parity
mechanical rather than conventional. CT-005f4 closes the authenticated CLI consumer without
inventing run-wide aggregation: the production resume authority is exact `(run_id, job_id)`, so the
code requires `--job` even though the frozen plan used the shorthand `ci watch <run>`. CT-005f5
closes the remaining composed CI-D11 committed-prefix drill without overstating unproven
commit-unknown or HTTP-wire failure modes. CT-005f6 and CT-005f7 made the founder act
machine-verifiable and genuinely triggerable without prematurely claiming that it happened. CT-005f8
proves the same composed path on an authorized disposable repository without relabeling it as the
founder/Myelin act; CT-005f8c records that later real act. CT-007 remains unopened.

**Pre-registered CT-007 cutover floor (does not open CT-007 early).** The founder marker pipeline is
an end-to-end transport/surfacing acceptance job, not workload parity with `.github/workflows/ci.yml`.
The current GitHub graph still owns the Rust build/test/warnings-denied clippy, frontend
lint/unit/build/Chromium suite, production web-container smoke, architecture and contract gates,
SUB/Identity/self-hosting scorecards, mutation gate, and release-bundle job. CT-007 may begin only
after the named founder acceptance above, and it may disable or remove GitHub Actions only after:

1. a committed Myelin-native workflow maps every still-required GitHub job to an executable Myelin
   job or names a mechanically gated, non-CI owner; an inventory test fails on silent job loss;
2. digest-pinned one-cell runner assets provide the actual Rust/Node/browser/container capabilities
   those jobs require without weakening gVisor, egress, or privilege boundaries;
3. the complete mapped graph passes on one exact Myelin commit, its required aggregate context is
   visible and trusted through Git's projection, and the permanent mutation gate has zero missed
   mutants; and
4. a second ordinary Myelin commit repeats the graph without GitHub execution or manual green.

P0 later promotes this one-cell runner asset into the signed/SBOM release supply-chain floor; that
later provenance work is not an excuse to delete the current CI before R4 workload parity exists.

The danger concentrates in CT-002/003 (untrusted execution + escape verification). Those get a security
verifier that actively tries to escape the production sandbox; "0 escapes" is only credible THROUGH the prod
path (CT-003's guard enforces that). CT-007 (the GitHub-Actions bill killer) is the reward, gated on CT-003.

**CT-007 gate 1/4 closed (2026-07-25): committed workload inventory.** CT-005f8c's real founder
acceptance act satisfied CT-007's one precondition, so CT-007 is now legitimately openable; this
closes only its first of four cutover-floor gates, nothing more. `ci-workload-inventory.toml`
(workspace root) now names all 12 currently-real GitHub jobs — the 9 in `ci.yml`
(`edge-release-bundle`, `frontend`, `web-container`, `architecture-lints`, `contract-coverage`,
`sub-m0-scorecard`, `id-m1-scorecard`, `self-hosting-ci`, `build-test-clippy`), the 1 in
`integration.yml` (`integration`), and the 2 in `security.yml` (`rustsec`, `pnpm`) — with an honest
`status` (uniformly `github-only`; none is `myelin-native`, since `.myelin/ci.toml` still defines
only the one trivial founder-dogfood marker job and provides zero workload parity for any of the
12) and an `owner` naming the accountable cutover-plan step (2, 3, or 4 above). A new
`crates/myelin-lints/tests/ci_workload_inventory.rs` scans every `jobs:` block in
`.github/workflows/*.yml` and fails RED in both directions: a real GitHub job with no manifest row,
or a manifest row naming a job that no longer exists, plus a duplicate-row check and an honesty
invariant forbidding `myelin_job` from being non-empty unless `status` is actually
`myelin-native`. An independent adversarial verifier hand-enumerated every job in all three workflow
files against the manifest (finding the builder's own initial job list was stale — it had missed
`sub-m0-scorecard`/`id-m1-scorecard`, which the builder itself caught via its own test going RED
before this landed), confirmed no YAML anchors/matrix strategies/`if:`-guards/reusable-workflow
calls exist anywhere today for the dumb line-scanning parser to trip on, deleted a real manifest row
to prove the gate genuinely fails RED (then restored it byte-identical), and confirmed
`cargo test -p myelin-lints` and `cargo clippy -p myelin-lints --all-targets -- -D warnings` are both
clean with no other files touched.

**CT-007 gate 2/4, first slice (2026-07-25): the Rust runner asset, proven under real gVisor.**
`runner-assets.toml` (workspace root) now tracks `linux-rust-v1` — a digest-pinned gVisor rootfs
covering only the `build-test-clippy` job's need ("the base Rust workload every other job's crates
depend on existing"). Built by the new `scripts/build-rust-rootfs.sh`, which deliberately does NOT
follow `stage-git-rootfs.sh`'s hand-copy-host-binary precedent: the host's system `cargo`/`rustc`
pull in dozens of transitive shared libs (libgit2, libssl, libcurl, icu, ...) that would make a
hand-staged tree fragile and not a real immutable artifact. Instead the script pulls the official
`rust:1.82-slim-bookworm` image (pinned source digest `sha256:1111c28d...`), `docker export`s its
real filesystem, and symlinks `usr/local/bin/{rustc,cargo,...}` to the real toolchain binaries under
`usr/local/rustup/toolchains/.../bin` (not the rustup-proxy hardlinks, which depend on env/HOME
resolution this sandbox does not set up) so they resolve on this repo's hardcoded guest `PATH`
without touching `OciConfig`/hardening code at all. The staged tree's canonical-tree digest
(`sha256:6feada1e...`, same recipe as `dogfood.sh verify_ci_rootfs()`) is committed and mechanically
enforced by a new `crates/myelin-lints/tests/runner_asset_digest_pin.rs`, which skips honestly on any
machine without the asset staged and hard-fails under `MYELIN_REQUIRE_RUST_ROOTFS_PIN=1`.

A new `crates/myelin-ci-sandbox/tests/rust_capable_rootfs_prod_exec_test.rs` proves the asset for
real: a genuine `runsc` sandbox on the exact same `GvisorBackend::launch` path every other job uses
(no new capability, no OCI-config change, no code touched in `hardening.rs`/`escape_corpus.rs`/
`CiExecutionProfileV1`/dispatch config) runs `rustc --version && cargo --version` and captures a real
`rustc 1.82.0`/`cargo 1.82.0` banner from inside the guest, plus a second payload proving non-root
uid `65534` (checked by value, not just exit code) and closed egress (a real `/dev/tcp` connect
attempt against a live IP:port, contained) hold unchanged. An independent adversarial verifier
re-ran all of this from scratch, applied harder pressure than the builder's own tests (a root-write
attempt against `/`, a second distinct network target), confirmed `.myelin/ci.toml` and both existing
staged rootfs trees (`rootfs`, `git-rootfs`) are untouched (the base rootfs's canonical-tree digest
still matches its `.myelin/ci.toml` pin exactly), proved the digest-pin gate genuinely fails RED on a
corrupted pin and the skip path is honest in both directions, and found no issues.

**Honest remaining CT-007 floor.** Gate 2 covers Rust only; Node (the `frontend` job's lint/unit/
build + pinned-Chromium browser suite), a headless-browser capability, Docker/Docker-in-Docker (the
`web-container` and `integration` jobs), and outbound egress to the RustSec/npm advisory DBs
(`rustsec`/`pnpm`) all still have no runner asset — later slices of this same gate. No GitHub job has
been wired to actually dispatch onto this or any Myelin-native runner asset yet (that is gate 3's
"complete mapped graph passes on one exact Myelin commit," not this slice — `linux-rust-v1` is proven
reachable by a direct test harness call, not by the real CI dispatch path). The permanent mutation
gate has not been re-proven under any mapped graph; no second ordinary commit has repeated anything
without GitHub execution (gate 4). GitHub Actions remains fully in force and must not be disabled or
removed before gate 4 lands.
