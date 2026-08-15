use std::collections::BTreeMap;

use myelin_query::{FieldType, FieldValue};

use crate::analysis::{Analyzer, Language};
use crate::indexer::{IndexSpec, SearchProjection};

pub const CI_SUBSYSTEM: &str = "ci";

pub const CI_LOG_TYPE: &str = "log";

pub const CI_LOG_ACL_OBJECT_TYPE: &str = "ci_run";
pub const CI_LOG_ACL_ARTIFACT_TYPE: &str = "run";

pub const FACET_RUN_ID: &str = "run_id";
pub const FACET_JOB_ID: &str = "job_id";
pub const FACET_STEP_NO: &str = "step_no";

pub fn ci_log_index_spec() -> IndexSpec {
    let mut struct_fields: BTreeMap<String, FieldType> = BTreeMap::new();
    struct_fields.insert(FACET_RUN_ID.to_string(), FieldType::Relation);
    struct_fields.insert(FACET_JOB_ID.to_string(), FieldType::Text);
    struct_fields.insert(FACET_STEP_NO.to_string(), FieldType::Int);

    IndexSpec::new(CI_SUBSYSTEM, CI_LOG_TYPE, struct_fields)
        .with_parent_acl_object_type(CI_LOG_ACL_OBJECT_TYPE, CI_LOG_ACL_ARTIFACT_TYPE)
}

pub fn ci_log_index_specs() -> Vec<IndexSpec> {
    vec![ci_log_index_spec()]
}

pub fn register_ci_log_index_specs() -> Vec<IndexSpec> {
    let specs = ci_log_index_specs();
    let _accepted = crate::indexer::IncrementalIndexer::new(
        specs.clone(),
        std::sync::Arc::new(NullProjectFetcher),
        std::sync::Arc::new(crate::indexer::MockEmbeddingAdapter::new(8)),
    );
    specs
}

struct NullProjectFetcher;

impl crate::indexer::ProjectFetcher for NullProjectFetcher {
    fn project(
        &self,
        _tenant: &myelin_tenancy::TenantId,
        _region: &myelin_tenancy::Region,
        _ref_: &myelin_tenancy::ArtifactRef,
    ) -> Result<SearchProjection, crate::indexer::ProjectFetchError> {
        Err(crate::indexer::ProjectFetchError::Gone)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CiLogProjectionInput {
    pub run_id: String,
    pub job_id: String,
    pub step_no: u32,
    pub log_text: String,
    pub lang: Option<String>,
}

pub fn ci_log_search_projection(input: &CiLogProjectionInput) -> SearchProjection {
    let code = Analyzer::for_language(Language::Code);
    let text = code.analyze(&input.log_text).join(" ");

    let mut fields: BTreeMap<String, FieldValue> = BTreeMap::new();
    if !input.run_id.is_empty() {
        fields.insert(
            FACET_RUN_ID.to_string(),
            FieldValue::Relation(input.run_id.clone()),
        );
    }
    if !input.job_id.is_empty() {
        fields.insert(
            FACET_JOB_ID.to_string(),
            FieldValue::Text(input.job_id.clone()),
        );
    }
    fields.insert(
        FACET_STEP_NO.to_string(),
        FieldValue::Int(i64::from(input.step_no)),
    );

    SearchProjection {
        text,
        fields,
        lang: input
            .lang
            .clone()
            .or_else(|| Some(Language::Code.tag().to_string())),
    }
}

pub fn ci_log_doc_ref(tenant: &str, run_id: &str, job_id: &str, step_no: u32) -> String {
    format!("myelin://{tenant}/ci/{CI_LOG_TYPE}/{run_id}:{job_id}:{step_no}")
}

pub fn ci_log_details_ref(tenant: &str, run_id: &str, step_no: u32) -> String {
    format!("myelin://{tenant}/ci/run/{run_id}#step-{step_no}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiLogStepAnchor {
    pub run_id: String,
    pub step_no: u32,
}

pub fn parse_step_anchor(anchor: &str) -> Option<CiLogStepAnchor> {
    let (path, frag) = anchor.split_once('#')?;
    let step_no: u32 = frag.strip_prefix("step-")?.parse().ok()?;
    let run_id = path
        .split("ci/run/")
        .nth(1)
        .filter(|r| !r.is_empty())
        .map(|r| r.trim_end_matches('/').to_string())?;
    if run_id.is_empty() {
        return None;
    }
    Some(CiLogStepAnchor { run_id, step_no })
}

#[derive(Clone, Copy, Debug)]
pub struct CiLogDurableSegmentNotFirehoseFloor;

impl CiLogDurableSegmentNotFirehoseFloor {
    pub const DURABLE_TRANSPORT: &'static str = "evt.* (durable bus)";
    pub const NOT_THE_FIREHOSE: &'static str = "firehose live tier";
    pub const PER_SUBJECT_DEK_OWNER: &'static str = "myelin_storage::ci_log_index::CiLogTier";
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::IncrementalIndexer;

    #[test]
    fn ci_log_spec_is_the_consumed_11_8_shape() {
        let s = ci_log_index_spec();
        assert_eq!(s.subsystem, "ci");
        assert_eq!(s.type_, "log");
        assert_eq!(
            s.acl_object_type, "ci_run",
            "a CI log's reachability is its parent CI run's `view` (the blob→repo analog)"
        );
        assert!(
            !s.semantic,
            "CI logs are literal/symbol/path-grade full-text, not vector-embedded in v1"
        );
        assert_eq!(
            s.struct_fields.len(),
            3,
            "exactly the three (job, step, byte-range)-index facets"
        );
        assert_eq!(
            s.struct_fields.get(FACET_RUN_ID),
            Some(&FieldType::Relation)
        );
        assert_eq!(s.struct_fields.get(FACET_JOB_ID), Some(&FieldType::Text));
        assert_eq!(s.struct_fields.get(FACET_STEP_NO), Some(&FieldType::Int));
    }

    #[test]
    fn acl_object_is_the_parent_run_not_the_log() {
        let s = ci_log_index_spec();
        assert_eq!(s.acl_object_type, "ci_run");
        assert_ne!(
            s.acl_object_type, s.type_,
            "the ACL anchor is the parent run, NOT the per-log doc (no per-log ACL object)"
        );
    }

    #[test]
    fn log_text_is_not_a_struct_facet() {
        let s = ci_log_index_spec();
        for absent in ["log", "log_text", "text", "body", "bytes", "segment"] {
            assert!(
                !s.struct_fields.contains_key(absent),
                "`{absent}` is full-text projection body, not a structured facet"
            );
        }
    }

    #[test]
    fn registration_is_accepted_by_search() {
        let accepted = register_ci_log_index_specs();
        assert_eq!(
            accepted,
            ci_log_index_specs(),
            "Search accepts the declared CI-log spec verbatim"
        );
        let _ix = IncrementalIndexer::new(
            ci_log_index_specs(),
            std::sync::Arc::new(NullProjectFetcher),
            std::sync::Arc::new(crate::indexer::MockEmbeddingAdapter::new(8)),
        );
    }

    #[test]
    fn projection_builds_typed_facets_and_log_body() {
        let input = CiLogProjectionInput {
            run_id: "myelin://acme/ci/run/run-7".into(),
            job_id: "build".into(),
            step_no: 3,
            log_text: "FAIL: assertion at src/scheduler/deadlock.rs:42 detectDeadlock".into(),
            lang: None,
        };
        let p = ci_log_search_projection(&input);

        let toks: std::collections::BTreeSet<&str> = p.text.split(' ').collect();
        assert!(
            toks.contains("scheduler"),
            "a path segment is searchable: {:?}",
            p.text
        );
        assert!(toks.contains("deadlock"));
        assert!(
            toks.contains("detect"),
            "the camel-split identifier is searchable"
        );
        assert_eq!(
            p.lang.as_deref(),
            Some("code"),
            "CI logs default to the code chain"
        );

        assert_eq!(
            p.fields.get(FACET_RUN_ID),
            Some(&FieldValue::Relation("myelin://acme/ci/run/run-7".into()))
        );
        assert_eq!(
            p.fields.get(FACET_JOB_ID),
            Some(&FieldValue::Text("build".into()))
        );
        assert_eq!(p.fields.get(FACET_STEP_NO), Some(&FieldValue::Int(3)));

        let spec = ci_log_index_spec();
        for (name, value) in &p.fields {
            assert_eq!(
                value.field_type(),
                *spec.struct_fields.get(name).expect("facet is declared"),
                "facet `{name}` value type matches its spec declaration"
            );
        }
    }

    #[test]
    fn step_facet_is_always_stamped() {
        let p = ci_log_search_projection(&CiLogProjectionInput {
            run_id: String::new(),
            job_id: String::new(),
            step_no: 5,
            log_text: String::new(),
            lang: None,
        });
        assert_eq!(
            p.fields.get(FACET_STEP_NO),
            Some(&FieldValue::Int(5)),
            "the step is always the index key, even with no run/job/text"
        );
        assert!(
            !p.fields.contains_key(FACET_RUN_ID),
            "an absent run is not stamped as empty"
        );
        assert!(
            !p.fields.contains_key(FACET_JOB_ID),
            "an absent job is not stamped as empty"
        );
    }

    #[test]
    fn doc_ref_is_the_run_job_step_key() {
        assert_eq!(
            ci_log_doc_ref("acme", "run-7", "build", 3),
            "myelin://acme/ci/log/run-7:build:3"
        );
    }

    #[test]
    fn details_ref_builds_and_parses_the_x1_anchor() {
        let anchor = ci_log_details_ref("acme", "run-42", 3);
        assert_eq!(anchor, "myelin://acme/ci/run/run-42#step-3");
        let parsed = parse_step_anchor(&anchor).expect("parse");
        assert_eq!(
            parsed,
            CiLogStepAnchor {
                run_id: "run-42".into(),
                step_no: 3
            }
        );
        assert_eq!(parse_step_anchor("acme/ci/run/run-42#step-3"), Some(parsed));
    }

    #[test]
    fn step_anchor_parser_rejects_malformation() {
        assert_eq!(parse_step_anchor("myelin://acme/ci/run/run-1"), None);
        assert_eq!(parse_step_anchor("myelin://acme/ci/run/run-1#frag-1"), None);
        assert_eq!(parse_step_anchor("myelin://acme/ci/run/run-1#step-x"), None);
        assert_eq!(
            parse_step_anchor("myelin://acme/issue/issue/42#step-1"),
            None
        );
        assert_eq!(parse_step_anchor("myelin://acme/ci/run/#step-1"), None);
    }

    #[test]
    fn floor_marker_names_durable_not_firehose() {
        assert_eq!(
            CiLogDurableSegmentNotFirehoseFloor::DURABLE_TRANSPORT,
            "evt.* (durable bus)"
        );
        assert_eq!(
            CiLogDurableSegmentNotFirehoseFloor::NOT_THE_FIREHOSE,
            "firehose live tier"
        );
        assert_eq!(
            CiLogDurableSegmentNotFirehoseFloor::PER_SUBJECT_DEK_OWNER,
            "myelin_storage::ci_log_index::CiLogTier"
        );
    }
}
