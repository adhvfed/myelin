# CI/CD — Key User Flows

> Phase 4 — CI design sketch (REQUIRED before architecture). The key flows, **including the agent/HITL
> flows** (proposed effects, approval cards, attribution) and the **cross-subsystem flows** CI
> participates in (design-language §6, §7.2; the spine's flagship walkthroughs, Phase-2 §6). Each flow
> names the events/contracts it rides so it stays grounded in the Phase-3 build-to surface.

---

## Flow 1 — Push → PR checks → merge gate (the Git ↔ CI seam, the everyday path)

1. Developer pushes to a PR branch. Git emits `git.pull_request.synchronized` (durable bus, via outbox).
2. CI's **Trigger/Dispatch** matches the repo's `on: pull_request` trigger via the shared
   **`EventMatcher`** (cheap, close to the bus), **dedups on `event_id`** (exactly-once *effect*),
   resolves the definition at the head commit → **content-addressed snapshot**, starts the
   `ci.pipeline` workflow (sketch 02), and the scheduler leases jobs onto the EU runner pool as
   **trusted** (member) or **untrusted** (fork) per the trust-tier evaluator (sketch 01/04).
3. Developer opens the **Single-run view**, watches the DAG go live; clicks a running job → **Live log
   view** streams (firehose, secret-masked). A failing step deep-links to its log range (jump-to-failure).
4. On completion CI emits `ci.status.updated`; **Git renders the checks badge** on the PR — green
   unblocks merge (branch protection "require CI green").

**States the flow must design:** run *queued* (no logs yet) → *running* (live) → *partial-failure* (one
job red, rest green) → *success* / *failed* / *cancelled* / *timed-out* / *dead-runner-reaped* (the run
recovered/failed because a runner died — honest, not silent). Refs: `ref.created` run→PR→commit.

## Flow 2 — CI fail → agent triage → issue → chat → fix PR (the agent-native flagship)

This is the spine's flagship (Phase-2 §6.2; system-overview §8.2); **CI is the origin**. It exercises
the full agent-native UX contract (design-language §6).

1. A job fails. CI emits **`ci.run.failed` with *structured* failure** (which step, which test, log
   excerpt — a deliberate agent-native design goal, CI-DD §8.2), not just a blob. A Signal is curated
   (`sig…ci-failure`) and the durable `ci.log.available` pointer published.
2. A trigger wakes **MockTriageAgent** (on-behalf-of the pusher, under a `RunBudget`; reserve/settle
   gate checks the wallet first). **Plan-then-apply (§6.2):** the agent *proposes* `issue.create` +
   `ref.create×2` + `chat.post`; `EffectApi` validates each against `agent.policy ∩ delegation ∩
   tenant.policy` and applies (Agent §5.2). The proposed effects render as a **plan** ("FixAgent
   proposes: open issue ENG-412, link RUN-991, post to #incidents") — visible *before* the effect.
3. `issue.created` wakes **FixAgent**, which proposes `git.open_pr` — **sensitive on a protected repo**,
   so Id returns **Gated** → a **durable-workflow HITL gate** opens, surfaced as a **chat approval card**.
4. The card (the §5.4/§6.3 component, the `agent` treatment) shows: the **pending action** (open PR
   #88), its **risk**, a **live cost estimate** (from the reserved budget), under **whose delegated
   authority**, with **Approve / Edit / Reject**. Humanised at the backend (NOTIF-1 — routable
   `ArtifactRef`s, never raw ids). It also lands in the **notifications inbox** (§5.8) so it's never missed.
5. A human approves (possibly **days** later — the durable signal holds, no runtime; Workflow §6.3). The
   workflow resumes; the gated step re-runs **with the tool now approved** (Agent §5.3); the PR opens.
6. **One `correlation_id` threads the whole chain**; **loop depth capped** (AG-6 guards); **full audit
   provenance** ("why did this happen?" answerable inline, §6.4). Every agent action is **labeled as an
   agent** and **attributed/audit-linked** like a human's.

**The strategy-pattern payoff (design-language §6 callout):** the *exact same* run list / single-run /
approval-card UI works whether the runtime is `MockAgentRuntime` today or `LlmAgentRuntime` later —
swapping the runtime changes **nothing** in the frontend.

## Flow 3 — Deploy gated on an issue transition (cross-subsystem trigger + HITL)

1. A PM moves an issue to *Deploy approved* in the issue tracker → `issue.transitioned`.
2. CI's trigger (`--on issue.transitioned --filter 'issue.status == "Deploy approved"'`, the shared
   query AST) matches → starts the `deploy` workflow.
3. The deploy hits a **protected-environment gate** → parks as a durable wait, surfaces a
   **deploy-approval card** in chat **and** an entry in the **Environments view → Approvals queue**.
4. Approve via `myelin ci deploy approve DEP-77`, the chat card, *or* an agent (uniform across sources
   — the agent-native payoff). The deploy proceeds; emits `ci.deployment.succeeded`; refs link
   run→issue ("deployed by RUN-X closes ISSUE-123"); notifications fan out; the issue can auto-transition.

**States:** *no deploys* → *awaiting-approval* (queue badge) → *deploying* (live) → *deployed* /
*rolled-back* / *failed*. Reversibility: rollback is a first-class action (`myelin ci deploy rollback`),
not a "are you sure?" — except the *deploy-to-prod* confirm, which IS a consequential/HITL gate (§8b.6
carve-out).

## Flow 4 — Shift-left: validate + plan before spending runner compute

1. Developer edits `.myelin/ci.yml` in the **Pipeline editor**; **schema validation + lint** run live
   (valid / schema-error with the offending line / lint-warning).
2. `myelin ci validate` (or the editor's Validate) — JSON-schema + lint, **no runner spend**.
3. `myelin ci plan --ref main` — shows the **resolved DAG + matrix expansion + which secrets are
   referenced** (and flags an **unknown-secret-reference** before a run wastes compute). This is the
   *same* content-addressed snapshot the run would pin (reproducibility).

**States:** *valid* → *plan-preview* (the DAG it would produce) → *schema-error* → *lint-warning* →
*unknown-secret-referenced* (caught pre-run).

## Flow 5 — Self-hosted runner registration + attestation (EU-enterprise path)

1. Admin runs `myelin ci runner register --pool eu-west --labels gpu,large`.
2. The runner **attests** (TPM / provisioning-signed token, sketch 05) → receives a **scoped job token**.
3. The **Runner fleet view** shows it: *pending-attestation* → *healthy* (capacity, jobs assigned) →
   *degraded* / *offline*. The admin sees attestation status as a first-class trust cue (P12/P15).

## Flow 6 — Data-subject erasure reaches CI (GDPR, cross-subsystem)

1. The platform DSR orchestrator fans out an `erase(subject)` to all holders (GDPR §4); CI is a holder.
2. CI's `PersonalDataHolder::erase` **crypto-shreds** the subject's PII in logs/artifacts/caches (key
   destroy) + **tombstones** identity fields in run metadata; run *structure* survives for audit (the
   delete-the-identity-not-the-fact rule, sketch 04).
3. Any chat/issue/doc unfurl of an affected run **degrades to a tombstone** (§5.10 erased state), never
   a dangling leak. The run's "triggered by" falls back to the opaque pseudonym.

**State:** the **erased/tombstoned** state is a designed state in every CI view that shows an actor or a
log (the run list, single-run, live-log "erased-by-DSR" banner, §5.10).

## Flow 7 — Cross-repo "is everything green?" + PM "Release Readiness" (the dual audience)

- **Engineer lens:** cross-repo **All Runs** dashboard — live, filterable (branch/status/actor/trigger),
  the "is main green across my repos?" view. Dense, keyboard-first (`j/k`, single-key retry/cancel).
- **PM/exec lens:** **Release Readiness** — the *same* deploy/health data presented as a spacious,
  chart-forward rollup (what's deployed where, what's blocked on approval, deploy frequency). Same data,
  different lens (design-language §2 persona-adaptivity) — not a separate product.

---

## Flow inventory ↔ event/contract grounding (the X-5 reconciliation cue)

| Flow | CI emits | CI consumes | Contracts used |
|---|---|---|---|
| 1 Push→checks | `ci.run.*`, `ci.status.updated`, `ci.log.available` | `git.pull_request.synchronized` | EventMatcher, outbox, `project`, refs, `ci.pipeline` workflow |
| 2 Agent triage | `ci.run.failed` (structured), `ci.log.available` | (wakes agent via Signal) | Signal, `EffectApi`, HITL gate (durable signal), reserve/settle, ToolDefs |
| 3 Deploy gate | `ci.deployment.*`, `ci.deployment.approval_required` | `issue.transitioned` | trigger (query AST), durable wait, approval card |
| 4 Validate/plan | (none — no run) | (none) | JSON-schema, `myelin ci validate/plan`, content-addressed snapshot |
| 5 Runner register | `ci.runner.registered` | (none) | `mint_run_token` (scoped job token), attestation |
| 6 Erasure | `ci.*.erased` tombstones | `*.erased` DSR fan-out | `PersonalDataHolder`, crypto-shred (Storage KMS) |
| 7 Release readiness | (read) | `ci.deployment.*`, `ci.run.*` (OLAP rollup) | OLAP read store (reindex-from-source), `list_objects` ACL filter |
