# Phase 7 — Prompt Ledger: Issue Tracker (the most cross-coupled consumer subsystem)

> Phase: 07-prompts (per-system file, Phase 7-A). The complete ordered set of implementation prompts that
> operationalize the entire issue-tracker roadmap (planning/06-roadmaps/subsystems/issue-tracker.md, milestones
> pre-work 3.0 (M1/M2) + M4-I1..M4-I8 + M5-I9 + M6-I10) into clean-context, independently-committable coding
> tasks. Built to the template in planning/07-prompts/00-ledger-overview.md §2 (every field present, never
> implicit) and banded to planning/06-roadmaps/00-master-sequencing.md §2 (M0..M6, the gate invariant). Frozen
> architecture (this file OPERATIONALIZES, it does not redesign):
> planning/04-subsystem-architectures/issue-tracker/architecture/ (00..07) +
> planning/04-subsystem-architectures/issue-tracker/design/ (information-architecture / user-flows / wireframes)
> + the build-to contracts in planning/05-refined-shared-systems-architecture/contract-index.md +
> 00-reconciliation-decisions.md (X-1/X-2/X-3/X-4/X-6/X-7, OQ-A/OQ-C/OQ-E/OQ-F/OQ-G/OQ-H/OQ-I/OQ-J/OQ-K/OQ-L).
> Drills: planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
> (ISS-D1..ISS-D14 + the shared families + E2E-1/E2E-2/E2E-3). Plain-text identifiers throughout (no
> backticks-as-emphasis). Markdown only; this file makes no commits. Date: 2026-06-19.
>
> The global P-NNN ids are assigned by the consolidated ledger index (Phase 7-B, 01-ledger-index.md) when these
> per-system prompts are interleaved into the single execution order. Here each prompt carries a stable local
> handle ISS-P<n> so its DEPENDS-ON edges are unambiguous before global numbering; the index rewrites ISS-P<n>
> to its P-NNN. Where a prompt depends on another system's prompt not yet numbered, it names that system's
> milestone (the index resolves it to the P-NNN).
>
> Issue-tracker is a CONSUMER subsystem and the most cross-subsystem-coupled of the five (arch 00 §1): it
> references Git commits/PRs, reads CI's CheckStatus to gate "Done", embeds Knowledge docs, turns Chat messages
> into issues, pages on-call on SLA breach, and is driven by agents. Its bulk lands in M4 (it can only be proven
> once its producers — Git in M3, CI also in M4 — exist and the reactive shared layer Refs/Search/Notif/Workflow/
> Agents in M2 is green). It has a freeze-so-dependents-compile slice in M1/M2 (3.0), world-scale + E2E follow-ons
> in M5, and the dogfood switch test in M6. Two seams sit on branches of the spine: the AG-D4 sandbox-escape GATE
> (upstream, M2 — gates Issues' agent tools) and the X-1 Git↔CI CheckStatus seam (consumed in M4-I7, proven
> end-to-end by GIT-D10/CI-D8 at the M4 exit). The gate invariant binds: no Issues code that writes real data is
> done over a red STOR-D1 (M1, the silent-data-loss floor); no Issues agent tool runs over a red AG-D4 (M2).
>
> Coverage: pre-work 3.0 → ISS-P1 (M1) + ISS-P2 (M2); M4-I1 → ISS-P3 + ISS-P4; M4-I2 → ISS-P5; M4-I3 → ISS-P6 +
> ISS-P7; M4-I4 → ISS-P8; M4-I5 → ISS-P9; M4-I6 → ISS-P10; M4-I7 → ISS-P11; M4-I8 → ISS-P12; M5-I9 → ISS-P13 +
> ISS-P14; M6-I10 → ISS-P15. Fifteen prompts, no milestone gap.

---

### ISS-P1 — Freeze the Issues ReBAC fragment + the worklog PersonalDataHolder tags (so dependents compile)

- **BAND.** M1.
- **ROADMAP MILESTONE.** Pre-work 3.0, the M1 slice (planning/06-roadmaps/subsystems/issue-tracker.md §3.0
  "Pre-work in M1/M2", the ReBAC fragment + the holder tags bullets).
- **DEPENDS-ON.** The M0 substrate prompts that lay down the Cargo workspace + the eight glue-crate skeletons +
  the twelve lints + the contract-coverage scanner (master §2 M0; substrate roadmap SUB-M0). The M1 Identity
  prompts that ship the ReBAC namespace engine (contract 4.9) into which fragments compile, and the M1 GDPR
  prompt that ships the #[personal_data] classify-derive (10.2). The index places this alongside the Identity M1
  work (Identity must accept the fragment so the SetExpr reverse index can be populated for the issue type).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md (always) §3 (name-your-floors, GDPR-safe by construction);
    ../../external-insights/01-process-and-quality-doctrine.md §5 (the ratchet — an uncommitted gate is no gate),
    §1 (name-your-floors, code-wins-over-docs).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md (the
    Issues ReBAC fragment definition + the worklog/free-text holder tags); 00-overview.md §1 (the most-coupled
    posture) + §2.2 (thin-shell-over-identical-plumbing).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §1 (the frozen ReBAC
    fragments — Issues: issue + field/transition caveats + watcher + the "- confidential" set-difference
    userset), OQ-H (the worklog/productivity classification, [OPEN — LEGAL]).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 4.9 (per-subsystem ReBAC namespace
    fragment; the Issues fragment frozen), 10.2 (the #[personal_data] classify-derive + the
    no-untagged-personal-data lint, with the worklog tags), 1.6 (the tenant-predicate +
    no-untagged-personal-data lints Issues compiles against).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §3.0 (the M1 bullets) + §2 (upstream deps table
    rows 4.9, 10.1/10.2) + §5 (the floors register — worklog tags row, R-2).
- **DELIVERABLE (what to build + exactly where in the repo).** In the issues service implementation crate
  (myelin-issues, the new subsystem crate under the workspace) plus its contributions into the shared cell
  schema:
  - The Issues ReBAC namespace fragment submitted into the one cell schema Identity compiles (contract 4.9): the
    issue definition with relations parent_project, assignee, watcher, confidential, confidential_grant; the
    permissions view = (parent_project->read - confidential) + confidential_grant, transition = assignee +
    parent_project->write, manage = parent_project->write; plus the issue_field and issue_transition caveat
    sub-objects (the field/transition ABAC sub-shapes). The fragment must COMPILE in the cell schema — that is
    the gate of this prompt, not a runtime property.
  - Declare the Issues PersonalDataHolder INTENT (the holder will be auto-registered by serve when the store
    opens in ISS-P3) and apply the #[personal_data(category, role, basis, retention, erasure, subject_locator)]
    tags on the (still-skeletal) issue schema types — the worklog/productivity/estimate fields tagged
    category=behavioural, role=tenant-content, basis=TBD-LEGAL, retention=tenant-policy, restricted-by-default
    (OQ-H), and the free-text title/props/comment/change-delta fields — so the no-untagged-personal-data lint is
    green from the first migration (ISS-P3).
  - FLOOR named: none new here (this is a contract-fragment freeze, not a feature). Name the [OPEN — LEGAL]
    residual on the worklog tag (basis=TBD-LEGAL, R-2: special-category-vs-elevated ratification is a parallel
    legal track, the structural tag ships now). State in the crate doc that no Issues feature ships here — only
    the shapes other systems compile against — and name ISS-P3 as the milestone where the holder is opened.
- **CONTRACTS TO IMPLEMENT.** 4.9 the Issues ReBAC fragment (owned — the fragment definition, compiled by
  Identity). 10.2 the #[personal_data] tags incl. the worklog tags (consumed — applied to issue types so the
  lint is green). Implement to the frozen shapes; a needed change is a whole-workspace contract PR, escalated and
  written down, not a local divergence (code-wins-over-docs).
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The Issues ReBAC fragment COMPILES in the shared cell schema Identity builds (a build-time gate, not a
    runtime drill) — CI, the compile is the green artifact.
  - The no-untagged-personal-data lint is GREEN on the issue skeleton schema (0 untagged PII fields; the lint red
    on a deliberately-untagged worklog fixture field, green on the tagged set) — CI, lint signal = 0 untagged
    fields. (No Issues-specific runtime drill here — §3.0 exit gate is explicitly compile-time, not a runtime
    property; no Issues data is written yet.)
- **TESTS (required).** Unit tests that the fragment compiles and that the "- confidential" set-difference
  userset resolves as specified (a confidential issue is absent from view for a non-grantee). The red+green
  fixture pair for the no-untagged-personal-data lint applied to the worklog tag. The provider/consumer CDC stub
  for contract-index row 4.9 (the Issues fragment). State the cargo-mutants mutation-score floor for the
  fragment-compile / userset-resolution module if it is mandatory-core (the confidential userset is leak-bearing
  — treat it as mandatory-core); if not, say so.
- **DEFINITION OF DONE.** The fragment compiles in the cell schema; the confidential userset resolves leak-free
  in unit tests; the worklog + free-text tags are applied and the no-untagged-personal-data lint is green with
  both fixtures; the CDC stub and unit tests pass; the contract-coverage scanner is green on the touched rows;
  the no-feature floor note + the [OPEN — LEGAL] worklog residual are written; the work is committed. No gate is
  greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M1: Issues ReBAC fragment + worklog holder tags. Body lists: contract 4.9 (Issues
  fragment) compiled, 10.2 worklog/free-text tags applied; the no-untagged-personal-data lint greened with
  red+green fixtures; the no-feature floor named (ISS-P3 opens the holder) + the worklog [OPEN — LEGAL] residual
  (R-2). Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P2 — Co-own myelin-query byte-identical + register the issue.* tokens + declare the IndexSpec/notif-rules

- **BAND.** M2.
- **ROADMAP MILESTONE.** Pre-work 3.0, the M2 slice (planning/06-roadmaps/subsystems/issue-tracker.md §3.0, the
  myelin-query co-ownership + the event-tokens + the declare bullets).
- **DEPENDS-ON.** ISS-P1 (the issues crate + the fragment exist). The M2 Bus prompt that seeds the event taxonomy
  grammar + registers the initiative token (contract 2.9). The M2 prompt(s) that freeze the myelin-content
  taxonomy (13.1) and that establish myelin-query as a frozen shared crate (13.3) — Knowledge leads the content
  freeze; Issues co-owns myelin-query. The M2 Search prompt that ships declare_indexable (6.3) and the M2 Notif
  prompt that ships define_notif_rule (7.6). The index places this in M2 alongside the reactive-layer + shared-
  crate freeze.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (one shared identity/permission/event model — no drift);
    ../../external-insights/01-process-and-quality-doctrine.md §7 (reconcile cross-component contracts at the
    plan layer before either side ships — a unit mismatch that ships calcifies), §5 (the ratchet).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/01-tech-and-data-model.md (the
    field-type enum + ViewSpec + the order_key/LexoRank codec Issues co-owns); 03-events-contracts-and-glue.md
    (the complete issue.* taxonomy incl. initiative; the declare_indexable IndexSpec; the define_notif_rule set).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-3 (the
    myelin-query primitive frozen byte-identical with Knowledge — field-type enum, ViewSpec, QueryAst, order_key),
    X-2 (the myelin-content taxonomy Issues consumes a subset of), §2 (the initiative token registered).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 13.3 (myelin-query frozen
    byte-identical: the field-type enum, ViewSpec, QueryAst, the order_key/LexoRank encoding — base-62
    0-9A-Za-z, lexicographic compare, midpoint bisection, 2-char jitter, 48-char rebalance, created_at+ULID
    tiebreak — Issues + Knowledge co-own), 2.9 (event taxonomy + the initiative type token), 6.3
    (declare_indexable — the issue.* facets projection), 7.6 (define_notif_rule — the Issues SLA/unblocked/
    approval reason set).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §3.0 (the M2 bullets) + §2 (upstream deps rows
    13.3, 2.9, 6.3, 7.6) + §4 (contracts-by-milestone, the 3.0 rows).
- **DELIVERABLE (what to build + exactly where in the repo).** In the shared myelin-query crate (co-owned) and
  the myelin-issues crate:
  - Contribute the storage discipline into the frozen myelin-query crate (13.3): the field-type enum, the
    ViewSpec, the QueryAst, and the order_key/LexoRank codec — definitions BYTE-IDENTICAL with Knowledge. Issues
    owns its own AST→store compiler (which lands in ISS-P6); the DEFINITIONS land here, frozen. Ship a round-trip
    + byte-identity test fixture (the drift-killer): the same ViewSpec/QueryAst/order_key serialized by Issues
    and by Knowledge produce byte-identical output, and order_key bisection/jitter/rebalance behave per the
    frozen codec.
  - Register the complete issue.* event taxonomy + the initiative type token (2.9) under the Bus §6 grammar, with
    names/units aligned to the EventEnvelope anchor: timestamps RFC-3339 UTC; SLA targets/stale_after/durations
    in seconds; estimates/story-points numeric; actor/subject as ArtifactRefs;
    contains_personal_data/data_role/pii_key_ref on any PII-bearing event. The complete v1 list named in arch 03
    (issue.created/updated/transitioned/commented/linked/reordered/assigned/erased, initiative.health_changed,
    etc.).
  - Declare the Issues declare_indexable IndexSpec (6.3): the issue.* facets projection shape (ft_fields,
    struct_fields, acl_object_type=issue) so Search knows Issues' projection exists; and the define_notif_rule
    set (7.6): the Issues reason set (SLA at-risk, unblocked, approval-requested) so Notif knows Issues' reasons
    exist. The wiring/emitter lands in M4; the DECLARATIONS are the deliverable here.
  - FLOOR named: none. State that no Issues data is written yet (the compiler/emitter wiring lands in M4-I1+).
- **CONTRACTS TO IMPLEMENT.** 13.3 myelin-query (co-owned — the byte-identical definitions + Issues' storage
  discipline; the compiler lands in ISS-P6). 2.9 the issue.* tokens + initiative (owned — registered into the
  Bus seed). 6.3 declare_indexable (owned — the IndexSpec declared). 7.6 define_notif_rule (owned — the reason
  set declared). Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The myelin-query byte-identity fixture is GREEN (0 byte differences between the Issues- and Knowledge-
    serialized ViewSpec/QueryAst/order_key; the order_key codec round-trips per the frozen rules) — CI, the
    byte-diff count = 0 is the green artifact. (This is the drift-killer the §3.0 exit gate names.)
  - The issue.* tokens (incl. initiative) parse under the §6.2 grammar (0 ungrammatical tokens) and the
    EventEnvelope units validate (durations in seconds, timestamps RFC-3339 UTC) — CI.
  - The declare_indexable IndexSpec registers with Search and the define_notif_rule set registers with Notif;
    both compile and the registrations are accepted (build-time gate) — CI.
- **TESTS (required).** Unit tests for the order_key bisection/2-char-jitter/48-char-rebalance behaviour and the
  created_at+ULID tiebreak. The byte-identity fixture cross-checked against Knowledge's serializer. Token
  grammar round-trip tests. The provider/consumer CDC pair for 13.3 (the co-owned definitions), 2.9 (the issue.*
  tokens), 6.3 + 7.6 (the declarations). State the cargo-mutants mutation-score floor for the order_key codec
  module (it is the rank source of truth — treat it as mandatory-core).
- **DEFINITION OF DONE.** The myelin-query definitions are byte-identical with Knowledge (fixture green); the
  issue.* tokens are registered and grammatical with valid units; the IndexSpec + notif-rules are declared and
  accepted; the CDC pairs + unit tests pass; the contract-coverage scanner is green on the touched rows; the
  no-data-yet note is written; the work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M2: Issues co-owns myelin-query + issue.* tokens + IndexSpec/notif declares. Body
  lists: 13.3 co-owned byte-identical (byte-diff = 0), 2.9 issue.* + initiative registered, 6.3/7.6 declared;
  the byte-identity fixture greened. Branch first if on default; do not push unless asked. End with the workspace
  Co-Authored-By trailer.

---

### ISS-P3 — The issue spine + the silent-data-loss-safe write path (the floor under all of Issues)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I1, the spine + write-path slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I1", the issue table + the minimal write path +
  pseudonymous identities bullets).
- **DEPENDS-ON.** ISS-P1 (the ReBAC fragment + holder tags), ISS-P2 (the issue.* tokens + myelin-query). The M0
  outbox prompts (OutboxTx::emit + outbox table + EventHandler + consumer_dedup, 2.2–2.5). The M1 Identity
  prompts (check + CaveatContext 4.2; write_tuples/zookie 4.6/4.10; resolve_pseudonym/erase + the pseudonym
  grammar 4.8). The M1 Storage prompts (OLTP + RLS + encrypted columns + the outbox 11.1; KMS hierarchy +
  per-subject DEK 11.3/11.4; backup/restore-verify 11.5 — STOR-D1). The M1 Tenancy prompts ((tenant,region)
  partition 12.1; residency_verify 12.4). The M1 GDPR prompt (PersonalDataHolder spine 10.1). This is the first
  Issues prompt that writes data — the index places it after the full M1 substrate, in M4.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scalable, GDPR-safe by construction, name-your-floors);
    ../../external-insights/01-process-and-quality-doctrine.md §2 (order-by-non-negotiability — silent data loss
    outranks every feature), §3 (prove-it — outbox emit-iff-committed with a telemetry signal);
    ../../external-insights/04-hard-problems.md §1 (erasure-vs-immutability — pseudonymous-by-default identity
    columns).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/01-tech-and-data-model.md (the issue
    table — typed core + JSONB tail + the (tenant,region) partition key + the lifecycle/GDPR columns;
    issue_relation TE-7 source of truth; issue_change_log; the scheme/cycle/milestone tables; prefix_counter);
    02-internals-and-algorithms.md §"write path" (validate → check → key-alloc → order_key CAS → mutate →
    OutboxTx::emit); 06-reconciliation-compliance.md (the pseudonymous identity columns + per-subject-DEK
    free-text).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-7 (the one
    free-text/immutable erasure posture — pseudonymous-by-default identity columns), §1 (the pseudonym grammar
    <pseudonym>@<tenant>.noreply frozen).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 1.1–1.4 (serve/three-surface/
    liveness≠readiness/PersonalDataHolder auto-reg), 2.1/2.2/2.3/2.5 (envelope/outbox/outbox-table/dedup — the
    issue is the aggregate, UNIQUE(aggregate, seq)), 4.2 (check + CaveatContext), 4.6/4.10 (write_tuples/zookie),
    4.8 (resolve_pseudonym/erase + the grammar), 11.1 (OLTP + RLS + encrypted columns + the outbox), 11.3/11.4
    (KMS + per-subject DEK for free-text), 11.5 (restore-verify — STOR-D1), 12.1/12.4 (partition key +
    residency_verify), 10.1 (PersonalDataHolder), 5.1 (the ArtifactRef <PROJECTKEY>-<seqno> grammar — the key
    shape; allocation is ISS-P4).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I1" (the work + the exit gate; the floor
    named) + §2 (the starred upstream deps) + §1 (the non-negotiability order — leak then write-loss).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows for the outbox emit-iff-committed shape (SUB-D1/BUS-D4 applied to Issues) and the upstream STOR-D1/ID-D3.
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate as an AppSpec + the
  harness-wired handlers/migrations (not a hand-rolled main, per the substrate convention):
  - The forward-only migrations for the issue table (typed core columns + a JSONB property-bag tail + the
    (tenant,region) partition key + the lifecycle/GDPR columns), issue_relation (TE-7 source of truth, forward
    edge), issue_change_log, the scheme/scheme_assignment/cycle/cycle_membership/milestone/prefix_counter tables,
    consumer_dedup, and the per-service outbox table. Flag issue/issue_relation/issue_change_log as hot tables
    (1.5; expand→backfill→contract; the forward-only-migration lint holds).
  - The minimal write path as a state-changing handler: validate → Id.check (+ CaveatContext) → mutate the typed
    core → OutboxTx::emit IN THE SAME TRANSACTION. The issue is the aggregate (UNIQUE(aggregate, seq) per-issue
    ordering). No publish_now — the no-raw-publish lint holds. (Key allocation + order_key CAS + the content body
    land in ISS-P4; here the write path emits with a placeholder key + a plain typed-core mutation so the
    emit-iff-committed seam is proven first.)
  - Pseudonymous-by-default identity columns (assignee/reporter/created_by = pseudonymous principal ids per the
    <pseudonym>@<tenant>.noreply grammar, EI-04 §1) + per-subject-DEK encryption for free-text title/props/
    change-deltas (11.4). Register Issues as a PersonalDataHolder (auto-registered by serve when the store opens,
    1.4) — declare the holder ops as todo-stubbed (the full locate/export/rectify/restrict/erase implementation
    is ISS-P12; the registration + the per-subject-DEK column wiring ship now).
  - FLOOR named: ranking = order_key + server-arbitrated CAS arrives in ISS-P4 (move-CRDT is the M5 follow-on,
    ISS-P13); storage = PG-hybrid sharded by tenant (distributed-SQL is the measured follow-on, R-6, ISS-P13);
    rollup deferred to ISS-P8; the holder ops are stubbed (full erasure fan-out is ISS-P12). Name each in the
    crate doc with its follow-on prompt.
- **CONTRACTS TO IMPLEMENT.** 1.1–1.4 (consumed — boot from serve, register as a holder). 2.1/2.2/2.3/2.5
  (consumed — the issue.* shapes via the one emit path, per-aggregate ordering, dedup). 4.2 (consumed — the write
  gate). 4.6/4.10 (consumed — write_tuples for assign/watch/confidential-grant + zookie). 4.8 (consumed —
  pseudonymous identities). 11.1/11.3/11.4/11.5 (consumed — OLTP + KMS + per-subject DEK + restore-verify).
  12.1/12.4 (consumed — partition + residency). 10.1 (consumed — the holder). Implement to the frozen shapes;
  escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - Outbox emit-iff-committed for the issue write path (the SUB-D1/BUS-D4 shape applied to Issues: kill the
    service between commit and publish → the issue.* event is delivered exactly when its row committed, never
    without it; 0 ghost, 0 lost) — CI, the outbox-depth + consumer-dedup telemetry signals are the green artifact.
  - Upstream STOR-D1 (restore-verify, RPO ≤ 5 min / RTO ≤ 1h-tenant) and ID-D3 (cross-tenant 0) GREEN — the gate
    invariant: Issues writes no real data over a red restore-verify or a red cross-tenant drill. State this
    explicitly; do not mark done if either upstream gate is red (record a "blocked on STOR-D1/ID-D3" scorecard
    row instead of weakening).
- **TESTS (required).** Unit tests for the write-path transaction (the emit is in the same tx; a rolled-back
  mutation emits nothing). A chained-mutation end-to-end test (create then update then transition — chained, not
  single-handler, per EI-01 §4) asserting per-aggregate seq monotonicity and dedup on replay. The drill-harness
  scenario for the kill-between-commit-and-publish (emit-iff-committed). The provider/consumer CDC pair for the
  issue.* outbox rows (2.2/2.3). State the cargo-mutants mutation-score floor for the write-path / outbox-emit
  module (mandatory-core — it is the write-loss seam).
- **DEFINITION OF DONE.** The migrations apply forward-only; the write path co-commits its event through the
  outbox (emit-iff-committed drill green with its telemetry signal); pseudonymous columns + per-subject-DEK
  free-text are in place; Issues registers as a holder; the upstream STOR-D1 + ID-D3 are green (else a dated
  blocked row, not a weakened gate); the unit + chained-e2e + drill tests pass; the contract-coverage scanner is
  green; the four floors are named with their follow-on prompts; the work is committed. "Looks done" is not done.
- **COMMIT.** Header: P-<NNN> M4: Issue spine + silent-data-loss-safe write path. Body lists: contracts 2.2/2.3
  (the outbox issue.* path), 4.2/4.6/4.8/11.x/12.x consumed; the emit-iff-committed drill greened (0 ghost/0
  lost, the measured signal); STOR-D1/ID-D3 confirmed green; the four floors named (CAS→ISS-P4 / rollup→ISS-P8 /
  distributed-SQL→ISS-P13 / holder-ops→ISS-P12). Branch first if on default; do not push unless asked. End with
  the workspace Co-Authored-By trailer.

---

### ISS-P4 — Hi/Lo human keys + the order_key CAS reorder floor + the content body (the create→edit→reorder loop)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I1, the keys + CAS + content slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I1", the Hi/Lo allocation + the server-arbitrated CAS +
  the content body bullets).
- **DEPENDS-ON.** ISS-P3 (the spine + write path + the outbox seam). ISS-P2 (the myelin-query order_key codec +
  the content subset). The M2 prompt that froze myelin-content + the WASM render target (13.1). The index places
  this immediately after ISS-P3 within M4 (it completes M4-I1's "first runnable" deliverable).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors); ../../external-insights/01-process-and-quality-doctrine.md §3
    (prove-it — 0 silent clobber with a converged-order signal), §1 (name-your-floors — CAS is the floor, CRDT
    the follow-on); ../../external-insights/04-hard-problems.md §2 (CRDT-after-CAS).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md (the
    Hi/Lo human-key allocator — per-prefix, gap-tolerant, monotonic, adaptive block, cell-local; the
    server-arbitrated order_key CAS reorder; the loser-re-bases discipline); 01-tech-and-data-model.md (the
    content body as a myelin-content block subtree + the version token single-author CAS).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-2 (myelin-content
    + the WASM render target render(parse(md)) === md), X-3 (the order_key/LexoRank codec frozen), REF-3 (the
    Issues <PROJECTKEY>-<seqno> key as the stored canonical id; #1421 is render-time only).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 5.1 (the ArtifactRef
    <PROJECTKEY>-<seqno> grammar — the stored canonical key), 13.3 (the order_key/LexoRank codec — base-62,
    midpoint bisection, 2-char jitter, 48-char rebalance, created_at+ULID tiebreak), 13.1 (the myelin-content
    block subset + the WASM render path; the three inline ref nodes), 2.2 (the reorder/key writes co-commit
    their events).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I1" (the exit gate — ISS-D10, ISS-D4, ISS-D5)
    + §5 (the floors register — CAS ranking row, R-3) + §6 (first-runnable definition).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows ISS-D4 (create-storm human-key), ISS-D5 (reorder 0-clobber), ISS-D10 (render(parse(md)) === md).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate:
  - The Hi/Lo human-key allocator per prefix: the frozen <PROJECTKEY>-<seqno> stored canonical id (the
    prefix_counter table holds the Hi block; the allocator hands out Lo seqnos), gap-tolerant, monotonic per
    prefix, adaptive block size, per-prefix isolation, cell-local. The stored key == the canonical ArtifactRef
    id (5.1); #1421 is a render-time display projection, never stored.
  - The server-arbitrated order_key CAS for drag-reorder: a reorder request carries the issue's last-seen
    order_key; the server bisects a new key (the frozen codec — 2-char jitter, 48-char rebalance trigger) and
    writes under a CAS on the prior key; on a precondition miss the LOSER is rejected and re-bases honestly
    against current server state — no silent clobber, no merge. This is the CAS floor (move-CRDT is the M5
    follow-on, ISS-P13). The 48-char rebalance must never reorder the displayed order.
  - The issue body + comments as a myelin-content block subtree (the consumed subset; single-author CAS on the
    version token; the WASM render path). render(parse(md)) === md must hold for bodies + comments (read + edit
    use the IDENTICAL WASM parser, not two code paths).
  - FLOOR named: ranking = order_key + server-arbitrated CAS; the move-CRDT (Yrs list / Fugue) is the named M5
    follow-on (ISS-P13), reusing the byte-identical order_key — promotion swaps the conflict engine, not the data
    model. Name it in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 5.1 (owned — the <PROJECTKEY>-<seqno> canonical key + the #1421 render projection).
  13.3 (consumed — the order_key codec, now executed). 13.1 (consumed — the content block subset + WASM render).
  2.2 (consumed — reorder/key writes co-commit). Implement to the frozen shapes; escalate a needed change, do not
  diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - ISS-D10 (render(parse(md)) === md 100% over a body+comment corpus; read+edit use the identical WASM parser) —
    CI, round-trip = 100% is the green artifact.
  - ISS-D4 (create-storm on one hot prefix, N workers — import + incident burst → no duplicate key, monotonic per
    prefix, gaps benign, per-prefix isolation, key == the stored canonical id) — SCHED, 0 dup key + monotonic is
    the green artifact.
  - ISS-D5 (N humans + an agent re-ranking the same region → 0 silent clobber, bounded re-base churn, converges
    with the 2-char jitter, the 48-char rebalance never reorders displayed order) — CI, 0 clobber + converged
    order is the green artifact.
- **TESTS (required).** Unit tests for the Hi/Lo allocator (gap-tolerance, per-prefix isolation, adaptive block)
  and the order_key CAS (precondition-miss → loser re-bases, no overwrite). A chained-mutation e2e test (create →
  reorder concurrently from N writers → assert converged order, 0 clobber). The drill scenarios for ISS-D4,
  ISS-D5, ISS-D10. The provider/consumer CDC pair for 5.1 (the key grammar). State the cargo-mutants
  mutation-score floor for the order_key CAS module (mandatory-core — it is the silent-clobber seam).
- **DEFINITION OF DONE.** Keys allocate uniquely + monotonically per prefix; reorder is 0-clobber and converges;
  bodies round-trip 100%; ISS-D4/ISS-D5/ISS-D10 emit dated green artifacts; the unit + chained-e2e + drill tests
  pass; the contract-coverage scanner is green; the CAS→move-CRDT floor is named with ISS-P13; the work is
  committed. The first-runnable bar (§6) is met: a tenant can create → key → edit → link → reorder, every write
  co-committing its event. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: Hi/Lo keys + order_key CAS reorder + content body. Body lists: contracts 5.1
  owned, 13.3/13.1 executed; ISS-D10 (100% round-trip), ISS-D4 (0 dup key), ISS-D5 (0 clobber) greened with
  measured numbers; the CAS→move-CRDT floor named (ISS-P13). Branch first if on default; do not push unless
  asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P5 — Governance schemes + the data-driven workflow FSM interpreter (config, never a data migration)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I2 (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I2", the five scheme kinds
  + the workflow FSM interpreter + the flexible-field model).
- **DEPENDS-ON.** ISS-P3 + ISS-P4 (the spine + the write path + the content body). ISS-P2 (the QueryAst — the
  guard predicate language). The M0 substrate prompts (ResilientClient/FailStatic 1.9/1.10; the
  forward-only-migration + flow-determinism lints 1.6). The index places this after the M4-I1 pair within M4.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1 (Issues serves engineers AND PMs — corporate workflows: roadmaps/sprints/hierarchies/
    custom fields/SLAs);
    ../../external-insights/01-process-and-quality-doctrine.md §7 (keep the architecture coherent — config not a
    bespoke object graph per scheme; no Jira-Groovy footgun), §5 (the ratchet — the flow-determinism lint).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md (the
    scheme-precedence algebra — most-specific-wins, cached, off the hot path; the data-driven workflow FSM
    interpreter; the fixed state-category set unstarted/started/completed/cancelled; the QueryAst guards;
    required-fields-on-transition; the post-actions); 01-tech-and-data-model.md (the JSONB property-bag tail +
    the GIN index default for flexible fields).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-3 (the QueryAst as
    the one bounded guard predicate language — no UDFs/loops/recursion), OQ-C (the flexible-field index posture).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 13.3 (the QueryAst guard
    predicate core), 3.4 (EventMatcher = QueryAst — the same bounded interpreter the guards use), 1.5 (forward-
    only migrations + the hot-table flags), 1.9/1.10 (ResilientClient/FailStatic for Issues→Id), 1.6 (the
    flow-determinism + forward-only-migration lints).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I2" (the work + the exit gate — the ISS-D12
    guard half + no-config=Linear-simple) + §5 (the floors register — issue-hierarchy=tree row + GIN-default
    row).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row ISS-D12 (the guard slice — "can't close while blocked_by an open issue").
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate:
  - The five scheme kinds (workflow/field/permission/sla/type) as interpreted JSONB config rows, assigned per
    (type × project × team); the deterministic, cached scheme-precedence algebra (most-specific-wins) computed
    OFF the hot path — the write loads the already-resolved compiled scheme, never resolves precedence inline.
  - The data-driven workflow FSM interpreter with the FIXED state-category set
    (unstarted/started/completed/cancelled) as the one mandatory governance invariant over unlimited named
    states; guards are the frozen QueryAst (bounded, no UDFs/loops/recursion); required-fields-on-transition;
    post-actions (assign/set-field/link/arm-trigger). Assigning a new scheme is a CONFIG write, never a row
    migration — prove this (a scheme reassignment touches no issue rows).
  - The flexible-field model: the JSONB property-bag tail (zero-DDL custom fields) + the GIN index default; the
    forward-only-migration lint on the hot issue/issue_relation/issue_change_log tables.
  - FLOOR named: issue hierarchy = tree parent (constrained-DAG portfolios are the opt-in follow-on, M5+); the
    projection-feeder generated-index promotion is deferred to ISS-P6 (cold facets ride the GIN index until
    measured). Name both in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 13.3 (consumed — the QueryAst guard core, executed by the FSM interpreter). 3.4
  (consumed — the EventMatcher=QueryAst alignment for arm-trigger post-actions). 1.5 (consumed — forward-only
  hot-table migrations). 1.9/1.10 (consumed — Issues→Id resilient/fail-static). Implement to the frozen shapes;
  escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The workflow-guard correctness slice of ISS-D12 ("can't close while blocked_by an open issue" → transition
    blocked with a pre-assembled reason) — CI, transition-blocked + the reason string is the green artifact. (The
    CI-red half lands in ISS-P11 when the X-1 seam closes.)
  - no-config = Linear-simple PROVEN: an org with zero scheme assignments resolves to org_default for every kind
    with no migration (0 issue rows touched by a scheme reassignment) — CI.
  - The flow-determinism lint holds on any workflow body that schedules a durable activity (a post-action that
    arms a trigger) — CI, lint green.
- **TESTS (required).** Unit tests for the scheme-precedence algebra (most-specific-wins determinism + caching)
  and the FSM interpreter (the fixed-category invariant; a guard rejects a transition; required-fields enforced;
  post-actions fire). A chained-mutation e2e test (assign a scheme → transition through states → assert the
  category invariant + guard). The drill scenario for the ISS-D12 guard half. The CDC stub for the scheme/guard
  config shape. State the cargo-mutants mutation-score floor for the guard-evaluation module if mandatory-core
  (the guard is governance-correctness-bearing — treat it as mandatory-core).
- **DEFINITION OF DONE.** Schemes resolve deterministically off the hot path; the FSM interpreter enforces the
  fixed category set + the QueryAst guards + required-fields + post-actions; a scheme reassignment migrates no
  data; the ISS-D12 guard half + no-config-Linear-simple are green; the flow-determinism lint holds; the unit +
  e2e + drill tests pass; the coverage scanner is green; the tree-hierarchy + GIN-default floors are named; the
  work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: Governance schemes + workflow FSM interpreter. Body lists: 13.3/3.4 consumed;
  the ISS-D12 guard half greened (transition blocked + reason), no-config-Linear-simple proven (0 rows touched),
  flow-determinism lint green; the tree-hierarchy + GIN-default floors named. Branch first if on default; do not
  push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P6 — The query planner: the SetExpr push-down + cost-bounding (leak-free + flexible-field latency)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I3, the planner slice (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I3",
  the AST→OLTP-store compiler + cost-bounding + the three-tier escalation). This is the zero-leak + latency
  milestone — Issues' two highest-stakes properties.
- **DEPENDS-ON.** ISS-P3 + ISS-P4 + ISS-P5 (the spine + keys + schemes/fields). ISS-P2 (myelin-query — the AST it
  compiles). The M1 Identity prompt that ships list_objects with the SetExpr push-down + the per-tenant authz
  reverse index (4.3) and the zookie semantics (4.10). The M2 Search prompt (query conjoins the Filter 6.1; the
  search-requires-acl-filter lint). The index places this after ISS-P5 within M4 — it is the make-or-break
  leak/latency milestone.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale from day 1; one permission model);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — 0 leak with a zero-escape counter;
    the <1s budget is a quantified gate), §2 (the leak is the catastrophe — it comes before breadth).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md (the
    AST→OLTP-store compiler — lower the SetExpr FIRST into a SQL predicate/JOIN over the authz reverse index keyed
    on issue.id; the Ids/NotIds/InRelation{relation,via_column}/TupleSet/Union/Intersect/Difference/All/None
    lowering; the zookie staleness bound; the cost-bounding + the three-tier escalation; the projection feeder);
    05-hard-problems.md (the leak-free-at-scale + the flexible-field-latency analysis).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-E (the SetExpr
    push-down — lowered to a SQL predicate/JOIN over the per-tenant authz reverse index; no N+1, no post-filter),
    OQ-C (the measured projection-feeder promotion threshold > 5% of view executions).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 4.3 (list_objects → Ids|Filter
    with the SetExpr push-down — the single most load-bearing inter-system contract; lower it first), 4.10
    (zookie — the new-enemy guard; a security-sensitive scan reads at-or-after the zookie revision), 6.1 (Search
    query conjoins the same Filter — the Tier-3 escalation valve; the search-requires-acl-filter lint), 13.3 (the
    QueryAst/ViewSpec the compiler lowers).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I3" (the work + the exit gate — ISS-D3,
    ISS-D2, ISS-D1; the floor named) + §1 (the cross-tenant/confidential leak is what kills us first inside
    Issues).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows ISS-D3 (cross-tenant + confidential IDOR 0 leak), ISS-D2 (50+ fields × 1M+ issues board query < 1s, no
    full scan).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate (the Issues-owned
  query planner):
  - The AST→OLTP-store compiler that LOWERS the frozen list_objects SetExpr FIRST into a SQL predicate / JOIN
    against the per-tenant authz reverse index keyed on issue.id (Ids / NotIds / InRelation{relation, via_column}
    / TupleSet / Union / Intersect / Difference / All / None) — ONE query, no N+1, no post-filter. The zookie
    bounds staleness: a security-sensitive scan reads at-or-after the zookie's revision (the new-enemy guard). A
    confidential issue is simply ABSENT from the result, never a "N hidden" count leak.
  - Cost-bounding + the three-tier escalation: Tier 1 typed-core index ranges (issue_board / issue_roadmap /
    issue_assignee); Tier 2 measured-hot generated indexes (the projection feeder; the GIN probe as 2b); Tier 3
    escalate to Search CONJOINING THE SAME Filter (the search-requires-acl-filter lint). Every query is paginated
    + statement-timeout'd; a query that would scan too much is pushed to Search or returns a Refine hint — never
    an unbounded JSONB scan.
  - The projection feeder consumer (watches issue.updated deltas + a per-(tenant,type,field_id) frequency
    counter; provisions a generated/expression index via a forward-only online migration when a facet crosses the
    measured threshold — promotion is MEASURED, never predicted; OQ-C calibration).
  - FLOOR named: the projection-feeder promotion threshold is the OQ-C default-to-beat (> 5% of a collection's
    view executions), calibrated by ISS-D2; distributed-SQL for a hot tenant is the measured follow-on (M5,
    ISS-P13). Name both in the crate doc. (The co-equal ViewSpec views + the Refs wiring land in ISS-P7.)
- **CONTRACTS TO IMPLEMENT.** 4.3 (consumed — the SetExpr push-down lowered first; the planner is the headline
  consumer of this contract). 4.10 (consumed — the zookie staleness bound). 6.1 (consumed — the Tier-3 Search
  escalation with the same Filter). 13.3 (consumed — the QueryAst lowered). Implement to the frozen shapes;
  escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - ISS-D3 (cross-tenant + confidential-issue IDOR → not in any board / SetExpr JOIN / search / backlink /
    context-pane result for an unauthorized viewer, incl. under zookie staleness; 0 leak) — CI, the zero-escape
    counter = 0 is the green artifact. (This is the F1 leak-free family; it re-runs inside the surge family in
    M5/ISS-P13.)
  - ISS-D2 (50+ custom fields × 1M+ issues board query under the <1s keyboard budget with the SetExpr JOIN; a
    cold ad-hoc query escalates to Search with the same Filter; the planner never emits a full JSONB scan) —
    SCHED, query p99 < 1s + no-full-scan is the green artifact. (Also the OQ-C calibration drill.)
- **TESTS (required).** Unit tests for the SetExpr lowering (each variant → the correct predicate/JOIN; the
  confidential set-difference excludes; the zookie watermark is honoured) and the cost-bounder (a too-large scan
  returns Refine / escalates, never an unbounded scan). A chained-mutation e2e test (grant then revoke
  confidential-grant → the revoke reflects in the next zookie-bounded read; 0 leak). The drill scenarios for
  ISS-D3 + ISS-D2. The provider/consumer CDC pair for 4.3 (the SetExpr push-down — Issues is the consumer side)
  + 6.1 (the conjoined Filter). State the cargo-mutants mutation-score floor for the SetExpr-lowering module
  (mandatory-core — it is THE leak seam).
- **DEFINITION OF DONE.** The planner lowers the SetExpr first into one leak-free query (no N+1, no post-filter);
  cost-bounding + the three-tier escalation hold; the projection feeder promotes on measured frequency; ISS-D3
  (0 leak) + ISS-D2 (<1s, no full scan) emit dated green artifacts; the unit + e2e + drill tests pass; the
  search-requires-acl-filter lint is green on the Tier-3 escalation; the coverage scanner is green; the OQ-C +
  distributed-SQL floors are named; the work is committed. No gate is greened by weakening a threshold or
  inverting the zero-escape assertion.
- **COMMIT.** Header: P-<NNN> M4: Query planner — SetExpr push-down + cost-bounding (leak-free at scale). Body
  lists: contract 4.3 (the SetExpr lowering) + 4.10/6.1 consumed; ISS-D3 (0 leak, zero-escape counter) + ISS-D2
  (p99 < 1s, no full scan) greened with measured numbers; the OQ-C + distributed-SQL floors named. Branch first
  if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P7 — The co-equal ViewSpec views + Refs/Search wiring (board/roadmap/backlog/table/calendar/cycle)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I3, the views + the Refs/Search wiring slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I3", the co-equal ViewSpec projections + the Refs
  resolve/project + the #sub mint + the issue.* Search projection bullets).
- **DEPENDS-ON.** ISS-P6 (the planner — every view conjoins the SetExpr Filter through it). ISS-P2 (the ViewSpec
  + the #sub grammar Issues mints). The M2 Refs prompts (ArtifactRef parse/format 5.1; resolve + the tombstone
  ladder 5.2/5.7; project REQUIRED 5.6; traverse 5.3; the TE-7 mirror 5.5; refs.edge via content nodes 5.4). The
  M2 Search prompt (declare_indexable 6.3; reindex 6.4). The index places this immediately after ISS-P6 within
  M4 (it completes M4-I3).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (top-of-the-line UX — co-equal views over one model);
    ../../external-insights/01-process-and-quality-doctrine.md §7 (the compounding payoff — each new view is a
    projection of the one table, not a new object graph); the design folder
    (../04-subsystem-architectures/issue-tracker/design/information-architecture.md + user-flows.md +
    wireframes.md — the board/roadmap/backlog/table/calendar/cycle screens incl. empty/loading/error/permission
    states).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md (the
    views as co-equal ViewSpec projections; the board↔roadmap structural co-equality, type_rank denormalised);
    03-events-contracts-and-glue.md (the Refs resolve/project unfurl; the #sub mints comment-/b/field-/row-; the
    issue.* Search projection); 04-views-cli-and-api.md (the view surfaces + the CLI parity).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-4 (the unified
    #sub grammar + the 4-step tombstone ladder), OQ-I (cell-local resolution).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 5.1 (ArtifactRef), 5.2 (resolve →
    Projection|Tombstone, per-viewer), 5.6 (project REQUIRED — {title,state,icon,render_hint,sub_anchor?};
    pre-permission-checked; the only cross-DB read of an Issues artifact — a confidential issue returns a
    tombstone carrying the root, never the title), 5.7 (the #sub grammar — mint comment-/b/field-/row-), 5.4
    (refs.edge via the inline mention/artifact_ref content nodes), 5.3 (traverse), 5.5 (the TE-7 mirror), 6.3
    (declare_indexable — the issue.* projection emitter), 13.3 (the ViewSpec).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I3" (the work + the exit gate — ISS-D1) + §2
    (the Refs/Search upstream rows) + §6 (first-useful — the co-equal board+roadmap+backlog).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row ISS-D1 (board↔roadmap same-row, 0 drift, asserted by row id).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate (+ the design record):
  - The views as co-equal ViewSpec projections over the one issue table (board / roadmap / backlog / list /
    table / calendar / cycle), each ALWAYS conjoining the SetExpr Filter through the ISS-P6 planner (a
    confidential issue is simply absent — no "N hidden" leak). The board↔roadmap co-equality is STRUCTURAL (same
    rows; type_rank denormalised) — an edit on one reflects the same row on the other.
  - Wire Refs resolve + project(ref, viewer) (5.6): the context-pane unfurl, pre-permission-checked; a
    confidential issue returns a tombstone carrying the root, never the title. Mint the unified #sub ids Issues
    owns (5.7: comment-/b/field-/row-) — stable opaque ids; Refs stores the full sub-URN + the stripped root.
    Emit refs.edge.created from the inline mention/artifact_ref content nodes (5.4). Wire traverse (5.3) for the
    bounded cycle-safe walk (depth 16) and the issue_relation TE-7 mirror (5.5).
  - Emit the issue.* Search projection (declare_indexable from ISS-P2, now the live emitter; 6.3) so Tier-3
    escalation has an index; reindex(scope) (6.4) as the only rebuild path.
  - The design-system pass (pre-frontend, per VISION §3 — no frontend code without a reviewed sketch): a
    visual/token-level pass over the board/roadmap/backlog/table/calendar/cycle screens in the design folder,
    INCLUDING the empty/loading/error/permission/tombstone states. Record the sign-off in the design folder. (The
    view affordances are not security-decision-shaped here; build them — but the sketch precedes the UI per the
    process.)
  - FLOOR named: none new (the views are projections; the planner floors were named in ISS-P6). State that the
    cross-cell portfolio rollup view is the M5 follow-on (ISS-P13, the CrossCellPointer bridge).
- **CONTRACTS TO IMPLEMENT.** 5.6 (owned — project REQUIRED on Issues). 5.1/5.2/5.7/5.4/5.3/5.5 (consumed/owned —
  the ArtifactRef, the resolve/tombstone, the #sub mints, the edges, the traverse, the TE-7 mirror). 6.3/6.4
  (consumed — the issue.* projection emitter + reindex). 13.3 (consumed — the ViewSpec). Implement to the frozen
  shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - ISS-D1 (edit an issue's date/scope on the board → roadmap reflects the SAME ROW, 0 drift, asserted by row id;
    and vice versa) — CI, the same-row-id assertion is the green artifact.
  - The Refs project() for a confidential issue returns a tombstone carrying the root, never the title (the 0-leak
    unfurl property, a slice of ISS-D3 re-asserted at the unfurl boundary) — CI.
  - The design pass is REVIEWED-AND-SIGNED-OFF in the design folder (incl. the empty/loading/error/permission/
    tombstone states) — sign-off recorded, dated, the green artifact for the pre-frontend gate.
- **TESTS (required).** Unit tests that each ViewSpec projection conjoins the Filter and that board↔roadmap share
  the row (type_rank denormalisation is consistent). A chained-mutation e2e test (edit on board → read on
  roadmap → assert same row id). The drill scenario for ISS-D1. The provider/consumer CDC pair for 5.6 (project —
  Issues owns the provider side) + 5.4 (the issue edges). State the cargo-mutants mutation-score floor for the
  project()/tombstone module if mandatory-core (the tombstone-vs-title decision is leak-bearing — treat it as
  mandatory-core).
- **DEFINITION OF DONE.** The co-equal views project over one table with the Filter conjoined; board↔roadmap show
  the same row (ISS-D1 green); project() returns a tombstone-not-title for a confidential issue; the #sub mints +
  edges + traverse + TE-7 mirror are wired; the issue.* Search projection emits + reindex works; the design pass
  is signed off; the unit + e2e + drill tests pass; the coverage scanner is green; the cross-cell rollup follow-on
  is named; the work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: Co-equal ViewSpec views + Refs/Search wiring. Body lists: contract 5.6 owned
  (project), 5.1/5.2/5.7/5.4/5.3/5.5 + 6.3/6.4 wired; ISS-D1 (same-row, 0 drift) greened; the confidential-unfurl
  tombstone proven; the design pass signed off; the cross-cell rollup follow-on named (ISS-P13). Branch first if
  on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P8 — Rollup + cycles/milestones + attachments + OLAP (the derived-aggregate breadth)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I4 (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I4", the incremental
  rollup consumer + the time axis + attachments + the OLAP wiring).
- **DEPENDS-ON.** ISS-P7 (the views + the Refs traverse + the TE-7 mirror — the rollup walks parent edges; the
  views read the rollup). The M2 Bus prompt (reindex-from-source / replay 2.6). The M1 Storage prompts (BlobStore
  content-addressed 11.2; the OLAP read store + restriction flag 11.6). The index places this after ISS-P7 within
  M4.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale); ../../external-insights/04-hard-problems.md §5 (reindex-from-source — the
    derived store rebuilds, never restored; steady-state and recovery share one code path), §2.4 (rollups
    computed off the bus, never in the write path);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the reindex-parity drift-free
    assertion).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md (the
    event-driven debounced incremental rollup consumer — depth-16 ceiling, visited-set, cycle-safe; the
    debounce-coalesce; the incremental re-sum; the input_hash no-op suppression for loop storms; the rollup row
    as a derived rebuildable aggregate); 01-tech-and-data-model.md (cycles/sprints + milestones as separate
    objects with membership edges; attachments in BlobStore; the OLAP CQRS model).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md (the OLAP
    restriction-flag propagation §8; the reindex-from-source as the only recovery path; OQ-K the debounce-window
    floor).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 2.6 (reindex-from-source —
    replay emits *.snapshot through the live consumer; the only recovery path for derived stores), 5.3 (traverse
    — the bounded cycle-safe ancestor walk depth 16), 11.2 (BlobStore content-addressed, residency-pinned — the
    row holds the pointer + per-subject-DEK metadata, not the bytes), 11.6 (the OLAP read store + restriction
    flag — CFD/cycle-time/velocity/SLA-compliance, never touching the OLTP issue table).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I4" (the work + the exit gate — ISS-D8; the
    floor named) + §5 (the floors register — read-time-rollup row, R-4).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row ISS-D8 ((a) rollup freshness under a 10k-issue import with bounded ancestor recomputes; (b) replay
    rebuilds rollup + the Refs edge projection drift-free vs live).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate (consumers + OLAP
  wiring):
  - The event-driven, debounced, incremental rollup consumer (off the bus, NEVER in the write path): walk parent
    edges (depth ceiling 16, visited-set, cycle-safe — a dependency cycle is a roadmap diagnostic, never a hang);
    debounce-coalesce a burst into one ancestor recompute; incremental re-sum; input_hash no-op suppression
    (stops loop storms, AG-6). The rollup row is derived (rebuildable by replay; edge truth stays in
    issue_relation).
  - The time axis: cycles/sprints + milestones as separate objects (membership edges, not containment);
    burndown/CFD fed to OLAP off the bus; carry-over provenance.
  - Attachments in BlobStore (content-addressed, residency-pinned; the row holds the pointer + per-subject-DEK
    metadata, not the bytes).
  - The OLAP read store wiring (CQRS, reindex-from-source ONLY, restriction-flag-honouring): CFD, cycle-time,
    velocity, SLA-compliance — never touching the OLTP issue table.
  - FLOOR named: rollup = read-time for small subtrees, materialise-on-measured-large (KN-3, the M5 follow-on,
    ISS-P13); the debounce-window + affected-ancestor fan-out policy is per-tenant-tunable, calibrated by the
    ISS-D8a window (OQ-K floor); forecast deferred to ISS-P10. Name each in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 2.6 (consumed — reindex-from-source / replay; the only recovery path). 5.3
  (consumed — the bounded ancestor walk). 11.2 (consumed — BlobStore for attachments). 11.6 (consumed — the OLAP
  read store + restriction flag). Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - ISS-D8(a) (rollup freshness under a 10k-issue import → a BOUNDED number of ancestor recomputes via debounce;
    initiative progress correct within the window) — SCHED, the debounce-bound is the green artifact.
  - ISS-D8(b) (reindex-from-source: replay rebuilds the rollup aggregate + the Refs edge projection DRIFT-FREE vs
    live — proving steady-state and recovery share one code path) — SCHED, the reindex-parity (0 drift) is the
    green artifact.
- **TESTS (required).** Unit tests for the rollup walk (depth-16 ceiling, cycle-safety via visited-set, the
  input_hash no-op suppression, the incremental re-sum) and the OLAP restriction-flag (a restricted subject is
  excluded from analytics). A chained-mutation e2e test (import a subtree → assert bounded recomputes → replay →
  assert drift-free). The drill scenario for ISS-D8. The provider/consumer CDC pair for 2.6 (replay) + 11.6 (the
  OLAP feed). State the cargo-mutants mutation-score floor for the rollup-consumer module if mandatory-core (the
  loop-storm suppression is correctness-bearing — treat it as mandatory-core).
- **DEFINITION OF DONE.** The rollup recomputes incrementally + debounced off the bus, cycle-safe, loop-storm-
  suppressed; cycles/milestones are membership-edged; attachments hold pointers not bytes; OLAP feeds off the bus
  and honours restriction; ISS-D8(a)+(b) emit dated green artifacts (bounded recomputes + 0-drift reindex); the
  unit + e2e + drill tests pass; the coverage scanner is green; the read-time-rollup + debounce-window floors are
  named; the work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: Rollup + cycles/milestones + attachments + OLAP. Body lists: contracts 2.6/5.3/
  11.2/11.6 consumed; ISS-D8(a) (bounded recomputes) + ISS-D8(b) (0-drift reindex-parity) greened with measured
  numbers; the read-time-rollup + debounce-window floors named (ISS-P13 / OQ-K). Branch first if on default; do
  not push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P9 — Import/export + "My Work" over the ONE inbox (the adoption gate + the inbox)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I5 (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I5", the two-pass
  ID-remapped import engine + the ADF lossy-map + "My Work" over the ONE inbox).
- **DEPENDS-ON.** ISS-P8 (the full issue model + rollup — import populates it). ISS-P2 (the issue.* tokens import
  emits + the notif-rules declared). The M2 Knowledge prompt that froze the ADF→myelin-content lossy-map (13.2).
  The M2 Notif prompts (list_inbox 7.1; mark/snooze 7.2; humanise 7.3; define_notif_rule 7.6). The M0
  protected-human-lane shed order (1.11). The index places this after ISS-P8 within M4 — it is the "first useful"
  milestone.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1 (EU-sovereign — "leave Atlassian cleanly" is a sovereignty credibility milestone) + §3
    (name-your-floors — the lossy nodes named never silent);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the export→import→export round-trip
    oracle; the resume-after-crash 0-dup), §4 (actually try it — the import is a real chained operation).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md (the
    two-pass, ID-remapped, idempotent + resumable import engine; the persisted source↔Myelin id map; the
    dry-run + reconciliation-report-first; the canonical interchange format; the per-tenant in-flight cap);
    03-events-contracts-and-glue.md (the import emits the normal issue.* events — one indexing path; "My Work"
    over the ONE inbox).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-2 (the
    ADF→myelin-content lossy-map frozen — lossy nodes named, never silent), OQ-L (the ONE templating surface),
    the permission-scheme mapping as the lossy/legal-review leg (R-9).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 13.2 (the ADF→myelin-content
    lossy-map — the import conversion table; every lossy/dropped node recorded in the import report), 7.1
    (list_inbox — the ONE inbox; "My Work" is a filter over reason/subject, never a second store), 7.2 (mark/
    snooze — one read-state truth), 7.3 (humanise — the ONE templating surface; the SLA at-risk/unblocked/
    approval-requested strings register here), 7.6 (define_notif_rule — the Issues reason set, now wired), 1.11
    (the protected-human-lane shed order — the import is capped so it never starves an interactive tenant), 2.2
    (the import emits issue.* via the one outbox path).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I5" (the work + the exit gate — ISS-D9; the
    floor named) + §6 (first-useful definition) + §5 (the import floor row, R-9).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row ISS-D9 ((a) export→import→export round-trips, ADF lossy nodes named; (b) a large import resumes after a
    crash, 0 duplicate creates; (c) the import doesn't starve another tenant).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate:
  - The two-pass, ID-remapped, idempotent + resumable import engine with a persisted source↔Myelin id map (the
    load-bearing artifact for idempotency/resume/rollback/round-trip); dry-run + reconciliation-report-first;
    source adapters (Jira/Linear/GitHub/CSV) normalising into one canonical interchange format that round-trips
    with the portability export. Import emits the normal issue.* events (one indexing path; reindex-from-source
    works on imported data for free), per-tenant in-flight capped (never starves another tenant — the protected
    human lane shed order 1.11).
  - Consume the frozen ADF→myelin-content lossy-map (13.2): every lossy/dropped conversion recorded in the import
    report, NEVER silent. The status/date/custom-emoji/layout/macro/permission-scheme degradations are named
    (permission-scheme mapping is the lossy/legal-review leg, R-9).
  - "My Work" (S10) = list_inbox(principal, filter) over the ONE Notif inbox (C-9): assigned/blocked/
    needs-approval/overdue are reason/subject filters with shared read-state — never a second store. Register the
    define_notif_rule set + the humanise templates (SLA at-risk / unblocked / approval-requested) into the ONE
    templating surface (no second template engine).
  - FLOOR named: import = canonical core + the four adapters + the frozen ADF map (permission-scheme mapping is
    the named lossy leg, R-9, M5+ legal); the canonical interchange is the round-trip oracle. Name it in the crate
    doc.
- **CONTRACTS TO IMPLEMENT.** 13.2 (consumed — the ADF lossy-map). 7.1/7.2 (consumed — list_inbox + mark/snooze
  for "My Work"). 7.3 (consumed — humanise, the ONE templating surface). 7.6 (consumed — the Issues notif-rules,
  now wired). 1.11 (consumed — the import shed budget). 2.2 (consumed — the import emits via the outbox).
  Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - ISS-D9(a) (export→import→export round-trips over a corpus; ADF lossy-map nodes NAMED never silent) — SCHED,
    the round-trip oracle (the named-lossy report) is the green artifact.
  - ISS-D9(b) (a large import resumes after a crash with 0 duplicate creates via the id map) — SCHED, 0 dup is the
    green artifact.
  - ISS-D9(c) (the import doesn't starve another tenant — a concurrent interactive tenant's latency stays within
    budget) — SCHED, the lane p99 within budget is the green artifact.
- **TESTS (required).** Unit tests for the id-map (idempotent re-create, resume, rollback) and the ADF lossy-map
  (each lossy node produces a report entry, never silent). A chained-mutation e2e test (export → import → export →
  assert round-trip + the named-lossy report). The drill scenario for ISS-D9(a/b/c). The provider/consumer CDC
  pair for 13.2 (the ADF map) + 7.1 (the inbox filter). State the cargo-mutants mutation-score floor for the
  id-map module if mandatory-core (the resume/dedup is data-loss-adjacent — treat it as mandatory-core).
- **DEFINITION OF DONE.** The import round-trips through the canonical interchange with named-lossy reporting, is
  idempotent/resumable (0 dup on crash), and respects the per-tenant lane budget; "My Work" reads the ONE inbox
  with one read-state truth; the humanise templates register on the ONE surface; ISS-D9(a/b/c) emit dated green
  artifacts; the unit + e2e + drill tests pass; the coverage scanner is green; the import floor (R-9) is named;
  the work is committed. The first-useful bar (§6) is met. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: Import/export + My Work over the ONE inbox. Body lists: contracts 13.2/7.1/7.2/
  7.3/7.6/1.11 consumed; ISS-D9(a) round-trip + (b) 0-dup-resume + (c) lane-within-budget greened with measured
  numbers; the import floor (R-9 permission-scheme leg) named. Branch first if on default; do not push unless
  asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P10 — The agent tool surface + reserve/settle + dry-run + the stateful Trigger (gated on AG-D4)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I6 (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I6", the Issues ToolDefs +
  EffectApi + the mock runtime + reserve/settle + the stateful Trigger). MUST NOT run any tool until AG-D4 /
  CI-T1 is green (the M2 GATE).
- **DEPENDS-ON.** ISS-P5 (the governed transitions the tools drive) + ISS-P8 (the OLAP the forecast agent reads).
  The M1 Identity prompts (delegation 4.5; mint_run_token 4.7). The M1 Storage prompt (reserve/settle cost gate
  11.7). The M2 Agent-fabric prompts (register_tool + the frozen requires_approval defaults 8.1; EffectApi::apply
  8.2; AgentRuntime::step --use-mock 8.3; ToolHands::exec the unified sandbox 8.4; run --dry-run 8.7). The M2 Bus
  prompts (arm_trigger/disarm_trigger 3.3; EventMatcher=QueryAst 3.4; the reactive/dispatch tier 3.6). The M2
  Workflow prompts (DurableExecutor 9.1; the timer wheel — stale_after 9.3; the workflow↔agent reserve/settle
  bookends 9.5). The AG-D4 / CI-T1 GATE (M2). The index places this after ISS-P8 within M4 — it requires AG-D4
  green.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native from the ground up; mock agents only during development — the strategy
    pattern; first-class triggers); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the
    HITL withhold 0-mutation, the mock-determinism), §8 (the human sign-off — HITL-gated governed transitions);
    ../../external-insights/03-agent-native-fabric.md (the plan-then-apply + the four uniform sandbox
    guarantees).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md (the
    Issues ToolDef catalogue; the frozen requires_approval defaults; the forecast + triage agents; the stateful
    Trigger flagship "Remind me when unblocked" — the armable-condition catalogue); 05-hard-problems.md (the
    reserve/settle + the agent-native posture).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-6 (the frozen
    requires_approval defaults — Issues forecast/triage/sla_draft = no, SLA transition = caveat-gated; the four
    uniform sandbox guarantees), OQ-F (the per-effect idem_key for HITL cards).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 8.1 (register_tool + the frozen
    requires_approval defaults), 8.2 (EffectApi::apply — plan-then-apply: schema → capability → delegation →
    tenant → budget → HITL gate → apply via the public endpoint → meter; a withheld gated tool does not mutate),
    8.3 (AgentRuntime::step --use-mock — the mock runtime; real-LLM is post-M5), 8.4 (ToolHands::exec — the
    unified sandbox; the AG-D4 gate), 8.7 (run --dry-run — proposed effects without applying), 4.5/4.7
    (delegation / mint_run_token — the run policy intersection + the per-run token), 11.7 (reserve/settle — the
    same wallet as CI runs), 3.3/3.4 (arm_trigger / EventMatcher=QueryAst — the stateful Trigger), 9.1/9.3/9.5
    (DurableExecutor / the stale_after timer / the reserve-settle bookends).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I6" (the work + the exit gate — ISS-D7,
    AG-D5/AG-D9 applied; the floor named) + §1 (sandbox escape is NOT owned by Issues — inherited; no agent tool
    over a red AG-D4) + §5 (the forecast + agent-runtime floor rows, R-5/R-10).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row ISS-D7 (the stateful Trigger — exactly-once across a restart, stale-once) + the shared AG-D5 (HITL
    withhold) + AG-D9 (mock determinism) applied to Issues' tools.
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate:
  - Register the Issues ToolDefs into the one ToolSurface (the same catalogue the command palette + CLI + agents
    share — UI=CLI=agent parity, no privileged back-channel): create/update/transition/comment/link/estimate/
    reorder/assign/close + the agent tools forecast/triage/sla_draft. Each declares required_caps, effect_kind,
    side_effecting, requires_approval, exposed_over_mcp.
  - The frozen requires_approval defaults (X-6): forecast/triage/sla_draft = no (suggest by default — the human
    accepts); transition(→done) on an SLA-bound issue = yes iff the transition has an approver edge; close = yes
    if confidential or governed. All side-effecting tools apply via EffectApi::apply (schema → capability →
    delegation → tenant → budget → HITL gate → apply via the public endpoint, NO carve-out → meter). A withheld
    gated tool does not mutate (AG-8).
  - The forecast agent (compute-only, reads OLAP; writes the forecast field + emits initiative.health_changed on
    crossing an at-risk threshold) and the triage agent (the S9 suggestion strip via run --dry-run — proposed
    effects without applying). The runtime is the MOCK (--use-mock, scripted-deterministic) per VISION §3; the
    real-LLM runtime is the post-M5 swap.
  - Reserve/settle on every spend-bearing run (reserve at dispatch — no balance, no start; settle on completion,
    never interrupt in-flight; integer minor-units; the same wallet as CI runs). The HITL approval card surfaces
    a live cost estimate before a human approves; the per-effect idem_key rule (OQ-F: card_id single,
    card_id:<effect_idx> multi/partial).
  - The stateful Trigger flagship ("Remind me when unblocked"): the armable-condition catalogue, each a frozen
    QueryAst over issue.* events + issue_relation projection state (Has/Ref/In); consumes the bus arm_trigger/
    disarm_trigger + the myelin-flow stale_after durable timer + the one inbox for on_resolve; fires once per
    arming; after stale_after (default 30d) a stale nudge fires once and the trigger goes stale.
  - FLOOR named: agent runtime = mock (the real-LLM runtime is post-M5, after the safety drills are green, R-10 —
    a config/impl swap, not a rewrite); forecast = linear remaining ÷ velocity (the Monte-Carlo agent is the
    follow-on, R-5, ISS-P13). Name both in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 8.1 (consumed — register the Issues ToolDefs with the frozen defaults). 8.2
  (consumed — apply via plan-then-apply, no carve-out). 8.3 (consumed — the mock runtime). 8.4 (consumed — the
  unified sandbox; AG-D4-gated). 8.7 (consumed — dry-run). 4.5/4.7 (consumed — delegation + the per-run token).
  11.7 (consumed — reserve/settle). 3.3/3.4 (consumed — the stateful Trigger). 9.1/9.3/9.5 (consumed — the
  durable timer + the reserve/settle bookends). Implement to the frozen shapes; escalate a needed change, do not
  diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - ISS-D7 (arm "remind me when unblocked"; resolve the last blocker across a restart → fires EXACTLY ONCE into
    the one inbox; after stale_after, the stale nudge fires once, the trigger goes stale) — CI, 1-fire +
    stale-once is the green artifact.
  - AG-D9 mock-determinism applied to Issues' agent tools (identical effect sequences across replays) — CI, the
    identical-sequence hash is the green artifact.
  - AG-D5 HITL withhold applied to a governed transition (0 mutation pre-approval, 1 apply post-approval) — CI,
    the 0-pre-approval-mutation counter is the green artifact.
  - Upstream AG-D4 / CI-T1 GREEN (the gate invariant — no Issues agent tool runs over a red sandbox-escape gate).
    State explicitly; do not mark done if AG-D4 is red (a dated "blocked on AG-D4" row, not a weakened gate).
- **TESTS (required).** Unit tests for the ToolDef defaults (a gated tool is withheld; a no-approval tool
  suggests) and the Trigger (exactly-once + stale-once across a simulated restart). A chained-mutation e2e test
  (dry-run a triage → human accepts → EffectApi applies once → reserve/settle balanced). The drill scenarios for
  ISS-D7 + the applied AG-D5/AG-D9. The provider/consumer CDC pair for 8.1 (the Issues ToolDefs) + 3.3 (the
  trigger). State the cargo-mutants mutation-score floor for the EffectApi-gate / HITL-withhold path
  (mandatory-core — the withhold is the no-unapproved-mutation seam).
- **DEFINITION OF DONE.** The Issues ToolDefs are registered with the frozen defaults; side-effecting tools apply
  only via plan-then-apply with no carve-out; the mock forecast/triage agents run deterministically; reserve/
  settle balances every run; the stateful Trigger fires exactly-once + stale-once; ISS-D7 + AG-D5 + AG-D9 emit
  dated green artifacts; AG-D4 is green (else a dated blocked row); the unit + e2e + drill tests pass; the
  coverage scanner is green; the mock-runtime + linear-forecast floors are named; the work is committed. No gate
  is greened by weakening a threshold or by running a tool over a red AG-D4.
- **COMMIT.** Header: P-<NNN> M4: Issues agent tools + reserve/settle + dry-run + stateful Trigger. Body lists:
  contracts 8.1/8.2/8.3/8.4/8.7 + 4.5/4.7 + 11.7 + 3.3/3.4 + 9.x consumed; ISS-D7 (1-fire/stale-once), AG-D9
  (identical sequences), AG-D5 (0 pre-approval mutation) greened; AG-D4 confirmed green; the mock-runtime (R-10)
  + linear-forecast (R-5) floors named. Branch first if on default; do not push unless asked. End with the
  workspace Co-Authored-By trailer.

---

### ISS-P11 — The SLA business-calendar engine + the CheckStatus guard (closing the X-1 consumer)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I7 (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I7", the SLA logic engine
  over myelin-flow + the CI-red governed-transition guard + the cross-subsystem reflexes + the governance admin
  views). Closes the Issues side of the X-1 seam — requires the CI producer (M4) + the Git projection (M3).
- **DEPENDS-ON.** ISS-P5 (the governed transitions) + ISS-P10 (the agent HITL-gated transition path) + ISS-P7
  (project() the guard reads through). The M2 Workflow prompts (the timer wheel + the durable signal 9.3/9.4).
  The M2 Notif prompts (oncall_now/page + the frozen escalation chain 7.5; humanise 7.3; list_subjects/explain
  4.4). The X-1 CheckStatus seam: the Git projection (M3, the consumer half) + the CI producer (M4) — proven
  end-to-end by GIT-D10 / CI-D8 (contract 5.9). The index places this LATE in M4 (after CI's producer lands) so
  the X-1 seam is closeable.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1 (corporate SLAs/reporting/audit); §3 (the poisoned-Done defence — never recompute trust);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — fire_at to-the-second across a
    restart; the guard blocks), §7 (the X-1 seam reconciled at the plan layer — Issues reads trust_tier off the
    fact, never recomputes it).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md (the
    SLA logic engine — the business-calendar arithmetic over an IANA-tz calendar; DST/holiday/multi-day; precompute
    fire_at + at_risk_fire_at; arm two timers; cheap disarm/re-arm; never poll); 03-events-contracts-and-glue.md
    (the CI-red governed-transition guard — read CheckStatus{state, trust_tier} via project(PR_ref); the
    cross-subsystem reflexes; the governance admin views S13–S18).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-1 (the Git↔CI
    CheckStatus seam — an untrusted_fork success is neutral until endorsed; Issues never recomputes trust), §5
    (the frozen escalation chain page → oncall_now → escalate-after-timer).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 5.9 (the Git↔CI CheckStatus seam
    — Issues reads CheckStatus{state, trust_tier} via the linked PR's project; never recomputes trust), 9.3 (the
    timer wheel — the SLA fire_at), 9.4 (the durable signal — multi-day HITL/escalation), 7.5 (oncall_now/page +
    the frozen escalation chain), 7.3 (humanise — the SLA strings), 4.4 (list_subjects/explain — the permission
    inspector S15), 4.2 (check + CaveatContext — the transition ABAC).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I7" (the work + the exit gate — ISS-D6, ISS-D12
    complete; needs GIT-D10/CI-D8) + §0 (the X-1 consumer-of-a-consumer posture) + §5 (the long-SLA
    history-compaction floor, R-11).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows ISS-D6 (SLA durability — fire after a restart; calendar corpus to-the-second; chain start), ISS-D12
    (the guard — "can't mark Done while CI red" + "can't close while blocked_by open"; the agent HITL-gated).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate:
  - The SLA logic engine over myelin-flow: the business-calendar arithmetic (convert a business-time budget into
    a wall-clock fire_at over an IANA-tz calendar; DST/holiday/multi-day correct); precompute fire_at +
    at_risk_fire_at, arm two timers on the wheel (9.3); cheap disarm/re-arm on pause/resume (the QueryAst
    pause_conditions); never poll, never pollute the wheel with calendar logic. On breach, start the FROZEN
    escalation chain (page → oncall_now → escalate-after-timer) as a durable workflow (7.5); breach/met feed OLAP
    for compliance reporting.
  - The CI-red governed-transition guard (the X-1 consumer half): the "can't mark Done while CI red on the linked
    PR" guard reads the linked PR's commit CheckStatus{state, trust_tier} via project(PR_ref) at transition time
    — checks state = success AND an acceptable trust posture (an untrusted_fork success is NEUTRAL until
    endorsed). Issues NEVER recomputes trust — it reads trust_tier off the fact. The agent hitting this governed
    transition is HITL-gated.
  - The cross-subsystem consumers (the cross-sub reflexes): git.branch.created / git.pr.opened / git.pr.merged →
    link + workflow-permitting auto-transition; chat.message.created → create issue with a relates edge;
    identity.member.* → reassign/anonymise; ci.check.updated → feed the guard.
  - The governance admin views (S13 workflow/scheme editor with the QueryAst guard builder; S14 SLA policy editor
    + calendar editor + breach-simulation; S15 team/project settings + the permission inspector via list_subjects/
    explain; S16 automation/trigger builder; S18 audit/change-history) — each preceded by its design sketch
    (VISION §3), the sketches signed off in the design folder.
  - FLOOR named: very-long time_to_resolution SLAs get history-compaction (the myelin-flow continue-as-new note)
    as the named follow-on (R-11, M5+). Name it in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 5.9 (consumed — the CheckStatus guard reads trust_tier off the fact, never
  recomputes). 9.3/9.4 (consumed — the SLA timers + the escalation durable signal). 7.5 (consumed — oncall_now/
  page + the chain). 7.3 (consumed — the SLA humanise strings). 4.4 (consumed — list_subjects/explain for the
  inspector). 4.2 (consumed — the transition ABAC). Implement to the frozen shapes; escalate a needed change, do
  not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - ISS-D6 ((a) breach fires after a process restart; (b) a business-calendar corpus — DST, multi-day, holiday,
    pause/resume → computed fire_at matches wall-clock TO THE SECOND; (c) breach starts the escalation chain) —
    CI, fire-at accuracy + chain-start is the green artifact.
  - ISS-D12 complete (the CI-red guard: "can't mark Done while CI red on the linked PR" reads CheckStatus + trust
    posture → transition blocked with a reason; "can't close while blocked_by open" blocks; an agent hitting a
    governed transition is HITL-gated, withheld, 0 mutation pre-approval) — CI, transition-blocked + 0
    pre-approval mutation is the green artifact.
  - Upstream GIT-D10 / CI-D8 GREEN (the X-1 seam end-to-end — the gate invariant: Issues' guard rests on a proven
    seam, not a doc claim). State explicitly; do not mark done if the seam is red (a dated "blocked on GIT-D10/
    CI-D8" row, not a guard that recomputes trust to fake green).
- **TESTS (required).** Unit tests for the business-calendar arithmetic (DST boundary, holiday, multi-day,
  pause/resume → fire_at correct) and the guard (an untrusted_fork success is neutral; a trusted success
  unblocks; the agent is withheld). A chained-mutation e2e test (arm an SLA → restart → assert breach fires +
  chain starts; attempt a Done transition while CI red → blocked → CI goes green → transition allowed). The drill
  scenarios for ISS-D6 + ISS-D12. The provider/consumer CDC pair for 5.9 (the CheckStatus consumer — Issues' read
  side) + 7.5 (the escalation chain). State the cargo-mutants mutation-score floor for the guard module
  (mandatory-core — the poisoned-Done defence is correctness-bearing).
- **DEFINITION OF DONE.** The SLA engine computes fire_at to-the-second over a business calendar, survives a
  restart, and starts the escalation chain on breach; the CI-red guard reads trust_tier off the fact (never
  recomputes) and blocks with a reason; the cross-sub reflexes fire; the admin views are sketched + signed off;
  ISS-D6 + ISS-D12 emit dated green artifacts; GIT-D10/CI-D8 are green (else a dated blocked row); the unit + e2e
  + drill tests pass; the coverage scanner is green; the long-SLA history-compaction floor (R-11) is named; the
  work is committed. No gate is greened by weakening a threshold or recomputing trust.
- **COMMIT.** Header: P-<NNN> M4: SLA business-calendar engine + CheckStatus guard (closes X-1 consumer). Body
  lists: contracts 5.9 (the CheckStatus consumer) + 9.3/9.4 + 7.5/7.3 + 4.4/4.2 consumed; ISS-D6 (fire-at
  to-the-second + chain start) + ISS-D12 (transition blocked, 0 pre-approval mutation) greened; GIT-D10/CI-D8
  confirmed green; the long-SLA history-compaction floor (R-11) named. Branch first if on default; do not push
  unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P12 — Real-time board sync + erasure-reaches-every-holder (the M4 consumer-band exit slice)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I8 (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I8", real-time board sync
  over the firehose resume-cursor protocol + the PersonalDataHolder erasure fan-out). The last M4 milestone
  before the band exit.
- **DEPENDS-ON.** ISS-P7 (the views the sync drives) + ISS-P3 (the holder registration + the per-subject-DEK
  columns) + ISS-P8 (the OLAP + the rollup holders) + ISS-P6 (the Search projection holder). The M2 Bus prompt
  (the firehose resume-cursor protocol — subscribe/resume/bounded scope 3.5). The M1 Identity prompt (erase + the
  pseudonym-map shred 4.8). The M1 GDPR prompts (the PersonalDataHolder ops 10.1; the erasure ledger 10.8; the
  post-restore re-erasure; the ONE posture by reference 10.9). The M1 Storage prompt (per-subject DEK crypto-shred
  11.4). The index places this LAST in M4 for Issues (it owns two M4 band-exit drills).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe by construction — data-subject erasure reaches every holder);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — 0 ops lost on reconnect; holder
    receipts on erasure), §1 (name-your-floors — the third-party residual is [OPEN — LEGAL]);
    ../../external-insights/04-hard-problems.md §1 (erasure-vs-immutability — the ONE posture).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md (the
    real-time board sync — optimistic local updates + bus-driven cache invalidation; subscribe(stream, scope =
    board:<id>) bounded never *; resume(stream, scope, last_seq) backfill then live; resync_required →
    *.snapshot; per-connection in-flight caps; presence/typing on the ephemeral firehose);
    06-reconciliation-compliance.md (the erasure fan-out across every Issues holder; the pseudonym-map shred;
    the *.erased tombstones; post-restore re-erasure).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-J (the firehose
    resume-cursor protocol — bounded scope, reconnect loses zero ops, resync_required fallback), X-7/OQ-G (the
    ONE free-text/immutable erasure posture — per-subject DEK + pseudonym-map shred + restrict; the third-party
    residual [OPEN — LEGAL]), OQ-K (the per-surface shed budget for the connection-storm).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 3.5 (the firehose resume-cursor
    protocol), 4.8 (erase + the pseudonym-map shred — "Former user 8a2f" without rewriting issues others own),
    10.1 (PersonalDataHolder{locate, export, rectify, restrict, erase} — across every Issues holder), 10.8 (the
    erasure ledger — post-restore re-erasure GD-14), 10.9 (the ONE posture by reference — the third-party residual
    [OPEN — LEGAL]), 11.4 (per-subject DEK crypto-shred), 2.7 (the *.erased tombstones live consumers act on),
    1.11 (the connection-storm shed budget).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I8" (the work + the exit gate — ISS-D13,
    ISS-D11; the floor named; the M4 band exit) + §5 (the sync + erasure floor rows, R-1/R-8).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows ISS-D13 (board sync — 0 ops lost on reconnect, resync fallback), ISS-D11 (erasure — PII gone from every
    holder, post-restore re-erasure, the third-party residual the documented limit).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate:
  - Real-time board sync over the frozen firehose resume-cursor protocol: optimistic local updates + bus-driven
    cache invalidation; subscribe(stream, scope = board:<id>) (bounded, NEVER *; a 50k-row board paginates its
    scope); on reconnect resume(stream, scope, last_seq) backfills (last_seq, now] then live — loses ZERO ops;
    last_seq past the retention window → resync_required → *.snapshot replay (NAMED, not silent). Per-connection
    in-flight frame caps; a slow consumer is dropped to resync_required (the OQ-K per-surface shed budget).
    Presence/typing ride the EPHEMERAL firehose, never the durable bus.
  - Erasure-reaches-every-holder: implement the PersonalDataHolder ops (locate/export/rectify/restrict/erase)
    across EVERY Issues holder (the issue row free-text via per-subject DEK shred, the change-log deltas,
    comments, attachment blobs, the OLAP read store + restriction flag, the Search index incl. embeddings, the
    Refs projection). Id erase shreds the pseudonym map ("Former user 8a2f" across history without rewriting
    issues others own); emit issue.*.erased tombstones (live consumers tombstone Search/Refs/OLAP/Notif);
    post-restore re-erasure (GD-14) runs against the erasure ledger. The third-party free-text residual is
    handled per the ONE platform posture BY REFERENCE (10.9), [OPEN — LEGAL].
  - FLOOR named: free-text PII erasure = per-subject DEK + pseudonym-map shred + restrict (the structural floor
    ships now; the third-party-mention residual basis is [OPEN — LEGAL], R-1); sync = optimistic + resume-cursor
    (offline/local-first is the named follow-on, R-8, post-M5, out of v1 scope unless promoted); worklog
    special-category classification is [OPEN — LEGAL], R-2. Name each in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 3.5 (consumed — the firehose resume-cursor protocol). 4.8 (consumed — erase + the
  pseudonym-map shred). 10.1 (owned — the Issues PersonalDataHolder ops). 10.8 (consumed — the erasure ledger +
  post-restore re-erasure). 10.9 (consumed — the ONE posture by reference). 11.4 (consumed — per-subject DEK
  shred). 2.7 (consumed — the *.erased tombstones). 1.11 (consumed — the connection-storm shed budget). Implement
  to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - ISS-D13 (a board at scope = board:<id> drops mid-edit-storm → resume backfill then live loses ZERO ops;
    last_seq past the window → resync_required → *.snapshot) — CI, 0 ops lost + the resync fallback is the green
    artifact.
  - ISS-D11 (erase a subject → PII gone from every holder: per-subject DEK, change-log, comments, attachments,
    OLAP + restriction, Search incl. embeddings, Refs; post-restore re-erasure catches a restore; the third-party
    residual is the documented [OPEN — LEGAL] limit) — SCHED, the per-holder receipts + the re-erasure is the
    green artifact.
- **TESTS (required).** Unit tests for the resume protocol (bounded scope; backfill then live; resync_required on
  a past-window last_seq) and the holder ops (each holder's erase shreds/tombstones; the pseudonym map shreds
  without rewriting others' issues). A chained-mutation e2e test (edit-storm → drop → resume → assert 0 ops lost;
  erase → assert every holder receipt → restore → assert re-erasure). The drill scenarios for ISS-D13 + ISS-D11.
  The provider/consumer CDC pair for 10.1 (the Issues holder ops) + 3.5 (the resume protocol). State the
  cargo-mutants mutation-score floor for the holder-erase module (mandatory-core — incomplete erasure is a GDPR
  failure).
- **DEFINITION OF DONE.** Real-time board sync over the resume-cursor protocol loses zero ops on reconnect and
  falls back to resync_required cleanly; erasure reaches every Issues holder with per-holder receipts +
  post-restore re-erasure, the third-party residual documented as [OPEN — LEGAL]; ISS-D13 + ISS-D11 emit dated
  green artifacts; the unit + e2e + drill tests pass; the coverage scanner is green; the sync + erasure floors
  (R-1/R-8/R-2) are named; the work is committed. The M4 band-exit slice Issues owns is green (ISS-D1/D2/D3 +
  ISS-D5/D6/D12 + ISS-D13/D11 + the X-1 seam). No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: Real-time board sync + erasure-reaches-every-holder. Body lists: contracts 3.5
  + 4.8 + 10.1/10.8/10.9 + 11.4 + 2.7 + 1.11 consumed; ISS-D13 (0 ops lost, resync fallback) + ISS-D11 (every
  holder receipt + re-erasure) greened; the sync (R-8) + erasure (R-1) + worklog (R-2) floors named. Branch first
  if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P13 — World-scale hardening + the floor follow-ons (the surge family + the measured promotions)

- **BAND.** M5.
- **ROADMAP MILESTONE.** M5-I9, the hardening + floor-follow-ons slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M5-I9", the move-CRDT / materialised rollup /
  distributed-SQL / cross-cell rollup / Monte-Carlo forecast / full DSR fan-out / column-store seam + the F6
  surge family + the scale benchmarks).
- **DEPENDS-ON.** ISS-P4 (the CAS floor the move-CRDT promotes) + ISS-P8 (the read-time rollup it materialises) +
  ISS-P10 (the linear forecast it promotes to Monte-Carlo) + ISS-P12 (the holders the DSR fan-out covers) +
  ISS-P6 (the planner the surge stresses). The M1 Tenancy prompt (the CrossCellPointer bridge frame 12.6, now
  live). The M5 cross-system prompts that bring multi-cell live + the full DSR fan-out (10.4; GA-D1/CP-D7/CP-D8).
  The M5 Knowledge prompt that promotes the CRDT (the shared Yrs type Issues reuses). The index places this in M5
  (all five subsystems on one substrate; the deterministic correctness drills green).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale; name-your-floors — promote on MEASURED evidence);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the 30x surge with quantified
    thresholds; the human lane holds, the agent lane sheds), §7 (the compounding payoff — promote a floor only on
    a measured trigger, never premature); ../../external-insights/04-hard-problems.md §2 (CRDT-after-CAS), §5
    (event-volume column-store seam — only once volume is measured).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/05-hard-problems.md (the move-CRDT
    promotion reusing the byte-identical order_key; the materialised-rollup trigger; the distributed-SQL trigger;
    the cross-cell portfolio rollup over the CrossCellPointer; the Monte-Carlo forecast; the column-store seam);
    07-drills-and-open-questions.md (the surge + scale drills).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-I (the cross-cell
    bridge — resolution always cell-local; the home cell renders + permission-checks; only the projection
    crosses), OQ-C (the materialisation trigger), OQ-K (the surge shed budgets).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 12.6 (the CrossCellPointer bridge
    — the cross-cell portfolio rollup), 10.4 (the DSR fan-out iterating member_cells), 3.5 (the resume-cursor
    transport the move-CRDT slots into), 11.6 (the OLAP for the Monte-Carlo throughput samples), 1.11 (the
    per-surface shed budgets the surge stresses).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M5-I9" (the work + the exit gate — the F6 surge,
    ISS-D2 at cell scale, ISS-D5 re-green across the CRDT boundary, GA-D1/CP-D7/CP-D8; the E2E wedge is ISS-P14)
    + §5 (the full floors register — every R-3..R-11 follow-on) + §6 (production-hardened definition).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows ISS-D5 (re-green across the move-CRDT engine-promote boundary), ISS-D2 (at cell scale under world-scale
    load) + the F6 surge family rows + GA-D1/CP-D7/CP-D8.
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate (the measured
  promotions are gated by their triggers — ship the seam + the measurement, promote on the measured signal):
  - The move-CRDT, after the CAS floor (R-3): a Yrs list / Fugue move-CRDT slotting into the SAME resume-cursor
    firehose transport, reusing Knowledge's Yrs type. Promoted ONLY on measured concurrent-reorder pain (the
    trigger). Because the order_key is already byte-identical, the promotion swaps the conflict-resolution engine,
    not the data model. ISS-D5 re-runs across the engine-promote boundary so it stays green when the CRDT lands.
  - Materialised rollup, after the read-time floor (R-4, KN-3): materialise a subtree's rollup only when it is
    MEASURED large; the read-time floor remains for small subtrees.
  - Distributed-SQL, after PG-sharded-by-tenant (R-6): only if a single tenant's shard is MEASURED to outgrow PG.
    Never premature — ship the measurement, not the migration, unless the trigger fires.
  - Cross-cell portfolio rollup, after single-cell (R-7, OQ-I): the rollup walk over a remote child rides the
    frozen PII-free CrossCellPointer{subject, type, correlation_id, home_cell}; resolution is always cell-local
    (the home cell renders + permission-checks; only the projection crosses). The FLOOR drills GA-D8/CP-D7/CP-D8
    are now owed (DSR fan-out iterates member_cells).
  - The Monte-Carlo forecast agent, after the linear floor (R-5): reads OLAP throughput samples; the swap is a
    strategy change, not a rewrite. (The real-LLM runtime swap, R-10, is post-M5/execution.)
  - The full DSR / erasure fan-out (10.4, GA-D1): every Issues holder now exists, so the fan-out is complete; the
    [OPEN — LEGAL] residual posture (10.9) is instantiated by reference.
  - The event-volume column-store seam (EI-04 §5): a seam for Issues' highest-volume streams (issue.updated, the
    change-log) — added only once volume is MEASURED, not before.
  - World-scale hardening: the 30x surge across the Issues owner (the protected human lane holds within budget;
    the agent lane sheds 429+Retry-After; cross-tenant impact 0); the prod-scale benchmarks (the 1M+-issue board,
    the 50-team-initiative rollup fan-out, millions of SLA timers as an indexed range read); online-migration-
    under-load on the hot issue tables; restore-verify at cell scale.
  - FLOOR named: each promotion is MEASURED — name the trigger in the crate doc (the floor stays until its
    measured signal fires); the real-LLM runtime (R-10) remains the post-M5 follow-on.
- **CONTRACTS TO IMPLEMENT.** 12.6 (consumed — the CrossCellPointer bridge, now live). 10.4 (consumed — the DSR
  fan-out across member_cells). 3.5 (consumed — the move-CRDT transport). 11.6 (consumed — the Monte-Carlo OLAP
  samples). 1.11 (consumed — the surge shed budgets). Implement to the frozen shapes; escalate a needed change,
  do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The F6 surge family across the Issues owner (SUB-D3-shaped: human lane within budget, agent sheds 429+
    Retry-After, cross-tenant impact 0) — SCHED, the lane-budget + the cross-tenant-0 signals are the green
    artifact.
  - ISS-D2 at cell scale re-confirmed (the 1M+-issue board under the <1s budget under world-scale load) — SCHED.
  - ISS-D5 re-green across the move-CRDT engine-promote boundary (if the CRDT is promoted; the drill was written
    to survive the swap) — CI.
  - GA-D1 / CP-D7 / CP-D8 (DSR fan-out 0 holders missed incl. Issues; cross-cell rollup per-cell receipt set +
    the PII-free bridge) — SCHED.
- **TESTS (required).** Unit tests for the move-CRDT promotion (the order_key data model is unchanged across the
  swap) and the cross-cell rollup (only the PII-free projection crosses; resolution is cell-local). A
  chained-mutation surge e2e test (30x mixed-principal load → assert the human lane holds + the agent lane sheds
  + cross-tenant impact 0). The drill scenarios for the F6 surge + ISS-D2-at-scale + ISS-D5-re-green +
  GA-D1/CP-D7/CP-D8. State the cargo-mutants mutation-score floor for any promoted core module (the move-CRDT
  conflict engine is correctness-bearing — treat it as mandatory-core when promoted).
- **DEFINITION OF DONE.** The floor follow-ons are promoted on their MEASURED triggers (or the floor + its
  measurement seam stand, named); the F6 surge holds (human lane within budget, agent sheds, cross-tenant 0);
  ISS-D2 at cell scale + ISS-D5 re-green + GA-D1/CP-D7/CP-D8 emit dated green artifacts; the unit + surge-e2e +
  drill tests pass; the coverage scanner is green; every measured-promotion trigger is named in writing; the work
  is committed. The production-hardened bar (§6) is met. No gate is greened by weakening a threshold or by
  promoting a floor without its measured trigger.
- **COMMIT.** Header: P-<NNN> M5: Issues world-scale hardening + floor follow-ons. Body lists: contracts 12.6/
  10.4/3.5/11.6/1.11 consumed; the F6 surge (lane budget + cross-tenant 0), ISS-D2-at-cell-scale, ISS-D5-re-green,
  GA-D1/CP-D7/CP-D8 greened with measured numbers; the measured-promotion triggers named (move-CRDT/materialised-
  rollup/distributed-SQL/cross-cell/Monte-Carlo/column-store); the real-LLM runtime (R-10) follow-on named.
  Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P14 — The whole-system E2E wedge (Issues' participation: E2E-1 / E2E-2 / E2E-3)

- **BAND.** M5.
- **ROADMAP MILESTONE.** M5-I9, the E2E-wedge slice (planning/06-roadmaps/subsystems/issue-tracker.md §"M5-I9",
  the whole-system E2E wedge — Issues' participation in E2E-1/E2E-2/E2E-3).
- **DEPENDS-ON.** ISS-P13 (the hardened Issues surface) + ISS-P11 (the X-1 guard + the governed transition) +
  ISS-P10 (the agent tool surface). The M5 cross-system E2E prompts that stand up the full cell with mock agents
  (testing-strategy §2). The Git/CI/Knowledge/Chat/Refs/Search/Notif prompts whose artifacts the scenarios chain
  (E2E-1 PR context pane; E2E-2 CI-fail → triage → issue → chat → fix-PR; E2E-3 spec-to-ship traceability). The
  index places this after ISS-P13 within M5 — it is the differentiator proof.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (the differentiator — work flows between tools; agents are first-class) + §1 (the agent-
    native flagship); ../../external-insights/01-process-and-quality-doctrine.md §4 (actually try it — the E2E
    scenarios chain mutations end-to-end, not single handlers), §3 (prove-it — each scenario emits a named green
    artifact).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/00-overview.md §1 (the cross-coupled
    posture — Issues is the node where the triaged failure becomes a governed work item); 03-events-contracts-and-
    glue.md (the cross-sub reflexes the scenarios exercise).
  - Testing strategy: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-
    catalogue.md §E2E (E2E-1 PR context pane: Issues' project resolves the linked issue per-viewer 0 leak; E2E-2
    the agent-native flagship: 0 effect outside the ∩, 0 mutation before approval, exactly-once approval + the
    governed transition across a kill, reserve/settle balanced; E2E-3 spec-to-ship: the spec→issue→PR→CI lineage
    per-viewer, cold-reindex == live, audit tamper detected) + §3.4 (the named green artifacts) + README.md (the
    strategy).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md (the X-1 seam + the
    agent ∩ the scenarios rest on).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M5-I9" (the E2E wedge bullets + the exit gate —
    E2E-1/E2E-2/E2E-3 green) + §0 (Issues on the E2E-2 flagship branch).
- **DELIVERABLE (what to build + exactly where in the repo).** In the workspace E2E test suite + the myelin-issues
  crate (Issues' participation in the chained scenarios against a full cell with MOCK agents):
  - E2E-1 PR context pane (Git+CI+Issues+Knowledge+Refs+Search+Id+Notif): Issues' project() resolves the linked
    issue per-viewer with 0 leak; the live check-update is within the freshness budget; a tombstone carries the
    root for a confidential issue.
  - E2E-2 CI-fail → triage agent → issue → chat → fix-PR (the agent-native flagship): Issues is the node where
    the triaged failure becomes a tracked, governed work item; 0 effect outside the ∩ (agent.policy ∩ delegation
    ∩ tenant.policy); 0 mutation before approval; exactly-once approval + the governed transition ACROSS A KILL;
    reserve/settle balanced.
  - E2E-3 Spec-to-ship traceability (Knowledge+Issues+Git+CI+Chat+Refs+Search+GDPR+Id): the spec→issue→PR→CI
    lineage per-viewer; cold-reindex == live (the reindex-from-source parity); audit tamper detected.
  - The Issues-side assertions + fixtures for each scenario, wired into the workspace E2E harness; each scenario
    emits its named green artifact (testing-strategy §3.4).
  - FLOOR named: none (the floors are promoted/named in ISS-P13). State that the scenarios run with the MOCK
    agent runtime (the real-LLM runtime is the post-M5 swap, R-10).
- **CONTRACTS TO IMPLEMENT.** No new contracts — this prompt EXERCISES the implemented contracts end-to-end (5.6
  project; 5.9 the CheckStatus guard; 8.2 EffectApi; 9.4 the durable HITL signal; 2.6 reindex-from-source; 10.4
  DSR). Assert each behaves to its frozen shape under the chained scenario; a divergence is escalated and written
  down (code-wins-over-docs), not papered over.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - E2E-1 green (Issues' linked-issue resolves per-viewer 0 leak; the live check-update within the freshness
    budget) — SCHED, the named E2E-1 green artifact.
  - E2E-2 green (0 effect outside the ∩; 0 mutation before approval; exactly-once approval + governed transition
    across a kill; reserve/settle balanced) — SCHED, the named E2E-2 green artifact (the agent-native flagship).
  - E2E-3 green (the spec→issue→PR→CI lineage per-viewer; cold-reindex == live; audit tamper detected) — SCHED,
    the named E2E-3 green artifact.
- **TESTS (required).** The three chained-mutation E2E scenarios themselves are the tests (they CHAIN operations
  across subsystems, not single handlers — EI-01 §4). Issues-side unit assertions for the per-viewer resolve (0
  leak), the governed-transition-across-a-kill (exactly-once), and the cold-reindex parity. The CDC pairs for the
  contracts Issues exercises (5.6, 5.9, 8.2) are re-asserted under the scenario. No new mutation floor (the core
  modules' floors were set in their own prompts); re-confirm they hold under the E2E load.
- **DEFINITION OF DONE.** E2E-1, E2E-2, E2E-3 each emit their dated named green artifact with Issues' assertions
  passing (per-viewer 0 leak; the agent-native flagship exactly-once + 0-pre-approval-mutation + reserve/settle
  balanced; cold-reindex == live + audit tamper detected); the scenarios chain mutations end-to-end with mock
  agents; the contract CDC pairs re-assert green under the scenario; the coverage scanner is green; the work is
  committed. No scenario is marked green by weakening an assertion. (Issues' M5 exit contribution — §"M5-I9" exit
  gate — is complete with ISS-P13's drills + these three E2E artifacts.)
- **COMMIT.** Header: P-<NNN> M5: Issues E2E wedge (E2E-1/E2E-2/E2E-3). Body lists: the three E2E scenarios
  greened (E2E-1 per-viewer 0 leak; E2E-2 the agent-native flagship — 0 effect outside ∩, exactly-once, reserve/
  settle balanced; E2E-3 cold-reindex == live + audit tamper detected); the contracts exercised (5.6/5.9/8.2/9.4/
  2.6/10.4); the mock-runtime note (real-LLM is post-M5). Branch first if on default; do not push unless asked.
  End with the workspace Co-Authored-By trailer.

---

### ISS-P15 — Dogfood: Myelin tracks its own issues (the switch test)

- **BAND.** M6.
- **ROADMAP MILESTONE.** M6-I10 (planning/06-roadmaps/subsystems/issue-tracker.md §"M6-I10", the Myelin roadmap/
  gap-report/scorecard as Myelin issues + the switch test). The done-bar for Issues as a product.
- **DEPENDS-ON.** ISS-P14 (the E2E wedge green — Issues carries its weight) + all prior Issues prompts (the full
  surface). The M5/M6 cross-system prompts that bring the platform to world-scale readiness + the self-hosting CI
  graph (master §2 M6). The index places this in M6 (you do not dogfood real team data onto a substrate whose
  restore-verify + DSAR fan-out are not green — master M6 entry dependency).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §5 (dogfooding — Myelin hosts itself); ../../external-insights/01-process-and-quality-
    doctrine.md §4 (the switch test — drive the real UI in a browser; a surface is done only when someone could
    move to it without hitting a wall the old tool didn't have — reached by DRIVING it, not reading the feature
    list), §1 (code-wins-over-docs — the truth-up pass).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/04-views-cli-and-api.md (the primary
    screens S1/S3/S5/S6/S9/S10/S13/S17/S19 + their empty/loading/error/permission/erased/agent-pending states);
    the design folder (information-architecture.md + user-flows.md + wireframes.md — the switch-test anchor).
  - Reconciliation/strategy: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-
    drill-catalogue.md row ISS-D14 (the switch test — create→triage→plan→board→done without a manual; measured
    contrast/latency on the primary screens incl. the empty/loading/error/permission/erased/agent-pending
    states; driven in a browser).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M6-I10" (the work + the exit gate — ISS-D14; no
    later-band gate red) + §6 (the done-bar).
  - Drills: ISS-D14 (the switch test, above).
- **DELIVERABLE (what to build + exactly where in the repo).** In the running Myelin platform + the myelin-issues
  crate:
  - The Myelin roadmap + gap report + scorecard live as MYELIN ISSUES (the every-incident-adds-a-drill loop files
    a Myelin issue + a reproducing drill); the team plans its own sprints on the platform's own board/roadmap.
  - Drive the real UI of the Issues surfaces (board/roadmap/backlog/table/cycle/triage/My Work + the admin/
    governance screens) in a BROWSER for the switch test (EI-01 §4, the frontend done-bar L5): can a Jira/Linear
    user complete the core loop create → triage → plan → board → done WITHOUT a manual? Measure contrast + latency
    on the primary screens S1/S3/S5/S6/S9/S10/S13/S17/S19, incl. the empty/loading/error/permission/erased/
    agent-pending states — reached by driving it, not by reading the feature list.
  - The truth-up pass (EI-01 §1): confirm every PROVEN Issues row rests on a dated green artifact (the gate
    invariant holds end-to-end); fix any doc the code has outrun (the code wins).
  - FLOOR named: any switch-test wall found is filed as a Myelin issue + a reproducing drill (the honest-floor
    rule — the gap is visible, never invisible); name the follow-on prompt/band for any deferred polish.
- **CONTRACTS TO IMPLEMENT.** No new contracts — this prompt DRIVES the implemented surface end-to-end as a real
  user. Where a screen lacks a sketch, produce one and have it reviewed before writing UI code (VISION §8). A
  doc/code divergence found in the truth-up pass is fixed in the doc (code-wins-over-docs), not papered over.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - ISS-D14 (the switch test: a Jira/Linear user completes create→triage→plan→board→done without a manual; +
    measured contrast/latency on the primary screens, incl. the empty/loading/error/permission/erased/agent-
    pending states — DRIVEN IN A BROWSER, not read off a feature list) — SCHED, the switch-test pass + the
    measured contrast/latency are the green artifact.
  - No later-band gate red (the truth-up pass confirms every PROVEN Issues row rests on a dated green artifact;
    code-wins-over-docs) — the truth-up scorecard is the green artifact.
- **TESTS (required).** The switch-test browser run itself (driven, with measured contrast/latency on the primary
  screens). The truth-up pass cross-checking every Issues PROVEN row against its dated green artifact. No new unit
  floor — re-confirm the existing drills (ISS-D1..ISS-D13) are still green on the self-hosted platform (the
  dogfood CI graph runs them on Myelin's own commits).
- **DEFINITION OF DONE.** Myelin tracks its own issues on the Issues surface; the switch test passes (driven in a
  browser, measured contrast/latency, all primary-screen states reached); the truth-up pass confirms 0 red
  earlier-band Issues gates and every PROVEN row dated-green; any switch-test wall is filed as a Myelin issue +
  drill (visible, named); the work is committed. The Issues done-bar (§6) is met. No gate is greened by reading a
  feature list instead of driving the UI.
- **COMMIT.** Header: P-<NNN> M6: Dogfood — Myelin tracks its own issues (the switch test). Body lists: ISS-D14
  greened (switch-test pass + measured contrast/latency, driven in a browser); the truth-up pass (0 red earlier
  gates, every PROVEN row dated-green); any switch-test wall filed as a Myelin issue + drill (named). Branch first
  if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

## Coverage digest (every issue-tracker roadmap milestone → its prompt(s))

| Roadmap milestone (06-roadmaps/subsystems/issue-tracker.md) | Band | Prompt(s) |
|---|---|---|
| 3.0 pre-work (M1 slice — ReBAC fragment + holder tags) | M1 | ISS-P1 |
| 3.0 pre-work (M2 slice — myelin-query co-own + issue.* tokens + IndexSpec/notif declares) | M2 | ISS-P2 |
| M4-I1 (issue spine + silent-data-loss-safe write path) | M4 | ISS-P3 |
| M4-I1 (Hi/Lo keys + order_key CAS + content body) | M4 | ISS-P4 |
| M4-I2 (governance schemes + workflow FSM interpreter) | M4 | ISS-P5 |
| M4-I3 (query planner — SetExpr push-down + cost-bounding) | M4 | ISS-P6 |
| M4-I3 (co-equal views + Refs/Search wiring) | M4 | ISS-P7 |
| M4-I4 (rollup + cycles/milestones + attachments + OLAP) | M4 | ISS-P8 |
| M4-I5 (import/export + My Work over the ONE inbox) | M4 | ISS-P9 |
| M4-I6 (agent tool surface + reserve/settle + dry-run + Trigger) | M4 | ISS-P10 |
| M4-I7 (SLA business-calendar engine + CheckStatus guard) | M4 | ISS-P11 |
| M4-I8 (real-time board sync + erasure-reaches-every-holder) | M4 | ISS-P12 |
| M5-I9 (world-scale hardening + floor follow-ons) | M5 | ISS-P13 |
| M5-I9 (the whole-system E2E wedge — E2E-1/E2E-2/E2E-3) | M5 | ISS-P14 |
| M6-I10 (dogfood — Myelin tracks its own issues; the switch test) | M6 | ISS-P15 |

**Drill coverage (every ISS drill → the prompt that greens it):** ISS-D10/D4/D5 → ISS-P4; ISS-D12 (guard half) →
ISS-P5; ISS-D3/D2/D1 → ISS-P6/ISS-P7; ISS-D8 → ISS-P8; ISS-D9 → ISS-P9; ISS-D7 + AG-D5/AG-D9 (applied) →
ISS-P10; ISS-D6/ISS-D12 (complete) → ISS-P11; ISS-D13/ISS-D11 → ISS-P12; the F6 surge + ISS-D2-at-cell-scale +
ISS-D5-re-green + GA-D1/CP-D7/CP-D8 → ISS-P13; E2E-1/E2E-2/E2E-3 → ISS-P14; ISS-D14 → ISS-P15. The
emit-iff-committed (SUB-D1/BUS-D4 shape applied to Issues) → ISS-P3. No ISS drill and no Issues milestone is
left ungreened.

**Floor coverage (every floor → the prompt that ships it + the follow-on prompt/band):** CAS ranking (ISS-P4) →
move-CRDT (ISS-P13, M5); read-time rollup (ISS-P8) → materialised (ISS-P13, M5); linear forecast (ISS-P10) →
Monte-Carlo (ISS-P13, M5); GIN-default facet (ISS-P6) → projection-feeder generated index (ISS-P6/ISS-P13,
measured); PG-sharded (ISS-P3) → distributed-SQL (ISS-P13, M5); optimistic+resume sync (ISS-P12) → offline/
local-first (post-M5); SLA business-calendar (ISS-P11) → long-SLA history-compaction (M5+); import canonical core
(ISS-P9) → permission-scheme mapping (M5+ legal); per-subject-DEK erasure (ISS-P12) → third-party residual
[OPEN — LEGAL] (parallel legal); worklog tags (ISS-P1) → special-category ratification (parallel legal);
single-cell (ISS-P3..P12) → cross-cell rollup over the CrossCellPointer bridge (ISS-P13, M5); mock agent runtime
(ISS-P10) → real-LLM runtime (post-M5/execution). Every floor's pair is linked, the gap visible.
