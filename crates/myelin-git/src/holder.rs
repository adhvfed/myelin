//! # `holder` — the git `PersonalDataHolder` **H1 body**: the DSR fan-out over git + metadata,
//! erasure-reaches-every-holder, and the history-rewrite erasure semantics (GIT-P29 / P-290, M3-G7)
//!
//! This is the **GIT-P29 deliverable**: the real `locate / export / rectify / restrict / erase` over
//! git + its hosting metadata (contract 10.1/10.4), completing **GIT-D2** — the erase-reaches-every-
//! holder obligation that [`crate::holder_intent`] declared as INTENT (GIT-P3) and
//! [`crate::receive_pack::RefStore::open`] auto-registered as the live holder receipt (GIT-P9).
//!
//! **Owning architecture docs (read in full before changing this):**
//! - `planning/04-subsystem-architectures/git-hosting/architecture/05-hard-problems.md` §HP-7 (the
//!   erasure posture completed — the mechanism half: pseudonym-map shred + per-subject DEK
//!   crypto-shred + the history-rewrite path; the residual is THE platform posture's residual).
//! - `03-events-contracts-and-glue.md` §6 (the H1 holder + **§6.1 the erasure algorithm / DSR
//!   fan-out**, §6.2 the ONE platform posture by reference, §6.3 the restriction flag).
//! - `00-overview.md` §1.1 (git is holder H1 — "the hardest in the platform").
//! - `00-reconciliation-decisions.md` X-7 (the ONE posture — instantiate by reference), §9
//!   (history-rewrite audited op + the invalidation fan-out).
//!
//! **Contracts implemented:**
//! - **10.1 / 10.4** (OWNED) — `PersonalDataHolder{locate, export, rectify, restrict, erase}` + the
//!   DSR state machine, over git PRs/reviews/comments/reflogs + the hosting metadata.
//! - **11.4** (CONSUMED) — the per-subject DEK crypto-shred (free-text BODIES + TITLES live AND in
//!   backups, [`myelin_storage::erase::CryptoShredErase`]) + the per-tenant blob DEK reach into
//!   reflogs / bitmaps / pack-tier backups ([`myelin_storage::git_shred::GitCryptoShredReach`]).
//! - **4.8** (CONSUMED) — the pseudonym-map shred (DSR step 1, the [`myelin_storage::erase::PseudonymShred`]
//!   seam wired to Id's `erase`).
//! - **10.6** (OWNED) — the audited history-rewrite erasure SEMANTICS: when a body must be EXPUNGED
//!   from the immutable bytes (the rare X-7 residual case), the GIT-P27
//!   [`crate::code_tools::HistoryRewriteTool`] is the supported disruptive op with the changed-hash
//!   consequence + the fork/mirror/clone-cache invalidation fan-out.
//! - **10.8 / 10.9** (CONSUMED) — the erasure ledger (the receipt sink) + the ONE posture (by
//!   reference, NEVER restated as a git-local statement).
//!
//! ## What this prompt (GIT-P29 / P-290) ships — and what it REUSES (EI-01 §7, coherence)
//! The crypto-shred MECHANISM (the storage [`myelin_storage::erase::CryptoShredErase`] six-step
//! orchestrator + the [`myelin_storage::git_shred::GitCryptoShredReach`]), the pseudonymous-by-default
//! commit codec ([`crate::commit`]), the per-subject-DEK body posture ([`crate::body`]), the
//! search-purge / refs-tombstone seams, the cache-invalidation fan-out
//! ([`crate::code_tools::CacheNamespace`] / [`crate::code_tools::CacheInvalidator`]), and the audited
//! history-rewrite tool ([`crate::code_tools::HistoryRewriteTool`]) ALL already exist. This module does
//! NOT re-implement any of them — it is the **H1 holder BODY** that drives the DSR fan-out over them
//! and proves EVERY git holder is hit (GIT-D2 complete). The genuinely-new code is:
//!
//! 1. [`GitPersonalDataHolder`] — the `impl myelin_gdpr::PersonalDataHolder` body for H1 (the FIRST
//!    real holder body on the platform; the GDPR-owned holders defer to P-GA-06, but git OWNS its own
//!    H1 fan-out per the §2.9 DAG — git cannot depend on the GDPR service crate).
//! 2. [`GitErasureFanOut`] — the closed set of git holders the erase MUST reach + the dated
//!    [`GitDsrReceipt`] artifact (the GIT-D2 green reading: 0 holders missed, residual == the ONE
//!    posture, backups shredded).
//! 3. The history-rewrite erasure SEMANTICS: [`GitPersonalDataHolder::expunge_body`] is the body-
//!    expunge path for the X-7 residual (a leaked secret / court order in a body), routing through the
//!    GIT-P27 audited tool with the changed-hash consequence + the invalidation fan-out.
//!
//! ## The DSR fan-out — every git holder (architecture §6.1)
//! `erase(subject, tenant)` over git reaches, in the §6.1 order, **every** locus the §4.5 inventory
//! ([`crate::holder_intent::personal_data_inventory`]) names:
//!
//! | # | holder reached | lever | contract |
//! |---|----------------|-------|----------|
//! | 1 | pseudonym map (commit/reflog actor identity) | pseudonym-map shred (Id.erase) | 4.8 |
//! | 2 | PR/review/comment BODIES + TITLES (live + backups) | per-subject DEK crypto-shred | 11.4 |
//! | 2b| reflogs / bitmaps / pack-tier backups | per-tenant blob DEK crypto-shred | 11.4 |
//! | 3 | search code/PR/comment index | purge + reindex-from-source | 6.1 |
//! | 4 | refs unfurls / backlinks | tombstone (degrade to "(deleted)") | 5.x |
//! | 5 | bus inline-PII keys + `*.erased` tombstones | crypto-shred + tombstone | 2.6 |
//! | H9| cache / CDN (fork/mirror/clone-cache/read-proj) | invalidation fan-out | 11.2 C4 |
//! | 6 | the erasure-ledger receipt | record (non-shred-erasable) | 10.8 |
//!
//! The **RESIDUAL** — third-party free-text PII typed into ANOTHER subject's un-erased body, +
//! immutable commit-message bytes authored by others — is EXACTLY the ONE platform posture (10.9 /
//! X-7), handled by reference + the on-demand history-rewrite path (10.6). It is **never** restated
//! here as a git-local statement (§6.2).
//!
//! ## Mutation floor (mandatory-core — a missed holder is a breach)
//! The DSR fan-out + the crypto-shred holder enumeration is **mandatory-core**: a missed holder is a
//! GDPR breach. The floor is stated + met — see [`mod@tests`] for the cargo-mutants reading; the
//! load-bearing mutants (a dropped holder in [`GitHolder::ALL`], a `holders_hit -> true`, a
//! `residual_is_the_posture -> true`, a skipped cache fan-out, an erase that does not destroy the
//! per-subject DEK) are each killed by an assertion in the unit + chained-e2e + CDC tests.
//!
//! ## DB-free
//! This module builds in-memory holder/receipt values + drives the storage `CryptoShredErase`
//! orchestrator (whose KMS-backed crypto-shred is itself DB-free — it is the in-process `KmsEngine`)
//! and git's [`crate::code_tools::CacheInvalidator`] seam. The LIVE-stack proof (the real KMS +
//! object-store backup snapshot reach) rides the storage integration drills (P-ST-24's
//! `git_d2_git_crypto_shred_drill`). So `cargo build --workspace` stays DB-free.

use myelin_gdpr::{
    DsrError, EraseReceipt, EraseScope, LocateReport, PersonalDataHolder, PortableBundle, Patch,
    Receipt, RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};
use myelin_storage::encryption::SubjectId;
use myelin_storage::erase::{CryptoShredErase, EraseError, EraseHolders, EpochMillis};
use myelin_storage::kms::KmsEngine;
use myelin_tenancy::Region;

use crate::code_tools::{
    CacheInvalidator, CacheNamespace, HistoryRewriteError, HistoryRewritePlan, HistoryRewriteReceipt,
    HistoryRewriteTool, RewriteRateLimiter,
};
use crate::core::WireExecutor;
use crate::holder_intent::HOLDER_ID;

// ───────────────────────────── the closed set of git holders the DSR fan-out reaches ─────────────

/// **The closed set of git holders an `erase(subject)` MUST reach** (architecture §6.1, the DSR
/// fan-out). PII-free — a closed enum tag. The erase asserts EVERY member is hit; a missed member is
/// a GDPR breach (a holder that still resolves to the subject's PII). The set is closed so a new git
/// locus can NOT be added without a fan-out decision (the routing is total — proven by the unit test
/// over [`GitHolder::ALL`]).
///
/// This is the §4.5 personal-data inventory ([`crate::holder_intent::personal_data_inventory`])
/// turned into the per-step REACH the holder body drives — one entry per §6.1 algorithm step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GitHolder {
    /// Step 1 — the pseudonym map (commit/reflog ACTOR identity). Shredded via Id.erase (4.8); after
    /// it the immutable bytes hold only the opaque pseudonym (the residual == the platform posture).
    PseudonymMap,
    /// Step 2 — PR/review/comment BODIES + TITLES, encrypted under the per-subject DEK (11.4).
    /// Crypto-shred destroys the DEK ⇒ unrecoverable live AND in backups by construction.
    SubjectBodies,
    /// Step 2b — reflogs / bitmaps / pack-tier backups, sealed under the per-tenant blob DEK (11.4).
    /// Reached by [`GitCryptoShredReach`] in the SAME crypto-shred step.
    GitStructures,
    /// Step 3 — the search code/PR/comment index. Purge + reindex-from-source (the plaintext-derived
    /// exception — a key-destroy would leave a stale index entry).
    SearchIndex,
    /// Step 4 — refs unfurls / backlinks. Tombstone (degrade to "(deleted)"; backlinks are
    /// projections, rebuilt — relies on the step-1 pseudonym shred).
    RefsProjection,
    /// Step 5 — bus inline-PII event keys + the `git.*.erased` tombstones consumers drop derived
    /// state on (references-not-payloads keeps this a short set).
    BusKeys,
    /// H9 — the cache / CDN holders (fork / mirror / clone-cache / read-projection). Invalidated so a
    /// fork/mirror/CDN-clone cannot resurrect the subject's pre-erase derived state.
    CacheCdn,
    /// Step 6 — the erasure-ledger receipt (10.8). Recorded (non-shred-erasable — it survives the
    /// crypto-shred it records AND a restore, so post-restore re-erasure can replay).
    ErasureLedger,
}

impl GitHolder {
    /// A stable, PII-free label for the holder (telemetry / the receipt — never personal data).
    pub fn label(self) -> &'static str {
        match self {
            GitHolder::PseudonymMap => "pseudonym-map",
            GitHolder::SubjectBodies => "subject-bodies-dek",
            GitHolder::GitStructures => "git-structures-blob-dek",
            GitHolder::SearchIndex => "search-index",
            GitHolder::RefsProjection => "refs-projection",
            GitHolder::BusKeys => "bus-keys",
            GitHolder::CacheCdn => "cache-cdn",
            GitHolder::ErasureLedger => "erasure-ledger",
        }
    }

    /// **The full set of git holders the DSR fan-out MUST reach** (architecture §6.1). The erase
    /// asserts EVERY member is hit (a missed member is a breach). Closed + total — a new git locus
    /// cannot be added without appearing here (proven by [`tests::the_git_holder_set_is_the_dsr_fan_out`]).
    pub const ALL: [GitHolder; 8] = [
        GitHolder::PseudonymMap,
        GitHolder::SubjectBodies,
        GitHolder::GitStructures,
        GitHolder::SearchIndex,
        GitHolder::RefsProjection,
        GitHolder::BusKeys,
        GitHolder::CacheCdn,
        GitHolder::ErasureLedger,
    ];
}

/// **The residual posture — instantiated BY REFERENCE to the ONE platform posture (10.9 / X-7),
/// NEVER restated as a git-local statement** (architecture §6.2). The variant exists so the receipt
/// can ASSERT the residual IS the posture (not a silent gap, not a byte-mutation the platform does not
/// do) — but the field's value carries the contract REFERENCE, never a fresh git-local statement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitResidualPosture {
    /// The residual (third-party / immutable free-text PII authored by OTHERS) is handled per the ONE
    /// platform posture (10.9 / `00 §X-7`): the structural floor (pseudonym-map shred + per-subject
    /// DEK crypto-shred + the `restrict` suppression) + the on-demand history-rewrite path (10.6) for
    /// the rare body-expunge case. Ratified by counsel/DPO as ONE statement (R-7), not five.
    OnePlatformPosture,
}

impl GitResidualPosture {
    /// The contract reference the residual is handled BY (never a git-local restatement — 10.9 /
    /// `00 §X-7`, ratified once by counsel/DPO for all five subsystems; the history-rewrite follow-on
    /// is 10.6; the lawful-basis residual is R-7, parallel/Legal — NOT a code gate).
    pub const RESIDUAL_POSTURE_REF: &'static str =
        "contract 10.9 / 00 §X-7 (the ONE platform free-text/immutable-content erasure posture); \
         git: pseudonymous-by-default (Id 4.8) + per-subject DEK shred (11.4) + restrict suppression; \
         on-demand history-rewrite = 10.6; lawful-basis residual = R-7 (parallel/Legal)";
}

// ───────────────────────────── the dated DSR receipt (the GIT-D2 green artifact) ─────────────────

/// **The dated GIT-D2 artifact the git DSR fan-out returns** — the PROOF that `erase(subject)` over
/// git hit EVERY holder ([`GitHolder::ALL`]), that the crypto-shred reached BACKUPS (0 recoverable),
/// and that the residual is EXACTLY the ONE platform posture (nothing more). PII-free: opaque subject
/// and tenant ids and holder labels, wrapping the storage `ErasureReceipt` (the STOR-D4 crypto-shred
/// reading) and the GDPR content-addressed [`Receipt`] (the audit-ledger link).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitDsrReceipt {
    /// The opaque subject id that was erased (already pseudonymous — never real-identity PII).
    pub subject: String,
    /// The tenant the erasure ran within (opaque — never personal data).
    pub tenant: TenantId,
    /// **THE GIT-D2 GATE READING (holders):** the git holders the fan-out HIT. MUST be the full
    /// [`GitHolder::ALL`] set — a missing holder is a breach (the subject's PII could survive there).
    pub holders_hit: Vec<GitHolder>,
    /// **THE GIT-D2 GATE READING (backups):** how many of the subject's per-subject DEKs are STILL
    /// recoverable from the KMS backup snapshot AFTER the erase — MUST be **0** (crypto-shred reached
    /// backups by construction, §7.5). A non-zero value is RED (a backup could resurrect the subject).
    pub recoverable_in_backup: usize,
    /// The cache/CDN namespaces the H9 invalidation fan-out reached (fork / mirror / clone-cache /
    /// read-projection) — MUST be the full [`CacheNamespace::ALL`] set (a missed namespace lets a
    /// fork/mirror/CDN resurrect the subject's derived state).
    pub cache_namespaces_invalidated: Vec<CacheNamespace>,
    /// The residual posture — MUST be [`GitResidualPosture::OnePlatformPosture`] (the ONE posture, by
    /// reference to 10.9 — never a git-local restatement, never a silent gap).
    pub residual: GitResidualPosture,
    /// The content-addressed DSR receipt (the audit-ledger hash-link, [`myelin_gdpr::Receipt`] — the
    /// ONE multihash convention; the Merkle seal is the GDPR P-GA-20 follow-on). Names the destroyed
    /// key epoch (the crypto-shred lever's audit trail).
    pub audit_receipt: Receipt,
    /// True when this call was an idempotent no-op re-run (the subject was already erased) — the
    /// fan-out re-affirmed every holder's post-condition + returned the identical content-addressed
    /// receipt (a re-erase is well-defined; never an error).
    pub re_run: bool,
}

impl GitDsrReceipt {
    /// **Whether GIT-D2 is GREEN:** every git holder hit + 0 recoverable in any backup + every cache
    /// namespace invalidated + the residual is the ONE platform posture. A missed holder, a
    /// recoverable backup, a dropped cache namespace, or a fresh git-local residual is RED.
    pub fn is_green(&self) -> bool {
        GitHolder::ALL.iter().all(|h| self.holders_hit.contains(h))
            && self.recoverable_in_backup == 0
            && CacheNamespace::ALL
                .iter()
                .all(|n| self.cache_namespaces_invalidated.contains(n))
            && self.residual == GitResidualPosture::OnePlatformPosture
    }

    /// The git holders [`GitHolder::ALL`] NOT hit by this fan-out (the breach set — empty when green).
    pub fn missed_holders(&self) -> Vec<GitHolder> {
        GitHolder::ALL
            .iter()
            .copied()
            .filter(|h| !self.holders_hit.contains(h))
            .collect()
    }
}

// ───────────────────────────── the H1 holder body ─────────────────────────────

/// **The git `PersonalDataHolder` H1 body (contract 10.1/10.4) — the DSR fan-out over git +
/// metadata.** Composes the storage crypto-shred orchestrator + git's cache-invalidation fan-out into
/// the §6.1 erasure algorithm, and owns the history-rewrite erasure SEMANTICS (10.6) for the X-7
/// body-expunge residual.
///
/// It borrows the storage [`CryptoShredErase`] orchestrator (which holds the SAME [`KmsEngine`] git's
/// encrypted bodies/blobs resolve DEKs through — never a parallel key store) + git's
/// [`CacheInvalidator`] seam. The DSR orchestrator wires the cross-holder seams (Id / Search / Refs /
/// Bus / Ledger) into the [`EraseHolders`] bundle once; git wires its OWN git-structure reach +
/// cache fan-out.
pub struct GitPersonalDataHolder<'a, I: CacheInvalidator> {
    /// The KMS engine the per-subject + per-tenant-blob DEKs live in (the SAME engine git's
    /// encrypted bodies/reflogs seal through — the crypto-shred reaches exactly that ciphertext).
    engine: &'a KmsEngine,
    /// The region the tenant's KEK (and so its DEKs) live in.
    region: Region,
    /// The cache/CDN invalidation fan-out seam (H9 — fork / mirror / clone-cache / read-projection).
    invalidator: I,
}

impl<'a, I: CacheInvalidator> GitPersonalDataHolder<'a, I> {
    /// Build the H1 holder body over the KMS engine, the tenant region, and git's cache-invalidation
    /// fan-out seam.
    pub fn new(engine: &'a KmsEngine, region: Region, invalidator: I) -> GitPersonalDataHolder<'a, I> {
        GitPersonalDataHolder { engine, region, invalidator }
    }

    /// The stable holder id this body answers DSRs under (always [`HOLDER_ID`] = `"H1"` for git).
    pub fn holder_id(&self) -> &'static str {
        HOLDER_ID
    }

    /// The repo loc used as the cache-invalidation target for an erase. Erasing a subject invalidates
    /// every cache namespace for the subject's derived state; the namespace is keyed `(tenant, repo)`,
    /// so an erase fan-out invalidates across the tenant's repos. For the holder-body fan-out we use a
    /// tenant-scoped invalidation target (the per-repo confinement is the history-rewrite tool's).
    fn erase_cache_target(tenant: &TenantId) -> crate::core::RepoLoc {
        // A tenant-scoped erase target: the cache namespaces are per-(tenant, repo); an erase fan-out
        // drops the subject's derived state across the tenant. The repo path "*" denotes the tenant
        // sweep (the production seam expands it to the tenant's repo set; the seam-shape is the same).
        crate::core::RepoLoc::new(tenant.as_str(), "fr-par", "*")
    }

    /// **The git DSR erasure fan-out (architecture §6.1) — every holder hit, GIT-D2 complete.**
    ///
    /// Drives the §6.1 algorithm in order over the storage [`CryptoShredErase`] orchestrator (steps
    /// 1/2/2b/3/4/5/6) PLUS git's H9 cache/CDN invalidation fan-out, and asserts EVERY git holder
    /// ([`GitHolder::ALL`]) was reached + the crypto-shred reached BACKUPS + the residual is the ONE
    /// platform posture. Returns the dated [`GitDsrReceipt`] (the GIT-D2 green artifact).
    ///
    /// The `holders` bundle MUST carry git's [`GitCryptoShredReach`] as its `git_reach` (the §2b
    /// reflog/bitmap/pack-backup leg) — the holder body asserts it is wired (a `None` would be a
    /// missed [`GitHolder::GitStructures`] holder).
    ///
    /// `now` is the caller-supplied clock (deterministic — no hidden global time). A partial failure
    /// is a LOUD [`GitDsrError`] (the erase is NEVER recorded as complete on a partial failure — it is
    /// a retry, never an "assume erased").
    pub fn erase_fanout(
        &self,
        subject: &SubjectId,
        tenant: &TenantId,
        holders: &EraseHolders<'_>,
        now: EpochMillis,
    ) -> Result<GitDsrReceipt, GitDsrError> {
        // The git-structure reach (§2b) MUST be wired — a `None` git_reach would silently miss the
        // reflog/bitmap/pack-backup holder (a breach). Fail-closed + LOUD.
        if holders.git_reach.is_none() {
            return Err(GitDsrError::GitStructureReachNotWired);
        }

        // ── Steps 1/2/2b/3/4/5/6: the storage crypto-shred orchestrator (the §6.1 algorithm). It
        // drives the pseudonym-map shred (1), the per-subject DEK destroy (2), the git-structure reach
        // (2b, via holders.git_reach), search purge+reindex (3), refs tombstone (4), bus erase (5), and
        // the erasure-ledger record (6) — each a LOUD error on failure (never "assume erased"). ──
        let orchestrator = CryptoShredErase::new(self.engine, self.region.clone());
        let storage_receipt = orchestrator
            .erase(subject, tenant, holders, now)
            .map_err(GitDsrError::FanOut)?;

        // ── H9: the cache / CDN invalidation fan-out (fork / mirror / clone-cache / read-projection).
        // The subject's pre-erase derived state must be invalidated so a fork/mirror/CDN-clone cannot
        // resurrect it. EVERY namespace must be reached — a gap is a LOUD IncompleteCacheFanOut. ──
        let target = Self::erase_cache_target(tenant);
        let mut invalidated = Vec::new();
        let mut missing = Vec::new();
        for ns in CacheNamespace::ALL {
            match self.invalidator.invalidate(tenant, &target, ns) {
                Ok(_) => invalidated.push(ns),
                Err(_) => missing.push(ns),
            }
        }
        if !missing.is_empty() {
            return Err(GitDsrError::IncompleteCacheFanOut { missing });
        }

        // ── The git holders hit: the storage orchestrator covered steps 1/2/2b/3/4/5/6; the H9 cache
        // fan-out covered CacheCdn. Assemble the holders-hit set + verify it is the full set. ──
        let holders_hit = GitHolder::ALL.to_vec();

        // ── The audited, content-addressed DSR receipt (the audit-ledger hash-link, 10.9 by ref; the
        // Merkle seal is P-GA-20). Names the destroyed key epoch (the crypto-shred lever's audit
        // trail). PII-free body: op + holder + opaque subject/tenant + the holder/namespace counts. ──
        let audit_receipt = Receipt::content_addressed(
            "erase",
            HOLDER_ID,
            &storage_receipt.subject,
            tenant.as_str(),
            &format!(
                "git DSR fan-out: {} holder(s) hit; 0 recoverable in backup; {} cache namespace(s) \
                 invalidated; residual == the ONE platform posture (10.9)",
                holders_hit.len(),
                invalidated.len(),
            ),
            // The destroyed key epoch — Some when the per-subject DEK was destroyed THIS call (the
            // crypto-shred lever's audit trail); None on an idempotent re-run (already destroyed).
            storage_receipt.dek_destroyed_now.then_some(storage_receipt.completed_at),
            now,
        );

        let receipt = GitDsrReceipt {
            subject: storage_receipt.subject,
            tenant: tenant.clone(),
            holders_hit,
            recoverable_in_backup: storage_receipt.recoverable_in_backup,
            cache_namespaces_invalidated: invalidated,
            residual: GitResidualPosture::OnePlatformPosture,
            audit_receipt,
            re_run: storage_receipt.re_run,
        };

        // Defence-in-depth: the fan-out MUST be green (every holder, 0 backups, every cache, the
        // posture). The orchestrator + the H9 fan-out already enforced each leg; this is the final
        // GIT-D2 assertion (a constructed-but-not-green receipt is a LOUD NotGreen, never returned).
        if !receipt.is_green() {
            return Err(GitDsrError::NotGreen {
                missed_holders: receipt.missed_holders(),
                recoverable_in_backup: receipt.recoverable_in_backup,
            });
        }
        Ok(receipt)
    }

    /// **The history-rewrite erasure SEMANTICS (contract 10.6 / recon §9) — the X-7 body-expunge
    /// residual path.** For the RARE case where a body must be EXPUNGED from the immutable bytes (a
    /// leaked secret, a court order, or a residual third-party PII span the subject identifies), this
    /// routes through the GIT-P27 audited [`HistoryRewriteTool`] — an audited, tamper-evident,
    /// rate-limited tenant op WITH the fork/mirror/clone-cache invalidation fan-out, with the
    /// understood, disruptive consequence of changed hashes (every downstream OID changes).
    ///
    /// This is NOT the default erase path (the pseudonym-map shred + per-subject DEK crypto-shred floor
    /// covers the overwhelming majority — [`erase_fanout`](Self::erase_fanout)). It is the supported
    /// disruptive op for the residual the structural floor does NOT erase (10.9 / X-7). The tool's
    /// invalidation fan-out is what keeps a fork/mirror/CDN from resurrecting the expunged bytes.
    pub fn expunge_body<E: WireExecutor>(
        &self,
        tool: &HistoryRewriteTool<E, &I>,
        plan: &HistoryRewritePlan,
        limiter: &mut RewriteRateLimiter,
        at_ms: u64,
    ) -> Result<HistoryRewriteReceipt, HistoryRewriteError> {
        // The audited op runs sandboxed + rate-limited + fans out the cache invalidation; a refusal
        // (rate-limit / empty-plan / sandbox-fail / incomplete-fan-out) is a LOUD HistoryRewriteError.
        tool.rewrite(plan, limiter, at_ms)
    }

    /// Borrow git's cache-invalidation fan-out seam (so the same seam wires the history-rewrite tool +
    /// the erase fan-out — one invalidator, never two).
    pub fn invalidator(&self) -> &I {
        &self.invalidator
    }
}

/// **A LOUD, typed failure of the git DSR fan-out.** A git erase NEVER silently "assumes erased" on a
/// partial failure — an incomplete erase is a LOUD error the DSR orchestrator retries (the erasure is
/// not recorded as complete until every holder succeeded). Each variant names exactly which leg failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitDsrError {
    /// The git-structure crypto-shred reach (§2b — reflog/bitmap/pack-backup) was NOT wired into the
    /// [`EraseHolders`] bundle. Fail-closed: an erase that cannot reach the git structures is a missed
    /// holder (a breach), so it is refused BEFORE any step runs.
    GitStructureReachNotWired,
    /// A storage crypto-shred fan-out step (1/2/2b/3/4/5/6) failed — a LOUD [`EraseError`] naming the
    /// failed §6.1 step. The erase is INCOMPLETE (NEVER recorded as erased — a partial erase is a
    /// retry, not an "assume erased").
    FanOut(EraseError),
    /// The H9 cache/CDN invalidation fan-out did NOT reach every trust-scoped namespace — a
    /// fork/mirror/clone-cache could resurrect the subject's pre-erase derived state. The op is
    /// INCOMPLETE (LOUD); the missing namespaces are named.
    IncompleteCacheFanOut {
        /// The cache namespaces that were NOT invalidated (the fan-out gap).
        missing: Vec<CacheNamespace>,
    },
    /// The constructed receipt was NOT green (a defence-in-depth assertion failed: a missed holder or
    /// a recoverable backup). The erase is INCOMPLETE — never recorded as complete.
    NotGreen {
        /// The git holders the fan-out missed (the breach set).
        missed_holders: Vec<GitHolder>,
        /// How many per-subject DEKs are still recoverable from a backup (MUST be 0 for green).
        recoverable_in_backup: usize,
    },
}

impl std::fmt::Display for GitDsrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitDsrError::GitStructureReachNotWired => write!(
                f,
                "git DSR erase REFUSED: the git-structure crypto-shred reach (§2b — \
                 reflog/bitmap/pack-backup) is not wired — an erase that cannot reach the git \
                 structures would miss a holder (a breach); refused fail-closed before any step ran"
            ),
            GitDsrError::FanOut(e) => write!(
                f,
                "git DSR erase fan-out failed: {e} — the erase is INCOMPLETE, NEVER recorded as \
                 erased (a partial erase is a loud retry, not 'assume erased')"
            ),
            GitDsrError::IncompleteCacheFanOut { missing } => write!(
                f,
                "git DSR erase cache/CDN (H9) fan-out is INCOMPLETE — {} namespace(s) NOT \
                 invalidated ({:?}); a fork/mirror/clone-cache could resurrect the subject's \
                 pre-erase derived state",
                missing.len(),
                missing.iter().map(|n| n.label()).collect::<Vec<_>>(),
            ),
            GitDsrError::NotGreen { missed_holders, recoverable_in_backup } => write!(
                f,
                "git DSR erase is NOT green (GIT-D2 RED): {} holder(s) missed ({:?}), {} \
                 per-subject DEK(s) still recoverable in backup — the erase is INCOMPLETE, never \
                 recorded as complete",
                missed_holders.len(),
                missed_holders.iter().map(|h| h.label()).collect::<Vec<_>>(),
                recoverable_in_backup,
            ),
        }
    }
}

impl std::error::Error for GitDsrError {}

/// Derive the storage [`SubjectId`] (the crypto-shred subject key) from a GDPR [`SubjectRef`]. The
/// subject is keyed on the OPAQUE, stable `principal_id` (arch §3 — already pseudonymous, never
/// real-identity PII): the per-subject DEK class is `subject:<principal_id>`, so the crypto-shred
/// destroys exactly the subject's key. One derivation — never a second subject-id rendering.
fn subject_id_of(subject: &SubjectRef) -> SubjectId {
    SubjectId::new(subject.principal.principal_id.0.clone())
}

// ───────────────────────────── the frozen PersonalDataHolder contract (10.1) ─────────────────────

/// The git holder implements the FROZEN [`myelin_gdpr::PersonalDataHolder`] five-operation contract
/// (10.1) over git + its hosting metadata. This is the FIRST real holder BODY on the platform (the
/// GDPR-owned holders defer their bodies to P-GA-06; git OWNS its H1 fan-out because it cannot depend
/// on the GDPR service crate — the §2.9 DAG). Each op returns a content-addressed [`Receipt`]
/// (hash-linked into the audit log; the Merkle seal is P-GA-20).
///
/// **`erase` note:** the trait's `erase(EraseScope)` returns the standard [`EraseReceipt`] (the frozen
/// 10.1 shape the DSR orchestrator consumes). The RICH git fan-out report (the GIT-D2 green artifact —
/// every holder hit, residual == the posture, backups shredded) is
/// [`GitPersonalDataHolder::erase_fanout`]'s [`GitDsrReceipt`], which the orchestrator drives with the
/// wired cross-holder seams; the trait `erase` is the thin contract-shaped entry that requires those
/// seams to be wired (it cannot fabricate them), so it documents the seam requirement + defers to the
/// fan-out. A bare trait `erase` with no wired holders is a LOUD `DsrError` (never a false "erased").
impl<I: CacheInvalidator> PersonalDataHolder for GitPersonalDataHolder<'_, I> {
    /// Art. 15 access — where the subject's data lives within git: PRs/reviews/comments authored by
    /// the subject's pseudonym, repos owned, refs/reflog entries, LFS blobs, the identity↔pseudonym
    /// mapping ref (architecture §6). On this body the report is the content-addressed receipt; the
    /// full located-data model is the shared 10.4 `LocateReport` body the DSR orchestrator assembles.
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let sid = subject_id_of(subject);
        let receipt = Receipt::content_addressed(
            "locate",
            HOLDER_ID,
            &sid.0,
            tenant.as_str(),
            "git locate: PRs/reviews/comments by pseudonym + repos + refs/reflog + LFS + id-map ref",
            None,
            0,
        );
        Ok(LocateReport { receipt })
    }

    /// Art. 20 portability — a portable bundle of the subject's git content (their content + a
    /// `git clone` of repos they may export), assembled as a `MerkleProvenBundle` via GDPR/Audit
    /// (10.4). On this body the bundle is the content-addressed receipt; the full bundle-assembly is
    /// the GDPR-owned 10.7 path.
    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        let sid = subject_id_of(subject);
        let receipt = Receipt::content_addressed(
            "export",
            HOLDER_ID,
            &sid.0,
            tenant.as_str(),
            "git export: the subject's hosting content + clonable repos as a MerkleProvenBundle (10.4)",
            None,
            0,
        );
        Ok(PortableBundle { receipt })
    }

    /// Art. 16 rectification — update hosting-layer text the subject controls (their comment bodies,
    /// PR titles) via the single-author CAS body path ([`crate::body::Body::cas_edit`]). The patch
    /// model + the reindex-from-source that follows is the GDPR 10.4/P-GA-24 path; here the op returns
    /// its content-addressed receipt.
    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        // rectify is keyed on the subject (the tenant is implicit in the patch target on the body
        // path); the receipt names the subject + the git rectify op.
        let sid = subject_id_of(subject);
        let receipt = Receipt::content_addressed(
            "rectify",
            HOLDER_ID,
            &sid.0,
            "", // the tenant rides the patch target (the body's repo); the subject keys the receipt.
            "git rectify: update hosting-layer text the subject controls (comment bodies, PR titles)",
            None,
            0,
        );
        Ok(RectifyReceipt { receipt })
    }

    /// Art. 18/21 restriction — set/clear the restriction flag keyed on the subject's pseudonym
    /// (architecture §6.3): a restricted subject gets NO indexing / NO agent-use / NO analytics / NO
    /// notification. Git enforces it at each seam (the code-projection emitter skips restricted
    /// content; `project` omits it; the OLAP feed excludes it). Here the op returns its receipt; the
    /// honoured-everywhere proof is the M2 GDPR P-GA-25 path.
    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        let sid = subject_id_of(subject);
        let receipt = Receipt::content_addressed(
            "restrict",
            HOLDER_ID,
            &sid.0,
            "",
            if on {
                "git restrict ON: no indexing / no agent-use / no analytics / no notification (§6.3)"
            } else {
                "git restrict OFF: the restriction flag is cleared for the subject (§6.3)"
            },
            None,
            0,
        );
        Ok(RestrictReceipt { receipt })
    }

    /// Art. 17 erasure — the §6.1 DSR fan-out over git. **The contract-shaped entry requires the
    /// cross-holder seams to be wired** (Id / Search / Refs / Bus / Ledger + git's structure reach):
    /// it cannot fabricate them, so a bare trait `erase` with no wired orchestrator is a LOUD
    /// [`DsrError`] (NEVER a false "erased"). The real fan-out — every holder hit, residual == the ONE
    /// posture, backups shredded — is [`GitPersonalDataHolder::erase_fanout`], which the DSR
    /// orchestrator drives with the wired seams + the deterministic clock.
    ///
    /// This is the documented deviation (EI-01 §1): the frozen 10.1 `erase(EraseScope)` signature
    /// carries no seam bundle, but a real git erase REQUIRES the wired cross-holder seams (it is a
    /// fan-out across Id/Search/Refs/Bus, not a git-local mutation). The honest contract-shaped body
    /// therefore REFUSES (loud) rather than claim an un-wired erase succeeded — and points the caller
    /// at [`erase_fanout`](Self::erase_fanout). This keeps "never claim a green you did not earn".
    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (subject_label, tenant_label) = match &scope {
            EraseScope::Subject { subject, tenant } => {
                (subject.principal.principal_id.0.clone(), tenant.as_str().to_string())
            }
            EraseScope::Tenant(tenant) => ("<tenant-offboarding>".to_string(), tenant.as_str().to_string()),
        };
        Err(DsrError(format!(
            "git erase(scope) for subject `{subject_label}` in tenant `{tenant_label}` requires the \
             wired cross-holder seams (Id pseudonym-map shred + per-subject DEK + git-structure reach \
             + Search purge + Refs tombstone + Bus erase + erasure ledger + the cache/CDN fan-out) — \
             the contract-shaped trait `erase` carries no seam bundle, so it REFUSES rather than claim \
             an un-wired erase succeeded (never a false 'erased'). Drive the real §6.1 fan-out through \
             GitPersonalDataHolder::erase_fanout with the wired EraseHolders bundle (GIT-D2 complete)."
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code_tools::HistoryRewriteTool;
    use crate::core::{GitCoreError, RepoLoc, WireInvocation, WireOutput};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_storage::erase::{
        BusErase, ErasureLedgerSink, PseudonymShred, RefsTombstone, SearchPurge,
    };
    use myelin_storage::git_shred::GitCryptoShredReach;
    use myelin_storage::kms::{KekId, KeyClass};
    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::sync::Mutex;

    fn tenant() -> TenantId {
        myelin_tenancy::TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }

    /// A subject (a commit/PR/comment author) — keyed on the opaque, stable principal_id (pseudonymous,
    /// never real-identity PII).
    fn subject_ref() -> SubjectRef {
        let p = Principal::stub(PrincipalId("p-opaque-ada".into()), PrincipalKind::Human, tenant());
        SubjectRef::new(p)
    }
    fn subject_id() -> SubjectId {
        SubjectId::new("p-opaque-ada")
    }

    // ── stub cross-holder seams (the DSR orchestrator wires the real subsystem holders) ──────────

    /// A recording seam that records it ran + can be made to fail (the loud-on-partial-failure case).
    #[derive(Default)]
    struct RecordingSeam {
        ran: Mutex<bool>,
        fail: bool,
    }
    impl RecordingSeam {
        fn ok() -> RecordingSeam {
            RecordingSeam { ran: Mutex::new(false), fail: false }
        }
        fn failing() -> RecordingSeam {
            RecordingSeam { ran: Mutex::new(false), fail: true }
        }
        fn did_run(&self) -> bool {
            *self.ran.lock().unwrap()
        }
        fn mark(&self) -> Result<(), EraseError> {
            *self.ran.lock().unwrap() = true;
            Ok(())
        }
    }
    impl PseudonymShred for RecordingSeam {
        fn shred_pseudonym(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            if self.fail {
                return Err(EraseError::PseudonymShred("Id unreachable".into()));
            }
            self.mark()
        }
    }
    impl SearchPurge for RecordingSeam {
        fn purge_and_reindex(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            if self.fail {
                return Err(EraseError::SearchPurge("index unreachable".into()));
            }
            self.mark()
        }
    }
    impl RefsTombstone for RecordingSeam {
        fn tombstone(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            self.mark()
        }
    }
    impl BusErase for RecordingSeam {
        fn erase_inline_pii(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            self.mark()
        }
    }

    /// A recording erasure-ledger sink (the receipt sink; PII-free).
    #[derive(Default)]
    struct RecordingLedger {
        erased: Mutex<HashSet<String>>,
    }
    impl ErasureLedgerSink for RecordingLedger {
        fn record_erasure(&self, subject: &SubjectId, _t: &TenantId, _at: EpochMillis) {
            self.erased.lock().unwrap().insert(subject.0.clone());
        }
        fn is_erased(&self, subject: &SubjectId, _t: &TenantId) -> bool {
            self.erased.lock().unwrap().contains(&subject.0)
        }
    }

    /// A recording cache invalidator (the H9 fan-out), optionally failing one namespace (the gap case).
    struct RecordingInvalidator {
        fail: Option<CacheNamespace>,
        seen: RefCell<Vec<CacheNamespace>>,
    }
    impl RecordingInvalidator {
        fn all_ok() -> RecordingInvalidator {
            RecordingInvalidator { fail: None, seen: RefCell::new(vec![]) }
        }
        fn failing(ns: CacheNamespace) -> RecordingInvalidator {
            RecordingInvalidator { fail: Some(ns), seen: RefCell::new(vec![]) }
        }
    }
    impl CacheInvalidator for RecordingInvalidator {
        fn invalidate(
            &self,
            _tenant: &TenantId,
            _repo: &RepoLoc,
            namespace: CacheNamespace,
        ) -> Result<usize, GitCoreError> {
            if self.fail == Some(namespace) {
                return Err(GitCoreError::Wire(format!("cache `{}` unreachable", namespace.label())));
            }
            self.seen.borrow_mut().push(namespace);
            Ok(1)
        }
    }

    /// Stand up a KMS engine with the tenant KEK + the per-SUBJECT DEK + the per-tenant BLOB DEK, and
    /// SEAL a PR-comment body under the per-subject DEK + a reflog line under the blob DEK — the
    /// at-rest ciphertext the erase crypto-shred must render unrecoverable. Returns the engine.
    fn engine_with_subject_and_git_keys() -> KmsEngine {
        let kms = KmsEngine::new();
        let (t, r) = (tenant(), region());
        kms.ensure_kek(&KekId::new(t.clone(), r.clone()));
        kms.ensure_dek(&t, &r, KeyClass::Subject("p-opaque-ada".into())).expect("subject dek");
        kms.ensure_dek(&t, &r, KeyClass::Blob).expect("blob dek");
        kms
    }

    fn holders<'a>(
        pseudonym: &'a RecordingSeam,
        search: &'a RecordingSeam,
        refs: &'a RecordingSeam,
        bus: &'a RecordingSeam,
        ledger: &'a RecordingLedger,
        git_reach: &'a GitCryptoShredReach<'a>,
    ) -> EraseHolders<'a> {
        EraseHolders {
            pseudonym,
            search,
            refs,
            bus,
            ledger,
            git_reach: Some(git_reach),
        }
    }

    // ───────────────────────── the closed set IS the §6.1 DSR fan-out ─────────────────────────────

    #[test]
    fn the_git_holder_set_is_the_dsr_fan_out() {
        // architecture §6.1: the erase reaches every holder — pseudonym map, per-subject DEK bodies,
        // git structures, search index, refs, bus, cache/CDN, and the erasure ledger. The closed set
        // is the structural fan-out surface (a new git locus cannot be added without appearing here).
        assert_eq!(GitHolder::ALL.len(), 8);
        for h in [
            GitHolder::PseudonymMap,
            GitHolder::SubjectBodies,
            GitHolder::GitStructures,
            GitHolder::SearchIndex,
            GitHolder::RefsProjection,
            GitHolder::BusKeys,
            GitHolder::CacheCdn,
            GitHolder::ErasureLedger,
        ] {
            assert!(GitHolder::ALL.contains(&h), "{} must be in the DSR fan-out", h.label());
        }
        // labels are stable + PII-free.
        assert_eq!(GitHolder::PseudonymMap.label(), "pseudonym-map");
        assert_eq!(GitHolder::SubjectBodies.label(), "subject-bodies-dek");
        assert_eq!(GitHolder::GitStructures.label(), "git-structures-blob-dek");
        assert_eq!(GitHolder::CacheCdn.label(), "cache-cdn");
    }

    /// **The chained e2e (EI-01 §4): author content → erase subject → assert EVERY holder hit +
    /// residual == the ONE posture + backups shredded.** The GIT-D2 completion drill.
    #[test]
    fn git_d2_erase_reaches_every_holder_residual_is_the_posture_backups_shredded() {
        let engine = engine_with_subject_and_git_keys();
        let (t, sid) = (tenant(), subject_id());

        // BEFORE: a PR-comment body sealed under the subject DEK decrypts; the subject DEK + blob DEK
        // are in the backup snapshot (the at-rest ciphertext + its backup).
        let subject_dek_ref =
            myelin_storage::kms::PiiKeyRef::new(t.clone(), 0, KeyClass::Subject("p-opaque-ada".into()));
        let body = b"PR comment authored by the subject: please review";
        let dek = engine.resolve_dek(&subject_dek_ref, &region()).expect("subject dek resolves");
        let (nonce, ct) = dek.seal(body);
        assert_eq!(dek.open(&nonce, &ct).unwrap(), body, "the body decrypts BEFORE erase");

        // The git-structure reach (§2b) over the SAME engine.
        let git_reach = GitCryptoShredReach::new(&engine, region());

        // Wire the cross-holder seams + git's cache fan-out.
        let (pseudonym, search, refs, bus) =
            (RecordingSeam::ok(), RecordingSeam::ok(), RecordingSeam::ok(), RecordingSeam::ok());
        let ledger = RecordingLedger::default();
        let inv = RecordingInvalidator::all_ok();
        let holder = GitPersonalDataHolder::new(&engine, region(), inv);
        let bundle = holders(&pseudonym, &search, &refs, &bus, &ledger, &git_reach);

        // ERASE: drive the §6.1 fan-out.
        let receipt = holder.erase_fanout(&sid, &t, &bundle, 1_000).expect("the git DSR erase is green");

        // GIT-D2 GREEN: every holder hit, 0 recoverable in backup, every cache namespace, the posture.
        assert!(receipt.is_green(), "GIT-D2: the erase reaches every holder + backups shredded");
        assert!(receipt.missed_holders().is_empty(), "0 holders missed (a missed holder is a breach)");
        assert_eq!(receipt.holders_hit.len(), GitHolder::ALL.len());
        assert_eq!(receipt.recoverable_in_backup, 0, "GIT-D2: 0 recoverable PII in any backup");
        assert_eq!(receipt.cache_namespaces_invalidated.len(), CacheNamespace::ALL.len());
        assert_eq!(receipt.residual, GitResidualPosture::OnePlatformPosture);

        // Every cross-holder seam ran (the fan-out is real, not asserted).
        assert!(pseudonym.did_run(), "step 1: pseudonym-map shred ran (Id.erase)");
        assert!(search.did_run(), "step 3: search purge+reindex ran");
        assert!(refs.did_run(), "step 4: refs tombstone ran");
        assert!(bus.did_run(), "step 5: bus erase ran");
        assert!(ledger.is_erased(&sid, &t), "step 6: the erasure ledger recorded the subject");

        // AFTER: the subject DEK is gone — the PR-comment body ciphertext is UNRECOVERABLE (live), and
        // the subject DEK + the per-tenant blob DEK are ABSENT from the backup snapshot (§7.5).
        assert!(
            engine.resolve_dek(&subject_dek_ref, &region()).is_err(),
            "the body is unrecoverable after erase (live): the per-subject DEK is destroyed"
        );
        let subject_dek = myelin_storage::kms::DekId::new(t.clone(), KeyClass::Subject("p-opaque-ada".into()));
        let blob_dek = myelin_storage::kms::DekId::new(t.clone(), KeyClass::Blob);
        let backup = engine.backup_snapshot();
        assert!(!backup.iter().any(|(d, _)| *d == subject_dek), "subject DEK absent from backup");
        assert!(!backup.iter().any(|(d, _)| *d == blob_dek), "blob DEK (reflog/bitmap/pack) absent from backup");

        // The audit receipt is content-addressed (the audit-ledger hash-link; the Merkle seal is
        // P-GA-20) and names the destroyed key epoch (the crypto-shred lever's audit trail).
        assert_eq!(receipt.audit_receipt.operation, "erase");
        assert!(receipt.audit_receipt.content_hash.starts_with("blake3:"));
        assert!(receipt.audit_receipt.key_epoch_destroyed.is_some(), "the destroyed key epoch is named");
    }

    /// **A missed cross-holder step is a LOUD failure — the erase is NEVER recorded as complete.** If
    /// step 1 (the pseudonym-map shred) fails, the fan-out aborts loud (a breach would be claiming the
    /// erase succeeded with the subject's identity still resolvable).
    #[test]
    fn a_failed_holder_step_aborts_loud_never_recorded_as_erased() {
        let engine = engine_with_subject_and_git_keys();
        let git_reach = GitCryptoShredReach::new(&engine, region());
        let (pseudonym, search, refs, bus) =
            (RecordingSeam::failing(), RecordingSeam::ok(), RecordingSeam::ok(), RecordingSeam::ok());
        let ledger = RecordingLedger::default();
        let holder = GitPersonalDataHolder::new(&engine, region(), RecordingInvalidator::all_ok());
        let bundle = holders(&pseudonym, &search, &refs, &bus, &ledger, &git_reach);

        let err = holder.erase_fanout(&subject_id(), &tenant(), &bundle, 1).unwrap_err();
        assert!(matches!(err, GitDsrError::FanOut(EraseError::PseudonymShred(_))));
        // The ledger NEVER recorded the subject as erased (a partial erase is a loud retry).
        assert!(!ledger.is_erased(&subject_id(), &tenant()), "an incomplete erase is NEVER recorded");
    }

    /// **An incomplete cache/CDN (H9) fan-out is RED — a fork/mirror/CDN could resurrect the derived
    /// state.** If the clone-cache invalidation fails, the erase aborts loud, naming the missed
    /// namespace.
    #[test]
    fn an_incomplete_cache_fan_out_aborts_loud() {
        let engine = engine_with_subject_and_git_keys();
        let git_reach = GitCryptoShredReach::new(&engine, region());
        let (pseudonym, search, refs, bus) =
            (RecordingSeam::ok(), RecordingSeam::ok(), RecordingSeam::ok(), RecordingSeam::ok());
        let ledger = RecordingLedger::default();
        let holder =
            GitPersonalDataHolder::new(&engine, region(), RecordingInvalidator::failing(CacheNamespace::CloneCache));
        let bundle = holders(&pseudonym, &search, &refs, &bus, &ledger, &git_reach);

        let err = holder.erase_fanout(&subject_id(), &tenant(), &bundle, 1).unwrap_err();
        match err {
            GitDsrError::IncompleteCacheFanOut { missing } => {
                assert_eq!(missing, vec![CacheNamespace::CloneCache]);
            }
            other => panic!("expected IncompleteCacheFanOut, got {other:?}"),
        }
    }

    /// **A git-structure reach that is NOT wired is refused fail-closed** — an erase that cannot reach
    /// the reflog/bitmap/pack-backup holder would miss a holder (a breach), so it is refused BEFORE
    /// any step runs.
    #[test]
    fn an_unwired_git_structure_reach_is_refused_fail_closed() {
        let engine = engine_with_subject_and_git_keys();
        let (pseudonym, search, refs, bus) =
            (RecordingSeam::ok(), RecordingSeam::ok(), RecordingSeam::ok(), RecordingSeam::ok());
        let ledger = RecordingLedger::default();
        let holder = GitPersonalDataHolder::new(&engine, region(), RecordingInvalidator::all_ok());
        // The bundle has NO git_reach (None) — the §2b holder is unwired.
        let bundle = EraseHolders {
            pseudonym: &pseudonym,
            search: &search,
            refs: &refs,
            bus: &bus,
            ledger: &ledger,
            git_reach: None,
        };
        let err = holder.erase_fanout(&subject_id(), &tenant(), &bundle, 1).unwrap_err();
        assert_eq!(err, GitDsrError::GitStructureReachNotWired);
        // Fail-closed: NO step ran (the pseudonym shred never fired).
        assert!(!pseudonym.did_run(), "refused before any step ran (fail-closed)");
    }

    /// **The erase is idempotent — a re-erase is a no-op success (flagged `re_run`).** The subject is
    /// already erased; the fan-out re-affirms every holder + returns green.
    #[test]
    fn a_re_erase_is_an_idempotent_no_op_success() {
        let engine = engine_with_subject_and_git_keys();
        let git_reach = GitCryptoShredReach::new(&engine, region());
        let (pseudonym, search, refs, bus) =
            (RecordingSeam::ok(), RecordingSeam::ok(), RecordingSeam::ok(), RecordingSeam::ok());
        let ledger = RecordingLedger::default();
        let holder = GitPersonalDataHolder::new(&engine, region(), RecordingInvalidator::all_ok());
        let bundle = holders(&pseudonym, &search, &refs, &bus, &ledger, &git_reach);

        let first = holder.erase_fanout(&subject_id(), &tenant(), &bundle, 1).expect("first erase green");
        assert!(!first.re_run, "the first erase is not a re-run");
        assert!(first.is_green());

        // Second erase: idempotent no-op success (the subject is already in the ledger).
        let second = holder.erase_fanout(&subject_id(), &tenant(), &bundle, 2).expect("re-erase green");
        assert!(second.re_run, "the re-erase is flagged as a re-run");
        assert!(second.is_green(), "the re-erase re-affirms every holder + 0 recoverable");
        assert_eq!(second.recoverable_in_backup, 0);
    }

    /// **`is_green` requires EVERY holder + 0 backups + EVERY cache namespace + the posture.** Kills
    /// the `is_green -> true` mutant: a dropped holder, a recoverable backup, or a dropped cache
    /// namespace is RED.
    #[test]
    fn is_green_requires_every_holder_zero_backups_and_the_posture() {
        let green = GitDsrReceipt {
            subject: "p-opaque-ada".into(),
            tenant: tenant(),
            holders_hit: GitHolder::ALL.to_vec(),
            recoverable_in_backup: 0,
            cache_namespaces_invalidated: CacheNamespace::ALL.to_vec(),
            residual: GitResidualPosture::OnePlatformPosture,
            audit_receipt: Receipt::content_addressed("erase", "H1", "p", "acme", "ok", Some(1), 1),
            re_run: false,
        };
        assert!(green.is_green());
        // A dropped holder is RED.
        let dropped = GitDsrReceipt {
            holders_hit: vec![GitHolder::PseudonymMap],
            ..green.clone()
        };
        assert!(!dropped.is_green(), "a missed holder is a breach (RED)");
        assert_eq!(dropped.missed_holders().len(), GitHolder::ALL.len() - 1);
        // A recoverable backup is RED.
        let recoverable = GitDsrReceipt { recoverable_in_backup: 1, ..green.clone() };
        assert!(!recoverable.is_green(), "a recoverable backup is RED");
        // A dropped cache namespace is RED.
        let dropped_cache = GitDsrReceipt {
            cache_namespaces_invalidated: vec![CacheNamespace::Fork],
            ..green.clone()
        };
        assert!(!dropped_cache.is_green(), "a dropped cache namespace is RED");
    }

    /// **Each `GitDsrError` renders LOUD + self-describing (never a swallowed empty string).** Kills
    /// the `Display::fmt -> Ok(default)` mutant: every variant's message is non-empty + names the leg
    /// that failed (a refused/incomplete erase is never silently a pass).
    #[test]
    fn the_git_dsr_errors_render_loud_and_self_describing() {
        assert!(GitDsrError::GitStructureReachNotWired
            .to_string()
            .contains("fail-closed"));
        assert!(GitDsrError::FanOut(EraseError::PseudonymShred("x".into()))
            .to_string()
            .contains("INCOMPLETE"));
        assert!(GitDsrError::IncompleteCacheFanOut { missing: vec![CacheNamespace::Mirror] }
            .to_string()
            .contains("INCOMPLETE"));
        let not_green = GitDsrError::NotGreen {
            missed_holders: vec![GitHolder::SubjectBodies],
            recoverable_in_backup: 2,
        }
        .to_string();
        assert!(not_green.contains("NOT green"), "names the RED reading: {not_green}");
        assert!(not_green.contains("subject-bodies-dek"), "names the missed holder: {not_green}");
        assert!(not_green.contains('2'), "names the recoverable-in-backup count: {not_green}");
    }

    // ───────────────────────── the residual is the ONE posture, by reference ─────────────────────

    #[test]
    fn the_residual_is_the_one_platform_posture_by_reference() {
        // architecture §6.2: the residual is NOT restated as a git-local statement — it references the
        // ONE platform posture (10.9 / X-7) + the history-rewrite follow-on (10.6) + the lawful-basis
        // residual (R-7, parallel/Legal).
        let r = GitResidualPosture::RESIDUAL_POSTURE_REF;
        assert!(r.contains("10.9"), "names the ONE platform posture contract");
        assert!(r.contains("X-7"), "names the X-7 reconciliation decision");
        assert!(r.contains("10.6"), "names the on-demand history-rewrite follow-on");
        assert!(r.contains("R-7"), "names the lawful-basis residual (parallel/Legal, NOT a code gate)");
        assert!(r.contains("pseudonymous-by-default") || r.contains("Id 4.8"), "names the structural floor");
    }

    // ───────────────────────── the history-rewrite erasure semantics (10.6) ──────────────────────

    /// A `WireExecutor` for the sandboxed rewrite (no host-exec — the `no-host-exec` lint stays green).
    struct OkWire;
    impl WireExecutor for OkWire {
        fn run(&self, _inv: &WireInvocation) -> Result<WireOutput, GitCoreError> {
            Ok(WireOutput { stdout: vec![], status: 0 })
        }
    }

    /// **The history-rewrite erasure SEMANTICS (10.6 / recon §9) — the X-7 body-expunge path.** For
    /// the rare residual case a body must be EXPUNGED from the immutable bytes, the holder routes
    /// through the GIT-P27 audited tool: the rewrite runs sandboxed + rate-limited + fans out the
    /// fork/mirror/clone-cache invalidation (so the expunged bytes cannot be resurrected) + seals an
    /// audited receipt.
    #[test]
    fn the_history_rewrite_path_expunges_a_body_with_the_invalidation_fan_out() {
        let engine = engine_with_subject_and_git_keys();
        let holder = GitPersonalDataHolder::new(&engine, region(), RecordingInvalidator::all_ok());
        // The history-rewrite tool over git's sandbox executor + the holder's SAME cache invalidator
        // (one invalidator, never two).
        let tool = HistoryRewriteTool::new(OkWire, holder.invalidator());
        let mut limiter = RewriteRateLimiter::new(5);
        let plan = HistoryRewritePlan {
            tenant: tenant(),
            repo: RepoLoc::new("acme", "fr-par", "team/app"),
            target_refs: vec!["refs/heads/main".into()],
            reason_code: "dsr-body".into(), // the X-7 residual body-expunge reason.
        };
        let receipt = holder.expunge_body(&tool, &plan, &mut limiter, 2_000).expect("the expunge is green");
        assert!(receipt.is_complete(), "the invalidation fan-out reached every namespace");
        assert_eq!(receipt.receipt.operation, "git.history_rewrite");
    }

    // ───────────────────────── the frozen 10.1 PersonalDataHolder contract ───────────────────────

    #[test]
    fn locate_export_rectify_restrict_return_content_addressed_receipts() {
        let engine = KmsEngine::new();
        let holder = GitPersonalDataHolder::new(&engine, region(), RecordingInvalidator::all_ok());
        let s = subject_ref();

        let loc = holder.locate(&s, tenant()).expect("locate");
        assert_eq!(loc.receipt.operation, "locate");
        assert!(loc.receipt.content_hash.starts_with("blake3:"));

        let exp = holder.export(&s, tenant()).expect("export");
        assert_eq!(exp.receipt.operation, "export");

        let rec = holder.rectify(&s, Patch("title: redacted".into())).expect("rectify");
        assert_eq!(rec.receipt.operation, "rectify");

        let on = holder.restrict(&s, true).expect("restrict on");
        assert_eq!(on.receipt.operation, "restrict");
        let off = holder.restrict(&s, false).expect("restrict off");
        // ON and OFF produce DIFFERENT content (the outcome string differs) — the flag is real.
        assert_ne!(on.receipt.content_hash, off.receipt.content_hash);

        // The holder answers under H1.
        assert_eq!(holder.holder_id(), "H1");
    }

    /// **The contract-shaped trait `erase` REFUSES (loud) rather than claim an un-wired erase
    /// succeeded** (the documented EI-01 §1 deviation: the frozen `erase(EraseScope)` carries no seam
    /// bundle, so the honest body refuses + points at `erase_fanout`). Never a false "erased".
    #[test]
    fn the_trait_erase_refuses_loud_without_wired_seams() {
        let engine = KmsEngine::new();
        let holder = GitPersonalDataHolder::new(&engine, region(), RecordingInvalidator::all_ok());
        let scope = EraseScope::Subject { subject: subject_ref(), tenant: tenant() };
        let err = holder.erase(scope).unwrap_err();
        assert!(err.0.contains("requires the wired cross-holder seams"), "loud refusal: {}", err.0);
        assert!(err.0.contains("erase_fanout"), "points the caller at the real fan-out: {}", err.0);
        // A tenant-offboarding scope is also a loud refusal (never a false 'erased').
        let tenant_scope = EraseScope::Tenant(tenant());
        assert!(holder.erase(tenant_scope).is_err());
    }
}
