use std::collections::BTreeMap;

use myelin_gdpr::{
    DsrError, EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle,
    Receipt, RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};

use crate::dsr::DsrId;

pub const ERASURE_LEDGER_STORE: &str = "gdpr_erasure_ledger:erasure_ledger";

pub const ERASURE_LEDGER_ENTRIES: (&str, &str) = ("gdpr.erasure_ledger_entries", "count");

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DestroyedKeyEpoch {
    pub holder_id: String,
    pub key_epoch_destroyed: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErasureLedgerEntry {
    pub dsr_id: DsrId,
    pub subject_token: String,
    pub tenant_token: String,
    pub holders_erased: Vec<String>,
    pub key_epochs_destroyed: Vec<DestroyedKeyEpoch>,
    pub completed_at_offset: u64,
    pub completed_at_secs: u64,
}

impl ErasureLedgerEntry {
    pub fn erased_holder(&self, holder_id: &str) -> bool {
        self.holders_erased.iter().any(|h| h == holder_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostPitRecord {
    pub subject: String,
    pub tenant: String,
    pub completed_at_offset: u64,
}

#[derive(Debug, Default)]
pub struct ErasureLedger {
    entries: std::sync::Mutex<BTreeMap<DsrId, ErasureLedgerEntry>>,
}

impl ErasureLedger {
    pub fn new() -> ErasureLedger {
        ErasureLedger::default()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_completion(
        &self,
        dsr_id: DsrId,
        subject_token: String,
        tenant_token: String,
        mut holders_erased: Vec<String>,
        mut key_epochs_destroyed: Vec<DestroyedKeyEpoch>,
        completed_at_offset: u64,
        completed_at_secs: u64,
    ) -> bool {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if entries.contains_key(&dsr_id) {
            return false;
        }
        holders_erased.sort();
        holders_erased.dedup();
        key_epochs_destroyed.sort();
        key_epochs_destroyed.dedup();
        entries.insert(
            dsr_id.clone(),
            ErasureLedgerEntry {
                dsr_id,
                subject_token,
                tenant_token,
                holders_erased,
                key_epochs_destroyed,
                completed_at_offset,
                completed_at_secs,
            },
        );
        true
    }

    pub fn entry(&self, dsr_id: &DsrId) -> Option<ErasureLedgerEntry> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(dsr_id)
            .cloned()
    }

    pub fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    }

    pub fn post_pit_records_after(&self, pit: u64) -> Vec<PostPitRecord> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<PostPitRecord> = entries
            .values()
            .filter(|e| e.completed_at_offset > pit)
            .filter(|e| e.subject_token != "*")
            .map(|e| PostPitRecord {
                subject: e.subject_token.clone(),
                tenant: e.tenant_token.clone(),
                completed_at_offset: e.completed_at_offset,
            })
            .collect();
        out.sort_by(|a, b| {
            a.completed_at_offset
                .cmp(&b.completed_at_offset)
                .then_with(|| a.subject.cmp(&b.subject))
        });
        out
    }
}

impl PersonalDataHolder for ErasureLedger {
    fn locate(&self, _subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                ERASURE_LEDGER_STORE,
                "*",
                &tenant.0,
                "located:0-recoverable",
                None,
                0,
            ),
        })
    }

    fn export(&self, _subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                ERASURE_LEDGER_STORE,
                "*",
                &tenant.0,
                "exported:pii-free-no-portable-data",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, _subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Err(DsrError(
            "erasure ledger (10.8): the PII-free completion record is NEVER edited - it is the source \
             that drives post-restore re-erasure; a rectification would corrupt the re-erasure source \
             and risk a restore resurrecting an erased subject (gdpr §3.2 / ADR-18). It holds no PII to \
             rectify."
                .to_string(),
        ))
    }

    fn restrict(&self, _subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        let outcome = if on {
            "restricted:noop-pii-free"
        } else {
            "restricted:clear-noop"
        };
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                ERASURE_LEDGER_STORE,
                "*",
                "*",
                outcome,
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (subject, tenant) = match scope {
            EraseScope::Subject { subject, tenant } => {
                (subject.principal.principal_id.0.clone(), tenant.0.clone())
            }
            EraseScope::Tenant(tenant) => ("*".to_string(), tenant.0.clone()),
        };
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                ERASURE_LEDGER_STORE,
                &subject,
                &tenant,
                "carve_out:retained-pii-free-record:non-shred-erasable:drives-re-erasure",
                None,
                0,
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }

    fn subject_ref(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            tenant(),
        ))
    }

    fn epoch(holder: &str, e: Option<u64>) -> DestroyedKeyEpoch {
        DestroyedKeyEpoch {
            holder_id: holder.into(),
            key_epoch_destroyed: e,
        }
    }

    #[test]
    fn the_ledger_schema_is_pii_free() {
        let entry = ErasureLedgerEntry {
            dsr_id: DsrId("dsr:0".into()),
            subject_token: "p-opaque-123".into(),
            tenant_token: "acme".into(),
            holders_erased: vec!["oltp:identity_oltp".into()],
            key_epochs_destroyed: vec![epoch("oltp:identity_oltp", Some(7))],
            completed_at_offset: 140,
            completed_at_secs: 1_700_000_000,
        };
        assert_eq!(entry.subject_token, "p-opaque-123");
        assert!(
            !entry.subject_token.contains('@'),
            "no email form in the subject token"
        );
        let ErasureLedgerEntry {
            dsr_id: _,
            subject_token: _,
            tenant_token: _,
            holders_erased: _,
            key_epochs_destroyed: _,
            completed_at_offset: _,
            completed_at_secs: _,
        } = entry;
    }

    #[test]
    fn a_dsr_completion_writes_a_pii_free_entry() {
        let ledger = ErasureLedger::new();
        let is_new = ledger.record_completion(
            DsrId("dsr:1".into()),
            "p-7".into(),
            "acme".into(),
            vec!["oltp:identity_oltp".into(), "blob:blob_store".into()],
            vec![
                epoch("oltp:identity_oltp", Some(7)),
                epoch("blob:blob_store", Some(9)),
            ],
            140,
            1_700_000_000,
        );
        assert!(is_new, "the first completion writes a NEW entry");
        assert_eq!(ledger.len(), 1);
        let e = ledger.entry(&DsrId("dsr:1".into())).unwrap();
        assert_eq!(e.subject_token, "p-7");
        assert_eq!(e.tenant_token, "acme");
        assert!(e.erased_holder("oltp:identity_oltp"));
        assert!(e.erased_holder("blob:blob_store"));
        assert!(!e.erased_holder("nonexistent:holder"));
        assert_eq!(e.key_epochs_destroyed.len(), 2);
        assert_eq!(e.completed_at_offset, 140);
    }

    #[test]
    fn the_ledger_write_is_idempotent_per_dsr() {
        let ledger = ErasureLedger::new();
        let id = DsrId("dsr:2".into());
        assert!(ledger.record_completion(
            id.clone(),
            "p-9".into(),
            "acme".into(),
            vec!["a".into()],
            vec![epoch("a", Some(1))],
            100,
            500,
        ));
        assert!(
            !ledger.record_completion(
                id.clone(),
                "p-9".into(),
                "acme".into(),
                vec!["a".into(), "b".into()],
                vec![epoch("a", Some(1)), epoch("b", Some(2))],
                200,
                999,
            ),
            "a duplicate completion is a no-op"
        );
        assert_eq!(ledger.len(), 1, "no duplicate entry");
        let e = ledger.entry(&id).unwrap();
        assert_eq!(
            e.completed_at_offset, 100,
            "the FIRST completion's offset is retained"
        );
        assert_eq!(
            e.completed_at_secs, 500,
            "the FIRST completion's time is retained"
        );
        assert_eq!(
            e.holders_erased,
            vec!["a".to_string()],
            "the first holder set is retained"
        );
    }

    #[test]
    fn post_pit_records_selects_only_post_pit_erasures() {
        let ledger = ErasureLedger::new();
        ledger.record_completion(
            DsrId("dsr:a".into()),
            "pre".into(),
            "acme".into(),
            vec![],
            vec![],
            50,
            0,
        );
        ledger.record_completion(
            DsrId("dsr:b".into()),
            "at".into(),
            "acme".into(),
            vec![],
            vec![],
            100,
            0,
        );
        ledger.record_completion(
            DsrId("dsr:c".into()),
            "post".into(),
            "acme".into(),
            vec![],
            vec![],
            140,
            0,
        );
        ledger.record_completion(
            DsrId("dsr:d".into()),
            "later".into(),
            "acme".into(),
            vec![],
            vec![],
            200,
            0,
        );

        let after = ledger.post_pit_records_after(100);
        let subjects: Vec<&str> = after.iter().map(|r| r.subject.as_str()).collect();
        assert_eq!(subjects, vec!["post", "later"]);
        assert_eq!(after[0].tenant, "acme");
        assert_eq!(after[0].completed_at_offset, 140);
    }

    #[test]
    fn a_tenant_offboarding_is_not_a_per_subject_post_pit_record() {
        let ledger = ErasureLedger::new();
        ledger.record_completion(
            DsrId("dsr:off".into()),
            "*".into(),
            "acme".into(),
            vec![],
            vec![],
            140,
            0,
        );
        ledger.record_completion(
            DsrId("dsr:sub".into()),
            "p-1".into(),
            "acme".into(),
            vec![],
            vec![],
            140,
            0,
        );
        assert_eq!(
            ledger.len(),
            2,
            "both are recorded (the offboarding IS in the ledger for audit)"
        );
        let after = ledger.post_pit_records_after(100);
        let subjects: Vec<&str> = after.iter().map(|r| r.subject.as_str()).collect();
        assert_eq!(
            subjects,
            vec!["p-1"],
            "only the per-subject erasure is a re-erasure target"
        );
    }

    #[test]
    fn a_pre_pit_erasure_is_not_a_re_erasure_target() {
        let ledger = ErasureLedger::new();
        ledger.record_completion(
            DsrId("dsr:pre".into()),
            "p-pre".into(),
            "acme".into(),
            vec![],
            vec![],
            60,
            0,
        );
        assert!(
            ledger.post_pit_records_after(100).is_empty(),
            "a pre-PIT erasure is not re-applied"
        );
    }

    #[test]
    fn len_and_is_empty_are_exact() {
        let ledger = ErasureLedger::new();
        assert!(ledger.is_empty());
        assert_eq!(ledger.len(), 0);
        ledger.record_completion(
            DsrId("dsr:1".into()),
            "p".into(),
            "acme".into(),
            vec![],
            vec![],
            10,
            0,
        );
        assert!(!ledger.is_empty());
        assert_eq!(ledger.len(), 1);
    }

    #[test]
    fn the_ledger_erase_is_a_non_shred_erasable_carve_out() {
        let ledger = ErasureLedger::new();
        let receipt = ledger
            .erase(EraseScope::Subject {
                subject: subject_ref("p-1"),
                tenant: tenant(),
            })
            .unwrap();
        assert_eq!(
            receipt.receipt.key_epoch_destroyed, None,
            "the ledger erase destroys NO key - it is non-shred-erasable (it must survive)"
        );
        assert_eq!(receipt.receipt.operation, "erase");
        ledger.record_completion(
            DsrId("dsr:1".into()),
            "p-1".into(),
            "acme".into(),
            vec![],
            vec![],
            140,
            0,
        );
        ledger
            .erase(EraseScope::Subject {
                subject: subject_ref("p-1"),
                tenant: tenant(),
            })
            .unwrap();
        assert_eq!(
            ledger.len(),
            1,
            "the erase RETAINS the record (it drives re-erasure)"
        );
        assert!(
            !ledger.post_pit_records_after(100).is_empty(),
            "the retained record STILL drives re-erasure after the subject's erase"
        );
    }

    #[test]
    fn the_recursive_holder_read_ops_report_pii_free() {
        let ledger = ErasureLedger::new();
        let loc = ledger.locate(&subject_ref("p-1"), tenant()).unwrap();
        assert_eq!(
            loc.receipt.content_hash,
            Receipt::content_addressed(
                "locate",
                ERASURE_LEDGER_STORE,
                "*",
                "acme",
                "located:0-recoverable",
                None,
                0
            )
            .content_hash,
        );
        assert!(ledger.export(&subject_ref("p-1"), tenant()).is_ok());
        assert!(
            ledger
                .rectify(&subject_ref("p-1"), Patch("x".into()))
                .is_err(),
            "the ledger is NEVER rectified"
        );
        assert!(
            ledger.restrict(&subject_ref("p-1"), true).is_ok(),
            "restrict is a no-op ack"
        );
    }

    #[test]
    fn telemetry_name_and_unit_are_pinned() {
        assert_eq!(ERASURE_LEDGER_ENTRIES.0, "gdpr.erasure_ledger_entries");
        assert_eq!(ERASURE_LEDGER_ENTRIES.1, "count");
        assert_eq!(ERASURE_LEDGER_STORE, "gdpr_erasure_ledger:erasure_ledger");
    }
}
