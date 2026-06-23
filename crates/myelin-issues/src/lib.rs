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
//! crypto-shred naming ISS-P07/P31, the `restrict` flag wired — contract 10.1/1.4). **NO Issues data
//! is written yet**: the silent-data-loss-safe write path is ISS-P06 (P-372); the per-subject DEK +
//! the full holder ops are ISS-P07 (P-373). Storage floor = PG-hybrid sharded by tenant;
//! distributed-SQL is the measured R-6 follow-on (ISS-P32) — named in [`migrations`].

#![forbid(unsafe_code)]

pub mod app;
pub mod declares;
pub mod events;
pub mod holder;
pub mod holder_intent;
pub mod migrations;
pub mod query_coown;
pub mod rebac_fragment;
pub mod replay;
pub mod schema;
pub mod sla_escalation;

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
