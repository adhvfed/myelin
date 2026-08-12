use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};

use crate::orchestration::{CanonicalErasePhase, RegisteredHolder};

pub mod derivative_holder_ids {
    pub const SEARCH_INDEX: &str = "search_index";
    pub const REFS_GRAPH: &str = "refs_graph";
    pub const NOTIF_HISTORY: &str = "notif_history";
}

pub const ERASED_USER: &str = "[erased user]";

pub fn derivative_phase_of(holder_id: &str) -> Option<CanonicalErasePhase> {
    match holder_id {
        derivative_holder_ids::SEARCH_INDEX => Some(CanonicalErasePhase::PurgeAndTombstoneDerived),
        derivative_holder_ids::REFS_GRAPH => Some(CanonicalErasePhase::PurgeAndTombstoneDerived),
        derivative_holder_ids::NOTIF_HISTORY => Some(CanonicalErasePhase::CachesAndDerivedCopies),
        _ => None,
    }
}

#[derive(Debug, Default)]
pub struct SearchIndexModel {
    docs: Mutex<BTreeMap<String, SearchDoc>>,
    erase_calls: Mutex<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SearchDoc {
    projection: String,
    embedding_present: bool,
}

impl SearchIndexModel {
    pub fn new() -> SearchIndexModel {
        SearchIndexModel::default()
    }

    pub fn index_from_source(&self, subject_token: &str, source_value: &str) {
        self.docs.lock().unwrap_or_else(|e| e.into_inner()).insert(
            subject_token.to_string(),
            SearchDoc {
                projection: source_value.to_string(),
                embedding_present: true,
            },
        );
    }

    pub fn hits(&self, subject_token: &str) -> usize {
        usize::from(
            self.docs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(subject_token),
        )
    }

    pub fn reidentify_hits(&self, subject_token: &str) -> usize {
        let docs = self.docs.lock().unwrap_or_else(|e| e.into_inner());
        usize::from(docs.get(subject_token).is_some_and(|d| d.embedding_present))
    }

    pub fn projection(&self, subject_token: &str) -> Option<String> {
        self.docs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(subject_token)
            .map(|d| d.projection.clone())
    }

    pub fn erase_call_count(&self) -> u32 {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn erase(&self, subject_token: &str) -> bool {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        self.docs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(subject_token)
            .is_some()
    }
}

pub struct SearchIndexHolder<'a> {
    model: &'a SearchIndexModel,
}

impl<'a> SearchIndexHolder<'a> {
    pub fn new(model: &'a SearchIndexModel) -> SearchIndexHolder<'a> {
        SearchIndexHolder { model }
    }
}

impl PersonalDataHolder for SearchIndexHolder<'_> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if self.model.hits(&sid) > 0 {
            "located:indexed"
        } else {
            "located:0-recoverable"
        };
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                derivative_holder_ids::SEARCH_INDEX,
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
                derivative_holder_ids::SEARCH_INDEX,
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
                derivative_holder_ids::SEARCH_INDEX,
                &sid,
                "*",
                "rectified:reindex_from_source",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if on {
            "restricted:set"
        } else {
            "restricted:clear"
        };
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                derivative_holder_ids::SEARCH_INDEX,
                &sid,
                "*",
                outcome,
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (sid, tenant) = subject_and_tenant(&scope);
        self.model.erase(&sid);
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                derivative_holder_ids::SEARCH_INDEX,
                &sid,
                &tenant,
                "purge_and_reindex:embeddings_purged_not_hidden",
                None,
                0,
            ),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefsResolve {
    Live(String),
    Tombstone,
    Missing,
}

#[derive(Debug, Default)]
pub struct RefsGraphModel {
    edges: Mutex<BTreeMap<String, String>>,
    tombstoned: Mutex<BTreeSet<String>>,
    erase_calls: Mutex<u32>,
}

impl RefsGraphModel {
    pub fn new() -> RefsGraphModel {
        RefsGraphModel::default()
    }

    pub fn add_edge_from_source(&self, subject_token: &str, target: &str) {
        self.edges
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(subject_token.to_string(), target.to_string());
        self.tombstoned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(subject_token);
    }

    pub fn resolve(&self, subject_token: &str) -> RefsResolve {
        if self
            .tombstoned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(subject_token)
        {
            return RefsResolve::Tombstone;
        }
        match self
            .edges
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(subject_token)
        {
            Some(target) => RefsResolve::Live(target.clone()),
            None => RefsResolve::Missing,
        }
    }

    pub fn recoverable_edges(&self, subject_token: &str) -> usize {
        usize::from(
            self.edges
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(subject_token),
        )
    }

    pub fn erase_call_count(&self) -> u32 {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn erase(&self, subject_token: &str) {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        self.edges
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(subject_token);
        self.tombstoned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(subject_token.to_string());
    }
}

pub struct RefsGraphHolder<'a> {
    model: &'a RefsGraphModel,
}

impl<'a> RefsGraphHolder<'a> {
    pub fn new(model: &'a RefsGraphModel) -> RefsGraphHolder<'a> {
        RefsGraphHolder { model }
    }
}

impl PersonalDataHolder for RefsGraphHolder<'_> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if self.model.recoverable_edges(&sid) > 0 {
            "located:edges-present"
        } else {
            "located:0-recoverable"
        };
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                derivative_holder_ids::REFS_GRAPH,
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
                derivative_holder_ids::REFS_GRAPH,
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
                derivative_holder_ids::REFS_GRAPH,
                &sid,
                "*",
                "rectified:reindex_from_source",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if on {
            "restricted:set"
        } else {
            "restricted:clear"
        };
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                derivative_holder_ids::REFS_GRAPH,
                &sid,
                "*",
                outcome,
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (sid, tenant) = subject_and_tenant(&scope);
        self.model.erase(&sid);
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                derivative_holder_ids::REFS_GRAPH,
                &sid,
                &tenant,
                "tombstone:0_recoverable:no_resolve_500",
                None,
                0,
            ),
        })
    }
}

#[derive(Debug, Default)]
pub struct NotifHistoryModel {
    items: Mutex<BTreeMap<String, String>>,
    erased: Mutex<BTreeSet<String>>,
    erase_calls: Mutex<u32>,
}

impl NotifHistoryModel {
    pub fn new() -> NotifHistoryModel {
        NotifHistoryModel::default()
    }

    pub fn add_item_from_source(&self, item_id: &str, mentioned_subject: &str) {
        self.items
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(item_id.to_string(), mentioned_subject.to_string());
    }

    pub fn render_mention(&self, item_id: &str) -> Option<String> {
        let items = self.items.lock().unwrap_or_else(|e| e.into_inner());
        let mentioned = items.get(item_id)?;
        if self
            .erased
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(mentioned)
        {
            Some(ERASED_USER.to_string())
        } else {
            Some(mentioned.clone())
        }
    }

    pub fn erase_call_count(&self) -> u32 {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn erase(&self, subject_token: &str) {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        self.erased
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(subject_token.to_string());
    }
}

pub struct NotifHistoryHolder<'a> {
    model: &'a NotifHistoryModel,
}

impl<'a> NotifHistoryHolder<'a> {
    pub fn new(model: &'a NotifHistoryModel) -> NotifHistoryHolder<'a> {
        NotifHistoryHolder { model }
    }
}

impl PersonalDataHolder for NotifHistoryHolder<'_> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                derivative_holder_ids::NOTIF_HISTORY,
                &sid,
                &tenant.0,
                "located:inbox-read-models",
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
                derivative_holder_ids::NOTIF_HISTORY,
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
                derivative_holder_ids::NOTIF_HISTORY,
                &sid,
                "*",
                "rectified:reindex_from_source",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if on {
            "restricted:set"
        } else {
            "restricted:clear"
        };
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                derivative_holder_ids::NOTIF_HISTORY,
                &sid,
                "*",
                outcome,
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (sid, tenant) = subject_and_tenant(&scope);
        self.model.erase(&sid);
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                derivative_holder_ids::NOTIF_HISTORY,
                &sid,
                &tenant,
                "purge_read_models:humanise_to_erased_user",
                None,
                0,
            ),
        })
    }
}

fn subject_and_tenant(scope: &EraseScope) -> (String, String) {
    match scope {
        EraseScope::Subject { subject, tenant } => {
            (subject.principal.principal_id.0.clone(), tenant.0.clone())
        }
        EraseScope::Tenant(tenant) => ("*tenant*".to_string(), tenant.0.clone()),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivativeEraseReceipt {
    pub subject_token: String,
    pub embeddings_purged: bool,
    pub refs_tombstoned: bool,
    pub notif_humanised: bool,
    pub holder_receipts: Vec<EraseReceipt>,
}

pub struct DerivativeErasureDriver;

impl DerivativeErasureDriver {
    pub fn register_derivatives<'a>(
        holders: Vec<(&'static str, &'a dyn PersonalDataHolder)>,
    ) -> Vec<RegisteredHolder<'a>> {
        holders
            .into_iter()
            .map(|(id, holder)| {
                let phase = derivative_phase_of(id).unwrap_or_else(|| {
                    panic!("derivative holder `{id}` has no canonical erase phase")
                });
                RegisteredHolder { id, phase, holder }
            })
            .collect()
    }

    pub fn fan_out_erase(
        scope: &EraseScope,
        search: &SearchIndexModel,
        search_holder: &dyn PersonalDataHolder,
        refs: &RefsGraphModel,
        refs_holder: &dyn PersonalDataHolder,
        notif: &NotifHistoryModel,
        notif_holder: &dyn PersonalDataHolder,
    ) -> DsrResult<DerivativeEraseReceipt> {
        let (sid, _tenant) = subject_and_tenant(scope);
        let search_receipt = search_holder.erase(scope.clone())?;
        let refs_receipt = refs_holder.erase(scope.clone())?;
        let notif_receipt = notif_holder.erase(scope.clone())?;

        let embeddings_purged = search.reidentify_hits(&sid) == 0;
        let refs_tombstoned = matches!(refs.resolve(&sid), RefsResolve::Tombstone);
        let notif_humanised = notif.erase_call_count() > 0;
        Ok(DerivativeEraseReceipt {
            subject_token: sid,
            embeddings_purged,
            refs_tombstoned,
            notif_humanised,
            holder_receipts: vec![search_receipt, refs_receipt, notif_receipt],
        })
    }

    pub fn rectify_via_reindex_from_source(
        subject_token: &str,
        corrected_source_value: &str,
        corrected_edge_target: &str,
        search: &SearchIndexModel,
        refs: &RefsGraphModel,
    ) -> RectifyOutcome {
        search.index_from_source(subject_token, corrected_source_value);
        refs.add_edge_from_source(subject_token, corrected_edge_target);
        RectifyOutcome {
            subject_token: subject_token.to_string(),
            search_projection: search.projection(subject_token),
            refs_target: match refs.resolve(subject_token) {
                RefsResolve::Live(t) => Some(t),
                _ => None,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RectifyOutcome {
    pub subject_token: String,
    pub search_projection: Option<String>,
    pub refs_target: Option<String>,
}

pub const DERIVATIVE_ERASE_FANOUT_COVERAGE: (&str, &str) =
    ("gdpr.derivative_erase_fanout_coverage", "ratio");

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

    fn subject_scope(s: &str) -> EraseScope {
        EraseScope::Subject {
            subject: subject(s),
            tenant: t("acme"),
        }
    }

    #[test]
    fn search_erase_purges_embeddings_not_hidden_zero_reidentification() {
        let model = SearchIndexModel::new();
        model.index_from_source("u-1", "alice@example.com");
        assert_eq!(model.hits("u-1"), 1, "indexed before erase");
        assert_eq!(
            model.reidentify_hits("u-1"),
            1,
            "the embedding re-identifies before erase"
        );

        let holder = SearchIndexHolder::new(&model);
        let receipt = holder.erase(subject_scope("u-1")).unwrap();

        assert_eq!(model.hits("u-1"), 0, "doc purged (0 hits)");
        assert_eq!(
            model.reidentify_hits("u-1"),
            0,
            "embedding purged - 0 re-identification (GA-D2)"
        );
        assert_eq!(
            receipt.receipt.operation, "erase",
            "the erase receipt names the op"
        );
        assert!(
            receipt.receipt.content_hash.starts_with("blake3:"),
            "the embedding-purge receipt is content-addressed (the green artifact)"
        );
    }

    #[test]
    fn search_has_no_hide_path_only_a_real_purge() {
        let model = SearchIndexModel::new();
        model.index_from_source("u-hide", "secret");
        SearchIndexHolder::new(&model)
            .erase(subject_scope("u-hide"))
            .unwrap();
        assert_eq!(model.reidentify_hits("u-hide"), 0);
        assert_eq!(model.hits("u-hide"), 0);
    }

    #[test]
    fn refs_erase_tombstones_zero_recoverable_no_resolve_500() {
        let model = RefsGraphModel::new();
        model.add_edge_from_source("u-2", "issue:42");
        assert_eq!(model.resolve("u-2"), RefsResolve::Live("issue:42".into()));
        assert_eq!(model.recoverable_edges("u-2"), 1);

        let holder = RefsGraphHolder::new(&model);
        holder.erase(subject_scope("u-2")).unwrap();

        assert_eq!(
            model.resolve("u-2"),
            RefsResolve::Tombstone,
            "resolve returns the tombstone, not a 500"
        );
        assert_eq!(
            model.recoverable_edges("u-2"),
            0,
            "0 recoverable edges after tombstone"
        );
    }

    #[test]
    fn refs_resolve_is_infallible_even_for_unknown_and_tombstoned() {
        let model = RefsGraphModel::new();
        assert_eq!(model.resolve("nobody"), RefsResolve::Missing);
        RefsGraphHolder::new(&model)
            .erase(subject_scope("u-gone"))
            .unwrap();
        assert_eq!(model.resolve("u-gone"), RefsResolve::Tombstone);
    }

    #[test]
    fn notif_erase_humanises_mentions_to_erased_user() {
        let model = NotifHistoryModel::new();
        model.add_item_from_source("inbox-1", "u-3");
        model.add_item_from_source("inbox-2", "u-other");
        assert_eq!(
            model.render_mention("inbox-1").as_deref(),
            Some("u-3"),
            "renders the token before erase"
        );

        NotifHistoryHolder::new(&model)
            .erase(subject_scope("u-3"))
            .unwrap();

        assert_eq!(
            model.render_mention("inbox-1").as_deref(),
            Some(ERASED_USER),
            "humanised to [erased user]"
        );
        assert_eq!(
            model.render_mention("inbox-1").as_deref(),
            Some("[erased user]")
        );
        assert_eq!(
            model.render_mention("inbox-2").as_deref(),
            Some("u-other"),
            "other mentions untouched"
        );
    }

    #[test]
    fn rectification_fans_out_via_reindex_from_source_drift_is_zero() {
        let search = SearchIndexModel::new();
        let refs = RefsGraphModel::new();
        search.index_from_source("u-4", "old name");
        refs.add_edge_from_source("u-4", "old-target");

        let outcome = DerivativeErasureDriver::rectify_via_reindex_from_source(
            "u-4",
            "new name",
            "new-target",
            &search,
            &refs,
        );

        assert_eq!(
            outcome.search_projection.as_deref(),
            Some("new name"),
            "Search reindexed from source"
        );
        assert_eq!(
            outcome.refs_target.as_deref(),
            Some("new-target"),
            "Refs rebuilt from source"
        );
        assert_eq!(search.projection("u-4").as_deref(), Some("new name"));
        assert_eq!(refs.resolve("u-4"), RefsResolve::Live("new-target".into()));
    }

    #[test]
    fn driver_fans_per_derivative_erase_and_builds_the_embedding_purge_receipt() {
        let search = SearchIndexModel::new();
        let refs = RefsGraphModel::new();
        let notif = NotifHistoryModel::new();
        search.index_from_source("u-5", "bob");
        refs.add_edge_from_source("u-5", "pr:7");
        notif.add_item_from_source("inbox-x", "u-5");

        let sh = SearchIndexHolder::new(&search);
        let rh = RefsGraphHolder::new(&refs);
        let nh = NotifHistoryHolder::new(&notif);

        let receipt = DerivativeErasureDriver::fan_out_erase(
            &subject_scope("u-5"),
            &search,
            &sh,
            &refs,
            &rh,
            &notif,
            &nh,
        )
        .unwrap();

        assert!(
            receipt.embeddings_purged,
            "Search embeddings purged, not hidden (GA-D2)"
        );
        assert!(
            receipt.refs_tombstoned,
            "Refs tombstoned, 0 recoverable, no resolve-500 (REF-D5)"
        );
        assert!(
            receipt.notif_humanised,
            "Notif humanised mentions (NOTIF-D6)"
        );
        assert_eq!(
            receipt.holder_receipts.len(),
            3,
            "Search + Refs + Notif receipts"
        );
        assert_eq!(
            notif.render_mention("inbox-x").as_deref(),
            Some(ERASED_USER)
        );
        assert_eq!(search.reidentify_hits("u-5"), 0);
        assert_eq!(refs.recoverable_edges("u-5"), 0);
    }

    #[test]
    fn derivative_phases_are_pinned() {
        assert_eq!(
            derivative_phase_of(derivative_holder_ids::SEARCH_INDEX),
            Some(CanonicalErasePhase::PurgeAndTombstoneDerived)
        );
        assert_eq!(
            derivative_phase_of(derivative_holder_ids::REFS_GRAPH),
            Some(CanonicalErasePhase::PurgeAndTombstoneDerived)
        );
        assert_eq!(
            derivative_phase_of(derivative_holder_ids::NOTIF_HISTORY),
            Some(CanonicalErasePhase::CachesAndDerivedCopies)
        );
        assert_eq!(derivative_phase_of("not_a_derivative"), None);
        assert!(
            CanonicalErasePhase::PurgeAndTombstoneDerived
                < CanonicalErasePhase::CachesAndDerivedCopies
        );
    }

    #[test]
    fn register_derivatives_assigns_canonical_phases() {
        let search = SearchIndexModel::new();
        let refs = RefsGraphModel::new();
        let notif = NotifHistoryModel::new();
        let sh = SearchIndexHolder::new(&search);
        let rh = RefsGraphHolder::new(&refs);
        let nh = NotifHistoryHolder::new(&notif);
        let registered = DerivativeErasureDriver::register_derivatives(vec![
            (
                derivative_holder_ids::SEARCH_INDEX,
                &sh as &dyn PersonalDataHolder,
            ),
            (derivative_holder_ids::REFS_GRAPH, &rh),
            (derivative_holder_ids::NOTIF_HISTORY, &nh),
        ]);
        assert_eq!(registered.len(), 3);
        let search_reg = registered
            .iter()
            .find(|r| r.id == derivative_holder_ids::SEARCH_INDEX)
            .unwrap();
        assert_eq!(
            search_reg.phase,
            CanonicalErasePhase::PurgeAndTombstoneDerived
        );
        let notif_reg = registered
            .iter()
            .find(|r| r.id == derivative_holder_ids::NOTIF_HISTORY)
            .unwrap();
        assert_eq!(notif_reg.phase, CanonicalErasePhase::CachesAndDerivedCopies);
    }

    #[test]
    fn derivative_erase_carries_no_destroyed_key_epoch() {
        let search = SearchIndexModel::new();
        search.index_from_source("u-6", "x");
        let r = SearchIndexHolder::new(&search)
            .erase(subject_scope("u-6"))
            .unwrap();
        assert_eq!(
            r.receipt.key_epoch_destroyed, None,
            "a derived purge destroys no key (plaintext-derived)"
        );
    }

    #[test]
    fn holder_ids_sentinel_and_telemetry_are_stable() {
        assert_eq!(derivative_holder_ids::SEARCH_INDEX, "search_index");
        assert_eq!(derivative_holder_ids::REFS_GRAPH, "refs_graph");
        assert_eq!(derivative_holder_ids::NOTIF_HISTORY, "notif_history");
        assert_eq!(ERASED_USER, "[erased user]");
        assert_eq!(
            DERIVATIVE_ERASE_FANOUT_COVERAGE.0,
            "gdpr.derivative_erase_fanout_coverage"
        );
        assert_eq!(DERIVATIVE_ERASE_FANOUT_COVERAGE.1, "ratio");
    }

    #[test]
    fn receipt_flags_read_the_exact_post_conditions_both_polarities() {
        let search = SearchIndexModel::new();
        let refs = RefsGraphModel::new();
        let notif = NotifHistoryModel::new();
        search.index_from_source("u-pol", "v");
        refs.add_edge_from_source("u-pol", "tgt");
        notif.add_item_from_source("i", "u-pol");
        assert_eq!(
            search.reidentify_hits("u-pol"),
            1,
            "embedding re-identifies ⇒ embeddings_purged would be FALSE"
        );
        assert!(
            !matches!(refs.resolve("u-pol"), RefsResolve::Tombstone),
            "Live ⇒ refs_tombstoned would be FALSE"
        );
        assert_eq!(
            notif.erase_call_count(),
            0,
            "no erase ⇒ notif_humanised would be FALSE"
        );

        let sh = SearchIndexHolder::new(&search);
        let rh = RefsGraphHolder::new(&refs);
        let nh = NotifHistoryHolder::new(&notif);
        let receipt = DerivativeErasureDriver::fan_out_erase(
            &subject_scope("u-pol"),
            &search,
            &sh,
            &refs,
            &rh,
            &notif,
            &nh,
        )
        .unwrap();
        assert!(
            receipt.embeddings_purged,
            "after erase: embeddings_purged TRUE (probe == 0)"
        );
        assert!(
            receipt.refs_tombstoned,
            "after erase: refs_tombstoned TRUE (resolve is Tombstone)"
        );
        assert!(
            receipt.notif_humanised,
            "after erase: notif_humanised TRUE (erase ran)"
        );
    }

    #[test]
    fn locate_verdicts_distinguish_present_from_zero_recoverable() {
        let search = SearchIndexModel::new();
        let refs = RefsGraphModel::new();
        search.index_from_source("u-loc", "x");
        refs.add_edge_from_source("u-loc", "e");
        let sh = SearchIndexHolder::new(&search);
        let rh = RefsGraphHolder::new(&refs);
        let s_present = sh.locate(&subject("u-loc"), t("acme")).unwrap().receipt;
        let r_present = rh.locate(&subject("u-loc"), t("acme")).unwrap().receipt;
        sh.erase(subject_scope("u-loc")).unwrap();
        rh.erase(subject_scope("u-loc")).unwrap();
        let s_zero = sh.locate(&subject("u-loc"), t("acme")).unwrap().receipt;
        let r_zero = rh.locate(&subject("u-loc"), t("acme")).unwrap().receipt;
        assert_ne!(
            s_present.content_hash, s_zero.content_hash,
            "Search locate verdict differs present vs 0-recoverable"
        );
        assert_ne!(
            r_present.content_hash, r_zero.content_hash,
            "Refs locate verdict differs present vs 0-recoverable"
        );
        let s_expect_present = Receipt::content_addressed(
            "locate",
            derivative_holder_ids::SEARCH_INDEX,
            "u-loc",
            "acme",
            "located:indexed",
            None,
            0,
        );
        let s_expect_zero = Receipt::content_addressed(
            "locate",
            derivative_holder_ids::SEARCH_INDEX,
            "u-loc",
            "acme",
            "located:0-recoverable",
            None,
            0,
        );
        assert_eq!(s_present, s_expect_present);
        assert_eq!(s_zero, s_expect_zero);
        let r_expect_present = Receipt::content_addressed(
            "locate",
            derivative_holder_ids::REFS_GRAPH,
            "u-loc",
            "acme",
            "located:edges-present",
            None,
            0,
        );
        let r_expect_zero = Receipt::content_addressed(
            "locate",
            derivative_holder_ids::REFS_GRAPH,
            "u-loc",
            "acme",
            "located:0-recoverable",
            None,
            0,
        );
        assert_eq!(r_present, r_expect_present);
        assert_eq!(r_zero, r_expect_zero);
    }

    #[test]
    fn derivative_erase_is_idempotent() {
        let search = SearchIndexModel::new();
        search.index_from_source("u-7", "y");
        let holder = SearchIndexHolder::new(&search);
        holder.erase(subject_scope("u-7")).unwrap();
        holder.erase(subject_scope("u-7")).unwrap();
        assert_eq!(search.reidentify_hits("u-7"), 0);
        assert_eq!(search.erase_call_count(), 2, "both erase calls counted");
    }
}
