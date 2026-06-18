# Phase 3 — Agent Fabric (`myelin-agent`): strategy pattern, plan-then-apply

> Phase: `03-shared-systems-architecture`. Canonical brief: [`VISION.md`](../../VISION.md). Doctrine
> bound: [`external-insights/03-agent-native-fabric.md`](../../external-insights/03-agent-native-fabric.md)
> (all), [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md)
> §2/§5/§6, [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §5.1.
> Directives bound: [`integration-directives.md`](../02b-doctrine-integration/integration-directives.md)
> Agent Fabric **AG-1…AG-8**, plus **X-1…X-5**, **CI-1/CI-2**, **CHAT-1**. Spine bound: **ADR-08, ADR-09**
> (also ADR-03, ADR-16, ADR-17, ADR-19, ADR-20, ADR-11, ADR-12, ADR-13). Decision-record §(c): **D6**
> (brain+hands+skeleton), D5/ADR-20 (one sandbox), D8 (reserve/settle), D7 (orchestrator gotchas).
> **Resolves AG-3** (the `Agent::handle` shape).
>
> **Foundational docs this design consumes (read first; not re-invented here):**
> [`00-platform-substrate.md`](./00-platform-substrate.md) (the consumer template §5, the resilient client
> §6, backpressure §7, `myelin-agent` trait stubs §2.4, the envelope/outbox §2.1),
> [`identity-and-access.md`](./identity-and-access.md) (`check`/`list_objects`/`delegation`/`mint_run_token`/
> `revoke`, the `agent.policy ∩ delegation ∩ tenant.policy` algebra §7, per-run attenuable tokens §4),
> [`event-bus.md`](./event-bus.md) (the four primitives, the `EventInbox` delivery via the reactive/dispatch
> tier §4.7, Signals, causality derivation, loop guards). Where this doc needs a change in a foundational
> system it says so explicitly (§12).
>
> **Status convention.** *DECIDED* = committed for Phase 4/5. *FLOOR* = a partial answer shipped with a
> named follow-on. *[OPEN → P4/P5/LEGAL]* = handed forward. Snippets are Rust-shaped signatures for the
> **contract surface** (ADR-02: glue crates are Rust); they are signatures, not implementations.

---

## 0. Reading map & floors named up front

- **§1** — purpose & responsibilities; the trait set at a glance.
- **§2** — the two strategy boundaries (D6): the **brain** (`step`, AG-1) and the **hands** (`exec`, AG-2),
  and the platform-owned agent loop that owns history. **Resolves AG-3.**
- **§3** — the three runtimes: **SKELETON** (first), **MockAgentRuntime** (deterministic, `--use-mock`),
  **LlmAgentRuntime** (the only place a vendor is named; not built in P3).
- **§4** — the data model / schemas: `Run`, `Conversation`, `ToolDef`/registry, `ProposedEffect`, the trace.
- **§5** — the algorithms: the agent loop, **plan-then-apply `EffectApi`**, HITL withhold→approve→resume,
  reserve/settle, the structural loop guards.
- **§6** — the permissioned tool registry + the MCP-exposure path.
- **§7** — contracts/APIs exposed and consumed (the glue). **Stable.**
- **§8** — scaling/sharding in the cell topology.
- **§9** — failure modes + the drills owed (quantified).
- **§10** — cited prior art.
- **§11** — required changes to foundational systems.
- **§12** — open questions for Phase 4.

**Floors named up front (VISION §3 / EI-04 §4):**
1. **SKELETON → mock → real** is the build order (AG-3): SKELETON and Mock are **built**; `LlmAgentRuntime`
   is **designed-not-built** — its trait seam is fixed here, its implementation is a P6/post-P4 follow-on.
2. The **sandbox is consumed, not owned** (ADR-20): the Agent Fabric feeds runs through the *one unified
   runner* (job-spec `kind=agent`); the runner + the **sandbox-escape drill on a real kernel** are CI's
   Phase-4 deliverable. The hands trait's *channel-proof marker* and the no-host-exec lint are built here;
   the real gVisor/microVM execution is the CI runner's. **This is the single hard gate before any agent
   tool runs untrusted code** (ADR-20; EI-03 §3.5).
3. **Explicit-first dispatch** (CHAT-1, EI-03 §7): runtime dispatch is **explicit "run an agent here"**;
   implicit auto-wake on casual mention is a separately-decided product feature, **not** built here.
4. Agent **long-term memory / embeddings** (RAG over prior runs) is a **named holder seam** (AG-7,
   ADR-12.1) but the embedding store + its erasure are a P4 (Search/Knowledge) follow-on; v1 agents are
   **stateless across runs except for the content-addressed trace document**.

---

## 1. Purpose, responsibilities, and the trait set

### 1.1 What `myelin-agent` owns

The **strategy-pattern boundary** (VISION §3 non-negotiable; ADR-08) behind which a `MockAgentRuntime`
lives today and an `LlmAgentRuntime` lives later — *"replacing one trait implementation lights up the
entire agent-first story"* (EI-03 §1). The thesis to internalise (EI-03 preamble): **if the substrate is
right, an agent needs almost no special code** — it is a `Principal` with `kind=agent` running through the
*same* identity, gateway, event log, and sandbox as everyone else. Concretely, `myelin-agent` owns:

1. The **small trait set** (§1.3): `AgentRuntime` (the brain), `Agent` (the platform loop), `ToolSurface`
   (the registry), `EventInbox` (delivery), `EffectApi` (plan-then-apply), `ToolHands` (the sandbox seam).
2. The **brain+hands strategy boundary** (D6/AG-1/AG-2; §2): brain = stateless `step(conversation) →
   {use_tools | submit}` with the **platform loop owning history**; hands = `exec(command) → result` with
   **no host path bypassing the trait**.
3. The three **runtimes** (§3): SKELETON, Mock (deterministic + shipped `--use-mock`), Llm (seam only).
4. The **plan-then-apply `EffectApi`** (§5.2): validate every proposed effect against
   permissions ∩ delegation ∩ tenant (Id), budget, HITL gates → apply → emit (ADR-08.3).
5. The **permissioned tool registry** (`ToolDef`; §6) and the **MCP-exposure path** (one catalogue, two
   front-ends; ADR-08.4).
6. The **safety machinery** (§5.5): least-privilege per-run identity (via Id), the **reserve/settle cost
   gate** in front of every run (D8), the **structural loop guards** (AG-6), HITL gates, attribution +
   audit, agents-labelled.

### 1.2 What `myelin-agent` does NOT own (consumes via contracts)

- **Permission decisions** — Id owns `check`/`list_objects`/`delegation`; the Fabric *asks* (ADR-03/13.3).
- **Per-run identity minting/revocation** — Id's `mint_run_token`/`revoke` (ID-2); the Fabric requests it.
- **The event log + inbox delivery + the reactive/dispatch tier** — the Bus owns it (ADR-04/19); the Fabric
  is the *target handler* the bus delivers to, not a second event system (ADR-08.5). The loop guards live
  in the dispatch tier (Bus §4.7); the Fabric *enforces* depth/budget at apply time too (defence in depth).
- **The sandbox runner** — CI's unified runner (ADR-20); the Fabric supplies a `kind=agent` job spec.
- **Durable waits / timers / long HITL pauses** — the durable-workflow engine (ADR-09); a run *is* a
  workflow whose model step and tool calls are activities (§5.6).
- **Audit storage** — GDPR/Audit owns the tamper-evident log (ADR-12.9); the Fabric emits to it via the
  outbox. **The execution trace is a separate Knowledge document** (AG-7).
- **The LLM vendor** — named *only* inside `LlmAgentRuntime` (ADR-08.2); zero model/SDK/prompt strings
  anywhere else in platform code, enforced by lint (§11, sibling to `no-host-exec`).

### 1.3 The trait set at a glance (the §7 contract; concrete shapes here)

```rust
// THE BRAIN — stateless, one method (AG-1 / D6). The ONLY place a model vendor appears (in the Llm impl).
pub trait AgentRuntime {
    fn step(&self, conv: &Conversation) -> Result<StepOutcome>;   // pure-ish: conv in, decision out
}
pub enum StepOutcome {
    UseTools(Vec<ToolCall>),     // "call these tools, give me the results, step me again"
    Submit(Submission),          // "I'm done; here is my final answer / proposed effects"
}

// THE PLATFORM LOOP — owns Conversation history, drives the brain, runs plan-then-apply (§5.1). NOT a strategy.
pub trait Agent {
    fn handle(&self, inbox: InboxEvent, runtime: &dyn AgentRuntime) -> Result<RunOutcome>;  // AG-3 (§2.3)
}

// THE HANDS — one method, no host-exec bypass (AG-2 / D6). Real impl runs in the sandbox; sim emits a marker.
pub trait ToolHands {
    fn exec(&self, cmd: Command) -> Result<ToolResult>;           // §2.2; lint: no path bypasses this
}

// THE TOOL REGISTRY — one permissioned catalogue, exposable over MCP later (ADR-08.4; §6).
pub trait ToolSurface {
    fn register_tool(&mut self, def: ToolDef);                    // every subsystem contributes its actions
    fn resolve(&self, name: &ToolName) -> Option<&ToolDef>;
}

// DELIVERY — the platform delivers matched events; agents don't poll (ADR-08; bus §4.7 dispatch tier).
pub trait EventInbox {
    fn deliver(&self, ev: InboxEvent);                            // carries envelope + binding + token + budget
}

// PLAN-THEN-APPLY — the platform-owned write-back path (ADR-08.3; §5.2). Agents NEVER mutate directly.
pub trait EffectApi {
    fn apply(&self, run: &RunCtx, effect: ProposedEffect) -> EffectResult;  // Applied | Gated | Denied
}
```

The **only** strategy-swappable members are `AgentRuntime` (brain) and `ToolHands` (hands). `Agent`,
`ToolSurface`, `EventInbox`, `EffectApi` are **platform-owned and identical for mock and real** — that is
the whole point of plan-then-apply (ADR-08 §Rationale; EI-03 §1): the thing under test is the wiring and
the sandbox, **not** model spend.

---

## 2. The two strategy boundaries — brain & hands (D6 / AG-1 / AG-2), and AG-3 resolved

EI-03 §1: *"Make the swap from mock to real a single trait implementation, by keeping the abstraction
surface minimal."* Two boundaries, each **one method**.

### 2.1 The brain — `step(conversation) -> {use_tools | submit}` (AG-1, DECIDED)

The provider trait is **stateless**: it takes the whole `Conversation` and returns a single decision. **The
platform-side agent loop owns the conversation history** (AG-1; EI-03 §1.2). This is the default-to-beat
AG-3 was waiting for, and it survives plan-then-apply: the brain *proposes* (tool calls or a submission); it
never *acts*.

**Why stateless `step`, not a stateful streaming session:**
- **Determinism & testability** (ADR-08 §Rationale). A stateless function of the conversation is trivially
  mockable (replay a scripted queue of `StepOutcome`s — §3.2) and trivially golden-testable + mutation-
  testable (`cargo-mutants` over event→trigger→effect→event, AG-4). A stateful streaming session hides the
  state the test must control.
- **The platform owns history → the platform owns truth.** Conversation history (the system prompt, prior
  tool results, the running transcript) is *platform data* (a `PersonalDataHolder`, residency-pinned, the
  trace document AG-7). If the provider owned it, history would leak into the vendor boundary and erasure
  would have to reach into the vendor — exactly the seam we keep on our side.
- **It matches the proven shape.** This is the OpenAI/Anthropic *tool-use loop* (ReAct: Yao et al.,
  *ReAct: Synergizing Reasoning and Acting in Language Models*, ICLR 2023) reduced to its essential
  step: the model emits either tool calls or a final answer; the harness executes tools and re-prompts.
  We name only the *shape*, not a vendor (ADR-08.2).

```rust
pub struct Conversation {
    pub system: SystemContext,        // task framing, the agent's role, the labelled-as-agent notice
    pub turns:  Vec<Turn>,            // prior model steps + tool results (platform-owned, the trace)
    pub tools:  Vec<ToolSchema>,      // the tools THIS run may call (already permission/delegation-scoped, §5.2)
    pub budget: BudgetView,           // remaining reserve (so the brain can choose to Submit early)
}
pub enum Turn { Model(StepOutcome), ToolResults(Vec<ToolResult>), Approval(ApprovalNote) }
```

### 2.2 The hands — `exec(command) -> result`, no host-exec bypass (AG-2, DECIDED)

The tool-execution trait is one method, and **there is no host-execution path that bypasses it** (AG-2;
EI-03 §1.5). Two implementations, both behind the same trait:

- **`SandboxedHands` (real)** — runs the command inside the **one unified sandbox** (ADR-20): it builds a
  job spec with `kind=agent`, the hardening profile (egress default-deny, read-only root + tmpfs, caps
  dropped, no-new-privileges, seccomp, digest-pinned image, `pids.max`, zero swap, whole-guest kill on
  teardown), submits it to CI's runner, and returns the result. **Secrets are resolved *inside* the
  boundary by name** and are **never handed to the agent runtime to forward** (ADR-20; CI-1; EI-03 §3).
- **`SimHands` (simulation)** — runs the command **in-process with an in-memory scratch space and emits a
  channel-proof marker** proving it went through the trait, not a host shell (EI-03 §1.5). The marker is
  asserted by a test so a regression that shells out directly fails loudly.

**The `no-host-exec` lint** (substrate §2.11, AG-2/E-5) is elevated here to an architecture-test obligation
sibling to ADR-01's `no-cross-db` lint: any `std::process`/`Command`/FFI exec outside `ToolHands::exec`
fails the build. This is the mechanical embodiment of "one escape is catastrophic" (EI-04 §5.1).

> **Boundary note.** *Side-effecting* tools (mutate platform state — open a PR, transition an issue) do
> **not** go through `ToolHands::exec` at all — they go through **`EffectApi`** (plan-then-apply, §5.2),
> which calls the subsystem's public endpoint as a human would (EI-03 §4: same gateway, no carve-out).
> `ToolHands::exec` is for **untrusted code execution** (run a test, a linter, a build, a script) — the CI-
> shaped work. The two are deliberately distinct: `EffectApi` is *governed mutation*, `ToolHands` is
> *sandboxed computation*. Both are "the hands"; only the second runs arbitrary code, so only it needs the
> kernel sandbox. (This resolves the latent ambiguity in "exec(command)" — see §5.0.)

### 2.3 `Agent::handle` — the shape AG-3 resolves (DECIDED)

ADR-08 §Deferred and AG-3 left open: *single-call vs driven multi-turn loop, streaming, context
management.* **Decision: `Agent::handle` is a platform-owned, bounded, driven multi-turn loop** — not a
single call, and not the provider's responsibility.

```rust
fn handle(&self, inbox: InboxEvent, runtime: &dyn AgentRuntime) -> Result<RunOutcome> {
    let mut conv = self.build_conversation(&inbox)?;        // platform builds history from the trace (AG-1)
    let run = self.reserve.open(&inbox)?;                   // reserve/settle gate — no balance → no run (D8)
    let mut steps = 0;
    loop {
        if steps >= run.max_steps { return Ok(self.settle(run, Halted::StepCap)); }   // bounded loop (§5.5)
        if run.budget.exhausted() { return Ok(self.settle(run, Halted::Budget)); }
        match runtime.step(&conv)? {                        // THE BRAIN — stateless (§2.1)
            StepOutcome::UseTools(calls) => {
                let results = self.run_tools(&run, calls)?; // §5.0 routes: EffectApi vs ToolHands
                conv.turns.push(Turn::ToolResults(results));// platform appends to history
                steps += 1;
            }
            StepOutcome::Submit(sub) => {
                let applied = self.apply_submission(&run, sub)?;  // plan-then-apply each proposed effect
                return Ok(self.settle(run, Done(applied)));
            }
        }
    }
}
```

Properties this shape pins (AG-3 answers):
- **Multi-turn, platform-driven.** The loop lives on our side; the brain is re-entered with grown history.
- **Streaming is a transport detail inside the runtime**, not in the trait. `step` returns a *complete*
  `StepOutcome`; a real runtime may stream tokens internally for UX, but the platform sees a decision. This
  keeps the trait stable across mock (no streaming) and real (streams internally).
- **Context management is the platform's job** (`build_conversation`, trace-backed) — so it is auditable,
  erasable, and residency-pinned, and the provider stays stateless.
- **The loop is bounded** by `max_steps`, the reserve/settle budget, and the causal-depth ceiling (§5.5) —
  three independent ceilings, none sufficient alone (EI-03 §5).
- **Plan-then-apply survives** (the AG-1 constraint): the brain only ever *proposes*; `apply_submission` /
  `run_tools` route proposals through `EffectApi`. Identical platform code for mock and real (ADR-08.3).

---

## 3. The three runtimes (AG-3 build order: SKELETON → mock → real)

### 3.1 SKELETON mode (the first runtime — AG-3, DECIDED, BUILT)

**No model, no tools.** Authenticate → fetch the task (the `InboxEvent`) → print a summary → exit (EI-03
§1.6; AG-3). It is a `dyn AgentRuntime` whose `step` immediately returns `Submit(Submission::summary())`.

Its job is to **prove the whole gateway/identity/dispatch path with zero model spend and zero effects**: the
per-run token is minted (Id §4), the run is reserved (and settled at ~zero, D8), the `InboxEvent` is
delivered, causality is threaded, the trace document is written, the run is audited and torn down (token
revoked, ID-2). It is the *first* runtime stood up (skeleton → mock → real) and the cheapest end-to-end
integration test of the substrate. **The escape drill (ADR-20) is NOT gated by skeleton** because skeleton
runs no untrusted code; the gate bites the moment `ToolHands::exec` runs real code (§9, D-4).

### 3.2 MockAgentRuntime (deterministic + shipped `--use-mock`; AG-4, DECIDED, BUILT)

A `dyn AgentRuntime` that **replays a scripted queue of `StepOutcome`s** — deterministic, zero cost (EI-03
§1.3). It is used by **both** unit/golden tests **and** a developer/operator **`--use-mock` runtime flag on
the same code path users hit** (AG-4): `myelin agent run <id> --use-mock` swaps `runtime_ref = mock` and
runs the *identical* loop, `EffectApi`, sandbox seam, audit, and reserve/settle — only `step` is scripted.

```rust
pub struct MockAgentRuntime { script: VecDeque<StepOutcome>, /* keyed by step index or by a matcher on conv */ }
impl AgentRuntime for MockAgentRuntime {
    fn step(&self, conv: &Conversation) -> Result<StepOutcome> {
        Ok(self.next_for(conv))   // deterministic: same conv prefix → same decision (golden-testable)
    }
}
```

This is what makes the entire **event→trigger→effect→event loop integration-testable** with golden tests +
**`cargo-mutants`** (the `.gitignore`-signalled quality bar, VISION §4; AG-4). The mock is *not* a test-only
harness — shipping it as `--use-mock` is the dogfooding/demo lever (EI-03 §1.3) and the `MockAgentRuntime`
that ADR-08.2 names as shipping during development.

### 3.3 LlmAgentRuntime (the only vendor seam — DESIGNED-NOT-BUILT, FLOOR)

The **only** place a model vendor, SDK, prompt, or model name appears (ADR-08.2). It carries attribution
fields (tenant, actor, run id, caused-by) so every call is traceable and metered (EI-03 §1.2). It is the
adapter that EI-03 §1 promises *"lights up the entire agent-first story"* when swapped in. **Not built in
P3.** Its seam is fixed: it implements `AgentRuntime::step`, it is EU-hostable (a swappable, region-aware,
EU-preferring adapter — ADR-12.8; AG-9), and it meters **one cost event per model call, wholesale ≠ markup**
(D8; §5.4). The swap is `myelin agent runtime set <id> llm:<adapter>` (a config change, not a rewrite).
**Follow-on owner:** P6 roadmap (post the safety drills); the EU-sovereign sub-processor question is
`[OPEN → LEGAL]` (AG-9).

---

## 4. The data model / schemas

All tables: `(tenant, region)` first column, RLS-enforced, no cross-tenant query path (EI-02 §1; ID-3);
residency-pinned, per-tenant envelope-encrypted, crypto-shred-capable, `PersonalDataHolder` (ADR-11/12).

### 4.1 `run` — the unit of agent execution (a durable-workflow instance; ADR-09)

```sql
CREATE TABLE run (
  tenant            uuid NOT NULL,
  region            text NOT NULL,
  run_id            uuid NOT NULL,                 -- ties to actor.run in the envelope (event-bus §3.1)
  agent_principal   uuid NOT NULL,                 -- the kind=agent Principal (Id §3); minted per run
  on_behalf_of      uuid,                          -- the human whose session caused this run (caused-by)
  binding_id        uuid,                          -- the subscription/automation binding that dispatched (ADR-19)
  trigger_event     text,                          -- the event_id that woke this run (the cause)
  correlation_id    text NOT NULL,                 -- the causal ROOT (loop tripwire reads this)
  causation_id      text,                          -- the immediate parent event_id (nested, BUS-5)
  depth             int  NOT NULL,                 -- causal depth at dispatch (ceiling check, AG-6)
  runtime_ref       text NOT NULL,                 -- skeleton | mock | llm:<adapter> (the strategy swap)
  state             run_state NOT NULL,            -- reserved | running | gated | settled | halted | failed
  reservation_id    uuid NOT NULL,                 -- the cost reserve (D8); released/settled on completion
  budget            jsonb NOT NULL,                -- RunBudget: caps in integer minor-units (X-5) + step/token caps
  trace_ref         text,                          -- ArtifactRef of the trace document (AG-7), content-addressed
  opened_at         timestamptz NOT NULL,
  settled_at        timestamptz,
  PRIMARY KEY (tenant, run_id)
);
CREATE INDEX run_active ON run (tenant) WHERE state IN ('running','gated');
```

A `run` is a **durable-execution instance** (ADR-09): the workflow owns budget/gates/state; the model step
and tool calls are **activities** (non-deterministic, retryable, sandboxed). A run may pause for *days* on a
HITL gate without holding resources (§5.3; EI-03 §5.1). `run_id` and `agent_principal` are the same fields
the envelope's `actor.run` / `actor.principal` carry (event-bus §3.1) — attribution is by-construction.

### 4.2 `tool_def` — the permissioned tool registry (one catalogue; ADR-08.4)

```sql
CREATE TABLE tool_def (
  tenant            uuid,                           -- NULL = platform-global tool; non-null = tenant-scoped
  name              text NOT NULL,                  -- canonical dotted name, e.g. 'issue.transition'
  subsystem         text NOT NULL,                  -- the contributing subsystem (event-bus §6.2 token)
  version           int  NOT NULL,                  -- ToolDef is versioned (forward-only, STOR-2)
  input_schema      jsonb NOT NULL,                 -- JSON Schema for the tool's input (validated pre-apply)
  required_caps     jsonb NOT NULL,                 -- the Permission(s) the run must hold (Id check, §5.2)
  effect_kind       effect_kind NOT NULL,           -- read | compute | mutate | external (routes §5.0)
  side_effecting    boolean NOT NULL,               -- true ⇒ goes through EffectApi (plan-then-apply)
  requires_approval boolean NOT NULL DEFAULT false, -- HITL gate by default for consequential mutations (AG-8)
  exposed_over_mcp  boolean NOT NULL DEFAULT false, -- the MCP-exposure flag (§6.2)
  PRIMARY KEY (subsystem, name, version)
);
```

This is the `ToolDef` ADR-08.4 names: **name + JSON-schema input + required caps + effect kind +
side-effecting flag** (extended here with `requires_approval` and `exposed_over_mcp`). Subsystems
`register_tool` at build time; the same registry is consumed internally **and exposable over MCP** (§6).

### 4.3 `proposed_effect` — the plan-then-apply audit row

```sql
CREATE TABLE proposed_effect (
  tenant            uuid NOT NULL,
  region            text NOT NULL,
  effect_id         uuid NOT NULL,
  run_id            uuid NOT NULL,
  step              int  NOT NULL,                  -- which loop step proposed it
  tool_name         text NOT NULL,
  tool_version      int  NOT NULL,
  input             jsonb NOT NULL,                 -- the validated input (schema-checked)
  outcome           effect_outcome NOT NULL,        -- applied | gated | denied
  denial_reason     text,                           -- Denied returns an ordinary tool error (AG-5)
  gate_id           uuid,                           -- the HITL gate if Gated (§5.3)
  cost_minor_units  bigint,                         -- metered cost (D8); integer, never float (X-5)
  emitted_event     text,                           -- the event_id of the resulting domain event (if applied)
  proposed_at       timestamptz NOT NULL,
  PRIMARY KEY (tenant, effect_id)
);
```

Every proposed effect is recorded **whether applied, gated, or denied** — this is the audit + replay
substrate (E-6 investigate-before-build) and the plan-then-apply payoff (`myelin agent run --dry-run` shows
proposed effects without applying — overview §6.5).

### 4.4 `hitl_gate` — the approval state (a durable-workflow signal; ADR-09 / AG-8)

```sql
CREATE TABLE hitl_gate (
  tenant            uuid NOT NULL,
  gate_id           uuid NOT NULL,
  run_id            uuid NOT NULL,
  effect_id         uuid NOT NULL,                  -- the withheld effect
  tool_name         text NOT NULL,
  risk_summary      text NOT NULL,                  -- humanised (NOTIF-1): what + risk + live cost estimate
  cost_estimate     bigint NOT NULL,               -- minor-units; shown on the approval card (AG-8)
  approver_filter   jsonb NOT NULL,                 -- who may approve (a list_subjects-derived set)
  state             gate_state NOT NULL,            -- pending | approved | rejected | expired
  decided_by        uuid,
  decided_at        timestamptz,
  card_ref          text,                           -- ArtifactRef of the chat approval card (Chat is the surface)
  PRIMARY KEY (tenant, gate_id)
);
```

The gate is a **durable-workflow wait surfaced as a chat approval card** (ADR-08.6/09; the Chat subsystem is
the HITL surface). The approve→resume bridge is wired end-to-end (§5.3; AG-8 names "don't ship the withhold
logic and the card but forget the bridge").

### 4.5 The execution trace — a content-addressed Knowledge document (AG-7, DECIDED)

The run's execution **trace is just a document** in the Knowledge subsystem (content-addressed, immutable),
reusing `myelin-content` — *"reusing it saves an entire schema and projection"* (EI-03 §4.4; AG-7). It is
**distinct from the tamper-evident audit log** (the audit log is GDPR/Audit's complete, retention-bounded,
tamper-evident holder; the trace is the human-readable narrative). The trace is a `PersonalDataHolder`
(AG-7): it holds the conversation (system context, tool inputs/results, the model's reasoning if surfaced),
some of which is personal data → residency-pinned, crypto-shred-capable, erasable. `run.trace_ref` is its
`ArtifactRef`. **Required change to foundational systems:** the Knowledge subsystem must accept a
content-addressed write of an agent trace (§11).

---

## 5. The algorithms

### 5.0 Routing a tool call (the dispatch the loop performs)

When the brain returns `UseTools(calls)`, the platform loop routes **per `effect_kind` / `side_effecting`**:

| `effect_kind` | `side_effecting` | Route | Why |
|---|---|---|---|
| `read` | false | direct (subsystem read API, permission-checked) | a permission-filtered read; no mutation, no sandbox |
| `compute` | false | **`ToolHands::exec`** (the sandbox; ADR-20) | runs untrusted code (test/build/lint/script) |
| `mutate` | true | **`EffectApi::apply`** (plan-then-apply; §5.2) | a governed platform mutation via the public endpoint |
| `external` | true | **`EffectApi::apply`** → an egress-reviewed adapter | a side-effecting external call (webhook, etc.) |

This is the concrete answer to the §2.2 ambiguity: **`exec` is for sandboxed computation; `EffectApi` is for
governed mutation.** Both are validated; only `compute`/`external` untrusted code touches the kernel sandbox.

### 5.1 The agent loop (the platform-owned driver — §2.3, expanded)

The loop (`Agent::handle`) is the heart. It: builds the conversation from the trace (AG-1), opens the
reserve (D8), then repeatedly steps the brain, routes tool calls (§5.0), appends results to history, and
settles. **It is identical for mock and real** (ADR-08.3). It carries causality **nested** (BUS-5): every
event it emits derives `causation_id = cause.event_id`, `correlation_id` carried, `depth = +1` — via
`OutboxTx::emit(draft, cause)` (substrate §2.1), so a loop guard reads platform metadata, not a convention
(EI-02 §6; the agent **cannot typo into a loop**).

### 5.2 Plan-then-apply `EffectApi` (ADR-08.3 — the core safety+testability choice, DECIDED)

Agents are a **pure-ish function `(event, context) → AgentDecision { effects }`**; they **never perform side
effects directly** (ADR-08.3). They emit **proposed effects**; the platform's `EffectApi` validates each and
applies it. The validation pipeline, **in order, fail-closed**:

```
EffectApi::apply(run, effect):
  1. SCHEMA      — validate `effect.input` against the ToolDef's JSON Schema; reject malformed (Denied).
  2. CAPABILITY  — the run's per-run identity must hold `tool_def.required_caps`:
                   Id.check(run.agent_principal, required_cap, effect.object, zookie)        ─┐
  3. DELEGATION  — Id.delegation(agent, trigger_actor) → the composed                          ├─ ALL must hold
                   `agent.policy ∩ delegation ∩ tenant.policy` (Id §7); attenuation, never up. │  (intersection)
  4. TENANT      — tenant guardrails (agent-allow-list, residency, AI-Act constraints) — in #3 │
                   as the tenant.policy term.                                                  ─┘
  5. BUDGET      — the reserve has remaining balance for this effect's metered cost (D8, §5.4).
  6. HITL GATE   — if `tool_def.requires_approval` AND not yet approved for this run → WITHHELD:
                   open a hitl_gate (§5.3), return Gated. (The tool returns an error; does NOT mutate. AG-8.)
  7. APPLY       — call the subsystem's PUBLIC endpoint as the agent principal (same gateway, no carve-out,
                   EI-03 §4) → on success, the subsystem emits its domain event via ITS outbox (which may
                   wake more agents, governed by loop caps). Record proposed_effect(outcome=applied).
  8. METER       — settle one cost event for this effect (D8); wholesale ≠ markup kept separate.
  → EffectResult ∈ { Applied(event_id) | Gated(gate_id) | Denied(reason) }
```

**Key properties:**
- **A denied effect (403/503) returns an ordinary `Denied` tool error to the loop** — there is **no
  privileged fallback path** (AG-5; EI-03 §4.2). The brain sees a tool error and re-plans, exactly as it
  would for any tool failure. An agent **can do nothing its identity is not permitted to do** (EI-03 §4).
- **Same gateway, no carve-out** (EI-03 §4): apply calls the *same public endpoint a human uses*, carrying
  the run's scoped token, so the existing Id `check` runs unchanged. There is no agent-only back door into
  a subsystem's database (ADR-13: no subsystem reads another's store; agents mutate *as tools*).
- **Intersection, not union** (Id §7): `agent.policy ∩ delegation ∩ tenant.policy` makes *"an agent can do
  things no human role can"* (the EI-02 §2 named failure) structurally impossible.
- **Identical mock/real** (ADR-08.3): the mock proposes effects the same way; `EffectApi` validates and
  applies them the same way. `--dry-run` stops after step 6 and shows the plan (overview §6.5).

### 5.3 HITL: withhold → approve → resume (end-to-end; AG-8, DECIDED)

EI-03 §5.1 / AG-8: *"Wire the approve→resume loop end to end — it's easy to ship the withhold logic and the
card but forget the bridge between them."* The full loop:

1. **Withhold.** A gated write tool whose effect is not yet approved is **withheld** at `EffectApi` step 6:
   it **returns a `Gated` error and does NOT mutate** (EI-03 §5.1). A `hitl_gate` row opens.
2. **Surface.** The gate becomes a **durable-workflow wait** (ADR-09) surfaced as a **chat approval card**
   (Chat is the HITL surface). The card shows the **pending action, its risk, and a live cost estimate**
   (AG-8), humanised at the backend (NOTIF-1: a routable `ArtifactRef` + humanised string, not raw ids).
   Approver set is `list_subjects(object, approve_perm)` (Id §8.3) — only an authorised human may approve.
3. **Decide.** A human approves/rejects (minutes or days later — the durable wait holds, EI-03 §5.1). A
   consequential approval may require **step-up MFA / break-glass** (Id §11) — that is the tenant's policy.
4. **Resume.** Approval **re-runs the step with the tool name added to "approved"** (EI-03 §5.1): the
   workflow signal resumes the run, `EffectApi::apply` re-evaluates, step 6 now passes, the effect applies.
   The bridge is the durable-workflow **signal** (ADR-09) → the loop resumes from the gated step. Rejection
   settles the run as `Halted::Rejected` with the rejection in the trace + audit.

**Suggest-by-default; human-confirm consequential actions** (ADR-08.6; GDPR Art. 22 / EU AI Act human-
oversight, L-3/L-4). Which tools are `requires_approval` by default is a per-subsystem call (ISS/GIT P4),
defaulting to *gated* for irreversible/consequential mutations.

### 5.4 Reserve/settle: the universal cost gate (D8 / CI-2, DECIDED)

A **universal reserve/settle gate in front of EVERY run** (D8; EI-03 §5.2; generalises ADR-08.6 and CI
metering CI-2): **reserve at dispatch, settle on completion, refuse to start when balance is exhausted,
never interrupt one in flight.**

- **Reserve at dispatch** (`run.open`): check a **prepaid balance + any per-capability add-on**; **refuse to
  *start* a new run when exhausted** (EI-03 §5.2). A runaway loop **spends down a wallet and stops — not a
  surprise infrastructure bill** (EI-03 §5.2). This is the economic backstop to the structural loop guards.
- **Meter one cost event per model call** (and per metered effect), **keeping wholesale and markup separate**
  so a pricing change never rewrites history (EI-03 §5.2; C-1: pricing history is immutable). Costs are
  **integer minor-units, never floats** (X-5, substrate §2.10).
- **Settle on completion**, releasing the unused reserve. Never interrupt a run in flight (EI-03 §5.2): the
  cap stops the *next* run, not the current one — so partial work is never corrupted.
- **Uniform across CI** (CI-2/D8): the same gate fronts CI runs ("no balance → no execution" is uniformly
  true). The wallet/pricing model is **Commercial's** (C-1); the *gate* is this substrate.

### 5.5 The structural loop guards (AG-6, DECIDED — defence in depth with the dispatch tier)

EI-03 §5.3–5.4 / AG-6: loop prevention is **structural, not a convention**. The guards live primarily in the
Bus reactive/dispatch tier (event-bus §4.7), and the Fabric **re-enforces them at apply time** (defence in
depth, since an agent can both *be woken by* and *emit* events):

| Guard | Mechanism | Where read |
|---|---|---|
| **Self-guard** | drop an inbound event whose `actor.principal` == this agent (skip the agent's own output) | dispatch tier; Fabric on inbox |
| **Reference gate** | only a **structured `artifact_ref` node** can re-trigger — never raw typed text (wired to ADR-05: only `artifact_ref` nodes emit `ref.created`) | dispatch tier (AG-6) |
| **Causal-depth ceiling** | drop/park dispatch when `depth > ceiling` (default 12); the Fabric also refuses to *emit* past the ceiling | both (envelope `depth`, §4.1) |
| **Shared-root tripwire** | if > K events share one `correlation_id` within a window, trip a **per-tenant circuit breaker** (EI-02 §6) | dispatch tier; per-tenant breaker |
| **Idempotent tools** | tools are idempotent on `(run, effect_id)` so a redelivery is a no-op (ADR-08.6) | EffectApi (dedup) |
| **Bounded dispatch pool** | a bounded worker pool **drops over-cap dispatches** (never forks unboundedly) — bounds a mention/event storm (EI-03 §5.4) | dispatch tier (AG-6) |

Because the guards read **platform causality metadata** (the envelope's `correlation_id`/`causation_id`/
`depth`, derived correct-by-construction), *"a human (or agent) can never typo their way into a loop"*
(EI-02 §6). The **causal-loop tripwire drill** (§9, D-7; AG-4 adversarial) proves it.

### 5.6 A run is a durable workflow (ADR-09 mapping)

The mapping (EI-03 §5.1; ADR-09): the **workflow is durable and owns budget/gates/state**; the **model step
(`step`) and tool calls (`exec`/`apply`) are activities** (non-deterministic, retryable, sandboxed). A HITL
gate is a **signal**; `stale_after`/timeout is a **durable timer**. This is why a run can pause for days on
an approval without holding a thread (EI-03 §5.1). Build-vs-adopt of the durable-execution substrate is the
Bus/workflow P3 item (TE-20); the Fabric *consumes* it — it does not reinvent durable waits.

### 5.7 Per-run identity: mint, scrub, revoke (ID-2, DECIDED)

At dispatch, the Fabric requests Id `mint_run_token(agent_id, run_id, delegation_caveats, ttl)` (Id §12):
**token life == run life** (ID-2). It **unsets any shared platform token in the child environment** so it
can't leak in as the tool identity (EI-03 §4; ID-2 anti-leak). On teardown it calls Id `revoke(token_jti)`
**idempotently even on crash** (an idempotent cleanup hook, EI-03 §4; ID-2) — belt-and-suspenders with Id's
**auto-expiring tuples** (`expires_at` == run life, Id §6) so even a failed revoke self-destructs inside the
staleness window. *"An agent literally cannot exceed its identity"* (EI-02 §2).

---

## 6. The permissioned tool registry + the MCP-exposure path (ADR-08.4)

### 6.1 One catalogue, two front-ends (DECIDED)

Every subsystem registers typed `ToolDef`s into **one shared `ToolSurface`** (§4.2): **name + JSON-schema
input + required caps + effect-kind + side-effecting flag** (ADR-08.4), plus `requires_approval` and
`exposed_over_mcp`. The same registry is:
- consumed **internally** by our runtimes (the `Conversation.tools` the brain sees are exactly the run's
  permitted, delegation-scoped subset — `list_objects`/`check`-filtered at conversation-build time), and
- **exposable over MCP** to external agents later — *defined once, governed once* (ADR-08.4).

The conversation's tool list is **pre-scoped** (§2.1): a run only ever *sees* tools its
`agent.policy ∩ delegation ∩ tenant.policy` permits, so the brain can't even propose a tool it could never
call. `EffectApi` re-checks at apply time (defence in depth; the scoping is an optimisation, the check is the
guarantee — fail-closed).

### 6.2 The MCP-exposure path (DECIDED — seam built, external exposure is FLOOR)

**MCP (Model Context Protocol, Anthropic, 2024)** is the emerging open standard for exposing tools to
external model-driven agents. We expose the **`exposed_over_mcp = true`** subset of the registry as an MCP
**server** behind the same gateway and the same Id `check` (EI-03 §4: no carve-out — an external MCP client
authenticates as a `Principal`, gets a per-run token, and its tool calls flow through `EffectApi` exactly
like an internal agent's). Properties:
- **The MCP surface is a projection of `ToolDef`** — `input_schema` → MCP tool schema, `required_caps` →
  enforced by Id, `side_effecting`/`requires_approval` → the same plan-then-apply + HITL path. No second
  governance model (ADR-08.4).
- **Per-run identity + reserve/settle + audit apply identically** — an external agent is a `Principal` with a
  per-run token, metered and audited like any other run (EI-02 §2).
- **FLOOR:** the v1 build registers the catalogue and the internal consumption path; the **external MCP
  server endpoint** (auth, rate-limit lane = agent lane, the MCP wire) is a named follow-on (P4/P6),
  because exposing tools to *external* agents is a product/security decision with its own threat model and a
  Legal/DPO sign-off (AI-Act, sub-processor). The *seam* (the `exposed_over_mcp` flag + the projection) is
  fixed here.

---

## 7. Contracts / APIs exposed and consumed (the glue — STABLE)

### 7.1 Exposed to subsystems (what they link against)

| Contract | Signature (illustrative) | Consumed by | Semantics |
|---|---|---|---|
| **register_tool** | `register_tool(ToolDef{name, input_schema, required_caps, effect_kind, side_effecting, requires_approval, exposed_over_mcp})` | every subsystem | contribute actions to the one catalogue (ADR-08.4; §6). |
| **EffectApi.apply** | `apply(run, ProposedEffect) → Applied(event_id) \| Gated(gate_id) \| Denied(reason)` | the loop; external MCP path | plan-then-apply (§5.2); the platform-owned write-back. Subsystems expose mutations *as tools*, not back-doors. |
| **AgentRuntime.step** | `step(&Conversation) → UseTools(Vec<ToolCall>) \| Submit(Submission)` | runtimes (mock/llm) | the stateless brain (AG-1); the strategy seam. |
| **ToolHands.exec** | `exec(Command) → ToolResult` | the loop (compute/external) | sandboxed computation; **no host bypass** (AG-2). |
| **Agent.handle** | `handle(InboxEvent, &dyn AgentRuntime) → RunOutcome` | the dispatch tier (bus) | the bounded multi-turn loop (AG-3; §2.3). |
| **run --dry-run** | `dry_run(InboxEvent) → Vec<ProposedEffect>` (no apply) | CLI / tests | plan-then-apply testability (overview §6.5). |

### 7.2 Consumed from other shared systems (the dependencies)

| From | Contract used | For |
|---|---|---|
| **Id** | `mint_run_token` / `revoke` (ID-2); `check` / `list_objects` / `delegation` (the `∩` algebra, Id §7) | per-run identity; effect validation (§5.2) |
| **Event Bus** | `EventInbox` delivery via the reactive/dispatch tier (§4.7); `OutboxTx::emit(draft, cause)` (causality, BUS-5); Signals (the curated trigger substrate, ADR-19) | wake-up; emitting domain events; the loop guards |
| **Durable-workflow engine (ADR-09)** | workflow open/signal/timer (a run *is* a workflow; §5.6) | HITL waits, budgets, long pauses |
| **CI unified runner (ADR-20)** | the `kind=agent` job spec + the hardening profile | `ToolHands::exec` real execution; **the escape drill is CI's gate** |
| **Knowledge (`myelin-content`)** | content-addressed write of the trace document (AG-7) | the execution trace (§4.5) — **required change, §11** |
| **GDPR/Audit** | `audit.record` (via outbox); `PersonalDataHolder` registration | tamper-evident audit of every agent action; erasure of run/trace/memory |
| **Commercial wallet (C-1)** | the prepaid balance the reserve/settle gate reads | the cost gate (§5.4) |

### 7.3 The resilient client + backpressure (consumed, not re-built)

All outbound inter-service calls (to Id, subsystems, the runner) go through the **shared resilient client**
(substrate §6): timeout / breaker / bulkhead / jittered-retry-idempotent-only, and **the agent runtime MUST
honour `Retry-After`** (ADR-16 §3; X-3) — *"ensure your own clients actually honour it, or shedding becomes
a retry storm"* (EI-02 §5). The agent lane is the **shed-before-human** lane (ADR-16 shed order: speculative
→ batch/CI → agent → human-last); a `429 + Retry-After` to an agent is an ordinary backoff, surfaced to the
loop as a transient tool error.

---

## 8. Scaling / sharding in the cell topology (ADR-11; X-4)

- **In-cell; agent processing stays in-region** — no cross-region agent runs on personal data (ADR-11).
- **The novel scale+safety concern is agent-generated load** (EI-02 §5): agents generate volume far beyond
  humans, and **agents-waking-agents can cascade**. Bounded by the **causal-depth ceiling**, the
  **reserve/settle budget**, **idempotent tools**, the **bounded dispatch pool that drops over-cap**, and
  **per-tenant circuit breakers** (§5.5; ADR-08.6). The **30× agent-surge drill** (D-6) asserts the human
  lane holds and other tenants are unaffected.
- **Runs are durable-workflow instances** (ADR-09): a long HITL pause holds no thread/connection (§5.6), so
  millions of paused runs cost storage, not compute — the same property SLA timers need (SC-11).
- **The runtime workers are stateless** (the brain is stateless; the loop driver is replaceable): a crashed
  worker's run resumes from the durable workflow + the trace. State lives in `run`/`hitl_gate`/the trace.

### 8.1 Stateful-component register + blast-radius note (X-4)

| Stateful component | Shard / state plan | Blast radius if it dies |
|---|---|---|
| `run` / `proposed_effect` / `hitl_gate` (PG, tenant-partitioned) | per-cell PG, `(tenant,region)`-keyed | that tenant's runs stall; durable workflow + trace recover them |
| The durable-workflow store (ADR-09, shared) | the workflow engine's store | paused runs wait; resume on recovery (no loss) |
| The tool registry (`tool_def`, PG) | per-cell, mostly read | dispatch degrades to last-known schema; rebuildable from build-time registration |
| The trace documents (Knowledge, content-addressed) | Knowledge's object store | traces unavailable; runs continue (trace is write-mostly) |
| The reserve/settle ledger (Commercial wallet) | per-tenant balance | reserve fails-closed → no new runs (correct: no balance → no execution) |
Everything else (runtime workers, the loop driver, the EffectApi evaluator, the MCP server) is **stateless
and horizontally replaceable** (EI-02 §10) — recoverable by resuming the durable workflow + reading the trace.

---

## 9. Failure modes + the drills owed (Phase 5 owns mechanics; we enumerate — PROVE-IT)

Per T-2/T-5 and the honesty rule: each property that can fail names the **quantified drill** that proves it
(a green artifact ⇒ "proven"; until then "claimed", T-4). The Fabric owes these:

| # | Property / failure mode | Drill (quantified gate) | Reads (telemetry §10/X-1) |
|---|---|---|---|
| D-1 | **Plan-then-apply: agent cannot mutate directly** | Adversarial: a tool that tries to write outside `EffectApi`; assert it is structurally impossible (no host/DB path; `no-host-exec` + `no-cross-db` lints green). Gate: **zero direct mutation; lints enforce.** | lint CI artifact |
| D-2 | **Denied effect → ordinary tool error, no escalation** (AG-5) | Propose an effect outside `agent.policy ∩ delegation ∩ tenant.policy`; assert `Denied` returns to the loop, **no privileged fallback** fires. Gate: **0 escalations; Denied surfaced.** | denial counter |
| D-3 | **Delegation intersection (least-privilege)** (Id §7; AG-2) | Adversarial: agent attempts an effect its own policy allows but delegation/tenant forbids (and vice-versa), incl. via a delegator who lost the right. Gate: **agent confined to the intersection; 0 over-privilege.** | delegation-deny counter |
| D-4 | **Sandbox escape on a real kernel** (ADR-20; EI-03 §3.5) | The **single hard gate before any agent runs untrusted code**: a `compute` tool attempts a kernel escape on a real host. Gate: **zero escapes.** *(Owned by CI's runner; the Fabric feeds the `kind=agent` job spec.)* | runner escape-drill artifact |
| D-5 | **HITL withhold→approve→resume** (AG-8) | A gated tool: assert it is **withheld** (returns error, does NOT mutate), the card shows action+risk+cost, approval **resumes** and applies, rejection halts. Gate: **0 mutations before approval; resume applies exactly once.** | gate-state, dedup |
| D-6 | **30× agent surge / fairness** (ADR-16; D8) | 30× agent dispatch surge on one tenant; assert the **human lane holds**, the **agent lane sheds** (429+Retry-After honoured), **other tenants unaffected**, and **reserve/settle refuses over-budget runs**. Gate: **human-lane latency within budget; cross-tenant unaffected.** | per-tenant in-flight, shed counters, reserve rejections |
| D-7 | **Causal-loop tripwire** (AG-6; AG-4 adversarial) | Adversarially construct an agent→agent self-trigger; assert the **depth ceiling + shared-root tripwire + bounded pool** halt it before runaway, and the per-tenant breaker trips. Gate: **loop halts ≤ ceiling; breaker trips; pool drops over-cap (never forks).** | depth histogram, tripwire counter, pool drops |
| D-8 | **Per-run token outlives the run** (ID-2) | Kill a run mid-flight; assert the token is **revoked on teardown** AND **auto-expires** (Id `expires_at`) within run-life ≤ W; assert no shared token leaked into the child env. Gate: **token dead ≤ W; 0 leaked tokens.** | revocation-lag |
| D-9 | **Determinism: mock loop is reproducible** (AG-4) | Run a scripted mock twice; assert identical proposed-effect sequences; run `cargo-mutants` over event→trigger→effect→event. Gate: **identical runs; mutation score ≥ threshold.** | mutation-test artifact |
| D-10 | **Erasure reaches the trace + memory** (AG-7; ADR-12) | Erase a subject; assert the run trace + any agent memory/embeddings are crypto-shredded/purged, run attribution falls back to the opaque pseudonym. Gate: **0 recoverable PII; attribution intact.** | holder erase receipts |
| D-11 | **Cost gate: runaway is self-limiting** (D8) | Drive a runaway loop against an exhausted wallet; assert the reserve **refuses to start** new runs (never interrupts one in flight) and the loop **stops at the wallet**, not at an infra bill. Gate: **0 runs started past exhaustion; in-flight runs complete.** | reserve rejections, cost-event count |

---

## 10. Cited prior art

- **Agent / tool-use loop.** Yao et al., *ReAct: Synergizing Reasoning and Acting in Language Models*
  (ICLR 2023) — the reason→act→observe loop our stateless `step` + platform-owned history reduces to.
  Schick et al., *Toolformer* (NeurIPS 2023) — tool-use as a model capability. The OpenAI/Anthropic
  **tool-use / function-calling** loop shape (model emits tool calls or a final answer; the harness executes
  and re-prompts) — we adopt the *shape*, name no vendor in platform code (ADR-08.2).
- **Model Context Protocol (MCP)** (Anthropic, 2024) — the open tool-exposure standard the `exposed_over_mcp`
  path projects onto (§6.2).
- **Capability security / least-privilege.** Saltzer & Schroeder, *The Protection of Information in Computer
  Systems* (1975) — least privilege + fail-safe defaults (the `∩`-intersection delegation + fail-closed
  `EffectApi`). Dennis & Van Horn, *Programming Semantics for Multiprogrammed Computations* (CACM 1966) —
  the capability model. **Macaroons** (Birgisson et al., NDSS 2014) and **biscuit** — attenuable,
  offline-narrowing capability tokens (the per-run delegation caveat chain, Id §7). Miller et al.,
  *Capability Myths Demolished* — why capabilities beat ACL-only for delegation.
- **Authorization at scale.** Zanzibar (Pang et al., USENIX ATC 2019) — the ReBAC `check`/`list_objects` the
  `EffectApi` calls (Id §8); the agent is a subject like any other (EI-02 §2).
- **Untrusted code isolation.** gVisor (Young et al., *The True Cost of Containing*, 2019) — userspace-kernel
  isolation; Firecracker (Agache et al., NSDI 2020) — microVM isolation. The ADR-20 sandbox floor; *"a
  property not drilled on a real kernel is a claim"* (EI-04 §5.1).
- **Durable execution.** Temporal / Cadence design (the workflow=durable, activities=retryable model) — a run
  is a workflow; the model step + tool calls are activities (ADR-09; §5.6).
- **Causality / loop safety.** Lamport, *Time, Clocks, and the Ordering of Events* (CACM 1978) — the
  happened-before basis for the nested `causation_id`/`depth` the loop guards read (EI-02 §6).
- **Overload / backpressure.** Nygard, *Release It!* (2nd ed.) — circuit breaker / bulkhead (the resilient
  client the runtime links). Google SRE ch. 21/22 — handling overload, cascading failures (the agent-surge
  drill). AWS Builders' Library — load shedding + `Retry-After` honouring.
- **Doctrine.** EI-03 §1 (brain+hands+skeleton), §2 (four primitives), §3 (one sandbox), §4 (same gateway,
  no carve-out), §5 (approval/cost/loops/storms), §6 (orchestrator gotchas), §7 (explicit-first dispatch);
  EI-02 §2 (one principal), §5 (backpressure), §6 (causality); EI-04 §5.1 (untrusted-exec is permanent).

---

## 11. Required changes to foundational systems (stated explicitly)

1. **Knowledge (`myelin-content`, P4) — accept a content-addressed agent-trace write (AG-7).** The execution
   trace is a Knowledge document (content-addressed, immutable, a `PersonalDataHolder`). Knowledge must
   expose a write path for an agent-authored trace and an `ArtifactRef` for it (`run.trace_ref`). *Not a new
   schema* — it reuses the block model — but Knowledge must accept the agent as an author and register the
   trace as an erasable holder. **(Owner: Knowledge P4; seam fixed here.)**
2. **CI runner (P4) — accept the `kind=agent` job spec and own the escape drill (ADR-20).** The Fabric's
   `SandboxedHands` submits a `kind=agent` job (the hardening profile, secrets-by-name-inside-the-boundary).
   CI owns the runner, the executor backend (gVisor/microVM), and the **sandbox-escape drill on a real
   kernel** — the single hard go/no-go before any agent runs untrusted code. **(Owner: CI P4.)**
3. **Durable-workflow engine (Bus/workflow P3) — expose open/signal/timer for runs (ADR-09).** A run is a
   workflow; HITL gates are signals; timeouts are durable timers. The Fabric consumes this; it needs the
   build-vs-adopt decision (TE-20) resolved with a self-hostable EU-deployable substrate. **(Owner: Bus/
   workflow P3.)**
4. **Commercial wallet (C-1) — the prepaid balance + per-capability add-on the reserve/settle gate reads.**
   The gate is this substrate; the wallet/pricing model (immutable pricing history, wholesale ≠ markup) is
   Commercial. **(Owner: Commercial.)**
5. **The `no-llm-in-platform` lint (new, sibling to `no-host-exec`).** No model/SDK/prompt/model-name string
   appears outside `LlmAgentRuntime` (ADR-08.2). Add to the substrate's architecture-lint set (§2.11). **(Owner:
   substrate/CI; rule fixed here.)**

No change is required to the **envelope** (the Fabric uses `actor.run`/`actor.principal`/causality as-is) or
to **Id's contracts** (`mint_run_token`/`revoke`/`delegation` are exactly the shapes Id §12 froze) — those
were designed with the Fabric as a consumer.

---

## 12. Open questions for Phase 4 (and the named within-Phase-3 floors)

**Named floors (within Phase 3; follow-ons owned elsewhere):**
- **`LlmAgentRuntime` is designed-not-built** (§3.3): the seam is fixed; the EU-sovereign adapter + its
  metering wholesale rates are a P6 follow-on; the EU-sovereign sub-processor is `[OPEN → LEGAL]` (AG-9).
- **External MCP server is a floor** (§6.2): the `exposed_over_mcp` seam + internal consumption are built;
  the external endpoint + its threat model + Legal sign-off are a P4/P6 follow-on.
- **Agent long-term memory / RAG over prior runs** (§0.4): a named holder seam; the embedding store + its
  erasure are a P4 (Search/Knowledge) follow-on. v1 agents are stateless across runs bar the trace document.

**Open questions for Phase 4:**
1. **Per-subsystem `requires_approval` defaults + role-bundle for "agent approver"** (AG-8; ISS/GIT P4): which
   mutations are HITL-gated by default, and the `list_subjects` approver set per tool, is a per-subsystem
   product call (defaulting to gated for consequential/irreversible effects).
2. **Explicit-vs-implicit dispatch policy** (CHAT-1; EI-03 §7): the explicit "run an agent here" action is
   v1; implicit auto-dispatch on casual mention is a separately-decided product feature with intent/cost
   detection — **and a DPO sign-off** (Art. 22 / AI-Act human-oversight, L-3). Owned by Chat P4 + Commercial.
3. **`Agent` vs `Service` in the dispatch path** (AG-1, resolved in Id as one kind / three faces): the loop
   guards (§5.5) must respect `actor.kind`; whether any agent-specific dispatch differs from a service-driven
   automation is a P4 detail the Id resolution already frames.
4. **The `EffectApi`↔`delegation()` call ergonomics** (Id §15): single composed decision vs decomposed terms
   — the *algebra* (§5.2) is decided; the call shape is joint with Id, co-finalised in P4.
5. **The MCP wire + external-agent rate-limit lane** (§6.2): the external MCP server's auth, the agent-lane
   shedding for external clients, and the per-external-tenant budget model.
6. **Trace verbosity / reasoning capture policy** (§4.5): how much of the model's intermediate reasoning the
   trace captures (a privacy + AI-Act + cost trade-off), and its retention — a product + Legal call (L-4).

---

## 13. Cross-references
- Spine: **ADR-08** (plan-then-apply / strategy boundary / tool registry / safety), **ADR-09** (durable
  workflow), ADR-03 (ReBAC `check`/`list_objects`/delegation), ADR-16 (backpressure / agent lane), ADR-17
  (fail-static, via Id), ADR-19 (Signals / the dispatch tier), ADR-20 (one sandbox), ADR-11 (cells), ADR-12
  (GDPR holders), ADR-13 (glue contracts / same-gateway).
- Directives: AG-1 (stateless `step`), AG-2 (`exec`, no host bypass), AG-3 (skeleton-first), AG-4
  (`--use-mock` same path + `cargo-mutants`), AG-5 (Denied = ordinary error), AG-6 (loop guards), AG-7 (trace
  = content-addressed doc), AG-8 (approve→resume); CI-1 (secrets-in-boundary), CI-2/D8 (reserve/settle);
  CHAT-1 (explicit-first); X-1…X-5.
- Foundational Phase-3 docs consumed: [`00-platform-substrate.md`](./00-platform-substrate.md) (consumer
  template, resilient client, backpressure, `myelin-agent` stubs), [`identity-and-access.md`](./identity-and-access.md)
  (mint/revoke, the `∩` delegation algebra, per-run tokens), [`event-bus.md`](./event-bus.md) (EventInbox via
  the dispatch tier, Signals, causality, loop guards).
- Doctrine: [`external-insights/03-agent-native-fabric.md`](../../external-insights/03-agent-native-fabric.md)
  (all), EI-02 §2/§5/§6, EI-04 §5.1.
- Prior art: ReAct (ICLR 2023); MCP (Anthropic 2024); macaroons (NDSS 2014); Zanzibar (ATC 2019); gVisor /
  Firecracker (NSDI 2020); Saltzer & Schroeder (1975); Temporal/Cadence; Lamport (1978).
