use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Holder {
    GitDb,
    CiDb,
    IssuesDb,
    KnowledgeDb,
    ChatDb,
    ObjectStore,
    SearchIndex,
    EventBus,
    CachesAndCdn,
    Backups,
    AgentMemory,
    ReferenceGraph,
    NotificationHistory,
    AuthzTuples,
    Identity,
    AuditCarveOut,
    AgentTrace,
    GdprOwnStores,
}

impl Holder {
    pub const ALL: &'static [Holder] = &[
        Holder::GitDb,
        Holder::CiDb,
        Holder::IssuesDb,
        Holder::KnowledgeDb,
        Holder::ChatDb,
        Holder::ObjectStore,
        Holder::SearchIndex,
        Holder::EventBus,
        Holder::CachesAndCdn,
        Holder::Backups,
        Holder::AgentMemory,
        Holder::ReferenceGraph,
        Holder::NotificationHistory,
        Holder::AuthzTuples,
        Holder::Identity,
        Holder::AuditCarveOut,
        Holder::AgentTrace,
        Holder::GdprOwnStores,
    ];

    pub fn h_label(self) -> &'static str {
        match self {
            Holder::GitDb => "H1",
            Holder::CiDb => "H2",
            Holder::IssuesDb => "H3",
            Holder::KnowledgeDb => "H4",
            Holder::ChatDb => "H5",
            Holder::ObjectStore => "H6",
            Holder::SearchIndex => "H7",
            Holder::EventBus => "H8",
            Holder::CachesAndCdn => "H9",
            Holder::Backups => "H10",
            Holder::AgentMemory => "H11",
            Holder::ReferenceGraph => "H12",
            Holder::NotificationHistory => "H13",
            Holder::AuthzTuples => "H14",
            Holder::Identity => "H15",
            Holder::AuditCarveOut => "H16",
            Holder::AgentTrace => "H17",
            Holder::GdprOwnStores => "H18",
        }
    }

    pub fn holder_id(self) -> &'static str {
        match self {
            Holder::GitDb => "git_db",
            Holder::CiDb => "ci_db",
            Holder::IssuesDb => "issues_db",
            Holder::KnowledgeDb => "knowledge_db",
            Holder::ChatDb => "chat_db",
            Holder::ObjectStore => "blob_store",
            Holder::SearchIndex => "search_index_vectors",
            Holder::EventBus => "event_bus",
            Holder::CachesAndCdn => "cache_cdn",
            Holder::Backups => "backups",
            Holder::AgentMemory => "agent_memory",
            Holder::ReferenceGraph => "refs_edges",
            Holder::NotificationHistory => "notif_inbox",
            Holder::AuthzTuples => "authz_tuples",
            Holder::Identity => "identity",
            Holder::AuditCarveOut => "audit_carve_out",
            Holder::AgentTrace => "agent_trace",
            Holder::GdprOwnStores => "gdpr_own_stores",
        }
    }

    pub fn erasure(self) -> HolderErasure {
        match self {
            Holder::GitDb
            | Holder::CiDb
            | Holder::IssuesDb
            | Holder::KnowledgeDb
            | Holder::ChatDb
            | Holder::AgentMemory
            | Holder::AgentTrace => HolderErasure::CryptoShredPerSubjectDek,
            Holder::ObjectStore | Holder::GdprOwnStores => HolderErasure::CryptoShredBlobDek,
            Holder::SearchIndex => HolderErasure::PurgeAndReindex,
            Holder::EventBus => HolderErasure::CryptoShredInlineKeysAndTombstone,
            Holder::CachesAndCdn | Holder::ReferenceGraph | Holder::NotificationHistory => {
                HolderErasure::PurgeOrTombstoneDerived
            }
            Holder::AuthzTuples => HolderErasure::DeleteTuples,
            Holder::Identity => HolderErasure::DeletePseudonymMapAndShredProfile,
            Holder::Backups => HolderErasure::CryptoShredByConstruction,
            Holder::AuditCarveOut => HolderErasure::AuditCarveOutResidual,
        }
    }

    pub fn is_audit_carve_out(self) -> bool {
        matches!(self, Holder::AuditCarveOut)
    }

    pub fn carries_vectors(self) -> bool {
        matches!(self, Holder::SearchIndex | Holder::AgentMemory)
    }

    pub fn from_id(id: &str) -> Option<Holder> {
        let bare = id.rsplit(':').next().unwrap_or(id);
        match bare {
            "git_db" | "git_oltp" | "git" => Some(Holder::GitDb),
            "ci_db" | "ci_oltp" | "ci_logs" | "ci" => Some(Holder::CiDb),
            "issues_db" | "issue_oltp" | "issues" => Some(Holder::IssuesDb),
            "knowledge_db" | "knowledge_oltp" | "knowledge" => Some(Holder::KnowledgeDb),
            "chat_db" | "chat_oltp" | "chat_bodies" | "chat" => Some(Holder::ChatDb),
            "blob_store" | "object_store" | "blob" => Some(Holder::ObjectStore),
            "search_index_vectors" | "search_index" | "search" => Some(Holder::SearchIndex),
            "event_bus" | "bus" => Some(Holder::EventBus),
            "cache_cdn" | "caches" | "cdn" => Some(Holder::CachesAndCdn),
            "backups" | "backup" => Some(Holder::Backups),
            "agent_memory" | "memory" => Some(Holder::AgentMemory),
            "refs_edges" | "refs_edge" | "reference_graph" | "refs" => Some(Holder::ReferenceGraph),
            "notif_inbox" | "notif_history" | "notify" | "notifications" => {
                Some(Holder::NotificationHistory)
            }
            "authz_tuples" | "authz" => Some(Holder::AuthzTuples),
            "identity" | "identity_oltp" => Some(Holder::Identity),
            "audit_carve_out" | "audit" => Some(Holder::AuditCarveOut),
            "agent_trace" | "agent_fabric_trace" | "agent_trace_seam" => Some(Holder::AgentTrace),
            "gdpr_own_stores" | "gdpr_owned" | "gdpr" => Some(Holder::GdprOwnStores),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HolderErasure {
    CryptoShredPerSubjectDek,
    CryptoShredBlobDek,
    PurgeAndReindex,
    CryptoShredInlineKeysAndTombstone,
    PurgeOrTombstoneDerived,
    DeleteTuples,
    DeletePseudonymMapAndShredProfile,
    CryptoShredByConstruction,
    AuditCarveOutResidual,
}

pub const ERASURE_FANOUT_COVERAGE: (&str, &str) = ("gdpr.erasure_fanout_coverage", "ratio");

#[derive(Clone, Debug, Default)]
pub struct FullFanOutCoverage {
    reached: BTreeSet<Holder>,
    unrecognised: BTreeSet<String>,
}

impl FullFanOutCoverage {
    pub fn new() -> FullFanOutCoverage {
        FullFanOutCoverage::default()
    }

    pub fn record_reached_id(&mut self, holder_id: &str) -> bool {
        match Holder::from_id(holder_id) {
            Some(h) => {
                self.reached.insert(h);
                true
            }
            None => {
                self.unrecognised.insert(holder_id.to_string());
                false
            }
        }
    }

    pub fn record_reached(&mut self, holder: Holder) {
        self.reached.insert(holder);
    }

    pub fn holders_missed(&self) -> usize {
        Holder::ALL
            .iter()
            .filter(|h| !self.reached.contains(h))
            .count()
    }

    pub fn missed(&self) -> Vec<Holder> {
        Holder::ALL
            .iter()
            .copied()
            .filter(|h| !self.reached.contains(h))
            .collect()
    }

    pub fn unrecognised(&self) -> Vec<String> {
        self.unrecognised.iter().cloned().collect()
    }

    pub fn erasure_fanout_coverage(&self) -> f64 {
        let reached_in_catalogue = Holder::ALL
            .iter()
            .filter(|h| self.reached.contains(h))
            .count();
        reached_in_catalogue as f64 / Holder::ALL.len() as f64
    }

    pub fn is_complete(&self) -> bool {
        self.holders_missed() == 0
    }

    pub fn reach_manifest(&self) -> Vec<HolderReach> {
        Holder::ALL
            .iter()
            .map(|&h| HolderReach {
                holder: h,
                reached: self.reached.contains(&h),
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HolderReach {
    pub holder: Holder,
    pub reached: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GaD1Certificate {
    pub scope_token: String,
    pub reach: Vec<HolderReach>,
    pub holders_missed: usize,
    pub erasure_fanout_coverage: f64,
    pub content_hash: String,
}

impl GaD1Certificate {
    pub fn seal(
        scope_token: &str,
        coverage: &FullFanOutCoverage,
    ) -> Result<GaD1Certificate, GaD1Gap> {
        if !coverage.is_complete() {
            return Err(GaD1Gap {
                missed: coverage.missed(),
                holders_missed: coverage.holders_missed(),
                erasure_fanout_coverage: coverage.erasure_fanout_coverage(),
            });
        }
        let reach = coverage.reach_manifest();
        let content_hash = content_address(scope_token, &reach, 0, 1.0);
        Ok(GaD1Certificate {
            scope_token: scope_token.to_string(),
            reach,
            holders_missed: 0,
            erasure_fanout_coverage: 1.0,
            content_hash,
        })
    }

    pub fn is_complete(&self) -> bool {
        self.holders_missed == 0
            && self.erasure_fanout_coverage == 1.0
            && self.reach.len() == Holder::ALL.len()
            && self.reach.iter().all(|r| r.reached)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GaD1Gap {
    pub missed: Vec<Holder>,
    pub holders_missed: usize,
    pub erasure_fanout_coverage: f64,
}

fn content_address(
    scope_token: &str,
    reach: &[HolderReach],
    holders_missed: usize,
    coverage: f64,
) -> String {
    let mut body = format!("ga_d1\u{1f}scope={scope_token}");
    for r in reach {
        body.push('\u{1f}');
        body.push_str(&format!("{}={}", r.holder.h_label(), r.reached));
    }
    body.push_str(&format!(
        "\u{1f}holders_missed={holders_missed}\u{1f}coverage={coverage}"
    ));
    let digest = blake3::hash(body.as_bytes());
    format!("blake3:{}", hex::encode(digest.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_is_exactly_h1_to_h18() {
        assert_eq!(
            Holder::ALL.len(),
            18,
            "the §3.2 list is exhaustive - 18 holders"
        );
        let labels: Vec<&str> = Holder::ALL.iter().map(|h| h.h_label()).collect();
        let expected: Vec<String> = (1..=18).map(|n| format!("H{n}")).collect();
        let expected_refs: Vec<&str> = expected.iter().map(|s| s.as_str()).collect();
        assert_eq!(labels, expected_refs, "labelled H1..H18 in order");
        let ids: BTreeSet<&str> = Holder::ALL.iter().map(|h| h.holder_id()).collect();
        assert_eq!(ids.len(), 18, "18 distinct holder ids");
        let label_set: BTreeSet<&str> = labels.iter().copied().collect();
        assert_eq!(label_set.len(), 18, "18 distinct H-labels");
    }

    #[test]
    fn from_id_resolves_canonical_and_aliases() {
        for &h in Holder::ALL {
            assert_eq!(
                Holder::from_id(h.holder_id()),
                Some(h),
                "{} canonical id",
                h.h_label()
            );
        }
        assert_eq!(Holder::from_id("oltp:ci_oltp"), Some(Holder::CiDb));
        assert_eq!(Holder::from_id("oltp:git_oltp"), Some(Holder::GitDb));
        assert_eq!(Holder::from_id("oltp:issue_oltp"), Some(Holder::IssuesDb));
        assert_eq!(
            Holder::from_id("oltp:knowledge_oltp"),
            Some(Holder::KnowledgeDb)
        );
        assert_eq!(Holder::from_id("oltp:chat_oltp"), Some(Holder::ChatDb));
        assert_eq!(
            Holder::from_id("blob:blob_store"),
            Some(Holder::ObjectStore)
        );
        assert_eq!(
            Holder::from_id("search_index:search_index"),
            Some(Holder::SearchIndex)
        );
        assert_eq!(
            Holder::from_id("refs_edge:refs_edge"),
            Some(Holder::ReferenceGraph)
        );
        assert_eq!(
            Holder::from_id("oltp:agent_fabric_trace"),
            Some(Holder::AgentTrace)
        );
        assert_eq!(Holder::from_id("identity_oltp"), Some(Holder::Identity));
        assert_eq!(
            Holder::from_id("notif_history"),
            Some(Holder::NotificationHistory)
        );
        assert_eq!(Holder::from_id("not_a_holder"), None);
    }

    #[test]
    fn a_full_fan_out_is_complete_0_missed_coverage_1() {
        let mut cov = FullFanOutCoverage::new();
        for &h in Holder::ALL {
            cov.record_reached(h);
        }
        assert_eq!(cov.holders_missed(), 0, "0 holders missed");
        assert_eq!(cov.erasure_fanout_coverage(), 1.0, "100% coverage");
        assert!(cov.is_complete());
        assert!(cov.missed().is_empty());
    }

    #[test]
    fn a_missed_holder_is_detected_not_masked() {
        let mut cov = FullFanOutCoverage::new();
        for &h in Holder::ALL {
            if h != Holder::SearchIndex {
                cov.record_reached(h);
            }
        }
        assert_eq!(cov.holders_missed(), 1, "the missed holder is COUNTED");
        assert_eq!(cov.missed(), vec![Holder::SearchIndex], "named: H7 Search");
        assert!(
            cov.erasure_fanout_coverage() < 1.0,
            "coverage dropped below 1.0"
        );
        assert!(!cov.is_complete(), "an incomplete fan-out is NOT complete");
    }

    #[test]
    fn coverage_denominator_is_the_whole_catalogue_not_the_reached_subset() {
        let mut cov = FullFanOutCoverage::new();
        cov.record_reached(Holder::Identity);
        assert!(
            (cov.erasure_fanout_coverage() - 1.0 / 18.0).abs() < 1e-12,
            "one reached holder is 1/18, NOT vacuously 1.0"
        );
        assert_eq!(cov.holders_missed(), 17);
    }

    #[test]
    fn an_unrecognised_id_is_not_counted_as_a_reach() {
        let mut cov = FullFanOutCoverage::new();
        assert!(
            !cov.record_reached_id("typo_holder"),
            "an unknown id does not resolve"
        );
        assert_eq!(cov.holders_missed(), 18, "nothing reached - all 18 missed");
        assert_eq!(cov.unrecognised(), vec!["typo_holder".to_string()]);
        assert!(cov.record_reached_id("identity"));
        assert_eq!(cov.holders_missed(), 17);
    }

    #[test]
    fn certificate_seals_only_on_a_complete_fan_out() {
        let mut full = FullFanOutCoverage::new();
        for &h in Holder::ALL {
            full.record_reached(h);
        }
        let cert = GaD1Certificate::seal("acme/u-1", &full).expect("a complete fan-out seals");
        assert!(cert.is_complete());
        assert_eq!(cert.holders_missed, 0);
        assert_eq!(cert.erasure_fanout_coverage, 1.0);
        assert_eq!(cert.reach.len(), 18);
        assert!(cert.reach.iter().all(|r| r.reached));
        assert!(cert.content_hash.starts_with("blake3:"));

        let mut partial = FullFanOutCoverage::new();
        for &h in Holder::ALL {
            if h != Holder::AuditCarveOut {
                partial.record_reached(h);
            }
        }
        let gap =
            GaD1Certificate::seal("acme/u-1", &partial).expect_err("a missed holder does NOT seal");
        assert_eq!(gap.holders_missed, 1);
        assert_eq!(gap.missed, vec![Holder::AuditCarveOut]);
        assert!(gap.erasure_fanout_coverage < 1.0);
    }

    #[test]
    fn certificate_is_complete_validates_each_field_independently() {
        let mut cov = FullFanOutCoverage::new();
        for &h in Holder::ALL {
            cov.record_reached(h);
        }
        let good = GaD1Certificate::seal("acme/u", &cov).unwrap();
        assert!(good.is_complete(), "the sealed certificate is complete");

        let mut t1 = good.clone();
        t1.holders_missed = 1;
        assert!(!t1.is_complete(), "a non-zero missed count fails the gate");

        let mut t2 = good.clone();
        t2.erasure_fanout_coverage = 0.5;
        assert!(!t2.is_complete(), "a coverage below 1.0 fails the gate");

        let mut t3 = good.clone();
        t3.reach.pop();
        assert!(
            !t3.is_complete(),
            "a manifest missing a holder line fails the gate"
        );

        let mut t4 = good.clone();
        t4.reach[0].reached = false;
        assert!(
            !t4.is_complete(),
            "a manifest line marked un-reached fails the gate"
        );
    }

    #[test]
    fn certificate_content_address_is_deterministic_and_scope_sensitive() {
        let mut cov = FullFanOutCoverage::new();
        for &h in Holder::ALL {
            cov.record_reached(h);
        }
        let a = GaD1Certificate::seal("acme/u-1", &cov).unwrap();
        let a2 = GaD1Certificate::seal("acme/u-1", &cov).unwrap();
        assert_eq!(a.content_hash, a2.content_hash, "deterministic");
        let b = GaD1Certificate::seal("acme/u-2", &cov).unwrap();
        assert_ne!(
            a.content_hash, b.content_hash,
            "the scope is in the content address"
        );
    }

    #[test]
    fn audit_carve_out_is_a_reached_holder_with_the_residual_modality() {
        assert!(Holder::AuditCarveOut.is_audit_carve_out());
        assert_eq!(
            Holder::AuditCarveOut.erasure(),
            HolderErasure::AuditCarveOutResidual
        );
        let mut cov = FullFanOutCoverage::new();
        for &h in Holder::ALL {
            if !h.is_audit_carve_out() {
                cov.record_reached(h);
            }
        }
        assert_eq!(
            cov.holders_missed(),
            1,
            "the carve-out is still a holder that must be reached"
        );
    }

    #[test]
    fn vector_carrying_holders_are_search_and_agent_memory() {
        let with_vectors: BTreeSet<Holder> = Holder::ALL
            .iter()
            .copied()
            .filter(|h| h.carries_vectors())
            .collect();
        assert_eq!(
            with_vectors,
            BTreeSet::from([Holder::SearchIndex, Holder::AgentMemory])
        );
    }

    #[test]
    fn coverage_telemetry_name_and_unit_are_pinned() {
        assert_eq!(ERASURE_FANOUT_COVERAGE.0, "gdpr.erasure_fanout_coverage");
        assert_eq!(ERASURE_FANOUT_COVERAGE.1, "ratio");
    }

    #[test]
    fn every_holder_has_an_erasure_modality() {
        for &h in Holder::ALL {
            let _ = h.erasure();
        }
        assert_eq!(
            Holder::Identity.erasure(),
            HolderErasure::DeletePseudonymMapAndShredProfile
        );
        assert_eq!(
            Holder::Backups.erasure(),
            HolderErasure::CryptoShredByConstruction
        );
        assert_eq!(
            Holder::SearchIndex.erasure(),
            HolderErasure::PurgeAndReindex
        );
    }
}
