//! The Issues `PersonalDataHolder` **H3 intent** declaration (contract 10.1; architecture
//! issue-tracker 00-overview §1, 03-events-contracts-and-glue §7, 01-tech-and-data-model §6.1).
//!
//! Issues is **holder H3** (the platform-wide H1–H18 holder catalog: H3 = "Issues subsystem DB —
//! assignees/watchers/mentions, free-text, worklog"). It WILL register as a
//! [`myelin_gdpr::PersonalDataHolder`] (auto-registered by `serve` when the store opens in ISS-P07),
//! implementing `locate / export / rectify / restrict / erase` over its issues + comments +
//! change-log + worklog.
//!
//! **This module declares the INTENT, not the live holder.** It encodes, as inspectable data, the
//! 03 §7 personal-data inventory (where personal data lives in Issues + the per-locus erasure lever)
//! so the holder's eventual fan-out (the §7 erase table) has a single, reviewable source of truth
//! from M1 onward. The trait BODY — the real `locate`/`erase` over the issue stores — is the
//! **ISS-P07** floor; the GDPR producer-holder registration is **P-GA-27** (M3). Nothing here opens
//! a store or touches a key.
//!
//! Why an INTENT record now (not just a doc): the inventory is the contract between Issues and the
//! DSR orchestrator. Encoding it as typed data — referencing the **frozen** [`myelin_gdpr`] tag
//! enums ([`ErasureMethod`], [`DataRole`]) — means a drift in those frozen enums fails THIS crate's
//! build, and the unit tests pin that the inventory stays exhaustive over the §7 loci. It is the
//! "name your floors as code, not prose" realization (EI-01 §1).

use myelin_gdpr::{DataRole, ErasureMethod};

/// The stable holder identifier for Issues in the platform-wide holder registry: **H3** (the
/// H1–H18 catalog; H3 = Issues subsystem DB). The `serve` auto-registration (contract 1.4) will
/// register the opened issue store under this id in ISS-P07.
pub const HOLDER_ID: &str = "H3";

/// One locus in Issues where personal data lives, plus the lever that erases it (the 03 §7
/// personal-data inventory, encoded). The `erasure` lever references the **frozen**
/// [`myelin_gdpr::ErasureMethod`] (contract 10.2) so the inventory can never name an erasure
/// mechanism the platform does not have.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataLocus {
    /// Human-readable description of WHERE the personal data lives (03 §7).
    pub locus: &'static str,
    /// The GDPR fan-out classification (controller/processor posture). Issue content is
    /// [`DataRole::TenantContent`] (processor; the tenant org is the controller of issue content; a
    /// DSR is answered by/for the tenant, Art. 28 — 03 §7).
    pub role: DataRole,
    /// The erasure mechanism for this locus (03 §7; the frozen 10.2 enum). For the `CryptoShred`
    /// levers the `key_class` string names which key-hierarchy class to destroy (resolved into the
    /// KMS hierarchy with the ISS-P07 / P-GA-05 bodies).
    pub erasure: ErasureMethod,
    /// Whether this locus is the genuinely-hard residual handled BY REFERENCE by the ONE platform
    /// erasure posture (contract 10.9 / recon §X-7) rather than a structural lever Issues owns
    /// directly (03 §7 — "third-party free-text PII typed into another person's issue body/comment,
    /// encrypted under the AUTHOR's DEK"). The structural floor (pseudonym + per-subject DEK) ships
    /// regardless; this flags the residual whose lawful-basis limit counsel ratifies (X-7), NOT a
    /// hidden field. Issues does NOT restate a separate residual — it points at the platform posture.
    pub is_x7_residual: bool,
}

/// The Issues personal-data inventory (architecture 03 §7, the table that drives holder H3).
///
/// Encoded once, here, as the typed source of truth. Each entry's `erasure` lever references the
/// frozen [`myelin_gdpr::ErasureMethod`] — so this list is build-coupled to the contract surface.
/// The structural floor (entries 0–1, 3–4: pseudonym indirection + per-subject DEK crypto-shred)
/// ships across ISS-P07; entry 2 (third-party/immutable free-text) is the X-7 residual handled by
/// reference (contract 10.9), never restated locally (03 §7).
pub fn personal_data_inventory() -> Vec<DataLocus> {
    vec![
        // Assignee/reporter/created_by/mentionee/watcher identity — pseudonym in the issue rows;
        // real identity in Identity's erasable map ⇒ erasure = Pseudonymise (delete the map,
        // contract 4.8 — "Former user 8a2f" across all history). The lever that makes erasure free.
        DataLocus {
            locus: "assignee/reporter/created_by/mentionee/watcher identity (opaque pseudonym; real identity in Identity's map)",
            role: DataRole::TenantContent,
            erasure: ErasureMethod::Pseudonymise,
            is_x7_residual: false,
        },
        // Free-text title/props/comment bodies/change-deltas — inline, encrypted under the
        // per-subject DEK (contract 11.4) ⇒ erasure = CryptoShred(subject DEK); reaches live + backups.
        DataLocus {
            locus: "issue title/props + comment bodies + change-log deltas (encrypted under the per-subject DEK)",
            role: DataRole::TenantContent,
            erasure: ErasureMethod::CryptoShred("subject_dek".into()),
            is_x7_residual: false,
        },
        // Personal data inside free-text typed into ANOTHER person's issue body/comment (encrypted
        // under the AUTHOR's DEK) — the genuinely-hard residual: the ONE platform erasure posture
        // (contract 10.9 / X-7). The structural floor still applies; the lawful-basis limit is
        // counsel-ratified (X-7). CarveOut == suppress/restrict (never silently hide).
        DataLocus {
            locus: "third-party free-text PII in another person's issue body/comment (author's DEK — X-7 residual)",
            role: DataRole::TenantContent,
            erasure: ErasureMethod::CarveOut,
            is_x7_residual: true,
        },
        // Worklog / productivity / estimate fields (OQ-H behavioural) — restricted-by-default;
        // encrypted under the per-subject DEK ⇒ erasure = CryptoShred(subject DEK). The basis residual
        // (TBD_LEGAL, R-2) is on the SCHEMA tag, not here — the erasure LEVER is structural + ships now.
        DataLocus {
            locus: "worklog / productivity / estimate fields (OQ-H behavioural; restricted-by-default; per-subject DEK)",
            role: DataRole::TenantContent,
            erasure: ErasureMethod::CryptoShred("subject_dek".into()),
            is_x7_residual: false,
        },
        // Attachment filenames / blobs (may contain PII) — content-addressed in BlobStore ⇒
        // crypto-shred the per-tenant blob DEK.
        DataLocus {
            locus: "attachment filenames / blobs (content-addressed; per-tenant blob DEK)",
            role: DataRole::TenantContent,
            erasure: ErasureMethod::CryptoShred("tenant_blob_dek".into()),
            is_x7_residual: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Issues' holder id is H3 (the H1–H18 catalog). A drift here is a drift in the holder
    /// enumeration.
    #[test]
    fn issues_is_holder_h3() {
        assert_eq!(HOLDER_ID, "H3");
    }

    /// The 03 §7 inventory is exhaustive over the documented loci and every entry names a frozen
    /// erasure lever — so the holder's eventual fan-out can never reference an erasure mechanism the
    /// platform does not have. Exactly one entry is the X-7 residual (the third-party/immutable
    /// free-text case handled by reference, §7), and it is the ONLY one — the structural floor
    /// (pseudonym + per-subject/per-tenant DEK) covers the rest, INCLUDING the OQ-H worklog locus.
    #[test]
    fn inventory_is_exhaustive_and_uses_frozen_levers() {
        let inv = personal_data_inventory();
        assert_eq!(inv.len(), 5, "the 03 §7 inventory has five loci");

        // Every locus is tenant-content (processor posture): Issues holds issue content; the
        // platform-operational (controller) PII lives in GDPR/Identity holders, not Issues (03 §7).
        assert!(
            inv.iter().all(|d| d.role == DataRole::TenantContent),
            "every Issues data locus is processor-posture tenant content (03 §7)"
        );

        // Exactly one X-7 residual locus (third-party/immutable free-text — §7, contract 10.9).
        let residuals: Vec<_> = inv.iter().filter(|d| d.is_x7_residual).collect();
        assert_eq!(
            residuals.len(),
            1,
            "exactly one locus is the X-7 residual (third-party/immutable free-text)"
        );
        assert_eq!(residuals[0].erasure, ErasureMethod::CarveOut);

        // The two headline structural levers are present: pseudonymise the identity, and crypto-shred
        // the per-subject DEK for free-text bodies + the OQ-H worklog (the lever that reaches backups).
        assert!(
            inv.iter().any(|d| d.erasure == ErasureMethod::Pseudonymise),
            "pseudonymise lever (issue identity, contract 4.8) must be in the inventory"
        );
        assert!(
            inv.iter()
                .any(|d| d.erasure == ErasureMethod::CryptoShred("subject_dek".into())),
            "per-subject DEK crypto-shred (free-text + OQ-H worklog, contract 11.4) must be in the inventory"
        );
    }
}
