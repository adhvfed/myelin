# Issue Tracker — 00 · Overview, Role & Responsibilities

> **Phase 4, Stage 2 — detailed subsystem architecture.** This document set is the build-to design for the
> Issue Tracker subsystem of Myelin: the engineering board, the PM roadmap, and the corporate governance surface
> as **one product over one model**. It builds directly on this subsystem's Stage-1 output
> ([`../sketches/00-findings.md`](../sketches/00-findings.md) + the nine sketches) and
> ([`../design/`](../design/) — IA, flows, wireframes), and on the Wave-A
> [Knowledge architecture](../../knowledge-platform/architecture/) whose `myelin-content` + `myelin-query`
> primitives this subsystem co-owns (ADR-05/06). It never contradicts [`VISION.md`](../../../VISION.md) and is
> grounded in the Phase-3 contracts ([`contract-index.md`](../../../03-shared-systems-architecture/contract-index.md)).
> Date: 2026-06-19.

**Document split** (you are reading 00):
- **00 — Overview, role & responsibilities** (this doc): what Issues owns vs delegates; the component map; the
  three-axis model; the scaling posture; the doc index.
- **[01 — Tech & data model](./01-tech-and-data-model.md):** the language/DB choice with written justification;
  the full schema (typed core + JSONB tail + schemes + relations + rollups + SLA + triggers + import).
- **[02 — Internals & algorithms](./02-internals-and-algorithms.md):** the workflow interpreter, the AST→store
  query compiler + cost-bounding, Hi/Lo key allocation, LexoRank ranking + CAS, the rollup engine, the
  business-calendar SLA arithmetic, real-time sync.
- **[03 — Events, contracts & glue](./03-events-contracts-and-glue.md):** the complete `issue.*` taxonomy;
  every glue contract (ArtifactRef, `project`, `replay`, outbox, Identity `check`/`list_objects` + the ReBAC
  fragment, `PersonalDataHolder`, ToolDefs, reserve/settle).
- **[04 — Views, CLI & API](./04-views-cli-and-api.md):** the view catalogue mapped to the data model; the
  `myelin issue` CLI; the public/agent API surface.
- **[05 — Hard problems](./05-hard-problems.md):** each subsystem-specific hard problem resolved with cited
  prior art and named floors.
- **[06 — Shared-system change requests](./06-shared-system-change-requests.md):** the itemized list for Phase-5
  reconciliation.
- **[07 — Drills & open questions](./07-drills-and-open-questions.md):** the quantified PROVE-IT drills + the
  open questions handed to Phase 5.

---

## 1. The role: the work-coordination spine of Myelin

Issues is where work is **named, ranked, governed, related, and tracked to done** — for three audiences that
historically needed three different tools (Jira for corporate governance, Linear for engineers, a spreadsheet
or a PowerPoint for PMs). Myelin's thesis (VISION §2; design-language §2) is that **these are three lenses on
one model, not three products.** The make-or-break architectural bet of this subsystem is that the engineer's
**board** and the PM's **roadmap** are *co-equal views over one `issue` table* — not two object graphs that an
integration keeps in sync (sketch 01; resolved in [05 §1](./05-hard-problems.md)). Get that wrong and PMs get a
parallel reality, which is the exact failure the platform exists to kill.

Issues is also the subsystem the rest of Myelin **coordinates around**: a Git branch auto-transitions an issue,
a CI failure blocks a "Done" transition, a chat message becomes an issue, an SLA breach pages on-call, an agent
triages an incoming bug, a roadmap forecast flags a slipping initiative. It is the **most cross-subsystem-coupled
of the five subsystems** — which is precisely why it must build *only* on the shared contracts (no cross-DB
reads; everything through `project`/`resolve`/the bus) and never grow private back-channels.

### 1.1 The three independent axes (the structural core)

The single most important structural decision (sketch 01 / deep-dive §3.6) is that work is organised along
**three independent axes**, each landed on the primitive that already owns it — never collapsed into one tree:

| Axis | What it is | Primitive it lands on | Why separate |
|---|---|---|---|
| **Containment** (scope) | sub-task → story/bug/task/chore/spike → **epic → initiative** | one `issue` table; `parent` edge in `issue_relation` (TE-7 truth) | board↔roadmap co-equality lives here — it must be *one table* |
| **Time** | cycles / sprints, milestones / releases | a separate small `cycle` / `milestone` object; **membership** edge, not containment | a cycle has no workflow state/assignee; modelling it as an issue is awkward-nulls |
| **Org-scope** | team / project | the **identity `project`** object (Id §5) — *not re-invented* | it is the authz boundary + the human-key prefix owner; Identity owns it |

An issue is reachable from **all three at once** (it is in cycle 14, under epic SSO-Hardening, owned by team
ENG) and from cross-artifact context (a PR, a chat message). It is never "filed" in one tree; it is an
addressable `ArtifactRef` node projected into many views. This is the "reference everything" wedge made
structural (design-language §6; IA §2).

---

## 2. What Issues owns vs delegates

### 2.1 Owned (the source of truth Issues is authoritative for)

| Owned thing | Form | Authority note |
|---|---|---|
| **The `issue` table** — the ranked-type containment spine | typed-core columns + JSONB tail (sketch 03) | the work spine; board + roadmap both read it |
| **`issue.*` event taxonomy** | the complete list under Bus §6 grammar ([03 §1](./03-events-contracts-and-glue.md)) | Issues extends the Bus §6.2 seed; adds the `initiative` type token |
| **`issue_relation`** — the TE-7 typed relation table | source of truth for `parent`/`blocks`/`blocked_by`/`closes`/`depends_on`/`relates` | **frozen contract** (Refs §3.3 / ISS-1); Refs holds the projection |
| **Governance schemes** — workflow/field/permission/SLA/type | interpreted config rows, assigned per (type × team/project) | "Linear-simple = empty config; Jira-powerful = more schemes; one product" |
| **The workflow state-machine interpreter** | data-driven, guards as safe AST predicates | not codegen; not user-scripting |
| **Human-readable keys** (`ENG-1421`) | Hi/Lo batched allocator, gap-tolerant, monotonic | the public id in the ArtifactRef + CLI |
| **Drag-to-reorder rank** | LexoRank string + server-arbitrated CAS | aligned with Knowledge's `order_key` family |
| **The rollup aggregate** | derived materialised aggregate per ancestor, rebuildable by replay | edge truth stays in `issue_relation`; the rollup is derived |
| **The SLA *logic* engine** | policy + business-calendar arithmetic + pause/resume + escalation | the *timers* are delegated (SC-11) |
| **The Issues-side stateful-Trigger UX** | armable conditions reading Issues state | "Remind me when unblocked" — the flagship |
| **The AST→OLTP-store query compiler + cost-bounding** | our query planner | we own the planner; we consume the AST grammar |
| **Cycle / sprint / milestone objects** | small time-axis tables | membership edges, not containment |
| **The import engine + canonical interchange format** | two-pass, ID-remapped, idempotent | round-trips with the portability export |
| **Co-ownership of `myelin-query`** (ADR-06) | field defs + view model + AST grammar (Knowledge leads) | we contribute the issue-tracker storage discipline |

### 2.2 Delegated (consumed via contracts; never rebuilt)

| Delegated to | What Issues consumes | Contract |
|---|---|---|
| **Identity** | `authenticate` / `check` / `list_objects` / `list_subjects` / `write_tuples` / `resolve_pseudonym`+`erase`; the seeded `issue` ReBAC namespace | Id §4/§5/§8; contract 4.* |
| **Event Bus** | the envelope; `OutboxTx::emit` (the only emit path); the consumer template; `EventMatcher`; `arm_trigger`; reindex-from-source | Bus §3/§4/§5/§6; contract 2.*/3.* |
| **Refs** | `ArtifactRef` parse/format; `resolve` (the context pane unfurl); `backlinks`/`traverse`; the typed-edge mirror | Refs §3/§4/§5; contract 5.* |
| **Search** | `query`/`semantic`/`declare_indexable`; the escalation valve for cold/ad-hoc/full-text | Search §4/§5; contract 6.* |
| **Notifications** | `list_inbox` (the "My Work" scoped view — C-9); `humanise`; `oncall_now`/`page`; `define_notif_rule` | Notif §1.3/§3/§4; contract 7.* |
| **Agent Fabric** | `register_tool` (the ToolDefs humans+agents share); `EffectApi`; the plan-then-apply loop; reserve/settle | Agent §5/§6/§7; contract 8.* |
| **Durable Workflow** | the **timer wheel** (SLA timers + `stale_after` + snooze ride SC-11); durable signals (multi-day HITL); escalation as a durable workflow | Workflow §3/§4/§5; contract 9.* |
| **Storage** | the OLTP pool (one DB per service); `BlobStore` (attachments); the **OLAP read store** (CQRS analytics); KMS; backup/restore | Storage §3/§4/§7; contract 11.* |
| **GDPR/Audit** | the DSR orchestrator; the classify derive; the tamper-evident audit log (Issues contributes attribution, not the log) | GDPR §3/§4/§6; contract 10.* |
| **Tenancy** | `(tenant, region)` partition key; `discover`/`place`/`residency_verify`; the isolation spectrum | Tenancy §6/§8/§12; contract 12.* |

**The delegation discipline:** Issues is a **thin shell over identical plumbing** (Phase-3 README §1). It calls
`serve(AppSpec)`, supplies handlers + migrations + consumer registrations, and inherits the outbox, idempotent
consumers, tenant-scoping, the resilient client, the protected human lane, fail-static, the three ports,
`PersonalDataHolder` auto-registration — none of which it may reimplement or skip.

---

## 3. Internal component architecture

Issues is one logical service (the `issue` subsystem) with one OLTP database (the `no-cross-db` boundary,
ADR-01) and a set of **bus consumers** that maintain derived state. The components, all in Rust unless noted:

```
                         ┌─────────────────────────── PUBLIC SURFACE (gateway-fronted, identity-injected) ──┐
   UI (React) ─────────► │  IssueApi (public RPC)   │  myelin issue CLI  │  ToolDefs (agent/MCP via Agent)  │
   firehose subscribe ◄─ │  (one API; UI=CLI=agent parity — no privileged back-channel)                    │
                         └───────────────────────────────┬───────────────────────────────────────────────┘
                                                          │  every write: Id.check → mutate + OutboxTx::emit (one tx)
   ┌──────────────────────────────────────────────────────▼──────────────────────────────────────────────┐
   │                                       ISSUE SERVICE (Rust)                                            │
   │  ┌────────────────┐  ┌────────────────────┐  ┌──────────────────┐  ┌───────────────────────────────┐ │
   │  │ Write path     │  │ Workflow           │  │ Query planner    │  │ Scheme resolver               │ │
   │  │ - validate     │  │ interpreter        │  │ (AST→store        │  │ - precedence algebra (cached) │ │
   │  │ - Id.check     │  │ - data-driven FSM  │  │  compiler;        │  │ - workflow/field/perm/SLA/    │ │
   │  │ - Hi/Lo key    │  │ - safe-AST guards  │  │  cost-bounding;   │  │   type schemes per (type×proj)│ │
   │  │ - LexoRank CAS │  │ - post-actions     │  │  Search escalate) │  └───────────────────────────────┘ │
   │  │ - outbox emit  │  └────────────────────┘  └──────────────────┘  ┌───────────────────────────────┐ │
   │  └────────────────┘  ┌────────────────────┐  ┌──────────────────┐  │ SLA engine                    │ │
   │  ┌────────────────┐  │ Trigger/automation │  │ Import engine    │  │ - business-calendar arith     │ │
   │  │ Relation writer│  │ - armable conds    │  │ - 2-pass ID-remap │  │ - precompute fire_at          │ │
   │  │ (issue_relation│  │ - EventMatcher     │  │ - canonical IR    │  │ - pause/resume re-arm         │ │
   │  │  = truth)      │  │ - on bus primitives│  │ - reconcile report│  │ (rides SC-11 timer wheel)     │ │
   │  └────────────────┘  └────────────────────┘  └──────────────────┘  └───────────────────────────────┘ │
   │                                                                                                       │
   │  OLTP DB (one per service):  issue · issue_relation · issue_change_log · scheme tables · cycle ·      │
   │     milestone · rollup · sla_instance · trigger · prefix_counter · import_map · projection feeder ·   │
   │     consumer_dedup · OUTBOX (the cross-seam anchor)                                                   │
   └──────────────────────────────────────────────────────┬────────────────────────────────────────────────┘
                              outbox relay (FOR UPDATE SKIP LOCKED) → BUS
   ┌──────────────────────────────────────────────────────▼────────────────────────────────────────────────┐
   │  BUS CONSUMERS (the substrate consumer template; idempotent on event_id; bounded prefetch; lag metric) │
   │  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐ ┌──────────────┐ ┌────────────────────────────┐   │
   │  │ Rollup       │ │ SLA driver   │ │ Trigger      │ │ Cross-sub    │ │ Projection feeder          │   │
   │  │ consumer     │ │ consumer     │ │ resolver     │ │ consumer     │ │ (promote hot custom facet  │   │
   │  │ (debounced,  │ │ (arm/disarm  │ │ (EventMatcher│ │ (git.*/ci.*/ │ │  to a generated index off  │   │
   │  │  incremental)│ │  on state)   │ │  watch)      │ │  chat.*/id.*)│ │  the bus; measured-promote)│   │
   │  └──────────────┘ └──────────────┘ └──────────────┘ └──────────────┘ └────────────────────────────┘   │
   └─────────────────────────────────────────────────────────────────────────────────────────────────────────┘
        derived stores rebuilt by reindex-from-source (replay → *.snapshot → the SAME consumer path):
        Search index ·  Refs edge projection ·  OLAP read store (CFD/cycle-time/velocity) ·  Notif inbox
```

**Why this shape:**
- **The write path is minimal and fast** — validate, `Id.check`, allocate key/rank, mutate the typed core,
  `OutboxTx::emit` in the *same transaction*. Everything heavy (rollup, SLA arming, search indexing, ref edges,
  analytics) is **off the bus, async** (ADR-11.5 in-cell-first; sketch 05). A leaf change never blocks on an
  ancestor walk.
- **The workflow interpreter and scheme resolver are config-driven and cached** — assigning a scheme is a config
  write, never a data migration (sketch 02). The hot transition path loads a small, cached, compiled scheme.
- **The query planner owns the AST→store compilation + cost-bounding** — it decides typed-column scan vs GIN vs
  generated index vs Search escalation, and bounds every query (sketch 03; [02 §3](./02-internals-and-algorithms.md)).
- **Derived state is rebuildable** — the rollup aggregate, the Search projection, the Refs edge projection, the
  OLAP read store are all rebuilt by reindex-from-source (`replay` → `*.snapshot` → the live consumer path), so
  steady-state and recovery share one code path and cannot drift (Phase-3 README §1, invariant 6).

---

## 4. Scaling & sharding within the cell topology

Issues inherits the cell model (ADR-11; Tenancy): a **cell** is a complete region-pinned stack; scale = add
cells; `(tenant, region)` is the first column of every table and the partition key. The Issues-specific scaling
posture and hot-spots (detailed in [02 §8](./02-internals-and-algorithms.md)):

| Concern | Posture | Hot-spot & mitigation |
|---|---|---|
| **OLTP** | one Postgres-class DB per service, sharded by tenant; the floor (sketch 03) | a hot tenant's `issue` table → tenant-shard split; distributed-SQL is the *measured* follow-on, never premature |
| **Human-key allocation** | per-prefix Hi/Lo batched (sketch 04) | a create-storm on a hot prefix (incident, import) → larger adaptive block size; per-prefix isolation (busy `ENG` doesn't slow `OPS`) |
| **Board/list query** | typed-core index range scan; cost-bounded | a large-custom-field board → generated index off the bus; cold/ad-hoc → Search escalation (never an unbounded JSONB scan) |
| **Rollup fan-out** | event-driven, debounced, incremental | a leaf under a 50-team initiative → debounce coalescing + per-tenant in-flight caps (X-3) + `input_hash` no-op suppression |
| **SLA timers** | ride the SC-11 minute-bucket wheel | millions of far-future timers cost an indexed range read; precompute `fire_at`, never poll |
| **Drag-reorder** | LexoRank O(1) per move + CAS | heavy concurrent reorder of one region → jittered inserts + region-local background rebalance (CAS floor; CRDT is the named follow-on) |
| **Import** | per-tenant in-flight caps (X-3) | a giant import (100k+ issues) → bounded backfill + fairness shed (protected human lane); never starves another tenant |
| **Real-time sync** | bus-driven cache invalidation over the shared firehose | a huge board's event stream → per-view subscription scope bounding + resume-cursor on reconnect (reuse KN-1) |

**Multi-cell** (SC-2/SC-3): a prefix belongs to a team, a team to a project, a project to a cell — so a prefix
lives in exactly one cell; key allocation is cell-local (no cross-region coordination). Cross-cell portfolio
rollup (an initiative whose children span cells) rides the PII-free pointer bridge (Tenancy §10) and is the
**named floor** — single-cell is the complete v1 (matches the Phase-3 multi-cell floor).

---

## 5. Where the design proves itself (forward pointers)

The make-or-break properties and the drills that prove them (full list in
[07](./07-drills-and-open-questions.md)):
- **Co-equal-view consistency** — edit on the board ⇒ the roadmap reflects the same `issue` row, no drift
  (because they are the same rows): a chained-mutation E2E.
- **Flexible-field query latency** — a large-custom-field tenant board query under the <1s keyboard budget; a
  cold ad-hoc query escalates to Search, never an unbounded OLTP scan.
- **Permission-leak (confidential)** — a cross-tenant + confidential-issue IDOR drill: zero leak via
  board/search/backlink.
- **SLA breach durability** — breach fires after a restart (the SC-11 rider) + a business-calendar arithmetic
  corpus (DST/holiday/multi-day).
- **Trigger fires-once-after-restart** — `stale_after` + resolve.
- **Concurrent-reorder** — N humans + an agent re-ranking one region: zero silent clobber, bounded re-base.
- **Import round-trip** — export→import→export round-trips; a large import resumes after crash with no
  duplicates; it doesn't starve other tenants.
- **Erasure reaches every holder** — change-log, comments, attachments, OLAP, Search, the Refs projection.

Continue to [`01-tech-and-data-model.md`](./01-tech-and-data-model.md).
