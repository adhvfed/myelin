# Phase 7 — Prompt Ledger: Git Hosting & Code Review (the producer subsystem) — FINER GRANULARITY

> Phase 7-A refinement pass. Prompt count: 15 (first pass) -> 35 (this finer-grained set). Every bundled
> multi-deliverable prompt has been split into single-deliverable, clean-context, independently-committable
> units; coverage is preserved (every milestone, contract, drill, and floor the first pass covered remains,
> now at finer granularity), DEPENDS-ON is re-threaded across the new local ids, and each prompt is
> self-contained per the template in planning/07-prompts/00-ledger-overview.md §2.
>
> Phase: 07-prompts (per-system file, Phase 7-A). The complete ordered set of implementation prompts that
> operationalize the entire git-hosting roadmap (planning/06-roadmaps/subsystems/git-hosting.md, milestones
> pre-work M1/M2 + M3-G1..M3-G8 + M5-G9 + M6-G10) into clean-context, independently-committable coding tasks.
> Built to the template in planning/07-prompts/00-ledger-overview.md §2 (every field present, never implicit)
> and banded to planning/06-roadmaps/00-master-sequencing.md §2 (M0..M6, the gate invariant). Frozen
> architecture (this file OPERATIONALIZES, it does not redesign):
> planning/04-subsystem-architectures/git-hosting/architecture/ (00..07) + the build-to contracts in
> planning/05-refined-shared-systems-architecture/contract-index.md + 00-reconciliation-decisions.md
> (X-1/X-4/X-6/X-7, OQ-A/OQ-D/OQ-E/OQ-G/OQ-I/OQ-K). Drills:
> planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
> (GIT-D1..GIT-D11 + the shared families). Plain-text identifiers throughout (no backticks-as-emphasis).
> Markdown only; this file makes no commits. Date: 2026-06-19.
>
> The global P-NNN ids are assigned by the consolidated ledger index (Phase 7-B, 01-ledger-index.md) when these
> per-system prompts are interleaved into the single execution order. Here each prompt carries a stable local
> handle GIT-P<n> so its DEPENDS-ON edges are unambiguous before global numbering; the index rewrites GIT-P<n>
> to its P-NNN. Where a prompt depends on another system's prompt not yet numbered, it names that system's
> milestone (the index resolves it to the P-NNN).
>
> Git hosting is a PRODUCER subsystem on the critical path (master §2 M3, §3.1): its bulk lands in M3, with a
> freeze-so-dependents-compile slice in M1/M2 and world-scale follow-ons in M5 and the dogfood switch test in
> M6. Two seams sit on the spine: the AG-D4 sandbox-escape GATE (upstream, M2 — gates git's code-executing
> tools) and the X-1 Git↔CI CheckStatus seam (consumer built here in M3 against a synthetic producer, proven
> end-to-end with CI's producer at the M4 exit).
>
> Coverage (milestone → finer prompt ids): pre-work M1 → GIT-P1 + GIT-P2 + GIT-P3; pre-work M2 → GIT-P4 +
> GIT-P5 + GIT-P6 + GIT-P7; M3-G1 → GIT-P8 + GIT-P9 + GIT-P10 + GIT-P11 + GIT-P12; M3-G2 → GIT-P13 + GIT-P14 +
> GIT-P15; M3-G3 → GIT-P16 + GIT-P17 + GIT-P18 + GIT-P19; M3-G4 → GIT-P20 + GIT-P21 + GIT-P22 + GIT-P23 +
> GIT-P24; M3-G5 → GIT-P25 + GIT-P26; M3-G6 → GIT-P27 + GIT-P28; M3-G7 → GIT-P29 + GIT-P30; M3-G8 → GIT-P31 +
> GIT-P32; M5-G9 → GIT-P33 + GIT-P34 (GIT-P33 covers the floor follow-ons; GIT-P34 the surge family + E2E
> slices); M6-G10 → GIT-P35. Thirty-five prompts, no milestone gap. (See the coverage matrix at the foot for the
> per-drill / per-floor map.)

---

### GIT-P1 — Freeze the Git ReBAC namespace fragment so Identity's cell schema compiles

- **BAND.** M1.
- **ROADMAP MILESTONE.** Pre-work M1 (planning/06-roadmaps/subsystems/git-hosting.md §3.0 "Pre-work in M1/M2",
  the M1 freeze-so-dependents-compile slice — the ReBAC-fragment half).
- **DEPENDS-ON.** The M0 substrate prompts that lay down the Cargo workspace + the eight glue-crate skeletons +
  the twelve lints + the contract-coverage scanner (master §2 M0; substrate roadmap SUB-M0). The M1 Identity
  prompt that ships the ReBAC namespace engine (contract 4.9) into which fragments compile. The index places
  this alongside the Identity M1 work (Identity must accept the fragment).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md (always) §3 (name-your-floors); ../../external-insights/01-process-and-quality-doctrine.md
    §5 (the ratchet — an uncommitted gate is no gate), §1 (code-wins-over-docs).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md (the
    Git ReBAC fragment); 00-overview.md §1.2 (owns-vs-delegates, the Git ReBAC fragment row) + §4 (inherited
    non-negotiables 1,2,4).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §1 (the frozen
    ReBAC fragments — Git: ref-glob + CODEOWNERS-as-relations + approve_untrusted_ci + watcher).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 4.9 (per-subsystem ReBAC
    namespace fragment; the Git fragment frozen).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3.0 (the M1 ReBAC bullet) + §2 (upstream dep row
    4.9).
- **DELIVERABLE (what to build + exactly where in the repo).** In the git service implementation crate
  (myelin-git, the new subsystem crate under the workspace) plus its contribution into the shared cell schema:
  - The Git ReBAC namespace fragment submitted into the one cell schema Identity compiles (contract 4.9):
    ref-glob-scoped relations (protected_push over a ref-glob), CODEOWNERS-as-relations (a CODEOWNERS path
    pattern compiles to a reviewer relation), approve_untrusted_ci (the maintainer endorsement relation the X-1
    fork gate rides), and the watcher relation per watchable type (repo/pr). The fragment must COMPILE in the
    cell schema — that is the gate of this prompt, not a runtime property.
  - FLOOR named: none. This is a contract-fragment freeze, not a feature. State in the crate doc that no git
    feature ships here — only the relation shapes Identity compiles against — and name GIT-P13 (M3-G2) as where
    the fragment is wired LIVE.
- **CONTRACTS TO IMPLEMENT.** 4.9 the Git ReBAC fragment (owned — the fragment definition, compiled by
  Identity). Implement to the frozen shape; a needed change is a whole-workspace contract PR, escalated and
  written down, not a local divergence (code-wins-over-docs).
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The Git ReBAC fragment COMPILES in the shared cell schema Identity builds (a build-time gate, not a runtime
    drill) — CI, the compile is the green artifact. (No git-specific runtime drill here — §3.0 exit gate is
    compile-time + sign-off, not a runtime property.)
- **TESTS (required).** Unit tests that the fragment compiles and that each relation (protected_push,
  CODEOWNERS-reviewer, approve_untrusted_ci, watcher) is well-formed in the namespace. The provider/consumer
  CDC stub for contract-index row 4.9 (the Git fragment). No cargo-mutants floor (fragment declaration, not
  core logic) — state that.
- **DEFINITION OF DONE.** The fragment compiles in the cell schema; the CDC stub and unit tests pass; the
  contract-coverage scanner is green on row 4.9; the no-feature floor note is written naming GIT-P13; the work
  is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M1: Git ReBAC namespace fragment (compiles in the cell schema). Body lists:
  contract 4.9 (Git fragment) compiled; the no-feature floor named (GIT-P13 wires it live). Branch first if on
  default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P2 — Register the git.* event tokens in the Bus taxonomy seed

- **BAND.** M1.
- **ROADMAP MILESTONE.** Pre-work M1 (planning/06-roadmaps/subsystems/git-hosting.md §3.0, the git.*-token
  half).
- **DEPENDS-ON.** The M0 substrate prompts (workspace + glue crates + lints). The M1 Bus prompts that ship the
  event taxonomy seed + the §6.2 singular token table (2.9) and the EventEnvelope freeze (2.1). GIT-P1 is not a
  hard dependency (independent deliverable) but the index may place it after GIT-P1 since both touch the git
  crate.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3; ../../external-insights/01-process-and-quality-doctrine.md §5 (the ratchet).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md (the
    complete git.* taxonomy — the v1 token list); 00-overview.md §4 (inherited non-negotiables 1,2).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 2.9 (event taxonomy + token
    table grammar <subsystem>.<artifact_type>.<event_name>; git.* tokens), 2.1 (EventEnvelope the tokens align
    to).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3.0 (the M1 token bullet) + §4 (the 2.9 row:
    register M1, emit M3-G1/G3).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - Register the git.* event tokens in the Bus taxonomy seed (2.9): git.ref.updated, git.pr.opened/updated/
    merged/closed, git.review.requested/submitted, git.comment.created (the complete v1 list named in arch 03).
    Validate against the Bus §6.2 singular token table (git is the canonical subsystem token) — git REGISTERS,
    it does not author the grammar.
  - FLOOR named: none. State that the tokens are registered here but ACTUALLY EMITTED only from the outbox in
    GIT-P8 (git.ref.updated) and GIT-P16 (git.pr.*/git.review.*/git.comment.*) — name those follow-ons.
- **CONTRACTS TO IMPLEMENT.** 2.9 the git.* event tokens (owned — registered into the Bus seed). Implement to
  the frozen grammar; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The git.* tokens are present in the Bus taxonomy and parse under the §6.2 grammar (0 ungrammatical tokens) —
    CI, the parse is the green artifact.
- **TESTS (required).** Unit tests that each git.* token round-trips the §6.2 grammar. The CDC stub for
  contract-index row 2.9 (the git tokens). No cargo-mutants floor (registration) — state that.
- **DEFINITION OF DONE.** The git.* tokens are registered and grammatical (0 ungrammatical); the CDC stub and
  unit tests pass; the contract-coverage scanner is green on row 2.9; the emit-follow-on note (GIT-P8/GIT-P16)
  is written; the work is committed.
- **COMMIT.** Header: P-<NNN> M1: git.* event tokens registered in the Bus taxonomy. Body lists: contract 2.9
  (git tokens) registered + grammatical; the emit follow-ons named (GIT-P8 ref.updated, GIT-P16 pr/review/
  comment). Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P3 — Declare the git PersonalDataHolder H1 intent + apply the #[personal_data] tags (no-untagged lint green)

- **BAND.** M1.
- **ROADMAP MILESTONE.** Pre-work M1 (planning/06-roadmaps/subsystems/git-hosting.md §3.0, the holder-tags
  half).
- **DEPENDS-ON.** The M0 substrate prompts (workspace + the no-untagged-personal-data lint wired into CI). The
  M1 GDPR prompts that ship the #[personal_data] classify-derive + the no-untagged-personal-data lint
  (10.2/1.6) and the PersonalDataHolder trait (10.1). The index may place this after GIT-P1/GIT-P2 (same crate).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe by construction); ../../external-insights/01-process-and-quality-doctrine.md
    §1 (name-your-floors), §5 (the ratchet).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md (the
    holder tags); 01-tech-and-data-model.md §4 (the schema types the tags apply to — author_pseudonym etc.);
    00-overview.md §1.1 (git is holder H1).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 10.2 (the #[personal_data]
    classify-derive + the no-untagged-personal-data lint), 1.6 (the tenant-predicate + no-untagged-personal-data
    lints git compiles against), 10.1 (PersonalDataHolder H1 intent).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3.0 (the M1 holder bullet) + §2 (upstream dep rows
    10.1/10.2).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - Declare the git PersonalDataHolder H1 INTENT (the holder will be auto-registered by serve when the store
    opens in GIT-P8) and apply the #[personal_data(category, role, basis, retention, erasure, subject_locator)]
    tags on the (still-skeletal) git schema types — author_pseudonym / reviewer_pseudonym / pusher_pseudonym and
    the free-text body fields — so the no-untagged-personal-data lint is green from the first migration (GIT-P8).
  - FLOOR named: none. State in the crate doc that no git feature ships here — only the holder-intent + the PII
    tags — and name GIT-P8 (M3-G1) as the milestone where the holder is actually OPENED and registered.
- **CONTRACTS TO IMPLEMENT.** 10.2 the #[personal_data] tags (consumed — applied to git types so the lint is
  green), 10.1 PersonalDataHolder H1 intent (consumed — declared, opened in GIT-P8). Implement to the frozen
  shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The no-untagged-personal-data lint is GREEN on the git skeleton schema (0 untagged PII fields; the lint red
    on a deliberately-untagged fixture field, green on the tagged set) — CI, lint signal = 0 untagged fields.
- **TESTS (required).** The red+green fixture pair for the no-untagged-personal-data lint applied to a git type.
  Unit tests that each PII field carries a well-formed #[personal_data] tag. No cargo-mutants floor (tag
  application) — state that.
- **DEFINITION OF DONE.** The no-untagged-personal-data lint is green with both fixtures (0 untagged); the
  holder H1 intent is declared; the unit tests pass; the contract-coverage scanner is green on rows 10.1/10.2;
  the holder-opens-in-GIT-P8 note is written; the work is committed. No gate is greened by weakening a
  threshold.
- **COMMIT.** Header: P-<NNN> M1: git PersonalDataHolder H1 intent + #[personal_data] tags. Body lists:
  contracts 10.1 (H1 intent) declared, 10.2 tags applied; the no-untagged-personal-data lint greened with
  red+green fixtures; the holder-opens floor named (GIT-P8). Branch first if on default; do not push unless
  asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P4 — Register the git #sub mints with Refs (comment-/thread-/L<a>-L<b> kinds)

- **BAND.** M2.
- **ROADMAP MILESTONE.** Pre-work M2 (planning/06-roadmaps/subsystems/git-hosting.md §3.0, the #sub-mint half).
- **DEPENDS-ON.** GIT-P1 (the git crate + ReBAC fragment exist). The M2 Refs prompts that freeze the #sub
  grammar + the 4-step tombstone ladder (contract 5.7). The index places this in M2 alongside the reactive-layer
  freeze.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (the cross-artifact reference graph);
    ../../external-insights/01-process-and-quality-doctrine.md §7 (keep contracts coherent).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md (the
    #sub mints git owns); 00-overview.md §0.1 Δ7 (the ArtifactRef id grammar — pr/<repo>:<n>, commit/<repo>:<sha>
    are stored canonical roots).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-4 (the #sub
    grammar frozen).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 5.7 (the unified #sub grammar —
    git owns comment-/thread-/L<a>-L<b> mints).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3.0 (the M2 #sub bullet) + §2 (upstream dep row
    5.7).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - Register with Refs the #sub kinds git owns (5.7): comment-<id>, thread-<id>, L<a>-L<b> (and the commit/pr/
    review canonical roots). git mints stable opaque ids; Refs stores the full sub-URN + the stripped root. The
    mint functions are stubbed-but-typed here (the comment/thread resolver lands in GIT-P16/GIT-P18; the
    L<a>-L<b> 4-state resolver lands in GIT-P24); the REGISTRATION (kinds declared to Refs) is the deliverable.
  - FLOOR named: none. State that only the kind registration ships here; the resolvers are named follow-ons
    (GIT-P18 comment/thread, GIT-P24 the L-range 4-state resolver).
- **CONTRACTS TO IMPLEMENT.** 5.7 the git #sub mints (owned — registered with Refs; the resolver is a follow-on).
  Implement to the frozen grammar; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The #sub kinds register with Refs and the registration is accepted; the mints produce grammatical sub-URNs
    (0 ungrammatical) — CI, build-time gate.
- **TESTS (required).** Unit tests that the #sub mints produce grammatical sub-URNs round-tripping the 5.7
  grammar. The CDC stub for row 5.7 (git's owned mint half). No cargo-mutants floor (registration) — state that.
- **DEFINITION OF DONE.** The #sub kind registrations compile and are accepted; the mints are grammatical; the
  CDC + unit tests pass; the contract-coverage scanner is green on row 5.7; the resolver follow-ons are named;
  the work is committed.
- **COMMIT.** Header: P-<NNN> M2: git #sub mints registered with Refs. Body lists: 5.7 (comment-/thread-/
  L<a>-L<b>) registered; the resolver follow-ons named (GIT-P18, GIT-P24). Branch first if on default; do not
  push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P5 — Register git's declare_indexable code-projection spec with Search

- **BAND.** M2.
- **ROADMAP MILESTONE.** Pre-work M2 (planning/06-roadmaps/subsystems/git-hosting.md §3.0, the
  declare_indexable half).
- **DEPENDS-ON.** GIT-P1 (the git crate exists). The M2 Search prompt that ships declare_indexable (6.3). The
  index places this in M2 alongside the Search freeze.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (search references any artifact);
    ../../external-insights/01-process-and-quality-doctrine.md §7 (keep contracts coherent).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md (the
    declare_indexable projection spec); 00-overview.md §1.1 (git owns what to index).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 6.3 (declare_indexable — the
    git.* code projection spec).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3.0 (the M2 declare_indexable bullet) + §2
    (upstream dep row 6.3).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - Register git's declare_indexable projection spec with Search (6.3): the git.* code projection shape
    {path, language, symbols (camel/snake split), literals, commit message, text}, ft_fields, struct_fields,
    acl_object_type=repo. The emitter lands in GIT-P25; the SPEC registration is the deliverable here.
  - FLOOR named: none. State that only the spec registration ships here; the emitter is the GIT-P25 follow-on.
- **CONTRACTS TO IMPLEMENT.** 6.3 declare_indexable (owned — the git projection spec registered with Search).
  Implement to the frozen shape; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The declare_indexable spec registers with Search and the registration is accepted; the spec serializes to
    the 6.3 shape (0 schema mismatches) — CI, build-time gate.
- **TESTS (required).** Unit tests that the declare_indexable spec serializes to the 6.3 shape. The CDC stub for
  row 6.3 (git's owned spec half). No cargo-mutants floor (registration) — state that.
- **DEFINITION OF DONE.** The declare_indexable spec registers and is accepted; the serialization matches 6.3;
  the CDC + unit tests pass; the contract-coverage scanner is green on row 6.3; the emitter follow-on (GIT-P25)
  is named; the work is committed.
- **COMMIT.** Header: P-<NNN> M2: git declare_indexable code-projection spec registered with Search. Body lists:
  6.3 registered; the emitter follow-on named (GIT-P25). Branch first if on default; do not push unless asked.
  End with the workspace Co-Authored-By trailer.

---

### GIT-P6 — Declare the X-1 CheckStatus consumer contract (the compiling, not-yet-live seam module)

- **BAND.** M2.
- **ROADMAP MILESTONE.** Pre-work M2 (planning/06-roadmaps/subsystems/git-hosting.md §3.0, the X-1 consumer
  declaration half).
- **DEPENDS-ON.** GIT-P1 (the git crate exists). The M2 reconciliation that froze the 5.9 CheckStatus shape.
  The index places this in M2 alongside the reactive-layer freeze.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (the merge gate); ../../external-insights/01-process-and-quality-doctrine.md §7 (keep
    contracts coherent — reconcile the cross-component seam at the plan layer before either side ships).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md (the
    CheckStatus consumer contract); 00-overview.md §0.1 Δ1/Δ2/Δ3 (the frozen CheckStatus fact, the ci.result
    rollup, untrusted-fork neutral-until-endorsed).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-1 (the
    CheckStatus seam declared M2, consumer half Git M3).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 5.9 (the Git↔CI CheckStatus
    seam — the projection-table schema keyed (commit_oid, context) + the run_attempt supersession rule + the
    required-set policy shape, written against the M2-frozen shape, ready to build in M3).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3.0 (the M2 X-1 bullet) + §2 (the seam
    frozen-but-not-live note) + §5 (the seam-floor).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate (a contract module):
  - Declare the X-1 CheckStatus consumer contract (5.9): write the check_status projection-table schema keyed
    (commit_oid, context) + the run_attempt monotonic supersession rule + the required-set policy shape, against
    the M2-frozen CheckStatus fact — as a written, compiling contract module (no live consumer yet; the consumer
    + gate land in GIT-P20/GIT-P20). This is the seam-floor named in §5 of the roadmap: built in M3 against a
    synthetic emitter, live end-to-end at M4.
  - FLOOR named: the X-1 seam-floor — the CheckStatus consumer is declared here and built against a synthetic
    ci.check.updated emitter in GIT-P20, the real CI producer wiring is the M4 co-gate (GIT-D10/CI-D8
    end-to-end). Name it in the contract-module doc.
- **CONTRACTS TO IMPLEMENT.** 5.9 the CheckStatus consumer contract (owned — declared/compiling, not yet live).
  Implement to the frozen shape; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The check_status projection-table schema + the run_attempt supersession rule + the required-set policy shape
    COMPILE against the M2-frozen 5.9 shape (build-time; the consumer goes live in GIT-P20) — CI.
- **TESTS (required).** A compile test for the check_status schema module against the frozen 5.9 shape. The CDC
  stub for row 5.9 (git's owned consumer half). No cargo-mutants floor (schema declaration) — state that.
- **DEFINITION OF DONE.** The check_status contract module compiles against the frozen 5.9 shape; the CDC +
  compile tests pass; the contract-coverage scanner is green on row 5.9; the X-1 seam-floor is named in the
  doc (GIT-P20 builds against synthetic, M4 goes live); the work is committed.
- **COMMIT.** Header: P-<NNN> M2: X-1 CheckStatus consumer contract declared (compiling). Body lists: 5.9
  consumer contract declared/compiling; the X-1 seam-floor named (GIT-P20 synthetic, M4 live). Branch first if
  on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P7 — The design-system pass + the X-1 affordances, with fork-trust UX human sign-off

- **BAND.** M2.
- **ROADMAP MILESTONE.** Pre-work M2-design (planning/06-roadmaps/subsystems/git-hosting.md §3.0, the
  M2-design / OQ-12 bullet).
- **DEPENDS-ON.** GIT-P6 (the X-1 affordances to sketch are anchored on the declared CheckStatus seam). The
  index places this in M2 alongside the reactive-layer freeze; the fork-trust sign-off blocks GIT-P31 (the Web
  UI).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (design comes before implementation for anything with a frontend — no frontend code
    without a reviewed design sketch); ../../external-insights/05-ux-and-design.md (the design-language bar);
    ../../external-insights/01-process-and-quality-doctrine.md §8 (the human sign-off is the bottleneck —
    fork-trust UX is decision-shaped: sketch + sign-off, do not build autonomously).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/04-views-cli-and-api.md §2.2 (the X-1
    affordances — fork-trust badge, checks panel, merge-queue affordances); the design/ folder (the present
    IA/flows/wireframes the pass refines).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-12 (the design
    pass).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3.0 (the M2-design bullet).
- **DELIVERABLE (what to build + exactly where in the repo).** In the design record (design/):
  - The design-system pass (pre-frontend, OQ-12): a visual/token-level pass over the present IA/flows/wireframes
    in design/, INCLUDING the new X-1 affordances (the fork-trust badge, the checks panel, the merge-queue
    affordances). The fork-trust UX is decision-shaped (EI-01 §8): produce the sketch and PAUSE for human
    sign-off; do not build the UI here (the UI lands in GIT-P31). Record the sign-off in design/, dated.
  - FLOOR named: none. State that this is the design sketch + sign-off only; the frontend lands in GIT-P31.
- **CONTRACTS TO IMPLEMENT.** None (a design-system pass, not a contract). State that.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The design pass is REVIEWED-AND-SIGNED-OFF in design/, with the fork-trust UX explicitly approved (the
    sign-off is the green artifact; EI-01 §8 — no frontend code without it) — sign-off recorded, dated.
- **TESTS (required).** None (a design pass). State that the proof is the dated sign-off artifact in design/,
  not a test.
- **DEFINITION OF DONE.** The design pass is signed off (dated) with the decision-shaped fork-trust UX approved;
  the X-1 affordances (fork-trust badge, checks panel, merge-queue) are sketched; the frontend-lands-in-GIT-P31
  note is written; the work is committed. The sign-off is real, not assumed.
- **COMMIT.** Header: P-<NNN> M2: git design-system pass + X-1 affordances (fork-trust UX signed off). Body
  lists: the design pass signed off (fork-trust UX approved, dated); the frontend follow-on named (GIT-P31).
  Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P8 — The GitCore layered seam (canonical git for the wire, gix in-process for read/diff/blame)

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G1 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G1", the GitCore-seam
  slice — the substrate the receive-pack path and the in-process read path both stand on).
- **DEPENDS-ON.** GIT-P3 (the holder H1 intent + the git crate). The M0 substrate prompts (serve(AppSpec), the
  no-host-exec lint). The index places this first in the M3 git band (the receive-pack path GIT-P9 builds on it).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale; name-your-floors);
    ../../external-insights/04-hard-problems.md §3 (world-scale git — canonical git for the server side, gix has
    no server-side receive-pack — the TE-8 Stage-1 position).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/01-tech-and-data-model.md §1-§2 (the
    GitCore layered seam — canonical git for the wire, gix in-process); 02-internals-and-algorithms.md §2 (the
    sandboxed canonical-git path); 00-overview.md §2 (B) (the serving tier), §4 (inherited non-negotiable 8).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G1 (the GitCore-seam bullet — "sandboxed
    canonical git for wire serving + maintenance; gix in-process for read/diff/blame") + the OQ-1 gix-ward
    spike note (a named M5+ spike, NOT in scope here).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - The GitCore layered seam (arch 01 §2, 02 §2): the sandboxed canonical git path for wire serving
    (upload-pack/receive-pack/ls-refs) + maintenance, and the gix (libgit2 fallback) in-process path for
    read/diff/blame. The TE-8 Stage-1 position — do NOT attempt a pure-gix server (gix has no server-side
    receive-pack). The seam is a trait with the two backends behind it; the wire ops route to canonical git,
    the read ops route to gix.
  - FLOOR named: the gix-ward server-side migration is OQ-1 — a NAMED M5+ spike (GIT-P33), gated on a
    capability-matrix + protocol-compat + sandbox-escape re-drill, NOT a guaranteed deliverable. Name it.
- **CONTRACTS TO IMPLEMENT.** None new (the GitCore seam is internal substrate; the wire contracts land in
  GIT-P9/GIT-P13). State that this is the internal seam the later prompts build on.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The GitCore seam compiles and routes wire ops to canonical git, read ops to gix (a build-time + smoke gate):
    a smoke test clones a fixture repo via the canonical path and diffs/blames it via the gix path; both succeed
    (0 routing errors) — CI.
  - The no-host-exec lint is green on the seam (the canonical-git path runs sandboxed, no host exec bypass) — CI.
- **TESTS (required).** Unit tests for the seam routing (wire op → canonical git; read op → gix). A smoke test:
  clone a fixture repo (canonical path) → diff + blame it (gix path). No CDC pair (internal seam, no contract
  row). The GitCore seam is mandatory-core (the wire path stands on it): state the cargo-mutants mutation-score
  floor for the routing module and meet it.
- **DEFINITION OF DONE.** The GitCore seam compiles and routes correctly (smoke green, 0 routing errors); the
  no-host-exec lint is green; the unit + smoke tests pass; the routing mutation score is measured; the OQ-1
  spike floor is named with its follow-on (GIT-P33); the work is committed.
- **COMMIT.** Header: P-<NNN> M3: the GitCore layered seam (canonical git wire + gix in-process). Body lists:
  the seam built; no-host-exec lint green; the routing mutation score measured; OQ-1 gix-ward spike named
  (GIT-P33). Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P9 — Receive-pack → one-tx ref-CAS + outbox (the silent-data-loss floor, GIT-D9)

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G1 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G1", the receive-pack +
  emit-iff-committed half — the Tier-1 silent-data-loss floor).
- **DEPENDS-ON.** GIT-P8 (the GitCore seam — the canonical receive-pack path), GIT-P3 (the holder H1 intent).
  The M0 outbox prompts (contracts 2.2-2.5) + the EventEnvelope freeze (2.1). The M1 Storage prompts that ship
  the OLTP tier + RLS + encrypted columns + the outbox (11.1), the KMS hierarchy + per-subject DEK (11.3/11.4),
  and — the hard gate — backup/restore + restore-verify STOR-D1 (11.5). GIT-P2 (the git.ref.updated token
  registered). The index places this after STOR-D1 is GREEN: git does not write real data over a red
  restore-verify (master M1→M2 gate, the silent-data-loss floor).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors); ../../external-insights/01-process-and-quality-doctrine.md §2
    (order-by-non-negotiability — silent data loss outranks every feature), §3 (prove-it: a property is not real
    until a drill forces the failure and observability watches it survive).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/02-internals-and-algorithms.md §2 (the
    sandboxed receive-pack → quarantine → in-process Rust policy → ref-CAS + outbox in ONE tx), §3 (the
    reftable-on-OLTP ref store, the ref as the aggregate); 01-tech-and-data-model.md §4 (the git object tier,
    reftable-on-OLTP); 00-overview.md §2 (B) + §4 (inherited non-negotiables 1,2,4,8).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 2.2/2.3 (OutboxTx::emit + the
    outbox table), 2.1 (EventEnvelope), 11.1 (OLTP tier + RLS + encrypted columns + the outbox), 11.5
    (backup/restore + restore-verify, the cross-seam cursor), 10.1 (PersonalDataHolder H1 — the store
    auto-registers here via serve).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G1 + §1 (non-negotiability item 1) + §2
    (★ STOR-D1 must be green) + §4 (the 2.2/2.3 row).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row GIT-D9 (crash mid-push →
    emit-iff-committed).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - The receive-pack path (arch 02 §2): sandboxed git receive-pack ingests the pack into a QUARANTINE;
    in-process Rust evaluates branch-protection / secret-scan / size rules — REJECT BEFORE THE REF MOVES; then
    OUR code does the ref CAS + the outbox insert in ONE DB TRANSACTION (BUS-2, emit-iff-committed). On abort,
    quarantine objects are discarded (never promoted).
  - The reftable-on-OLTP ref store (arch 02 §3): the ref-update transaction is the linearisation point; the
    aggregate for git.ref.updated is the REF. (The per-ref ordering AT PUSH QPS / hot-ref burst behaviour and
    GIT-D1 are the GIT-P10 follow-on — here the single-push correctness + emit-iff-committed is proven.)
  - The repo / fork-network / quarantine schema + the control-plane DB (one DB per service, RLS, per-tenant
    envelope-encrypted, per-subject DEK for free-text bodies); the store auto-registers as PersonalDataHolder H1
    (via serve, contract 1.4). Emit git.ref.updated via the outbox ONLY (no-raw-publish).
  - FLOOR named: none new here (the local-disk pack floor is GIT-P11; per-ref burst ordering is GIT-P10). State
    that GIT-P10 hardens the per-ref-order-under-burst property and GIT-P11 lands the pack tier.
- **CONTRACTS TO IMPLEMENT.** 2.2/2.3 OutboxTx::emit + the per-ref aggregate (owned — the receive-pack →
  ref-CAS → outbox emit in one tx). 2.9 git.ref.updated emission (owned). 10.1 PersonalDataHolder H1
  registration (consumed — the store auto-registers; locate/export/erase land in GIT-P29). Implement to the
  frozen shapes; escalate any needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - GIT-D9 (CI): crash the serving tier mid-push (after policy, before AND after commit) → git.ref.updated is
    emitted IFF the ref move committed; 0 ghost, 0 lost; quarantine objects discarded on abort. Green artifact:
    the outbox emit-iff-committed signal (contract 1.8) shows 0 ghost / 0 lost across the kill. PERMANENT-gate
    family (STOR-D1/D2 re-run on every store-touching change — say so).
  - The no-raw-publish + tenant-predicate + residency-pin lints green on the git schema — CI.
- **TESTS (required).** Unit tests for the quarantine→policy→ref-CAS→outbox state machine (reject-before-ref-move;
  abort-discards-quarantine). An END-TO-END chained test (EI-01 §4): push → policy reject path; push → commit →
  kill before publish → recover → assert emit-iff-committed. The provider/consumer CDC pair for rows 2.2/2.3.
  The GIT-D9 drill scenario on the failure-injection harness. The ref-store + receive-pack path is
  mandatory-core: state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** The receive-pack → one-tx ref-CAS+outbox path exists and compiles; GIT-D9 emits its
  dated green artifact (0 ghost / 0 lost); the lints are green; the unit + chained-e2e + CDC + drill tests pass;
  the store is registered as holder H1; the GIT-P10/GIT-P11 follow-ons are named; the work is committed. A red
  GIT-D9 does NOT become green by weakening the assertion — it becomes a dated claimed-not-proven scorecard row
  and blocks M4.
- **COMMIT.** Header: P-<NNN> M3: receive-pack → one-tx ref-CAS + outbox (the silent-data-loss floor). Body
  lists: contracts 2.2/2.3/2.9/10.1 implemented; GIT-D9 greened (0 ghost / 0 lost, measured); the ref-store
  mutation score measured; the GIT-P10 (burst order) + GIT-P11 (pack tier) follow-ons named. Branch first if on
  default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P10 — Per-ref aggregate ordering at push QPS (the hot-ref burst, GIT-D1)

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G1 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G1", the
  per-ref-ordering-under-burst half — GIT-D1).
- **DEPENDS-ON.** GIT-P9 (the receive-pack → one-tx ref-CAS + outbox path this proves ordered under burst). The
  M0 outbox prompts (the UNIQUE(aggregate, seq) per-aggregate ordering, 2.3) + the failure-injection harness
  (the 1×/10×/30× load generator). The index places this directly after GIT-P9 (a sibling in M3-G1; split out
  because the burst-ordering property has its own scheduled drill, GIT-D1).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it —
    the failure-injection harness, the 1×/10×/30× surge; observability is part of the pass).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/02-internals-and-algorithms.md §3 (the
    reftable-on-OLTP ref store — per-ref ordering at push QPS via the outbox UNIQUE(aggregate, seq); refs fan
    out parallel); 00-overview.md §2 (B).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 2.3 (the outbox table,
    per-aggregate ordering at push QPS — UNIQUE(aggregate, seq)), 1.8 (the per-aggregate-order + outbox-depth
    survival signal).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G1 (the GIT-D1 bullet — "burst force-pushes +
    rapid pushes to one hot ref → git.ref.updated in push order per ref; refs fan out parallel").
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row GIT-D1 (burst force-push per-ref
    order).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - Harden the per-ref aggregate ordering on the GIT-P9 outbox path: per-ref ordering at push QPS via the outbox
    UNIQUE(aggregate, seq); refs fan out PARALLEL (the aggregate is the ref, so different refs are independent);
    the outbox order == the ref-update order per ref. Concurrency control on the hot-ref CAS so rapid pushes
    serialise per ref without serialising the whole repo.
  - FLOOR named: none. (World-scale concurrent-merge linearizability under failover is GIT-D5/GIT-P33 — name it
    as the world-scale follow-on of the ordering property.)
- **CONTRACTS TO IMPLEMENT.** 2.3 the per-ref aggregate ordering (owned — the UNIQUE(aggregate, seq) ordering at
  push QPS). Implement to the frozen shape; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - GIT-D1 (SCHED): burst force-pushes + rapid pushes to one hot ref at 1×/10×/30× → git.ref.updated in PUSH
    ORDER PER REF; refs fan out parallel; 0 lost/ghost; outbox order == ref-update order. Green artifact: the
    per-aggregate-order + outbox-depth signal (contract 1.8).
- **TESTS (required).** Unit tests for the hot-ref CAS concurrency control (rapid same-ref pushes serialise;
  different-ref pushes proceed parallel). The GIT-D1 drill scenario on the failure-injection harness (1×/10×/30×
  to one hot ref). The CDC pair for row 2.3 (the per-ref aggregate ordering). The ordering path is
  mandatory-core: state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** Per-ref ordering holds under burst; GIT-D1 emits its dated green artifact (push order
  per ref, 0 lost/ghost, outbox order == ref-update order); the unit + drill + CDC tests pass; the ordering
  mutation score is measured; the GIT-D5 world-scale follow-on is named (GIT-P33); the work is committed.
- **COMMIT.** Header: P-<NNN> M3: per-ref aggregate ordering at push QPS (GIT-D1). Body lists: contract 2.3
  implemented; GIT-D1 greened (per-ref order, 0 lost/ghost, measured); the ordering mutation score measured;
  the GIT-D5 follow-on named (GIT-P33). Branch first if on default; do not push unless asked. End with the
  workspace Co-Authored-By trailer.

---

### GIT-P11 — Pack/delta storage on the local-NVMe BlobStore floor (relocatable, never node-pinned)

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G1 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G1", the pack-storage
  floor half — GF-1/GF-2/GF-2b).
- **DEPENDS-ON.** GIT-P9 (the object store + receive-pack the packs back). The M1 Storage prompts that ship the
  content-addressed BlobStore fs-floor (11.2). The index places this after GIT-P9 (a sibling in M3-G1; split out
  because the pack tier + maintenance + the three world-scale floors are a separately-committable slice).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale; name-your-floors);
    ../../external-insights/04-hard-problems.md §3 (world-scale git — authoritative bytes on local disk first,
    object-backed is the named follow-on).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/01-tech-and-data-model.md §4 (the git
    object tier — packs, commit-graph, bitmaps, MIDX); 02-internals-and-algorithms.md §2 (pack maintenance);
    00-overview.md §4 (inherited non-negotiable 8 — repos relocatable, never node-pinned, STOR-5).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §8 (the BlobStore
    fs↔object one-line swap, the local-NVMe floor).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 11.2 (BlobStore content-addressed,
    fs-backed floor — the pack tier).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G1 (the pack-floor bullet) + §4 (the 11.2 row)
    + §5 (the floors register — GF-1/GF-2/GF-2b/GF-4).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - Pack/delta storage on the local-NVMe floor behind the BlobStore trait (GF-1): repos RELOCATABLE, never
    node-pinned (STOR-5). Commit-graph + reachability bitmaps + MIDX maintenance. The fs↔object one-line swap
    point is the BlobStore trait (the object-backed impl is the GIT-P33 follow-on).
  - FLOOR named: the local-disk pack floor (GF-1 — object-backed packs follow in GIT-P33) + the single-cell
    primary+quorum replication floor (GF-2 — cross-cell follows in GIT-P33) + SHA-1+sha1dc default,
    hash-agnostic model (GF-2b — SHA-256 flip follows in GIT-P33) + the large-but-normal-monorepo floor (GF-4 —
    Mononoke-class backend is M5+, triggered by the GIT-D4 ceiling). Name each in the crate doc with its
    follow-on prompt.
- **CONTRACTS TO IMPLEMENT.** 11.2 BlobStore pack tier (consumed — local-NVMe floor; the fs↔object swap point).
  Implement to the frozen shape; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - A clone/fetch served from the local-NVMe pack tier round-trips byte-identical to the receive-pack input
    (0 corruption; commit-graph + bitmaps + MIDX consistent) — CI, the pack-round-trip signal.
  - The residency-pin lint green on the pack placement (repos relocatable, never node-pinned) — CI.
- **TESTS (required).** Unit tests for the pack read/write through the BlobStore trait + the commit-graph/
  bitmap/MIDX maintenance. A round-trip test: receive-pack → store → clone → byte-identical. The CDC pair for
  row 11.2 (git's consumer half of the pack tier). The pack path is mandatory-core: state the cargo-mutants
  mutation-score floor and meet it.
- **DEFINITION OF DONE.** The pack tier on the local-NVMe floor exists; the clone round-trip is byte-identical
  (0 corruption); the residency-pin lint is green; the unit + round-trip + CDC tests pass; the pack mutation
  score is measured; the four floors (GF-1/GF-2/GF-2b/GF-4) are named with their follow-ons (GIT-P33); the work
  is committed.
- **COMMIT.** Header: P-<NNN> M3: pack/delta storage on the local-NVMe BlobStore floor. Body lists: contract
  11.2 implemented; the clone round-trip byte-identical (measured); the residency-pin lint green; the pack
  mutation score measured; floors GF-1/GF-2/GF-2b/GF-4 named with follow-on GIT-P33. Branch first if on default;
  do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P12 — Pseudonymous-by-default commit identities (the erasure-vs-immutability data-model gate)

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G1 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G1", the
  pseudonymous-by-default half — GIT-1, the data-model gate that MUST be decided and enforced before the git
  data model is fixed, EI-04 §1).
- **DEPENDS-ON.** GIT-P9 (the receive-pack policy engine + the schema this prompt pins the pseudonym columns
  into; this prompt is sequenced in the SAME band immediately after GIT-P9 because pseudonymity gates the data
  model — it cannot be bolted on), GIT-P3 (the #[personal_data] tags on the pseudonym columns). The M1 Identity
  prompt that ships resolve_pseudonym/erase + the pseudonym grammar <pseudonym>@<tenant>.noreply (4.8). The
  index places this directly after GIT-P9/GIT-P11 and before any feature prompt.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe by construction; name-your-floors);
    ../../external-insights/04-hard-problems.md §1 (erasure vs immutability — the immutable bytes never bake in
    erasable PII in the first place; this MUST be decided before the data model freezes);
    ../../external-insights/01-process-and-quality-doctrine.md §2 (order-by-non-negotiability).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/05-hard-problems.md §HP-7 (the erasure
    posture, the pseudonymous-commit mechanism); 01-tech-and-data-model.md §4 (the author_pseudonym /
    reviewer_pseudonym / pusher_pseudonym columns); 00-overview.md §1.1 (pseudonymous-commit-by-default GIT-1 as
    a commit-time prerequisite) + §0.1 Δ6 (the residual stated by reference to 10.9, not restated).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-7 (the ONE
    free-text/immutable erasure posture — pseudonymous-by-default + the residual instantiated by reference),
    §1 (the pseudonym grammar pinned).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 4.8 (resolve_pseudonym/erase +
    the frozen <pseudonym>@<tenant>.noreply grammar; Git commits pseudonymous-by-default), 10.9 (the ONE erasure
    posture — instantiate by reference, never restate).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G1 (the pseudonymous-by-default bullet) + §1
    (non-negotiability item 2) + §5 (GF-7 floor) + the OQ-10/R-8 spike note (enforcement mode:
    client-cooperative sha-stable vs server-side rewrite-at-push — the PROPERTY is decided, the default is the
    call).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row GIT-D2 (the GIT-1 half asserted
    here; completed at GIT-P29).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - The schema stores author_pseudonym / reviewer_pseudonym / pusher_pseudonym (NEVER name/email); the
    person↔pseudonym map is Identity's erasable record (4.8 — git holds only the opaque pseudonym +
    <pseudonym>@<tenant>.noreply in the immutable bytes).
  - Push policy enforces the pseudonym at receive-pack (the GIT-P9 in-process policy engine gains the
    pseudonymity rule): a commit whose author/committer identity is not the principal's tenant pseudonym is
    REJECTED before the ref moves (or rewritten at push — pin the enforcement default per the OQ-10/R-8 decision:
    the PROPERTY "immutable bytes carry only the opaque pseudonym" is decided here; record the chosen default and
    its rationale in the crate doc).
  - The residual lawful-basis posture is instantiated BY REFERENCE to the ONE platform posture (10.9 / recon
    §X-7) — NOT restated as a git-local statement (arch 00 §0.1 Δ6). The [OPEN — LEGAL] Art. 17 ratification is
    R-7 (Legal/DPO, parallel — not a code gate).
  - FLOOR named: GF-7 — the structural mechanism (pseudonymous-by-default + per-subject DEK shred +
    history-rewrite) ships across GIT-P9/GIT-P12/GIT-P29; the lawful-basis residual is the ONE posture's
    [OPEN — LEGAL] statement (R-7, parallel-legal, not a code gate). Name it.
- **CONTRACTS TO IMPLEMENT.** 4.8 the pseudonym consumer (owned — git enforces pseudonymous-by-default and
  stores only the opaque pseudonym; the map + erase are Identity's). 10.9 the ONE posture (consumed by
  reference — instantiated, never restated). Implement to the frozen grammar; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - GIT-D2 (SCHED, the GIT-1 half here): a stored commit carries author_pseudonym in the
    <pseudonym>@<tenant>.noreply form, never a name/email (0 name/email bytes in newly-stored commit identity
    fields). Green artifact: a scan of newly-stored commit identities shows 0 cleartext PII. (The full
    erase-reaches-every-holder GIT-D2 completes at GIT-P29.)
  - The pseudonymity push-policy rule rejects a non-pseudonymous identity at receive-pack (the policy denies
    before the ref moves; 0 cleartext-PII commits admitted) — CI.
  - The no-untagged-personal-data lint green on the pseudonym columns (tagged correctly) — CI.
- **TESTS (required).** Unit tests for the push-policy pseudonymity rule (reject/rewrite a non-pseudonymous
  identity; accept a pseudonymous one). A test that stored commit identity fields are the
  <pseudonym>@<tenant>.noreply form. The CDC pair for the git half of 4.8. The GIT-D2 (GIT-1 half) drill
  scenario. The pseudonymity rule is mandatory-core (it gates the data model): state the cargo-mutants
  mutation-score floor and meet it.
- **DEFINITION OF DONE.** Pseudonymous-by-default is enforced at receive-pack and in the schema; GIT-D2's GIT-1
  half emits its dated green artifact (0 cleartext PII in commit identities); the chosen enforcement default is
  recorded with rationale; the residual is instantiated by reference to 10.9 (not restated); the no-untagged
  lint is green; the unit + CDC + drill tests pass; GF-7 is named with its follow-on (GIT-P29); the work is
  committed.
- **COMMIT.** Header: P-<NNN> M3: pseudonymous-by-default commit identities (the data-model gate). Body lists:
  contract 4.8 enforced (pseudonymous-by-default), 10.9 instantiated by reference; GIT-D2 GIT-1 half greened
  (0 cleartext PII, measured); the enforcement default recorded; GF-7 floor named with follow-on GIT-P29 +
  R-7 (Legal). Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By
  trailer.

---

### GIT-P13 — The front door (SSH + smart-HTTP v2): authenticate, check, placement, residency reject, cross-tenant isolation (GIT-D8)

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G2 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G2", the router +
  authenticate/check/placement/residency-reject half — GIT-D8; FIRST RUNNABLE).
- **DEPENDS-ON.** GIT-P9 (the serving tier + the receive-pack backend it routes to), GIT-P11 (repo placement /
  pack tier), GIT-P1 (the Git ReBAC fragment to check against — wired live in GIT-P14). The M1 Identity prompts
  that ship authenticate (machine-identity SSH/deploy-key/PAT/per-job, 4.1), check + CaveatContext (4.2),
  write_tuples/zookie (4.6/4.10). The M1 Tenancy prompts that ship the (tenant,region) partition (12.1),
  discover/placement_of repo-granular relocatable (12.2), residency_verify (12.4). The M0 ResilientClient
  (1.9). The index places this after GIT-P9/GIT-P11 and the M1 Identity/Tenancy work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (EU-sovereign, residency by construction);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — cross-tenant 0 is a quantified
    gate), §5 (the lints as committed ratchet).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/02-internals-and-algorithms.md §1 (the
    front door / router); 00-overview.md §2 (A) (the stateless front door — authenticate → check → placement_of
    → residency reject → stream; liveness≠readiness), §1.2 (the SSH/HTTPS front door); 01-tech-and-data-model.md
    §1 (russh + axum/hyper).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §1 (machine-identity
    resolution), §10 (repo-granular placement, residency).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 4.1 (authenticate —
    machine-identity SSH/deploy-key/PAT/per-job → Principal), 4.2 (check + CaveatContext), 12.1/12.2/12.4
    (partition + placement_of + residency_verify), 1.9 (ResilientClient).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G2 + §1 (non-negotiability item 3) + §2
    (★ rows 4.1/4.2/12.2) + §6 (first runnable = end of M3-G2).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row GIT-D8 (cross-tenant front-door
    isolation).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - The stateless front door / router: SSH (russh) + smart-HTTP protocol-v2 (axum/hyper). Id.authenticate
    resolves every machine-identity (SSH pubkey / deploy-key / PAT / per-job token, 4.1) → Principal; the
    per-action Id.check with CaveatContext (4.2) gates push/merge/review; discover/placement_of(repo) → cell +
    backend node (12.2); REJECT any route that would leave the region (ADR-11, residency-pin lint); streams
    packs WITHOUT full buffering; liveness ≠ readiness (readiness gates on backend reachability, liveness does
    not).
  - FLOOR named: none here (the FailStatic degrade-not-cascade is GIT-P14; the shed order + CDN floor is
    GIT-P15). State that GIT-P14 wires the ReBAC fragment LIVE + FailStatic and GIT-P15 lands the shed order.
- **CONTRACTS TO IMPLEMENT.** 4.1 authenticate (consumed — every entrypoint resolves a Principal), 4.2 check +
  CaveatContext (consumed — per-action gate), 12.2/12.4 placement_of + residency_verify (consumed —
  region-pinned placement, reject-if-leaving-region), 1.9 ResilientClient (consumed). Implement to the frozen
  shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - GIT-D8 (CI): cross-tenant repo access via a token whose tenant ≠ the URL-path tenant → TENANT FROM THE
    TOKEN; 0 cross-tenant read; rejected at the front door. Green artifact: the authz-deny signal + the
    tenant-predicate lint green (cross-tenant-read-count = 0).
  - A route that would leave the region is REJECTED at the front door (residency-pin lint green; 0
    out-of-region routes admitted) — CI.
- **TESTS (required).** Unit tests for the authenticate → check → placement_of → residency-reject pipeline
  (each machine-identity kind resolves; a wrong-tenant token denies; an out-of-region route rejects). A chained
  e2e test: SSH clone → push → check gate → residency reject path. The CDC pairs for the consumed rows
  4.1/4.2/12.2. The GIT-D8 drill scenario. The router/authz path is mandatory-core: state the cargo-mutants
  mutation-score floor and meet it.
- **DEFINITION OF DONE.** The front door authenticates every machine-identity, checks every action, and rejects
  out-of-region routes; GIT-D8 emits its dated green artifact (0 cross-tenant read); the residency-pin +
  tenant-predicate lints are green; the unit + chained-e2e + CDC + drill tests pass; the GIT-P14/GIT-P15
  follow-ons are named; the work is committed. This is FIRST RUNNABLE (roadmap §6): clone/push works,
  authenticated, tenant-isolated, region-pinned, never loses an event (with GIT-P14/P15 completing the shed +
  degrade behaviour).
- **COMMIT.** Header: P-<NNN> M3: git front door — SSH + smart-HTTP v2, authenticate/check/placement/residency
  (GIT-D8). Body lists: contracts 4.1/4.2/12.2/12.4/1.9 implemented; GIT-D8 greened (0 cross-tenant read,
  measured); the router mutation score measured; the GIT-P14 (ReBAC live + FailStatic) + GIT-P15 (shed + CDN)
  follow-ons named. Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By
  trailer.

---

### GIT-P14 — Wire the Git ReBAC fragment LIVE + the FailStatic bound on the Id dependency

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G2 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G2", the ReBAC-live +
  FailStatic half).
- **DEPENDS-ON.** GIT-P13 (the front door that checks against the fragment), GIT-P1 (the Git ReBAC fragment
  frozen, now wired live). The M1 Identity prompts that ship the ReBAC engine (4.9), write_tuples/zookie
  (4.6/4.10), and fail-static (4.11). The index places this directly after GIT-P13.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (EU-sovereign); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it —
    a degrade is proven by a forced dependency break + observability), §5 (the lints as committed ratchet).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/00-overview.md §1.2 (the Git ReBAC
    fragment live — ref-glob relations, CODEOWNERS-as-relations, protected_push, approve_untrusted_ci) + §2 (A)
    (FailStatic on the Id dependency — degrade not cascade); 02-internals-and-algorithms.md §1 (the check path).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §1 (the frozen Git
    ReBAC fragment).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 4.9 (the Git ReBAC fragment
    live), 4.6/4.10 (write_tuples / zookie — read-your-writes), 4.11 (FailStatic bound on the Id dependency —
    static_max ≤ revocation SLA), 1.10 (FailStatic<T>).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G2 (the ReBAC-live + FailStatic bullet) + §2
    (rows 4.6/4.9/4.11).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - The Git ReBAC fragment wired LIVE (4.9): ref-glob relations, CODEOWNERS-as-relations, protected_push,
    approve_untrusted_ci evaluated at the front-door check (GIT-P13) and at the push policy (GIT-P9).
    write_tuples/zookie (4.6/4.10) for read-your-writes on a just-granted relation.
  - The FailStatic bound on the Id dependency (4.11) so an Id hiccup DEGRADES, not cascades (just-revoked still
    denied; static_max ≤ revocation SLA): the git→Id check rides ResilientClient + FailStatic<T> (1.10).
  - FLOOR named: none. State that the shed order + CDN floor is the GIT-P15 follow-on.
- **CONTRACTS TO IMPLEMENT.** 4.9 the Git fragment (owned, now live), 4.6/4.10 write_tuples/zookie (consumed),
  4.11 FailStatic (consumed — degrade-not-cascade). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The Git ReBAC fragment is live: a protected_push / CODEOWNERS / approve_untrusted_ci relation is enforced
    at the check (0 unauthorized actions admitted; read-your-writes within the zookie bound) — CI.
  - A forced Id-dependency break: the git→Id check DEGRADES under FailStatic (just-revoked still denied;
    static_max ≤ revocation SLA; 0 cascade) — CI, the fail-static survival signal. (This is the degrade-not-
    cascade property — proven by a scoped reversible dependency break, EI-01 §3.)
- **TESTS (required).** Unit tests for the live fragment evaluation (each relation enforced) + the FailStatic
  degrade path (break Id → degrade, not cascade; just-revoked denied). A chained e2e test: grant a relation →
  read-your-writes within the zookie → break Id → assert degrade + just-revoked-denied. The CDC pairs for rows
  4.9/4.11. The fragment-evaluation + fail-static path is mandatory-core: state the cargo-mutants mutation-score
  floor and meet it.
- **DEFINITION OF DONE.** The Git ReBAC fragment is live and enforced; the FailStatic bound degrades (not
  cascades) under a forced Id break with just-revoked still denied; the unit + chained-e2e + CDC tests pass; the
  fragment/fail-static mutation score is measured; the GIT-P15 follow-on is named; the work is committed.
- **COMMIT.** Header: P-<NNN> M3: Git ReBAC fragment live + FailStatic on the Id dependency. Body lists:
  contracts 4.9 (live)/4.6/4.10/4.11 implemented; the live-fragment + fail-static degrade greened (just-revoked
  denied, 0 cascade, measured); the mutation score measured; the GIT-P15 shed/CDN follow-on named. Branch first
  if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P15 — The protected-human-lane shed order + the CDN bundle-URI accelerated-clone floor

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G2 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G2", the shed-order +
  CDN-floor half).
- **DEPENDS-ON.** GIT-P13 (the front door the shed order sits at). The M0 ResilientClient + FailStatic +
  shed-order primitives (1.9/1.10/1.11, the protected-human-lane ADR-16). The M1 Storage CDN clone/bundle class
  (11.2 C3). The index places this directly after GIT-P13/GIT-P14.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (top-of-the-line UX under load);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the shed order is proven under a
    synthetic storm; observability is part of the pass).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/00-overview.md §2 (A) (the shed order
    at the front door — speculative → batch/CI → agent → human-last); 02-internals-and-algorithms.md §1 (the
    clone/bundle accelerated-clone path).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-K (per-surface
    shed budgets), §8 (the within-EU CDN clone/bundle class).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 1.11 (the protected-human-lane
    shed order, ADR-16), 11.2 (the within-EU CDN clone/bundle blob class — C3).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G2 (the shed-order + CDN-floor bullet) + §5
    (the per-surface shed budget floor OQ-K + the bundle-URI floor).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - The protected-human-lane shed order (ADR-16, the OQ-K per-surface budget floor: speculative → batch/CI →
    agent → human-last) at the front door, with 429 + Retry-After. The shed budget is read from the thresholds
    file.
  - The CDN clone/bundle accelerated-clone path (11.2 C3) ships its bundle-URI floor here: a clone may be served
    a bundle-URI from the within-EU CDN class.
  - FLOOR named: the per-surface shed budget floor (OQ-K) is TUNED by GIT-D6 (the clone-storm 30× drill lands in
    GIT-P34/M5); the CDN bundle-URI floor here hardens to the full within-EU CDN class in GIT-P33. Name both.
- **CONTRACTS TO IMPLEMENT.** 1.11 the shed order (consumed — the protected-human-lane order at the front door),
  11.2 C3 the CDN clone/bundle class (consumed — the bundle-URI floor). Implement to the frozen shapes; escalate
  a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The shed order holds under a synthetic mixed-principal storm: the human lane is served while the agent/CI
    lane sheds (429 + Retry-After) — CI (the full 30× surge is GIT-D6 in GIT-P34; here the order is asserted at
    1× with mixed principals). Green artifact: the per-lane shed-count signal (human lane 0 shed, agent lane
    sheds).
  - A clone served a bundle-URI from the CDN class round-trips a valid clone (the accelerated-clone floor
    holds) — CI.
- **TESTS (required).** Unit tests for the shed-order decision (human-last; 429 + Retry-After on the shed lane)
  and the bundle-URI clone path. A chained e2e test: mixed-principal storm → human served, agent shed →
  bundle-URI clone valid. The CDC pairs for rows 1.11/11.2 C3. The shed-order path is mandatory-core: state the
  cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** The shed order holds under a synthetic storm (human lane served, agent lane sheds with
  429 + Retry-After); the CDN bundle-URI clone round-trips; the unit + chained-e2e + CDC tests pass; the
  shed-order mutation score is measured; the shed-budget (OQ-K → GIT-P34) + CDN (→ GIT-P33) floors are named;
  the work is committed.
- **COMMIT.** Header: P-<NNN> M3: protected-human-lane shed order + CDN bundle-URI clone floor. Body lists:
  contracts 1.11/11.2 C3 implemented; the shed order greened (human served, agent shed, measured); the
  bundle-URI clone valid; the shed-order mutation score measured; the shed-budget + CDN floors named with
  follow-ons (GIT-P34/GIT-P33). Branch first if on default; do not push unless asked. End with the workspace
  Co-Authored-By trailer.

---

### GIT-P16 — The PR/review/inline-thread lifecycle + branch-protection rulesets + the CODEOWNERS resolver

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G3 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G3", the
  domain-entities half).
- **DEPENDS-ON.** GIT-P9 (the control-plane OLTP + the object store), GIT-P13 (the front door + check gate),
  GIT-P14 (the live ReBAC fragment the CODEOWNERS resolver feeds). The index places this after GIT-P13/GIT-P14.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (top-of-the-line UX); ../../external-insights/01-process-and-quality-doctrine.md §4
    (actually try it — chain mutations end-to-end).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md (the
    PR/review/comment entities); 00-overview.md §1.1 (the PR lifecycle, reviews, inline comment threads,
    CODEOWNERS, branch-protection rulesets).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 4.9 (the CODEOWNERS-as-relations
    fragment the resolver compiles to — the reviewer relation).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G3 (the PR/review/threads/rulesets/CODEOWNERS
    bullet).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate (control-plane OLTP):
  - The Pull Request lifecycle, Reviews, inline comment THREADS (the thread entity — the body content is
    GIT-P17, the line-anchor resolver is GIT-P23), branch-protection rulesets, and the CODEOWNERS resolver — all
    on the control-plane OLTP (one DB, RLS, per-subject DEK for free-text bodies). The CODEOWNERS resolver
    compiles a CODEOWNERS path pattern to the reviewer relation (4.9, the fragment from GIT-P1).
  - FLOOR named: none new. State that PR/review/thread bodies are single-author CAS (the body content +
    round-trip is GIT-P17), with the multi-author collab story owned by Knowledge, not git.
- **CONTRACTS TO IMPLEMENT.** 4.9 the CODEOWNERS-as-relations consumer (owned — the resolver compiling
  CODEOWNERS patterns to the reviewer relation). Implement to the frozen shape; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The PR/review/thread lifecycle state machine is correct: open → review → merge/close transitions are
    well-formed (0 illegal transitions); the CODEOWNERS resolver maps a path pattern to the right reviewer
    relation (0 mis-resolved owners on a fixture CODEOWNERS) — CI.
  - The branch-protection ruleset is enforced at the entity layer (a protected base_ref requires the ruleset's
    conditions; 0 unprotected merges to a protected ref at the entity gate) — CI.
- **TESTS (required).** Unit tests for the PR/review/thread lifecycle state machine, the branch-protection
  ruleset evaluation, and the CODEOWNERS resolver. A chained e2e test: open PR → request review (CODEOWNERS
  resolves) → submit review → close. The CDC pair for the CODEOWNERS half of row 4.9. The lifecycle + CODEOWNERS
  path is mandatory-core: state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** The PR/review/thread entities + branch-protection rulesets + the CODEOWNERS resolver
  exist; the lifecycle transitions are well-formed (0 illegal); the CODEOWNERS resolver is correct (0
  mis-resolved); the unit + chained-e2e + CDC tests pass; the lifecycle mutation score is measured; the
  single-author-CAS + body-content-in-GIT-P17 notes are written; the work is committed.
- **COMMIT.** Header: P-<NNN> M3: PR/review/thread lifecycle + branch-protection rulesets + CODEOWNERS resolver.
  Body lists: 4.9 CODEOWNERS consumer implemented; the lifecycle (0 illegal transitions) + CODEOWNERS (0
  mis-resolved) greened; the lifecycle mutation score measured; the body-content follow-on named (GIT-P17).
  Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P17 — PR/review/comment bodies on the myelin-content subset + the content-node → refs.edge.created emission

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G3 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G3", the content-bodies +
  reference-graph-edges half — render==md + 1-edge-per-node).
- **DEPENDS-ON.** GIT-P16 (the PR/review/thread entities the bodies attach to), GIT-P4 (the comment-/thread-
  #sub mints). The M2 Refs prompts that ship refs.edge.created from content nodes (5.4). The M2 myelin-content
  freeze (13.1 — the markdown-subset + the three structured inline nodes). The index places this after GIT-P16
  and the M2 Refs/content work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (the cross-artifact reference graph); §3 (content round-trips);
    ../../external-insights/01-process-and-quality-doctrine.md §4 (chain mutations end-to-end).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md (the
    inline nodes → refs.edge.created; the content bodies); 00-overview.md §1.1 (the inline comment thread
    bodies, single-author CAS).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-2 (the
    myelin-content taxonomy + the three content nodes byte-identical).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 5.4 (refs.edge.created from the
    mention/artifact_ref/embed content nodes — no standalone edge-write API), 13.1 (the myelin-content
    markdown-subset + render(parse(md)) === md).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G3 (the content-bodies bullet — "bodies use
    the frozen myelin-content markdown-subset + the three structured inline nodes which produce
    refs.edge.created uniformly; render(parse(md)) === md") + §4 (row 5.4).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md the KN-D2-class round-trip applied to
    git content.
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - PR/review/comment bodies use the FROZEN myelin-content markdown-subset + the three structured inline nodes
    (mention/artifact_ref/embed, 13.1). Single-author CAS over the content subset; render(parse(md)) === md.
  - The three inline nodes produce refs.edge.created UNIFORMLY via the outbox (Closes <ISSUEKEY> / @alice /
    embeds → edges, 5.4) — no standalone edge-write API; the edges are emitted from the content nodes only.
  - FLOOR named: none. State that the typed-edge lifecycle mirror (PR-link/commit-trailer closes/relates edges)
    is the GIT-P19 follow-on, distinct from these content-node mention/ref/embed edges.
- **CONTRACTS TO IMPLEMENT.** 5.4 refs.edge.created (owned — emitted from the content nodes via outbox), 13.1
  myelin-content (consumed — the markdown-subset for bodies). Implement to the frozen shapes; escalate a needed
  change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The three inline ref nodes each emit EXACTLY ONE refs.edge.created (mention/artifact_ref/embed → 1 edge
    each; 0 duplicate, 0 missed) — CI.
  - render(parse(md)) === md is 100% on PR/comment bodies (the KN-D2-class round-trip applied to git content; a
    corpus of git bodies round-trips byte-identical) — CI, round-trip-parity = 100%.
- **TESTS (required).** Unit tests for the content-node → edge emission (1 edge per node; via outbox) and the
  round-trip parser. A chained e2e test (EI-01 §4): add an inline comment with a mention + a Closes trailer +
  an embed → assert exactly the right edges + the round-trip parity. The CDC pairs for rows 5.4/13.1. The
  content-node → edge path is mandatory-core: state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** PR/review/comment bodies are on the myelin-content subset; the three content nodes
  each emit exactly one edge (0 dup/missed); render(parse(md)) === md is 100% on git bodies; the unit +
  chained-e2e + CDC tests pass; the content-node mutation score is measured; the typed-edge-mirror follow-on
  (GIT-P19) is named; the work is committed.
- **COMMIT.** Header: P-<NNN> M3: PR/comment bodies on myelin-content + content-node reference edges. Body
  lists: contracts 5.4/13.1 implemented; edges (1-per-node, measured) + render==md (100%, measured) greened;
  the content-node mutation score measured; the typed-edge-mirror follow-on named (GIT-P19). Branch first if on
  default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P18 — project(ref, viewer) for git artifacts + the ArtifactRef id grammar (per-viewer permission-checked)

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G3 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G3", the project() +
  ArtifactRef-grammar half).
- **DEPENDS-ON.** GIT-P16 (the PR/commit/review entities project() reads), GIT-P14 (the live check project()
  permission-filters against). The M2 Refs prompts that ship ArtifactRef parse/format (5.1) + resolve/project
  (5.2/5.6). The index places this after GIT-P16 and the M2 Refs work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (the cross-artifact reference graph);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — a viewer without access gets a
    tombstone, never the title; 0 leak is quantified).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md
    (project(); the ArtifactRef id grammar); 00-overview.md §0.1 Δ7 (the ArtifactRef id grammar — pr/<repo>:<n>,
    commit/<repo>:<sha> are the stored canonical keys, #1421 is render-time).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md REF-3 (display keys
    render-time only).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 5.1 (ArtifactRef id grammar —
    git's stable canonical keys), 5.2/5.6 (resolve / project(ref, viewer) — the only way Refs/Search/Notif read
    git artifacts, per-viewer permission-checked).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G3 (the project() + ArtifactRef bullet) + §4
    (rows 5.1/5.2/5.6).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - project(ref, viewer) (5.6) for git artifacts (PR/commit/review) — the ONLY way Refs/Search/Notif read git's
    artifacts, per-viewer permission-checked: a viewer without access gets a TOMBSTONE, never the title.
  - ArtifactRef id grammar (5.1, REF-3): git's stored canonical key is the sha / PR-number (already stable —
    commit/<repo>:<sha>, pr/<repo>:<n>); the #1421-style display is render-time only.
  - FLOOR named: none. State that project() here feeds the M3-G5/M5 leak drills (GIT-D11, SRCH-D1/D3).
- **CONTRACTS TO IMPLEMENT.** 5.1 ArtifactRef id grammar (owned — git's stable keys), 5.2/5.6 resolve/project
  (owned — project(ref, viewer) for git artifacts). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - project(ref, viewer) returns a per-viewer permission-checked projection; a viewer without access gets a
    tombstone, never the title (0 title leaks to an unauthorized viewer; feeds the M3-G5/M5 leak drills) — CI.
  - The ArtifactRef id grammar round-trips: commit/<repo>:<sha> and pr/<repo>:<n> parse/format stably; the
    #1421 display is render-time only (0 stored display keys) — CI.
- **TESTS (required).** Unit tests for project(ref, viewer) (authorized viewer gets the projection; unauthorized
  gets a tombstone) and the ArtifactRef parse/format round-trip. A chained e2e test: resolve a PR ref as an
  authorized viewer (gets the title) and as an unauthorized viewer (gets a tombstone). The CDC pairs for rows
  5.1/5.2/5.6. The project() permission-filter path is mandatory-core (a leak is the failure): state the
  cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** project(ref, viewer) is per-viewer permission-checked (unauthorized → tombstone, 0
  title leak); the ArtifactRef grammar round-trips (0 stored display keys); the unit + chained-e2e + CDC tests
  pass; the project() mutation score is measured; the leak-drill-feed note is written; the work is committed.
- **COMMIT.** Header: P-<NNN> M3: project(ref, viewer) for git artifacts + ArtifactRef id grammar. Body lists:
  contracts 5.1/5.2/5.6 implemented; project() per-viewer (unauthorized → tombstone, 0 leak, measured) +
  ArtifactRef round-trip greened; the project() mutation score measured. Branch first if on default; do not
  push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P19 — The typed-edge mirror (PR-link / commit-trailer lifecycle edges into the Refs projection)

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G3 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G3", the
  typed-edge-mirror half).
- **DEPENDS-ON.** GIT-P17 (the content-node edges this mirror is distinct from), GIT-P16 (the PR entities whose
  lifecycle produces the typed edges). The M2 Refs prompts that ship the typed-edge mirror (5.5). The index
  places this after GIT-P17.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (the cross-artifact reference graph);
    ../../external-insights/01-process-and-quality-doctrine.md §7 (keep contracts coherent).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md (the
    typed-edge mirror — PR-link / commit-trailer lifecycle edges); 00-overview.md §1.1 (the PR/commit-trailer
    lifecycle).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 5.5 (the typed-edge mirror —
    PR-link / commit-trailer lifecycle edges closes/relates into the Refs projection).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G3 (the typed-edge-mirror bullet) + §4 (row
    5.5).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - The typed-edge mirror (5.5): PR-link / commit-trailer lifecycle edges (closes/relates) emitted into the Refs
    projection via the outbox as the PR lifecycle advances (a Closes <ISSUEKEY> trailer on a merged PR produces
    a closes edge; a PR-link produces a relates edge). These are lifecycle edges, distinct from the content-node
    mention/ref/embed edges (GIT-P17).
  - FLOOR named: none.
- **CONTRACTS TO IMPLEMENT.** 5.5 the typed-edge mirror (owned — PR/trailer lifecycle edges). Implement to the
  frozen shape; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - A PR lifecycle transition emits the correct typed edge (a Closes trailer on merge → exactly one closes edge;
    a PR-link → exactly one relates edge; 0 duplicate, 0 missed) — CI.
- **TESTS (required).** Unit tests for the typed-edge emission per lifecycle transition (closes on merge;
  relates on link). A chained e2e test: open PR with a Closes trailer → merge → assert exactly one closes edge.
  The CDC pair for row 5.5. The typed-edge path is mandatory-core: state the cargo-mutants mutation-score floor
  and meet it.
- **DEFINITION OF DONE.** The typed-edge mirror emits the correct lifecycle edges (0 dup/missed); the unit +
  chained-e2e + CDC tests pass; the typed-edge mutation score is measured; the work is committed.
- **COMMIT.** Header: P-<NNN> M3: typed-edge mirror (PR-link / commit-trailer lifecycle edges). Body lists:
  contract 5.5 implemented; the lifecycle edges (0 dup/missed, measured) greened; the typed-edge mutation score
  measured. Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P20 — The check_status projection table + run_attempt monotonic supersession (the X-1 consumer core)

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G4 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G4", the
  check_status-projection + supersession slice of the X-1 consumer — GIT-D10 part (a)).
- **DEPENDS-ON.** GIT-P16 (the PR entities the projection keys to), GIT-P6 (the declared CheckStatus consumer
  contract module). The M2 freeze of the 5.9 CheckStatus shape. The CI producer side (5.9, M4) is the
  co-dependency — built here against a SYNTHETIC ci.check.updated emitter, proven end-to-end at the M4 exit.
  The index places this after GIT-P16 and the M2 X-1 declaration (GIT-P6).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (the merge gate); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it
    — exactly 1 current row per key is a quantified gate, the bus is at-least-once so the stale drop is
    mandatory).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/02-internals-and-algorithms.md §6 (the
    check_status projection + run_attempt supersession); 00-overview.md §0.1 Δ1 (the frozen CheckStatus fact).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-1 (the seam).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 5.9 (the check_status projection
    keyed (commit_oid, context), run_attempt last-writer-wins supersession, idempotent on event_id).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G4 (the check_status-projection bullet) + §2
    (the seam frozen-but-not-live note) + §4 (row 5.9).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row GIT-D10 part (a) (out-of-order/dup →
    supersession holds the correct current row).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate (control-plane OLTP):
  - The check_status PROJECTION TABLE keyed (commit_oid, context) — the X-1 consumer core (5.9): consumes
    ci.check.updated (from a SYNTHETIC emitter here), applies MONOTONIC run_attempt supersession (>= supersedes,
    < dropped as stale re-delivery — the bus is at-least-once so the drop is mandatory), idempotent on event_id,
    holds EXACTLY ONE current row per key. Git READS trust_tier OFF THE FACT, never recomputes it.
  - FLOOR named: the seam-floor — built here against a synthetic ci.check.updated emitter, the real CI producer
    goes live at M4 (GIT-D10/CI-D8 end-to-end). Name it. State that the required-set merge gate is GIT-P21, the
    fork-endorsement is GIT-P22, the merge queue is GIT-P23.
- **CONTRACTS TO IMPLEMENT.** 5.9 the CheckStatus projection + supersession (owned — the projection table,
  monotonic run_attempt supersession, event_id idempotency). Implement to the frozen shape; escalate a needed
  change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - GIT-D10 part (a) (CI, against the synthetic producer here; RE-CONFIRMED end-to-end at the M4 exit):
    out-of-order/dup ci.check.updated → run_attempt-monotonic supersession holds the correct current row, drops
    stale lower attempts (EXACTLY 1 current row per key; idempotent on event_id). Green artifact: the
    1-current-row-per-key signal.
  - The no-cross-sync-cycle lint green (git makes 0 synchronous calls to CI — it reads its own projection) — CI.
- **TESTS (required).** Unit tests for the supersession rule (>= supersedes, < dropped; idempotent on event_id;
  exactly one current row). A chained e2e test (EI-01 §4): synthetic ci.check.updated out-of-order + dup →
  projection holds exactly one current row per key. The provider/consumer CDC pair for row 5.9 (git's consumer
  projection half). The GIT-D10 part-(a) drill scenario against the synthetic producer. The supersession path is
  mandatory-core: state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** The check_status projection + supersession exist; GIT-D10 part (a) emits its dated
  green artifact against the synthetic producer (1 current row/key, idempotent); the no-cross-sync-cycle lint is
  green; the unit + chained-e2e + CDC + drill tests pass; the supersession mutation score is measured; the
  seam-floor + the GIT-P21/P22/P23 follow-ons are named; the work is committed.
- **COMMIT.** Header: P-<NNN> M3: check_status projection + run_attempt supersession (X-1 consumer core). Body
  lists: contract 5.9 (projection+supersession) implemented; GIT-D10 part (a) greened against the synthetic
  producer (1 row/key, measured); the no-cross-sync-cycle lint green; the supersession mutation score measured;
  the seam-floor + merge-gate/endorse/queue follow-ons named (GIT-P21/P22/P23, M4 co-gate). Branch first if on
  default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P21 — The merge gate + the required-set policy (Git owns what is allowed to land)

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G4 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G4", the merge-gate +
  required-set-policy slice).
- **DEPENDS-ON.** GIT-P20 (the check_status projection the gate reads), GIT-P16 (the PR/ruleset entities the
  gate guards). The index places this after GIT-P20.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (the merge gate); ../../external-insights/01-process-and-quality-doctrine.md §7 (keep
    contracts coherent — git reads its own projection, never calls CI synchronously).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/02-internals-and-algorithms.md §6 (the
    merge gate + the required-set policy); 00-overview.md §1.1 (Git owns what is allowed to land; reads
    trust_tier off the fact, never recomputes).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 5.9 (the required-set policy —
    ruleset.required_contexts; CI reports facts, Git decides which contexts gate this base_ref).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G4 (the merge-gate bullet — "Git owns the
    required-set policy; reads trust_tier off the fact").
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - The MERGE GATE: Git owns the required-set policy (ruleset.required_contexts — CI reports facts, Git decides
    which contexts gate this base_ref). The gate reads the check_status projection (GIT-P20) and the
    branch-protection ruleset (GIT-P16): a base_ref's required contexts must all be green-and-current for the
    merge to be allowed. Git READS trust_tier OFF THE FACT, never recomputes it.
  - FLOOR named: none here (the fork/trust endorsement gate is GIT-P22; the merge queue is GIT-P23). State both
    follow-ons.
- **CONTRACTS TO IMPLEMENT.** 5.9 the required-set policy (owned — ruleset.required_contexts gating the merge).
  Implement to the frozen shape; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The required-set policy gates correctly: a base_ref with an unmet required context is BLOCKED; all required
    contexts green-and-current → allowed (0 merges admitted with a missing/stale required context) — CI. Green
    artifact: the required-set-gate signal (0 under-gated merges).
- **TESTS (required).** Unit tests for the required-set policy (missing context blocks; stale context blocks;
  all-green allows). A chained e2e test: configure required contexts → project some green/some missing → assert
  the gate blocks → complete the set → assert allowed. The CDC pair for the required-set-policy half of row 5.9.
  The merge-gate policy path is mandatory-core: state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** The merge gate + required-set policy exist; the gate blocks on a missing/stale required
  context and allows on a complete green set (0 under-gated merges); the unit + chained-e2e + CDC tests pass; the
  merge-gate mutation score is measured; the GIT-P22/P23 follow-ons are named; the work is committed.
- **COMMIT.** Header: P-<NNN> M3: merge gate + required-set policy. Body lists: contract 5.9 (required-set
  policy) implemented; the required-set gate greened (0 under-gated merges, measured); the merge-gate mutation
  score measured; the fork-endorse (GIT-P22) + merge-queue (GIT-P23) follow-ons named. Branch first if on
  default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P22 — The fork / trust-tier endorsement gate (the poisoned-pipeline defence, GIT-D10 (b)+(c))

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G4 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G4", the
  fork-endorsement slice — GIT-D10 parts (b) neutral-until-endorsed + (c) endorse-flips).
- **DEPENDS-ON.** GIT-P21 (the merge gate the endorsement feeds), GIT-P20 (the projection carrying trust_tier),
  GIT-P14 (the live approve_untrusted_ci relation). The M1 Storage trust-scoped cache (11.2 C4). The index
  places this after GIT-P21.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (the merge gate); ../../external-insights/01-process-and-quality-doctrine.md §2
    (poisoned-pipeline / supply-chain is a non-negotiability), §3 (prove-it — a fork must never green its own
    required gate).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/02-internals-and-algorithms.md §6.3
    (the fork / trust-tier gate — untrusted_fork neutral until endorsed; fork cache confined to fork:<pr_id>);
    00-overview.md §0.1 Δ3 (untrusted-fork neutral-until-endorsed).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-1 (the seam) +
    §8 (the trust-scoped cache namespaces — an UntrustedFork write cannot reach the trusted cache scope).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 5.9 (fork-endorsement — an
    untrusted_fork success is neutral until check(subject, approve_untrusted_ci, repo)), 11.2 C4 (the
    fork:<pr_id> trust-scoped cache), 4.9 (the approve_untrusted_ci relation).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G4 (the fork/trust-tier-gate bullet) + §5
    (the trust-scoped cache floor).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row GIT-D10 parts (b) (fork self-green
    neutral) + (c) (maintainer endorse → gate flips green).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - The fork / trust-tier gate (the poisoned-pipeline defence): an untrusted_fork success is NEUTRAL FOR GATING
    until a maintainer endorses via check(subject, approve_untrusted_ci, repo) OR the context is re-run trusted.
    Fork-PR cache writes confined to the fork:<pr_id> scope (11.2 C4) — a fork cannot reach the trusted cache or
    the trusted gate.
  - FLOOR named: none here (the merge queue is GIT-P23). State the follow-on.
- **CONTRACTS TO IMPLEMENT.** 5.9 the fork-endorsement (owned — neutral-until-endorsed), 11.2 C4 the
  trust-scoped cache (consumed — fork:<pr_id> scope), 4.9 approve_untrusted_ci (owned — the endorsement
  relation, wired live in GIT-P14). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - GIT-D10 part (b) (CI): a fork PR self-greens → NEUTRAL FOR GATING (merge blocked; 0 forks green their own
    required gate). Green artifact: the fork-neutral signal.
  - GIT-D10 part (c) (CI): a maintainer endorses via approve_untrusted_ci → the gate flips green. Green
    artifact: the endorse-flips signal.
  - A fork cache write cannot reach the trusted cache scope (the fork:<pr_id> confinement holds; 0 fork writes
    in the trusted scope) — CI.
- **TESTS (required).** Unit tests for the fork-neutral-until-endorsed flow (fork self-green stays neutral;
  endorse flips green; re-run-trusted flips green) + the trust-scoped cache confinement. A chained e2e test
  (EI-01 §4): fork self-green → assert neutral (merge blocked) → maintainer endorses → assert green. The CDC
  pair for the fork-endorsement half of row 5.9. The fork-gate path is mandatory-core (a poisoned pipeline is
  the failure): state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** The fork/trust-tier gate exists; GIT-D10 parts (b)+(c) emit their dated green
  artifacts (fork self-green neutral, endorse flips); the fork:<pr_id> cache confinement holds (0 fork writes
  in the trusted scope); the unit + chained-e2e + CDC + drill tests pass; the fork-gate mutation score is
  measured; the merge-queue follow-on (GIT-P23) is named; the work is committed.
- **COMMIT.** Header: P-<NNN> M3: fork / trust-tier endorsement gate (poisoned-pipeline defence). Body lists:
  contracts 5.9 (fork-endorsement)/11.2 C4/4.9 implemented; GIT-D10 (b) fork-neutral + (c) endorse-flips greened
  (measured); the cache confinement held; the fork-gate mutation score measured; the merge-queue follow-on named
  (GIT-P23). Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P23 — The merge queue as a durable workflow (parks on ci.result; exactly-once merge; GIT-D10 (d) + the aggregate)

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G4 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G4", the merge-queue
  durable-workflow slice — GIT-D10 part (d) exactly-once merge + the full GIT-D10 aggregate confirmation).
- **DEPENDS-ON.** GIT-P21 (the merge gate the queue serialises), GIT-P22 (the fork-endorsement the queue
  respects), GIT-P20 (the projection it waits on). The M2 Workflow prompts that ship DurableExecutor + WfCtx +
  SCHEDULE_AND_RUN_JOB + the durable signal (9.1/9.2/9.4) + the timer wheel (9.3). The CI producer (5.9, M4) is
  the co-dependency — proven end-to-end at the M4 exit. The index places this after GIT-P22 and the M2 Workflow
  work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (the merge gate); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it
    — 0 double-merge is a quantified gate; a doubly-delivered ci.result wakes the workflow exactly once).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/02-internals-and-algorithms.md §6 (the
    merge queue as a durable workflow per target ref, parks on the rollup ci.result signal); 00-overview.md §0.1
    Δ2 (the rollup ci.result wait).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-1 (the seam),
    OQ-F (per-effect idem_key + SCHEDULE_AND_RUN_JOB).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 5.9 (the merge queue waking on
    the ci.result rollup; idempotent on merge_attempt_id), 9.1/9.2/9.4 (DurableExecutor + SCHEDULE_AND_RUN_JOB +
    the durable ci.result signal), 9.3 (the timer wheel).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G4 (the merge-queue bullet) + §5 (GF-8
    single-lane queue floor + the seam-floor) + §4 (rows 9.1/9.2/9.4).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row GIT-D10 part (d) (doubly-delivered
    ci.result → merge wakes exactly once; 0 double-merge) + CI-D8 (the CI side, proven together at M4).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate (the merge-queue
  workflow):
  - The MERGE QUEUE as a DURABLE WORKFLOW per target ref (9.1/9.2/9.4): parks on the rollup ci.result signal via
    SCHEDULE_AND_RUN_JOB (holds no runtime while CI runs for hours); idempotent on the merge_attempt_id
    idem_token; a doubly-delivered ci.result wakes the workflow EXACTLY ONCE. Single-lane serialised (GF-8
    floor). Git NEVER synchronously calls CI (no-cross-sync-cycle lint) — it reads its own projection (GIT-P20).
  - FLOOR named: single-lane merge queue (GF-8 — speculative/parallel batching is GIT-P33/M5); the seam-floor —
    the real CI producer goes live at M4 (GIT-D10/CI-D8 end-to-end). Name both.
- **CONTRACTS TO IMPLEMENT.** 5.9 the merge-queue ci.result wait (owned — parks on ci.result, exactly-once
  merge), 9.1/9.2/9.4 the merge-queue durable workflow (consumed). Implement to the frozen shapes; escalate a
  needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - GIT-D10 part (d) (CI, against the synthetic producer here; RE-CONFIRMED end-to-end at the M4 exit): a
    doubly-delivered ci.result → the merge workflow wakes EXACTLY ONCE; 0 double-merge (merge-count == 1). Green
    artifact: the merge-count==1 signal.
  - THE FULL GIT-D10 AGGREGATE confirmed (against the synthetic producer): parts (a) supersession [GIT-P20], (b)
    fork-neutral + (c) endorse-flips [GIT-P22], (d) merge-once [here] all green-and-dated — CI. (Re-confirmed
    end-to-end with the real CI producer at the M4 exit.)
  - The no-cross-sync-cycle lint green (git makes 0 synchronous calls to CI) — CI.
- **TESTS (required).** Unit tests for the merge-queue idempotency (doubly-delivered ci.result → 1 merge;
  single-lane serialisation). A chained e2e test (EI-01 §4): synthetic ci.check.updated (out-of-order + dup) →
  projection holds → fork self-green neutral → endorse → ci.result doubly-delivered → merge wakes once (the full
  GIT-D10 chain). The CDC pair for the merge-queue half of row 5.9. The merge-queue path is mandatory-core:
  state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** The ci.result-waiting merge queue exists; GIT-D10 part (d) emits its dated green
  artifact (merge-count==1) and the FULL GIT-D10 aggregate is confirmed green-and-dated against the synthetic
  producer; the no-cross-sync-cycle lint is green; the unit + chained-e2e + CDC + drill tests pass; the
  merge-queue mutation score is measured; the GF-8 + seam-floor are named with their follow-ons (GIT-P33 / M4
  co-gate); the work is committed.
- **COMMIT.** Header: P-<NNN> M3: merge queue durable workflow (parks on ci.result, exactly-once merge). Body
  lists: contracts 5.9 (merge-queue)/9.1/9.2/9.4 implemented; GIT-D10 (d) merge-count==1 + the full GIT-D10
  aggregate greened against the synthetic producer (measured); the merge-queue mutation score measured; GF-8 +
  the seam-floor named (GIT-P33 / M4 co-gate). Branch first if on default; do not push unless asked. End with
  the workspace Co-Authored-By trailer.

---

### GIT-P24 — Content-anchored inline-thread line ranges (the #sub 4-state resolver, GIT-D7)

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G4 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G4", the
  content-anchored line-range resolver half — GIT-D7).
- **DEPENDS-ON.** GIT-P16 (the inline threads this anchors), GIT-P4 (the L<a>-L<b> #sub kind registered with
  Refs). The M2 Refs prompts that ship the unified #sub grammar + the 4-step tombstone ladder (5.7). The index
  places this after GIT-P16 (a sibling of GIT-P20..P23 in M3-G4; split out because the diff-anchor resolver has
  its own green gate, GIT-D7).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (code review with content-anchored line ranges);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — 0 mis-anchored is quantified;
    never silently wrong).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/02-internals-and-algorithms.md (the
    diff-anchoring as the content-fingerprint resolver, OQ-D); 00-overview.md §0.1 Δ4 (the frozen #sub resolver
    — git mints #L<a>-L<b> storing a BLAKE3 fingerprint + a context window + the mint-time blob oid, resolves
    through exact/rebased(moved)/partial(outdated)/tombstone(gone)); 01-tech-and-data-model.md §1 (imara-diff +
    BLAKE3).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-4 (the unified
    #sub grammar + the one tombstone ladder), OQ-D (content-anchored line ranges).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 5.7 (the unified #sub scheme +
    the 4-step tombstone ladder; git line-ranges content-anchored: BLAKE3 fingerprint + 3-way context match →
    exact/rebased/partial/tombstone; git is the owner's sub-anchor resolver the Refs ladder calls).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G4 (the content-anchored resolver bullet) +
    §5 (GF-5 per-pair fingerprint floor).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row GIT-D7 (force-push/rebase a PR with
    open inline threads → anchors resolve LIVE/MOVED/OUTDATED/GONE; 0 mis-anchored).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate (the diff-anchor service):
  - Mint #L<a>-L<b> storing a BLAKE3 content fingerprint of the anchored lines + a context window + the
    mint-time blob oid (the L<a>-L<b> #sub kind registered in GIT-P4).
  - Resolve through the unified 4-state ladder (5.7): exact (LIVE), rebased (MOVED — the lines moved but match),
    partial (OUTDATED — context drifted), tombstone (GONE — content_gone). Git is the owner's sub-anchor
    resolver the Refs ladder calls (Refs handles permission → root → sub-resolve → erased; git answers the
    sub-resolve step for L-ranges).
  - The "view in original context" render path for MOVED/OUTDATED/GONE (never silently wrong — always show the
    resolution state).
  - FLOOR named: GF-5 — per-pair fingerprint diff-anchor remap (4-state); patch-id-chain carry-over across a
    multi-commit rebase is the follow-on (GIT-P33/M5, R-6). Name it.
- **CONTRACTS TO IMPLEMENT.** 5.7 the git sub-anchor resolver (owned — the L<a>-L<b> mint + the 4-state
  content-fingerprint resolution the Refs ladder calls). Implement to the frozen ladder; escalate a needed
  change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - GIT-D7 (CI): force-push/rebase a PR with open inline threads → anchors resolve LIVE/MOVED/OUTDATED/GONE
    correctly; 0 MIS-ANCHORED; never silently wrong; "view in original context" renders. Green artifact: the
    per-anchor state distribution shows 0 mis-anchored across a rebase corpus.
- **TESTS (required).** Unit tests for each of the 4 resolution states (exact match; moved-but-match;
  partial-drift; content-gone) + the fingerprint+context-window matcher. A chained e2e test (EI-01 §4): open a
  thread on a line → force-push a rebase → assert each anchor resolves to the correct state, 0 mis-anchored. The
  CDC pair for the git half of row 5.7. The diff-anchor resolver is mandatory-core (silent mis-anchoring is the
  failure): state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** The #L<a>-L<b> mint + the 4-state resolver exist; GIT-D7 emits its dated green
  artifact (0 mis-anchored); "view in original context" renders for every non-LIVE state; the unit + chained-e2e
  + CDC + drill tests pass; the GF-5 floor is named with its follow-on (GIT-P33); the work is committed.
- **COMMIT.** Header: P-<NNN> M3: content-anchored inline-thread line ranges (#sub 4-state resolver). Body
  lists: contract 5.7 (git sub-anchor resolver) implemented; GIT-D7 greened (0 mis-anchored, measured); the
  resolver mutation score measured; GF-5 floor named with follow-on GIT-P33. Branch first if on default; do not
  push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P25 — The code-projection emitter for search (declare_indexable emit, incremental on push)

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G5 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G5", the
  code-projection-emitter half).
- **DEPENDS-ON.** GIT-P9 (the object store + the receive-pack post-commit path the emitter hooks into), GIT-P5
  (the declare_indexable spec registered with Search). The M2 Search prompts that ship declare_indexable +
  the index build (6.3/6.5). The index places this after GIT-P9 and the M2 Search work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (search references any artifact);
    ../../external-insights/01-process-and-quality-doctrine.md §7 (keep contracts coherent — git owns what to
    index, Search owns the index; no cross-DB).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/02-internals-and-algorithms.md §9 (the
    code-projection emitter); 03-events-contracts-and-glue.md §5.3 (the git.* code projection); 00-overview.md
    §1.1 (git owns what to index).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 6.3/6.5 (declare_indexable + the
    git.* code projection — path/language/symbols/literals/commit-msg/text; incremental update on push).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G5 (the code-projection-emitter bullet) + §4
    (rows 6.3/6.5) + §5 (GF-3 trigram floor).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - The code-projection emitter (6.3/6.5): per changed blob, emit {path, language, symbols (camel/snake split),
    literals, commit message, text}; INCREMENTAL update on push (hooked into the GIT-P9 receive-pack post-commit
    path). Search builds the trigram indices (symbol/path/literal/trigram-grade v1, GF-3) — git emits the
    projection, it does NOT build the index (no cross-DB).
  - FLOOR named: trigram/lexical code search v1 (GF-3); the AST-aware "find usages" via CI-produced SCIP/LSIF
    (R-3) is the GIT-P33/M5 follow-on. State that the SetExpr leak-free list push-down is GIT-P26.
- **CONTRACTS TO IMPLEMENT.** 6.3/6.5 declare_indexable + the code projection (owned — the emitter). Implement
  to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The emitter produces the 6.3/6.5 projection shape per changed blob and updates INCREMENTALLY on push (a push
    of N changed blobs emits exactly N projection updates; 0 missed, 0 stale) — CI. Green artifact: the
    projection-emit signal (emit-count == changed-blob-count).
- **TESTS (required).** Unit tests for the code-projection emitter (per-blob shape: path/symbols/literals/
  commit-msg; camel/snake symbol split; incremental update on push). A chained e2e test: push code → assert the
  projection emitted per changed blob, incremental. The CDC pairs for rows 6.3/6.5. The emitter path is
  mandatory-core: state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** The code-projection emitter exists and emits the 6.3/6.5 shape incrementally on push
  (emit-count == changed-blob-count, 0 missed/stale); the unit + chained-e2e + CDC tests pass; the emitter
  mutation score is measured; the GF-3 floor is named with its follow-on (GIT-P33); the SetExpr-list follow-on
  (GIT-P26) is named; the work is committed.
- **COMMIT.** Header: P-<NNN> M3: code-projection emitter for search (declare_indexable emit). Body lists:
  contracts 6.3/6.5 implemented; the projection emit greened (per-blob, incremental, measured); the emitter
  mutation score measured; GF-3 floor named with follow-on GIT-P33; the SetExpr-list follow-on named (GIT-P26).
  Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P26 — Leak-free fast repo/PR lists + the code-search pre-filter (the list_objects SetExpr push-down, GIT-D11)

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G5 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G5", the
  SetExpr-push-down half — GIT-D11).
- **DEPENDS-ON.** GIT-P25 (the code projection the search pre-filter conjoins), GIT-P16 (the PR/repo entities
  the lists scan), GIT-P18 (project() the lists return through). The M1 Identity list_objects SetExpr push-down
  (4.3 — the critical dependency). The M2 Search prompts that ship query conjoining the list_objects Filter
  (6.1). The index places this after GIT-P25 and the M1 Identity + M2 Search work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (search references any artifact);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — 0 leak + one query are quantified
    gates).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md (the
    list_objects consumer — SetExpr → SQL JOIN); 00-overview.md §0.1 Δ5 (the SetExpr push-down —
    Ids|Filter{set_expr,zookie}, via_column lowering to repo.id/pr.id, the JOIN against Identity's per-tenant
    authz reverse index, no N+1/post-filter).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-E (the SetExpr
    push-down).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 4.3 (list_objects → Ids |
    Filter{set_expr, zookie}; the SetExpr lowered to a SQL JOIN over the consumer's own id column via the
    per-tenant authz reverse index; no N+1, no post-filter), 6.1 (query always conjoins the list_objects Filter
    before scoring; search-requires-acl-filter lint).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G5 + §2 (★ row 4.3) + §4 (row 4.3).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row GIT-D11 (partial-visibility PR list
    via the SetExpr JOIN) + the shared SRCH-D1/D3 (confidential code never in any result).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - The list_objects SetExpr push-down wired for repo/PR lists AND the code-search pre-filter (4.3, OQ-E): lower
    the Ids | Filter{set_expr, zookie} to a SQL JOIN over git's own id column (repo.id / pr.id) against
    Identity's per-tenant authz reverse index — NO N+1, NO post-filter. ALWAYS conjoined before scoring
    (search-requires-acl-filter lint).
  - FLOOR named: none (the GF-3 trigram floor was named in GIT-P25). State that git's code projection is
    asserted leak-free in the shared SRCH-D1/D3 here.
- **CONTRACTS TO IMPLEMENT.** 4.3 the list_objects consumer (owned — the SetExpr → SQL JOIN over repo.id/pr.id),
  6.1 query Filter conjoin (consumed — the code-search pre-filter). Implement to the frozen shapes; escalate a
  needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - GIT-D11 (SCHED): a viewer with PARTIAL repo/PR visibility lists a 100k-PR tenant → the SetExpr JOIN returns
    ONLY VISIBLE ROWS (0 leak), in ONE QUERY (no N+1, no post-filter); a just-revoked grant is reflected within
    the zookie bound. Green artifact: the 0-leak + 1-SQL-query + revoke-latency signals.
  - The search-requires-acl-filter lint green (the code-search query always conjoins the list_objects Filter;
    0 unfiltered scoring paths) — CI. Feeds the shared SRCH-D1/D3 (confidential code never in any result incl.
    counts/IDF) — git's projection asserted leak-free there.
- **TESTS (required).** Unit tests for the SetExpr → SQL JOIN lowering (via_column = repo.id/pr.id; one query;
  no post-filter). A chained e2e test (EI-01 §4): grant partial visibility → list PRs → assert 0 leak + one
  query → revoke → assert reflected. The CDC pair for row 4.3. The SetExpr-lowering path is mandatory-core (a
  leak is the failure): state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** The SetExpr JOIN exists for repo/PR lists + the code-search pre-filter; GIT-D11 emits
  its dated green artifact (0 leak, 1 SQL query, revoke reflected); the search-requires-acl-filter lint is
  green; the unit + chained-e2e + CDC + drill tests pass; the SetExpr-lowering mutation score is measured; the
  work is committed.
- **COMMIT.** Header: P-<NNN> M3: leak-free SetExpr push-down for repo/PR lists + code-search pre-filter
  (GIT-D11). Body lists: contracts 4.3/6.1 implemented; GIT-D11 greened (0 leak, 1 query, measured); the
  search-requires-acl-filter lint green; the SetExpr-lowering mutation score measured. Branch first if on
  default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P27 — Code-executing git tools (history-rewrite, SCIP indexing) on the unified sandbox (the AG-D4 gate)

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G6 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G6", the
  code-executing-tools-on-the-unified-sandbox half).
- **DEPENDS-ON.** GIT-P21 (the merge tool the git.merge ToolDef gates), GIT-P16 (the PR surface). The M2 Agent
  fabric prompts that ship ToolSurface + EffectApi + ToolHands::exec the unified sandbox (8.1/8.2/8.4) — and the
  HARD upstream gate: AG-D4 / CI-T1 (the real-kernel sandbox-escape GATE) GREEN on the production backend. The
  M1 reserve/settle cost gate (11.7), Id mint_run_token (4.7). The index places this after GIT-P21 and ONLY
  after AG-D4 is green (master M2→M3 gate — no code-executing git tool runs until then).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native; the four uniform sandbox guarantees by construction);
    ../../external-insights/01-process-and-quality-doctrine.md §2 (RCE/sandbox-escape outranks features — the
    AG-D4 gate); §3 (prove-it — the four uniform guarantees by construction, never re-implemented).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md §7 (git
    ToolDefs with the frozen requires_approval defaults; the code-executing tools on the unified sandbox);
    00-overview.md §0.1 Δ8 (git.merge=yes, open_pr=no; the four uniform sandbox guarantees inherited by
    construction), §4 (inherited non-negotiable 8).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-6 (the
    requires_approval defaults + the four uniform sandbox guarantees), §9 (history-rewrite audited op).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 8.1 (ToolSurface::register_tool
    + the frozen requires_approval defaults: git.merge=yes, open_pr=no), 8.2 (EffectApi::apply plan-then-apply),
    8.4 (ToolHands::exec the unified sandbox = the CI runner's kind=agent job; the four uniform guarantees; the
    real-kernel escape drill gates both), 11.7 (reserve/settle fronts every run), 4.7 (mint_run_token), 10.6
    (history-rewrite as an audited op — built here as the TOOL; erasure semantics complete at GIT-P29).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G6 + §1 (sandbox escape is the shared AG-D4
    gate; git must not run code-executing tools until it is green) + §4 (rows 8.1/8.4, 10.6) + §5 (GF-9 MCP
    floor).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md AG-D4 (re-run on the git tool image).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - Register git's ToolDefs with the FROZEN requires_approval defaults (8.1, X-6): git.merge = YES, git.open_pr
    = NO. Code-executing tools (history-rewrite, SCIP indexing) go through EffectApi::apply (plan-then-apply)
    and ToolHands::exec (= the CI runner's kind=agent job, 8.4) — inheriting the FOUR UNIFORM GUARANTEES
    (reserve/settle cost gate, per-run attenuated token, HITL withhold, isolation floor + the real-kernel escape
    drill) BY CONSTRUCTION, never re-implemented.
  - The history-rewrite erasure path as an audited, rate-limited tenant op (10.6) with fork/mirror/clone-cache
    invalidation fan-out (built here as the TOOL; its erasure SEMANTICS complete at GIT-P29).
  - FLOOR named: GF-9 — exposed_over_mcp flags set, no external endpoint (the platform MCP server + threat model
    is the follow-on, GIT-P33/P6+Legal). Name it. State that agents-as-first-class-authors/reviewers is GIT-P28.
- **CONTRACTS TO IMPLEMENT.** 8.1 git ToolDefs (owned — the requires_approval defaults), 8.2/8.4 EffectApi +
  ToolHands::exec (consumed — git's code-executing tools ride the unified sandbox), 11.7 reserve/settle
  (consumed — fronts every run), 4.7 mint_run_token (consumed — per-run token), 10.6 history-rewrite (owned —
  the audited op TOOL). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - AG-D4 / CI-T1 is GREEN on the git tool image (re-run on the git tool image — the permanent escape gate;
    ZERO escapes). This is the upstream go/no-go: if AG-D4 is red on the git image, NO code-executing git tool
    runs and this prompt does NOT proceed — record a dated claimed-not-proven row and escalate. PERMANENT-gate
    (re-run on every backend/image/kernel change — say so).
  - The no-host-exec lint green (git's tools have no sandbox bypass) — CI.
- **TESTS (required).** Unit tests for the git ToolDef registration (defaults: merge=yes, open_pr=no) and the
  EffectApi plan-then-apply path for the code-executing tools. The AG-D4 re-run on the git tool image. The CDC
  pairs for rows 8.1/10.6. State the cargo-mutants mutation-score floor for the EffectApi-integration module and
  meet it.
- **DEFINITION OF DONE.** Git's ToolDefs are registered with the frozen defaults; the code-executing tools
  (history-rewrite tool, SCIP indexing) ride the unified sandbox with the four guarantees by construction; AG-D4
  is green on the git tool image (PROVEN, not claimed); the no-host-exec lint is green; the unit + CDC + AG-D4
  tests pass; the EffectApi-integration mutation score is measured; the GF-9 floor is named; the
  agents-as-authors follow-on (GIT-P28) is named; the work is committed. A red AG-D4 BLOCKS this prompt — it is
  not greened by weakening the assertion.
- **COMMIT.** Header: P-<NNN> M3: code-executing git tools (history-rewrite, SCIP) on the unified sandbox. Body
  lists: contracts 8.1/8.2/8.4/11.7/4.7/10.6 implemented; AG-D4 re-confirmed green on the git tool image (0
  escapes); the no-host-exec lint green; the EffectApi-integration mutation score measured; GF-9 floor named;
  the agents-as-authors follow-on named (GIT-P28). Branch first if on default; do not push unless asked. End
  with the workspace Co-Authored-By trailer.

---

### GIT-P28 — Agents as first-class authors/reviewers (legible, bounded; HITL on git.merge; AG-D1/D2/D3/D5)

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G6 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G6", the
  agent-authors/reviewers half).
- **DEPENDS-ON.** GIT-P27 (the git ToolDefs + the unified-sandbox integration the agents author through),
  GIT-P16 (the PR surface agents author into). The M2 Agent fabric (8.1/8.2/8.4), reserve/settle (11.7), Id
  mint_run_token (4.7). The index places this after GIT-P27.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native; mock agents only during development — strategy pattern, --use-mock);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — a HITL-gated tool is WITHHELD →
    0 mutation pre-approval, 1 apply post-approval), §8 (the human is the bottleneck — git.merge is HITL).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md §7 (git
    ToolDefs; agents as authors/reviewers via EffectApi); 00-overview.md §0.1 Δ8 (git.merge=yes HITL,
    open_pr=no; the four uniform guarantees).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-6 (the
    requires_approval defaults + the effect-intersection denial).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 8.1 (the requires_approval
    defaults), 8.2 (EffectApi::apply — plan-then-apply; no write outside EffectApi), 8.4 (the four uniform
    guarantees), 4.7 (mint_run_token), 11.7 (reserve/settle).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G6 (the agent-authors bullet) + the Done gate
    (inherits AG-D1/D2/D3/D5).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md AG-D1/D2/D3/D5 (git's tools assert they
    honour them).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - Agents as FIRST-CLASS authors/reviewers (legible, bounded): an agent can open a PR (git.open_pr, no
    approval), comment, review — via EffectApi (8.2), with mock runtimes during development (--use-mock, VISION
    §3 — no real agents integrated in dev). git.merge is HITL-gated (requires_approval=yes): WITHHELD until a
    human approves.
  - FLOOR named: none new (GF-9 named in GIT-P27). State that mock agents only in dev (--use-mock).
- **CONTRACTS TO IMPLEMENT.** 8.1 the requires_approval defaults (owned — git.open_pr=no, git.merge=yes), 8.2
  EffectApi (consumed — agents write only through it), 8.4 the four guarantees (consumed), 4.7 mint_run_token
  (consumed), 11.7 reserve/settle (consumed). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - Inherits AG-D1/D2/D3/D5 (CI): no write outside EffectApi; an effect outside the agent.policy ∩ delegation ∩
    tenant.policy intersection is DENIED; a HITL-gated tool (git.merge) is WITHHELD → 0 mutation pre-approval,
    1 apply post-approval. Green artifact: the per-run effect-attribution + 0-pre-approval-mutation signals.
- **TESTS (required).** Unit tests for the agent-author path (open PR without approval; the
  effect-intersection denial). A chained e2e test (EI-01 §4): a mock agent opens a PR (no approval) → proposes a
  merge (gated) → withhold → assert 0 mutation → approve → assert 1 apply. The CDC pair for row 8.1 (the
  requires_approval defaults). State the cargo-mutants mutation-score floor for the agent-author module and meet
  it.
- **DEFINITION OF DONE.** Mock agents can author/review (open PR, comment, review via EffectApi); AG-D1/D2/D3/D5
  emit their dated green artifacts on git's tools (git.merge withheld → 0 pre-approval mutation, 1 apply
  post-approval); the unit + chained-e2e + CDC + drill tests pass; the agent-author mutation score is measured;
  the work is committed.
- **COMMIT.** Header: P-<NNN> M3: agents as first-class authors/reviewers (HITL on git.merge). Body lists:
  contracts 8.1/8.2/8.4/4.7/11.7 implemented; AG-D1/D2/D3/D5 honoured (git.merge withheld → 0 pre-approval, 1
  apply, measured); the agent-author mutation score measured. Branch first if on default; do not push unless
  asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P29 — Erasure-reaches-every-holder + history-rewrite erasure semantics (the GDPR git-history obligation, GIT-D2 complete)

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G7 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G7", the
  DSR-fan-out / erase-reaches-every-holder + history-rewrite-semantics half — GIT-D2 completed).
- **DEPENDS-ON.** GIT-P9 (the holder H1 + per-subject DEK), GIT-P12 (pseudonymous-by-default — the GIT-1 half),
  GIT-P25 (the search code index this purges/reindexes), GIT-P27 (the history-rewrite tool). The M1 GDPR DSR
  orchestrator (10.1/10.4) + the erasure ledger (10.8). The M1 Storage crypto-shred (11.4). The M1 Id erase
  (4.8). The index places this after GIT-P27.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR — data subject rights; erasure by construction);
    ../../external-insights/04-hard-problems.md §1 (erasure vs immutability — every holder hit, the residual is
    the ONE posture); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — every holder hit
    is quantified).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/05-hard-problems.md §HP-7 (the erasure
    posture completed); 03-events-contracts-and-glue.md §6 (the DSR fan-out over git); 00-overview.md §1.1 (git
    is holder H1, the hardest in the platform).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-7 (the ONE
    posture — instantiate by reference), §9 (history-rewrite audited op + invalidation fan-out).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 10.1 (PersonalDataHolder{locate/
    export/rectify/restrict/erase} over git + metadata), 10.4 (the DSR state machine), 11.4 (per-subject DEK
    crypto-shred — bodies/titles + reflogs/bitmaps/pack-backup reach), 4.8 (erase — the pseudonym-map shred, DSR
    step 1), 10.8 (the erasure ledger), 10.6 (history-rewrite audited op), 10.9 (the ONE posture by reference).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G7 (the DSR-fan-out bullet) + §4 (rows
    10.1/10.4, 10.6, 11.4) + §5 (GF-7 floor — the lawful-basis residual is R-7, parallel-legal).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row GIT-D2 (erase reaches every holder).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate (the PersonalDataHolder
  impl):
  - DSR fan-out over git: pseudonym-map delete (Id, step 1) ⇒ immutable bytes hold only the opaque pseudonym;
    per-subject DEK crypto-shred for PR/review/comment BODIES + TITLES (11.4) reaching live + BACKUPS by
    construction; reflogs / bitmaps / pack-tier backups shreddable via the per-tenant blob DEK; the search code
    index purge+reindex; refs tombstone; cache/CDN invalidation (H9). locate/export/rectify/restrict/erase over
    git + metadata (10.1/10.4).
  - The history-rewrite path (10.6) as the supported disruptive op for PII-in-content (the rare case a body must
    be expunged), with the understood changed-hash consequence + the invalidation fan-out (the TOOL was built in
    GIT-P27; the erasure SEMANTICS complete here).
  - The residual is instantiated BY REFERENCE to the ONE platform posture (10.9 / X-7), NOT restated as a
    git-local statement. The [OPEN — LEGAL] Art. 17 ratification is R-7 (Legal/DPO, parallel).
  - FLOOR named: GF-7 — the structural floor (pseudonymous-by-default + per-subject DEK shred + history-rewrite)
    ships here regardless; the lawful-basis residual is one ratified statement (R-7, parallel-legal, not a code
    gate). Name it. State that the reindex-from-cold parity (GIT-D3) is GIT-P30.
- **CONTRACTS TO IMPLEMENT.** 10.1/10.4 PersonalDataHolder + DSR (owned — locate/export/rectify/restrict/erase
  over git + metadata), 11.4 per-subject DEK crypto-shred (consumed — bodies/titles + reflogs/bitmaps/backups),
  4.8 erase (consumed — the pseudonym-map shred), 10.6 history-rewrite (owned — the audited op semantics), 10.8
  the erasure ledger (consumed), 10.9 the ONE posture (consumed by reference). Implement to the frozen shapes;
  escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - GIT-D2 (SCHED, completed here): erase a subject who authored commits/PRs/comments + uploaded LFS → EVERY
    HOLDER HIT (pseudonym map, per-subject DEK bodies live+backups, reflogs/bitmaps/pack backups, search index,
    refs, cache/CDN); the residual is EXACTLY the ONE platform-posture residual (10.9), nothing more;
    crypto-shred reaches BACKUPS. Green artifact: the DSR receipt set + the erasure-ledger entry (0 holders
    missed; 0 recoverable PII beyond the named residual).
- **TESTS (required).** Unit tests for the holder locate/export/erase over git + metadata and the crypto-shred
  key choice (per-subject DEK for bodies/titles; per-tenant for reflogs/bitmaps). A chained e2e test (EI-01 §4):
  author content → erase subject → assert every holder hit + residual == the ONE posture + backups shredded. The
  CDC pairs for rows 10.1/10.4. The DSR fan-out + crypto-shred path is mandatory-core (a missed holder is a
  breach): state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** The DSR fan-out hits every git holder; GIT-D2 emits its dated green artifact (every
  holder hit, residual == the ONE posture, backups shredded); the history-rewrite erasure semantics are
  complete; the residual is by reference to 10.9; the unit + chained-e2e + CDC + drill tests pass; the DSR
  mutation score is measured; the GF-7 floor is named with its follow-on (R-7, Legal); the reindex-parity
  follow-on (GIT-P30) is named; the work is committed.
- **COMMIT.** Header: P-<NNN> M3: erasure-reaches-every-holder + history-rewrite semantics (GIT-D2 complete).
  Body lists: contracts 10.1/10.4/11.4/4.8/10.6/10.8/10.9 implemented; GIT-D2 greened (every holder hit, backups
  shredded, measured); the DSR-fan-out mutation score measured; GF-7 floor named with R-7 (Legal); the
  reindex-parity follow-on named (GIT-P30). Branch first if on default; do not push unless asked. End with the
  workspace Co-Authored-By trailer.

---

### GIT-P30 — Reindex-from-source parity (cold rebuild byte-matches live; no cross-DB read; GIT-D3)

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G7 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G7", the
  reindex-from-cold-parity half — GIT-D3).
- **DEPENDS-ON.** GIT-P25 (the code index that rebuilds), GIT-P20 (the check_status projection that rebuilds
  from CI's re-emit), GIT-P17/GIT-P19 (the refs edges that rebuild), GIT-P29 (the erase path the reindex
  re-runs after). The Bus reindex-from-source (2.6). The index places this after GIT-P29.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (erasure by construction);
    ../../external-insights/04-hard-problems.md §5 (reindex-from-source — derived stores rebuild, never read
    owner DBs); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — cold == live parity is
    quantified).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md §6
    (reindex-from-source for the check_status projection + code index + refs edges); 00-overview.md §4
    (inherited non-negotiable 6 — reindex is the only recovery path).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 2.6 (reindex-from-source /
    replay), 10.9 (the ONE posture by reference, re the erased residual after reindex).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G7 (the GIT-D3 bullet) + §4 (row 2.6).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row GIT-D3 (reindex-from-cold parity).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate (the reindex path):
  - reindex-from-source (2.6): replay rebuilds the check_status projection (from CI's ci.check.updated re-emit),
    the code index, and the refs edges — cold rebuild BYTE-MATCHES live (one code path, no drift); NO cross-DB
    read. The erased-subject residual after a reindex is EXACTLY the ONE posture's residual (an erased body does
    not resurrect on rebuild).
  - FLOOR named: none.
- **CONTRACTS TO IMPLEMENT.** 2.6 reindex-from-source (owned — the git replay rebuilding the projection + code
  index + refs edges), 10.9 the ONE posture (consumed by reference — the post-reindex residual). Implement to
  the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - GIT-D3 (SCHED): wipe the Search code index + Refs edges + the check_status projection; reindex/replay → cold
    rebuild BYTE-MATCHES live (one code path, no drift); the check_status projection rebuilds from CI's
    ci.check.updated re-emit; NO cross-DB read. Green artifact: the reindex-parity hash (cold == live) + the
    no-cross-db lint green.
  - An erased subject's body does NOT resurrect on reindex (the post-reindex residual == the ONE posture; 0
    resurrected PII) — CI.
- **TESTS (required).** A reindex-parity test (cold rebuild byte-matches live; no cross-DB read). A test that an
  erased body does not resurrect on reindex. The CDC pair for row 2.6. The reindex path is mandatory-core (a
  drift or a resurrected body is the failure): state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** The reindex-from-source path rebuilds the projection + code index + refs edges; GIT-D3
  emits its dated green artifact (cold == live parity, no cross-DB); an erased body does not resurrect (0
  resurrected PII); the no-cross-db lint is green; the unit + CDC + drill tests pass; the reindex mutation score
  is measured; the work is committed.
- **COMMIT.** Header: P-<NNN> M3: reindex-from-source parity (cold == live, no cross-DB; GIT-D3). Body lists:
  contract 2.6 implemented; GIT-D3 greened (cold==live parity, 0 resurrected PII, measured); the no-cross-db
  lint green; the reindex mutation score measured. Branch first if on default; do not push unless asked. End
  with the workspace Co-Authored-By trailer.

---

### GIT-P31 — Git notification rules + humanise (confidential subject → tombstone, title never leaks; NOTIF-D4-class)

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G8 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G8", the
  notifications + humanise half).
- **DEPENDS-ON.** GIT-P16 (PR/review/threads — the notifiable subjects), GIT-P18 (project()/resolve for the
  per-viewer projection humanise tombstones), GIT-P22 (the review-requested / PR-status events). The M2 Notif
  prompts that ship humanise (7.3), define_notif_rule (7.6), the ONE inbox (7.1). The M2 Refs resolve for
  unfurls (5.2). The index places this after GIT-P16/GIT-P18 and the M2 Notif work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (top-of-the-line UX);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — title never leaks is a quantified
    gate; 0 confidential titles in delivered notifications).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/00-overview.md §2 (D) (the notification
    routing); 04-views-cli-and-api.md (the notif surface).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-L (humanise the
    ONE templating surface).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.6 (define_notif_rule —
    review-requested / PR-status / mention), 7.3 (humanise the ONE templating surface — confidential subject →
    humanised tombstone, title never leaks), 7.1 (the ONE inbox — review-requests are a filter, never a second
    store), 5.2 (resolve for unfurls).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G8 (the notifications bullet) + §4 (rows
    7.1/7.3/7.6).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md NOTIF-D4-class (confidential git subject
    → humanised tombstone, title never leaks).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - The git notification rules + targets via Signals (define_notif_rule 7.6: review-requested / PR-status /
    mention); the summary template keys resolved through humanise (7.3) per-viewer (the ONE templating surface —
    confidential subject → humanised tombstone, TITLE NEVER LEAKS). Review-requests appear as a FILTER over the
    ONE inbox (7.1), never a second store. resolve (5.2) for unfurls.
  - FLOOR named: none. State that the Web UI + CLI/API + the M3 band-exit aggregate is GIT-P32.
- **CONTRACTS TO IMPLEMENT.** 7.6 define_notif_rule (owned — git's rules), 7.3 humanise (consumed — the summary
  template keys), 7.1 the ONE inbox (consumed — the review-requests filter), 5.2 resolve (consumed — unfurls).
  Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - NOTIF-D4-class (CI): a confidential PR/commit subject → humanised TOMBSTONE; the title NEVER leaks in the
    notification (0 confidential titles in delivered notifications). Green artifact: the humanise-tombstone
    signal (0 title leaks).
- **TESTS (required).** Unit tests for the notif-rule registration + the humanise template keys (confidential
  subject → tombstone). A chained e2e test: a confidential PR review-requested to a viewer without access →
  assert the delivered notification is a tombstone (0 title). The CDC pairs for rows 7.6/7.3/7.1. The
  notif-rule matcher is mandatory-core: state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** The git notification rules + humanise wiring exist; NOTIF-D4-class emits its dated
  green artifact (0 title leaks); review-requests are a filter over the ONE inbox (not a second store); the unit
  + chained-e2e + CDC tests pass; the notif-rule mutation score is measured; the Web-UI follow-on (GIT-P32) is
  named; the work is committed.
- **COMMIT.** Header: P-<NNN> M3: git notification rules + humanise (title never leaks; NOTIF-D4-class). Body
  lists: contracts 7.6/7.3/7.1/5.2 implemented; NOTIF-D4-class greened (0 title leaks, measured); the notif-rule
  mutation score measured; the Web-UI follow-on named (GIT-P32). Branch first if on default; do not push unless
  asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P32 — The Web UI + CLI/API (driven in a browser) + the M3 producer-band exit aggregate

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G8 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G8", the Web-UI + CLI/API
  + M3-band-exit half — FIRST USEFUL).
- **DEPENDS-ON.** GIT-P16 (PR/review/threads — the UI surface), GIT-P21/GIT-P22 (the merge gate + checks panel
  + fork-trust badge), GIT-P24 (the inline-thread anchors), GIT-P7 (the signed-off design pass), GIT-P31 (the
  notification wiring). The index places this LAST in the M3 git band (it closes the producer-band exit
  aggregate).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (top-of-the-line UX; design sketches precede frontend; the switch test);
    ../../external-insights/05-ux-and-design.md (the design-language bar; overlays/states);
    ../../external-insights/01-process-and-quality-doctrine.md §4 (actually try it — drive the real UI in a
    browser before claiming done).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/04-views-cli-and-api.md (the views — IA
    + flows + states; the two CLI surfaces; the HTTP/RPC + agent-tool API); the signed-off design/ sketches
    (incl. the X-1 affordances from GIT-P7).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G8 (the Web-UI + CLI/API bullet) + §6 (first
    useful = end of M3-G8) + §5 (GF-6 single-file web-edit floor).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md the M3 band-exit aggregate (GIT-D9 +
    GIT-D8 + GIT-D11 + GIT-D7 + GIT-D2).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate + the git web UI app:
  - The Web UI: repo browse, code view, PR/review/inline-thread, the checks panel + fork-trust badge +
    merge-queue affordances (the X-1 design pass from GIT-P7), single-file WEB EDIT + commit (GF-6 floor — no
    3-way conflict editor in v1). Built against the REVIEWED design sketches (VISION §3); DRIVEN IN A BROWSER
    before "done" (the switch-test rehearsal; the full switch test is GIT-P35/M6).
  - The myelin CLI git surface + the HTTP/RPC + agent-tool API (arch 04).
  - FLOOR named: single-file web edit (GF-6 — in-browser conflict resolution is the follow-on, GIT-P33/M5+).
    Name it.
- **CONTRACTS TO IMPLEMENT.** None new (the notif/humanise contracts are GIT-P31; the views consume the already
  -built project()/resolve). The CLI/API surfaces the existing handlers. State that.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The Web UI is DRIVEN IN A BROWSER (EI-01 §4 — the switch-test rehearsal): repo browse, PR review, the checks
    panel + fork-trust badge render against the signed-off sketches; the overlays/states (empty/loading/error)
    render correctly; no off-screen-picker / clipped-dialog regression (the shared overlay primitives hold).
    Recorded yes/no/partial per surface (untested-but-named is acceptable; silent skipping is not).
  - THE M3 BAND EXIT AGGREGATE: GIT-D9 + GIT-D8 + GIT-D11 + GIT-D7 + GIT-D2 are all GREEN (the master §2 M3 git
    exit) ⇒ M3 done for git, M4 may start. Confirm each rests on a dated green artifact (the truth-up check).
- **TESTS (required).** A browser-driven e2e walkthrough (EI-01 §4) of repo browse → PR review → checks panel →
  web edit + commit, recorded yes/no/partial. The M3 band-exit aggregate confirmation (each of GIT-D9/D8/D11/D7/
  D2 rests on a dated green artifact). State the cargo-mutants mutation-score floor for any mandatory-core
  module touched (web-edit commit path) and meet it.
- **DEFINITION OF DONE.** The Web UI + CLI/API exist; the Web UI is driven in a browser with states recorded;
  the M3 band-exit aggregate (GIT-D9/D8/D11/D7/D2) is confirmed all-green-and-dated; the browser-e2e tests pass;
  the GF-6 floor is named with its follow-on (GIT-P33); the work is committed. This is FIRST USEFUL (roadmap
  §6): a team could host real repositories and review code (still on the local-disk/single-cell/single-lane
  floors; the X-1 seam fully live at M4).
- **COMMIT.** Header: P-<NNN> M3: git Web UI + CLI/API (the M3 producer-band exit). Body lists: the Web UI
  driven in a browser (states recorded); the M3 band-exit aggregate confirmed all-green (GIT-D9/D8/D11/D7/D2);
  GF-6 floor named with follow-on GIT-P33. Branch first if on default; do not push unless asked. End with the
  workspace Co-Authored-By trailer.

---

### GIT-P33 — World-scale floor follow-ons: object-backed packs, cross-cell replication, speculative queue, SHA-256, SCIP

- **BAND.** M5.
- **ROADMAP MILESTONE.** M5-G9 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M5-G9", the named floor
  follow-ons half; the world-scale surge drills + E2E slices are GIT-P34).
- **DEPENDS-ON.** GIT-P8..GIT-P32 (all the M3 floors this promotes). The M4 exit GREEN (all five subsystems
  exist; the deterministic correctness drills green — master M4→M5 gate). The M5 Storage object-store BlobStore
  swap (11.2). The M5 multi-cell bridge (12.6, OQ-I). The index places this in M5 after the M4 exit.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale means world-scale; name-your-floors — the follow-on lands as its own
    committable unit); ../../external-insights/04-hard-problems.md §3 (world-scale git — the explicit local-disk
    → object-backed transition, sequenced not bolted on).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/05-hard-problems.md (object-backed
    packs, replication, the SHA-256 flip, the speculative queue — the named floors); 02-internals-and-algorithms.md
    (replication TE-24, GC/repack, the merge queue); 01-tech-and-data-model.md §1 (the BlobStore fs↔object swap,
    the hash-agnostic model).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §8 (the
    object-backed pack/delta seam, the within-EU CDN clone class, the trust-scoped cache), OQ-I (the cross-cell
    bridge).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 11.2 (BlobStore object-backed
    pack/delta seam + the within-EU CDN clone/bundle class; the fs↔object one-line swap), 12.2 (repo-granular
    relocatable placement — a BlobStore-impl swap + a transport path, not a data-model rewrite), 12.6 (the
    cross-cell PII-free pointer bridge), 6.5 (SCIP/LSIF follow-on).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M5-G9 (the named floor follow-ons) + §5 (the
    floors register — GF-1/GF-2/GF-2b/GF-3/GF-5/GF-8 follow-ons; the OQ-1 gitoxide spike, OQ-4/OQ-5/OQ-9 the
    investigations).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate (each follow-on its own
  bounded, committable slice; the prompt is executed as a sequence of commits, each follow-on named done with
  its own gate):
  - Object-backed git packs (GF-1 → R-1/OQ-4): authoritative pack bytes move from node-local NVMe to the object
    store behind BlobStore — delta/pack management, sharding, replication, the smart-transport read path from
    object-tier blobs, the within-EU CDN clone/bundle class (11.2). The EXPLICIT local-disk → object-backed
    transition (EI-04 §3) — a BlobStore-impl swap + a transport path, not a data-model rewrite (repos were never
    node-pinned, 12.2). The quorum-ack protocol + fencing (update_seq) + object pack layout is OQ-4.
  - Cross-cell / multi-region replication (GF-2): cross-cell active replica sets within-EU; geo read-replicas;
    the single-cell floor's primary+quorum lifts to multi-cell (rides the OQ-I cross-cell bridge, 12.6).
  - Speculative/parallel merge-queue batching (GF-8 → OQ-5): promote from single-lane once the promotion trigger
    is MEASURED.
  - SHA-256 default flip (GF-2b → OQ-9): flip new-repo default from SHA-1+sha1dc to SHA-256 once the
    stock-client/tooling bar is met — a default-change, not a migration (hash-agnostic model).
  - Patch-id-chain anchor carry-over (GF-5 → R-6): a thread follows a rebased hunk through a multi-commit rebase.
  - SCIP/LSIF "find usages" (GF-3 → R-3): AST-aware code intelligence fed by CI-produced SCIP indices.
  - The gitoxide server-side migration spike (OQ-1 → R-2): per-op, gated on the capability-matrix spike + a
    protocol-compat + sandbox-escape re-drill — iff it clears (a NAMED SPIKE, not a guaranteed deliverable;
    record the verdict).
  - In-browser conflict resolution (GF-6 → OQ-8) — measured-trigger; External MCP endpoint (GF-9 → R-9,
    P6+Legal) — record as deferred with its follow-on owner.
  - FLOOR named: each promotion is itself a named, dated transition in the floors register; the gitoxide spike
    and the MCP endpoint remain named floors if not cleared here.
- **CONTRACTS TO IMPLEMENT.** 11.2 the object-backed BlobStore pack tier + CDN clone class (owned — the swap),
  12.6 the cross-cell bridge (consumed — multi-cell replication), 6.5 SCIP/LSIF (owned — the follow-on
  projection). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - GIT-D4 (SCHED): grow a synthetic monorepo until partial-clone/sparse/bitmaps degrade → DOCUMENTED v1 CEILING
    (measured, not guessed); clone/fetch p99 held below it. Green artifact: the ceiling numbers + clone p99.
  - GIT-D5 (SCHED): concurrent merges + force-push to one protected base_ref + DB-replica failover + node
    recovery mid-merge → LINEARIZABLE on the ref CAS; NO split-brain; 0 LOST MERGE; update_seq monotonic + the
    fence honoured. Green artifact: 0 conflicting tips + the reconcile log.
  - The object-backed pack swap holds: a clone/fetch served from object-tier blobs byte-matches the local-disk
    path (smart-transport parity) — CI.
- **TESTS (required).** Unit tests for the object-backed pack read/write path, the quorum-ack + fencing protocol,
  and the SHA-256 default flip (a new repo defaults SHA-256; existing repos unchanged). A chained e2e test:
  relocate a repo across the BlobStore swap → clone parity. The CDC pairs for rows 11.2/12.6/6.5. The
  GIT-D4 + GIT-D5 drill scenarios. The replication + fencing path is mandatory-core (a lost merge is the
  failure): state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** The object-backed packs + cross-cell replication + speculative queue + SHA-256 flip +
  patch-id-chain + SCIP follow-ons are shipped (each named done with its gate); GIT-D4 emits its dated ceiling
  artifact and GIT-D5 its linearizable-no-split-brain-0-lost-merge artifact; the object-backed swap byte-matches
  the local-disk path; the gitoxide spike verdict + the MCP-endpoint deferral are recorded; the unit +
  chained-e2e + CDC + drill tests pass; the work is committed. Any follow-on NOT cleared here (gitoxide, MCP)
  remains a NAMED floor with its owner — silent skipping is the only failure.
- **COMMIT.** Header: P-<NNN> M5: git world-scale floor follow-ons (object-backed packs, cross-cell, SHA-256,
  SCIP). Body lists: contracts 11.2/12.6/6.5 implemented; GIT-D4 (ceiling, measured) + GIT-D5 (linearizable,
  0 lost merge, measured) greened; the replication mutation score measured; each floor promotion dated; the
  gitoxide spike verdict + MCP deferral recorded. Branch first if on default; do not push unless asked. End with
  the workspace Co-Authored-By trailer.

---

### GIT-P34 — World-scale hardening (the F6 surge family, GIT-D6) + git's slices of the four whole-system E2E scenarios

- **BAND.** M5.
- **ROADMAP MILESTONE.** M5-G9 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M5-G9", the world-scale
  hardening + the E2E-slice half).
- **DEPENDS-ON.** GIT-P33 (the promoted floors — surge drills run against the object-backed/cross-cell git). The
  M4 exit GREEN (CI's CheckStatus producer closed the X-1 seam end-to-end — GIT-D10/CI-D8). The M5 prompts of
  the other four subsystems (CI/Issues/Chat/Knowledge) + Refs/Search/Id/Notif for the cross-subsystem E2E
  wedge. The index places this after GIT-P33 and the M4 seam closure.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale; agent-native — the E2E-2 flagship);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (the failure-injection harness — the 1×/10×/30×
    surge, the telemetry-assertion library; observability is part of the pass), §4 (chained-mutation E2E, not
    single handlers).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/07-drills-and-open-questions.md (the
    quantified drills owed); 02-internals-and-algorithms.md (the clone-storm shed, the monorepo ceiling).
  - Testing strategy: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    §2 (the four E2E scenarios E2E-1..E2E-4; git's slices) + the F6 surge family rows; README.md (the strategy).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 1.8 (the telemetry signal set —
    the surge survival signals), 1.11 (the shed order under surge), 11.5 (restore-verify at cell scale, STOR-D2).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M5-G9 (the F6 surge family + the E2E slices) +
    §6 (production-hardened = end of M5-G9).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md rows GIT-D6 (the 30× clone surge) +
    E2E-1/E2E-2/E2E-3 (git's slices) + STOR-D2 (cell scale).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate + the E2E test harness:
  - The world-scale hardening: the monorepo-ceiling benchmark; concurrent-merge linearizability under failover
    (built on GIT-P33's replication); the CLONE-STORM SHED (the OQ-K per-surface shed budget tuned by GIT-D6);
    cross-tenant fairness; prod-scale benchmarks (100k-PR list, monorepo ceiling); online-migration-under-load;
    restore-verify at cell scale.
  - Git's contribution to the four whole-system E2E scenarios (testing-strategy §2): E2E-1 PR context pane (git
    is the PR host + the reference producer); E2E-2 CI-fail → triage agent → issue → chat → fix-PR (git hosts
    the fix-PR; the git.merge HITL approval + the X-1/GIT-D10 gate + git.pr.merged closing the issue via the
    Closes trailer — the agent-native flagship); E2E-3 Spec-to-ship traceability (git provides the
    commit→PR→merge lineage; cold-reindex == live).
  - FLOOR named: none new — this prompt PROVES the promoted floors under load; record any residual surge-budget
    tuning as a dated note.
- **CONTRACTS TO IMPLEMENT.** 1.8 the telemetry survival signals (consumed — the surge assertions read from the
  metrics port), 1.11 the shed order under surge (consumed). No new owned contract — this is the drill +
  E2E-slice prompt. Implement the drill scenarios against the failure-injection harness.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - GIT-D6 (SCHED): 30× agent/CI clone surge on a hot repo → HUMAN fetch p99 HELD; agent/CI lane SHEDS (429 +
    Retry-After); 0 cross-tenant starvation; the CDN hit-rate measured. Green artifact: the shed-counts + fetch
    p99 + CDN-hit signals (human lane within budget, agent sheds, cross-tenant impact 0).
  - E2E-1, E2E-2, E2E-3 GREEN (their git slices): each emits its named green artifact (testing-strategy §3.4).
    E2E-2 (the flagship): the HITL merge approval gates, the X-1 gate holds, git.pr.merged closes the issue —
    exactly-once HITL + merge, 0 leak.
  - STOR-D2 at cell scale RE-CONFIRMED (RPO/RTO under world-scale load — the permanent restore-verify gate) —
    SCHED.
- **TESTS (required).** The GIT-D6 surge drill scenario (1×/10×/30×, mixed principal kinds) on the
  failure-injection harness. The git slices of the E2E-1/E2E-2/E2E-3 chained-mutation scenarios against a full
  cell with mock agents. The STOR-D2 re-confirmation at cell scale. State that these are drill/E2E scenarios
  (not unit logic) — the proof is the dated green artifact from the harness telemetry, not a new mutation floor.
- **DEFINITION OF DONE.** GIT-D6 emits its dated green artifact (human lane held, agent shed, cross-tenant 0);
  E2E-1/E2E-2/E2E-3 git slices are green (each with its named artifact, the E2E-2 flagship proving
  exactly-once HITL+merge); STOR-D2 is re-confirmed at cell scale; the surge drills run on the harness; any
  residual surge-budget tuning is recorded as a dated note; the work is committed. This is PRODUCTION-HARDENED
  (roadmap §6). No threshold is weakened to manufacture a green.
- **COMMIT.** Header: P-<NNN> M5: git world-scale hardening (F6 surge) + the E2E slices. Body lists: GIT-D6
  greened (fetch p99 held, agent shed, cross-tenant 0, measured); E2E-1/E2E-2/E2E-3 git slices green;
  STOR-D2 re-confirmed at cell scale; any residual surge-budget tuning dated. Branch first if on default; do
  not push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P35 — Dogfood: Myelin hosts its own repositories (the switch test)

- **BAND.** M6.
- **ROADMAP MILESTONE.** M6-G10 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M6-G10").
- **DEPENDS-ON.** GIT-P34 (git is world-scale-ready and the E2E wedge is proven). The M5 exit GREEN (the
  platform is world-scale-ready — master M5→M6 gate; you do not dogfood real team data onto a substrate whose
  restore-verify and DSAR fan-out are not green). The M6 CI/Issues/Knowledge dogfood prompts (the self-hosting
  CI graph + the roadmap-as-issues). The index places this last in the git ledger.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (the switch test — top-of-the-line UX, driven in a browser); §5 (dogfooding);
    ../../external-insights/01-process-and-quality-doctrine.md §4 (the switch test — drive the real UI, not the
    feature list); §1 (the truth-up pass — every PROVEN row rests on a dated green artifact).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/04-views-cli-and-api.md (the views the
    switch test drives); 06-reconciliation-compliance.md (the conformance map the truth-up confirms).
  - Testing strategy: ../05-refined-shared-systems-architecture/testing-strategy/00-philosophy-levels-and-gates.md
    (the frontend done-bar L5 — the switch test; design-language §8b — measured contrast + latency + overlays).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 13.1 (render(parse(md)) === md —
    the round-trip the switch test measures), 1.6 (the lints + the mandatory-core mutation gate now run as
    Myelin CI jobs on every Myelin commit — the dogfood loop).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M6-G10 + §6 (the done-bar).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md Git OQ-12 switch test.
- **DELIVERABLE (what to build + exactly where in the repo).**
  - Migrate the Myelin monorepo onto Myelin git hosting; the build/test/lint/mutation pipeline becomes a Myelin
    CI pipeline (the gates — the twelve lints + the mandatory-core cargo-mutants gate — now run as Myelin CI jobs
    ON THE PLATFORM'S OWN GIT COMMITS).
  - The roadmap + gap report live as Myelin issues + a Myelin Knowledge space; the every-incident-adds-a-drill
    loop files a Myelin issue + a reproducing git drill.
  - The truth-up pass (EI-01 §1): confirm every PROVEN git row (GIT-D1..GIT-D11, the E2E slices) rests on a
    DATED green artifact — fix any doc that disagrees with the running code (the code wins), then proceed.
  - FLOOR named: none — this is the done-bar. Record any switch-test wall found (a place the old tool did
    better) as a dated gap-report item with its follow-on owner.
- **CONTRACTS TO IMPLEMENT.** No new contract — this prompt drives the real UI + closes the dogfood loop + runs
  the truth-up pass. The mandatory-core mutation gate (1.6) now runs as a Myelin CI job.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - Git OQ-12 switch test (SCHED): DRIVEN IN A BROWSER — could a GitHub user move to Myelin git hosting WITHOUT
    HITTING A WALL the old tool didn't have (EI-01 §4)? Measured: contrast + latency budgets +
    render(parse(md)) === md + overlays against the real anchor (design-language §8b). Green artifact: the
    switch-test scorecard (measured contrast/latency within budget; round-trip 100%; no regressed overlay).
  - The Myelin self-hosting CI graph is GREEN on the platform's own git commits (the dogfood loop is live) —
    SCHED.
  - NO later-band git gate is red (the truth-up pass confirms every PROVEN git row rests on a dated green
    artifact) — the gate invariant holds end-to-end.
- **TESTS (required).** The browser-driven switch-test walkthrough (EI-01 §4, recorded yes/no/partial per
  surface, with the measured contrast/latency/round-trip/overlay numbers). The Myelin CI graph green on a real
  platform commit. The truth-up audit (every PROVEN row → its dated artifact). No new mutation floor — the
  dogfood CI graph runs the existing mandatory-core gate on every commit.
- **DEFINITION OF DONE.** The Myelin monorepo is hosted on Myelin git; the self-hosting CI graph is green on the
  platform's own commits; the Git OQ-12 switch test passes driven in a browser (measured contrast/latency/
  round-trip/overlays within budget); the truth-up pass confirms no later-band git gate is red (every PROVEN row
  dated); any switch-test wall is recorded as a dated gap-report item; the work is committed. This is THE
  DONE-BAR for git hosting (roadmap §6).
- **COMMIT.** Header: P-<NNN> M6: dogfood — Myelin hosts its own repositories (the switch test). Body lists:
  the monorepo migrated; the self-hosting CI graph green on platform commits; Git OQ-12 switch test passed
  (measured contrast/latency/round-trip, browser-driven); the truth-up pass confirmed (0 red later-band git
  gates); any switch-test wall recorded. Branch first if on default; do not push unless asked. End with the
  workspace Co-Authored-By trailer.

---
## Coverage matrix (this file → the consolidated index verifies it is total)

| Roadmap milestone (planning/06-roadmaps/subsystems/git-hosting.md) | Band | Prompt(s) | Primary drills / gates greened |
|---|---|---|---|
| Pre-work M1 — ReBAC fragment | M1 | GIT-P1 | fragment compiles in the cell schema |
| Pre-work M1 — git.* event tokens | M1 | GIT-P2 | §6.2 grammar parse (0 ungrammatical) |
| Pre-work M1 — holder H1 intent + #[personal_data] tags | M1 | GIT-P3 | no-untagged-personal-data lint (0 untagged) |
| Pre-work M2 — #sub mints | M2 | GIT-P4 | #sub kinds registered with Refs (grammatical) |
| Pre-work M2 — declare_indexable spec | M2 | GIT-P5 | spec registered with Search (6.3 shape) |
| Pre-work M2 — X-1 CheckStatus consumer declaration | M2 | GIT-P6 | check_status schema compiles vs frozen 5.9 |
| Pre-work M2-design — design pass + X-1 affordances | M2 | GIT-P7 | design sign-off (fork-trust UX approved, dated) |
| M3-G1 — GitCore layered seam | M3 | GIT-P8 | seam smoke (canonical-wire + gix-read); no-host-exec lint |
| M3-G1 — receive-pack → one-tx ref-CAS + outbox | M3 | GIT-P9 | GIT-D9 (0 ghost / 0 lost) |
| M3-G1 — per-ref aggregate ordering at push QPS | M3 | GIT-P10 | GIT-D1 (push order per ref) |
| M3-G1 — pack/delta storage on the local-NVMe BlobStore floor | M3 | GIT-P11 | clone round-trip byte-identical; residency-pin lint |
| M3-G1 — pseudonymous-by-default commits (the data-model gate) | M3 | GIT-P12 | GIT-D2 (GIT-1 half — 0 cleartext PII) |
| M3-G2 — front door: authenticate/check/placement/residency | M3 | GIT-P13 | GIT-D8 (0 cross-tenant read) |
| M3-G2 — Git ReBAC fragment live + FailStatic on Id | M3 | GIT-P14 | live-fragment enforce; fail-static degrade (just-revoked denied) |
| M3-G2 — protected-human-lane shed order + CDN bundle-URI floor | M3 | GIT-P15 | shed order (human served, agent sheds); bundle-URI clone valid |
| M3-G3 — PR/review/thread lifecycle + rulesets + CODEOWNERS | M3 | GIT-P16 | lifecycle (0 illegal); CODEOWNERS (0 mis-resolved) |
| M3-G3 — bodies on myelin-content + content-node edges | M3 | GIT-P17 | render==md (100%); 1-edge-per-node |
| M3-G3 — project(ref, viewer) + ArtifactRef id grammar | M3 | GIT-P18 | project() per-viewer (unauthorized → tombstone, 0 leak) |
| M3-G3 — typed-edge mirror | M3 | GIT-P19 | lifecycle edges (0 dup/missed) |
| M3-G4 — check_status projection + run_attempt supersession | M3 | GIT-P20 | GIT-D10 (a) (1 current row/key); no-cross-sync-cycle lint |
| M3-G4 — merge gate + required-set policy | M3 | GIT-P21 | required-set gate (0 under-gated merges) |
| M3-G4 — fork / trust-tier endorsement gate | M3 | GIT-P22 | GIT-D10 (b)+(c) (fork-neutral, endorse-flips) |
| M3-G4 — merge queue durable workflow | M3 | GIT-P23 | GIT-D10 (d) + the full GIT-D10 aggregate (merge-count==1) |
| M3-G4 — content-anchored line ranges | M3 | GIT-P24 | GIT-D7 (0 mis-anchored) |
| M3-G5 — code-projection emitter | M3 | GIT-P25 | projection emit (per-blob, incremental) |
| M3-G5 — leak-free SetExpr lists + code-search pre-filter | M3 | GIT-P26 | GIT-D11 (0 leak, 1 query); search-requires-acl-filter lint |
| M3-G6 — code-executing tools on the unified sandbox | M3 | GIT-P27 | AG-D4 (git tool image, 0 escapes); no-host-exec lint |
| M3-G6 — agents as first-class authors/reviewers | M3 | GIT-P28 | AG-D1/D2/D3/D5 (git.merge withheld → 0 pre-approval) |
| M3-G7 — erasure-reaches-every-holder + history-rewrite semantics | M3 | GIT-P29 | GIT-D2 (complete — every holder hit, backups shredded) |
| M3-G7 — reindex-from-source parity | M3 | GIT-P30 | GIT-D3 (cold == live; no-cross-db lint) |
| M3-G8 — notifications + humanise | M3 | GIT-P31 | NOTIF-D4-class (0 title leaks) |
| M3-G8 — Web UI + CLI/API; the M3 band exit | M3 | GIT-P32 | browser-driven; the M3 exit aggregate (D9/D8/D11/D7/D2) |
| M5-G9 — object-backed packs, cross-cell, speculative queue, SHA-256, SCIP | M5 | GIT-P33 | GIT-D4, GIT-D5 |
| M5-G9 — the F6 surge family + the E2E slices | M5 | GIT-P34 | GIT-D6, E2E-1/E2E-2/E2E-3, STOR-D2 |
| M6-G10 — dogfood + the switch test | M6 | GIT-P35 | Git OQ-12 switch test; self-hosting CI graph green |

Every GIT-D drill is greened by a prompt: GIT-D1 (GIT-P10), GIT-D2 GIT-1-half (GIT-P12) + complete (GIT-P29),
GIT-D3 (GIT-P30), GIT-D4 (GIT-P33), GIT-D5 (GIT-P33), GIT-D6 (GIT-P34), GIT-D7 (GIT-P24), GIT-D8 (GIT-P13),
GIT-D9 (GIT-P9), GIT-D10 parts (a) (GIT-P20) + (b)/(c) (GIT-P22) + (d)/aggregate (GIT-P23), GIT-D11 (GIT-P26);
the shared families AG-D4 (GIT-P27), AG-D1/D2/D3/D5 (GIT-P28), NOTIF-D4-class (GIT-P31), SRCH-D1/D3 fed
(GIT-P26), STOR-D1 upstream-gate (GIT-P9), STOR-D2 cell-scale (GIT-P34), E2E-1/2/3 (GIT-P34), Git OQ-12
switch test (GIT-P35).

Floors (each named in its prompt with its follow-on prompt): GF-1 local-disk packs (GIT-P11) → object-backed
(GIT-P33); GF-2 single-cell (GIT-P11) → cross-cell (GIT-P33); GF-2b SHA-1+sha1dc (GIT-P11) → SHA-256 flip
(GIT-P33); GF-3 trigram search (GIT-P25) → SCIP find-usages (GIT-P33); GF-4 large-but-normal monorepo (GIT-P11)
→ Mononoke-class (M5+, GIT-D4-triggered); GF-5 per-pair anchor (GIT-P24) → patch-id-chain (GIT-P33); GF-6
single-file web edit (GIT-P32) → in-browser conflict (GIT-P33); GF-7 pseudonymous-by-default + DEK shred +
history-rewrite (GIT-P12/GIT-P29) → the [OPEN — LEGAL] lawful-basis residual (R-7, parallel/Legal); GF-8
single-lane queue (GIT-P23) → speculative queue (GIT-P33); GF-9 MCP flags (GIT-P27) → platform MCP server
(GIT-P33/P6+Legal); the X-1 seam-floor — synthetic producer (GIT-P6 declared, GIT-P20/P23 built) → real CI
producer end-to-end at the M4 exit (GIT-D10/CI-D8); the OQ-K per-surface shed-budget floor (GIT-P15) → tuned by
GIT-D6 (GIT-P34); the CDN bundle-URI floor (GIT-P15) → full within-EU CDN class (GIT-P33); the OQ-1 gitoxide
server-side spike (GIT-P8 named) → per-op verdict (GIT-P33).

The two named spikes the roadmap schedules (not floors — investigations): OQ-1 (gitoxide server-side capability
matrix — named in GIT-P8, verdict recorded in GIT-P33) and OQ-10/R-8 (pseudonym enforcement mode:
client-cooperative sha-stable vs server-side rewrite-at-push — the PROPERTY is decided and the default is
recorded with rationale in GIT-P12).
