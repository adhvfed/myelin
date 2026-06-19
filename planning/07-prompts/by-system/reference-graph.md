# Phase 7 — Prompt Ledger: Cross-Artifact Reference Graph (myelin-refs)

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
> Coverage: R-M0 → REF-P1; R-M1 → REF-P2; R-M2 → REF-P3..REF-P9 (seven slices, each its own green gate);
> R-M3 → REF-P10; R-M4 → REF-P11; R-M5 → REF-P12..REF-P14; R-M6 → REF-P15. Fifteen prompts, no milestone gap.

---

### REF-P1 — Ship the myelin-refs glue crate: ArtifactRef value type + the frozen #sub grammar + the four Refs lints

- **BAND.** M0.
- **ROADMAP MILESTONE.** R-M0 (planning/06-roadmaps/shared/reference-graph.md §2 "R-M0 — The ArtifactRef value
  type + the Refs ratchet").
- **DEPENDS-ON.** The M0 substrate prompts that lay down the Cargo workspace + the eight glue-crate skeletons
  and the lint framework + contract-coverage scanner (master §2 M0; substrate roadmap SUB-M0). The index slots
  this after those workspace-bootstrap prompts; REF-P1 ships the myelin-refs crate body into that skeleton.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md (always) §1 (the reference graph as connective tissue), §3 (name-your-floors,
    code-wins-over-docs); ../../external-insights/01-process-and-quality-doctrine.md §5 (the ratchet / committed
    gates — an uncommitted lint is no lint), §1 (name-your-floors).
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §3.1 (the URN ArtifactRef + the
    frozen Issues key grammar C-3), §3.5 (the unified #sub grammar, the complete v1 vocabulary, C-1/C-6), §4.8
    (display keys render-time only, REF-3).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md C-1 (#sub grammar
    frozen), C-3 (Issues <PROJECTKEY>-<seqno> stored canonical key), C-6 (check-/step- first-class #sub kinds),
    X-2 (the three content nodes byte-identical).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 5.1 (ArtifactRef parse/format,
    Issues key frozen); the four lint rows of 1.6 this prompt wires (tenant-predicate, no-raw-publish,
    no-cross-db, no-cross-sync-cycle); row 2.9 (the <subsystem>/<type> token table owned by Bus §6.2 — Refs is
    validator, not author).
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
  - Wire the four lints Refs leans on into CI, each with a red-fixture (proves it rejects) + a green-fixture
    (proves it admits), loud-never-swallowed (no "... || true"): tenant-predicate, no-raw-publish, no-cross-db,
    no-cross-sync-cycle. If the M0 substrate prompt already ships these lints centrally, this prompt instead adds
    the Refs-specific red+green fixtures and confirms they are wired; name in the commit which case applies.
  - FLOOR named: none — this is the contract crate + the ratchet, not the engine. State in the module doc that
    the value type is complete at M0 but the resolver over it is the R-M2 follow-on (REF-P3..REF-P9), so the
    value type is not mistaken for the working graph.
- **CONTRACTS TO IMPLEMENT.** 5.1 ArtifactRef parse/format (owned, the value-type half; the resolve half lands
  in REF-P5). Consumed-as-validator: 2.9 the token table (Refs validates, never authors). The four lints of 1.6
  (wired as gates). Implement to the frozen signatures — a needed shape change is a whole-workspace contract PR,
  escalated and written down, not a local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The myelin-refs crate compiles and is linked by the workspace; a change to ArtifactRef breaks every
    consumer's build now (the ADR-01 compile-time-carrier property) — CI.
  - parse/format round-trip + ambiguity-rejection: a fuzz corpus of malformed / short-hash / ambiguous /
    unknown-#sub-kind URNs is rejected (0 guessed scopes); every well-formed URN round-trips byte-identical
    (format(parse(s)) == canonical(s)) — CI.
  - The four lints (tenant-predicate, no-raw-publish, no-cross-db, no-cross-sync-cycle) green with both fixtures,
    wired into CI, loud, never "|| true" — CI (these are permanent ratchet gates; say so).
  - The contract-coverage scanner passes on the myelin-refs rows (5.1 has a provider+consumer CDC stub) — CI.
- **TESTS (required).** Unit tests for parse/format on every #sub kind + the Issues key + the rejection cases.
  A property/fuzz test for ambiguity-rejection (no input ever yields a guessed scope). The provider+consumer CDC
  stub for contract row 5.1. The red+green lint fixtures (each lint proven to reject and to admit). myelin-refs
  is a mandatory-core glue crate: state the cargo-mutants mutation-score floor for the parse module in this
  field and meet it.
- **DEFINITION OF DONE.** myelin-refs compiles in the workspace and is linked by consumers; parse/format + the
  frozen #sub grammar implement contract 5.1's frozen shape; the four lints emit dated green artifacts with both
  fixtures; the fuzz/property + unit + CDC tests pass; the contract-coverage scanner is green; the floor note
  (resolver is R-M2) is written in the module doc; the work is committed. No gate is greened by weakening a
  threshold.
- **COMMIT.** Header: P-<NNN> M0: myelin-refs glue crate — ArtifactRef + #sub grammar + Refs lints. Body lists:
  contract 5.1 (value-type half) implemented; the four lints greened with red+green fixtures; the parse
  mutation-score measured; the floor named (resolver follow-on REF-P3..REF-P9). Branch first if on default;
  do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### REF-P2 — Register Refs as a PersonalDataHolder + pin the per-tenant DEK + confirm residency

- **BAND.** M1.
- **ROADMAP MILESTONE.** R-M1 (planning/06-roadmaps/shared/reference-graph.md §2 "R-M1 — Refs as a holder + the
  edge-index encryption floor").
- **DEPENDS-ON.** REF-P1 (the myelin-refs crate exists). The M1 Identity/Storage/GDPR prompts that ship the
  holder harness auto-registration (contract 1.4), the KMS hierarchy (11.3/11.4), and the residency-pin
  (tenancy 12.x) — Refs registers into those. The index places this after those M1 substrate prompts.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe by construction; name-your-floors);
    ../../external-insights/04-hard-problems.md §1 (erasure vs immutability — the pseudonymous-id posture);
    ../../external-insights/01-process-and-quality-doctrine.md §1 (name-your-floors).
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §3 (tables are (tenant, region)
    first, RLS, per-tenant DEK, holder auto-registered), §3.6 (the projection cache as a holder), §4.6 tail (the
    small structural erasure surface; the residual instantiated by reference to 10.9).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-7 (the one
    free-text/immutable erasure posture — Refs adds no new residual), reference to OQ on residency-pin.
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 10.1
    (PersonalDataHolder{locate/export/rectify/restrict/erase}); 1.4 (harness holder auto-registration); 11.3/11.4
    (KMS hierarchy: per-cell root -> per-tenant KEK -> per-tenant DEK; per-subject DEK backstop, crypto-shred);
    10.9 (the one residual posture).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M1 + §1.1 row (10.1) + §1.2 rows 11.3/11.4.
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs implementation crate (the Refs
  service crate, not the glue crate):
  - Register Refs as a PersonalDataHolder via the harness auto-registration (1.4) so the H1..H18 holder list is
    exhaustive before any real tenant data exists (10.1). At M1 the holder is a STUB — no edge index exists yet
    to purge — but it is on the list, so the M5 DSAR fan-out cannot silently miss it. Implement the holder trait
    surface against the future edge index + R2 cache (locate/export/restrict/erase return empty-but-correct
    now). State in writing that Refs' erasure surface is small and structural: only pseudonymous opaque ids
    (origin_actor) + cache titles, never third-party free-text bodies — so Refs instantiates the one platform
    residual posture (10.9 / X-7) BY REFERENCE and adds NO new [OPEN — LEGAL] residual.
  - Pin the per-tenant DEK for the (future) edge index + R2 cache into the KMS hierarchy (11.3): per-cell root ->
    per-tenant KEK -> per-tenant DEK as the tenant-decommission crypto-shred unit; reserve the per-subject DEK
    (11.4) backstop for a name landing in a cached title. No index exists yet — this reserves the key class so
    R-M2's index is encrypted-from-birth, and confirms destroy is callable on the key class.
  - Confirm the residency-pin applies to the (future) per-tenant edge table + R2 cache: all Refs state is
    cell-local, (tenant, region)-partitioned, no cross-tenant query path. The residency-pin + tenant-predicate
    lints (M0) already enforce it structurally — assert the Refs crate links them.
  - FLOOR named: per-tenant DEK (the crypto-shred + backup-backstop unit) is the floor; the structural erasure
    surface (R2-cache PII purge + reliance on Id's pseudonym shred for origin_actor + *.erased tombstoning)
    is the follow-on, landing in REF-P9 once the index exists. Write this so the DEK is not mistaken for the
    whole erasure answer.
- **CONTRACTS TO IMPLEMENT.** 10.1 PersonalDataHolder (owned by Refs as a holder; stub surface now, real erase
  in REF-P9) — wired to the harness 1.4 auto-registration. Consumed: 11.3/11.4 (the KMS key class), the
  residency-pin (tenancy). To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - Refs appears in the harness-generated holder registry — 0 stores unregistered; the contract-coverage scanner
    confirms 10.1 coverage — CI (structural).
  - The per-tenant Refs DEK is a destroyable key in the KMS hierarchy: the key class exists and destroy is
    callable (proven fully later by REF-D5 in REF-P9/REF-P13; here the check is structural) — CI (structural).
  - Refs' M1 work does not begin its M2 engine over a red STOR-D1/STOR-D2 (restore-verify), ID-D3 (cross-tenant
    0), ID-D2 (fail-static), ID-D1 (disabled-user N=5 min), CP-D2/CP-D3 (misroute + residency-pin): name these
    inherited M1 platform gates as the precondition for REF-P3. Refs does not re-prove them; it cannot build the
    edge index over a red STOR-D1 — DEPENDS-ON makes this concrete.
- **TESTS (required).** Unit test: the holder stub surface returns empty-but-correct locate/export for a tenant
  with no edges. A structural test asserting Refs is in the holder registry and the per-tenant DEK class is
  destroyable. The provider+consumer CDC pair for the Refs side of 10.1. No drill greens here (the engine is
  R-M2) — record this surface as untested-at-runtime-but-named (the real erase is REF-P9), honestly.
- **DEFINITION OF DONE.** Refs is registered as an exhaustive-list holder (0 unregistered); the per-tenant DEK
  class is reserved + destroyable; residency-pin confirmed structurally; the floor (DEK now; structural erasure
  in REF-P9) is named in writing; the no-new-residual posture is recorded; the holder CDC pair passes; committed.
- **COMMIT.** Header: P-<NNN> M1: Refs PersonalDataHolder registration + per-tenant DEK + residency-pin. Body
  lists: 10.1 holder stub registered (exhaustive-list); the per-tenant DEK class reserved/destroyable; the
  floor named (structural erasure follow-on REF-P9); no new [OPEN — LEGAL] residual. Branch first; do not push
  unless asked. Co-Authored-By trailer.

---

### REF-P3 — The edge inverse-index schema + the two consumers (steady-state == cold-rebuild path)

- **BAND.** M2.
- **ROADMAP MILESTONE.** R-M2 slice 1 of 7 (planning/06-roadmaps/shared/reference-graph.md §2 "R-M2 — The Refs
  core", the "edge inverse-index schema + the two consumers" deliverable).
- **DEPENDS-ON.** REF-P1, REF-P2; M1 fully green (Identity 4.3/4.2/4.10/4.8, KMS 11.3/11.4, STOR-D1/D2,
  CP-D2/D3); M0 outbox + consumer template (2.2/2.3/2.4/2.5) + the failure-injection harness. The index resolves
  these to their P-NNN (Identity M1, Storage M1, Bus M0).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1; ../../external-insights/02-platform-substrate.md §7 (backlinks are event-sourced
    projections; reindex-from-source); ../../external-insights/04-hard-problems.md §5.3 (reindex-from-source the
    resilience primitive); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it; observability
    is part of the pass).
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §3.2 (the edge table + the exact
    columns + the three indexes), §4.1 (extraction -> emit; deterministic edge_id idempotent rebuild), §4.3 (the
    two consumers refs-edge-builder + refs-projection-invalidator; steady-state == cold-rebuild), §3.7 (the
    stateful-component register R1/R3).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 5.4 (refs.edge.created/.removed
    emitted by producers via outbox; no standalone edge-write API), 2.1 (EventEnvelope), 2.4/2.5 (EventHandler
    template + consumer_dedup), 5.5 (the typed-lifecycle subjects the builder also whitelists).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M2 (the edge schema + consumers bullets) +
    §1.1 rows 5.4.
  - Drill source: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md REF-D7 (line ~352), REF-D4 (~349).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate:
  - The edge table migration (forward-only, 1.5): (tenant, region) first columns / partition prefix, RLS,
    per-tenant DEK; columns edge_id (deterministic hash(tenant, source, target, rel)), source, source_root,
    target, target_root, rel (edge_rel), rel_class (reference | lifecycle), origin_event, origin_actor
    (pseudonymous Principal ref), created_at, zookie, tombstoned; PRIMARY KEY (tenant, edge_id), UNIQUE (tenant,
    source, target, rel); the three indexes edge_inbound (tenant, target_root) WHERE NOT tombstoned, edge_outbound
    (tenant, source_root), edge_by_rel (tenant, target_root, rel) WHERE rel_class='lifecycle'. Exactly the §3.2
    shape.
  - The refs-edge-builder consumer (an ordinary EventHandler, 2.4): subjects() whitelists refs.edge.> plus the
    typed-lifecycle subjects issue.relation.> and knowledge.page.> (NEVER "*"; one of the reviewed firehose-class
    infra consumers, BUS-4); upsert on created (ON CONFLICT DO NOTHING/UPDATE — idempotent via the deterministic
    edge_id), delete/soft-delete on removed, tombstone on *.erased; ack-after-apply; idempotent on event_id via
    consumer_dedup (2.5). It writes source_root/target_root by strip_sub (REF-P1).
  - The refs-projection-invalidator consumer: busts R2 (REF-P8 builds R2; here register the consumer + the
    invalidation interface; if R2 lands in REF-P8, this prompt ships the consumer that R2 plugs into and a no-op
    cache shim, named as a floor).
  - Steady-state ingestion and cold rebuild MUST be the same code path (so they cannot drift, REF-D4) — there is
    NO "load the edge table from an owner's DB" backdoor (no-cross-db lint).
  - FLOOR named: the projection cache invalidator targets a no-op shim until REF-P8 ships R2; named so the
    invalidation is not mistaken for live.
- **CONTRACTS TO IMPLEMENT.** 5.4 refs.edge.created/.removed (consumed via the builder; emitted by producers —
  NO standalone edge-write API). Consumed: 2.1 EventEnvelope, 2.4/2.5 the consumer template + dedup ledger. To
  the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - REF-D7 (F5, edge loss / no-ghost): crash a producer between the content/relation commit and the relay
    publish -> the edge event is still delivered (outbox), never an edge without its content. 0 ghost, 0 lost.
    Green artifact: outbox emit-iff-committed telemetry — CI.
  - Idempotent rebuild: replaying refs.edge.created twice upserts one row (deterministic edge_id); the dedup
    ledger drops the duplicate — CI.
  - The no-cross-db + no-raw-publish + tenant-predicate lints green on this crate (no owner-DB read, no edge
    escapes the outbox, every edge query carries the tenant predicate) — CI (permanent ratchet).
  - Telemetry index_lag emitted by the builder (1.8) — no signal = failed drill — CI.
- **TESTS (required).** Unit tests for upsert/delete/tombstone idempotency on the deterministic edge_id;
  source_root/target_root derivation. A chained-mutation test: emit created -> removed -> created again across a
  simulated consumer restart, asserting exactly-once-in-effect. The drill-harness scenario for REF-D7 (the
  producer-crash injection). The provider+consumer CDC pair for 5.4. Mutation-score floor for the edge-builder
  module (mandatory-core) stated and met.
- **DEFINITION OF DONE.** The edge table + the two consumers exist and compile; steady-state == cold-rebuild is
  one code path with no owner-DB backdoor; REF-D7 emits its dated green artifact (0 ghost, 0 lost); the
  idempotency + lint + telemetry gates are green; the R2-invalidator floor is named (REF-P8); unit + chained +
  CDC tests pass; the mutation floor is met; committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: Refs edge inverse-index + the two consumers. Body lists: 5.4 wired (consumer
  side); REF-D7 greened (0 ghost / 0 lost, outbox emit-iff-committed); idempotent-rebuild proven; the
  R2-invalidator floor named (REF-P8); the edge-builder mutation score measured. Branch first; do not push
  unless asked. Co-Authored-By trailer.

---

### REF-P4 — The edge-extraction emit seam + the loop-guard causal-depth stamp

- **BAND.** M2.
- **ROADMAP MILESTONE.** R-M2 slice 2 of 7 (the "edge extraction -> emit (the producer seam)" deliverable).
- **DEPENDS-ON.** REF-P3 (the edge index + builder exist to ingest what this seam emits). The M2 myelin-content
  freeze (13.1, X-2 — the three inline ref nodes) the index resolves to its P-NNN. M0 outbox 2.2.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (Chat references any artifact); ../../external-insights/04-hard-problems.md §2.4
    (structured-node extraction, not regex over prose — the reliability guarantee);
    ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §4.1 (the two producers; the
    same-transaction emit; OutboxTx::emit(draft, cause = Some(content_event)); causality depth +1 — the loop
    guard reads it, AG-6).
  - Reconciliation: 00-reconciliation-decisions.md X-2 (the three nodes byte-identical across Chat/Issues/KN).
  - Contracts: contract-index.md rows 5.4 (the edge events), 13.1 (the myelin-content three structured inline
    nodes mention/artifact_ref/embed — the producers), 2.2 (OutboxTx::emit(draft, cause)).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M2 (the edge-extraction emit seam bullet) +
    §1.2 row 13.1.
  - Drill source: REF-D7 (the seam's no-ghost half is drilled in REF-P3; here the loop-guard depth stamp).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs (or a shared extraction)
  crate:
  - The edge-extraction emit seam: given a myelin-content document containing the three structured inline nodes
    (mention(Principal), artifact_ref(ArtifactRef), embed(ArtifactRef) — 13.1, byte-identical X-2), emit one
    refs.edge.created per structured ref node (rel in {mentions, links, embeds}, rel_class='reference') in the
    SAME transaction that writes content, via OutboxTx::emit(draft, cause = Some(content_event)) so the
    correlation root carries and causation = the content event with depth +1. Extraction is structured-node-
    driven, NOT a regex over prose.
  - The loop-guard causal-depth stamp: the depth +1 on every refs.edge.* so the AG-6 loop guard treats only a
    structured artifact_ref node as a re-trigger source. Build + drill the stamp now.
  - At M2 the producers are exercised with a synthetic/test content writer (the first real ones land in
    REF-P10/REF-P11); the seam + the loop-guard depth stamp are built and proven here. FLOOR named: producers
    are synthetic until R-M3/R-M4 — write this so the seam is not mistaken for live producer edges.
- **CONTRACTS TO IMPLEMENT.** 5.4 (the emit half of the producer seam — emitted via outbox, never a standalone
  write API). Consumed: 13.1 (the three content nodes), 2.2 (OutboxTx::emit(draft, cause)). To the frozen shape.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - One refs.edge.created per structured node, in the same transaction as the content write — proven by the
    test content writer: N nodes -> N edges, emit-iff-committed (the REF-D7 floor extended to the producer side)
    — CI.
  - The causal-depth stamp: every emitted refs.edge.* carries depth = content_event.depth + 1; the loop guard
    treats an artifact_ref node as a re-trigger source and the depth-ceiling tripwire fires before runaway — CI
    (asserts the causal-depth telemetry signal 1.8).
  - The no-raw-publish lint green (no edge escapes the outbox; there is no standalone edge-write API) — CI.
- **TESTS (required).** Unit tests: extraction yields exactly one edge per node kind, correct rel/rel_class; a
  document with no ref nodes yields zero edges. A chained test: write content -> assert edge emitted iff the
  content tx commits (abort -> no edge). The loop-guard depth-stamp test (depth increments; ceiling tripwire).
  The CDC pair for the emit side of 5.4. Mutation floor on the extraction module stated and met.
- **DEFINITION OF DONE.** The emit seam + the loop-guard depth stamp exist and compile; one edge per structured
  node emit-iff-committed; the depth stamp drives the loop guard; the no-raw-publish lint is green; the
  synthetic-producer floor is named (real producers REF-P10/REF-P11); unit + chained + CDC tests pass; the
  mutation floor is met; committed.
- **COMMIT.** Header: P-<NNN> M2: Refs edge-extraction emit seam + loop-guard depth stamp. Body lists: 5.4 emit
  side wired; emit-iff-committed proven (N nodes -> N edges); the causal-depth stamp greened; the synthetic-
  producer floor named (REF-P10/REF-P11). Branch first; do not push unless asked. Co-Authored-By trailer.

---

### REF-P5 — The per-viewer resolution service: the chokepoint (denied -> tombstone, never leak)

- **BAND.** M2.
- **ROADMAP MILESTONE.** R-M2 slice 3 of 7 (the "per-viewer resolution service — the chokepoint" deliverable;
  contract 5.2).
- **DEPENDS-ON.** REF-P1 (ArtifactRef + parse), REF-P3 (the edge index — not strictly read here but the crate
  context). M1 Identity check (4.2) + zookie (4.10), the resilient client (1.9) + fail-static (1.10), each
  subsystem's project(ref, viewer) shape (5.6). The index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1; ../../external-insights/02-platform-substrate.md §7;
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it; observability).
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §4.2 (resolution: parse ->
    Id.check -> denied returns a tombstone never a leak -> projection via R2 cache hit, else owner's project
    through the resilient client, Refs never reads the owner DB -> subscribe to *.updated/*.erased;
    per-viewer correctness without per-viewer caching), §3.6 (R2 cache), and the cross-cell pinning C-5 (frozen
    semantics; the fan-out build stays a floor).
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
    via the R2 cache hit (REF-P8), else the owner's project(ref, viewer) through the ResilientClient (1.9) —
    Refs NEVER reads the owner's DB (no-cross-db lint); (4) the caller subscribes to *.updated/*.erased so the
    rendered ref stays live.
  - Per-viewer correctness WITHOUT per-viewer caching: the per-viewer check (step 2) gates a viewer-independent,
    ref-keyed cache (step 3) — shared without leaking because no content returns until the check passes. Document
    this explicitly.
  - Fail-static (1.10) under an Id hiccup: resolve degrades on the coarse cache rather than cascading; a
    zookie-stamped read bypasses fail-static (the new-enemy defense is exercised fully in REF-P6).
  - Cross-cell resolution is pinned cell-local (C-5): a cross-cell target resolves in the home cell; only the
    already-filtered projection or a tombstone crosses, over the frozen CrossCellPointer. FLOOR named: the
    cross-cell fan-out BUILD is R-M5 (REF-P14); the resolution SEMANTICS are frozen here.
- **CONTRACTS TO IMPLEMENT.** 5.2 resolve (owned). Consumed: 4.2 check, 5.6 project, 1.9 ResilientClient, 1.10
  FailStatic. To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - REF-D1 (F1, the resolve half): a confidential artifact resolves to a Tombstone{denied} for an unauthorized
    viewer — the title/state/icon are NEVER in the tombstone; 0 leak. Green artifact: the zero-escape counter at
    0 — CI. (The backlink/traverse half of REF-D1 is drilled in REF-P6/REF-P7.)
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
  half, 0 leak); fail-static degradation proven; the cross-cell-fan-out floor named (REF-P14); the no-cross-db
  lint + telemetry green; unit + chained + CDC tests pass; the mutation floor is met; committed. No threshold
  weakened.
- **COMMIT.** Header: P-<NNN> M2: Refs per-viewer resolution chokepoint (denied -> tombstone). Body lists: 5.2
  implemented; REF-D1 resolve-half greened (0 leak); fail-static degradation proven; the cross-cell fan-out
  floor named (REF-P14); resolve mutation score measured. Branch first; do not push unless asked.
  Co-Authored-By trailer.

---

### REF-P6 — The permission-filtered backlink read: lower the SetExpr ACL filter over source_root (the crux)

- **BAND.** M2.
- **ROADMAP MILESTONE.** R-M2 slice 4 of 7 (the "permission-filtered backlink read — the crux" deliverable;
  contract 5.3 backlinks/edges).
- **DEPENDS-ON.** REF-P3 (the edge index), REF-P5 (resolution context). M1 Identity 4.3 (list_objects with the
  frozen SetExpr push-down — THE crux dependency) + 4.10 (zookie + the authz reverse-index revision watermark).
  The index resolves these to Identity's M1 P-NNN.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1; ../../external-insights/02-platform-substrate.md §7 (permission-filtered set reads;
    Leopard/Zanzibar); ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §4.4 (the leak-free backlink read;
    the FROZEN SetExpr lowering over source_root — the three forms Ids/NotIds -> IN/NOT IN, InRelation/TupleSet
    -> JOIN authz_visible, Union/Intersect/Difference -> AND/OR/EXCEPT, All -> no predicate, None -> WHERE false;
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
    budget in R-M5 (REF-P12). Write this so "we page them, we don't materialise them" is not mistaken for the
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
  artifact (0 leak, 0 cross-tenant, no stale allow); the hot-artifact R4 floor is named (REF-P12); the
  query-count + filter-mode-split telemetry fire; unit + chained + CDC tests pass; the mutation floor is met;
  committed. No threshold weakened, no assertion inverted.
- **COMMIT.** Header: P-<NNN> M2: Refs permission-filtered backlink read (SetExpr over source_root). Body
  lists: 5.3 backlinks/edges implemented; the frozen SetExpr lowering (all forms); REF-D1 (backlink)/REF-D2/
  REF-D6 greened (0 leak / 0 cross-tenant / no stale allow); one-query-no-N+1 proven; the R4 hot-fanout floor
  named (REF-P12); SetExpr-lowering mutation score measured. Branch first; do not push unless asked.
  Co-Authored-By trailer.

---

### REF-P7 — The bounded cycle-safe recursive-CTE traverse + the TE-7 typed-edge mirror discipline

- **BAND.** M2.
- **ROADMAP MILESTONE.** R-M2 slice 5 of 7 (the "recursive-CTE traversal" + "the TE-7 typed-edge mirror
  discipline" deliverables; contracts 5.3 traverse + 5.5).
- **DEPENDS-ON.** REF-P3 (the edge adjacency list), REF-P6 (the list_objects filter, reused as the
  collected-node post-filter). Identity 4.3 (the post-filter). The index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1; ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §4.5 (the bounded cycle-safe
    recursive-CTE traverse: WITH RECURSIVE over edge filtered by rel/rel_class, visited-set cycle guard
    (path-array / SQL:2023 CYCLE), depth ceiling default 16, statement timeout, ONE list_objects post-filter
    over the collected node set not per-hop, prune the branch on an unreadable hop, partial result + truncated
    marker, cycle -> diagnostic not hang), §3.3 (the TE-7 hybrid: lifecycle edges dual-homed, the typed table is
    truth, Refs fixes the rel vocabulary + the inverse pairing + the rel_class='lifecycle' mirror discipline),
    §3.4 (the adjacency structure).
  - Contracts: contract-index.md rows 5.3 (traverse), 5.5 (TE-7 typed-edge mirror: the lifecycle relation set
    closes/blocks/blocked_by/depends_on/parent/assigns/relates, the inverse pairing, the typed table wins on
    drift).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M2 (the traverse + TE-7 mirror bullets) + §1.1
    rows 5.3, 5.5.
  - Drill source: REF-D8 (cycle / unbounded walk, ~353), REF-D1 (the traverse leak half, ~346).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate:
  - traverse(root, rels, depth, viewer) (5.3): a WITH RECURSIVE walk over the edge adjacency list filtered by
    rel/rel_class, with a visited-set cycle guard (path-array or SQL:2023 CYCLE), a depth ceiling (default 16,
    read from the thresholds file), a statement timeout, and ONE list_objects post-filter over the COLLECTED
    node set (not per-hop) where a hop into an unreadable artifact PRUNES that branch (the traversal is not a
    side-channel). A request exceeding the budget returns a PARTIAL result + a "truncated" marker, never an
    unbounded scan; a dependency cycle is surfaced as a DIAGNOSTIC, not a hang.
  - The TE-7 typed-edge mirror discipline (5.5): fix the rel vocabulary (closes/blocks/blocked_by/depends_on/
    parent/assigns/relates), the rel_class='lifecycle' mirror discipline, and the inverse pairing
    (blocks<->blocked_by, parent<->child). Consume the typed lifecycle events (already whitelisted by the
    builder in REF-P3) and project lifecycle-class edges so cross-subsystem traversal is one Refs query. At M2
    the discipline + the consumer are built; the typed TABLES are owned by Issues/KN and arrive in R-M3/R-M4 —
    so the lifecycle producers are exercised here with SYNTHETIC typed events. FLOOR named: real typed mirrors
    land in R-M3 (KN page_parent, REF-P10) and R-M4 (Issues issue_relation, REF-P11).
- **CONTRACTS TO IMPLEMENT.** 5.3 traverse (owned), 5.5 TE-7 mirror discipline (owned — the vocabulary + inverse
  pairing; the tables are the subsystems'). Consumed: 4.3 list_objects (the post-filter). To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - REF-D8 (traversal bound): a cycle + a 1000-deep chain -> the CTE terminates (visited-set + depth ceiling
    16), the cycle is surfaced as a diagnostic (not a hang), the statement timeout is respected. Green artifact:
    depth-bound honoured — CI.
  - REF-D1 (traverse leak half): a hop into an unreadable artifact prunes the branch; traverse never reveals an
    edge into an artifact the viewer cannot read. 0 leak — CI.
  - The inverse-pairing correctness: a synthetic blocks event yields both blocks and blocked_by lifecycle edges
    with the correct direction — CI.
- **TESTS (required).** Unit tests: the cycle guard terminates a self-referential graph; the depth ceiling
  truncates at 16 with the marker; the post-filter prunes (not per-hop). A chained test: synthetic
  lifecycle events -> traverse the epic tree -> correct inverse pairing across hops. The drill scenarios for
  REF-D8 and the REF-D1 traverse half. The CDC pair for 5.3 (traverse) and 5.5. Mutation floor on the
  traverse + mirror module stated and met.
- **DEFINITION OF DONE.** traverse + the TE-7 mirror discipline exist and compile; REF-D8 (bounded, cycle ->
  diagnostic) and the REF-D1 traverse half (0 leak, branch-prune) emit dated green artifacts; the inverse
  pairing is correct; the synthetic-typed-events floor is named (real mirrors REF-P10/REF-P11); unit + chained +
  CDC tests pass; the mutation floor is met; committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: Refs recursive-CTE traverse + TE-7 mirror discipline. Body lists: 5.3
  traverse + 5.5 mirror discipline implemented; REF-D8 greened (depth 16, cycle -> diagnostic); REF-D1 traverse
  half greened (0 leak, branch-prune); inverse pairing proven; the synthetic-typed-events floor named
  (REF-P10/REF-P11); traverse mutation score measured. Branch first; do not push unless asked. Co-Authored-By
  trailer.

---

### REF-P8 — The unified 4-step #sub tombstone ladder + the R2 projection cache

- **BAND.** M2.
- **ROADMAP MILESTONE.** R-M2 slice 6 of 7 (the "unified #sub resolution ladder" + "the projection cache (R2)"
  deliverables; contracts 5.7 + 5.6 sub_anchor + the R2 holder).
- **DEPENDS-ON.** REF-P1 (the frozen #sub grammar), REF-P5 (resolve — the ladder is its #sub extension), REF-P3
  (the *.updated/*.erased consumer the invalidator plugs into). Each subsystem's project sub_anchor shape (5.6).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1; ../../external-insights/04-hard-problems.md §1 (erasure vs immutability — the tombstone
    carries the root); ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §4.6 (the one resolution ladder,
    frozen: 1 permission -> Tombstone{denied}; 2 root resolve -> Tombstone{root_gone}; 3 sub resolve via the
    owner's project sub_anchor -> LIVE->Projection / MOVED->Projection+moved / OUTDATED->Projection(partial)+
    outdated / GONE->Tombstone{sub_gone, root}; 4 ERASED -> Tombstone{erased}; a tombstone ALWAYS carries the
    root), §3.5 (Git line-ranges content-anchored: exact->LIVE, rebased->MOVED, partial->OUTDATED,
    content_gone->GONE via BLAKE3 + 3-way context match), §3.6 (the R2 projection cache as a bounded
    invalidatable holder, never truth).
  - Reconciliation: 00-reconciliation-decisions.md C-2 (the 4-step ladder frozen), C-1/C-6 (the grammar + the
    check-/step- kinds).
  - Contracts: contract-index.md rows 5.7 (the unified #sub scheme + the 4-step ladder), 5.6 (project's
    sub_anchor resolver returns the frozen live/moved/outdated/gone state), 5.2 (resolve, extended for #sub).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M2 (the #sub ladder + the R2 cache bullets) +
    §1.1 rows 5.7, (1.8).
  - Drill source: REF-D9 (sub-tombstone, the unified ladder, ~354).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate:
  - The unified 4-step #sub resolution ladder (5.7), extending resolve (REF-P5) for any #sub: (1) permission ->
    Deny ⇒ Tombstone{denied} (never leak); (2) root resolve -> No ⇒ Tombstone{root_gone}; (3) sub resolve via
    the owner's project sub_anchor resolver -> LIVE -> Projection / MOVED -> Projection + flag moved / OUTDATED
    -> Projection(partial) + flag outdated / GONE -> Tombstone{sub_gone, root}; (4) ERASED -> Tombstone{erased}.
    A tombstone ALWAYS carries the root (an embed degrades to "this referenced <parent> (the specific part is no
    longer available)" rather than vanishing). The same live/moved/outdated/gone shape covers Git line-ranges,
    KN block/heading/row anchors, Chat message/thread anchors, and the check-/step- CI kinds (C-6) — one ladder.
  - The R2 projection cache (3.6): bounded, invalidatable, event-busted per ArtifactRef (title/state/icon/render
    hint), keyed (tenant, ref), TTL + *.updated/*.erased invalidation (the refs-projection-invalidator from
    REF-P3 now drives a real cache, replacing the REF-P3 no-op shim). A PersonalDataHolder (may hold a name in a
    title), NEVER a source of truth; on miss/erasure it re-resolves. Residency-pinned, crypto-shred-able
    (Valkey-class), under the per-tenant DEK reserved in REF-P2.
  - Wire the telemetry (1.8): tombstone_count (+ the ladder-state distribution), resolve_cache_hit_ratio.
  - FLOOR named: each subsystem's STABLE #sub mint (a block id survives moves, a message/comment id is
    immutable, a Git range carries the BLAKE3 fingerprint) is the subsystem's deliverable, asserted by REF-D9 on
    real producers in R-M3/R-M4 (REF-P10/REF-P11). At M2 the ladder is exercised against synthetic + the
    available producers. Write this so the frozen grammar is not mistaken for a working sub-anchor everywhere.
- **CONTRACTS TO IMPLEMENT.** 5.7 the unified #sub scheme + the 4-step ladder (owned — grammar + ladder; the
  stable mint is each subsystem's). 10.1 R2 as a holder (the cache half). Consumed: 5.6 project's sub_anchor.
  To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - REF-D9 (sub-tombstone, the unified ladder): delete a doc block / PR comment / chat message / make a Git
    line-range outdated that others embed -> each degrades through the frozen live/moved/outdated/gone ladder to
    the correct state (moved / outdated / sub_gone) with the ROOT carried — 0 dangling embed, 0 hard 404, no
    leak. At M2 exercised against the available producers + synthetic ones; re-run on each real producer in
    R-M3/R-M4. Green artifact: the tombstone-ladder state distribution telemetry — CI.
  - R2 invalidation: a *.updated busts the cached entry; a *.erased tombstones it; a miss re-resolves — CI.
  - resolve_cache_hit_ratio + tombstone_count telemetry emitted (1.8) — CI.
- **TESTS (required).** Unit tests for every ladder branch (denied/root_gone/each sub-state/erased) and that the
  root is always carried. Unit tests for the Git content-anchored states (exact/rebased/partial/content_gone)
  against synthetic blob fingerprints. A chained test: cache hit -> *.updated -> miss -> re-resolve. The drill
  scenario for REF-D9 across the three content shapes (synthetic). The CDC pair for 5.7 + the R2 holder side of
  10.1. Mutation floor on the ladder module stated and met.
- **DEFINITION OF DONE.** The 4-step ladder + the R2 cache exist and compile; REF-D9 emits a dated green
  artifact (correct state, root carried, 0 dangling/404/leak) across synthetic + available producers; R2
  invalidation is proven; the per-subsystem-stable-mint floor is named (REF-P10/REF-P11); the R2-invalidator
  shim from REF-P3 is replaced by the live cache; unit + chained + CDC tests pass; the mutation floor is met;
  committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: Refs unified #sub tombstone ladder + R2 projection cache. Body lists: 5.7 the
  ladder implemented; R2 holder live (replaces the REF-P3 shim); REF-D9 greened (root carried, 0 dangling/404/
  leak); the per-subsystem stable-mint floor named (REF-P10/REF-P11); ladder mutation score measured. Branch
  first; do not push unless asked. Co-Authored-By trailer.

---

### REF-P9 — The structural erasure holder + reindex-from-source (the recovery + erasure path)

- **BAND.** M2.
- **ROADMAP MILESTONE.** R-M2 slice 7 of 7 (the "erasure as a real (small) holder" + "reindex-from-source"
  deliverables; contracts 10.1 real erase + 5.8).
- **DEPENDS-ON.** REF-P2 (the holder stub + the per-tenant DEK), REF-P3 (the builder — the one ingest path
  reindex re-drives), REF-P8 (the R2 cache to purge). Identity pseudonym shred (4.8); the Bus reindex re-emit
  (2.6) + each owner's replay. The index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe); ../../external-insights/04-hard-problems.md §1 (erasure over immutable
    structure — pseudonym + tombstone), §5.3 (reindex-from-source the only recovery path);
    ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §4.6 tail (Refs as a
    PersonalDataHolder: locate -> edges/cache naming the subject; erase -> purge R2 PII + rely on Id's pseudonym
    shred for origin_actor + tombstone content-erased targets via *.erased; restrict suppression; no erasure
    backdoor — driven by *.erased through the same live consumer), §4.7 (reindex-from-source: events::reindex
    (scope) -> each owner's replay(scope, since) emits *.snapshot sub-artifact-granular -> refs-edge-builder
    ingests idempotently -> the rebuilt edge index byte-matches live; one code path; on a TE-7 drift a scoped
    reindex reconverges to the typed table which wins; never reads an owner DB).
  - Reconciliation: 00-reconciliation-decisions.md X-7 (the one residual posture, by reference).
  - Contracts: contract-index.md rows 10.1 (the holder erase surface), 5.8 (reindex(scope), never reads owner
    DBs), 2.6 (the reindex re-emit + *.snapshot/*.erased sub-artifact-granular), 4.8 (resolve_pseudonym/erase).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M2 (the erasure holder + reindex-from-source
    bullets) + §1.1 rows (10.1), 5.8 + §1.2 row 2.6.
  - Drill source: REF-D5 (erasure CI variant, ~350), REF-D4 (reindex parity CI variant, ~349).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate:
  - The real erasure holder (10.1), replacing the REF-P2 stub: locate(subject) -> the edges/cache entries naming
    the subject; erase(subject) -> purge R2 cache PII + rely on Id's pseudonym shred (4.8) for origin_actor (the
    edge keeps the opaque id; the human becomes unresolvable) + tombstone content-erased targets via the
    *.erased consumer (REF-P3). NO erasure backdoor — driven by *.erased through the same live consumer path.
    restrict(subject) suppression keeps a restricted subject's references out of indexing/agent-use/analytics.
    Refs holds only pseudonymous opaque ids + cache titles, never third-party free-text bodies (the residual is
    the one platform posture instantiated by reference, X-7).
  - reindex(scope) (5.8): call the Bus re-emit protocol (2.6) -> each owner's replay(scope, since) emits
    *.snapshot (content nodes + typed relations, sub-artifact-granular) -> refs-edge-builder ingests idempotently
    -> the rebuilt edge index byte-matches the live index. ONE code path for steady-state and recovery; NO
    "load the edge table from an owner's DB" backdoor (no-cross-db). On a Refs<->typed-table TE-7 drift, a scoped
    reindex reconverges Refs to the typed table (which always wins).
  - Wire the reindex_parity telemetry (1.8).
  - FLOOR named: the CI-variant drills (small corpus) gate this band; the full-scale REF-D4 (reindex parity at
    scale) + REF-D5 (erasure at backup scale, folded into E2E-4) are R-M5 (REF-P12/REF-P13). Write this so the
    CI variant is not mistaken for the backup-level proof.
- **CONTRACTS TO IMPLEMENT.** 10.1 the real erase surface (owned), 5.8 reindex (owned). Consumed: 2.6 the
  re-emit + replay, 4.8 pseudonym shred. To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - REF-D5 (erasure, CI variant): erase a subject + a referenced artifact -> references tombstone, the person is
    unresolvable, 0 recoverable PII in edge/cache, no 500 on resolve. Green artifact: erase-receipt + 0
    resolve-error — CI variant (full backup-level proof joins E2E-4 in R-M5).
  - REF-D4 (reindex-from-cold parity, CI variant): wipe the edge index, reindex(scope) -> the rebuilt index
    byte-matches live; a synthetic TE-7 drift reconverges to the typed table (typed wins). Green artifact: the
    reindex-parity hash — CI variant (full-scale REF-D4 is SCHED at R-M5).
  - The no-cross-db lint green (reindex re-drives the builder, never reads an owner DB) — CI.
- **TESTS (required).** Unit tests: locate/erase/restrict on a subject; the *.erased path tombstones; no
  backdoor. A chained test: build edges -> wipe -> reindex -> assert byte-parity; then erase -> re-resolve ->
  tombstone, person unresolvable. The drill scenarios for the REF-D5 + REF-D4 CI variants. The CDC pair for 10.1
  (erase) and 5.8. Mutation floor on the reindex + erase module stated and met.
- **DEFINITION OF DONE.** The real erase holder + reindex(scope) exist and compile; REF-D5 (CI) and REF-D4 (CI)
  emit dated green artifacts (0 recoverable PII / byte-parity / typed-wins); the one-code-path no-backdoor
  property holds (no-cross-db green); the full-scale-drill floor is named (REF-P12/REF-P13); the REF-P2 holder
  stub is replaced by the real surface; unit + chained + CDC tests pass; the mutation floor is met; committed.
  This slice completes R-M2; the master M2 exit gate cites REF-D1/REF-D2/REF-D8 (greened across REF-P5/P6/P7) —
  M3 does not start over a red REF-D1. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: Refs structural erasure holder + reindex-from-source. Body lists: 10.1 real
  erase + 5.8 reindex implemented; REF-D5 (CI) + REF-D4 (CI) greened (0 recoverable PII, byte-parity,
  typed-wins); the full-scale-drill floor named (REF-P12/REF-P13); the REF-P2 stub replaced; reindex mutation
  score measured. Branch first; do not push unless asked. Co-Authored-By trailer.

---

### REF-P10 — Producer edges light up: Git links + Knowledge embeds + the first lifecycle mirror

- **BAND.** M3.
- **ROADMAP MILESTONE.** R-M3 (planning/06-roadmaps/shared/reference-graph.md §2 "R-M3 — Producer edges light
  up").
- **DEPENDS-ON.** REF-P3..REF-P9 (the Refs core green — REF-D1/D2/D7/D8/D9/D4-CI/D5-CI). The M3 Git + Knowledge
  producer prompts that ship the three content nodes, project(ref, viewer), the sub_anchor resolvers, per-blob/
  ref and page-subtree replay, pseudonymous commit authors, and KN's page_parent typed events. The index
  resolves these to the Git/Knowledge M3 P-NNN. (AG-D4 green is a band precondition, not a Refs dependency —
  Refs runs no untrusted code.)
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1, §3 (name-your-floors); ../../external-insights/04-hard-problems.md §1 (erasure vs
    immutability — pseudonymous commit authors); ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §3.5 (Git line-ranges
    content-anchored), §4.6 (the ladder on real sub-anchors), §3.3 (the TE-7 mirror — KN page_parent the first
    real mirror), §4.7 (sub-artifact-granular replay).
  - Contracts: contract-index.md rows 5.4 (the producer edges), 5.6 (Git/KN project + sub_anchor), 5.7 (the #sub
    kinds on real sub-anchors), 5.5 (KN page_parent typed mirror), 2.6 (sub-artifact-granular replay), 5.9 (the
    Git<->CI CheckStatus seam — Refs' grammar half: check-/step- kinds now used; Git ships the consumer/
    projection awaiting CI's producer in R-M4).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M3 (all bullets) + §1.2 rows 5.5 (KN), 5.6,
    5.9.
  - Drill source: REF-D1/REF-D2 (re-confirm on real corpora, ~346/347), REF-D9 (real sub-anchors, ~354), REF-D4
    (Git+KN corpus, ~349); GIT-D7 (the Refs half of force-push anchor resolution; in the master M3 exit gate).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate (the engine is
  UNCHANGED; this prompt wires Refs to the first REAL producers and re-confirms the invariants on real corpora):
  - Consume Git-produced reference edges: commit-trailer / PR-link / "Closes <issue>" references emit
    refs.edge.created via the three content nodes; resolve the Git sub_anchor resolver for content-anchored line
    ranges (#L<a>-L<b> -> exact/rebased/partial/tombstone via BLAKE3 + 3-way context match) and PR review-thread
    comment-/thread- anchors through the REF-P8 ladder. The Git ReBAC fragment (4.9) flows through list_objects
    so the PR/repo backlink lists are leak-free (the GIT-D11 SetExpr JOIN, reusing REF-P6).
  - Consume Knowledge-produced edges: KN block/heading/row embeds emit refs.edge.created; resolve KN's sub_anchor
    resolver for b<id>/h<id>/row-/field- anchors (stable -> LIVE; edited -> OUTDATED; deleted -> GONE). Project
    KN's page_parent typed-lifecycle events (the FIRST real lifecycle mirror) as lifecycle-class edges with the
    REF-P7 inverse pairing.
  - Drive sub-artifact-granular replay (2.6) for Git (per-blob/ref) and KN (page-subtree at block granularity)
    so a scoped reindex re-emits the right grain and the content-anchored line-range / block anchors re-derive
    (never a stale raw line number / positional index).
  - Use (do not build) the check-/step- #sub kinds (frozen in REF-P1): Git's check_status projection +
    details_ref (#step-<n>) resolve through the same Refs ladder; CI's producer half lands in R-M4 (REF-P11).
  - FLOOR named: in-cell single-home-cell graph build (cross-cell fan-out is R-M5, REF-P14); Git
    pseudonymous-by-default commit authors as origin_actor (the audited history-rewrite erasure path 10.6 is
    R-M5/on-demand). Both are deliverables Refs depends on; named here because they gate Refs' clean erasure
    surface (REF-D5 / GIT-D2).
- **CONTRACTS TO IMPLEMENT.** 5.4 (consume the real Git/KN producer edges), 5.6 (consume Git/KN project +
  sub_anchor), 5.7 (the #sub kinds on real sub-anchors), 5.5 (project KN page_parent — the first real mirror),
  2.6 (drive Git/KN replay). To the frozen shapes; the engine does not change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - REF-D1 / REF-D2 re-confirmed green on the REAL Git + KN edge corpora (the leak + IDOR invariants hold on
    production-shaped edges, not just the M2 synthetic corpus) — CI (the gate-invariant ratchet: M2 drills re-run
    on each new producer corpus).
  - REF-D9 green on Git content-anchored line-ranges + KN block/row anchors: a force-pushed PR line-range
    resolves MOVED/OUTDATED/GONE; an edited/deleted KN block resolves OUTDATED/GONE; the root is always carried.
    0 dangling embed, 0 hard 404, no leak. This is the Refs half of GIT-D7 (anchors resolve LIVE/MOVED/OUTDATED/
    GONE; 0 mis-anchored) — CI.
  - REF-D4 reindex-parity green on a Git + KN corpus (cold == live incl. content-anchored line-ranges +
    block-granular sub-artifacts + the KN page_parent lifecycle mirror reconverging to the typed table),
    small-to-moderate scale — CI/SCHED.
- **TESTS (required).** Integration tests against the real Git + KN producers: edges ingested, sub-anchors
  resolved through the ladder, the page_parent mirror projected. A chained test: force-push a PR line-range
  others embed -> the embed resolves MOVED/OUTDATED/GONE with the root carried. The drill scenarios for REF-D1/
  REF-D2 (real corpora), REF-D9 (real sub-anchors), REF-D4 (Git+KN). No new Refs mutation-core module (the
  engine is fixed) — state that the REF-P6/P7/P8 mutation floors still hold on the real corpora.
- **DEFINITION OF DONE.** Refs ingests + resolves the real Git + KN producer edges; REF-D1/REF-D2 re-confirmed
  on real corpora; REF-D9 green on real sub-anchors (the Refs half of GIT-D7); REF-D4 reindex-parity green on a
  Git+KN corpus; the engine is unchanged; the cross-cell + pseudonymous-author floors are named (REF-P14;
  10.6 R-M5); the Refs half of E2E-1 (the PR pane) behaviour is proven in-context (the confidential linked
  issue unfurls to a tombstone carrying the root, title never present); tests pass; committed. No threshold
  weakened.
- **COMMIT.** Header: P-<NNN> M3: Refs consumes Git + Knowledge producer edges. Body lists: 5.4/5.6/5.7/5.5/2.6
  wired to real Git + KN producers; REF-D1/REF-D2 re-confirmed on real corpora; REF-D9 greened on real
  sub-anchors (Refs half of GIT-D7); REF-D4 Git+KN reindex-parity greened; the cross-cell + pseudonymous-author
  floors named. Branch first; do not push unless asked. Co-Authored-By trailer.

---

### REF-P11 — Consumer-subsystem edges: the CI check seam closes + issue relations + chat unfurls

- **BAND.** M4.
- **ROADMAP MILESTONE.** R-M4 (planning/06-roadmaps/shared/reference-graph.md §2 "R-M4 — Consumer-subsystem
  edges").
- **DEPENDS-ON.** REF-P10 (Git + KN edges traversable; the ladder green on real sub-anchors). The M4 CI + Issues
  + Chat producer prompts that ship CI's ci.check.updated producer half + details_ref step anchor, Issues' three
  content nodes + issue_relation typed events + project + key/sub-anchors, Chat's three content nodes + message-/
  thread- anchors + the channel ReBAC fragment. The index resolves these to the CI/Issues/Chat M4 P-NNN. (AG-D4
  re-confirmed green is a band precondition, not a Refs dependency.)
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (Chat references any artifact); ../../external-insights/01-process-and-quality-doctrine.md
    §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §3.5 / §4.6 (the check-/step-
    kinds resolve through the one ladder, C-6), §3.3 (the second real TE-7 mirror — Issues issue_relation), §4.2
    (Chat unfurls via resolve + the shared per-ref cache busting on *.updated).
  - Reconciliation: 00-reconciliation-decisions.md X-1 (the Git<->CI CheckStatus seam; CI is the producer half),
    C-6 (check-/step- first-class #sub kinds).
  - Contracts: contract-index.md rows 5.9 (the Git<->CI CheckStatus seam — CI's producer half closes X-1; Refs
    resolves the check-/step- sub-anchors), 5.5 (Issues issue_relation — the second TE-7 mirror), 5.4 (Chat
    edges), 5.6 (Issues/Chat project + sub-anchors), 5.7 (the field-/row-/message-/thread- kinds), 11.8 (the
    sealed CI log segments the details_ref resolves through).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M4 (all bullets) + §1.2 rows 5.5, 5.9.
  - Drill source: REF-D1/REF-D2 (full five-producer corpus, ~346/347), REF-D9 (CI/Issues/Chat anchors, ~354),
    REF-D4 (TE-7 second-mirror reconvergence, ~349); CHAT-D5 (confidential-unfurl tombstone, master M4 exit
    gate); the X-1 seam GIT-D10/CI-D8 (Refs proves only that the check/step anchors resolve).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate (engine
  unchanged; the remaining producers' edges arrive):
  - The Git<->CI CheckStatus seam, CI's producer half closes (5.9, X-1): CI emits ci.check.updated per
    (commit_oid, context) with run_attempt monotonic supersession; the details_ref = #step-<n> jump-to-failure
    anchor resolves through the Refs ladder (the grammar Refs froze in REF-P1, the consumer Git built in
    REF-P10). Refs' role is the SUB-ANCHOR resolution of check-<context> / step-<n> — the seam itself
    (out-of-order supersession, fork-success-neutral, the merge-queue wake) is the Git+CI X-1 deliverable
    (GIT-D10/CI-D8); Refs proves only that the check/step anchors resolve correctly through the one ladder
    (incl. resolving through the 11.8 sealed log segments).
  - Issues lifecycle edges — the second TE-7 mirror (5.5): Issues' issue_relation typed events (closes/blocks/
    blocked_by/depends_on/relates/parent/assigns) land here; Refs projects them as lifecycle-class edges with
    the REF-P7 inverse pairing, so the spec-to-ship lineage (initiative -> child issues -> PRs -> commits -> CI
    -> deploy -> chat decision) is ONE Refs traverse, not a five-way fan-out. Resolve Issues' project for the
    <PROJECTKEY>-<seqno> key + field-/row- sub-anchors.
  - Chat unfurls — the maximal consumer: Chat's mention/artifact_ref/embed nodes produce edges; Chat consumes
    resolve for every unfurl (commit / issue / doc / CI run) through the 4-step ladder + the shared per-ref cache
    busting on *.updated; message-/thread- sub-anchors resolve (immutable -> LIVE; deleted -> GONE). The Chat
    ReBAC fragment (channel.read = member + parent_project->read) flows through list_objects so a search/backlink
    as a non-member returns 0.
  - Cross-subsystem traversal is now COMPLETE: all five producers emit the structured inline nodes uniformly
    (X-2) + Issues/KN own both typed-relation tables, so mention/ref/lifecycle edges are dependable across
    Git/CI/KN/Issues/Chat.
  - FLOOR named: in-cell single-home-cell graph build (cross-cell fan-out R-M5, REF-P14); no new Refs floor in
    M4 — the engine is fixed at M2; M4 adds only the remaining producer edges + the second lifecycle mirror.
- **CONTRACTS TO IMPLEMENT.** 5.9 (resolve the CI check-/step- sub-anchors — the Refs half of X-1), 5.5 (project
  Issues issue_relation — the second real mirror), 5.4 (consume Chat edges), 5.6/5.7 (Issues/Chat project +
  sub-anchors). To the frozen shapes; the engine does not change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - REF-D1 / REF-D2 green on the FULL five-producer corpus — the leak + IDOR invariants hold across Issues + CI
    check/step anchors + Chat unfurls (the most adversarial corpus: confidential issues, private channels,
    fork-scoped CI). 0 leak, 0 cross-tenant edge — CI.
  - REF-D9 green on CI check-/step- + Issues field-/row- + Chat message-/thread- anchors — every #sub kind
    resolves through the one ladder to the correct state with the root carried (the Refs half of the X-1
    details_ref resolution + the Chat unfurl tombstone — supports CHAT-D5 confidential-unfurl -> tombstone,
    0 title leak) — CI.
  - The lifecycle-mirror reconvergence check (TE-7): an out-of-band edit to an issue_relation row -> a scoped
    reindex reconverges Refs to the typed table (typed wins) — proves REF-D4's TE-7 half on the SECOND real
    mirror (supports ISS-D6) — CI.
- **TESTS (required).** Integration tests against the CI + Issues + Chat producers: check/step anchors resolve;
  issue_relation projected with correct inverse pairing; chat unfurls degrade via the ladder; a non-member
  search/backlink returns 0. A chained test: emit an out-of-order ci.check.updated -> Refs resolves the latest
  by run_attempt context; an out-of-band issue_relation edit -> scoped reindex reconverges. The drill scenarios
  for REF-D1/REF-D2 (full corpus), REF-D9 (all #sub kinds), the TE-7 reconvergence. The CDC pair for the Refs
  consumer side of 5.9 + 5.5 (Issues). State the REF-P6/P7/P8 mutation floors still hold on the full corpus.
- **DEFINITION OF DONE.** Refs resolves the CI check/step anchors (the Refs half of X-1), projects the Issues
  second mirror, and serves Chat unfurls; REF-D1/REF-D2 green on the full five-producer corpus; REF-D9 green on
  every #sub kind; the TE-7 second-mirror reconvergence proven; cross-subsystem traversal is complete; the
  engine is unchanged; the cross-cell floor is named (REF-P14); the Refs half of E2E-1 lights up end-to-end
  in-context; tests pass; committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M4: Refs CI check seam + issue relations + chat unfurls. Body lists: 5.9 (Refs
  half of X-1), 5.5 (Issues second mirror), 5.4 (Chat) wired; REF-D1/REF-D2 greened on the full five-producer
  corpus; REF-D9 greened on all #sub kinds; the TE-7 second-mirror reconvergence proven; the cross-cell floor
  named (REF-P14). Branch first; do not push unless asked. Co-Authored-By trailer.

---

### REF-P12 — World-scale: the 30x surge + the hot-artifact reach index R4 (measured-trigger)

- **BAND.** M5.
- **ROADMAP MILESTONE.** R-M5 partial (planning/06-roadmaps/shared/reference-graph.md §2 "R-M5", the REF-D10
  surge + the REF-D3 hot-fanout / R4 follow-on).
- **DEPENDS-ON.** REF-P11 (all five producer corpora traversable; the deterministic correctness drills green).
  The M5 storage read replica + the surge harness numbers (OQ-K). The protected-human-lane shed order (1.11).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scalable from day 1); ../../external-insights/01-process-and-quality-doctrine.md §3
    (prove-it: the 1x/10x/30x load generator), §2 (the protected human lane);
    ../../external-insights/02-platform-substrate.md §7 (Leopard reach index, measured-trigger).
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §6.2 (measure before you shard;
    the read replica the named first move), §6.3 (the hot-artifact backlink scale — the read-time CTE +
    list_objects filter + pagination + replica floor; the Leopard R4 follow-on promoted at measured hot-fanout >
    read budget), §3.7 (R4 the FLOOR component).
  - Contracts: contract-index.md rows 5.3 (the backlink read, now at scale), 1.11 (the protected-human-lane
    shed order + per-surface shed budgets OQ-K), 1.8 (the hot_artifact_fanout + shed-count telemetry).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M5 (the 30x surge + the hot-artifact reach
    index bullets) + §3 (production-hardened).
  - Drill source: REF-D10 (30x surge, ~355), REF-D3 (hot-fanout, ~348).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate:
  - The 30x agent ref-creation + backlink-read surge handling (REF-D10): tune the protected-human-lane shed
    order (1.11) to Refs' two surfaces — a human's interactive backlink/traverse read holds the protected lane;
    agent ref-creation + backlink-read sheds with 429 + Retry-After; per-tenant in-flight caps keep one tenant's
    agent storm off another's humans (the per-tenant bulkhead). Set the per-surface shed-budget NUMBERS (OQ-K)
    from MEASUREMENT, not prediction; write them into the thresholds file.
  - The hot-artifact backlink scale, the "viral PR / referenced-by-50,000" case (REF-D3): the read-time CTE +
    list_objects filter + pagination + read replica (the doctrine's named first scaling move) is the BUILT
    floor (REF-P6). Build the Leopard-style flattened reach index R4 — derived/rebuildable from R1, incrementally
    maintained from refs.edge.*, gated by the SAME list_objects filter (REF-P6) — and PROMOTE it only when
    measured hot-fanout exceeds the read budget (R5), not predicted. R4 serves post-promotion; the property
    (paginated, leak-free) is fixed at M2, the index is measured here.
  - Sharding edge IF measured (§6.2): the shard key is already (tenant, region) + target_root hash, so a measured
    hot tenant outgrowing one shard is a re-home, not a redesign — measured here, not before. (State as a
    measured-only branch.)
  - FLOOR resolved: this prompt SHIPS the R4 follow-on whose floor was named in REF-P6 — link the pair in the
    commit so the gap is visible.
- **CONTRACTS TO IMPLEMENT.** 5.3 at scale (owned), 1.11 the shed order tuned to Refs' surfaces. Consumed: 4.3
  (R4 gated by the same filter). To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - REF-D10 (30x surge): the human backlink-read lane holds (interactive latency within budget), the agent
    ref-creation + read lane sheds (429 + Retry-After honoured), other tenants unaffected. Green artifact:
    shed-counts + read p99 — SCHED (part of the master M5 F6 surge family).
  - REF-D3 (hot-fanout): "referenced-by-50,000" under concurrent permission-filtered reads -> paginated p99
    within budget; the hot_artifact_fanout telemetry fires; R4 serves post-promotion — SCHED.
- **TESTS (required).** The drill-harness scenarios for REF-D10 (the 30x mixed-principal surge) and REF-D3 (the
  50,000-backlink hot artifact under concurrent filtered reads). A test that R4, once promoted, returns the same
  leak-free result set as the CTE floor (parity between the two paths). State the REF-P6 SetExpr-lowering
  mutation floor still holds on R4 (R4 is gated by the same filter — the leak invariant must not regress).
- **DEFINITION OF DONE.** The shed order is tuned + the budgets measured into the thresholds file; R4 is built +
  measured-promotion-gated; REF-D10 and REF-D3 emit dated green artifacts (human lane holds / agent sheds /
  paginated p99 within budget / R4 parity); the R4 follow-on is linked to its REF-P6 floor; tests pass;
  committed. No threshold weakened (a missed budget becomes a dated claimed-not-proven row, not an edited green).
- **COMMIT.** Header: P-<NNN> M5: Refs 30x surge + hot-artifact reach index R4. Body lists: 1.11 shed order
  tuned (measured OQ-K budgets); R4 built + measured-trigger-gated; REF-D10 + REF-D3 greened (shed-counts /
  read p99 / R4 parity); the R4 follow-on linked to the REF-P6 floor. Branch first; do not push unless asked.
  Co-Authored-By trailer.

---

### REF-P13 — World-scale: reindex-parity + restore + re-erase at backup scale

- **BAND.** M5.
- **ROADMAP MILESTONE.** R-M5 partial (the REF-D4-at-scale + REF-D5-at-backup-scale deliverables).
- **DEPENDS-ON.** REF-P9 (reindex + the erase holder), REF-P11 (the full five-producer corpus). The M5
  restore-verify at cell scale (STOR-D2) + the full DSR fan-out (10.4) + the erasure ledger (10.8). The index
  resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe, world-scale); ../../external-insights/04-hard-problems.md §5.3
    (reindex-from-source at scale), §1 (no resurrected PII past an erasure);
    ../../external-insights/01-process-and-quality-doctrine.md §3.
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §4.7 (reindex-from-source), §4.6
    tail (the erase holder), §7 D-4/D-5 (the scale variants).
  - Contracts: contract-index.md rows 5.8 (reindex at scale), 10.1 (the erase holder at backup scale), 10.8
    (the erasure ledger — post-restore re-erasure), 11.5 (backup/restore/cross-seam).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M5 (the restore + cross-seam + re-erase at
    scale bullet) + §3.
  - Drill source: REF-D4 (reindex-parity at full scale, ~349), REF-D5 (erasure at backup scale, ~350).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate:
  - Restore + cross-seam + re-erase at scale (REF-D5 at backup scale, F3): restore the edge index with
    OLTP/blob/offsets to a consistent point -> NO resurrected edges past an erasure (post-restore re-erasure runs
    from the erasure ledger, 10.8); references stay tombstoned, the person stays unresolvable. This folds into
    the M5 DSAR fan-out (E2E-4 — REF-P14 carries the E2E run; this prompt builds + drills the Refs restore/
    re-erase mechanism it depends on).
  - reindex-parity at full scale (REF-D4 at scale): wipe the edge index, reindex -> byte-matches live across the
    FULL five-producer corpus incl. BOTH TE-7 lifecycle mirrors (KN page_parent + Issues issue_relation). The
    reindex_parity telemetry (1.8) fires.
  - FLOOR resolved: this prompt promotes the REF-P9 CI-variant drills (REF-D4/REF-D5) to their full-scale/
    backup-scale form — link the pair in the commit.
- **CONTRACTS TO IMPLEMENT.** 5.8 reindex at scale (owned), 10.1 erase at backup scale (owned). Consumed: 10.8
  the erasure ledger, 11.5 restore. To the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - REF-D4 (reindex-parity at full scale): rebuilt edge index byte-matches live across the full five-producer
    corpus + both TE-7 mirrors. Green artifact: the reindex-parity hash — SCHED.
  - REF-D5 (erasure at backup scale): erase a subject + a referenced artifact -> references tombstone, the
    person unresolvable, 0 recoverable PII in edge/cache/backups, no 500 on resolve; post-restore re-erasure from
    the ledger leaves 0 resurrected PII. Green artifact: erase-receipt + 0 resolve-error — SCHED (folded into
    E2E-4).
- **TESTS (required).** The drill-harness scenarios for REF-D4 (full-scale reindex) and REF-D5 (backup-scale
  erase + the post-restore re-erase). A chained test: erase -> restore from a pre-erase backup -> re-erase from
  the ledger -> assert 0 recoverable PII. State the REF-P9 reindex/erase mutation floor still holds at scale.
- **DEFINITION OF DONE.** reindex byte-parity at full scale across both TE-7 mirrors; restore + re-erase leaves
  0 resurrected PII; REF-D4 + REF-D5 emit dated green artifacts (parity hash / 0 recoverable PII / 0
  resolve-error); the CI-variant -> full-scale promotion is linked to its REF-P9 floor; tests pass; committed.
  No threshold weakened.
- **COMMIT.** Header: P-<NNN> M5: Refs reindex-parity + restore + re-erase at scale. Body lists: 5.8/10.1 at
  scale; REF-D4 (full-scale parity) + REF-D5 (backup-scale erase, 0 recoverable PII) greened; the full-scale
  promotion linked to the REF-P9 CI floor. Branch first; do not push unless asked. Co-Authored-By trailer.

---

### REF-P14 — World-scale: the cross-cell backlink fan-out build + the E2E wedge (E2E-1/E2E-3/E2E-4)

- **BAND.** M5.
- **ROADMAP MILESTONE.** R-M5 partial (the cross-cell fan-out follow-on + the whole-system E2E scenarios Refs
  crosses).
- **DEPENDS-ON.** REF-P12, REF-P13 (the surge + reindex/re-erase at scale green). The M5 multi-cell bridge live
  (12.6) + the FLOOR drills GA-D8/CP-D7/CP-D8. The other systems' M5 E2E prompts (E2E-1 PR pane, E2E-3
  spec-to-ship, E2E-4 DSAR). The index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1, §3 (EU-sovereign, residency by construction);
    ../../external-insights/04-hard-problems.md §1 (cross-region PII-free); §5.3;
    ../../external-insights/01-process-and-quality-doctrine.md §3, §4 (chained-mutation E2E — drive the whole
    thing).
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §4.2 (cross-cell resolution
    pinned cell-local, C-5 — the home cell renders + permission-checks; only the projection/tombstone crosses,
    over the frozen CrossCellPointer), §6.5 (the cross-cell backlink fan-out FLOOR build).
  - Reconciliation: 00-reconciliation-decisions.md C-5 (cross-cell resolution semantics frozen), OQ-I
    (single-cell -> multi-cell).
  - Contracts: contract-index.md row 12.6 (the cross-cell PII-free pointer bridge CrossCellPointer{subject,
    type, correlation_id, home_cell}; resolution always cell-local), 5.2 (resolve, now cross-cell), 5.3
    (traverse, the E2E-3 lineage walk).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M5 (the cross-cell fan-out + the E2E bullets) +
    §3 (production-hardened).
  - Drill source: GA-D8/CP-D7/CP-D8 (the cross-cell FLOOR drills, master M5 exit gate); E2E-1/E2E-3/E2E-4
    (testing-strategy/01... §2; the four chained-mutation scenarios).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate:
  - The cross-cell backlink fan-out BUILD — the named M2 floor's follow-on (the deepest remaining Refs unknown):
    when multi-cell goes live (12.6), the cross-cell RESOLUTION semantics (already frozen cell-local in REF-P5 —
    the home cell renders + permission-checks; only the projection or a tombstone crosses, over the frozen
    CrossCellPointer) get their FAN-OUT build (ISS cross-cell portfolio rollup, KN cross-cell collab, CHAT
    cross-org channels). The §5 contracts are cell-agnostic so the build EXTENDS WITHOUT A REWRITE. The FLOOR
    drills GA-D8/CP-D7/CP-D8 are now owed and run. Until multi-cell goes live the single-cell path is complete
    and the design is the named floor — link this build to the REF-P5 floor in the commit.
  - The whole-system E2E scenarios Refs crosses (run the Refs side against a full cell with mock agents):
    E2E-1 (the PR context pane — Refs is the spine: every connected artifact resolves per-viewer, the
    confidential issue -> tombstone carrying the root, 0 title/count/backlink leak, the live check-update lands
    within the freshness budget); E2E-3 (spec-to-ship — traverse(spec_doc, viewer) walks the ENTIRE lineage
    depth-16 cycle-safe per-viewer, and the wiped Refs edge index reindexes to byte-match live, F4/REF-D4 at
    scale); E2E-4 (DSAR fan-out — Refs' edges + cache return 0 recoverable PII, unfurls degrade to tombstones,
    the holder-coverage receipt includes Refs). Each emits its named green artifact.
- **CONTRACTS TO IMPLEMENT.** 12.6 the cross-cell PII-free bridge (consumed; the fan-out build rides it), 5.2/5.3
  cross-cell + the E2E lineage walk (owned, extended). To the frozen shapes; the build extends without a rewrite.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - GA-D8 / CP-D7 / CP-D8 (the cross-cell FLOOR drills, now owed): the cross-cell erasure receipt set; cell->cell
    migration 0 loss; the cross-cell ref PII-free bridge (only the projection/tombstone crosses, never raw rows/
    PII). Green artifacts: the per-cell receipt set / 0-loss migration / PII-free assertion — SCHED.
  - E2E-1 green (the PR pane — Refs is the spine; 0 title/count/backlink leak; the confidential issue ->
    tombstone carrying the root; the live check-update within budget) — SCHED.
  - E2E-3 green (the full lineage traverse depth-16 per-viewer + the wiped index reindexes to byte-match live) —
    SCHED.
  - E2E-4 green (Refs' edges + cache return 0 recoverable PII; unfurls -> tombstones; the holder-coverage
    receipt includes Refs) — SCHED.
- **TESTS (required).** The cross-cell fan-out integration test (a viewer in cell A resolving a pointer homed in
  cell B -> only the projection/tombstone crosses, never raw rows). The drill-harness scenarios for GA-D8/CP-D7/
  CP-D8. The three chained-mutation E2E scenarios E2E-1/E2E-3/E2E-4 (the Refs side), each driving the whole flow
  end-to-end (not single handlers). State the REF-P5/P6 leak-invariant mutation floors still hold cross-cell
  (the leak invariant must not regress across the cell boundary).
- **DEFINITION OF DONE.** The cross-cell fan-out is built (extends without a rewrite); GA-D8/CP-D7/CP-D8 emit
  dated green artifacts; E2E-1 (Refs as spine), E2E-3, E2E-4 each emit their named green artifact; the
  cross-cell build is linked to its REF-P5 floor; tests pass; committed. This completes R-M5 — the master M5
  exit gate cites E2E-1..E2E-4 green; M6 does not start over a red E2E-1. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M5: Refs cross-cell fan-out build + the E2E wedge. Body lists: 12.6 cross-cell
  fan-out built (extends, no rewrite); GA-D8/CP-D7/CP-D8 greened; E2E-1/E2E-3/E2E-4 greened (Refs side); the
  cross-cell build linked to the REF-P5 floor. Branch first; do not push unless asked. Co-Authored-By trailer.

---

### REF-P15 — Dogfooding: the reference graph over Myelin's own work + the switch-test surfaces

- **BAND.** M6.
- **ROADMAP MILESTONE.** R-M6 (planning/06-roadmaps/shared/reference-graph.md §2 "R-M6 — Dogfooding").
- **DEPENDS-ON.** REF-P12, REF-P13, REF-P14 (the production-hardened reference graph — all Refs drills + the
  E2E wedge green). The M6 self-hosting CI graph + the Myelin monorepo on Myelin git hosting + the Myelin issues/
  Knowledge spaces. The index resolves these.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §5 (dogfooding); ../../external-insights/01-process-and-quality-doctrine.md §4 (the switch
    test — drive the real UI in a browser; §4 actually try it).
  - Architecture: ../05-refined-shared-systems-architecture/reference-graph.md §1 (the moat thesis — jump from a
    failing test to the line of code to the issue to the conversation in four keystrokes).
  - Roadmap: planning/06-roadmaps/shared/reference-graph.md §2 R-M6 (the work + the gate) + §3
    (production-hardened -> only here carries the builders' own data).
  - Master sequencing: planning/06-roadmaps/00-master-sequencing.md §2 M6 (the switch tests; the self-hosting CI
    graph green; the truth-up pass — no earlier-band gate red).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-refs service crate + the Myelin
  self-hosting deployment config:
  - Run the reference graph over Myelin's own work: the PR context pane on the Myelin monorepo's PRs (commits <->
    issues <-> CI checks <-> KN docs <-> chat threads), the spec-to-ship lineage on the roadmap/gap-report/
    scorecard living as Myelin issues + a Myelin Knowledge space (the every-incident-adds-a-drill loop files a
    Myelin issue + a reproducing drill, both reference-linked). The Refs drills run as Myelin CI jobs on Myelin's
    own commits (the dogfood loop).
  - The reference-graph contribution to the per-subsystem SWITCH TESTS (folded into the L5 done-bars): does a
    GitHub/Jira/Linear/Notion user's cross-artifact navigation work — unfurls live, backlinks complete,
    tombstones graceful — without hitting a wall the old tool didn't have? Measured against latency budgets
    (backlink read / unfurl within the keyboard / no-spinner-flash budgets). Drive the four-keystroke
    cross-artifact jump IN A BROWSER, not by reading the feature list.
  - FLOOR named: none new — M6 promotes nothing; it exercises the production-hardened reference graph on real
    (self-)tenant data.
- **CONTRACTS TO IMPLEMENT.** None new — the engine is fixed at M2 and hardened through M5. This prompt
  exercises the production surface (5.2/5.3/5.7) on real self-tenant data and wires the Refs drills into the
  Myelin self-hosting CI graph.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - Refs is green on the self-hosting CI graph (the Refs drills run as Myelin CI jobs on Myelin's own commits —
    the dogfood loop is live) — SCHED.
  - The reference-graph switch-test surfaces pass when driven in a browser (measured latency; the four-keystroke
    cross-artifact jump works — backlink read / unfurl within the keyboard / no-spinner-flash budgets) — SCHED.
  - The truth-up pass: every Refs PROVEN row (REF-D1..D10 + the E2E rows) rests on a DATED green artifact, never
    a doc claim — no earlier-band Refs gate is red (code-wins-over-docs, EI-01 §1) — SCHED.
- **TESTS (required).** The switch-test browser drive (the four-keystroke jump across the five real subsystems on
  Myelin's own data, against the latency budgets). The Refs drills wired as Myelin CI jobs (the dogfood loop).
  A truth-up audit script that confirms every Refs PROVEN row links a dated green artifact. Record honestly
  (yes/no/partial) which switch-test surfaces were driven in a browser vs. only automated.
- **DEFINITION OF DONE.** The reference graph runs over Myelin's own work; the Refs drills are green as Myelin
  CI jobs; the switch-test surfaces pass when driven in a browser (measured latency); the truth-up pass confirms
  no earlier-band Refs gate is red (every PROVEN row dated-and-green); any surface only-automated-not-browser-
  driven is named honestly; committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M6: reference graph dogfooded on Myelin's own work. Body lists: the Refs drills
  green as Myelin CI jobs (the dogfood loop); the switch-test surfaces driven in a browser (measured latency,
  the four-keystroke jump); the truth-up pass (0 red earlier-band Refs gates); any only-automated surface named.
  Branch first; do not push unless asked. Co-Authored-By trailer.

---

## Coverage check (every R-M milestone -> its prompt(s))

| Roadmap milestone | Band | Prompt(s) |
|---|---|---|
| R-M0 (ArtifactRef value type + the Refs ratchet) | M0 | REF-P1 |
| R-M1 (Refs as a holder + the edge-index encryption floor) | M1 | REF-P2 |
| R-M2 (the Refs core — edge index, emit seam, resolution, backlink read, traverse, #sub ladder, erasure, reindex) | M2 | REF-P3, REF-P4, REF-P5, REF-P6, REF-P7, REF-P8, REF-P9 |
| R-M3 (Git links + KN embeds + the first lifecycle mirror) | M3 | REF-P10 |
| R-M4 (CI check seam + issue relations + chat unfurls) | M4 | REF-P11 |
| R-M5 (30x surge + R4; reindex/restore/re-erase at scale; cross-cell fan-out + the E2E wedge) | M5 | REF-P12, REF-P13, REF-P14 |
| R-M6 (dogfooding) | M6 | REF-P15 |

**Floor -> follow-on pairing (name-your-floors, EI-01 §1 / master §5):**
- per-tenant DEK (REF-P2) -> the structural erasure surface (REF-P9).
- R2-invalidator no-op shim (REF-P3) -> the live R2 cache (REF-P8).
- synthetic producers / synthetic typed events (REF-P4, REF-P7) -> real producer edges + real mirrors (REF-P10,
  REF-P11).
- read-time CTE + pagination + replica for hot backlinks (REF-P6) -> the Leopard reach index R4 (REF-P12).
- the #sub grammar + the one ladder (REF-P8) -> each subsystem's stable #sub mint (REF-P10, REF-P11).
- CI-variant REF-D4/REF-D5 (REF-P9) -> full-scale / backup-scale REF-D4/REF-D5 (REF-P13).
- cell-local cross-cell resolution semantics (REF-P5) -> the cross-cell backlink fan-out build (REF-P14).
- Git pseudonymous-by-default commit authors (REF-P10, Git deliverable) -> the audited history-rewrite erasure
  path (10.6, R-M5 / on-demand — owned by Git, named here because it gates REF-D5).

**Drill coverage (every REF-D greened by some prompt's GATE/DRILLS):** REF-D1 (REF-P5 resolve half + REF-P6
backlink half + REF-P7 traverse half, re-confirmed REF-P10/REF-P11), REF-D2 (REF-P6, re-confirmed REF-P10/
REF-P11), REF-D3 (REF-P12), REF-D4 (REF-P9 CI + REF-P10 Git+KN + REF-P13 full-scale), REF-D5 (REF-P9 CI +
REF-P13 backup-scale + REF-P14 E2E-4), REF-D6 (REF-P6), REF-D7 (REF-P3 + REF-P4 emit side), REF-D8 (REF-P7),
REF-D9 (REF-P8 synthetic + REF-P10/REF-P11 real), REF-D10 (REF-P12); plus GA-D8/CP-D7/CP-D8 + E2E-1/E2E-3/E2E-4
(REF-P14) and the switch tests + self-hosting CI graph (REF-P15).
