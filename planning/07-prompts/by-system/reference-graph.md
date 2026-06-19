# Phase 7 — Prompt Ledger: Cross-Artifact Reference Graph (myelin-refs)

> Prompt count: 15 (first pass, Phase 7-A pass 1) -> 29 (this finer-grained pass, Phase 7-A pass 2). Every
> bundled multi-deliverable prompt is split into single-deliverable clean-context units; all coverage
> (milestones R-M0..R-M6, contracts 5.1..5.9 + 10.1 + the consumed rows, drills REF-D1..REF-D10 + GA-D8/CP-D7/
> CP-D8 + E2E-1/E2E-3/E2E-4, every named floor) is preserved at finer granularity. See the coverage check at
> the foot of the file.

> Phase: 07-prompts (per-system file, Phase 7-A). The complete ordered set of implementation prompts that
> operationalize the entire reference-graph roadmap (planning/06-roadmaps/shared/reference-graph.md, milestones
> R-M0..R-M6) into clean-context, independently-committable coding tasks. Built to the template in
> planning/07-prompts/00-ledger-overview.md §2 (every field present, never implicit) and banded to
> planning/06-roadmaps/00-master-sequencing.md §2 (M0..M6, the gate invariant). Frozen architecture (this file
> OPERATIONALIZES, it does not redesign): planning/05-refined-shared-systems-architecture/reference-graph.md +
> contract-index.md §5 + 00-reconciliation-decisions.md (C-1..C-6, X-1/X-2/X-4/X-7, OQ-D/OQ-E/OQ-I/OQ-K).
> Plain-text identifiers throughout (no backticks-as-emphasis). Markdown only; this file makes no commits.
> Date: 2026-06-19.
>
> The global P-NNN ids are assigned by the consolidated ledger index (Phase 7-B, 01-ledger-index.md) when these
> per-system prompts are interleaved into the single execution order. Here each prompt carries a stable local
> handle REF-P<n> so its DEPENDS-ON edges are unambiguous before global numbering; the index rewrites REF-P<n>
> to its P-NNN. Where a prompt depends on another system's prompt not yet numbered, it names that system's
> milestone (the index resolves it to the P-NNN).
>
> Coverage: R-M0 -> REF-P1, REF-P2; R-M1 -> REF-P3, REF-P4; R-M2 -> REF-P5..REF-P16 (twelve slices, each its
> own green gate); R-M3 -> REF-P17, REF-P18; R-M4 -> REF-P19, REF-P20, REF-P21; R-M5 -> REF-P22..REF-P27;
> R-M6 -> REF-P28, REF-P29. Twenty-nine single-deliverable prompts (REF-P1..REF-P29), no milestone gap.
>
> **Note on numbering.** This finer pass renumbers the per-file local ids contiguously REF-P1..REF-P29 (the
> first pass used REF-P1..REF-P15). The mapping first-pass -> finer is recorded in the coverage check at the
> foot of the file so no first-pass deliverable is lost in the renumber.

---

### REF-P1 — Ship the myelin-refs glue crate: the ArtifactRef value type + the Issues key + the frozen #sub grammar

- **BAND.** M0.
- **ROADMAP MILESTONE.** R-M0 (planning/06-roadmaps/shared/reference-graph.md §2 "R-M0 — The ArtifactRef value
  type + the Refs ratchet", the value-type half).
- **DEPENDS-ON.** The M0 substrate prompts that lay down the Cargo workspace + the eight glue-crate skeletons
  (master §2 M0; substrate roadmap SUB-M0) and freeze EventEnvelope (2.1) + the ArtifactRef token table
  (Bus §6.2). The index slots this after those workspace-bootstrap prompts; REF-P1 ships the myelin-refs crate
  body into that skeleton. (The four Refs lints are split into REF-P2 — this prompt is the value type only.)
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md (always) §1 (the reference graph as connective tissue), §3 (name-your-floors,
    code-wins-over-docs); ../../external-insights/01-process-and-quality-doctrine.md §1 (name-your-floors).
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §3.1 (the URN ArtifactRef + the
    frozen Issues key grammar C-3), §3.5 (the unified #sub grammar, the complete v1 vocabulary, C-1/C-6), §4.8
    (display keys render-time only, REF-3).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md C-1 (#sub grammar
    frozen), C-3 (Issues <PROJECTKEY>-<seqno> stored canonical key), C-6 (check-/step- first-class #sub kinds),
    X-2 (the three content nodes byte-identical).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 5.1 (ArtifactRef parse/format,
    Issues key frozen); row 2.9 (the <subsystem>/<type> token table owned by Bus §6.2 — Refs is validator, not
    author).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M0 (the work + the gate) and §1.1 row 5.1.
- **DELIVERABLE (what to build + exactly where in the repo).** In the glue crate myelin-refs (the M0 skeleton):
  - The ArtifactRef value type: myelin://<tenant>/<subsystem>/<type>/<id>[#sub], with parse(&str) ->
    Result<ArtifactRef> and format(&ArtifactRef) -> String. parse enforces total explicit scope (tenant,
    subsystem, type, id all required) and rejects ambiguity: a scope-less / short-hash ref (#42, @alice,
    ~general, a 7-char prefix) is rejected, never guessed. format(parse(s)) round-trips canonically.
  - The Issues key grammar <PROJECTKEY>-<seqno> (e.g. ENG-1421) is the stored canonical <id> segment; #1421 is
    NOT parseable as a scope (it is a render-time display projection — assert it is rejected by parse).
  - The frozen #sub grammar vocabulary as the parse/format target: comment-<id>, thread-<id>, message-<id>,
    b<id>, h<id>, row-<id>, field-<id>, L<start>-L<end>, check-<context>, step-<n>. The kind prefix is
    self-describing; an unknown/ambiguous #sub kind is rejected. Provide strip_sub(&ArtifactRef) -> ArtifactRef
    (the #sub-stripped root) and the sub-kind accessor.
  - Validate (do NOT author) the <subsystem>/<type> token set + the initiative type token +
    ci.check.updated/ci.result tokens as the parse vocabulary, sourced from the Bus §6.2 token table.
  - FLOOR named: none — this is the contract value type, not the engine. State in the module doc that the value
    type is complete at M0 but the resolver over it is the R-M2 follow-on (REF-P9), and the four lints it leans
    on land in REF-P2, so the value type is not mistaken for the working graph.
- **CONTRACTS TO IMPLEMENT.** 5.1 ArtifactRef parse/format (owned, the value-type half; the resolve half lands
  in REF-P9). Consumed-as-validator: 2.9 the token table (Refs validates, never authors). Implement to the
  frozen signatures — a needed shape change is a whole-workspace contract PR, escalated and written down, not a
  local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The myelin-refs crate compiles and is linked by the workspace; a change to ArtifactRef breaks every
    consumer's build now (the ADR-01 compile-time-carrier property) — CI.
  - parse/format round-trip + ambiguity-rejection: a fuzz corpus of malformed / short-hash / ambiguous /
    unknown-#sub-kind URNs is rejected (0 guessed scopes); every well-formed URN round-trips byte-identical
    (format(parse(s)) == canonical(s)) — CI.
  - The contract-coverage scanner passes on the myelin-refs row 5.1 (a provider+consumer CDC stub) — CI.
- **TESTS (required).** Unit tests for parse/format on every #sub kind + the Issues key + the rejection cases.
  A property/fuzz test for ambiguity-rejection (no input ever yields a guessed scope). The provider+consumer CDC
  stub for contract row 5.1. myelin-refs is a mandatory-core glue crate: state the cargo-mutants mutation-score
  floor for the parse module in this field and meet it.
- **DEFINITION OF DONE.** myelin-refs compiles in the workspace and is linked by consumers; parse/format + the
  frozen #sub grammar implement contract 5.1's frozen shape; the fuzz/property + unit + CDC tests pass; the
  contract-coverage scanner is green on 5.1; the floor note (lints REF-P2, resolver REF-P9) is written in the
  module doc; the parse mutation-score floor is met; the work is committed. No gate is greened by weakening a
  threshold.
- **COMMIT.** Header: P-<NNN> M0: myelin-refs glue crate — ArtifactRef + Issues key + #sub grammar. Body lists:
  contract 5.1 (value-type half) implemented; the parse mutation-score measured; the floor named (lints REF-P2,
  resolver REF-P9). Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By
  trailer.

---

### REF-P2 — Wire the four Refs lints into CI with red+green fixtures (the M0 ratchet)

- **BAND.** M0.
- **ROADMAP MILESTONE.** R-M0 (planning/06-roadmaps/shared/reference-graph.md §2 "R-M0", the lint-ratchet half).
- **DEPENDS-ON.** REF-P1 (the myelin-refs crate exists to attach Refs-specific lint fixtures to). The M0
  substrate prompt that ships the twelve-lint framework centrally (1.6; substrate SUB-M0). The index slots this
  after that lint-framework prompt.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (code-wins-over-docs); ../../external-insights/01-process-and-quality-doctrine.md §5 (the
    ratchet / committed gates — an uncommitted lint is no lint), §1 (name-your-floors).
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §3 (the structural enforcement
    Refs leans on — tenant predicate, outbox-only emit, no owner-DB read, acyclic cross-subsystem edges).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md the four lint rows of 1.6 this prompt
    wires (tenant-predicate, no-raw-publish, no-cross-db, no-cross-sync-cycle).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M0 (the "lean on the four committed lints"
    bullet) + §1.2 row 1.6.
- **DELIVERABLE (what to build + exactly where in the repo).** In the workspace lint configuration + the
  myelin-refs crate's lint fixtures:
  - Wire the four lints Refs leans on into CI, each with a red-fixture (proves it rejects) + a green-fixture
    (proves it admits), loud-never-swallowed (no "... || true"): tenant-predicate (no cross-tenant edge query
    compiles — every edge query carries the tenant predicate, ID-3), no-raw-publish (no edge escapes the outbox
    — there is no standalone edge-write API, 5.4), no-cross-db (Refs never reads an owner's DB — only
    project/events), no-cross-sync-cycle (no synchronous cross-subsystem call cycle — every cross-subsystem edge
    is an async event/projection, the acyclicity rule).
  - If the M0 substrate prompt already ships these four lints centrally, this prompt instead adds the
    Refs-specific red+green fixtures (a tenant-less edge query, a standalone edge-write attempt, an owner-DB
    read, a synchronous cross-subsystem cycle) and confirms they are wired loud; name in the commit which case
    applies.
  - FLOOR named: none — these are permanent ratchet gates, not a feature. State that every later Refs prompt's
    DEFINITION OF DONE requires these four green.
- **CONTRACTS TO IMPLEMENT.** 1.6 the four lints (wired as permanent CI gates). To the frozen lint contract.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The four lints (tenant-predicate, no-raw-publish, no-cross-db, no-cross-sync-cycle) green with BOTH fixtures
    each, wired into CI, loud, never "|| true" — CI (these are permanent ratchet gates; say so).
- **TESTS (required).** The red+green fixture pair for each of the four lints (each lint proven to reject its
  red fixture and to admit its green fixture). No mutation floor (lint config, not a core module) — state so.
- **DEFINITION OF DONE.** The four lints emit dated green artifacts with both fixtures, wired loud into CI; the
  fixtures pass; the permanent-ratchet note is written; committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M0: the four Refs lints wired with red+green fixtures. Body lists: the four lints
  greened with red+green fixtures (and whether they were added Refs-specific or confirmed central). Branch
  first; do not push unless asked. Co-Authored-By trailer.

---

### REF-P3 — Register Refs as a PersonalDataHolder (stub surface) + confirm residency-pin

- **BAND.** M1.
- **ROADMAP MILESTONE.** R-M1 (planning/06-roadmaps/shared/reference-graph.md §2 "R-M1 — Refs as a holder + the
  edge-index encryption floor", the holder-registration half).
- **DEPENDS-ON.** REF-P1 (the myelin-refs crate exists). The M1 Identity/Storage/GDPR prompts that ship the
  holder harness auto-registration (contract 1.4) and the residency-pin (tenancy 12.x). The index places this
  after those M1 substrate prompts. (The per-tenant DEK pin is split into REF-P4.)
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe by construction; name-your-floors);
    ../../external-insights/04-hard-problems.md §1 (erasure vs immutability — the pseudonymous-id posture);
    ../../external-insights/01-process-and-quality-doctrine.md §1 (name-your-floors).
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §3 (tables are (tenant, region)
    first, RLS, holder auto-registered), §3.6 (the projection cache as a holder), §4.6 tail (the small
    structural erasure surface; the residual instantiated by reference to 10.9).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-7 (the one
    free-text/immutable erasure posture — Refs adds no new residual).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 10.1
    (PersonalDataHolder{locate/export/rectify/restrict/erase}); 1.4 (harness holder auto-registration); 10.9
    (the one residual posture).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M1 + §1.1 row (10.1).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs implementation crate (the Refs
  service crate, not the glue crate):
  - Register Refs as a PersonalDataHolder via the harness auto-registration (1.4) so the H1..H18 holder list is
    exhaustive before any real tenant data exists (10.1). At M1 the holder is a STUB — no edge index exists yet
    to purge — but it is on the list, so the M5 DSAR fan-out cannot silently miss it. Implement the holder trait
    surface against the future edge index + R2 cache (locate/export/restrict/erase return empty-but-correct
    now).
  - State in writing that Refs' erasure surface is small and structural: only pseudonymous opaque ids
    (origin_actor) + cache titles, never third-party free-text bodies — so Refs instantiates the one platform
    residual posture (10.9 / X-7) BY REFERENCE and adds NO new [OPEN — LEGAL] residual.
  - Confirm the residency-pin applies to the (future) per-tenant edge table + R2 cache: all Refs state is
    cell-local, (tenant, region)-partitioned, no cross-tenant query path. The residency-pin + tenant-predicate
    lints (REF-P2) already enforce it structurally — assert the Refs crate links them.
  - FLOOR named: the holder is a stub surface now; the structural erasure surface (R2-cache PII purge + reliance
    on Id's pseudonym shred for origin_actor + *.erased tombstoning) is the follow-on, landing in REF-P15 once
    the index exists. The per-tenant DEK that makes that surface crypto-shred-able lands in REF-P4. Write this so
    the stub is not mistaken for the whole erasure answer.
- **CONTRACTS TO IMPLEMENT.** 10.1 PersonalDataHolder (owned by Refs as a holder; stub surface now, real erase
  in REF-P15) — wired to the harness 1.4 auto-registration. Consumed: the residency-pin (tenancy). To the frozen
  shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - Refs appears in the harness-generated holder registry — 0 stores unregistered; the contract-coverage scanner
    confirms 10.1 coverage — CI (structural).
  - The residency-pin + tenant-predicate lints are linked by the Refs crate (no cross-tenant query path) — CI.
- **TESTS (required).** Unit test: the holder stub surface returns empty-but-correct locate/export for a tenant
  with no edges. A structural test asserting Refs is in the holder registry. The provider+consumer CDC pair for
  the Refs side of 10.1. No drill greens here (the engine is R-M2) — record this surface as
  untested-at-runtime-but-named (the real erase is REF-P15), honestly.
- **DEFINITION OF DONE.** Refs is registered as an exhaustive-list holder (0 unregistered); residency-pin
  confirmed structurally; the floor (stub now; structural erasure REF-P15; DEK REF-P4) is named in writing; the
  no-new-residual posture is recorded; the holder CDC pair passes; committed.
- **COMMIT.** Header: P-<NNN> M1: Refs PersonalDataHolder registration + residency-pin. Body lists: 10.1 holder
  stub registered (exhaustive-list); residency-pin confirmed; the floor named (DEK REF-P4, structural erasure
  REF-P15); no new [OPEN — LEGAL] residual. Branch first; do not push unless asked. Co-Authored-By trailer.

---

### REF-P4 — Pin the per-tenant DEK for the edge index + R2 cache into the KMS hierarchy

- **BAND.** M1.
- **ROADMAP MILESTONE.** R-M1 (planning/06-roadmaps/shared/reference-graph.md §2 "R-M1", the encryption-floor
  half).
- **DEPENDS-ON.** REF-P3 (the Refs holder + service crate exist). The M1 Storage/GDPR prompts that ship the KMS
  hierarchy (11.3/11.4). The index places this after those M1 KMS prompts.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe by construction; name-your-floors);
    ../../external-insights/01-process-and-quality-doctrine.md §1 (name-your-floors).
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §3 (per-tenant DEK on the edge
    table + R2 cache), §3.6 (the R2 cache may hold a name in a title — the per-subject DEK backstop).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 11.3/11.4 (KMS hierarchy:
    per-cell root -> per-tenant KEK -> per-tenant DEK; per-subject DEK backstop, crypto-shred).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M1 (the per-tenant DEK bullet) + §1.2 rows
    11.3/11.4.
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate:
  - Pin the per-tenant DEK for the (future) edge index + R2 cache into the KMS hierarchy (11.3): per-cell root ->
    per-tenant KEK -> per-tenant DEK as the tenant-decommission crypto-shred unit; reserve the per-subject DEK
    (11.4) backstop for a name landing in a cached title. No index exists yet — this reserves the key class so
    R-M2's index is encrypted-from-birth, and confirms destroy is callable on the key class.
  - FLOOR named: per-tenant DEK (the crypto-shred + backup-backstop unit) is THE floor; the structural erasure
    surface that USES the DEK is the follow-on, landing in REF-P15 once the index exists. Write this so the DEK
    is not mistaken for the whole erasure answer.
- **CONTRACTS TO IMPLEMENT.** Consumed: 11.3/11.4 (the KMS key class — Refs reserves the per-tenant DEK class +
  per-subject backstop). To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The per-tenant Refs DEK is a destroyable key in the KMS hierarchy: the key class exists and destroy is
    callable (proven fully later by REF-D5 in REF-P15/REF-P25; here the check is structural) — CI (structural).
  - Refs' M1 work does not begin its M2 engine over a red STOR-D1/STOR-D2 (restore-verify), ID-D3 (cross-tenant
    0), ID-D2 (fail-static), ID-D1 (disabled-user N=5 min), CP-D2/CP-D3 (misroute + residency-pin): name these
    inherited M1 platform gates as the precondition for REF-P5. Refs does not re-prove them; it cannot build the
    edge index over a red STOR-D1 — DEPENDS-ON makes this concrete.
- **TESTS (required).** A structural test asserting the per-tenant DEK class is destroyable (destroy callable on
  the key class). No drill greens here (the engine is R-M2) — record honestly that the DEK is reserved but the
  real crypto-shred is REF-P15/REF-P25.
- **DEFINITION OF DONE.** The per-tenant DEK class is reserved + destroyable; the inherited M1 platform gates are
  named as the REF-P5 precondition; the floor (DEK now; structural erasure REF-P15) is named in writing;
  committed.
- **COMMIT.** Header: P-<NNN> M1: Refs per-tenant DEK + KMS hierarchy pin. Body lists: the per-tenant DEK class
  reserved/destroyable (per-subject backstop reserved); the inherited M1 gates named as the REF-P5 precondition;
  the floor named (structural erasure follow-on REF-P15). Branch first; do not push unless asked. Co-Authored-By
  trailer.

---

### REF-P5 — The edge inverse-index schema migration

- **BAND.** M2.
- **ROADMAP MILESTONE.** R-M2 slice 1 of 12 (planning/06-roadmaps/shared/reference-graph.md §2 "R-M2 — The Refs
  core", the "edge inverse-index schema" deliverable).
- **DEPENDS-ON.** REF-P1, REF-P3, REF-P4; M1 fully green (Identity 4.3/4.2/4.10/4.8, KMS 11.3/11.4, STOR-D1/D2,
  CP-D2/D3); M0 forward-only migration framework (1.5). The index resolves these to their P-NNN.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1; ../../external-insights/02-platform-substrate.md §7 (backlinks are event-sourced
    projections); ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §3.2 (the edge table + the exact
    columns + the three indexes), §3.7 (the stateful-component register R1).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 1.5 (forward-only online
    migration), 11.3 (the per-tenant DEK the table is encrypted under).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M2 (the edge-schema bullet).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate:
  - The edge table migration (forward-only, 1.5): (tenant, region) first columns / partition prefix, RLS,
    per-tenant DEK; columns edge_id (deterministic hash(tenant, source, target, rel)), source, source_root,
    target, target_root, rel (edge_rel), rel_class (reference | lifecycle), origin_event, origin_actor
    (pseudonymous Principal ref), created_at, zookie, tombstoned; PRIMARY KEY (tenant, edge_id), UNIQUE (tenant,
    source, target, rel); the three indexes edge_inbound (tenant, target_root) WHERE NOT tombstoned,
    edge_outbound (tenant, source_root), edge_by_rel (tenant, target_root, rel) WHERE rel_class='lifecycle'.
    Exactly the §3.2 shape.
  - FLOOR named: this is the schema only — the builder/invalidator consumers that populate it land in REF-P6 /
    REF-P7. Write this so the empty schema is not mistaken for a working index.
- **CONTRACTS TO IMPLEMENT.** Consumed: 1.5 forward-only migration, 11.3 the per-tenant DEK. The edge table is
  the substrate for 5.4 (the consumer side lands REF-P6). To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The migration applies forward-only online (1.5 — no destructive rewrite); the forward-only-migration lint is
    green — CI.
  - The tenant-predicate + residency-pin lints are green: every index/query path is tenant-first, the table is
    (tenant, region)-partitioned, RLS on — CI (permanent ratchet).
  - The edge table is encrypted under the per-tenant DEK reserved in REF-P4 (encrypted-from-birth) — CI
    (structural).
- **TESTS (required).** A migration test (forward-only apply + the three indexes present with the WHERE
  predicates). A structural test that the table is RLS-on, (tenant, region)-partitioned, DEK-encrypted. No
  mutation floor (schema migration) — state so; the consumer-side mutation floors land in REF-P6.
- **DEFINITION OF DONE.** The edge table + its three indexes exist and apply forward-only online; encrypted
  under the per-tenant DEK; the lint + structural gates green; the builder/invalidator floor is named (REF-P6/
  REF-P7); the migration test passes; committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: Refs edge inverse-index schema. Body lists: the edge table + three indexes
  (§3.2 shape) migrated forward-only; encrypted-from-birth under the per-tenant DEK; the builder/invalidator
  floor named (REF-P6/REF-P7). Branch first; do not push unless asked. Co-Authored-By trailer.

---

### REF-P6 — The refs-edge-builder consumer (steady-state == cold-rebuild, idempotent)

- **BAND.** M2.
- **ROADMAP MILESTONE.** R-M2 slice 2 of 12 (the "refs-edge-builder consumer" deliverable; contract 5.4 consumer
  side).
- **DEPENDS-ON.** REF-P5 (the edge table exists to upsert into). M0 outbox + consumer template (2.2/2.3/2.4/2.5)
  + the failure-injection harness. The index resolves these to Bus M0.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1; ../../external-insights/02-platform-substrate.md §7 (reindex-from-source);
    ../../external-insights/04-hard-problems.md §5.3 (reindex-from-source the resilience primitive);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it; observability is part of the pass).
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §4.1 (deterministic edge_id
    idempotent rebuild), §4.3 (refs-edge-builder; steady-state == cold-rebuild), §3.7 (R1).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 5.4 (refs.edge.created/.removed
    via outbox; no standalone edge-write API), 2.1 (EventEnvelope), 2.4/2.5 (EventHandler template +
    consumer_dedup), 5.5 (the typed-lifecycle subjects the builder also whitelists).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M2 (the edge-builder bullet) + §1.1 row 5.4.
  - Drill source: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md REF-D7 (line ~352), REF-D4 (~349).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate:
  - The refs-edge-builder consumer (an ordinary EventHandler, 2.4): subjects() whitelists refs.edge.> plus the
    typed-lifecycle subjects issue.relation.> and knowledge.page.> (NEVER "*"; one of the reviewed
    firehose-class infra consumers, BUS-4); upsert on created (ON CONFLICT DO NOTHING/UPDATE — idempotent via the
    deterministic edge_id), delete/soft-delete on removed, tombstone on *.erased; ack-after-apply; idempotent on
    event_id via consumer_dedup (2.5). It writes source_root/target_root by strip_sub (REF-P1).
  - Steady-state ingestion and cold rebuild MUST be the same code path (so they cannot drift, REF-D4) — there is
    NO "load the edge table from an owner's DB" backdoor (no-cross-db lint).
  - Telemetry index_lag emitted by the builder (1.8) — no signal = failed drill.
  - FLOOR named: the builder ingests; the *.updated cache invalidation it would drive is REF-P7's invalidator
    (and a live cache is REF-P12). Named so ingestion is not mistaken for a live projection.
- **CONTRACTS TO IMPLEMENT.** 5.4 refs.edge.created/.removed (consumed via the builder; emitted by producers —
  NO standalone edge-write API). Consumed: 2.1 EventEnvelope, 2.4/2.5 the consumer template + dedup ledger. To
  the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - REF-D7 (F5, edge loss / no-ghost, the ingest half): crash a producer between the content/relation commit and
    the relay publish -> the edge event is still delivered (outbox), never an edge without its content. 0 ghost,
    0 lost. Green artifact: outbox emit-iff-committed telemetry — CI. (The producer-emit half of REF-D7 is
    REF-P8.)
  - Idempotent rebuild: replaying refs.edge.created twice upserts one row (deterministic edge_id); the dedup
    ledger drops the duplicate — CI.
  - The no-cross-db + no-raw-publish + tenant-predicate lints green on this crate — CI (permanent ratchet).
  - Telemetry index_lag emitted by the builder (1.8) — CI.
- **TESTS (required).** Unit tests for upsert/delete/tombstone idempotency on the deterministic edge_id;
  source_root/target_root derivation. A chained-mutation test: emit created -> removed -> created again across a
  simulated consumer restart, asserting exactly-once-in-effect. The drill-harness scenario for the REF-D7 ingest
  half (the producer-crash injection). The provider+consumer CDC pair for 5.4. Mutation-score floor for the
  edge-builder module (mandatory-core) stated and met.
- **DEFINITION OF DONE.** The refs-edge-builder exists and compiles; steady-state == cold-rebuild is one code
  path with no owner-DB backdoor; REF-D7 (ingest half) emits its dated green artifact (0 ghost, 0 lost); the
  idempotency + lint + index_lag telemetry gates green; the invalidator floor is named (REF-P7); unit + chained
  + CDC tests pass; the mutation floor is met; committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: Refs refs-edge-builder consumer. Body lists: 5.4 wired (consumer side);
  REF-D7 ingest-half greened (0 ghost / 0 lost, outbox emit-iff-committed); idempotent-rebuild proven; the
  invalidator floor named (REF-P7); the edge-builder mutation score measured. Branch first; do not push unless
  asked. Co-Authored-By trailer.

---

### REF-P7 — The refs-projection-invalidator consumer + the no-op cache shim

- **BAND.** M2.
- **ROADMAP MILESTONE.** R-M2 slice 3 of 12 (the "refs-projection-invalidator consumer" deliverable).
- **DEPENDS-ON.** REF-P6 (the builder consumer exists; the *.updated/*.erased subjects flow). M0 consumer
  template (2.4/2.5). The index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1; ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §4.3 (the
    refs-projection-invalidator busts R2 on *.updated/*.erased), §3.6 (the R2 cache it will drive once live).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 2.4/2.5 (EventHandler template +
    consumer_dedup), 5.6 (the projection the cache will hold).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M2 (the refs-projection-invalidator bullet).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate:
  - The refs-projection-invalidator consumer (an ordinary EventHandler, 2.4): whitelists *.updated/*.erased;
    on each, busts the projection cache entry per ArtifactRef; idempotent on event_id via consumer_dedup.
  - Because R2 (the real cache) lands in REF-P12, this prompt ships the consumer + a NO-OP cache shim it plugs
    into (the invalidation interface is real; the cache behind it is a shim that records invalidation calls).
  - FLOOR named: the projection cache invalidator targets a NO-OP shim until REF-P12 ships R2; named so the
    invalidation is not mistaken for live cache-busting. REF-P12 replaces the shim with the live cache.
- **CONTRACTS TO IMPLEMENT.** Consumed: 2.4/2.5 the consumer template + dedup ledger. The invalidation interface
  R2 (REF-P12) plugs into. To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The invalidator consumes *.updated/*.erased and calls the invalidation interface idempotently (the shim
    records the call) — CI.
  - The no-cross-db + tenant-predicate lints green on this crate — CI (permanent ratchet).
- **TESTS (required).** Unit test: a *.updated event drives one invalidation call per ArtifactRef; a duplicate
  event_id is dropped by the dedup ledger. The CDC pair for the consumer side of the invalidation contract.
  No mutation floor on a no-op shim — state that the real cache mutation floor lands in REF-P12.
- **DEFINITION OF DONE.** The refs-projection-invalidator exists and compiles; it drives the invalidation
  interface idempotently against the no-op shim; the lint gates green; the R2-shim floor is named (REF-P12);
  unit + CDC tests pass; committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: Refs refs-projection-invalidator + no-op cache shim. Body lists: the
  invalidator consumer wired (idempotent); the no-op shim floor named (REF-P12 ships the live R2 cache). Branch
  first; do not push unless asked. Co-Authored-By trailer.

---

### REF-P8 — The edge-extraction emit seam (one edge per structured node, emit-iff-committed)

- **BAND.** M2.
- **ROADMAP MILESTONE.** R-M2 slice 4 of 12 (the "edge extraction -> emit (the producer seam)" deliverable,
  emit half; contract 5.4 emit side).
- **DEPENDS-ON.** REF-P6 (the builder exists to ingest what this seam emits). The M2 myelin-content freeze
  (13.1, X-2 — the three inline ref nodes) the index resolves to its P-NNN. M0 outbox 2.2. (The loop-guard depth
  stamp is split into REF-P9.)
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (Chat references any artifact); ../../external-insights/04-hard-problems.md §2.4
    (structured-node extraction, not regex over prose — the reliability guarantee);
    ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §4.1 (the two producers; the
    same-transaction emit; OutboxTx::emit(draft, cause = Some(content_event))).
  - Reconciliation: 00-reconciliation-decisions.md X-2 (the three nodes byte-identical across Chat/Issues/KN).
  - Contracts: contract-index.md rows 5.4 (the edge events), 13.1 (the myelin-content three structured inline
    nodes mention/artifact_ref/embed — the producers), 2.2 (OutboxTx::emit(draft, cause)).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M2 (the edge-extraction emit seam bullet) +
    §1.2 row 13.1.
  - Drill source: REF-D7 (the producer-emit half of no-ghost; the ingest half is REF-P6).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs (or a shared extraction)
  crate:
  - The edge-extraction emit seam: given a myelin-content document containing the three structured inline nodes
    (mention(Principal), artifact_ref(ArtifactRef), embed(ArtifactRef) — 13.1, byte-identical X-2), emit one
    refs.edge.created per structured ref node (rel in {mentions, links, embeds}, rel_class='reference') in the
    SAME transaction that writes content, via OutboxTx::emit(draft, cause = Some(content_event)) so the
    correlation root carries and causation = the content event. Extraction is structured-node-driven, NOT a
    regex over prose.
  - At M2 the producers are exercised with a synthetic/test content writer (the first real ones land in REF-P17/
    REF-P18). FLOOR named: producers are synthetic until R-M3/R-M4 — write this so the seam is not mistaken for
    live producer edges. The loop-guard causal-depth stamp on these emits lands in REF-P9.
- **CONTRACTS TO IMPLEMENT.** 5.4 (the emit half of the producer seam — emitted via outbox, never a standalone
  write API). Consumed: 13.1 (the three content nodes), 2.2 (OutboxTx::emit(draft, cause)). To the frozen shape.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - One refs.edge.created per structured node, in the same transaction as the content write — proven by the test
    content writer: N nodes -> N edges, emit-iff-committed (the REF-D7 producer-emit half) — CI.
  - The no-raw-publish lint green (no edge escapes the outbox; there is no standalone edge-write API) — CI.
- **TESTS (required).** Unit tests: extraction yields exactly one edge per node kind, correct rel/rel_class; a
  document with no ref nodes yields zero edges. A chained test: write content -> assert edge emitted iff the
  content tx commits (abort -> no edge). The CDC pair for the emit side of 5.4. Mutation floor on the extraction
  module stated and met.
- **DEFINITION OF DONE.** The emit seam exists and compiles; one edge per structured node emit-iff-committed;
  the no-raw-publish lint is green; the synthetic-producer floor is named (real producers REF-P17/REF-P18; loop
  guard REF-P9); unit + chained + CDC tests pass; the mutation floor is met; committed.
- **COMMIT.** Header: P-<NNN> M2: Refs edge-extraction emit seam. Body lists: 5.4 emit side wired;
  emit-iff-committed proven (N nodes -> N edges); the synthetic-producer floor named (REF-P17/REF-P18; loop
  guard REF-P9). Branch first; do not push unless asked. Co-Authored-By trailer.

---

### REF-P9 — The loop-guard causal-depth stamp on every refs.edge.* emit

- **BAND.** M2.
- **ROADMAP MILESTONE.** R-M2 slice 5 of 12 (the "loop-guard causal-depth stamp" deliverable).
- **DEPENDS-ON.** REF-P8 (the emit seam exists to stamp). M0 outbox causality fields (2.1/2.2). The index
  resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2; ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §4.1 (causality depth +1 — the
    loop guard reads it, AG-6).
  - Contracts: contract-index.md rows 2.1 (EventEnvelope causality fields), 2.2 (OutboxTx::emit(draft, cause)),
    1.8 (the causal-depth telemetry signal).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M2 (the loop-guard depth stamp bullet).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs (or the shared extraction)
  crate:
  - The loop-guard causal-depth stamp: depth = content_event.depth + 1 on every refs.edge.* so the AG-6 loop
    guard treats only a structured artifact_ref node as a re-trigger source. A depth-ceiling tripwire fires
    before runaway. Build + drill the stamp now over the REF-P8 emit seam.
- **CONTRACTS TO IMPLEMENT.** Consumed: 2.1 EventEnvelope causality fields, 2.2 OutboxTx::emit(draft, cause).
  To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The causal-depth stamp: every emitted refs.edge.* carries depth = content_event.depth + 1; the loop guard
    treats an artifact_ref node as a re-trigger source and the depth-ceiling tripwire fires before runaway — CI
    (asserts the causal-depth telemetry signal 1.8).
- **TESTS (required).** The loop-guard depth-stamp test (depth increments; ceiling tripwire fires). Mutation
  floor on the depth-stamp logic stated and met (it is leak-of-runaway-critical).
- **DEFINITION OF DONE.** Every refs.edge.* carries the +1 depth stamp; the loop guard treats artifact_ref as a
  re-trigger source; the ceiling tripwire fires before runaway; the causal-depth telemetry fires; the test
  passes; the mutation floor is met; committed.
- **COMMIT.** Header: P-<NNN> M2: Refs loop-guard causal-depth stamp. Body lists: the +1 depth stamp on every
  refs.edge.*; the loop guard + ceiling tripwire greened (causal-depth telemetry). Branch first; do not push
  unless asked. Co-Authored-By trailer.

---

### REF-P10 — The per-viewer resolution chokepoint (denied -> tombstone, never leak)

- **BAND.** M2.
- **ROADMAP MILESTONE.** R-M2 slice 6 of 12 (the "per-viewer resolution service — the chokepoint" deliverable;
  contract 5.2).
- **DEPENDS-ON.** REF-P1 (ArtifactRef + parse), REF-P5 (the edge table — crate context), REF-P7 (the no-op cache
  shim resolve reads through). M1 Identity check (4.2) + zookie (4.10), the resilient client (1.9) + fail-static
  (1.10), each subsystem's project(ref, viewer) shape (5.6). The index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1; ../../external-insights/02-platform-substrate.md §7;
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it; observability).
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §4.2 (resolution: parse ->
    Id.check -> denied returns a tombstone never a leak -> projection via R2 cache hit, else owner's project
    through the resilient client, Refs never reads the owner DB -> subscribe to *.updated/*.erased; per-viewer
    correctness without per-viewer caching), §3.6 (R2 cache), and the cross-cell pinning C-5 (frozen semantics;
    the fan-out build stays a floor).
  - Contracts: contract-index.md rows 5.2 (resolve(ref, viewer, mode) -> Projection | Tombstone), 4.2
    (check(subject, perm, object, zookie?, caveat?)), 5.6 (project(ref, viewer) -> {title, state, icon,
    render_hint, sub_anchor?}), 1.9 ResilientClient, 1.10 FailStatic.
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M2 (the resolution chokepoint bullet) + §1.1
    row 5.2.
  - Drill source: REF-D1 (the leak invariant — the resolve half; line ~346).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate:
  - The resolve(ref, viewer, mode) -> Projection | Tombstone service: (1) parse + validate (REF-P1); (2)
    Id.check(viewer, view, ref) -> DENIED returns a Tombstone, never a leak (the chokepoint that makes every
    unfurl non-leaking — a confidential issue degrades to a placeholder, title never present); (3) projection
    via the R2 cache hit (the REF-P7 shim now; REF-P12 makes it live), else the owner's project(ref, viewer)
    through the ResilientClient (1.9) — Refs NEVER reads the owner's DB (no-cross-db lint); (4) the caller
    subscribes to *.updated/*.erased so the rendered ref stays live.
  - Per-viewer correctness WITHOUT per-viewer caching: the per-viewer check (step 2) gates a viewer-independent,
    ref-keyed cache (step 3) — shared without leaking because no content returns until the check passes. Document
    this explicitly.
  - Fail-static (1.10) under an Id hiccup: resolve degrades on the coarse cache rather than cascading; a
    zookie-stamped read bypasses fail-static (the new-enemy defense is exercised fully in REF-P11/REF-P12).
  - Cross-cell resolution is pinned cell-local (C-5): a cross-cell target resolves in the home cell; only the
    already-filtered projection or a tombstone crosses, over the frozen CrossCellPointer. FLOOR named: the
    cross-cell fan-out BUILD is R-M5 (REF-P26); the resolution SEMANTICS are frozen here.
- **CONTRACTS TO IMPLEMENT.** 5.2 resolve (owned). Consumed: 4.2 check, 5.6 project, 1.9 ResilientClient, 1.10
  FailStatic. To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - REF-D1 (F1, the resolve half): a confidential artifact resolves to a Tombstone{denied} for an unauthorized
    viewer — the title/state/icon are NEVER in the tombstone; 0 leak. Green artifact: the zero-escape counter at
    0 — CI. (The backlink/traverse half of REF-D1 is drilled in REF-P11/REF-P13.)
  - Fail-static: with Id forced unavailable, resolve degrades on the coarse cache (no cascade); the fail-static
    ratio telemetry (1.8) fires — CI.
  - The no-cross-db lint green (resolve reads project, never the owner DB) — CI.
  - resolve_cache_hit_ratio telemetry emitted (1.8) — CI.
- **TESTS (required).** Unit tests: denied -> tombstone carries no content; allowed -> projection from project.
  A chained test: resolve same ref as two viewers (one permitted, one denied) -> the shared ref-keyed cache
  serves the permitted viewer and denies the other with no content. The drill scenario for the REF-D1 resolve
  half + the fail-static injection. The provider+consumer CDC pair for 5.2. Mutation floor on the resolve module
  (mandatory-core) stated and met.
- **DEFINITION OF DONE.** resolve exists and compiles; denied -> tombstone-never-leak proven (REF-D1 resolve
  half, 0 leak); fail-static degradation proven; the cross-cell-fan-out floor named (REF-P26); the no-cross-db
  lint + telemetry green; unit + chained + CDC tests pass; the mutation floor is met; committed. No threshold
  weakened.
- **COMMIT.** Header: P-<NNN> M2: Refs per-viewer resolution chokepoint (denied -> tombstone). Body lists: 5.2
  implemented; REF-D1 resolve-half greened (0 leak); fail-static degradation proven; the cross-cell fan-out
  floor named (REF-P26); resolve mutation score measured. Branch first; do not push unless asked. Co-Authored-By
  trailer.

---

### REF-P11 — The permission-filtered backlink read: lower the SetExpr ACL filter over source_root (the crux)

- **BAND.** M2.
- **ROADMAP MILESTONE.** R-M2 slice 7 of 12 (the "permission-filtered backlink read — the crux" deliverable;
  contract 5.3 backlinks/edges).
- **DEPENDS-ON.** REF-P5 (the edge index), REF-P10 (resolution context). M1 Identity 4.3 (list_objects with the
  frozen SetExpr push-down — THE crux dependency) + 4.10 (zookie + the authz reverse-index revision watermark).
  The index resolves these to Identity's M1 P-NNN.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1; ../../external-insights/02-platform-substrate.md §7 (permission-filtered set reads;
    Leopard/Zanzibar); ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §4.4 (the leak-free backlink read;
    the FROZEN SetExpr lowering over source_root — the forms Ids/NotIds -> IN/NOT IN, InRelation/TupleSet ->
    JOIN authz_visible, Union/Intersect/Difference -> AND/OR/EXCEPT, All -> no predicate, None -> WHERE false;
    zookie carried bypasses fail-static at-or-after the revision watermark; no N+1, no post-filter; always
    paginated), §3.2 (source_root is the filter column).
  - Reconciliation: 00-reconciliation-decisions.md C-4 (the SetExpr encoding frozen), OQ-E (the set algebra).
  - Contracts: contract-index.md rows 5.3 (backlinks/edges), 4.3 (list_objects -> Ids | Filter{set_expr,
    zookie}), 4.10 (Consistency/zookie + the authz reverse-index revision watermark).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M2 (the backlink-read crux bullet) + §1.2
    rows 4.3, 4.10 + §4 the critical-upstream note.
  - Drill source: REF-D1 (backlink-leak half, ~346), REF-D2 (cross-tenant edge, ~347), REF-D6 (new-enemy zookie,
    ~351).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate:
  - backlinks(target, viewer, page) and edges(ref, viewer) (5.3): target_root := strip_sub(target); result :=
    Id.list_objects(viewer, perm=view, type, zookie?) -> Ids{ids, zookie} | Filter{set_expr, zookie} (4.3).
    Lower BOTH frozen shapes and the SetExpr over the edge.source_root column BEFORE the row scan:
    Ids/NotIds -> source_root IN/NOT IN (...) (inlined under the cardinality cap); InRelation{relation,
    via_column}/TupleSet{index} -> JOIN authz_visible av ON av.object_id = edge.source_root AND av.subject =
    :viewer AND av.relation = view (the per-tenant residency-pinned authz reverse index); Union/Intersect/
    Difference -> AND/OR/EXCEPT; All -> no predicate; None -> WHERE false. ONE query, NO N+1, NO post-filter
    (Refs never loops check per inbound edge). Always paginated (hot-artifact safety). The query carries
    WHERE tenant = :viewer.tenant (no cross-tenant path) and ORDER BY created_at DESC LIMIT :page.
  - The zookie is carried so a just-revoked grant can't read stale: the JOIN reads the authz reverse index
    at-or-after the zookie's revision watermark, bypassing Id's fail-static cache (the new-enemy defense, 4.10).
  - FLOOR named: the read-time CTE + list_objects filter + pagination + (M5) read replica is the hot-artifact
    floor; the Leopard-style flattened reach index R4 is the follow-on, promoted at measured hot-fanout > read
    budget in R-M5 (REF-P23). Write this so "we page them, we don't materialise them" is not mistaken for the
    final hot-path answer.
- **CONTRACTS TO IMPLEMENT.** 5.3 backlinks/edges (owned). Consumed: 4.3 list_objects (the frozen SetExpr —
  lowered over source_root), 4.10 zookie/Consistency. To the frozen shapes; Refs is one of the five named
  SetExpr consumers — no Id signature change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - REF-D1 (F1, the cardinal sin, backlink half): a confidential artifact referencing a public one is ABSENT
    from backlinks/edges for an unauthorized viewer — incl. filter-mode and under zookie staleness. 0
    unauthorized backlinks. Green artifact: the zero-escape counter at 0 — CI.
  - REF-D2 (F2): a cross-tenant edge read via path spoof / crafted cross-tenant URN -> 0 cross-tenant edge
    readable; the tenant-predicate lint catches a tenant-less query at compile — CI.
  - REF-D6 (F8, new-enemy): revoke access, immediately re-read backlinks with the post-revoke zookie -> no stale
    allow (the zookie bypasses fail-static + honours the reverse-index revision watermark) — CI.
  - No N+1: the backlink read issues ONE query (assert via query-count telemetry); the filter-mode split
    (Ids vs Filter/TupleSet) telemetry (1.8) fires — CI.
- **TESTS (required).** Unit tests for every SetExpr lowering form (Ids, NotIds, InRelation, TupleSet, Union,
  Intersect, Difference, All, None) -> the correct SQL predicate/JOIN. A chained test: grant -> read backlinks
  (visible) -> revoke with new zookie -> re-read (absent), proving the new-enemy bypass. The drill scenarios for
  REF-D1 (backlink half), REF-D2, REF-D6. The provider+consumer CDC pair for 5.3 (backlinks) and the consumer
  CDC for 4.3. Mutation floor on the SetExpr-lowering module (mandatory-core — this is the leak-critical code)
  stated and met.
- **DEFINITION OF DONE.** backlinks/edges exist and compile; the frozen SetExpr lowering over source_root is
  one query with no N+1 and no post-filter; REF-D1 (backlink half), REF-D2, REF-D6 each emit a dated green
  artifact (0 leak, 0 cross-tenant, no stale allow); the hot-artifact R4 floor is named (REF-P23); the
  query-count + filter-mode-split telemetry fire; unit + chained + CDC tests pass; the mutation floor is met;
  committed. No threshold weakened, no assertion inverted.
- **COMMIT.** Header: P-<NNN> M2: Refs permission-filtered backlink read (SetExpr over source_root). Body
  lists: 5.3 backlinks/edges implemented; the frozen SetExpr lowering (all forms); REF-D1 (backlink)/REF-D2/
  REF-D6 greened (0 leak / 0 cross-tenant / no stale allow); one-query-no-N+1 proven; the R4 hot-fanout floor
  named (REF-P23); SetExpr-lowering mutation score measured. Branch first; do not push unless asked.
  Co-Authored-By trailer.

---

### REF-P12 — The R2 projection cache (bounded, invalidatable holder; replaces the REF-P7 shim)

- **BAND.** M2.
- **ROADMAP MILESTONE.** R-M2 slice 8 of 12 (the "projection cache (R2)" deliverable; contract 5.6 holder side
  + the R2 holder).
- **DEPENDS-ON.** REF-P7 (the invalidator consumer + the no-op shim it replaces), REF-P10 (resolve reads the
  cache), REF-P4 (the per-tenant DEK the cache is encrypted under). Each subsystem's project shape (5.6). The
  index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1; ../../external-insights/04-hard-problems.md §1 (erasure vs immutability — a name in a
    title); ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §3.6 (the R2 projection cache as
    a bounded invalidatable holder, never truth), §4.2 (resolve reads R2 first).
  - Contracts: contract-index.md rows 5.6 (project's title/state/icon/render_hint the cache holds), 10.1 (R2 as
    a PersonalDataHolder, the cache half), 11.3/11.4 (the per-tenant DEK), 1.8 (resolve_cache_hit_ratio).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M2 (the R2 cache bullet).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate:
  - The R2 projection cache (3.6): bounded, invalidatable, event-busted per ArtifactRef (title/state/icon/render
    hint), keyed (tenant, ref), TTL + *.updated/*.erased invalidation (the refs-projection-invalidator from
    REF-P7 now drives a REAL cache, replacing the REF-P7 no-op shim). A PersonalDataHolder (may hold a name in a
    title), NEVER a source of truth; on miss/erasure it re-resolves. Residency-pinned, crypto-shred-able
    (Valkey-class), under the per-tenant DEK reserved in REF-P4.
  - Wire the resolve_cache_hit_ratio telemetry (1.8).
  - FLOOR named: the cache holds PII (a name in a title) but the structural ERASE of that PII is the holder erase
    surface in REF-P15; named so the cache is not mistaken for a complete erasure answer.
- **CONTRACTS TO IMPLEMENT.** 10.1 R2 as a holder (the cache half — owned). Consumed: 5.6 project (the projection
  the cache holds), 11.3/11.4 the per-tenant DEK. To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - R2 invalidation: a *.updated busts the cached entry; a *.erased tombstones it; a miss re-resolves — CI (the
    REF-P7 invalidator now drives the live cache, not the shim).
  - resolve_cache_hit_ratio telemetry emitted (1.8) — CI.
  - The cache is encrypted under the per-tenant DEK + residency-pinned (crypto-shred-able) — CI (structural).
- **TESTS (required).** A chained test: cache hit -> *.updated -> miss -> re-resolve. A test that the cache is
  never a source of truth (on erasure it re-resolves, never serving stale). The CDC pair for the R2 holder side
  of 10.1. Mutation floor on the cache invalidation/keying module stated and met.
- **DEFINITION OF DONE.** The R2 cache exists and compiles; it replaces the REF-P7 no-op shim; R2 invalidation
  is proven (busts/tombstones/re-resolves); the cache is DEK-encrypted + residency-pinned; the hit-ratio
  telemetry fires; the erase-of-cache-PII floor is named (REF-P15); the chained + CDC tests pass; the mutation
  floor is met; committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: Refs R2 projection cache. Body lists: R2 holder live (replaces the REF-P7
  shim); invalidation proven (bust/tombstone/re-resolve); DEK-encrypted + residency-pinned; cache-PII-erase
  floor named (REF-P15); cache mutation score measured. Branch first; do not push unless asked. Co-Authored-By
  trailer.

---

### REF-P13 — The bounded cycle-safe recursive-CTE traverse (depth-16, branch-prune)

- **BAND.** M2.
- **ROADMAP MILESTONE.** R-M2 slice 9 of 12 (the "recursive-CTE traversal" deliverable; contract 5.3 traverse).
- **DEPENDS-ON.** REF-P5 (the edge adjacency list), REF-P11 (the list_objects filter, reused as the
  collected-node post-filter). Identity 4.3 (the post-filter). The index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1; ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §4.5 (the bounded cycle-safe
    recursive-CTE traverse: WITH RECURSIVE over edge filtered by rel/rel_class, visited-set cycle guard
    (path-array / SQL:2023 CYCLE), depth ceiling default 16, statement timeout, ONE list_objects post-filter
    over the collected node set not per-hop, prune the branch on an unreadable hop, partial result + truncated
    marker, cycle -> diagnostic not hang), §3.4 (the adjacency structure).
  - Contracts: contract-index.md row 5.3 (traverse).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M2 (the traverse bullet) + §1.1 row 5.3.
  - Drill source: REF-D8 (cycle / unbounded walk, ~353), REF-D1 (the traverse leak half, ~346).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate:
  - traverse(root, rels, depth, viewer) (5.3): a WITH RECURSIVE walk over the edge adjacency list filtered by
    rel/rel_class, with a visited-set cycle guard (path-array or SQL:2023 CYCLE), a depth ceiling (default 16,
    read from the thresholds file), a statement timeout, and ONE list_objects post-filter over the COLLECTED
    node set (not per-hop) where a hop into an unreadable artifact PRUNES that branch (the traversal is not a
    side-channel). A request exceeding the budget returns a PARTIAL result + a "truncated" marker, never an
    unbounded scan; a dependency cycle is surfaced as a DIAGNOSTIC, not a hang.
  - FLOOR named: the traverse filters by rel/rel_class but the lifecycle-class edges it walks are minted by the
    TE-7 mirror discipline (REF-P14); named so the traverse is not mistaken for cross-subsystem-lifecycle-aware
    before the mirror discipline lands.
- **CONTRACTS TO IMPLEMENT.** 5.3 traverse (owned). Consumed: 4.3 list_objects (the post-filter). To the frozen
  shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - REF-D8 (traversal bound): a cycle + a 1000-deep chain -> the CTE terminates (visited-set + depth ceiling
    16), the cycle is surfaced as a diagnostic (not a hang), the statement timeout is respected. Green artifact:
    depth-bound honoured — CI.
  - REF-D1 (traverse leak half): a hop into an unreadable artifact prunes the branch; traverse never reveals an
    edge into an artifact the viewer cannot read. 0 leak — CI.
- **TESTS (required).** Unit tests: the cycle guard terminates a self-referential graph; the depth ceiling
  truncates at 16 with the marker; the post-filter prunes (not per-hop). The drill scenarios for REF-D8 and the
  REF-D1 traverse half. The CDC pair for 5.3 (traverse). Mutation floor on the traverse module stated and met.
- **DEFINITION OF DONE.** traverse exists and compiles; REF-D8 (bounded, cycle -> diagnostic) and the REF-D1
  traverse half (0 leak, branch-prune) emit dated green artifacts; the TE-7-mirror floor is named (REF-P14);
  unit + CDC tests pass; the mutation floor is met; committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: Refs recursive-CTE traverse. Body lists: 5.3 traverse implemented; REF-D8
  greened (depth 16, cycle -> diagnostic); REF-D1 traverse half greened (0 leak, branch-prune); the TE-7-mirror
  floor named (REF-P14); traverse mutation score measured. Branch first; do not push unless asked. Co-Authored-By
  trailer.

---

### REF-P14 — The TE-7 typed-edge mirror discipline (vocabulary + inverse pairing, synthetic events)

- **BAND.** M2.
- **ROADMAP MILESTONE.** R-M2 slice 10 of 12 (the "TE-7 typed-edge mirror discipline" deliverable; contract
  5.5).
- **DEPENDS-ON.** REF-P6 (the builder already whitelists the typed lifecycle subjects), REF-P13 (the traverse
  that walks the lifecycle-class edges this projects). The index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1; ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §3.3 (the TE-7 hybrid: lifecycle
    edges dual-homed, the typed table is truth, Refs fixes the rel vocabulary + the inverse pairing + the
    rel_class='lifecycle' mirror discipline).
  - Contracts: contract-index.md row 5.5 (TE-7 typed-edge mirror: the lifecycle relation set
    closes/blocks/blocked_by/depends_on/parent/assigns/relates, the inverse pairing, the typed table wins on
    drift).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M2 (the TE-7 mirror bullet) + §1.1 row 5.5.
  - Drill source: REF-D4 (the TE-7 drift-reconvergence half, ~349; the inverse-pairing correctness).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate:
  - The TE-7 typed-edge mirror discipline (5.5): fix the rel vocabulary (closes/blocks/blocked_by/depends_on/
    parent/assigns/relates), the rel_class='lifecycle' mirror discipline, and the inverse pairing
    (blocks<->blocked_by, parent<->child). Consume the typed lifecycle events (already whitelisted by the builder
    in REF-P6) and project lifecycle-class edges so cross-subsystem traversal is one Refs query.
  - At M2 the discipline + the consumer projection are built; the typed TABLES are owned by Issues/KN and arrive
    in R-M3/R-M4 — so the lifecycle producers are exercised here with SYNTHETIC typed events. FLOOR named: real
    typed mirrors land in R-M3 (KN page_parent, REF-P18) and R-M4 (Issues issue_relation, REF-P20). Write this
    so the discipline is not mistaken for a working mirror over real tables.
- **CONTRACTS TO IMPLEMENT.** 5.5 TE-7 mirror discipline (owned — the vocabulary + inverse pairing; the tables
  are the subsystems'). To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The inverse-pairing correctness: a synthetic blocks event yields both blocks and blocked_by lifecycle edges
    with the correct direction — CI.
  - The TE-7 drift-reconvergence half of REF-D4: a synthetic drift between Refs' projection and the typed table
    reconverges to the typed table (typed wins) on a scoped reindex — CI (the full reindex parity is REF-P16).
- **TESTS (required).** Unit tests: the inverse pairing across the lifecycle relation set; rel_class='lifecycle'
  mirror discipline. A chained test: synthetic lifecycle events -> traverse an epic tree -> correct inverse
  pairing across hops. The CDC pair for 5.5. Mutation floor on the mirror module stated and met.
- **DEFINITION OF DONE.** The TE-7 mirror discipline exists and compiles; the inverse pairing is correct; the
  TE-7 drift-reconvergence (typed wins) is proven on synthetic events; the synthetic-typed-events floor is named
  (real mirrors REF-P18/REF-P20); unit + chained + CDC tests pass; the mutation floor is met; committed. No
  threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: Refs TE-7 mirror discipline. Body lists: 5.5 mirror discipline implemented
  (vocabulary + inverse pairing); inverse pairing proven; the TE-7 drift-reconvergence greened (typed wins);
  the synthetic-typed-events floor named (REF-P18/REF-P20); mirror mutation score measured. Branch first; do not
  push unless asked. Co-Authored-By trailer.

---

### REF-P15 — The unified 4-step #sub tombstone ladder (root always carried) + the structural erasure holder

- **BAND.** M2.
- **ROADMAP MILESTONE.** R-M2 slice 11 of 12 (the "unified #sub resolution ladder" deliverable + the "erasure as
  a real (small) holder" deliverable; contracts 5.7 + 5.6 sub_anchor + 10.1 real erase).
- **DEPENDS-ON.** REF-P1 (the frozen #sub grammar), REF-P10 (resolve — the ladder is its #sub extension), REF-P7
  (the *.erased consumer), REF-P12 (the R2 cache to purge), REF-P3 (the holder stub), REF-P4 (the per-tenant
  DEK). Each subsystem's project sub_anchor shape (5.6). Identity pseudonym shred (4.8). The index resolves
  these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1; ../../external-insights/04-hard-problems.md §1 (erasure vs immutability — the tombstone
    carries the root; pseudonym + tombstone); ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §4.6 (the one resolution ladder,
    frozen: 1 permission -> Tombstone{denied}; 2 root resolve -> Tombstone{root_gone}; 3 sub resolve via the
    owner's project sub_anchor -> LIVE->Projection / MOVED->Projection+moved / OUTDATED->Projection(partial)+
    outdated / GONE->Tombstone{sub_gone, root}; 4 ERASED -> Tombstone{erased}; a tombstone ALWAYS carries the
    root), §3.5 (Git line-ranges content-anchored: exact->LIVE, rebased->MOVED, partial->OUTDATED,
    content_gone->GONE via BLAKE3 + 3-way context match), §4.6 tail (Refs as a PersonalDataHolder: locate ->
    edges/cache naming the subject; erase -> purge R2 PII + rely on Id's pseudonym shred for origin_actor +
    tombstone content-erased targets via *.erased; restrict suppression; no erasure backdoor).
  - Reconciliation: 00-reconciliation-decisions.md C-2 (the 4-step ladder frozen), C-1/C-6 (the grammar + the
    check-/step- kinds), X-7 (the one residual posture, by reference).
  - Contracts: contract-index.md rows 5.7 (the unified #sub scheme + the 4-step ladder), 5.6 (project's
    sub_anchor resolver returns the frozen live/moved/outdated/gone state), 5.2 (resolve, extended for #sub),
    10.1 (the holder erase surface), 4.8 (resolve_pseudonym/erase).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M2 (the #sub ladder + the erasure-holder
    bullets) + §1.1 rows 5.7, (10.1).
  - Drill source: REF-D9 (sub-tombstone, the unified ladder, ~354), REF-D5 (erasure CI variant, ~350).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate:
  - The unified 4-step #sub resolution ladder (5.7), extending resolve (REF-P10) for any #sub: (1) permission ->
    Deny ⇒ Tombstone{denied} (never leak); (2) root resolve -> No ⇒ Tombstone{root_gone}; (3) sub resolve via
    the owner's project sub_anchor resolver -> LIVE -> Projection / MOVED -> Projection + flag moved / OUTDATED
    -> Projection(partial) + flag outdated / GONE -> Tombstone{sub_gone, root}; (4) ERASED -> Tombstone{erased}.
    A tombstone ALWAYS carries the root (an embed degrades to "this referenced <parent> (the specific part is no
    longer available)" rather than vanishing). The same live/moved/outdated/gone shape covers Git line-ranges,
    KN block/heading/row anchors, Chat message/thread anchors, and the check-/step- CI kinds (C-6) — one ladder.
  - The real erasure holder (10.1), replacing the REF-P3 STUB: locate(subject) -> the edges/cache entries naming
    the subject; erase(subject) -> purge R2 cache PII (the REF-P12 cache) + rely on Id's pseudonym shred (4.8)
    for origin_actor (the edge keeps the opaque id; the human becomes unresolvable) + tombstone content-erased
    targets via the *.erased consumer (REF-P7). NO erasure backdoor — driven by *.erased through the same live
    consumer path. restrict(subject) suppression keeps a restricted subject's references out of indexing/
    agent-use/analytics. Refs holds only pseudonymous opaque ids + cache titles, never third-party free-text
    bodies (the one platform residual posture instantiated by reference, X-7).
  - Wire the telemetry (1.8): tombstone_count (+ the ladder-state distribution).
  - FLOOR named: each subsystem's STABLE #sub mint (a block id survives moves, a message/comment id is
    immutable, a Git range carries the BLAKE3 fingerprint) is the subsystem's deliverable, asserted by REF-D9 on
    real producers in R-M3/R-M4 (REF-P17/REF-P18/REF-P19/REF-P20/REF-P21). At M2 the ladder is exercised against
    synthetic + the available producers. The full-scale erasure (REF-D5 at backup scale) is REF-P25. Write this
    so the frozen grammar is not mistaken for a working sub-anchor everywhere.
- **CONTRACTS TO IMPLEMENT.** 5.7 the unified #sub scheme + the 4-step ladder (owned — grammar + ladder; the
  stable mint is each subsystem's). 10.1 the real erase surface (owned — replaces the REF-P3 stub). Consumed:
  5.6 project's sub_anchor, 4.8 pseudonym shred. To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - REF-D9 (sub-tombstone, the unified ladder): delete a doc block / PR comment / chat message / make a Git
    line-range outdated that others embed -> each degrades through the frozen live/moved/outdated/gone ladder to
    the correct state (moved / outdated / sub_gone) with the ROOT carried — 0 dangling embed, 0 hard 404, no
    leak. At M2 exercised against the available producers + synthetic ones; re-run on each real producer in
    R-M3/R-M4. Green artifact: the tombstone-ladder state distribution telemetry — CI.
  - REF-D5 (erasure, CI variant): erase a subject + a referenced artifact -> references tombstone, the person is
    unresolvable, 0 recoverable PII in edge/cache, no 500 on resolve. Green artifact: erase-receipt + 0
    resolve-error — CI variant (full backup-level proof joins E2E-4 in R-M5, REF-P25).
  - tombstone_count telemetry emitted (1.8) — CI.
- **TESTS (required).** Unit tests for every ladder branch (denied/root_gone/each sub-state/erased) and that the
  root is always carried. Unit tests for the Git content-anchored states (exact/rebased/partial/content_gone)
  against synthetic blob fingerprints. Unit tests: locate/erase/restrict on a subject; the *.erased path
  tombstones; no backdoor. A chained test: cache hit -> *.updated -> miss -> re-resolve; then erase ->
  re-resolve -> tombstone, person unresolvable. The drill scenarios for REF-D9 (across the three content shapes,
  synthetic) and the REF-D5 CI variant. The CDC pair for 5.7 + the holder erase side of 10.1. Mutation floor on
  the ladder + erase module stated and met.
- **DEFINITION OF DONE.** The 4-step ladder + the real erase holder exist and compile; REF-D9 emits a dated
  green artifact (correct state, root carried, 0 dangling/404/leak) across synthetic + available producers;
  REF-D5 (CI) emits its green artifact (0 recoverable PII, no resolve-error); the REF-P3 holder stub is replaced
  by the real surface; the per-subsystem-stable-mint floor is named (REF-P17/REF-P18/REF-P19/REF-P20/REF-P21);
  the full-scale-erasure floor is named (REF-P25); unit + chained + CDC tests pass; the mutation floor is met;
  committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: Refs unified #sub tombstone ladder + structural erasure holder. Body lists:
  5.7 the ladder implemented; 10.1 real erase (replaces the REF-P3 stub); REF-D9 greened (root carried, 0
  dangling/404/leak); REF-D5 (CI) greened (0 recoverable PII); the per-subsystem stable-mint floor named; the
  full-scale-erasure floor named (REF-P25); ladder/erase mutation score measured. Branch first; do not push
  unless asked. Co-Authored-By trailer.

---

### REF-P16 — Reindex-from-source: rebuild byte-parity (the recovery path, one code path, no backdoor)

- **BAND.** M2.
- **ROADMAP MILESTONE.** R-M2 slice 12 of 12 (the "reindex-from-source" deliverable; contract 5.8). This slice
  completes R-M2.
- **DEPENDS-ON.** REF-P6 (the builder — the one ingest path reindex re-drives), REF-P14 (the TE-7 mirror
  discipline reindex reconverges), REF-P15 (the erase holder / ladder). The Bus reindex re-emit (2.6) + each
  owner's replay. The index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe); ../../external-insights/04-hard-problems.md §5.3 (reindex-from-source the
    only recovery path); ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §4.7 (reindex-from-source:
    events::reindex (scope) -> each owner's replay(scope, since) emits *.snapshot sub-artifact-granular ->
    refs-edge-builder ingests idempotently -> the rebuilt edge index byte-matches live; one code path; on a TE-7
    drift a scoped reindex reconverges to the typed table which wins; never reads an owner DB).
  - Contracts: contract-index.md rows 5.8 (reindex(scope), never reads owner DBs), 2.6 (the reindex re-emit +
    *.snapshot/*.erased sub-artifact-granular).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M2 (the reindex-from-source bullet) + §1.1 row
    5.8 + §1.2 row 2.6.
  - Drill source: REF-D4 (reindex parity CI variant, ~349).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate:
  - reindex(scope) (5.8): call the Bus re-emit protocol (2.6) -> each owner's replay(scope, since) emits
    *.snapshot (content nodes + typed relations, sub-artifact-granular) -> refs-edge-builder ingests
    idempotently -> the rebuilt edge index byte-matches the live index. ONE code path for steady-state and
    recovery; NO "load the edge table from an owner's DB" backdoor (no-cross-db). On a Refs<->typed-table TE-7
    drift, a scoped reindex reconverges Refs to the typed table (which always wins).
  - Wire the reindex_parity telemetry (1.8).
  - FLOOR named: the CI-variant drill (small corpus) gates this band; the full-scale REF-D4 (reindex parity at
    scale) is R-M5 (REF-P24). Write this so the CI variant is not mistaken for the at-scale proof.
- **CONTRACTS TO IMPLEMENT.** 5.8 reindex (owned). Consumed: 2.6 the re-emit + replay. To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - REF-D4 (reindex-from-cold parity, CI variant): wipe the edge index, reindex(scope) -> the rebuilt index
    byte-matches live; a synthetic TE-7 drift reconverges to the typed table (typed wins). Green artifact: the
    reindex-parity hash — CI variant (full-scale REF-D4 is SCHED at R-M5, REF-P24).
  - The no-cross-db lint green (reindex re-drives the builder, never reads an owner DB) — CI.
  - reindex_parity telemetry emitted (1.8) — CI.
- **TESTS (required).** A chained test: build edges -> wipe -> reindex -> assert byte-parity; a synthetic TE-7
  drift -> scoped reindex -> typed wins. The drill scenario for the REF-D4 CI variant. The CDC pair for 5.8.
  Mutation floor on the reindex module stated and met.
- **DEFINITION OF DONE.** reindex(scope) exists and compiles; REF-D4 (CI) emits a dated green artifact
  (byte-parity / typed-wins); the one-code-path no-backdoor property holds (no-cross-db green); the
  reindex_parity telemetry fires; the full-scale-drill floor is named (REF-P24); unit + CDC tests pass; the
  mutation floor is met; committed. This slice completes R-M2; the master M2 exit gate cites REF-D1/REF-D2/REF-D8
  (greened across REF-P10/P11/P13) — M3 does not start over a red REF-D1. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: Refs reindex-from-source. Body lists: 5.8 reindex implemented; REF-D4 (CI)
  greened (byte-parity, typed-wins); one-code-path no-backdoor (no-cross-db green); the full-scale-drill floor
  named (REF-P24); reindex mutation score measured. Branch first; do not push unless asked. Co-Authored-By
  trailer.

---

### REF-P17 — Git producer edges + content-anchored line-range sub-anchors + per-blob replay

- **BAND.** M3.
- **ROADMAP MILESTONE.** R-M3 (planning/06-roadmaps/shared/reference-graph.md §2 "R-M3 — Producer edges light
  up", the Git half).
- **DEPENDS-ON.** REF-P5..REF-P16 (the Refs core green — REF-D1/D2/D7/D8/D9/D4-CI/D5-CI). The M3 Git producer
  prompts that ship the three content nodes, project(ref, viewer), the content-anchored line-range sub_anchor
  resolver, per-blob/ref replay, and pseudonymous commit authors. The index resolves these to the Git M3 P-NNN.
  (AG-D4 green is a band precondition, not a Refs dependency.)
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1, §3 (name-your-floors); ../../external-insights/04-hard-problems.md §1 (erasure vs
    immutability — pseudonymous commit authors); ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §3.5 (Git line-ranges
    content-anchored), §4.6 (the ladder on real sub-anchors), §4.7 (sub-artifact-granular replay).
  - Contracts: contract-index.md rows 5.4 (the Git producer edges), 5.6 (Git project + sub_anchor), 5.7 (the
    #sub kinds on real Git sub-anchors), 2.6 (Git per-blob/ref replay), 5.9 (the Git<->CI CheckStatus seam —
    Refs' grammar half: check-/step- kinds now used; Git ships the consumer/projection awaiting CI's producer in
    R-M4), 4.9 (the Git ReBAC fragment flowing through list_objects).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M3 (the Git bullets) + §1.2 rows 5.6, 5.9.
  - Drill source: REF-D1/REF-D2 (re-confirm on the real Git corpus, ~346/347), REF-D9 (real Git sub-anchors,
    ~354), REF-D4 (Git corpus, ~349); GIT-D7 (the Refs half of force-push anchor resolution; in the master M3
    exit gate).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate (the engine is
  UNCHANGED; this prompt wires Refs to the REAL Git producer and re-confirms the invariants on the Git corpus):
  - Consume Git-produced reference edges: commit-trailer / PR-link / "Closes <issue>" references emit
    refs.edge.created via the three content nodes; resolve the Git sub_anchor resolver for content-anchored line
    ranges (#L<a>-L<b> -> exact/rebased/partial/tombstone via BLAKE3 + 3-way context match) and PR review-thread
    comment-/thread- anchors through the REF-P15 ladder.
  - The Git ReBAC fragment (4.9) flows through list_objects so the PR/repo backlink lists are leak-free (the
    GIT-D11 SetExpr JOIN, reusing REF-P11).
  - Drive sub-artifact-granular replay (2.6) for Git (per-blob/ref) so a scoped reindex re-emits the right grain
    and the content-anchored line-range anchors re-derive (never a stale raw line number).
  - Use (do not build) the check-/step- #sub kinds (frozen in REF-P1): Git's check_status projection +
    details_ref (#step-<n>) resolve through the same Refs ladder; CI's producer half lands in R-M4 (REF-P19).
  - FLOOR named: in-cell single-home-cell graph build (cross-cell fan-out is R-M5, REF-P26); Git
    pseudonymous-by-default commit authors as origin_actor (the audited history-rewrite erasure path 10.6 is
    R-M5/on-demand). Both are Git deliverables Refs depends on; named here because they gate Refs' clean erasure
    surface (REF-D5 / GIT-D2). KN edges + the first lifecycle mirror are REF-P18.
- **CONTRACTS TO IMPLEMENT.** 5.4 (consume the real Git producer edges), 5.6 (consume Git project + sub_anchor),
  5.7 (the #sub kinds on real Git sub-anchors), 2.6 (drive Git replay). To the frozen shapes; the engine does
  not change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - REF-D1 / REF-D2 re-confirmed green on the REAL Git edge corpus (the leak + IDOR invariants hold on
    production-shaped Git edges, not just the M2 synthetic corpus) — CI (the gate-invariant ratchet).
  - REF-D9 green on Git content-anchored line-ranges + PR comment-/thread- anchors: a force-pushed PR line-range
    resolves MOVED/OUTDATED/GONE; the root is always carried. 0 dangling embed, 0 hard 404, no leak. This is the
    Refs half of GIT-D7 (anchors resolve LIVE/MOVED/OUTDATED/GONE; 0 mis-anchored) — CI.
  - REF-D4 reindex-parity green on a Git corpus (cold == live incl. content-anchored line-ranges +
    block-granular sub-artifacts), small-to-moderate scale — CI/SCHED.
- **TESTS (required).** Integration tests against the real Git producer: edges ingested, Git sub-anchors resolved
  through the ladder. A chained test: force-push a PR line-range others embed -> the embed resolves
  MOVED/OUTDATED/GONE with the root carried. The drill scenarios for REF-D1/REF-D2 (real Git corpus), REF-D9
  (real Git sub-anchors), REF-D4 (Git). No new Refs mutation-core module (the engine is fixed) — state that the
  REF-P11/P13/P15 mutation floors still hold on the Git corpus.
- **DEFINITION OF DONE.** Refs ingests + resolves the real Git producer edges; REF-D1/REF-D2 re-confirmed on the
  Git corpus; REF-D9 green on real Git sub-anchors (the Refs half of GIT-D7); REF-D4 reindex-parity green on a
  Git corpus; the engine is unchanged; the cross-cell + pseudonymous-author floors are named (REF-P26; 10.6
  R-M5); tests pass; committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M3: Refs consumes Git producer edges + content-anchored line-ranges. Body lists:
  5.4/5.6/5.7/2.6 wired to the real Git producer; REF-D1/REF-D2 re-confirmed on the Git corpus; REF-D9 greened
  on real Git sub-anchors (Refs half of GIT-D7); REF-D4 Git reindex-parity greened; the cross-cell +
  pseudonymous-author floors named. Branch first; do not push unless asked. Co-Authored-By trailer.

---

### REF-P18 — Knowledge producer edges + block/row sub-anchors + the first real lifecycle mirror (page_parent)

- **BAND.** M3.
- **ROADMAP MILESTONE.** R-M3 (planning/06-roadmaps/shared/reference-graph.md §2 "R-M3", the Knowledge half +
  the first lifecycle mirror).
- **DEPENDS-ON.** REF-P17 (the real-producer wiring pattern + Git edges in place), REF-P14 (the TE-7 mirror
  discipline this lights up with the first real mirror). The M3 Knowledge producer prompts that ship the three
  content nodes, project(ref, viewer), the block/heading/row sub_anchor resolver, page-subtree replay, and KN's
  page_parent typed-lifecycle events. The index resolves these to the Knowledge M3 P-NNN.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1, §3; ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §4.6 (the ladder on real KN
    sub-anchors), §3.3 (the TE-7 mirror — KN page_parent the first real mirror), §4.7 (sub-artifact-granular
    replay at block granularity).
  - Contracts: contract-index.md rows 5.4 (the KN producer edges), 5.6 (KN project + sub_anchor), 5.7 (the #sub
    kinds on real KN sub-anchors), 5.5 (KN page_parent typed mirror — the FIRST real mirror), 2.6
    (sub-artifact-granular replay at block granularity).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M3 (the KN bullets + the page_parent mirror) +
    §1.2 rows 5.5 (KN), 5.6.
  - Drill source: REF-D1/REF-D2 (re-confirm on the real KN corpus, ~346/347), REF-D9 (real KN sub-anchors, ~354),
    REF-D4 (KN corpus + the page_parent mirror reconvergence, ~349).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate (engine
  UNCHANGED; the KN producer edges + the first real lifecycle mirror arrive):
  - Consume Knowledge-produced edges: KN block/heading/row embeds emit refs.edge.created; resolve KN's sub_anchor
    resolver for b<id>/h<id>/row-/field- anchors (stable -> LIVE; edited -> OUTDATED; deleted -> GONE) through
    the REF-P15 ladder.
  - Project KN's page_parent typed-lifecycle events (the FIRST real lifecycle mirror) as lifecycle-class edges
    with the REF-P14 inverse pairing — the first time the TE-7 mirror discipline runs over a real typed table.
  - Drive sub-artifact-granular replay (2.6) for KN (page-subtree at block granularity) so a scoped reindex
    re-emits the right grain and the block anchors re-derive (never a stale positional index).
  - FLOOR named: in-cell single-home-cell graph build (cross-cell fan-out R-M5, REF-P26). Issues issue_relation
    (the second mirror) + CI check seam + Chat unfurls are R-M4 (REF-P19/REF-P20/REF-P21).
- **CONTRACTS TO IMPLEMENT.** 5.4 (consume the real KN producer edges), 5.6 (consume KN project + sub_anchor),
  5.7 (the #sub kinds on real KN sub-anchors), 5.5 (project KN page_parent — the first real mirror), 2.6 (drive
  KN replay). To the frozen shapes; the engine does not change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - REF-D1 / REF-D2 re-confirmed green on the REAL KN edge corpus — CI (the gate-invariant ratchet).
  - REF-D9 green on KN block/row anchors: an edited/deleted KN block resolves OUTDATED/GONE; the root is always
    carried. 0 dangling embed, 0 hard 404, no leak — CI.
  - REF-D4 reindex-parity green on a KN corpus incl. the KN page_parent lifecycle mirror reconverging to the
    typed table (typed wins) — CI/SCHED.
- **TESTS (required).** Integration tests against the real KN producer: edges ingested, KN sub-anchors resolved
  through the ladder, the page_parent mirror projected with correct inverse pairing. A chained test: edit a KN
  block others embed -> the embed resolves OUTDATED with the root carried; an out-of-band page_parent change ->
  scoped reindex reconverges to the typed table. The drill scenarios for REF-D1/REF-D2 (real KN corpus), REF-D9
  (real KN sub-anchors), REF-D4 (KN + page_parent). State the REF-P11/P13/P14/P15 mutation floors still hold on
  the KN corpus.
- **DEFINITION OF DONE.** Refs ingests + resolves the real KN producer edges; the page_parent first real mirror
  is projected with correct inverse pairing; REF-D1/REF-D2 re-confirmed on the KN corpus; REF-D9 green on real
  KN sub-anchors; REF-D4 reindex-parity green on a KN corpus incl. the mirror reconvergence; the engine is
  unchanged; the cross-cell floor is named (REF-P26); the Refs half of E2E-1 (the PR pane) behaviour is proven
  in-context (the confidential linked issue unfurls to a tombstone carrying the root, title never present);
  tests pass; committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M3: Refs consumes Knowledge edges + the first lifecycle mirror. Body lists:
  5.4/5.6/5.7/5.5/2.6 wired to the real KN producer; the page_parent first real mirror projected; REF-D1/REF-D2
  re-confirmed on the KN corpus; REF-D9 greened on real KN sub-anchors; REF-D4 KN reindex-parity greened (mirror
  reconverges, typed wins); the cross-cell floor named. Branch first; do not push unless asked. Co-Authored-By
  trailer.

---

### REF-P19 — The Git<->CI CheckStatus seam closes: resolve the check-/step- sub-anchors (Refs' half of X-1)

- **BAND.** M4.
- **ROADMAP MILESTONE.** R-M4 (planning/06-roadmaps/shared/reference-graph.md §2 "R-M4 — Consumer-subsystem
  edges", the CI check-seam half).
- **DEPENDS-ON.** REF-P17 (Git's check_status projection + #step- consumer awaiting CI), REF-P18 (the KN edges
  traversable; the ladder green on real sub-anchors). The M4 CI producer prompts that ship CI's ci.check.updated
  producer half + details_ref step anchor + the sealed CI log segments (11.8). The index resolves these to the
  CI M4 P-NNN. (AG-D4 re-confirmed green is a band precondition.)
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2; ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §3.5 / §4.6 (the check-/step-
    kinds resolve through the one ladder, C-6).
  - Reconciliation: 00-reconciliation-decisions.md X-1 (the Git<->CI CheckStatus seam; CI is the producer half),
    C-6 (check-/step- first-class #sub kinds).
  - Contracts: contract-index.md rows 5.9 (the Git<->CI CheckStatus seam — CI's producer half closes X-1; Refs
    resolves the check-/step- sub-anchors), 5.7 (the check-/step- kinds), 11.8 (the sealed CI log segments the
    details_ref resolves through).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M4 (the CI check-seam bullet) + §1.2 row 5.9.
  - Drill source: REF-D9 (CI check-/step- anchors, ~354); the X-1 seam GIT-D10/CI-D8 (Refs proves only that the
    check/step anchors resolve).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate (engine
  unchanged; CI's producer half arrives):
  - The Git<->CI CheckStatus seam, CI's producer half closes (5.9, X-1): CI emits ci.check.updated per
    (commit_oid, context) with run_attempt monotonic supersession; the details_ref = #step-<n> jump-to-failure
    anchor resolves through the Refs ladder (the grammar Refs froze in REF-P1, the consumer Git built in
    REF-P17). Refs' role is the SUB-ANCHOR resolution of check-<context> / step-<n> — the seam itself
    (out-of-order supersession, fork-success-neutral, the merge-queue wake) is the Git+CI X-1 deliverable
    (GIT-D10/CI-D8); Refs proves only that the check/step anchors resolve correctly through the one ladder
    (incl. resolving through the 11.8 sealed log segments).
  - FLOOR named: no new Refs floor — the engine is fixed at M2; this prompt adds only the CI sub-anchor
    resolution. Issues issue_relation (the second mirror) is REF-P20; Chat unfurls are REF-P21.
- **CONTRACTS TO IMPLEMENT.** 5.9 (resolve the CI check-/step- sub-anchors — the Refs half of X-1), 5.7 (the
  check-/step- kinds on real CI sub-anchors). Consumed: 11.8 (the sealed CI log segments). To the frozen shapes;
  the engine does not change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - REF-D9 green on CI check-/step- anchors: every check-<context> / step-<n> resolves through the one ladder to
    the correct state with the root carried (the Refs half of the X-1 details_ref resolution, incl. resolving
    through the 11.8 sealed log segments) — CI.
  - An out-of-order ci.check.updated -> Refs resolves the latest by run_attempt context (monotonic supersession
    honoured at the sub-anchor level) — CI.
- **TESTS (required).** Integration tests against the CI producer: check/step anchors resolve through the ladder
  incl. through the sealed log segments. A chained test: emit an out-of-order ci.check.updated -> Refs resolves
  the latest by run_attempt context. The drill scenario for REF-D9 (CI anchors). The CDC pair for the Refs
  consumer side of 5.9. State the REF-P15 ladder mutation floor still holds on the CI anchors.
- **DEFINITION OF DONE.** Refs resolves the CI check/step anchors (the Refs half of X-1) incl. through the
  sealed log segments; REF-D9 green on the CI anchors; out-of-order supersession honoured at the sub-anchor
  level; the engine is unchanged; tests pass; committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M4: Refs resolves the CI check seam sub-anchors (Refs half of X-1). Body lists:
  5.9 wired (Refs sub-anchor half); REF-D9 greened on CI check-/step- anchors (incl. sealed log segments);
  out-of-order supersession honoured. Branch first; do not push unless asked. Co-Authored-By trailer.

---

### REF-P20 — Issues lifecycle edges: the second real TE-7 mirror (issue_relation)

- **BAND.** M4.
- **ROADMAP MILESTONE.** R-M4 (planning/06-roadmaps/shared/reference-graph.md §2 "R-M4", the issue-relations
  half).
- **DEPENDS-ON.** REF-P18 (the first real mirror — page_parent — in place; the mirror discipline proven on a
  real table), REF-P19 (the M4 producer-edge wiring in motion). The M4 Issues producer prompts that ship Issues'
  three content nodes + issue_relation typed events + project + key/sub-anchors. The index resolves these to the
  Issues M4 P-NNN.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2; ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §3.3 (the second real TE-7 mirror
    — Issues issue_relation), §4.5 (the lineage traverse it enables).
  - Contracts: contract-index.md rows 5.5 (Issues issue_relation — the second TE-7 mirror), 5.6 (Issues project
    for the <PROJECTKEY>-<seqno> key + field-/row- sub-anchors), 5.7 (the field-/row- kinds).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M4 (the issue-relations bullet) + §1.2 row 5.5.
  - Drill source: REF-D4 (the TE-7 second-mirror reconvergence, ~349); supports ISS-D6 typed-relation
    correctness.
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate (engine
  unchanged; the second lifecycle mirror arrives):
  - Issues lifecycle edges — the second TE-7 mirror (5.5): Issues' issue_relation typed events (closes/blocks/
    blocked_by/depends_on/relates/parent/assigns) land here; Refs projects them as lifecycle-class edges with
    the REF-P14 inverse pairing, so the spec-to-ship lineage (initiative -> child issues -> PRs -> commits -> CI
    -> deploy -> chat decision) is ONE Refs traverse, not a five-way fan-out.
  - Resolve Issues' project for the <PROJECTKEY>-<seqno> key + field-/row- sub-anchors through the REF-P15
    ladder.
  - FLOOR named: no new Refs floor — the engine is fixed at M2. Chat unfurls (the maximal consumer) are REF-P21.
- **CONTRACTS TO IMPLEMENT.** 5.5 (project Issues issue_relation — the second real mirror), 5.6/5.7 (Issues
  project + field-/row- sub-anchors). To the frozen shapes; the engine does not change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - REF-D9 green on Issues field-/row- anchors: every #sub kind resolves through the one ladder to the correct
    state with the root carried — CI.
  - The lifecycle-mirror reconvergence check (TE-7): an out-of-band edit to an issue_relation row -> a scoped
    reindex reconverges Refs to the typed table (typed wins) — proves REF-D4's TE-7 half on the SECOND real
    mirror (supports ISS-D6) — CI.
- **TESTS (required).** Integration tests against the Issues producer: issue_relation projected with correct
  inverse pairing; Issues sub-anchors resolve. A chained test: an out-of-band issue_relation edit -> scoped
  reindex reconverges to the typed table. The drill scenarios for REF-D9 (Issues anchors) + the TE-7
  reconvergence. The CDC pair for the Refs consumer side of 5.5 (Issues). State the REF-P14/P15 mutation floors
  still hold on the Issues corpus.
- **DEFINITION OF DONE.** Refs projects the Issues second mirror with correct inverse pairing; REF-D9 green on
  Issues field-/row- anchors; the TE-7 second-mirror reconvergence proven (typed wins); the spec-to-ship lineage
  is one traverse; the engine is unchanged; tests pass; committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M4: Refs Issues lifecycle edges (the second TE-7 mirror). Body lists: 5.5 wired
  (Issues second mirror); REF-D9 greened on Issues field-/row- anchors; the TE-7 second-mirror reconvergence
  proven (typed wins). Branch first; do not push unless asked. Co-Authored-By trailer.

---

### REF-P21 — Chat unfurls: the maximal consumer + cross-subsystem traversal complete

- **BAND.** M4.
- **ROADMAP MILESTONE.** R-M4 (planning/06-roadmaps/shared/reference-graph.md §2 "R-M4", the chat-unfurls half;
  completes R-M4).
- **DEPENDS-ON.** REF-P19 (CI check anchors resolve), REF-P20 (Issues second mirror). The M4 Chat producer
  prompts that ship Chat's three content nodes + message-/thread- anchors + the channel ReBAC fragment. The
  index resolves these to the Chat M4 P-NNN.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (Chat references any artifact); ../../external-insights/01-process-and-quality-doctrine.md
    §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §4.2 (Chat unfurls via resolve +
    the shared per-ref cache busting on *.updated).
  - Contracts: contract-index.md rows 5.4 (Chat edges), 5.6 (Chat project + message-/thread- sub-anchors), 5.7
    (the message-/thread- kinds), 4.9 (the Chat channel ReBAC fragment flowing through list_objects).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M4 (the chat-unfurls + cross-subsystem-complete
    bullets).
  - Drill source: REF-D1/REF-D2 (full five-producer corpus, ~346/347), REF-D9 (Chat message-/thread- anchors,
    ~354); CHAT-D5 (confidential-unfurl tombstone, master M4 exit gate).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate (engine
  unchanged; the final producer's edges arrive):
  - Chat unfurls — the maximal consumer: Chat's mention/artifact_ref/embed nodes produce edges; Chat consumes
    resolve for every unfurl (commit / issue / doc / CI run) through the 4-step ladder + the shared per-ref cache
    busting on *.updated; message-/thread- sub-anchors resolve (immutable -> LIVE; deleted -> GONE). The Chat
    ReBAC fragment (channel.read = member + parent_project->read) flows through list_objects so a search/backlink
    as a non-member returns 0.
  - Cross-subsystem traversal is now COMPLETE: all five producers emit the structured inline nodes uniformly
    (X-2) + Issues/KN own both typed-relation tables, so mention/ref/lifecycle edges are dependable across
    Git/CI/KN/Issues/Chat.
  - FLOOR named: in-cell single-home-cell graph build (cross-cell fan-out R-M5, REF-P26); no new Refs floor in
    M4 — the engine is fixed at M2.
- **CONTRACTS TO IMPLEMENT.** 5.4 (consume Chat edges), 5.6/5.7 (Chat project + message-/thread- sub-anchors).
  To the frozen shapes; the engine does not change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - REF-D1 / REF-D2 green on the FULL five-producer corpus — the leak + IDOR invariants hold across Issues + CI
    check/step anchors + Chat unfurls (the most adversarial corpus: confidential issues, private channels,
    fork-scoped CI). 0 leak, 0 cross-tenant edge — CI.
  - REF-D9 green on Chat message-/thread- anchors — every #sub kind resolves through the one ladder to the
    correct state with the root carried (supports CHAT-D5 confidential-unfurl -> tombstone, 0 title leak) — CI.
  - A non-member search/backlink returns 0 (the Chat channel ReBAC fragment flows through list_objects) — CI.
- **TESTS (required).** Integration tests against the Chat producer: chat unfurls degrade via the ladder; a
  non-member search/backlink returns 0; message-/thread- anchors resolve. A chained test: a deleted chat message
  others embed -> the embed resolves GONE with the root carried. The drill scenarios for REF-D1/REF-D2 (full
  corpus), REF-D9 (Chat anchors). The CDC pair for the Refs consumer side of 5.4 (Chat). State the REF-P11/P15
  mutation floors still hold on the full corpus.
- **DEFINITION OF DONE.** Refs serves Chat unfurls through the ladder; REF-D1/REF-D2 green on the full
  five-producer corpus; REF-D9 green on Chat anchors; the non-member-returns-0 ReBAC property holds;
  cross-subsystem traversal is complete; the engine is unchanged; the cross-cell floor is named (REF-P26); the
  Refs half of E2E-1 lights up end-to-end in-context; tests pass; committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M4: Refs Chat unfurls + cross-subsystem traversal complete. Body lists: 5.4 wired
  (Chat); REF-D1/REF-D2 greened on the full five-producer corpus; REF-D9 greened on Chat anchors; non-member
  search/backlink returns 0; the cross-cell floor named (REF-P26). Branch first; do not push unless asked.
  Co-Authored-By trailer.

---

### REF-P22 — World-scale: the 30x surge + the protected-human-lane shed order (REF-D10)

- **BAND.** M5.
- **ROADMAP MILESTONE.** R-M5 partial (planning/06-roadmaps/shared/reference-graph.md §2 "R-M5", the REF-D10
  surge deliverable).
- **DEPENDS-ON.** REF-P21 (all five producer corpora traversable; the deterministic correctness drills green).
  The M5 surge harness numbers (OQ-K). The protected-human-lane shed order (1.11). The index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scalable from day 1); ../../external-insights/01-process-and-quality-doctrine.md §3
    (the 1x/10x/30x load generator), §2 (the protected human lane).
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §6.2 (measure before you shard).
  - Contracts: contract-index.md rows 5.3 (the backlink read at scale), 1.11 (the protected-human-lane shed
    order + per-surface shed budgets OQ-K), 1.8 (the shed-count telemetry).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M5 (the 30x surge bullet) + §3
    (production-hardened).
  - Drill source: REF-D10 (30x surge, ~355).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate:
  - The 30x agent ref-creation + backlink-read surge handling (REF-D10): tune the protected-human-lane shed
    order (1.11) to Refs' two surfaces — a human's interactive backlink/traverse read holds the protected lane;
    agent ref-creation + backlink-read sheds with 429 + Retry-After; per-tenant in-flight caps keep one tenant's
    agent storm off another's humans (the per-tenant bulkhead). Set the per-surface shed-budget NUMBERS (OQ-K)
    from MEASUREMENT, not prediction; write them into the thresholds file.
  - Sharding edge IF measured (§6.2): the shard key is already (tenant, region) + target_root hash, so a measured
    hot tenant outgrowing one shard is a re-home, not a redesign — measured here, not before. (State as a
    measured-only branch.)
  - FLOOR named: the hot-artifact reach index R4 (the named REF-P11 floor's follow-on) is REF-P23 — this prompt
    is the surge/shed-order half only.
- **CONTRACTS TO IMPLEMENT.** 1.11 the shed order tuned to Refs' surfaces. Consumed: 5.3 at scale (the read the
  shed order protects). To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - REF-D10 (30x surge): the human backlink-read lane holds (interactive latency within budget), the agent
    ref-creation + read lane sheds (429 + Retry-After honoured), other tenants unaffected. Green artifact:
    shed-counts + read p99 — SCHED (part of the master M5 F6 surge family).
- **TESTS (required).** The drill-harness scenario for REF-D10 (the 30x mixed-principal surge). State the
  REF-P11 SetExpr-lowering mutation floor still holds under load (the leak invariant must not regress under
  shed).
- **DEFINITION OF DONE.** The shed order is tuned + the budgets measured into the thresholds file; REF-D10 emits
  a dated green artifact (human lane holds / agent sheds / other tenants unaffected); the R4 follow-on is named
  (REF-P23); tests pass; committed. No threshold weakened (a missed budget becomes a dated claimed-not-proven
  row, not an edited green).
- **COMMIT.** Header: P-<NNN> M5: Refs 30x surge + protected-human-lane shed order. Body lists: 1.11 shed order
  tuned (measured OQ-K budgets); REF-D10 greened (shed-counts / read p99); the R4 follow-on named (REF-P23).
  Branch first; do not push unless asked. Co-Authored-By trailer.

---

### REF-P23 — World-scale: the hot-artifact reach index R4 (measured-trigger; the REF-P11 floor's follow-on)

- **BAND.** M5.
- **ROADMAP MILESTONE.** R-M5 partial (the REF-D3 hot-fanout / R4 follow-on).
- **DEPENDS-ON.** REF-P22 (the surge/shed-order in place; the read budget measured), REF-P11 (the backlink-read
  CTE floor whose follow-on this is). The M5 storage read replica. The index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scalable from day 1); ../../external-insights/01-process-and-quality-doctrine.md §3
    (prove-it); ../../external-insights/02-platform-substrate.md §7 (Leopard reach index, measured-trigger).
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §6.2 (the read replica the named
    first move), §6.3 (the hot-artifact backlink scale — the read-time CTE + list_objects filter + pagination +
    replica floor; the Leopard R4 follow-on promoted at measured hot-fanout > read budget), §3.7 (R4 the FLOOR
    component).
  - Contracts: contract-index.md rows 5.3 (the backlink read at scale), 4.3 (R4 gated by the same list_objects
    filter), 1.8 (the hot_artifact_fanout telemetry).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M5 (the hot-artifact reach index bullet).
  - Drill source: REF-D3 (hot-fanout, ~348).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate:
  - The hot-artifact backlink scale, the "viral PR / referenced-by-50,000" case (REF-D3): the read-time CTE +
    list_objects filter + pagination + read replica (the doctrine's named first scaling move) is the BUILT floor
    (REF-P11). Build the Leopard-style flattened reach index R4 — derived/rebuildable from R1, incrementally
    maintained from refs.edge.*, gated by the SAME list_objects filter (REF-P11) — and PROMOTE it only when
    measured hot-fanout exceeds the read budget (R5), not predicted. R4 serves post-promotion; the property
    (paginated, leak-free) is fixed at M2, the index is measured here.
  - FLOOR resolved: this prompt SHIPS the R4 follow-on whose floor was named in REF-P11 — link the pair in the
    commit so the gap is visible.
- **CONTRACTS TO IMPLEMENT.** 5.3 at scale (owned, the R4 path). Consumed: 4.3 (R4 gated by the same filter). To
  the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - REF-D3 (hot-fanout): "referenced-by-50,000" under concurrent permission-filtered reads -> paginated p99
    within budget; the hot_artifact_fanout telemetry fires; R4 serves post-promotion — SCHED.
- **TESTS (required).** The drill-harness scenario for REF-D3 (the 50,000-backlink hot artifact under concurrent
  filtered reads). A test that R4, once promoted, returns the same leak-free result set as the CTE floor (parity
  between the two paths). State the REF-P11 SetExpr-lowering mutation floor still holds on R4 (R4 is gated by
  the same filter — the leak invariant must not regress).
- **DEFINITION OF DONE.** R4 is built + measured-promotion-gated; REF-D3 emits a dated green artifact (paginated
  p99 within budget / R4 parity); the R4 follow-on is linked to its REF-P11 floor; tests pass; committed. No
  threshold weakened.
- **COMMIT.** Header: P-<NNN> M5: Refs hot-artifact reach index R4. Body lists: R4 built +
  measured-trigger-gated; REF-D3 greened (read p99 / R4 parity); the R4 follow-on linked to the REF-P11 floor.
  Branch first; do not push unless asked. Co-Authored-By trailer.

---

### REF-P24 — World-scale: reindex-parity at full scale across both TE-7 mirrors (REF-D4 at scale)

- **BAND.** M5.
- **ROADMAP MILESTONE.** R-M5 partial (the REF-D4-at-scale deliverable).
- **DEPENDS-ON.** REF-P16 (reindex), REF-P21 (the full five-producer corpus incl. both TE-7 mirrors). The index
  resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe, world-scale); ../../external-insights/04-hard-problems.md §5.3
    (reindex-from-source at scale); ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §4.7 (reindex-from-source), §7
    D-4 (the scale variant).
  - Contracts: contract-index.md rows 5.8 (reindex at scale), 1.8 (reindex_parity telemetry).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M5 (the reindex-parity-at-full-scale bullet) +
    §3.
  - Drill source: REF-D4 (reindex-parity at full scale, ~349).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate:
  - reindex-parity at full scale (REF-D4 at scale): wipe the edge index, reindex -> byte-matches live across the
    FULL five-producer corpus incl. BOTH TE-7 lifecycle mirrors (KN page_parent + Issues issue_relation). The
    reindex_parity telemetry (1.8) fires.
  - FLOOR resolved: this prompt promotes the REF-P16 CI-variant drill (REF-D4) to its full-scale form — link the
    pair in the commit.
- **CONTRACTS TO IMPLEMENT.** 5.8 reindex at scale (owned). To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - REF-D4 (reindex-parity at full scale): rebuilt edge index byte-matches live across the full five-producer
    corpus + both TE-7 mirrors. Green artifact: the reindex-parity hash — SCHED.
- **TESTS (required).** The drill-harness scenario for REF-D4 (full-scale reindex). State the REF-P16 reindex
  mutation floor still holds at scale.
- **DEFINITION OF DONE.** reindex byte-parity at full scale across both TE-7 mirrors; REF-D4 emits a dated green
  artifact (parity hash); the CI-variant -> full-scale promotion is linked to its REF-P16 floor; tests pass;
  committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M5: Refs reindex-parity at full scale. Body lists: 5.8 at scale; REF-D4
  (full-scale parity across both TE-7 mirrors) greened; the full-scale promotion linked to the REF-P16 CI floor.
  Branch first; do not push unless asked. Co-Authored-By trailer.

---

### REF-P25 — World-scale: restore + re-erase at backup scale (REF-D5 at backup scale)

- **BAND.** M5.
- **ROADMAP MILESTONE.** R-M5 partial (the REF-D5-at-backup-scale deliverable).
- **DEPENDS-ON.** REF-P15 (the erase holder / ladder), REF-P24 (reindex at scale). The M5 restore-verify at cell
  scale (STOR-D2) + the full DSR fan-out (10.4) + the erasure ledger (10.8) + backup/restore/cross-seam (11.5).
  The index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe, world-scale); ../../external-insights/04-hard-problems.md §1 (no resurrected
    PII past an erasure); ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §4.6 tail (the erase holder), §7
    D-5 (the scale variant).
  - Contracts: contract-index.md rows 10.1 (the erase holder at backup scale), 10.8 (the erasure ledger —
    post-restore re-erasure), 11.5 (backup/restore/cross-seam).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M5 (the restore + cross-seam + re-erase at
    scale bullet) + §3.
  - Drill source: REF-D5 (erasure at backup scale, ~350).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate:
  - Restore + cross-seam + re-erase at scale (REF-D5 at backup scale, F3): restore the edge index with
    OLTP/blob/offsets to a consistent point -> NO resurrected edges past an erasure (post-restore re-erasure runs
    from the erasure ledger, 10.8); references stay tombstoned, the person stays unresolvable. This folds into
    the M5 DSAR fan-out (E2E-4 — REF-P27 carries the E2E run; this prompt builds + drills the Refs restore/
    re-erase mechanism it depends on).
  - FLOOR resolved: this prompt promotes the REF-P15 CI-variant drill (REF-D5) to its backup-scale form — link
    the pair in the commit.
- **CONTRACTS TO IMPLEMENT.** 10.1 erase at backup scale (owned). Consumed: 10.8 the erasure ledger, 11.5
  restore. To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - REF-D5 (erasure at backup scale): erase a subject + a referenced artifact -> references tombstone, the
    person unresolvable, 0 recoverable PII in edge/cache/backups, no 500 on resolve; post-restore re-erasure from
    the ledger leaves 0 resurrected PII. Green artifact: erase-receipt + 0 resolve-error — SCHED (folded into
    E2E-4).
- **TESTS (required).** The drill-harness scenario for REF-D5 (backup-scale erase + the post-restore re-erase).
  A chained test: erase -> restore from a pre-erase backup -> re-erase from the ledger -> assert 0 recoverable
  PII. State the REF-P15 erase mutation floor still holds at scale.
- **DEFINITION OF DONE.** restore + re-erase leaves 0 resurrected PII; REF-D5 emits a dated green artifact (0
  recoverable PII / 0 resolve-error); the CI-variant -> backup-scale promotion is linked to its REF-P15 floor;
  tests pass; committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M5: Refs restore + re-erase at backup scale. Body lists: 10.1 at backup scale;
  REF-D5 (backup-scale erase, 0 recoverable PII) greened; the backup-scale promotion linked to the REF-P15 CI
  floor. Branch first; do not push unless asked. Co-Authored-By trailer.

---

### REF-P26 — World-scale: the cross-cell backlink fan-out build (the REF-P10 floor's follow-on)

- **BAND.** M5.
- **ROADMAP MILESTONE.** R-M5 partial (the cross-cell fan-out follow-on).
- **DEPENDS-ON.** REF-P10 (the cell-local cross-cell resolution semantics whose fan-out this builds), REF-P23,
  REF-P24, REF-P25 (the surge + reindex/re-erase at scale green). The M5 multi-cell bridge live (12.6) + the
  FLOOR drills GA-D8/CP-D7/CP-D8. The index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1, §3 (EU-sovereign, residency by construction);
    ../../external-insights/04-hard-problems.md §1 (cross-region PII-free); §5.3;
    ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §4.2 (cross-cell resolution
    pinned cell-local, C-5 — the home cell renders + permission-checks; only the projection/tombstone crosses,
    over the frozen CrossCellPointer), §6.5 (the cross-cell backlink fan-out FLOOR build).
  - Reconciliation: 00-reconciliation-decisions.md C-5 (cross-cell resolution semantics frozen), OQ-I
    (single-cell -> multi-cell).
  - Contracts: contract-index.md row 12.6 (the cross-cell PII-free pointer bridge CrossCellPointer{subject,
    type, correlation_id, home_cell}; resolution always cell-local), 5.2 (resolve, now cross-cell), 5.3
    (traverse, cross-cell).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M5 (the cross-cell fan-out bullet) + §3.
  - Drill source: GA-D8/CP-D7/CP-D8 (the cross-cell FLOOR drills, master M5 exit gate).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate:
  - The cross-cell backlink fan-out BUILD — the named M2 floor's follow-on (the deepest remaining Refs unknown):
    when multi-cell goes live (12.6), the cross-cell RESOLUTION semantics (already frozen cell-local in REF-P10 —
    the home cell renders + permission-checks; only the projection or a tombstone crosses, over the frozen
    CrossCellPointer) get their FAN-OUT build (ISS cross-cell portfolio rollup, KN cross-cell collab, CHAT
    cross-org channels). The §5 contracts are cell-agnostic so the build EXTENDS WITHOUT A REWRITE. The FLOOR
    drills GA-D8/CP-D7/CP-D8 are now owed and run. Until multi-cell goes live the single-cell path is complete
    and the design is the named floor — link this build to the REF-P10 floor in the commit.
  - FLOOR resolved: this prompt SHIPS the cross-cell fan-out follow-on whose floor was named in REF-P10. The
    whole-system E2E wedge (E2E-1/E2E-3/E2E-4) is REF-P27.
- **CONTRACTS TO IMPLEMENT.** 12.6 the cross-cell PII-free bridge (consumed; the fan-out build rides it), 5.2/5.3
  cross-cell (owned, extended). To the frozen shapes; the build extends without a rewrite.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - GA-D8 / CP-D7 / CP-D8 (the cross-cell FLOOR drills, now owed): the cross-cell erasure receipt set; cell->cell
    migration 0 loss; the cross-cell ref PII-free bridge (only the projection/tombstone crosses, never raw rows/
    PII). Green artifacts: the per-cell receipt set / 0-loss migration / PII-free assertion — SCHED.
- **TESTS (required).** The cross-cell fan-out integration test (a viewer in cell A resolving a pointer homed in
  cell B -> only the projection/tombstone crosses, never raw rows). The drill-harness scenarios for
  GA-D8/CP-D7/CP-D8. State the REF-P10/P11 leak-invariant mutation floors still hold cross-cell (the leak
  invariant must not regress across the cell boundary).
- **DEFINITION OF DONE.** The cross-cell fan-out is built (extends without a rewrite); GA-D8/CP-D7/CP-D8 emit
  dated green artifacts; the cross-cell build is linked to its REF-P10 floor; the E2E-wedge follow-on is named
  (REF-P27); tests pass; committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M5: Refs cross-cell backlink fan-out build. Body lists: 12.6 cross-cell fan-out
  built (extends, no rewrite); GA-D8/CP-D7/CP-D8 greened; the cross-cell build linked to the REF-P10 floor; the
  E2E-wedge follow-on named (REF-P27). Branch first; do not push unless asked. Co-Authored-By trailer.

---

### REF-P27 — World-scale: the whole-system E2E wedge (E2E-1 PR pane / E2E-3 spec-to-ship / E2E-4 DSAR)

- **BAND.** M5.
- **ROADMAP MILESTONE.** R-M5 partial (the whole-system E2E scenarios Refs crosses; completes R-M5).
- **DEPENDS-ON.** REF-P24, REF-P25, REF-P26 (reindex/re-erase at scale + the cross-cell fan-out green). The
  other systems' M5 E2E prompts (E2E-1 PR pane, E2E-3 spec-to-ship, E2E-4 DSAR). The index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1, §3; ../../external-insights/04-hard-problems.md §1 (cross-region PII-free); §5.3;
    ../../external-insights/01-process-and-quality-doctrine.md §3, §4 (chained-mutation E2E — drive the whole
    thing).
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §1 (the moat thesis), §4.5 (the
    lineage traverse), §4.7 (reindex parity).
  - Contracts: contract-index.md rows 5.2 (resolve, the E2E-1 unfurl), 5.3 (traverse, the E2E-3 lineage walk),
    10.1 (the E2E-4 holder fan-out).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M5 (the E2E bullets) + §3
    (production-hardened).
  - Drill source: E2E-1/E2E-3/E2E-4 (testing-strategy/01... §2; the chained-mutation scenarios).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate (run the Refs
  side against a full cell with mock agents):
  - The whole-system E2E scenarios Refs crosses:
    E2E-1 (the PR context pane — Refs is the spine: every connected artifact resolves per-viewer, the
    confidential issue -> tombstone carrying the root, 0 title/count/backlink leak, the live check-update lands
    within the freshness budget);
    E2E-3 (spec-to-ship — traverse(spec_doc, viewer) walks the ENTIRE lineage depth-16 cycle-safe per-viewer,
    and the wiped Refs edge index reindexes to byte-match live, F4/REF-D4 at scale);
    E2E-4 (DSAR fan-out — Refs' edges + cache return 0 recoverable PII, unfurls degrade to tombstones, the
    holder-coverage receipt includes Refs).
  - Each emits its named green artifact.
  - FLOOR named: none new — this is the E2E run over the production-hardened engine.
- **CONTRACTS TO IMPLEMENT.** 5.2/5.3 the E2E unfurl + lineage walk (owned, exercised at E2E scale), 10.1 the
  E2E-4 holder fan-out (owned). To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - E2E-1 green (the PR pane — Refs is the spine; 0 title/count/backlink leak; the confidential issue ->
    tombstone carrying the root; the live check-update within budget) — SCHED.
  - E2E-3 green (the full lineage traverse depth-16 per-viewer + the wiped index reindexes to byte-match live) —
    SCHED.
  - E2E-4 green (Refs' edges + cache return 0 recoverable PII; unfurls -> tombstones; the holder-coverage receipt
    includes Refs) — SCHED.
- **TESTS (required).** The three chained-mutation E2E scenarios E2E-1/E2E-3/E2E-4 (the Refs side), each driving
  the whole flow end-to-end (not single handlers). State the REF-P10/P11 leak-invariant mutation floors still
  hold at E2E scale.
- **DEFINITION OF DONE.** E2E-1 (Refs as spine), E2E-3, E2E-4 each emit their named green artifact; tests pass;
  committed. This completes R-M5 — the master M5 exit gate cites E2E-1..E2E-4 green; M6 does not start over a red
  E2E-1. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M5: Refs whole-system E2E wedge (E2E-1/E2E-3/E2E-4). Body lists: E2E-1/E2E-3/E2E-4
  greened (Refs side); the PR pane (Refs as spine), the lineage traverse + reindex parity, the DSAR tombstone
  fan-out. Branch first; do not push unless asked. Co-Authored-By trailer.

---

### REF-P28 — Dogfooding: the reference graph over Myelin's own work + the self-hosting CI graph

- **BAND.** M6.
- **ROADMAP MILESTONE.** R-M6 (planning/06-roadmaps/shared/reference-graph.md §2 "R-M6 — Dogfooding", the
  run-over-own-work + self-hosting-CI-graph half).
- **DEPENDS-ON.** REF-P23, REF-P24, REF-P25, REF-P26, REF-P27 (the production-hardened reference graph — all
  Refs drills + the E2E wedge green). The M6 self-hosting CI graph + the Myelin monorepo on Myelin git hosting +
  the Myelin issues/Knowledge spaces. The index resolves these. (The switch-test browser drive is REF-P29.)
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §5 (dogfooding); ../../external-insights/01-process-and-quality-doctrine.md §1
    (code-wins-over-docs — the truth-up pass).
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §1 (the moat thesis — jump from a
    failing test to the line of code to the issue to the conversation in four keystrokes).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M6 (the run-over-own-work + self-hosting CI
    graph bullets) + §3.
  - Master sequencing: planning/06-roadmaps/00-master-sequencing.md §2 M6 (the self-hosting CI graph green; the
    truth-up pass — no earlier-band gate red).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate + the Myelin
  self-hosting deployment config:
  - Run the reference graph over Myelin's own work: the PR context pane on the Myelin monorepo's PRs (commits <->
    issues <-> CI checks <-> KN docs <-> chat threads), the spec-to-ship lineage on the roadmap/gap-report/
    scorecard living as Myelin issues + a Myelin Knowledge space (the every-incident-adds-a-drill loop files a
    Myelin issue + a reproducing drill, both reference-linked).
  - Wire the Refs drills as Myelin CI jobs on Myelin's own commits (the dogfood loop is live).
  - Run the truth-up pass: every Refs PROVEN row (REF-D1..D10 + the E2E rows) rests on a DATED green artifact,
    never a doc claim — no earlier-band Refs gate is red (code-wins-over-docs, EI-01 §1).
  - FLOOR named: none new — M6 promotes nothing; it exercises the production-hardened reference graph on real
    (self-)tenant data. The switch-test browser drive is REF-P29.
- **CONTRACTS TO IMPLEMENT.** None new — the engine is fixed at M2 and hardened through M5. This prompt exercises
  the production surface (5.2/5.3/5.7) on real self-tenant data and wires the Refs drills into the Myelin
  self-hosting CI graph.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - Refs is green on the self-hosting CI graph (the Refs drills run as Myelin CI jobs on Myelin's own commits —
    the dogfood loop is live) — SCHED.
  - The truth-up pass: every Refs PROVEN row rests on a dated green artifact; no earlier-band Refs gate is red —
    SCHED.
- **TESTS (required).** The Refs drills wired as Myelin CI jobs (the dogfood loop). A truth-up audit script that
  confirms every Refs PROVEN row links a dated green artifact.
- **DEFINITION OF DONE.** The reference graph runs over Myelin's own work; the Refs drills are green as Myelin CI
  jobs; the truth-up pass confirms no earlier-band Refs gate is red (every PROVEN row dated-and-green); the
  switch-test floor is named (REF-P29); committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M6: reference graph dogfooded on Myelin's own work + self-hosting CI graph. Body
  lists: the Refs drills green as Myelin CI jobs (the dogfood loop); the truth-up pass (0 red earlier-band Refs
  gates); the switch-test floor named (REF-P29). Branch first; do not push unless asked. Co-Authored-By trailer.

---

### REF-P29 — Dogfooding: the reference-graph switch-test surfaces driven in a browser

- **BAND.** M6.
- **ROADMAP MILESTONE.** R-M6 (planning/06-roadmaps/shared/reference-graph.md §2 "R-M6", the switch-test half).
- **DEPENDS-ON.** REF-P28 (the reference graph running over Myelin's own work; the dogfood CI loop live). The M6
  per-subsystem switch-test surfaces. The index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §5 (dogfooding); ../../external-insights/01-process-and-quality-doctrine.md §4 (the switch
    test — drive the real UI in a browser; actually try it).
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §1 (the moat thesis — the
    four-keystroke cross-artifact jump).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M6 (the switch-test bullet, the latency budgets)
    + §3.
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate + the switch-test
  harness:
  - The reference-graph contribution to the per-subsystem SWITCH TESTS (folded into the L5 done-bars): does a
    GitHub/Jira/Linear/Notion user's cross-artifact navigation work — unfurls live, backlinks complete,
    tombstones graceful — without hitting a wall the old tool didn't have? Measured against latency budgets
    (backlink read / unfurl within the keyboard / no-spinner-flash budgets). Drive the four-keystroke
    cross-artifact jump IN A BROWSER, not by reading the feature list.
  - FLOOR named: none new — M6 promotes nothing.
- **CONTRACTS TO IMPLEMENT.** None new — exercises the production surface (5.2/5.3/5.7) via the real UI.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The reference-graph switch-test surfaces pass when driven in a browser (measured latency; the four-keystroke
    cross-artifact jump works — backlink read / unfurl within the keyboard / no-spinner-flash budgets) — SCHED.
- **TESTS (required).** The switch-test browser drive (the four-keystroke jump across the five real subsystems on
  Myelin's own data, against the latency budgets). Record honestly (yes/no/partial) which switch-test surfaces
  were driven in a browser vs. only automated.
- **DEFINITION OF DONE.** The switch-test surfaces pass when driven in a browser (measured latency); any surface
  only-automated-not-browser-driven is named honestly; committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M6: reference-graph switch-test surfaces driven in a browser. Body lists: the
  switch-test surfaces driven in a browser (measured latency, the four-keystroke jump); any only-automated
  surface named. Branch first; do not push unless asked. Co-Authored-By trailer.

---

## Coverage check (every R-M milestone -> its prompt(s))

| Roadmap milestone | Band | Prompt(s) |
|---|---|---|
| R-M0 (ArtifactRef value type + the Refs ratchet) | M0 | REF-P1 (value type), REF-P2 (the four lints) |
| R-M1 (Refs as a holder + the edge-index encryption floor) | M1 | REF-P3 (holder + residency), REF-P4 (per-tenant DEK) |
| R-M2 (the Refs core) | M2 | REF-P5 (edge schema), REF-P6 (edge-builder), REF-P7 (invalidator + shim), REF-P8 (emit seam), REF-P9 (loop-guard depth stamp), REF-P10 (resolution chokepoint), REF-P11 (backlink read / SetExpr), REF-P12 (R2 cache), REF-P13 (traverse), REF-P14 (TE-7 mirror discipline), REF-P15 (#sub ladder + erasure holder), REF-P16 (reindex-from-source) |
| R-M3 (Git links + KN embeds + the first lifecycle mirror) | M3 | REF-P17 (Git edges + line-ranges), REF-P18 (KN edges + page_parent first mirror) |
| R-M4 (CI check seam + issue relations + chat unfurls) | M4 | REF-P19 (CI check seam), REF-P20 (Issues second mirror), REF-P21 (Chat unfurls + cross-subsystem complete) |
| R-M5 (30x surge + R4; reindex/restore/re-erase at scale; cross-cell fan-out + the E2E wedge) | M5 | REF-P22 (30x surge), REF-P23 (R4 reach index), REF-P24 (reindex at scale), REF-P25 (restore + re-erase at scale), REF-P26 (cross-cell fan-out), REF-P27 (E2E wedge) |
| R-M6 (dogfooding) | M6 | REF-P28 (run-over-own-work + self-hosting CI graph), REF-P29 (switch-test browser drive) |

**First-pass -> finer-pass mapping (no first-pass deliverable lost in the renumber):**
- REF-P1 (old: value type + #sub grammar + four lints) -> REF-P1 (value type + #sub grammar) + REF-P2 (the four lints).
- REF-P2 (old: holder + DEK + residency) -> REF-P3 (holder + residency) + REF-P4 (per-tenant DEK).
- REF-P3 (old: edge schema + two consumers) -> REF-P5 (schema) + REF-P6 (edge-builder) + REF-P7 (invalidator + shim).
- REF-P4 (old: emit seam + loop-guard stamp) -> REF-P8 (emit seam) + REF-P9 (loop-guard depth stamp).
- REF-P5 (old: resolution chokepoint) -> REF-P10 (atomic; unchanged in scope).
- REF-P6 (old: backlink read / SetExpr) -> REF-P11 (atomic; unchanged in scope).
- REF-P7 (old: traverse + TE-7 mirror) -> REF-P13 (traverse) + REF-P14 (TE-7 mirror discipline).
- REF-P8 (old: #sub ladder + R2 cache) -> REF-P15 (#sub ladder, joined with old REF-P9's erase holder) + REF-P12 (R2 cache).
- REF-P9 (old: erasure holder + reindex) -> REF-P15 (erase holder, joined with old REF-P8's ladder) + REF-P16 (reindex-from-source).
- REF-P10 (old: Git + KN + first mirror) -> REF-P17 (Git) + REF-P18 (KN + page_parent first mirror).
- REF-P11 (old: CI seam + issue relations + chat) -> REF-P19 (CI seam) + REF-P20 (Issues second mirror) + REF-P21 (Chat unfurls).
- REF-P12 (old: 30x surge + R4) -> REF-P22 (30x surge) + REF-P23 (R4 reach index).
- REF-P13 (old: reindex-parity + restore + re-erase at scale) -> REF-P24 (reindex at scale) + REF-P25 (restore + re-erase at scale).
- REF-P14 (old: cross-cell fan-out + E2E wedge) -> REF-P26 (cross-cell fan-out) + REF-P27 (E2E wedge).
- REF-P15 (old: dogfood + switch tests) -> REF-P28 (run-over-own-work + self-hosting CI graph) + REF-P29 (switch-test browser drive).

(Note: the structural erasure holder and the #sub ladder were adjacent half-deliverables across old REF-P8/REF-P9;
the finer pass groups the #sub ladder with the real erasure holder in REF-P15 — both are the "tombstone carries
the root over erasable structure" deliverable and ship one clean unit — and isolates the R2 cache (REF-P12) and
reindex-from-source (REF-P16) as their own units. No coverage lost: ladder, erase holder, R2 cache, reindex are
all present, now each independently gateable.)

**Floor -> follow-on pairing (name-your-floors, EI-01 §1 / master §5):**
- per-tenant DEK (REF-P4) -> the structural erasure surface (REF-P15) -> full-scale erasure (REF-P25).
- holder stub (REF-P3) -> the real erasure holder (REF-P15).
- R2-invalidator no-op shim (REF-P7) -> the live R2 cache (REF-P12).
- edge schema (REF-P5) -> the builder + invalidator that populate it (REF-P6, REF-P7).
- synthetic producers (REF-P8) / synthetic typed events (REF-P14) -> real producer edges + real mirrors
  (REF-P17, REF-P18, REF-P19, REF-P20, REF-P21).
- read-time CTE + pagination + replica for hot backlinks (REF-P11) -> the Leopard reach index R4 (REF-P23).
- the #sub grammar + the one ladder (REF-P15) -> each subsystem's stable #sub mint (REF-P17, REF-P18, REF-P19,
  REF-P20, REF-P21).
- CI-variant REF-D4 (REF-P16) -> full-scale REF-D4 (REF-P24); CI-variant REF-D5 (REF-P15) -> backup-scale
  REF-D5 (REF-P25).
- cell-local cross-cell resolution semantics (REF-P10) -> the cross-cell backlink fan-out build (REF-P26).
- Git pseudonymous-by-default commit authors (REF-P17, Git deliverable) -> the audited history-rewrite erasure
  path (10.6, R-M5 / on-demand — owned by Git, named here because it gates REF-D5).

**Drill coverage (every REF-D greened by some prompt's GATE/DRILLS):** REF-D1 (REF-P10 resolve half + REF-P11
backlink half + REF-P13 traverse half, re-confirmed REF-P17/P18/P21), REF-D2 (REF-P11, re-confirmed
REF-P17/P18/P21), REF-D3 (REF-P23), REF-D4 (REF-P14 TE-7 half + REF-P16 CI + REF-P17 Git + REF-P18 KN + REF-P24
full-scale), REF-D5 (REF-P15 CI + REF-P25 backup-scale + REF-P27 E2E-4), REF-D6 (REF-P11), REF-D7 (REF-P6 ingest
half + REF-P8 emit half), REF-D8 (REF-P13), REF-D9 (REF-P15 synthetic + REF-P17/P18 real Git/KN + REF-P19 CI +
REF-P20 Issues + REF-P21 Chat), REF-D10 (REF-P22); plus GA-D8/CP-D7/CP-D8 (REF-P26) + E2E-1/E2E-3/E2E-4
(REF-P27) and the self-hosting CI graph + truth-up (REF-P28) + the switch tests (REF-P29).

**Contract coverage (every owned/consumed row still covered):** 5.1 (REF-P1 + REF-P10 resolve wiring), 5.2
(REF-P10, cross-cell REF-P26, E2E REF-P27), 5.3 backlinks/edges (REF-P11) + traverse (REF-P13) + at-scale
(REF-P23) + cross-cell/E2E (REF-P26/P27), 5.4 (REF-P6 consumer + REF-P8 emit + real producers REF-P17/P18/P21),
5.5 (REF-P14 discipline + REF-P18 KN mirror + REF-P20 Issues mirror), 5.6 (consumed REF-P10/P12/P15/P17/P18/P20/
P21), 5.7 (REF-P1 grammar + REF-P15 ladder + per-producer REF-P17/P18/P19/P20/P21), 5.8 (REF-P16 + at-scale
REF-P24), 5.9 (REF-P17 grammar half + REF-P19 CI half), 10.1 (REF-P3 stub + REF-P15 real erase + REF-P12 R2
holder + REF-P25 backup-scale + REF-P27 E2E-4), 12.6 (REF-P26), 1.6 (REF-P2 lints, re-asserted every later
prompt), 1.8 (telemetry across REF-P6/P9/P10/P11/P12/P15/P16/P22/P23/P24), 1.11 (REF-P22), 11.3/11.4 (REF-P4
DEK + REF-P12 cache), 2.1/2.2/2.4/2.5 (REF-P6/P7/P8/P9), 2.6 (REF-P16 + per-producer replay REF-P17/P18), 2.9
(REF-P1 validator), 4.2/4.3/4.8/4.10 (REF-P10/P11/P15), 1.9/1.10 (REF-P10), 13.1 (REF-P8), 11.5/10.8 (REF-P25),
11.8 (REF-P19).
