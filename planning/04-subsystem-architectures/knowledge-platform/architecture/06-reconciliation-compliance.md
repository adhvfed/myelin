# Knowledge Platform — 06 · Reconciliation Compliance (how this subsystem IMPLEMENTS the frozen contracts)

> See [`00-overview.md`](./00-overview.md) for framing (and §0 for the reconciliation deltas). This doc was the
> Phase-4 "required shared-system changes"; in Phase 5-B it is **how Knowledge now IMPLEMENTS the FROZEN
> reconciled contracts** (no drift) — the contracts that bind Knowledge: `myelin-content`, `myelin-query` +
> `order_key`, the `list_objects` `Filter`/`SetExpr`, the `CaveatContext`, the unified `#sub` grammar +
> tombstone ladder, the firehose resume-cursor protocol, the erasure posture, the sandbox `requires_approval`
> defaults, the per-effect `idem_key`, the sole `humanise` surface — plus any **residual request for Phase 6**.
> Every row cites the frozen contract-index entry it implements.
> The frozen surface: [`05/contract-index.md`](../../../05-refined-shared-systems-architecture/contract-index.md).

---

## 1. The contracts Knowledge LEADS / CO-OWNS (its frozen deliverables)

| Contract | Status | How Knowledge implements it |
|---|---|---|
| **13.1 `myelin-content` taxonomy** (X-2/OQ-B) — **Knowledge LEADS + FREEZES** | SHARPENED → frozen | The full v1 Block set ([01 §2.1](./01-tech-and-data-model.md)) + inline = markdown-subset string with the three structured nodes (`mention`/`artifact_ref`/`embed`, [01 §2.2](./01-tech-and-data-model.md)). Chat/Issues consume strict subsets (Chat excludes `db_view`/`sync_block`/`toggle`). The three inline nodes are the **uniform producers** of `refs.edge.created` ([03 §1.4](./03-events-contracts-and-glue.md)). WASM compile target for the one editor render path; `render(parse(md)) === md` corpus gate (KD-2). |
| **13.2 ADF → `myelin-content` lossy-map** (X-2) — Knowledge owns the import | SHARPENED → frozen | The Import service (`myelin kb import --from adf`) applies the frozen conversion table (panel→callout, mention→`mention` if resolved else text, inlineCard→`artifact_ref` if a Myelin artifact else link, macro→callout(note)+marker, layout→flattened). Every lossy conversion is **recorded in a per-import Knowledge doc** (the import report) — named, not silent. |
| **13.3 `myelin-query` + `order_key`** (X-3/OQ-C) — **Knowledge CO-OWNS with Issues** | SHARPENED → frozen | Knowledge stores the frozen `FieldType` enum, `ViewSpec`, `QueryAst` ([01 §4](./01-tech-and-data-model.md)) and owns its **executor** (the JSONB lowering + read-time formula engine, [02 §4](./02-internals-and-algorithms.md)). The `order_key` uses the frozen LexoRank encoding **byte-identical with Issues** ([01 §2.5](./01-tech-and-data-model.md)): base-62 `0-9 A-Z a-z`, `"U"` first, 2-char jitter, 48-char rebalance, ULID tiebreak. `rollup`/`formula` are read-time-computed (KN-3). |
| **8.8 AG-7 agent-trace holder** | CONFIRMED | `write_agent_trace` accepts a content-addressed trace (reuses the block model, no new schema) + registers it as an erasable `PersonalDataHolder` (K8, [03 §5.2](./03-events-contracts-and-glue.md)). |
| **3.5 firehose resume-cursor transport (the collab tier)** (OQ-J) — Knowledge owns the protocol over the bus seam | SHARPENED → NEW protocol | The collab op-stream is built FIRST over `firehose::subscribe(stream, scope=doc:<id>, cursor)` / `resume(stream, scope, last_seq)` ([02 §2](./02-internals-and-algorithms.md)); per-`(stream,scope)` `seq` == the doc `op_seq`; `resync_required` → `knowledge.page.snapshot` (block-granular). KD-1 is Knowledge's headline drill. |

---

## 2. The frozen `#sub` grammar + tombstone ladder (contract 5.7, X-4 — Δ4)

Knowledge mints the frozen `#sub` kinds with **stable opaque ids**: `b<opaqueid>` (block), `h<opaqueid>`
(heading, **hyphen dropped** vs Phase 4), `row-<opaqueid>` (db row), `field-<opaqueid>` (**new**, a db field),
`comment-<opaqueid>` / `thread-<opaqueid>` (shared with Chat, OQ-L). Refs stores the full sub-URN + the
`#sub`-stripped root; Knowledge's `project` sub-anchor resolver returns LIVE / MOVED (tree move) / OUTDATED
(edited block) / GONE / ERASED through the **one 4-step ladder** ([03 §2.1](./03-events-contracts-and-glue.md)).
A tombstone always carries the root so an embed degrades to "this referenced <parent> (the part is no longer
available)." (Knowledge anchors are stable opaque block ids, so MOVED is a tree move, not Git's content-anchored
3-way diff match.)

---

## 3. The frozen `list_objects` `SetExpr` + `CaveatContext` (contracts 4.3 / 4.2, OQ-E — Δ7)

Knowledge conjoins the returned `Filter{set_expr, zookie}` into every list/board/view/search query, lowering
`SetExpr` over **its own `db_row.id` column** ([02 §4.1](./02-internals-and-algorithms.md)):

- Row-level visibility → `InRelation { relation: row_reader, via_column: db_row.id }` / `TupleSet` → a **JOIN
  against the per-tenant authz reverse index** Identity maintains. **No N+1, no post-filter** — closing the
  count-leak (KD-5).
- Field-level hiding → the frozen `CaveatContext{object, field, attrs}` caveat at `check`-time on the
  already-filtered rows, **off the hot path** ([01 §5.1](./01-tech-and-data-model.md)).
- The zookie bounds staleness; a just-revoked grant is reflected because the JOIN reads the tuple index at-or-
  after the zookie revision (contract 4.10). The `search-requires-acl-filter` lint holds (Search conjoins the
  **same** `Filter`, contract 6.1).

This is the platform's single most-repeated ask; Knowledge is one of the five `via_column` consumers (KN
`database_row` row column).

---

## 4. The frozen ReBAC namespace fragment + zookie (contracts 4.9 / 4.6 / 4.10)

Knowledge declares the page-tree **inherit-with-overrides** fragment + a `row_reader` relation + a field caveat
+ a `watcher` relation per watchable type ([01 §5](./01-tech-and-data-model.md)); Id owns the engine and
compiles it. ACL changes write tuples via `write_tuples([Δ]) → zookie`, stamped on `page.acl_zookie`, so
collab/read authz cannot read a just-revoked grant stale (the new-enemy guard). Pseudonymous attribution uses
the frozen grammar `<pseudonym>@<tenant>.noreply` (contract 4.8).

---

## 5. The frozen sandbox guarantees + `requires_approval` defaults + per-effect `idem_key` (contracts 8.1/8.2/8.4, 9.1/9.4 — X-6, OQ-F)

- **`requires_approval` defaults frozen** (X-6, contract 8.1): `publish`/`edit(confidential)` = **yes**;
  `draft`/`comment` = **no**; `turn_into_issues` = **yes** (and the cross-subsystem effect **inherits Issues'
  default** where it lands). The approver set = `list_subjects(object, manage)` ([03 §5.1](./03-events-contracts-and-glue.md)).
- **The four uniform guarantees** (X-6, contract 8.4): every agent edit through `EffectApi` inherits the cost
  gate (reserve/settle, 11.7), per-run attenuated-token attribution (4.7), HITL withhold (a gated tool returns
  `Denied`, does not mutate, AG-8), and the isolation floor + escape drill (any `compute` is the CI runner's
  `kind=agent` job) — Knowledge re-implements none of these.
- **Per-effect `idem_key`** (OQ-F, contract 9.1/9.4): the Chat approval card resumes the run via a durable
  signal keyed `card_id` (single) / `card_id:<effect_idx>` (batch/partial), so a double-click is one approval
  and a partial approval is well-defined. Scheduled living-doc automations use `SCHEDULE_AND_RUN_JOB` (9.2) —
  the activity dispatches the job (reserve at dispatch) and parks; completion arrives as a durable signal.

---

## 6. The sole `humanise` templating surface (contract 7.3, OQ-L)

Knowledge registers no second template engine. Living-doc/daily-note templates, status strings, and notification
strings all register into the **ONE `humanise`/ICU-MessageFormat surface** (contract 7.3); `project` Display
mode feeds it a routable `ArtifactRef` + a per-viewer humanised string ("alice mentioned you in <Incident
runbook>"). Knowledge declares its `define_notif_rule` set + `watcher` relation (contract 7.6/4.9) for
mentions/comments/shares/watched-page changes into the **one inbox** (contract 7.1).

---

## 7. Search, Storage, Tenancy compliance (the consumed contracts)

| Contract | How Knowledge complies |
|---|---|
| **6.1/6.3 Search** | declares two `IndexSpec`s (page + significant-block, multilingual, **vector-in-v1** for RAG); every query conjoins the `list_objects` `Filter`; embeddings purged on `knowledge.*.erased`; the measured projection-feeder promotion (>5% threshold). |
| **6.4 / 2.6 reindex** | `replay(scope, since)` emits block-granular `*.snapshot` via the outbox→bus→consumer path; the only rebuild path; also the `resync_required` fallback target. |
| **11.2 BlobStore** | media + CRDT snapshots content-addressed (BLAKE3), residency-pinned; immutable-blob erasure = crypto-shred. |
| **11.4 per-subject DEK** | free-text blocks/ops/agent-trace under `<class> = subject:<id>`; one DEK per (subject, tenant), selective on flagged classes (CR-I); crypto-shred reaches op-log + snapshots + backups. |
| **11.5 restore-consistency** | the row↔snapshot-blob↔op-log↔index↔event-offset cross-seam asserted by the restore-verify drill (KD-6 parity + the shared restore drill). |
| **11.6 OLAP + restriction flag** | a restricted subject's content is excluded from analytics (the `restrict` suppression flows into OLAP). |
| **5.5 TE-7 mirror** | `db_relation`/`page_parent` are the typed source of truth; Refs holds the rebuildable projection + fixes inverse pairing ([03 §3.1](./03-events-contracts-and-glue.md)). |
| **12.6 cross-cell bridge** | a doc's collab session is cell-pinned; the control plane carries only the PII-free `CrossCellPointer`; resolution is cell-local ([05 §9](./05-hard-problems.md)). |
| **10.9 erasure posture** | instantiated **by reference**, not restated (§8 below). |
| **2.9 event taxonomy** | the `knowledge.*` list ([03 §1](./03-events-contracts-and-glue.md)) registered + validated under the Bus §6 grammar; the `knowledge.doc.updated` pointer + `knowledge.*.erased`/`*.snapshot` cross-cutting tokens. |
| **1.5 hot-table flags** | `block`, `db_row`, `doc_op` declared hot (forward-only-migration lint). |
| **1.11 per-surface shed budgets** | the KN collab op-stream / hot-doc read-storm budget floor (OQ-K): per-doc op in-flight cap + read-fanout bounded + a reserved fraction for **active editors** vs passive viewers; viewers shed before editors, agents before humans. Tuned by KD-8. |

---

## 8. The erasure residual — by reference, not restated (contract 10.9, X-7 `[OPEN — LEGAL]`)

Per X-7, the platform states **ONE** free-text/immutable-content erasure posture; Knowledge **instantiates it
by reference**, it does not write a fifth residual statement. The structural floor (per-subject DEK crypto-shred
for self-authored content + pseudonym-map shred for identity + `restrict` suppression) is **fully built** and
covers the overwhelming majority. The residual — third-party free-text PII typed by someone else into that
other person's content — is under the documented lawful-basis limit (best-effort `rectify`/tombstone +
never-indexed/never-agent-readable/never-in-analytics-for-a-restricted-subject). **What counsel/DPO ratify (in
one statement, not five):** the lawful basis + documented limit for residual third-party free-text PII. The
Knowledge-specific GD-6 write-up from Phase 4 is **subsumed** by the platform posture 10.9; Knowledge no longer
owns a separate residual artifact (it points at 10.9).

---

## 9. Threading consolidation — confirmed floor + named follow-on (OQ-L)

v1 ships KB-native comment threads (own store) over the **shared `#thread-`/`#comment-` scheme + `myelin-content`
AST + `refs.edge.created`**; the consolidation onto the Chat threading primitive + the firehose resume-cursor
transport is the named follow-on (promote on the real-time-presence trigger). Tracked in the gap report (E-3)
as "KB-native comments floor → Chat-threading consolidation" ([05 §10](./05-hard-problems.md)).

---

## 10. Residual requests for Phase 6 (the only things NOT closed by the frozen contracts)

Every Phase-4 change-request is now **either a frozen contract Knowledge implements (above) or a measured/legal
item in the honesty register**. The genuine residuals:

| # | Residual | Why it's residual | Owner |
|---|---|---|---|
| R6-1 | **The CRDT promotion threshold** (KQ-1) — the measured CAS-conflict rate that fires Yrs. | Measured-not-predicted (the transport + editor are CRDT-ready day one). | Knowledge P6 (measured) |
| R6-2 | **The flexible-DB / rollup materialisation thresholds** (KQ-4) — the measured latency that promotes a facet/rollup. | The >5% view-execution threshold is frozen (6.3); the *latency* trigger for rollup materialisation is measured by KD-9/KD-10. | Knowledge P6 (measured) |
| R6-3 | **The field-level ABAC predicate catalogue per database** (KQ-5) — which columns, which `CaveatContext` attrs. | The mechanism (`CaveatContext` on `view_field`) is frozen; the per-db catalogue is co-designed with Id's role-bundle catalogue. | Knowledge + Id P6 |
| R6-4 | **Synced-block editable-in-place engine** (KQ-6) — the multi-home merge designed against the CRDT. | The node + read-projection floor ship now (Δ3); the editable engine is the CRDT-era follow-on. | Knowledge P6+ |
| R6-5 | **True cross-cell collab op fan-out** (KQ-7) — simultaneous co-editing across cells. | The pointer-bridge frame is frozen (12.6); the op-fan-out is owned by control-plane / multi-cell tenancy. | Control-plane P6 |
| R6-6 | **The erasure residual lawful-basis ratification** (KQ-8) — `[OPEN — LEGAL]`. | Subsumed into the ONE platform posture (10.9); counsel/DPO ratify in one statement; the structural floor ships regardless. | Legal/DPO |
| R6-7 | **The KB-comments → Chat-threading consolidation** (OQ-L) — promote on the real-time-presence trigger. | Floor + named follow-on; same `#sub`/content/refs scheme so it's a merge, not a rewrite. | Knowledge + Chat P6 |

**No residual reverses a frozen contract.** Every Phase-4 "shared-system change" either became a frozen shape
Knowledge now builds to (§1–§7) or is a measured/legal item in the honesty register (above + [07](./07-drills-and-open-questions.md)).

Continue to [`07-drills-and-open-questions.md`](./07-drills-and-open-questions.md).
