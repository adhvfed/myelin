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

**[SUPERSEDED BY THE CORRECTION FURTHER BELOW — gate 1 is NOT closed; only its inventory subgate
is] CT-007 gate 1/4 closed (2026-07-25): committed workload inventory.** CT-005f8c's real founder
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

**Corrections from an independent peer-review round (2026-07-25; reviewer gpt-5.6-sol via a
persistent Codex CLI session, not this repo's own MR/GT/CT builder/verifier pair).** The "gate 1/4
closed" and "capability... covering the build-test-clippy job's need" framing above overclaimed.
Corrected:

- **Gate 1 is NOT closed.** The ledger's own literal gate-1 wording requires mapping every job to
  "an executable Myelin job OR a mechanically gated non-CI owner." The committed inventory's
  original free-text `owner` field named a future step number in prose that no test examined — a
  plan, not an enforced mechanism. Only the inventory-and-honesty SUBGATE is real and enforced;
  gate 1 itself stays open. `ci-workload-inventory.toml`'s `owner` field is replaced with structured,
  test-enforced `migration_step`/`migration_state` fields (a closed enum: `not-started` →
  `capability-smoke` → `capability-proven` → `graph-passing` → `cutover-repeated`), and
  `crates/myelin-lints/tests/ci_workload_inventory.rs` now mechanically verifies a `myelin-native`
  row's `myelin_job` actually exists in `.myelin/ci.toml` rather than trusting a plausible string.
- **The Rust runner asset proves toolchain execution, not the job's capability.**
  `rust_capable_rootfs_prod_exec_test.rs` only ran `rustc --version && cargo --version` — it never
  mounted an exact-commit checkout (gVisor's default is an unmounted `/`, no repo), had no vendored
  dependency cache (so `cargo build --workspace --locked` would have nothing to fetch under
  deny-all egress), never propagated the job's env (`OciConfig::from_spec` ignores `spec.env`;
  `crates/myelin-ci-sandbox/src/gvisor.rs`), and sized resources far below a real build's needs.
  `build-test-clippy`'s row now honestly reads `migration_state = "capability-smoke"`, not
  `capability-proven`. Separately, the "digest-pinned" claim did not bind to what actually launches:
  `scripts/build-rust-rootfs.sh` pulled a mutable tag without verifying the resolved digest, and
  production selects the rootfs from `MYELIN_GVISOR_ROOTFS`, never from `JobSpec.image` — so the
  image field was disconnected from reality ("security theatre," in the reviewer's words). Fixed in
  the script (digest-change-before-promotion refusal, `ALLOW_DIGEST_CHANGE=1` override); binding
  `JobSpec.image` itself to a resolved, hashed asset at launch is the first item of the vertical
  slice below, not yet done.
- **`scripts/build-rust-rootfs.sh` hardened against three real safety gaps** found in the same
  round: (1) it staged directly in place with a bare `rm -rf` before rebuilding — a typo'd
  `MYELIN_GVISOR_RUST_ROOTFS` override, or a failed build, could destroy an unrelated directory or
  the last known-good asset with no fallback; (2) the rebuilt digest was never checked against the
  committed pin before promotion, so a mutable-tag drift or export anomaly could silently replace a
  trusted asset; (3) an unrestricted override target could still be renamed/deleted outright. Fixed:
  content now stages into immutable digest-named directories under `<asset>.versions/`, promoted
  only by an atomic symlink swap (`mv -T`) after the digest is verified to match the committed pin
  (or `ALLOW_DIGEST_CHANGE=1` is explicitly set); an existing non-symlink path is only ever touched
  if a sidecar `<asset>.myelin-managed` marker proves this script created it, or
  `MYELIN_ALLOW_REPLACE_UNMANAGED=1` is set (and even then it is renamed aside, never deleted). All
  three fixes were exercised for real on this host (forced digest-mismatch refusal, forced override
  with the env flag, legacy-tree migration) and the real `runsc` prod-exec + digest-pin lint tests
  reconfirmed green after each change — including one genuine regression the fix itself introduced
  (`mktemp -d`'s default 0700 mode broke the sandboxed non-root uid's ability to traverse into the
  rootfs; caught by actually re-running the test, not assumed) and then corrected.
- **The `pnpm` job may already be broken independent of CT-007:** pnpm 10.5.2
  (`frontend/package.json`) may be affected by the npm registry's 2026 retirement of the legacy
  audit endpoints pnpm 10.x used — flagged, not yet investigated; unrelated to this ledger's gates.

**Vertical-slice redirection (2026-07-25).** The reviewer's strongest structural point: stop adding
capability assets horizontally (one job's toolchain at a time) and instead finish ONE job
(`build-test-clippy`) end to end — exact-commit checkout, vendored/locked dependencies as their own
digest-keyed asset, real env/cwd propagation, resource sizing proven against a real build, image
resolved and hash-verified at launch, and the real dispatch path (not a direct `GvisorBackend` call
from a test) — before starting Node/browser/Docker work for other jobs. This is now the plan of
record for gate 2's continuation; see the task list in the working session for the ordered steps.

**Vertical-slice step 1 (2026-07-25): `JobSpec.image` is now the real gVisor launch authority.**
Closes the "digest-pinned claims are disconnected from what actually executes" gap named above: a
launch used to resolve its rootfs solely from `MYELIN_GVISOR_ROOTFS`, ignoring `spec.image` entirely.
A new `crates/myelin-ci-sandbox/src/canonical_tar.rs` reimplements, in pure Rust (no host `tar`
process — the `no-host-exec` lint forbids that from a trusted path), the exact byte stream
`tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner --format=gnu -C <dir> -cf - . |
sha256sum` produces, discovered and fixed one genuine subtlety along the way (`--sort=name` compares
BARE per-directory entry names, not full flat-sorted archived path strings — a real Debian rootfs
containing both `etc/ca-certificates.conf` and `etc/ca-certificates/` in the same parent orders them
differently under a naive flat sort), and reproduces the exact already-committed digest for BOTH real
staged assets: `linux-rust-v1` (`6feada1e...`) and, critically, `linux-small-v1`
(`f9bd3926...`) — the asset the real founder-dogfood pipeline runs today.

A new `GvisorAssetRegistry` (`crates/myelin-ci-sandbox/src/asset_registry.rs`) is an exact-reference,
sha256-only map from an `ImageRef` to an already-[`VerifiedRootfs`]. `GvisorBackend::new` now requires
one explicitly (`#[derive(Default)]` removed — no registry-less production backend can be constructed
by accident); `GvisorBackend::git_wire_only()` is the separate constructor for the git-wire path
(which keeps its own, different rootfs resolver) and provably refuses ordinary
`launch`/`launch_streaming`. `myelin-ci-controlplane`'s `production_gvisor_registry()` (the one real
production call site, `runner_bind.rs`) registers both real assets, reusing the SAME existing
resolvers (`resolved_gvisor_rootfs`/`resolved_gvisor_rust_rootfs`) production already used — nothing
about which bytes execute changed, only that `spec.image` is now checked against them.

An independent adversarial review (gpt-5.6-sol) caught a real design flaw before this landed: the
first version re-verified (re-canonicalized AND re-hashed the whole directory) on EVERY launch,
before the isolation floor even ran — measured at ~15 seconds against the >800MiB Rust asset, paid on
every single job launch, with an exhausted-wallet caller able to force the expensive scan repeatedly
with zero chance of ever launching, and a RED isolation floor not blocking it either. Fixed:
verification now happens exactly ONCE, at `GvisorAssetRegistry::from_bindings` construction time
(runner startup) — a runner that cannot verify even one configured asset refuses to start, loudly.
`resolve` is now an O(1) map lookup against already-verified entries; measured on this host: ~14.9s to
construct (once), ~298ns average per `resolve()` call afterward. `GvisorBackend::launch_with`'s order
is now isolation floor → hardening assert → registry lookup → reserve → launch-permit CAS → run →
settle — matching the Firecracker backend's own mandated ordering; a dedicated test proves the floor
fires before the (cheap) lookup by using a genuinely unregistered image (a wrong-order implementation
would refuse via `Image` without the floor hook ever running — the test asserts the floor WAS called).

The same review also caught that the digest existed as six independent hardcoded copies (two Rust
constants, two TOML fields, two test literals) with nothing tying them together — a source edit could
leave every test green while production silently diverged. Fixed: a new unconditional
`crates/myelin-lints/tests/runner_asset_digest_pin.rs` test mechanically asserts the Rust constants
match `runner-assets.toml`'s and `.myelin/ci.toml`'s real fields (verified this fires RED by
corrupting one value and confirming the test fails with a clear message, then restoring it
byte-identical); the duplicate literals in the canonical-tar test were removed in favor of importing
the real constants.

Independently re-verified by me, twice (once per fix round), reading every file rather than trusting
either the builder's or the reviewer's summary: full `cargo test -p myelin-ci-sandbox --features
integration` and `cargo test -p myelin-ci-controlplane --features integration` (every test binary,
not just new ones), `cargo test -p myelin-lints`, and `cargo clippy --workspace --all-targets
--features integration -- -D warnings` all clean, with exactly one pre-existing failure class in
`myelin-ci-controlplane`/`myelin-edge` (7 tests total, all Postgres-integration-shaped) confirmed —
via `git stash` + rerunning the identical tests against the prior committed code — to be unrelated to
this work, not a regression. Also found and reverted ~12 files a builder pass had reformatted with
`cargo fmt` beyond the scope of this change (confirmed via grep that none referenced
gvisor/registry/asset_registry), keeping the landed diff to what actually matters.

**Honest remaining floor.** This is the FIRST item of the vertical slice, not the whole slice:
exact-commit checkout mounting, vendored/locked dependencies as their own asset, real env/cwd
propagation, and resource sizing proven against a real build are all still open — `build-test-clippy`
still reads `migration_state = "capability-smoke"` in `ci-workload-inventory.toml`, honestly, not
`capability-proven`. No job dispatches through this registry yet; `production_gvisor_registry()` is
wired at the real call site but nothing currently launches naming either registered image outside of
tests. The 7 pre-existing test failures found during this work remain open and unowned by this
ledger's gates — a separate issue, flagged here for whoever picks it up next.

**The "7 pre-existing failures" root-caused and fixed (2026-07-25).** Per the user's explicit
instruction ("flaky tests indicate dragons... the flakiness should excite you") the 7 failures above
were not left as a footnote — they were investigated to root cause, planned as 5 parallel workstreams,
built by 5 concurrent agents on disjoint file sets, then independently re-verified end to end
(including finding and fixing gaps the agents' own reports missed). Two genuinely distinct root causes
were found, both real, neither a flake in the usual sense:

1. **A systemic missing-teardown gap, not a flake.** 21 integration test files across 8 crates
   (`myelin-ci-controlplane`, `myelin-storage`, `myelin-ci-dispatch`, `myelin-ci-sandbox`,
   `myelin-edge`, `myelin-flow`, `myelin-mcp`) create an ad-hoc per-process-id Postgres schema (or, in
   two files' cases, ad-hoc tables/rows in `public`) but only ever cleaned up at the START of their own
   NEXT run — never at the end of the current run, and never on a panicking assertion. This let 234
   orphaned schemas accumulate on this host (confirmed: every single one had a PID in its name; nothing
   static was among them), some of them reachable broadly enough to trip
   `dedicated_scheduler_role_is_region_bound_least_privilege_and_reset_safe`'s real least-privilege
   check — which then re-contaminated the REAL shared `public.job_queue`/`ci_run` tables with its own
   stray tenants every time IT failed, which is what the `production_pg_bootstrap_source.rs`
   "zero pre-existing active work" smoke test was actually tripping over. Fixed: every affected file now
   wraps its real test body in a `catch_unwind` + unconditional cleanup + `resume_unwind` helper
   (`tests/common::with_schema_cleanup` in the two crates large enough to warrant a shared module;
   inlined per-file elsewhere) — cleanup now runs whether the test passes, fails an assertion, or
   panics. Proven by running every affected file's suite twice in a row with `pg_catalog.pg_namespace`
   counts checked between runs (stable, not growing) across all 5 agents' independent verification
   passes, PLUS my own from-scratch full-workspace sweep afterward. One file
   (`integration_ci_manifest_pipeline.rs`) was missed by the initial file survey (it names its schema
   helper `pool_on` instead of `admin_pool`/`schema_name`) and was found + fixed in the final review
   pass.
2. **A real, previously-latent production bug: crash-then-retry permanently strands a job.**
   `AUTHORIZE_JOB_LAUNCH_QUERY`'s launch CAS crosses `ci_job.state` to `'running'` in the SAME
   statement as the `job_queue` launch fence, and requires `surface.state IN ('queued', 'leased')` to
   do so — but neither the dead-runner reaper (`job_queue_region.rs::reap_region_scoped`) nor the
   retryable-attempt requeue path (`ci_pipeline_driver.rs::record_retryable_attempt_on_conn`) ever
   reset `ci_job.state` back to `'queued'` after re-queuing `job_queue`. A job whose launch CAS
   committed and then crashed (before completing) was reaped/retried at the `job_queue` layer but could
   NEVER re-win the launch fence — `ci_job.state` stayed stuck at `'running'` forever, permanently
   stranding the job. This affects the REAL production runner: `main.rs` wires the reaper through
   `scheduler_provider.region_queue_store()`, the SAME dedicated least-privilege role the boundary test
   exercises. Fixed with `RESET_REAPED_CI_JOB_SURFACE_QUERY` (reaper) and a mirrored reset (retryable-
   attempt path), each resetting only the exact jobs just re-queued, only from `'running'` (a terminal
   DAG fact is never reopened). This surfaced a THIRD, cascading gap: the dedicated scheduler role had
   NEVER been granted any privilege on `ci_job` at all (a table that didn't exist when that role's
   grants were last touched) — so the fix above would have failed in real production with "permission
   denied for table ci_job" the first time it ever reaped a launched job, exactly as the dedicated-role
   boundary test caught. Fixed with a new forward-only migration
   (`ci_0018h_scheduler_ci_job_reap_reset_grant`) granting exactly `SELECT (tenant_id, job_id, state)` +
   `UPDATE (state)` — column-scoped, never a blanket table grant. The first version of this migration
   granted only `UPDATE (state)` and still failed ("permission denied") because the reset query's OWN
   `WHERE state = 'running'` filter also needs SELECT on `state` (an UPDATE's WHERE-clause predicate
   needs SELECT on every column it inspects, not just the column being written) — caught by actually
   re-running the dedicated-role test after applying the first version, not assumed correct from the
   DDL alone. Three migration-count assertions (`migrations.rs` ×2, `lib.rs` ×1) were pinned at the old
   total and needed updating alongside — exactly the kind of load-bearing test this repo's migration
   discipline is supposed to produce.

**Honest remaining floor from this investigation.** Two failures remain, both understood, neither
silently swept aside:
- `boot_time_sigterm_is_latched_before_the_real_runner_host_can_claim` fails not from a bug but from a
  genuine tension with an EARLIER, deliberate decision: the historical negative-evidence row
  `myelin`/`5db61d81-...` (R4.2 publisher work, ledger entry above) is intentionally preserved in a
  `running` state, and this smoke test's own precondition is "zero pre-existing active work." As long
  as that row is preserved (as instructed), this test will fail its own precondition. Not fixed;
  flagged, since resolving the tension isn't this investigation's call to make unilaterally.
- `starter_manifest_drives_dag_across_worker_restarts_and_loader_retry`
  (`integration_ci_manifest_pipeline.rs`) fails on a genuinely SEPARATE, unrelated bug:
  `pg_pipeline_starter.rs`'s "manifest check context `build` has no run-scoped allocation ledger" —
  a different subsystem (check-attempt allocation, not schema teardown or the ci_job reset above).
  Its schema-teardown gap (found alongside it) is fixed; the underlying CorruptRun bug is NOT — flagged
  for a dedicated follow-up, not chased further today given how far this investigation had already
  gone.
- Two more real, PRE-EXISTING (confirmed independently by two different agents plus my own git-stash
  comparisons) failures outside today's scope: `myelin-edge`'s
  `production_sink_and_edge_resume_exactly_after_both_services_are_severed` (a "resolve canonical CI
  log run: no rows returned" Postgres error, unrelated to schema teardown), and a real cross-crate
  taxonomy gap found while fixing an unrelated `myelin-edge` test —
  `myelin_events::taxonomy::SUBSYSTEM_TOKENS` never admits the `"iam"` subsystem token, so every
  `iam.tuple.written`/`iam.role.granted`/`iam.break_glass.invoked` event the real elected outbox relay
  processes is permanently quarantined (an `ON DELETE RESTRICT` FK then blocks any cleanup that touches
  it). This surfaces identically in `myelin-mcp`'s
  `response_lost_retry_is_exactly_once_for_open_review_and_events`. Both are `myelin-events`/
  `myelin-storage` contract issues, out of scope for this investigation, worth a dedicated follow-up.

Full re-verification after all fixes (mine, independently, not trusting any agent's self-report):
`cargo test -p myelin-ci-controlplane --features integration` (610+ passed, exactly the 2 documented
failures above), `myelin-storage`/`myelin-edge`/`myelin-ci-dispatch`/`myelin-ci-sandbox`/`myelin-flow`
all fully green except the one pre-existing `myelin-edge` failure and one confirmed-transient
`myelin-storage` flake (passed cleanly on a clean full-file rerun), `myelin-mcp` green except the
documented `iam`-taxonomy symptom, and `cargo clippy --workspace --all-targets --features integration
-- -D warnings` clean across the entire workspace.

**2026-07-25: closing out the four remaining known issues from the investigation above, one agent at
a time (sequential, not parallel — running multiple agents against this repo's one shared, live
Postgres was itself found to compound the exact contamination class the investigation above spent
hours fixing).**

1. **The `boot_time_sigterm_is_latched...` test/historical-data tension — fixed by narrowing the
   test's query, not touching the preserved row.** `production_pg_bootstrap_source.rs`'s "zero
   pre-existing active work" precondition now excludes exactly
   `(tenant_id, run_id) = ('myelin', '5db61d81-6aea-7dd9-b3f1-035abcf56b26')` — the R4.2 negative-
   evidence row this ledger already says must be preserved — from the `ci_run` active-work count only
   (never from `job_queue`). The row itself is untouched.
2. **The `pg_pipeline_starter.rs` allocation-ledger bug — confirmed as a test-fixture gap, not a
   production bug.** `integration_ci_manifest_pipeline.rs`'s `starter_manifest_drives_dag_...` test
   never seeded `ci_run_check_attempt` rows for the `build`/`package`/`test` contexts it exercises
   (only `allocate_reserve_check_attempts`'s own contexts were seeded). Fixed with a
   `reserve_test_check_attempts()` fixture helper mirroring the existing seeding pattern; no
   production code changed.
3. **`myelin-edge`'s pre-existing Postgres resume-error test — fixed with a regression-proof
   fixture gap, not a production bug.** `integration_ci_http_surface.rs`'s
   `production_sink_and_edge_resume_exactly_after_both_services_are_severed` failed with "resolve
   canonical CI log run: no rows returned" because the test never seeded the `job_queue`/`ci_job_spec`
   rows its own log-resume path needs. Fixed with a `seed_log_route` helper (same shape as
   `integration_ci_ct004f_durable_log_persist.rs`'s existing one).
4. **The `iam`/`identity` taxonomy gap — a real cross-crate contract bug, fixed with a rename +
   regression test, reviewed by Sol (gpt-5.6-sol).** `myelin-identity::iam_events` minted its three
   event tokens with an `iam.` subsystem prefix; `myelin-events::taxonomy::SUBSYSTEM_TOKENS` has
   NEVER admitted `iam` (deliberately — `identity` is already the canonical §6.2 token for this exact
   subsystem, since the crate IS `myelin-identity`). Every real `iam.tuple.written` /
   `iam.role.granted` / `iam.break_glass.invoked` row the elected outbox relay processed hit
   `UnknownSubsystem` and was quarantined — permanently, since `outbox_quarantine` has `ON DELETE
   RESTRICT` back to `outbox` and the relay itself has no remediation path. Fixed by renaming the
   tokens (and the URN/`AggregateKey` segments, `iam:tuple:…` → `identity:tuple:…`) to the
   already-canonical `identity.*` prefix across 27 files in 7 crates — NOT by admitting `iam` as a
   second subsystem token, which would just re-admit the bug under a different grammar rule. Added a
   grammar regression test (`identity_tuple_written_and_siblings_are_admitted_by_the_grammar`) that
   iterates the REAL `myelin_identity::iam_events::IDENTITY_EVENT_TOKENS` table through `validate()`
   (not copy-pasted literals, per Sol's review — a copy would silently drift from what the crate
   actually emits) and separately proves the old `iam.tuple.written` spelling still correctly fails.
   Sol's review confirmed the rename direction and the module-doc/debug_assert polish; it also
   confirmed, unprompted, the deliberate choice to keep the `iam_events` module name and
   `IamEventProjection`/`IamSubjectRef` types as-is (IAM is a reasonable domain term for the
   Rust-level API; `identity` is the canonical wire-level namespace — renaming the types would be
   cosmetic churn, not a contract fix).
   **Production remediation, not just prevention:** this bug had already quarantined 19 real
   `iam:tuple:*` rows on this host between 2026-07-24 and 2026-07-25 (their envelope bodies were
   still intact — quarantine fences a row, it doesn't destroy it). Checked whether this represented a
   live authz gap before touching anything: `TupleStore::write_tuples` (the only real emit path for
   this token) is still `AuthzError::NotYetImplemented` in production — so these 19 rows are all
   pre-M1 bootstrap/dogfood noise, not customer-facing state. Per explicit confirmation that this host
   has no production tenant yet ("Myelin does not have a prod. 0 risk."), deleted all 19
   `invalid_event_taxonomy` / `iam:tuple:*` quarantine rows directly; the 2 unrelated
   `invalid_event_taxonomy` rows (a different, `issue:`-aggregate cause) were left untouched.

**A fifth issue surfaced while re-verifying #4, genuinely separate and NOT yet fixed:**
`myelin-mcp/tests/git_effect_governed.rs::response_lost_retry_is_exactly_once_for_open_review_and_events`
still fails post-fix — but now with `wrong_relay_region`, not `invalid_event_taxonomy`, confirming
the taxonomy fix above is real and complete. The new failure is a test-isolation gap: this test's
events are tagged `region="eu-west"`, but they're inserted into the SAME shared `outbox` table this
host's real, long-running `myelin-outbox-publisher serve` process (PID 3577784, running since
2026-07-24 — the founder's actual dogfood relay) polls continuously. That live process claims the
test's rows before the test's own cleanup can, sees the region disagree with its own cell's region,
and quarantines them — so the test's own `DELETE FROM outbox` then hits the `ON DELETE RESTRICT` FK.
Nothing in the test suite currently stops the live dogfood relay from racing test-inserted rows in
the shared outbox table. Not fixed today: this predates today's batch, is unrelated to the taxonomy
bug, and touches how test runs and the real persistent relay process share (or should not share)
the same table — a scope decision, not a quick fix, and one that shouldn't be made unilaterally
against a live process a human depends on. Flagged for a dedicated follow-up.

Full re-verification of this batch: `cargo build --workspace --locked` clean; `cargo clippy
--workspace --all-targets --features integration -- -D warnings` clean; unit tests (`--lib`) green
across all 7 touched crates (myelin-events 205, myelin-identity 16, myelin-identity-service 387,
myelin-gdpr-service 333, myelin-storage 470, myelin-edge 217, myelin-chat 305); integration suites
for myelin-identity-service/myelin-gdpr-service/myelin-chat/myelin-edge fully green;
myelin-ci-controlplane green (false-bug + allocation-ledger fixes both hold); the one
`myelin-storage` outbox-relay count flake reconfirmed transient (0 unsent rows, clean on rerun,
unrelated to this batch — `relay_once` sweeping unrelated concurrent rows, a pre-existing
sensitivity, not introduced here). The fifth issue above (`wrong_relay_region`) is the only known
open item after this batch.

**2026-07-25 (later): the fifth issue (`wrong_relay_region`) closed — NOT by touching the relay's
validation contract.** Investigated whether `pgrelay.rs`'s claim query should filter by region at
the SQL level (my first hypothesis). Wrong: `myelin-storage/tests/integration_outbox_quarantine.rs`
already has a deliberate, existing, previously-reviewed test asserting a wrong-region row is
quarantined exactly like every other permanent envelope defect — that behavior is intentional, not
an oversight, and changing it would overturn an already-tested contract on a hunch. The real,
already-documented root cause is in `myelin-storage/tests/common/mod.rs`'s own doc comment: this
dev host's `outbox`/`outbox_quarantine` tables are a REAL SHARED resource between the live founder
dogfood relay process and every test suite, and "genuinely serializing against an independent test
suite's concurrent writes on a live shared table is a bigger, separate concern than this crate's
own per-test cleanup" — the established, already-accepted mitigation is bounded-retry cleanup
(`delete_outbox_for_aggregate`), not SQL-level filtering. `myelin-mcp`'s
`git_effect_governed.rs` never adopted that pattern — its cleanup did a single un-retried
`DELETE FROM outbox WHERE envelope->>'tenant'=$1`, which aborts on the `outbox_quarantine` FK the
instant the live relay wins the race (real, observed: `wrong_relay_region`, since this test's
`REGION="eu-west"` differs from the live relay's own configured region). Fixed by porting the exact
same retry pattern (`delete_outbox_for_tenant`: delete matching `outbox_quarantine` rows first,
then `outbox`, retrying up to 5 times) into this test. Verified: the target test passes, the full
`myelin-mcp` integration suite (21+5+1+5+2+21+1+1+1 = all green) passes, `cargo clippy -p
myelin-mcp --all-targets --features integration -- -D warnings` clean. This closes the last open
item from the 2026-07-25 flaky-test investigation.

**2026-07-26: production-readiness push begins (`/goal` set: "bring Myelin as far as you can
towards production readiness").** Assessed the release-track ledger (14): R0/R1/R2/R3 all
CONFIRMED DONE and verified; R4 is the live edge — the founder's own dogfood push/PR/CI cycle has
already run real end-to-end, hosted PR #1 is open (merge is explicitly the founder's own act, not
mine), and the one substantive engineering gate still open is CT-007 (cut CI over from GitHub
Actions) — 11 of 12 GitHub-job rows in `ci-workload-inventory.toml` are still `not-started`, and
even the furthest-along (`build-test-clippy`) is only `capability-smoke`. Resumed the pending
vertical-slice-step-2 design (writable/disk-backed workspace storage) with Sol.

**Sol's step-2 design (concrete, opinionated):** primary mechanism is a per-job **Btrfs subvolume +
hard qgroup quota**, bind-mounted writable at `/workspace` (this host's `/` is confirmed Btrfs) —
fits gVisor's OCI-bind-mount model directly, gives fast deterministic per-job deletion; fail the
runner startup check loud if Btrfs/qgroup support is unavailable, never silently fall back to an
unbounded directory (a loop-mounted sparse file is the portability fallback, not the primary
design). Rejected overlayfs (no meaningful lower layer for cargo scratch; gVisor's own `overlay2`
facility risks making read-only inputs writable if misapplied). Found a real prerequisite gap:
`runsc --rootless` only maps the caller to container UID 0, it cannot map UID 65534 to a writable
host identity — the fix is an explicit multi-ID OCI user namespace (`newuidmap`/`newgidmap`), not
the single-UID `--rootless` shortcut, with the per-job subvolume `chown`'d to the mapped host IDs;
this becomes part of the hardened host surface and re-runs the production escape corpus. Cargo
dependencies: a SEPARATE read-only EROFS asset (`cargo vendor --locked --versioned-dirs` baked into
an immutable, digest-pinned artifact keyed by `Cargo.lock`'s own hash), not baked into the
`linux-rust-v1` toolchain rootfs — coupling the Rust toolchain to this repo's exact lockfile would
force a full rootfs rebuild on every dependency bump and make the "reusable Rust capability" asset
repository-specific. Drift enforced twice: a permanent lint pinning the current `Cargo.lock` hash
to the asset row, and a runtime hash re-check after exact checkout, before the launch-permit CAS.
`WorkspaceSpec` gains `storage: WorkspaceStorage` (`None | EphemeralDisk`) and `cargo_deps:
Option<CargoDependencySet>`; host paths stay OUT of the public/serializable spec entirely (a new
internal-only `PreparedWorkspace` guard carries them, mirroring the existing run-token-credential
redaction discipline) — fixed mount destinations, no caller-supplied paths, ever. Launch ordering
gains one stage (resolve assets → reserve disk capacity + create/quota workspace → exact checkout +
lock/deps hash match → assert resolved mounts/quota) ahead of the existing launch-permit CAS, with
a crash-safe cleanup guard and boot-time orphan-subvolume reconciliation (refuse new work if an
orphan can't be removed — fail-closed on capacity, not fail-open).

**Step 2a landed (commit `4c3f8a1f`):** the pure type/plumbing half of the above — `ResourceLimits`
now carries `disk_bytes` (the disk-backed ephemeral-workspace quota Sol's design targets — still
unwired, a later step) separately from the new `tmpfs_bytes` (the existing RAM-backed `/tmp`
ceiling, rewired to read from the new field with byte-identical runtime behavior). No mount/quota
logic yet. Full workspace build/test clean, both touched integration crates' suites green,
`cargo clippy --workspace` and the integration-feature variant both clean. `firecracker.rs` has the
identical tmpfs/disk conflation, scoped out of this prompt deliberately — flagged for the same fix
in step 2b.

**Three more dragons found and fixed while verifying this batch (none touch the actual step-2a
diff's own correctness — found via the standing "never accept a non-mint test environment"
discipline, same as 2026-07-25's investigation):**
1. **3 stale test literals hardcoding the pre-hardening `git.ref.updated` aggregate/subject key
   format** (`"<repo>:<ref>"` instead of the real, already-correctly-tested-elsewhere
   `"ref:<encoded_repo>:<encoded_ref>"` `GitRefEventKey::aggregate`/`subject` actually produce) —
   confirmed pre-existing and unrelated via `git stash` before touching anything. Fixed the two
   simple literal mismatches directly; fixed the third (`drills_git_d1_hot_ref_burst.rs`) by
   deriving the expected value from the real constructor instead of a second hand-rolled format
   string, so it cannot drift the same way again (commit `af54940a`).
2. **`myelin-control-plane` was missed entirely from 2026-07-25's 8-crate/22-file panic-safe-
   cleanup sweep** — the identical bug (a bare `cleanup(...)` call only at the end of the happy
   path, skipped by any earlier assertion panic). Confirmed live: 8 orphaned `cellself*`/`cellw*`
   rows in `public.cell`, one old enough to make a later invocation's own unscoped single-cell
   lookup pick up a stale prior run's row instead of its own fresh one
   (`self_host_boots_on_pg_and_routing_survives_restart`). Fixed across both affected files (6 test
   functions) with the same `with_cleanup(body, cleanup)` pattern already established in
   `myelin-storage`; verified stable (0 growth) across repeated runs (commit `de481f62`).
3. **A real cross-crate enum-contract bug, same class as the `iam`/`identity` taxonomy fix**:
   `myelin-mcp`'s `git_effect_governed.rs` wrote `isolation_kind`/`isolation_tier: "Shared"` into
   the durable `cell`/`tenant_placement`/`local_tenant` tables — `myelin-control-plane`'s own
   `IsolationKind` enum has never had a `"Shared"` variant (only `Pool`/`Bridge`/`Dedicated`; `Pool`
   is explicitly documented as "the shared-pool tier"). Every row this test ever wrote was silently
   unparseable by `myelin-control-plane`'s own strict registry reader — invisible until that
   crate's OWN test did a broad scan and hit its `corrupt_row_panic` on one of the 9 leftover rows.
   Fixed by using the correct canonical spelling; cleaned up the 9 contaminated rows (commit
   `924a01a9`).

All three were confirmed pre-existing (via `git stash` or direct row inspection) before being
touched — none were caused by the step-2a diff. Full re-verification after all four commits:
`cargo build --workspace --locked` clean, `cargo test --workspace --locked` clean, `cargo test`
with `--features integration` clean across `myelin-ci-sandbox`/`myelin-ci-controlplane`/
`myelin-agent-service`/`myelin-edge`/`myelin-control-plane`/`myelin-git`/`myelin-mcp`, `cargo
clippy --workspace --all-targets -- -D warnings` clean, integration-feature clippy clean.

**2026-07-26 (later): step 2b landed (commit `32dff06d`) — the Btrfs subvolume+qgroup workspace
module, adversarially reviewed by Sol across SIX rounds before being approved.** Also fixed
`firecracker.rs`'s identical tmpfs/disk conflation flagged above (commit `852bc7ae`).

Every round found a genuine, concrete bug — not style preference, and none anticipated by my own
first draft:
1. ABA-unsafe deletion (a stale handle could delete a same-path-but-different subvolume created
   after it) — fixed with a captured subvolume id; found STILL racy one round later (id-compare-
   then-path-delete still re-resolves the leaf path at the delete call itself) — final fix deletes
   by persistent subvolume id against the filesystem anchor, never touching the job's own path at
   delete time.
2. Deletion authority was unrestricted (any `(path, id)` pair could be handed to the API) — fixed
   with a `WorkspaceStorage` handle owning the base directory, consuming `PreparedWorkspace`/
   `OrphanCandidate` BY VALUE; found STILL forgeable ACROSS two different `WorkspaceStorage`
   instances one round later — fixed by having both capability types carry the canonical base
   they were minted against, checked before any deletion acts.
3. Rollback failures on partial-provisioning were silently swallowed — fixed with a compound
   `UnrecoverableLeak` error carrying both the original and the cleanup failure, requiring the
   caller to treat it as "mark unhealthy, refuse admissions until a human reconciles" — never an
   ordinary retryable error.
4. The quota-enforcement precondition only checked "some qgroup interface exists," not that
   quotas were ACTUALLY enforcing (Btrfs quotas can be nominally enabled but marked inconsistent,
   which silently disables enforcement) — fixed with a real `btrfs quota status` parse requiring
   all four fields (`Enabled`/`Mode`/`Inconsistent`/`Override limits`).
5. The quota itself was never verified as a postcondition, just "the command exited 0" — fixed by
   re-reading the qgroup's own row and asserting the applied limit matches exactly, and (found
   while writing the exceed-quota test) an all-zero test payload would have "passed" without ever
   proving enforcement at all under this mount's zstd compression — fixed with deterministic
   incompressible test data and a specific `ENOSPC`/`EDQUOT` errno assertion.
6. `btrfs inspect-internal rootid` silently resolves to the CONTAINING subvolume's id for a
   non-subvolume path instead of erroring — a real bug found empirically (not theorized) while
   testing orphan listing against a stray file — fixed with an inode-256
   (`BTRFS_FIRST_FREE_OBJECTID`) check before ever trusting `rootid`'s output.
7. `--commit-after` was the wrong tool for crash-durable deletion: btrfs-progs performs the
   destroy ioctl FIRST and only then waits for the commit, so a `--commit-after` failure can mean
   the destroy already happened while this code assumed "nonzero exit ⇒ nothing committed" —
   removed the flag entirely; the already-unconditional `subvolume sync` afterward IS the real
   "fully removed" postcondition and subsumes it (confirmed directly: sync on an already-fully-
   gone id succeeds trivially).
8. The base directory's ownership/permissions were only DOCUMENTED as a precondition for the
   still-open create-time TOCTOU, not enforced — fixed: `WorkspaceStorage::open` fails loud
   unless the base is owned by the calling process's own effective uid with no group/world write
   bit, turning the mitigation from a wish into a real permission failure to obtain.
9. Every mutating method now takes `&mut self` so the borrow checker forbids concurrent calls into
   one handle within a process — a real gap found via the same shared-test-fixture reasoning this
   whole day's work kept surfacing: my own first test suite shared ONE base directory across all
   tests, which Rust runs concurrently by default; fixed with a per-test unique tag.

**Honestly documented, not hidden:** the window from `subvolume create` through the eventual
gVisor bind mount still resolves the leaf pathname more than once — genuinely closing it needs an
exclusive lock or the raw `BTRFS_IOC_INO_LOOKUP` ioctl, neither of which this crate has today.
Named as a required prerequisite for the `gvisor.rs` wiring, explicitly NOT solved in this module,
per Sol's own accepted deferral for a staged, not-yet-integrated increment.

Verified end-to-end against this host's real Btrfs filesystem (subvolume create/quota-limit/
verify/exceed/delete/sync, plus the id-addressed qgroup-limit/qgroup-show forms) via direct shell
reproduction of every mechanism this module uses, independent of the Rust test suite. 7 unit tests
(4 run for real on this unprivileged host — job-id/quota validation, orphan-entry verification,
tmpfs rejection — 3 correctly skip on the confirmed-specific `EPERM` for `CAP_SYS_ADMIN`-gated
operations). Full workspace build/test/clippy/rustfmt clean.

**Still open for CT-007:** the actual `gvisor.rs` wiring (OCI-bundle mount + the UID-namespace
rework `runsc --rootless`'s single-UID shortcut needs — the writable-mount gap this whole module
exists to close), the EROFS cargo-vendor asset pipeline, the launch-ordering insert, crash
reconciliation orchestration (the admission-barrier discipline this module's `list_orphaned_
workspaces` doc names as the caller's job) — all deferred, none built yet. 11 of 12
`ci-workload-inventory.toml` rows remain `not-started`; the other CI job families
(frontend+Chromium+Valkey, web-container Docker-in-guest, integration multi-service stack, rustsec
advisory-DB broker, pnpm) have no runner-asset design started at all yet.

**2026-07-26 (later): planned the `gvisor.rs` integration with Sol before writing any code —
the scope is substantially larger than a single reviewable slice, recorded here in full rather
than discovered piecemeal mid-implementation.**

**Revised launch order** (vs. today's isolation-floor → hardening → registry-resolve → reserve →
permit → run → settle): resolve immutable assets (rootfs + any read-only dep asset) → a read-only
workspace-storage health check → `reserve` (now must include AGGREGATE host-disk admission, not
just this job's own quota — a per-job qgroup limit is a ceiling, not a reservation of physical free
space) → create the Btrfs workspace → build the OCI config with the workspace mount → acquire the
launch permit → run to completion, confirming the runsc/gofer process tree is actually gone → settle
→ delete+sync the workspace regardless of settlement outcome → release the local disk-capacity
lease. Workspace creation must NOT happen before `reserve` (as I first proposed) — that would let
jobs that can never get capacity still churn privileged Btrfs subvolume/qgroup operations, an
avoidable host-DoS surface.

**A persistent workspace manager, not per-launch `WorkspaceStorage::open`:** `GvisorBackend` needs
one long-lived manager owning a process-lifetime exclusive lock on the workspace base, a
`Mutex<WorkspaceStorage>` for short create/delete calls, the active workspace-id set, a
poisoned/unhealthy admission flag, and boot-time orphan reconciliation run while admission is
closed. The launch-permit CAS is NOT a workspace lock — it protects a scheduler generation, not
concurrent workspace mutation. This is the module's own documented, deliberately-deferred
create-to-bind race; the process-lifetime base lock + centralized active-id tracking is the
MINIMUM acceptable close for a single-process runner. (If multiple same-euid host processes must
be in the threat model, an advisory lock alone is insuficient and needs an FD/stable-mount-source
solution — out of scope for now.)

**Job/subvolume naming:** don't reuse the raw idempotency token as the Btrfs directory name — use
a server-resolved job/claim-generation identifier encoded as safe hex.

**Spec shape:** an explicit `WorkspaceStorageMode::{Disabled, EphemeralDisk}` (not a bare bool);
`disk_bytes` stays the quota; host path and ownership are derived INSIDE the backend, never
caller-supplied in `WorkspaceSpec`. A checkout-bearing spec with storage disabled should be
refused; an agent job with no repo may still request scratch storage.

**OCI mount details, several of them real gotchas already documented elsewhere in this exact
file:** bind `/workspace` read-write with `rw,nosuid,nodev` (deliberately NOT `noexec` — cargo
build scripts and produced binaries must execute from `target/`); set `process.cwd` to
`/workspace` (currently hardcoded to `/`); keep `/tmp` as the separately-bounded RAM tmpfs
untouched; construct the mount only from a borrowed `PreparedWorkspace`, never a caller-supplied
host path; generalize the existing `WireMount` type into a private OCI bind-mount representation
rather than widening a wire-specific public type into a general host-path authority. **Critically:**
`root.path` must be set to the ABSOLUTE verified rootfs path for workspace jobs — this file already
documents (line ~249) that a bundle-relative rootfs symlink combined with a host bind mount breaks
rootless gofer startup; naively appending `/workspace` to `extra_mounts` while leaving
`root_path=None` is very likely broken the same way.

**Lifetime/failure handling:** a launch-local `WorkspaceLease` owning the non-`Clone`
`PreparedWorkspace`; explicit cleanup consumes it, `Drop` never runs the privileged delete — but
`Drop` (an un-consumed lease, e.g. an unexpected early return) should ATOMICALLY POISON workspace
admission and emit a critical event, not merely log. `launch_with` should become one inner
operation plus a single unconditional finalization epilogue, not per-branch duplicated cleanup,
with these rules: failure before permit commit → rollback permit, release reserve at zero, delete
workspace; failure after permit commit → terminate/reap the sandbox, settle actual-or-conservative
usage, delete workspace; settlement failure → still attempt workspace deletion; deletion/sync
failure → mark the workspace subsystem unhealthy and refuse new admissions (never fold into an
ordinary retryable "launch failed" that could cause the SAME job to execute twice).

**A real, pre-existing bug found independently while planning this (unrelated to workspace
storage, a genuine production gap in the CURRENTLY LIVE launch path), confirmed by reading the
code directly:** `run_production_container_streaming` (`gvisor.rs:918-941`) can return `Err(e)`
from `run_and_capture` for reasons that occur AFTER the sandbox has actually spawned (the comment
even says "spawning/waiting failed before a trustworthy result" — not "before spawning"). That
`Err` bubbles straight up through `launch_with`'s `run(...).map_err(GvisorError::Runtime)?` at line
~774, which returns EARLY — calling neither `hooks.release_unused` (already past permit
acquisition) nor `hooks.settle_completed`. **`hooks.reserve(spec)`'s reservation from step 4 is
never released or settled on this path — a silent, permanent capacity leak in the real, currently
live production CI launch path**, independent of and blocking-for the workspace-storage
integration (which needs an honest `NotStarted`/`Started{result, usage, error}` distinction on the
run outcome to decide release-vs-settle correctly, which today's opaque `Result<ContainerRun,
String>` cannot express). Also noted: `delete_container` is currently best-effort and ignores its
own result — workspace jobs need to confirm the runsc/gofer cgroup is actually empty and container
deletion is complete BEFORE deleting the subvolume underneath it.

**UID namespace: confirmed no shortcut exists.** `runsc --rootless` maps only the caller's host
UID/GID to container UID/GID 0; container UID 65534 (the non-root payload identity this sandbox
already uses via `--reuid`/`--regid`) has NO mapping at all — there is no host UID a workspace
could simply be `chown`'d to that would satisfy it, and no permissive mode-bit/ACL fixes an
unmapped-id `EINVAL`. Real production Rust jobs writing to `/workspace` as UID 65534 need a real
fix, not a workaround. Two choices exist: run the payload as container UID 0 (abandons this
sandbox's explicit non-root-inside invariant — rejected) or a caller-configured multi-ID OCI user
namespace (the correct path): allocate a subordinate UID/GID lease per concurrent job, map
container 0 → the runner's own identity and container 65534 → the leased subordinate ids, chown
the workspace to those mapped HOST ids, set explicit OCI user-namespace mappings, and invoke
`runsc` directly (not via its single-ID `--rootless` shortcut). A `UserNamespaceLease` type used
consistently by `run`/`kill`/`delete`, released only after runtime teardown AND workspace
deletion, tested against the exact digest-pinned `runsc` build this repo already pins.

**Explicitly out of scope for this integration, named rather than assumed:** the in-guest git
checkout transport (the scoped job-token git wire `WorkspaceSpec`'s own doc comment describes).
Ordinary gVisor launch today exposes no `repo_ref`/`commit`/usable scoped git transport to the
guest at all — `/workspace` will correctly be writable but EMPTY until that separate, required
slice lands. This integration is pure storage provisioning, never a host-side checkout.

**Assessment:** this is now clearly a multi-week body of work (a new `UserNamespaceLease`
subsystem, host-disk-capacity accounting in `reserve`, a persistent locked/poisoned workspace
manager, a generalized OCI mount abstraction, the pre-existing `ContainerRun` opacity fix as a
genuine prerequisite, plus the separately-scoped in-guest checkout transport) — not a single
slice like `workspace_storage.rs` itself was. Not started; checked in with the founder on
sequencing before touching `gvisor.rs` at all, given its criticality and the size just revealed.

## 2026-07-26 — the pre-existing `ContainerRun` reserve/settle leak: fixed, with Sol (5 review rounds)

The founder's answer to the sequencing check-in above was explicit: **"Have sol help you and go the
whole way."** This entry covers the FIRST piece of that — the pre-existing `ContainerRun`
reserve/settle leak flagged above as a genuine prerequisite — landed as its own focused unit before
starting the larger workspace-storage OCI/UID-namespace body of work.

**The bug, restated precisely:** `launch_with` (gVisor) and `launch_git_command` (the git wire) each
called `run(...)`/`run_git_wire_container(...)` and, on `Err`, returned the error immediately —
calling **neither** `hooks.release_unused` **nor** `hooks.settle_completed`. This silently leaked the
job's cost/capacity reservation on every single run failure, regardless of whether the sandbox had
actually spawned and consumed real host resources by the time it failed. Live, in the currently-wired
production launch path — not a staged/unwired module like `workspace_storage.rs`.

**Design (Sol, corrected twice from my own first two proposals):**
- My first proposal ("settle at zero on any run failure") was wrong: a subsecond failed spawn could
  legitimately be free, but a job engineered to fail exactly after a real spawn must never be charged
  zero — a real host-DoS surface. Sol's fix: a phase-tagged failure type distinguishing what was
  durably true at the moment of failure.
- My second proposal (3 phases: `Uncommitted` / `CommittedButNotExecuted` / `Executed`, dispatching
  unconditionally to `release_unused` / `settle_completed(zero)` / `settle_completed(fallback)`) was
  ALSO wrong, caught on the FIRST full-implementation review round: `settle_completed` is a documented
  no-op under `CompletionSettlementOwner::TerminalReporter` (production's real setting —
  `CiPipelineReporter::completion_settlement_owner()` returns it whenever `ReporterAccounting::Durable`
  is in play). Calling it for a post-commit failure under reporter ownership silently discards the
  accounting with **no terminal report ever following** — reproducing the exact leak class this fix
  exists to close, just relocated. Sol's corrected design added a 4th phase and made the dispatch
  depend on `CompletionSettlementOwner` too:

  | Phase                     | `Hook` owner                        | `TerminalReporter` owner                          |
  |----------------------------|--------------------------------------|----------------------------------------------------|
  | `Uncommitted`              | `release_unused`, then `Failed`      | `release_unused`, then `Failed` (owner-independent) |
  | `CommitOutcomeUnknown`     | `DurableOutcomeUnknown` (neither)     | `DurableOutcomeUnknown` (neither, owner-independent)|
  | `CommittedButNotExecuted`  | settle zero, then `Failed`           | `RetryableAttempt(SandboxInfrastructure, zero)`     |
  | `Executed`                 | settle usage, then `Failed`          | `RetryableAttempt(SandboxInfrastructure, usage)`    |

  `CommitOutcomeUnknown` covers a genuinely new case Sol caught: a `permit.commit()` **error** does
  NOT prove the underlying durable store didn't actually commit (e.g. a Postgres commit whose result
  never reached the caller) — calling this `Uncommitted` would let a caller release a reservation the
  store may still consider owned. Neither release nor settle; the existing durable lease/claim reaper
  reconciles it.

**What shipped (production `myelin-ci-sandbox`/`myelin-ci-controlplane`/`myelin-agent-service` code,
not staged):**
- `SandboxLaunchError<E> { Failed(E), RetryableAttempt{source, cause, usage}, DurableOutcomeUnknown(E) }`
  — the new shared return type for `SandboxBackend::launch`/`launch_streaming`, replacing the bare
  backend error. Deliberately NO blanket `From<E>` impl (Sol: it would let `?` silently reclassify
  every new error as an uninformative `Failed`, recreating the exact bug this closes) — every site
  must explicitly choose a variant.
- `RetryableAttemptCause` (runner.rs) gained `SandboxInfrastructure` alongside the existing
  `OutputPersistence`, plus the ONE canonical `as_storage_token()`/`from_storage_token()` mapping.
  This exposed and fixed a second, independent pre-existing bug Sol caught while reviewing the design:
  `ci_pipeline_driver.rs`'s `record_retryable_attempt_on_conn` **hardcoded** `OUTPUT_PERSISTENCE_CAUSE`
  into the persisted record regardless of `failure.cause` — harmless only because there was exactly
  one cause until now. Fixed at the write site (a small pure `expected_retry_attempt_record()` helper,
  now unit-tested to prove `SandboxInfrastructure` produces its own cause AND its own distinct receipt
  hash — a test that would have caught the original bug, unlike decode-only coverage) and generalized
  `decode_retry_attempts`'s validation from `== OUTPUT_PERSISTENCE_CAUSE` to
  `RetryableAttemptCause::from_storage_token(...).is_some()`.
- `launch_gate.rs`: `SpawnPhase` gained `CommitOutcomeUnknown`. `SandboxChild` now carries a mandatory
  `executed_at: Instant` — captured immediately before `Command::spawn` (unfenced) or immediately
  before the gate-release write (fenced), fixing a real timing bug caught in round 2 where the
  timestamp was captured AFTER the write (undercounting real execution time in every fallback
  computation, the successful `RunscOutcome.wall`, and the timeout-deadline comparison).
- `gvisor.rs`: `RunFailure` is the 4-variant enum matching the table above (`Executed`'s usage is
  mandatory, not `Option` — an "executed but no usage computed" state cannot be constructed, though a
  genuinely-zero usage still could be; `executed_fallback_usage`'s 1-second wall-floor is what
  actually prevents that in practice). `launch_with`'s post-run dispatch lives in a new
  `dispose_run_failure` method implementing the exact table. The `try_wait()` polling-loop error path
  now kills/reaps the child THEN joins the stdout/stderr/stdin drain threads THEN returns (previously
  an early `?` leaked all three threads on a wait-syscall failure — found during round 2 review).

  Git-wire (`launch_git_command`) had its OWN, structurally different instance of the same bug class,
  found while implementing the gVisor fix: it called `hooks.attribute(&job)` (commit-and-release
  attribution) BEFORE ever attempting to spawn, then passed `None` for the launch permit into
  `run_and_capture` — decoupling the durable commit from the actual OS spawn entirely, so EVERY
  post-spawn `RunFailure` was structurally mislabeled `Uncommitted` regardless of what actually
  happened. Fixed by threading the real, retained `LaunchPermit` through
  (`acquire_launch_permit` → `run_git_wire_container(..., permit)` →
  `run_and_capture(..., Some(permit))`) so the SAME durable-commit gate the CI/agent path uses also
  governs the git-wire spawn. Git-wire also now refuses up front (before reserve) if
  `hooks.completion_settlement_owner() != Hook` — it is a direct synchronous call with no terminal
  reporter above it to route a `RetryableAttempt` through, so reporter-owned hooks here would silently
  lose accounting; its own 4-phase dispatch (extracted into a standalone, unit-testable
  `dispose_git_wire_run_failure`) always settles synchronously, never emitting `RetryableAttempt`. Its
  over-cap response-truncation path (`OutputTooLarge`, a REAL completed execution — deterministic for
  the same request/limit, never retryable) now settles `result.usage` synchronously before returning,
  instead of leaving a completed attempt unsettled.
- `runner.rs`'s `run_one` (the actual CI-job poll loop): `CompletionClaim` is now constructed BEFORE
  the launch (needed inside the launch-result dispatch, which a launch-level `RetryableAttempt` can
  reach with no successful `SandboxLaunch`/handle ever having existed). Dispatches
  `SandboxLaunchError` into: `Failed` → `RunnerError::LaunchFailed`; `DurableOutcomeUnknown` → a loud
  `LaunchFailed` naming reconciliation ownership, no report at all; `RetryableAttempt` →
  `self.reporter.report_retryable_attempt(...)` then `RunnerError::RetryableAttemptRecorded` (now
  carrying a `message: String` field too, so an operator can distinguish a wait failure from an
  ownership failure from a drain failure instead of seeing only the opaque `cause` enum — the raw
  message is still NOT threaded into the durable retry-accrual record itself, matching the existing
  precedent).
- `myelin-agent-service`'s `SandboxToolHands::dispatch_compute` (the OTHER direct-synchronous-call
  site, `ToolHands::exec`'s explicit fallible form) has the exact same "no reporter above it" shape as
  git-wire — found by Sol on the FIRST full-implementation review, not something I'd considered.
  Fixed identically: refuses (`ExecError::SettlementOwnerNotHook`) before reserve/launch if hooks
  aren't Hook-owned; `ExecError::Launch` now carries the full `SandboxLaunchError<E>` instead of a
  bare backend error.
- `firecracker.rs` (confirmed dormant/unwired — `runner_bind.rs`, the real production wiring,
  constructs ONLY `GvisorBackend`): mechanically compatibility-wrapped as `SandboxLaunchError::Failed`
  (phase-unclassified — this backend doesn't yet make the 4-way distinction, so no retryable-attempt
  record is available through this return, which is honestly true today). Sol required more than a
  comment here: `production_pg_bootstrap_source.rs` (the existing source-text-inspection guard suite
  for controlplane's production wiring) now asserts `runner_bind.rs`'s source never contains
  `"FirecrackerBackend"`, failing RED the moment anyone wires it in, with a message naming this a
  production-activation blocker until Firecracker gets gVisor's same phase-aware treatment.

**Review process:** built jointly with Sol (gpt-5.6-sol) across 5 rounds on the full diff (not just
the design) — matching the rigor already established for `workspace_storage.rs`. Every round found a
genuine, concrete defect, not style preference: round 1 (design) caught my two wrong zero-usage
proposals; round 2 (full implementation) caught the reporter-ownership silent-discard bug, the
git-wire structural mislabeling, the `permit.commit()`-error ambiguity, the timing bug, the
thread-leak on `try_wait()` error, the missing agent-service parity fix, and the Firecracker
mechanical-guard gap; round 3 confirmed the fixes and asked for concrete test coverage (added: 6
gVisor phase×owner dispatch tests, 3 `run_one` dispatch tests via a new `forced_launch_error` fixture
field, git-wire's 4-phase dispatch + ownership-refusal tests, launch-gate exact-phase assertions, the
cause-decode/round-trip/write-side-binding tests) plus wording precision fixes; round 4 caught one
test assertion that didn't account for `HookError`'s own `Display` prefix (fixed) plus 3 more wording
nits; round 5 confirmed clean with no remaining findings.

**Verified:** `cargo build`/`test`/`clippy -D warnings` clean across the FULL workspace
(`--all-targets --features integration`), `myelin-lints`'s `no-host-exec` gate at 0 violations
(768 files scanned), all touched files `rustfmt --check` clean (formatted only the files this diff
actually touched, not `cargo fmt --all` — confirmed via per-file `--check` first that no unrelated
pre-existing drift got swept in). Also found and fixed, in scope, one MORE pre-existing bug from
earlier in this same effort: a doc comment (written during the design phase, before this diff's
`RunFailure` rewrite) contained the literal source text `` gate.write_all(b"launch\n") `` ABOVE the
real code, breaking `production_pg_bootstrap_source.rs`'s byte-offset ordering assertion on
`launch_gate.rs`'s source. Confirmed via `git stash` this was a real, currently-red regression from
earlier in the session, not a pre-existing failure — fixed by rewording the comment.

**Two pre-existing failures found during full verification, confirmed via `git stash` against the
base commit to be unrelated to this diff, deliberately left out of scope:**
1. `firecracker_production_launch_contains_the_corpus_non_root` (myelin-ci-sandbox) — requires real
   `/dev/kvm` + a staged kernel/rootfs + the `firecracker` binary; fails identically on the base
   commit. A genuine Firecracker escape-drill/corpus-routing issue, unrelated to gVisor/git-wire.
2. `chat_p5_co_commit_idempotent_send_and_per_conversation_order` (myelin-chat) — a hardcoded test
   ULID collides with a leftover row from an earlier run against the shared dev Postgres; fails
   identically on the base commit. Dev-DB contamination, unrelated to this subsystem.

Neither is chased further here; both are named rather than silently dropped.

**Still open (the original, much larger body of work this was a prerequisite for):** the actual
`workspace_storage.rs` wiring into `gvisor.rs` — the persistent locked/poisoned `WorkspaceStorage`
manager, host-disk-capacity accounting in `reserve`, the generalized OCI mount abstraction, and the
`UserNamespaceLease` subsystem for the UID-65534 mapping problem — none of that has started yet. This
entry closes exactly the named prerequisite, as its own focused commit.

## 2026-07-26 — agreed 4-slice sequencing for the workspace_storage.rs wiring (Sol)

With the `ContainerRun` prerequisite above landed, consulted Sol on how to slice the remaining body
of work into independently-shippable, independently-reviewable units (same discipline as the
prerequisite fix). Agreed sequencing, recorded here before any of it is implemented:

1. **Persistent workspace manager + startup reconciliation** — `GvisorBackend::try_new(registry,
   WorkspaceStorageMode, incident_sink)` (explicit, fallible construction — no silent production
   default). The manager owns: one process-lifetime directory-FD lock (`O_CLOEXEC`, or placed
   OUTSIDE the workspace base — never a lockfile inside it, since `workspace_storage.rs`'s own orphan
   scanner already rejects every non-subvolume entry as unexpected); `Mutex<WorkspaceStorage>`;
   monotonic admission state `Reconciling | Healthy | Poisoned`; active workspace metadata; host-local
   capacity accounting via non-`Clone` capacity leases; the critical-incident sink an abnormal
   `WorkspaceLease::drop` reports through. A new read-only `WorkspaceStorage::check_health()` (not
   repeated `open()` calls). Lock acquired BEFORE orphan enumeration/deletion; admission stays closed
   until every orphan delete+sync succeeds; any unexpected entry/failed delete-sync/poisoned mutex/
   abandoned capability makes poisoning MONOTONIC for that process. `Disabled` mode performs NO
   Btrfs/lock/quota/helper I/O at all. Does NOT call `create_workspace` in this slice (no owner
   UID/GID needed yet). Production passes `Disabled` explicitly until slice 4. `runner_bind.rs`
   currently constructs the backend infallibly inside the runner thread — this slice adds typed
   startup refusal before any claim is accepted. **This is the next concrete unit of work** — no OCI
   or UID-namespace dependency, independently reviewable/testable without a real `runsc`.

2. **Explicit user-namespace subsystem** — fully separate slice, large and security-critical enough
   for its own review/commit. Cross-process-safe (not process-local-counter) subordinate UID/GID
   allocation; non-`Clone` `UserNamespaceLease`; exact two-entry OCI mapping (container 0 → runner
   identity, container 65534 → the leased subordinate id); absolute pinned `newuidmap`/`newgidmap`
   invocation with cleared environment; fail-closed subordinate-range parsing/validation; a
   `RunscInvocationMode::{Rootless, ExplicitUserNamespace(UserNamespaceConfig)}` centralizing the
   currently-hardcoded `--rootless` flag (gvisor.rs's `run`/`kill`/`delete` all hardcode it
   separately today). Ordinary CI/agent launches can move to explicit user namespaces here even
   before workspace mounts exist (still `cwd="/"`, no workspace) — git-wire stays rootless unless
   deliberately migrated and drilled on its own, since it shares `run_and_capture` and the
   distinction must be explicit, never inferred. Owns the exact-pinned-runsc drill proving: ordinary
   launch succeeds without `--rootless`; OCI userns/mappings are exact; guest process is UID/GID
   65534; two concurrent leases get distinct subordinate ids; helper/allocation failure refuses
   before launch; git-wire's rootless behavior is unchanged.

3. **Workspace lifecycle integration** — only now does `PreparedWorkspace` get wired into
   `launch_with`. Refined 15-step launch order (isolation floor → hardening → resolve rootfs/deps →
   manager health/admission → `hooks.reserve` (durable/global) → host-local disk-capacity lease →
   subordinate-id lease → create/chown Btrfs workspace → build OCI config → acquire launch permit →
   run + prove the runsc/gofer cgroup is quiescent → settle/construct the correct disposition →
   delete+sync the workspace REGARDLESS of settlement outcome → release/quarantine the UID+disk
   leases → return). An explicit workspace config field inside `OciConfig` (fixed `/workspace`,
   `rw,nosuid,nodev`, `process.cwd=/workspace`, absolute rootfs path) — NOT the generic git-wire mount
   API, and no host path in `WorkspaceSpec` or any durable type. The launch-local lease's abnormal
   `Drop` must: perform no privileged deletion; poison workspace admission; emit the critical
   incident; QUARANTINE (not immediately recycle) the subordinate id + local capacity tied to the
   leaked workspace — otherwise a leaked subvolume owned by subordinate UID X could survive while X
   is reassigned to a different job. The failure matrix is the main review target (pre-reserve →
   no mutation; post-reserve/pre-commit → release+delete; commit-outcome-unknown → neither
   release/settle but still delete after quiescence; reporter-owned post-commit → preserve
   `RetryableAttempt` cause+usage through cleanup; success-then-cleanup-failure → `RetryableAttempt`
   under reporter ownership, loud `Failed` under hook ownership after settlement; cleanup failure
   must AUGMENT the original diagnostic, never collapse `DurableOutcomeUnknown` or lose retryable
   usage).

4. **Production activation + drills** — enable `EphemeralDisk` only after proving: writable
   `/workspace` + read-only root + `cwd=/workspace`; Btrfs quota exhaustion produces `EDQUOT`/`ENOSPC`
   without host overuse; aggregate+local capacity refusal before creation/spawn; timeout/cancellation/
   runsc-failure/drain-failure/settlement-failure/success ALL delete+sync; two concurrent jobs get
   distinct subvolumes+subordinate ids; a runner SIGKILL leaves an orphan that restart reconciliation
   removes BEFORE admission opens; lock contention prevents two runner processes managing the same
   base; the exact pinned runsc passes the explicit-userns workspace drill.

Not started. Slice 1 is the next concrete unit.

## 2026-07-26 — CT-007 slice 1 landed: persistent `WorkspaceManager` + boot reconciliation (Sol, 6 review rounds)

Slice 1 of the agreed 4-slice `workspace_storage.rs` → `gvisor.rs` integration (previous entry,
above) is complete: a new `crates/myelin-ci-sandbox/src/workspace_manager.rs` module, plus two
small additions to the existing `workspace_storage.rs` primitive. Deliberately decoupled from
`GvisorBackend`/`gvisor.rs` for this slice — re-reading Sol's own slice-1 description confirmed the
manager itself was the ask, not the wiring; an initial attempt to also add a `workspace` field +
`try_new` + accessor to `GvisorBackend` triggered `dead_code` under `-D warnings` (nothing consumes
it yet) and was reverted. `gvisor.rs`/`lib.rs` are back to their exact prior committed state except
for one added `pub mod workspace_manager;` line. Sol confirmed this narrower boundary was the right
call in round 2 ("The narrower slice is the right boundary"). The `GvisorBackend` wiring itself is
now deferred to slice 3, where it gets a genuine immediate production consumer (the launch path).

**What `workspace_manager.rs` provides:** one persistent `WorkspaceStorage` owned for the life of
the `GvisorBackend` process; a process-lifetime exclusive `flock` on `base_dir`'s own directory FD
(so a second runner process sharing the same base refuses at startup instead of silently
corrupting the first process's bookkeeping); boot-time orphan reconciliation (deletes every
subvolume found under the base — at boot NOTHING is active yet, so every discovered subvolume is
necessarily an orphan from a crashed prior instance) BEFORE admission ever opens; a monotonic
`WorkspaceAdmission::{Reconciling, Healthy, Poisoned}` state machine that never resets to `Healthy`
once poisoned; and non-`Clone` `CapacityLease`s bounding AGGREGATE host-disk capacity across
concurrently-running jobs (a per-job Btrfs qgroup limit alone only bounds one job's own usage, not
how many jobs run at once).

**Two additions to `workspace_storage.rs`:** `WorkspaceStorage::check_health()` — a genuinely
read-only re-verification (exclusive ownership + quota-enforcement only; never calls `open()`,
never `create_dir_all`s anything, unlike `open()` itself); and `probe_qgroup_privilege()` (test-only,
`pub(crate)`) — a read-only `btrfs qgroup show -r --raw` preflight so tests can detect a missing
`CAP_SYS_ADMIN` gap WITHOUT ever attempting a real mutating `create_workspace` just to find out (see
the leaked-subvolume incident below, which is exactly what happens when a mutating attempt's
cleanup fails for the same missing-privilege reason).

**Design/review process:** consulted with Sol (gpt-5.6-sol, persistent session `myelin-ct007`)
across 6 rounds total — 2 design rounds before implementation (recorded in the prior ledger entry),
then 4 adversarial-review rounds on the actual implementation, at the same rigor
`workspace_storage.rs` itself got. Real bugs found and fixed each round:

- **Round 1 (Sol):** `check_health()` delegated to `WorkspaceStorage::open()`, which itself
  `create_dir_all`s a missing base — contradicting its own claimed side-effect-free contract, and
  opened a second throwaway `WorkspaceStorage` instead of re-checking the one already under lock.
  Admission and capacity bookkeeping lived in two separate mutexes, racing a concurrent poison.
  `CapacityLease::release` marked itself released BEFORE the capacity update, so a poisoned lock
  would let `Drop` silently skip both the bookkeeping and the abandonment incident. A capacity
  underflow was silently absorbed by `saturating_sub`. Every mutex access used `.lock().unwrap()`,
  contradicting the documented "an internally poisoned mutex also poisons this manager" claim. 5 of
  7 tests silently skipped in any environment lacking real Btrfs+quota privilege even though the
  logic they exercised had nothing to do with Btrfs at all.
- **Round 2 (Sol):** confirmed round 1's fixes and the narrower (no-`GvisorBackend`) slice boundary.
- **Round 3 (Sol):** `check_health()`'s failure branches invoked the incident sink through a helper
  that only received `&mut MutexGuard` — a borrow, not an owned guard — so the caller's own lock was
  still held for the entire sink call despite a comment claiming otherwise; a reentrant sink would
  have deadlocked. A `CapacityLease` could outlive the manager's own directory lock (the lock lived
  directly on `WorkspaceManager` while the lease retained only `Arc<SharedState>`; dropping the
  manager while a lease was outstanding released the flock, letting a second manager falsely lock
  the same base and reconcile the first manager's still-live workspace as an orphan). Also: an
  `EphemeralDisk` manager with no open `WorkspaceStorage` was silently treated as healthy (a broken
  internal invariant); `lock_state()`'s poisoned-mutex recovery emitted no incident, contradicting
  the `IncidentSink` contract.
- **Round 4 (Sol):** the locked directory and the opened `WorkspaceStorage` were never proven to be
  the same inode — `acquire_directory_lock` records identity A from `base_dir` at one instant, but
  `WorkspaceStorage::open` independently `canonicalize`s `base_dir` a moment later; a symlink
  retargeted A → B and back to A between those two calls could leave the manager admitting against a
  `WorkspaceStorage` permanently bound to canonical B while `locked_identity` (and every later
  `check_health` call) kept reading A, both checks passing independently while the admitted
  capability protected a DIFFERENT directory than the one the flock actually covers.
- **Round 5 (Sol):** boot-time orphan deletion is exactly the kind of operation
  `WorkspaceStorage::check_health`'s own doc warns can leave Btrfs quota inconsistent — the
  original `open()` call's quota-enforcing check (taken BEFORE any deletion) did not prove quota was
  still enforcing after reconciliation deleted subvolumes.
- **Round 6 (Sol):** confirmed the final ordering closes both gaps — **slice 1 cleared to commit.**

**What shipped, mechanically:** `try_new`'s `EphemeralDisk` branch now runs: acquire lock → record
identity → `WorkspaceStorage::open` → require locked identity (base_dir AND storage's own canonical
base_dir both match) → `reconcile_orphans_at_boot` → require locked identity again →
`storage.check_health()` (re-validates quota post-deletion) → require locked identity a third time
→ only then set `Healthy`. The directory lock (`OwnedFd`) moved off `WorkspaceManager` entirely onto
`Arc<SharedState>`, which every `CapacityLease` already held — the lock now survives as long as
EITHER the manager or any outstanding lease is alive, whichever drops last. `check_health()` (the
periodic path) mirrors the same three-layer discipline: base-dir identity → storage-base-dir
identity → `storage.check_health()` → a final identity recheck before ever returning `Ok`. Every
failure path funnels through one `poison_and_report(state: MutexGuard<...>, error)` method that
takes the guard BY VALUE specifically so it — not the caller — controls when the lock drops before
the incident sink is invoked.

**Tests:** 21 tests in the two modules combined (14 in `workspace_manager` — 12 always-on via a
`#[cfg(test)]`-only `new_for_state_tests` constructor that takes the same real directory lock but
skips real Btrfs, plus 2 privileged real-Btrfs-lifecycle tests; 7 pre-existing in
`workspace_storage`). Tests added across the review rounds specifically for Sol's findings: a
reentrant-sink test through `check_health()`'s own failure path (proving no deadlock — the earlier
`a_panicking_incident_sink_never_escapes_poison` test only exercised `poison()`'s own call path via
an abandoned lease); a test that drops the manager while a lease is still outstanding and proves a
second manager still gets `AlreadyLocked`; a pure-function test proving `require_locked_identity`
catches a storage-base-dir mismatch even when `base_dir` alone still matches. Two nonblocking
hygiene fixes from round 4: the reentrant-sink test's `Arc` cycle (sink held a strong `Arc` back to
the slot containing its own manager) fixed via `Weak`; `probe_qgroup_privilege`'s and
`ephemeral_disk_available`'s doc comments narrowed to state they confirm `CAP_SYS_ADMIN` only, never
`CAP_CHOWN` (a host could pass the probe and still fail the real lifecycle test at the `chown` step
— that path is still exercised honestly by the one test that reaches `create_workspace` for real).

**Verification, final round:** `cargo build` (crate + full workspace) clean; `cargo clippy
--all-targets` with both `--features integration` and `--features integration,test-support`, `-D
warnings`, clean on both; `rustfmt --check` confined to touched lines only; `myelin-lints`
`lint-gate` clean (769 files, 0 violations); full `myelin-ci-sandbox` test suite (`--all-targets
--features integration`): 191 lib tests + all integration test files pass. One pre-existing,
unrelated failure remains and is NOT chased here (confirmed via `git stash` to fail identically on
base HEAD, already named in the prior ledger entry):
`firecracker_production_launch_contains_the_corpus_non_root` — requires real `/dev/kvm` + a staged
kernel/rootfs + the `firecracker` binary reaching a real guest boot; this session has `/dev/kvm` and
the binaries present but the corpus's guest stdout comes back empty (`exit=None timed_out=false`),
an existing Firecracker escape-drill/corpus-routing gap unrelated to gVisor or this workspace-manager
work.

**Operational debt — two leaked Btrfs subvolumes, still not cleaned up:** while developing this
slice's own test harness, an early flawed privilege-probe helper (since replaced by the read-only
`probe_qgroup_privilege` described above) called `create_workspace()` directly and only pattern-matched
`QuotaLimitFailed`/`OwnershipFailed` on a missing-privilege error — but when the qgroup-limit step
fails for lacking `CAP_SYS_ADMIN`, the best-effort cleanup delete fails for the SAME reason,
producing `WorkspaceStorageError::UnrecoverableLeak` instead (a variant that helper didn't handle),
and two real, small (~8 MiB quota each) Btrfs subvolumes were created and left stuck on this host's
actual root Btrfs filesystem before the bug was caught:

- `$HOME/.local/state/myelin-workspace-manager-tests-boot-reconcile-*/privilege-probe`
- `$HOME/.local/state/myelin-workspace-manager-tests-health-check-real-happy-path-*/privilege-probe`

Both need a privileged `btrfs subvolume delete` to remove, which this sandboxed session cannot
perform (confirmed via `sudo -n true` failing — no passwordless sudo available here). Per Sol's
explicit guidance, this is acceptable to commit past as long as the exact locations are durably
recorded and the cleanup remains tracked — which this entry does. **This debt must be verified
cleaned up (with a privileged session) before slice 4's production-activation drills**, and should
not be allowed to silently age past that point.

**Still open (superseded by the slice-2 entry below):** slice 2 (explicit user-namespace
subsystem), slice 3 (workspace lifecycle wired into `launch_with`, which is also where
`GvisorBackend`'s own `workspace_manager` field/accessor finally lands), slice 4 (production
activation + drills, which must also verify the leaked subvolumes above are gone). None of the 4
slices are wired into production yet — `EphemeralDisk` is not constructed anywhere outside this
module's own tests; `runner_bind.rs` is untouched.

## 2026-07-26 — CT-007 slice 2 landed: explicit user-namespace subsystem (Sol, 9 review rounds)

Slice 2 of the agreed 4-slice `workspace_storage.rs` → `gvisor.rs` integration (prior entry, above)
is complete: a new `crates/myelin-ci-sandbox/src/user_namespace.rs` module (`UserNamespaceLease`/
`UserNamespaceAllocator`), a new shared `crate::dirlock::verify_ancestors_not_writable_by_us`
primitive (used by both this module and `gvisor.rs`), and substantial `gvisor.rs` additions wiring
`RunscInvocationMode::ExplicitUserNamespace` into the real `run`/`kill`/`delete` invocation path.
Same deliberate scope discipline as slice 1: `GvisorBackend` itself is still untouched outside its
own `invocation_mode()`/`to_json()` plumbing — wiring a real lease into `launch_with`'s lifecycle is
slice 3's job, which is also where the deferred `UserNamespaceQuiescenceProof` production
constructor finally lands.

**What `user_namespace.rs` provides:** `UserNamespaceAllocator` — a `WorkspaceManager`-shaped
persistent, process-lifetime-locked owner of subordinate-uid/gid-slot admission and leasing, parsing
`/etc/subuid`/`/etc/subgid` once at construction, backed by durable JSON marker files
(`slot-<N>.json`) as the crash-recovery source of truth, with FD-relative (`openat`/`unlinkat`,
never path-based) marker I/O against a held directory-lock FD. `UserNamespaceLease` — a non-`Clone`
hold on exactly one subordinate uid/gid pair, releasable only against a
`UserNamespaceQuiescenceProof` bound to that lease's own nonce; an unreleased (abandoned) lease
quarantines its slot forever rather than risking reissue. `UserNamespaceConfig` — opaque (private
fields, accessors only), mintable only via `UserNamespaceLease::config()` or a `#[cfg(test)]`
constructor, so no caller can forge a mapping bypassing the allocator. A constructor split
(`try_new` — strict, hardcoded to the real `/etc/subuid`/`/etc/subgid`, no mutation, requires
pre-provisioned state — vs. `#[cfg(test)] try_new_for_tests` — relaxed, fixture paths, auto-creates
convenience state) mirrors `workspace_manager.rs`'s own production-vs-test split, needed here because
this module's strict hardening genuinely cannot be satisfied by non-root test fixtures.

**What `gvisor.rs` gained:** `OciConfig::user_namespace: Option<UserNamespaceConfig>` as the ONE
source of truth `invocation_mode()` derives `RunscInvocationMode` from (mismatched
mode/mapping combinations are unrepresentable, not merely validated); `to_json()` emits
byte-identical JSON to before this slice when `None` (today's only production behavior,
untouched), or the exact two-entry `uidMappings`/`gidMappings` OCI `user` namespace when `Some`.
`apply_runsc_invocation_policy` is the ONE place `run`/`kill`/`delete` decide global flags AND
environment — no call site makes an independent decision. A `ResolvedExplicitUsernsPolicy`
(helper directory + runsc state root, installed atomically together) plus a pinned
version+content-digest check (`PINNED_EXPLICIT_USERNS_RUNSC_VERSION`/`_SHA256_HEX`, validated
against the exact `release-20260608.0` build this integration's live spike was developed against)
gate `ExplicitUserNamespace` mode entirely: it REFUSES outright — never falling back to ad hoc
unvalidated resolution — unless `preflight_explicit_userns_policy` has already succeeded.

**Design/review process:** consulted with Sol (gpt-5.6-sol, persistent session `myelin-ct007`)
across 9 rounds — far more than slice 1's 6, reflecting how much more adversarial surface a new
security-load-bearing subsystem (real host uid/gid mappings, a new binary-execution trust
boundary) exposes versus slice 1's storage-accounting scope. Real, distinct bugs found and fixed
each round:

- **Round 1:** boot reconciliation parsed markers through `serde_json::Value` first to peek
  `schema_version` — `Value` cannot losslessly represent a real random `u128`, so every genuine
  marker was rejected as corrupt (fixed via a minimal `SchemaPeek` struct deserialized directly from
  the raw string). Path-based TOCTOU on marker create/delete (fixed via FD-relative `openat`/
  `unlinkat` against the held lock FD). The single most protracted investigation of this slice: an
  intermittent "two threads leased the same host_uid" failure that survived multiple standalone
  minimal reproductions of `openat`/mutex primitives (all clean) before the true root cause was
  found — not an allocator bug at all, but the TEST releasing leases mid-loop while sibling threads
  were still retrying, legitimately re-leasing a freed slot. Fixed by restructuring the test to
  collect all leases before asserting uniqueness, and the module's own doc comments (which had
  briefly, incorrectly blamed "directory-listing staleness" from an earlier wrong diagnosis) were
  corrected to describe the actual, verified cause.
- **Round 2:** the directory lock protected against a REPLACEMENT directory but not this SAME
  process renaming `leases_dir` away and creating a fresh one at the same path (closed then via an
  immediate-parent-only `access(2)` check — later found insufficient, see round 3). Marker reads had
  a leaf-level TOCTOU and were unbounded/blocking. Identity-range validation was incomplete (no
  root-runner refusal, no self-uid-in-range refusal, no enforced minimum pool size). `gvisor.rs`'s
  `ExplicitUserNamespace` mode needed environment-clearing (`runsc` invokes `newuidmap`/`newgidmap`
  internally, not this process — confirmed against gVisor's own docs) plus a fixed `--root=`.
- **Round 3:** the round-2 `access(2)`-based parent check missed three live attacks: owning the
  parent (mode alone doesn't stop a `chmod`), a writable GRANDPARENT with a safe-looking immediate
  parent, and a symlinked ancestor — fixed with a full `openat(O_NOFOLLOW)` ancestor walk from `/`
  down, checking both ownership and `faccessat(AT_EACCESS)` writability at every level (later
  extracted into the shared `dirlock.rs` primitive both this module and `gvisor.rs` use). `lease()`
  only consulted `active_slots`/`quarantined_slots` AFTER an `EEXIST`, not before attempting create —
  fixed to skip known slots unconditionally first. The `runsc` invocation policy was resolved via
  three independently-`OnceLock`-cached values with no binding between "validated" and "used."
  Subordinate-range overlaps between different owners were silently accepted.
- **Round 4:** the round-3 `faccessat` ancestor check treated ANY nonzero result as "not writable" —
  only `EACCES` actually proves that; every other errno (`EINVAL`/`ENOSYS`/`EBADF`) was fail-OPEN.
  The round-3 ownership test drove the full ancestor walk, which refused at `/tmp` before ever
  reaching the fixture it claimed to test — isolated into a standalone, directly-testable function.
  A REAL bug of this session's own making: `NoSubordinateEntry` was split out of `SubordinateConfig`
  this round, but the live drill's error-variant match was never updated to match it — found by Sol,
  fixed. `PINNED_EXPLICIT_USERNS_RUNSC_VERSION` existed but nothing checked the binary's own content
  digest, so a same-version rebuild would pass — added a real SHA-256 pin.
- **Round 5:** the digest pin verified a PATH, not the binary eventually executed — nothing stopped
  the runsc binary (or the state-root directory) from being replaced between preflight and any later
  launch. Closed via the SAME ancestor-immutability + ownership/mode hardening already proven for the
  leases directory, applied to both the runsc binary's parent chain and the state-root's.
- **Round 6:** two more severe, distinct bugs. (1) `verify_pinned_explicit_userns_runsc` executed
  `bin --version` BEFORE checking the digest — an unvalidated, potentially malicious candidate got
  arbitrary host execution before this function could ever reject it; fixed to hash first, execute
  only on a matching digest, and to check `output.status.success()` (previously unchecked). (2) the
  new state-root hardening (and, per Sol's extension, the PRE-EXISTING leases-directory hardening
  from round 2) auto-created the leaf via `create_dir_all` even in strict mode — internally
  contradictory, since creating a missing leaf requires write access to its own parent, exactly what
  the ancestor-immutability check exists to forbid; fixed in both places to perform NO mutation in
  strict mode, requiring the leaf to be pre-provisioned instead.
- **Rounds 7–9:** the drill's own standalone pin-check call, run before hardening, still violated
  the (now-fixed) function's documented precondition — removed entirely, relying solely on
  `preflight_explicit_userns_policy`'s correct ordering. Both new leaf-mode checks rejected
  group/other bits but not missing owner `rwx` (a `0500`/`0000` directory would have passed) — fixed
  in both `gvisor.rs` and `user_namespace.rs`. Finally, the drill validated the process-global cached
  `runsc_bin()` via preflight but then launched/deleted using its OWN separately-PATH-resolved `bin`
  — structurally capable of validating binary A and executing binary B — fixed by removing the
  drill's independent resolution and using `runsc_bin()` throughout. **Round 9: cleared to commit.**

**Verification, final round:** `cargo build` (crate + full workspace) clean; `cargo clippy
--all-targets` with `--features integration`, `--features integration,test-support`, and no
features, all `-D warnings`, clean; `rustfmt --check` clean (both touched files — `gvisor.rs` is
tracked/committed, `user_namespace.rs`/`dirlock.rs`'s additions are new/uncommitted this slice, so a
whole-file reformat carried no risk of diverging from a committed baseline); `myelin-lints`
`lint-gate` clean (771 files, 0 violations); full `myelin-ci-sandbox` test suite (`--all-targets
--features integration`): 245 lib tests pass (up from 191 at the end of slice 1), plus all
integration test files. `concurrent_lease_calls_never_poison_the_allocator` specifically run 40
times in a tight loop with 0 failures, both after this slice's own concurrency fix and again after
every subsequent round's changes. Only the SAME pre-existing, unrelated
`firecracker_production_launch_contains_the_corpus_non_root` failure remains (named in the slice-1
entry above; not chased here).

**Operational debt — the strict production-layout path has never been genuinely live-drilled, by
explicit agreement with Sol (not an oversight):** this development host's real `runsc` binary
(`~/.local/bin/runsc`) and the default explicit-userns state-root
(`~/.local/state/myelin-runsc-explicit-userns`) both live under this runner's OWN home directory
tree — exactly the deployment layout the round-5/6 hardening now correctly REFUSES (a non-root-owned
binary/directory, or one reachable through a runner-writable ancestor, can be replaced by the
runner itself, defeating the whole point of pinning it). Consequently:

- The live drill (`gvisor::tests::explicit_user_namespace_boots_through_the_real_production_run_path`)
  currently SKIPS on this host — it prints
  `SKIP: preflight_explicit_userns_policy failed: "/home/adhv/.local/bin/runsc" must be owned by
  root (uid 0), got uid 1000` and returns early (reporting `ok`, per its own established
  skip-gracefully contract). It does NOT currently prove the fully-hardened strict production path
  boots a real container end-to-end.
- The drill's `UserNamespaceAllocator` is constructed via `try_new_for_tests` (the relaxed,
  fixture-path test constructor), never the strict production `try_new` — mirroring the EXACT same,
  already-accepted gap slice 1's own leases-directory drill has had since round 2 of that slice's
  review.
- Individual mechanisms (ancestor-chain immutability, ownership/mode enforcement, symlink refusal,
  hash-before-exec ordering, no-mutation-in-strict-mode) are each independently proven via 15+
  isolated unit tests added across rounds 3–7 — but no SINGLE test currently exercises all of them
  together against a genuinely root-owned, immutably-anchored deployment.
- **Before slice 4 can claim this subsystem is live-proven, it must run the fully-hardened strict
  path once for real**, with: a root-owned pinned `runsc` binary under an immutable ancestor chain;
  pre-provisioned, runner-owned state-root and lease directories under root-owned immutable parents
  (e.g. `/var/lib/myelin/...`, with only the leaf delegated to the runner); and a second preflight
  run AFTER the container completes, proving `runsc` itself did not leave the state-root in an
  incompatible mode. This requires `sudo`/root-level host setup this session deliberately did not
  perform without a fresh, explicit ask outside of Sol's own review — Sol agreed this is acceptable
  to defer, not a slice-2 commit blocker, given it exactly mirrors slice 1's own accepted gap.
- **Sequencing constraint slice 3/4's activation wiring must honor:** `preflight_explicit_userns_policy`
  must run BEFORE any generic version-probing preflight (e.g. anything shaped like
  `preflight_gvisor_runner_host`) that could execute the same candidate binary — running the generic
  probe first would re-open the exact exec-before-verify TOCTOU round 6 closed for THIS preflight
  specifically.

**Still open:** slice 3 (workspace lifecycle wired into `launch_with` — also where `GvisorBackend`'s
own `workspace_manager`/`user_namespace_allocator` fields and the deferred
`UserNamespaceQuiescenceProof` production constructor finally land), slice 4 (production activation
+ drills, which must verify: the slice-1 leaked Btrfs subvolumes are gone, AND run this slice's
own genuine strict-path live drill per the operational debt above). Neither `EphemeralDisk` nor
`ExplicitUserNamespace` is constructed anywhere outside test/drill code yet; `runner_bind.rs` remains
untouched.

## 2026-07-27 — CT-007 slice 3 landed: `Enabled` workspace/userns lifecycle wired into `launch_with` (Sol, pieces 7a–7c)

Slice 3 of the agreed 4-slice `workspace_storage.rs` → `gvisor.rs` integration (both prior entries,
above) is complete, split into three committed pieces:

- **Piece 7a** (`aa339e7f`): `container_id` generation and the `runsc_root` identity revalidation
  moved UP into `launch_with` itself (previously buried inside the run closure), since the `Enabled`
  path needs both values before it can durably bind a lease — ahead of any actual binding logic.
- **Piece 7b** (`6678031b`): checked runtime finalization — `RuntimeQuiescenceEvidence`/
  `RuntimeFinalization`/`finalize_runtime` replace the old best-effort `runsc delete`/`cgroup.cleanup()`
  pair with a VERIFIED teardown (confirmed container delete, confirmed cgroup quiescence, confirmed
  namespace identity unchanged) whose failure now changes the disposition `launch_with` sees, instead
  of being silently swallowed. Fixed a real pre-existing leak in the same pass: a bundle directory
  from a successful run that was later converted to a failure (teardown found an issue) was never
  removed. Still Rootless-only — `WorkspaceIntegration::Enabled` remained unconsumed.
- **Piece 7c** (`726e7e6a`, this entry's main subject): `launch_with` now fully consumes `Enabled` —
  health checks before `reserve`, capacity+lease+workspace acquisition, a durable lease `bind()`
  immediately after cgroup creation (and ONLY there — never before, never followed by
  `run_and_capture` on a bind failure), evidence-validated workspace deletion before lease release on
  clean finalization, and conservative abandonment of BOTH resources on the structurally-impossible
  post-bind outer failure. `GvisorWorkspaceConfig`/`GvisorBackendInitError`/`GvisorBackend::try_new`
  are `pub` as of this piece — the first commit where a caller of the public `gvisor` module can
  actually construct and run the `Enabled` path.

**Key new types/functions (piece 7c, `gvisor.rs`):** `LeaseBindState` (`Allocated`/
`Bound{container_id, runsc_root_identity, cgroup_identity}`/`Unreleasable`) — `launch_with`'s own
durable-bind-progress memory, since the actual `bind()` call happens one frame down inside
`run_production_container_streaming`. `EnabledLaunchContext` (owns `ManagedWorkspace` +
`UserNamespaceLease` + `bind_state`) and `RuntimeBinding`/`RuntimePreparation` (validates the OCI
layout agrees with the prepared mode via `require_oci_layout_matches_prepared_mode` before any
downstream code can act on it). `bind_enabled_lease_given` and `bind_then_continue` — the bind
classification and bind-then-capture composition, both deliberately decoupled from
`EnabledLaunchContext`/`RuntimePreparation` (taking a bare `&mut UserNamespaceLease`/
`&mut LeaseBindState` instead) specifically so they are unit-testable against a real, cheap lease
with NO privileged Btrfs object required. `classify_workspace_deletion` /
`delete_workspace_then_release_lease_if_absent` / `cleanup_pre_bind_failure` /
`settle_enabled_workspace_and_lease` implement Sol's full pre-bind/post-finalize disposition matrix
(capacity refusal releases nothing else; recoverable provisioning failure releases the lease;
`Storage(UnrecoverableLeak)` quarantines it; `DeleteWorkspaceError::InternalInvariantViolated` proves
disk absence and still releases the lease while surfacing the bookkeeping error; a `Storage`/sync
failure proves nothing and retains the lease). `augment_run_failure_message` /
`augment_settled_result_with_enabled_cleanup_failure` keep workspace/lease cleanup failures a
SEPARATE safety domain from `RuntimeTeardownError` (never fabricating a fake teardown issue just to
reuse its plumbing) while still compounding into the primary result — a clean success becomes
`Executed` with its measured usage; an existing failure keeps its exact phase/usage and gains the
diagnostic. `discard_container_run(run, skip_path_kill)` factored out of
`discard_container_run_after_teardown_failure` so the enabled-cleanup discard path has a real,
non-fabricated entry point. `UserNamespaceQuiescenceProof::from_runtime_evidence` (`user_namespace.rs`)
is the first production constructor for this type — takes the nonce directly from the lease (never
caller-suppliable) and refuses `Rootless` evidence.

**Design/review process:** design discussed with Sol before implementation (locked: bind immediately
after cgroup creation with no `SandboxCommand` callback; delete-before-release ordering; workspace
cleanup as a separate safety domain; one commit for the whole activation, never a half-wired
intermediate state). Piece 7c itself then went through 4 adversarial rounds:

- **Round 1 (4 blockers):** pre-permit cleanup failures were silently discarded/replaced instead of
  compounded (fixed via `dispose_run_failure`/`augment_run_failure_message` reuse everywhere);
  `Bound` and `Unreleasable` were incorrectly handled identically in `cleanup_pre_bind_failure` (split
  into three arms, `Bound` now ALWAYS abandons both and ALWAYS surfaces an invariant-violation
  diagnostic); workspace cleanup was folded into `RuntimeTeardownError` (a domain conflation —
  extracted into the generic `augment_run_failure_message`/`augment_settled_result_with_enabled_cleanup_failure`
  pair instead); the `Enabled` constructor remained `pub(crate)` with stale "not yet consumed"
  comments despite this being the activation commit (promoted to `pub`).
- **Round 2 (2 code blockers + 2 coverage gaps):** the acquisition-failure Err arm still lost its
  original error to a bare `?` on `release_unused` (fixed via `dispose_run_failure` reuse); the
  Enabled-cleanup discard path fabricated an invalid empty `RuntimeTeardownError` just to reuse
  `discard_container_run_after_teardown_failure`'s signature (fixed by extracting
  `discard_container_run` as its own real entry point); no test proved a bind failure actually
  prevents the capture continuation (fixed via the new `bind_then_continue` seam + 4 tests using a
  bare counting closure); and — flagged as a hard blocker for THIS activation commit specifically,
  since `try_new` is now `pub` — no test exercised the real `GvisorBackend::try_new(Enabled)` +
  `.launch()` path at all.
- **Round 3 (1 blocker):** the new live drill's own `leases_dir` was self-generated under
  `std::env::temp_dir()`, which the STRICT production `UserNamespaceAllocator::try_new` can NEVER
  accept (it requires a pre-provisioned, euid-owned, mode-0700-or-stricter leaf under a
  non-writable-by-us ancestor chain) — making the drill catch-and-skip a GUARANTEED refusal on every
  host, an unconditional skip that proved nothing. Fixed via a `MYELIN_USERNS_DRILL_LEASES_DIR` env
  var: the drill now skips ONLY when that's unset (it has no business fabricating the directory
  itself — that's a real operator install step), and once set, requires `try_new(Enabled)` to
  succeed outright (no more catching arbitrary constructor errors) — reaching that point asserts a
  correctly-provisioned host, so any further failure is a genuine regression. Also fixed a stale doc
  comment on `cleanup_pre_bind_failure` that still described the pre-round-1 Bound-treated-like-
  Unreleasable behavior.
- **Round 4: cleared to commit**, no remaining blockers.

**Verification, final round:** `cargo build`/`cargo clippy --all-targets -D warnings` clean across
no-features / `--features integration` / `--features integration,test-support`; `rustfmt --check`
clean; full `myelin-ci-sandbox` lib suite: 357 tests pass (124 in the `gvisor` module alone, up from
99 at the end of piece 7b); `gvisor_prod_exec_test` 6/6, `escape_drill_gvisor_test` 1/1, and
`escape_prod_path_test`'s gvisor half all green; `cargo build --workspace` + `cargo clippy --workspace
--all-targets -D warnings` clean; `myelin-lints` `lint-gate` clean (771 files, 0 violations); `runsc
list` shows no leaked containers, host disk healthy (771G free of 1.9T). `git_wire_prod_exec_test`
still shows only the SAME pre-existing, already-tracked "runsc stdin pipe unavailable" flake (task
#33, confirmed unrelated across every check this slice). One NEW unrelated finding this slice:
`escape_prod_path_test::firecracker_production_launch_contains_the_corpus_non_root` fails on this
host — confirmed unrelated to this slice (no Firecracker code touched), an environment gap worth its
own investigation, not chased here.

**Operational debt carried forward (by explicit agreement with Sol, mirroring slices 1/2's own
accepted gaps):** this development host lacks `CAP_SYS_ADMIN`, so every test that needs a REAL,
privileged `ManagedWorkspace` (`create_workspace`, needing `btrfs qgroup limit`) skips gracefully here
— the full acquire/bind/settle state machine against a genuinely privileged host has never run on
this machine; only the deterministic `_given`/decoupled seams (`bind_enabled_lease_given`,
`bind_then_continue`, `classify_workspace_deletion`, `delete_workspace_then_release_lease_if_absent`
with an injected `delete_workspace` operation) are proven to run for real here. Separately, the new
`explicit_user_namespace_boots_through_the_real_enabled_backend_and_launch` live drill (through the
real `GvisorBackend::try_new(Enabled)` + `.launch()`) is genuinely runnable — the code compiles and
type-checks against the real production signatures, and its skip condition is now narrow and correct
— **UPDATE 2026-07-27: this has since been exercised to a real, reproducible pass — see the dated
entry below.** The gap as originally written (no root, no `CAP_SYS_ADMIN`) was accurate at the time;
it no longer is.

**Still open:** slice 4's CODE-side wiring (`runner_bind.rs` constructing `GvisorBackend::try_new(..,
GvisorWorkspaceConfig::Enabled { .. }, ..)` instead of `::new(..)`, `main.rs`'s
`preflight_runner_host()` additionally calling `preflight_explicit_userns_policy`), and the slice-1/2
leaked-Btrfs-subvolume + strict-runsc-path drills those slices deferred. The pre-existing
`git_ref_updated_provider_consumer_wire_shape_round_trips`/git-wire stdin-pipe flakes (task #33) and
the newly-noted Firecracker corpus-launch failure remain unaddressed, tracked separately from this CI
track's own scope.

## 2026-07-27 — the `Enabled` activation path live-proven for real; host-provisioning artifacts landed

The operational debt the slice-3 entry above named — "someone with root on a correctly provisioned
host must actually run this drill to a real pass" — is closed. With the user's explicit, repeated
sudo authorization this session, the full `explicit_user_namespace_boots_through_the_real_enabled_
backend_and_launch` drill was run to a genuine, reproducible pass (multiple consecutive runs, zero
leaked Btrfs subvolumes/qgroups/lease markers each time): real subvolume creation, real `btrfs qgroup
limit`, real `chown` to the job's userns-mapped uid/gid, real memory cgroup creation, a real durable
lease bind, a real `runsc` container boot under the explicit user-namespace mapping (guest reporting
`uid=65534`/`gid=65534` as expected), and clean teardown/workspace-deletion/lease-release.

Getting there surfaced real, previously-undiscovered host-provisioning requirements beyond what the
piece 7c code review alone could find (none of these are code bugs — all are deployment-shape facts
about running this specific `CAP_SYS_ADMIN`/explicit-userns combination for real):

- The pinned `runsc` binary and its explicit-userns state-root must live under a genuinely root-owned,
  ancestor-immutable path — this development host's usual `~/.local/bin/runsc` fails
  `harden_explicit_userns_runsc_binary` precisely as designed. Relocated to `/opt/myelin/bin/runsc`
  (root:root 0755) with a separate `/opt/myelin/gvisor-runsc-root` state-root leaf, same digest,
  confirmed matching the pinned `PINNED_EXPLICIT_USERNS_RUNSC_SHA256_HEX`.
- Granting `CAP_SYS_ADMIN` via `setcap` on the compiled TEST BINARY does not work: per
  `capabilities(7)`, executing a binary that carries its own file capabilities clears the process's
  ambient capability set entirely, so the capability never reaches the `btrfs`/`chown` children it
  spawns — confirmed by direct reproduction (a minimal Rust binary with no file capabilities of its
  own DID propagate ambient `CAP_SYS_ADMIN` to a `btrfs qgroup limit` child correctly; the exact same
  code compiled into the real, previously-`setcap`'d test binary did not, until the leftover file
  capability was removed). The correct mechanism is `AmbientCapabilities=` on a systemd unit (or
  `systemd-run --property=AmbientCapabilities=`), which was not previously used anywhere in this repo.
- `CAP_CHOWN` is a SEPARATE capability from `CAP_SYS_ADMIN`, needed for the ownership handoff to the
  job's userns-mapped uid/gid — easy to miss since only `CAP_SYS_ADMIN` was anticipated from the
  Btrfs qgroup requirement alone.
- Restricting `CapabilityBoundingSet` to just the two capabilities above breaks `runsc`'s
  `newuidmap`/`newgidmap` setuid-root escalation outright (`newuidmap failed: ... operation not
  permitted`) — a setuid-root exec can never gain a capability outside the CALLING process's bounding
  set, so the bounding set must stay at its default (full) rather than being narrowed alongside
  `AmbientCapabilities`.
- `MemoryCgroup::create`'s sibling-cgroup design needs real cgroup v2 delegation
  (`Delegate=memory`+`DelegateSubgroup=supervisor`), and the `+memory` `cgroup.subtree_control` write
  this requires MUST happen from the unit's `ExecStart`, never `ExecStartPre` — the identical write
  from `ExecStartPre` deterministically fails the entire unit's startup with `Failed to spawn
  executor: Device or resource busy` (a systemd/kernel cgroup-v2 ordering interaction confirmed by
  direct, repeated, isolated reproduction), while the same write from `ExecStart` (once the process
  has already settled into its final `supervisor/` location) succeeds every time.

**New artifacts:** `scripts/install-ci-runner-host.sh` (idempotent host provisioning — service
account, pinned-binary placement, directory shapes — following `docs/edge-deployment.md`'s existing
path conventions), `deploy/systemd/myelin-ci-controlplane.service` (the first systemd unit in this
repo, encoding every finding above), `docs/ci-runner-deployment.md` (the operator-facing runbook,
including a copy-pasteable recipe for re-running the live drill against a real host without
installing the service). Also fixed a real bug the drill's own workspace_base_dir hit:
`std::env::temp_dir()` (`/tmp`) is frequently tmpfs, not Btrfs — rooted under `$HOME/.local/state`
instead, matching every other real `WorkspaceManager` fixture in the file.

**Still open (unchanged from above):** the CODE-side wiring in `runner_bind.rs`/`main.rs` that would
make any of this reachable through the real `ci-controlplane` binary rather than the test drill.
Once that lands, this session's host-provisioning artifacts should need no further changes — they
were validated against the exact `GvisorBackend::try_new(Enabled)` + `.launch()` call shape that
wiring will use.

## 2026-07-27 — CT-007 slice 4 landed: production activation wired (Sol, 2 review rounds)

The "still open" item directly above is closed: `ci-controlplane` now actually constructs
`GvisorBackend::try_new(GvisorWorkspaceConfig::Enabled { .. })` in production, gated behind a new,
independent `MYELIN_CI_WORKSPACE_MODE` env var (separate from `MYELIN_CI_RUNNER`, which only gates
whether the runner host exists at all). Commit `42a240a0`.

**Shape (designed with Sol before writing code, per this track's standing process for a change that
activates a new production code path):** `MYELIN_CI_WORKSPACE_MODE` parses strictly — unset/
`disabled` → `Disabled` (the rootless base, unchanged from before this slice); `enabled` → requires
four absolute-path/positive-integer variables (`MYELIN_EXPLICIT_USERNS_RUNSC_ROOT`,
`MYELIN_USERNS_LEASES_DIR`, `MYELIN_CI_WORKSPACES_DIR`, `MYELIN_CI_WORKSPACE_CAPACITY_BYTES`, no
default for capacity — an operator/storage-layout decision) plus an optional
`MYELIN_EXPLICIT_USERNS_HELPER_DIR` (defaults to `/usr/bin`); anything else refuses loudly, never a
silent downgrade to `Disabled`. Parsed and preflighted exactly once in `main()`, before PostgreSQL
bootstrap, and carried as one owned value all the way to `CiRunnerLoop::new` (a new mandatory
constructor parameter) — activation can never diverge between what was preflighted and what the
runner thread constructs against. `min_pool_size` is fixed at 1 (`RunnerAgent::run_one` performs
exactly one synchronous launch at a time; a larger pool would be arbitrary slack). Preflight order is
explicit-userns first, then the always-required rootless preflight, with the explicit-userns step
short-circuiting the rootless one on failure. Backend-construction failure is a new typed,
non-panicking `CiRunnerLoopExit::SandboxBackendInitializationFailed` /
`CiRunnerHostFailure::SandboxBackendInitializationFailed`, surfaced through the existing
`classify_runner_lane` path exactly like every other runner-lane failure — never a `process::exit` on
the runner thread. The construction incident sink is a plain `eprintln!` naming the worker id.

**What review round 2 caught (both fixed before commit):** the original env-parsing seam accepted
every `Enabled`-only variable as an already-evaluated `Result`, so the Disabled/unset-mode test could
only prove the parser *ignored* an already-read (poisoned) value — not that it never read the real
environment variable at all. Fixed by making those five reads lazy (`impl FnOnce() -> Result<...>`
closures, only invoked once mode resolves to `enabled`), with a new test that hands the parser a
panicking closure for every Enabled-only argument in Disabled/unset mode — if the parser ever called
one, the test panics rather than merely asserting a value. Also: the optional helper-dir variable
silently defaulted to `/usr/bin` for an explicitly empty value (only `NotPresent` should default) and
its relative-path case was untested; both fixed, and the relative-path test now covers all four
Enabled-only directories individually, not just `runsc_root`. A required host-failure-propagation
test for the new `SandboxBackendInitializationFailed` variant was also added, mirroring the existing
`static_runner_refusal_stops_async_intake_and_surfaces` shape.

**Deployment artifacts updated in the same commit:** `deploy/systemd/myelin-ci-controlplane.service`
now ships the `MYELIN_CI_WORKSPACE_MODE=enabled` block uncommented as the checked-in unit's canonical
target posture (an operator wanting rootless-only deletes that block before installing);
`docs/ci-runner-deployment.md`'s "Two activation levels" and "Installing the service" sections
describe the landed contract instead of a still-pending one.

**Verification:** `cargo build -p myelin-ci-controlplane --all-targets`; `cargo clippy
-p myelin-ci-controlplane -p myelin-ci-sandbox --all-targets -- -D warnings`; `cargo build
--workspace`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo run -p myelin-lints
--bin lint-gate` (771 files, 0 violations); `cargo test -p myelin-ci-controlplane --lib` (455
passed); `cargo test -p myelin-ci-controlplane --test production_pg_bootstrap_source` (5 passed,
extended with two new ordering assertions proving activation is parsed/preflighted before bootstrap
and that the preflighted config, not a fresh env read, reaches `CiRunnerLoop::new`).

Aside, unrelated to this slice: the lib suite briefly showed one failure in
`production_gvisor_registry_tests` (a `DigestMismatch` on the pinned `linux-small-v1-rootfs`).
Root-caused (via `git stash` + a re-hash excluding the offending path) as an empty `workspace/`
directory left inside the canonical staged rootfs at `~/.local/share/gvisor-assets/rootfs` by an
earlier real `runsc` run against the live default path during this session's own slice-3
host-provisioning verification — not a code issue, not part of this commit. Removed.

**Now open:** CT-007's pre-registered cutover floor (ledger entry near the top of this file) still
needs steps 3–4 — the complete mapped graph passing on one real Myelin commit through the actual
self-hosted instance, twice, with no GitHub execution and no manual green. That is real production
dogfood infrastructure and higher blast radius than anything in this slice; per the standing
instruction, it needs an explicit check-in before proceeding, not an autonomous continuation.

## 2026-07-27 — check-in received; re-scoped steps 3-4 against the honest gate-2 floor; design correction with Sol on the checkout mechanism

The founder gave explicit go-ahead to continue autonomously with Sol toward steps 3-4. Before
attempting them, re-grounded against `ci-workload-inventory.toml`/`runner-assets.toml`: steps 3-4
require ALL 12 GitHub jobs mapped and passing as one graph, and today only 1 of 12
(`build-test-clippy`) has any progress at all, still `capability-smoke` not `capability-proven`.
Steps 3-4 are not yet reachable; the honest next concrete unit is finishing `build-test-clippy`'s
own vertical slice (per the 2026-07-25 reviewer redirection: one job fully end-to-end before
Node/browser/Docker work for the other 11 rows starts).

**Design consultation with Sol on the remaining shape of that slice.** Sol's initial recommended
sequence (5a exact-commit materializer → 5b bind into `launch_with` → 6 cargo-vendor EROFS asset →
7 dependency mount + env/cwd propagation → 8 direct capability proof + resource sizing → 9 real
dispatch activation) was corrected on its own first slice once the concrete implementation
architecture was researched and put to Sol directly:

**Rejected: a host-side exact-commit Git-tree materializer.** The original slice-5a design
(walk the commit's tree via `myelin-git`/git2, write it fd-relative/no-follow into the prepared
Btrfs workspace on the HOST, entirely independent of gVisor) would require either promoting
`myelin-ci-sandbox`'s `myelin-git` dependency from dev-only to a real production edge, or adding a
new host-side Git-tree-walking API surface reachable from the CI runner. Sol's correction: this
silently replaces the codebase's existing, deliberate boundary — CI checkout is supposed to go
through the SCOPED JOB-TOKEN GIT WIRE (the same transport CT-006's git-wire work already hardened),
never through the runner opening Git's bare-repository storage directly. A host-side materializer
would let a malicious repository path/symlink get interpreted by host code before the sandbox ever
exists; the git-wire path confines that entirely to the guest mount namespace, where the host never
interprets repository paths at all.

**Corrected sequencing:**
- **Slice 5a (revised): fix the git-wire transport reliability prerequisite.** Task #33's
  pre-existing "runsc stdin pipe unavailable" flake is now a hard blocker, not a side issue — the
  checkout mechanism depends on the exact transport that flake lives in. Root-cause and fix it for
  real: prove bounded bidirectional git smart-protocol transport through the production git-wire
  path across empty input, early EOF, client disconnect, large pack data, timeout, cancellation,
  and checked teardown, with no leaked runsc container/cgroup on any path.
- **Slice 5b: a dedicated in-gVisor checkout preparation run.** Mounts only the prepared writable
  workspace; runs as the mapped non-root uid/gid; authenticates with the scoped job token without
  the bearer ever touching argv/env-logs/remote-url/`.git/config`; fetches the exact commit oid and
  checks it out detached; disables hooks/global config/credential persistence/smudge-clean
  filters/submodule recursion; rejects gitlinks before admitting the workspace; verifies `HEAD`
  equals the requested oid; hashes the materialized `Cargo.lock`; confirms checked teardown before
  the real workload's launch permit is acquired; follows the existing 7c cleanup-matrix rules
  (release/settle/quarantine) on any failure path.
- **Slice 6 (unchanged): the cargo-vendor EROFS asset** — keyed by `Cargo.lock` hash, built via
  `cargo vendor --locked --versioned-dirs` from a clean verified checkout, a deterministic
  (fixed-timestamp/UUID) `mkfs.erofs` image built twice and required byte-identical, promoted via
  the same atomic immutable-version-directory pattern as the rootfs assets, with drift caught at
  three boundaries (commit lint on `Cargo.lock` hash, startup verification of the staged EROFS
  bytes/mount identity, and a per-launch re-hash of the materialized `Cargo.lock` before permit
  commit).
- **Slice 7 (unchanged): dependency mount + real env/cwd propagation.** A fixed read-only
  `/opt/myelin/cargo-vendor` mount (`ro,nosuid,nodev`); a trusted, runtime-constructed Cargo
  home/config forcing offline mode (never repository-authored); `CARGO_TARGET_DIR=/workspace/target`;
  `OciConfig::from_spec` finally consuming validated `spec.env` (currently ignored); `process.cwd`
  derived as `/workspace` for checkout-bearing jobs, `/` for compute jobs (never caller-selectable);
  reserved runtime-owned variable names (`PATH`/`HOME`/`CARGO_HOME`/`CARGO_TARGET_DIR`); the real
  GitHub job's env propagated (`CARGO_TERM_COLOR`/`CARGO_INCREMENTAL`/`RUSTFLAGS`/`RUSTDOCFLAGS`).
  Noted as larger than `gvisor.rs` alone: the authored-job/resolved-plan/launch-authority schema
  layers don't carry env values today even though `JobSpec` does — those need to carry requested env
  through to server-authority validation, not let authored TOML bypass it.
- **Slice 8 (unchanged): direct capability proof + resource sizing.** Run the real three-command
  workload (`cargo build --workspace --locked && cargo test --workspace --locked && cargo clippy
  --workspace --all-targets -D warnings`) against a clean checkout + verified vendor asset + offline
  Cargo, first with generous limits while measuring peak memory/disk/pids/CPU/wall time, then rerun
  from a clean workspace at final documented limits with headroom. Also resolves a real toolchain-
  parity gap found during this design pass: GitHub's job requests floating `stable` while
  `linux-rust-v1` is pinned to Rust 1.82 — both sides need pinning to the same explicit version
  before this can honestly count as proof.
- **Slice 9 (unchanged): real dispatch activation.** A committed `.myelin/ci.toml` job naming the
  digest-pinned asset and the exact three-command workload, with env/dependency-asset/source-commit/
  measured-limits flowing through the REAL path (`PipelineStarter` → resolved plan → runtime
  authority → durable manifest/spec → runner poll loop → the `Enabled` `GvisorBackend`), a real exact
  commit dispatched through it, and the real check/log projection reporting the result. A direct
  `GvisorBackend` test call (slice 8) is necessary but insufficient for `capability-proven` — this is
  the honest minimum, and gate 3 (all 12 jobs as one graph) does not own this missing per-job
  dispatch machinery.

**Immediate next unit of work:** slice 5a as revised — root-cause and fix task #33 (the git-wire
"runsc stdin pipe unavailable" flake), since slice 5b's entire checkout mechanism depends on that
exact transport. Not yet started as of this entry.

## 2026-07-27 — task #33 root-caused and fixed: the git-wire "runsc stdin pipe unavailable" failure was a real, dated regression, not a flake (Sol, 2 review rounds)

Slice 5a as revised (previous entry): task #33 is closed. Not a flake — a real, always-reproducible
bug in the launch-gate seam, silent until a specific prior commit exposed it.

**Root cause.** `SandboxCommand::spawn()` (`launch_gate.rs`)'s fenced branch always `child.stdin.
take()`s the child's stdin pipe to write the gate-release byte, then unconditionally `drop(gate)`
right after — permanently closing that pipe's write end inside `spawn()` itself, so `SandboxChild::
stdin()` returned `None` for every fenced launch, full stop. Commit `ceb0d297a` ("R4.2: fence
durable sandbox launch", 2026-07-23) introduced this; it was harmless as long as the only fenced
launches were CI/agent jobs (which always pass `stdin: None`). Commit `30854984` ("CT-007: fix the
pre-existing ContainerRun reserve/settle leak", 2026-07-26 — this same track's own prior work)
threaded a real launch permit through git-wire for the first time, making ITS launches fenced too —
and git-wire is exactly the caller that needs to pipe the stateless-rpc request body into the
guest's stdin AFTER the gate releases. Reproduced live before any fix: 3/4 tests in
`git_wire_prod_exec_test.rs` failed with exactly "runsc stdin pipe unavailable"; confirmed via
`git stash` that the base commit fails identically (a real, dated regression, not environmental).

**Fix, designed with Sol before writing code.** A new `PostGateStdin` enum (`Close` default /
`ReturnToCaller`) on `SandboxCommand`, set via `return_stdin_to_caller_after_gate()` (asserts
fenced). `spawn()`'s fenced branch now defers closing/restoring the pipe until AFTER
`ownership.release()` succeeds — restoring it any earlier would leave an open writer inside `child`
on a release-failure path, where a failed best-effort `kill_process_group` could then let
`child.wait()` hang behind a guest still waiting for EOF on a pipe this function itself kept open.
Every earlier failure branch (readiness-wait, `permit.commit()`, `ownership.validate()`, the
gate-write itself, and the post-gate release failure) now explicitly `drop(gate)` before its
kill/wait call, as an independent EOF backstop rather than relying solely on the process-group
signal. `gvisor.rs` calls the new setter exactly when `fenced && stdin.is_some()` — the same
condition it already computes today.

**Review process:** 2 rounds with Sol. Round 1 confirmed the design and the fix mechanically, but
found 2 real defects: the rustdoc referenced a nonexistent `LaunchPermitOwnership::release` type
(fixed to plain text); and the required test list was short one case —
`ownership.validate()` failure in retained mode (added, mirroring the existing
`lost_post_commit_ownership_kills_guard_before_runtime_execution` test, asserting
`SpawnPhase::CommittedButNotExecuted`, no exposed stdin, and a prompt return). Also took 2
non-blocking recommendations: reworded a comment ("today's only behavior" → "the default/legacy
behavior"), and hardened the new watchdog test against scheduler flakiness (100ms → 750ms deadline,
plus an explicit poll-for-the-guest-actually-started step before relying on the watchdog). Round 2:
**cleared to commit**, with two acknowledged non-blocking nits (a stale "test above" cross-reference
that should say "below" — fixed directly; and a noted, accepted timing looseness in the watchdog
test that Sol judged acceptable given the repo's existing timing-test precedent).

**Tests added (10, all in `launch_gate.rs`'s existing `mod tests`, file total 7→17):** default
Close still returns no stdin + immediate EOF; retained mode returns exactly one live pipe (and
taking it twice yields `None` the second time); byte-exact payload round-trip for embedded
newlines, a payload whose own leading bytes spell `launch\n` (proving the shell gate's `read -r`
consumed only its OWN release line), NUL bytes, and an empty payload; failed commit in retained
mode never exposes stdin; failed `ownership.validate()` in retained mode never exposes stdin;
failed post-gate `ownership.release()` in retained mode drops stdin and returns promptly (elapsed
< 2s asserted); a watchdog kills a runtime genuinely blocked reading a deliberately-never-closed
retained pipe.

**Verified:** all 4 `git_wire_prod_exec_test.rs` tests pass (were 3/4 failing before this fix,
identically reproduced via `git stash` against the pre-fix tree); all 17 `launch_gate.rs` unit
tests pass; `cargo clippy -p myelin-ci-sandbox --all-targets` clean on both feature-flag
combinations; `cargo build --workspace --locked` clean; `cargo clippy --workspace --all-targets`
clean on both feature-flag combinations; `myelin-lints` `lint-gate` clean (771 files, 0
violations); `rustfmt --check` clean on both touched files; full `myelin-ci-sandbox`
`--all-targets --features integration` suite green (337 lib tests + every integration file) except
the SAME pre-existing, already-tracked `firecracker_production_launch_contains_the_corpus_non_root`
failure (reconfirmed via `git stash` against clean HEAD to fail identically — unrelated to this fix).

**Now open:** slice 5b — the dedicated in-gVisor checkout preparation run using this now-repaired
git-wire transport (task #60), per Sol's architecture correction recorded in the entry above.

## 2026-07-27 — slice 5b design locked in with Sol: two job-side runtimes, a new checkout-specific
## path, NOT a `GitWireSpec` extension

Consulted Sol on how the checkout preparation run composes with what already exists before writing
any code. Two real architectural questions came up during research, both resolved:

**Two `runsc run` invocations per checkout-bearing job, not one.** A same-container "checkout then
exec workload" wrapper was explicitly rejected: it would let the workload's durable launch CAS
commit before checkout is proven, turn a checkout failure into an executed workload attempt, make
repository/token/transport authority hard to revoke before `exec`, and remove the host's ability to
independently verify preparation before workload code begins. The real lifecycle for a
checkout-bearing job: reserve aggregate resources + acquire workspace/userns identity → run a
dedicated checkout-preparation container (its own internal preparation gate/watchdog, mechanically
similar to today's immediate-permit gate — NOT the durable launch-permit CAS, since this run isn't
itself billable CI work) → check its exit status/exact-commit result/complete runtime quiescence →
only then acquire the REAL workload's `LaunchPermit` → run the workload in a second container →
checked-add `checkout usage + workload usage` into one final settlement → delete the workspace →
release/quarantine the identity. Checkout is NOT free/uncounted: it's attacker-influenced CPU/I/O/
wall-time/disk activity, so the SAME reservation must cover both phases, and a failure once
preparation has actually executed needs its own distinct preparation-attempt result — it must not
masquerade as either an `Uncommitted` zero-usage failure or a workload `Executed` result.

**Consequence found during this design pass: the current lease-bind model can't simply be reused
across two runtimes.** `LeaseBindState`'s `Allocated`/`Bound{container_id, runsc_root_identity,
cgroup_identity}` model durably binds ONE lease to ONE exact runtime identity — two sequential
containers can't honestly share it as-is. Slice 5b needs an extended sequential session lifecycle:
`Allocated → PreparationBound{prep identity} → Prepared{preparation quiescence proven} →
WorkloadBound{workload identity} → releasable (after workload quiescence + workspace deletion)`. The
subordinate uid/gid identity and the Btrfs workspace stay stable across both runs; the container/
cgroup identities do not, and must never overlap. Key dispositions: preparation teardown unproven →
quarantine both lease and workspace, never start the workload; preparation proven but a later
pre-workload failure → delete the workspace, then release from `Prepared`; workload bound → the
existing evidence-backed final-release matrix applies, now additionally conditioned on the recorded
successful preparation transition; the subordinate identity must never be released/reallocated
between the two runs while its chowned workspace still exists.

**Rejected: extending `GitWireSpec`/reusing `run_git_wire_container`, or bind-mounting `/repo` into
either runtime.** `GitWireSpec`/`GitWireMounts` are server-side smart-protocol concepts (`/repo` is
bare storage, stdin/stdout are bounded RPC transport, the command SERVES `upload-pack`/
`receive-pack`) with their own distinct authority/accounting lifecycle — conflating "serve a clone
to an external client" with "a CI job's own internal checkout" is the same class of concern-
conflation earlier reviews already caught (the `iam`/`identity` taxonomy bug; folding workspace
cleanup into `RuntimeTeardownError`). Bind-mounting `/repo` into the checkout helper (or the
workload) was separately rejected: it would leave bare-repo storage reachable inside the same
container as the workload, bypassing the scoped job-token boundary entirely. New, dedicated types
instead: `CheckoutPreparationSpec` / `CheckoutTransport` / `CheckoutPreparationOutcome` /
`PreparedCheckoutEvidence` — the last mintable only once the scoped transport resolved the requested
repository, checked-out `HEAD` equals the exact requested commit OID, the checkout destination is
the managed `/workspace`, the helper exited successfully, AND the helper runtime was fully finalized/
quiesced (also expected to carry the observed `Cargo.lock` digest, for slice 6's vendor-asset
composition). Reuse only the LOWER-level machinery: OCI mount serialization, explicit-userns mode
validation, cgroup creation/identity, `run_and_capture`, checked runtime finalization, bounded
output/error handling — never `run_git_wire_container` itself, never `GitWireMounts` as the checkout
mount contract. The existing `launch_git_wire` may still implement the SERVER side of individual
RPCs the checkout transport calls into, but it is not itself the checkout client.

**Agreed commit boundaries for slice 5b (not started as of this entry):**
- **5b.1 — sequential preparation session:** extend the durable lease/binding state machine above +
  introduce the aggregate preparation/workload usage result types. Exhaustively test every state
  transition and quarantine disposition WITHOUT running any real git command yet.
- **5b.2 — checkout runtime:** the checkout-specific spec, transport seam, OCI shape, preparation
  gate, exact-OID verification, and checked finalization, plus a live helper drill. No workload
  activation wired in yet.
- **5b.3 — atomic composition:** integrate both phases into `launch_with`'s real sequence (reserve →
  prepare → verify/quiesce → acquire workload permit → workload → aggregate settle/release),
  including the real Enabled-path drill.

**Immediate next unit of work:** 5b.1 — the lease/binding state-machine extension. Not yet started.

## 2026-07-27 — CT-007 slice 5b.1 landed: sequential preparation-session lease lifecycle (Sol, 4 review rounds)

Slice 5b.1 (previous entry) is complete: `crates/myelin-ci-sandbox/src/user_namespace.rs` now
supports a checkout-bearing job's identity visiting a preparation runtime BEFORE the real workload
runs, without touching the existing single-runtime `Allocated -> Bound` path used by every
non-checkout CI/agent job at all. Commit `495ee8b3`.

**Shape:** `Allocated -> PreparationBound -> Prepared -> Bound` (the last transition, via the new
`bind_workload`, produces the SAME `LeasePhaseV2::Bound` the ordinary `bind()` does, so every
existing `release()`/final-settlement call site takes over unchanged). New durable
`UserNamespaceLease` methods: `bind_preparation`, `confirm_prepared` (verifies a
`PreparationQuiescenceProof` — a type distinct from the existing `UserNamespaceQuiescenceProof`),
`release_prepared` (the `Prepared`-phase counterpart to `release_unused`), `bind_workload`. A new
`CheckoutPreparationSession` capability wrapper (in-memory, `pub(crate)`-only) enforces correct
transition order and binds itself to the SPECIFIC lease it started with, panicking on any
out-of-order call or cross-lease substitution — defense in depth on top of the durable marker's
own checks, which remain independently sufficient on their own.

**A real schema-versioning gap found and fixed mid-review (Sol's round-1 review, point 4):** the
first draft added the two new phases directly into the existing `schema_version: 1` marker shape.
Sol caught that this breaks the codebase's own stated schema-versioning invariant (`schema_version`
must fully determine the marker's shape) — a rollback to a pre-5b.1 binary encountering a
`schema_version: 1` marker with a `PreparationBound`/`Prepared` phase its own 2-variant enum can't
represent would misdiagnose "corrupt marker" instead of an honest "newer/unrecognized schema."
Fixed by freezing the original 2-variant shape as `LeaseMarkerV1`/`LeasePhaseV1`
(`schema_version` 1, read-only, boot-reconciliation-only) and introducing the 4-variant shape as
`LeaseMarkerV2`/`LeasePhaseV2` (`schema_version` 2) — `lease()` (the one marker-minting call) always
writes V2 going forward; every other lease method only ever reads/rewrites V2, since every ACTIVE
lease this process itself issues is always V2. Boot reconciliation gained a matching second
schema-dispatch arm. A dedicated test plants a real legacy V1 marker and proves it's still
recognized/quarantined correctly, and that a freshly minted marker is always V2.

**Three more real design gaps caught across the review, each fixed:**
- **Session not bound to one lease (round 1):** the original session tracked only a phase, so a
  session prepared with lease A could confirm/release/bind-workload using an unrelated lease B.
  Fixed by carrying the lease's own nonce in `PreparationBound`/`Prepared` state and asserting it
  against every passed lease; 2 `#[should_panic]` cross-lease-substitution tests added.
- **`bind_workload` consumed the session on a retryable refusal (round 1):** the original by-value
  signature destroyed the only preparation capability even on `InvalidContainerId`/`MarkerTooLarge`
  — caller-fixable failures that leave the lease genuinely untouched. Fixed with `&mut self`; a
  retry-survival test proves a corrected retry still reaches a real `release()`.
- **`bind_workload`'s success handoff needed to be a real capability, not `Ok(())` (round 3):**
  fixed with `WorkloadBindingIdentity` — private fields, no `Clone`, obtainable ONLY from a genuine
  successful `bind_workload`, consumable ONLY via `into_parts()` — so the 5b.3 caller constructs
  `LeaseBindState::Bound` from the exact durably-committed triple, never a separately-remembered
  copy that could silently diverge.

**Also fixed:** the session's `confirm_prepared` deliberately abandons (terminal `Unreleasable`) on
ANY failure, including an ordinary wrong/mismatched proof — stricter than the raw
`UserNamespaceLease::confirm_prepared` it wraps (which still leaves the marker genuinely untouched
on a wrong proof, matching `release()`'s own established retry-tolerant precedent), since a real
caller has exactly one proof-minting opportunity per real preparation run; a dedicated test proves
the full disposition (allocator stays healthy, session becomes `Unreleasable`, a later correct
proof cannot advance it, dropping the lease quarantines exactly its own slot). `confirm_prepared`
returns a new dedicated `PreparationConfirmationError` rather than reusing
`UserNamespaceReleaseError` (whose `ProofMismatch` doc specifically claims the lease is consumed —
true for `release()`'s by-value signature, false for `confirm_prepared`'s `&mut self`).
`UserNamespaceBindError`'s doc/Display generalized to cover all three binding callers (`bind`,
`bind_preparation`, `bind_workload`) instead of hardcoding "Allocated -> Bound" language that was
already false for two of the three. Deterministic failure coverage added for every duplicated
durable operation: the three rewrite paths via a pre-planted `<marker>.tmp` colliding with
`rewrite_marker_atomically`'s own `O_EXCL` create; `release_prepared`'s unlink ambiguity via a new
`release_prepared_given` seam (an injectable unlink operation) rather than a chmod-based `EACCES`,
which Sol flagged as non-deterministic under `CAP_DAC_OVERRIDE`. New types/methods are `pub(crate)`,
not `pub`, so no external consumer of this crate can bypass `CheckoutPreparationSession`'s ordering.

**Review process:** 4 rounds with Sol (gpt-5.6-sol), each finding genuine, concrete defects — not
style preference. Round 1: the schema-version gap, session cross-lease binding, `bind_workload`
retry-destruction, plus required deterministic-failure/legacy-marker/cross-lease test coverage.
Round 2: the missing `WorkloadBindingIdentity` capability and the missing terminal-confirmation
test, plus wording/visibility cleanups. Round 3: `WorkloadBindingIdentity` still forgeable via
public `Clone`+fields; `UserNamespaceBindError`'s docs still named only `bind`/`bind_workload`, not
`bind_preparation`. Round 4: cleared.

**Verified, final round:** `cargo clippy -p myelin-ci-sandbox --all-targets` clean on all three
feature combinations (none/`integration`/`integration,test-support`); `cargo build --workspace
--locked` clean; `cargo clippy --workspace --all-targets` clean on both feature combinations;
`myelin-lints` `lint-gate` clean (771 files, 0 violations); `rustfmt --check` clean; the module's
own test suite: 62 → 84 tests (28 new), all passing; full `myelin-ci-sandbox` lib suite unaffected
elsewhere.

**Still open:** 5b.2 (the checkout-specific runtime: `CheckoutPreparationSpec`/`CheckoutTransport`/
`CheckoutPreparationOutcome`/`PreparedCheckoutEvidence`, OCI shape, exact-commit-OID verification,
checked finalization, a live helper drill — no workload activation yet) and 5b.3 (atomic composition
into `launch_with`'s real sequence, including the real Enabled-path drill). Neither started. This
slice's own `CheckoutPreparationSession`/`WorkloadBindingIdentity`/durable lease machinery is fully
built and tested but not yet wired into `GvisorBackend`/`launch_with` — same deliberate deferral
discipline as every earlier slice's own "not yet consumed" scaffolding.

## 2026-07-27 — CT-007 slice 5b.2 landed: the checkout-specific runtime (Sol, 5 review rounds)

Slice 5b.2 (previous entry's "still open") is closed: `fetch_checkout_pack` (Hop A) +
`run_checkout_preparation` (Hop B) — a full, real checkout-preparation transport and sandboxed
execution path, built on 5b.1's sequential lease lifecycle. Design was locked in with Sol across 2
rounds before any code was written (a two-hop, host-glued sequence — Hop A finishes fully before Hop
B starts, never a live inter-container pipe, never `/repo` mounted into either runtime), then
implemented and adversarially reviewed across 5 more rounds before commit.

**What Hop A is.** `fetch_checkout_pack`: a REAL, billed use of the EXISTING, unchanged
`GvisorBackend::launch_git_wire` — advertise-refs, then a single-shot `want <oid> ... / deepen 1 /
done` fetch, both under a dedicated per-invocation `-c uploadpack.allowReachableSHA1InWant=true`
(never a general-serving-path reconfiguration; CI/merge-queue dispatch commonly targets a commit
that is reachable but no longer an exact advertised tip by the time a queued attempt starts, and this
codebase has no per-attempt ref to pin it with today — Sol's round-2 finding, resolved as a CHECKED
precondition: parse the advertisement, proceed only if the oid is a direct tip OR the server actually
offers the capability, never silently assume the `-c` was honored). A new hand-written pkt-line
reader (`read_pkt_line`) implements the EXACT grammar for this one specialized request (never the
general git-protocol grammar, which allows far more): `0000`/`0001`-`0003` reserved/`>0xfff0`
refused; the fetch response's mandatory shallow-info section (always present because `deepen 1` was
requested, even for a root commit) ends in exactly one flush, then exactly one `NAK` (no ACK — no
`have` lines were ever sent), then the raw pack read directly to EOF, never scanned for. The
advertisement parser requires the first line's capability section to be present (its absence is a
malformed/spoofed response, not a capability-less repo) and REQUIRES `shallow`/`no-progress`/
`ofs-delta` actually be advertised (this transport always relies on them), refusing on any missing
capability or trailing bytes after the terminating flush. `PrefetchedCheckoutPack` carries the fetched
pack as a file (never a second in-memory `Vec` — the git-wire capture path already materializes one).

**What Hop B is.** `run_checkout_preparation`: a NEW, dedicated checkout-preparation container —
`ExplicitUserNamespace` + the workspace mount (the lease's `PreparationBound` identity), no `/repo`,
no network — NOT billable on its own (no `reserve`/`settle` call of its own; its measured usage is
carried in every post-spawn `CheckoutPreparationError` variant for 5b.3's aggregate settlement to
fold in, never silently free). Uses an internally-minted `LaunchPermit::immediate()` (never `None`,
which would skip the mechanical launch-gate/watchdog entirely, not merely the durable CAS — Sol's
round-1 finding). The checkout script (object-format-aware `git init`, `core.hooksPath=/dev/null` +
empty `--template=`, seeded `.git/shallow`, `index-pack --stdin --strict` with no `--fix-thin`/
thin-pack, `test "$(...)" = "..."` comparisons never bare exit codes, detached checkout, `diff-index
--quiet` against the wanted tree) runs, and its confirmation line + exit status are checked — but
NOT before the runtime's teardown is independently proven and `session.confirm_prepared` has already
run REGARDLESS of the checkout's own outcome (an ordinary corrupt pack or wrong tree must never force
permanent quarantine of a lease whose runtime genuinely tore down cleanly — the load-bearing ordering
property from Sol's round-1 design review, extracted into a standalone `evaluate_checkout_finalization`
specifically so it is unit-testable against synthetic `RuntimeFinalization`/`RuntimeQuiescenceEvidence`
values against a REAL, non-privileged `UserNamespaceAllocator` lease, no `runsc` spawn needed).
`CheckoutPreparationError` has four dispositions (`Refused` — free, nothing spawned;
`Unreleasable`/`TeardownUnproven`/`RejectedAfterQuiescence` — every one carries real measured usage).

**The locked ledger-12 contract (rejecting gitlinks, hashing `Cargo.lock`) — implemented.** The
checkout script now runs `git ls-tree -r "$oid"` right after confirming the object exists (before
checkout) and refuses if any gitlink (mode `160000`) entry is present — this transport never fetches
submodule repos, so a superproject with unpopulated submodules would silently build wrong. A new
`hash_workspace_cargo_lock_no_follow` re-reads `Cargo.lock` host-side (FD-safe, `openat(O_NOFOLLOW)`
relative to the workspace, never a guest-reported hash — the same reasoning that makes `HEAD` itself
host-re-verified: this digest becomes slice 6's cache/asset key, so it must be independently
authoritative), SHA-256 via the `sha2` crate already used by `canonical_tar.rs` (no new dependency),
bounded at 16 MiB, refuses absence/symlink/non-regular-file. `PreparedCheckoutEvidence` now carries
`cargo_lock_sha256_hex`.

**Host-side FD-safe re-verification, both HEAD and Cargo.lock.** Neither the guest's own
`rev-parse`/`diff-index` claims nor a guest-reported hash are trusted alone. `verify_workspace_head_
no_follow` walks `<workspace>/.git` via `openat(O_NOFOLLOW)` at every component and requires
`.git/HEAD` to be a bounded real regular file containing exactly `<oid>\n`. A real defect found here
(Sol's round-3 review): the file-open itself used plain `O_RDONLY`, which BLOCKS FOREVER on a
guest-planted FIFO before the "is it a regular file" check is ever reached — fixed with `O_NONBLOCK`
(harmless for a real regular file's reads) plus a bounded-READ (not merely a preceding
`metadata().len()` stat-then-act gap) via `.take(129).read_to_end(...)`. A dedicated test proves the
fix by running the check on a background thread with a bounded `recv_timeout`, so a regression fails
loudly instead of hanging the whole suite.

**Review process: 5 rounds with Sol, each finding genuine, concrete defects.** Round 1 (6 blockers):
Hop A leaked both `SandboxLaunch` handles on every path and never checked `passed()` before trusting
stdout; the claimed bind-boundary identity revalidation didn't actually happen (the STALE early-read
value was durably bound, never a fresh live one — mirrors `bind_enabled_lease_given`'s two-check
pattern, now actually implemented); the FIFO-blocking HEAD-reader bug above; bypassing `JobSpec` also
bypassed its mandatory `pids_max`/`timeout_secs` validation (fixed via a new shared
`validate_execution_limits`, `CheckoutPreparationSpec::new` now fallible); both new/pre-existing
temp-file staging points used a plain, symlink-following, umask-dependent `File::create` rather than
`create_new`+explicit `0600` (fixed in both `tempfile_for_checkout_pack` AND the pre-existing
`drain_to_temp_file`, which now also carries 5b.2's private packs); the advertisement parser didn't
enforce the capabilities its own request relies on. Round 2 (4 more): Hop A's cleanup still leaked on
an early-return before `backend.kill` was ever reached; a successful checkout evaluation ignored
`RunscOutcome::stdout_truncated`/`stream_error`; the gitlink/Cargo.lock requirement above was
entirely missing (a real, previously-recorded ledger requirement this session's own design pass had
overlooked); the live drill was a hard commit requirement, not something 5b.1's "not yet a live
consumer" precedent could excuse (Sol: 5b.1 was predominantly a deterministic state machine; 5b.2 is
a real interoperability composition across Git protocol, OCI, `runsc`, cgroups, user namespaces,
Btrfs, and durable lease transitions — a materially different risk profile). Round 3 (3 more, after
the live drill was built and genuinely exercised its own skip path): the live drill released the
lease BEFORE deleting its workspace (violates the central identity invariant — fixed:
`workspace_manager.delete_workspace` now required to succeed first); the two live drills (this one +
the pre-existing Enabled activation drill) share the SAME operator-provisioned `leases_dir` and could
race under `cargo test`'s default parallelism (fixed: a shared, poison-tolerant
`USERNS_DRILL_LEASES_DIR_LOCK` both acquire first); a parse error's `?` could propagate before a
simultaneous `kill` error was even inspected, silently hiding a real runtime leak (fixed: combined
into one `match` over all four outcome combinations); the gitlink-detection pipeline treated a
FAILING `git ls-tree` the same as "no gitlinks found," because a bare pipe's exit status is its LAST
command's (`grep`'s) — fixed with an explicit `command_substitution || { exit 1; }` for `ls-tree`
and an `if/else`-captured (hence `set -e`-exempt) grep exit status distinguishing found/not-found/
hard-failure, plus 3 new tests running the identical snippet against real host `git`+`sh` (no
`runsc` needed) proving the clean-commit, gitlink-present, AND ls-tree-failure dispositions. Round 4:
**cleared to commit**, with one non-blocking note (a test-count miscount in my own summary, not a
code defect).

**The live drill.** `checkout_preparation_runs_end_to_end_through_real_git_wire_and_runsc`,
`#[cfg(feature = "integration")]`, lives in `gvisor.rs`'s own `mod tests` (not an external
`tests/*.rs` file — `run_checkout_preparation`/`fetch_checkout_pack`/`CheckoutPreparationSpec`/
`CheckoutPreparationSession` are all `pub(crate)`, unreachable from outside the crate). Stages a
real git-bearing rootfs from the busybox base (adapted from `tests/git_wire_prod_exec_test.rs`'s own
recipe — that file's staging can't be reused directly since it runs in a separate test-binary
process), builds a REAL bare repo with TWO commits + a committed `Cargo.lock`, requests the OLDER
(non-tip) commit specifically to exercise `allow-reachable-sha1-in-want` + the shallow boundary (Sol's
suggestion), runs Hop A against a real `GvisorBackend`, acquires a real workspace+lease via
`acquire_enabled_workspace` (the SAME function `launch_with`'s Enabled path uses) against a real
`GvisorBackend::try_new(Enabled)`, runs Hop B, and asserts the returned evidence's commit hex + a
non-empty Cargo.lock digest before deleting the workspace and releasing the lease. Gating mirrors the
pre-existing Enabled activation drill exactly (`runsc` absent / `preflight_explicit_userns_policy`
failing / base rootfs absent / `MYELIN_USERNS_DRILL_LEASES_DIR` unset are the ONLY legitimate skips;
every other failure is a hard `.expect`, never caught-and-skipped). Verified it actually RUNS (not
merely compiles): on this dev host it correctly reaches and fails exactly the
`preflight_explicit_userns_policy` gate (`runsc`'s own ancestor chain, under this user's home
directory, is honestly refused by the strict own-euid-ownership check) — an honest, expected skip,
not a vacuous one. Real end-to-end execution (provisioning a hardened `newuidmap`/`newgidmap` helper
dir + `runsc` state root + an operator-installed `leases_dir`) remains explicitly tracked as
follow-up infrastructure work, separate from this commit, per Sol's own call.

**Verified, final round:** `cargo test -p myelin-ci-sandbox --lib --features "test-support
integration"` — 459 passed, 0 failed (67 new in `checkout_preparation_5b2`, on top of the pre-existing
392). `cargo clippy -p myelin-ci-sandbox --lib --all-targets` clean on both feature combinations.
`cargo build --workspace --locked` clean. `cargo check --workspace` clean.

**Still open:** 5b.3 (atomic composition into `launch_with`'s real sequence: the resource-reservation
choreography — reserve aggregate resources, acquire workspace/userns identity, run Hop A + Hop B in
sequence, acquire the real workload's own `LaunchPermit`, checked-add both usages into one final
settlement — none of which 5b.2 attempts; it is a pure sandboxed-execution unit given
already-acquired capabilities). Also open: task #20 (the EROFS cargo-vendor asset, now unblocked in
principle since `PreparedCheckoutEvidence` carries the `Cargo.lock` digest it needs to key off of,
but still gated behind 5b.3 landing a real materialized checkout in production). And the live-drill
infrastructure gap noted above (host provisioning for a genuine green execution, not just an honest
skip).

## 2026-07-27 — host provisioning attempt for the live drills: got further, found the exact
## remaining wall, declined to cross it

With sudo authorized for host setup, provisioned real infrastructure for the two live drills
(`explicit_user_namespace_boots_through_the_real_enabled_backend_and_launch` and 5b.2's new
`checkout_preparation_runs_end_to_end_through_real_git_wire_and_runsc`): copied the exact
pinned-digest `runsc` binary to `/usr/local/bin/runsc` (root-owned, satisfying
`harden_explicit_userns_runsc_binary`'s non-writable-ancestor-chain requirement — the prior
`~/.local/bin/runsc` install lived under the user's own home dir, which the strict policy correctly
refuses to anchor on); created `/opt/myelin/runsc-root` and `/opt/myelin/userns-leases` (euid-owned,
mode 0700, under the root-owned `/opt/myelin` — the exact contract `harden_explicit_userns_runsc_root`
and the strict `UserNamespaceAllocator::try_new` require). Both drills now clear EVERY gate they
previously skipped at (`preflight_explicit_userns_policy`, the staged git rootfs, the leases
directory) and reach real `runsc`/OCI/userns/workspace-acquisition code.

**The wall both drills now hit identically:** `WorkspaceManager`'s real Btrfs subvolume provisioning
calls `btrfs qgroup limit`, which fails `Operation not permitted` — this process lacks
`CAP_SYS_ADMIN` for quota operations. Confirmed this is NOT specific to the new checkout drill: the
PRE-EXISTING Enabled activation drill hits the byte-identical failure. It is also not a surprise or a
product defect — `workspace_storage.rs`'s own `full_privileged_lifecycle_create_quota_verify_exceed_
delete_sync` test already probes for exactly this and skips gracefully when absent; the two
higher-level drills simply don't have that same probe-and-skip for this ONE specific dimension (they
check `runsc`/policy/leases-dir presence, not Btrfs privilege).

**Why this wasn't pushed through:** granting the compiled TEST BINARY `cap_sys_admin` via `setcap`
(mirroring the documented `myelin-runner` systemd `AmbientCapabilities=CAP_SYS_ADMIN` pattern) did
NOT help — Linux file capabilities do not propagate across `exec` into a child process unless ambient
capabilities are explicitly raised beforehand, and `WorkspaceManager` invokes `btrfs` as a genuinely
separate child process. The capability would need to live on `/usr/bin/btrfs` itself to take effect —
but that grants CAP_SYS_ADMIN to EVERY invocation of `btrfs` by EVERY user on this shared dev host,
permanently, until explicitly removed — a materially larger and harder-to-reverse change than
anything done so far this session, and not something "use sudo as needed" was read to authorize
unilaterally. Declined; the test binary's capability was stripped back off immediately after
confirming it didn't help.

**Cleanup:** both drill runs left a real, `CAP_SYS_ADMIN`-requiring Btrfs subvolume+qgroup behind
(their own `WorkspaceManager` incident log says so explicitly — "manual reconciliation required, do
not retry silently"). All three leaked subvolumes (two from the checkout drill's probe, one from the
re-confirmation run against the pre-existing drill) were deleted via `sudo btrfs subvolume delete`;
their now-empty parent directories removed. The provisioned `/usr/local/bin/runsc`, `/opt/myelin/
runsc-root`, and `/opt/myelin/userns-leases` are left in place (harmless, reusable by a future session
that does grant `/usr/bin/btrfs` the capability, or that runs the drills inside the already-documented
`myelin-runner` systemd context instead). Two userns-lease slots are now permanently quarantined in
`/opt/myelin/userns-leases` from these probe runs — expected, harmless (the allocator simply issues
higher-numbered slots going forward), not touched further.

Full `myelin-ci-sandbox` suite reconfirmed green afterward (459 passed) with no lingering effects from
the provisioning/cleanup. **Now open, for whoever picks up real live-drill execution:** either grant
`/usr/bin/btrfs` `cap_sys_admin` system-wide (an explicit, standalone decision — not bundled into a
future feature commit), or run the drills inside the `myelin-runner` systemd service context that
already carries `AmbientCapabilities=CAP_SYS_ADMIN` per `scripts/install-ci-runner-host.sh`.

## 2026-07-27 — CT-007 slice 5b.3-1 landed, 5b.3-2a landed (Sol, 3 review rounds), task #75 flaky
## test root-caused and fixed

**5b.3-1 — `WorkspaceIntent`/`ValidatedCheckoutRequest` (`workspace_intent.rs`, new module,
committed `22fa82dd`):** derives a job's checkout intent from `JobSpec.workspace: WorkspaceSpec
{ repo_ref, commit }`, backend-independent (deliberately not in `gvisor.rs`). `(None, None)` →
`Compute`; `(Some, Some)` → `Checkout` (CI-only); mixed is refused. Validates `repo_ref` via
`myelin_refs::parse_scoped` (requiring `subsystem == "git"`, `type_ == "repo"`, no `sub`), and
`commit` via a new `ExpectedGitCommitId::parse_exact` (infers SHA-1/40-char vs SHA-256/64-char from
width). Named `ValidatedCheckoutRequest`, not `ResolvedCheckoutRequest` — Sol's correction: no
storage-path resolution or authorization has happened at this stage, only syntactic validation.
Sol's round-1 review caught 2 issues: keep `artifact_ref` as the typed `myelin_events::ArtifactRef`
rather than re-stringified (fixed), and a Cargo.toml DAG-position comment whose rewritten history was
inaccurate (fixed — the crate genuinely began with only the `myelin-tenancy` edge and gained further
individually-justified deps later, rather than the comment having been wrong from the start).

**5b.3-2a — sandbox-side `CheckoutAuthorizationScope`/`CheckoutAuthorizationProof` + hook shape
(committed `db065848`):** the pre-Hop-A checkout-authorization hook plumbing (the real control-plane
authority chain is 5b.3-2b/2c, not yet started). `CheckoutAuthorizationScope` is a narrow read-only
DTO (tenant/repo_ref/repo_id/commit) handed to a new `RunnerHooks.checkout_authorization` hook,
`None` by default on both existing constructors (every call site byte-unchanged).
`CheckoutAuthorizationProof` is an unforgeable, one-shot capability binding the exact scope AND the
`run_token.jti` it was checked against. Took 3 Sol review rounds to land:
- Round 2 found 3 blockers: `CheckoutAuthorizationScope`'s fields were publicly constructible
  (redundant tenant/repo_ref/repo_id and commit_hex/commit_format pairs could disagree) — fixed with
  private fields + a `pub(crate)` constructor + read-only accessors. `GitObjectFormat` was duplicated
  (one copy in `workspace_intent.rs`, a drifting mirror in `lib.rs`) — fixed to one definition,
  re-exported. `CheckoutAuthorizationProof::into_scope()` stripped the proof into a freely-clonable
  scope, defeating the one-shot guarantee — fixed by binding `run_token_jti` into the proof and
  replacing `into_scope()` with borrowing-only accessors, with 5b.3-3's Hop A required to consume the
  whole proof by value.
- Round 3 (a genuine, non-obvious Rust semantics catch) found that `CheckoutAuthorizationProof`,
  even with private fields, was STILL forgeable via a bare struct literal from `gvisor.rs` — because
  Rust's privacy rules make a private field visible to every DESCENDANT module of its defining
  module, and every module in this crate (including `gvisor.rs`) is a descendant of the crate root.
  Defining the proof at the crate root therefore gave `gvisor.rs` full construction rights despite
  the "private" fields. Fixed by moving the proof and `RunnerHooks::authorize_checkout`'s
  implementation into a brand-new private SIBLING module (`checkout_authorization.rs`, a child of
  the crate root like `gvisor.rs`, but not an ancestor of it) — `gvisor.rs` can name and consume the
  minted proof but can no longer construct or destructure one directly. Also fixed 3 rustdoc
  public-to-private intra-doc links this surfaced under `cargo doc -D warnings` (the crate has other,
  unrelated pre-existing rustdoc failures — confirmed untouched by this change).
- Final round: cleared with no remaining blocker. Sol's standing note for 5b.3-3: Hop A must accept
  `CheckoutAuthorizationProof` by value and validate/use its bound `run_token_jti`; no scope-only or
  borrowed-proof alternate entry point.

**Task #75 — flaky `user_namespace` "survives reopening and is quarantined" tests, root-caused and
fixed:** an intermittent `AlreadyLocked` failure (~1-in-4 to 1-in-6 full-suite `cargo test` runs,
pre-existing since 5b.1, not introduced this session). Sol's root-cause read: under `cargo test`'s
default parallelism, an unrelated concurrent test's `Command::spawn()` can transiently inherit
another test's directory-lock fd during the fork-to-exec window (before `O_CLOEXEC` takes effect at
`exec`); a same-test drop-then-immediately-reopen racing that window can spuriously observe the lock
as still held. Fixed with a bounded, test-only retry (up to 20 attempts, 5ms apart) on `AlreadyLocked`
in `UserNamespaceAllocator::try_new_for_tests` — the real, process-lifetime production constructor
(`try_new`) is untouched and still fails closed on the first `AlreadyLocked`, since in production that
error means a genuine second runner process, not a transient fork-window artifact. Verified with 8
full-suite runs + 25 targeted `user_namespace` runs, 0 failures (committed `12a78a72`).

**Still open:** 5b.3-2b (the real control-plane authority chain — v2 digest, claim-time cross-check
loading `ci_job_spec`, `CiJobAuthorizationContext.checkout` field, exact-repo capability grant
derivation) and 5b.3-2c (wiring the hook + failure disposition + blocker tests) are designed at a
detailed level with Sol but not yet implemented. 5b.3-3 through 5b.3-7 remain scoped only at a
task-description level. Task #20 (EROFS cargo-vendor asset) and the live-drill host-provisioning gap
(previous entry) are both still open, unchanged by this entry's work.

## 2026-07-27 — CT-007 slice 5b.3-2b landed: the durable checkout authority chain (Sol, design +
## 3 review rounds)

Closes the gap 5b.3-2a's hook shape was built to eventually enforce: nothing previously verified
that a job's launched `JobSpec.workspace` (repo/commit) matched what its durable claim was
originally authorized against. Committed `221e4535`.

**What landed:**
- `myelin_ci_sandbox::derive_checkout_authorization_scope(kind, &workspace)` — the one sanctioned
  facade an external crate may use to derive a checkout scope, so the control-plane's authority side
  and the sandbox's launch side can never silently diverge in how a workspace is parsed.
- `CiJobRuntimeAuthorityRequest.checkout: Option<CheckoutAuthorizationScope>`, derived at both
  construction sites (materialize time from `ci_run`, claim time from the manifest's per-job grant).
- Digest versioning: `token_authority_digest` (v1) stays byte-frozen, sharing nothing with the new
  logic; `token_authority_digest_v2` is a wholly separate encoder binding the checkout scope. Every
  newly minted handle is v2; `verifies()` still accepts a legacy v1 handle for a compute-only
  request (never for a checkout-bearing one — v1 never bound a checkout target).
- Claim-time cross-check: `LockedManifestCiJobTokenIssuer::mint` now loads the durable `ci_job_spec`
  row in the same locked tx and verifies its own identity (`ci_run_id`, `token_authority_handle`,
  `idem_token`) plus workspace equality against the locked claim/run/manifest — the piece that
  actually closes the launch-time substitution gap.

**Sol's review, 3 rounds:** Round 1 found the durable launch template's own identity fields
(`ci_run_id`, `token_authority_handle`) were being silently discarded (only `.spec` reached
verification), self-referential v1/v2 "frozen encoding" tests (computed and compared via the same
function), 3 integration test fixtures that wouldn't compile under `--features integration`, and
several fixtures using abbreviated non-hex commit values that the new validation correctly rejects.
Round 2 found the cross-tenant check's rationale was factually wrong — `CiDriveManifestV1::validate`
already enforces `repo_ref`'s embedded tenant equals `manifest.tenant_id` via
`validate_canonical_ref`, making the new check genuine defense-in-depth, not an independently
reachable gap (doc corrected; a real full-chain test added showing `validate()` itself is what
catches it) — and asked for direct `idem_token` divergence test coverage (added). Final round:
cleared with no remaining blocker.

**Still open (at the time of that entry):** 5b.3-2c (dynamic `repo:<id>#pull` capability
derivation/minting in `ci_identity_adapter.rs`, plus real `CheckoutAuthorizationHook` wiring) was
designed with Sol but not yet implemented. See the next entry — it has since landed.

## 2026-07-27 — CT-007 slice 5b.3-2c landed: dynamic exact-repo capability + real
## checkout-authorization hook (Sol, design + 2 review rounds) — 5b.3-2 (the full checkout
## authorization proof, umbrella task #66) is now COMPLETE

Closes the last open piece of the checkout authorization chain: the exact-repo capability grant is
now actually derived and minted (not just structurally checked), and
`RunnerHooks.checkout_authorization` is wired to a real implementation in production instead of
staying unconfigured. Committed `7e436775`.

**What landed:**
- `CiJobAuthorizationContext.checkout_scope: Option<CheckoutAuthorizationScope>` — the dynamic
  `repo:<ref>#pull` capability alone proves repo-read authority but not the exact commit; this field
  binds the exact commit separately.
- `required_ci_capabilities(checkout)` — the one shared helper for minting, context construction,
  and verification, so all three can never disagree. `repo:<full ArtifactRef>#pull`, never a bare
  `repo_id`. Compute jobs keep exactly the original two capabilities.
- `mint_claim_credential` now actually grants the capability at mint time (threaded from
  `authority.checkout`), not just a context checked against a mint that never had it.
- `IdentityCiJobLaunchAuthorizer::verify_ci_job_signed` — the shared, read-only verification core
  both `authorize_retained` (the real workload launch boundary) and the new `authorize_checkout`
  (the pre-Hop-A hook) call. A genuine re-verification each time: job kind, context shape, the exact
  capability vector, that the checkout scope re-derived from the in-hand `JobSpec.workspace` agrees
  exactly with the server-resolved authorization context, the cryptographic bearer, and that the
  credential's expiry never outlives the durable claim.
- `CiJobLaunchClaimGate::verify_live` + a new `CiJobQueueStore::verify_launch_live` query — the
  read-only sibling of the real launch CAS (same generation predicate, including the `ci_job`
  surface-state gate), never mutating state or holding an advisory lock.
- `ci_runner_composition::ci_runner_hooks` now configures `.with_checkout_authorization(...)`,
  backed by the same `Arc<IdentityCiJobLaunchAuthorizer>` the launch fence already uses.

**Sol's review, 2 rounds:** Round 1 found the read-only durable-claim query omitted the `ci_job`
surface gate (letting the checkout hook proceed for a job the real launch CAS would refuse),
substitution tests that didn't exercise the actual production attack shapes (commit substitution
must fail at the context-scope comparison specifically; repository substitution must fail at signed-
token verification specifically, since structural comparisons alone would pass), and asked for a
live-PostgreSQL proof of the new query's production semantics — extended the existing
`integration_ci_drive_manifest_store` test (not a new one) to call `authorize_checkout` against real
durable state at 4 points, run repeatably against the dev PostgreSQL instance. Round 2: cleared after
two wording corrections (the context is server-resolved but not itself signed; only the bearer is
signed — and precisely scoping what the live test does vs. does not yet prove about `RunnerHooks`
invocation).

**Still open:** 5b.3-3 through 5b.3-7 (refactor `fetch_checkout_pack` into a parent-attempt
transport; durable pre-workload usage; honest terminal dispositions; full `launch_with` composition;
the live drill through the public launch path) remain scoped only at a task-description level — none
of the checkout-authorization plumbing landed in 5b.3-2a/2b/2c is actually INVOKED by a real launch
yet (the hook is configured but nothing calls it until 5b.3-3+ wires Hop A through it). Task #20 and
the live-drill host-provisioning gap (two entries up) are unchanged.

## 2026-07-29 — CT-007 slice 5b.3-3 landed: parent-attempt Hop A git-wire transport (Sol, design +
## 4 review rounds)

`fetch_checkout_pack` (Hop A) is now refactored into a parent-attempt-native transport that can run
its two nested git-wire executions (advertise-refs, fetch) entirely within an outer attempt's own
reserve/settle cycle, instead of opening its own independent one — the structural double-reserve
hazard ledger 12 flagged when 5b.3-3 was scoped. NOT yet wired into `launch_with` (that's 5b.3-6,
deliberately deferred per the design) — this slice lands only the transport itself and its test
coverage.

**What landed:**
- `build_git_wire_job`/`build_git_wire_oci_config` — the standalone billed path
  (`launch_git_command`) and the new parent-attempt transport now share the SAME hooks-free
  job/config construction; `launch_git_command`'s own hooks-dependent checks stay interleaved in the
  exact same relative order as before (confirmed behavior-preserving — all 440 pre-existing tests
  unchanged throughout).
- `run_git_wire_container_raw` — the pre-settlement half of `run_git_wire_container`, returning the
  structured `RuntimeFinalization` (or, for genuine pre-finalize failures, a bare `RunFailure`)
  instead of the standalone path's already-collapsed `Result`. Paired with a `BundleCleanupProof`
  (`Result<(), String>`) carried OUTSIDE the `RunFailure`/`RuntimeTeardownError` machinery (both are
  shared with every other `finalize_and_merge` caller in the file — widening either was out of
  scope) — `Err` whenever this function's own best-effort bundle-dir cleanup couldn't be verified.
- `fetch_checkout_pack_within_parent_attempt` — takes no `RunnerHooks`/`MeterTarget`/`IdemToken`/
  workload `LaunchPermit`; consumes the one-shot `CheckoutAuthorizationProof` by value and verifies
  its scope + run-token generation against the request BEFORE spawning anything; drives both hops
  with `LaunchPermit::immediate()`; fully retires each child+bundle itself.
- `CheckoutTransportError` — `Refused` (nothing spawned) / `Failed` (usage safely settleable) /
  `TeardownUnproven` (teardown or cleanup not independently proven) / `UsageUnrepresentable`
  (checked-add overflow; carries `teardown_unproven: bool` as an orthogonal fact, never collapsed
  into one variant choice).
- Fixed a real pre-existing leak found along the way: `stage_config_only_bundle` (shared by git-wire
  and Hop B's `run_checkout_preparation`) could leave its just-created directory behind if writing
  `config.json` failed — now best-effort cleaned up, reported via `StageBundleError::leaked`.

**Sol's review, 4 rounds — every round found real, substantive issues, none rubber-stamped:**
- Round 1: a non-passing/timed-out guest execution with syntactically valid output could be accepted
  as success (no `passed()` check); the executor seam collapsed genuine teardown-unproven outcomes
  into ordinary `RunFailure` before the parent-attempt code ever saw them; the overflow disposition
  silently discarded a simultaneous retirement failure and mislabeled the result; `usage_before ==
  zero` was used as a "no prior hop ran" marker, conflating a numeric fact with a phase fact (a
  completed hop can genuinely measure zero usage).
- Round 2: pre-finalization bundle-cleanup failures (cgroup-creation-failure arm, and the
  `run_and_capture`-returned-`Err` arm that still flows through `finalize_and_merge`) were still
  silently discarded, so a first-hop failure could map to the free `Refused` even when a bundle
  leaked; usage-representability and teardown-proof were treated as one choice instead of orthogonal
  facts; a finalization failure's message reported only the teardown problem, dropping a
  simultaneous non-passing/truncated/stream-errored guest result.
- Round 3: `CommitOutcomeUnknown` (an already-unreachable case under an immediate permit) still
  erased a real, independent teardown failure by returning ordinary `Failed`; `BundleCleanupProof`
  was enforced only on the error side of the result, leaving a type-level (never currently
  production-reachable) gap where an `Ok` finalization paired with a failed cleanup proof would
  silently drop the cleanup fact.
- Round 4: cleared — both fixes confirmed correct, diff independently verified against the worktree.

**Test coverage:** grew from the original 13 to 21 tests in the new `checkout_transport_5b3_3`
module (deterministic executor-injection throughout — real runsc/git-rootfs integration is 5b.3-7).
Full sweep clean each round: `cargo test -p myelin-ci-sandbox --lib` (461 passed, 0 failed),
`cargo clippy` `-D warnings` on both default and `--features integration`, `cargo check --workspace
--tests`.

**Still open:** 5b.3-4 through 5b.3-7 (durable pre-workload usage across the launch-CAS ambiguity
window; honest terminal dispositions for preparation-only outcomes; composing the full sequence into
`launch_with`; the live drill through the public launch path) remain scoped only at a
task-description level. The parent-attempt transport landed here is not yet called from anywhere —
5b.3-6 wires it in. Task #20 and the live-drill host-provisioning gap are unchanged.

## 2026-07-29 — CT-007 slice 5b.3-4a.1a landed: `v2` operational-reservation budget-authority
## machinery (Sol, design + 2 review rounds) — pure calculator/digest-encoder, NOT wired into any
## live reservation path yet

Designing 5b.3-4 (durable pre-workload usage across the launch-CAS ambiguity window) with Sol
surfaced that the problem is bigger than a journal: a checkout-bearing parent attempt now runs FOUR
sequential sandboxed executions (Hop A's advertise-refs + fetch, Hop B's checkout-materialization,
then the workload) against the SAME single `v1` reservation, which was only ever sized for one. Sol's
design: don't divide the existing per-container `limits` four ways (would silently weaken every
individual execution's own ceiling) — add a separate, ADDITIVE parent-attempt budget instead, via a
new versioned `v2` reservation policy. This also exposed that job retries have no existing cap at all
(reservations are job-scoped, but `lease_epoch`/`retry_attempts` can accumulate without limit) — Sol
recommended a finite, explicit, digest-bound `max_parent_attempts` policy constant. The user's call:
**5, and configurable** — implemented as a `CiAttemptBudgetPolicy` struct field (`production()` = 5),
matching this codebase's existing convention for policy constants (no env-var/per-tenant config
mechanism exists anywhere in this crate or its siblings to imitate instead — verified before
implementing, not assumed).

Given the scope (this is a live billing/capacity-reservation change to a system with real traffic,
not a self-contained refactor), the user was asked explicitly how to proceed rather than continuing
under standing autonomy alone; they confirmed: proceed with the full scope, apply a
production-capable-at-scale lens when unsure.

**What landed (5b.3-4a.1a only — the pure machinery, genuinely inert, confirmed via the dead_code
compiler check that nothing calls any of it yet):**
- `CiAttemptBudgetRevision::V1` / `CiAttemptBudgetPolicy{revision, max_parent_attempts: NonZeroU32}`
  — `production()` = V1 + 5. A "parent attempt" is a durably-begun claim generation: before Hop A for
  a checkout job, before workload launch itself for a compute job (which gets the SAME 5-attempt
  ceiling despite having no Hop A/B preparation at all — 5b.3-4a.2 must enforce the cap for both
  shapes).
- `ResourceCeiling{cpu_seconds, mem_byte_seconds}` — raw dimensions kept separate; `v1`'s own
  `operational_reservation_amount` combines them into one pre-converted `i64` at the first step, `v2`
  stays in raw dimensions through every intermediate aggregation and converts to operational units
  exactly ONCE, at the very end (`operational_amount_from_ceiling`).
- `raw_execution_ceiling` (mirrors `v1`'s own formula) → `parent_attempt_ceiling` (derives checkout
  presence ONLY from `request.checkout`, never a caller-supplied bool; named execution counts
  `CHECKOUT_TRANSPORT_EXECUTIONS=2`/`CHECKOUT_MATERIALIZATION_EXECUTIONS=1`/`WORKLOAD_EXECUTIONS=1`,
  never a bare `4`) → `job_lifetime_ceiling` (× `max_parent_attempts`) → `operational_reservation_amount_v2`.
- `operational_reservation_digest_v2` — a wholly separate encoder from `runtime_authority_digest`
  (byte-frozen, still shared by the `v1` batch domain), mirroring `token_authority_digest_v2`'s
  checkout-hashing pattern, additionally binding the budget-policy revision, `max_parent_attempts`,
  and the resulting ceilings — derives its ceiling inputs INTERNALLY from `request`/`policy` rather
  than accepting them as parameters (Sol's round-1 review: no authority encoder may accept
  independently forgeable derived facts).

**Sol's review, 2 rounds:** Round 1 found the digest function originally accepted caller-supplied
ceilings (a forgery risk — fixed by deriving them internally and returning `Result`), the final
pricing conversion lacked direct overflow-boundary tests (fixed by extracting
`operational_amount_from_ceiling` and testing both the `checked_add` overflow and the `i64`-range
overflow directly, since forcing them through the full realistic-limits pipeline was impractical),
and a doc correction (a "parent attempt" must cover compute jobs too, not just checkout preparation).
Round 2: cleared — 17/17 tests pass, `v1` completely byte-frozen (a hardcoded golden digest vector
proves it), two golden `v2` vectors (compute + checkout) pinned.

Sol also corrected the planned sequencing: activation splits further into **5b.3-4a.1b**
(compatibility/read-support only — every existing `v1`-only consumer in `ci_launch_authority.rs`
audited and made to recognize both prefixes; fresh writes stay `v1`) landing BEFORE **5b.3-4a.1c**
(writer activation — `PgTierPCiJobBudgetReservation` actually writes `v2` for fresh batches), so
every reader understands `v2` before any writer creates it — needed because keeping the existing
advisory lock key is necessary but not sufficient for safe rolling deployment (an old binary can't
discover or count `v2` rows after acquiring that lock, even though the lock still serializes it
against a new binary correctly).

**Test coverage:** 17 new tests in `ci_launch_authority.rs`'s new `v2_budget_authority_5b3_4a1a`
module. Full sweep clean: `cargo test -p myelin-ci-controlplane --lib` (500 passed, 0 failed — 483
pre-existing + 17 new), `cargo clippy` `-D warnings`, `cargo check --workspace --tests`. This file had
no pre-existing rustfmt skew (confirmed via a clean baseline check), so rustfmt ran directly with a
clean diff.

**Still open:** 5b.3-4a.1b (replay-compatibility read-side audit) through 5b.3-4a.1c (writer
activation), then 5b.3-4a.2 (the durable prelaunch-usage journal itself) and 5b.3-4b (reaper/
reconciliation/settlement consumers) all remain. 5b.3-5 through 5b.3-7 and the rest of ledger 12's
open items are unchanged.

---

## 2026-07-29 — CT-007 slice 5b.3-4a.1b landed: `v2` reservation-handle read-side compatibility
(Sol, design + 2 review rounds) — every existing `v1`-only consumer now recognizes `ci-reserve:v2:`;
fresh writes still mint only `v1`, unchanged

Design check-in with Sol before implementing surfaced that "recognize both prefixes" alone is not
enough for genuine dual-version replay: a `v2` batch's durable ceiling depends on
`CiAttemptBudgetPolicy` (revision + `max_parent_attempts`), and if that policy is ever revised
(e.g. `max_parent_attempts` changed from 5 to something else), a reader that recomputes an expected
`v2` candidate under its OWN currently-configured policy would never match an older durable row
minted under the policy that was live when it was written — breaking exact acknowledgement-loss
replay. Sol's design: embed the policy descriptor in the plain-text handle itself
(`ci-reserve:v2:<run_id>:<budget_tag>:<attempts_tag>:<batch_digest>:<job_id>:<request_digest>`, e.g.
`...:budget-v1:a5:...`) so replay can recover the EXACT policy a durable row was minted under from
the row alone, never from "whatever the provider is configured with right now."

**What landed** (six production consumers widened, all confirmed exhaustive by Sol — no other place
in the codebase assumes a `v1`-only reservation handle):

1. `ci_pipeline_driver.rs`: new `TIER_P_OPERATIONAL_RESERVATION_V2_PREFIX` constant.
   `validate_reservation_pricing_policy`'s gate now skips only if a handle matches NEITHER prefix
   (the zero-markup pricing formula underneath was already prefix-agnostic).
2. `ci_run_supersession.rs`: `settle_cancelled_job`'s hard refusal (previously: any non-`v1` handle
   could never settle at all) now accepts either prefix. `cancel_stale_queued_on_conn`'s
   `cost_reservation` attachment check now also matches a `v2`-shaped row.
3. `ci_runner_composition.rs`: `valid_reserve_handle` (the inbound claim-time format validator) now
   accepts either prefix.
4. `ci_launch_authority.rs` — the core of this slice:
   - `reserve_operational_batch_on_conn`'s replay-lookup query now searches both prefixes; the
     decision logic (split-by-version, refuse-if-mixed, replay-v1-unchanged, or hand off to `v2`)
     was factored into a new pure, synchronous, database-free `resolve_durable_replay` function so
     every branch is directly unit-testable.
   - `replay_v2_batch` parses every durable `v2` handle, requires them to all agree on the SAME
     embedded policy descriptor (refuses "disagree on budget policy descriptor" otherwise),
     reconstructs a `CiAttemptBudgetPolicy` from that parsed descriptor, and recomputes the expected
     candidate set to check for exact/tampered/partial divergence — the same rigor `v1` replay
     already had.
   - The active-reservation-ceiling COUNT query now counts rows matching either prefix together.
   - Fresh writes are UNCHANGED: still insert only `v1` handles. `PreparedOperationalReservation`
     carries no `v2` data at all this slice.

**Sol's round-1 review found a real bug in the first draft:** an earlier version had
`prepare_operational_batch` EAGERLY compute a `v2` candidate under the provider's current policy on
every batch (intended as groundwork for 4a.1c's writer switch). Sol caught that this meant a FRESH
`v1` write — or even an exact `v1` REPLAY — could fail purely because unrelated, never-written `v2`
ceiling arithmetic overflowed for the current policy, before any durable row was even read. Fixed by
removing all eager `v2` computation from `prepare_operational_batch` entirely; `v2` candidates are
now computed ONLY lazily, inside `replay_v2_batch`, using a policy reconstructed from an
already-durable row's own descriptor — never the "current" policy, which no longer exists as a
concept anywhere in this slice at all (the writer-activation policy field/constructor param move to
4a.1c). Round 1 also required an external (non-tautological) golden vector for the new `v2` batch
digest framing (added, computed once via a deliberately-wrong-placeholder-then-paste-panic-output
technique) and a dedicated partial-batch refusal test (the module doc had claimed this was covered;
it wasn't, now is). Round 2: cleared, no remaining blockers.

**Test coverage:** 32 new tests total. `ci_launch_authority.rs`'s new `handle_replay_5b3_4a1b`
module (27 tests): handle-format parse/round-trip + every malformed-input rejection, and the full
`resolve_durable_replay`/`replay_v2_batch` matrix — exact v1/v2 replay, tampered amount, tampered
digest, mixed-version refusal, disagreeing-policy-descriptor refusal, partial-batch refusal,
duplicate-row refusal, replay surviving a non-default written policy, the external golden batch
vector, and the three overflow-isolation regressions Sol required (fresh `v1` prep succeeds, exact
`v1` replay succeeds, and exact `v2` replay under a safe written policy succeeds — all using limits
that make `operational_reservation_amount_v2` under `CiAttemptBudgetPolicy::production()` overflow,
proving none of those three paths ever evaluates that arithmetic). Plus 1 unit test each in
`ci_pipeline_driver_tests.rs` (pricing gate) and `ci_runner_composition.rs` (handle validator), and
3 new integration tests against live Postgres: a cancelled job with a `v2` handle reaches real
settlement (`integration_ci_terminal_accounting_atomic.rs`), a `v2`-attached stale-queued run is
refused cancellation rather than silently superseded (`integration_pg_ci_pipeline_starter.rs`), and
a manually-inserted `v2` row counts against the same tenant ceiling as `v1` rows
(`integration_ci_operational_reservation.rs`).

While building the stale-queued-cancellation test, found and fixed (test-only, not introduced by
this slice) a pre-existing bug in `integration_pg_ci_pipeline_starter.rs`'s fixtures: they hardcoded
`commit_oid = 'deadbeef'` (8 chars), which the already-committed production checkout validation (CT-007
slice 5b.3-2, commit `221e4535`) rejects as not a valid 40/64-char hex commit id. Fixed with a
40-char `TEST_COMMIT_OID` constant used everywhere the old literal was.

Full sweep: `cargo test -p myelin-ci-controlplane --lib` → 527 passed, 0 failed (500 pre-existing +
27 new). The 3 new integration tests plus every pre-existing test in the 3 files they were added to
pass against live Postgres. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
clean. `cargo check --workspace --tests --all-features` clean. rustfmt applied cleanly to
`ci_launch_authority.rs` and the two operational-reservation/pipeline-starter test files (confirmed
no pre-existing skew via a `git stash` baseline check first); `integration_ci_terminal_accounting_atomic.rs`
has known pre-existing rustfmt-version skew unrelated to this change (same baseline check) — left
untouched, with the new test intentionally matching that file's existing style.

**Still open:** 5b.3-4a.1c (writer activation — inject/store `CiAttemptBudgetPolicy` in the
provider again, this time as the small, already-reviewed switch selecting the `v2` candidate for
fresh batches), then 5b.3-4a.2 (the durable prelaunch-usage journal itself) and 5b.3-4b (reaper/
reconciliation/settlement consumers). 5b.3-5 through 5b.3-7 and the rest of ledger 12's open items
are unchanged.

---

## 2026-07-29 — CT-007 slice 5b.3-4a.1c landed: `v2` writer activation, gated OFF in production
(Sol, design + 1 review round) — fresh batches can now genuinely mint `v2` handles; the production
composition root stays pinned to `v1` pending a deliberate, separately-tracked fleet-convergence flip

Design check-in with Sol before implementing: the original task sketch (written before 4a.1b's
round-1 bug) proposed a unified version-tagged `PreparedOperationalReservation`. Sol agreed the
simpler shape established by 4a.1b's fix — `PreparedOperationalReservation` stays `v1`-only forever;
`v2` candidates are computed lazily, only exactly where needed — should carry through unchanged.
Sol also flagged a real rollout-safety gap: writing `v2` is only safe once every reader in the fleet
already understands it (4a.1b deployed everywhere), and a preceding commit landing is NOT the same
guarantee as fleet convergence during a rolling deploy. Since this codebase has no config/feature-
flag mechanism, the fix is a new `OperationalReservationWriteVersion` enum (`V1`/`V2`) the provider
stores explicitly; the production composition root in `lib.rs` passes `V1` deliberately, with a
comment explaining the gate must be flipped in its own commit once convergence is confirmed. The
writer is fully implemented and tested here; it is simply not live yet.

**What landed:**
- `build_v2_candidates` now calls `validate_handle` internally on every candidate before returning,
  so the complete batch is validated before any candidate is usable (Sol's requirement).
- `reserve_operational_batch_on_conn` gained `budget_policy`/`write_version` params. The advisory
  lock, replay-lookup query, and `resolve_durable_replay` dispatch are completely UNCHANGED — durable
  `v1` replay and durable `v2` replay (via reconstructed policy) never touch `write_version` or the
  current `budget_policy`. Only in the genuinely-fresh branch (no durable rows, ceiling check passed)
  does it branch: `V1` keeps today's insert; `V2` calls `build_v2_candidates` for the first time in
  the whole call and inserts those instead. The complete candidate `Vec` is built before any INSERT
  starts, so a `v2` overflow anywhere in a fresh batch refuses atomically with zero rows written.

**Sol's one review round** found no correctness blockers, only a stale doc comment (fixed —
`PreparedOperationalReservation`'s doc now correctly names both live `v2` call sites: the fresh-write
branch under the current policy, and durable replay under a reconstructed one) and a test-naming
nit (`a_manually_seeded_v1_batch...` renamed to `an_existing_v1_batch...` since it seeds via a real
`V1`-configured provider, not manual SQL — stronger evidence than the old name implied). Sol
confirmed the `v1` literal gate is sufficient enforcement (no alternate production composition root
found that could enable `V2`) but that the later flip must stay its own explicitly-tracked commit.

**Test coverage:** the existing `tier_p_operational_reservation_is_atomic_retry_stable_and_bounded`
megatest's four providers now all use `OperationalReservationWriteVersion::V2`, exercising the real
writer end-to-end — handle-prefix and amount assertions flipped to `v2` (15,000, since the fixture's
jobs are checkout-bearing under production's 5-attempt policy vs. `v1`'s 750), and the crash-
injection trigger's `LIKE` pattern updated to match `v2`'s handle shape (otherwise it would silently
stop firing and stop testing the rollback path at all). Three new dedicated integration tests:
`an_existing_v1_batch_replays_unchanged_through_a_v2_writing_provider` (durable-row precedence over
the provider's write setting), `v2_written_under_one_policy_replays_correctly_under_a_differently_configured_provider`
(replay recovers the ORIGINAL embedded policy, not the replaying provider's current one), and
`fresh_v2_overflow_leaves_zero_rows` (atomicity on a genuinely fresh overflowing batch). The prior
4a.1b ceiling-counting test still passes unchanged.

Full sweep: `cargo test -p myelin-ci-controlplane --lib` → 527 passed (unchanged — no new unit tests
needed; the interesting logic here is the async DB-touching branch selection, covered by
integration tests instead). `integration_ci_operational_reservation.rs` now has 5 tests, all passing
against live Postgres; the other two integration files from 4a.1b (`v2` settlement, `v2` stale-
queued-cancellation refusal) still pass unchanged since they use the production composition root,
which stays gated at `v1`. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
clean. `cargo check --workspace --tests --all-features` clean. rustfmt applied cleanly (confirmed no
pre-existing skew via a `git stash` baseline check first) — caught and reverted one mistake mid-
session where running `rustfmt` directly on `lib.rs` in write mode (the crate root) had also
reformatted the unrelated, pre-existing-skewed `job_queue_region.rs` as a side effect; reverted that
file specifically before finishing. Lesson for future rustfmt passes in this crate: never run
`rustfmt` (write mode) directly on `lib.rs` or any other crate-root file — it silently traverses and
rewrites the WHOLE module tree; always target the specific file(s) actually changed.

**Still open:** 5b.3-4a.2 (the durable prelaunch-usage journal itself: `ci_job_prelaunch_usage`
table, write-ahead `begin`/`complete`/reaper-`seal` state machine) and 5b.3-4b (reaper/reconciliation/
settlement consumers of that journal) remain. The `v1`→`v2` fleet-convergence flip in `lib.rs` is a
separate, explicitly-tracked follow-up once deployment safety is confirmed — not yet scheduled as a
ledger task. 5b.3-5 through 5b.3-7 and the rest of ledger 12's open items are unchanged.

---

## 2026-07-29 — CT-007 slice 5b.3-4a.2 schema landed (Sol, design + 2 review rounds): the durable
`ci_job_parent_attempt` + `ci_job_prelaunch_usage` journal pair — schema and RLS/grants only, the
Rust `begin`/`complete`/`seal` state machine is still open

Sol's initial design check surfaced that "recognize both job shapes" needed a genuinely different
shape than task #89's original single-table sketch: a **common `ci_job_parent_attempt` table**
counts durably-begun claim generations as ROWS (never `MAX(lease_epoch)`, never phase rows) so a
checkout job's two prelaunch phases count as exactly one attempt while a compute job (no phase at
all) still counts correctly without inventing a fake phase for it — plus a **checkout-only child
`ci_job_prelaunch_usage`** table for the two real phases. `max_parent_attempts`/`budget_revision` are
persisted on the ATTEMPT row itself (never re-derived from current configuration later).

**What landed:**
- `ci_job_parent_attempt`: tenant/region/job/wf_run/ci_run/reserve_handle/lease_owner/lease_epoch/
  claim_nonce/claim_started_at_epoch_secs/claim_expires_at_epoch_secs/budget_revision/
  max_parent_attempts/begun_at. PK `(tenant,region,job,lease_epoch,claim_nonce)` plus two `UNIQUE`
  constraints preventing a divergent epoch/nonce pairing from becoming a second attempt row. FK to
  `ci_run` only (mirrors `ci_job_accounting`'s FK posture, avoiding lock contention on the hot
  `job_queue` table). Immutable: `REVOKE UPDATE, DELETE` + reject-mutation trigger, the exact
  `ci_job_accounting`/`ci_drive_manifest` convention.
- `ci_job_prelaunch_usage`: adds `phase`/`status` to the parent-attempt key, FK-anchored to it.
  `status` is `started` (ceiling only) → `measured` (exact usage, `complete_phase`) or
  `sealed_ceiling` (the reaper's fallback when a worker never reports, `seal_phase`) — both terminal.
  Deliberately NO database `exact <= ceiling` CHECK (an honest over-ceiling measurement must stay
  writable). A `BEFORE UPDATE` trigger — not just `REVOKE`, since legitimate transitions need
  `UPDATE` — refuses any UPDATE once a row leaves `started` (both terminal states are unconditionally
  terminal) and refuses tampering with identity/ceiling/`started_at` on the one legal transition.
  `DELETE` is revoked outright.
- A non-blocking partial reaper index `(region, started_at) WHERE status = 'started'`, plus (added in
  round 2 after Sol caught the gap) a full scheduler RLS/grant boundary mirroring `job_queue`'s: the
  real `myelin_ci_region_scheduler` role gets read-only SELECT on the parent table and SELECT +
  column-scoped `UPDATE (status, resolved_at)` on the child — column-scoped, never table-wide, same
  as every other scheduler grant in this file.
- Both tables added to `ci_controlplane_hot_tables()` (per-attempt/per-phase write churn comparable
  to `ci_cost_event`'s per-metered-unit rate).

**Sol's round-1 review found two real schema blockers**, not caught by my first draft:
1. The four usage columns were originally `bigint`, but `ResourceUsage`/`ResourceCeiling` are `u64`
   and — as CT-007 slice 5b.3-4a.1a's own overflow tests already demonstrate — a scaled ceiling can
   legitimately exceed `i64::MAX` while still fitting in `u64`. Fixed with `numeric(20,0)` +
   explicit `CHECK (... BETWEEN 0 AND 18446744073709551615)` bounds; `max_parent_attempts` moved from
   `integer` (32-bit signed, can't hold half of `u32`'s range) to `bigint CHECK (... BETWEEN 1 AND
   4294967295)`. No new Rust dependency needed yet (schema only, nothing binds these columns until
   the state-machine slice; the plan is a `::text` cast round trip, not `bigdecimal`/`rust_decimal`).
2. The `(region, started_at)` reaper index implied a cross-tenant regional scan, but the tables
   carried only ordinary per-tenant RLS — the real scheduler role would have hit "permission denied"
   the first time it tried to reap, the exact gap `GRANT_SCHEDULER_CI_JOB_REAP_RESET_DDL`'s own doc
   comment already names as a real production incident elsewhere in this file. Fixed with the
   scheduler RLS boundary described above. Round 1 also required three additional CHECK constraints
   (`lease_epoch > 0`, `claim_expires_at_epoch_secs > claim_started_at_epoch_secs`,
   `resolved_at >= started_at` when resolved) and real trigger-exercising integration tests instead
   of DDL-text pinning alone. Round 2: cleared, no remaining blockers.

**Test coverage:** two new unit tests pin the immutable/FK/CHECK shape of both DDL constants
(mirroring the existing `job_accounting_is_complete_unique_and_structurally_insert_only` pattern);
four existing hardcoded-count assertions updated (18→20 tables, 44→48 total migrations, five→seven
hot tables). One new dedicated integration test
(`tests/integration_ci_prelaunch_usage_journal.rs`) against live Postgres exercises the REAL state
machine, not just DDL text: parent-attempt UPDATE/DELETE refusal; a genuine CHECK violation (a
`started` row with non-null exact usage); the legal `started→measured` transition; the illegal
`measured→started` revert refusing via the trigger's `RAISE EXCEPTION` (SQLSTATE `P0001` — caught
and fixed a mismatch where I'd initially expected `23514`, the actual-CHECK-constraint code, for a
trigger-raised refusal instead); a second phase reaching `started→sealed_ceiling`; a late completion
after sealing refusing; identity/ceiling tampering on the one legal transition refusing; DELETE
refusing outright; and the REAL `myelin_ci_scheduler_fr_par` production reaper role successfully
sealing a started row through the exact column-scoped grant while being refused on any other
column/table.

Full sweep: `cargo test -p myelin-ci-controlplane --lib` → 529 passed (0 failed — the two new DDL-
pinning tests, no regressions). The new dedicated integration test plus the two existing full-
migration-application integration tests all pass against live Postgres. `cargo clippy --workspace
--all-targets --all-features -- -D warnings` clean. rustfmt applied cleanly to every touched file
(confirmed no pre-existing skew via a `git stash` baseline check first) — caught and reverted, a
SECOND time this session, the same `rustfmt` (write mode) directly on `lib.rs` collateral-damage
mistake reformatting the unrelated, pre-existing-skewed `job_queue_region.rs`; reverted it again
before finishing both times. This is now a standing rule for this crate: never run `rustfmt` write
mode on `lib.rs` (or any crate-root file) directly — always target the specific non-root file(s)
actually changed.

**Deliberately NOT in this commit** (Sol's explicit scope line): the Rust `begin_parent_attempt`/
`begin_phase`/`complete_phase`/`seal_phase` state machine itself. Sol's design for
`begin_parent_attempt` requires a "narrow verified resolver," not the simpler plan I'd sketched
(parse the v2 handle's embedded run/job id + check `cost_reservation` exists) — Sol's review: a
durable-but-corrupted/manually-inserted `cost_reservation` row could carry a tampered digest/amount,
so the parser's own contract is explicitly non-authoritative. The real resolver must, in one tenant
transaction: require agreement among the claim, the manifest job's `reserve_handle`, and durable
`ci_job_spec.spec.meter_to.reserve_id`; reconstruct the policy from the `v2` descriptor; recompute
the COMPLETE expected `v2` handle and amount from durable authority and compare both byte-for-byte
against `cost_reservation`; require the reservation to already be `inflight` if `hooks.reserve` has
completed, or accept `reserved` only if this operation atomically transitions it to `inflight`; and
refuse `v1` outright for the capped journal path. This closes the journal's OWN authority check;
task #91 (binding `reserve_id` into the signed launch credential) remains a separate, still-necessary
fix for the broader launch-hook boundary — the two are complementary, not duplicative.

**Still open:** the Rust state machine described above (a substantial piece in its own right, likely
warranting its own multi-round design/review cycle), 5b.3-4b (reaper/reconciliation/settlement
consumers), and task #91 (credential binding). The `v1`→`v2` fleet-convergence flip in `lib.rs`
remains a separate follow-up. 5b.3-5 through 5b.3-7 and the rest of ledger 12's open items are
unchanged.

---

## 2026-07-30 — CT-007 slice 5b.3-4a.2 complete: the Rust `begin_parent_attempt`/`begin_phase`/
`complete_phase`/`seal_phase` state machine (Sol, implementation; Sonnet, independent review)

Sol implemented the narrow verified resolver its own prior design round specified, directly in
`crates/myelin-ci-controlplane/src/ci_prelaunch_usage_journal.rs`. I (Sonnet) read the full diff and
independently re-ran the gate rather than trust the self-report, per this ledger's evidence-over-
assertion rule; Sol was the builder here, not the reviewer, so the adversarial-verifier role fell to
me for this slice.

**What landed:** `CiPrelaunchUsageJournal::begin_parent_attempt` admits one exact live claim
generation inside a single tenant transaction: it locks the scheduler claim and run-of-record (the
same lock order as `LockedManifestCiJobTokenIssuer::mint`), loads the immutable manifest and durable
launch template, cross-checks the caller-supplied `reserve_handle` against both the manifest job's
own `reserve_handle` and the durable `ci_job_spec.spec.meter_to.reserve_id`, locks the
`cost_reservation` row `FOR UPDATE`, recomputes the policy from the `v2` handle and rebuilds the
*complete* candidate batch (every job in the manifest, not only the claimed one — the batch digest
binds all of them) to compare the expected handle and amount byte-for-byte against the durable row,
requires `inflight` or atomically advances `reserved`→`inflight`, and refuses `v1` outright. A
tenant/region/job-scoped advisory lock plus an exact epoch/nonce replay check make a second call with
the same claim generation return the existing row (`Replayed`) rather than double-insert; a genuinely
new generation is admitted only under the policy's `max_parent_attempts` cap, which counts durable
attempt ROWS (a zero-preparation generation still counts). `begin_phase`/`complete_phase`/`seal_phase`
follow the schema's own `started→{measured,sealed_ceiling}` monotonic contract: exact retries replay,
divergent measurements or illegal crossings (e.g. sealing after measuring, completing after sealing)
refuse — the DB trigger's `P0001` is mapped to a typed `IllegalPhaseTransition`, never leaked raw.

To support this, `ci_claim_token_issuer::authority_from_durable_claim` was refactored (behavior-
preserving for its existing claim-token-minting caller) into a shared
`runtime_authorities_from_durable_claim` that reconstructs every manifest job's runtime-authority
request in canonical order; both the original minting path and the new journal's batch-digest
resolver now consume the same reconstruction, so there is exactly one place that builds this
authority list from durable facts. `ci_launch_authority` gained the actual narrow resolver,
`verify_v2_operational_reservation`, plus `raw_execution_usage_ceiling` (exposed so the journal
derives phase ceilings from the same checked arithmetic as the `v2` reservation itself, scaled by
each phase's known execution count — 2 for checkout transport, 1 for materialization).

**Sol's own self-review caught the one bug that mattered:** its first draft recomputed the expected
`v2` handle from only the claimed job's authority; since `build_v2_candidates` binds a batch digest
over every job in the manifest, that would have refused every real multi-job manifest. Fixed by
reconstructing the complete batch before recomputing the candidate for the claimed job specifically.

**My independent review:** read the full diff (`ci_prelaunch_usage_journal.rs` new, plus the
`ci_claim_token_issuer.rs`/`ci_launch_authority.rs` diffs) against the checkout-authority-chain
precedent and the `reserve_operational_batch_on_conn` locking convention gathered beforehand. Traced
each of the five admission requirements to its exact code path and confirmed the batch-digest fix is
real (`verify_v2_operational_reservation` takes the complete `requests` slice, not a single job).
Confirmed the `lib.rs` diff is a clean two-line addition (module + re-export) — the crate-root
rustfmt mistake from earlier sessions was not repeated. Re-ran independently rather than trust the
report: `cargo test -p myelin-ci-controlplane --lib` → 533 passed; the new
`integration_ci_prelaunch_usage_state_machine.rs` against live Postgres → 1 passed (a single
comprehensive test exercising fresh admission, exact replay for all four functions, every one of the
five refusal seams individually, both illegal-transition directions, the attempt cap counting a
zero-preparation generation, and a full `u64::MAX` round-trip through the `numeric` text-cast path);
the neighboring `integration_ci_prelaunch_usage_journal.rs` and `integration_ci_operational_reservation.rs`
suites → 6 passed, no regressions; `cargo clippy --workspace --all-targets --all-features -- -D
warnings` clean; `cargo check --workspace --tests --all-features` clean.

**Not covered, flagged as a non-blocking gap:** the test suite is single-threaded/sequential: no
concurrency test exercises two simultaneous `begin_parent_attempt` calls racing on the same
reservation or the same fresh generation. The `FOR UPDATE` lock plus the advisory lock give good
reason to expect this is race-safe, but it is asserted from code reading, not measured under real
concurrency — worth a dedicated concurrency drill before this path carries production traffic.

**Still open:** 5b.3-4b (reaper/reconciliation/settlement consumers of this journal — the natural
next slice), task #91 (credential binding), and the `v1`→`v2` fleet-convergence flip in `lib.rs`.
5b.3-5 through 5b.3-7 and the rest of ledger 12's open items are unchanged.

---

## 2026-07-30 — CT-007 slice 5b.3-4b.1: topology-aware regional sealer + shared settlement resolver
(Sol, design + implementation; Sonnet, design pushback + independent review)

Before any code, Sol proposed a design (regional sealer extending `JobQueueReaper`, a shared
`resolve_prelaunch_usage_on_conn` reader, one-shot final settlement, journal-authoritative/workload-
only reporting contract) and flagged its own gap: the sealer's obvious deadline signal,
`claim_expires_at_epoch_secs`, is driven by the flat `CI_RUNNER_LEASE_TTL_SECS` (`MAX_JOB_TIMEOUT_SECS
+ 600s` = ~6h10m), but one checkout-bearing parent attempt can contain four sequential full-limit
executions (Hop A x2, Hop B x1, workload x1), each legally configurable up to `MAX_JOB_TIMEOUT_SECS`
(6h) — so a legitimately-configured long job could need ~24h while its lease reads as expired at
~6h10m. I independently checked the actual constants (`runner_bind.rs:145`,
`job_spec_store.rs:75`) before accepting this as real, then required it be resolved as PART of 4b.1,
not deferred to 5b.3-6 as Sol's design had proposed — the sealer's own correctness depends on it.

**Sol's fix:** decouple the sealing deadline from the flat lease entirely. Each phase now gets a
durable, topology-derived `seal_after` timestamp written once at `begin_phase` time:
`timeout_secs * phase.execution_count() + 600s` headroom (transport = 2x, materialization = 1x) —
sized from the job's own durable limits, immutable thereafter (the existing transition trigger now
also guards `seal_after` identity). The regional sealer scans `WHERE status = 'started' AND
seal_after IS NOT NULL AND seal_after <= statement_timestamp()` via a bounded (64-row),
`FOR UPDATE SKIP LOCKED` materialized-CTE page — never the flat lease. NULL legacy deadlines
(pre-migration rows) are deliberately invisible to the sealer, never guessed abandoned. New
migrations `ci_0020c`/`ci_0020d` are purely additive (nullable expand, `NOT VALID` + `VALIDATE
CONSTRAINT` online pattern, `CREATE INDEX CONCURRENTLY`) — migration count 48→50, confirmed no
existing migration DDL text was modified.

**What else landed:**
- The sealer is wired into the existing `JobQueueReaper::reap_once` cadence (15s), not a second
  background loop; prelaunch sealing, lease requeue, and cancelled-reconciliation are now attempted
  independently within one sweep so a failure in one surface can't suppress the others.
- `resolve_prelaunch_usage_on_conn`: the shared settlement reader. Locks the `job_queue` row first
  (canonical queue→advisory→journal lock order), validates every durable parent-attempt row against
  the caller's full settlement identity (tenant/region/job/wf_run/ci_run/reserve_handle) before
  trusting any of them, sums measured-exact or sealed-ceiling usage per phase with checked arithmetic,
  and refuses (`UnresolvedPhase`)/optionally force-seals (`SealToCeiling`, for authoritative
  cancellation owners) any phase still `started`. v1 reservations return zero journal usage
  unconditionally (preserves existing behavior); v2 requires >=1 parent attempt unless the caller
  explicitly allows zero (unlaunched/skipped jobs).
- Closed the race Sol's own design flagged: `begin_phase`/`complete_phase`/`seal_phase` now re-lock
  and re-verify the *exact live* `job_queue` generation (state, lease identity, timestamps,
  `completion_receipt IS NULL`) before any journal mutation — a stale `CiJobParentAttempt` handle
  held past terminal accounting can no longer write to the journal.
- Scheduler privilege probe extended to the two new tables, preserving the existing least-privilege
  convention exactly: read-only SELECT plus column-scoped UPDATE on `status`/`resolved_at` only,
  with `excess_privilege` now catching any other grant (INSERT/UPDATE-on-other-columns/DELETE/
  TRUNCATE/REFERENCES/TRIGGER) on either table.

**My independent review:** read every file in the diff (`ci_prelaunch_usage_journal.rs`,
`migrations.rs`, `job_queue_region.rs`, `job_queue_store.rs`, `ci_scheduler_db.rs`) before accepting
the self-report. Confirmed the migration diff is additive-only (no existing DDL constant edited —
Sol's own note about the checksum guard catching an attempted edit mid-session was already reverted
in the final diff). Confirmed `phase_seal_window_secs` matches Sol's stated numbers via its own unit
test (43,800s / 22,200s at the 6h ceiling). Confirmed the concurrency drill is real, not asserted:
the new `integration_ci_prelaunch_usage_state_machine.rs` test exercises two simultaneous
`begin_parent_attempt` calls (exactly one applies, one replays, one durable row) and a genuine
stale-generation drill (a requeued `job_queue` row correctly refuses a subsequent `begin_phase` via
the new live-generation check) — this closes the concurrency gap I flagged as non-blocking in the
prior 4a.2 entry. Re-ran the full gate myself rather than trust the report: `cargo test -p
myelin-ci-controlplane --lib` → 535 passed; the state-machine, journal-schema, terminal-accounting,
and operational-reservation live-Postgres suites → 9 passed, no regressions (including the
mentioned test-only mutex fix for the terminal-accounting file's migration-setup race); `cargo
clippy --workspace --all-targets --all-features -- -D warnings` clean; `cargo check --workspace
--tests --all-features` clean.

**Explicitly NOT settled here (correctly deferred):** the flat claim-expiry/lease-renewal topology
mismatch is no longer a sealer-correctness problem, but it's still a real prerequisite before
5b.3-6 can run genuinely long real-world preparation end-to-end — a job whose lease still expires at
~6h10m will get requeued by the ordinary lease reaper long before a legitimate 24h worst-case
checkout could finish, independent of the journal. That reconciliation (lease sizing or renewal) is
tracked as a named prerequisite for 5b.3-6, not resolved by this slice.

**Still open:** 5b.3-4b.2 (wire `resolve_prelaunch_usage_on_conn` into every terminal owner —
`CiPipelineReporter` normal completion, `report_retryable_attempt` cancellation, `PgCiRunSupersession`
queued/leased cancellation, `reconcile_abandoned_job`, and existing-accounting replay verification —
and prove one-shot settlement end to end; 4b is inert until both land), task #91 (credential
binding), the `v1`→`v2` fleet-convergence flip, and the lease/topology prerequisite for 5b.3-6 noted
above. 5b.3-5 and 5b.3-7 and the rest of ledger 12's open items are unchanged.

---

## 2026-07-30 — CT-007 slice 5b.3-4b.2: wired the settlement resolver into all five real terminal
owners; 4b is no longer inert (Sol, implementation; Sonnet, independent review + two unrelated fixes)

Sol wired `resolve_prelaunch_usage_on_conn` into every real settlement path per its own 4b.1 design:
normal completion (`CiPipelineReporter`), cancellation-terminal retry (`report_retryable_attempt`),
queued/leased cancellation (`PgCiRunSupersession`), abandoned/expired launched-work reconciliation,
and existing-accounting replay verification. Confirmed a key finding before accepting it: checkout
preparation is not yet composed into `launch_with` (that's 5b.3-6, still open), so `TerminalReport`
today genuinely carries workload-only usage already — no call site needed the workload-only
reporting-contract change flagged as an open issue in the 4b.1 entry, because none of the five
currently reports checkout-hop usage. The contract is now documented in code comments at both real
call sites (`ci_pipeline_driver.rs`) so it stays correct once 5b.3-6 lands.

**Settlement math per owner** (matches 4b.1's design exactly): normal completion = prelaunch (all
parent attempts) + prior retries + current workload, `Required + Refuse`. Cancellation-terminal retry
= prelaunch + all recorded workload attempts, `Required + Refuse`. Queued/leased cancellation =
prelaunch + prior retries, `OptionalBeforeLaunch + SealToCeiling` (a job cancelled before ever being
claimed may have zero parent attempts and, for a job that was never enqueued at all, no `job_queue`
row either — `resolve_prelaunch_usage_on_conn` was extended to tolerate a missing queue row only
under `OptionalBeforeLaunch`, while still refusing if parent-attempt rows exist without a backing
queue row, an internal-consistency check). Abandoned/expired launched work = prelaunch + prior
retries + the existing immutable workload ceiling (never re-measured usage), `Required +
SealToCeiling`. Existing-accounting replay = re-resolves the journal, verifies the previously-written
receipt's usage against the recomputed floor (`>=` for normal-completion replay, exact match for
superseded/cancelled replay), and settles nothing new — read/verify-only. A new
`CiUsageAggregationError` (`Overflow`/`DurableRange`) is checked at every aggregation point before
usage reaches pricing or the `bigint`-backed accounting tables, refusing typed rather than
clamping/wrapping, proven by a direct unit test at both boundaries.

**My independent review:** read the full diff (`ci_pipeline_driver.rs`, `ci_run_supersession.rs`,
`ci_pipeline_driver_tests.rs`, the extended `integration_ci_terminal_accounting_atomic.rs`, the small
`ci_prelaunch_usage_journal.rs` queue-existence relaxation). Traced the workload-only claim by
grep — confirmed no production call site composes checkout preparation into `launch_with` yet.
Confirmed the settlement math per owner against the design. Confirmed the overflow/bigint-range
guard is real via its dedicated test.

**While running the full crate test suite (not just the curated subset from 4a.2/4b.1) to verify,
found and fixed two bugs — committed separately as `4dba5754` before this slice's commit:**
1. `production_pg_bootstrap_source.rs`'s static source-inspection test failed on an unscoped
   `str::find` matching an earlier doc comment instead of the real `ownership.release()` call site
   in `launch_gate.rs` — confirmed pre-existing (reproduced against last HEAD with zero uncommitted
   changes applied) and unrelated to any file in this session's diffs.
2. The SAME test file's pinned call-site string for `authority_from_durable_claim` was stale: this
   session's own earlier 4a.2 commit (`c61b4b55`) added a fourth argument (`&launch_template`) to
   that real call, but I had not run this test file as part of that commit's verification, so the
   regression passed uncaught through both the 4a.2 and 4b.1 commits. Root-caused and fixed; this is
   a gap in my own verification process, not Sol's — the full crate test suite (all targets, not a
   curated subset) is now what I run before every commit going forward on this track.
3. (unrelated third bug, same investigation) `integration_pg_ci_pipeline_starter.rs`'s two tests each
   run independent `PgMigrator` sequences against the same live Postgres; run concurrently (Rust's
   default), they hit a genuine advisory-lock deadlock. Fixed with the same `MIGRATION_SCENARIO_LOCK`
   serialization guard already used in `integration_ci_terminal_accounting_atomic.rs`. Also confirmed
   pre-existing and unrelated to this session's changes.

**Full gate, re-run after both fixes:** `cargo test -p myelin-ci-controlplane --lib` clean; the
*entire* `--all-targets --features integration` matrix (every integration/unit test file in the
crate, not a named subset) — all green, including the 7 terminal-accounting-atomic tests (one new
test per owner, each proving prelaunch usage — a mix of a measured phase and a sealed-ceiling phase —
lands in the final settlement exactly once) and the 2 pipeline-starter tests re-run three times to
confirm the deadlock fix is stable, not merely quieter; `cargo clippy --workspace --all-targets
--all-features -- -D warnings` clean; `cargo check --workspace --tests --all-features` clean.

**4b is complete.** The journal (4a.2), the topology-aware sealer (4b.1), and the settlement wiring
(4b.2) together mean prelaunch checkout-preparation usage — once 5b.3-6 actually composes checkout
into `launch_with` — will be durably admitted, reaped on a correct deadline, and settled exactly once
through every real terminal path, with no silent double-count and no silent overflow.

**Still open:** task #91 (credential binding), the `v1`→`v2` fleet-convergence flip in `lib.rs`, the
lease/topology prerequisite for 5b.3-6 (noted in the 4b.1 entry), and 5b.3-5/5b.3-6/5b.3-7 themselves
(5b.3-6 is what will actually compose checkout preparation into `launch_with` and make this journal
load-bearing in production — until then 4b is correct but dormant). The rest of ledger 12's open
items are unchanged.

---

## 2026-07-30 — CT-007 slice 5b.3-5a: preparation-terminal disposition vocabulary + durable
accounting compatibility, deliberately NOT wired to any live completion path yet (Sol, design +
implementation; Sonnet, independent review via a fresh-context reviewer + two required fixes)

**The gap:** a job that gets through checkout preparation but never reaches workload launch (prep
itself failed, or the job was cancelled/superseded mid-prep) has no honest way to report a terminal
result today, because the existing workload-completion CAS (`CONSUME_CLAIM_QUERY`, `scheduler.rs`)
hard-requires `state = 'running'` in its `WHERE` clause — a preparation-only job stays `leased`.
Verified this myself directly against the query text before accepting the premise. Loosening that
CAS to also accept `leased` would erase the guarantee it exists for (a merely-leased worker can't
fabricate workload results) — so 5b.3-5 needs a genuinely separate completion path, not a relaxed
existing one. This slice (5a) builds only the vocabulary and durable-schema half; the actual new
completion CAS (5b) is deliberately deferred to its own review, same pacing as 4a.2/4b.1/4b.2.

**What landed:** a closed `PreparationTerminalDisposition` (`Failed{phase}`/`TimedOut{phase}`/
`AttemptsExhausted`) and `PreparationAttemptDisposition` (`Terminal`/`RefusedBeforeExecution`/
`RetryableInfrastructure`/`ReconciliationRequired{teardown_unproven,usage_unrepresentable,
quarantine_required}`) — deliberately carrying no `ResourceUsage`, no caller-settable `passed`, and
no arbitrary free text, so a preparation outcome can never be structurally confused with a workload
result. Every Hop A/Hop B checkout-preparation error path is now structurally classified into this
vocabulary (replacing the old `CheckoutTransportError::Failed`/`CheckoutPreparationError::
RejectedAfterQuiescence` conflation). `ci_job_accounting` gained an additive nullable-column v4
receipt encoding (closed disposition + a v4 completion-receipt shape) alongside the existing v3
columns, with historical v3 rows replaying byte-for-byte unchanged.

**Independent review (a fresh-context reviewer agent, not Sol, then verified myself) found two real
issues before I'd commit anything:**

1. **A genuine Hop A/Hop B inconsistency.** Hop A's `map_hop_run_failure` already special-cased
   `RunFailure::CommitOutcomeUnknown` to `ReconciliationRequired`, with a comment noting it's
   "unreachable in practice" via the immediate-launch-permit invariant but must never be silently
   downgraded if it somehow occurred. Hop B's equivalent function did NOT match by variant — every
   `RunFailure`, including `CommitOutcomeUnknown`, folded uniformly into ordinary
   `RetryableInfrastructure`. I confirmed this myself by reading `gvisor.rs` directly before raising
   it. Not currently exploitable (both paths reach it only through the same "unreachable" invariant),
   but 5b.3-5b is about to build real reconciliation routing on this classification, and a future bug
   violating that invariant would fail OPEN in Hop B while failing CLOSED in Hop A — a real latent
   risk, not cosmetic. **Fixed:** `map_checkout_materialization_run_failure` now matches every
   variant explicitly, routing `CommitOutcomeUnknown` to `ReconciliationRequired` exactly like Hop A,
   proven by a dedicated regression test
   (`hop_b_commit_outcome_unknown_is_never_downgraded_to_an_ordinary_retry`).
2. **A judgment call I raised rather than assumed, and Sol reasoned through more precisely than my
   own hypothesis.** Three already-live production completion paths (ordinary completion,
   cancellation-terminal retry, supersession/skip-before-start) started unconditionally writing the
   new v4 disposition data with no feature flag. My own first-pass read was that this looked lower-
   risk than the v1→v2 reservation-writer precedent (pure additive metadata, no dollar amount
   changes) — but I asked Sol for its own reasoning rather than accept that lean. Sol identified the
   real risk precisely: it's not about money, it's about **fleet-convergence replay-safety** — an
   older reporter binary replaying a row a newer binary wrote as v4 would recompute the OLD v3
   receipt/summary shape, fail exact-equality against the durable v4 row, and spuriously refuse an
   otherwise-legitimate idempotent redelivery during a rolling deployment. That is the identical risk
   class the v1→v2 reservation writer was gated for, just via a different mechanism. **Fixed:** a new
   `CiJobAccountingWriteVersion` (`V3`/`V4`) on `CiJobAccountingStore`, defaulting to `V3` via
   `with_pg`; `with_pg_and_write_version(..., V4)` is the explicit opt-in, and the store refuses an
   accidental V4 write while configured for V3. I confirmed every production composition-root call
   site (`ci_runtime_composition.rs`, `ci_run_supersession.rs`, `lib.rs`) uses the plain `with_pg`
   default — V4 is genuinely gated off in production, not just claimed to be.

**Full independent verification, re-run after both fixes** (not the self-report): `myelin-ci-sandbox
--lib` → 463 passed; `myelin-ci-controlplane --lib` → 540 passed; the *entire* control-plane
integration matrix (`--all-targets --features integration`, every test binary in the crate) → all
green; `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean; `cargo check
--workspace --tests --all-features` clean.

**Confirmed nothing new is live-callable yet:** the new classification functions and result-summary
encoder are reachable only from their own unit tests; `CONSUME_CLAIM_QUERY` is untouched and a test
still pins that it never accepts `leased`. 5a is genuinely dormant scaffolding plus (now properly
gated) durable-schema enrichment — no new production behavior beyond the v4 opt-in seam itself.

**Still open:** 5b.3-5b (the actual `report_preparation_terminal` completion CAS, racing safely
against the existing workload CAS — deliberately not started this slice), 5b.3-6, 5b.3-7, task #91,
the `v1`→`v2` reservation fleet-convergence flip, and the lease/topology prerequisite for 5b.3-6. The
rest of ledger 12's open items are unchanged.
