# Phase 7 — Prompt Ledger: Durable-Workflow Substrate (myelin-flow)

> Phase: 07-prompts (per-system file, Phase 7-A). The complete ordered set of implementation prompts that
> operationalize the entire Phase-6 roadmap for the durable-workflow shared system (the myelin-flow crate).
> Authored against the ledger template planning/07-prompts/00-ledger-overview.md §2 (every prompt follows that
> exact shape), the master sequencing planning/06-roadmaps/00-master-sequencing.md (bands M0..M6 + the gate
> invariant), this system's roadmap planning/06-roadmaps/shared/durable-workflow.md, and the frozen architecture
> planning/05-refined-shared-systems-architecture/durable-workflow.md + contract-index.md §9. Plain-text
> identifiers (no backticks-as-emphasis where the template forbids them). Markdown only; this file makes no
> commits. Global P-NNN ids are assigned by Phase 7-B (01-ledger-index.md) when these interleave with all other
> systems; the placeholder ids below (P-FLOW-NN) are local sequence markers the index will rewrite. Date:
> 2026-06-19.
>
> **Coverage of this system's roadmap (planning/06-roadmaps/shared/durable-workflow.md §2):** M2.1 (engine
> heartbeat), M2.2 (durable timers), M2.3 (durable signals + HITL + per-effect idem_key), M2.4 (long-park +
> reserve/settle + merge-queue frame), M3 (merge-gate consumer + resumable maintenance activities), M4 (X-1 seam
> end-to-end + CI-pipeline determinism), M5 (1M-timer scale, 30x surge, crypto-shred-to-history, restore-verify,
> E2E-2 spine), M6 (dogfood). Every milestone maps to >=1 prompt; floors are paired with their follow-on prompt.
>
> **A note every prompt below assumes (do not re-derive it):** myelin-flow is a BUILD, DBOS-class,
> Postgres-embedded engine (architecture §2, ADR-09) — NO new datastore. It sits inside the myelin-flow service's
> own Postgres and inherits the M0 substrate's outbox / idempotent-consumer / crypto-shred / tenant-partition /
> fail-static primitives. Every prompt's deliverable is an AppSpec + the workflow/activity/handler code the
> serve(AppSpec) harness wires (contract 1.1) — never a hand-rolled main. Every emit is via OutboxTx (contract
> 2.2, the no-raw-publish lint forbids any other path). Every table is tenant+region first column, RLS-enforced,
> per-tenant envelope-encrypted, crypto-shred-capable, and auto-registers as a PersonalDataHolder via the harness
> (contracts 1.4 / 10.1). All committed lints (contract 1.6) must be green for any prompt to be done; the
> contract-coverage scanner (M0) must pass for any contract row a prompt owns or consumes. Units, frozen once
> (architecture §5.1): timers/timeouts in seconds; timestamps RFC-3339 UTC; budgets/costs integer minor-units,
> never floats.

---

### P-FLOW-01 — myelin-flow data model + the workflow worker shell

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.1 (the data-model half of the engine heartbeat) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M2.1 "Work" bullet 1).
- **DEPENDS-ON.** The M0 substrate prompts that ship serve(AppSpec) (contract 1.1), the transactional outbox +
  idempotent-consumer template (contracts 2.2-2.5), the EventEnvelope (2.1), the forward-only-migration lint, and
  the contract-coverage scanner; and the M1 prompts that ship the tenant+region partition key (12.1) + the OLTP
  RLS tier (11.1). (The Phase 7-B index resolves these to concrete P-NNN ids; they MUST be merged before this
  prompt starts.)
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md (always) and external-insights/01-process-and-quality-doctrine.md §2 (order-by-non-negotiability
    — silent data loss outranks every feature) + §5 (the committed ratchet) + §1 (name-your-floors).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §3 (the data model — workflow_run,
    wf_history, wf_timer, wf_signal, wf_activity_attempt, wf_definition; carried verbatim from Phase-3 §3) + §2
    (BUILD/DBOS-class decision) + the header "A note every prompt below assumes" above.
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.1, 9.6 (the surface these tables
    back), 11.1 (OLTP/RLS), 12.1 (tenant+region partition), 2.2-2.5 (outbox), 1.1 (serve(AppSpec)).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.1 ("Work" bullet 1 — the data model carried verbatim) +
    §0 (the placement paragraph) + §1 (the consumed-rows table).
- **DELIVERABLE (what to build + exactly where in the repo).** In a new crate myelin-flow under the Cargo
  workspace: (a) the forward-only migrations creating workflow_run (state in {running, waiting, completed, failed,
  nondeterministic, terminated}; cursor; budget as RunBudget; causality columns correlation_id/causation_id/
  caused_by/depth; partition + lease_owner/lease_expires), wf_history (append-only journal; command_id
  deterministic from workflow position; UNIQUE(tenant, run_id, command_id); result_key_ref for the rare inline-PII
  result), wf_timer (bucket = epoch_minute(fire_at) + the partial index (bucket, partition) WHERE NOT fired),
  wf_signal (PK (tenant, run_id, signal_name, idem_key); payload_key_ref), wf_activity_attempt (idem_token ledger),
  wf_definition (versioned; a run pinned to wf_version at start) — all with tenant+region as the leading columns,
  RLS policies, per-tenant envelope encryption hooks, and input/state stored as references-not-payloads (refs, not
  payloads); (b) an AppSpec for the myelin-flow service that the harness boots (migrate + outbox relay + the
  consumer wiring slot, empty for now); (c) the PersonalDataHolder auto-registration over workflow_run/wf_history/
  wf_signal (locate/export/erase signatures stubbed to the crypto-shred path, full reach is FLOW-M5 / P-FLOW-13 —
  NAMED FLOOR, follow-on P-FLOW-13). This prompt ships NO algorithms yet (replay/timers/signals are the next
  prompts) — it is the schema + shell only.
- **CONTRACTS TO IMPLEMENT.** 9.6 PersonalDataHolder(workflow history) — owned, structural half (trait +
  auto-registration; crypto-shred reach is the named M5 follow-on). Consumes 1.1 serve(AppSpec), 11.1 OLTP/RLS,
  12.1 (tenant, region), 2.2-2.5 outbox (wired, exercised by later prompts).
- **GATE / DRILLS (quantified; must be green to call this done).** No FLOW drill greens at this prompt (the
  drills need the replay engine); the gate here is structural: the migrations apply forward-only (forward-only-
  migration lint green), the no-cross-db + no-untagged-personal-data lints green over the new crate (every PII
  column tagged), the tenant-predicate lint green (every table query carries the tenant predicate), and the
  service boots under serve(AppSpec) with liveness != readiness (a smoke test asserts the metrics-health port
  comes up). Green artifact: a dated CI run showing migrate-up + boot + the four named lints green.
- **TESTS (required).** Unit tests: command_id determinism from a workflow position; the UNIQUE(tenant, run_id,
  command_id) constraint rejects a duplicate journal row; the wf_signal PK rejects a duplicate (tenant, run_id,
  signal_name, idem_key); RLS denies a cross-tenant select on every table. CDC: the provider+consumer pair for
  9.6 (a DSR orchestrator consumer calling locate/export over an empty history). No mutation floor applies yet (no
  decision logic shipped).
- **DEFINITION OF DONE.** The crate compiles in the workspace; the six tables exist via forward-only migrations;
  the AppSpec boots under the harness with the three ports up; 9.6 is wired and CDC-covered; the named lints +
  the contract-coverage scanner are green and dated; the crypto-shred-reach FLOOR is named in writing with its
  follow-on (P-FLOW-13); untested surfaces (replay/timers/signals — not yet built) are recorded as such; the work
  is committed.
- **COMMIT.** Header `P-FLOW-01 M2: myelin-flow data model + worker shell`. Body lists: contracts 9.6 (structural)
  implemented, 1.1/11.1/12.1/2.2-2.5 consumed; lints greened; the crypto-shred-reach floor named with follow-on
  P-FLOW-13; the algorithm surfaces recorded as not-yet-built. Branch first if on the default branch. End with the
  workspace's required Co-Authored-By trailer.

---

### P-FLOW-02 — WfCtx core surface + deterministic replay + the outbox co-commit (FLOW-D1, FLOW-D5)

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.1 (the engine-heartbeat algorithm) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M2.1 "Work" bullets 2-4, 6).
- **DEPENDS-ON.** P-FLOW-01 (the data model + the worker shell). The M0 failure-injection harness prompt (so
  FLOW-D1/D5 are drillable). All merged before this starts.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (prove-it-or-it-isn't-real + the
    failure-injection harness + observability-is-part-of-the-pass).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §4 (the algorithms — §4.1 deterministic
    replay/recovery carried from Phase-3 §4.1; §4.4 activity execution + retry; §4.5 the outbox seam, no second
    emit path; §4.7 lease-based dispatch + crash recovery) + §5.1 (the WfCtx trait surface: activity, now, rand,
    emit) + §3.2 (wf_history as the journal source of truth).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.2 (WfCtx: activity/now/rand/emit;
    the deterministic surface), 9.1 (DurableExecutor start/describe/cancel), 2.2 (OutboxTx::emit — the only emit
    path), 1.8 (the telemetry set).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.1 (Work + Exit gate) + §4 (the drills table:
    FLOW-D1/D5).
  - planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows FLOW-D1 + FLOW-D5 (the exact thresholds) + testing-strategy/02-parts-contracts-and-mock-agents.md
    FLOW-G2 / FLOW-G3.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: (a) the WfCtx core
  implementation — activity<I,O> (journaled to wf_history under its deterministic command_id, retried per §4.4),
  now() and rand() as journaled side-markers, emit(EventDraft) via OutboxTx so the journal row and the outbox row
  co-commit in ONE transaction (no second emit path — the no-raw-publish lint forbids it); (b) DurableExecutor
  start/describe/cancel (contract 9.1, partial — signal lands P-FLOW-06); (c) the deterministic replay/recovery
  algorithm (§4.1): a worker leases a runnable run, replays wf_history to the cursor short-circuiting already-
  journaled commands (0 re-execution), and continues from the first un-journaled command; lease-based dispatch +
  crash recovery (§4.7, lease_owner/lease_expires with expiry re-lease); (d) the metrics-health telemetry
  (contract 1.8, architecture §5.4): runnable-run lag, replay rate, activity queue depth + retry + dead-letter.
- **CONTRACTS TO IMPLEMENT.** 9.1 DurableExecutor (start, describe, cancel) — owned. 9.2 WfCtx (activity, now,
  rand, emit) — owned. Consumes 2.2 OutboxTx::emit.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - FLOW-D1 (CI) — kill a worker at activity 5 of 10 mid-run; another re-leases, replays wf_history, resumes at
    step 6 with 0 re-executed side effects, 0 lost progress, exactly-once-in-effect. Green artifact: the replay-
    rate signal emitted + a 0-double-effect counter on the metrics port, dated CI.
  - FLOW-D5 (CI) — crash between journaling an activity's DB write and emitting its event; the journal row and the
    outbox row are committed together (one txn) — 0 ghost, 0 lost. Green artifact: the co-commit proof (the run is
    either fully journaled-and-emitted or neither), dated CI. (This is myelin-flow's face of the Tier-1
    silent-data-loss floor — BUS-D4-equivalent for the workflow journal; never weaken it.)
- **TESTS (required).** Unit tests: replay short-circuits a journaled command (assert 0 re-execution); a lease
  expiry re-leases to a new worker; emit and journal share one txn (inject a failure between them, assert atomic).
  Chained drill tests (preferred over single-handler tests — EI-01 §4): the FLOW-D1 scenario on the
  failure-injection harness (kill at 5/10, assert resume-at-6), the FLOW-D5 scenario (crash between journal and
  emit, assert atomicity). CDC: provider+consumer pair for 9.1 (start/describe/cancel) and 9.2
  (activity/now/rand/emit). Mutation floor: the replay/determinism core (testing-strategy/00 mandatory-core table)
  carries >= 90% cargo-mutants score over the replay short-circuit + the co-commit path.
- **DEFINITION OF DONE.** WfCtx core + DurableExecutor(start/describe/cancel) + replay/recovery + lease dispatch
  compile and run; FLOW-D1 and FLOW-D5 each emit a dated green artifact (PROVEN, not CLAIMED); the unit + drill +
  CDC tests pass; the replay-core mutation score >= 90%; all committed lints + the coverage scanner green; the
  divergence guard + the lint fixtures are explicitly recorded as not-yet-shipped (P-FLOW-03); the work is
  committed.
- **COMMIT.** Header `P-FLOW-02 M2: WfCtx core + deterministic replay + outbox co-commit`. Body lists contracts
  9.1/9.2 implemented; FLOW-D1 (replay-rate, 0 double-effect) + FLOW-D5 (co-commit proof) greened with measured
  numbers; the divergence-guard/lint-fixtures recorded as the next prompt. Co-Authored-By trailer.

---

### P-FLOW-03 — The divergence guard + the flow-determinism lint fixtures (FLOW-D2)

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.1 (the determinism guard + the lint proof) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M2.1 "Work" bullets 5, 7).
- **DEPENDS-ON.** P-FLOW-02 (the replay engine the guard sits inside). The M0 prompt that ships the
  flow-determinism lint itself (contract 1.6) — this prompt ships myelin-flow's red+green FIXTURES, not the lint.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §5 (the ratchet: a committed lint
    with a red-fixture that proves it rejects + a green-fixture that proves it admits; loud-never-swallowed) + §3
    (a red gate is information; never invert an assertion).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §2 (the determinism constraint + lint +
    replay-divergence guard, carried from Phase-3 §2.5) + §4.6 (versioning — a run pinned to wf_version) +
    "Changes vs Phase 3" item on the determinism guard.
  - planning/05-refined-shared-systems-architecture/contract-index.md row 1.6 (the flow-determinism lint, in the
    twelve-lint set) + 9.2 (WfCtx, the surface the lint enforces non-determinism flows through).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.1 (the flow-determinism fixtures bullet + the Exit gate
    FLOW-D2 line) + §1 (the lint row: "myelin-flow ships its red+green fixtures in M2.1").
  - testing-strategy/01-whole-system-e2e-and-drill-catalogue.md FLOW-D2 + testing-strategy/02 FLOW-G1.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: (a) the replay-divergence
  guard — when a replay encounters a journaled command that does not match the definition's command at that
  position (a divergent or wrong-version body), the run halts as state=nondeterministic and dead-letters; 0 silent
  divergence, never a silent continue; (b) the flow-determinism lint FIXTURES under the crate's fixtures dir: a
  RED fixture (a workflow body that reads SystemTime/IO/RNG outside WfCtx — MUST fail to compile under the lint)
  and a GREEN fixture (the same logic expressed via ctx.now()/ctx.rand()/ctx.activity() — MUST compile); both
  wired into CI loud-never-swallowed (no `|| true`); (c) the nondeterministic-halt-count telemetry on the metrics
  port (contract 1.8).
- **CONTRACTS TO IMPLEMENT.** 9.2 WfCtx — the determinism-enforcement half (the lint + the divergence guard are
  what make the surface's determinism contract real). No new owned row; this hardens 9.2.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - FLOW-D2 (CI) — replay against a divergent / wrong-version definition; the divergence guard halts the run as
    nondeterministic and dead-letters it; 0 silent divergence. Green artifact: the nondeterministic-halt-count
    signal increments by exactly the injected count, dated CI.
  - The flow-determinism lint green on BOTH fixtures: the red fixture fails to compile (proves the lint rejects),
    the green fixture compiles (proves it admits). Green artifact: a dated CI run showing red-rejects + green-admits.
- **TESTS (required).** Unit test: a position-mismatch on replay raises the halt, not a continue. Drill test: the
  FLOW-D2 scenario on the harness (wrong-version replay → halt + dead-letter, assert 0 silent divergence). The
  fixture pair is itself the lint test. Mutation floor: the determinism-guard path carries >= 90% cargo-mutants
  (testing-strategy/00 — "a surviving mutant = a silent double-effect on replay"); a mutant that turns the halt
  into a continue MUST be caught.
- **DEFINITION OF DONE.** The divergence guard halts-not-continues; the red+green fixtures are committed and CI
  proves the lint rejects the red and admits the green; FLOW-D2 emits its dated green artifact; the
  determinism-guard mutation score >= 90%; all lints + the coverage scanner green; the work is committed. (M2.1 is
  now fully covered across P-FLOW-01..03.)
- **COMMIT.** Header `P-FLOW-03 M2: divergence guard + flow-determinism fixtures`. Body lists the FLOW-D2
  greening (nondeterministic-halt count) + the lint red/green-fixture proof + the >= 90% guard mutation score.
  Co-Authored-By trailer.

---

### P-FLOW-04 — Durable timers at scale: the minute-bucket wheel + sleep_until/sleep_for (FLOW-D3 floor)

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.2 (durable timers at scale) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M2.2).
- **DEPENDS-ON.** P-FLOW-02 (the engine + the journal), P-FLOW-03 (a green M2.1 — the gate invariant: no M2.2
  work claimed done over a red FLOW-D1/D2/D5).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (quantified thresholds; the
    1x/10x/30x load generator) + §1 (name-your-floors: the 100k-timer floor with the 1M follow-on).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §3.3 (wf_timer; bucket =
    epoch_minute(fire_at); the partial index (bucket, partition) WHERE NOT fired — the SC-11 world-scale move) +
    §4.2 (the timer wheel: scan bucket <= now AND NOT fired, FOR UPDATE SKIP LOCKED, no calendar logic on the
    wheel) + §7.3 (the millions-of-timers scaling story) + §5.4 (timer-wheel-lag telemetry).
  - planning/05-refined-shared-systems-architecture/contract-index.md row 9.3 (the durable timer wheel;
    cheap disarm/re-arm of a precomputed fire_at without calendar logic) + 9.2 (WfCtx sleep_until/sleep_for).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.2 (Work + Exit gate FLOW-D3 floor) + §3 (the
    100k-timer floor row → the 1M+ cell-scale follow-on in M5) + §4 (FLOW-D3 floor threshold).
  - testing-strategy/01-whole-system-e2e-and-drill-catalogue.md FLOW-D3.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: (a) WfCtx sleep_until(t) and
  sleep_for(d) — each arms a durable wf_timer row (bucket = epoch_minute(fire_at)), parks the run holding no
  runtime, and fires effectively-once (a crash re-fires only the unfired); (b) the timer-wheel scan loop:
  bucket <= now AND NOT fired, FOR UPDATE SKIP LOCKED, with NO calendar logic on the wheel (a 30-day timer is
  never read until its minute); (c) the cheap SLA-timer disarm/re-arm (architecture §6.6, the Issues ask): a
  re-arm is a row update of fire_at + bucket; a disarm sets fired=true or deletes the row — millions re-arm at
  row-update cost, not wheel-scan cost; (d) the timer-wheel-lag telemetry (contract 1.8, the SC-11 health
  signal). NAMED FLOOR: this prompt proves the algorithm at 100k+ timers (six figures); the 1M+ cell-scale run +
  the per-cell timer-wheel-promotion threshold is the M5 follow-on P-FLOW-12.
- **CONTRACTS TO IMPLEMENT.** 9.3 Durable timer wheel — owned. 9.2 WfCtx sleep_until/sleep_for — owned (the
  timer half).
- **GATE / DRILLS (quantified; must be green to call this done).**
  - FLOW-D3 (SCHED, run at 100k+ timers in M2 — the floor): arm 100k+ durable timers plus a burst all due in one
    minute; the due timers fire WITHIN the tick budget; far-future timers cost ~nothing (never scanned); a crash
    re-fires only the unfired. 0 lost, 0 double-fire. Green artifact: the timer-wheel-lag signal stays within
    budget + a 0-lost / 0-dup counter, dated SCHED run. (Floor named: the seven-figure run is P-FLOW-12.)
- **TESTS (required).** Unit tests: a far-future timer is never read by the wheel scan (assert the partial index
  is used, not a full scan); a re-arm is a single row update (no wheel pollution); a disarm makes the timer never
  fire; effectively-once fire across a simulated crash. Drill test: the FLOW-D3-floor scenario on the
  failure-injection harness at 100k+ timers with the one-minute burst, asserting tick-budget + 0 lost / 0 dup.
- **DEFINITION OF DONE.** sleep_until/sleep_for + the bucketed wheel + cheap re-arm compile and run; FLOW-D3 at
  100k+ emits its dated green artifact within the tick budget with 0 lost / 0 double-fire; the unit + drill tests
  pass; lints + coverage scanner green; the 100k→1M floor is named with its follow-on P-FLOW-12; the work is
  committed.
- **COMMIT.** Header `P-FLOW-04 M2: durable timer wheel + sleep_until/sleep_for (100k floor)`. Body lists
  contract 9.3 + the 9.2 timer half; FLOW-D3-floor greened (timer-wheel-lag within budget, 0 lost/dup at 100k+);
  the 1M+ cell-scale floor named with follow-on P-FLOW-12. Co-Authored-By trailer.

---

### P-FLOW-05 — Durable signals + multi-day HITL wait + the per-effect idem_key (FLOW-D4)

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.3 (durable signals + multi-day HITL + per-effect idem_key) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M2.3).
- **DEPENDS-ON.** P-FLOW-04 (the engine + the timeout timer the wait needs). Co-built with the Agent Fabric M2
  prompt that owns the gated-tool set + EffectApi (the withheld-effect target) and the Notif humanise prompt
  (contract 7.3) that renders the card — those provide the consumer side of the round-trip but are not blockers
  for this engine's mechanics.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (prove-it: double-signal = one
    approval; a withheld effect makes 0 mutation) + §4 (chain mutations end-to-end, not single handlers).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §3.4 (wf_signal; PK (tenant, run_id,
    signal_name, idem_key) — the PK that makes idempotency true by construction) + §4.3 (the signal round-trip;
    state=waiting holds no runtime) + §6.3 (the HITL approval-card round-trip mechanics) + §6.4 (the per-effect
    idem_key rule — card_id single / card_id:effect_idx multi; partial approval well-defined; a declined effect
    withheld, AG-8) + §5.4 (signal-buffer-depth + oldest-unconsumed-wait-age telemetry).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.4 (durable signal, multi-day HITL;
    the approval/cancel waits) + 9.1 (DurableExecutor::signal idempotent on idem_key + the per-effect rule) + 7.3
    (humanise — consumed; the card is rendered there, the one templating surface).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.3 (Work + Exit gate FLOW-D4) + §4 (FLOW-D4 threshold).
  - testing-strategy/01-whole-system-e2e-and-drill-catalogue.md FLOW-D4 + the F-4 extended assertion in
    architecture §8.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: (a) DurableExecutor::signal
  (contract 9.1) — idempotent on idem_key via INSERT ... ON CONFLICT (tenant, run_id, signal_name, idem_key) DO
  NOTHING; (b) the per-effect idem_key rule (§6.4): idem_key = card_id for a single-effect card, idem_key =
  card_id ":" effect_idx for a multi/partial-approval card — a partial approval (approve 0 and 2, decline 1) is
  three independently-idempotent signals, each mapping to exactly one EffectApi::apply, a declined effect WITHHELD
  (returns Denied, never mutates, AG-8); (c) WfCtx wait_for_signal(name, timeout) — state=waiting holds no
  runtime; registers approval and cancel as names now (ci.result and job.done are registered names too but their
  long-park producer wiring lands P-FLOW-06); the timeout branch uses the durable timer (P-FLOW-04); (d) the HITL
  approval-card round-trip mechanics (§6.3): a gated tool → wait_for_signal("approval:<call>", timeout=window) →
  emits agent.approval.requested via the outbox (the card UX/visual is Chat+Agent-Fabric product work, OQ #1 —
  NOT this engine); (e) the signal-buffer-depth + oldest-unconsumed-wait-age telemetry (contract 1.8).
- **CONTRACTS TO IMPLEMENT.** 9.1 DurableExecutor::signal + the per-effect idem_key rule — owned. 9.4 durable
  signal (approval/cancel waits) — owned. 9.2 WfCtx wait_for_signal — owned. Consumes 7.3 humanise.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - FLOW-D4 (CI) — a gated workflow waits across a worker restart + a deploy; deliver the approval days later with
    a double-click; the workflow resumes, consumes the approval exactly ONCE, and runs (approved) or withholds
    (denied → 0 mutation) correctly. Green artifact: 1 consume on the signal-buffer-depth ledger; withhold = 0
    mutation, dated CI. (The partial-approval per-effect form is asserted in F-4's extended form, architecture §8,
    and at the subsystem face in CHAT-D10, M4 — referenced, not owned here.)
- **TESTS (required).** Unit tests: a double-delivered signal under the same idem_key inserts once (ON CONFLICT DO
  NOTHING); three per-effect keys (card_id:0/1/2) apply/decline independently; a declined effect returns Denied
  and makes 0 mutation; a wait that times out takes the timeout branch. Chained drill test (preferred — EI-01 §4):
  the FLOW-D4 scenario across a restart+deploy with a days-later double-click, asserting 1 consume + correct
  withhold. CDC: provider+consumer pair for 9.1 (signal + per-effect rule) and 9.4.
- **DEFINITION OF DONE.** signal + the per-effect rule + wait_for_signal + the HITL round-trip compile and run;
  FLOW-D4 emits its dated green artifact (1 consume, withhold = 0 mutation); the unit + drill + CDC tests pass;
  lints + coverage scanner green; the ci.result/job.done long-park wiring is recorded as the next prompt
  (P-FLOW-06); the card visual/data model is recorded as Chat+Agent-Fabric product work, not this engine; the work
  is committed.
- **COMMIT.** Header `P-FLOW-05 M2: durable signals + multi-day HITL + per-effect idem_key`. Body lists contracts
  9.1/9.4 + the 9.2 wait half; FLOW-D4 greened (1 consume, 0-mutation withhold); the per-effect rule; the
  long-park wiring deferred to P-FLOW-06. Co-Authored-By trailer.

---

### P-FLOW-06 — The SCHEDULE_AND_RUN_JOB long-park + reserve/settle + mint_run_token re-mint (FLOW-D6, FLOW-D7)

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.4 (the long-park idiom + reserve/settle; the engine half — the merge-queue body
  is P-FLOW-07) — roadmap file planning/06-roadmaps/shared/durable-workflow.md §2 (M2.4 "Work" bullets 1-3).
- **DEPENDS-ON.** P-FLOW-05 (durable signals — the long-park parks on a signal). The Identity M1 prompt that ships
  mint_run_token (contract 4.7, callable mid-workflow). The Storage M1 prompt that ships the reserve/settle cost
  gate + the wallet (contract 11.7). The Agent Fabric M2 prompt that ships the unified runner ToolHands::exec
  (contract 8.4) — the dispatch TARGET. NOTE the band gate: AG-D4 (the sandbox-escape GATE) is owned by Agent
  Fabric/CI and must be green before any SCHEDULE_AND_RUN_JOB dispatch executes untrusted code; this engine
  dispatches into the runner, it does not own the sandbox.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (reserve refuses when exhausted;
    in-flight never interrupted; loop tripwire) + external-insights/04-hard-problems.md §5 (untrusted-code
    execution — the runner is the sandbox, not this engine).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §4.9 (the SCHEDULE_AND_RUN_JOB
    long-park-completed-by-signal idiom — the four-step mechanics: dispatch as a journaled activity minting
    idem_token deterministic on command_id, reserve at dispatch, return; park on wait_for_signal("job.done",
    idem_key=idem_token) + a timeout timer; idempotent completion; settle) + §6.2 (mid-workflow mint_run_token
    re-mint on resume — token life == activity life, not the days-long workflow life) + §6.2 (loop safety:
    causal-depth ceiling + shared-root tripwire + bounded activity pool).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.2 (the SCHEDULE_AND_RUN_JOB idiom on
    WfCtx) + 9.4 (the job.done durable signal wait) + 9.5 (workflow↔agent mapping; reserve/settle bookend) + 11.7
    (reserve/settle cost gate — consumed) + 4.7 (mint_run_token mid-workflow re-mint — consumed) + 8.4 (the
    unified runner — the dispatch target, consumed).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.4 (Work bullets 1-3 + the Exit gate FLOW-D6/FLOW-D7 +
    the "Note on the band gate" AG-D4 paragraph) + §4 (FLOW-D6/D7 thresholds).
  - testing-strategy/01-whole-system-e2e-and-drill-catalogue.md FLOW-D6 + FLOW-D7 + the F-6 extended assertion
    (reserve-at-dispatch for the long-park, architecture §8).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: (a) the
  SCHEDULE_AND_RUN_JOB idiom (architecture §4.9) — an ordinary journaled activity that mints idem_token
  (deterministic from command_id so producer and consumer agree without coordination), stamps it on the
  JobSpec{kind: ci|agent}, hands the spec to the unified runner (ToolHands::exec, contract 8.4), RESERVES budget
  at dispatch (contract 11.7 — no balance → no dispatch), journals activity_completed{job_dispatched: true,
  idem_token}, and RETURNS immediately (frees the worker); then wait_for_signal("job.done", idem_key=idem_token)
  with a timeout timer bounding a vanished runner; (b) the reserve/settle bookend (contract 9.5/11.7): reserve at
  dispatch, settle on the job.done/ci.result signal, never interrupt in-flight, meter into the same wallet as a
  synchronous activity; (c) mint_run_token mid-workflow re-mint on resume (contract 4.7): a days-later resume
  re-mints a fresh short-lived attenuated per-run token (token life == activity life); (d) the loop-safety
  enforcement (§6.2): causal-depth ceiling + shared-root tripwire + bounded activity pool — an adversarial
  workflow→event→workflow loop is dropped/parked, never forked; (e) the reserve/settle-reject-rate +
  causal-depth-histogram telemetry (contract 1.8).
- **CONTRACTS TO IMPLEMENT.** 9.2 WfCtx (the SCHEDULE_AND_RUN_JOB idiom) — owned. 9.4 (the job.done wait) —
  owned. 9.5 workflow↔agent mapping (reserve/settle bookend) — owned. Consumes 11.7 reserve/settle, 4.7
  mint_run_token, 8.4 ToolHands::exec.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - FLOW-D6 (CI) — a runaway agent loop against a depleting wallet; a new spend-bearing activity (INCLUDING a
    SCHEDULE_AND_RUN_JOB dispatch) is refused at reserve when the wallet is exhausted; an in-flight one is NEVER
    interrupted (settles on completion). Green artifact: reserve-refusal count > 0 + 0-interrupt counter, dated CI.
  - FLOW-D7 (CI) — an adversarial workflow→event→workflow loop; the depth ceiling + the bus tripwire + the
    bounded activity pool stop it (drops/parks, never forks). Green artifact: the causal-depth signal stays under
    the ceiling + a 0-fork counter, dated CI.
- **TESTS (required).** Unit tests: idem_token is deterministic from command_id (producer and consumer derive the
  same key); a double-delivered job.done wakes the workflow once; a reserve against an empty wallet refuses the
  dispatch (the job never starts); an in-flight job is not interrupted by exhaustion; a re-mint on resume yields a
  short-lived token, not the workflow-lifetime token; the depth ceiling halts a self-feeding loop. Chained drill
  tests: FLOW-D6 (depleting wallet vs dispatch + in-flight) and FLOW-D7 (the adversarial loop) on the
  failure-injection harness. Mutation floor: the reserve/settle gate carries >= 90% cargo-mutants
  (testing-strategy/00 — "a surviving mutant = a runaway spend or a refused-when-funded"); a mutant that drops a
  reserve check or interrupts an in-flight job MUST be caught.
- **DEFINITION OF DONE.** SCHEDULE_AND_RUN_JOB + reserve/settle + the re-mint + loop safety compile and run;
  FLOW-D6 and FLOW-D7 each emit a dated green artifact; the unit + drill tests pass; the reserve/settle mutation
  score >= 90%; lints + coverage scanner green; it is recorded in writing that the dispatch path into the runner
  is GATED by AG-D4 (Agent Fabric/CI-owned) and that no long-park executes untrusted code until AG-D4 is green;
  the work is committed.
- **COMMIT.** Header `P-FLOW-06 M2: SCHEDULE_AND_RUN_JOB long-park + reserve/settle + token re-mint`. Body lists
  contracts 9.2/9.4/9.5 + 11.7/4.7/8.4 consumed; FLOW-D6 (reserve refusals, 0 interrupt) + FLOW-D7 (causal-depth,
  0 fork) greened; the AG-D4 gating on the dispatch path recorded; reserve/settle mutation score. Co-Authored-By
  trailer.

---

### P-FLOW-07 — The merge-queue durable workflow body, drilled in isolation against a mock ci.result (M2 exit)

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.4 (the merge-queue workflow frame — the durable-execution half of the X-1 seam,
  built-and-drilled-in-isolation) — roadmap file planning/06-roadmaps/shared/durable-workflow.md §2 (M2.4 "Work"
  bullet 4 + the merge-queue in-isolation drill in the Exit gate).
- **DEPENDS-ON.** P-FLOW-06 (the long-park idiom + reserve/settle the merge queue rides). NOTE: the real ci.result
  PRODUCER is CI (M4, contract 5.9) — this prompt ships the workflow side + the wait, drilled against a MOCK
  producer; the seam goes live end-to-end in P-FLOW-10 (M4).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §1 (name-your-floors: the
    built-in-isolation merge-queue is a floor whose follow-on is the X-1 seam end-to-end in M4) + §7
    (reconcile cross-component contracts at the plan layer before either side ships — the idem_token agreement).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §6.5 (the merge-queue durable workflow +
    the ci.result rollup wait — one workflow per target ref; dispatch required CI via SCHEDULE_AND_RUN_JOB; park on
    wait_for_signal("ci.result", idem_key=merge_attempt_id); on success-for-all-required-contexts merge + emit
    git.pr.merged via the outbox + settle; on failure dequeue with a humanised reason; the ci.result payload
    shape; ci.result-the-rollup-signal vs ci.check.updated-the-events split) + §4.9 (the long-park it rides).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.4 (the ci.result wait) + 5.9 (the
    Git↔CI CheckStatus seam — CI/Git own the DATA shape; this engine owns ONLY the durable-workflow mechanics; the
    merge queue is a workflow waking on the rollup ci.result; an untrusted_fork success is neutral until endorsed)
    + 7.3 (humanise — the dequeue reason).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.4 (the merge-queue body bullet + the in-isolation drill
    in the Exit gate) + §3 (the floor row: merge-queue-built-in-isolation → X-1 seam end-to-end at M4) + §1 (the
    5.9 consumed row).
  - testing-strategy/01-whole-system-e2e-and-drill-catalogue.md GIT-D10/CI-D8 (the M4 end-to-end the
    in-isolation drill anticipates).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: (a) the merge-queue durable
  workflow body — one workflow per target ref; for each queued PR: compute the speculative merge commit, dispatch
  the required CI via SCHEDULE_AND_RUN_JOB (reserve at dispatch, return), wait_for_signal("ci.result",
  idem_key=merge_attempt_id) parking with no runtime, with the timeout branch bounding a vanished CI run; on a
  success ci.result for ALL required contexts → perform the merge + emit git.pr.merged via the outbox + settle;
  on failure/error → dequeue the PR with a humanised reason (contract 7.3) and continue the queue; (b) a MOCK
  ci.result producer harness fixture (this engine does not own the real producer — that is CI, M4) so the body is
  drillable in isolation; (c) the signal-buffer/oldest-wait telemetry already covers the merge-queue backlog (no
  new metric). The merge-queue body consumes ONLY the durable-workflow mechanics; it imports the CheckStatus /
  ci.result data shape from the myelin-refs / CI contract crate (5.9) — it does not redefine it.
- **CONTRACTS TO IMPLEMENT.** 9.4 (the ci.result wait — owned, the durable half). Consumes 5.9 (the CheckStatus /
  ci.result data shape, awaiting CI's producer in M4) + 7.3 (humanise). Owns the merge-queue workflow body that
  the Git merge gate (M3) and the CI producer (M4) wire to.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The merge-queue in-isolation drill (CI) — against a MOCK ci.result producer: a double-delivered ci.result for
    a merge_attempt → exactly ONE wake (idempotent on idem_key); a vanished CI run → the timeout branch fires and
    bounds the wait; a success-for-all-required-contexts → exactly one merge + one git.pr.merged emit + one
    settle; a failure → one dequeue with a humanised reason, queue continues. Green artifact: 0 double-merge, 1
    wake per attempt, dated CI. (NAMED FLOOR: the full GIT-D10/CI-D8 end-to-end against CI's REAL producer is the
    M4 gate, follow-on P-FLOW-10.)
- **TESTS (required).** Unit tests: a double-delivered ci.result wakes once (idem_key); the timeout branch fires
  on a vanished run; only all-required-contexts-green merges; a merge emits exactly one git.pr.merged. Chained
  drill test: the in-isolation merge-queue scenario on the harness with the mock producer (double-delivery → one
  wake; timeout bounds the vanished runner; success → one merge; failure → one dequeue). CDC: the consumer half of
  5.9 (the merge queue consuming a mock ci.result) paired with CI's provider half landing in M4.
- **DEFINITION OF DONE.** The merge-queue workflow body compiles and runs; the in-isolation drill emits its dated
  green artifact (0 double-merge, 1 wake/attempt, timeout-bounded); the unit + drill tests pass; lints + coverage
  scanner green; it is recorded in writing that this is the merge-queue FLOOR (built against a mock producer) with
  the X-1-seam-end-to-end follow-on P-FLOW-10 (M4); the work is committed. (M2.4 — and the whole M2 engine surface
  for myelin-flow — is now covered across P-FLOW-04..07. The M2→M3 band gate is AG-D4, Agent Fabric/CI-owned; this
  system's M2 work is green here.)
- **COMMIT.** Header `P-FLOW-07 M2: merge-queue workflow body (in-isolation, mock ci.result)`. Body lists contract
  9.4 owned + 5.9/7.3 consumed; the in-isolation drill greened (0 double-merge, 1 wake/attempt); the
  merge-queue-in-isolation floor named with follow-on P-FLOW-10. Co-Authored-By trailer.

---

### P-FLOW-08 — Resumable maintenance activities + the cheap SLA-timer re-arm for Git/Issues (M3 support)

- **BAND.** M3.
- **ROADMAP MILESTONE.** FLOW-M3 (the merge-gate consumer half goes live with Git; no new engine — the resumable
  maintenance activities) — roadmap file planning/06-roadmaps/shared/durable-workflow.md §2 (M3).
- **DEPENDS-ON.** P-FLOW-07 (the full M2 engine, including the merge-queue body Git adopts). The Git M3 prompts
  that build the merge gate + check_status projection (consumers of the merge-queue body) — co-built; this prompt
  ships the myelin-flow contribution.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (a crash mid-repack replays to the
    un-journaled step — exercised, not asserted) + external-insights/04-hard-problems.md §1 (history-rewrite
    erasure as an audited follow-on).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §6.6 (resumable maintenance activities +
    cheap SLA-timer disarm/re-arm — Git GC/repack/bundle-gen/history-rewrite as resumable journaled activities or
    SCHEDULE_AND_RUN_JOB long-parks; the history-rewrite invalidation fan-out as a sequence of journaled
    activities; Issues re-arms a precomputed fire_at by updating wf_timer.fire_at + bucket, no calendar logic on
    the wheel) + §4.1 (replay-to-the-un-journaled-step) + §4.2 (the wheel).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.3 (the timer wheel + cheap
    disarm/re-arm) + 10.6 (the history-rewrite erasure-admin op — consumed by Git) + 11.2 (the trust-scoped cache
    namespaces the invalidation fan-out touches).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M3 (Work + Exit gate — no new FLOW drill is owed in M3;
    GIT-D9 is Git's gate; this engine's relevant assertion is that the merge-queue workflow holds no runtime
    across the wait, re-confirmed by the M2.4 in-isolation drill).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: (a) a reusable
  resumable-maintenance-activity helper so Git's GC / repack / bundle-gen / history-rewrite run as journaled
  activities (or SCHEDULE_AND_RUN_JOB long-parks for the heavy ones) on a workflow — a crash mid-repack replays to
  the un-journaled step (§4.1) with no re-executed side effect; (b) the history-rewrite invalidation fan-out
  expressed as a sequence of journaled activities (fork/mirror/clone-cache → the trust-scoped cache namespaces,
  contract 11.2); (c) the cheap SLA-timer disarm/re-arm surface for Issues (it is already in 9.3 from P-FLOW-04;
  this prompt confirms-and-tests it under the Git/Issues call sites and exposes it as a documented helper). NO new
  engine primitive — this is the application of the existing activity model (§4.4) + timer wheel (§4.2) to the M3
  producers' maintenance work.
- **CONTRACTS TO IMPLEMENT.** 9.3 (the cheap re-arm — confirmed under the Issues call site). Consumes 10.6
  (history-rewrite) + 11.2 (cache namespaces) — wires the call sites Git/Issues invoke.
- **GATE / DRILLS (quantified; must be green to call this done).** No NEW FLOW drill is owed in M3 (roadmap §2 M3
  Exit gate). The gate here: a maintenance-activity crash-and-resume test proves a crash mid-repack replays to the
  un-journaled step with 0 re-executed side effect (the FLOW-D1 property reused on a maintenance workflow); the
  cheap re-arm test proves a re-arm is a single row update (no wheel pollution). The merge-queue-holds-no-runtime
  assertion is re-confirmed by re-running the P-FLOW-07 in-isolation drill green. Green artifact: a dated CI run
  showing crash-mid-repack-resumes + re-arm-is-row-update + merge-queue-in-isolation re-green.
- **TESTS (required).** Unit tests: a journaled maintenance activity replays to the un-journaled step (0
  re-execution); a re-arm of a precomputed fire_at is a single row update; the invalidation fan-out is a journaled
  sequence (replays from the last journaled step). Drill test: a crash-mid-repack scenario on the harness
  asserting resume-with-no-side-effect.
- **DEFINITION OF DONE.** The resumable-maintenance helper + the invalidation fan-out + the cheap re-arm helper
  compile and run; the crash-mid-repack resume + the re-arm-is-row-update tests pass; the P-FLOW-07 in-isolation
  drill re-greens; lints + coverage scanner green; it is recorded that no new FLOW drill is owed in M3 (Git owns
  GIT-D9); the work is committed.
- **COMMIT.** Header `P-FLOW-08 M3: resumable maintenance activities + cheap SLA-timer re-arm`. Body lists
  contract 9.3 (re-arm confirmed) + 10.6/11.2 consumed; the crash-mid-repack-resumes proof + the merge-queue
  in-isolation re-green; no new FLOW drill owed in M3. Co-Authored-By trailer.

---

### P-FLOW-09 — The CI-pipeline-as-workflow body: WfCtx + SCHEDULE_AND_RUN_JOB + flow-determinism (CI-D9)

- **BAND.** M4.
- **ROADMAP MILESTONE.** FLOW-M4 (CI pipelines as durable workflows — the myelin-flow contribution to CI's
  pipeline body) — roadmap file planning/06-roadmaps/shared/durable-workflow.md §2 (M4 "Work (myelin-flow
  contribution)" — CI's pipeline-as-workflow uses WfCtx + SCHEDULE_AND_RUN_JOB + the flow-determinism lint).
- **DEPENDS-ON.** P-FLOW-07 (the full M2 engine + the long-park). The M3 Git prompts merged (the band gate: M4
  starts only after M3 green). The CI M4 prompts that own the ci.pipeline definition + the CheckStatus producer
  (co-built; this prompt provides the durable-execution substrate CI's pipeline body sits on). AG-D4 green (the CI
  runner is the unified sandbox).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (replay bit-identical; only
    journaled job.done feeds the body) + §5 (the flow-determinism lint is a committed gate).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §4.9 (SCHEDULE_AND_RUN_JOB for each long
    CI stage) + §2 (the flow-determinism constraint: no clock/RNG/IO outside WfCtx) + "Changes vs Phase 3" item 6
    (CI-pipeline-as-workflow stage/step granularity answered via SCHEDULE_AND_RUN_JOB + the unified-runner kind=ci
    job spec, X-6).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.2 (WfCtx + SCHEDULE_AND_RUN_JOB, the
    surface CI's pipeline body is written against) + 1.6 (the flow-determinism lint) + 5.9 (the CheckStatus
    producer CI owns — this engine provides the workflow substrate, not the producer) + 11.7 (reserve/settle on
    every CI stage).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M4 (Work + the Exit gate CI-D9 line: the ci.pipeline
    workflow body — no clock/RNG/IO outside WfCtx, flow-determinism lint passes, replay bit-identical, only
    journaled job.done feeds the body) + §4 (CI-D9 / CI-D1 rows — exercise this engine).
  - testing-strategy/01-whole-system-e2e-and-drill-catalogue.md CI-D1 + CI-D9 (CI/Git-owned, exercising this
    engine's long-park + idempotent-signal mechanics).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: the documented,
  test-covered CI-pipeline-as-workflow PATTERN that CI's M4 build uses — a deterministic WfCtx workflow body whose
  every long stage is a SCHEDULE_AND_RUN_JOB dispatch (kind=ci) into the unified runner, with reserve/settle on
  each stage, and the flow-determinism lint applied to the body (no clock/RNG/IO outside WfCtx). This prompt does
  NOT build CI's pipeline definitions or the CheckStatus producer (those are CI's M4 deliverable); it builds the
  myelin-flow substrate + a reference ci.pipeline workflow fixture that proves the determinism + replay-bit-
  identical + only-journaled-job.done properties, so CI's prompt can build its real pipelines on a proven base.
- **CONTRACTS TO IMPLEMENT.** 9.2 (WfCtx + SCHEDULE_AND_RUN_JOB — the CI-pipeline surface) — owned. Consumes 5.9
  (CI's CheckStatus producer, awaited) + 11.7 (reserve/settle per stage) + 1.6 (the flow-determinism lint).
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CI-D9 (CI, exercises this engine) — the ci.pipeline workflow body (the reference fixture): no clock/RNG/IO
    outside WfCtx (flow-determinism lint passes); a replay is bit-identical; ONLY a journaled job.done feeds the
    body. Green artifact: replay-bit-identical proof + flow-determinism lint green on the body, dated CI.
  - CI-D1 (CI, exercises this engine) — kill the runner + the control plane mid-run; the run resumes (replay +
    SCHEDULE_AND_RUN_JOB idempotent re-dispatch); effectively-once; 0 lost runs, 0 double-deploys, 0 duplicate
    publishes. Green artifact: replay-rate + 0-double-effect on the reference pipeline, dated CI.
- **TESTS (required).** Unit tests: the reference pipeline body compiles under the flow-determinism lint (and a
  body with a raw SystemTime read fails the lint); a replay of the body is bit-identical; an idempotent
  re-dispatch after a kill produces one job, not two. Chained drill tests: CI-D1 (runner + control-plane kill →
  replay + idempotent re-dispatch) and CI-D9 (determinism + replay-bit-identical) on the harness against the
  reference pipeline fixture.
- **DEFINITION OF DONE.** The CI-pipeline-as-workflow substrate + the reference fixture compile and run; CI-D9 and
  CI-D1 each emit a dated green artifact against the reference pipeline; the unit + drill tests pass; the
  flow-determinism lint is green on the body and rejects a non-deterministic one; lints + coverage scanner green;
  it is recorded that CI's real pipeline definitions + the CheckStatus producer are CI's M4 deliverable (this
  prompt ships the substrate they sit on); the work is committed.
- **COMMIT.** Header `P-FLOW-09 M4: CI-pipeline-as-workflow substrate + reference fixture`. Body lists contract
  9.2 (CI-pipeline surface) + 5.9/11.7/1.6 consumed; CI-D9 (replay-bit-identical, lint green) + CI-D1 (replay +
  idempotent re-dispatch) greened; CI's real pipelines recorded as CI's M4 deliverable. Co-Authored-By trailer.

---

### P-FLOW-10 — The X-1 seam end-to-end: the merge-queue long-park wakes on the real ci.result (GIT-D10/CI-D8)

- **BAND.** M4.
- **ROADMAP MILESTONE.** FLOW-M4 (the X-1 seam end-to-end — the merge-queue floor's follow-on against CI's REAL
  ci.result producer) — roadmap file planning/06-roadmaps/shared/durable-workflow.md §2 (M4 thesis + Exit gate
  GIT-D10/CI-D8).
- **DEPENDS-ON.** P-FLOW-07 (the merge-queue body built in isolation — this prompt is its named follow-on),
  P-FLOW-09 (the CI-pipeline substrate). The CI M4 prompt that ships the real CheckStatus/ci.result PRODUCER
  (contract 5.9) and the Git M3 prompt that ships the merge gate consuming the merge-queue body. AG-D4 green.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §7 (reconcile cross-component
    contracts — the idem_token agreement across the scheduler boundary) + §3 (0 double-merge is the quantified
    gate; never weaken it).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §6.5 (the merge-queue durable workflow +
    the ci.result rollup wait — now wired to the REAL producer; the ci.result-rollup vs ci.check.updated-events
    split) + §4.9 (the long-park) + the "Changes vs Phase 3" item 3 (the merge-queue + ci.result wait, wiring
    pinned).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 5.9 (the Git↔CI CheckStatus seam —
    keyed (commit_oid, context), last-writer-wins by run_attempt, untrusted_fork success neutral until endorsed;
    CI is the producer, Git the gate, this engine the durable-workflow mechanics) + 9.4 (the ci.result wait).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M4 (Exit gate GIT-D10/CI-D8 + the note that the
    durable-execution half is this engine's long-park + wait_for_signal) + §3 (the floor row:
    merge-queue-in-isolation → X-1 seam end-to-end at M4, trigger = CI's CheckStatus producer ships).
  - testing-strategy/01-whole-system-e2e-and-drill-catalogue.md GIT-D10/CI-D8 (the X-1 seam end-to-end).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: replace the P-FLOW-07 MOCK
  ci.result producer with the REAL wiring — the merge-queue workflow now subscribes to and wakes on CI's live
  ci.result rollup signal (contract 5.9), keyed by merge_attempt_id, idempotently. No new engine primitive: this
  is the floor's follow-on (the mock → the real producer), proving the long-park + idempotent-signal mechanics
  end-to-end with Git's merge gate and CI's producer. The CheckStatus/ci.result data shape is imported from the
  5.9 contract crate (CI-owned); this engine owns only the durable-workflow half.
- **CONTRACTS TO IMPLEMENT.** 9.4 (the ci.result wait — now end-to-end). Consumes 5.9 (CI's real CheckStatus /
  ci.result producer). Closes the durable-execution half of the X-1 seam.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - GIT-D10 / CI-D8 (CI) — push → ci.check.updated per context → green → merge; out-of-order / re-delivered
    ci.check.updated → run_attempt supersession; a fork self-green is neutral for gating; the merge-queue workflow
    wakes on ci.result idempotently — 0 double-merge, 0 spurious unblocks. Green artifact: the correct merge row +
    a 0-double-merge counter (merge-count == 1 per attempt), dated CI. (This is the most load-bearing
    cross-subsystem contract; the durable-execution half is this engine's long-park + wait_for_signal. Never
    invert the 0-double-merge assertion to pass.)
- **TESTS (required).** Unit tests: a doubly-delivered ci.result wakes the merge queue once (idem_key =
  merge_attempt_id); a fork self-green ci.result does not unblock the merge (neutral until endorsed — the gating
  rule is CI/Git-owned but this engine must not wake-and-merge on it); out-of-order ci.check.updated supersedes by
  run_attempt before the rollup. Chained drill test: GIT-D10/CI-D8 end-to-end on the harness (push → checks →
  ci.result → merge), asserting 0 double-merge across re-delivery + restart. CDC: the consumer half of 5.9 (the
  merge queue) now paired with CI's real provider half — the contract-coverage scanner must show both green.
- **DEFINITION OF DONE.** The merge queue wakes on the real ci.result; GIT-D10/CI-D8 emits its dated green
  artifact (0 double-merge, merge-count == 1/attempt) across re-delivery and restart; the unit + drill tests pass;
  the 5.9 provider+consumer CDC pair is green; lints + coverage scanner green; it is recorded that the
  merge-queue-in-isolation FLOOR (P-FLOW-07) is now promoted to the seam end-to-end (the floor is closed); the
  work is committed.
- **COMMIT.** Header `P-FLOW-10 M4: X-1 seam end-to-end (merge-queue wakes on real ci.result)`. Body lists
  contract 9.4 end-to-end + 5.9 consumed (CDC pair green); GIT-D10/CI-D8 greened (0 double-merge); the
  merge-queue floor promoted from in-isolation to end-to-end. Co-Authored-By trailer.

---

### P-FLOW-11 — Crypto-shred reaching history + restore-verify to a consistent point (FLOW-D9, FLOW-D10)

- **BAND.** M5.
- **ROADMAP MILESTONE.** FLOW-M5 (world-scale hardening — the durability + erasure half: crypto-shred reach +
  restore-verify) — roadmap file planning/06-roadmaps/shared/durable-workflow.md §2 (M5 "Work — world-scale
  hardening" bullets crypto-shred + restore-verify).
- **DEPENDS-ON.** P-FLOW-10 (the X-1 seam end-to-end — M4 green; the band gate). The Storage M1 restore-verify
  CI job (STOR-D1/D2, contract 11.5) and the KMS/per-subject-DEK crypto-shred substrate (contract 11.3/11.4) it
  builds on. P-FLOW-01 (the references-not-payloads structural floor whose crypto-shred reach this prompt
  completes — this is P-FLOW-01's named follow-on).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (restore to a consistent point;
    crypto-shred unrecoverable incl. backups) + external-insights/04-hard-problems.md §1 (erasure-vs-immutability;
    references-not-payloads + crypto-shred + tombstone).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §4.8 (GDPR erasure on history via
    references-not-payloads + crypto-shred + tombstone; structure preserved) + §5.5 (PersonalDataHolder over
    workflow_run/wf_history/wf_signal; payload_key_ref crypto-shred) + §3.2/§3.4 (result_key_ref / payload_key_ref,
    the per-subject-DEK envelope) + §7 (restore + cross-seam integrity, F-10).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.6 (PersonalDataHolder + replay —
    crypto-shred reach now complete) + 11.5 (backup/restore + restore-verify — consumed) + 11.3/11.4 (KMS
    hierarchy + per-subject DEK — consumed).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M5 (the FLOW-D9 + FLOW-D10 bullets) + §4 (FLOW-D9 / FLOW-D10
    thresholds) + §3 (no floor remains on crypto-shred — this completes the P-FLOW-01 structural floor).
  - testing-strategy/01-whole-system-e2e-and-drill-catalogue.md FLOW-D9 + FLOW-D10.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: (a) complete the
  PersonalDataHolder erase path (contract 9.6) — erasing a subject with inline-PII history/signal rows destroys
  the per-subject DEK so the result_key_ref / payload_key_ref ciphertext is unrecoverable INCLUDING in backups,
  tombstones the references, and PRESERVES the structure (the journal shape survives, the PII does not); the
  crypto-shred-lag telemetry (contract 1.8); (b) the restore-verify integration with Storage's restore (contract
  11.5): after a restore to a consistent point, in-flight runs resume, and store ↔ outbox offsets ↔ referenced
  rows are at ONE consistent point — no run pointing at a vanished result. This is the named follow-on to
  P-FLOW-01's structural-floor crypto-shred reach.
- **CONTRACTS TO IMPLEMENT.** 9.6 PersonalDataHolder + replay — owned, now COMPLETE (crypto-shred reach). Consumes
  11.5 restore-verify, 11.3/11.4 KMS + per-subject DEK.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - FLOW-D9 (SCHED) — erase a subject with inline-PII history/signal rows; the keys are destroyed (unrecoverable
    including in backups), the references tombstoned, the structure preserved. Green artifact: the crypto-shred-lag
    signal + a 0-recoverable-PII assertion (including a backup-restore-then-read attempt that fails to decrypt),
    dated SCHED.
  - FLOW-D10 (SCHED) — restore the myelin-flow Postgres to a consistent point; in-flight runs resume; store ↔
    outbox offsets ↔ referenced rows are at one consistent point; no run points at a vanished result. Green
    artifact: the restore-verify signal + a consistent-point assertion, dated SCHED.
- **TESTS (required).** Unit tests: erasing a subject destroys the per-subject DEK and the inline-PII ciphertext
  is undecryptable; the journal structure survives the erase (replay still works, the PII is a tombstone). Chained
  drill tests: FLOW-D9 (erase → 0 recoverable incl. a backup-restore attempt) and FLOW-D10 (restore →
  consistent-point resume) on the harness. Mutation floor: the crypto-shred / erasure key-selection path carries
  >= 95% cargo-mutants (testing-strategy/00 — "a surviving mutant = PII that survives erasure"); a mutant that
  selects the per-tenant DEK instead of the per-subject DEK, or skips the shred, MUST be caught.
- **DEFINITION OF DONE.** The crypto-shred reach + the restore-verify integration compile and run; FLOW-D9 and
  FLOW-D10 each emit a dated green artifact; the unit + drill tests pass; the crypto-shred mutation score >= 95%;
  lints + coverage scanner green; the P-FLOW-01 crypto-shred-reach FLOOR is recorded as now closed; the work is
  committed.
- **COMMIT.** Header `P-FLOW-11 M5: crypto-shred-to-history + restore-verify`. Body lists contract 9.6 completed +
  11.5/11.3/11.4 consumed; FLOW-D9 (0 recoverable incl. backups) + FLOW-D10 (consistent-point restore) greened;
  the crypto-shred mutation score; the P-FLOW-01 floor closed. Co-Authored-By trailer.

---

### P-FLOW-12 — World-scale: the 1M+ timer cell-scale run + the 30x agent-workflow surge (FLOW-D3 full, FLOW-D8)

- **BAND.** M5.
- **ROADMAP MILESTONE.** FLOW-M5 (world-scale hardening — the scale half: 1M+ timers + the 30x surge with lane
  shedding; the named timer floor's follow-on) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M5 "Work — world-scale hardening" bullets the 1M+ timer run
  + the 30x surge + the per-cell timer-wheel-promotion threshold).
- **DEPENDS-ON.** P-FLOW-04 (the timer wheel + the 100k floor — this prompt is its named follow-on), P-FLOW-06
  (reserve/settle + the per-surface shed budgets the surge sheds against), P-FLOW-10 (M4 green). The M0
  failure-injection harness's 30x load generator + per-surface storm profiles (contract 1.11 shed order).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (the 1x/10x/30x load generator;
    the protected-human-lane shed order; observability is part of the pass).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §7.3 (the SC-11 millions-of-timers case +
    the per-cell timer-wheel-promotion threshold, OQ #5) + §7.6 (bounded everything with the principal-aware shed
    order) + §7 (the long-park does not change the scaling story — a parked wait is a row, not a runtime).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.3 (the timer wheel at scale) + 1.11
    (the protected-human-lane shed order — consumed) + 1.8 (timer-wheel-lag + shed-counts telemetry).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M5 (the FLOW-D3 full + FLOW-D8 bullets + the per-cell
    timer-wheel-promotion threshold measured) + §3 (the 100k → 1M floor row + the trigger = measured due-now rate,
    OQ #5) + §4 (FLOW-D3 full / FLOW-D8 thresholds).
  - testing-strategy/01-whole-system-e2e-and-drill-catalogue.md FLOW-D3 (full) + FLOW-D8.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: (a) the prod-scale timer
  run + any worker-sharding/wheel tuning the 1M+ scale needs (the algorithm is unchanged from P-FLOW-04 — this
  proves it at seven figures and MEASURES the per-cell timer-wheel-promotion threshold, OQ #5: the due-now rate at
  which the PG-indexed wheel yields to a dedicated scheduling tier; the threshold is recorded in the thresholds
  file, the dedicated tier itself is a named follow-on if the measured rate demands it); (b) the per-surface shed
  enforcement under the 30x agent-workflow surge (contract 1.11): the human-initiated lane holds within budget,
  the agent lane sheds with 429 + Retry-After, other tenants are unaffected; the shed-counts/lane telemetry
  (contract 1.8). This is the named follow-on to P-FLOW-04's 100k-timer floor.
- **CONTRACTS TO IMPLEMENT.** 9.3 (the timer wheel at cell scale — the floor follow-on). Consumes 1.11 (the shed
  order).
- **GATE / DRILLS (quantified; must be green to call this done).**
  - FLOW-D3 full (SCHED, cell scale) — arm 1M+ durable timers + a burst due in one minute; the due timers fire
    within the tick budget; far-future timers are ~free; a crash re-fires only the unfired. 0 lost, 0 double-fire.
    Green artifact: the timer-wheel-lag signal within budget at 1M+ + a 0-lost / 0-dup counter, dated SCHED; the
    per-cell promotion threshold recorded in the thresholds file.
  - FLOW-D8 (SCHED) — a 30x surge of agent-initiated workflows; the human-initiated lane HOLDS, the agent lane
    SHEDS (429 + Retry-After), other tenants are unaffected. Green artifact: the shed-counts/lane signal showing
    the agent lane shedding while the human lane stays within budget, dated SCHED.
- **TESTS (required).** Drill tests (these are scheduled, not CI-cheap): FLOW-D3 at 1M+ on the harness asserting
  tick-budget + 0 lost/dup; FLOW-D8 at 30x on the harness asserting human-lane-holds + agent-lane-sheds +
  cross-tenant-0-impact. Unit tests: the shed order routes a human-initiated workflow ahead of an agent-initiated
  one under saturation; a 429 carries a Retry-After.
- **DEFINITION OF DONE.** The 1M+ timer run + the 30x surge shed enforcement run and pass; FLOW-D3 full and
  FLOW-D8 each emit a dated green artifact; the per-cell timer-wheel-promotion threshold is measured and recorded
  in the thresholds file (with the dedicated-scheduling-tier named as a follow-on IF the measured rate demands
  it); the drill + unit tests pass; lints + coverage scanner green; the P-FLOW-04 100k-timer FLOOR is recorded as
  now closed; the work is committed.
- **COMMIT.** Header `P-FLOW-12 M5: 1M+ timer cell-scale run + 30x agent-workflow surge`. Body lists contract 9.3
  at cell scale + 1.11 consumed; FLOW-D3 full (1M+ within tick budget) + FLOW-D8 (human lane holds, agent sheds)
  greened; the measured promotion threshold; the 100k floor closed. Co-Authored-By trailer.

---

### P-FLOW-13 — The E2E-2 flagship: the durable-workflow + HITL spine across the kill + days-later approval

- **BAND.** M5.
- **ROADMAP MILESTONE.** FLOW-M5 (the whole-system E2E wedge — myelin-flow's role in E2E-2, the agent-native
  flagship) — roadmap file planning/06-roadmaps/shared/durable-workflow.md §2 (M5 "Work — the E2E wedge").
- **DEPENDS-ON.** P-FLOW-11 (crypto-shred + restore-verify), P-FLOW-12 (the scale drills) — myelin-flow's M5
  hardening green. The Agent Fabric, CI, Issues, Chat, Git M3/M4 prompts (E2E-2 spans CI, Agent, Workflow, Issues,
  Chat, Git, Id, Notif, Storage); this prompt owns the durable-workflow + HITL SPINE of the scenario, co-built
  with the owning subsystems' E2E prompts.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §4 (chain mutations end-to-end; drive
    the real thing) + §3 (exactly-once across a kill is the quantified gate).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §8 (F-4 extended — the long-park + the
    per-effect idem_key asserted across a restart+deploy with a days-later double-click) + §6.2 (mid-workflow
    token re-mint on resume) + §6.5 (the merge-queue wakes on ci.result).
  - planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    E2E-2 (CI-fail → triage agent → issue → chat → fix-PR — the assert: 0 effect outside the intersection; 0
    mutation before approval; exactly-once approval + merge across a kill; reserve/settle balanced; merge-count ==
    1) + the E2E-2 narrative §lines describing the durable-workflow resume (FLOW-D4), the exactly-once consume, the
    token re-mint (4.7), the merge-applies-once (FLOW-D1), the merge-queue wake on ci.result (X-1).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M5 (the E2E wedge paragraph — myelin-flow's spine: a
    failing CI run wakes a mock triage agent; the agent run is a workflow; open_pr/git.merge is HITL-gated; the
    Agent+Workflow services are killed mid-ack_window; approval arrives days later (double-click) → the workflow
    resumes, consumes once, re-mints the run token, the merge applies once; the fix-PR CI goes green → the
    merge-queue wakes on ci.result idempotently and merges) + §4 (the E2E-2 row).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.1/9.4 (signal + the wait), 4.7
    (re-mint), 5.9 (the ci.result the merge-queue wakes on), 11.7 (reserve/settle parity).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow (the E2E test harness scope):
  the durable-workflow + HITL SPINE of E2E-2 as an end-to-end scenario test against a full cell with MOCK agents
  (VISION §3 — no real agents during development) — a failing CI run wakes a mock triage agent whose run is a
  myelin-flow workflow; the open_pr / git.merge effect is HITL-gated (wait_for_signal); the Agent + Workflow
  services are KILLED mid-ack_window; the approval arrives days later as a double-click; the workflow RESUMES
  (FLOW-D4), consumes the approval EXACTLY ONCE, re-mints the run token on resume (contract 4.7), and the merge
  applies ONCE (FLOW-D1, no double-effect); the fix-PR's CI goes green → the merge-queue workflow wakes on
  ci.result idempotently (X-1) and merges; reserve/settle is balanced across the whole run. This prompt owns the
  myelin-flow assertions in the shared E2E-2 scenario; the other subsystems' E2E prompts own their faces.
- **CONTRACTS TO IMPLEMENT.** No new owned contract — this exercises 9.1/9.4 (signal + wait), 4.7 (re-mint), 5.9
  (the merge-queue wake), 9.5/11.7 (reserve/settle parity) end-to-end in the flagship scenario.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - E2E-2 (SCHED), the durable-workflow + HITL spine — the agent run resumes after the kill, consumes the
    days-later approval exactly once (1 HITL withhold→approve→apply ledger entry), re-mints the run token on
    resume, the merge applies exactly once (merge-count == 1), the merge-queue wakes on ci.result idempotently,
    and reserve/settle is balanced (the parity assertion). Green artifact: the deterministic run trace + the
    HITL withhold→approve→apply ledger + reserve/settle parity + merge-count == 1, dated SCHED. (Never invert the
    merge-count == 1 or the exactly-once assertions to pass.)
- **TESTS (required).** The E2E-2 scenario test on the full-cell harness with mock agents, chaining the mutations
  (CI-fail → agent workflow → HITL gate → kill → days-later double-click → resume → re-mint → merge-once →
  fix-PR-CI-green → merge-queue-wake) — preferred over any single-handler test (EI-01 §4, the scenario IS a
  sequence property). Assert: 0 mutation before approval; exactly-once approval + merge across the kill;
  reserve/settle balanced; merge-count == 1.
- **DEFINITION OF DONE.** The E2E-2 durable-workflow + HITL spine scenario runs against a full cell with mock
  agents and emits its dated green artifact (run trace + HITL ledger + reserve/settle parity + merge-count == 1);
  the scenario test passes across the kill + the days-later approval; lints + coverage scanner green; any part of
  the scenario owned by another subsystem is recorded as such (this prompt owns only the workflow + HITL spine);
  the work is committed. (M5 for myelin-flow is now covered across P-FLOW-11..13.)
- **COMMIT.** Header `P-FLOW-13 M5: E2E-2 durable-workflow + HITL spine (agent-native flagship)`. Body lists the
  contracts exercised (9.1/9.4/4.7/5.9/9.5/11.7); E2E-2 spine greened (exactly-once across the kill, merge-count
  == 1, reserve/settle balanced); the cross-subsystem faces recorded as their owners'. Co-Authored-By trailer.

---

### P-FLOW-14 — Dogfooding: Myelin's own pipelines / merge queue / SLA timers as myelin-flow workflows

- **BAND.** M6.
- **ROADMAP MILESTONE.** FLOW-M6 (dogfooding — no new engine work; the self-hosting truth-up) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M6).
- **DEPENDS-ON.** P-FLOW-13 (M5 green — the gate invariant: you do not dogfood real team data onto a substrate
  whose restore-verify + DSAR fan-out are not green). The M6 dogfood prompts that migrate the Myelin monorepo onto
  Myelin git hosting + stand up the self-hosting CI graph.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §1 (code-wins-over-docs; the truth-up
    pass — date every status note; a claim that outlives its verification misleads the next agent) + §5 (the
    mandatory-core mutation gate now runs as a Myelin CI job on every Myelin commit).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §1 (the engine paths the dogfood loop
    exercises) — no new architecture; this is the dogfood application.
  - planning/06-roadmaps/shared/durable-workflow.md §2 M6 (Myelin's own CI pipelines, merge queue, SLA timers, and
    any agent runs become myelin-flow workflows; the dogfood loop exercises every engine path on the platform's
    own commits; the gate is the self-hosting CI graph green + the truth-up pass confirming 0 red earlier FLOW
    gates).
  - planning/06-roadmaps/00-master-sequencing.md §2 M6 (the dogfood band) + §4 (the M6 done-bar: 0 red earlier-band
    gate; the truth-up pass).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow + the Myelin self-hosting
  config: wire Myelin's own CI pipelines, merge queue, and SLA timers to run as myelin-flow workflows on the
  self-hosting platform (the dogfood loop), so every engine path (replay, timers, signals, the long-park, the
  merge-queue wake) is exercised on the platform's own commits; and run the myelin-flow truth-up pass — re-confirm
  that every PROVEN FLOW drill row (FLOW-D1..D10, the E2E-2 spine) rests on a DATED green artifact, not a doc
  claim, and that no later-band FLOW gate is red (the gate invariant end-to-end). Record any drift between the
  code and these prompts (code wins; fix the prompt/doc) in the gap report. No new engine primitive.
- **CONTRACTS TO IMPLEMENT.** None new — the dogfood loop exercises the already-shipped 9.1-9.6 surface on Myelin's
  own commits.
- **GATE / DRILLS (quantified; must be green to call this done).** The self-hosting CI graph is green on the
  platform's own commits (Myelin's CI pipelines run as myelin-flow workflows; the merge queue merges Myelin's own
  PRs; the SLA timers fire on Myelin's own issues) AND the truth-up pass confirms every FLOW drill (FLOW-D1..D10 +
  the E2E-2 spine) has a dated green artifact and 0 red earlier-band FLOW gate. Green artifact: the dated
  self-hosting-CI-graph-green run + the truth-up scorecard showing every FLOW row PROVEN-with-a-dated-artifact.
- **TESTS (required).** The dogfood loop IS the test (the real thing exercised — EI-01 §4): assert Myelin's own CI
  pipeline runs as a workflow end-to-end, Myelin's own merge queue merges a real Myelin PR exactly once, and a
  Myelin SLA timer fires on a real Myelin issue. The truth-up pass re-runs the FLOW drill scorecard.
- **DEFINITION OF DONE.** Myelin's own pipelines / merge queue / SLA timers run as myelin-flow workflows on the
  self-hosting platform; the self-hosting CI graph is green on the platform's own commits; the truth-up pass
  confirms every FLOW drill rests on a dated green artifact with 0 red earlier-band FLOW gate; any code-vs-doc
  drift is recorded (code wins, doc fixed); the work is committed.
- **COMMIT.** Header `P-FLOW-14 M6: dogfood Myelin's pipelines/merge-queue/SLA-timers as myelin-flow workflows`.
  Body lists: the dogfood loop wired; the self-hosting CI graph green on Myelin's own commits; the truth-up pass
  result (every FLOW drill PROVEN-with-a-dated-artifact, 0 red earlier gate); any drift recorded. Co-Authored-By
  trailer.

---

## Coverage check (this system's roadmap → prompts)

| Roadmap milestone (planning/06-roadmaps/shared/durable-workflow.md §2) | Prompt(s) | Band |
|---|---|---|
| FLOW-M2.1 — engine heartbeat (data model; WfCtx core + replay + co-commit; divergence guard + lint fixtures) | P-FLOW-01, P-FLOW-02, P-FLOW-03 | M2 |
| FLOW-M2.2 — durable timers at scale (100k floor) | P-FLOW-04 | M2 |
| FLOW-M2.3 — durable signals + multi-day HITL + per-effect idem_key | P-FLOW-05 | M2 |
| FLOW-M2.4 — long-park + reserve/settle + token re-mint; merge-queue frame (in isolation) | P-FLOW-06, P-FLOW-07 | M2 |
| FLOW-M3 — merge-gate consumer goes live with Git; resumable maintenance activities | P-FLOW-08 | M3 |
| FLOW-M4 — CI-pipeline-as-workflow substrate; the X-1 seam end-to-end | P-FLOW-09, P-FLOW-10 | M4 |
| FLOW-M5 — crypto-shred + restore-verify; 1M+ timers + 30x surge; E2E-2 spine | P-FLOW-11, P-FLOW-12, P-FLOW-13 | M5 |
| FLOW-M6 — dogfood + truth-up | P-FLOW-14 | M6 |

**Floors paired with follow-ons (name-your-floors):** P-FLOW-01 crypto-shred-reach floor → P-FLOW-11; P-FLOW-04
100k-timer floor → P-FLOW-12; P-FLOW-07 merge-queue-in-isolation floor → P-FLOW-10 (X-1 seam end-to-end). The
mock-agent-runtime floor (M2) → real LlmAgentRuntime is a post-M5 config/impl swap owned by Agent Fabric (named
in the roadmap §3, not a myelin-flow prompt). The cross-cell-workflow-spanning floor and the
history-archival/continue-as-new floor are designed-not-built per roadmap §3 with measured-trigger follow-ons
(no prompt yet — they are added by Phase 7-B as appended prompts when their measured trigger fires; the
DurableExecutor contract is cell-agnostic + engine-agnostic so they extend without a rewrite).

**Drills greened across the ledger:** FLOW-D1/D5 (P-FLOW-02), FLOW-D2 + lint fixtures (P-FLOW-03), FLOW-D3 floor
(P-FLOW-04), FLOW-D4 (P-FLOW-05), FLOW-D6/D7 (P-FLOW-06), merge-queue in-isolation (P-FLOW-07), CI-D9/CI-D1
(P-FLOW-09), GIT-D10/CI-D8 (P-FLOW-10), FLOW-D9/D10 (P-FLOW-11), FLOW-D3 full + FLOW-D8 (P-FLOW-12), E2E-2 spine
(P-FLOW-13), self-hosting CI graph + truth-up (P-FLOW-14). The must-be-green-first pair FLOW-D1 + FLOW-D5
(P-FLOW-02) is the gate nothing rides this engine until green.
