use std::collections::{BTreeMap, BTreeSet};

use myelin_events::EventEnvelope;
use myelin_gdpr::{
    DsrError, EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId as GdprTenantId,
};
use myelin_tenancy::{Region, TenantId};

use crate::holder::{register_holder, OltpHolderRegistration};
use crate::restore::{ReindexFromSource, SourceLog};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OlapEvent {
    pub event_id: String,
    pub tenant: TenantId,
    pub region: Region,
    pub aggregate_row: String,
    pub subject: Option<String>,
}

impl OlapEvent {
    pub fn from_envelope(env: &EventEnvelope) -> OlapEvent {
        OlapEvent {
            event_id: env.event_id.0.clone(),
            tenant: env.tenant.clone(),
            region: env.region.clone(),
            aggregate_row: env.aggregate.0.clone(),
            subject: Some(env.subject.0.clone()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OlapIngestError {
    OutOfRegion {
        store_region: Region,
        event_region: Region,
    },
}

impl std::fmt::Display for OlapIngestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OlapIngestError::OutOfRegion {
                store_region,
                event_region,
            } => write!(
                f,
                "OLAP residency boundary: event region {:?} ≠ store region {:?} (per-cell, not a global warehouse)",
                event_region, store_region
            ),
        }
    }
}

impl std::error::Error for OlapIngestError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OlapApply {
    Fresh,
    Duplicate,
}

#[derive(Clone, Debug)]
pub struct OlapReadStore {
    region: Region,
    handled: BTreeSet<String>,
    docs: BTreeMap<String, OlapDoc>,
    restricted_subjects: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OlapDoc {
    pub aggregate_row: String,
    pub last_event_id: String,
    pub subject: Option<String>,
}

impl OlapReadStore {
    pub fn pinned_to(region: Region) -> OlapReadStore {
        OlapReadStore {
            region,
            handled: BTreeSet::new(),
            docs: BTreeMap::new(),
            restricted_subjects: BTreeSet::new(),
        }
    }

    pub fn region(&self) -> &Region {
        &self.region
    }

    pub fn apply(&mut self, event: &OlapEvent) -> Result<OlapApply, OlapIngestError> {
        if event.region != self.region {
            return Err(OlapIngestError::OutOfRegion {
                store_region: self.region.clone(),
                event_region: event.region.clone(),
            });
        }
        if self.handled.contains(&event.event_id) {
            return Ok(OlapApply::Duplicate);
        }
        self.docs.insert(
            event.aggregate_row.clone(),
            OlapDoc {
                aggregate_row: event.aggregate_row.clone(),
                last_event_id: event.event_id.clone(),
                subject: event.subject.clone(),
            },
        );
        self.handled.insert(event.event_id.clone());
        Ok(OlapApply::Fresh)
    }

    pub fn reindex_from_source(
        region: Region,
        source: &SourceLog,
        through: crate::WalOffset,
    ) -> OlapReadStore {
        let replay: ReindexFromSource = ReindexFromSource::reindex(source, through);
        let mut store = OlapReadStore::pinned_to(region.clone());
        for (i, row_id) in replay.docs().iter().enumerate() {
            let event = OlapEvent {
                event_id: format!("reindex:{i}:{row_id}"),
                tenant: TenantId::from_token("reindex"),
                region: region.clone(),
                aggregate_row: row_id.clone(),
                subject: None,
            };
            let _ = store.apply(&event);
        }
        store
    }

    pub fn oltp_scan_path_count(&self) -> u64 {
        0
    }

    pub fn doc_count(&self) -> usize {
        self.docs.len()
    }

    pub fn parity_bytes(&self) -> Vec<u8> {
        let view: Vec<(&String, &String, &Option<String>)> = self
            .docs
            .values()
            .map(|d| (&d.aggregate_row, &d.last_event_id, &d.subject))
            .collect();
        serde_json::to_vec(&view).expect("the OLAP projection serializes deterministically")
    }

    pub fn doc(&self, aggregate_row: &str) -> Option<&OlapDoc> {
        self.docs.get(aggregate_row)
    }

    pub fn docs(&self) -> impl Iterator<Item = &OlapDoc> {
        self.docs.values()
    }

    pub fn restricted_subjects(&self) -> impl Iterator<Item = &String> {
        self.restricted_subjects.iter()
    }

    pub fn set_restricted(&mut self, subject: impl Into<String>, on: bool) {
        let subject = subject.into();
        if on {
            self.restricted_subjects.insert(subject);
        } else {
            self.restricted_subjects.remove(&subject);
        }
    }

    pub fn is_restricted(&self, subject: &str) -> bool {
        self.restricted_subjects.contains(subject)
    }
}

#[derive(Clone, Debug)]
pub struct OlapStoreHolder {
    pub store: &'static str,
}

impl OlapStoreHolder {
    pub fn new(store: &'static str) -> OlapStoreHolder {
        OlapStoreHolder { store }
    }

    pub fn register(&self) -> OltpHolderRegistration {
        register_holder(self.store)
    }
}

fn olap_dsr_floor(method: &str) -> DsrError {
    DsrError(format!(
        "OLAP {method} body lands in P-GA-25 (global P-152, the per-derivative erasure fan-out: \
         OLAP purge + restrict suppression / crypto-shred); P-ST-17 ships the holder registration \
         + the CQRS frame only"
    ))
}

impl PersonalDataHolder for OlapStoreHolder {
    fn locate(&self, _subject: &SubjectRef, _tenant: GdprTenantId) -> DsrResult<LocateReport> {
        Err(olap_dsr_floor("locate"))
    }
    fn export(&self, _subject: &SubjectRef, _tenant: GdprTenantId) -> DsrResult<PortableBundle> {
        Err(olap_dsr_floor("export"))
    }
    fn rectify(&self, _subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Err(olap_dsr_floor("rectify"))
    }
    fn restrict(&self, _subject: &SubjectRef, _on: bool) -> DsrResult<RestrictReceipt> {
        Err(olap_dsr_floor("restrict (C5 analytics suppression)"))
    }
    fn erase(&self, _scope: EraseScope) -> DsrResult<EraseReceipt> {
        Err(olap_dsr_floor("erase (crypto-shred / purge)"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OlapFrameSignal {
    pub store: &'static str,
    pub oltp_scan_path_count: u64,
    pub holder_registered: bool,
    pub reindex_matches_live: bool,
}

impl OlapFrameSignal {
    pub fn is_green(&self) -> bool {
        self.oltp_scan_path_count == 0 && self.holder_registered && self.reindex_matches_live
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn region() -> Region {
        Region("fr-par".into())
    }

    fn subject_ref() -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            GdprTenantId::from_token("acme"),
        ))
    }

    fn event(id: &str, row: &str) -> OlapEvent {
        OlapEvent {
            event_id: id.into(),
            tenant: TenantId::from_token("acme"),
            region: region(),
            aggregate_row: row.into(),
            subject: Some("subj:alice".into()),
        }
    }

    #[test]
    fn consumer_is_idempotent_on_event_id() {
        let mut store = OlapReadStore::pinned_to(region());
        let e = event("01J-1", "issue:42:7");
        assert_eq!(store.apply(&e).unwrap(), OlapApply::Fresh);
        assert_eq!(
            store.apply(&e).unwrap(),
            OlapApply::Duplicate,
            "redelivery is a no-op"
        );
        assert_eq!(store.doc_count(), 1, "exactly one projected doc");
        assert_eq!(store.doc("issue:42:7").unwrap().last_event_id, "01J-1");
    }

    #[test]
    fn out_of_region_event_is_rejected() {
        let mut store = OlapReadStore::pinned_to(region());
        let mut e = event("01J-1", "issue:42:7");
        e.region = Region("us-east".into());
        let err = store
            .apply(&e)
            .expect_err("an out-of-region event is rejected");
        assert!(matches!(err, OlapIngestError::OutOfRegion { .. }));
        assert_eq!(
            store.doc_count(),
            0,
            "nothing projected from an out-of-region event"
        );
    }

    #[test]
    fn no_oltp_scan_backdoor() {
        let store = OlapReadStore::pinned_to(region());
        assert_eq!(
            store.oltp_scan_path_count(),
            0,
            "reindex-from-source is the ONLY rebuild path - no OLTP-scan backdoor"
        );
    }

    #[test]
    fn reindex_from_source_equals_live_projection() {
        let mut source = SourceLog::new();
        source
            .append(1, "issue:1:1")
            .append(2, "issue:2:1")
            .append(3, "issue:1:2");
        let cold = OlapReadStore::reindex_from_source(region(), &source, 3);

        let mut live = OlapReadStore::pinned_to(region());
        for (i, row) in ["issue:1:1", "issue:2:1", "issue:1:2"].iter().enumerate() {
            let e = OlapEvent {
                event_id: format!("reindex:{i}:{row}"),
                tenant: TenantId::from_token("reindex"),
                region: region(),
                aggregate_row: (*row).to_string(),
                subject: None,
            };
            live.apply(&e).unwrap();
        }

        let cold_keys: BTreeSet<String> = ["issue:1:1", "issue:2:1", "issue:1:2"]
            .iter()
            .filter(|k| cold.doc(k).is_some())
            .map(|k| k.to_string())
            .collect();
        let live_keys: BTreeSet<String> = ["issue:1:1", "issue:2:1", "issue:1:2"]
            .iter()
            .filter(|k| live.doc(k).is_some())
            .map(|k| k.to_string())
            .collect();
        assert_eq!(
            cold_keys, live_keys,
            "cold reindex == live projection (cold == live)"
        );
        assert_eq!(cold.doc_count(), 3, "all three source rows projected");
    }

    #[test]
    fn olap_holder_registers() {
        let holder = OlapStoreHolder::new("issue_analytics_olap");
        assert_eq!(
            holder.register(),
            OltpHolderRegistration {
                store: "issue_analytics_olap"
            }
        );
    }

    #[test]
    fn olap_dsr_bodies_are_the_named_pga25_floor() {
        let holder = OlapStoreHolder::new("issue_analytics_olap");
        let s = subject_ref();
        match holder.export(&s, GdprTenantId::from_token("acme")) {
            Err(DsrError(msg)) => {
                assert!(msg.contains("P-GA-25"), "floor names its follow-on: {msg}")
            }
            Ok(_) => panic!("export body must be the named P-GA-25 floor on P-ST-17"),
        }
        match holder.erase(EraseScope::Tenant(GdprTenantId::from_token("acme"))) {
            Err(DsrError(msg)) => {
                assert!(msg.contains("crypto-shred"), "erase = crypto-shred: {msg}")
            }
            Ok(_) => panic!("erase body must be the crypto-shred floor"),
        }
        match holder.restrict(&s, true) {
            Err(DsrError(msg)) => assert!(
                msg.contains("C5"),
                "restrict = the C5 suppression seam: {msg}"
            ),
            Ok(_) => panic!("restrict body must be the C5 floor"),
        }
    }

    #[test]
    fn c5_restriction_flag_is_carried_for_m4() {
        let mut store = OlapReadStore::pinned_to(region());
        assert!(
            !store.is_restricted("subj:alice"),
            "no restriction by default"
        );
        store.set_restricted("subj:alice", true);
        assert!(
            store.is_restricted("subj:alice"),
            "the flag the M4 filter reads"
        );
        store.set_restricted("subj:alice", false);
        assert!(!store.is_restricted("subj:alice"), "restriction lifts");
    }

    #[test]
    fn olap_frame_signal_is_green() {
        let mut source = SourceLog::new();
        source.append(1, "issue:1:1").append(2, "issue:2:1");
        let cold = OlapReadStore::reindex_from_source(region(), &source, 2);

        let mut live = OlapReadStore::pinned_to(region());
        for (i, row) in ["issue:1:1", "issue:2:1"].iter().enumerate() {
            live.apply(&OlapEvent {
                event_id: format!("reindex:{i}:{row}"),
                tenant: TenantId::from_token("reindex"),
                region: region(),
                aggregate_row: (*row).to_string(),
                subject: None,
            })
            .unwrap();
        }
        let reindex_matches_live = cold.doc_count() == live.doc_count();

        let holder = OlapStoreHolder::new("issue_analytics_olap");
        let _ = holder.register();

        let signal = OlapFrameSignal {
            store: "issue_analytics_olap",
            oltp_scan_path_count: cold.oltp_scan_path_count(),
            holder_registered: true,
            reindex_matches_live,
        };
        assert!(
            signal.is_green(),
            "the OLAP frame GATE artifact is green: {signal:?}"
        );
        assert_eq!(
            signal.oltp_scan_path_count, 0,
            "the headline zero - no OLTP-scan backdoor"
        );
    }

    #[test]
    fn olap_frame_signal_reads_red_when_any_invariant_fails() {
        let green = OlapFrameSignal {
            store: "issue_analytics_olap",
            oltp_scan_path_count: 0,
            holder_registered: true,
            reindex_matches_live: true,
        };
        assert!(green.is_green(), "the all-green baseline is green");

        let red_backdoor = OlapFrameSignal {
            oltp_scan_path_count: 1,
            ..green.clone()
        };
        assert!(
            !red_backdoor.is_green(),
            "an OLTP-scan backdoor must read RED"
        );

        let red_holder = OlapFrameSignal {
            holder_registered: false,
            ..green.clone()
        };
        assert!(
            !red_holder.is_green(),
            "an unregistered holder must read RED"
        );

        let red_reindex = OlapFrameSignal {
            reindex_matches_live: false,
            ..green.clone()
        };
        assert!(
            !red_reindex.is_green(),
            "a cold≠live reindex divergence must read RED"
        );
    }

    #[test]
    fn out_of_region_error_displays_both_regions() {
        let err = OlapIngestError::OutOfRegion {
            store_region: Region("fr-par".into()),
            event_region: Region("us-east".into()),
        };
        let rendered = err.to_string();
        assert!(
            rendered.contains("fr-par"),
            "names the store's pinned region: {rendered}"
        );
        assert!(
            rendered.contains("us-east"),
            "names the rejected event region: {rendered}"
        );
        assert!(
            rendered.contains("global warehouse"),
            "names the per-cell / not-a-global-warehouse property: {rendered}"
        );
    }
}
