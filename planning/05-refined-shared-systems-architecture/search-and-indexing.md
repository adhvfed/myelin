# Phase 5 — Search & Indexing (`myelin-search`) — REFINED / canonical

> Phase: `05-refined-shared-systems-architecture`. Canonical brief: [`VISION.md`](../../VISION.md)
> (single source of truth). Binding doctrine: [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md)
> and [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §5 (Search +
> reference graph are easy to under-budget; reindex-from-source is the resilience primitive).
> Reconciliation spine (this phase): [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md)
> (resolves X-1..X-7, OQ-A..OQ-L) + the refined [`contract-index.md`](./contract-index.md) (the frozen
> build-to surface; supersedes Phase 3). Carried-forward base:
> [`../03-shared-systems-architecture/search-and-indexing.md`](../03-shared-systems-architecture/search-and-indexing.md).
> Spine: **ADR-03** (ReBAC / `list_objects` pre-filter), **ADR-07** (single query AST), **ADR-10/14**
> (Search tier engine + vector), ADR-11 (cells/residency), ADR-12 (PersonalDataHolder / crypto-shred /
> embeddings-are-personal-data), ADR-13 (envelope/`ArtifactRef`), ADR-16/17 (backpressure/fail-static).
> Directives: **SEARCH-1** (reindex-from-source is the only recovery path), **SEARCH-2** (budget reindex
> up front). Date: 2026-06-19.
>
> **What this doc is.** The refined, canonical Search architecture Phase 6/7 build on. It **carries forward
> the Phase-3 design as the base** and **applies the Phase-5 reconciliation**. Where a thing is unchanged
> from Phase 3 it says so and cites the section rather than restating it. The contracts Search exposes and
> consumes are made final here, matching the refined contract index (§6 cluster + §4.3 of the index).
>
> **Status convention.** *CONFIRMED* = Phase-3 seam ratified unchanged. *SHARPENED* = the Phase-3 contract
> stood but an open encoding is now frozen concrete. *NEW* = named for the first time in Phase 5. Code
> identifiers are plain text. Snippets are illustrative signatures, not implementations.

---

## 0. Changes vs Phase 3 (the complete list)

Search was, and remains, **overwhelmingly a consumer** — it owns no contract crate, composes others'
(`myelin-query`, `myelin-identity`, `myelin-events`, `myelin-gdpr`). So most reconciliation lands on the
*shapes Search consumes*, now frozen, rather than on Search's own surface. The full delta:

1. **SHARPENED — the `list_objects` push-down encoding is now frozen (OQ-E / contract 4.3).** Phase 3 left
   "the exact push-down encoding (filter-clause shape vs id-set vs bloom membership)" as `[OPEN → P4 Search]`
   (Phase-3 §4.2, §10). Phase 5 **freezes** it: `list_objects` returns `Ids{ids, zookie}` **or**
   `Filter{set_expr, zookie}`, where `set_expr` is a structured, **consumer-composable** `SetExpr` (the
   All/None/Ids/NotIds/`InRelation{relation, via_column}`/Union/Intersect/Difference/`TupleSet{index}`
   algebra) that Search **lowers into a native query predicate / a JOIN against the per-tenant authz reverse
   index** over its own doc-id / facet space, conjoined before scoring. This closes Search's single biggest
   open question. **The `set_expr` is no longer opaque** — it is facet-/id-/tuple-expressible by contract
   (Phase-3 §9 ask #1, satisfied). See §4.2.

2. **SHARPENED — the query AST + structured field types Search compiles are now frozen byte-identical
   (X-2/X-3, OQ-C; contract 13.3).** Phase 3 compiled "the shared query AST" (ADR-07) but the grammar,
   field-type enum, and view-model were not frozen across the co-owners. Phase 5 freezes the `QueryAst`
   grammar (And/Or/Not/Cmp/In/Has/Text/Ref + Op + Literal), the `FieldType` enum, and `ViewSpec` as
   byte-identical primitives. The `Text{query, fields}` AST node is **Search's compile entry point** from a
   saved view; `rollup`/`formula` fields are **read-time-computed, never stored**, so Search indexes their
   inputs, not the derived values (a freshness consequence, see §4.6). The `EventMatcher` core is the same
   `QueryAst` — one grammar, one validator.

3. **SHARPENED — the indexable content shape is now frozen (X-2/OQ-B; contract 13.1).** The text Search
   analyses is the `myelin-content` markdown-subset inline grammar + the canonical block taxonomy, now
   frozen. The three structured inline nodes (`mention`/`artifact_ref`/`embed`) are extracted reliably (they
   are stored structured), which is what makes mention/ref **facets** dependable for the structured index.
   Code-block text is raw (not markdown-parsed) — Search tokenises it with the code tokenizer (§4.4), not a
   natural-language analyzer.

4. **SHARPENED — the measured projection-feeder promotion threshold (OQ-C / contract 6.3).** Phase 3 named
   "measured-volume promotion" loosely. Phase 5 pins the **discipline and the default-to-beat**: a custom
   facet filtered in **> 5% of a collection's view executions over a rolling window** is promoted from a
   GIN-indexed JSONB scan to a generated/columnar index. The threshold is a **Search-owned tunable, not a
   contract constant** — measured, never predicted (EI-02 §8). See §4.6.1.

5. **SHARPENED — Issues Tier-3 board-escalation valve is now unblocked (contract 6.1, CR §4).** Phase 3
   named the valve (a board query over budget compiles to a Search query); it was *blocking for Tier-3*
   because the ACL push-down shape was open. With OQ-E frozen, the valve compiles the board query to Search
   **with the same `Filter` conjoined** — Search and the Issues board now use byte-identical ACL pre-filter
   semantics. No leak, no N+1, on either path. See §4.2.4.

6. **SHARPENED — Issues `ArtifactRef` id grammar frozen `<PROJECTKEY>-<seqno>` (REF-3 reconciliation,
   contract 5.1).** The doc `doc_id` for issues is now the canonical stored key (e.g.
   `myelin://acme-eu/issue/issue/ENG-1421`); the `#1421` short form is render-time only. Search keys and
   returns the canonical id; the display projection is the UI's, never Search's. A one-line schema note
   (§3.1), no mechanism change.

7. **SHARPENED — the unified `#sub` sub-artifact grammar + content-anchoring (X-4/OQ-D; contract 5.7).**
   Search indexes sub-artifacts (a PR comment, a doc block, a CI step, a content-anchored line range). The
   `#sub` grammar Search keys on is now the one frozen vocabulary; the **doc_id may carry a `#sub`** of any
   frozen kind. Git line-ranges are **content-anchored** (BLAKE3 fingerprint), so the Search projection for a
   line-range doc is re-derived on the owner's resolve, not stored as a raw line number (§3.1, §4.9 ask).

8. **NEW (future, named) — consume CI-produced SCIP/LSIF for "find usages" (contract 6.5, CR §4 / GF-3).**
   Phase 3 scoped code-search v1 to symbol/path/literal-grade. Phase 5 **names the follow-on input**:
   semantic code search ("find usages", cross-reference) consumes a CI-produced SCIP/LSIF projection, jointly
   owned Git+CI, a later index input. Tracked in the gap report; not built v1. See §4.4.

9. **CONFIRMED — per-subject DEK granularity for indexed PII (contract 11.4).** Phase 3 used a per-tenant
   index DEK as the crypto-shred unit. Phase 5's storage reconciliation moves *source* free-text to a
   **per-subject DEK** (incl. CI log segments). Search's **primary** per-subject erasure remains
   **purge + re-index** (a real purge, not hide); the per-tenant index DEK remains the **tenant-decommission**
   shred unit and the backup/immutable-segment **backstop**. No change to Search's erase mechanism; the
   source-side per-subject DEK is an additional backstop layer (§4.8).

10. **CONFIRMED — embeddings purged with source on `*.erased` (contract 10.1 / 11.3).** Unchanged from
    Phase-3 §4.8. Restated as a one-liner: vectors live in the same doc-id space, erased with the doc; HYOK
    `can_derive_plaintext_index() = false` **structurally skips** Search indexing (no plaintext to embed).

11. **CONFIRMED — the firehose resume-cursor protocol (OQ-J) does NOT change Search.** Search consumes the
    **durable bus** (`evt.*` as an excepted infra consumer), not the firehose live tier. The OQ-J
    `subscribe/resume/scope` protocol (contract 3.5) is for KN collab / Chat presence / live boards — Search
    is unaffected. Noted explicitly so no one wires Search onto the firehose.

12. **CONFIRMED — everything else.** The Tantivy engine choice + the `IndexBackend` trait (Phase-3 §2.1),
    the three index shapes (§2.2/§3.2), BM25 (§4.3), HNSW + filter-during-traversal + RRF (§4.5),
    multilingual analyzers (§4.7), reindex-from-source as the only rebuild path (§4.9), the
    `search-requires-acl-filter` lint (now a committed substrate lint, contract 1.6), the per-tenant
    residency-pinned layout (§3.4), the shed-lane / fairness story (§6.3), and the nine drills (§7) are
    **carried forward unchanged**.

Net for Phase 6/7: Search's own exposed surface is unchanged in *shape*; what is newly **frozen** is the
encoding of the contracts it consumes (the `SetExpr` push-down, the `QueryAst`/field-type/content
primitives), which removes Search's largest Phase-3 open question and unblocks the Issues Tier-3 valve.

---

## 1. Purpose, responsibilities, and the two invariants — CONFIRMED

Unchanged from Phase-3 §1. Summary (cited, not re-derived):

- **Owns** (Phase-3 §1.1): the per-tenant residency-pinned index tier with three co-located shapes
  (full-text inverted, structured/columnar, vector/HNSW); the near-real-time incremental indexer fed off
  the bus; the permission-aware query path (the `list_objects` pre-filter — the crux); the query-AST
  compiler; embeddings-as-personal-data erasure; reindex-from-source as the only rebuild path.
- **Is NOT** (Phase-3 §1.2): not a system of record, not an authorization decision-maker (Id is; Search
  *consumes* `list_objects`), not the reference graph (Refs is), not a second query language (it speaks the
  one `QueryAst`, now frozen). It holds only derived, reconstructible state.
- **The two invariants Search must never break** (Phase-3 §1.3), both **CONFIRMED**:
  1. **Permission-aware reads everywhere** — *a user must never find what they cannot access.* A leak is
     simultaneously a security breach and a GDPR breach (SC-1). Enforced by the `list_objects` pre-filter
     composed by construction into every query (§4.2); proven by the zero-escape leak drill (§7 D1).
  2. **Erasure reaches everything** — Search is on the exhaustive `PersonalDataHolder` list; auto-registered
     by the harness (contract 1.4). Erasure **purges and re-indexes**; embeddings erased with their source
     (§4.8); proven by D4.
- **Platform non-negotiables inherited** (Phase-3 §1.4): tenant+region is the first partition key of every
  index/segment/vector/cursor/cache; no cross-tenant query path; tenant from the verified token, never the
  URL path; every store residency-pinned, per-tenant envelope-encrypted, crypto-shred-capable, a holder; no
  cross-DB read; the outbox is the only emit path (Search emits only holder receipts + telemetry).

**Floors named up front (unchanged, Phase-3 floors):** (a) semantic/vector search is **built**; the
embedding **model** is a swappable EU-hostable adapter behind a trait (the model is a P6/runtime concern,
ADR-12.8); (b) code-search v1 is symbol/path/literal-grade, fed by a Git projection — AST/cross-reference
"find usages" is the **named follow-on**, now pinned to the SCIP/LSIF input (§4.4, change #8); (c) cross-cell
federated search is **designed-not-built** (§6.4), inheriting the cross-cell PII-free pointer bridge
(contract 12.6, OQ-I) and rendering results per-viewer in their home cell.

---

## 2. Engine choice & the three index shapes — CONFIRMED

**Unchanged from Phase-3 §2** (cited, not re-litigated):

- **§2.1 — Tantivy is the in-process reference engine**, behind the `IndexBackend` trait
  (`open/upsert/delete/search/merge/snapshot`); the crux is that the ACL pre-filter compiles to a **native
  conjunctive filter clause at the posting-list level**, not a post-filter — which an embedded library makes
  first-class. OpenSearch is the reserved per-cell upgrade behind the same trait (a measured-volume
  promotion); Meilisearch/Typesense rejected as the core. Rust-native, single-binary, per-tenant index
  directories give residency + crypto-shred-per-index for free. **There is no path that bypasses the
  ACL-filter composition** (the `search-requires-acl-filter` lint, now committed — contract 1.6).
- **§2.2 — the three query shapes** compiled from the one (now frozen) `QueryAst`: full-text (BM25),
  structured/field (columnar fast-fields over the frozen `FieldType` enum), semantic/vector (HNSW k-NN). A
  single query commonly compiles to a hybrid plan, and **the ACL filter is conjoined into every branch
  before any scoring**, so no branch can surface a hidden doc.

The only Phase-5 sharpening here is upstream: the field-type enum the structured shape filters over and the
AST it compiles are now **frozen byte-identical** (X-3/OQ-C, change #2), removing the risk that a structured
predicate means something different in Search than in the Issues/Knowledge compilers.

---

## 3. Data model / schema

### 3.1 The index document — CONFIRMED, with three frozen-key notes

The canonical projection is **unchanged from Phase-3 §3.1** (the `doc_id`=`ArtifactRef` key, tenant/region
first, `acl_object`/`acl_object_type` stored explicitly for the cheap pre-filter, `indexed_zookie`+`version`
as the staleness anchor, `lang` for analyzer selection, full-text + structured `fields` + `embedding` +
GDPR routing, the whole doc envelope-encrypted with the per-tenant index DEK). Phase-5 notes that **pin**
previously-loose keys (no mechanism change):

- **Issues `doc_id` is the canonical `<PROJECTKEY>-<seqno>` key** (change #6): e.g.
  `myelin://acme-eu/issue/issue/ENG-1421`. The `#1421` short form is the UI render-time projection, never
  stored, never the Search key (REF-3 reconciliation, contract 5.1).
- **`doc_id` may carry a frozen `#sub`** (change #7): a sub-artifact doc is keyed by the unified `#sub`
  grammar (`comment-`/`thread-`/`message-`/`b<id>`/`h<id>`/`row-`/`field-`/`L<a>-L<b>`/`check-`/`step-`,
  contract 5.7). For Git **content-anchored line ranges**, Search does **not** store a raw line number —
  the searchable projection is re-derived from the owner's resolve (which returns the
  exact/rebased/partial/tombstone state), so a Search hit on a line range resolves through the owner's
  content-anchoring ladder at render time (§4.9 ask).
- **`fields` keys are the frozen `FieldType`-typed facets** (change #2): `relation`/`select`/`principal`/
  `date` etc. carry their frozen encodings; `rollup`/`formula` results are **never indexed as stored
  values** (they are read-time-computed) — Search indexes their *inputs* and the view re-computes the
  derived value at read time (§4.6). `order_key` (the LexoRank fractional index, contract 13.3) is a
  columnar fast-field for sort, byte-identical to Issues' and Knowledge's encoding.

The Phase-3 "inline-text vs reference" resolution stands (Phase-3 §3.1 tail): Search **must** hold analyzed
text to be searchable, classified `contains_personal_data` per the source event, which makes Search a true
holder whose `erase` is a real purge — resolved by crypto-shred + purge-and-reindex, not by pretending the
text isn't there.

### 3.2 The three sub-indices — CONFIRMED

Unchanged from Phase-3 §3.2: full-text inverted (term→posting list, BM25 stats per segment), structured
columnar fast-fields, vector HNSW — **all in one per-tenant index space keyed by the same `doc_id`**, so a
hybrid query fuses results that already share an ACL filter. There is no separate vector store that could
leak a doc the inverted index would have filtered.

### 3.3 The vector index detail (HNSW, erasable) — CONFIRMED

Unchanged from Phase-3 §3.3: HNSW (logarithmic-ish search, incremental insert; IVF-PQ the per-cell
memory-pressure upgrade — a measured promotion); soft-delete then compact-on-merge (critical for erasure);
**embeddings are personal data** — the vector is erased with its source doc, `model_ref` pins the adapter so
a model swap triggers a re-embed reindex (§4.9), never a silent mixed-model index.

### 3.4 Index layout, residency, and the stateful-component register — CONFIRMED

Unchanged from Phase-3 §3.4. The register S1–S5 (per-tenant FT+structured index; per-tenant vector index;
indexer dedup ledger; reindex cursor store; query/`list_objects`-filter cache) — **all derived and
rebuildable** by reindex-from-source. There is no system-of-record state in Search. Residency is enforced
because the index *directory* lives in the tenant's cell; no cross-region index read on personal data.
**Measure before you shard** — the first scaling move is more index nodes per cell + the result cache (S5),
not premature tenant-index splitting (§6).

One Phase-5 alignment note on S5: the cache key is `(tenant, region, subject, type, zookie-bucket)`; the
cached object is now a **`ListObjectsResult` (`Ids` or `Filter{set_expr}`)**, not an opaque blob — the same
zookie-bucketing and `TTL ≤ revocation SLA` rules apply (contract 4.10), and zookie-stamped queries bypass
it (§4.2.3).

---

## 4. The algorithms

### 4.1 Near-real-time incremental indexing — CONFIRMED

Unchanged from Phase-3 §4.1. The indexer is an **ordinary `myelin-events` consumer** built from the
substrate template (contract 2.4) — one of the excepted infra consumers allowed on the raw `evt.*` firehose
(it genuinely needs every domain event). It inherits mechanically: idempotent on `event_id` (dedup ledger
S3), whitelist subjects (never `*`), bind-durable-by-name, ack-after-enqueue, terminate-non-retryable to
DLQ, bounded prefetch + per-tenant in-flight caps. Per-event pipeline (idempotent, ordered per aggregate by
`(aggregate, seq)`):

```
event → (dedup? skip) → resolve projection for (subsystem, type)
      → fetch source-text projection via the owner's project(ref, viewer)/replay (NOT its DB; contract 5.6)
      → analyze (language-detect → tokenize → normalize; §4.7)
      → embed (call the embedding adapter; §4.8) if the type is semantically indexed
      → build IndexDocument (§3.1), stamp indexed_zookie + version (from event)
      → upsert into S1/S2 atomically per doc_id  → mark dedup  → ack
```

**ACL state is indexed too** (Phase-3 §4.1 tail, unchanged): a permission-change event updates the affected
docs' `indexed_zookie` (and can invalidate cached filters). Search indexes the **object**; Id computes the
**subject's reachable set at query time** — the deliberate split that avoids the N+1 at index time.

**One Phase-5 confirmation:** the projection Search fetches is the subsystem's `project(ref, viewer)`
(contract 5.6, REQUIRED on every subsystem) / its `replay` snapshot — and for sub-artifact docs it resolves
**sub-artifact `ArtifactRef`s** through the unified `#sub` resolver (contract 5.7). This is the Phase-3 §9
ask #2 (sub-artifact-granular projection), now a confirmed obligation on owners.

### 4.2 The permission-aware query pipeline — the crux — SHARPENED (OQ-E frozen)

This is the single most load-bearing mechanism (ADR-03; contract 4.3). A leak is both a security and a GDPR
breach (SC-1). The Phase-3 *mechanism* (pre-filter, never post-filter) is unchanged; the Phase-3 **open
encoding is now frozen**.

**The pipeline** (unchanged shape):

```
query(ast, viewer, zookie?):
  1. acl ← Id.list_objects(viewer, read, T, zookie?)   // → Ids{ids,zookie} | Filter{set_expr,zookie} (contract 4.3)
  2. plan  ← compile(ast)                               // §4.6 → FT / structured / vector branches
  3. plan' ← plan  ⨯  acl_clause(acl)                   // CONJOIN the ACL filter into EVERY branch
  4. hits  ← engine.search(plan')                       // posting-list-level filtering, then BM25/HNSW
  5. rank/fuse (§4.5), paginate, project, return
```

**Step 1 returns one of two frozen shapes (OQ-E, contract 4.3) and Search handles both:**

- **`Filter{set_expr, zookie}` (push-down mode — the default at scale).** `set_expr` is the **frozen
  `SetExpr` algebra**, not an opaque blob:

  ```
  SetExpr =
    | All                                    // sees everything of this type in the tenant (admin) → no clause
    | None                                   // sees nothing → engine.search short-circuits to empty (WHERE false)
    | Ids(Vec<ObjectId>)                     // small allow-set → a doc-id set membership clause
    | NotIds(Vec<ObjectId>)                  // deny-set over an otherwise-visible space
    | InRelation { relation, via_column }    // objects where doc_id is the object of <relation> for the subject
    | TupleSet { index: AuthzIndexRef }      // a server-materialised tuple set to JOIN against (the big-result path)
    | Union([SetExpr]) | Intersect([SetExpr]) | Difference(SetExpr, SetExpr)
  ```

  Search **lowers `set_expr` into the engine query** (the same lowering its `myelin-query` compiler does for
  saved-view ASTs):
  - `Ids`/`NotIds` → a **doc-id set membership** filter clause (Tantivy term-set over `doc_id`/`acl_object`).
  - `InRelation{relation, via_column}` / `TupleSet{index}` → a **JOIN/semijoin against the per-tenant authz
    reverse index** (Identity's materialised `(subject, relation, object_id)` projection, kept fresh off the
    bus). In Search's embedded model this is realised as a **co-located visible-id set** (the reverse index
    replicated/queried per cell) intersected at the posting-list level — the SpiceDB/Zanzibar
    `LookupResources` reverse index as a conjoinable filter. **One query, no N+1, no post-filter.**
  - `Union`/`Intersect`/`Difference` → the boolean composition of the above (`OR`/`AND`/`EXCEPT` over the
    filter clauses).
  - `All` → no ACL clause needed (the type-and-tenant scope already bounds it); `None` → short-circuit empty.

- **`Ids{ids, zookie}` (pre-fetch mode — small/bounded reachable sets).** A doc-id set filter directly; used
  when the set is small (a user's starred repos). Id chooses the mode on a cardinality cap; above it, Search
  gets `Filter`/`TupleSet` and JOINs.

**Step 3 is enforced structurally, not by discipline** (unchanged): `engine.search` is private; the only
public entry composes the ACL filter first. The **`search-requires-acl-filter` lint** (contract 1.6, now a
committed substrate lint) fails any query path that reaches the engine without a composed filter. *Permission-
aware by construction* is a compile-time property.

**4.2.1 Why pre-filter, not post-filter** — CONFIRMED, unchanged (Phase-3 §4.2.1): post-filtering leaks two
ways (count/ranking/IDF leakage reveals hidden docs even when bodies aren't shown; the N+1 `check` melts the
authz hot path) and is slow. Pre-filtering at the posting-list level eliminates both — hidden docs never
enter the candidate set, never contribute to counts or IDF, and cost one `list_objects` per query, not one
`check` per result.

**4.2.2 Hybrid/vector and the ACL filter** — CONFIRMED, unchanged (Phase-3 §4.2.2): **filter-during-
traversal** — the ACL clause (and structured predicates) are evaluated as the HNSW graph is traversed, so
the k returned are the k-nearest **visible** neighbours, not k-nearest-then-filtered (the filtered-ANN
recall correctness property). Very selective filters fall back to brute-force over the small visible set.
The `set_expr` lowering above is exactly the predicate fed into the traversal. (Strategy is `[OPEN → P6]`;
the *property* — k visible neighbours, no leak — is fixed; D8.)

**4.2.3 Consistency: zookies, fail-static, no-stale-grant** — CONFIRMED, unchanged (Phase-3 §4.2.3): a query
may pass a **zookie** (read-your-writes after a sharing change); Search forwards it; Id evaluates the
reachable set at ≥ that snapshot; a candidate doc whose `indexed_zookie` is older than the passed zookie for
an ACL-relevant facet is **re-validated** (a bounded `check` on the affected candidates only) or excluded
pending re-index — never served stale-allow. **Zookie-stamped queries bypass the fail-static cache**
(contract 4.10); default-consistency queries may use the cached filter during an Id hiccup (bounded
staleness ≤ revocation SLA W). One Phase-5 alignment: the authz reverse index honours a **revision
watermark** (contract 4.10) — a JOIN requiring a fresher revision than the index carries waits or falls back
to a bounded `check`. Asserted by D2.

**4.2.4 The Issues Tier-3 board-escalation valve — SHARPENED (now unblocked).** Phase 3 named the valve but
it was *blocking for Tier-3* because the push-down shape was open. With OQ-E frozen, Issues' board (when its
filtered scan goes over its OLTP budget) compiles the **board query** to a **Search `query(ast, viewer)`**
that conjoins **the same `Filter{set_expr}`** the OLTP board would have used — so the board and Search apply
byte-identical ACL pre-filter semantics. No leak, no N+1, on either tier. This is now unblocked (contract
6.1, CR §4; punch list "Search").

### 4.3 Full-text ranking — BM25 — CONFIRMED

Unchanged from Phase-3 §4.3: BM25 default (Tantivy default; the proven probabilistic baseline). Tenant/
subsystem-level relevance tuning (field boosts, recency, exact-match-for-code) is a config layer over BM25.
Learning-to-rank / semantic re-rank is the **named follow-on** (P6/P7), measured-relevance-gap-triggered,
not v1.

### 4.4 Code search v1 — FLOOR, with the SCIP/LSIF follow-on now named — SHARPENED (input pinned)

- **v1 = symbol/path/literal-grade** (unchanged from Phase-3 §4.4): file paths, identifiers/symbols (a
  camel/snake tokenizer keeping operators), string literals, commit messages, with **trigram/n-gram
  indexing** for substring/regex-lite code search (Russ Cox's trigram-index approach). Code-block text from
  `myelin-content` is **raw, not markdown-parsed** (X-2, the `code_block.text` is raw) — Search tokenises it
  with the code tokenizer, not a language stemmer.
- **The input is a Git projection** (unchanged): Git emits a `git.*` indexable projection per blob/ref/symbol
  at the granularity Search consumes (contract 6.5). Search does **not** parse repos (no cross-DB read).
- **The follow-on is now NAMED and pinned (change #8, contract 6.5):** AST-aware / cross-reference / "find
  usages" semantic code search consumes a **CI-produced SCIP/LSIF** projection (jointly Git+CI, GF-3), a
  later index input. Code embeddings for semantic code retrieval ride the same vector path. Tracked in the
  gap report; **not built v1**, promotion-triggered by demand.

### 4.5 Semantic / vector search & hybrid fusion — CONFIRMED

Unchanged from Phase-3 §4.5: vector k-NN over the per-tenant HNSW index, **ACL-filtered during traversal**
(§4.2.2), serving semantic search, **agent RAG** (an agent gets the top-k *visible* passages — RAG is
permission-correct by the same pre-filter, so an agent never retrieves a doc its delegated principal can't
see, contract 6.2), and **triage/dedup** near-duplicate detection. Hybrid lexical+semantic fuses with
**Reciprocal Rank Fusion** (score-scale-free, no per-corpus calibration); both branches carry the same ACL
filter so fusion can never introduce a hidden doc. Embeddings are produced by a **swappable EU-hostable
adapter** (the model is the strategy-pattern adapter, ADR-12.8; the vector math is ours).

### 4.6 The query-AST compiler — SHARPENED (the AST is now frozen)

Search is **one compile target of the single, now-frozen `QueryAst`** (contract 13.3; the same AST the
bus's `EventMatcher` and saved views compile, contract 3.4). The compiler (unchanged steps, Phase-3 §4.6):

1. **Validates** against the frozen `FieldType` definitions and the bounded-cost guard (no UDFs/loops/
   recursion — statically cost-bounded; a crafted query cannot DoS the engine).
2. **Lowers** predicates to the three shapes: `Text{query, fields}` → FT clauses; `Cmp`/`In`/`Has`/`Ref`
   over typed fields → structured fast-field clauses; a `semantic`/`near` request → a vector branch.
3. **Always conjoins** `acl_clause(list_objects(viewer, read, type))` (§4.2) — there is no compiled plan
   without it.
4. **Renders back to human-readable** for the UI (one parser, one validator, one renderer).

Because Search shares the AST, **an agent and the UI emit the same query**, permission-filtered identically
(no agent search back-door).

**Read-time fields (change #2 consequence).** `rollup` and `formula` fields are **computed at read time,
never stored** (contract 13.3, KN-3). Search therefore **indexes their inputs** (the relation targets, the
formula's source fields), not the derived value — a `Cmp` over a `rollup`/`formula` field compiles to a
predicate the view evaluates after fetch, or (when the input is a stored facet) to a structured clause over
the inputs. This is a deliberate freshness/consistency choice: a derived value is never a stale indexed
artifact.

#### 4.6.1 The projection-feeder promotion threshold — SHARPENED (OQ-C, measured)

A custom facet (a flexible-DB / issue custom field) is served by a **GIN-indexed JSONB scan** until it is
filtered often enough to warrant a **generated/columnar index**. Phase 5 pins the discipline and the
**default-to-beat**:

- **Promotion trigger:** a facet appearing in **> 5% of a collection's view executions over a rolling
  window** is promoted from the GIN scan to a generated index (a Tantivy fast-field / a materialised column).
- **It is a Search-owned tunable, not a contract constant** — and it is **measured, never predicted**
  (EI-02 §8): the GIN scan serves correctly meanwhile; promotion only changes cost, never correctness.
- **Owner of the frequency signal:** Issues/Knowledge emit the per-facet filter-frequency signal (CR §4);
  Search consumes it and decides promotion. This is the OQ-C tail, frozen (contract 6.3).

### 4.7 Multilingual analysis (EU) — CONFIRMED

Unchanged from Phase-3 §4.7: per-language analyzers are mandatory (EU = many languages). Language detection
at index time sets `lang` (source-declared language overrides); per-language analyzer chain (UAX #29
tokenization → Snowball/Porter-family stemming → stopwords → diacritic-fold; CJK/non-segmented via n-gram/
ICU); query-time analyzer matches index-time per field-language; code/identifiers use the camel/snake
tokenizer. The **mechanism is decided**; the exact initial EU language list + CJK strategy remains
`[OPEN → P6]` (§10).

### 4.8 Embeddings & text are personal data — erasure — CONFIRMED (+ per-subject DEK backstop)

Search is a `PersonalDataHolder` (auto-registered, contract 1.4). It implements `locate/export/rectify/
restrict/erase` exactly as Phase-3 §4.8 (unchanged mechanism):

- **`locate(subject)`** — find every doc/field/vector referencing the subject (by `acl_object`, by
  `actor`/`assignee`/`mention` facets, by the subject's pseudonym `<pseudonym>@<tenant>.noreply`, contract
  4.8).
- **`erase(subject)`** — **purge + re-index, not hide**: delete the affected docs/fields, **tombstone +
  compact the vectors** (§3.3), re-index the surviving artifact from the source's now-tombstoned projection.
  The DSR orchestrator gets a receipt.
- **`restrict(subject)`** — **suppresses indexing/agent-use/analytics/notification** for a subject pending
  erasure (contract 10.1). A restricted subject's content is not surfaced in search results or RAG. This is
  the suppression the platform erasure posture (contract 10.9, X-7) relies on for the residual it can't
  crypto-shred.
- **Crypto-shred layering (change #9, alignment):** Search's **primary** per-subject erasure is purge +
  re-index. The **per-tenant index DEK** (`pii_key_ref`) crypto-shreds the whole tenant index on
  tenant-decommission and **backstops** backups/immutable segments. The Phase-5 storage move to a
  **per-subject source DEK** (incl. CI log segments, contract 11.4) is an **additional backstop** on the
  *source* side — Search's own erase mechanism is unchanged.
- **Embeddings erased with their source** (unchanged, the `gdpr §6.6` requirement): same doc-id space, no
  orphan embedding; a model swap (`model_ref` change) triggers a re-embed reindex purging old-model vectors
  in the same pass.
- **`*.erased` tombstone** drives the purge via the **same live consumer path** as everything else — no
  bespoke erasure backdoor (SEARCH-1 symmetry).
- **HYOK structural skip:** when `can_derive_plaintext_index() = false` (contract 11.3), Search
  **structurally skips** indexing — there is no plaintext to embed or analyse, so the tenant's content is
  not in the index at all (the no-leak property holds by construction for HYOK).

### 4.9 Reindex-from-source — the ONLY rebuild path — CONFIRMED

Unchanged from Phase-3 §4.9 (SEARCH-1/2 / D11 / EI-04 §5.3). **Search never reads owner databases.** On any
rebuild — cold start, corruption, schema change, a new sub-index, post-restore re-erasure, an embedding-model
swap — Search calls the bus reindex-from-source re-emit protocol (contract 2.6):

```
reindex(scope=(tenant|subsystem|type)):
  for each owning subsystem in scope:
     subsystem.replay(scope, since=cursor) → emits `*.snapshot` via its outbox → the live bus
  Search's ordinary indexer (§4.1) ingests them, idempotent on event_id (snapshot ids deterministic)
```

One code path for steady-state and recovery; budgeted up front (the cursor store S4, throttled/resumable
replay, per-tenant in-flight caps are v1); idempotent + resumable (deterministic snapshot `event_id`); the
only consumer-bootstrap path (a new sub-index = a reindex from `since=0`). **No "load the index from
Postgres" backdoor** (the SEARCH-1 anti-pattern).

**Phase-5 ask on owners (confirmed obligation, change #7):** `replay` must be **sub-artifact-granular**
(contract 2.6) — CI one-run scope, KN page-subtree at block granularity, Git per-blob/ref — and each
subsystem's projection must resolve **sub-artifact `ArtifactRef`s** through the unified `#sub` resolver
(contract 5.7). For Git content-anchored line ranges, the projection re-derives the searchable span from the
owner's resolve (exact/rebased/partial/tombstone), so the index never holds a stale raw line number. Drill
D5 asserts cold == live parity.

### 4.10 Caching & freshness — CONFIRMED

Unchanged from Phase-3 §4.10: the `list_objects` filter cache (S5) — now caching the typed `ListObjectsResult`
(§3.4) — per `(tenant, subject, type, zookie-bucket)`, `TTL ≤ revocation SLA`, never source of truth,
bypassed for zookie-stamped queries; the hot-query result cache (bounded, zookie-bucketed, request-coalesced);
the seconds-grade p99 freshness budget (D7).

### 4.11 Telemetry contract — CONFIRMED

Unchanged from Phase-3 §4.11 (the Phase-5 drill survival signals, contract 1.8). Search exports on its
metrics-health port: index lag; query latency RED per principal-kind+tenant (FT/structured/vector/hybrid);
`list_objects` call rate + cache hit ratio + **filter-mode split (`Ids` vs `Filter`/`TupleSet`)**;
zero-escape assertion counters (zookie-bypass, stale-served); reindex progress + cold-vs-live parity hash;
erase receipts + vector-tombstone/compaction lag; consumer lag (`num_pending`); per-tenant in-flight + shed
counts.

---

## 5. Contracts exposed & consumed — final, matching the refined contract index

`myelin-search` is overwhelmingly a **consumer**; it exposes a small, stable surface and owns **no contract
crate of its own** (it composes `myelin-query`, `myelin-identity`, `myelin-events`, `myelin-gdpr`). All
field names + units align to the reconciliation anchors (the `EventEnvelope` field list + units; the
`ArtifactRef` token table).

### 5.1 Exposed (final — contract index §6 cluster)

| # | Contract | Signature (illustrative) | Consumed by | Status vs P3 |
|---|---|---|---|---|
| 6.1 | **`query`** | `query(ast: QueryAst, viewer: Principal, zookie?: Zookie, page) → RankedResults` | every search UI, CLI, **agents (RAG)**, **Issues Tier-3 board valve** | **SHARPENED** — the OQ-E `Filter` conjoin frozen; Tier-3 valve unblocked. No path bypasses the ACL filter. |
| 6.2 | **`semantic`** | `semantic(text\|vec, viewer, k, filter_ast?) → k visible NN` | agent RAG, dedup/triage | CONFIRMED — ACL-filtered-during-traversal; k = k *visible* neighbours. |
| 6.3 | **`declare_indexable`** | `declare_indexable(IndexSpec{subsystem, type, projection, ft_fields, struct_fields, semantic, acl_object_type})` | each subsystem (build-time) | **SHARPENED** — the measured projection-feeder promotion threshold (OQ-C); struct_fields are the frozen `FieldType` facets. |
| 6.4 | **`reindex`** | `reindex(scope) → job` | admin/ops, post-restore | CONFIRMED — invokes the bus re-emit protocol; the only rebuild path (SEARCH-1); sub-artifact-granular. |
| 6.5 | **code-search input** | Git emits an indexable `git.*` projection per blob/ref/symbol | Git (+ CI future) | **SHARPENED** — the SCIP/LSIF "find usages" follow-on input named. |
| (10.1) | **`PersonalDataHolder`** | `locate/export/rectify/restrict/erase(subject) → receipt` | DSR orchestrator | CONFIRMED — purge+reindex erasure; embeddings erased with source; `restrict` suppression. |
| (1.8) | **telemetry** | the §4.11 signal set | Phase-5/6 drills | CONFIRMED — survival signals (+ filter-mode split). |

### 5.2 Consumed (final — the contracts Search depends on, now frozen)

| # | Consumed contract | From | Role | Status vs P3 |
|---|---|---|---|---|
| 4.3 | `list_objects(subject, read, type, zookie?) → Ids{ids,zookie} \| Filter{set_expr,zookie}` with the frozen `SetExpr` | **Id** | the leak-free pre-filter — **the crux** (§4.2) | **SHARPENED → frozen** (OQ-E) |
| 4.2 | `check(subject, perm, object, zookie?, caveat?: CaveatContext)` | **Id** | bounded re-validation of stale candidates (§4.2.3); field-level redaction off the hot path | CONFIRMED (+ `CaveatContext` available) |
| 4.10 | `Consistency`/zookie semantics + authz reverse-index revision watermark | **Id** | no-stale-grant + degrade-not-cascade (§4.2.3) | CONFIRMED |
| 2.1/2.4 | `EventEnvelope` + the consumer template | **Bus** | the indexer (§4.1) | CONFIRMED |
| 2.6 | `reindex(scope)` re-emit + `*.snapshot`/`*.erased`, **sub-artifact-granular** | **Bus** | rebuild + erasure (§4.9/§4.8) | CONFIRMED (granularity obligation) |
| 5.6/5.7 | subsystem `project(ref, viewer)` + the unified `#sub` resolver | **each subsystem** | fetch the searchable projection (NOT the DB); resolve sub-artifact refs | **SHARPENED** (`#sub` frozen) |
| 13.1/13.3 | `myelin-content` taxonomy + `QueryAst`/`FieldType`/`ViewSpec`/`order_key` | **`myelin-content`/`myelin-query`** | the analyzable text + the query/structured surface | **SHARPENED → frozen** (X-2/X-3) |
| 10.1/11.3 | `PersonalDataHolder` + KMS/crypto-shred (per-tenant index DEK + per-subject source DEK backstop) + `BlobStore` | **GDPR/Storage** | erasure + index encryption | CONFIRMED (per-subject backstop) |
| 1.1/1.2/1.8 | `serve(AppSpec)` + three-surface + telemetry | **substrate** | the service shell | CONFIRMED |
| 1.6 | the `search-requires-acl-filter` lint | **substrate/CI** | compile-time no-leak | CONFIRMED (committed) |

### 5.3 The indexing contract is implicit — CONFIRMED

Unchanged from Phase-3 §5.3: a subsystem does **not** call a write API per change. It (a) emits domain
events via its outbox, and (b) declares its `IndexSpec` + implements its `project(ref, viewer)` projection
(contract 5.6). Search subscribes, fetches the projection, indexes. This keeps the no-cross-DB rule and
makes reindex-from-source the natural rebuild (the projection serves both live `*.snapshot` replay and the
per-event fetch).

---

## 6. Scaling / sharding in the cell topology — CONFIRMED

Unchanged from Phase-3 §6:

- **§6.1** In-cell, per-tenant, residency-pinned, fed async off the bus. The dominant scale risk is keeping
  permission-filtered queries fast over large result sets — solved by the `list_objects` pre-filter
  (now frozen, §4.2), not post-filtering.
- **§6.2** Measure before you shard: (1) the filter/result cache (S5); (2) more embedded-Tantivy index nodes
  per cell; (3) per-subsystem index split for a hot tenant; (4) only then the per-cell OpenSearch-class
  upgrade behind the `IndexBackend` trait — a measured-volume promotion. Premature sharding is its own
  outage.
- **§6.3** Agent/CI load & fairness: the query path runs under the **principal-aware shed lane** (contract
  1.11) — a human's interactive search holds the protected lane; agent/CI search sheds with `429 +
  Retry-After`; per-tenant in-flight caps keep one tenant's agent storm off another's humans. Bounded
  everything (query concurrency, indexer prefetch, vector ef-search budget). Proven by D6. (The Phase-5
  per-surface shed-budget floors, OQ-K, name "every surface bounded + a reserved human lane + the shed
  order" — Search's query surface is one of them; concrete numbers are a P6 budget call tuned by drills.)
- **§6.4** Cross-cell federated search — **designed-not-built** (FLOOR): scatter-gather, each cell runs the
  same permission-filtered query locally over its own index/`list_objects`/residency, a residency-free merge
  fuses only ranking metadata + `ArtifactRef`s (never payload/PII) at the control-plane boundary; result
  rows resolved **per-viewer in their home cell** — riding the cross-cell PII-free pointer bridge (contract
  12.6, OQ-I; resolution always cell-local). The single-cell path is complete; the §5 contracts are
  cell-agnostic so this extends without a rewrite. Owner: P6 control-plane / multi-cell tenancy.
- **§6.5** All Search state is **derived and rebuildable**; blast radius of any index loss is bounded to one
  tenant and recovered by reindex; everything else is stateless and replaceable.

---

## 7. Failure modes + the drills owed — CONFIRMED

Unchanged from Phase-3 §7 (the obligation register; Phase 6 executes; each emits a green artifact when it
passes). The nine drills, carried forward verbatim in intent:

| # | Failure mode | Drill (quantified gate) |
|---|---|---|
| **D1** | Search returns a result the viewer can't access (the cardinal sin) | **zero-escape leak drill**: a confidential issue/overridden page/private channel/private repo file must **never** appear in any `query`/`semantic` result (incl. counts, IDF, "more results", RAG) for an unauthorized viewer. Gate: **0 leaked docs, 0 count-leak** across an adversarial corpus. |
| **D2** | Leak under staleness (revoked grant read stale) | **zero-escape-under-staleness**: revoke, re-search with the post-revoke **zookie** → excluded (zookie bypasses fail-static + honours the reverse-index revision watermark); default-consistency excludes within **W ≤ revocation SLA**. Gate: **0 stale-allow with zookie; ≤ W without**. |
| **D3** | Cross-tenant search (IDOR) | **cross-tenant IDOR**: spoof path-tenant; assert **0 cross-tenant docs** (tenant from token, partition key enforced). |
| **D4** | Erasure doesn't reach the index (the named structural failure) | **erasure-reaches-search**: erase a subject; assert every doc/field/**vector/embedding** purged (not hidden) and unrecoverable (per-tenant + per-subject DEK backstop); no orphan embedding. Gate: **0 recoverable personal data post-erasure, incl. vectors**. |
| **D5** | Index drifts from source / unrecoverable | **reindex-from-cold parity** (SEARCH-1): wipe; `reindex`; rebuilt index **== live** (docs, ACL behaviour, ranking, vectors, sub-artifact granularity); rebuild uses the live consumer path only. |
| **D6** | Agent search surge starves human search | **30× agent-surge**: human lane holds (interactive latency within budget), agent lane sheds (`429+Retry-After` honoured), other tenants unaffected. |
| **D7** | Index lag breaks "I can't find what I just wrote" | **freshness drill**: event→searchable p99 within the seconds-grade budget; lag alarms before user-visible. |
| **D8** | Filtered-ANN recall collapse | **vector-recall-under-filter**: a selective ACL/structured filter still returns the **k nearest *visible*** neighbours (filter-during-traversal). Gate: **recall@k ≥ threshold under filter; no leak**. |
| **D9** | Post-restore resurrects erased docs | **restore + cross-seam + re-erase** (ADR-18): restore to a consistent point with OLTP/blob/offsets; assert no resurrected erased docs (post-restore re-erasure runs, contract 10.8); no row↔doc↔vector mismatch. |

---

## 8. Cited prior art — CONFIRMED

Unchanged from Phase-3 §8 (cited, not restated):

- **Inverted index / full-text:** Zobel & Moffat (ACM CSUR 2006); Lucene segment/DocValues; Tantivy.
- **Ranking:** Robertson & Spärck Jones (1976); Robertson et al. *Okapi at TREC-3* (BM25); Spärck Jones
  (IDF, 1972); Cormack, Clarke & Büttcher (RRF, SIGIR 2009).
- **Vector / ANN:** Malkov & Yashunin (HNSW, IEEE TPAMI 2018); Johnson, Douze & Jégou (FAISS, IVF-PQ);
  filtered-ANN / predicate-aware traversal (ACORN-class) for ACL-during-traversal.
- **Permission-aware search / authorization:** Pang et al., *Zanzibar* (USENIX ATC 2019) — `list-objects` /
  Leopard set index + zookie consistency that Search's pre-filter and no-stale-grant consume via Id; the
  **reverse index / `LookupResources`** pattern is the OQ-E `TupleSet`/`InRelation` JOIN target. SpiceDB/
  OpenFGA as the EU-self-hostable implementations.
- **Code search:** Russ Cox, *Regular Expression Matching with a Trigram Index* (2012) — code-search v1; the
  SCIP/LSIF follow-on (change #8) adopts the Sourcegraph/Microsoft code-intelligence index formats.
- **Multilingual:** Unicode UAX #29; Snowball/Porter; ICU tokenization.
- **Stream indexing / recovery:** Kreps, *The Log* (2013); Helland, *Idempotence Is Not a Medical Condition*
  (2012); Kleppmann, *DDIA* ch. 11; the reindex-from-source primitive (EI-04 §5.3).
- **Backpressure / fail-static:** Welsh et al., *SEDA* (SOSP 2001); Google SRE ch. 21/22.
- **Doctrine:** EI-02 §1/§4/§5/§8/§10; EI-04 §1 (embeddings/text are personal data), §5 (Search/Refs easy
  to under-budget; reindex-from-source first-class). Spine ADR-03/07/10/11/12/13/16/17.

---

## 9. Required changes to foundational systems — RESOLVED (the Phase-3 asks, now closed)

The five Phase-3 §9 asks, with their Phase-5 disposition:

1. **Id — `list_objects` `Filter` compilable to a posting-list-level predicate, not opaque-id-only.**
   **RESOLVED (OQ-E, contract 4.3):** `set_expr` is the frozen, consumer-composable `SetExpr` algebra (facet-/
   id-/tuple-expressible), lowered to a native filter / a JOIN against the per-tenant authz reverse index.
   No longer open.
2. **Bus — sub-artifact-granular `*.snapshot` replay.** **RESOLVED (contract 2.6):** `replay(scope, since)`
   is sub-artifact-granular; each subsystem's projection resolves sub-artifact `ArtifactRef`s via the unified
   `#sub` resolver (contract 5.7). A confirmed owner obligation.
3. **Git — an indexable code projection (code-search v1 input).** **CONFIRMED seam (contract 6.5):** Git
   emits the `git.*` per-blob/ref/symbol projection; the SCIP/LSIF "find usages" follow-on input is now named
   (change #8). A Git/CI deliverable in 5-B.
4. **GDPR/Storage — per-tenant index DEK in the KMS hierarchy.** **CONFIRMED (contract 11.3):** the per-tenant
   index DEK is a shred unit; the per-subject source DEK (contract 11.4) is an added backstop. No new
   decision needed.
5. **Substrate — the `search-requires-acl-filter` lint.** **CONFIRMED committed (contract 1.6):** in the lint
   table; the no-leak property is compile-time.

---

## 10. Open questions remaining for Phase 6 — honesty register

Search's single biggest Phase-3 open question (the `list_objects`↔index integration encoding) is **closed**
by OQ-E. What remains is genuinely Phase-6 build/tuning, not architecture:

- **[OPEN → P6 Search]** The **filtered-ANN traversal strategy** under selective ACL/structured filters
  (filter-during-traversal vs brute-force fallback threshold; HNSW vs IVF-PQ promotion point, §4.2.2/§3.3).
  The *property* (k visible neighbours, no leak) is fixed; the strategy is the open call. (D8.)
- **[OPEN → P6 Search]** The **embedding model adapter** — which EU-hostable model, dimension, the mock→real
  swap (FLOOR §4.5/§4.8). The vector machinery + erasure are decided; the model is a strategy-pattern adapter
  (ADR-12.8) chosen at runtime.
- **[OPEN → P6]** The **initial EU multilingual analyzer set** (which languages ship v1) + the CJK/
  non-segmented-script tokenization strategy (§4.7). The per-language mechanism is decided.
- **[OPEN → P6 Git/CI]** **Code-search v1 scope + the SCIP/LSIF "find usages" input** (§4.4, contract 6.5):
  the exact per-file/per-symbol projection event, and the AST/cross-reference follow-on. A Git+CI joint
  deliverable; named in the gap report.
- **[OPEN → P6 / control plane]** **Cross-cell federated search** for multi-cell tenants (scatter-gather +
  residency-free merge, §6.4) — designed-not-built, inheriting the cross-cell PII-free pointer bridge
  (contract 12.6, OQ-I). Owner: multi-cell tenancy / control-plane.
- **[OPEN → P6/P7]** **Relevance tuning + learning-to-rank** — per-tenant/subsystem boosts over BM25 (§4.3)
  and the promotion trigger to a learned re-ranker (measured relevance gap, not v1).
- **[MEASURED, not predicted — P6 tunes]** The **projection-feeder promotion threshold** (default-to-beat
  > 5% of view executions, §4.6.1, OQ-C — a Search-owned tunable, not a contract constant), and **all drill
  thresholds** (the freshness p99 budget, the surge multiplier, recall@k under filter, the staleness window
  W's measured headroom, the per-surface query shed budget OQ-K). Phase-3 proposed defaults-to-beat; Phase 6
  sets the numbers from measurement (EI-02 §8).

No `[OPEN — LEGAL]` item is owned by Search: embeddings-are-personal-data is settled (erased with source);
the free-text/immutable erasure residual (X-7, contract 10.9) is the platform posture Search satisfies by
**purge + re-index + `restrict` suppression**, instantiated by reference, not restated here.

---

## 11. Cross-references

- Reconciliation spine: [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md) (OQ-E the
  `SetExpr` push-down; OQ-C the promotion threshold; X-2/X-3 the content/query primitives; X-7 the erasure
  posture); refined [`contract-index.md`](./contract-index.md) (§4 Identity, §6 Search, §13 shared crates).
- Phase-3 base (carried forward): [`../03-shared-systems-architecture/search-and-indexing.md`](../03-shared-systems-architecture/search-and-indexing.md);
  consumed foundational docs [`identity-and-access.md`](../03-shared-systems-architecture/identity-and-access.md)
  (`list_objects`), [`event-bus.md`](../03-shared-systems-architecture/event-bus.md) (envelope/consumer/
  reindex), [`00-platform-substrate.md`](../03-shared-systems-architecture/00-platform-substrate.md)
  (harness/lints/fail-static).
- Change requests (primary input): [`../04-subsystem-architectures/cross-subsystem-change-requests.md`](../04-subsystem-architectures/cross-subsystem-change-requests.md)
  (§4 Search; X-2/X-3; OQ-C/OQ-E).
- Spine: [`../02-holistic-architecture/architecture-decisions.md`](../02-holistic-architecture/architecture-decisions.md)
  (ADR-03/07/10/11/12/13/16/17); [`../02b-doctrine-integration/integration-directives.md`](../02b-doctrine-integration/integration-directives.md)
  (SEARCH-1/2).
- Doctrine: [`../../external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md),
  [`../../external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §5.
- Sibling systems: **Refs** (the other `list_objects`-pre-filter / reindex-from-source twin — shared
  mechanism, distinct store), **Identity** (the `SetExpr` push-down + authz reverse index Search JOINs
  against), **Agent Fabric** (RAG via `semantic`, permission-filtered), every **subsystem** (declares
  `IndexSpec` + the `project` projection).
