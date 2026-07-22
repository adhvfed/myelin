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
/// **Agents as first-class, legible, bounded authors/reviewers** (GIT-P28 / P-289, M3-G6): the
/// GIT-domain half of the agent author/reviewer surface — the tool identity constants
/// ([`agent_author::COMMENT_TOOL`] / `SUBMIT_REVIEW_TOOL` / `SUGGEST_CHANGE_TOOL` /
/// `RESOLVE_THREAD_TOOL`), the `required_caps` built from the frozen Git ReBAC fragment
/// ([`agent_author::review_authoring_required_caps`] = `pull_request.review` — the SAME permission a
/// human reviewer is governed by, EI-02 §2), and the typed [`agent_author::Authorship`] legibility
/// value (ADR-08 / EU AI-Act: an agent author carries its provenance — which agent, which run, why —
/// and is STRUCTURALLY never disguised as a human). Authoring is reversible → NOT HITL-gated (the
/// ONLY consequential git gate stays `git.merge`, §6.3). Every authored effect routes through the
/// Fabric `EffectApi::apply` (8.2 plan-then-apply, AG-D1/D2/D3); agents never write directly. The
/// thin `ToolDef` registration lives in `myelin_agent_service::git_tools` (the §2.9 DAG — git is a
/// leaf), keyed on this module's identity constants.
pub mod agent_author;
/// The **content-anchored inline-thread line-range resolver: the `#sub` 4-state ladder** (GIT-P24 /
/// P-286, M3-G4 — GIT-D7). The mint half ([`subs::mint_blob_line_range`], GIT-P4) ships the
/// grammatical `#L<a>-L<b>` sub-URN; this module is the RESOLVER the Refs ladder calls in its
/// sub-resolve step. A [`anchor::LineAnchor`] stores the mint-time blob oid + path + side + range +
/// commit + the **BLAKE3 fingerprint** of the anchored lines + a [`anchor::CONTEXT_WINDOW`] context
/// window; [`anchor::resolve`] resolves it against the head blob through the frozen 4-state ladder —
/// **EXACT→Live / REBASED→Moved / PARTIAL→Outdated / GONE→content_gone** (contract 5.7 / X-4 / arch
/// §5.1) — and ALWAYS carries the state + the "view in original context" position so a relocated/lost
/// anchor is **never silently wrong** (EI-01 §3; GIT-D7 = 0 mis-anchored). Floor named: **GF-5** —
/// patch-id-chain carry-over across a multi-commit rebase is the **GIT-P33 / M5** follow-on; v1 does
/// the per-pair blob-diff fingerprint remap.
pub mod anchor;
pub mod body;
pub mod check_status;
/// The **STORE-BACKED `check_status` projection** (GIT-P20 / P-281, M3): the LIVE Postgres binding of
/// the in-memory [`check_status::CheckStatusProjection`] — the real table + migration + the same-tx
/// idempotent-on-`event_id` + monotonic `run_attempt` supersession apply (contract 5.9 / X-1 / §6.1).
/// Compiled ONLY under `--features integration` (the default build stays DB-free); the GIT-D10
/// part-(a) green artifact is proven against the dev Postgres stack.
#[cfg(feature = "integration")]
pub mod check_status_store;
/// The **code-projection EMITTER for Search** (GIT-P25 / P-287, M3-G5 — the §9 TE-27 code
/// projection). The receive-pack post-commit hook that, on a `git.ref.updated` to an indexed ref,
/// diffs `new_tip ∖ last_indexed_oid` ([`code_projection::CodeProjectionCursor`]) and emits ONE
/// `git.blob.snapshot` projection doc per changed blob through the outbox (per-blob, incremental —
/// the GATE `emit-count == changed-blob-count`, 0 missed / 0 stale). Each upserted doc carries the
/// §9 shape (path / detected language / camel·snake-split symbols / literals / blob text / tip commit
/// message / blob_oid) lowering to the GIT-P5 Search-owned [`search_projection`] spec's facets; a
/// delete is a tombstone; a `restrict`-ed subject's body is suppressed (§6). Git emits the projection;
/// Search builds the trigram/symbol/path/literal index (no cross-DB). Floors: **GF-3** trigram/lexical
/// search v1 is what this feeds — AST-aware "find usages" via CI-produced SCIP/LSIF (6.5, R-3) is the
/// **GIT-P33/M5** follow-on; the leak-free `list_objects` SetExpr push-down + code-search pre-filter
/// (GIT-D11) that conjoins this per viewer is **GIT-P26/P-288**; the production tree-walk/blob read
/// rides the [`gix_backend`]/[`core`] seam (GIT-P13).
pub mod code_projection;
/// The **code-executing git tools on the unified sandbox** (GIT-P27 / P-283, M3-G6 — the AG-D4
/// gate): the audited, tamper-evident, **rate-limited** [`code_tools::HistoryRewriteTool`] erasure
/// op (contract 10.6 / recon §9) with the **fork/mirror/clone-cache invalidation fan-out** (the
/// trust-scoped cache namespaces, 11.2 C4), and the [`code_tools::ScipIndexJob`] SCIP-indexing
/// compute descriptor. Both ride git's [`core::WireExecutor`] no-host-exec sandbox seam (= the CI
/// `kind=agent` job the AG-D4 escape drill gates), inheriting the FOUR uniform sandbox guarantees
/// BY CONSTRUCTION. The Fabric `ToolDef` registration that catalogues them (`git.history_rewrite`
/// gated, `git.scip_index` compute) lives in `myelin_agent_service::git_tools` (the §2.9 DAG — git
/// is a leaf), keyed on [`code_tools`]'s identity constants. Floors: GF-9 (`exposed_over_mcp=false`,
/// the external MCP server is GIT-P33/P6); the erasure SEMANTICS complete at GIT-P29;
/// agents-as-authors is GIT-P28.
pub mod code_tools;
pub mod commit;
/// The **GitCore layered seam** (GIT-P8 / P-269, M3-G1): the strategy trait + router that sends
/// wire/maintenance ops to sandboxed canonical `git` (the [`core::WireExecutor`] port) and read
/// ops to the in-process backend. The internal substrate GIT-P9 (receive-pack) + GIT-P13 (serving
/// tier) build on. See the module docs for the TE-8 position, the no-host-exec discipline, and the
/// OQ-1 gix-ward floor (GIT-P33).
pub mod core;
/// **Cross-cell / multi-region git replication** (GF-2 → GIT-P33, M5): the single-cell primary+quorum
/// floor lifts to cross-cell active replica sets within-EU, riding the OQ-I [`myelin_tenancy::
/// CrossCellPointer`] bridge ([`cross_cell::CrossCellReplicaSet`]). `update_seq` is the fence; resolution
/// is always cell-local; the bridge frame is PII-free.
pub mod cross_cell;
pub mod events;
/// The **fork / trust-tier endorsement gate** (GIT-P22 / P-284, M3-G4 — the poisoned-pipeline defence).
/// Closes the [`merge_gate`] floor by shipping the two halves the merge gate consumed as explicit
/// inputs in GIT-P21: (A) the LIVE endorsement RESOLVER ([`fork_gate::EndorsementResolver`]) that runs
/// the maintainer's `check(subject, approve_untrusted_ci, repo)` through the LIVE
/// [`live_check::GitCheckGate`] (GIT-P14) for each required context whose CURRENT row is an un-endorsed
/// `untrusted_fork` success, PRODUCING the `endorsed_contexts` set [`merge_gate::evaluate_merge_gate`]
/// consumes (a fork can never self-green — the subject is the maintainer who holds the relation, never
/// the fork author); and (B) the **`fork:<pr_id>` trust-scoped cache confinement**
/// ([`fork_gate::TrustScope`] + [`fork_gate::ScopedCache`], contract 11.2 C4 / recon §8) — a scope-key
/// convention over the per-tenant [`myelin_storage::Cache`] so an `UntrustedFork` write can NEVER reach
/// the `trusted:` cache scope (0 fork writes in the trusted scope; the scope is DERIVED from the
/// CI-stamped trust tier, never caller-chosen). Floor: the merge queue (durable workflow, exactly-once
/// merge, the `ci.result` rollup wait) is GIT-P23.
pub mod fork_gate;
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
/// **Git `resolve(ref, viewer, Display)` for unfurls, wired through Notif's humanise** (GIT-P31 /
/// P-292, M3-G8 — the notifications + humanise half). [`git_resolve::GitRefResolver`] implements
/// Notif's frozen [`myelin_notif::RefResolvePort`] (contract 5.2 — the resolve seam humanise consumes)
/// by delegating to Git's REAL [`project::Projector`] (contract 5.6): a confidential PR/commit subject
/// resolves to a **humanised tombstone, the TITLE NEVER LEAKS** (NOTIF-D4-class, threshold 0). This
/// PROMOTES the GIT-P19 test-local resolve stand-in to a real producer-crate seam (EI-01 §7) — the
/// leak property is now exercised over Git's real permission-first projection. Review-requests are a
/// FILTER over the ONE inbox ([`git_resolve::git_review_requests_filter`], contract 7.1), never a second
/// store. Floors: the Web-UI/CLI render is GIT-P32; the live OLTP store is GIT-P20; cross-cell resolve
/// is single-home (contract 5.2 / OQ-I).
pub mod git_resolve;
/// The in-process read backend ([`gix_backend::GixCore`]) over `git2` (libgit2 — the
/// architecture-named fallback; gix-preferred is the OQ-1 floor, GIT-P33). Read/diff/blame with no
/// `git` fork (no-host-exec by construction).
pub mod gix_backend;
/// **Durable on-disk git STORAGE (GT-001 / E1.1).** Real on-disk bare repos at
/// `<root>/<tenant>/<region>/<repo>.git` via `git2` ([`durable::DurableGitStore`] /
/// [`durable::DurableGitRepo`]) — repo lifecycle (`init_bare`), durable refs + reflog + CAS
/// (fixes SI-012: `open` loads refs from disk), and the on-disk odb as the object tier (fixes
/// F-git-2: the oid→object lookup is the real odb, no in-memory index). The WRITE/lifecycle
/// companion to the read backend [`gix_backend::GixCore`] — REUSES the same resolver + `git2`,
/// never reimplements git. The smart-transport WIRE sits on this (GT-006, sandbox-gated).
pub mod durable;
/// Bounded, stable keyset pagination over the durable branch/tag namespace. Cursors are opaque
/// consistency fences scoped to one verified repository and normalized query; they never grant
/// object access.
pub mod refs_pagination;
/// Bounded, snapshot-stable keyset pagination over one durable Git tree directory. Cursor object
/// ids are consistency fences only; every request resolves its ref and path normally first.
pub mod tree_pagination;
/// **GT-003 (E1.2) — the cross-system recovery reconciler.** [`reconcile::reconcile_refs`] replays the
/// committed `git.ref.updated` outbox rows against the durable on-disk repo, re-applying any whose
/// on-disk `update_seq` is behind the durable reflog (the apply-after-outbox-commit crash window —
/// [`receive_pack::CrashPoint::AfterCommitBeforeApply`]). At-least-once + idempotent on `update_seq`
/// (arch §4.2); the GT-001 prerequisite for the durable store reaching the live front door. Reuses the
/// durable per-ref CAS + the on-disk reflog as the generation — no parallel ref store, no parallel seq.
pub mod reconcile;
/// **GT-003 (E1.2) — the DURABLE PR/review store + the gated, durable merge.** [`pr_store::DurablePrStore`]
/// persists the [`lifecycle`] PR/review entities as on-disk repo metadata (tenant/region path-isolated via
/// the SAME validated resolver the durable git store uses); [`pr_store::merge_pr`] evaluates the reused
/// [`merge_gate`] (required-set + fork-trust) + [`lifecycle::evaluate_ruleset`] (approvals/CODEOWNERS/
/// conversations) and advances the target ref via the durable per-ref CAS ONLY on a fully-admitted gate —
/// never a policy bypass. The PG home for these rows (MR-022 provider) is the named GT-003b follow-on.
pub mod pr_store;
/// GT-003b — the forward-only PostgreSQL PR lifecycle schema and provider-backed store.
pub mod pg_pr_store;
pub mod pr_list_pagination;
/// **R3.3 / R3.2 (shared) — the DURABLE PR review-thread / comment / review-batch store.** The ONE
/// canonical conversation store both R3 packs consume (the `_gate.md` §02/§03 cross-pack
/// reconciliation): the model is THREADS (an optional content anchor; comments belong to threads);
/// review batching layers on via `review_id` + the [`pr_threads::ReviewBatch`] lifecycle; pending
/// comments are visible only to their author until submit; submit yields exactly ONE batch event
/// (R-BATCH-1). Keyed by the canonical [`myelin_refs::object_key`] tuple key so issues/docs mount the
/// SAME store later. JSON-on-disk (the [`pr_store`] pattern; the PG home is the GT-003b follow-on).
pub mod pr_threads;
/// **GT-002 (E1.1) — REAL git-repo backup + DESTRUCTIVE restore.** [`backup::GitRepoBackup`] captures
/// a GT-001 on-disk bare repo's complete object graph + refs into a single self-contained artifact (a
/// ref snapshot + a non-thin libgit2 packfile — the canonical `git bundle`-style mechanism, NOT a
/// modeled WAL offset), from which [`backup::restore_repo`] reconstructs the repo onto a CLEAN target
/// alone — proven IDENTICAL on read-back + `git fsck --full --strict` clean (the external oracle).
/// Fixes census SI-014/015 for the git slice; the DB-PITR offset tier stays the deferred
/// [`myelin_storage::backup`]/[`myelin_storage::restore`] floor. Reconciles as the content-addressed
/// T2 object tier ([`backup::GitRepoBackup::store_tier`]) — composes with the storage framework, does
/// not fork it. Reuses the GT-001 durable store + the validated path resolver (no traversal bypass).
pub mod backup;
/// The **git `PersonalDataHolder` H1 BODY: the DSR fan-out + history-rewrite erasure semantics**
/// (GIT-P29 / P-290, M3-G7 — GIT-D2 complete). [`holder::GitPersonalDataHolder`] is the real
/// `locate/export/rectify/restrict/erase` over git + metadata (contract 10.1/10.4), completing the H1
/// holder [`holder_intent`] declared (GIT-P3) + [`receive_pack::RefStore::open`] auto-registered
/// (GIT-P9). [`holder::GitPersonalDataHolder::erase_fanout`] drives the architecture-§6.1 erasure
/// algorithm over the storage [`myelin_storage::erase::CryptoShredErase`] orchestrator (pseudonym-map
/// shred 4.8 + per-subject DEK crypto-shred 11.4 + the [`myelin_storage::git_shred::GitCryptoShredReach`]
/// reflog/bitmap/pack-backup reach + Search purge + Refs tombstone + Bus erase + erasure ledger 10.8)
/// PLUS git's H9 cache/CDN invalidation fan-out, asserting EVERY git holder ([`holder::GitHolder::ALL`])
/// is hit (GIT-D2: 0 holders missed, 0 recoverable PII in backups, residual == the ONE platform posture
/// 10.9/X-7 — by reference, never restated). [`holder::GitPersonalDataHolder::expunge_body`] is the
/// history-rewrite erasure SEMANTICS (10.6): the X-7 body-expunge residual path through the GIT-P27
/// audited [`code_tools::HistoryRewriteTool`] (changed-hash consequence + the invalidation fan-out).
/// Floor (GF-7): the structural floor ships here regardless; the lawful-basis residual is R-7
/// (parallel/Legal, NOT a code gate). The reindex-from-cold parity (GIT-D3) is GIT-P30.
pub mod holder;
pub mod holder_intent;
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
/// The **leak-free `list_objects` `SetExpr` push-down for repo/PR lists + the code-search pre-filter**
/// (GIT-P26 / P-288, M3-G5 — the GIT-D11 gate). The git-SIDE consumer of the frozen contract-4.3
/// push-down: [`list_filter::lower_over_repo_id`] / [`list_filter::lower_over_pr_id`] lower the
/// returned `SetExpr` into a SQL predicate + `authz_visible` JOIN over Git's OWN id column
/// (`repo.id` / `pr.id`, §5.3 / §7.3) — **one query, no N+1, no post-filter**;
/// [`list_filter::compose_repo_list_query`] / [`list_filter::compose_pr_list_query`] conjoin it into
/// the ONE leak-free list statement (the ACL pre-filter BEFORE pagination, the tenant predicate
/// always emitted); [`list_filter::code_search_pre_filter`] is the 6.1 code-search pre-filter (the
/// blob doc's ACL object is the parent `repo`, GIT-P5) the `search-requires-acl-filter` lint requires
/// conjoined before scoring. [`list_filter::AuthzVisibleIndex`] models the per-tenant residency-pinned
/// reverse index + the new-enemy zookie guard for the DB-free unit/CDC drills; the live one-query/
/// 0-leak/revoke-reflected GIT-D11 proof is the `--features integration` test against the dev-stack
/// Postgres. Git's code projection is asserted leak-free in the shared SRCH-D1/D3 here. No new floor.
pub mod list_filter;
/// The Git ReBAC fragment wired LIVE + the FailStatic bound on the Id dependency (GIT-P14 / P-275,
/// M3-G2): the [`live_check::GitCheckGate`] runs the front door's `pull`/`push` + the push-policy
/// `protected_push` + the merge gate's `merge` + the X-1 `approve_untrusted_ci` fork-endorsement +
/// CODEOWNERS `list_subjects` against the live fragment (contract 4.9), with the git→Id `check`
/// bounded by the shared `myelin_substrate::FailStaticAuthz` (1.10/4.11) so an Id hiccup DEGRADES
/// (bounded-stale coarse grant) instead of cascading, a just-revoked subject is still denied, and a
/// zookie read bypasses the cache (4.10 read-your-writes).
pub mod live_check;
/// The **merge gate + the required-set policy** (GIT-P21 / P-282, M3-G4 — "Git owns what is allowed to
/// land"). The bridge the merge gate fires across: it parses a `base_ref`'s branch-protection
/// [`lifecycle::BranchProtectionRuleset`] `required_contexts` strings into typed
/// [`check_status::CheckContext`]s ([`merge_gate::parse_required_context`]), resolves each against
/// Git's OWN [`check_status`] projection for the PR `head_oid`, applies the fork-endorsement trust
/// posture (reading `trust_tier` OFF the fact, never recomputing it), and returns the typed
/// [`merge_gate::MergeGateOutcome`] — the **0-under-gated-merges** decision (a missing/stale/un-endorsed
/// required context ALWAYS blocks). REUSES the per-context gate logic in [`check_status`] (EI-01 §7 —
/// extend/reconcile, never duplicate). The live-store gate read is
/// [`check_status_store::PgCheckStatusProjection::merge_gate`] (proven against the dev Postgres stack).
/// Floors: the LIVE `approve_untrusted_ci` fork-endorsement resolution + the `fork:<pr_id>` cache
/// confinement is GIT-P22; the merge queue (durable workflow, exactly-once merge, the `ci.result`
/// rollup wait) is GIT-P23.
pub mod merge_gate;
/// The **merge queue as a durable workflow** (GIT-P23 / P-285, M3-G4 — parks on `ci.result`,
/// exactly-once merge, GIT-D10 part (d) + the full GIT-D10 aggregate). The Git-side COMPOSITION of the
/// generic durable merge-queue body ([`myelin_flow::WfCtx::run_merge_attempt`], contract 9.4 — already
/// live at P-FLOW-19): it binds Git's OWN merge gate ([`merge_gate`], §6.2) + the fork-endorsement
/// posture ([`fork_gate`], §6.3) into the durable [`myelin_flow::MergePerformer`] seam via
/// [`merge_queue::GitMergePerformer`], so a merge is performed ONLY when EVERY required context is a
/// current trusted/endorsed success on Git's projection (an under-gated / fork-self-greened / missing
/// context REFUSES the merge → the flow body dequeues with a humanised reason; 0 such merges land). The
/// doubly-delivered-`ci.result`-wakes-once + exactly-once-merge mechanics come FROM `myelin-flow` (not
/// re-implemented, EI-01 §7); Git reads its OWN `check_status` projection and NEVER synchronously calls
/// CI (no-cross-sync-cycle, X-1 / EI-02 §3). Floors: GF-8 single-lane (the speculative queue is
/// GIT-P33/M5); the seam-floor — the REAL CI producer is EB-27/M4 (the X-1 seam goes end-to-end at the
/// M4 co-gate GIT-D10 / CI-D8; here the `ci.result` rollup is the synthetic
/// [`myelin_flow::MockCiResultProducer`]).
pub mod merge_queue;
pub mod notif_rules;
/// **The SHA-256 default flip** (GF-2b → OQ-9, GIT-P33 / P-482, M5): the new-repo default object format
/// flips from SHA-1+`sha1dc` to SHA-256, hash-AGNOSTIC — a default-CHANGE, not a migration. The flip is
/// GATED on the stock-tooling interop bar ([`object_format::flip_default_to_sha256`]); existing repos
/// are untouched (their [`object_format::ObjectFormat`] is immutable).
pub mod object_format;
/// **Git-side OBJECT-BACKED packs** (GF-1 → R-1/OQ-4, GIT-P33 / P-482, M5): the [`pack_tier::
/// PackObjectDb`] promoted from the local-NVMe floor onto the storage OBJECT tier
/// ([`myelin_storage::object_backed_pack_tier`]) — a type-parameter SWAP, not a rewrite. Adds the OQ-4
/// quorum-ack property ([`object_packs::object_backed_migration_acks_on_quorum`]) and the
/// smart-transport byte-parity GATE ([`object_packs::smart_transport_parity`]).
pub mod object_packs;
/// The git **PACK TIER on the local-NVMe `BlobStore` floor** (GIT-P11 / P-272, M3-G1): the git-side
/// object-DB migration THROUGH the [`myelin_storage::GitPackTier`] (closing the receive-pack
/// `QuarantineMigration` floor), the commit-graph/reachability-bitmap/MIDX maintenance artifacts +
/// their staleness fences (arch `01 §4.1` / `02 §8`), the byte-identical clone round-trip (the GATE),
/// and the residency-pin lint (repos relocatable, never node-pinned — STOR-5). Floors GF-1 (object-
/// backed packs) / GF-2 (cross-cell) / GF-2b (SHA-256 flip) / GF-4 (Mononoke-class) all → GIT-P33.
pub mod pack_tier;
/// **Patch-id-chain anchor carry-over** (GF-5 → R-6, GIT-P33, M5): a content-anchored inline thread
/// ([`anchor`]) follows a rebased hunk through a MULTI-commit rebase by matching `git patch-id` across
/// the pre/post-rebase commit sequence ([`patch_id_chain::carry_anchor_through_rebase`]) — so it
/// resolves `Moved`, not degraded to `Outdated`, when an intermediate commit perturbs context.
pub mod patch_id_chain;
/// **`project(ref, viewer)` for git artifacts + the `ArtifactRef` id grammar** (GIT-P18 / P-279,
/// M3-G3 — the projection half): the [`project::Projector::project`] is the ONLY way Refs/Search/Notif
/// read a git artifact (no cross-DB), **per-viewer permission-checked** — a viewer without access gets
/// a [`project::Tombstone`], NEVER the title (0 title leaks; feeds the M3-G5/M5 leak drills GIT-D11 /
/// SRCH-D1/D3). The [`project::git_pr_ref`] / [`project::git_commit_ref`] helpers mint git's STABLE
/// canonical keys (`pr/<repo>:<n>`, `commit/<repo>:<sha>`) through the ONE Refs codec; the
/// [`project::display_key`] `#1421`/short-sha is render-time ONLY (0 stored display keys, REF-3).
/// Floors: the live OLTP store is GIT-P20; the `blob`/`#L<a>-L<b>` content-anchored 4-state resolver is
/// GIT-P24; cross-cell projection is single-home (the named multi-cell floor).
pub mod project;
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
/// **SCIP/LSIF "find usages"** (GF-3 → R-3, GIT-P33, M5): AST-aware code intelligence fed by
/// CI-produced SCIP indices ([`scip::ScipIndex`]). Git OWNS the find-usages projection (contract 6.5);
/// Search owns the index. The lexical trigram floor ([`code_projection`]) stays; this adds the
/// symbol-occurrence "find usages"/"go to definition" layer on top.
pub mod scip;
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
/// **The speculative/parallel merge queue** (GF-8 → OQ-5, GIT-P33, M5): the single-lane serialised
/// queue promotes to speculative batching once the promotion trigger is MEASURED
/// ([`speculative_queue::PromotionTrigger`]). A speculative batch builds on optimistic tips; a base
/// movement bisects + rebases the survivors — linearizable on the protected `base_ref` (GIT-D5).
pub mod speculative_queue;
pub mod subs;
/// **World-scale hardening: the GIT-D6 clone-storm surge + git's E2E slices** (GIT-P34 / P-483, M5).
/// The M5 production-hardening face of the Git front door: the [`surge::run_git_clone_surge`] runner
/// drives the LIVE [`shed_clone::GitFrontDoorShed`] at the 30× clone surge (human fetch HELD, agent + CI
/// SHED, cross-tenant impact 0 — the F6 surge family's git row, GIT-D6), and [`surge::run_git_e2e_wedge`]
/// composes git's slices of the three whole-system E2E scenarios (E2E-1 the PR-context reference
/// producer; E2E-2 the agent-native flagship — the `git.merge` HITL gate + the X-1 CheckStatus gate +
/// `git.pr.merged` closing the issue via the `Closes` trailer, exactly-once HITL + merge; E2E-3 the
/// commit→PR→merge lineage, cold-reindex == live). Authors NO new mechanism — it is the world-scale
/// composition + drill over the engine the M3/M5 prompts shipped (EI-01 §7). No new floor; the one
/// remaining floor is the world-scale 30× run on real fleet hardware (the shared §4.1 fleet drill).
pub mod surge;
/// The **typed-edge mirror: PR-link / commit-trailer lifecycle edges into the Refs projection**
/// (GIT-P19 / P-280, M3-G3 — the typed-edge-mirror half). As the PR lifecycle advances, a Git PR emits
/// **lifecycle edges** (`closes`/`relates`, `rel_class='lifecycle'`) via the outbox — DISTINCT from the
/// content-node `mention`/`artifact_ref`/`embed` REFERENCE edges ([`body`], GIT-P17,
/// `rel_class='reference'`). A `Closes <ISSUEKEY>` trailer on a MERGED PR
/// ([`typed_edges::parse_closes_trailers`]) produces exactly one `closes` edge (PR→issue); an explicit
/// PR-link produces one `relates` edge. [`typed_edges::emit_lifecycle_edges`] emits one
/// `refs.edge.created` per linkage in the SAME outbox transaction as the PR's `git.pr.merged` /
/// `git.pr.updated` lifecycle event (emit-iff-committed — 0 dup/missed; no lifecycle edge without its
/// committed transition). This is the **Git-owned producer half** of contract 5.5 (Git cannot depend on
/// the Refs service crate — the §2.9 DAG); it produces the byte-identical lifecycle-edge wire-shape the
/// Refs mirror consumer (`myelin_refs_service::mirror`) ingests (CDC-pinned). Floors: the inverse
/// projection is the Refs mirror's (Git emits forward only); `blocks`/`depends_on`/`parent`/`assigns`
/// are the Issues/Knowledge typed tables' (REF-P18/REF-P20); the live PR-merge transition wiring is
/// GIT-P20/GIT-P22.
pub mod typed_edges;

/// The **Git Web UI view-model + HTML render layer** (GIT-P32 / P-293, M3-G8 — FIRST USEFUL). The
/// server-rendered view-model for the load-bearing Web UI surfaces (repo home + file view, the PR
/// overview centrepiece, the **checks panel**, the signed-off **fork-trust badge**, the
/// **merge-readiness** affordance, the single-file **web-edit** form), built TO the GIT-P7 signed-off
/// design pass and conforming to the frozen design system (DESIGN-MANUAL, direction A "Instrument").
/// NO new contract — the views CONSUME the already-built [`project::Projected`] (0-leak per-viewer
/// projection), [`check_status`] (the X-1 consumer rows), [`merge_gate`] (the gate outcome), and
/// [`lifecycle`] (the PR state). Status is glyph + label + token (never colour alone, WCAG 1.4.1);
/// a tombstone never leaks a title; the inline-colour ban holds. Driven in a real browser by
/// `tests/e2e_git_p32_web_browser.rs` (the switch-test rehearsal, EI-01 §4). Floor named: **GF-6**
/// single-file web edit (the in-browser 3-way conflict editor is GIT-P33/M5+).
pub mod web;

/// The **Git CLI + HTTP/RPC + agent-tool API surface catalogue** (GIT-P32 / P-293, M3-G8 — arch
/// `04-views-cli-and-api.md` §3/§4). The `myelin …` git CLI command surface ([`api::CliCommand`]) +
/// the HTTP endpoint route table ([`api::Endpoint`]) — ONE API, three consumers (UI, CLI, agents). NO
/// new handler: each CLI verb / HTTP route maps to an EXISTING git handler (the merge gate, the
/// fork-endorsement, the projection, the receive-pack path). The surface is the catalogue + the
/// parse/route logic; it surfaces the existing handlers (the prompt states this). Every write route is
/// `Id.check` → state change + outbox emit in one transaction (BUS-2); every cross-subsystem read uses
/// `project`/`resolve` (cell-local, never cross-DB).
pub mod api;

/// **Dogfood: git hosts Myelin's OWN repositories (GIT-P35 / P-518, M6 — THE DONE-BAR).** M6 promotes
/// NOTHING and freezes NO new contract — the engine is fixed at M3 and hardened through M5. This module is
/// the dogfood DRIVER over the already-shipped git surface on the Myelin self-tenant: the platform's own
/// monorepo on Myelin git hosting, its build/test/lint/mutation pipeline as a Myelin CI graph. It REUSES
/// the existing E2E-wedge runners ([`surge::run_e2e_1_pr_pane`] / [`surge::run_e2e_2_fix_pr`] /
/// [`surge::run_e2e_3_spec_to_ship`], EI-01 §7 — never a second engine) for the three dogfood faces (the
/// PR-context pane + the agent-native fix-PR flagship + the spec-to-ship lineage), plus the git TRUTH-UP
/// pass ([`dogfood::run_git_truth_up_scorecard`] over [`dogfood::proven_git_rows`]) — every PROVEN git row
/// (GIT-D1..GIT-D11 + the E2E slices) rests on a DATED green artifact whose proof source exists on disk; a
/// vanished/undated row is surfaced CLAIMED-NOT-PROVEN, never trusted on faith — and the
/// every-incident-adds-a-drill loop ([`dogfood::GitIncident`]). No new floor; the one remaining floor is
/// the world-scale 30× fleet-hardware load drill (the shared §4.1 fleet drill). The switch test is the
/// sibling [`switch_test`] module.
pub mod dogfood;

/// **The Git OQ-12 SWITCH TEST driven over the real surface (GIT-P35 / P-518, M6 — THE DONE-BAR).** The
/// "actually try it" gate (EI-01 §4): could a GitHub user move to Myelin git hosting WITHOUT hitting a
/// wall the old tool didn't have — MEASURED against the contrast + latency budgets + `render(parse(md)) ===
/// md` + the status overlays (git-hosting §3 M6-G10; VISION §3)? The DRIVER renders the real
/// [`web::PrOverviewPage`] (measured against the render-latency budget), round-trips the [`body::Body`]
/// corpus (`render(parse(md)) === md` at 100%, contract 13.1), and resolves every [`web::StatusCue`]
/// overlay's contrast against the design-language §8b measured floor — over the Myelin self-tenant. Reused,
/// never re-implemented (EI-01 §7). The pixel-level browser drive over the live WASM editor + `<svg>` icon
/// binding is the honest named floor — recorded per surface, never claimed.
pub mod switch_test;
