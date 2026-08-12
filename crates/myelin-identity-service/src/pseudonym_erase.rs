use myelin_storage::{ContentHash, DekId, KmsEngine, TenantScope};
use myelin_tenancy::{Region, TenantId};
#[cfg(any(test, feature = "test-support"))]
use std::collections::BTreeMap;
use std::sync::Arc;
#[cfg(any(test, feature = "test-support"))]
use std::sync::Mutex;

use crate::pseudonym_store::{PseudonymError, PseudonymStore};
use myelin_identity::PrincipalId;

pub const ERASURE_LEDGER: &str = "identity_pseudonym_erasure_ledger";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PseudonymEraseError {
    Erased { subject: String },
    NoMapping { subject: String },
    Infrastructure { detail: String },
}

impl core::fmt::Display for PseudonymEraseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PseudonymEraseError::Erased { subject } => write!(
                f,
                "subject `{subject}` is ERASED (the pseudonym-map row + per-subject DEK are \
                 crypto-shredded) - its real identity is unrecoverable; resolve fails CLOSED, never \
                 a fabricated handle (the opaque principal_id still attributes events)"
            ),
            PseudonymEraseError::NoMapping { subject } => write!(
                f,
                "subject `{subject}` has no pseudonym mapping in the verified (tenant, region) scope \
                 (never registered, or a different partition) - refused"
            ),
            PseudonymEraseError::Infrastructure { detail } => write!(
                f,
                "pseudonym resolution could not establish mapping or erasure state - refused: \
                 {detail}"
            ),
        }
    }
}

impl std::error::Error for PseudonymEraseError {}

impl From<PseudonymError> for PseudonymEraseError {
    fn from(error: PseudonymError) -> Self {
        PseudonymEraseError::Infrastructure {
            detail: error.to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErasureReceipt {
    pub subject: PrincipalId,
    pub tenant: TenantId,
    pub region: Region,
    pub shredded_dek_class: String,
    pub dek_destroyed: bool,
    pub row_shredded: bool,
    pub erased_at: myelin_events::Timestamp,
    pub content_hash: String,
}

impl ErasureReceipt {
    pub fn for_erase(
        subject: PrincipalId,
        tenant: TenantId,
        region: Region,
        shredded_dek_class: String,
        dek_destroyed: bool,
        row_shredded: bool,
        erased_at: myelin_events::Timestamp,
    ) -> ErasureReceipt {
        let body = format!(
            "erase|{}|{}|{}|{}|{}",
            subject.0, tenant.0, region.0, shredded_dek_class, erased_at.0
        );
        let content_hash = ContentHash::blake3(body.as_bytes()).to_multihash_string();
        ErasureReceipt {
            subject,
            tenant,
            region,
            shredded_dek_class,
            dek_destroyed,
            row_shredded,
            erased_at,
            content_hash,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReErasureReceipt {
    pub tenant: TenantId,
    pub region: Region,
    pub re_erased: usize,
    pub resurrected: usize,
    pub pre_pass_resurrected: usize,
    pub per_subject: Vec<ErasureReceipt>,
    pub ran_at: myelin_events::Timestamp,
}

impl ReErasureReceipt {
    pub fn is_green(&self) -> bool {
        self.resurrected == 0
    }

    pub(crate) fn with_pre_pass_resurrected(mut self, pre_pass: usize) -> ReErasureReceipt {
        self.pre_pass_resurrected = pre_pass;
        self
    }

    pub fn summary(&self) -> String {
        format!(
            "ID-D8 re-erasure [{}]: tenant={} region={} re_erased={} \
             pre_pass_resurrected={} resurrected={} → {}",
            self.ran_at.0,
            self.tenant.0,
            self.region.0,
            self.re_erased,
            self.pre_pass_resurrected,
            self.resurrected,
            if self.is_green() { "GREEN" } else { "RED" },
        )
    }
}

#[cfg(any(test, feature = "test-support"))]
type LedgerByPartition = BTreeMap<(String, String), BTreeMap<String, ErasureLedgerEntry>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErasureLedgerEntry {
    pub subject: PrincipalId,
    pub dek_class: myelin_storage::KeyClass,
    pub erased_at: myelin_events::Timestamp,
}

#[derive(Clone)]
pub struct PseudonymErasureLedger {
    backend: ErasureLedgerBackend,
}

#[derive(Clone)]
enum ErasureLedgerBackend {
    #[cfg(any(test, feature = "test-support"))]
    Memory(Arc<Mutex<LedgerByPartition>>),
    Pg(PgErasureLedgerBacking),
}

#[derive(Clone)]
struct PgErasureLedgerBacking {
    backing: Arc<myelin_storage::DurableErasureLedgerBacking>,
    rt: tokio::runtime::Handle,
}

impl PgErasureLedgerBacking {
    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

impl PseudonymErasureLedger {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new() -> PseudonymErasureLedger {
        PseudonymErasureLedger {
            backend: ErasureLedgerBackend::Memory(Arc::new(Mutex::new(LedgerByPartition::new()))),
        }
    }

    pub fn with_pg(
        backing: myelin_storage::DurableErasureLedgerBacking,
        rt: tokio::runtime::Handle,
    ) -> PseudonymErasureLedger {
        PseudonymErasureLedger {
            backend: ErasureLedgerBackend::Pg(PgErasureLedgerBacking {
                backing: Arc::new(backing),
                rt,
            }),
        }
    }

    pub fn record(
        &self,
        scope: &TenantScope,
        subject: &PrincipalId,
        dek_class: myelin_storage::KeyClass,
        erased_at: myelin_events::Timestamp,
    ) {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            ErasureLedgerBackend::Memory(inner_arc) => {
                let part = Self::part_key(scope);
                let entry = ErasureLedgerEntry {
                    subject: subject.clone(),
                    dek_class,
                    erased_at,
                };
                inner_arc
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .entry(part)
                    .or_default()
                    .insert(subject.0.clone(), entry);
            }
            ErasureLedgerBackend::Pg(pg) => {
                if let Err(e) = pg.block(pg.backing.record(
                    &scope.tenant().0,
                    &subject.0,
                    &dek_class.as_token(),
                    &erased_at.0,
                )) {
                    panic!(
                        "ERASURE-LEDGER DURABILITY FAILURE (fail-static): the erasure record for \
                         subject={} tenant={} could NOT be persisted - an unrecorded erasure is a \
                         silent resurrection path across PIT restore (P-ID-20/ID-D8); refusing to \
                         report the erasure as recorded: {e}",
                        subject.0,
                        scope.tenant().0
                    );
                }
            }
        }
    }

    pub fn entries_in(&self, scope: &TenantScope) -> Vec<ErasureLedgerEntry> {
        self.try_entries_in(scope)
            .unwrap_or_else(|e| panic!("erasure ledger: replay read failed loud: {e}"))
    }

    pub fn try_entries_in(
        &self,
        scope: &TenantScope,
    ) -> Result<Vec<ErasureLedgerEntry>, PseudonymError> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            ErasureLedgerBackend::Memory(inner_arc) => Ok(inner_arc
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&Self::part_key(scope))
                .map(|m| m.values().cloned().collect())
                .unwrap_or_default()),
            ErasureLedgerBackend::Pg(pg) => pg
                .block(pg.backing.entries_in(&scope.tenant().0))
                .map_err(|error| PseudonymError::Storage(error.to_string()))?
                .into_iter()
                .map(Self::durable_to_entry)
                .collect(),
        }
    }

    fn durable_to_entry(
        row: myelin_storage::DurableErasureLedgerRow,
    ) -> Result<ErasureLedgerEntry, PseudonymError> {
        let dek_class = myelin_storage::KeyClass::parse_token(&row.dek_class).ok_or_else(|| {
            PseudonymError::Storage(format!(
                "malformed erasure-ledger DEK class `{}`",
                row.dek_class
            ))
        })?;
        Ok(ErasureLedgerEntry {
            subject: PrincipalId(row.subject),
            dek_class,
            erased_at: myelin_events::Timestamp(row.erased_at),
        })
    }

    pub fn is_erased(&self, scope: &TenantScope, subject: &PrincipalId) -> bool {
        self.try_is_erased(scope, subject)
            .unwrap_or_else(|e| panic!("erasure ledger: state read failed loud: {e}"))
    }

    pub fn try_is_erased(
        &self,
        scope: &TenantScope,
        subject: &PrincipalId,
    ) -> Result<bool, PseudonymError> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            ErasureLedgerBackend::Memory(inner_arc) => Ok(inner_arc
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&Self::part_key(scope))
                .map(|m| m.contains_key(&subject.0))
                .unwrap_or(false)),
            ErasureLedgerBackend::Pg(pg) => pg
                .block(pg.backing.is_erased(&scope.tenant().0, &subject.0))
                .map_err(|error| PseudonymError::Storage(error.to_string())),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    fn part_key(scope: &TenantScope) -> (String, String) {
        (scope.tenant().0.clone(), scope.region().0.clone())
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Default for PseudonymErasureLedger {
    fn default() -> PseudonymErasureLedger {
        PseudonymErasureLedger::new()
    }
}

pub(crate) struct EraseEngine;

impl EraseEngine {
    pub(crate) fn shred(
        store: &PseudonymStore,
        kms: &KmsEngine,
        scope: &TenantScope,
        subject: &PrincipalId,
        dek_class: &myelin_storage::KeyClass,
    ) -> Result<(bool, bool, String), PseudonymError> {
        let dek_id = DekId::new(scope.tenant().clone(), dek_class.clone());
        let dek_destroyed = kms
            .destroy_dek(&dek_id)
            .map_err(|error| PseudonymError::Storage(error.to_string()))?;
        let row_shredded = store.shred_row(scope, subject);
        Ok((dek_destroyed, row_shredded, dek_class.as_token()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalKind};
    use myelin_storage::KeyClass;

    fn scope(tenant: &str, region: &str) -> TenantScope {
        let p = Principal::stub(
            PrincipalId("admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        );
        TenantScope::from_verified_token(&p, Region(region.into()))
    }

    fn ts(s: &str) -> myelin_events::Timestamp {
        myelin_events::Timestamp(s.into())
    }

    #[test]
    fn erasure_ledger_records_and_remembers() {
        let ledger = PseudonymErasureLedger::new();
        let s = scope("acme", "eu-west");
        let subject = PrincipalId("p:alice".into());
        assert!(
            !ledger
                .try_is_erased(&s, &subject)
                .expect("ledger state read succeeds"),
            "not erased before record"
        );
        ledger.record(
            &s,
            &subject,
            KeyClass::Subject("p:alice".into()),
            ts("2026-06-19T00:00:00Z"),
        );
        assert!(
            ledger.is_erased(&s, &subject),
            "the ledger remembers the erasure"
        );
        let entries = ledger
            .try_entries_in(&s)
            .expect("ledger replay read succeeds");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].subject, subject);
        assert_eq!(entries[0].dek_class, KeyClass::Subject("p:alice".into()));
    }

    #[test]
    fn erasure_ledger_is_partitioned() {
        let ledger = PseudonymErasureLedger::new();
        let acme = scope("acme", "eu-west");
        let globex = scope("globex", "eu-west");
        let acme_us = scope("acme", "us-east");
        let subject = PrincipalId("p:alice".into());
        ledger.record(
            &acme,
            &subject,
            KeyClass::Subject("p:alice".into()),
            ts("t"),
        );
        assert!(ledger.is_erased(&acme, &subject));
        assert!(
            !ledger.is_erased(&globex, &subject),
            "no cross-tenant ledger read"
        );
        assert!(
            !ledger.is_erased(&acme_us, &subject),
            "no cross-region ledger read"
        );
        assert!(ledger.entries_in(&globex).is_empty());
    }

    #[test]
    fn erasure_ledger_record_is_idempotent() {
        let ledger = PseudonymErasureLedger::new();
        let s = scope("acme", "eu-west");
        let subject = PrincipalId("p:alice".into());
        ledger.record(&s, &subject, KeyClass::Subject("p:alice".into()), ts("t1"));
        ledger.record(&s, &subject, KeyClass::Subject("p:alice".into()), ts("t2"));
        let entries = ledger.entries_in(&s);
        assert_eq!(entries.len(), 1, "a re-record does not duplicate");
        assert_eq!(entries[0].erased_at, ts("t2"), "the timestamp updates");
    }

    #[test]
    fn erase_receipt_is_dated_content_addressed_and_pii_free() {
        let r = ErasureReceipt::for_erase(
            PrincipalId("p:alice".into()),
            TenantId("acme".into()),
            Region("eu-west".into()),
            "subject:p:alice".into(),
            true,
            true,
            ts("2026-06-19T00:00:00Z"),
        );
        assert_eq!(r.erased_at, ts("2026-06-19T00:00:00Z"), "dated");
        assert!(
            r.content_hash.starts_with("blake3:"),
            "content-addressed: {}",
            r.content_hash
        );
        let r2 = ErasureReceipt::for_erase(
            PrincipalId("p:alice".into()),
            TenantId("acme".into()),
            Region("eu-west".into()),
            "subject:p:alice".into(),
            true,
            true,
            ts("2026-06-19T00:00:00Z"),
        );
        assert_eq!(
            r.content_hash, r2.content_hash,
            "deterministic content-address"
        );
        let r3 = ErasureReceipt::for_erase(
            PrincipalId("p:bob".into()),
            TenantId("acme".into()),
            Region("eu-west".into()),
            "subject:p:bob".into(),
            true,
            true,
            ts("2026-06-19T00:00:00Z"),
        );
        assert_ne!(r.content_hash, r3.content_hash);
    }

    #[test]
    fn re_erasure_receipt_green_iff_zero_resurrected() {
        let green = ReErasureReceipt {
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            re_erased: 3,
            resurrected: 0,
            pre_pass_resurrected: 3,
            per_subject: Vec::new(),
            ran_at: ts("2026-06-19T00:00:00Z"),
        };
        assert!(green.is_green(), "0 resurrected ⇒ green");
        assert!(green.summary().contains("GREEN"));
        assert!(green.summary().contains("re_erased=3"));
        assert!(green.summary().contains("2026-06-19T00:00:00Z"), "dated");

        let red = ReErasureReceipt {
            resurrected: 1,
            ..green
        };
        assert!(
            !red.is_green(),
            "a resurrected subject ⇒ RED (never softened)"
        );
        assert!(red.summary().contains("RED"));
    }

    #[test]
    fn erase_errors_render_loud_distinct_messages() {
        let erased = PseudonymEraseError::Erased {
            subject: "p:alice".into(),
        }
        .to_string();
        let no_map = PseudonymEraseError::NoMapping {
            subject: "p:bob".into(),
        }
        .to_string();
        assert!(erased.contains("ERASED"), "{erased}");
        assert!(erased.contains("fails CLOSED"), "{erased}");
        assert!(no_map.contains("no pseudonym mapping"), "{no_map}");
        assert_ne!(erased, no_map);
        let infrastructure = PseudonymEraseError::from(PseudonymError::CorruptMapping).to_string();
        assert!(
            infrastructure.contains("could not establish"),
            "{infrastructure}"
        );
    }

    #[test]
    fn malformed_durable_ledger_key_invalidates_replay() {
        let row = myelin_storage::DurableErasureLedgerRow {
            subject: "p:alice".into(),
            dek_class: "not-a-key-class".into(),
            erased_at: "2026-06-19T00:00:00Z".into(),
        };
        assert!(matches!(
            PseudonymErasureLedger::durable_to_entry(row),
            Err(PseudonymError::Storage(message)) if message.contains("malformed erasure-ledger")
        ));
    }
}
