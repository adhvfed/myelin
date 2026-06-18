# Phase 3 — Search & Indexing (`myelin-search`)

> Phase: `03-shared-systems-architecture`. Canonical brief: [`VISION.md`](../../VISION.md). Doctrine
> (binding): [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md)
> (always) and [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §5
> (Search + reference graph are easy to under-budget; reindex-from-source is the resilience primitive).
> Spine: **ADR-03** (ReBAC / `list_objects` pre-filter), **ADR-07** (single query AST), **ADR-10**
> (Search tier; Tantivy/OpenSearch/Meilisearch-class + vector), with ADR-11 (cells/residency),
> ADR-12 (PersonalDataHolder / crypto-shred / embeddings are personal data), ADR-13 (envelope/`ArtifactRef`),
> ADR-04 (bus semantics), ADR-16/17 (backpressure/fail-static). Directives: **SEARCH-1** (reindex-from-source
> is the only recovery path), **SEARCH-2** (budget the reindex capability up front), X-1…X-5, BUS-3/BUS-4.
> Decision-record §(c) D11 (reindex-from-source first-class), §(e) priors.
>
> **Foundational docs consumed (contracts, NOT re-invented):**
> [`00-platform-substrate.md`](./00-platform-substrate.md) (the consumer template §5, the bootstrap
> harness §3, the three-surface topology §4, fail-static §8, the telemetry contract §10.2, the blob trait,
> PersonalDataHolder auto-registration §3.4); [`identity-and-access.md`](./identity-and-access.md) (the
> `list_objects(subject, perm, type, zookie?) → {ids | Filter{set_expr, zookie}}` pre-filter §8.2, zookie
> consistency §8.4, fail-static interplay §10); [`event-bus.md`](./event-bus.md) (the envelope §3.1, the
> outbox-only emit path, the consumer template §4.2, Signals vs the firehose §4.4, the reindex-from-source
> re-emit protocol §4.9, the `*.erased` tombstone + `*.snapshot` events, the taxonomy/token table §6).
>
> **Status convention.** *DECIDED* = committed for P4/P5; *FLOOR* = a partial answer shipped with a named
> follow-on; *[OPEN → P4/P5]* = handed forward. Every property that can fail names the **drill** that
> proves it (Phase 5 owns execution; this doc enumerates the obligation). Snippets are illustrative
> signatures/schema, not implementations.

---

## 0. Reading map

- **§1** — purpose, responsibilities, the two invariants Search must never break.
- **§2** — the engine choice (Tantivy) with the written why, and the three index shapes.
- **§3** — the data model: the index-document schema, the three sub-indices (FT / structured / vector),
  the per-tenant residency-pinned layout, the dedup/cursor stores.
- **§4** — the algorithms: incremental indexing off the bus, the **permission-aware query pipeline**
  (the `list_objects` pre-filter — the no-leak/no-N+1 crux), BM25 ranking, ANN/HNSW vector search,
  the query-AST compiler, multilingual analysis, embeddings-as-personal-data erasure,
  reindex-from-source.
- **§5** — the contracts/APIs it exposes and consumes. **Stable.**
- **§6** — scaling/sharding in the cell topology.
- **§7** — failure modes + the drills owed (quantified).
- **§8** — cited prior art.
- **§9** — required changes to foundational systems.
- **§10** — open questions for Phase 4.

**Floors named up front:** (a) **semantic/vector search is built; the embedding *model* is a swappable,
EU-hostable adapter** behind a trait — the real embedding backend is a P4/runtime concern (mock→real,
same strategy-pattern mandate as agents, ADR-12.8); (b) **code-search v1 is symbol/path/literal-grade,
fed by a Git P4 input** — world-scale semantic code search (AST/cross-reference) is the named follow-on
(ADR-10/14, `git-hosting §4.5`); (c) **cross-cell federated search for multi-cell tenants is
designed-not-built** (§6.4), inheriting the bus's cross-cell floor (event-bus §7.4).

---

## 1. Purpose, responsibilities, and the two invariants

### 1.1 What `myelin-search` owns

**Unified ranking across all artifact types in one query** *and* per-subsystem search (code, docs, chat,
issues, CI runs) — full-text + structured/field + **semantic/vector** — **permission-aware at query
time**, multilingual (EU), residency-pinned (ADR-03, ADR-07, ADR-10; `technical-structuring §2.4`).

It owns, end to end:

1. **The index tier** — per-tenant, residency-pinned indices with three co-located shapes:
   **inverted-index full-text** (multilingual analyzers), a **structured/field index** (issue custom
   fields, knowledge DB properties, CI run facets), and a **vector index** (HNSW) for semantic search,
   agent RAG, and triage dedup (§3).
2. **The indexer** — **near-real-time incremental indexing fed off the bus** as an ordinary
   `myelin-events` consumer (the substrate template, §00 §5 / event-bus §4.2), idempotent on `event_id`.
   Search **never reads owner databases** (ADR-01/13; SEARCH-1).
3. **The permission-aware query path** — every query **pre-filters via Id's `list_objects`** rather than
   post-filtering results (no leak, no N+1; ADR-03 §Consequences; identity §8.2). This is the **crux**
   and the single most load-bearing inter-system contract (§4.2).
4. **The query-AST compiler** — queries arrive as the shared **query AST** (`myelin-query`, ADR-07),
   permission-aware by construction, compiled to the three index shapes (§4.6).
5. **Embeddings-as-personal-data erasure** — embeddings are derived from personal data and **are personal
   data**; the index is a `PersonalDataHolder` whose `erase` **purges + re-indexes** (not "hide"), and
   crypto-shreds per-tenant index keys (ADR-12; `gdpr-eu-sovereignty §6.6`; §4.8).
6. **Reindex-from-source** as the **only** rebuild path (SEARCH-1, EI-04 §5.3): on rebuild Search asks
   each owner to re-emit `*.snapshot` events through the **live consumer path**; recovery and steady
   state share one code path and cannot drift (§4.9).

### 1.2 What Search is NOT

It is **not** the system of record for any artifact (owners hold that), **not** an authorization
decision-maker (Id is — Search *consumes* `list_objects`), **not** the reference graph (Refs is — though
both consume `list_objects` and both rebuild-from-source), and **not** a second query language (it speaks
the one query AST, ADR-07). It holds only **derived, reconstructible** state; it is a *projection*, which
is exactly why reindex-from-source is its recovery primitive (D11).

### 1.3 The two invariants Search must never break (overview §12)

1. **Permission-aware reads everywhere** — *"a user must never find what they cannot access."* A leak here
   is simultaneously a **security breach and a GDPR breach** (SC-1). Enforced by the `list_objects`
   pre-filter composed *by construction* into every query (§4.2); proven by the zero-escape leak drill (§7).
2. **Erasure reaches everything** — Search is on the exhaustive `PersonalDataHolder` list ("we forgot the
   search index" is the canonical structural failure, ADR-12.1/GD-3). Auto-registered by the bootstrap
   harness (§00 §3.4). Erasure **purges and re-indexes**; embeddings are erased with their source (§4.8).

### 1.4 Platform non-negotiables inherited (not repeated below)

Tenant+region is the first partition key of every index, segment, vector, cursor, and cache (EI-02 §1;
ID-3) — **no cross-tenant query path**; tenant comes from the verified token, never the URL path. Every
store is residency-pinned, per-tenant envelope-encrypted, crypto-shred-capable, and a `PersonalDataHolder`
(ADR-11/12). No subsystem reads another's store (ADR-01/13). The transactional outbox is the only emit
path (Search emits nothing of its own except its `PersonalDataHolder` receipts and telemetry; it is
overwhelmingly a *consumer*).

---

## 2. Engine choice & the three index shapes

### 2.1 Decision: **Tantivy** is the reference engine (DECIDED, with a written escape hatch)

**Decision: Tantivy (Rust) is the in-process index engine library**, embedded in the `myelin-search`
service, one index per `(tenant, subsystem)` (or per tenant with a subsystem facet — §3.4), inside each
cell. This adopts ADR-10/14's leading candidate and the Rust-default (ADR-02). The crux that decides it is
**not raw speed — it is that the ACL pre-filter (§4.2) must be a first-class, cheap query operator**, and
an embedded library lets us compose Id's `list_objects` filter into the query plan natively rather than
bolting a post-filter onto a remote search cluster.

**Why Tantivy over the same-class alternatives** (the *written why* the doctrine requires):

| Candidate | Verdict | Reasoning |
|---|---|---|
| **Tantivy** (Rust) | **Chosen default** | A Lucene-architecture inverted-index library (segment-based, immutable segments + merge, BM25 default scoring, fast fields/columnar for structured filters, multi-language tokenizers). **Embedded in-process** → the `list_objects` `Filter` (a set predicate over doc ids, identity §8.2) compiles to a **native conjunctive filter clause** in the query plan, so permission-filtering is a **pre-filter at the posting-list level, not a post-filter over results** — the no-leak/no-N+1 mechanism (§4.2). Rust-native (ADR-02, no JVM), single-binary → **self-host = one cell, same artifacts** (ADR-11). Per-tenant index directories give residency-pinning + crypto-shred-per-index for free. Vector search is added as a **co-located HNSW index** (§3.3) over the same doc-id space, so FT∩structured∩vector results share one ACL filter. |
| **OpenSearch / Elasticsearch** | Same-class, allowed with a written why | Mature, distributed, has document-/field-level security and a kNN plugin. **But**: JVM footprint taxes one-cell self-host parity; document-level security is **post-filter-shaped** (a query-time filter clause evaluated *after* shard fan-out) and tempts the N+1/leak anti-pattern; operating a cluster per cell is heavier. **Reserved as the per-cell upgrade** when a single tenant's index outgrows the embedded-Tantivy node (the BUS-6-style measured-volume promotion). The §5 contracts are engine-agnostic, so the swap is an indexer/query-backend change, not a contract change. |
| **Meilisearch / Typesense** | Rejected as the core | Excellent instant-search UX and DX, **but** weaker on the structured/field query surface, on a *pushed-down* ACL set filter at scale, and on a controllable vector/ANN integration with erasure. Good for a typeahead front but not for the unified, permission-correct, multilingual, vector-bearing core. |

The engine sits behind a thin `IndexBackend` trait (`open/upsert/delete/search/merge/snapshot`) so
Tantivy→OpenSearch is a one-binding swap, mirroring the bus's `BusTransport` and Storage's `BlobStore`
trait philosophy (§00 §2.7, event-bus §2.1). **There is no path that bypasses the ACL-filter composition**
(§4.2) — it is enforced in the query builder, not left to the engine.

### 2.2 The three query shapes (compiled from the one AST, ADR-07)

Every query is the shared query AST (ADR-07), permission-aware by construction. The compiler (§4.6) lowers
it to one or more of three shapes, then **intersects all of them with the same `list_objects` filter**:

1. **Full-text** — `match`/`phrase`/`prefix` over analyzed text fields, BM25-ranked (Robertson/Spärck
   Jones; §4.3). Chat/docs/code/issues/commit-messages/CI-log-summaries.
2. **Structured / field** — exact/range/`in`/`exists` predicates over typed fields (issue custom fields
   from `myelin-query`'s field definitions, ADR-06; knowledge DB row properties; CI run facets), served
   from Tantivy **fast fields** (columnar) — the structured-filter surface.
3. **Semantic / vector** — k-NN over dense embeddings (HNSW; Malkov & Yashunin 2018) for semantic search,
   agent RAG retrieval, and triage/dedup nearest-neighbour. Hybrid ranking fuses lexical + vector (§4.5).

A single user query commonly compiles to a **hybrid** plan (FT + vector, with structured filters), and the
ACL filter is conjoined into **all** branches before any scoring, so no branch can surface a hidden doc.

---

## 3. Data model / schema

### 3.1 The index document (the canonical projection — references-not-payloads where personal)

Each indexable artifact (or sub-artifact) projects to **one index document**. Subsystems declare *what* is
indexable and *how* they project (§5.3); Search owns the document layout. The doc is keyed by the
`ArtifactRef` and always carries tenant/region first.

```jsonc
{
  // ── identity & ACL key (first-class; ADR-11 / ADR-13) ───────────────
  "doc_id":       "myelin://acme-eu/issue/issue/ABC-123", // the ArtifactRef (PK; sub-artifact granular)
  "tenant":       "acme-eu",                               // partition key — no cross-tenant doc, ever
  "region":       "eu-central",                            // immutable; index inherits tenant's region
  "subsystem":    "issue",                                 // canonical token (event-bus §6.2)
  "type":         "issue",
  "acl_object":   "myelin://acme-eu/issue/issue/ABC-123",  // the object Id's list_objects authorizes on
  "acl_object_type": "issue",                              // the `type` arg to list_objects (§4.2)

  // ── consistency (the zookie that gates fresh-vs-stale reads; identity §8.4) ──
  "indexed_zookie": "zk_01J8...",        // the authz snapshot this doc's ACL state was indexed at
  "version":      42,                    // monotonic per-aggregate (from outbox seq) — staleness/ordering
  "event_id":     "01J8...ULID",         // the event that produced this doc state (idempotency)

  // ── full-text fields (analyzed; language-tagged) ────────────────────
  "lang":         "de",                  // detected/declared language → analyzer selection (§4.7)
  "title":        "Login schlägt fehl bei SSO",
  "body":         "…markdown-subset text, sanitized, mentions/refs extracted…",
  "ft_extra":     { "code_symbols": [...], "path": "src/auth/sso.rs" }, // code-search v1 (§4.4)

  // ── structured/field facets (columnar fast-fields) ──────────────────
  "fields": {                            // typed, from myelin-query field defs (ADR-06)
     "status": "open", "priority": 2, "assignee": "myelin://acme-eu/identity/human/bob",
     "labels": ["auth","regression"], "created_at": "2026-06-10T...", "severity_rank": 3
  },

  // ── vector (semantic) ───────────────────────────────────────────────
  "embedding":    { "model_ref": "emb:eu-mini-v1", "dim": 768, "vec_ref": "vec_seg7_991" },
  // embeddings ARE personal data → erased with the doc; model_ref pins the adapter (ADR-12.8)

  // ── GDPR routing ────────────────────────────────────────────────────
  "contains_personal_data": true,        // from the source event envelope (event-bus §3.1)
  "pii_key_ref":  "kms://acme-eu/2026Q2/tenant", // the per-tenant index DEK; crypto-shred unit
  "visibility":   "project"              // a HINT only; list_objects is authoritative (never trust this)
}
```

**Why these choices.** The `doc_id` **is** the `ArtifactRef` so erasure, refs, and projection all key on
one address (ADR-13). `acl_object`/`acl_object_type` are stored explicitly so the query path can compose
`list_objects(viewer, read, acl_object_type)` **without** re-deriving the authz object from the doc — this
is what keeps the pre-filter cheap (§4.2). `indexed_zookie` + `version` are the **staleness/consistency
anchor** (identity §8.4) that lets a freshly-revoked grant *not* be read stale (§4.2.3). `lang` drives
analyzer selection (§4.7). The whole document is **envelope-encrypted with the per-tenant index DEK**
(`pii_key_ref`); **crypto-shredding that key renders the tenant's entire index unrecoverable** —
tenant-decommission erasure for free (ADR-12.3).

**Inline-text vs reference:** the index *must* hold analyzed text to be searchable (you cannot FT-rank a
reference). So Search holds a **searchable projection of the source text**, classified
`contains_personal_data` per the source event. This makes Search a true holder whose `erase` is a real
purge (§4.8) — not "references-only." The tension is resolved by crypto-shred + purge-and-reindex, not by
pretending the text isn't there.

### 3.2 The three sub-indices (co-located, one doc-id space)

| Sub-index | Structure | Prior art | Purpose |
|---|---|---|---|
| **Full-text (inverted)** | term → posting list (doc ids + positions + freqs); BM25 stats per segment | Inverted index (Zobel & Moffat, *Inverted Files for Text Search Engines*, ACM CSUR 2006); Lucene segment model; BM25 (Robertson & Spärck Jones) | lexical search/rank |
| **Structured (columnar fast-fields)** | per-field columnar values; doc-value style | Lucene DocValues; column stores | exact/range/`in`/facet filters + sort |
| **Vector (HNSW)** | hierarchical navigable small-world proximity graph over dense vectors | Malkov & Yashunin, *Efficient and robust ANN using HNSW*, IEEE TPAMI 2018; Johnson et al. (FAISS) | semantic k-NN |

All three live in **one per-tenant index space keyed by the same `doc_id`**, so a hybrid query (§4.5)
fuses results that already share an ACL filter — there is no separate "vector store" that could leak a
doc the inverted index would have filtered.

### 3.3 The vector index detail (HNSW, erasable)

- **HNSW** is chosen over IVF/flat for the read-heavy, incrementally-updated, moderate-recall workload:
  logarithmic-ish search, good recall at low latency, supports incremental insert (Malkov & Yashunin 2018).
  IVF-PQ is the **per-cell memory-pressure upgrade** when a tenant's vector count makes a flat HNSW too
  large (a measured-volume promotion, BUS-6-style; [OPEN → P4 Search]).
- **Deletes** are soft (tombstone the node) then **compacted on merge** — critical for erasure: an erased
  doc's vector is tombstoned immediately (excluded from k-NN) and physically removed on the next segment
  merge, and the per-tenant key crypto-shred backstops anything in immutable/backup tiers (§4.8).
- **Embeddings are personal data** (`gdpr-eu-sovereignty §6.6`): the vector is erased with its source doc;
  the `model_ref` pins which embedding adapter produced it so a model swap triggers a re-embed reindex
  (§4.9), never a silent mixed-model index.

### 3.4 Index layout, residency, and the stateful-component register (X-4)

| # | Component | Engine | Holds | Shard key | Blast radius | Crypto-shred unit |
|---|---|---|---|---|---|---|
| S1 | **Per-tenant FT+structured index** | Tantivy (per-tenant dir) | inverted + columnar segments | `(tenant, region)` [+ subsystem] | one tenant | per-tenant index DEK |
| S2 | **Per-tenant vector index** | HNSW (co-located) | dense vectors + proximity graph | `(tenant, region)` | one tenant | per-tenant DEK (shared with S1) |
| S3 | **Indexer dedup ledger** | Postgres (consumer template, event-bus §3.3) | `(consumer, event_id)` | `(tenant, region)` | redelivery → re-index (idempotent) | n/a (derived) |
| S4 | **Reindex cursor store** | Postgres | per-(tenant, subsystem) replay cursor | `(tenant, region)` | re-derivable | n/a |
| S5 | **Query result / `list_objects` filter cache** | Redis/Valkey (NEVER source of truth, STOR-3) | recent ACL filters + hot query results, zookie-bucketed | `(tenant, region, subject)` | one cell; staleness-bounded | ephemeral; TTL ≤ revocation SLA |

**All of S1–S5 are derived** and rebuildable by reindex-from-source (§4.9) — there is **no** system-of-
record state in Search, which is the whole point of D11/SEARCH-1. Residency is enforced because the index
*directory* lives in the tenant's cell; there is no cross-region index read on personal data (ADR-11).
Sharding: **measure before you shard** (ADR-10/§(e) prior) — the first scaling move is more index nodes
per cell and the result cache (S5), not premature tenant-index splitting (§6).

---

## 4. The algorithms

### 4.1 Near-real-time incremental indexing (off the bus; the substrate consumer template)

The indexer is an **ordinary `myelin-events` consumer** built from the substrate template (§00 §5;
event-bus §4.2) — it is one of the *excepted infra consumers* allowed on the raw `evt.*` firehose
(BUS-4) because it genuinely needs every domain event. It inherits, mechanically:

- **Idempotent on `event_id`** (ADR-04.1) via the dedup ledger (S3): a redelivered event is a no-op.
- **Whitelist subjects** (`evt.<tenant>.<subsystem>.>` per the projections it registers) — never `*`
  (BUS-3); bind the durable consumer **by name**, never re-declare start policy on reconnect (BUS-3).
- **Ack-after-enqueue** (durably commit the index write before ack); **terminate non-retryable** malformed
  / un-upcastable events to DLQ (BUS-3) rather than burning the redelivery budget.
- **Bounded prefetch + per-tenant in-flight caps** (X-3/ADR-16) so one tenant's index storm can't starve
  another's freshness.

**Per-event pipeline** (idempotent, ordered per aggregate by `(aggregate, seq)`):

```
event → (dedup? skip) → resolve projection for (subsystem, type)
      → fetch source-text projection via the owner's projection API (NOT its DB; §5 / ADR-13)
      → analyze (language-detect → tokenize → normalize; §4.7)
      → embed (call the embedding adapter; §4.8) if the type is semantically indexed
      → build IndexDocument (§3.1), stamp indexed_zookie + version (from event)
      → upsert into S1/S2 atomically per doc_id  → mark dedup  → ack
```

**Near-real-time, not synchronous** (ADR-11.5): indexing is async off the write path. Target freshness
budget is **seconds** (p99), surfaced as a telemetry signal (index lag, §4.11) and asserted by the
freshness drill (§7). A `*.erased` tombstone triggers a **purge + re-index** (§4.8); a `*.snapshot` event
(reindex) is ingested identically (§4.9) — one code path for live, recovery, and erasure.

**ACL state is indexed too.** A permission-change event (`identity.permission.granted|revoked`,
membership change) that affects an artifact's visibility updates the affected docs' `indexed_zookie` (and,
for `ids`-mode caching, can invalidate cached filters, §4.2.3). The doc stores the *object* it is
authorized on, not a materialized allow-list — the allow-list is computed at query time by Id, so a single
permission change does **not** require re-indexing every affected user's view (that would be the N+1 at
index time). This is the deliberate split: **Search indexes the object; Id computes the subject's reachable
set at query time** (§4.2).

### 4.2 The permission-aware query pipeline — **the crux** (filter push-down, no leak, no N+1)

This is the single most load-bearing mechanism in the document (ADR-03 §Consequences; identity §8.2;
overview §10.2). A leak is both a security and a GDPR breach (SC-1).

**The mechanism: pre-filter, never post-filter.** For a query by `viewer` over object type `T`:

```
query(ast, viewer):
  1. filter ← Id.list_objects(viewer, read, T, zookie?)        // identity §8.2 — the leak-free pre-filter
  2. plan   ← compile(ast)                                     // §4.6 → FT / structured / vector branches
  3. plan'  ← plan  ⨯  acl_clause(filter)                      // CONJOIN the ACL filter into EVERY branch
  4. results← engine.search(plan')                             // posting-list-level filtering, then BM25/HNSW
  5. rank/fuse (§4.5), paginate, project, return
```

**Step 1 — `list_objects` returns one of two shapes (identity §8.2), and Search handles both:**

- **`Filter{set_expr, zookie}` (push-down mode — the default at scale).** Id returns a **compiled set
  predicate** (e.g. "objects whose project ∈ {p1,p2} minus confidential-without-grant", expressed as a
  set expression over indexed facets / a reachable-set membership). Search compiles `set_expr` into a
  **native Tantivy filter clause** (a conjunction over the `acl_object`/project/`fields` facets, or a
  doc-id set membership) that runs **at the posting-list level before scoring**. This is the no-N+1, no-leak
  path: the engine never scores, never returns, never paginates a doc the viewer can't see — there is no
  "fetch then check" step to leak through, and no per-result `check` call.
- **`ids` (pre-fetch mode — for small/bounded reachable sets).** Id returns the enumerated reachable id
  set (from its Leopard-class flattened index, identity §8.2). Search applies it as a **doc-id set filter**.
  Used when the set is small (a user's starred repos); a bounded id list is cheaper to intersect than a
  predicate. Search chooses mode on a size threshold returned by Id (or falls back to push-down above it).

**The exact push-down encoding (filter-clause shape vs id-set vs a bloom-style membership) is
[OPEN → P4 Search]** — but the **contract is frozen here and in identity §8.2**: Id returns a
**zookie-stamped `Filter` (or bounded `ids`)**, Search **conjoins it into every branch before scoring**.
The push-down-vs-pre-fetch decision is a *cost* choice; the *no-leak* property holds in both modes because
the ACL clause is part of the matching predicate, not a post-step.

**Step 3 is enforced structurally, not by discipline.** The query builder has **no API to run a search
without an ACL clause** — `engine.search` is private; the only public entry composes `list_objects` first.
This mirrors the substrate's `no-raw-publish` lint (§00 §2.11): a `search-requires-acl-filter` architecture
test fails any query path that reaches the engine without a composed filter. *Permission-aware by
construction* (ADR-07) is thereby a compile-time property, not a code-review hope.

**4.2.1 Why pre-filter, not post-filter (the justification).** Post-filtering — score the top-K, then drop
the ones the viewer can't see — leaks two ways and is slow: (i) **count/ranking leakage** (result counts,
"more results" affordances, and BM25 IDF statistics reveal the existence of hidden docs even if their
bodies aren't shown); (ii) **the N+1 `check`** (one authz call per candidate result) which melts the authz
hot path under the agent/CI load the platform is built for (ADR-16). Pre-filtering at the posting-list
level eliminates both: hidden docs never enter the candidate set, never contribute to counts or IDF, and
cost one `list_objects` call per query, not one `check` per result. This is the doctrine's explicit
mandate (ADR-03; EI-04 §5 "Search … the connective tissue, easy to under-budget").

**4.2.2 Hybrid/vector and the ACL filter (the subtle part).** k-NN over HNSW returns the *approximate*
nearest neighbours; naively filtering *after* k-NN can return fewer than k visible results (the
"filtered-ANN" recall problem). Search uses **filter-during-traversal**: the ACL clause (and structured
predicates) are evaluated *as the HNSW graph is traversed*, so the k returned are the k-nearest **visible**
neighbours, not the k-nearest then filtered. This keeps recall correct under permissions and avoids a
second leak vector (a hidden doc was never a candidate). For very selective filters where graph traversal
starves, the planner falls back to **brute-force over the (small) visible set** — correct by construction.
(Prior art: filtered-ANN / ACORN-style predicate-aware traversal; the exact strategy is [OPEN → P4 Search],
the *property* — k visible neighbours, no leak — is fixed.)

**4.2.3 Consistency: zookies, fail-static, and the no-stale-grant rule (identity §8.4 / §10).** A revoked
grant must not be read stale (Zanzibar's "new enemy" problem). Search threads the **zookie**:

- A query may pass a **zookie** (e.g. the user just changed a sharing setting, read-your-writes). Search
  forwards it to `list_objects`; Id evaluates the reachable set at ≥ that snapshot. The doc's
  `indexed_zookie` (§3.1) records the authz snapshot the *doc's ACL facets* were indexed at; if a passed
  zookie is **newer** than a candidate doc's `indexed_zookie` for an ACL-relevant facet, that doc is
  **re-validated against Id** (a bounded `check` only for the affected candidates, not all) or excluded
  pending re-index — never served stale-allow.
- **Fail-static interplay (identity §10):** *default-consistency* queries (no zookie) may use the cached
  `list_objects` filter (S5) during an Id hiccup (bounded-staleness, fail-static). **Zookie-stamped queries
  bypass the cache** (they demand the named snapshot or wait/deny). So fail-static keeps *unchanged*
  authority searchable during a blip but **never re-surfaces a just-revoked doc** — security-sensitive
  changes always carry a zookie. The cache TTL ≤ the revocation SLA (W, identity §10), so a revoked grant
  ages out of every search within W. This is asserted by the zero-escape-under-staleness drill (§7, D4).

### 4.3 Full-text ranking — BM25 (DECIDED)

Default scoring is **BM25** (Robertson & Spärck Jones; Robertson et al., *Okapi at TREC*), Tantivy's
default: `score = Σ_t IDF(t) · (tf·(k1+1)) / (tf + k1·(1 - b + b·|d|/avgdl))`. Rationale: BM25 is the
proven, well-understood probabilistic baseline with strong relevance at near-zero tuning cost, and its
length-normalization (`b`) and saturation (`k1`) suit our mixed corpus (short chat messages ↔ long docs).
**Tenant/subsystem-level relevance tuning** (field boosts: title > body; recency boost for chat/issues;
exact-match boost for code) is a config layer over BM25, not a re-rank engine. **Learning-to-rank /
semantic re-rank is the named follow-on** (P4/P5), triggered by measured relevance gaps — not built v1.

### 4.4 Code search v1 (FLOOR — a Phase-4 Git input)

Per ADR-10/14 and `git-hosting §4.5`, **world-scale semantic code search is a multi-year effort and is
scoped down for v1**:

- **v1 = symbol/path/literal-grade**: index file paths, identifiers/symbols (a lightweight per-language
  tokenizer that splits on camelCase/snake_case and keeps operators), string literals, and commit
  messages, with **trigram or n-gram indexing for substring/regex-lite** code search (the
  Russ-Cox/Google-Code-Search trigram-index approach — Cox, *Regular Expression Matching with a Trigram
  Index*, 2012). This gives "find this identifier / path / literal across the repo" without an AST.
- **The input is a Git P4 deliverable**: the Git subsystem emits a `git.*` indexable projection per blob/
  ref at the granularity Search consumes; **Search does not parse repos itself** (no cross-DB read). The
  exact code-projection event (per-file, per-symbol) is owned by Git P4 (§9 required change, §10 OQ).
- **Follow-on (named):** AST-aware / cross-reference / "find usages" semantic code search, and code
  embeddings for semantic code retrieval — a scheduled later step (P4/P5), promotion-triggered by demand,
  not built now.

### 4.5 Semantic / vector search & hybrid fusion (HNSW + RRF)

- **Vector retrieval:** k-NN over the per-tenant HNSW index (§3.3), **ACL-filtered during traversal**
  (§4.2.2). Serves semantic search, **agent RAG** (an agent asks Search for the top-k *visible* passages —
  RAG is permission-correct by the same pre-filter, so an agent never retrieves a doc its delegated
  principal can't see, §4.2), and **triage/dedup** ("is this issue a near-duplicate of an existing one?").
- **Hybrid fusion:** when a query has both lexical and semantic intent, the planner runs both branches and
  fuses with **Reciprocal Rank Fusion** (Cormack et al., SIGIR 2009) — `score(d) = Σ 1/(k + rank_i(d))` —
  a robust, score-scale-free fusion that needs no per-corpus calibration. Weighted fusion / learned fusion
  is the follow-on. Both branches carry the **same ACL filter**, so fusion can never introduce a hidden doc.
- **Embeddings are produced by a swappable EU-hostable adapter** (FLOOR, §4.8) — the vector math is ours;
  the model is a strategy-pattern adapter (ADR-12.8, the same mock→real mandate as agents).

### 4.6 The query-AST compiler (ADR-07)

Search is **one compile target of the single query AST** (`myelin-query`, ADR-07; the same AST the bus's
`EventMatcher` and saved views compile — event-bus §4.5). The compiler:

1. **Validates** the AST against the field definitions (ADR-06) and the bounded-cost guard (no
   Turing-complete predicates; AG-7 / §00 §7.5) — a crafted query cannot DoS the engine.
2. **Lowers** predicates to the three shapes: text predicates → FT clauses; typed-field predicates →
   structured fast-field clauses; `semantic(...)`/`near(...)` → a vector branch; `ref_in(view)` →
   composition with `list_objects` (ADR-07's permission-by-construction).
3. **Always conjoins** `acl_clause(list_objects(viewer, read, type))` (§4.2) — there is no compiled plan
   without it.
4. **Renders back to human-readable** for the UI (one parser, one validator, one renderer — ADR-07).

Because Search shares the AST, an **agent and the UI emit the same query** — the agent-native mandate, and
the agent's query is permission-filtered identically (no agent search back-door).

### 4.7 Multilingual analysis (EU)

EU = many languages; **per-language analyzers** are mandatory (`chat.md §5.5`; `knowledge-platform §2.9`).

- **Language detection** at index time (a fast n-gram detector, e.g. CLD/whatlang-class) sets the doc's
  `lang`; an explicit source-declared language (knowledge page language, repo `.gitattributes`) overrides
  detection.
- **Per-language analyzer chain**: Unicode tokenization (UAX #29 word boundaries) → language-specific
  **stemming** (Snowball/Porter-family stemmers, e.g. German/French/Spanish/Italian/Dutch/Nordic) →
  stopword filtering → diacritic-fold for the major EU scripts. CJK and other non-segmented scripts use
  n-gram/ICU tokenization. Code and identifiers use the camel/snake tokenizer (§4.4), not a natural-language
  stemmer.
- **Query-time analyzer matches index-time analyzer per field-language**, so a German query stems like the
  German body. Mixed-language corpora are handled per-doc (the `lang` field selects the analyzer), with a
  **language-agnostic fallback** (Unicode tokenization, no stemming) for undetected text.
- The **analyzer set is a config catalogue** ([OPEN → P4]: the exact initial EU language list and the CJK
  strategy), but the **per-language-analyzer mechanism is DECIDED** here.

### 4.8 Embeddings & text are personal data — erasure (ADR-12 / GD-3 / `gdpr §6.6`)

Search is a `PersonalDataHolder` (auto-registered by the harness, §00 §3.4). It implements
`locate/export/rectify/restrict/erase`:

- **`locate(subject)`** — find every doc/field/vector referencing the subject (by `acl_object`, by
  `actor`/`assignee`/`mention` facets, by the subject's pseudonym).
- **`erase(subject)`** — **purge + re-index, not hide** (`gdpr §3.5`): delete the affected docs/fields and
  **tombstone+compact the vectors** (§3.3); where the subject's PII was inline text, the doc is removed and
  (if the artifact survives, only the person is erased) re-indexed from the source's now-tombstoned
  projection (§4.9). The DSR orchestrator gets a receipt.
- **Crypto-shred backstop**: the per-tenant index DEK (`pii_key_ref`) crypto-shreds the whole tenant index
  on tenant-decommission; for per-subject erasure, purge+reindex is primary and the key backstops backups/
  immutable segments (ADR-12.3).
- **Embeddings erased with their source.** Because the vector is in the same doc-id space (§3.2), erasing
  the doc erases the vector — there is no orphan embedding holding residual personal data (the
  `gdpr §6.6` requirement). A **model swap** (`model_ref` change) triggers a re-embed reindex; old-model
  vectors are purged in the same pass.
- **Tombstone handling**: a `*.erased` event from the bus (event-bus §4.8) is consumed like any other and
  drives the purge — so erasure reaches Search via the *same live consumer path* as everything else
  (no bespoke erasure backdoor; SEARCH-1 symmetry).

### 4.9 Reindex-from-source — the ONLY rebuild path (SEARCH-1 / SEARCH-2 / D11 / EI-04 §5.3)

**Search never reads owner databases.** On any rebuild — cold start, corruption, schema change, a new
sub-index, post-restore re-erasure, or an embedding-model swap — Search calls the bus's **reindex-from-
source re-emit protocol** (event-bus §4.9):

```
reindex(scope=(tenant|subsystem|type)):
  for each owning subsystem in scope:
     subsystem.replay(scope, since=cursor) → emits `*.snapshot` events via its outbox → the live bus
  Search's ordinary indexer (§4.1) ingests them, idempotent on event_id (snapshot ids deterministic)
```

- **One code path** for steady-state and recovery (SEARCH-1): the snapshot events go through the **same
  consumer template** as live events, so there is no separate "rebuild reader" that could drift. This is
  the doctrine's first-class resilience primitive (EI-04 §5.3; D11).
- **Budgeted up front** (SEARCH-2): the reindex cursor store (S4), the throttled/resumable replay, and the
  per-tenant in-flight caps are part of v1, not an afterthought — Search and Refs are *easy to under-budget*
  and this is the explicit counter.
- **Idempotent + resumable**: snapshot `event_id` is deterministic from `(aggregate, version)`, so a
  re-run or a mid-rebuild crash resumes from the cursor with no double-indexing.
- **Drill (D5, §7):** reindex-from-cold parity — wipe the index, `reindex`, assert the rebuilt index
  **matches the live state** (same docs, same ACL behaviour, same ranking).

This is also how a **brand-new consumer** (a new vector sub-index, a new subsystem) bootstraps: a reindex
from `since=0`. There is **no** "load the index from Postgres" backdoor (the anti-pattern SEARCH-1 forbids).

### 4.10 Caching & freshness

- **`list_objects` filter cache (S5)**: per `(tenant, subject, type, zookie-bucket)`, TTL ≤ revocation SLA,
  **never source of truth** (STOR-3), **bypassed for zookie-stamped queries** (§4.2.3). The dominant win:
  one `list_objects` for a user's session serves many queries.
- **Hot-query result cache**: bounded, zookie-bucketed, invalidated on relevant index updates; a viral
  query is request-coalesced.
- **Freshness budget**: index lag (event → searchable) is a telemetry signal with a seconds-grade p99
  budget (§4.11, §7).

### 4.11 Telemetry contract (X-1 — the Phase-5 drill survival signals)

Per §00 §10.2, Search exports on its metrics-health port (consumed by the §7 drills):

| Signal | Feeds drill |
|---|---|
| **Index lag** (event→searchable, per tenant) | freshness / near-real-time |
| **Query latency** RED, per principal-kind + tenant (FT / structured / vector / hybrid) | 30× agent-surge (human lane holds) |
| **`list_objects` call rate + cache hit ratio + filter-mode (ids vs push-down) split** | leak/no-N+1 health |
| **Zero-escape assertion counters** (zookie-bypass count, stale-served count) | zero-escape leak drill |
| **Reindex progress + cold-vs-live parity hash** | reindex-from-cold parity |
| **Erase receipts + vector-tombstone/compaction lag** | erasure-reaches-search |
| **Consumer lag** (`num_pending`) on the indexer | event-loss / head-of-line (BUS-3) |
| **Per-tenant in-flight + shed counts** | agent-surge / fairness |

---

## 5. Contracts / APIs exposed and consumed (the glue — STABLE)

Field names + units reconciled per X-5 against the foundational docs. `myelin-search` is overwhelmingly a
**consumer**; it exposes a small, stable surface. Search has **no Rust glue crate of its own** — it
consumes `myelin-query` (AST), `myelin-identity` (`list_objects`), `myelin-events` (envelope/consumer),
`myelin-gdpr` (`PersonalDataHolder`) — per overview §0 (Search owns no contract crate; it composes others').

### 5.1 Exposed

| Contract | Signature (illustrative) | Consumed by | Semantics |
|---|---|---|---|
| **query** | `query(ast: QueryAst, viewer: Principal, zookie?: Zookie, page) → RankedResults` | every subsystem search UI, CLI, **agents (RAG)** | AST compiled to FT/structured/vector; **always** composed with `list_objects(viewer, read, type)` (§4.2). No path bypasses the ACL filter. |
| **semantic** | `semantic(text\|vec, viewer, k, filter_ast?) → k visible NN` | agent RAG, dedup/triage | ACL-filtered-during-traversal k-NN (§4.2.2); k = k *visible* neighbours. |
| **declare_indexable** | `declare_indexable(IndexSpec{ subsystem, type, projection, ft_fields, struct_fields, semantic: bool, acl_object_type })` | each subsystem (build-time) | how an artifact projects to an index doc (§3.1). Search indexes **implicitly** off the bus; this declares the mapping (§5.3). |
| **reindex** | `reindex(scope) → job` | admin/ops, post-restore | invokes the bus re-emit protocol (§4.9); the only rebuild path (SEARCH-1). |
| **PersonalDataHolder** | `locate/export/rectify/restrict/erase(subject) → receipt` | DSR orchestrator | purge+reindex erasure; embeddings erased with source (§4.8). |
| **telemetry** | the §4.11 signal set | Phase-5 drills (X-1) | survival signals. |

### 5.2 Consumed (the contracts Search depends on — already defined by the foundational docs)

| Consumed contract | From | Role |
|---|---|---|
| `list_objects(subject, perm, type, zookie?) → {ids \| Filter{set_expr, zookie}}` | **Id** (identity §8.2) | the leak-free pre-filter — **the crux** (§4.2). |
| Fail-static `Consistency`/zookie semantics | **Id** (identity §8.4/§10) | no-stale-grant + degrade-not-cascade (§4.2.3). |
| `EventEnvelope` + the consumer template `events::consume(...)` | **Bus** (event-bus §3.1/§4.2) | the indexer (§4.1). |
| `reindex(scope)` re-emit + `*.snapshot` / `*.erased` events | **Bus** (event-bus §4.9/§4.8) | rebuild + erasure (§4.9/§4.8). |
| The subsystem **`ArtifactRef` projection API** | **each subsystem** (ADR-13.1) | fetch the searchable text projection (NOT the DB). |
| `QueryAst` + field definitions | **`myelin-query`** (ADR-06/07) | the query surface + structured fields (§4.6). |
| `PersonalDataHolder` + KMS/crypto-shred + `BlobStore` | **GDPR/Storage** (§00 §2.5/§2.7) | erasure + per-tenant index DEK. |
| The bootstrap harness `serve(AppSpec)` + three-surface + telemetry | **substrate** (§00 §3/§4/§10) | the service shell. |

### 5.3 The indexing contract is implicit (a subsystem emits; Search indexes)

A subsystem does **not** call a write API per change. It (a) emits domain events via its outbox (it already
does), and (b) declares its `IndexSpec` (§5.1) + implements its `ArtifactRef` projection API (ADR-13.1).
Search subscribes, fetches the projection, indexes. This keeps the no-cross-DB rule (ADR-01) and makes
reindex-from-source the natural rebuild (the projection API serves both live `*.snapshot` replay and the
per-event fetch).

---

## 6. Scaling / sharding in the cell topology (ADR-11)

### 6.1 In-cell, per-tenant, residency-pinned (ADR-11.5)

Indices are **per-tenant, in-cell, fed async off the bus** — never synchronous in the write path. The
index inherits the tenant's region (residency by construction; no cross-region index read on personal
data). The dominant scale risk is **keeping permission-filtered queries fast over large result sets** —
solved by the `list_objects` pre-filter co-designed with Id (§4.2), **not** by post-filtering (ADR-03).

### 6.2 Measure before you shard (ADR-10 / §(e) prior)

The first scaling moves, in order: (1) the `list_objects` filter cache + hot-query cache (S5);
(2) more embedded-Tantivy **index nodes per cell** (a tenant's index is a directory; route by
`(tenant, subsystem)`); (3) **per-subsystem index split** for a hot tenant (the issue index separate from
the chat index); (4) only when a *single* tenant's *single* index outgrows a node — the per-cell
**OpenSearch-class upgrade** behind the `IndexBackend` trait (§2.1), a measured-volume promotion. Premature
index sharding is its own outage (EI-02 §8).

### 6.3 Agent/CI load & fairness (ADR-16)

Agents query at machine speed (RAG, dedup). The query path runs under the **principal-aware shed lane**
(§00 §7): a human's interactive search holds the protected lane; agent/CI search sheds with `429 +
Retry-After` under pressure; **per-tenant in-flight caps** keep one tenant's agent storm off another's
humans. Bounded everything: bounded query concurrency, bounded indexer prefetch, bounded vector-search
ef-search budget. Proven by the 30× agent-surge drill (§7, D6).

### 6.4 Cross-cell federated search (FLOOR — designed-not-built)

A **multi-cell tenant** (10,000-person org spanning cells, SC-2/SC-3) needs a query to span its cells
**without** moving personal data across regions. **Named floor, not built v1.** Design seam: a
**scatter-gather** that fans the query to each of the tenant's cells, each cell runs the **same
permission-filtered query locally** (its own `list_objects`, its own index, its own residency), and a
**residency-free merge** fuses only **ranking metadata + `ArtifactRef`s** (never payload/PII) at the
control-plane boundary — the result rows are resolved per-viewer in their home cell. This rides the bus's
cross-cell pointer-event bridge (event-bus §7.4). **Follow-on owner: P4 control-plane / multi-cell tenancy
(SC-2/SC-3).** The single-cell path is complete; the §5 contracts are cell-agnostic so this extends without
a rewrite.

### 6.5 Stateful-component register & blast radius (X-4) — see §3.4

All Search state is **derived and rebuildable** (reindex-from-source). Blast radius of any index loss is
**bounded to one tenant** and **recovered by reindex** (no data loss — the source of truth is the owners +
the bus). Everything else (query nodes, the indexer workers, the compiler) is **stateless and replaceable**.

---

## 7. Failure modes + the drills owed (PROVE-IT — quantified)

Per the honesty rule (EI-01 P3; T-2/T-5), each property that can fail names the **quantified drill** that
proves it (Phase 5 executes; this is the obligation register). Each emits a **green artifact** when it
passes; until then the property is **claimed, not proven** (T-4).

| # | Failure mode | Drill (quantified gate) | Telemetry read (§4.11) | Directive/ADR |
|---|---|---|---|---|
| **D1** | **Search returns a result the viewer can't access** (the cardinal sin) | **zero-escape leak drill**: a confidential issue / overridden knowledge page / private channel / private repo file must **never** appear in any `query`/`semantic` result (incl. counts, IDF, "more results", RAG) for an unauthorized viewer. Gate: **zero leaked docs, zero count-leak** across an adversarial corpus. | zero-escape counters | ADR-03, SC-1, T-2 |
| **D2** | **Leak under staleness** (revoked grant read stale) | **zero-escape-under-staleness**: revoke a grant, immediately re-search with the post-revoke **zookie**; assert the doc is excluded (zookie reads bypass the fail-static cache, §4.2.3); assert a default-consistency search excludes it within **W ≤ revocation SLA**. Gate: **0 stale-allow with zookie; ≤ W without**. | stale-served, zookie-bypass | identity §8.4/§10, ADR-17 |
| **D3** | **Cross-tenant search (IDOR)** | **cross-tenant IDOR**: attempt a search scoped to another tenant via path-tenant spoofing; assert **zero cross-tenant docs** (tenant from token, partition key enforced). Gate: **0 cross-tenant results**. | per-tenant counters | EI-02 §1, ID-3, T-5 |
| **D4** | **Erasure doesn't reach the index** (the named structural failure) | **erasure-reaches-search**: erase a subject; assert every doc/field/**vector/embedding** is purged (not hidden) and unrecoverable (key crypto-shred backstop); assert no orphan embedding. Gate: **0 recoverable personal data post-erasure, incl. vectors**. | erase receipts, tombstone-compaction lag | ADR-12, GD-3, `gdpr §6.6`, T-5 |
| **D5** | **Index drifts from source / unrecoverable** | **reindex-from-cold parity** (SEARCH-1): wipe the index; `reindex(scope)`; assert the rebuilt index **matches live** (docs, ACL behaviour, ranking, vectors). Gate: **cold == live**; rebuild uses the live consumer path only. | reindex parity hash | SEARCH-1/2, EI-04 §5.3, T-5 |
| **D6** | **Agent search surge starves human search** | **30× agent-surge**: 30× agent/CI query surge on one tenant; assert the **human lane holds** (interactive search latency within budget), the **agent lane sheds** (429+Retry-After honoured), **other tenants unaffected**. Gate: **human-lane latency within budget; cross-tenant unaffected**. | per-tenant in-flight, shed counts | ADR-16, T-5 |
| **D7** | **Index lag breaks "I can't find what I just wrote"** | **freshness drill**: under load, assert event→searchable p99 within the seconds-grade budget; assert index lag alarms before staleness is user-visible. Gate: **freshness p99 ≤ budget**. | index lag | ADR-11.5 |
| **D8** | **Filtered-ANN recall collapse** | **vector-recall-under-filter**: a selective ACL/structured filter must still return the **k nearest *visible*** neighbours (filter-during-traversal, §4.2.2), not k-then-filter. Gate: **recall@k ≥ threshold under filter; no leak**. | vector recall, filter-mode | §4.2.2 |
| **D9** | **Post-restore resurrects erased docs** | **restore + cross-seam + re-erase** (ADR-18/STOR-4): restore the index to a consistent point with OLTP/blob/offsets; assert no resurrected erased docs (**post-restore re-erasure** runs, GD-14); assert no row↔doc↔vector mismatch. Gate: **0 resurrected personal data; cross-seam consistent**. | erase receipts | ADR-18, STOR-4, GD-14 |

---

## 8. Cited prior art

- **Inverted index / full-text.** Justin Zobel & Alistair Moffat, *Inverted Files for Text Search Engines*
  (ACM Computing Surveys, 2006); Apache Lucene's segment/DocValues architecture (the model Tantivy
  implements); Tantivy (Paul Masurel) documentation.
- **Ranking.** Stephen Robertson & Karen Spärck Jones, *Relevance Weighting of Search Terms* (1976);
  Robertson et al., *Okapi at TREC-3* (BM25, 1994); Spärck Jones, *A Statistical Interpretation of Term
  Specificity* (IDF, 1972). Hybrid fusion: Cormack, Clarke & Büttcher, *Reciprocal Rank Fusion outperforms
  Condorcet…* (SIGIR 2009).
- **Vector / ANN.** Yu. A. Malkov & D. A. Yashunin, *Efficient and robust approximate nearest neighbor
  search using Hierarchical Navigable Small World graphs* (IEEE TPAMI 2018) — HNSW; Johnson, Douze &
  Jégou, *Billion-scale similarity search with GPUs* (FAISS, 2017) — IVF-PQ as the memory-pressure upgrade;
  filtered-ANN / predicate-aware traversal (ACORN-class) for ACL-during-traversal (§4.2.2).
- **Permission-aware search / authorization.** Pang et al., *Zanzibar: Google's Consistent, Global
  Authorization System* (USENIX ATC 2019) — `list-objects`/Leopard set index + zookie consistency that
  Search's pre-filter (§4.2) and no-stale-grant (§4.2.3) consume (via Id, identity §8). SpiceDB/OpenFGA as
  the EU-self-hostable implementations.
- **Code search.** Russ Cox, *Regular Expression Matching with a Trigram Index* (Google Code Search, 2012)
  — the trigram-index substring/regex approach for code-search v1 (§4.4).
- **Multilingual analysis.** Unicode UAX #29 (text segmentation); the Snowball/Porter stemmer family;
  ICU tokenization for non-segmented scripts.
- **Stream indexing / recovery.** Jay Kreps, *The Log* (2013) — the log-as-source thesis behind
  reindex-from-source; at-least-once + idempotent (Helland, *Idempotence Is Not a Medical Condition*, 2012;
  Kleppmann, *DDIA* ch. 11) for the idempotent indexer (§4.1); the reindex-from-source primitive (EI-04
  §5.3).
- **Backpressure / fail-static.** Welsh et al., *SEDA* (SOSP 2001); Google SRE ch. 21/22 (overload,
  graceful degradation) for the shed lane (§6.3) and fail-static query path (§4.2.3) — both inherited from
  the substrate (§00 §6–§8).
- **Doctrine.** EI-02 (tenant-first/§1, outbox/§4, backpressure/§5, fail-static/§10); EI-04 §1 (erasure vs
  immutability — embeddings/text are personal data), §5 (Search/Refs easy to under-budget; reindex-from-
  source first-class). Spine ADR-03/07/10/11/12/13/16/17; directives SEARCH-1/2, X-1…X-5, BUS-3/4.

---

## 9. Required changes to foundational systems

These are **explicit asks** on the foundational docs (per the prompt's rule: state required changes, don't
re-invent contracts). None reverses a foundational decision; each is a sharpening or a confirmed dependency.

1. **Identity — confirm the `list_objects` `Filter` is compilable to a posting-list-level predicate
   (push-down), not only an enumerated id set.** Identity §8.2 already returns `{ids | Filter{set_expr,
   zookie}}`; Search depends on the `Filter` `set_expr` being expressible over **indexed facets** (project
   membership, confidential-exclusion) so it conjoins natively (§4.2). **Required:** the `set_expr` grammar
   that Id emits and Search compiles is co-designed (identity §15 already flags this `[OPEN → P4 Search]`;
   this doc commits Search to the push-down compilation and asks Id to keep `set_expr` facet-expressible,
   not opaque-id-only at scale).
2. **Event Bus — a per-artifact `*.snapshot` replay at sub-artifact granularity.** Search indexes
   sub-artifacts (a PR comment, a doc block, a CI step) and re-indexes them on reindex. **Required:** the
   `replay(scope, since)` re-emit (event-bus §4.9) must support **sub-artifact-granular** snapshots, and
   each subsystem's projection API (ADR-13.1) must resolve sub-artifact `ArtifactRef`s. (Event-bus already
   provides the protocol; this asks the *granularity* be honoured by owners — a P4 obligation, §10.)
3. **Event Bus / owners — an indexable code projection from Git (code-search v1 input).** Search does not
   read repos. **Required:** Git P4 emits a `git.*` indexable projection per blob/ref/symbol at the
   granularity §4.4 consumes (path, symbols, literals, commit message), via its outbox + projection API.
   (A Git P4 deliverable; flagged here so the seam exists.)
4. **GDPR/Storage — per-tenant index DEK in the KMS hierarchy.** Search needs a per-tenant index
   encryption key (`pii_key_ref`) whose crypto-shred destroys the tenant's whole index (§3.1/§4.8).
   **Required:** the KMS key hierarchy (GDPR §8 `[OPEN → P3]`) includes a per-tenant Search-index DEK as a
   shred unit. (Confirmation, not a new decision — ADR-12.3 already mandates per-tier crypto-shred.)
5. **Substrate — `search-requires-acl-filter` architecture lint.** Add to the §00 §2.11 lint table a check
   that no query path reaches the index engine without a composed `list_objects` filter (sibling to
   `no-raw-publish`/`tenant-predicate`). **Required:** the lint is committed to CI (E-4) so the no-leak
   property is compile-time, not review-time.

---

## 10. Open questions for Phase 4 / Phase 5

- **[OPEN → P4 Search]** The exact **`list_objects` ↔ index integration encoding** — push-down filter-clause
  shape vs enumerated id-set vs bloom-membership, and the size threshold Id uses to choose (§4.2). The
  *contract* (zookie-stamped `Filter`/`ids`, conjoined before scoring) is **frozen** (identity §8.2 + §4.2);
  the encoding is the open call.
- **[OPEN → P4 Search]** The **filtered-ANN traversal strategy** (filter-during-traversal vs brute-force
  fallback threshold; HNSW vs IVF-PQ promotion point) under selective ACL/structured filters (§4.2.2/§3.3).
- **[OPEN → P4 Search]** The **embedding model adapter** — which EU-hostable model, dimension, and the
  mock→real swap (FLOOR §4.5/§4.8). The vector machinery + erasure are decided; the model is a strategy-
  pattern adapter (ADR-12.8) chosen at runtime.
- **[OPEN → P4]** The **initial EU multilingual analyzer set** (which languages ship v1) + the CJK/
  non-segmented-script tokenization strategy (§4.7). The per-language-analyzer mechanism is decided.
- **[OPEN → P4 Git]** **Code-search v1 scope + the Git indexable-projection event** (§4.4/§9.3): the exact
  per-file/per-symbol projection, and the named follow-on to AST/cross-reference semantic code search.
- **[OPEN → P4 / control plane]** **Cross-cell federated search** for multi-cell tenants (scatter-gather +
  residency-free merge, §6.4) — inherits the bus cross-cell floor (event-bus §7.4); owner is the multi-cell
  tenancy / control-plane resolution (SC-2/SC-3).
- **[OPEN → P4/P5]** **Relevance tuning + learning-to-rank** — per-tenant/subsystem boosts over BM25 (§4.3)
  and the promotion trigger to a learned re-ranker / semantic re-rank (measured relevance gap, not v1).
- **[OPEN → P5]** All **drill thresholds** (the freshness p99 budget, the surge multiplier, recall@k under
  filter, the staleness window W's measured headroom) — proposed defaults-to-beat here; Phase 5 sets the
  numbers.

---

## 11. Cross-references

- Foundational P3 docs consumed: [`00-platform-substrate.md`](./00-platform-substrate.md) (consumer
  template, harness, fail-static, telemetry, blob trait, holder auto-registration),
  [`identity-and-access.md`](./identity-and-access.md) (`list_objects` §8.2, zookies §8.4, fail-static §10),
  [`event-bus.md`](./event-bus.md) (envelope §3.1, consumer §4.2, Signals/firehose §4.4, reindex §4.9,
  erasure/tombstone §4.8, taxonomy §6).
- Spine: ADR-03 (ReBAC `list_objects`), ADR-07 (query AST), ADR-10/14 (Search tier engine + vector),
  ADR-11 (cells/residency), ADR-12 (PersonalDataHolder/crypto-shred/embeddings-are-personal-data),
  ADR-13 (envelope/`ArtifactRef`), ADR-16 (backpressure), ADR-17 (fail-static).
- Directives: SEARCH-1 (reindex-from-source only recovery path), SEARCH-2 (budget reindex up front),
  X-1…X-5, BUS-3/BUS-4. Decision-record §(c) D11, §(e) priors.
- Doctrine: EI-02 §1/§4/§5/§10; EI-04 §1 (erasure vs immutability), §5 (Search/Refs under-budget +
  reindex-from-source).
- Sibling/consumer systems: **Refs** (the other `list_objects`-pre-filter, reindex-from-source twin —
  shared mechanism, distinct store), **Notif** (consumes search? no — both bus-driven), **Agent Fabric**
  (RAG via §5.1 `semantic`, permission-filtered), every **subsystem** (declares `IndexSpec` + projection API).
