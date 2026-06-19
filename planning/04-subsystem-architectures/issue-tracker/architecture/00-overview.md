# Issue Tracker — 00 · Overview, Role & Responsibilities

> **Phase 5-B — detailed subsystem architecture, rewritten against the RECONCILED shared layer.** This document
> set is the build-to design for the Issue Tracker subsystem of Myelin: the engineering board, the PM roadmap,
> and the corporate governance surface as **one product over one model**. It carries forward the sound Phase-4
> first pass ([the design record](../design/) — IA, flows, wireframes — PRESERVED; the nine
> [sketches](../sketches/) — PRESERVED) and conforms to **every** Phase-5 reconciliation decision and the
> **frozen** contract surface:
> [`00-reconciliation-decisions.md`](../../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md)
> and [`contract-index.md`](../../../05-refined-shared-systems-architecture/contract-index.md) (which **supersedes**
> the Phase-3 index). It never contradicts [`VISION.md`](../../../VISION.md) and obeys the doctrine
> ([`02-platform-substrate`](../../../external-insights/02-platform-substrate.md),
> [`04-hard-problems`](../../../external-insights/04-hard-problems.md),
> [`05-ux-and-design`](../../../external-insights/05-ux-and-design.md)). Date: 2026-06-19.

**Document split** (you are reading 00):
- **00 — Overview, role & responsibilities** (this doc): the changes vs the Phase-4 first pass; what Issues owns
  vs delegates; the component map; the three-axis model; the scaling posture; the doc index.
- **[01 — Tech & data model](./01-tech-and-data-model.md):** the language/DB choice (carried forward + confirmed);
  the full schema (typed core + JSONB tail + schemes + relations + rollups + SLA + triggers + import).
- **[02 — Internals & algorithms](./02-internals-and-algorithms.md):** the workflow interpreter, the AST→store
  query compiler + cost-bounding, Hi/Lo key allocation, the frozen LexoRank `order_key` encoding + CAS, the
  rollup engine, the business-calendar SLA arithmetic, real-time sync over the frozen firehose protocol.
- **[03 — Events, contracts & glue](./03-events-contracts-and-glue.md):** the complete `issue.*` taxonomy; every
  glue contract implemented against the **frozen shapes** (ArtifactRef + `#sub`, `project`, `replay`, outbox,
  Identity `check`/`list_objects` `SetExpr` + the ReBAC fragment + `CaveatContext`, `PersonalDataHolder`,
  ToolDefs + frozen `requires_approval` defaults, reserve/settle).
- **[04 — Views, CLI & API](./04-views-cli-and-api.md):** the view catalogue mapped to the data model; the
  `myelin issue` CLI; the public/agent API/tool surface.
- **[05 — Hard problems](./05-hard-problems.md):** each subsystem-specific hard problem resolved with cited prior
  art and named floors.
- **[06 — Reconciliation compliance](./06-reconciliation-compliance.md):** how this subsystem now **implements**
  the frozen reconciled contracts (the `list_objects` `SetExpr` push-down, `myelin-content`, `myelin-query`, the
  `#sub` grammar, the REF-3 key reconciliation, CheckStatus consumption, the erasure posture by reference) plus
  any **residual** request for Phase 6.
- **[07 — Drills & open questions](./07-drills-and-open-questions.md):** the quantified PROVE-IT drills + the
  open questions handed to Phase 6.

---

## 0. Changes vs the Phase-4 first pass (the reconciliation deltas absorbed)

The Phase-4 first pass was sound and most of its change-requests were **CONFIRMED** by reconciliation — none of
its ADRs was reversed, and its core bets (three axes, co-equal views, typed-core+JSONB, the SC-11 timer wheel)
stand unchanged. What changed is that **every open encoding the first pass asked for is now frozen concrete**, so
this rewrite *builds to the frozen shape* rather than *requesting* it. The deltas, each with its reason:

| # | What changed | From (Phase-4) | To (Phase-5 frozen) | Why / where |
|---|---|---|---|---|
| Δ1 | **`list_objects` push-down is now a concrete `SetExpr`** | CR-1 asked "confirm `Filter` is consumer-composable over `issue.id`" | the planner lowers the frozen `SetExpr` (All/None/Ids/NotIds/`InRelation{relation, via_column}`/Union/Intersect/Difference/`TupleSet`) into a SQL predicate / JOIN against the **per-tenant authz reverse index** keyed on `issue.id`; no N+1, no post-filter | OQ-E; contract 4.3. My blocking ask, **granted** |
| Δ2 | **Field/transition ABAC is now the frozen `CaveatContext`** | CR-2 asked for "the caveat-context shape" | `check(subject, view_field\|perform_transition, object, zookie?, caveat: CaveatContext{object, field?, transition?, attrs})` evaluated at `check`-time, off the hot `list_objects` path | OQ-E; contract 4.2 |
| Δ3 | **The Issues ArtifactRef id grammar is frozen** | CR-3 asked "confirm `ENG-1421` is the canonical `<id>`, not a render-time alias" | the stored canonical `<id>` segment is **`<PROJECTKEY>-<seqno>`** (e.g. `ENG-1421`); `#1421` is the **render-time display projection**, never the stored link | §3 / REF-3; contract 5.1. My blocking ask, **granted** |
| Δ4 | **The `#sub` grammar is the unified frozen vocabulary** | first pass used ad-hoc `#comment-N`/`#field-…`/`#sub-…`/`#rel-…` | the frozen kinds: `comment-<opaqueid>`, `b<opaqueid>` (issue-description block), `field-<opaqueid>`, `row-<opaqueid>` (issue-as-row), plus the shared `thread-`/`message-` where applicable; opaque ids stable across edits; one 4-step tombstone ladder | X-4/OQ-D; contract 5.7 |
| Δ5 | **`myelin-content` consumed subset is now declared explicitly** | first pass said "issue body is `myelin-content` blocks" | Issues consumes the **frozen Chat-equivalent block subset** (paragraph/heading/lists/task_list/blockquote/code_block/callout/table/divider/image) + all three inline ref nodes; **excludes** `db_view`/`sync_block`/`toggle` from inline authoring; description concurrency is single-author CAS | X-2/OQ-B; contract 13.1 |
| Δ6 | **The ADF→`myelin-content` lossy-map is frozen (consumed, not co-authored)** | CR-9 asked to "co-design the converter fidelity" | Knowledge **owns and froze** the ADF→content lossy-node map; Issues' import **consumes** exactly that map and records every lossy conversion in the import report | X-2; contracts 13.1/13.2 |
| Δ7 | **`myelin-query` field-type enum + view-model + AST + `order_key` are byte-identical frozen** | CR-10 asked to "confirm primitive parity" | the four shapes are frozen byte-identical with Knowledge; the `order_key` encoding is the frozen base-62 `0-9A-Za-z` lexicographic LexoRank with 2-char jitter + 48-char rebalance trigger + `created_at`+ULID tiebreak. Issues owns its compiler; the definitions are shared | X-3/OQ-C; contract 13.3 |
| Δ8 | **The Trigger condition is the frozen `QueryAst`** | CR-5 asked "can the matcher express 'all `blocked_by` resolved'" | `arm_trigger`'s `condition` is the frozen `myelin-query` `QueryAst` over projection state (`Has`/`Ref`/`In` predicates over `issue_relation`); the `QueryAst` **is** the `EventMatcher` core — no per-subsystem CEL | OQ-C; contracts 3.3/3.4 |
| Δ9 | **My Work is a `list_inbox` view over the ONE inbox** | CR-12 asked to "confirm My Work = filter over one inbox" | "My Work" (S10) is `list_inbox(principal, filter)` over the one Notif inbox (C-9); assigned/blocked/needs-approval/overdue are `reason`/`subject` filters with shared read-state — never a second store | C-9; contract 7.1 |
| Δ10 | **CI status arrives as the frozen `CheckStatus` fact / `ci.check.updated`** | first pass read "linked PR CI status" loosely via `project` | the "can't mark Done while CI red" guard reads the CI-owned `CheckStatus` projection (state + trust posture) surfaced through the linked PR's `project`; Issues never recomputes trust | X-1; contract 5.9 |
| Δ11 | **SLA escalation chain shape is frozen on the timer wheel** | CR-6/CR-13 asked to "confirm re-arm + chain shape" | the escalation chain (`page → oncall_now → escalate-after-timer`) is the frozen Notif `7.5` shape; SLA timers ride the SC-11 wheel via cheap disarm/re-arm of a precomputed `fire_at` | CR-6/CR-13; contracts 7.5/9.3 |
| Δ12 | **Worklog/productivity/estimate fields carry the frozen sensitivity tags** | CR-8 asked "classify worklog sensitivity" | these fields are tagged `#[personal_data(category = behavioural, role = tenant-content, basis = TBD-LEGAL, retention = tenant-policy)]`, **restricted by default**: excluded from cross-individual analytics + agent-use for a restricted subject; per-individual productivity rollups off by default. `[OPEN — LEGAL]` ratification flagged | OQ-H; contract 10.2 |
| Δ13 | **The free-text PII erasure residual is now ONE platform posture by reference** | CR-7 stated an Issues-local GD-6 residual | Issues **instantiates the ONE platform erasure posture by reference** (recon §X-7, contract 10.9); it does not restate a separate residual. The structural floor (per-subject DEK crypto-shred + pseudonym-map shred + `restrict`) ships now | X-7/OQ-G; contract 10.9 |
| Δ14 | **The firehose resume-cursor protocol is frozen and co-designed once** | sketch 08 open Q6 "co-design with Chat/KN" | real-time sync uses the frozen `subscribe(stream, scope, cursor?)` / `resume(stream, scope, last_seq)` protocol with per-`(stream,scope)` monotonic `seq` and `resync_required` → `*.snapshot` fallback; scope is a bounded selector (`board:<id>`), never `*` | OQ-J; contract 3.5 |
| Δ15 | **Tier-3 Search escalation valve is unblocked** | CR-11 was "partially blocking" | the board/list query over budget compiles to Search with the **same** OQ-E `Filter` conjoined (ACL-pre-filtered); the `search-requires-acl-filter` lint holds | OQ-E + §4; contract 6.1 |
| Δ16 | **The agent ToolDef `requires_approval` defaults are frozen jointly with the Fabric** | first pass set its own per-tool defaults | the frozen X-6 defaults bind: `forecast`/`triage`/`sla_draft` = no (suggest); `transition(issue, →done)` on an SLA-bound issue = **yes if the transition has an approver edge** (the field/transition caveat) | X-6; contract 8.1 |
| Δ17 | **The cross-cell portfolio rollup rides the frozen `CrossCellPointer` frame** | CR-15 asked to "confirm the bridge" | the rollup walk over a remote child uses the frozen PII-free `CrossCellPointer{subject, type, correlation_id, home_cell}`; resolution is **always cell-local** (the home cell renders + permission-checks; only the projection crosses). Single-cell is the complete v1 | OQ-I; contract 12.6 |

**Net:** the Phase-4 architecture is carried forward essentially intact; this rewrite swaps every "→ requested /
confirm" seam for the frozen shape, declares the consumed subsets explicitly, and folds the five legal-posture
items into the ONE platform posture by reference. The five Phase-4 *blocking* asks (CR-1, CR-3, CR-6, CR-11,
CR-12) are all **granted/frozen**.

---

## 1. The role: the work-coordination spine of Myelin

Issues is where work is **named, ranked, governed, related, and tracked to done** — for three audiences that
historically needed three different tools (Jira for corporate governance, Linear for engineers, a spreadsheet or
a slide deck for PMs). Myelin's thesis (VISION §2; design-language §2) is that **these are three lenses on one
model, not three products.** The make-or-break architectural bet is that the engineer's **board** and the PM's
**roadmap** are *co-equal views over one `issue` table* — not two object graphs an integration keeps in sync
(sketch 01; resolved in [05 §1](./05-hard-problems.md)). Get that wrong and PMs get a parallel reality, which is
the exact failure the platform exists to kill.

Issues is also the subsystem the rest of Myelin **coordinates around**: a Git branch auto-transitions an issue,
a CI failure (the frozen `CheckStatus`) blocks a "Done" transition, a chat message becomes an issue, an SLA
breach pages on-call, an agent triages an incoming bug, a roadmap forecast flags a slipping initiative. It is the
**most cross-subsystem-coupled of the five subsystems** — which is precisely why it must build *only* on the
shared contracts (no cross-DB reads; everything through `project`/`resolve`/the bus) and never grow private
back-channels.

### 1.1 The three independent axes (the structural core)

The single most important structural decision (sketch 01) is that work is organised along **three independent
axes**, each landed on the primitive that already owns it — never collapsed into one tree:

| Axis | What it is | Primitive it lands on | Why separate |
|---|---|---|---|
| **Containment** (scope) | sub-task → story/bug/task/chore/spike → **epic → initiative** | one `issue` table; `parent` edge in `issue_relation` (TE-7 truth) | board↔roadmap co-equality lives here — it must be *one table* |
| **Time** | cycles / sprints, milestones / releases | a separate small `cycle` / `milestone` object; **membership** edge, not containment | a cycle has no workflow state/assignee; modelling it as an issue is awkward-nulls |
| **Org-scope** | team / project | the **identity `project`** object (Id §5) — *not re-invented* | it is the authz boundary + the human-key prefix owner; Identity owns it |

An issue is reachable from **all three at once** (it is in cycle 14, under epic SSO-Hardening, owned by team ENG)
and from cross-artifact context (a PR, a chat message). It is never "filed" in one tree; it is an addressable
`ArtifactRef` node projected into many views — `myelin://<tenant>/issue/issue/ENG-1421` (the frozen
`<PROJECTKEY>-<seqno>` id grammar, Δ3). This is the "reference everything" wedge made structural (design-language
§6; IA §2).

---

## 2. What Issues owns vs delegates

### 2.1 Owned (the source of truth Issues is authoritative for)

| Owned thing | Form | Authority note |
|---|---|---|
| **The `issue` table** — the ranked-type containment spine | typed-core columns + JSONB tail (sketch 03) | the work spine; board + roadmap both read it |
| **`issue.*` event taxonomy** | the complete list under Bus §6 grammar ([03 §1](./03-events-contracts-and-glue.md)) | Issues extends the Bus §6.2 seed; the `initiative` type token is now a **registered** token (contract 2.9) |
| **`issue_relation`** — the TE-7 typed relation table | source of truth for `parent`/`blocks`/`blocked_by`/`closes`/`depends_on`/`relates` | **frozen contract** (5.5 / Refs §3.3); Refs holds the rebuildable projection + fixes inverse pairing |
| **Governance schemes** — workflow/field/permission/SLA/type | interpreted config rows, assigned per (type × team/project) | "Linear-simple = empty config; Jira-powerful = more schemes; one product" |
| **The workflow state-machine interpreter** | data-driven; guards as the frozen `QueryAst` predicates | not codegen; not user-scripting (no Jira-Groovy footgun) |
| **Human-readable keys** (`ENG-1421`) | Hi/Lo batched allocator, gap-tolerant, monotonic | the **stored canonical `<id>`** in the ArtifactRef + CLI (Δ3) |
| **Drag-to-reorder rank** | the frozen `order_key` LexoRank string + server-arbitrated CAS | byte-identical with Knowledge's `db_row` order (contract 13.3) |
| **The rollup aggregate** | derived materialised aggregate per ancestor, rebuildable by replay | edge truth stays in `issue_relation`; the rollup is derived |
| **The SLA *logic* engine** | policy + business-calendar arithmetic + pause/resume + escalation chain | the *timers* are the SC-11 wheel; the chain shape is frozen (7.5) |
| **The Issues-side stateful-Trigger UX** | armable conditions = the frozen `QueryAst` over Issues state | "Remind me when unblocked" — the flagship (contract 3.3) |
| **The AST→OLTP-store query compiler + cost-bounding** | our query planner; lowers the `SetExpr` push-down | we own the planner; we consume the shared AST + the `SetExpr` |
| **Cycle / sprint / milestone objects** | small time-axis tables | membership edges, not containment |
| **The import engine + canonical interchange format** | two-pass, ID-remapped, idempotent; consumes the frozen ADF map | round-trips with the portability export |
| **Co-ownership of `myelin-query`** (ADR-06) | field-type enum + view model + AST + `order_key`, byte-identical (Knowledge leads) | we contribute the issue-tracker storage discipline + our own compiler |

### 2.2 Delegated (consumed via the frozen contracts; never rebuilt)

| Delegated to | What Issues consumes | Contract |
|---|---|---|
| **Identity** | `authenticate` / `check` (+ `CaveatContext`) / `list_objects` (the `SetExpr` push-down) / `list_subjects` / `write_tuples` / `resolve_pseudonym`+`erase`; the `issue` ReBAC fragment | 4.1/4.2/4.3/4.4/4.6/4.8/4.9 |
| **Event Bus** | the `EventEnvelope`; `OutboxTx::emit` (the only emit path); the consumer template; the `QueryAst` `EventMatcher`; `arm_trigger`/`disarm_trigger`; reindex-from-source; the firehose resume-cursor protocol | 2.1–2.9; 3.1–3.6 |
| **Refs** | `ArtifactRef` parse/format; `resolve` (the context-pane unfurl, with the frozen tombstone ladder); `backlinks`/`traverse`; the typed-edge mirror (5.5); the unified `#sub` grammar | 5.1/5.2/5.3/5.5/5.7 |
| **Search** | `query`/`semantic`/`declare_indexable`; the Tier-3 escalation valve conjoining the OQ-E `Filter` | 6.1/6.2/6.3 |
| **Notifications** | `list_inbox` (the "My Work" view — C-9); `humanise` (the ONE templating surface); `oncall_now`/`page` + the escalation chain; `define_notif_rule` | 7.1/7.3/7.5/7.6 |
| **Agent Fabric** | `register_tool` (the shared ToolDefs + frozen `requires_approval` defaults); `EffectApi::apply`; the plan-then-apply loop; reserve/settle | 8.1/8.2/8.5/8.7; 11.7 |
| **Durable Workflow** | the **timer wheel** (SLA + `stale_after` + snooze + HITL timeout); durable signals (multi-day HITL); escalation as a durable workflow | 9.1/9.2/9.3/9.4 |
| **Storage** | the OLTP pool (one DB per service); `BlobStore` (attachments); the **OLAP read store** (CQRS analytics, restriction-flag-honouring); KMS (per-subject DEK); backup/restore | 11.1/11.2/11.3/11.4/11.5/11.6 |
| **GDPR/Audit** | the DSR orchestrator; the classify derive; the tamper-evident audit log (Issues contributes attribution); the **ONE erasure posture by reference** (10.9) | 10.1/10.2/10.3/10.6/10.9 |
| **Tenancy** | `(tenant, region)` partition key; `discover`/`place`/`residency_verify`; the isolation spectrum; the cross-cell `CrossCellPointer` bridge | 12.1/12.2/12.3/12.5/12.6 |

**The delegation discipline:** Issues is a **thin shell over identical plumbing**. It calls `serve(AppSpec)`,
supplies handlers + migrations + consumer registrations, and inherits the outbox, idempotent consumers,
tenant-scoping, the resilient client, the protected human lane, fail-static, the three ports,
`PersonalDataHolder` auto-registration — none of which it may reimplement or skip (contracts 1.1–1.11).

---

## 3. Internal component architecture

Issues is one logical service (the `issue` subsystem) with one OLTP database (the `no-cross-db` boundary,
ADR-01) and a set of **bus consumers** that maintain derived state. All in Rust unless noted (carried forward,
[01 §1](./01-tech-and-data-model.md)):

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
   │  │ - Hi/Lo key    │  │ - QueryAst guards  │  │  cost-bounding;   │  │   type schemes per (type×proj)│ │
   │  │ - order_key CAS│  │ - post-actions     │  │  SetExpr lowering;│  └───────────────────────────────┘ │
   │  │ - outbox emit  │  └────────────────────┘  │  Search escalate) │  ┌───────────────────────────────┐ │
   │  └────────────────┘  ┌────────────────────┐  └──────────────────┘  │ SLA engine                    │ │
   │  ┌────────────────┐  │ Trigger/automation │  ┌──────────────────┐  │ - business-calendar arith     │ │
   │  │ Relation writer│  │ - armable conds    │  │ Import engine    │  │ - precompute fire_at          │ │
   │  │ (issue_relation│  │   (QueryAst)       │  │ - 2-pass ID-remap │  │ - pause/resume re-arm         │ │
   │  │  = truth)      │  │ - on bus primitives│  │ - frozen ADF map  │  │ (rides the SC-11 timer wheel) │ │
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
   │  │ (debounced,  │ │ (arm/disarm  │ │ (QueryAst    │ │ (git.*/ci.*/ │ │  to a generated index off  │   │
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
- **The query planner owns the AST→store compilation + cost-bounding + the `SetExpr` lowering** — it conjoins the
  frozen `list_objects` `Filter` first (leak-free), then decides typed-column scan vs GIN vs generated index vs
  Search escalation, and bounds every query ([02 §3](./02-internals-and-algorithms.md)).
- **Derived state is rebuildable** — the rollup aggregate, the Search projection, the Refs edge projection, and
  the OLAP read store are all rebuilt by reindex-from-source (`replay` → `*.snapshot` → the live consumer path),
  so steady-state and recovery share one code path and cannot drift (contract 2.6).

---

## 4. Scaling & sharding within the cell topology

Issues inherits the cell model (ADR-11; Tenancy): a **cell** is a complete region-pinned stack; scale = add
cells; `(tenant, region)` is the first column of every table and the partition key. The Issues-specific scaling
posture and hot-spots (detailed in [02 §8](./02-internals-and-algorithms.md)):

| Concern | Posture | Hot-spot & mitigation |
|---|---|---|
| **OLTP** | one Postgres-class DB per service, sharded by tenant; the floor (sketch 03) | a hot tenant's `issue` table → tenant-shard split; distributed-SQL is the *measured* follow-on, never premature |
| **Human-key allocation** | per-prefix Hi/Lo batched (sketch 04) | a create-storm on a hot prefix (incident, import) → larger adaptive block size; per-prefix isolation (busy `ENG` doesn't slow `OPS`) |
| **Board/list query** | typed-core index range scan; cost-bounded; the `SetExpr` JOIN pushed down | a large-custom-field board → generated index off the bus; cold/ad-hoc → Search escalation (the OQ-E `Filter` conjoined; never an unbounded JSONB scan) |
| **Rollup fan-out** | event-driven, debounced, incremental | a leaf under a 50-team initiative → debounce coalescing + per-tenant in-flight caps + `input_hash` no-op suppression |
| **SLA timers** | ride the SC-11 minute-bucket wheel | millions of far-future timers cost an indexed range read; precompute `fire_at`, cheap disarm/re-arm, never poll |
| **Drag-reorder** | frozen `order_key` LexoRank O(1) per move + CAS | heavy concurrent reorder of one region → 2-char jitter + region-local background rebalance at 48 chars (CAS floor; CRDT is the named follow-on) |
| **Import** | per-tenant in-flight caps + the protected human lane shed order | a giant import (100k+ issues) → bounded backfill + fairness shed; never starves another tenant (humans last to shed) |
| **Real-time sync** | the frozen firehose `subscribe/resume/scope` protocol | a huge board's event stream → bounded `scope = board:<id>` + resume-cursor on reconnect (loses zero ops, OQ-J) |

**Multi-cell** (OQ-I): a prefix belongs to a team, a team to a project, a project to a cell — so a prefix lives
in exactly one cell; key allocation is cell-local (no cross-region coordination). Cross-cell portfolio rollup (an
initiative whose children span cells) rides the frozen PII-free `CrossCellPointer` bridge (contract 12.6),
resolution always cell-local, and is the **named floor** — single-cell is the complete v1.

---

## 5. Where the design proves itself (forward pointers)

The make-or-break properties and the drills that prove them (full list in
[07](./07-drills-and-open-questions.md)):
- **Co-equal-view consistency** (D1) — edit on the board ⇒ the roadmap reflects the same `issue` row, no drift.
- **Flexible-field query latency** (D2) — a large-custom-field board query under the <1s keyboard budget; a cold
  ad-hoc query escalates to Search with the `Filter` conjoined, never an unbounded OLTP scan.
- **Permission-leak (confidential)** (D3) — a cross-tenant + confidential-issue IDOR drill: zero leak via
  board/search/backlink, including under zookie staleness and the `SetExpr` JOIN.
- **SLA breach durability** (D6) — breach fires after a restart + a business-calendar arithmetic corpus.
- **Trigger fires-once-after-restart** (D7) — `stale_after` + resolve, the `QueryAst` condition.
- **Concurrent-reorder** (D5) — N humans + an agent re-ranking one region: zero silent clobber, bounded re-base.
- **Import round-trip** (D9) — export→import→export round-trips; a large import resumes after crash with no
  duplicates; it doesn't starve other tenants.
- **Erasure reaches every holder** (D11) — change-log, comments, attachments, OLAP, Search, the Refs projection.

Continue to [`01-tech-and-data-model.md`](./01-tech-and-data-model.md).
