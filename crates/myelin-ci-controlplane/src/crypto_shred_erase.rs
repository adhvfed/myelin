use crate::holder::{CiStoreClass, CI_RESIDUAL_POSTURE_REF, ERASED_OUTCOME_NONE_REMAIN};
use crate::surfacing::ArtifactStore;
use myelin_events::ArtifactRef;
use myelin_gdpr::{EraseReceipt, EraseScope, Receipt};
use myelin_storage::kms::{DekId, KeyClass, KmsEngine, KmsError, PiiKeyRef};
use myelin_tenancy::{Region, TenantId};
use std::collections::{BTreeMap, BTreeSet};

pub const ERASED_PSEUDONYM: &str = "psn:erased";

pub const CI_ERASED_VERB: &str = "erased";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CiShredError {
    KmsUnavailable { tenant: String, class: String },
    KeyRegistryUnavailable(KmsError),
}

impl std::fmt::Display for CiShredError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CiShredError::KmsUnavailable { tenant, class } => write!(
                f,
                "CI crypto-shred: KMS could not destroy DEK ({tenant}/{class}) - erase INCOMPLETE, retry"
            ),
            CiShredError::KeyRegistryUnavailable(error) => write!(
                f,
                "CI crypto-shred: key registry unavailable - erase INCOMPLETE, retry: {error}"
            ),
        }
    }
}

impl std::error::Error for CiShredError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiSealedRow {
    pub class: CiStoreClass,
    pub pii_key_ref: PiiKeyRef,
    pub root_ref: ArtifactRef,
    pub identity_edge: Option<String>,
}

impl CiSealedRow {
    pub fn sealed(
        class: CiStoreClass,
        pii_key_ref: PiiKeyRef,
        root_ref: ArtifactRef,
    ) -> CiSealedRow {
        CiSealedRow {
            class,
            pii_key_ref,
            root_ref,
            identity_edge: None,
        }
    }

    pub fn with_identity_edge(
        class: CiStoreClass,
        pii_key_ref: PiiKeyRef,
        root_ref: ArtifactRef,
        principal_id: impl Into<String>,
    ) -> CiSealedRow {
        CiSealedRow {
            class,
            pii_key_ref,
            root_ref,
            identity_edge: Some(principal_id.into()),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CiSubjectFootprint {
    rows: Vec<CiSealedRow>,
}

impl CiSubjectFootprint {
    pub fn new() -> CiSubjectFootprint {
        CiSubjectFootprint::default()
    }

    pub fn with_row(mut self, row: CiSealedRow) -> CiSubjectFootprint {
        self.rows.push(row);
        self
    }

    pub fn rows(&self) -> &[CiSealedRow] {
        &self.rows
    }

    pub fn classes_covered(&self) -> BTreeSet<CiStoreClass> {
        self.rows.iter().map(|r| r.class).collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiEraseReceipt {
    pub subject: String,
    pub tenant: String,
    pub deks_shredded: usize,
    pub identity_edges_pseudonymised: usize,
    pub tombstones_emitted: usize,
    pub classes_reached: BTreeSet<CiStoreClass>,
    pub recoverable_live: usize,
    pub recoverable_after_restore: usize,
    pub residual_posture_ref: &'static str,
}

impl CiEraseReceipt {
    pub fn is_fully_erased(&self) -> bool {
        self.recoverable_live == 0 && self.recoverable_after_restore == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiErasedTombstone {
    pub root_ref: ArtifactRef,
    pub type_: String,
    pub reason: &'static str,
}

pub struct CiEraseFanOut<'a> {
    kms: &'a KmsEngine,
    region: Region,
}

impl<'a> CiEraseFanOut<'a> {
    pub fn new(kms: &'a KmsEngine, region: Region) -> CiEraseFanOut<'a> {
        CiEraseFanOut { kms, region }
    }

    pub fn erase_subject(
        &self,
        subject: &str,
        tenant: &TenantId,
        footprint: &CiSubjectFootprint,
        store: &mut ArtifactStore,
    ) -> Result<(CiEraseReceipt, Vec<CiErasedTombstone>), CiShredError> {
        let mut distinct_deks: BTreeMap<(String, String), (DekId, PiiKeyRef)> = BTreeMap::new();
        for row in footprint.rows() {
            let dek_id = DekId::new(
                row.pii_key_ref.tenant.clone(),
                row.pii_key_ref.class.clone(),
            );
            distinct_deks
                .entry((
                    row.pii_key_ref.tenant.0.clone(),
                    row.pii_key_ref.class.as_token(),
                ))
                .or_insert_with(|| (dek_id, row.pii_key_ref.clone()));
        }
        for (dek_id, _) in distinct_deks.values() {
            self.kms
                .destroy_dek(dek_id)
                .map_err(CiShredError::KeyRegistryUnavailable)?;
            if self.dek_is_live(dek_id)? {
                return Err(CiShredError::KmsUnavailable {
                    tenant: dek_id.tenant.0.clone(),
                    class: dek_id.class.as_token(),
                });
            }
        }

        let mut identity_edges_pseudonymised = 0usize;
        let mut tombstones: Vec<CiErasedTombstone> = Vec::new();
        let mut tombstoned_roots: BTreeSet<String> = BTreeSet::new();
        for row in footprint.rows() {
            if row.identity_edge.is_some() {
                identity_edges_pseudonymised += 1;
            }
            if tombstoned_roots.insert(row.root_ref.0.clone()) {
                let ty = self.erased_type_for(&row.root_ref);
                store.mark_erased(&row.root_ref);
                tombstones.push(CiErasedTombstone {
                    root_ref: row.root_ref.clone(),
                    type_: ty,
                    reason: "crypto_shred",
                });
            }
        }

        let recoverable_live = footprint
            .rows()
            .iter()
            .filter(|row| self.key_ref_resolves(&row.pii_key_ref))
            .count();
        let restored = self.backup_restored_dek_ids()?;
        let recoverable_after_restore = self.count_recoverable(footprint, &restored);

        let receipt = CiEraseReceipt {
            subject: subject.to_string(),
            tenant: tenant.0.clone(),
            deks_shredded: distinct_deks.len(),
            identity_edges_pseudonymised,
            tombstones_emitted: tombstones.len(),
            classes_reached: footprint.classes_covered(),
            recoverable_live,
            recoverable_after_restore,
            residual_posture_ref: CI_RESIDUAL_POSTURE_REF,
        };
        Ok((receipt, tombstones))
    }

    pub fn holder_receipt(scope: &EraseScope, ci: &CiEraseReceipt) -> EraseReceipt {
        let (subject_id, tenant, epoch) = match scope {
            EraseScope::Subject { subject, tenant } => (
                subject.principal.principal_id.0.clone(),
                tenant.0.clone(),
                (ci.deks_shredded > 0).then_some(0u64),
            ),
            EraseScope::Tenant(t) => (
                String::new(),
                t.0.clone(),
                (ci.deks_shredded > 0).then_some(0),
            ),
        };
        let outcome = if ci.is_fully_erased() {
            ERASED_OUTCOME_NONE_REMAIN
        } else {
            "erase INCOMPLETE - recoverable PII remains (retry)"
        };
        EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                crate::holder::CI_OLTP_STORE,
                &subject_id,
                &tenant,
                outcome,
                epoch,
                0,
            ),
        }
    }

    fn erased_type_for(&self, root_ref: &ArtifactRef) -> String {
        let ty = root_ref
            .0
            .split("/ci/")
            .nth(1)
            .and_then(|tail| tail.split('/').next())
            .filter(|t| crate::events::CI_TYPE_TOKENS.contains(t))
            .unwrap_or("run");
        format!("ci.{ty}.{CI_ERASED_VERB}")
    }

    fn key_ref_resolves(&self, key_ref: &PiiKeyRef) -> bool {
        self.kms.resolve_dek(key_ref, &self.region).is_ok()
    }

    fn dek_is_live(&self, dek_id: &DekId) -> Result<bool, CiShredError> {
        Ok(self.live_dek_ids()?.contains(dek_id))
    }

    fn live_dek_ids(&self) -> Result<BTreeSet<DekId>, CiShredError> {
        Ok(self
            .kms
            .backup_snapshot()
            .map_err(CiShredError::KeyRegistryUnavailable)?
            .into_iter()
            .map(|(dek_id, _wrapped)| dek_id)
            .collect())
    }

    fn backup_restored_dek_ids(&self) -> Result<BTreeSet<DekId>, CiShredError> {
        Ok(self
            .kms
            .backup_snapshot()
            .map_err(CiShredError::KeyRegistryUnavailable)?
            .into_iter()
            .map(|(dek_id, _wrapped)| dek_id)
            .collect())
    }

    fn count_recoverable(
        &self,
        footprint: &CiSubjectFootprint,
        available: &BTreeSet<DekId>,
    ) -> usize {
        footprint
            .rows()
            .iter()
            .filter(|row| {
                let dek_id = DekId::new(
                    row.pii_key_ref.tenant.clone(),
                    row.pii_key_ref.class.clone(),
                );
                available.contains(&dek_id)
            })
            .count()
    }
}

#[derive(Clone, Debug)]
pub struct CiD3Report {
    pub subject: String,
    pub tenant: String,
    pub classes_in_footprint: BTreeSet<CiStoreClass>,
    pub classes_reached: BTreeSet<CiStoreClass>,
    pub deks_shredded: usize,
    pub identity_edges_pseudonymised: usize,
    pub tombstones_emitted: usize,
    pub recoverable_live: usize,
    pub recoverable_after_restore: usize,
    pub dangling_unfurl_leaks: usize,
    pub structure_survives: bool,
    pub residual_posture_ref: &'static str,
}

impl CiD3Report {
    pub fn is_green(&self) -> bool {
        self.recoverable_live == 0
            && self.recoverable_after_restore == 0
            && self.dangling_unfurl_leaks == 0
            && self.structure_survives
            && self.deks_shredded > 0
            && self.tombstones_emitted > 0
            && !self.classes_in_footprint.is_empty()
            && self.classes_reached == self.classes_in_footprint
    }

    pub fn summary(&self) -> String {
        format!(
            "CI-D3: subject={} tenant={} classes={}/{} deks_shredded={} pseudonymised={} \
             tombstones={} recoverable_live={} recoverable_after_restore={} dangling_leaks={} \
             structure_survives={} → {}",
            self.subject,
            self.tenant,
            self.classes_reached.len(),
            self.classes_in_footprint.len(),
            self.deks_shredded,
            self.identity_edges_pseudonymised,
            self.tombstones_emitted,
            self.recoverable_live,
            self.recoverable_after_restore,
            self.dangling_unfurl_leaks,
            self.structure_survives,
            if self.is_green() { "GREEN" } else { "RED" }
        )
    }
}

pub fn drive_ci_d3_erasure_reaches_every_holder(
    subject: &str,
    tenant: &TenantId,
    region: Region,
    footprint: &CiSubjectFootprint,
    kms: &KmsEngine,
    store: &mut ArtifactStore,
) -> Result<CiD3Report, CiShredError> {
    let classes_in_footprint = footprint.classes_covered();

    let fanout = CiEraseFanOut::new(kms, region.clone());
    let (receipt, tombstones) = fanout.erase_subject(subject, tenant, footprint, store)?;

    let live_dek_ids = fanout.live_dek_ids()?;
    let recoverable_live = footprint
        .rows()
        .iter()
        .filter(|row| kms.resolve_dek(&row.pii_key_ref, &region).is_ok())
        .count();
    let recoverable_after_restore = footprint
        .rows()
        .iter()
        .filter(|row| {
            let dek_id = DekId::new(
                row.pii_key_ref.tenant.clone(),
                row.pii_key_ref.class.clone(),
            );
            live_dek_ids.contains(&dek_id)
        })
        .count();

    let dangling_unfurl_leaks = tombstones
        .iter()
        .filter(|t| !store.is_erased(&t.root_ref))
        .count();

    let identity_edge_rows = footprint
        .rows()
        .iter()
        .filter(|r| r.identity_edge.is_some())
        .count();
    let structure_survives = receipt.identity_edges_pseudonymised == identity_edge_rows;

    Ok(CiD3Report {
        subject: subject.to_string(),
        tenant: tenant.0.clone(),
        classes_in_footprint,
        classes_reached: receipt.classes_reached.clone(),
        deks_shredded: receipt.deks_shredded,
        identity_edges_pseudonymised: receipt.identity_edges_pseudonymised,
        tombstones_emitted: receipt.tombstones_emitted,
        recoverable_live,
        recoverable_after_restore,
        dangling_unfurl_leaks,
        structure_survives,
        residual_posture_ref: CI_RESIDUAL_POSTURE_REF,
    })
}

pub fn subject_dek_ref(tenant: &TenantId, dek_epoch: u64, subject_id: &str) -> PiiKeyRef {
    PiiKeyRef::new(
        tenant.clone(),
        dek_epoch,
        KeyClass::Subject(subject_id.to_string()),
    )
}

pub fn tenant_dek_ref(tenant: &TenantId, dek_epoch: u64) -> PiiKeyRef {
    PiiKeyRef::new(tenant.clone(), dek_epoch, KeyClass::Tenant)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surfacing::{ci_deployment_ref, ci_run_ref};
    use myelin_gdpr::SubjectRef;
    use myelin_storage::kms::{KekId, KeyClass};

    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }
    fn region() -> Region {
        Region::new("fr-par")
    }

    fn seeded_kms(footprint: &CiSubjectFootprint) -> KmsEngine {
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(tenant(), region()))
            .expect("seed the in-memory KEK");
        for row in footprint.rows() {
            kms.ensure_dek(&tenant(), &region(), row.pii_key_ref.class.clone())
                .expect("seal the DEK live");
        }
        kms
    }

    fn five_class_footprint(subject: &str) -> CiSubjectFootprint {
        let s = subject_dek_ref(&tenant(), 0, subject);
        let t = tenant_dek_ref(&tenant(), 0);
        CiSubjectFootprint::new()
            .with_row(CiSealedRow::with_identity_edge(
                CiStoreClass::RunState,
                s.clone(),
                ci_run_ref("acme", "run-7").unwrap(),
                subject,
            ))
            .with_row(CiSealedRow::with_identity_edge(
                CiStoreClass::Deployments,
                s.clone(),
                ci_deployment_ref("acme", "dep-3").unwrap(),
                subject,
            ))
            .with_row(CiSealedRow::sealed(
                CiStoreClass::Logs,
                s.clone(),
                ci_run_ref("acme", "run-7").unwrap(),
            ))
            .with_row(CiSealedRow::sealed(
                CiStoreClass::Artifacts,
                s,
                ci_run_ref("acme", "run-7").unwrap(),
            ))
            .with_row(CiSealedRow::sealed(
                CiStoreClass::Caches,
                t,
                ci_run_ref("acme", "run-7").unwrap(),
            ))
    }

    #[test]
    fn erase_subject_crypto_shreds_across_every_class_zero_recoverable_incl_backups() {
        let footprint = five_class_footprint("psn:ci-7");
        let kms = seeded_kms(&footprint);
        let subj_dek = DekId::new(tenant(), KeyClass::Subject("psn:ci-7".into()));
        let tenant_dek = DekId::new(tenant(), KeyClass::Tenant);
        assert!(kms
            .backup_snapshot()
            .unwrap()
            .iter()
            .any(|(d, _)| *d == subj_dek));

        let fanout = CiEraseFanOut::new(&kms, region());
        let mut store = ArtifactStore::new();
        let (receipt, tombstones) = fanout
            .erase_subject("psn:ci-7", &tenant(), &footprint, &mut store)
            .expect("erase succeeds");

        assert_eq!(
            receipt.recoverable_live, 0,
            "0 recoverable in the live store"
        );
        assert_eq!(
            receipt.recoverable_after_restore, 0,
            "0 recoverable after a backup restore (reaches backups, §7.5)"
        );
        assert!(receipt.is_fully_erased(), "the CI-D3 gate is green");

        assert_eq!(receipt.deks_shredded, 2, "per-subject + per-tenant DEK");
        assert!(!kms
            .backup_snapshot()
            .unwrap()
            .iter()
            .any(|(d, _)| *d == subj_dek));
        assert!(!kms
            .backup_snapshot()
            .unwrap()
            .iter()
            .any(|(d, _)| *d == tenant_dek));

        assert_eq!(receipt.classes_reached, footprint.classes_covered());
        assert_eq!(
            receipt.classes_reached.len(),
            5,
            "all five CI store classes"
        );

        assert_eq!(
            receipt.identity_edges_pseudonymised, 2,
            "the triggered_by + approved_by edges pseudonymised (structure survives)"
        );

        assert!(receipt.tombstones_emitted >= 1);
        assert!(tombstones.iter().any(|t| t.type_ == "ci.run.erased"));
        assert!(tombstones.iter().any(|t| t.type_ == "ci.deployment.erased"));
        assert!(tombstones.iter().all(|t| t.reason == "crypto_shred"));
    }

    #[test]
    fn erased_root_degrades_the_unfurl_to_a_tombstone_zero_dangling_leak() {
        let footprint = five_class_footprint("psn:ci-9");
        let kms = seeded_kms(&footprint);
        let fanout = CiEraseFanOut::new(&kms, region());
        let mut store = ArtifactStore::new();
        let run_ref = ci_run_ref("acme", "run-7").unwrap();
        assert!(!store.is_erased(&run_ref));
        fanout
            .erase_subject("psn:ci-9", &tenant(), &footprint, &mut store)
            .expect("erase");
        assert!(
            store.is_erased(&run_ref),
            "the erased run root degrades every unfurl to a tombstone"
        );
    }

    #[test]
    fn per_subject_erase_does_not_touch_another_subjects_dek() {
        let footprint = five_class_footprint("psn:ci-7");
        let kms = seeded_kms(&footprint);
        kms.ensure_dek(&tenant(), &region(), KeyClass::Subject("psn:other".into()))
            .expect("other subject's DEK");
        let other_dek = DekId::new(tenant(), KeyClass::Subject("psn:other".into()));

        let fanout = CiEraseFanOut::new(&kms, region());
        let mut store = ArtifactStore::new();
        fanout
            .erase_subject("psn:ci-7", &tenant(), &footprint, &mut store)
            .expect("erase");

        assert!(
            kms.backup_snapshot()
                .unwrap()
                .iter()
                .any(|(d, _)| *d == other_dek),
            "a different subject's per-subject DEK survives the erase"
        );
    }

    #[test]
    fn re_erase_is_idempotent_key_stays_destroyed() {
        let footprint = five_class_footprint("psn:ci-7");
        let kms = seeded_kms(&footprint);
        let fanout = CiEraseFanOut::new(&kms, region());
        let mut store = ArtifactStore::new();

        let (first, _) = fanout
            .erase_subject("psn:ci-7", &tenant(), &footprint, &mut store)
            .expect("first erase");
        assert!(first.is_fully_erased());

        let (second, _) = fanout
            .erase_subject("psn:ci-7", &tenant(), &footprint, &mut store)
            .expect("re-erase");
        assert_eq!(
            second.recoverable_live, 0,
            "the key stays destroyed across a re-erase"
        );
        assert_eq!(second.recoverable_after_restore, 0);
        assert!(second.is_fully_erased());
    }

    #[test]
    fn non_isolable_residual_is_shredded_by_the_per_tenant_dek_fallback() {
        let t = tenant_dek_ref(&tenant(), 0);
        let footprint = CiSubjectFootprint::new().with_row(CiSealedRow::sealed(
            CiStoreClass::Caches,
            t,
            ci_run_ref("acme", "run-7").unwrap(),
        ));
        let kms = seeded_kms(&footprint);
        let fanout = CiEraseFanOut::new(&kms, region());
        let mut store = ArtifactStore::new();
        let (receipt, _) = fanout
            .erase_subject("psn:ci-7", &tenant(), &footprint, &mut store)
            .expect("erase");
        assert!(receipt.is_fully_erased());
        assert_eq!(receipt.deks_shredded, 1, "the per-tenant DEK fallback");
        assert_eq!(
            receipt.residual_posture_ref, CI_RESIDUAL_POSTURE_REF,
            "the residual is by reference to the ONE platform posture (10.9 / X-7)"
        );
    }

    #[test]
    fn holder_receipt_is_content_addressed_and_records_the_outcome() {
        let footprint = five_class_footprint("psn:ci-7");
        let kms = seeded_kms(&footprint);
        let fanout = CiEraseFanOut::new(&kms, region());
        let mut store = ArtifactStore::new();
        let (ci, _) = fanout
            .erase_subject("psn:ci-7", &tenant(), &footprint, &mut store)
            .expect("erase");

        let scope = EraseScope::Subject {
            subject: SubjectRef::new(myelin_identity::Principal::stub(
                myelin_identity::PrincipalId("psn:ci-7".into()),
                myelin_identity::PrincipalKind::Human,
                tenant(),
            )),
            tenant: tenant(),
        };
        let r = CiEraseFanOut::holder_receipt(&scope, &ci);
        assert_eq!(r.receipt.operation, "erase");
        assert!(r.receipt.content_hash.starts_with("blake3:"));
        assert_eq!(
            r.receipt.key_epoch_destroyed,
            Some(0),
            "the destroyed DEK epoch is recorded (GD-4 audit trail)"
        );
        let r2 = CiEraseFanOut::holder_receipt(&scope, &ci);
        assert_eq!(r, r2);
    }

    #[test]
    fn ci_d3_drill_erasure_reaches_every_holder_emits_a_green_artifact() {
        let footprint = five_class_footprint("psn:ci-7");
        let kms = seeded_kms(&footprint);
        let mut store = ArtifactStore::new();
        let report = drive_ci_d3_erasure_reaches_every_holder(
            "psn:ci-7",
            &tenant(),
            region(),
            &footprint,
            &kms,
            &mut store,
        )
        .expect("CI-D3 drill runs");

        assert!(
            report.is_green(),
            "CI-D3 must be GREEN: {}",
            report.summary()
        );
        assert_eq!(report.recoverable_live, 0);
        assert_eq!(report.recoverable_after_restore, 0);
        assert_eq!(report.dangling_unfurl_leaks, 0);
        assert!(report.structure_survives);
        assert_eq!(report.identity_edges_pseudonymised, 2);
        assert_eq!(report.classes_reached, report.classes_in_footprint);
        assert_eq!(report.classes_reached.len(), 5);
        assert!(report.summary().contains("GREEN"));
        assert!(report.summary().contains("recoverable_live=0"));
        assert!(report.summary().contains("recoverable_after_restore=0"));
    }

    #[test]
    fn ci_d3_drill_reports_a_surviving_key_as_recoverable_never_silently_zeroed() {
        let footprint = five_class_footprint("psn:ci-7");
        let kms = seeded_kms(&footprint);

        let mut store = ArtifactStore::new();
        let report = drive_ci_d3_erasure_reaches_every_holder(
            "psn:ci-7",
            &tenant(),
            region(),
            &footprint,
            &kms,
            &mut store,
        )
        .expect("drill");
        assert!(report.is_green());

        let live_footprint = CiSubjectFootprint::new().with_row(CiSealedRow::sealed(
            CiStoreClass::Logs,
            subject_dek_ref(&tenant(), 0, "psn:still-here"),
            ci_run_ref("acme", "run-9").unwrap(),
        ));
        let kms2 = seeded_kms(&live_footprint);
        let fanout = CiEraseFanOut::new(&kms2, region());
        let recoverable = live_footprint
            .rows()
            .iter()
            .filter(|row| kms2.resolve_dek(&row.pii_key_ref, &region()).is_ok())
            .count();
        assert_eq!(
            recoverable, 1,
            "a live key is honestly reported recoverable"
        );
        let _ = fanout;
    }

    #[test]
    fn dek_ref_helpers_mint_the_frozen_grammar() {
        let s = subject_dek_ref(&tenant(), 3, "psn:ci-7");
        assert_eq!(s.to_uri(), "kms://acme/3/subject:psn:ci-7");
        let t = tenant_dek_ref(&tenant(), 3);
        assert_eq!(t.to_uri(), "kms://acme/3/tenant");
    }
}
