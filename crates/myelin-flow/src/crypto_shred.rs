use myelin_gdpr::{EraseReceipt, EraseScope, ErasureMethod, Receipt, SubjectRef};
use myelin_storage::encryption::{ColumnCryptor, EncryptedColumn, KeyChoiceError, SubjectId};
use myelin_storage::kms::{DekId, KekId, KeyClass, KmsEngine};
use myelin_tenancy::{Region, TenantId as TenancyTenantId};

use crate::engine::FlowTelemetry;
use crate::holder::FLOW_OLTP_STORE;
use crate::schema::{WfHistoryRow, WfSignalRow};

pub fn subject_dek_erasure() -> ErasureMethod {
    ErasureMethod::CryptoShred("subject_dek".to_string())
}

pub fn subject_dek_id(tenant: &TenancyTenantId, subject_id: &str) -> DekId {
    DekId::new(tenant.clone(), KeyClass::Subject(subject_id.to_string()))
}

pub fn seal_inline_pii(
    engine: &KmsEngine,
    region: &Region,
    tenant: &TenancyTenantId,
    subject: &SubjectId,
    plaintext: &[u8],
) -> Result<EncryptedColumn, KeyChoiceError> {
    engine.ensure_kek(&KekId::new(tenant.clone(), region.clone()))?;
    let cryptor = ColumnCryptor::new(engine, region.clone());
    cryptor.encrypt(tenant, Some(subject), &subject_dek_erasure(), plaintext)
}

pub fn open_inline_pii(
    engine: &KmsEngine,
    region: &Region,
    column: &EncryptedColumn,
) -> Result<Vec<u8>, KeyChoiceError> {
    let cryptor = ColumnCryptor::new(engine, region.clone());
    cryptor.decrypt(column)
}

pub fn is_inline_pii_unrecoverable(
    engine: &KmsEngine,
    region: &Region,
    column: &EncryptedColumn,
) -> bool {
    open_inline_pii(engine, region, column).is_err()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WfShredReport {
    pub destroyed_key_epoch: Option<u64>,
    pub inline_pii_rows_shredded: usize,
    pub crypto_shred_lag_secs: u64,
}

pub struct WfCryptoShred<'a> {
    kms: &'a KmsEngine,
    region: Region,
    telemetry: Option<&'a FlowTelemetry>,
}

impl<'a> WfCryptoShred<'a> {
    pub fn new(kms: &'a KmsEngine, region: Region) -> WfCryptoShred<'a> {
        WfCryptoShred {
            kms,
            region,
            telemetry: None,
        }
    }

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
                let dek_id = subject_dek_id(&tenancy_tenant, &sid);
                let epoch = self.dek_epoch(&tenancy_tenant, &sid, &dek_id);
                self.kms.destroy_dek(&dek_id);
                epoch
            }
            EraseScope::Tenant(_) => None,
        };

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

pub fn aggregate_receipt(report: &WfShredReport, scope: &EraseScope) -> EraseReceipt {
    let (subject_token, tenant) = match scope {
        EraseScope::Subject { subject, tenant } => (subject_token(subject), tenant.0.clone()),
        EraseScope::Tenant(t) => (String::new(), t.0.clone()),
    };
    let outcome = match scope {
        EraseScope::Subject { .. } => format!(
            "crypto-shred reach (P-FLOW-24): per-subject DEK destroyed (epoch={:?}) - {} inline-PII \
             wf_history/wf_signal rows unrecoverable incl. backups, structure preserved (replay \
             works, the PII is a tombstone); crypto-shred-lag={}s; refs-stored rows tombstone for \
             free (P-FLOW-03); residual = the ONE posture 10.9/X-7 by reference",
            report.destroyed_key_epoch, report.inline_pii_rows_shredded, report.crypto_shred_lag_secs,
        ),
        EraseScope::Tenant(_) => {
            "tenant crypto-shred: destroy the per-tenant KEK (11.3/11.4) - every workflow row \
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

fn subject_token(subject: &SubjectRef) -> String {
    subject.principal.principal_id.0.clone()
}

pub fn history_row_has_inline_pii(row: &WfHistoryRow, subject_id: &str) -> bool {
    key_ref_names_subject(row.result_key_ref.as_deref(), subject_id)
}

pub fn signal_row_has_inline_pii(row: &WfSignalRow, subject_id: &str) -> bool {
    key_ref_names_subject(row.payload_key_ref.as_deref(), subject_id)
}

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

    #[test]
    fn erased_subject_inline_pii_is_unrecoverable_live_and_after_backup_restore() {
        let kms = KmsEngine::new();
        let plaintext = b"the subject's medical note inlined into a run result".to_vec();
        let column = seal_inline_pii(&kms, &region(), &tenancy(), &sid("psn:ada"), &plaintext)
            .expect("seal under the subject's per-subject DEK");
        assert!(!is_inline_pii_unrecoverable(&kms, &region(), &column));
        assert_eq!(
            open_inline_pii(&kms, &region(), &column).expect("opens"),
            plaintext
        );

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

        assert!(
            is_inline_pii_unrecoverable(&kms, &region(), &column),
            "0 recoverable: the inline PII is unrecoverable after the per-subject DEK shred"
        );
        let snapshot = kms.backup_snapshot();
        assert!(
            !snapshot
                .into_iter()
                .any(|(id, _)| id == DekId::new(tenancy(), KeyClass::Subject("psn:ada".into()))),
            "the crypto-shredded DEK is EXCLUDED from backups - a restore cannot read the PII"
        );
    }

    #[test]
    fn shred_destroys_only_the_erased_subjects_dek_not_the_tenant_or_others() {
        let kms = KmsEngine::new();
        let col1 = seal_inline_pii(&kms, &region(), &tenancy(), &sid("u-1"), b"u-1 private")
            .expect("seal u-1");
        let col2 = seal_inline_pii(&kms, &region(), &tenancy(), &sid("u-2"), b"u-2 private")
            .expect("seal u-2");
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

        assert!(
            is_inline_pii_unrecoverable(&kms, &region(), &col1),
            "u-1's inline PII is unrecoverable (their DEK was shredded)"
        );
        assert!(
            !is_inline_pii_unrecoverable(&kms, &region(), &col2),
            "u-2's inline PII still opens (a DIFFERENT subject's DEK - not touched)"
        );
        assert!(
            !is_inline_pii_unrecoverable(&kms, &region(), &tenant_col),
            "the per-tenant bulk DEK is untouched - a per-subject erase is NOT a tenant wipe"
        );
    }

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

    #[test]
    fn subject_with_no_inline_pii_shreds_no_key() {
        let kms = KmsEngine::new();
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

    #[test]
    fn crypto_shred_preserves_journal_structure_replay_still_works() {
        let kms = KmsEngine::new();
        let column = seal_inline_pii(&kms, &region(), &tenancy(), &sid("u-keep"), b"pii body")
            .expect("seal");
        let journal = WfJournal::new();
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

        let after = journal.history_in_tenant(&TenancyTenantId::from_token("acme"));
        assert_eq!(
            after, before,
            "the journal rows survive the shred byte-identical (structure preserved, replay works)"
        );
        assert!(
            is_inline_pii_unrecoverable(&kms, &region(), &column),
            "the inline PII the surviving row referenced is unrecoverable (the PII is a tombstone)"
        );
    }

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
        let refs_only = WfHistoryRow {
            result_key_ref: None,
            ..h_schema
        };
        assert!(!history_row_has_inline_pii(&refs_only, "u-x"));
    }
}
