//! # `crypto_shred` — the PersonalDataHolder erase path COMPLETED: the per-subject-DEK crypto-shred
//! reaching the inline-PII `wf_history` / `wf_signal` rows (FLOW-D9 / P-FLOW-24, M5).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/durable-workflow.md` §4.8 (GDPR erasure on
//! history via the references-not-payloads + crypto-shred + tombstone TRIAD — the structure is
//! preserved, the PII is destroyed) + §5.5 (`PersonalDataHolder` over `workflow_run`/`wf_history`/
//! `wf_signal`; the rare inline-PII result/payload crypto-shreds via `result_key_ref`/`payload_key_ref`)
//! + §3.2/§3.4 (the inline-PII envelope key refs — the ONLY PII locators in the engine).
//!
//! **Contract-index:** row 9.6 `PersonalDataHolder(workflow history) + replay` — the erase /
//! crypto-shred reach now **COMPLETE** (the M5 half P-FLOW-03 named as its floor). Consumes 11.3/11.4
//! (the KMS hierarchy + per-subject DEK — the [`KmsEngine`] crypto-shred lever) + 1.8 (the
//! crypto-shred-lag telemetry).
//!
//! ## What P-FLOW-24 COMPLETES — the crypto-shred reach (the P-FLOW-03 floor closed)
//! P-FLOW-03 ([`crate::holder`]) shipped the STRUCTURAL references-not-payloads erase: a refs-stored
//! `wf_history` row tombstones for free (Identity's §4.8 pseudonym-shred makes the opaque actor id
//! unresolvable; 0 PII columns mutated). It named ONE floor: the RARE inline-PII history/signal rows —
//! the rows whose `result_key_ref` / `payload_key_ref` names a per-subject DEK that sealed inline PII
//! into the journal — were NOT yet crypto-shredded (`key_epoch_destroyed = None`). This module CLOSES
//! that floor: erasing a subject now **destroys their per-subject DEK over the ONE [`KmsEngine`]**
//! (11.4 / GD-4), so the inline-PII ciphertext sealed under it becomes **unrecoverable — in the live
//! journal AND in every backup** (`backup_snapshot` excludes a shredded key, storage §7.5), WITHOUT
//! rewriting the append-only journal. The journal SHAPE survives (replay still works — every row is
//! byte-identical after the shred); only the PII is gone (the inline-PII cells decrypt to nothing — a
//! tombstone). This is the §4.8 triad complete: references-not-payloads (the structural floor) +
//! crypto-shred (this prompt) + tombstone (the structure preserved).
//!
//! ## The 0-recoverable-PII property (the FLOW-D9 core)
//! The drill asserts: after the erase, every inline-PII column the subject's history/signal rows
//! sealed is unrecoverable — `decrypt` fails LOUDLY (never a plaintext-without-key fall-through),
//! INCLUDING after a backup-restore-then-read attempt (the backup snapshot no longer carries the DEK,
//! so a restore cannot resurrect it — the shred stays dead across a restore, §7.5). A `false` here for
//! an erased subject's inline-PII column is the FLOW-D9 red drill (recoverable PII survived erasure).
//!
//! ## Structure preserved — replay still works (the §4.8 tombstone half)
//! The crypto-shred destroys the KEY, not the row. The `wf_history` / `wf_signal` rows are NEVER
//! deleted or mutated — the journal shape (the `command_id` replay-match keys, the `seq` order, the
//! refs-stored `result`/`payload`) is byte-identical after the erase, so deterministic replay
//! (P-FLOW-05) still rebuilds the run. The ONLY change is that an inline-PII cell that used to decrypt
//! to PII now decrypts to nothing (the per-subject DEK is gone) — the PII is a tombstone, the
//! structure lives. "Delete the content, keep the fact."
//!
//! ## The residual — BY REFERENCE, never restated (10.9 / X-7)
//! A subject P's name typed into the inline-PII `result` of someone ELSE's un-erased run is sealed
//! under a DIFFERENT subject's DEK (the run's actor, not P), so P's erasure does not crypto-shred it.
//! This is the ONE platform posture (10.9 / X-7, the references-not-payloads + crypto-shred + tombstone
//! triad §4.8) — cited, never re-authored flow-local. The structural floor (the per-subject DEK shred)
//! ships regardless.
//!
//! ## Mutation floor (mandatory-core, >= 95% — prompt TESTS field)
//! This module is the flow crypto-shred / erasure key-selection CORE: a surviving mutant is PII that
//! survives erasure. The mutation-tested path is the per-subject-DEK SELECTION ([`subject_dek_id`]
//! picking the [`KeyClass::Subject`] DEK for the erased subject, NOT the per-tenant DEK) + the
//! 0-recoverable predicate ([`WfCryptoShred::shred_subject`] destroying that DEK so hot + backup
//! decrypts all fail). A mutant that selects the per-tenant DEK instead of the per-subject DEK, or
//! skips the shred, MUST be caught — `cargo mutants -p myelin-flow -f crates/myelin-flow/src/crypto_shred.rs`.
//! The measured % is the CI artifact, registered red-until-run in the scorecard, never self-asserted.
//!
//! ## DB-free
//! This module operates over the in-memory [`crate::wfctx::WfJournal`] + the ONE [`KmsEngine`]; the
//! real per-subject-DEK destroy across the live `myelin-flow` Postgres + the KMS backups rides the
//! FLOW-D9 drill against the dev stack (the storage restore-verify drill STOR-D3 proves the
//! backup-exclusion at cell scale). `cargo build --workspace` stays DB-free.

use myelin_gdpr::{EraseReceipt, EraseScope, ErasureMethod, Receipt, SubjectRef};
use myelin_storage::encryption::{ColumnCryptor, EncryptedColumn, KeyChoiceError, SubjectId};
use myelin_storage::kms::{DekId, KekId, KeyClass, KmsEngine};
use myelin_tenancy::{Region, TenantId as TenancyTenantId};

use crate::engine::FlowTelemetry;
use crate::holder::FLOW_OLTP_STORE;
use crate::schema::{WfHistoryRow, WfSignalRow};

/// **The frozen per-subject-DEK erasure method for an inline-PII `wf_history`/`wf_signal` column
/// (contract 11.4 / GD-4).** `CryptoShred("subject_dek")` is the SAME class-ref the
/// [`crate::schema`] `#[personal_data(erasure = CryptoShred(subject_dek))]` tag carries on
/// `result_key_ref` / `payload_key_ref` — [`key_class_for`](myelin_storage::encryption::key_class_for)
/// routes it to a [`KeyClass::Subject`] DEK (the individual crypto-shred lever). A constant so the
/// inline-PII seal path and the schema tag speak ONE vocabulary (EI-01 §7), never two.
pub fn subject_dek_erasure() -> ErasureMethod {
    ErasureMethod::CryptoShred("subject_dek".to_string())
}

/// **The per-subject DEK id the crypto-shred destroys (the GD-4 key-selection — the mutation-tested
/// core).** Picks the [`KeyClass::Subject`] DEK for the ERASED subject — NOT the per-tenant DEK. A
/// mutant that selects [`KeyClass::Tenant`] here (or another subject) is exactly the "PII survives
/// erasure" / "a whole tenant got crypto-shredded for one person" bug the >= 95% floor must catch. The
/// `subject_id` is the OPAQUE pseudonymous principal id (never a name/email).
pub fn subject_dek_id(tenant: &TenancyTenantId, subject_id: &str) -> DekId {
    DekId::new(tenant.clone(), KeyClass::Subject(subject_id.to_string()))
}

/// **Seal an inline-PII `wf_history.result` / `wf_signal.payload` value under the run actor's
/// per-subject DEK (contract 11.4 / GD-4 — the RARE inline-PII write path, §3.2/§3.4).** The vast
/// majority of journal rows are references-not-payloads (refs, never PII bodies); the RARE inline-PII
/// case (a result/payload that genuinely carries personal data with no erasable owner) is sealed here
/// under the SUBJECT's per-subject DEK so erasing the subject crypto-shreds it. Routes through the ONE
/// shared [`ColumnCryptor`] over the P-058 [`KmsEngine`] — the field's [`subject_dek_erasure`] tag
/// drives the key choice to [`KeyClass::Subject`] keyed on `subject`. A subject-class tag with no
/// subject is a LOUD [`KeyChoiceError::SubjectClassMissingSubject`] — never a silent per-tenant
/// downgrade (that would lose the individual-erasure lever). The returned [`EncryptedColumn`] carries
/// the `pii_key_ref` the `result_key_ref` / `payload_key_ref` column stores (the crypto-shred locator).
pub fn seal_inline_pii(
    engine: &KmsEngine,
    region: &Region,
    tenant: &TenancyTenantId,
    subject: &SubjectId,
    plaintext: &[u8],
) -> Result<EncryptedColumn, KeyChoiceError> {
    // The tenant's L1 KEK must exist before a per-subject DEK can be wrapped under it. In production
    // the harness provisions it at tenant store-open; ensure it idempotently here (a second call
    // returns the existing KEK's epoch, it does NOT rotate — kms.rs §ensure_kek).
    engine.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
    let cryptor = ColumnCryptor::new(engine, region.clone());
    cryptor.encrypt(tenant, Some(subject), &subject_dek_erasure(), plaintext)
}

/// **Open a sealed inline-PII journal column back to plaintext WHILE the subject's DEK lives (the read
/// path / holder `export`).** Resolves the DEK named by the column's `pii_key_ref` and decrypts. After
/// the subject's `erase` (this prompt) shreds the DEK, this fails LOUDLY ([`KeyChoiceError::Kms`]) —
/// the inline PII is unrecoverable (the GD-4 crypto-shred lever working), NEVER a plaintext-without-key
/// fall-through. This holds for the SAME ciphertext whether read live OR restored from a backup (the
/// backup no longer carries the DEK, §7.5).
pub fn open_inline_pii(
    engine: &KmsEngine,
    region: &Region,
    column: &EncryptedColumn,
) -> Result<Vec<u8>, KeyChoiceError> {
    let cryptor = ColumnCryptor::new(engine, region.clone());
    cryptor.decrypt(column)
}

/// **Whether an inline-PII journal column is UNRECOVERABLE (the FLOW-D9 0-recoverable predicate).**
/// `true` iff decrypting the column FAILS — the per-subject DEK was crypto-shredded, so the ciphertext
/// (live OR restored from a backup) can never become plaintext again. This is the property the FLOW-D9
/// drill asserts across the live journal + a backup-restore-then-read attempt: after the erase, every
/// inline-PII column the subject sealed returns `is_inline_pii_unrecoverable == true`. A `false` here
/// for an erased subject's column is the FLOW-D9 red drill (recoverable PII).
pub fn is_inline_pii_unrecoverable(
    engine: &KmsEngine,
    region: &Region,
    column: &EncryptedColumn,
) -> bool {
    open_inline_pii(engine, region, column).is_err()
}

/// **The aggregate crypto-shred report (FLOW-D9 — the erase/crypto-shred outcome).** Returned from
/// [`WfCryptoShred::shred_subject`]: the destroyed per-subject DEK epoch (the GD-4 audit trail driving
/// post-restore re-erasure, 10.8), how many inline-PII history/signal rows the shred reached (the
/// structural tombstone count), and the crypto-shred-lag the shred took (the FLOW-D9 telemetry signal).
/// PII-free — a (epoch, count, lag) tag, never personal data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WfShredReport {
    /// The per-subject DEK epoch the crypto-shred destroyed (`None` for a tenant offboarding, which
    /// destroys the KEK, or when the subject had NO inline-PII rows — nothing to shred). Drives the
    /// post-restore re-erasure (10.8).
    pub destroyed_key_epoch: Option<u64>,
    /// How many inline-PII `wf_history` + `wf_signal` rows the subject appeared in (the rows whose
    /// `result_key_ref` / `payload_key_ref` named the destroyed DEK — now unrecoverable, the structure
    /// tombstoned in place). The references-not-payloads rows tombstone for free and are NOT counted
    /// here (they need no key destroyed — the P-FLOW-03 structural surface).
    pub inline_pii_rows_shredded: usize,
    /// The crypto-shred-lag (seconds) between the erase request and the shred completing — the FLOW-D9
    /// green-artifact signal recorded on [`FlowTelemetry`].
    pub crypto_shred_lag_secs: u64,
}

/// **The flow crypto-shred cascade — the live binding behind the H8 `PersonalDataHolder::erase` seam
/// (P-FLOW-24, contract 9.6 / 11.4 / 1.8).** Holds the REAL flow dependencies the frozen 10.1
/// `erase(scope)` signature has no room for: the ONE [`KmsEngine`] (the crypto-shred lever) + the
/// cell's residency [`Region`] (the DEK/KEK live in it) + an optional [`FlowTelemetry`] (the
/// crypto-shred-lag signal). ONE cascade destroys the per-subject DEK — NEVER a second key store.
pub struct WfCryptoShred<'a> {
    kms: &'a KmsEngine,
    region: Region,
    telemetry: Option<&'a FlowTelemetry>,
}

impl<'a> WfCryptoShred<'a> {
    /// Build the cascade over the ONE live [`KmsEngine`] + the cell's residency region (the boot-time
    /// binding). No telemetry sink (a unit caller that does not assert the lag signal).
    pub fn new(kms: &'a KmsEngine, region: Region) -> WfCryptoShred<'a> {
        WfCryptoShred {
            kms,
            region,
            telemetry: None,
        }
    }

    /// Build the cascade with a [`FlowTelemetry`] sink so the crypto-shred-lag signal (FLOW-D9 / 1.8)
    /// is recorded on each shred — the dated green artifact the metrics-health port exports.
    pub fn with_telemetry(
        kms: &'a KmsEngine,
        region: Region,
        telemetry: &'a FlowTelemetry,
    ) -> WfCryptoShred<'a> {
        WfCryptoShred {
            kms,
            region,
            telemetry: Some(telemetry),
        }
    }

    /// **The crypto-shred reach — the FLOW-D9 0-recoverable-PII core (contract 9.6 erase COMPLETE).**
    /// For an [`EraseScope::Subject`]: destroy the subject's per-subject DEK over the ONE [`KmsEngine`]
    /// — every inline-PII `result`/`payload` the subject sealed becomes unrecoverable ciphertext in the
    /// live journal AND in every backup SIMULTANEOUSLY (the rows are sealed under THIS DEK at rest in
    /// both, and `backup_snapshot` excludes the shredded key, §7.5), WITHOUT rewriting the append-only
    /// journal. `inline_pii_rows` is the count of the subject's inline-PII history/signal rows (the rows
    /// whose key ref named the destroyed DEK) — supplied by the holder's `locate` walk so the receipt
    /// records the reach. `now_secs` / `requested_at_secs` bound the crypto-shred-lag the receipt + the
    /// telemetry record (the FLOW-D9 signal).
    ///
    /// For an [`EraseScope::Tenant`]: the whole-tenant erasure rides the tenant-KEK destroy (the
    /// GDPR-side offboarding lever, P-GA-13); here no per-subject DEK is destroyed (the report records
    /// `destroyed_key_epoch = None`).
    pub fn shred_subject(
        &self,
        scope: &EraseScope,
        inline_pii_rows: usize,
        requested_at_secs: u64,
        now_secs: u64,
    ) -> WfShredReport {
        let crypto_shred_lag_secs = now_secs.saturating_sub(requested_at_secs);

        let destroyed_key_epoch = match scope {
            EraseScope::Subject { subject, tenant } => {
                let sid = subject_token(subject);
                let tenancy_tenant = TenancyTenantId(tenant.0.clone());
                // THE GD-4 KEY SELECTION (the mutation-tested core): pick the ERASED subject's
                // per-subject DEK — NOT the per-tenant DEK. Selecting the tenant DEK here would
                // crypto-shred the WHOLE tenant for one person (or, if the subject DEK is skipped,
                // leave their PII recoverable). The >= 95% floor catches both mutants.
                let dek_id = subject_dek_id(&tenancy_tenant, &sid);
                // Resolve the epoch BEFORE destroying (the audit trail the ledger records). `None`
                // when the subject had no inline-PII DEK (they sealed nothing — nothing to shred).
                let epoch = self.dek_epoch(&tenancy_tenant, &sid, &dek_id);
                // DESTROY the per-subject DEK — the GD-4 individual crypto-shred lever. One destroy
                // renders every inline-PII row the subject sealed unrecoverable in the live journal
                // AND every backup. The journal rows are NEVER touched (structure preserved).
                self.kms.destroy_dek(&dek_id);
                epoch
            }
            // Tenant offboarding: the KEK destroy is the GDPR-side P-GA-13 lever (it cascades to
            // every DEK under it). No per-subject epoch is recorded here.
            EraseScope::Tenant(_) => None,
        };

        // Record the crypto-shred-lag signal (FLOW-D9 / contract 1.8) — the dated green artifact. Only
        // a real per-subject shred (a destroyed DEK) bumps the counter; a tenant scope or a subject
        // with no inline-PII rows records no shred (there was nothing to shred).
        if destroyed_key_epoch.is_some() {
            if let Some(t) = self.telemetry {
                t.record_crypto_shred(crypto_shred_lag_secs);
            }
        }

        WfShredReport {
            destroyed_key_epoch,
            inline_pii_rows_shredded: inline_pii_rows,
            crypto_shred_lag_secs,
        }
    }

    /// Resolve the live epoch of a subject's per-subject DEK (so the receipt records WHICH epoch the
    /// shred destroyed — the GD-4 audit trail driving post-restore re-erasure, 10.8). `None` if no DEK
    /// is present (the subject sealed nothing under their per-subject DEK → nothing to shred).
    ///
    /// Presence is probed via the backup snapshot (a live DEK appears there); the epoch is read via the
    /// idempotent `ensure_dek` (which, for an EXISTING DEK, returns its current epoch WITHOUT rotating
    /// or creating — verified-present first, so this never fabricates a key to then shred).
    fn dek_epoch(&self, tenant: &TenancyTenantId, sid: &str, dek_id: &DekId) -> Option<u64> {
        let present = self
            .kms
            .backup_snapshot()
            .into_iter()
            .any(|(id, _)| &id == dek_id);
        if !present {
            return None;
        }
        self.kms
            .ensure_dek(tenant, &self.region, KeyClass::Subject(sid.to_string()))
            .ok()
            .map(|key_ref| key_ref.dek_epoch)
    }
}

/// **The P-FLOW-24 erase receipt the holder's trait-surface `erase` returns (contract 9.6 / 10.1).**
/// Folds the crypto-shred report into the frozen [`EraseReceipt`] shape: content-addressed over the
/// holder + subject + tenant + the destroyed epoch, so the aggregate is hash-linked into the audit
/// log. The `key_epoch_destroyed` field carries the destroyed per-subject DEK epoch — the audit trail
/// the crypto-shred reach now records (P-FLOW-03's `None` is replaced by the real epoch when inline-PII
/// rows were shredded). Used by [`crate::holder::WfHistoryHolder::erase`] when bound to a live shred.
pub fn aggregate_receipt(report: &WfShredReport, scope: &EraseScope) -> EraseReceipt {
    let (subject_token, tenant) = match scope {
        EraseScope::Subject { subject, tenant } => (subject_token(subject), tenant.0.clone()),
        EraseScope::Tenant(t) => (String::new(), t.0.clone()),
    };
    let outcome = match scope {
        EraseScope::Subject { .. } => format!(
            "crypto-shred reach (P-FLOW-24): per-subject DEK destroyed (epoch={:?}) — {} inline-PII \
             wf_history/wf_signal rows unrecoverable incl. backups, structure preserved (replay \
             works, the PII is a tombstone); crypto-shred-lag={}s; refs-stored rows tombstone for \
             free (P-FLOW-03); residual = the ONE posture 10.9/X-7 by reference",
            report.destroyed_key_epoch, report.inline_pii_rows_shredded, report.crypto_shred_lag_secs,
        ),
        EraseScope::Tenant(_) => {
            "tenant crypto-shred: destroy the per-tenant KEK (11.3/11.4) — every workflow row \
             unrecoverable (the P-GA-13 offboarding lever)"
                .to_string()
        }
    };
    EraseReceipt {
        receipt: Receipt::content_addressed(
            "erase",
            FLOW_OLTP_STORE,
            &subject_token,
            &tenant,
            &outcome,
            report.destroyed_key_epoch,
            0,
        ),
    }
}

/// The opaque, PII-free subject token (the pseudonymous principal id) — never a name/email. ONE
/// derivation, shared with the holder (EI-01 §7).
fn subject_token(subject: &SubjectRef) -> String {
    subject.principal.principal_id.0.clone()
}

/// **Whether a `wf_history` row carries inline PII naming the subject (the crypto-shred reach
/// predicate, §3.2/§4.8).** `true` iff the row's `result_key_ref` names the subject's per-subject DEK
/// (`…/subject/<id>` in the schema-tag grammar OR `subject:<id>` in the `pii_key_ref` grammar). These
/// are the rows the per-subject-DEK shred makes unrecoverable; the references-not-payloads rows (no
/// key ref) tombstone for free and are NOT in this set. Used by the holder's `locate` to COUNT the
/// inline-PII reach the receipt records.
pub fn history_row_has_inline_pii(row: &WfHistoryRow, subject_id: &str) -> bool {
    key_ref_names_subject(row.result_key_ref.as_deref(), subject_id)
}

/// **Whether a `wf_signal` row carries inline PII naming the subject (the crypto-shred reach
/// predicate, §3.4/§4.8).** `true` iff the row's `payload_key_ref` names the subject's per-subject DEK.
/// The signal half of [`history_row_has_inline_pii`] — the buffered-signal inline-PII rows the
/// per-subject-DEK shred reaches.
pub fn signal_row_has_inline_pii(row: &WfSignalRow, subject_id: &str) -> bool {
    key_ref_names_subject(row.payload_key_ref.as_deref(), subject_id)
}

/// Whether a stored inline-PII key ref names the subject's per-subject DEK. Accepts BOTH the schema-tag
/// grammar (`…/subject/<id>`, the `result_key_ref` the §3.2 tag documents) AND the `pii_key_ref`
/// grammar (`subject:<id>`, what [`seal_inline_pii`] writes) — ONE predicate over both so the holder's
/// `locate` and the cryptor's `key_ref` speak one vocabulary (EI-01 §7).
fn key_ref_names_subject(key_ref: Option<&str>, subject_id: &str) -> bool {
    let Some(k) = key_ref else {
        return false;
    };
    k.ends_with(&format!("/subject/{subject_id}"))
        || k.contains(&format!("/subject/{subject_id}/"))
        || k.ends_with(&format!("subject:{subject_id}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wfctx::WfJournal;
    use myelin_gdpr::TenantId;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_refs::ArtifactRef;

    fn region() -> Region {
        Region::new("fr-par")
    }
    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }
    fn tenancy() -> TenancyTenantId {
        TenancyTenantId::from_token("acme")
    }
    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId::from_token("acme"),
        ))
    }
    fn sid(id: &str) -> SubjectId {
        SubjectId::new(id)
    }

    /// **The FLOW-D9 CORE: an erased subject's inline-PII journal cell becomes UNRECOVERABLE in the
    /// live journal + a backup-restore-then-read attempt (0 recoverable PII).** An inline-PII result
    /// sealed under the subject's per-subject DEK opens while the key lives; after the cascade
    /// crypto-shreds the DEK, the SAME ciphertext (read live OR restored from a backup snapshot) is
    /// unrecoverable — `open_inline_pii` fails LOUDLY, never a plaintext-without-key fall-through. The
    /// backup snapshot no longer carries the DEK (it stays dead across a restore, §7.5).
    #[test]
    fn erased_subject_inline_pii_is_unrecoverable_live_and_after_backup_restore() {
        let kms = KmsEngine::new();
        let plaintext = b"the subject's medical note inlined into a run result".to_vec();
        // Seal an inline-PII result under the subject's per-subject DEK (the same ciphertext rests in
        // the live journal column AND any backup of it).
        let column = seal_inline_pii(&kms, &region(), &tenancy(), &sid("psn:ada"), &plaintext)
            .expect("seal under the subject's per-subject DEK");
        // While the key lives: recoverable.
        assert!(!is_inline_pii_unrecoverable(&kms, &region(), &column));
        assert_eq!(
            open_inline_pii(&kms, &region(), &column).expect("opens"),
            plaintext
        );

        // Run the crypto-shred (the per-subject DEK destroy).
        let shred = WfCryptoShred::new(&kms, region());
        let report = shred.shred_subject(
            &EraseScope::Subject {
                subject: subject("psn:ada"),
                tenant: tenant(),
            },
            1,
            100,
            103,
        );
        assert_eq!(
            report.destroyed_key_epoch,
            Some(0),
            "the receipt records the destroyed key epoch (the post-restore re-erase audit trail)"
        );

        // After the shred: the SAME column ciphertext (live OR backup-restored — the bytes are
        // identical in both) is UNRECOVERABLE.
        assert!(
            is_inline_pii_unrecoverable(&kms, &region(), &column),
            "0 recoverable: the inline PII is unrecoverable after the per-subject DEK shred"
        );
        // A BACKUP-RESTORE-THEN-READ attempt fails too — the shredded DEK is excluded from the backup
        // snapshot (it stays dead across a restore, §7.5), so a restore cannot resurrect it.
        let snapshot = kms.backup_snapshot();
        assert!(
            !snapshot
                .into_iter()
                .any(|(id, _)| id == DekId::new(tenancy(), KeyClass::Subject("psn:ada".into()))),
            "the crypto-shredded DEK is EXCLUDED from backups — a restore cannot read the PII"
        );
    }

    /// **THE GD-4 KEY-SELECTION (the mutation-tested core): erasing ONE subject shreds ONLY their
    /// per-subject DEK — the tenant + other subjects are untouched.** Two subjects seal inline PII; the
    /// erase of subject u-1 makes u-1's cell unrecoverable while u-2's still opens. A mutant that
    /// selects the per-tenant DEK (or u-2's) is caught here (it would crypto-shred the wrong key).
    #[test]
    fn shred_destroys_only_the_erased_subjects_dek_not_the_tenant_or_others() {
        let kms = KmsEngine::new();
        let col1 = seal_inline_pii(&kms, &region(), &tenancy(), &sid("u-1"), b"u-1 private")
            .expect("seal u-1");
        let col2 = seal_inline_pii(&kms, &region(), &tenancy(), &sid("u-2"), b"u-2 private")
            .expect("seal u-2");
        // A per-tenant DEK also exists (bulk content) — it must survive a per-subject erase.
        let tenant_col = {
            let cryptor = ColumnCryptor::new(&kms, region());
            cryptor
                .encrypt(
                    &tenancy(),
                    None,
                    &ErasureMethod::CryptoShred("tenant".to_string()),
                    b"bulk tenant content",
                )
                .expect("seal tenant bulk")
        };

        // Erase ONLY u-1.
        let shred = WfCryptoShred::new(&kms, region());
        shred.shred_subject(
            &EraseScope::Subject {
                subject: subject("u-1"),
                tenant: tenant(),
            },
            1,
            0,
            0,
        );

        // u-1 unrecoverable; u-2 + the tenant bulk untouched (the GD-4 individual lever, not a tenant
        // wipe).
        assert!(
            is_inline_pii_unrecoverable(&kms, &region(), &col1),
            "u-1's inline PII is unrecoverable (their DEK was shredded)"
        );
        assert!(
            !is_inline_pii_unrecoverable(&kms, &region(), &col2),
            "u-2's inline PII still opens (a DIFFERENT subject's DEK — not touched)"
        );
        assert!(
            !is_inline_pii_unrecoverable(&kms, &region(), &tenant_col),
            "the per-tenant bulk DEK is untouched — a per-subject erase is NOT a tenant wipe"
        );
    }

    /// **The crypto-shred-lag telemetry signal is recorded (FLOW-D9 / contract 1.8).** A subject with
    /// inline PII is erased; the cascade records the lag (now − requested) on the telemetry sink and
    /// bumps the shred counter. The FLOW-D9 drill reads this dated green signal.
    #[test]
    fn crypto_shred_records_the_lag_telemetry_signal() {
        let kms = KmsEngine::new();
        let _col =
            seal_inline_pii(&kms, &region(), &tenancy(), &sid("u-lag"), b"pii").expect("seal");
        let telemetry = FlowTelemetry::new();
        let shred = WfCryptoShred::with_telemetry(&kms, region(), &telemetry);
        let report = shred.shred_subject(
            &EraseScope::Subject {
                subject: subject("u-lag"),
                tenant: tenant(),
            },
            1,
            1000,
            1005,
        );
        assert_eq!(report.crypto_shred_lag_secs, 5, "lag = now − requested");
        assert_eq!(
            telemetry.crypto_shred_lag_secs(),
            5,
            "the crypto-shred-lag signal is on the telemetry sink (FLOW-D9 green artifact)"
        );
        assert_eq!(
            telemetry.crypto_shreds_count(),
            1,
            "one subject's inline-PII rows made unrecoverable"
        );
    }

    /// **A subject with NO inline-PII rows shreds nothing (the references-not-payloads common case).**
    /// The vast majority of erasures touch only refs-stored rows (tombstone for free, P-FLOW-03); a
    /// subject who sealed no inline PII has no per-subject DEK → no shred, no telemetry record. The
    /// receipt records `destroyed_key_epoch = None` (correct — nothing to crypto-shred).
    #[test]
    fn subject_with_no_inline_pii_shreds_no_key() {
        let kms = KmsEngine::new();
        // No inline PII sealed for u-none.
        let telemetry = FlowTelemetry::new();
        let shred = WfCryptoShred::with_telemetry(&kms, region(), &telemetry);
        let report = shred.shred_subject(
            &EraseScope::Subject {
                subject: subject("u-none"),
                tenant: tenant(),
            },
            0,
            0,
            0,
        );
        assert_eq!(
            report.destroyed_key_epoch, None,
            "no inline-PII DEK → nothing to shred (refs-stored rows tombstone for free)"
        );
        assert_eq!(
            telemetry.crypto_shreds_count(),
            0,
            "no shred recorded when there was no inline-PII key to destroy"
        );
    }

    /// **The crypto-shred PRESERVES the journal structure — replay still works (the §4.8 tombstone
    /// half).** The shred destroys the KEY, never the row: a populated journal is byte-identical after
    /// the erase (the `command_id` replay-match keys + the `seq` order + the refs survive), so
    /// deterministic replay still rebuilds the run. Only the inline-PII cell is now a tombstone.
    #[test]
    fn crypto_shred_preserves_journal_structure_replay_still_works() {
        let kms = KmsEngine::new();
        let column = seal_inline_pii(&kms, &region(), &tenancy(), &sid("u-keep"), b"pii body")
            .expect("seal");
        let journal = WfJournal::new();
        // An inline-PII history row whose result_key_ref names u-keep's per-subject DEK.
        journal.append_history_for_test(WfHistoryRow {
            tenant: TenancyTenantId::from_token("acme"),
            region: region(),
            run_id: "run-1".into(),
            seq: 0,
            kind: "activity_completed".into(),
            command_id: "agent.run:0".into(),
            result: Some(vec![ArtifactRef("myelin://acme/agent/effect/e1".into())]),
            result_key_ref: Some(column.key_ref.to_uri()),
        });
        let before = journal.history_in_tenant(&TenancyTenantId::from_token("acme"));

        let shred = WfCryptoShred::new(&kms, region());
        shred.shred_subject(
            &EraseScope::Subject {
                subject: subject("u-keep"),
                tenant: tenant(),
            },
            1,
            0,
            0,
        );

        // The journal is byte-identical: structure preserved, replay still works.
        let after = journal.history_in_tenant(&TenancyTenantId::from_token("acme"));
        assert_eq!(
            after, before,
            "the journal rows survive the shred byte-identical (structure preserved, replay works)"
        );
        // But the inline PII the row points at is now unrecoverable (the PII is a tombstone).
        assert!(
            is_inline_pii_unrecoverable(&kms, &region(), &column),
            "the inline PII the surviving row referenced is unrecoverable (the PII is a tombstone)"
        );
    }

    /// **A tenant-scope erase destroys NO per-subject key (the KEK destroy is the GDPR-side P-GA-13
    /// lever).** The cascade records `destroyed_key_epoch = None` for a `Tenant` scope and the
    /// aggregate receipt names the per-tenant KEK lever.
    #[test]
    fn tenant_scope_records_no_per_subject_shred() {
        let kms = KmsEngine::new();
        let shred = WfCryptoShred::new(&kms, region());
        let scope = EraseScope::Tenant(tenant());
        let report = shred.shred_subject(&scope, 0, 0, 0);
        assert_eq!(report.destroyed_key_epoch, None);
        let agg = aggregate_receipt(&report, &scope);
        assert_eq!(agg.receipt.operation, "erase");
        assert!(agg.receipt.content_hash.starts_with("blake3:"));
        assert!(agg.receipt.key_epoch_destroyed.is_none());
    }

    /// **The aggregate receipt folds the shred report into the frozen `EraseReceipt` (10.1) +
    /// content-addresses over the destroyed epoch.** It carries the destroyed per-subject DEK epoch in
    /// `key_epoch_destroyed` — the audit trail the crypto-shred reach now records (P-FLOW-03's `None` is
    /// replaced by the real epoch).
    #[test]
    fn aggregate_receipt_carries_the_destroyed_epoch() {
        let kms = KmsEngine::new();
        let _col = seal_inline_pii(&kms, &region(), &tenancy(), &sid("u-r"), b"pii").expect("seal");
        let shred = WfCryptoShred::new(&kms, region());
        let scope = EraseScope::Subject {
            subject: subject("u-r"),
            tenant: tenant(),
        };
        let report = shred.shred_subject(&scope, 1, 0, 0);
        let agg = aggregate_receipt(&report, &scope);
        assert_eq!(agg.receipt.operation, "erase");
        assert_eq!(agg.receipt.key_epoch_destroyed, report.destroyed_key_epoch);
        assert!(
            agg.receipt.key_epoch_destroyed.is_some(),
            "the crypto-shred reach records a destroyed epoch (P-FLOW-03's None is now filled)"
        );
        assert!(agg.receipt.content_hash.starts_with("blake3:"));
    }

    /// **The inline-PII reach predicates count BOTH the schema-tag grammar AND the pii_key_ref
    /// grammar.** `result_key_ref`/`payload_key_ref` may carry `…/subject/<id>` (the §3.2 schema tag)
    /// OR `subject:<id>` (what `seal_inline_pii` writes); ONE predicate over both (EI-01 §7).
    #[test]
    fn inline_pii_predicates_accept_both_key_ref_grammars() {
        let h_schema = WfHistoryRow {
            tenant: TenancyTenantId::from_token("acme"),
            region: region(),
            run_id: "r".into(),
            seq: 0,
            kind: "activity_completed".into(),
            command_id: "c:0".into(),
            result: None,
            result_key_ref: Some("kms://acme/subject/u-x".into()),
        };
        assert!(history_row_has_inline_pii(&h_schema, "u-x"));
        assert!(!history_row_has_inline_pii(&h_schema, "u-y"));

        let h_pii = WfHistoryRow {
            result_key_ref: Some("kms://acme/0/subject:u-x".into()),
            ..h_schema.clone()
        };
        assert!(history_row_has_inline_pii(&h_pii, "u-x"));

        let s = WfSignalRow {
            tenant: TenancyTenantId::from_token("acme"),
            region: region(),
            run_id: "r".into(),
            signal_name: "approval".into(),
            idem_key: "k".into(),
            payload: Vec::new(),
            payload_key_ref: Some("kms://acme/subject/u-z".into()),
            consumed_seq: None,
        };
        assert!(signal_row_has_inline_pii(&s, "u-z"));
        assert!(!signal_row_has_inline_pii(&s, "u-x"));
        // A refs-stored row (no key ref) is NOT inline PII — it tombstones for free (P-FLOW-03).
        let refs_only = WfHistoryRow {
            result_key_ref: None,
            ..h_schema
        };
        assert!(!history_row_has_inline_pii(&refs_only, "u-x"));
    }
}
