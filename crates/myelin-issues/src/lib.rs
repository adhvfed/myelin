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

#![forbid(unsafe_code)]

pub mod holder_intent;
pub mod rebac_fragment;
pub mod schema;
