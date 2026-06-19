# Issue Tracker — 02 · Internals & Algorithms

> See [`01-tech-and-data-model.md`](./01-tech-and-data-model.md) for the schema. This doc details the
> subsystem-specific algorithms — now built to the **frozen** shared shapes: the scheme-resolution precedence
> algebra, the workflow interpreter (frozen `QueryAst` guards), the AST→store query compiler that lowers the
> frozen `list_objects` `SetExpr` push-down + cost-bounding + the projection feeder, Hi/Lo key allocation, the
> frozen `order_key` LexoRank ranking + CAS, the rollup engine, the business-calendar SLA arithmetic, and
> real-time sync over the frozen firehose resume-cursor protocol. The hard-problem *framing* (candidates, prior
> art) is in [`05-hard-problems.md`](./05-hard-problems.md), which this doc implements.

---

## 1. Scheme resolution — the precedence algebra

A write to issue *X* of type *T* in project *P* (owned by team *M*) must deterministically resolve **which
workflow/field/permission/SLA scheme applies**. The algebra is a most-specific-wins lattice over the three
assignment axes (`type_id`, `project_id`, `team_id`), each nullable (NULL = "any"):

```
resolve(kind, T, P, M) =
  first non-empty of, in this fixed order:
    (T, P, M)  →  (T, P, ·)  →  (T, ·, M)  →  (T, ·, ·)
                  (·, P, M)  →  (·, P, ·)  →  (·, ·, M)  →  org_default(kind)
  // type-specificity dominates project, project dominates team, team dominates org.
```

- **Determinism:** the order is total — there is never a tie.
- **Cached:** the resolution for a given `(kind, T, P, M)` is computed once and cached per-cell (invalidated on a
  `scheme_assignment` change via the bus). It is **off the per-write hot path** — the write loads the
  *already-resolved* compiled scheme.
- **No-config = Linear-simple:** an org with zero assignments resolves to `org_default` for every kind — the
  typed core + the one 3-state default workflow + no field/permission/SLA overlays. Adding governance is adding
  assignments, never migrating data.

The validation UX (S13) surfaces the resolved scheme + a "this transition is governed by scheme *S* assigned at
*(T,P)*" explainer, so an admin can see *why* a scheme applies.

---

## 2. The workflow interpreter — data-driven FSM, frozen-`QueryAst` guards (sketch 02)

A transition request `transition(issue, target_state, actor)` runs the interpreter against the resolved workflow
scheme. **Not codegen** (you cannot recompile the binary per tenant) and **not user-scripting** (no Jira-Groovy
footgun) — a single interpreter over a config FSM, guards as the **frozen `myelin-query` `QueryAst`** (= the
`EventMatcher` core, contract 3.4).

```
fn transition(issue, target, actor):
    wf      = resolve('workflow', issue.type_id, issue.project_id, team_of(actor))   // §1, cached
    t       = wf.transitions.find(from=issue.state, to=target)        ?? Err(NoSuchTransition)
    // ReBAC + the frozen transition CaveatContext (off the hot list_objects path, contract 4.2):
    Id.check(actor, 'perform_transition', issue.ref(), zookie,
             CaveatContext{ object: issue.ref(), transition: t.id, attrs: issue_attrs })  ?? Err(Denied)
    for g in t.guards:                                                                  // frozen QueryAst predicates
        eval_guard(g, ctx{issue, linked_refs, actor})                ?? Err(GuardFailed{reason})  // pre-assembled reason
    for f in wf.required_fields_on(target):
        present(issue, f)                                            ?? Err(MissingRequired{f})
    BEGIN TX
        issue.state          = target
        issue.state_category = t.to_category                          // the FIXED category (the invariant)
        issue.state_changed_at = now
        append issue_change_log(actor, {state: from→to})
        for a in t.post_actions: stage(a)                             // assign / set-field / link / arm-trigger
        OutboxTx::emit(issue.transitioned{from, to, category}, cause)  // the ONLY emit path
    COMMIT
    // post-COMMIT, off the bus: rollup, SLA pause/resume eval, Search/Refs index, Notif
```

**Guard examples** (each a frozen `QueryAst` predicate — bounded, no UDF/loop/recursion):
- `guard: linked PR CI is green` → reads the linked `closes`/`relates` PR's **frozen `CheckStatus`** (contract
  5.9) via `project(ref, viewer)` (no cross-DB; CI owns the status, Git owns the projection). The "can't mark Done
  while CI red" guard (flow C1) checks `state = success` **and** an acceptable trust posture (trusted, or
  fork-endorsed — Issues reads `trust_tier` off the fact, never recomputes it; Δ10).
- `guard: not blocked_by an open issue` → a `Has`/`Ref` `QueryAst` over `issue_relation` `blocks` edges pointing
  at this issue (the `issue_rel_dst` index), checking none has an open `state_category`.
- `guard: approver-role signed off` → reads a `field.approval` value gated by a HITL durable signal (flow B2,
  contract 9.4).

**Why frozen-`QueryAst`, not scripting:** the same predicate language powers saved views, CLI filters, automation
matchers, trigger conditions, and SLA pause conditions — **one grammar, four compile targets** (OLTP, Search,
EventMatcher, Notif prefs; contract 3.4). One validator, one cost model, one permission-aware-by-construction
guarantee. An admin authors a guard in the **guard builder** (S13), not a code editor; an unreachable-state /
missing-category-mapping is flagged inline before save.

**Agent parity:** an agent transitions through the **same** interpreter via `EffectApi::apply` (no carve-out). A
governed transition is HITL-gated per the frozen `requires_approval` defaults (contract 8.1, X-6): a
`transition(issue, →done)` on an SLA-bound issue with an approver edge defaults to `requires_approval = yes` — the
gated tool is **withheld** (returns `Gated`, does not mutate), the workflow emits `approval.requested`, and on
`signal(approval)` (contract 9.4) the step re-runs with the tool now allowed (flow B2; contract 8.2).

---

## 3. The query planner — AST→store compiler + `SetExpr` push-down + cost-bounding + the projection feeder (TE-17)

Issues **owns the compiler from the shared `myelin-query` AST to its OLTP store** and the **cost-bounding** (the
ADR-06 line: "share the schema language and the view model, not the query planner"). The planner's first job is
**lowering the frozen `list_objects` `SetExpr`** into a SQL predicate over `issue.id` — the no-leak,
no-N+1, no-post-filter pre-filter (contract 4.3, OQ-E; Δ1):

```
plan(ast, viewer, zookie?) =
  // 1. THE LEAK-FREE PRE-FILTER (always first):
  result = list_objects(viewer, 'view', 'issue', zookie?)            // → Ids{ids,zookie} | Filter{set_expr,zookie}
  acl_predicate = lower_set_expr(result, ColRef{ table:"issue", column:"id" }):
    Ids(ids) / NotIds(ids)          → WHERE issue.id IN (...) / NOT IN (...)       (inlined under the cardinality cap)
    InRelation{relation,via_column} → JOIN authz_visible av ON av.object_id = issue.id
                                          AND av.subject = $viewer AND av.relation = $relation   // the per-tenant authz reverse index
    TupleSet{index}                 → JOIN that server-materialised tuple set on issue.id
    All                             → (no restriction; admin)        None → WHERE false
    Union/Intersect/Difference      → AND/OR/EXCEPT of the above
  // 2. CLASSIFY each user predicate and pick the cheapest correct tier:
    ├─ typed-core field (state/category/priority/assignee/type/parent/cycle/project)  → TIER 1: indexed OLTP scan
    ├─ custom field that is a MEASURED-HOT facet (has a generated index)              → TIER 2: generated/expression index
    ├─ custom field, cold facet, small bounded result                                → TIER 2b: GIN probe (jsonb_path_ops)
    └─ full-text / cross-artifact / cold facet on a HUGE result / semantic           → TIER 3: escalate to Search
  cost = estimate(rows_scanned, tiers)
  if cost > budget:  push the heaviest leg to Search (the SAME acl Filter conjoined)  OR  return Refine{hint}
  every query: paginated + statement-timeout'd
```

**The `SetExpr` lowering is the resolution of the Phase-4 blocking CR-1** (Δ1). The `authz_visible` JOIN target
is the **per-tenant, residency-pinned authz reverse index** Identity maintains off the bus (the
SpiceDB/Zanzibar `LookupResources` reverse index realised as a co-located JOIN target). The returned `zookie`
bounds staleness; a security-sensitive scan passes the zookie so the read does not use the fail-static cache
(contract 4.10) — the JOIN reads the tuple index at-or-after the zookie's revision (read-your-writes for a
just-revoked grant). A confidential issue is **absent by construction** (the ReBAC `- confidential` set-difference
userset, §6 of doc 03) — never a post-filter, never an "N hidden" leak.

**The three-tier escalation** (sketch 03), with **Search as the pressure-release valve** (now unblocked — Δ15):
1. **Tier 1 (typed core):** `issue_board` / `issue_roadmap` / `issue_assignee` index ranges. The 90% hot path —
   Linear-fast.
2. **Tier 2 (custom hot facet):** the per-facet generated index, provisioned **off the bus** by the projection
   feeder. Until promoted, the GIN index (2b) serves it.
3. **Tier 3 (Search):** full-text, cross-artifact, semantic, or a cold facet on a huge result →
   `query(ast, viewer, zookie, page)` (contract 6.1), which **conjoins the same OQ-E `Filter`** before scoring
   (the `search-requires-acl-filter` lint) — ACL-pre-filtered during traversal, no leak, no N+1.

**The projection feeder** (the measured-promotion path): a bus consumer watches `issue.updated` deltas and a
per-(tenant, type, field_id) **filter/sort frequency counter**; when a custom facet crosses the **measured**
threshold (contract 6.3, OQ-C — the default-to-beat is a facet appearing in `> 5%` of a collection's view
executions over a rolling window; a Search-owned tunable, not a contract constant), the feeder provisions a
generated/expression index via a forward-only online migration (expand→backfill→contract; no blocking `ALTER` on
the flagged-hot `issue` table). Promotion is **measured, never predicted** (EI-02 §8). The cost model + the
OLTP↔Search escalation threshold are the D2 PROVE-IT drill.

**Which fields earn a typed column:** a field earns a **typed core column** iff it is *always present AND on the
hot board/report path* — `state`/`state_category`/`priority`/`assignee`/`reporter`/`type`/`parent`/`project`/
`cycle`/`rank`/timestamps. Everything else (governance custom fields like `severity`, `story_points`,
`customer_tier`) lives in the JSONB tail; the *measured-hot* subset gets a generated index (Tier 2).

---

## 4. Human-key allocation — Hi/Lo (sketch 04; TE-14)

```
fn allocate_key(prefix) -> String:
    if local_block_for(prefix).is_empty():
        (lo, hi) = UPDATE prefix_counter
                     SET high_water = high_water + block_size
                     WHERE (tenant, prefix) = …
                     RETURNING high_water - block_size, high_water     // ONE atomic reserve; gap-tolerant
        local_block_for(prefix) = (lo+1 ..= hi)
        maybe_grow_block_size(prefix)        // adaptive: raise block_size on a measured high create-rate
    n = local_block_for(prefix).next()       // handed out from memory, no DB contact
    return format!("{}-{}", prefix, n)       // the STORED CANONICAL <id> = <PROJECTKEY>-<seqno> (contract 5.1)
```

**Crash-safety:** the `UPDATE … RETURNING` is the single atomic reserve step. A worker that crashes after
reserving but before using a block loses that block — a **gap**, never a double-allocation, never a reuse
(monotonic). Reserve-then-use, leak-a-block-on-crash is the same at-least-once + idempotent shape as the rest of
the platform. The **adaptive block size**: start small (50) → tiny gaps for cold prefixes; grow (toward 1000) on
a measured high create-rate so a hot prefix (an incident storm, an import) drops contention by N× without
serialising on the counter row. The minted key is the stored `<id>` in the ArtifactRef (Δ3); `#1421` is
render-time.

---

## 5. Ranking — the frozen `order_key` LexoRank + server-arbitrated CAS (sketch 06; TE-19; contract 13.3)

The `rank text` column is the **frozen `order_key`** (Δ7) — **byte-identical** with a Knowledge `db_row` drag, so
a future shared CRDT/render path treats the field uniformly. The frozen encoding (contract 13.3, the drift-killer):

- **Alphabet / base:** base-62, ordered `0-9 A-Z a-z` (ASCII-ordinal, so byte comparison == rank order).
- **Encoding:** an `order_key` is a non-empty string over the alphabet; ranking is **lexicographic string
  comparison**. Between two keys `a < b`, a new key is the midpoint via digit-wise bisection; when no digit fits
  between, **append** a midpoint digit (the key grows by one char) rather than rebalancing.
- **Initial spacing:** first item `"U"` (mid of the alphabet); appended items step by a fixed gap; a bulk insert
  spreads evenly across the range.
- **Jitter:** a new key appends a **2-char random suffix** from the alphabet (the LexoRank bucket/jitter) so two
  clients independently inserting "at the same midpoint" produce **distinct** keys — no two concurrent drags
  collide on an identical key.
- **Rebalance:** when a key exceeds **48 chars** (measured pathology, not predicted), a background rebalance pass
  re-spaces the collection's keys; rebalance is a `myelin-flow` activity, idempotent, emitted via outbox so views
  resubscribe. It never reorders the *displayed* order.
- **Tiebreak:** when two `order_key`s compare equal (should not happen with jitter), the deterministic tiebreak is
  `created_at` then `id` (ULID) — total order guaranteed.

```
fn reorder(issue, before_rank, after_rank, expected_version) -> Result:
    new_rank = order_key::between(before_rank, after_rank)        // frozen midpoint bisection + 2-char jitter (contract 13.3)
    // SERVER-ARBITRATED CAS — the no-silent-clobber guarantee (humans AND agents, one path):
    n = UPDATE issue SET rank = new_rank, version = version+1
          WHERE id = issue AND version = expected_version
          RETURNING …
    if n == 0:  return Conflict{authoritative_order}              // the loser re-bases against fresh state (honest rollback)
    OutboxTx::emit(issue.updated{rank delta}, cause)
```

- **Concurrency:** two concurrent moves to the *same gap* → one wins the CAS, the loser is returned the
  authoritative order and **re-bases** — **no silent overwrite** (the CAS floor, KN-1 / EI-04 §2). The UI shows
  the honest "reordered by someone else — your change was re-applied below" (S3/S6).
- **Precision exhaustion:** handled by the frozen 48-char rebalance trigger above.
- **Agent parity:** an agent reorders via the **same** `issue.reorder` `ToolDef` through `EffectApi` — the same
  CAS arbitration. An agent that loses the CAS gets an ordinary `Denied`/stale result and re-plans. Server-
  arbitrated CAS (not client-trust) makes human and agent reorders **uniformly safe through one mechanism**.
- **Floor → follow-on:** the CAS floor ships v1; the **move-CRDT (Yrs list / Fugue)** is the named follow-on,
  promoted only on *measured* concurrent-reorder pain, reusing Knowledge's Yrs type. Because the `order_key` is
  already byte-identical, the promotion swaps the conflict-resolution engine, not the data model.

---

## 6. The rollup engine + business-calendar SLA arithmetic

### 6.1 Rollup — event-driven, debounced, incremental (sketch 05B; TE-18)

A child change emits `issue.updated` (with field deltas); the **rollup consumer** recomputes affected ancestors
**asynchronously, debounced, incrementally**:

```
on issue.updated(child, deltas):                          // off the bus, never in the write path (ADR-11.5)
    ancestors = walk_parent_edges(child, depth_ceiling=16, visited_set)   // issue_relation 'parent'; cycle-safe (contract 5.3)
    for a in ancestors:
        debounce(a, window):                              // coalesce a burst of child changes into ONE ancestor recompute
            recompute_incremental(a, deltas):             // only re-sum what changed (estimate? state_category? dates?)
                new = aggregate(children_of(a))
                if hash(new) == rollup[a].input_hash: SKIP // no-op suppression: no change → no event (stops loop storms, AG-6)
                else:
                    UPDATE rollup[a] = new
                    OutboxTx::emit(issue.rollup_recomputed{a, new})   // feeds roadmap + the forecast agent
```

- **Write path is just "emit the event"** — a leaf change never blocks on an ancestor walk; a 10,000-issue import
  triggers a *bounded* number of ancestor recomputes (debounce coalescing), not 10,000.
- **Incremental = cheap:** if only `estimate` changed, only re-sum estimates; if only `state_category`, only
  re-count done/total. The aggregate is the `rollup` row, a **derived** value — the edge truth stays in
  `issue_relation`, so the rollup is rebuildable by reindex-from-source (`replay`, contract 2.6).
- **Cycle-safe:** the ancestor walk uses the visited-set + depth ceiling 16 (matching contract 5.3). A dependency
  cycle is a roadmap diagnostic, never a hang.
- **`input_hash` no-op suppression:** a recompute producing the same input hash emits **no** event — stopping
  rollup-event storms and loop amplification (AG-6).
- **Floor → follow-on:** **read-time rollup for small subtrees** (cheap, always-fresh) is the floor;
  **materialise only when a subtree is measured large** (KN-3 measured-promotion). The debounce-window policy is
  per-tenant-tunable; the affected-ancestor fan-out is bounded by per-tenant in-flight caps (the per-surface shed
  budget floor, OQ-K) for the 50-team-initiative case.
- **Cross-cell ancestors** (an initiative whose children span cells, OQ-I): the walk over a remote child uses the
  frozen `CrossCellPointer{subject, type, correlation_id, home_cell}` (contract 12.6); resolution is **cell-local**
  (the home cell renders the child's progress projection per-viewer; only the projection crosses). Single-cell is
  the complete v1; cross-cell is the named floor.
- **Forecast** is **not** in the hot rollup path: the rollup provides the inputs (remaining estimate; historical
  throughput from OLAP); a **forecast agent** (swappable strategy, ADR-08) runs over throughput samples and writes
  the `forecast` field + emits `initiative.health_changed` on crossing an at-risk threshold → a trigger flags the
  PM (flow B4). Floor: linear `remaining ÷ velocity`; follow-on: Monte-Carlo agent.

### 6.2 Business-calendar SLA arithmetic (the genuinely-owned hard part)

The SLA *timers* are the SC-11 wheel (we consume, never rebuild). The owned algorithm is **converting a
business-time budget into a wall-clock `fire_at`** over a calendar, and **re-arming on pause/resume**:

```
fn business_fire_at(start: ts, budget_secs: i64, cal: Calendar) -> ts:
    cursor   = start; remaining = budget_secs
    loop:
        win = next_working_window(cursor, cal)            // DST-correct via IANA tz; skips nights/weekends/holidays
        avail = win.end - max(cursor, win.start)
        if remaining <= avail: return max(cursor, win.start) + remaining
        remaining -= avail; cursor = win.end              // advance to the next window
```

- **On SLA start:** compute `fire_at` (breach) and `at_risk_fire_at` (the 80% nudge); arm **two** `myelin-flow`
  timers. The wheel only ever holds concrete wall-clock `fire_at`s — it stays the dumb, calendar-agnostic SC-11
  substrate (we never pollute it with calendar logic).
- **On pause** (an `issue.updated` matching the policy's `pause_conditions` `QueryAst` — e.g.
  `state:waiting-on-customer`): disarm the timer (cheap, contract 9.3), store `remaining_business_secs`.
- **On resume:** recompute `fire_at = business_fire_at(now, remaining_business_secs, cal)`, re-arm. A handful of
  timer ops per SLA, not a hot loop — the **cheap disarm/re-arm of a precomputed `fire_at`** the frozen contract
  9.3 names.
- **Correctness corpus** (the D6 drill): DST transitions, multi-day spans, holiday boundaries, mid-window
  pause/resume — a deterministic, testable arithmetic corpus + a breach-fires-after-restart drill. Prior art:
  business-day libraries; iCalendar RRULE/`VTIMEZONE` for recurrence/timezone correctness; Varghese & Lauck
  *Timing Wheels* (1987) for the wheel.
- **Escalation** on `sla.at_risk`/`sla.breached`: emit a Signal → Notif routes it; a breach starts the **frozen
  escalation chain** (contract 7.5, Δ11) as a durable workflow: `page → oncall_now → escalate-after-timer`, all on
  the wheel. The `summary` and inbox strings are humanised via the ONE templating surface (contract 7.3). An SLA
  at 80% can wake a drafting agent ("draft a holding response," HITL-gated). Breach/met feed OLAP.
- **Floor:** very-long `time_to_resolution` SLAs spanning many days of pauses get **history-compaction** (the
  `myelin-flow` continue-as-new note) — the named follow-on.

---

## 7. Real-time sync — optimistic UI + the frozen firehose resume-cursor protocol (sketch 08A; OQ-J)

**Floor (v1):** optimistic local updates + bus-driven cache invalidation over the **shared firehose** using the
**frozen `subscribe/resume/scope` resume-cursor protocol** (contract 3.5, OQ-J — co-designed once for ISS boards,
KN hot docs, CHAT hot channels; Δ14):

```
client:
    subscribe(stream = fan.<tenant>.<project>, scope = board:<id>)     // scope BOUNDS what frames arrive; never *
    on local mutation: apply optimistically; send through the SAME permissioned API (UI=CLI=agent parity)
    on server confirm: keep; on server reject: roll back + one quiet line + the field to fix
    on Frame{seq, ...} for scope: patch the normalised cache (an agent-moved card animates in, labelled)
    on reconnect: resume(stream, scope, last_seq) → backfill (last_seq, now] then live   // loses ZERO ops (T-5)
    on resync_required (last_seq older than the retention window): full *.snapshot replay (contract 2.6)
```

- **Per-view scope bounding (the head-of-line + cost discipline):** a board subscribes to `scope = board:<id>`
  (the issues in *that* board's current filter); a 50k-row board **paginates its scope** (the visible window + a
  margin), so it does not stream 50k live frames to one client. The transport rejects an unbounded/over-broad
  scope (the whitelist-not-`*` rule generalised to the firehose, BUS-3). Presence/typing/cursor ride the
  ephemeral firehose, never the durable bus.
- **Resume cursor:** every frame carries a per-`(stream, scope)` monotonic `seq`; reconnect backfills
  `(last_seq, now]` from the bounded firehose retention window, then resumes live — **loses zero ops** (the
  pass condition for T-5). If `last_seq` is older than the window, `resync_required` falls back to a `*.snapshot`
  replay — the cold-rebuild path, named, not silent.
- **Backpressure:** per-connection in-flight frame caps; a slow consumer is dropped to `resync_required` rather
  than buffering unboundedly (the OQ-K per-surface shed budget; the collab op-stream / board profile).
- **Issue-body concurrency is single-author CAS** (ADR-05) — the `version` token; NOT the Knowledge CRDT. Board
  concurrency is server-arbitrated CAS (§5). **No Issues CRDT in v1.**
- **Floor → follow-on:** offline/local-first is a named follow-on, out of v1 scope unless promoted.

---

## 8. Scaling internals & hot-spots

| Hot-spot | Mechanism | Bound |
|---|---|---|
| **Board scan latency** | Tier-1 index range over `issue_board` + the `SetExpr` JOIN | the `(tenant, project, state_category, rank)` index keeps it an index range even at millions of issues; the keyboard <1s budget is the gate (D2) |
| **Custom-field filter** | feeder-promoted generated index (Tier 2) → GIN (2b) → Search (Tier 3) | cost-bounded; a query that would scan too much is pushed to Search (same `Filter` conjoined) or returns `Refine{hint}` — **never** an unbounded JSONB scan |
| **Rollup fan-out** | debounce coalescing + incremental recompute + `input_hash` suppression + per-tenant in-flight caps | a leaf under a 50-team initiative → a bounded number of recomputes; rollup-event storms suppressed |
| **Key allocation** | per-prefix Hi/Lo, adaptive block size, per-prefix isolation | a create-storm → 1 counter write per block; a busy `ENG` doesn't slow `OPS` |
| **SLA timers** | SC-11 minute-bucket wheel; precomputed `fire_at`; cheap disarm/re-arm | millions of far-future timers = an indexed range read per minute per partition; no poll |
| **Concurrent reorder** | frozen `order_key` O(1) + CAS + 2-char jitter + 48-char region-local rebalance | the CAS floor; bounded re-base; CRDT is the measured follow-on |
| **Import** | per-tenant in-flight caps + the protected human lane shed order | a 100k-issue import is bounded backfill; it never starves another tenant's interactive traffic (humans last to shed) |
| **Real-time sync** | the frozen firehose `subscribe/resume/scope` protocol | bounded `scope = board:<id>` + resume-cursor; reconnect loses zero ops |
| **OLTP shard** | one DB per service, sharded by tenant | a hot tenant → tenant-shard split; distributed-SQL is the *measured* follow-on |

Continue to [`03-events-contracts-and-glue.md`](./03-events-contracts-and-glue.md).
