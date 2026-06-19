# Issue Tracker — 06 · Reconciliation Compliance (how this subsystem implements the frozen contracts)

> See [`00-overview.md`](./00-overview.md) §0 for the full Phase-4→Phase-5 delta table. This doc is the **proof of
> conformance**: for each reconciled contract Issues touches, how this subsystem now **implements** the frozen
> shape (no drift), keyed to the
> [contract index](../../../05-refined-shared-systems-architecture/contract-index.md) and
> [`00-reconciliation-decisions.md`](../../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md).
> It replaces the Phase-4 "shared-system change requests" doc: every Phase-4 CR-1…CR-16 is now either **frozen
> (implemented here)** or carried as a **residual for Phase 6**. The five Phase-4 *blocking* asks (CR-1, CR-3,
> CR-6, CR-11, CR-12) are all **granted/frozen**.

---

## 1. The Issues punch-list from reconciliation (recon §4, "Subsystems → Issues") — each implemented

The reconciliation per-system punch list for Issues:

> **Issues:** consume the OQ-E `Filter` (board, blocking — now unblocked); `<PROJECTKEY>-<seqno>` keys; the
> `myelin-query` parity (X-3); ADF import map (X-2); typed `issue_relation` table (TE-7); SLA escalation
> workflow; worklog sensitivity tags.

| Punch-list item | Frozen contract | How Issues implements it | Doc |
|---|---|---|---|
| Consume the OQ-E `Filter` (board, **unblocked**) | 4.3 `list_objects` `SetExpr` | the planner lowers `SetExpr` over `ColRef{issue, id}` into a SQL predicate / JOIN against the per-tenant authz reverse index; no N+1, no post-filter; Tier-3 escalation conjoins the same `Filter` into Search | [02 §3](./02-internals-and-algorithms.md), [03 §6.2](./03-events-contracts-and-glue.md) |
| `<PROJECTKEY>-<seqno>` keys | 5.1 ArtifactRef id grammar | Hi/Lo mints the key; it is the **stored canonical `<id>`**; `#1421` is render-time | [01 §7](./01-tech-and-data-model.md), [03 §2](./03-events-contracts-and-glue.md) |
| `myelin-query` parity (X-3) | 13.3 | links the frozen field-type enum + `ViewSpec` + `QueryAst` + `order_key` crate (byte-identical); owns only the AST→store compiler | [01 §1.3](./01-tech-and-data-model.md), [02 §5](./02-internals-and-algorithms.md) |
| ADF import map (X-2) | 13.1/13.2 | import **consumes** the frozen ADF→`myelin-content` lossy-map (Knowledge-owned); records every lossy node in the report | [05 §10](./05-hard-problems.md), [01 §8](./01-tech-and-data-model.md) |
| Typed `issue_relation` (TE-7) | 5.5 | owns the table as source of truth (forward edge + one event); Refs mirrors both directions + fixes inverse pairing | [01 §4](./01-tech-and-data-model.md) |
| SLA escalation workflow | 7.5 + 9.3/9.4 | precompute `fire_at` on the SC-11 wheel (cheap disarm/re-arm); breach starts the frozen `page → oncall_now → escalate-after-timer` durable workflow | [02 §6.2](./02-internals-and-algorithms.md) |
| Worklog sensitivity tags | 10.2 (OQ-H) | `estimate`/`story_points`/worklog tagged `category=behavioural, role=tenant-content, restricted by default`; per-individual rollups off by default | [01 §6.1](./01-tech-and-data-model.md) |

---

## 2. Every frozen shared contract Issues consumes — conformance per contract

### 2.1 The `list_objects` `SetExpr` push-down (contract 4.3 — the single most load-bearing, the granted CR-1)
Issues conjoins `list_objects(viewer, 'view', 'issue', zookie?)` **first** in every read; lowers the returned
`SetExpr` (`Ids`/`NotIds`/`InRelation{relation, via_column}`/`TupleSet`/Union/Intersect/Difference/All/None) into
a SQL predicate / JOIN against the per-tenant authz reverse index keyed on `issue.id`. **No drift:** the consumer
table/column is `ColRef{ table:"issue", column:"id" }`; the JOIN is `... JOIN authz_visible av ON av.object_id =
issue.id AND av.subject = $1 AND av.relation = $2`. The Tier-3 board-escalation valve compiles the board query to
Search with the **same** `Filter` (the `search-requires-acl-filter` lint holds).

### 2.2 The field/transition ABAC `CaveatContext` (contract 4.2 — the granted CR-2)
Field-hiding and governed-transition approval are evaluated at `check`-time with the frozen
`CaveatContext{object, field?, transition?, attrs}` on already-filtered, already-fetched rows — **off** the hot
`list_objects` path. **No drift:** Issues passes `field: Some(field_id)` for column-hiding and
`transition: Some(t.id)` for approver gates.

### 2.3 The ReBAC namespace fragment (contract 4.9 — frozen)
Issues declares the frozen `issue` fragment: the `confidential` set-difference exclusion, the `watcher`
read-fanout relation, and the `issue_field`/`issue_transition` caveat sub-objects. Identity owns the engine and
maintains the reverse index; Issues only `write_tuples` (contract 4.6, the zookie stamped on the object).

### 2.4 `myelin-content` consumed subset (contract 13.1 — X-2)
Issues consumes the **frozen Chat-equivalent block subset** (paragraph/heading/lists/task_list/blockquote/
code_block/callout/table/divider/image) + all three inline ref nodes (`mention`/`artifact_ref`/`embed`);
**excludes** `db_view`/`sync_block`/`toggle` from inline authoring. Description concurrency is single-author CAS
(ADR-05). The inline ref nodes are the producers of `refs.edge.created` (contract 5.4) — uniform across Chat,
Issues, Knowledge. The WASM editor render path holds the `render(parse(md)) === md` round-trip (D10).

### 2.5 `myelin-query` byte-identical (contract 13.3 — X-3)
The field-type enum, `ViewSpec`, `QueryAst`, and `order_key` are the **frozen shared crate** — not
re-implemented. The `order_key` is the frozen base-62 `0-9A-Za-z` LexoRank (lexicographic compare, midpoint
bisection, 2-char jitter, 48-char rebalance trigger, `created_at`+ULID tiebreak), so an issue dragged in a backlog
and a row dragged in a Knowledge db produce **byte-identical** keys.

### 2.6 The unified `#sub` grammar (contract 5.7 — X-4)
Issues mints stable opaque sub-ids of the frozen kinds: `comment-<opaqueid>`, `b<opaqueid>` (description block),
`field-<opaqueid>`, `row-<opaqueid>` (issue-as-row). Refs stores the full sub-URN + the stripped root; the one
4-step tombstone ladder (permission → root → sub-resolve {live/moved/outdated/gone} → erased) governs resolution;
a tombstone always carries the root. **No drift:** opaque ids are stable across edits (the stability obligation
is Issues').

### 2.7 The `CheckStatus` seam (contract 5.9 — X-1, consumed)
Issues is a **consumer** of the CI-owned `CheckStatus` fact (Git owns the projection + gate). The "can't mark Done
while CI red" guard reads the linked PR's commit `CheckStatus{state, trust_tier}` via `project(PR_ref)`; it checks
`state = success` **and** an acceptable trust posture (an `untrusted_fork` success is neutral until
endorsed/re-run-trusted). Issues **never recomputes trust** — it reads `trust_tier` off the fact (Δ10).

### 2.8 My Work over the ONE inbox (contract 7.1 — C-9, the granted CR-12)
"My Work" (S10) is `list_inbox(principal, filter)` over the one Notif inbox; assigned/blocked/needs-approval/
overdue are `reason`/`subject` filters; read-state is the one truth (`mark`/`snooze`, contract 7.2). **No second
store.** Issues registers its `define_notif_rule` set (SLA at-risk/unblocked/approval-requested) and its
`humanise` templates into the **ONE templating surface** (contract 7.3 — backend-humanised, `ArtifactRef`-paired,
ICU MessageFormat; the SLA strings register here, no second template engine).

### 2.9 The escalation chain (contract 7.5 — the granted CR-13) + the timer wheel + durable signal (9.3/9.4)
SLA timers ride the SC-11 wheel via cheap disarm/re-arm of a precomputed `fire_at` (the granted CR-6); the Trigger
`stale_after`, snooze re-surfacing, and HITL timeouts ride the same wheel. A breach starts the frozen escalation
chain (`page → oncall_now → escalate-after-timer`) as a durable workflow; multi-day HITL approval holds no runtime
and resumes on `signal(approval)` (contract 9.4).

### 2.10 The `arm_trigger` condition = the frozen `QueryAst` (contract 3.3/3.4 — the granted CR-5)
The Trigger condition is the frozen `myelin-query` `QueryAst` over projection state (`Has`/`Ref`/`In` express "all
`blocked_by` resolved"); the `QueryAst` **is** the `EventMatcher` core — no per-subsystem CEL.

### 2.11 ToolDef `requires_approval` defaults (contract 8.1 — X-6, frozen)
Issues registers its ToolDefs with the frozen defaults: `forecast`/`triage`/`sla_draft` = no (suggest);
`transition(issue, →done)` on an SLA-bound issue = **yes if the transition has an approver edge**. The four
uniform sandbox guarantees (contract 8.4) are inherited; reserve/settle (11.7) fronts every spend-bearing run into
the same wallet.

### 2.12 The OLAP restriction-flag + per-subject DEK (contracts 11.6/11.4)
The OLAP read store consumes Issues' `issue.*`/`sla.*`/`cycle.*` stream (reindex-from-source only) and **honours
the restriction flag** (no analytics for a restricted subject — a compliance gate). Free-text/body/change-delta
columns use the per-subject DEK (GD-4).

### 2.13 The ONE erasure posture by reference (contract 10.9 — X-7)
Issues instantiates the platform posture by reference; it does not restate a separate residual (Δ13). Structural
floor ships now (per-subject DEK + pseudonym-map shred + `restrict`); the third-party free-text residual is under
the documented lawful-basis limit, `[OPEN — LEGAL]`.

### 2.14 The cross-cell pointer bridge (contract 12.6 — OQ-I)
Cross-cell portfolio rollup rides the frozen PII-free `CrossCellPointer{subject, type, correlation_id,
home_cell}`; resolution is always cell-local. Single-cell is the complete v1; cross-cell is the named floor.

### 2.15 The `initiative` token + event taxonomy (contract 2.9)
`initiative` is now a **registered** type token; Issues owns its complete `issue.*` dotted-name list under the Bus
§6 grammar ([03 §1](./03-events-contracts-and-glue.md)).

---

## 3. Residual asks carried to Phase 6 (roadmaps / build)

Reconciliation **granted or froze every Phase-4 ask**; nothing remains *blocking*. The residuals are the named
floors + the `[OPEN — LEGAL]` items, all of which ship a structural floor now and have a named follow-on:

| Residual | Nature | Carried to | Floor that ships now |
|---|---|---|---|
| **R-1 · Third-party free-text PII residual basis** | `[OPEN — LEGAL]` (contract 10.9) | GDPR/Counsel (Phase 6 legal track) | per-subject DEK + pseudonym shred + `restrict`; documented limit + best-effort `rectify` |
| **R-2 · Worklog/productivity special-category classification** | `[OPEN — LEGAL]` (contract 10.2, OQ-H) | GDPR/Counsel + works-council | fields tagged `behavioural`/`restricted by default`; per-individual rollups off by default |
| **R-3 · Move-CRDT for concurrent reorder** | named floor (R-5 promotion) | Phase 6 build (measured) | frozen `order_key` + CAS; CRDT swaps the engine, not the data model |
| **R-4 · Materialised-when-measured rollup** | named floor (KN-3) | Phase 6 build (measured) | read-time for small subtrees; materialise on measured-large |
| **R-5 · Monte-Carlo forecast** | named floor | Phase 6 (post real-LLM runtime) | linear `remaining ÷ velocity` agent (mock) |
| **R-6 · Distributed-SQL for a hot tenant** | named floor (measured) | Phase 6 build (measured) | PG sharded by tenant |
| **R-7 · Cross-cell portfolio rollup** | named multi-cell floor (OQ-I) | Phase 6 (multi-cell) | single-cell complete; `CrossCellPointer` frame frozen |
| **R-8 · Offline/local-first sync** | named follow-on | Phase 6 (if promoted) | optimistic + frozen resume-cursor protocol |
| **R-9 · Permission-scheme import mapping** | lossy/legal-review leg | Phase 6 (legal) | named-lossy in the reconciliation report, never silent |
| **R-10 · Real-LLM forecast/triage/SLA-draft runtime** | named floor (ADR-08, post safety drills) | Phase 6 (post the real-kernel escape drill, contract 8.4) | mock runtime; ToolDefs + `EffectApi` registered |
| **R-11 · History-compaction for very-long SLAs** | named follow-on | Phase 6 build (measured) | full business-calendar logic over the wheel |

**Net:** the Phase-5 reconciliation closed all five Phase-4 blocking items; the residuals are floors-with-named-
follow-ons and the two `[OPEN — LEGAL]` posture ratifications (where the structural floor ships regardless of
counsel).

Continue to [`07-drills-and-open-questions.md`](./07-drills-and-open-questions.md).
