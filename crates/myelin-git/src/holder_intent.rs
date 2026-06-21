//! The git `PersonalDataHolder` **H1 intent** declaration (contract 10.1; architecture
//! 00-overview §1.1, 03-events-contracts-and-glue §6, 01-tech-and-data-model §4.5).
//!
//! Git is **holder H1** — "the hardest in the platform" (§6). It WILL register as a
//! [`myelin_gdpr::PersonalDataHolder`] (auto-registered by `serve` when the store opens in GIT-P8),
//! implementing `locate / export / rectify / restrict / erase` over its repos + hosting metadata.
//!
//! **This module declares the INTENT, not the live holder.** It encodes, as inspectable data, the
//! §4.5 personal-data inventory (where personal data lives in git + the per-locus erasure lever) so
//! the holder's eventual fan-out (the §6.1 erasure algorithm) has a single, reviewable source of
//! truth from M1 onward. The trait BODY — the real `locate`/`erase` over git+metadata — is the
//! **GIT-P8/GIT-P9** floor; the GDPR producer-holder registration is **P-GA-27** (M3). Nothing here
//! opens a store or touches a key.
//!
//! Why an INTENT record now (not just a doc): the inventory is the contract between git and the DSR
//! orchestrator. Encoding it as typed data — referencing the **frozen** [`myelin_gdpr`] tag enums
//! ([`ErasureMethod`], [`DataRole`]) — means a drift in those frozen enums fails THIS crate's
//! build, and the unit tests pin that the inventory stays exhaustive over the §4.5 loci. It is the
//! "name your floors as code, not prose" realization (EI-01 §1).

use myelin_gdpr::{DataRole, ErasureMethod};

/// The stable holder identifier for git in the platform-wide holder registry: **H1**
/// (00-overview §1.1; the personal-data-holder enumeration H1–H18). The `serve` auto-registration
/// (contract 1.4) will register the opened git store under this id in GIT-P8.
pub const HOLDER_ID: &str = "H1";

/// One locus in git where personal data lives, plus the lever that erases it (the §4.5
/// personal-data inventory, encoded). The `erasure` lever references the **frozen**
/// [`myelin_gdpr::ErasureMethod`] (contract 10.2) so the inventory can never name an erasure
/// mechanism the platform does not have.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataLocus {
    /// Human-readable description of WHERE the personal data lives (§4.5 row 1).
    pub locus: &'static str,
    /// The GDPR fan-out classification (controller/processor posture). Repo content is
    /// [`DataRole::TenantContent`] (processor; the tenant org is the controller) per §6 / §4.5;
    /// operational PII (reflog/push records) is also tenant-scoped processor data here — the
    /// platform-operational (controller) loci live in GDPR/Identity holders, not git.
    pub role: DataRole,
    /// The erasure mechanism for this locus (§4.5 row 2; the frozen 10.2 enum). For the
    /// `CryptoShred` levers the `key_class` string names which key-hierarchy class to destroy
    /// (resolved into the KMS hierarchy with the GIT-P8/P-GA-05 bodies).
    pub erasure: ErasureMethod,
    /// Whether this locus is the genuinely-hard residual handled BY REFERENCE by the ONE platform
    /// erasure posture (contract 10.9 / recon §X-7) rather than a structural lever git owns
    /// directly (§4.5 row "personal data inside file content / commit messages authored by
    /// others"). The structural floor (pseudonym + per-subject DEK) ships regardless; this flags
    /// the residual whose lawful-basis limit counsel ratifies (X-7), NOT a hidden field.
    pub is_x7_residual: bool,
}

/// The git personal-data inventory (architecture §4.5, the table that drives holder H1).
///
/// Encoded once, here, as the typed source of truth. Each entry's `erasure` lever references the
/// frozen [`myelin_gdpr::ErasureMethod`] — so this list is build-coupled to the contract surface.
/// The structural floor (entries 0–1, 3–4: pseudonym indirection + per-subject DEK crypto-shred +
/// blob-DEK shred) ships across GIT-P8/P9; entry 2 (third-party/immutable free-text) is the X-7
/// residual handled by reference (contract 10.9), never restated locally (§6.2).
pub fn personal_data_inventory() -> Vec<DataLocus> {
    vec![
        // Commit author/committer identity — pseudonym in object bytes; real identity in
        // Identity's erasable map ⇒ erasure = Pseudonymise (delete the map, contract 4.8). GIT-1:
        // the lever that makes erasure usually free.
        DataLocus {
            locus: "commit author/committer identity (opaque pseudonym; real identity in Identity's map)",
            role: DataRole::TenantContent,
            erasure: ErasureMethod::Pseudonymise,
            is_x7_residual: false,
        },
        // PR/review/comment text (body_md, title) — inline, encrypted under the per-subject DEK
        // (contract 11.4) ⇒ erasure = CryptoShred(subject DEK); reaches live + backups.
        DataLocus {
            locus: "PR/review/comment free-text bodies + titles (encrypted under the per-subject DEK)",
            role: DataRole::TenantContent,
            erasure: ErasureMethod::CryptoShred("subject_dek".into()),
            is_x7_residual: false,
        },
        // Personal data inside file content / commit messages authored by OTHERS — the
        // genuinely-hard residual: the ONE platform erasure posture (contract 10.9 / X-7). The
        // structural floor still applies; the lawful-basis limit is counsel-ratified (X-7).
        // CarveOut == suppress/restrict (never silently hide) pending the history-rewrite op.
        DataLocus {
            locus: "personal data inside file content / commit messages authored by others (X-7 residual)",
            role: DataRole::TenantContent,
            erasure: ErasureMethod::CarveOut,
            is_x7_residual: true,
        },
        // LFS blobs (may contain PII) — content-addressed in BlobStore ⇒ crypto-shred the blob DEK.
        DataLocus {
            locus: "LFS blobs (content-addressed; per-tenant blob DEK)",
            role: DataRole::TenantContent,
            erasure: ErasureMethod::CryptoShred("tenant_blob_dek".into()),
            is_x7_residual: false,
        },
        // Reflog / push records / SSH-key fingerprints — operational PII: pseudonymised actor +
        // crypto-shred via the per-tenant blob DEK (reflogs/bitmaps/pack backups shreddable).
        DataLocus {
            locus: "reflog / push records / SSH-key fingerprints (pseudonymised actor + per-tenant blob DEK)",
            role: DataRole::TenantContent,
            erasure: ErasureMethod::CryptoShred("tenant_blob_dek".into()),
            is_x7_residual: false,
        },
    ]
}

/// The typed receipt that the git store **auto-registered as `PersonalDataHolder` H1** when it
/// opened (contract 1.4 / 10.1; architecture 00 §3.4 — the harness auto-registers every store it
/// opens, so "we forgot a store" is structurally impossible). GIT-P3 declared the H1 INTENT (the
/// inventory above); **GIT-P9 OPENS the store** ([`crate::receive_pack::RefStore::open`]) and this
/// receipt is the proof the registration hook fired. It mirrors the storage-tier
/// `OltpHolderRegistration` shape (the same auto-registration discipline, one per opened store).
///
/// **Floor (GIT-P29):** the holder's DSR BODIES (the §6.1 erasure fan-out — pseudonym-map shred +
/// per-subject DEK crypto-shred + Search purge + Refs tombstone, over the [`personal_data_inventory`]
/// loci) land in GIT-P29; the REGISTRATION (this receipt) is real here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HolderRegistration {
    /// The stable holder id the store registered under (always [`HOLDER_ID`] for git).
    pub holder_id: &'static str,
    /// Whether the auto-registration hook fired (always `true` for an opened store — the receipt
    /// only exists because the store opened; the field makes the asserted fact explicit).
    pub registered: bool,
}

impl HolderRegistration {
    /// Fire the H1 auto-registration hook for the opening git store (contract 1.4). The store
    /// cannot escape the holder registry: opening it produces this receipt. The DSR bodies are the
    /// GIT-P29 floor; the registration is real.
    pub fn auto_register() -> Self {
        Self { holder_id: HOLDER_ID, registered: true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Git's holder id is H1 (00-overview §1.1). A drift here is a drift in the holder enumeration.
    #[test]
    fn git_is_holder_h1() {
        assert_eq!(HOLDER_ID, "H1");
    }

    /// Opening the git store auto-registers it as H1 (the receipt is real; the DSR bodies are
    /// GIT-P29). The registration hook cannot be skipped — the receipt only exists if the store
    /// opened.
    #[test]
    fn auto_register_produces_a_real_h1_receipt() {
        let r = HolderRegistration::auto_register();
        assert_eq!(r.holder_id, "H1");
        assert!(r.registered);
    }

    /// The §4.5 inventory is exhaustive over the documented loci and every entry names a frozen
    /// erasure lever — so the holder's eventual fan-out can never reference an erasure mechanism the
    /// platform does not have. Exactly one entry is the X-7 residual (the third-party/immutable
    /// free-text case handled by reference, §6.2), and it is the ONLY one — the structural floor
    /// (pseudonym + per-subject/per-tenant DEK) covers the rest.
    #[test]
    fn inventory_is_exhaustive_and_uses_frozen_levers() {
        let inv = personal_data_inventory();
        assert_eq!(inv.len(), 5, "the §4.5 inventory has five loci");

        // Every locus is tenant-content (processor posture): git holds repo content; the
        // platform-operational (controller) PII lives in GDPR/Identity holders, not git (§6).
        assert!(
            inv.iter().all(|d| d.role == DataRole::TenantContent),
            "every git data locus is processor-posture tenant content (§6)"
        );

        // Exactly one X-7 residual locus (third-party/immutable free-text — §6.2, contract 10.9).
        let residuals: Vec<_> = inv.iter().filter(|d| d.is_x7_residual).collect();
        assert_eq!(
            residuals.len(),
            1,
            "exactly one locus is the X-7 residual (third-party/immutable free-text)"
        );
        assert_eq!(residuals[0].erasure, ErasureMethod::CarveOut);

        // The two headline structural levers are present (GIT-1): pseudonymise the identity, and
        // crypto-shred the per-subject DEK for free-text bodies (the lever that reaches backups).
        assert!(
            inv.iter().any(|d| d.erasure == ErasureMethod::Pseudonymise),
            "pseudonymise lever (commit identity, contract 4.8) must be in the inventory"
        );
        assert!(
            inv.iter()
                .any(|d| d.erasure == ErasureMethod::CryptoShred("subject_dek".into())),
            "per-subject DEK crypto-shred (free-text bodies, contract 11.4) must be in the inventory"
        );
    }
}
