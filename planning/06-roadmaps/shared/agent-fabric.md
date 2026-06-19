# Phase 6 — Roadmap: Agent Fabric (`myelin-agent`)

> Phase: `06-roadmaps/shared`. The detailed sequenced roadmap for the **agent-fabric** shared system. Slots
> into the master sequencing bands M0..M6:
> [`../00-master-sequencing.md`](../00-master-sequencing.md) (§2 bands, §3 critical-path/DAG, §4 gate
> invariant, §5 name-your-floors). Frozen architecture (this roadmap SEQUENCES, it does not redesign):
> [`../../05-refined-shared-systems-architecture/agent-fabric.md`](../../05-refined-shared-systems-architecture/agent-fabric.md)
> (the refined Fabric architecture) + the refined
> [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md)
> §8 (the contracts the Fabric owns) + the dependency rows §1/§3/§4/§7/§9/§10/§11/§13 (what it consumes).
> Drills owed:
> [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md)
> §4.2 (AG-D1..AG-D11), §3.5 (the AG-D4/CI-T1 hard gate), §2 (E2E-2 the flagship). Doctrine:
> [`../../../external-insights/01-process-and-quality-doctrine.md`](../../../external-insights/01-process-and-quality-doctrine.md)
> (§2 order-by-non-negotiability — RCE/sandbox-escape before any feature; §3 prove-it-or-it-isn't-real + the
> failure-injection harness; §5 the committed ratchet; §1 name-your-floors / code-wins-over-docs) and
> [`../../../external-insights/04-hard-problems.md`](../../../external-insights/04-hard-problems.md) (§5
> untrusted-code-execution is a permanent never-"done" surface; §4 the deferral discipline). Spine: ADR-08
> (plan-then-apply / strategy boundary / tool registry), ADR-09 (a run is a durable workflow), ADR-20 (one
> unified sandbox), ADR-16 (backpressure / agent lane), ADR-17 (fail-static), ADR-03 (ReBAC), ADR-11 (cells),
> ADR-12 (GDPR holders). Date: 2026-06-19.
>
> **The shape of this system, and what that means for sequencing.** The Agent Fabric is the **substrate of the
> "agent-native from the ground up" promise** (VISION §3) — but its single most load-bearing piece, the unified
> sandbox `ToolHands::exec` (8.4), carries the **Tier-2 RCE/sandbox-escape floor** (master §1 Tier-2): one
> escape is catastrophic, and a property not drilled on a real kernel is a claim. Four consequences for this
> roadmap:
> 1. **The Fabric's core lands in M2, not before — it depends on the dependency root (Id) and the durability
>    floor (Storage/restore-verify) being green first.** An agent is a `Principal` with `kind=agent` flowing
>    through the *same* identity, gateway, event log, sandbox, and cost gate as everyone else (EI-03 preamble):
>    `list_objects`/`check`/`mint_run_token`/delegation (M1 Id), the outbox (M0 Bus), the durable-workflow
>    engine + firehose transport (M2 Bus/Flow), reserve/settle (M1 Storage). The Fabric cannot exist over a red
>    M0/M1.
> 2. **AG-D4 / CI-T1 — the real-kernel sandbox-escape drill — is the single hard GATE of the whole M2 band and
>    the whole build.** It is the M2→M3 go/no-go (master §2): until it is **green on the production backend**,
>    no untrusted CI step and no agent compute call runs in M3+. CI owns the runner + the drill (ADR-20); the
>    Fabric feeds the `kind=agent` job spec and is its first consumer. It is a **permanent gate** (master §4),
>    re-run on every backend/image/kernel change forever.
> 3. **The mock runtime is the named floor; the real LLM runtime is the scheduled follow-on (post-M5).** v1
>    ships `MockAgentRuntime` (`--use-mock`, a *real* runtime flag on the same code path users hit) so the whole
>    event→trigger→effect→event loop is golden- and mutation-testable; `LlmAgentRuntime` is the only vendor seam,
>    designed-not-built, swapped in *after* the safety drills are green (VISION §3; master §5). A config/impl
>    swap, not a rewrite.
> 4. **The agent-lane shed budget and the external-MCP endpoint are named floors with M5/post-M5 follow-ons.**
>    The per-tenant agent-run in-flight cap is a v1 floor tuned by the 30× surge drill (AG-D6); the external MCP
>    server endpoint (its auth/rate-limit/per-external-tenant budget/threat-model/legal sign-off) is a post-M5
>    follow-on.
>
> The honest progression: **first runnable** = M2 SKELETON (the whole gateway/identity/dispatch/reserve/trace
> path proven at ~zero cost with no model, no tools). **First useful** = M2 mock + plan-then-apply + HITL + the
> sandbox green (a mock triage agent can plan, get approval, and apply one governed effect, metered, with zero
> escapes). **Production-hardened** = M5 (the 30× agent-surge family holds the human lane, the E2E-2 flagship is
> green end-to-end across a kill, erasure reaches the trace, the real LLM runtime is the named post-M5 swap).

---

## 0. Where the Fabric lands in the master bands (the one-paragraph map)

The Fabric is an **M2 reactive-layer system** (master §2 M2: "the band where the agent-native-from-the-ground-up
promise gets its substrate"). It has **no M0/M1 work of its own** — but it is the heaviest *consumer* of M0
(outbox/causality) and M1 (Id `list_objects`/`check`/`mint_run_token`/delegation, Storage reserve/settle +
per-subject DEK, Tenancy partition), so its M2 entry is gated on M1 being green. **M2 is the Fabric's one large
band**: the trait set (brain/hands/loop/surface/inbox/effect), the SKELETON→mock runtimes, the plan-then-apply
`EffectApi`, the HITL withhold→approve→resume loop, the structural loop guards, the reserve/settle cost gate,
the per-run-token mint/scrub/revoke, the run-is-a-durable-workflow mapping + the `SCHEDULE_AND_RUN_JOB` idiom,
the permissioned tool registry + frozen `requires_approval` defaults + the MCP-exposure *seam*, and — the
keystone — **`ToolHands::exec` realised as CI's `kind=agent` job on the one unified sandbox, gated by AG-D4**.
M3/M4 add **no Fabric engine**: they register per-subsystem `ToolDef`s (Knowledge edit/publish, Issues
triage/transition, Chat post, Git merge, CI deploy) into the existing surface, and the AG-D4 gate is re-confirmed
on the production CI image in M4. M5 is the Fabric's **world-scale hardening + floor follow-ons** (the 30× surge
family AG-D6, the agent-lane shed-budget tuning, erasure-reaches-the-trace AG-D10 across all holders, and the
E2E-2 flagship). The Fabric is the **flagship of the M5 E2E wedge** (E2E-2 CI-fail → triage agent → issue →
chat → fix-PR is the agent-native proof) and rides the M6 dogfood. The **real `LlmAgentRuntime` and the external
MCP endpoint are post-M5** (after the safety drills are green).

**First runnable / first useful / production-hardened:**
- **First runnable (M2, SKELETON):** the SKELETON runtime drives the whole gateway → identity → dispatch →
  reserve → trace path at ~zero cost (no model, no tools) — an `InboxEvent` wakes a run, mints a per-run token,
  opens a reservation, writes a trace, settles. Proves the substrate path before any brain exists.
- **First useful (M2, mock + sandbox green):** a `MockAgentRuntime` (`--use-mock`) plans deterministically;
  `EffectApi` runs plan-then-apply; a gated effect is **withheld** until HITL approval; a `compute` tool runs
  in the unified sandbox with **AG-D4 green (zero escapes)**; the run is metered into the one wallet. The whole
  loop is golden + `cargo-mutants`-testable (AG-D9).
- **Production-hardened (M5):** the 30× agent-dispatch surge sheds the agent lane, holds the human lane, and
  reserve/settle refuses over-budget runs across every tenant (AG-D6); erasing a subject crypto-shreds the run
  trace + memory (AG-D10); E2E-2 proves exactly-once HITL + merge across a service kill; the real
  `LlmAgentRuntime` is the named post-M5 swap behind the frozen seam.

---

## 1. The contracts the Fabric owns / consumes, mapped to the milestone they land in

From contract-index §8 (owned: the eight Fabric contracts 8.1–8.8) + the dependency rows (consumed: §1 service
shell, §3 dispatch/firehose, §4 Id, §7.3 humanise, §9 workflow, §10 GDPR, §11 storage, §13 content/query).
"Lands" = the milestone by which the contract must be implemented or callable for the Fabric's gate to be green.

### 1.1 Owned by the Fabric (contract-index §8)

| # | Contract | Lands | Notes / floor |
|---|---|---|---|
| 8.3 | `AgentRuntime::step(&Conversation) → UseTools \| Submit` — the stateless brain; strategy seam (skeleton/mock/llm); `--use-mock` is a real flag | **M2** | SKELETON + Mock land M2; **`LlmAgentRuntime` is the named post-M5 floor** (the only vendor seam, designed-not-built, `no-llm-in-platform` lint). |
| 8.5 | `Agent::handle(InboxEvent, &dyn AgentRuntime) → RunOutcome` — platform-owned bounded multi-turn loop; nested causality; a run is a durable workflow | **M2** | the loop body is identical for mock and real. No floor. |
| 8.4 | `ToolHands::exec(Command) → ToolResult` — sandboxed computation; no host-exec bypass; **= CI's `kind=agent` job**; **four uniform guarantees** | **M2** | **the Tier-2 RCE floor.** The escape drill (AG-D4) is the M2 hard GATE; the sandbox runner is CI-owned (ADR-20), the Fabric is its first consumer. |
| 8.2 | `EffectApi::apply(run, ProposedEffect) → Applied \| Gated \| Denied` — plan-then-apply: schema → capability → delegation → tenant → budget → HITL → apply via public endpoint → meter | **M2** | the core safety+testability seam. No floor — identical mock/real. |
| 8.1 | `ToolSurface::register_tool(ToolDef{...})` + `resolve` — one permissioned catalogue; **frozen `requires_approval` defaults** (CI deploy/secret=yes; Git merge=yes; KN publish=yes; Chat post=no; cross-subsystem inherits target's default) | **M2** (surface) / **M3–M4** (per-subsystem `ToolDef`s) | the catalogue + internal consumption + the `exposed_over_mcp` **seam** land M2; subsystem tools register as each subsystem lands. |
| 8.6 | `EventInbox::deliver(InboxEvent)` — platform delivers matched events; **explicit-first dispatch** (a mention notifies, does not auto-spawn a costed run; implicit is L-3) | **M2** | implicit auto-dispatch is **`[OPEN → LEGAL]`** (counsel-gated, never wired in v1). |
| 8.7 | `run --dry-run(InboxEvent) → Vec<ProposedEffect>` — plan-then-apply testability | **M2** | the dry-run lever every E2E + every drill uses. No floor. |
| 8.8 | AG-7 agent trace — Knowledge accepts a content-addressed agent-trace write + registers it as an erasable holder | **M3** | the *seam* is M2; the trace **holder lands with Knowledge in M3** (the Knowledge deliverable). |

### 1.2 Consumed from other shared systems (the dependencies — the Fabric's M2 entry gate)

| From | Contract used | Needed by | Must be green by |
|---|---|---|---|
| **Substrate** (§1) | `serve(AppSpec)` (1.1), three-surface (1.2), `ResilientClient` honouring `Retry-After` (1.9), `FailStatic` (1.10), shed order (1.11), telemetry signals (1.8) | the service shell; the agent shed lane; survival-signal assertions | **M0** |
| **Bus** (§2/§3) | `OutboxTx::emit(draft, cause)` nested causality (2.2); the dispatch/reactive tier + loop guards (3.6); Signals (3.1); the firehose resume-cursor transport (3.5) | wake-up; emit domain events; loop guards; (long-job/live transport) | **M0** (outbox), **M2** (dispatch/firehose) |
| **Identity** (§4) | `mint_run_token`/`revoke` re-mintable on resume (4.7); `check(.., caveat?: CaveatContext)` (4.2); `list_objects → SetExpr` push-down (4.3, tool-list scoping); `delegation` the `∩` algebra (4.5); `list_subjects` HITL approver set (4.4) | per-run identity; effect validation; tool scoping; HITL approvers | **M1** |
| **Durable workflow** (§9) | `DurableExecutor{start, signal, describe, cancel}` per-effect `idem_key` (9.1); `WfCtx` + `SCHEDULE_AND_RUN_JOB` long-park idiom (9.2); durable HITL signal (9.4); the timer wheel (9.3) | HITL waits, long sandbox jobs, budgets, multi-day pauses | **M2** |
| **CI unified runner** | the `kind=agent` job spec + the hardening profile + the escape drill (8.4; ADR-20) | `ToolHands::exec` real execution | **M2** (the AG-D4 gate) |
| **Storage** (§11) | reserve/settle cost gate (11.7); per-subject DEK / crypto-shred (11.3/11.4) for the trace | the universal cost gate; erasable trace | **M1** |
| **Notifications** (§7) | `humanise((template_key, args), viewer, locale)` (7.3) | HITL card text + agent-authored messages (the ONE templating surface) | **M2** |
| **Knowledge** (§8.8/§13.1) | content-addressed trace write; frozen `myelin-content` block model | the execution trace (a holder) | **M3** |
| **GDPR/Audit** (§10) | `audit.record` via outbox (10.6); `PersonalDataHolder` registration (10.1); the ONE erasure posture by reference (10.9) | tamper-evident audit; erasure of run/trace/memory | **M1** (structural) / **M5** (full fan-out) |
| **Commercial wallet** (§11.7) | the prepaid balance the reserve/settle gate reads (C-1) | the cost gate | **M1** |
| **Shared crates** (§13) | `myelin-content` frozen (13.1, the trace block model + the inline ref nodes the reference-gate loop guard reads) | the trace; the loop guard reference gate | **M2** (frozen) |

The Fabric **owns no M0/M1 contract** — its earliest dependency is the M1 dependency root (Id) and durability
floor (Storage). This is why the Fabric is correctly an M2 system: it is a projection of capabilities that
already exist by M1 (the compounding-payoff test, EI-01 closing).

---

## 2. The milestones (the floor-then-full progression, mapped to the bands)

Each milestone states its **work**, its **upstream dependency** (what must be green to start), the **floor it
ships + the named follow-on**, and the **exit gate** (the quantified drills that must be green to call it done).

### M2-A — SKELETON: the substrate path proven at zero cost (first runnable)

**Band:** M2 (early). **Thesis:** prove the whole agent path — gateway → identity → dispatch → reserve → trace
— *before* any brain or tool exists, so the substrate is exercised at ~zero cost (the AG-3 build order
SKELETON → mock → real, §3 of the architecture).

**Work:**
- The trait set as the M2 contract surface: `AgentRuntime` (8.3), `Agent::handle` (8.5), `ToolSurface` (8.1),
  `EventInbox` (8.6), `EffectApi` (8.2), `ToolHands` (8.4) — the **only** strategy-swappable members are
  `AgentRuntime` (brain) and `ToolHands` (hands); the rest are platform-owned and identical for mock and real.
- The data model (`run`/`tool_def`/`proposed_effect`/`hitl_gate`/`trace`), all `(tenant, region)`-first,
  RLS-enforced, residency-pinned, per-tenant envelope-encrypted, `PersonalDataHolder` auto-registered (1.4).
- The SKELETON runtime: no model, no tools; a stub `step` that submits immediately. It drives `Agent::handle`
  → mint a per-run token (4.7) → open a reservation (11.7) → write a (near-empty) trace → settle. This is the
  **first-runnable** proof: an `InboxEvent` produces a complete, attributed, metered, traced run.
- The run-is-a-durable-workflow mapping (ADR-09, §5.6): the workflow owns budget/gates/state; `step`/`exec` are
  activities; reserve/settle are the bookends.

**Upstream dependency:** M1 green (Id `mint_run_token` + delegation; Storage reserve/settle + per-subject DEK;
Tenancy partition); M0 green (outbox, the harness, the lints). M2 Bus dispatch tier (3.6) + Flow
`DurableExecutor` (9.1) callable.

**Floor / follow-on:** SKELETON itself is the floor under Mock and Llm — it is *named* the skeleton (EI-04 §4:
a skeleton is not done; the half that's missing is the brain + the tools). Follow-on = M2-B.

**Exit gate:** the SKELETON path emits a complete trace + a balanced reserve/settle ledger; AG-D8 (per-run
token revoked on teardown AND auto-expires; **0 shared token leaked into the child env**) green on the
no-tool path — CI.

### M2-B — Mock runtime + plan-then-apply + HITL: the governed agent loop (first useful, part 1)

**Band:** M2. **Thesis:** a deterministic mock agent that *plans* effects, has them validated by
plan-then-apply, and is **withheld** on consequential effects until a human approves — the whole safety loop,
golden- and mutation-testable, before any real model.

**Work:**
- `MockAgentRuntime` (`--use-mock`, a **real** runtime flag on the same code path users hit — mock agents only
  during development per VISION §3): deterministic scripted `StepOutcome`s; the lever for golden +
  `cargo-mutants` testing of the event→trigger→effect→event loop.
- The plan-then-apply `EffectApi::apply` pipeline (8.2), **in order, fail-closed**: SCHEMA → CAPABILITY (with
  the `CaveatContext` for field/transition ABAC, evaluated at `check`-time off the hot `list_objects` path,
  4.2) → DELEGATION (`agent.policy ∩ delegation ∩ tenant.policy`, intersection never union, 4.5) → TENANT →
  BUDGET (11.7) → HITL GATE → APPLY via the subsystem's **public endpoint** as the agent principal (same
  gateway, no carve-out) → METER. A denied effect returns an **ordinary `Denied` tool error — no privileged
  fallback** (AG-5).
- The tool-list scoping: the `Conversation.tools` the brain sees = the run's permitted, delegation-scoped
  subset computed via the `list_objects` `SetExpr` push-down (4.3, no N+1); `EffectApi` still re-checks at
  apply time (the scoping is an optimisation, the check is the guarantee).
- The HITL withhold → approve → resume loop (AG-8, §5.3): a gated effect returns `Gated`, **does not mutate**;
  a durable-workflow wait (9.4) surfaces as a chat approval card showing action + risk + **live cost
  estimate**, approver set = `list_subjects(object, approve_perm)` (4.4); resume re-runs the step with the tool
  added to "approved". Resume idempotency is **per-effect** (`idem_key = card_id` single, `card_id:<effect_idx>`
  multi/partial; 9.1, OQ-F): a double-click is one approval, a partial approval is well-defined.
- The frozen `requires_approval` defaults table (8.1, §6.3) seeded into `tool_def.requires_approval`; the
  cross-subsystem rule (an effect that mutates another subsystem inherits **that** subsystem's default).
- Card text + agent-authored messages go through the ONE templating surface `humanise` (7.3) — never raw
  strings; `(template_key, args)` + `ArtifactRef`, per-viewer, erasure-safe.
- `run --dry-run` (8.7): stops after the HITL step and shows the plan (the testability lever).
- The structural loop guards (AG-6, §5.5): self-guard, reference gate (only a structured `artifact_ref` node
  re-triggers, wired to the frozen `myelin-content` inline nodes 13.1), causal-depth ceiling (default 12),
  shared-root tripwire, idempotent tools, bounded dispatch pool. Loop prevention is **structural, not
  convention** — a human or agent can never typo into a loop.
- Per-run identity (§5.7): mint at dispatch with token life == run life, scrub any shared platform token in the
  child env, revoke idempotently on teardown even on crash, **re-mintable mid-workflow on resume** (4.7, C6) so
  a multi-day HITL pause never widens the attribution window.

**Upstream dependency:** M2-A green; Id `check`+`CaveatContext`+`list_objects`+`list_subjects`+`delegation`
(M1); Flow durable HITL signal + per-effect `idem_key` (9.1/9.4); Notif `humanise` (7.3); `myelin-content`
frozen (13.1).

**Floor / follow-on:** **Mock runtime is the named floor** (VISION §3) — the real `LlmAgentRuntime` is the
post-M5 follow-on, swapped in *after* the safety drills are green (a config/impl swap, not a rewrite; the
trigger is the safety drills going green). The **external MCP server endpoint is a floor** — v1 builds the
catalogue + internal consumption + the `exposed_over_mcp` seam; the external endpoint is a post-M5 follow-on.

**Exit gate (the M2 deterministic correctness family, CI):**
- **AG-D1** (a tool tries to write outside `EffectApi` → structurally impossible; `no-host-exec` + `no-cross-db`
  lints green) — CI.
- **AG-D2** (effect outside the `∩` → `Denied` returns, **0 privileged fallback**) — CI.
- **AG-D3** (effect policy allows but delegation/tenant forbids, and vice-versa → confined to the
  intersection; **0 over-privilege**) — CI.
- **AG-D5** (gated tool → withheld, returns error, **does NOT mutate**; card shows action+risk+cost; approval
  resumes + applies **exactly once**; rejection halts; **0 mutation pre-approval, 1 apply**; per-effect
  idempotency — partial approval + double-click well-defined) — CI.
- **AG-D7** (adversarial agent→agent self-trigger → depth ceiling (12) + tripwire + bounded pool **halt ≤
  ceiling**; per-tenant breaker trips) — CI.
- **AG-D9** (run a scripted mock twice → **identical proposed-effect sequences**; `cargo-mutants` over
  event→trigger→effect→event ≥ the mutation threshold) — CI.
- **AG-D11** (runaway loop vs an exhausted wallet → reserve refuses new runs, never interrupts in-flight;
  **stops at the wallet**) — CI.

### M2-C — `ToolHands::exec` on the unified sandbox + the hard escape GATE (first useful, part 2 — the keystone)

**Band:** M2 (the band's go/no-go). **Thesis:** the unified sandbox — `ToolHands::exec` realised as CI's
`kind=agent` job on the ONE runner (ADR-20) — with the **four uniform guarantees** every subsystem's tool
inherits by construction, and **the real-kernel escape drill green on the production backend**. This is the
Tier-2 RCE floor; nothing downstream of untrusted execution proceeds over a red AG-D4.

**Work:**
- `ToolHands::exec` (8.4): one method, **no host-execution path that bypasses it** (the `no-host-exec` lint,
  1.6). It carries **only** untrusted code execution (`compute`/`external` — a test, build, linter, script) —
  the only thing that touches the kernel sandbox; *side-effecting* mutation goes through `EffectApi`, never
  through `exec` (the routing split is the safety boundary, §5.0).
- The four uniform guarantees pinned (X-6, §2.2), inherited by every subsystem tool without re-implementation:
  (1) **universal cost gate** (reserve/settle 11.7, same wallet as CI); (2) **attribution** (per-run attenuated
  token 4.7, re-mintable on resume); (3) **HITL withhold** (plan-then-apply); (4) **isolation floor + drill**
  (gVisor-class userspace-kernel or microVM; egress default-deny, read-only root + tmpfs, caps dropped,
  no-new-privileges, seccomp, digest-pinned images fail-closed on un-digested tags, whole-guest kill on
  teardown, `pids.max` + zero swap, secrets resolved *inside* the boundary and never forwarded via the runtime).
- The `SCHEDULE_AND_RUN_JOB` long-park idiom (9.2/9.4, C5): a long sandbox job dispatches (reserve at dispatch)
  and returns; the run **parks holding no runtime**; completion arrives hours later as a durable signal
  idempotent on `idem_token` (the runner can deliver "done" twice; the workflow wakes once). The same idiom CI's
  merge-queue uses.
- CI owns the runner + the drill (ADR-20); the Fabric feeds the `kind=agent` job spec and is its first
  consumer. The drill runs an adversarial corpus on a **real kernel**: kernel-exploit primitives, cloud-metadata
  SSRF (169.254.169.254) → cred theft, control-plane/internal-RPC reach, cross-tenant network/storage, fork
  bomb, disk fill, secret exfil via egress.

**Upstream dependency:** M2-A/M2-B green; the CI unified-runner skeleton + hardening profile (CI-owned, M2);
Flow `SCHEDULE_AND_RUN_JOB` (9.2); the reserve/settle gate (11.7).

**Floor / follow-on:** there is **no floor on AG-D4** — zero escapes is the floor and the full answer; it is a
**permanent GATE** (master §4), re-run on every backend/image/kernel change forever (untrusted-code execution
is a never-"done" surface, EI-04 §5). The named follow-on inside the sandbox family is the real
`LlmAgentRuntime` running its compute against this same hardened runner (post-M5, after the gate is green).

**Exit gate (the M2 hard go/no-go — blocks ALL of M3+):**
- **AG-D4 / CI-T1** (`compute` tool attempts a kernel escape on a real kernel → **ZERO escapes**; emits a green
  escape attestation **or CI is no-go for untrusted code**) — **GATE**. *This is the single hard go/no-go
  before any untrusted CI step or agent compute call runs in M3+; re-run on every backend/image/kernel change.*

### M3 — Per-subsystem tools register; the trace holder lands (no new Fabric engine)

**Band:** M3 (the producer subsystems). **Thesis:** Knowledge + Git register their `ToolDef`s into the existing
surface and the agent-trace holder goes live; the Fabric adds **no engine** — each new tool is a projection of
the M2 plan-then-apply path.

**Work:**
- Git registers `git.merge` (`requires_approval=yes` — the consequential gate, AG-8) and `open_pr`
  (`requires_approval=no`, reversible); the Git ReBAC fragment (4.9) supplies the caps.
- Knowledge registers `publish`/`edit(confidential_page)` (`requires_approval=yes`, approver set) and
  `draft`/`comment` (`no`); the agent-trace holder (8.8) goes live — Knowledge accepts the content-addressed
  trace write (reusing the frozen `myelin-content` block model 13.1) and registers it as an erasable
  `PersonalDataHolder` (the KN deliverable; the Fabric's `run.trace_ref` resolves to it).
- The KN HITL path is exercised in-context (an agent edit via `EffectApi` is attributed "suggested by agent"; a
  consequential publish/confidential edit is HITL-withheld until approval; double-click is one approval) — this
  is KN-D11, owned by Knowledge but a Fabric-loop assertion.

**Upstream dependency:** M2 green (**AG-D4 green** so any agent `compute`/edit can run); Knowledge produces
docs/databases (the trace holder); the frozen `myelin-content` taxonomy (13.1).

**Floor / follow-on:** **agent long-term memory / RAG over prior runs is a named holder seam, not built** —
v1 agents are stateless across runs except for the content-addressed trace document; the embedding store + its
erasure are a Search/Knowledge follow-on (when built, it indexes via Search `semantic` 6.2, ACL-filtered, and
purges on `*.erased`).

**Exit gate (folded into the M3 band gate, master §2):**
- **KN-D11** (agent edit governed: 0 ungoverned / 0 pre-approval / 0 double-apply) — CI.
- **KN-D12** (erase a subject → content-addressed agent traces crypto-shredded/purged; attribution falls back
  to the pseudonym; **0 recoverable PII**, attribution intact) — SCHED (the trace-holder erasure proof).

### M4 — Consumer-subsystem tools register; AG-D4 re-confirmed on the prod CI image

**Band:** M4 (the consumer subsystems). **Thesis:** Issues + Chat + CI register their `ToolDef`s; the unified
runner — the same hardened runner the Fabric already drilled — is re-confirmed on the production CI image (AG-D4
== CI-T1, the same gate).

**Work:**
- Issues registers `forecast`/`triage`/`sla_draft` (`no`, advisory/suggest) and `transition(issue, →done)` on
  an SLA-bound issue (`yes` if the transition has an approver edge — the field/transition ABAC caveat, §5.2
  step 2). The Issues ReBAC fragment (4.9) supplies the caps.
- Chat registers `post_message`/`react` (`no`, reversible) and any `EffectApi` tool that mutates another
  subsystem (inherits **that** subsystem's default — "governed where it lands"). **Explicit-first dispatch**
  (8.6): a casual `@agent` mention notifies the inbox, does **not** auto-spawn a costed run; only an explicit
  action/structured trigger dispatches; reserve/settle gates even the explicit run.
- CI registers `deploy(env)`/`approve_deploy`/`write_secret` (`yes`) and `run_pipeline` non-prod (`no`); CI's
  runner **is** the Fabric's `ToolHands::exec` runner (ADR-20) — **AG-D4 / CI-T1 is re-confirmed green on the
  production runner image** (the M4 hard gate).

**Upstream dependency:** M3 green; the consumer subsystems exist (Issues/Chat/CI); the CI unified runner is the
production image.

**Floor / follow-on:** implicit auto-dispatch on a casual mention remains **`[OPEN → LEGAL]`** (L-3,
counsel-gated; GDPR Art. 22 / EU AI-Act human-oversight) — explicit-first is v1; no auto-spawn path is wired
until counsel ratifies the human-oversight basis.

**Exit gate (folded into the M4 band gate, master §2):**
- **CI-T1 / AG-D4** re-confirmed green on the production CI runner (the hard GATE; re-run on the CI image) —
  GATE.
- **CHAT-D17** (a casual `@agent` mention → **0 auto-spawn**, reserve gate on the explicit run) — CI.
- **CHAT-D9 / CHAT-D10** (HITL bridge across a Chat+Workflow kill → gated tool runs exactly once, double-click
  is one approval; batch 2-of-3 → per-effect idempotency, the withheld never mutates) — CI.
- **ISS-D12** (an agent hitting a governed transition is HITL-gated, withheld, no mutation until approval) — CI.

### M5 — World-scale hardening + the agent-surge family + the E2E-2 flagship + erasure fan-out

**Band:** M5 (world-scale hardening + the floor follow-ons + the E2E wedge). **Thesis:** with all five
subsystems on one substrate and the deterministic correctness drills green, prove the Fabric **under world-scale
agent load**, green the agent-native E2E flagship, and complete the erasure fan-out across all holders.

**Work — world-scale hardening (the F6 surge family):**
- The 30× agent-dispatch surge (AG-D6): the **protected human lane holds**, the **agent lane sheds** (`429 +
  Retry-After`, honoured by the resilient client 1.9), reserve/settle **refuses over-budget runs**, and other
  tenants are unaffected (the per-tenant bulkhead). This **tunes the named v1 agent-lane shed budget**
  (the per-tenant agent-run in-flight cap; humans never queue behind agents — C10/OQ-K): the concrete number is
  set here by measurement, not predicted (the master §5 floor "the agent lane is bounded, has a reserved human
  lane, and applies the shed order"; an unbounded lane is the cascade, EI-02 §5).

**Work — the floor follow-on (the real runtime, scheduled post-M5/execution per master §5):**
- `LlmAgentRuntime` — the only vendor seam, the only place a model/SDK/prompt/model-name string appears
  (enforced by the `no-llm-in-platform` lint 1.6): EU-hostable, region-aware, swappable; metering one cost event
  per model call (wholesale ≠ markup). Swapped in *after* the safety drills (AG-D4/D2/D3/D5) are green — a
  config/impl swap behind the frozen `AgentRuntime` seam, not a rewrite. The EU-sovereign sub-processor is
  `[OPEN → LEGAL]` (AG-9).

**Work — erasure fan-out (master §5, the full DSR across all H1–H18 holders):**
- AG-D10: erasing a subject crypto-shreds/purges the run trace + agent memory/embeddings; attribution → an
  opaque pseudonym; **0 recoverable PII** — reads the ONE erasure posture (10.9) by reference, instantiated for
  the Fabric (run/trace/memory) not restated.

**Work — the whole-system E2E wedge (the flagship):**
- **E2E-2 — CI-fail → triage agent → issue → chat → fix-PR** (the agent-native flagship, the M5 differentiator
  proof). A push fails CI; a Signal wakes a **mock** triage agent (explicit-first, Signal-driven not a casual
  mention); reserve at dispatch (exhausted-wallet variant → refuse-start); the agent plans
  `[create_issue, post_chat_message, open_pr]` **deterministically** (AG-D9); `create_issue` applies (no
  approval); a `git.merge` proposal is **withheld** (`requires_approval=yes`, returns `Denied`, does not mutate);
  the Agent+Workflow services are **killed mid-`ack_window`**; the human approves **days later (double-click)**;
  the durable workflow resumes, re-mints the token (4.7), consumes the approval **exactly once**, and the merge
  applies **once** (no double-effect); the fix-PR's CI goes green; the merge-queue wakes on `ci.result`
  idempotently and merges; `git.pr.merged` closes the issue. The Fabric is the spine of this scenario.

**Upstream dependency:** M4 green (all five subsystems exist; the deterministic correctness drills are green;
the safety drills are green so the real runtime *can* be swapped).

**Exit gate (the M5 band gate, master §2/§4):**
- **AG-D6** (30× agent dispatch surge → human lane holds, agent sheds, reserve/settle refuses over-budget,
  others unaffected; the **named agent-lane shed budget** asserted) — SCHED.
- **AG-D10** (erase a subject → trace + memory/embeddings crypto-shredded/purged; attribution → opaque
  pseudonym; **0 recoverable PII**) — SCHED.
- **E2E-2 green** (0 effect outside the `∩`; 0 mutation before approval; exactly-once approval + merge across
  the kill; reserve/settle balanced; merge-count == 1; deterministic run trace) — SCHED (the flagship green
  artifact).

### M6 — Dogfooding: the platform's own agents run on the platform

**Band:** M6 (Myelin hosts itself). **Thesis:** the Fabric's agents (mock in v1; real if the post-M5 swap has
landed) run on the platform's own commits/issues/chats — the cheapest, most honest load generator.

**Work:**
- The Myelin development loop uses the Fabric: a triage agent on the self-hosting CI graph, an agent-trace
  holder for the platform's own runs, the every-incident-adds-a-drill loop filing Myelin issues + reproducing
  drills.
- The Fabric participates in the M6 dogfood gate: the self-hosting CI graph is green on the platform's own
  commits (the dogfood loop is live); the gate invariant holds end-to-end (no later-band Fabric gate is red).

**Upstream dependency:** M5 green (the platform is world-scale-ready; AG-D4 + AG-D6 + E2E-2 green; you do not
dogfood real team data onto an unhardened agent substrate).

**Exit gate (folded into the M6 done-bar, master §2):**
- The Fabric's runs on the self-hosting graph emit balanced reserve/settle ledgers + traces; no later-band
  Fabric gate is red (the truth-up pass confirms every PROVEN Fabric row rests on a dated green artifact).

---

## 3. The hard-problem / world-scale work, sequenced explicitly (name-your-floors)

The doctrine binds: name the floor, name the follow-on, schedule it (EI-04 §4; master §5). Every Fabric floor,
with its ship-band and follow-on-band:

| Floor (shipped) | Band | The full answer (follow-on) | Band | The trigger |
|---|---|---|---|---|
| **Mock agent runtime** (`--use-mock`, scripted-deterministic; the same code path users hit) | **M2-B** | **`LlmAgentRuntime`** (the real adapter, region-aware EU-hostable sub-processor, the only vendor seam) | **post-M5 / execution** | the safety drills (AG-D4/D2/D3/D5) are green; a config/impl swap, not a rewrite (VISION §3) |
| **The agent-lane shed budget** (per-tenant in-flight cap; humans never queue behind agents; `429 + Retry-After`) | **M2** (floor named) | **The measured cap** (set by telemetry, not predicted) | **M5** | the 30× agent-surge drill AG-D6 measures the cap (OQ-K) |
| **External MCP server endpoint** (the `exposed_over_mcp` seam + internal consumption built) | **M2-B** | **The external endpoint** (its auth, agent-lane rate-limit, per-external-tenant budget, threat model, Legal/DPO sign-off) | **post-M5** | external-agent demand + counsel sign-off |
| **Agent statelessness-across-runs except the trace** (no long-term memory/RAG) | **M2/M3** | **Agent long-term memory / RAG** (an embedding store via Search `semantic`, ACL-filtered, purged on `*.erased`) | **post-M5 (Search/KN follow-on)** | a measured need for cross-run recall; the holder seam already exists |
| **The structural erasure floor for the trace** (per-subject DEK crypto-shred + pseudonym shred) | **M2** (structural) | **The full DSR fan-out across all H1–H18 holders** (the trace + memory legs) | **M5** | every holder exists (master §5); AG-D10 owed |

**The two `[OPEN → LEGAL]` items (the structural floor ships regardless; the residual is flagged to
counsel/DPO):**
- **Implicit auto-dispatch on a casual mention** (CHAT-1, **L-3**): explicit-first is v1; no auto-spawn path is
  wired until counsel ratifies the GDPR Art. 22 / EU AI-Act human-oversight basis. Owned by Chat P6 +
  Commercial + Legal.
- **Trace verbosity / reasoning-capture policy** (**L-4**): capture the tool-call/result transcript by default
  (load-bearing for audit + replay); gate free-form chain-of-thought capture behind a tenant setting tagged
  `#[personal_data]` under the one erasure posture (10.9); the retention + AI-Act classification is flagged for
  counsel.
- **Build-data-as-LLM-training basis** (OQ-H): **foreclosed by default** — no platform code path feeds tenant
  content to model training; flagged for counsel.

The honest-floor rule binds all of these: each is tracked in the gap report with its claimed/proven status and
its linked follow-on; the gap being *invisible* is the only failure.

---

## 4. The drills the Fabric owes, mapped to the milestone that greens them (the gate invariant, quantified)

From the catalogue §4.2 (AG-D1..AG-D11), §3.5 (the AG-D4 GATE), §2 (E2E-2). The green artifact for each is the
named telemetry assertion (contract 1.8) passing. Every threshold is a **default-to-beat** measured in Phase 6.

| Drill | Property | Quantified gate | Greened by | Freq |
|---|---|---|---|---|
| **AG-D4 / CI-T1** | **Real-kernel sandbox escape** | **ZERO escapes** on a real-kernel adversarial corpus; green attestation or CI is no-go | **M2-C** (re-confirmed M4 on prod image) | **GATE** (permanent; every backend/image/kernel change) |
| AG-D1 | Agent cannot mutate directly | `no-host-exec` + `no-cross-db` lints green; **0 direct mutation** | M2-B | CI |
| AG-D2 | Denied → ordinary tool error | effect outside the `∩` → `Denied`; **0 privileged fallback** | M2-B | CI |
| AG-D3 | Delegation intersection / least-privilege | confined to `agent.policy ∩ delegation ∩ tenant.policy`; **0 over-privilege** | M2-B | CI |
| AG-D5 | HITL withhold→approve→resume | withheld (no mutate), card shows action+risk+cost, approval applies **exactly once**; **0 mutation pre-approval, 1 apply**; per-effect idempotency | M2-B | CI |
| AG-D7 | Causal-loop tripwire | depth ceiling (12) + tripwire + bounded pool **halt ≤ ceiling**; breaker trips | M2-B | CI |
| AG-D8 | Per-run token outlives the run | revoked on teardown AND auto-expires ≤ W; **0 leaked token** into child env; re-mint on resume keeps a multi-day pause attributed | M2-A/M2-B | CI |
| AG-D9 | Mock determinism | scripted mock twice → **identical proposed-effect sequences**; `cargo-mutants` ≥ threshold | M2-B | CI |
| AG-D11 | Cost gate: runaway self-limiting | refuses to start past exhaustion, never interrupts in-flight; **stops at the wallet** | M2-B | CI |
| AG-D6 | 30× agent surge / fairness | human lane holds, agent sheds (429+Retry-After), reserve refuses over-budget, others unaffected; **named shed budget** | **M5** | SCHED |
| AG-D10 | Erasure reaches the trace + memory | trace + memory/embeddings crypto-shredded/purged; attribution → pseudonym; **0 recoverable PII** | **M5** (M3 for the trace-holder leg KN-D12) | SCHED |
| **E2E-2** | The agent-native flagship | 0 effect outside the `∩`; 0 mutation before approval; exactly-once approval + merge across a kill; reserve/settle balanced; merge-count == 1 | **M5** | SCHED |

**The Fabric's must-be-green-first ordering (order-by-non-negotiability):** AG-D4 (M2-C) is the single hard
go/no-go — it sequences everything downstream of untrusted execution and is the M2→M3 gate. The
deterministic-correctness family (AG-D1/D2/D3/D5/D7/D9/D11) is the M2 floor that proves plan-then-apply + the
loop guards before any subsystem tool registers. The surge + erasure + flagship (AG-D6/D10/E2E-2) are M5 — they
cannot be green until all five subsystems and all holders exist.

---

## 5. Digest

**Milestones (band → work):**
- **M2-A — SKELETON (first runnable):** the trait set + data model + the SKELETON runtime proving the
  gateway→identity→dispatch→reserve→trace path at ~zero cost; the run-is-a-durable-workflow mapping.
- **M2-B — Mock + plan-then-apply + HITL (first useful, pt.1):** `MockAgentRuntime` (`--use-mock`),
  `EffectApi::apply` (schema→capability→delegation→tenant→budget→HITL→apply→meter), the withhold→approve→resume
  loop (per-effect idempotency), the frozen `requires_approval` defaults, the structural loop guards, per-run
  identity (re-mintable on resume), `humanise` card text, `run --dry-run`.
- **M2-C — Unified sandbox + the hard GATE (first useful, pt.2, the keystone):** `ToolHands::exec` = CI's
  `kind=agent` job, the four uniform guarantees, the `SCHEDULE_AND_RUN_JOB` long-park idiom, and **AG-D4 green
  on the production backend**.
- **M3 — Per-producer tools + the trace holder:** Git (`git.merge` gated) + Knowledge (`publish` gated, the
  content-addressed agent-trace holder) register; no new engine.
- **M4 — Per-consumer tools + AG-D4 re-confirmed:** Issues (transition ABAC) + Chat (explicit-first) + CI
  (deploy gated; the runner is the Fabric's runner) register; AG-D4/CI-T1 re-confirmed on the prod image.
- **M5 — World-scale + the flagship:** the 30× agent-surge family (AG-D6, the shed budget tuned), erasure
  fan-out (AG-D10), the **E2E-2 flagship green**, the real `LlmAgentRuntime` named as the post-M5 swap.
- **M6 — Dogfood:** the platform's own agents run on its own commits/issues/chat.

**Floors + follow-ons:**
- **Mock runtime** (M2-B floor) → **`LlmAgentRuntime`** (post-M5; trigger: the safety drills are green).
- **Agent-lane shed budget** (M2 floor, the cap) → **measured cap** (M5; trigger: AG-D6 measures it).
- **External MCP endpoint** (M2-B seam) → **the external endpoint** (post-M5; trigger: demand + Legal sign-off).
- **Stateless-except-trace** (M2/M3 floor) → **long-term memory/RAG** (post-M5; the holder seam exists).
- **Structural trace-erasure** (M2 floor) → **full DSR fan-out across H1–H18** (M5; AG-D10).
- **`[OPEN → LEGAL]`:** implicit auto-dispatch (L-3, explicit-first only in v1), reasoning-capture policy (L-4),
  build-data-as-training (foreclosed by default).

**Critical upstream dependencies:**
- **M0:** the outbox (`OutboxTx::emit` nested causality), the harness/service shell (`serve`, three-surface,
  `ResilientClient`+`Retry-After`, `FailStatic`, the shed order), the `no-host-exec` + `no-cross-db` +
  `no-llm-in-platform` lints, the failure-injection harness (AG-D4's machine).
- **M1 (the hard entry gate — the Fabric is an M2 system because it needs M1 green):** Identity
  (`mint_run_token` re-mintable, `check`+`CaveatContext`, `list_objects` `SetExpr` push-down, `delegation` the
  `∩` algebra, `list_subjects`), Storage (reserve/settle + per-subject DEK), Tenancy (partition key).
- **M2 (co-band):** the Bus dispatch tier + the firehose resume-cursor transport, the durable-workflow engine
  (`DurableExecutor` per-effect `idem_key`, `SCHEDULE_AND_RUN_JOB`, durable HITL signal, the timer wheel), Notif
  `humanise`, the CI unified-runner skeleton + hardening profile (AG-D4), `myelin-content` frozen.
- **M3:** Knowledge (the agent-trace holder, the frozen block model).
- **M4:** the consumer subsystems + the production CI runner image (AG-D4 re-confirm).
- **M5:** all five subsystems + all H1–H18 holders (E2E-2, AG-D6, AG-D10).

**The two hardest single seams on the Fabric's path:** **AG-D4** (the sandbox-escape GATE — blocks all
untrusted execution, both CI and agent compute; the M2→M3 go/no-go and a permanent gate) and the **plan-then-
apply intersection** (`agent.policy ∩ delegation ∩ tenant.policy` — an agent can do nothing no human role can,
proven by AG-D2/D3 before any tool registers).
