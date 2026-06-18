# Doctrine Integration Analysis — `03-agent-native-fabric.md`

> Phase: `02b-doctrine-integration`. Source doctrine:
> [`external-insights/03-agent-native-fabric.md`](../../../external-insights/03-agent-native-fabric.md)
> (canonical-status DEFAULT per `external-insights/README.md` — follow unless a written reason
> says otherwise). Planning baseline weighed:
> [`02-holistic-architecture/architecture-decisions.md`](../../02-holistic-architecture/architecture-decisions.md)
> (ADR-04, ADR-08, ADR-09, ADR-13), the Phase-1 design
> [`01-research/agent-native-design.md`](../../01-research/agent-native-design.md),
> [`02-holistic-architecture/shared-systems-overview.md`](../../02-holistic-architecture/shared-systems-overview.md)
> (§2 Event Bus + trigger engine, §6 Agent Fabric), and
> [`02-holistic-architecture/subsystems/continuous-integration.md`](../../02-holistic-architecture/subsystems/continuous-integration.md)
> (the CI↔agent sandbox seam, TE-31).
>
> **Headline:** the doctrine overwhelmingly **CONFIRMS** Myelin's agent spine — plan-then-apply,
> the strategy boundary, one trigger engine, first-class agent principals, structural loop
> protection, and HITL-in-the-tool-layer are all already committed in ADR-08. The doctrine's
> *value* is concentrated in a handful of **SHARPENS / RESOLVES-OPEN / NEW** deltas: a richer
> **four-primitive vocabulary** (Event/Signal/Automation-rule/Trigger), a **DECISION to unify the
> CI and agent sandbox** (resolves our open TE-31), a **universal reserve/settle cost gate in
> front of every run** (CI included), a **skeleton mode**, a set of **orchestrator operational
> gotchas** that bind in Phase 3 and Phase 8, and the **casual-mention auto-spawn** product call.

---

## 0. How to read this

Each row classifies one doctrine insight as **CONFIRMS** (already committed — brief validation),
**SHARPENS** (we have it weaker; the insight tightens it), **RESOLVES-OPEN** (answers a tracked
open question with a default-to-beat), **CONFLICTS** (disagrees with a committed decision — flagged
honestly), or **NEW** (net-new). Every non-CONFIRMS row names the integration **ACTION** and
**WHERE IT BINDS**.

---

## 1. Section-by-section classification

### §1 — Two strategy boundaries: the brain and the hands; skeleton mode

| # | Insight | Class | Where it binds & action |
|---|---|---|---|
| 1.1 | The mock→real swap is **a single trait implementation**; strategy boundary kept minimal | **CONFIRMS** | ADR-08.2 + `agent-native-design.md §4.1`: the `AgentRuntime`/`Agent` trait set, `MockAgentRuntime` now / `LlmAgentRuntime` later, swap = repoint `runtime_ref`. No action. |
| 1.2 | **Brain = one method** `step(conversation) -> {use_tools \| submit}`; **agent loop owns conversation history, provider is stateless** | **SHARPENS** | Phase 3 (Agent Fabric). Our `Agent::handle(event, ctx) -> AgentDecision` (AG-3) leaves the single-call-vs-driven-loop question explicitly open. The doctrine resolves the *shape*: a **stateless provider** with a one-method `step`, history owned by the platform-side loop. **Action:** hand Phase 3 the default-to-beat — *provider trait is stateless `step`; the agent loop (platform code) owns history* — and reconcile it with the provisional `Agent::handle` signature so plan-then-apply survives. This is the concrete answer AG-3 was waiting for. |
| 1.3 | Mock provider replays a **scripted queue of steps**, used by **both unit tests and a `--use-mock` runtime flag** (same path users hit) | **SHARPENS** | Phase 3 (Agent Fabric) + Phase 8 (execution). We have a deterministic rule-driven mock (`agent-native-design.md §4.5`) but never said it doubles as a **shipped runtime flag on the same code path**. **Action:** make `--use-mock` (or equivalent runtime config) a first-class product/runtime path, not just a test harness — the dogfooding/demo lever (VISION §3 "mock implementations"). Note it in the Agent Fabric CLI surface (`myelin agent runtime set` already exists; add the per-invocation mock flag). |
| 1.4 | The real provider is the **only** place a model vendor is named; carries **attribution fields (tenant, actor, run id, caused-by)** so every call is traceable/metered | **CONFIRMS** | ADR-08.2 ("no LLM SDK/prompt/model name anywhere in platform code") + ADR-13.2 envelope (`actor`, `causation_id`, `correlation_id`) + ADR-12.9 audit. No action. |
| 1.5 | **The hands = one method** `exec(command) -> result` with **no host-execution path that bypasses it**; simulation impl runs in-process with a marker proving it went through the channel | **SHARPENS** | Phase 3 (Agent Fabric) + Phase 5 (testing). Our `ToolSurface`/`EffectApi` is plan-then-apply, but "**no host-execution path bypasses the trait**" is stated more weakly. **Action:** elevate this to an **architecture-test/lint obligation** (sibling to ADR-01's no-cross-DB lint): the only path to side effects is `EffectApi`/the tool-exec trait; a simulation impl must emit a **channel-proof marker**. Bind as a Phase-5 testing invariant + a Phase-3 trait contract note. |
| 1.6 | **Skeleton mode** — no model, no tools: authenticate, fetch task, print summary, exit; verifies the whole gateway/identity path with zero model spend | **NEW** | Phase 3 (Agent Fabric) + Phase 6 (roadmap sequencing) + Phase 8. We have *mock* (no model, but does emit effects) and *real*; we do **not** have an even-thinner skeleton that proves the identity/gateway/dispatch wiring with **zero effects and zero spend**. **Action:** add **skeleton mode** as the *first* runtime to stand up (roadmap: skeleton → mock → real), and as the smoke-test for the dispatch path. Add to `myelin-agent` runtime enum. |
| 1.7 | Payoff: **prove the whole agent story on the mock brain first** (pick up → act → caused-by chained → trace queryable); replacing one trait lights up the agent-first story; the thing under test is the **wiring and the sandbox, not model spend** | **CONFIRMS** | ADR-08.3/§Rationale (plan-then-apply makes the mock loop deterministically testable; golden tests + `cargo-mutants`). VISION §3. No action beyond 1.6's sequencing note. |

### §2 — Four distinct primitives (Event / Signal / Automation-rule / Trigger)

| # | Insight | Class | Where it binds & action |
|---|---|---|---|
| 2.1 | **Event** = a fact ("X happened"), every state change, fired over the durable log via the outbox | **CONFIRMS** | ADR-04.3 (transactional-outbox-by-default) + ADR-13.2 (the envelope). No action. |
| 2.2 | **Signal** = a **curated, deduplicated, severity-ranked subset of events** actors should actually react to; "don't make everything react to everything" — the trigger substrate | **SHARPENS** (the key vocabulary delta) | Phase 3 (Event Bus / trigger engine) — **new ADR or design-language augmentation**. Our model has Event → `EventMatcher` (filter) → Subscription/Automation/Agent (ADR-04.5, ADR-08.5, overview §2.2). We have **no first-class *Signal* tier** — the curated/deduped/severity-ranked subset sitting *between* the raw event firehose and reactive consumers. This is exactly the upstream defence against §6.1's head-of-line blocking. **Action:** **back-patch the trigger-engine model to name Signal as a distinct tier** (curate + dedup + severity-rank before any matcher runs). Cleanest as a Phase-2 ADR addendum to ADR-04/ADR-08 ("the four reactive primitives"), implemented in Phase 3 Event Bus. *Default-to-beat for Phase 3:* a consumer subscribes to **Signals**, not raw Events, unless it is an infra consumer (indexer/refs-builder) that genuinely needs the firehose. |
| 2.3 | **Automation rule** = a *reflex the project owns*: "when X, do Y." **Stateless, per-event.** | **SHARPENS** | Phase 3 (Event Bus / trigger engine). We have "Automations" but bundle the durable-workflow framing into them (overview §2.2 calls Automations "durable multi-step workflows on the ADR-09 substrate"). The doctrine **distinguishes stateless per-event reflexes** (automation rule) **from stateful promises** (trigger, below). **Action:** in the §2.2 back-patch, split our single "Automation" notion into **(a) stateless automation rule** (reflex, per-event, no durable state) and **(b) the durable workflow** it may *invoke* — the durable-execution substrate (ADR-09) is for the *multi-step/HITL* case, not every reflex. Avoids over-using durable execution for trivial reflexes. |
| 2.4 | **Trigger** = a *stateful promise a person owns*: "wait until condition C, then unblock this task." A small **state machine (armed → resolved / stale / disarmed)**, fires **once per arming** | **SHARPENS** | Phase 3 (Event Bus / trigger engine) + Phase 4 (Issues — task-unblock UX). Our `Trigger` (ADR-08.5, `agent-native-design.md §3.1`) is the **binding of matcher→target** — a *routing* concept, not a *stateful per-person promise with a lifecycle*. The doctrine's "Trigger" is a narrower, richer primitive. **Action:** in the back-patch, **rename/disambiguate**: our current "Trigger" binding object becomes (e.g.) a *subscription/automation binding*, and adopt the doctrine's **stateful Trigger** (armed→resolved/stale/disarmed, fires-once) as a distinct user-facing primitive (the "remind/unblock me when…" promise). This is a genuine vocabulary collision worth fixing now to prevent two meanings of "trigger" downstream. |
| 2.5 | One-liner: *a trigger is a promise the system keeps for you; an automation rule is a reflex the project has* | **SHARPENS** | Design-language augmentation (Phase 2 / Phase 3 admin UX). **Action:** adopt this framing verbatim in the trigger/automation **authoring UX** copy (the Zapier-class builder, overview §2.5) so the product surfaces the distinction to users. |

> **Net of §2:** this is the single most important conceptual delta in the doc. We collapsed
> Event/Signal/Automation/Trigger into "event → matcher → {subscription, automation, agent}". The
> doctrine's four-primitive split is **strictly richer** and directly motivates the §6.1 fix
> (subscribe to curated Signals, not the raw firehose). Recommended: a **Phase-2 back-patch ADR**
> ("ADR-08b / ADR-04b: the four reactive primitives") rather than silently folding it into Phase 3,
> because it changes shared vocabulary every downstream phase uses.

### §3 — One sandbox for CI **and** agents

| # | Insight | Class | Where it binds & action |
|---|---|---|---|
| 3.1 | CI steps and agent tool calls are the **same problem (running untrusted code)** — build **one** isolation primitive and harden it once; **a single job spec with a `kind` field (ci \| agent)** feeds the same runner | **RESOLVES-OPEN (TE-31)** | Phase 3 (Agent Fabric) + Phase 4 (CI), **jointly**. ADR-08 §Consequences and CI §8.9 flag CI↔agent substrate unification as **`[OPEN → P4 (CI)+P3]` TE-31** ("flagged, not decided"). The doctrine **decides it**: *unify*, via one runtime-agnostic **job spec with a `kind` field**. **Action:** hand TE-31 the **default-to-beat = UNIFY** (one sandbox, one job spec, `kind ∈ {ci, agent}`). Phase 4 CI must justify in writing if it *diverges* from unification (inverts the current "prove it's worth unifying" burden). Keeps the backend swappable behind the job spec. |
| 3.2 | Settled building block: **userspace-kernel sandbox (gVisor-class) or microVM**; plain containers share the host kernel — one escape is a cross-tenant catastrophe; keep backend swappable | **CONFIRMS / SHARPENS** | Phase 4 (CI) TE-28. CI already commits "**microVM-class default for untrusted**, pluggable executor strategy" (CI §2.2, §3, TE-28 "leaning microVM"). The doctrine **broadens the acceptable floor to "gVisor-class userspace-kernel *or* microVM"** and reframes it as the **shared** agent+CI boundary. **Action:** carry into TE-28 that the chosen isolation must serve **both** kinds (per 3.1) and that plain shared-kernel containers are **rejected by default** for untrusted code. |
| 3.3 | Defaults: **no host network (egress default-deny, allowlist opt-in), read-only root + tmpfs, all caps dropped, no-new-privileges, seccomp, images pinned by digest (reject tag-without-digest, fail-closed), whole-guest kill on teardown, cgroup limits incl. `pids.max` + zero swap** | **SHARPENS** (concrete hardening checklist) | Phase 4 (CI) TE-28 threat model + Phase 3 (Agent Fabric). CI has egress control, trust-tiers, ephemeral one-job-per-sandbox, and pin-by-digest for *components* (CI §2.2–2.4), but **no consolidated hardening default-set**. **Action:** adopt this as the **named default sandbox hardening profile** in the TE-28 threat model — most are settled industry practice, so deviations must be written down. Notably **fail-closed on un-digested image tags** and **`pids.max` fork-bomb ceiling** are specifics our CI doc doesn't yet name. |
| 3.4 | **Secrets by name only, resolved *inside* the boundary** per run, scoped to exactly this job's references; never baked into images; never handed to the agent runtime to forward | **CONFIRMS / SHARPENS** | Phase 4 (CI) §2.3 + Phase 3 (Agent Fabric). CI commits OIDC short-lived scoped creds, secret-by-tier, masked-in-logs (CI §2.3, §7.3). The doctrine **sharpens the resolution boundary**: resolve **inside** the sandbox, scoped to *this job's references*, and — critically for agents — **never hand a secret to the agent runtime to forward**. **Action:** add the "resolve-inside-boundary, never-forwarded-via-agent-runtime" rule to the shared secret-brokering capability (CI §8.7 flags it for Phase 3 placement under Id/GDPR). |
| 3.5 | Untrusted execution is a **permanent target**; one escape is catastrophic; **an undrilled security property is a claim, not a fact** — the **escape drill on a real kernel is the single hard blocker before anything runs customer code** | **SHARPENS / NEW (the gate)** | Phase 5 (testing) + Phase 6 (roadmap sequencing) + Phase 8 (execution discipline). We treat isolation as a design choice (TE-28) but never named a **drill-on-real-hardware gate** as the *blocker before customer code runs*. This is the README's "name your floors / untested is a claim" honesty rule applied to the sharpest edge. **Action:** make the **sandbox-escape drill a Phase-5 testing-strategy gate and a Phase-6 roadmap milestone** that **must pass before any run executes untrusted customer code** (CI *or* agent). Flag as an explicit go/no-go in Phase 8. |

### §4 — Agents act through the same gateway as humans (no carve-out)

| # | Insight | Class | Where it binds & action |
|---|---|---|---|
| 4.1 | Agent write tools call the **same public endpoints a human uses**, carrying the run's scoped token; existing authz check runs unchanged | **CONFIRMS** | ADR-08.1/.3/.4 + ADR-13.3 (one `Principal`, one policy engine, humans/agents/services identically) + overview §6.4 ("subsystems expose mutations *as tools*, not agent-callable back-doors"). No action. |
| 4.2 | A `403`/`503` surfaces to the agent loop as an **ordinary tool error — never an escalation to a privileged path**; an agent can do nothing its identity is not permitted to do | **SHARPENS** | Phase 3 (Agent Fabric). Implied by plan-then-apply but never stated as an explicit invariant. **Action:** record the **no-escalation-on-denial** rule as an Agent Fabric design invariant (denied effect → `Denied` tool error returned to the loop; there is no privileged fallback). Cheap, prevents a classic footgun. |
| 4.3 | **Mint per-run identity at dispatch; unset any shared platform token in the child env** so it can't leak in as the tool identity; **revoke on teardown even on crash** (idempotent cleanup hook) | **SHARPENS** | Phase 3 (Agent Fabric + Id) + Phase 4 (CI). We commit short-lived scoped per-run tokens (CI §7.3; overview §1.4 intersection). The doctrine adds two **operational** musts: **(a) scrub the parent/platform token from the child environment** (anti-leak), and **(b) idempotent revoke-on-teardown even on crash**. **Action:** add both to the per-run token lifecycle spec (Phase 3 Id token model, CI §8.3 token requirement). |
| 4.4 | Reuse existing substrate for agent artifacts: an agent's **execution trace is just a document** in the knowledge subsystem (content-addressed, immutable) — saves a whole schema + projection | **NEW** | Phase 3 (Agent Fabric) + Phase 4 (Knowledge). We have the audit log (ADR-12.9) and treat traces as audit records, but never said the **rich human-readable trace = a content-addressed Knowledge document** (reusing `myelin-content`, ADR-05). This is a reuse win and a UX win (traces become referenceable artifacts via `ArtifactRef`). **Action:** adopt as the **default representation for agent execution traces** — a content-addressed immutable Knowledge doc — distinct from (and complementary to) the tamper-evident audit log. Bind in Phase 3 Agent Fabric; flag the dependency to Phase 4 Knowledge. *(Note: must remain a `PersonalDataHolder` / erasure-aware per ADR-12.)* |

### §5 — Safety: approval, cost, loops, storms

| # | Insight | Class | Where it binds & action |
|---|---|---|---|
| 5.1 | **HITL approval lives in the tool layer.** A gated write tool whose name is in "requires approval" but not "approved" is **withheld — returns an error, does not mutate**; approval re-runs the step with the name added. Approval UI shows pending action + risk + **live cost estimate**. **Wire approve→resume end to end** (easy to ship withhold + card but forget the bridge) | **CONFIRMS / SHARPENS** | Phase 3 (Agent Fabric) + Phase 4 (Chat HITL surface) + Phase 8. ADR-08.6 + ADR-09 + overview §6.2 commit HITL gates as durable workflow waits surfaced as chat approval cards. The doctrine **sharpens the mechanism** (gate = tool **withheld** unless name in "approved"; approval *re-runs the step*) and flags the **easy-to-miss approve→resume bridge** and the **live cost estimate on the card**. **Action:** (a) record the tool-layer withhold/re-run mechanism as the Phase-3 HITL design default; (b) add **"approve→resume bridge is wired end-to-end"** as a Phase-5 test + Phase-8 execution checklist item; (c) add **live cost estimate** to the approval card spec (ties to §5.2). |
| 5.2 | **Cost pre-flight makes a runaway loop self-limiting.** Before a real-spend run, check prepaid balance + per-capability add-on; **refuse to *start* when exhausted (never interrupt one in flight)**. Meter **one cost event per model call**, wholesale and markup kept separate. **Put a universal reserve/settle gate in front of EVERY kind of run (CI included)** — "no balance → no execution" uniformly true | **SHARPENS / NEW (the universal gate)** | Phase 3 (Agent Fabric + a billing/metering capability) + Phase 4 (CI) + **Commercial**. We have per-run/agent/tenant **budgets** (ADR-08.6, `agent-native-design.md §5.2`) and CI metering is flagged (TE-32), but we have **no platform-wide reserve/settle gate** and never unified CI + agent spend under one "no balance → no execution" rule. The doctrine's **reserve/settle-in-front-of-every-run** is net-new and ties CI metering (TE-32) and agent budgets into one substrate. **Action:** (1) add a **universal reserve/settle cost gate** as a shared capability that *both* CI runs and agent runs pass through before starting (resolves part of TE-32 with a default-to-beat: *reserve at dispatch, settle on completion, refuse-start-on-exhaustion, never-interrupt-in-flight*); (2) **meter one cost event per model call, wholesale ≠ markup** (Commercial / pricing-history-immutability concern); (3) bind the gate decision as a Phase-2 back-patch note to ADR-08.6 (budgets) generalised to *all runs*, designed in Phase 3, with Commercial owning the pricing/wallet model. |
| 5.3 | **Loop prevention is structural**: self-guard (skip the agent's own output), a **reference gate** (raw typed text must not re-trigger — only a structured picker-produced reference can), a **causal-depth ceiling**, a **shared-root tripwire** | **CONFIRMS / SHARPENS** | Phase 3 (Agent Fabric) + Phase 5 (adversarial testing, AG-4). ADR-08.6 + overview §6.6 commit `causation_id` depth caps + cycle detection + idempotent tools + per-tenant circuit breakers. The doctrine adds two **specific, named** structural guards we don't enumerate: **(a) self-guard** (skip the agent's own output) and **(b) the reference gate** (only a *structured* reference, not raw typed text, can re-trigger — ties directly to ADR-05's `artifact_ref`/`mention` being structured content nodes). **Action:** add **self-guard** and the **reference gate** to the loop-protection inventory; the reference gate is a clean fit with ADR-05 (only picker-produced `artifact_ref` nodes emit `ref.created`, not free text). Bind as Phase-3 design + Phase-5 adversarial-validation cases (AG-4). |
| 5.4 | **Concurrency caps** bound a mention storm: a **bounded worker pool drops over-cap dispatches** rather than forking unboundedly | **SHARPENS** | Phase 3 (Agent Fabric / Event Bus dispatch). We have per-tenant circuit breakers + budgets (ADR-08.6) but not the specific **bounded-worker-pool-drops-over-cap** mechanism for mention storms. **Action:** name the bounded dispatch worker pool (drop, don't fork) as the storm-control default in the orchestrator/dispatch design (Phase 3). Ties to §7's casual-mention concern. |

### §6 — Orchestrator gotchas (the reactive automation tier)

> The doctrine's framing — *the reactive automation tier is a **stateful exception** that needs an
> explicit design* — is itself a **SHARPENS**: our planning treats the trigger engine as a thin
> layer on the bus (overview §2.2) and does not flag it as the one stateful, failure-prone
> component that warrants a dedicated design. **Action:** add a Phase-3 note that the
> orchestrator/dispatch tier gets an **explicit, separately-reviewed design**, not folded into the
> bus.

| # | Gotcha | Class | Where it binds & action |
|---|---|---|---|
| 6.1 | **Over-broad subscription head-of-line-blocks everything** — one durable consumer subscribed to *all* events accumulates tens of millions of unhandled messages behind unhandled types and silently stalls every real-time agent feature. **Whitelist the subjects you handle; monitor consumer lag (pending count).** | **NEW (operational discipline)** | Phase 3 (Event Bus consumer design) + Phase 8 (execution / ops). Not in our planning. Directly motivates §2.2's **Signal** tier (subscribe to curated subjects, not the firehose). **Action:** (a) **whitelist-subjects-only** as a Phase-3 consumer-design rule (no consumer subscribes to `*`); (b) **consumer-lag/pending-count monitoring** as a Phase-8 ops gate and a Phase-3 observability requirement. This is the concrete failure the Signal vocabulary prevents. |
| 6.2 | **A durable consumer's start policy is immutable** — re-asserting start position on every reconnect can wedge the broker and stop delivering *all* events. **Bind to an existing durable consumer by name; never re-declare its start policy on reconnect.** | **NEW (operational discipline)** | Phase 3 (Event Bus consumer template, `myelin-events`) + Phase 8. Not in our planning. **Action:** bake **"bind-by-name, never re-declare start policy on reconnect"** into the shared consumer template in `myelin-events` (the same template that already enforces idempotency-on-`event_id`, overview §2.2). One place to get it right for every subsystem. |
| 6.3 | **Acknowledge only after the work is enqueued** (at-least-once to subscribers); **terminate non-retryable messages (malformed bytes) immediately** rather than burning the redelivery budget | **SHARPENS** | Phase 3 (Event Bus consumer template). ADR-04.1 commits at-least-once + idempotent, but **ack-after-enqueue ordering** and **poison-message termination** are not spelled out. **Action:** add both to the `myelin-events` consumer template contract: **ack only after downstream enqueue**, and **dead-letter/terminate malformed (non-retryable) messages** instead of redelivering. Ties to the existing dead-letter/replay tooling (overview §2.5). |
| 6.4 | **Carry causality through the dispatch path** so an agent action is attributable to the original human action — and thread it **nested, not flat**, or the "why" chain collapses to a single hop | **CONFIRMS / SHARPENS** | ADR-13.2 (`causation_id` + `correlation_id`) + ADR-08.6 (depth caps) commit causal threading. The doctrine **sharpens one detail**: thread `causation_id` **nested** (parent→child chain), **not flat** (everything → the root), or depth-capping and provenance both break. **Action:** record **"`causation_id` is the immediate parent, nested — not the root"** as an explicit envelope-semantics note (Phase-3 taxonomy work, TE-10); it's load-bearing for both loop-depth accounting and audit provenance. |

### §7 — Product judgment the platform can't make

| # | Insight | Class | Where it binds & action |
|---|---|---|---|
| 7.1 | **Should a casual mention auto-spawn an autonomous, potentially costly run?** Getting it wrong is a real cost + UX regression. Ship the **explicit "run an agent here" action first**; treat implicit auto-dispatch as a **deliberate, separately-decided feature** with intent/cost detection | **NEW (product decision)** | **Commercial / Product** + Phase 4 (Chat) + Phase 6 (roadmap sequencing). Our flagship walkthroughs (`agent-native-design.md §8.5`, CI §6.2) casually assume `chat.mention.created` for an agent principal **auto-wakes** an agent. The doctrine flags this as a **product/cost call, not an engineering default**. **Action:** make **explicit "run an agent here" the Phase-1-build default**; gate **implicit auto-dispatch-on-casual-mention** behind a deliberate product decision (intent + cost detection). Carry as a **Commercial/Product open item** and a Phase-6 sequencing note (explicit action first; auto-dispatch later/optional). |
| 7.2 | Keep these as **plan-and-sign-off items (process doctrine §8), not 3am autonomous builds** | **NEW (process discipline)** | Phase 8 (execution discipline) + Legal/DPO. **Action:** route auto-dispatch (and any "agent acts without an explicit human ask" behaviour) through an explicit plan-and-sign-off gate; it also intersects GDPR Art. 22 / AI-Act human-oversight (ADR-08.6, GD-9) — flag for DPO when it binds. |

---

## 2. Genuine conflicts

**None.** The only friction is **vocabulary** (§2): our single "Trigger = matcher→target binding"
collides with the doctrine's narrower "Trigger = stateful per-person promise," and we lack the
"Signal" and "stateless automation rule" tiers. This is a **SHARPENS to resolve by renaming/adding
primitives**, not a CONFLICT — nothing committed is *wrong*, it's *coarser than it should be*.
Recommended resolution is the Phase-2 back-patch ADR in §3 below.

---

## 3. Prioritized deltas (the 5–8 that matter)

1. **The four-primitive vocabulary — Event / Signal / Automation-rule / Trigger (§2).**
   *SHARPENS, the headline.* We collapsed these; the split is strictly richer and motivates the
   §6.1 head-of-line fix. **Binds:** Phase-2 **back-patch ADR** (addendum to ADR-04/ADR-08, "the
   four reactive primitives"); implemented Phase 3 Event Bus; resolves the "Trigger" name collision.
   *Default-to-beat:* consumers subscribe to **curated Signals**, not raw Events.

2. **UNIFY the CI and agent sandbox — resolves TE-31 (§3).**
   *RESOLVES-OPEN.* The doctrine turns our open "should we unify?" into a **decision: yes, one job
   spec with `kind ∈ {ci, agent}`, one hardened runner.** **Binds:** Phase 4 (CI) + Phase 3
   (Agents), joint. *Default-to-beat = UNIFY*; CI must justify divergence in writing.

3. **Universal reserve/settle cost gate in front of *every* run, CI included (§5.2).**
   *NEW.* "No balance → no execution," uniformly; meter one cost event per model call, wholesale ≠
   markup. Unifies agent budgets (ADR-08.6) and CI metering (TE-32). **Binds:** Phase 3 (shared
   metering capability) + Phase 4 (CI) + **Commercial** (wallet/pricing) + Phase-2 back-patch note
   to ADR-08.6.

4. **Orchestrator operational gotchas — whitelist subjects + monitor lag; bind-by-name (immutable
   start policy); ack-after-enqueue + poison-termination; nested (not flat) causality (§6).**
   *NEW + SHARPENS.* Concrete, expensive traps; cheap to design around now. **Binds:** Phase 3
   (the `myelin-events` consumer template + an explicitly-designed reactive/dispatch tier) +
   Phase 8 (consumer-lag ops gate).

5. **Sandbox-escape drill on a real kernel as the single hard gate before any customer code runs
   (§3.5).**
   *NEW (the honesty-rule gate).* An undrilled isolation property is a claim, not a fact.
   **Binds:** Phase 5 (testing strategy) + Phase 6 (roadmap milestone) + Phase 8 (go/no-go).

6. **Skeleton mode + mock-as-shipped-runtime-flag + stateless `step` provider (§1).**
   *NEW + SHARPENS.* Skeleton (no model/no tools, proves the gateway path, zero spend) is the
   *first* runtime to build; `--use-mock` is a real runtime path, not just tests; the provider is a
   stateless one-method `step` with the platform owning history (answers AG-3). **Binds:** Phase 3
   (Agent Fabric) + Phase 6 (roadmap: skeleton → mock → real) + Phase 8.

7. **Casual-mention auto-spawn is a product/cost decision, not a default (§7).**
   *NEW (product call).* Ship explicit "run an agent here" first; gate implicit auto-dispatch
   behind a deliberate, intent/cost-aware decision. Corrects an assumption baked into our flagship
   walkthroughs. **Binds:** **Commercial/Product** + Phase 4 (Chat) + Phase 6 + Legal/DPO (Art. 22).

8. **Tool-layer safety sharpenings — no-host-execution-bypass lint (§1.5), no-escalation-on-403
   (§4.2), scrub-parent-token + idempotent-revoke (§4.3), self-guard + reference-gate loop guards
   (§5.3), agent-trace-as-Knowledge-doc reuse (§4.4), wire-the-approve→resume-bridge (§5.1).**
   *Mostly SHARPENS, two NEW.* Individually small, collectively the difference between a sound spine
   and a safe one. **Binds:** Phase 3 (Agent Fabric design invariants) + Phase 5 (the bypass-lint,
   approve→resume, and loop-guard adversarial tests) + Phase 4 (Knowledge, for the trace doc).

---

## 4. Digest (top deltas → where they bind)

- **Four reactive primitives (Event/Signal/Automation-rule/Trigger)** — *SHARPENS* →
  **Phase-2 back-patch ADR** + Phase 3 Event Bus. (Resolves the "Trigger" name collision; Signals
  are the upstream fix for §6.1.)
- **Unify CI + agent sandbox (one job spec, `kind` field)** — *RESOLVES-OPEN TE-31* →
  **Phase 4 (CI) + Phase 3 (Agents), default-to-beat = UNIFY.**
- **Universal reserve/settle cost gate before every run (CI + agents)** — *NEW* →
  **Phase 3 (metering capability) + Phase 4 (CI) + Commercial** (resolves part of TE-32).
- **Orchestrator gotchas (whitelist+lag, bind-by-name, ack-after-enqueue, nested causality)** —
  *NEW/SHARPENS* → **Phase 3 `myelin-events` consumer template + Phase 8 ops gate.**
- **Sandbox-escape drill = hard gate before customer code runs** — *NEW* →
  **Phase 5 testing + Phase 6 milestone + Phase 8 go/no-go.**
- **Skeleton mode + stateless `step` provider + mock-as-runtime-flag** — *NEW/SHARPENS (answers
  AG-3)* → **Phase 3 Agent Fabric + Phase 6 roadmap sequencing.**
- **Casual-mention auto-spawn is a product decision** — *NEW* →
  **Commercial/Product + Phase 4 (Chat) + Legal/DPO (Art. 22).**
- **No genuine CONFLICTS.** The bulk of the doc **CONFIRMS** ADR-08/ADR-04/ADR-13 (plan-then-apply,
  strategy boundary, one trigger engine, first-class agent principals, structural loop protection,
  HITL-in-the-tool-layer) — validation, not work.
