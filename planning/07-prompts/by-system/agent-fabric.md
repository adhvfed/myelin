# Phase 7 — Prompt Ledger: Agent Fabric (myelin-agent)

> Phase: 07-prompts (per-system file, Phase 7-A). The complete ordered set of implementation prompts that
> operationalize the entire agent-fabric roadmap (planning/06-roadmaps/shared/agent-fabric.md, milestones
> M2-A, M2-B, M2-C, M3, M4, M5, M6) into clean-context, independently-committable coding tasks. Built to the
> template in planning/07-prompts/00-ledger-overview.md §2 (every field present, never implicit) and banded to
> planning/06-roadmaps/00-master-sequencing.md §2 (M0..M6, the gate invariant). Frozen architecture (this file
> OPERATIONALIZES, it does not redesign): planning/05-refined-shared-systems-architecture/agent-fabric.md +
> contract-index.md §8 (owned 8.1..8.8) + the dependency rows (1.x/2.x/3.x/4.x/7.3/9.x/10.x/11.7/13.1) +
> 00-reconciliation-decisions.md (X-6, OQ-E, OQ-F, OQ-K, OQ-L). Plain-text identifiers throughout (no
> backticks-as-emphasis). Markdown only; this file makes no commits. Date: 2026-06-19.
>
> The global P-NNN ids are assigned by the consolidated ledger index (Phase 7-B, 01-ledger-index.md) when these
> per-system prompts are interleaved into the single execution order. Here each prompt carries a stable local
> handle AG-P<n> so its DEPENDS-ON edges are unambiguous before global numbering; the index rewrites AG-P<n> to
> its P-NNN. Where a prompt depends on another system's prompt not yet numbered, it names that system's
> milestone (the index resolves it to the P-NNN).
>
> The shape of this system (from the roadmap §0): the Fabric owns NO M0/M1 work — it is the heaviest consumer
> of M0 (outbox/causality) and M1 (Id list_objects/check/mint_run_token/delegation, Storage reserve/settle +
> per-subject DEK, Tenancy partition), so its earliest band is M2. M2 is the one large band (SKELETON -> mock ->
> the unified sandbox + the hard AG-D4 escape GATE). M3/M4 add NO new engine — each subsystem registers
> ToolDefs into the existing surface. M5 is world-scale hardening + the E2E-2 flagship + erasure fan-out. M6 is
> dogfood. The real LlmAgentRuntime and the external MCP endpoint are post-M5 (after the safety drills are
> green) — named floors, not built here.
>
> Coverage: M2-A -> AG-P1, AG-P2 (crate skeleton + data model; the SKELETON runtime). M2-B -> AG-P3, AG-P4,
> AG-P5, AG-P6, AG-P7 (mock runtime; the plan-then-apply pipeline; the HITL withhold/approve/resume loop with
> per-effect idempotency; the structural loop guards + per-run identity; the cost-gate self-limiting + dry-run).
> M2-C -> AG-P8 (ToolHands::exec on the unified sandbox + the AG-D4 hard GATE). M3 -> AG-P9 (per-producer
> ToolDefs + the trace holder seam wired). M4 -> AG-P10 (per-consumer ToolDefs + AG-D4 re-confirm on the prod
> CI image). M5 -> AG-P11, AG-P12, AG-P13 (the 30x agent-surge family + shed budget; erasure fan-out AG-D10;
> the E2E-2 flagship). M6 -> AG-P14 (dogfood). Fourteen prompts, no milestone gap.

---

### AG-P1 — Ship the myelin-agent glue crate: the trait set, the data model, and the no-llm-in-platform lint

- **BAND.** M2.
- **ROADMAP MILESTONE.** M2-A (planning/06-roadmaps/shared/agent-fabric.md §2 "M2-A — SKELETON: the substrate
  path proven at zero cost"), the trait-surface + data-model slice.
- **DEPENDS-ON.** The M0 substrate prompts that lay down the Cargo workspace + the eight glue-crate skeletons,
  the serve(AppSpec) harness, the transactional outbox + EventHandler template, the twelve-lint framework + the
  contract-coverage scanner (master §2 M0; substrate roadmap SUB-M0; event-bus M0). The M1 prompts that ship
  Identity (mint_run_token 4.7, delegation 4.5, check 4.2), Storage (reserve/settle 11.7, per-subject DEK
  11.3/11.4), Tenancy (partition key 12.1) and the GDPR PersonalDataHolder spine (10.1, 1.4) — all must be
  merged because the data model registers into them and references their types. The index places this at the
  head of the M2 band, after M1 is green.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md (always) §3 (agent-native from the ground up: strategy pattern everywhere agents plug in,
    mock implementations during development; name-your-floors; code-wins-over-docs);
    ../../external-insights/01-process-and-quality-doctrine.md §5 (the ratchet / committed gates — an
    uncommitted lint is no lint), §1 (name-your-floors);
    ../../external-insights/03-agent-native-fabric.md preamble + §1 (an agent is a Principal with kind=agent
    through the same identity/gateway/log/sandbox/cost-gate as everyone else; the substrate-is-right thesis).
  - Architecture: ../05-refined-shared-systems-architecture/agent-fabric.md §1 (purpose + the trait set at a
    glance; the only strategy-swappable members are AgentRuntime and ToolHands), §4 (the data model:
    run/tool_def/proposed_effect/hitl_gate/trace, all (tenant, region)-first, RLS, residency-pinned,
    per-tenant envelope-encrypted, PersonalDataHolder auto-registered), §3 (the three runtimes SKELETON ->
    Mock -> Llm — Llm is designed-not-built).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-6 (the four
    uniform guarantees; the requires_approval defaults — pinned here as the column shape, seeded in AG-P4).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md §8 rows 8.1 (ToolSurface/ToolDef
    field list), 8.2 (EffectApi::apply signature), 8.3 (AgentRuntime::step signature), 8.4 (ToolHands::exec
    signature), 8.5 (Agent::handle signature), 8.6 (EventInbox::deliver signature), 8.7 (run --dry-run); row
    1.6 (the no-llm-in-platform + no-host-exec + no-cross-db lints this crate is bound by); row 1.4
    (PersonalDataHolder auto-registration); 10.1 (the holder trait).
  - Roadmap: planning/06-roadmaps/shared/agent-fabric.md §1.1 (the owned-contracts -> milestone map) + §2 M2-A.
- **DELIVERABLE (what to build + exactly where in the repo).** In the glue crate myelin-agent (the M0
  skeleton), the compile-time contract surface — types and trait signatures only, NO engine logic:
  - The six traits to the frozen 8.x signatures: AgentRuntime{step(&Conversation) -> StepOutcome} (8.3);
    Agent{handle(InboxEvent, &dyn AgentRuntime) -> RunOutcome} (8.5); ToolHands{exec(Command) -> ToolResult}
    (8.4); ToolSurface{register_tool(ToolDef); resolve(&ToolName) -> Option<&ToolDef>} (8.1);
    EventInbox{deliver(InboxEvent)} (8.6); EffectApi{apply(&RunCtx, ProposedEffect) -> EffectResult} (8.2). Plus
    the value enums StepOutcome{UseTools(Vec<ToolCall>), Submit(Submission)} and EffectResult{Applied(event_id),
    Gated(gate_id), Denied(reason)}.
  - The ToolDef struct with the frozen field list (8.1): name, subsystem, version, input_schema, required_caps,
    effect_kind (enum read|compute|mutate|external), side_effecting (bool), requires_approval (bool),
    exposed_over_mcp (bool). The requires_approval column exists here; its per-subsystem seed defaults land in
    AG-P4 (do not seed values here).
  - The data model migrations in the myelin-agent implementation crate (the service crate, distinct from the
    glue crate): tables run, tool_def, proposed_effect, hitl_gate, trace — each with (tenant, region) as the
    first column(s), RLS enabled, no cross-tenant query path, residency-pinned, per-tenant envelope-encrypted
    columns for any PII-bearing field. run carries: run_id, agent_principal, on_behalf_of, binding_id,
    trigger_event, correlation_id/causation_id/depth, runtime_ref (the strategy swap), state, reservation_id,
    budget (integer minor-units), trace_ref. Forward-only online migrations (contract 1.5).
  - Register every store the crate opens as a PersonalDataHolder via the harness (1.4/10.1) — the holder
    implementations themselves (locate/export/erase) land in AG-P12; here the registration seam exists so the
    no-untagged-personal-data lint passes on every PII field (tag each #[personal_data] now).
  - Confirm the no-llm-in-platform lint (1.6) is wired and green over myelin-agent: NO model/SDK/prompt/
    model-name string anywhere in this crate (the only place such a string may ever appear is LlmAgentRuntime,
    a post-M5 floor). Add a red-fixture (a file with a forbidden model string is rejected) + a green-fixture
    (the clean crate admits), loud-never-swallowed.
  - FLOOR named: LlmAgentRuntime is designed-not-built (the trait seam AgentRuntime exists; the real adapter is
    the post-M5 follow-on, AG-P13 names it). State this in the module doc so the trait is not mistaken for a
    working brain. The runtime workers are stateless (a crashed worker's run resumes from the durable workflow
    + the trace) — note this premise.
- **CONTRACTS TO IMPLEMENT.** 8.1 (ToolSurface/ToolDef — owned, the struct + register/resolve signatures),
  8.2/8.3/8.4/8.5/8.6/8.7 (owned, signatures only — bodies land in later prompts), 1.4/10.1 (consumed — holder
  registration seam), 1.6 (no-llm-in-platform + no-host-exec + no-cross-db wired as gates). Implement to the
  frozen signatures; a needed shape change is a whole-workspace contract PR, escalated and written down, not a
  local divergence (code-wins-over-docs: if reality forces a deviation, write it down and fix the doc).
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The myelin-agent glue crate compiles and is linked by the workspace; a change to any 8.x trait/struct
    breaks every consumer's build now (the ADR-01 compile-time-carrier property) — CI.
  - The data-model migrations apply forward-only (forward-only-migration lint green); every run/tool_def/
    proposed_effect/hitl_gate/trace table is (tenant, region)-first with RLS, asserted by the tenant-predicate
    lint green + a fixture that a non-tenant-scoped query is rejected — CI (permanent ratchet gate).
  - The no-llm-in-platform lint green over myelin-agent with both fixtures; the no-untagged-personal-data lint
    green (every PII column tagged) — CI (permanent ratchet gates; say so).
  - The contract-coverage scanner passes on the myelin-agent §8 rows (each has a provider+consumer CDC stub) —
    CI.
- **TESTS (required).** Unit tests that the trait signatures compile against mock impls; a migration test that
  the schema applies forward-only and RLS denies a cross-tenant read; the provider+consumer CDC stubs for rows
  8.1..8.7; the red+green fixtures for no-llm-in-platform. myelin-agent is a mandatory-core glue crate: state
  the cargo-mutants mutation-score floor for the ToolDef/EffectResult/StepOutcome value-type module in this
  field and meet it (the value types are pure and must be mutation-covered).
- **DEFINITION OF DONE.** myelin-agent compiles in the workspace and is linked by consumers; the 8.x signatures
  + the data model match the frozen shapes; the tenant-predicate/no-llm-in-platform/no-untagged-personal-data/
  forward-only-migration lints emit dated green artifacts with their fixtures; the migration + CDC + unit tests
  pass; the contract-coverage scanner is green; the floor (LlmAgentRuntime designed-not-built; holder bodies in
  AG-P12) is named in the module doc; the work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M2: myelin-agent glue crate — trait set + data model + no-llm-in-platform lint.
  Body lists: contracts 8.1..8.7 (signatures) + 1.4/10.1 (holder seam) implemented; the lints greened with
  fixtures; the value-type mutation-score measured; the floor named (LlmAgentRuntime post-M5 AG-P13; holder
  bodies AG-P12). Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By
  trailer.

---

### AG-P2 — The SKELETON runtime: prove the gateway -> identity -> dispatch -> reserve -> trace path at zero cost

- **BAND.** M2.
- **ROADMAP MILESTONE.** M2-A (planning/06-roadmaps/shared/agent-fabric.md §2 "M2-A — SKELETON"), the
  first-runnable proof slice.
- **DEPENDS-ON.** AG-P1 (the crate + traits + data model exist). The M1 Identity prompt that ships
  mint_run_token/revoke (4.7) and the Storage prompt that ships reserve/settle (11.7). The M2 Event-bus prompt
  that ships the reactive/dispatch tier (3.6) and the M2 Durable-workflow prompt that ships
  DurableExecutor{start} + WfCtx (9.1/9.2) — the SKELETON drives a run as a durable workflow. The index places
  this after those M2 substrate prompts are callable.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (mock implementations during development; the strategy pattern);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it-or-it-isn't-real — exercise the
    substrate path before claiming it; observability is part of the pass), §4 (chain operations end to end);
    ../../external-insights/03-agent-native-fabric.md §3 (the SKELETON -> mock -> real build order).
  - Architecture: ../05-refined-shared-systems-architecture/agent-fabric.md §3.1 (the SKELETON runtime: no
    model, no tools; drives the whole gateway/identity/dispatch/reserve/trace path at ~zero cost), §2.3
    (Agent::handle — the bounded driven multi-turn loop, the loop body), §5.1 (the agent loop driver), §5.6 (a
    run is a durable workflow: the workflow owns budget/gates/state; step/exec are activities; reserve/settle
    are the bookends), §5.7 (per-run identity: mint at dispatch, token life == run life, scrub shared token,
    revoke idempotently on teardown).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 8.3/8.5 (step + handle), 8.7
    (dry-run), 4.7 (mint_run_token/revoke), 11.7 (reserve/settle), 9.1/9.5 (DurableExecutor + workflow<->agent
    mapping), 3.6 (the dispatch tier), 2.2 (OutboxTx::emit nested causality), 1.8 (the telemetry signal set —
    the trace + the reserve/settle ledger are the green artifacts).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row AG-D8 (per-run token revoked on teardown AND auto-expires; 0 shared token leaked into the child env) —
    the no-tool path leg.
  - Roadmap: planning/06-roadmaps/shared/agent-fabric.md §2 M2-A (work + exit gate).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-agent implementation crate:
  - The SkeletonAgentRuntime: an AgentRuntime impl with no model and no tools; its step() submits immediately
    (returns Submit). It exists to exercise the substrate, not to think.
  - The Agent::handle loop body wired as a durable workflow (9.1/9.5): on an InboxEvent, the dispatch tier
    (3.6) starts a run-as-durable-workflow that (1) mints a per-run attenuated token via Id mint_run_token
    (4.7), token life == run life; (2) opens a reservation via reserve/settle (11.7) at dispatch; (3) builds the
    Conversation (empty for the SKELETON); (4) steps the brain (immediate Submit); (5) writes a (near-empty)
    trace row; (6) settles the reservation; (7) on teardown revokes the token idempotently (even on crash),
    belt-and-suspenders with the auto-expiring tuple. Causality carried nested via OutboxTx::emit(draft, cause).
  - The serve(AppSpec) for the agent service: the AppSpec + the handlers/consumers the harness wires (NOT a
    hand-rolled main) — three ports, liveness != readiness, holder auto-registration, the dispatch consumer
    bound by name with a subjects() whitelist (never *).
  - The metrics: the run emits the survival signals (1.8) — a balanced reserve/settle ledger (reserved ==
    settled), a written trace_ref, token-revocation lag — so the path is observable (a path that survives but
    emits no signal has failed the drill).
  - FLOOR named: this is the SKELETON — the missing half is the brain (a real step) + the tools (exec/apply);
    the follow-on is M2-B (AG-P3..AG-P7) + M2-C (AG-P8). State this in writing; a skeleton that masquerades as a
    working agent is the failure.
- **CONTRACTS TO IMPLEMENT.** 8.3 (step — the SKELETON impl), 8.5 (Agent::handle — the loop body, owned),
  consumed: 4.7 (mint_run_token/revoke), 11.7 (reserve/settle), 9.1/9.5 (DurableExecutor + the bookends), 3.6
  (dispatch tier delivery), 2.2 (OutboxTx::emit nested causality), 1.1 (serve(AppSpec)).
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The SKELETON path: an InboxEvent produces a complete, attributed, metered, traced run — assert one trace
    row written, reserved == settled (ledger balanced), and the run principal == the per-run token's principal
    (attribution intact); telemetry signals emitted — CI.
  - AG-D8 (no-tool leg): kill the run mid-flight -> the per-run token is revoked on teardown AND auto-expires <=
    W; 0 shared platform token leaked into the child env (assert the child env has no inherited platform token);
    token-revocation-lag signal within bound — CI.
- **TESTS (required).** Unit tests for the loop driver state transitions. An end-to-end test that CHAINS the
  operations (deliver -> mint -> reserve -> step -> trace -> settle -> revoke), not a single handler call (EI-01
  §4: real sessions chain mutations). The AG-D8 no-tool-leg drill scenario on the failure-injection harness
  (kill mid-flight, assert revocation + 0 leak). The provider+consumer CDC for 8.5. Mutation-score floor for the
  loop-driver module stated and met.
- **DEFINITION OF DONE.** The SKELETON runtime + the durable-workflow loop body exist and compile; the
  zero-cost path emits a complete trace + a balanced reserve/settle ledger with the telemetry signals; AG-D8 on
  the no-tool path is green and dated (PROVEN, not CLAIMED); the unit + chained-e2e + drill + CDC tests pass;
  the floor (SKELETON -> M2-B/M2-C) is named; the work is committed. A red AG-D8 becomes a dated scorecard row,
  never edited green.
- **COMMIT.** Header: P-<NNN> M2: SKELETON runtime — gateway/identity/dispatch/reserve/trace path at zero cost.
  Body lists: 8.5/8.3 implemented; AG-D8 (no-tool leg) greened with its measured numbers (0 token leak,
  revocation lag); the floor named (SKELETON; brain+tools follow in AG-P3..AG-P8). Branch first if on default;
  do not push unless asked. End with the Co-Authored-By trailer.

---

### AG-P3 — The MockAgentRuntime: a deterministic scripted brain on the same code path users hit

- **BAND.** M2.
- **ROADMAP MILESTONE.** M2-B (planning/06-roadmaps/shared/agent-fabric.md §2 "M2-B — Mock runtime +
  plan-then-apply + HITL"), the mock-runtime slice.
- **DEPENDS-ON.** AG-P2 (the SKELETON + the loop body exist; the mock plugs into the same handle loop).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (mock implementations during development; the strategy pattern everywhere agents plug in;
    switching mock -> real is a config/impl swap, not a rewrite);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it; the failure-injection harness),
    §4 (untested is acceptable only if named); ../../external-insights/03-agent-native-fabric.md §3 (mock as
    the lever for golden + mutation testing of the event->trigger->effect->event loop).
  - Architecture: ../05-refined-shared-systems-architecture/agent-fabric.md §3.2 (MockAgentRuntime —
    deterministic scripted StepOutcomes; shipped as a real --use-mock runtime flag on the SAME code path users
    hit; the golden + cargo-mutants lever), §2.1 (the brain is stateless; the platform owns the Conversation
    history — a PersonalDataHolder, residency-pinned, the trace).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 8.3 (AgentRuntime::step;
    --use-mock is a real flag), 8.7 (run --dry-run).
  - Drills: 01-whole-system-e2e-and-drill-catalogue.md row AG-D9 (run a scripted mock twice -> identical
    proposed-effect sequences; cargo-mutants over event->trigger->effect->event >= threshold).
  - Roadmap: planning/06-roadmaps/shared/agent-fabric.md §2 M2-B (the MockAgentRuntime work bullet + AG-D9).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-agent implementation crate:
  - The MockAgentRuntime: an AgentRuntime impl whose step() returns scripted, deterministic StepOutcomes driven
    by a test-fixture script (a sequence of UseTools/Submit). It is gated by the --use-mock runtime flag (a real
    flag on the same gateway/identity/dispatch/reserve/trace path, NOT a test-only stub) — VISION §3 binds: mock
    agents only during development, on the code path users will hit.
  - The platform-owned Conversation history: build_conversation reconstructs the Conversation from the trace
    (the system context + prior tool results + the running transcript); the brain is stateless and is passed a
    &Conversation. The history is a PersonalDataHolder, residency-pinned.
  - The determinism property wired: given the same script + the same inbound event, two runs produce identical
    proposed-effect sequences (the foundation AG-D9 asserts; the full plan-then-apply that produces effects
    lands in AG-P4, so this prompt asserts determinism of the step sequence and the conversation reconstruction
    — note that the effect-sequence determinism completes once AG-P4 lands and AG-D9 is re-asserted there).
  - FLOOR named: the MockAgentRuntime is THE named v1 floor (VISION §3, roadmap §3) — the real LlmAgentRuntime
    is the post-M5 follow-on (AG-P13), swapped in after the safety drills (AG-D4/D2/D3/D5) are green, a
    config/impl swap behind the frozen AgentRuntime seam, not a rewrite. State this in the module doc.
- **CONTRACTS TO IMPLEMENT.** 8.3 (AgentRuntime::step — the Mock impl + the --use-mock flag, owned). Consumed:
  none new.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - AG-D9 (the step-determinism leg): run the same scripted mock twice -> identical step/conversation sequences
    (byte-identical StepOutcome stream); the cargo-mutants score over the brain/loop seam >= the stated mutation
    threshold — CI. (The full proposed-effect-sequence determinism is re-asserted in AG-P4 once apply produces
    effects; this prompt greens the step-sequence half and names the completion.)
  - --use-mock drives the full AG-P2 substrate path (mint/reserve/trace/settle) unchanged — the mock is on the
    real code path, not a bypass — CI.
- **TESTS (required).** A golden test: a fixed script -> a recorded StepOutcome stream, asserted byte-identical
  across two runs. The cargo-mutants run over the brain/loop module with the stated score floor. The
  provider+consumer CDC for 8.3. Mutation-score floor stated and met.
- **DEFINITION OF DONE.** The MockAgentRuntime + --use-mock flag exist and run on the same code path as the
  SKELETON; the step-determinism leg of AG-D9 is green and dated with the cargo-mutants score; the golden + CDC
  tests pass; the floor (Mock -> LlmAgentRuntime post-M5) is named; the work is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: MockAgentRuntime — deterministic scripted brain on the --use-mock real path.
  Body lists: 8.3 (Mock + --use-mock) implemented; AG-D9 step-determinism greened with the mutation score; the
  floor named (Mock; real runtime AG-P13). Branch first if on default; do not push unless asked. End with the
  Co-Authored-By trailer.

---

### AG-P4 — Plan-then-apply EffectApi::apply: the schema -> capability -> delegation -> tenant -> budget -> HITL -> apply -> meter pipeline + the frozen requires_approval defaults

- **BAND.** M2.
- **ROADMAP MILESTONE.** M2-B (planning/06-roadmaps/shared/agent-fabric.md §2 "M2-B"), the plan-then-apply +
  delegation-intersection + requires_approval-defaults slice (the core safety+testability seam).
- **DEPENDS-ON.** AG-P3 (the mock produces proposed effects to validate). The M1 Identity prompts that ship
  check + CaveatContext (4.2), list_objects SetExpr push-down (4.3), delegation the ∩ algebra (4.5),
  write_tuples/zookie (4.6/4.10). The Storage reserve/settle (11.7). The index places this after those Id/
  Storage contracts are callable.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agents are first-class but bounded; a deviation must be written down);
    ../../external-insights/03-agent-native-fabric.md §2/§4 (plan-then-apply: agents are a pure-ish function
    (event, context) -> {effects}; they never side-effect directly; intersection not union — an agent can do
    nothing no human role can; same gateway no carve-out); ../../external-insights/01-process-and-quality-
    doctrine.md §3 (a property does not exist until a test forces the failure).
  - Architecture: ../05-refined-shared-systems-architecture/agent-fabric.md §5.2 (the plan-then-apply pipeline,
    in order, fail-closed: SCHEMA -> CAPABILITY (with CaveatContext for field/transition ABAC, evaluated at
    check-time off the hot list_objects path) -> DELEGATION (agent.policy ∩ delegation ∩ tenant.policy,
    intersection never union) -> TENANT -> BUDGET -> HITL GATE -> APPLY via the subsystem's PUBLIC endpoint as
    the agent principal -> METER; a denied effect returns an ordinary Denied tool error, no privileged
    fallback), §2.1 (the tool-list scoping via the list_objects push-down — an optimisation; the check is the
    guarantee, fail-closed), §5.0 (the routing table: mutate/external -> EffectApi), §6.3 (the FROZEN
    requires_approval defaults table + the cross-subsystem rule).
  - Reconciliation: 00-reconciliation-decisions.md X-6 (the four uniform guarantees; the requires_approval
    defaults), OQ-E (the CaveatContext + the SetExpr push-down).
  - Contracts: contract-index.md rows 8.2 (EffectApi::apply), 8.1 (the requires_approval defaults frozen), 4.2
    (check + CaveatContext{object, field?, transition?, attrs}), 4.3 (list_objects -> SetExpr), 4.5
    (delegation -> EffectivePolicy, the ∩ algebra), 11.7 (BUDGET/reserve), 8.7 (--dry-run stops after step 6).
  - Drills: 01-whole-system-e2e-and-drill-catalogue.md rows AG-D1 (no write outside EffectApi; no-host-exec +
    no-cross-db lints green; 0 direct mutation), AG-D2 (effect outside the ∩ -> Denied; 0 privileged fallback),
    AG-D3 (delegation intersection / least-privilege; 0 over-privilege), AG-D9 (re-assert effect-sequence
    determinism now that apply produces effects).
  - Roadmap: planning/06-roadmaps/shared/agent-fabric.md §2 M2-B (the EffectApi work bullets + AG-D1/D2/D3).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-agent implementation crate:
  - EffectApi::apply (8.2) implementing the eight-step pipeline IN ORDER, FAIL-CLOSED, exactly per §5.2: (1)
    SCHEMA validate effect.input against the ToolDef JSON Schema (malformed -> Denied); (2) CAPABILITY — call
    Id.check(run.agent_principal, required_cap, effect.object, zookie, caveat: CaveatContext{object, field?,
    transition?, attrs}) — the caveat carries the field/transition ABAC evaluated HERE, off the hot list path;
    (3) DELEGATION — Id.delegation(agent, trigger_actor) -> agent.policy ∩ delegation ∩ tenant.policy,
    intersection never union, attenuation never up; (4) TENANT — tenant guardrails (agent-allow-list, residency,
    AI-Act); (5) BUDGET — the reserve has remaining balance (11.7); (6) HITL GATE — if tool_def.requires_approval
    AND not yet approved -> WITHHELD (open a hitl_gate, return Gated; the tool returns an error and does NOT
    mutate — the HITL machinery itself is AG-P5; here, return Gated and stop); (7) APPLY — call the subsystem's
    PUBLIC endpoint as the agent principal (same gateway, no carve-out) so the subsystem emits its domain event
    via ITS outbox; (8) METER — settle one cost event. Return EffectResult ∈ {Applied(event_id), Gated(gate_id),
    Denied(reason)}.
  - The tool-list scoping: Conversation.tools = the run's permitted, delegation-scoped subset computed via the
    list_objects SetExpr push-down (4.3, no N+1). EffectApi STILL re-checks at apply time (the scoping is an
    optimisation; the check is the guarantee, fail-closed). A denied effect is an ordinary Denied tool error —
    NO privileged fallback (AG-5).
  - The frozen §6.3 requires_approval defaults table seeded into tool_def.requires_approval as the per-subsystem
    seed (CI deploy/secret = yes; Git merge = yes, open_pr = no; Issues forecast/triage/sla_draft = no, SLA
    transition = caveat-gated; KN publish/confidential = yes, draft/comment = no; Chat post/react = no; a
    cross-subsystem effect inherits the TARGET subsystem's default — "governed where it lands"). A subsystem may
    tighten a default but may not loosen a yes->no for a consequential action without a written deviation.
  - run --dry-run (8.7): a code path that runs steps 1..6 and returns Vec<ProposedEffect> WITHOUT applying — the
    testability lever every E2E and drill uses.
  - FLOOR named: none — the pipeline is identical for mock and real; there is no floor on EffectApi.
- **CONTRACTS TO IMPLEMENT.** 8.2 (EffectApi::apply — owned), 8.1 (the requires_approval defaults seed — owned),
  8.7 (--dry-run — owned). Consumed: 4.2 (check + CaveatContext), 4.3 (list_objects SetExpr), 4.5 (delegation),
  11.7 (reserve). Implement to the frozen shapes; a needed shape change is escalated, not diverged.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - AG-D1 (a tool tries to write outside EffectApi -> structurally impossible; no-host-exec + no-cross-db lints
    green; 0 direct mutation) — CI (the lints are permanent ratchet gates).
  - AG-D2 (an effect outside the agent.policy ∩ delegation ∩ tenant.policy -> Denied returns to the loop; 0
    privileged fallback fires; the denial-counter signal increments, the fallback-counter is 0) — CI.
  - AG-D3 (an effect policy allows but delegation/tenant forbids, AND vice-versa -> confined to the
    intersection; 0 over-privilege; the intersection-proof artifact) — CI.
  - AG-D9 (re-asserted): run a scripted mock twice through the full apply pipeline -> identical proposed-effect
    sequences (byte-identical); cargo-mutants over event->trigger->effect->event >= the threshold — CI.
- **TESTS (required).** Unit tests for each pipeline step (schema reject, capability deny, delegation
  intersection, tenant deny, budget refuse). An end-to-end test that CHAINS a mock run through apply for an
  allowed effect (Applied) and a disallowed effect (Denied) in one session. The AG-D1/D2/D3 drill scenarios on
  the failure-injection harness with adversarial over/under-privilege corpora (including a delegator who lost
  the right). The AG-D9 determinism golden re-run. The provider+consumer CDC for 8.2/8.1/8.7 and the consumer
  CDC for 4.2/4.3/4.5. Mutation-score floor for the apply-pipeline module stated and met (this is mandatory
  core).
- **DEFINITION OF DONE.** EffectApi::apply implements the eight-step fail-closed pipeline + the SetExpr tool
  scoping + the frozen requires_approval defaults + --dry-run; AG-D1/AG-D2/AG-D3 are green and dated (0 direct
  mutation, 0 privileged fallback, 0 over-privilege) and AG-D9 re-asserted with its mutation score; the
  no-host-exec/no-cross-db lints are green; the unit + chained-e2e + drill + CDC tests pass; the
  contract-coverage scanner is green; the work is committed. No assertion inverted to manufacture green.
- **COMMIT.** Header: P-<NNN> M2: EffectApi plan-then-apply — schema/cap/delegation/tenant/budget/HITL/apply/
  meter + frozen requires_approval defaults. Body lists: 8.2/8.1/8.7 implemented; AG-D1/D2/D3 greened with
  measured numbers + AG-D9 re-asserted; the apply-pipeline mutation score; no floor. Branch first if on default;
  do not push unless asked. End with the Co-Authored-By trailer.

---

### AG-P5 — The HITL withhold -> approve -> resume loop, with per-effect idempotency and humanise card text

- **BAND.** M2.
- **ROADMAP MILESTONE.** M2-B (planning/06-roadmaps/shared/agent-fabric.md §2 "M2-B"), the HITL slice.
- **DEPENDS-ON.** AG-P4 (EffectApi returns Gated; the HITL machinery resumes that). The M2 Durable-workflow
  prompt that ships the durable HITL signal (9.4) + DurableExecutor signal with per-effect idem_key (9.1). The
  M1 Identity list_subjects (4.4, the approver set). The M2 Notifications prompt that ships humanise (7.3). The
  index places this after those are callable.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (suggest-by-default; human-confirm consequential actions);
    ../../external-insights/03-agent-native-fabric.md §4 (HITL withhold; a gated effect does not mutate);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (exactly-once is a quantified gate — 0
    mutation pre-approval, 1 apply; never weaken a threshold), §4 (chained mutations mid-flight — where the bugs
    live).
  - Architecture: ../05-refined-shared-systems-architecture/agent-fabric.md §5.3 (withhold -> surface (a
    durable-workflow wait surfaced as a chat approval card showing action + risk + LIVE cost estimate, approver
    set = list_subjects(object, approve_perm)) -> decide (minutes or days; the wait holds no runtime) -> resume
    (the workflow signal re-runs the step with the tool added to "approved"; step 6 now passes; the effect
    applies); C4 per-effect resume idempotency (idem_key = card_id single, card_id:<effect_idx> multi/partial —
    a double-click is one approval, a partial approval is well-defined, each effect maps to exactly one
    EffectApi::apply); C9 the card text goes through the ONE templating surface humanise — never raw strings),
    §4.4 (the hitl_gate table: gate_id, run_id, effect_id, risk_summary humanised, cost_estimate, approver_
    filter, state, card_ref).
  - Reconciliation: 00-reconciliation-decisions.md OQ-F (per-effect idem_key), OQ-L (humanise the sole
    templating surface).
  - Contracts: contract-index.md rows 8.2 (the HITL GATE step + AG-8 a withheld gated tool does not mutate), 9.4
    (durable HITL signal — state=waiting holds no runtime; an approval/cancel signal arrives hours/days later
    idempotent, re-leases + replays + consumes), 9.1 (signal idempotent on idem_key, the per-effect rule), 4.4
    (list_subjects -> SubjectTree, the approver set), 7.3 (humanise((template_key, args), viewer, locale) ->
    HumanisedString — per-viewer, permission/erasure-safe, ICU MessageFormat).
  - Drills: 01-whole-system-e2e-and-drill-catalogue.md row AG-D5 (gated tool -> withheld, returns error, does
    NOT mutate; card shows action+risk+cost; approval resumes + applies exactly once; rejection halts; 0
    mutation pre-approval, 1 apply; per-effect idempotency — partial approval + double-click well-defined).
  - Roadmap: planning/06-roadmaps/shared/agent-fabric.md §2 M2-B (the HITL work bullet + AG-D5).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-agent implementation crate:
  - The withhold -> approve -> resume loop on top of AG-P4's Gated result: when EffectApi returns Gated, open a
    hitl_gate row + a durable-workflow wait (9.4) that holds no runtime; surface it as a chat approval card
    (via Notif) showing the pending action + risk + a LIVE cost estimate; the approver set =
    list_subjects(object, approve_perm) (4.4).
  - The resume signal: the workflow signal re-runs the step with the tool name added to the run's "approved"
    set; step 6 of the pipeline now passes; the effect applies exactly once. Rejection settles Halted::Rejected
    with the reason in the trace + audit.
  - Per-effect idempotency (C4/OQ-F): the resume signal's idem_key is per-effect — idem_key = card_id for a
    single-effect card; idem_key = card_id ":" effect_idx for a multi-effect card. A batch card gating N effects
    sends N independently-idempotent signals; a partial approval (approve 0 and 2, decline 1) sends three
    signals, each mapping to exactly one EffectApi::apply; a declined effect is withheld; a double-click on
    "approve all" re-sends the same keys -> no double-apply. Both "a double-click is one approval" and "a
    partial approval is well-defined" are true by construction.
  - The card text path (C9/OQ-L): the hitl_gate.risk_summary and any agent-authored message are NEVER raw
    strings — they are a (template_key, args) pair + an ArtifactRef, humanised per-viewer by Notif humanise
    (7.3). There is no second template engine and no frontend string map.
  - FLOOR named: none for the HITL machinery itself. (Implicit auto-dispatch on a casual mention remains
    [OPEN -> LEGAL] L-3 and is handled at the dispatch layer in AG-P10/Chat, not here.)
- **CONTRACTS TO IMPLEMENT.** 8.2 (the HITL GATE step completion — owned). Consumed: 9.4 (durable HITL signal),
  9.1 (per-effect idem_key), 4.4 (list_subjects), 7.3 (humanise). Implement to the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - AG-D5: a gated tool is withheld (returns an error, does NOT mutate); the card shows action + risk + cost;
    approval resumes and applies EXACTLY ONCE; rejection halts; 0 mutations before approval, 1 apply; per-effect
    idempotency proven — a partial approval (2-of-3) applies exactly the approved effects and withholds the
    declined one, and a double-click is one approval (the apply-counter == the approved-effect count, never
    more) — CI.
- **TESTS (required).** Unit tests for the hitl_gate state machine (waiting -> approved/rejected) and the
  idem_key derivation (single vs multi-effect). An end-to-end CHAINED test: a mock run proposes a gated effect
  -> withheld (assert 0 mutation) -> the workflow parks -> an approval signal arrives -> resume -> exactly one
  apply; then a double-click variant (same idem_key, 0 extra apply); then a partial-approval variant (2-of-3, 2
  applies, 1 withheld). The AG-D5 drill scenario on the harness (kill mid-park is exercised end-to-end in AG-P8
  / E2E-2; here assert the idempotency + withhold). The consumer CDC for 9.4/9.1/4.4/7.3. Mutation-score floor
  for the HITL state machine stated and met.
- **DEFINITION OF DONE.** The withhold -> approve -> resume loop with per-effect idempotency + humanise card
  text exists; AG-D5 is green and dated with the measured 0-mutation-pre-approval / 1-apply / partial-approval
  numbers; the unit + chained-e2e + drill + CDC tests pass; the work is committed. The exactly-once threshold is
  never weakened — a red AG-D5 is a dated scorecard row.
- **COMMIT.** Header: P-<NNN> M2: HITL withhold/approve/resume — per-effect idempotency + humanise card text.
  Body lists: 8.2 HITL step + 9.4/9.1/4.4/7.3 wired; AG-D5 greened with the measured apply-count parity; no
  floor (auto-dispatch L-3 deferred to Chat/AG-P10). Branch first if on default; do not push unless asked. End
  with the Co-Authored-By trailer.

---

### AG-P6 — The structural loop guards + per-run identity (mint, scrub, revoke, re-mintable on resume)

- **BAND.** M2.
- **ROADMAP MILESTONE.** M2-B (planning/06-roadmaps/shared/agent-fabric.md §2 "M2-B"), the loop-guards +
  per-run-identity slice.
- **DEPENDS-ON.** AG-P4 (the apply pipeline the guards re-enforce at apply time) + AG-P5 (the HITL pause that
  re-mints the token). The M2 Event-bus reactive/dispatch tier (3.6, where the guards primarily live) + the
  frozen myelin-content inline ref nodes (13.1, the reference gate reads them). The M1 Identity mint_run_token
  re-mintable-on-resume (4.7).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (event propagation/triggers across subsystems; agents first-class but bounded);
    ../../external-insights/03-agent-native-fabric.md §6 (loop prevention is structural, not convention — a
    human or agent can never typo into a loop), §2 (an agent literally cannot exceed its identity);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (the causal-loop tripwire is a quantified gate
    — loop halts <= ceiling).
  - Architecture: ../05-refined-shared-systems-architecture/agent-fabric.md §5.5 (the structural loop guards:
    self-guard (drop an inbound event whose actor.principal == this agent); reference gate (ONLY a structured
    artifact_ref node can re-trigger — never raw typed text; wired to the frozen myelin-content inline ref
    nodes); causal-depth ceiling (drop/park when depth > ceiling, default 12); shared-root tripwire (> K events
    on one correlation_id in a window trips a per-tenant circuit breaker); idempotent tools (on (run,
    effect_id)); bounded dispatch pool (drops over-cap, never forks unboundedly) — the guards read platform
    causality metadata via OutboxTx::emit(draft, cause)), §5.7 (per-run identity: mint at dispatch (token life
    == run life), scrub any shared platform token in the child env, revoke idempotently on teardown even on
    crash; C6 re-mintable mid-workflow on resume — a multi-day HITL pause re-mints a fresh attenuated token with
    the same delegation caveats and the remaining run life, so a long pause never widens the attribution
    window).
  - Contracts: contract-index.md rows 3.6 (the reactive/dispatch tier — structural loop guards, bounded
    dispatch, nested causality), 2.2 (OutboxTx::emit(draft, cause) — the causality the guards read), 13.1 (the
    frozen myelin-content inline ref nodes the reference gate keys on), 4.7 (mint_run_token re-mintable on
    resume + revoke idempotent), 5.5/§5.5 (idempotent tools on (run, effect_id)).
  - Drills: 01-whole-system-e2e-and-drill-catalogue.md rows AG-D7 (adversarial agent->agent self-trigger ->
    depth ceiling (12) + tripwire + bounded pool halt <= ceiling; per-tenant breaker trips), AG-D8 (re-asserted
    with re-mint-on-resume: a multi-day pause stays attributed within the TTL bound).
  - Roadmap: planning/06-roadmaps/shared/agent-fabric.md §2 M2-B (the loop-guards + per-run-identity bullets +
    AG-D7/AG-D8).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-agent implementation crate (the
  guards live primarily in the Bus reactive/dispatch tier; the Fabric re-enforces at apply time — defence in
  depth):
  - The five structural guards re-enforced in the Fabric's dispatch + apply path: self-guard, reference gate
    (only a structured artifact_ref node re-triggers — assert raw typed text never does), causal-depth ceiling
    (default 12, configurable), shared-root tripwire (per-tenant circuit breaker on > K events per correlation_id
    per window), idempotent tools (keyed (run, effect_id)). The bounded dispatch pool is owned by the Bus (3.6);
    the Fabric asserts it does not fork unboundedly.
  - Per-run identity (§5.7): at dispatch, request Id.mint_run_token(agent_id, run_id, delegation_caveats, ttl)
    with token life == run life; unset any shared platform token in the child environment (the anti-leak scrub);
    on teardown call Id.revoke(jti) idempotently even on crash, belt-and-suspenders with the auto-expiring
    tuple. On resume after a multi-day HITL pause, RE-MINT a fresh attenuated token with the same delegation
    caveats and the remaining run life (4.7, C6) — never leave a resumed run unattributed or widen the
    attribution window beyond the TTL bound.
  - FLOOR named: none — loop prevention is structural and complete (the ceiling default is tunable but the
    mechanism is not a floor). The agent-lane shed budget (the in-flight cap) is a separate floor tuned in
    AG-P11; note the cross-reference.
- **CONTRACTS TO IMPLEMENT.** Consumed/re-enforced: 3.6 (the dispatch-tier guards), 2.2 (nested causality), 4.7
  (re-mintable token + idempotent revoke), 13.1 (the reference-gate ref nodes). Owned: the apply-time
  re-enforcement of idempotent tools (on (run, effect_id)).
- **GATE / DRILLS (quantified; must be green to call this done).**
  - AG-D7 (adversarial agent->agent self-trigger): a deliberate loop is HALTED at or below the depth ceiling
    (12); the shared-root tripwire trips the per-tenant breaker; the bounded pool drops over-cap; the
    causal-depth + tripwire + breaker telemetry signals fire; loop halts <= ceiling (0 unbounded fork) — CI.
  - AG-D8 (re-mint-on-resume leg): a run parks for a simulated multi-day HITL pause spanning the token TTL ->
    on resume a fresh token is re-minted with the same caveats and the remaining run life; the run stays
    attributed within the TTL bound; 0 unattributed window — CI.
- **TESTS (required).** Unit tests for each guard (self-guard drop, reference-gate raw-text rejection,
  depth-ceiling drop/park, tripwire breaker trip, idempotent-tool dedup). An end-to-end CHAINED drill on the
  failure-injection harness: an agent emits an event that would re-trigger itself across N hops -> assert it
  halts <= the ceiling and the breaker trips. A re-mint test: park past the TTL, resume, assert the new token's
  caveats + remaining life + attribution. The consumer CDC for 3.6/4.7/13.1. Mutation-score floor for the guard
  module stated and met.
- **DEFINITION OF DONE.** The five structural loop guards + per-run identity (mint/scrub/revoke, re-mintable on
  resume) exist; AG-D7 is green and dated (loop halts <= ceiling, breaker trips) and the AG-D8 re-mint leg is
  green; the unit + drill + CDC tests pass; the work is committed. The depth ceiling is never raised to make a
  loop "pass" — a red AG-D7 is a dated scorecard row.
- **COMMIT.** Header: P-<NNN> M2: structural loop guards + per-run identity (re-mintable on resume). Body lists:
  3.6/2.2/4.7/13.1 wired + the apply-time idempotent-tool re-enforcement; AG-D7 greened (halt <= 12, breaker
  trips) + AG-D8 re-mint leg; no floor (shed budget AG-P11 cross-referenced). Branch first if on default; do not
  push unless asked. End with the Co-Authored-By trailer.

---

### AG-P7 — The reserve/settle cost gate as the runaway self-limiter (AG-D11) + the full run --dry-run lever

- **BAND.** M2.
- **ROADMAP MILESTONE.** M2-B (planning/06-roadmaps/shared/agent-fabric.md §2 "M2-B"), the cost-gate
  self-limiting + dry-run slice (closes the M2-B deterministic-correctness family).
- **DEPENDS-ON.** AG-P4 (the BUDGET step of the pipeline) + AG-P2 (the reserve/settle bookends). The Storage
  reserve/settle gate (11.7) + the Commercial wallet balance (11.7/C-1). The index places this after AG-P4.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (cost/irreversible-scope are decision-shaped — the gate is the bound);
    ../../external-insights/03-agent-native-fabric.md §5 (agent-generated load is the novel scale+safety concern
    — bounded by reserve/settle + the depth ceiling + idempotent tools + the bounded pool);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (stops-at-the-wallet is a quantified gate —
    refuses to start past exhaustion, never interrupts in-flight).
  - Architecture: ../05-refined-shared-systems-architecture/agent-fabric.md §5.4 (reserve at dispatch, settle on
    completion, refuse to start when balance is exhausted, NEVER interrupt one in flight; meter one cost event
    per model call and per metered effect, wholesale != markup, integer minor-units never floats; the gate
    UNIFORMLY fronts CI runs and agent runs into the SAME wallet — uniform guarantee #1), §5.2 step 5 (BUDGET),
    §7.1 (run --dry-run stops after the HITL step and shows the plan).
  - Contracts: contract-index.md rows 11.7 (reserve/settle cost gate — reserve at dispatch, no balance -> no
    start, settle on completion, never interrupt in-flight; fronts every agent run + every CI run + every
    SCHEDULE_AND_RUN_JOB), 8.7 (run --dry-run -> Vec<ProposedEffect>, no apply).
  - Drills: 01-whole-system-e2e-and-drill-catalogue.md rows AG-D11 (runaway loop vs an exhausted wallet ->
    reserve refuses new runs, never interrupts in-flight; loop stops at the wallet; reserve-refusal + 0-interrupt
    signals), AG-D9 (the dry-run is the lever the determinism golden uses).
  - Roadmap: planning/06-roadmaps/shared/agent-fabric.md §2 M2-B (the run --dry-run bullet + AG-D11; §4 row
    AG-D11).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-agent implementation crate:
  - The reserve/settle cost gate as the runaway self-limiter: every run reserves at dispatch (no balance -> the
    run never starts), settles on completion, and the gate NEVER interrupts an in-flight run. Meter one cost
    event per model call (zero for the SKELETON/Mock; the real cost event arrives with LlmAgentRuntime post-M5)
    and per metered effect; wholesale != markup kept separate; integer minor-units, never floats. CI runs and
    agent runs meter into the SAME wallet (11.7/C-1).
  - The full run --dry-run (8.7) lever surfaced as a CLI + a test API: dry_run(InboxEvent) -> Vec<ProposedEffect>
    runs steps 1..6 of the pipeline and returns the plan WITHOUT applying or metering a real effect — the lever
    every E2E + every drill + the determinism golden uses.
  - FLOOR named: none. (The real per-model-call cost metering arrives with LlmAgentRuntime post-M5; the gate
    mechanism is complete — note that the Mock meters zero, which is correct.)
- **CONTRACTS TO IMPLEMENT.** Consumed: 11.7 (reserve/settle — the cost gate). Owned: 8.7 (run --dry-run, the
  full lever). Implement to the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - AG-D11: a runaway loop against an exhausted wallet -> reserve REFUSES to start new runs (the reserve-refusal
    counter increments) and NEVER interrupts an in-flight run (the 0-interrupt signal); the loop stops at the
    wallet, not by a kill — CI.
  - run --dry-run returns the full proposed-effect plan with 0 applies and 0 metered effects (assert the wallet
    balance is unchanged after a dry-run) — CI.
- **TESTS (required).** Unit tests for the reserve/settle state (reserve -> settle balanced; reserve refused on
  exhaustion; in-flight never interrupted). An end-to-end CHAINED drill: drive a runaway mock loop into an
  exhausted wallet -> assert reserve refuses the next run, the in-flight one completes, and the loop stops at
  the wallet. A dry-run test asserting 0 applies + 0 meter + unchanged balance. The consumer CDC for 11.7 and
  the provider CDC for 8.7. Mutation-score floor for the cost-gate module stated and met.
- **DEFINITION OF DONE.** The reserve/settle self-limiter + the full run --dry-run lever exist; AG-D11 is green
  and dated (reserve refuses past exhaustion, 0 interrupt) and dry-run is proven side-effect-free; the unit +
  chained-e2e + drill + CDC tests pass; the M2-B deterministic-correctness family (AG-D1/D2/D3/D5/D7/D8/D9/D11)
  is now complete and green; the work is committed. The "stops at the wallet" threshold is never softened.
- **COMMIT.** Header: P-<NNN> M2: reserve/settle runaway self-limiter (AG-D11) + run --dry-run lever. Body
  lists: 11.7 wired + 8.7 implemented; AG-D11 greened (reserve refusals, 0 interrupt) + dry-run side-effect-free;
  the M2-B correctness family complete; no floor. Branch first if on default; do not push unless asked. End with
  the Co-Authored-By trailer.

---

### AG-P8 — ToolHands::exec on the unified sandbox + the SCHEDULE_AND_RUN_JOB long-park idiom + the AG-D4 hard escape GATE

- **BAND.** M2.
- **ROADMAP MILESTONE.** M2-C (planning/06-roadmaps/shared/agent-fabric.md §2 "M2-C — ToolHands::exec on the
  unified sandbox + the hard escape GATE"), the keystone — the M2 band's go/no-go.
- **DEPENDS-ON.** AG-P4 (the routing split: compute/external -> exec, mutate -> EffectApi) + AG-P6 (per-run
  identity inside the job) + AG-P7 (reserve at dispatch). The CI prompt(s) that ship the unified-runner skeleton
  + the hardening profile + the real-kernel escape drill harness (CI owns the runner + the drill, ADR-20 — this
  is contract 8.4's CI half; the index resolves it to CI's M2 runner prompt). The M2 Durable-workflow prompt
  that ships SCHEDULE_AND_RUN_JOB (9.2) + the durable signal (9.4). The Storage reserve/settle (11.7). The index
  places this LAST in the Fabric's M2 work — it is the band's go/no-go and AG-D4 must be green on the production
  backend before M3+ starts.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale, security by construction; agent-native);
    ../../external-insights/04-hard-problems.md §5 (untrusted-code execution is a permanent never-"done" surface;
    one escape is catastrophic; a property not drilled on a real kernel is a claim);
    ../../external-insights/01-process-and-quality-doctrine.md §2 (order-by-non-negotiability: RCE/sandbox-escape
    before any feature), §3 (prove-it on a real kernel; the green escape attestation IS the pass condition), §5
    (the gate is committed and re-run forever).
  - Architecture: ../05-refined-shared-systems-architecture/agent-fabric.md §2.2 (the hands — exec, no host-exec
    bypass + the four uniform guarantees: (1) universal cost gate, (2) per-run attenuated-token attribution, (3)
    HITL withhold via EffectApi, (4) the isolation floor + the real-kernel escape drill — the named hardening
    profile: gVisor-class userspace-kernel OR microVM; egress default-deny; read-only root + tmpfs; caps
    dropped; no-new-privileges; seccomp; digest-pinned images fail-closed on un-digested tags; whole-guest kill
    on teardown; pids.max + zero swap; secrets resolved INSIDE the boundary and never forwarded via the
    runtime), §5.0 (the routing table — exec carries ONLY compute/external untrusted code; mutation goes through
    EffectApi, never exec — the routing split is the safety boundary), §5.6 (the SCHEDULE_AND_RUN_JOB long-park
    idiom: the activity dispatches the kind=agent job (reserve at dispatch) and returns; completion arrives as a
    durable signal idempotent on idem_token hours later; the run holds no runtime — the same idiom CI's
    merge-queue uses).
  - Reconciliation: 00-reconciliation-decisions.md X-6 (the four uniform guarantees pinned; exec = CI's
    kind=agent job), OQ-F (SCHEDULE_AND_RUN_JOB).
  - Contracts: contract-index.md rows 8.4 (ToolHands::exec = CI's kind=agent job on the unified sandbox; the
    real-kernel escape drill gates both kinds; the four uniform guarantees; no host-exec bypass), 9.2
    (SCHEDULE_AND_RUN_JOB), 9.4 (the durable job.done signal idempotent on idem_token), 11.7 (reserve at
    dispatch), 1.6 (the no-host-exec lint).
  - Drills: 01-whole-system-e2e-and-drill-catalogue.md row AG-D4 / CI-T1 (the compute tool attempts a kernel
    escape on a real kernel -> ZERO escapes; green escape attestation or CI is no-go) + §3.5 (the one hard gate —
    the adversarial corpus: kernel-exploit primitives, cloud-metadata SSRF 169.254.169.254 -> cred theft,
    control-plane/internal-RPC reach, cross-tenant network/storage, fork bomb, disk fill, secret exfil via
    egress; re-run on every backend/image/kernel change).
  - Roadmap: planning/06-roadmaps/shared/agent-fabric.md §2 M2-C (the work + the AG-D4 GATE) + §4 row AG-D4 + §0
    (the AG-D4 framing as the M2->M3 go/no-go and a permanent gate).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-agent implementation crate (the
  CI-owned runner + the escape drill are CI's deliverable — this prompt builds the FABRIC side: the
  kind=agent job spec, the routing, and the consumption of the drill as a gate):
  - ToolHands::exec (8.4) realised as the dispatch of CI's kind=agent job on the ONE unified sandbox — one
    method, with NO host-execution path that bypasses it (the no-host-exec lint, 1.6, green). exec carries ONLY
    untrusted code execution (compute/external — a test, a build, a linter, a script) — the only thing that
    touches the kernel sandbox. Side-effecting mutation goes through EffectApi, never through exec (the routing
    split from AG-P4's §5.0 table is the safety boundary; assert a mutate effect can never reach exec).
  - The four uniform guarantees wired so every subsystem tool inherits them by construction (NO subsystem
    re-implements any): (1) reserve at dispatch (11.7); (2) the per-run attenuated token (AG-P6); (3) the HITL
    withhold (mutation via EffectApi, AG-P4/P5); (4) the isolation floor — feed CI's runner the named hardening
    profile in the kind=agent job spec and consume CI's real-kernel escape drill as a gate.
  - The SCHEDULE_AND_RUN_JOB long-park idiom (9.2/9.4): a long compute/external job dispatches (reserve at
    dispatch via the activity SCHEDULE_AND_RUN_JOB, which returns immediately) and the run PARKS holding no
    runtime; completion arrives hours later as a durable signal(run, "job.done", {result}, idem_key=job.idem_
    token) idempotent on idem_token (the runner can deliver "done" twice; the workflow wakes once). The Fabric
    CONSUMES this idiom; it does not reinvent durable waits.
  - Feed the kind=agent job spec to CI's unified runner and wire the consumption of AG-D4 / CI-T1 as a gate that
    must be green on the PRODUCTION backend before any downstream untrusted execution.
  - FLOOR named: there is NO floor on AG-D4 — zero escapes is both the floor and the full answer; it is a
    PERMANENT GATE (master §4), re-run on every backend/image/kernel change forever (untrusted-code execution is
    a never-"done" surface, EI-04 §5). The named follow-on inside the sandbox family is the real LlmAgentRuntime
    running its compute against this same hardened runner (post-M5, AG-P13). State this in writing.
- **CONTRACTS TO IMPLEMENT.** 8.4 (ToolHands::exec — the Fabric half: the kind=agent job spec + routing + the
  four-guarantee wiring; CI owns the runner + the drill). Consumed: 9.2/9.4 (SCHEDULE_AND_RUN_JOB + the job.done
  signal), 11.7 (reserve at dispatch), 1.6 (no-host-exec). Implement to the frozen shapes; the runner contract
  is CI's — feed it, do not redesign it.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - AG-D4 / CI-T1 (THE HARD GATE): a compute tool attempts a kernel escape on a REAL kernel against the
    adversarial corpus (kernel exploits, metadata SSRF, control-plane reach, cross-tenant network/storage, fork
    bomb, disk fill, secret exfil) -> ZERO escapes; the run emits a dated GREEN ESCAPE ATTESTATION or CI is
    no-go for untrusted code. This is the single hard go/no-go before any untrusted CI step or agent compute
    call runs in M3+; re-run on every backend/image/kernel change — GATE (permanent).
  - The no-host-exec lint green over the crate (no exec path bypasses ToolHands::exec); a mutate effect routed
    to exec is rejected (the routing-split safety boundary holds) — CI.
  - The SCHEDULE_AND_RUN_JOB idiom: a doubly-delivered job.done signal wakes the workflow EXACTLY ONCE (idem
    on idem_token); the run holds no runtime while parked — CI.
- **TESTS (required).** Unit tests for the routing split (compute/external -> exec; mutate/external-side-effect
  -> EffectApi; assert mutate can never reach exec). The AG-D4 / CI-T1 real-kernel escape drill scenario on the
  failure-injection harness against the full adversarial corpus, asserting 0 escapes and the green attestation
  (this is the permanent gate). A double-delivery test of job.done asserting exactly-once wake. The consumer CDC
  for 8.4/9.2/9.4/11.7. Mutation-score floor for the routing module stated and met.
- **DEFINITION OF DONE.** ToolHands::exec (the kind=agent job spec + the four-guarantee wiring + the routing
  split + the SCHEDULE_AND_RUN_JOB long-park idiom) exists; AG-D4 / CI-T1 emits a DATED GREEN ESCAPE
  ATTESTATION on the production backend (PROVEN, ZERO escapes — never a doc claim); the no-host-exec lint is
  green; the exactly-once job.done wake is proven; the unit + drill + CDC tests pass; the floor (none on AG-D4;
  it is a permanent gate; the real runtime is the post-M5 follow-on) is named; the work is committed. AG-D4 is
  NEVER claimed green over a red attestation — a red AG-D4 blocks ALL of M3+ and becomes a dated no-go row.
- **COMMIT.** Header: P-<NNN> M2: ToolHands::exec unified sandbox + SCHEDULE_AND_RUN_JOB + AG-D4 hard escape
  GATE. Body lists: 8.4 (Fabric half) + 9.2/9.4/11.7 wired; AG-D4/CI-T1 greened with the ZERO-escapes
  attestation (the permanent gate); the routing-split + exactly-once job.done proven; the floor named (none on
  AG-D4; LlmAgentRuntime post-M5 AG-P13). Branch first if on default; do not push unless asked. End with the
  Co-Authored-By trailer.

---

### AG-P9 — Per-producer ToolDefs (Git merge / Knowledge publish gated) + the content-addressed agent-trace holder seam

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3 (planning/06-roadmaps/shared/agent-fabric.md §2 "M3 — Per-subsystem tools register;
  the trace holder lands"), the producer-subsystem tools + the trace-holder slice.
- **DEPENDS-ON.** AG-P8 (AG-D4 green — any agent compute/edit can run) + AG-P4/P5 (the apply + HITL path the new
  tools project onto). The M3 Git prompt(s) that ship the Git ReBAC fragment (4.9) + the git.merge/open_pr
  endpoints. The M3 Knowledge prompt(s) that ship the publish/edit/draft/comment endpoints, the KN ReBAC
  fragment (4.9), and — critically — the agent-trace HOLDER (8.8: Knowledge accepts the content-addressed trace
  write + registers it as an erasable PersonalDataHolder, reusing the frozen myelin-content block model 13.1).
  The index places this in M3 after Git + Knowledge land.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (suggest-by-default; consequential actions human-confirmed; GDPR-safe by construction);
    ../../external-insights/03-agent-native-fabric.md §4 (each new tool is a projection of the existing
    plan-then-apply path — no new engine); ../../external-insights/01-process-and-quality-doctrine.md §7 (the
    compounding payoff — each new surface is smaller than the last; if a tool needs new engine, the substrate is
    wrong).
  - Architecture: ../05-refined-shared-systems-architecture/agent-fabric.md §6.3 (the frozen requires_approval
    defaults — Git merge = yes, open_pr = no; KN publish/confidential = yes, draft/comment = no; a
    cross-subsystem effect inherits the target's default), §4.5 (the trace = a content-addressed Knowledge
    document reusing the frozen myelin-content block model; a PersonalDataHolder, residency-pinned,
    crypto-shred-capable; run.trace_ref is its ArtifactRef; DISTINCT from the tamper-evident audit log).
  - Contracts: contract-index.md rows 8.1 (register_tool — the producer ToolDefs), 8.8 (AG-7 trace — Knowledge
    is the deliverable, the Fabric the seam: run.trace_ref resolves to it), 4.9 (the Git + KN ReBAC fragments
    supply the caps), 13.1 (the frozen content block model the trace reuses).
  - Drills: 01-whole-system-e2e-and-drill-catalogue.md rows KN-D11 (agent edit governed: 0 ungoverned / 0
    pre-approval / 0 double-apply — owned by Knowledge but a Fabric-loop assertion), KN-D12 (erase a subject ->
    content-addressed agent traces crypto-shredded/purged; attribution falls back to the pseudonym; 0
    recoverable PII, attribution intact — the trace-holder erasure proof, the M3 leg of AG-D10).
  - Roadmap: planning/06-roadmaps/shared/agent-fabric.md §2 M3 (the work + KN-D11/KN-D12 + the no-new-engine
    thesis) + §3 (the stateless-except-trace floor: long-term memory/RAG is a named holder seam, not built).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-agent implementation crate (NO new
  engine — register ToolDefs into the existing ToolSurface; the per-subsystem endpoints + ReBAC fragments are
  the subsystems' deliverables):
  - Register the Git producer ToolDefs into the ToolSurface (8.1): git.merge (requires_approval = yes — the
    consequential gate, AG-8) and open_pr (requires_approval = no, reversible). The required_caps come from the
    Git ReBAC fragment (4.9). Assert git.merge routes through EffectApi -> HITL withhold (AG-P5) by the frozen
    default.
  - Register the Knowledge producer ToolDefs: publish and edit(confidential_page) (requires_approval = yes,
    approver set via list_subjects) and draft / comment (requires_approval = no). The required_caps come from
    the KN ReBAC fragment (4.9).
  - Wire the agent-trace holder seam (8.8): run.trace_ref resolves to the content-addressed Knowledge document
    (the holder itself is the Knowledge deliverable). Assert the Fabric writes the trace via the
    content-addressed write reusing the frozen myelin-content block model (13.1), and that the trace registers
    as an erasable PersonalDataHolder (the KN side), distinct from the audit log.
  - FLOOR named: agent long-term memory / RAG over prior runs is a NAMED HOLDER SEAM, NOT BUILT — v1 agents are
    stateless across runs EXCEPT for the content-addressed trace document. The embedding store + its erasure are
    a Search/Knowledge follow-on (post-M5; when built it indexes via Search semantic 6.2, ACL-filtered, purged
    on *.erased). State this in writing.
- **CONTRACTS TO IMPLEMENT.** 8.1 (register the Git + KN producer ToolDefs — owned, the registration). Consumed:
  8.8 (the trace-holder seam — the Fabric resolves run.trace_ref to the KN holder), 4.9 (the Git + KN ReBAC
  fragments supply caps), 13.1 (the trace block model). Implement to the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D11 (a Fabric-loop assertion): an agent edit via EffectApi is governed — 0 ungoverned edits / 0 mutations
    before approval / 0 double-apply; a consequential publish/confidential-edit is HITL-withheld until approval;
    a double-click is one approval — CI.
  - KN-D12 (the trace-holder erasure leg, the M3 part of AG-D10): erase a subject -> the content-addressed agent
    traces are crypto-shredded/purged; attribution falls back to the opaque pseudonym; 0 recoverable PII;
    attribution intact — SCHED.
  - git.merge + KN publish/confidential carry requires_approval = yes (the frozen defaults) and route through
    the HITL withhold; open_pr / draft / comment are no — CI.
- **TESTS (required).** Unit tests asserting each producer ToolDef's frozen requires_approval default and its
  routing (gated -> EffectApi HITL; ungated -> direct apply). An end-to-end CHAINED test: a mock agent proposes
  git.merge -> withheld -> approve -> exactly one merge applied; a publish -> withheld; a draft -> applied. The
  KN-D11 governed-edit drill + the KN-D12 trace-erasure drill scenario on the harness. The consumer CDC for
  8.8/4.9/13.1. Mutation-score floor for the tool-registration module stated and met.
- **DEFINITION OF DONE.** The Git + KN producer ToolDefs are registered with their frozen requires_approval
  defaults; the agent-trace holder seam is wired (run.trace_ref resolves to the KN holder); KN-D11 is green
  (0 ungoverned / 0 pre-approval / 0 double-apply) and KN-D12 is green and dated (0 recoverable PII, attribution
  intact); no new Fabric engine was added (the compounding-payoff check holds); the unit + chained-e2e + drill +
  CDC tests pass; the floor (stateless-except-trace; long-term memory post-M5) is named; the work is committed.
- **COMMIT.** Header: P-<NNN> M3: producer ToolDefs (git.merge/KN publish gated) + agent-trace holder seam. Body
  lists: 8.1 producer ToolDefs registered + 8.8/4.9/13.1 wired; KN-D11 + KN-D12 greened with the measured 0-PII
  numbers; no new engine; the floor named (long-term memory/RAG post-M5). Branch first if on default; do not
  push unless asked. End with the Co-Authored-By trailer.

---

### AG-P10 — Per-consumer ToolDefs (Issues transition ABAC / Chat explicit-first / CI deploy gated) + AG-D4 re-confirmed on the prod CI image

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4 (planning/06-roadmaps/shared/agent-fabric.md §2 "M4 — Consumer-subsystem tools
  register; AG-D4 re-confirmed on the prod CI image").
- **DEPENDS-ON.** AG-P9 (the producer tools + the trace holder exist) + AG-P8 (AG-D4 the gate being re-confirmed)
  + AG-P5 (the HITL machinery the consumer gates use). The M4 Issues prompt(s) (the Issues ReBAC fragment 4.9 +
  the field/transition caveat), the M4 Chat prompt(s) (explicit-first dispatch 8.6 + the Chat ReBAC fragment),
  the M4 CI prompt(s) (the deploy/secret endpoints + the PRODUCTION CI runner image on which AG-D4 is re-run).
  The index places this in M4 after Issues/Chat/CI land.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (explicit, governed agent dispatch; consequential actions human-confirmed);
    ../../external-insights/03-agent-native-fabric.md §7 (explicit-first dispatch: a mention notifies, does not
    auto-spawn a costed run), §5 (the runner re-confirmed is the SAME runner the Fabric already drilled);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (AG-D4 is a permanent gate, re-run on the prod
    image), §5 (the committed gate re-runs on every image change).
  - Architecture: ../05-refined-shared-systems-architecture/agent-fabric.md §6.3 (the frozen requires_approval
    defaults — CI deploy/secret/approve_deploy = yes, run_pipeline non-prod = no; Issues forecast/triage/
    sla_draft = no, SLA transition = caveat-gated; Chat post/react = no; a cross-subsystem effect inherits the
    target's default), §3.4 (explicit-first dispatch pinned — a mention notifies via Notif's one inbox, does NOT
    auto-spawn a costed run; even an explicit run passes reserve/settle), §5.2 step 2 (the field/transition ABAC
    caveat for the Issues SLA-bound transition), §2.2 (exec = CI's kind=agent job — AG-D4 == CI-T1, the same
    gate, re-confirmed on the production image).
  - Contracts: contract-index.md rows 8.1 (register the consumer ToolDefs), 8.6 (EventInbox::deliver —
    explicit-first dispatch; implicit auto-dispatch is L-3 counsel-gated), 4.2 (check + CaveatContext for the
    transition ABAC), 4.9 (the Issues + Chat + CI ReBAC fragments), 8.4 / CI-T1 (the prod-image re-confirm),
    11.7 (reserve gates even the explicit run).
  - Drills: 01-whole-system-e2e-and-drill-catalogue.md rows CI-T1 / AG-D4 (re-confirmed green on the production
    CI runner image — the M4 hard GATE), CHAT-D17 (a casual @agent mention -> 0 auto-spawn, reserve gate on the
    explicit run), CHAT-D9/D10 (HITL bridge across a Chat+Workflow kill -> gated tool runs exactly once,
    double-click is one approval; batch 2-of-3 per-effect idempotency, the withheld never mutates), ISS-D12 (an
    agent hitting a governed transition is HITL-gated, withheld, no mutation until approval).
  - Roadmap: planning/06-roadmaps/shared/agent-fabric.md §2 M4 (the work + the AG-D4 re-confirm GATE +
    CHAT-D17/CHAT-D9/D10/ISS-D12) + the [OPEN -> LEGAL] L-3 floor.
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-agent implementation crate (again
  NO new engine — register ToolDefs + wire explicit-first dispatch + re-confirm the gate):
  - Register the Issues consumer ToolDefs (8.1): forecast / triage / sla_draft (requires_approval = no,
    advisory/suggest) and transition(issue, ->done) on an SLA-bound issue (requires_approval = yes IF the
    transition has an approver edge — the field/transition ABAC caveat evaluated via check + CaveatContext, 4.2,
    §5.2 step 2). The Issues ReBAC fragment (4.9) supplies the caps.
  - Register the Chat consumer ToolDefs: post_message / react (requires_approval = no, reversible) and any
    EffectApi tool that mutates another subsystem (inherits THAT subsystem's default — "governed where it
    lands"). Wire EXPLICIT-FIRST dispatch (8.6): a casual @agent mention NOTIFIES the inbox and does NOT
    auto-spawn a costed run; only an explicit action / structured trigger dispatches; reserve/settle gates even
    the explicit run.
  - Register the CI consumer ToolDefs: deploy(env) / approve_deploy / write_secret (requires_approval = yes) and
    run_pipeline non-prod (no). CI's runner IS the Fabric's ToolHands::exec runner (ADR-20) — re-confirm AG-D4 /
    CI-T1 green on the PRODUCTION CI runner IMAGE (the M4 hard gate; re-run on the image).
  - FLOOR named: implicit auto-dispatch on a casual mention remains [OPEN -> LEGAL] (L-3, counsel-gated — GDPR
    Art. 22 / EU AI-Act human-oversight). Explicit-first is v1; NO auto-spawn path is wired until counsel
    ratifies the human-oversight basis. State this in writing as the defensible posture.
- **CONTRACTS TO IMPLEMENT.** 8.1 (register the Issues + Chat + CI consumer ToolDefs — owned), 8.6
  (explicit-first dispatch — owned wiring). Consumed: 4.2 (CaveatContext for the transition ABAC), 4.9 (the
  consumer ReBAC fragments), 8.4 / CI-T1 (the prod-image re-confirm), 11.7 (reserve on the explicit run).
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CI-T1 / AG-D4 re-confirmed GREEN on the PRODUCTION CI runner image (ZERO escapes; the dated green attestation
    on the prod image) — GATE (the M4 hard gate; permanent, re-run on the image).
  - CHAT-D17: a casual @agent mention -> 0 auto-spawn (the dispatch-counter stays 0); the explicit run passes
    the reserve gate — CI.
  - CHAT-D9 / CHAT-D10: across a Chat+Workflow kill, a gated tool runs EXACTLY ONCE, a double-click is one
    approval; a batch 2-of-3 approval applies per-effect (the withheld never mutates) — CI.
  - ISS-D12: an agent hitting a governed (SLA-bound, approver-edged) transition is HITL-gated, withheld, no
    mutation until approval — CI.
- **TESTS (required).** Unit tests for each consumer ToolDef's frozen default + the transition-ABAC caveat
  evaluation. An end-to-end CHAINED test: a casual mention -> notify only (0 spawn); an explicit trigger ->
  reserve -> run; a governed transition -> withheld -> approve -> exactly one apply (across a simulated kill).
  The CHAT-D17 / CHAT-D9 / CHAT-D10 / ISS-D12 drill scenarios on the harness, and the AG-D4 / CI-T1 re-confirm
  on the production image. The consumer CDC for 8.6/4.2/4.9. Mutation-score floor for the consumer-dispatch
  module stated and met.
- **DEFINITION OF DONE.** The Issues + Chat + CI consumer ToolDefs are registered with their frozen defaults;
  explicit-first dispatch is wired (0 auto-spawn on a casual mention); AG-D4 / CI-T1 is re-confirmed green and
  dated on the production CI runner image; CHAT-D17 / CHAT-D9 / CHAT-D10 / ISS-D12 are green; no new engine; the
  unit + chained-e2e + drill + CDC tests pass; the floor (implicit auto-dispatch L-3, not wired) is named; the
  work is committed. AG-D4 is never claimed green on the prod image over a red attestation.
- **COMMIT.** Header: P-<NNN> M4: consumer ToolDefs (Issues transition ABAC / Chat explicit-first / CI deploy)
  + AG-D4 re-confirmed on the prod image. Body lists: 8.1 consumer ToolDefs + 8.6 explicit-first wired; AG-D4/
  CI-T1 re-confirmed green on the prod image + CHAT-D17/D9/D10 + ISS-D12 greened; the floor named (auto-dispatch
  L-3). Branch first if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### AG-P11 — The 30x agent-dispatch surge family (AG-D6): the human lane holds, the agent lane sheds, the shed budget tuned

- **BAND.** M5.
- **ROADMAP MILESTONE.** M5 (planning/06-roadmaps/shared/agent-fabric.md §2 "M5 — World-scale hardening"), the
  surge-family + shed-budget slice.
- **DEPENDS-ON.** AG-P10 (all consumer tools + the prod runner exist) + AG-P7 (reserve/settle). The substrate
  shed order + the ResilientClient honouring Retry-After (1.9/1.11). The failure-injection harness's 1x/10x/30x
  load generator with mixed principal kinds (M0). The index places this in M5 after all five subsystems exist.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale from day 1; agents generate volume far beyond humans);
    ../../external-insights/03-agent-native-fabric.md §5 (the novel scale+safety concern is agent-generated
    load; the agent lane is the shed-before-human lane; an unbounded lane is the cascade; the agent runtime MUST
    honour Retry-After or shedding becomes a retry storm); ../../external-insights/01-process-and-quality-
    doctrine.md §3 (the surge drill is a quantified gate — human lane within budget, agent sheds, cross-tenant
    impact 0; the shed budget is set by MEASUREMENT, not prediction).
  - Architecture: ../05-refined-shared-systems-architecture/agent-fabric.md §8 (the C10 floor: the
    agent-mention-storm shed budget named as a v1 floor — a per-tenant agent-run in-flight cap (reserve/settle
    refuses over-cap), humans NEVER queue behind agent runs (the protected human lane), the agent lane sheds with
    429 + Retry-After honoured by the runtime; the concrete number is the budget call TUNED by the 30x
    agent-surge drill — the floor is "the agent lane is bounded, has a reserved human lane, and applies the shed
    order"), §7.3 (the resilient client + backpressure — the shed order speculative -> batch/CI -> agent ->
    human-last; a 429 + Retry-After to an agent is an ordinary backoff surfaced as a transient tool error).
  - Reconciliation: 00-reconciliation-decisions.md OQ-K (the per-surface shed budgets named as v1 floors tuned
    by drills).
  - Contracts: contract-index.md rows 1.11 (the protected-human-lane shed order + per-surface shed budgets), 1.9
    (ResilientClient honours Retry-After), 11.7 (reserve/settle refuses over-budget), 1.8 (the survival signals:
    shed-counts, reserve-refusals, RED/USE per principal-kind — the green artifacts).
  - Drills: 01-whole-system-e2e-and-drill-catalogue.md row AG-D6 (30x agent dispatch surge -> human lane holds,
    agent sheds, reserve/settle refuses over-budget runs, others unaffected; shed-counts + reserve-refusal
    signals — the named shed budget asserted).
  - Roadmap: planning/06-roadmaps/shared/agent-fabric.md §2 M5 (the F6 surge work + AG-D6) + §3 (the agent-lane
    shed budget floor -> the measured cap, trigger AG-D6) + §4 row AG-D6.
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-agent implementation crate + the
  thresholds file (the versioned defaults-to-beat):
  - The agent-lane shed budget made concrete: the per-tenant agent-run in-flight cap (reserve/settle refuses
    over-cap), the protected human lane (humans never queue behind agent runs), and the agent lane shedding with
    429 + Retry-After that the agent runtime HONOURS (1.9). The agent lane is the shed-before-human lane (shed
    order speculative -> batch/CI -> agent -> human-last).
  - Run the 30x agent-dispatch surge drill (AG-D6) with the harness's mixed-principal load generator; READ the
    measured cap off the telemetry (the cap is set by measurement, not predicted) and WRITE it into the
    thresholds file as the tuned default-to-beat. The per-tenant bulkhead must hold: a 30x surge on one tenant
    leaves OTHER tenants unaffected (cross-tenant impact 0).
  - FLOOR named: the shed budget was the M2 floor (the bound existed; the number was a placeholder). This prompt
    is the named follow-on — the MEASURED cap. State the before (placeholder) and after (measured) values in the
    thresholds file with the date.
- **CONTRACTS TO IMPLEMENT.** Consumed: 1.11 (the shed order + the agent-lane budget), 1.9 (Retry-After honoured),
  11.7 (reserve refuses over-budget). Owned: the thresholds-file row for the measured agent-lane cap.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - AG-D6: a 30x agent-dispatch surge -> the protected human lane HOLDS (human-lane latency within budget), the
    agent lane SHEDS (429 + Retry-After; the shed-counts signal rises), reserve/settle REFUSES over-budget runs
    (the reserve-refusal counter rises), and OTHER tenants are unaffected (cross-tenant impact = 0); the named
    agent-lane shed budget is asserted (the measured cap is written to the thresholds file) — SCHED.
- **TESTS (required).** A surge drill scenario on the failure-injection harness driving 1x/10x/30x agent
  dispatch with mixed principal kinds, asserting the human-lane budget, the agent shed-counts, the reserve
  refusals, and the 0 cross-tenant impact. A property test that the runtime honours Retry-After (no retry
  storm). The consumer CDC for 1.11/1.9/11.7. (No new core module needs a mutation floor; if the shed-decision
  logic is new, state its floor.)
- **DEFINITION OF DONE.** The agent-lane shed budget is tuned by AG-D6 and written to the thresholds file
  (dated); AG-D6 is green and dated (human lane holds, agent sheds, reserve refuses over-budget, cross-tenant
  impact 0); the runtime honours Retry-After (no storm); the drill + CDC tests pass; the floor (the placeholder
  -> the measured cap) is named with both values; the work is committed. The cap is set by measurement, never by
  a number chosen to make the drill pass.
- **COMMIT.** Header: P-<NNN> M5: 30x agent-surge family (AG-D6) — human lane holds, agent sheds, shed budget
  tuned. Body lists: 1.11/1.9/11.7 wired; AG-D6 greened with the measured human-lane latency + shed-counts +
  reserve-refusals + 0 cross-tenant impact; the measured shed cap written to the thresholds file. Branch first
  if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### AG-P12 — Erasure reaches the trace + agent memory (AG-D10): the Fabric's full DSR holder legs

- **BAND.** M5.
- **ROADMAP MILESTONE.** M5 (planning/06-roadmaps/shared/agent-fabric.md §2 "M5"), the erasure-fan-out slice
  (the Fabric's legs of the full DSR across all H1-H18 holders).
- **DEPENDS-ON.** AG-P1 (the holder registration seam) + AG-P9 (the trace holder + the KN-D12 trace-erasure leg).
  The M5 GDPR prompt(s) that ship the full dsr_submit fan-out (10.4) + the ONE erasure posture (10.9). The
  Storage per-subject DEK crypto-shred (11.3/11.4). The Identity resolve_pseudonym/erase (4.8). The index places
  this in M5 after every holder exists.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe & EU-sovereign by construction: erasure is architectural);
    ../../external-insights/04-hard-problems.md §1 (erasure vs immutability — crypto-shred + pseudonymise, never
    hide); ../../external-insights/01-process-and-quality-doctrine.md §3 (0 recoverable PII is a quantified gate,
    proven on a real erase, including backups).
  - Architecture: ../05-refined-shared-systems-architecture/agent-fabric.md §9 row D-10 (erasure reaches the
    trace + memory — reads the ONE erasure posture 10.9 by reference, instantiated for the Fabric: run / trace /
    memory, NOT restated), §4.5 (the trace is a PersonalDataHolder, residency-pinned, crypto-shred-capable;
    attribution -> pseudonym on erase), §3 (the structural trace-erasure floor: per-subject DEK crypto-shred +
    pseudonym shred -> the full DSR fan-out is the M5 follow-on, AG-D10 owed).
  - Contracts: contract-index.md rows 10.1 (PersonalDataHolder{locate, export, rectify, restrict, erase} — the
    Fabric's run/trace/memory holders), 10.9 (the ONE free-text/immutable erasure posture — instantiated by
    reference), 10.4 (the DSR fan-out iterates holders), 11.3/11.4 (per-subject DEK crypto-shred), 4.8
    (resolve_pseudonym/erase — attribution falls back to the pseudonym), 6.2 (Search semantic — the memory/
    embedding leg, if the memory floor has been promoted; else the seam is named).
  - Drills: 01-whole-system-e2e-and-drill-catalogue.md row AG-D10 (erase a subject -> run trace + agent memory/
    embeddings crypto-shredded/purged; attribution -> opaque pseudonym; 0 recoverable PII).
  - Roadmap: planning/06-roadmaps/shared/agent-fabric.md §2 M5 (the erasure-fan-out work + AG-D10) + §4 row
    AG-D10.
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-agent implementation crate:
  - Implement the PersonalDataHolder bodies (10.1) for the Fabric's holders: the run table, the
    proposed_effect / hitl_gate rows that carry inline PII, the Conversation history, and the trace (via the KN
    trace holder, AG-P9). erase(subject) crypto-shreds the per-subject DEK (11.3/11.4) for self-authored
    free-text + purges the memory/embeddings, and attribution falls back to the opaque pseudonym
    (resolve_pseudonym, 4.8). Reads the ONE erasure posture (10.9) by reference — do NOT restate the posture;
    instantiate it for run / trace / memory.
  - Wire the Fabric's holders into the GDPR DSR fan-out (10.4) so dsr_submit reaches them; the residual
    (third-party/immutable free-text PII authored by others) is the documented lawful-basis limit per 10.9, not
    a new posture.
  - The agent-memory leg: if the long-term-memory/RAG floor has been promoted (post-M5), erase purges the
    embedding store via Search and the *.erased path; if NOT yet built, the structural seam is named (the holder
    is registered, the body is a no-op-with-a-named-follow-on) — state which case applies.
  - FLOOR named: if agent long-term memory/RAG is not yet built, the memory-erasure body is the named structural
    seam (the per-subject DEK + the *.erased purge path exist; the embedding store is the post-M5 follow-on).
    State this honestly (yes/no/partial per EI-01 §4).
- **CONTRACTS TO IMPLEMENT.** 10.1 (the Fabric's PersonalDataHolder bodies — owned: locate/export/erase for run/
  trace/memory). Consumed: 10.9 (the posture, by reference), 10.4 (the DSR fan-out), 11.3/11.4 (crypto-shred),
  4.8 (pseudonym fallback), 6.2 (the memory leg, if promoted).
- **GATE / DRILLS (quantified; must be green to call this done).**
  - AG-D10: erase a subject -> the run trace + the agent memory/embeddings are crypto-shredded/purged (the
    per-subject DEK destroyed; the embedding store purged or the seam named); attribution -> an opaque pseudonym;
    0 recoverable PII (including in backups, via the post-restore re-erasure path); the erase-receipt signal —
    SCHED.
- **TESTS (required).** Unit tests for each holder's locate/export/erase. An end-to-end erasure drill on the
  harness: write a run + trace + (memory if present) for a subject -> erase -> assert 0 recoverable PII (probe
  the trace, the run inline fields, the memory) and that attribution renders the pseudonym. A post-restore
  re-erasure assertion (the erasure ledger drives re-erasure after a backup restore). The consumer CDC for
  10.1/10.9/10.4/11.4/4.8. Mutation-score floor for the holder/erase module stated and met.
- **DEFINITION OF DONE.** The Fabric's PersonalDataHolder bodies (run / trace / memory) are implemented and
  wired into the DSR fan-out, reading the ONE erasure posture by reference; AG-D10 is green and dated (0
  recoverable PII, attribution -> pseudonym, including post-restore); the memory leg's status (built or named
  seam) is honestly recorded; the unit + drill + CDC tests pass; the work is committed. The 0-recoverable-PII
  threshold is never softened.
- **COMMIT.** Header: P-<NNN> M5: erasure reaches the trace + memory (AG-D10) — Fabric DSR holder legs. Body
  lists: 10.1 holder bodies + 10.9/10.4/11.4/4.8 wired; AG-D10 greened with the measured 0-recoverable-PII +
  pseudonym attribution; the memory-leg status (built / named seam) recorded. Branch first if on default; do not
  push unless asked. End with the Co-Authored-By trailer.

---

### AG-P13 — The E2E-2 flagship: CI-fail -> triage agent -> issue -> chat -> fix-PR across a kill + name the LlmAgentRuntime post-M5 swap

- **BAND.** M5.
- **ROADMAP MILESTONE.** M5 (planning/06-roadmaps/shared/agent-fabric.md §2 "M5"), the whole-system E2E wedge
  flagship (the agent-native differentiator proof) + the named LlmAgentRuntime follow-on.
- **DEPENDS-ON.** AG-P12 (erasure) + AG-P11 (surge) + AG-P10 (all consumer tools) + AG-P8 (AG-D4) + AG-P5 (HITL).
  This is a whole-system scenario: the M4 CI (the CheckStatus producer + ci.result), Git (git.merge + the merge
  queue), Issues (create_issue), Chat (post_chat_message), Notif (the HITL card), and the M2 durable-workflow
  signal must all be green. The index places this near the END of M5 (it is the band's flagship gate).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1 (autonomous agents are first-class citizens — the differentiator), §3 (mock agents during
    development; the strategy pattern); ../../external-insights/03-agent-native-fabric.md (all — the
    agent-native flagship is the proof of the whole thesis); ../../external-insights/01-process-and-quality-
    doctrine.md §3 (the E2E emits its named green artifact: deterministic run trace + HITL withhold->approve->
    apply ledger + reserve/settle parity + merge-count == 1), §4 (chained mutations end-to-end across a kill).
  - Architecture: ../05-refined-shared-systems-architecture/agent-fabric.md §5.2/§5.3 (plan-then-apply + HITL —
    the spine of E2E-2), §5.6 (the run is a durable workflow that survives a kill; the SCHEDULE_AND_RUN_JOB
    idiom), §5.7 (the per-run token re-minted on resume — the multi-day approval), §3.3 (LlmAgentRuntime is
    designed-not-built — the only vendor seam, post-M5, after the safety drills are green; the no-llm-in-platform
    lint; EU-hostable region-aware; a config/impl swap behind the frozen AgentRuntime seam, NOT a rewrite).
  - Contracts: contract-index.md rows 8.5 (Agent::handle — the loop spine), 8.2 (EffectApi — the create_issue
    applies, the git.merge is withheld), 9.4 (the durable signal — the approval days later + the merge-queue
    ci.result wait), 4.7 (re-mint on resume), 5.9 (the CheckStatus seam + ci.result), 11.7 (reserve at dispatch
    + the exhausted-wallet variant).
  - Drills: 01-whole-system-e2e-and-drill-catalogue.md §2 E2E-2 (CI-fail -> triage agent -> issue -> chat ->
    fix-PR; the full scenario steps) + the E2E-2 gate row (0 effect outside the ∩; 0 mutation before approval;
    exactly-once approval + merge across a kill; reserve/settle balanced; merge-count == 1; deterministic run
    trace).
  - Roadmap: planning/06-roadmaps/shared/agent-fabric.md §2 M5 (the E2E-2 work bullet — the full step list) + §4
    row E2E-2 + §3 (the LlmAgentRuntime floor row, post-M5, the safety-drills-green trigger).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-agent E2E test harness (the
  whole-system scenario against a full cell with MOCK agents) + the LlmAgentRuntime seam doc:
  - The E2E-2 scenario wired as an automated whole-system test against a full cell with mock agents: a push
    fails CI; a Signal (NOT a casual mention — explicit-first, Signal-driven) wakes a MOCK triage agent; reserve
    at dispatch (include the exhausted-wallet variant -> refuse-to-start); the agent plans
    [create_issue, post_chat_message, open_pr] DETERMINISTICALLY (AG-D9); create_issue APPLIES (no approval);
    post_chat_message applies; a git.merge proposal is WITHHELD (requires_approval = yes, returns Gated/Denied,
    does NOT mutate); the Agent + Workflow services are KILLED mid-ack_window; the human approves DAYS LATER
    (double-click); the durable workflow resumes, RE-MINTS the token (4.7), consumes the approval EXACTLY ONCE,
    and the merge applies ONCE (no double-effect); the fix-PR's CI goes green; the merge-queue wakes on
    ci.result idempotently and merges; git.pr.merged closes the issue. Assert the named green artifact at every
    step.
  - Name (do NOT build) the LlmAgentRuntime as the post-M5 follow-on in a seam doc: the trait seam AgentRuntime
    is frozen; the real adapter (the only place a model/SDK/prompt/model-name string appears, enforced by the
    no-llm-in-platform lint) is EU-hostable, region-aware, swappable; it meters one cost event per model call
    (wholesale != markup); it is swapped in AFTER the safety drills (AG-D4/D2/D3/D5) are green — a config/impl
    swap, NOT a rewrite. The EU-sovereign sub-processor is [OPEN -> LEGAL] (AG-9). State the trigger (the safety
    drills green) explicitly.
  - FLOOR named: LlmAgentRuntime is the post-M5 floor (designed, seam frozen, not built — the trigger is the
    safety drills green, which this E2E proves); the external MCP endpoint is post-M5; the agent long-term
    memory/RAG is post-M5. The E2E-2 flagship runs on the MOCK runtime (VISION §3 — mock during development).
- **CONTRACTS TO IMPLEMENT.** None new owned — E2E-2 EXERCISES the already-built Fabric contracts (8.5/8.2/8.4/
  9.4/4.7/11.7) end-to-end across a kill, plus the cross-subsystem seams (5.9 ci.result, git.merge, create_issue,
  post_chat_message). The LlmAgentRuntime seam is NAMED, not implemented.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - E2E-2 GREEN: 0 effect outside the agent.policy ∩ delegation ∩ tenant.policy; 0 mutation before approval;
    EXACTLY-ONCE approval + merge across the service kill; reserve/settle BALANCED (reserved == settled);
    MERGE-COUNT == 1 (no double-merge); a DETERMINISTIC run trace (the proposed-effect sequence identical across
    two runs); the named green artifact (the deterministic run trace + the HITL withhold->approve->apply ledger
    + the reserve/settle parity + merge-count == 1) emitted — SCHED (the flagship green artifact).
- **TESTS (required).** The E2E-2 chained-mutation scenario as an automated SCHED test on the full-cell harness
  with mock agents, asserting every named green-artifact property across the mid-ack_window kill and the
  multi-day approval. A re-mint-on-resume assertion (the token is fresh after the kill). An exhausted-wallet
  variant (reserve refuses the start). The cross-system CDC for the seams it drives (5.9 ci.result, git.merge).
  No new core module; the E2E asserts against already-mutation-covered modules.
- **DEFINITION OF DONE.** The E2E-2 flagship scenario is wired and GREEN and dated (0 effect outside the ∩; 0
  mutation before approval; exactly-once approval + merge across the kill; reserve/settle balanced; merge-count
  == 1; deterministic run trace); the LlmAgentRuntime post-M5 swap is NAMED in the seam doc with its
  safety-drills-green trigger (designed, not built); the floor (mock runtime; real runtime post-M5; external MCP
  post-M5; memory post-M5) is named; the E2E + CDC tests pass; the work is committed. The exactly-once + 0-leak +
  merge-count == 1 thresholds are never softened — a red E2E-2 is a dated scorecard row, never edited green.
- **COMMIT.** Header: P-<NNN> M5: E2E-2 flagship (CI-fail -> triage -> issue -> chat -> fix-PR across a kill) +
  name LlmAgentRuntime post-M5 swap. Body lists: E2E-2 greened with the measured 0-leak / 0-pre-approval-mutation
  / exactly-once / merge-count==1 / deterministic-trace numbers; the LlmAgentRuntime + external-MCP + memory
  floors named with their post-M5 triggers. Branch first if on default; do not push unless asked. End with the
  Co-Authored-By trailer.

---

### AG-P14 — Dogfood: the platform's own agents run on the platform's own commits/issues/chat

- **BAND.** M6.
- **ROADMAP MILESTONE.** M6 (planning/06-roadmaps/shared/agent-fabric.md §2 "M6 — Dogfooding: the platform's own
  agents run on the platform").
- **DEPENDS-ON.** AG-P13 (E2E-2 green) + AG-P11 (AG-D6) + AG-P12 (AG-D10) + AG-P8 (AG-D4). The M6 dogfood
  prompts of the other systems (the self-hosting CI graph, the Myelin git hosting + Issues + Knowledge + Chat).
  The index places this in M6 after the platform is world-scale-ready and self-hosting.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §5 (execution + dogfooding) / §3 (the every-incident-adds-a-drill loop);
    ../../external-insights/01-process-and-quality-doctrine.md §1 (the code wins over the docs; the truth-up
    pass), §3 (every real incident ends by adding a drill that reproduces it), §4 (drive the real thing).
  - Architecture: ../05-refined-shared-systems-architecture/agent-fabric.md §0 framing (the Fabric rides the M6
    dogfood) + §9 (every PROVEN row rests on a dated green artifact).
  - Contracts: contract-index.md rows 8.5/8.2/8.4/11.7 (the Fabric runs unchanged on the self-hosting graph —
    no new contract), 1.8 (the telemetry — balanced reserve/settle ledgers + traces are the dogfood green
    artifacts).
  - Drills: 01-whole-system-e2e-and-drill-catalogue.md §1 (dogfooding is the cheapest honest load generator) +
    the M6 done-bar (the self-hosting CI graph green; no later-band gate red).
  - Roadmap: planning/06-roadmaps/shared/agent-fabric.md §2 M6 (the work + the folded-in done-bar) + §5 digest.
- **DELIVERABLE (what to build + exactly where in the repo).** In the Myelin self-hosting configuration (the
  dogfood loop), not new Fabric engine:
  - Wire a triage agent (mock in v1; the real LlmAgentRuntime IF the post-M5 swap has landed) onto the
    self-hosting CI graph: when the platform's own CI fails, the triage agent runs (explicit-first / Signal-
    driven), writes an agent-trace holder for the platform's own runs, and the every-incident-adds-a-drill loop
    files a Myelin issue + a reproducing drill.
  - Confirm the Fabric's runs on the self-hosting graph emit BALANCED reserve/settle ledgers + traces (the
    dogfood green artifacts); run the truth-up pass that confirms every PROVEN Fabric row (AG-D1..AG-D11, E2E-2)
    rests on a DATED green artifact, never a doc claim (code-wins-over-docs); confirm NO later-band Fabric gate
    is red (the gate invariant holds end-to-end).
  - FLOOR named: if the real LlmAgentRuntime swap has not yet landed, the dogfood agents run on the MOCK runtime
    (correct per VISION §3 during development) — state this honestly; the real-runtime swap remains the named
    post-M5/execution follow-on.
- **CONTRACTS TO IMPLEMENT.** None new — the Fabric runs unchanged on the self-hosting graph (8.5/8.2/8.4/11.7
  exercised on real platform data). The dogfood is a configuration + a truth-up pass, not new engine.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The Fabric's runs on the self-hosting CI graph emit balanced reserve/settle ledgers + traces (reserved ==
    settled on every run; a trace per run) — SCHED.
  - The truth-up pass confirms every PROVEN Fabric row rests on a dated green artifact (0 rows resting on a doc
    claim); no later-band Fabric gate is red (the gate invariant holds end-to-end) — SCHED.
- **TESTS (required).** The dogfood loop exercised on the real platform: a real CI failure on a Myelin commit
  triggers a triage run with a balanced ledger + a trace; the every-incident-adds-a-drill loop files an issue +
  a drill. The truth-up pass as a checklist over the AG-D drill scorecard (each row dated + green). No new unit
  module; the dogfood asserts against the live system.
- **DEFINITION OF DONE.** The platform's own agents run on the self-hosting graph with balanced reserve/settle
  ledgers + traces; the truth-up pass confirms every PROVEN Fabric row rests on a dated green artifact and no
  later-band Fabric gate is red; the every-incident-adds-a-drill loop is live; the floor (mock vs real runtime)
  is named; the work is committed. No PROVEN claim outlives its dated verification.
- **COMMIT.** Header: P-<NNN> M6: dogfood — the platform's own agents run on its own commits/issues/chat. Body
  lists: the self-hosting triage agent wired + balanced ledgers + traces; the truth-up pass confirming 0 red
  later-band Fabric gates + every PROVEN row dated; the floor named (mock vs real runtime). Branch first if on
  default; do not push unless asked. End with the Co-Authored-By trailer.

---

## Coverage digest

Every agent-fabric roadmap milestone maps to at least one prompt (planning/06-roadmaps/shared/agent-fabric.md
§2):

- **M2-A** (SKELETON: the substrate path at zero cost) -> AG-P1 (glue crate + data model + no-llm lint), AG-P2
  (the SKELETON runtime + AG-D8 no-tool leg).
- **M2-B** (Mock + plan-then-apply + HITL, the deterministic-correctness family) -> AG-P3 (MockAgentRuntime +
  AG-D9 step-determinism), AG-P4 (EffectApi plan-then-apply + frozen requires_approval defaults + AG-D1/D2/D3/D9),
  AG-P5 (HITL withhold/approve/resume + per-effect idempotency + AG-D5), AG-P6 (structural loop guards + per-run
  identity + AG-D7/D8), AG-P7 (reserve/settle self-limiter + dry-run + AG-D11).
- **M2-C** (the unified sandbox + the hard GATE) -> AG-P8 (ToolHands::exec + SCHEDULE_AND_RUN_JOB + AG-D4 / CI-T1
  the permanent escape GATE).
- **M3** (per-producer tools + the trace holder) -> AG-P9 (Git merge / KN publish ToolDefs + the agent-trace
  holder seam + KN-D11/KN-D12).
- **M4** (per-consumer tools + AG-D4 re-confirm) -> AG-P10 (Issues transition ABAC / Chat explicit-first / CI
  deploy ToolDefs + AG-D4 re-confirmed on the prod image + CHAT-D17/D9/D10/ISS-D12).
- **M5** (world-scale + the flagship + erasure) -> AG-P11 (AG-D6 surge + shed budget), AG-P12 (AG-D10 erasure
  fan-out), AG-P13 (E2E-2 flagship + the named LlmAgentRuntime post-M5 swap).
- **M6** (dogfood) -> AG-P14 (the platform's own agents + the truth-up pass).

Drills greened across the ledger: AG-D8 (AG-P2/P6), AG-D9 (AG-P3/P4), AG-D1/D2/D3 (AG-P4), AG-D5 (AG-P5), AG-D7
(AG-P6), AG-D11 (AG-P7), AG-D4 / CI-T1 (AG-P8, re-confirmed AG-P10 — the permanent GATE), KN-D11/KN-D12 (AG-P9),
CHAT-D17/CHAT-D9/CHAT-D10/ISS-D12 (AG-P10), AG-D6 (AG-P11), AG-D10 (AG-P12), E2E-2 (AG-P13). All eleven AG-D
drills + the E2E-2 flagship + the borrowed KN/CHAT/ISS agent-loop assertions are greened by a named prompt; no
drill is left ungated.

Floors + follow-ons, each paired across bands: Mock runtime (AG-P3, M2-B) -> LlmAgentRuntime named (AG-P13, M5;
built post-M5/execution); the agent-lane shed budget placeholder (AG-P6/M2) -> the measured cap (AG-P11, M5);
the external MCP seam (AG-P4, M2 — the exposed_over_mcp column) -> the external endpoint (post-M5, named in
AG-P13); stateless-except-trace (AG-P9, M3) -> long-term memory/RAG (post-M5, the holder seam in AG-P12); the
structural trace-erasure floor (AG-P1/M2) -> the full DSR fan-out (AG-P12, M5). The [OPEN -> LEGAL] items
(implicit auto-dispatch L-3 named in AG-P10; reasoning-capture L-4; build-data-as-training foreclosed) are
flagged, not wired.
