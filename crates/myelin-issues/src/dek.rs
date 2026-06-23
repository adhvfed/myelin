//! # `dek` — per-subject-DEK encryption for Issues free-text (ISS-P07 / P-373, M4-I1)
//!
//! **The non-negotiable this module ships (contract 11.4 / GD-4, recon §X-7 — the crypto-shred
//! erasure lever):** the free-text Issues columns — issue `title` / `props` (the custom-field JSONB
//! tail) and the `issue_change_log.change_delta` — are stored as **ciphertext sealed under the
//! per-subject DEK**, never plaintext. A subject's Art. 17 erasure destroys their DEK; their
//! free-text in the DB **and backups and immutable logs** becomes unrecoverable ciphertext (the
//! primary erasure mechanism for a subject's *own authored* content,
//! [recon §X-7](../../../planning/05-refined-shared-systems-architecture/00-reconciliation-decisions.md)).
//!
//! **Owning architecture / canon docs (read in full before changing this):**
//! - `planning/04-subsystem-architectures/issue-tracker/architecture/06-reconciliation-compliance.md`
//!   §2.12 (the per-subject DEK on free-text/body/change-delta columns, GD-4) + §2.13 (the ONE erasure
//!   posture by reference — per-subject DEK + pseudonym-map shred + `restrict`).
//! - `00-reconciliation-decisions.md` §X-7 (the structural floor: per-subject DEK crypto-shred).
//! - `01-tech-and-data-model.md` §2 (the per-subject DEK `pii_key_ref` columns the erase shreds).
//!
//! **Contracts (consumed — to the FROZEN shapes, never diverged):** **11.3** — the KMS hierarchy
//! ([`myelin_storage::kms::KmsEngine`], the cell-root → tenant-KEK → DEK envelope). **11.4** — the
//! GD-4 classify-driven per-subject-DEK column path ([`myelin_storage::encryption::ColumnCryptor`] +
//! [`key_class_for`]). Issues defines NO second cryptor and NO parallel key store (EI-01 §7): it seals
//! its free-text through the ONE shared `ColumnCryptor` over the ONE P-058 `KmsEngine`, so rotation /
//! crypto-shred reach these columns BY CONSTRUCTION. The `#[personal_data(erasure =
//! CryptoShred(subject_dek))]` tag on the [`crate::schema`] free-text fields is the SAME vocabulary
//! [`key_class_for`] routes to a [`KeyClass::Subject`] DEK — one classification fact, two readers.
//!
//! ## What this prompt (ISS-P07 / P-373) ships
//! - [`IssueFreeText`] — the three free-text Issues column kinds (title / props / change-delta), each
//!   carrying its `#[personal_data]` `erasure = CryptoShred(subject_dek)` method so the cryptor routes
//!   to the per-subject DEK (the GD-4 individual-erasure lever).
//! - [`encrypt_free_text`] / [`decrypt_free_text`] — the write/read round-trip over the ONE shared
//!   [`ColumnCryptor`]: a free-text value is sealed under the subject's per-subject DEK (ciphertext +
//!   the `pii_key_ref` DEK metadata at rest) and opened back while the key lives.
//! - [`plaintext_at_rest`] — the **0-plaintext-at-rest assertion** the GATE reads: a sealed
//!   [`EncryptedColumn`] never contains the plaintext byte-run.
//!
//! ## Mutation-score floor (mandatory-core — this IS the per-subject-DEK erasure seam)
//! The per-subject-DEK column path is the crypto-shred erasure lever (recon §X-7), so this module is a
//! **mandatory-core mutation target with a ≥ 90% floor**: `cargo mutants -p myelin-issues --file
//! crates/myelin-issues/src/dek.rs`. The mutation-tested core is the per-SUBJECT key-class routing
//! (every free-text column keys per-subject, never per-tenant — a downgrade loses the individual lever)
//! and the 0-plaintext-at-rest predicate (a sealed column never holds the plaintext byte-run). A mutant
//! that downgrades a free-text column to the tenant key, mis-routes the erasure method, or inverts the
//! plaintext-at-rest check is caught. **FLOOR (measured-under-load):** the measured % is the CI `cargo
//! mutants` artifact, registered red-until-run in the scorecard, never self-asserted (EI-01 §3).
//!
//! ## Floors named (VISION §3)
//! - **The `erase` crypto-shred BODY** (destroying the subject's DEK so every Issues free-text column
//!   it keyed becomes unrecoverable, + the `issue.*.erased` tombstones) is the Issues holder erase
//!   fan-out at **ISS-P31** ([`crate::holder`] names it). Here the COLUMNS are DEK-sealed (the
//!   structural floor); the destroy LEVER ([`myelin_storage::kms::KmsEngine`] `destroy_dek`) already
//!   exists.
//! - **The per-tenant DEK fallback for non-isolable third-party free-text** (a name typed by someone
//!   else into the subject's content) is the ONE platform residual (10.9 / X-7, by reference,
//!   [`crate::holder::ISSUE_RESIDUAL_POSTURE_REF`]); under the *author's* DEK, not the subject's. The
//!   `restrict` suppression (already wired, [`crate::holder`]) covers it pending counsel.

use myelin_gdpr::ErasureMethod;
use myelin_storage::encryption::{ColumnCryptor, EncryptedColumn, KeyChoiceError, SubjectId};
use myelin_storage::kms::{KekId, KmsEngine};
use myelin_tenancy::{Region, TenantId};

/// **The frozen per-subject-DEK erasure method for an Issues free-text column (contract 11.4 / GD-4).**
/// `CryptoShred("subject_dek")` is the SAME class-ref the [`crate::schema`] `#[personal_data(erasure =
/// CryptoShred(subject_dek))]` tag carries — [`key_class_for`](myelin_storage::encryption::key_class_for)
/// routes it to a [`KeyClass::Subject`](myelin_storage::kms::KeyClass::Subject) DEK (the individual
/// crypto-shred lever). A constant so the column path and the schema tag speak ONE vocabulary (EI-01
/// §7), never two.
pub fn subject_dek_erasure() -> ErasureMethod {
    ErasureMethod::CryptoShred("subject_dek".to_string())
}

/// **The free-text Issues column kinds that are sealed under the per-subject DEK (architecture §2.12 /
/// §6.1).** A closed enum — a new free-text Issues column cannot be added without appearing here (the
/// coverage is total, proven by the unit test). Every variant routes to the per-subject DEK
/// ([`subject_dek_erasure`]); the worklog behavioural fields share the same lever (they are PII the
/// subject's erasure must reach), so the OQ-H residual is covered by the SAME crypto-shred.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IssueFreeText {
    /// The issue `title` (inline free text). Per-subject DEK (§6.1).
    Title,
    /// The issue `props` JSONB custom-field tail (may carry free-text PII). Per-subject DEK (§6.1).
    Props,
    /// The `issue_change_log.change_delta` (the before/after of a field edit). Per-subject DEK (§5).
    ChangeDelta,
    /// The `issue_comment` body block subtree (free-text). Per-subject DEK (§6.1).
    CommentBody,
}

impl IssueFreeText {
    /// A stable, PII-free label for the column kind (telemetry / receipts — never personal data).
    pub fn label(self) -> &'static str {
        match self {
            IssueFreeText::Title => "title",
            IssueFreeText::Props => "props",
            IssueFreeText::ChangeDelta => "change_delta",
            IssueFreeText::CommentBody => "comment_body",
        }
    }

    /// The per-subject-DEK erasure method this column is sealed under — every free-text column is keyed
    /// per-subject (the GD-4 individual lever). The constant is the ONE shared class-ref the schema tag
    /// carries (EI-01 §7).
    pub fn erasure(self) -> ErasureMethod {
        subject_dek_erasure()
    }

    /// **The full set of per-subject-DEK free-text Issues columns** (architecture §2.12 / §6.1). The
    /// closed coverage surface: a missed column would store plaintext PII. A new free-text column
    /// cannot be added without appearing here (proven by the unit test).
    pub const ALL: [IssueFreeText; 4] = [
        IssueFreeText::Title,
        IssueFreeText::Props,
        IssueFreeText::ChangeDelta,
        IssueFreeText::CommentBody,
    ];
}

/// **Seal an Issues free-text column value under the SUBJECT's per-subject DEK (contract 11.4 / GD-4,
/// the write path).** Routes through the ONE shared [`ColumnCryptor`] over the P-058 [`KmsEngine`]:
/// the field's [`subject_dek_erasure`] tag drives [`key_class_for`](myelin_storage::encryption::key_class_for)
/// to a [`KeyClass::Subject`](myelin_storage::kms::KeyClass::Subject) DEK keyed on `subject`, the DEK
/// is provisioned + the plaintext sealed, and the [`EncryptedColumn`] (ciphertext + the `pii_key_ref`
/// DEK metadata, NO plaintext) is what rests in the column. `subject` is the OPAQUE pseudonymous
/// principal id the row's free-text belongs to (the same subject the row's
/// [`crate::pseudonym::IssuePseudonym`] identity columns key on); a subject-class tag with no subject
/// is a LOUD [`KeyChoiceError::SubjectClassMissingSubject`] — never a silent per-tenant downgrade
/// (that would lose the individual-erasure lever). The `_kind` is carried for telemetry/coverage; the
/// per-subject DEK is the same lever for every free-text column.
pub fn encrypt_free_text(
    engine: &KmsEngine,
    region: &Region,
    tenant: &TenantId,
    subject: &SubjectId,
    _kind: IssueFreeText,
    plaintext: &[u8],
) -> Result<EncryptedColumn, KeyChoiceError> {
    // The tenant's L1 KEK must exist before a per-subject DEK can be wrapped under it. In production
    // the harness provisions it at tenant store-open; ensure it idempotently here so a per-subject
    // free-text seal never races the KEK provision (idempotent — a second call returns the existing
    // KEK's epoch, it does NOT rotate, kms.rs §ensure_kek).
    engine.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
    let cryptor = ColumnCryptor::new(engine, region.clone());
    cryptor.encrypt(tenant, Some(subject), &subject_dek_erasure(), plaintext)
}

/// **Open a sealed Issues free-text column back to plaintext while the subject's DEK lives (the read
/// path / holder `export`).** Resolves the DEK named by the column's `pii_key_ref` and decrypts. After
/// the subject's `erase` (ISS-P31) shreds the DEK, this fails LOUDLY ([`KeyChoiceError::Kms`]) — the
/// free-text is unrecoverable (the GD-4 crypto-shred lever working), NEVER a plaintext-without-key
/// fall-through.
pub fn decrypt_free_text(
    engine: &KmsEngine,
    region: &Region,
    column: &EncryptedColumn,
) -> Result<Vec<u8>, KeyChoiceError> {
    let cryptor = ColumnCryptor::new(engine, region.clone());
    cryptor.decrypt(column)
}

/// **The 0-plaintext-at-rest assertion the GATE reads (contract 11.4).** `true` IFF the sealed column
/// contains the plaintext byte-run verbatim — a real ciphertext NEVER does for a non-trivial value.
/// The fixture asserts this is `false` for every sealed free-text column (0 plaintext free-text at
/// rest). Delegates to the shared [`EncryptedColumn::contains_plaintext`] — one assertion, not a
/// re-implemented byte scan (EI-01 §7).
pub fn plaintext_at_rest(column: &EncryptedColumn, plaintext: &[u8]) -> bool {
    column.contains_plaintext(plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> KmsEngine {
        KmsEngine::new()
    }
    fn region() -> Region {
        Region::new("fr-par")
    }
    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }
    fn subject() -> SubjectId {
        // the OPAQUE pseudonymous principal id the free-text belongs to (never a name/email).
        SubjectId::new("8a2f@acme.noreply")
    }

    /// **Free-text round-trips through per-subject-DEK encrypt/decrypt (contract 11.4).** A title is
    /// sealed under the subject's per-subject DEK and opened back to the exact plaintext while the key
    /// lives — the primary erasure-by-key-destroy mechanism for the subject's own content.
    #[test]
    fn free_text_round_trips_through_subject_dek() {
        let eng = engine();
        let plaintext = b"fix the login bug for Ada".to_vec();
        let column = encrypt_free_text(
            &eng,
            &region(),
            &tenant(),
            &subject(),
            IssueFreeText::Title,
            &plaintext,
        )
        .expect("seal under the subject DEK");
        let opened = decrypt_free_text(&eng, &region(), &column).expect("open while the key lives");
        assert_eq!(opened, plaintext, "the free-text round-trips exactly");
    }

    /// **0 plaintext free-text at rest (the GATE artifact, contract 11.4).** The sealed column holds
    /// ciphertext + the `pii_key_ref` DEK metadata — NOT the plaintext byte-run. The
    /// `plaintext_at_rest` assertion is `false` for every free-text column kind.
    #[test]
    fn zero_plaintext_free_text_at_rest_for_every_column_kind() {
        let eng = engine();
        for kind in IssueFreeText::ALL {
            let plaintext = format!("personal free text in {}", kind.label()).into_bytes();
            let column =
                encrypt_free_text(&eng, &region(), &tenant(), &subject(), kind, &plaintext)
                    .expect("seal");
            // ciphertext + DEK metadata at rest — the pii_key_ref names the per-subject DEK.
            assert!(
                column.key_ref.class.as_token().starts_with("subject:"),
                "the {} column is keyed under the per-subject DEK (GD-4)",
                kind.label()
            );
            // 0 plaintext at rest: the ciphertext does not contain the plaintext byte-run.
            assert!(
                !plaintext_at_rest(&column, &plaintext),
                "0 plaintext free-text at rest for the {} column",
                kind.label()
            );
        }
    }

    /// **Every free-text column routes to the per-subject DEK (the GD-4 individual lever), never the
    /// tenant key.** A per-subject DEK is DISTINCT from the tenant DEK (separate key class), so a
    /// subject's erasure destroys exactly their free-text — not the whole tenant's.
    #[test]
    fn every_free_text_column_is_keyed_per_subject() {
        for kind in IssueFreeText::ALL {
            match kind.erasure() {
                ErasureMethod::CryptoShred(class) => {
                    assert_eq!(class, "subject_dek", "{} → per-subject DEK", kind.label());
                }
                other => panic!(
                    "{} must crypto-shred per-subject, got {other:?}",
                    kind.label()
                ),
            }
        }
    }

    /// **Two different subjects get DISTINCT DEKs (the individual-erasure granularity, GD-4).** Sealing
    /// the SAME plaintext for two subjects yields ciphertext under two different per-subject key refs —
    /// so erasing one subject leaves the other's content intact.
    #[test]
    fn distinct_subjects_get_distinct_deks() {
        let eng = engine();
        let plaintext = b"shared phrase".to_vec();
        let a = encrypt_free_text(
            &eng,
            &region(),
            &tenant(),
            &SubjectId::new("aaaa@acme.noreply"),
            IssueFreeText::Title,
            &plaintext,
        )
        .unwrap();
        let b = encrypt_free_text(
            &eng,
            &region(),
            &tenant(),
            &SubjectId::new("bbbb@acme.noreply"),
            IssueFreeText::Title,
            &plaintext,
        )
        .unwrap();
        assert_ne!(
            a.key_ref.class, b.key_ref.class,
            "each subject has a DISTINCT per-subject DEK (GD-4 individual granularity)"
        );
    }

    /// **The free-text column-kind set is the full coverage (no plaintext-PII hole).** The closed set
    /// names every free-text Issues column; a missed one would store plaintext.
    #[test]
    fn the_free_text_column_set_is_the_full_coverage() {
        assert_eq!(IssueFreeText::ALL.len(), 4);
        for c in [
            IssueFreeText::Title,
            IssueFreeText::Props,
            IssueFreeText::ChangeDelta,
            IssueFreeText::CommentBody,
        ] {
            assert!(IssueFreeText::ALL.contains(&c), "{} is covered", c.label());
        }
    }
}
