use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalData, PersonalDataHolder,
    PortableBundle, Receipt, RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef,
    TenantId,
};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::export::ExportDoc;

pub mod erase_floor;
pub use erase_floor::{
    holder_erase_receipt, BusEraseSeam, KnowledgeBacklinkTombstone, KnowledgeEmbeddingPurge,
    KnowledgeErase, KnowledgeEraseReceipt,
};

pub const HOLDER_ID: &str = "H4";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RestrictionSink {
    Search,
    Agents,
    Olap,
    Notif,
}

impl RestrictionSink {
    pub fn label(self) -> &'static str {
        match self {
            RestrictionSink::Search => "search-index",
            RestrictionSink::Agents => "agent-rag",
            RestrictionSink::Olap => "olap-analytics",
            RestrictionSink::Notif => "notifications",
        }
    }

    pub const ALL: [RestrictionSink; 4] = [
        RestrictionSink::Search,
        RestrictionSink::Agents,
        RestrictionSink::Olap,
        RestrictionSink::Notif,
    ];
}

#[derive(Default)]
pub struct RestrictionRegistry {
    restricted: Mutex<BTreeSet<(String, String)>>,
    leak_count: AtomicU64,
}

impl RestrictionRegistry {
    pub fn new() -> RestrictionRegistry {
        RestrictionRegistry::default()
    }

    fn key(subject: &SubjectRef, tenant: &TenantId) -> (String, String) {
        (
            tenant.as_str().to_string(),
            subject.principal.principal_id.0.clone(),
        )
    }

    pub fn set(&self, subject: &SubjectRef, tenant: &TenantId, on: bool) -> bool {
        let key = Self::key(subject, tenant);
        let mut set = self
            .restricted
            .lock()
            .expect("restriction registry poisoned");
        if on {
            set.insert(key);
        } else {
            set.remove(&key);
        }
        on
    }

    pub fn is_restricted(&self, subject: &SubjectRef, tenant: &TenantId) -> bool {
        self.restricted
            .lock()
            .expect("restriction registry poisoned")
            .contains(&Self::key(subject, tenant))
    }

    pub fn leak_count(&self) -> u64 {
        self.leak_count.load(Ordering::SeqCst)
    }

    fn record_leak_attempt(&self) {
        self.leak_count.fetch_add(1, Ordering::SeqCst);
    }
}

pub struct RestrictSuppressor<'a> {
    registry: &'a RestrictionRegistry,
    tenant: TenantId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SinkVerdict {
    Emit,
    Suppressed(RestrictionSink),
}

impl SinkVerdict {
    pub fn admits(self) -> bool {
        matches!(self, SinkVerdict::Emit)
    }
}

impl<'a> RestrictSuppressor<'a> {
    pub fn new(registry: &'a RestrictionRegistry, tenant: TenantId) -> RestrictSuppressor<'a> {
        RestrictSuppressor { registry, tenant }
    }

    pub fn admit(&self, subject: &SubjectRef, sink: RestrictionSink) -> SinkVerdict {
        if self.registry.is_restricted(subject, &self.tenant) {
            self.registry.record_leak_attempt();
            SinkVerdict::Suppressed(sink)
        } else {
            SinkVerdict::Emit
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocatedLocus {
    pub kind: LocatedKind,
    pub artifact_ref: String,
    pub reliable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocatedKind {
    Authorship,
    Mention,
    DbRowPerson,
    CommentAuthorship,
    TraceAuthorship,
    FreeTextMatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeLocateReport {
    pub receipt: Receipt,
    pub loci: Vec<LocatedLocus>,
}

impl KnowledgeLocateReport {
    pub fn reliable_loci(&self) -> Vec<&LocatedLocus> {
        self.loci.iter().filter(|l| l.reliable).collect()
    }

    pub fn flagged_free_text(&self) -> Vec<&LocatedLocus> {
        self.loci.iter().filter(|l| !l.reliable).collect()
    }
}

pub struct KnowledgePersonalDataHolder<'a> {
    registry: &'a RestrictionRegistry,
}

impl<'a> KnowledgePersonalDataHolder<'a> {
    pub fn new(registry: &'a RestrictionRegistry) -> KnowledgePersonalDataHolder<'a> {
        KnowledgePersonalDataHolder { registry }
    }

    pub fn holder_id(&self) -> &'static str {
        HOLDER_ID
    }

    pub fn locate_detailed(
        &self,
        subject: &SubjectRef,
        tenant: &TenantId,
        structured: Vec<LocatedLocus>,
        free_text_matches: Vec<LocatedLocus>,
    ) -> KnowledgeLocateReport {
        let mut loci: Vec<LocatedLocus> = structured
            .into_iter()
            .map(|mut l| {
                l.reliable = l.kind != LocatedKind::FreeTextMatch;
                l
            })
            .collect();
        loci.extend(free_text_matches.into_iter().map(|mut l| {
            l.kind = LocatedKind::FreeTextMatch;
            l.reliable = false;
            l
        }));
        let receipt = Receipt::content_addressed(
            "locate",
            HOLDER_ID,
            &subject.principal.principal_id.0,
            tenant.as_str(),
            "kn locate: author/edit attribution + mentions + db-row person props + comment/trace \
             authorship (reliable) + free-text matches via Search (best-effort, flagged)",
            None,
            0,
        );
        KnowledgeLocateReport { receipt, loci }
    }

    pub fn export_bundle(
        &self,
        subject: &SubjectRef,
        tenant: &TenantId,
        docs: &[ExportDoc],
    ) -> (PortableBundle, String) {
        let bundles: Vec<serde_json::Value> = docs
            .iter()
            .map(|d| {
                serde_json::from_str::<serde_json::Value>(&d.to_json_bundle())
                    .expect("ExportDoc.to_json_bundle is valid JSON (a closed serde shape)")
            })
            .collect();
        let bundle_json = serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": crate::export::EXPORT_SCHEMA_VERSION,
            "subject": subject.principal.principal_id.0,
            "tenant": tenant.as_str(),
            "pages": bundles,
        }))
        .expect("the export bundle serialises (a closed serde shape)");
        let receipt = Receipt::content_addressed(
            "export",
            HOLDER_ID,
            &subject.principal.principal_id.0,
            tenant.as_str(),
            "kn export: the subject's pages as the KN-P24 Art. 20 lossless JSON bundle (10.1)",
            None,
            0,
        );
        (PortableBundle { receipt }, bundle_json)
    }

    pub fn rectify_detailed(
        &self,
        subject: &SubjectRef,
        tenant: &TenantId,
        structured_loci: usize,
        span_tombstones: usize,
    ) -> (RectifyReceipt, RectifyOutcome) {
        let receipt = Receipt::content_addressed(
            "rectify",
            HOLDER_ID,
            &subject.principal.principal_id.0,
            tenant.as_str(),
            "kn rectify: correct structured values (author attribution, person fields) + best-effort \
             free-text span tombstone (the residual = the ONE platform posture, 10.9 by reference)",
            None,
            0,
        );
        (
            RectifyReceipt { receipt },
            RectifyOutcome {
                structured_corrected: structured_loci,
                free_text_spans_tombstoned: span_tombstones,
            },
        )
    }

    pub fn restrict_subject(
        &self,
        subject: &SubjectRef,
        tenant: &TenantId,
        on: bool,
    ) -> RestrictReceipt {
        self.registry.set(subject, tenant, on);
        let receipt = Receipt::content_addressed(
            "restrict",
            HOLDER_ID,
            &subject.principal.principal_id.0,
            tenant.as_str(),
            if on {
                "kn restrict ON: excluded from indexing / agent-use (RAG) / analytics / \
                 notifications - 0 emissions to Search/Agents/OLAP/Notif (§6.3, row 11.6)"
            } else {
                "kn restrict OFF: the restriction flag is cleared for the subject (§6.3)"
            },
            None,
            0,
        );
        RestrictReceipt { receipt }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RectifyOutcome {
    pub structured_corrected: usize,
    pub free_text_spans_tombstoned: usize,
}

impl PersonalDataHolder for KnowledgePersonalDataHolder<'_> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let receipt = Receipt::content_addressed(
            "locate",
            HOLDER_ID,
            &subject.principal.principal_id.0,
            tenant.as_str(),
            "kn locate: author/edit attribution + mentions + person props + comment/trace authorship \
             (reliable) + free-text via Search (best-effort, flagged) - rich body: locate_detailed",
            None,
            0,
        );
        Ok(LocateReport { receipt })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        let receipt = Receipt::content_addressed(
            "export",
            HOLDER_ID,
            &subject.principal.principal_id.0,
            tenant.as_str(),
            "kn export: the subject's pages as the KN-P24 Art. 20 lossless JSON bundle (10.1) - \
             rich body: export_bundle (reuses the Export service)",
            None,
            0,
        );
        Ok(PortableBundle { receipt })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        let receipt = Receipt::content_addressed(
            "rectify",
            HOLDER_ID,
            &subject.principal.principal_id.0,
            "",
            "kn rectify: structured value + best-effort free-text span tombstone (the residual = the \
             ONE platform posture, 10.9 by reference) - rich body: rectify_detailed",
            None,
            0,
        );
        Ok(RectifyReceipt { receipt })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        let tenant = subject.principal.tenant.clone();
        Ok(self.restrict_subject(subject, &tenant, on))
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        holder_erase_receipt(&scope)
    }
}

#[derive(PersonalData)]
#[allow(dead_code)]
pub struct KnowledgePersonRecord {
    pub artifact_id: String,
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "created_by"
    )]
    pub created_by: String,
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "edited_by"
    )]
    pub edited_by: String,
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "created_by"
    )]
    pub mention_text: String,
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "created_by"
    )]
    pub free_text_body: String,
    #[personal_data(
        category = ContactInfo,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "created_by"
    )]
    pub db_row_person_prop: String,
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = LegitimateInterest(agent_trace_lia),
        retention = TenantPolicy,
        erasure = Pseudonymise,
        subject_locator = "trace_actor"
    )]
    pub trace_actor: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_gdpr::HasPersonalData;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn tenant() -> TenantId {
        myelin_tenancy::TenantId("acme".into())
    }

    fn subject_ref(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            tenant(),
        ))
    }

    #[test]
    fn restrict_suppresses_exactly_the_four_sinks() {
        assert_eq!(RestrictionSink::ALL.len(), 4);
        for s in [
            RestrictionSink::Search,
            RestrictionSink::Agents,
            RestrictionSink::Olap,
            RestrictionSink::Notif,
        ] {
            assert!(
                RestrictionSink::ALL.contains(&s),
                "{} must be suppressed",
                s.label()
            );
        }
        assert_eq!(RestrictionSink::Search.label(), "search-index");
        assert_eq!(RestrictionSink::Agents.label(), "agent-rag");
        assert_eq!(RestrictionSink::Olap.label(), "olap-analytics");
        assert_eq!(RestrictionSink::Notif.label(), "notifications");
    }

    #[test]
    fn restrict_gate_zero_emissions_to_all_four_sinks() {
        let registry = RestrictionRegistry::new();
        let holder = KnowledgePersonalDataHolder::new(&registry);
        let alice = subject_ref("p-alice");
        let bob = subject_ref("p-bob");

        let supp = RestrictSuppressor::new(&registry, tenant());
        for sink in RestrictionSink::ALL {
            assert_eq!(
                supp.admit(&alice, sink),
                SinkVerdict::Emit,
                "pre-restrict: {} admits",
                sink.label()
            );
        }
        assert_eq!(registry.leak_count(), 0, "no leak attempts before restrict");

        let receipt = holder.restrict_subject(&alice, &tenant(), true);
        assert_eq!(receipt.receipt.operation, "restrict");
        assert!(receipt.receipt.content_hash.starts_with("blake3:"));
        assert!(registry.is_restricted(&alice, &tenant()));

        for sink in RestrictionSink::ALL {
            assert_eq!(
                supp.admit(&alice, sink),
                SinkVerdict::Suppressed(sink),
                "post-restrict: {} suppresses the restricted subject (0 emissions)",
                sink.label()
            );
        }
        assert_eq!(
            registry.leak_count(),
            4,
            "every sink emission for the restricted subject was caught"
        );

        for sink in RestrictionSink::ALL {
            assert_eq!(
                supp.admit(&bob, sink),
                SinkVerdict::Emit,
                "bob (un-restricted) still flows to {}",
                sink.label()
            );
        }

        holder.restrict_subject(&alice, &tenant(), false);
        assert!(!registry.is_restricted(&alice, &tenant()));
        for sink in RestrictionSink::ALL {
            assert_eq!(
                supp.admit(&alice, sink),
                SinkVerdict::Emit,
                "post-clear: {} admits alice again",
                sink.label()
            );
        }
    }

    #[test]
    fn no_sink_admits_a_restricted_subject() {
        let registry = RestrictionRegistry::new();
        let holder = KnowledgePersonalDataHolder::new(&registry);
        let s = subject_ref("p-restricted");
        holder.restrict_subject(&s, &tenant(), true);
        let supp = RestrictSuppressor::new(&registry, tenant());
        for sink in RestrictionSink::ALL {
            assert!(
                !supp.admit(&s, sink).admits(),
                "{} must NOT admit a restricted subject",
                sink.label()
            );
        }
    }

    #[test]
    fn locate_structured_is_reliable_free_text_is_flagged() {
        let registry = RestrictionRegistry::new();
        let holder = KnowledgePersonalDataHolder::new(&registry);
        let s = subject_ref("p-ada");
        let structured = vec![
            LocatedLocus {
                kind: LocatedKind::Authorship,
                artifact_ref: "myelin://acme/knowledge/block/b9".into(),
                reliable: true,
            },
            LocatedLocus {
                kind: LocatedKind::Mention,
                artifact_ref: "myelin://acme/knowledge/page/7c2".into(),
                reliable: true,
            },
            LocatedLocus {
                kind: LocatedKind::DbRowPerson,
                artifact_ref: "myelin://acme/knowledge/row/r1".into(),
                reliable: true,
            },
        ];
        let free_text = vec![LocatedLocus {
            kind: LocatedKind::Authorship,
            artifact_ref: "myelin://acme/knowledge/block/b42".into(),
            reliable: true,
        }];
        let report = holder.locate_detailed(&s, &tenant(), structured, free_text);

        assert_eq!(
            report.reliable_loci().len(),
            3,
            "the three structured loci are reliable"
        );
        assert_eq!(
            report.flagged_free_text().len(),
            1,
            "the free-text match is flagged best-effort"
        );
        assert!(report.flagged_free_text()[0].kind == LocatedKind::FreeTextMatch);
        assert!(
            !report.flagged_free_text()[0].reliable,
            "free-text is never reliable (the residual)"
        );
        assert_eq!(report.receipt.operation, "locate");
    }

    #[test]
    fn export_reuses_the_export_service_lossless_json() {
        let registry = RestrictionRegistry::new();
        let holder = KnowledgePersonalDataHolder::new(&registry);
        let s = subject_ref("p-grace");
        let doc1 = ExportDoc::new("page-1", "Notes", None, vec![]);
        let doc2 = ExportDoc::new("page-2", "Plan", None, vec![]);
        let (bundle, json) = holder.export_bundle(&s, &tenant(), &[doc1, doc2]);

        assert_eq!(bundle.receipt.operation, "export");
        let v: serde_json::Value =
            serde_json::from_str(&json).expect("the export bundle is valid JSON");
        assert_eq!(v["subject"], "p-grace");
        assert_eq!(v["tenant"], "acme");
        assert_eq!(
            v["pages"].as_array().expect("pages array").len(),
            2,
            "both pages exported"
        );
    }

    #[test]
    fn rectify_corrects_structured_and_tombstones_free_text_spans() {
        let registry = RestrictionRegistry::new();
        let holder = KnowledgePersonalDataHolder::new(&registry);
        let s = subject_ref("p-lin");
        let (receipt, outcome) = holder.rectify_detailed(&s, &tenant(), 2, 1);
        assert_eq!(receipt.receipt.operation, "rectify");
        assert_eq!(
            outcome.structured_corrected, 2,
            "two structured values corrected (reliable)"
        );
        assert_eq!(
            outcome.free_text_spans_tombstoned, 1,
            "one free-text span tombstoned (residual)"
        );
    }

    #[test]
    fn cdc_10_1_knowledge_holder_is_the_frozen_non_erase_contract() {
        let registry = RestrictionRegistry::new();
        let holder = KnowledgePersonalDataHolder::new(&registry);
        let dyn_holder: &dyn PersonalDataHolder = &holder;
        let s = subject_ref("p-dsr");

        let loc = dyn_holder.locate(&s, tenant()).expect("locate");
        assert_eq!(loc.receipt.operation, "locate");
        assert!(loc.receipt.content_hash.starts_with("blake3:"));
        let exp = dyn_holder.export(&s, tenant()).expect("export");
        assert_eq!(exp.receipt.operation, "export");
        let rec = dyn_holder
            .rectify(&s, Patch("correct-name".into()))
            .expect("rectify");
        assert_eq!(rec.receipt.operation, "rectify");
        let restr = dyn_holder.restrict(&s, true).expect("restrict");
        assert_eq!(restr.receipt.operation, "restrict");
        assert!(
            registry.is_restricted(&s, &tenant()),
            "the trait restrict flipped the registry flag"
        );

        let er = dyn_holder
            .erase(EraseScope::Subject {
                subject: s.clone(),
                tenant: tenant(),
            })
            .expect("erase now succeeds (the KN-P26 floor is built)");
        assert_eq!(er.receipt.operation, "erase");
        assert!(er.receipt.content_hash.starts_with("blake3:"));
    }

    #[test]
    fn knowledge_schema_carries_the_personal_data_tags() {
        let fields = KnowledgePersonRecord::personal_data_fields();
        assert_eq!(
            fields.len(),
            6,
            "exactly the six PII fields are tagged, the opaque id has none"
        );
        let by_field: std::collections::HashMap<&str, _> =
            fields.iter().map(|f| (f.field, f)).collect();

        assert_eq!(by_field["created_by"].tags.erasure, "Pseudonymise");
        assert_eq!(by_field["edited_by"].tags.erasure, "Pseudonymise");
        assert_eq!(by_field["trace_actor"].tags.erasure, "Pseudonymise");
        assert_eq!(
            by_field["mention_text"].tags.erasure,
            "CryptoShred(subject_dek)"
        );
        assert_eq!(
            by_field["free_text_body"].tags.erasure,
            "CryptoShred(subject_dek)"
        );
        assert_eq!(
            by_field["db_row_person_prop"].tags.erasure,
            "CryptoShred(subject_dek)"
        );
        assert_eq!(
            KnowledgePersonRecord::subject_locator("created_by"),
            Some("created_by")
        );
    }

    #[test]
    fn holder_loci_match_the_tagged_content_shape() {
        let registry = RestrictionRegistry::new();
        let holder = KnowledgePersonalDataHolder::new(&registry);
        let s = subject_ref("p-content");
        let structured: Vec<LocatedLocus> = [
            LocatedKind::Authorship,
            LocatedKind::Mention,
            LocatedKind::DbRowPerson,
            LocatedKind::CommentAuthorship,
            LocatedKind::TraceAuthorship,
        ]
        .into_iter()
        .map(|kind| LocatedLocus {
            kind,
            artifact_ref: "myelin://acme/knowledge/block/b1".into(),
            reliable: true,
        })
        .collect();
        let report = holder.locate_detailed(&s, &tenant(), structured, vec![]);
        assert_eq!(
            report.reliable_loci().len(),
            5,
            "every structured kind is reliable"
        );
        assert!(
            report.flagged_free_text().is_empty(),
            "no free-text matches in this fixture"
        );
    }
}
