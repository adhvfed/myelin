# Issue Tracker — 02 · Internals & Algorithms

> See [`01-tech-and-data-model.md`](./01-tech-and-data-model.md) for the schema. This doc details the
> subsystem-specific algorithms: the scheme-resolution precedence algebra, the workflow interpreter, the
> AST→store query compiler + cost-bounding + the projection feeder, Hi/Lo key allocation, LexoRank ranking +
> CAS, the rollup engine, the business-calendar SLA arithmetic, and the real-time sync protocol. Each is the
> concrete resolution of an open question handed forward from Stage 1; the hard-problem *framing* (candidates,
> prior art) is in [`05-hard-problems.md`](./05-hard-problems.md), which this doc implements.

---

## 1. Scheme resolution — the precedence algebra (resolves sketch 02 open Q1)

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

- **Determinism:** the order is total — there is never a tie. (The lattice is enumerated; the first hit wins.)
- **Cached:** the resolution for a given `(kind, T, P, M)` is computed once and cached per-cell (invalidated on a
  `scheme_assignment` change via the bus). It is **off the per-write hot path** — the write loads the *already-
  resolved* compiled scheme.
- **No-config = Linear-simple:** an org with zero assignments resolves to `org_default` for every kind — the
  typed core + the one 3-state default workflow + no field/permission/SLA overlays. Adding governance is adding
  assignments, never migrating data.

This is the resolution of the sketch-02 hand-forward: "deterministic precedence when team/project/type defaults
disagree." The validation UX (S13) surfaces the resolved scheme + a "this transition is governed by scheme *S*
assigned at *(T,P)*" explainer, so an admin can see *why* a scheme applies.

---

## 2. The workflow interpreter — data-driven FSM, safe-AST guards (sketch 02)

A transition request `transition(issue, target_state, actor)` runs the interpreter against the resolved workflow
scheme. **Not codegen** (you cannot recompile the binary per tenant) and **not user-scripting** (no Jira-Groovy
footgun) — a single interpreter over a config FSM, guards as bounded AST predicates.

```
fn transition(issue, target, actor):
    wf      = resolve('workflow', issue.type_id, issue.project_id, team_of(actor))   // §1, cached
    t       = wf.transitions.find(from=issue.state, to=target)        ?? Err(NoSuchTransition)
    Id.check(actor, 'transition', issue)                              ?? Err(Denied)   // ReBAC + transition ABAC overlay
    for g in t.guards:                                                                  // safe-AST predicates (Bus §4.5)
        eval_guard(g, ctx{issue, linked_refs, actor})                ?? Err(GuardFailed{reason})  // pre-assembled reason
    for f in wf.required_fields_on(target):                                             // required-fields-on-transition
        present(issue, f)                                            ?? Err(MissingRequired{f})
    BEGIN TX
        issue.state          = target
        issue.state_category = t.to_category                          // the FIXED category (the invariant)
        issue.state_changed_at = now
        append issue_change_log(actor, {state: from→to})
        for a in t.post_actions: stage(a)                             // assign / set-field / link / arm-trigger (staged, applied in-tx where safe)
        OutboxTx::emit(issue.transitioned{from, to, category}, cause)  // the ONLY emit path
    COMMIT
    // post-COMMIT, off the bus: rollup, SLA pause/resume eval, Search/Refs index, Notif
```

**Guard examples** (each a safe-AST `EventMatcher` predicate, statically cost-bounded, no UDF/loop/recursion):
- `guard: linked PR CI is green` → reads the linked `closes`/`relates` PR ArtifactRefs' CI status via
  `project(ref, viewer)` (no cross-DB; the status is in the projection). The "can't mark Done while CI red"
  guard (flow C1) is this.
- `guard: not blocked_by an open issue` → reads `issue_relation` `blocks` edges pointing at this issue (the
  `issue_rel_dst` index) and checks none has an open `state_category`.
- `guard: approver-role signed off` → reads a `field.approval` value gated by a HITL durable signal (flow B2).

**Why safe-AST, not scripting:** the same predicate language powers saved views, CLI filters, automation
matchers, trigger conditions, and SLA pause conditions (sketch 02/03) — one validator, one cost model, one
permission-aware-by-construction guarantee. An admin authors a guard in the **guard builder** (S13), not a code
editor; an unreachable-state / missing-category-mapping is flagged inline before save.

**Agent parity:** an agent transitions through the **same** interpreter via `EffectApi` (no carve-out, AG-5). A
governed transition is HITL-gated: the gated `ToolDef` is withheld (returns `Gated`, does not mutate), the
workflow emits `agent.approval.requested`, and on `signal(approval)` the step re-runs with the tool now allowed
(flow B2; contract 8.2 / 9.4).

---

## 3. The query planner — AST→store compiler + cost-bounding + the projection feeder (resolves sketch 03 open Q2/Q3; TE-17)

Issues **owns the compiler from the shared `myelin-query` AST to its OLTP store** and the **cost-bounding** (the
ADR-06 line: "share the schema language and the view model, not the query planner"). A query (a saved view, a
CLI filter, a board scan, an agent search) is one AST; the planner picks the cheapest correct execution **tier**:

```
plan(ast, viewer) =
  filter_acl = list_objects(viewer, 'view', 'issue')          // ALWAYS conjoined first (no leak; the pre-filter)
  classify each predicate:
    ├─ typed-core field (state/category/priority/assignee/type/parent/cycle/project)  → TIER 1: indexed OLTP scan
    ├─ custom field that is a MEASURED-HOT facet (has a generated index)              → TIER 2: generated/expression index
    ├─ custom field, cold facet, small bounded result                                → TIER 2b: GIN probe (jsonb_path_ops)
    └─ full-text / cross-artifact / cold facet on a HUGE result / semantic           → TIER 3: escalate to Search
  cost = estimate(rows_scanned, tiers)
  if cost > budget:  push the heaviest leg to Search  OR  return Refine{hint}         // X-3: never an unbounded JSONB scan
  every query: paginated + statement-timeout'd
```

**The three-tier escalation** (sketch 03), with **Search as the pressure-release valve** that stops a cold
ad-hoc JSONB scan from killing OLTP:
1. **Tier 1 (typed core):** `issue_board` / `issue_roadmap` / `issue_assignee` index ranges. The 90% hot path
   (board grouped by state, "my open issues by priority") — Linear-fast.
2. **Tier 2 (custom hot facet):** the per-facet generated index. "This org filters on `severity` constantly" →
   a `CREATE INDEX issue_sev ON issue ((props->>'severity')) WHERE type_id = :bug` provisioned **off the bus**
   by the projection feeder (below). Until promoted, the GIN index (2b) serves it.
3. **Tier 3 (Search):** full-text, cross-artifact, semantic (RAG/dedup), or a cold facet on a huge result →
   `query(ast, viewer, zookie, page)`, ACL-pre-filtered during traversal (Search §4.2; no leak, no N+1). This is
   the deep-dive §6.7 "search-index projection" leg.

**The projection feeder** (resolves sketch-03 open Q3 — "how a field gets promoted to a generated index off the
bus"): a bus consumer watches `issue.updated` deltas and a per-(tenant, type, field_id) **filter/sort frequency
counter**; when a custom facet crosses a measured threshold (KN-3 measured-promotion, never speculative), the
feeder provisions a generated/expression index via a forward-only online migration (expand→backfill→contract; no
blocking `ALTER` on the flagged-hot `issue` table). This is shared conceptually with Knowledge's K7 feeder. **The
cost model + the OLTP↔Search escalation threshold are the PROVE-IT drill** — a large-custom-field tenant board
query under the <1s keyboard budget (T-8; [07](./07-drills-and-open-questions.md)).

**Which fields earn a typed column** (resolves sketch-03 open Q2, co-reviewed with Knowledge for primitive
parity): a field earns a **typed core column** iff it is *always present AND on the hot board/report path* —
`state`/`state_category`/`priority`/`assignee`/`reporter`/`type`/`parent`/`project`/`cycle`/`rank`/timestamps.
Everything else (including governance custom fields like `severity`, `story_points`, `customer_tier`) lives in
the JSONB tail; the *measured-hot* subset of those gets a generated index (Tier 2). `story_points`/`estimate` is
a borderline case kept in JSONB but promoted early by the feeder for orgs that roll up by estimate.

---

## 4. Human-key allocation — Hi/Lo (sketch 04; TE-14)

```
fn allocate_key(prefix) -> String:
    if local_block_for(prefix).is_empty():
        // ONE counter write reserves a block of N; gap-tolerant; the atomic step:
        (lo, hi) = UPDATE prefix_counter
                     SET high_water = high_water + block_size
                     WHERE (tenant, prefix) = …
                     RETURNING high_water - block_size, high_water
        local_block_for(prefix) = (lo+1 ..= hi)
        maybe_grow_block_size(prefix)        // adaptive: raise block_size on a measured high create-rate
    n = local_block_for(prefix).next()       // handed out from memory, no DB contact
    return format!("{}-{}", prefix, n)
```

**Crash-safety:** the `UPDATE … RETURNING` is the single atomic reserve step. A worker that crashes after
reserving but before using a block loses that block — a **gap**, never a double-allocation, never a reuse
(monotonic). Reserve-then-use, leak-a-block-on-crash is the same at-least-once + idempotent shape as the rest of
the platform. The **adaptive block size** (resolves sketch-04 hand-forward): start small (50) to minimise gaps
for cold prefixes; grow (toward 1000) on a measured high create-rate so a hot prefix (an incident storm, an
import) drops contention by N× without serialising on the counter row.

---

## 5. Ranking — LexoRank + server-arbitrated CAS (sketch 06; TE-19)

The `rank text` column is a **fractional/LexoRank string** strictly between an item's new neighbours; a move is
a single-row update (O(1), not a renumber). The encoding is **aligned with Knowledge's `order_key` family**
(primitive parity — same base, same jitter discipline; resolves sketch-06 hand-forward on the encoding).

```
fn reorder(issue, before_rank, after_rank, expected_version) -> Result:
    new_rank = midpoint(before_rank, after_rank) + jitter()      // jitter reduces concurrent same-gap collisions (Figma/Wallace)
    // SERVER-ARBITRATED CAS — the no-silent-clobber guarantee (humans AND agents, one path):
    n = UPDATE issue SET rank = new_rank, version = version+1
          WHERE id = issue AND version = expected_version
          RETURNING …
    if n == 0:  return Conflict{authoritative_order}              // the loser re-bases against fresh state (honest rollback)
    OutboxTx::emit(issue.updated{rank delta}, cause)
```

- **Concurrency:** two concurrent moves to the *same gap* → one wins the CAS, the loser is returned the
  authoritative order and **re-bases** (re-drops) — **no silent overwrite** (the CAS floor philosophy, KN-1 /
  EI-04 §2). The UI shows the honest "reordered by someone else — your change was re-applied below" (S3/S6).
- **Precision exhaustion:** when a gap runs out of digits, a **region-local background rebalance** re-spreads
  that local region's keys (rare, bounded); it never reorders the *displayed* order.
- **Agent parity:** an agent reorders via the **same** `issue.reorder` `ToolDef` through `EffectApi` — the same
  CAS arbitration. An agent that loses the CAS gets an ordinary `Denied`/stale result and re-plans (AG-5). This
  is *why* server-arbitrated CAS (not client-trust) is the floor: it makes human and agent reorders **uniformly
  safe through one mechanism**.
- **Floor → follow-on:** the CAS floor ships v1; the **move-CRDT (Yrs list / Fugue)** is the named follow-on,
  promoted only on *measured* concurrent-reorder pain (R-5 promotion discipline), reusing Knowledge's Yrs type.

---

## 6. The rollup engine + business-calendar SLA arithmetic

### 6.1 Rollup — event-driven, debounced, incremental (sketch 05B; TE-18)

A child change emits `issue.updated` (with field deltas); the **rollup consumer** (the substrate consumer
template) recomputes affected ancestors **asynchronously, debounced, incrementally**:

```
on issue.updated(child, deltas):                          // off the bus, never in the write path (ADR-11.5)
    ancestors = walk_parent_edges(child, depth_ceiling=16, visited_set)   // issue_relation 'parent'; cycle-safe (Refs §4.5)
    for a in ancestors:
        debounce(a, window):                              // coalesce a burst of child changes into ONE ancestor recompute
            recompute_incremental(a, deltas):             // only re-sum what changed (estimate? state_category? dates?)
                new = aggregate(children_of(a))
                if hash(new) == rollup[a].input_hash: SKIP // no-op suppression: no change → no event (stops loop storms, AG-6)
                else:
                    UPDATE rollup[a] = new
                    OutboxTx::emit(issue.rollup_recomputed{a, new})   // feeds roadmap + the forecast agent
```

- **Write path is just "emit the event"** (Phase-2 §7; ADR-11.5) — a leaf change never blocks on an ancestor
  walk; a 10,000-issue import triggers a *bounded* number of ancestor recomputes (debounce coalescing), not
  10,000.
- **Incremental = cheap:** if only `estimate` changed, only re-sum estimates; if only `state_category`, only
  re-count done/total. The aggregate is the `rollup` row (§6 of doc 01), a **derived** value — the edge truth
  stays in `issue_relation`, so the rollup is rebuildable by reindex-from-source (replay).
- **Cycle-safe:** the ancestor walk uses the visited-set + depth ceiling 16 (matching Refs §4.5). A dependency
  cycle is surfaced as a roadmap diagnostic, never a hang.
- **`input_hash` no-op suppression** (resolves sketch-05 hand-forward): a recompute producing the same input
  hash emits **no** `issue.rollup_recomputed` — stopping rollup-event storms and loop amplification (AG-6).
- **Floor → follow-on** (resolves sketch-05 open Q4): **read-time rollup for small subtrees** (cheap, always-
  fresh) is the floor; **materialise (the consumer above) only when a subtree is measured large** (KN-3
  measured-promotion). The debounce-window policy is per-tenant-tunable; the affected-ancestor fan-out is
  bounded by per-tenant in-flight caps (X-3) for the 50-team-initiative case.
- **Forecast** is **not** in the hot rollup path: the rollup provides the inputs (remaining estimate; historical
  throughput from OLAP); a **forecast agent** (swappable strategy, ADR-08) runs a Monte-Carlo over throughput
  samples and writes the `forecast` field + emits `initiative.health_changed` on crossing an at-risk threshold →
  a trigger flags the PM in chat (flow B4). Floor: linear `remaining ÷ velocity`; follow-on: Monte-Carlo agent.

### 6.2 Business-calendar SLA arithmetic (resolves sketch 07 open Q5; the genuinely-owned hard part)

The SLA *timers* are SC-11 (we consume, never rebuild). The owned algorithm is **converting a business-time
budget into a wall-clock `fire_at`** over a calendar, and **re-arming on pause/resume** (Candidate A):

```
fn business_fire_at(start: ts, budget_secs: i64, cal: Calendar) -> ts:
    cursor   = start; remaining = budget_secs
    loop:
        win = next_working_window(cursor, cal)            // DST-correct via IANA tz; skips nights/weekends/holidays
        avail = win.end - max(cursor, win.start)
        if remaining <= avail: return max(cursor, win.start) + remaining
        remaining -= avail; cursor = win.end              // advance to the next window
```

- **On SLA start:** compute `fire_at` (the breach) and `at_risk_fire_at` (the 80% nudge); arm **two**
  `myelin-flow` timers. The wheel only ever holds concrete wall-clock `fire_at`s — it stays the dumb,
  calendar-agnostic SC-11 substrate (we never pollute it — sketch 07 Candidate C rejected).
- **On pause** (an `issue.updated` matching the policy's `pause_conditions` AST — e.g. `state:waiting-on-
  customer`): disarm the timer, store `remaining_business_secs` (the business-time left, computed from now back
  to `started_at`).
- **On resume:** recompute `fire_at = business_fire_at(now, remaining_business_secs, cal)`, re-arm. A handful of
  timer ops per SLA, not a hot loop.
- **Correctness corpus** (the PROVE-IT drill): DST transitions, multi-day spans, holiday boundaries, mid-window
  pause/resume — a deterministic, testable arithmetic corpus + a breach-fires-after-restart drill (the SC-11
  rider). Prior art: business-day libraries; iCalendar RRULE/`VTIMEZONE` for recurrence/timezone correctness.
- **Escalation** on `sla.at_risk`/`sla.breached`: emit a Signal → Notif routes it; a breach can start a
  **durable escalation workflow** (`oncall_now`/`page` → `myelin-flow`) and/or wake a drafting agent ("SLA at
  80% → agent drafts a holding response," HITL-gated). Breach/met feed OLAP for compliance reporting.
- **Floor:** long-`time_to_resolution` SLAs spanning many days of pauses get **history-compaction** (the
  `myelin-flow` continue-as-new note, Workflow §7.5) — flagged as the named follow-on for very-long SLAs.

---

## 7. Real-time sync — optimistic UI + bus-driven cache over the firehose (sketch 08A)

**Floor (v1):** optimistic local updates + bus-driven cache invalidation over the **shared firehose** with a
**resume-cursor on reconnect** — reusing KN-1's reconnect-loses-zero-ops substrate rather than inventing an
Issues-specific socket protocol.

```
client:
    subscribe(scope)                                  // a board / view / issue → an EventMatcher-bounded firehose stream
    on local mutation: apply optimistically; send through the SAME permissioned API (UI=CLI=agent parity)
    on server confirm: keep; on server reject: roll back + one quiet line + the field to fix (§8b.6)
    on firehose event for scope: patch the normalised cache (an agent-moved card animates in, labelled — §6.1)
    on reconnect: send last-seen cursor → replay events since → no silent gap (the KN-1 resume-cursor)
```

- **Subscription scope bounding** (resolves sketch-08 open Q6): a huge board does not subscribe to "all
  `issue.*` for the tenant" — it subscribes to an `EventMatcher`-bounded stream (this project + this view's
  filter), so the per-viewer fanout is bounded. Presence/typing/cursor ride the **ephemeral firehose**, never
  the durable bus (event-bus §4.3; design-language §5.11).
- **Issue-body concurrency is single-author CAS** (ADR-05) — the `version` token; NOT the Knowledge CRDT (the
  description is a single-author edit, not character-level collab). Board concurrency is server-arbitrated CAS
  (§5). **No Issues CRDT in v1.**
- **Floor → follow-on:** offline/local-first is a **named follow-on**, out of v1 scope unless promoted
  (design-language §9 open). The optimistic+resume floor is the v1 bar; the drill rides KN-1's reconnect-loses-
  zero-ops drill.

---

## 8. Scaling internals & hot-spots (the X-4 detail behind [00 §4](./00-overview.md))

| Hot-spot | Mechanism | Bound |
|---|---|---|
| **Board scan latency** | Tier-1 index range over `issue_board` | the `(tenant, project, state_category, rank)` index keeps it an index range even at millions of issues; the keyboard <1s budget is the gate (T-8) |
| **Custom-field filter** | feeder-promoted generated index (Tier 2) → GIN (2b) → Search (Tier 3) | cost-bounded; a query that would scan too much is pushed to Search or returns `Refine{hint}` — **never** an unbounded JSONB scan (X-3) |
| **Rollup fan-out** | debounce coalescing + incremental recompute + `input_hash` suppression + per-tenant in-flight caps | a leaf under a 50-team initiative → a bounded number of recomputes; rollup-event storms suppressed |
| **Key allocation** | per-prefix Hi/Lo, adaptive block size, per-prefix isolation | a create-storm → 1 counter write per block; a busy `ENG` doesn't slow `OPS` |
| **SLA timers** | SC-11 minute-bucket wheel; precomputed `fire_at` | millions of far-future timers = an indexed range read per minute per partition; no poll |
| **Concurrent reorder** | LexoRank O(1) + CAS + jitter + region-local rebalance | the CAS floor; bounded re-base; CRDT is the measured follow-on |
| **Import** | per-tenant in-flight caps (X-3) + the protected human lane shed order | a 100k-issue import is bounded backfill; it never starves another tenant's interactive traffic (humans last to shed) |
| **OLTP shard** | one DB per service, sharded by tenant | a hot tenant → tenant-shard split; distributed-SQL is the *measured* follow-on |

Continue to [`03-events-contracts-and-glue.md`](./03-events-contracts-and-glue.md).
