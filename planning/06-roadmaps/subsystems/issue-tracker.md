# Issue Tracker — Subsystem Roadmap (Phase 6)

> Phase: `06-roadmaps/subsystems`. The detailed, sequenced build roadmap for the **issue-tracker** subsystem.
> Slots into the master sequencing bands M0..M6 ([`../00-master-sequencing.md`](../00-master-sequencing.md)) —
> it refines the work *inside* the bands and must not contradict the band ordering or the gate invariant.
> Frozen architecture (this roadmap sequences, it does not redesign):
> [`../../04-subsystem-architectures/issue-tracker/architecture/`](../../04-subsystem-architectures/issue-tracker/architecture/)
> (00..07) + [`../../04-subsystem-architectures/issue-tracker/design/`](../../04-subsystem-architectures/issue-tracker/design/).
> Build-to contracts: [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md).
> Drills: [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md)
> (ISS-D1..ISS-D14 + the shared families + E2E-1/E2E-2/E2E-3). Binding doctrine: EI-01 (order-by-non-negotiability,
> prove-it, the ratchet, name-your-floors) + EI-04 (CRDT-after-CAS, reindex-from-source, erasure-vs-immutability,
> event volume). Plain-text identifiers (no backticks-as-emphasis). Markdown only; no commits. Date: 2026-06-19.

---

## 0. Where issue-tracker lives in the master sequence

Issues is a **consumer subsystem** (master §2 M4, §3.2). It is the **most cross-subsystem-coupled of the five**
(arch 00 §1): it references Git commits/PRs, reads CI's CheckStatus to gate "Done", embeds Knowledge docs,
turns Chat messages into issues, pages on-call on SLA breach, and is driven by agents (triage/forecast/sla-draft).
That coupling is exactly why the bulk of its build is **M4** — it can only be proven once its producers (Git in
M3, CI also in M4) exist and the reactive shared layer (Refs/Search/Notif/Workflow/Agents, M2) is green.

Issues is **not** on the single longest critical-path spine (that runs harness → Identity → agent fabric → Git →
CI → X-1 seam → dogfood; master §3.1). But it sits on two important branches of it:

- **The X-1 CheckStatus seam (contract 5.9) consumer-of-a-consumer:** Issues' "can't mark Done while CI red"
  guard reads the CI-owned CheckStatus through the **linked PR's `project`** (Git owns the projection). So Issues
  depends on the X-1 seam being closed end-to-end (CI producer in M4, Git consumer/projection in M3) — its
  ISS-D12 guard drill cannot go green until GIT-D10 / CI-D8 is green within M4.
- **The agent-native flagship E2E-2** (CI-fail → triage agent → issue → chat → fix-PR; master §2 M5,
  catalogue §E2E-2): Issues is the node where the triaged failure becomes a tracked, governed work item with an
  agent-authored-then-human-approved transition. E2E-2 cannot be claimed until Issues' agent-tool surface +
  HITL-gated governed transition are proven.

Issues **participates earlier** than M4: it freezes its ReBAC fragment, its `issue.*` event tokens (incl. the
registered `initiative` token, contract 2.9), and its co-owned `myelin-query` compiler obligations in M1/M2 so
dependents (Search, Refs, Notif, the agent fabric) compile and so the frozen byte-identical `order_key` /
`ViewSpec` / `QueryAst` shapes do not drift from Knowledge. Its **world-scale / hard-problem follow-ons**
(move-CRDT, materialised rollup, distributed-SQL shard split, cross-cell portfolio rollup, Monte-Carlo forecast,
real-LLM runtime) are explicitly scheduled into **M5** (or post-M5 for the LLM swap); the switch test lands in
**M6**.

Per master §2 ("within a band, the per-system roadmaps parallelise the work"), the milestones below are the
Issues-internal decomposition of the M4 consumer work plus its M1/M2 pre-work and its M5/M6 follow-ons. The band
a milestone belongs to is named on every milestone. **The gate invariant binds**: no Issues milestone is "done"
over a red earlier-band gate (master §4) — most pointedly, no Issues code that writes real data is "done" over a
red STOR-D1 (the silent-data-loss floor, M1), and no Issues agent tool runs over a red AG-D4 (the sandbox-escape
GATE, M2).

---

## 1. The non-negotiability order applied to Issues (what kills us first, inside Issues)

Following EI-01 §2 and master §1, the Issues work is internally ordered by what is catastrophic, not by feature
size. Issues owns no Tier-0/Tier-1/Tier-2 *substrate* gate (those are the harness, the outbox, restore-verify,
and the sandbox-escape GATE — all upstream). What Issues owns is the *correct application* of those guarantees to
its own surface:

1. **Cross-tenant / confidential permission leak (Tier-1-of-Issues).** A confidential or cross-tenant issue
   appearing in any board / `list_objects` SetExpr JOIN / search / backlink / context-pane result is the worst
   thing Issues can do — it is the platform's core promise inverted. The defence is **by construction** (the
   ReBAC `- confidential` set-difference userset, the SetExpr push-down conjoined *first* in every read, never a
   post-filter), and it must hold even under zookie staleness. Proven by **ISS-D3** before any list surface is
   claimed, re-proven inside the surge family.
2. **Silent write loss on the seam (Tier-1).** A state change that committed without its event, or an event
   without the state change. Issues inherits the outbox guarantee (every state-changing handler ends in
   `OutboxTx::emit` in the same transaction, no `publish_now`); the *issue is the aggregate*, so per-issue
   ordering is `UNIQUE(aggregate, seq)`. Proven by the substrate gates (SUB-D1/SUB-D2/BUS-D4, M0) and by the
   reindex-from-source parity drill (ISS-D8b).
3. **Silent clobber on a concurrent write (the CAS floor).** Two humans + an agent reordering the same backlog
   region, or two edits to the same issue body, must never silently overwrite. Server-arbitrated CAS on the
   frozen `order_key` and on the `version` token is the floor (no merge — the loser re-bases honestly). Proven by
   **ISS-D5** before the move-CRDT is even considered.
4. **The governed-transition guard correctness (the poisoned-Done defence).** "Can't mark Done while CI red"
   must read the CI-owned trust posture and never recompute trust (an `untrusted_fork` success is neutral until
   endorsed); a governed transition by an agent must be HITL-gated (tool withheld, zero mutation pre-approval).
   Proven by **ISS-D12**.
5. **SLA + trigger durability across restart.** A breach that does not fire after a process restart, or a
   "remind me when unblocked" that fires twice or never, is a silent governance failure. Proven by
   **ISS-D6 / ISS-D7**.
6. Then the breadth (schemes, cycles, rollup, import, views, CLI), then the world-scale hardening (the 30x surge
   family, the flexible-field latency at 1M+ issues, the floor follow-ons), then the switch-test polish.

Sandbox escape (Tier 2) is **not** owned by Issues — it is the shared AG-D4 gate (master M2). Issues inherits it
by construction (the four uniform sandbox guarantees, contract 8.4 / X-6) and **must not run any agent tool
(triage/forecast/sla-draft) until AG-D4 is green.**

---

## 2. Upstream dependencies (what must exist + be green before Issues work starts)

Issues is a **thin shell over identical plumbing** (arch 00 §2.2): it calls `serve(AppSpec)`, supplies handlers +
migrations + consumer registrations, and inherits the outbox, idempotent consumers, tenant-scoping, the resilient
client, the protected human lane, fail-static, the three ports, and `PersonalDataHolder` auto-registration. The
table below names, per Issues milestone, the contracts that must already be implemented. The critical ones are
starred.

| Upstream (contract) | Owner / band | Why Issues needs it | Blocks Issues milestone |
|---|---|---|---|
| serve(AppSpec), three-surface, liveness≠readiness (1.1–1.3) | substrate / M0 | every Issues service boots from it | all |
| **OutboxTx::emit + outbox per-aggregate (issue=aggregate) + EventHandler + consumer_dedup (2.2–2.5)** | Bus / M0 | the only emit path; every write co-commits its event; the consumer template for rollup/SLA/trigger/feeder | M4-I1 (★) |
| EventEnvelope frozen (2.1) + token table incl. `initiative` (2.9) | Bus / M0 | the `issue.*` shapes + the registered `initiative` token align to it | M4-I1 (★) |
| The 12 lints incl. tenant-predicate, no-raw-publish, no-cross-db, residency-pin, search-requires-acl-filter, no-untagged-personal-data, flow-determinism (1.6) | substrate / M0 | Issues compiles against the ratchet | all |
| ResilientClient + FailStatic (1.9/1.10) | substrate / M0 | Issues→Id, Issues→CheckStatus-projection calls degrade not cascade | M4-I2 |
| **Identity: check + CaveatContext (4.2)** | Id / M1 | per-action write gate + field/transition ABAC (column-hide, approver gate) | M4-I1 (★) |
| **Identity: list_objects SetExpr push-down over issue.id (4.3)** | Id / M1 | the leak-free, no-N+1 board/list/search pre-filter — the single most load-bearing seam Issues consumes | M4-I3 (★) |
| Identity: list_subjects + explain (4.4) | Id / M1 | watcher read-fanout for Notif; the permission inspector (S15) | M4-I7 |
| Identity: delegation / mint_run_token (4.5/4.7) | Id / M1 | agent run policy intersection + per-run token | M4-I6 |
| Identity: write_tuples/zookie (4.6/4.10) | Id / M1 | the Issues ReBAC fragment compiles; read-your-writes after a just-revoked grant | M4-I1 (★) |
| **Identity: resolve_pseudonym/erase + pseudonym grammar `<pseudonym>@<tenant>.noreply` (4.8)** | Id / M1 | pseudonymous assignee/reporter/actor; DSR step 1 | M4-I1 (★) |
| Identity: ReBAC engine (4.9) accepting the Issues fragment | Id / M1 | `issue` namespace + field/transition caveats + `watcher` + `- confidential` | M4-I1 (★) |
| Tenancy: (tenant, region) partition (12.1); discover/placement_of (12.2); residency_verify (12.4); isolation tiers (12.5) | Tenancy / M1 | the partition key on every table; cell placement honouring residency | M4-I1 (★) |
| **Storage: OLTP tier + RLS + encrypted columns + the outbox (11.1)** | Storage / M1 | the `issue` spine + relations + schemes + change-log + the outbox | M4-I1 (★) |
| Storage: BlobStore content-addressed, fs-backed floor (11.2) | Storage / M1 | attachment bytes (the row holds the pointer) | M4-I4 |
| Storage: KMS hierarchy + per-subject DEK (11.3/11.4) | Storage / M1 | crypto-shred for free-text title/props/comment/change-delta columns | M4-I1, M4-I8 |
| **Storage: backup/restore + restore-verify, RPO ≤ 5min / RTO ≤ 1h-tenant (11.5)** | Storage / M1 | the silent-data-loss floor; Issues does not write real data over a red STOR-D1 | M4-I1 (★) |
| Storage: OLAP read store + restriction-flag (11.6) | Storage / M1→M2 | CFD/cycle-time/velocity/SLA-compliance analytics off the bus | M4-I5 |
| Storage: reserve/settle cost gate (11.7) | Storage+Commercial / M1 | fronts every spend-bearing agent run (triage/forecast/sla-draft) | M4-I6 |
| GDPR: PersonalDataHolder spine + classify-derive + erasure ledger (10.1/10.2/10.8) | GDPR / M1 | Issues registers as a holder; the worklog/free-text tags; post-restore re-erasure | M4-I1, M4-I8 |
| **myelin-content taxonomy frozen + WASM target (13.1)** | Knowledge (leads) / M2 | the issue-body/comment block subset + the three inline ref nodes; `render(parse(md)) === md` | M4-I1 (★) |
| ADF→content lossy-map frozen (13.2) | Knowledge / M2 | the import conversion table Issues consumes | M4-I5 |
| **myelin-query frozen byte-identical (field-type enum + ViewSpec + QueryAst + order_key) (13.3)** | Issues+Knowledge co-own / M2 | the view model, the guard/trigger/SLA predicate language, the rank codec — all byte-identical with Knowledge | M4-I1 (★) |
| Refs: ArtifactRef parse/format (5.1) + project REQUIRED (5.6) + resolve/tombstone ladder (5.2/5.7) + traverse (5.3) + TE-7 mirror (5.5) + refs.edge via content nodes (5.4) | Refs / M2 | the `<PROJECTKEY>-<seqno>` id; the context pane unfurl; the typed-edge mirror | M4-I3 (★) |
| Search: query (conjoins the Filter) + declare_indexable + reindex (6.1/6.3/6.4) | Search / M2 | the Tier-3 escalation valve; the `issue.*` projection | M4-I3 |
| Notif: list_inbox + mark/snooze + humanise + oncall_now/page + define_notif_rule (7.1/7.2/7.3/7.5/7.6) | Notif / M2 | "My Work" over the ONE inbox; the SLA escalation chain; the ONE templating surface | M4-I5, M4-I7 |
| Workflow: DurableExecutor + WfCtx + timer wheel + durable signal (9.1/9.2/9.3/9.4) | Workflow / M2 | SLA timers, trigger `stale_after`, snooze, multi-day HITL — Issues never rebuilds durable waits | M4-I6, M4-I7 |
| Agent: register_tool + EffectApi::apply + AgentRuntime::step (--use-mock) + ToolHands::exec + run --dry-run (8.1–8.4/8.7) | Agent / M2 | the ToolDef catalogue; plan-then-apply; the mock runtime; the unified sandbox | M4-I6 (★) |
| Bus: arm_trigger/disarm_trigger + EventMatcher=QueryAst + reindex + the firehose resume-cursor protocol (3.3/3.4/3.5/2.6) | Bus / M2 | the stateful Trigger; real-time board sync; reindex-from-source | M4-I6, M4-I7 |
| **AG-D4 / CI-T1 (real-kernel sandbox escape = 0)** | Agent+CI / M2 GATE | no Issues agent tool runs until green | M4-I6 (★) |
| **The X-1 CheckStatus seam (5.9): CI producer (M4) + Git projection (M3)** | CI+Git / M3+M4 | the "can't mark Done while CI red" guard reads it via the linked PR's `project` | M4-I7 (★, closes in M4) |
| Tenancy: cross-cell CrossCellPointer bridge frame (12.6) | Tenancy / M1 frame, M5 live | the cross-cell portfolio rollup follow-on | M5-I9 |

**The compounding-payoff check (EI-01 closing).** If Issues is built right on this substrate, each new view
(board → roadmap → backlog → table → calendar → cycle) is *smaller* than the last, because each is just another
`ViewSpec` over the one `issue` table conjoined with the same SetExpr Filter. If a new view needs a new object
graph or a private back-channel, the substrate is wrong — stop and repair, do not add feature surface.

---

## 3. The milestones (each mapped to a master band, with the work)

### 3.0 — Pre-work in M1/M2 (the freeze-so-dependents-compile slice)

**Band:** M1 (the ReBAC fragment + the holder tags) and M2 (the shared-crate co-ownership + the event tokens).
This is the only Issues work that happens *before* the M4 producer-band entry, and it exists so that dependents
compile and the frozen shapes cannot drift.

**Work:**
- **Freeze the Issues ReBAC namespace fragment (4.9)**: the `issue` definition (`parent_project`, `assignee`,
  `watcher`, `confidential`, `confidential_grant`; `view = (parent_project->read - confidential) +
  confidential_grant`; `transition = assignee + parent_project->write`; `manage = parent_project->write`) plus
  the `issue_field` and `issue_transition` caveat sub-objects. Identity owns the engine; Issues only declares the
  fragment + `write_tuples`. This must exist in M1 so the SetExpr reverse index can be populated for Issues' type.
- **Register the complete `issue.*` event taxonomy + the `initiative` type token (2.9)** under the Bus §6 grammar,
  with names/units aligned to the EventEnvelope anchor (timestamps RFC-3339 UTC; SLA targets/`stale_after`/
  durations in seconds; estimates/story-points numeric; actor/subject as ArtifactRefs;
  `contains_personal_data`/`data_role`/`pii_key_ref` on any PII-bearing event).
- **Co-own the frozen `myelin-query` crate (13.3)** with Knowledge: the field-type enum, the `ViewSpec`, the
  `QueryAst`, and the `order_key`/LexoRank codec (base-62 `0-9A-Za-z`, midpoint bisection, 2-char jitter, 48-char
  rebalance, `created_at`+ULID tiebreak). Issues contributes the storage discipline + owns its own AST→store
  compiler; the *definitions* are byte-identical with Knowledge. A round-trip + byte-identity test fixture lands
  here (the drift-killer).
- **Declare the worklog/productivity/estimate sensitivity tags (10.2, OQ-H)** so the `no-untagged-personal-data`
  lint passes when the schema lands: `#[personal_data(category = behavioural, role = tenant-content, basis =
  TBD-LEGAL, retention = tenant-policy)]`, restricted-by-default.
- **Declare the Issues `declare_indexable` IndexSpec (6.3)** and the `define_notif_rule` set (7.6) so Search and
  Notif know Issues' projection + reasons exist (the wiring lands in M4).

**Entry dependency:** M1 entry for the ReBAC fragment + holder tags (Identity engine + GDPR derive exist); M2
entry for the shared-crate freeze (the content/query crates are being frozen in M2).

**Exit gate (contributes to M1/M2 band gates, not an Issues-local gate):** the Issues ReBAC fragment compiles
into the cell schema; the `myelin-query` byte-identity fixture is green; the `no-untagged-personal-data` lint is
green on the (declared) worklog tags; the contract-coverage scanner sees Issues' declared contracts. No Issues
*data* is written yet.

---

### M4-I1 — The issue spine + the write path + the silent-data-loss-safe seam (the floor under all of Issues)

**Band:** M4. The first milestone that writes Issues data. **It must not be claimed done over a red STOR-D1
(restore-verify, M1) or a red ID-D3 (cross-tenant, M1).**

**Work:**
- The `issue` table (typed core + JSONB tail + the `(tenant, region)` partition key + the lifecycle/GDPR columns)
  and `issue_relation` (TE-7 source of truth, forward edge + one event; Refs mirrors both directions),
  `issue_change_log`, the `scheme`/`scheme_assignment` tables, `cycle`/`cycle_membership`/`milestone`,
  `prefix_counter`, `consumer_dedup`, and the per-service `outbox`.
- **The minimal write path:** validate → `Id.check` (+ CaveatContext) → Hi/Lo key allocation → `order_key` CAS →
  mutate the typed core → `OutboxTx::emit` *in the same transaction*. Every state-changing handler ends in the
  one sanctioned emit path (no `publish_now`; the `no-raw-publish` lint holds). The issue is the aggregate
  (`UNIQUE(aggregate, seq)` per-issue ordering).
- **Hi/Lo human-key allocation** per prefix (the frozen `<PROJECTKEY>-<seqno>` stored canonical id; `#1421` is
  render-time only), gap-tolerant, monotonic, adaptive block size, per-prefix isolation, cell-local.
- **Server-arbitrated `order_key` CAS** for drag-reorder (the frozen codec; 2-char jitter; the loser re-bases —
  no silent clobber). This is the CAS floor (move-CRDT is the M5 follow-on).
- **Pseudonymous-by-default identity columns** (`assignee`/`reporter`/`created_by` = pseudonymous principal ids,
  EI-04 §1) + per-subject-DEK encryption for free-text `title`/`props`/change-deltas (GD-4). Register Issues as a
  `PersonalDataHolder` (auto-registered by the harness, 1.4).
- The issue body as a `myelin-content` block subtree (the consumed subset; single-author CAS on the `version`
  token; the WASM render path); `render(parse(md)) === md` for bodies + comments.

**Floor named:** ranking = `order_key` + server-arbitrated CAS (move-CRDT is the named M5 follow-on); storage =
PG-hybrid sharded by tenant (distributed-SQL is the measured follow-on); rollup deferred to M4-I4.

**Exit gate (must be green to claim M4-I1):**
- **ISS-D10** (`render(parse(md)) === md` 100% over a body+comment corpus; read+edit use the identical WASM
  parser) — CI.
- **ISS-D4** (create-storm on one hot prefix, N workers → no duplicate key, monotonic per prefix, gaps benign,
  per-prefix isolation, key == the stored canonical `<id>`) — SCHED.
- **ISS-D5** (N humans + an agent re-ranking the same region → 0 silent clobber, bounded re-base, converges,
  48-char rebalance never reorders displayed order) — CI.
- **Outbox emit-iff-committed** for the issue write path (the SUB-D1/BUS-D4 shape applied to Issues: kill the
  service between commit and publish → the `issue.*` event is delivered exactly when its row committed, never
  without it) — CI.
- Upstream STOR-D1 and ID-D3 green (the gate invariant — Issues writes no real data over a red restore-verify or
  a red cross-tenant drill).

---

### M4-I2 — Governance schemes + the workflow interpreter (config, never a data migration)

**Band:** M4. Builds on M4-I1.

**Work:**
- The five scheme kinds (workflow/field/permission/sla/type) as interpreted JSONB config rows, assigned per
  (type × project × team); the deterministic, cached **scheme-precedence algebra** (most-specific-wins; off the
  hot path — the write loads the already-resolved compiled scheme).
- **The data-driven workflow FSM interpreter** with the **fixed state-category set**
  (`unstarted/started/completed/cancelled`) as the one mandatory governance invariant over unlimited named
  states; guards are the frozen `QueryAst` (bounded, no UDFs/loops/recursion — no Jira-Groovy footgun); required-
  fields-on-transition; post-actions (assign/set-field/link/arm-trigger). Assigning a new scheme is a config
  write, never a row migration.
- The flexible-field model: the JSONB property bag tail (zero-DDL custom fields) + the GIN index default; the
  `forward-only-migration` lint on the hot `issue`/`issue_relation`/`issue_change_log` tables.

**Floor named:** issue hierarchy = tree `parent` (constrained-DAG portfolios are the opt-in follow-on); the
projection-feeder generated-index promotion is deferred to M4-I3 (cold facets ride the GIN index until measured).

**Exit gate:** the workflow guard correctness slice of **ISS-D12** ("can't close while `blocked_by` an open
issue" → transition blocked with a pre-assembled reason; the CI-red half lands in M4-I7 when the seam closes);
no-config = Linear-simple proven (an org with zero assignments resolves to `org_default` for every kind, no
migration). The `flow-determinism` lint holds on any workflow body that schedules a durable activity.

---

### M4-I3 — The query planner: the SetExpr push-down, cost-bounding, the views, leak-free at scale

**Band:** M4. The zero-leak + flexible-field-latency milestone. This is where Issues' two highest-stakes
properties (D3 leak-free, D2 latency) are proven.

**Work:**
- **The AST→OLTP-store compiler** (Issues-owned) that **lowers the frozen `list_objects` SetExpr first** into a
  SQL predicate / JOIN against the per-tenant authz reverse index keyed on `issue.id`
  (`Ids`/`NotIds`/`InRelation{relation,via_column}`/`TupleSet`/Union/Intersect/Difference/All/None) — one query,
  no N+1, no post-filter. The `zookie` bounds staleness (a security-sensitive scan reads at-or-after the zookie's
  revision; the new-enemy guard).
- **Cost-bounding + the three-tier escalation:** Tier 1 typed-core index ranges (`issue_board`/`issue_roadmap`/
  `issue_assignee`), Tier 2 measured-hot generated indexes (the projection feeder, off the bus) / 2b GIN probe,
  Tier 3 escalate to Search **conjoining the same Filter** (the `search-requires-acl-filter` lint). Every query
  is paginated + statement-timeout'd; a query that would scan too much is pushed to Search or returns a `Refine`
  hint — never an unbounded JSONB scan.
- **The views as co-equal `ViewSpec` projections over the one `issue` table** (board / roadmap / backlog / list /
  table / calendar / cycle), each always conjoining the SetExpr Filter (a confidential issue is simply absent —
  no "N hidden" leak). The board↔roadmap co-equality is structural (same rows, `type_rank` denormalised).
- The projection feeder consumer (watches `issue.updated` deltas + a per-(tenant,type,field_id) frequency
  counter; provisions a generated/expression index via a forward-only online migration when a facet crosses the
  measured threshold — promotion is measured, never predicted; OQ-C calibration).
- Wire Refs `resolve` + `project(ref, viewer)` (the context-pane unfurl; pre-permission-checked; a confidential
  issue returns a tombstone carrying the root, never the title); the unified `#sub` grammar mint
  (`comment-`/`b`/`field-`/`row-`); `declare_indexable` + the Issues `git.*`-style `issue.*` Search projection.

**Floor named:** the projection-feeder promotion threshold is the OQ-C default-to-beat (`> 5%` of a collection's
view executions), calibrated by ISS-D2; distributed-SQL for a hot tenant is the measured follow-on (M5).

**Exit gate (the two make-or-break properties):**
- **ISS-D3** (cross-tenant + confidential-issue IDOR → not in any board / SetExpr JOIN / search / backlink /
  context-pane result, incl. under zookie staleness; 0 leak) — CI. *(This is the F1 leak-free family; it
  re-runs inside the surge family in M5.)*
- **ISS-D2** (50+ custom fields × 1M+ issues board query under the <1s keyboard budget with the SetExpr JOIN; a
  cold ad-hoc query escalates to Search with the same Filter; the planner never emits a full JSONB scan) —
  SCHED. *(This is also the OQ-C calibration drill.)*
- **ISS-D1** (edit on the board → roadmap reflects the same row, 0 drift, asserted by row id; and vice versa) —
  CI.

---

### M4-I4 — Rollup, cycles, milestones, attachments (the derived-aggregate breadth)

**Band:** M4. Builds on M4-I3.

**Work:**
- **The event-driven, debounced, incremental rollup consumer** (off the bus, never in the write path): walk
  parent edges (depth ceiling 16, visited-set, cycle-safe — a dependency cycle is a roadmap diagnostic, never a
  hang); debounce-coalesce a burst into one ancestor recompute; incremental re-sum; `input_hash` no-op
  suppression (stops loop storms, AG-6). The `rollup` row is derived (rebuildable by replay; edge truth stays in
  `issue_relation`).
- **The time axis:** cycles/sprints + milestones as separate objects (membership edges, not containment);
  burndown/CFD fed to OLAP off the bus; carry-over provenance.
- Attachments in BlobStore (content-addressed, residency-pinned; the row holds the pointer + per-subject-DEK
  metadata, not the bytes).
- **The OLAP read store wiring** (CQRS, reindex-from-source only, restriction-flag-honouring): CFD, cycle-time,
  velocity, SLA-compliance — never touching the OLTP `issue` table.

**Floor named:** rollup = read-time for small subtrees, materialise-on-measured-large (KN-3, the M5 follow-on);
the debounce-window + affected-ancestor fan-out policy is per-tenant-tunable, calibrated by ISS-D8a (OQ-K
floor); forecast deferred to M4-I6.

**Exit gate:**
- **ISS-D8** (a) rollup freshness under a 10k-issue import → a *bounded* number of ancestor recomputes (debounce),
  initiative progress correct within the window; (b) reindex-from-source: `replay` rebuilds the rollup aggregate +
  the Refs edge projection **drift-free** vs live — proving steady-state and recovery share one code path — SCHED.

---

### M4-I5 — Import + export + "My Work" over the ONE inbox (the adoption gate + the inbox)

**Band:** M4. The "leave Atlassian cleanly" sovereignty credibility milestone + the inbox surface.

**Work:**
- **The two-pass, ID-remapped, idempotent + resumable import engine** with a persisted source↔Myelin id map (the
  load-bearing artifact for idempotency/resume/rollback/round-trip); dry-run + reconciliation-report-first;
  source adapters (Jira/Linear/GitHub/CSV) normalising into one canonical interchange format that round-trips
  with the portability export. Import emits the normal `issue.*` events (one indexing path; reindex-from-source
  works on imported data for free), per-tenant in-flight capped (never starves another tenant — the protected
  human lane shed order).
- **Consume the frozen ADF→`myelin-content` lossy-map (13.2, Knowledge-owned):** every lossy/dropped conversion
  recorded in the import report, never silent. The status/date/custom-emoji/layout/macro/permission-scheme
  degradations are named (permission-scheme mapping is the lossy/legal-review leg, R-9).
- **"My Work" (S10) = `list_inbox(principal, filter)` over the ONE Notif inbox** (C-9): assigned/blocked/
  needs-approval/overdue are `reason`/`subject` filters with shared read-state — never a second store. Register
  the `define_notif_rule` set + the `humanise` templates (SLA at-risk/unblocked/approval-requested) into the ONE
  templating surface (no second template engine).

**Floor named:** import = canonical core + the four adapters + the frozen ADF map (permission-scheme mapping is
the named lossy leg); the canonical interchange is the round-trip oracle.

**Exit gate:**
- **ISS-D9** (a) `export→import→export` round-trips over a corpus, ADF lossy-map nodes named never silent; (b) a
  large import resumes after a crash with 0 duplicate creates (the id map); (c) the import doesn't starve another
  tenant (a concurrent interactive tenant's latency stays within budget) — SCHED.

---

### M4-I6 — The agent tool surface + reserve/settle + dry-run (agent-native, gated on AG-D4)

**Band:** M4. **Must not run any tool until AG-D4 / CI-T1 is green (M2 GATE).**

**Work:**
- **Register the Issues ToolDefs** into the one `ToolSurface` (the same catalogue the command palette + CLI +
  agents share — UI=CLI=agent parity, no privileged back-channel): create/update/transition/comment/link/
  estimate/reorder/assign/close + the agent tools forecast/triage/sla_draft. Each declares `required_caps`,
  `effect_kind`, `side_effecting`, `requires_approval`, `exposed_over_mcp`.
- **The frozen `requires_approval` defaults (X-6):** forecast/triage/sla_draft = no (suggest by default — the
  human accepts); `transition(→done)` on an SLA-bound issue = yes iff the transition has an approver edge;
  `close` = yes if confidential or governed. All side-effecting tools apply via `EffectApi::apply` (schema →
  capability → delegation → tenant → budget → HITL gate → apply via the public endpoint, no carve-out → meter).
  A withheld gated tool does not mutate (AG-8).
- **The forecast agent** (compute-only, reads OLAP; writes the `forecast` field + emits `initiative.health_changed`
  on crossing an at-risk threshold) and the **triage agent** (S9 suggestion strip via `run --dry-run` — proposed
  effects without applying). The runtime is the **mock** (`--use-mock`, scripted-deterministic) per VISION §3;
  the real-LLM runtime is the post-M5 swap.
- **Reserve/settle on every spend-bearing run** (reserve at dispatch — no balance, no start; settle on completion,
  never interrupt in-flight; integer minor-units; the same wallet as CI runs). The HITL approval card surfaces a
  live cost estimate before a human approves.
- The **stateful Trigger** flagship ("Remind me when unblocked"): the armable-condition catalogue, each a frozen
  `QueryAst` over `issue.*` events + `issue_relation` projection state (`Has`/`Ref`/`In`); consumes the bus
  `arm_trigger`/`disarm_trigger` + the `myelin-flow` `stale_after` durable timer + the one inbox for `on_resolve`;
  fires once per arming; after `stale_after` (default 30d) a stale nudge fires once and the trigger goes stale.

**Floor named:** agent runtime = mock (the real-LLM runtime is post-M5, after the safety drills, R-10); forecast =
linear `remaining ÷ velocity` (Monte-Carlo agent is the follow-on, R-5).

**Exit gate:**
- **ISS-D7** (arm "remind me when unblocked"; resolve the last blocker across a restart → fires exactly once into
  the one inbox; after `stale_after`, the stale nudge fires once, the trigger goes stale) — CI.
- **AG-D9 mock-determinism** applied to Issues' agent tools (identical effect sequences across replays) and
  **AG-D5 HITL withhold** applied to a governed `transition` (0 mutation pre-approval, 1 apply post-approval) —
  CI. *(These are the shared agent-fabric drills; Issues proves its tools obey them.)*
- Upstream AG-D4 green (the gate invariant).

---

### M4-I7 — SLA business-calendar engine + the CheckStatus guard (closing the X-1 consumer)

**Band:** M4. This milestone closes the Issues side of the X-1 seam — it requires the CI producer (M4) and the
Git projection (M3) to exist, so it lands late in M4.

**Work:**
- **The SLA logic engine over `myelin-flow`:** the business-calendar arithmetic (convert a business-time budget
  into a wall-clock `fire_at` over an IANA-tz calendar; DST/holiday/multi-day correct); precompute `fire_at` +
  `at_risk_fire_at`, arm two SC-11 timers; cheap disarm/re-arm on pause/resume (the `QueryAst` `pause_conditions`);
  never poll, never pollute the wheel with calendar logic. On breach, start the **frozen escalation chain** (`page
  → oncall_now → escalate-after-timer`) as a durable workflow; breach/met feed OLAP for compliance reporting.
- **The CI-red governed-transition guard (the X-1 consumer half):** the "can't mark Done while CI red on the
  linked PR" guard reads the linked PR's commit `CheckStatus{state, trust_tier}` via `project(PR_ref)` at
  transition time — checks `state = success` **and** an acceptable trust posture (an `untrusted_fork` success is
  neutral until endorsed). Issues **never recomputes trust** — it reads `trust_tier` off the fact. The agent
  hitting this governed transition is HITL-gated.
- The cross-subsystem consumers (the cross-sub reflexes): `git.branch.created`/`git.pr.opened`/`git.pr.merged` →
  link + workflow-permitting auto-transition; `chat.message.created` → create issue with a `relates` edge;
  `identity.member.*` → reassign/anonymise; `ci.check.updated` → feed the guard.
- The governance admin views (S13 workflow/scheme editor with the `QueryAst` guard builder; S14 SLA policy editor
  + calendar editor + breach-simulation; S15 team/project settings + the permission inspector via
  `list_subjects`/`explain`; S16 automation/trigger builder; S18 audit/change-history).

**Floor named:** very-long `time_to_resolution` SLAs get history-compaction (the `myelin-flow` continue-as-new
note) as the named follow-on, R-11.

**Exit gate (also contributes to the M4 band exit, master §2):**
- **ISS-D6** (a) breach fires after a process restart; (b) a business-calendar corpus (DST, multi-day, holiday,
  pause/resume) → computed `fire_at` matches wall-clock to the second; (c) breach starts the escalation chain —
  CI.
- **ISS-D12** complete (the CI-red guard: "can't mark Done while CI red on the linked PR" reads CheckStatus +
  trust posture → transition blocked with a reason; "can't close while `blocked_by` open" blocks; an agent hitting
  a governed transition is HITL-gated, withheld, 0 mutation pre-approval) — CI.
- Upstream **GIT-D10 / CI-D8** green (the X-1 seam end-to-end; the gate invariant — Issues' guard rests on a
  proven seam, not a doc claim).

---

### M4-I8 — Real-time board sync + erasure-reaches-every-holder (the M4 consumer-band exit slice)

**Band:** M4. The last M4 milestone before the band exit; it proves reconnect-loses-nothing and the GDPR
holder fan-out.

**Work:**
- **Real-time board sync over the frozen firehose resume-cursor protocol:** optimistic local updates + bus-driven
  cache invalidation; `subscribe(stream, scope = board:<id>)` (bounded, never `*`; a 50k-row board paginates its
  scope); on reconnect `resume(stream, scope, last_seq)` backfills `(last_seq, now]` then live — loses zero ops;
  `last_seq` past the retention window → `resync_required` → `*.snapshot` replay (named, not silent). Per-
  connection in-flight frame caps; a slow consumer is dropped to `resync_required` (the OQ-K per-surface shed
  budget). Presence/typing ride the ephemeral firehose, never the durable bus.
- **Erasure-reaches-every-holder:** implement the `PersonalDataHolder` ops (locate/export/rectify/restrict/erase)
  across every Issues holder (the `issue` row free-text via per-subject DEK shred, the change-log deltas, comments,
  attachment blobs, the OLAP read store + restriction flag, the Search index incl. embeddings, the Refs
  projection). Id `erase` shreds the pseudonym map ("Former user 8a2f" across history without rewriting issues
  others own); emit `issue.*.erased` tombstones (live consumers tombstone Search/Refs/OLAP/Notif); post-restore
  re-erasure (GD-14) runs against the erasure ledger. The third-party free-text residual is handled per the ONE
  platform posture by reference (contract 10.9), `[OPEN — LEGAL]`.

**Floor named:** free-text PII erasure = per-subject DEK + pseudonym-map shred + `restrict` (the structural floor
ships now; the third-party-mention residual basis is `[OPEN — LEGAL]`, R-1); sync = optimistic + resume-cursor
(offline/local-first is the named follow-on, R-8, out of v1 scope unless promoted); worklog special-category
classification is `[OPEN — LEGAL]`, R-2.

**Exit gate (the M4 band-exit drills Issues owns, master §2 / §4 M4→M5):**
- **ISS-D13** (a board at `scope = board:<id>` drops mid-edit-storm → `resume` backfill then live loses zero ops;
  `last_seq` past the window → `resync_required` → `*.snapshot`) — CI.
- **ISS-D11** (erase a subject → PII gone from every holder: per-subject DEK, change-log, comments, attachments,
  OLAP + restriction, Search incl. embeddings, Refs; post-restore re-erasure catches a restore; the third-party
  residual is the documented `[OPEN — LEGAL]` limit) — SCHED.

**M4 band exit (Issues' contribution to master §4 M4→M5):** ISS-D1 / ISS-D2 / ISS-D3 (co-equal, latency,
IDOR-0-leak) + ISS-D5 / ISS-D6 / ISS-D12 (reorder-0-clobber, SLA to-the-second across restart, guard blocks) all
green. The X-1 seam (GIT-D10 / CI-D8) green. No earlier-band gate red.

---

### M5-I9 — World-scale hardening + the floor follow-ons + the E2E wedge (the hard-problem band)

**Band:** M5. With all five subsystems on one substrate and the deterministic correctness drills green, prove
Issues under world-scale load and ship the named follow-ons.

**Work — the floor follow-ons (each was named in its band; here is its scheduled follow-on, master §5):**
- **The move-CRDT, after the CAS floor** (R-3, EI-04 §2): a Yrs list / Fugue move-CRDT slotting into the same
  resume-cursor firehose transport, reusing Knowledge's Yrs type. Promoted **only on measured concurrent-reorder
  pain** (the trigger). Because the `order_key` is already byte-identical, the promotion swaps the
  conflict-resolution engine, not the data model. ISS-D5 re-runs across the engine-promote boundary so it stays
  green when the CRDT lands.
- **Materialised rollup, after the read-time floor** (R-4, KN-3): materialise a subtree's rollup only when it is
  *measured* large; the read-time floor remains for small subtrees.
- **Distributed-SQL, after PG-sharded-by-tenant** (R-6): only if a single tenant's shard is *measured* to outgrow
  PG. Never premature.
- **Cross-cell portfolio rollup, after single-cell** (R-7, OQ-I): the rollup walk over a remote child rides the
  frozen PII-free `CrossCellPointer{subject, type, correlation_id, home_cell}`; resolution is always cell-local
  (the home cell renders + permission-checks; only the projection crosses). The FLOOR drills GA-D8 / CP-D7 / CP-D8
  are now owed (DSR fan-out iterates `member_cells`).
- **The Monte-Carlo forecast agent, after the linear floor** (R-5): reads OLAP throughput samples; the swap is a
  strategy change, not a rewrite. (The real-LLM runtime swap, R-10, is post-M5 / execution — after the safety
  drills are green; a config/impl swap per VISION §3.)
- **The full DSR / erasure fan-out** (10.4, GA-D1): every Issues holder now exists, so the fan-out is complete;
  the `[OPEN — LEGAL]` residual posture (10.9) is instantiated by reference.
- **The event-volume column-store seam** (EI-04 §5): a seam for Issues' highest-volume streams (`issue.updated`,
  the change-log) — added only once volume is *measured*, not before.

**Work — world-scale hardening (the F6 surge family + the scale drills):** the 30x surge across the Issues owner
(the protected human lane holds within budget; the agent lane sheds 429+Retry-After; cross-tenant impact 0); the
prod-scale benchmarks (the 1M+-issue board, the 50-team-initiative rollup fan-out, millions of SLA timers as an
indexed range read); online-migration-under-load on the hot `issue` tables; restore-verify at cell scale.

**Work — the whole-system E2E wedge (Issues' participation, catalogue §E2E):**
- **E2E-1 PR context pane** (Git+CI+Issues+Knowledge+Refs+Search+Id+Notif): Issues' `project` resolves the linked
  issue per-viewer with 0 leak; the live check-update is within the freshness budget; a tombstone carries the root.
- **E2E-2 CI-fail → triage agent → issue → chat → fix-PR** (the agent-native flagship): Issues is the node where
  the triaged failure becomes a tracked, governed work item; 0 effect outside the `∩`; 0 mutation before approval;
  exactly-once approval + the governed transition across a kill; reserve/settle balanced.
- **E2E-3 Spec-to-ship traceability** (Knowledge+Issues+Git+CI+Chat+Refs+Search+GDPR+Id): the spec→issue→PR→CI
  lineage per-viewer; cold-reindex == live (the reindex-from-source parity); audit tamper detected.

**Exit gate (Issues' contribution to master §4 M5→M6):**
- **The F6 surge family** across the Issues owner (SUB-D3-shaped: human lane within budget, agent sheds,
  cross-tenant impact 0) — SCHED.
- **ISS-D2 at cell scale** re-confirmed (the 1M+-issue board under the <1s budget under world-scale load) — SCHED.
- **ISS-D5 re-green across the move-CRDT engine-promote boundary** (if the CRDT is promoted) — CI.
- **GA-D1 / CP-D7 / CP-D8** (DSR fan-out 0 holders missed incl. Issues; cross-cell rollup per-cell receipt set +
  the PII-free bridge) — SCHED.
- **E2E-1 / E2E-2 / E2E-3 green** (each emitting its named green artifact) — SCHED.

---

### M6-I10 — Dogfood: Myelin tracks its own issues (the switch test)

**Band:** M6. The done-bar for Issues as a product.

**Work:**
- The Myelin roadmap + gap report + scorecard live as **Myelin issues** (the every-incident-adds-a-drill loop
  files a Myelin issue + a reproducing drill); the team plans its own sprints on the platform's own board/roadmap.
- Drive the real UI of the Issues surfaces (board/roadmap/backlog/table/cycle/triage/My Work + the admin/
  governance screens) in a browser for the **switch test** (EI-01 §4, the frontend done-bar L5).

**Exit gate (the done-bar):**
- **ISS-D14** (switch test: can a Jira/Linear user complete the core loop create → triage → plan → board → done
  without a manual? + measured contrast/latency on the primary screens S1/S3/S5/S6/S9/S10/S13/S17/S19, incl. the
  empty/loading/error/permission/erased/agent-pending states — driven in a browser, not read off a feature list)
  — SCHED.
- No later-band gate red (the truth-up pass confirms every PROVEN Issues row rests on a dated green artifact;
  code-wins-over-docs).

---

## 4. The contracts Issues must implement, by milestone

Per the contract index (the frozen build-to surface). "Implement" = Issues is the consumer (calls it) or the
co-owner (Issues + Knowledge for `myelin-query`); the *owner* column in the index is unchanged.

| Contract | What Issues does | By milestone |
|---|---|---|
| 1.1–1.4 serve/three-surface/liveness≠readiness/PersonalDataHolder auto-reg | boot the Issues service from the shell; register as a holder | M4-I1 |
| 1.5 forward-only migrations + hot-table flags | flag `issue`/`issue_relation`/`issue_change_log` hot; expand→backfill→contract | M4-I1 |
| 1.6 the 12 lints | compile clean against tenant-predicate / no-raw-publish / no-cross-db / residency-pin / search-requires-acl-filter / no-untagged-personal-data / flow-determinism | 3.0 (declare) → all |
| 1.9/1.10 ResilientClient / FailStatic | Issues→Id, Issues→CheckStatus-projection calls degrade not cascade | M4-I2 |
| 1.11 protected-human-lane shed order + per-surface budgets | the import + board-sync + agent-mention shed budgets (OQ-K floors) | M4-I5, M4-I8, M5-I9 |
| 2.1/2.2/2.3/2.4/2.5 envelope/outbox/EventHandler/dedup | the `issue.*` shapes; the only emit path; the rollup/SLA/trigger/feeder consumers | M4-I1 |
| 2.6 reindex-from-source (`replay`) | sub-artifact-granular `*.snapshot`; the only recovery path for derived stores | M4-I4 (proven D8b) |
| 2.9 event taxonomy + the `initiative` token | the complete `issue.*` list registered | 3.0 |
| 3.3/3.4 arm_trigger / EventMatcher=QueryAst | the stateful Trigger; the armable-condition catalogue | M4-I6 |
| 3.5 firehose resume-cursor protocol | real-time board sync (`subscribe`/`resume`/bounded `scope`) | M4-I8 |
| 3.6 reactive/dispatch + reserve/settle-before-run | the agent-via-automation/trigger dispatch | M4-I6 |
| 4.2 check + CaveatContext | per-action write gate; field/transition ABAC | M4-I1, M4-I7 |
| 4.3 list_objects SetExpr push-down | the planner lowers it first over `issue.id`; the leak-free pre-filter | M4-I3 (★) |
| 4.4 list_subjects + explain | watcher read-fanout; the permission inspector (S15) | M4-I7 |
| 4.5/4.7 delegation / mint_run_token | agent run policy intersection + per-run token | M4-I6 |
| 4.6/4.10 write_tuples/zookie | assign/watch/confidential-grant; read-your-writes (new-enemy guard) | M4-I1, M4-I3 |
| 4.8 resolve_pseudonym/erase + grammar | pseudonymous identities; DSR step 1 | M4-I1, M4-I8 |
| 4.9 ReBAC fragment | the `issue` namespace + field/transition caveats + watcher + `- confidential` | 3.0 (★) |
| 5.1 ArtifactRef `<PROJECTKEY>-<seqno>` | the stored canonical id; `#1421` render-time | M4-I1 |
| 5.2/5.6 resolve / project | the context-pane unfurl; the only cross-DB read of an Issues artifact | M4-I3 |
| 5.3 traverse | the bounded cycle-safe ancestor walk (depth 16) | M4-I4 |
| 5.4 refs.edge via content nodes | inline mention/artifact_ref produce edges | M4-I1, M4-I3 |
| 5.5 TE-7 typed-edge mirror | own `issue_relation` (forward edge + one event); Refs mirrors both directions | M4-I1 |
| 5.7 unified `#sub` grammar + tombstone ladder | mint `comment-`/`b`/`field-`/`row-`; stable opaque ids | M4-I3 |
| 5.9 the Git↔CI CheckStatus seam (consumer) | read `CheckStatus{state, trust_tier}` via the linked PR's `project`; never recompute trust | M4-I7 (★) |
| 6.1/6.3/6.4 query (conjoins Filter) / declare_indexable / reindex | the Tier-3 escalation valve; the `issue.*` projection | M4-I3 (declare 3.0) |
| 7.1/7.2 list_inbox / mark/snooze | "My Work" over the ONE inbox; one read-state truth | M4-I5 |
| 7.3 humanise (the ONE templating surface) | SLA/unblocked/approval strings register here; per-viewer ref resolution | M4-I5, M4-I7 |
| 7.5 oncall_now/page + the frozen escalation chain | breach → `page → oncall_now → escalate-after-timer` | M4-I7 |
| 7.6 define_notif_rule | the Issues reason set (SLA at-risk/unblocked/approval) | 3.0 (declare) → M4-I5 |
| 8.1 register_tool + frozen requires_approval defaults | the Issues ToolDefs; X-6 defaults | M4-I6 |
| 8.2/8.4/8.7 EffectApi::apply / ToolHands::exec / run --dry-run | plan-then-apply; the unified sandbox; dry-run proposals | M4-I6 |
| 8.3 AgentRuntime::step (--use-mock) | the mock runtime (real-LLM is post-M5) | M4-I6 |
| 9.1/9.2/9.3/9.4 DurableExecutor / WfCtx / timer wheel / durable signal | SLA timers, `stale_after`, snooze, multi-day HITL — Issues never rebuilds durable waits | M4-I6, M4-I7 |
| 9.5 workflow↔agent reserve/settle bookends | the spend-bearing agent run as a durable workflow | M4-I6 |
| 10.1/10.2/10.8/10.9 PersonalDataHolder / classify / erasure ledger / the ONE posture by reference | the holder ops; the worklog/free-text tags; post-restore re-erasure; the residual by reference | 3.0 (tags) → M4-I8 |
| 11.1/11.2/11.3/11.4/11.5/11.6 OLTP / BlobStore / KMS / per-subject DEK / restore-verify / OLAP | the spine + attachments + crypto-shred + analytics; Issues writes no data over a red STOR-D1 | M4-I1, M4-I4 |
| 11.7 reserve/settle cost gate | fronts every agent run (same wallet as CI) | M4-I6 |
| 12.1/12.2/12.4/12.5 partition key / placement / residency_verify / isolation tiers | the `(tenant, region)` partition on every table; cell-local | M4-I1 |
| 12.6 cross-cell CrossCellPointer bridge | the cross-cell portfolio rollup follow-on (frame frozen M1, live M5) | M5-I9 |
| 13.1/13.2 myelin-content subset + ADF map | the issue-body block subset; the import conversion | M4-I1, M4-I5 |
| 13.3 myelin-query byte-identical (co-own) | the field-type enum / ViewSpec / QueryAst / order_key; Issues owns its compiler | 3.0 (★) |

---

## 5. The floors register (name the floor, name the follow-on — VISION §3, EI-04 §4)

Each floor ships in its band; each is tracked in the gap report with claimed/proven status and its linked
follow-on. The gap being *invisible* is the only failure.

| Floor (ships) | Band | The full answer (follow-on) | Band | The trigger |
|---|---|---|---|---|
| Issue hierarchy = tree `parent` | M4-I2 | Constrained-DAG portfolios (opt-in per `type_scheme`) | M5+ | cross-team multi-parent demand |
| Ranking = `order_key` + server-arbitrated CAS (no merge, loser re-bases) | M4-I1 | Move-CRDT (Yrs list / Fugue), same byte-identical `order_key` | M5-I9 | *measured* concurrent-reorder pain (R-3) |
| Rollup = read-time for small subtrees | M4-I4 | Materialise-on-measured-large (KN-3) | M5-I9 | a subtree *measured* large (R-4) |
| Forecast = linear `remaining ÷ velocity` (mock agent) | M4-I6 | Monte-Carlo forecast agent (reads OLAP) | M5-I9 | promotion (R-5) |
| Flexible-field index = GIN default | M4-I3 | Generated projection-feeder index per hot facet | M4-I3/M5 | a facet in > 5% of view executions, *measured* (OQ-C) |
| Storage = PG hybrid sharded by tenant | M4-I1 | Distributed-SQL | M5-I9 | a single tenant's shard *measured* to outgrow PG (R-6) |
| Sync = optimistic + frozen resume-cursor | M4-I8 | Offline / local-first | post-M5 | promotion (R-8) |
| SLA = full business-calendar logic over the wheel | M4-I7 | History-compaction for very-long `time_to_resolution` SLAs | M5+ | *measured* long-SLA history (R-11) |
| Import = canonical core + Jira/Linear/GitHub/CSV + the frozen ADF map | M4-I5 | Permission-scheme mapping (lossy/legal-review leg) | M5+ legal | named-lossy now, never silent (R-9) |
| Free-text PII erasure = per-subject DEK + pseudonym-map shred + `restrict` | M4-I8 | Third-party-mention residual basis is `[OPEN — LEGAL]` | parallel (legal) | DPO/counsel ratify one statement (R-1) |
| Worklog/productivity classification = `behavioural`/`restricted-by-default` tags | 3.0/M4-I1 | Special-category vs elevated ratification + works-council trigger | parallel (legal) | counsel + works-council (R-2) |
| Single-cell complete | M4 | Cross-cell portfolio rollup over the `CrossCellPointer` bridge | M5-I9 | cross-cell rollup demand (R-7); GA-D8/CP-D7/CP-D8 owed |
| Agent runtime = mock (`--use-mock`), ToolDefs registered | M4-I6 | The real-LLM runtime (region-aware, EU-hostable) | post-M5 / execution | after AG-D4/D2/D3/D5 green; config/impl swap (R-10) |

---

## 6. The honest first-runnable / first-useful / production-hardened progression

- **First runnable (end of M4-I1):** a single tenant in a single cell can create an issue, give it a
  `<PROJECTKEY>-<seqno>` key, edit its typed core + JSONB tail, link a typed relation, reorder it, and have every
  write co-commit its `issue.*` event through the outbox — with the body round-tripping `render(parse(md)) === md`
  and free-text under a per-subject DEK. It is permission-gated by `Id.check`. **Not yet useful:** no schemes, no
  views beyond a raw list, no rollup/SLA/import/agents, no real-time sync. *Honestly partial: the spine + the
  safe seam, named.*

- **First useful (end of M4-I5):** an engineering team can run its real work — governed workflows over the fixed
  state-category set, a co-equal board + roadmap + backlog over the one `issue` table (leak-free at scale, under
  the <1s budget), rollup to epics/initiatives, cycles/sprints, "My Work" over the one inbox, and a one-shot
  import from Jira/Linear/GitHub/CSV with a named lossy report. The switch from another tool is *possible* (the
  data comes in, the daily loop works). **Not yet hardened:** agents are not wired, SLA + the CI-red guard are not
  closed, erasure fan-out + real-time sync + the 30x surge are not proven.

- **Production-hardened (end of M5-I9):** Issues is proven under world-scale load (the 30x surge with the human
  lane holding and the agent lane shedding, cross-tenant impact 0; the 1M+-issue board at cell scale; millions of
  SLA timers); the agent surface is reserve/settled and HITL-gated on a green AG-D4; the X-1 CheckStatus guard
  rests on a proven seam (GIT-D10/CI-D8); erasure reaches every holder with post-restore re-erasure; the floor
  follow-ons (move-CRDT, materialised rollup, cross-cell rollup, Monte-Carlo forecast) are promoted on *measured*
  evidence; and Issues carries its weight in E2E-1/E2E-2/E2E-3. **The done-bar (M6-I10):** the switch test passes,
  driven in a browser, and Myelin tracks its own issues.

---

## 7. Digest

**Milestones (band → milestone → the work):**
- **3.0 (M1/M2 pre-work):** freeze the Issues ReBAC fragment + the `issue.*` tokens (incl. `initiative`) +
  co-own `myelin-query` byte-identical + declare the worklog tags / IndexSpec / notif-rules — so dependents
  compile and the shapes don't drift.
- **M4-I1:** the `issue` spine + the silent-data-loss-safe write path + Hi/Lo keys + `order_key` CAS +
  pseudonymous identities + the content body. *(gates: ISS-D10, ISS-D4, ISS-D5, outbox emit-iff-committed)*
- **M4-I2:** governance schemes + the workflow interpreter (config, never a migration; fixed category set;
  `QueryAst` guards). *(gate: ISS-D12 guard half)*
- **M4-I3:** the query planner — the SetExpr push-down, cost-bounding, the co-equal views, leak-free at scale.
  *(gates: ISS-D3, ISS-D2, ISS-D1)*
- **M4-I4:** rollup + cycles/milestones + attachments + OLAP. *(gate: ISS-D8)*
- **M4-I5:** import/export + "My Work" over the ONE inbox. *(gate: ISS-D9)*
- **M4-I6:** the agent tool surface + reserve/settle + dry-run + the stateful Trigger (gated on AG-D4).
  *(gates: ISS-D7, AG-D5/AG-D9 applied)*
- **M4-I7:** the SLA business-calendar engine + the CheckStatus guard (closes the X-1 consumer). *(gates: ISS-D6,
  ISS-D12 complete; needs GIT-D10/CI-D8)*
- **M4-I8:** real-time board sync + erasure-reaches-every-holder (the M4 band-exit slice). *(gates: ISS-D13,
  ISS-D11)*
- **M5-I9:** world-scale hardening + the floor follow-ons + E2E-1/E2E-2/E2E-3. *(gates: F6 surge, ISS-D2 at cell
  scale, ISS-D5 re-green across CRDT boundary, GA-D1/CP-D7/CP-D8, the E2E wedge)*
- **M6-I10:** dogfood — Myelin tracks its own issues; the switch test. *(gate: ISS-D14)*

**Floors + follow-ons (named):** CAS ranking → move-CRDT (M5, measured); read-time rollup → materialised (M5,
measured); linear forecast → Monte-Carlo (M5); GIN facet → projection-feeder generated index (measured, OQ-C);
PG-sharded → distributed-SQL (M5, measured); optimistic+resume sync → offline/local-first (post-M5); single-cell
→ cross-cell over the `CrossCellPointer` bridge (M5); mock agent runtime → real-LLM (post-M5, after AG-D4);
per-subject-DEK erasure → the third-party residual is `[OPEN — LEGAL]` (legal, parallel); worklog tags →
special-category ratification (legal, parallel).

**Critical upstream dependencies (must exist + be green before the named Issues milestone):**
- **M0:** the outbox + EventEnvelope + the 12 lints + the failure-injection harness (the only emit path, the
  ratchet) — gates M4-I1.
- **M1 (the dependency root + the data-loss floor):** Identity `check`+`CaveatContext` / `list_objects` SetExpr
  push-down / ReBAC engine / `resolve_pseudonym`+`erase`; Storage restore-verify (STOR-D1 — the silent-data-loss
  floor, Issues writes no data over a red one) + KMS per-subject DEK; Tenancy `(tenant, region)` partition +
  residency. — gate M4-I1/M4-I3.
- **M2 (the reactive layer + the safety GATE):** the frozen `myelin-content`/`myelin-query` crates; Refs
  `project`/`resolve`/TE-7 mirror; Search (conjoins the Filter); Notif (the ONE inbox + humanise + escalation);
  Workflow (the timer wheel + durable signal); the agent fabric (ToolDefs/EffectApi/mock runtime/the unified
  sandbox); the firehose resume-cursor protocol; **AG-D4 / CI-T1 green** (no Issues agent tool runs over a red
  one). — gate M4-I3/M4-I6/M4-I8.
- **M3+M4 (the X-1 seam):** the Git CheckStatus projection (M3) + the CI producer (M4), proven end-to-end by
  **GIT-D10 / CI-D8** — gates M4-I7 (the "can't mark Done while CI red" guard rests on a proven seam, not a doc
  claim).
