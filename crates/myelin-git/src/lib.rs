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
//!   - **GIT-P12 / P-273 (M3)** adds the receive-pack ENFORCEMENT half of the same data-model gate
//!     ([`commit::enforce_pseudonymous_commit`] + [`receive_pack::PushPolicy`]'s pseudonymity rule):
//!     a pushed commit whose author/committer identity is NOT the principal's tenant pseudonym
//!     `<pseudonym>@<tenant>.noreply` is REJECTED at receive-pack BEFORE the ref moves, so the
//!     immutable object DB admits **0 cleartext PII** in a commit identity field (the GIT-D2 GIT-1
//!     half). **The chosen enforcement default (OQ-10 / R-8): REJECT-AT-PUSH (client-cooperative,
//!     sha-stable)** — the decided PROPERTY ("immutable bytes carry only the opaque pseudonym",
//!     §X-7) is enforced at the door, not by silently rewriting the client's commit SHAs; the
//!     server-side rewrite-at-push mode is the named GIT-P29 follow-on (the rationale is in
//!     [`commit`]). The residual lawful-basis posture is instantiated BY REFERENCE to the ONE
//!     platform posture (10.9 / recon §X-7) — never restated as a git-local statement. **Floor
//!     (GF-7):** the structural mechanism ships across GIT-P9/GIT-P12/GIT-P29; the lawful-basis
//!     residual is the ONE posture's `[OPEN — LEGAL]` statement (R-7, parallel/Legal — NOT a code
//!     gate); the erase-reaches-every-holder GIT-D2 completion is GIT-P29.
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

/// **PR/review/comment BODIES on the frozen `myelin-content` subset + the content-node →
/// `refs.edge.created` emission** (GIT-P17 / P-278, M3-G3 — the content-bodies half). A Git body is a
/// real [`myelin_content`] document ([`body::Body`]): the markdown-subset string + the three positional
/// structured inline nodes, round-tripped through the ONE editor render path (`render(parse(md)) ===
/// md`) and **single-author CAS** ([`body::Body::cas_edit`]). The three inline ref nodes produce
/// `refs.edge.created` UNIFORMLY via the outbox ([`body::emit_body_edges`], contract 5.4 — no standalone
/// edge-write API), in the SAME transaction as the body's `git.pr.*`/`git.comment.*` content event
/// (emit-iff-committed). This is the **Git-owned producer half** of 5.4 (Git cannot depend on the Refs
/// service crate — the §2.9 DAG); it produces the byte-identical edge wire-shape the Refs edge-builder
/// consumes (CDC-pinned). Replaces the GIT-P16 opaque `BodyRef` ciphertext floor for the body content.
/// Floors: the typed-edge LIFECYCLE mirror (`Closes`/PR-link edges) is the DISTINCT GIT-P19 follow-on;
/// the per-subject-DEK at-rest seal rides the GIT-P20 store wiring.
pub mod body;
pub mod check_status;
pub mod commit;
/// The **GitCore layered seam** (GIT-P8 / P-269, M3-G1): the strategy trait + router that sends
/// wire/maintenance ops to sandboxed canonical `git` (the [`core::WireExecutor`] port) and read
/// ops to the in-process backend. The internal substrate GIT-P9 (receive-pack) + GIT-P13 (serving
/// tier) build on. See the module docs for the TE-8 position, the no-host-exec discipline, and the
/// OQ-1 gix-ward floor (GIT-P33).
pub mod core;
pub mod events;
/// The **stateless Git front door / router** (GIT-P13 / P-274, M3-G2 — FIRST RUNNABLE): the one
/// pipeline every SSH (`russh`) + smart-HTTP-v2 (`axum`/`hyper`) entrypoint funnels through —
/// `authenticate` (Id 4.1) → `check` + `CaveatContext` (Id 4.2) → `placement_of(repo)` (12.2) →
/// **residency reject-if-leaving-region** (ADR-11 / 12.4) → stream packs through the [`core`] seam
/// (no whole-pack buffering); `liveness ≠ readiness`. The GIT-D8 invariant — **tenant from the
/// TOKEN, never the URL path** — is enforced as a structural cross-tenant deny BEFORE check/
/// placement/stream (0 cross-tenant read). Floors: GIT-P14 wires the ReBAC fragment LIVE + the
/// FailStatic degrade-not-cascade bound; GIT-P15 lands the protected-human-lane shed order + the
/// CDN bundle-URI accelerated-clone. See [`front_door::FrontDoor`].
pub mod front_door;
/// The in-process read backend ([`gix_backend::GixCore`]) over `git2` (libgit2 — the
/// architecture-named fallback; gix-preferred is the OQ-1 floor, GIT-P33). Read/diff/blame with no
/// `git` fork (no-host-exec by construction).
pub mod gix_backend;
pub mod holder_intent;
/// The Git ReBAC fragment wired LIVE + the FailStatic bound on the Id dependency (GIT-P14 / P-275,
/// M3-G2): the [`live_check::GitCheckGate`] runs the front door's `pull`/`push` + the push-policy
/// `protected_push` + the merge gate's `merge` + the X-1 `approve_untrusted_ci` fork-endorsement +
/// CODEOWNERS `list_subjects` against the live fragment (contract 4.9), with the git→Id `check`
/// bounded by the shared `myelin_substrate::FailStaticAuthz` (1.10/4.11) so an Id hiccup DEGRADES
/// (bounded-stale coarse grant) instead of cascading, a just-revoked subject is still denied, and a
/// zookie read bypasses the cache (4.10 read-your-writes).
pub mod live_check;
/// The **PR/review/inline-thread lifecycle + branch-protection rulesets + the CODEOWNERS resolver**
/// (GIT-P16 / P-277, M3-G3 — the domain-entities half): the hosting-layer entities not in git itself
/// (00-overview §1.1). [`lifecycle::PullRequest`] + [`lifecycle::PrState`] are the PR lifecycle state
/// machine (0 illegal transitions); [`lifecycle::Review`] / [`lifecycle::Thread`] the review +
/// inline-comment-THREAD entities; [`lifecycle::BranchProtectionRuleset`] +
/// [`lifecycle::evaluate_ruleset`] the entity-layer branch-protection gate (0 unprotected merges to a
/// protected ref); and [`lifecycle::CodeOwners`] the **4.9 CODEOWNERS-as-relations resolver** —
/// compiling CODEOWNERS path patterns to `code_owner` relation tuples (0 mis-resolved owners), so "who
/// must approve this path" is the ordinary `list_subjects(ref, code_owner)` Expand
/// [`live_check::GitCheckGate::code_owners`] already runs. Floors: PR/review/thread BODIES are
/// single-author CAS — the body content + the myelin-content round-trip is GIT-P17; the diff
/// line-anchor 4-state resolver is GIT-P23/GIT-P24; the live OLTP store + per-ref ruleset persistence
/// (+ the `write_tuples` CODEOWNERS tuple write) is GIT-P20/GIT-P22.
pub mod lifecycle;
pub mod notif_rules;
/// The git **PACK TIER on the local-NVMe `BlobStore` floor** (GIT-P11 / P-272, M3-G1): the git-side
/// object-DB migration THROUGH the [`myelin_storage::GitPackTier`] (closing the receive-pack
/// `QuarantineMigration` floor), the commit-graph/reachability-bitmap/MIDX maintenance artifacts +
/// their staleness fences (arch `01 §4.1` / `02 §8`), the byte-identical clone round-trip (the GATE),
/// and the residency-pin lint (repos relocatable, never node-pinned — STOR-5). Floors GF-1 (object-
/// backed packs) / GF-2 (cross-cell) / GF-2b (SHA-256 flip) / GF-4 (Mononoke-class) all → GIT-P33.
pub mod pack_tier;
pub mod rebac_fragment;
/// The **receive-pack write path** (GIT-P9 / P-270, M3-G1): the in-process Rust policy +
/// one-transaction ref-CAS + `git.ref.updated` outbox emit (the silent-data-loss floor, GIT-D9 —
/// emit-iff-committed, 0 ghost / 0 lost). The [`receive_pack::RefStore`] models the reftable-on-OLTP
/// ref store (the per-ref CAS is the linearisation point) and co-commits the ref move with the event
/// through the frozen [`myelin_events::OutboxStore`] same-transaction surface. Opening the store
/// auto-registers it as `PersonalDataHolder` H1 (the DSR bodies are the GIT-P29 floor).
pub mod receive_pack;
pub mod replay;
pub mod schema;
pub mod search_projection;
/// The **protected-human-lane shed order + the CDN bundle-URI accelerated-clone** (GIT-P15 / P-276,
/// M3-G2): the [`shed_clone::GitFrontDoorShed`] wires the substrate's shed lane
/// (`speculative → batch/CI → agent → human-last`, `429 + Retry-After`) over the new
/// `myelin_substrate::shed::Surface::GitFrontDoor` budget read from the thresholds file — a clone
/// storm's agent/CI lane sheds while the human's interactive fetch is served (the OQ-K per-surface
/// budget floor; the 30× clone storm GIT-D6 tunes the numbers in GIT-P34). The
/// [`shed_clone::BundleUriClone`] serves a clone a **bundle-URI** from the within-EU CDN clone/bundle
/// class (`myelin_storage::cdn::CdnCloneClass`, 11.2 C3) — a content-address-verified accelerated
/// clone (the full within-EU CDN class hardens in GIT-P33).
pub mod shed_clone;
pub mod subs;
