//! # `myelin-git` — the Git-hosting subsystem (GIT-P3 / P-063 floor: the H1 holder INTENT
//! + the `#[personal_data(...)]` classification tags on the skeletal git schema)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/git-hosting/architecture/00-overview.md` §1.1 (git OWNS
//! its erasure obligations as `PersonalDataHolder` **holder H1** — "the hardest in the platform"),
//! `03-events-contracts-and-glue.md` §6 (the H1 `locate/export/rectify/restrict/erase` holder +
//! the erasure algorithm), and `01-tech-and-data-model.md` §4.3/§4.5 (the schema types the tags
//! apply to — `author_pseudonym` / `reviewer_pseudonym` / `pusher_pseudonym` + the free-text body
//! fields, and the personal-data inventory that drives the holder).
//!
//! **Contract-index rows (consumed here — implemented to their frozen `myelin-gdpr` shapes):**
//! - **10.1** `PersonalDataHolder{locate, export, rectify, restrict, erase}` — git declares the
//!   **H1 INTENT** here (the holder is OPENED + auto-registered by `serve` when the store opens in
//!   GIT-P8). The trait BODY (the real locate/erase fan-out over git+metadata, the §6.1 erasure
//!   algorithm) is the GIT-P8/GIT-P9 floor, not built here.
//! - **10.2** the `#[personal_data(category, role, basis, retention, erasure, subject_locator)]`
//!   classify-derive — APPLIED here to every PII-carrying field of the (still-skeletal) git schema
//!   types so the `no-untagged-personal-data` lint (contract 1.6) is **green from the first
//!   migration** (GIT-P8). The macro is a NO-OP at its M0 floor (P-050); applying it freezes the
//!   classification so the lint admits the schema + the M1 stores compile against the tags.
//!
//! ## What this prompt (GIT-P3 / P-063) ships — and what it deliberately does NOT
//! **Ships:** the [`holder_intent`] declaration (git = holder H1, the §4.5 personal-data inventory
//! encoded as data) + the [`schema`] module — the skeletal git OLTP row types (`PullRequest`,
//! `Review`, `ReviewComment`, `Reflog`) carrying the `#[personal_data(...)]` tags on their
//! pseudonym + free-text-body fields. The goal is the GATE: the `no-untagged-personal-data` lint is
//! green on the git skeleton (0 untagged PII fields), with a red-fixture witness proving the lint
//! still REJECTS a deliberately-untagged git PII field.
//!
//! **Does NOT ship (floors named — VISION §3 name-your-floors):**
//! - **No git FEATURE.** No receive-pack, no ref-CAS, no PR/review logic, no migrations. The schema
//!   types here are skeletal row-shape carriers for the tags, not the live tables.
//! - **The holder is NOT opened/registered here.** It is declared as INTENT (data). The holder is
//!   actually **OPENED and auto-registered by `serve`** when the git store opens in **GIT-P8 (M3-G1
//!   — the GitCore layered seam)**; the `PersonalDataHolder` trait BODY (the real §6.1 erasure
//!   algorithm: pseudonym-map shred + per-subject DEK crypto-shred + Search purge + Refs tombstone)
//!   lands across **GIT-P8 / GIT-P9** and the GDPR producer-holder wiring **P-GA-27 (M3)**.
//! - **The classify-derive macro BODY** (parsing the tags into the data-map/RoPA registry) is the
//!   GDPR floor **P-GA-07 (M1)**; here the derive is the no-op floor (P-050) and the tags are the
//!   classification facts a store applies today.
//!
//! ## Personal-data handling posture (architecture §4.3 / §4.5)
//! References-not-payloads + **pseudonym indirection**: author/committer/pusher/reviewer identity is
//! an **opaque pseudonym** resolved through Identity, never a raw name/email (GIT-1, contract 4.8) —
//! so its `erasure = Pseudonymise` (delete the Identity map, contract 4.8 — the lever that makes
//! erasure usually free). Free-text **bodies/titles are encrypted under a per-subject DEK**
//! (contract 11.4) — so their `erasure = CryptoShred(subject_dek)`, reaching live + backups by
//! construction. Both are `role = TenantContent` (processor posture: the customer org is the
//! controller of repo content; a DSR is answered by/for the tenant, Art. 28).

#![forbid(unsafe_code)]

pub mod holder_intent;
pub mod schema;
