# Phase 5 — Agent Fabric (`myelin-agent`): the refined, canonical shared-system architecture

> Phase: `05-refined-shared-systems-architecture` (the reconciliation; VISION §5). Canonical brief:
> [`VISION.md`](../../VISION.md). Binding doctrine:
> [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md),
> [`external-insights/03-agent-native-fabric.md`](../../external-insights/03-agent-native-fabric.md) (all),
> [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §5.1.
> Reconciliation spine: [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md) (X-6, OQ-E,
> OQ-F, OQ-K, OQ-L; CR §6) + [`contract-index.md`](./contract-index.md) (the frozen build-to surface this
> doc matches — contracts 8.1..8.8, plus the dependencies 4.x/7.3/9.x/11.7). Carries forward Phase 3:
> [`../03-shared-systems-architecture/agent-fabric.md`](../03-shared-systems-architecture/agent-fabric.md).
> Spine: ADR-08, ADR-09 (also ADR-03, ADR-11, ADR-12, ADR-13, ADR-16, ADR-17, ADR-19, ADR-20). Date: 2026-06-19.
>
> **What this is.** The REFINED Agent Fabric that Phase 6 (roadmaps) and Phase 7/8 (build) implement. The
> Phase-3 design is the base and is **carried forward**; this doc applies the Phase-5 reconciliation
> decisions and the Agent-Fabric change requests, and makes the exposed contracts explicit and final to
> match the refined contract index. Where something is unchanged from Phase 3, it says so and cites it
> rather than restating it.
>
> **Status convention.** *DECIDED* = committed. *FLOOR* = a partial answer shipped with a named follow-on
> (VISION §3, EI-04 §4). *[OPEN → P6/LEGAL]* = handed forward. Snippets are Rust-shaped contract signatures
> (ADR-02), not implementations.

---

## Changes vs Phase 3 (every change, with its source)

The Agent-Fabric change requests (CR §6) are **all CONFIRM** — the Phase-3 seams were correct. The deltas
are the X-6 **SHARPEN** (the four uniform sandbox guarantees + the frozen `requires_approval` defaults
table), and a set of adjacent-contract sharpenings the Fabric now *relies on* but does not itself own. No
ADR is reversed; nothing in the Phase-3 trait set, the plan-then-apply pipeline, the three runtimes, or the
loop guards changes shape.

| # | Change | Kind | Source | Where in this doc |
|---|---|---|---|---|
| C1 | **The four uniform sandbox guarantees pinned** — every execution (CI *or* agent) inherits, by construction: (1) the universal reserve/settle cost gate, (2) per-run attenuated-token attribution, (3) HITL withhold (plan-then-apply), (4) the isolation floor + the real-kernel escape drill. No subsystem re-implements them. | **SHARPEN** (contract 8.4) | recon §X-6 | §2.2, §5.0, §5.7 |
| C2 | **The per-subsystem `requires_approval` defaults table is frozen** — CI deploy/secret = yes; Git merge = yes, open_pr = no; Issues forecast/triage = no, SLA transition = caveat-gated; KN publish/confidential = yes; Chat post = no; a cross-subsystem effect inherits the **target** subsystem's default. | **SHARPEN** (contract 8.1) | recon §X-6 | §6.3 |
| C3 | **`EffectApi` capability check now passes a `CaveatContext`** for field/transition ABAC (Issues SLA-bound transition, KN confidential field), evaluated at `check`-time — off the hot `list_objects` path. | **SHARPEN** (consumes 4.2) | recon §OQ-E | §5.2 |
| C4 | **HITL resume idempotency is now per-effect** — a batch approval card uses `idem_key = card_id:<effect_idx>`; a double-click is one approval, a partial approval is well-defined, each effect maps to exactly one `EffectApi::apply`. | **SHARPEN** (consumes 9.1) | recon §OQ-F | §5.3 |
| C5 | **`ToolHands::exec` is realised as the CI runner's `kind=agent` job on the ONE unified sandbox**, and the `SCHEDULE_AND_RUN_JOB` long-park-completed-by-signal idiom is the dispatch shape (the activity dispatches, completion arrives as a durable signal hours later; the run holds no runtime). | **CONFIRM + idiom pinned** (consumes 9.2/9.4) | recon §X-6/§OQ-F | §2.2, §5.6 |
| C6 | **`mint_run_token` is re-mintable mid-workflow on resume** (a multi-day HITL pause that survives a token TTL re-mints on wake). The per-run token contract gained this specificity in Id (4.7). | **CONFIRM (Id sharpen)** (consumes 4.7) | recon §1 | §5.7 |
| C7 | **Explicit-first dispatch pinned** — a mention notifies; it does **not** auto-spawn a costed run. Implicit auto-dispatch is **L-3** (counsel-gated). | **CONFIRM** (contract 8.6) | recon §6, CHAT-1 | §3.4, §12 |
| C8 | **AG-7 trace = a content-addressed Knowledge document + erasable holder** — re-confirmed; the trace reuses the `myelin-content` block model (now frozen, contract 13.1) and registers as a `PersonalDataHolder`. | **CONFIRM** (contract 8.8) | recon §6, AG-7 | §4.5 |
| C9 | **The humanisation of HITL cards + agent-authored messages goes through the ONE templating surface** (`humanise`, contract 7.3) — no agent-authored raw strings; `(template_key, args)` + `ArtifactRef`, per-viewer, erasure-safe. | **CONFIRM (Notif sharpen)** (consumes 7.3) | recon §OQ-L | §5.3 |
| C10 | **Per-surface agent-lane shed budget named as a v1 floor** — the agent-mention-storm budget (per-tenant agent-run in-flight cap; humans never queue behind agents; `429 + Retry-After`). | **CONFIRM (floor)** (consumes 1.11) | recon §OQ-K | §8 |

**Unchanged from Phase 3 (carried forward, cited, not restated):** the trait set (§1.3) — brain `step` /
hands `exec` / `Agent::handle` / `ToolSurface` / `EventInbox` / `EffectApi`; the brain+hands strategy
boundary and the platform-owned-history loop (§2); the three runtimes SKELETON → Mock(`--use-mock`) → Llm
(seam-only) (§3); the data model `run`/`tool_def`/`proposed_effect`/`hitl_gate`/trace (§4); the
plan-then-apply pipeline (§5.2); the structural loop guards (§5.5); a-run-is-a-durable-workflow (§5.6); the
MCP-exposure path (§6); the scaling/blast-radius register (§8); the drill set D-1..D-11 (§9). All as Phase 3.

---

## 1. Purpose, responsibilities, and the trait set (unchanged from Phase 3)

`myelin-agent` is **the strategy-pattern boundary** behind which a `MockAgentRuntime` lives today and an
`LlmAgentRuntime` lives later (VISION §3 non-negotiable; ADR-08; EI-03 §1). The thesis is unchanged: **if the
substrate is right, an agent needs almost no special code** — an agent is a `Principal` with `kind=agent`
running through the *same* identity, gateway, event log, sandbox, and cost gate as everyone else (EI-03
preamble; EI-02 §2). What it owns, what it consumes-not-owns, and the trait set are exactly as Phase-3 §1.1,
§1.2, §1.3 — **carried forward unchanged**. The trait set at a glance (the §7 contract surface; concrete
shapes unchanged):

```rust
pub trait AgentRuntime { fn step(&self, conv: &Conversation) -> Result<StepOutcome>; }   // THE BRAIN (AG-1)
pub enum StepOutcome { UseTools(Vec<ToolCall>), Submit(Submission) }
pub trait Agent { fn handle(&self, inbox: InboxEvent, runtime: &dyn AgentRuntime) -> Result<RunOutcome>; } // THE LOOP (AG-3)
pub trait ToolHands { fn exec(&self, cmd: Command) -> Result<ToolResult>; }              // THE HANDS (AG-2)
pub trait ToolSurface { fn register_tool(&mut self, def: ToolDef); fn resolve(&self, name: &ToolName) -> Option<&ToolDef>; }
pub trait EventInbox { fn deliver(&self, ev: InboxEvent); }                              // DELIVERY (ADR-08)
pub trait EffectApi { fn apply(&self, run: &RunCtx, effect: ProposedEffect) -> EffectResult; } // PLAN-THEN-APPLY (ADR-08.3)
```

The **only** strategy-swappable members are `AgentRuntime` (brain) and `ToolHands` (hands). `Agent`,
`ToolSurface`, `EventInbox`, `EffectApi` are platform-owned and identical for mock and real — the whole
point of plan-then-apply (Phase-3 §1.3, unchanged).

**Reconciliation note (no ownership change).** Phase 5 does not move any boundary: Id still owns permission
decisions, per-run token minting, and the new authz reverse index; the Bus still owns the inbox/dispatch
tier; CI still owns the unified sandbox runner + the escape drill (ADR-20); the durable-workflow engine still
owns waits/timers/signals; GDPR/Audit still owns the tamper-evident log; Commercial still owns the wallet.
The Fabric *consumes* each via a now-frozen contract. The X-6 reconciliation only **pins the guarantees** a
subsystem's tool inherits when it executes here — it does not change who owns what.

---

## 2. The two strategy boundaries — brain & hands (unchanged shape; hands guarantees pinned)

### 2.1 The brain — `step(conversation) -> {use_tools | submit}` (AG-1, DECIDED, unchanged)

Unchanged from Phase-3 §2.1. The provider trait is **stateless**; the platform-side loop owns the
`Conversation` history (the system context, prior tool results, the running transcript — platform data, a
`PersonalDataHolder`, residency-pinned, the trace AG-7). Rationale unchanged: determinism & testability
(stateless `step` is trivially mockable + golden/mutation-testable), platform-owns-history-so-platform-owns-truth,
and the proven ReAct tool-use-loop shape (Yao et al., ICLR 2023) named as a *shape*, no vendor (ADR-08.2).
The `Conversation`/`Turn` structs are exactly Phase-3 §2.1.

**One reconciliation touch (additive).** The tool list the brain sees (`Conversation.tools`) is the run's
permitted, delegation-scoped subset, computed at conversation-build time. Phase 5 freezes how that subset is
computed without an N+1: it is the OQ-E **`list_objects` push-down** (contract 4.3) — the brain is shown
exactly the tools whose target objects survive the ACL pre-filter, lowered to a single query, never a
per-tool `check`. (`EffectApi` still re-checks at apply time — the scoping is an optimisation, the check is
the guarantee, fail-closed.)

### 2.2 The hands — `exec(command) -> result`, no host-exec bypass + the four uniform guarantees (AG-2; X-6 SHARPEN)

The hands trait is one method with **no host-execution path that bypasses it** (AG-2; the `no-host-exec`
lint, contract 1.6) — unchanged from Phase-3 §2.2. The two implementations (`SandboxedHands` real /
`SimHands` simulation with the channel-proof marker) are unchanged. The boundary note is unchanged:
*side-effecting* mutation goes through **`EffectApi`** (governed mutation, §5.2), never through
`ToolHands::exec`; `exec` carries only **untrusted code execution** (`compute`/`external` — a test, a build,
a linter, a script) — the only thing that touches the kernel sandbox.

**The X-6 SHARPEN (the load-bearing Phase-5 change here).** `ToolHands::exec` **is** the CI runner's
`kind=agent` job on the **ONE unified sandbox** (ADR-20; contract 8.4), and Phase 5 pins the **four uniform
guarantees** every subsystem's tool inherits *by construction* — so Issues/Knowledge/Chat (which register
gated `ToolDef`s that ultimately execute here) never re-implement any of them:

1. **Universal cost gate.** Every execution passes the reserve/settle bookend (contract 11.7): reserve at
   dispatch, refuse-on-exhaustion, settle on completion, never interrupt in-flight. CI runs and agent runs
   meter into the **same wallet** (Commercial C-1). A subsystem tool cannot opt out. (§5.4.)
2. **Attribution.** The job executes under a **per-run attenuated token** (contract 4.7, `mint_run_token`),
   life == run life, auto-revoked on teardown, **re-mintable mid-workflow on resume** (C6/S-11). Every effect
   is attributed to the run principal with nested causality (BUS-5). (§5.7.)
3. **HITL withhold (plan-then-apply).** Side-effecting mutation goes through `EffectApi::apply`, which
   enforces schema → capability → delegation → tenant → budget → **HITL gate** → apply-via-public-endpoint →
   meter. A gated tool whose name is not in the approved set is **withheld** (returns a `Denied`/`Gated` tool
   error, does **not** mutate — AG-8). `ToolHands::exec` carries only untrusted code, never privileged
   mutation — the routing split is the safety boundary. (§5.0, §5.2.)
4. **Isolation floor + drill.** gVisor-class userspace-kernel **or** microVM; the named hardening profile
   (egress default-deny, read-only root + tmpfs, caps dropped, no-new-privileges, seccomp, digest-pinned
   images fail-closed on un-digested tags, whole-guest kill on teardown, `pids.max` + zero swap, secrets
   resolved *inside* the boundary and never forwarded via the runtime). **The real-kernel escape drill is the
   single hard go/no-go before any untrusted customer code runs — CI *or* agent** (D-4; T-5; EI-04 §5.1).
   CI owns the runner + the drill (ADR-20); the Fabric feeds the `kind=agent` job spec.

This is the answer to the change-requests risk that "a subsystem assumes execution semantics the unified
runner must guarantee uniformly" (X-6): the four guarantees are now contract, not convention.

### 2.3 `Agent::handle` — the bounded driven multi-turn loop (AG-3, DECIDED, unchanged)

Unchanged from Phase-3 §2.3. `Agent::handle` is a **platform-owned, bounded, driven multi-turn loop** — not
a single call, not the provider's responsibility. The loop body (`build_conversation` from the trace → open
the reserve → repeatedly `step` the brain → route tool calls per §5.0 → append results → settle) is exactly
Phase-3 §2.3. Properties pinned there hold unchanged: multi-turn platform-driven; streaming is a transport
detail *inside* the runtime (the trait sees a complete `StepOutcome`); context management is the platform's
job; the loop is bounded by three independent ceilings (`max_steps`, the reserve/settle budget, the causal-
depth ceiling — none sufficient alone); plan-then-apply survives (the brain only ever *proposes*).

---

## 3. The three runtimes (AG-3 build order: SKELETON → mock → real) — unchanged

§3.1 **SKELETON** (built; no model, no tools; proves the whole gateway/identity/dispatch/reserve/trace path
at ~zero cost), §3.2 **MockAgentRuntime** (built; deterministic scripted `StepOutcome`s; shipped as a real
`--use-mock` runtime flag on the *same* code path users hit; the lever for golden + `cargo-mutants` testing
of the event→trigger→effect→event loop, AG-4), and §3.3 **LlmAgentRuntime** (the only vendor seam;
**designed-not-built**, FLOOR; the only place a model/SDK/prompt/model-name string appears, enforced by the
`no-llm-in-platform` lint, contract 1.6; EU-hostable, region-aware, swappable; metering one cost event per
model call, wholesale ≠ markup) are **carried forward unchanged from Phase-3 §3**.

### 3.4 Explicit-first dispatch (CHAT-1; CONFIRM, pinned)

Runtime dispatch is **explicit** "run an agent here" (contract 8.6; EI-03 §7). A mention **notifies** (via
Notif's one inbox) — it does **not** auto-spawn a costed agent run. Even an explicit run passes the
reserve/settle gate. Implicit auto-dispatch on a casual mention (with intent/cost detection) is a separately-
decided product feature, **L-3 (counsel-gated** — GDPR Art. 22 / EU AI-Act human-oversight) — **not built
here**, carried to §12. This is the Phase-3 floor #3, now ratified by the reconciliation.

---

## 4. The data model / schemas (unchanged from Phase 3, with two annotations)

All tables: `(tenant, region)` first column, RLS-enforced, no cross-tenant query path (EI-02 §1; ID-3);
residency-pinned, per-tenant envelope-encrypted, crypto-shred-capable, `PersonalDataHolder` (ADR-11/12;
contracts 1.4/10.1). The four tables are **carried forward unchanged from Phase-3 §4**:

- **`run`** (§4.1) — the unit of agent execution; a durable-workflow instance (ADR-09). Fields unchanged
  (`run_id`, `agent_principal`, `on_behalf_of`, `binding_id`, `trigger_event`, `correlation_id`/
  `causation_id`/`depth`, `runtime_ref` = the strategy swap, `state`, `reservation_id`, `budget` in integer
  minor-units, `trace_ref`). A run may pause for *days* on a HITL gate holding no thread (§5.6).
- **`tool_def`** (§4.2) — the one permissioned registry. Fields unchanged (`name`, `subsystem`, `version`,
  `input_schema`, `required_caps`, `effect_kind` ∈ read|compute|mutate|external, `side_effecting`,
  `requires_approval`, `exposed_over_mcp`). **Annotation (C2):** the *default value* of `requires_approval`
  per subsystem tool is now frozen by the §6.3 table — the column is unchanged; its seed values are pinned.
- **`proposed_effect`** (§4.3) — the plan-then-apply audit row; every proposed effect recorded whether
  applied, gated, or denied. Fields unchanged.
- **`hitl_gate`** (§4.4) — the approval state, a durable-workflow wait surfaced as a chat approval card.
  Fields unchanged (`gate_id`, `run_id`, `effect_id`, `risk_summary` humanised, `cost_estimate`,
  `approver_filter` = a `list_subjects`-derived set, `state`, `card_ref`). **Annotation (C4):** the resume
  signal's idempotency key is per-effect (`card_id:<effect_idx>` for a multi-effect card; §5.3).
- **`trace`** (§4.5) — **the execution trace is a content-addressed Knowledge document** reusing
  `myelin-content` (AG-7; contract 8.8). Unchanged, with one tightening: `myelin-content` is now the **frozen
  taxonomy** (contract 13.1, recon §X-2), so the trace writes to a stable block model. The trace is a
  `PersonalDataHolder` (residency-pinned, crypto-shred-capable, erasable); `run.trace_ref` is its
  `ArtifactRef`; it is **distinct from** the tamper-evident audit log (GDPR/Audit owns that). Required change
  to Knowledge (accept a content-addressed agent-trace write) is unchanged — §11.

---

## 5. The algorithms

### 5.0 Routing a tool call (unchanged, with the four guarantees made explicit)

When the brain returns `UseTools(calls)`, the platform loop routes **per `effect_kind` / `side_effecting`** —
the Phase-3 §5.0 table, unchanged:

| `effect_kind` | `side_effecting` | Route | Why |
|---|---|---|---|
| `read` | false | direct (subsystem read API, permission-checked) | a permission-filtered read; no mutation, no sandbox |
| `compute` | false | **`ToolHands::exec`** (the unified sandbox; ADR-20) | runs untrusted code (test/build/lint/script) |
| `mutate` | true | **`EffectApi::apply`** (plan-then-apply; §5.2) | a governed platform mutation via the public endpoint |
| `external` | true | **`EffectApi::apply`** → an egress-reviewed adapter | a side-effecting external call (webhook, etc.) |

`exec` is for sandboxed computation; `EffectApi` is for governed mutation. Both are validated; only
`compute`/`external` untrusted code touches the kernel sandbox — and **whichever route runs, the four X-6
guarantees apply** (cost gate fronts both; both run under the per-run token; mutation is HITL-gated; the
escape drill gates the sandbox path).

### 5.1 The agent loop (the platform-owned driver) — unchanged

Carried forward from Phase-3 §5.1: builds the conversation from the trace (AG-1), opens the reserve (D8),
steps the brain, routes (§5.0), appends results, settles. **Identical for mock and real.** Causality is
carried **nested** (BUS-5) via `OutboxTx::emit(draft, cause)` so a loop guard reads platform metadata, not a
convention — *the agent cannot typo into a loop* (EI-02 §6).

### 5.2 Plan-then-apply `EffectApi` (ADR-08.3 — core safety+testability; one step gains a `CaveatContext`)

Agents are a **pure-ish function `(event, context) → AgentDecision { effects }`**; they **never perform side
effects directly** (ADR-08.3). They emit *proposed* effects; `EffectApi` validates each and applies it. The
pipeline is **in order, fail-closed**, unchanged from Phase-3 §5.2 except step 2's signature (C3):

```
EffectApi::apply(run, effect):
  1. SCHEMA      — validate `effect.input` against the ToolDef JSON Schema; malformed ⇒ Denied.
  2. CAPABILITY  — the run's per-run identity must hold `tool_def.required_caps`:
                   Id.check(run.agent_principal, required_cap, effect.object, zookie, caveat?) ─┐
                   where `caveat: CaveatContext{object, field?, transition?, attrs}` carries the  │ ALL must
                   field/transition ABAC condition (OQ-E) — e.g. an Issues SLA-bound              ├─ hold
                   `transition(issue, →done)` gates on the approver edge; a KN confidential-field │ (intersection)
                   write gates the field. The caveat is evaluated HERE, off the hot list path.    │
  3. DELEGATION  — Id.delegation(agent, trigger_actor) → `agent.policy ∩ delegation ∩ tenant.policy` │
                   (Id §7); attenuation, never up.                                                  │
  4. TENANT      — tenant guardrails (agent-allow-list, residency, AI-Act) — the tenant.policy term ─┘
  5. BUDGET      — the reserve has remaining balance for this effect's metered cost (11.7, §5.4).
  6. HITL GATE   — if `tool_def.requires_approval` (per the §6.3 frozen defaults) AND not yet approved
                   for this run ⇒ WITHHELD: open a hitl_gate (§5.3), return Gated. (Tool returns an error;
                   does NOT mutate. AG-8.)
  7. APPLY       — call the subsystem's PUBLIC endpoint as the agent principal (same gateway, no carve-out,
                   EI-03 §4) ⇒ the subsystem emits its domain event via ITS outbox. Record applied.
  8. METER       — settle one cost event for this effect (D8); wholesale ≠ markup kept separate.
  → EffectResult ∈ { Applied(event_id) | Gated(gate_id) | Denied(reason) }
```

**The C3 change (SHARPEN, consumes contract 4.2):** step 2 now passes a `CaveatContext` so **field-level**
hiding and **transition** approver checks are evaluated at `check`-time on the already-fetched object —
never on the hot `list_objects` path (recon §OQ-E). The `requires_approval` resolution in step 6 (C2) reads
the **frozen §6.3 defaults**, including the cross-subsystem rule (a Chat-invoked effect that mutates another
subsystem inherits **that** subsystem's default — "the effect is governed where it lands").

Key properties unchanged from Phase 3: a denied effect returns an **ordinary `Denied` tool error** — **no
privileged fallback** (AG-5); **same gateway, no carve-out** (EI-03 §4); **intersection, not union** so an
agent can do nothing no human role can (EI-02 §2); **identical mock/real**; `--dry-run` stops after step 6
and shows the plan (contract 8.7).

### 5.3 HITL: withhold → approve → resume (AG-8; resume idempotency now per-effect — C4)

The end-to-end loop is unchanged from Phase-3 §5.3: **withhold** (a gated effect returns `Gated`, does not
mutate) → **surface** (a durable-workflow wait, contract 9.4, surfaced as a chat approval card showing the
pending action + risk + **live cost estimate**, with the approver set = `list_subjects(object, approve_perm)`,
contract 4.4) → **decide** (minutes or days later; the durable wait holds no runtime) → **resume** (the
workflow signal re-runs the step with the tool name added to "approved"; step 6 now passes; the effect
applies). Rejection settles `Halted::Rejected` with the reason in the trace + audit.

**Two reconciliation tightenings:**

- **C4 — per-effect resume idempotency (consumes contract 9.1, OQ-F).** A batch approval card may gate
  **multiple effects** ("approve these 3 proposed merges"). The resume signal's idempotency key is
  **per-effect**: `idem_key = card_id` for a single-effect card; `idem_key = card_id ":" effect_idx` for a
  multi-effect card. A **partial approval** (approve effects 0 and 2, decline 1) sends three independently-
  idempotent signals; each maps to **exactly one** `EffectApi::apply`; a declined effect is **withheld**
  (AG-8). A double-click on "approve all" re-sends the same keys → no double-apply. "A double-click is one
  approval" and "a partial approval is well-defined" are both true by construction.
- **C9 — the card text goes through the ONE templating surface (consumes contract 7.3, OQ-L).** The card's
  `risk_summary` and any agent-authored message are **never raw strings**: they are a `(template_key, args)`
  pair + an `ArtifactRef`, humanised per-viewer by Notif `humanise` (ICU MessageFormat, permission/erasure-
  safe). There is no second template engine and no frontend string map.

**Suggest-by-default; human-confirm consequential actions** (ADR-08.6; GDPR Art. 22 / EU AI-Act human-
oversight). Which tools are `requires_approval` by default is now the **frozen §6.3 table** (was a Phase-4
open question — closed).

### 5.4 Reserve/settle: the universal cost gate (D8 / CI-2, DECIDED — now guarantee #1) — unchanged

Carried forward from Phase-3 §5.4: **reserve at dispatch, settle on completion, refuse to start when balance
is exhausted, never interrupt one in flight.** Meter one cost event per model call (and per metered effect),
wholesale ≠ markup kept separate (C-1: pricing history immutable), integer minor-units never floats. The
gate **uniformly fronts CI runs and agent runs into the same wallet** (CI-2/D8). Phase 5 elevates this to
**uniform guarantee #1** (§2.2) — it is not new, but it is now contract-level mandatory for every execution,
CI or agent, via the same reserve/settle bookend (contract 11.7) and the `SCHEDULE_AND_RUN_JOB` reserve-at-
dispatch (§5.6).

### 5.5 The structural loop guards (AG-6, DECIDED) — unchanged

Carried forward from Phase-3 §5.5, unchanged. Loop prevention is **structural, not a convention**; the guards
live primarily in the Bus reactive/dispatch tier (contract 3.6) and the Fabric re-enforces at apply time
(defence in depth): **self-guard** (drop an inbound event whose `actor.principal` == this agent),
**reference gate** (only a structured `artifact_ref` node can re-trigger — never raw typed text; wired to
the now-frozen `myelin-content` inline ref nodes, contract 13.1), **causal-depth ceiling** (drop/park when
`depth > ceiling`, default 12), **shared-root tripwire** (> K events on one `correlation_id` in a window
trips a per-tenant circuit breaker), **idempotent tools** (on `(run, effect_id)`), **bounded dispatch pool**
(drops over-cap — never forks unboundedly). Because the guards read platform causality metadata, *a human
(or agent) can never typo their way into a loop* (EI-02 §6). The causal-loop tripwire drill (D-7) proves it.

### 5.6 A run is a durable workflow (ADR-09) + the `SCHEDULE_AND_RUN_JOB` long-park idiom (C5)

The Phase-3 §5.6 mapping is unchanged: the **workflow is durable and owns budget/gates/state**; the **model
step (`step`) and tool calls (`exec`/`apply`) are activities**; a HITL gate is a **signal**; `stale_after`/
timeout is a **durable timer**. A run can pause for *days* on an approval without holding a thread.

**The C5 pin (consumes contracts 9.2/9.4, OQ-F).** A long sandbox job (a `compute`/`external` tool whose CI
run takes minutes-to-hours) uses the frozen **`SCHEDULE_AND_RUN_JOB` long-park-completed-by-signal idiom**:

```
let job = ctx.activity(SCHEDULE_AND_RUN_JOB, JobSpec{ kind: agent, ..., idem_token })?;  // dispatches; returns immediately (reserve at dispatch — 11.7)
ctx.wait_for_signal("job.done", idem_key = job.idem_token)?;                              // parks: holds NO runtime (9.4)
// ... woken hours later by signal(run, "job.done", {result}, idem_key=job.idem_token) ...
```

The activity **dispatches** the `kind=agent` job (reserve at dispatch — guarantee #1) and returns; it does
**not** block on completion. Completion arrives as a **durable signal idempotent on `idem_token`** (the
runner can deliver "done" twice; the workflow wakes once). The run holds no runtime while the multi-hour
sandbox job runs. This is the same idiom CI's merge-queue uses for `ci.result` (recon §X-1); the Fabric uses
it for any long `ToolHands::exec`. **The Fabric consumes this; it does not reinvent durable waits** (TE-20
build-vs-adopt is the Bus/workflow item).

### 5.7 Per-run identity: mint, scrub, revoke — re-mintable on resume (ID-2; C6)

Carried forward from Phase-3 §5.7: at dispatch the Fabric requests Id `mint_run_token(agent_id, run_id,
delegation_caveats, ttl)` with **token life == run life** (ID-2); it unsets any shared platform token in the
child environment (anti-leak); on teardown it calls Id `revoke(jti)` **idempotently even on crash**, belt-
and-suspenders with Id's auto-expiring tuples (`expires_at` == run life). *An agent literally cannot exceed
its identity* (EI-02 §2).

**The C6 tightening (consumes contract 4.7).** A run that parks for **days** on a HITL gate (or a long
`SCHEDULE_AND_RUN_JOB`) may outlive its token's TTL. The per-run token is therefore **re-mintable mid-
workflow on resume** (S-11): on wake, the workflow re-mints a fresh attenuated token with the same delegation
caveats and the remaining run life, so a long pause never widens the attribution window beyond the TTL bound
and never leaves a run unattributed. This is an Id contract specificity (4.7) the Fabric now relies on.

---

## 6. The permissioned tool registry + the MCP-exposure path + the frozen `requires_approval` defaults

### 6.1 One catalogue, two front-ends (DECIDED) — unchanged

Carried forward from Phase-3 §6.1: every subsystem registers typed `ToolDef`s into **one shared
`ToolSurface`** (`name` + JSON-schema input + required caps + effect-kind + side-effecting +
`requires_approval` + `exposed_over_mcp`). The same registry is consumed **internally** (the
`Conversation.tools` the brain sees are the run's permitted, delegation-scoped subset — now via the OQ-E
push-down, §2.1) and **exposable over MCP** to external agents later — *defined once, governed once*.

### 6.2 The MCP-exposure path (DECIDED — seam built, external endpoint FLOOR) — unchanged

Carried forward from Phase-3 §6.2: we expose the `exposed_over_mcp = true` subset as an MCP **server** behind
the same gateway and the same Id `check` (no carve-out — an external MCP client is a `Principal`, gets a
per-run token, flows through `EffectApi` exactly like an internal agent). The MCP surface is a **projection
of `ToolDef`** (input_schema → MCP schema, required_caps → Id-enforced, side_effecting/requires_approval →
the same plan-then-apply + HITL path) — no second governance model. **FLOOR:** v1 builds the catalogue + the
internal consumption path + the `exposed_over_mcp` seam; the **external MCP server endpoint** (its auth,
agent-lane rate-limit, per-external-tenant budget, threat model, and Legal/DPO sign-off) is a named P6
follow-on. MCP = Model Context Protocol (Anthropic, 2024).

### 6.3 The per-subsystem `requires_approval` defaults table (FROZEN — C2 / X-6)

The Phase-4 open question ("which mutations are HITL-gated by default") is **closed**. The product-call
defaults, gated-by-default for any consequential/irreversible action (ADR-08.6 suggest-by-default; GDPR Art.
22), are frozen jointly with the Fabric (contract 8.1):

| Subsystem | Tool (examples) | `requires_approval` default | Rationale |
|---|---|---|---|
| **CI** | `deploy(env)` to a protected env | **yes** | protected-env deploy is consequential |
| **CI** | `approve_deploy`, `write_secret` | **yes** | secret write + approval are privileged |
| **CI** | `run_pipeline` (non-prod) | no | cheap, reversible, metered |
| **Git** | `git.merge` | **yes** | merge is the consequential gate (AG-8) |
| **Git** | `open_pr` | no | reversible |
| **Issues** | `forecast`, `triage`, `sla_draft` | no (suggest) | advisory; the human accepts the suggestion |
| **Issues** | `transition(issue, →done)` on an SLA-bound issue | **yes** if the transition has an approver edge (ABAC) | the field/transition ABAC caveat (§5.2 step 2, OQ-E) |
| **Knowledge** | `publish`, `edit(confidential_page)` | **yes** | publishing/confidential edits are consequential (approver set) |
| **Knowledge** | `draft`, `comment` | no | reversible |
| **Chat** | `post_message`, `react` | no | reversible, cheap |
| **Chat** | any `EffectApi` tool that mutates another subsystem | inherits **that** subsystem's default | the effect is governed where it lands, not where it's invoked |

This table is the seed for the `tool_def.requires_approval` column (§4.2). A subsystem may tighten a default
(mark more tools gated) per tenant policy, but may not loosen a `yes` to `no` for a consequential action
without a written deviation (VISION §3).

---

## 7. Contracts / APIs exposed and consumed (the glue — STABLE, final)

These match [`contract-index.md`](./contract-index.md) §8 (and the dependency rows). Stability semantics:
changing one is a whole-workspace PR that breaks every consumer's build *now* (ADR-01).

### 7.1 Exposed by the Fabric (contracts 8.1–8.8)

| Contract | Signature | Status vs P3 | Consumed by |
|---|---|---|---|
| **8.1 `ToolSurface::register_tool` + `resolve`** | `register_tool(ToolDef{name, input_schema, required_caps, effect_kind, side_effecting, requires_approval, exposed_over_mcp})` | **SHARPENED** (the §6.3 defaults table frozen) | every subsystem |
| **8.2 `EffectApi::apply`** | `apply(run, ProposedEffect) → Applied(event_id) \| Gated(gate_id) \| Denied(reason)` — plan-then-apply; withheld gated tool does not mutate (AG-8) | CONFIRMED | the loop, external MCP, workflow activities |
| **8.3 `AgentRuntime::step`** | `step(&Conversation) → UseTools(Vec<ToolCall>) \| Submit(Submission)` — the stateless brain; strategy seam (skeleton/mock/llm); `--use-mock` is a real flag | CONFIRMED | runtimes; the loop (an activity) |
| **8.4 `ToolHands::exec`** | `exec(Command) → ToolResult` — sandboxed computation; no host bypass; **= CI's `kind=agent` job**; the real-kernel escape drill gates both kinds; **the four uniform guarantees** | **SHARPENED** (four guarantees pinned, X-6) | the loop |
| **8.5 `Agent::handle`** | `handle(InboxEvent, &dyn AgentRuntime) → RunOutcome` — the bounded multi-turn loop; nested causality; a run is a durable workflow | CONFIRMED | dispatch tier |
| **8.6 `EventInbox::deliver`** | `deliver(InboxEvent)` — platform delivers matched events (envelope + binding + token + budget); **explicit-first dispatch** (a mention notifies, does not auto-spawn a costed run; implicit is L-3) | **SHARPENED** (explicit-first pinned) | Bus → Agent |
| **8.7 `run --dry-run`** | `dry_run(InboxEvent) → Vec<ProposedEffect>` (no apply) — plan-then-apply testability | CONFIRMED | CLI, tests |
| **8.8 AG-7 trace** | Knowledge accepts a content-addressed agent-trace write (reuses the block model, contract 13.1) + registers it as an erasable `PersonalDataHolder` | CONFIRMED | Agent ↔ Knowledge |

### 7.2 Consumed from other shared systems (the dependencies — final)

| From | Contract used | For | Status vs P3 |
|---|---|---|---|
| **Id** | `mint_run_token`/`revoke` (4.7, **re-mintable on resume**); `check(.., caveat?: CaveatContext)` (4.2); `list_objects → SetExpr` push-down (4.3); `delegation` (4.5, the `∩` algebra); `list_subjects` (4.4, HITL approver set) | per-run identity; effect validation; tool-list scoping; HITL approvers | **SHARPENED** (CaveatContext 4.2, SetExpr 4.3, re-mint 4.7) |
| **Event Bus** | `EventInbox` via the dispatch tier (3.6); `OutboxTx::emit(draft, cause)` (2.2, causality); Signals (3.1) | wake-up; emitting domain events; loop guards | CONFIRMED |
| **Durable-workflow engine** | `DurableExecutor{start, signal, describe, cancel}` (9.1, **per-effect `idem_key`**); `WfCtx` + `SCHEDULE_AND_RUN_JOB` long-park idiom (9.2); durable HITL signal (9.4) | HITL waits, long sandbox jobs, budgets, pauses | **SHARPENED** (per-effect idem_key 9.1, SCHEDULE_AND_RUN_JOB 9.2/9.4) |
| **CI unified runner** | the `kind=agent` job spec + the hardening profile + the escape drill (8.4; ADR-20) | `ToolHands::exec` real execution | **SHARPENED** (four guarantees) |
| **Knowledge** | content-addressed write of the trace document (8.8; `myelin-content` 13.1) | the execution trace (§4.5) | CONFIRMED |
| **GDPR/Audit** | `audit.record` via outbox (10.6); `PersonalDataHolder` registration (10.1); the **one free-text erasure posture by reference** (10.9) | tamper-evident audit; erasure of run/trace/memory | CONFIRMED |
| **Notifications** | `humanise((template_key, args), viewer, locale)` (7.3) | HITL card text + agent-authored messages | **SHARPENED** (sole templating surface, OQ-L) |
| **Commercial wallet** | the prepaid balance + per-capability add-on the reserve/settle gate reads (11.7, C-1) | the cost gate (§5.4) | CONFIRMED |

### 7.3 The resilient client + backpressure (consumed, not re-built) — unchanged

All outbound inter-service calls go through the **shared resilient client** (contract 1.9): timeout / breaker
/ bulkhead / jittered-retry-idempotent-only, and **the agent runtime MUST honour `Retry-After`** (ADR-16
§3) — *or shedding becomes a retry storm* (EI-02 §5). The agent lane is the **shed-before-human** lane (shed
order: speculative → batch/CI → agent → human-last); a `429 + Retry-After` to an agent is an ordinary
backoff surfaced to the loop as a transient tool error.

---

## 8. Scaling / sharding in the cell topology (ADR-11) — unchanged, with the agent-lane shed budget named

Carried forward from Phase-3 §8: **in-cell, agent processing stays in-region** (no cross-region agent runs on
personal data); **the novel scale+safety concern is agent-generated load** (agents generate volume far
beyond humans and agents-waking-agents can cascade), bounded by the causal-depth ceiling + reserve/settle +
idempotent tools + the bounded dispatch pool + per-tenant circuit breakers (§5.5); **runs are durable-
workflow instances** so millions of paused runs cost storage not compute; **the runtime workers are
stateless** (a crashed worker's run resumes from the durable workflow + the trace). The §8.1 stateful-
component register + blast-radius note is unchanged.

**The C10 floor (consumes contract 1.11, OQ-K).** The **agent-mention-storm** shed budget is named as a v1
floor: a **per-tenant agent-run in-flight cap** (reserve/settle refuses over-cap), **humans never queue
behind agent runs** (the protected human lane), and the agent lane sheds with `429 + Retry-After` that the
agent runtime honours (§7.3). The concrete number is the subsystem's P4/P6 budget call tuned by the **30×
agent-surge drill** (D-6); the floor is "the agent lane is bounded, has a reserved human lane, and applies
the shed order" — an unbounded one is the cascade (EI-02 §5).

---

## 9. Failure modes + the drills owed (PROVE-IT) — unchanged D-1..D-11

The drill set is **carried forward unchanged from Phase-3 §9** (a green artifact ⇒ "proven"; until then
"claimed", T-4). Phase 5 changes none of them; it tightens the inputs two drills read:

| # | Property | Drill (gate) | Phase-5 note |
|---|---|---|---|
| D-1 | Agent cannot mutate directly | adversarial: no host/DB path; `no-host-exec` + `no-cross-db` lints green; **0 direct mutation** | unchanged |
| D-2 | Denied → ordinary tool error, no escalation (AG-5) | propose outside the `∩`; `Denied` returns; **0 escalations** | unchanged |
| D-3 | Delegation intersection / least-privilege (Id §7) | adversarial over/under-privilege incl. a delegator who lost the right; **0 over-privilege** | unchanged |
| D-4 | **Sandbox escape on a real kernel** (ADR-20) | the single hard gate before any untrusted code runs (CI *or* agent); **0 escapes** | the X-6 guarantee #4 the drill proves; owned by CI |
| D-5 | HITL withhold→approve→resume (AG-8) | withheld (error, no mutate), card shows action+risk+cost, approval resumes & applies **exactly once**; **0 mutations before approval** | now also asserts **per-effect idempotency** (C4): partial approval + double-click are well-defined |
| D-6 | 30× agent surge / fairness (ADR-16) | human lane holds, agent lane sheds (429+Retry-After honoured), other tenants unaffected, reserve refuses over-budget | now asserts the **named agent-lane shed budget** (C10/OQ-K) |
| D-7 | Causal-loop tripwire (AG-6) | depth ceiling + shared-root tripwire + bounded pool halt it; breaker trips; **loop halts ≤ ceiling** | unchanged |
| D-8 | Per-run token outlives the run (ID-2) | revoked on teardown AND auto-expires ≤ W; **0 leaked tokens** | now also asserts **re-mint on resume** (C6) keeps a multi-day pause attributed within the TTL bound |
| D-9 | Determinism: mock loop reproducible (AG-4) | identical runs; `cargo-mutants` score ≥ threshold | unchanged |
| D-10 | Erasure reaches the trace + memory (AG-7) | erase a subject; trace + memory crypto-shredded; attribution → pseudonym; **0 recoverable PII** | reads the **one erasure posture** (contract 10.9) by reference |
| D-11 | Cost gate: runaway self-limiting (D8) | refuses to start past exhaustion, never interrupts in-flight; **stops at the wallet** | unchanged |

Phase 5 owns the drill **mechanics** (the testing strategy); this section enumerates the properties the
Fabric owes.

---

## 10. Cited prior art (unchanged from Phase 3)

Carried forward from Phase-3 §10, unchanged: **ReAct** (Yao et al., ICLR 2023) + Toolformer (NeurIPS 2023)
+ the OpenAI/Anthropic tool-use loop *shape* (no vendor named in platform code); **MCP** (Anthropic, 2024);
capability security — Saltzer & Schroeder (1975), Dennis & Van Horn (CACM 1966), **macaroons** (Birgisson et
al., NDSS 2014) / biscuit, Miller et al. *Capability Myths Demolished*; **Zanzibar** (Pang et al., ATC 2019)
for `check`/`list_objects` (the OQ-E reverse-index push-down is Zanzibar's `LookupResources` realised as a
co-located JOIN target); **gVisor** (Young et al., 2019) / **Firecracker** (Agache et al., NSDI 2020) for the
sandbox floor; **Temporal/Cadence** durable execution (a run is a workflow); **Lamport** (CACM 1978) for the
happened-before basis the loop guards read; **Nygard** *Release It!* + Google SRE ch. 21/22 + AWS Builders'
Library for backpressure/shedding. Doctrine: EI-03 §1–§7, EI-02 §2/§5/§6/§8, EI-04 §5.1.

---

## 11. Required changes to foundational systems (now satisfied by the frozen contracts)

The Phase-3 §11 required changes are now **frozen contracts** in the refined index (no longer "required
changes" — they are committed seams):

1. **Knowledge — content-addressed agent-trace write (AG-7).** Now contract 8.8 + the frozen `myelin-content`
   taxonomy (13.1). Knowledge is the deliverable; the seam is fixed.
2. **CI runner — the `kind=agent` job spec + the escape drill (ADR-20).** Now contract 8.4 with the four
   uniform guarantees pinned. CI owns the runner + the drill (X-6).
3. **Durable-workflow engine — open/signal/timer + the `SCHEDULE_AND_RUN_JOB` long-park idiom.** Now
   contracts 9.1/9.2/9.4 (OQ-F). The Fabric consumes; TE-20 build-vs-adopt is the Bus/workflow item.
4. **Commercial wallet — the prepaid balance the reserve/settle gate reads.** Contract 11.7 + C-1.
5. **The `no-llm-in-platform` lint.** Contract 1.6 (the lint set). No model/SDK/prompt string outside
   `LlmAgentRuntime`.

No change is required to the **envelope** (the Fabric uses `actor.run`/`actor.principal`/causality as-is) or
to **Id's core contracts** — and the Id sharpenings the Fabric now relies on (CaveatContext 4.2, SetExpr
4.3, re-mintable token 4.7) were designed with the Fabric as a consumer.

---

## 12. Open questions remaining for Phase 6 (honesty register)

**Named floors (built-with-a-named-follow-on; VISION §3):**
- **`LlmAgentRuntime` is designed-not-built** (§3.3): the trait seam is fixed; the EU-sovereign region-aware
  adapter + its wholesale metering rates are a P6 follow-on; the EU-sovereign sub-processor is `[OPEN →
  LEGAL]` (AG-9).
- **External MCP server is a floor** (§6.2): the `exposed_over_mcp` seam + internal consumption are built;
  the external endpoint + its auth/agent-lane rate-limit/per-external-tenant budget/threat model + Legal/DPO
  sign-off are a P6 follow-on.
- **Agent long-term memory / RAG over prior runs** (Phase-3 §0.4): a named holder seam; the embedding store
  + its erasure are a Search/Knowledge follow-on. v1 agents are **stateless across runs except for the
  content-addressed trace document**. (When built, it indexes via Search `semantic` (contract 6.2), ACL-
  filtered-during-traversal, and is purged on `*.erased` — the structural erasure path already exists.)
- **The per-surface agent-lane shed budget is a v1 floor** (§8, OQ-K): the concrete in-flight cap + human-
  lane reservation are tuned by the 30× agent-surge drill (D-6).

**`[OPEN → LEGAL]` (flagged to counsel/DPO; the structural floor ships regardless):**
- **Implicit auto-dispatch on casual mention** (CHAT-1, **L-3**): explicit-first is v1 (§3.4); implicit
  auto-wake (with intent/cost detection) is a separately-decided product feature requiring a DPO sign-off
  (GDPR Art. 22 / EU AI-Act human-oversight). **Defensible engineering posture:** ship explicit-first only;
  do not wire any auto-spawn path until counsel ratifies the human-oversight basis. Owned by Chat P6 +
  Commercial + Legal. (We are not counsel — flagged.)
- **Trace verbosity / reasoning-capture policy** (§4.5, **L-4**): how much of the model's intermediate
  reasoning the trace captures (a privacy + AI-Act + cost trade-off) and its retention. **Defensible
  posture:** capture the tool-call/result transcript (load-bearing for audit + replay) by default; gate
  capture of free-form chain-of-thought behind a tenant setting tagged `#[personal_data]` under the one
  erasure posture (contract 10.9); flag the retention + AI-Act classification for counsel.
- **Build-data-as-LLM-training basis** (OQ-H, AG-8): **foreclosed by default** — no platform code path feeds
  tenant content to model training; training-on-tenant-data is a separately-ratified opt-in, not a default.
  Flagged for counsel.

**Cross-system seams ratified elsewhere (no longer Fabric open questions):** the `requires_approval` defaults
(§6.3, was P4-open — closed); the `EffectApi`↔`delegation` call ergonomics (the `∩` algebra is decided;
contract 4.5 froze the call shape); `Agent` vs `Service` in the dispatch path (Id resolved as one kind /
three faces — the loop guards respect `actor.kind`).

---

## 13. Cross-references
- Refined index: [`contract-index.md`](./contract-index.md) §8 (8.1–8.8) + the dependency rows 4.2/4.3/4.7/
  7.3/9.1/9.2/9.4/11.7. Rationale: [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md)
  §X-6, §OQ-E, §OQ-F, §OQ-K, §OQ-L, §6.
- Phase-3 base (carried forward): [`../03-shared-systems-architecture/agent-fabric.md`](../03-shared-systems-architecture/agent-fabric.md).
- Spine: ADR-08 (plan-then-apply / strategy boundary / tool registry / safety), ADR-09 (durable workflow),
  ADR-03 (ReBAC), ADR-16 (backpressure / agent lane), ADR-17 (fail-static), ADR-19 (Signals / dispatch tier),
  ADR-20 (one sandbox), ADR-11 (cells), ADR-12 (GDPR holders), ADR-13 (glue contracts / same-gateway).
- Directives: AG-1..AG-8, CI-1/CI-2 (D8), CHAT-1, X-1..X-6.
- Doctrine: [`../../external-insights/03-agent-native-fabric.md`](../../external-insights/03-agent-native-fabric.md)
  (all), [`../../external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md)
  §2/§5/§6, [`../../external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §5.1.
