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
