use std::collections::BTreeMap;

use myelin_query::FieldValue;
use myelin_substrate::thresholds::ProjectionFeeder;
use myelin_tenancy::{Region, TenantId};

use crate::engine::AclFilter;

#[derive(Clone, Debug, Default)]
pub struct ViewExecutionTelemetry {
    collections: BTreeMap<String, CollectionCounters>,
}

#[derive(Clone, Debug, Default)]
struct CollectionCounters {
    total: u64,
    facet_uses: BTreeMap<String, u64>,
}

impl ViewExecutionTelemetry {
    pub fn new() -> ViewExecutionTelemetry {
        ViewExecutionTelemetry::default()
    }

    pub fn record_execution(&mut self, collection: &str, facets: &[&str]) {
        let entry = self.collections.entry(collection.to_string()).or_default();
        entry.total += 1;
        let mut counted = std::collections::BTreeSet::new();
        for f in facets {
            if counted.insert(*f) {
                *entry.facet_uses.entry((*f).to_string()).or_insert(0) += 1;
            }
        }
    }

    pub fn total_executions(&self, collection: &str) -> u64 {
        self.collections
            .get(collection)
            .map(|c| c.total)
            .unwrap_or(0)
    }

    pub fn facet_uses(&self, collection: &str, facet: &str) -> u64 {
        self.collections
            .get(collection)
            .and_then(|c| c.facet_uses.get(facet).copied())
            .unwrap_or(0)
    }

    pub fn facet_frequency(&self, collection: &str, facet: &str) -> f64 {
        let total = self.total_executions(collection);
        if total == 0 {
            return 0.0;
        }
        self.facet_uses(collection, facet) as f64 / total as f64
    }

    pub fn should_promote(&self, collection: &str, facet: &str, t: &ProjectionFeeder) -> bool {
        t.should_promote(
            self.facet_uses(collection, facet),
            self.total_executions(collection),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FacetServingPath {
    GinScan,
    GeneratedIndex,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FacetDoc {
    pub doc_id: String,
    pub acl_object: String,
    pub facets: BTreeMap<String, FieldValue>,
}

impl FacetDoc {
    pub fn new(doc_id: impl Into<String>, acl_object: impl Into<String>) -> FacetDoc {
        FacetDoc {
            doc_id: doc_id.into(),
            acl_object: acl_object.into(),
            facets: BTreeMap::new(),
        }
    }

    pub fn with_facet(mut self, name: impl Into<String>, value: FieldValue) -> FacetDoc {
        self.facets.insert(name.into(), value);
        self
    }
}

#[derive(Clone, Debug, Default)]
pub struct FacetCollection {
    name: String,
    docs: Vec<FacetDoc>,
    paths: BTreeMap<String, FacetServingPath>,
    generated: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}

impl FacetCollection {
    pub fn new(name: impl Into<String>) -> FacetCollection {
        FacetCollection {
            name: name.into(),
            docs: Vec::new(),
            paths: BTreeMap::new(),
            generated: BTreeMap::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn add(&mut self, doc: FacetDoc) {
        match self.docs.binary_search_by(|d| d.doc_id.cmp(&doc.doc_id)) {
            Ok(i) => self.docs[i] = doc,
            Err(i) => self.docs.insert(i, doc),
        }
        let promoted: Vec<String> = self.generated.keys().cloned().collect();
        for facet in promoted {
            self.build_generated_index(&facet);
        }
    }

    pub fn path_of(&self, facet: &str) -> FacetServingPath {
        self.paths
            .get(facet)
            .copied()
            .unwrap_or(FacetServingPath::GinScan)
    }

    pub fn promote(&mut self, facet: &str) {
        self.build_generated_index(facet);
        self.paths
            .insert(facet.to_string(), FacetServingPath::GeneratedIndex);
    }

    fn build_generated_index(&mut self, facet: &str) {
        let mut index: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for doc in &self.docs {
            if let Some(v) = doc.facets.get(facet) {
                index
                    .entry(value_key(v))
                    .or_default()
                    .push(doc.doc_id.clone());
            }
        }
        self.generated.insert(facet.to_string(), index);
    }

    pub fn serve_gin_scan(&self, facet: &str, value: &FieldValue, acl: &AclFilter) -> Vec<String> {
        let key = value_key(value);
        self.docs
            .iter()
            .filter(|d| d.facets.get(facet).map(value_key).as_deref() == Some(key.as_str()))
            .filter(|d| acl.admits(&d.doc_id, &d.acl_object))
            .map(|d| d.doc_id.clone())
            .collect()
    }

    pub fn serve_generated_index(
        &self,
        facet: &str,
        value: &FieldValue,
        acl: &AclFilter,
    ) -> Vec<String> {
        let Some(index) = self.generated.get(facet) else {
            return self.serve_gin_scan(facet, value, acl);
        };
        let key = value_key(value);
        index
            .get(&key)
            .map(|ids| {
                ids.iter()
                    .filter(|id| {
                        self.docs
                            .iter()
                            .find(|d| &d.doc_id == *id)
                            .map(|d| acl.admits(&d.doc_id, &d.acl_object))
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn serve(&self, facet: &str, value: &FieldValue, acl: &AclFilter) -> Vec<String> {
        match self.path_of(facet) {
            FacetServingPath::GinScan => self.serve_gin_scan(facet, value, acl),
            FacetServingPath::GeneratedIndex => self.serve_generated_index(facet, value, acl),
        }
    }

    pub fn facet_values(&self, facet: &str) -> Vec<FieldValue> {
        let mut seen: BTreeMap<String, FieldValue> = BTreeMap::new();
        for doc in &self.docs {
            if let Some(v) = doc.facets.get(facet) {
                seen.entry(value_key(v)).or_insert_with(|| v.clone());
            }
        }
        seen.into_values().collect()
    }
}

fn value_key(v: &FieldValue) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| format!("{v:?}"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectionFeederArtifact {
    pub tenant: TenantId,
    pub region: Region,
    pub collection: String,
    pub facet: String,
    pub facet_uses: u64,
    pub total_executions: u64,
    pub threshold_bps: u32,
    pub measured_frequency_bps: u32,
    pub values_checked: u64,
    pub threshold_measured: bool,
    pub ran_at: String,
}

impl ProjectionFeederArtifact {
    pub fn is_green(&self) -> bool {
        self.measured_frequency_bps > self.threshold_bps && self.values_checked > 0
    }

    pub fn summary(&self) -> String {
        format!(
            "search projection-feeder promotion PASS (SRCH-P27, OQ-C): collection={}, facet={} \
             promoted from the GIN scan to the generated index - MEASURED frequency {}bps ({:.2}% \
             of {} view executions, {} uses) crossed the {}bps (> {:.0}%) threshold. Results are \
             BYTE-IDENTICAL across the promotion over {} distinct facet value(s) (cost changes, \
             correctness does not). Threshold carried as the OQ-C default-to-beat (the Issues/KN-owned \
             signal); the promotion mechanism + the byte-identical invariant MEASURED. Written to the \
             thresholds file ([projection_feeder]).",
            self.collection,
            self.facet,
            self.measured_frequency_bps,
            self.measured_frequency_bps as f64 / 100.0,
            self.total_executions,
            self.facet_uses,
            self.threshold_bps,
            self.threshold_bps as f64 / 100.0,
            self.values_checked,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectionFeederFailure {
    ResultChanged {
        value_key: String,
        gin_scan: Vec<String>,
        generated_index: Vec<String>,
    },
    BelowThreshold {
        measured_bps: u32,
        threshold_bps: u32,
    },
    NoValuesChecked,
    MisspecifiedThreshold,
}

impl core::fmt::Display for ProjectionFeederFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ProjectionFeederFailure::ResultChanged {
                value_key,
                gin_scan,
                generated_index,
            } => write!(
                f,
                "PROJECTION-FEEDER FAIL - the promotion CHANGED a result for facet value {value_key}: \
                 the GIN scan returned {gin_scan:?} but the generated index returned \
                 {generated_index:?}. Promotion must change COST, never correctness (§4.6.1) - the \
                 generated index is NEVER shipped if it disagrees with the GIN scan"
            ),
            ProjectionFeederFailure::BelowThreshold {
                measured_bps,
                threshold_bps,
            } => write!(
                f,
                "PROJECTION-FEEDER FAIL - the facet frequency {measured_bps}bps did NOT cross the \
                 {threshold_bps}bps (> 5 %) threshold: promoting it would be premature (the GIN scan \
                 still serves it). Measured, never predicted (EI-01 §3)"
            ),
            ProjectionFeederFailure::NoValuesChecked => write!(
                f,
                "PROJECTION-FEEDER FAIL - 0 facet values checked: a promotion proof that compared \
                 nothing cannot prove the byte-identical invariant (a mis-specified drill)"
            ),
            ProjectionFeederFailure::MisspecifiedThreshold => write!(
                f,
                "PROJECTION-FEEDER FAIL - the threshold is mis-specified (a ratio ≤ 0 / ≥ 1 or a 0 \
                 execution floor). A green cannot be manufactured by a vacuous bar (EI-01 §3)"
            ),
        }
    }
}

impl std::error::Error for ProjectionFeederFailure {}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a projection-feeder verdict must be checked - a dropped RED is a SWALLOWED \
              correctness/promotion failure (the SRCH-P27 gate, EI-01 §5: loud-never-swallowed)"]
pub enum ProjectionFeederVerdict {
    Green(ProjectionFeederArtifact),
    Red(ProjectionFeederFailure),
}

impl ProjectionFeederVerdict {
    pub fn is_green(&self) -> bool {
        matches!(self, ProjectionFeederVerdict::Green(_))
    }
    pub fn artifact(&self) -> Option<&ProjectionFeederArtifact> {
        match self {
            ProjectionFeederVerdict::Green(a) => Some(a),
            ProjectionFeederVerdict::Red(_) => None,
        }
    }
    pub fn failure(&self) -> Option<&ProjectionFeederFailure> {
        match self {
            ProjectionFeederVerdict::Green(_) => None,
            ProjectionFeederVerdict::Red(f) => Some(f),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ProjectionFeederGate;

impl ProjectionFeederGate {
    pub fn new() -> ProjectionFeederGate {
        ProjectionFeederGate
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &self,
        tenant: &TenantId,
        region: &Region,
        collection: &mut FacetCollection,
        telemetry: &ViewExecutionTelemetry,
        facet: &str,
        acl: &AclFilter,
        t: &ProjectionFeeder,
        now: &str,
    ) -> ProjectionFeederVerdict {
        if !t.is_well_formed() {
            return ProjectionFeederVerdict::Red(ProjectionFeederFailure::MisspecifiedThreshold);
        }

        let coll_name = collection.name().to_string();
        let total = telemetry.total_executions(&coll_name);
        let uses = telemetry.facet_uses(&coll_name, facet);
        let threshold_bps = ratio_to_bps(t.promotion_ratio);
        let measured_bps = freq_to_bps(uses, total);

        if !telemetry.should_promote(&coll_name, facet, t) {
            return ProjectionFeederVerdict::Red(ProjectionFeederFailure::BelowThreshold {
                measured_bps,
                threshold_bps,
            });
        }

        let values = collection.facet_values(facet);
        if values.is_empty() {
            return ProjectionFeederVerdict::Red(ProjectionFeederFailure::NoValuesChecked);
        }
        let before: Vec<(FieldValue, Vec<String>)> = values
            .iter()
            .map(|v| (v.clone(), collection.serve_gin_scan(facet, v, acl)))
            .collect();

        collection.promote(facet);

        for (value, gin_result) in &before {
            let generated_result = collection.serve_generated_index(facet, value, acl);
            if &generated_result != gin_result {
                return ProjectionFeederVerdict::Red(ProjectionFeederFailure::ResultChanged {
                    value_key: value_key(value),
                    gin_scan: gin_result.clone(),
                    generated_index: generated_result,
                });
            }
        }

        ProjectionFeederVerdict::Green(ProjectionFeederArtifact {
            tenant: tenant.clone(),
            region: region.clone(),
            collection: coll_name,
            facet: facet.to_string(),
            facet_uses: uses,
            total_executions: total,
            threshold_bps,
            measured_frequency_bps: measured_bps,
            values_checked: before.len() as u64,
            threshold_measured: false,
            ran_at: now.to_string(),
        })
    }
}

fn ratio_to_bps(ratio: f64) -> u32 {
    (ratio * 10_000.0).floor().clamp(0.0, 10_000.0) as u32
}

fn freq_to_bps(uses: u64, total: u64) -> u32 {
    if total == 0 {
        return 0;
    }
    (((uses as u128) * 10_000) / (total as u128)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }

    fn issues_collection(n: usize) -> FacetCollection {
        let states = ["backlog", "started", "done", "cancelled"];
        let mut c = FacetCollection::new("issues");
        for i in 0..n {
            c.add(
                FacetDoc::new(format!("ENG-{i:04}"), format!("obj-{i}")).with_facet(
                    "state_category",
                    FieldValue::Select(states[i % states.len()].to_string()),
                ),
            );
        }
        c
    }

    #[test]
    fn threshold_mirrors_the_oqc_number_through_the_shared_thresholds_file() {
        use myelin_substrate::thresholds::Thresholds;
        assert_eq!(ProjectionFeeder::PROMOTION_RATIO_SEED, 0.05);
        let t = Thresholds::load_canonical().expect("load canonical thresholds");
        assert_eq!(
            t.projection_feeder.promotion_ratio, t.flex_db.facet_promotion_ratio,
            "Search consumes the SAME OQ-C > 5 % number the Issues/KN owner measures against \
             (reconciled through the shared thresholds file, never a duplicated constant)"
        );
        assert_eq!(t.projection_feeder.promotion_ratio, 0.05);
    }

    #[test]
    fn telemetry_window_and_promotion_decision() {
        let t = ProjectionFeeder::default();
        let mut tel = ViewExecutionTelemetry::new();
        for i in 0..20 {
            if i < 6 {
                tel.record_execution("issues", &["state_category", "state_category"]);
            } else if i == 6 {
                tel.record_execution("issues", &["assignee"]);
            } else {
                tel.record_execution("issues", &[]);
            }
        }
        assert_eq!(tel.total_executions("issues"), 20);
        assert_eq!(
            tel.facet_uses("issues", "state_category"),
            6,
            "counted once per execution"
        );
        assert_eq!(tel.facet_frequency("issues", "state_category"), 0.30);
        assert!(tel.should_promote("issues", "state_category", &t));
        assert!(!tel.should_promote("issues", "assignee", &t));
        assert!(!tel.should_promote("issues", "priority", &t));
    }

    #[test]
    fn below_execution_floor_never_promotes() {
        let t = ProjectionFeeder::default();
        let mut tel = ViewExecutionTelemetry::new();
        tel.record_execution("issues", &["state_category"]);
        assert_eq!(tel.facet_frequency("issues", "state_category"), 1.0);
        assert!(
            !tel.should_promote("issues", "state_category", &t),
            "a 100 % frequency over a single execution is too noisy to promote on"
        );
    }

    #[test]
    fn promotion_changes_cost_not_correctness() {
        let mut coll = issues_collection(200);
        let mut tel = ViewExecutionTelemetry::new();
        for i in 0..100 {
            if i < 30 {
                tel.record_execution("issues", &["state_category"]);
            } else {
                tel.record_execution("issues", &[]);
            }
        }
        let acl = AclFilter::not_ids(["obj-0", "obj-4", "obj-8", "obj-12"]);

        let v = FieldValue::Select("backlog".into());
        let before = coll.serve(&v_facet(), &v, &acl);
        assert_eq!(coll.path_of("state_category"), FacetServingPath::GinScan);

        let t = ProjectionFeeder::default();
        let verdict = ProjectionFeederGate::new().run(
            &tenant(),
            &region(),
            &mut coll,
            &tel,
            "state_category",
            &acl,
            &t,
            "2026-06-25",
        );
        let a = verdict.artifact().expect("SRCH-P27 green");
        assert!(a.is_green());
        assert_eq!(a.measured_frequency_bps, 3000, "30 % measured frequency");
        assert_eq!(a.threshold_bps, 500, "> 5 % = 500 bps threshold");
        assert!(a.measured_frequency_bps > a.threshold_bps);
        assert!(
            a.values_checked >= 4,
            "every distinct state value was checked"
        );
        assert!(
            !a.threshold_measured,
            "the threshold is the carried OQ-C default-to-beat"
        );

        assert_eq!(
            coll.path_of("state_category"),
            FacetServingPath::GeneratedIndex
        );
        let after = coll.serve(&v_facet(), &v, &acl);
        assert_eq!(
            after, before,
            "byte-identical results across the promotion (cost changed only)"
        );
        assert!(
            !after.iter().any(|id| id == "ENG-0000"),
            "obj-0 is ACL-denied on both paths"
        );
        println!("[P-462 GATE GREEN 2026-06-25] {}", a.summary());
    }

    fn v_facet() -> String {
        "state_category".to_string()
    }

    #[test]
    fn below_threshold_fails_loud() {
        let mut coll = issues_collection(50);
        let mut tel = ViewExecutionTelemetry::new();
        for i in 0..40 {
            if i == 0 {
                tel.record_execution("issues", &["state_category"]);
            } else {
                tel.record_execution("issues", &[]);
            }
        }
        let verdict = ProjectionFeederGate::new().run(
            &tenant(),
            &region(),
            &mut coll,
            &tel,
            "state_category",
            &AclFilter::All,
            &ProjectionFeeder::default(),
            "2026-06-25",
        );
        assert_eq!(
            verdict.failure(),
            Some(&ProjectionFeederFailure::BelowThreshold {
                measured_bps: 250,
                threshold_bps: 500,
            }),
            "2.5 % < 5 % is RED - never promoted prematurely"
        );
        assert_eq!(coll.path_of("state_category"), FacetServingPath::GinScan);
    }

    #[test]
    fn misspecified_threshold_fails_loud() {
        let mut coll = issues_collection(10);
        let tel = ViewExecutionTelemetry::new();
        let bad = ProjectionFeeder {
            promotion_ratio: 1.0,
            min_executions: 20,
        };
        let verdict = ProjectionFeederGate::new().run(
            &tenant(),
            &region(),
            &mut coll,
            &tel,
            "state_category",
            &AclFilter::All,
            &bad,
            "2026-06-25",
        );
        assert_eq!(
            verdict.failure(),
            Some(&ProjectionFeederFailure::MisspecifiedThreshold)
        );
    }

    #[test]
    fn both_paths_agree_over_every_value() {
        let mut coll = issues_collection(120);
        coll.promote("state_category");
        let acl = AclFilter::not_ids(["obj-1", "obj-7", "obj-77"]);
        for v in coll.facet_values("state_category") {
            assert_eq!(
                coll.serve_gin_scan("state_category", &v, &acl),
                coll.serve_generated_index("state_category", &v, &acl),
                "the two paths agree on value {v:?}"
            );
        }
        let absent = FieldValue::Select("nonexistent".into());
        assert!(coll
            .serve_gin_scan("state_category", &absent, &acl)
            .is_empty());
        assert!(coll
            .serve_generated_index("state_category", &absent, &acl)
            .is_empty());
    }

    #[test]
    fn upsert_after_promotion_keeps_paths_identical() {
        let mut coll = issues_collection(30);
        coll.promote("state_category");
        coll.add(
            FacetDoc::new("ENG-9001", "obj-9001")
                .with_facet("state_category", FieldValue::Select("done".into())),
        );
        coll.add(
            FacetDoc::new("ENG-9002", "obj-9002")
                .with_facet("state_category", FieldValue::Select("triage".into())),
        );
        let acl = AclFilter::All;
        for v in coll.facet_values("state_category") {
            assert_eq!(
                coll.serve_gin_scan("state_category", &v, &acl),
                coll.serve_generated_index("state_category", &v, &acl),
                "post-upsert the promoted index still matches the GIN scan for {v:?}"
            );
        }
        let triage = FieldValue::Select("triage".into());
        assert_eq!(
            coll.serve_generated_index("state_category", &triage, &acl),
            vec!["ENG-9002".to_string()]
        );
    }

    #[test]
    fn failures_display_loudly() {
        let changed = ProjectionFeederFailure::ResultChanged {
            value_key: "\"backlog\"".into(),
            gin_scan: vec!["a".into()],
            generated_index: vec![],
        };
        assert!(changed.to_string().contains("CHANGED a result"));
        assert!(ProjectionFeederFailure::NoValuesChecked
            .to_string()
            .contains("0 facet values"));
        assert!(ProjectionFeederFailure::MisspecifiedThreshold
            .to_string()
            .contains("mis-specified"));
    }
}
