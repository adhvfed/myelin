# Phase 7 — Prompt Ledger: Knowledge Platform (the producer subsystem, Notion-class)

> Granularity note (Phase 7-A finer-pass): prompt count 16 (first pass) → 34 (this finer-grained set). Every
> first-pass prompt that bundled more than one independently-committable deliverable has been split into
> single-deliverable, clean-context prompts; the genuinely atomic ones (the myelin-content freeze, the
> resume-cursor transport headline, the dogfood) stay atomic. The union of these 34 prompts covers every
> milestone, contract, drill (KN-D1..KN-D13 + the E2E wedge legs), and named floor the first pass covered —
> nothing dropped, the bundles now exposed as their own gateable units.
>
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
> co-owns myelin-query + order_key, X-2/X-3), its world-scale follow-ons (the Yrs CRDT promotion, cross-cell
> collab, facet/rollup materialisation) in M5, and the dogfood switch test in M6. The headline drill KN-D1
> (reconnect-loses-zero-ops) is Knowledge's deliverable over the M2 firehose resume-cursor transport (contract
> 3.5) — the build-order law (KN-1, EI-04 §2.2) makes that transport item 0.
>
> Coverage map (milestone → finer prompts):
> - KN-M2 → KN-P01 (myelin-content freeze + WASM) + KN-P02 (myelin-query + ADF map) + KN-P03 (order_key/LexoRank).
> - KN-M3a → KN-P04 (service shell) + KN-P05 (OLTP store + partition) + KN-P06 (outbox + taxonomy) + KN-P07
>   (resume-cursor transport).
> - KN-M3b → KN-P08 (editor primitives standalone) + KN-P09 (integrated editor) + KN-P10 (block tree + stable ids
>   + page hierarchy) + KN-P11 (version history + snapshots) + KN-P12 (sync_block read-projection floor).
> - KN-M3c → KN-P13 (CAS merge floor + soft-locks + offline) + KN-P14 (Layer-2 per-op authority + zookie guard) +
>   KN-P15 (ReBAC page-tree fragment) + KN-P16 (list_objects SetExpr push-down + zookie write).
> - KN-M3d → KN-P17 (flexible DB) + KN-P18 (read-time formula/rollup) + KN-P19 (refs glue + TE-7 mirror) + KN-P20
>   (replay/reindex) + KN-P21 (search feed) + KN-P22 (notif/humanise) + KN-P23 (KB comment threads) + KN-P24
>   (Export/Import).
> - KN-M3e → KN-P25 (PersonalDataHolder ops + tags) + KN-P26 (per-subject DEK crypto-shred) + KN-P27 (agent
>   governance) + KN-P28 (AG-7 agent-trace holder).
> - KN-M5 → KN-P29 (Yrs CRDT promotion) + KN-P30 (cross-cell collab) + KN-P31 (facet/rollup materialisation +
>   object-store blob) + KN-P32 (hot-doc surge) + KN-P33 (E2E-1/E2E-3 legs).
> - KN-M6 → KN-P34 (dogfood + switch test).
> Thirty-four prompts, no milestone gap; every KN-D and E2E leg greened by at least one prompt; every floor paired
> with its follow-on prompt.

---

### KN-P01 — Freeze myelin-content (the v1 block + inline taxonomy) and compile the WASM render path (KN-D2)

- **BAND.** M2.
- **ROADMAP MILESTONE.** KN-M2 (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M2", the
  myelin-content freeze + the WASM render-path half; the myelin-query/order_key freeze is KN-P02/KN-P03).
- **DEPENDS-ON.** The M0 substrate prompts that lay down the Cargo workspace + the eight glue-crate skeletons
  (including the myelin-content crate skeleton) + the twelve lints + the contract-coverage scanner (master §2
  M0; substrate roadmap SUB-M0). The index places this in the M2 reactive-layer band; no Knowledge runtime
  dependency yet, but myelin-content must freeze before Chat/Issues consume the subset, so this is among the
  earliest M2 prompts.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (top-of-the-line UX; design comes before frontend; name-your-floors);
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
    frozen + the WASM compile target + render(parse(md))===md). Lint row 1.6 referenced for the
    no-untagged-personal-data discipline the inline free-text classification will later satisfy.
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M2" + §2 (the row 13.1 obligation:
    LEADS + FREEZES).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    KN-D2 (render(parse(md))===md, 100% round-trip, 0 corpus regressions).
- **DELIVERABLE (what to build + exactly where in the repo).** In the shared glue crate myelin-content (the M0
  skeleton crate under the Cargo workspace):
  - The frozen v1 Block enum byte-for-byte from arch 01 §2.1 (paragraph/heading/bullet_list/ordered_list/
    task_list/blockquote/code_block/callout/table/divider/image/embed/db_view/toggle/sync_block). code_block.text
    is raw (NOT markdown-parsed). sync_block exists as a node here (the taxonomy is complete); its engine is the
    floor shipped in KN-P12 — name that here.
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
    floor landing in KN-P12 (follow-on: editable-in-place multi-home, post-M5, KQ-6). State in the crate doc that
    no Knowledge feature ships here — only the shared shape Chat/Issues/Search compile against.
- **CONTRACTS TO IMPLEMENT.** 13.1 myelin-content taxonomy + the WASM render target (owned/frozen — Knowledge
  is the freeze authority; Chat/Issues consume subsets, they do not redefine). Implement to the frozen shape; a
  needed shape change is a whole-workspace contract PR, escalated and written down, not a local divergence
  (code-wins-over-docs).
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D2 → render(parse(md)) === md over the frozen corpus: 100% round-trip, 0 regressions; the corpus-pass-rate
    telemetry signal = 100% is the dated green artifact — CI. (KN-D2 is owed in M2 because the WASM render path
    freezes here; it re-runs over the integrated editor in KN-P09.)
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
  floor is named with its KN-P12 follow-on; the no-feature freeze note is written; the work is committed. No gate
  is greened by weakening a threshold or shrinking the corpus.
- **COMMIT.** Header: P-<NNN> M2: freeze myelin-content taxonomy + WASM render path (KN-D2). Body lists: contract
  13.1 frozen; KN-D2 greened (render(parse(md))===md = 100%, 0 regressions, measured); the mutation floor for the
  parser stated; the sync_block read-projection floor named (KN-P12 ships it, post-M5 editable follow-on KQ-6).
  Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### KN-P02 — Freeze myelin-query (FieldType/ViewSpec/QueryAst = the EventMatcher core) + the ADF lossy-map (13.2)

- **BAND.** M2.
- **ROADMAP MILESTONE.** KN-M2 (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M2", the
  myelin-query type/grammar freeze + the ADF-map freeze; the order_key/LexoRank encoding is KN-P03).
- **DEPENDS-ON.** KN-P01 (the content crate exists alongside the query crate; the ADF map targets myelin-content
  nodes). The M0 substrate prompts (the myelin-query glue-crate skeleton). The Bus M2 prompt that froze the
  EventMatcher core (QueryAst = 3.4). The Issues M2 prompt that co-authors the myelin-query freeze
  byte-identical (the X-3 reconciliation at the plan layer) — the index pairs this with that Issues prompt so the
  shared type/grammar shape is authored once and both sides build to it.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors); ../../external-insights/01-process-and-quality-doctrine.md §7
    (reconcile cross-component contracts at the plan layer before either side ships — a unit/encoding mismatch
    that ships on one side calcifies; the X-3 anti-drift directive).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/01-tech-and-data-model.md §1.2
    (JSONB + derived projection, not per-tenant DDL — the why) + the data-model sections naming FieldType/
    ViewSpec; 02-internals-and-algorithms.md §4.1 (the SetExpr push-down the executor lowers, built in KN-P16/
    KN-P17 — referenced here only so the QueryAst shape supports it).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-3 (myelin-query +
    order_key byte-identical with Issues), OQ-C (the QueryAst = EventMatcher core, bounded, no UDFs/loops/
    recursion), X-2 (the ADF map feeds Issues import).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 13.3 (the myelin-query primitive
    frozen byte-identical — the field-type enum, ViewSpec, the QueryAst grammar = the EventMatcher core 3.4; the
    order_key half is implemented in KN-P03), 13.2 (the ADF → myelin-content lossy-map for the Issues import;
    lossy nodes named + recorded), 3.4 (the EventMatcher = the frozen QueryAst — the same core).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M2" + §2 (rows 13.3 co-owns, 13.2).
  - Drills: testing-strategy/01-...-catalogue.md — the QueryAst cost-bound fixture (no construct admits unbounded
    cost).
- **DELIVERABLE (what to build + exactly where in the repo).** In the shared glue crate myelin-query (the M0
  skeleton) + the myelin-content crate's ADF-map module:
  - The frozen FieldType enum, the ViewSpec view-model, and the QueryAst grammar (the bounded interpreter core =
    the bus EventMatcher 3.4: no UDFs, no loops, no recursion, statically cost-bounded, permission-aware shape).
    These are the definitions both Knowledge and Issues compile their own executors against (each owns its
    executor; the definitions are byte-identical). The order_key field is referenced by ViewSpec ordering but its
    encoding lands in KN-P03.
  - The ADF → myelin-content lossy-map table (13.2): the conversion table for the Issues import, with lossy
    nodes named and an import-report shape that records each lossy conversion.
  - FLOOR named: none — this is a freeze. Note that the executor lowering (the SetExpr conjoin + the read-time
    formula/rollup) is built later (KN-P16/KN-P17/KN-P18); here only the shared definitions freeze.
- **CONTRACTS TO IMPLEMENT.** 13.3 myelin-query FieldType/ViewSpec/QueryAst (co-owned/frozen with Issues —
  Knowledge ships the shared definitions; the order_key encoding is KN-P03). 13.2 the ADF lossy-map (Knowledge
  ships the table; Issues consumes it at import). 3.4 referenced (QueryAst = EventMatcher; Knowledge does not
  redefine it). Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The QueryAst is statically cost-bounded (a fixture asserting no construct admits unbounded cost; a red
    fixture of an unbounded expression is rejected) — CI.
  - The FieldType/ViewSpec/QueryAst serialization is stable (a golden-serialization fixture both Knowledge and
    Issues build to; 0 shape divergences) — CI.
- **TESTS (required).** Unit tests for FieldType/ViewSpec/QueryAst serialization + the QueryAst cost-bounding
  (red+green fixtures). Unit tests for the ADF lossy-map (each lossy node recorded in the import report). The CDC
  pairs for rows 13.3 (the type/grammar half) and 13.2. The QueryAst cost-bounding is mandatory-core: state the
  cargo-mutants mutation-score floor for the cost-bound module.
- **DEFINITION OF DONE.** The frozen FieldType/ViewSpec/QueryAst + the ADF map compile; the QueryAst cost-bound
  holds (red+green fixtures); the golden-serialization fixture is stable; unit tests + the 13.3/13.2 CDC pairs
  pass; the contract-coverage scanner is green; the no-feature freeze note is written; the work is committed. No
  threshold is weakened.
- **COMMIT.** Header: P-<NNN> M2: freeze myelin-query (FieldType/ViewSpec/QueryAst) + ADF lossy-map. Body lists:
  contract 13.3 type/grammar half frozen byte-identical with Issues; 13.2 ADF map shipped; the QueryAst
  cost-bound greened; the mutation floor stated. Branch first if on default; do not push unless asked. End with
  the workspace Co-Authored-By trailer.

---

### KN-P03 — Freeze the order_key/LexoRank fractional-index encoding + the X-3 conformance vector (byte-identical with Issues)

- **BAND.** M2.
- **ROADMAP MILESTONE.** KN-M2 (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M2", the order_key
  encoding + the X-3 LexoRank conformance vector).
- **DEPENDS-ON.** KN-P02 (myelin-query frozen; ViewSpec references order_key). The Issues M2 prompt that co-owns
  order_key byte-identical (the X-3 reconciliation) — the index pairs this with that Issues prompt so the shared
  conformance vector is authored once and both sides build to it.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors); ../../external-insights/01-process-and-quality-doctrine.md §7 (the X-3
    anti-drift directive — a unit/encoding mismatch that ships on one side calcifies), §3 (prove-it: byte-for-byte
    rank parity is the quantified gate).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md
    §3.5 (the LexoRank encoding under concurrency — the midpoint bisection, the 2-char jitter for concurrent
    same-gap inserts, the 48-char rebalance).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-3 (order_key
    byte-identical with Issues; the LexoRank conformance vector).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 13.3 (the order_key/LexoRank
    fractional-index encoding: base-62 0-9A-Za-z, lexicographic compare, "U" first, midpoint bisection, 2-char
    jitter, 48-char rebalance, created_at+ULID tiebreak — the co-owned/frozen encoding).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M2" + §2 (row 13.3 co-owns).
  - Drills: testing-strategy/01-...-catalogue.md — the X-3 LexoRank conformance vector (byte-for-byte rank parity
    with Issues).
- **DELIVERABLE (what to build + exactly where in the repo).** In the shared glue crate myelin-query, the
  order_key module:
  - The order_key/LexoRank encoding exactly to the 13.3 spec: base-62 0-9A-Za-z, "U" first, midpoint bisection,
    2-char jitter for concurrent same-gap inserts, 48-char rebalance trigger, created_at+ULID tiebreak. Public
    operations: rank_between(a, b), rank_first(), rank_last(), needs_rebalance(rank).
  - A shared conformance-vector fixture (a deterministic sequence of rank operations + their expected base-62
    outputs, incl. a concurrent same-gap collision exercising the jitter and a 48-char rebalance) committed in
    the crate and consumed identically by Issues — the X-3 anti-drift artifact.
  - FLOOR named: none — this is a freeze; the CRDT lands over the order model without changing it (KN-P29), so
    say the order_key stays the OLTP ordering encoding now and becomes a CRDT-derived hint post-promotion.
- **CONTRACTS TO IMPLEMENT.** 13.3 order_key/LexoRank (co-owned/frozen with Issues — Knowledge ships the shared
  encoding + its half of the conformance vector). Implement to the frozen shape; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The LexoRank conformance vector passes byte-for-byte identically on the Knowledge side and the Issues side
    (the X-3 anti-drift check): 0 rank divergences across the shared vector; the shared-vector parity signal = 0
    divergences is the dated green artifact — CI.
- **TESTS (required).** Unit tests for the order_key operations (insert-between, jitter under collision, rebalance
  at 48 chars, created_at+ULID tiebreak). The shared LexoRank conformance-vector test run on both sides (the
  build fails if the two outputs differ). The CDC pair for row 13.3 (the order_key half). order_key is
  mandatory-core: state the cargo-mutants mutation-score floor for the rank-encoding module.
- **DEFINITION OF DONE.** The order_key encoding compiles; the LexoRank conformance vector is byte-identical on
  both sides (0 divergences, dated); unit tests + the 13.3 (order_key half) CDC pair pass; the contract-coverage
  scanner is green; the no-feature freeze note is written; the work is committed. No threshold is weakened; the
  parity is real, not asserted.
- **COMMIT.** Header: P-<NNN> M2: freeze order_key/LexoRank encoding + X-3 conformance vector. Body lists:
  contract 13.3 order_key frozen byte-identical with Issues; the LexoRank conformance vector greened (0
  divergences, measured); the mutation floor stated. Branch first if on default; do not push unless asked. End
  with the workspace Co-Authored-By trailer.

---

### KN-P04 — The Knowledge service shell over serve(AppSpec) (boot → three surfaces → drain; hot-table flags)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3a (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3a", the service
  shell half; the OLTP store/partition is KN-P05, the outbox is KN-P06, the transport is KN-P07).
- **DEPENDS-ON.** KN-P01, KN-P02, KN-P03 (the content + query crates frozen). The M1 Identity prompts that ship
  authenticate (4.1) + check (4.2). The M0 substrate prompts (serve(AppSpec) 1.1, the three-surface 1.2,
  liveness≠readiness 1.3, the forward-only online-migration runner 1.5).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale + multi-tenant from day 1); ../../external-insights/01-process-and-quality-
    doctrine.md §5 (the ratchet — the lints are committed gates).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/01-tech-and-data-model.md §1
    (the Rust + Postgres choice; the per-service DB, the no-cross-db boundary); 03-events-contracts-and-glue.md
    §4 (the service is a thin shell over the harness, not a hand-rolled main).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 1.1/1.2/1.3 (serve(AppSpec) +
    three surfaces + liveness≠readiness), 1.5 (forward-only online migrations + the hot-table flags block/db_row/
    doc_op), 4.1/4.2 (authenticate/check on the entrypoints — call sites; full ABAC filtering lands in KN-P16).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3a" + §2 (rows 1.1/1.5/4.1/4.2) + §4
    (first runnable: the service boots over serve(AppSpec)).
  - Drills: testing-strategy/01-...-catalogue.md — the harness boot gate (boot → ready → drain; liveness≠readiness).
- **DELIVERABLE (what to build + exactly where in the repo).** In a new subsystem implementation crate
  myelin-knowledge under the workspace:
  - The Knowledge service as an AppSpec over serve (1.1) — boot → migrate → outbox relay (the relay is wired in
    KN-P06; declare the hook here) → consumers (the EventHandler set wired in KN-P06) → the three ports (public/
    internal/metrics-health, 1.2) → graceful drain; liveness≠readiness (1.3). Not a hand-rolled main.
  - Declare the hot-table flags block / db_row / doc_op to the migration runner (1.5, forward-only online
    migrations) — the high-write tables Knowledge will create in KN-P05.
  - authenticate/check (4.1/4.2) wired on the read/write entrypoints as call sites (the per-op Layer-2 check is
    KN-P14; full ABAC list_objects push-down is KN-P16).
  - FLOOR named: the store + partition land in KN-P05 and the outbox in KN-P06 — this prompt ships the boot
    skeleton only; name those two as the immediate follow-ons so the shell is not mistaken for the full service.
- **CONTRACTS TO IMPLEMENT.** 1.1/1.2/1.3 serve(AppSpec) + three-surface + liveness≠readiness (consumed —
  Knowledge boots from the harness). 1.5 hot-table flags + forward-only migrations (owned — declared). 4.1/4.2
  authenticate/check (consumed — call sites). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The service boots over serve(AppSpec): boot → ready transition is gated on migrations applied; liveness and
    readiness are distinct (liveness up during a drain, readiness false); graceful drain completes within budget
    — the boot/ready/drain telemetry is the dated green artifact — CI.
  - The forward-only-migration lint green on the Knowledge migrations (0 backward/destructive migrations) — CI.
- **TESTS (required).** Unit tests for the AppSpec wiring (the three ports bound, liveness≠readiness, the drain
  order), and the hot-table-flag declaration. The CDC pair for rows 1.1/1.5 (Knowledge's consumed/owned halves).
  No mutation floor (this is the boot skeleton; state so) unless the drain logic is core — if so, state the floor.
- **DEFINITION OF DONE.** The service boots over serve(AppSpec) with three surfaces + liveness≠readiness; the
  hot-table flags are declared; authenticate/check are wired on the entrypoints; the boot/ready/drain gate emits
  its dated green; the forward-only-migration lint is green; unit + the 1.1/1.5 CDC pass; the contract-coverage
  scanner is green; the KN-P05/KN-P06 follow-ons are named; the work is committed. No gate is weakened.
- **COMMIT.** Header: P-<NNN> M3: Knowledge service shell over serve(AppSpec) + hot-table flags. Body lists:
  contracts 1.1/1.2/1.3/1.5/4.1/4.2 wired; the boot/ready/drain gate greened; the forward-only-migration lint
  green; the store (KN-P05) + outbox (KN-P06) follow-ons named. Branch first if on default; do not push unless
  asked. End with the workspace Co-Authored-By trailer.

---

### KN-P05 — The OLTP store + the (tenant,region) partition + RLS + tenant-predicate discipline (KN-D13)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3a (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3a", the OLTP
  store + (tenant,region) partition half; the outbox is KN-P06).
- **DEPENDS-ON.** KN-P04 (the service shell). The M1 Storage prompts that ship the OLTP client + RLS (11.1) and
  pass STOR-D1/STOR-D2 (restore-verify — the silent-data-loss floor; Knowledge writes no row until green). The M1
  Tenancy prompt that ships the (tenant,region) partition (12.1) + the residency-pin lint. The M0 tenant-predicate
  lint. The M1 Storage prompt that ships the fs-backed BlobStore floor (11.2).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe & EU-sovereign by construction; world-scale + multi-tenant from day 1);
    ../../external-insights/01-process-and-quality-doctrine.md §2 (silent-data-loss outranks every feature — no
    write before restore-verify is green).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/01-tech-and-data-model.md §1
    (the per-service DB, the no-cross-db boundary) + the block/db_row/op-log/snapshot schema sections.
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 11.1 (the OLTP client + RLS),
    12.1 ((tenant,region) partition), 11.2 (the fs-backed BlobStore floor for media/snapshots), 1.5 (the
    hot-table flags declared in KN-P04 — the tables created here).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3a" + §2 (rows 11.1/11.2/12.1).
  - Drills: testing-strategy/01-...-catalogue.md KN-D13 (cross-tenant read via path-tenant spoof → 0;
    tenant-predicate lint catches a tenant-less query at compile).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge:
  - The OLTP store: the block table, db_row, the typed relation tables (page_parent/db_relation — populated in
    KN-P10/KN-P17), the op-log + snapshot metadata tables, all (tenant,region)-partitioned with RLS (11.1/12.1).
    Every query carries the tenant predicate via tenant-scoped query helpers (the tenant-predicate lint must
    compile-reject a tenant-less query).
  - The fs-backed BlobStore wiring (11.2) Knowledge uses for media/snapshots — the M1 floor.
  - FLOOR named: fs-backed BlobStore (11.2) is the M1 floor Knowledge uses; follow-on object-store BlobStore is
    KN-P31 (M5, one-line swap). Name it. The (tenant,region) pin is the single-cell-collab floor; the cross-cell
    follow-on is KN-P30 — name it.
- **CONTRACTS TO IMPLEMENT.** 11.1 OLTP client + RLS (consumed). 12.1 (tenant,region) partition (consumed). 11.2
  fs-backed BlobStore (consumed — the floor). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D13 → read a page/db/row across tenants via path-tenant spoofing → 0 cross-tenant read (tenant from the
    token, never the path); the tenant-predicate lint is RED on a deliberately tenant-less query fixture and GREEN
    on the Knowledge schema; the per-tenant counters signal = 0 cross-tenant is the dated green artifact — CI.
  - The residency-pin lint green on the Knowledge schema (every store is (tenant,region)-pinned) — CI.
- **TESTS (required).** Unit tests for the tenant-scoped query helpers and the RLS predicate. The drill-harness
  scenario for KN-D13 (a cross-tenant path-tenant spoof attempt → rejected). The CDC pairs for rows 11.1/12.1.
  State the cargo-mutants mutation-score floor for the tenant-scoping helper if mandatory-core; if not, say so.
- **DEFINITION OF DONE.** The OLTP store exists, (tenant,region)-partitioned with RLS; every query carries the
  tenant predicate; the fs-backed BlobStore is wired; KN-D13 emits its dated green (0 cross-tenant); the
  tenant-predicate + residency-pin lints green with fixtures; unit + the KN-D13 drill + the CDC pairs pass; the
  contract-coverage scanner is green; the fs-BlobStore floor (KN-P31) + the single-cell floor (KN-P30) are named;
  the work is committed. No gate is weakened.
- **COMMIT.** Header: P-<NNN> M3: Knowledge OLTP store + (tenant,region) partition + RLS (KN-D13). Body lists:
  contracts 11.1/12.1/11.2 wired; KN-D13 greened (0 cross-tenant, measured); the tenant-predicate + residency-pin
  lints green; the fs-BlobStore floor (KN-P31) + single-cell floor (KN-P30) named. Branch first if on default; do
  not push unless asked. End with the workspace Co-Authored-By trailer.

---

### KN-P06 — The transactional outbox (emit-iff-committed, relay, dedup) + the knowledge.* event taxonomy (KN-D7)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3a (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3a", the outbox +
  consumer template + taxonomy registration half).
- **DEPENDS-ON.** KN-P05 (the OLTP store the outbox table sits beside; the partition). The M0 substrate prompts
  (the OutboxTx::emit 2.2 + outbox table 2.3 + EventHandler template 2.4 + dedup 2.5 + the event-taxonomy grammar
  2.9, the no-raw-publish lint).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native: event propagation first-class; no silent data loss);
    ../../external-insights/01-process-and-quality-doctrine.md §2 (silent-data-loss outranks every feature), §5
    (the ratchet — the no-raw-publish lint is a committed gate).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/03-events-contracts-and-glue.md
    §4 (the envelope via the transactional outbox ONLY — no fire-and-forget; the aggregate = the doc/row/db;
    coalescing before emit) + §1 (the complete knowledge.* event taxonomy registered here).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 2.2/2.3/2.4/2.5 (OutboxTx::emit,
    the outbox table with UNIQUE(aggregate, seq), the EventHandler template with whitelisted subjects never *, the
    consumer_dedup ledger), 2.9 (the event taxonomy grammar — register the knowledge.* tokens).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3a" + §2 (rows 2.2/2.4/2.9).
  - Drills: testing-strategy/01-...-catalogue.md KN-D7 (crash between commit and relay-publish → 0 ghost, 0 lost).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge:
  - The per-service outbox table (2.3) beside the OLTP store, (tenant,region)-partitioned. The OutboxTx::emit path
    (2.2): every state change emits iff committed, in the same DB transaction; no fire-and-forget (the
    no-raw-publish lint must compile-reject a publish outside the outbox). The relay drains FOR UPDATE SKIP LOCKED,
    dedups on the ULID event_id, dead-letters after bounded retries; wire the relay into the KN-P04 boot hook.
  - Register the complete knowledge.* event taxonomy under the Bus §6 grammar (2.9) — page/doc/block/database/
    view/row/comment/mention lifecycle + the cross-cutting knowledge.*.erased and knowledge.*.snapshot tokens
    (arch 03 §1).
  - The EventHandler consumer template (2.4) with a whitelisted subjects() set (never *) + the consumer_dedup
    ledger (2.5) — the consumers themselves land in KN-P19/KN-P20/KN-P21/KN-P25/KN-P27.
  - FLOOR named: none — the outbox is the full emit discipline. Name that the concrete consumers (refs/search/
    notif/gdpr/agent) are wired in their own M3d/M3e prompts.
- **CONTRACTS TO IMPLEMENT.** 2.2/2.3/2.4/2.5 the outbox + consumer template (consumed — wired into the Knowledge
  store). 2.9 the knowledge.* tokens (owned — registered). Implement to the frozen shapes; escalate a needed
  change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D7 → crash the Knowledge service between the block/row commit and relay-publish → the event is still
    delivered (outbox survived) and never delivered without the state change: 0 ghost, 0 lost; the outbox
    depth+age telemetry returns to baseline after recovery — CI.
  - The no-raw-publish lint green on the Knowledge crate (0 publish paths outside the outbox) — CI.
- **TESTS (required).** Unit tests for the outbox emit-iff-committed path, the relay drain (SKIP LOCKED + dedup +
  dead-letter), and the knowledge.* token grammar round-trip. The drill-harness scenario for KN-D7 (write → crash
  mid-relay → recover → assert exactly-once). The CDC pairs for rows 2.2/2.3/2.4/2.9. The outbox emit path is
  mandatory-core: state the cargo-mutants mutation-score floor for the emit-iff-committed module.
- **DEFINITION OF DONE.** The outbox emits iff committed; the relay drains exactly-once with dead-lettering; the
  knowledge.* tokens register and parse; the consumer template + dedup ledger exist; KN-D7 emits its dated green
  (0 ghost/0 lost); the no-raw-publish lint green with fixtures; unit + the KN-D7 drill + the CDC pairs pass; the
  contract-coverage scanner is green; the consumer follow-ons are named; the work is committed. No gate is
  weakened to pass.
- **COMMIT.** Header: P-<NNN> M3: Knowledge transactional outbox + knowledge.* event taxonomy (KN-D7). Body
  lists: contracts 2.2/2.3/2.4/2.5/2.9 wired; KN-D7 (0 ghost/0 lost) greened with measured numbers; the
  no-raw-publish lint green; the mutation floor stated. Branch first if on default; do not push unless asked. End
  with the workspace Co-Authored-By trailer.

---

### KN-P07 — Transport item 0: the resume-cursor durable collab transport over the firehose (KN-D1, the headline)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3a (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3a", the
  resume-cursor durable transport — build-order item 0, KN-1 / EI-04 §2.2).
- **DEPENDS-ON.** KN-P05 (the OLTP store + the doc_op op-log table exist), KN-P06 (the outbox the coalesced
  pointer events emit through). The M2 Bus/Signals prompt that ships the frozen firehose resume-cursor transport +
  the subscribe/resume protocol (contract 3.5, OQ-J) — the bus provides the seam; Knowledge owns the
  resume-cursor + idempotent-apply + (later) the CRDT over it. The M2 prompt that ships the *.snapshot resync
  fallback target (2.6).
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
    3.5) + §4 (first runnable: a second connection sees edits live, kill/reconnect loses zero ops).
  - Drills: testing-strategy/01-...-catalogue.md KN-D1 (kill a collab client mid-edit + sever during sustained
    multi-author edit; on resume(scope=doc:<id>, last_seq) → 0 ops lost, 0 duplicate; written to re-run across the
    engine_promote boundary in KN-P29).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge, a collab transport
  module:
  - The resume-cursor durable transport over firehose::subscribe(stream=fan.<tenant>.knowledge, scope=doc:<page_id>,
    cursor?) / firehose::resume(stream, scope, last_seq) (3.5). The doc_op op-log table with a per-doc monotonic
    op_seq (== the firehose seq) and UNIQUE(tenant, page_id, op_id) idempotent apply (op_id = (client_id, lamport)).
  - CONNECT: authorize (Id.check edit|comment on the page_ref, zookie — Layer 2 stub here, full ABAC in KN-P16) →
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
  - FLOOR named: the transport carries CAS op bytes in v1 (the merge engine is KN-P13); the op-log carries Yrs
    update bytes after the engine_promote swap (KN-P29, M5) — the transport is unchanged. KN-D1 is written to
    re-run green across that boundary. Name it.
- **CONTRACTS TO IMPLEMENT.** 3.5 the firehose resume-cursor transport (owned by Knowledge over the bus seam —
  the resume-cursor + idempotent-apply discipline is Knowledge's deliverable). 2.6 the *.snapshot resync fallback
  (consumed — the cold path). Implement to the frozen protocol shape; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D1 → kill a collab client mid-edit + sever the connection during a sustained multi-author edit; on
    resume(scope=doc:<id>, last_seq) assert 0 ops lost, 0 duplicate effects (the UNIQUE(op_id) idempotent apply);
    telemetry: op-log apply lag, op dedup hit-rate, resume-gap size, resync_required rate — all read, the
    0-lost/0-dup counters are the dated green artifact — CI. (Runs on the CAS transport now; the CRDT-boundary
    re-run is owed in KN-P29.)
- **TESTS (required).** Unit tests for op_seq monotonicity, the idempotent ON CONFLICT apply (a re-delivered op
  is a no-op), the resync_required → snapshot path, and scope-bound rejection. The KN-D1 drill scenario as a
  CHAINED test (multi-author edits → kill + sever → reconnect → resume → assert the full op set applied exactly
  once), not a single-handler test (the property is a sequence property). The CDC pair for row 3.5 (Knowledge's
  resume-cursor half). The transport idempotent-apply path is mandatory-core: state the cargo-mutants
  mutation-score floor.
- **DEFINITION OF DONE.** The transport relays + persists + resumes; an op re-send is a no-op; resync falls back
  to the snapshot; presence is ephemeral; KN-D1 emits its dated green artifact (0 lost, 0 duplicate across a kill +
  sever, measured); unit + the chained drill test + the 3.5 CDC pass; the contract-coverage scanner is green; the
  CAS-bytes floor is named with its KN-P29 CRDT follow-on; the work is committed. No threshold is weakened; the
  drill is run against a real kill, not asserted.
- **COMMIT.** Header: P-<NNN> M3: resume-cursor durable collab transport (KN-D1 headline). Body lists: contract
  3.5 implemented (the resume-cursor + idempotent apply over the firehose seam); KN-D1 greened (0 ops lost, 0
  duplicate across kill+sever, measured); the CAS-bytes transport floor named (KN-P29 promotes to Yrs over the
  unchanged transport); the mutation floor stated. Branch first if on default; do not push unless asked. End with
  the workspace Co-Authored-By trailer.

---

### KN-P08 — The editor primitives standalone (serializer + offset model + DOM-surgery), unit-tested before integration

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3b (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3b", the editor
  primitives standalone; the integrated editor is KN-P09).
- **DEPENDS-ON.** KN-P01 (myelin-content + the WASM render path frozen). The Knowledge design sketches under
  design/ (IA, user flows, wireframes with empty/loading/error states) — VISION §3, no frontend code without a
  reviewed design sketch.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (top-of-the-line UX; design comes before frontend);
    ../../external-insights/05-ux-and-design.md §2 (the one-render-path editor mandate; controlled
    contenteditable not textarea; caret = char offset; Enter/IME/paste are the "this isn't a real editor" tells);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it: round-trip is the quantified gate).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md
    §8 (the one editor render path; §8.1 the shared Rust core compiled to WASM, one parser client+server; §8.2
    the three primitives shipped + unit-tested standalone BEFORE the integrated editor — the serializer, the
    offset model, the DOM-surgery for Enter-splits-block + caret-after-split; §8.3 why a markdown-subset string).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 13.1 (the WASM render target the
    primitives consume — reuse the same myelin-content WASM core, no second renderer).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3b" (the correctness-bar-regardless-
    of-engine thesis, KN-4 — primitives standalone first).
  - Drills: testing-strategy/01-...-catalogue.md KN-D2 (the standalone serializer leg: render(parse(md))===md over
    the corpus on the primitive, before integration).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge's editor crate/package
  (the TypeScript/React shell consuming the myelin-content WASM core) + reusing the KN-P01 WASM artifact:
  - The three primitives, each shipped and unit-tested STANDALONE before the integrated editor (arch §8.2): (1)
    the serializer (inline AST ↔ markdown-subset string, with mention/artifact_ref/embed as structured U+FFFC
    nodes) — reuse the KN-P01 WASM core, do not write a second renderer; (2) the offset model (the caret = a char
    offset into the serialized markdown, bridged to/from DOM positions; a structured node is one caret position);
    (3) the DOM-surgery module (Enter-splits-a-block + caret-placement-after-split; controlled contenteditable
    intercepting structural input, plain text through, normalize on serialize; IME/paste handling — the named top
    risk).
  - FLOOR named: these are primitives only — no integrated editor, no transport, no merge, no permissions. The
    integrated editor is KN-P09; name it as the immediate follow-on so a green primitive is not mistaken for an
    editor.
- **CONTRACTS TO IMPLEMENT.** 13.1 the WASM render target (consumed — the serializer runs the identical parser
  code, client + server). Implement to the frozen shape; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D2 (standalone leg) → render(parse(md)) === md 100%, 0 regressions over the corpus on the serializer
    primitive (not yet the integrated path) — CI.
  - An offset/DOM-surgery property gate: a caret round-trips DOM-position ↔ char-offset across every structured
    node (0 off-by-one), and Enter-split places the caret at the start of the new block (the caret-placement
    counter is the green artifact) — CI.
- **TESTS (required).** Standalone unit tests for each of the three primitives (serializer round-trip, offset
  model bridging, Enter-split + caret placement, IME/paste edge cases). The serializer/offset model are
  mandatory-core: state the cargo-mutants mutation-score floor.
- **DEFINITION OF DONE.** The three primitives pass standalone; KN-D2 (standalone leg) emits its dated green
  (100%, 0 regressions on the serializer); the offset/caret gate is green; unit tests pass; the design sketches
  are referenced (or produced + reviewed for any missing primitive surface, VISION §3); the integrated-editor
  follow-on (KN-P09) is named; the work is committed. No gate is weakened.
- **COMMIT.** Header: P-<NNN> M3: editor primitives standalone (serializer + offset model + DOM-surgery). Body
  lists: the three primitives shipped + standalone-tested; KN-D2 standalone leg greened (100%, 0 regressions on
  the serializer); the offset/caret gate greened; the integrated-editor follow-on named (KN-P09); the mutation
  floor stated. Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By
  trailer.

---

### KN-P09 — The integrated single-doc editor over the primitives + the transport (KN-D2 re-run, browser-drive)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3b (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3b", the
  integrated editor; the block-tree storage is KN-P10).
- **DEPENDS-ON.** KN-P08 (the three editor primitives standalone). KN-P07 (the transport the editor sends ops
  over). The M2 design-system prompt that ships the shared overlay/state primitives (the off-screen-picker /
  clipped-dialog / focus-leak foreclosure) the editor's menus/pickers consume (master §2 M0/M2). The Knowledge
  design sketches under design/.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (top-of-the-line UX; the switch-test bar);
    ../../external-insights/05-ux-and-design.md §2 (the one-render-path editor mandate);
    ../../external-insights/01-process-and-quality-doctrine.md §4 (actually try it — drive the real editor in a
    browser before claiming it works).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md
    §8 (the integrated editor over the three primitives + the WASM core);
    ../04-subsystem-architectures/knowledge-platform/architecture/04-views-cli-and-api.md (the editor views/
    affordances); the design/ folder.
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 13.1 (the WASM render target the
    integrated editor consumes — no second renderer), 3.5 (the transport the editor sends ops over — KN-P07).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3b" + §4 (first runnable: a single
    editor + a live second viewer).
  - Drills: testing-strategy/01-...-catalogue.md KN-D2 (render(parse(md))===md re-run over the INTEGRATED editor:
    100%, 0 regressions).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge's editor package:
  - The integrated single-doc editor over the KN-P08 primitives + the transport (KN-P07): create a page, type
    blocks, a second connection sees edits live. Consume the shared design-system overlay/state primitives for any
    menu/picker/dialog (no bespoke off-screen picker).
  - FLOOR named: no merge engine yet (KN-P13 is the CAS floor) and no permissions beyond tenant isolation
    (KN-P16); a single editor + a live second viewer is "first runnable" (roadmap §4). Name it.
- **CONTRACTS TO IMPLEMENT.** 13.1 the WASM render target (consumed — the editor runs the identical parser code,
  client + server). 3.5 the transport (consumed — the editor's op channel). Implement to the frozen shapes;
  escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D2 re-run over the INTEGRATED editor: render(parse(md)) === md 100%, 0 regressions (the corpus-pass-rate
    signal = 100% on the integrated path, not just the library) — CI.
  - The switch-test driving evidence: the editor is driven in a browser (Enter/IME/paste exercised), recorded
    against the design sketches (EI-01 §4 — actually try it; the driven-in-a-browser note dated) — a recorded
    manual-drive artifact, honestly marked yes/partial.
- **TESTS (required).** The KN-D2 corpus test over the integrated editor. An integration test (create page → type
  blocks → a second connection observes the edits live over the transport). The browser-drive evidence recorded
  (yes/no/partial). State the mutation floor if the integration glue is core; if not, say so (the primitives'
  floor is in KN-P08).
- **DEFINITION OF DONE.** The integrated editor runs over the transport; KN-D2 re-emits its dated green (100%, 0
  regressions) over the integrated path; the browser-drive is recorded (honestly marked); the design sketches are
  referenced (or produced + reviewed for any missing screen, VISION §3); the no-merge/no-perms floor is named
  (KN-P13/KN-P16); the work is committed. No gate is weakened.
- **COMMIT.** Header: P-<NNN> M3: integrated single-doc editor (KN-D2 re-run + browser-drive). Body lists: the
  integrated editor over the primitives + transport; KN-D2 greened on the integrated path (100%, 0 regressions);
  the browser-drive evidence recorded; the no-merge/no-perms floor named (KN-P13/KN-P16). Branch first if on
  default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### KN-P10 — The block tree (adjacency list + LexoRank) + stable block ids + page hierarchy

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3b (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3b", the block
  tree + stable ids + page hierarchy; version history is KN-P11, the sync_block floor is KN-P12).
- **DEPENDS-ON.** KN-P03 (the frozen LexoRank order_key). KN-P05 (the OLTP store + the block table). KN-P09 (the
  editor that creates/moves blocks). The M2 Refs prompt that froze the #sub grammar b<id>/h<id> (5.7) so the
  stable block ids are the #sub targets.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale + name-your-floors); ../../external-insights/01-process-and-quality-doctrine.md
    §3 (prove-it).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/01-tech-and-data-model.md §1.2
    (the block tree is an adjacency list — parent_id + a fractional order_key; subtree reads are an index range;
    moves are an order_key write; recursive CTEs for deep walks) + the block-table schema sections;
    02-internals-and-algorithms.md §3.5 (the LexoRank jitter/rebalance as idempotent replayable move ops).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 13.3 (the order_key the tree uses),
    5.7 (the #sub kinds b<opaqueid> / h<opaqueid> — the stable block id is the #sub target; block.block_id stable
    across edits/moves).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3b" (the block tree + page hierarchy).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge:
  - The block tree as per-block rows in an adjacency list (parent_id + the frozen LexoRank order_key from
    KN-P03); subtree reads as an index range; block moves as an order_key write; the rare deep subtree walk as a
    recursive CTE. Stable opaque block ids (block.block_id stable across edits/moves/collaboration — the #sub
    b<opaqueid> / h<opaqueid> targets, 5.7) so an embed of "block b9 of page 7c2" never dangles when the block is
    reordered.
  - Page hierarchy: sub-pages = folder-like nesting (a page is a block subtree root); the page_parent typed
    relation table (the TE-7 source of truth, mirrored to Refs in KN-P19).
  - FLOOR named: none new — the version history/snapshots are KN-P11 and the sync_block read-projection is KN-P12;
    name both as the immediate follow-ons.
- **CONTRACTS TO IMPLEMENT.** 5.7 the stable block-id mint (owned — the b/h #sub targets; stability is
  Knowledge's obligation). 13.3 order_key (consumed — the tree ordering). Implement to the frozen shapes;
  escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - A stable-id property gate: a block reordered/edited keeps its block_id — an embed of b<id> resolves to the
    same block after a move (the moved-block-id-stability counter = 0 dangles) — CI.
  - A subtree-read range gate: a subtree read uses an index range (not a full scan) and a deep walk uses the
    recursive CTE — the query-plan check is the green artifact — CI.
  - (No new leak/loss drill here; the block tree feeds KN-D2/KN-D1 already greened. The KN-D2 corpus re-runs to
    confirm tree edits round-trip.)
- **TESTS (required).** Unit tests for adjacency-list insert/move (order_key bisection + jitter), recursive-CTE
  subtree walk, and stable block-id survival across moves. The CDC pair for row 5.7 (the Knowledge stable-id
  mint). State the cargo-mutants mutation-score floor for the order_key tree-write module if mandatory-core; if
  not, say so.
- **DEFINITION OF DONE.** The block tree + page hierarchy exist; block ids are stable across moves (the stability
  gate green); subtree reads are index-served; KN-D2 still green over tree edits; unit tests + the 5.7 CDC pass;
  the contract-coverage scanner is green; the version-history (KN-P11) + sync_block (KN-P12) follow-ons are named;
  the work is committed. No gate is weakened.
- **COMMIT.** Header: P-<NNN> M3: block tree (adjacency list + LexoRank) + stable ids + page hierarchy. Body
  lists: contract 5.7 stable-id mint owned; 13.3 order_key consumed; the block-id stability gate greened (0
  dangles across moves); the subtree-range gate greened; the version-history + sync_block follow-ons named. Branch
  first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### KN-P11 — Version history + op-log compaction → content-addressed snapshots + op-log GC

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3b (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3b", version
  history + snapshots).
- **DEPENDS-ON.** KN-P10 (the block tree + the op-log the snapshot compacts). KN-P05 (the snapshot metadata table
  + the fs-backed BlobStore). KN-P07 (the doc_op op-log the compaction reads).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it:
    snapshot round-trip is the quantified gate).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/01-tech-and-data-model.md (the
    op-log + snapshot schema sections); 02-internals-and-algorithms.md §7 (op-log compaction → content-addressed
    snapshot, the live tail kept; op-log GC).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 11.2 (the BlobStore for compacted
    snapshots, fs-backed floor), 2.6 (the block-granular *.snapshot the snapshot feeds — replay is KN-P20).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3b" (version history/snapshots; op-log
    compaction/GC).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge:
  - Version history + snapshots: op-log compaction to a content-addressed (BLAKE3) snapshot in the (fs-backed
    floor) BlobStore, keeping the live op-log tail; op-log GC of compacted ranges. A deterministic snapshot
    content-address from (aggregate, version) so the same state compacts to the same snapshot (the replay path in
    KN-P20 re-emits these as knowledge.page.snapshot block-granular).
  - Version-history read: reconstruct a page at a prior version from the nearest snapshot + the op-log tail.
  - FLOOR named: fs-backed BlobStore for snapshots (11.2) — the object-store swap is KN-P31 (M5). Name it.
- **CONTRACTS TO IMPLEMENT.** 11.2 BlobStore (consumed — snapshots, fs floor). 2.6 the *.snapshot target shape
  (referenced — the replay re-emit is KN-P20). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - An op-log compaction round-trip gate: compact an op-log range to a snapshot, GC the range, reconstruct the
    page from snapshot + tail → byte-identical to the pre-compaction state (the compaction-round-trip counter = 0
    mismatches is the green artifact) — CI.
  - A snapshot-determinism gate: the same state compacts to the same content-address (BLAKE3) — CI.
- **TESTS (required).** Unit tests for op-log compaction → snapshot round-trip, the deterministic content-address,
  op-log GC (a GC'd range is reconstructable from the snapshot), and version-history read. State the cargo-mutants
  mutation-score floor for the compaction module if mandatory-core; if not, say so.
- **DEFINITION OF DONE.** Version history + snapshots exist; compaction round-trips byte-identically; the snapshot
  content-address is deterministic; op-log GC is reconstructable; the compaction + determinism gates emit their
  dated green; unit tests pass; the contract-coverage scanner is green; the fs-BlobStore floor (KN-P31) is named;
  the work is committed. No gate is weakened.
- **COMMIT.** Header: P-<NNN> M3: version history + op-log compaction → content-addressed snapshots. Body lists:
  contract 11.2 consumed; the compaction-round-trip gate greened (0 mismatches); the snapshot-determinism gate
  greened; the fs-BlobStore floor named (KN-P31). Branch first if on default; do not push unless asked. End with
  the workspace Co-Authored-By trailer.

---

### KN-P12 — The sync_block read-projection floor (permission-filtered, not editable multi-home)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3b (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3b", the
  sync_block read-projection floor Δ3).
- **DEPENDS-ON.** KN-P10 (the block tree the sync_block node lives in). KN-P01 (the sync_block node in the frozen
  taxonomy). The M2 Refs prompt that froze resolve(ref, viewer) (5.2) — sync_block renders via resolve like embed.
  (The full per-viewer permission filtering it relies on lands in KN-P16; here the floor renders via resolve's
  per-viewer check.)
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors — a floor that masquerades as done is the failure);
    ../../external-insights/04-hard-problems.md §2 (CRDT-after-CAS — editable multi-home awaits the CRDT);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/01-tech-and-data-model.md §2.1
    (the sync_block node in the taxonomy); 02-internals-and-algorithms.md (the sync_block render-via-resolve
    read-projection — like embed, permission-filtered per viewer).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 5.2 (resolve(ref, viewer) — the
    per-viewer projection sync_block renders through), 13.1 (the sync_block node).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3b" (the sync_block read-projection
    floor Δ3) + §5 (sync_block read-projection → editable multi-home, KQ-6, post-M5).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge:
  - The sync_block read-projection FLOOR (Δ3): the sync_block node renders via Refs resolve(ref, viewer) (like
    embed), permission-filtered per viewer — NOT editable-in-place multi-home. A sync_block points at a source
    block subtree; rendering resolves it per viewer (a viewer without read on the source sees a tombstone, never
    the content).
  - FLOOR named: sync_block = read-projection only (no shared-mutable node). Follow-on: editable-in-place
    multi-home synced blocks designed against the CRDT (most-restrictive-of-sites permission + reference-counted
    erasure via the edge index), KQ-6 — post-M5 (enabled by KN-P29's CRDT). Name it.
- **CONTRACTS TO IMPLEMENT.** 5.2 resolve(ref, viewer) (consumed — sync_block renders through it). 13.1 the
  sync_block node (consumed — the frozen taxonomy node). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - A sync_block permission gate: a sync_block of a source the viewer cannot read renders a tombstone, never the
    source content (the sync_block-leak counter = 0 is the green artifact) — CI.
  - A sync_block reflect gate: an edit to the source block reflects in the sync_block's read-projection (the
    projection is live, not a stale copy) — CI.
- **TESTS (required).** Unit tests for the sync_block resolve (permission-filtered render; a tombstone on
  no-read), and the reflect-on-source-edit path. State the mutation floor if the resolve glue is core; if not,
  say so (the permission floor is in KN-P16).
- **DEFINITION OF DONE.** The sync_block read-projection renders permission-filtered (0 leak), reflects source
  edits; the floor is named with its KQ-6 editable follow-on (post-M5, on the CRDT); unit tests pass; the
  contract-coverage scanner is green; the work is committed. No gate is weakened.
- **COMMIT.** Header: P-<NNN> M3: sync_block read-projection floor. Body lists: contract 5.2 consumed; the
  sync_block-leak gate greened (0 leak) + the reflect gate greened; the sync_block read-projection floor named
  (KQ-6 editable-multi-home follow-on, post-M5 on the CRDT). Branch first if on default; do not push unless asked.
  End with the workspace Co-Authored-By trailer.

---

### KN-P13 — The per-block CAS merge floor (no silent overwrite) + soft-locks + offline reconcile (KN-D3, the named-floor proof)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3c (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3c", the CAS
  merge floor; the Layer-2 per-op authority is KN-P14, the ReBAC fragment is KN-P15, the SetExpr push-down is
  KN-P16).
- **DEPENDS-ON.** KN-P07 (the transport the ops ride), KN-P10 (the block tree with block.version). The index
  keeps KN-P13 before KN-P14/KN-P15/KN-P16 in the band (the merge floor lands before the permission fragment, the
  M3-G3 split).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors — a floor that masquerades as done is the failure);
    ../../external-insights/04-hard-problems.md §2.1 (CRDT-after-CAS: the v1 floor guarantees no SILENT overwrite,
    does not merge; the loser reconciles); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it:
    0 silent overwrites is a quantified gate).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md
    §3.2 (the CAS floor: per-block optimistic compare-and-swap on block.version; rows_affected==0 →
    Conflict{current}; the loser reconciles, never silently overwritten; different blocks edit freely in parallel;
    the conflict rate is the CRDT-promotion trigger metric).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 3.5 (the transport the ops ride —
    consumed).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3c" (the CAS floor thesis; the
    advisory soft-locks + snapshot/restore; offline = read + queued light-edit reconciled via the CAS floor) + §5
    (CAS → Yrs CRDT; offline = read+queued → full offline-first).
  - Drills: testing-strategy/01-...-catalogue.md KN-D3 (two clients edit the same block concurrently → the loser
    is rejected with current state, never silently overwritten; different blocks edit in parallel with no false
    conflict; 0 silent overwrites — the named-floor proof in the master M3→M4 gate).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge's collab/merge module
  (Layer 3a over the transport):
  - The per-block optimistic compare-and-swap: EDIT_BLOCK(block_id, expected_version, new_inline, new_props)
    runs UPDATE block SET ... version=version+1 WHERE tenant=? AND block_id=? AND version=expected_version; on
    rows_affected==0 return Conflict{current: server state} — the loser RECONCILES, never silently overwritten.
    Different blocks edit freely in parallel (the guard is per-block).
  - Advisory soft-locks ("someone is editing this block," over the awareness channel) + snapshot/restore layered
    on the CAS guard. Offline = read + queued light-edit reconciled via the CAS floor (the deep offline-first
    answer arrives with the CRDT, KN-P29).
  - The CAS-conflict-rate metric (rows_affected==0 fraction) emitted to telemetry — it is the CRDT-promotion
    trigger metric (KQ-1) KN-P29 reads.
  - FLOOR named: CAS (no merge). Follow-on: the Yrs CRDT (KN-1, KN-P29, M5), triggered by the first true
    concurrent-edit conflict measured via the KN-D3 CAS-conflict-rate metric. Also: offline = read + queued
    light-edit → full offline-first (KN-P29). Name both. (The per-op permission check is KN-P14; this prompt is
    the merge guard only.)
- **CONTRACTS TO IMPLEMENT.** 3.5 the transport (consumed — the ops ride it). Implement to the frozen shape;
  escalate a needed change. (4.2/4.10 check/zookie are wired in KN-P14, the per-op authority prompt.)
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D3 → two clients edit the same block concurrently → the loser is rejected with current state (never
    silently overwritten); different blocks edit in parallel with no false conflict; 0 silent overwrites; the
    CAS-conflict-rate metric is emitted (the CRDT-promotion trigger) — CI. (This is the named-floor proof in the
    master M3→M4 gate.)
- **TESTS (required).** Unit tests for the CAS guard (winner commits, loser gets Conflict{current}), per-block
  independence (different blocks no false conflict), the soft-lock advisory, and the offline queued-edit
  reconcile. The KN-D3 drill as a CHAINED concurrent-edit scenario (two clients, same block, interleaved) — the
  property is a concurrency property, not a single handler. The CAS guard is mandatory-core: state the
  cargo-mutants mutation-score floor (the no-silent-overwrite property must survive mutation).
- **DEFINITION OF DONE.** The CAS guard rejects the loser with current state; different blocks are independent;
  the soft-locks + offline reconcile work; the conflict-rate metric is emitted; KN-D3 emits its dated green (0
  silent overwrites); unit + the chained drill pass; the contract-coverage scanner is green; the CAS floor is
  named with its KN-P29 CRDT follow-on + the offline-first follow-on; the work is committed. No gate is weakened;
  the drill runs a real concurrent edit, not an asserted one.
- **COMMIT.** Header: P-<NNN> M3: per-block CAS merge floor + soft-locks + offline reconcile (KN-D3 named-floor
  proof). Body lists: contract 3.5 consumed; KN-D3 greened (0 silent overwrites, conflict-rate metric emitted);
  the CAS floor named (KN-P29 Yrs CRDT follow-on, trigger: first true concurrent conflict via the conflict-rate
  metric) + the offline-first follow-on; the mutation floor stated. Branch first if on default; do not push unless
  asked. End with the workspace Co-Authored-By trailer.

---

### KN-P14 — The Layer-2 per-op authority checks (permission/schema/erasure) + the zookie new-enemy guard

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3c (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3c", the per-op
  Layer-2 authority + the zookie read-your-writes; the ReBAC fragment is KN-P15, the SetExpr push-down is KN-P16).
- **DEPENDS-ON.** KN-P13 (the CAS merge guard the per-op check sits above), KN-P10 (the block/db_row the op
  targets). The M1 Identity prompts that ship check + CaveatContext (4.2) + write_tuples/zookie (4.6/4.10). The M2
  Bus prompt that ships the *.erased tombstone (the erased-content degrade target).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (one permission model; GDPR-safe by construction);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it: 0 stale-grant writes is the quantified
    gate).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md
    §3.1 (Layer 2 authority — permission/schema/erasure checks on every op, above the merge layer);
    03-events-contracts-and-glue.md §3.3 (the zookie new-enemy guard stamped on page.acl_zookie).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 4.2 (check + CaveatContext on each
    op — Layer 2), 4.10 (the zookie read-your-writes so a just-revoked editor's op is rejected).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3c" (Layer 2 authority checks on every
    op; the zookie new-enemy guard).
  - Drills: testing-strategy/01-...-catalogue.md — the just-revoked-editor leg (an op from an editor revoked
    at-or-after the zookie revision is rejected; 0 stale-grant writes; part of the KN-D5/KN-D3 family).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge's collab/merge module
  (Layer 2, above the CAS guard from KN-P13):
  - The Layer 2 authority checks that run on EVERY incoming op before merge (arch §3.1): the permission check
    (Id.check edit|comment with the zookie — read-your-writes; a just-revoked editor's op is rejected), schema
    validation (a db-row op must satisfy the FieldType defs), and the erased-content degrade (an op against
    *.erased content degrades, never resurrects). (The full ABAC list_objects push-down for reads is KN-P16; the
    per-op write check is here.)
  - The zookie new-enemy guard: reads/writes pass the page.acl_zookie revision so a grant revoked at-or-after the
    zookie cannot be read/written stale (4.10).
  - FLOOR named: none — the per-op authority is the full v1 write-side check. Note the read-side list_objects
    push-down is KN-P16.
- **CONTRACTS TO IMPLEMENT.** 4.2 check + CaveatContext (consumed — the per-op Layer 2 check). 4.10 the zookie
  read-your-writes (consumed — the just-revoked op rejection). Implement to the frozen shapes; escalate a needed
  change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - A just-revoked-editor gate: an op from an editor revoked at-or-after the zookie revision is rejected (the
    new-enemy guard) — 0 stale-grant writes; the stale-grant counter = 0 is the dated green artifact — CI.
  - A schema-validation gate: a db-row op violating the FieldType defs is rejected before merge (0 invalid rows
    persisted) — CI.
- **TESTS (required).** Unit tests for the per-op permission check (edit|comment with the zookie), schema
  validation, the erased-content degrade, and the zookie revocation rejection. The just-revoked-editor drill
  scenario (grant → revoke → op straddling the zookie → rejected). The CDC pair for row 4.2 (Knowledge's consumer
  half). The per-op authority check is mandatory-core: state the cargo-mutants mutation-score floor.
- **DEFINITION OF DONE.** The Layer 2 per-op checks run on every op (permission/schema/erasure); the zookie
  rejects a stale-grant op; the just-revoked + schema-validation gates emit their dated green; unit + the
  just-revoked drill + the 4.2 CDC pass; the contract-coverage scanner is green; the read-side push-down (KN-P16)
  is named; the work is committed. No gate is weakened.
- **COMMIT.** Header: P-<NNN> M3: Layer-2 per-op authority + zookie new-enemy guard. Body lists: contract
  4.2/4.10 consumed (per-op check + zookie read-your-writes); the just-revoked-editor gate greened (0 stale-grant
  writes); the schema-validation gate greened; the read-side list_objects push-down follow-on named (KN-P16); the
  mutation floor stated. Branch first if on default; do not push unless asked. End with the workspace
  Co-Authored-By trailer.

---

### KN-P15 — The Knowledge ReBAC page-tree namespace fragment (compiled into the cell schema)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3c (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3c", the ReBAC
  namespace fragment; the list_objects SetExpr push-down is KN-P16).
- **DEPENDS-ON.** KN-P10 (the page/block/db_row id columns the relations reference). The M1 Identity prompts that
  ship the ReBAC namespace engine (4.9) that compiles the per-subsystem fragments into one cell schema, and check
  + CaveatContext (4.2). The M0 no-cross-db lint.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (one permission model; GDPR-safe by construction);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/01-tech-and-data-model.md §5
    (the page-tree ReBAC fragment: page.read = (parent_page->read + direct_reader) - direct_block; the row_reader
    userset; the view_field CaveatContext off the hot path).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §1 (the frozen ReBAC
    fragments — Knowledge: page-tree inherit-with-overrides + row + field caveat).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 4.9 (the per-subsystem ReBAC
    namespace fragment — the Knowledge fragment), 4.2 (check + CaveatContext for the field caveat off the hot
    path).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3c" + §2 (row 4.9).
  - Drills: testing-strategy/01-...-catalogue.md — the page.read override-formula unit gate (direct_block removes a
    narrowed sub-page); the fragment COMPILES in the cell schema (build-time gate; feeds KN-D5 in KN-P16).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge:
  - The Knowledge ReBAC namespace fragment submitted into the one cell schema Identity compiles (4.9): page-tree
    inherit-with-overrides (page.read = (parent_page->read + direct_reader) - direct_block); row-level via the
    row_reader userset (the relation InRelation lowers over in KN-P16); field-level via the frozen
    CaveatContext{object, field, attrs} on view_field, evaluated at check-time OFF the hot path. The fragment must
    COMPILE in the cell schema.
  - FLOOR named: none — this is the full permission model shape for v1 (the page-tree fragment is complete). The
    field-level predicate catalogue per database is co-designed with Id's role-bundle catalogue (KQ-5, parallel,
    not a floor of this prompt) — note it. The list_objects push-down that lowers this fragment into SQL is KN-P16
    — name it.
- **CONTRACTS TO IMPLEMENT.** 4.9 the Knowledge ReBAC fragment (owned — compiled by Identity). 4.2 check +
  CaveatContext (consumed — field-level ABAC off the hot path). Implement to the frozen shapes; escalate a needed
  change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The Knowledge ReBAC fragment COMPILES in the shared cell schema (a build-time gate; the compiled-schema
    artifact is the green) — CI.
  - A page.read override-formula gate: direct_block removes a narrowed sub-page from a viewer's reachable set; a
    direct_reader on a sub-page adds it; the override formula evaluates correctly (the formula-correctness unit
    gate is the green artifact) — CI.
  - The no-cross-db lint green (Knowledge declares relations, never reads another owner's DB) — CI.
- **TESTS (required).** Unit tests for the page.read override formula (direct_block / direct_reader / inheritance),
  the row_reader userset shape, and the field-level CaveatContext hiding. The CDC pair for row 4.9. State the
  mutation floor for the override-formula module if mandatory-core; if not, say so (the leak-critical lowering is
  KN-P16).
- **DEFINITION OF DONE.** The ReBAC fragment compiles in the cell schema; the page.read override formula is
  correct (the formula gate green); the no-cross-db lint green; unit + the 4.9 CDC pass; the contract-coverage
  scanner is green; the KQ-5 field-predicate-catalogue note is written; the list_objects push-down follow-on
  (KN-P16) is named; the work is committed. No gate is weakened.
- **COMMIT.** Header: P-<NNN> M3: Knowledge ReBAC page-tree namespace fragment. Body lists: contract 4.9 fragment
  compiled in the cell schema; the page.read override-formula gate greened; the no-cross-db lint green; the
  list_objects SetExpr push-down follow-on named (KN-P16); the KQ-5 field-predicate-catalogue parallel work noted.
  Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### KN-P16 — The list_objects SetExpr push-down + write_tuples/zookie ACL writes (KN-D5, zero leak incl. COUNT)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3c (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3c", the
  list_objects SetExpr push-down half — closes the count-leak).
- **DEPENDS-ON.** KN-P15 (the Knowledge ReBAC fragment compiled), KN-P14 (the per-op check it generalises to
  reads), KN-P10 (the page/block/db_row id columns the JOIN lowers over). The M1 Identity prompts that ship
  list_objects + the SetExpr push-down (4.3), write_tuples/zookie (4.6/4.10), and the per-tenant authz reverse
  index (authz_visible). The M0 no-cross-db + tenant-predicate lints.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (one permission model; GDPR-safe by construction — a leak is a security AND a GDPR
    breach); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it: 0 leak incl. COUNT is the
    quantified gate; observability is the pass).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md
    §4.1 (the frozen SetExpr lowering over db_row.id — the All/None/Ids/InRelation{row_reader, via_column}/Union
    table; the JOIN against authz_visible; closing the count-leak because the ACL conjunct is INSIDE the query) +
    §5 (permission-filtered reads everywhere — never post-filter); 03-events-contracts-and-glue.md §3.2 (the
    list_objects/check glue).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-E (the SetExpr
    facet lowering, the named authz_visible JOIN).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 4.3 (list_objects + the SetExpr
    push-down), 4.6/4.10 (write_tuples → zookie; the zookie consistency).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3c" + §2 (rows 4.3, 4.6/4.10).
  - Drills: testing-strategy/01-...-catalogue.md KN-D5 (a confidential page / overridden sub-page / row-restricted
    db / field-hidden column never appears in any view/backlink/search/embed/RAG result for an unauthorized viewer
    — INCLUDING an aggregate COUNT; 0 leaked artifacts, 0 count-leak).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge:
  - The list_objects SetExpr push-down: Knowledge calls list_objects(viewer, read, 'page'|'database_row', zookie)
    and lowers the returned Filter{set_expr, zookie} into every list/board/view/search query via the frozen
    lowering over its own id column (the All/None/Ids/InRelation/Union table from arch §4.1) — the InRelation case
    (row_reader, via_column: db_row.id) is a JOIN against the per-tenant authz_visible reverse index. No N+1, no
    post-filter; the ACL conjunct is INSIDE the query so even a COUNT is permission-correct (the count-leak
    closed).
  - write_tuples → zookie on a page ACL change (knowledge.access.* events), stamped on page.acl_zookie (4.6/4.10);
    subsequent reads pass the zookie so a just-revoked grant cannot be read stale (the authz index honours the
    zookie revision watermark — composing with the KN-P14 write-side guard).
  - FLOOR named: none — this is the full read-side permission filtering for v1.
- **CONTRACTS TO IMPLEMENT.** 4.3 list_objects + the SetExpr lowering (consumed — Knowledge lowers the Filter into
  its SQL). 4.6/4.10 write_tuples → zookie (consumed — the ACL-change path + the read-your-writes watermark).
  Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D5 → a confidential page / overridden sub-page / row-restricted db / field-hidden column never appears in
    any view / list / embed result for an unauthorized viewer — INCLUDING an aggregate COUNT; 0 leaked artifacts,
    0 count-leak; the zero-escape counter = 0 is the dated green artifact — CI. (Re-confirmed over search/embed/RAG
    in KN-P21 once those paths exist.)
  - The no-cross-db + tenant-predicate lints green on the read paths — CI.
- **TESTS (required).** Unit tests for the SetExpr lowering over each SetExpr variant (All → no conjunct,
  InRelation → the JOIN against authz_visible, None → WHERE false, Union/Ids), and the permission-correct COUNT.
  The KN-D5 drill scenario including the COUNT path (a permission-correct COUNT over a row-restricted db) + the
  write_tuples→zookie read-your-writes path. The CDC pair for row 4.3. The SetExpr lowering is mandatory-core (a
  leak is catastrophic): state the cargo-mutants mutation-score floor (the no-leak property must survive mutation).
- **DEFINITION OF DONE.** The SetExpr push-down conjoins the ACL inside every read query (no post-filter); the
  COUNT is permission-correct; write_tuples → zookie stamps the watermark; KN-D5 emits its dated green (0 leak, 0
  count-leak, measured); the no-cross-db + tenant-predicate lints green; unit + the KN-D5 drill (incl. COUNT) +
  the 4.3 CDC pass; the contract-coverage scanner is green; the work is committed. No gate is weakened; the leak
  drill runs real unauthorized reads.
- **COMMIT.** Header: P-<NNN> M3: list_objects SetExpr push-down + zookie ACL writes (KN-D5 0 leak incl. COUNT).
  Body lists: contract 4.3 SetExpr lowered (ACL inside the query, count-leak closed), 4.6/4.10 write_tuples →
  zookie wired; KN-D5 greened (0 leaked artifacts, 0 count-leak, measured); the no-cross-db + tenant-predicate
  lints green; the mutation floor stated. Branch first if on default; do not push unless asked. End with the
  workspace Co-Authored-By trailer.

---

### KN-P17 — The flexible database (JSONB property bag + GIN-indexed projection + views + relations) (KN-D9)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3d (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3d", the
  flexible-DB half; the read-time formula/rollup is KN-P18, the refs/search/notif glue is KN-P19..KN-P24).
- **DEPENDS-ON.** KN-P02 (the frozen myelin-query FieldType/ViewSpec/QueryAst). KN-P16 (the SetExpr push-down the
  db query conjoins). KN-P10 (the db_row store + page hierarchy). The M1 Storage prompt (the GIN/expression-index
  substrate).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (Notion-class; world-scale); ../../external-insights/04-hard-problems.md §2.4 (the
    derived-projection scaling discipline); ../../external-insights/01-process-and-quality-doctrine.md §3
    (prove-it: the p99-within-budget + measured-promotion-trigger gate).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/01-tech-and-data-model.md §1.2
    (JSONB property-bag rows source of truth + GIN/expression indexes + generated columns for measured-hot facets
    — the derived projection; the >5% promotion threshold frozen 6.3/OQ-C);
    02-internals-and-algorithms.md §4.1 (VIEW_QUERY with the SetExpr conjoin; measured-hot facets → generated
    index, cold → bounded paginated GIN scan).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 13.3 (FieldType/ViewSpec/QueryAst
    — Knowledge owns its executor), 4.3 (the SetExpr conjoin), 6.3 (the >5% facet-promotion threshold —
    measured here, acted on in KN-P31).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3d" (Floor 1 JSONB+GIN → per-facet
    generated index) + §5.
  - Drills: testing-strategy/01-...-catalogue.md KN-D9 (filter/sort/group a large multi-tenant database → p99
    within budget; measure the >5% facet-promotion trigger).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge's database module:
  - The Database Service: a JSONB property bag per row (db_row.props, the source of truth) + a derived,
    GIN-indexed projection (jsonb_path_ops) + generated columns for the measured-hot facets; typed field
    definitions (the frozen FieldType enum); views as ViewSpec query projections (table/board/calendar/timeline);
    two-way relations (db_relation, the TE-7 source of truth — the Refs mirror is KN-P19). The VIEW_QUERY path
    conjoins the SetExpr Filter into every db query (arch §4.1) — paginated, row-capped, statement-timeout.
  - FLOOR 1 named: JSONB bag + GIN-indexed projection (read-time facets). Follow-on: per-facet generated/
    expression-column index promoted when a facet crosses the frozen >5% view-execution threshold (6.3/OQ-C,
    measured here) — KN-P31 (M5). Name it. (The read-time formula/rollup over these rows is KN-P18 — name it.)
- **CONTRACTS TO IMPLEMENT.** 13.3 the FieldType/ViewSpec/QueryAst executor (owned — Knowledge owns its executor;
  the definitions are the frozen shared shapes). 4.3 the SetExpr conjoin (consumed). 6.3 the >5% facet-promotion
  threshold (consumed — measured here, acted on in KN-P31). Implement to the frozen shapes; escalate a needed
  change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D9 → filter/sort/group a large multi-tenant database (JSONB + projection + the SetExpr conjoin) →
    read-time p99 within the budget in the thresholds file; the facet-execution-frequency telemetry measures the
    >5% promotion trigger (recorded, not yet acted on) — SCHED.
  - A view-permission gate: every view/board query conjoins the SetExpr Filter inside the query (0 post-filter; 0
    leak across a view over a row-restricted db — composing with KN-D5) — CI.
- **TESTS (required).** Unit tests for the VIEW_QUERY SetExpr lowering into the db query, the typed FieldType
  validation, the two-way relation maintenance, and the generated-column facet path. The KN-D9 drill scenario on
  the failure-injection harness at scale (a large multi-tenant db). The view executor is mandatory-core (a view
  leak is catastrophic): state the cargo-mutants mutation-score floor.
- **DEFINITION OF DONE.** The flexible DB serves views over the frozen shapes with the SetExpr conjoined; KN-D9
  emits its dated green (p99 within budget; the promotion trigger measured); the view-permission gate is green;
  unit + the scale drill pass; the contract-coverage scanner is green; Floor 1 (per-facet index) is named with its
  KN-P31 follow-on + the formula/rollup follow-on (KN-P18); the work is committed. No gate is weakened; the budget
  is read from the thresholds file, never edited to pass.
- **COMMIT.** Header: P-<NNN> M3: flexible database (JSONB + GIN projection + views + relations) (KN-D9). Body
  lists: contract 13.3 executor owned, 4.3 SetExpr conjoined, 6.3 threshold measured; KN-D9 greened (db p99 within
  budget, promotion trigger measured); the view-permission gate greened; Floor 1 (per-facet index) named (KN-P31).
  Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### KN-P18 — The read-time formula/rollup engine (bounded FormulaAst evaluator, never stored) (KN-D10)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3d (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3d", the
  read-time formula/rollup half).
- **DEPENDS-ON.** KN-P17 (the flexible DB the formulas/rollups read over). KN-P02 (the frozen myelin-query
  expression core = the FormulaAst). KN-P16 (the list_objects conjoin the rollups apply).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (Notion-class; world-scale); ../../external-insights/04-hard-problems.md §2.4 (rollups/
    formulas computed at read time, never stored — the Notion scaling pain avoided);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it: the p99-within-budget +
    measured-promotion-trigger gate).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md
    §4.2 (the read-time formula/rollup engine — the bounded dependency-graph evaluator; rollups conjoin
    list_objects; cycle → #CYCLE; the FormulaAst = the bounded myelin-query expression core; the named
    materialised follow-on per measured-slow rollup).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 13.3 (rollup/formula computed at
    read time never stored), 4.3 (the rollup list_objects conjoin), 11.6 (the OLAP read store the materialised
    rollup follow-on feeds — referenced; built in KN-P31).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3d" (Floor 2 read-time formula/rollup
    → per-rollup materialised aggregate) + §5.
  - Drills: testing-strategy/01-...-catalogue.md KN-D10 (a rollup over a large related set at read time → p99
    within budget; measure when incremental materialisation is needed).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge's database module:
  - The read-time formula/rollup engine (arch §4.2): formulas + rollups computed at READ TIME, never stored; a
    bounded dependency-graph evaluator over the FormulaAst (the bounded myelin-query expression core — no UDFs/
    loops/recursion, statically cost-bounded); rollups over a relation conjoin list_objects (permission-filtered);
    a cycle surfaces as #CYCLE (a diagnostic cell), never an infinite loop.
  - FLOOR 2 named: read-time formula/rollup. Follow-on: per-rollup incrementally-maintained materialised aggregate
    fed off the bus (knowledge.row.updated deltas → the OLAP read store 11.6) when read-time recompute is measured
    too slow (KQ-4) — KN-P31 (M5). Name it.
- **CONTRACTS TO IMPLEMENT.** 13.3 the read-time formula/rollup (owned — the bounded evaluator over the frozen
  expression core). 4.3 the rollup list_objects conjoin (consumed). Implement to the frozen shapes; escalate a
  needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D10 → a rollup over a large related set computed at read time (permission-filtered) → p99 within budget;
    the rollup-latency telemetry measures when incremental materialisation is needed (recorded) — SCHED.
  - A formula-cycle gate: a cyclic formula surfaces as #CYCLE, never an infinite loop (bounded-evaluation
    counter = the green artifact) — CI.
  - A rollup-permission gate: a rollup over a relation conjoins list_objects so a restricted related row is never
    counted/summed for an unauthorized viewer (0 rollup leak — composing with KN-D5) — CI.
- **TESTS (required).** Unit tests for the read-time formula evaluator (each RollupFn; the depth-bound + cycle
  detection → #CYCLE), the permission-filtered rollup conjoin, and the dependency-graph ordering. The KN-D10 drill
  scenario on the failure-injection harness at scale (a large related set). The formula evaluator is
  mandatory-core (cost-bounding + no rollup leak): state the cargo-mutants mutation-score floor.
- **DEFINITION OF DONE.** The read-time formula/rollup engine is bounded + cycle-safe (#CYCLE, never a loop) and
  permission-filtered; KN-D10 emits its dated green (p99 within budget; the promotion trigger measured); the cycle
  + rollup-permission gates are green; unit + the scale drill pass; the contract-coverage scanner is green; Floor
  2 (per-rollup materialisation) is named with its KN-P31 follow-on; the work is committed. No gate is weakened;
  the budget is read from the thresholds file.
- **COMMIT.** Header: P-<NNN> M3: read-time formula/rollup engine (KN-D10). Body lists: contract 13.3 read-time
  formula/rollup owned, 4.3 conjoined; KN-D10 greened (rollup p99 within budget, materialisation trigger measured);
  the #CYCLE + rollup-permission gates greened; Floor 2 (per-rollup materialisation) named (KN-P31); the mutation
  floor stated. Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By
  trailer.

---

### KN-P19 — Refs glue: #sub mints + 4-step tombstone ladder + edge events + resolve/project + TE-7 typed-edge mirror

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3d (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3d", the refs
  glue; the replay/reindex is KN-P20, the search feed is KN-P21).
- **DEPENDS-ON.** KN-P10 (the block tree + page_parent typed table), KN-P17 (the db_relation rows the typed-edge
  mirror connects), KN-P16 (the SetExpr/permission filtering project applies). The M2 Refs prompt (ArtifactRef
  5.1, resolve 5.2, backlinks/traverse 5.3, refs.edge.created 5.4, the TE-7 mirror 5.5, project 5.6, the #sub
  grammar + tombstone ladder 5.7).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (one cross-artifact reference graph; not a silo);
    ../../external-insights/01-process-and-quality-doctrine.md §7 (reconcile cross-component contracts; one
    reference scheme).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/03-events-contracts-and-glue.md
    §2.1 (ArtifactRef + the #sub grammar b/h/row-/field-/comment-/thread- with stable opaque ids; the 4-step
    tombstone ladder LIVE/MOVED/OUTDATED/GONE/ERASED; a tombstone always carries the root), §2.2 (project(ref,
    viewer) — the frozen shape {title,state,icon,render_hint,sub_anchor?}; a confidential page → tombstone, never
    leaks), §3.1 (the TE-7 typed-edge mirror: db_relation → knowledge.relation.* → Refs lifecycle edge;
    page_parent → knowledge.page.parent_set → Refs parent edge; the typed table is truth).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-4 (the #sub grammar
    frozen — the field- node is new; h has no hyphen).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 5.1/5.2/5.3/5.4/5.5/5.6/5.7 (the
    refs glue).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3d" + §2 (rows 5.x).
  - Drills: testing-strategy/01-...-catalogue.md — the tombstone-ladder + project leak gates (a confidential page
    → tombstone, never leaks; feeds KN-D5 re-confirm in KN-P21 and KN-D6 in KN-P20).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge:
  - The three inline nodes (mention/artifact_ref/embed) emit refs.edge.created on persist (5.4, NOT coalesced);
    implement resolve/backlinks/traverse (5.2/5.3); the #sub grammar b/h/row-/field-/comment-/thread- with
    stable-id mint (5.7) + the 4-step tombstone ladder (permission → root → sub LIVE/MOVED/OUTDATED/GONE →
    ERASED; a tombstone always carries the root). project(ref, viewer) (5.6) — the frozen
    {title,state,icon,render_hint,sub_anchor?} shape, per-viewer permission-checked, a confidential page →
    tombstone.
  - The TE-7 typed-edge mirror (5.5): the same transaction that writes a page_parent / db_relation typed row emits
    knowledge.page.parent_set / knowledge.relation.* so Refs projects the lifecycle edge (the typed table is
    truth; Refs holds the rebuildable projection).
  - FLOOR named: none new — the tombstone ladder + project + TE-7 mirror are complete. The replay/reindex that
    rebuilds the Refs projection is KN-P20 — name it. (The >5% search-block prune is KQ-10, measured, parallel —
    note it; it lands with the search feed in KN-P21.)
- **CONTRACTS TO IMPLEMENT.** 5.6 project (owned — the Knowledge impl), 5.7 the #sub stable-id mint (owned), 5.5
  the TE-7 typed-edge mirror (owned source of truth), 5.2/5.3/5.4 resolve/backlinks/traverse + edge events
  (consumed/produced). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - A project-leak gate: project(ref, viewer) of a confidential page returns a tombstone carrying the root, never
    the title/content, for an unauthorized viewer (the project-leak counter = 0 is the green artifact) — CI.
  - A tombstone-ladder gate: each of LIVE/MOVED/OUTDATED/GONE/ERASED returns the right tombstone carrying the root
    (the 4-step ladder property) — CI.
  - A TE-7 mirror gate: writing a page_parent / db_relation typed row emits the edge event in the same
    transaction (0 typed-row-without-edge) — CI.
- **TESTS (required).** Unit tests for the #sub mints (grammatical sub-URNs), the 4-step tombstone ladder, project
  (a confidential page → tombstone), and the TE-7 mirror (typed row → emitted edge event). The CDC pairs for rows
  5.6, 5.7, 5.5. The project permission path is mandatory-core (a project leak is a leak): state the cargo-mutants
  mutation-score floor.
- **DEFINITION OF DONE.** The refs glue (edges, resolve, tombstone ladder, project, TE-7 mirror) exists; project
  never leaks a confidential title; the tombstone ladder + TE-7 mirror gates emit their dated green; unit + the
  CDC pairs pass; the contract-coverage scanner is green; the replay follow-on (KN-P20) + KQ-10 are noted; the
  work is committed. No gate is weakened.
- **COMMIT.** Header: P-<NNN> M3: refs glue (#sub + tombstone ladder + edge events + project + TE-7 mirror). Body
  lists: contracts 5.6/5.7/5.5/5.2/5.3/5.4 wired; the project-leak + tombstone-ladder + TE-7 mirror gates greened;
  the replay follow-on (KN-P20) + KQ-10 noted; the mutation floor stated. Branch first if on default; do not push
  unless asked. End with the workspace Co-Authored-By trailer.

---

### KN-P20 — replay(scope)/reindex-from-source (block-granular *.snapshot via the outbox) (KN-D6, cold == live)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3d (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3d", the
  reindex-from-source recovery path).
- **DEPENDS-ON.** KN-P11 (the content-addressed snapshots replay re-emits), KN-P19 (the Refs edge projection
  replay rebuilds), KN-P06 (the outbox replay re-emits through). The M2 Bus prompt (reindex(scope) → replay 2.6).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (not a silo); ../../external-insights/04-hard-problems.md §5 (reindex-from-source —
    Search/Refs are derived stores, rebuilt via the live consumer, never reading the owner DB);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it: cold==live).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/03-events-contracts-and-glue.md
    §2.3 (replay(scope) block-granular *.snapshot via the outbox — the only recovery path; deterministic snapshot
    event_id from (aggregate, version)); §3.1 (the TE-7 drift-correction — a scoped replay reconverges Refs to the
    typed table).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 2.6 (reindex/replay — the
    block-granular *.snapshot re-emit via the outbox through the live consumer; the only recovery path).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3d" + §2 (row 2.6).
  - Drills: testing-strategy/01-...-catalogue.md KN-D6 (wipe Knowledge's derived state — the Refs edge projection
    / Search index — replay(scope) block-granular → rebuilt state matches live; rebuild uses the live consumer
    path only; cold == live).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge:
  - replay(scope, since) (2.6): emit knowledge.page.snapshot (block-granular) / knowledge.row.snapshot /
    refs.edge.snapshot via the OUTBOX through the live bus — the only recovery path; deterministic snapshot
    event_id from (aggregate, version) so a re-run is idempotent. The TE-7 drift-correction (a scoped replay
    reconverges Refs to the typed table).
  - FLOOR named: none — replay is the full recovery path. (The Search index this rebuilds is fed in KN-P21; a
    scoped replay rebuilds it identically.)
- **CONTRACTS TO IMPLEMENT.** 2.6 replay (owned — the Knowledge re-emit). Implement to the frozen shape; escalate
  a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D6 → wipe Knowledge's derived state (Refs edge projection / Search index — fed in KN-P21); replay(scope)
    (block-granular *.snapshot) → rebuilt state matches live; rebuild uses the live consumer path only; the
    reindex-parity-hash telemetry = live is the dated green (cold == live) — SCHED.
  - A replay-idempotence gate: a re-run of replay(scope) produces identical snapshot event_ids (deterministic from
    (aggregate, version)) — no double-apply on the consumer — CI.
- **TESTS (required).** Unit tests for the block-granular snapshot emit (deterministic event_id), the TE-7
  drift-correction, and replay idempotence. The KN-D6 drill (wipe + replay → parity hash) on the failure-injection
  harness — rebuild via the live consumer path only. The CDC pair for row 2.6. The replay path is mandatory-core
  (cold==live is a recovery property): state the cargo-mutants mutation-score floor.
- **DEFINITION OF DONE.** replay re-emits block-granular snapshots through the outbox; a re-run is idempotent; the
  TE-7 drift-correction reconverges Refs; KN-D6 emits its dated green (cold == live, parity hash); the
  replay-idempotence gate is green; unit + the KN-D6 drill + the 2.6 CDC pass; the contract-coverage scanner is
  green; the work is committed. No gate is weakened; the reindex drill rebuilds via the live consumer only.
- **COMMIT.** Header: P-<NNN> M3: replay/reindex-from-source (block-granular *.snapshot) (KN-D6 cold==live). Body
  lists: contract 2.6 owned; KN-D6 greened (cold==live, parity hash); the replay-idempotence gate greened; the
  mutation floor stated. Branch first if on default; do not push unless asked. End with the workspace
  Co-Authored-By trailer.

---

### KN-P21 — The Search feed: declare_indexable(IndexSpec) + query/semantic with the Filter conjoined (KN-D5 re-confirm)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3d (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3d", the search
  glue).
- **DEPENDS-ON.** KN-P19 (project — the projection fed to the index), KN-P16 (the list_objects Filter the search
  query conjoins), KN-P20 (the replay that rebuilds the index, KN-D6). The M2 Search prompt (declare_indexable
  6.3, query/semantic with the Filter conjoin 6.1/6.2, the search-requires-acl-filter lint).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (one cross-artifact reference graph; the prime RAG corpus);
    ../../external-insights/04-hard-problems.md §5 (Search is a derived store — it consumes off the bus, never
    reads the owner DB); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it: 0 leak incl.
    COUNT across search).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md §6
    (search indexing granularity — page + significant-block, vector-in-v1, multilingual);
    03-events-contracts-and-glue.md §2.2 (project feeds the index).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 6.3 (declare_indexable — the page
    + block IndexSpec, vector-in-v1, JSONB struct), 6.1/6.2 (query/semantic with the list_objects Filter
    conjoined).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3d" + §2 (rows 6.x).
  - Drills: testing-strategy/01-...-catalogue.md KN-D5 re-confirmed now that search/embed/RAG paths exist (the
    count-leak path is live here); KN-D6 (the Search index is one of the derived stores replay rebuilds).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge:
  - Search glue: declare_indexable(IndexSpec) — two specs (a page doc title+body language-tagged; a
    per-significant-block doc) + vector-in-v1 + JSONB struct fields (6.3); feed project to the index; query/
    semantic with the list_objects Filter conjoined (6.1/6.2, the search-requires-acl-filter lint). Knowledge
    never indexes itself — it projects text, Search consumes off the bus (no cross-DB).
  - FLOOR named: none new. The >5% search-block prune is KQ-10 (measured, parallel) — note it.
- **CONTRACTS TO IMPLEMENT.** 6.3 declare_indexable (owned — the Knowledge projection spec), 6.1/6.2
  query/semantic (consumed — the Filter conjoin). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D5 re-confirmed with the search/embed/RAG paths live: 0 leak incl. COUNT across search/embed/backlink; the
    search-requires-acl-filter lint green is part of the artifact — CI.
  - A reindex-via-search gate: a scoped replay (KN-P20) rebuilds the Search index to parity with live (the search
    leg of KN-D6) — SCHED.
- **TESTS (required).** Unit tests for the declare_indexable spec serialization (page + block specs), the project
  feed, and the query/semantic Filter conjoin (a restricted artifact never appears in results incl. COUNT). The
  KN-D5 re-confirmation over search/embed. The CDC pair for row 6.3. The search Filter conjoin is mandatory-core
  (a search leak is a leak): state the cargo-mutants mutation-score floor.
- **DEFINITION OF DONE.** The search feed declares its IndexSpecs, feeds project, and conjoins the Filter into
  query/semantic; KN-D5 is re-confirmed over search/embed/RAG (0 leak, lint green); the reindex-via-search leg
  contributes to KN-D6; unit + the 6.3 CDC pass; the contract-coverage scanner is green; KQ-10 is noted; the work
  is committed. No gate is weakened.
- **COMMIT.** Header: P-<NNN> M3: search feed (declare_indexable + query/semantic Filter conjoin) (KN-D5
  re-confirm). Body lists: contracts 6.3/6.1/6.2 wired; KN-D5 re-confirmed over search/embed (0 leak); the
  search-requires-acl-filter lint green; KQ-10 noted; the mutation floor stated. Branch first if on default; do
  not push unless asked. End with the workspace Co-Authored-By trailer.

---

### KN-P22 — Notif/humanise glue + watcher rules (the ONE templating surface, no second engine)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3d (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3d", the
  notif/humanise glue).
- **DEPENDS-ON.** KN-P19 (project Display mode + the edge events the notif rules fire on). The M2 Notif prompt
  (humanise 7.3 — the ONE templating surface; define_notif_rule 7.6; the watcher relation).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (one notification inbox); ../../external-insights/01-process-and-quality-doctrine.md §7
    (abstract at the third copy — one templating surface, not a second engine).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/03-events-contracts-and-glue.md
    §1.5 (comments/mentions events → Notif), §2.2 (project Display mode = the humanisation projection Notif uses —
    a routable ArtifactRef + a humanised string, the sole humanise surface, no second template engine).
  - Reconciliation: 00-reconciliation-decisions.md OQ-L (the one humanise surface).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.3 (humanise — the ONE ICU
    templating surface), 7.6 (define_notif_rule + the watcher relation).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3d" + §2 (rows 7.3, 7.6).
  - Drills: testing-strategy/01-...-catalogue.md — NOTIF-D4 (humanised tombstone never leaks a title), inherited
    via project Display mode.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge:
  - Notif glue: declare the watcher relation + define_notif_rule for mentions/comments/shares/watched
    (knowledge.mention.created / knowledge.comment.created / knowledge.access.granted / a watched-page change,
    7.6); feed project Display mode into the ONE humanise ICU-MessageFormat surface (7.3) so "alice mentioned you
    in <Incident runbook>" renders per-viewer — register NO second template engine; living-doc/SLA-style strings
    register here.
  - FLOOR named: none — the notif/humanise glue routes through the one surface. (The comment events the rules fire
    on are produced by KN-P23 — name it as the sibling.)
- **CONTRACTS TO IMPLEMENT.** 7.3 humanise (consumed — Knowledge registers its Display projection into the one
  surface), 7.6 define_notif_rule + watcher (owned — the Knowledge rules + relation). Implement to the frozen
  shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The humanise integration gate: a Knowledge mention/comment renders through the ONE humanise surface per-viewer
    and a confidential subject degrades to a humanised tombstone with the title never leaking (the NOTIF-D4 class,
    inherited); the no-second-template-engine check is the green artifact — CI.
- **TESTS (required).** Unit tests for the watcher-rule firing (each of mentions/comments/shares/watched) and the
  humanise Display projection (per-viewer rendering + the confidential tombstone). The CDC pairs for rows 7.6,
  7.3. State the mutation floor if the rule-firing is core; if not, say so.
- **DEFINITION OF DONE.** The notif/humanise glue routes through the one surface (no second template engine); the
  watcher rules fire on the edge events; a confidential subject humanises to a tombstone (title never leaks); the
  humanise gate emits its dated green; unit + the CDC pairs pass; the contract-coverage scanner is green; the
  comment-event sibling (KN-P23) is named; the work is committed. No gate is weakened.
- **COMMIT.** Header: P-<NNN> M3: notif/humanise glue + watcher rules (the one templating surface). Body lists:
  contracts 7.6/7.3 wired; the humanise-tombstone gate greened; the one-humanise-surface / no-second-template-
  engine discipline held; the comment-event sibling (KN-P23) named. Branch first if on default; do not push unless
  asked. End with the workspace Co-Authored-By trailer.

---

### KN-P23 — KB-native comment threads over the shared #sub grammar (Floor 4: one scheme, two stores with Chat)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3d (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3d", the KB-native
  comment store Floor 4).
- **DEPENDS-ON.** KN-P19 (the #sub comment/thread mints + the edge/anchor model), KN-P01 (myelin-content for the
  comment AST). The M2 Refs prompt (the shared comment/thread #sub grammar 5.7). The shared design-system thread
  primitive (Δ9/OQ-L).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (one comment scheme shared with Chat); ../../external-insights/01-process-and-quality-
    doctrine.md §7 (abstract at the third copy — one comment scheme, two stores).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/03-events-contracts-and-glue.md
    §1.5 (a comment is a sub-artifact #comment-/#thread- — the same grammar as Chat, OQ-L: one scheme, two
    stores); 04-views-cli-and-api.md (the comment affordances).
  - Reconciliation: 00-reconciliation-decisions.md OQ-L (the comments one-scheme-two-stores with Chat).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 5.7 (the #comment-/#thread- #sub
    grammar).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3d" (the KB-native comment store
    Floor 4) + §5 (Floor 4 → Chat-threading consolidation, KQ-9, post-M5).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge:
  - KB-native comment threads over the shared #thread-/#comment- #sub grammar (5.7) + the myelin-content AST + the
    shared design-system thread primitive (Δ9/OQ-L): inline comments anchored to a block/range, emitting
    knowledge.comment.created/.resolved (the events KN-P22's notif rules fire on).
  - FLOOR 4 named: KB-native comment store (one scheme, two stores with Chat). Follow-on: consolidation onto the
    Chat threading primitive + the firehose transport on the real-time-presence trigger (KQ-9) — post-M5; a merge,
    not a rewrite (they already share #sub + content + refs). Name it.
- **CONTRACTS TO IMPLEMENT.** 5.7 the comment/thread #sub mints (owned). Implement to the frozen shape; escalate a
  needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - A comment-anchor gate: a comment anchored to a block/range survives a block move (the #sub stable-id anchor
    holds; 0 dangling comments across a move) — CI.
  - A comment-event gate: creating/resolving a comment emits knowledge.comment.created/.resolved through the
    outbox (the events KN-P22's rules consume) — CI.
- **TESTS (required).** Unit tests for the comment-thread #sub anchoring (anchor survives a move) and the
  comment-event emission. State the mutation floor if the anchoring is core; if not, say so.
- **DEFINITION OF DONE.** KB-native comment threads use the shared #sub grammar + the myelin-content AST + the
  shared thread primitive; comment anchors survive moves; comment events emit through the outbox; the anchor +
  event gates emit their dated green; unit tests pass; the contract-coverage scanner is green; Floor 4 (KB
  comments → Chat consolidation) is named with its KQ-9 follow-on; the work is committed. No gate is weakened.
- **COMMIT.** Header: P-<NNN> M3: KB-native comment threads (shared #sub grammar, Floor 4). Body lists: contract
  5.7 comment/thread mints owned; the comment-anchor + comment-event gates greened; Floor 4 (KB comments) named
  (KQ-9 Chat-threading consolidation, post-M5). Branch first if on default; do not push unless asked. End with the
  workspace Co-Authored-By trailer.

---

### KN-P24 — The Export/Import service (lossless JSON Art. 20 + Markdown/HTML/PDF/CSV + ADF lossy-map import)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3d (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3d", the
  Export/Import service).
- **DEPENDS-ON.** KN-P01 (myelin-content for export fidelity), KN-P02 (the ADF lossy-map for import), KN-P10
  (the block tree/page hierarchy exported). (The GDPR export(subject) holder in KN-P25 reuses this Export service.)
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR portability is an architectural constraint — Art. 20 export);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it: the export/import round-trip is the
    quantified gate).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/06-reconciliation-compliance.md
    (the Export service = the Art. 20 mechanism); 04-views-cli-and-api.md (the export affordances).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 13.2 (the ADF → myelin-content
    import map), 10.1 (export(subject) — the lossless JSON the GDPR holder in KN-P25 reuses this Export service
    for), 13.1 (the content model the export round-trips, render(parse(md))===md).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3d" (the Export/Import service).
  - Drills: testing-strategy/01-...-catalogue.md — the export/import round-trip (content fidelity via KN-D2;
    each ADF lossy node recorded in the import report).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge:
  - The Export/Import service: the lossless JSON export (Art. 20 portability — the mechanism the GDPR
    export(subject) holder reuses in KN-P25), Markdown/HTML/PDF, CSV; the ADF → myelin-content lossy-map import
    (13.2) with the import report recording each lossy conversion.
  - FLOOR named: none — the lossless JSON export is the full Art. 20 mechanism; the lossy-import nodes are named in
    the import report (lossy by source-format limit, not a Myelin floor).
- **CONTRACTS TO IMPLEMENT.** 13.2 the ADF import map (consumed — the import path), 10.1 export(subject) mechanism
  (owned — the Export service the holder reuses). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - An export/import round-trip gate: a doc exported to lossless JSON and re-imported is byte-faithful for the
    content model (render(parse(md))===md holds across export/import); the ADF import records each lossy node in
    the import report (the round-trip + import-report telemetry is the green artifact) — CI.
- **TESTS (required).** Unit tests for the lossless JSON export round-trip, the Markdown/HTML/PDF/CSV exporters,
  and the ADF lossy-map import report. The CDC pair for rows 13.2, 10.1 (the export half). State the cargo-mutants
  mutation-score floor for the export round-trip module if mandatory-core; if not, say so.
- **DEFINITION OF DONE.** The Export/Import service round-trips losslessly (JSON Art. 20) and records ADF lossy
  conversions; the multi-format exporters work; the round-trip gate emits its dated green; unit + the CDC pairs
  pass; the contract-coverage scanner is green; the work is committed. No gate is weakened.
- **COMMIT.** Header: P-<NNN> M3: Export/Import service (Art. 20 lossless JSON + multi-format + ADF import). Body
  lists: contracts 13.2/10.1(export) wired; the export/import round-trip gate greened; the lossless JSON the GDPR
  holder (KN-P25) reuses. Branch first if on default; do not push unless asked. End with the workspace
  Co-Authored-By trailer.

---

### KN-P25 — The PersonalDataHolder{locate/export/rectify/restrict} + the #[personal_data] classify-derive tags

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3e (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3e", the GDPR
  holder ops + the classify tags; the per-subject DEK crypto-shred erase floor is KN-P26).
- **DEPENDS-ON.** KN-P04 (the holder auto-registers when the store opens), KN-P21 (the Search feed locate uses for
  free-text best-effort; restrict stops emitting to Search), KN-P24 (the Export service the holder's export
  reuses). The M1 GDPR prompts (PersonalDataHolder trait 10.1, the #[personal_data] classify-derive 10.2, the ONE
  platform erasure posture 10.9). The M2 Bus prompt (the *.erased tombstone 2.7 — referenced; erase is KN-P26).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe & EU-sovereign by construction — data subject rights are architectural);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it: locate/restrict are quantified —
    restrict = 0 emissions to Search/Agents/OLAP/Notif for the subject).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/03-events-contracts-and-glue.md
    §6 (the PersonalDataHolder: locate/export/rectify/restrict/erase — this prompt ships locate/export/rectify/
    restrict; erase §6.1 is KN-P26); 06-reconciliation-compliance.md (the restrict posture into OLAP per 11.6).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 10.1 (PersonalDataHolder{locate/
    export/rectify/restrict/erase} — the non-erase ops here), 10.2 (#[personal_data] tags + the
    no-untagged-personal-data lint), 10.9 (the ONE platform erasure posture — referenced).
  - Reconciliation: 00-reconciliation-decisions.md X-7 (the pseudonymous-by-default + the one erasure posture).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3e" + §2 (rows 10.1/10.2/10.9).
  - Drills: testing-strategy/01-...-catalogue.md — the no-untagged-personal-data lint gate; the restrict gate (a
    restricted subject is excluded from indexing/agent-use(RAG)/analytics/notifications); KN-D4 erase is KN-P26.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge's gdpr module:
  - The PersonalDataHolder locate/export/rectify/restrict impl (10.1) across blocks, rows, history, mentions,
    authorship: locate (structured PII reliably + free-text best-effort via Search, flagged); export (reuse the
    KN-P24 Export service — lossless JSON, Art. 20); rectify (a structured value + best-effort free-text span
    tombstone); restrict (exclude the subject from indexing/agent-use(RAG)/analytics/notifications — stop emitting
    to Search/Agents/OLAP/Notif, mark rows/blocks restricted, the restriction flowing into OLAP per 11.6).
  - The #[personal_data(category, role, basis, retention, erasure, subject_locator)] classify-derive tags (10.2)
    on the Knowledge schema (person fields, mention nodes, author/edit attribution, free-text body, trace
    authorship) so the no-untagged-personal-data lint is green.
  - FLOOR named: none for locate/export/rectify/restrict — these are complete. The erase op (the structural
    crypto-shred floor) is KN-P26 — name it as the immediate follow-on (the holder is not done until erase ships).
- **CONTRACTS TO IMPLEMENT.** 10.1 PersonalDataHolder locate/export/rectify/restrict (owned — the non-erase ops),
  10.2 the #[personal_data] tags (consumed — applied to Knowledge types), 10.9 the posture (referenced). Implement
  to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - A restrict gate: a restricted subject is excluded from indexing/agent-use(RAG)/analytics/notifications — 0
    emissions to Search/Agents/OLAP/Notif for the subject after restrict (the restriction-leak counter = 0 is the
    green artifact) — CI.
  - The no-untagged-personal-data lint green on the Knowledge schema (0 untagged PII fields; red on a deliberately
    untagged fixture) — CI.
- **TESTS (required).** Unit tests for locate (structured reliable + free-text flagged best-effort), export (reuse
  the Export service), rectify (structured + span tombstone), and restrict (suppression across all four sinks).
  The CDC pair for row 10.1 (the non-erase ops). State the cargo-mutants mutation-score floor for the restrict
  suppression if mandatory-core; if not, say so (the crypto-shred floor is in KN-P26).
- **DEFINITION OF DONE.** The holder implements locate/export/rectify/restrict; restrict suppresses across all
  four sinks (0 emissions); the no-untagged-personal-data lint green; the restrict gate emits its dated green;
  unit + the 10.1 CDC pass; the contract-coverage scanner is green; the erase follow-on (KN-P26) is named; the
  work is committed. No gate is weakened.
- **COMMIT.** Header: P-<NNN> M3: PersonalDataHolder locate/export/rectify/restrict + #[personal_data] tags. Body
  lists: contract 10.1 (non-erase ops) owned, 10.2 tags applied; the restrict gate greened (0 emissions for a
  restricted subject); the no-untagged-personal-data lint green; the erase crypto-shred floor (KN-P26) named.
  Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### KN-P26 — The erase structural floor: per-subject DEK crypto-shred + pseudonym shred + tombstone/embedding purge (KN-D4)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3e (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3e", the erase op
  — the per-subject DEK crypto-shred structural floor, the hardest GDPR surface).
- **DEPENDS-ON.** KN-P25 (the holder locate/restrict the erase composes), KN-P11 (the op-log/snapshots the
  crypto-shred reaches), KN-P19 (the backlinks tombstoned on erase), KN-P21 (the embeddings purged via the search
  feed). The M1 Storage/KMS prompts (the per-subject DEK hierarchy 11.3/11.4 — the crypto-shred substrate). The
  M1 GDPR prompts (the erasure ledger 10.8, the ONE platform erasure posture 10.9). The M2 Bus prompt (the
  *.erased tombstone 2.7). The M1 Identity prompt (the pseudonym-map shred 4.8).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe & EU-sovereign by construction);
    ../../external-insights/04-hard-problems.md §1 (erasure-vs-immutability — you cannot delete a CAS/CRDT op; you
    destroy its key; the structural floor + the documented residual);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it: 0 recoverable PII incl. vectors incl.
    backups is the quantified gate).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/03-events-contracts-and-glue.md
    §6.1 (the erasure algorithm — the structural floor: pseudonym-map shred + per-subject DEK crypto-shred (one
    DEK per (subject, tenant), CR-I, O(subjects with inline PII) not O(blocks)) + structural tombstoning +
    embeddings purged; the residual by reference to 10.9, NOT restated); 05-hard-problems.md §6 (erasure depth);
    06-reconciliation-compliance.md §8 (the residual posture).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 10.1 (the erase op), 11.4 (the
    per-subject DEK crypto-shred), 4.8 (the pseudonym map shred), 2.7 (the *.erased tombstone), 10.9 (the ONE
    platform erasure posture — instantiated by reference), 10.8 (the erasure ledger the receipt links into).
  - Reconciliation: 00-reconciliation-decisions.md X-7 (the one erasure posture).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3e" + §2 (rows 10.1/11.4) + §5 (the
    free-text PII structural floor → residual per 10.9).
  - Drills: testing-strategy/01-...-catalogue.md KN-D4 (erase a subject → structured PII purged/pseudonymised,
    free-text under a per-subject DEK crypto-shredded — unrecoverable in op-log/snapshots/backups, embeddings
    purged, backlinks tombstoned; 0 recoverable structured PII incl. vectors; residual per 10.9).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge's gdpr module:
  - The erase op (10.1) + the erasure algorithm structural floor (§6.1): (1) pseudonym-map shred (attribution is
    the opaque principal_id, never PII; erasing the <pseudonym>@<tenant>.noreply map makes the id un-resolvable,
    4.8); (2) per-subject DEK crypto-shred (free-text blocks/ops holding the subject's PII are envelope-encrypted
    under a per-subject DEK class subject:<id>, 11.4; erasure destroys the key → the ciphertext in op-log,
    snapshots, AND backups is unrecoverable; ONE DEK per (subject, tenant), applied only to PII-bearing classes,
    CR-I); (3) structural tombstoning (mentions/backlinks tombstone via the *.erased consumer; the Search + vector
    index purges in lockstep — embeddings of personal data are personal data; published pages unpublish + CDN/cache
    purge); returns a receipt hash-linked into the audit log (10.8).
  - FLOOR named: the structural floor is fully built + reliable for structured/self-authored PII. RESIDUAL:
    third-party free-text PII (a name typed by someone else into that other person's content) is under the
    author's DEK, handled per the ONE platform posture (10.9, [OPEN — LEGAL], KQ-8 — counsel/DPO ratify in one
    statement; never indexed/agent-readable/in-analytics for a restricted subject; the structural floor ships
    regardless). Instantiate by reference, do NOT restate. Name it.
- **CONTRACTS TO IMPLEMENT.** 10.1 the erase op (owned), 11.4 the per-subject DEK crypto-shred (consumed — the key
  destroy), 4.8 the pseudonym-map shred (consumed), 2.7 the *.erased tombstone (consumed/produced), 10.8 the
  erasure ledger (consumed — the receipt), 10.9 the posture (consumed by reference). Implement to the frozen
  shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D4 → erase a subject → structured PII purged/pseudonymised, free-text under the per-subject DEK
    crypto-shredded (key destroyed → unrecoverable in op-log/snapshots/backups), embeddings purged, backlinks
    tombstoned; 0 recoverable structured PII incl. vectors; the residual covered by the platform posture 10.9;
    telemetry: holder erase receipts, vector-tombstone lag, key-shred count (bounded: one key per subject) — the
    0-recoverable counter is the dated green artifact — SCHED.
- **TESTS (required).** Unit tests for the per-subject DEK envelope-encrypt → key-destroy → ciphertext-
  unrecoverable path, the pseudonym-map shred, and the backlink/embedding tombstone-in-lockstep. The KN-D4 drill
  as a CHAINED scenario (subject authors PII → erase → assert unrecoverable across op-log/snapshots/backups +
  vectors purged). The CDC pair for row 10.1 (the erase op). The crypto-shred path is mandatory-core
  (unrecoverability is the property): state the cargo-mutants mutation-score floor.
- **DEFINITION OF DONE.** The erase op crypto-shreds free-text under the per-subject DEK (unrecoverable in
  backups), pseudonymises attribution, tombstones backlinks, purges embeddings, returns a receipt into the audit
  ledger; KN-D4 emits its dated green (0 recoverable PII incl. vectors, measured); the residual is handled by
  reference to 10.9 (not restated); unit + the chained KN-D4 drill + the 10.1 CDC pass; the contract-coverage
  scanner is green; the residual ([OPEN — LEGAL], KQ-8) is named; the work is committed. No gate is weakened; the
  erasure drill verifies real backups.
- **COMMIT.** Header: P-<NNN> M3: erase structural floor — per-subject DEK crypto-shred (KN-D4). Body lists:
  contract 10.1 erase owned, 11.4 per-subject DEK crypto-shred wired, 4.8 pseudonym shred, 2.7 *.erased, 10.8
  receipt; KN-D4 greened (0 recoverable structured PII incl. vectors, measured); the free-text residual named by
  reference to 10.9 ([OPEN — LEGAL], KQ-8); the structural floor ships regardless; the mutation floor stated.
  Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### KN-P27 — Agent governance: Knowledge ToolDefs + EffectApi apply + HITL withhold + per-effect idem_key + reserve/settle (KN-D11)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3e (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3e", agent
  governance; the AG-7 content-addressed agent-trace holder is KN-P28 — together they complete the master M3→M4
  exit for Knowledge).
- **DEPENDS-ON.** KN-P07 (agent edits flow through the SAME collab protocol as humans), KN-P14 (the per-op
  authority the agent op passes), KN-P16 (the permission filtering on agent reads). The M2 agent-fabric prompts
  (ToolSurface::register_tool with the frozen requires_approval defaults 8.1, EffectApi::apply plan-then-apply
  8.2, AgentRuntime::step --use-mock 8.3, ToolHands::exec the unified sandbox 8.4) with AG-D4 (sandbox-escape)
  GREEN. The M2 durable-workflow prompts (SCHEDULE_AND_RUN_JOB 9.2, the durable HITL signal + per-effect idem_key
  9.1/9.4, the timer wheel 9.3). The M1 Storage prompt (reserve/settle 11.7).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native from the ground up; mock agents only during development — --use-mock);
    ../../external-insights/03-agent-native-fabric.md (the four uniform guarantees; plan-then-apply; HITL
    withhold); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it: 0 ungoverned mutation / 0
    mutation before approval / 0 double-apply), §8 (human sign-off is the bottleneck — publish/confidential edits
    are decision-shaped, HITL-gated).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/03-events-contracts-and-glue.md
    §5.1 (the ToolDef registrations + the frozen requires_approval defaults — publish/confidential=yes, draft/
    comment=no; side-effecting tools go through EffectApi::apply; HITL withhold returns Denied + does not mutate;
    the approval card resumes via a durable signal with the per-effect idem_key rule); §7 (reserve/settle —
    Knowledge is not spend-bearing; an agent write passes the Fabric's reserve/settle gate);
    02-internals-and-algorithms.md §9 (agent edits flow through the same collab protocol; the four uniform sandbox
    guarantees by construction).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 8.1 (ToolDef + the frozen
    requires_approval defaults), 8.2 (EffectApi::apply), 8.4 (the unified sandbox, AG-D4 drilled in M2), 9.1/9.4
    (the per-effect idem_key + durable HITL signal), 9.2 (SCHEDULE_AND_RUN_JOB), 11.7 (reserve/settle).
  - Reconciliation: 00-reconciliation-decisions.md X-6 (the four uniform guarantees + the frozen approval
    defaults), OQ-F (the per-effect idem_key — a double-click is one approval, a partial approval is well-defined).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3e" + §2 (rows 8.1/8.2/9.x/11.7).
  - Drills: testing-strategy/01-...-catalogue.md KN-D11 (an agent edits a doc via EffectApi → attributed
    "suggested by agent"; a consequential edit publish/confidential is HITL-withheld (returns Denied, no mutation)
    until approval; a double-click is one approval; denied effects return ordinary tool errors; the run passed
    reserve/settle; 0 ungoverned mutation, 0 mutation before approval, 0 double-apply).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge's agent module:
  - Register Knowledge ToolDefs into the one catalogue (8.1) with the frozen requires_approval defaults
    (knowledge.search/page.read/summarise = read, no; page.create/append/comment/draft = mutate, no; row.upsert =
    no, but YES for a PII-bearing database; page.publish / edit(confidential_page) = mutate, YES; page.turn_into_issues
    = YES, inherits Issues' default). Side-effecting tools go through EffectApi::apply (8.2, plan-then-apply:
    schema→capability→delegation→tenant→budget→HITL→apply→meter) → the Knowledge public endpoint as the agent
    principal → the collab protocol (KN-P07) with "suggested by agent" attribution. The four uniform sandbox
    guarantees hold by construction (8.4 — AG-D4 already green from M2; any compute the tool runs is the CI
    runner's kind=agent job).
  - HITL withhold (8.2/AG-8): a gated tool not in the approved set returns Denied and does NOT mutate; the
    approval card surfaces in Chat (live cost estimate) and resumes the run via a durable signal — the per-effect
    idem_key rule (card_id single, card_id:<effect_idx> for a batch/partial approval, 9.1/9.4, OQ-F) makes a
    double-click one approval and a partial approval well-defined. Denied = ordinary tool error (no privileged
    fallback). Scheduled living-doc automations as SCHEDULE_AND_RUN_JOB jobs (9.2); reserve/settle on every agent
    run (11.7 — the Fabric's bookends; Knowledge tools are ordinary metered effects).
  - FLOOR named: none — agent governance is the full v1 surface. The mock runtime (--use-mock) is the platform
    floor (the real LlmAgentRuntime is the post-M5 config/impl swap, owned by the Fabric, not Knowledge) — note
    it. (The AG-7 trace holder the run writes into is KN-P28 — name it.)
- **CONTRACTS TO IMPLEMENT.** 8.1 the Knowledge ToolDefs + frozen approval defaults (owned), 8.2 EffectApi::apply
  (consumed — the apply path), 9.1/9.4 the durable HITL signal + idem_key (consumed), 9.2 SCHEDULE_AND_RUN_JOB
  (consumed), 11.7 reserve/settle (consumed). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D11 → an agent edit via EffectApi is attributed "suggested by agent"; a publish/confidential edit is
    HITL-withheld (returns Denied, no mutation) until approval; a double-click is one approval (per-effect
    idem_key); denied effects are ordinary tool errors; the run passed reserve/settle; 0 ungoverned mutation, 0
    mutation before approval, 0 double-apply; the gate-state + denial-counter + idem-key-dedup telemetry is the
    dated green artifact — CI.
- **TESTS (required).** Unit tests for the ToolDef registration (the frozen approval defaults), the EffectApi
  apply path (attribution via the collab protocol), the HITL withhold (Denied, no mutation), the per-effect
  idem_key (double-click = one apply; partial approval well-defined), and reserve/settle gating. The KN-D11 drill
  as a CHAINED scenario (agent plans → consequential effect → withheld → approve → applied once across a
  double-click). The CDC pairs for rows 8.1, 8.2. The HITL/idem_key path is mandatory-core: state the
  cargo-mutants mutation-score floor.
- **DEFINITION OF DONE.** The Knowledge ToolDefs register with the frozen defaults; agent edits flow through
  EffectApi → the collab protocol with attribution; the HITL withhold + per-effect idem_key + reserve/settle hold;
  KN-D11 emits its dated green (0 ungoverned/0 pre-approval/0 double-apply); unit + the chained drill + the CDC
  pairs pass; the contract-coverage scanner is green; the mock-runtime floor + the AG-7 trace follow-on (KN-P28)
  are noted; the work is committed. No gate is weakened.
- **COMMIT.** Header: P-<NNN> M3: agent governance (ToolDefs + EffectApi + HITL + idem_key + reserve/settle)
  (KN-D11). Body lists: contracts 8.1/8.2/9.1/9.4/9.2/11.7 wired; KN-D11 greened (0 ungoverned mutation, 0
  mutation before approval, 0 double-apply); the four uniform guarantees held by construction; the mock-runtime
  floor + AG-7 trace follow-on (KN-P28) noted; the mutation floor stated. Branch first if on default; do not push
  unless asked. End with the workspace Co-Authored-By trailer.

---

### KN-P28 — The AG-7 content-addressed agent-trace holder (erasable, distinct from the audit log) (KN-D12)

- **BAND.** M3.
- **ROADMAP MILESTONE.** KN-M3e (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3e", the AG-7
  agent-trace holder — completes the master M3→M4 exit for Knowledge alongside KN-P27).
- **DEPENDS-ON.** KN-P27 (the agent runs that write traces), KN-P26 (the crypto-shred the trace holder reuses for
  erasure), KN-P11 (the block model the content-addressed trace reuses). The M2 agent-fabric prompt (the
  content-addressed agent-trace holder seam 8.8).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agents are first-class citizens; an agent trace is erasable personal data);
    ../../external-insights/03-agent-native-fabric.md (the trace is a holder, distinct from the tamper-evident
    audit log); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it: 0 recoverable PII in
    traces, attribution intact).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/03-events-contracts-and-glue.md
    §5.2 (the AG-7 content-addressed agent-trace holder — accept a content-addressed trace write reusing the block
    model, an erasable holder, distinct from the audit log).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 8.8 (the AG-7 content-addressed
    agent-trace holder), 10.1 (the trace holder is an erasable PersonalDataHolder).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M3e" + §2 (row 8.8).
  - Drills: testing-strategy/01-...-catalogue.md KN-D12 (erase a subject → content-addressed agent traces
    crypto-shredded/purged, attribution falls back to the pseudonym; 0 recoverable PII in traces, attribution
    intact).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge's agent module:
  - The AG-7 content-addressed agent-trace holder (8.8): write_agent_trace(run_id, content, actor) accepts a
    content-addressed (BLAKE3) trace write reusing the block model (no new schema), returns run.trace_ref, and
    registers it as an erasable PersonalDataHolder (distinct from the tamper-evident audit log) — erasing a
    subject (via the KN-P26 crypto-shred) shreds their trace content, attribution falls back to the pseudonym.
  - FLOOR named: none — the trace holder is the full v1 surface.
- **CONTRACTS TO IMPLEMENT.** 8.8 the AG-7 trace holder (owned — a Knowledge deliverable), 10.1 the trace as an
  erasable holder (owned). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D12 → erase a subject → their content-addressed agent traces crypto-shredded/purged, attribution falls
    back to the pseudonym; 0 recoverable PII in traces, attribution intact; the trace-shred + attribution-fallback
    telemetry is the dated green artifact — SCHED.
  - A content-address gate: a trace write is content-addressed (BLAKE3) and idempotent (the same content writes
    once); distinct from the audit log (the audit log is unaffected by a trace erase) — CI.
- **TESTS (required).** Unit tests for the AG-7 trace write (content-addressed, reuses the block model) +
  holder registration, and the trace-erasure path (shred → attribution falls back to the pseudonym). The KN-D12
  trace-erasure drill. The CDC pair for row 8.8. State the cargo-mutants mutation-score floor for the trace-shred
  path if mandatory-core (it reuses the KN-P26 crypto-shred core); if not, say so.
- **DEFINITION OF DONE.** The AG-7 trace holder accepts content-addressed writes and is erasable; KN-D12 emits its
  dated green (0 recoverable trace PII, attribution intact); the content-address gate is green; unit + the KN-D12
  drill + the 8.8 CDC pass; the contract-coverage scanner is green; the work is committed. This — with KN-P27 —
  completes the master M3→M4 exit for Knowledge (with KN-D3/KN-D1/KN-D2/KN-D7/KN-D5/KN-D13/KN-D4/KN-D11 already
  green). No gate is weakened.
- **COMMIT.** Header: P-<NNN> M3: AG-7 content-addressed agent-trace holder (KN-D12). Body lists: contract 8.8/
  10.1 owned; KN-D12 greened (0 recoverable trace PII, attribution intact); the content-address gate greened; the
  master M3→M4 Knowledge exit complete (alongside KN-P27). Branch first if on default; do not push unless asked.
  End with the workspace Co-Authored-By trailer.

---

### KN-P29 — The Yrs CRDT promotion over the unchanged transport + the online engine_promote migration (KN-D1 re-green)

- **BAND.** M5.
- **ROADMAP MILESTONE.** KN-M5 (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M5", the Yrs CRDT
  promotion half; cross-cell collab is KN-P30, materialisation/surge/E2E are KN-P31..KN-P33).
- **DEPENDS-ON.** KN-P07 (the resume-cursor transport the CRDT slots into), KN-P13 (the CAS floor + the
  conflict-rate trigger metric), KN-P10/KN-P11 (the block tree + LexoRank the move-CRDT replaces; the snapshots
  the Yrs seed loads from). M4 green (all five subsystems exist; the deterministic correctness drills green).
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
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 3.5 (the transport the CRDT slots
    into — unchanged).
  - Reconciliation: 00-reconciliation-decisions.md OQ-I (multi-cell after single-cell — referenced; cross-cell is
    KN-P30).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M5" (the CRDT follow-on; KQ-1 the CRDT
    promotion timing) + §5 (the floors → follow-ons table).
  - Drills: testing-strategy/01-...-catalogue.md KN-D1 re-green across the engine_promote boundary (0 lost/0 dup
    survives the swap).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge's collab/merge module
  (the Layer-3 swap):
  - The Yrs CRDT engine slotting into the M2/M3a resume-cursor transport as a Layer-3 swap (the op-log carries Yrs
    update bytes; the transport, resume cursor, idempotent apply, and op-log are unchanged): a per-block content
    CRDT (Y.Text/Y.XmlFragment) for inline runs + a tree/move CRDT (Kleppmann move op) for block structure (the
    hybrid granularity); rich-text marks per Peritext. The server stays a "dumb relay + persistence + authority"
    (Yrs being Rust-native keeps it in-process).
  - The online CAS→CRDT migration per-doc, no stop-the-world (arch §3.4): quiesce-lite snapshot → deterministic
    Yrs seed from the snapshot → a single engine_promote op at the next op_seq (from there forward Yrs bytes,
    before it CAS deltas) → in-flight CAS edits straddling the cutover reconcile via the last CAS conflict check;
    reversible from the pre-cutover snapshot. Trigger: the first true concurrent-edit conflict, measured via the
    KN-P13 CAS-conflict-rate metric (KQ-1). Full offline-first arrives here.
  - The LexoRank-under-CRDT interaction (arch §3.5): the move-CRDT's list type owns sibling ordering; order_key
    becomes a derived OLTP-index hint recomputed from CRDT state; the bespoke jitter/rebalance retires.
  - FLOOR resolved: this is the named CRDT follow-on to the KN-P13 CAS floor + the full-offline-first follow-on.
    Editable-in-place synced blocks (KQ-6, KN-P12's floor) is enabled by the CRDT — pull into here or name
    post-M5. State which.
- **CONTRACTS TO IMPLEMENT.** 3.5 the transport (consumed — unchanged; the CRDT rides it). Implement to the frozen
  shape; escalate a needed change.
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
  migration is deterministic + reversible; concurrent same-block edits converge; KN-D1 re-greens across the
  engine_promote boundary (0 lost/0 dup, dated); the convergence gate is green; unit + the across-boundary drill +
  the convergence test pass; the contract-coverage scanner is green; the CAS→CRDT + offline-first floors are
  resolved (KQ-6 disposition stated); the work is committed. No gate is weakened; the re-green drill runs a real
  promotion + kill.
- **COMMIT.** Header: P-<NNN> M5: Yrs CRDT promotion + engine_promote migration (KN-D1 re-green). Body lists: the
  CRDT (per-block content + tree/move) slotted into the unchanged transport; the online per-doc engine_promote
  migration; KN-D1 re-greened across the boundary (0 lost/0 dup); the CAS→CRDT + offline-first floors resolved;
  KQ-6 disposition stated; the mutation floor stated. Branch first if on default; do not push unless asked. End
  with the workspace Co-Authored-By trailer.

---

### KN-P30 — Cross-cell collab: true cross-cell op fan-out over the PII-free CrossCellPointer bridge

- **BAND.** M5.
- **ROADMAP MILESTONE.** KN-M5 (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M5", cross-cell
  collab — the single-cell→cross-cell floor follow-on).
- **DEPENDS-ON.** KN-P29 (the CRDT op fan-out cross-cell extends), KN-P05 (the (tenant,region) single-cell pin
  this lifts). The M5 control-plane multi-cell prompt (the cross-cell PII-free CrossCellPointer bridge 12.6 going
  live).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale; EU-sovereign by construction — the bridge is PII-free);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it: cross-cell fan-out with 0 PII over the
    bridge).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md
    §3.3/§3.4 (the CRDT op fan-out the cross-cell path extends); 06-reconciliation-compliance.md (the cell-local
    residency resolution stays cell-local).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 12.6 (the cross-cell PII-free
    pointer bridge — frame frozen, live in M5).
  - Reconciliation: 00-reconciliation-decisions.md OQ-I (multi-cell after single-cell — cross-cell op fan-out over
    the bridge).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M5" (cross-cell collab; KQ-7) + §5 (the
    single-cell → true cross-cell op fan-out floor, over the PII-free pointer bridge 12.6).
  - Drills: testing-strategy/01-...-catalogue.md — the cross-cell fan-out leg (a multi-cell tenant's doc op fans
    out cross-cell; 0 PII over the bridge; resolution cell-local).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge's collab module:
  - Cross-cell collab (KQ-7/OQ-I/12.6): true cross-cell op fan-out for a multi-cell tenant over the PII-free
    CrossCellPointer bridge (owned by control-plane; the Knowledge collab contracts are cell-agnostic so this
    extends without a rewrite). v1 pinned a doc's session to one cell (KN-P05's floor); this lifts the pin to
    cross-cell fan-out while resolution stays cell-local (residency by construction — only PII-free pointers cross
    the bridge).
  - FLOOR resolved: this is the named cross-cell follow-on to the KN-P05 single-cell floor. Name it resolved.
- **CONTRACTS TO IMPLEMENT.** 12.6 the cross-cell pointer bridge (consumed — cross-cell fan-out). Implement to the
  frozen shape; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - A cross-cell fan-out gate: a multi-cell tenant's doc op fans out to collaborators in other cells over the
    bridge with 0 PII crossing the bridge (only PII-free pointers); the cross-cell-PII counter = 0 is the dated
    green artifact — SCHED.
  - A residency gate: resolution stays cell-local (a doc's content never leaves its residency cell — only the
    PII-free pointer crosses) — CI.
- **TESTS (required).** Unit tests for the cross-cell op fan-out over the bridge (PII-free pointer only) and the
  cell-local resolution. The cross-cell fan-out drill on the failure-injection harness (a multi-cell tenant). The
  CDC pair for row 12.6 (Knowledge's consumer half). The cross-cell PII-free discipline is mandatory-core (a PII
  leak across the bridge is a sovereignty breach): state the cargo-mutants mutation-score floor.
- **DEFINITION OF DONE.** Cross-cell op fan-out works over the PII-free bridge (0 PII crossing); resolution stays
  cell-local; the fan-out + residency gates emit their dated green; unit + the fan-out drill + the 12.6 CDC pass;
  the contract-coverage scanner is green; the single-cell→cross-cell floor is resolved; the work is committed. No
  gate is weakened.
- **COMMIT.** Header: P-<NNN> M5: cross-cell collab over the PII-free CrossCellPointer bridge. Body lists:
  contract 12.6 consumed (cross-cell fan-out); the cross-cell fan-out gate greened (0 PII over the bridge); the
  residency gate greened; the single-cell→cross-cell floor resolved; the mutation floor stated. Branch first if on
  default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### KN-P31 — Facet/rollup materialisation + the object-store BlobStore swap (KN-D9/D10 at scale, KN-P17/P18/P05/P11 floors resolved)

- **BAND.** M5.
- **ROADMAP MILESTONE.** KN-M5 (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M5", the
  materialisation follow-ons + the object-store swap).
- **DEPENDS-ON.** KN-P17 (the JSONB+GIN floor + the measured >5% facet trigger), KN-P18 (the read-time
  formula/rollup + the measured rollup-too-slow trigger), KN-P05/KN-P11 (the fs-backed BlobStore floor the
  object-store swap replaces). The M5 Storage prompt (the object-store BlobStore 11.2; the OLAP read store 11.6).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale from day 1); ../../external-insights/01-process-and-quality-doctrine.md §7
    (the measured-promotion trigger — materialise when measured, not speculatively), §3 (prove-it: p99 within
    budget after promotion).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md
    §4.1 (the measured-hot facet → generated index promotion) + §4.2 (the per-rollup materialised aggregate fed
    off the bus → the OLAP read store 11.6).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 6.3 (the >5% facet-promotion
    threshold — acted on here), 11.6 (the OLAP read store the rollup materialisation feeds), 11.2 (the
    object-store BlobStore — the one-line swap from the fs floor).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M5" (per-facet/per-rollup
    materialisation; object-store BlobStore) + §5 (the floors → follow-ons table).
  - Drills: testing-strategy/01-...-catalogue.md KN-D9/KN-D10 re-confirmed at world scale (the promotion triggers
    measured + acted on).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge:
  - Per-facet materialisation: promote a facet past the frozen >5% view-execution threshold (6.3, measured in
    KN-P17) to a generated/expression-column index via the expand→backfill→contract online-migration path
    (knowledge.database.schema.changed drives the feeder). Per-rollup materialisation: promote a rollup measured
    too slow (KN-P18's KN-D10 telemetry) to an incrementally-maintained materialised aggregate fed off the bus
    (knowledge.row.updated deltas → the OLAP read store 11.6) — per-rollup, not wholesale.
  - The object-store BlobStore swap (11.2): move media + CRDT snapshots from the fs-backed floor (KN-P05/KN-P11)
    to the S3-compatible object store (the one-line swap), residency-pinned, content-addressed (BLAKE3).
  - FLOOR resolved: this ships the named per-facet (KN-P17 Floor 1) + per-rollup (KN-P18 Floor 2) materialisation
    follow-ons and the object-store follow-on (KN-P05/KN-P11 fs-BlobStore floor). Name them resolved.
- **CONTRACTS TO IMPLEMENT.** 6.3 the facet-promotion (consumed — acted on), 11.6 the OLAP read store (consumed —
  the rollup materialisation feeder), 11.2 the object-store BlobStore (consumed — the swap). Implement to the
  frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D9 / KN-D10 re-confirmed at world scale with the materialisation acted on (p99 within budget after
    promotion; the post-promotion p99 telemetry is the green artifact) — SCHED.
  - An object-store-parity gate: content-addressed put/get on the object store is byte-identical to the fs floor
    (the swap is behaviour-preserving) — CI.
- **TESTS (required).** Unit tests for the facet-promotion expand→backfill→contract path, the incremental rollup
  aggregate maintained off the bus, and the object-store BlobStore swap (content-addressed put/get parity with the
  fs floor). The KN-D9/D10 at-scale re-runs on the failure-injection harness. The incremental-rollup maintenance is
  mandatory-core: state the cargo-mutants mutation-score floor.
- **DEFINITION OF DONE.** The per-facet + per-rollup materialisation are promoted where measured (p99 within budget
  after); the object-store BlobStore swap is live (parity with the fs floor); KN-D9/D10-at-scale + the parity gate
  emit their dated green; unit + the at-scale re-runs pass; the contract-coverage scanner is green; the
  materialisation + object-store floors are named resolved; the work is committed. No gate is weakened; the budgets
  are read from the thresholds file.
- **COMMIT.** Header: P-<NNN> M5: facet/rollup materialisation + object-store BlobStore swap (KN-D9/D10 at scale).
  Body lists: the per-facet generated index + the per-rollup materialised aggregate (KN-P17/KN-P18 floors
  resolved); the object-store BlobStore swap (KN-P05/KN-P11 fs floor resolved); KN-D9/D10 re-confirmed at scale;
  the object-store-parity gate greened; the mutation floor stated. Branch first if on default; do not push unless
  asked. End with the workspace Co-Authored-By trailer.

---

### KN-P32 — The all-hands-doc surge controls + the concurrent-same-gap LexoRank storm (KN-D8 + the F6 leg)

- **BAND.** M5.
- **ROADMAP MILESTONE.** KN-M5 (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M5", the
  all-hands-doc surge + the LexoRank insert storm).
- **DEPENDS-ON.** KN-P29 (the CRDT under the surge; the move-CRDT under the LexoRank storm), KN-P07 (the transport
  the op fan-out rides). The M0/M2 protected-human-lane shed-order prompt (1.11).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale from day 1); ../../external-insights/01-process-and-quality-doctrine.md §3
    (prove-it: the surge shed budget; observability is the pass).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md
    §3.5 (the concurrent-same-gap LexoRank insert storm — no key-collision reorder, bounded rebalance, now under
    the move-CRDT); 05-hard-problems.md (the hot-doc thundering-herd discipline).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 1.11 (the protected-human-lane shed
    order — viewers shed before editors, agents before humans).
  - Reconciliation: 00-reconciliation-decisions.md OQ-K (the per-surface storm profiles / the shed budget).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M5" (the all-hands-doc surge — per-doc
    op cap + read-fanout bound + active-editor lane reservation; the LexoRank storm) + the F6 surge family leg.
  - Drills: testing-strategy/01-...-catalogue.md KN-D8 (an all-hands doc with thousands of concurrent
    readers/editors → per-doc op cap + read-fanout bound + active-editor lane reservation hold within budget;
    other tenants unaffected; the concurrent-same-gap LexoRank insert storm → 0 reorder); the F6 surge family leg
    (human lane holds, agent lane sheds 429+Retry-After, cross-tenant impact 0).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge:
  - The all-hands-doc surge controls (KN-D8 / OQ-K / 1.11): a per-doc op in-flight cap + a read-fanout bound + an
    active-editor lane reservation (viewers shed before editors, agents shed before humans) so the op fan-out
    holds within budget and other tenants are unaffected; the concurrent-same-gap LexoRank insert storm handled
    (no key-collision reorder, bounded rebalance — now under the move-CRDT from KN-P29).
  - FLOOR resolved: none new — this hardens the existing transport/CRDT under surge. (Builds on the shed order
    1.11 owned by the substrate.)
- **CONTRACTS TO IMPLEMENT.** 1.11 the shed order (consumed — the surge lane reservation). Implement to the frozen
  shape; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - KN-D8 → an all-hands doc with thousands of concurrent readers/editors → the per-doc op cap + read-fanout
    bound + active-editor lane reservation hold within budget; other tenants unaffected; the concurrent-same-gap
    LexoRank insert storm → 0 reorder; the per-tenant in-flight + op-fanout + rebalance-cost telemetry is the
    dated green — SCHED.
  - The F6 surge family leg for Knowledge: human lane holds, agent lane sheds 429+Retry-After, cross-tenant impact
    0 — SCHED.
- **TESTS (required).** Unit tests for the per-doc op-cap + read-fanout bound + the active-editor lane reservation
  (the shed order: viewers before editors, agents before humans). The KN-D8 surge drill on the failure-injection
  harness at 30× with the LexoRank storm. The surge lane-reservation is mandatory-core: state the cargo-mutants
  mutation-score floor.
- **DEFINITION OF DONE.** The all-hands-doc surge holds within budget (0 reorder, other tenants unaffected); KN-D8
  + the F6 leg emit their dated green; unit + the surge drill pass; the contract-coverage scanner is green; the
  work is committed. No gate is weakened; the surge runs a real 30× storm.
- **COMMIT.** Header: P-<NNN> M5: all-hands-doc surge controls + LexoRank storm (KN-D8 + F6 leg). Body lists: the
  per-doc op cap + read-fanout bound + active-editor lane reservation (shed order 1.11); KN-D8 greened (surge
  within budget, 0 reorder); the F6 Knowledge leg greened; the mutation floor stated. Branch first if on default;
  do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### KN-P33 — Knowledge's legs of the whole-system E2E wedge (E2E-1 PR context pane + E2E-3 spec-to-ship lineage)

- **BAND.** M5.
- **ROADMAP MILESTONE.** KN-M5 (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M5", the E2E wedge
  legs).
- **DEPENDS-ON.** KN-P19 (project — the per-viewer embed resolution), KN-P16 (the permission filtering 0 leak),
  KN-P20 (the cold-reindex == live property), KN-P29 (the CRDT under the E2E scenarios). The M5 whole-system E2E
  wedge prompt (E2E-1 PR context pane, E2E-3 spec-to-ship traceability) — Knowledge supplies its legs.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (one cross-artifact reference graph; the switch-test bar);
    ../../external-insights/01-process-and-quality-doctrine.md §4 (the E2E chained-mutation scenarios — drive the
    whole thing end to end), §3 (prove-it: 0 leak; lineage live==cold; audit tamper detected).
  - Architecture: ../04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md
    (the project per-viewer embed + the TE-7 lineage); the design sketches under design/ (the E2E-1 PR-context
    embed; the E2E-3 spec-to-ship lineage).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 5.6 (project — the per-viewer
    embed), 4.3 (the 0-leak filter), 2.6 (cold-reindex == live).
  - Roadmap: planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M5" (the E2E legs: E2E-1 PR-context
    embed, E2E-3 spec-to-ship lineage).
  - Drills: testing-strategy/01-...-catalogue.md E2E-1 (Knowledge design-doc embed resolves per-viewer, 0 leak) +
    E2E-3 (a Knowledge spec doc → initiative → issues lineage; cold-reindex == live; audit tamper detected).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-knowledge:
  - Knowledge's legs of the whole-system E2E wedge: E2E-1 (a Knowledge design-doc embed in the PR context pane
    resolves per-viewer via project, 0 leak to the unauthorized viewer — a confidential doc → tombstone carrying
    root) and E2E-3 (a Knowledge spec doc → initiative → issues lineage via the TE-7 typed edges; cold-reindex ==
    live via replay; audit tamper detected).
  - FLOOR resolved: none — these are E2E integration legs exercising already-shipped contracts end-to-end.
- **CONTRACTS TO IMPLEMENT.** None new — the E2E legs exercise 5.6 project / 4.3 filter / 2.6 replay /
  5.5 TE-7 lineage end-to-end (already owned). Implement the test harness scenarios; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - E2E-1 green for Knowledge's leg: a Knowledge design-doc embed in the PR context pane resolves per-viewer; 0
    leak to the unauthorized viewer (the tombstone carries root) — SCHED.
  - E2E-3 green for Knowledge's leg: a Knowledge spec doc → initiative → issues lineage is traceable; cold-reindex
    == live; audit tamper detected — SCHED.
- **TESTS (required).** The E2E-1/E2E-3 chained-mutation scenarios against a full cell with mock agents (drive the
  whole thing end to end, chaining mutations mid-flight, not single handlers). No new unit/CDC unless an E2E leg
  surfaces a fix (then that fix gets its own test + a drill). State that no new mutation floor applies (these are
  integration legs) unless a code fix lands.
- **DEFINITION OF DONE.** E2E-1 and E2E-3 emit their dated green for Knowledge's legs (0 leak to the unauthorized
  viewer; lineage live == cold; the tombstone carries root; audit tamper detected); the chained scenarios pass
  against a full cell; the contract-coverage scanner is green; the work is committed. No gate is weakened; the E2E
  legs run real chained mutations.
- **COMMIT.** Header: P-<NNN> M5: Knowledge E2E wedge legs (E2E-1 PR context pane + E2E-3 spec-to-ship lineage).
  Body lists: E2E-1 greened (per-viewer embed, 0 leak, tombstone carries root); E2E-3 greened (spec→initiative→
  issues lineage, cold-reindex==live, audit tamper detected). Branch first if on default; do not push unless
  asked. End with the workspace Co-Authored-By trailer.

---

### KN-P34 — Dogfooding: Myelin's own docs in Knowledge + the switch test driven in a browser

- **BAND.** M6.
- **ROADMAP MILESTONE.** KN-M6 (planning/06-roadmaps/subsystems/knowledge-platform.md §3 "KN-M6", dogfooding +
  the switch test).
- **DEPENDS-ON.** KN-P29..KN-P33 (M5 green — you do not dogfood real team knowledge onto a substrate whose
  restore-verify and DSAR fan-out KN-D4/KN-D6 are not green). The M6 dogfood prompts (Myelin hosts its own
  roadmap/gap-report/scorecard; the self-hosting CI graph green). The truth-up pass (the gate invariant — no red
  earlier gate).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (top-of-the-line UX; the switch test is the done-bar);
    ../../external-insights/01-process-and-quality-doctrine.md §4 (actually try it — the switch test is reached by
    DRIVING the real UI in a browser, not by reading the feature list; a Notion user could move without hitting a
    wall the old tool didn't have), §1 (the truth-up pass — every PROVEN row rests on a dated green KN-D artifact,
    code-wins-over-docs).
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
