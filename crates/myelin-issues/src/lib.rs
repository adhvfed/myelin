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

//! ## ISS-P31 / P-385 (M4-I8, the band exit) — Erasure-reaches-every-holder + post-restore re-erasure
//! [`holder_erase`] wires the `PersonalDataHolder::erase` BODY (contract 10.1, now FULL — ISS-P05
//! registered the holder, ISS-P07 wired the two LEVERS). [`holder_erase::IssueEraseFanout::erase`] runs
//! the storage §5.2 algorithm over EVERY Issues holder ([`holder_erase::HolderTarget::ALL`] — pseudonym
//! map / per-subject free-text DEK / attachment-blob DEK / OLAP+restriction / Search+embeddings / Refs),
//! returning ONE [`holder_erase::HolderReceipt`] per holder (the ISS-D11 per-holder green artifact). It
//! OWNS the per-subject DEK crypto-shred directly (it holds the ONE `KmsEngine` ISS-P07 sealed through —
//! `destroy_dek` reaches the free-text/change-log/comments/worklog ciphertext live AND in backups);
//! drives Identity's `erase` for the pseudonym-map shred (4.8 — "Former user 8a2f" without rewriting
//! issues others own); sets the [`holder::RestrictionFlag`]; and emits the `issue.*.erased` tombstones
//! (contract 2.7) Search/Refs/OLAP/Notif consume. Every erase records into the PII-free,
//! non-shred-erasable [`holder_erase::IssueErasureLedger`] (10.8) so
//! [`holder_erase::IssueEraseFanout::re_erase_after_restore`] (GD-14) re-destroys any DEK a backup
//! restore resurrected (0 resurrected post-restore — the gate). An incomplete erase is LOUD, never a
//! false-green ([`holder_erase::EraseFanoutError`]). FLOORS named: the third-party free-text residual is
//! the ONE platform posture (10.9 / X-7, by reference, [`holder::ISSUE_RESIDUAL_POSTURE_REF`] —
//! `[OPEN — LEGAL]`, R-1); the OQ-H worklog special-category classification is `[OPEN — LEGAL]` R-2 (the
//! erasure LEVER is structural — the worklog keys per-subject, reached by the DEK shred — only the
//! lawful-basis tag is counsel-ratified).

//! ## ISS-P21 / P-388 (M4-I5, the adoption gate) — the two-pass ID-remapped import engine + ADF map
//! [`import`] ships the **adoption gate** ("leave Atlassian cleanly", VISION §1): the two-pass,
//! ID-remapped, idempotent + resumable import engine ([`import::ImportEngine`]) over a persisted
//! source-to-Myelin id map ([`import::SourceIdMap`] — the load-bearing artifact for idempotency /
//! resume / rollback / round-trip). Four source adapters ([`import::JiraAdapter`]/
//! [`import::LinearAdapter`]/[`import::GitHubAdapter`]/[`import::CsvAdapter`]) normalise an
//! already-parsed provider payload into ONE canonical interchange format ([`import::CanonicalImport`])
//! that round-trips with the portability export (the ISS-D9(a) round-trip oracle). The Jira adapter
//! converts ADF description bodies through the FROZEN [`myelin_content::adf`] lossy-map (13.2 consumed
//! — every lossy/dropped node recorded in the [`myelin_content::ImportReport`], NEVER silent; the
//! permission-scheme mapping is the named lossy/legal-review leg R-9,
//! [`import::UNSUPPORTED_PERMISSION_SCHEME`], M5+). Pass 1 mints a canonical key (REUSING
//! [`keys::HiLoKeyAllocator`] — an imported issue is just an issue), records the id-map entry, and
//! emits `issue.created` via the ONE [`myelin_events::OutboxTx::emit`] (2.2 consumed — one indexing
//! path; reindex-from-source works on imported data for free); an already-mapped source id is SKIPPED
//! (0 duplicate creates on a crash, ISS-D9(b)). Pass 2 resolves relation endpoints through the id-map
//! and emits `issue.relation.created` (an unmapped endpoint is a NAMED [`import::Unresolved`] gap,
//! never a silent dangling edge). [`import::ImportEngine::dry_run`] builds the FULL
//! [`import::ReconciliationReport`] WITHOUT emitting (reconciliation-report-first); the per-tenant
//! in-flight cap ([`import::ImportLaneBudget`], 1.11 consumed — the import is the BATCH lane, shed
//! before the human lane) bounds a 100k-issue import to capped batches that never starve another
//! tenant. FLOORS named: the byte-level provider wire parsers are upstream of the adapters; the id-map
//! BACKEND is the OLTP store in prod ([`import::InMemorySourceIdMap`] is the DB-free drill model — the
//! live PgStore integration is the named follow-on); the permission-scheme mapping is the R-9 legal leg.

#![forbid(unsafe_code)]

pub mod agent_spend;
pub mod app;
pub mod board_sync;
pub mod ci_guard;
pub mod content;
pub mod cost_bounder;
pub mod cross_cell_rollup;
pub mod declares;
pub mod dek;
pub mod events;
pub mod floor_triggers;
pub mod governance;
pub mod holder;
pub mod holder_erase;
pub mod holder_intent;
pub mod import;
pub mod keys;
pub mod migrations;
pub mod move_crdt;
pub mod my_work;
pub mod olap_feed;
pub mod planner;
pub mod projection_feeder;
pub mod pseudonym;
pub mod query_coown;
pub mod rebac_fragment;
pub mod reflexes;
pub mod refs_glue;
pub mod reorder;
pub mod replay;
pub mod rollup;
pub mod schema;
pub mod schemes;
pub mod sla_calendar;
pub mod sla_escalation;
pub mod time_axis;
pub mod trigger;
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

// ISS-P32 (P-495, M5): the MEASURED move-CRDT promotion of the CAS reorder floor (R-3, arch §5
// "Floor → follow-on"). `MoveCrdtBoard` is the convergent Yrs-list engine over the BYTE-IDENTICAL
// order_key (the data model is unchanged across the engine swap — `derived_order_keys` recomputes the
// frozen codec from the convergent list order); `ReorderPressure` is the MEASURED concurrent-reorder
// trigger that promotes a board off the CAS floor only on a measured re-base rate (VISION §3 — the
// floor stands until the signal fires). The move-CRDT re-greens ISS-D5 across the engine-promote
// boundary (0 clobber holds, now stronger — two concurrent distinct-issue moves both survive). Reuses
// the cited `yrs` structure (VISION §4 — the SAME crate Knowledge's yrs_engine uses), never a second
// frame.
pub use move_crdt::{MoveCrdtBoard, MoveCrdtError, MoveCrdtFloors, ReorderPressure};

// ISS-P32 (P-495, M5): the cross-cell portfolio rollup over the PII-free CrossCellPointer bridge
// (R-7 / OQ-I, arch §7 "Floor → follow-on": single-cell rollup → cross-cell). `CrossCellPortfolioRollup`
// fans a remote portfolio child out as a PII-free pointer homed in the child's cell; resolution is
// ALWAYS cell-local (`resolve_cell_local` → only the `RollupAggregate` numbers cross back, never a leaf
// row). `CrossCellDsrFanout` is the DSR fan-out iterating member_cells (GA-D1 / CP-D7 / CP-D8 — 0 cell
// missed, per-cell receipt, 0 PII crosses). Consumes the frozen contract 12.6/10.4, never a second
// bridge frame (EI-01 §7 — the Issues twin of Knowledge's collab::CrossCellCollab).
pub use cross_cell_rollup::{
    CellLocalRollupResolver, CrossCellDsrFanout, CrossCellPortfolioRollup, CrossCellRollupFloors,
    CrossCellRollupPointer, DsrCellReceipt, PortfolioProjection,
};

// ISS-P32 (P-495, M5): the MEASURED promotion triggers for the four remaining floor follow-ons that
// ship the measurement seam, NOT the migration (VISION §3 — promote only on a measured signal):
// materialised-rollup-on-measured-large (R-4), distributed-SQL-on-shard-outgrows-PG (R-6),
// Monte-Carlo-on-measured-variance (R-5), column-store-on-measured-volume (EI-04 §5). `Iss32FloorRegister`
// is the executable floor register naming every follow-on WITH its trigger + the post-M5 R-10.
pub use floor_triggers::{
    ColumnStoreTrigger, DistributedSqlTrigger, Iss32FloorRegister, MaterialisedRollupTrigger,
    MonteCarloForecastTrigger,
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

// ISS-P27 (P-394, M4): the CI-red governed-transition guard — closing the X-1 consumer. At transition
// time Issues reads the linked PR's CURRENT `CheckStatus{state, trust_tier}` OFF THE FACT (via
// `project(PR_ref)`, contract 5.9 / 5.6), binds it into the guard context, and runs
// `plan_transition` — "can't mark Done while CI red on the linked PR" BLOCKS with a reason; an
// un-endorsed `untrusted_fork` success is NEUTRAL (the poisoned-Done defence). Issues NEVER recomputes
// trust (the SAME posture Git's merge gate applies — one trust rule, EI-01 §7). The agent path is
// HITL-gated (a permitted transition is WITHHELD for approval — 0 pre-approval mutation, AG-8). The
// guard RESTS ON THE PROVEN X-1 SEAM (GIT-D10/CI-D8 GREEN), not a doc claim; no floor new.
pub use ci_guard::{
    bind_linked_pr_ctx, ci_done_guard, plan_agent_ci_gated_transition, plan_ci_gated_transition,
    AgentTransitionOutcome, LinkedPrCheck, CHECK_STATE_NEUTRAL, CHECK_STATE_SUCCESS,
    TRUST_TIER_TRUSTED, TRUST_TIER_UNTRUSTED_FORK,
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

// ISS-P30 (P-397, M4): the real-time board sync over the firehose resume-cursor protocol (the
// ISS-D13 zero-ops-lost-on-reconnect gate). `board_sync` is the Issues-layer CONSUMER of the ONE
// frozen Bus-owned firehose protocol (`myelin_events::Firehose` — subscribe/resume/resync_required,
// contract 3.5) + the substrate's paginated bounded scope (`ScopeWindow`, never `*`): optimistic
// local updates + bus-driven cache invalidation; a reconnect backfills (last_seq, now] then live
// (0 ops lost); a past-window cursor → resync_required → a full *.snapshot replay (NAMED). The sync
// floor (R-8 offline/local-first) is the named follow-on; the real connection tier is P-403.
pub use board_sync::{
    board_stream, BoardCache, BoardCard, BoardOp, BoardSync, BoardSyncFloors, LocalMutationError,
    BOARD_FIREHOSE_STREAM_PREFIX,
};

// ISS-P29 (P-396, M4): the governance admin views S13–S18 (the M4-I7 admin-views slice). The
// `governance` module ships the backend view-models for the six governance screens (each naming the
// REAL engine it writes through — never a parallel calc, EI-01 §7), the S15 permission inspector
// (the CONSUMER of contract 4.4 — reads `list_subjects`/`explain` through the `PermissionResolver`
// port, 0 private recompute; the inspector's answer EQUALS Identity's `explain`), the S14 breach-
// simulation (the REAL ISS-P26 `business_fire_at`, not a parallel calc), and the S13 unreachable-
// state inline validation (over the REAL ISS-P12 `Workflow` FSM). The design sketches (S13/S14/S15/
// S16/S18, incl. the empty/loading/error/permission states) are signed off in the design folder
// (governance-admin-pass.md + governance-signoff.md). FLOOR: none new — the inspector reads 4.4, the
// breach-sim reuses the SLA engine, the guard/condition builders reuse the frozen QueryAst.
pub use governance::{
    simulate_breach, workflow_unreachable_states, BreachSimulation, GovernanceFloors,
    GovernanceView, GovernanceViewModel, GuardLanguage, InspectorAnswer, PermissionInspector,
    PermissionResolver,
};

// ISS-P17 (P-383, M4): the Refs wiring (resolve/project/#sub/edges/traverse/TE-7 mirror) + the
// issue.* Search projection emitter. `project(ref, viewer)` (5.6, OWNED) is the only cross-DB read of
// an Issues artifact — permission FIRST, a confidential issue returns a tombstone carrying the root,
// NEVER the title (the ISS-D3 slice re-asserted at the unfurl boundary). The `#sub` mints
// (comment-/b/field-/row-, 5.7) go through the ONE Refs codec; `emit_content_edges` (5.4) emits one
// refs.edge.created per mention/artifact_ref/embed node; `emit_relation_edge` (5.5) is the TE-7
// typed-edge mirror (the issue_relation table is truth); `IssueRelationGraph::traverse` (5.3) is the
// bounded cycle-safe depth-16 walk; `IssueProjectFetcher` (6.3) is the LIVE issue.* Search projection
// emitter (reindex 6.4 rides the ONE replay-from-source path). FLOOR: the cross-cell projection bridge
// is the M5 follow-on (ISS-P32).
// ISS-P18 (P-384, M4): the event-driven, debounced, incremental rollup consumer (off the bus, NEVER
// in the write path — ADR-11.5). `RollupConsumer` is the `EventHandler` (contract 2.4 consumed) that
// watches the rollup-driving issue.* deltas, walks the affected ancestors (`walk_parent_edges` —
// contract 5.3 consumed, the depth-16 cycle-safe walk reusing the ONE IssueRelationGraph forward-parent
// traverse), debounce-coalesces a burst into ONE recompute per ancestor (DebounceCoalescer, OQ-K),
// re-sums incrementally (`recompute_incremental`), and SUPPRESSES the input_hash no-op (the loop-storm
// guard, AG-6 — an unchanged recompute emits NO event). `flush` drains the coalesced burst → the owed
// issue.rollup.recomputed drafts (`rollup_recomputed_draft`); `reindex_from` rebuilds the derived rollup
// DRIFT-FREE off the source of truth (issue_relation edges + leaf facts — contract 2.6 consumed, the
// ONLY recovery path; steady-state + recovery share one code path). The rollup row is DERIVED (no
// migration table; rebuildable). FLOORS named in `rollup` (RollupFloors): read-time rollup →
// materialise-on-measured-large (KN-3, ISS-P32 / P-495); the debounce-window per-tenant tunable (OQ-K);
// cross-cell ancestors (OQ-I); the forecast agent off issue.rollup.recomputed (ADR-08).
pub use rollup::{
    aggregate_snapshot, recompute_incremental, rollup_recomputed_draft, walk_parent_edges,
    DebounceCoalescer, DebounceWindow, LeafFact, RecomputeOutcome, RollupAggregate, RollupConsumer,
    RollupFloors, RollupStore,
};

// ISS-P31 (P-385, M4-I8): Erasure-reaches-every-holder + post-restore re-erasure (the band-exit slice).
// `IssueEraseFanout::erase` runs the storage §5.2 algorithm over every Issues holder (`HolderTarget::ALL`),
// returning a `HolderReceipt` per holder (the ISS-D11 per-holder artifact) + the `issue.*.erased`
// tombstone count; it crypto-shreds the real per-subject DEK over the ONE KmsEngine ISS-P07 sealed through
// (free-text/change-log/comments/worklog + the attachment blob), drives Identity's `erase` for the
// pseudonym-map shred (4.8), sets the RestrictionFlag, and records into the PII-free `IssueErasureLedger`
// (10.8). `re_erase_after_restore` (GD-14) re-destroys any DEK a backup restore resurrected (0
// resurrected post-restore). An incomplete erase is LOUD (`EraseFanoutError`), never a false-green.
// FLOORS named: the third-party free-text residual is the ONE posture 10.9/X-7 by reference (R-1); the
// OQ-H worklog special-category basis is [OPEN — LEGAL] R-2 (the erasure lever is structural + ships now).
pub use holder_erase::{
    store_classes_reached_by_free_text_shred, EraseFanoutError, HolderReceipt, HolderTarget,
    IssueEraseFanout, IssueEraseOutcome, IssueErasedSubject, IssueErasureLedger,
    IssueReErasureReceipt, ERASED_TOMBSTONE_TOKENS,
};

// ISS-P20 (P-387, M4): the OLAP read store (CQRS, reindex-from-source only, restriction-flag-honouring).
// `IssueOlapConsumer` is the Issues-side bus `EventHandler` (contract 2.4 + 11.6 + 2.6 consumed) that
// feeds the SHARED `myelin_storage::olap::OlapReadStore` (REUSED, never a parallel store — EI-01 §7)
// off the analytics-driving issue.*/sla.*/cycle.* stream — NEVER the OLTP issue table (the 0-OLTP-read
// gate, `oltp_read_count == 0` by construction). It keeps the OLAP store's C5 restriction set in sync
// with the shared holder `RestrictionFlag` so a restricted subject contributes 0 rows to analytics
// (recon §8 / contract 11.6). `IssueOlapAnalytics` reuses Storage's frozen CFD/cycle-time/velocity
// aggregates (a restricted subject excluded at query time) + ADDS the Issues ask's SLA-compliance leg
// with the SAME restriction filter; `leak_audit` (`restricted_subject_leak == 0`) is the
// restriction-exclusion gate. `reindex_from` rebuilds the DERIVED feed DRIFT-FREE off Issues' source of
// truth (`IssueReindexSource` → *.snapshot → the SAME handle body — steady-state + recovery share one
// code path, contract 2.6); the cold rebuild byte-matches the live projection (`parity_bytes`, the
// ISS-D8b OLAP-feed reindex-parity). FLOORS named in `olap_feed` (IssueOlapFeedFloors): the Monte-Carlo
// forecast that reads OLAP throughput is ISS-P32 (linear floor → Monte-Carlo, ADR-08); the ClickHouse
// columnar backend is behind OlapReadStore (Storage P-ST-18, wired); the OQ-H worklog eligibility seam
// is Storage's AnalyticsEligibility ([OPEN — LEGAL]).
pub use olap_feed::{
    issue_analytics_aggregate_names, IssueOlapAnalytics, IssueOlapConsumer, IssueOlapFeedFloors,
    IssueOlapFeedSignal, IssueRestrictionLeakAudit, ReindexCtx, ISSUE_ANALYTICS_OLAP,
};

// ISS-P21 (P-388, M4-I5): the two-pass ID-remapped import engine + the ADF lossy-map (the adoption
// gate). `ImportEngine` mints canonical keys (REUSING HiLoKeyAllocator) over the persisted `SourceIdMap`
// (idempotency/resume/rollback/round-trip), runs pass 1 (mint+map+emit issue.created, SKIP an
// already-mapped source id = 0 dup on crash) + pass 2 (resolve relations through the id-map + emit
// issue.relation.created, an unmapped endpoint = a named Unresolved gap), and `dry_run` builds the full
// `ReconciliationReport` WITHOUT emitting. The four `SourceAdapter`s (Jira/Linear/GitHub/CSV) normalise
// an already-parsed provider payload into the canonical `CanonicalImport`; the Jira adapter converts ADF
// bodies through the FROZEN `myelin_content::adf` map (13.2 consumed — every lossy node recorded, never
// silent). `ImportLaneBudget` is the per-tenant in-flight cap (1.11 — the import is bounded backfill).
// FLOORS named: the wire parsers are upstream; the id-map backend is the live PgStore (the in-memory
// model is the drill); the permission-scheme mapping is the R-9 legal leg (`UNSUPPORTED_PERMISSION_SCHEME`).
pub use import::{
    adapter_for, AdfBodyNode, CanonicalImport, CanonicalIssue, CanonicalRelation, CsvAdapter,
    DryRun, GitHubAdapter, IdMapEntry, ImportEngine, ImportError, ImportLaneBudget,
    InMemorySourceIdMap, JiraAdapter, LinearAdapter, ProviderRecord, ReconciliationReport,
    SourceAdapter, SourceIdMap, SourceSystem, Unresolved, UNSUPPORTED_PERMISSION_SCHEME,
};

// ISS-P22 (P-389, M4-I5): "My Work" over the ONE Notif inbox + the humanise templates (one read-state
// truth). `my_work_filter`/`list_my_work` read the ONE inbox through the frozen
// `InboxFilter::issues_my_work` (contract 7.1 — a FILTER, never a second store); a mark/snooze on a My
// Work item reflects in the unified inbox (contract 7.2 — one read-state truth).
// `register_issue_humanise_templates` registers the Issues SLA-at-risk / unblocked /
// approval-requested strings into the ONE `myelin_notif::TemplateStore` (contract 7.3 — the OQ-L ONE
// templating surface, 0 second template engine); `wire_issues_my_work` ties the reason set (7.6, now
// wired) AND the templates onto the ONE Notif surfaces in one call. FLOOR named: the SLA-breach
// escalation ENGINE that FIRES the at-risk string is ISS-P26 (the SLA engine on the myelin-flow wheel);
// the at-risk STRING registers here.
pub use my_work::{
    issue_humanise_templates, list_my_work, list_my_work_default, my_work_filter,
    register_issue_humanise_templates, wire_issues_my_work, ISSUE_HUMANISE_TEMPLATES,
    TPL_APPROVAL_REQUESTED, TPL_SLA_AT_RISK, TPL_UNBLOCKED,
};

// ISS-P28 (P-395, M4-I7): the cross-subsystem reflexes (git/chat/identity/ci consumers). The
// `ReflexConsumer` is the bus `EventHandler` (contract 2.4 consumed) over the `*`-free foreign-subject
// whitelist (`REFLEX_SUBJECTS` — git.branch.created/pr.opened/pr.merged, ci.check.updated,
// chat.message.created, identity.member.added/deactivated/erased); each reflex is a PURE planner that
// reduces an incoming foreign event to a typed `ReflexEffect` the ISS-P06 write path drives (plan,
// don't mutate — the ci_guard pattern). A `git.branch.created` links + auto-advances to In Progress; a
// `git.pr.opened`/`merged` mints the `closes` edge and (on merge) auto-closes to Done IFF the CI-red
// Done guard permits (5.9 consumed, read off the fact — never recompute trust); a `chat.message.created`
// "create issue" mints an issue + a `relates` edge (5.4 consumed); an `identity.member.*` anonymises/
// reassigns the actor across history (§7 erasure lever, 4.8). Each is idempotent on `event_id` (the
// within-handler dedup on the runtime's `consumer_dedup` ledger) → 0 duplicate on replay (the gate).
// Every auto-transition runs through the FSM interpreter (`Workflow::plan_transition`) — NEVER a
// governance bypass (the 0-bypass gate; a CI-red merge surfaces a loud `TransitionBlocked`, never a
// Done). FLOOR named: none new — auto-transitions are workflow-permitting only (noted in `reflexes`).
pub use reflexes::{
    linked_pr_from_payload, plan_branch_created, plan_chat_message_created, plan_check_updated,
    plan_member_event, plan_pr_merged, plan_pr_opened, plan_reflex, reflex_subjects,
    ReflexConsumer, ReflexEffect, AUTO_STATE_DONE, AUTO_STATE_IN_PROGRESS, CHAT_MESSAGE_CREATED,
    CI_CHECK_UPDATED, GIT_BRANCH_CREATED, GIT_PR_MERGED, GIT_PR_OPENED, IDENTITY_MEMBER_ADDED,
    IDENTITY_MEMBER_DEACTIVATED, IDENTITY_MEMBER_ERASED, REFLEX_SUBJECTS,
};

pub use refs_glue::{
    block_sub_ref, comment_sub_ref, edge_aggregate_key, emit_content_edges, emit_relation_edge,
    field_sub_ref, issue_root_ref, row_sub_ref, IssueLifecycleRel, IssueMeta, IssueProjectFetcher,
    IssueProjectionStore, IssueRelationGraph, LadderRung, ProjectError, Projected, Projection,
    Projector, RelationEdge, SubAnchor, SubState, Tombstone, TombstoneReason, TraversedNode,
    REFS_EDGE_CREATED, REL_CLASS_LIFECYCLE, REL_CLASS_REFERENCE, TRAVERSE_MAX_DEPTH,
};

pub use agent_spend::{
    per_effect_idem_key, spend_bearing_run, BalancedRunSignal, DispatchedRun, IssueRunKind,
    IssueSpendGate, SpendError,
};

// ISS-P25 (P-392, M4-I6): the stateful Trigger flagship ("Remind me when unblocked") — exactly-once
// across a restart, stale-once. `IssueTriggerEngine` is the Issues-side stateful Trigger over the
// CONSUMED bus arm_trigger/disarm_trigger primitive (3.3/3.4, the frozen `myelin_query::TriggerEngine`
// fire-once-per-arming), the CONSUMED `myelin-flow` `stale_after` durable timer (9.3, the
// `myelin_query::DurableTimer` wheel seam), and the ONE Notif inbox for on_resolve (7.1) — never a
// second engine/timer/store (EI-01 §7). It adds the Issues-owned semantics: the armable-condition
// catalogue (`ArmableCondition` — each a frozen EventMatcher = QueryAst over issue.* events +
// issue_relation projection state; the flagship `RemindWhenUnblocked` is `blocked_by_unresolved == 0`),
// the ONE inbox item per resolve, the stale nudge that fires ONCE after `stale_after` (default 30d, a
// per-tenant tunable — `DEFAULT_STALE_AFTER_DAYS`), and the snapshot/restore durability across a
// restart (`TriggerSnapshot` = the durable `trigger` row, §3.6). ISS-D7 (1-fire + stale-once across a
// restart) is the green artifact. FLOOR named: none new beyond the 30d default note.
pub use trigger::{
    default_stale_after, ArmRequest, ArmableCondition, IssueTriggerEngine, TriggerInboxItem,
    TriggerInboxKind, TriggerSnapshot, DEFAULT_STALE_AFTER_DAYS, VAR_ASSIGNEE,
    VAR_BLOCKED_BY_UNRESOLVED, VAR_STATE_CATEGORY,
};
