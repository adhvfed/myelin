# Issue Tracker — 05 · Hard Problems Resolved (with cited prior art + named floors)

> See [`00-overview.md`](./00-overview.md) for the framing. This doc consolidates the **resolution of every
> subsystem-specific hard problem**, each with **cited prior art** and a **named floor** where v1 is partial, now
> conformed to the **frozen** reconciled contracts. The detailed mechanics live in
> [`01`](./01-tech-and-data-model.md)/[`02`](./02-internals-and-algorithms.md)/[`03`](./03-events-contracts-and-glue.md);
> this doc is the decision register. The Stage-1 exploration (candidates weighed) is in
> [`../sketches/`](../sketches/) (PRESERVED).

---

## 1. Issue-model duality — board↔roadmap as co-equal views; Epic/Initiative type-vs-level (PR-2)

**The make-or-break UX bet.** Get it wrong → PMs get a parallel reality (design-language §2).

**Resolution (Candidate C, sketch 01):** **one `issue` table = the ranked-type containment spine**
(sub-task → story/bug/task/chore/spike → **epic → initiative**, all `issue` rows). `parent` is the single
containment edge in `issue_relation` (the TE-7 source of truth, contract 5.5). **The board and the roadmap are
co-equal `myelin-query` `ViewSpec` views over that one table** — board = `type_rank ≤ 1` grouped by
`state_category`; roadmap = `type_rank ≥ 2` on a date axis ([04 §1](./04-views-cli-and-api.md)). The three axes
stay separate ([00 §1.1](./00-overview.md)): cycle/sprint = a separate time-axis object (membership, not
containment); project/space = the identity scope object (Id §5), never re-invented. Rank is config (`type_scheme`)
with **rank-monotonic parenting** as the default guardrail. `type_rank` is denormalised so both views are
index-range scans.

**Why structural, not a feature:** the roadmap **cannot drift** from the board because they read the *same rows*.

**Prior art cited:**
- The **frozen ArtifactRef grammar pre-decided it** — the token table lists `epic` as an `issue`-family *type*
  (and now registers `initiative`, contract 2.9 / index §14), ruling out the "Projects/Initiatives are separate
  tables" model for the spine.
- **Jira's Epic-as-type** (an issue with `issuetype=Epic`) — adopted, with the fix that **rank is a clean
  ordering and `parent` is the one containment edge** (no parallel "Epic Link").
- **Linear's first-class Cycles/Projects** — adopted for the *genuinely-different* axis-objects (a cycle has no
  workflow state; modelling it as an issue is the awkward-nulls problem).

**Floor → follow-on:** tree `parent` is v1; **constrained-DAG portfolios** (cross-team multi-parent) are opt-in
per `type_scheme`, the named follow-on. Rollup cycle-detection already handles the DAG lateral edges.

---

## 2. Governance — baked-in vs opt-in schemes (PR-3)

**Resolution (Candidate C, sketch 02):** **typed-core columns (hot path) + layered optional schemes interpreted
(data-driven), not baked.** The **fixed state-*category* set** (`unstarted/started/completed/cancelled`) over
unlimited named states is the **one mandatory invariant** (cross-project reporting reads the category). Workflow/
field/permission/SLA/type schemes are assigned per (type × team/project); **assigning a scheme is config, never a
data migration** ([01 §3](./01-tech-and-data-model.md)). The precedence algebra is deterministic + cached
([02 §1](./02-internals-and-algorithms.md)). Guards are the **frozen `myelin-query` `QueryAst`** (= the
`EventMatcher` core, contract 3.4), not scripting. Linear-simple = empty config; Jira-powerful = more schemes;
**one product, one code path, no fork.**

**Prior art cited:**
- **Jira's history of *lacking* a fixed category** is the cited failure (heterogeneous custom workflows that
  couldn't roll up). The fixed category is the HOUSE invariant with a PROVEN payoff.
- **The data-driven interpreter over codegen** — you cannot recompile the binary per tenant.
- **The frozen `QueryAst`** for guards — *not* CEL/JSONLogic, *not* Jira-Groovy (the scripting footgun). One
  grammar, four compile targets (OLTP, Search, EventMatcher, Notif prefs).

**Floor → follow-on:** the default scheme-set ships v1; the scheme-assignment precedence algebra is the resolved
detail.

---

## 3. Flexible-field storage / query (TE-17 — the JQL performance trap)

**The dominant per-subsystem risk.** ADR-06 is explicit this is **NOT** solved by sharing the field/view
primitive — the physical storage + query execution is tracker-owned.

**Resolution (Candidate B, sketch 03):** **typed-core columns + JSONB property-bag tail + a derived indexable
projection (GIN + measured-hot generated indexes off the bus) + Search as the cold/ad-hoc/full-text escalation
valve + OLAP for analytics.** Issues **owns the AST→OLTP-store compiler + the cost-bounding** (three-tier
escalation; the `SetExpr` push-down lowered first; never an unbounded JSONB scan —
[02 §3](./02-internals-and-algorithms.md)); it **consumes** the frozen field-type enum / view-model / `QueryAst`
(contract 13.3). **Deliberately byte-aligned with Knowledge's `db_row`** — the four `myelin-query` shapes are
frozen byte-identical (Δ7).

**Prior art cited:**
- **EAV is a SQL antipattern** (Karwin, *SQL Antipatterns*, 2010) — rejected (N self-joins per N-field filter).
- **Column-per-field doesn't scale** (DDL-per-tenant; Knowledge 01 §1.2 rejects identically).
- **JSONB + GIN** (`jsonb_path_ops`) + **Bigtable's sparse column families** (Chang et al.) as the conceptual
  ancestor — the property-bag source of truth.
- **CQRS / read-model materialise-when-measured** (Young; Fowler; KN-3) — the projection feeder promotes a hot
  facet to a generated index only on the **measured** threshold (contract 6.3, OQ-C — default-to-beat `> 5%` of a
  collection's view executions), never speculatively.
- **The search-index projection** (Tantivy via `declare_indexable`, contract 6.3) — the pressure-release valve,
  now unblocked: Tier-3 compiles the board query to Search with the **same** OQ-E `Filter` conjoined (Δ15).

**Floor → follow-on:** PG-hybrid (typed core + JSONB + projection feeder) is the floor; **distributed-SQL** is the
named, *measured* follow-on, only if a single tenant's shard outgrows PG.

---

## 4. Human-readable monotonic keys at scale (TE-14 / the frozen REF-3 reconciliation)

**`ENG-1421` — a per-team prefix + monotonic counter at world scale, where users perceive gaps as bugs but
gaplessness is a distributed-contention hotspot.**

**Resolution (Candidate B, sketch 04):** **Hi/Lo batched allocation per prefix, gap-tolerant, monotonic,
never-reused**, with an adaptive block size (small for cold prefixes → tiny gaps; large for hot → low contention).
UUID internal PK; the **human key `<PROJECTKEY>-<seqno>` is the stored canonical `<id>` in the ArtifactRef** (the
frozen REF-3 reconciliation, Δ3, contract 5.1) — `#1421` is the render-time display projection, never stored.
Cell-local (a prefix lives in one cell — no cross-region coordination). Gaps documented as benign.

**Prior art cited:**
- **Hi/Lo** (Hibernate's HiLo allocator) — turns N creates into 1 counter write, dropping contention by N×;
  gap-tolerant by construction.
- **GitHub/GitLab/Jira's real behaviour** — all have gaps in practice; gaplessness is a *perception*, not a
  requirement.

**Floor → follow-on:** none needed — single-cell allocation is the whole requirement; multi-cell tenants home each
prefix in its team's cell (keys unique within a prefix; prefixes don't span cells).

---

## 5. Drag-to-reorder ranking (TE-19 / the frozen `order_key`)

**Stable fractional ranking for drag-to-prioritise backlogs, with the concurrent-reorder conflict story (humans
AND agents reordering).**

**Resolution (Candidate A, sketch 06):** the **frozen `order_key` LexoRank string + server-arbitrated CAS** as the
floor — **byte-identical** with Knowledge's `db_row` drag (contract 13.3, the drift-killer): base-62 `0-9A-Za-z`,
lexicographic compare, midpoint bisection, **2-char jitter**, **48-char rebalance trigger**, `created_at`+ULID
tiebreak. Agents reorder through the **same** permissioned `ToolDef` + the **same** CAS arbitration as humans — one
safe path ([02 §5](./02-internals-and-algorithms.md)).

**Prior art cited:**
- **LexoRank** (Atlassian's Jira ranking system) — the production reference; base-N string keys between
  neighbours; O(1) moves. The frozen encoding *is* the LexoRank scheme.
- **Fractional indexing + jitter** (Figma / Evan Wallace's notes) — pick a midpoint string strictly between
  neighbours; append jitter to reduce concurrent same-gap collisions (the frozen 2-char suffix).
- **The CAS floor** (KN-1 / EI-04 §2) — server-arbitrated conditional write; the loser re-bases (no silent
  clobber) — the doctrine ladder's first rung before a CRDT.

**Floor → follow-on:** the CAS floor ships v1; the **move-CRDT (RGA / Yjs-Yrs list / Fugue)** is the named
follow-on, promoted only on *measured* concurrent-reorder pain, reusing Knowledge's Yrs list type. Because the
`order_key` is already frozen byte-identical, the promotion swaps the conflict-resolution engine, not the data
model.

---

## 6. TE-7 typed relations + the stateful Trigger (frozen `QueryAst` condition)

**Resolution (sketch 05A / 08B):** Issues **owns `issue_relation` as the source of truth** (the frozen contract
5.5); writes the **forward** edge transactionally + emits **one** typed event; **Refs materialises both
directions** and fixes the inverse pairing (no dual-write drift). `parent` = tree (rank-monotonic);
`depends_on`/`blocks`/`relates` = DAG with cycle detection (visited-set + depth ceiling 16, contract 5.3) in the
walk. The **stateful Trigger** ("Remind me when unblocked") reads this table; Issues owns the armable-condition
catalogue ([03 §10](./03-events-contracts-and-glue.md)) where each condition is the **frozen `QueryAst`** over
projection state (the granted CR-5, Δ8); the bus `arm_trigger` primitive + the `myelin-flow` `stale_after` durable
timer + the one Notif inbox for `on_resolve` are consumed.

**Prior art cited:** the TE-7 hybrid (typed table = truth, Refs = projection); Refs §4.5's bounded cycle-safe
recursive-CTE walk (Celko; SQL:1999 `WITH RECURSIVE`).

**Floor → follow-on:** none for the relation table (frozen contract); the Trigger is complete (the
armable-condition catalogue is the resolved detail).

---

## 7. Rollup / forecast engine (TE-18)

**Roll up progress/estimate/dates from millions of leaf issues to epics to initiatives, fresh on every child
change, cycle-detecting, and forecast "will this land by Q3?"**

**Resolution (Candidate B + the C compromise, sketch 05B):** **event-driven, debounced, incremental rollup off the
bus**, storing a derived materialised aggregate per ancestor (rebuildable by replay, contract 2.6), with
**read-time floor for small subtrees, materialise-on-measured-large**. Cycle-safe (visited-set + depth ceiling).
`input_hash` no-op suppression (loop safety, AG-6). **Forecast = an agent-powered swappable strategy reading
OLAP** — floor linear, follow-on Monte-Carlo ([02 §6.1](./02-internals-and-algorithms.md)). **Cross-cell
ancestors** ride the frozen `CrossCellPointer` bridge with cell-local resolution (contract 12.6, OQ-I; the named
multi-cell floor).

**Prior art cited:**
- **Event-driven async off the write path** (Phase-2 §7 / ADR-11.5) — a leaf change never blocks on an ancestor
  walk; rejects the synchronous-rollup-in-the-write-path candidate.
- **CQRS / measured-promotion** (KN-3) — read-time for small subtrees, materialise only when measured large.
- **The recursive-CTE visited-set + depth ceiling** (contract 5.3) for cycle safety.
- **Monte-Carlo forecasting over throughput samples** as a swappable agent strategy (ADR-08) — keeps the heavy
  stochastic compute off OLTP (reads OLAP).

**Floor → follow-on:** read-time floor → materialised-on-measured-large; linear forecast → Monte-Carlo agent;
single-cell rollup → cross-cell over the `CrossCellPointer` bridge.

---

## 8. SLA business-calendar engine

**Resolution (Candidate A, sketch 07):** **build the SLA *logic*** (policy + business-calendar arithmetic +
`QueryAst`-driven pause/resume + escalation orchestration) **over the `myelin-flow` timer/signal/workflow
substrate.** Precompute the wall-clock `fire_at` over the calendar, arm the SC-11 timer; **cheap disarm/re-arm on
pause/resume** (contract 9.3, the granted CR-6); **don't build timers, don't poll, don't pollute the shared
wheel** ([02 §6.2](./02-internals-and-algorithms.md)). On breach, the **frozen escalation chain** (`page →
oncall_now → escalate-after-timer`, contract 7.5, the granted CR-13) runs as a durable workflow. Breach/met feed
OLAP.

**Prior art cited:**
- **The SC-11 minute-bucket partial-index timer wheel** (Varghese & Lauck *Timing Wheels*, 1987; `FOR UPDATE SKIP
  LOCKED`) — millions of far-future timers cost an indexed range read; rejects the O(active-SLAs)-per-tick poll.
- **Business-day libraries + iCalendar RRULE/`VTIMEZONE`** for DST/holiday/recurrence/timezone correctness.
- **The frozen `QueryAst`** for pause/resume conditions (one predicate language).

**Floor → follow-on:** the SLA logic is complete; long-`time_to_resolution` SLAs spanning many days of pauses get
history-compaction (the `myelin-flow` continue-as-new note) as the named follow-on.

---

## 9. Real-time sync (the frozen firehose resume-cursor protocol)

**Resolution (Candidate A, sketch 08A):** **optimistic UI + bus-driven cache invalidation over the shared firehose
using the frozen `subscribe/resume/scope` resume-cursor protocol** (contract 3.5, OQ-J — co-designed once with
Chat/KN, Δ14). Issue-body concurrency = single-author CAS (ADR-05); board concurrency = server-arbitrated CAS —
**no Issues CRDT** ([02 §7](./02-internals-and-algorithms.md)). Per-view scope bounding (`scope = board:<id>`,
never `*`) keeps per-viewer fanout bounded; reconnect `resume(stream, scope, last_seq)` loses zero ops;
`resync_required` falls back to a `*.snapshot` replay.

**Prior art cited:**
- **Linear's sync-engine shape** (optimistic local updates + live multi-user board) — the bar.
- **The frozen resume-cursor durable transport** (the doctrine's "build the durable resume-cursor transport
  FIRST," EI-04 §2.2) — reconnect replays from the cursor, loses zero ops; reused, not re-invented.
- **Optimistic update + honest rollback** (design-language §8b.6 / P2).

**Floor → follow-on:** optimistic+resume is the v1 floor; **offline/local-first** is the named follow-on, out of
v1 scope unless promoted.

---

## 10. Import fidelity from Jira/Linear (PR-8) — the frozen ADF map

**The adoption gate + the "leave Atlassian cloud cleanly" sovereignty credibility signal.**

**Resolution (Candidate A, sketch 09):** **two-pass, ID-remapped (persisted source↔Myelin map), idempotent +
resumable, dry-run + reconciliation-report-first**; source adapters (Jira/Linear/GitHub/CSV) normalise into **one
canonical interchange format that round-trips with the portability export**; import emits the normal `issue.*`
events (one indexing path; per-tenant capped); **the ADF→`myelin-content` conversion uses the FROZEN lossy-map**
(contract 13.2, X-2 — Knowledge owns it; Issues consumes it, Δ6), and **every lossy conversion is recorded in the
import report, never silently dropped** ([01 §8](./01-tech-and-data-model.md)).

**Prior art cited:**
- **Two-pass create-then-wire with a persisted ID map** (the cited migration best practice) — pass 1 creates all
  entities recording the source-ID↔Myelin-ID map; pass 2 wires links/parents against the map (avoids
  forward-reference problems); the map is the load-bearing artifact for idempotency/resume/rollback/round-trip.
- **The canonical interchange as the round-trip oracle** — `export→import→export` must round-trip; one importer
  core, N source adapters.
- **Per-tenant in-flight caps** (the per-surface shed budget, OQ-K) + the protected human lane — a giant import
  never starves another tenant's interactive traffic.

**The frozen ADF→`myelin-content` map (consumed, contract 13.2):** paragraph/heading/blockquote/codeBlock/rule/
lists/table/image map directly (lossless); taskList→task_list; panel→callout; mention→`mention(Principal)` **if
the principal resolves in-tenant** else a plain-text `@name` (lossy, named); inlineCard/blockCard→
`artifact_ref(ArtifactRef)` **if the URL resolves to a Myelin artifact** else a link (lossy, named);
status/date/custom-emoji/layout columns/macros degrade per the named lossy rows. **Other mapping tables resolved**
(sketch-09 hand-forward): statuses → named states + the **fixed category** (unmapped → `unstarted` + flag); Jira
"Epic Link" / Linear Projects → `issue_relation` `parent`; issue links → the `issue_rel` set via a mapping table
(flagged where lossy); JQL filters → the frozen `QueryAst` (some JQL has no clean analogue → flagged); permission
schemes → ReBAC tuples (lossy, flagged for legal/user review).

**Floor → follow-on:** the canonical core + Jira/Linear/GitHub/CSV adapters ship v1; the permission-scheme mapping
is the lossy/legal-review leg (named, not silently dropped).

---

## 11. Free-text PII erasure — the ONE platform posture, by reference (X-7/OQ-G, [OPEN — LEGAL])

**The one honestly-unsolved residual.** PII someone *typed into another person's issue body/comment* cannot be
cleanly crypto-shredded the way structured PII can (it is encrypted under the *author's* DEK, in content others
own).

**Resolution (the ONE platform posture, instantiated by reference — Δ13).** Issues no longer states a separate
GD-6 residual; it instantiates the frozen platform erasure posture (contract 10.9, recon §X-7). The structural
floor — built now, no legal dependency — is: per-subject DEK crypto-shred for the subject's *own* free-text and
attachments + pseudonym-map shred for identity (the frozen `<pseudonym>@<tenant>.noreply` grammar, contract 4.8) +
the `restrict` suppression (never indexed / never agent-readable / never in analytics for a restricted subject).
The residual — third-party free-text mentions authored by others — is handled under the **documented lawful-basis
limit**: best-effort `rectify`/tombstone of the specific span where the subject identifies it, plus the standing
structural guarantee above. `[OPEN — LEGAL]`: the DPO/counsel ratify the residual basis in **one statement, not
five** ([03 §7](./03-events-contracts-and-glue.md)).

**Prior art cited:** crypto-shred + references-not-payloads + pseudonym indirection (Kleppmann *DDIA* ch.5; EI-04
§1; the GD-4 per-subject-DEK rule).

**Floor → follow-on:** the structural floor ships; the residual basis is `[OPEN — LEGAL]`. Worklog/productivity/
estimate-field sensitivity (works-council/labour-law, OQ-H) is the sibling open legal classification — the fields
carry the frozen `behavioural`/`restricted-by-default` tags now ([01 §6.1](./01-tech-and-data-model.md)).

Continue to [`06-reconciliation-compliance.md`](./06-reconciliation-compliance.md).
