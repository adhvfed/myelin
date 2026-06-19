# Phase 6 — Roadmap: Search & Indexing (`myelin-search`)

> Phase: `06-roadmaps/shared`. The detailed sequenced roadmap for the **search-and-indexing** shared system.
> Slots into the master sequencing bands M0..M6:
> [`../00-master-sequencing.md`](../00-master-sequencing.md) (§2 bands, §3 critical-path/DAG, §4 gate
> invariant, §5 name-your-floors). Frozen architecture (this roadmap SEQUENCES, it does not redesign):
> [`../../05-refined-shared-systems-architecture/search-and-indexing.md`](../../05-refined-shared-systems-architecture/search-and-indexing.md)
> (the refined Search architecture) + the refined
> [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md)
> §6 (the contracts Search owns) + §4/§13 (the contracts Search consumes). Drills owed:
> [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md)
> §4.2 (SRCH-D1..SRCH-D10) + architecture §7 (the nine carried-forward drills). Doctrine:
> [`../../../external-insights/01-process-and-quality-doctrine.md`](../../../external-insights/01-process-and-quality-doctrine.md)
> (order-by-non-negotiability; name-your-floors; the committed gates; prove-it-or-it-isn't-real) and
> [`../../../external-insights/04-hard-problems.md`](../../../external-insights/04-hard-problems.md) §5
> (Search + reference graph are easy to under-budget; reindex-from-source is the resilience primitive;
> embeddings/text are personal data). Spine: ADR-03 (`list_objects` pre-filter), ADR-07 (one query AST),
> ADR-10/14 (engine + vector), ADR-11/12/13/16/17 (cells/holder/envelope/backpressure/fail-static). Date:
> 2026-06-19.
>
> **The shape of this system, and what that means for sequencing.** Search is **overwhelmingly a consumer**.
> It owns **no contract crate** — it composes `myelin-query`, `myelin-identity`, `myelin-events`,
> `myelin-gdpr`, `myelin-content`. It holds **only derived, reconstructible state** (architecture §1, §3.4,
> §6.5). Two consequences for the roadmap: (1) Search **cannot start its core build until its upstreams are
> frozen and green** — the `list_objects` `SetExpr` push-down (4.3), the `QueryAst`/`FieldType`/content
> primitives (13.1/13.3), the durable bus + consumer template (2.x), `project(ref, viewer)` (5.6), the `#sub`
> grammar (5.7), the KMS/per-subject-DEK hierarchy (11.3/11.4). (2) Its two cardinal invariants — **a user
> must never find what they cannot access** (F1) and **erasure reaches everything incl. vectors** (the
> erasure family) — are not Search-local features; they are properties of how Search composes upstream
> contracts, so they are drilled the moment the composition exists, never deferred. Search's whole reason to
> exist is the **permission-aware pre-filter at scale**; the rest is engine integration.

---

## 0. Where Search lands in the master bands (the one-paragraph map)

Search's **core build is M2** (the reactive shared layer). Nothing of Search ships before M2 because every
Search code path calls `list_objects` (M1), consumes the outbox (M0), reads `project(ref, viewer)` (M2 Refs),
and is residency-pinned + crypto-shred-capable (M1). But Search is **named in M0 and M1**: its committed lint
(`search-requires-acl-filter`, contract 1.6) ships in **M0** as part of the ratchet, and its
`PersonalDataHolder` auto-registration + per-tenant-index-DEK shred unit are part of the **M1** storage/GDPR
floor (the holder list must be exhaustive before any real data, contract 10.1). Search's **producer-fed index
projections light up incrementally across M3/M4** as each subsystem declares its `IndexSpec` and ships its
`project`. Search's **world-scale hardening + the floor follow-ons are M5** (the 30× surge family, the
filtered-ANN strategy, the object-store backstop, cross-cell federated search designed-not-built). Search
participates in the **M5 whole-system E2E wedge** (E2E-1 the PR pane, E2E-3 reindex-parity, E2E-4 DSAR) and in
the **M6 dogfood**.

The honest progression: **first runnable** = M2 (the leak-free indexer + query path on a single tenant);
**first useful** = late M3/M4 (real producer corpora — git code, KN docs, issues — searchable per-viewer);
**production-hardened** = M5 (30× surge holds, recall@k-under-filter proven, restore + re-erase + cross-cell
designed).

---

## 1. The contracts Search owns / consumes, mapped to the milestone they land in

From contract-index §6 (owned), §4/§5/§13 (consumed). "Lands" = the milestone by which the contract must be
implemented or callable for Search's gate to be green.

### 1.1 Owned by Search (contract-index §6)

| # | Contract | Lands | Notes / floor |
|---|---|---|---|
| 6.1 | `query(ast, viewer, zookie?, page) → RankedResults` — always conjoins the OQ-E `Filter` before scoring | **M2** | the crux. The Issues Tier-3 board-escalation valve is *unblocked* here; its consumer (Issues board) wires in **M4**. |
| 6.2 | `semantic(text\|vec, viewer, k, filter_ast?) → k visible NN` — ACL-filtered-during-traversal | **M2** (built) | vector machinery v1; the **filtered-ANN traversal strategy** + HNSW↔IVF-PQ promotion is the **M5** tuning follow-on (D8). The embedding **model** is a mock→real adapter swap (post-M5/runtime). |
| 6.3 | `declare_indexable(IndexSpec{...})` — per-subsystem projection | **M2** API frozen; **per-subsystem instances M3/M4** | KN/Git in M3; Issues/CI/Chat in M4. The measured projection-feeder promotion threshold (> 5% of view executions) is an **M5** tunable. |
| 6.4 | `reindex(scope) → job` — the only rebuild path; sub-artifact-granular `*.snapshot` replay | **M2** | depends on Bus 2.6 sub-artifact-granular replay (M2) + each owner's `replay` (M3/M4). |
| 6.5 | Code-search input — Git `git.*` projection per blob/ref/symbol | **M3** (Git projection) | v1 = symbol/path/literal-grade. The CI-produced SCIP/LSIF "find usages" follow-on is **post-M4 / named, not built v1**. |
| (10.1) | `PersonalDataHolder{locate/export/rectify/restrict/erase}` — Search is a holder | **M1** registration; **M2** real erase mechanism | auto-registered by the harness (1.4) in M1 so the holder list is exhaustive; the real purge+reindex erase runs once the index exists (M2). |
| (1.8) | telemetry signal set (+ filter-mode split `Ids` vs `Filter`/`TupleSet`) | **M2** | every Search drill asserts against this; no signal = failed drill. |

### 1.2 Consumed by Search — the upstream dependencies that must exist first (contract-index §4/§5/§13)

| # | Consumed contract | From | Must be green by | Why Search blocks on it |
|---|---|---|---|---|
| 4.3 | `list_objects(...) → Ids \| Filter{set_expr}` with the frozen `SetExpr` algebra | **Id (M1)** | **M1** | the leak-free pre-filter — the single most load-bearing dependency. Search cannot query safely until this is frozen + green. |
| 4.2 | `check(..., caveat?: CaveatContext)` | **Id (M1)** | **M1** | bounded re-validation of stale candidates (§4.2.3); field-level redaction off the hot path. |
| 4.10 | `Consistency`/zookie + authz reverse-index revision watermark | **Id (M1)** | **M1** | no-stale-grant + degrade-not-cascade (the D2 property). |
| 2.1/2.4 | `EventEnvelope` + the consumer template | **Bus (M0)** | **M0** | the indexer is an ordinary `myelin-events` consumer; names/units anchor. |
| 2.6 | `reindex(scope)` re-emit + `*.snapshot`/`*.erased`, **sub-artifact-granular** | **Bus + every subsystem (M2/M3/M4)** | **M2** seam; per-owner replay M3/M4 | rebuild + erasure; the only recovery path (SEARCH-1). |
| 5.6/5.7 | `project(ref, viewer)` + the unified `#sub` resolver | **each subsystem (M2 grammar; M3/M4 owners)** | **M2** | Search fetches the searchable projection (NOT the DB) and resolves sub-artifact refs. |
| 13.1/13.3 | `myelin-content` taxonomy + `QueryAst`/`FieldType`/`ViewSpec`/`order_key` | **`myelin-content`/`myelin-query` (M2 frozen)** | **M2** | the analyzable text + the query/structured surface; frozen byte-identical so the compiler is one. |
| 10.1/11.3/11.4 | `PersonalDataHolder` + KMS hierarchy (per-tenant index DEK + per-subject source DEK backstop) | **GDPR/Storage (M1)** | **M1** | erasure + index encryption; HYOK `can_derive_plaintext_index()=false` structural skip. |
| 1.1/1.2/1.8 | `serve(AppSpec)` + three-surface + telemetry | **substrate (M0)** | **M0** | the service shell. |
| 1.6 | the `search-requires-acl-filter` lint | **substrate/CI (M0)** | **M0** | compile-time no-leak; ships in the M0 ratchet. |
| 1.11 | protected-human-lane shed order + per-surface shed budgets (OQ-K) | **harness + Search budget (M0 harness; M5 tuned)** | **M2** mechanism; **M5** numbers | the query surface is one shed lane; D6 proves it. |
| 12.6 | cross-cell PII-free pointer bridge | **control plane (M1 frame; M5 live)** | **M5** | cross-cell federated search rides it (designed-not-built until M5). |

**The critical upstream dependency, stated plainly:** Search's entire correctness story is downstream of
**Identity 4.3 (`list_objects` `SetExpr` push-down)**. If 4.3 is not frozen + drilled green in M1, Search
cannot begin M2 — there is no leak-free query path to build. The second hard dependency is the **frozen
`QueryAst`/content primitives (13.1/13.3)** in M2: without them Search's compiler means something different
from the Issues/KN compilers and the Tier-3 valve cannot share semantics.

---

## 2. The sequenced milestones (Search's slice of each band)

Each milestone below states **the work**, the **floor-then-full progression** (each floor named with its
scheduled follow-on), the **upstream dependencies** (what must be green first), and the **quantified
gates/drills** that call it done. Drill thresholds carry the Q32 defaults-to-beat; Phase 6 measures the final
numbers (EI-02 §8).

---

### S-M0 — The Search ratchet (inside master band M0)

**Master band:** M0 (substrate, harness, committed gates).

**The work (Search's contribution to M0, not Search's own code):**
- **Ship the `search-requires-acl-filter` lint** (contract 1.6) with a **red-fixture** (a query path that
  reaches `engine.search` without a composed ACL filter — must be rejected) and a **green-fixture** (a path
  that conjoins the filter — must be admitted). This makes "permission-aware by construction" a compile-time
  property of every later Search query path. The lint exists *before* the query path it guards, so the path is
  never written without it.
- **Anchor the index document's field/unit names** to the frozen `EventEnvelope` (2.1) and `ArtifactRef` token
  table — `doc_id` = the `ArtifactRef` key, tenant/region first, `indexed_zookie`+`version` as the staleness
  anchor (architecture §3.1). No mechanism, just the names so M2 doesn't drift.

**Floor-then-full:** none — this is a ratchet, not a feature. (A lint with only one fixture is the floor; the
follow-on is the matching second fixture. Both ship in M0 per the contract.)

**Upstream dependencies:** the lint framework + the contract-coverage scanner (M0 substrate). The
`EventEnvelope`/`ArtifactRef` anchors (M0 Bus).

**Gate (green to satisfy the M0→M1 boundary's "all 12 lints green w/ fixtures" clause):**
- **`search-requires-acl-filter` green** with both fixtures — the red-fixture proves it rejects an
  unfiltered query path, the green-fixture proves it admits a filtered one. Wired into CI, loud, never
  `|| true`.

---

### S-M1 — Search as a holder + the index encryption floor (inside master band M1)

**Master band:** M1 (Identity + storage durability + tenancy).

**The work (Search's contribution to the M1 data-loss/holder floor — still no query/index engine yet):**
- **Register Search as `PersonalDataHolder`** via harness auto-registration (contract 1.4) so the H1–H18
  holder list is **exhaustive before any real tenant data exists** (10.1). At M1 the holder is a stub — it has
  no index to purge yet — but it is on the list, so the M5 DSAR fan-out cannot silently miss it.
- **Pin the per-tenant index DEK into the KMS hierarchy** (11.3): per-cell root → per-tenant KEK →
  **per-tenant index DEK** as the tenant-decommission crypto-shred unit and the backup/immutable-segment
  backstop; the per-subject source DEK (11.4) is the additional source-side backstop layer (architecture §4.8,
  change #9). No index exists yet; this reserves the key class so M2's index is encrypted-from-birth.
- **Confirm the residency-pin** applies to the (future) per-tenant index directory: the index lives in the
  tenant's cell; no cross-region index read on personal data (architecture §3.4). The `residency-pin` lint
  (M0) already enforces it structurally.

**Floor-then-full:**
- **Floor: per-tenant index DEK** (the crypto-shred + backup-backstop unit). **Follow-on: per-subject erasure
  by purge+reindex** lands in S-M2 once the index exists (the *primary* erase mechanism; the DEK is the
  backstop). Named so the index-DEK is not mistaken for the whole erasure answer.

**Upstream dependencies (must be green to do this work):**
- **M1 Identity** must exist (Search's holder + future query path are meaningless without it) — but Search's
  M1 work only needs the **holder harness (1.4)** and the **KMS hierarchy (11.3/11.4)**, both M1 storage/GDPR.
- The **M1 exit gate itself** — STOR-D1/STOR-D2 (restore-verify, the silent-data-loss floor), ID-D3
  (cross-tenant 0), ID-D2 (fail-static), ID-D1 (disabled-user N=5min), CP-D2/CP-D3 (misroute + residency-pin)
  — **must be green before Search's M2 core build starts.** Search inherits these; it does not re-prove them,
  but it cannot build the index over a red STOR-D1.

**Gate (Search's piece of the M1→M2 boundary — these are inherited platform gates Search depends on, plus
Search's holder-registration check):**
- Search appears in the harness-generated holder registry (a structural check, not a drill) — **0 stores
  unregistered** (the contract-coverage scanner confirms 10.1 coverage).
- The per-tenant index DEK is a destroyable key in the KMS hierarchy (proven later by SRCH-D4/D9 in M2/M5; at
  M1 the check is structural: the key class exists and `destroy` is callable).

---

### S-M2 — The Search core: leak-free indexer + permission-aware query path (master band M2)

**Master band:** M2 (the reactive shared layer + the safety drills). **This is Search's primary build
milestone.**

**The work (the full Search engine, single-cell, single-tenant-correct):**
- **The engine + three index shapes** (architecture §2): Tantivy in-process behind the `IndexBackend` trait
  (`open/upsert/delete/search/merge/snapshot`); the three co-located shapes keyed by one `doc_id` — full-text
  inverted (BM25), structured/columnar fast-fields over the frozen `FieldType` enum, vector HNSW. OpenSearch
  is the reserved per-cell upgrade behind the same trait (a measured M5 promotion, not built now).
- **The near-real-time incremental indexer** (architecture §4.1): an ordinary `myelin-events` consumer from
  the substrate template (2.4) — idempotent on `event_id` (dedup ledger S3), whitelist subjects (never `*`),
  ack-after-enqueue, bounded prefetch + per-tenant in-flight caps. Per-event pipeline: dedup → fetch the
  owner's `project(ref, viewer)`/`replay` snapshot (NOT its DB, 5.6) → analyze (language-detect → tokenize →
  normalize) → embed (the adapter) if semantically indexed → build `IndexDocument`, stamp
  `indexed_zookie`+`version` → upsert S1/S2 atomically per `doc_id` → mark dedup → ack. ACL state is indexed
  too (a permission-change event updates affected docs' `indexed_zookie`).
- **The permission-aware query pipeline — the crux** (architecture §4.2): `query` calls
  `list_objects(viewer, read, T, zookie?)` → handles **both frozen shapes** — `Filter{set_expr}` (lower the
  `SetExpr` algebra to a native posting-list-level predicate / a JOIN against the per-tenant authz reverse
  index) and `Ids{ids}` (a doc-id set membership clause) — and **conjoins the ACL clause into every branch
  before any scoring**. `engine.search` is private; the only public entry composes the filter first
  (enforced by the M0 lint). `All` → no clause; `None` → short-circuit empty.
- **The query-AST compiler** (architecture §4.6): Search is one compile target of the frozen `QueryAst`
  (13.3) — validate against the frozen `FieldType` + the bounded-cost guard (no UDFs/loops/recursion); lower
  to FT/structured/vector branches; **always conjoin** `acl_clause(list_objects(...))`; render back for the
  UI. `rollup`/`formula` fields are read-time-computed — Search indexes their *inputs*, never the derived
  value.
- **Hybrid + vector** (architecture §4.5, §4.2.2): RRF fusion (score-scale-free); **filter-during-traversal**
  so the k returned are the k-nearest **visible** neighbours, not k-then-filtered. Agent RAG via `semantic` is
  permission-correct by the same pre-filter.
- **Erasure as a real holder** (architecture §4.8): `locate/export/rectify/restrict/erase` — **erase =
  purge + re-index, not hide**; vectors tombstoned + compacted; embeddings erased with their source doc;
  `restrict` suppresses indexing/agent-use/analytics. Driven by `*.erased` via the same live consumer path —
  no erasure backdoor. HYOK structural skip when `can_derive_plaintext_index()=false`.
- **Reindex-from-source** (architecture §4.9): `reindex(scope)` calls the Bus re-emit protocol (2.6) →
  owners `replay` → `*.snapshot` through the live indexer. One code path for steady-state and recovery; no
  "load the index from Postgres" backdoor (SEARCH-1). The cursor store S4 (throttled/resumable/per-tenant
  caps) is v1.
- **Caching + freshness** (architecture §4.10): the `list_objects` filter cache (S5) caching the typed
  `ListObjectsResult`, `TTL ≤ revocation SLA`, bypassed for zookie-stamped queries; the hot-query result
  cache (zookie-bucketed, request-coalesced).
- **Telemetry** (architecture §4.11): index lag; query RED per principal-kind+tenant; `list_objects` rate +
  cache hit + **filter-mode split**; zero-escape counters; reindex parity hash; erase receipts;
  vector-tombstone/compaction lag; consumer lag; per-tenant in-flight + shed counts.

**Floor-then-full progressions (each named with its scheduled follow-on):**
- **Floor: BM25 default ranking.** **Follow-on: learning-to-rank / semantic re-rank** (post-M5, measured
  relevance-gap-triggered, architecture §4.3). Built as a config layer over BM25 so the follow-on is a
  re-ranker swap, not a rewrite.
- **Floor: the vector machinery (HNSW + filter-during-traversal) with a *mock* embedding adapter behind a
  trait.** **Follow-on: the real EU-hostable embedding model adapter** (post-M5/runtime, ADR-12.8) — a
  config/impl swap, not a rewrite; the vector math + erasure are ours and decided now. The `model_ref` pins
  the adapter so a swap triggers a re-embed reindex, never a silent mixed-model index.
- **Floor: filter-during-traversal as the recall mechanism.** **Follow-on: the tuned filtered-ANN strategy**
  (M5) — the brute-force-fallback threshold under very selective filters + the HNSW↔IVF-PQ promotion point.
  The *property* (k visible neighbours, no leak) is fixed now; the strategy is M5 (D8).
- **Floor: the `IndexSpec` API + the first producer-fed projections.** **Follow-on: each subsystem's real
  `IndexSpec`** lands M3 (KN/Git) and M4 (Issues/CI/Chat). At M2 the API is frozen and exercised with a
  synthetic/test producer.

**Upstream dependencies (all must be green before S-M2 starts):**
- **M1 fully green** — Identity 4.3 (`SetExpr` push-down, the crux dependency), 4.2 (`check`+`CaveatContext`),
  4.10 (zookie + revision watermark); KMS 11.3/11.4; restore-verify STOR-D1/D2; residency CP-D2/D3.
- **M0 green** — the outbox + consumer template (2.x), the `search-requires-acl-filter` lint, the
  failure-injection harness (the unit of proof for every Search drill).
- **M2 siblings, frozen, that Search composes:** the frozen `myelin-content`/`myelin-query` crates
  (13.1/13.3); the Bus reindex-from-source re-emit + sub-artifact-granular `*.snapshot` (2.6); the `#sub`
  grammar + tombstone ladder (5.7) and `project(ref, viewer)` (5.6) from Refs. (Search consumes the **durable
  bus**, NOT the firehose live tier — OQ-J does not touch Search; noted so no one wires Search onto the
  firehose, architecture change #11.)

**Gate (the Search rows of the M2→M3 boundary — these are CI-tier deterministic correctness drills that must
emit green telemetry to call S-M2 done):**
- **SRCH-D1 (F1, the cardinal sin) — zero-escape leak:** a confidential issue / overridden page / private
  channel / private repo file **never** appears in any `query`/`semantic` result, **including counts, IDF,
  "more results", and RAG**, for an unauthorized viewer. Gate: **0 leaked docs, 0 count-leak** across an
  adversarial corpus. Green artifact: the zero-escape counter at 0. — **CI.**
- **SRCH-D2 (F1/F8) — zero-escape-under-staleness:** revoke a grant, re-search with the post-revoke **zookie**
  → excluded (zookie bypasses fail-static + honours the reverse-index revision watermark); default-consistency
  excludes within **W ≤ revocation SLA**. Gate: **0 stale-allow with zookie; ≤ W without**. — **CI.**
- **SRCH-D3 (F2) — cross-tenant IDOR:** spoof path-tenant → **0 cross-tenant results** (tenant from token,
  partition key enforced). — **CI.**
- **SRCH-D4 (erasure) — erasure-reaches-search:** erase a subject → every doc/field/**vector/embedding**
  purged (not hidden) and unrecoverable (per-tenant + per-subject DEK backstop); **0 orphan embedding, 0
  recoverable personal data incl. vectors**. Green artifact: the embedding-purge receipt. — **SCHED** (its
  cold cousin SRCH-D5 + a moderate-scale CI variant gate the band; the full backup-level proof joins the M5
  DSAR fan-out).
- **SRCH-D5 (F4) — reindex-from-cold parity:** wipe the index; `reindex(scope)` → the rebuilt index **== live**
  (docs, ACL behaviour, ranking, vectors, sub-artifact granularity) using the live consumer path only. Green
  artifact: the reindex-parity hash. — **SCHED** (a small-corpus CI variant gates the band; full-scale is M5).

(The remaining Search drills — SRCH-D6 surge, SRCH-D7 freshness, SRCH-D8 filtered-ANN recall, SRCH-D9 restore,
SRCH-D10 HYOK — are scheduled/scale drills that land in **M5**, see S-M5. SRCH-D10's HYOK *structural skip* is
built in M2; the cross-store-plaintext assertion at scale is M5.)

**This milestone is part of the master M2 exit gate** (the §2 list cites SRCH-D1 / SRCH-D3 as the Search rows
of the M2→M3 boundary). M3 does not start over a red SRCH-D1.

---

### S-M3 — Producer corpora light up: code search + knowledge docs (master band M3)

**Master band:** M3 (the producer subsystems — Git hosting + Knowledge platform).

**The work (Search consumes the first real producers; the engine is unchanged, the *projections* arrive):**
- **Code search v1** (architecture §4.4, contract 6.5): consume Git's `git.*` indexable projection per
  blob/ref/symbol. v1 = **symbol/path/literal-grade** — file paths, identifiers (camel/snake tokenizer keeping
  operators), string literals, commit messages, with **trigram indexing** for substring/regex-lite. Code-block
  text from `myelin-content` is **raw**, tokenized with the code tokenizer, not a language stemmer. Search does
  **not** parse repos (no cross-DB read) — Git emits the projection.
- **Knowledge indexing** (contract 6.3): consume KN's `IndexSpec` — block + page text (multilingual analyzers,
  architecture §4.7), the structured inline nodes (`mention`/`artifact_ref`/`embed`) as dependable facets, the
  in-doc database JSONB struct fields (GIN-indexed scan floor), vector-in-v1 for semantic KN search. `rollup`/
  `formula` inputs indexed, derived values never stored.
- **Sub-artifact-granular projections** (architecture §4.1 tail, §4.9 ask; contracts 5.7/2.6): index doc
  blocks (`b<id>`/`h<id>`), KN db rows/fields (`row-`/`field-`), Git line-ranges (`L<a>-L<b>`,
  **content-anchored** — the searchable span is re-derived from the owner's resolve, never a stale raw line
  number). KN `replay` must be page-subtree at block granularity; Git `replay` per-blob/ref.

**Floor-then-full:**
- **Floor: code search v1 = symbol/path/literal-grade + trigram.** **Follow-on: AST-aware "find usages" /
  cross-reference** consuming a **CI-produced SCIP/LSIF** projection (jointly Git+CI, contract 6.5, change #8)
  — a later index input, **post-M4 / demand-triggered, named in the gap report, not built v1.**
- **Floor: GIN-indexed JSONB facet scan** for KN/Issues custom fields. **Follow-on: the generated
  projection-feeder index** promoted per facet — triggered at **> 5% of a collection's view executions over a
  rolling window** (a Search-owned tunable, **measured, M5**, OQ-C/§4.6.1). The GIN scan serves correctly
  meanwhile; promotion changes cost, never correctness.

**Upstream dependencies:**
- **M2 green** (the Search core — the indexer, the query path, SRCH-D1/D3 green).
- **M3 producers' deliverables Search consumes:** Git's `git.*` projection + its `project(ref, viewer)` +
  per-blob/ref `replay`; KN's `IndexSpec` + block/page `project` + page-subtree `replay`. Search blocks on
  these projections existing — but Search's *engine* does not change, only the corpora it ingests.

**Gate (Search's rows within / supporting the M3→M4 boundary):**
- **SRCH-D1 / SRCH-D3 re-confirmed green on the real Git + KN corpora** (the leak + IDOR invariants must hold
  on production-shaped data, not just the M2 synthetic corpus) — **CI.** This is the gate-invariant ratchet:
  the M2 drills re-run on each new producer corpus.
- **SRCH-D5 reindex-parity green on a Git + KN corpus** (cold == live incl. content-anchored line-ranges +
  sub-artifact granularity), small-to-moderate scale — **CI/SCHED.** (Supports the master-band KN/Git exit
  drills KN-D1/GIT-D7 by proving the searchable projection re-derives correctly.)
- The Search half of **E2E-1 (PR context pane)** participates: a Search hit on a confidential issue resolves
  to a tombstone, **0 title leak, 0 count-leak** (SRCH-D1 in-context) — this is exercised when E2E-1 runs at
  M5 but the Search behaviour it depends on is proven here.

---

### S-M4 — The consumer corpora + the Issues Tier-3 valve (master band M4)

**Master band:** M4 (the consumer subsystems — CI + Issues + Chat).

**The work:**
- **Issues indexing + the Tier-3 board-escalation valve** (architecture §4.2.4, contract 6.1): consume
  Issues' `IndexSpec` (the frozen `FieldType` facets, `order_key` columnar fast-field for sort). When an Issues
  board's filtered scan goes **over its OLTP budget**, the board compiles its query to a Search
  `query(ast, viewer)` that conjoins **the same `Filter{set_expr}`** the OLTP board would have used — so the
  board and Search apply **byte-identical ACL pre-filter semantics**. No leak, no N+1, on either tier. (This
  valve was *unblocked* by OQ-E in M2; its consumer wires in here.)
- **CI log search input** (contract 11.8): index the per-subject-DEK CI-log segments / the `(job, step,
  byte-range)` index so `details_ref` (`#step-<n>`) resolves; CI logs ride the firehose **for live tail** but
  Search consumes the durable sealed segments (not the firehose).
- **Chat indexing** (contract 6.3): consume Chat's `IndexSpec` (message bodies as the markdown subset);
  **search-as-non-member returns 0 results** (the Chat ReBAC fragment `channel.read = member + parent` flows
  through `list_objects` — proven by SRCH-D1 on the Chat corpus, the CHAT-D11 analog).
- **Cross-subsystem facets dependable** now that all five producers emit the structured inline nodes
  uniformly: mention/ref facets are reliable across Git/KN/Issues/Chat.

**Floor-then-full:**
- **Floor: the GIN-scan custom-field path serves Issues board facets.** **Follow-on: the measured
  projection-feeder promotion** per hot facet (M5, OQ-C) — owner of the frequency signal is Issues/KN; Search
  consumes it and decides promotion.

**Upstream dependencies:**
- **M3 green** (Git + KN corpora searchable).
- **M4 producers' deliverables:** Issues `IndexSpec` + the board valve seam (the OLTP-budget escalation
  contract); CI's sealed log segments + the `(job,step,byte-range)` index (11.8); Chat's `IndexSpec` + the
  channel ReBAC fragment. **AG-D4 green** (the sandbox-escape gate) is a band precondition but is not a Search
  dependency directly — Search runs no untrusted code.

**Gate (Search's rows within the M4→M5 boundary):**
- **SRCH-D1 / SRCH-D3 green on the full five-producer corpus** — the leak + IDOR invariants hold across
  Issues + CI logs + Chat (the most adversarial corpus: confidential issues, private channels, fork-scoped CI
  logs). **0 leak, 0 cross-tenant.** — **CI.**
- **The Tier-3 valve parity check:** the same board query run through the OLTP board path and through the
  Search valve returns **byte-identical visible rows** (0 leak divergence between the two ACL pre-filters) —
  **CI.** (Supports the master-band ISS-D2 board-query-<1s gate by giving it a leak-equivalent escalation
  path.)
- **Chat search-as-non-member = 0 results** on the Chat corpus (SRCH-D1 instance, the CHAT-D11 analog) —
  **CI.**

---

### S-M5 — World-scale hardening + the floor follow-ons + the E2E wedge (master band M5)

**Master band:** M5 (world-scale hardening + floor follow-ons + the four whole-system E2E scenarios).

**The work — the world-scale / hard-problem work, explicitly scheduled here (not deferred silently):**
- **The 30× agent/CI query surge** (SRCH-D6, F6 family): the protected-human-lane shed order (1.11) tuned to
  Search's query surface — a human's interactive search holds the protected lane; agent/CI search sheds with
  `429 + Retry-After`; per-tenant in-flight caps keep one tenant's agent storm off another's humans. The
  per-surface shed-budget *numbers* (OQ-K) are set here from measurement, not predicted.
- **The filtered-ANN strategy follow-on** (SRCH-D8, the named M2 floor's follow-on): the brute-force-fallback
  threshold under very selective ACL/structured filters + the HNSW↔IVF-PQ promotion point. Gate: **recall@k ≥
  threshold under filter, no leak** — the property was fixed in M2; the *strategy* is measured here.
- **The freshness budget** (SRCH-D7): event→searchable p99 within the seconds-grade budget under load; index
  lag alarms before user-visible staleness. The number is measured here.
- **The measured projection-feeder promotion** (OQ-C, §4.6.1): wire the per-facet filter-frequency signal from
  Issues/KN; promote a facet past **> 5% of view executions** from the GIN scan to a generated index. The
  threshold is set by measurement.
- **Restore + cross-seam + re-erase at scale** (SRCH-D9, F3): restore the index with OLTP/blob/offsets to a
  consistent point → **no resurrected erased docs** (post-restore re-erasure runs from the erasure ledger,
  10.8); **no row↔doc↔vector mismatch.**
- **HYOK at scale** (SRCH-D10): mark a content class HYOK → Search skips it
  (`can_derive_plaintext_index()=false`); **0 HYOK plaintext in any derived store** (the cross-store
  assertion, jointly with Storage + Agent).
- **The full erasure proof** (SRCH-D4 at backup scale): joins the M5 DSAR fan-out E2E-4 — every doc/field/
  **vector** purged + unrecoverable **incl. backups**.
- **The object-store index backstop**: the fs-backed `BlobStore` → object-store swap (11.2, one-line per the
  floor table) rides the M5 storage promotion; Search's index segments + immutable backstop move with it.
- **Cross-cell federated search — designed-not-built → the design holds** (architecture §6.4): the §5
  contracts are cell-agnostic; scatter-gather (each cell runs the same permission-filtered query locally over
  its own index/`list_objects`/residency, a residency-free merge fuses only ranking metadata + `ArtifactRef`s,
  never payload/PII; rows resolved **per-viewer in their home cell**) extends without a rewrite, riding the
  cross-cell PII-free pointer bridge (12.6, OQ-I). **Built only when multi-cell goes live in M5**; until then
  the single-cell path is complete and the design is the named floor.

**Floor-then-full (the M5 follow-ons whose floors were named earlier):**
- **Floor (M2): mock embedding adapter.** **Follow-on: the real EU-hostable model adapter** — lands
  **post-M5/runtime** after the safety drills are green; a config swap. (Search's vector math + erasure are
  done; only the model is deferred.)
- **Floor (M3): code search v1.** **Follow-on: SCIP/LSIF "find usages"** — **post-M4/demand-triggered.**
- **Floor (M3/M4): GIN facet scan.** **Follow-on: generated projection-feeder index** — **promoted here**,
  measured.
- **Floor (M2): filter-during-traversal.** **Follow-on: the tuned filtered-ANN strategy** — **here** (D8).

**Upstream dependencies:**
- **M4 green** (all five producer corpora searchable; the deterministic correctness drills green).
- **M5 platform pieces Search consumes:** the multi-cell bridge live (12.6) for federated search; the
  object-store `BlobStore` (11.2); restore-verify at cell scale (STOR-D2); the full DSR fan-out (10.4) for the
  backup-level erasure proof.

**Gate (the Search rows of the M5→M6 boundary — the scale/surge drills + the whole-system wedge):**
- **SRCH-D6 (30× surge)** — human search lane holds (interactive latency within budget), agent lane sheds
  (`429+Retry-After` honoured), other tenants unaffected. Green artifact: shed-counts/lane + search p99. —
  **SCHED.**
- **SRCH-D7 (freshness)** — event→searchable p99 within the seconds-grade budget under load; lag alarms first.
  — **SCHED.**
- **SRCH-D8 (filtered-ANN recall)** — selective filter → k nearest **visible** neighbours; **recall@k ≥
  threshold, 0 leak.** — **SCHED.**
- **SRCH-D9 (restore + cross-seam + re-erase)** — **0 resurrected erased docs, 0 row↔doc↔vector mismatch**
  post-restore. — **SCHED.**
- **SRCH-D10 (HYOK)** — **0 HYOK plaintext in any derived store.** — **SCHED.**
- **SRCH-D4 at backup scale** — **0 recoverable personal data incl. vectors incl. backups** (folded into
  E2E-4). — **SCHED.**
- **The whole-system E2E scenarios Search crosses are green:** **E2E-1** (PR context pane — Search hit on a
  confidential issue resolves to a tombstone, 0 title/count leak); **E2E-3** (spec-to-ship — the wiped Search
  index `reindex`es to **byte-match live**, F4 / SRCH-D5 at scale); **E2E-4** (DSAR fan-out — Search's docs +
  **embeddings** return 0 recoverable PII, the holder-coverage receipt includes Search). Each emits its named
  green artifact.

---

### S-M6 — Dogfooding: Search over Myelin's own work (master band M6)

**Master band:** M6 (Myelin hosts itself).

**The work:**
- Search runs over Myelin's own repositories (code search on the Myelin monorepo), its own Knowledge space
  (the roadmap/gap-report/scorecard docs), its own issues, its own chat. The builders drive real
  cross-artifact search in a browser.
- The **switch test** for search-bearing surfaces (folded into the per-subsystem L5 done-bars): could a
  GitHub/Notion/Jira user find what they expect — code by symbol, a doc by content, an issue by facet — without
  hitting a wall the old tool didn't have? Reached by *driving the real UI*, measured against latency budgets.

**Floor-then-full:** none new — M6 promotes nothing; it exercises the production-hardened system on real
(self-)tenant data.

**Upstream dependencies:** **M5 green** — you do not put real team data (the builders' own work) onto a Search
tier whose restore + re-erase + DSAR fan-out are not green (Tier-1/Tier-6 of the thesis: the team's data is
real tenant data).

**Gate (Search's piece of the M6 done-bar):**
- Search is green on the **self-hosting CI graph** (the Search drills run as Myelin CI jobs on Myelin's own
  commits — the dogfood loop).
- The search switch-test surfaces pass when driven in a browser (measured latency).
- **No earlier-band Search gate is red** (the truth-up pass: every Search PROVEN row rests on a dated green
  artifact — SRCH-D1..D10 + the E2E rows — never a doc claim).

---

## 3. The honest progression — first runnable / first useful / production-hardened

- **First runnable (end of S-M2 / master M2):** a single-tenant, single-cell Search that indexes off the bus
  and answers a `query`/`semantic` permission-filtered by `list_objects`, with **zero leak proven**
  (SRCH-D1/D2/D3 green) and a real purge-and-reindex erase (SRCH-D4 CI variant). It is *correct* before it is
  *broad* or *fast* — the leak invariant is non-negotiable and lands first. The embedding model is a mock; the
  corpora are synthetic.
- **First useful (end of S-M3 → S-M4 / master M3–M4):** real corpora — code by symbol/path (Git), docs by
  content (KN), issues by facet, CI logs by step, chat by message — all per-viewer leak-free, with the Issues
  Tier-3 valve sharing byte-identical ACL semantics. A developer can actually *find their work* across
  subsystems. Ranking is BM25; custom fields are GIN-scanned; the vector model is still mock-or-early.
- **Production-hardened (end of S-M5 / master M5):** the 30× surge holds with the protected human lane,
  recall@k-under-filter is proven, freshness p99 is measured, restore + re-erase + HYOK at scale are green, the
  generated projection-feeder index is promoted by measurement, the real embedding adapter is in (post-M5
  swap), cross-cell federated search is designed-and-extends, and Search passes E2E-1/E2E-3/E2E-4. Only here is
  Search "done" enough to carry the builders' own data (M6).

---

## 4. Digest

**Milestones (Search's slice of each master band):**
- **S-M0 (band M0):** ship the `search-requires-acl-filter` lint (red+green fixtures) + anchor the index-doc
  names to the frozen envelope. The compile-time no-leak ratchet, before the query path it guards.
- **S-M1 (band M1):** register Search as an exhaustive-list `PersonalDataHolder`; pin the per-tenant index DEK
  into the KMS hierarchy; confirm residency-pin. No engine yet — the holder + encryption floor.
- **S-M2 (band M2, the primary build):** the full engine — Tantivy + three index shapes; the bus-fed
  incremental indexer; **the permission-aware query pipeline (the crux: conjoin the `SetExpr`/`Ids` ACL filter
  into every branch before scoring)**; the frozen-`QueryAst` compiler; RRF hybrid + filter-during-traversal;
  purge+reindex erasure; reindex-from-source; caches + telemetry.
- **S-M3 (band M3):** code search v1 (Git `git.*` projection, symbol/path/literal + trigram) + Knowledge
  indexing (blocks/pages multilingual + vector + JSONB facets); sub-artifact-granular + content-anchored
  projections.
- **S-M4 (band M4):** Issues facets + the unblocked Tier-3 board-escalation valve (byte-identical ACL
  pre-filter); CI log search; Chat indexing (search-as-non-member = 0).
- **S-M5 (band M5, world-scale):** the 30× surge + protected lane; the filtered-ANN strategy; freshness; the
  measured projection-feeder promotion; restore+re-erase+HYOK at scale; the object-store backstop; cross-cell
  federated search (designed-and-extends); E2E-1/E2E-3/E2E-4.
- **S-M6 (band M6):** Search over Myelin's own work; the switch test in a browser; green on the self-hosting
  CI graph.

**Floors + follow-ons (name-your-floors):**
- per-tenant index DEK (S-M1) → **purge+reindex per-subject erasure** (S-M2, the primary mechanism).
- mock embedding adapter (S-M2) → **real EU-hostable model adapter** (post-M5/runtime swap).
- filter-during-traversal (S-M2) → **tuned filtered-ANN strategy / HNSW↔IVF-PQ** (S-M5, D8).
- BM25 ranking (S-M2) → **learning-to-rank / semantic re-rank** (post-M5, measured-gap-triggered).
- code search v1 symbol/path/literal+trigram (S-M3) → **SCIP/LSIF "find usages"** (post-M4, demand-triggered,
  Git+CI joint input).
- GIN-indexed JSONB facet scan (S-M3/S-M4) → **generated projection-feeder index** (S-M5, promoted at > 5% of
  view executions, measured).
- single-cell Search (S-M2..S-M4) → **cross-cell federated search** (S-M5, designed-and-extends over the
  PII-free bridge).
- fs-backed `BlobStore` (S-M1/S-M2) → **object-store backstop** (S-M5).

**The critical upstream dependencies (what must exist first):**
1. **Identity 4.3 — `list_objects` `SetExpr` push-down (M1)** — the single most load-bearing dependency;
   Search's entire correctness story is downstream of it. No core build (S-M2) without it frozen + green.
2. **The frozen `QueryAst`/`FieldType`/`myelin-content` primitives 13.1/13.3 (M2)** — so Search's compiler
   means the same thing as Issues/KN and the Tier-3 valve can share semantics.
3. **Bus 2.x — the durable outbox + consumer template (M0) and reindex-from-source + sub-artifact-granular
   `*.snapshot` (M2/2.6)** — the indexer's substrate and the only rebuild path.
4. **`project(ref, viewer)` 5.6 + the `#sub` grammar 5.7 (M2 + per-owner M3/M4)** — Search reads projections,
   never owner DBs.
5. **KMS hierarchy 11.3/11.4 + restore-verify STOR-D1/D2 (M1)** — index encryption, crypto-shred, and the
   silent-data-loss floor Search builds over.
6. **Per-producer `IndexSpec` + `replay` (M3 Git/KN; M4 Issues/CI/Chat)** — the corpora; Search's engine is
   fixed at M2, only the producer-fed projections arrive later.

**The two cardinal invariants drilled earliest, never deferred:** F1 zero-escape leak (SRCH-D1, the cardinal
sin — a leak is simultaneously a security and a GDPR breach) and erasure-reaches-everything-incl-vectors
(SRCH-D4) — both proven the moment the composition exists in S-M2 and re-run as a ratchet on every new producer
corpus (S-M3, S-M4) and at scale (S-M5).
