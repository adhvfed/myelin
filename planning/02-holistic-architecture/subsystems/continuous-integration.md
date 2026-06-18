# Phase 2 — Subsystem Architecture: Continuous Integration / CD

> Phase: `02-holistic-architecture`. Altitude: **high-level architecture** (VISION §5.2) — what
> the parts contain and how they interact, not full implementation. Canonical brief:
> [`VISION.md`](../../../VISION.md) (never contradicted). Phase-2 spine:
> [`architecture-decisions.md`](../architecture-decisions.md) (the ADRs),
> [`system-overview.md`](../system-overview.md) (the holistic narrative). Phase-1 deep-dive this
> builds on: [`01-research/subsystem-deep-dives/continuous-integration.md`](../../01-research/subsystem-deep-dives/continuous-integration.md)
> (cited as **CI-DD §n**). Structural foundation:
> [`01-research/technical-structuring.md`](../../01-research/technical-structuring.md) (**TS §n**).
>
> Deep technical detail (scheduler internals, isolation threat model, storage engines, config
> grammar) is **deferred to Phase 4 (CI)**; this doc commits the *structure* and flags the open
> items it inherits (CI-DD §11; ADR-15 TE-28/29/30/31/32).

---

## 0. One-paragraph summary

CI/CD is **the execution arm of the event fabric** (CI-DD §1): the one subsystem where an event
causes *sandboxed work to run* and where **untrusted, customer-supplied code executes**, making it
simultaneously the most operationally heavy and most security-sensitive subsystem. It owns the
**pipeline definition model, a distributed job scheduler, an elastic runner fleet, the
sandbox/isolation substrate, the run state machine, and the live-log/artifact/cache pipeline**. It
**delegates everything cross-cutting to the shared layer** — identity & authz, the event bus +
trigger engine, storage tiers, search, notifications, refs, agent fabric, GDPR/audit, durable
workflow — exactly per the platform thesis (`system-overview.md §4`). Its triggering model is
*uniform across all event sources* (git, schedule, issue transition, chat command, **agent
trigger**), which is the agent-native payoff (CI-DD §4). Two structural commitments from the spine
shape it most: the **firehose/control split** (ADR-04 — CI logs ride a dedicated transport, not the
durable bus) and the **agent ↔ CI execution-substrate convergence** (ADR-08 — a CI job and an agent
run are the same shape; whether they share one substrate is `[OPEN → P4]` TE-31).

---

## 1. Role & responsibilities — what CI/CD OWNS vs DELEGATES

### 1.1 Scope decision (resolving CI-DD §1, §11 PR-5)

**Phase-2 position: "CD" = deployment *mechanics*, not a hosted PaaS runtime** (CI-DD §1
assumption, §11 Q1). Myelin ships pipeline mechanics for deployment — deploy steps, environment
gating, approvals, promotion, rollback orchestration — that **call out to customer-owned or
third-party targets**. Myelin does **not** host customer apps in the Phase-1 build. Whether Myelin
later offers an EU-sovereign deploy target is a **commercial/product decision carried forward**
(ADR-15 commercial PR-5), and the architecture must not foreclose it (the deploy-step abstraction is
target-agnostic). *Rationale:* a PaaS runtime is a second product with its own world-scale and
sovereignty surface; bundling it now would dilute the five-subsystem thesis. Flagged, not foreclosed.

### 1.2 OWNS (core competency — TS §4.1, CI-DD §2/§5/§6)

| Owns | Why CI, not shared |
|---|---|
| **Pipeline definition model** — config-as-code (`.myelin/ci.*`), the resolution engine (matrix expansion, conditionals → concrete DAG), per-run **content-addressed definition snapshot** | Domain-specific semantics; pinning/reproducibility is CI's correctness property (CI-DD §2.2, §3) |
| **Distributed job scheduler** — match queued jobs → heterogeneous elastic fleet; fairness across tenants, priority lanes, concurrency groups, affinity, leases + heartbeats + dead-runner reaping, exactly-once assignment, graceful backpressure | A known-hard distributed-systems core (Borg/Nomad/Buildkite territory, CI-DD §5.2); the platform's single heaviest scheduling problem |
| **Runner fleet & elasticity** — pool management, scale-to-zero, cold-start/pre-warm, bin-packing, autoscale-on-queue-depth, on **EU-controlled infra**; **self-hosted runner** registration/attestation | Compute is the dominant cost; sovereignty constrains the provider menu (CI-DD §5.3, §6.1) |
| **Sandbox / isolation substrate** — one-job-per-sandbox ephemeral execution, microVM-class default for untrusted, trust-tier-gated capabilities, egress control | The untrusted-code boundary is CI's #1 problem (CI-DD §5.1, §7); also the agent execution boundary (ADR-08) |
| **Run state machine** — durable, crash-recoverable lifecycle (queued→assigned→running→terminal + manual-gate-waiting) with leases/heartbeats | Source of truth cannot live on a runner that can die mid-job (CI-DD §2.2) |
| **Live-log + artifact + cache pipeline** — live-tail fan-out, durable archive, range read, in-flight secret redaction, content-addressed dedup blobs, retention/TTL/GC | CI is the platform's heaviest storage consumer and the firehose source (CI-DD §5.4/5.5, §6.6) |
| **Trust-tier model & CI-local secret brokering** — trusted vs fork/untrusted vs self-hosted; gate secrets/cache-write/protected-envs/egress on tier; OIDC short-lived cred minting | The fork-untrusted footgun is CI-specific (CI-DD §7); secret *storage* is delegated (below) |

### 1.3 DELEGATES to the shared systems (TS §4.1; `system-overview.md §3/§4`)

| Delegated to | What CI gets / gives | Spine |
|---|---|---|
| **Identity & Access** | Authz on every trigger/cancel/approve/edit/secret action via `check`; **short-lived scoped per-run job tokens**; actor references (never copied PII); page/repo-tree ACL inheritance | ADR-03, ADR-13 |
| **Event Bus** | The *entire* trigger + status-emit model; transactional-outbox emission; at-least-once + idempotent-on-`event_id` consume; per-aggregate (per-run) ordering | ADR-04, ADR-13 |
| **Storage** | Three tiers — **OLTP** (run state), **log/firehose** (logs), **object/blob** (artifacts/caches, content-addressed, residency-pinned); per-tenant envelope encryption + crypto-shred | ADR-10, ADR-12 |
| **Agent Fabric** | Agents as trigger sources; `ci.run.failed` as a triage hook; agent runs in the (possibly shared) sandbox substrate; CI **registers ToolDefs** | ADR-08 |
| **Reference Graph** | run ↔ commit/PR ↔ issue ↔ chat ↔ knowledge edges via `ref.created` events | ADR-13 |
| **Search** | run/log/artifact indexing, ACL-pre-filtered ("find the run where test X first failed") | ADR-03, ADR-10 |
| **Notifications** | run failure/success, deploy-approval, quota alerts → the one prioritised inbox | ADR-12 |
| **Durable-workflow substrate** | manual-gate/approval waits (a run parked for *days* on a HITL signal); SLA-style timers | ADR-09 |
| **GDPR/Audit** | `PersonalDataHolder` registration; tamper-evident audit of triggers/approvals/secret-access/config changes | ADR-12 |
| **Scheduling/cron capability** | scheduled triggers — CI *consumes* the shared cron capability, does not reinvent it (CI-DD §4) | ADR-04 (trigger engine) |
| **Billing/quota/metering** | usage metering (the metered cost center), quota enforcement, abuse caps | `[OPEN → P4]` TE-32 |

**Hard rule (ADR-01/ADR-13):** CI never reads another subsystem's DB. Source checkout, commit
status, PR/fork-trust signals come from Git hosting **via shared contracts** (events + projection
API + job-token-scoped git wire access), not direct table reads.

---

## 2. High-level internal structure

Architecture altitude — major components and how they connect, not full impl. CI is best
understood as a **control plane** (durable, transactional, region-pinned) feeding an **execution
plane** (ephemeral, elastic, isolated), bridged by a **lease-based work queue**.

```
                         EVENT BUS (durable, per-run order, idempotent)        [ADR-04]
                              │ trigger matches (EventMatcher / query AST)      [ADR-07/08]
                              ▼
  ┌──────────────────────────────────────────────────────────────────────────────┐
  │  CI CONTROL PLANE   (Rust; region-pinned; transactional)                       │
  │                                                                                │
  │  ┌────────────┐   ┌──────────────┐   ┌───────────────┐   ┌──────────────────┐  │
  │  │ Trigger/   │──►│ Definition   │──►│ Run Planner    │──►│ Run State Machine │  │
  │  │ Dispatch   │   │ Resolver     │   │ (DAG/matrix    │   │ (durable, leases, │  │
  │  │ (filter,   │   │ (snapshot,   │   │  expansion,    │   │  heartbeats,      │  │
  │  │  loop/storm│   │  pin-by-     │   │  concurrency   │   │  dead-runner      │  │
  │  │  guard,    │   │  digest)     │   │  groups)       │   │  reaper)          │  │
  │  │  dedup)    │   └──────────────┘   └───────────────┘   └──────────────────┘  │
  │        │                                      │                    │           │
  │        │                                      ▼                    ▼           │
  │        │                            ┌──────────────────┐  ┌──────────────────┐ │
  │        │                            │ Distributed      │  │ Outbox emitter   │ │
  │        │                            │ Job Scheduler    │  │ (ci.* envelope)  │ │
  │        │                            │ (fair-share,     │  └──────────────────┘ │
  │        │                            │  priority lanes, │                       │
  │        │                            │  affinity, leases)│                      │
  │        │                            └────────┬─────────┘                       │
  │  ┌─────▼──────────┐  ┌───────────────┐       │       ┌──────────────────────┐  │
  │  │ Secret broker  │  │ Trust-tier    │       │       │ Log/Artifact/Cache   │  │
  │  │ (OIDC mint,    │  │ evaluator     │       │       │ pipeline coordinator │  │
  │  │  scope, mask)  │  │ (fork? env?)  │       │       │ (tail fan-out +      │  │
  │  └────────────────┘  └───────────────┘       │       │  archive + redact)   │  │
  └──────────────────────────────────────────────┼───────┴──────────────────────┘ │
                                                  │ lease-claim (pull) + heartbeat
  ┌───────────────────────────────────────────────▼──────────────────────────────┐
  │  CI EXECUTION PLANE   (elastic, ephemeral, EU-region-pinned)                   │
  │   Runner Pool Manager (autoscale, pre-warm, bin-pack, scale-to-zero)           │
  │     ├─ Runner ──► Sandbox (microVM-class default; one-job-per-sandbox)   [TE-28]│
  │     │             ├─ Workspace (checkout via job-token git wire)               │
  │     │             ├─ Step executor (shell | action/plugin | AGENT invocation)  │
  │     │             ├─ egress policy (block metadata/control-plane/cross-tenant) │
  │     │             └─ log shipper → firehose transport (NOT durable bus) [ADR-04]│
  │     └─ Self-hosted runners (customer infra; attested; semi-trusted)            │
  └───────────────────────────────────────────────────────────────────────────────┘
        │ artifacts/caches → object store     │ run state → OLTP     │ logs → log tier
        ▼ (S3-compat, content-addressed)      ▼ (Postgres)           ▼ (append+archive)
                              STORAGE (ADR-10) — all residency-pinned, crypto-shred-capable
```

### 2.1 Control-plane components

- **Trigger/Dispatch** — subscribes to the durable bus; cheap `EventMatcher` predicate eval
  (branch glob / path filter / label / payload) *close to the bus*, before any heavy planning
  (CI-DD §4); dedup on `event_id` (exactly-once *effect* over at-least-once delivery); **loop/storm
  guard** via `causation_id` depth caps (shared with ADR-08's agent loop machinery), `[skip ci]`
  markers, per-org rate limiting, concurrency dedup. Triggers are uniform across *all* sources.
- **Definition Resolver** — loads `.myelin/ci.*` at the triggering commit, resolves reusable
  components **pinned by content digest** (supply-chain), expands to an effective definition, and
  writes a **content-addressed snapshot** so the run is reproducible/auditable (CI-DD §2.2, §3).
- **Run Planner** — deterministic matrix expansion + conditional evaluation → the concrete job
  DAG (`needs` edges); resolves concurrency groups (cancel-superseded), secret references, cache
  keys. Determinism here is an audit requirement (CI-DD §3).
- **Run State Machine** — the durable, crash-recoverable source of truth (Postgres, ADR-14):
  lifecycle states + leases + heartbeats + **dead-runner reaper** (re-queue or fail orphaned jobs).
  This is "essentially a distributed job scheduler's persistence layer" (CI-DD §2.2).
- **Distributed Job Scheduler** — the hardest core (CI-DD §5.2): fair-share across tenants (no
  starvation), priority lanes (interactive PR checks > nightly batch), per-org/per-pipeline/
  per-env concurrency limits, affinity (GPU/ARM/large), exactly-once lease assignment, graceful
  backpressure. **Pull-based leasing** (runners claim work) is the directional model; final design
  is `[OPEN → P4]`.
- **Outbox emitter** — emits `ci.*` envelope events transactionally with state changes (ADR-04).
- **Secret broker / Trust-tier evaluator / Log-pipeline coordinator** — cross-cutting control
  services described in §2.3–2.4.

### 2.2 Execution-plane components

- **Runner Pool Manager** — autoscale on queue depth, pre-warm pools (cold-start vs cost tension,
  CI-DD §5.3/§5.8), bin-packing, scale-to-zero for idle orgs, spot/preemptible handling — **on
  EU-controlled infra** (Hetzner/OVH/Scaleway/sovereign-cloud/bare-metal; the exact menu is
  `[OPEN → P4]` TE-29).
- **Runner** — claims a leased job, heartbeats, drives the sandbox, ships logs to the firehose.
- **Sandbox** — **one-job-per-sandbox, ephemeral, never reused across tenants/jobs**;
  **microVM-class isolation (Firecracker/Cloud Hypervisor) is the conservative default for
  untrusted** (CI-DD §6.1/§7); a pluggable **executor strategy** allows hardened-container or
  full-VM variants. Final default + threat model is `[OPEN → P4]` TE-28.
- **Step executor** — a step is a shell command, an action/plugin, **or a strategy-pattern agent
  invocation** (ADR-08) — the executor abstraction is designed so "a step can be a command *or* an
  agent" from day one (CI-DD §1, §11 Q11).
- **Self-hosted runners** — semi-trusted nodes on customer infra calling the control plane;
  attestation + scoped job tokens; **non-negotiable for the EU-enterprise audience** (CI-DD §6.1).

### 2.3 Trust tiers & secrets (CI-DD §6.4, §7)

Trust tiers gate capabilities: **(a) trusted** (push by a write-access member), **(b) untrusted**
(PR from a fork / external contributor), **(c) self-hosted** (customer-trusted node). Secrets,
cache-write, protected-env access, and network egress are **gated on tier**. Untrusted runs get
**no secrets** by default (the canonical "fork exfiltrates prod secrets" CVE class). Secrets are
**stored/brokered by the shared secret capability**, injected at runtime, scoped (org/project/env),
masked in logs (best-effort defense-in-depth, not a boundary). **OIDC short-lived audience-scoped
credentials over long-lived static keys** is the target (CI-DD §6.4) — a strong EU-sovereign +
least-privilege fit.

### 2.4 The log/artifact/cache pipeline (the firehose — ADR-04)

This is where ADR-04's **firehose/control split is mandatory, not optional**. CI **log lines are a
firehose** and must **not** traverse the durable bus the same way control events do. The pipeline
provides: **live-tail** (low-latency SSE/websocket fan-out to many viewers) + **durable archive**
(object store) + **range read** + **search** + **in-flight secret redaction** + **GDPR erasure**
— different systems bridged by one API (CI-DD §5.4/§6.6). The durable bus carries only **pointer
events** (`ci.log.available`/`updated`), never one event per log line. Artifacts and caches are
**content-addressed, deduplicated, tenant-isolated, residency-pinned** blobs with TTL/GC; cache
writes from untrusted/fork runs **must not poison** trusted caches (scope by trust tier, CI-DD
§6.2).

---

## 3. Technology

Consistent with **ADR-02** (Rust default, justify divergence) and **ADR-14** (the directional
datastore map). CI/CD's ADR-14 row: *Rust; PG (run state) + object/log tiers; microVM isolation;
flagged divergence = runner elasticity on EU infra (TE-29).*

| Concern | Choice (Phase-2 directional) | Rationale / citation |
|---|---|---|
| **Control-plane language** | **Rust** | ADR-02 hot-path default; the scheduler/state-machine is a latency- and correctness-critical hot path explicitly named to stay Rust (ADR-02 "CI scheduler"). No divergence justification exists. |
| **Run-state datastore** | **Postgres-class (OLTP)** | ADR-10/ADR-14; small, transactional, frequently-queried run/job state with leases; portable + EU-self-hostable (ADR-11). |
| **Log/firehose store** | **Append-mostly tail+archive tier**; object store for cold archive; a low-latency fan-out path for live tail | ADR-04 firehose split, ADR-10 log tier; CI-DD §5.4. Concrete engine `[OPEN → P4]`. |
| **Artifact/cache store** | **S3-compatible** (MinIO/Ceph self-hostable; EU providers), content-addressed, dedup, residency-pinned | ADR-10 object tier; CI-DD §5.5. |
| **Isolation substrate** | **microVM-class (Firecracker/Cloud Hypervisor) default for untrusted**, via a **pluggable executor strategy** | CI-DD §6.1/§7; aligns with ADR-08's strategy-pattern mandate. Threat model + final default `[OPEN → P4]` TE-28. |
| **Runner agent (on the runner host)** | **Rust** | Single small attested binary; memory-safe at the trust boundary; same artifact for hosted + self-hosted. |
| **Trigger predicate eval** | shared **query AST / `EventMatcher`** (declarative, safe-to-evaluate, no Turing-complete hot-path predicates) | ADR-07/ADR-04; one matcher engine platform-wide. |
| **Config format** | **declarative, JSON-schema-validated, with an escape hatch to dynamic generation** | CI-DD §3 (lean), §11 Q2 (unresolved). *Selection criteria committed: agent-generatability + human-diffability are first-class* (CI-DD §3) — biases away from pure-programmatic. Final grammar `[OPEN → P4]`. |
| **OLAP (delivery-health analytics)** | **ClickHouse-class read store fed by the bus** | ADR-10/ADR-14 CQRS; PM-facing release-readiness analytics (CI-DD §10.2). |
| **Durable waits (approvals/gates)** | the **durable-workflow substrate** (ADR-09) | A deploy approval can park a run for *days* — durable-execution semantics, not a polling loop. |

**No divergence from the Rust default is justified for CI/CD.** Every component is either a hot
path (scheduler, state machine, runner) or a contract surface (outbox, ToolDefs). The only place CI
"diverges" from a *cloud-managed-services* baseline is by **constraint, not language**: ADR-11
forbids hyperscaler autoscaling primitives, so the runner fleet runs on EU-deployable/self-hostable
infra — the flagged hard problem (TE-29), not a language choice.

---

## 4. VIEWS / SCREENS the UI requires

Feeds the shared design-language work and the Phase-4 design sketches (VISION §3 — *design before
implementation*; each screen needs empty/loading/error states sketched first). Enumerated from
CI-DD Appendix A, refined to platform-coherent screens. **Logs/DAG/runner-fleet must be built on the
shared design language and the one views component (ADR-06) where applicable.**

| # | View | Purpose | Key states to design |
|---|---|---|---|
| 1 | **Run list / dashboard** | per-repo + cross-repo, filterable (branch/status/actor/trigger), live status — the "is main green?" view | empty (no runs), loading, live-updating, filtered-empty, error |
| 2 | **Single-run view** | DAG/stage visualization, per-job/step status + timing, **jump-to-failure**, retry/cancel, triggering cause, the pinned definition snapshot | queued, running (live), partial-failure, success, cancelled, timed-out, dead-runner-reaped |
| 3 | **Live log view** | streaming tail, **collapsible per step**, search-in-log, **secret-masked**, deep-link-to-line, downloadable | connecting, streaming, archived (cold), redaction-applied, truncated, erased-by-DSR |
| 4 | **Matrix view** | the fan-out grid, partial-failure highlighting | full pass, partial fail, in-progress mixed |
| 5 | **Pipeline / definition editor + validator** | edit config-as-code with **schema validation + lint + dry-run/plan** (show resolved DAG/matrix before running) | valid, schema-error, lint-warning, plan-preview, unknown-secret-referenced |
| 6 | **Environments & deployments view** | what's deployed where, history, **approvals queue**, rollback; **PM-friendly "release readiness"** framing (serves the issue-tracker PM audience) | no deploys, awaiting-approval, deploying, deployed, rolled-back, failed |
| 7 | **Secrets & variables management** | scoped (org/project/env), **audit of access**, rotation; set never echoes value | empty, scoped-view, rotation-due, access-audit trail |
| 8 | **Runner fleet / self-hosted runner mgmt** | registration, health, capacity, **job-assignment visibility**, attestation status | no runners, healthy, degraded, offline, pending-attestation |
| 9 | **Caches & artifacts browser** | retention/size, download, GC controls; reference-graph links out | empty, over-quota, expiring-soon, erased |
| 10 | **Usage / quota / billing view** | minutes/credits by repo/runner class; abuse caps | within-quota, near-limit, exceeded, throttled |
| 11 | **Triggers management view** | the cross-subsystem-event subscription model — what runs on which events, with filters | no triggers, active, paused, loop-guard-tripped |
| 12 | **Agent-surfaced run view** | failures formatted for agent/human triage; **an agent's proposed fix attached to a failed run**; HITL **approval card** surface (rendered in Chat per ADR-09) | failure-structured, agent-proposing, awaiting-approval, approved/rejected |

Cross-subsystem surfacing (not CI-owned screens, but CI feeds them): the **commit-status / checks
badge** on the Git PR view (the merge gate), CI status on linked **issues**, and run references
inside **chat** unfurls and **knowledge** runbooks.

---

## 5. CLI commands

A first-class, scriptable, **agent-friendly** surface under the unified `myelin` CLI, namespaced
`myelin ci …` (CI-DD Appendix B). **Every command supports `--json`** for agent/automation
consumption (CI-DD App B), authorizes via the one `Principal` (ADR-13), and respects residency
(commands route to the tenant's cell).

```bash
# Dispatch & observe
myelin ci run --workflow build --ref refs/heads/main --input env=staging
myelin ci list --repo acme/web --status failed --branch main --json
myelin ci watch RUN-991                       # live status of a run
myelin ci logs RUN-991 --job test --follow    # tail logs (firehose), secret-masked
myelin ci retry RUN-991 --failed-only
myelin ci cancel RUN-991

# Shift-left (no runner spend)
myelin ci validate .myelin/ci.yml             # JSON-schema validate + lint
myelin ci plan --ref main                     # resolved DAG + matrix expansion + referenced secrets

# Secrets & environments (set never echoes the value)
myelin ci secret set DEPLOY_TOKEN --scope project --repo acme/web
myelin ci secret list --scope env --env prod
myelin ci env list --repo acme/web
myelin ci deploy approve DEP-77               # resolve a HITL gate (also doable from chat)
myelin ci deploy rollback --env prod --to RUN-988

# Self-hosted runners
myelin ci runner register --pool eu-west --labels gpu,large
myelin ci runner list --json
myelin ci runner status RUNNER-12

# Artifacts & caches
myelin ci artifact list RUN-991 / download RUN-991 sbom.json / rm RUN-991 coverage
myelin ci cache list --repo acme/web / purge --key deps-linux-x86

# Triggers (the uniform cross-subsystem subscription model — the agent-native surface)
myelin ci trigger create --on issue.transitioned \
  --filter 'issue.status == "Deploy approved" && issue.project == "web"' \
  --workflow deploy --input env=prod
myelin ci trigger list / pause TRG-3 / rm TRG-3

# (Possibly, P4 open) local execution for fast iteration
myelin ci local build                         # [OPEN → P4] CI-DD §11 Q12
```

---

## 6. Usage examples (end-to-end)

### 6.1 Push → PR checks → merge gate (the Git ↔ CI seam, TS §4.2)

**UI flow.** A developer pushes to a PR branch. Git hosting emits `git.push` /
`git.pull_request.synchronized` to the bus. CI's Trigger/Dispatch matches the repo's
`on: pull_request` trigger (cheap filter close to the bus), dedups on `event_id`, resolves the
definition at the head commit (snapshot pinned), plans the DAG, and schedules jobs onto the EU
runner pool as **untrusted** (fork) or **trusted** (member) per the trust-tier evaluator. As jobs
run, the developer opens the **single-run view** and watches the **live log view** stream
(secret-masked). On completion CI emits `ci.status.updated`; Git hosting renders the **checks
badge** on the PR — green unblocks the merge button (branch protection: "require CI green").

**CLI / API.**
```bash
myelin ci list --repo acme/web --branch feat/login --json   # find the run
myelin ci watch RUN-991                                       # or watch live
myelin ci logs RUN-991 --job test --follow                    # jump-to-failure
myelin ci retry RUN-991 --failed-only                         # flaky test re-run
```
Events: CI **consumes** `git.pull_request.synchronized`; **emits** `ci.run.created/started`,
`ci.job.*`, `ci.status.updated`. Refs: `ref.created` run→PR→commit. No subsystem reads another's
DB — checkout is via a **scoped job token** over the git wire (ADR-13).

### 6.2 CI fail → agent triage → issue → chat → fix PR (the agent-native flagship, `system-overview.md §8.2`)

This is the spine's flagship walkthrough; CI is the *origin*. CI emits **structured** failure data
(`ci.run.failed` with which step, which test, log excerpt — a deliberate agent-native design goal,
CI-DD §8.2) — not just a log blob. A trigger wakes `MockTriageAgent` (on-behalf-of the pusher,
under a `RunBudget`); plan-then-apply: it *proposes* `issue.create` + `ref.create×2` + `chat.post`;
`EffectApi` validates against perms ∩ delegation ∩ tenant and applies. `issue.created` wakes
`FixAgent`, which proposes `git.open_pr` — **sensitive on a protected repo**, so Identity returns
**Gated → HITL**; the durable-workflow substrate opens a gate and surfaces a **chat approval card**.
A human approves (a durable signal, possibly *days* later); the workflow resumes and the PR opens.
One `correlation_id` throughout; loop depth capped; full audit provenance. **The same mock agent
code runs deterministically today and an `LlmAgentRuntime` later with zero platform changes** — the
strategy-pattern payoff (ADR-08).

### 6.3 Scheduled deploy gated on an issue transition (cross-subsystem trigger)

```bash
myelin ci trigger create --on issue.transitioned \
  --filter 'issue.status == "Deploy approved"' --workflow deploy --input env=prod
```
When a PM moves an issue to *Deploy approved* in the issue tracker, `issue.transitioned` hits the
bus; CI's trigger matches and dispatches the `deploy` workflow. The deploy run hits a
**protected-environment gate** (required approval); it parks as a durable wait, surfaces a
deploy-approval card in chat and an entry in the **environments view**'s approvals queue. On
approval (`myelin ci deploy approve DEP-77`, the chat card, *or* an agent), the deploy proceeds and
emits `ci.deployment.succeeded`; refs link run→issue ("deployed by RUN-X closes ISSUE-123");
notifications fan out; the issue tracker can auto-transition. **Uniform trigger model across
sources** — git, schedule, issue, chat, agent — is the agent-native payoff (CI-DD §4).

---

## 7. Interactions — events, refs, authz, search, notifications, agent tools, PersonalDataHolder

### 7.1 Events CI consumes (CI-DD §8.1)

`git.push`, `git.branch.created/deleted`, `git.tag.created`,
`git.pull_request.opened/updated/synchronized/merged/closed`, `git.review.submitted`,
`git.comment.command` (`/ci retry`); `schedule.fired` (shared cron); `issue.transitioned`,
`issue.labeled`; `knowledge.page.published` (runbook trigger); `chat.command.invoked`;
`agent.action.requested`; `ci.run.completed` (chained, loop-guarded);
`deployment.approval.granted/denied`; `identity.permission.changed` (revalidate in-flight authz);
`secret.rotated/revoked` (invalidate cached creds). *Exact dotted names are the Phase-3 taxonomy
deliverable (ADR-13); the shape is the contract.*

### 7.2 Events CI emits (CI-DD §8.2)

Lifecycle: `ci.run.created/started/succeeded/failed/cancelled/timed_out` + `ci.job.*` / `ci.step.*`.
**`ci.status.updated`** — the commit-status/checks contract that gates merges (the critical Git seam).
Firehose-pointer: `ci.log.available/updated` (logs themselves ride the dedicated transport, ADR-04),
`ci.artifact.published`, `ci.test_result.reported`, `ci.coverage.reported`. Deployment:
`ci.deployment.started/succeeded/failed/rolled_back`, `ci.deployment.approval_required`. Agent-
relevant: **`ci.run.failed` with structured failure** (the prime fix-it-agent hook). Resource:
`ci.quota.exceeded`, `ci.run.queued_too_long`. All via **transactional outbox**, idempotent on
`event_id`, per-run ordered (ADR-04).

### 7.3 Authz (ADR-03/ADR-13)

Every trigger/cancel/approve/edit/secret/runner action passes `check(principal, permission,
object)` against the one ReBAC engine — **humans, agents, services identically**. Per-run **scoped
job tokens** (least-privilege, short-TTL, bound to the one job/repo) are the runner→control-plane
and runner→git-wire credential. Protected environments and secret-by-tier gating compose with
ReBAC + ABAC-at-the-edge (e.g. "secret available only if trust_tier == trusted").

### 7.4 Refs, Search, Notifications

**Refs:** run ↔ commit/PR ↔ issue ↔ chat ↔ knowledge edges via `ref.created` (ADR-13); artifacts
are referenceable (a build linked from a chat message or release). **Search:** runs/logs/artifacts
indexed off the bus, **ACL-pre-filtered** via `list-objects` ("find the run where test X first
failed"). **Notifications:** run failure/success, deploy-approval, quota → the one prioritised
inbox; PM-friendly release-readiness framing (CI-DD §10.2), never a bespoke notifier.

### 7.5 Agent tools & triggers CI registers (ADR-08)

CI registers typed **`ToolDef`s** into the shared `ToolSurface` (name + JSON-schema input + required
caps + effect kind + side-effecting flag) — e.g. `ci.run_pipeline`, `ci.retry_run`, `ci.cancel_run`,
`ci.read_logs`, `ci.read_failure` (structured), `ci.approve_deploy`. Defined once, governed once,
**MCP-exposable** to external agents later. Triggers (`agent.action.requested` → run a pipeline)
flow through the **one trigger engine** (ADR-08). CI is also the **agent execution substrate**: a
step can be an agent invocation; whether CI's sandbox *is* the agent sandbox is `[OPEN → P4]` TE-31
— but the threat model is *shared* (an agent running tool calls is, security-wise, untrusted code,
CI-DD §7).

### 7.6 PersonalDataHolder duties (ADR-12)

CI is a **`PersonalDataHolder`** and a GDPR-spicy one — personal data leaks **incidentally** into
build artifacts, not just into obvious fields (CI-DD §9):
- **Where PII hides:** commit/PR author, "triggered by", "approved by" (direct); **logs** (the worst
  offender — emails/usernames/IPs/tokens/real fixtures, append-mostly + huge); artifacts; caches;
  self-hosted runner IPs/hostnames.
- **Design steer (committed): CI references identities, never copies PII** — actor stored as an id
  resolved via Identity; erasing the identity propagates (CI-DD §9, "strong design steer").
- **Erasure mechanism:** **crypto-shredding** logs/artifacts (per-tenant/per-subject keys destroyed
  → effective erasure of immutable/append-only stores) + **short default retention/TTL** (Art. 5
  storage-limitation, shrinks the erasure burden) + **tombstoning** identity fields in run metadata.
  This is exactly ADR-12's "keep PII out of immutable structures + crypto-shred." (Final granularity
  `[OPEN → P3]` GD-4 / `[OPEN — LEGAL]`.)
- `locate/export/rectify/restrict/erase` implemented over run state, logs, artifacts, caches.
- **Residency by construction (ADR-11):** an EU-resident tenant's run **executes on EU runners**
  with logs/artifacts/caches/state **in-region** — the scheduler + storage are pinned; **no global
  runner pool** (partitioned per residency zone). This is the platform's strongest data-residency
  argument because of what build compute touches (CI-DD §1, §5.6, §9).
- **Audit vs erasure tension:** audit retains pseudonymized actor + legal-basis-justified retention;
  PII resolves through Identity and is erasable there (shared with the issue tracker — coordinate).
- **Lawful basis:** build data processed under contract/legitimate interest; **must not be
  repurposed** (e.g. silently training models on customer build logs) without basis — flagged for
  the real-LLM era (CI-DD §9; AG-8/AG-9 `[OPEN — LEGAL]`).

---

## 8. Changes CI implies for the SHARED systems (flag for Phase 3)

CI is the heaviest/most-demanding consumer; it imposes concrete requirements on the shared layer.

1. **Event Bus — the firehose transport is a hard requirement, and CI is its primary driver
   (ADR-04).** Phase 3 must select a dedicated low-latency fan-out + append-archive transport for
   **CI log lines** (live-tail to many viewers + durable archive + range read + in-flight
   redaction), with the durable bus carrying only `ci.log.available` pointers. CI-DD §5.4/§8.3 says
   this is a top-3 platform storage challenge. *(P3: bus + firehose transport selection, TE-9/11.)*
2. **Event Bus / trigger engine — cheap predicate matching at firehose ingress.** Every push to
   every repo is an event; the `EventMatcher` (query AST, ADR-07) must evaluate branch/path/label
   filters **cheaply, close to the bus**, before any heavy planning (CI-DD §4). *(P3: `EventMatcher`
   predicate language, AG-7.)*
3. **Identity — short-lived, narrowly-scoped per-run job tokens** as a first-class token type
   (bound to one job/repo, attested for self-hosted runners), plus **OIDC federation token minting**
   for short-lived cloud/registry creds (CI-DD §6.4). *(P3: token model under ADR-03.)*
4. **Storage — three tiers exercised at their extremes by CI:** OLTP run state (leases at high
   churn), object store (content-addressed, dedup, residency-pinned artifacts/caches with locality
   near the runner), and the log/firehose tier. **Crypto-shred granularity must support
   per-subject/per-tenant erasure of logs/artifacts** (ADR-12). *(P3: storage engines + KMS
   hierarchy + crypto-shred granularity, GD-4.)*
5. **Durable-workflow substrate — deploy-approval/manual gates park runs for days** (ADR-09); CI is
   a primary consumer of durable timers + HITL signals alongside SLA timers. *(P3: build-vs-adopt,
   TE-20.)*
6. **Scheduling/cron capability — a shared platform cron** CI consumes for `schedule.fired` (CI must
   not reinvent cron, CI-DD §4). *(P3: where cron lives — bus/agent/control-plane.)*
7. **Secret management capability — scoped storage/brokering, rotation, access-audit, OIDC mint**
   (CI-DD §6.4) — a shared capability CI depends on; Phase 3 should place it (likely under
   Identity/GDPR). *(P3.)*
8. **Billing/quota/metering hooks** — CI is the metered compute cost center and abuse magnet; needs
   shared usage-metering + quota-enforcement + abuse-cap hooks (CI-DD §10.1, §5.9). *(P3/P4: TE-32.)*
9. **Agent Fabric — the CI sandbox as the agent execution substrate** (ADR-08); Phase 3 (Agents) +
   Phase 4 (CI) must jointly resolve the convergence depth (TE-31) and confirm the shared
   untrusted-code threat model.

---

## 9. Open questions for Phase 4 (CI detailed architecture)

Inherited from CI-DD §11 and the spine's `[OPEN → P4]` backlog (ADR-15), pruned to CI:

- **TE-28 — default isolation model.** microVM (Firecracker) vs hardened containers (gVisor) as the
  *default* untrusted boundary; needs a **security threat model + perf/cost study** (start-latency
  vs isolation tension). *Leaning microVM.*
- **TE-29 — runner ownership & EU infra.** Which EU-sovereign infra underpins the hosted fleet
  (Hetzner/OVH/Scaleway/Exoscale/bare-metal)? macOS/Windows targets (if at all)? Self-hosted runner
  trust/attestation model.
- **Config format & grammar.** Declarative + JSON-schema + dynamic-generation escape hatch is the
  lean (CI-DD §3); the concrete grammar, expression language, and reusable-component model are P4.
- **TE-30 — component/action registry.** Does Myelin host an EU-sovereign reusable-component
  registry, and its supply-chain trust model (pin-by-digest, signatures, SLSA provenance)?
- **TE-31 — CI ↔ agent substrate unification depth.** How far to merge the CI sandbox and the agent
  execution substrate (joint with Phase-3 Agents); security implications.
- **TE-32 — metering/billing unit.** Build-minutes vs credits vs resource-seconds per runner class
  — affects scheduler + quota design.
- **Scheduler internals.** Pull-leasing vs push-assignment; the fair-share algorithm; backpressure
  policy; priority-lane design — the hardest core (CI-DD §5.2).
- **Multi-region/residency execution boundaries.** Country vs EU vs per-tenant residency zones;
  cross-region opt-in semantics (CI-DD §11 Q9) — interacts with the multi-cell-tenant `[OPEN → P3]`
  (SC-2/SC-3).
- **Local execution (`myelin ci local`).** Developer-laptop pipeline runs — UX win vs
  maintenance/fidelity cost (CI-DD §11 Q12).
- **Diff-anchored log↔step structure & jump-to-failure UX** at firehose scale (design-language work).
- **[OPEN — LEGAL]** crypto-shred completeness for free-text PII in logs/artifacts (GD-6);
  build-data-as-LLM-training lawful basis (AG-8); CD-scope-as-PaaS product call (PR-5).

---

## 10. Cross-references

- [`VISION.md`](../../../VISION.md) — non-negotiables (world-scale, top UX, agent-native, GDPR/EU,
  Rust-default).
- [`architecture-decisions.md`](../architecture-decisions.md) — ADR-01..15; CI most touches ADR-04
  (firehose split), ADR-08 (agent substrate + tools), ADR-09 (HITL gates), ADR-10/14 (storage +
  tech), ADR-11 (residency), ADR-12 (PersonalDataHolder), ADR-13 (glue contracts).
- [`system-overview.md`](../system-overview.md) — §4 (owns/delegates), §7 (request/event
  lifecycles), §8.2 (the CI-origin agent-native flagship walkthrough).
- [`01-research/subsystem-deep-dives/continuous-integration.md`](../../01-research/subsystem-deep-dives/continuous-integration.md)
  — the research territory (§5 hardest problems, §6 isolation, §7 security, §8 events, §9 GDPR,
  §11 open questions, App A views, App B CLI).
- **Tightest seam:** Git hosting (the commit-status/checks merge gate, fork-trust signals — TS §4.2)
  — joint Phase-4 design.
