# Phase 7 — Prompt Ledger: Durable-Workflow Substrate (myelin-flow)

> **Finer-granularity pass (Phase 7-A refinement): prompt count 14 → 29.** Every multi-deliverable first-pass
> prompt is split into single-deliverable, clean-context, independently-committable units; all coverage
> (milestones, contracts, drills, floors) is preserved and re-threaded across the new finer ids. The genuinely
> atomic first-pass prompts (the merge-queue body in isolation, the CI-pipeline substrate, the X-1 seam
> end-to-end, the E2E-2 spine, the dogfood truth-up) are kept atomic.
>
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
>
> **A note on the split granularity (why 29, not 14):** the first pass bundled the engine heartbeat (data model +
> WfCtx core + replay + co-commit + guard + fixtures) into three large prompts; durable signals into one;
> long-park + reserve/settle + re-mint + loop-safety into one; crypto-shred + restore-verify into one. Each of
> those carried several independently-gateable sub-deliverables (its own contract row, its own drill, its own
> mutation floor). This pass exposes each sub-deliverable as its own clean-context prompt and re-threads the
> DEPENDS-ON chain across them. The union still covers FLOW-D1..D10 + the lint fixtures + GIT-D10/CI-D8 + CI-D1/D9
> + E2E-2 + the dogfood truth-up, and every named floor still has its follow-on.

---

### P-FLOW-01 — myelin-flow crate + the six-table data model (forward-only migrations)

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.1 (the data-model half of the engine heartbeat) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M2.1 "Work" bullet 1).
- **DEPENDS-ON.** The M0 substrate prompts that ship the forward-only-migration lint, the no-untagged-personal-data
  lint, the tenant-predicate lint, and the contract-coverage scanner; the M1 prompts that ship the tenant+region
  partition key (12.1) + the OLTP RLS tier (11.1). (The Phase-7-B index resolves these to concrete P-NNN ids; they
  MUST be merged before this prompt starts.)
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md (always) and external-insights/01-process-and-quality-doctrine.md §2 (order-by-non-negotiability
    — silent data loss outranks every feature) + §1 (name-your-floors).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §3 (the data model — workflow_run,
    wf_history, wf_timer, wf_signal, wf_activity_attempt, wf_definition; carried verbatim from Phase-3 §3) + §2
    (BUILD/DBOS-class decision) + the header "A note every prompt below assumes" above.
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 11.1 (OLTP/RLS), 12.1 (tenant+region
    partition), 9.1/9.6 (the surface these tables back — read for context; this prompt ships only the schema).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.1 ("Work" bullet 1 — the data model carried verbatim) +
    §0 (the placement paragraph) + §1 (the consumed-rows table).
- **DELIVERABLE (what to build + exactly where in the repo).** Create a new crate myelin-flow under the Cargo
  workspace, and in it the forward-only migrations creating exactly these six tables, all with tenant+region as the
  leading columns, RLS policies, per-tenant envelope-encryption hooks, and input/state stored as
  references-not-payloads (refs, not payloads):
  - workflow_run — state in {running, waiting, completed, failed, nondeterministic, terminated}; cursor (the replay
    short-circuit floor); budget as RunBudget; causality columns correlation_id/causation_id/caused_by/depth;
    partition + lease_owner/lease_expires for sharded lease dispatch.
  - wf_history — append-only journal, the source of truth; command_id deterministic from workflow position (the
    replay-match key); UNIQUE(tenant, run_id, command_id) makes journaling idempotent; result_key_ref envelope-
    encrypts the rare inline-PII result for crypto-shred.
  - wf_timer — bucket = epoch_minute(fire_at) + the partial index (bucket, partition) WHERE NOT fired (the SC-11
    world-scale move).
  - wf_signal — PK (tenant, run_id, signal_name, idem_key); payload_key_ref for inline-PII crypto-shred.
  - wf_activity_attempt — the idem_token idempotency ledger (bridges to BUS-2 so a retried emit is broker-deduped).
  - wf_definition — versioned registry; a run pinned to wf_version at start so a deploy cannot diverge an in-flight
    run.
  This prompt ships SCHEMA ONLY — no AppSpec wiring (P-FLOW-02), no holder registration (P-FLOW-03), no algorithms.
- **CONTRACTS TO IMPLEMENT.** None owned yet (the schema backs 9.1/9.6 but the trait surfaces ship later). Consumes
  11.1 OLTP/RLS, 12.1 (tenant, region).
- **GATE / DRILLS (quantified; must be green to call this done).** No FLOW drill greens here (the drills need the
  engine). The gate is structural: the migrations apply forward-only (forward-only-migration lint green); the
  no-untagged-personal-data lint green over the new crate (every PII column — result_key_ref, payload_key_ref,
  input/state refs — tagged); the tenant-predicate lint green (every table query carries the tenant predicate);
  no-cross-db green. Green artifact: a dated CI run showing migrate-up + the four named lints green.
- **TESTS (required).** Unit tests: command_id determinism from a workflow position; UNIQUE(tenant, run_id,
  command_id) rejects a duplicate journal row; the wf_signal PK rejects a duplicate (tenant, run_id, signal_name,
  idem_key); the wf_timer partial index covers a (bucket, partition) lookup WHERE NOT fired; RLS denies a
  cross-tenant select on every one of the six tables. No CDC pair yet (no contract surface shipped); no mutation
  floor (no decision logic).
- **DEFINITION OF DONE.** The crate compiles in the workspace; the six tables exist via forward-only migrations;
  the four named lints + the contract-coverage scanner are green and dated; the algorithm/surface work
  (AppSpec, holder registration, replay, timers, signals) is recorded as not-yet-built with its follow-on prompts
  (P-FLOW-02..03); the work is committed.
- **COMMIT.** Header `P-FLOW-01 M2: myelin-flow crate + six-table data model`. Body lists: no contract owned yet,
  11.1/12.1 consumed; the four lints greened; the AppSpec/holder/algorithm surfaces recorded as not-yet-built with
  follow-ons P-FLOW-02..03. Branch first if on the default branch. End with the workspace's required Co-Authored-By
  trailer.

---

### P-FLOW-02 — The myelin-flow AppSpec service shell (boot + migrate + outbox relay + empty consumer slot)

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.1 (the service-shell half of the engine heartbeat) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M2.1 "Work" bullet 1).
- **DEPENDS-ON.** P-FLOW-01 (the six-table data model). The M0 substrate prompts that ship serve(AppSpec) (contract
  1.1), the transactional outbox + idempotent-consumer template (contracts 2.2-2.5), and the EventEnvelope (2.1).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (observability-is-part-of-the-pass —
    liveness != readiness on the three ports).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §1 (the engine's responsibilities) + §10
    (no second emit path; the engine boots from serve(AppSpec)) + the header note above.
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 1.1 (serve(AppSpec) — boot/migrate/
    outbox-relay/three-ports/graceful-drain), 1.2 (the three ports), 1.3 (liveness != readiness), 2.2-2.5 (the
    outbox relay the shell wires, empty consumer slot for now).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.1 ("Work" bullet 1 — the worker shell) + §0.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: an AppSpec for the myelin-flow
  service that the harness boots — serve(AppSpec) does boot → run the P-FLOW-01 migrations → start the outbox relay
  → wire the consumer registration slot (EMPTY for now: the replay engine and the signal/timer consumers land in
  P-FLOW-02b..05) → bring up the three ports (public / internal / metrics-health) with liveness != readiness and
  graceful drain. No algorithms; this is the runnable shell the later engine prompts hang code on.
- **CONTRACTS TO IMPLEMENT.** None owned. Consumes 1.1 serve(AppSpec), 1.2/1.3 the ports, 2.2-2.5 outbox (relay
  wired, consumers empty).
- **GATE / DRILLS (quantified; must be green to call this done).** Structural: the service boots under
  serve(AppSpec) with the three ports up and liveness != readiness — a smoke test asserts the metrics-health port
  comes up and reports ready only after migrate + relay are live. Green artifact: a dated CI run showing boot + the
  three-port smoke test green.
- **TESTS (required).** Unit/integration tests: serve(AppSpec) boots the service against an ephemeral Postgres;
  migrate-up runs the six tables; the metrics-health port reports not-ready before migrate completes and ready
  after; graceful drain completes cleanly. No contract-owned CDC yet.
- **DEFINITION OF DONE.** The AppSpec boots under the harness with the three ports up and liveness != readiness;
  the outbox relay is wired (consumers empty, recorded as such); lints + coverage scanner green; the work is
  committed.
- **COMMIT.** Header `P-FLOW-02 M2: myelin-flow AppSpec service shell (boot + relay + empty consumer slot)`. Body
  lists 1.1/1.2/1.3/2.2-2.5 consumed; the three-port boot smoke test greened; the empty consumer slot recorded with
  follow-ons. Co-Authored-By trailer.

---

### P-FLOW-03 — PersonalDataHolder auto-registration over workflow_run/wf_history/wf_signal (structural half)

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.1 (the PersonalDataHolder structural half of the engine heartbeat) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M2.1 "Work" — the PersonalDataHolder auto-registration
  bullet).
- **DEPENDS-ON.** P-FLOW-01 (the tables), P-FLOW-02 (the AppSpec the harness auto-registers the holder on). The M0
  prompt that ships the PersonalDataHolder trait + auto-registration hook (contract 1.4 / 10.1) and the DSR
  orchestrator consumer shape.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §1 (name-your-floors — the crypto-shred
    reach is a named M5 follow-on) + external-insights/04-hard-problems.md §1 (references-not-payloads + crypto-shred
    + tombstone).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §5.5 (PersonalDataHolder over
    workflow_run/wf_history/wf_signal; references-not-payloads; the rare inline-PII case crypto-shreds via
    result_key_ref/payload_key_ref — the full reach is M5) + §4.8 (GDPR erasure on history, the structural floor).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.6 (PersonalDataHolder(workflow history)
    + replay — owned, the STRUCTURAL half here) + 1.4 / 10.1 (the holder auto-registration hook).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.1 (the holder bullet) + §1 (the 9.6 row: trait +
    auto-registration in M2.1; crypto-shred reach in M5).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: the PersonalDataHolder
  auto-registration over workflow_run / wf_history / wf_signal via the harness hook — locate/export signatures fully
  implemented over the references-not-payloads journal (a DSR orchestrator can locate and export a subject's run/
  history/signal rows by tenant+subject), and the erase signature STUBBED to the crypto-shred path (it tombstones
  and is structurally wired but the per-subject-DEK destruction reach into history/backups is the named M5
  follow-on). NAMED FLOOR: the crypto-shred-reach is the structural floor here; its follow-on is P-FLOW-23 (M5).
- **CONTRACTS TO IMPLEMENT.** 9.6 PersonalDataHolder(workflow history) — owned, STRUCTURAL half (trait +
  auto-registration; locate/export real, erase structurally wired; crypto-shred reach is the named M5 follow-on).
  Consumes 1.4 / 10.1 the holder hook.
- **GATE / DRILLS (quantified; must be green to call this done).** No FLOW drill greens here. Structural gate: the
  holder auto-registers on boot (a startup assertion shows workflow_run/wf_history/wf_signal each registered);
  locate/export over an empty-and-a-populated history return the correct (PII-free reference) rows. Green artifact:
  a dated CI run showing the holder registered on boot + the locate/export CDC pair green.
- **TESTS (required).** Unit tests: the holder is auto-registered for all three tables on boot; locate returns a
  subject's rows scoped to tenant; export emits references-not-payloads (no inline PII leaks); the erase stub
  tombstones the references (the per-subject-DEK destruction is asserted at P-FLOW-23, recorded here as not-yet-full).
  CDC: the provider+consumer pair for 9.6 (a DSR orchestrator consumer calling locate/export over the history).
- **DEFINITION OF DONE.** 9.6 is wired and CDC-covered (locate/export real, erase structurally stubbed); the holder
  auto-registers on boot; the crypto-shred-reach FLOOR is named in writing with its follow-on (P-FLOW-23); lints +
  coverage scanner green; the work is committed. (M2.1's structural surface — schema + shell + holder — is now
  covered across P-FLOW-01..03; the algorithms follow in P-FLOW-04..08.)
- **COMMIT.** Header `P-FLOW-03 M2: PersonalDataHolder auto-registration (structural half)`. Body lists contract 9.6
  (structural) implemented + 1.4/10.1 consumed; the locate/export CDC pair greened; the crypto-shred-reach floor
  named with follow-on P-FLOW-23. Co-Authored-By trailer.

---

### P-FLOW-04 — WfCtx core: activity + now + rand + emit, with the journal/outbox co-commit (FLOW-D5)

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.1 (the WfCtx deterministic surface + the silent-data-loss co-commit floor) —
  roadmap file planning/06-roadmaps/shared/durable-workflow.md §2 (M2.1 "Work" bullets 2-3, 6).
- **DEPENDS-ON.** P-FLOW-01 (wf_history + wf_activity_attempt), P-FLOW-02 (the AppSpec). The M0 failure-injection
  harness prompt (so FLOW-D5 is drillable) and the OutboxTx::emit template (contract 2.2). All merged first.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (prove-it-or-it-isn't-real + the
    failure-injection harness) + §2 (silent data loss outranks every feature — the co-commit IS the Tier-1 floor).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §5.1 (the WfCtx trait surface: activity,
    now, rand, emit) + §4.4 (activity execution + retry) + §4.5 (the outbox seam, no second emit path) + §3.2
    (wf_history as the journal source of truth).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.2 (WfCtx: activity/now/rand/emit — the
    deterministic surface, OWNED here in part) + 2.2 (OutboxTx::emit — the only emit path) + 1.8 (telemetry set).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.1 (Work bullets 2-3) + §4 (the FLOW-D5 row).
  - testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row FLOW-D5 (the exact co-commit threshold) +
    testing-strategy/02-parts-contracts-and-mock-agents.md FLOW-G3.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: the WfCtx core surface —
  activity<I,O> (journaled to wf_history under its deterministic command_id, retried per §4.4, the idem_token
  recorded in wf_activity_attempt); now() and rand() as journaled side-markers; emit(EventDraft) via OutboxTx so the
  journal row and the outbox row co-commit in ONE transaction (no second emit path — the no-raw-publish lint forbids
  it). This prompt ships the SINGLE-TXN write discipline + the activity primitive; the replay short-circuit that
  reads these journal rows back lands in P-FLOW-05. Telemetry: activity queue depth + retry + dead-letter on the
  metrics-health port (contract 1.8).
- **CONTRACTS TO IMPLEMENT.** 9.2 WfCtx (activity, now, rand, emit) — owned (the write half; replay/wait/timer
  halves of 9.2 land in later prompts). Consumes 2.2 OutboxTx::emit.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - FLOW-D5 (CI) — crash between journaling an activity's DB write and emitting its event; the journal row and the
    outbox row are committed together (one txn) — 0 ghost, 0 lost. Green artifact: the co-commit proof (the run is
    either fully journaled-and-emitted or neither), dated CI. (This is myelin-flow's face of the Tier-1
    silent-data-loss floor — BUS-D4-equivalent for the workflow journal; never weaken it.)
- **TESTS (required).** Unit tests: an activity journals exactly one wf_history row under its command_id; emit and
  journal share one txn (inject a failure between them, assert atomic — neither or both); a retried activity reuses
  its idem_token (no duplicate effect). Chained drill test (preferred — EI-01 §4): the FLOW-D5 scenario on the
  failure-injection harness (crash between journal and emit, assert atomicity). CDC: the provider half of 9.2
  (activity/now/rand/emit) paired with a consumer fixture. Mutation floor: the co-commit path carries >= 90%
  cargo-mutants (testing-strategy/00 — "a surviving mutant = a ghost or lost emit"); a mutant that splits the
  journal and emit into two txns MUST be caught.
- **DEFINITION OF DONE.** WfCtx core (activity/now/rand/emit) + the single-txn co-commit compile and run; FLOW-D5
  emits a dated green artifact (PROVEN, not CLAIMED); the unit + drill + CDC tests pass; the co-commit mutation
  score >= 90%; all committed lints + the coverage scanner green; the replay short-circuit + lease dispatch +
  start/describe/cancel are recorded as not-yet-shipped (P-FLOW-05..06); the work is committed.
- **COMMIT.** Header `P-FLOW-04 M2: WfCtx core + journal/outbox co-commit`. Body lists contract 9.2 (write half)
  implemented + 2.2 consumed; FLOW-D5 (co-commit proof) greened with the measured 0-ghost/0-lost numbers; the
  co-commit mutation score; the replay/lease/executor surfaces recorded as next. Co-Authored-By trailer.

---

### P-FLOW-05 — Deterministic replay/recovery + lease-based dispatch + crash recovery (FLOW-D1)

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.1 (the replay engine + lease dispatch — the exactly-once heartbeat) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M2.1 "Work" bullet 4 + the lease/recovery bullet).
- **DEPENDS-ON.** P-FLOW-04 (the WfCtx core that writes the journal rows replay reads back). The M0
  failure-injection harness (so FLOW-D1 is drillable).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (prove-it; exactly-once-in-effect is
    the quantified gate) + §4 (chain mutations — kill mid-run, assert resume).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §4.1 (the deterministic replay/recovery
    algorithm — replay wf_history to the cursor, short-circuit already-journaled commands, continue from the first
    un-journaled command) + §4.7 (lease-based dispatch + crash recovery — lease_owner/lease_expires with expiry
    re-lease) + §3.2 (wf_history as the journal source of truth).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.2 (WfCtx — the replay-determinism half)
    + 1.8 (telemetry: runnable-run lag + replay rate).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.1 (Work bullet 4) + §4 (the FLOW-D1 row).
  - testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row FLOW-D1 (the exact thresholds) +
    testing-strategy/02 FLOW-G2.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: (a) the deterministic
  replay/recovery algorithm (§4.1) — a worker leases a runnable run, replays wf_history to the cursor
  short-circuiting already-journaled commands (0 re-execution of side effects), and continues from the first
  un-journaled command; (b) lease-based dispatch + crash recovery (§4.7) — a runnable run is leased
  (lease_owner/lease_expires), a lease expiry re-leases to another worker, and a crash mid-run leaves the journal as
  the source of truth so the replay resumes exactly; (c) wire the replay/lease loop into the P-FLOW-02 consumer
  slot; (d) the runnable-run-lag + replay-rate telemetry (contract 1.8).
- **CONTRACTS TO IMPLEMENT.** 9.2 WfCtx — the replay-determinism half (no new owned row; this makes the activity
  surface's replay contract real). Consumes nothing new.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - FLOW-D1 (CI) — kill a worker at activity 5 of 10 mid-run; another re-leases, replays wf_history, resumes at step
    6 with 0 re-executed side effects, 0 lost progress, exactly-once-in-effect. Green artifact: the replay-rate
    signal emitted + a 0-double-effect counter on the metrics port, dated CI.
- **TESTS (required).** Unit tests: replay short-circuits a journaled command (assert 0 re-execution); a lease
  expiry re-leases to a new worker; resume continues from the first un-journaled command. Chained drill test
  (preferred — EI-01 §4): the FLOW-D1 scenario on the failure-injection harness (kill at 5/10, assert resume-at-6
  with 0 re-executed side effects). Mutation floor: the replay short-circuit path carries >= 90% cargo-mutants
  (testing-strategy/00 mandatory-core — "a surviving mutant = a silent double-effect on replay"); a mutant that
  skips the short-circuit (re-executes a journaled command) MUST be caught.
- **DEFINITION OF DONE.** Replay/recovery + lease dispatch compile and run, wired into the consumer slot; FLOW-D1
  emits a dated green artifact (PROVEN, not CLAIMED); the unit + drill tests pass; the replay-core mutation score >=
  90%; all committed lints + the coverage scanner green; the divergence guard is recorded as not-yet-shipped
  (P-FLOW-07); the work is committed. (FLOW-D1 + FLOW-D5 — the must-be-green-first pair — are now both green.)
- **COMMIT.** Header `P-FLOW-05 M2: deterministic replay/recovery + lease dispatch`. Body lists contract 9.2
  (replay half); FLOW-D1 (replay-rate, 0 double-effect) greened with measured numbers; the replay-core mutation
  score; the divergence guard recorded as P-FLOW-07. Co-Authored-By trailer.

---

### P-FLOW-06 — DurableExecutor start/describe/cancel + the engine telemetry set

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.1 (the DurableExecutor control surface + the metrics-health telemetry) — roadmap
  file planning/06-roadmaps/shared/durable-workflow.md §2 (M2.1 "Work" — DurableExecutor::{start, describe, cancel}
  + the telemetry set).
- **DEPENDS-ON.** P-FLOW-05 (the replay/lease engine start/describe/cancel drive). The signal half of 9.1 lands in
  P-FLOW-09; the telemetry signals for timers/signals/budget are added by the prompts that ship those surfaces.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (observability-is-part-of-the-pass —
    a target you cannot measure is not a gate).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §5.1 (the DurableExecutor trait: start,
    signal, describe, cancel — signal lands later; StartSpec{wf_type, input, budget, idem_key}) + §5.4 (the
    telemetry contract, the drill-survival signals on the metrics-health port).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.1 (DurableExecutor start/describe/cancel
    — OWNED, partial; signal is P-FLOW-09) + 1.8 (the telemetry set).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.1 (the DurableExecutor bullet + the telemetry bullet) + §1
    (the 9.1 row: start/describe/cancel in M2.1).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: (a) DurableExecutor::start
  (StartSpec{wf_type, input, budget, idem_key} → RunId; idempotent on idem_key; input stored as a reference),
  describe (RunId → RunStatus), cancel (RunId, reason) — the engine-agnostic control surface; (b) the engine
  telemetry set on the metrics-health port (contract 1.8): runnable-run lag, replay rate, activity queue depth +
  retry + dead-letter (the timer/signal/budget signals are added by their owning prompts). This is the
  control-plane surface bus automations / Agent / CI / Issues call to start and steer a run.
- **CONTRACTS TO IMPLEMENT.** 9.1 DurableExecutor (start, describe, cancel) — owned (the signal method is P-FLOW-09).
- **GATE / DRILLS (quantified; must be green to call this done).** No new FLOW drill is owed by this surface (it is
  exercised by FLOW-D1's start + the later drills' start/cancel). Structural gate: the telemetry signals named in
  contract 1.8 (runnable-run lag, replay rate, activity queue/retry/dead-letter) are emitted on the metrics-health
  port and assert-readable by the telemetry-assertion library; start is idempotent on idem_key. Green artifact: a
  dated CI run showing the named telemetry signals readable + an idempotent-start assertion.
- **TESTS (required).** Unit tests: start is idempotent on idem_key (a re-start with the same key returns the same
  RunId, not a second run); describe returns the run's RunStatus; cancel transitions a run to terminated; the four
  named telemetry signals are readable. CDC: the provider+consumer pair for 9.1 (start/describe/cancel) paired with
  a bus-automation consumer fixture.
- **DEFINITION OF DONE.** DurableExecutor start/describe/cancel + the engine telemetry set compile and run; the
  telemetry signals are readable + the idempotent-start assertion passes; the 9.1 CDC pair (start/describe/cancel)
  is green; lints + coverage scanner green; the signal method is recorded as the M2.3 follow-on (P-FLOW-09); the
  work is committed.
- **COMMIT.** Header `P-FLOW-06 M2: DurableExecutor start/describe/cancel + telemetry set`. Body lists contract 9.1
  (start/describe/cancel) implemented; the telemetry signals readable; the signal method deferred to P-FLOW-09.
  Co-Authored-By trailer.

---

### P-FLOW-07 — The replay-divergence guard (halt-as-nondeterministic + dead-letter) (FLOW-D2)

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.1 (the determinism guard — the replay-divergence halt) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M2.1 "Work" bullet 5).
- **DEPENDS-ON.** P-FLOW-05 (the replay engine the guard sits inside). The M0 failure-injection harness (so FLOW-D2
  is drillable).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (a red gate is information; never
    invert an assertion — halt, never silent-continue) + §2 (silent divergence is a Tier-1 failure).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §2 (the determinism constraint + the
    replay-divergence guard, carried from Phase-3 §2.5) + §4.6 (versioning — a run pinned to wf_version) + "Changes
    vs Phase 3" item on the determinism guard.
  - planning/05-refined-shared-systems-architecture/contract-index.md row 9.2 (WfCtx — the surface the guard
    enforces non-determinism flows through) + 1.8 (nondeterministic-halt-count telemetry).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.1 (the divergence-guard bullet + the Exit gate FLOW-D2
    line) + §4 (the FLOW-D2 threshold).
  - testing-strategy/01-whole-system-e2e-and-drill-catalogue.md FLOW-D2.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: the replay-divergence guard —
  when a replay encounters a journaled command that does not match the definition's command at that position (a
  divergent body or a wrong-version body, wf_version pinned at start), the run halts as state=nondeterministic and
  dead-letters; 0 silent divergence, never a silent continue. Plus the nondeterministic-halt-count telemetry on the
  metrics port (contract 1.8).
- **CONTRACTS TO IMPLEMENT.** 9.2 WfCtx — the determinism-enforcement half (the divergence guard is what makes the
  surface's determinism contract real at runtime). No new owned row; this hardens 9.2.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - FLOW-D2 (CI) — replay against a divergent / wrong-version definition; the divergence guard halts the run as
    nondeterministic and dead-letters it; 0 silent divergence. Green artifact: the nondeterministic-halt-count
    signal increments by exactly the injected count, dated CI.
- **TESTS (required).** Unit test: a position-mismatch on replay raises the halt, not a continue; a wrong-version
  replay (the pinned wf_version mismatches) halts. Drill test: the FLOW-D2 scenario on the harness (wrong-version
  replay → halt + dead-letter, assert 0 silent divergence). Mutation floor: the determinism-guard path carries >=
  90% cargo-mutants (testing-strategy/00 — "a surviving mutant = a silent double-effect on replay"); a mutant that
  turns the halt into a continue MUST be caught.
- **DEFINITION OF DONE.** The divergence guard halts-not-continues and dead-letters; FLOW-D2 emits its dated green
  artifact; the determinism-guard mutation score >= 90%; all lints + the coverage scanner green; the
  flow-determinism lint FIXTURES are recorded as the next prompt (P-FLOW-08); the work is committed.
- **COMMIT.** Header `P-FLOW-07 M2: replay-divergence guard (halt-as-nondeterministic + dead-letter)`. Body lists
  the FLOW-D2 greening (nondeterministic-halt count) + the >= 90% guard mutation score; the lint fixtures deferred
  to P-FLOW-08. Co-Authored-By trailer.

---

### P-FLOW-08 — The flow-determinism lint red+green fixtures (proves the lint rejects and admits)

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.1 (the flow-determinism lint proof — myelin-flow's red+green fixtures) — roadmap
  file planning/06-roadmaps/shared/durable-workflow.md §2 (M2.1 "Work" bullet 7).
- **DEPENDS-ON.** P-FLOW-04 (the WfCtx surface the fixtures express). The M0 prompt that ships the flow-determinism
  lint ITSELF (contract 1.6) — this prompt ships myelin-flow's red+green FIXTURES, not the lint.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §5 (the ratchet: a committed lint with
    a red-fixture that proves it rejects + a green-fixture that proves it admits; loud-never-swallowed, no `|| true`).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §2 (the determinism constraint + lint,
    carried from Phase-3 §2.5) + §10.3 (the flow-determinism lint: a workflow reading clock/RNG/IO outside WfCtx
    fails to compile).
  - planning/05-refined-shared-systems-architecture/contract-index.md row 1.6 (the flow-determinism lint, in the
    twelve-lint set) + 9.2 (WfCtx, the surface the lint enforces non-determinism flows through).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.1 (the flow-determinism fixtures bullet + the Exit gate
    "flow-determinism lint green" line) + §1 (the lint row: "myelin-flow ships its red+green fixtures in M2.1").
  - testing-strategy/02-parts-contracts-and-mock-agents.md FLOW-G1.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow, under the crate's fixtures dir:
  the flow-determinism lint FIXTURES — a RED fixture (a workflow body that reads SystemTime/IO/RNG outside WfCtx —
  MUST fail to compile under the lint) and a GREEN fixture (the same logic expressed via ctx.now() / ctx.rand() /
  ctx.activity() — MUST compile); both wired into CI loud-never-swallowed (no `|| true`). The red fixture is the
  proof the lint rejects; the green is the proof it admits.
- **CONTRACTS TO IMPLEMENT.** 9.2 WfCtx — the lint-proof half (the committed fixtures are what make the lint a real
  gate over this surface). No new owned row.
- **GATE / DRILLS (quantified; must be green to call this done).** The flow-determinism lint green on BOTH fixtures:
  the red fixture fails to compile (proves the lint rejects), the green fixture compiles (proves it admits). Green
  artifact: a dated CI run showing red-rejects + green-admits, wired loud-never-swallowed.
- **TESTS (required).** The fixture pair is itself the lint test: CI must show the red fixture's compile FAILS (and
  CI is green BECAUSE it fails — the assertion is inverted-safe, never `|| true`) and the green fixture's compile
  succeeds. A regression test asserts the lint wiring is loud (a swallowed lint failure would itself fail CI).
- **DEFINITION OF DONE.** The red+green fixtures are committed and CI proves the lint rejects the red and admits the
  green, loud-never-swallowed; all lints + the coverage scanner green; the work is committed. (M2.1 is now fully
  covered across P-FLOW-01..08 — schema, shell, holder, WfCtx core + co-commit, replay + lease, executor +
  telemetry, divergence guard, lint fixtures.)
- **COMMIT.** Header `P-FLOW-08 M2: flow-determinism lint red+green fixtures`. Body lists the lint red/green-fixture
  proof (red-rejects + green-admits, loud-never-swallowed). Co-Authored-By trailer.

---

### P-FLOW-09 — Durable signals: DurableExecutor::signal + wf_signal idempotency-by-construction

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.3 (durable signals — the idempotent signal delivery half) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M2.3 "Work" — wf_signal + DurableExecutor::signal).
- **DEPENDS-ON.** P-FLOW-06 (the DurableExecutor surface signal joins), P-FLOW-08 (a green M2.1 — the gate invariant:
  no M2.3 work claimed done over a red FLOW-D1/D2/D5).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (prove-it: a double-signal = one
    delivery).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §3.4 (wf_signal; PK (tenant, run_id,
    signal_name, idem_key) — the PK that makes idempotency true by construction; payload_key_ref) + §4.3 (the signal
    round-trip — the delivery side) + §5.4 (signal-buffer-depth telemetry).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.1 (DurableExecutor::signal — idempotent
    on idem_key; OWNED here, the delivery mechanism; the per-effect rule is P-FLOW-10) + 1.8 (signal-buffer-depth
    telemetry).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.3 (Work — wf_signal + signal) + §1 (the 9.1 signal row).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: DurableExecutor::signal
  (contract 9.1) — idempotent on idem_key via INSERT ... ON CONFLICT (tenant, run_id, signal_name, idem_key) DO
  NOTHING into wf_signal, so a double-delivered signal is buffered exactly once; the inbound-signal consumer wired
  into the P-FLOW-02 consumer slot; payload stored references-not-payloads (the rare inline-PII case crypto-shreds
  via payload_key_ref); the signal-buffer-depth telemetry (contract 1.8). This prompt ships the DELIVERY +
  IDEMPOTENCY mechanism; the consuming wait (wait_for_signal) is P-FLOW-11 and the per-effect key rule is P-FLOW-10.
- **CONTRACTS TO IMPLEMENT.** 9.1 DurableExecutor::signal — owned (the idempotent-delivery half; the per-effect rule
  is P-FLOW-10).
- **GATE / DRILLS (quantified; must be green to call this done).** No standalone FLOW drill greens here (FLOW-D4
  needs the wait, P-FLOW-11). Structural gate: a doubly-delivered signal under the same (tenant, run_id,
  signal_name, idem_key) inserts exactly once (the signal-buffer-depth increments by one, not two). Green artifact:
  a dated CI run showing the ON CONFLICT DO NOTHING dedup + the signal-buffer-depth signal.
- **TESTS (required).** Unit tests: a double-delivered signal under the same idem_key inserts once (ON CONFLICT DO
  NOTHING); two signals differing only in idem_key both insert; a signal payload stores as a reference, not inline
  PII. CDC: the provider+consumer pair for 9.1 (signal delivery).
- **DEFINITION OF DONE.** DurableExecutor::signal + the wf_signal ON CONFLICT idempotency compile and run, wired
  into the consumer slot; the double-delivery-inserts-once gate is green and dated; the 9.1 (signal) CDC pair is
  green; lints + coverage scanner green; the per-effect rule (P-FLOW-10) and the wait (P-FLOW-11) are recorded as
  follow-ons; the work is committed.
- **COMMIT.** Header `P-FLOW-09 M2: durable signals (DurableExecutor::signal + wf_signal idempotency)`. Body lists
  contract 9.1 (signal delivery) owned; the double-delivery-inserts-once proof; the per-effect rule + wait deferred
  to P-FLOW-10/11. Co-Authored-By trailer.

---

### P-FLOW-10 — The per-effect idem_key rule for batch / partial HITL approval (single vs multi-effect)

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.3 (the per-effect idem_key rule — batch/partial approval well-defined) — roadmap
  file planning/06-roadmaps/shared/durable-workflow.md §2 (M2.3 "Work" — the per-effect idem_key rule).
- **DEPENDS-ON.** P-FLOW-09 (the signal-delivery + wf_signal idempotency the per-effect keys ride). Co-built with
  the Agent Fabric M2 prompt that owns the gated-tool set + EffectApi::apply (the withheld-effect target).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (prove-it: double-click = one
    approval; a declined effect makes 0 mutation).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §6.4 (the per-effect idem_key rule — idem_key
    = card_id single / card_id ":" effect_idx multi; partial approval well-defined; a declined effect withheld,
    returns Denied, never mutates, AG-8) + §3.4 (the wf_signal PK that makes it true by construction).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.1 (the per-effect idem_key rule on
    DurableExecutor::signal — OWNED) + 8.x EffectApi::apply (consumed, the apply/withhold target — Agent Fabric).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.3 (the per-effect bullet) + §1 (the 9.1 per-effect row).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §8 (F-4 extended — the per-effect form).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: the per-effect idem_key rule
  (§6.4) over the P-FLOW-09 signal delivery — idem_key = card_id for a single-effect card; idem_key = card_id ":"
  effect_idx for a multi/partial-approval card. A partial approval (approve effects 0 and 2, decline 1) is three
  independently-idempotent signals ({card_id:0=approve, card_id:1=decline, card_id:2=approve}), each mapping to
  exactly one EffectApi::apply; a declined effect is WITHHELD (returns Denied, never mutates, AG-8); a double-click
  on "approve all" re-sends the same keys → ON CONFLICT DO NOTHING → no double-apply. The engine contribution is
  the key-construction rule + the PK; the EffectApi set is Agent Fabric's.
- **CONTRACTS TO IMPLEMENT.** 9.1 DurableExecutor::signal — the per-effect idem_key rule (owned). Consumes the Agent
  Fabric EffectApi::apply (the apply/withhold target).
- **GATE / DRILLS (quantified; must be green to call this done).** No standalone FLOW drill greens here; the
  per-effect form is asserted in F-4's extended form (architecture §8) at P-FLOW-12 and at the subsystem face in
  CHAT-D10 (M4, referenced). Structural gate: three per-effect keys (card_id:0/1/2) apply/decline independently and
  idempotently; a double-click re-send is a no-op (0 double-apply); a declined effect makes 0 mutation. Green
  artifact: a dated CI run showing 3 independent apply/decline ledger entries + a 0-double-apply counter + a
  0-mutation-on-decline assertion.
- **TESTS (required).** Unit tests: three per-effect keys (card_id:0/1/2) apply/decline independently; a declined
  effect returns Denied and makes 0 mutation; a double-click on "approve all" re-sends the same keys and applies
  each effect exactly once (ON CONFLICT DO NOTHING). CDC: the provider half of the per-effect rule paired with an
  Agent Fabric EffectApi consumer fixture.
- **DEFINITION OF DONE.** The per-effect idem_key rule compiles and runs over the signal delivery; the
  independent-apply / 0-double-apply / 0-mutation-on-decline gate is green and dated; the CDC pair is green; lints +
  coverage scanner green; it is recorded that the per-effect F-4-extended drill lands at P-FLOW-12 and at CHAT-D10
  (M4); the work is committed.
- **COMMIT.** Header `P-FLOW-10 M2: per-effect idem_key rule (batch/partial HITL approval)`. Body lists contract 9.1
  (per-effect rule) owned; the 3-independent-apply / 0-double-apply / 0-mutation-on-decline proof; the F-4-extended
  drill recorded as P-FLOW-12. Co-Authored-By trailer.

---

### P-FLOW-11 — WfCtx wait_for_signal + the multi-day HITL approval-card round-trip (FLOW-D4)

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.3 (the multi-day HITL wait — state=waiting holds no runtime) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M2.3 "Work" — wait_for_signal + the HITL round-trip).
- **DEPENDS-ON.** P-FLOW-10 (the per-effect signal the wait consumes), P-FLOW-13 (the durable timer the timeout
  branch uses — see note below on ordering). Co-built with the Notif humanise prompt (contract 7.3) that renders the
  card. NOTE: the wait's timeout branch needs the durable timer (P-FLOW-13); within the M2 band the index orders
  P-FLOW-13 before this prompt's timeout path is drilled, or the timeout branch is wired against the timer the
  moment P-FLOW-13 lands — DEPENDS-ON P-FLOW-13 makes this concrete.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (prove-it: resume after days across
    a restart + a deploy; a withheld effect makes 0 mutation) + §4 (chain mutations end-to-end).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §4.3 (the signal round-trip; state=waiting
    holds no runtime) + §6.3 (the HITL approval-card round-trip mechanics: gated tool → wait_for_signal →
    agent.approval.requested via the outbox → Approve/Deny → resume/withhold/timeout) + §5.4 (oldest-unconsumed-
    wait-age telemetry).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.4 (durable signal, multi-day HITL — the
    approval/cancel waits, OWNED) + 9.2 (WfCtx wait_for_signal — OWNED, the wait half) + 7.3 (humanise — consumed;
    the card is rendered there, the one templating surface).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.3 (Work — wait_for_signal + the HITL round-trip + the Exit
    gate FLOW-D4) + §4 (the FLOW-D4 threshold).
  - testing-strategy/01-whole-system-e2e-and-drill-catalogue.md FLOW-D4 + the F-4 assertion in architecture §8.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: (a) WfCtx wait_for_signal(name,
  timeout) — state=waiting holds no runtime; registers approval and cancel as names now (ci.result and job.done are
  registered names too but their long-park producer wiring lands P-FLOW-14/15); the timeout branch uses the durable
  timer (P-FLOW-13); (b) the HITL approval-card round-trip mechanics (§6.3): a gated tool →
  wait_for_signal("approval:<call>", timeout=window) → emits agent.approval.requested via the outbox (the card
  UX/visual is Chat+Agent-Fabric product work, OQ #1 — NOT this engine); on a delivered approval the workflow
  resumes and runs (approved) or withholds (denied → 0 mutation, AG-8) or takes the timeout path; (c) the
  oldest-unconsumed-wait-age telemetry (contract 1.8).
- **CONTRACTS TO IMPLEMENT.** 9.4 durable signal (approval/cancel waits) — owned. 9.2 WfCtx wait_for_signal — owned
  (the wait half). Consumes 7.3 humanise.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - FLOW-D4 (CI) — a gated workflow waits across a worker restart + a deploy; deliver the approval days later with a
    double-click; the workflow resumes, consumes the approval exactly ONCE, and runs (approved) or withholds (denied
    → 0 mutation) correctly. Green artifact: 1 consume on the signal-buffer-depth ledger; withhold = 0 mutation,
    dated CI.
- **TESTS (required).** Unit tests: a wait parks the run as state=waiting holding no runtime; a delivered approval
  resumes the run; a denied approval withholds (0 mutation); a wait that times out takes the timeout branch (the
  durable timer fires). Chained drill test (preferred — EI-01 §4): the FLOW-D4 scenario across a restart+deploy with
  a days-later double-click, asserting 1 consume + correct withhold. CDC: the provider+consumer pair for 9.4
  (approval/cancel waits).
- **DEFINITION OF DONE.** wait_for_signal + the HITL round-trip compile and run; FLOW-D4 emits its dated green
  artifact (1 consume, withhold = 0 mutation); the unit + drill + CDC tests pass; lints + coverage scanner green;
  the ci.result/job.done long-park wiring is recorded as P-FLOW-14/15; the card visual/data model is recorded as
  Chat+Agent-Fabric product work, not this engine; the work is committed. (M2.3 is now covered across
  P-FLOW-09..11.)
- **COMMIT.** Header `P-FLOW-11 M2: wait_for_signal + multi-day HITL approval-card round-trip`. Body lists contracts
  9.4 + the 9.2 wait half + 7.3 consumed; FLOW-D4 greened (1 consume, 0-mutation withhold); the long-park wiring
  deferred to P-FLOW-14/15. Co-Authored-By trailer.

---

### P-FLOW-12 — F-4 extended: the per-effect partial-approval drill across a restart + deploy

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.3 (the per-effect partial-approval drill — F-4's extended form) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M2.3 — FLOW-D4 extended to the per-effect form, architecture
  §8).
- **DEPENDS-ON.** P-FLOW-10 (the per-effect idem_key rule), P-FLOW-11 (the wait + FLOW-D4). This is the drill that
  proves P-FLOW-10's rule under the durable wait across a restart.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §4 (chain mutations end-to-end — a
    partial approval IS a sequence property) + §3 (a declined effect makes 0 mutation; never weaken it).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §8 (F-4 extended — a multi-effect approval
    card sends {card_id:0=approve, card_id:1=decline, card_id:2=approve} with a double-click on "approve all" →
    assert each effect applies/withholds exactly once, the declined effect never mutates, AG-8) + §6.4.
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.1 (the per-effect idem_key rule) + 9.4
    (the durable wait it parks on).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.3 (the partial-approval-per-effect note) + §4 (FLOW-D4).
  - testing-strategy/01-whole-system-e2e-and-drill-catalogue.md FLOW-D4 (the per-effect extended assertion).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow (the drill harness scope): the
  F-4-extended drill scenario — a gated workflow with a multi-effect approval card parks on the durable wait across
  a worker restart + a deploy; the partial approval ({card_id:0=approve, card_id:1=decline, card_id:2=approve})
  arrives later WITH a double-click on "approve all"; assert each effect applies/withholds EXACTLY ONCE, the
  declined effect never mutates (AG-8), and the double-click is absorbed (0 double-apply). No new engine primitive —
  this drills P-FLOW-10's rule + P-FLOW-11's wait together under failure injection.
- **CONTRACTS TO IMPLEMENT.** None new — this exercises 9.1 (per-effect rule) + 9.4 (the wait) end-to-end under a
  restart.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - FLOW-D4 (per-effect extended, CI) — the partial-approval card across a restart+deploy with a double-click: each
    effect applies/withholds exactly once; the declined effect makes 0 mutation; the double-click is absorbed. Green
    artifact: a dated CI run showing 3 per-effect ledger entries (apply/decline/apply), a 0-double-apply counter,
    and a 0-mutation-on-decline assertion across the restart.
- **TESTS (required).** Chained drill test (preferred — EI-01 §4): the F-4-extended scenario on the
  failure-injection harness (multi-effect card → park across restart+deploy → partial approval + double-click →
  assert per-effect exactly-once + 0-mutation-on-decline). Unit tests: the partial-approval ledger has exactly three
  entries; the double-click adds no fourth apply.
- **DEFINITION OF DONE.** The F-4-extended per-effect drill emits its dated green artifact (3 per-effect entries, 0
  double-apply, 0 mutation on decline across the restart); the drill + unit tests pass; lints + coverage scanner
  green; the CHAT-D10 (M4) subsystem face is recorded as Chat's; the work is committed.
- **COMMIT.** Header `P-FLOW-12 M2: F-4 extended per-effect partial-approval drill`. Body lists the FLOW-D4
  per-effect extended greening (3 entries, 0 double-apply, 0 mutation on decline across restart); the contracts
  exercised (9.1/9.4). Co-Authored-By trailer.

---

### P-FLOW-13 — Durable timers: the minute-bucket wheel + sleep_until/sleep_for (FLOW-D3 floor)

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.2 (durable timers at scale — the wheel + sleep) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M2.2).
- **DEPENDS-ON.** P-FLOW-05 (the engine + the journal), P-FLOW-08 (a green M2.1 — the gate invariant: no M2.2 work
  claimed done over a red FLOW-D1/D2/D5). The M0 failure-injection harness's load generator (so FLOW-D3 is
  drillable at 100k+).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (quantified thresholds; the
    1x/10x/30x load generator) + §1 (name-your-floors: the 100k-timer floor with the 1M follow-on).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §3.3 (wf_timer; bucket =
    epoch_minute(fire_at); the partial index (bucket, partition) WHERE NOT fired) + §4.2 (the timer wheel: scan
    bucket <= now AND NOT fired, FOR UPDATE SKIP LOCKED, no calendar logic on the wheel) + §7.3 (the
    millions-of-timers scaling story) + §5.4 (timer-wheel-lag telemetry).
  - planning/05-refined-shared-systems-architecture/contract-index.md row 9.3 (the durable timer wheel; cheap
    disarm/re-arm of a precomputed fire_at — the re-arm half is P-FLOW-14) + 9.2 (WfCtx sleep_until/sleep_for).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.2 (Work + Exit gate FLOW-D3 floor) + §3 (the 100k-timer
    floor row → the 1M+ cell-scale follow-on in M5) + §4 (FLOW-D3 floor threshold).
  - testing-strategy/01-whole-system-e2e-and-drill-catalogue.md FLOW-D3.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: (a) WfCtx sleep_until(t) and
  sleep_for(d) — each arms a durable wf_timer row (bucket = epoch_minute(fire_at)), parks the run holding no
  runtime, and fires effectively-once (a crash re-fires only the unfired); (b) the timer-wheel scan loop wired into
  the consumer slot: bucket <= now AND NOT fired, FOR UPDATE SKIP LOCKED, with NO calendar logic on the wheel (a
  30-day timer is never read until its minute); (c) the timer-wheel-lag telemetry (contract 1.8, the SC-11 health
  signal). The cheap disarm/re-arm surface is P-FLOW-14. NAMED FLOOR: this prompt proves the algorithm at 100k+
  timers (six figures); the 1M+ cell-scale run + the per-cell timer-wheel-promotion threshold is the M5 follow-on
  P-FLOW-24.
- **CONTRACTS TO IMPLEMENT.** 9.3 Durable timer wheel — owned (the wheel + arm/fire; the disarm/re-arm half is
  P-FLOW-14). 9.2 WfCtx sleep_until/sleep_for — owned (the timer half).
- **GATE / DRILLS (quantified; must be green to call this done).**
  - FLOW-D3 (SCHED, run at 100k+ timers in M2 — the floor): arm 100k+ durable timers plus a burst all due in one
    minute; the due timers fire WITHIN the tick budget; far-future timers cost ~nothing (never scanned); a crash
    re-fires only the unfired. 0 lost, 0 double-fire. Green artifact: the timer-wheel-lag signal stays within budget
    + a 0-lost / 0-dup counter, dated SCHED run. (Floor named: the seven-figure run is P-FLOW-24.)
- **TESTS (required).** Unit tests: a far-future timer is never read by the wheel scan (assert the partial index is
  used, not a full scan); the FOR UPDATE SKIP LOCKED scan does not double-claim a timer; effectively-once fire
  across a simulated crash. Drill test: the FLOW-D3-floor scenario on the failure-injection harness at 100k+ timers
  with the one-minute burst, asserting tick-budget + 0 lost / 0 dup.
- **DEFINITION OF DONE.** sleep_until/sleep_for + the bucketed wheel compile and run, wired into the consumer slot;
  FLOW-D3 at 100k+ emits its dated green artifact within the tick budget with 0 lost / 0 double-fire; the unit +
  drill tests pass; lints + coverage scanner green; the 100k→1M floor is named with its follow-on P-FLOW-24; the
  disarm/re-arm half is recorded as P-FLOW-14; the work is committed.
- **COMMIT.** Header `P-FLOW-13 M2: durable timer wheel + sleep_until/sleep_for (100k floor)`. Body lists contract
  9.3 (wheel half) + the 9.2 timer half; FLOW-D3-floor greened (timer-wheel-lag within budget, 0 lost/dup at
  100k+); the 1M+ cell-scale floor named with follow-on P-FLOW-24; the re-arm half deferred to P-FLOW-14.
  Co-Authored-By trailer.

---

### P-FLOW-14 — Cheap SLA-timer disarm/re-arm (row-update cost, no wheel pollution)

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.2 (the cheap SLA-timer disarm/re-arm — the Issues ask) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M2.2 "Work" — cheap SLA-timer disarm/re-arm).
- **DEPENDS-ON.** P-FLOW-13 (the wf_timer wheel the re-arm updates).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (the SC-11 property holds under
    churn — millions re-arm at row-update cost, not wheel-scan cost).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §6.6 (cheap SLA-timer disarm/re-arm: a
    re-arm is a row update of fire_at + bucket; a disarm sets fired=true or deletes; no calendar logic on the wheel)
    + §4.2 (the wheel only scans bucket <= now AND NOT fired).
  - planning/05-refined-shared-systems-architecture/contract-index.md row 9.3 (the durable timer wheel — the cheap
    disarm/re-arm half, OWNED here).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.2 (the disarm/re-arm bullet — was blocking, now confirmed)
    + §1 (the 9.3 row).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: the cheap SLA-timer
  disarm/re-arm surface (architecture §6.6) — a re-arm is a single row update of wf_timer.fire_at + its bucket; a
  disarm sets fired=true or deletes the row. Millions of timers re-arm at row-update cost, not wheel-scan cost — no
  calendar logic ever pollutes the wheel. Exposed as a documented helper Issues (SLA timers) and Trigger
  (stale_after) call. (The Issues/Trigger call sites are confirmed-and-tested under their producers in M3,
  P-FLOW-17.)
- **CONTRACTS TO IMPLEMENT.** 9.3 Durable timer wheel — the cheap disarm/re-arm half (owned).
- **GATE / DRILLS (quantified; must be green to call this done).** No new FLOW drill is owed by the re-arm itself
  (FLOW-D3 is the wheel's gate, P-FLOW-13). Structural gate: a re-arm of a precomputed fire_at is a SINGLE row
  update (asserted via the query plan / a single-statement assertion — no wheel pollution, no calendar scan); a
  disarm makes the timer never fire. Green artifact: a dated CI run showing re-arm-is-one-row-update + a
  disarmed-timer-never-fires assertion.
- **TESTS (required).** Unit tests: a re-arm is a single row update (fire_at + bucket changed, no new row, no
  calendar scan); a disarm (fired=true or delete) makes the timer never fire; re-arming N timers is N row updates,
  not a wheel rescan. CDC: the provider half of 9.3 (disarm/re-arm) paired with an Issues/Trigger consumer fixture.
- **DEFINITION OF DONE.** The cheap disarm/re-arm surface compiles and runs; the re-arm-is-one-row-update +
  disarmed-never-fires gate is green and dated; the unit + CDC tests pass; lints + coverage scanner green; the
  Issues/Trigger call-site confirmation is recorded as P-FLOW-17 (M3); the work is committed. (M2.2 is now covered
  across P-FLOW-13..14.)
- **COMMIT.** Header `P-FLOW-14 M2: cheap SLA-timer disarm/re-arm`. Body lists contract 9.3 (disarm/re-arm half)
  owned; the re-arm-is-one-row-update proof; the Issues/Trigger call-site confirmation deferred to P-FLOW-17.
  Co-Authored-By trailer.

---

### P-FLOW-15 — The SCHEDULE_AND_RUN_JOB long-park idiom (dispatch-and-return + park-on-job.done)

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.4 (the long-park idiom — the dispatch/park half) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M2.4 "Work" bullet 1).
- **DEPENDS-ON.** P-FLOW-11 (durable signals + wait_for_signal — the long-park parks on a signal), P-FLOW-13 (the
  durable timer the timeout branch uses). The Agent Fabric M2 prompt that ships the unified runner ToolHands::exec
  (contract 8.4) — the dispatch TARGET. NOTE the band gate: AG-D4 (the sandbox-escape GATE) is owned by Agent
  Fabric/CI and must be green before any SCHEDULE_AND_RUN_JOB dispatch executes untrusted code; this engine
  dispatches into the runner, it does not own the sandbox.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (in-flight never interrupted) +
    external-insights/04-hard-problems.md §5 (untrusted-code execution — the runner is the sandbox, not this engine).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §4.9 (the SCHEDULE_AND_RUN_JOB
    long-park-completed-by-signal idiom — the four-step mechanics: dispatch as a journaled activity minting
    idem_token deterministic on command_id, stamp it on JobSpec{kind: ci|agent}, hand to the unified runner, journal
    activity_completed{job_dispatched: true, idem_token}, and RETURN; then wait_for_signal("job.done",
    idem_key=idem_token) + a timeout timer; idempotent completion) + §3.5 (wf_activity_attempt — the dispatch
    attempt).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.2 (the SCHEDULE_AND_RUN_JOB idiom on
    WfCtx — OWNED) + 9.4 (the job.done durable signal wait — OWNED) + 8.4 (the unified runner — the dispatch target,
    consumed). The reserve/settle bookend is P-FLOW-16.
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.4 (Work bullet 1 + the "Note on the band gate" AG-D4
    paragraph).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: the SCHEDULE_AND_RUN_JOB idiom
  (architecture §4.9) — an ordinary journaled activity that mints idem_token (deterministic from command_id so
  producer and consumer agree without coordination), stamps it on the JobSpec{kind: ci|agent}, hands the spec to the
  unified runner (ToolHands::exec, contract 8.4), journals activity_completed{job_dispatched: true, idem_token}, and
  RETURNS immediately (frees the worker); then wait_for_signal("job.done", idem_key=idem_token) with a timeout timer
  (P-FLOW-13) bounding a vanished runner; idempotent completion (a double-delivered job.done wakes the workflow
  once). This prompt ships the DISPATCH + PARK + IDEMPOTENT-COMPLETION mechanics; reserve/settle is P-FLOW-16,
  re-mint is P-FLOW-17b, loop safety is P-FLOW-18.
- **CONTRACTS TO IMPLEMENT.** 9.2 WfCtx (the SCHEDULE_AND_RUN_JOB idiom) — owned. 9.4 (the job.done wait) — owned.
  Consumes 8.4 ToolHands::exec.
- **GATE / DRILLS (quantified; must be green to call this done).** No standalone FLOW drill greens here (FLOW-D6
  needs reserve/settle, P-FLOW-16; FLOW-D7 needs loop safety, P-FLOW-18). Structural gate: idem_token is
  deterministic from command_id (producer and consumer derive the same key); a double-delivered job.done wakes the
  workflow once; a vanished runner's timeout timer fires and bounds the wait. Green artifact: a dated CI run showing
  the deterministic-idem_token assertion + a 1-wake-per-job counter + a timeout-bounds-vanished-runner assertion.
- **TESTS (required).** Unit tests: idem_token is deterministic from command_id (producer and consumer derive the
  same key); the dispatch activity returns immediately (the worker is freed, the workflow parks); a double-delivered
  job.done wakes the workflow once; a vanished runner's timeout branch fires. CDC: the provider half of 9.2
  (SCHEDULE_AND_RUN_JOB) + 9.4 (job.done wait) paired with a runner consumer fixture.
- **DEFINITION OF DONE.** SCHEDULE_AND_RUN_JOB dispatch + park + idempotent completion compile and run; the
  deterministic-idem_token + 1-wake + timeout-bounds gate is green and dated; the 9.2/9.4 CDC pairs are green; lints
  + coverage scanner green; it is recorded that the dispatch path into the runner is GATED by AG-D4 (Agent
  Fabric/CI-owned) and that no long-park executes untrusted code until AG-D4 is green; reserve/settle (P-FLOW-16),
  re-mint (P-FLOW-17), and loop safety (P-FLOW-18) are recorded as follow-ons; the work is committed.
- **COMMIT.** Header `P-FLOW-15 M2: SCHEDULE_AND_RUN_JOB long-park (dispatch + park + idempotent completion)`. Body
  lists contracts 9.2/9.4 owned + 8.4 consumed; the deterministic-idem_token + 1-wake + timeout proof; the AG-D4
  gating recorded; reserve/settle + re-mint + loop-safety deferred to P-FLOW-16/17/18. Co-Authored-By trailer.

---

### P-FLOW-16 — The reserve/settle bookend on every dispatch (FLOW-D6)

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.4 (reserve-at-dispatch / settle-on-completion — the cost gate) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M2.4 "Work" bullet 2).
- **DEPENDS-ON.** P-FLOW-15 (the SCHEDULE_AND_RUN_JOB dispatch the bookend fronts). The Storage M1 prompt that ships
  the reserve/settle cost gate + the wallet (contract 11.7).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (reserve refuses when exhausted;
    in-flight never interrupted).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §4.9 step 1+4 (reserve at dispatch — no
    balance → no dispatch; settle on the job.done/ci.result signal; never interrupt in-flight; meter into the same
    wallet as a synchronous activity) + §4.4 (the reserve/settle bookend on the synchronous activity) + §8 (F-6
    extended — reserve-at-dispatch for the long-park) + §5.4 (reserve/settle-reject-rate telemetry).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.5 (workflow↔agent mapping; the
    reserve/settle bookend — OWNED) + 11.7 (reserve/settle cost gate — consumed).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.4 (Work bullet 2 + the Exit gate FLOW-D6) + §4 (FLOW-D6
    threshold).
  - testing-strategy/01-whole-system-e2e-and-drill-catalogue.md FLOW-D6 + the F-6 extended assertion (architecture
    §8).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: the reserve/settle bookend
  (contract 9.5/11.7) — RESERVE budget at dispatch (no balance → no dispatch; the job never starts when the wallet
  is exhausted), SETTLE on the job.done/ci.result signal, NEVER interrupt in-flight, meter into the same wallet as a
  synchronous activity. The bookend wraps both the SCHEDULE_AND_RUN_JOB long-park dispatch (P-FLOW-15) and the
  synchronous activity. Plus the reserve/settle-reject-rate telemetry (contract 1.8).
- **CONTRACTS TO IMPLEMENT.** 9.5 workflow↔agent mapping (reserve/settle bookend) — owned. Consumes 11.7
  reserve/settle.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - FLOW-D6 (CI) — a runaway agent loop against a depleting wallet; a new spend-bearing activity (INCLUDING a
    SCHEDULE_AND_RUN_JOB dispatch) is refused at reserve when the wallet is exhausted; an in-flight one is NEVER
    interrupted (settles on completion). Green artifact: reserve-refusal count > 0 + 0-interrupt counter, dated CI.
- **TESTS (required).** Unit tests: a reserve against an empty wallet refuses the dispatch (the job never starts); an
  in-flight job is not interrupted by exhaustion; settle on completion meters into the same wallet as a sync
  activity. Chained drill test: FLOW-D6 (depleting wallet vs dispatch + in-flight) on the failure-injection harness.
  Mutation floor: the reserve/settle gate carries >= 90% cargo-mutants (testing-strategy/00 — "a surviving mutant =
  a runaway spend or a refused-when-funded"); a mutant that drops a reserve check or interrupts an in-flight job
  MUST be caught.
- **DEFINITION OF DONE.** The reserve/settle bookend compiles and runs over the dispatch + the sync activity;
  FLOW-D6 emits its dated green artifact (reserve refusals, 0 interrupt); the unit + drill tests pass; the
  reserve/settle mutation score >= 90%; lints + coverage scanner green; the work is committed.
- **COMMIT.** Header `P-FLOW-16 M2: reserve/settle bookend on every dispatch`. Body lists contract 9.5 owned + 11.7
  consumed; FLOW-D6 (reserve refusals, 0 interrupt) greened; the reserve/settle mutation score. Co-Authored-By
  trailer.

---

### P-FLOW-17 — mint_run_token mid-workflow re-mint on resume (token life == activity life)

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.4 (mid-workflow token re-mint on resume) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M2.4 "Work" bullet 3).
- **DEPENDS-ON.** P-FLOW-11 (the durable wait a days-later resume crosses), P-FLOW-15 (the long-park resume that
  re-mints). The Identity M1 prompt that ships mint_run_token (contract 4.7, callable mid-workflow).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (a multi-day workflow holds no
    long-lived privileged token).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §6.2 (mid-workflow mint_run_token re-mint on
    resume — token life == activity life, not the days-long workflow life; the workflow never holds a long-lived
    privileged token) + §5.2 (the contract 4.7 pin — mint_run_token callable mid-workflow on resume).
  - planning/05-refined-shared-systems-architecture/contract-index.md row 4.7 (mint_run_token mid-workflow re-mint —
    consumed).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.4 (Work bullet 3) + §1 (the 4.7 consumed row).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: mint_run_token mid-workflow
  re-mint on resume (contract 4.7) — when a workflow resumes after a multi-day wait (a HITL approval or a long-park
  job.done), it re-mints a fresh short-lived attenuated per-run token via mint_run_token, so the token life ==
  activity life, not the days-long workflow life. The workflow never holds a long-lived privileged token across a
  wait. Wired into the P-FLOW-11 wait-resume path and the P-FLOW-15 long-park resume path.
- **CONTRACTS TO IMPLEMENT.** None owned. Consumes 4.7 mint_run_token (mid-workflow re-mint).
- **GATE / DRILLS (quantified; must be green to call this done).** No standalone FLOW drill (it is asserted in the
  E2E-2 spine, P-FLOW-27). Structural gate: a days-later resume re-mints a fresh short-lived token (not the
  workflow-lifetime token); the re-minted token is attenuated per-run. Green artifact: a dated CI run showing a
  re-mint-on-resume yields a short-lived token + an attenuation assertion.
- **TESTS (required).** Unit tests: a re-mint on resume yields a short-lived token, not the workflow-lifetime token;
  the re-minted token is attenuated to the run's scope; a resume without a prior token still re-mints. CDC: the
  consumer half of 4.7 (mint_run_token mid-workflow) paired with Identity's provider.
- **DEFINITION OF DONE.** mint_run_token mid-workflow re-mint compiles and runs over the wait-resume + long-park
  resume paths; the re-mint-yields-short-lived-token gate is green and dated; the 4.7 CDC pair is green; lints +
  coverage scanner green; it is recorded that the E2E-2 re-mint assertion lands at P-FLOW-27; the work is committed.
- **COMMIT.** Header `P-FLOW-17 M2: mint_run_token mid-workflow re-mint on resume`. Body lists 4.7 consumed; the
  re-mint-yields-short-lived-token proof; the E2E-2 re-mint assertion recorded as P-FLOW-27. Co-Authored-By trailer.

---

### P-FLOW-18 — Loop safety: causal-depth ceiling + shared-root tripwire + bounded activity pool (FLOW-D7)

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.4 (loop safety — the adversarial-loop stop) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M2.4 "Work" — loop safety).
- **DEPENDS-ON.** P-FLOW-15 (the dispatch the loop self-feeds through), P-FLOW-06 (the causality columns on
  workflow_run the depth ceiling reads). The M0 failure-injection harness (so FLOW-D7 is drillable).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (loop tripwire — drops/parks, never
    forks).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §6.2 (loop safety: causal-depth ceiling +
    shared-root tripwire + bounded activity pool — an adversarial workflow→event→workflow loop is dropped/parked,
    never forked) + §3.1 (the causality columns correlation_id/causation_id/caused_by/depth) + §5.4 (causal-depth-
    histogram telemetry).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.2 (the WfCtx the loop rides) + 1.8
    (causal-depth-histogram telemetry).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.4 (the loop-safety bullet + the Exit gate FLOW-D7) + §4
    (FLOW-D7 threshold).
  - testing-strategy/01-whole-system-e2e-and-drill-catalogue.md FLOW-D7.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: the loop-safety enforcement
  (§6.2) — a causal-depth ceiling (the workflow_run depth column, incremented on each caused-by hop), a shared-root
  tripwire (a workflow→event→workflow loop sharing a correlation root is detected), and a bounded activity pool;
  when the ceiling is hit or the tripwire fires, the run is dropped/parked, NEVER forked. Plus the
  causal-depth-histogram telemetry (contract 1.8).
- **CONTRACTS TO IMPLEMENT.** 9.2 WfCtx — the loop-safety enforcement half (no new owned row; this hardens the
  surface against self-feeding loops). Consumes nothing new.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - FLOW-D7 (CI) — an adversarial workflow→event→workflow loop; the depth ceiling + the shared-root tripwire + the
    bounded activity pool stop it (drops/parks, never forks). Green artifact: the causal-depth signal stays under
    the ceiling + a 0-fork counter, dated CI.
- **TESTS (required).** Unit tests: the depth ceiling halts a self-feeding loop at the ceiling; the shared-root
  tripwire detects a workflow→event→workflow loop; the bounded activity pool caps concurrent activities. Chained
  drill test: FLOW-D7 (the adversarial loop) on the failure-injection harness, asserting causal-depth under the
  ceiling + 0 fork. Mutation floor: the loop-safety path carries >= 90% cargo-mutants (a mutant that lets the depth
  ceiling be exceeded, or forks instead of dropping/parking, MUST be caught).
- **DEFINITION OF DONE.** Loop safety compiles and runs; FLOW-D7 emits its dated green artifact (causal-depth under
  ceiling, 0 fork); the unit + drill tests pass; the loop-safety mutation score >= 90%; lints + coverage scanner
  green; the work is committed. (M2.4's engine half — long-park, reserve/settle, re-mint, loop safety — is now
  covered across P-FLOW-15..18; the merge-queue body is P-FLOW-19.)
- **COMMIT.** Header `P-FLOW-18 M2: loop safety (causal-depth ceiling + tripwire + bounded pool)`. Body lists
  contract 9.2 (loop-safety half); FLOW-D7 (causal-depth, 0 fork) greened; the loop-safety mutation score.
  Co-Authored-By trailer.

---

### P-FLOW-19 — The merge-queue durable workflow body, drilled in isolation against a mock ci.result (M2 exit)

- **BAND.** M2.
- **ROADMAP MILESTONE.** FLOW-M2.4 (the merge-queue workflow frame — the durable-execution half of the X-1 seam,
  built-and-drilled-in-isolation) — roadmap file planning/06-roadmaps/shared/durable-workflow.md §2 (M2.4 "Work"
  bullet 4 + the merge-queue in-isolation drill in the Exit gate).
- **DEPENDS-ON.** P-FLOW-15 (the long-park idiom the merge queue rides), P-FLOW-16 (reserve/settle), P-FLOW-18 (a
  green M2.4 engine half). NOTE: the real ci.result PRODUCER is CI (M4, contract 5.9) — this prompt ships the
  workflow side + the wait, drilled against a MOCK producer; the seam goes live end-to-end in P-FLOW-22 (M4).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §1 (name-your-floors: the
    built-in-isolation merge-queue is a floor whose follow-on is the X-1 seam end-to-end in M4) + §7 (reconcile
    cross-component contracts at the plan layer before either side ships — the idem_token agreement).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §6.5 (the merge-queue durable workflow + the
    ci.result rollup wait — one workflow per target ref; dispatch required CI via SCHEDULE_AND_RUN_JOB; park on
    wait_for_signal("ci.result", idem_key=merge_attempt_id); on success-for-all-required-contexts merge + emit
    git.pr.merged via the outbox + settle; on failure dequeue with a humanised reason; the ci.result payload shape
    {commit_oid, overall, contexts, idem_token}; ci.result-the-rollup-signal vs ci.check.updated-the-events split) +
    §4.9 (the long-park it rides).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.4 (the ci.result wait) + 5.9 (the
    Git↔CI CheckStatus seam — CI/Git own the DATA shape; this engine owns ONLY the durable-workflow mechanics; an
    untrusted_fork success is neutral until endorsed) + 7.3 (humanise — the dequeue reason).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M2.4 (the merge-queue body bullet + the in-isolation drill in
    the Exit gate) + §3 (the floor row: merge-queue-built-in-isolation → X-1 seam end-to-end at M4) + §1 (the 5.9
    consumed row).
  - testing-strategy/01-whole-system-e2e-and-drill-catalogue.md GIT-D10/CI-D8 (the M4 end-to-end the in-isolation
    drill anticipates).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: (a) the merge-queue durable
  workflow body — one workflow per target ref; for each queued PR: compute the speculative merge commit, dispatch
  the required CI via SCHEDULE_AND_RUN_JOB (reserve at dispatch, return), wait_for_signal("ci.result",
  idem_key=merge_attempt_id) parking with no runtime, with the timeout branch bounding a vanished CI run; on a
  success ci.result for ALL required contexts → perform the merge + emit git.pr.merged via the outbox + settle; on
  failure/error → dequeue the PR with a humanised reason (contract 7.3) and continue the queue; (b) a MOCK ci.result
  producer harness fixture (this engine does not own the real producer — that is CI, M4) so the body is drillable in
  isolation. The merge-queue body consumes ONLY the durable-workflow mechanics; it imports the CheckStatus /
  ci.result data shape from the myelin-refs / CI contract crate (5.9) — it does not redefine it.
- **CONTRACTS TO IMPLEMENT.** 9.4 (the ci.result wait — owned, the durable half). Consumes 5.9 (the CheckStatus /
  ci.result data shape, awaiting CI's producer in M4) + 7.3 (humanise). Owns the merge-queue workflow body that the
  Git merge gate (M3) and the CI producer (M4) wire to.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The merge-queue in-isolation drill (CI) — against a MOCK ci.result producer: a double-delivered ci.result for a
    merge_attempt → exactly ONE wake (idempotent on idem_key); a vanished CI run → the timeout branch fires and
    bounds the wait; a success-for-all-required-contexts → exactly one merge + one git.pr.merged emit + one settle;
    a failure → one dequeue with a humanised reason, queue continues. Green artifact: 0 double-merge, 1 wake per
    attempt, dated CI. (NAMED FLOOR: the full GIT-D10/CI-D8 end-to-end against CI's REAL producer is the M4 gate,
    follow-on P-FLOW-22.)
- **TESTS (required).** Unit tests: a double-delivered ci.result wakes once (idem_key); the timeout branch fires on
  a vanished run; only all-required-contexts-green merges; a merge emits exactly one git.pr.merged. Chained drill
  test: the in-isolation merge-queue scenario on the harness with the mock producer (double-delivery → one wake;
  timeout bounds the vanished runner; success → one merge; failure → one dequeue). CDC: the consumer half of 5.9
  (the merge queue consuming a mock ci.result) paired with CI's provider half landing in M4.
- **DEFINITION OF DONE.** The merge-queue workflow body compiles and runs; the in-isolation drill emits its dated
  green artifact (0 double-merge, 1 wake/attempt, timeout-bounded); the unit + drill tests pass; lints + coverage
  scanner green; it is recorded in writing that this is the merge-queue FLOOR (built against a mock producer) with
  the X-1-seam-end-to-end follow-on P-FLOW-22 (M4); the work is committed. (M2.4 — and the whole M2 engine surface
  for myelin-flow — is now covered across P-FLOW-13..19. The M2→M3 band gate is AG-D4, Agent Fabric/CI-owned; this
  system's M2 work is green here.)
- **COMMIT.** Header `P-FLOW-19 M2: merge-queue workflow body (in-isolation, mock ci.result)`. Body lists contract
  9.4 owned + 5.9/7.3 consumed; the in-isolation drill greened (0 double-merge, 1 wake/attempt); the
  merge-queue-in-isolation floor named with follow-on P-FLOW-22. Co-Authored-By trailer.

---

### P-FLOW-20 — Resumable maintenance activities + the history-rewrite invalidation fan-out (M3 support)

- **BAND.** M3.
- **ROADMAP MILESTONE.** FLOW-M3 (the resumable maintenance activities Git's M3 maintenance rides) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M3).
- **DEPENDS-ON.** P-FLOW-19 (the full M2 engine, including the long-park heavy maintenance rides). The Git M3
  prompts that build GC/repack/bundle-gen/history-rewrite (consumers) — co-built; this prompt ships the myelin-flow
  helper.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (a crash mid-repack replays to the
    un-journaled step — exercised, not asserted) + external-insights/04-hard-problems.md §1 (history-rewrite
    erasure as an audited follow-on).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §6.6 (resumable maintenance activities — Git
    GC/repack/bundle-gen/history-rewrite as resumable journaled activities or SCHEDULE_AND_RUN_JOB long-parks; the
    history-rewrite invalidation fan-out as a sequence of journaled activities over the trust-scoped cache
    namespaces) + §4.1 (replay-to-the-un-journaled-step).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.3 (the activity model + wheel the
    maintenance rides) + 10.6 (the history-rewrite erasure-admin op — consumed by Git) + 11.2 (the trust-scoped
    cache namespaces the invalidation fan-out touches).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M3 (Work + Exit gate — no new FLOW drill is owed in M3;
    GIT-D9 is Git's gate).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: (a) a reusable
  resumable-maintenance-activity helper so Git's GC / repack / bundle-gen / history-rewrite run as journaled
  activities (or SCHEDULE_AND_RUN_JOB long-parks for the heavy ones) on a workflow — a crash mid-repack replays to
  the un-journaled step (§4.1) with no re-executed side effect; (b) the history-rewrite invalidation fan-out
  expressed as a sequence of journaled activities (fork/mirror/clone-cache → the trust-scoped cache namespaces,
  contract 11.2). NO new engine primitive — this is the application of the existing activity model (§4.4) to the M3
  maintenance work.
- **CONTRACTS TO IMPLEMENT.** Consumes 10.6 (history-rewrite) + 11.2 (cache namespaces) — wires the call sites
  Git invokes.
- **GATE / DRILLS (quantified; must be green to call this done).** No NEW FLOW drill is owed in M3 (roadmap §2 M3
  Exit gate). The gate here: a maintenance-activity crash-and-resume test proves a crash mid-repack replays to the
  un-journaled step with 0 re-executed side effect (the FLOW-D1 property reused on a maintenance workflow); the
  invalidation fan-out replays from the last journaled step. Green artifact: a dated CI run showing
  crash-mid-repack-resumes-with-no-side-effect + the fan-out-replays-from-last-step assertion.
- **TESTS (required).** Unit tests: a journaled maintenance activity replays to the un-journaled step (0
  re-execution); the invalidation fan-out is a journaled sequence (replays from the last journaled step). Drill
  test: a crash-mid-repack scenario on the harness asserting resume-with-no-side-effect.
- **DEFINITION OF DONE.** The resumable-maintenance helper + the invalidation fan-out compile and run; the
  crash-mid-repack resume test passes; lints + coverage scanner green; it is recorded that no new FLOW drill is owed
  in M3 (Git owns GIT-D9); the cheap SLA re-arm confirmation under Issues/Trigger is recorded as P-FLOW-21; the work
  is committed.
- **COMMIT.** Header `P-FLOW-20 M3: resumable maintenance activities + history-rewrite invalidation fan-out`. Body
  lists 10.6/11.2 consumed; the crash-mid-repack-resumes proof; no new FLOW drill owed in M3; the re-arm
  confirmation deferred to P-FLOW-21. Co-Authored-By trailer.

---

### P-FLOW-21 — The cheap SLA-timer re-arm confirmed under Git/Issues + the merge-queue holds-no-runtime re-green (M3)

- **BAND.** M3.
- **ROADMAP MILESTONE.** FLOW-M3 (the M3 confirmation that the existing timer re-arm + merge-queue hold under the
  Git/Issues call sites) — roadmap file planning/06-roadmaps/shared/durable-workflow.md §2 (M3).
- **DEPENDS-ON.** P-FLOW-14 (the cheap disarm/re-arm surface this confirms under the Issues/Trigger call sites),
  P-FLOW-19 (the merge-queue in-isolation drill this re-greens), P-FLOW-20 (the M3 maintenance co-built with Git).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (the SC-11 property holds under
    churn) + §1 (a confirmation is a re-run green artifact, not a doc claim).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §6.6 (the cheap SLA-timer disarm/re-arm
    under the Issues call site — a re-arm is a row update of fire_at + bucket, no calendar logic on the wheel) +
    §4.2 (the wheel) + §7 (a parked merge-queue wait is a row, not a runtime).
  - planning/05-refined-shared-systems-architecture/contract-index.md row 9.3 (the cheap re-arm — confirmed under
    the Issues call site).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M3 (Exit gate — the merge-queue workflow holds no runtime
    across the wait, re-confirmed by the M2.4 in-isolation drill; no new FLOW drill owed).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: (a) confirm-and-test the cheap
  SLA-timer disarm/re-arm (P-FLOW-14) under the Git/Issues call sites — expose it as a documented helper Issues (SLA
  timers) and Trigger (stale_after) call, with a call-site test proving a re-arm is a single row update at the
  Issues call boundary; (b) re-run the P-FLOW-19 merge-queue in-isolation drill green, re-confirming the merge-queue
  workflow holds no runtime across the wait (the relevant M3 assertion). NO new engine primitive — this is the M3
  confirmation pass for the timer + merge-queue surfaces under their real call sites.
- **CONTRACTS TO IMPLEMENT.** 9.3 (the cheap re-arm — confirmed under the Issues call site). No new owned row.
- **GATE / DRILLS (quantified; must be green to call this done).** No NEW FLOW drill is owed in M3. The gate here:
  the cheap re-arm test proves a re-arm is a single row update at the Issues/Trigger call boundary (no wheel
  pollution); the P-FLOW-19 merge-queue-in-isolation drill re-greens (merge-queue holds no runtime across the wait).
  Green artifact: a dated CI run showing re-arm-is-row-update-at-call-site + merge-queue-in-isolation re-green.
- **TESTS (required).** Unit tests: a re-arm of a precomputed fire_at at the Issues call site is a single row
  update; the Trigger stale_after re-arm uses the same path. Drill test: re-run the P-FLOW-19 merge-queue
  in-isolation scenario, asserting holds-no-runtime + 0 double-merge re-green.
- **DEFINITION OF DONE.** The cheap re-arm is confirmed-and-tested at the Git/Issues call sites; the P-FLOW-19
  in-isolation drill re-greens; lints + coverage scanner green; it is recorded that no new FLOW drill is owed in M3
  (Git owns GIT-D9); the work is committed. (M3 is covered across P-FLOW-20..21.)
- **COMMIT.** Header `P-FLOW-21 M3: cheap SLA-timer re-arm confirmed + merge-queue holds-no-runtime re-green`. Body
  lists contract 9.3 (re-arm confirmed under the call site); the re-arm-is-row-update proof + the merge-queue
  in-isolation re-green; no new FLOW drill owed in M3. Co-Authored-By trailer.

---

### P-FLOW-22 — The CI-pipeline-as-workflow substrate + reference fixture (CI-D9, CI-D1)

- **BAND.** M4.
- **ROADMAP MILESTONE.** FLOW-M4 (CI pipelines as durable workflows — the myelin-flow substrate contribution) —
  roadmap file planning/06-roadmaps/shared/durable-workflow.md §2 (M4 "Work (myelin-flow contribution)" — CI's
  pipeline-as-workflow uses WfCtx + SCHEDULE_AND_RUN_JOB + the flow-determinism lint).
- **DEPENDS-ON.** P-FLOW-19 (the full M2 engine + the long-park). The M3 Git prompts merged (the band gate: M4
  starts only after M3 green). The CI M4 prompts that own the ci.pipeline definition + the CheckStatus producer
  (co-built; this prompt provides the durable-execution substrate CI's pipeline body sits on). AG-D4 green (the CI
  runner is the unified sandbox).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (replay bit-identical; only journaled
    job.done feeds the body) + §5 (the flow-determinism lint is a committed gate).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §4.9 (SCHEDULE_AND_RUN_JOB for each long CI
    stage) + §2 (the flow-determinism constraint: no clock/RNG/IO outside WfCtx) + "Changes vs Phase 3" item 6
    (CI-pipeline-as-workflow stage/step granularity answered via SCHEDULE_AND_RUN_JOB + the unified-runner kind=ci
    job spec, X-6).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.2 (WfCtx + SCHEDULE_AND_RUN_JOB, the
    surface CI's pipeline body is written against) + 1.6 (the flow-determinism lint) + 5.9 (the CheckStatus producer
    CI owns) + 11.7 (reserve/settle on every CI stage).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M4 (Work + the Exit gate CI-D9 line) + §4 (CI-D9 / CI-D1
    rows).
  - testing-strategy/01-whole-system-e2e-and-drill-catalogue.md CI-D1 + CI-D9.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: the documented, test-covered
  CI-pipeline-as-workflow PATTERN + a reference ci.pipeline workflow fixture that CI's M4 build uses — a
  deterministic WfCtx workflow body whose every long stage is a SCHEDULE_AND_RUN_JOB dispatch (kind=ci) into the
  unified runner, with reserve/settle on each stage, and the flow-determinism lint applied to the body (no
  clock/RNG/IO outside WfCtx). The reference fixture proves the determinism + replay-bit-identical +
  only-journaled-job.done properties. This prompt does NOT build CI's pipeline definitions or the CheckStatus
  producer (those are CI's M4 deliverable); it builds the substrate they sit on.
- **CONTRACTS TO IMPLEMENT.** 9.2 (WfCtx + SCHEDULE_AND_RUN_JOB — the CI-pipeline surface) — owned. Consumes 5.9
  (CI's CheckStatus producer, awaited) + 11.7 (reserve/settle per stage) + 1.6 (the flow-determinism lint).
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CI-D9 (CI, exercises this engine) — the ci.pipeline workflow body (the reference fixture): no clock/RNG/IO
    outside WfCtx (flow-determinism lint passes); a replay is bit-identical; ONLY a journaled job.done feeds the
    body. Green artifact: replay-bit-identical proof + flow-determinism lint green on the body, dated CI.
  - CI-D1 (CI, exercises this engine) — kill the runner + the control plane mid-run; the run resumes (replay +
    SCHEDULE_AND_RUN_JOB idempotent re-dispatch); effectively-once; 0 lost runs, 0 double-deploys, 0 duplicate
    publishes. Green artifact: replay-rate + 0-double-effect on the reference pipeline, dated CI.
- **TESTS (required).** Unit tests: the reference pipeline body compiles under the flow-determinism lint (and a body
  with a raw SystemTime read fails the lint); a replay of the body is bit-identical; an idempotent re-dispatch after
  a kill produces one job, not two. Chained drill tests: CI-D1 (runner + control-plane kill → replay + idempotent
  re-dispatch) and CI-D9 (determinism + replay-bit-identical) on the harness against the reference pipeline fixture.
- **DEFINITION OF DONE.** The CI-pipeline-as-workflow substrate + the reference fixture compile and run; CI-D9 and
  CI-D1 each emit a dated green artifact against the reference pipeline; the unit + drill tests pass; the
  flow-determinism lint is green on the body and rejects a non-deterministic one; lints + coverage scanner green;
  it is recorded that CI's real pipeline definitions + the CheckStatus producer are CI's M4 deliverable; the work
  is committed.
- **COMMIT.** Header `P-FLOW-22 M4: CI-pipeline-as-workflow substrate + reference fixture`. Body lists contract 9.2
  (CI-pipeline surface) + 5.9/11.7/1.6 consumed; CI-D9 (replay-bit-identical, lint green) + CI-D1 (replay +
  idempotent re-dispatch) greened; CI's real pipelines recorded as CI's M4 deliverable. Co-Authored-By trailer.

---

### P-FLOW-23 — The X-1 seam end-to-end: the merge-queue long-park wakes on the real ci.result (GIT-D10/CI-D8)

- **BAND.** M4.
- **ROADMAP MILESTONE.** FLOW-M4 (the X-1 seam end-to-end — the merge-queue floor's follow-on against CI's REAL
  ci.result producer) — roadmap file planning/06-roadmaps/shared/durable-workflow.md §2 (M4 thesis + Exit gate
  GIT-D10/CI-D8).
- **DEPENDS-ON.** P-FLOW-19 (the merge-queue body built in isolation — this prompt is its named follow-on),
  P-FLOW-22 (the CI-pipeline substrate). The CI M4 prompt that ships the real CheckStatus/ci.result PRODUCER
  (contract 5.9) and the Git M3 prompt that ships the merge gate consuming the merge-queue body. AG-D4 green.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §7 (reconcile cross-component contracts
    — the idem_token agreement across the scheduler boundary) + §3 (0 double-merge is the quantified gate; never
    weaken it).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §6.5 (the merge-queue durable workflow + the
    ci.result rollup wait — now wired to the REAL producer; the ci.result-rollup vs ci.check.updated-events split) +
    §4.9 (the long-park) + the "Changes vs Phase 3" item 3.
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 5.9 (the Git↔CI CheckStatus seam — keyed
    (commit_oid, context), last-writer-wins by run_attempt, untrusted_fork success neutral until endorsed; CI is the
    producer, Git the gate, this engine the durable-workflow mechanics) + 9.4 (the ci.result wait).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M4 (Exit gate GIT-D10/CI-D8 + the note that the
    durable-execution half is this engine's long-park + wait_for_signal) + §3 (the floor row:
    merge-queue-in-isolation → X-1 seam end-to-end at M4, trigger = CI's CheckStatus producer ships).
  - testing-strategy/01-whole-system-e2e-and-drill-catalogue.md GIT-D10/CI-D8.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: replace the P-FLOW-19 MOCK
  ci.result producer with the REAL wiring — the merge-queue workflow now subscribes to and wakes on CI's live
  ci.result rollup signal (contract 5.9), keyed by merge_attempt_id, idempotently. No new engine primitive: this is
  the floor's follow-on (the mock → the real producer), proving the long-park + idempotent-signal mechanics
  end-to-end with Git's merge gate and CI's producer. The CheckStatus/ci.result data shape is imported from the 5.9
  contract crate (CI-owned); this engine owns only the durable-workflow half.
- **CONTRACTS TO IMPLEMENT.** 9.4 (the ci.result wait — now end-to-end). Consumes 5.9 (CI's real CheckStatus /
  ci.result producer). Closes the durable-execution half of the X-1 seam.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - GIT-D10 / CI-D8 (CI) — push → ci.check.updated per context → green → merge; out-of-order / re-delivered
    ci.check.updated → run_attempt supersession; a fork self-green is neutral for gating; the merge-queue workflow
    wakes on ci.result idempotently — 0 double-merge, 0 spurious unblocks. Green artifact: the correct merge row + a
    0-double-merge counter (merge-count == 1 per attempt), dated CI. (This is the most load-bearing cross-subsystem
    contract; the durable-execution half is this engine's long-park + wait_for_signal. Never invert the
    0-double-merge assertion to pass.)
- **TESTS (required).** Unit tests: a doubly-delivered ci.result wakes the merge queue once (idem_key =
  merge_attempt_id); a fork self-green ci.result does not unblock the merge (neutral until endorsed); out-of-order
  ci.check.updated supersedes by run_attempt before the rollup. Chained drill test: GIT-D10/CI-D8 end-to-end on the
  harness (push → checks → ci.result → merge), asserting 0 double-merge across re-delivery + restart. CDC: the
  consumer half of 5.9 (the merge queue) now paired with CI's real provider half — the contract-coverage scanner
  must show both green.
- **DEFINITION OF DONE.** The merge queue wakes on the real ci.result; GIT-D10/CI-D8 emits its dated green artifact
  (0 double-merge, merge-count == 1/attempt) across re-delivery and restart; the unit + drill tests pass; the 5.9
  provider+consumer CDC pair is green; lints + coverage scanner green; it is recorded that the
  merge-queue-in-isolation FLOOR (P-FLOW-19) is now promoted to the seam end-to-end (the floor is closed); the work
  is committed.
- **COMMIT.** Header `P-FLOW-23 M4: X-1 seam end-to-end (merge-queue wakes on real ci.result)`. Body lists contract
  9.4 end-to-end + 5.9 consumed (CDC pair green); GIT-D10/CI-D8 greened (0 double-merge); the merge-queue floor
  promoted from in-isolation to end-to-end. Co-Authored-By trailer.

---

### P-FLOW-24 — Crypto-shred reaching history: the PersonalDataHolder erase path completed (FLOW-D9)

- **BAND.** M5.
- **ROADMAP MILESTONE.** FLOW-M5 (the erasure half — crypto-shred reach into history) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M5 "Work — world-scale hardening" — crypto-shred).
- **DEPENDS-ON.** P-FLOW-23 (the X-1 seam end-to-end — M4 green; the band gate). The KMS/per-subject-DEK crypto-shred
  substrate (contract 11.3/11.4) it builds on. P-FLOW-03 (the references-not-payloads structural floor whose
  crypto-shred reach this prompt completes — this is P-FLOW-03's named follow-on).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (crypto-shred unrecoverable incl.
    backups) + external-insights/04-hard-problems.md §1 (erasure-vs-immutability; references-not-payloads +
    crypto-shred + tombstone).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §4.8 (GDPR erasure on history via
    references-not-payloads + crypto-shred + tombstone; structure preserved) + §5.5 (PersonalDataHolder over
    workflow_run/wf_history/wf_signal; payload_key_ref crypto-shred) + §3.2/§3.4 (result_key_ref / payload_key_ref,
    the per-subject-DEK envelope).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.6 (PersonalDataHolder + replay —
    crypto-shred reach now complete) + 11.3/11.4 (KMS hierarchy + per-subject DEK — consumed).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M5 (the FLOW-D9 bullet) + §4 (FLOW-D9 threshold) + §3 (no
    floor remains on crypto-shred — this completes the P-FLOW-03 structural floor).
  - testing-strategy/01-whole-system-e2e-and-drill-catalogue.md FLOW-D9.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: complete the PersonalDataHolder
  erase path (contract 9.6) — erasing a subject with inline-PII history/signal rows DESTROYS the per-subject DEK so
  the result_key_ref / payload_key_ref ciphertext is unrecoverable INCLUDING in backups, TOMBSTONES the references,
  and PRESERVES the structure (the journal shape survives, the PII does not — replay still works, the PII is a
  tombstone). Plus the crypto-shred-lag telemetry (contract 1.8). This is the named follow-on to P-FLOW-03's
  structural-floor crypto-shred reach.
- **CONTRACTS TO IMPLEMENT.** 9.6 PersonalDataHolder + replay — owned, now COMPLETE (the erase/crypto-shred reach).
  Consumes 11.3/11.4 KMS + per-subject DEK.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - FLOW-D9 (SCHED) — erase a subject with inline-PII history/signal rows; the keys are destroyed (unrecoverable
    including in backups), the references tombstoned, the structure preserved. Green artifact: the crypto-shred-lag
    signal + a 0-recoverable-PII assertion (including a backup-restore-then-read attempt that fails to decrypt),
    dated SCHED.
- **TESTS (required).** Unit tests: erasing a subject destroys the per-subject DEK and the inline-PII ciphertext is
  undecryptable; the journal structure survives the erase (replay still works, the PII is a tombstone). Chained
  drill test: FLOW-D9 (erase → 0 recoverable incl. a backup-restore attempt) on the harness. Mutation floor: the
  crypto-shred / erasure key-selection path carries >= 95% cargo-mutants (testing-strategy/00 — "a surviving mutant
  = PII that survives erasure"); a mutant that selects the per-tenant DEK instead of the per-subject DEK, or skips
  the shred, MUST be caught.
- **DEFINITION OF DONE.** The crypto-shred reach compiles and runs; FLOW-D9 emits its dated green artifact (0
  recoverable incl. backups); the unit + drill tests pass; the crypto-shred mutation score >= 95%; lints + coverage
  scanner green; the P-FLOW-03 crypto-shred-reach FLOOR is recorded as now closed; the restore-verify follow-on is
  recorded as P-FLOW-25; the work is committed.
- **COMMIT.** Header `P-FLOW-24 M5: crypto-shred reaching history (PersonalDataHolder erase complete)`. Body lists
  contract 9.6 completed + 11.3/11.4 consumed; FLOW-D9 (0 recoverable incl. backups) greened; the crypto-shred
  mutation score; the P-FLOW-03 floor closed; restore-verify deferred to P-FLOW-25. Co-Authored-By trailer.

---

### P-FLOW-25 — Restore-verify to a consistent point: in-flight runs resume, no vanished result (FLOW-D10)

- **BAND.** M5.
- **ROADMAP MILESTONE.** FLOW-M5 (the durability half — restore-verify to a consistent point) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M5 "Work — world-scale hardening" — restore-verify).
- **DEPENDS-ON.** P-FLOW-24 (the crypto-shred reach — the M5 erasure/durability half is built together). The Storage
  M1 restore-verify CI job (STOR-D1/D2, contract 11.5) it builds on.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (restore to a consistent point) +
    external-insights/04-hard-problems.md §1.
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §7 (restore + cross-seam integrity, F-10 —
    store ↔ outbox offsets ↔ referenced rows at one consistent point) + §4.1 (in-flight runs resume on replay).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 11.5 (backup/restore + restore-verify —
    consumed) + 9.6 (the holder whose references must point at live rows after restore).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M5 (the FLOW-D10 bullet) + §4 (FLOW-D10 threshold).
  - testing-strategy/01-whole-system-e2e-and-drill-catalogue.md FLOW-D10.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: the restore-verify integration
  with Storage's restore (contract 11.5) — after a restore to a consistent point, in-flight runs resume, and store
  ↔ outbox offsets ↔ referenced rows are at ONE consistent point (no run pointing at a vanished result). Plus the
  restore-verify telemetry (contract 1.8).
- **CONTRACTS TO IMPLEMENT.** 9.6 PersonalDataHolder + replay — the restore-consistency half. Consumes 11.5
  restore-verify.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - FLOW-D10 (SCHED) — restore the myelin-flow Postgres to a consistent point; in-flight runs resume; store ↔
    outbox offsets ↔ referenced rows are at one consistent point; no run points at a vanished result. Green
    artifact: the restore-verify signal + a consistent-point assertion, dated SCHED.
- **TESTS (required).** Unit tests: after a restore, an in-flight run resumes via replay; no wf_history row points
  at a result that vanished in the restore; store ↔ outbox offsets reconcile. Chained drill test: FLOW-D10 (restore
  → consistent-point resume) on the harness.
- **DEFINITION OF DONE.** The restore-verify integration compiles and runs; FLOW-D10 emits its dated green artifact
  (consistent-point restore); the unit + drill tests pass; lints + coverage scanner green; the work is committed.
  (The M5 erasure + durability half is now covered across P-FLOW-24..25.)
- **COMMIT.** Header `P-FLOW-25 M5: restore-verify to a consistent point`. Body lists contract 9.6 (restore half) +
  11.5 consumed; FLOW-D10 (consistent-point restore) greened. Co-Authored-By trailer.

---

### P-FLOW-26 — World-scale: the 1M+ timer cell-scale run + the per-cell promotion threshold (FLOW-D3 full)

- **BAND.** M5.
- **ROADMAP MILESTONE.** FLOW-M5 (the scale half — 1M+ timers; the named 100k-timer floor's follow-on) — roadmap
  file planning/06-roadmaps/shared/durable-workflow.md §2 (M5 "Work — world-scale hardening" — the 1M+ timer run +
  the per-cell timer-wheel-promotion threshold).
- **DEPENDS-ON.** P-FLOW-13 (the timer wheel + the 100k floor — this prompt is its named follow-on), P-FLOW-23 (M4
  green). The M0 failure-injection harness's load generator at 1M+ scale.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (the 1x/10x/30x load generator;
    observability is part of the pass).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §7.3 (the SC-11 millions-of-timers case +
    the per-cell timer-wheel-promotion threshold, OQ #5) + §7 (the long-park does not change the scaling story — a
    parked wait is a row, not a runtime).
  - planning/05-refined-shared-systems-architecture/contract-index.md row 9.3 (the timer wheel at scale) + 1.8
    (timer-wheel-lag telemetry).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M5 (the FLOW-D3 full bullet + the per-cell
    timer-wheel-promotion threshold measured) + §3 (the 100k → 1M floor row + the trigger = measured due-now rate,
    OQ #5) + §4 (FLOW-D3 full threshold).
  - testing-strategy/01-whole-system-e2e-and-drill-catalogue.md FLOW-D3 (full).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: the prod-scale timer run + any
  worker-sharding/wheel tuning the 1M+ scale needs (the algorithm is unchanged from P-FLOW-13 — this proves it at
  seven figures and MEASURES the per-cell timer-wheel-promotion threshold, OQ #5: the due-now rate at which the
  PG-indexed wheel yields to a dedicated scheduling tier; the threshold is recorded in the thresholds file, the
  dedicated tier itself is a named follow-on IF the measured rate demands it). This is the named follow-on to
  P-FLOW-13's 100k-timer floor.
- **CONTRACTS TO IMPLEMENT.** 9.3 (the timer wheel at cell scale — the floor follow-on). No new owned row.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - FLOW-D3 full (SCHED, cell scale) — arm 1M+ durable timers + a burst due in one minute; the due timers fire
    within the tick budget; far-future timers are ~free; a crash re-fires only the unfired. 0 lost, 0 double-fire.
    Green artifact: the timer-wheel-lag signal within budget at 1M+ + a 0-lost / 0-dup counter, dated SCHED; the
    per-cell promotion threshold recorded in the thresholds file.
- **TESTS (required).** Drill test (scheduled, not CI-cheap): FLOW-D3 at 1M+ on the harness asserting tick-budget +
  0 lost/dup. Unit tests: the worker-sharding split at 1M+ does not double-claim a timer; the promotion-threshold
  measurement reads the due-now rate.
- **DEFINITION OF DONE.** The 1M+ timer run runs and passes; FLOW-D3 full emits a dated green artifact; the per-cell
  timer-wheel-promotion threshold is measured and recorded in the thresholds file (with the dedicated-scheduling-tier
  named as a follow-on IF the measured rate demands it); the drill + unit tests pass; lints + coverage scanner
  green; the P-FLOW-13 100k-timer FLOOR is recorded as now closed; the work is committed.
- **COMMIT.** Header `P-FLOW-26 M5: 1M+ timer cell-scale run + per-cell promotion threshold`. Body lists contract
  9.3 at cell scale; FLOW-D3 full (1M+ within tick budget) greened; the measured promotion threshold; the 100k
  floor closed. Co-Authored-By trailer.

---

### P-FLOW-27 — World-scale: the 30x agent-workflow surge with lane shedding (FLOW-D8)

- **BAND.** M5.
- **ROADMAP MILESTONE.** FLOW-M5 (the surge half — the 30x agent-workflow surge with lane shedding) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M5 "Work — world-scale hardening" — the 30x surge).
- **DEPENDS-ON.** P-FLOW-16 (reserve/settle + the per-surface shed budgets the surge sheds against), P-FLOW-26 (the
  scale-half timer run co-built). The M0 failure-injection harness's 30x load generator + per-surface storm
  profiles (contract 1.11 shed order).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §3 (the 30x load generator; the
    protected-human-lane shed order; observability is part of the pass).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §7.6 (bounded everything with the
    principal-aware shed order: speculative → batch/CI → agent → human-last) + §7 (the per-surface shed budgets name
    CI-dispatch as a bounded run-queue per tenant; an agent-mention storm sheds its lane with 429 + Retry-After
    while human-initiated workflows hold the protected lane — F-8).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 1.11 (the protected-human-lane shed order
    — consumed) + 1.8 (shed-counts/lane telemetry).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M5 (the FLOW-D8 bullet) + §4 (FLOW-D8 threshold).
  - testing-strategy/01-whole-system-e2e-and-drill-catalogue.md FLOW-D8.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow: the per-surface shed
  enforcement under the 30x agent-workflow surge (contract 1.11) — the human-initiated lane holds within budget, the
  agent lane sheds with 429 + Retry-After, other tenants are unaffected. Plus the shed-counts/lane telemetry
  (contract 1.8).
- **CONTRACTS TO IMPLEMENT.** Consumes 1.11 (the shed order). No new owned row (the shed enforcement applies the
  substrate's shed budgets to the workflow lanes).
- **GATE / DRILLS (quantified; must be green to call this done).**
  - FLOW-D8 (SCHED) — a 30x surge of agent-initiated workflows; the human-initiated lane HOLDS, the agent lane SHEDS
    (429 + Retry-After), other tenants are unaffected. Green artifact: the shed-counts/lane signal showing the agent
    lane shedding while the human lane stays within budget, dated SCHED.
- **TESTS (required).** Drill test (scheduled): FLOW-D8 at 30x on the harness asserting human-lane-holds +
  agent-lane-sheds + cross-tenant-0-impact. Unit tests: the shed order routes a human-initiated workflow ahead of an
  agent-initiated one under saturation; a 429 carries a Retry-After.
- **DEFINITION OF DONE.** The 30x surge shed enforcement runs and passes; FLOW-D8 emits its dated green artifact
  (human lane holds, agent sheds); the drill + unit tests pass; lints + coverage scanner green; the work is
  committed. (M5's scale half is now covered across P-FLOW-26..27.)
- **COMMIT.** Header `P-FLOW-27 M5: 30x agent-workflow surge with lane shedding`. Body lists contract 1.11 consumed;
  FLOW-D8 (human lane holds, agent sheds) greened. Co-Authored-By trailer.

---

### P-FLOW-28 — The E2E-2 flagship: the durable-workflow + HITL spine across the kill + days-later approval

- **BAND.** M5.
- **ROADMAP MILESTONE.** FLOW-M5 (the whole-system E2E wedge — myelin-flow's role in E2E-2, the agent-native
  flagship) — roadmap file planning/06-roadmaps/shared/durable-workflow.md §2 (M5 "Work — the E2E wedge").
- **DEPENDS-ON.** P-FLOW-24 (crypto-shred), P-FLOW-25 (restore-verify), P-FLOW-26 + P-FLOW-27 (the scale drills) —
  myelin-flow's M5 hardening green. P-FLOW-17 (the re-mint the spine asserts), P-FLOW-23 (the X-1 seam the
  merge-queue wakes on). The Agent Fabric, CI, Issues, Chat, Git M3/M4 prompts (E2E-2 spans CI, Agent, Workflow,
  Issues, Chat, Git, Id, Notif, Storage); this prompt owns the durable-workflow + HITL SPINE, co-built with the
  owning subsystems' E2E prompts.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §4 (chain mutations end-to-end; drive
    the real thing) + §3 (exactly-once across a kill is the quantified gate).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §8 (F-4 extended — the long-park + the
    per-effect idem_key asserted across a restart+deploy with a days-later double-click) + §6.2 (mid-workflow token
    re-mint on resume) + §6.5 (the merge-queue wakes on ci.result).
  - planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    E2E-2 (CI-fail → triage agent → issue → chat → fix-PR — the assert: 0 effect outside the intersection; 0
    mutation before approval; exactly-once approval + merge across a kill; reserve/settle balanced; merge-count ==
    1) + the E2E-2 narrative describing the durable-workflow resume (FLOW-D4), the exactly-once consume, the token
    re-mint (4.7), the merge-applies-once (FLOW-D1), the merge-queue wake on ci.result (X-1).
  - planning/06-roadmaps/shared/durable-workflow.md §2 M5 (the E2E wedge paragraph — myelin-flow's spine) + §4 (the
    E2E-2 row).
  - planning/05-refined-shared-systems-architecture/contract-index.md rows 9.1/9.4 (signal + the wait), 4.7
    (re-mint), 5.9 (the ci.result the merge-queue wakes on), 11.7 (reserve/settle parity).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow (the E2E test harness scope):
  the durable-workflow + HITL SPINE of E2E-2 as an end-to-end scenario test against a full cell with MOCK agents
  (VISION §3 — no real agents during development) — a failing CI run wakes a mock triage agent whose run is a
  myelin-flow workflow; the open_pr / git.merge effect is HITL-gated (wait_for_signal); the Agent + Workflow
  services are KILLED mid-ack_window; the approval arrives days later as a double-click; the workflow RESUMES
  (FLOW-D4), consumes the approval EXACTLY ONCE, re-mints the run token on resume (contract 4.7), and the merge
  applies ONCE (FLOW-D1, no double-effect); the fix-PR's CI goes green → the merge-queue workflow wakes on ci.result
  idempotently (X-1) and merges; reserve/settle is balanced across the whole run. This prompt owns the myelin-flow
  assertions in the shared E2E-2 scenario; the other subsystems' E2E prompts own their faces.
- **CONTRACTS TO IMPLEMENT.** No new owned contract — this exercises 9.1/9.4 (signal + wait), 4.7 (re-mint), 5.9
  (the merge-queue wake), 9.5/11.7 (reserve/settle parity) end-to-end in the flagship scenario.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - E2E-2 (SCHED), the durable-workflow + HITL spine — the agent run resumes after the kill, consumes the
    days-later approval exactly once (1 HITL withhold→approve→apply ledger entry), re-mints the run token on resume,
    the merge applies exactly once (merge-count == 1), the merge-queue wakes on ci.result idempotently, and
    reserve/settle is balanced (the parity assertion). Green artifact: the deterministic run trace + the HITL
    withhold→approve→apply ledger + reserve/settle parity + merge-count == 1, dated SCHED. (Never invert the
    merge-count == 1 or the exactly-once assertions to pass.)
- **TESTS (required).** The E2E-2 scenario test on the full-cell harness with mock agents, chaining the mutations
  (CI-fail → agent workflow → HITL gate → kill → days-later double-click → resume → re-mint → merge-once →
  fix-PR-CI-green → merge-queue-wake) — preferred over any single-handler test (EI-01 §4, the scenario IS a sequence
  property). Assert: 0 mutation before approval; exactly-once approval + merge across the kill; reserve/settle
  balanced; merge-count == 1.
- **DEFINITION OF DONE.** The E2E-2 durable-workflow + HITL spine scenario runs against a full cell with mock agents
  and emits its dated green artifact (run trace + HITL ledger + reserve/settle parity + merge-count == 1); the
  scenario test passes across the kill + the days-later approval; lints + coverage scanner green; any part of the
  scenario owned by another subsystem is recorded as such (this prompt owns only the workflow + HITL spine); the
  work is committed. (M5 for myelin-flow is now covered across P-FLOW-24..28.)
- **COMMIT.** Header `P-FLOW-28 M5: E2E-2 durable-workflow + HITL spine (agent-native flagship)`. Body lists the
  contracts exercised (9.1/9.4/4.7/5.9/9.5/11.7); E2E-2 spine greened (exactly-once across the kill, merge-count ==
  1, reserve/settle balanced); the cross-subsystem faces recorded as their owners'. Co-Authored-By trailer.

---

### P-FLOW-29 — Dogfooding: Myelin's own pipelines / merge queue / SLA timers as myelin-flow workflows

- **BAND.** M6.
- **ROADMAP MILESTONE.** FLOW-M6 (dogfooding — no new engine work; the self-hosting truth-up) — roadmap file
  planning/06-roadmaps/shared/durable-workflow.md §2 (M6).
- **DEPENDS-ON.** P-FLOW-28 (M5 green — the gate invariant: you do not dogfood real team data onto a substrate whose
  restore-verify + DSAR fan-out are not green). The M6 dogfood prompts that migrate the Myelin monorepo onto Myelin
  git hosting + stand up the self-hosting CI graph.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md and external-insights/01-process-and-quality-doctrine.md §1 (code-wins-over-docs; the truth-up
    pass — date every status note) + §5 (the mandatory-core mutation gate now runs as a Myelin CI job on every
    Myelin commit).
  - planning/05-refined-shared-systems-architecture/durable-workflow.md §1 (the engine paths the dogfood loop
    exercises) — no new architecture; this is the dogfood application.
  - planning/06-roadmaps/shared/durable-workflow.md §2 M6 (Myelin's own CI pipelines, merge queue, SLA timers, and
    any agent runs become myelin-flow workflows; the dogfood loop exercises every engine path on the platform's own
    commits; the gate is the self-hosting CI graph green + the truth-up pass confirming 0 red earlier FLOW gates).
  - planning/06-roadmaps/00-master-sequencing.md §2 M6 (the dogfood band) + §4 (the M6 done-bar: 0 red earlier-band
    gate; the truth-up pass).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-flow + the Myelin self-hosting
  config: wire Myelin's own CI pipelines, merge queue, and SLA timers to run as myelin-flow workflows on the
  self-hosting platform (the dogfood loop), so every engine path (replay, timers, signals, the long-park, the
  merge-queue wake) is exercised on the platform's own commits; and run the myelin-flow truth-up pass — re-confirm
  that every PROVEN FLOW drill row (FLOW-D1..D10, the E2E-2 spine) rests on a DATED green artifact, not a doc claim,
  and that no later-band FLOW gate is red (the gate invariant end-to-end). Record any drift between the code and
  these prompts (code wins; fix the prompt/doc) in the gap report. No new engine primitive.
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
  confirms every FLOW drill rests on a dated green artifact with 0 red earlier-band FLOW gate; any code-vs-doc drift
  is recorded (code wins, doc fixed); the work is committed.
- **COMMIT.** Header `P-FLOW-29 M6: dogfood Myelin's pipelines/merge-queue/SLA-timers as myelin-flow workflows`.
  Body lists: the dogfood loop wired; the self-hosting CI graph green on Myelin's own commits; the truth-up pass
  result (every FLOW drill PROVEN-with-a-dated-artifact, 0 red earlier gate); any drift recorded. Co-Authored-By
  trailer.

---

## Coverage check (this system's roadmap → prompts)

| Roadmap milestone (planning/06-roadmaps/shared/durable-workflow.md §2) | Prompt(s) | Band |
|---|---|---|
| FLOW-M2.1 — engine heartbeat: data model | P-FLOW-01 | M2 |
| FLOW-M2.1 — engine heartbeat: AppSpec service shell | P-FLOW-02 | M2 |
| FLOW-M2.1 — engine heartbeat: PersonalDataHolder structural half | P-FLOW-03 | M2 |
| FLOW-M2.1 — engine heartbeat: WfCtx core + journal/outbox co-commit | P-FLOW-04 | M2 |
| FLOW-M2.1 — engine heartbeat: deterministic replay/recovery + lease dispatch | P-FLOW-05 | M2 |
| FLOW-M2.1 — engine heartbeat: DurableExecutor start/describe/cancel + telemetry | P-FLOW-06 | M2 |
| FLOW-M2.1 — engine heartbeat: replay-divergence guard | P-FLOW-07 | M2 |
| FLOW-M2.1 — engine heartbeat: flow-determinism lint fixtures | P-FLOW-08 | M2 |
| FLOW-M2.2 — durable timers: the minute-bucket wheel + sleep (100k floor) | P-FLOW-13 | M2 |
| FLOW-M2.2 — durable timers: cheap SLA disarm/re-arm | P-FLOW-14 | M2 |
| FLOW-M2.3 — durable signals: signal delivery + wf_signal idempotency | P-FLOW-09 | M2 |
| FLOW-M2.3 — durable signals: per-effect idem_key rule | P-FLOW-10 | M2 |
| FLOW-M2.3 — durable signals: wait_for_signal + HITL round-trip | P-FLOW-11 | M2 |
| FLOW-M2.3 — durable signals: F-4 extended per-effect drill | P-FLOW-12 | M2 |
| FLOW-M2.4 — long-park: SCHEDULE_AND_RUN_JOB dispatch + park | P-FLOW-15 | M2 |
| FLOW-M2.4 — long-park: reserve/settle bookend | P-FLOW-16 | M2 |
| FLOW-M2.4 — long-park: mint_run_token mid-workflow re-mint | P-FLOW-17 | M2 |
| FLOW-M2.4 — long-park: loop safety | P-FLOW-18 | M2 |
| FLOW-M2.4 — merge-queue frame (in isolation) | P-FLOW-19 | M2 |
| FLOW-M3 — resumable maintenance activities + history-rewrite fan-out | P-FLOW-20 | M3 |
| FLOW-M3 — cheap re-arm confirmed under call sites + merge-queue re-green | P-FLOW-21 | M3 |
| FLOW-M4 — CI-pipeline-as-workflow substrate | P-FLOW-22 | M4 |
| FLOW-M4 — the X-1 seam end-to-end | P-FLOW-23 | M4 |
| FLOW-M5 — crypto-shred reaching history | P-FLOW-24 | M5 |
| FLOW-M5 — restore-verify to a consistent point | P-FLOW-25 | M5 |
| FLOW-M5 — 1M+ timer cell-scale run + promotion threshold | P-FLOW-26 | M5 |
| FLOW-M5 — 30x agent-workflow surge with lane shedding | P-FLOW-27 | M5 |
| FLOW-M5 — E2E-2 durable-workflow + HITL spine | P-FLOW-28 | M5 |
| FLOW-M6 — dogfood + truth-up | P-FLOW-29 | M6 |

**Floors paired with follow-ons (name-your-floors):** P-FLOW-03 crypto-shred-reach floor → P-FLOW-24; P-FLOW-13
100k-timer floor → P-FLOW-26; P-FLOW-19 merge-queue-in-isolation floor → P-FLOW-23 (X-1 seam end-to-end). The
mock-agent-runtime floor (M2) → real LlmAgentRuntime is a post-M5 config/impl swap owned by Agent Fabric (named in
the roadmap §3, not a myelin-flow prompt). The cross-cell-workflow-spanning floor and the
history-archival/continue-as-new floor are designed-not-built per roadmap §3 with measured-trigger follow-ons (no
prompt yet — they are added by Phase 7-B as appended prompts when their measured trigger fires; the DurableExecutor
contract is cell-agnostic + engine-agnostic so they extend without a rewrite).

**Drills greened across the ledger:** FLOW-D5 (P-FLOW-04), FLOW-D1 (P-FLOW-05), FLOW-D2 (P-FLOW-07), flow-determinism
lint fixtures (P-FLOW-08), FLOW-D4 (P-FLOW-11), FLOW-D4 per-effect extended (P-FLOW-12), FLOW-D3 floor (P-FLOW-13),
FLOW-D6 (P-FLOW-16), FLOW-D7 (P-FLOW-18), merge-queue in-isolation (P-FLOW-19), CI-D9/CI-D1 (P-FLOW-22),
GIT-D10/CI-D8 (P-FLOW-23), FLOW-D9 (P-FLOW-24), FLOW-D10 (P-FLOW-25), FLOW-D3 full (P-FLOW-26), FLOW-D8 (P-FLOW-27),
E2E-2 spine (P-FLOW-28), self-hosting CI graph + truth-up (P-FLOW-29). The must-be-green-first pair FLOW-D5
(P-FLOW-04) + FLOW-D1 (P-FLOW-05) is the gate nothing rides this engine until green.

**Note on band ordering within the file.** The prompts are numbered by their authored sequence (M2.1 heartbeat
P-FLOW-01..08, then the M2.3 signal sub-chain P-FLOW-09..12, the M2.2 timer sub-chain P-FLOW-13..14, the M2.4
long-park sub-chain P-FLOW-15..19), which interleaves the M2 sub-milestones by their DEPENDS-ON edges rather than
by strict M2.x order: the signals chain (09..12) depends only on M2.1 + the timer for its timeout branch, the timer
chain (13..14) depends on M2.1, and the long-park chain (15..19) depends on both signals and timers. Phase 7-B's
index assigns the global P-NNN order from these DEPENDS-ON edges; the local P-FLOW-NN ids are sequence markers only.
The coverage table above lists each milestone with its prompt(s) regardless of file position.
