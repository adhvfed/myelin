# Issue Tracker — Subsystem Design & Architecture (Phase 5-B)

> The Issue Tracker subsystem of Myelin: the engineering **board**, the PM **roadmap**, and the corporate
> **governance** surface as **one product over one model**. EU-sovereign, GDPR-by-construction, agent-native,
> top-tier-UX. This README indexes the Phase-4 design record (Stage 1, PRESERVED) + the **detailed architecture
> rewritten in Phase 5-B against the RECONCILED shared layer**. Canonical brief: [`VISION.md`](../../../VISION.md).
> Build-to surface (frozen): the Phase-5
> [`contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) (supersedes Phase 3) +
> [`00-reconciliation-decisions.md`](../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md).
> Date: 2026-06-19. The Phase-4→Phase-5 delta table is in
> [`architecture/00-overview.md`](./architecture/00-overview.md) §0.

---

## The one-paragraph thesis

Work is organised along **three independent axes** — **containment** (sub-task → story → epic → initiative, one
`issue` table, `parent` edge), **time** (cycles/milestones, a separate object), and **org-scope** (the identity
`project`) — never collapsed into one tree. The make-or-break bet is that the **board and the roadmap are
co-equal `myelin-query` AST views over the one `issue` table**, so they *structurally cannot drift*. Governance
is **layered schemes interpreted as config** (Linear-simple = empty config; Jira-powerful = more schemes; one
product, no fork, no migration), with the **fixed state-category set** as the one mandatory invariant. The whole
thing is **Rust over PostgreSQL** (typed core + JSONB tail + a derived projection — no per-tenant DDL, no JQL
trap), built on the **frozen** shared substrate (the bus, Identity's ReBAC + the `SetExpr` push-down over
`issue.id`, the SC-11 timer wheel, the one Notif inbox, the frozen `myelin-content`/`myelin-query` crates, the
agent fabric) with named floors and measured promotions.

---

## Architecture (the Phase 5-B build-to design, rewritten against the reconciled layer)

| Doc | Covers |
|---|---|
| [`architecture/00-overview.md`](./architecture/00-overview.md) | **The Phase-4→Phase-5 delta table (§0)**; role & responsibilities; owns-vs-delegates; the three-axis model; the component map; scaling/sharding + hot-spots; the doc index. |
| [`architecture/01-tech-and-data-model.md`](./architecture/01-tech-and-data-model.md) | **Rust + PostgreSQL** (carried forward + confirmed); the full schema — typed-core+JSONB `issue` spine, schemes, `issue_relation` (TE-7 truth), change-log, cycles/milestones, rollup, SLA, triggers, Hi/Lo keys (the frozen `<PROJECTKEY>-<seqno>` id), the frozen worklog tags, import map, the stateful-component register. |
| [`architecture/02-internals-and-algorithms.md`](./architecture/02-internals-and-algorithms.md) | Scheme-precedence algebra; the workflow interpreter (frozen `QueryAst` guards); the **AST→store compiler that lowers the frozen `SetExpr` push-down + cost-bounding + projection feeder**; Hi/Lo allocation; the **frozen `order_key` LexoRank** + CAS; the rollup engine; the **business-calendar SLA arithmetic**; real-time sync over the frozen firehose protocol. |
| [`architecture/03-events-contracts-and-glue.md`](./architecture/03-events-contracts-and-glue.md) | The complete `issue.*` taxonomy + consumed events (incl. the frozen `CheckStatus`); **every glue contract against the frozen shapes** — ArtifactRef+unified `#sub`, `project`, `replay`, the outbox, Identity `check`(+`CaveatContext`)/`list_objects`(`SetExpr`)+the ReBAC fragment, `PersonalDataHolder`+the ONE erasure posture by reference, ToolDefs+frozen `requires_approval` defaults, reserve/settle, the stateful Trigger. |
| [`architecture/04-views-cli-and-api.md`](./architecture/04-views-cli-and-api.md) | The view catalogue (each = a frozen `ViewSpec` over one table); the `myelin issue` CLI; the public/agent API + tool surface. |
| [`architecture/05-hard-problems.md`](./architecture/05-hard-problems.md) | Each hard problem resolved with **cited prior art** + named floors (PR-2/PR-3/TE-17/TE-14/TE-19/TE-7/TE-18/SLA/sync/PR-8 + the ONE erasure posture by reference). |
| [`architecture/06-reconciliation-compliance.md`](./architecture/06-reconciliation-compliance.md) | How this subsystem **implements** the frozen reconciled contracts (the `SetExpr` push-down, `myelin-content`, `myelin-query`, the `#sub` grammar, the REF-3 key reconciliation, `CheckStatus` consumption, the erasure posture) + the residual asks for Phase 6 (all five Phase-4 blocking items granted). |
| [`architecture/07-drills-and-open-questions.md`](./architecture/07-drills-and-open-questions.md) | The quantified PROVE-IT drills (D1…D14); the named floors; the open questions handed to Phase 6 (the Phase-4 questions resolved by reconciliation). |

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
`issue_relation` (TE-7 source of truth) · the governance schemes + the workflow interpreter · human keys (Hi/Lo,
the frozen `<PROJECTKEY>-<seqno>` id) · the frozen `order_key` rank · the rollup aggregate · the SLA *logic*
engine + the frozen escalation chain · the Issues-side stateful-Trigger UX (frozen `QueryAst` condition) · the
AST→store query compiler + cost-bounding + the `SetExpr` lowering · cycles/milestones · the import engine +
canonical interchange (consuming the frozen ADF map) · co-ownership of `myelin-query` (ADR-06, Knowledge leads).

**Delegates (via the frozen contracts, never rebuilt):** Identity (`check`+`CaveatContext` / `list_objects`
`SetExpr` push-down / the `issue` ReBAC fragment) · the Bus (envelope/outbox/the `QueryAst` `EventMatcher`/
`arm_trigger`/reindex/the firehose resume-cursor protocol) · Refs (ArtifactRef/`resolve`/the unified `#sub`
grammar/the typed-edge mirror) · Search (the cold/ad-hoc/full-text valve, ACL-pre-filtered) · Notif (the "My
Work" `list_inbox` view — C-9; the ONE templating surface) · Agents
(ToolDefs/`EffectApi`/reserve-settle) · Workflow (the SC-11 timer wheel for SLA + `stale_after`; durable HITL
signals) · Storage (OLTP/`BlobStore`/OLAP/KMS) · GDPR/Audit (DSR/classify/the tamper-evident log) · Tenancy
(the partition key / residency / multi-cell).

## The five make-or-break properties (each with a drill)

1. **Co-equal-view consistency** (D1) — board ⇄ roadmap are the same rows.
2. **Flexible-field query never recreates the JQL trap** (D2) — typed core + bounded planner + Search valve.
3. **Zero permission leak** (D3) — the confidential exclusion is by-construction.
4. **SLA breach durability + calendar correctness** (D6).
5. **Trigger fires-once-after-restart** (D7) — "Remind me when unblocked."
