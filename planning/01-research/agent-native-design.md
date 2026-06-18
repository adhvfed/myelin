# Agent-Native Design — Research

> Phase 1 research. This is exploratory and deliberately broad. Nothing here is a final
> contract; the abstraction sketches below are *candidates* to be refined in
> `02-holistic-architecture` and `03-shared-systems-architecture`. Where I am uncertain I
> say so explicitly (see the "Uncertainty & open questions" section, and inline `⚠` flags).
>
> Canonical premises (from `VISION.md`): agents are first-class citizens, not bolt-ons;
> the platform provides first-class **event propagation and triggers** across all five
> subsystems; during development we ship **mock** agent implementations and use the
> **strategy pattern** so that swapping mock→real is a config/implementation swap, not a
> rewrite; everything is GDPR-safe and EU-sovereign by construction; honesty about
> uncertainty is required.

---

## 0. Executive summary

"Agent-native from the ground up" decomposes into four concrete platform capabilities that
must exist as *shared backend systems*, not per-subsystem features:

1. **The agent fabric** — agents are real principals in the identity & access system. They
   have identities, scoped permissions, audit trails, rate budgets, and lifecycle. A human
   user and an agent are the *same kind of thing* to authorization, attribution, and audit
   (with extra constraints on agents). This is what makes "humans and agents in the same
   chat channels" coherent rather than a hack.

2. **The event bus + event propagation** — every subsystem emits a canonical, versioned
   stream of domain events (commit pushed, PR opened, CI failed, issue transitioned, doc
   edited, message posted, artifact referenced, …). Events are the substrate agents observe
   and react to, and the substrate that drives the cross-artifact reference graph,
   notifications, and search indexing. Agents are *just another consumer* of this stream.

3. **Triggers / subscriptions / automations** — a declarative layer on top of the event bus
   that says "when event matching X happens, run automation/agent Y under principal Z with
   budget B." This is the platform's native answer to webhooks + Zapier + GitHub Actions
   `on:` + Temporal-style durable workflows, unified.

4. **The agent runtime abstraction (strategy pattern)** — a stable trait/interface boundary
   (`AgentRuntime` + `Agent` + a `ToolSurface` + an `EventInbox` + an `EffectApi`) behind
   which a `MockAgentRuntime` lives today and an `LlmAgentRuntime` lives later. The platform
   core depends only on the trait, never on an LLM SDK.

The hard, non-negotiable cross-cutting concern across all four is **safety & governance**:
permissions, rate limits, loop/runaway protection, human-in-the-loop (HITL) gates, and
attribution/audit strong enough to satisfy GDPR and the EU AI Act. Agent-native without this
is a liability machine. I treat safety as a first-class design driver, not an appendix.

---

## 1. The agent fabric: agents as first-class principals

### 1.1 Principle: one principal model, three principal *kinds*

The identity & access (IAM) shared system should model a single `Principal` abstraction with
discriminated kinds:

- `Human` — a person, authenticates via OIDC/SSO/passkey.
- `Agent` — an autonomous actor (mock or LLM-backed) that acts under delegated authority.
- `Service` — non-agent system automation (CI runners, webhooks-out, indexers). Distinguished
  from `Agent` because services are *deterministic plumbing*, while agents are *reasoning,
  potentially non-deterministic actors* and therefore carry extra governance (AI Act, loop
  protection, HITL). ⚠ Whether `Service` and `Agent` should be one type with a flag, or two
  types, is an open design question — see §7.

Why "first-class" matters concretely:

- **Authorization** evaluates the same policy engine for an agent comment as for a human
  comment. No separate "bot API" backdoor that bypasses permission checks.
- **Attribution**: every mutation in every subsystem records `actor: PrincipalRef`. An agent
  edit to a doc is attributable to *that agent identity*, and (crucially) to the
  *on-behalf-of* human/team that delegated authority.
- **Audit & GDPR**: agents are data processors that can read personal data; their reads and
  writes must be logged with lawful basis, and must be erasable/exportable like any other
  actor's contributions (see §6).

### 1.2 The agent identity record (sketch)

```
AgentIdentity {
  id:            PrincipalId,            // stable, globally unique
  kind:          Agent,
  display:       { name, avatar, handle } // shows up in chat/PR/issue UIs like a user
  owner:         PrincipalRef,           // human or team that owns/operates this agent
  on_behalf_of:  Option<PrincipalRef>,   // delegated authority context (see §1.3)
  runtime_ref:   AgentRuntimeId,         // which runtime impl backs it (mock|llm|...)
  capabilities:  Set<Capability>,        // declared tool/skill surface it MAY use
  policy:        AgentPolicy,            // permissions, budgets, HITL gates (see §5)
  lifecycle:     Active | Suspended | Retired,
  tenant:        TenantId,               // multi-tenant isolation, EU residency tag
  created_at, created_by, ...
}
```

### 1.3 Delegated authority ("on behalf of")

The trickiest IAM question. An agent that triages CI failures might need to read a private
repo, open an issue, and post in chat. Two models:

- **A. Agent has its own static role/permissions** (like a GitHub "machine user" / app).
  Simple, auditable, but coarse: the agent can do *everything its role allows* regardless of
  who triggered it.
- **B. Agent acts *on behalf of* a triggering principal**, inheriting the *intersection* of
  the agent's own grant and the triggering principal's grant (least privilege per
  invocation). More GDPR-friendly (data minimisation) but more complex; requires threading a
  delegation token through the whole invocation.

**Recommendation (tentative):** support both, default to **B with intersection semantics**
for event-triggered runs, and require explicit static grants (A) only for "house agents"
that run with no human trigger (e.g. a nightly housekeeping agent). The effective permission
set for any agent action = `agent.policy.permissions ∩ delegation.permissions ∩ tenant.policy`.
⚠ The exact algebra (intersection vs. additive scopes, how to represent "the agent may do X
only when triggered by someone who can do X") needs a dedicated authz design pass.

### 1.4 Why not just "a user with a bot flag"?

We *almost* could, and the principal model deliberately keeps agents close to users. But
agents need three things humans don't, which justify the distinct kind:

1. **Budgets & loop protection** (a human can't infinite-loop the API at machine speed).
2. **AI Act governance** — disclosure that you're talking to an AI, logging of automated
   decisions, the right to human review.
3. **A runtime binding** — agents are *executed* by a runtime; humans are not.

---

## 2. Event propagation as a platform primitive

### 2.1 The shared event bus

Every subsystem is an **event producer**. The event bus is a shared system that:

- Accepts **canonical domain events** (typed, versioned, immutable, append-only).
- Fans them out to consumers: the trigger/automation engine, the agent fabric, the
  cross-artifact reference graph builder, the search indexer, the notifications service, and
  external webhook delivery.
- Provides **ordering guarantees per aggregate** (e.g. all events for a single PR are
  ordered) and **at-least-once delivery** with idempotency keys for consumers.

This is the classic **pub/sub + event-sourcing-flavoured** backbone. Two design axes worth
separating early:

- **Event notification vs. event-carried state vs. event sourcing.** ⚠ Open question per
  subsystem: do we *source* the aggregate from events (full event sourcing, replayable
  state) or do we keep authoritative state in a DB and *emit* events as a side effect
  (transactional outbox)? My tentative steer: **transactional outbox / change-data-capture
  for most subsystems** (simpler, proven, lets each subsystem own its storage), and reserve
  true event sourcing for aggregates where the audit/replay value is highest (issue tracker
  transitions, permission changes). The platform-level *contract* — "a reliable ordered
  stream of canonical events" — is the same either way, which is what lets us defer the
  internal decision to each subsystem architecture phase.

### 2.2 The canonical event taxonomy (what every subsystem MUST emit)

A non-exhaustive but representative catalogue. The naming convention proposed:
`<subsystem>.<aggregate>.<verb_past_tense>`, e.g. `git.push.created`. Every event carries a
**common envelope** (see §2.3).

**Git hosting**
- `git.repo.created` / `archived` / `deleted` / `visibility_changed`
- `git.branch.created` / `deleted`
- `git.push.created` (commits pushed to a ref)
- `git.commit.created` (per-commit, may be derived/batched)
- `git.tag.created`
- `git.pr.opened` / `updated` / `closed` / `merged` / `reopened` / `marked_ready`
- `git.pr.review_requested` / `review_submitted` (approved/changes_requested/commented)
- `git.pr.comment_created` (incl. line/thread comments)

**CI**
- `ci.pipeline.triggered` / `started` / `succeeded` / `failed` / `cancelled` / `timed_out`
- `ci.job.started` / `finished` (with status, logs ref, artifacts ref)
- `ci.step.failed` (granular, useful for triage agents)
- `ci.artifact.published`
- `ci.deployment.requested` / `succeeded` / `failed` (if CD is in scope)

**Issue tracker**
- `issue.created` / `updated` / `deleted`
- `issue.transitioned` (state machine move; carries from→to, who, why)
- `issue.assigned` / `unassigned`
- `issue.commented`
- `issue.linked` (blocks/blocked-by/duplicate/relates-to)
- `issue.field_changed` (custom fields, priority, SLA clock events)
- `issue.sla.breached` / `sla.at_risk`
- `sprint.started` / `closed`; `roadmap.item.scheduled`

**Knowledge platform**
- `doc.created` / `edited` / `deleted` / `moved` / `restored`
- `doc.published` / `unpublished`
- `doc.comment_created` / `suggestion_made` / `suggestion_resolved`
- `db.row.created` / `updated` / `deleted` (Notion-style databases)
- `db.schema_changed`
- `folder.created` / `moved`

**Chat**
- `chat.message.posted` / `edited` / `deleted`
- `chat.thread.created`
- `chat.reaction.added`
- `chat.channel.created` / `member_added` / `member_removed`
- `chat.mention.created` (human OR agent mentioned — a key agent trigger)

**Cross-cutting / shared**
- `ref.created` — an artifact referenced another (the reference graph edge; emitted whenever
  a commit message cites an issue, a chat message links a doc, a PR closes an issue, etc.).
  This is special: it's both an event *and* the thing that builds the reference graph.
- `iam.principal.created` / `permission_granted` / `revoked` / `role_changed`
- `agent.run.started` / `finished` / `failed` / `gated` (the agent fabric emits its own
  events, so agents can observe agents — see loop concerns in §5).
- `gdpr.erasure.requested` / `export.requested` (drives compliance workflows).

### 2.3 The common event envelope (sketch)

```
EventEnvelope<P> {
  event_id:     Uuid,               // unique; idempotency anchor
  type:         "git.pr.opened",    // dotted canonical type
  schema_ver:   u32,                // versioned payloads, additive evolution
  occurred_at:  Timestamp,          // domain time
  recorded_at:  Timestamp,          // bus ingest time
  tenant:       TenantId,           // isolation + EU residency
  actor:        PrincipalRef,       // who caused it (human/agent/service)
  subject:      ArtifactRef,        // canonical ref to the aggregate (see §2.4)
  causation_id: Option<Uuid>,       // the event/command that caused this one
  correlation_id: Uuid,            // ties a whole workflow/run together
  trace_id:     TraceId,            // distributed tracing
  payload:      P,                  // typed, subsystem-specific
  // governance:
  contains_personal_data: bool,     // hint for GDPR routing/redaction
  visibility:   VisibilityScope,    // who/what may consume this event
}
```

`causation_id` + `correlation_id` are doing heavy lifting: they let us reconstruct *and cap*
agent-triggered chains (loop protection, §5) and give auditors a full provenance graph.

### 2.4 Artifact references & the reference graph

A `ArtifactRef` is a universal addressing scheme for *anything* in the platform —
`myelin://<tenant>/<subsystem>/<type>/<id>[#sub]` (e.g. a PR, an issue, a doc block, a chat
message, a CI run). Chat "references any artifact" and the reference graph are both built on
this. Every `ref.created` event adds an edge `(source ArtifactRef) --rel--> (target
ArtifactRef)`. Agents are first-class consumers: a triage agent that opens an issue and links
it to a commit is just emitting `ref.created` edges that the UI renders as backlinks.

---

## 3. Triggers, subscriptions & automations

This is the declarative control plane on top of the event bus. Conceptually three layers,
increasing in power:

| Layer | Analogy | What it does | Determinism |
|---|---|---|---|
| **Subscriptions** | webhook / pub-sub topic | "deliver events matching filter F to consumer C" | deterministic routing |
| **Automations** | Zapier / GitHub Actions `on:` | "when F, run action A (possibly multi-step) under principal Z, budget B" | deterministic-ish (declarative steps) |
| **Agent triggers** | agent inbox | "when F, wake agent G with the event as input; it decides what to do" | non-deterministic (reasoning) |

### 3.1 Trigger definition (sketch)

```
Trigger {
  id:        TriggerId,
  tenant:    TenantId,
  match:     EventMatcher,        // type globs + payload predicates, e.g.
                                  //   type == "ci.pipeline.failed"
                                  //   && payload.branch == "main"
  target:    TriggerTarget,       // Subscription(consumer)
                                  // | Automation(workflow_id)
                                  // | Agent(agent_id)
  run_as:    PrincipalRef,        // identity the resulting actions attribute to
  delegation: DelegationPolicy,   // static | on_behalf_of(triggering actor) (see §1.3)
  budget:    RunBudget,           // max actions, max wall-clock, max LLM calls/cost
  gates:     Vec<HitlGate>,       // human approvals required before certain effects (§5)
  dedup_key: Option<Template>,    // collapse duplicate triggers
  enabled:   bool,
}
```

`EventMatcher` should be expressive but *safe to evaluate cheaply* (no Turing-complete
predicates on the hot path) — a CEL-like or JSONLogic-like expression sandbox is the usual
choice. ⚠ Exact predicate language TBD.

### 3.2 Why a workflow engine, and which flavour

Single-step "event → one action" (webhook/Zapier-lite) is insufficient for the multi-subsystem
workflows the vision demands (CI fail → triage → open issue → link commit → post chat →
propose PR). Those are **durable, multi-step, possibly long-running, partially human-gated**
workflows. The relevant patterns:

- **GitHub Actions** model: declarative YAML, triggered by events, runs jobs/steps. Great UX
  for users, but the engine is opaque and not durable across very long human-in-the-loop
  waits.
- **Temporal / durable execution** model: code-as-workflow with durable state, retries,
  timers, signals, and the ability to *wait days for a human signal* without holding
  resources. This is the right substrate for agent workflows that pause on HITL gates.
- **Zapier** model: best-in-class *authoring UX* and connector catalogue; the lesson is the
  no-code trigger/filter/action builder, not the runtime.

**Tentative steer:** the *runtime* should be **durable-execution-style** (Temporal-like
semantics: deterministic workflow + non-deterministic activities, durable timers, signals
for HITL). Whether we adopt Temporal itself, build on a Rust durable-execution library, or
build bespoke is a Phase 2/3 decision. ⚠ Building durable execution from scratch is a large
undertaking; flagged as a major architectural risk/decision. The agent run loop maps cleanly:
the *workflow* is durable and owns budget/gates/state; the agent's *reasoning step* and *tool
calls* are *activities* (non-deterministic, retryable, sandboxed).

### 3.3 Subscriptions also serve non-agent needs

The same subscription primitive powers: outbound webhooks (for users integrating Myelin with
external systems ⚠ subject to EU-sovereignty review of where data egresses), search indexing,
notification fan-out, and the reference-graph builder. Designing it agent-first but
general-purpose avoids a parallel "events for agents" vs "events for everything else" split.

---

## 4. The strategy-pattern agent runtime abstraction

This is the heart of the "mock now, real later, trivial swap" requirement. The platform core
must depend **only** on a small set of traits. No LLM SDK, no prompt, no model name appears
anywhere in the platform; all of that lives behind `LlmAgentRuntime` and is introduced later.

### 4.1 The abstraction boundary (the contract)

Four traits + their data types form the boundary. (Rust-flavoured pseudocode; this is a
*sketch*, not the final API — see §7.)

```rust
/// A runtime that can host agents. The platform holds runtimes behind this trait only.
/// Implementations: MockAgentRuntime (now), LlmAgentRuntime (later), and possibly
/// per-vendor runtimes — all interchangeable via config.
trait AgentRuntime: Send + Sync {
    fn id(&self) -> AgentRuntimeId;

    /// Construct/resume an agent instance bound to an identity + its tool surface.
    fn instantiate(&self, agent: &AgentIdentity, tools: ToolSurface) -> Box<dyn Agent>;

    fn capabilities(&self) -> RuntimeCapabilities; // streaming? tools? max ctx? etc.
}

/// A single agent that reacts to events delivered to its inbox and emits effects.
#[async_trait]
trait Agent: Send {
    /// Core reaction. Given an event (and accumulated context), decide what to do.
    /// Returns a *plan of effects*, never performs side effects directly — the platform
    /// applies effects through the EffectApi after policy/gate checks. This separation is
    /// what makes mock agents deterministic and real agents safely sandboxed.
    async fn handle(&mut self, event: InboxEvent, ctx: &AgentContext)
        -> Result<AgentDecision, AgentError>;
}

/// What the agent decided to do this turn.
struct AgentDecision {
    rationale: String,          // human-readable, logged for audit/AI-Act transparency
    effects:   Vec<Effect>,     // proposed actions back into the platform
    follow_up: FollowUp,        // Done | AwaitEvent(matcher) | Sleep(dur) | NeedsHuman(gate)
    cost:      RuntimeCost,     // tokens/$/wallclock — for budget accounting
}
```

### 4.2 The tool/skill surface (MCP-style)

The agent's *only* way to affect the world is by emitting `Effect`s, and the *catalogue* of
effects it may emit is the **`ToolSurface`** — a curated, permissioned set of typed tools.
This mirrors **MCP (Model Context Protocol)** and the "tools" surface of modern agent
frameworks: each tool has a name, a JSON-schema'd input, a description, and a permission
requirement. ⚠ I'm confident about the *shape* of MCP-style tool exposure (name + schema +
description + invoke); I'd verify exact MCP wire details before claiming protocol-level
compatibility.

```rust
struct ToolSurface { tools: Vec<ToolDef> }

struct ToolDef {
    name:        String,                 // "issue.create", "chat.post", "git.open_pr"
    description: String,                 // shown to the agent (real) / ignored (mock)
    input_schema: JsonSchema,
    required_caps: Set<Capability>,      // checked against agent + delegation perms
    effect_kind:  EffectKind,            // maps tool → platform Effect
    side_effecting: bool,                // read-only tools need no HITL gate
}
```

Every Myelin subsystem registers tools into this surface. Crucially **the same registry can
be exposed over MCP to external/3rd-party agents** later, *and* consumed internally by our own
runtimes — one tool catalogue, two front-ends. This is the unification payoff: tools are
defined once, governed once, and the agent fabric, the LLM runtime, and any external MCP
client all see the same permissioned catalogue.

Representative tools by subsystem: `git.open_pr`, `git.comment_on_pr`, `ci.rerun_pipeline`,
`ci.read_logs` (read-only), `issue.create`, `issue.transition`, `issue.link`, `doc.read`,
`doc.create`, `doc.suggest_edit`, `chat.post`, `chat.react`, `search.query` (read-only),
`ref.create`.

### 4.3 The event inbox

Agents don't poll; the platform **delivers** matched events to an agent's inbox (driven by
triggers, §3). The inbox abstraction:

```rust
struct InboxEvent {
    envelope:   EventEnvelope,    // the canonical event that woke the agent
    trigger:    TriggerId,        // which trigger matched
    delegation: DelegationToken,  // effective authority for this run (§1.3)
    budget:     RunBudget,        // remaining budget for this run
    history:    RunHistory,       // prior turns in this correlation_id (for multi-turn)
}
```

This makes the agent's input fully explicit and reproducible — essential for deterministic
mocks and for replaying/auditing real runs.

### 4.4 The effect / action API (write-back into the platform)

Effects are the *requested* mutations. They are **not executed by the agent**; the platform's
`EffectApi` validates each against permissions + budget + HITL gates, then applies it,
emitting the resulting domain events (which may wake more agents — governed by loop
protection, §5).

```rust
enum Effect {
    InvokeTool { tool: String, input: JsonValue },   // the general case
    EmitReference { from: ArtifactRef, to: ArtifactRef, rel: RelKind },
    RequestHumanApproval { gate: HitlGate, summary: String },
    NoOp { reason: String },
}

#[async_trait]
trait EffectApi {
    async fn apply(&self, run: &RunContext, effect: Effect)
        -> Result<EffectOutcome, EffectError>; // Applied | Gated(pending_approval) | Denied
}
```

The **plan-then-apply** split (agent returns `Vec<Effect>`; platform applies) is the single
most important design choice for safety *and* testability: the agent is a pure-ish function
from `(event, context) → decision`, and all the dangerous, stateful, permissioned stuff lives
in platform code that is identical for mock and real agents.

### 4.5 The mock agent (deterministic, testable)

`MockAgentRuntime` produces `MockAgent`s whose behaviour is **rule-driven and deterministic**,
so the *entire* event→trigger→agent→effect→event loop can be integration-tested without any
LLM. Design:

- **Scripted/rule-based decisions:** a `MockAgent` is configured with an ordered list of
  rules `(EventMatcher → Vec<Effect>)`. Given an `InboxEvent`, it returns the first matching
  rule's effects, a canned rationale, and `cost = 0` (or a fixed synthetic cost to exercise
  budget logic). Same input ⇒ same output, always.
- **Deterministic "reasoning" when needed:** for tests that need variation, seed a PRNG from
  `event_id` so outputs are *deterministic per event* but varied across events.
- **Fixture personas:** ship named mock agents — e.g. `MockTriageAgent` (on `ci.pipeline.failed`
  → create issue + link commit + post chat), `MockReviewerAgent` (on `git.pr.opened` → post a
  templated review comment), `MockDocBot` (on `chat.mention` in a #docs channel → create a doc
  stub). These double as **executable examples** of the workflows in §8 and as the seed data
  that makes the product demoable end-to-end before any real model exists.
- **Failure injection:** mock agents can be configured to return `AgentError`, exceed budget,
  or request a HITL gate, so safety machinery is testable.
- **Golden tests / mutation testing:** because mocks are deterministic, we can write golden
  tests over the emitted event sequence and use `cargo-mutants` (per the tech steer) to verify
  the trigger/effect/gate logic is actually exercised.

The swap to real is then: register `LlmAgentRuntime` (which internally does prompt
construction, tool-call loop, an LLM client) under the same `AgentRuntime` trait and point the
agent identity's `runtime_ref` at it. **No platform code changes.** That is the whole point of
the strategy pattern here.

### 4.6 What the LlmAgentRuntime will add later (so we don't paint ourselves in)

Even though we don't build it now, the trait must *leave room* for: streaming partial outputs,
multi-turn tool-call loops (the model calls a read-only tool, gets a result, reasons again),
context-window management, model/provider selection per tenant (EU-hosted models for
sovereignty ⚠), prompt/response logging for AI-Act transparency, and structured tool-call
outputs. The traits above (multi-turn via `history` + `follow_up`, tools via `ToolSurface`,
cost via `RuntimeCost`) are designed to accommodate this; if any of these turns out to be
missing, that's a contract revision we should anticipate (§7).

---

## 5. Safety, permissions, rate limits, loop protection, HITL, audit

Agent-native makes safety load-bearing. The mechanisms:

### 5.1 Permissions (least privilege, per-run)

- Every effect is permission-checked by the *same* policy engine as human actions (§1).
- Effective perms = `agent.policy ∩ delegation ∩ tenant.policy` (§1.3).
- Read-only tools vs. side-effecting tools are distinguished so read-heavy agents need fewer
  dangerous grants.
- Sensitive effect kinds (deleting data, merging to protected branches, changing permissions,
  emailing externally, anything touching personal data) require an explicit capability *and*
  typically a HITL gate.

### 5.2 Rate limits & budgets

- Per-run `RunBudget`: max effects, max wall-clock, max LLM calls / token cost, max fan-out
  (events emitted).
- Per-agent and per-tenant rolling budgets (e.g. N runs/min, $X/day) to cap blast radius and
  cost.
- Budgets are enforced by *platform* code in the run/workflow engine, not trusted to the
  agent. Exceeding budget terminates the run and emits `agent.run.failed{reason: budget}`.

### 5.3 Loop / runaway protection (the scariest failure mode)

Agents emit events; events wake agents; this can cascade. Mitigations, layered:

- **Causation depth cap:** each event carries `causation_id`; the engine computes chain depth
  within a `correlation_id` and refuses to wake an agent past a max depth.
- **Cycle detection:** if an agent's effects would re-trigger the same agent on a
  substantially-similar subject, dampen or stop (dedup keys, §3.1).
- **Convergence / idempotency:** tools should be idempotent where possible (creating the
  "same" triage issue twice should dedupe), so loops are harmless even if they occur.
- **Global circuit breakers:** per-tenant automation kill-switch; auto-trip if event volume or
  cost spikes; alarms to operators.
- **Agents observing agents:** allowed (powerful), but `agent.run.*` events feed the same
  depth/cycle accounting so agent-to-agent chains can't escape the caps.

⚠ This is the area I'd most want a dedicated adversarial design + load test for. The cascade
risk is real and easy to underestimate.

### 5.4 Human-in-the-loop gates

- A `HitlGate` pauses a workflow (durable wait, §3.2) until a human with the right permission
  approves/rejects/edits the proposed effect.
- Gates are declarative on the trigger and/or required by tool definition (`side_effecting +
  sensitive`).
- The chat subsystem is a natural HITL surface: "Agent X wants to merge PR #42 — Approve /
  Reject / Edit." This reuses the same chat/notification plumbing — agents and humans in one
  channel is exactly what makes HITL ergonomic.
- AI Act tie-in: where automated actions have legal/significant effect, human review must be
  *available*; gates are the mechanism.

### 5.5 Attribution & audit (GDPR / EU AI Act)

- **Attribution:** every mutation records `actor = agent`, `on_behalf_of`, `trigger`,
  `correlation_id`. UIs visibly mark agent-authored content (transparency; AI Act
  disclosure). No "silent" agent edits.
- **Audit log:** append-only, tamper-evident record of: which agent, triggered by which
  event, under which delegation, proposed which effects, which were applied/gated/denied, with
  rationale and (for LLM runtimes later) the model + a reference to the prompt/response for
  explainability. Retention + access controlled.
- **GDPR specifics:**
  - Agents are *processors*; their access to personal data is logged with lawful basis and
    minimised via delegation/intersection perms.
  - `gdpr.erasure.requested` must reach agent-produced artifacts and agent run logs; agent
    contributions are erasable/exportable like any actor's. ⚠ Erasing data that fed an LLM
    decision is genuinely hard (the model "saw" it); our mitigation is that we don't fine-tune
    on tenant data and we log inputs/outputs as erasable records, not in model weights — needs
    a dedicated GDPR-vs-LLM design note.
  - EU residency: agent runtimes (especially future LLM ones) must run in EU-controlled
    infra / use EU-hosted models per tenant policy; the runtime abstraction carries a
    residency/sovereignty constraint that the platform enforces before dispatching a run.
- **AI Act tie-in (high level, ⚠ not legal advice):** transparency (users know they're
  dealing with AI), logging/record-keeping of automated decisions, human oversight (HITL),
  and risk management. Most Myelin agent use looks like *limited-risk* (transparency
  obligations) rather than high-risk, but anything touching employment/HR-like decisions in
  the issue tracker could escalate — flagged for legal review in a later phase.

---

## 6. Patterns to learn from (synthesis)

| Pattern | What we borrow | What we change |
|---|---|---|
| **Webhooks** | event-driven outbound delivery; retries; signing | internalise as first-class subscriptions; avoid HTTP-only/loss-prone semantics; sovereignty controls on egress |
| **Event sourcing** | append-only, immutable, replayable audit-grade event log; provenance | apply selectively (outbox by default), not universally; keep per-aggregate ordering |
| **Pub/Sub** | decoupled fan-out; many consumers per event; topic/filter routing | typed canonical events + envelope; at-least-once + idempotency, not fire-and-forget |
| **Agent-framework "tools"** | typed tool surface, name+schema+description, tool-call loop | platform-owned, permissioned registry; plan-then-apply; same registry internal & external |
| **MCP** | standard way to expose tools/resources to LLM agents | expose our tool registry over MCP so external agents are first-class too; verify wire details ⚠ |
| **Zapier** | no-code trigger/filter/action authoring UX; connector catalogue | the *authoring UX* is the lesson; our runtime is durable, not single-shot |
| **GitHub Actions** | `on: <event>` declarative triggers; great DX; matrix/jobs | events come from the shared bus, not just git; durable HITL waits |
| **Temporal / durable execution** | durable workflows, signals, timers, retries, await-human-for-days | likely our automation runtime; map agent-turn=activity, workflow=durable orchestrator. Build-vs-adopt is open ⚠ |

The synthesis: **events (event-sourcing/pub-sub discipline) → triggers (webhook/Actions DX) →
durable automations (Temporal semantics) → agents (tools/MCP surface with plan-then-apply)**,
all sharing one principal, permission, and audit model.

---

## 7. Uncertainty, assumptions, and what I deferred

**Assumptions I made (to be validated):**
- Rust backend per the tech steer; trait sketches are Rust-flavoured but the *boundary* matters
  more than the language.
- Multi-tenant from day one; `tenant` on every event/principal.
- The reference graph is built from `ref.created` events (events are authoritative for edges).

**Genuinely open / uncertain (flagged ⚠ inline, collected here):**
1. **Agent vs. Service principal kinds** — one type with flags or two distinct types? Affects
   how much governance plumbing services inherit.
2. **Delegation algebra** — the exact "intersection" authz semantics for on-behalf-of runs is
   under-specified and deserves a dedicated authz design.
3. **Event sourcing vs. outbox per subsystem** — platform contract is the same, but the
   internal choice (and its replay/audit implications) is deferred to each subsystem phase.
4. **Durable-execution runtime: build vs. adopt vs. Temporal** — large decision with big cost
   and sovereignty implications (Temporal-the-service vs self-hosted vs Rust-native lib).
5. **Predicate/expression language for `EventMatcher`** — CEL/JSONLogic/custom; needs a safe,
   cheap-to-evaluate choice.
6. **MCP protocol-level compatibility** — I'm confident about the tool-surface *shape*; I have
   not verified current MCP wire specifics and would before promising compatibility.
7. **The agent runtime trait surface is provisional** — multi-turn tool-call loops, streaming,
   and context management may force revisions when `LlmAgentRuntime` is actually built. The
   plan-then-apply core should survive, but the exact `Agent::handle` signature (single call
   vs. a driven loop with intermediate read-tool results) is the most likely thing to change.
8. **GDPR erasure vs. LLM exposure** — erasing personal data that influenced a model decision
   is unresolved industry-wide; our "no fine-tuning on tenant data + erasable I/O logs"
   mitigation needs a dedicated GDPR note and legal review.
9. **EU AI Act risk classification** — high-level reasoning only; not legal advice; needs a
   compliance/legal pass, especially for issue-tracker workflows touching HR-like decisions.
10. **Loop/runaway protection** — designed defensively but unproven; wants adversarial design
    + load testing before trusting it in production.

**Deferred to later phases (out of scope for this research doc):**
- Concrete storage/transport tech for the event bus (Kafka/NATS/Postgres-logical/…).
- The LLM runtime internals (prompting, model selection, eval).
- UI/UX of the automation/trigger authoring experience (Zapier-class builder).
- Quantitative rate-limit/budget defaults.

---

## 8. Worked examples: multi-subsystem agent workflows

These trace concrete event→trigger→agent→effect chains across subsystems. Each is implementable
*today* as a deterministic `MockAgent` and *later* as an `LlmAgentRuntime` agent with zero
platform changes.

### 8.1 CI failure → triage → issue → chat → fix PR (the flagship)

1. `ci.pipeline.failed` emitted (branch `main`, repo `acme/api`).
2. Trigger `T1` matches → wakes `TriageAgent` (`run_as` a house agent, `on_behalf_of` the
   pusher; budget: 8 effects, 60s, 1 gate allowed).
3. Agent inbox event includes the failed pipeline + a read-only `ci.read_logs` result and the
   triggering commit. Agent decision:
   - `Effect::InvokeTool issue.create` → "CI failing on main: TypeError in payment handler"
     (with failing step + log excerpt).
   - `Effect::EmitReference` issue → commit (`caused_by`) and issue → pipeline run.
   - `Effect::InvokeTool chat.post` in `#acme-api-alerts`: "🔴 main is red — opened ISSUE-412,
     caused by commit `a1b2c3`. Triaging." (references render as live backlinks).
4. Platform applies effects (permissions OK, no sensitive effect → no gate), emitting
   `issue.created`, `ref.created`×2, `chat.message.posted`.
5. A second trigger `T2` on `issue.created` with label `ci-failure` wakes `FixAgent` (or the
   same agent's follow-up turn). It proposes a fix branch + `git.open_pr` — but `git.open_pr`
   on a protected repo is `sensitive`, so the effect returns `Gated`. A HITL approval card
   posts to chat: "FixAgent proposes PR #88 to fix ISSUE-412 — Approve / Edit / Reject."
6. A human approves in chat (durable workflow signal); the PR opens, `git.pr.opened` emitted,
   `ReviewerAgent` may then post an automated review. The whole chain shares one
   `correlation_id`; the audit log shows the full provenance; loop depth never exceeds the cap.

### 8.2 Spec doc edited → issue tasks updated → PM notified

1. `doc.edited` on a "Q3 Spec" knowledge doc, a requirements section changed.
2. Trigger wakes `SpecSyncAgent`. It diffs the section, finds linked issues (via reference
   graph), and proposes `issue.comment` on each affected issue ("Spec section 3.2 changed —
   acceptance criteria may need review") + a `chat.post` to the PM in `#product`.
3. Read-only doc + issue access; non-sensitive comments → applied without gate. PM sees a
   single threaded summary with live references back to the exact doc block.

### 8.3 Issue stuck (SLA at risk) → nudge + roll-up

1. `issue.sla.at_risk` (a high-priority bug unassigned for 24h).
2. Trigger wakes `SlaAgent` → `chat.post` mentioning the team lead, `issue.comment` with the
   SLA clock, and (if still unactioned after a durable timer) escalates by transitioning
   priority — a `sensitive` transition that is HITL-gated to the lead.

### 8.4 PR opened → review + doc-impact check

1. `git.pr.opened`.
2. `ReviewerAgent` posts review comments (mock: templated checklist; real later: actual code
   review). `DocImpactAgent` checks whether changed files are referenced by any knowledge docs
   (reference graph) and, if so, posts "This PR may affect DOC-77 (API reference)."

### 8.5 Chat command → cross-subsystem action

1. A user posts in chat: "@myelin open an issue from this thread and link the failing run."
2. `chat.mention.created` for an agent principal wakes `AssistantAgent` with the thread as
   context. It resolves references in the thread (the CI run), `issue.create`, `ref.create`,
   and replies in-thread. This is the "humans and agents in the same channel" UX made literal.

---

*End of research document. All trait/interface sketches are candidates for refinement in the
architecture phases; the plan-then-apply + strategy-pattern boundary and the event/trigger/
fabric/safety decomposition are the load-bearing recommendations I'd carry forward.*
