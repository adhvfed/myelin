# Issue Tracker — Subsystem Design & Architecture (Phase 4)

> The Issue Tracker subsystem of Myelin: the engineering **board**, the PM **roadmap**, and the corporate
> **governance** surface as **one product over one model**. EU-sovereign, GDPR-by-construction, agent-native,
> top-tier-UX. This README indexes the full Phase-4 design (Stage 1) + detailed architecture (Stage 2). Canonical
> brief: [`VISION.md`](../../../VISION.md). Build-to surface: the Phase-3
> [`contract-index.md`](../../03-shared-systems-architecture/contract-index.md). Date: 2026-06-19.

---

## The one-paragraph thesis

Work is organised along **three independent axes** — **containment** (sub-task → story → epic → initiative, one
`issue` table, `parent` edge), **time** (cycles/milestones, a separate object), and **org-scope** (the identity
`project`) — never collapsed into one tree. The make-or-break bet is that the **board and the roadmap are
co-equal `myelin-query` AST views over the one `issue` table**, so they *structurally cannot drift*. Governance
is **layered schemes interpreted as config** (Linear-simple = empty config; Jira-powerful = more schemes; one
product, no fork, no migration), with the **fixed state-category set** as the one mandatory invariant. The whole
thing is **Rust over PostgreSQL** (typed core + JSONB tail + a derived projection — no per-tenant DDL, no JQL
trap), built on the shared substrate (the bus, Identity's ReBAC, the SC-11 timer wheel, the one Notif inbox, the
agent fabric) with named floors and measured promotions.

---

## Architecture (Stage 2 — the build-to design)

| Doc | Covers |
|---|---|
| [`architecture/00-overview.md`](./architecture/00-overview.md) | Role & responsibilities; owns-vs-delegates; the three-axis model; the component map; scaling/sharding + hot-spots; the doc index. |
| [`architecture/01-tech-and-data-model.md`](./architecture/01-tech-and-data-model.md) | **Rust + PostgreSQL** choice (written justification); the full schema — typed-core+JSONB `issue` spine, schemes, `issue_relation` (TE-7 truth), change-log, cycles/milestones, rollup, SLA, triggers, Hi/Lo keys, import map, the X-4 stateful-component register. |
| [`architecture/02-internals-and-algorithms.md`](./architecture/02-internals-and-algorithms.md) | Scheme-precedence algebra; the workflow interpreter (safe-AST guards); the **AST→store query compiler + cost-bounding + projection feeder**; Hi/Lo allocation; LexoRank+CAS; the rollup engine; the **business-calendar SLA arithmetic**; real-time sync. |
| [`architecture/03-events-contracts-and-glue.md`](./architecture/03-events-contracts-and-glue.md) | The complete `issue.*` taxonomy + consumed events; **every glue contract** — ArtifactRef+`#sub`, `project`, `replay`, the outbox, Identity `check`/`list_objects`+the ReBAC fragment, `PersonalDataHolder`, ToolDefs, reserve/settle, the stateful Trigger. |
| [`architecture/04-views-cli-and-api.md`](./architecture/04-views-cli-and-api.md) | The view catalogue (each = an AST projection over one table); the `myelin issue` CLI; the public/agent API surface. |
| [`architecture/05-hard-problems.md`](./architecture/05-hard-problems.md) | Each hard problem resolved with **cited prior art** + named floors (PR-2/PR-3/TE-17/TE-14/TE-19/TE-7/TE-18/SLA/sync/PR-8 + the GD-6 residual). |
| [`architecture/06-shared-system-change-requests.md`](./architecture/06-shared-system-change-requests.md) | The itemized CR-1…CR-16 list for Phase-5 reconciliation (5 blocking; no ADR reversal). |
| [`architecture/07-drills-and-open-questions.md`](./architecture/07-drills-and-open-questions.md) | The quantified PROVE-IT drills (D1…D13); the named floors; the open questions by resolver. |

## Design (Stage 1 — sketches + UX)

| Doc | Covers |
|---|---|
| [`sketches/00-findings.md`](./sketches/00-findings.md) | The Stage-1 closing: committed decisions per hard problem, the floors, the PROVE-IT drill list, the open questions handed to architecture. |
| [`sketches/01-…`](./sketches/01-issue-model-duality.md) … [`09-…`](./sketches/09-import-fidelity.md) | The nine exploration notes (candidates weighed, prior art, leanings) for each hard problem. |
| [`design/information-architecture.md`](./design/information-architecture.md) | The one-shell fit; the three-axis nav; the screen inventory (S1…S19). |
| [`design/user-flows.md`](./design/user-flows.md) | Core human flows (A1–A5), agent/HITL flows (B1–B4), cross-subsystem flows (C1–C4), designed non-happy states. |
| [`design/wireframes.md`](./design/wireframes.md) | ASCII wireframes of the primary screens with empty/loading/error/permission/erased/agent-pending states. |

---

## What Issues owns vs delegates (the one-glance map)

**Owns:** the `issue` table (ranked-type spine) · the `issue.*` taxonomy (+ the `initiative` token) ·
`issue_relation` (TE-7 source of truth) · the governance schemes + the workflow interpreter · human keys (Hi/Lo)
· LexoRank rank · the rollup aggregate · the SLA *logic* engine · the Issues-side stateful-Trigger UX · the
AST→store query compiler + cost-bounding · cycles/milestones · the import engine + canonical interchange ·
co-ownership of `myelin-query` (ADR-06, Knowledge leads).

**Delegates (via contracts, never rebuilt):** Identity (`check`/`list_objects`/the seeded `issue` namespace) ·
the Bus (envelope/outbox/`EventMatcher`/`arm_trigger`/reindex) · Refs (ArtifactRef/`resolve`/the typed-edge
mirror) · Search (the cold/ad-hoc/full-text valve) · Notif (the "My Work" scoped view — C-9) · Agents
(ToolDefs/`EffectApi`/reserve-settle) · Workflow (the SC-11 timer wheel for SLA + `stale_after`; durable HITL
signals) · Storage (OLTP/`BlobStore`/OLAP/KMS) · GDPR/Audit (DSR/classify/the tamper-evident log) · Tenancy
(the partition key / residency / multi-cell).

## The five make-or-break properties (each with a drill)

1. **Co-equal-view consistency** (D1) — board ⇄ roadmap are the same rows.
2. **Flexible-field query never recreates the JQL trap** (D2) — typed core + bounded planner + Search valve.
3. **Zero permission leak** (D3) — the confidential exclusion is by-construction.
4. **SLA breach durability + calendar correctness** (D6).
5. **Trigger fires-once-after-restart** (D7) — "Remind me when unblocked."
