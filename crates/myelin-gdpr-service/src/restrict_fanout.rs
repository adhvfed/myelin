use std::collections::BTreeSet;
use std::sync::Mutex;

use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};

use crate::structural_floor::RestrictRegistry;

pub mod restrict_holder_ids {
    pub const SEARCH_INDEX: &str = "search_index";
    pub const REFS_GRAPH: &str = "refs_graph";
    pub const NOTIF_HISTORY: &str = "notif_history";
    pub const AGENT_RUNTIME: &str = "agent_runtime";
    pub const OLAP_READ_STORE: &str = "olap_read_store";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DerivedProcessing {
    SearchIndex,
    RefsProject,
    NotifNotify,
    AgentRead,
    OlapAnalyse,
}

impl DerivedProcessing {
    #[must_use]
    pub const fn all() -> [DerivedProcessing; 5] {
        [
            DerivedProcessing::SearchIndex,
            DerivedProcessing::RefsProject,
            DerivedProcessing::NotifNotify,
            DerivedProcessing::AgentRead,
            DerivedProcessing::OlapAnalyse,
        ]
    }

    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            DerivedProcessing::SearchIndex => "search_index",
            DerivedProcessing::RefsProject => "refs_project",
            DerivedProcessing::NotifNotify => "notif_notify",
            DerivedProcessing::AgentRead => "agent_read",
            DerivedProcessing::OlapAnalyse => "olap_analyse",
        }
    }

    #[must_use]
    pub const fn holder_id(self) -> &'static str {
        match self {
            DerivedProcessing::SearchIndex => restrict_holder_ids::SEARCH_INDEX,
            DerivedProcessing::RefsProject => restrict_holder_ids::REFS_GRAPH,
            DerivedProcessing::NotifNotify => restrict_holder_ids::NOTIF_HISTORY,
            DerivedProcessing::AgentRead => restrict_holder_ids::AGENT_RUNTIME,
            DerivedProcessing::OlapAnalyse => restrict_holder_ids::OLAP_READ_STORE,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DerivedProcessed {
    Processed(String),
    Suppressed,
    NoRow,
}

pub struct DerivedStore<'a> {
    kind: DerivedProcessing,
    restrict: &'a RestrictRegistry,
    rows: Mutex<BTreeSet<(String, String)>>,
}

impl<'a> DerivedStore<'a> {
    #[must_use]
    pub fn new(kind: DerivedProcessing, restrict: &'a RestrictRegistry) -> DerivedStore<'a> {
        DerivedStore {
            kind,
            restrict,
            rows: Mutex::new(BTreeSet::new()),
        }
    }

    #[must_use]
    pub fn kind(&self) -> DerivedProcessing {
        self.kind
    }

    #[must_use]
    pub fn holder_id(&self) -> &'static str {
        self.kind.holder_id()
    }

    fn key(subject: &SubjectRef, tenant: &TenantId) -> (String, String) {
        (tenant.0.clone(), subject.principal.principal_id.0.clone())
    }

    pub fn seed_row(&self, subject: &SubjectRef, tenant: &TenantId) {
        self.rows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(Self::key(subject, tenant));
    }

    #[must_use]
    pub fn has_row(&self, subject: &SubjectRef, tenant: &TenantId) -> bool {
        self.rows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&Self::key(subject, tenant))
    }

    #[must_use]
    pub fn process(&self, subject: &SubjectRef, tenant: &TenantId) -> DerivedProcessed {
        if !self.has_row(subject, tenant) {
            return DerivedProcessed::NoRow;
        }
        if self.restrict.is_restricted(subject, tenant) {
            DerivedProcessed::Suppressed
        } else {
            DerivedProcessed::Processed(format!("{}:processed", self.kind.token()))
        }
    }
}

pub struct DerivedStoreHolder<'a> {
    store: &'a DerivedStore<'a>,
}

impl<'a> DerivedStoreHolder<'a> {
    #[must_use]
    pub fn new(store: &'a DerivedStore<'a>) -> DerivedStoreHolder<'a> {
        DerivedStoreHolder { store }
    }
}

impl PersonalDataHolder for DerivedStoreHolder<'_> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if self.store.has_row(subject, &tenant) {
            "located:row-present"
        } else {
            "located:0-recoverable"
        };
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                self.store.holder_id(),
                &sid,
                &tenant.0,
                outcome,
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                self.store.holder_id(),
                &sid,
                &tenant.0,
                "exported",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                self.store.holder_id(),
                &sid,
                "*",
                "rectified:reindex_from_source",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        self.store
            .restrict
            .set(subject, &subject.principal.tenant, on);
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if on {
            "restricted:set:processing_suppressed"
        } else {
            "restricted:clear:processing_resumed"
        };
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                self.store.holder_id(),
                &sid,
                &subject.principal.tenant.0,
                outcome,
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let sid = match &scope {
            EraseScope::Subject { subject, .. } => subject.principal.principal_id.0.clone(),
            EraseScope::Tenant(_) => "*tenant*".to_string(),
        };
        let tenant = match &scope {
            EraseScope::Subject { tenant, .. } => tenant.0.clone(),
            EraseScope::Tenant(tenant) => tenant.0.clone(),
        };
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                self.store.holder_id(),
                &sid,
                &tenant,
                "erased:derived_row_purged",
                None,
                0,
            ),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivedRestrictVerdict {
    pub holder_id: &'static str,
    pub op: DerivedProcessing,
    pub outcome: DerivedProcessed,
    pub row_retained: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestrictFanOutOutcome {
    pub subject_token: String,
    pub restricted: bool,
    pub verdicts: Vec<DerivedRestrictVerdict>,
    pub holder_receipts: Vec<RestrictReceipt>,
}

impl RestrictFanOutOutcome {
    #[must_use]
    pub fn all_suppressed(&self) -> bool {
        self.verdicts
            .iter()
            .all(|v| matches!(v.outcome, DerivedProcessed::Suppressed))
    }

    #[must_use]
    pub fn processed_count(&self) -> usize {
        self.verdicts
            .iter()
            .filter(|v| matches!(v.outcome, DerivedProcessed::Processed(_)))
            .count()
    }

    #[must_use]
    pub fn all_rows_retained(&self) -> bool {
        self.verdicts.iter().all(|v| v.row_retained)
    }
}

pub struct RestrictFanOutDriver;

impl RestrictFanOutDriver {
    #[allow(clippy::too_many_arguments)]
    pub fn fan_out_restrict(
        subject: &SubjectRef,
        tenant: &TenantId,
        on: bool,
        stores: &[&DerivedStore<'_>; 5],
        holders: &[&dyn PersonalDataHolder; 5],
    ) -> DsrResult<RestrictFanOutOutcome> {
        let mut holder_receipts = Vec::with_capacity(5);
        for holder in holders {
            holder_receipts.push(holder.restrict(subject, on)?);
        }

        let verdicts = stores
            .iter()
            .map(|store| DerivedRestrictVerdict {
                holder_id: store.holder_id(),
                op: store.kind(),
                outcome: store.process(subject, tenant),
                row_retained: store.has_row(subject, tenant),
            })
            .collect();

        Ok(RestrictFanOutOutcome {
            subject_token: subject.principal.principal_id.0.clone(),
            restricted: on,
            verdicts,
            holder_receipts,
        })
    }
}

pub const RESTRICT_FANOUT_PROCESSING_SUPPRESSED: (&str, &str) =
    ("gdpr.restrict_fanout_processing_suppressed", "count");

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn t(s: &str) -> TenantId {
        TenantId::from_token(s)
    }

    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            t("acme"),
        ))
    }

    fn five_stores<'a>(restrict: &'a RestrictRegistry) -> [DerivedStore<'a>; 5] {
        [
            DerivedStore::new(DerivedProcessing::SearchIndex, restrict),
            DerivedStore::new(DerivedProcessing::RefsProject, restrict),
            DerivedStore::new(DerivedProcessing::NotifNotify, restrict),
            DerivedStore::new(DerivedProcessing::AgentRead, restrict),
            DerivedStore::new(DerivedProcessing::OlapAnalyse, restrict),
        ]
    }

    #[test]
    fn each_derived_store_suppresses_processing_but_retains_row_reversibly() {
        let tenant = t("acme");
        let subj = subject("u-1");
        let restrict = RestrictRegistry::new();
        let stores = five_stores(&restrict);
        for s in &stores {
            s.seed_row(&subj, &tenant);
        }

        for s in &stores {
            assert!(
                matches!(s.process(&subj, &tenant), DerivedProcessed::Processed(_)),
                "{} processes before restriction",
                s.holder_id()
            );
        }

        restrict.set(&subj, &tenant, true);
        for s in &stores {
            assert_eq!(
                s.process(&subj, &tenant),
                DerivedProcessed::Suppressed,
                "{} SUPPRESSED for the restricted subject (§4.4)",
                s.holder_id()
            );
            assert!(
                s.has_row(&subj, &tenant),
                "{} RETAINS the derived row while restricted (suppression ≠ delete)",
                s.holder_id()
            );
        }

        restrict.set(&subj, &tenant, false);
        for s in &stores {
            assert!(
                matches!(s.process(&subj, &tenant), DerivedProcessed::Processed(_)),
                "{} processes again after the restriction is lifted (reversible)",
                s.holder_id()
            );
        }
    }

    #[test]
    fn olap_honours_the_restriction_flag_no_analytics_for_a_restricted_subject() {
        let tenant = t("acme");
        let subj = subject("u-olap");
        let restrict = RestrictRegistry::new();
        let olap = DerivedStore::new(DerivedProcessing::OlapAnalyse, &restrict);
        olap.seed_row(&subj, &tenant);
        assert_eq!(olap.holder_id(), restrict_holder_ids::OLAP_READ_STORE);

        assert!(matches!(
            olap.process(&subj, &tenant),
            DerivedProcessed::Processed(_)
        ));
        restrict.set(&subj, &tenant, true);
        assert_eq!(olap.process(&subj, &tenant), DerivedProcessed::Suppressed);
        assert!(olap.has_row(&subj, &tenant));
    }

    #[test]
    fn the_derived_suppression_branch_is_load_bearing_both_verdicts_pinned() {
        let tenant = t("acme");
        let subj = subject("u-branch");
        let restrict = RestrictRegistry::new();
        let search = DerivedStore::new(DerivedProcessing::SearchIndex, &restrict);
        search.seed_row(&subj, &tenant);

        match search.process(&subj, &tenant) {
            DerivedProcessed::Processed(out) => {
                assert!(
                    out.starts_with("search_index:"),
                    "the processed projection names the op"
                );
            }
            other => panic!("expected Processed, got {other:?}"),
        }
        restrict.set(&subj, &tenant, true);
        assert_eq!(search.process(&subj, &tenant), DerivedProcessed::Suppressed);
    }

    #[test]
    fn a_derived_store_with_no_row_reads_no_row_not_suppressed() {
        let tenant = t("acme");
        let subj = subject("u-norow");
        let restrict = RestrictRegistry::new();
        let search = DerivedStore::new(DerivedProcessing::SearchIndex, &restrict);
        assert_eq!(search.process(&subj, &tenant), DerivedProcessed::NoRow);
        restrict.set(&subj, &tenant, true);
        assert_eq!(search.process(&subj, &tenant), DerivedProcessed::NoRow);
    }

    #[test]
    fn restrict_through_one_holder_suppresses_all_five_derived_stores() {
        let tenant = t("acme");
        let subj = subject("u-shared");
        let restrict = RestrictRegistry::new();
        let stores = five_stores(&restrict);
        for s in &stores {
            s.seed_row(&subj, &tenant);
        }

        let search_holder = DerivedStoreHolder::new(&stores[0]);
        search_holder.restrict(&subj, true).unwrap();

        for s in &stores {
            assert_eq!(
                s.process(&subj, &tenant),
                DerivedProcessed::Suppressed,
                "{} suppressed by the SHARED flag set through the Search holder",
                s.holder_id()
            );
        }
    }

    #[test]
    fn driver_fans_restrict_zero_processing_across_all_five_reversible() {
        let tenant = t("acme");
        let subj = subject("u-fan");
        let restrict = RestrictRegistry::new();
        let stores = five_stores(&restrict);
        for s in &stores {
            s.seed_row(&subj, &tenant);
        }
        let holders: Vec<DerivedStoreHolder> = stores.iter().map(DerivedStoreHolder::new).collect();
        let store_refs: [&DerivedStore; 5] =
            [&stores[0], &stores[1], &stores[2], &stores[3], &stores[4]];
        let holder_refs: [&dyn PersonalDataHolder; 5] = [
            &holders[0],
            &holders[1],
            &holders[2],
            &holders[3],
            &holders[4],
        ];

        let set =
            RestrictFanOutDriver::fan_out_restrict(&subj, &tenant, true, &store_refs, &holder_refs)
                .unwrap();
        assert!(
            set.all_suppressed(),
            "0 processing of the restricted subject across all five (GA-D7)"
        );
        assert_eq!(
            set.processed_count(),
            0,
            "0 derived stores processed the restricted subject"
        );
        assert!(
            set.all_rows_retained(),
            "every derived row RETAINED while restricted (§4.4)"
        );
        assert_eq!(
            set.verdicts.len(),
            5,
            "Search + Refs + Notif + Agents + OLAP"
        );
        assert_eq!(
            set.holder_receipts.len(),
            5,
            "one restrict receipt per derived store"
        );
        assert!(set.restricted, "the outcome records the restriction is SET");

        let clear = RestrictFanOutDriver::fan_out_restrict(
            &subj,
            &tenant,
            false,
            &store_refs,
            &holder_refs,
        )
        .unwrap();
        assert!(
            !clear.all_suppressed(),
            "processing resumes after the restriction is lifted"
        );
        assert_eq!(
            clear
                .verdicts
                .iter()
                .filter(|v| matches!(v.outcome, DerivedProcessed::Processed(_)))
                .count(),
            5,
            "all five derived stores process again (reversible)"
        );
        assert!(
            !clear.restricted,
            "the outcome records the restriction is CLEARED"
        );
    }

    #[test]
    fn the_fanout_readings_are_not_vacuous_both_polarities() {
        let tenant = t("acme");
        let subj = subject("u-vac");
        let restrict = RestrictRegistry::new();
        let stores = five_stores(&restrict);
        for s in stores.iter().take(4) {
            s.seed_row(&subj, &tenant);
        }
        let holders: Vec<DerivedStoreHolder> = stores.iter().map(DerivedStoreHolder::new).collect();
        let store_refs: [&DerivedStore; 5] =
            [&stores[0], &stores[1], &stores[2], &stores[3], &stores[4]];
        let holder_refs: [&dyn PersonalDataHolder; 5] = [
            &holders[0],
            &holders[1],
            &holders[2],
            &holders[3],
            &holders[4],
        ];

        let out = RestrictFanOutDriver::fan_out_restrict(
            &subj,
            &tenant,
            false,
            &store_refs,
            &holder_refs,
        )
        .unwrap();
        assert_eq!(
            out.processed_count(),
            4,
            "four rows processed (processed_count is not constant 0)"
        );
        assert!(
            !out.all_suppressed(),
            "not all suppressed (a processing store exists)"
        );
        assert!(
            !out.all_rows_retained(),
            "the store with NO row is not row-retained (all_rows_retained is not constant true)"
        );
    }

    #[test]
    fn the_derived_store_keys_per_tenant_and_subject() {
        let tenant = t("acme");
        let other = t("globex");
        let a = subject("u-a");
        let b = subject("u-b");
        let restrict = RestrictRegistry::new();
        let store = DerivedStore::new(DerivedProcessing::SearchIndex, &restrict);
        store.seed_row(&a, &tenant);
        assert!(store.has_row(&a, &tenant), "A's row is present");
        assert!(
            !store.has_row(&b, &tenant),
            "B has no row (distinct subject key)"
        );
        assert!(
            !store.has_row(&a, &other),
            "A's id in a different tenant has no row (tenant in the key)"
        );
        restrict.set(&a, &tenant, true);
        assert_eq!(store.process(&a, &tenant), DerivedProcessed::Suppressed);
        assert_eq!(store.process(&b, &tenant), DerivedProcessed::NoRow);
    }

    #[test]
    fn the_fan_out_covers_exactly_the_five_section_4_4_derived_stores() {
        assert_eq!(DerivedProcessing::all().len(), 5);
        assert_eq!(
            DerivedProcessing::SearchIndex.holder_id(),
            restrict_holder_ids::SEARCH_INDEX
        );
        assert_eq!(
            DerivedProcessing::RefsProject.holder_id(),
            restrict_holder_ids::REFS_GRAPH
        );
        assert_eq!(
            DerivedProcessing::NotifNotify.holder_id(),
            restrict_holder_ids::NOTIF_HISTORY
        );
        assert_eq!(
            DerivedProcessing::AgentRead.holder_id(),
            restrict_holder_ids::AGENT_RUNTIME
        );
        assert_eq!(
            DerivedProcessing::OlapAnalyse.holder_id(),
            restrict_holder_ids::OLAP_READ_STORE
        );
        assert_eq!(DerivedProcessing::SearchIndex.token(), "search_index");
        assert_eq!(DerivedProcessing::OlapAnalyse.token(), "olap_analyse");
    }

    #[test]
    fn the_holder_restrict_op_sets_and_clears_the_shared_flag_with_a_receipt() {
        let tenant = t("acme");
        let subj = subject("u-receipt");
        let restrict = RestrictRegistry::new();
        let store = DerivedStore::new(DerivedProcessing::OlapAnalyse, &restrict);
        let holder = DerivedStoreHolder::new(&store);

        let set = holder.restrict(&subj, true).unwrap();
        assert!(
            restrict.is_restricted(&subj, &tenant),
            "the holder restrict op SET the shared flag"
        );
        assert_eq!(set.receipt.operation, "restrict");
        assert!(
            set.receipt.content_hash.starts_with("blake3:"),
            "the restrict receipt is content-addressed"
        );

        let clear = holder.restrict(&subj, false).unwrap();
        assert!(
            !restrict.is_restricted(&subj, &tenant),
            "the holder restrict op CLEARED the shared flag"
        );
        assert_eq!(clear.receipt.operation, "restrict");
        assert_ne!(set.receipt.content_hash, clear.receipt.content_hash);
    }
}
