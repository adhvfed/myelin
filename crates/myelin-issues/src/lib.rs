//! # `myelin-issues` — the Issue-tracker subsystem (the M1 freeze-so-dependents-compile slice)
//!
//! Issue-tracker is the **most cross-subsystem-coupled** consumer subsystem (architecture
//! issue-tracker 00-overview §1): it references Git commits/PRs, reads CI's `CheckStatus`, embeds
//! Knowledge docs, turns Chat messages into issues, pages on-call on SLA breach, and is driven by
//! agents. Its feature bulk lands in M4; this crate carries its **M1 contract freeze** — the
//! relation/holder SHAPES dependents compile against, ahead of that bulk (roadmap §3.0):
//! - [`rebac_fragment`] — **ISS-P01 / P-125**: the frozen Issues ReBAC namespace fragment (contract
//!   4.9) Identity compiles into the one cell schema — the `issue` namespace + the `- confidential`
//!   set-difference userset + `watcher` (Notif read-fanout) + the `issue_field` / `issue_transition`
//!   ABAC sub-objects. Names freeze here; the permission rewrites + the `CaveatContext` field/
//!   transition redaction are wired LIVE on Identity's M2 `list_objects`/`CaveatContext` bodies
//!   (ISS-P11 / P-ID-*).
//! - [`holder_intent`] + [`schema`] — **ISS-P01 / P-125**: the H3 holder INTENT + the
//!   `#[personal_data(...)]` classification tags (see below).
//! - [`query_coown`] — **ISS-P02 / P-241**: Issues **co-owns `myelin-query` byte-identical** with
//!   Knowledge (contract 13.3, X-3/OQ-C). The four shared shapes (the `FieldType` enum, the
//!   `ViewSpec` view-model, the `QueryAst` grammar, the `order_key`/LexoRank codec) are **linked**
//!   from the frozen shared crate — *the same bytes* Knowledge uses, never a re-implementation — and
//!   the byte-identity drift-killer (`tests/cdc_13_3_issues_coown.rs`) replays the shared
//!   conformance vector + serializes the shared `ViewSpec` *from the Issues crate* and proves **0
//!   byte differences** vs Knowledge's frozen outputs. NO Issues data is written yet; Issues' own
//!   AST→store compiler lands in **ISS-P13**, the `order_key` CAS reorder in **ISS-P09**, the
//!   co-equal views in **ISS-P16** (floors named in [`query_coown`]).
//! - [`events`] — **ISS-P03 / P-242**: the complete `issue.*` **event taxonomy** Issues owns
//!   (arch §1), **registered against the frozen Bus §6 grammar** (contract 2.9) — the named
//!   `&'static str` token constants + the [`events::ISSUE_EVENT_TOKENS`] registry, each PROVEN
//!   grammatical by the ONE Bus validator (`myelin_events::validate_event_type`; **0 ungrammatical
//!   tokens**). Includes the registered **`initiative`** type token
//!   ([`events::INITIATIVE_HEALTH_CHANGED`], recon §2 / §6.2). Plus the Issues-side
//!   [`events::unit_check`] pinning the EventEnvelope unit anchor (contract 2.1) for issue
//!   payloads — durations in **seconds**, timestamps RFC-3339 UTC — with a loud seconds-vs-millis
//!   rejection. **NO Issues data is written yet**: these tokens are REGISTERED (a names freeze);
//!   the emit bodies attach to them via `OutboxTx::emit` in the write path (ISS-P06 / P-372) on the
//!   issue-spine migrations (ISS-P05 / P-371) — the floor named in [`events`].
//! - [`declares`] — **ISS-P04 / P-243**: the Issues **`declare_indexable` IndexSpec** (contract
//!   6.3 — the `issue.*` facets projection: the seven structured board/list/search facets +
//!   `acl_object_type = "issue"`, registered with + accepted by Search) and the **`define_notif_rule`
//!   reason set** (contract 7.6 — SLA-at-risk / unblocked / approval-requested, registered against
//!   Notif's ONE §3.1 ranking table). Both construct the ONE frozen consumer-owned shape
//!   ([`myelin_search::IndexSpec`] / [`myelin_notif::NotifRule`]); Issues defines no second indexing
//!   contract and no second reason vocabulary (EI-01 §7). **NO emitter / NO wiring ships here**: the
//!   live `issue.*` Search projection emitter is **ISS-P17** and the live trigger/SLA → Notif inbox
//!   wiring ("My Work") is **ISS-P22** — both floors named in [`declares`].
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md`
//! §6.1 (the Issues ReBAC fragment — the `issue` namespace + the `- confidential` set-difference +
//! `watcher` + the field/transition caveat sub-objects) + §7 (the `PersonalDataHolder` H3 holder +
//! the erase table), `00-overview.md` §1 (the most-coupled posture) + §2.2 (thin-shell-over-
//! identical-plumbing), and `01-tech-and-data-model.md` §6.1 (the schema types the tags apply to —
//! the pseudonymous identity fields + the free-text bodies + the OQ-H worklog/productivity fields).
//!
//! **Contract-index rows:**
//! - **4.9** the per-subsystem ReBAC namespace fragment — Issues OWNS *this fragment's definition*
//!   (the frozen [`rebac_fragment`] carriers); Identity owns the engine + admit-contract. The gate
//!   of this prompt is the **build-time compile**: Identity's cell schema compiles against the Issues
//!   fragment (a build-time property, not a runtime drill).
//! - **10.1** `PersonalDataHolder{locate, export, rectify, restrict, erase}` — Issues declares the
//!   **H3 INTENT** here (the holder is OPENED + auto-registered by `serve` when the store opens in
//!   ISS-P07). The trait BODY (the real locate/erase fan-out, the §7 erase table) is the ISS-P07
//!   floor, not built here.
//! - **10.2** the `#[personal_data(category, role, basis, retention, erasure, subject_locator)]`
//!   classify-derive — APPLIED here to every PII-carrying field of the (still-skeletal) issue schema
//!   types so the `no-untagged-personal-data` lint (contract 1.6) is **green from the first
//!   migration** (ISS-P05). The OQ-H worklog/productivity/estimate fields carry the frozen
//!   behavioural tags (`category = Behavioural`, `basis = TBD_LEGAL` — the `[OPEN — LEGAL]` residual
//!   R-2, restricted-by-default). The macro is a NO-OP at its M0 floor (P-050); applying it freezes
//!   the classification so the lint admits the schema + the M4 stores compile against the tags.
//!
//! ## What this prompt (ISS-P01 / P-125) ships — and what it deliberately does NOT
//! **Ships:** the [`rebac_fragment`] freeze (the three Issues object types + the set-difference
//! `view` shape), the [`holder_intent`] declaration (Issues = holder H3, the §7 personal-data
//! inventory encoded as data), and the [`schema`] module — the skeletal issue OLTP row types
//! (`Issue`, `IssueComment`, `IssueChangeLog`) carrying the `#[personal_data(...)]` tags on their
//! pseudonym + free-text-body + OQ-H worklog fields. The goal is the GATE: Identity's cell schema
//! compiles against the Issues fragment AND the `no-untagged-personal-data` lint is green on the
//! issue skeleton (0 untagged PII fields), with a red-fixture witness proving the lint still REJECTS
//! a deliberately-untagged Issues PII field.
//!
//! **Does NOT ship (floors named — VISION §3 name-your-floors):**
//! - **No Issues FEATURE.** No board scan, no write path, no `list_objects`/`CaveatContext`
//!   evaluation, no migrations. The schema types here are skeletal row-shape carriers for the tags,
//!   not the live tables; the fragment is the relation/permission SHAPES, not a runtime check.
//! - **The holder is NOT opened/registered here.** It is declared as INTENT (data). The holder is
//!   actually **OPENED and auto-registered by `serve`** when the issue store opens in **ISS-P07**;
//!   the `PersonalDataHolder` trait BODY (the §7 erase table: pseudonym-map shred + per-subject DEK
//!   crypto-shred + Search purge + Refs tombstone) lands in **ISS-P07** and the GDPR producer-holder
//!   wiring **P-GA-27 (M3)**.
//! - **The classify-derive macro BODY** (parsing the tags into the data-map/RoPA registry) is the
//!   GDPR floor **P-GA-07 (M1)**; here the derive is the no-op floor (P-050) and the tags are the
//!   classification facts a store applies today.
//! - **The fragment permission REWRITES + the `CaveatContext` field/transition redaction** are wired
//!   LIVE on Identity's M2 bodies (ISS-P11 / P-ID-*); here only the NAMES freeze (the rewrite
//!   structure is documented + proven admissible by the CDC against the real engine).
//!
//! ## The `[OPEN — LEGAL]` worklog residual (R-2, OQ-H)
//! The OQ-H worklog/productivity/estimate fields are tagged `category = Behavioural`,
//! `basis = TBD_LEGAL` (a NAMED residual recorded against the field, never a blocker) — counsel/DPO
//! ratify whether they are special-category (Art. 9) or merely elevated, and the works-council
//! consultation trigger per jurisdiction. The **structural floor ships now**: the fields are
//! restricted-by-default (excluded from cross-individual analytics + agent-use for a restricted
//! subject), per-individual rollups are off-by-default behind tenant-admin enablement, and they carry
//! the same per-subject DEK crypto-shred as other free-text PII. (Recon §OQ-H, contract 10.2; the
//! ratification is a parallel legal track — P-GA-08's DPIA router consumes the `SpecialCategory` flag
//! if counsel reclassifies.)

//! ## ISS-P05 / P-371 (M4) — the issue-spine migrations + the bootable service shell
//! M4 opens with the schema floor under all of Issues. [`app`] assembles the Issue Tracker
//! [`myelin_substrate::AppSpec`] the harness drives (boot → migrate → relay → three ports → graceful
//! drain, liveness ≠ readiness — contract 1.1, the EXACT analog of the CI / Search / Refs shells);
//! [`migrations`] is the COMPLETE forward-only issue-spine data model (arch 01 §2–§8: the `issue`
//! typed-core + JSONB-tail spine, `issue_relation` TE-7 source-of-truth, `issue_change_log`, the
//! `scheme`/`scheme_assignment`/`cycle`/`cycle_membership`/`milestone`/`prefix_counter` tables, the
//! platform `consumer_dedup` + `outbox`), each domain table `(tenant_id, region)`-first + RLS-on
//! (11.1/12.1/1.5) with `issue`/`issue_relation`/`issue_change_log` flagged HOT (§8.1); [`holder`]
//! is the auto-registered **H3** `PersonalDataHolder` (locate/export typed, erase stubbed to
//! crypto-shred naming ISS-P31, the `restrict` flag wired — contract 10.1/1.4). The
//! silent-data-loss-safe write path is ISS-P06 (P-372). Storage floor = PG-hybrid sharded by tenant;
//! distributed-SQL is the measured R-6 follow-on (ISS-P32) — named in [`migrations`].
//!
//! ## ISS-P07 / P-373 (M4) — pseudonymous-by-default identity columns + per-subject-DEK free-text
//! The GDPR-safe-by-construction half (VISION §3): the structural erasure floor (recon §X-7) ships at
//! the COLUMN.
//! - [`pseudonym`] — **the pseudonymous-by-default identity columns (contract 4.8).** The Issues
//!   `assignee` / `reporter` / `created_by` / comment-author / change-log-actor columns hold an OPAQUE
//!   `<pseudonym>@<tenant>.noreply` handle ([`myelin_identity::PseudonymHandle`], the SAME frozen
//!   grammar the Git commit codec bakes into immutable bytes) — NEVER a raw id (the 0-raw-id GATE).
//!   [`pseudonym::pseudonymise`] resolves a subject through the ONE Identity person↔pseudonym map
//!   ([`resolve_pseudonym`], 4.8 — the erasable record); a value not in the grammar is REFUSED.
//! - [`dek`] — **the per-subject-DEK free-text columns (contract 11.3/11.4).** The free-text `title` /
//!   `props` / `change_delta` / comment body are sealed under the SUBJECT's per-subject DEK through the
//!   ONE shared [`myelin_storage::encryption::ColumnCryptor`] over the P-058 `KmsEngine` (rotation /
//!   crypto-shred reach by construction, EI-01 §7) — ciphertext + the `pii_key_ref` DEK metadata at
//!   rest, **0 plaintext free-text** (the at-rest GATE). A subject's erasure destroys their DEK ⇒ their
//!   free-text in DBs + backups + immutable logs is unrecoverable ciphertext (the GD-4 individual lever).
//! - [`write_path::apply_mutation_sealed`] wires BOTH through the ISS-P06 seam: it pseudonymises the
//!   reporter + seals the free-text BEFORE validate → check → mutate → emit, threading the REAL
//!   `kms://<tenant>/<epoch>/subject:<id>` `pii_key_ref` onto the `issue.created` event (the erase
//!   fan-out destroys exactly that key). A pseudonymise/seal failure FAILS THE WRITE CLOSED.
//! - The **H3 holder is registered** ([`holder`], 10.1/1.4 — confirmed in place from ISS-P05, the ops
//!   still stubbed). **FLOORS named:** the `erase` crypto-shred + pseudonym-map shred BODY (the full
//!   DSR fan-out) is **ISS-P31**; the third-party free-text residual is the ONE platform posture
//!   (10.9 / X-7, by reference, [`holder::ISSUE_RESIDUAL_POSTURE_REF`] — `[OPEN — LEGAL]`, R-1).
//!
//! ## ISS-P08 / P-374 (M4) — Hi/Lo human-key allocation (the `<PROJECTKEY>-<seqno>` canonical id)
//! [`keys`] ships the per-prefix **Hi/Lo** allocator that mints the issue's **stored canonical id**
//! `<PROJECTKEY>-<seqno>` (contract 5.1, recon REF-3) — the stored `<id>` segment of the issue's
//! [`myelin_events::ArtifactRef`]. The `prefix_counter` table (ISS-P05) holds the durable **Hi** block
//! high-water; [`keys::HiLoKeyAllocator`] reserves a block atomically over the [`keys::PrefixReserve`]
//! port (the live `UPDATE … RETURNING` in production) and hands out the **Lo** seqnos from memory (1
//! counter write per block, not per key). Gap-tolerant (a leaked block on crash is a benign gap, never
//! a reuse), monotonic per prefix, adaptive block size (50 → 1000 on a hot prefix), per-prefix
//! isolation (`ENG` never slows `OPS`), cell-local. [`write_path::create_issue`] slots the allocation
//! into the ISS-P06 write path — the minted canonical key co-commits with the issue's `issue.created`
//! event (replacing the ISS-P06 placeholder). **FLOOR named:** the render-time `#<seqno>` projection
//! ([`keys::render_display_key`]) is display-only, never stored, never an `ArtifactRef` link.
//!
//! ## ISS-P10 / P-376 (M4) — the issue body + comments as a `myelin-content` block subtree
//! [`content`] makes an issue's **description body** and its **comments** real [`myelin_content`]
//! documents: a **block subtree** (the consumed Issues SUBSET of the frozen contract-13.1 [`Block`]
//! taxonomy — the full block set MINUS the Knowledge-only `db_view`/`sync_block`/`toggle`, X-2) whose
//! every inline run round-trips `render(parse(md)) === md` through the **ONE WASM render path** (the
//! SAME [`myelin_content::parse_inline`]/[`myelin_content::serialize_inline`] compiled native on the
//! server and to `wasm32-unknown-unknown` for the editor — read + edit use the IDENTICAL parser, no
//! second renderer, EI-01 §7). [`content::validate_subtree`] REJECTS a Knowledge-only node LOUDLY
//! (never a silent drop); [`content::IssueContent::cas_edit`] is the **single-author version-token
//! CAS** (arch §1.3 — NOT the Knowledge CRDT; a stale write loses loudly); [`content::emit_content_event`]
//! co-commits the body/comment `issue.*` event through the ONE [`myelin_events::OutboxTx::emit`]
//! (contract 2.2, emit-iff-committed). This completes M4-I1's first-runnable create → key → edit →
//! link → reorder loop. **FLOORS named:** the move-CRDT body collaboration is OUT of v1 scope
//! (single-author version-CAS is the floor); the at-rest per-subject-DEK body ciphertext is the
//! storage layer's ([`dek`], ISS-P07 — `content` is the cleartext document); the structured-node →
//! `refs.edge.created` emission is the Issues Refs-producer band (the node walk is
//! [`content::IssueContent::structured_nodes`]).
//!
//! ## ISS-P11 / P-377 (M4) — governance schemes + the precedence algebra + the flexible-field model
//! [`schemes`] ships the BEHAVIOUR over the ISS-P05 `scheme`/`scheme_assignment` tables (the schema
//! floor named this prompt): the five interpreted scheme kinds ([`schemes::SchemeKind`] —
//! workflow/field/permission/sla/type, byte-identical to the `scheme.kind` CHECK vocabulary), the
//! deterministic, CACHED, off-the-hot-path scheme-precedence algebra
//! ([`schemes::SchemeResolver`]/[`schemes::resolve`] — most-specific-wins over the fixed eight-row
//! `(type × project × team)` lattice; the write loads the ALREADY-RESOLVED compiled scheme via
//! [`schemes::SchemeResolver::load_resolved`], never resolves precedence inline) and the
//! flexible-field model ([`schemes::FlexibleField`]/[`schemes::add_flexible_field`] — a custom field
//! is a JSONB `props` write + a default-GIN-indexable facet over `issue_props_gin`, NEVER a DDL). A
//! scheme reassignment is a CONFIG write — [`schemes::Reassignment`] proves `issue_rows_touched == 0`
//! (the no-config = Linear-simple gate). The `FieldType` on a `field` scheme is the frozen
//! [`myelin_query::FieldType`] (contract 13.3 — no second vocabulary). FLOORS named: the hierarchy is
//! a TREE parent (constrained-DAG portfolios are M5+, [`schemes::TypeDef`]); the projection-feeder
//! generated-index promotion is ISS-P15 (a cold facet rides the GIN index until a measured OQ-C
//! threshold, [`schemes::IndexPosture`]). The FSM interpreter + the QueryAst guards that RUN the
//! workflow body are the next prompt, ISS-P12.
//!
//! ## ISS-P12 / P-378 (M4) — the data-driven workflow FSM interpreter + the QueryAst guards
//! [`workflow`] ships the BEHAVIOUR over the ISS-P11 `workflow`-scheme config: the data-driven FSM
//! interpreter ([`workflow::Workflow`] / [`workflow::Workflow::plan_transition`]) over a CONFIG FSM —
//! not codegen, not user-scripting (EI-01 §7). The ONE mandatory governance invariant is the FIXED
//! [`workflow::StateCategory`] set (unstarted/started/completed/cancelled) over unlimited admin-named
//! [`workflow::WorkflowState`]s; guards are the frozen [`myelin_query::QueryAst`]
//! ([`workflow::WorkflowGuard`] — bounded, no UDFs/loops/recursion); required-fields-on-transition;
//! and the post-actions ([`workflow::PostAction`] — assign/set-field/link/arm-trigger), the
//! arm-trigger carrying the frozen [`myelin_query::EventMatcher`] = the SAME `QueryAst` core (contract
//! 3.4). A blocked transition returns a pre-assembled, admin-authored reason
//! ([`workflow::TransitionBlocked`]) — never a silent allow, never a silent drop; an un-evaluable
//! guard fails CLOSED. The interpreter is the pure governance decision the ISS-P06 write path then
//! drives (the transition ABAC — `Id.check` + the transition `CaveatContext`, contract 4.2 — is
//! already wired on [`write_path::apply_mutation`] for a [`write_path::MutationKind::Transition`]); the
//! interpreter NEVER emits (emit is the ONE `OutboxTx::emit` verb). The ISS-D12 guard-half green
//! artifact is the canonical [`workflow::blocked_by_guard`] ("can't close while `blocked_by` an open
//! issue" → blocked + reason). The flow-determinism lint (1.6) holds on the arm-trigger workflow body
//! ([`workflow::arm_trigger_body`] reads time through `WfCtx`, never a raw clock). **FLOOR named:** the
//! CI-red guard half of ISS-D12 ("can't mark Done while CI red on the linked PR" — the X-1
//! `CheckStatus` + `trust_tier` off the fact, contract 5.9 / Δ10) lands in **ISS-P27 (P-394)** when the
//! X-1 check-seam closes; the guard SHAPE is here ([`workflow::linked_pr_ci_green_guard`], fails closed
//! until then).

#![forbid(unsafe_code)]

pub mod app;
pub mod content;
pub mod cost_bounder;
pub mod declares;
pub mod dek;
pub mod events;
pub mod holder;
pub mod holder_intent;
pub mod keys;
pub mod migrations;
pub mod planner;
pub mod projection_feeder;
pub mod pseudonym;
pub mod query_coown;
pub mod rebac_fragment;
pub mod reorder;
pub mod replay;
pub mod schema;
pub mod schemes;
pub mod sla_escalation;
pub mod views;
pub mod workflow;
pub mod write_path;

pub use replay::{IssueReindexSource, IssueReplayKind};

// ISS-P05 (P-371, M4): the bootable service shell + the complete issue-spine data model + the H3
// holder. The `serve(AppSpec)` shell (contract 1.1), the forward-only migrations (1.5/11.1/12.1), and
// the auto-registered PersonalDataHolder (10.1/1.4).
pub use app::{boot_issues, issues_app_spec, run_issues, SERVICE_NAME};
pub use holder::{
    issue_store_classifier, register_issue_holders, IssueHolder, IssueHolderRegistration,
    IssueStoreClass, RestrictionFlag, ISSUE_OLTP_STORE, ISSUE_RESIDUAL_POSTURE_REF,
};
pub use migrations::{
    issues_hot_tables, issues_migrations, make_tenant_scoped_ddl, CONSUMER_DEDUP_TABLE,
    CREATE_CONSUMER_DEDUP_DDL, CREATE_CYCLE_DDL, CREATE_CYCLE_MEMBERSHIP_DDL,
    CREATE_ISSUE_CHANGE_LOG_DDL, CREATE_ISSUE_DDL, CREATE_ISSUE_INDEXES_DDL,
    CREATE_ISSUE_RELATION_DDL, CREATE_ISSUE_RELATION_INDEXES_DDL, CREATE_MILESTONE_DDL,
    CREATE_PREFIX_COUNTER_DDL, CREATE_SCHEME_ASSIGNMENT_DDL, CREATE_SCHEME_DDL,
    CYCLE_MEMBERSHIP_TABLE, CYCLE_TABLE, ISSUE_ASSIGNEE_INDEX, ISSUE_BOARD_INDEX,
    ISSUE_CHANGE_LOG_TABLE, ISSUE_CYCLE_INDEX, ISSUE_PARENT_INDEX, ISSUE_PROPS_GIN_INDEX,
    ISSUE_RELATION_TABLE, ISSUE_ROADMAP_INDEX, ISSUE_TABLE, MILESTONE_TABLE, OUTBOX_TABLE,
    PREFIX_COUNTER_TABLE, SCHEME_ASSIGNMENT_TABLE, SCHEME_TABLE,
};

// ISS-P06 (P-372, M4): the silent-data-loss-safe write path. `apply_mutation` runs validate →
// Id.check (+ CaveatContext) → mutate the typed core → OutboxTx::emit IN ONE TRANSACTION (the issue
// is the aggregate, UNIQUE(aggregate, seq)); emit-iff-committed (0 ghost / 0 lost), the no-raw-publish
// lint holds (emit is the only path), write_tuples + the zookie wired on the relation-changing
// mutations (4.6/4.10). Floors named in `write_path`: the canonical key is ISS-P08, the order_key CAS
// reorder is ISS-P09, the myelin-content body is ISS-P10.
pub use write_path::{
    apply_mutation, issue_aggregate_key, issue_ref, IssueDraft, MutationKind, WriteError,
    WriteOutcome, PERM_COMMENT, PERM_MANAGE, PERM_PERFORM_TRANSITION, PERM_TRANSITION,
};

// ISS-P07 (P-373, M4): pseudonymous-by-default identity columns (4.8) + per-subject-DEK free-text
// (11.4) + the holder registration (10.1/1.4). `pseudonymise`/`IssuePseudonym` (the 0-raw-id identity
// columns), `encrypt_free_text`/`IssueFreeText` (the 0-plaintext-at-rest free-text columns), and
// `apply_mutation_sealed` (the GDPR-safe write path threading both onto the ISS-P06 seam + the REAL
// per-subject-DEK pii_key_ref). Floors named: the erase crypto-shred + pseudonym-map shred fan-out is
// ISS-P31 (`holder`); the third-party free-text residual is the ONE posture 10.9/X-7, by reference.
pub use dek::{
    decrypt_free_text, encrypt_free_text, plaintext_at_rest, subject_dek_erasure, IssueFreeText,
};
pub use pseudonym::{
    is_raw_principal_id, is_resolvable_pseudonym, pseudonymise, IssuePseudonym, PseudonymError,
};
pub use write_path::{apply_mutation_sealed, SealError, SealedCreate};

// ISS-P08 (P-374, M4): the Hi/Lo human-key allocator. `HiLoKeyAllocator` mints the stored canonical
// `<PROJECTKEY>-<seqno>` id (5.1) over the `PrefixReserve` port (the live `prefix_counter`
// `UPDATE … RETURNING` reserve); gap-tolerant, monotonic per prefix, adaptive block size, per-prefix
// isolation, cell-local. `CanonicalKey::render` is the stored id; `render_display_key` is the
// display-only `#<seqno>` projection (REF-3). `write_path::create_issue` slots the allocation into the
// ISS-P06 write path so the minted key co-commits with `issue.created`. Floor: the `#<seqno>` form is
// display-only (named in `keys`).
pub use keys::{
    render_display_key, CanonicalKey, HiLoKeyAllocator, InMemoryPrefixCounter, PrefixReserve,
    ReserveError, ReservedBlock, INITIAL_BLOCK_SIZE, MAX_BLOCK_SIZE,
};
pub use write_path::create_issue;

// ISS-P09 (P-375, M4): the server-arbitrated order_key CAS reorder (the silent-clobber floor).
// `reorder` bisects a new rank via the frozen byte-identical `OrderKey::rank_between` (contract 13.3,
// executed) and writes it under a server-arbitrated CAS on the moved issue's last-seen version: a
// stale version LOSES and re-bases against the authoritative order (0 silent clobber); a win
// co-commits the `issue.reordered` event (contract 2.2 consumed) on the SAME outbox transaction
// (emit-iff-committed). `BoardRanking` is the CAS-guarded `(order_key, version)` ranking store;
// `rebalance` re-spaces the keys at the 48-char trigger WITHOUT reordering the displayed order.
// FLOOR named: ranking = order_key + server-arbitrated CAS; the move-CRDT (Yrs list / Fugue) reusing
// the byte-identical order_key is the measured M5 follow-on (ISS-P32) — the promotion swaps the
// conflict engine, not the data model (`reorder` crate doc).
pub use reorder::{
    cmp_ranked, rebalance, reorder, same_displayed_sequence, BoardRanking, RankedIssue,
    ReorderError, ReorderOutcome, ReorderRequest,
};

// ISS-P10 (P-376, M4): the issue body + comments as a `myelin-content` block subtree (the consumed
// Issues SUBSET, X-2) under a single-author version-token CAS, round-tripping `render(parse(md)) ===
// md` through the ONE WASM render path (the SAME parse/serialize the editor compiles to wasm32 — no
// second renderer). `validate_subtree`/`is_issue_block` enforce the Issues subset (reject the
// Knowledge-only db_view/sync_block/toggle LOUDLY); `IssueContent::cas_edit` is the single-author CAS
// (stale write loses); `emit_content_event` co-commits the body/comment event through the ONE
// `OutboxTx::emit` (contract 2.2). `roundtrips_md`/`paragraph_body` are the editor-entry helpers the
// ISS-D10 corpus gate feeds. FLOOR named: the move-CRDT body collaboration is out of v1 scope
// (single-author version-CAS is the floor); the at-rest per-subject-DEK body ciphertext is the
// storage layer's (`dek`); the structured-node → refs.edge.created emission is the Refs-producer band.
pub use content::{
    emit_content_event, is_issue_block, paragraph_body, roundtrips_md, validate_subtree,
    CasConflict, ContentError, ContentKind, IssueContent, SubsetError, ISSUES_EXCLUDED_BLOCKS,
};

// ISS-P11 (P-377, M4): governance schemes + the precedence algebra + the flexible-field model.
// `SchemeResolver`/`resolve` is the deterministic, cached, off-the-hot-path most-specific-wins
// precedence algebra over the fixed eight-row (type × project × team) lattice; `load_resolved` is the
// write-path seam (loads the already-resolved compiled scheme, never resolves inline). `Reassignment`
// proves a scheme reassignment is a CONFIG write (0 issue rows touched). `FlexibleField`/
// `add_flexible_field` is the zero-DDL JSONB property-bag custom-field model (default GIN over
// issue_props_gin). FLOORS named in `schemes`: the hierarchy is a tree parent (DAG portfolios M5+);
// the projection-feeder generated-index promotion is ISS-P15. The FSM interpreter is ISS-P12.
pub use schemes::{
    add_flexible_field, org_default_scheme_id, resolve, specificity_rank, FlexibleField,
    FlexibleFieldWrite, IndexPosture, Reassignment, ResolveContext, ResolveKey, Scheme,
    SchemeAssignment, SchemeKind, SchemeResolver, TypeDef, TypeSchemeBody,
};

// ISS-P12 (P-378, M4): the data-driven workflow FSM interpreter + the frozen-QueryAst guards.
// `Workflow::plan_transition` is the pure governance decision (the FIXED state-category invariant +
// the bounded QueryAst guards + required-fields + the staged post-actions) the ISS-P06 write path
// drives; a blocked transition returns a pre-assembled reason (the ISS-D12 guard half — `blocked_by`
// an open issue → blocked + reason via `blocked_by_guard`). Guards are `myelin_query::QueryAst`, the
// arm-trigger post-action carries `myelin_query::EventMatcher` (= QueryAst, 3.4); the arm-trigger
// workflow body (`arm_trigger_body`) reads time through WfCtx (flow-determinism, 1.6). FLOOR: the
// CI-red guard half (X-1 CheckStatus + trust_tier) lands in ISS-P27 (`linked_pr_ci_green_guard` shape).
pub use workflow::{
    arm_trigger_body, blocked_by_guard, example_arm_trigger, linked_pr_ci_green_guard,
    ArmedTrigger, GuardVar, IssueContext, PostAction, StateCategory, TransitionBlocked,
    TransitionPlan, Workflow, WorkflowError, WorkflowGuard, WorkflowState, WorkflowTransition,
};

// ISS-P13 (P-379, M4): the AST→OLTP-store compiler — the `list_objects` `SetExpr` push-down lowered
// FIRST into a leak-free SQL predicate / JOIN over `issue.id` (contract 4.3, the highest-stakes
// leak-free property; 4.10 the zookie new-enemy guard). Issues is the headline consumer of 4.3.
pub use planner::{
    compose_board_query, issue_id_colref, lower_over_issue_id, AuthzJoin, AuthzVisibleIndex,
    BoundParam, ComposedBoardQuery, FilterMode, LoweredFilter, AUTHZ_VISIBLE_TABLE,
    ISSUE_VIEW_PERMISSION,
};

// ISS-P14 (P-380, M4): the cost-bounding + three-tier escalation ON TOP of the ISS-P13 leak-free
// pre-filter — classify each predicate (typed-core / generated facet / GIN probe / Search), bound
// the cost, and escalate to Search (the SAME `Filter` conjoined, 6.1) or return Refine — NEVER an
// unbounded JSONB scan. Every query paginated + statement-timeout'd (the ISS-D2 `<1s` latency gate).
pub use cost_bounder::{
    classify_field, estimate_cost, lower_acl, plan_board_query, BoundedBoardQuery,
    CostBounderFloors, CostBudget, FacetCatalog, PlanOutcome, RefineHint, SearchEscalation, Tier,
    TIER3_FIELDS, TYPED_CORE_FIELDS,
};

// ISS-P16 (P-382, M4): the co-equal `ViewSpec` views over the ONE issue table + the design-system pass.
// `IssueView` is the seven canonical views (board/roadmap/backlog/list/table/calendar/cycle), each a
// frozen `myelin_query::ViewSpec` projection over the one `issue` table (13.3, co-owned); `IssueView::plan`
// is the executor seam that ALWAYS conjoins the leak-free ACL `Filter` through plan_board_query (4.3 —
// ISS-P13/P14; a confidential issue is ABSENT, no "N hidden" leak). The board↔roadmap co-equality is
// STRUCTURAL: the denormalised `type_rank` (board ≤ 1, roadmap ≥ 2) partitions the SAME rows;
// `RowProjection` + `board_and_roadmap_share_row` + `edit_on_board_reflects_on_roadmap` prove an edit on
// one lens reflects the SAME row id on the other (ISS-D1, 0 drift, asserted by row id). The design-system
// pass (all states) is recorded + signed off in the design folder. FLOORS named: the cross-cell portfolio
// rollup view is the M5 follow-on (ISS-P32, the CrossCellPointer bridge); the real-time board sync is
// ISS-P30 (P-397). Both named in `ViewFloors`.
pub use views::{
    board_and_roadmap_share_row, edit_on_board_reflects_on_roadmap, type_rank_split_is_partition,
    IssueView, RowProjection, ViewFloors, BOARD_TYPE_RANK_MAX, CYCLE_FIELD, ORDER_KEY_FIELD,
    ROADMAP_TYPE_RANK_MIN, STATE_CATEGORY_FIELD, TYPE_RANK_FIELD,
};
