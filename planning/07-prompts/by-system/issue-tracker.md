# Phase 7 — Prompt Ledger: Issue Tracker (the most cross-coupled consumer subsystem)

> Granularity note (Phase 7-A finer-granularity pass): the first pass shipped 15 prompts (ISS-P1..ISS-P15);
> this rewrite splits every multi-deliverable prompt into single-deliverable, clean-context units — **15 → 37
> prompts (ISS-P01..ISS-P37)** — preserving all coverage (every milestone / contract / drill / floor the first
> pass covered remains, now at finer granularity). No milestone is dropped; the splits expose the bundled
> sub-deliverables (e.g. the spine vs the write-path vs the pseudonymous columns; the SetExpr lowering vs the
> cost-bounder vs the projection feeder; the SLA engine vs the CheckStatus guard vs the cross-sub reflexes vs
> the admin views) as their own independently-committable prompts.
>
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
> handle ISS-P<nn> so its DEPENDS-ON edges are unambiguous before global numbering; the index rewrites ISS-P<nn>
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
> Coverage (milestone → finer prompts): pre-work 3.0 → ISS-P01 (M1) + ISS-P02/P03/P04 (M2); M4-I1 →
> ISS-P05/P06/P07 (spine) + ISS-P08/P09/P10 (keys/CAS/content); M4-I2 → ISS-P11/P12; M4-I3 → ISS-P13/P14/P15
> (planner) + ISS-P16/P17 (views + Refs/Search wiring); M4-I4 → ISS-P18/P19/P20; M4-I5 → ISS-P21/P22; M4-I6 →
> ISS-P23/P24/P25; M4-I7 → ISS-P26/P27/P28/P29; M4-I8 → ISS-P30/P31; M5-I9 → ISS-P32/P33 (hardening) +
> ISS-P34/P35/P36 (E2E wedge — split below); M6-I10 → ISS-P37 (the dogfood switch test). (See the coverage
> digest at the foot for the exact final numbering — 37 prompts, no milestone gap.)

---

### ISS-P01 — Freeze the Issues ReBAC fragment + the worklog PersonalDataHolder tags (so dependents compile)

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
    opens in ISS-P07) and apply the #[personal_data(category, role, basis, retention, erasure, subject_locator)]
    tags on the (still-skeletal) issue schema types — the worklog/productivity/estimate fields tagged
    category=behavioural, role=tenant-content, basis=TBD-LEGAL, retention=tenant-policy, restricted-by-default
    (OQ-H), and the free-text title/props/comment/change-delta fields — so the no-untagged-personal-data lint is
    green from the first migration (ISS-P05).
  - FLOOR named: none new here (this is a contract-fragment freeze, not a feature). Name the [OPEN — LEGAL]
    residual on the worklog tag (basis=TBD-LEGAL, R-2: special-category-vs-elevated ratification is a parallel
    legal track, the structural tag ships now). State in the crate doc that no Issues feature ships here — only
    the shapes other systems compile against — and name ISS-P07 as the milestone where the holder is opened.
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
  red+green fixtures; the no-feature floor named (ISS-P07 opens the holder) + the worklog [OPEN — LEGAL] residual
  (R-2). Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P02 — Co-own myelin-query byte-identical with Knowledge (the field-type enum / ViewSpec / QueryAst / order_key codec)

- **BAND.** M2.
- **ROADMAP MILESTONE.** Pre-work 3.0, the M2 slice — the myelin-query co-ownership bullet
  (planning/06-roadmaps/subsystems/issue-tracker.md §3.0, the "Co-own the frozen myelin-query crate" bullet).
- **DEPENDS-ON.** ISS-P01 (the issues crate exists). The M2 prompt(s) that establish myelin-query as a frozen
  shared crate (13.3) — Knowledge leads, Issues co-owns. The index places this in M2 alongside the shared-crate
  freeze, paired with Knowledge's myelin-query prompt (the byte-identity fixture cross-checks both serializers).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (one shared identity/permission/event model — no drift);
    ../../external-insights/01-process-and-quality-doctrine.md §7 (reconcile cross-component contracts at the
    plan layer before either side ships — a unit mismatch that ships calcifies), §5 (the ratchet).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/01-tech-and-data-model.md (the
    field-type enum + ViewSpec + the order_key/LexoRank codec Issues co-owns).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-3 (the
    myelin-query primitive frozen byte-identical with Knowledge — field-type enum, ViewSpec, QueryAst, order_key).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 13.3 (myelin-query frozen
    byte-identical: the field-type enum, ViewSpec, QueryAst, the order_key/LexoRank encoding — base-62
    0-9A-Za-z, lexicographic compare, midpoint bisection, 2-char jitter, 48-char rebalance, created_at+ULID
    tiebreak — Issues + Knowledge co-own).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §3.0 (the myelin-query bullet) + §2 (upstream dep
    row 13.3) + §4 (contracts-by-milestone, the 3.0 row 13.3).
- **DELIVERABLE (what to build + exactly where in the repo).** In the shared myelin-query crate (co-owned):
  - Contribute the storage discipline into the frozen myelin-query crate (13.3): the field-type enum, the
    ViewSpec, the QueryAst, and the order_key/LexoRank codec — definitions BYTE-IDENTICAL with Knowledge. Issues
    owns its own AST→store compiler (which lands in ISS-P13); the DEFINITIONS land here, frozen.
  - Ship the round-trip + byte-identity test fixture (the drift-killer): the same ViewSpec/QueryAst/order_key
    serialized by Issues and by Knowledge produce byte-identical output, and the order_key codec round-trips per
    the frozen rules (midpoint bisection, 2-char jitter, 48-char rebalance trigger, created_at+ULID tiebreak).
  - FLOOR named: none. State that no Issues data is written yet (the compiler/emitter wiring lands in M4-I1+, and
    the AST→store lowering is ISS-P13). Name those follow-ons in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 13.3 myelin-query (co-owned — the byte-identical definitions + Issues' storage
  discipline; the compiler lands in ISS-P13). Implement to the frozen shape; escalate a needed change, do not
  diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The myelin-query byte-identity fixture is GREEN (0 byte differences between the Issues- and Knowledge-
    serialized ViewSpec/QueryAst/order_key; the order_key codec round-trips per the frozen rules) — CI, the
    byte-diff count = 0 is the green artifact. (This is the drift-killer the §3.0 exit gate names.)
- **TESTS (required).** Unit tests for the order_key bisection/2-char-jitter/48-char-rebalance behaviour and the
  created_at+ULID tiebreak. The byte-identity fixture cross-checked against Knowledge's serializer. The
  provider/consumer CDC pair for 13.3 (the co-owned definitions). State the cargo-mutants mutation-score floor
  for the order_key codec module (it is the rank source of truth — treat it as mandatory-core).
- **DEFINITION OF DONE.** The myelin-query definitions are byte-identical with Knowledge (fixture green, byte-diff
  = 0); the order_key codec round-trips; the CDC pair + unit tests pass; the contract-coverage scanner is green
  on row 13.3; the no-data-yet + compiler-in-ISS-P13 notes are written; the work is committed. No gate is greened
  by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M2: Issues co-owns myelin-query byte-identical. Body lists: 13.3 co-owned
  byte-identical (byte-diff = 0); the byte-identity fixture greened; the order_key codec round-trip proven; the
  compiler follow-on named (ISS-P13). Branch first if on default; do not push unless asked. End with the
  workspace Co-Authored-By trailer.

---

### ISS-P03 — Register the complete issue.* event taxonomy + the initiative token (under the Bus grammar)

- **BAND.** M2.
- **ROADMAP MILESTONE.** Pre-work 3.0, the M2 slice — the issue.* event-tokens bullet
  (planning/06-roadmaps/subsystems/issue-tracker.md §3.0, the "Register the complete issue.* event taxonomy +
  the initiative type token" bullet).
- **DEPENDS-ON.** ISS-P01 (the issues crate + the fragment exist). The M2 Bus prompt that seeds the event
  taxonomy grammar + registers the initiative token (contract 2.9) under the EventEnvelope anchor (2.1). The
  index places this in M2 alongside the Bus taxonomy seed.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (one shared event model — no drift);
    ../../external-insights/01-process-and-quality-doctrine.md §7 (reconcile field names + units at the plan
    layer before either side ships), §5 (the ratchet).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md (the
    complete issue.* taxonomy incl. initiative; the EventEnvelope alignment).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §2 (the initiative
    token registered).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 2.9 (event taxonomy + the
    initiative type token), 2.1 (the EventEnvelope frozen anchor — timestamps RFC-3339 UTC; durations in seconds;
    actor/subject as ArtifactRefs; contains_personal_data/data_role/pii_key_ref on PII-bearing events).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §3.0 (the issue.* tokens bullet) + §2 (upstream dep
    rows 2.1/2.9) + §4 (the 3.0 row 2.9).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate (the token
  registration into the Bus §6 grammar seed):
  - Register the complete issue.* event taxonomy + the initiative type token (2.9) under the Bus §6 grammar, with
    names/units aligned to the EventEnvelope anchor: timestamps RFC-3339 UTC; SLA targets/stale_after/durations
    in seconds; estimates/story-points numeric; actor/subject as ArtifactRefs;
    contains_personal_data/data_role/pii_key_ref on any PII-bearing event. The complete v1 list named in arch 03
    (issue.created/updated/transitioned/commented/linked/reordered/assigned/erased, initiative.health_changed,
    etc.).
  - FLOOR named: none. State that no Issues data is written yet (the emitter wiring lands in M4-I1+, ISS-P06);
    name it in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 2.9 the issue.* tokens + initiative (owned — registered into the Bus seed). 2.1
  (consumed — the EventEnvelope unit anchor). Implement to the frozen shapes; escalate a needed change, do not
  diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The issue.* tokens (incl. initiative) parse under the §6.2 grammar (0 ungrammatical tokens) and the
    EventEnvelope units validate (durations in seconds, timestamps RFC-3339 UTC) — CI, 0 ungrammatical tokens +
    unit-valid is the green artifact.
- **TESTS (required).** Token grammar round-trip tests (every issue.* token parses + serializes). Unit tests that
  the EventEnvelope units validate (a seconds-vs-millis fixture is rejected). The provider/consumer CDC pair for
  2.9 (the issue.* tokens). No mutation floor required here unless the token validator is mandatory-core; state
  yes/no.
- **DEFINITION OF DONE.** The issue.* tokens are registered and grammatical with valid units; the CDC pair + unit
  tests pass; the contract-coverage scanner is green on row 2.9; the no-data-yet note is written; the work is
  committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M2: Issues issue.* event taxonomy + initiative token. Body lists: 2.9 issue.* +
  initiative registered, 2.1 EventEnvelope units validated; the grammar round-trip greened (0 ungrammatical
  tokens). Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P04 — Declare the Issues IndexSpec (declare_indexable) + the define_notif_rule reason set

- **BAND.** M2.
- **ROADMAP MILESTONE.** Pre-work 3.0, the M2 slice — the declare bullet
  (planning/06-roadmaps/subsystems/issue-tracker.md §3.0, the "Declare the Issues declare_indexable IndexSpec +
  the define_notif_rule set" bullet).
- **DEPENDS-ON.** ISS-P03 (the issue.* tokens — the IndexSpec projects them; the notif reasons name them). The
  M2 Search prompt that ships declare_indexable (6.3) and the M2 Notif prompt that ships define_notif_rule (7.6).
  The index places this in M2 after ISS-P03, alongside the Search + Notif declares.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (one search/notif model — no drift);
    ../../external-insights/01-process-and-quality-doctrine.md §7 (declare the projection + reasons at the plan
    layer so the consumers compile), §5 (the ratchet).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md (the
    declare_indexable IndexSpec — the issue.* facets projection; the define_notif_rule reason set).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 6.3 (declare_indexable — the
    issue.* facets projection: ft_fields, struct_fields, acl_object_type=issue), 7.6 (define_notif_rule — the
    Issues SLA/unblocked/approval reason set).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §3.0 (the declare bullet) + §2 (upstream dep rows
    6.3, 7.6) + §4 (the 3.0 declare rows for 6.3/7.6).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate (the declarations
  Search + Notif compile against):
  - Declare the Issues declare_indexable IndexSpec (6.3): the issue.* facets projection shape (ft_fields,
    struct_fields, acl_object_type=issue) so Search knows Issues' projection exists. The live emitter lands in
    ISS-P17 (the issue.* Search projection); the DECLARATION is the deliverable here.
  - Declare the define_notif_rule set (7.6): the Issues reason set (SLA at-risk, unblocked, approval-requested) so
    Notif knows Issues' reasons exist. The wiring lands in ISS-P22 ("My Work"); the DECLARATION is the deliverable
    here.
  - FLOOR named: none. State that the emitter/wiring lands in M4 (the projection emitter in ISS-P17, the notif
    wiring in ISS-P22); name both in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 6.3 declare_indexable (owned — the IndexSpec declared). 7.6 define_notif_rule
  (owned — the reason set declared). Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The declare_indexable IndexSpec registers with Search and the define_notif_rule set registers with Notif; both
    compile and the registrations are accepted (a build-time gate) — CI, the accepted-registration is the green
    artifact.
- **TESTS (required).** Unit tests that the IndexSpec + the notif reason set serialize to the frozen shape. The
  provider/consumer CDC pair for 6.3 (the IndexSpec) + 7.6 (the reason set). No mutation floor required (these are
  declarations, not core logic); state so.
- **DEFINITION OF DONE.** The IndexSpec + notif-rules are declared and accepted; the CDC pairs + unit tests pass;
  the contract-coverage scanner is green on rows 6.3/7.6; the emitter/wiring follow-ons (ISS-P17/ISS-P22) are
  named; the work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M2: Issues IndexSpec + notif-rule declares. Body lists: 6.3/7.6 declared and
  accepted; the projection-emitter (ISS-P17) + notif-wiring (ISS-P22) follow-ons named. Branch first if on
  default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P05 — The issue-spine migrations (the typed core + JSONB tail + relations + change-log + scheme/cycle/milestone tables)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I1, the schema/migration slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I1", the issue table + issue_relation + change-log +
  scheme/cycle/milestone/prefix_counter/consumer_dedup/outbox bullets).
- **DEPENDS-ON.** ISS-P01 (the ReBAC fragment + holder tags), ISS-P03 (the issue.* tokens). The M0 outbox
  prompts (the outbox table + consumer_dedup, 2.3/2.5). The M1 Storage prompts (OLTP + RLS + encrypted columns +
  the outbox 11.1; restore-verify 11.5 — STOR-D1). The M1 Tenancy prompts ((tenant,region) partition 12.1;
  residency_verify 12.4). The M1 GDPR prompt (PersonalDataHolder spine 10.1). This is the first Issues prompt
  that lays down schema — the index places it after the full M1 substrate, in M4.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scalable, GDPR-safe by construction, name-your-floors);
    ../../external-insights/01-process-and-quality-doctrine.md §2 (order-by-non-negotiability — the schema is the
    floor under all of Issues), §5 (the ratchet — the forward-only-migration lint).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/01-tech-and-data-model.md (the issue
    table — typed core + JSONB tail + the (tenant,region) partition key + the lifecycle/GDPR columns;
    issue_relation TE-7 source of truth; issue_change_log; the scheme/cycle/milestone tables; prefix_counter).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-7 (the lifecycle/
    GDPR columns the erasure posture needs).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 1.1–1.4 (serve/three-surface/
    liveness≠readiness/PersonalDataHolder auto-reg — boot from the shell), 1.5 (forward-only migrations + the
    hot-table flags; expand→backfill→contract), 2.3/2.5 (the outbox table + consumer_dedup), 11.1 (OLTP + RLS +
    encrypted columns + the outbox), 11.5 (restore-verify — STOR-D1), 12.1/12.4 (partition key +
    residency_verify), 10.1 (PersonalDataHolder).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I1" (the schema bullets) + §2 (the starred
    upstream deps rows 11.1, 12.1, 11.5) + §4 (the 1.5 hot-table-flag row).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    the upstream STOR-D1 row (the migrations apply onto a restore-verified store).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate as an AppSpec + the
  harness-wired migrations (not a hand-rolled main, per the substrate convention):
  - The forward-only migrations for the issue table (typed core columns + a JSONB property-bag tail + the
    (tenant,region) partition key + the lifecycle/GDPR columns), issue_relation (TE-7 source of truth, forward
    edge), issue_change_log, the scheme/scheme_assignment/cycle/cycle_membership/milestone/prefix_counter tables,
    consumer_dedup, and the per-service outbox table.
  - Flag issue/issue_relation/issue_change_log as hot tables (1.5; expand→backfill→contract; the
    forward-only-migration lint holds). Boot the Issues service from serve(AppSpec) (1.1–1.3); register the store
    so PersonalDataHolder auto-registration fires (1.4 — the holder ops are todo-stubbed; ISS-P07 wires the
    per-subject-DEK columns, ISS-P31 the full ops). Every table carries the (tenant,region) partition + RLS.
  - FLOOR named: storage = PG-hybrid sharded by tenant (distributed-SQL is the measured follow-on, R-6, ISS-P32).
    Name it in the crate doc; the write path is ISS-P06, the pseudonymous/DEK columns ISS-P07.
- **CONTRACTS TO IMPLEMENT.** 1.1–1.4 (consumed — boot from serve, holder auto-reg). 1.5 (consumed — forward-only
  hot-table migrations). 2.3/2.5 (consumed — the outbox table + dedup table shapes). 11.1 (consumed — OLTP + RLS +
  encrypted columns + the outbox). 11.5 (consumed — restore-verify). 12.1/12.4 (consumed — partition + residency).
  10.1 (consumed — the holder). Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The migrations apply forward-only on a freshly-served store (expand→backfill→contract; the
    forward-only-migration lint green; the hot-table flags present) — CI, the lint signal + a clean migrate is the
    green artifact.
  - The tenant-predicate + residency-pin lints are GREEN on every Issues table (0 un-scoped tables) — CI.
  - Upstream STOR-D1 (restore-verify, RPO ≤ 5 min / RTO ≤ 1h-tenant) GREEN — the gate invariant: Issues lays no
    schema that will hold real data over a red restore-verify. State explicitly; record a dated "blocked on
    STOR-D1" scorecard row rather than weakening if it is red.
- **TESTS (required).** Unit/migration tests that the schema applies forward-only and re-applies idempotently; a
  fixture asserting every table is (tenant,region)-partitioned + RLS-scoped (the tenant-predicate lint's green
  fixture). The provider/consumer CDC stub for the outbox table shape (2.3). No mutation floor on pure schema; if
  the partition/RLS predicate helper is core, state its floor.
- **DEFINITION OF DONE.** The migrations apply forward-only and re-apply idempotently; every table is partitioned
  + RLS-scoped (lints green); the holder auto-registers (ops stubbed); STOR-D1 is green (else a dated blocked
  row); the tests + coverage scanner pass; the PG-sharded floor is named with ISS-P32; the work is committed.
  "Looks done" is not done.
- **COMMIT.** Header: P-<NNN> M4: Issue-spine migrations (typed core + JSONB tail + relations + scheme/cycle
  tables). Body lists: contracts 1.1–1.5/11.1/11.5/12.1/12.4/10.1 consumed; the forward-only-migration +
  tenant-predicate + residency-pin lints greened; STOR-D1 confirmed green; the PG-sharded floor named (ISS-P32).
  Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P06 — The silent-data-loss-safe write path (validate → check → mutate → OutboxTx::emit in one tx)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I1, the write-path slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I1", the minimal write path + the outbox
  emit-iff-committed bullets).
- **DEPENDS-ON.** ISS-P05 (the schema + the outbox table). ISS-P03 (the issue.* tokens). The M0 outbox prompts
  (OutboxTx::emit + EventHandler + consumer_dedup, 2.2–2.5). The M1 Identity prompts (check + CaveatContext 4.2;
  write_tuples/zookie 4.6/4.10). This is the first Issues prompt that writes data — the index places it
  immediately after ISS-P05 within M4.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors); ../../external-insights/01-process-and-quality-doctrine.md §2
    (order-by-non-negotiability — silent data loss outranks every feature), §3 (prove-it — outbox
    emit-iff-committed with a telemetry signal); §4 (chained-mutation tests, not single-handler).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md
    §"write path" (validate → check → mutate → OutboxTx::emit; the issue is the aggregate, UNIQUE(aggregate,seq)).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 2.1/2.2/2.3/2.5
    (envelope/outbox/outbox-table/dedup — the issue is the aggregate, UNIQUE(aggregate, seq)), 4.2 (check +
    CaveatContext — the write gate), 4.6/4.10 (write_tuples/zookie — assign/watch/confidential-grant + zookie).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I1" (the write-path bullet + the
    emit-iff-committed exit gate) + §1 (the non-negotiability order — write-loss is Tier-1).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    the outbox emit-iff-committed shape (SUB-D1/BUS-D4 applied to Issues).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate (the state-changing
  handler):
  - The minimal write path as a state-changing handler: validate → Id.check (+ CaveatContext) → mutate the typed
    core → OutboxTx::emit IN THE SAME TRANSACTION. The issue is the aggregate (UNIQUE(aggregate, seq) per-issue
    ordering). No publish_now — the no-raw-publish lint holds. (Key allocation + order_key CAS land in ISS-P08/
    ISS-P09; here the write path emits with a placeholder key + a plain typed-core mutation so the
    emit-iff-committed seam is proven FIRST, before keys/CAS/content layer on top.)
  - Wire write_tuples for assign/watch/confidential-grant (4.6) + the zookie return (4.10) on the mutating path.
  - FLOOR named: ranking = order_key + server-arbitrated CAS arrives in ISS-P09 (move-CRDT is the M5 follow-on,
    ISS-P32). Name it in the crate doc; the keys land in ISS-P08, the content body in ISS-P10.
- **CONTRACTS TO IMPLEMENT.** 2.1/2.2/2.3/2.5 (consumed — the issue.* shapes via the one emit path, per-aggregate
  ordering, dedup). 4.2 (consumed — the write gate). 4.6/4.10 (consumed — write_tuples + zookie). Implement to the
  frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - Outbox emit-iff-committed for the issue write path (the SUB-D1/BUS-D4 shape applied to Issues: kill the
    service between commit and publish → the issue.* event is delivered exactly when its row committed, never
    without it; 0 ghost, 0 lost) — CI, the outbox-depth + consumer-dedup telemetry signals are the green artifact.
  - The no-raw-publish lint is GREEN (0 publish_now call sites; the emit is the only path) — CI.
- **TESTS (required).** Unit tests for the write-path transaction (the emit is in the same tx; a rolled-back
  mutation emits nothing). A chained-mutation end-to-end test (create then update then transition — chained, not
  single-handler, per EI-01 §4) asserting per-aggregate seq monotonicity and dedup on replay. The drill-harness
  scenario for the kill-between-commit-and-publish (emit-iff-committed). The provider/consumer CDC pair for the
  issue.* outbox rows (2.2/2.3). State the cargo-mutants mutation-score floor for the write-path / outbox-emit
  module (mandatory-core — it is the write-loss seam).
- **DEFINITION OF DONE.** The write path co-commits its event through the outbox (emit-iff-committed drill green
  with its telemetry signal, 0 ghost/0 lost); the no-raw-publish lint is green; per-aggregate seq is monotonic +
  dedup-safe on replay; the unit + chained-e2e + drill tests pass; the coverage scanner is green; the
  CAS→ISS-P09 floor is named; the work is committed. "Looks done" is not done.
- **COMMIT.** Header: P-<NNN> M4: Silent-data-loss-safe write path. Body lists: contracts 2.2/2.3/4.2/4.6/4.10
  consumed; the emit-iff-committed drill greened (0 ghost/0 lost, the measured signal); the no-raw-publish lint
  green; the CAS floor named (ISS-P09). Branch first if on default; do not push unless asked. End with the
  workspace Co-Authored-By trailer.

---

### ISS-P07 — Pseudonymous-by-default identity columns + per-subject-DEK free-text + the holder registration

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I1, the pseudonymous-identities slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I1", the pseudonymous identity columns +
  per-subject-DEK + PersonalDataHolder registration bullet).
- **DEPENDS-ON.** ISS-P05 (the schema with the lifecycle/GDPR columns), ISS-P06 (the write path the columns flow
  through). The M1 Identity prompt (resolve_pseudonym/erase + the pseudonym grammar 4.8). The M1 Storage prompts
  (KMS hierarchy + per-subject DEK 11.3/11.4). The M1 GDPR prompt (PersonalDataHolder spine 10.1). The index
  places this immediately after ISS-P06 within M4 (it completes the safe-write floor before keys/content).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe by construction);
    ../../external-insights/01-process-and-quality-doctrine.md §1 (name-your-floors — the holder ops are stubbed
    here, full fan-out is ISS-P31); ../../external-insights/04-hard-problems.md §1 (erasure-vs-immutability —
    pseudonymous-by-default identity columns).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/06-reconciliation-compliance.md (the
    pseudonymous identity columns + the per-subject-DEK free-text).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-7 (the one
    free-text/immutable erasure posture — pseudonymous-by-default identity columns), §1 (the pseudonym grammar
    <pseudonym>@<tenant>.noreply frozen).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 4.8 (resolve_pseudonym/erase + the
    grammar), 11.3/11.4 (KMS + per-subject DEK for free-text), 10.1 (PersonalDataHolder), 1.4 (holder
    auto-registration on store open).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I1" (the pseudonymous-identities bullet) + §2
    (upstream dep rows 4.8, 11.3/11.4) + §1 (leak-then-write-loss order).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate:
  - Pseudonymous-by-default identity columns (assignee/reporter/created_by = pseudonymous principal ids per the
    <pseudonym>@<tenant>.noreply grammar, EI-04 §1) wired through the ISS-P06 write path + read path.
  - Per-subject-DEK encryption for free-text title/props/change-deltas (11.4) — the columns hold ciphertext keyed
    by per-subject DEK metadata, not plaintext.
  - Confirm Issues is registered as a PersonalDataHolder (auto-registered by serve when the store opens, 1.4) and
    declare the holder ops as todo-stubbed (the full locate/export/rectify/restrict/erase implementation is
    ISS-P31; the registration + the per-subject-DEK column wiring ship now).
  - FLOOR named: the holder ops are stubbed (full erasure fan-out is ISS-P31); free-text erasure = per-subject
    DEK + pseudonym-map shred is the structural floor (the third-party-mention residual basis is [OPEN — LEGAL],
    R-1). Name each in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 4.8 (consumed — pseudonymous identities). 11.3/11.4 (consumed — KMS + per-subject
  DEK). 10.1 (consumed/owned — the holder registration; ops stubbed). 1.4 (consumed — auto-registration).
  Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The identity columns store pseudonymous principal ids (0 raw principal identifiers in assignee/reporter/
    created_by; the grammar validates) — CI, the 0-raw-id assertion is the green artifact.
  - Free-text title/props/change-deltas are stored under per-subject DEK (0 plaintext free-text at rest; a
    fixture asserts ciphertext + DEK metadata) — CI.
- **TESTS (required).** Unit tests that an assignee/reporter resolves to a pseudonym and that free-text round-trips
  through per-subject-DEK encrypt/decrypt; a fixture asserting 0 plaintext free-text at rest. The provider/consumer
  CDC pair for 4.8 (the pseudonym grammar) + 11.4 (the DEK column wiring). State the cargo-mutants mutation-score
  floor for the DEK-column / pseudonym-resolution module (mandatory-core — it is the erasure seam).
- **DEFINITION OF DONE.** Identity columns are pseudonymous (0 raw ids); free-text is per-subject-DEK encrypted (0
  plaintext at rest); the holder is registered (ops stubbed, ISS-P31 named); the unit + CDC tests pass; the
  coverage scanner is green; the holder-ops + DEK-erasure floors are named; the work is committed. No gate is
  greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: Pseudonymous identity columns + per-subject-DEK + holder registration. Body
  lists: contracts 4.8/11.3/11.4/10.1/1.4 consumed; 0-raw-id + 0-plaintext-free-text greened; the holder-ops
  (ISS-P31) + DEK-erasure (R-1) floors named. Branch first if on default; do not push unless asked. End with the
  workspace Co-Authored-By trailer.

---

### ISS-P08 — Hi/Lo human-key allocation (the <PROJECTKEY>-<seqno> stored canonical id)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I1, the Hi/Lo key-allocation slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I1", the Hi/Lo human-key allocation bullet).
- **DEPENDS-ON.** ISS-P05 (the prefix_counter table), ISS-P06 (the write path the key flows through). The M2 Refs
  prompt that ships ArtifactRef parse/format (5.1). The index places this immediately after the write-path floor
  within M4.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors); ../../external-insights/01-process-and-quality-doctrine.md §3
    (prove-it — a create-storm with 0 duplicate key + monotonic, measured).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md (the
    Hi/Lo human-key allocator — per-prefix, gap-tolerant, monotonic, adaptive block, cell-local).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md REF-3 (the Issues
    <PROJECTKEY>-<seqno> key as the stored canonical id; #1421 is render-time only).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 5.1 (the ArtifactRef
    <PROJECTKEY>-<seqno> grammar — the stored canonical key), 2.2 (the key write co-commits its event).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I1" (the Hi/Lo bullet + the ISS-D4 exit gate)
    + §6 (first-runnable definition).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row ISS-D4 (create-storm human-key).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate:
  - The Hi/Lo human-key allocator per prefix: the frozen <PROJECTKEY>-<seqno> stored canonical id (the
    prefix_counter table holds the Hi block; the allocator hands out Lo seqnos), gap-tolerant, monotonic per
    prefix, adaptive block size, per-prefix isolation, cell-local. The stored key == the canonical ArtifactRef id
    (5.1); #1421 is a render-time display projection, never stored. The allocation slots into the ISS-P06 write
    path (replacing the placeholder key).
  - FLOOR named: none new (the storage floor was named in ISS-P05). State that the render-time #1421 projection is
    display-only; name it in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 5.1 (owned — the <PROJECTKEY>-<seqno> canonical key + the #1421 render projection).
  2.2 (consumed — the key write co-commits). Implement to the frozen shapes; escalate a needed change, do not
  diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - ISS-D4 (create-storm on one hot prefix, N workers — import + incident burst → no duplicate key, monotonic per
    prefix, gaps benign, per-prefix isolation, key == the stored canonical id) — SCHED, 0 dup key + monotonic is
    the green artifact.
- **TESTS (required).** Unit tests for the Hi/Lo allocator (gap-tolerance, per-prefix isolation, adaptive block,
  monotonicity). A chained-mutation e2e test (N concurrent creates on one prefix → assert 0 dup, monotonic). The
  drill scenario for ISS-D4. The provider/consumer CDC pair for 5.1 (the key grammar). State the cargo-mutants
  mutation-score floor for the allocator module (mandatory-core — a duplicate key is a correctness failure).
- **DEFINITION OF DONE.** Keys allocate uniquely + monotonically per prefix with per-prefix isolation; the stored
  key == the canonical ArtifactRef id; ISS-D4 emits a dated green artifact (0 dup key); the unit + e2e + drill
  tests pass; the coverage scanner is green; the render-time #1421 note is written; the work is committed. No gate
  is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: Hi/Lo human-key allocation. Body lists: contract 5.1 owned, 2.2 consumed; ISS-D4
  (0 dup key, monotonic) greened with measured numbers. Branch first if on default; do not push unless asked. End
  with the workspace Co-Authored-By trailer.

---

### ISS-P09 — The server-arbitrated order_key CAS reorder (the silent-clobber floor)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I1, the order_key CAS slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I1", the server-arbitrated order_key CAS bullet).
- **DEPENDS-ON.** ISS-P06 (the write path), ISS-P02 (the myelin-query order_key codec). The index places this
  immediately after ISS-P08 within M4 (it is the next floor on the create→edit→reorder loop).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors — CAS is the floor, CRDT the follow-on);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — 0 silent clobber with a
    converged-order signal); ../../external-insights/04-hard-problems.md §2 (CRDT-after-CAS).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md (the
    server-arbitrated order_key CAS reorder; the loser-re-bases discipline).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-3 (the order_key/
    LexoRank codec frozen).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 13.3 (the order_key/LexoRank
    codec — base-62, midpoint bisection, 2-char jitter, 48-char rebalance, created_at+ULID tiebreak), 2.2 (the
    reorder write co-commits its event).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I1" (the CAS bullet + the ISS-D5 exit gate) +
    §5 (the floors register — CAS ranking row, R-3) + §1 (silent clobber is Tier-1-of-Issues #3).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row ISS-D5 (reorder 0-clobber).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate:
  - The server-arbitrated order_key CAS for drag-reorder: a reorder request carries the issue's last-seen
    order_key; the server bisects a new key (the frozen codec — 2-char jitter, 48-char rebalance trigger) and
    writes under a CAS on the prior key; on a precondition miss the LOSER is rejected and re-bases honestly against
    current server state — no silent clobber, no merge. This is the CAS floor. The 48-char rebalance must never
    reorder the displayed order. The reorder write co-commits its issue.reordered event (2.2).
  - FLOOR named: ranking = order_key + server-arbitrated CAS; the move-CRDT (Yrs list / Fugue) is the named M5
    follow-on (ISS-P32), reusing the byte-identical order_key — promotion swaps the conflict engine, not the data
    model. Name it in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 13.3 (consumed — the order_key codec, now executed). 2.2 (consumed — the reorder
  write co-commits). Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - ISS-D5 (N humans + an agent re-ranking the same region → 0 silent clobber, bounded re-base churn, converges
    with the 2-char jitter, the 48-char rebalance never reorders displayed order) — CI, 0 clobber + converged
    order is the green artifact.
- **TESTS (required).** Unit tests for the order_key CAS (precondition-miss → loser re-bases, no overwrite; the
  48-char rebalance preserves displayed order). A chained-mutation e2e test (create → reorder concurrently from N
  writers → assert converged order, 0 clobber). The drill scenario for ISS-D5. The provider/consumer CDC pair for
  the reorder event (2.2). State the cargo-mutants mutation-score floor for the order_key CAS module
  (mandatory-core — it is the silent-clobber seam).
- **DEFINITION OF DONE.** Reorder is 0-clobber and converges; the loser re-bases honestly; the 48-char rebalance
  never reorders displayed order; ISS-D5 emits a dated green artifact (0 clobber, converged); the unit + e2e +
  drill tests pass; the coverage scanner is green; the CAS→move-CRDT floor is named with ISS-P32; the work is
  committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: order_key CAS reorder (silent-clobber floor). Body lists: contracts 13.3
  executed, 2.2 consumed; ISS-D5 (0 clobber, converged order) greened with measured numbers; the CAS→move-CRDT
  floor named (ISS-P32). Branch first if on default; do not push unless asked. End with the workspace
  Co-Authored-By trailer.

---

### ISS-P10 — The issue body + comments as a myelin-content block subtree (render(parse(md)) === md)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I1, the content-body slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I1", the issue body as a myelin-content block subtree
  bullet). Completes the "first runnable" create→edit→reorder loop.
- **DEPENDS-ON.** ISS-P06 (the write path the body version-CAS flows through), ISS-P09 (the reorder loop). The M2
  prompt that froze myelin-content + the WASM render target (13.1). The index places this immediately after
  ISS-P09 within M4 (it completes M4-I1's first-runnable deliverable).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (top-of-the-line UX — one content model);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — round-trip 100% over a corpus).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/01-tech-and-data-model.md (the content
    body as a myelin-content block subtree + the version token single-author CAS).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-2 (myelin-content +
    the WASM render target render(parse(md)) === md).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 13.1 (the myelin-content block
    subset + the WASM render path; the three inline ref nodes), 2.2 (the body write co-commits its event).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I1" (the content-body bullet + the ISS-D10
    exit gate) + §6 (first-runnable definition).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row ISS-D10 (render(parse(md)) === md).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate:
  - The issue body + comments as a myelin-content block subtree (the consumed subset; single-author CAS on the
    version token; the WASM render path). render(parse(md)) === md must hold for bodies + comments (read + edit
    use the IDENTICAL WASM parser, not two code paths). The body/comment write co-commits its event (2.2).
  - FLOOR named: none new (the body is a projection of the frozen content subset). State that the move-CRDT body
    collaboration is out of v1 scope (single-author version-CAS is the floor); name it in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 13.1 (consumed — the content block subset + WASM render). 2.2 (consumed — the body
  write co-commits). Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - ISS-D10 (render(parse(md)) === md 100% over a body+comment corpus; read+edit use the identical WASM parser) —
    CI, round-trip = 100% is the green artifact.
- **TESTS (required).** Unit tests that body/comment edits parse + render through the single WASM path and that the
  version-token single-author CAS rejects a stale write. A round-trip corpus test (render(parse(md)) === md). The
  provider/consumer CDC pair for 13.1 (the content subset). State the cargo-mutants mutation-score floor for the
  render/parse round-trip module if mandatory-core (content fidelity is correctness-bearing — treat it as
  mandatory-core).
- **DEFINITION OF DONE.** Bodies + comments round-trip 100% through the single WASM parser; the version-token CAS
  rejects stale writes; ISS-D10 emits a dated green artifact (100% round-trip); the unit + corpus + CDC tests
  pass; the coverage scanner is green; the work is committed. The first-runnable bar (§6) is met: a tenant can
  create → key → edit → link → reorder, every write co-committing its event. No gate is greened by weakening a
  threshold.
- **COMMIT.** Header: P-<NNN> M4: Issue content body + comments (render round-trip). Body lists: contract 13.1
  executed, 2.2 consumed; ISS-D10 (100% round-trip) greened; the first-runnable bar met. Branch first if on
  default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P11 — Governance schemes + the scheme-precedence algebra + the flexible-field model (config, never a migration)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I2, the schemes + flexible-field slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I2", the five scheme kinds + the precedence algebra +
  the flexible-field model bullets).
- **DEPENDS-ON.** ISS-P05 (the scheme/scheme_assignment tables + the JSONB tail) + ISS-P06 (the write path that
  loads the resolved scheme). The M0 substrate prompts (forward-only-migration lint 1.6). The index places this
  after the M4-I1 spine within M4.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1 (Issues serves engineers AND PMs — corporate workflows: roadmaps/sprints/hierarchies/
    custom fields/SLAs); ../../external-insights/01-process-and-quality-doctrine.md §7 (config not a bespoke
    object graph per scheme; no Jira-Groovy footgun), §5 (the ratchet).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md (the
    scheme-precedence algebra — most-specific-wins, cached, off the hot path); 01-tech-and-data-model.md (the
    JSONB property-bag tail + the GIN index default for flexible fields).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-C (the
    flexible-field index posture).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 1.5 (forward-only migrations +
    the hot-table flags), 1.9/1.10 (ResilientClient/FailStatic for Issues→Id), 1.6 (the forward-only-migration
    lint).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I2" (the schemes + no-config=Linear-simple
    exit gate slice) + §5 (the floors register — issue-hierarchy=tree row + GIN-default row).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate:
  - The five scheme kinds (workflow/field/permission/sla/type) as interpreted JSONB config rows, assigned per
    (type × project × team); the deterministic, cached scheme-precedence algebra (most-specific-wins) computed
    OFF the hot path — the write loads the already-resolved compiled scheme, never resolves precedence inline.
    Assigning a new scheme is a CONFIG write, never a row migration — prove this (a scheme reassignment touches no
    issue rows).
  - The flexible-field model: the JSONB property-bag tail (zero-DDL custom fields) + the GIN index default; the
    forward-only-migration lint on the hot issue/issue_relation/issue_change_log tables.
  - FLOOR named: issue hierarchy = tree parent (constrained-DAG portfolios are the opt-in follow-on, M5+); the
    projection-feeder generated-index promotion is deferred to ISS-P15 (cold facets ride the GIN index until
    measured). Name both in the crate doc. (The FSM interpreter + the QueryAst guards are ISS-P12.)
- **CONTRACTS TO IMPLEMENT.** 1.5 (consumed — forward-only hot-table migrations). 1.9/1.10 (consumed — Issues→Id
  resilient/fail-static). Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - no-config = Linear-simple PROVEN: an org with zero scheme assignments resolves to org_default for every kind
    with no migration (0 issue rows touched by a scheme reassignment) — CI, the 0-rows-touched assertion is the
    green artifact.
  - The forward-only-migration lint holds on the hot tables under a flexible-field add (0 destructive migrations)
    — CI, lint green.
- **TESTS (required).** Unit tests for the scheme-precedence algebra (most-specific-wins determinism + caching) and
  the zero-DDL flexible-field add (a custom field is a JSONB write + a GIN-indexable facet, not a DDL). A
  chained-mutation e2e test (assign org_default → reassign a project scheme → assert 0 issue rows touched). The
  CDC stub for the scheme config shape. State the cargo-mutants mutation-score floor for the precedence-resolution
  module if mandatory-core (precedence determinism is governance-correctness-bearing — treat it as mandatory-core).
- **DEFINITION OF DONE.** Schemes resolve deterministically off the hot path; a scheme reassignment migrates no
  data (0 rows touched, proven); the flexible-field model is zero-DDL + GIN-default; the forward-only-migration
  lint holds; the unit + e2e tests pass; the coverage scanner is green; the tree-hierarchy + GIN-default floors
  are named; the work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: Governance schemes + precedence algebra + flexible-field model. Body lists:
  1.5/1.9/1.10 consumed; no-config-Linear-simple proven (0 rows touched), forward-only-migration lint green; the
  tree-hierarchy + GIN-default floors named. Branch first if on default; do not push unless asked. End with the
  workspace Co-Authored-By trailer.

---

### ISS-P12 — The data-driven workflow FSM interpreter + the QueryAst guards (the fixed state-category invariant)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I2, the workflow FSM slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I2", the data-driven workflow FSM interpreter + the
  QueryAst guards + required-fields + post-actions bullet).
- **DEPENDS-ON.** ISS-P11 (the workflow scheme config the interpreter runs), ISS-P06 (the write path the
  transition mutates). ISS-P02 (the QueryAst — the guard predicate language). The M0 substrate prompts
  (flow-determinism lint 1.6). The index places this immediately after ISS-P11 within M4.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1 (corporate workflows — governed transitions);
    ../../external-insights/01-process-and-quality-doctrine.md §7 (keep the architecture coherent — bounded guard
    language, no UDFs/loops/recursion), §5 (the ratchet — the flow-determinism lint).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md (the
    data-driven workflow FSM interpreter; the fixed state-category set unstarted/started/completed/cancelled; the
    QueryAst guards; required-fields-on-transition; the post-actions).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-3 (the QueryAst as
    the one bounded guard predicate language — no UDFs/loops/recursion).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 13.3 (the QueryAst guard predicate
    core), 3.4 (EventMatcher = QueryAst — the same bounded interpreter the arm-trigger post-action uses), 1.6 (the
    flow-determinism lint), 4.2 (check + CaveatContext — the transition ABAC).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I2" (the FSM exit gate — the ISS-D12 guard
    half) + §5 (the floors register).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row ISS-D12 (the guard slice — "can't close while blocked_by an open issue").
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate:
  - The data-driven workflow FSM interpreter with the FIXED state-category set
    (unstarted/started/completed/cancelled) as the one mandatory governance invariant over unlimited named
    states; guards are the frozen QueryAst (bounded, no UDFs/loops/recursion); required-fields-on-transition;
    post-actions (assign/set-field/link/arm-trigger). The transition runs through Id.check (+ CaveatContext) for
    the transition ABAC (4.2).
  - FLOOR named: none new (the FSM is config-interpreted). State that the CI-red guard half of ISS-D12 lands in
    ISS-P27 when the X-1 seam closes; name it in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 13.3 (consumed — the QueryAst guard core, executed by the FSM interpreter). 3.4
  (consumed — the EventMatcher=QueryAst alignment for arm-trigger post-actions). 1.6 (consumed — flow-determinism
  lint). 4.2 (consumed — the transition ABAC). Implement to the frozen shapes; escalate a needed change, do not
  diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The workflow-guard correctness slice of ISS-D12 ("can't close while blocked_by an open issue" → transition
    blocked with a pre-assembled reason) — CI, transition-blocked + the reason string is the green artifact. (The
    CI-red half lands in ISS-P27 when the X-1 seam closes.)
  - The flow-determinism lint holds on any workflow body that schedules a durable activity (a post-action that
    arms a trigger) — CI, lint green.
- **TESTS (required).** Unit tests for the FSM interpreter (the fixed-category invariant; a guard rejects a
  transition; required-fields enforced; post-actions fire). A chained-mutation e2e test (transition through states
  → assert the category invariant + a blocked_by guard rejects → reason). The drill scenario for the ISS-D12 guard
  half. The CDC stub for the guard config shape. State the cargo-mutants mutation-score floor for the
  guard-evaluation module (the guard is governance-correctness-bearing — treat it as mandatory-core).
- **DEFINITION OF DONE.** The FSM interpreter enforces the fixed category set + the QueryAst guards + required-
  fields + post-actions; the ISS-D12 guard half is green (transition blocked + reason); the flow-determinism lint
  holds; the unit + e2e + drill tests pass; the coverage scanner is green; the CI-red-guard-half follow-on
  (ISS-P27) is named; the work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: Workflow FSM interpreter + QueryAst guards. Body lists: 13.3/3.4/4.2 consumed;
  the ISS-D12 guard half greened (transition blocked + reason), flow-determinism lint green; the CI-red-guard-half
  follow-on named (ISS-P27). Branch first if on default; do not push unless asked. End with the workspace
  Co-Authored-By trailer.

---

### ISS-P13 — The AST→OLTP-store compiler: the SetExpr push-down lowered first (leak-free, no N+1, no post-filter)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I3, the SetExpr-lowering slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I3", the AST→OLTP-store compiler that lowers the
  list_objects SetExpr first). This is the zero-leak milestone — Issues' highest-stakes property.
- **DEPENDS-ON.** ISS-P05 + ISS-P06 + ISS-P11 (the spine + write path + schemes/fields). ISS-P02 (myelin-query —
  the AST it compiles). The M1 Identity prompt that ships list_objects with the SetExpr push-down + the per-tenant
  authz reverse index (4.3) and the zookie semantics (4.10). The index places this after ISS-P12 within M4 — it
  is the make-or-break leak milestone.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale from day 1; one permission model);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — 0 leak with a zero-escape counter),
    §2 (the leak is the catastrophe — it comes before breadth).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md (the
    AST→OLTP-store compiler — lower the SetExpr FIRST into a SQL predicate/JOIN over the authz reverse index keyed
    on issue.id; the Ids/NotIds/InRelation{relation,via_column}/TupleSet/Union/Intersect/Difference/All/None
    lowering; the zookie staleness bound); 05-hard-problems.md (the leak-free-at-scale analysis).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-E (the SetExpr
    push-down — lowered to a SQL predicate/JOIN over the per-tenant authz reverse index; no N+1, no post-filter).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 4.3 (list_objects → Ids|Filter
    with the SetExpr push-down — the single most load-bearing inter-system contract; lower it first), 4.10
    (zookie — the new-enemy guard; a security-sensitive scan reads at-or-after the zookie revision), 13.3 (the
    QueryAst/ViewSpec the compiler lowers).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I3" (the ISS-D3 exit gate) + §1 (the
    cross-tenant/confidential leak is what kills us first inside Issues).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row ISS-D3 (cross-tenant + confidential IDOR 0 leak).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate (the Issues-owned query
  planner — the lowering half):
  - The AST→OLTP-store compiler that LOWERS the frozen list_objects SetExpr FIRST into a SQL predicate / JOIN
    against the per-tenant authz reverse index keyed on issue.id (Ids / NotIds / InRelation{relation, via_column}
    / TupleSet / Union / Intersect / Difference / All / None) — ONE query, no N+1, no post-filter. The zookie
    bounds staleness: a security-sensitive scan reads at-or-after the zookie's revision (the new-enemy guard). A
    confidential issue is simply ABSENT from the result, never a "N hidden" count leak.
  - FLOOR named: none new (the cost-bounder + escalation are ISS-P14; the feeder is ISS-P15). State that the
    SetExpr lowering is the leak seam; name the cost-bounding follow-on (ISS-P14) in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 4.3 (consumed — the SetExpr push-down lowered first; the planner is the headline
  consumer of this contract). 4.10 (consumed — the zookie staleness bound). 13.3 (consumed — the QueryAst lowered).
  Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - ISS-D3 (cross-tenant + confidential-issue IDOR → not in any SetExpr JOIN result for an unauthorized viewer,
    incl. under zookie staleness; 0 leak) — CI, the zero-escape counter = 0 is the green artifact. (This is the F1
    leak-free family; it re-runs across surfaces in ISS-P16/P17 and inside the surge family in ISS-P32.)
- **TESTS (required).** Unit tests for the SetExpr lowering (each variant → the correct predicate/JOIN; the
  confidential set-difference excludes; the zookie watermark is honoured; no N+1). A chained-mutation e2e test
  (grant then revoke confidential-grant → the revoke reflects in the next zookie-bounded read; 0 leak). The drill
  scenario for ISS-D3. The provider/consumer CDC pair for 4.3 (the SetExpr push-down — Issues is the consumer
  side). State the cargo-mutants mutation-score floor for the SetExpr-lowering module (mandatory-core — it is THE
  leak seam).
- **DEFINITION OF DONE.** The planner lowers the SetExpr first into one leak-free query (no N+1, no post-filter);
  the zookie bounds staleness; a confidential issue is absent (no count leak); ISS-D3 emits a dated green artifact
  (0 leak, zero-escape counter); the unit + e2e + drill tests pass; the coverage scanner is green; the
  cost-bounding follow-on (ISS-P14) is named; the work is committed. No gate is greened by weakening a threshold
  or inverting the zero-escape assertion.
- **COMMIT.** Header: P-<NNN> M4: Query planner — SetExpr push-down lowered first (leak-free). Body lists:
  contract 4.3 (the SetExpr lowering) + 4.10/13.3 consumed; ISS-D3 (0 leak, zero-escape counter) greened; the
  cost-bounding follow-on named (ISS-P14). Branch first if on default; do not push unless asked. End with the
  workspace Co-Authored-By trailer.

---

### ISS-P14 — Cost-bounding + the three-tier escalation (the <1s flexible-field latency floor)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I3, the cost-bounding slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I3", the cost-bounding + the three-tier escalation
  bullet). The latency milestone — Issues' second highest-stakes property.
- **DEPENDS-ON.** ISS-P13 (the SetExpr lowering every tier conjoins). The M2 Search prompt (query conjoins the
  Filter 6.1; the search-requires-acl-filter lint). The index places this immediately after ISS-P13 within M4.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale from day 1);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the <1s budget is a quantified
    gate; never a full scan).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md (the
    cost-bounding + the three-tier escalation; the projection feeder reference — Tier 2); 05-hard-problems.md (the
    flexible-field-latency analysis).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 6.1 (Search query conjoins the
    same Filter — the Tier-3 escalation valve; the search-requires-acl-filter lint), 4.3 (the lowered SetExpr
    every tier reuses), 13.3 (the QueryAst).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I3" (the ISS-D2 exit gate; the GIN-default
    floor) + §5 (the floors register — GIN-default row).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row ISS-D2 (50+ fields × 1M+ issues board query < 1s, no full scan).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate (the Issues-owned query
  planner — the cost-bounding half):
  - Cost-bounding + the three-tier escalation: Tier 1 typed-core index ranges (issue_board / issue_roadmap /
    issue_assignee); Tier 2 measured-hot generated indexes (the projection feeder from ISS-P15; the GIN probe as
    2b); Tier 3 escalate to Search CONJOINING THE SAME Filter (the search-requires-acl-filter lint). Every query
    is paginated + statement-timeout'd; a query that would scan too much is pushed to Search or returns a Refine
    hint — never an unbounded JSONB scan.
  - FLOOR named: flexible-field index = GIN default; the projection-feeder generated-index promotion is the
    measured follow-on (ISS-P15, OQ-C > 5% of view executions); distributed-SQL for a hot tenant is the measured
    follow-on (M5, ISS-P32). Name each in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 6.1 (consumed — the Tier-3 Search escalation with the same Filter). 4.3 (consumed —
  the lowered SetExpr each tier reuses). 13.3 (consumed — the QueryAst). Implement to the frozen shapes; escalate a
  needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - ISS-D2 (50+ custom fields × 1M+ issues board query under the <1s keyboard budget with the SetExpr JOIN; a cold
    ad-hoc query escalates to Search with the same Filter; the planner never emits a full JSONB scan) — SCHED,
    query p99 < 1s + no-full-scan is the green artifact. (Also the OQ-C calibration drill.)
  - The search-requires-acl-filter lint is GREEN on the Tier-3 escalation (0 Search calls without the conjoined
    Filter) — CI.
- **TESTS (required).** Unit tests for the cost-bounder (a too-large scan returns Refine / escalates, never an
  unbounded scan; each tier picks the right index). A chained-mutation e2e test (a 50+-field board query stays
  under budget; a cold ad-hoc query escalates to Search with the same Filter). The drill scenario for ISS-D2. The
  provider/consumer CDC pair for 6.1 (the conjoined Filter). State the cargo-mutants mutation-score floor for the
  cost-bounder module if mandatory-core (escalating-vs-scanning is latency-correctness-bearing — treat it as
  mandatory-core).
- **DEFINITION OF DONE.** Cost-bounding + the three-tier escalation hold; ISS-D2 emits a dated green artifact (p99
  < 1s, no full scan); the search-requires-acl-filter lint is green; the unit + e2e + drill tests pass; the
  coverage scanner is green; the GIN-default + distributed-SQL floors are named; the work is committed. No gate is
  greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: Cost-bounding + three-tier escalation (latency floor). Body lists: contract 6.1
  consumed; ISS-D2 (p99 < 1s, no full scan) greened with measured numbers; the search-requires-acl-filter lint
  green; the GIN-default + distributed-SQL floors named. Branch first if on default; do not push unless asked. End
  with the workspace Co-Authored-By trailer.

---

### ISS-P15 — The projection-feeder consumer (the measured generated-index promotion)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I3, the projection-feeder slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I3", the projection feeder consumer bullet).
- **DEPENDS-ON.** ISS-P14 (the cost-bounder Tier-2 that reads the generated indexes), ISS-P06 (the issue.updated
  deltas the feeder watches). The M0 outbox/consumer prompts (EventHandler 2.4). The index places this immediately
  after ISS-P14 within M4.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors — promote on MEASURED evidence);
    ../../external-insights/01-process-and-quality-doctrine.md §7 (promote a floor only on a measured trigger,
    never premature).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md (the
    projection feeder — watches issue.updated deltas + a per-(tenant,type,field_id) frequency counter; provisions
    a generated/expression index via a forward-only online migration when a facet crosses the measured threshold).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-C (the measured
    projection-feeder promotion threshold > 5% of view executions).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 2.4 (EventHandler — the feeder is
    a consumer), 1.5 (forward-only online migration for the generated index).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I3" (the feeder + the OQ-C floor) + §5 (the
    floors register — GIN-default → projection-feeder generated-index row).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row ISS-D2 (the OQ-C calibration — the feeder promotes a hot facet within the latency budget).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate (a bus consumer):
  - The projection feeder consumer (watches issue.updated deltas + a per-(tenant,type,field_id) frequency counter;
    provisions a generated/expression index via a forward-only online migration when a facet crosses the measured
    threshold — promotion is MEASURED, never predicted; OQ-C calibration). The promoted index is what ISS-P14's
    Tier 2 reads.
  - FLOOR named: the projection-feeder promotion threshold is the OQ-C default-to-beat (> 5% of a collection's
    view executions), calibrated by ISS-D2. Name it in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 2.4 (consumed — EventHandler; the feeder is a consumer). 1.5 (consumed — the
  forward-only online migration that provisions the index). Implement to the frozen shapes; escalate a needed
  change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The feeder promotes a facet ONLY when it crosses the measured threshold (> 5% of view executions; a
    below-threshold facet is NOT promoted; the promotion is a forward-only online migration with 0 downtime) — CI,
    the threshold-gated promotion + the online-migration signal is the green artifact. (Calibrated under ISS-D2.)
- **TESTS (required).** Unit tests for the frequency counter + the threshold gate (a below-threshold facet stays
  on the GIN index; an above-threshold facet provisions a generated index). A chained-mutation e2e test (drive a
  facet past the threshold → assert the online migration provisions the index → the next query uses Tier 2). The
  provider/consumer CDC pair for the issue.updated feeder consumer (2.4). State the cargo-mutants mutation-score
  floor for the threshold-gate module if mandatory-core; state yes/no.
- **DEFINITION OF DONE.** The feeder promotes on the measured threshold only (never predicted), via a 0-downtime
  forward-only online migration; the promoted index is read by Tier 2; the unit + e2e + CDC tests pass; the
  coverage scanner is green; the OQ-C floor is named; the work is committed. No gate is greened by weakening the
  threshold.
- **COMMIT.** Header: P-<NNN> M4: Projection-feeder consumer (measured generated-index promotion). Body lists:
  2.4/1.5 consumed; the threshold-gated promotion greened (> 5% measured, online migration); the OQ-C floor named.
  Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P16 — The co-equal ViewSpec views + the design-system pass (board/roadmap/backlog/table/calendar/cycle)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I3, the views slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I3", the co-equal ViewSpec projections bullet + the
  pre-frontend design pass).
- **DEPENDS-ON.** ISS-P13 + ISS-P14 (the planner — every view conjoins the SetExpr Filter through it). ISS-P02
  (the ViewSpec Issues projects). The index places this after the planner within M4 (it completes the
  first-useful view surface).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (top-of-the-line UX — co-equal views over one model; no frontend code without a reviewed
    sketch); ../../external-insights/01-process-and-quality-doctrine.md §7 (each new view is a projection of the
    one table, not a new object graph); the design folder
    (../04-subsystem-architectures/issue-tracker/design/information-architecture.md + user-flows.md +
    wireframes.md — the board/roadmap/backlog/table/calendar/cycle screens incl. empty/loading/error/permission
    states).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md (the
    views as co-equal ViewSpec projections; the board↔roadmap structural co-equality, type_rank denormalised);
    04-views-cli-and-api.md (the view surfaces + the CLI parity).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 13.3 (the ViewSpec), 4.3 (every
    view conjoins the lowered SetExpr Filter).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I3" (the ISS-D1 exit gate) + §6 (first-useful
    — the co-equal board+roadmap+backlog).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row ISS-D1 (board↔roadmap same-row, 0 drift, asserted by row id).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate (+ the design record):
  - The views as co-equal ViewSpec projections over the one issue table (board / roadmap / backlog / list / table
    / calendar / cycle), each ALWAYS conjoining the SetExpr Filter through the ISS-P13/P14 planner (a confidential
    issue is simply absent — no "N hidden" leak). The board↔roadmap co-equality is STRUCTURAL (same rows;
    type_rank denormalised) — an edit on one reflects the same row on the other.
  - The design-system pass (pre-frontend, per VISION §3 — no frontend code without a reviewed sketch): a
    visual/token-level pass over the board/roadmap/backlog/table/calendar/cycle screens in the design folder,
    INCLUDING the empty/loading/error/permission/tombstone states. Record the sign-off in the design folder.
  - FLOOR named: none new (the views are projections; the planner floors were named in ISS-P14). State that the
    cross-cell portfolio rollup view is the M5 follow-on (ISS-P32, the CrossCellPointer bridge); name it.
- **CONTRACTS TO IMPLEMENT.** 13.3 (consumed — the ViewSpec). 4.3 (consumed — every view conjoins the lowered
  Filter). Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - ISS-D1 (edit an issue's date/scope on the board → roadmap reflects the SAME ROW, 0 drift, asserted by row id;
    and vice versa) — CI, the same-row-id assertion is the green artifact.
  - The design pass is REVIEWED-AND-SIGNED-OFF in the design folder (incl. the empty/loading/error/permission/
    tombstone states) — sign-off recorded, dated, the green artifact for the pre-frontend gate.
- **TESTS (required).** Unit tests that each ViewSpec projection conjoins the Filter and that board↔roadmap share
  the row (type_rank denormalisation is consistent). A chained-mutation e2e test (edit on board → read on roadmap
  → assert same row id). The drill scenario for ISS-D1. No new contract CDC beyond 13.3 (the views are
  projections). State the cargo-mutants mutation-score floor for the ViewSpec-projection module if mandatory-core;
  state yes/no.
- **DEFINITION OF DONE.** The co-equal views project over one table with the Filter conjoined; board↔roadmap show
  the same row (ISS-D1 green); the design pass is signed off (all states); the unit + e2e + drill tests pass; the
  coverage scanner is green; the cross-cell rollup follow-on is named; the work is committed. No gate is greened by
  weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: Co-equal ViewSpec views + design pass. Body lists: 13.3/4.3 consumed; ISS-D1
  (same-row, 0 drift) greened; the design pass signed off (all states); the cross-cell rollup follow-on named
  (ISS-P32). Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P17 — Refs wiring (resolve/project/#sub/edges/traverse/TE-7 mirror) + the issue.* Search projection emitter

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I3, the Refs/Search wiring slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I3", the Refs resolve/project + the #sub mint + the
  issue.* Search projection bullets).
- **DEPENDS-ON.** ISS-P13 (the planner — project() reads through the leak-free path), ISS-P16 (the views the
  context-pane unfurls into). ISS-P02 (the #sub grammar Issues mints), ISS-P04 (the declared IndexSpec, now the
  live emitter). The M2 Refs prompts (ArtifactRef parse/format 5.1; resolve + the tombstone ladder 5.2/5.7;
  project REQUIRED 5.6; traverse 5.3; the TE-7 mirror 5.5; refs.edge via content nodes 5.4). The M2 Search prompt
  (declare_indexable 6.3; reindex 6.4). The index places this immediately after ISS-P16 within M4 (it completes
  M4-I3).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (one ref/search model);
    ../../external-insights/01-process-and-quality-doctrine.md §7 (the compounding payoff — projections, not new
    object graphs).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md (the
    Refs resolve/project unfurl; the #sub mints comment-/b/field-/row-; the issue.* Search projection).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-4 (the unified #sub
    grammar + the 4-step tombstone ladder), OQ-I (cell-local resolution).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 5.1 (ArtifactRef), 5.2 (resolve →
    Projection|Tombstone, per-viewer), 5.6 (project REQUIRED — {title,state,icon,render_hint,sub_anchor?};
    pre-permission-checked; the only cross-DB read of an Issues artifact — a confidential issue returns a
    tombstone carrying the root, never the title), 5.7 (the #sub grammar — mint comment-/b/field-/row-), 5.4
    (refs.edge via the inline mention/artifact_ref content nodes), 5.3 (traverse), 5.5 (the TE-7 mirror), 6.3
    (declare_indexable — the issue.* projection emitter), 6.4 (reindex).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I3" (the Refs/Search wiring) + §2 (the
    Refs/Search upstream rows).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row ISS-D3 (the confidential-unfurl tombstone slice re-asserted at the project() boundary).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate:
  - Wire Refs resolve + project(ref, viewer) (5.6): the context-pane unfurl, pre-permission-checked; a
    confidential issue returns a tombstone carrying the root, never the title. Mint the unified #sub ids Issues
    owns (5.7: comment-/b/field-/row-) — stable opaque ids; Refs stores the full sub-URN + the stripped root.
    Emit refs.edge.created from the inline mention/artifact_ref content nodes (5.4). Wire traverse (5.3) for the
    bounded cycle-safe walk (depth 16) and the issue_relation TE-7 mirror (5.5).
  - Emit the issue.* Search projection (declare_indexable from ISS-P04, now the live emitter; 6.3) so Tier-3
    escalation has an index; reindex(scope) (6.4) as the only rebuild path.
  - FLOOR named: none new. State that the cross-cell projection bridge is the M5 follow-on (ISS-P32); name it.
- **CONTRACTS TO IMPLEMENT.** 5.6 (owned — project REQUIRED on Issues). 5.1/5.2/5.7/5.4/5.3/5.5 (consumed/owned —
  the ArtifactRef, the resolve/tombstone, the #sub mints, the edges, the traverse, the TE-7 mirror). 6.3/6.4
  (consumed — the issue.* projection emitter + reindex). Implement to the frozen shapes; escalate a needed change,
  do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The Refs project() for a confidential issue returns a tombstone carrying the root, never the title (the 0-leak
    unfurl property, a slice of ISS-D3 re-asserted at the unfurl boundary) — CI, the tombstone-not-title assertion
    is the green artifact.
  - The issue.* Search projection emits + reindex(scope) rebuilds it (the projection is present + ACL-filtered) —
    CI.
- **TESTS (required).** Unit tests for project() (a confidential issue → tombstone-not-title; a permitted issue →
  the projection) and the #sub mints (stable opaque ids; the TE-7 mirror reflects both directions). A
  chained-mutation e2e test (mint a #sub → resolve per-viewer → confidential viewer gets a tombstone). The
  provider/consumer CDC pair for 5.6 (project — Issues owns the provider side) + 5.4 (the issue edges). State the
  cargo-mutants mutation-score floor for the project()/tombstone module (mandatory-core — the tombstone-vs-title
  decision is leak-bearing).
- **DEFINITION OF DONE.** project() returns a tombstone-not-title for a confidential issue; the #sub mints + edges
  + traverse + TE-7 mirror are wired; the issue.* Search projection emits + reindex works; the unit + e2e + drill
  tests pass; the coverage scanner is green; the cross-cell follow-on is named; the work is committed. No gate is
  greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: Refs wiring + issue.* Search projection. Body lists: contract 5.6 owned
  (project), 5.1/5.2/5.7/5.4/5.3/5.5 + 6.3/6.4 wired; the confidential-unfurl tombstone proven; the Search
  projection emits + reindex; the cross-cell follow-on named (ISS-P32). Branch first if on default; do not push
  unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P18 — The event-driven incremental rollup consumer (off the bus, never in the write path)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I4, the rollup slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I4", the incremental rollup consumer bullet).
- **DEPENDS-ON.** ISS-P17 (the Refs traverse + the TE-7 mirror — the rollup walks parent edges), ISS-P06 (the
  issue.* events the rollup consumes). The M2 Bus prompt (reindex-from-source / replay 2.6). The index places
  this after ISS-P17 within M4.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale); ../../external-insights/04-hard-problems.md §5 (reindex-from-source — the
    derived store rebuilds, never restored; steady-state and recovery share one code path), §2.4 (rollups
    computed off the bus, never in the write path);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the reindex-parity drift-free
    assertion).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md (the
    event-driven debounced incremental rollup consumer — depth-16 ceiling, visited-set, cycle-safe; the
    debounce-coalesce; the incremental re-sum; the input_hash no-op suppression for loop storms; the rollup row
    as a derived rebuildable aggregate).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md (the
    reindex-from-source as the only recovery path; OQ-K the debounce-window floor).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 2.6 (reindex-from-source — replay
    emits *.snapshot through the live consumer; the only recovery path for derived stores), 5.3 (traverse — the
    bounded cycle-safe ancestor walk depth 16), 2.4 (EventHandler — the rollup is a consumer).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I4" (the rollup + the ISS-D8 exit gate) + §5
    (the floors register — read-time-rollup row, R-4).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row ISS-D8 ((a) rollup freshness under a 10k-issue import with bounded ancestor recomputes; (b) replay
    rebuilds rollup + the Refs edge projection drift-free vs live).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate (a bus consumer):
  - The event-driven, debounced, incremental rollup consumer (off the bus, NEVER in the write path): walk parent
    edges (depth ceiling 16, visited-set, cycle-safe — a dependency cycle is a roadmap diagnostic, never a hang);
    debounce-coalesce a burst into one ancestor recompute; incremental re-sum; input_hash no-op suppression
    (stops loop storms, AG-6). The rollup row is derived (rebuildable by replay; edge truth stays in
    issue_relation).
  - FLOOR named: rollup = read-time for small subtrees, materialise-on-measured-large (KN-3, the M5 follow-on,
    ISS-P32); the debounce-window + affected-ancestor fan-out policy is per-tenant-tunable, calibrated by the
    ISS-D8a window (OQ-K floor). Name each in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 2.6 (consumed — reindex-from-source / replay; the only recovery path). 5.3
  (consumed — the bounded ancestor walk). 2.4 (consumed — the rollup consumer). Implement to the frozen shapes;
  escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - ISS-D8(a) (rollup freshness under a 10k-issue import → a BOUNDED number of ancestor recomputes via debounce;
    initiative progress correct within the window) — SCHED, the debounce-bound is the green artifact.
  - ISS-D8(b) (reindex-from-source: replay rebuilds the rollup aggregate + the Refs edge projection DRIFT-FREE vs
    live — proving steady-state and recovery share one code path) — SCHED, the reindex-parity (0 drift) is the
    green artifact.
- **TESTS (required).** Unit tests for the rollup walk (depth-16 ceiling, cycle-safety via visited-set, the
  input_hash no-op suppression, the incremental re-sum). A chained-mutation e2e test (import a subtree → assert
  bounded recomputes → replay → assert drift-free). The drill scenario for ISS-D8. The provider/consumer CDC pair
  for 2.6 (replay). State the cargo-mutants mutation-score floor for the rollup-consumer module (mandatory-core —
  the loop-storm suppression is correctness-bearing).
- **DEFINITION OF DONE.** The rollup recomputes incrementally + debounced off the bus, cycle-safe, loop-storm-
  suppressed; ISS-D8(a)+(b) emit dated green artifacts (bounded recomputes + 0-drift reindex); the unit + e2e +
  drill tests pass; the coverage scanner is green; the read-time-rollup + debounce-window floors are named; the
  work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: Incremental rollup consumer. Body lists: contracts 2.6/5.3/2.4 consumed;
  ISS-D8(a) (bounded recomputes) + ISS-D8(b) (0-drift reindex-parity) greened with measured numbers; the
  read-time-rollup + debounce-window floors named (ISS-P32 / OQ-K). Branch first if on default; do not push unless
  asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P19 — The time axis (cycles/sprints + milestones) + attachments in BlobStore

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I4, the time-axis + attachments slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I4", the cycles/sprints + milestones + attachments
  bullets).
- **DEPENDS-ON.** ISS-P05 (the cycle/cycle_membership/milestone tables), ISS-P18 (the rollup the burndown/CFD
  feeds reference). The M1 Storage prompt (BlobStore content-addressed 11.2). The index places this after ISS-P18
  within M4.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale); ../../external-insights/01-process-and-quality-doctrine.md §7 (the time axis
    is membership edges over the one model, not a new containment graph).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/01-tech-and-data-model.md (cycles/
    sprints + milestones as separate objects with membership edges; attachments in BlobStore — the row holds the
    pointer).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 11.2 (BlobStore content-addressed,
    residency-pinned — the row holds the pointer + per-subject-DEK metadata, not the bytes), 5.5 (the membership
    edges as TE-7 mirrors).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I4" (the time-axis + attachments bullets).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate:
  - The time axis: cycles/sprints + milestones as separate objects (membership edges, not containment);
    burndown/CFD fed to OLAP off the bus (the OLAP wiring lands in ISS-P20); carry-over provenance.
  - Attachments in BlobStore (content-addressed, residency-pinned; the row holds the pointer + per-subject-DEK
    metadata, not the bytes).
  - FLOOR named: none new. State that the burndown/CFD analytics land in the OLAP prompt (ISS-P20); name it.
- **CONTRACTS TO IMPLEMENT.** 11.2 (consumed — BlobStore for attachments). 5.5 (consumed/owned — the membership
  edges as TE-7 mirrors). Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - Cycles/milestones are membership-edged (a cycle add/remove is an edge write, not a containment migration; the
    edge mirrors via TE-7) — CI, the edge-not-containment assertion is the green artifact.
  - Attachments hold pointers, not bytes (the issue row stores a content-addressed BlobStore pointer +
    per-subject-DEK metadata; 0 bytes in the OLTP row) — CI, the 0-bytes-in-row assertion is the green artifact.
- **TESTS (required).** Unit tests for cycle/milestone membership (add/remove is an edge write; carry-over
  provenance is preserved) and the attachment pointer (the row holds a pointer + DEK metadata, never bytes). A
  chained-mutation e2e test (add an issue to a cycle → roll it over → assert carry-over provenance). The
  provider/consumer CDC pair for 11.2 (the BlobStore pointer). State the cargo-mutants mutation-score floor if the
  membership/attachment module is mandatory-core; state yes/no.
- **DEFINITION OF DONE.** Cycles/milestones are membership-edged (not containment); attachments hold pointers not
  bytes; the edge-not-containment + 0-bytes-in-row assertions are green; the unit + e2e + CDC tests pass; the
  coverage scanner is green; the OLAP-analytics follow-on (ISS-P20) is named; the work is committed. No gate is
  greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: Time axis (cycles/milestones) + attachments. Body lists: contracts 11.2/5.5
  consumed; the edge-not-containment + 0-bytes-in-row assertions greened; the OLAP-analytics follow-on named
  (ISS-P20). Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P20 — The OLAP read store (CQRS, reindex-from-source only, restriction-flag-honouring)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I4, the OLAP slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I4", the OLAP read-store wiring bullet).
- **DEPENDS-ON.** ISS-P18 (the rollup whose aggregates feed analytics) + ISS-P19 (the cycles/milestones the
  burndown/CFD project). The M1 Storage prompt (the OLAP read store + restriction flag 11.6). The M2 Bus prompt
  (reindex-from-source 2.6). The index places this after ISS-P19 within M4 (it closes M4-I4).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale analytics off the bus);
    ../../external-insights/04-hard-problems.md §5 (reindex-from-source — the OLAP store rebuilds, never
    restored); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the restriction flag
    excludes a restricted subject).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/01-tech-and-data-model.md (the OLAP
    CQRS model — CFD/cycle-time/velocity/SLA-compliance, never touching the OLTP issue table).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §8 (the OLAP
    restriction-flag propagation).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 11.6 (the OLAP read store +
    restriction flag — CFD/cycle-time/velocity/SLA-compliance, never touching the OLTP issue table), 2.6
    (reindex-from-source — the OLAP store rebuilds by replay).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I4" (the OLAP bullet + the ISS-D8b reindex
    parity it shares).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row ISS-D8 (the reindex-from-source parity applies to the OLAP feed).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate (the OLAP CQRS feed):
  - The OLAP read store wiring (CQRS, reindex-from-source ONLY, restriction-flag-honouring): CFD, cycle-time,
    velocity, SLA-compliance — never touching the OLTP issue table. The feed is off the bus; a restricted subject
    (per the GDPR restriction flag) is excluded from analytics.
  - FLOOR named: none new (the OLAP store is derived, rebuildable by replay). State that the Monte-Carlo forecast
    (which reads OLAP throughput samples) is the M5 follow-on (ISS-P32); name it.
- **CONTRACTS TO IMPLEMENT.** 11.6 (consumed — the OLAP read store + restriction flag). 2.6 (consumed —
  reindex-from-source rebuilds the OLAP feed). Implement to the frozen shapes; escalate a needed change, do not
  diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The OLAP feed is off the bus and never touches the OLTP issue table (0 OLTP reads from the analytics path) —
    CI, the 0-OLTP-read assertion is the green artifact.
  - The restriction flag excludes a restricted subject from analytics (a restricted subject contributes 0 rows to
    CFD/velocity) — CI, the restriction-exclusion assertion is the green artifact. (Shares the ISS-D8b
    reindex-parity drill — the OLAP feed rebuilds drift-free by replay.)
- **TESTS (required).** Unit tests for the OLAP restriction-flag (a restricted subject is excluded from analytics)
  and the CQRS isolation (the feed reads the bus, never the OLTP table). A chained-mutation e2e test (restrict a
  subject → assert it drops from CFD → replay → assert drift-free). The drill scenario for ISS-D8b (the OLAP feed
  half). The provider/consumer CDC pair for 11.6 (the OLAP feed). State the cargo-mutants mutation-score floor for
  the restriction-flag module if mandatory-core (excluding a restricted subject is GDPR-correctness-bearing —
  treat it as mandatory-core).
- **DEFINITION OF DONE.** The OLAP feed is off the bus, never touches the OLTP table, honours the restriction
  flag, and rebuilds drift-free by replay; the 0-OLTP-read + restriction-exclusion assertions are green (ISS-D8b
  shared); the unit + e2e + drill tests pass; the coverage scanner is green; the Monte-Carlo follow-on is named;
  the work is committed. The M4-I4 milestone is complete with ISS-P18/P19/P20. No gate is greened by weakening a
  threshold.
- **COMMIT.** Header: P-<NNN> M4: OLAP read store (CQRS, restriction-flag-honouring). Body lists: contracts
  11.6/2.6 consumed; the 0-OLTP-read + restriction-exclusion + ISS-D8b-OLAP-parity greened; the Monte-Carlo
  follow-on named (ISS-P32). Branch first if on default; do not push unless asked. End with the workspace
  Co-Authored-By trailer.

---

### ISS-P21 — The two-pass ID-remapped import engine + the ADF lossy-map (the adoption gate)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I5, the import slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I5", the two-pass ID-remapped import engine + the ADF
  lossy-map bullets).
- **DEPENDS-ON.** ISS-P18+ISS-P19 (the full issue model + rollup + cycles — import populates it), ISS-P06 (the
  issue.* tokens import emits via the outbox). The M2 Knowledge prompt that froze the ADF→myelin-content
  lossy-map (13.2). The M0 protected-human-lane shed order (1.11). The index places this after the M4-I4 model is
  complete — it is the "first useful" milestone's adoption gate.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1 (EU-sovereign — "leave Atlassian cleanly" is a sovereignty credibility milestone) + §3
    (name-your-floors — the lossy nodes named never silent);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the export→import→export round-trip
    oracle; the resume-after-crash 0-dup), §4 (actually try it — the import is a real chained operation).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md (the
    two-pass, ID-remapped, idempotent + resumable import engine; the persisted source↔Myelin id map; the dry-run
    + reconciliation-report-first; the canonical interchange format; the per-tenant in-flight cap);
    03-events-contracts-and-glue.md (the import emits the normal issue.* events — one indexing path).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-2 (the
    ADF→myelin-content lossy-map frozen — lossy nodes named, never silent), the permission-scheme mapping as the
    lossy/legal-review leg (R-9).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 13.2 (the ADF→myelin-content
    lossy-map — the import conversion table; every lossy/dropped node recorded in the import report), 1.11 (the
    protected-human-lane shed order — the import is capped so it never starves an interactive tenant), 2.2 (the
    import emits issue.* via the one outbox path).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I5" (the import + the ISS-D9 exit gate) + §6
    (first-useful definition) + §5 (the import floor row, R-9).
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
  - FLOOR named: import = canonical core + the four adapters + the frozen ADF map (permission-scheme mapping is the
    named lossy leg, R-9, M5+ legal); the canonical interchange is the round-trip oracle. Name it in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 13.2 (consumed — the ADF lossy-map). 1.11 (consumed — the import shed budget). 2.2
  (consumed — the import emits via the outbox). Implement to the frozen shapes; escalate a needed change, do not
  diverge.
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
  pair for 13.2 (the ADF map). State the cargo-mutants mutation-score floor for the id-map module (mandatory-core
  — the resume/dedup is data-loss-adjacent).
- **DEFINITION OF DONE.** The import round-trips through the canonical interchange with named-lossy reporting, is
  idempotent/resumable (0 dup on crash), and respects the per-tenant lane budget; ISS-D9(a/b/c) emit dated green
  artifacts; the unit + e2e + drill tests pass; the coverage scanner is green; the import floor (R-9) is named;
  the work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: Import engine + ADF lossy-map (the adoption gate). Body lists: contracts
  13.2/1.11/2.2 consumed; ISS-D9(a) round-trip + (b) 0-dup-resume + (c) lane-within-budget greened with measured
  numbers; the import floor (R-9 permission-scheme leg) named. Branch first if on default; do not push unless
  asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P22 — "My Work" over the ONE Notif inbox + the humanise templates (one read-state truth)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I5, the My-Work slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I5", the "My Work" over the ONE inbox bullet).
- **DEPENDS-ON.** ISS-P04 (the declared define_notif_rule set, now wired), ISS-P06 (the issue.* events that
  produce inbox reasons). The M2 Notif prompts (list_inbox 7.1; mark/snooze 7.2; humanise 7.3; define_notif_rule
  7.6). The index places this after ISS-P21 within M4 (it completes M4-I5).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (one inbox — no second store);
    ../../external-insights/01-process-and-quality-doctrine.md §7 ("My Work" is a filter over the ONE inbox, not a
    new store; the ONE templating surface).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md ("My
    Work" over the ONE inbox; the reason/subject filters).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-L (the ONE
    templating surface).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.1 (list_inbox — the ONE inbox;
    "My Work" is a filter over reason/subject, never a second store), 7.2 (mark/snooze — one read-state truth),
    7.3 (humanise — the ONE templating surface; the SLA at-risk/unblocked/approval-requested strings register
    here), 7.6 (define_notif_rule — the Issues reason set, now wired).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I5" (the My-Work bullet) + §6 (first-useful).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate:
  - "My Work" (S10) = list_inbox(principal, filter) over the ONE Notif inbox (C-9): assigned/blocked/
    needs-approval/overdue are reason/subject filters with shared read-state (mark/snooze 7.2) — never a second
    store. Register the define_notif_rule set (7.6, now wired) + the humanise templates (SLA at-risk / unblocked /
    approval-requested, 7.3) into the ONE templating surface (no second template engine).
  - FLOOR named: none new. State that the SLA-breach escalation strings register here but the SLA engine lands in
    ISS-P26; name it in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 7.1/7.2 (consumed — list_inbox + mark/snooze for "My Work"). 7.3 (consumed —
  humanise, the ONE templating surface). 7.6 (consumed — the Issues notif-rules, now wired). Implement to the
  frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - "My Work" reads the ONE inbox with one read-state truth (a mark/snooze on a "My Work" item reflects in the
    underlying inbox; 0 second store) — CI, the one-read-state + 0-second-store assertion is the green artifact.
  - The humanise templates register on the ONE surface (0 second template engine; the SLA/unblocked/approval
    strings render via humanise) — CI.
- **TESTS (required).** Unit tests that "My Work" is a filter over list_inbox (not a separate store) and that
  mark/snooze shares read-state. A chained-mutation e2e test (assign an issue → it appears in "My Work" → snooze →
  it reflects in the inbox). The provider/consumer CDC pair for 7.1 (the inbox filter) + 7.6 (the reason set).
  State the cargo-mutants mutation-score floor if mandatory-core; state yes/no (the filter is not data-loss-bearing
  — likely no).
- **DEFINITION OF DONE.** "My Work" reads the ONE inbox with one read-state truth (0 second store); the humanise
  templates register on the ONE surface; the one-read-state + 0-second-store + ONE-template assertions are green;
  the unit + e2e + CDC tests pass; the coverage scanner is green; the SLA-engine follow-on (ISS-P26) is named; the
  work is committed. The first-useful bar (§6) is met with ISS-P21+ISS-P22. No gate is greened by weakening a
  threshold.
- **COMMIT.** Header: P-<NNN> M4: My Work over the ONE inbox + humanise templates. Body lists: contracts
  7.1/7.2/7.3/7.6 consumed; the one-read-state + 0-second-store + ONE-template assertions greened; the SLA-engine
  follow-on named (ISS-P26). Branch first if on default; do not push unless asked. End with the workspace
  Co-Authored-By trailer.

---

### ISS-P23 — The Issues ToolDefs + EffectApi plan-then-apply + the mock forecast/triage agents (gated on AG-D4)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I6, the tool-surface slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I6", the Issues ToolDefs + EffectApi + the mock runtime
  + the forecast/triage agents). MUST NOT run any tool until AG-D4 / CI-T1 is green (the M2 GATE).
- **DEPENDS-ON.** ISS-P12 (the governed transitions the tools drive) + ISS-P20 (the OLAP the forecast agent
  reads). The M1 Identity prompts (delegation 4.5; mint_run_token 4.7). The M2 Agent-fabric prompts (register_tool
  + the frozen requires_approval defaults 8.1; EffectApi::apply 8.2; AgentRuntime::step --use-mock 8.3;
  ToolHands::exec the unified sandbox 8.4; run --dry-run 8.7). The AG-D4 / CI-T1 GATE (M2). The index places this
  after ISS-P20 within M4 — it requires AG-D4 green.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native from the ground up; mock agents only during development — the strategy
    pattern); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the HITL withhold
    0-mutation, the mock-determinism), §8 (the human sign-off — HITL-gated governed transitions);
    ../../external-insights/03-agent-native-fabric.md (the plan-then-apply + the four uniform sandbox guarantees).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md (the
    Issues ToolDef catalogue; the frozen requires_approval defaults; the forecast + triage agents);
    05-hard-problems.md (the agent-native posture).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-6 (the frozen
    requires_approval defaults — Issues forecast/triage/sla_draft = no, SLA transition = caveat-gated; the four
    uniform sandbox guarantees).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 8.1 (register_tool + the frozen
    requires_approval defaults), 8.2 (EffectApi::apply — plan-then-apply: schema → capability → delegation →
    tenant → budget → HITL gate → apply via the public endpoint → meter; a withheld gated tool does not mutate),
    8.3 (AgentRuntime::step --use-mock — the mock runtime; real-LLM is post-M5), 8.4 (ToolHands::exec — the
    unified sandbox; the AG-D4 gate), 8.7 (run --dry-run — proposed effects without applying), 4.5/4.7
    (delegation / mint_run_token — the run policy intersection + the per-run token).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I6" (the ToolDefs + the AG-D5/AG-D9 exit gate)
    + §1 (sandbox escape is NOT owned by Issues — inherited; no agent tool over a red AG-D4) + §5 (the
    forecast/agent-runtime floor rows, R-5/R-10).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    the shared AG-D5 (HITL withhold) + AG-D9 (mock determinism) applied to Issues' tools.
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
    effects without applying). The runtime is the MOCK (--use-mock, scripted-deterministic) per VISION §3.
  - FLOOR named: agent runtime = mock (the real-LLM runtime is post-M5, after the safety drills are green, R-10 —
    a config/impl swap, not a rewrite, ISS-P32 names the swap); forecast = linear remaining ÷ velocity (the
    Monte-Carlo agent is the follow-on, R-5, ISS-P32). Name both in the crate doc. (Reserve/settle is ISS-P24; the
    stateful Trigger is ISS-P25.)
- **CONTRACTS TO IMPLEMENT.** 8.1 (consumed — register the Issues ToolDefs with the frozen defaults). 8.2
  (consumed — apply via plan-then-apply, no carve-out). 8.3 (consumed — the mock runtime). 8.4 (consumed — the
  unified sandbox; AG-D4-gated). 8.7 (consumed — dry-run). 4.5/4.7 (consumed — delegation + the per-run token).
  Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - AG-D9 mock-determinism applied to Issues' agent tools (identical effect sequences across replays) — CI, the
    identical-sequence hash is the green artifact.
  - AG-D5 HITL withhold applied to a governed transition (0 mutation pre-approval, 1 apply post-approval) — CI,
    the 0-pre-approval-mutation counter is the green artifact.
  - Upstream AG-D4 / CI-T1 GREEN (the gate invariant — no Issues agent tool runs over a red sandbox-escape gate).
    State explicitly; do not mark done if AG-D4 is red (a dated "blocked on AG-D4" row, not a weakened gate).
- **TESTS (required).** Unit tests for the ToolDef defaults (a gated tool is withheld; a no-approval tool
  suggests) and the EffectApi gate (a withheld gated tool does not mutate). A chained-mutation e2e test (dry-run a
  triage → human accepts → EffectApi applies once). The drill scenarios for the applied AG-D5/AG-D9. The
  provider/consumer CDC pair for 8.1 (the Issues ToolDefs). State the cargo-mutants mutation-score floor for the
  EffectApi-gate / HITL-withhold path (mandatory-core — the withhold is the no-unapproved-mutation seam).
- **DEFINITION OF DONE.** The Issues ToolDefs are registered with the frozen defaults; side-effecting tools apply
  only via plan-then-apply with no carve-out; the mock forecast/triage agents run deterministically; AG-D5 + AG-D9
  emit dated green artifacts; AG-D4 is green (else a dated blocked row); the unit + e2e + drill tests pass; the
  coverage scanner is green; the mock-runtime + linear-forecast floors are named; the work is committed. No gate
  is greened by weakening a threshold or by running a tool over a red AG-D4.
- **COMMIT.** Header: P-<NNN> M4: Issues ToolDefs + EffectApi + mock forecast/triage agents. Body lists:
  contracts 8.1/8.2/8.3/8.4/8.7 + 4.5/4.7 consumed; AG-D9 (identical sequences) + AG-D5 (0 pre-approval mutation)
  greened; AG-D4 confirmed green; the mock-runtime (R-10) + linear-forecast (R-5) floors named. Branch first if on
  default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P24 — Reserve/settle on every spend-bearing agent run (the same wallet as CI)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I6, the reserve/settle slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I6", the reserve/settle on every spend-bearing run
  bullet).
- **DEPENDS-ON.** ISS-P23 (the agent runs that spend). The M1 Storage prompt (reserve/settle cost gate 11.7). The
  M2 Workflow prompts (the workflow↔agent reserve/settle bookends 9.5). The index places this immediately after
  ISS-P23 within M4.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (cost-bounded agents); ../../external-insights/01-process-and-quality-doctrine.md §3
    (prove-it — reserve-iff-balance, settle-on-completion, never interrupt in-flight), §8 (the HITL approval card
    surfaces a live cost estimate).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/05-hard-problems.md (the reserve/settle
    posture).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-F (the per-effect
    idem_key for HITL cards).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 11.7 (reserve/settle — the same
    wallet as CI runs; reserve at dispatch — no balance, no start; settle on completion, never interrupt
    in-flight; integer minor-units), 9.5 (the workflow↔agent reserve/settle bookends).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I6" (the reserve/settle bullet).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate:
  - Reserve/settle on every spend-bearing run (reserve at dispatch — no balance, no start; settle on completion,
    never interrupt in-flight; integer minor-units; the same wallet as CI runs, 11.7). The reserve/settle bookends
    are the workflow bookends (9.5). The HITL approval card surfaces a live cost estimate before a human approves;
    the per-effect idem_key rule (OQ-F: card_id single, card_id:<effect_idx> multi/partial).
  - FLOOR named: none new (reserve/settle is the floor; the wallet is shared with CI). State the per-effect
    idem_key rule (OQ-F) in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 11.7 (consumed — reserve/settle). 9.5 (consumed — the workflow reserve/settle
  bookends). Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - Reserve-iff-balance + settle-on-completion balances every run (no balance → no start; settle never interrupts
    in-flight; the wallet nets to 0 over a run — reserve == settle for a completed run) — CI, the balanced-wallet
    (reserve == settle) signal is the green artifact.
  - The HITL approval card surfaces a live cost estimate before approval (0 approvals without a cost estimate) —
    CI.
- **TESTS (required).** Unit tests for reserve/settle (no balance → no start; settle on completion; never
  interrupt in-flight; integer minor-units) and the per-effect idem_key (card_id single, card_id:<effect_idx>
  multi). A chained-mutation e2e test (dispatch a spend-bearing run → reserve → complete → settle → assert the
  wallet balanced). The provider/consumer CDC pair for 11.7 (reserve/settle). State the cargo-mutants
  mutation-score floor for the reserve/settle module (mandatory-core — an unbalanced wallet is a cost-correctness
  failure).
- **DEFINITION OF DONE.** Reserve/settle balances every run (reserve == settle for a completed run; no balance →
  no start; never interrupts in-flight); the HITL card surfaces a live cost estimate before approval; the
  balanced-wallet + cost-estimate assertions are green; the unit + e2e + CDC tests pass; the coverage scanner is
  green; the per-effect idem_key rule is named; the work is committed. No gate is greened by weakening a
  threshold.
- **COMMIT.** Header: P-<NNN> M4: Reserve/settle on spend-bearing agent runs. Body lists: contracts 11.7/9.5
  consumed; the balanced-wallet (reserve == settle) + cost-estimate-before-approval greened; the per-effect
  idem_key rule (OQ-F) named. Branch first if on default; do not push unless asked. End with the workspace
  Co-Authored-By trailer.

---

### ISS-P25 — The stateful Trigger flagship ("Remind me when unblocked" — exactly-once across a restart)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I6, the stateful-Trigger slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I6", the stateful Trigger flagship bullet).
- **DEPENDS-ON.** ISS-P23 (the agent/tool surface), ISS-P18 (the issue_relation projection state the armable
  conditions read). The M2 Bus prompts (arm_trigger/disarm_trigger 3.3; EventMatcher=QueryAst 3.4; the
  reactive/dispatch tier 3.6). The M2 Workflow prompts (the timer wheel — stale_after 9.3). The M2 Notif prompt
  (the one inbox for on_resolve 7.1). The index places this immediately after ISS-P24 within M4 (it closes
  M4-I6).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (first-class triggers); ../../external-insights/01-process-and-quality-doctrine.md §3
    (prove-it — exactly-once across a restart, stale-once).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md (the
    stateful Trigger flagship "Remind me when unblocked" — the armable-condition catalogue).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 3.3/3.4 (arm_trigger /
    EventMatcher=QueryAst — the stateful Trigger), 3.6 (the reactive/dispatch tier), 9.3 (the stale_after durable
    timer), 7.1 (the one inbox for on_resolve).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I6" (the Trigger + the ISS-D7 exit gate).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row ISS-D7 (the stateful Trigger — exactly-once across a restart, stale-once).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate:
  - The stateful Trigger flagship ("Remind me when unblocked"): the armable-condition catalogue, each a frozen
    QueryAst over issue.* events + issue_relation projection state (Has/Ref/In); consumes the bus arm_trigger/
    disarm_trigger (3.3/3.4) + the myelin-flow stale_after durable timer (9.3) + the one inbox for on_resolve
    (7.1); fires once per arming; after stale_after (default 30d) a stale nudge fires once and the trigger goes
    stale.
  - FLOOR named: none new. State that the stale_after default is 30d (per-tenant tunable); name it in the crate
    doc.
- **CONTRACTS TO IMPLEMENT.** 3.3/3.4 (consumed — the stateful Trigger). 3.6 (consumed — the reactive/dispatch
  tier). 9.3 (consumed — the stale_after durable timer). 7.1 (consumed — the one inbox for on_resolve). Implement
  to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - ISS-D7 (arm "remind me when unblocked"; resolve the last blocker across a restart → fires EXACTLY ONCE into
    the one inbox; after stale_after, the stale nudge fires once, the trigger goes stale) — CI, 1-fire +
    stale-once is the green artifact.
- **TESTS (required).** Unit tests for the Trigger (exactly-once + stale-once across a simulated restart; the
  armable-condition QueryAst evaluates over issue_relation state). A chained-mutation e2e test (arm → resolve the
  last blocker across a restart → assert 1 fire → advance past stale_after → assert 1 stale nudge → stale). The
  drill scenario for ISS-D7. The provider/consumer CDC pair for 3.3 (the trigger). State the cargo-mutants
  mutation-score floor for the trigger fire/stale module (mandatory-core — fire-twice-or-never is a governance
  failure).
- **DEFINITION OF DONE.** The stateful Trigger fires exactly-once per arming + stale-once after stale_after,
  across a restart; ISS-D7 emits a dated green artifact (1-fire + stale-once); the unit + e2e + drill tests pass;
  the coverage scanner is green; the stale_after default note is written; the work is committed. The M4-I6
  milestone is complete with ISS-P23/P24/P25. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: Stateful Trigger (remind-me-when-unblocked, exactly-once). Body lists: contracts
  3.3/3.4/3.6/9.3/7.1 consumed; ISS-D7 (1-fire/stale-once) greened; the stale_after default named. Branch first if
  on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P26 — The SLA business-calendar engine over myelin-flow (fire_at to-the-second across a restart)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I7, the SLA-engine slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I7", the SLA logic engine over myelin-flow bullet).
- **DEPENDS-ON.** ISS-P12 (the governed transitions SLA arms over), ISS-P25 (the durable-timer/trigger
  infrastructure pattern), ISS-P22 (the humanise SLA strings). The M2 Workflow prompts (the timer wheel + the
  durable signal 9.3/9.4). The M2 Notif prompts (oncall_now/page + the frozen escalation chain 7.5; humanise 7.3).
  The index places this in M4 alongside the other M4-I7 slices (before the X-1 guard, which needs the CI
  producer).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1 (corporate SLAs/reporting/audit);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — fire_at to-the-second across a
    restart).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md (the SLA
    logic engine — the business-calendar arithmetic over an IANA-tz calendar; DST/holiday/multi-day; precompute
    fire_at + at_risk_fire_at; arm two timers; cheap disarm/re-arm; never poll).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §5 (the frozen
    escalation chain page → oncall_now → escalate-after-timer).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 9.3 (the timer wheel — the SLA
    fire_at), 9.4 (the durable signal — multi-day HITL/escalation), 7.5 (oncall_now/page + the frozen escalation
    chain), 7.3 (humanise — the SLA strings).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I7" (the SLA + the ISS-D6 exit gate) + §5 (the
    long-SLA history-compaction floor, R-11).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row ISS-D6 (SLA durability — fire after a restart; calendar corpus to-the-second; chain start).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate:
  - The SLA logic engine over myelin-flow: the business-calendar arithmetic (convert a business-time budget into a
    wall-clock fire_at over an IANA-tz calendar; DST/holiday/multi-day correct); precompute fire_at +
    at_risk_fire_at, arm two timers on the wheel (9.3); cheap disarm/re-arm on pause/resume (the QueryAst
    pause_conditions); never poll, never pollute the wheel with calendar logic. On breach, start the FROZEN
    escalation chain (page → oncall_now → escalate-after-timer) as a durable workflow (7.5); breach/met feed OLAP
    for compliance reporting.
  - FLOOR named: very-long time_to_resolution SLAs get history-compaction (the myelin-flow continue-as-new note)
    as the named follow-on (R-11, M5+). Name it in the crate doc. (The CheckStatus guard is ISS-P27.)
- **CONTRACTS TO IMPLEMENT.** 9.3/9.4 (consumed — the SLA timers + the escalation durable signal). 7.5 (consumed —
  oncall_now/page + the chain). 7.3 (consumed — the SLA humanise strings). Implement to the frozen shapes;
  escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - ISS-D6 ((a) breach fires after a process restart; (b) a business-calendar corpus — DST, multi-day, holiday,
    pause/resume → computed fire_at matches wall-clock TO THE SECOND; (c) breach starts the escalation chain) —
    CI, fire-at accuracy (to-the-second) + chain-start is the green artifact.
- **TESTS (required).** Unit tests for the business-calendar arithmetic (DST boundary, holiday, multi-day,
  pause/resume → fire_at correct to-the-second). A chained-mutation e2e test (arm an SLA → restart → assert breach
  fires + chain starts). The drill scenario for ISS-D6. The provider/consumer CDC pair for 7.5 (the escalation
  chain). State the cargo-mutants mutation-score floor for the business-calendar module (mandatory-core — a
  mis-fired SLA is a governance failure).
- **DEFINITION OF DONE.** The SLA engine computes fire_at to-the-second over a business calendar, survives a
  restart, and starts the escalation chain on breach; ISS-D6 emits a dated green artifact (to-the-second +
  chain-start); the unit + e2e + drill tests pass; the coverage scanner is green; the long-SLA history-compaction
  floor (R-11) is named; the work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: SLA business-calendar engine. Body lists: contracts 9.3/9.4/7.5/7.3 consumed;
  ISS-D6 (fire-at to-the-second + chain start) greened; the long-SLA history-compaction floor (R-11) named. Branch
  first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P27 — The CI-red governed-transition guard (closing the X-1 consumer; reads trust_tier off the fact)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I7, the CheckStatus-guard slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I7", the CI-red governed-transition guard bullet).
  Closes the Issues side of the X-1 seam — requires the CI producer (M4) + the Git projection (M3).
- **DEPENDS-ON.** ISS-P12 (the governed transitions + the ISS-D12 guard half) + ISS-P23 (the agent HITL-gated
  transition path) + ISS-P17 (project() the guard reads through). The X-1 CheckStatus seam: the Git projection
  (M3, the consumer half) + the CI producer (M4) — proven end-to-end by GIT-D10 / CI-D8 (contract 5.9). The index
  places this LATE in M4 (after CI's producer lands) so the X-1 seam is closeable.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (the poisoned-Done defence — never recompute trust);
    ../../external-insights/01-process-and-quality-doctrine.md §7 (the X-1 seam reconciled at the plan layer —
    Issues reads trust_tier off the fact, never recomputes it), §3 (prove-it — the guard blocks).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md (the
    CI-red governed-transition guard — read CheckStatus{state, trust_tier} via project(PR_ref)).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-1 (the Git↔CI
    CheckStatus seam — an untrusted_fork success is neutral until endorsed; Issues never recomputes trust).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 5.9 (the Git↔CI CheckStatus seam —
    Issues reads CheckStatus{state, trust_tier} via the linked PR's project; never recomputes trust), 4.2 (check +
    CaveatContext — the transition ABAC).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I7" (the guard + the ISS-D12-complete exit
    gate; needs GIT-D10/CI-D8) + §0 (the X-1 consumer-of-a-consumer posture).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row ISS-D12 (the guard — "can't mark Done while CI red" + the agent HITL-gated half).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate:
  - The CI-red governed-transition guard (the X-1 consumer half): the "can't mark Done while CI red on the linked
    PR" guard reads the linked PR's commit CheckStatus{state, trust_tier} via project(PR_ref) at transition time —
    checks state = success AND an acceptable trust posture (an untrusted_fork success is NEUTRAL until endorsed).
    Issues NEVER recomputes trust — it reads trust_tier off the fact. The agent hitting this governed transition
    is HITL-gated (the ISS-P23 path).
  - FLOOR named: none new. State that the guard rests on the proven X-1 seam (GIT-D10/CI-D8), not a doc claim;
    name it in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 5.9 (consumed — the CheckStatus guard reads trust_tier off the fact, never
  recomputes). 4.2 (consumed — the transition ABAC). Implement to the frozen shapes; escalate a needed change, do
  not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - ISS-D12 complete (the CI-red guard: "can't mark Done while CI red on the linked PR" reads CheckStatus + trust
    posture → transition blocked with a reason; an agent hitting a governed transition is HITL-gated, withheld, 0
    mutation pre-approval) — CI, transition-blocked + 0 pre-approval mutation is the green artifact.
  - Upstream GIT-D10 / CI-D8 GREEN (the X-1 seam end-to-end — the gate invariant: Issues' guard rests on a proven
    seam, not a doc claim). State explicitly; do not mark done if the seam is red (a dated "blocked on GIT-D10/
    CI-D8" row, not a guard that recomputes trust to fake green).
- **TESTS (required).** Unit tests for the guard (an untrusted_fork success is neutral; a trusted success
  unblocks; the agent is withheld). A chained-mutation e2e test (attempt a Done transition while CI red → blocked
  → CI goes green → transition allowed). The drill scenario for ISS-D12. The provider/consumer CDC pair for 5.9
  (the CheckStatus consumer — Issues' read side). State the cargo-mutants mutation-score floor for the guard
  module (mandatory-core — the poisoned-Done defence is correctness-bearing).
- **DEFINITION OF DONE.** The CI-red guard reads trust_tier off the fact (never recomputes) and blocks with a
  reason; the agent is HITL-gated; ISS-D12 is complete (transition blocked, 0 pre-approval mutation); GIT-D10/CI-D8
  are green (else a dated blocked row); the unit + e2e + drill tests pass; the coverage scanner is green; the
  proven-seam note is written; the work is committed. No gate is greened by weakening a threshold or recomputing
  trust.
- **COMMIT.** Header: P-<NNN> M4: CheckStatus guard (closes X-1 consumer). Body lists: contract 5.9 (the
  CheckStatus consumer) + 4.2 consumed; ISS-D12 (transition blocked, 0 pre-approval mutation) greened; GIT-D10/CI-D8
  confirmed green. Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By
  trailer.

---

### ISS-P28 — The cross-subsystem reflexes (git/chat/identity/ci consumers)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I7, the cross-sub-reflexes slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I7", the cross-subsystem consumers bullet).
- **DEPENDS-ON.** ISS-P12 (the workflow auto-transitions the reflexes drive) + ISS-P27 (the guard the
  ci.check.updated reflex feeds) + ISS-P06 (the create path the chat reflex drives) + ISS-P07 (the
  reassign/anonymise on identity.member.*). The M3 Git producer (git.* events), the M4 CI producer (ci.check.*),
  the Chat producer (chat.message.created), Identity (identity.member.*). The index places this alongside the
  other M4-I7 slices, after the producers exist.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (the differentiator — work flows between tools);
    ../../external-insights/01-process-and-quality-doctrine.md §7 (cross-sub reactions are consumers off the bus,
    not bespoke integrations).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md (the
    cross-sub reflexes: git.branch.created/git.pr.opened/git.pr.merged → link + auto-transition;
    chat.message.created → create issue with a relates edge; identity.member.* → reassign/anonymise;
    ci.check.updated → feed the guard).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 2.4 (EventHandler — the reflexes
    are consumers), 5.4 (refs.edge — the link/relates edges), 5.9 (the ci.check.updated → guard feed).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I7" (the cross-sub reflexes bullet).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate (bus consumers):
  - The cross-subsystem consumers (the cross-sub reflexes): git.branch.created / git.pr.opened / git.pr.merged →
    link + workflow-permitting auto-transition; chat.message.created → create issue with a relates edge;
    identity.member.* → reassign/anonymise; ci.check.updated → feed the guard (ISS-P27). Each is an idempotent
    consumer (consumer_dedup); a workflow-permitting auto-transition runs through the FSM interpreter (ISS-P12),
    never a bypass.
  - FLOOR named: none new. State that auto-transitions are workflow-permitting only (never a governance bypass);
    name it in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 2.4 (consumed — the reflexes are EventHandlers). 5.4 (consumed — the link/relates
  edges). 5.9 (consumed — the ci.check.updated → guard feed). Implement to the frozen shapes; escalate a needed
  change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - Each reflex is idempotent (a replayed git.pr.merged / chat.message.created produces 0 duplicate links/issues
    via consumer_dedup) — CI, the 0-duplicate-on-replay assertion is the green artifact.
  - A workflow-permitting auto-transition runs through the FSM interpreter (0 governance bypass; a guarded
    transition is still blocked) — CI, the no-bypass assertion is the green artifact.
- **TESTS (required).** Unit tests for each reflex (idempotent; the auto-transition respects the workflow guard;
  identity.member.* reassigns/anonymises). A chained-mutation e2e test (git.pr.merged → link + auto-transition →
  replay → assert 0 duplicate). The provider/consumer CDC pair for the reflex consumers (2.4). State the
  cargo-mutants mutation-score floor if the reflex module is mandatory-core; state yes/no.
- **DEFINITION OF DONE.** The cross-sub reflexes fire idempotently (0 duplicate on replay) and auto-transitions
  respect the workflow guard (0 bypass); the assertions are green; the unit + e2e + CDC tests pass; the coverage
  scanner is green; the no-bypass note is written; the work is committed. No gate is greened by weakening a
  threshold.
- **COMMIT.** Header: P-<NNN> M4: Cross-subsystem reflexes (git/chat/identity/ci). Body lists: contracts
  2.4/5.4/5.9 consumed; the 0-duplicate-on-replay + no-governance-bypass assertions greened. Branch first if on
  default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P29 — The governance admin views (S13–S18; each preceded by its design sketch)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I7, the admin-views slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I7", the governance admin views bullet).
- **DEPENDS-ON.** ISS-P11 (the schemes the editor edits) + ISS-P12 (the workflow/guard builder) + ISS-P26 (the
  SLA policy editor + calendar) + ISS-P25 (the trigger builder). The M2 Identity prompt (list_subjects/explain 4.4
  — the permission inspector). The index places this alongside the other M4-I7 slices.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1 (corporate SLAs/reporting/audit) + §3 (no frontend code without a reviewed sketch);
    ../../external-insights/01-process-and-quality-doctrine.md §8 (the human sign-off — sketch-then-build for
    decision-shaped surfaces).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md +
    04-views-cli-and-api.md (the governance admin views S13–S18); the design folder
    (../04-subsystem-architectures/issue-tracker/design/wireframes.md — the admin/governance screens).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 4.4 (list_subjects/explain — the
    permission inspector S15).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I7" (the admin views bullet).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate (+ the design record):
  - The governance admin views (S13 workflow/scheme editor with the QueryAst guard builder; S14 SLA policy editor
    + calendar editor + breach-simulation; S15 team/project settings + the permission inspector via list_subjects/
    explain; S16 automation/trigger builder; S18 audit/change-history) — each preceded by its design sketch
    (VISION §3), the sketches signed off in the design folder.
  - FLOOR named: none new. State that the permission inspector reads list_subjects/explain (4.4), never a private
    recompute; name it in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 4.4 (consumed — list_subjects/explain for the inspector). Implement to the frozen
  shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The admin-view design sketches are REVIEWED-AND-SIGNED-OFF in the design folder (S13/S14/S15/S16/S18, incl.
    the empty/loading/error/permission states) — sign-off recorded, dated, the green artifact for the
    pre-frontend gate.
  - The permission inspector (S15) reads list_subjects/explain (0 private recompute; the inspector's answer equals
    Identity's explain) — CI, the inspector-equals-explain assertion is the green artifact.
- **TESTS (required).** Unit tests that the S15 inspector's answer equals Identity's explain (0 private recompute)
  and that the S14 breach-simulation uses the ISS-P26 SLA engine (not a parallel calc). The design-sketch sign-off
  recorded in the design folder. The provider/consumer CDC pair for 4.4 (list_subjects/explain). State the
  cargo-mutants mutation-score floor if the inspector module is mandatory-core; state yes/no.
- **DEFINITION OF DONE.** The admin views are sketched + signed off (all states); the S15 inspector reads
  list_subjects/explain (0 private recompute); the S14 breach-simulation uses the real SLA engine; the sign-off +
  inspector-equals-explain assertions are green; the unit + CDC tests pass; the coverage scanner is green; the
  work is committed. The M4-I7 milestone is complete with ISS-P26/P27/P28/P29. No gate is greened by weakening a
  threshold.
- **COMMIT.** Header: P-<NNN> M4: Governance admin views (S13–S18). Body lists: contract 4.4 consumed; the design
  sketches signed off (all states), the S15-inspector-equals-explain greened. Branch first if on default; do not
  push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P30 — Real-time board sync over the firehose resume-cursor protocol (0 ops lost on reconnect)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I8, the board-sync slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I8", real-time board sync over the firehose
  resume-cursor protocol bullet).
- **DEPENDS-ON.** ISS-P16 (the views the sync drives). The M2 Bus prompt (the firehose resume-cursor protocol —
  subscribe/resume/bounded scope 3.5). The M0 protected-human-lane shed order (1.11). The index places this in M4
  (a M4 band-exit drill).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (top-of-the-line UX — real-time, never lose an op);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — 0 ops lost on reconnect, resync
    fallback named not silent).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md (the
    real-time board sync — optimistic local updates + bus-driven cache invalidation; subscribe(stream, scope =
    board:<id>) bounded never *; resume(stream, scope, last_seq) backfill then live; resync_required → *.snapshot;
    per-connection in-flight caps; presence/typing on the ephemeral firehose).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-J (the firehose
    resume-cursor protocol — bounded scope, reconnect loses zero ops, resync_required fallback), OQ-K (the
    per-surface shed budget for the connection-storm).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 3.5 (the firehose resume-cursor
    protocol), 1.11 (the connection-storm shed budget).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I8" (the board-sync + the ISS-D13 exit gate) +
    §5 (the sync floor row, R-8).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row ISS-D13 (board sync — 0 ops lost on reconnect, resync fallback).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate:
  - Real-time board sync over the frozen firehose resume-cursor protocol: optimistic local updates + bus-driven
    cache invalidation; subscribe(stream, scope = board:<id>) (bounded, NEVER *; a 50k-row board paginates its
    scope); on reconnect resume(stream, scope, last_seq) backfills (last_seq, now] then live — loses ZERO ops;
    last_seq past the retention window → resync_required → *.snapshot replay (NAMED, not silent). Per-connection
    in-flight frame caps; a slow consumer is dropped to resync_required (the OQ-K per-surface shed budget).
    Presence/typing ride the EPHEMERAL firehose, never the durable bus.
  - FLOOR named: sync = optimistic + resume-cursor (offline/local-first is the named follow-on, R-8, post-M5, out
    of v1 scope unless promoted). Name it in the crate doc. (Erasure is ISS-P31.)
- **CONTRACTS TO IMPLEMENT.** 3.5 (consumed — the firehose resume-cursor protocol). 1.11 (consumed — the
  connection-storm shed budget). Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - ISS-D13 (a board at scope = board:<id> drops mid-edit-storm → resume backfill then live loses ZERO ops;
    last_seq past the window → resync_required → *.snapshot) — CI, 0 ops lost + the resync fallback is the green
    artifact.
- **TESTS (required).** Unit tests for the resume protocol (bounded scope; backfill then live; resync_required on
  a past-window last_seq). A chained-mutation e2e test (edit-storm → drop → resume → assert 0 ops lost). The drill
  scenario for ISS-D13. The provider/consumer CDC pair for 3.5 (the resume protocol). State the cargo-mutants
  mutation-score floor for the resume-protocol module (mandatory-core — a lost op is a correctness failure).
- **DEFINITION OF DONE.** Real-time board sync over the resume-cursor protocol loses zero ops on reconnect and
  falls back to resync_required cleanly; ISS-D13 emits a dated green artifact (0 ops lost, resync fallback); the
  unit + e2e + drill tests pass; the coverage scanner is green; the sync floor (R-8) is named; the work is
  committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: Real-time board sync (resume-cursor, 0 ops lost). Body lists: contracts 3.5/1.11
  consumed; ISS-D13 (0 ops lost, resync fallback) greened; the sync floor (R-8) named. Branch first if on default;
  do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P31 — Erasure-reaches-every-holder (the PersonalDataHolder fan-out + post-restore re-erasure)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-I8, the erasure slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I8", the PersonalDataHolder erasure fan-out bullet). The
  last M4 milestone before the band exit.
- **DEPENDS-ON.** ISS-P07 (the holder registration + the per-subject-DEK columns) + ISS-P18 (the rollup holder) +
  ISS-P20 (the OLAP holder) + ISS-P17 (the Search projection + Refs holder) + ISS-P19 (the attachment blobs). The
  M1 Identity prompt (erase + the pseudonym-map shred 4.8). The M1 GDPR prompts (the PersonalDataHolder ops 10.1;
  the erasure ledger 10.8; the ONE posture by reference 10.9). The M1 Storage prompt (per-subject DEK crypto-shred
  11.4). The index places this LAST in M4 for Issues (a M4 band-exit drill).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe by construction — data-subject erasure reaches every holder);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — holder receipts on erasure), §1
    (name-your-floors — the third-party residual is [OPEN — LEGAL]);
    ../../external-insights/04-hard-problems.md §1 (erasure-vs-immutability — the ONE posture).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/06-reconciliation-compliance.md (the
    erasure fan-out across every Issues holder; the pseudonym-map shred; the *.erased tombstones; post-restore
    re-erasure).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-7/OQ-G (the ONE
    free-text/immutable erasure posture — per-subject DEK + pseudonym-map shred + restrict; the third-party
    residual [OPEN — LEGAL]).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 4.8 (erase + the pseudonym-map
    shred — "Former user 8a2f" without rewriting issues others own), 10.1 (PersonalDataHolder{locate, export,
    rectify, restrict, erase} — across every Issues holder), 10.8 (the erasure ledger — post-restore re-erasure
    GD-14), 10.9 (the ONE posture by reference — the third-party residual [OPEN — LEGAL]), 11.4 (per-subject DEK
    crypto-shred), 2.7 (the *.erased tombstones live consumers act on).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M4-I8" (the erasure + the ISS-D11 exit gate; the
    M4 band exit) + §5 (the erasure floor rows, R-1/R-2).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row ISS-D11 (erasure — PII gone from every holder, post-restore re-erasure, the third-party residual the
    documented limit).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate:
  - Erasure-reaches-every-holder: implement the PersonalDataHolder ops (locate/export/rectify/restrict/erase)
    across EVERY Issues holder (the issue row free-text via per-subject DEK shred, the change-log deltas,
    comments, attachment blobs, the OLAP read store + restriction flag, the Search index incl. embeddings, the
    Refs projection). Id erase shreds the pseudonym map ("Former user 8a2f" across history without rewriting
    issues others own); emit issue.*.erased tombstones (live consumers tombstone Search/Refs/OLAP/Notif);
    post-restore re-erasure (GD-14) runs against the erasure ledger. The third-party free-text residual is handled
    per the ONE platform posture BY REFERENCE (10.9), [OPEN — LEGAL].
  - FLOOR named: free-text PII erasure = per-subject DEK + pseudonym-map shred + restrict (the structural floor
    ships now; the third-party-mention residual basis is [OPEN — LEGAL], R-1); worklog special-category
    classification is [OPEN — LEGAL], R-2. Name each in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 4.8 (consumed — erase + the pseudonym-map shred). 10.1 (owned — the Issues
  PersonalDataHolder ops, now full). 10.8 (consumed — the erasure ledger + post-restore re-erasure). 10.9
  (consumed — the ONE posture by reference). 11.4 (consumed — per-subject DEK shred). 2.7 (consumed — the *.erased
  tombstones). Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - ISS-D11 (erase a subject → PII gone from every holder: per-subject DEK, change-log, comments, attachments,
    OLAP + restriction, Search incl. embeddings, Refs; post-restore re-erasure catches a restore; the third-party
    residual is the documented [OPEN — LEGAL] limit) — SCHED, the per-holder receipts + the re-erasure is the
    green artifact.
- **TESTS (required).** Unit tests for the holder ops (each holder's erase shreds/tombstones; the pseudonym map
  shreds without rewriting others' issues). A chained-mutation e2e test (erase → assert every holder receipt →
  restore → assert re-erasure). The drill scenario for ISS-D11. The provider/consumer CDC pair for 10.1 (the
  Issues holder ops). State the cargo-mutants mutation-score floor for the holder-erase module (mandatory-core —
  incomplete erasure is a GDPR failure).
- **DEFINITION OF DONE.** Erasure reaches every Issues holder with per-holder receipts + post-restore re-erasure,
  the third-party residual documented as [OPEN — LEGAL]; ISS-D11 emits a dated green artifact (per-holder receipts
  + re-erasure); the unit + e2e + drill tests pass; the coverage scanner is green; the erasure floors (R-1/R-2)
  are named; the work is committed. The M4 band-exit slice Issues owns is green (ISS-D1/D2/D3 + ISS-D5/D6/D12 +
  ISS-D13/D11 + the X-1 seam). No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: Erasure-reaches-every-holder. Body lists: contracts 4.8 + 10.1/10.8/10.9 + 11.4
  + 2.7 consumed; ISS-D11 (every holder receipt + re-erasure) greened; the erasure (R-1) + worklog (R-2) floors
  named. Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P32 — The measured floor follow-ons (move-CRDT / materialised rollup / distributed-SQL / cross-cell / Monte-Carlo / column-store)

- **BAND.** M5.
- **ROADMAP MILESTONE.** M5-I9, the floor-follow-ons slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M5-I9", the move-CRDT / materialised rollup / distributed-SQL
  / cross-cell rollup / Monte-Carlo forecast / full DSR fan-out / column-store seam bullets).
- **DEPENDS-ON.** ISS-P09 (the CAS floor the move-CRDT promotes) + ISS-P18 (the read-time rollup it materialises) +
  ISS-P23 (the linear forecast it promotes to Monte-Carlo) + ISS-P20 (the OLAP the Monte-Carlo reads) + ISS-P31
  (the holders the DSR fan-out covers) + ISS-P05 (the PG-sharded floor distributed-SQL promotes). The M1 Tenancy
  prompt (the CrossCellPointer bridge frame 12.6, now live). The M5 cross-system prompts that bring multi-cell live
  + the full DSR fan-out (10.4). The M5 Knowledge prompt that promotes the CRDT (the shared Yrs type Issues
  reuses). The index places this in M5 (all five subsystems on one substrate; the deterministic correctness drills
  green).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors — promote on MEASURED evidence);
    ../../external-insights/01-process-and-quality-doctrine.md §7 (promote a floor only on a measured trigger,
    never premature); ../../external-insights/04-hard-problems.md §2 (CRDT-after-CAS), §5 (event-volume
    column-store seam — only once volume is measured).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/05-hard-problems.md (the move-CRDT
    promotion reusing the byte-identical order_key; the materialised-rollup trigger; the distributed-SQL trigger;
    the cross-cell portfolio rollup over the CrossCellPointer; the Monte-Carlo forecast; the column-store seam).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-I (the cross-cell
    bridge — resolution always cell-local; only the projection crosses), OQ-C (the materialisation trigger).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 12.6 (the CrossCellPointer bridge
    — the cross-cell portfolio rollup), 10.4 (the DSR fan-out iterating member_cells), 3.5 (the resume-cursor
    transport the move-CRDT slots into), 11.6 (the OLAP for the Monte-Carlo throughput samples).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M5-I9" (the floor-follow-ons work + the
    ISS-D5-re-green + GA-D1/CP-D7/CP-D8 exit gates) + §5 (the full floors register — every R-3..R-11 follow-on).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows ISS-D5 (re-green across the move-CRDT engine-promote boundary) + GA-D1/CP-D7/CP-D8.
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate (the measured promotions
  are gated by their triggers — ship the seam + the measurement, promote on the measured signal):
  - The move-CRDT, after the CAS floor (R-3): a Yrs list / Fugue move-CRDT slotting into the SAME resume-cursor
    firehose transport, reusing Knowledge's Yrs type. Promoted ONLY on measured concurrent-reorder pain. Because
    the order_key is already byte-identical, the promotion swaps the conflict-resolution engine, not the data
    model. ISS-D5 re-runs across the engine-promote boundary so it stays green when the CRDT lands.
  - Materialised rollup, after the read-time floor (R-4, KN-3): materialise a subtree's rollup only when it is
    MEASURED large; the read-time floor remains for small subtrees.
  - Distributed-SQL, after PG-sharded-by-tenant (R-6): only if a single tenant's shard is MEASURED to outgrow PG.
    Never premature — ship the measurement, not the migration, unless the trigger fires.
  - Cross-cell portfolio rollup, after single-cell (R-7, OQ-I): the rollup walk over a remote child rides the
    frozen PII-free CrossCellPointer{subject, type, correlation_id, home_cell}; resolution is always cell-local
    (the home cell renders + permission-checks; only the projection crosses). The FLOOR drills GA-D8/CP-D7/CP-D8
    are now owed (DSR fan-out iterates member_cells).
  - The Monte-Carlo forecast agent, after the linear floor (R-5): reads OLAP throughput samples; the swap is a
    strategy change, not a rewrite.
  - The full DSR / erasure fan-out (10.4, GA-D1): every Issues holder now exists, so the fan-out is complete; the
    [OPEN — LEGAL] residual posture (10.9) is instantiated by reference.
  - The event-volume column-store seam (EI-04 §5): a seam for Issues' highest-volume streams (issue.updated, the
    change-log) — added only once volume is MEASURED, not before.
  - FLOOR named: each promotion is MEASURED — name the trigger in the crate doc (the floor stays until its measured
    signal fires); the real-LLM runtime (R-10) remains the post-M5 follow-on.
- **CONTRACTS TO IMPLEMENT.** 12.6 (consumed — the CrossCellPointer bridge, now live). 10.4 (consumed — the DSR
  fan-out across member_cells). 3.5 (consumed — the move-CRDT transport). 11.6 (consumed — the Monte-Carlo OLAP
  samples). Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - ISS-D5 re-green across the move-CRDT engine-promote boundary (if the CRDT is promoted; the drill was written to
    survive the swap — 0 clobber still holds) — CI, the re-green (0 clobber) is the green artifact.
  - GA-D1 / CP-D7 / CP-D8 (DSR fan-out 0 holders missed incl. Issues; cross-cell rollup per-cell receipt set + the
    PII-free bridge) — SCHED, the 0-holders-missed + per-cell-receipt + PII-free-bridge signals are the green
    artifact.
- **TESTS (required).** Unit tests for the move-CRDT promotion (the order_key data model is unchanged across the
  swap) and the cross-cell rollup (only the PII-free projection crosses; resolution is cell-local). A
  chained-mutation e2e test (promote the move-CRDT → re-run ISS-D5 → assert 0 clobber; a cross-cell rollup → assert
  only the PII-free projection crosses). The drill scenarios for ISS-D5-re-green + GA-D1/CP-D7/CP-D8. State the
  cargo-mutants mutation-score floor for any promoted core module (the move-CRDT conflict engine is
  correctness-bearing — treat it as mandatory-core when promoted).
- **DEFINITION OF DONE.** The floor follow-ons are promoted on their MEASURED triggers (or the floor + its
  measurement seam stand, named); ISS-D5 re-green + GA-D1/CP-D7/CP-D8 emit dated green artifacts; the unit + e2e +
  drill tests pass; the coverage scanner is green; every measured-promotion trigger is named in writing; the
  real-LLM runtime (R-10) follow-on is named; the work is committed. No gate is greened by weakening a threshold or
  by promoting a floor without its measured trigger.
- **COMMIT.** Header: P-<NNN> M5: Issues measured floor follow-ons. Body lists: contracts 12.6/10.4/3.5/11.6
  consumed; ISS-D5-re-green + GA-D1/CP-D7/CP-D8 greened; the measured-promotion triggers named (move-CRDT/
  materialised-rollup/distributed-SQL/cross-cell/Monte-Carlo/column-store); the real-LLM runtime (R-10) follow-on
  named. Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P33 — World-scale hardening (the F6 surge family + the scale benchmarks)

- **BAND.** M5.
- **ROADMAP MILESTONE.** M5-I9, the world-scale-hardening slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M5-I9", the F6 surge family + the prod-scale benchmarks +
  online-migration-under-load + restore-verify at cell scale).
- **DEPENDS-ON.** ISS-P13/P14 (the planner the surge stresses) + ISS-P26 (the SLA timers at scale) + ISS-P32 (the
  promoted floors the surge re-confirms). The M1 substrate failure-injection harness (the 1×/10×/30× load
  generator). The M5 cross-system prompts that bring multi-cell live. The index places this in M5 alongside
  ISS-P32.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the 30x surge with quantified
    thresholds; the human lane holds, the agent lane sheds).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/05-hard-problems.md (the scale
    posture); 07-drills-and-open-questions.md (the surge + scale drills).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-K (the surge shed
    budgets).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 1.11 (the per-surface shed budgets
    the surge stresses), 11.6 (the OLAP at cell scale), 3.5 (the firehose under surge).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M5-I9" (the F6 surge + ISS-D2-at-cell-scale exit
    gates) + §6 (production-hardened definition).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows ISS-D2 (at cell scale under world-scale load) + the F6 surge family rows.
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-issues crate (drills + hardening, no
  new feature surface):
  - World-scale hardening: the 30x surge across the Issues owner (the protected human lane holds within budget;
    the agent lane sheds 429+Retry-After; cross-tenant impact 0); the prod-scale benchmarks (the 1M+-issue board,
    the 50-team-initiative rollup fan-out, millions of SLA timers as an indexed range read);
    online-migration-under-load on the hot issue tables; restore-verify at cell scale.
  - FLOOR named: none new (this prompt hardens; the floor follow-ons are ISS-P32). State that the surge re-runs the
    F1 leak-free + the reorder-0-clobber families under load; name them in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 1.11 (consumed — the surge shed budgets). 11.6 (consumed — the OLAP at cell scale).
  3.5 (consumed — the firehose under surge). Implement to the frozen shapes; escalate a needed change, do not
  diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The F6 surge family across the Issues owner (SUB-D3-shaped: human lane within budget, agent sheds 429+
    Retry-After, cross-tenant impact 0) — SCHED, the lane-budget + the cross-tenant-0 signals are the green
    artifact.
  - ISS-D2 at cell scale re-confirmed (the 1M+-issue board under the <1s budget under world-scale load) — SCHED,
    p99 < 1s at cell scale is the green artifact.
- **TESTS (required).** A chained-mutation surge e2e test (30x mixed-principal load → assert the human lane holds +
  the agent lane sheds + cross-tenant impact 0). The drill scenarios for the F6 surge + ISS-D2-at-scale. Re-confirm
  the online-migration-under-load on the hot issue tables (0 downtime) and restore-verify at cell scale. No new
  mutation floor (the core modules' floors were set in their own prompts); re-confirm they hold under the surge.
- **DEFINITION OF DONE.** The F6 surge holds (human lane within budget, agent sheds, cross-tenant 0); ISS-D2 at
  cell scale + online-migration-under-load + restore-verify-at-cell-scale emit dated green artifacts; the surge-e2e
  + drill tests pass; the coverage scanner is green; the work is committed. The production-hardened bar (§6) is met
  with ISS-P32+ISS-P33. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M5: Issues world-scale hardening (F6 surge + scale benchmarks). Body lists:
  contracts 1.11/11.6/3.5 consumed; the F6 surge (lane budget + cross-tenant 0) + ISS-D2-at-cell-scale greened with
  measured numbers; online-migration-under-load + restore-verify-at-cell-scale confirmed. Branch first if on
  default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P34 — E2E-1: the PR context pane (Issues' linked-issue resolves per-viewer, 0 leak)

- **BAND.** M5.
- **ROADMAP MILESTONE.** M5-I9, the E2E-1 slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M5-I9", the whole-system E2E wedge — Issues' participation
  in E2E-1).
- **DEPENDS-ON.** ISS-P17 (project() the unfurl reads through) + ISS-P33 (the hardened Issues surface). The M5
  cross-system E2E prompts that stand up the full cell with mock agents (testing-strategy §2). The
  Git/CI/Knowledge/Refs/Search/Notif prompts whose artifacts E2E-1 chains. The index places this in M5 after
  ISS-P33.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (the differentiator — work flows between tools);
    ../../external-insights/01-process-and-quality-doctrine.md §4 (actually try it — the E2E scenario chains
    mutations end-to-end), §3 (prove-it — the named green artifact).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/00-overview.md §1 (the cross-coupled
    posture); 03-events-contracts-and-glue.md (the cross-sub reflexes the scenario exercises).
  - Testing strategy: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-
    catalogue.md §E2E-1 (PR context pane: Issues' project resolves the linked issue per-viewer 0 leak; the live
    check-update within the freshness budget; a tombstone carries the root) + §3.4 (the named green artifacts) +
    README.md (the strategy).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M5-I9" (the E2E wedge — E2E-1).
- **DELIVERABLE (what to build + exactly where in the repo).** In the workspace E2E test suite + the myelin-issues
  crate (Issues' participation against a full cell with MOCK agents):
  - E2E-1 PR context pane (Git+CI+Issues+Knowledge+Refs+Search+Id+Notif): Issues' project() resolves the linked
    issue per-viewer with 0 leak; the live check-update is within the freshness budget; a tombstone carries the
    root for a confidential issue. The Issues-side assertions + fixtures wired into the workspace E2E harness.
  - FLOOR named: none (the floors were promoted/named in ISS-P32). State that the scenario runs with the MOCK
    agent runtime (the real-LLM runtime is the post-M5 swap, R-10); name it.
- **CONTRACTS TO IMPLEMENT.** No new contracts — this prompt EXERCISES the implemented contracts end-to-end (5.6
  project; 5.9 the CheckStatus freshness). Assert each behaves to its frozen shape under the chained scenario; a
  divergence is escalated and written down (code-wins-over-docs), not papered over.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - E2E-1 green (Issues' linked-issue resolves per-viewer 0 leak; the live check-update within the freshness
    budget; a confidential issue returns a tombstone carrying the root) — SCHED, the named E2E-1 green artifact.
- **TESTS (required).** The E2E-1 chained-mutation scenario itself is the test (it chains across subsystems, not
  single handlers — EI-01 §4). Issues-side unit assertions for the per-viewer resolve (0 leak) + the tombstone.
  The CDC pair for 5.6 re-asserted under the scenario. No new mutation floor (re-confirm the core modules' floors
  hold under the E2E load).
- **DEFINITION OF DONE.** E2E-1 emits its dated named green artifact with Issues' assertions passing (per-viewer 0
  leak; the freshness budget; the confidential tombstone); the scenario chains mutations end-to-end with mock
  agents; the 5.6 CDC pair re-asserts green; the coverage scanner is green; the work is committed. No scenario is
  marked green by weakening an assertion.
- **COMMIT.** Header: P-<NNN> M5: Issues E2E-1 (PR context pane). Body lists: E2E-1 greened (per-viewer 0 leak +
  freshness budget + confidential tombstone); the contracts exercised (5.6/5.9); the mock-runtime note (real-LLM is
  post-M5). Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### ISS-P35 — E2E-2: the agent-native flagship (CI-fail → triage → issue → chat → fix-PR)

- **BAND.** M5.
- **ROADMAP MILESTONE.** M5-I9, the E2E-2 slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M5-I9", the whole-system E2E wedge — Issues' participation
  in E2E-2, the agent-native flagship).
- **DEPENDS-ON.** ISS-P23 (the agent tool surface) + ISS-P24 (reserve/settle) + ISS-P27 (the governed transition +
  the X-1 guard) + ISS-P33 (the hardened surface). The M5 cross-system E2E prompts (the full cell + mock agents).
  The CI/Chat/Git prompts whose artifacts E2E-2 chains. The index places this after ISS-P34 within M5 — it is the
  differentiator proof.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (the differentiator — agents are first-class) + §1 (the agent-native flagship);
    ../../external-insights/01-process-and-quality-doctrine.md §4 (the E2E scenario chains mutations across a
    kill), §3 (prove-it — the named green artifact).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/00-overview.md §1 (Issues is the node
    where the triaged failure becomes a governed work item); 03-events-contracts-and-glue.md (the cross-sub
    reflexes).
  - Testing strategy: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-
    catalogue.md §E2E-2 (the agent-native flagship: 0 effect outside the ∩, 0 mutation before approval,
    exactly-once approval + the governed transition across a kill, reserve/settle balanced) + §3.4 + README.md.
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md (the agent ∩ the
    scenario rests on; the X-1 seam).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M5-I9" (E2E-2) + §0 (Issues on the E2E-2 flagship
    branch).
- **DELIVERABLE (what to build + exactly where in the repo).** In the workspace E2E test suite + the myelin-issues
  crate (Issues' participation against a full cell with MOCK agents):
  - E2E-2 CI-fail → triage agent → issue → chat → fix-PR (the agent-native flagship): Issues is the node where the
    triaged failure becomes a tracked, governed work item; 0 effect outside the ∩ (agent.policy ∩ delegation ∩
    tenant.policy); 0 mutation before approval; exactly-once approval + the governed transition ACROSS A KILL;
    reserve/settle balanced. The Issues-side assertions + fixtures wired into the workspace E2E harness.
  - FLOOR named: none. State that the scenario runs with the MOCK agent runtime (real-LLM is post-M5, R-10); name
    it.
- **CONTRACTS TO IMPLEMENT.** No new contracts — this prompt EXERCISES the implemented contracts end-to-end (8.2
  EffectApi; 9.4 the durable HITL signal; 5.9 the CheckStatus guard; 11.7 reserve/settle). Assert each behaves to
  its frozen shape under the chained scenario; a divergence is escalated and written down, not papered over.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - E2E-2 green (0 effect outside the ∩; 0 mutation before approval; exactly-once approval + governed transition
    across a kill; reserve/settle balanced) — SCHED, the named E2E-2 green artifact (the agent-native flagship).
- **TESTS (required).** The E2E-2 chained-mutation scenario itself is the test (it chains across subsystems across
  a kill). Issues-side unit assertions for the governed-transition-across-a-kill (exactly-once) + the
  0-pre-approval-mutation + reserve/settle balanced. The CDC pairs for 8.2 + 5.9 re-asserted under the scenario. No
  new mutation floor (re-confirm the EffectApi-gate + guard floors hold under the E2E load).
- **DEFINITION OF DONE.** E2E-2 emits its dated named green artifact with Issues' assertions passing (0 effect
  outside ∩; 0 mutation before approval; exactly-once approval + governed transition across a kill; reserve/settle
  balanced); the scenario chains mutations end-to-end with mock agents; the 8.2/5.9 CDC pairs re-assert green; the
  coverage scanner is green; the work is committed. No scenario is marked green by weakening an assertion.
- **COMMIT.** Header: P-<NNN> M5: Issues E2E-2 (agent-native flagship). Body lists: E2E-2 greened (0 effect outside
  ∩, exactly-once, reserve/settle balanced); the contracts exercised (8.2/9.4/5.9/11.7); the mock-runtime note
  (real-LLM is post-M5). Branch first if on default; do not push unless asked. End with the workspace
  Co-Authored-By trailer.

---

### ISS-P36 — E2E-3: spec-to-ship traceability (the spec→issue→PR→CI lineage per-viewer)

- **BAND.** M5.
- **ROADMAP MILESTONE.** M5-I9, the E2E-3 slice
  (planning/06-roadmaps/subsystems/issue-tracker.md §"M5-I9", the whole-system E2E wedge — Issues' participation
  in E2E-3).
- **DEPENDS-ON.** ISS-P17 (the lineage links + the Search projection) + ISS-P20 (the OLAP reindex parity) +
  ISS-P31 (the DSR/audit holders) + ISS-P33 (the hardened surface). The M5 cross-system E2E prompts (the full cell
  + mock agents). The Knowledge/Git/CI/Chat/Refs/Search/GDPR prompts whose artifacts E2E-3 chains. The index
  places this after ISS-P35 within M5.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (the differentiator — traceability across tools);
    ../../external-insights/01-process-and-quality-doctrine.md §4 (the E2E scenario chains mutations), §3
    (prove-it — cold-reindex == live; audit tamper detected).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/00-overview.md §1 (the cross-coupled
    posture).
  - Testing strategy: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-
    catalogue.md §E2E-3 (spec-to-ship: the spec→issue→PR→CI lineage per-viewer, cold-reindex == live, audit tamper
    detected) + §3.4 + README.md.
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M5-I9" (E2E-3).
- **DELIVERABLE (what to build + exactly where in the repo).** In the workspace E2E test suite + the myelin-issues
  crate (Issues' participation against a full cell with MOCK agents):
  - E2E-3 Spec-to-ship traceability (Knowledge+Issues+Git+CI+Chat+Refs+Search+GDPR+Id): the spec→issue→PR→CI
    lineage per-viewer; cold-reindex == live (the reindex-from-source parity); audit tamper detected. The
    Issues-side assertions + fixtures wired into the workspace E2E harness.
  - FLOOR named: none. State that the scenario runs with the MOCK agent runtime (real-LLM is post-M5, R-10); name
    it.
- **CONTRACTS TO IMPLEMENT.** No new contracts — this prompt EXERCISES the implemented contracts end-to-end (2.6
  reindex-from-source; 5.6 project; 10.4 DSR). Assert each behaves to its frozen shape under the chained scenario;
  a divergence is escalated and written down, not papered over.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - E2E-3 green (the spec→issue→PR→CI lineage per-viewer; cold-reindex == live; audit tamper detected) — SCHED,
    the named E2E-3 green artifact.
- **TESTS (required).** The E2E-3 chained-mutation scenario itself is the test. Issues-side unit assertions for the
  per-viewer lineage resolve + the cold-reindex parity (cold-reindex == live). The CDC pairs for 2.6 + 5.6
  re-asserted under the scenario. No new mutation floor (re-confirm the reindex + project floors hold under the E2E
  load).
- **DEFINITION OF DONE.** E2E-3 emits its dated named green artifact with Issues' assertions passing (per-viewer
  lineage; cold-reindex == live; audit tamper detected); the scenario chains mutations end-to-end with mock
  agents; the 2.6/5.6 CDC pairs re-assert green; the coverage scanner is green; the work is committed. Issues' M5
  exit contribution (§"M5-I9" exit gate) is complete with ISS-P32/P33's drills + ISS-P34/P35/P36's three E2E
  artifacts. No scenario is marked green by weakening an assertion.
- **COMMIT.** Header: P-<NNN> M5: Issues E2E-3 (spec-to-ship traceability). Body lists: E2E-3 greened (per-viewer
  lineage + cold-reindex == live + audit tamper detected); the contracts exercised (2.6/5.6/10.4); the
  mock-runtime note (real-LLM is post-M5). Branch first if on default; do not push unless asked. End with the
  workspace Co-Authored-By trailer.

---

### ISS-P37 — Dogfood: Myelin tracks its own issues (the switch test)

- **BAND.** M6.
- **ROADMAP MILESTONE.** M6-I10 (planning/06-roadmaps/subsystems/issue-tracker.md §"M6-I10", the Myelin roadmap/
  gap-report/scorecard as Myelin issues + the switch test). The done-bar for Issues as a product.
- **DEPENDS-ON.** ISS-P34/P35/P36 (the E2E wedge green — Issues carries its weight) + all prior Issues prompts
  (the full surface). The M5/M6 cross-system prompts that bring the platform to world-scale readiness + the
  self-hosting CI graph (master §2 M6). The index places this in M6 (you do not dogfood real team data onto a
  substrate whose restore-verify + DSAR fan-out are not green — master M6 entry dependency).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §5 (dogfooding — Myelin hosts itself); ../../external-insights/01-process-and-quality-
    doctrine.md §4 (the switch test — drive the real UI in a browser; a surface is done only when someone could
    move to it without hitting a wall the old tool didn't have — reached by DRIVING it, not reading the feature
    list), §1 (code-wins-over-docs — the truth-up pass).
  - Architecture: ../04-subsystem-architectures/issue-tracker/architecture/04-views-cli-and-api.md (the primary
    screens S1/S3/S5/S6/S9/S10/S13/S17/S19 + their empty/loading/error/permission/erased/agent-pending states);
    the design folder (information-architecture.md + user-flows.md + wireframes.md — the switch-test anchor).
  - Testing strategy: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-
    catalogue.md row ISS-D14 (the switch test — create→triage→plan→board→done without a manual; measured
    contrast/latency on the primary screens incl. the empty/loading/error/permission/erased/agent-pending states;
    driven in a browser).
  - Roadmap: planning/06-roadmaps/subsystems/issue-tracker.md §"M6-I10" (the work + the exit gate — ISS-D14; no
    later-band gate red) + §6 (the done-bar).
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
    measured contrast/latency on the primary screens, incl. the empty/loading/error/permission/erased/
    agent-pending states — DRIVEN IN A BROWSER, not read off a feature list) — SCHED, the switch-test pass + the
    measured contrast/latency are the green artifact.
  - No later-band gate red (the truth-up pass confirms every PROVEN Issues row rests on a dated green artifact;
    code-wins-over-docs) — the truth-up scorecard is the green artifact.
- **TESTS (required).** The switch-test browser run itself (driven, with measured contrast/latency on the primary
  screens). The truth-up pass cross-checking every Issues PROVEN row against its dated green artifact. No new unit
  floor — re-confirm the existing drills (ISS-D1..ISS-D13) are still green on the self-hosted platform (the dogfood
  CI graph runs them on Myelin's own commits).
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

## Coverage digest (every issue-tracker roadmap milestone → its finer prompt(s))

| Roadmap milestone (06-roadmaps/subsystems/issue-tracker.md) | Band | First-pass prompt | Finer prompt(s) |
|---|---|---|---|
| 3.0 pre-work (M1 slice — ReBAC fragment + holder tags) | M1 | ISS-P1 | ISS-P01 |
| 3.0 pre-work (M2 — myelin-query co-own) | M2 | ISS-P2 | ISS-P02 |
| 3.0 pre-work (M2 — issue.* tokens + initiative) | M2 | ISS-P2 | ISS-P03 |
| 3.0 pre-work (M2 — IndexSpec + notif-rule declares) | M2 | ISS-P2 | ISS-P04 |
| M4-I1 (issue-spine migrations) | M4 | ISS-P3 | ISS-P05 |
| M4-I1 (silent-data-loss-safe write path) | M4 | ISS-P3 | ISS-P06 |
| M4-I1 (pseudonymous columns + per-subject-DEK + holder reg) | M4 | ISS-P3 | ISS-P07 |
| M4-I1 (Hi/Lo human keys) | M4 | ISS-P4 | ISS-P08 |
| M4-I1 (order_key CAS reorder) | M4 | ISS-P4 | ISS-P09 |
| M4-I1 (content body + render round-trip) | M4 | ISS-P4 | ISS-P10 |
| M4-I2 (governance schemes + precedence + flexible fields) | M4 | ISS-P5 | ISS-P11 |
| M4-I2 (workflow FSM interpreter + QueryAst guards) | M4 | ISS-P5 | ISS-P12 |
| M4-I3 (SetExpr push-down lowered first) | M4 | ISS-P6 | ISS-P13 |
| M4-I3 (cost-bounding + three-tier escalation) | M4 | ISS-P6 | ISS-P14 |
| M4-I3 (projection-feeder consumer) | M4 | ISS-P6 | ISS-P15 |
| M4-I3 (co-equal ViewSpec views + design pass) | M4 | ISS-P7 | ISS-P16 |
| M4-I3 (Refs wiring + issue.* Search projection) | M4 | ISS-P7 | ISS-P17 |
| M4-I4 (incremental rollup consumer) | M4 | ISS-P8 | ISS-P18 |
| M4-I4 (time axis cycles/milestones + attachments) | M4 | ISS-P8 | ISS-P19 |
| M4-I4 (OLAP read store) | M4 | ISS-P8 | ISS-P20 |
| M4-I5 (import engine + ADF lossy-map) | M4 | ISS-P9 | ISS-P21 |
| M4-I5 (My Work over the ONE inbox + humanise) | M4 | ISS-P9 | ISS-P22 |
| M4-I6 (ToolDefs + EffectApi + mock forecast/triage) | M4 | ISS-P10 | ISS-P23 |
| M4-I6 (reserve/settle) | M4 | ISS-P10 | ISS-P24 |
| M4-I6 (stateful Trigger) | M4 | ISS-P10 | ISS-P25 |
| M4-I7 (SLA business-calendar engine) | M4 | ISS-P11 | ISS-P26 |
| M4-I7 (CheckStatus guard — closes X-1) | M4 | ISS-P11 | ISS-P27 |
| M4-I7 (cross-subsystem reflexes) | M4 | ISS-P11 | ISS-P28 |
| M4-I7 (governance admin views) | M4 | ISS-P11 | ISS-P29 |
| M4-I8 (real-time board sync) | M4 | ISS-P12 | ISS-P30 |
| M4-I8 (erasure-reaches-every-holder) | M4 | ISS-P12 | ISS-P31 |
| M5-I9 (measured floor follow-ons) | M5 | ISS-P13 | ISS-P32 |
| M5-I9 (world-scale hardening — F6 surge + benchmarks) | M5 | ISS-P13 | ISS-P33 |
| M5-I9 (E2E-1 PR context pane) | M5 | ISS-P14 | ISS-P34 |
| M5-I9 (E2E-2 agent-native flagship) | M5 | ISS-P14 | ISS-P35 |
| M5-I9 (E2E-3 spec-to-ship traceability) | M5 | ISS-P14 | ISS-P36 |
| M6-I10 (dogfood — switch test) | M6 | ISS-P15 | ISS-P37 |

**Prompt count: 15 (first pass) → 37 (finer). No milestone gap; every first-pass prompt's coverage is preserved,
now at single-deliverable granularity.**

**Drill coverage (every ISS drill → the finer prompt that greens it):** the outbox emit-iff-committed (SUB-D1/
BUS-D4 shape) → ISS-P06; ISS-D4 → ISS-P08; ISS-D5 → ISS-P09 (re-green at ISS-P32); ISS-D10 → ISS-P10; ISS-D12
(guard half) → ISS-P12, (complete) → ISS-P27; ISS-D3 → ISS-P13 (re-asserted at the unfurl boundary in ISS-P17;
re-run under surge in ISS-P33); ISS-D2 → ISS-P14 (at cell scale in ISS-P33); ISS-D1 → ISS-P16; ISS-D8(a/b) →
ISS-P18 (the OLAP-feed half of D8b in ISS-P20); ISS-D9(a/b/c) → ISS-P21; AG-D5/AG-D9 (applied) → ISS-P23; ISS-D7
→ ISS-P25; ISS-D6 → ISS-P26; ISS-D13 → ISS-P30; ISS-D11 → ISS-P31; the F6 surge + ISS-D2-at-cell-scale → ISS-P33;
ISS-D5-re-green + GA-D1/CP-D7/CP-D8 → ISS-P32; E2E-1 → ISS-P34; E2E-2 → ISS-P35; E2E-3 → ISS-P36; ISS-D14 →
ISS-P37. No ISS drill and no Issues milestone is left ungreened.

**Floor coverage (every floor → the finer prompt that ships it + the follow-on prompt/band):** CAS ranking
(ISS-P09) → move-CRDT (ISS-P32, M5); read-time rollup (ISS-P18) → materialised (ISS-P32, M5); linear forecast
(ISS-P23) → Monte-Carlo (ISS-P32, M5); GIN-default facet (ISS-P14) → projection-feeder generated index (ISS-P15;
measured promotion, OQ-C); PG-sharded (ISS-P05) → distributed-SQL (ISS-P32, M5); optimistic+resume sync (ISS-P30)
→ offline/local-first (post-M5); SLA business-calendar (ISS-P26) → long-SLA history-compaction (M5+, R-11); import
canonical core (ISS-P21) → permission-scheme mapping (M5+ legal, R-9); per-subject-DEK erasure (ISS-P07/ISS-P31) →
third-party residual [OPEN — LEGAL] (parallel legal, R-1); worklog tags (ISS-P01) → special-category ratification
(parallel legal, R-2); single-cell (ISS-P05..P31) → cross-cell rollup over the CrossCellPointer bridge (ISS-P32,
M5, R-7); mock agent runtime (ISS-P23) → real-LLM runtime (post-M5/execution, R-10). Every floor's pair is linked,
the gap visible.

**Contract coverage (the first-pass contract set, preserved across the splits):** 4.9 → ISS-P01; 13.3 → ISS-P02
(definitions) + ISS-P09/P12/P13/P14/P16 (executed); 2.9/2.1 → ISS-P03; 6.3/7.6 → ISS-P04 (declared) + ISS-P17/P22
(wired); 1.1–1.5/11.1/11.5/12.1/12.4/10.1 → ISS-P05; 2.1/2.2/2.3/2.5/4.2/4.6/4.10 → ISS-P06; 4.8/11.3/11.4/1.4 →
ISS-P07; 5.1 → ISS-P08; 2.2 (reorder) → ISS-P09; 13.1 → ISS-P10; 1.5/1.9/1.10 → ISS-P11; 13.3/3.4 → ISS-P12;
4.3/4.10 → ISS-P13; 6.1 → ISS-P14; 2.4/1.5 → ISS-P15; 13.3/4.3 → ISS-P16; 5.6/5.1/5.2/5.7/5.4/5.3/5.5/6.3/6.4 →
ISS-P17; 2.6/5.3/2.4 → ISS-P18; 11.2/5.5 → ISS-P19; 11.6/2.6 → ISS-P20; 13.2/1.11/2.2 → ISS-P21; 7.1/7.2/7.3/7.6
→ ISS-P22; 8.1/8.2/8.3/8.4/8.7/4.5/4.7 → ISS-P23; 11.7/9.5 → ISS-P24; 3.3/3.4/3.6/9.3/7.1 → ISS-P25; 9.3/9.4/7.5/
7.3 → ISS-P26; 5.9/4.2 → ISS-P27; 2.4/5.4/5.9 → ISS-P28; 4.4 → ISS-P29; 3.5/1.11 → ISS-P30; 4.8/10.1/10.8/10.9/
11.4/2.7 → ISS-P31; 12.6/10.4/3.5/11.6 → ISS-P32; 1.11/11.6/3.5 → ISS-P33; 5.6/5.9 (exercised) → ISS-P34; 8.2/9.4/
5.9/11.7 (exercised) → ISS-P35; 2.6/5.6/10.4 (exercised) → ISS-P36. Every contract row the first pass implemented
or consumed is carried by exactly the finer prompt that owns its deliverable; the contract-coverage scanner sees
the same union.
