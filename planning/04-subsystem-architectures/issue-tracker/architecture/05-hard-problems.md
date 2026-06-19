# Issue Tracker — 05 · Hard Problems Resolved (with cited prior art + named floors)

> See [`00-overview.md`](./00-overview.md) for the framing. This doc consolidates the **resolution of every
> subsystem-specific hard problem** named in the Phase-4 charge, each with **cited prior art** and a **named
> floor** where v1 is partial. The detailed mechanics live in [`01`](./01-tech-and-data-model.md)/[`02`](./02-internals-and-algorithms.md)/[`03`](./03-events-contracts-and-glue.md);
> this doc is the decision register. The Stage-1 exploration (candidates weighed) is in [`../sketches/`](../sketches/).

---

## 1. Issue-model duality — board↔roadmap as co-equal views; Epic/Initiative type-vs-level (PR-2)

**The make-or-break UX bet.** Get it wrong → PMs get a parallel reality, re-creating the "engineering tool vs
management tool" split the platform exists to kill (design-language §2).

**Resolution (Candidate C, sketch 01):** **one `issue` table = the ranked-type containment spine**
(sub-task → story/bug/task/chore/spike → **epic → initiative**, all `issue` rows). `parent` is the single
containment edge in `issue_relation` (the TE-7 source of truth). **The board and the roadmap are co-equal
`myelin-query` AST views over that one table** — board = `type_rank ≤ 1` grouped by `state_category`; roadmap =
`type_rank ≥ 2` on a date axis ([04 §1](./04-views-cli-and-api.md)). The three axes stay separate
([00 §1.1](./00-overview.md)): **cycle/sprint = a separate time-axis object** (membership edge, not
containment); **project/space = the identity scope object** (Id §5), never re-invented. Rank is config
(`type_scheme`) with **rank-monotonic parenting** as the default guardrail. `type_rank` is denormalised onto the
row so both views are index-range scans.

**Why this is structural, not a feature:** the roadmap **cannot drift** from the board because they read the
*same rows*. Co-equality is a property of the schema, not an integration we maintain.

**Prior art cited:**
- The **frozen ArtifactRef grammar pre-decided it** — Bus §6.2 lists `epic` as an `issue`-family *type*, ruling
  out the Linear-style "Projects/Initiatives are separate tables" model for the spine.
- **Jira's Epic-as-type** (an issue with `issuetype=Epic`) — adopted for the spine, but with the fix that **rank
  is a clean ordering and `parent` is the one containment edge** (no parallel "Epic Link" — the cited Jira UX
  smell, deep-dive §3.6).
- **Linear's first-class Cycles/Projects** — adopted for the *genuinely-different* axis-objects (a cycle has no
  workflow state; modelling it as an issue is the awkward-nulls problem).

**Floor → follow-on:** tree `parent` is v1; **constrained-DAG portfolios** (cross-team initiatives that need
multi-parent) are opt-in per `type_scheme`, the named follow-on. Rollup cycle-detection already handles the DAG
lateral edges.

---

## 2. Governance — baked-in vs opt-in schemes (PR-3)

**The "Linear-fast by default, Jira-powerful on demand, no fork, no migration" decision.**

**Resolution (Candidate C, sketch 02):** **typed-core columns (hot path) + layered optional schemes interpreted
(data-driven), not baked.** The **fixed state-*category* set** (`unstarted/started/completed/cancelled`) over
unlimited named states is the **one mandatory invariant** (cross-project reporting/boards/burndown read the
category, never the name). Workflow/field/permission/SLA/type schemes are assigned per (type × team/project);
**assigning a scheme is config, never a data migration** ([01 §3](./01-tech-and-data-model.md)). The precedence
algebra is deterministic + cached ([02 §1](./02-internals-and-algorithms.md)). Guards are **safe query-AST
predicates** ([02 §2](./02-internals-and-algorithms.md)), not scripting. Linear-simple = empty config;
Jira-powerful = more schemes; **one product, one code path, no fork.**

**Prior art cited:**
- **Jira's history of *lacking* a fixed category** is the cited failure (heterogeneous custom workflows that
  couldn't roll up — deep-dive §3.3). The fixed category is the HOUSE invariant with a PROVEN payoff.
- **The data-driven interpreter over codegen** (Phase-2 §3) — you cannot recompile the binary per tenant; schemes
  are user-authored config the one interpreter runs.
- **The safe-AST `EventMatcher`** (Bus §4.5 / ADR-07) for guards — *not* CEL/JSONLogic, *not* Jira-Groovy
  (the scripting footgun). One predicate language across guards, views, automations, triggers, SLA conditions.

**Floor → follow-on:** the default scheme-set ships v1; the scheme-assignment precedence algebra is the
resolved detail ([02 §1](./02-internals-and-algorithms.md)).

---

## 3. Flexible-field storage / query (TE-17 — the JQL performance trap)

**The dominant per-subsystem risk.** Arbitrary user-defined fields + a flexible query language = "the JQL
performance trap." ADR-06 is explicit this is **NOT** solved by sharing the field/view primitive — the physical
storage + query execution is tracker-owned.

**Resolution (Candidate B, sketch 03):** **typed-core columns + JSONB property-bag tail + a derived indexable
projection (GIN + measured-hot generated indexes off the bus) + Search as the cold/ad-hoc/full-text escalation
valve + OLAP for analytics.** Issues **owns the AST→OLTP-store compiler + the cost-bounding** (three-tier
escalation; never an unbounded JSONB scan — [02 §3](./02-internals-and-algorithms.md)); it **consumes** the
AST grammar / field types / view model (ADR-06). **Deliberately byte-aligned with Knowledge's `db_row`** so the
two co-owners of ADR-06 share the storage discipline.

**Prior art cited:**
- **EAV is a SQL antipattern** (Karwin, *SQL Antipatterns*, 2010) — rejected (N self-joins per N-field filter;
  what early Jira did and why JQL got slow).
- **Column-per-field doesn't scale** (DDL-per-tenant — deep-dive §3.4/§6.7; Knowledge 01 §1.2 rejects identically).
- **JSONB + GIN** (`jsonb_path_ops`) + **Bigtable's sparse column families** (Chang et al.) as the conceptual
  ancestor — the property-bag source of truth.
- **CQRS / read-model materialise-when-measured** (Young; Fowler; KN-3) — the projection feeder promotes a hot
  facet to a generated index only on measured frequency, never speculatively.
- **The search-index projection** (Tantivy via `declare_indexable`, Search §5.3) — the pressure-release valve.

**Floor → follow-on:** PG-hybrid (typed core + JSONB + projection feeder) is the floor; **distributed-SQL**
(CockroachDB/Yugabyte) is the named, *measured* follow-on, only if a single tenant's shard outgrows PG.

---

## 4. Human-readable monotonic keys at scale (TE-14)

**`ENG-1421` — a per-team prefix + monotonic counter at world scale, where users perceive gaps as bugs but
gaplessness is a distributed-contention hotspot.**

**Resolution (Candidate B, sketch 04):** **Hi/Lo batched allocation per prefix, gap-tolerant, monotonic,
never-reused**, with an adaptive block size (small for cold prefixes → tiny gaps; large for hot → low
contention). UUID internal PK; the human key is the public id in the ArtifactRef + CLI, allocated once at create
([01 §7](./01-tech-and-data-model.md), [02 §4](./02-internals-and-algorithms.md)). Cell-local (a prefix lives
in one cell — no cross-region coordination). Gaps documented as benign.

**Prior art cited:**
- **Hi/Lo** (Hibernate's HiLo allocator; the standard batched-sequence pattern) — turns N creates into 1 counter
  write, dropping contention by N×; gap-tolerant by construction.
- **GitHub/GitLab/Jira's real behaviour** — all have gaps in practice (deleted/failed creates); gaplessness is a
  *perception*, not a requirement, and its single-writer cost is the hotspot we refuse to pay (deep-dive §6.2).

**Floor → follow-on:** none needed — single-cell allocation is the whole requirement; multi-cell tenants home
each prefix in its team's cell (keys unique within a prefix; prefixes don't span cells).

---

## 5. Drag-to-reorder ranking (TE-19)

**Stable fractional ranking for drag-to-prioritise backlogs, with the concurrent-reorder conflict story (humans
AND agents reordering).**

**Resolution (Candidate A, sketch 06):** **LexoRank/fractional `rank` string + server-arbitrated CAS + jittered
inserts + region-local background rebalance**, as the floor — aligned with Knowledge's `order_key` family
(primitive parity). Agents reorder through the **same** permissioned `ToolDef` + the **same** CAS arbitration as
humans — one safe path ([02 §5](./02-internals-and-algorithms.md)).

**Prior art cited:**
- **LexoRank** (Atlassian's Jira ranking system) — the production reference for issue-tracker ranking; base-N
  string keys between neighbours; O(1) moves.
- **Fractional indexing + jitter** (Figma / Evan Wallace's notes) — pick a midpoint string strictly between
  neighbours; append jitter to reduce concurrent same-gap collisions.
- **The CAS floor** (KN-1 / EI-04 §2) — server-arbitrated conditional write; the loser re-bases against fresh
  state (no silent clobber) — the doctrine ladder's first rung before a CRDT.

**Floor → follow-on:** the CAS floor ships v1; the **move-CRDT (RGA / Yjs-Yrs list / Fugue to fix
interleaving)** is the named follow-on, promoted only on *measured* concurrent-reorder pain (R-5), reusing
Knowledge's Yrs list type rather than building our own.

---

## 6. TE-7 typed relations + the stateful Trigger (ISS-1)

**Resolution (sketch 05A / 08B):** Issues **owns `issue_relation` as the source of truth** (the frozen Refs
§3.3 contract); writes the **forward** edge transactionally + emits **one** typed event; **Refs materialises
both directions** (no dual-write drift). `parent` = tree (rank-monotonic); `depends_on`/`blocks`/`relates` = DAG
with cycle detection (visited-set + depth ceiling 16) in the walk ([01 §4](./01-tech-and-data-model.md)). The
**stateful Trigger** ("Remind me when unblocked") reads this table; Issues owns the armable-condition catalogue
([03 §10](./03-events-contracts-and-glue.md)); the bus `arm_trigger` primitive + the `myelin-flow` `stale_after`
durable timer + the one Notif inbox for `on_resolve` are consumed.

**Prior art cited:** the TE-7 hybrid (typed table = truth, Refs = projection — the decision-record resolution);
Refs §4.5's bounded cycle-safe recursive-CTE walk (Celko; SQL:1999 `WITH RECURSIVE`).

**Floor → follow-on:** none for the relation table (frozen contract); the Trigger is complete (the
armable-condition catalogue is the resolved detail).

---

## 7. Rollup / forecast engine (TE-18)

**Roll up progress/estimate/dates from millions of leaf issues to epics to initiatives, fresh on every child
change, cycle-detecting, and forecast "will this land by Q3?"**

**Resolution (Candidate B + the C compromise, sketch 05B):** **event-driven, debounced, incremental rollup off
the bus**, storing a derived materialised aggregate per ancestor (rebuildable by replay), with **read-time floor
for small subtrees, materialise-on-measured-large**. Cycle-safe (visited-set + depth ceiling). `input_hash`
no-op suppression (loop safety, AG-6). **Forecast = an agent-powered swappable strategy reading OLAP** — floor
linear, follow-on Monte-Carlo ([02 §6.1](./02-internals-and-algorithms.md)).

**Prior art cited:**
- **Event-driven async off the write path** (Phase-2 §7 / ADR-11.5) — a leaf change never blocks on an ancestor
  walk; rejects the synchronous-rollup-in-the-write-path Candidate A (deep-dive §6.3).
- **CQRS / measured-promotion** (KN-3) — read-time for small subtrees, materialise only when measured large.
- **The recursive-CTE visited-set + depth ceiling** (Refs §4.5) for cycle safety.
- **Monte-Carlo forecasting over throughput samples** (the PM differentiator, deep-dive §4.2) as a swappable
  agent strategy (ADR-08) — keeps the heavy stochastic compute off OLTP (reads OLAP).

**Floor → follow-on:** read-time floor → materialised-on-measured-large; linear forecast → Monte-Carlo agent.

---

## 8. SLA business-calendar engine

**Resolution (Candidate A, sketch 07):** **build the SLA *logic*** (policy + business-calendar arithmetic +
AST-driven pause/resume + escalation orchestration) **over the `myelin-flow` timer/signal/workflow substrate.**
Precompute the wall-clock `fire_at` over the calendar, arm the SC-11 timer; re-arm on pause/resume; **don't
build timers (consume SC-11), don't poll, don't pollute the shared wheel** ([02 §6.2](./02-internals-and-algorithms.md)).
Breach/met feed OLAP for compliance reporting.

**Prior art cited:**
- **The SC-11 minute-bucket partial-index timer wheel** (Varghese & Lauck *Timing Wheels*, 1987; `FOR UPDATE
  SKIP LOCKED`) — millions of far-future timers cost an indexed range read; rejects the O(active-SLAs)-per-tick
  poll (Candidate B).
- **Business-day libraries + iCalendar RRULE/`VTIMEZONE`** for DST/holiday/recurrence/timezone correctness.
- **The safe-AST `EventMatcher`** for pause/resume conditions (one predicate language).

**Floor → follow-on:** the SLA logic is complete; long-`time_to_resolution` SLAs spanning many days of pauses
get history-compaction (the `myelin-flow` continue-as-new note) as the named follow-on for very-long SLAs.

---

## 9. Real-time sync

**Resolution (Candidate A, sketch 08A):** **optimistic UI + bus-driven cache invalidation over the shared
firehose with a resume-cursor on reconnect (reuse KN-1's substrate).** Issue-body concurrency = single-author
CAS (ADR-05); board concurrency = server-arbitrated CAS — **no Issues CRDT** ([02 §7](./02-internals-and-algorithms.md)).
Per-view subscription scope bounding keeps per-viewer fanout bounded.

**Prior art cited:**
- **Linear's sync-engine shape** (optimistic local updates + live multi-user board, deep-dive §5.5/§6.6) — the
  bar.
- **KN-1's resume-cursor durable transport** — reconnect replays from the cursor, loses zero ops; reused rather
  than re-invented.
- **Optimistic update + honest rollback** (design-language §8b.6 / P2).

**Floor → follow-on:** optimistic+resume is the v1 floor; **offline/local-first** is the named follow-on, out
of v1 scope unless promoted.

---

## 10. Import fidelity from Jira/Linear (PR-8)

**The adoption gate + the "leave Atlassian cloud cleanly" sovereignty credibility signal.** A correctness/
migration-engineering problem, not a throughput one (deep-dive §10.4).

**Resolution (Candidate A, sketch 09):** **two-pass, ID-remapped (persisted source↔Myelin map), idempotent +
resumable, dry-run + reconciliation-report-first**; source adapters (Jira/Linear/GitHub/CSV) normalise into
**one canonical interchange format that round-trips with the portability export**; import emits the normal
`issue.*` events (one indexing path; per-tenant capped via X-3); **lossy mappings named explicitly in the
reconciliation report, never silently dropped** ([01 §8](./01-tech-and-data-model.md); [04 §2](./04-views-cli-and-api.md)).

**Prior art cited:**
- **Two-pass create-then-wire with a persisted ID map** (the cited migration best practice, deep-dive §10.4) —
  pass 1 creates all entities recording the source-ID↔Myelin-ID map; pass 2 wires links/parents against the map
  (avoids forward-reference problems); the map is the load-bearing artifact for idempotency/resume/rollback/
  re-sync/round-trip.
- **The canonical interchange as the round-trip oracle** (deep-dive §10.4/§8.6) — `export→import→export` must
  round-trip; one importer core, N source adapters (abstract at the third source).
- **Per-tenant in-flight caps** (X-3) + the protected human lane (substrate §7) — a giant import never starves
  another tenant's interactive traffic.

**Mapping tables resolved** (sketch-09 hand-forward):
- Statuses → named states + the **fixed category** (unmapped → `unstarted` + flag).
- Jira "Epic Link" / Linear Projects → `issue_relation` `parent` (ranked types).
- Issue links → the `issue_rel` set via a **mapping table** (Jira link types ≠ ours; flagged where lossy).
- Jira ADF / wiki-markup → `myelin-content` (co-designed with Knowledge, who owns the content taxonomy) — the
  messiest; **lossy nodes flagged** in the report.
- JQL filters → the shared query AST (some JQL has no clean AST analogue → flagged).
- Permission schemes → ReBAC tuples — **lossy, flagged for legal/user review** (deep-dive §10.3).

**Floor → follow-on:** the canonical core + Jira/Linear/GitHub/CSV adapters ship v1; the permission-scheme
mapping is the lossy/legal-review leg (named, not silently dropped).

---

## 11. Free-text PII erasure (the GDPR residual — GD-6, [OPEN — LEGAL])

**The one honestly-unsolved residual.** PII someone *typed into another person's issue body/comment* cannot be
cleanly crypto-shredded the way structured PII can (it is in content others own).

**Resolution (floor + documented residual):** anonymise-actor (pseudonym-map shred — Id `erase`) +
redaction-tombstone + agent-assisted free-text scan + crypto-shred of the per-subject DEK for the erasing
person's *own* free-text and attachments. The residual — third-party free-text mentions of the subject — is
**documented honestly as a residual risk**, not claimed solved ([03 §7](./03-events-contracts-and-glue.md)).

**Prior art cited:** crypto-shred + references-not-payloads + pseudonym indirection (Kleppmann *DDIA* ch.5; EI-04
§1; the GD-4 per-subject-DEK rule). The git-history-erasure GD-1 reconciliation is the sibling honest-residual
precedent.

**Floor → follow-on:** the floor ships; the residual is [OPEN — LEGAL] (GD-6) — architecture + legal review
(CR-7 in [06](./06-shared-system-change-requests.md)). Worklog/productivity-field sensitivity (works-council/
labour-law, GD-13) is a sibling open legal classification.

Continue to [`06-shared-system-change-requests.md`](./06-shared-system-change-requests.md).
