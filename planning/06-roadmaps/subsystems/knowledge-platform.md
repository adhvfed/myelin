# Phase 6 — Knowledge Platform Roadmap (the sequenced build)

> Phase: `06-roadmaps/subsystems`. **Refines the work inside the master bands; does not contradict them.**
> Master spine: [`../00-master-sequencing.md`](../00-master-sequencing.md) (M0..M6, the critical path, the gate
> invariant). Frozen architecture this roadmap sequences (it does NOT redesign):
> [`../../04-subsystem-architectures/knowledge-platform/architecture/`](../../04-subsystem-architectures/knowledge-platform/architecture/)
> (00–07) + the design sketches
> [`../../04-subsystem-architectures/knowledge-platform/design/`](../../04-subsystem-architectures/knowledge-platform/design/)
> (IA, user-flows, wireframes). Contracts: [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md).
> Drills: [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md)
> (KN-D1..KN-D13 + the E2E wedge). Doctrine: EI-01 (order-by-non-negotiability, the gate invariant, name-your-
> floors), EI-04 §2 (real-time collab — the named hard problem). Plain-text identifiers. Markdown only; no
> commits. Date: 2026-06-19.
>
> **What this is.** The detailed sequenced roadmap for the Knowledge Platform (Notion-class workspace,
> subsystem #4). Knowledge is a **producer** subsystem: it lands in master band **M3** (the producer band,
> alongside Git), with a structural floor wired into M2 (it LEADS and FREEZES `myelin-content`) and its
> world-scale follow-ons (the CRDT promotion, multi-cell collab) scheduled into **M5**. Every milestone maps
> to a master band, names its floor + follow-on, lists its upstream dependencies, and is bounded by quantified
> KN-D drills that must emit a green artifact (PROVEN, not CLAIMED) before the milestone is done.

---

## 1. How Knowledge slots into the master bands

The master sequencing puts Knowledge's bulk in **M3 (the producer subsystems)** with Git, because Knowledge
*produces* the docs/databases that Issues/Chat/Search/the-agent-trace consume, and it is the heaviest collab
surface to scale. But Knowledge has obligations **earlier and later** than M3:

| Master band | Knowledge's obligation in that band | Why it lands there |
|---|---|---|
| **M0** | Consume the workspace/glue-crate skeleton; declare its hot-table flags (`block`/`db_row`/`doc_op`, contract 1.5); nothing Knowledge-specific ships. | Knowledge is a service shell over `serve(AppSpec)`; the lints + outbox + harness are preconditions. |
| **M1** | Nothing ships, but Knowledge's writes are blocked until restore-verify (STOR-D1/D2) + Identity (`list_objects`/`check`) + the per-subject DEK KMS hierarchy are green. | The silent-data-loss floor + the dependency root must exist before Knowledge writes a row (gate invariant). |
| **M2** | **LEADS and FREEZES `myelin-content` (13.1) and co-owns `myelin-query` + `order_key` (13.3)** — byte-identical, before Issues/Chat consume the subset; depends on the firehose resume-cursor transport (3.5) landing here. | Knowledge owns the shared content/query crates; they must freeze in M2 so Issues/Chat/Search cannot drift (X-2/X-3). The collab transport rides the M2 firehose seam. |
| **M3** | **The subsystem itself** — KN-M3a..KN-M3e below (transport-first, editor primitives, CAS floor, flexible-DB, refs/search/GDPR/agent-trace). | The producer band; Knowledge ships its floor-first here. |
| **M5** | The CRDT promotion (Yrs over the M2 transport), cross-cell collab, per-facet/per-rollup materialisation, the all-hands-doc surge, and the E2E-1/E2E-3 wedge legs. | World-scale hardening + the named floor follow-ons; the CRDT's trigger is the first true concurrent-edit conflict (measured). |
| **M6** | Knowledge hosts Myelin's own roadmap/gap-report/scorecard; the switch-test driven in a browser. | Dogfooding. |

**The one-line thesis Knowledge must hold to:** *the resume-cursor durable transport is item 0 (built FIRST,
in M2/early-M3); the editor round-trip (`render(parse(md)) === md`) is the correctness bar regardless of
concurrency engine; the v1 merge floor is per-block CAS (no silent overwrite, no blend); the CRDT is the named
M5 promotion that slots into the same transport without touching the data model.*

---

## 2. The contracts Knowledge must implement, and by which milestone

From the frozen contract index. **Owns/leads** = Knowledge is the definition site or freeze authority;
**implements** = Knowledge satisfies a shared-owned contract.

| Contract | Role | Milestone due | Notes |
|---|---|---|---|
| 1.1 `serve(AppSpec)`, 1.2 three-surface, 1.3 liveness≠readiness | implements | KN-M3a (shell) | every Knowledge service is a thin shell over the harness. |
| 1.4 `PersonalDataHolder` auto-registration | implements | KN-M3a (shell) → fully KN-M3e | the harness auto-registers each store; the holder methods land in KN-M3e. |
| 1.5 forward-only migrations + **hot-table flags `block`/`db_row`/`doc_op`** | declares | M0 / KN-M3a | the high-write tables Knowledge declares to the migration runner. |
| **13.1 `myelin-content` taxonomy (LEADS + FREEZES)** + 13.2 ADF lossy-map | **owns/freezes** | **M2 (freeze) → KN-M3b (impl)** | the block + inline AST Chat/Issues consume as subsets; the 3 inline ref nodes; the WASM `render(parse(md))===md` target. ADF map feeds Issues import. |
| **13.3 `myelin-query` + `order_key`/LexoRank (co-owns, byte-identical with Issues)** | **co-owns/freezes** | **M2 (freeze) → KN-M3d (executor)** | FieldType enum, ViewSpec, QueryAst (= EventMatcher core 3.4), LexoRank. Knowledge owns its executor. |
| **3.5 firehose `subscribe/resume(scope=doc:<id>)`** (collab transport) | implements (owns the resume-cursor + idempotent apply over the seam) | **M2 (seam) → KN-M3a (transport item 0)** | the bus provides the seam; Knowledge owns the resume-cursor protocol + CRDT over it (KN-1). |
| 2.1 EventEnvelope, 2.2 `OutboxTx::emit`, 2.4 `EventHandler`, 2.5 dedup, 2.8 upcasters | implements | KN-M3a (outbox) → KN-M3e (consumers) | coalesced semantic events, never raw keystrokes; the only emit path. |
| 2.6 reindex-from-source (`replay(scope)`, block-granular `*.snapshot`) | implements | KN-M3e | the only recovery path for Knowledge's derived state; gate KN-D6. |
| 2.7 crypto-shred / `*.erased` tombstones | implements | KN-M3e | bus is a holder; Knowledge's op-log PII is per-subject-DEK envelope-encrypted. |
| 4.1 `authenticate`, 4.2 `check`+`CaveatContext`, 4.3 `list_objects`+`SetExpr`, 4.6 `write_tuples`/zookie, 4.10 zookie consistency | implements (calls) | KN-M3a (check/auth) → KN-M3c (list_objects SetExpr) | every read/write path; field/transition ABAC via CaveatContext off the hot path. |
| **4.9 Knowledge ReBAC namespace fragment** (page-tree inherit-with-overrides + row + field caveat) | **owns the fragment** | KN-M3c | `page.read = (parent_page->read + direct_reader) - direct_block`; `row_reader` userset; `view_field` caveat. |
| 5.1 ArtifactRef, 5.2 `resolve`, 5.3 backlinks/traverse, 5.4 `refs.edge.created`, 5.5 TE-7 typed-edge mirror (`page_parent`/`db_relation`), 5.6 `project(ref,viewer)`, **5.7 `#sub` grammar (`b`/`h`/`row-`/`field-`/`comment-`) + tombstone ladder** | implements (5.6 owns the `project` impl; 5.7 owns the stable-id mint) | KN-M3d | every embed/mention/backlink; the 4-step tombstone ladder on loss. |
| 6.3 `declare_indexable(IndexSpec)`, 6.1/6.2 `query`/`semantic` (with the `Filter` conjoin) | implements (feeds) | KN-M3d | Knowledge is a primary feeder + the prime RAG corpus; block+page multilingual + vector-in-v1 + JSONB struct. |
| 7.3 `humanise` (the ONE templating surface), 7.6 `define_notif_rule`, `watcher` relation | implements (registers) | KN-M3d | living-doc templates + SLA-style strings register into the one ICU surface; mentions/comments/shares/watched rules. |
| 8.1 `ToolDef`s (frozen `requires_approval` defaults: publish/confidential = yes), 8.2 `EffectApi` apply, **8.8 AG-7 content-addressed agent-trace holder** | implements (8.8 is a Knowledge deliverable) | KN-M3e | agent edits flow through the SAME collab protocol as humans; the four uniform sandbox guarantees apply (8.4, drilled AG-D4 in M2). |
| 9.1/9.2/9.4 `SCHEDULE_AND_RUN_JOB` + per-effect `idem_key` + durable HITL signal, 9.3 timer wheel | implements (calls) | KN-M3e | scheduled living-doc automations; HITL approval cards resume via durable signal. |
| 10.1 `PersonalDataHolder{locate/export/rectify/restrict/erase}`, 10.2 `#[personal_data]` tags, 10.9 the ONE erasure posture (by reference) | implements | KN-M3e | the **hardest GDPR surface** in Myelin; per-subject DEK crypto-shred reaching the immutable op-log. |
| 11.1 OLTP client, 11.2 `BlobStore` (media + CRDT snapshots), **11.4 per-subject DEK crypto-shred** (free-text/op-log) | implements (calls) | KN-M3a (OLTP) / KN-M3e (DEK) | fs-backed blob floor in M1 → object-store in M5. |
| 12.1 `(tenant,region)` partition, 12.6 cross-cell pointer bridge (frame only) | implements | KN-M3a (partition) / M5 (cross-cell live) | v1 pins a doc's collab session to one cell (residency by construction). |

**The two highest-fan-in contracts Knowledge owns/co-owns (highest drift risk, frozen in M2): `myelin-content`
(13.1) and `myelin-query`+`order_key` (13.3).** Both must freeze before Issues/Chat consume the subset.

---

## 3. The milestones (each mapped to a master band, with the work)

### KN-M2 — Freeze the shared content + query crates (the structural pre-floor) · master band M2

**Thesis.** Knowledge is the *owner* of two shared crates every other content-authoring subsystem depends on.
They must freeze in M2 (the reactive-shared-layer band) so Issues/Chat/Search build on the frozen subset and
cannot drift (X-2/X-3). This is not yet "the Knowledge subsystem"; it is the shared substrate Knowledge leads.

**Work.**
- Freeze and ship `myelin-content` (13.1): the v1 block taxonomy (paragraph/heading/bullet_list/ordered_list/
  task_list/blockquote/code_block/callout/table/divider/image/embed/db_view/toggle/sync_block) + inline =
  markdown-subset string with the three structured nodes (`mention`/`artifact_ref`/`embed`) at `U+FFFC`
  positional anchors. Compile the parser/serializer to the **WASM target** (one render path, client + server).
- Freeze `myelin-query` (13.3) co-owned with Issues, byte-identical: the FieldType enum, ViewSpec, QueryAst
  (= the bus EventMatcher core 3.4), and the LexoRank `order_key` encoding (base-62 `0-9A-Za-z`, `"U"` first,
  midpoint bisection, 2-char jitter, 48-char rebalance, `created_at`+ULID tiebreak).
- Freeze the ADF→`myelin-content` lossy-map (13.2) so Issues import has its conversion table.
- Confirm Knowledge's collab transport rides the **frozen firehose resume-cursor protocol** (3.5) being built
  in M2 by the bus — Knowledge owns the resume-cursor + idempotent-apply *over* the seam (built in KN-M3a).

**Floor-then-full.** None — this is a freeze, committed (the CRDT lands *over* the order model, not by changing
it). The `sync_block` node exists in the taxonomy here (completeness); its *engine* is floored in KN-M3b.

**Upstream dependencies.** M1 green (Identity/storage/tenancy). The bus firehose seam (3.5) co-designed in M2.
Issues must co-author the `myelin-query`/`order_key` freeze (byte-identical) — a cross-subsystem reconciliation
at the plan layer (EI-01 §7).

**Gates (must be green to call the freeze done).**
- **KN-D2** — `render(parse(md)) === md` 100% round-trip over the markdown-subset corpus (the 3 structured
  nodes `U+FFFC`-anchored × nesting in bold/lists/tables, code, IME/paste). **100% round-trip, 0 regressions.**
  CI. (KN-D2 is owed in M2 because the WASM render path freezes here; re-run in M3 over the integrated editor.)
- A LexoRank conformance vector shared with Issues passes **identically on both sides** (byte-for-byte rank
  parity) — the X-3 anti-drift check.

---

### KN-M3a — Transport item 0 + the service shell + the outbox · master band M3 (build-order steps 1)

**Thesis (the build-order law, KN-1 / EI-04 §2.2).** A real-time relay *without* resume cursors silently loses
the gap on a reconnect — so the **resume-cursor durable transport is the FIRST thing built**, before any merge
engine, before any editor. This is the silent-data-loss floor for collaboration.

**Work.**
- The Knowledge service shell over `serve(AppSpec)` (1.1/1.2/1.3); declare the hot-table flags `block`/
  `db_row`/`doc_op` (1.5); wire the transactional outbox (2.2/2.3) so every state change emits iff committed;
  the `EventHandler` consumer template (2.4) + dedup ledger (2.5).
- The **resume-cursor durable transport** over the frozen `firehose::subscribe/resume(scope=doc:<page_id>,
  cursor)` protocol (3.5): idempotent apply (`UNIQUE(op_id)`), a durable op-log, per-`(stream,scope)` monotonic
  `seq`, `resync_required` → `*.snapshot` fallback. The presence/awareness ephemeral tier.
- The `(tenant,region)` partition (12.1); the OLTP client + RLS (11.1); the fs-backed `BlobStore` for snapshots
  (11.2, the M1 floor); the `tenant-predicate` discipline (no tenant-less query compiles).
- `authenticate`/`check` on the read/write entrypoints (4.1/4.2).

**Floor-then-full.**
- **Floor:** the transport is CRDT-ready but carries **CAS op bytes** in v1 (the merge engine is KN-M3c).
- **Follow-on:** the op-log carries Yrs update bytes after the `engine_promote` swap (M5); the transport is
  unchanged. KN-D1 is deliberately written to re-run green across that boundary.

**Upstream dependencies.** M2 green (the firehose seam 3.5; `myelin-content` frozen). M1 green (the outbox,
restore-verify STOR-D1/D2, the `(tenant,region)` partition, the per-tenant KMS). The harness (M0).

**Gates.**
- **KN-D1** — kill a collab client mid-edit + sever the connection during sustained multi-author edit; on
  `resume(scope=doc:<id>, last_seq)` assert **0 ops lost, 0 duplicate effects**. (This leg runs on the CAS
  transport now; the CRDT-boundary re-run is owed in M5.) Telemetry: op-log apply lag, dedup hit-rate,
  resume-gap size, `resync_required` rate. **CI.**
- **KN-D7** — crash Knowledge *between* the block/row commit and relay-publish → the event is still delivered
  (outbox survived) and never delivered without the state change. **0 ghost, 0 lost.** CI.
- **KN-D13** — read a page/db/row across tenants via path-tenant spoofing → **0 cross-tenant read**; the
  `tenant-predicate` lint catches a tenant-less query at compile. CI.

---

### KN-M3b — The editor primitives + the block tree · master band M3 (build-order steps 2)

**Thesis (KN-4).** The editor round-trip is the correctness bar *regardless of concurrency engine* — so the
serializer, offset model, and DOM-surgery primitives ship and are unit-tested standalone **before** the
integrated editor and before any merge logic.

**Work.**
- The editor primitives standalone: the markdown-subset serializer, the offset model, the DOM-surgery for
  Enter-splits-block / caret-after-split, IME/paste handling.
- The block tree: per-block rows in an adjacency list (`parent_id` + the frozen LexoRank `order_key`); stable
  opaque block ids that survive moves/edits/collaboration (the `#sub` `b<opaqueid>` targets); page hierarchy
  (sub-pages = folder-like nesting); version history/snapshots; op-log compaction/GC.
- The `sync_block` **read-projection floor** (Δ3): the node exists in the taxonomy; v1 renders it via Refs
  `resolve(ref, viewer)` (like `embed`), permission-filtered per viewer — **not** editable-in-place multi-home.

**Floor-then-full.**
- **Floor:** `sync_block` = read-projection only (no shared-mutable node).
- **Follow-on:** editable-in-place multi-home synced blocks designed against the CRDT (most-restrictive-of-sites
  permission + reference-counted erasure via the edge index), KQ-6 — post-M5.

**Upstream dependencies.** KN-M2 (`myelin-content` + `order_key` frozen). KN-M3a (the transport + outbox).

**Gates.**
- **KN-D2** re-run over the integrated editor: `render(parse(md)) === md` **100%, 0 regressions.** CI.

---

### KN-M3c — The CAS merge floor + the ReBAC permission fragment · master band M3 (build-order step 3)

**Thesis.** The v1 merge floor that *does not blend but never silently overwrites* (EI-04 §2.1), plus the
page-tree permission fragment so every read is leak-free by construction (the `SetExpr` push-down).

**Work.**
- Per-block optimistic compare-and-swap: each write guarded on the block's last-modified token; on a
  precondition miss, reject the loser and hand back current server state to reconcile. Advisory soft-locks +
  snapshot/restore layered on. Offline = read + queued light-edit reconciled via the CAS floor (the deep
  offline-first answer arrives with the CRDT).
- The **Knowledge ReBAC namespace fragment** (4.9): page-tree inherit-with-overrides
  (`page.read = (parent_page->read + direct_reader) - direct_block`); row-level via the `row_reader` userset
  pushed down by `InRelation{relation: row_reader, via_column: db_row.id}`; field-level via the frozen
  `CaveatContext{object, field, attrs}` on `view_field`, evaluated at `check`-time **off the hot path**.
- `write_tuples` → zookie stamped on `page.acl_zookie` (4.6/4.10); the `list_objects` `SetExpr` Filter lowered
  to a SQL JOIN over Knowledge's own `db_row.id`/`page.id` against the per-tenant authz reverse index (4.3) —
  no N+1, no post-filter.

**Floor-then-full.**
- **Floor:** CAS (no merge). **Follow-on:** the Yrs CRDT (KN-1, M5), triggered by the first true concurrent-edit
  conflict (measured via the KN-D3 CAS-conflict-rate metric — KQ-1).

**Upstream dependencies.** KN-M3a/b. Identity `list_objects`+`SetExpr`+`CaveatContext` (4.2/4.3, M1) + the
per-tenant authz reverse index. The `no-cross-db` + `tenant-predicate` lints (M0).

**Gates.**
- **KN-D3** — two clients edit the same block concurrently → the loser is **rejected with current state** (never
  silently overwritten); different blocks edit in parallel with no false conflict. **0 silent overwrites.** CI.
  *(This is the named-floor proof in the master M3→M4 gate — KN-D3 is the CAS-floor correctness gate before the
  CRDT.)*
- **KN-D5** — a confidential page / overridden sub-page / row-restricted db / field-hidden column must **never**
  appear in any view / backlink / search / embed / RAG result for an unauthorized viewer — **including an
  aggregate `COUNT`** (the `SetExpr` conjoin is *inside* the query). **0 leaked artifacts; 0 count-leak.** CI.

---

### KN-M3d — Flexible-DB + refs/search/notif glue · master band M3 (build-order steps 4–5)

**Thesis.** The structured-collection (database) surface over the frozen `myelin-query` shapes, plus the
connective glue (refs/backlinks/embeds, search feed, notif/humanise) that makes Knowledge *not a silo*.

**Work.**
- The Database Service: a JSONB property bag per row (source of truth) + a derived, GIN-indexed projection;
  typed field definitions; views as `ViewSpec` query projections (table/board/calendar/timeline); two-way
  relations (`db_relation`); the `SetExpr` push-down conjoined into every db query.
- The read-time formula/rollup engine: formulas + rollups **computed at READ TIME, never stored** (13.3 by
  contract); bounded cycle-safe evaluation over the `FormulaAst` (the bounded `myelin-query` expression core);
  a cycle surfaces as `#CYCLE`, never an infinite loop.
- Refs glue: the three inline nodes emit `refs.edge.created` (5.4); implement `resolve`/`backlinks`/`traverse`
  (5.2/5.3); the `#sub` grammar (`b`/`h`/`row-`/`field-`/`comment-`) + stable-id mint + the 4-step tombstone
  ladder (5.7); the TE-7 typed-edge mirror (`page_parent`/`db_relation` source of truth, Refs holds the
  rebuildable projection, 5.5); `project(ref, viewer)` (5.6).
- Search glue: `declare_indexable(IndexSpec)` (block+page multilingual + vector-in-v1 + JSONB struct, 6.3);
  feed `project` to the index; `query`/`semantic` **with the `list_objects` Filter conjoined** (6.1/6.2,
  `search-requires-acl-filter` lint).
- Notif glue: declare the `watcher` relation + `define_notif_rule` (mentions/comments/shares/watched, 7.6);
  feed `project` Display mode into the ONE `humanise` ICU-MessageFormat surface (7.3); living-doc templates
  register here — no second template engine.
- KB-native comment threads over the shared `#thread-`/`#comment-` `#sub` grammar + `myelin-content` AST + the
  shared design-system thread primitive (Δ9 / OQ-L).
- The Export/Import service: lossless JSON (Art. 20 portability), Markdown/HTML/PDF, CSV; the ADF→`myelin-content`
  lossy-map import (13.2).

**Floor-then-full.**
- **Floor 1:** JSONB bag + GIN-indexed projection (read-time facets). **Follow-on:** per-facet generated/
  expression-column index, promoted when a facet crosses the **frozen >5% view-execution threshold** (6.3 /
  OQ-C, measured) — M5.
- **Floor 2:** read-time formula/rollup. **Follow-on:** per-rollup incrementally-maintained materialised
  aggregate, when read-time recompute is **measured** too slow (KQ-4) — M5.
- **Floor 3:** `sync_block` read-projection (carried from KN-M3b). **Follow-on:** editable multi-home — post-M5.
- **Floor 4:** KB-native comment store (one scheme, two stores with Chat). **Follow-on:** consolidation onto the
  Chat threading primitive + the firehose transport on the real-time-presence trigger (KQ-9) — post-M5; a merge,
  not a rewrite (they already share `#sub` + content + refs).

**Upstream dependencies.** KN-M3a/b/c. Refs (`resolve`/`#sub` grammar/tombstone ladder, M2). Search
(`declare_indexable`/`query`, M2). Notif (`humanise`, M2). The frozen `myelin-query` (KN-M2). Issues co-owns
the `myelin-query` executor parity.

**Gates.**
- **KN-D6** — wipe Knowledge's derived state (the Refs edge projection / Search index); `replay(scope)` (block-
  granular `*.snapshot`) → rebuilt state **matches live**; rebuild uses the live consumer path only.
  **cold == live.** SCHED.
- **KN-D9** — filter/sort/group a large multi-tenant database (JSONB + projection + `SetExpr` conjoin) →
  read-time **p99 within budget**; measure the >5% facet-promotion trigger. SCHED.
- **KN-D10** — a rollup over a large related set, computed at read time (permission-filtered) → **p99 within
  budget**; measure when incremental materialisation is needed. SCHED.
- (KN-D5 re-confirmed now that search/embed/RAG paths exist — the count-leak path is live here.)

---

### KN-M3e — GDPR holder + the AG-7 agent-trace + agent governance · master band M3 (build-order steps 6)

**Thesis.** Knowledge is the **hardest GDPR surface** in Myelin (free-text PII in an immutable op-log + history)
and the agent-native write surface — both must be drilled before the band closes.

**Work.**
- The `PersonalDataHolder{locate/export/rectify/restrict/erase}` impl (10.1) across blocks, rows, history,
  mentions, authorship; the `#[personal_data]` classify-derive tags (10.2); **per-subject DEK crypto-shred**
  (11.4) for free-text/op-log columns (one DEK per (subject, tenant), CR-I — key count O(subjects with inline
  PII), not O(blocks)); embeddings purged on erase (vectors are personal data). The residual handled **by
  reference** to the ONE platform erasure posture (10.9, X-7) — not restated.
- `*.erased` tombstones via the bus (2.7); `restrict` suppresses indexing/agent-use/analytics/notif for a
  subject.
- The **AG-7 content-addressed agent-trace holder** (8.8): accept an agent execution trace as a Knowledge
  document (reusing the block model), register it as an erasable holder.
- Agent governance: register Knowledge `ToolDef`s with the frozen `requires_approval` defaults (publish/
  confidential = yes, 8.1); agent edits flow through the **same collab protocol** as humans; `EffectApi::apply`
  plan-then-apply (8.2); HITL approval cards resume via a durable signal with the per-effect `idem_key` rule
  (9.1/9.4); scheduled living-doc automations as `SCHEDULE_AND_RUN_JOB` jobs (9.2); reserve/settle on every
  agent run (11.7). The four uniform sandbox guarantees apply (8.4 — AG-D4 already green from M2).

**Floor-then-full.**
- **Floor:** the structural erasure floor (per-subject crypto-shred + pseudonym-map shred + `restrict`) is fully
  built and reliable for structured/self-authored PII. **Residual:** third-party free-text PII under the
  documented lawful-basis limit (10.9, `[OPEN — LEGAL]`, KQ-8) — counsel/DPO ratify in one statement; the
  structural floor ships regardless.

**Upstream dependencies.** GDPR/Audit spine (10.x, M1 structural half). The KMS per-subject DEK hierarchy
(11.3/11.4, M1). The agent fabric (`EffectApi`/`ToolHands`/sandbox, M2, **AG-D4 green**). Durable workflow
(9.x, M2).

**Gates (these complete the master M3→M4 exit for Knowledge).**
- **KN-D4** — erase a subject → structured PII purged/pseudonymised, free-text under a per-subject DEK
  crypto-shredded (**unrecoverable in op-log/snapshots/backups**), embeddings purged, backlinks tombstoned.
  **0 recoverable structured PII incl. vectors;** residual per 10.9. SCHED.
- **KN-D12** — erase a subject → content-addressed agent traces crypto-shredded/purged; attribution falls back
  to the pseudonym. **0 recoverable PII in traces; attribution intact.** SCHED.
- **KN-D11** — an agent edits a doc via `EffectApi` → attributed "suggested by agent"; a consequential edit
  (publish/confidential) is **HITL-withheld** (returns `Denied`, no mutation) until approval; a double-click is
  one approval (per-effect `idem_key`); the run passed reserve/settle. **0 ungoverned mutation; 0 mutation
  before approval; 0 double-apply.** CI.

**The master M3→M4 go/no-go for Knowledge (from the master gate table):** KN-D3 (CAS 0 silent overwrites),
KN-D1 (resume 0 lost/dup), KN-D2 (md round-trip 100%), KN-D7 (outbox emit-iff-committed), KN-D5/KN-D13
(confidential + cross-tenant 0 leak). All green → Knowledge does not block M4.

---

### KN-M5 — The CRDT promotion + cross-cell + materialisation + the surge + the E2E wedge · master band M5

**Thesis.** With all five subsystems on one substrate and the deterministic correctness drills green, ship the
named floor follow-ons and prove Knowledge under world-scale load and inside the whole-system E2E scenarios.

**Work — the floor follow-ons (each named in its M3 milestone above).**
- **The Yrs CRDT, after the CAS floor** (KN-1, EI-04 §2): an Automerge-/Yjs-class engine slotting into the M2/M3a
  resume-cursor transport as a Layer-3 swap (the op-log carries Yrs update bytes; the transport is unchanged);
  hybrid granularity — a per-block content CRDT + a tree/move CRDT (Kleppmann move op) for block structure;
  migrated per-doc **online** via the `engine_promote` op. **Trigger:** the first true concurrent-edit conflict,
  measured via the KN-D3 CAS-conflict-rate metric (KQ-1). Full offline-first arrives here (the CRDT is what makes
  deep offline correct).
- **Cross-cell collab, after single-cell** (OQ-I / 12.6): true cross-cell op fan-out for a multi-cell tenant over
  the PII-free `CrossCellPointer` bridge (owned by control-plane / multi-cell tenancy; the contracts are
  cell-agnostic so this extends without a rewrite). v1 pins a doc's collab session to one cell; resolution stays
  cell-local.
- **Per-facet / per-rollup materialisation** (KQ-4): promote a facet past the >5% threshold to a generated index;
  promote a rollup to an incrementally-maintained aggregate when measured too slow.
- **Object-store `BlobStore`** (one-line swap from the fs-backed floor, 11.2) for media + CRDT snapshots.
- **Editable-in-place synced blocks** (KQ-6) and **KB-comments → Chat-threading consolidation** (KQ-9) — both
  enabled by the CRDT; named post-M5 if not pulled into M5.

**Work — world-scale hardening + the E2E wedge.**
- The all-hands-doc surge (the OQ-K shed budget): per-doc op in-flight cap + read-fanout bound + active-editor
  lane reservation (viewers shed before editors, agents before humans); a concurrent-same-gap LexoRank insert
  storm (no key-collision reorder, bounded rebalance).
- Knowledge's legs of the whole-system E2E scenarios: **E2E-1 (PR context pane** — Knowledge design-doc embed
  resolves per-viewer, 0 leak) and **E2E-3 (Spec-to-ship traceability** — a Knowledge spec doc → initiative →
  issues lineage, cold-reindex == live, audit tamper detected).

**Upstream dependencies.** M4 green (all five subsystems exist; the deterministic correctness drills green; the
object store + multi-cell bridge in place to promote onto). The control-plane multi-cell work (M5) for cross-cell.

**Gates.**
- **KN-D1 re-green across the CRDT `engine_promote` boundary** (the floor-promotion is itself drilled —
  0 lost/dup survives the swap). CI/SCHED.
- **KN-D8** — an all-hands doc with thousands of concurrent readers/editors → per-doc op cap + read-fanout bound
  + active-editor lane reservation hold within budget; **other tenants unaffected; 0 reorder** under the same-gap
  LexoRank storm. SCHED.
- **KN-D9 / KN-D10** re-confirmed at world scale; the >5% facet-promotion + rollup-materialisation triggers
  measured and acted on. SCHED.
- **E2E-1 / E2E-3** green (Knowledge's legs): 0 leak to the unauthorized viewer; lineage live == cold; the
  tombstone carries root. SCHED.
- The F6 surge family leg for Knowledge (human lane holds, agent lane sheds 429+Retry-After, cross-tenant
  impact 0). SCHED.

---

### KN-M6 — Dogfooding · master band M6

**Work.** Myelin's own roadmap, gap report, and scorecard live as a Myelin Knowledge space; the team documents
itself in its own Knowledge platform. Drive the real UI in a browser for the **switch test** (EI-01 §4): could a
Notion user move to Myelin without hitting a wall the old tool didn't have? — measured contrast + latency
budgets + `render(parse(md)) === md` against the real anchor (design sketches).

**Upstream dependency.** M5 green (you do not dogfood real team knowledge onto a substrate whose restore-verify
and DSAR fan-out — KN-D4/KN-D6 — are not green).

**Gate.** The switch test passes (driven in a browser, measured), and the truth-up pass confirms every Knowledge
PROVEN row rests on a dated green KN-D artifact.

---

## 4. First runnable / first useful / production-hardened (the honest progression)

- **First runnable** (end of KN-M3a): a Knowledge service boots over `serve(AppSpec)`, a single editor can
  create a page, type blocks, and a *second* connection sees edits live over the resume-cursor transport — and
  killing/reconnecting a client **loses zero ops** (KN-D1 on the CAS transport). No merge engine yet; no
  databases; no permissions beyond tenant isolation. It is a real-time editor floor whose durability is proven,
  not a demo.
- **First useful** (end of KN-M3d): the full Notion-class surface a single team can adopt — block editor with
  the frozen content taxonomy, the CAS merge floor (no silent overwrite), in-document databases with views +
  read-time formulas/rollups, page-tree permissions (0 leak incl. count-leak), backlinks/embeds/mentions,
  search, mentions-notifications. Concurrent editors of the *same block* get a conflict, not a blend (the named
  CAS floor); cross-cell collab and the CRDT are not here yet. This is the "switch test could plausibly pass for
  a small team" point — but only on a single cell, single-region.
- **Production-hardened** (end of KN-M5): the Yrs CRDT (true concurrent-edit merge + deep offline-first),
  multi-cell residency-pinned collab, measured facet/rollup materialisation, the all-hands-doc surge held within
  budget, the GDPR erasure fan-out (per-subject crypto-shred reaching backups, 0 recoverable PII incl. vectors),
  and the whole-system E2E legs green. Object-backed blob storage. This is world-scale-ready.

---

## 5. Digest

**Milestones (each → master band):**
- **KN-M2 (M2)** — freeze `myelin-content` (13.1, LEADS) + `myelin-query`/`order_key` (13.3, co-own with Issues,
  byte-identical) + the ADF lossy-map (13.2). Gate: KN-D2 (md round-trip 100%) + LexoRank parity with Issues.
- **KN-M3a (M3)** — the resume-cursor durable transport (item 0, over the frozen firehose 3.5) + service shell +
  outbox. Gates: KN-D1 (resume 0 lost/dup), KN-D7 (outbox emit-iff-committed), KN-D13 (cross-tenant 0).
- **KN-M3b (M3)** — editor primitives standalone + the block tree (adjacency list + LexoRank). Gate: KN-D2.
- **KN-M3c (M3)** — the per-block CAS merge floor + the ReBAC page-tree fragment (4.9) + the `SetExpr` push-down.
  Gates: KN-D3 (CAS 0 silent overwrites — the named-floor proof), KN-D5 (0 leak incl. COUNT).
- **KN-M3d (M3)** — flexible-DB (JSONB + projection) + read-time formula/rollup + refs/search/notif/humanise
  glue + KB-comments. Gates: KN-D6 (cold==live), KN-D9 (db p99), KN-D10 (rollup p99).
- **KN-M3e (M3)** — GDPR holder + per-subject DEK crypto-shred + AG-7 agent-trace + agent governance. Gates:
  KN-D4 (erasure 0 recoverable incl. vectors), KN-D12 (trace erasure), KN-D11 (agent HITL governed).
- **KN-M5 (M5)** — Yrs CRDT promotion, cross-cell collab, facet/rollup materialisation, object-store blob, the
  all-hands surge, the E2E legs. Gates: KN-D1 re-green across the `engine_promote` boundary, KN-D8 (hot-doc
  surge), KN-D9/D10 at scale, E2E-1/E2E-3.
- **KN-M6 (M6)** — dogfood: Myelin's own docs in Knowledge; the switch test in a browser.

**Floors + follow-ons (name-your-floors):**
- Per-block CAS (no merge, no silent overwrite) → **Yrs CRDT** [M3c → M5; trigger: first true concurrent-edit
  conflict, measured via KN-D3].
- Read-time formula/rollup → **per-rollup materialised aggregate** [M3d → M5; trigger: measured too slow].
- JSONB + GIN projection → **per-facet generated index** [M3d → M5; trigger: facet in >5% of view executions].
- `sync_block` read-projection → **editable-in-place multi-home** [M3b → post-M5; on the CRDT].
- Offline = read + queued light-edit → **full offline-first** [M3c → M5; with the CRDT].
- Single-cell collab → **true cross-cell op fan-out** [M3a → M5; over the PII-free pointer bridge 12.6].
- fs-backed BlobStore → **object-store BlobStore** [M3a → M5; one-line swap].
- KB-native comments (one scheme, two stores) → **Chat-threading consolidation** [M3d → post-M5; a merge].
- Free-text PII structural floor (reliable) → **residual per the platform posture 10.9** [`[OPEN — LEGAL]`,
  counsel ratifies; the structural floor ships regardless].

**Critical upstream dependencies (what must exist first):**
- **M1:** restore-verify (STOR-D1/D2, the silent-data-loss floor — Knowledge writes no row until green);
  Identity `list_objects`+`SetExpr`+`check`+`CaveatContext` (the leak-free pre-filter + field ABAC); the
  per-tenant/per-subject KMS DEK hierarchy (crypto-shred substrate); `(tenant,region)` partition + residency-pin.
- **M2:** the **firehose resume-cursor transport** (3.5) — Knowledge's collab transport rides it; the agent
  fabric with **AG-D4 (sandbox-escape) green** (agent edits/compute); the durable workflow (9.x, HITL +
  scheduled automations); Refs (`resolve`/`#sub`/tombstone ladder); Search (`declare_indexable`/`query` Filter);
  Notif (`humanise`).
- **Cross-subsystem reconciliation (plan layer):** Issues must co-freeze `myelin-query`/`order_key` byte-
  identical in M2 (X-3); Chat/Issues consume the `myelin-content` subset (X-2). These freezes are KN-M2's gate.
- **M5:** the control-plane multi-cell bridge (for cross-cell collab); the object store (for the blob swap).
