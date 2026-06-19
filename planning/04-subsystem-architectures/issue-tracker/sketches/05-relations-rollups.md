# Sketch 05 — TE-7 typed relations + the rollup/forecast engine (TE-18)

> Exploration note. Weighs two coupled items: (a) the TE-7 typed-relation table I OWN (`issue_relation` =
> source of truth — REF-1/ISS-1; the Phase-3 contract is *frozen*, this sketch is about the *owned details*),
> and (b) Phase-2 §11 Q8 / deep-dive §6.3: the incremental, debounced, cycle-detecting **rollup/forecast
> engine** (TE-18). Leans; commit in `00-findings.md`.

## Part A — The typed relation table (TE-7; mostly settled by Phase 3, details owned here)

Phase 3 **froze** the hybrid (reference-graph §3.3, REF-1): the typed relation table in Issues is the **source
of truth** for lifecycle edges; Refs holds a rebuildable projection; the same transaction that writes a typed
row emits `issue.relation.created` which the Refs edge-builder consumes. The `rel` vocabulary is fixed by Refs:
`blocks / blocked_by / closes / depends_on / parent / relates` (+ `assigns`). What I **own and must detail**:

```sql
-- SOURCE OF TRUTH for issue lifecycle edges (the reference-graph §3.3 illustrative table, now mine).
CREATE TABLE issue_relation (
  tenant uuid, region text, relation_id uuid,
  src_issue uuid NOT NULL,        -- internal id; FK referential integrity Refs cannot give
  dst_ref   text NOT NULL,        -- ArtifactRef of the other end (may be cross-subsystem: a PR, a doc)
  rel issue_rel NOT NULL,         -- blocks|blocked_by|closes|depends_on|parent|relates
  created_by uuid, created_at timestamptz,
  PRIMARY KEY (tenant, relation_id),
  UNIQUE (tenant, src_issue, dst_ref, rel),
  FOREIGN KEY (tenant, src_issue) REFERENCES issue(tenant, id)
);
```

**Owned decisions:**
- **Inverse pairing is maintained by Issues, not just projected.** `blocks(A,B)` implies `blocked_by(B,A)`. We
  write the **forward** edge transactionally and emit *one* typed event; **Refs materialises both directions**
  in its projection (reference-graph §3.3 "a single typed event yields both directions"). Inside Issues, a
  *query* for "what blocks me" reads the projection direction we need; we do **not** store both rows (avoids
  dual-write drift). The transition guard "can't close while `blocked_by` an open issue" reads the typed table
  for the forward `blocks` edges pointing at this issue (indexed on `dst_ref`).
- **`parent` is a tree** (single parent per issue; sketch 01 rank-monotonic). `depends_on`/`relates`/`blocks`
  form a **DAG** (multi-edge) → these are where cycles can appear (A blocks B blocks A) → cycle detection lives
  in the rollup/traversal path (Part B), surfaced as a *diagnostic*, never an infinite loop (reference-graph
  §4.5 depth-ceiling + visited-set).
- **Cross-subsystem ends** (`closes` a PR, `relates` a doc) put an ArtifactRef in `dst_ref` — the FK only
  constrains the `src_issue` end; the far end's integrity is the projection's concern (eventually consistent,
  EI-04 §2 best-effort bidirectional).
- **The stateful Trigger ("unblock me when…")** reads this table: arming a Trigger on "ENG-1421 becomes
  unblocked" watches for the `blocked_by` edges to all resolve; its `stale_after` is a `myelin-flow` durable
  timer (event-bus §3.6; durable-workflow §4.2). Detailed in sketch 08 + user-flows.

## Part B — The rollup/forecast engine (TE-18) — the hard part

Roll up progress/estimate/dates from millions of leaf issues to epics to initiatives, keep it fresh on every
child change, detect cycles, and (the PM differentiator) **forecast** "will this land by Q3?" Naive synchronous
recompute on every child write is O(bad) (deep-dive §6.3).

### Candidate A — Synchronous rollup in the write path
Every `issue.updated` recomputes all ancestors inline before the write returns.
- **Against:** a leaf change in a deep tree blocks the writer on an ancestor walk; a hot initiative with
  thousands of descendants serialises on rollup; agents mutating many issues amplify it. Rejected (deep-dive
  §6.3 names this as the thing to avoid; Phase-2 §7 / ADR-11.5 mandate event-driven async).

### Candidate B — Event-driven, debounced, incremental rollup off the bus (the Phase-2 direction)
A child change emits `issue.updated` (with field deltas); a **rollup consumer** (the substrate consumer
template) recomputes affected ancestors **asynchronously**, **debounced** (coalesce a burst of child changes
into one ancestor recompute), **incrementally** (recompute only the deltas that changed: if only `estimate`
changed, only re-sum estimates; if `state_category` changed, only re-count done/total).

- **For:** the write path is just "emit the event" (Phase-2 §7; ADR-11.5). Rollup is off the hot path, debounced
  so a 10,000-issue import triggers a bounded number of ancestor recomputes, not 10,000.
- **For:** incremental = cheap. The rollup stores a **materialised aggregate per ancestor** (`rollup` row:
  done_count, total_count, sum_estimate, earliest_start, latest_due, computed_at, input_hash) updated by deltas.
  This is the "local materialised tree" TE-7/§6.3 anticipated rollups might force — and it's fine because the
  *edge truth* is still `issue_relation`; the rollup row is a **derived aggregate**, rebuildable by
  reindex-from-source (replay).
- **For (cycle safety):** the ancestor walk is the reference-graph recursive-CTE pattern with the visited-set
  guard + depth ceiling (reference-graph §4.5). A dependency cycle is detected and surfaced as a roadmap
  diagnostic ("⚠ dependency cycle: A→B→A"), not a hang.
- **Cost:** debounce windows + the "which ancestors are affected" fan-out need care (a leaf under an initiative
  spanning 50 teams). Bounded by: debounce coalescing + per-tenant in-flight caps (X-3) + the rollup aggregate
  storing `input_hash` so a recompute that produces no change emits no `issue.rollup_recomputed` event (stops
  rollup-event storms / loop amplification — AG-6).

### Candidate C — Pure read-time rollup (compute on roadmap render, never store)
KN-3's "compute at read time, never store" applied to rollups.
- **For:** no rollup consumer, no materialised aggregate, no staleness.
- **Against:** a portfolio/roadmap render then walks the *entire* subtree of every initiative live — fine for a
  small org, O(bad) for a 100k-issue portfolio (the exact deep-dive §6.3 scale case). And the **forecast/health
  agent** (deep-dive §7.3) needs the aggregate to *watch* for drift — it can't subscribe to "read-time."
- **Compromise:** read-time for *small* subtrees (cheap, always-fresh); **materialise (Candidate B) only when a
  subtree is measured large** (KN-3 measured-promotion). This is the floor→follow-on framing: ship read-time
  rollup as the floor, promote hot/large subtrees to incremental-materialised on measured slowness.

### Forecasting (the PM differentiator — deep-dive §4.2)
"Will this initiative land by Q3?" from velocity + remaining scope, Monte-Carlo-style.
- **Lean:** forecasting is **not** in the hot rollup path — it is an **agent-powered, on-demand / scheduled**
  computation (deep-dive §4.2/§7.3). The rollup engine provides the *inputs* (remaining estimate, historical
  throughput from the OLAP store); a forecast agent (mock now, real later — strategy pattern, ADR-08) runs a
  Monte-Carlo over throughput samples and writes a `forecast` field + emits `initiative.health_changed` when it
  crosses an at-risk threshold → a trigger flags the PM in chat (deep-dive §6.4). This keeps the heavy stochastic
  compute off OLTP (it reads OLAP) and makes forecasting a *swappable strategy*, not baked math. Floor: a simple
  deterministic linear forecast (remaining ÷ velocity); follow-on: Monte-Carlo agent.

## Leaning

- **Part A:** own `issue_relation` as specified; maintain forward edges transactionally, emit one typed event,
  let Refs materialise both directions; `parent`=tree, `depends_on/blocks/relates`=DAG with cycle detection in
  the walk. The Trigger reads this table; `stale_after` rides `myelin-flow`.
- **Part B:** **event-driven, debounced, incremental rollup off the bus, storing a derived materialised
  aggregate per ancestor (rebuildable by replay)** — Candidate B — with the **read-time floor for small
  subtrees, materialise-on-measured-large** promotion (Candidate C compromise). Forecasting is an
  **agent-powered swappable strategy** reading OLAP, floor = linear, follow-on = Monte-Carlo.

## Hands forward

- The rollup aggregate schema + the debounce-window policy + the affected-ancestor fan-out algorithm —
  architecture.
- The `input_hash` no-op-suppression detail (loop safety, AG-6) — architecture.
- Forecast agent ToolDef + the at-risk threshold config — architecture (ties to the agent fabric).
- PROVE-IT: rollup-freshness-under-import-storm drill + reindex-from-source rollup parity drill (findings §drills).
