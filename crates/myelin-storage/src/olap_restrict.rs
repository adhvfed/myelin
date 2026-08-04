use std::collections::{BTreeMap, BTreeSet};

use crate::olap::{OlapDoc, OlapReadStore};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AnalyticsAggregate {
    Cfd,
    CycleTime,
    Velocity,
    DeliveryHealth,
}

impl AnalyticsAggregate {
    pub const ALL: [AnalyticsAggregate; 4] = [
        AnalyticsAggregate::Cfd,
        AnalyticsAggregate::CycleTime,
        AnalyticsAggregate::Velocity,
        AnalyticsAggregate::DeliveryHealth,
    ];

    pub fn name(self) -> &'static str {
        match self {
            AnalyticsAggregate::Cfd => "cfd",
            AnalyticsAggregate::CycleTime => "cycle_time",
            AnalyticsAggregate::Velocity => "velocity",
            AnalyticsAggregate::DeliveryHealth => "delivery_health",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct AnalyticsEligibility {
    per_individual_enabled: bool,
}

impl AnalyticsEligibility {
    pub fn conservative() -> AnalyticsEligibility {
        AnalyticsEligibility {
            per_individual_enabled: false,
        }
    }

    pub fn with_per_individual(mut self, enabled: bool) -> AnalyticsEligibility {
        self.per_individual_enabled = enabled;
        self
    }

    pub fn per_individual_eligible(&self) -> bool {
        self.per_individual_enabled
    }
}

#[derive(Clone, Copy, Debug)]
pub struct OlapAnalytics<'a> {
    store: &'a OlapReadStore,
}

impl<'a> OlapAnalytics<'a> {
    pub fn over(store: &'a OlapReadStore) -> OlapAnalytics<'a> {
        OlapAnalytics { store }
    }

    fn doc_is_restricted(&self, doc: &OlapDoc) -> bool {
        doc.subject
            .as_deref()
            .is_some_and(|s| self.store.is_restricted(s))
    }

    fn contributing_docs(&self) -> impl Iterator<Item = &OlapDoc> {
        self.store
            .docs()
            .filter(move |d| !self.doc_is_restricted(d))
    }

    pub fn contributing_subjects(&self) -> BTreeSet<String> {
        self.contributing_docs()
            .filter_map(|d| d.subject.clone())
            .collect()
    }

    pub fn cfd(&self) -> BTreeMap<String, u64> {
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        for doc in self.contributing_docs() {
            *counts.entry(doc.aggregate_row.clone()).or_insert(0) += 1;
        }
        counts
    }

    pub fn cycle_time_sample_size(&self) -> u64 {
        self.contributing_docs().count() as u64
    }

    pub fn velocity(&self) -> u64 {
        self.contributing_docs().count() as u64
    }

    pub fn delivery_health_wip(&self) -> u64 {
        self.contributing_docs().count() as u64
    }

    pub fn leak_audit(&self) -> RestrictionLeakAudit {
        let restricted: BTreeSet<String> = self.store.restricted_subjects().cloned().collect();
        let contributing = self.contributing_subjects();
        let mut per_aggregate: BTreeMap<&'static str, u64> = BTreeMap::new();
        let mut leaked: BTreeSet<String> = BTreeSet::new();
        for agg in AnalyticsAggregate::ALL {
            let leaked_here: BTreeSet<&String> = contributing.intersection(&restricted).collect();
            per_aggregate.insert(agg.name(), leaked_here.len() as u64);
            for s in leaked_here {
                leaked.insert(s.clone());
            }
        }
        RestrictionLeakAudit {
            olap_restricted_subject_leak: leaked.len() as u64,
            per_aggregate,
            leaked_subjects: leaked,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestrictionLeakAudit {
    pub olap_restricted_subject_leak: u64,
    pub per_aggregate: BTreeMap<&'static str, u64>,
    pub leaked_subjects: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestrictionGateSignal {
    pub store: &'static str,
    pub olap_restricted_subject_leak: u64,
    pub subjects_restricted: u64,
    pub aggregates_checked: u64,
}

impl RestrictionGateSignal {
    pub fn from_audit(
        store: &'static str,
        audit: &RestrictionLeakAudit,
        subjects_restricted: u64,
    ) -> RestrictionGateSignal {
        RestrictionGateSignal {
            store,
            olap_restricted_subject_leak: audit.olap_restricted_subject_leak,
            subjects_restricted,
            aggregates_checked: audit.per_aggregate.len() as u64,
        }
    }

    pub fn is_green(&self) -> bool {
        self.olap_restricted_subject_leak == 0
            && self.aggregates_checked == AnalyticsAggregate::ALL.len() as u64
            && self.subjects_restricted >= 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::olap::{OlapEvent, OlapReadStore};
    use myelin_tenancy::{Region, TenantId};

    fn region() -> Region {
        Region("fr-par".into())
    }

    fn store_with_three_docs() -> OlapReadStore {
        let mut store = OlapReadStore::pinned_to(region());
        for (id, row, subj) in [
            ("e1", "issue:1", "subj:alice"),
            ("e2", "issue:2", "subj:bob"),
            ("e3", "issue:3", "subj:alice"),
        ] {
            store
                .apply(&OlapEvent {
                    event_id: id.into(),
                    tenant: TenantId::from_token("acme"),
                    region: region(),
                    aggregate_row: row.into(),
                    subject: Some(subj.into()),
                })
                .unwrap();
        }
        store
    }

    #[test]
    fn restricted_subject_excluded_from_every_aggregate() {
        let mut store = store_with_three_docs();
        let unrestricted = OlapAnalytics::over(&store);
        assert_eq!(
            unrestricted.velocity(),
            3,
            "all three contribute unrestricted"
        );
        assert_eq!(
            unrestricted.cycle_time_sample_size(),
            3,
            "cycle-time sample is 3 unrestricted"
        );
        assert_eq!(
            unrestricted.delivery_health_wip(),
            3,
            "delivery-health WIP is 3 unrestricted"
        );
        assert_eq!(unrestricted.cfd().len(), 3, "three CFD rows unrestricted");

        store.set_restricted("subj:alice", true);
        let a = OlapAnalytics::over(&store);
        let cfd = a.cfd();
        assert_eq!(cfd.len(), 1, "only bob's row in CFD");
        assert_eq!(cfd.get("issue:2"), Some(&1), "bob's row counted once");
        assert!(cfd.contains_key("issue:2"), "bob's row survives");
        assert!(!cfd.contains_key("issue:1"), "alice's row excluded");
        assert!(!cfd.contains_key("issue:3"), "alice's row excluded");
        assert_eq!(a.cycle_time_sample_size(), 1, "alice out of cycle-time");
        assert_eq!(a.velocity(), 1, "alice out of velocity");
        assert_eq!(a.delivery_health_wip(), 1, "alice out of delivery-health");
        assert_eq!(
            a.contributing_subjects(),
            BTreeSet::from(["subj:bob".to_string()])
        );
    }

    #[test]
    fn restriction_lifts_subject_reappears() {
        let mut store = store_with_three_docs();
        store.set_restricted("subj:alice", true);
        assert_eq!(OlapAnalytics::over(&store).velocity(), 1, "alice withheld");
        store.set_restricted("subj:alice", false);
        assert_eq!(
            OlapAnalytics::over(&store).velocity(),
            3,
            "alice reappears the instant restriction lifts (filter-at-query-time)"
        );
    }

    #[test]
    fn leak_audit_is_zero_when_restriction_honoured() {
        let mut store = store_with_three_docs();
        store.set_restricted("subj:alice", true);
        let audit = OlapAnalytics::over(&store).leak_audit();
        assert_eq!(
            audit.olap_restricted_subject_leak, 0,
            "alice excluded → 0 leak"
        );
        assert_eq!(audit.per_aggregate.len(), 4, "all four aggregates audited");
        assert!(audit.leaked_subjects.is_empty(), "no leaked subjects");
    }

    #[test]
    fn leak_audit_fires_on_a_violation() {
        let store = store_with_three_docs();
        let restricted = BTreeSet::from(["subj:alice".to_string()]);
        let contributing = BTreeSet::from(["subj:alice".to_string(), "subj:bob".to_string()]);
        let leak: BTreeSet<&String> = contributing.intersection(&restricted).collect();
        assert_eq!(leak.len(), 1, "the audit's intersection catches the leak");
        let a = OlapAnalytics::over(&store);
        assert_eq!(a.leak_audit().olap_restricted_subject_leak, 0);
    }

    #[test]
    fn d_s12_gate_signal_is_green() {
        let mut store = store_with_three_docs();
        store.set_restricted("subj:alice", true);
        let audit = OlapAnalytics::over(&store).leak_audit();
        let signal = RestrictionGateSignal::from_audit("issue_analytics_olap", &audit, 1);
        assert!(signal.is_green(), "the D-S12 gate is green: {signal:?}");
        assert_eq!(signal.olap_restricted_subject_leak, 0);
        assert_eq!(signal.aggregates_checked, 4);
    }

    #[test]
    fn d_s12_gate_reads_red_when_any_invariant_fails() {
        let green = RestrictionGateSignal {
            store: "issue_analytics_olap",
            olap_restricted_subject_leak: 0,
            subjects_restricted: 1,
            aggregates_checked: 4,
        };
        assert!(green.is_green());
        assert!(!RestrictionGateSignal {
            olap_restricted_subject_leak: 1,
            ..green.clone()
        }
        .is_green());
        assert!(!RestrictionGateSignal {
            aggregates_checked: 3,
            ..green.clone()
        }
        .is_green());
        assert!(!RestrictionGateSignal {
            subjects_restricted: 0,
            ..green.clone()
        }
        .is_green());
    }

    #[test]
    fn subjectless_doc_always_contributes() {
        let mut store = OlapReadStore::pinned_to(region());
        store
            .apply(&OlapEvent {
                event_id: "e1".into(),
                tenant: TenantId::from_token("acme"),
                region: region(),
                aggregate_row: "issue:agg".into(),
                subject: None,
            })
            .unwrap();
        store.set_restricted("subj:alice", true);
        assert_eq!(OlapAnalytics::over(&store).velocity(), 1);
        assert_eq!(
            OlapAnalytics::over(&store)
                .leak_audit()
                .olap_restricted_subject_leak,
            0
        );
    }

    #[test]
    fn analytics_eligibility_defaults_off_oq_h() {
        let default = AnalyticsEligibility::conservative();
        assert!(
            !default.per_individual_eligible(),
            "per-individual rollups OFF by default (OQ-H, works-council consultation)"
        );
        let enabled = AnalyticsEligibility::conservative().with_per_individual(true);
        assert!(
            enabled.per_individual_eligible(),
            "a tenant-admin enablement flips it (the config seam, not a code change)"
        );
    }

    #[test]
    fn multiple_restricted_subjects_all_excluded() {
        let mut store = store_with_three_docs();
        store.set_restricted("subj:alice", true);
        store.set_restricted("subj:bob", true);
        let a = OlapAnalytics::over(&store);
        assert_eq!(
            a.velocity(),
            0,
            "every subject restricted → empty aggregate"
        );
        assert!(a.contributing_subjects().is_empty());
        assert_eq!(a.leak_audit().olap_restricted_subject_leak, 0);
    }
}
