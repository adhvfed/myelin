# Phase 7 — Prompt Ledger: Search & Indexing (myelin-search)

> Phase: 07-prompts (per-system file, Phase 7-A). The complete ordered set of implementation prompts that
> operationalize the entire search-and-indexing roadmap (planning/06-roadmaps/shared/search-and-indexing.md,
> milestones S-M0..S-M6) into clean-context, independently-committable coding tasks. Built to the template in
> planning/07-prompts/00-ledger-overview.md §2 (every field present, never implicit) and banded to
> planning/06-roadmaps/00-master-sequencing.md §2 (M0..M6, the gate invariant). Frozen architecture (this file
> OPERATIONALIZES, it does not redesign): planning/05-refined-shared-systems-architecture/search-and-indexing.md
> + contract-index.md §6 (owned) + §4/§5/§13 (consumed) + 00-reconciliation-decisions.md (OQ-E the SetExpr
> push-down, OQ-C the promotion threshold, X-2/X-3 the content/query primitives, X-7 the erasure posture,
> OQ-I/OQ-J/OQ-K). Plain-text identifiers throughout (no backticks-as-emphasis). Markdown only; this file makes
> no commits. Date: 2026-06-19.
>
> The global P-NNN ids are assigned by the consolidated ledger index (Phase 7-B, 01-ledger-index.md) when these
> per-system prompts are interleaved into the single execution order. Here each prompt carries a stable local
> handle SRCH-P<n> so its DEPENDS-ON edges are unambiguous before global numbering; the index rewrites SRCH-P<n>
> to its P-NNN. Where a prompt depends on another system's prompt not yet numbered, it names that system's
> milestone (the index resolves it to the P-NNN).
>
> The shape of this system: Search is overwhelmingly a CONSUMER. It owns no contract crate (it composes
> myelin-query, myelin-identity, myelin-events, myelin-gdpr, myelin-content) and holds only derived,
> reconstructible state. Its entire correctness story is downstream of Identity 4.3 (the list_objects SetExpr
> push-down). So its core build is one band (M2) but it splits into seven independently-gateable slices, each
> with its own green gate; M3/M4 light up real producer corpora over the unchanged engine; M5 is world-scale +
> the floor follow-ons + the E2E wedge.
>
> Coverage: S-M0 → SRCH-P1; S-M1 → SRCH-P2; S-M2 → SRCH-P3..SRCH-P9 (seven slices, each its own green gate);
> S-M3 → SRCH-P10; S-M4 → SRCH-P11; S-M5 → SRCH-P12..SRCH-P14; S-M6 → SRCH-P15. Fifteen prompts, no milestone
> gap.

---

### SRCH-P1 — Ship the search-requires-acl-filter lint (red+green fixtures) + anchor the index-doc names

- **BAND.** M0.
- **ROADMAP MILESTONE.** S-M0 (planning/06-roadmaps/shared/search-and-indexing.md §2 "S-M0 — The Search ratchet").
- **DEPENDS-ON.** The M0 substrate prompts that lay down the Cargo workspace + the eight glue-crate skeletons,
  the lint framework + the contract-coverage scanner (master §2 M0; substrate roadmap SUB-M0), and the Bus M0
  prompt that freezes the EventEnvelope (2.1) + the ArtifactRef token table. The index slots SRCH-P1 after those
  workspace-bootstrap + lint-framework prompts.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md (always) §1 (search as shared connective tissue), §3 (name-your-floors,
    code-wins-over-docs); ../../external-insights/01-process-and-quality-doctrine.md §5 (the ratchet / committed
    gates — an uncommitted lint is no lint; loud-never-swallowed, no "... || true"), §3 (prove-it: a lint with
    only one fixture is not proven to reject).
  - Architecture: ../05-refined-shared-systems-architecture/search-and-indexing.md §2.1 (the only public query
    entry composes the ACL filter first; engine.search is private; there is no path that bypasses the
    composition), §4.2 step 3 (the lint enforces compose-first structurally, not by discipline), §3.1 (the index
    document key/unit anchors — doc_id = the ArtifactRef key, tenant/region first, indexed_zookie + version the
    staleness anchor).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 1.6 (the twelve architecture lints
    — search-requires-acl-filter is Search's row), 2.1 (EventEnvelope — the names/units anchor), 5.1 (ArtifactRef
    — the doc_id key).
  - Roadmap: planning/06-roadmaps/shared/search-and-indexing.md §2 S-M0 (the work + the gate) and §1.1 row (1.8),
    §0 (the one-paragraph band map: Search is named in M0 by its lint).
- **DELIVERABLE (what to build + exactly where in the repo).** In the workspace's architecture-lint crate (the
  M0 lint framework) + a doc-anchor note in the myelin-search service crate skeleton:
  - The search-requires-acl-filter lint: a compile-time check that fails any code path which reaches the private
    engine.search entry without a composed ACL filter clause (the list_objects Filter / Ids conjoin). The lint
    exists BEFORE the query path it guards (SRCH-P5), so the path can never be written without it. Make
    engine.search non-public by construction and require the public query entry to take the composed filter — the
    lint enforces "permission-aware by construction" as a compile-time property.
  - The red-fixture: a query path that calls engine.search without a composed ACL filter — the lint MUST reject
    it. The green-fixture: a path that conjoins the filter first — the lint MUST admit it. Both ship in M0 per the
    contract; a lint with only the green fixture is the floor, the red fixture is the follow-on, both here.
  - Wire the lint into CI, loud, never "... || true" — a swallowed lint is no lint (EI-01 §5).
  - Anchor the index document's field/unit names to the frozen EventEnvelope (2.1) + the ArtifactRef token table:
    doc_id = the ArtifactRef key, tenant/region first, indexed_zookie + version the staleness anchor, lang for
    analyzer selection, the GDPR routing fields (contains_personal_data/data_role/visibility/pii_key_ref). NO
    mechanism — just the names, written into the myelin-search crate's module doc, so the S-M2 build does not
    drift from the envelope.
  - FLOOR named: none — this is a ratchet, not a feature. State in the module doc that the query path the lint
    guards is the S-M2 follow-on (SRCH-P5), so the lint is not mistaken for a working query engine.
- **CONTRACTS TO IMPLEMENT.** 1.6 the search-requires-acl-filter lint (wired as a permanent ratchet gate; say
  so). Consumed-as-anchor: 2.1 EventEnvelope + 5.1 ArtifactRef (the names/units the index doc aligns to; Search
  is a name-consumer here, not an author). Implement the lint to the frozen contract — a needed shape change is a
  whole-workspace contract PR, escalated and written down, not a local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - search-requires-acl-filter green with BOTH fixtures: the red-fixture proves it rejects an unfiltered query
    path (1 rejection), the green-fixture proves it admits a filtered one (0 false reject). Wired into CI, loud,
    never "|| true" — CI (this is a permanent ratchet gate; it ships in the M0→M1 boundary's "all 12 lints green
    w/ fixtures" clause).
  - The contract-coverage scanner passes on the Search 1.6 row — CI.
- **TESTS (required).** The red+green lint fixtures (the lint proven to reject the bypass path and admit the
  composed path). A unit test asserting the index-doc anchor names match the frozen EventEnvelope field list
  (doc_id/tenant/region/indexed_zookie/version/lang + the GDPR routing fields) — a names/units drift test, so a
  later rename of an envelope field breaks this test now, not in prod (EI-01 §7, reconcile names/units up front).
  No CDC pair (Search owns no contract crate at M0; the lint is the deliverable). State that no runtime drill
  greens here — the engine is S-M2 — honestly.
- **DEFINITION OF DONE.** The search-requires-acl-filter lint compiles and is wired into CI with both fixtures
  emitting a dated green artifact (rejects the bypass, admits the composed path); the index-doc anchor names
  match the frozen envelope (the drift test passes); the contract-coverage scanner is green on the Search 1.6
  row; the floor note (the query path is SRCH-P5) is written in the crate module doc; the work is committed. No
  gate is greened by weakening a threshold or inverting an assertion.
- **COMMIT.** Header: P-<NNN> M0: search-requires-acl-filter lint + index-doc name anchors. Body lists: the
  search-requires-acl-filter lint (1.6) greened with red+green fixtures (a permanent ratchet); the index-doc
  names anchored to the frozen EventEnvelope; the floor named (query path follow-on SRCH-P5). Branch first if on
  default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### SRCH-P2 — Register Search as a PersonalDataHolder + pin the per-tenant index DEK + confirm residency

- **BAND.** M1.
- **ROADMAP MILESTONE.** S-M1 (planning/06-roadmaps/shared/search-and-indexing.md §2 "S-M1 — Search as a holder
  + the index encryption floor").
- **DEPENDS-ON.** SRCH-P1 (the myelin-search crate skeleton + the lint exist). The M1 Identity/Storage/GDPR
  prompts that ship the holder harness auto-registration (contract 1.4), the KMS hierarchy (11.3/11.4), and the
  residency-pin (tenancy 12.x) — Search registers into those. The index places this after those M1 substrate
  prompts.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe & EU-sovereign by construction; name-your-floors);
    ../../external-insights/04-hard-problems.md §5 (embeddings/text are personal data; Search easy to
    under-budget); ../../external-insights/01-process-and-quality-doctrine.md §1 (name-your-floors).
  - Architecture: ../05-refined-shared-systems-architecture/search-and-indexing.md §1 ("Floors named up front",
    Search is a true holder whose erase is a real purge), §3.4 (index layout, residency: the index directory
    lives in the tenant's cell, no cross-region index read on personal data), §4.8 (the crypto-shred layering:
    the per-tenant index DEK is the tenant-decommission shred unit + the backup/immutable-segment backstop; the
    per-subject source DEK 11.4 is an added source-side backstop; the PRIMARY per-subject erasure is purge +
    reindex, landing in S-M2).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md change #9 (the
    per-subject DEK granularity for indexed PII), X-7 (the one free-text/immutable erasure residual posture —
    Search satisfies it by purge + reindex + restrict, adds no new residual).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 10.1 (PersonalDataHolder{locate/
    export/rectify/restrict/erase}; exhaustive H1–H18 list; harness auto-registers), 1.4 (the harness holder
    auto-registration), 11.3 (KMS hierarchy + KeyOrigin; can_derive_plaintext_index; destroy), 11.4 (crypto-shred
    + per-subject DEK granularity), 10.9 (the one residual posture).
  - Roadmap: planning/06-roadmaps/shared/search-and-indexing.md §2 S-M1 (the work + the gate) + §1.1 row (10.1) +
    §1.2 rows 10.1/11.3/11.4.
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-search service crate:
  - Register Search as a PersonalDataHolder via the harness auto-registration (1.4) so the H1–H18 holder list is
    EXHAUSTIVE before any real tenant data exists (10.1). At M1 the holder is a STUB — there is no index to purge
    yet — but it is on the list, so the M5 DSAR fan-out cannot silently miss it. Implement the holder trait
    surface (locate/export/rectify/restrict/erase) against the future index, returning empty-but-correct now.
  - Pin the per-tenant index DEK into the KMS hierarchy (11.3): per-cell root → per-tenant KEK → per-tenant index
    DEK as the tenant-decommission crypto-shred unit and the backup/immutable-segment backstop; reserve the
    per-subject source DEK (11.4) as the added source-side backstop. No index exists yet — this RESERVES the key
    class so the S-M2 index is encrypted-from-birth, and confirms destroy is callable on the key class.
  - Confirm the residency-pin applies to the (future) per-tenant index directory: the index lives in the tenant's
    cell; no cross-region index read on personal data (§3.4). The residency-pin + tenant-predicate lints (M0)
    already enforce it structurally — assert the Search crate links them.
  - FLOOR named: the per-tenant index DEK (the crypto-shred + backup-backstop unit) is the floor; the PRIMARY
    per-subject erasure by purge + reindex is the follow-on, landing in SRCH-P9 once the index exists. Write this
    so the index DEK is not mistaken for the whole erasure answer. State that Search instantiates the one platform
    residual posture (10.9 / X-7) BY REFERENCE and adds NO new [OPEN — LEGAL] residual.
- **CONTRACTS TO IMPLEMENT.** 10.1 PersonalDataHolder (owned by Search as a holder; stub surface now, real erase
  in SRCH-P9) — wired to the harness 1.4 auto-registration. Consumed: 11.3/11.4 (the KMS index-DEK key class),
  the residency-pin (tenancy). To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - Search appears in the harness-generated holder registry — 0 stores unregistered; the contract-coverage
    scanner confirms 10.1 coverage — CI (structural).
  - The per-tenant index DEK is a destroyable key in the KMS hierarchy: the key class exists and destroy is
    callable (proven fully later by SRCH-D4/D9 in SRCH-P9/SRCH-P13; here the check is structural) — CI
    (structural).
  - Search's M1 work does not begin its M2 engine over a red STOR-D1/STOR-D2 (restore-verify, the silent-data-
    loss floor), ID-D3 (cross-tenant 0), ID-D2 (fail-static), ID-D1 (disabled-user N=5 min), CP-D2/CP-D3
    (misroute + residency-pin): name these inherited M1 platform gates as the precondition for SRCH-P3. Search
    does not re-prove them; it cannot build the index over a red STOR-D1 — DEPENDS-ON makes this concrete.
- **TESTS (required).** Unit test: the holder stub surface returns empty-but-correct locate/export for a tenant
  with no index. A structural test asserting Search is in the holder registry and the per-tenant index DEK class
  is destroyable. The provider+consumer CDC pair for the Search side of 10.1. No drill greens here (the engine is
  S-M2) — record this surface as untested-at-runtime-but-named (the real erase is SRCH-P9), honestly.
- **DEFINITION OF DONE.** Search is registered as an exhaustive-list holder (0 unregistered); the per-tenant
  index DEK class is reserved + destroyable; residency-pin confirmed structurally; the floor (DEK now;
  purge+reindex erasure in SRCH-P9) is named in writing; the no-new-residual posture is recorded; the holder CDC
  pair passes; committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M1: Search PersonalDataHolder registration + per-tenant index DEK + residency-pin.
  Body lists: 10.1 holder stub registered (exhaustive-list); the per-tenant index DEK class reserved/destroyable;
  the floor named (purge+reindex erasure follow-on SRCH-P9); no new [OPEN — LEGAL] residual. Branch first; do not
  push unless asked. Co-Authored-By trailer.

---

### SRCH-P3 — The engine + three index shapes behind the IndexBackend trait (Tantivy, encrypted-from-birth)

- **BAND.** M2.
- **ROADMAP MILESTONE.** S-M2 slice 1 of 7 (planning/06-roadmaps/shared/search-and-indexing.md §2 "S-M2 — The
  Search core", the engine + three-index-shape deliverable).
- **DEPENDS-ON.** SRCH-P1 (the lint + name anchors), SRCH-P2 (the holder + the per-tenant index DEK). M1 fully
  green (Identity 4.3/4.2/4.10, KMS 11.3/11.4, STOR-D1/D2, CP-D2/D3); M0 the harness serve(AppSpec) + the
  failure-injection harness. The frozen myelin-content/myelin-query crates (13.1/13.3) — the FieldType enum the
  structured shape is typed over. The index resolves these to their P-NNN.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scalable from day 1; Rust default); ../../external-insights/04-hard-problems.md §5
    (Search/Refs easy to under-budget); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it;
    observability is part of the pass).
  - Architecture: ../05-refined-shared-systems-architecture/search-and-indexing.md §2.1 (Tantivy in-process
    behind the IndexBackend trait open/upsert/delete/search/merge/snapshot; the ACL filter compiles to a native
    posting-list-level conjunctive clause; OpenSearch the reserved per-cell upgrade behind the same trait), §2.2
    (the three query shapes), §3.1 (the index document), §3.2 (the three sub-indices in one per-tenant space
    keyed by one doc_id), §3.3 (the HNSW vector index, soft-delete then compact-on-merge), §3.4 (the per-tenant
    residency-pinned layout + the S1–S5 stateful-component register).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 1.1 (serve(AppSpec) — the service
    shell; Search is an AppSpec + handlers, not a hand-rolled main), 11.3 (the per-tenant index DEK
    encrypted-from-birth), 13.3 (the FieldType enum the structured fast-fields are typed over), 13.1
    (myelin-content taxonomy — the analyzable text shape).
  - Roadmap: planning/06-roadmaps/shared/search-and-indexing.md §2 S-M2 (the engine + three index shapes bullet)
    + §1.1 rows 6.1/6.2.
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-search service crate:
  - The IndexBackend trait (open/upsert/delete/search/merge/snapshot) with a Tantivy in-process implementation as
    the v1 reference engine. The trait is the seam OpenSearch slots behind as a measured per-cell upgrade
    (M5/§6.2) — do NOT build OpenSearch now; build the trait so it is a config/impl swap, not a rewrite.
  - The three co-located index shapes, all in one per-tenant index space keyed by the same doc_id: full-text
    inverted (term → posting list, BM25 stats per segment); structured/columnar fast-fields over the frozen
    FieldType enum (13.3, byte-identical to Issues'/Knowledge's encoding, incl. order_key as a columnar
    fast-field for sort); vector HNSW (incremental insert; soft-delete-then-compact-on-merge — critical for
    erasure, §3.3). There is NO separate vector store that could leak a doc the inverted index would have
    filtered (§3.2).
  - The per-tenant residency-pinned index layout (§3.4): the index DIRECTORY lives in the tenant's cell,
    (tenant, region)-keyed, envelope-encrypted with the per-tenant index DEK reserved in SRCH-P2 (encrypted-from-
    birth). The S1–S5 stateful-component register declared as derived/rebuildable scaffolding (S1 per-tenant
    FT+structured; S2 per-tenant vector; S3 dedup ledger; S4 reindex cursor; S5 filter/result cache) — the
    indexer/query/erase/reindex code that fills them is SRCH-P4..SRCH-P9.
  - The service boots from serve(AppSpec) (1.1): the AppSpec declares the three ports, the migrations (forward-
    only, 1.5), and the consumer/ports the later slices wire — NOT a hand-rolled main.
  - FLOOR named: OpenSearch is the reserved per-cell upgrade (M5/§6.2, behind the IndexBackend trait); IVF-PQ is
    the per-cell vector memory-pressure upgrade (§3.3, measured M5). Both named so the Tantivy + HNSW v1 is not
    mistaken for the final-scale engine. State that this slice ships the engine SHELL — the indexer (SRCH-P4) and
    the query path (SRCH-P5) are the follow-ons that make it answer anything.
- **CONTRACTS TO IMPLEMENT.** None owned-and-callable yet at the API level (6.1/6.2 land in SRCH-P5). Consumed:
  1.1 serve(AppSpec), 11.3 the per-tenant index DEK, 13.3 the FieldType enum, 13.1 the content taxonomy. To the
  frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The myelin-search service compiles and boots from serve(AppSpec) with three ports and liveness ≠ readiness
    (1.1/1.2/1.3); a forward-only migration creates a per-tenant index directory — CI.
  - engine.search is private (the search-requires-acl-filter lint from SRCH-P1 holds: no public path reaches it
    without a composed filter) — CI (permanent ratchet).
  - The index is encrypted-from-birth: a per-tenant index directory is envelope-encrypted under the per-tenant
    index DEK; destroying the DEK renders the directory unrecoverable (the structural crypto-shred check, the
    SRCH-D4 backstop substrate) — CI.
  - The three shapes round-trip a synthetic IndexDocument (upsert → search → delete) per shape; a hybrid query
    fuses results sharing one doc_id (no separate vector store) — CI.
- **TESTS (required).** Unit tests for the IndexBackend trait operations (open/upsert/delete/search/merge/
  snapshot) on a synthetic corpus; the three-shape round-trip; the per-tenant directory residency + encryption
  (the DEK-destroy renders it unrecoverable). The provider CDC stub for the engine seam (the consumer side is the
  query path SRCH-P5). myelin-search is mandatory-core: state the cargo-mutants mutation-score floor for the
  IndexBackend + encryption module and meet it.
- **DEFINITION OF DONE.** The IndexBackend trait + Tantivy engine + the three co-located index shapes exist and
  compile; the service boots from serve(AppSpec) with three ports; the index is encrypted-from-birth under the
  per-tenant DEK and DEK-destroy renders it unrecoverable; engine.search is private (the lint holds); the
  OpenSearch/IVF-PQ floors are named (M5); the engine-shell floor (indexer SRCH-P4, query SRCH-P5) is named; unit
  + CDC tests pass; the mutation floor is met; committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: Search engine + three index shapes (Tantivy, IndexBackend, encrypted-from-
  birth). Body lists: the IndexBackend trait + Tantivy + the three shapes; boots from serve(AppSpec); encrypted-
  from-birth (DEK-destroy unrecoverable); the OpenSearch/IVF-PQ floors named (M5); the engine-shell floor named
  (SRCH-P4/SRCH-P5); the IndexBackend mutation score measured. Branch first; do not push unless asked.
  Co-Authored-By trailer.

---

### SRCH-P4 — The near-real-time incremental indexer: the bus consumer (projection-fed, idempotent)

- **BAND.** M2.
- **ROADMAP MILESTONE.** S-M2 slice 2 of 7 (the near-real-time incremental indexer deliverable; §4.1).
- **DEPENDS-ON.** SRCH-P3 (the engine + the three shapes to upsert into). M0 outbox + consumer template
  (2.1/2.4/2.5) + the failure-injection harness. M2 siblings Search composes: project(ref, viewer) (5.6) + the
  #sub resolver (5.7) from Refs; the Bus reindex-from-source re-emit + sub-artifact-granular *.snapshot (2.6);
  the frozen myelin-content taxonomy (13.1). The embedding adapter trait (mock v1). The index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native — strategy pattern for the embedding adapter; mock during development);
    ../../external-insights/04-hard-problems.md §5.3 (reindex-from-source the resilience primitive; the projection
    not the DB); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it; observability),
    §4 (chain mutations end-to-end).
  - Architecture: ../05-refined-shared-systems-architecture/search-and-indexing.md §4.1 (the indexer is an
    ordinary myelin-events consumer; the per-event pipeline dedup → fetch project(ref, viewer)/replay snapshot
    NOT the DB → analyze → embed-if-semantic → build IndexDocument, stamp indexed_zookie+version → upsert S1/S2
    atomically per doc_id → mark dedup → ack; ACL state is indexed too — a permission-change event updates
    affected docs' indexed_zookie), §3.1 (the IndexDocument; doc_id may carry a frozen #sub), §4.5/§4.8 (the
    embedding adapter; model_ref pins the adapter).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 2.1 (EventEnvelope), 2.4
    (EventHandler consumer template — subjects() whitelist never "*", ack-after-enqueue, dedup ledger, bounded
    prefetch), 2.5 (consumer_dedup), 5.6 (project(ref, viewer) — the only way to read another subsystem's
    artifact, NOT its DB), 5.7 (the #sub resolver), 13.1 (the analyzable content text).
  - Roadmap: planning/06-roadmaps/shared/search-and-indexing.md §2 S-M2 (the incremental indexer bullet) + §1.2
    rows 2.1/2.4, 5.6/5.7, 13.1.
  - Drill source: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md SRCH-D7 (freshness, ~362; the CI
    freshness floor here, full-scale M5), SRCH-D5 (reindex-parity, ~360; the indexer is the one path D5 reuses).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-search service crate:
  - The near-real-time incremental indexer as an ordinary EventHandler (2.4): subjects() whitelists the
    domain-event subjects Search indexes (NEVER "*" — one of the reviewed excepted infra consumers that genuinely
    needs every domain event, §4.1); idempotent on event_id via the dedup ledger S3 (2.5); durable-bind-by-name,
    ack-after-enqueue, terminate-non-retryable to DLQ, bounded prefetch + per-tenant in-flight caps.
  - The per-event pipeline (idempotent, ordered per aggregate by (aggregate, seq)): dedup (skip if seen) → fetch
    the owner's project(ref, viewer)/replay snapshot via the ResilientClient — NOT its DB (5.6, no-cross-db lint)
    — and for sub-artifact docs resolve the #sub ArtifactRef through the unified #sub resolver (5.7) → analyze
    (language-detect → tokenize → normalize; the analyzer chain is SRCH-P8's depth, here a pass-through-correct
    floor) → embed via the embedding adapter (mock v1, behind a trait, model_ref pinned) if the type is
    semantically indexed → build IndexDocument (§3.1), stamp indexed_zookie + version from the event → upsert
    S1/S2 atomically per doc_id → mark dedup → ack.
  - ACL state is indexed too: a permission-change event updates the affected docs' indexed_zookie (and can
    invalidate cached filters) — Search indexes the OBJECT, Id computes the subject's reachable set at query time
    (the deliberate split that avoids the N+1 at index time, §4.1 tail).
  - The embedding adapter as a swappable trait with a MOCK deterministic implementation v1 (VISION §3: mock
    during development; the real EU-hostable model is a post-M5 config swap). model_ref pins the adapter so a swap
    triggers a re-embed reindex, never a silent mixed-model index.
  - FLOOR named: the mock embedding adapter (the real EU-hostable model adapter is the post-M5/runtime follow-on,
    ADR-12.8 — a config swap, the vector math + erasure are built now); the IndexSpec API + synthetic/test
    producer (the real per-subsystem IndexSpecs land M3 Git/KN, M4 Issues/CI/Chat — here exercised with a
    synthetic producer). Both named so the indexer is not mistaken for fed-by-real-producers or real-model.
- **CONTRACTS TO IMPLEMENT.** Consumed: 2.1 EventEnvelope, 2.4/2.5 the consumer template + dedup ledger, 5.6
  project(ref, viewer), 5.7 the #sub resolver, 13.1 the content text. The IndexSpec API (6.3) is FROZEN here as a
  shape exercised by a synthetic producer (the real instances land M3/M4). To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - Idempotent indexing: replaying the same event twice upserts one IndexDocument (dedup on event_id); ordering
    per aggregate by (aggregate, seq) is respected — CI.
  - The indexer reads the owner's project(ref, viewer) / replay snapshot, NEVER the owner DB: the no-cross-db
    lint is green on the indexer (no owner-DB read path compiles) — CI (permanent ratchet).
  - SRCH-D7 freshness (CI floor): a synthetic event → searchable within the seconds-grade budget; the index_lag
    telemetry (1.8) emits and alarms before user-visible staleness. Green artifact: the freshness p99 + index-lag
    signal (full-scale-under-load is SRCH-P12/M5) — CI.
  - Telemetry index_lag + consumer lag (num_pending) emitted (1.8) — no signal = failed drill — CI.
- **TESTS (required).** Unit tests for the per-event pipeline (dedup, project-fetch, IndexDocument build,
  indexed_zookie stamp, atomic upsert); the ACL-state-indexed path (a permission-change event updates
  indexed_zookie). A chained-mutation test: index → permission-change → re-index across a simulated consumer
  restart, asserting exactly-once-in-effect (EI-01 §4 — chain, don't single-handler). The drill-harness scenario
  for the SRCH-D7 CI freshness floor. The CDC pair for the consumer side of 2.4 + the consumer side of 5.6. State
  the SRCH-P3 IndexBackend mutation floor still holds; state the indexer module's own mutation floor and meet it.
- **DEFINITION OF DONE.** The incremental indexer exists and compiles as an ordinary EventHandler; idempotent on
  event_id; reads project(ref, viewer)/replay never the owner DB (the no-cross-db lint holds); the embedding
  adapter is a mock-behind-trait with model_ref pinned; ACL state is indexed; SRCH-D7's CI freshness floor emits
  its dated green artifact; the mock-adapter + synthetic-producer floors are named (real model post-M5, real
  IndexSpecs M3/M4); unit + chained + CDC tests pass; the mutation floor is met; committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: Search near-real-time incremental indexer. Body lists: the bus-consumer indexer
  wired (2.4/2.5/5.6/5.7); idempotent + project-fed (no-cross-db green); SRCH-D7 CI freshness floor greened; the
  mock-embedding-adapter + synthetic-producer floors named (post-M5 model, M3/M4 IndexSpecs); the indexer
  mutation score measured. Branch first; do not push unless asked. Co-Authored-By trailer.

---

### SRCH-P5 — The permission-aware query pipeline: conjoin the SetExpr/Ids ACL filter into every branch (THE crux)

- **BAND.** M2.
- **ROADMAP MILESTONE.** S-M2 slice 3 of 7 (the permission-aware query pipeline — the crux; contract 6.1; §4.2).
- **DEPENDS-ON.** SRCH-P3 (the engine), SRCH-P4 (the indexed corpus to query). M1 Identity 4.3 (list_objects with
  the frozen SetExpr push-down — THE crux dependency) + 4.2 (check + CaveatContext) + 4.10 (zookie + the authz
  reverse-index revision watermark). The per-tenant authz reverse index Search JOINs against (Id's materialised
  projection, kept fresh off the bus). The index resolves these to Identity's M1 P-NNN.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1, §3 (GDPR-safe by construction — a leak is both a security and a GDPR breach);
    ../../external-insights/02-platform-substrate.md §7 (permission-filtered set reads; Zanzibar/Leopard
    LookupResources reverse index); ../../external-insights/04-hard-problems.md §5;
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it; never weaken a threshold).
  - Architecture: ../05-refined-shared-systems-architecture/search-and-indexing.md §4.2 (the pipeline: acl ←
    list_objects → compile(ast) → CONJOIN acl_clause into EVERY branch → engine.search posting-list-level → rank/
    fuse; the two frozen shapes Filter{set_expr} and Ids{ids} and how Search lowers each; All → no clause, None →
    short-circuit empty), §4.2.1 (why pre-filter not post-filter — count/IDF leakage + N+1 melt), §4.2.3
    (consistency: zookies, fail-static, no-stale-grant; the reverse-index revision watermark), §3.4 tail (the S5
    cache holds a typed ListObjectsResult).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-E (the SetExpr
    push-down frozen — the All/None/Ids/NotIds/InRelation/Union/Intersect/Difference/TupleSet algebra).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 6.1 (query(ast, viewer, zookie?,
    page) → RankedResults; always conjoins the OQ-E Filter; the search-requires-acl-filter lint), 4.3
    (list_objects → Ids{ids,zookie} | Filter{set_expr,zookie} with the frozen SetExpr), 4.2 (check +
    CaveatContext), 4.10 (Consistency/zookie + the revision watermark), 1.6 (the lint).
  - Roadmap: planning/06-roadmaps/shared/search-and-indexing.md §2 S-M2 (the permission-aware query pipeline
    bullet, the crux) + §1.2 rows 4.3 (the single most load-bearing dependency), 4.2, 4.10 + §4 the
    critical-upstream note.
  - Drill source: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md SRCH-D1 (~356), SRCH-D2 (~357),
    SRCH-D3 (~358).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-search service crate:
  - The public query(ast, viewer, zookie?, page) → RankedResults entry (6.1) — the ONLY public query entry,
    composing the ACL filter first (engine.search stays private; the search-requires-acl-filter lint from SRCH-P1
    holds). The pipeline: (1) acl ← Id.list_objects(viewer, read, type, zookie?) → Ids{ids,zookie} |
    Filter{set_expr,zookie} (4.3); (2) plan ← compile(ast) (the AST compiler is SRCH-P6's depth; here the FT/
    structured/vector branch shells); (3) plan' ← plan ⨯ acl_clause(acl) — CONJOIN the ACL filter into EVERY
    branch BEFORE any scoring; (4) hits ← engine.search(plan') posting-list-level; (5) rank/fuse/paginate/project.
  - The SetExpr lowering, BOTH frozen shapes (OQ-E, 4.3): Ids/NotIds → a doc-id set membership filter clause
    (Tantivy term-set over doc_id/acl_object); InRelation{relation, via_column} / TupleSet{index} → a JOIN/
    semijoin against the per-tenant authz reverse index (Id's materialised (subject, relation, object_id)
    projection, replicated/queried per cell, intersected at the posting-list level — the Zanzibar LookupResources
    reverse index as a conjoinable filter, ONE query, no N+1, no post-filter); Union/Intersect/Difference → the
    boolean composition (OR/AND/EXCEPT over the clauses); All → no ACL clause (the type-and-tenant scope bounds
    it); None → short-circuit empty (WHERE false). Ids{ids} mode → a doc-id set filter directly (small/bounded
    reachable sets).
  - The tenant from the verified token, NEVER the URL path; the partition key (tenant, region) enforced
    (tenant-predicate lint). No cross-tenant query path.
  - Consistency (§4.2.3): a query may carry a zookie (read-your-writes after a sharing change); Search forwards
    it; a candidate doc whose indexed_zookie is older than the passed zookie for an ACL-relevant facet is
    re-validated (a bounded check on the affected candidates only, 4.2) or excluded pending re-index — never
    served stale-allow. Zookie-stamped queries bypass the fail-static cache (4.10); the authz reverse index JOIN
    honours the revision watermark (a JOIN needing a fresher revision than the index carries waits or falls back
    to a bounded check). (The full new-enemy + fail-static drill SRCH-D2 is greened in SRCH-P7; the mechanism is
    built here.)
  - FLOOR named: BM25 default ranking (the learning-to-rank / semantic re-rank follow-on is post-M5,
    measured-gap-triggered, §4.3 — built as a config layer over BM25 so the follow-on is a re-ranker swap). Named
    so BM25 is not mistaken for the final ranking answer.
- **CONTRACTS TO IMPLEMENT.** 6.1 query (owned). Consumed: 4.3 list_objects (the frozen SetExpr — lowered into
  every branch; Search is one of the five named SetExpr consumers, NO Id signature change), 4.2 check, 4.10
  zookie/Consistency. To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - SRCH-D1 (F1, the cardinal sin — the zero-escape leak): a confidential issue / overridden page / private
    channel / private repo file NEVER appears in any query result — including counts, IDF, "more results", and
    RAG — for an unauthorized viewer, across an adversarial corpus. Gate: 0 leaked docs, 0 count-leak. Green
    artifact: the zero-escape counter at 0 — CI. (This is a Search row of the master M2→M3 boundary; M3 does not
    start over a red SRCH-D1.)
  - SRCH-D3 (F2, cross-tenant IDOR): spoof the path-tenant → 0 cross-tenant results (tenant from token, partition
    key enforced) — CI.
  - No N+1: the query issues ONE list_objects per query (assert via query-count telemetry), never one check per
    result; the filter-mode split (Ids vs Filter/TupleSet) telemetry (1.8) fires — CI.
  - The search-requires-acl-filter lint green: no compiled plan reaches engine.search without the conjoined ACL
    clause — CI (permanent ratchet).
- **TESTS (required).** Unit tests for every SetExpr lowering form (All, None, Ids, NotIds, InRelation, TupleSet,
  Union, Intersect, Difference) → the correct engine filter clause / reverse-index JOIN; the conjoin-into-every-
  branch (FT, structured, vector) invariant. A chained test: index a confidential + a public doc → query as an
  unauthorized viewer → 0 leak incl. counts/IDF; then grant → re-query (now visible). The drill scenarios for
  SRCH-D1 (the adversarial corpus, incl. the count/IDF/more-results leak vectors) and SRCH-D3. The CDC pair for
  6.1 (provider) + the consumer CDC for 4.3. Mutation floor on the ACL-conjoin + SetExpr-lowering module
  (mandatory-core — this is the leak-critical code) stated and met.
- **DEFINITION OF DONE.** query exists and compiles as the only public entry composing the filter first; the
  frozen SetExpr lowering conjoins into every branch with no N+1 and no post-filter; SRCH-D1 (0 leak incl.
  counts/IDF/RAG) and SRCH-D3 (0 cross-tenant) each emit a dated green artifact; the search-requires-acl-filter
  lint + the filter-mode-split telemetry are green; the BM25-ranking floor is named (post-M5 re-rank); unit +
  chained + CDC tests pass; the mutation floor is met; committed. No threshold weakened, no assertion inverted.
- **COMMIT.** Header: P-<NNN> M2: Search permission-aware query pipeline (conjoin the SetExpr ACL filter). Body
  lists: 6.1 query implemented; the frozen SetExpr lowering (all forms) conjoined into every branch; SRCH-D1
  (0 leak incl. counts/IDF/RAG) + SRCH-D3 (0 cross-tenant) greened; one-list_objects-no-N+1 proven; the
  BM25-ranking floor named (post-M5 re-rank); the ACL-conjoin mutation score measured. Branch first; do not push
  unless asked. Co-Authored-By trailer.

---

### SRCH-P6 — The query-AST compiler: one compile target of the frozen QueryAst (+ read-time rollup/formula inputs)

- **BAND.** M2.
- **ROADMAP MILESTONE.** S-M2 slice 4 of 7 (the query-AST compiler deliverable; §4.6).
- **DEPENDS-ON.** SRCH-P5 (the query pipeline the compiler feeds; the always-conjoin step). The frozen
  myelin-query crate (13.3 — the QueryAst grammar, the FieldType enum, ViewSpec, order_key) + myelin-content
  (13.1). The index resolves these to their M2 P-NNN.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3; ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it; a crafted query
    cannot DoS the engine), §7 (one parser/validator/renderer — reconcile the shared primitive, no drift).
  - Architecture: ../05-refined-shared-systems-architecture/search-and-indexing.md §4.6 (Search is one compile
    target of the single frozen QueryAst; validate against the frozen FieldType + the bounded-cost guard — no
    UDFs/loops/recursion, statically cost-bounded; lower Text → FT clauses, Cmp/In/Has/Ref over typed fields →
    structured fast-field clauses, semantic/near → a vector branch; ALWAYS conjoin acl_clause; render back; an
    agent and the UI emit the same query), §4.6 tail (read-time rollup/formula: Search indexes their INPUTS,
    never the derived value).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-3/OQ-C (the
    QueryAst/FieldType/ViewSpec frozen byte-identical), KN-3 (rollup/formula read-time-computed never stored).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 13.3 (the myelin-query primitive
    frozen byte-identical — the QueryAst grammar = the EventMatcher core 3.4, the FieldType enum, ViewSpec,
    order_key/LexoRank), 6.1 (query — the compiler feeds it), 3.4 (the EventMatcher = the same QueryAst, one
    grammar one validator).
  - Roadmap: planning/06-roadmaps/shared/search-and-indexing.md §2 S-M2 (the query-AST compiler bullet) + §1.2
    rows 13.1/13.3 + §4 the second critical-upstream note (the frozen primitives so Search means the same as
    Issues/KN and the Tier-3 valve can share semantics).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-search service crate:
  - The query-AST compiler, Search as ONE compile target of the single frozen QueryAst (13.3 — the SAME AST the
    bus's EventMatcher 3.4 and saved views compile): (1) VALIDATE against the frozen FieldType definitions + the
    bounded-cost guard (no UDFs, no loops, no recursion — statically cost-bounded so a crafted query cannot DoS
    the engine); (2) LOWER Text{query, fields} → FT clauses, Cmp/In/Has/Ref over typed fields → structured
    fast-field clauses, a semantic/near request → a vector branch; (3) ALWAYS conjoin acl_clause(list_objects(...))
    (the SRCH-P5 step — there is no compiled plan without it); (4) RENDER back to human-readable for the UI (one
    parser, one validator, one renderer). Because Search shares the AST, an agent and the UI emit the SAME query,
    permission-filtered identically — no agent search back-door.
  - Read-time rollup/formula handling (X-3 / KN-3): rollup and formula fields are computed at READ TIME, never
    stored. Search indexes their INPUTS (the relation targets, the formula's source fields), not the derived
    value — a Cmp over a rollup/formula field compiles to a predicate the view evaluates after fetch, or (when
    the input is a stored facet) to a structured clause over the inputs. A derived value is never a stale indexed
    artifact (the freshness/consistency choice).
  - order_key (the LexoRank fractional index, 13.3) compiles to a columnar fast-field for sort, byte-identical to
    Issues'/Knowledge's encoding.
  - FLOOR named: none new — the compiler is the full shape at M2. State that the analyzer DEPTH (multilingual
    chain) is SRCH-P8's deliverable and the producer-specific IndexSpec facets arrive M3/M4 — so the compiler is
    not mistaken for fed-by-real-facets.
- **CONTRACTS TO IMPLEMENT.** Consumed: 13.3 the QueryAst/FieldType/ViewSpec/order_key (Search is the compile
  target — it speaks the one frozen AST, it does NOT define a second query language), 13.1 the content text. The
  6.1 query pipeline is fed by this compiler. To the frozen byte-identical shapes — any needed grammar change is
  a whole-workspace contract PR (Issues/KN co-own), escalated, never a local Search divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The bounded-cost guard: a crafted/adversarial QueryAst (deep nesting, large IN sets) is rejected or
    statically cost-bounded — 0 engine DoS (assert query-cost telemetry stays within the bound) — CI.
  - AST round-trip: render(compile(ast)) is the canonical human-readable form; a saved-view AST compiles to the
    same plan an agent's identical AST compiles to (no agent back-door) — CI.
  - Read-time-field correctness: a Cmp over a rollup/formula field does NOT read a stored derived value (Search
    indexed only the inputs); the derived value is computed after fetch — CI.
  - The conjoin-always invariant holds (the search-requires-acl-filter lint green on every compiled plan) — CI
    (permanent ratchet).
- **TESTS (required).** Unit tests for every QueryAst node (And/Or/Not/Cmp/In/Has/Text/Ref + Op + Literal) →
  the correct FT/structured/vector lowering; the bounded-cost guard (adversarial ASTs rejected); the read-time
  rollup/formula path (inputs indexed, derived computed after fetch); order_key → columnar fast-field. A
  byte-identical-semantics test: the same AST compiled by Search vs. the EventMatcher core (3.4) means the same
  thing (a drift test, so a later FieldType change breaks this test now). The CDC pair for the Search compile-
  target side of 13.3. Mutation floor on the compiler + cost-guard module stated and met.
- **DEFINITION OF DONE.** The query-AST compiler exists and compiles every frozen QueryAst node to the FT/
  structured/vector branches with the always-conjoin step; the bounded-cost guard prevents engine DoS;
  rollup/formula are read-time (inputs indexed, never the derived value); the agent/UI emit the same query
  (no back-door); the byte-identical-semantics drift test passes; unit + CDC tests pass; the mutation floor is
  met; committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: Search query-AST compiler (frozen QueryAst, read-time rollup/formula). Body
  lists: the compiler implemented as one compile target of 13.3; the bounded-cost guard (0 DoS); read-time
  rollup/formula (inputs indexed); the byte-identical-semantics drift test green; the compiler mutation score
  measured. Branch first; do not push unless asked. Co-Authored-By trailer.

---

### SRCH-P7 — Hybrid + vector: RRF fusion + filter-during-traversal + the no-stale-grant zookie path

- **BAND.** M2.
- **ROADMAP MILESTONE.** S-M2 slice 5 of 7 (the hybrid + vector deliverable + the consistency/no-stale-grant
  drill; §4.5, §4.2.2, §4.2.3; contract 6.2).
- **DEPENDS-ON.** SRCH-P3 (the HNSW vector shape), SRCH-P5 (the ACL conjoin — the same predicate fed into
  traversal), SRCH-P6 (the compiler's vector branch). M1 Identity 4.10 (zookie + the revision watermark) + the
  fail-static cache (1.10). The mock embedding adapter (SRCH-P4). The index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native — RAG permission-correct by the same pre-filter);
    ../../external-insights/04-hard-problems.md §5; ../../external-insights/01-process-and-quality-doctrine.md §3
    (prove-it; never invert an assertion).
  - Architecture: ../05-refined-shared-systems-architecture/search-and-indexing.md §4.5 (vector k-NN over the
    per-tenant HNSW, ACL-filtered during traversal; agent RAG gets the top-k VISIBLE passages; RRF fusion —
    score-scale-free, both branches carry the same ACL filter so fusion can never introduce a hidden doc), §4.2.2
    (filter-during-traversal — the ACL clause + structured predicates evaluated AS the HNSW graph is traversed,
    so the k returned are the k-nearest VISIBLE neighbours not k-then-filtered; very selective filters fall back
    to brute-force over the small visible set; the property — k visible neighbours, no leak — is fixed, the
    STRATEGY is the M5 follow-on D8), §4.2.3 (zookies, fail-static, no-stale-grant; the revision watermark),
    §3.3 (the model_ref pins the adapter).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 6.2 (semantic(text|vec, viewer,
    k, filter_ast?) → k visible NN — ACL-filtered-during-traversal; agent RAG, dedup), 4.10 (Consistency/zookie +
    the revision watermark), 1.10 (FailStatic — bounded-staleness; static_max ≤ revocation SLA).
  - Roadmap: planning/06-roadmaps/shared/search-and-indexing.md §2 S-M2 (the hybrid + vector bullet; the
    filter-during-traversal FLOOR named with its M5 D8 follow-on) + §1.1 row 6.2.
  - Drill source: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md SRCH-D2 (~357), SRCH-D1 (~356, the
    RAG/vector leak half).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-search service crate:
  - semantic(text|vec, viewer, k, filter_ast?) → k visible NN (6.2): vector k-NN over the per-tenant HNSW index,
    ACL-FILTERED DURING TRAVERSAL — the ACL clause (the SRCH-P5 SetExpr lowering) and any structured predicates
    are evaluated as the HNSW graph is traversed, so the k returned are the k-nearest VISIBLE neighbours, NOT
    k-nearest-then-filtered. Very selective filters fall back to brute-force over the small visible set. Agent RAG
    via semantic is permission-correct by the same pre-filter — an agent never retrieves a doc its delegated
    principal can't see.
  - RRF (Reciprocal Rank Fusion) for hybrid lexical+semantic queries: score-scale-free (no per-corpus
    calibration); BOTH branches carry the same ACL filter so fusion can never introduce a hidden doc (§4.5).
  - The no-stale-grant zookie path (§4.2.3, the SRCH-P5 mechanism now fully drilled): a zookie-stamped query
    bypasses the fail-static cache (4.10) and the authz reverse-index JOIN honours the revision watermark; a
    just-revoked grant is excluded; a default-consistency query may use the cached filter during an Id hiccup
    (bounded staleness ≤ revocation SLA W) and degrades-not-cascades (fail-static 1.10).
  - FLOOR named: filter-during-traversal as the recall mechanism (the PROPERTY — k visible neighbours, no leak —
    is fixed here; the TUNED filtered-ANN strategy — the brute-force-fallback threshold under very selective
    filters + the HNSW↔IVF-PQ promotion point — is the M5 follow-on, SRCH-P12, drill D8). The mock embedding
    adapter (real model post-M5). Both named so the v1 vector path is not mistaken for the tuned-at-scale answer.
- **CONTRACTS TO IMPLEMENT.** 6.2 semantic (owned). Consumed: 4.10 zookie/Consistency + the revision watermark,
  1.10 FailStatic. To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - SRCH-D1 (the vector/RAG leak half): a confidential doc NEVER appears in a semantic / RAG result for an
    unauthorized viewer — the filter-during-traversal returns k VISIBLE neighbours; RRF fusion introduces no
    hidden doc. 0 leak — CI.
  - SRCH-D2 (F1/F8, zero-escape-under-staleness): revoke a grant, re-search with the post-revoke zookie →
    excluded (the zookie bypasses fail-static + honours the reverse-index revision watermark); default-
    consistency excludes within W ≤ revocation SLA. Gate: 0 stale-allow with zookie; ≤ W without. Green artifact:
    the exclusion-within-W signal — CI.
  - Fail-static: with Id forced unavailable, a default-consistency query degrades on the coarse cache (no
    cascade); the fail-static ratio telemetry (1.8) fires; a zookie-stamped query does NOT use the stale cache —
    CI.
- **TESTS (required).** Unit tests for filter-during-traversal (k visible neighbours under a selective filter;
  brute-force fallback over a small visible set); RRF fusion (both branches carry the ACL filter; no hidden doc).
  A chained test: grant → semantic search (visible) → revoke with a new zookie → re-search (excluded), proving
  the new-enemy bypass + fail-static. The drill scenarios for the SRCH-D1 vector/RAG half + SRCH-D2. The CDC pair
  for 6.2. Mutation floor on the filter-during-traversal + zookie-revalidation module (mandatory-core — leak-
  critical) stated and met.
- **DEFINITION OF DONE.** semantic + RRF fusion exist and compile; filter-during-traversal returns k visible
  neighbours (SRCH-D1 vector/RAG half: 0 leak); SRCH-D2 (0 stale-allow with zookie, ≤ W without) emits a dated
  green artifact; fail-static degrades-not-cascades and zookie bypasses it; the filtered-ANN-strategy + mock-
  adapter floors are named (M5 D8 SRCH-P12; post-M5 model); unit + chained + CDC tests pass; the mutation floor
  is met; committed. No threshold weakened, no assertion inverted.
- **COMMIT.** Header: P-<NNN> M2: Search hybrid + vector (RRF, filter-during-traversal, no-stale-grant). Body
  lists: 6.2 semantic implemented; filter-during-traversal (k visible, SRCH-D1 vector half: 0 leak); RRF fusion
  (no hidden doc); SRCH-D2 greened (0 stale-allow, ≤ W); fail-static degrades-not-cascades; the filtered-ANN
  strategy floor named (M5 D8); the traversal mutation score measured. Branch first; do not push unless asked.
  Co-Authored-By trailer.

---

### SRCH-P8 — Multilingual analysis + caching/freshness + the telemetry contract

- **BAND.** M2.
- **ROADMAP MILESTONE.** S-M2 slice 6 of 7 (the multilingual analysis + the caches + the telemetry deliverables;
  §4.7, §4.10, §4.11).
- **DEPENDS-ON.** SRCH-P4 (the indexer the analyzer plugs into), SRCH-P5 (the query the result cache fronts +
  the list_objects filter the S5 cache holds). M1 Identity 4.10 (the zookie-bucketing + TTL ≤ revocation SLA).
  The index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1 (EU-sovereign — many languages); ../../external-insights/01-process-and-quality-doctrine.md
    §3 (observability is part of the pass — a system that survives a drill but emits no signal has FAILED it).
  - Architecture: ../05-refined-shared-systems-architecture/search-and-indexing.md §4.7 (per-language analyzers
    mandatory for the EU; language detection at index time sets lang; the per-language chain UAX #29 tokenization
    → Snowball/Porter stemming → stopwords → diacritic-fold; CJK/non-segmented via n-gram/ICU; query-time
    analyzer matches index-time per field-language; code/identifiers use the camel/snake tokenizer; the EXACT
    initial EU language list + CJK strategy remains [OPEN → P6]), §4.10 (the list_objects filter cache S5 holding
    a typed ListObjectsResult, TTL ≤ revocation SLA, bypassed for zookie-stamped queries; the hot-query result
    cache zookie-bucketed + request-coalesced), §4.11 (the telemetry contract — the §4.11 signal set).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 1.8 (the telemetry signal set +
    the filter-mode split Ids vs Filter/TupleSet — every Search drill asserts against this), 4.10 (the
    zookie-bucketing + TTL ≤ revocation SLA), 13.1 (the content text the analyzer chain processes).
  - Roadmap: planning/06-roadmaps/shared/search-and-indexing.md §2 S-M2 (the caching + telemetry bullets) +
    §1.1 row (1.8) + §10 (the [OPEN → P6] analyzer-set note).
  - Drill source: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md SRCH-D7 (~362, freshness — the
    caches + telemetry are what alarm before user-visible staleness).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-search service crate:
  - The multilingual analysis chain (§4.7): language detection at index time sets lang (a source-declared
    language overrides); per-language analyzer chain — UAX #29 tokenization → Snowball/Porter-family stemming →
    stopwords → diacritic-fold; CJK/non-segmented scripts via n-gram/ICU; the query-time analyzer matches the
    index-time analyzer per field-language; code/identifiers use the camel/snake tokenizer (the code-search
    tokenizer, the depth lands in SRCH-P10). FLOOR named: the EXACT initial EU language list + the CJK
    tokenization strategy remain [OPEN → P6] (§10) — ship a named, extensible default set (e.g. the major EU
    languages) and write the open call into the gap report; the MECHANISM (per-language chain) is decided.
  - The caches (§4.10): the list_objects filter cache S5 — caching the typed ListObjectsResult (Ids or
    Filter{set_expr}) per (tenant, region, subject, type, zookie-bucket), TTL ≤ revocation SLA, NEVER the source
    of truth, bypassed for zookie-stamped queries; the hot-query result cache (bounded, zookie-bucketed,
    request-coalesced). Both residency-pinned + crypto-shred-able under the per-tenant index DEK.
  - The telemetry contract (§4.11, contract 1.8) exported on the metrics-health port: index lag; query latency
    RED per principal-kind + tenant (FT/structured/vector/hybrid); the list_objects call rate + cache hit ratio +
    the filter-mode split (Ids vs Filter/TupleSet); zero-escape assertion counters (zookie-bypass, stale-served);
    the reindex progress + cold-vs-live parity hash; erase receipts + vector-tombstone/compaction lag; consumer
    lag (num_pending); per-tenant in-flight + shed counts. Every later Search drill asserts against these — no
    signal = a failed drill (EI-01 §3).
- **CONTRACTS TO IMPLEMENT.** 1.8 the telemetry signal set (+ the filter-mode split) — the green-artifact source
  every Search drill reads. Consumed: 4.10 the zookie-bucketing + TTL ≤ revocation SLA, 13.1 the content text. To
  the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - Analyzer correctness: a query in language L matches index-time-L tokens (stem/diacritic-fold/stopword
    parity); a CJK/non-segmented query matches via n-gram/ICU; code identifiers tokenize via camel/snake keeping
    operators — CI.
  - Cache correctness: the S5 filter cache is bypassed for zookie-stamped queries (no stale-allow); a TTL ≤
    revocation SLA bound holds; the result cache coalesces concurrent identical requests and is zookie-bucketed
    (no cross-zookie bleed) — CI.
  - The telemetry signal set is emitted on the metrics-health port (1.8): every signal in the §4.11 list is
    present and readable by the telemetry-assertion library — a missing signal fails the gate (observability is
    part of the pass) — CI.
- **TESTS (required).** Unit tests for the per-language analyzer chain (tokenize/stem/fold/stopword per language;
  CJK n-gram; code camel/snake); the S5 cache (zookie-bypass, TTL bound, residency); the result cache (coalesce,
  zookie-bucket). A telemetry-assertion test reading each §4.11 signal from the metrics port. No new owned
  contract (1.8 is the harness's; Search emits the signals). Mutation floor on the analyzer + cache module
  stated and met.
- **DEFINITION OF DONE.** The multilingual analyzer chain (with the EU-language-set + CJK floor named [OPEN →
  P6]), the S5 filter cache + the result cache (zookie-bypass + TTL ≤ revocation SLA), and the full §4.11
  telemetry signal set exist and compile; the analyzer/cache correctness gates are green; every telemetry signal
  is emitted and readable; the analyzer-set floor is written into the gap report; unit + telemetry-assertion
  tests pass; the mutation floor is met; committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: Search multilingual analysis + caches + telemetry contract. Body lists: the
  per-language analyzer chain (EU-set + CJK floor named [OPEN → P6]); the S5 filter cache + result cache
  (zookie-bypass, TTL ≤ revocation SLA); the §4.11 telemetry signal set emitted (observability is part of the
  pass); the analyzer + cache mutation score measured. Branch first; do not push unless asked. Co-Authored-By
  trailer.

---

### SRCH-P9 — Erasure as a real holder (purge + reindex, vectors compacted) + reindex-from-source (the only rebuild path)

- **BAND.** M2.
- **ROADMAP MILESTONE.** S-M2 slice 7 of 7 (erasure as a real holder + reindex-from-source; §4.8, §4.9;
  contracts 10.1 real erase + 6.4 reindex).
- **DEPENDS-ON.** SRCH-P2 (the holder stub + the per-tenant index DEK), SRCH-P3 (the engine + the soft-delete-
  then-compact vector shape), SRCH-P4 (the indexer — the one ingest path reindex re-drives). Identity pseudonym
  resolve/erase (4.8); the Bus reindex-from-source re-emit + sub-artifact-granular *.snapshot/*.erased (2.6); the
  erasure ledger (10.8). The index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (data-subject erasure is an architectural constraint);
    ../../external-insights/04-hard-problems.md §1 (erasure vs immutability — erase is a real purge, not hide),
    §5.3 (reindex-from-source the only recovery path, no bespoke recovery reader);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it).
  - Architecture: ../05-refined-shared-systems-architecture/search-and-indexing.md §4.8 (locate/export/rectify/
    restrict/erase; erase = PURGE + RE-INDEX not hide; vectors tombstoned + compacted; embeddings erased with
    their source doc; restrict suppresses indexing/agent-use/analytics; the *.erased tombstone drives the purge
    via the SAME live consumer path — no erasure backdoor; HYOK structural skip when
    can_derive_plaintext_index()=false; the per-tenant index DEK + per-subject source DEK backstop layering),
    §4.9 (reindex-from-source the ONLY rebuild path — Search never reads owner DBs; reindex(scope) → owner
    replay → *.snapshot through the live indexer; the cursor store S4 throttled/resumable/per-tenant caps;
    deterministic snapshot event_id; NO "load the index from Postgres" backdoor, SEARCH-1).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md change #9 (the
    per-subject DEK backstop), X-7 (the residual posture Search satisfies by purge + reindex + restrict).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 10.1 (PersonalDataHolder real
    erase — purge/crypto-shred/pseudonymise never hide; restrict suppression), 6.4 (reindex(scope) → job — the
    only rebuild path, sub-artifact-granular *.snapshot replay), 2.6 (the Bus re-emit + sub-artifact-granular
    replay/erased), 4.8 (resolve_pseudonym/erase — the pseudonym Search locates by), 10.8 (the erasure ledger),
    11.3 (HYOK can_derive_plaintext_index).
  - Roadmap: planning/06-roadmaps/shared/search-and-indexing.md §2 S-M2 (the erasure + reindex-from-source
    bullets; the per-tenant-index-DEK floor → purge+reindex follow-on) + §1.1 rows 6.4, (10.1).
  - Drill source: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md SRCH-D4 (~359), SRCH-D5 (~360),
    SRCH-D10 (~365, the HYOK structural skip built here, the at-scale assertion M5).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-search service crate (this slice
  replaces the SRCH-P2 holder stub with the real mechanism):
  - The real PersonalDataHolder mechanism (10.1): locate(subject) — find every doc/field/vector referencing the
    subject (by acl_object, by actor/assignee/mention facets, by the subject's pseudonym
    <pseudonym>@<tenant>.noreply, 4.8); erase(subject) — PURGE + RE-INDEX not hide: delete the affected docs/
    fields, tombstone + COMPACT the vectors (§3.3, no orphan embedding), re-index the surviving artifact from the
    source's now-tombstoned projection, return a receipt; restrict(subject) — suppress indexing/agent-use/
    analytics/notification for a subject pending erasure (a restricted subject's content is not surfaced in
    results or RAG); export/rectify per 10.1.
  - The *.erased tombstone drives the purge via the SAME live consumer path as everything else (the SRCH-P4
    indexer) — NO bespoke erasure backdoor. The per-tenant index DEK crypto-shreds the whole tenant index on
    tenant-decommission and backstops backups/immutable segments; the per-subject source DEK (11.4) is the added
    source-side backstop. Embeddings erased with their source (model_ref change → a re-embed reindex purging
    old-model vectors in the same pass).
  - The HYOK structural skip: when can_derive_plaintext_index()=false (11.3), Search structurally SKIPS indexing
    — there is no plaintext to embed or analyse, so the tenant's content is not in the index at all (the no-leak
    property holds by construction). Build the structural skip here; the cross-store-plaintext assertion AT SCALE
    is M5 (SRCH-D10, SRCH-P13).
  - reindex(scope) → job (6.4): the ONLY rebuild path (SEARCH-1). reindex calls the Bus re-emit protocol (2.6) →
    each owning subsystem replay(scope, since=cursor) emits *.snapshot via its outbox → Search's ordinary indexer
    (SRCH-P4) ingests them, idempotent on the deterministic snapshot event_id. ONE code path for steady-state and
    recovery; the cursor store S4 (throttled/resumable/per-tenant caps) is v1; a new sub-index = a reindex from
    since=0. NO "load the index from Postgres" backdoor (the no-cross-db lint catches it).
  - FLOOR named: the CI-scale SRCH-D4/SRCH-D5 variants gate this band; the full backup-level erasure proof
    (SRCH-D4 at backup scale, folded into E2E-4) and the full-scale reindex-parity (SRCH-D5 at scale) are the M5
    follow-ons (SRCH-P13/SRCH-P14). Named so the CI-variant green is not mistaken for the backup-scale proof.
- **CONTRACTS TO IMPLEMENT.** 10.1 PersonalDataHolder real erase (owned — replaces the SRCH-P2 stub), 6.4 reindex
  (owned). Consumed: 2.6 the Bus re-emit + sub-artifact-granular replay/erased, 4.8 resolve_pseudonym/erase, 10.8
  the erasure ledger, 11.3 HYOK can_derive_plaintext_index. To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - SRCH-D4 (erasure, CI variant): erase a subject → every doc/field/vector/embedding purged (NOT hidden) and
    unrecoverable (per-tenant + per-subject DEK backstop); 0 orphan embedding, 0 recoverable personal data incl.
    vectors. Green artifact: the embedding-purge receipt — CI (a moderate-scale CI variant; the full backup-level
    proof joins the M5 DSAR fan-out E2E-4, SRCH-P13/SRCH-P14).
  - SRCH-D5 (F4, reindex-from-cold parity, CI variant): wipe the index; reindex(scope) → the rebuilt index == live
    (docs, ACL behaviour, ranking, vectors, sub-artifact granularity) using the live consumer path ONLY. Green
    artifact: the reindex-parity hash — CI (a small-corpus CI variant; full-scale is M5, SRCH-P13).
  - The HYOK structural skip: a content class marked HYOK (can_derive_plaintext_index()=false) is NOT in the
    index at all — 0 plaintext indexed (the structural check; the at-scale cross-store assertion is M5) — CI.
  - The no-cross-db lint green: reindex re-drives the indexer, never reads an owner DB; there is no "load the
    index from Postgres" path — CI (permanent ratchet).
- **TESTS (required).** Unit tests for locate/erase/restrict (purge not hide; vectors compacted; no orphan
  embedding; restrict suppresses results + RAG); the *.erased-via-live-consumer path (no backdoor); the HYOK
  structural skip. A chained test: index → erase → assert 0 recoverable incl. vectors → reindex from source →
  the rebuilt index excludes the erased subject (re-erasure does not resurrect). The drill scenarios for the
  SRCH-D4 CI variant + the SRCH-D5 CI variant. The CDC pair for the Search side of 10.1 (real erase) + 6.4.
  Mutation floor on the erase + reindex module (mandatory-core — erasure-critical) stated and met.
- **DEFINITION OF DONE.** The real holder erase (purge + reindex, vectors compacted, restrict suppression) +
  reindex-from-source (the only rebuild path, no Postgres backdoor) + the HYOK structural skip exist and compile;
  SRCH-D4 (CI: 0 recoverable incl. vectors) and SRCH-D5 (CI: cold == live) each emit a dated green artifact; the
  no-cross-db lint is green; the SRCH-P2 holder stub is replaced by the real mechanism; the full-scale/backup-
  scale drill floors are named (SRCH-P13/SRCH-P14); unit + chained + CDC tests pass; the mutation floor is met;
  committed. This completes the master M2→M3 boundary's Search rows (SRCH-D1/SRCH-D3 green from SRCH-P5; SRCH-D2
  from SRCH-P7; SRCH-D4/D5 CI here) — M3 does not start over a red SRCH-D1. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: Search erasure holder (purge+reindex) + reindex-from-source. Body lists: 10.1
  real erase + 6.4 reindex implemented (replaces the SRCH-P2 stub); SRCH-D4 (CI: 0 recoverable incl. vectors) +
  SRCH-D5 (CI: cold == live) greened; the HYOK structural skip; the no-cross-db lint green (no Postgres
  backdoor); the full-scale/backup-scale floors named (SRCH-P13/SRCH-P14); the erase mutation score measured.
  Branch first; do not push unless asked. Co-Authored-By trailer.

---

### SRCH-P10 — Producer corpora light up: code search v1 (Git) + Knowledge indexing + sub-artifact projections

- **BAND.** M3.
- **ROADMAP MILESTONE.** S-M3 (planning/06-roadmaps/shared/search-and-indexing.md §2 "S-M3 — Producer corpora
  light up: code search + knowledge docs").
- **DEPENDS-ON.** SRCH-P3..SRCH-P9 (the Search core green — SRCH-D1/D2/D3/D4-CI/D5-CI). The M3 Git + Knowledge
  producer prompts that ship Git's git.* indexable projection per blob/ref/symbol + per-blob/ref replay + the
  content-anchored line-range sub_anchor, and KN's IndexSpec + block/page project + page-subtree replay + the
  block/heading/row/field sub-anchors. Refs' project(ref, viewer) + the #sub resolver (SRCH consumes via
  SRCH-P4). The index resolves these to the Git/Knowledge M3 P-NNN. (AG-D4 green is a band precondition, not a
  Search dependency — Search runs no untrusted code.)
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (find any artifact); ../../external-insights/04-hard-problems.md §5 (code search v1 scope;
    Search easy to under-budget); ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/search-and-indexing.md §4.4 (code search v1 =
    symbol/path/literal-grade — file paths, identifiers via the camel/snake tokenizer keeping operators, string
    literals, commit messages, trigram/n-gram indexing for substring/regex-lite, Russ Cox's trigram approach;
    code-block text is RAW not markdown-parsed; the SCIP/LSIF "find usages" follow-on NAMED not built v1; Search
    does NOT parse repos — Git emits the projection), §4.7 (the multilingual analyzers for KN), §4.1 tail / §4.9
    ask (sub-artifact-granular projections; Git line-ranges content-anchored — the searchable span re-derived
    from the owner's resolve, never a stale raw line number; KN replay page-subtree at block granularity), §4.6.1
    (the GIN-indexed JSONB facet scan floor for KN/Issues custom fields).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md change #8 (the
    SCIP/LSIF follow-on input named), X-2 (the three content nodes byte-identical).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 6.5 (the Git git.* projection per
    blob/ref/symbol; the SCIP/LSIF follow-on), 6.3 (declare_indexable — KN's IndexSpec; the measured
    projection-feeder promotion), 5.7 (the #sub kinds on real sub-anchors; Git line-ranges content-anchored),
    2.6 (sub-artifact-granular replay).
  - Roadmap: planning/06-roadmaps/shared/search-and-indexing.md §2 S-M3 (all bullets; the code-search-v1 and
    GIN-scan floors named) + §1.1 rows 6.3/6.5.
  - Drill source: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md SRCH-D1/SRCH-D3 (re-confirm on real
    corpora, ~356/358), SRCH-D5 (Git+KN corpus, ~360).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-search service crate (the ENGINE is
  UNCHANGED; this prompt wires Search to the first REAL producers and re-confirms the invariants on real
  corpora):
  - Code search v1 (6.5): consume Git's git.* indexable projection per blob/ref/symbol — file paths, identifiers
    (the camel/snake tokenizer keeping operators), string literals, commit messages, with trigram/n-gram indexing
    for substring/regex-lite (Russ Cox). Code-block text from myelin-content is RAW (tokenized with the code
    tokenizer, not a language stemmer). Search does NOT parse repos (no-cross-db lint) — Git emits the projection.
  - Knowledge indexing (6.3): consume KN's IndexSpec — block + page text (the multilingual analyzers, SRCH-P8),
    the structured inline nodes (mention/artifact_ref/embed) as dependable facets, the in-doc database JSONB
    struct fields (the GIN-indexed scan floor), vector-in-v1 for semantic KN search. rollup/formula INPUTS
    indexed, derived values never stored (the SRCH-P6 read-time path).
  - Sub-artifact-granular projections (2.6 / 5.7): index doc blocks (b<id>/h<id>), KN db rows/fields (row-/
    field-), Git line-ranges (L<a>-L<b>, CONTENT-ANCHORED — the searchable span is re-derived from the owner's
    resolve, never a stale raw line number). KN replay is page-subtree at block granularity; Git replay is
    per-blob/ref. Drive the scoped reindex (SRCH-P9) at the right grain.
  - FLOOR named: code search v1 = symbol/path/literal + trigram (the AST-aware "find usages" / cross-reference
    consuming a CI-produced SCIP/LSIF projection is the post-M4/demand-triggered follow-on, named in the gap
    report, change #8); the GIN-indexed JSONB facet scan for KN custom fields (the generated projection-feeder
    index promoted per facet at > 5% of view executions is the MEASURED M5 follow-on, SRCH-P12 — the GIN scan
    serves correctly meanwhile, promotion changes cost never correctness). Both named.
- **CONTRACTS TO IMPLEMENT.** 6.5 (consume the real Git git.* projection), 6.3 (consume KN's IndexSpec), 5.7 (the
  #sub kinds on real sub-anchors), 2.6 (drive Git/KN replay). To the frozen shapes; the engine does not change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - SRCH-D1 / SRCH-D3 re-confirmed green on the REAL Git + KN corpora — the leak + IDOR invariants hold on
    production-shaped data (a private repo file / overridden page never in any result incl. counts, not just the
    M2 synthetic corpus). 0 leak, 0 cross-tenant — CI (the gate-invariant ratchet: the M2 drills re-run on each
    new producer corpus).
  - SRCH-D5 reindex-parity green on a Git + KN corpus: cold == live incl. content-anchored line-ranges +
    sub-artifact granularity (the rebuilt searchable projection re-derives correctly through the owner's resolve)
    — CI/SCHED (small-to-moderate scale; supports the master-band GIT-D7/KN-D1 by proving the projection
    re-derives).
  - Code search v1 correctness: a symbol/path/literal/commit-message query returns the right blob/ref; a trigram
    substring/regex-lite query works; code identifiers tokenize via camel/snake — CI.
- **TESTS (required).** Integration tests against the real Git + KN producers: code indexed by symbol/path/
  literal/commit-message + trigram; KN blocks/pages indexed multilingual + the structured facets + JSONB
  GIN-scan; sub-anchors resolved at the right grain. A chained test: force-push a Git line-range others embed →
  the indexed line-range re-derives (content-anchored, never a stale raw line number) through a scoped reindex.
  The drill scenarios for SRCH-D1/SRCH-D3 (real corpora) + SRCH-D5 (Git+KN). No new mutation-core module (the
  engine is fixed) — state that the SRCH-P5/P6/P7/P9 mutation floors still hold on the real corpora.
- **DEFINITION OF DONE.** Search indexes the real Git (code search v1) + KN corpora; SRCH-D1/SRCH-D3 re-confirmed
  on real corpora; SRCH-D5 reindex-parity green on a Git+KN corpus (content-anchored line-ranges re-derive); the
  engine is unchanged; the SCIP/LSIF + GIN-projection-feeder floors are named (post-M4 / M5 SRCH-P12); the Search
  half of E2E-1 behaviour (a hit on a confidential issue resolves to a tombstone, 0 title/count leak) is proven
  in-context; tests pass; committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M3: Search code search v1 (Git) + Knowledge indexing. Body lists: 6.5/6.3/5.7/2.6
  wired to real Git + KN producers; SRCH-D1/SRCH-D3 re-confirmed on real corpora; SRCH-D5 Git+KN reindex-parity
  greened; the SCIP/LSIF + GIN-projection-feeder floors named (post-M4 / M5); the engine unchanged. Branch first;
  do not push unless asked. Co-Authored-By trailer.

---

### SRCH-P11 — Consumer corpora + the Issues Tier-3 board-escalation valve (byte-identical ACL pre-filter)

- **BAND.** M4.
- **ROADMAP MILESTONE.** S-M4 (planning/06-roadmaps/shared/search-and-indexing.md §2 "S-M4 — The consumer
  corpora + the Issues Tier-3 valve").
- **DEPENDS-ON.** SRCH-P10 (Git + KN corpora searchable; the engine green on real producers). The M4 CI + Issues
  + Chat producer prompts that ship Issues' IndexSpec + the board-valve OLTP-budget-escalation seam + the
  FieldType facets + order_key, CI's sealed log segments + the (job, step, byte-range) index (11.8), Chat's
  IndexSpec + the channel ReBAC fragment. The index resolves these to the CI/Issues/Chat M4 P-NNN. (AG-D4
  re-confirmed green is a band precondition, not a Search dependency — Search runs no untrusted code.)
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (Chat references any artifact); ../../external-insights/01-process-and-quality-doctrine.md
    §3, §7 (byte-identical semantics across the two ACL pre-filters — reconcile the contract, no drift).
  - Architecture: ../05-refined-shared-systems-architecture/search-and-indexing.md §4.2.4 (the Issues Tier-3
    board-escalation valve — when a board's filtered scan goes over its OLTP budget, the board compiles its query
    to a Search query(ast, viewer) that conjoins THE SAME Filter{set_expr} the OLTP board would have used → the
    board and Search apply byte-identical ACL pre-filter semantics; no leak, no N+1, on either tier), §4.4 (CI
    log search — the sealed segments), §4.7 (Chat multilingual), §4.6.1 (the measured projection-feeder
    promotion).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 6.1 (query — the Tier-3 valve
    consumer wires here), 6.3 (Issues/Chat IndexSpec; the measured promotion), 11.8 (the CI per-subject-DEK log
    segments + the (job, step, byte-range) index; details_ref = #step-<n>), 4.3 (the SetExpr the valve conjoins —
    byte-identical to the OLTP board's).
  - Roadmap: planning/06-roadmaps/shared/search-and-indexing.md §2 S-M4 (all bullets; the GIN-scan floor → the
    M5 projection-feeder follow-on) + §1.1 row 6.1 (the Tier-3 valve unblocked) + §1.2 row 11.8.
  - Drill source: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md SRCH-D1/SRCH-D3 (full five-producer
    corpus, ~356/358); the Chat search-as-non-member = 0 (the CHAT-D11 analog, master M4 exit gate).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-search service crate (engine
  UNCHANGED; the remaining producer corpora arrive + the Tier-3 valve consumer wires in):
  - Issues indexing + the Tier-3 board-escalation valve (6.1, §4.2.4): consume Issues' IndexSpec (the frozen
    FieldType facets, order_key columnar fast-field for sort). When an Issues board's filtered scan goes OVER its
    OLTP budget, the board compiles its query to a Search query(ast, viewer) that conjoins THE SAME
    Filter{set_expr} the OLTP board would have used (the SRCH-P5 SetExpr lowering, 4.3) — so the board and Search
    apply BYTE-IDENTICAL ACL pre-filter semantics. No leak, no N+1, on either tier. (This valve was unblocked by
    OQ-E in M2; its consumer wires in here.)
  - CI log search input (11.8): index the per-subject-DEK CI-log sealed segments / the (job, step, byte-range)
    index so details_ref (#step-<n>) resolves. CI logs ride the firehose for LIVE TAIL but Search consumes the
    DURABLE sealed segments (NOT the firehose — confirmation #11, no one wires Search onto the firehose).
  - Chat indexing (6.3): consume Chat's IndexSpec (message bodies as the markdown subset); search-as-non-member
    returns 0 results (the Chat ReBAC fragment channel.read = member + parent flows through list_objects — the
    CHAT-D11 analog, proven by SRCH-D1 on the Chat corpus).
  - Cross-subsystem facets dependable now that all five producers emit the structured inline nodes uniformly
    (X-2): mention/ref facets reliable across Git/KN/Issues/Chat.
  - FLOOR named: the GIN-scan custom-field path serves Issues board facets (the measured projection-feeder
    promotion per hot facet is the M5 follow-on, SRCH-P12, OQ-C — owner of the frequency signal is Issues/KN,
    Search consumes it and decides promotion). Named so the GIN scan is not mistaken for the final-cost answer.
- **CONTRACTS TO IMPLEMENT.** 6.1 query (the Tier-3 valve consumer now wired — Issues calls it), 6.3 (consume
  Issues/Chat IndexSpec), 11.8 (consume the CI sealed log segments + the (job, step, byte-range) index). To the
  frozen shapes; the engine does not change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - SRCH-D1 / SRCH-D3 green on the FULL five-producer corpus — the leak + IDOR invariants hold across Issues + CI
    logs + Chat (the most adversarial corpus: confidential issues, private channels, fork-scoped CI logs). 0
    leak, 0 cross-tenant — CI.
  - The Tier-3 valve parity check: the same board query run through the OLTP board path AND through the Search
    valve returns BYTE-IDENTICAL visible rows (0 leak divergence between the two ACL pre-filters) — CI (supports
    the master-band ISS-D2 board-query-<1s gate by giving it a leak-equivalent escalation path).
  - Chat search-as-non-member = 0 results on the Chat corpus (the SRCH-D1 instance, the CHAT-D11 analog) — CI.
- **TESTS (required).** Integration tests against the CI + Issues + Chat producers: Issues facets indexed +
  sortable by order_key; the Tier-3 valve escalates an over-budget board to Search with the same Filter; CI
  details_ref resolves via the sealed segments; Chat indexed; a non-member search returns 0. A chained test: run
  an over-budget Issues board through the OLTP path and the Search valve → assert byte-identical visible rows. The
  drill scenarios for SRCH-D1/SRCH-D3 (full corpus) + the valve parity + the Chat non-member. State the SRCH-P5/
  P6/P7/P9 mutation floors still hold on the full corpus.
- **DEFINITION OF DONE.** Search indexes the full five-producer corpus (Issues + CI logs + Chat); the Issues
  Tier-3 valve conjoins the same Filter as the OLTP board (byte-identical visible rows); SRCH-D1/SRCH-D3 green on
  the full corpus; Chat search-as-non-member = 0; the engine is unchanged; the GIN-scan → projection-feeder floor
  is named (M5 SRCH-P12); the Search half of E2E-1 lights up end-to-end in-context; tests pass; committed. No
  threshold weakened.
- **COMMIT.** Header: P-<NNN> M4: Search consumer corpora + the Issues Tier-3 valve. Body lists: 6.1 (the Tier-3
  valve consumer), 6.3 (Issues/Chat IndexSpec), 11.8 (CI sealed log segments) wired; SRCH-D1/SRCH-D3 greened on
  the full five-producer corpus; the valve byte-identical-rows parity proven; Chat search-as-non-member = 0; the
  GIN-scan → projection-feeder floor named (M5). Branch first; do not push unless asked. Co-Authored-By trailer.

---

### SRCH-P12 — World-scale: the 30x surge + filtered-ANN strategy + freshness + the projection-feeder promotion

- **BAND.** M5.
- **ROADMAP MILESTONE.** S-M5 partial (planning/06-roadmaps/shared/search-and-indexing.md §2 "S-M5", the SRCH-D6
  surge + SRCH-D7 freshness + SRCH-D8 filtered-ANN + the OQ-C projection-feeder promotion).
- **DEPENDS-ON.** SRCH-P11 (all five producer corpora searchable; the deterministic correctness drills green).
  The M5 storage read-node scaling + the surge harness numbers (OQ-K). The protected-human-lane shed order
  (1.11). The Issues/KN per-facet filter-frequency signal (OQ-C). The index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scalable from day 1); ../../external-insights/01-process-and-quality-doctrine.md §3
    (prove-it: the 1x/10x/30x load generator; observability), §2 (the protected human lane);
    ../../external-insights/04-hard-problems.md §5.
  - Architecture: ../05-refined-shared-systems-architecture/search-and-indexing.md §6.3 (the principal-aware shed
    lane — a human's interactive search holds the protected lane, agent/CI search sheds with 429 + Retry-After,
    per-tenant in-flight caps; the per-surface shed-budget floors OQ-K, numbers a P6 budget call tuned by drills),
    §4.2.2 (filter-during-traversal — the brute-force-fallback threshold under selective filters + the HNSW↔IVF-PQ
    promotion point, the M5 strategy follow-on D8; the property fixed in M2), §4.10 (the freshness budget D7),
    §4.6.1 (the projection-feeder promotion threshold > 5% of view executions, measured never predicted, OQ-C),
    §6.2 (measure before you shard — more index nodes per cell + the result cache first).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 1.11 (the protected-human-lane
    shed order + per-surface shed budgets OQ-K), 6.2 (the filtered-ANN traversal at scale), 6.3 (the measured
    projection-feeder promotion), 1.8 (the shed-count + freshness-p99 + recall telemetry).
  - Roadmap: planning/06-roadmaps/shared/search-and-indexing.md §2 S-M5 (the surge + filtered-ANN + freshness +
    projection-feeder bullets) + §3 (production-hardened).
  - Drill source: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md SRCH-D6 (~361), SRCH-D7 (~362),
    SRCH-D8 (~363).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-search service crate:
  - The 30x agent/CI query surge handling (SRCH-D6): tune the protected-human-lane shed order (1.11) to Search's
    query surface — a human's interactive search holds the PROTECTED lane; agent/CI search sheds with 429 +
    Retry-After (honoured); per-tenant in-flight caps keep one tenant's agent storm off another's humans (the
    per-tenant bulkhead). Set the per-surface shed-budget NUMBERS (OQ-K) from MEASUREMENT, not prediction; write
    them into the thresholds file.
  - The filtered-ANN strategy follow-on (SRCH-D8, the named M2 floor's follow-on): the brute-force-fallback
    threshold under very selective ACL/structured filters + the HNSW↔IVF-PQ promotion point. The PROPERTY (k
    visible neighbours, no leak) was fixed in M2 (SRCH-P7); the STRATEGY is measured here. Gate: recall@k ≥
    threshold under filter, no leak.
  - The freshness budget (SRCH-D7, full-scale): event → searchable p99 within the seconds-grade budget UNDER
    LOAD; index-lag alarms before user-visible staleness. The number is measured here (the M2 SRCH-P4 floor was
    the CI variant).
  - The measured projection-feeder promotion (OQ-C, §4.6.1): wire the per-facet filter-frequency signal from
    Issues/KN; promote a facet past > 5% of view executions from the GIN scan to a generated/columnar index. The
    threshold is set by MEASUREMENT; the GIN scan serves correctly meanwhile (promotion changes cost, never
    correctness).
  - Sharding/scaling edge IF measured (§6.2): the first scaling move is the result cache + more embedded-Tantivy
    index nodes per cell, then a per-subsystem index split for a hot tenant, then the OpenSearch-class upgrade
    behind the IndexBackend trait — a MEASURED-volume promotion, not premature (premature sharding is its own
    outage). State as a measured edge, not a built default.
  - FLOOR named: BM25 ranking (the learning-to-rank / semantic re-rank is post-M5, measured-gap-triggered);
    the real EU-hostable embedding model adapter (post-M5/runtime, a config swap; the vector math + erasure are
    done). Both named so the M5-hardened path is not mistaken for the final-model / final-ranking answer.
- **CONTRACTS TO IMPLEMENT.** Consumed: 1.11 the shed order + per-surface budgets (Search's query surface is one
  lane), 6.2 the filtered-ANN traversal (the strategy tuned), 6.3 the measured promotion (the OQ-C tail). To the
  frozen shapes; the §5 contracts are unchanged in shape — this is tuning + a promotion, not a redesign.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - SRCH-D6 (30x surge): the human search lane holds (interactive latency within budget), the agent lane sheds
    (429 + Retry-After honoured), other tenants unaffected. Green artifact: shed-counts/lane + search p99 —
    SCHED.
  - SRCH-D7 (freshness, full-scale): event → searchable p99 within the seconds-grade budget under load; the
    index-lag alarm fires first. Green artifact: the index-lag alarm + freshness p99 — SCHED.
  - SRCH-D8 (filtered-ANN recall): a selective filter → the k nearest VISIBLE neighbours; recall@k ≥ threshold,
    0 leak. Green artifact: recall@k + the zero-escape counter — SCHED.
  - The projection-feeder promotion: a facet crossing the measured > 5%-of-view-executions threshold is promoted
    from the GIN scan to a generated index; results stay correct across the promotion (cost changes, correctness
    does not) — SCHED.
- **TESTS (required).** The surge-harness scenario (SRCH-D6, the 1x/10x/30x load generator with mixed
  human/agent/CI principal kinds, per-tenant bulkhead). The filtered-ANN recall measurement (SRCH-D8, recall@k
  under a selective filter, 0 leak). The full-scale freshness measurement (SRCH-D7). A promotion test (a facet
  crossing the measured threshold → promoted, results unchanged). State the SRCH-P5/P7 mutation floors still
  hold. Record honestly (yes/no/partial) which numbers were measured vs. carried as defaults-to-beat.
- **DEFINITION OF DONE.** The 30x surge handling (protected human lane, agent sheds, per-tenant bulkhead), the
  tuned filtered-ANN strategy, the full-scale freshness budget, and the measured projection-feeder promotion
  exist and compile; SRCH-D6/SRCH-D7/SRCH-D8 each emit a dated green artifact (with the measured numbers written
  into the thresholds file); the promotion changes cost not correctness; the BM25-ranking + real-model floors are
  named (post-M5); tests pass; committed. No threshold weakened to manufacture green — a red gate becomes a dated
  "claimed, not proven" scorecard row.
- **COMMIT.** Header: P-<NNN> M5: Search 30x surge + filtered-ANN + freshness + projection-feeder promotion.
  Body lists: SRCH-D6 (human lane holds, agent sheds, per-tenant 0-impact), SRCH-D7 (freshness p99 measured),
  SRCH-D8 (recall@k ≥ threshold, 0 leak) greened; the projection-feeder promoted at the measured > 5% threshold;
  the measured OQ-K shed budgets written to the thresholds file; the BM25 + real-model floors named (post-M5).
  Branch first; do not push unless asked. Co-Authored-By trailer.

---

### SRCH-P13 — World-scale: restore + cross-seam + re-erase at scale + HYOK + the object-store backstop

- **BAND.** M5.
- **ROADMAP MILESTONE.** S-M5 partial (the SRCH-D9 restore + SRCH-D4-at-backup-scale + SRCH-D10 HYOK + the
  object-store index backstop).
- **DEPENDS-ON.** SRCH-P9 (the erase + reindex mechanism — the CI variant; here at scale), SRCH-P12 (the
  world-scale query surface). The M5 storage object-store BlobStore swap (11.2), restore-verify at cell scale
  (STOR-D2), the full DSR fan-out (10.4), the erasure ledger (10.8). The index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (data residency + erasure are architectural constraints);
    ../../external-insights/04-hard-problems.md §1 (erasure vs immutability — re-erasure after restore), §5.3
    (reindex-from-source); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it; the restore-
    verify gate is a CI job not an aspiration).
  - Architecture: ../05-refined-shared-systems-architecture/search-and-indexing.md §4.8 (the crypto-shred
    layering: per-tenant index DEK + per-subject source DEK backstop; HYOK structural skip), §4.9 (reindex-from-
    source post-restore re-erasure runs from the erasure ledger 10.8; no row↔doc↔vector mismatch), §3.4 / §6.2
    (the object-store backstop — the fs-backed BlobStore → object-store swap rides the M5 storage promotion;
    Search's index segments + immutable backstop move with it).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 10.1 (PersonalDataHolder erase at
    backup scale), 6.4 (reindex post-restore), 11.2 (the BlobStore object-store swap), 11.3 (HYOK
    can_derive_plaintext_index — the cross-store assertion), 10.8 (the erasure ledger driving post-restore
    re-erasure), 10.4 (the DSR fan-out).
  - Roadmap: planning/06-roadmaps/shared/search-and-indexing.md §2 S-M5 (the restore + HYOK + object-store
    backstop bullets; the full erasure proof folded into the DSAR fan-out) + §1.2 row 12.6 (cross-cell, designed-
    not-built here — SRCH-P14).
  - Drill source: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md SRCH-D9 (~364), SRCH-D4 (~359, at
    backup scale), SRCH-D10 (~365).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-search service crate:
  - Restore + cross-seam + re-erase at scale (SRCH-D9, F3): restore the index with OLTP/blob/offsets to a
    CONSISTENT point → no resurrected erased docs (post-restore re-erasure runs FROM the erasure ledger 10.8 via
    the live reindex path, SRCH-P9); no row↔doc↔vector mismatch. This is the permanent restore-verify gate
    (STOR-D1/D2 family) applied to Search's derived store.
  - HYOK at scale (SRCH-D10): mark a content class HYOK → Search skips it (can_derive_plaintext_index()=false,
    the SRCH-P9 structural skip); the CROSS-STORE assertion now runs at scale (jointly with Storage + Agent) —
    0 HYOK plaintext in ANY derived store (index segments, vectors, caches, backups).
  - The full erasure proof (SRCH-D4 at backup scale): every doc/field/VECTOR purged + unrecoverable INCL.
    BACKUPS (the per-tenant index DEK + per-subject source DEK backstop renders backup segments unrecoverable);
    this joins the M5 DSAR fan-out E2E-4 (SRCH-P14) — the holder-coverage receipt includes Search.
  - The object-store index backstop: the fs-backed BlobStore → object-store swap (11.2, the one-line floor swap)
    rides the M5 storage promotion; Search's index segments + immutable backstop move with it (residency-pinned,
    per-tenant-DEK-encrypted in the object store). A config/impl swap behind the BlobStore, not a rewrite.
  - FLOOR named: none new — these ARE the named floor follow-ons (the per-tenant index DEK → backup-scale erasure
    proof; the fs-backed BlobStore → object-store backstop). State that cross-cell federated search is the
    remaining S-M5 piece (SRCH-P14).
- **CONTRACTS TO IMPLEMENT.** 10.1 erase at backup scale + 6.4 reindex post-restore (owned, hardened). Consumed:
  11.2 the object-store BlobStore, 11.3 HYOK, 10.8 the erasure ledger, 10.4 the DSR fan-out. To the frozen
  shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - SRCH-D9 (F3, restore + cross-seam + re-erase): restore to a consistent point → 0 resurrected erased docs
    (re-erasure runs from the ledger); 0 row↔doc↔vector mismatch. Green artifact: restore-verify + re-erasure
    receipt — SCHED (a permanent gate, re-run on every store-touching change).
  - SRCH-D4 at backup scale: erase a subject → 0 recoverable personal data INCL. vectors INCL. backups. Green
    artifact: the embedding-purge receipt at backup scale (folded into E2E-4) — SCHED.
  - SRCH-D10 (HYOK): a HYOK content class → 0 HYOK plaintext in any derived store (index/vectors/caches/backups),
    the cross-store assertion with Storage + Agent — SCHED.
  - The object-store backstop swap: Search's index segments + immutable backstop live in the object store,
    residency-pinned + per-tenant-DEK-encrypted, with no behaviour change (a measured swap) — SCHED.
- **TESTS (required).** The restore-verify drill scenario (SRCH-D9: restore index+OLTP+blob+offsets to a
  consistent point, re-erasure from the ledger, no resurrected docs, no row↔doc↔vector mismatch). The
  backup-scale erasure scenario (SRCH-D4: 0 recoverable incl. vectors incl. backups). The HYOK cross-store
  scenario (SRCH-D10: 0 plaintext in any derived store). The object-store swap integration test (segments move,
  behaviour unchanged). Record honestly which were run at full backup scale vs. a scaled-down variant.
- **DEFINITION OF DONE.** Restore + cross-seam + re-erase at scale (SRCH-D9), the backup-scale erasure proof
  (SRCH-D4), HYOK cross-store (SRCH-D10), and the object-store index backstop swap exist and compile; each emits
  a dated green artifact (0 resurrected, 0 recoverable incl. vectors incl. backups, 0 HYOK plaintext, segments
  moved with no behaviour change); these are the named floor follow-ons; cross-cell is named as the remaining
  S-M5 piece (SRCH-P14); tests pass; committed. No threshold weakened — a red gate becomes a dated "claimed, not
  proven" row.
- **COMMIT.** Header: P-<NNN> M5: Search restore + re-erase at scale + HYOK + object-store backstop. Body lists:
  SRCH-D9 (0 resurrected, 0 row↔doc↔vector mismatch), SRCH-D4 backup-scale (0 recoverable incl. vectors incl.
  backups), SRCH-D10 (0 HYOK plaintext cross-store) greened; the fs→object-store BlobStore backstop swapped
  (behaviour unchanged); cross-cell named as the remaining S-M5 piece (SRCH-P14). Branch first; do not push
  unless asked. Co-Authored-By trailer.

---

### SRCH-P14 — Cross-cell federated search (designed-and-extends) + the whole-system E2E wedge (E2E-1/E2E-3/E2E-4)

- **BAND.** M5.
- **ROADMAP MILESTONE.** S-M5 partial (the cross-cell federated search + the E2E-1/E2E-3/E2E-4 rows Search
  crosses).
- **DEPENDS-ON.** SRCH-P12, SRCH-P13 (the world-scale-hardened single-cell Search). The M5 multi-cell bridge
  going live (12.6, OQ-I), the cross-cell PII-free pointer bridge. The E2E wedge prompts (the cross-subsystem
  harness for E2E-1/E2E-3/E2E-4). The index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scalable; EU-sovereign — residency-free merge crosses only ranking metadata, never
    PII); ../../external-insights/04-hard-problems.md §1 (residency); ../../external-insights/01-process-and-
    quality-doctrine.md §3, §4 (the whole-system chained-mutation E2E — drive the real thing end to end).
  - Architecture: ../05-refined-shared-systems-architecture/search-and-indexing.md §6.4 (cross-cell federated
    search — designed-not-built: scatter-gather, each cell runs the SAME permission-filtered query locally over
    its own index/list_objects/residency; a residency-free merge fuses only ranking metadata + ArtifactRefs,
    never payload/PII; rows resolved PER-VIEWER in their home cell; the §5 contracts are cell-agnostic so this
    extends without a rewrite; built only when multi-cell goes live in M5), §1 ("Floors named up front" (c)).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-I (the cross-cell
    PII-free pointer bridge frame), X-7 (the erasure posture E2E-4 proves).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 12.6 (the cross-cell PII-free
    pointer bridge — resolution always cell-local, only the projection crosses), 6.1/6.2 (query/semantic, now
    cross-cell-extended), 5.6 (project — the per-viewer home-cell resolution), 10.1/10.4 (the DSR fan-out for
    E2E-4).
  - Roadmap: planning/06-roadmaps/shared/search-and-indexing.md §2 S-M5 (the cross-cell + E2E bullets; the
    single-cell → cross-cell floor) + §3 (production-hardened) + §4 (the two cardinal invariants the E2E re-runs).
  - Drill source: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md the E2E scenarios E2E-1 (PR context
    pane), E2E-3 (spec-to-ship / reindex-parity), E2E-4 (DSAR fan-out); SRCH-D1 in-context (E2E-1), SRCH-D5 at
    scale (E2E-3), SRCH-D4 at backup scale (E2E-4).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-search service crate + the E2E
  harness:
  - Cross-cell federated search (§6.4), built only WHEN multi-cell goes live (12.6): scatter-gather — each cell
    runs the SAME permission-filtered query LOCALLY over its own index / list_objects / residency; a residency-
    free merge fuses ONLY ranking metadata + ArtifactRefs (never payload/PII) at the control-plane boundary;
    result rows are resolved PER-VIEWER in their HOME CELL over the cross-cell PII-free pointer bridge (12.6).
    The single-cell path was complete from M2; the §5 contracts are cell-agnostic so this EXTENDS without a
    rewrite. The leak-free property holds: each cell applies its own list_objects pre-filter; no payload crosses.
  - The whole-system E2E wedge rows Search crosses (each emitting its named green artifact):
    - E2E-1 (PR context pane): a Search hit on a CONFIDENTIAL issue resolves to a tombstone — 0 title leak, 0
      count-leak (SRCH-D1 in-context across Git+CI+Issues+Knowledge+Refs+Id+Notif).
    - E2E-3 (spec-to-ship traceability): the WIPED Search index reindexes to BYTE-MATCH live (F4 / SRCH-D5 at
      scale; cold-reindex == live; audit tamper detected).
    - E2E-4 (DSAR fan-out): Search's docs + EMBEDDINGS return 0 recoverable PII; the holder-coverage receipt
      INCLUDES Search (0 holders missed; 0 recoverable PII incl. vectors incl. backups; certificate sealed).
  - FLOOR named: none new — cross-cell federated search is the named single-cell follow-on, and these are the M5
    E2E wedge rows. State that the cross-cell BUILD is gated on multi-cell going live; until then the single-cell
    path is complete and the design holds (designed-and-extends).
- **CONTRACTS TO IMPLEMENT.** Consumed: 12.6 the cross-cell PII-free pointer bridge (the scatter-gather +
  residency-free merge rides it; resolution cell-local), 5.6 the per-viewer home-cell project, 10.4 the DSR
  fan-out (E2E-4). 6.1/6.2 query/semantic extended cross-cell (no shape change — the §5 contracts are
  cell-agnostic). To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - Cross-cell leak-free: a federated query across two cells returns only the viewer's visible rows, resolved
    per-viewer in their home cell; the residency-free merge carries only ranking metadata + ArtifactRefs, NEVER
    payload/PII; 0 cross-cell leak, 0 PII crossing the merge boundary — SCHED.
  - E2E-1 green (Search row): a hit on a confidential issue resolves to a tombstone, 0 title/count leak — SCHED.
  - E2E-3 green (Search row): the wiped index reindexes to byte-match live (SRCH-D5 at scale) — SCHED.
  - E2E-4 green (Search row): Search's docs + embeddings return 0 recoverable PII incl. vectors incl. backups;
    the holder-coverage receipt includes Search — SCHED.
- **TESTS (required).** The cross-cell scatter-gather integration test (two cells, per-viewer home-cell
  resolution, residency-free merge, 0 PII crossing). The three E2E scenario harness drives Search crosses
  (E2E-1/E2E-3/E2E-4), each chaining mutations across the real subsystems with mock agents. Record honestly which
  E2E rows were driven end-to-end vs. a scaled-down variant. State the SRCH-P5 (leak) + SRCH-P9 (erase) mutation
  floors hold under the E2E.
- **DEFINITION OF DONE.** Cross-cell federated search extends the single-cell path without a rewrite (scatter-
  gather, residency-free merge, per-viewer home-cell resolution, 0 PII crossing); the three E2E wedge rows Search
  crosses (E2E-1 tombstone 0 leak, E2E-3 reindex byte-match, E2E-4 DSAR 0 recoverable incl. vectors) each emit a
  dated green artifact; the single-cell → cross-cell floor is fulfilled; tests pass; committed. This completes
  the master M5→M6 boundary's Search rows. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M5: Search cross-cell federated search + the E2E wedge (E2E-1/E2E-3/E2E-4). Body
  lists: cross-cell scatter-gather + residency-free merge (per-viewer home-cell, 0 PII crossing); E2E-1
  (tombstone 0 leak), E2E-3 (reindex byte-match), E2E-4 (DSAR 0 recoverable incl. vectors) greened; cross-cell
  designed-and-extends fulfilled. Branch first; do not push unless asked. Co-Authored-By trailer.

---

### SRCH-P15 — Dogfooding: Search over Myelin's own work + the switch test + the self-hosting CI graph

- **BAND.** M6.
- **ROADMAP MILESTONE.** S-M6 (planning/06-roadmaps/shared/search-and-indexing.md §2 "S-M6 — Dogfooding: Search
  over Myelin's own work").
- **DEPENDS-ON.** SRCH-P14 (Search world-scale-ready: restore + re-erase + DSAR fan-out green; the E2E wedge
  proven). The M6 self-hosting prompts (Myelin git hosting + the self-hosting CI graph + the Myelin Knowledge
  space + the Myelin issues/chat). The index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (the switch test — top-of-the-line UX; could a user move without hitting a wall);
    ../../external-insights/01-process-and-quality-doctrine.md §4 (actually try it — drive the real UI in a
    browser; the switch test is reached by driving it, not reading the feature list), §1 (code-wins-over-docs —
    the truth-up pass).
  - Architecture: ../05-refined-shared-systems-architecture/search-and-indexing.md §3 (the honest progression —
    production-hardened before real self-tenant data), §7 (the nine drills now run as Myelin CI jobs).
  - Roadmap: planning/06-roadmaps/shared/search-and-indexing.md §2 S-M6 (the work + the gate) + §3 (production-
    hardened before M6).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-search service crate + the Myelin
  self-hosting deployment config:
  - Run Search over Myelin's own work: code search on the Myelin monorepo, search over its own Knowledge space
    (the roadmap/gap-report/scorecard docs), its own issues, its own chat. The builders drive real cross-artifact
    search IN A BROWSER. The Search drills run as Myelin CI jobs on Myelin's own commits (the dogfood loop).
  - The Search contribution to the per-subsystem SWITCH TESTS (folded into the L5 done-bars): could a
    GitHub/Notion/Jira user FIND what they expect — code by symbol, a doc by content, an issue by facet — without
    hitting a wall the old tool didn't have? Reached by DRIVING THE REAL UI in a browser, measured against the
    latency budgets (interactive search within the keyboard / no-spinner-flash budget), not by reading the
    feature list.
  - FLOOR named: none new — M6 promotes nothing; it exercises the production-hardened Search on real (self-)tenant
    data. (State that the real EU-hostable embedding model adapter swap remains the post-M5/runtime follow-on —
    M6 may run on the mock or an early real adapter; record which honestly.)
- **CONTRACTS TO IMPLEMENT.** None new — the engine is fixed at M2 and hardened through M5. This prompt exercises
  the production surface (6.1/6.2/6.4/6.5) on real self-tenant data and wires the Search drills into the Myelin
  self-hosting CI graph.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - Search is green on the self-hosting CI graph (the Search drills SRCH-D1..D10 run as Myelin CI jobs on
    Myelin's own commits — the dogfood loop is live) — SCHED.
  - The search switch-test surfaces pass when driven in a browser: code-by-symbol / doc-by-content /
    issue-by-facet found within the measured latency budget, no wall the old tool didn't have — SCHED.
  - The truth-up pass: every Search PROVEN row (SRCH-D1..D10 + the E2E rows) rests on a DATED green artifact,
    never a doc claim — no earlier-band Search gate is red (code-wins-over-docs, EI-01 §1) — SCHED.
- **TESTS (required).** The switch-test browser drive (code-by-symbol / doc-by-content / issue-by-facet across
  the five real subsystems on Myelin's own data, against the latency budgets). The Search drills wired as Myelin
  CI jobs (the dogfood loop). A truth-up audit script confirming every Search PROVEN row links a dated green
  artifact. Record honestly (yes/no/partial) which switch-test surfaces were driven in a browser vs. only
  automated, and whether M6 ran on the mock or a real embedding adapter.
- **DEFINITION OF DONE.** Search runs over Myelin's own work; the Search drills are green as Myelin CI jobs; the
  switch-test surfaces pass when driven in a browser (measured latency); the truth-up pass confirms no
  earlier-band Search gate is red (every PROVEN row dated-and-green); any surface only-automated-not-browser-
  driven, and the embedding-adapter posture (mock vs real), are named honestly; committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M6: Search dogfooded on Myelin's own work. Body lists: the Search drills green as
  Myelin CI jobs (the dogfood loop); the switch-test surfaces driven in a browser (measured latency, code-by-
  symbol / doc-by-content / issue-by-facet); the truth-up pass (0 red earlier-band Search gates); the
  embedding-adapter posture (mock/real) named. Branch first; do not push unless asked. Co-Authored-By trailer.

---

## Coverage check (every S-M milestone -> its prompt(s))

| Roadmap milestone | Band | Prompt(s) |
|---|---|---|
| S-M0 (the search-requires-acl-filter lint + the index-doc name anchors) | M0 | SRCH-P1 |
| S-M1 (Search as a holder + the index encryption floor) | M1 | SRCH-P2 |
| S-M2 (the Search core — engine, indexer, query pipeline, AST compiler, hybrid+vector, analysis/caches/telemetry, erasure+reindex) | M2 | SRCH-P3, SRCH-P4, SRCH-P5, SRCH-P6, SRCH-P7, SRCH-P8, SRCH-P9 |
| S-M3 (code search v1 + Knowledge indexing + sub-artifact projections) | M3 | SRCH-P10 |
| S-M4 (consumer corpora + the Issues Tier-3 valve) | M4 | SRCH-P11 |
| S-M5 (30x surge + filtered-ANN + freshness + projection-feeder; restore+re-erase+HYOK+object-store; cross-cell + the E2E wedge) | M5 | SRCH-P12, SRCH-P13, SRCH-P14 |
| S-M6 (dogfooding) | M6 | SRCH-P15 |

**Floor -> follow-on pairing (name-your-floors, EI-01 §1 / master §5):**
- per-tenant index DEK (SRCH-P2) -> the primary per-subject erasure by purge + reindex (SRCH-P9).
- engine shell (SRCH-P3) -> the indexer (SRCH-P4) + the query path (SRCH-P5) that make it answer.
- mock embedding adapter (SRCH-P4) -> the real EU-hostable model adapter (post-M5/runtime config swap, named in
  SRCH-P12).
- synthetic/test producer + the IndexSpec API frozen (SRCH-P4) -> real per-subsystem IndexSpecs (Git/KN
  SRCH-P10; Issues/CI/Chat SRCH-P11).
- BM25 default ranking (SRCH-P5) -> learning-to-rank / semantic re-rank (post-M5, measured-gap-triggered, named
  in SRCH-P12).
- filter-during-traversal as the recall mechanism (SRCH-P7) -> the tuned filtered-ANN strategy / HNSW↔IVF-PQ
  promotion (SRCH-P12, drill D8).
- the EU language set + CJK strategy (SRCH-P8) -> the [OPEN -> P6] full analyzer set (named in the gap report).
- code search v1 symbol/path/literal + trigram (SRCH-P10) -> SCIP/LSIF "find usages" (post-M4, demand-triggered,
  Git+CI joint input).
- GIN-indexed JSONB facet scan (SRCH-P10, SRCH-P11) -> the generated projection-feeder index promoted at the
  measured > 5% of view executions (SRCH-P12, OQ-C).
- CI-variant SRCH-D4/SRCH-D5 (SRCH-P9) -> full-scale / backup-scale SRCH-D4/SRCH-D5 (SRCH-P13, SRCH-P14 E2E).
- fs-backed BlobStore (SRCH-P2, SRCH-P3) -> object-store index backstop (SRCH-P13).
- single-cell Search (SRCH-P3..SRCH-P11) -> cross-cell federated search, designed-and-extends (SRCH-P14).

**Drill coverage (every SRCH-D greened by some prompt's GATE/DRILLS):** SRCH-D1 (SRCH-P5 query half + SRCH-P7
vector/RAG half, re-confirmed SRCH-P10/SRCH-P11 real corpora, in-context E2E-1 SRCH-P14), SRCH-D2 (SRCH-P7),
SRCH-D3 (SRCH-P5, re-confirmed SRCH-P10/SRCH-P11), SRCH-D4 (SRCH-P9 CI variant + SRCH-P13 backup-scale + SRCH-P14
E2E-4), SRCH-D5 (SRCH-P9 CI variant + SRCH-P10 Git+KN + SRCH-P14 E2E-3 at scale), SRCH-D6 (SRCH-P12), SRCH-D7
(SRCH-P4 CI floor + SRCH-P12 full-scale), SRCH-D8 (SRCH-P12), SRCH-D9 (SRCH-P13), SRCH-D10 (SRCH-P9 structural
skip + SRCH-P13 cross-store at scale); plus the search-requires-acl-filter lint (SRCH-P1, re-asserted every
query prompt), the holder-registry structural check (SRCH-P2), E2E-1/E2E-3/E2E-4 (SRCH-P14), and the switch
tests + the self-hosting CI graph (SRCH-P15).
