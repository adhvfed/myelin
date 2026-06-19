# 00 — Findings: what I learned, what I'm committing, what I hand to architecture

> Phase 4, Issue Tracker subsystem, Stage 1 (design & sketch). This file closes the exploration: the **decisions
> committed** for the architecture stage, what each hard problem resolves to, and the **open questions** handed
> forward. Date: 2026-06-19. Grounded in: VISION; EI-04/EI-05; the Phase-2b directives (ISS-1, SUB-X, X-1…X-5,
> T-*, NOTIF-*); the Phase-3 contracts (contract-index; reference-graph TE-7; durable-workflow SC-11;
> identity §5 seeded `issue` namespace; event-bus §6.2 frozen tokens; notifications C-9); Phase-2 §11 open
> questions; the Phase-1 deep-dive; and the **Knowledge Wave-A** design (01 §4) since I co-own ADR-06 with it.

## What I learned (the load-bearing facts that shaped the commitments)

1. **The ArtifactRef grammar is already frozen and it pre-decides the issue model.** event-bus §6.2 lists
   `issue` types as `issue, epic, sprint, field, comment, relation` — so **epic is an `issue`-family *type***,
   not a separate object. This rules out the Linear-style "Projects/Initiatives are separate tables" model for
   the *containment spine* and points squarely at "everything on the spine is an `issue` row, ranked by type."
2. **Knowledge already solved the flexible-field storage problem** (01 §4.1): JSONB property-bag source of truth
   + derived projection (GIN + per-hot-facet generated indexes off the bus, never per-tenant DDL) + the same
   LexoRank `order_key` family. Co-owning ADR-06 means I should **share the storage discipline, not just the
   field-type enum** — divergence here would be gratuitous.
3. **The hard substrate is already built for me by Phase 3.** Durable timers at world scale (SC-11 wheel), the
   stateful Trigger `stale_after` timer, HITL durable signals, the typed-relation→Refs mirror protocol, the
   leak-free `list_objects` pre-filter, the one Notif inbox, the `issue` ReBAC namespace (with the `confidential`
   exclusion + field/transition ABAC edges) — all frozen. My job is the **Issues-specific logic layered over
   them**, not rebuilding them.
4. **Board↔roadmap co-equality is the make-or-break bet, and it has to be a property of the schema** (one
   `issue` table), not a feature I maintain — otherwise the roadmap drifts from the board and PMs get a parallel
   reality (the exact failure VISION §2 / design-language §2 exist to kill).
5. **Rust is right with no divergence** (Phase-2 §3): transactional OLTP + event processing + query compilation
   + a state-machine interpreter are squarely Rust's strengths; the one specialised future piece (a move-CRDT
   for ranking) is Yrs, itself Rust-native. No reason to diverge from the default.

## Committed decisions (per hard problem)

| # | Hard problem | COMMITTED direction | Floor → follow-on |
|---|---|---|---|
| 1 | **Issue-model duality / Epic-Initiative type-vs-level (PR-2)** | **One `issue` table = the ranked-type containment spine** (sub-task→story/bug/task/chore/spike→epic→initiative); `parent` is the single containment edge in `issue_relation` (TE-7 truth). **Board and roadmap are co-equal `myelin-query` AST views over that one table** (board=rank≤1 by state; roadmap=rank≥2 on a date axis). **Cycle/sprint = a separate small time-axis object** (membership edge, not containment). **Project/space = the identity scope object**, not re-invented. Rank is config with **rank-monotonic parenting** as the default guardrail. *Add `initiative` to the Issues type-token taxonomy.* (sketch 01) | tree `parent` v1; constrained-DAG portfolios as opt-in |
| 2 | **Governance baked-in vs opt-in (PR-3)** | **Typed-core columns (hot path) + layered optional schemes interpreted (data-driven), not baked.** The **fixed state-*category* set** (`unstarted/started/completed/cancelled`) over unlimited named states is the one mandatory invariant. Workflow/field/permission/SLA/type schemes assigned per (type×team/project); **assigning a scheme is config, never a data migration**. Guards are **safe query-AST predicates**, not scripting. Linear-simple = empty config; Jira-powerful = more schemes; **one product, no fork**. (sketch 02) | default scheme-set v1; scheme-assignment precedence algebra is the detail |
| 3 | **Flexible-field storage/query (TE-17, JQL trap)** | **Typed-core columns + JSONB property-bag tail + derived indexable projection (GIN + measured-hot generated indexes off the bus) + Search as the cold/ad-hoc/full-text escalation valve + OLAP for analytics.** I own the **AST→OLTP-store compiler + cost-bounding**; I consume the AST/types/view-model (ADR-06). **Deliberately aligned with Knowledge's `db_row`.** PG-sharded-by-tenant floor; distributed-SQL is the measured follow-on. (sketch 03) | PG hybrid floor → distributed-SQL on measured shard-outgrowth |
| 4 | **Human-readable monotonic keys (TE-14)** | **Hi/Lo batched per-prefix allocation, gap-tolerant, monotonic, never-reused**, adaptive block size (small=fewer gaps, large=less contention). UUID internal PK; human key = the public id in the ArtifactRef/CLI/UI, allocated once. Cell-local (no cross-region coordination). Gaps documented as benign. (sketch 04) | — (single-cell is the whole requirement) |
| 5 | **Drag-to-reorder ranking (TE-19)** | **LexoRank/fractional `rank` string + server-arbitrated CAS + jittered inserts + region-local background rebalance.** Aligned with Knowledge's `order_key`. **Agents reorder through the same permissioned tool + same CAS** as humans (one safe path). (sketch 06) | CAS floor → move-CRDT (reuse Yrs) on *measured* concurrent-reorder pain |
| 6 | **TE-7 typed relations** | **Own `issue_relation` as source of truth** (frozen contract); write forward edges transactionally, emit one typed event, let Refs materialise both directions; `parent`=tree, `depends_on/blocks/relates`=DAG with cycle detection in the walk. The Trigger reads this table. (sketch 05A) | — |
| 7 | **Rollup/forecast engine (TE-18)** | **Event-driven, debounced, incremental rollup off the bus, storing a derived materialised aggregate per ancestor (rebuildable by replay)**, with **read-time floor for small subtrees, materialise-on-measured-large**. Cycle-safe (visited-set + depth ceiling). `input_hash` no-op-suppression (loop safety, AG-6). **Forecast = an agent-powered swappable strategy reading OLAP** (floor=linear, follow-on=Monte-Carlo). (sketch 05B) | read-time floor → materialised; linear forecast → Monte-Carlo agent |
| 8 | **SLA business-calendar engine** | **Build the SLA *logic* (policy + business-calendar arithmetic + AST-driven pause/resume + escalation orchestration) over the `myelin-flow` timer/signal/workflow substrate.** Precompute wall-clock `fire_at`, re-arm on pause/resume (Candidate A). Don't build timers (consume SC-11), don't poll, don't pollute the shared wheel. Breach feeds OLAP. (sketch 07) | — (timers are SC-11; long-`time_to_resolution` history-compaction is a flag) |
| 9 | **Real-time sync** | **Optimistic UI + bus-driven cache invalidation over the shared firehose with a resume-cursor on reconnect (reuse KN-1's substrate).** Issue-body concurrency = single-author CAS (ADR-05), board concurrency = server-arbitrated — **no Issues CRDT.** (sketch 08A) | optimistic+resume floor → offline/local-first follow-on (out of v1 scope unless promoted) |
| 10 | **Stateful Trigger UX (ISS-1)** | **Own the Issues-side Trigger UX** (armable conditions reading Issues state + the armed/resolved/stale surface); consume the bus `arm_trigger` primitive, the `myelin-flow` `stale_after` timer, the one Notif inbox for `on_resolve`. Ship **"Remind me when unblocked"** as the flagship. (sketch 08B) | — |
| 11 | **Import fidelity (PR-8)** | **Two-pass, ID-remapped (persisted source↔Myelin map), idempotent+resumable, dry-run + reconciliation-report-first**; source adapters (Jira/Linear/GitHub/CSV) normalise into **one canonical interchange format that round-trips with the portability export**; import emits normal `issue.*` events (one indexing path; per-tenant capped); **lossy mappings named, never silently dropped.** (sketch 09) | canonical core + N adapters; permission-scheme mapping is the lossy/legal-review leg |

### Cross-cutting commitments
- **Language/DB:** Rust services; PostgreSQL-class OLTP (typed core + JSONB) sharded by tenant; OLAP columnar
  (ClickHouse-class) read model fed by the bus (CQRS) for analytics; shared Search for full-text/ad-hoc;
  `BlobStore` for attachments. **No divergence from the Rust default** (justified: squarely OLTP+event+query
  work). Self-hostable/EU-deployable by construction (all components are).
- **Glue contracts I implement** (the build-to surface): `serve(AppSpec)`; `OutboxTx::emit` as the only emit
  path; the **`project(ref,viewer)` projection API** (so Git's PR pane, Chat unfurls, Refs, Search, Notif read
  Issues without cross-DB); **`replay(scope,since)`** emitting `*.snapshot` (reindex-from-source);
  `ArtifactRef` parse/format with stable **`#sub`** ids (`#comment-12`, sub-issue, `#field-…`); register
  `ToolDef`s (create/update/transition/link/comment/estimate/reorder, each with required caps +
  side-effecting + `requires_approval` defaults); `declare_indexable` IndexSpec; the **`issue` ReBAC namespace
  fragment** (identity §5 seed + field/transition ABAC edges + `confidential` exclusion + a `watcher`
  relation for Notif fan-out); `#[personal_data(...)]` tags + `PersonalDataHolder`; reserve/settle on
  spend-bearing agent work; flag hot tables (`issue`, `issue_relation`, the change-log) for the
  forward-only-migration lint.
- **Taxonomy I own (extend the event-bus §6.2 seed):** the complete `issue.*` list — `issue.issue.created/
  updated[field deltas]/transitioned/closed/reopened/deleted/restored/assigned/priority_changed/type_changed`;
  `issue.relation.created/removed`; `issue.parent_changed`; `issue.rollup_recomputed`; `issue.added_to_cycle`;
  `issue.cycle.started/completed`; `issue.sla.started/paused/at_risk/breached/met`; `issue.approval.*`;
  `issue.triaged/duplicate_suspected/labelled_by_agent`; `issue.initiative.health_changed`;
  `issue.milestone.released`; + `*.erased`/`*.snapshot`. **Add the `initiative` type token.** (Reconcile names+
  units with X-5 in architecture.)

## Primary screens designed (this stage)
Information architecture (the one-shell fit + the three-axis nav), the key flows (human + agent/HITL +
cross-subsystem), and wireframes (with empty/loading/error/permission/erased/agent-pending states) for:
**S1 Issue detail · S3 Board · S5 Roadmap · S6 Backlog · S9 Triage · S10 My Work · S13 Workflow editor ·
S17 Import wizard · S19 Command palette/quick-create.** (S2 list / S4 table / S7 calendar / S8 cycle are the
same views component, noted; S11 reports / S12 saved-views / S14 SLA editor / S15 settings / S16 automation
builder / S18 audit are enumerated in the IA and get full wireframes in the architecture stage.)

## Named floors (E-3 gap-report seeds, dated 2026-06-19, status "claimed")
- **Rollup:** read-time floor → materialised-on-measured-large (sketch 05).
- **Ranking:** CAS floor → move-CRDT (Yrs) on measured concurrent-reorder pain (sketch 06).
- **Sync:** optimistic+resume floor; offline/local-first deferred out of v1 (sketch 08).
- **Storage:** PG-hybrid floor → distributed-SQL on measured shard-outgrowth (sketch 03).
- **Forecast:** linear floor → Monte-Carlo agent (sketch 05).
- **Free-text PII erasure:** anonymise-actor + redaction-tombstone + crypto-shred-attachment; **residual risk
  documented** ([OPEN — LEGAL] GD-6, sketch in user-flows C3).

## PROVE-IT — the quantified drill per failable property (named here; Phase 5 executes; T-4 scorecard)
- **Co-equal-view consistency:** edit on the board ⇒ roadmap reflects the same `issue` row (no drift) — a
  chained-mutation E2E (T-6).
- **Flexible-field query latency:** a large-custom-field tenant board query under the keyboard/<1s budget
  (T-8); cold ad-hoc query escalates to Search, never an unbounded OLTP scan.
- **Permission-leak (confidential):** cross-tenant + confidential-issue IDOR drill — zero leak via board/
  search/backlink (T-5 cross-tenant IDOR; deep-dive §8.4).
- **Human-key correctness:** Hi/Lo under create-storm — no duplicate, monotonic, gaps-only (sketch 04).
- **Concurrent-reorder:** N humans + an agent re-ranking one region — zero silent clobber, bounded re-base,
  order converges (sketch 06).
- **SLA breach durability:** breach fires after a restart (SC-11 rider) + business-calendar arithmetic corpus
  (sketch 07).
- **Trigger fires-once-after-restart** (`stale_after` + resolve) (sketch 08).
- **Rollup freshness under import-storm** + **reindex-from-source rollup/edge parity** (T-5 reindex-from-cold
  parity) (sketch 05).
- **Import round-trip:** export→import→export round-trips; large import resumes after crash with no duplicates;
  import doesn't starve other tenants (X-3) (sketch 09).
- **Editor round-trip:** `render(parse(md)) === md` over a corpus for issue bodies/comments (T-5/§8b.2).
- **Erasure reaches every holder** (issue change-log, comments, attachments, OLAP, Search, Refs projection) (T-5).
- **Frontend switch-test** (T-7) + measured-contrast/latency gates (T-8) on the primary screens.

## Open questions handed to the architecture stage
1. The **scheme-assignment precedence algebra** (deterministic resolution when team/project/type defaults
   disagree) — sketch 02.
2. Exactly **which core fields earn a typed column vs generated column vs JSONB** — sketch 03, co-reviewed with
   Knowledge for ADR-06 primitive parity.
3. The **projection-feeder** mechanism (how a custom field gets promoted to a generated index off the bus) +
   the AST→store cost model + the OLTP↔Search escalation threshold — sketch 03.
4. **Tree-vs-constrained-DAG** for `parent` (cross-team portfolio depth) + the rollup affected-ancestor fan-out
   + debounce-window policy — sketches 01/05.
5. The **business-calendar arithmetic** algorithm (DST/holiday/multi-day) + long-`time_to_resolution`
   history-compaction — sketch 07.
6. The **reconnect/resync protocol** + per-view subscription scope bounding (co-design with Chat connection
   tier / KN-1) — sketch 08.
7. The **canonical interchange schema** (the round-trip oracle) + the link-type/status-category/permission-
   scheme mapping tables (permission lossy → legal review) + ADF→`myelin-content` fidelity (co-design with
   Knowledge) — sketch 09.
8. Reconcile **"human key = ArtifactRef id"** with REF-3 "display keys are render-time" (resolution leaned in
   sketch 04: the *full* key is the stable public id; *short* forms are render-time) — confirm with Refs.
9. **Free-text PII erasure completeness** + the documented residual ([OPEN — LEGAL] GD-6) — architecture +
   legal.
10. **Worklog/productivity field sensitivity** (works-council/labour-law, GD-13) classification — architecture
    + legal.
11. **Forecast agent ToolDef** + at-risk threshold config — architecture (ties to the agent fabric).
12. The full **armable-condition catalogue** for stateful Triggers + the Trigger management surface — sketch 08
    + wireframes (architecture stage).
