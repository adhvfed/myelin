# Phase 7 — Prompt Ledger: Git Hosting & Code Review (the producer subsystem)

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
> Coverage: pre-work M1 → GIT-P1; pre-work M2 → GIT-P2; M3-G1 → GIT-P3 + GIT-P4; M3-G2 → GIT-P5; M3-G3 →
> GIT-P6; M3-G4 → GIT-P7 + GIT-P8; M3-G5 → GIT-P9; M3-G6 → GIT-P10; M3-G7 → GIT-P11; M3-G8 → GIT-P12; M5-G9 →
> GIT-P13 + GIT-P14; M6-G10 → GIT-P15. Fifteen prompts, no milestone gap.

---

### GIT-P1 — Freeze the Git ReBAC fragment, the git.* event tokens, and the PersonalDataHolder tags (so dependents compile)

- **BAND.** M1.
- **ROADMAP MILESTONE.** Pre-work M1 (planning/06-roadmaps/subsystems/git-hosting.md §3.0 "Pre-work in M1/M2",
  the M1 freeze-so-dependents-compile slice).
- **DEPENDS-ON.** The M0 substrate prompts that lay down the Cargo workspace + the eight glue-crate skeletons +
  the twelve lints + the contract-coverage scanner (master §2 M0; substrate roadmap SUB-M0). The M1 Identity
  prompts that ship the ReBAC namespace engine (contract 4.9) into which fragments compile, and the Bus event
  taxonomy seed (2.9). The index places this alongside the Identity M1 work (Identity must accept the fragment).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md (always) §3 (name-your-floors, GDPR-safe by construction);
    ../../external-insights/01-process-and-quality-doctrine.md §5 (the ratchet — an uncommitted gate is no gate),
    §1 (name-your-floors, code-wins-over-docs).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md (the
    complete git.* taxonomy + the Git ReBAC fragment + the holder tags); 00-overview.md §1.2 (owns-vs-delegates,
    the Git ReBAC fragment row) + §4 (inherited non-negotiables 1,2,4).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §1 (the frozen
    ReBAC fragments — Git: ref-glob + CODEOWNERS-as-relations + approve_untrusted_ci + watcher).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 4.9 (per-subsystem ReBAC
    namespace fragment; the Git fragment frozen), 2.9 (event taxonomy + token table grammar
    <subsystem>.<artifact_type>.<event_name>; git.* tokens), 10.2 (the #[personal_data] classify-derive + the
    no-untagged-personal-data lint), 1.6 (the tenant-predicate + no-untagged-personal-data lints git compiles
    against).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3.0 (the M1 bullet) + §2 (upstream deps table rows
    4.9, 2.9, 10.1/10.2).
- **DELIVERABLE (what to build + exactly where in the repo).** In the git service implementation crate
  (myelin-git, the new subsystem crate under the workspace) plus its contributions into the shared cell schema:
  - The Git ReBAC namespace fragment submitted into the one cell schema Identity compiles (contract 4.9):
    ref-glob-scoped relations (protected_push over a ref-glob), CODEOWNERS-as-relations (a CODEOWNERS path
    pattern compiles to a reviewer relation), approve_untrusted_ci (the maintainer endorsement relation the X-1
    fork gate rides), and the watcher relation per watchable type (repo/pr). The fragment must COMPILE in the
    cell schema — that is the gate of this prompt, not a runtime property.
  - Register the git.* event tokens in the Bus taxonomy seed (2.9): git.ref.updated, git.pr.opened/updated/
    merged/closed, git.review.requested/submitted, git.comment.created (the complete v1 list named in arch 03).
    Validate against the Bus §6.2 singular token table (git is the canonical subsystem token) — git registers,
    it does not author the grammar.
  - Declare the git PersonalDataHolder H1 INTENT (the holder will be auto-registered by serve when the store
    opens in GIT-P3) and apply the #[personal_data(category, role, basis, retention, erasure, subject_locator)]
    tags on the (still-skeletal) git schema types — author_pseudonym / reviewer_pseudonym / pusher_pseudonym and
    the free-text body fields — so the no-untagged-personal-data lint is green from the first migration (GIT-P3).
  - FLOOR named: none. This is a contract-fragment freeze, not a feature. State in the crate doc that no git
    feature ships here — only the shapes other systems compile against — and name GIT-P3 as the milestone where
    the holder is actually opened and the tokens actually emitted.
- **CONTRACTS TO IMPLEMENT.** 4.9 the Git ReBAC fragment (owned — the fragment definition, compiled by Identity).
  2.9 the git.* event tokens (owned — registered into the Bus seed). 10.2 the #[personal_data] tags (consumed —
  applied to git types so the lint is green). Implement to the frozen shapes; a needed change is a
  whole-workspace contract PR, escalated and written down, not a local divergence (code-wins-over-docs).
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The Git ReBAC fragment COMPILES in the shared cell schema Identity builds (a build-time gate, not a runtime
    drill) — CI, the compile is the green artifact.
  - The no-untagged-personal-data lint is GREEN on the git skeleton schema (0 untagged PII fields; the lint red
    on a deliberately-untagged fixture field, green on the tagged set) — CI, lint signal = 0 untagged fields.
  - The git.* tokens are present in the Bus taxonomy and parse under the §6.2 grammar (0 ungrammatical tokens) —
    CI. (No git-specific runtime drill here — §3.0 exit gate is explicitly compile-time + sign-off, not a
    runtime property.)
- **TESTS (required).** Unit tests that the fragment compiles and that each git.* token round-trips the §6.2
  grammar. The red+green fixture pair for the no-untagged-personal-data lint applied to a git type. The
  provider/consumer CDC stub for contract-index row 4.9 (the Git fragment) and 2.9 (the git tokens). State the
  cargo-mutants mutation-score floor for the fragment-compile module if it is mandatory-core; if not, say so.
- **DEFINITION OF DONE.** The fragment compiles in the cell schema; the git.* tokens are registered and
  grammatical; the no-untagged-personal-data lint is green with both fixtures; the CDC stubs and unit tests
  pass; the contract-coverage scanner is green on the touched rows; the no-feature floor note is written; the
  work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M1: Git ReBAC fragment + git.* event tokens + holder tags. Body lists: contract
  4.9 (Git fragment) compiled, 2.9 (git tokens) registered, 10.2 tags applied; the no-untagged-personal-data
  lint greened with red+green fixtures; the no-feature floor named (GIT-P3 opens the holder). Branch first if on
  default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P2 — Register the git #sub mints + the code-search projection spec + declare the X-1 CheckStatus consumer contract + the design-system pass

- **BAND.** M2.
- **ROADMAP MILESTONE.** Pre-work M2 (planning/06-roadmaps/subsystems/git-hosting.md §3.0, the M2 + M2-design
  bullets).
- **DEPENDS-ON.** GIT-P1 (the git crate + tokens exist). The M2 Refs prompts that freeze the #sub grammar +
  the 4-step tombstone ladder (contract 5.7) and the project() requirement (5.6). The M2 Search prompt that
  ships declare_indexable (6.3). The M2 reconciliation that froze the 5.9 CheckStatus shape. The index places
  this in M2 alongside the reactive-layer freeze.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (design comes before implementation for anything with a frontend — no frontend code
    without a reviewed design sketch); ../../external-insights/05-ux-and-design.md (the design-language bar);
    ../../external-insights/01-process-and-quality-doctrine.md §8 (the human sign-off is the bottleneck —
    fork-trust UX is decision-shaped: sketch + sign-off, do not build autonomously).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md (the
    #sub mints git owns; the declare_indexable projection spec; the CheckStatus consumer contract);
    04-views-cli-and-api.md §2.2 (the X-1 affordances — fork-trust badge, checks panel, merge-queue affordances);
    the design/ folder (the present IA/flows/wireframes the pass refines).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-1 (the
    CheckStatus seam declared M2, consumer half Git M3), X-4 (the #sub grammar frozen), OQ-D (the
    content-anchored line range), OQ-12 (the design pass).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 5.7 (the unified #sub grammar —
    git owns comment-/thread-/L<a>-L<b> mints), 5.9 (the Git↔CI CheckStatus seam — the projection-table schema
    + the run_attempt supersession rule, written against the M2-frozen shape, ready to build in M3), 6.3 (
    declare_indexable — the git.* code projection spec).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3.0 (the M2 + M2-design bullets) + §2 (upstream
    deps rows 5.7, 6.3, 5.9).
- **DELIVERABLE (what to build + exactly where in the repo).** In the git crate + the design record:
  - Register with Refs the #sub kinds git owns (5.7): comment-<id>, thread-<id>, L<a>-L<b> (and the commit/pr/
    review canonical roots). git mints stable opaque ids; Refs stores the full sub-URN + the stripped root. The
    mint functions are stubbed-but-typed here (the resolver lands in GIT-P6/GIT-P8); the REGISTRATION (kinds
    declared to Refs) is the deliverable.
  - Register git's declare_indexable projection spec with Search (6.3): the git.* code projection shape
    {path, language, symbols (camel/snake split), literals, commit message, text}, ft_fields, struct_fields,
    acl_object_type=repo. The emitter lands in GIT-P9; the SPEC registration is the deliverable here.
  - Declare the X-1 CheckStatus consumer contract (5.9): write the check_status projection-table schema keyed
    (commit_oid, context) + the run_attempt monotonic supersession rule + the required-set policy shape, against
    the M2-frozen CheckStatus fact — as a written, compiling contract module (no live consumer yet; the consumer
    + gate land in GIT-P7). This is the seam-floor named in §5 of the roadmap: built in M3 against a synthetic
    emitter, live end-to-end at M4.
  - The design-system pass (pre-frontend, OQ-12): a visual/token-level pass over the present IA/flows/wireframes
    in design/, INCLUDING the new X-1 affordances (the fork-trust badge, the checks panel, the merge-queue
    affordances). The fork-trust UX is decision-shaped (EI-01 §8): produce the sketch and PAUSE for human
    sign-off; do not build the UI here (the UI lands in GIT-P12). Record the sign-off in design/.
  - FLOOR named: the X-1 seam-floor — the CheckStatus consumer is declared here and built against a synthetic
    ci.check.updated emitter in GIT-P7; the real CI producer wiring is the M4 co-gate (GIT-D10/CI-D8
    end-to-end). Name it in the contract module doc.
- **CONTRACTS TO IMPLEMENT.** 5.7 the git #sub mints (owned — registered with Refs). 6.3 declare_indexable
  (owned — the git projection spec registered with Search). 5.9 the CheckStatus consumer contract (owned —
  declared/compiling, not yet live). Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The #sub kinds register with Refs and the declare_indexable spec registers with Search; both compile and the
    registrations are accepted (build-time gate) — CI.
  - The check_status projection-table schema + the run_attempt supersession rule compile against the M2-frozen
    5.9 shape (build-time; the consumer goes live in GIT-P7) — CI.
  - The design pass is REVIEWED-AND-SIGNED-OFF in design/, with the fork-trust UX explicitly approved (the
    sign-off is the green artifact; EI-01 §8 — no frontend code without it) — sign-off recorded, dated.
- **TESTS (required).** Unit tests that the #sub mints produce grammatical sub-URNs and that the declare_indexable
  spec serializes to the 6.3 shape. A compile test for the check_status schema module. The CDC stubs for rows
  5.7, 6.3, 5.9 (git's owned half). No mutation floor (registration + schema declaration, not core logic) —
  state that.
- **DEFINITION OF DONE.** The #sub + declare_indexable registrations compile and are accepted; the check_status
  contract module compiles against the frozen 5.9 shape; the design pass is signed off (dated) with the
  decision-shaped fork-trust UX approved; the CDC + unit tests pass; the seam-floor is named in the doc; the
  work is committed. No gate is weakened to pass; the sign-off is real, not assumed.
- **COMMIT.** Header: P-<NNN> M2: git #sub mints + code-projection spec + X-1 consumer contract + design pass.
  Body lists: 5.7/6.3 registered, 5.9 consumer contract declared (compiling); the design pass signed off
  (fork-trust UX approved); the X-1 seam-floor named (GIT-P7 builds against synthetic, M4 goes live). Branch
  first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P3 — The git object store + receive-pack + the silent-data-loss floor (the one-tx ref-CAS + outbox)

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G1 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G1", the receive-pack +
  data-loss-floor half; the pseudonymous-commit data-model gate is GIT-P4, committed in the same band before any
  feature reads the stream).
- **DEPENDS-ON.** GIT-P1 (the holder intent + tokens). The M0 outbox prompts (contracts 2.2-2.5) + the
  EventEnvelope freeze (2.1). The M1 Storage prompts that ship the OLTP tier + RLS + encrypted columns + the
  outbox (11.1), the content-addressed BlobStore fs-floor (11.2), the KMS hierarchy + per-subject DEK (11.3/
  11.4), and — the hard gate — backup/restore + restore-verify STOR-D1 (11.5). The index places this after
  STOR-D1 is GREEN: git does not write real data over a red restore-verify (master M1→M2 gate, the
  silent-data-loss floor).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale; name-your-floors); ../../external-insights/01-process-and-quality-doctrine.md
    §2 (order-by-non-negotiability — silent data loss outranks every feature), §3 (prove-it: a property is not
    real until a drill forces the failure and observability watches it survive); ../../external-insights/04-hard-problems.md
    §3 (world-scale git — authoritative bytes on local disk first, object-backed is the named follow-on).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/02-internals-and-algorithms.md §2 (the
    sandboxed receive-pack → quarantine → in-process Rust policy → ref-CAS + outbox in ONE tx), §3 (the
    reftable-on-OLTP ref store, the ref as the aggregate); 01-tech-and-data-model.md §1-§2 (the GitCore layered
    seam — canonical git for the wire, gix in-process), §4 (the data model — git object tier, reftable-on-OLTP);
    00-overview.md §2 (B) (the serving tier), §4 (inherited non-negotiables 1,2,4,8).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §8 (the BlobStore
    fs↔object one-line swap, the local-NVMe floor).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 2.2/2.3 (OutboxTx::emit + the
    outbox table, per-ref aggregate ordering at push QPS), 2.1 (EventEnvelope), 11.1 (OLTP tier + RLS +
    encrypted columns + the outbox), 11.2 (BlobStore content-addressed, fs-backed floor), 11.3/11.4 (KMS +
    per-subject DEK crypto-shred), 11.5 (backup/restore + restore-verify, the cross-seam cursor).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G1 + §1 (non-negotiability item 1) + §2
    (★ STOR-D1 must be green) + §4 (the contracts-by-milestone rows 2.2/2.3, 11.2).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md rows GIT-D9 (crash mid-push →
    emit-iff-committed) + GIT-D1 (burst force-push per-ref order).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - The GitCore layered seam (arch 01 §2, 02 §2): the sandboxed canonical git path for wire serving
    (upload-pack/receive-pack/ls-refs) + maintenance, and the gix (libgit2 fallback) in-process path for
    read/diff/blame. The TE-8 Stage-1 position — do not attempt a pure-gix server (gix has no server-side
    receive-pack).
  - The receive-pack path (arch 02 §2): sandboxed git receive-pack ingests the pack into a QUARANTINE;
    in-process Rust evaluates branch-protection / secret-scan / size rules — REJECT BEFORE THE REF MOVES; then
    OUR code does the ref CAS + the outbox insert in ONE DB TRANSACTION (BUS-2, emit-iff-committed). On abort,
    quarantine objects are discarded (never promoted).
  - The reftable-on-OLTP ref store (arch 02 §3): the ref-update transaction is the linearisation point; the
    aggregate for git.ref.updated is the REF (per-ref ordering at push QPS via the outbox UNIQUE(aggregate,seq)).
  - Pack/delta storage on the local-NVMe floor behind the BlobStore trait (GF-1): repos RELOCATABLE, never
    node-pinned (STOR-5). Commit-graph + reachability bitmaps + MIDX maintenance.
  - The repo / fork-network / quarantine schema + the control-plane DB (one DB per service, RLS, per-tenant
    envelope-encrypted, per-subject DEK for free-text bodies); the store auto-registers as PersonalDataHolder H1
    (via serve, contract 1.4). Emit the git.ref.updated / git.* taxonomy via the outbox ONLY (no-raw-publish).
  - FLOOR named: the local-disk pack floor (GF-1 — object-backed packs follow in GIT-P13) + the single-cell
    primary+quorum replication floor (GF-2 — cross-cell follows in GIT-P13) + SHA-1+sha1dc default, hash-agnostic
    model (GF-2b — SHA-256 flip follows in GIT-P13). Name each in the crate doc with its follow-on prompt.
- **CONTRACTS TO IMPLEMENT.** 2.2/2.3 OutboxTx::emit + the per-ref aggregate (owned — the receive-pack →
  ref-CAS → outbox emit in one tx). 2.9 git.ref.updated / git.* emission (owned). 11.2 BlobStore pack tier
  (consumed — local-NVMe floor). 10.1 PersonalDataHolder H1 registration (consumed — the store auto-registers;
  locate/export/erase land fully in GIT-P11). Implement to the frozen shapes; escalate any needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - GIT-D9 (CI): crash the serving tier mid-push (after policy, before AND after commit) → git.ref.updated is
    emitted IFF the ref move committed; 0 ghost, 0 lost; quarantine objects discarded on abort. Green artifact:
    the outbox emit-iff-committed signal (contract 1.8) shows 0 ghost / 0 lost across the kill. PERMANENT-gate
    family (STOR-D1/D2 re-run on every store-touching change — say so).
  - GIT-D1 (SCHED): burst force-pushes + rapid pushes to one hot ref at 1×/10×/30× → git.ref.updated in PUSH
    ORDER PER REF; refs fan out parallel; 0 lost/ghost; outbox order == ref-update order. Green artifact: the
    per-aggregate-order + outbox-depth signal.
  - The no-raw-publish + tenant-predicate + residency-pin lints green on the git schema — CI.
- **TESTS (required).** Unit tests for the quarantine→policy→ref-CAS→outbox state machine (reject-before-ref-move;
  abort-discards-quarantine). An END-TO-END chained test (EI-01 §4 — chain mutations, not single handlers): push
  → policy reject path; push → commit → kill before publish → recover → assert emit-iff-committed. The
  provider/consumer CDC pair for rows 2.2/2.3. The GIT-D9 + GIT-D1 drill scenarios on the failure-injection
  harness. myelin-git's ref-store + receive-pack path is mandatory-core: state the cargo-mutants mutation-score
  floor and meet it.
- **DEFINITION OF DONE.** The receive-pack → one-tx ref-CAS+outbox path exists and compiles; GIT-D9 emits its
  dated green artifact (0 ghost / 0 lost) and GIT-D1 its per-ref-order artifact; the lints are green; the
  unit + chained-e2e + CDC + drill tests pass; the three floors (GF-1/GF-2/GF-2b) are named with their follow-on
  (GIT-P13); the store is registered as holder H1; the work is committed. A red GIT-D9 does NOT become green by
  weakening the assertion — it becomes a dated claimed-not-proven scorecard row and blocks M4.
- **COMMIT.** Header: P-<NNN> M3: git object store + receive-pack + the silent-data-loss floor. Body lists:
  contracts 2.2/2.3/2.9/11.2/10.1 implemented; GIT-D9 greened (0 ghost / 0 lost, measured) + GIT-D1 (per-ref
  order, measured); the ref-store mutation score measured; floors GF-1/GF-2/GF-2b named with follow-on GIT-P13.
  Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P4 — Pseudonymous-by-default commit identities (the erasure-vs-immutability data-model gate)

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G1 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G1", the
  pseudonymous-by-default half — GIT-1, the data-model gate that MUST be decided and enforced before the git
  data model is fixed, EI-04 §1).
- **DEPENDS-ON.** GIT-P3 (the git object store + the schema this prompt pins the pseudonym columns into; this
  prompt is sequenced in the SAME band immediately after GIT-P3 because pseudonymity gates the data model — it
  cannot be bolted on). The M1 Identity prompt that ships resolve_pseudonym/erase + the pseudonym grammar
  <pseudonym>@<tenant>.noreply (4.8). The index places this directly after GIT-P3 and before any feature prompt.
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
    the frozen <pseudonym>@<tenant>.noreply grammar; Git commits pseudonymous-by-default), 10.9 (the ONE
    erasure posture — instantiate by reference, never restate).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G1 (the pseudonymous-by-default bullet) + §1
    (non-negotiability item 2) + §5 (GF-7 floor) + the OQ-10/R-8 spike note (enforcement mode: client-cooperative
    sha-stable vs server-side rewrite-at-push — the PROPERTY is decided, the default is the call).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row GIT-D2 (the GIT-1 half asserted here;
    completed at GIT-P11).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - The schema stores author_pseudonym / reviewer_pseudonym / pusher_pseudonym (NEVER name/email); the
    person↔pseudonym map is Identity's erasable record (4.8 — git holds only the opaque pseudonym +
    <pseudonym>@<tenant>.noreply in the immutable bytes).
  - Push policy enforces the pseudonym at receive-pack (the GIT-P3 in-process policy engine gains the
    pseudonymity rule): a commit whose author/committer identity is not the principal's tenant pseudonym is
    REJECTED before the ref moves (or rewritten at push — pin the enforcement default per the OQ-10/R-8 decision:
    the PROPERTY "immutable bytes carry only the opaque pseudonym" is decided here; record the chosen default and
    its rationale in the crate doc).
  - The residual lawful-basis posture is instantiated BY REFERENCE to the ONE platform posture (10.9 / recon
    §X-7) — NOT restated as a git-local statement (arch 00 §0.1 Δ6). The [OPEN — LEGAL] Art. 17 ratification is
    R-7 (Legal/DPO, parallel — not a code gate).
  - FLOOR named: GF-7 — the structural mechanism (pseudonymous-by-default + per-subject DEK shred +
    history-rewrite) ships across GIT-P3/GIT-P4/GIT-P11; the lawful-basis residual is the ONE posture's
    [OPEN — LEGAL] statement (R-7, parallel-legal, not a code gate). Name it.
- **CONTRACTS TO IMPLEMENT.** 4.8 the pseudonym consumer (owned — git enforces pseudonymous-by-default and
  stores only the opaque pseudonym; the map + erase are Identity's). 10.9 the ONE posture (consumed by
  reference — instantiated, never restated). Implement to the frozen grammar; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - GIT-D2 (SCHED, the GIT-1 half here): a stored commit carries author_pseudonym in the
    <pseudonym>@<tenant>.noreply form, never a name/email (0 name/email bytes in newly-stored commit identity
    fields). Green artifact: a scan of newly-stored commit identities shows 0 cleartext PII. (The full
    erase-reaches-every-holder GIT-D2 completes at GIT-P11.)
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
  lint is green; the unit + CDC + drill tests pass; GF-7 is named with its follow-on; the work is committed.
- **COMMIT.** Header: P-<NNN> M3: pseudonymous-by-default commit identities (the data-model gate). Body lists:
  contract 4.8 enforced (pseudonymous-by-default), 10.9 instantiated by reference; GIT-D2 GIT-1 half greened
  (0 cleartext PII, measured); the enforcement default recorded; GF-7 floor named with follow-on GIT-P11 +
  R-7 (Legal). Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By
  trailer.

---

### GIT-P5 — The front door (SSH + smart-HTTP v2), authenticate/check, residency reject, ReBAC live, shed order

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G2 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G2").
- **DEPENDS-ON.** GIT-P3 (the serving tier + repo placement). GIT-P1 (the Git ReBAC fragment, now wired live).
  The M1 Identity prompts that ship authenticate (machine-identity SSH/deploy-key/PAT/per-job, 4.1), check +
  CaveatContext (4.2), write_tuples/zookie (4.6/4.10), the ReBAC engine (4.9), and fail-static (4.11). The M1
  Tenancy prompts that ship the (tenant,region) partition (12.1), discover/placement_of repo-granular
  relocatable (12.2), residency_verify (12.4). The M0 ResilientClient + FailStatic (1.9/1.10). The index places
  this after GIT-P3 (the backend it routes to) and the M1 Identity/Tenancy work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (EU-sovereign, residency by construction);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — cross-tenant 0 is a quantified
    gate), §5 (the lints as committed ratchet).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/02-internals-and-algorithms.md §1 (the
    front door / router); 00-overview.md §2 (A) (the stateless front door — authenticate → check → placement_of
    → residency reject → stream → shed order; liveness≠readiness), §1.2 (the Git ReBAC fragment + the SSH/HTTPS
    front door); 01-tech-and-data-model.md §1 (russh + axum/hyper).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §1 (machine-identity
    resolution), §10 (repo-granular placement, residency), OQ-K (per-surface shed budgets).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 4.1 (authenticate —
    machine-identity SSH/deploy-key/PAT/per-job → Principal), 4.2 (check + CaveatContext), 4.9 (the Git ReBAC
    fragment live), 4.11 (FailStatic bound on the Id dependency), 12.1/12.2/12.4 (partition + placement_of +
    residency_verify), 1.9/1.10/1.11 (ResilientClient/FailStatic/shed order, the protected-human-lane ADR-16).
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
  - The Git ReBAC fragment wired LIVE (4.9): ref-glob relations, CODEOWNERS-as-relations, protected_push,
    approve_untrusted_ci. The FailStatic bound on the Id dependency (4.11) so an Id hiccup DEGRADES, not
    cascades (just-revoked still denied; static_max ≤ revocation SLA).
  - The protected-human-lane shed order (ADR-16, the OQ-K per-surface budget floor: speculative → batch/CI →
    agent → human-last) at the front door, with 429 + Retry-After.
  - FLOOR named: the per-surface shed budget floor (OQ-K) is tuned by GIT-D6 (the clone-storm drill lands in
    GIT-P14/M5); the CDN clone/bundle accelerated-clone path (11.2 C3) ships its bundle-URI floor here, the full
    within-EU CDN class hardens in GIT-P13. Name both.
- **CONTRACTS TO IMPLEMENT.** 4.1 authenticate (consumed — every entrypoint resolves a Principal), 4.2 check +
  CaveatContext (consumed — per-action gate), 4.9 the Git fragment (owned, now live), 4.11 FailStatic (consumed),
  12.2/12.4 placement_of + residency_verify (consumed — region-pinned placement, reject-if-leaving-region),
  1.11 the shed order (consumed). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - GIT-D8 (CI): cross-tenant repo access via a token whose tenant ≠ the URL-path tenant → TENANT FROM THE
    TOKEN; 0 cross-tenant read; rejected at the front door. Green artifact: the authz-deny signal + the
    tenant-predicate lint green (cross-tenant-read-count = 0).
  - A route that would leave the region is REJECTED at the front door (residency-pin lint green; 0
    out-of-region routes admitted) — CI.
  - The shed order holds under a synthetic mixed-principal storm: the human lane is served while the agent/CI
    lane sheds (429 + Retry-After) — CI (the full 30× surge is GIT-D6 in GIT-P14; here the order is asserted at
    1×).
- **TESTS (required).** Unit tests for the authenticate → check → placement_of → residency-reject → shed
  pipeline (each machine-identity kind resolves; a wrong-tenant token denies; an out-of-region route rejects).
  A chained e2e test: SSH clone → push → check gate → residency reject path. The CDC pairs for the consumed
  rows 4.1/4.2/12.2. The GIT-D8 drill scenario. The router/authz path is mandatory-core: state the
  cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** The front door authenticates every machine-identity, checks every action, rejects
  out-of-region routes, and sheds in the protected-human-lane order; the Git ReBAC fragment is live; GIT-D8
  emits its dated green artifact (0 cross-tenant read); the residency-pin + tenant-predicate lints are green;
  the unit + chained-e2e + CDC + drill tests pass; the shed-budget + CDN floors are named with their follow-ons;
  the work is committed. This is FIRST RUNNABLE (roadmap §6): clone/push works, authenticated, tenant-isolated,
  region-pinned, never loses an event.
- **COMMIT.** Header: P-<NNN> M3: git front door — SSH + smart-HTTP v2, authz, residency, shed order. Body
  lists: contracts 4.1/4.2/4.9/4.11/12.2/12.4/1.11 implemented; GIT-D8 greened (0 cross-tenant read, measured);
  the router mutation score measured; the shed-budget + CDN floors named with follow-ons (GIT-P14/GIT-P13).
  Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P6 — Pull/merge requests, reviews, inline threads, and the reference-graph edges

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G3 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G3").
- **DEPENDS-ON.** GIT-P3 (the control-plane OLTP + the object store), GIT-P5 (the front door + check gate). The
  M2 Refs prompts that ship ArtifactRef parse/format (5.1), resolve/project (5.2/5.6), refs.edge.created from
  content nodes (5.4), the typed-edge mirror (5.5). The M2 myelin-content freeze (13.1 — the markdown-subset +
  the three structured inline nodes). The index places this after GIT-P5 and the M2 Refs/content work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (the cross-artifact reference graph); §3 (top-of-the-line UX — content round-trips);
    ../../external-insights/01-process-and-quality-doctrine.md §4 (actually try it — chain mutations end-to-end).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md (the
    PR/review/comment entities; the inline nodes → refs.edge.created; project(); the typed-edge mirror);
    00-overview.md §1.1 (the PR lifecycle, reviews, inline comment threads, CODEOWNERS) + §0.1 Δ7 (the
    ArtifactRef id grammar — pr/<repo>:<n>, commit/<repo>:<sha> are the stored canonical keys, #1421 is
    render-time).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-2 (the
    myelin-content taxonomy + the three content nodes byte-identical), REF-3 (display keys render-time only).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 5.1 (ArtifactRef id grammar —
    git's stable canonical keys), 5.2/5.6 (resolve / project(ref,viewer) — the only way Refs/Search/Notif read
    git artifacts, per-viewer permission-checked), 5.4 (refs.edge.created from the mention/artifact_ref/embed
    content nodes — no standalone edge-write API), 5.5 (the typed-edge mirror — PR-link / commit-trailer
    lifecycle edges), 13.1 (the myelin-content markdown-subset + render(parse(md)) === md).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G3 + §4 (rows 5.1/5.2/5.6/5.4/5.5).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate (control-plane OLTP):
  - The Pull Request lifecycle, Reviews, inline comment THREADS, branch-protection rulesets, the CODEOWNERS
    resolver — all on the control-plane OLTP (one DB, RLS, per-subject DEK for free-text bodies).
  - PR/review/comment bodies use the FROZEN myelin-content markdown-subset + the three structured inline nodes
    (mention/artifact_ref/embed, 13.1) which produce refs.edge.created UNIFORMLY (Closes <ISSUEKEY> / @alice /
    embeds → edges, 5.4). Single-author CAS over the content subset; render(parse(md)) === md.
  - project(ref, viewer) (5.6) for git artifacts (PR/commit/review) — the ONLY way Refs/Search/Notif read git's
    artifacts, per-viewer permission-checked.
  - The typed-edge mirror (5.5): PR-link / commit-trailer lifecycle edges (closes/relates) into the Refs
    projection.
  - ArtifactRef id grammar (5.1, REF-3): git's stored canonical key is the sha / PR-number (already stable); the
    #1421-style display is render-time only.
  - FLOOR named: none new (the diff-anchor line-range resolver is GIT-P8; the merge gate is GIT-P7) — but state
    that PR/review bodies are single-author CAS here, with the multi-author collab story owned by Knowledge, not
    git.
- **CONTRACTS TO IMPLEMENT.** 5.1 ArtifactRef id grammar (owned — git's stable keys), 5.2/5.6 resolve/project
  (owned — project(ref,viewer) for git artifacts), 5.4 refs.edge.created (owned — emitted from the content
  nodes via outbox), 5.5 the typed-edge mirror (owned — PR/trailer lifecycle edges), 13.1 myelin-content
  (consumed — the markdown-subset for bodies). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The three inline ref nodes each emit EXACTLY ONE refs.edge.created (mention/artifact_ref/embed →
    1 edge each; 0 duplicate, 0 missed) — CI.
  - render(parse(md)) === md is 100% on PR/comment bodies (the KN-D2-class round-trip applied to git content;
    a corpus of git bodies round-trips byte-identical) — CI, round-trip-parity = 100%.
  - project(ref, viewer) returns a per-viewer permission-checked projection; a viewer without access gets a
    tombstone, never the title (feeds the M3-G5/M5 leak drills) — CI.
- **TESTS (required).** Unit tests for the PR/review/thread lifecycle, the CODEOWNERS resolver, and the content
  node → edge emission. A chained e2e test (EI-01 §4): open PR → add inline comment with a mention + a Closes
  trailer → assert exactly the right edges + the round-trip parity. The CDC pairs for rows 5.1/5.2/5.6/5.4/5.5.
  The content-node → edge path is mandatory-core: state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** The PR/review/thread entities + the CODEOWNERS resolver exist; the three content nodes
  each emit exactly one edge; render(parse(md)) === md is 100% on git bodies; project(ref,viewer) is
  per-viewer permission-checked; the typed-edge mirror is wired; the unit + chained-e2e + CDC tests pass; the
  single-author-CAS note is written; the work is committed.
- **COMMIT.** Header: P-<NNN> M3: PRs, reviews, inline threads + the reference-graph edges. Body lists:
  contracts 5.1/5.2/5.6/5.4/5.5/13.1 implemented; edges (1-per-node, measured) + render==md (100%, measured)
  greened; the content-node mutation score measured. Branch first if on default; do not push unless asked. End
  with the workspace Co-Authored-By trailer.

---

### GIT-P7 — The merge gate, the CheckStatus projection, fork-endorsement, and the merge queue (the X-1 consumer half)

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G4 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G4", the X-1 consumer +
  merge-gate + merge-queue half; the content-anchored line-range resolver / GIT-D7 is GIT-P8).
- **DEPENDS-ON.** GIT-P6 (the PR entities the gate guards), GIT-P2 (the declared CheckStatus consumer contract +
  the approve_untrusted_ci relation registered). The M2 Workflow prompts that ship DurableExecutor +
  WfCtx + SCHEDULE_AND_RUN_JOB + the durable signal (9.1/9.2/9.4) + the timer wheel (9.3). The M2 freeze of the
  5.9 CheckStatus shape. The M1 Storage trust-scoped cache (11.2 C4). The CI producer side (5.9, M4) is the
  co-dependency — built here against a SYNTHETIC ci.check.updated emitter, proven end-to-end at the M4 exit.
  The index places this after GIT-P6 and the M2 Workflow work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (the merge gate); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it
    — 0 double-merge is a quantified gate), §7 (keep contracts coherent — git reads its own projection, never
    calls CI synchronously, the no-cross-sync-cycle lint).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/02-internals-and-algorithms.md §6 (the
    merge gate + merge queue implementing the X-1 CheckStatus consumer + run_attempt supersession +
    fork-endorsement + the ci.result durable-signal wait); 00-overview.md §0.1 Δ1/Δ2/Δ3 (the frozen CheckStatus
    fact, the rollup ci.result wait, untrusted-fork neutral-until-endorsed); §1.1 (Git owns what is allowed to
    land; reads trust_tier off the fact, never recomputes).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-1 (the seam),
    OQ-F (per-effect idem_key + SCHEDULE_AND_RUN_JOB).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 5.9 (the Git↔CI CheckStatus
    seam — Git the consumer + gate: the check_status projection keyed (commit_oid,context), run_attempt
    last-writer-wins supersession, the required-set policy, fork-endorsement, the merge queue waking on the
    ci.result rollup), 9.1/9.2/9.4 (DurableExecutor + SCHEDULE_AND_RUN_JOB + the durable ci.result signal),
    11.2 C4 (the fork:<pr_id> trust-scoped cache), 4.9 (the approve_untrusted_ci relation).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G4 + §0 (the X-1 split) + §2 (the seam
    frozen-but-not-live note) + §4 (row 5.9) + §5 (the seam-floor + GF-8 single-lane queue).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row GIT-D10 (the X-1 check seam) +
    CI-D8 (the CI side, proven together at M4).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate (control-plane OLTP +
  the merge-queue workflow):
  - The check_status PROJECTION TABLE keyed (commit_oid, context) — the X-1 consumer (5.9): consumes
    ci.check.updated (from a SYNTHETIC emitter here), applies MONOTONIC run_attempt supersession (>= supersedes,
    < dropped as stale re-delivery — the bus is at-least-once so the drop is mandatory), idempotent on event_id,
    holds EXACTLY ONE current row per key.
  - The MERGE GATE: Git owns the required-set policy (ruleset.required_contexts — CI reports facts, Git decides
    which contexts gate this base_ref). Git READS trust_tier OFF THE FACT, never recomputes it.
  - The fork / trust-tier gate (the poisoned-pipeline defence): an untrusted_fork success is NEUTRAL FOR GATING
    until a maintainer endorses via check(subject, approve_untrusted_ci, repo) OR the context is re-run trusted.
    Fork-PR cache writes confined to the fork:<pr_id> scope (11.2 C4) — a fork cannot reach the trusted cache or
    the trusted gate.
  - The MERGE QUEUE as a DURABLE WORKFLOW per target ref (9.1/9.2/9.4): parks on the rollup ci.result signal via
    SCHEDULE_AND_RUN_JOB (holds no runtime while CI runs for hours); idempotent on the merge_attempt_id
    idem_token. Single-lane serialised (GF-8 floor).
  - Git NEVER synchronously calls CI (no-cross-sync-cycle lint) — it reads its own projection.
  - FLOOR named: single-lane merge queue (GF-8 — speculative/parallel batching is GIT-P13/M5); the seam-floor —
    built here against a synthetic ci.check.updated emitter, the real CI producer goes live at M4 (GIT-D10/CI-D8
    end-to-end). Name both.
- **CONTRACTS TO IMPLEMENT.** 5.9 the CheckStatus consumer + merge gate (owned — the projection, supersession,
  required-set policy, fork-endorsement, the merge-queue ci.result wait), 9.1/9.2/9.4 the merge-queue durable
  workflow (consumed), 11.2 C4 the trust-scoped cache (consumed), 4.9 approve_untrusted_ci (owned — the
  endorsement relation). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - GIT-D10 (CI, against the synthetic producer here; RE-CONFIRMED end-to-end with the real CI producer at the
    M4 exit): (a) out-of-order/dup ci.check.updated → run_attempt-monotonic supersession holds the correct
    current row, drops stale lower attempts (exactly 1 current row per key); (b) a fork PR self-greens → NEUTRAL
    FOR GATING (merge blocked); (c) a maintainer endorses via approve_untrusted_ci → gate flips green; (d) a
    doubly-delivered ci.result → the merge workflow wakes EXACTLY ONCE; 0 double-merge (merge-count == 1). Green
    artifact: the 1-current-row-per-key + merge-count==1 signals.
  - The no-cross-sync-cycle lint green (git makes 0 synchronous calls to CI) — CI.
- **TESTS (required).** Unit tests for the supersession rule (>= supersedes, < dropped; idempotent on event_id;
  exactly one current row), the required-set policy, the fork-neutral-until-endorsed flow, and the merge-queue
  idempotency. A chained e2e test (EI-01 §4): synthetic ci.check.updated (out-of-order + dup) → projection holds
  → fork self-green neutral → endorse → ci.result doubly-delivered → merge wakes once. The provider/consumer
  CDC pair for row 5.9 (git's consumer half). The GIT-D10 drill scenario against the synthetic producer. The
  supersession + merge-queue path is mandatory-core: state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** The check_status projection + supersession + required-set policy + fork-endorsement +
  the ci.result-waiting merge queue exist; GIT-D10 emits its dated green artifact against the synthetic producer
  (1 current row/key, fork-neutral, endorse-flips, merge-count==1); the no-cross-sync-cycle lint is green; the
  unit + chained-e2e + CDC + drill tests pass; the single-lane (GF-8) + seam (synthetic-producer) floors are
  named with their follow-ons (GIT-P13 / M4 co-gate); the work is committed.
- **COMMIT.** Header: P-<NNN> M3: merge gate + CheckStatus projection + fork-endorsement + merge queue (X-1
  consumer). Body lists: contract 5.9 (consumer) + 9.1/9.2/9.4 + 11.2 C4 implemented; GIT-D10 greened against
  the synthetic producer (1 row/key, merge-count==1, measured); the supersession mutation score measured; GF-8
  + the seam-floor named (GIT-P13 / M4 co-gate). Branch first if on default; do not push unless asked. End with
  the workspace Co-Authored-By trailer.

---

### GIT-P8 — Content-anchored inline-thread line ranges (the #sub 4-state resolver)

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G4 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G4", the
  content-anchored line-range resolver half — GIT-D7).
- **DEPENDS-ON.** GIT-P6 (the inline threads this anchors), GIT-P2 (the L<a>-L<b> #sub kind registered with
  Refs). The M2 Refs prompts that ship the unified #sub grammar + the 4-step tombstone ladder (5.7). The index
  places this after GIT-P6 (a sibling of GIT-P7 in M3-G4; split out because the diff-anchor resolver has its
  own green gate, GIT-D7).
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
    mint-time blob oid (the L<a>-L<b> #sub kind registered in GIT-P2).
  - Resolve through the unified 4-state ladder (5.7): exact (LIVE), rebased (MOVED — the lines moved but match),
    partial (OUTDATED — context drifted), tombstone (GONE — content_gone). Git is the owner's sub-anchor
    resolver the Refs ladder calls (Refs handles permission → root → sub-resolve → erased; git answers the
    sub-resolve step for L-ranges).
  - The "view in original context" render path for MOVED/OUTDATED/GONE (never silently wrong — always show the
    resolution state).
  - FLOOR named: GF-5 — per-pair fingerprint diff-anchor remap (4-state); patch-id-chain carry-over across a
    multi-commit rebase is the follow-on (GIT-P13/M5, R-6). Name it.
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
  + CDC + drill tests pass; the GF-5 floor is named with its follow-on (GIT-P13); the work is committed.
- **COMMIT.** Header: P-<NNN> M3: content-anchored inline-thread line ranges (#sub 4-state resolver). Body
  lists: contract 5.7 (git sub-anchor resolver) implemented; GIT-D7 greened (0 mis-anchored, measured); the
  resolver mutation score measured; GF-5 floor named with follow-on GIT-P13. Branch first if on default; do not
  push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P9 — The code projection for search + leak-free fast lists at scale (the SetExpr push-down)

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G5 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G5").
- **DEPENDS-ON.** GIT-P3 (the object store the projection reads), GIT-P6 (the PR/repo entities the lists scan),
  GIT-P2 (the declare_indexable spec registered with Search). The M1 Identity list_objects SetExpr push-down
  (4.3 — the critical dependency). The M2 Search prompts that ship query conjoining the list_objects Filter +
  declare_indexable (6.1/6.3/6.5). The index places this after GIT-P6 and the M1 Identity + M2 Search work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (search references any artifact); ../../external-insights/01-process-and-quality-doctrine.md
    §3 (prove-it — 0 leak + one query are quantified gates).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/02-internals-and-algorithms.md §9 (the
    code-projection emitter); 03-events-contracts-and-glue.md (the list_objects consumer — SetExpr → SQL JOIN);
    00-overview.md §0.1 Δ5 (the SetExpr push-down — Ids|Filter{set_expr,zookie}, via_column lowering to repo.id/
    pr.id, the JOIN against Identity's per-tenant authz reverse index, no N+1/post-filter), §1.1 (the indexable
    code projection — git owns what to index).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-E (the SetExpr
    push-down).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 4.3 (list_objects → Ids |
    Filter{set_expr, zookie}; the SetExpr lowered to a SQL JOIN over the consumer's own id column via the
    per-tenant authz reverse index; no N+1, no post-filter), 6.1 (query always conjoins the list_objects Filter
    before scoring; search-requires-acl-filter lint), 6.3/6.5 (declare_indexable + the git.* code projection).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G5 + §2 (★ row 4.3) + §4 (rows 4.3, 6.3/6.5)
    + §5 (GF-3 trigram floor).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row GIT-D11 (partial-visibility PR list
    via the SetExpr JOIN) + the shared SRCH-D1/D3 (confidential code never in any result).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - The code-projection emitter (6.3/6.5): per changed blob, emit {path, language, symbols (camel/snake split),
    literals, commit message, text}; INCREMENTAL update on push (hooked into the GIT-P3 receive-pack
    post-commit path). Search builds the trigram indices (symbol/path/literal/trigram-grade v1, GF-3).
  - The list_objects SetExpr push-down wired for repo/PR lists AND the code-search pre-filter (4.3, OQ-E): lower
    the Ids | Filter{set_expr, zookie} to a SQL JOIN over git's own id column (repo.id / pr.id) against
    Identity's per-tenant authz reverse index — NO N+1, NO post-filter. ALWAYS conjoined before scoring
    (search-requires-acl-filter lint).
  - FLOOR named: trigram/lexical code search v1 (GF-3); the AST-aware "find usages" via CI-produced SCIP/LSIF
    (R-3) is the GIT-P13/M5 follow-on. Name it.
- **CONTRACTS TO IMPLEMENT.** 4.3 the list_objects consumer (owned — the SetExpr → SQL JOIN over repo.id/pr.id),
  6.3/6.5 declare_indexable + the code projection (owned — the emitter), 6.1 query Filter conjoin (consumed —
  the code-search pre-filter). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - GIT-D11 (SCHED): a viewer with PARTIAL repo/PR visibility lists a 100k-PR tenant → the SetExpr JOIN returns
    ONLY VISIBLE ROWS (0 leak), in ONE QUERY (no N+1, no post-filter); a just-revoked grant is reflected within
    the zookie bound. Green artifact: the 0-leak + 1-SQL-query + revoke-latency signals.
  - The search-requires-acl-filter lint green (the code-search query always conjoins the list_objects Filter;
    0 unfiltered scoring paths) — CI. Feeds the shared SRCH-D1/D3 (confidential code never in any result incl.
    counts/IDF) — git's projection asserted leak-free there.
- **TESTS (required).** Unit tests for the code-projection emitter (per-blob shape; incremental update on push)
  and the SetExpr → SQL JOIN lowering (via_column = repo.id/pr.id; one query; no post-filter). A chained e2e
  test (EI-01 §4): grant partial visibility → push code → list PRs → assert 0 leak + one query → revoke → assert
  reflected. The CDC pairs for rows 4.3/6.3/6.5. The SetExpr-lowering path is mandatory-core (a leak is the
  failure): state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** The code-projection emitter + the SetExpr JOIN exist; GIT-D11 emits its dated green
  artifact (0 leak, 1 SQL query, revoke reflected); the search-requires-acl-filter lint is green; the unit +
  chained-e2e + CDC + drill tests pass; the GF-3 floor is named with its follow-on (GIT-P13); the work is
  committed.
- **COMMIT.** Header: P-<NNN> M3: code projection for search + leak-free fast lists (SetExpr push-down). Body
  lists: contracts 4.3/6.3/6.5 implemented; GIT-D11 greened (0 leak, 1 query, measured); the SetExpr-lowering
  mutation score measured; GF-3 floor named with follow-on GIT-P13. Branch first if on default; do not push
  unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P10 — Code-executing git tools (history-rewrite, SCIP indexing) + agent authors/reviewers on the unified sandbox

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G6 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G6").
- **DEPENDS-ON.** GIT-P6 (the PR surface agents author into), GIT-P7 (the merge tool). The M2 Agent fabric
  prompts that ship ToolSurface + EffectApi + ToolHands::exec the unified sandbox (8.1/8.2/8.4) — and the HARD
  upstream gate: AG-D4 / CI-T1 (the real-kernel sandbox-escape GATE) GREEN on the production backend. The M1
  reserve/settle cost gate (11.7), Id mint_run_token (4.7). The index places this after GIT-P7 and ONLY after
  AG-D4 is green (master M2→M3 gate — no code-executing git tool runs until then).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native; mock agents only during development — strategy pattern, --use-mock);
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
    (history-rewrite as an audited op — built here as the tool; erasure semantics complete at GIT-P11).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G6 + §1 (sandbox escape is the shared AG-D4
    gate; git must not run code-executing tools until it is green) + §4 (rows 8.1/8.4, 10.6) + §5 (GF-9 MCP
    floor).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md AG-D4 (re-run on the git tool image) +
    AG-D1/D2/D3/D5 (git's tools assert they honour them).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate:
  - Register git's ToolDefs with the FROZEN requires_approval defaults (8.1, X-6): git.merge = YES, git.open_pr
    = NO. Code-executing tools (history-rewrite, SCIP indexing) go through EffectApi::apply (plan-then-apply)
    and ToolHands::exec (= the CI runner's kind=agent job, 8.4) — inheriting the FOUR UNIFORM GUARANTEES
    (reserve/settle cost gate, per-run attenuated token, HITL withhold, isolation floor + the real-kernel escape
    drill) BY CONSTRUCTION, never re-implemented.
  - Agents as FIRST-CLASS authors/reviewers (legible, bounded): an agent can open a PR, comment, review — via
    EffectApi, with mock runtimes during development (--use-mock, VISION §3 — no real agents integrated in dev).
  - The history-rewrite erasure path as an audited, rate-limited tenant op (10.6) with fork/mirror/clone-cache
    invalidation fan-out (built here as the TOOL; its erasure SEMANTICS complete at GIT-P11).
  - FLOOR named: GF-9 — exposed_over_mcp flags set, no external endpoint (the platform MCP server + threat model
    is the follow-on, P6+Legal). Name it.
- **CONTRACTS TO IMPLEMENT.** 8.1 git ToolDefs (owned — the requires_approval defaults), 8.2/8.4 EffectApi +
  ToolHands::exec (consumed — git's code-executing tools ride the unified sandbox), 11.7 reserve/settle
  (consumed — fronts every run), 4.7 mint_run_token (consumed — per-run token), 10.6 history-rewrite (owned —
  the audited op). Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - AG-D4 / CI-T1 is GREEN on the git tool image (re-run on the git tool image — the permanent escape gate;
    ZERO escapes). This is the upstream go/no-go: if AG-D4 is red on the git image, NO code-executing git tool
    runs and this prompt does NOT proceed — record a dated claimed-not-proven row and escalate.
  - Inherits AG-D1/D2/D3/D5 (CI): no write outside EffectApi; an effect outside the agent.policy ∩ delegation ∩
    tenant.policy intersection is DENIED; a HITL-gated tool (git.merge) is WITHHELD → 0 mutation pre-approval,
    1 apply post-approval. Green artifact: the per-run effect-attribution + 0-pre-approval-mutation signals.
  - The no-host-exec lint green (git's tools have no sandbox bypass) — CI.
- **TESTS (required).** Unit tests for the git ToolDef registration (defaults: merge=yes, open_pr=no) and the
  EffectApi plan-then-apply path. A chained e2e test (EI-01 §4): a mock agent opens a PR (no approval) → proposes
  a merge (gated) → withhold → assert 0 mutation → approve → assert 1 apply. The CDC pairs for rows 8.1/10.6.
  The AG-D4 re-run on the git tool image + the AG-D1/D2/D3/D5 scenarios. State the cargo-mutants mutation-score
  floor for the EffectApi-integration module and meet it.
- **DEFINITION OF DONE.** Git's ToolDefs are registered with the frozen defaults; the code-executing tools ride
  the unified sandbox with the four guarantees by construction; AG-D4 is green on the git tool image (PROVEN,
  not claimed); AG-D1/D2/D3/D5 emit their dated green artifacts on git's tools; mock agents can author/review;
  the history-rewrite tool exists (semantics complete at GIT-P11); the no-host-exec lint is green; the unit +
  chained-e2e + CDC + drill tests pass; the GF-9 floor is named with its follow-on; the work is committed. A red
  AG-D4 BLOCKS this prompt — it is not greened by weakening the assertion.
- **COMMIT.** Header: P-<NNN> M3: code-executing git tools + agent authors/reviewers on the unified sandbox.
  Body lists: contracts 8.1/8.2/8.4/11.7/4.7/10.6 implemented; AG-D4 re-confirmed green on the git tool image
  (0 escapes); AG-D1/D2/D3/D5 honoured (measured); the EffectApi-integration mutation score measured; GF-9 floor
  named. Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P11 — Erasure-reaches-every-holder + history-rewrite semantics + reindex-from-cold parity

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G7 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G7").
- **DEPENDS-ON.** GIT-P3 (the holder H1 + per-subject DEK), GIT-P4 (pseudonymous-by-default — the GIT-1 half),
  GIT-P9 (the search code index + the SetExpr lists), GIT-P10 (the history-rewrite tool). The M1 GDPR DSR
  orchestrator (10.1/10.4) + the erasure ledger (10.8). The M1 Storage crypto-shred (11.4). The M1 Id erase
  (4.8). The Bus reindex-from-source (2.6). The index places this after GIT-P10.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR — data subject rights; erasure by construction);
    ../../external-insights/04-hard-problems.md §1 (erasure vs immutability — every holder hit, the residual is
    the ONE posture); §5 (reindex-from-source — derived stores rebuild, never read owner DBs);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — every holder hit is quantified).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/05-hard-problems.md §HP-7 (the erasure
    posture completed); 03-events-contracts-and-glue.md §6 (the DSR fan-out over git; reindex-from-source for
    the check_status projection + code index + refs edges); 00-overview.md §1.1 (git is holder H1, the hardest
    in the platform) + §4 (inherited non-negotiable 6 — reindex is the only recovery path).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-7 (the ONE
    posture — instantiate by reference), §9 (history-rewrite audited op + invalidation fan-out).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 10.1 (PersonalDataHolder{locate/
    export/rectify/restrict/erase} over git + metadata), 10.4 (the DSR state machine), 11.4 (per-subject DEK
    crypto-shred — bodies/titles + reflogs/bitmaps/pack-backup reach), 4.8 (erase — the pseudonym-map shred,
    DSR step 1), 10.8 (the erasure ledger), 10.6 (history-rewrite audited op), 2.6 (reindex-from-source /
    replay), 10.9 (the ONE posture by reference).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G7 + §4 (rows 2.6, 10.1/10.4, 10.6, 11.4) +
    §5 (GF-7 floor — the lawful-basis residual is R-7, parallel-legal, not a code gate).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md rows GIT-D2 (erase reaches every holder)
    + GIT-D3 (reindex-from-cold parity).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate (the
  PersonalDataHolder impl + the reindex path):
  - DSR fan-out over git: pseudonym-map delete (Id, step 1) ⇒ immutable bytes hold only the opaque pseudonym;
    per-subject DEK crypto-shred for PR/review/comment BODIES + TITLES (11.4) reaching live + BACKUPS by
    construction; reflogs / bitmaps / pack-tier backups shreddable via the per-tenant blob DEK; the search code
    index purge+reindex; refs tombstone; cache/CDN invalidation (H9).
  - The history-rewrite path (10.6) as the supported disruptive op for PII-in-content (the rare case a body must
    be expunged), with the understood changed-hash consequence + the invalidation fan-out (the TOOL was built in
    GIT-P10; the erasure SEMANTICS complete here).
  - The residual is instantiated BY REFERENCE to the ONE platform posture (10.9 / X-7), NOT restated as a
    git-local statement. The [OPEN — LEGAL] Art. 17 ratification is R-7 (Legal/DPO, parallel).
  - reindex-from-source (2.6): replay rebuilds the check_status projection (from CI's ci.check.updated re-emit),
    the code index, and the refs edges — cold rebuild byte-matches live; NO cross-DB read.
  - FLOOR named: GF-7 — the structural floor (pseudonymous-by-default + per-subject DEK shred + history-rewrite)
    ships here regardless; the lawful-basis residual is one ratified statement (R-7, parallel-legal, not a code
    gate). Name it.
- **CONTRACTS TO IMPLEMENT.** 10.1/10.4 PersonalDataHolder + DSR (owned — locate/export/rectify/restrict/erase
  over git + metadata), 11.4 per-subject DEK crypto-shred (consumed — bodies/titles + reflogs/bitmaps/backups),
  4.8 erase (consumed — the pseudonym-map shred), 10.6 history-rewrite (owned — the audited op semantics), 2.6
  reindex-from-source (owned — the git replay), 10.9 the ONE posture (consumed by reference). Implement to the
  frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - GIT-D2 (SCHED, completed here): erase a subject who authored commits/PRs/comments + uploaded LFS → EVERY
    HOLDER HIT (pseudonym map, per-subject DEK bodies live+backups, reflogs/bitmaps/pack backups, search index,
    refs, cache/CDN); the residual is EXACTLY the ONE platform-posture residual (10.9), nothing more;
    crypto-shred reaches BACKUPS. Green artifact: the DSR receipt set + the erasure-ledger entry (0 holders
    missed; 0 recoverable PII beyond the named residual).
  - GIT-D3 (SCHED): wipe the Search code index + Refs edges + the check_status projection; reindex/replay → cold
    rebuild BYTE-MATCHES live (one code path, no drift); the check_status projection rebuilds from CI's
    ci.check.updated re-emit; NO cross-DB read. Green artifact: the reindex-parity hash (cold == live) + the
    no-cross-db lint green.
- **TESTS (required).** Unit tests for the holder locate/export/erase over git + metadata and the crypto-shred
  key choice (per-subject DEK for bodies/titles; per-tenant for reflogs/bitmaps). A chained e2e test (EI-01 §4):
  author content → erase subject → assert every holder hit + residual == the ONE posture + backups shredded. A
  reindex-parity test (cold rebuild byte-matches live; no cross-DB read). The CDC pairs for rows 10.1/10.4/2.6.
  The DSR fan-out + crypto-shred path is mandatory-core (a missed holder is a breach): state the cargo-mutants
  mutation-score floor and meet it.
- **DEFINITION OF DONE.** The DSR fan-out hits every git holder; GIT-D2 emits its dated green artifact (every
  holder hit, residual == the ONE posture, backups shredded) and GIT-D3 its reindex-parity artifact (cold ==
  live, no cross-DB); the history-rewrite semantics are complete; the residual is by reference to 10.9; the
  no-cross-db lint is green; the unit + chained-e2e + CDC + drill tests pass; the GF-7 floor is named with its
  follow-on (R-7, Legal); the work is committed.
- **COMMIT.** Header: P-<NNN> M3: erasure-reaches-every-holder + history-rewrite + reindex parity. Body lists:
  contracts 10.1/10.4/11.4/4.8/10.6/2.6/10.9 implemented; GIT-D2 greened (every holder hit, backups shredded,
  measured) + GIT-D3 (cold==live parity, measured); the DSR-fan-out mutation score measured; GF-7 floor named
  with R-7 (Legal). Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By
  trailer.

---

### GIT-P12 — Notifications, Web UI, CLI/API, and the M3 producer-band exit

- **BAND.** M3.
- **ROADMAP MILESTONE.** M3-G8 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M3-G8", the M3 band exit).
- **DEPENDS-ON.** GIT-P6 (PR/review/threads — the UI surface), GIT-P7 (the merge gate + checks panel + fork-trust
  badge), GIT-P8 (the inline-thread anchors), GIT-P2 (the signed-off design pass). The M2 Notif prompts that
  ship humanise (7.3), define_notif_rule (7.6), the ONE inbox (7.1). The M2 Refs resolve for unfurls (5.2). The
  index places this LAST in the M3 git band (it closes the producer-band exit aggregate).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (top-of-the-line UX; design sketches precede frontend; the switch test);
    ../../external-insights/05-ux-and-design.md (the design-language bar; overlays/states);
    ../../external-insights/01-process-and-quality-doctrine.md §4 (actually try it — drive the real UI in a
    browser before claiming done).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/04-views-cli-and-api.md (the views — IA
    + flows + states; the two CLI surfaces; the HTTP/RPC + agent-tool API); 00-overview.md §2 (D) (the
    notification routing); the signed-off design/ sketches (incl. the X-1 affordances from GIT-P2).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-L (humanise the
    ONE templating surface).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.6 (define_notif_rule —
    review-requested / PR-status / mention), 7.3 (humanise the ONE templating surface — confidential subject →
    humanised tombstone, title never leaks), 7.1 (the ONE inbox — review-requests are a filter, never a second
    store), 5.2 (resolve for unfurls).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M3-G8 + §6 (first useful = end of M3-G8) + §5
    (GF-6 single-file web-edit floor).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md NOTIF-D4-class (confidential git subject
    → humanised tombstone, title never leaks).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate + the git web UI app:
  - The git notification rules + targets via Signals (define_notif_rule 7.6: review-requested / PR-status /
    mention); the summary template keys resolved through humanise (7.3) per-viewer (the ONE templating surface —
    confidential subject → humanised tombstone, TITLE NEVER LEAKS). Review-requests appear as a filter over the
    ONE inbox (7.1), never a second store.
  - The Web UI: repo browse, code view, PR/review/inline-thread, the checks panel + fork-trust badge +
    merge-queue affordances (the X-1 design pass from GIT-P2), single-file WEB EDIT + commit (GF-6 floor — no
    3-way conflict editor in v1). Built against the REVIEWED design sketches (VISION §3); DRIVEN IN A BROWSER
    before "done" (the switch-test rehearsal; the full switch test is GIT-P15/M6).
  - The myelin CLI git surface + the HTTP/RPC + agent-tool API (arch 04).
  - FLOOR named: single-file web edit (GF-6 — in-browser conflict resolution is the follow-on, GIT-P13/M5+).
    Name it.
- **CONTRACTS TO IMPLEMENT.** 7.6 define_notif_rule (owned — git's rules), 7.3 humanise (consumed — the summary
  template keys), 7.1 the ONE inbox (consumed — the review-requests filter), 5.2 resolve (consumed — unfurls).
  Implement to the frozen shapes; escalate a needed change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - NOTIF-D4-class (CI): a confidential PR/commit subject → humanised TOMBSTONE; the title NEVER leaks in the
    notification (0 confidential titles in delivered notifications). Green artifact: the humanise-tombstone
    signal (0 title leaks).
  - The Web UI is DRIVEN IN A BROWSER (EI-01 §4 — the switch-test rehearsal): repo browse, PR review, the checks
    panel + fork-trust badge render against the signed-off sketches; the overlays/states (empty/loading/error)
    render correctly; no off-screen-picker / clipped-dialog regression (the shared overlay primitives hold).
    Recorded yes/no/partial per surface (untested-but-named is acceptable; silent skipping is not).
  - THE M3 BAND EXIT AGGREGATE: GIT-D9 + GIT-D8 + GIT-D11 + GIT-D7 + GIT-D2 are all GREEN (the master §2 M3
    git exit) ⇒ M3 done for git, M4 may start. Confirm each rests on a dated green artifact (the truth-up check).
- **TESTS (required).** Unit tests for the notif-rule registration + the humanise template keys. A browser-driven
  e2e walkthrough (EI-01 §4) of repo browse → PR review → checks panel → web edit + commit, recorded yes/no/
  partial. The CDC pairs for rows 7.6/7.3/7.1. State the cargo-mutants mutation-score floor for any
  mandatory-core module touched (the notif-rule matcher) and meet it.
- **DEFINITION OF DONE.** The notification rules + humanise wiring + the Web UI + CLI/API exist; NOTIF-D4-class
  emits its dated green artifact (0 title leaks); the Web UI is driven in a browser with states recorded; the M3
  band-exit aggregate (GIT-D9/D8/D11/D7/D2) is confirmed all-green-and-dated; the unit + browser-e2e + CDC tests
  pass; the GF-6 floor is named with its follow-on; the work is committed. This is FIRST USEFUL (roadmap §6):
  a team could host real repositories and review code (still on the local-disk/single-cell/single-lane floors;
  the X-1 seam fully live at M4).
- **COMMIT.** Header: P-<NNN> M3: git notifications + Web UI + CLI/API (the M3 producer-band exit). Body lists:
  contracts 7.6/7.3/7.1/5.2 implemented; NOTIF-D4-class greened (0 title leaks, measured); the Web UI driven in
  a browser (states recorded); the M3 band-exit aggregate confirmed all-green; GF-6 floor named with follow-on
  GIT-P13. Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### GIT-P13 — World-scale floor follow-ons: object-backed packs, cross-cell replication, speculative queue, SHA-256, SCIP

- **BAND.** M5.
- **ROADMAP MILESTONE.** M5-G9 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M5-G9", the named floor
  follow-ons half; the world-scale surge drills + E2E slices are GIT-P14).
- **DEPENDS-ON.** GIT-P3..GIT-P12 (all the M3 floors this promotes). The M4 exit GREEN (all five subsystems
  exist; the deterministic correctness drills green — master M4→M5 gate). The M5 Storage object-store BlobStore
  swap (11.2). The M5 multi-cell bridge (12.6, OQ-I). The index places this in M5 after the M4 exit.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale means world-scale; name-your-floors — the follow-on lands as its own
    committable unit); ../../external-insights/04-hard-problems.md §3 (world-scale git — the explicit local-disk
    → object-backed transition, sequenced not bolted on); §2 (CRDT-after-CAS pattern parallel — not git's).
  - Architecture: ../04-subsystem-architectures/git-hosting/architecture/05-hard-problems.md (object-backed
    packs, replication, the SHA-256 flip, the speculative queue — the named floors); 02-internals-and-algorithms.md
    (replication TE-24, GC/repack, the merge queue); 01-tech-and-data-model.md §1 (the BlobStore fs↔object swap,
    the hash-agnostic model).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §8 (the
    object-backed pack/delta seam, the within-EU CDN clone class, the trust-scoped cache), OQ-I (the cross-cell
    bridge).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 11.2 (BlobStore object-backed
    pack/delta seam + the within-EU CDN clone/bundle class; the fs↔object one-line swap), 12.2 (repo-granular
    relocatable placement — repos were never node-pinned, so this is a BlobStore-impl swap + a transport path,
    not a data-model rewrite), 12.6 (the cross-cell PII-free pointer bridge), 6.5 (SCIP/LSIF follow-on).
  - Roadmap: planning/06-roadmaps/subsystems/git-hosting.md §3 M5-G9 (the named floor follow-ons) + §5 (the
    floors register — GF-1/GF-2/GF-2b/GF-3/GF-5/GF-8 follow-ons; the OQ-1 gitoxide spike, OQ-4/OQ-5/OQ-9 the
    investigations).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-git crate (each follow-on its own
  bounded, committable slice; the prompt may be executed as a sequence of commits if a single agent splits it,
  but each follow-on must be named done with its own gate):
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

### GIT-P14 — World-scale hardening (the F6 surge family) + git's slices of the four whole-system E2E scenarios

- **BAND.** M5.
- **ROADMAP MILESTONE.** M5-G9 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M5-G9", the world-scale
  hardening + the E2E-slice half).
- **DEPENDS-ON.** GIT-P13 (the promoted floors — surge drills run against the object-backed/cross-cell git). The
  M4 exit GREEN (CI's CheckStatus producer closed the X-1 seam end-to-end — GIT-D10/CI-D8). The M5 prompts of
  the other four subsystems (CI/Issues/Chat/Knowledge) + Refs/Search/Id/Notif for the cross-subsystem E2E
  wedge. The index places this after GIT-P13 and the M4 seam closure.
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
    (built on GIT-P13's replication); the CLONE-STORM SHED (the OQ-K per-surface shed budget tuned by GIT-D6);
    cross-tenant fairness; prod-scale benchmarks (100k-PR list, monorepo ceiling); online-migration-under-load;
    restore-verify at cell scale.
  - Git's contribution to the four whole-system E2E scenarios (testing-strategy §2): E2E-1 PR context pane (git
    is the PR host + the reference producer); E2E-2 CI-fail → triage agent → issue → chat → fix-PR (git hosts
    the fix-PR; the git.merge HITL approval + the X-1/GIT-D10 gate + git.pr.merged closing the issue via the
    Closes trailer — the agent-native flagship); E2E-3 Spec-to-ship traceability (git provides the
    commit→PR→merge lineage; cold-reindex == live).
  - FLOOR named: none new — this prompt PROVES the promoted floors under load; record any residual surge-budget
    tuning as a dated note.
- **CONTRACTS TO IMPLEMENT.** 1.8 the telemetry survival signals (consumed — the surge assertions read from
  the metrics port), 1.11 the shed order under surge (consumed). No new owned contract — this is the drill +
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

### GIT-P15 — Dogfood: Myelin hosts its own repositories (the switch test)

- **BAND.** M6.
- **ROADMAP MILESTONE.** M6-G10 (planning/06-roadmaps/subsystems/git-hosting.md §3 "M6-G10").
- **DEPENDS-ON.** GIT-P14 (git is world-scale-ready and the E2E wedge is proven). The M5 exit GREEN (the
  platform is world-scale-ready — master M5→M6 gate; you do not dogfood real team data onto a substrate whose
  restore-verify and DSAR fan-out are not green). The M6 CI/Issues/Knowledge dogfood prompts (the self-hosting
  CI graph + the roadmap-as-issues). The index places this last in the git ledger.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (the switch test — top-of-the-line UX, driven in a browser); §5 (dogfooding);
    ../../external-insights/01-process-and-quality-doctrine.md §4 (the switch test — drive the real UI, not the
    feature list; §1 the truth-up pass — every PROVEN row rests on a dated green artifact).
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
  - FLOOR named: none — this is the done-bar. Record any switch-test wall found (a place the old tool did better)
    as a dated gap-report item with its follow-on owner.
- **CONTRACTS TO IMPLEMENT.** No new contract — this prompt drives the real UI + closes the dogfood loop +
  runs the truth-up pass. The mandatory-core mutation gate (1.6) now runs as a Myelin CI job.
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

| Roadmap milestone (planning/06-roadmaps/subsystems/git-hosting.md) | Band | Prompt(s) | Primary drills greened |
|---|---|---|---|
| Pre-work M1 (ReBAC fragment + git.* tokens + holder tags) | M1 | GIT-P1 | (compile + no-untagged-personal-data lint) |
| Pre-work M2 (#sub mints + declare_indexable + X-1 consumer decl + design pass) | M2 | GIT-P2 | (compile + design sign-off) |
| M3-G1 (object store + receive-pack + data-loss floor) | M3 | GIT-P3 | GIT-D9, GIT-D1 |
| M3-G1 (pseudonymous-by-default commits — the data-model gate) | M3 | GIT-P4 | GIT-D2 (GIT-1 half) |
| M3-G2 (front door + authz + residency + shed) | M3 | GIT-P5 | GIT-D8 |
| M3-G3 (PRs/reviews/threads + ref edges + project) | M3 | GIT-P6 | render==md, 1-edge-per-node |
| M3-G4 (merge gate + check_status projection + fork-endorse + merge queue) | M3 | GIT-P7 | GIT-D10 (synthetic) |
| M3-G4 (content-anchored line ranges) | M3 | GIT-P8 | GIT-D7 |
| M3-G5 (code projection + leak-free SetExpr lists) | M3 | GIT-P9 | GIT-D11 |
| M3-G6 (code-executing tools + agent authors on the unified sandbox) | M3 | GIT-P10 | AG-D4 (git image), AG-D1/D2/D3/D5 |
| M3-G7 (erasure-reaches-every-holder + history-rewrite + reindex parity) | M3 | GIT-P11 | GIT-D2 (complete), GIT-D3 |
| M3-G8 (notifications + Web UI + CLI/API; the M3 band exit) | M3 | GIT-P12 | NOTIF-D4-class; the M3 exit aggregate |
| M5-G9 (object-backed packs, cross-cell, speculative queue, SHA-256, SCIP) | M5 | GIT-P13 | GIT-D4, GIT-D5 |
| M5-G9 (the F6 surge family + the E2E slices) | M5 | GIT-P14 | GIT-D6, E2E-1/E2E-2/E2E-3, STOR-D2 |
| M6-G10 (dogfood + the switch test) | M6 | GIT-P15 | Git OQ-12 switch test; self-hosting CI graph green |

Floors (each named in its prompt with its follow-on prompt): GF-1 local-disk packs (GIT-P3) → object-backed
(GIT-P13); GF-2 single-cell (GIT-P3) → cross-cell (GIT-P13); GF-2b SHA-1+sha1dc (GIT-P3) → SHA-256 flip
(GIT-P13); GF-3 trigram search (GIT-P9) → SCIP find-usages (GIT-P13); GF-5 per-pair anchor (GIT-P8) →
patch-id-chain (GIT-P13); GF-6 single-file web edit (GIT-P12) → in-browser conflict (GIT-P13); GF-7
pseudonymous-by-default + DEK shred + history-rewrite (GIT-P4/GIT-P11) → the [OPEN — LEGAL] lawful-basis
residual (R-7, parallel/Legal); GF-8 single-lane queue (GIT-P7) → speculative queue (GIT-P13); GF-9 MCP flags
(GIT-P10) → platform MCP server (GIT-P13/P6+Legal); the X-1 seam-floor — synthetic producer (GIT-P7) → real CI
producer end-to-end at the M4 exit (GIT-D10/CI-D8).
