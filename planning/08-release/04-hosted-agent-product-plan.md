# Hosted Agent — product vision & productionization plan

> Status: draft (2026-08-04). Scope: the **hosted-agent offering** — Myelin runs Claude/GPT
> agents for users over their own software org, billed by token usage, taking a platform cut.
> This is a *product/UX + roadmap* doc; the fabric *architecture* is already covered in
> `planning/0{3,5}-shared-systems-architecture/agent-fabric.md` and `01-research/agent-native-design.md`.
> Grounding: the Agent Fabric (`crates/myelin-agent{,-service}`, `myelin-mcp`, sandbox `agent_job()`)
> already exists; this plan productionizes it. See the four named-missing pieces in §4.

## 1. North star — what makes this a joy to use, and unsurprising

**The pitch:** *a zero-setup software org where you can hand work to an agent, watch it work,
and pay only for the tokens it uses.* The moat is that the agent lives **inside** the substrate —
it can read your repos, issues, CI results, chat, and docs, and produce **real artifacts** (a PR,
an issue comment, a doc), not chat text to copy-paste. A standalone coding assistant is
context-starved; an agent native to the org is not.

"Unsurprising to **both** me (operator) and them (users)" is the design constraint, and it splits
cleanly:

- **Unsurprising to users:**
  - *You always see the cost.* A live token meter ticks up during the run; each run ends with a
    plain "this run cost $0.031." A per-run hard cap you set means a run can never overspend.
  - *You always see what it's doing.* The run is a live transcript — every thought, tool-call, and
    proposed change streams as it happens. No black box.
  - *It asks before doing anything risky.* Reads/investigation just happen. **Mutations** (merge,
    deploy, delete, push) surface as **approval cards** you accept or reject. You are never
    surprised by an action the agent took.
  - *The output is real and reviewable.* A PR you can read, an issue comment, a doc — landed in
    your org through the same governed paths a human uses.
- **Unsurprising to the operator (you):**
  - *Margin is structural.* Users pay provider-token-cost × (1 + platform cut). You are never
    underwater on inference; the cut is the business.
  - *No runaway spend.* Every run reserves against a prepaid wallet before it starts (no
    balance → no run) and settles to actual usage; a hostile or buggy agent burns only its
    reservation, capped.
  - *Safe to hand to strangers.* Sandbox isolation (default-deny egress), tenant RLS, an
    append-only audit of every effect, and a hard lint that keeps model SDKs out of platform
    code. This is what makes a public, self-serve launch to untrusted users survivable day one.

## 2. Billing model — token-based, prepaid wallet

Decided (founder, 2026-08-03/04): **bill in tokens, prepaid.** Rationale: tokens are exactly what
the platform pays the provider, so pass-through-plus-margin is the most honest and least surprising
model, and it's the unit developers already understand.

- **The wallet is a provider-agnostic credit ledger.** Top-ups *credit* it (a payment concern);
  usage *debits* it (a metering concern). The two never know each other's mechanism — so
  Stripe → an EU-sovereign provider, or adding auto-top-off, never touches the metering path.
- **Metering (already modeled in the cost ledger):** each model call's `wholesale` = token cost at
  provider rates (input / cached-input / output tiers), `markup` = `round(wholesale × cut)` (~2%).
  Both are debited from the wallet at settle.
- **Micro-unit precision (required).** A ~2% cut on a sub-cent Luna call rounds to 0 in integer
  cents. Meter `wholesale`/`markup` in **micro-units** (`u64`, cf. ovim's `amount_micros`) so small
  margins are representable and never silently dropped.
- **Never fabricate usage.** Cost derives from the provider's `usage` block. If a provider omits it,
  record `NotReported` and **fail the run closed** rather than estimate (ovim's
  `AgentReported<T> = Reported(T) | NotReported` discipline). Reconciliation against provider
  invoices is a later back-office concern.
- **Reserve → settle is the runaway guard.** Dispatch reserves an upper-bound estimate against
  `available = balance − Σ outstanding reservations`; per-call metering accumulates; a **pre-call
  cap check** halts the loop before any over-budget paid call; settle clamps `billed ≤ reserved` and
  writes an immutable wallet debit. A per-call `max_tokens` ceiling bounds single-call overshoot.

## 3. The safety & observability model (the "unsurprising" machinery)

These already exist in the Fabric; the product surfaces them:

- **Plan-then-apply.** The brain never executes a mutation — it emits a *proposed effect* that runs
  the `EffectApi` pipeline (schema → capability → delegation → tenant → budget → HITL → apply →
  meter). Structurally, no tool output can mutate state. Prompt-injection's blast radius is bounded
  to *wasted budget* (wallet-capped) or a *proposed, gated* effect (HITL-stopped).
- **HITL approval cards** for anything `requires_approval` — the user's accept/reject is a durable,
  server-side verdict.
- **Sandboxed compute.** Code-execution tools run in the gVisor `agent_job()` sandbox
  (default-deny egress, digest-pinned image, zero-swap, gated by a green escape attestation) — the
  same isolation proven by CT-007.
- **Event-sourced trace = the live view.** The run owns a content-addressed, residency-pinned
  `trace`; borrowing ovim's model, it is an append-only event log keyed by an authoritative
  `sequence`, and `events_after(after_sequence, limit)` is the one tail/observe verb the UI (and
  the API) reads to render "watch it work." Live state is always a projection of the log.
- **Tenant isolation + audit.** Every row leads with `(tenant, region)` under FORCE RLS; an agent
  principal authorizes through the *identical* fail-closed ReBAC path as a human; every tool call is
  audited through the transactional outbox.

## 4. From here to there — the productionization gap

The Fabric is built; four specific pieces are not (verified 2026-08-03, see
`memory/agent-service-plan.md`):

1. **`myelin-agent-model`** (new crate) — the real vendor brain: `LlmAgentRuntime: AgentRuntime`
   + a `ModelClient` trait (`LunaClient`, `AnthropicClient`). The *only* place a model SDK/prompt
   may appear (the single `no-llm-in-platform` lint exception; a whole-crate boundary).
2. **The multi-turn loop body** in `handle_run` (`UseTools → route → append → step again`), today a
   documented stub.
3. **Real per-call token metering** — productionize the spike's `cost_of` into `wholesale`/`markup`
   (micro-units).
4. **The durable prepaid wallet** — `agent_wallet` + an append-only immutable `agent_wallet_ledger`.
   Today the balance is a *parameter with no backing table*. This is the critical-path gap (not
   Stripe).

## 5. Roadmap — thin end-to-end slices, built from the end goal

Each slice is a **complete vertical** (a usable increment), not a horizontal layer. Scope is
discovered by widening, not by front-loading.

- **Slice 1 — the walking skeleton (the whole loop, thinnest).**
  A hosted **Luna** session runs against one real tenant repo: reads context via a governed **read**
  tool, and drives ONE **mutating** tool (propose an **issue comment**) through the full
  `EffectApi` + **HITL approval** path to land a real artifact. Live token metering in micro-units
  debits the **durable wallet** with a hard cap; the run is persisted and tailable via
  `events_after`. Deferred: Stripe (seed the wallet via an internal admin path), Anthropic, code
  execution, the UI. *This touches every pillar once* — governed tools, plan-then-apply + HITL,
  metering, wallet, live trace — and is the smallest thing that is recognizably the product.
- **Slice 2 — breadth + the second vendor.** Widen the tool set (more read + low-risk mutating
  tools: PR open, doc write); add `AnthropicClient` to validate the `ModelClient` abstraction isn't
  Luna-shaped (prove it early — the two wire protocols differ: Luna needs `reasoning_effort:"none"`
  on chat/completions, Anthropic uses native Messages tool-use).
- **Slice 3 — real money in.** Stripe top-up (wallet fill) + the billing surface (balance, history,
  low-balance nudge); optional auto-top-off. Metering is untouched (the decoupling pays off).
- **Slice 4 — code execution.** Compute/`External` tools via the gVisor `agent_job()` sandbox
  (requires a green escape attestation for the agent backend) — the agent can now run tests, build,
  reproduce. This is where the CT-007 isolation moat directly becomes agent capability.
- **Slice 5 — self-serve + UI + launch hardening.** Signup/OIDC-SSO, the run view (transcript +
  live meter + approval cards) and launch points ("assign this issue to an agent", "ask an agent to
  review this PR"), abuse/cost controls for public untrusted users, and an independent pentest
  before the X → HN launch.

## 6. Open decisions carried

- **EU sovereignty vs US model APIs** — Luna/Anthropic are US sub-processors reached over egress
  while `Region` pins data to `fr-par`. *Resolved for v1 dogfooding on our own repo (US APIs OK,
  no external tenant PII);* it is a legal/posture gate before agents touch *other tenants'* data at
  public launch (US-under-DPA vs an EU-hosted model).
- **Key custody + egress** — `LlmAgentRuntime` runs in the platform process (not the sandbox), so
  the platform tier holds the provider API key and performs outbound egress (as the spike already
  does via `fed`). Needs an explicit key-handling + egress-allowlist decision for that tier.
- **What "an agent" is launched from** in the end-state UX — an issue, a PR, a chat mention, a bare
  prompt — shapes the entry points in Slice 5.
