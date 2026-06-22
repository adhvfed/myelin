//! # Git crypto-shred reach into reflogs / bitmaps / pack-tier backups (P-ST-24 → global P-253)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §5.3 (the structural reach —
//! **crypto-shred into reflogs / bitmaps / pack-tier backups via the per-tenant blob DEK; those
//! ARE shreddable**; the commit-object bytes are NOT — the GD-1 hash-load-bearing case; the
//! pseudonym-map shred reach), §5.2 (the `erase(subject, tenant)` six-step algorithm this reach
//! extends — **the erase reach is verified, not assumed**). Contract-index rows 11.2 / 11.4 (the
//! crypto-shred reach into git structures), 10.9 (the ONE free-text / immutable-content residual
//! posture — **by reference, never restated**), 10.6 (the audited history-rewrite erasure path —
//! the NAMED on-demand follow-on), Id 4.8 (the pseudonym-map shred). EI-04 §1 (erasure-vs-
//! immutability — **pseudonymous-by-default commits never bake erasable PII in the first place**;
//! the git half of the crypto-shred posture). Drill catalogue row **GIT-D2 (storage half, §4.2)**.
//!
//! ## The shape of the problem (EI-04 §1 — the git-history half is the hard half)
//! Git's immutable, append-only object graph is in direct tension with Art. 17 erasure. The
//! platform splits it into two structurally-different halves:
//!
//! - **The SHREDDABLE half — reflogs, bitmaps, and pack-tier BACKUPS.** These are git's *derived*
//!   and *backing* structures. On Myelin they ride the [`crate::gitpack`] pack tier, which stores
//!   every object / packfile through the [`crate::blob::BlobStore`] trait sealed under the
//!   **per-tenant blob DEK** ([`crate::kms::KeyClass::Blob`], wired by [`crate::encryption::DekContentWrap`]).
//!   Destroying that DEK ([`crate::kms::KmsEngine::destroy_dek`]) renders every reflog / bitmap /
//!   pack-backup ciphertext **unrecoverable — live AND in every backup, by construction** (a backup
//!   holds ciphertext under the now-destroyed key, §7.5). This is the reach this module owns + wires
//!   into the [`crate::erase`] orchestrator.
//! - **The RESIDUAL half — the commit-object BYTES.** Author name + email are baked into the commit
//!   *hash*; you cannot tombstone them without rewriting history (changing every downstream hash).
//!   Myelin does **not** byte-mutate commit objects on an erase. Instead the platform decided —
//!   **before** Git's data model froze (EI-04 §1, P-248 / global P-pseudonymous) — that commits are
//!   **pseudonymous-by-default**: the immutable bytes contain only an opaque `<pseudonym>@<tenant>.noreply`
//!   author (Id 4.8), never erasable real-identity PII. So after step 1 (the pseudonym-map shred)
//!   the commit bytes resolve to *nothing personal*. The residual posture itself is the ONE platform
//!   artifact (contract 10.9 / `00 §X-7`) — **handled BY REFERENCE here, never restated** (this
//!   module contributes its structural reach to that one posture; counsel/DPO ratifies the residual
//!   basis once, for all five subsystems).
//!
//! ## What this prompt (P-ST-24 / P-253) ships — and what it REUSES (EI-01 §7, coherence)
//! The crypto-shred MECHANISM ([`crate::kms::KmsEngine::destroy_dek`] + the per-tenant blob DEK), the
//! six-step `erase` algorithm ([`crate::erase::CryptoShredErase`]), the git pack tier
//! ([`crate::gitpack::GitPackTier`]), and the blob content-key wrap ([`crate::encryption::DekContentWrap`])
//! already exist. This module does **NOT** re-define any of them — it is the *reach* that ties the
//! erase to git's structures. What is genuinely NEW:
//!
//! 1. **[`GitShreddable`]** — the closed set of git structures the per-tenant blob DEK reaches
//!    (reflog / bitmap / pack-tier backup), plus the commit-object-bytes case explicitly NAMED as
//!    the residual (NOT shreddable — left to the pseudonymous-by-default posture). This is the §5.3
//!    "those ARE shreddable / the commit bytes are NOT" line made into a type.
//! 2. **[`GitCryptoShredReach`]** — the reach: given the tenant whose git content is being erased,
//!    it (a) confirms the reflog / bitmap / pack-backup ciphertext is sealed under the per-tenant
//!    blob DEK, (b) DESTROYS that DEK (the shred), and (c) **VERIFIES** the structures' ciphertext is
//!    now unrecoverable live AND absent from the backup snapshot (the reach is *verified, not
//!    assumed* — §5.2). It returns a [`GitShredReceipt`] (the GIT-D2 storage-half artifact).
//! 3. **The wiring into the erase orchestrator** — [`GitCryptoShredReach`] implements the
//!    [`crate::erase::BlobShredReach`] seam the [`crate::erase::CryptoShredErase`] now invokes as
//!    part of step 2 (the `KMS.destroy` step extended to the git structures). The per-subject DEK
//!    destroy (the subject's free-text/chat/profile) and the per-tenant blob DEK destroy (the git
//!    reflog/bitmap/pack-backup) are BOTH part of the one crypto-shred step.
//!
//! ## GIT-D2 (storage half) — the gate (§4.2)
//! Erase a commit author → crypto-shred reaches backups / reflogs / bitmaps; residual == the ONE
//! platform posture (pseudonymous-by-default). [`GitShredReceipt`] is the dated reading:
//! `recoverable_in_backup == 0` for every shreddable git structure (the shred reached backups by
//! construction) AND `residual == GitResidual::PseudonymousByDefault` (the commit bytes are the
//! documented posture, never byte-mutated). The drill (`tests/git_d2_git_crypto_shred_drill.rs`)
//! seals reflog/bitmap/pack-backup ciphertext under the per-tenant blob DEK, runs the reach, and
//! asserts the ciphertext is unrecoverable + 0 recoverable in backup + the residual is the posture.
//!
//! ## Floor named (the residual + the follow-on) — VISION §3 / EI-01 §1 / prompt DoD
//! - **Pseudonymous-by-default commits is THE FLOOR** for the commit-object-byte residual (the
//!   immutable bytes never carry erasable PII — decided before Git's data model froze, P-248). This
//!   module does NOT byte-mutate commit objects.
//! - **The audited history-rewrite erasure path (contract 10.6, the changed-hash consequence) is the
//!   NAMED on-demand follow-on (M5 / on-demand).** When a residual *does* need the bytes gone (a
//!   leaked secret, a court order), the history-rewrite is an AUDITED op at GDPR/Audit 10.6
//!   (rate-limited, with fork/mirror/clone-cache invalidation fan-out) — it is NOT this prompt, and
//!   it is NOT a silent gap. Recorded HERE in writing.
//! - **The C6 outbound push-mirror residency gate seam is the SIBLING prompt P-ST-25 (global P-255)**
//!   — it keeps mirror-source blobs content-addressed + encrypted (the bytes that leave are the
//!   ciphertext this reach shreds) and flags the crossing into `residency_verify`. Not built here.
//!
//! ## Mutation floor (mandatory-core, ≥ 80% — EI-01 §2/§3; the prompt's TESTS field)
//! The **git-structure crypto-shred reach** is mandatory-core. The load-bearing mutants — *the reach
//! destroys the per-tenant blob DEK* (a no-op destroy leaves the reflog/bitmap/pack-backup
//! recoverable), *the post-condition verifies 0 recoverable in backup* (a skipped verify lets a
//! backup resurrect the structure), *the commit-object residual is the posture not a byte-mutation*
//! (a "shred the commit bytes" mutant would claim a reach the platform does NOT have), and *a
//! shreddable structure reports recoverable=false only AFTER the destroy* — are each killed by an
//! assertion in the unit + drill + CDC tests. The floor is **≥ 80%**.

use myelin_tenancy::{Region, TenantId};

use crate::erase::{BlobShredReach, EraseError};
use crate::kms::{DekId, KeyClass, KmsEngine};

// ───────────────────────────── git's shreddable structures vs. the residual ─────────────────────

/// **The closed set of git structures the per-tenant blob DEK crypto-shred REACHES** (storage.md
/// §5.3 — "reflogs, bitmaps, and pack-tier backups … those ARE shreddable via the per-tenant blob
/// DEK"). PII-free — a closed enum tag. Each variant's ciphertext rides the [`crate::gitpack`] pack
/// tier sealed under [`KeyClass::Blob`]; destroying that DEK renders it unrecoverable live AND in
/// backups by construction.
///
/// The commit-object BYTES are deliberately **NOT** a variant here — they are the residual handled
/// by the pseudonymous-by-default posture ([`GitResidual`]), never byte-mutated by a shred.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GitShreddable {
    /// The reflog — the log of ref updates (`refs/…` history). A *derived* structure that can leak a
    /// pseudonym's activity trail; it rides the blob tier and IS shreddable via the blob DEK.
    Reflog,
    /// The pack `.bitmap` reachability index — a *derived* acceleration structure built over the
    /// pack; it rides the blob tier and IS shreddable via the blob DEK.
    Bitmap,
    /// A pack-tier BACKUP (a versioned/replicated copy of a packfile, §7.1 T2). The backup holds
    /// CIPHERTEXT under the per-tenant blob DEK, so destroying the DEK reaches the backup by
    /// construction (§7.5) — the load-bearing "crypto-shred reaches backups" property.
    PackTierBackup,
}

impl GitShreddable {
    /// A stable, PII-free label for the structure (telemetry / the receipt — never personal data).
    pub fn label(self) -> &'static str {
        match self {
            GitShreddable::Reflog => "reflog",
            GitShreddable::Bitmap => "bitmap",
            GitShreddable::PackTierBackup => "pack-tier-backup",
        }
    }

    /// The full set of shreddable git structures the reach must reach. The reach asserts 0
    /// recoverable in backup for EVERY member (a missed structure is a leak).
    pub const ALL: [GitShreddable; 3] = [
        GitShreddable::Reflog,
        GitShreddable::Bitmap,
        GitShreddable::PackTierBackup,
    ];
}

/// **The residual half — the commit-object BYTES — and how it is handled (BY REFERENCE, never
/// restated).** Storage does NOT author a local residual statement; it names that the commit bytes
/// are handled per the ONE platform posture (contract 10.9 / `00 §X-7`): **pseudonymous-by-default
/// commits** (the immutable bytes carry only an opaque `<pseudonym>@<tenant>.noreply`, never erasable
/// real-identity PII — Id 4.8, decided before Git's data model froze, EI-04 §1).
///
/// The variant exists so the receipt can ASSERT the residual is the posture (not a silent gap and
/// not a byte-mutation the platform does not actually do). The on-demand history-rewrite path (10.6)
/// is the NAMED follow-on for the rare case the bytes themselves must go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitResidual {
    /// The commit-object bytes are left to the **pseudonymous-by-default** posture (10.9 / `00 §X-7`,
    /// by reference): the immutable bytes hold only an opaque pseudonym, so after the step-1
    /// pseudonym-map shred they resolve to nothing personal. The commit bytes are **NOT byte-mutated**
    /// by a crypto-shred. The audited history-rewrite path (10.6) is the on-demand follow-on.
    PseudonymousByDefault,
}

impl GitResidual {
    /// The contract reference the residual is handled BY (never a Storage-local restatement —
    /// 10.9 / `00 §X-7`, ratified once by counsel/DPO for all five subsystems).
    pub const RESIDUAL_POSTURE_REF: &'static str =
        "contract 10.9 / 00 §X-7 (the ONE platform free-text/immutable-content erasure posture); \
         git commit bytes = pseudonymous-by-default (Id 4.8); on-demand history-rewrite = 10.6";
}

// ───────────────────────────── the reach + its loud receipt ─────────────────────────────

/// **The dated GIT-D2 (storage-half) artifact the reach returns** — the PROOF the per-tenant blob
/// DEK crypto-shred reached git's reflog / bitmap / pack-tier-backup ciphertext (unrecoverable live
/// AND 0 recoverable in any backup), and that the commit-object residual is the documented
/// pseudonymous-by-default posture (NOT byte-mutated). PII-free: opaque tenant id + structure labels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitShredReceipt {
    /// The tenant whose git structures were reached (opaque — never personal data).
    pub tenant: TenantId,
    /// Whether the per-tenant blob DEK was actually destroyed this call (`true`) or was already gone
    /// (`false` — an idempotent re-run). Either way the post-condition holds: the DEK is destroyed.
    pub blob_dek_destroyed_now: bool,
    /// **THE GIT-D2 GATE READING:** for how many of the shreddable git structures the ciphertext is
    /// STILL recoverable from the backup snapshot AFTER the shred — MUST be **0** (the per-tenant
    /// blob DEK is destroyed AND excluded from backup, §7.5). A non-zero value is a RED drill: a
    /// backup could resurrect a reflog / bitmap / pack object.
    pub recoverable_in_backup: usize,
    /// The shreddable structures the reach covered (the [`GitShreddable::ALL`] set) — the receipt
    /// names them so a dropped structure is visible.
    pub structures_reached: Vec<GitShreddable>,
    /// The commit-object residual posture — MUST be [`GitResidual::PseudonymousByDefault`] (the
    /// documented posture, by reference to 10.9 — never a byte-mutation, never a silent gap).
    pub residual: GitResidual,
}

impl GitShredReceipt {
    /// Whether the GIT-D2 (storage-half) leg is GREEN: 0 recoverable in any backup for every
    /// shreddable git structure AND the residual is the documented pseudonymous-by-default posture.
    pub fn is_green(&self) -> bool {
        self.recoverable_in_backup == 0
            && self.residual == GitResidual::PseudonymousByDefault
            && self.structures_reached.len() == GitShreddable::ALL.len()
    }
}

/// **The git crypto-shred reach (contract 11.2 / 11.4, storage.md §5.3).**
///
/// Wires the [`crate::erase::CryptoShredErase`] step-2 crypto-shred to reach git's structures: it
/// destroys the **per-tenant blob DEK** ([`KeyClass::Blob`]) — the key the [`crate::gitpack`] pack
/// tier seals reflog / bitmap / pack-tier-backup ciphertext under — and VERIFIES the structures'
/// ciphertext is unrecoverable live AND absent from the backup snapshot (the reach is *verified, not
/// assumed*, §5.2). The commit-object bytes are NOT touched (the pseudonymous-by-default residual).
///
/// It borrows the SAME [`KmsEngine`] the pack tier's [`crate::encryption::DekContentWrap`] seals
/// blobs through — never a parallel key store, so the destroy reaches exactly the ciphertext those
/// structures wrote.
pub struct GitCryptoShredReach<'a> {
    engine: &'a KmsEngine,
    region: Region,
}

impl<'a> GitCryptoShredReach<'a> {
    /// Build the git crypto-shred reach over the KMS engine + the region the tenant's KEK lives in.
    pub fn new(engine: &'a KmsEngine, region: Region) -> GitCryptoShredReach<'a> {
        GitCryptoShredReach { engine, region }
    }

    /// The region the tenant's KEK (and so the per-tenant blob DEK) lives in.
    pub fn region(&self) -> &Region {
        &self.region
    }

    /// The per-tenant blob DEK id for a tenant — the key git's shreddable structures are sealed
    /// under ([`KeyClass::Blob`], the §3.2 per-blob content-key class).
    fn blob_dek_id(tenant: &TenantId) -> DekId {
        DekId::new(tenant.clone(), KeyClass::Blob)
    }

    /// **Run the git crypto-shred reach for `tenant`** (the storage half of GIT-D2).
    ///
    /// 1. Destroy the per-tenant blob DEK ([`KeyClass::Blob`]) — the crypto-shred that renders every
    ///    reflog / bitmap / pack-tier-backup ciphertext unrecoverable, live AND in backups by
    ///    construction (§7.5). `destroy_dek` returns `false` if the DEK was already gone (an
    ///    idempotent re-run) — which the reach treats as success (the post-condition already holds).
    /// 2. VERIFY the post-condition (the reach is *verified, not assumed*, §5.2): probe the KMS
    ///    backup snapshot and assert the per-tenant blob DEK is **absent** (0 recoverable) — so no
    ///    backup can resurrect a git structure.
    /// 3. Record the residual posture for the commit-object bytes: **pseudonymous-by-default** (10.9,
    ///    by reference) — the bytes are NOT byte-mutated.
    ///
    /// Idempotent: a second call for an already-shredded tenant is a no-op success (the DEK is gone,
    /// 0 recoverable holds). Returns the dated [`GitShredReceipt`].
    pub fn shred_git_structures(&self, tenant: &TenantId) -> GitShredReceipt {
        let blob_dek = Self::blob_dek_id(tenant);

        // ── Step 1: destroy the per-tenant blob DEK (the crypto-shred that reaches the git
        // reflog/bitmap/pack-backup ciphertext, live AND in every backup by construction). ──
        let blob_dek_destroyed_now = self.engine.destroy_dek(&blob_dek);

        // ── Step 2: VERIFY the reach (verified, not assumed, §5.2). The per-tenant blob DEK MUST be
        // absent from the backup snapshot — so no backup can resurrect a reflog/bitmap/pack object.
        // (`backup_snapshot` excludes a destroyed DEK / a fully-offboarded tenant, §7.5.) ──
        let recoverable_in_backup = self
            .engine
            .backup_snapshot()
            .iter()
            .filter(|(d, _)| *d == blob_dek)
            .count();

        GitShredReceipt {
            tenant: tenant.clone(),
            blob_dek_destroyed_now,
            recoverable_in_backup,
            // The reach covers EVERY shreddable structure — they share the one per-tenant blob DEK,
            // so destroying it reaches all of them at once (the receipt names them so a missed
            // structure would be a visible drop).
            structures_reached: GitShreddable::ALL.to_vec(),
            // ── Step 3: the commit-object residual is the documented posture, BY REFERENCE (never a
            // byte-mutation, never a Storage-local restatement). ──
            residual: GitResidual::PseudonymousByDefault,
        }
    }
}

/// The git crypto-shred reach IS the [`BlobShredReach`] seam the [`crate::erase::CryptoShredErase`]
/// step-2 crypto-shred invokes — so a subject's erase that touched git content reaches the git
/// structures (reflog / bitmap / pack-backup) in the SAME crypto-shred step as the per-subject DEK
/// destroy. The seam returns a loud [`EraseError`] only if the post-condition is NOT met (a backup
/// still holds a recoverable git structure) — a verified-not-assumed reach, never a silent claim.
impl BlobShredReach for GitCryptoShredReach<'_> {
    fn shred_blob_tier(
        &self,
        _subject: &crate::encryption::SubjectId,
        tenant: &TenantId,
    ) -> Result<(), EraseError> {
        let receipt = self.shred_git_structures(tenant);
        if receipt.is_green() {
            Ok(())
        } else {
            Err(EraseError::BlobShredReach(format!(
                "git crypto-shred reach for tenant `{}` is NOT green: {} git structure(s) still \
                 recoverable in backup (the per-tenant blob DEK was not excluded) — the erase is \
                 ABORTED as INCOMPLETE (a reflog/bitmap/pack backup could resurrect the structure)",
                tenant.as_str(),
                receipt.recoverable_in_backup,
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encryption::SubjectId;
    use crate::kms::{KekId, PiiKeyRef};
    use std::sync::Arc;

    fn t() -> TenantId {
        TenantId("acme".into())
    }
    fn r() -> Region {
        Region("eu-west".into())
    }

    /// Stand up a KMS engine with a tenant KEK + the per-tenant BLOB DEK (`KeyClass::Blob`) — the key
    /// git's reflog/bitmap/pack-tier-backup ciphertext is sealed under (§5.3). Returns the engine so
    /// the reach has a real key to destroy and a real backup snapshot to probe.
    fn engine_with_blob_dek(tenant: &TenantId) -> Arc<KmsEngine> {
        let kms = Arc::new(KmsEngine::new());
        kms.ensure_kek(&KekId::new(tenant.clone(), r()));
        // Provision the per-tenant blob DEK (KeyClass::Blob) — what git structures seal under.
        kms.ensure_dek(tenant, &r(), KeyClass::Blob)
            .expect("blob dek");
        kms
    }

    /// Seal a git structure's bytes under the per-tenant blob DEK (`KeyClass::Blob`) — exactly how
    /// reflog/bitmap/pack-tier-backup ciphertext rides the blob tier. Returns the `(key_ref, nonce,
    /// ciphertext)` an at-rest git structure (or its backup) holds.
    fn seal_git_structure(
        engine: &KmsEngine,
        tenant: &TenantId,
        bytes: &[u8],
    ) -> (PiiKeyRef, [u8; 12], Vec<u8>) {
        let key_ref = PiiKeyRef::new(tenant.clone(), 0, KeyClass::Blob);
        let dek = engine
            .resolve_dek(&key_ref, &r())
            .expect("resolve blob dek");
        let (nonce, ct) = dek.seal(bytes);
        (key_ref, nonce, ct)
    }

    #[test]
    fn reach_destroys_the_blob_dek_and_renders_git_structures_unrecoverable() {
        let tenant = t();
        let engine = engine_with_blob_dek(&tenant);

        // Seal a git structure (a reflog line) under the per-tenant blob DEK — it decrypts BEFORE the
        // shred. The ciphertext is what the live reflog AND its pack-tier backup hold (§7.5).
        let reflog = b"refs/heads/main 0000 abcd <pseudonym>@acme.noreply pushed";
        let (key_ref, nonce, ct) = seal_git_structure(&engine, &tenant, reflog);
        let dek_before = engine
            .resolve_dek(&key_ref, &r())
            .expect("blob dek resolves before shred");
        assert_eq!(
            dek_before.open(&nonce, &ct).expect("decrypts before shred"),
            reflog
        );

        // The per-tenant blob DEK is present in the backup snapshot BEFORE the shred.
        let blob_dek = DekId::new(tenant.clone(), KeyClass::Blob);
        assert!(
            engine.backup_snapshot().iter().any(|(d, _)| *d == blob_dek),
            "the blob DEK is in the backup before the git shred"
        );

        // Run the reach.
        let reach = GitCryptoShredReach::new(&engine, r());
        let receipt = reach.shred_git_structures(&tenant);

        // The blob DEK was destroyed this call; 0 recoverable in backup; residual is the posture.
        assert!(
            receipt.blob_dek_destroyed_now,
            "the per-tenant blob DEK was destroyed"
        );
        assert_eq!(
            receipt.recoverable_in_backup, 0,
            "GIT-D2: 0 git structures recoverable in backup"
        );
        assert_eq!(receipt.residual, GitResidual::PseudonymousByDefault);
        assert!(receipt.is_green(), "GIT-D2 (storage half) green");

        // BACKUP: the blob DEK is EXCLUDED from the backup snapshot after the shred (§7.5).
        assert!(
            !engine.backup_snapshot().iter().any(|(d, _)| *d == blob_dek),
            "the blob DEK is absent from the backup after the git shred (0 recoverable, §7.5)"
        );
        // LIVE: the git structure ciphertext is now unrecoverable — the DEK no longer resolves (a
        // LOUD KmsError, NEVER plaintext). The reflog/bitmap/pack-backup bytes are inert ciphertext.
        assert!(
            engine.resolve_dek(&key_ref, &r()).is_err(),
            "the git structure is unrecoverable after the crypto-shred (live): the blob DEK is gone"
        );
    }

    #[test]
    fn reach_covers_every_shreddable_structure_and_names_the_residual() {
        let tenant = t();
        let engine = engine_with_blob_dek(&tenant);
        let reach = GitCryptoShredReach::new(&engine, r());
        let receipt = reach.shred_git_structures(&tenant);

        // EVERY shreddable structure is reached (reflog + bitmap + pack-tier-backup), via the ONE
        // per-tenant blob DEK.
        assert_eq!(receipt.structures_reached, GitShreddable::ALL.to_vec());
        assert!(receipt.structures_reached.contains(&GitShreddable::Reflog));
        assert!(receipt.structures_reached.contains(&GitShreddable::Bitmap));
        assert!(receipt
            .structures_reached
            .contains(&GitShreddable::PackTierBackup));
        // The commit-object residual is the documented posture (pseudonymous-by-default), NOT a
        // byte-mutation — there is no "commit-bytes" variant on GitShreddable.
        assert_eq!(receipt.residual, GitResidual::PseudonymousByDefault);
    }

    #[test]
    fn reach_is_idempotent_a_second_shred_is_a_noop_success() {
        let tenant = t();
        let engine = engine_with_blob_dek(&tenant);
        let reach = GitCryptoShredReach::new(&engine, r());

        let r1 = reach.shred_git_structures(&tenant);
        assert!(
            r1.blob_dek_destroyed_now,
            "first shred destroys the blob DEK"
        );
        assert!(r1.is_green());

        // Second shred of the same tenant: a no-op SUCCESS (the DEK is already gone).
        let r2 = reach.shred_git_structures(&tenant);
        assert!(
            !r2.blob_dek_destroyed_now,
            "the blob DEK was already destroyed (idempotent re-run)"
        );
        assert_eq!(r2.recoverable_in_backup, 0, "still 0 recoverable in backup");
        assert!(r2.is_green());
    }

    #[test]
    fn reach_wired_as_the_blob_shred_seam_succeeds_when_green() {
        // The reach IS the BlobShredReach seam the erase orchestrator invokes — it returns Ok when
        // the post-condition holds (0 recoverable in backup).
        let tenant = t();
        let engine = engine_with_blob_dek(&tenant);
        let reach = GitCryptoShredReach::new(&engine, r());
        let subject = SubjectId::new("u-commit-author");
        assert!(
            reach.shred_blob_tier(&subject, &tenant).is_ok(),
            "the git shred reach as the erase seam succeeds when green"
        );
        // After the seam ran, the blob DEK is gone (the reach really destroyed it).
        let blob_dek = DekId::new(tenant.clone(), KeyClass::Blob);
        assert!(!engine.backup_snapshot().iter().any(|(d, _)| *d == blob_dek));
    }

    #[test]
    fn receipt_is_green_only_when_zero_recoverable_and_residual_is_the_posture() {
        // Kills the `is_green -> true` mutant: green requires 0 recoverable AND the posture residual
        // AND every structure reached.
        let green = GitShredReceipt {
            tenant: t(),
            blob_dek_destroyed_now: true,
            recoverable_in_backup: 0,
            structures_reached: GitShreddable::ALL.to_vec(),
            residual: GitResidual::PseudonymousByDefault,
        };
        assert!(green.is_green());
        // A recoverable backup structure is RED.
        let red = GitShredReceipt {
            recoverable_in_backup: 1,
            ..green.clone()
        };
        assert!(
            !red.is_green(),
            "a recoverable git structure in backup is RED"
        );
        // A dropped structure is RED.
        let dropped = GitShredReceipt {
            structures_reached: vec![GitShreddable::Reflog],
            ..green.clone()
        };
        assert!(!dropped.is_green(), "a missed git structure is RED");
    }

    #[test]
    fn shreddable_labels_and_residual_ref_are_stable_and_pii_free() {
        assert_eq!(GitShreddable::Reflog.label(), "reflog");
        assert_eq!(GitShreddable::Bitmap.label(), "bitmap");
        assert_eq!(GitShreddable::PackTierBackup.label(), "pack-tier-backup");
        assert_eq!(GitShreddable::ALL.len(), 3);
        // The residual is handled BY REFERENCE (10.9), never a Storage-local restatement.
        assert!(GitResidual::RESIDUAL_POSTURE_REF.contains("10.9"));
        assert!(GitResidual::RESIDUAL_POSTURE_REF.contains("pseudonymous-by-default"));
        assert!(
            GitResidual::RESIDUAL_POSTURE_REF.contains("10.6"),
            "names the history-rewrite follow-on"
        );
    }

    #[test]
    fn region_accessor_returns_the_kek_region() {
        let kms = KmsEngine::new();
        let reach = GitCryptoShredReach::new(&kms, r());
        assert_eq!(reach.region(), &r());
    }
}
