# Phase 7 — Prompt Ledger: Knowledge Platform (the producer subsystem, Notion-class)

> Phase: 07-prompts (per-system file, Phase 7-A). The complete ordered set of implementation prompts that
> operationalize the entire knowledge-platform roadmap
> (planning/06-roadmaps/subsystems/knowledge-platform.md, milestones KN-M2 + KN-M3a..KN-M3e + KN-M5 + KN-M6)
> into clean-context, independently-committable coding tasks. Built to the template in
> planning/07-prompts/00-ledger-overview.md §2 (every field present, never implicit) and banded to
> planning/06-roadmaps/00-master-sequencing.md §2 (M0..M6, the gate invariant). Frozen architecture (this file
> OPERATIONALIZES, it does not redesign): planning/04-subsystem-architectures/knowledge-platform/architecture/
> (00..07) + the design sketches under planning/04-subsystem-architectures/knowledge-platform/design/ + the
> build-to contracts in planning/05-refined-shared-systems-architecture/contract-index.md +
> 00-reconciliation-decisions.md (X-2/X-3/X-4/X-6/X-7, OQ-C/OQ-E/OQ-F/OQ-I/OQ-J/OQ-K/OQ-L). Drills:
> planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
> (KN-D1..KN-D13 + the E2E wedge). Plain-text identifiers throughout (no backticks-as-emphasis). Markdown only;
> this file makes no commits. Date: 2026-06-19.
>
> The global P-NNN ids are assigned by the consolidated ledger index (Phase 7-B, 01-ledger-index.md) when these
> per-system prompts are interleaved into the single execution order. Here each prompt carries a stable local
> handle KN-P<n> so its DEPENDS-ON edges are unambiguous before global numbering; the index rewrites KN-P<n> to
> its P-NNN. Where a prompt depends on another system's prompt not yet numbered, it names that system's
> milestone (the index resolves it to the P-NNN).
>
> Knowledge is a PRODUCER subsystem (master §2 M3, §3.2). Its bulk lands in M3 (the producer band, alongside
> Git), with a freeze-so-dependents-compile slice in M2 (Knowledge LEADS and FREEZES myelin-content and
> co-owns myelin-query + order_key, X-2/X-3 — Issues/Chat/Search build on the frozen subset and cannot drift),
> its world-scale follow-ons (the Yrs CRDT promotion, cross-cell collab, facet/rollup materialisation) in M5,
> and the dogfood switch test in M6. The headline drill KN-D1 (reconnect-loses-zero-ops) is Knowledge's
> deliverable over the M2 firehose resume-cursor transport (contract 3.5) — the build-order law (KN-1, EI-04
> §2.2) makes that transport item 0.
>
> Coverage: KN-M2 → KN-P1 + KN-P2; KN-M3a → KN-P3 + KN-P4; KN-M3b → KN-P5 + KN-P6; KN-M3c → KN-P7 + KN-P8;
> KN-M3d → KN-P9 + KN-P10 + KN-P11; KN-M3e → KN-P12 + KN-P13; KN-M5 → KN-P14 + KN-P15; KN-M6 → KN-P16. Sixteen
> prompts, no milestone gap.

---

### KN-P1 — Freeze myelin-content (the v1 block + inline taxonomy) and compile the WASM render path (KN-D2)

- **BAND.** M2.
- **ROADMAP MILESTONE.** KN-M2 (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M2", the
  myelin-content freeze + the WASM render-path half; the myelin-query/order_key freeze is KN-P2).
- **DEPENDS-ON.** The M0 substrate prompts that lay down the Cargo workspace + the eight glue-crate skeletons
  (including the myelin-content crate skeleton) + the twelve lints + the contract-coverage scanner (master §2
  M0; substrate roadmap SUB-M0). The index places this in the M2 reactive-layer band; no Knowledge runtime
  dependency yet, but myelin-content must freeze before Chat/Issues consume the subset, so this is among the
  earliest M2 prompts.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md (always) §3 (top-of-the-line UX; design comes before frontend; name-your-floors);
    ../../external-insights/01-process-and-quality-doctrine.md §7 (reconcile cross-component contracts at the
    plan layer before either side ships — field names AND units; the two-divergent-renderers trap), §3
    (prove-it: render(parse(md))===md is the quantified gate); ../../external-insights/05-ux-and-design.md §2
    (the one-render-path editor mandate).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/01-tech-and-data-model.md §2
    (the complete frozen myelin-content block taxonomy §2.1 + the markdown-subset inline grammar with the three
    structured nodes mention/artifact_ref/embed at U+FFFC anchors §2.2);
    02-internals-and-algorithms.md §8 (the one editor render path; the WASM target; §8.3 why a markdown-subset
    string not inline-range JSON).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-2 (myelin-content
    frozen; Chat/Issues consume subsets; the three inline ref nodes uniformly produce refs.edge.created).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 13.1 (the myelin-content taxonomy
    frozen + the WASM compile target + render(parse(md))===md), 13.2 row referenced only for the ADF map
    boundary (built in KN-P2). Lint rows 1.6 referenced for the no-untagged-personal-data discipline the inline
    free-text classification will later satisfy.
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M2" + §2 (the row 13.1 obligation:
    LEADS + FREEZES).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    KN-D2 (render(parse(md))===md, 100% round-trip, 0 corpus regressions).
- **DELIVERABLE (what to build + exactly where in the repo).** In the shared glue crate myelin-content (the M0
  skeleton crate under the Cargo workspace):
  - The frozen v1 Block enum byte-for-byte from arch 01 §2.1 (paragraph/heading/bullet_list/ordered_list/
    task_list/blockquote/code_block/callout/table/divider/image/embed/db_view/toggle/sync_block). code_block.text
    is raw (NOT markdown-parsed). sync_block exists as a node here (the taxonomy is complete); its engine is the
    floor shipped in KN-P6 — name that here.
  - The inline grammar: a markdown-subset parser (parse_inline) + serializer (serialize_inline) over the subset
    bold/italic/code/strike/link, plus the three structured inline nodes mention(Principal)/artifact_ref(ArtifactRef)/
    embed(ArtifactRef) represented as opaque U+FFFC sentinels with a positional inline_nodes array (so
    reference-extraction is a node-array walk, server-side reliable).
  - Compile the parser/serializer to the WASM target (one render implementation, client + server) so the
    round-trip gate holds on identical code — wire the wasm build into the crate so the corpus gate runs against
    the WASM artifact, not a second renderer.
  - A frozen corpus fixtures directory (the three structured nodes U+FFFC-anchored × nesting in bold/lists/
    tables, code blocks, IME/paste edge cases) that the KN-D2 gate runs over.
  - FLOOR named: none for the taxonomy (this is a freeze). Name that sync_block's engine is a read-projection
    floor landing in KN-P6 (follow-on: editable-in-place multi-home, post-M5, KQ-6). State in the crate doc that
    no Knowledge feature ships here — only the shared shape Chat/Issues/Search compile against.
- **CONTRACTS TO IMPLEMENT.** 13.1 myelin-content taxonomy + the WASM render target (owned/frozen — Knowledge
  is the freeze authority; Chat/Issues consume subsets, they do not redefine). Implement to the frozen shape; a
  needed shape change is a whole-workspace contract PR, escalated and written down, not a local divergence
  (code-wins-over-docs).
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D2 → render(parse(md)) === md over the frozen corpus: 100% round-trip, 0 regressions; the corpus-pass-rate
    telemetry signal = 100% is the dated green artifact — CI. (KN-D2 is owed in M2 because the WASM render path
    freezes here; it re-runs over the integrated editor in KN-P5.)
  - The crate compiles to both the native and the WASM target from one source (build-time gate; the WASM artifact
    is the green) — CI.
- **TESTS (required).** Unit tests for parse_inline/serialize_inline over each subset construct and each
  structured node; property-style round-trip tests feeding the corpus through render(parse(md)). The
  provider/consumer CDC pair for contract-index row 13.1 (Knowledge provides the frozen taxonomy; a consumer
  stub for Chat/Issues/Search subset compilation). myelin-content's parser is mandatory-core: state the
  cargo-mutants mutation-score floor for the parse/serialize module (the round-trip property must survive
  mutation).
- **DEFINITION OF DONE.** The frozen Block enum + inline grammar compile native and WASM from one source; KN-D2
  emits its dated green artifact (100% round-trip, 0 regressions); unit + round-trip tests and the 13.1 CDC pair
  pass; the contract-coverage scanner is green on row 13.1; all committed lints green; the sync_block engine
  floor is named with its KN-P6 follow-on; the no-feature freeze note is written; the work is committed. No gate
  is greened by weakening a threshold or shrinking the corpus.
- **COMMIT.** Header: P-<NNN> M2: freeze myelin-content taxonomy + WASM render path (KN-D2). Body lists: contract
  13.1 frozen; KN-D2 greened (render(parse(md))===md = 100%, 0 regressions, measured); the mutation floor for the
  parser stated; the sync_block read-projection floor named (KN-P6 ships it, post-M5 editable follow-on KQ-6).
  Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### KN-P2 — Freeze myelin-query (FieldType/ViewSpec/QueryAst) + the order_key/LexoRank encoding + the ADF lossy-map (X-3 byte-identical with Issues)

- **BAND.** M2.
- **ROADMAP MILESTONE.** KN-M2 (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M2", the
  myelin-query + order_key + ADF-map freeze half).
- **DEPENDS-ON.** KN-P1 (the content crate exists alongside the query crate). The M0 substrate prompts (the
  myelin-query glue-crate skeleton). The Issues M2 prompt that co-authors the myelin-query/order_key freeze
  byte-identical (the X-3 reconciliation at the plan layer) — the index pairs this with that Issues prompt so the
  shared conformance vector is authored once and both sides build to it. The Bus M2 prompt that froze the
  EventMatcher core (QueryAst = 3.4).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors); ../../external-insights/01-process-and-quality-doctrine.md §7
    (reconcile cross-component contracts at the plan layer before either side ships — a unit/encoding mismatch
    that ships on one side calcifies; this is the X-3 anti-drift directive verbatim).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/01-tech-and-data-model.md §1.2
    (JSONB + derived projection, not per-tenant DDL — the why) + the data-model sections naming FieldType/
    ViewSpec; 02-internals-and-algorithms.md §3.5 (the LexoRank encoding under concurrency — the 2-char jitter,
    the 48-char rebalance) + §4.1 (the SetExpr push-down the executor lowers, built in KN-P8).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-3 (myelin-query +
    order_key byte-identical with Issues; the LexoRank conformance vector), OQ-C (the QueryAst = EventMatcher
    core, bounded, no UDFs/loops/recursion).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 13.3 (the myelin-query primitive
    frozen byte-identical — the field-type enum, ViewSpec, the QueryAst grammar = the EventMatcher core 3.4, and
    the order_key/LexoRank fractional-index encoding: base-62 0-9A-Za-z, lexicographic compare, "U" first,
    midpoint bisection, 2-char jitter, 48-char rebalance, created_at+ULID tiebreak), 13.2 (the ADF →
    myelin-content lossy-map for the Issues import; lossy nodes named + recorded), 3.4 (the EventMatcher = the
    frozen QueryAst — the same core).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M2" + §2 (rows 13.3 co-owns, 13.2).
  - Drills: testing-strategy/01-...-catalogue.md — the X-3 LexoRank conformance vector (byte-for-byte rank parity
    with Issues).
- **DELIVERABLE (what to build + exactly where in the repo).** In the shared glue crate myelin-query (the M0
  skeleton) + the myelin-content crate's ADF-map module:
  - The frozen FieldType enum, the ViewSpec view-model, and the QueryAst grammar (the bounded interpreter core =
    the bus EventMatcher 3.4: no UDFs, no loops, no recursion, statically cost-bounded, permission-aware shape).
    These are the definitions both Knowledge and Issues compile their own executors against (each owns its
    executor; the definitions are byte-identical).
  - The order_key/LexoRank encoding exactly to the 13.3 spec (base-62 0-9A-Za-z, "U" first, midpoint bisection,
    2-char jitter for concurrent same-gap inserts, 48-char rebalance trigger, created_at+ULID tiebreak), with a
    shared conformance-vector fixture (a sequence of rank operations + expected outputs) committed in the crate
    and consumed identically by Issues.
  - The ADF → myelin-content lossy-map table (13.2): the conversion table for the Issues import, with lossy
    nodes named and an import-report shape that records each lossy conversion.
  - FLOOR named: none — this is a freeze; the CRDT lands over the order model without changing it (KN-P14), so
    say the order_key stays the OLTP ordering encoding now and becomes a CRDT-derived hint post-promotion.
- **CONTRACTS TO IMPLEMENT.** 13.3 myelin-query + order_key (co-owned/frozen with Issues — Knowledge ships the
  shared definitions + its half of the conformance vector). 13.2 the ADF lossy-map (Knowledge ships the table;
  Issues consumes it at import). 3.4 referenced (QueryAst = EventMatcher; Knowledge does not redefine it).
  Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The LexoRank conformance vector passes byte-for-byte identically on the Knowledge side and the Issues side
    (the X-3 anti-drift check): 0 rank divergences across the shared vector — CI, the shared-vector parity signal
    = 0 divergences is the green artifact.
  - The QueryAst is statically cost-bounded (a fixture asserting no construct admits unbounded cost; a red
    fixture of an unbounded expression is rejected) — CI.
- **TESTS (required).** Unit tests for FieldType/ViewSpec/QueryAst serialization and the order_key operations
  (insert-between, jitter under collision, rebalance at 48 chars). The shared LexoRank conformance-vector test
  run on both sides (the build fails if the two outputs differ). The CDC pair for rows 13.3 and 13.2. order_key
  + QueryAst are mandatory-core: state the cargo-mutants mutation-score floor for the rank-encoding module and
  the QueryAst cost-bounding.
- **DEFINITION OF DONE.** The frozen FieldType/ViewSpec/QueryAst + the order_key encoding + the ADF map compile;
  the LexoRank conformance vector is byte-identical on both sides (0 divergences, dated); the QueryAst
  cost-bound holds (red+green fixtures); unit tests + the 13.3/13.2 CDC pairs pass; the contract-coverage scanner
  is green; all committed lints green; the no-feature freeze note is written; the work is committed. No threshold
  is weakened; the parity is real, not asserted.
- **COMMIT.** Header: P-<NNN> M2: freeze myelin-query + order_key/LexoRank + ADF lossy-map (X-3 parity). Body
  lists: contract 13.3 frozen (FieldType/ViewSpec/QueryAst + order_key) byte-identical with Issues; 13.2 ADF map
  shipped; the LexoRank conformance vector greened (0 divergences, measured); the QueryAst cost-bound greened;
  mutation floors stated. Branch first if on default; do not push unless asked. End with the workspace
  Co-Authored-By trailer.

---

### KN-P3 — The Knowledge service shell over serve(AppSpec) + the transactional outbox + OLTP partition (KN-D7, KN-D13)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3a (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3a", the service
  shell + outbox + partition half; the resume-cursor transport is KN-P4 in the same band before any editor).
- **DEPENDS-ON.** KN-P1, KN-P2 (the content + query crates frozen). The M1 Identity prompts that ship
  authenticate (4.1) + check (4.2). The M1 Storage prompts that ship the OLTP client + RLS + the outbox table
  (11.1) and pass STOR-D1/STOR-D2 (restore-verify — the silent-data-loss floor; Knowledge writes no row until
  green). The M1 Tenancy prompt that ships the (tenant,region) partition (12.1) + the residency-pin lint. The M0
  substrate prompts (serve(AppSpec) 1.1, the three-surface 1.2, liveness≠readiness 1.3, the OutboxTx::emit 2.2 +
  outbox table 2.3 + EventHandler template 2.4 + dedup 2.5, the tenant-predicate + no-raw-publish lints).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe & EU-sovereign by construction; world-scale + multi-tenant from day 1);
    ../../external-insights/01-process-and-quality-doctrine.md §2 (silent-data-loss outranks every feature — no
    write before restore-verify is green), §5 (the ratchet — the lints are committed gates).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/01-tech-and-data-model.md §1
    (the Rust + Postgres choice; the per-service DB, the no-cross-db boundary; the outbox is the cross-seam
    anchor); 03-events-contracts-and-glue.md §4 (the envelope via the transactional outbox ONLY — no
    fire-and-forget; the aggregate = the doc/row/db; coalescing before emit) + §1 (the complete knowledge.*
    event taxonomy registered here).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 1.1/1.2/1.3 (serve(AppSpec) +
    three surfaces + liveness≠readiness), 2.2/2.3/2.4/2.5 (OutboxTx::emit, the outbox table with UNIQUE(aggregate,
    seq), the EventHandler template with whitelisted subjects never *, the consumer_dedup ledger), 2.9 (the
    event taxonomy grammar — register the knowledge.* tokens), 11.1 (the OLTP client + RLS), 12.1 ((tenant,region)
    partition), 4.1/4.2 (authenticate/check on the entrypoints), 1.5 (the hot-table flags block/db_row/doc_op).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3a" + §2 (rows 1.1/1.4/1.5/2.x/4.1/
    4.2/11.1/12.1).
  - Drills: testing-strategy/01-...-catalogue.md KN-D7 (crash between commit and relay-publish → 0 ghost, 0 lost),
    KN-D13 (cross-tenant read via path-tenant spoof → 0; tenant-predicate lint catches a tenant-less query at
    compile).
- **DELIVERABLE (what to build + exactly where in the repo).** In a new subsystem implementation crate
  myelin-knowledge under the workspace:
  - The Knowledge service as an AppSpec over serve (1.1) — boot → migrate → outbox relay → consumers → the three
    ports (public/internal/metrics-health, 1.2) → graceful drain; liveness≠readiness (1.3). Not a hand-rolled
    main.
  - Declare the hot-table flags block / db_row / doc_op to the migration runner (1.5, forward-only online
    migrations).
  - The OLTP store: the block table, db_row, the typed relation tables, the op-log + snapshot metadata, and the
    per-service outbox table (2.3) — all (tenant,region)-partitioned with RLS (11.1/12.1). Every query carries
    the tenant predicate (the tenant-predicate lint must compile-reject a tenant-less query).
  - The OutboxTx::emit path (2.2): every state change emits iff committed, in the same DB transaction; no
    fire-and-forget (the no-raw-publish lint). The relay drains FOR UPDATE SKIP LOCKED, dedups on the ULID
    event_id, dead-letters after bounded retries.
  - Register the complete knowledge.* event taxonomy under the Bus §6 grammar (2.9) — page/doc/block/database/
    view/row/comment/mention lifecycle + the cross-cutting knowledge.*.erased and knowledge.*.snapshot tokens
    (arch 03 §1). The EventHandler consumer template (2.4) with a whitelisted subjects() set (never *) + the
    dedup ledger (2.5) — the consumers themselves land in KN-P10/KN-P12.
  - authenticate/check (4.1/4.2) on the read/write entrypoints (full permission filtering lands in KN-P8).
  - FLOOR named: fs-backed BlobStore (11.2) is the M1 floor Knowledge uses for media/snapshots; follow-on
    object-store BlobStore is KN-P15 (M5, one-line swap). Name it.
- **CONTRACTS TO IMPLEMENT.** 1.1/1.2/1.3 serve(AppSpec) + three-surface + liveness≠readiness (consumed —
  Knowledge boots from the harness). 2.2/2.3/2.4/2.5 the outbox + consumer template (consumed — wired into the
  Knowledge store). 2.9 the knowledge.* tokens (owned — registered). 11.1 OLTP client + RLS (consumed). 12.1
  (tenant,region) partition (consumed). 4.1/4.2 authenticate/check (consumed — call sites). 1.5 hot-table flags
  (owned — declared). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D7 → crash the Knowledge service between the block/row commit and relay-publish → the event is still
    delivered (outbox survived) and never delivered without the state change: 0 ghost, 0 lost; the outbox
    depth+age telemetry returns to baseline after recovery — CI.
  - KN-D13 → read a page/db/row across tenants via path-tenant spoofing → 0 cross-tenant read (tenant from the
    token); the tenant-predicate lint is RED on a deliberately tenant-less query fixture and GREEN on the Knowledge
    schema (per-tenant counters signal = 0 cross-tenant) — CI.
  - The no-raw-publish lint green on the Knowledge crate (0 publish paths outside the outbox) — CI.
- **TESTS (required).** Unit tests for the AppSpec wiring, the outbox emit-iff-committed path, the tenant-scoped
  query helpers, and the knowledge.* token grammar round-trip. The drill-harness scenarios for KN-D7 and KN-D13
  (chained: write → crash mid-relay → recover → assert exactly-once; and a cross-tenant spoof attempt). The CDC
  pairs for rows 2.2/2.3/2.4/2.9 (Knowledge's owned/consumed halves). The outbox emit path is mandatory-core:
  state the cargo-mutants mutation-score floor for the emit-iff-committed module.
- **DEFINITION OF DONE.** The service boots over serve(AppSpec); the outbox emits iff committed; the (tenant,
  region) partition + RLS hold; the knowledge.* tokens register and parse; KN-D7 and KN-D13 emit their dated
  green artifacts (0 ghost/0 lost; 0 cross-tenant); the no-raw-publish + tenant-predicate lints green with
  fixtures; unit + drill tests + the CDC pairs pass; the contract-coverage scanner is green; the fs-backed
  BlobStore floor is named (KN-P15 swaps it); the work is committed. No gate is weakened to pass.
- **COMMIT.** Header: P-<NNN> M3: Knowledge service shell + transactional outbox + (tenant,region) partition
  (KN-D7/KN-D13). Body lists: contracts 1.1/2.2/2.3/2.4/2.5/2.9/11.1/12.1/4.1/4.2 wired; KN-D7 (0 ghost/0 lost)
  and KN-D13 (0 cross-tenant) greened with measured numbers; the no-raw-publish + tenant-predicate lints green;
  the fs-backed BlobStore floor named (KN-P15 follow-on). Branch first if on default; do not push unless asked.
  End with the workspace Co-Authored-By trailer.

---

### KN-P4 — Transport item 0: the resume-cursor durable collab transport over the firehose (KN-D1, the headline)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3a (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3a", the
  resume-cursor durable transport — build-order item 0, KN-1 / EI-04 §2.2).
- **DEPENDS-ON.** KN-P3 (the service shell + outbox + the doc_op op-log table exist). The M2 Bus/Signals prompt
  that ships the frozen firehose resume-cursor transport + the subscribe/resume protocol (contract 3.5, OQ-J) —
  the bus provides the seam; Knowledge owns the resume-cursor + idempotent-apply + (later) the CRDT over it. The
  M2 prompt that ships the *.snapshot resync fallback target.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native: event propagation + triggers first-class; no silent data loss);
    ../../external-insights/04-hard-problems.md §2 (real-time collab — the named hard problem; §2.2 a relay
    without resume cursors silently loses the gap on reconnect — the transport is item 0);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it: 0 ops lost / 0 duplicate across a
    kill is a quantified gate; observability is part of the pass).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md
    §1 (the layered collab stack — transport is Layer 1, built first), §2 (the full resume-cursor protocol:
    CONNECT/SEND_OP/RECONNECT pseudocode; op_seq = the firehose seq per (stream, scope); op_id = (client_id,
    lamport); UNIQUE(tenant,page_id,op_id) idempotent apply; resync_required → *.snapshot fallback; bounded
    scope=doc:<page_id>; presence ephemeral never persisted §2.3).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 3.5 (the firehose subscribe/resume
    protocol + the resume-cursor + idempotent-apply property + the CRDT-over-it as Knowledge's deliverable),
    2.6 (the *.snapshot reindex-from-source re-emit, the resync fallback target).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-J (the firehose
    resume-cursor subscription protocol — subscribe/resume + the resync_required snapshot fallback).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3a" (the build-order law) + §2 (row
    3.5).
  - Drills: testing-strategy/01-...-catalogue.md KN-D1 (kill a collab client mid-edit + sever during sustained
    multi-author edit; on resume(scope=doc:<id>, last_seq) → 0 ops lost, 0 duplicate; written to re-run across the
    engine_promote boundary in KN-P14).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge, a collab transport
  module:
  - The resume-cursor durable transport over firehose::subscribe(stream=fan.<tenant>.knowledge, scope=doc:<page_id>,
    cursor?) / firehose::resume(stream, scope, last_seq) (3.5). The doc_op op-log table with a per-doc monotonic
    op_seq (== the firehose seq) and UNIQUE(tenant, page_id, op_id) idempotent apply (op_id = (client_id, lamport)).
  - CONNECT: authorize (Id.check edit|comment on the page_ref, zookie — Layer 2 stub here, full ABAC in KN-P8) →
    resume backfills (cursor, now] then live; on resync_required (cursor below the retention window) load the
    knowledge.page.snapshot (block-granular *.snapshot) then go live. SEND_OP: assign op_seq + op_id → PERSIST
    INSERT ... ON CONFLICT DO NOTHING (idempotent) → apply to live state → firehose.publish the frame → coalesce
    (debounced) → emit knowledge.doc.updated pointer + knowledge.page.updated semantic via the OUTBOX (never
    per-keystroke on the durable bus). RECONNECT: re-run CONNECT(last_durably_applied_op_seq) → resume replays
    exactly (last_seq, now]; the UNIQUE(op_id) makes re-sends no-ops.
  - The bounded-scope discipline: reject an unbounded scope (the whitelist-not-* rule generalised to the
    firehose); a huge doc paginates its scope to the visible block window + a margin.
  - The ephemeral presence/awareness tier (cursors/selections/who-is-here) over the firehose presence channel —
    throttled, NOT persisted (arch 02 §2.3).
  - FLOOR named: the transport carries CAS op bytes in v1 (the merge engine is KN-P7); the op-log carries Yrs
    update bytes after the engine_promote swap (KN-P14, M5) — the transport is unchanged. KN-D1 is written to
    re-run green across that boundary. Name it.
- **CONTRACTS TO IMPLEMENT.** 3.5 the firehose resume-cursor transport (owned by Knowledge over the bus seam —
  the resume-cursor + idempotent-apply discipline is Knowledge's deliverable). 2.6 the *.snapshot resync fallback
  (consumed — the cold path). Implement to the frozen protocol shape; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D1 → kill a collab client mid-edit + sever the connection during a sustained multi-author edit; on
    resume(scope=doc:<id>, last_seq) assert 0 ops lost, 0 duplicate effects (the UNIQUE(op_id) idempotent apply);
    telemetry: op-log apply lag, op dedup hit-rate, resume-gap size, resync_required rate — all read, the
    0-lost/0-dup counters are the dated green artifact — CI. (Runs on the CAS transport now; the CRDT-boundary
    re-run is owed in KN-P14.)
- **TESTS (required).** Unit tests for op_seq monotonicity, the idempotent ON CONFLICT apply (a re-delivered op
  is a no-op), the resync_required → snapshot path, and scope-bound rejection. The KN-D1 drill scenario as a
  CHAINED test (multi-author edits → kill + sever → reconnect → resume → assert the full op set applied exactly
  once), not a single-handler test (the property is a sequence property). The CDC pair for row 3.5 (Knowledge's
  resume-cursor half). The transport idempotent-apply path is mandatory-core: state the cargo-mutants
  mutation-score floor.
- **DEFINITION OF DONE.** The transport relays + persists + resumes; an op re-send is a no-op; resync falls back
  to the snapshot; presence is ephemeral; KN-D1 emits its dated green artifact (0 lost, 0 duplicate across a kill +
  sever, measured); unit + the chained drill test + the 3.5 CDC pass; the contract-coverage scanner is green; the
  CAS-bytes floor is named with its KN-P14 CRDT follow-on; the work is committed. No threshold is weakened; the
  drill is run against a real kill, not asserted.
- **COMMIT.** Header: P-<NNN> M3: resume-cursor durable collab transport (KN-D1 headline). Body lists: contract
  3.5 implemented (the resume-cursor + idempotent apply over the firehose seam); KN-D1 greened (0 ops lost, 0
  duplicate across kill+sever, measured); the CAS-bytes transport floor named (KN-P14 promotes to Yrs over the
  unchanged transport); the mutation floor stated. Branch first if on default; do not push unless asked. End with
  the workspace Co-Authored-By trailer.

---

### KN-P5 — The editor primitives standalone (serializer, offset model, DOM-surgery) + the integrated editor (KN-D2 re-run)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3b (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3b", the editor
  primitives + the integrated editor; the block-tree storage is KN-P6).
- **DEPENDS-ON.** KN-P1 (myelin-content + the WASM render path frozen). KN-P4 (the transport the editor sends
  ops over). The M2 design-system prompt that ships the shared overlay/state primitives (the off-screen-picker /
  clipped-dialog / focus-leak foreclosure) the editor's menus/pickers consume (master §2 M0/M2). The Knowledge
  design sketches under design/ (IA, user flows, wireframes with empty/loading/error states) — VISION §3, no
  frontend code without a reviewed design sketch.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (top-of-the-line UX; design comes before frontend; the switch-test bar);
    ../../external-insights/05-ux-and-design.md §2 (the one-render-path editor mandate; controlled
    contenteditable not textarea; caret = char offset; Enter/IME/paste are the "this isn't a real editor" tells);
    ../../external-insights/01-process-and-quality-doctrine.md §4 (actually try it — drive the real editor in a
    browser before claiming it works).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md
    §8 (the one editor render path; §8.1 the shared Rust core compiled to WASM, one parser client+server; §8.2
    the three primitives shipped + unit-tested standalone BEFORE the integrated editor — the serializer, the
    offset model, the DOM-surgery for Enter-splits-block + caret-after-split; §8.3 why a markdown-subset string);
    ../04-subsystem-architectures/knowledge-platform/architecture/04-views-cli-and-api.md (the editor views/
    affordances); the design/ folder.
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 13.1 (the WASM render target the
    editor consumes — the editor reuses the same myelin-content WASM core, no second renderer).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3b" (the correctness-bar-regardless-
    of-engine thesis, KN-4).
  - Drills: testing-strategy/01-...-catalogue.md KN-D2 (render(parse(md))===md re-run over the INTEGRATED editor:
    100%, 0 regressions).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge's editor crate/package
  (the TypeScript/React shell consuming the myelin-content WASM core) + reusing the KN-P1 WASM artifact:
  - The three primitives, each shipped and unit-tested STANDALONE before the integrated editor (arch §8.2): (1)
    the serializer (inline AST ↔ markdown-subset string, with mention/artifact_ref/embed as structured U+FFFC
    nodes) — reuse the KN-P1 WASM core, do not write a second renderer; (2) the offset model (the caret = a char
    offset into the serialized markdown, bridged to/from DOM positions; a structured node is one caret position);
    (3) the DOM-surgery module (Enter-splits-a-block + caret-placement-after-split; controlled contenteditable
    intercepting structural input, plain text through, normalize on serialize; IME/paste handling — the named top
    risk).
  - The integrated single-doc editor over those primitives + the transport (KN-P4): create a page, type blocks,
    a second connection sees edits live. Consume the shared design-system overlay/state primitives for any menu/
    picker/dialog (no bespoke off-screen picker).
  - FLOOR named: no merge engine yet (KN-P7 is the CAS floor) and no permissions beyond tenant isolation
    (KN-P8); a single editor + a live second viewer is "first runnable" (roadmap §4). Name it.
- **CONTRACTS TO IMPLEMENT.** 13.1 the WASM render target (consumed — the editor runs the identical parser code,
  client + server). Implement to the frozen shape; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D2 re-run over the INTEGRATED editor: render(parse(md)) === md 100%, 0 regressions (the corpus-pass-rate
    signal = 100% on the integrated path, not just the library) — CI.
  - The switch-test driving evidence: the editor is driven in a browser (Enter/IME/paste exercised), recorded
    against the design sketches (EI-01 §4 — actually try it; the driven-in-a-browser note dated) — a recorded
    manual-drive artifact, honestly marked yes/partial.
- **TESTS (required).** Standalone unit tests for each of the three primitives (serializer round-trip, offset
  model bridging, Enter-split + caret placement, IME/paste). The KN-D2 corpus test over the integrated editor.
  The browser-drive evidence recorded (yes/no/partial). The serializer/offset model are mandatory-core: state the
  cargo-mutants mutation-score floor.
- **DEFINITION OF DONE.** The three primitives pass standalone; the integrated editor runs over the transport;
  KN-D2 re-emits its dated green (100%, 0 regressions) over the integrated path; the browser-drive is recorded
  (honestly marked); the design sketches are referenced (or produced + reviewed for any missing screen, VISION
  §3); the no-merge/no-perms floor is named; the work is committed. No gate is weakened.
- **COMMIT.** Header: P-<NNN> M3: editor primitives + integrated editor (KN-D2 re-run). Body lists: the three
  primitives shipped + standalone-tested; the integrated editor over the transport; KN-D2 greened on the
  integrated path (100%, 0 regressions); the browser-drive evidence recorded; the no-merge/no-perms floor named
  (KN-P7/KN-P8). Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By
  trailer.

---

### KN-P6 — The block tree (adjacency list + LexoRank), stable block ids, page hierarchy, version history + the sync_block read-projection floor

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3b (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3b", the block
  tree + page hierarchy + version history + the sync_block floor).
- **DEPENDS-ON.** KN-P2 (the frozen LexoRank order_key). KN-P3 (the OLTP store + the block table). KN-P5 (the
  editor that creates/moves blocks). The M2 Refs prompt that froze the #sub grammar b<id>/h<id> (5.7) so the
  stable block ids are the #sub targets.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale + name-your-floors); ../../external-insights/01-process-and-quality-doctrine.md
    §3 (prove-it).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/01-tech-and-data-model.md §1.2
    (the block tree is an adjacency list — parent_id + a fractional order_key; subtree reads are an index range;
    moves are an order_key write; recursive CTEs for deep walks) + the block-table + op-log + snapshot schema
    sections; 02-internals-and-algorithms.md §3.5 (the LexoRank jitter/rebalance as idempotent replayable move
    ops) + §7 (op-log compaction → content-addressed snapshot, the live tail kept).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 13.3 (the order_key the tree uses),
    5.7 (the #sub kinds b<opaqueid> / h<opaqueid> — the stable block id is the #sub target; block.block_id stable
    across edits/moves), 11.2 (the BlobStore for compacted snapshots, fs-backed floor).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3b" (the block tree + the sync_block
    read-projection floor Δ3).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge:
  - The block tree as per-block rows in an adjacency list (parent_id + the frozen LexoRank order_key from
    KN-P2); subtree reads as an index range; block moves as an order_key write; the rare deep subtree walk as a
    recursive CTE. Stable opaque block ids (block.block_id stable across edits/moves/collaboration — the #sub
    b<opaqueid> / h<opaqueid> targets, 5.7) so an embed of "block b9 of page 7c2" never dangles when the block is
    reordered.
  - Page hierarchy: sub-pages = folder-like nesting (a page is a block subtree root); the page_parent typed
    relation table (the TE-7 source of truth, mirrored to Refs in KN-P10).
  - Version history + snapshots: op-log compaction to a content-addressed snapshot in the (fs-backed floor)
    BlobStore, keeping the live op-log tail; op-log GC.
  - The sync_block read-projection FLOOR (Δ3): the sync_block node renders via Refs resolve(ref, viewer) (like
    embed), permission-filtered per viewer — NOT editable-in-place multi-home.
  - FLOOR named: sync_block = read-projection only (no shared-mutable node). Follow-on: editable-in-place
    multi-home synced blocks designed against the CRDT (most-restrictive-of-sites permission + reference-counted
    erasure via the edge index), KQ-6 — post-M5. Name it.
- **CONTRACTS TO IMPLEMENT.** 5.7 the stable block-id mint (owned — the b/h #sub targets; stability is
  Knowledge's obligation). 13.3 order_key (consumed — the tree ordering). 11.2 BlobStore (consumed — snapshots,
  fs floor). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - A stable-id property gate: a block reordered/edited keeps its block_id — an embed of b<id> resolves to the
    same block after a move (the moved-block-id-stability counter = 0 dangles) — CI.
  - A subtree-read range gate: a subtree read uses an index range (not a full scan) and a deep walk uses the
    recursive CTE — the query-plan check is the green artifact — CI.
  - (No new leak/loss drill here; the block tree feeds KN-D2/KN-D1 already greened. The KN-D2 corpus re-runs to
    confirm tree edits round-trip.)
- **TESTS (required).** Unit tests for adjacency-list insert/move (order_key bisection + jitter), recursive-CTE
  subtree walk, stable block-id survival across moves, op-log compaction → snapshot round-trip, and the
  sync_block read-projection resolve. The CDC pair for row 5.7 (the Knowledge stable-id mint). State the
  cargo-mutants mutation-score floor for the order_key tree-write module if mandatory-core; if not, say so.
- **DEFINITION OF DONE.** The block tree + page hierarchy + version history + snapshots exist; block ids are
  stable across moves (the stability gate green); subtree reads are index-served; the sync_block read-projection
  floor renders permission-filtered; KN-D2 still green over tree edits; unit tests + the 5.7 CDC pass; the
  contract-coverage scanner is green; the sync_block floor is named with its KQ-6 follow-on; the work is
  committed. No gate is weakened.
- **COMMIT.** Header: P-<NNN> M3: block tree (adjacency list + LexoRank) + stable ids + page hierarchy +
  sync_block floor. Body lists: contract 5.7 stable-id mint owned; 13.3 order_key consumed; the block-id
  stability gate greened (0 dangles across moves); the subtree-range gate greened; the sync_block read-projection
  floor named (KQ-6 editable follow-on, post-M5). Branch first if on default; do not push unless asked. End with
  the workspace Co-Authored-By trailer.

---

### KN-P7 — The per-block CAS merge floor (no silent overwrite) + soft-locks + offline reconcile (KN-D3, the named-floor proof)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3c (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3c", the CAS
  merge floor half; the ReBAC fragment + SetExpr is KN-P8).
- **DEPENDS-ON.** KN-P4 (the transport the ops ride), KN-P6 (the block tree with block.version). The M2
  agent-fabric prompt is not required for the CAS floor itself. The index keeps KN-P7 before KN-P8 in the band
  (the merge floor + the permission fragment are the M3-G3 split).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors — a floor that masquerades as done is the failure);
    ../../external-insights/04-hard-problems.md §2.1 (CRDT-after-CAS: the v1 floor guarantees no SILENT overwrite,
    does not merge; the loser reconciles); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it:
    0 silent overwrites is a quantified gate).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md
    §3.1 (Layer 2 authority — permission/schema/erasure checks on every op, above the merge layer), §3.2 (the CAS
    floor: per-block optimistic compare-and-swap on block.version; rows_affected==0 → Conflict{current}; the
    loser reconciles, never silently overwritten; different blocks edit freely in parallel; the conflict rate is
    the CRDT-promotion trigger metric).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 4.2 (check + CaveatContext on each
    op — Layer 2), 4.10 (the zookie read-your-writes so a just-revoked editor's op is rejected).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3c" (the CAS floor thesis; the
    advisory soft-locks + snapshot/restore; offline = read + queued light-edit reconciled via the CAS floor).
  - Drills: testing-strategy/01-...-catalogue.md KN-D3 (two clients edit the same block concurrently → the loser
    is rejected with current state, never silently overwritten; different blocks edit in parallel with no false
    conflict; 0 silent overwrites — the named-floor proof in the master M3→M4 gate).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge's collab/merge module
  (Layer 3a over the transport):
  - The per-block optimistic compare-and-swap: EDIT_BLOCK(block_id, expected_version, new_inline, new_props)
    runs UPDATE block SET ... version=version+1 WHERE tenant=? AND block_id=? AND version=expected_version; on
    rows_affected==0 return Conflict{current: server state} — the loser RECONCILES, never silently overwritten.
    Different blocks edit freely in parallel (the guard is per-block).
  - The Layer 2 authority checks that run on EVERY incoming op before merge (arch §3.1): the permission check
    (Id.check edit|comment with the zookie — read-your-writes; a just-revoked editor's op is rejected), schema
    validation (a db-row op must satisfy the FieldType defs), and the erased-content degrade. (The full ABAC
    list_objects push-down is KN-P8; the per-op check is here.)
  - Advisory soft-locks ("someone is editing this block," over the awareness channel) + snapshot/restore layered
    on the CAS guard. Offline = read + queued light-edit reconciled via the CAS floor (the deep offline-first
    answer arrives with the CRDT, KN-P14).
  - The CAS-conflict-rate metric (rows_affected==0 fraction) emitted to telemetry — it is the CRDT-promotion
    trigger metric (KQ-1) KN-P14 reads.
  - FLOOR named: CAS (no merge). Follow-on: the Yrs CRDT (KN-1, KN-P14, M5), triggered by the first true
    concurrent-edit conflict measured via the KN-D3 CAS-conflict-rate metric. Name it.
- **CONTRACTS TO IMPLEMENT.** 4.2 check + CaveatContext (consumed — the per-op Layer 2 check). 4.10 the zookie
  read-your-writes (consumed — the just-revoked op rejection). Implement to the frozen shapes; escalate a needed
  change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D3 → two clients edit the same block concurrently → the loser is rejected with current state (never
    silently overwritten); different blocks edit in parallel with no false conflict; 0 silent overwrites; the
    CAS-conflict-rate metric is emitted (the CRDT-promotion trigger) — CI. (This is the named-floor proof in the
    master M3→M4 gate.)
  - A just-revoked-editor gate: an op from an editor revoked at-or-after the zookie revision is rejected (the
    new-enemy guard) — 0 stale-grant writes — CI.
- **TESTS (required).** Unit tests for the CAS guard (winner commits, loser gets Conflict{current}), per-block
  independence (different blocks no false conflict), the soft-lock advisory, and the zookie revocation rejection.
  The KN-D3 drill as a CHAINED concurrent-edit scenario (two clients, same block, interleaved) — the property is
  a concurrency property, not a single handler. The CDC pair for row 4.2 (Knowledge's consumer half). The CAS
  guard is mandatory-core: state the cargo-mutants mutation-score floor (the no-silent-overwrite property must
  survive mutation).
- **DEFINITION OF DONE.** The CAS guard rejects the loser with current state; different blocks are independent;
  the Layer 2 per-op checks run; the zookie rejects a stale-grant op; KN-D3 emits its dated green (0 silent
  overwrites, the conflict-rate metric emitted); unit + the chained drill + the 4.2 CDC pass; the
  contract-coverage scanner is green; the CAS floor is named with its KN-P14 CRDT follow-on; the work is
  committed. No gate is weakened; the drill runs a real concurrent edit, not an asserted one.
- **COMMIT.** Header: P-<NNN> M3: per-block CAS merge floor + soft-locks + zookie guard (KN-D3 named-floor
  proof). Body lists: contract 4.2/4.10 consumed (per-op Layer 2 check + zookie read-your-writes); KN-D3 greened
  (0 silent overwrites, conflict-rate metric emitted); the CAS floor named (KN-P14 Yrs CRDT follow-on, trigger:
  first true concurrent conflict via the conflict-rate metric); the mutation floor stated. Branch first if on
  default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### KN-P8 — The Knowledge ReBAC page-tree fragment + the list_objects SetExpr push-down (KN-D5, zero leak incl. COUNT)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3c (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3c", the ReBAC
  fragment + the SetExpr push-down half).
- **DEPENDS-ON.** KN-P7 (the per-op check it generalises). KN-P6 (the page/block/db_row id columns the JOIN
  lowers over). The M1 Identity prompts that ship the ReBAC namespace engine (4.9), list_objects + the SetExpr
  push-down (4.3), check + CaveatContext (4.2), write_tuples/zookie (4.6/4.10), and the per-tenant authz reverse
  index (authz_visible). The M0 no-cross-db + tenant-predicate lints.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (one permission model; GDPR-safe by construction — a leak is a security AND a GDPR
    breach); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it: 0 leak incl. COUNT is the
    quantified gate; observability is the pass).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/01-tech-and-data-model.md §5
    (the page-tree ReBAC fragment: page.read = (parent_page->read + direct_reader) - direct_block; the row_reader
    userset; the view_field CaveatContext off the hot path); 02-internals-and-algorithms.md §4.1 (the frozen
    SetExpr lowering over db_row.id — the All/None/Ids/InRelation{row_reader, via_column}/Union table; the JOIN
    against authz_visible; closing the count-leak because the ACL conjunct is INSIDE the query) + §5
    (permission-filtered reads everywhere — never post-filter); 03-events-contracts-and-glue.md §3.2/§3.3 (the
    list_objects/check glue + the zookie new-enemy guard stamped on page.acl_zookie).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §1 (the frozen ReBAC
    fragments — Knowledge: page-tree inherit-with-overrides + row + field caveat), OQ-E (the SetExpr facet
    lowering, now the named authz_visible JOIN).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 4.9 (the per-subsystem ReBAC
    namespace fragment — the Knowledge fragment), 4.3 (list_objects + the SetExpr push-down), 4.2 (check +
    CaveatContext), 4.6/4.10 (write_tuples → zookie; the zookie consistency).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3c" + §2 (rows 4.9, 4.3, 4.2).
  - Drills: testing-strategy/01-...-catalogue.md KN-D5 (a confidential page / overridden sub-page / row-restricted
    db / field-hidden column never appears in any view/backlink/search/embed/RAG result for an unauthorized
    viewer — INCLUDING an aggregate COUNT; 0 leaked artifacts, 0 count-leak).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge:
  - The Knowledge ReBAC namespace fragment submitted into the one cell schema Identity compiles (4.9): page-tree
    inherit-with-overrides (page.read = (parent_page->read + direct_reader) - direct_block); row-level via the
    row_reader userset pushed down by InRelation{relation: row_reader, via_column: db_row.id}; field-level via the
    frozen CaveatContext{object, field, attrs} on view_field, evaluated at check-time OFF the hot path. The
    fragment must COMPILE in the cell schema.
  - The list_objects SetExpr push-down: Knowledge calls list_objects(viewer, read, 'page'|'database_row', zookie)
    and lowers the returned Filter{set_expr, zookie} into every list/board/view/search query via the frozen
    lowering over its own id column (the All/None/Ids/InRelation/Union table from arch §4.1) — the InRelation case
    is a JOIN against the per-tenant authz_visible reverse index. No N+1, no post-filter; the ACL conjunct is
    INSIDE the query so even a COUNT is permission-correct (the count-leak closed).
  - write_tuples → zookie on a page ACL change (knowledge.access.* events), stamped on page.acl_zookie (4.6/4.10);
    subsequent reads pass the zookie so a just-revoked grant cannot be read stale (the new-enemy guard; the authz
    index honours the zookie revision watermark).
  - FLOOR named: none — this is the full permission model for v1 (the page-tree fragment is complete). The
    field-level predicate catalogue per database is co-designed with Id's role-bundle catalogue (KQ-5, parallel,
    not a floor of this prompt) — note it.
- **CONTRACTS TO IMPLEMENT.** 4.9 the Knowledge ReBAC fragment (owned — compiled by Identity). 4.3 list_objects +
  the SetExpr lowering (consumed — Knowledge lowers the Filter into its SQL). 4.2 check + CaveatContext (consumed —
  field-level ABAC off the hot path). 4.6/4.10 write_tuples → zookie (consumed — the ACL-change path + the
  new-enemy guard). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D5 → a confidential page / overridden sub-page / row-restricted db / field-hidden column never appears in
    any view / backlink / search / embed / RAG result for an unauthorized viewer — INCLUDING an aggregate COUNT;
    0 leaked artifacts, 0 count-leak; the zero-escape counter = 0 is the dated green artifact — CI. (Re-confirmed
    in KN-P10 once search/embed/RAG paths exist — the count-leak path goes live there.)
  - The Knowledge ReBAC fragment COMPILES in the shared cell schema (a build-time gate) — CI.
  - The no-cross-db lint green (Knowledge never reads another owner's DB — it projects) — CI.
- **TESTS (required).** Unit tests for the page.read override formula (direct_block removes a narrowed sub-page
  from list_objects), the SetExpr lowering over each SetExpr variant (All → no conjunct, InRelation → the JOIN,
  None → WHERE false), and the field-level CaveatContext hiding. The KN-D5 drill scenario including the COUNT
  path (a permission-correct COUNT over a row-restricted db). The CDC pair for rows 4.9, 4.3. The SetExpr lowering
  is mandatory-core (a leak is catastrophic): state the cargo-mutants mutation-score floor (the no-leak property
  must survive mutation).
- **DEFINITION OF DONE.** The ReBAC fragment compiles in the cell schema; the SetExpr push-down conjoins the ACL
  inside every query (no post-filter); the zookie new-enemy guard holds; KN-D5 emits its dated green (0 leak, 0
  count-leak, measured); the no-cross-db lint green; unit + the KN-D5 drill (incl. COUNT) + the 4.9/4.3 CDC pass;
  the contract-coverage scanner is green; the KQ-5 field-predicate-catalogue note is written; the work is
  committed. No gate is weakened; the leak drill runs real unauthorized reads.
- **COMMIT.** Header: P-<NNN> M3: Knowledge ReBAC page-tree fragment + list_objects SetExpr push-down (KN-D5 0
  leak incl. COUNT). Body lists: contract 4.9 fragment compiled, 4.3 SetExpr lowered (ACL inside the query),
  4.2/4.10 field-ABAC + zookie guard wired; KN-D5 greened (0 leaked artifacts, 0 count-leak, measured); the
  no-cross-db lint green; the KQ-5 field-predicate-catalogue parallel work noted. Branch first if on default; do
  not push unless asked. End with the workspace Co-Authored-By trailer.

---

### KN-P9 — The flexible database (JSONB bag + GIN projection) + the read-time formula/rollup engine (KN-D9, KN-D10)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3d (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3d", the
  flexible-DB + read-time formula/rollup half; the refs/search and notif glue are KN-P10/KN-P11).
- **DEPENDS-ON.** KN-P2 (the frozen myelin-query FieldType/ViewSpec/QueryAst). KN-P8 (the SetExpr push-down the
  db query conjoins). KN-P6 (the db_row store). The M1 Storage prompt (the GIN/expression index substrate).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (Notion-class; world-scale); ../../external-insights/04-hard-problems.md §2.4 (rollups/
    formulas computed at read time, never stored — the Notion scaling pain avoided);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it: the p99-within-budget +
    measured-promotion-trigger gates).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/01-tech-and-data-model.md §1.2
    (JSONB property-bag rows source of truth + GIN/expression indexes + generated columns for measured-hot facets
    — the derived projection maintained off the bus; the >5% promotion threshold frozen 6.3/OQ-C);
    02-internals-and-algorithms.md §4.1 (VIEW_QUERY with the SetExpr conjoin; measured-hot facets → generated
    index, cold → bounded paginated GIN scan), §4.2 (the read-time formula/rollup engine — the bounded
    dependency-graph evaluator; rollups conjoin list_objects; cycle → #CYCLE; the FormulaAst = the bounded
    myelin-query expression core; the named materialised follow-on per measured-slow rollup).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 13.3 (FieldType/ViewSpec/QueryAst
    + rollup/formula computed at read time never stored), 4.3 (the SetExpr conjoin), 6.3 (the >5% facet-promotion
    threshold), 11.6 (the OLAP read store the materialised rollup follow-on feeds).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3d" (Floor 1 JSONB+GIN → per-facet
    generated index; Floor 2 read-time formula/rollup → per-rollup materialised aggregate).
  - Drills: testing-strategy/01-...-catalogue.md KN-D9 (filter/sort/group a large multi-tenant database → p99
    within budget; measure the >5% facet-promotion trigger), KN-D10 (a rollup over a large related set at read
    time → p99 within budget; measure when incremental materialisation is needed).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge's database module:
  - The Database Service: a JSONB property bag per row (db_row.props, the source of truth) + a derived,
    GIN-indexed projection (jsonb_path_ops) + generated columns for the measured-hot facets; typed field
    definitions (the frozen FieldType enum); views as ViewSpec query projections (table/board/calendar/timeline);
    two-way relations (db_relation, the TE-7 source of truth — the Refs mirror is KN-P10). The VIEW_QUERY path
    conjoins the SetExpr Filter into every db query (arch §4.1) — paginated, row-capped, statement-timeout.
  - The read-time formula/rollup engine (arch §4.2): formulas + rollups computed at READ TIME, never stored; a
    bounded dependency-graph evaluator over the FormulaAst (the bounded myelin-query expression core — no UDFs/
    loops/recursion, statically cost-bounded); rollups over a relation conjoin list_objects (permission-filtered);
    a cycle surfaces as #CYCLE (a diagnostic cell), never an infinite loop.
  - FLOOR 1 named: JSONB bag + GIN-indexed projection (read-time facets). Follow-on: per-facet generated/
    expression-column index promoted when a facet crosses the frozen >5% view-execution threshold (6.3/OQ-C,
    measured) — KN-P15 (M5). FLOOR 2 named: read-time formula/rollup. Follow-on: per-rollup incrementally-
    maintained materialised aggregate fed off the bus (knowledge.row.updated deltas → the OLAP read store 11.6)
    when read-time recompute is measured too slow (KQ-4) — KN-P15 (M5). Name both.
- **CONTRACTS TO IMPLEMENT.** 13.3 the FieldType/ViewSpec/QueryAst executor + the read-time formula/rollup
  (owned — Knowledge owns its executor; the definitions are the frozen shared shapes). 4.3 the SetExpr conjoin
  (consumed). 6.3 the >5% facet-promotion threshold (consumed — measured here, acted on in KN-P15). Implement to
  the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D9 → filter/sort/group a large multi-tenant database (JSONB + projection + the SetExpr conjoin) →
    read-time p99 within the budget in the thresholds file; the facet-execution-frequency telemetry measures the
    >5% promotion trigger (recorded, not yet acted on) — SCHED.
  - KN-D10 → a rollup over a large related set computed at read time (permission-filtered) → p99 within budget;
    the rollup-latency telemetry measures when incremental materialisation is needed (recorded) — SCHED.
  - A formula-cycle gate: a cyclic formula surfaces as #CYCLE, never an infinite loop (bounded-evaluation
    counter) — CI.
- **TESTS (required).** Unit tests for the VIEW_QUERY SetExpr lowering into the db query, the read-time formula
  evaluator (each RollupFn; the depth-bound + cycle detection → #CYCLE), and the generated-column facet path. The
  KN-D9/KN-D10 drill scenarios on the failure-injection harness at scale (a large multi-tenant db; a large
  related set). The formula evaluator is mandatory-core (cost-bounding): state the cargo-mutants mutation-score
  floor.
- **DEFINITION OF DONE.** The flexible DB serves views over the frozen shapes with the SetExpr conjoined; the
  read-time formula/rollup engine is bounded + cycle-safe (#CYCLE, never a loop); KN-D9 and KN-D10 emit their
  dated green (p99 within budget; the promotion triggers measured); unit + the two scale drills pass; the
  contract-coverage scanner is green; Floor 1 and Floor 2 are named with their KN-P15 follow-ons; the work is
  committed. No gate is weakened; the budgets are read from the thresholds file, never edited to pass.
- **COMMIT.** Header: P-<NNN> M3: flexible database (JSONB + GIN projection) + read-time formula/rollup engine
  (KN-D9/KN-D10). Body lists: contract 13.3 executor + read-time formula/rollup owned, 4.3 SetExpr conjoined, 6.3
  threshold measured; KN-D9 (db p99) and KN-D10 (rollup p99) greened with measured numbers + promotion triggers;
  Floor 1 (per-facet index) and Floor 2 (per-rollup materialisation) named (KN-P15 follow-ons). Branch first if on
  default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### KN-P10 — Refs glue (#sub mints + tombstone ladder + TE-7 typed-edge mirror + project) + Search feed (declare_indexable + query) (KN-D6)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3d (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3d", the refs +
  search glue; the notif/humanise/comments + export/import are KN-P11).
- **DEPENDS-ON.** KN-P6 (the block tree + page_parent/db_relation typed tables), KN-P8 (the SetExpr/project
  permission filtering), KN-P9 (the db rows the relations connect). The M2 Refs prompt (ArtifactRef 5.1, resolve
  5.2, backlinks/traverse 5.3, refs.edge.created 5.4, the TE-7 mirror 5.5, project 5.6, the #sub grammar +
  tombstone ladder 5.7). The M2 Search prompt (declare_indexable 6.3, query/semantic with the Filter conjoin
  6.1/6.2, the search-requires-acl-filter lint).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (one cross-artifact reference graph; not a silo); ../../external-insights/04-hard-problems.md
    §5 (reindex-from-source — Search/Refs are derived stores, rebuilt via the live consumer, never reading the
    owner DB); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it: cold==live).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/03-events-contracts-and-glue.md
    §2.1 (ArtifactRef + the #sub grammar b/h/row-/field-/comment-/thread- with stable opaque ids; the 4-step
    tombstone ladder LIVE/MOVED/OUTDATED/GONE/ERASED; a tombstone always carries the root), §2.2 (project(ref,
    viewer) — the frozen shape {title,state,icon,render_hint,sub_anchor?}; a confidential page → tombstone, never
    leaks), §2.3 (replay(scope) block-granular *.snapshot via the outbox — the only recovery path), §3.1 (the
    TE-7 typed-edge mirror: db_relation → knowledge.relation.* → Refs lifecycle edge; page_parent →
    knowledge.page.parent_set → Refs parent edge; the typed table is truth); 02-internals-and-algorithms.md §6
    (search indexing granularity — page + significant-block, vector-in-v1, multilingual).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 5.1/5.2/5.3/5.4/5.5/5.6/5.7
    (the refs glue), 6.3 (declare_indexable — the page + block IndexSpec, vector-in-v1, JSONB struct), 6.1/6.2
    (query/semantic with the list_objects Filter conjoined), 2.6 (replay).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-4 (the #sub grammar
    frozen — the field- node is new; h has no hyphen).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3d" + §2 (rows 5.x, 6.x, 2.6).
  - Drills: testing-strategy/01-...-catalogue.md KN-D6 (wipe Knowledge's derived state — the Refs edge projection
    / Search index — replay(scope) block-granular → rebuilt state matches live; rebuild uses the live consumer
    path only; cold == live). KN-D5 re-confirmed now that search/embed/RAG paths exist (the count-leak path is
    live here).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge:
  - Refs glue: the three inline nodes (mention/artifact_ref/embed) emit refs.edge.created on persist (5.4, NOT
    coalesced); implement resolve/backlinks/traverse (5.2/5.3); the #sub grammar b/h/row-/field-/comment-/thread-
    with stable-id mint (5.7) + the 4-step tombstone ladder (permission → root → sub LIVE/MOVED/OUTDATED/GONE →
    ERASED; a tombstone always carries the root). The TE-7 typed-edge mirror (5.5): the same transaction that
    writes a page_parent / db_relation typed row emits knowledge.page.parent_set / knowledge.relation.* so Refs
    projects the lifecycle edge (the typed table is truth; Refs holds the rebuildable projection). project(ref,
    viewer) (5.6) — the frozen {title,state,icon,render_hint,sub_anchor?} shape, per-viewer permission-checked,
    a confidential page → tombstone.
  - replay(scope, since) (2.6): emit knowledge.page.snapshot (block-granular) / knowledge.row.snapshot /
    refs.edge.snapshot via the OUTBOX through the live bus — the only recovery path; deterministic snapshot
    event_id from (aggregate, version). The TE-7 drift-correction (a scoped replay reconverges Refs to the typed
    table).
  - Search glue: declare_indexable(IndexSpec) — two specs (a page doc title+body language-tagged; a
    per-significant-block doc) + vector-in-v1 + JSONB struct fields (6.3); feed project to the index; query/
    semantic with the list_objects Filter conjoined (6.1/6.2, the search-requires-acl-filter lint). Knowledge
    never indexes itself — it projects text, Search consumes off the bus (no cross-DB).
  - FLOOR named: none new (the tombstone ladder + project are complete; the >5% search-block prune is KQ-10,
    measured, parallel). Note KQ-10.
- **CONTRACTS TO IMPLEMENT.** 5.6 project (owned — the Knowledge impl), 5.7 the #sub stable-id mint (owned),
  5.5 the TE-7 typed-edge mirror (owned source of truth), 5.2/5.3/5.4 resolve/backlinks/traverse + edge events
  (consumed/produced), 2.6 replay (owned), 6.3 declare_indexable (owned — the Knowledge projection spec), 6.1/6.2
  query/semantic (consumed — the Filter conjoin). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D6 → wipe Knowledge's derived state (Refs edge projection / Search index); replay(scope) (block-granular
    *.snapshot) → rebuilt state matches live; rebuild uses the live consumer path only; the reindex-parity-hash
    telemetry = live is the dated green (cold == live) — SCHED.
  - KN-D5 re-confirmed with the search/embed/RAG paths live: 0 leak incl. COUNT across search/embed/backlink —
    CI (the search-requires-acl-filter lint green is part of the artifact).
- **TESTS (required).** Unit tests for the #sub mints (grammatical sub-URNs), the 4-step tombstone ladder (each
  of LIVE/MOVED/OUTDATED/GONE/ERASED returns the right tombstone carrying the root), project (a confidential page
  → tombstone), the TE-7 mirror (typed row → emitted edge event), and the declare_indexable spec serialization.
  The KN-D6 drill (wipe + replay → parity hash). The KN-D5 re-confirmation over search/embed. The CDC pairs for
  rows 5.6, 5.7, 5.5, 2.6, 6.3. The replay path is mandatory-core (cold==live is a recovery property): state the
  cargo-mutants mutation-score floor.
- **DEFINITION OF DONE.** The refs glue (edges, resolve, tombstone ladder, TE-7 mirror, project) + replay + the
  search feed exist; KN-D6 emits its dated green (cold == live, parity hash); KN-D5 is re-confirmed over search/
  embed/RAG (0 leak, lint green); unit + the KN-D6 drill + the CDC pairs pass; the contract-coverage scanner is
  green; KQ-10 (block-vs-page index size, measured) is noted; the work is committed. No gate is weakened; the
  reindex drill rebuilds via the live consumer only.
- **COMMIT.** Header: P-<NNN> M3: refs glue (#sub + tombstone ladder + TE-7 mirror + project) + search feed
  (KN-D6). Body lists: contracts 5.6/5.7/5.5/5.2/5.3/5.4/2.6/6.3/6.1/6.2 wired; KN-D6 greened (cold==live, parity
  hash); KN-D5 re-confirmed over search/embed (0 leak); the search-requires-acl-filter lint green; KQ-10 noted.
  Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### KN-P11 — Notif/humanise glue + watcher rules + KB-native comment threads + the Export/Import service (Art. 20 portability)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3d (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3d", the
  notif/humanise + comments + export/import glue).
- **DEPENDS-ON.** KN-P10 (project Display mode + the #sub comment/thread mints + the edge events the notif rules
  fire on). KN-P2 (the ADF lossy-map for import; myelin-content for export). The M2 Notif prompt (humanise 7.3 —
  the ONE templating surface; define_notif_rule 7.6; the watcher relation). The M2 Refs prompt (the shared
  comment/thread #sub grammar). The shared design-system thread primitive (Δ9/OQ-L).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (one notification inbox; GDPR portability is an architectural constraint —
    Art. 20 export); ../../external-insights/01-process-and-quality-doctrine.md §7 (abstract at the third copy —
    one templating surface, not a second engine; one comment scheme shared with Chat).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/03-events-contracts-and-glue.md
    §1.5 (comments/mentions events → Notif; a comment is a sub-artifact #comment-/#thread- — the same grammar as
    Chat, OQ-L: one scheme, two stores), §2.2 (project Display mode = the humanisation projection Notif uses — a
    routable ArtifactRef + a humanised string, the sole humanise surface, no second template engine);
    06-reconciliation-compliance.md (the Export service = the Art. 20 mechanism); 04-views-cli-and-api.md (the
    comment/export affordances).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.3 (humanise — the ONE ICU
    templating surface), 7.6 (define_notif_rule + the watcher relation), 5.7 (the #comment-/#thread- #sub
    grammar), 13.2 (the ADF → myelin-content import map), 10.1 (export(subject) — the lossless JSON the GDPR
    holder calls in KN-P12 reuses this Export service).
  - Reconciliation: 00-reconciliation-decisions.md OQ-L (the one humanise surface; the comments one-scheme-two-
    stores with Chat).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3d" (the KB-native comment store
    Floor 4; the Export/Import service) + §2 (rows 7.3, 7.6).
  - Drills: testing-strategy/01-...-catalogue.md — Knowledge inherits the Notif drills (NOTIF-D4 humanised
    tombstone never leaks a title) via project Display mode; the round-trip of export/import is covered by KN-D2
    for content fidelity.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge:
  - Notif glue: declare the watcher relation + define_notif_rule for mentions/comments/shares/watched
    (knowledge.mention.created / knowledge.comment.created / knowledge.access.granted / a watched-page change,
    7.6); feed project Display mode into the ONE humanise ICU-MessageFormat surface (7.3) so "alice mentioned you
    in <Incident runbook>" renders per-viewer — register NO second template engine; living-doc/SLA-style strings
    register here.
  - KB-native comment threads over the shared #thread-/#comment- #sub grammar (5.7) + the myelin-content AST + the
    shared design-system thread primitive (Δ9/OQ-L): inline comments anchored to a block/range, emitting
    knowledge.comment.created/.resolved.
  - The Export/Import service: the lossless JSON export (Art. 20 portability — the mechanism the GDPR
    export(subject) holder reuses in KN-P12), Markdown/HTML/PDF, CSV; the ADF → myelin-content lossy-map import
    (13.2) with the import report recording each lossy conversion.
  - FLOOR 4 named: KB-native comment store (one scheme, two stores with Chat). Follow-on: consolidation onto the
    Chat threading primitive + the firehose transport on the real-time-presence trigger (KQ-9) — post-M5; a merge,
    not a rewrite (they already share #sub + content + refs). Name it.
- **CONTRACTS TO IMPLEMENT.** 7.3 humanise (consumed — Knowledge registers its Display projection into the one
  surface), 7.6 define_notif_rule + watcher (owned — the Knowledge rules + relation), 5.7 the comment/thread #sub
  mints (owned), 13.2 the ADF import map (consumed — the import path), 10.1 export(subject) mechanism (owned — the
  Export service the holder reuses). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The humanise integration gate: a Knowledge mention/comment renders through the ONE humanise surface per-viewer
    and a confidential subject degrades to a humanised tombstone with the title never leaking (the NOTIF-D4 class,
    inherited) — CI; the no-second-template-engine check is the green artifact.
  - An export/import round-trip gate: a doc exported to lossless JSON and re-imported is byte-faithful for the
    content model (render(parse(md))===md holds across export/import); the ADF import records each lossy node in
    the import report — CI.
- **TESTS (required).** Unit tests for the watcher-rule firing, the humanise Display projection (per-viewer
  rendering + the confidential tombstone), the comment-thread #sub anchoring, the lossless JSON export round-trip,
  and the ADF lossy-map import report. The CDC pairs for rows 7.6, 7.3, 13.2. State the cargo-mutants
  mutation-score floor for the export round-trip module if mandatory-core; if not, say so.
- **DEFINITION OF DONE.** The notif/humanise glue routes through the one surface (no second template engine); the
  KB-native comment threads use the shared #sub grammar; the Export/Import service round-trips losslessly and
  records ADF lossy conversions; the humanise + export/import gates emit their dated green; unit tests + the
  CDC pairs pass; the contract-coverage scanner is green; Floor 4 (KB comments → Chat consolidation) is named with
  its KQ-9 follow-on; the work is committed. No gate is weakened.
- **COMMIT.** Header: P-<NNN> M3: notif/humanise glue + KB comment threads + Export/Import (Art. 20). Body lists:
  contracts 7.6/7.3/5.7/13.2/10.1(export) wired; the humanise-tombstone gate + the export/import round-trip gate
  greened; the one-humanise-surface / no-second-template-engine discipline held; Floor 4 (KB comments) named
  (KQ-9 Chat-threading consolidation, post-M5). Branch first if on default; do not push unless asked. End with the
  workspace Co-Authored-By trailer.

---

### KN-P12 — The PersonalDataHolder + per-subject DEK crypto-shred (the hardest GDPR surface) (KN-D4)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3e (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3e", the GDPR
  holder + per-subject DEK crypto-shred half; the AG-7 trace + agent governance is KN-P13).
- **DEPENDS-ON.** KN-P3 (the holder auto-registers when the store opens), KN-P6 (the op-log/snapshots the
  crypto-shred reaches), KN-P10 (the backlinks tombstoned on erase; the embeddings purged via the search feed),
  KN-P11 (the Export service the holder's export reuses). The M1 GDPR prompts (PersonalDataHolder trait 10.1, the
  #[personal_data] classify-derive 10.2, the erasure ledger 10.8, the ONE platform erasure posture 10.9). The M1
  Storage/KMS prompts (the per-subject DEK hierarchy 11.3/11.4 — the crypto-shred substrate). The M2 Bus prompt
  (the *.erased tombstone 2.7).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe & EU-sovereign by construction — data subject rights are architectural);
    ../../external-insights/04-hard-problems.md §1 (erasure-vs-immutability — you cannot delete a CAS/CRDT op; you
    destroy its key; the structural floor + the documented residual);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it: 0 recoverable PII incl. vectors incl.
    backups is the quantified gate).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/03-events-contracts-and-glue.md
    §6 (the full PersonalDataHolder: locate/export/rectify/restrict/erase; §6.1 the erasure algorithm — the
    structural floor: pseudonym-map shred + per-subject DEK crypto-shred (one DEK per (subject, tenant), CR-I,
    O(subjects with inline PII) not O(blocks)) + structural tombstoning + embeddings purged; the residual by
    reference to 10.9, NOT restated); 05-hard-problems.md §6 (erasure depth); 06-reconciliation-compliance.md §8
    (the residual posture).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 10.1 (PersonalDataHolder{locate/
    export/rectify/restrict/erase}), 10.2 (#[personal_data] tags + the no-untagged-personal-data lint), 10.9 (the
    ONE platform erasure posture — instantiated by reference), 11.4 (the per-subject DEK crypto-shred), 4.8 (the
    pseudonym map shred), 2.7 (the *.erased tombstone).
  - Reconciliation: 00-reconciliation-decisions.md X-7 (the pseudonymous-by-default + the one erasure posture).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3e" + §2 (rows 10.1/10.2/10.9/11.4).
  - Drills: testing-strategy/01-...-catalogue.md KN-D4 (erase a subject → structured PII purged/pseudonymised,
    free-text under a per-subject DEK crypto-shredded — unrecoverable in op-log/snapshots/backups, embeddings
    purged, backlinks tombstoned; 0 recoverable structured PII incl. vectors; residual per 10.9).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge's gdpr module:
  - The PersonalDataHolder{locate, export, rectify, restrict, erase} impl (10.1) across blocks, rows, history,
    mentions, authorship: locate (structured PII reliably + free-text best-effort via Search, flagged); export
    (reuse the KN-P11 Export service — lossless JSON, Art. 20); rectify (a structured value + best-effort
    free-text span tombstone); restrict (exclude the subject from indexing/agent-use(RAG)/analytics/notifications
    — stop emitting to Search/Agents/OLAP/Notif, mark rows/blocks restricted, the restriction flowing into OLAP
    per 11.6); erase (the structural floor §6.1).
  - The #[personal_data(category, role, basis, retention, erasure, subject_locator)] classify-derive tags (10.2)
    on the Knowledge schema (person fields, mention nodes, author/edit attribution, free-text body, trace
    authorship) so the no-untagged-personal-data lint is green.
  - The erasure algorithm structural floor (§6.1): (1) pseudonym-map shred (attribution is the opaque
    principal_id, never PII; erasing the <pseudonym>@<tenant>.noreply map makes the id un-resolvable, 4.8); (2)
    per-subject DEK crypto-shred (free-text blocks/ops holding the subject's PII are envelope-encrypted under a
    per-subject DEK class subject:<id>, 11.4; erasure destroys the key → the ciphertext in op-log, snapshots, AND
    backups is unrecoverable; ONE DEK per (subject, tenant), applied only to PII-bearing classes, CR-I); (3)
    structural tombstoning (mentions/backlinks tombstone via the *.erased consumer; the Search + vector index
    purges in lockstep — embeddings of personal data are personal data; published pages unpublish + CDN/cache
    purge); returns a receipt hash-linked into the audit log (10.8).
  - FLOOR named: the structural floor is fully built + reliable for structured/self-authored PII. RESIDUAL:
    third-party free-text PII (a name typed by someone else into that other person's content) is under the
    author's DEK, handled per the ONE platform posture (10.9, [OPEN — LEGAL], KQ-8 — counsel/DPO ratify in one
    statement; never indexed/agent-readable/in-analytics for a restricted subject; the structural floor ships
    regardless). Instantiate by reference, do NOT restate. Name it.
- **CONTRACTS TO IMPLEMENT.** 10.1 PersonalDataHolder (owned — the Knowledge holder), 10.2 the #[personal_data]
  tags (consumed — applied to Knowledge types), 11.4 the per-subject DEK crypto-shred (consumed — the key
  destroy), 4.8 the pseudonym-map shred (consumed), 2.7 the *.erased tombstone (consumed/produced), 10.9 the
  posture (consumed by reference). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D4 → erase a subject → structured PII purged/pseudonymised, free-text under the per-subject DEK
    crypto-shredded (key destroyed → unrecoverable in op-log/snapshots/backups), embeddings purged, backlinks
    tombstoned; 0 recoverable structured PII incl. vectors; the residual covered by the platform posture 10.9;
    telemetry: holder erase receipts, vector-tombstone lag, key-shred count (bounded: one key per subject) — the
    0-recoverable counter is the dated green artifact — SCHED.
  - The no-untagged-personal-data lint green on the Knowledge schema (0 untagged PII fields; red on a deliberately
    untagged fixture) — CI.
- **TESTS (required).** Unit tests for each holder op (locate/export/rectify/restrict/erase), the per-subject DEK
  envelope-encrypt → key-destroy → ciphertext-unrecoverable path, the pseudonym-map shred, and the backlink/
  embedding tombstone-in-lockstep. The KN-D4 drill as a CHAINED scenario (subject authors PII → erase → assert
  unrecoverable across op-log/snapshots/backups + vectors purged). The CDC pair for row 10.1. The crypto-shred
  path is mandatory-core (unrecoverability is the property): state the cargo-mutants mutation-score floor.
- **DEFINITION OF DONE.** The holder implements all five ops; the structural erasure floor crypto-shreds
  free-text under the per-subject DEK (unrecoverable in backups), pseudonymises attribution, tombstones
  backlinks, purges embeddings; KN-D4 emits its dated green (0 recoverable PII incl. vectors, measured); the
  no-untagged-personal-data lint green; the residual is handled by reference to 10.9 (not restated); unit + the
  chained KN-D4 drill + the 10.1 CDC pass; the contract-coverage scanner is green; the residual ([OPEN — LEGAL],
  KQ-8) is named; the work is committed. No gate is weakened; the erasure drill verifies real backups.
- **COMMIT.** Header: P-<NNN> M3: PersonalDataHolder + per-subject DEK crypto-shred (KN-D4). Body lists: contract
  10.1 holder owned, 10.2 tags applied, 11.4 per-subject DEK crypto-shred wired, 4.8 pseudonym shred, 2.7 *.erased;
  KN-D4 greened (0 recoverable structured PII incl. vectors, measured); the no-untagged-personal-data lint green;
  the free-text residual named by reference to 10.9 ([OPEN — LEGAL], KQ-8); the structural floor ships regardless.
  Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### KN-P13 — The AG-7 content-addressed agent-trace holder + agent governance (EffectApi, HITL, reserve/settle) (KN-D11, KN-D12)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3e (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3e", the AG-7
  agent-trace holder + agent governance half — completes the master M3→M4 exit for Knowledge).
- **DEPENDS-ON.** KN-P4 (agent edits flow through the SAME collab protocol as humans), KN-P12 (the holder/
  crypto-shred the trace holder reuses). The M2 agent-fabric prompts (ToolSurface::register_tool with the frozen
  requires_approval defaults 8.1, EffectApi::apply plan-then-apply 8.2, AgentRuntime::step --use-mock 8.3,
  ToolHands::exec the unified sandbox 8.4, the content-addressed agent-trace holder seam 8.8) with AG-D4
  (sandbox-escape) GREEN. The M2 durable-workflow prompts (SCHEDULE_AND_RUN_JOB 9.2, the durable HITL signal +
  per-effect idem_key 9.1/9.4, the timer wheel 9.3). The M1 Storage prompt (reserve/settle 11.7).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native from the ground up; agents are first-class citizens; mock agents only during
    development — the strategy pattern, --use-mock); ../../external-insights/03-agent-native-fabric.md (the four
    uniform guarantees; plan-then-apply; HITL withhold); ../../external-insights/01-process-and-quality-doctrine.md
    §3 (prove-it: 0 ungoverned mutation / 0 mutation before approval / 0 double-apply), §8 (human sign-off is the
    bottleneck — publish/confidential edits are decision-shaped, HITL-gated).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/03-events-contracts-and-glue.md
    §5.1 (the ToolDef registrations + the frozen requires_approval defaults — publish/confidential=yes, draft/
    comment=no; side-effecting tools go through EffectApi::apply; HITL withhold returns Denied + does not mutate;
    the approval card resumes via a durable signal with the per-effect idem_key rule), §5.2 (the AG-7
    content-addressed agent-trace holder — accept a content-addressed trace write reusing the block model, an
    erasable holder, distinct from the audit log); §7 (reserve/settle — Knowledge is not spend-bearing; an agent
    write passes the Fabric's reserve/settle gate); 02-internals-and-algorithms.md §9 (agent edits flow through
    the same collab protocol; the four uniform sandbox guarantees by construction).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 8.1 (ToolDef + the frozen
    requires_approval defaults), 8.2 (EffectApi::apply), 8.4 (the unified sandbox, AG-D4 drilled in M2), 8.8 (the
    AG-7 content-addressed agent-trace holder), 9.1/9.4 (the per-effect idem_key + durable HITL signal), 9.2
    (SCHEDULE_AND_RUN_JOB), 11.7 (reserve/settle), 10.1 (the trace holder is an erasable PersonalDataHolder).
  - Reconciliation: 00-reconciliation-decisions.md X-6 (the four uniform guarantees + the frozen approval
    defaults), OQ-F (the per-effect idem_key — a double-click is one approval, a partial approval is well-defined).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3e" + §2 (rows 8.1/8.2/8.8/9.x/11.7).
  - Drills: testing-strategy/01-...-catalogue.md KN-D11 (an agent edits a doc via EffectApi → attributed
    "suggested by agent"; a consequential edit publish/confidential is HITL-withheld (returns Denied, no mutation)
    until approval; a double-click is one approval; denied effects return ordinary tool errors; the run passed
    reserve/settle; 0 ungoverned mutation, 0 mutation before approval, 0 double-apply), KN-D12 (erase a subject →
    content-addressed agent traces crypto-shredded/purged, attribution falls back to the pseudonym; 0 recoverable
    PII in traces, attribution intact).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge's agent module:
  - Register Knowledge ToolDefs into the one catalogue (8.1) with the frozen requires_approval defaults
    (knowledge.search/page.read/summarise = read, no; page.create/append/comment/draft = mutate, no; row.upsert =
    no, but YES for a PII-bearing database; page.publish / edit(confidential_page) = mutate, YES; page.turn_into_issues
    = YES, inherits Issues' default). Side-effecting tools go through EffectApi::apply (8.2, plan-then-apply:
    schema→capability→delegation→tenant→budget→HITL→apply→meter) → the Knowledge public endpoint as the agent
    principal → the collab protocol (KN-P4) with "suggested by agent" attribution. The four uniform sandbox
    guarantees hold by construction (8.4 — AG-D4 already green from M2; any compute the tool runs is the CI
    runner's kind=agent job).
  - HITL withhold (8.2/AG-8): a gated tool not in the approved set returns Denied and does NOT mutate; the
    approval card surfaces in Chat (live cost estimate) and resumes the run via a durable signal — the per-effect
    idem_key rule (card_id single, card_id:<effect_idx> for a batch/partial approval, 9.1/9.4, OQ-F) makes a
    double-click one approval and a partial approval well-defined. Denied = ordinary tool error (no privileged
    fallback). Scheduled living-doc automations as SCHEDULE_AND_RUN_JOB jobs (9.2); reserve/settle on every agent
    run (11.7 — the Fabric's bookends; Knowledge tools are ordinary metered effects).
  - The AG-7 content-addressed agent-trace holder (8.8): write_agent_trace(run_id, content, actor) accepts a
    content-addressed (BLAKE3) trace write reusing the block model (no new schema), returns run.trace_ref, and
    registers it as an erasable PersonalDataHolder (distinct from the tamper-evident audit log) — erasing a
    subject crypto-shreds their trace content, attribution falls back to the pseudonym.
  - FLOOR named: none — agent governance is the full v1 surface. The mock runtime (--use-mock) is the platform
    floor (the real LlmAgentRuntime is the post-M5 config/impl swap, owned by the Fabric, not Knowledge) — note
    it.
- **CONTRACTS TO IMPLEMENT.** 8.1 the Knowledge ToolDefs + frozen approval defaults (owned), 8.2 EffectApi::apply
  (consumed — the apply path), 8.8 the AG-7 trace holder (owned — a Knowledge deliverable), 9.1/9.4 the durable
  HITL signal + idem_key (consumed), 9.2 SCHEDULE_AND_RUN_JOB (consumed), 11.7 reserve/settle (consumed), 10.1
  the trace as an erasable holder (owned). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D11 → an agent edit via EffectApi is attributed "suggested by agent"; a publish/confidential edit is
    HITL-withheld (returns Denied, no mutation) until approval; a double-click is one approval (per-effect
    idem_key); denied effects are ordinary tool errors; the run passed reserve/settle; 0 ungoverned mutation, 0
    mutation before approval, 0 double-apply; the gate-state + denial-counter + idem-key-dedup telemetry is the
    dated green artifact — CI.
  - KN-D12 → erase a subject → their content-addressed agent traces crypto-shredded/purged, attribution falls
    back to the pseudonym; 0 recoverable PII in traces, attribution intact — SCHED.
- **TESTS (required).** Unit tests for the ToolDef registration (the frozen approval defaults), the EffectApi
  apply path (attribution via the collab protocol), the HITL withhold (Denied, no mutation), the per-effect
  idem_key (double-click = one apply; partial approval well-defined), reserve/settle gating, and the AG-7 trace
  write + holder registration. The KN-D11 drill as a CHAINED scenario (agent plans → consequential effect →
  withheld → approve → applied once across a double-click). The KN-D12 trace-erasure drill. The CDC pairs for rows
  8.1, 8.2, 8.8. The HITL/idem_key path is mandatory-core: state the cargo-mutants mutation-score floor.
- **DEFINITION OF DONE.** The Knowledge ToolDefs register with the frozen defaults; agent edits flow through
  EffectApi → the collab protocol with attribution; the HITL withhold + per-effect idem_key + reserve/settle hold;
  the AG-7 trace holder accepts content-addressed writes and is erasable; KN-D11 and KN-D12 emit their dated green
  (0 ungoverned/0 pre-approval/0 double-apply; 0 recoverable trace PII); unit + the chained drills + the CDC pairs
  pass; the contract-coverage scanner is green; the mock-runtime floor is noted; the work is committed. This
  completes the master M3→M4 exit for Knowledge (with KN-D3/KN-D1/KN-D2/KN-D7/KN-D5/KN-D13 already green). No gate
  is weakened.
- **COMMIT.** Header: P-<NNN> M3: AG-7 agent-trace holder + agent governance (KN-D11/KN-D12). Body lists:
  contracts 8.1/8.2/8.8/9.1/9.4/9.2/11.7/10.1 wired; KN-D11 greened (0 ungoverned mutation, 0 mutation before
  approval, 0 double-apply) and KN-D12 greened (0 recoverable trace PII, attribution intact); the four uniform
  guarantees held by construction; the master M3→M4 Knowledge exit complete; the mock-runtime floor noted. Branch
  first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### KN-P14 — The Yrs CRDT promotion over the unchanged transport + cross-cell collab (KN-D1 re-green across engine_promote)

- **BAND.** M5.
- **ROADMAP MILESTONE.** KN-M5 (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M5", the Yrs CRDT
  promotion + cross-cell collab half; the materialisation + surge + E2E legs are KN-P15).
- **DEPENDS-ON.** KN-P4 (the resume-cursor transport the CRDT slots into), KN-P7 (the CAS floor + the
  conflict-rate trigger metric), KN-P6 (the block tree + LexoRank the move-CRDT replaces). M4 green (all five
  subsystems exist; the deterministic correctness drills green). The M5 control-plane multi-cell prompt (the
  cross-cell PII-free CrossCellPointer bridge 12.6 going live) for cross-cell collab.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors: the CRDT is the named M5 promotion; world-scale);
    ../../external-insights/04-hard-problems.md §2 (CRDT-after-CAS — the trigger is the first true concurrent-edit
    conflict; the CRDT slots into the transport without touching the data model);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it: KN-D1 re-runs green across the
    engine_promote boundary — the floor's promotion is itself drilled).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md
    §3.3 (the Yrs CRDT: a per-block content CRDT Y.Text/Y.XmlFragment + a tree/move CRDT Kleppmann move op for
    block structure — the hybrid granularity; rich-text marks → Peritext; the op-log becomes Yrs updates, the
    transport unchanged), §3.4 (the online CAS→CRDT migration per-doc, no stop-the-world — quiesce-lite →
    deterministic Yrs seed → the single engine_promote cutover op → reconcile in-flight CAS edits; reversible from
    the pre-cutover snapshot), §3.5 (LexoRank under the CRDT — the move-CRDT's list type owns sibling ordering;
    order_key becomes a derived hint).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 3.5 (the transport the CRDT slots
    into — unchanged), 12.6 (the cross-cell PII-free pointer bridge — frame frozen, live in M5).
  - Reconciliation: 00-reconciliation-decisions.md OQ-I (multi-cell after single-cell — cross-cell op fan-out
    over the bridge).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M5" (the floor follow-ons; KQ-1 the
    CRDT promotion timing; KQ-7 cross-cell) + §5 (the floors → follow-ons table).
  - Drills: testing-strategy/01-...-catalogue.md KN-D1 re-green across the engine_promote boundary (0 lost/0 dup
    survives the swap).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge's collab/merge module
  (the Layer-3 swap):
  - The Yrs CRDT engine slotting into the M2/M3a resume-cursor transport as a Layer-3 swap (the op-log carries Yrs
    update bytes; the transport, resume cursor, idempotent apply, and op-log are unchanged): a per-block content
    CRDT (Y.Text/Y.XmlFragment) for inline runs + a tree/move CRDT (Kleppmann move op) for block structure (the
    hybrid granularity); rich-text marks per Peritext. The server stays a "dumb relay + persistence + authority"
    (it does not transform — Yrs being Rust-native keeps it in-process).
  - The online CAS→CRDT migration per-doc, no stop-the-world (arch §3.4): quiesce-lite snapshot → deterministic
    Yrs seed from the snapshot → a single engine_promote op at the next op_seq (from there forward Yrs bytes,
    before it CAS deltas) → in-flight CAS edits straddling the cutover reconcile via the last CAS conflict check;
    reversible from the pre-cutover snapshot. Trigger: the first true concurrent-edit conflict, measured via the
    KN-P7 CAS-conflict-rate metric (KQ-1). Full offline-first arrives here.
  - The LexoRank-under-CRDT interaction (arch §3.5): the move-CRDT's list type owns sibling ordering; order_key
    becomes a derived OLTP-index hint recomputed from CRDT state; the bespoke jitter/rebalance retires.
  - Cross-cell collab (KQ-7/OQ-I/12.6): true cross-cell op fan-out for a multi-cell tenant over the PII-free
    CrossCellPointer bridge (owned by control-plane; the contracts are cell-agnostic so this extends without a
    rewrite). v1 pinned a doc's session to one cell; resolution stays cell-local.
  - FLOOR resolved: this is the named CRDT follow-on to the KN-P7 CAS floor and the KN-P14 cross-cell follow-on to
    the single-cell floor. Editable-in-place synced blocks (KQ-6) is enabled by the CRDT — pull into here or name
    post-M5. State which.
- **CONTRACTS TO IMPLEMENT.** 3.5 the transport (consumed — unchanged; the CRDT rides it), 12.6 the cross-cell
  pointer bridge (consumed — cross-cell fan-out). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D1 re-green across the CRDT engine_promote boundary: kill + sever across a per-doc CAS→CRDT cutover →
    still 0 ops lost, 0 duplicate (the floor's promotion is itself drilled; the transport survived the swap) —
    CI/SCHED.
  - A CRDT-convergence gate: concurrent edits to the same block from N clients converge to one state (no blend
    lost, no divergence) — the CRDT's convergence property, measured — CI.
- **TESTS (required).** Unit tests for the deterministic Yrs seed (reproducible from a snapshot), the
  engine_promote cutover (op_seq continuity across CAS→Yrs), in-flight-CAS reconcile at the boundary, and the
  move-CRDT block-structure ops. The KN-D1 re-green drill run ACROSS an engine_promote boundary (chained: edits →
  promote → kill+sever → resume → 0 lost/0 dup). A convergence test (N concurrent same-block edits converge). The
  CRDT merge + the engine_promote cutover are mandatory-core: state the cargo-mutants mutation-score floor.
- **DEFINITION OF DONE.** The Yrs CRDT slots into the unchanged transport; the online per-doc engine_promote
  migration is deterministic + reversible; concurrent same-block edits converge; cross-cell op fan-out works over
  the bridge; KN-D1 re-greens across the engine_promote boundary (0 lost/0 dup, dated); the convergence gate is
  green; unit + the across-boundary drill + the convergence test pass; the contract-coverage scanner is green; the
  CAS→CRDT and single-cell→cross-cell floors are resolved (KQ-6 named pulled-in or post-M5); the work is committed.
  No gate is weakened; the re-green drill runs a real promotion + kill.
- **COMMIT.** Header: P-<NNN> M5: Yrs CRDT promotion + cross-cell collab (KN-D1 re-green across engine_promote).
  Body lists: the CRDT (per-block content + tree/move) slotted into the unchanged transport; the online per-doc
  engine_promote migration; cross-cell fan-out over the 12.6 bridge; KN-D1 re-greened across the boundary (0
  lost/0 dup); the CAS→CRDT + single-cell→cross-cell floors resolved; KQ-6 disposition stated; the mutation floor
  stated. Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### KN-P15 — Facet/rollup materialisation + object-store blob swap + the hot-doc surge + the E2E-1/E2E-3 legs (KN-D8, KN-D9/D10 at scale)

- **BAND.** M5.
- **ROADMAP MILESTONE.** KN-M5 (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M5", the
  materialisation + the surge + the E2E wedge legs half).
- **DEPENDS-ON.** KN-P9 (the JSONB+GIN floor + the read-time formula/rollup the materialisation promotes; the
  measured triggers), KN-P3 (the fs-backed BlobStore floor the object-store swap replaces), KN-P14 (the CRDT under
  the surge). The M5 Storage prompt (the object-store BlobStore 11.2). The M5 whole-system E2E wedge prompt
  (E2E-1 PR context pane, E2E-3 spec-to-ship traceability) — Knowledge supplies its legs.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale from day 1); ../../external-insights/01-process-and-quality-doctrine.md §3
    (prove-it: the surge shed budget + the E2E chained-mutation scenarios; observability is the pass), §7 (the
    measured-promotion trigger — abstract at the third copy / materialise when measured).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md
    §4.1 (the measured-hot facet → generated index promotion) + §4.2 (the per-rollup materialised aggregate fed
    off the bus → the OLAP read store 11.6), §3.5 (the concurrent-same-gap LexoRank insert storm — no
    key-collision reorder, bounded rebalance); 05-hard-problems.md (the hot-doc thundering-herd discipline);
    the design sketches under design/ (the E2E-1 PR-context embed; the E2E-3 spec-to-ship lineage).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 6.3 (the >5% facet-promotion
    threshold — acted on here), 11.6 (the OLAP read store the rollup materialisation feeds), 11.2 (the object-store
    BlobStore — the one-line swap from the fs floor), 1.11 (the protected-human-lane shed order — viewers shed
    before editors, agents before humans).
  - Reconciliation: 00-reconciliation-decisions.md OQ-K (the per-surface storm profiles / the shed budget).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M5" (the floor follow-ons + the
    all-hands-doc surge + the E2E legs) + §5 (the floors → follow-ons table).
  - Drills: testing-strategy/01-...-catalogue.md KN-D8 (an all-hands doc with thousands of concurrent
    readers/editors → per-doc op cap + read-fanout bound + active-editor lane reservation hold within budget;
    other tenants unaffected; the concurrent-same-gap LexoRank insert storm → 0 reorder), KN-D9/KN-D10
    re-confirmed at world scale (the promotion triggers measured + acted on), E2E-1 (Knowledge design-doc embed
    resolves per-viewer, 0 leak) + E2E-3 (a Knowledge spec doc → initiative → issues lineage; cold-reindex ==
    live; audit tamper detected).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge:
  - Per-facet materialisation: promote a facet past the frozen >5% view-execution threshold (6.3, measured in
    KN-P9) to a generated/expression-column index via the expand→backfill→contract online-migration path
    (knowledge.database.schema.changed drives the feeder). Per-rollup materialisation: promote a rollup measured
    too slow (KN-P9's KN-D10 telemetry) to an incrementally-maintained materialised aggregate fed off the bus
    (knowledge.row.updated deltas → the OLAP read store 11.6) — per-rollup, not wholesale.
  - The object-store BlobStore swap (11.2): move media + CRDT snapshots from the fs-backed floor to the
    S3-compatible object store (the one-line swap KN-P3 named), residency-pinned, content-addressed (BLAKE3).
  - The all-hands-doc surge controls (KD-8 / OQ-K / 1.11): a per-doc op in-flight cap + a read-fanout bound + an
    active-editor lane reservation (viewers shed before editors, agents shed before humans) so the op fan-out
    holds within budget and other tenants are unaffected; the concurrent-same-gap LexoRank insert storm handled
    (no key-collision reorder, bounded rebalance — now under the move-CRDT from KN-P14).
  - Knowledge's legs of the whole-system E2E wedge: E2E-1 (a Knowledge design-doc embed in the PR context pane
    resolves per-viewer, 0 leak) and E2E-3 (a Knowledge spec doc → initiative → issues lineage; cold-reindex ==
    live; audit tamper detected).
  - FLOOR resolved: this ships the named per-facet + per-rollup materialisation follow-ons (KN-P9 Floor 1/2) and
    the object-store follow-on (KN-P3 floor). Name them resolved.
- **CONTRACTS TO IMPLEMENT.** 6.3 the facet-promotion (consumed — acted on), 11.6 the OLAP read store (consumed —
  the rollup materialisation feeder), 11.2 the object-store BlobStore (consumed — the swap), 1.11 the shed order
  (consumed — the surge lane reservation). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D8 → an all-hands doc with thousands of concurrent readers/editors → the per-doc op cap + read-fanout
    bound + active-editor lane reservation hold within budget; other tenants unaffected; the concurrent-same-gap
    LexoRank insert storm → 0 reorder; the per-tenant in-flight + op-fanout + rebalance-cost telemetry is the
    dated green — SCHED.
  - KN-D9 / KN-D10 re-confirmed at world scale with the materialisation acted on (p99 within budget after
    promotion) — SCHED.
  - E2E-1 and E2E-3 green for Knowledge's legs (0 leak to the unauthorized viewer; lineage live == cold; the
    tombstone carries root) — SCHED.
  - The F6 surge family leg for Knowledge (human lane holds, agent lane sheds 429+Retry-After, cross-tenant
    impact 0) — SCHED.
- **TESTS (required).** Unit tests for the facet-promotion expand→backfill→contract path, the incremental rollup
  aggregate maintained off the bus, and the object-store BlobStore swap (content-addressed put/get parity with
  the fs floor). The KN-D8 surge drill on the failure-injection harness at 30× with the LexoRank storm. The
  KN-D9/D10 at-scale re-runs. The E2E-1/E2E-3 chained-mutation scenarios against a full cell with mock agents.
  The surge lane-reservation + the incremental-rollup maintenance are mandatory-core: state the cargo-mutants
  mutation-score floor.
- **DEFINITION OF DONE.** The per-facet + per-rollup materialisation are promoted where measured; the object-store
  BlobStore swap is live (parity with the fs floor); the all-hands-doc surge holds within budget (0 reorder, other
  tenants unaffected); KN-D8, KN-D9/D10-at-scale, the F6 leg, and E2E-1/E2E-3 emit their dated green; unit + the
  surge + the E2E drills pass; the contract-coverage scanner is green; the materialisation + object-store floors
  are named resolved; the work is committed. No gate is weakened; the surge runs a real 30× storm.
- **COMMIT.** Header: P-<NNN> M5: facet/rollup materialisation + object-store blob + hot-doc surge + E2E-1/E2E-3
  legs (KN-D8). Body lists: the per-facet generated index + the per-rollup materialised aggregate (KN-P9 floors
  resolved); the object-store BlobStore swap (KN-P3 floor resolved); KN-D8 greened (surge within budget, 0
  reorder); KN-D9/D10 re-confirmed at scale; the F6 Knowledge leg greened; E2E-1/E2E-3 Knowledge legs greened;
  the mutation floors stated. Branch first if on default; do not push unless asked. End with the workspace
  Co-Authored-By trailer.

---

### KN-P16 — Dogfooding: Myelin's own docs in Knowledge + the switch test driven in a browser

- **BAND.** M6.
- **ROADMAP MILESTONE.** KN-M6 (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M6", dogfooding +
  the switch test).
- **DEPENDS-ON.** KN-P15 (M5 green — you do not dogfood real team knowledge onto a substrate whose restore-verify
  and DSAR fan-out KN-D4/KN-D6 are not green). The M6 dogfood prompts (Myelin hosts its own roadmap/gap-report/
  scorecard; the self-hosting CI graph green). The truth-up pass (the gate invariant — no red earlier gate).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (top-of-the-line UX; the switch test is the done-bar);
    ../../external-insights/01-process-and-quality-doctrine.md §4 (actually try it — the switch test is reached by
    DRIVING the real UI in a browser, not by reading the feature list; the "switch test": a Notion user could move
    without hitting a wall the old tool didn't have), §1 (the truth-up pass — every PROVEN row rests on a dated
    green KN-D artifact, code-wins-over-docs).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/00-overview.md (the
    switch-test/done-bar framing); the design sketches under design/ (the real anchor the contrast + latency
    budgets measure against).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M6" + §4 (production-hardened — the
    end-state the dogfood drives) + the master §2 M6 band.
  - Drills: testing-strategy/01-...-catalogue.md — the Knowledge switch test (driven in a browser; measured
    contrast + latency budgets + render(parse(md))===md against the real anchor).
- **DELIVERABLE (what to build + exactly where in the repo).** In the running Myelin platform (the dogfood loop)
  + the Knowledge design records:
  - Migrate Myelin's own roadmap, gap report, and scorecard into a Myelin Knowledge space (the team documents
    itself in its own Knowledge platform); the every-incident-adds-a-drill loop files a Myelin issue + a
    reproducing drill, documented in Knowledge.
  - Drive the real Knowledge UI of the editor + databases + permissions + backlinks/embeds + search + comments in
    a browser for the switch test (EI-01 §4): a Notion user could move to Myelin without hitting a wall the old
    tool didn't have — measured contrast + latency budgets + render(parse(md))===md against the real anchor (the
    design sketches). Record the verdict (reached by driving, honestly yes/no/partial per surface).
  - The truth-up pass for Knowledge: confirm every Knowledge PROVEN row rests on a dated green KN-D artifact (no
    claim outlives its verification); fix any doc that disagrees with the code (code-wins-over-docs).
  - FLOOR named: none — this is the done-bar. Any surface that fails the switch test is recorded as a named gap
    with a follow-on, never silently passed.
- **CONTRACTS TO IMPLEMENT.** None new — this exercises the already-implemented contracts end-to-end via the
  real UI. (The dogfood loop consumes Git/CI/Issues/Chat contracts owned elsewhere.)
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The Knowledge switch test passes driven in a browser: measured contrast + latency budgets met,
    render(parse(md))===md against the real anchor, the editor/db/permissions/backlinks/search surfaces usable
    without a wall the old tool didn't have — the dated switch-test verdict is the green artifact — SCHED.
  - The truth-up pass: every Knowledge PROVEN row rests on a dated green KN-D artifact; 0 red earlier Knowledge
    gates — SCHED.
- **TESTS (required).** The browser-driven switch-test session recorded (per surface, honestly yes/no/partial,
  with the measured contrast + latency numbers). The truth-up checklist over KN-D1..KN-D13 (each row → its dated
  green artifact). No new unit/CDC code unless a switch-test wall surfaces a fix (then that fix gets its own
  test + a drill per the every-incident-adds-a-drill loop). State that no mutation floor applies (this is a
  driving + truth-up prompt) unless a code fix lands.
- **DEFINITION OF DONE.** Myelin's own docs live in Knowledge; the switch test is driven in a browser and passes
  (measured contrast + latency + round-trip against the real anchor), with any wall recorded as a named gap +
  follow-on; the truth-up pass confirms every Knowledge PROVEN row rests on a dated green KN-D artifact (0 red
  earlier gates); the work is committed. No gate is weakened; the switch-test verdict is reached by driving, not
  by reading the feature list.
- **COMMIT.** Header: P-<NNN> M6: Knowledge dogfood + switch test (driven in a browser). Body lists: Myelin's own
  roadmap/gap-report/scorecard in a Knowledge space; the switch-test verdict (measured contrast + latency +
  render(parse(md))===md, per-surface yes/no/partial); any wall recorded as a named gap + follow-on; the truth-up
  pass result (every PROVEN row on a dated green KN-D artifact, 0 red earlier gates). Branch first if on default;
  do not push unless asked. End with the workspace Co-Authored-By trailer.
