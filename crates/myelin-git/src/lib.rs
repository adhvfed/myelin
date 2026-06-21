//! # `myelin-git` — the Git-hosting subsystem (the M1 freeze-so-dependents-compile slice)
//!
//! This crate carries the Git subsystem's **M1 contract freezes** — the relation/event/holder
//! SHAPES dependents compile against, ahead of the M3 feature bulk:
//! - [`rebac_fragment`] — **GIT-P1 / P-123**: the frozen Git ReBAC namespace fragment (contract
//!   4.9) Identity compiles into the one cell schema (ref-glob + CODEOWNERS-as-relations +
//!   `approve_untrusted_ci` + per-watchable `watcher`). Names freeze here; the permission rewrites
//!   are wired LIVE in GIT-P13 (M3-G2) / P-ID-24.
//! - [`events`] — **GIT-P2 / P-124**: the complete `git.*` event-token registration (contract 2.9)
//!   — git COMPLETES its dotted-name list, each token validated against the one Bus grammar. The
//!   tokens are registered here but EMITTED only from the outbox in GIT-P8 (`git.ref.updated`) /
//!   GIT-P16 (`git.pr.*` / `git.review.*` / `git.comment.*`).
//! - [`holder_intent`] + [`schema`] — **GIT-P3 / P-063**: the H1 holder INTENT + the
//!   `#[personal_data(...)]` classification tags (see below).
//! - [`subs`] — **GIT-P4 / P-230**: git's `#sub` mints registered with Refs (contract 5.7) — the
//!   `comment-` / `thread-` / `L<a>-L<b>` kinds git owns are DECLARED to Refs ([`subs::register_git_sub_kinds`])
//!   and minted as grammatical sub-URNs through the one Refs codec ([`subs::mint_pr_comment`] etc.).
//!   The kind REGISTRATION + the grammatical mints ship; the per-kind resolvers are named follow-ons
//!   (GIT-P18 comment/thread, GIT-P24 the L-range 4-state content-anchored resolver).
//! - [`commit`] — **GIT-P25 / P-ID-25**: Git **pseudonymous-by-default commits** consuming the 4.8
//!   grammar. A commit's author/committer line is the per-tenant pseudonym
//!   `<pseudonym>@<tenant>.noreply` ([`myelin_identity::PseudonymHandle`]) baked into the IMMUTABLE
//!   commit bytes — never the erasable real identity. After `erase(subject)` shreds the pseudonym
//!   map (DSR step 1, X-7) the bytes carry 0 recoverable real identity (the GIT-D2 residual ==
//!   the one platform posture); the opaque `principal_id` still attributes the commit for authz
//!   out-of-band. Floor: the audited history-rewrite path (a *body* expunge) is the M5/on-demand
//!   follow-on (00-recon §X-7 / CR §9 10.6, Git+GDPR roadmaps).
//! - [`notif_rules`] — **GIT-P19 / P-263 (M3)**: producer accretion — Git **registers** its
//!   `define_notif_rule` set (review_requested / mentioned / watched, contract 7.6) into the frozen
//!   `myelin_notif::NotifRuleRegistry` and **wires** its REAL PR/repo watcher reverse index behind the
//!   frozen `myelin_notif::WatcherResolvePort` (contract 4.3/4.10), over the `watcher` relation its
//!   [`rebac_fragment`] declares (4.9). Git registers/produces; Notif owns the seams (ZERO Notif code
//!   change — the inverse-signal property, EI-01 §1). The Knowledge half is NOTIF-P20; Issues/Chat/CI
//!   are M4 (NOTIF-P21/P22/P23); cross-cell is single-home (NOTIF-P24).
//! - [`check_status`] — **GIT-P6 / P-232**: the X-1 Git↔CI `CheckStatus` **consumer contract**
//!   (contract 5.9) — the `check_status` projection-table schema keyed `(commit_oid, context)`, the
//!   monotonic `run_attempt` supersession rule, and the `required`-set policy shape, declared as a
//!   COMPILING (not-yet-live) seam module against the M2-frozen 5.9 shape. The live consumer + merge
//!   gate land in GIT-P20 (against a synthetic `ci.check.updated` emitter — the seam-floor); the real
//!   CI producer wiring is the M4 co-gate (GIT-D10 / CI-D8 end-to-end). No event consumer is wired
//!   and no migration is run here.
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

pub mod check_status;
pub mod commit;
pub mod events;
pub mod holder_intent;
pub mod notif_rules;
pub mod rebac_fragment;
pub mod replay;
pub mod schema;
pub mod search_projection;
pub mod subs;
