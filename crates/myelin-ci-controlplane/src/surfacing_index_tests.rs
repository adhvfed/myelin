//! Unit tests for [`crate::surfacing_index`] — the CI `ci/run` `IndexSpec` (6.3) + the
//! replay-no-cross-db rebuild GATE (2.6) + the restriction-honouring index admission (§7.4).

use super::*;
use myelin_events::{
    reindex, snapshot_event_id, Actor, CorrelationId, DerivedStore, EmitContextBase, EventEnvelope,
    OutboxStore, Region, ReindexSource, SnapshotDraft, SnapshotScope, TenantId, Timestamp,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

// ════════════════════════════════════════════════════════════════════════════════════════════════
// declare_indexable — the ci/run IndexSpec (6.3, §7.4)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The spec is CI's owned 6.3 shape.** Pins every field of the frozen `IndexSpec` CI registers (a
/// rename of a Search field would break this — the registrant catches it).
#[test]
fn spec_is_cis_owned_6_3_run_shape() {
    let s = ci_run_index_spec();
    assert_eq!(s.subsystem, "ci", "CI owns the `ci` subsystem projection");
    assert_eq!(s.type_, "run", "the indexed artifact type is a run");
    assert_eq!(
        s.acl_object_type, "ci_run",
        "a run's reachability is decided by the ci_run ACL object (the §5.1 push-down anchor)"
    );
    assert!(
        s.semantic,
        "the failure_summary field is vector-embedded in v1 (RAG/dedup, §7.4)"
    );
    // The seven structured facets (the columnar filter shape).
    assert_eq!(
        s.struct_fields.len(),
        7,
        "exactly the seven structured run facets"
    );
    assert_eq!(s.struct_fields.get("state"), Some(&FieldType::Select));
    assert_eq!(s.struct_fields.get("trust_tier"), Some(&FieldType::Select));
    assert_eq!(s.struct_fields.get("env"), Some(&FieldType::Select));
    assert_eq!(
        s.struct_fields.get("actor_pseudonym"),
        Some(&FieldType::Principal)
    );
    assert_eq!(s.struct_fields.get("created_at"), Some(&FieldType::Date));
    assert_eq!(s.struct_fields.get("repo_ref"), Some(&FieldType::Relation));
    assert_eq!(s.struct_fields.get("commit_oid"), Some(&FieldType::Text));
}

/// **The `acl_object_type` is `ci_run` — the SAME anchor the firehose `ci_log` doc keys on (no ACL
/// drift across CI's two doc types).** Both a run doc and its log docs resolve through ONE
/// reachable-set filter (`list_objects(viewer, read, ci_run)`).
#[test]
fn run_and_log_docs_share_the_one_ci_run_acl_anchor() {
    let run = ci_run_index_spec();
    let log = myelin_search::ci_log_index_spec();
    assert_eq!(
        run.acl_object_type, log.acl_object_type,
        "the run doc and the log doc share ONE ci_run ACL anchor (no per-doc-type drift)"
    );
    assert_eq!(run.acl_object_type, "ci_run");
}

/// **The full-text projection body is NOT a structured facet.** `pipeline_name` / `branch` /
/// `trigger_kind` / `failed_test_name` / `log_excerpt_of_failure` / `failure_summary` arrive at emit
/// time in `SearchProjection.text`, so they must be absent from `struct_fields` (the schema is the
/// columnar half, not the body).
#[test]
fn fulltext_and_semantic_body_is_not_a_struct_facet() {
    let s = ci_run_index_spec();
    for absent in [
        "pipeline_name",
        "branch",
        "trigger_kind",
        "failed_test_name",
        "log_excerpt_of_failure",
        "failure_summary",
    ] {
        assert!(
            !s.struct_fields.contains_key(absent),
            "`{absent}` is full-text/semantic projection body, not a structured facet"
        );
    }
}

/// **The spec serializes to the 6.3 wire shape (0 schema mismatches — the build-time gate).** The
/// serialized JSON key set + values against the frozen contract-6.3 keys. A wire rename of any key
/// is caught here (this IS the CDC of CI's owned run spec half).
#[test]
fn spec_serializes_to_the_6_3_wire_shape() {
    let s = ci_run_index_spec();
    let json = serde_json::to_value(&s).expect("the spec serializes");
    let obj = json.as_object().expect("the spec is a JSON object");

    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "acl_object_type",
            "semantic",
            "struct_fields",
            "subsystem",
            "type"
        ],
        "the 6.3 wire key set"
    );
    assert_eq!(obj["subsystem"], serde_json::json!("ci"));
    assert_eq!(obj["type"], serde_json::json!("run"));
    assert_eq!(obj["semantic"], serde_json::json!(true));
    assert_eq!(obj["acl_object_type"], serde_json::json!("ci_run"));
}

/// **The registration is ACCEPTED by Search (the 6.3 GATE).** Search admits the spec into a live
/// indexer's per-tenant facet union without a schema mismatch — the returned accepted spec is
/// byte-equal to the declared one (registration neither mutates nor rejects the shape).
#[test]
fn registration_is_accepted_by_search() {
    let accepted = register_ci_run_index_spec();
    assert_eq!(
        accepted,
        ci_run_index_spec(),
        "Search accepts the declared run spec verbatim"
    );
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// declare_indexable honours restriction (§7.4 — the GATE: 0 restricted rows indexed)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **THE RESTRICTION GATE (§7.4): a restricted subject's runs are EXCLUDED from the index (0
/// restricted rows indexed).** A run whose subject is restricted (the GDPR `restrict` flag) OR whose
/// run is erased is NOT indexable — the index never carries a restricted/erased run, so a search
/// result/count/rank never leaks one. An unrestricted, present run IS indexable.
#[test]
fn declare_indexable_honours_restriction_zero_restricted_rows_indexed() {
    // The plain run is indexable.
    assert!(
        run_doc_is_indexable(false, false),
        "an unrestricted, present run is indexed"
    );
    // A RESTRICTED subject's run is EXCLUDED (0 restricted rows indexed).
    assert!(
        !run_doc_is_indexable(true, false),
        "a restricted subject's run is EXCLUDED from the index (§7.4)"
    );
    // An ERASED run is EXCLUDED (erasure-safe).
    assert!(
        !run_doc_is_indexable(false, true),
        "an erased run is EXCLUDED from the index"
    );
    // Restricted AND erased → still excluded.
    assert!(!run_doc_is_indexable(true, true));
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// replay is the only rebuild path (2.6 GATE) — rebuild WITHOUT reading CI's DB
// ════════════════════════════════════════════════════════════════════════════════════════════════

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("ci".into()),
            PrincipalKind::Service,
            TenantId("acme".into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-23T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T00:00:00Z".into()),
        caused_by: None,
    }
}

/// Build the `*.snapshot` envelope a relay would deliver for a draft (the consumer's input — the
/// SAME envelope shape as a live event, with the deterministic snapshot event_id).
fn snapshot_envelope(draft: &SnapshotDraft) -> EventEnvelope {
    EventEnvelope {
        event_id: draft.event_id(),
        type_: draft.type_.clone(),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("ci".into()),
            PrincipalKind::Service,
            TenantId("acme".into()),
        )),
        subject: draft.subject.clone(),
        aggregate: draft.aggregate.clone(),
        causation_id: None,
        correlation_id: CorrelationId(draft.event_id().0),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: draft.data_role,
        visibility: draft.visibility,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-23T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T00:00:00Z".into()),
        payload: draft.payload.clone(),
    }
}

fn ci_source_with_two_runs() -> CiReindexSource {
    let mut src = CiReindexSource::new();
    src.upsert(
        CiReplayKind::Run,
        "myelin://acme/ci/run/r1",
        1,
        "myelin://acme/ci/run/r1",
        serde_json::json!({ "overall": "success", "commit": "abc" }),
    );
    src.upsert(
        CiReplayKind::Run,
        "myelin://acme/ci/run/r2",
        3,
        "myelin://acme/ci/run/r2",
        serde_json::json!({ "overall": "failure", "commit": "def" }),
    );
    src
}

/// **THE 2.6 GATE: replay is the ONLY rebuild path — a `ci.run.snapshot` rebuilds the derived view
/// WITHOUT reading CI's DB (the no-cross-db property).** The derived store is rebuilt purely from
/// the `*.snapshot` drafts CI's replay re-emits through the outbox → the live consumer path; the
/// `Projector`/`ArtifactStore` (CI's OWN OLTP read) is NEVER touched in the rebuild. We assert the
/// cold rebuild is byte-identical to the live projection, AND that the rebuild input came solely
/// from the replay snapshots (the only legal recovery input).
#[test]
fn rebuild_from_snapshots_without_ci_db() {
    let src = ci_source_with_two_runs();
    let scope = SnapshotScope::new("ci", "run:all");

    // LIVE projection — ingest the drafts as the live `ci.run.succeeded`/`failed` events would have.
    let mut live = DerivedStore::new();
    for draft in src.replay(&scope, None) {
        live.ingest(&snapshot_envelope(&draft));
    }

    // COLD rebuild — wiped, rebuilt ONLY from the reindex snapshot replay through the outbox. NO
    // read of CI's OLTP / the Projector — the snapshot drafts are the sole input.
    let mut cold = DerivedStore::new();
    let sources: &[&dyn ReindexSource] = &[&src];
    let mut outbox = OutboxStore::new();
    let r1 = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex");
    assert_eq!(
        r1.snapshots_emitted, 2,
        "both runs re-emitted through the outbox"
    );
    for draft in src.replay(&scope, None) {
        let row = outbox
            .row(&draft.event_id())
            .expect("snapshot row present in the outbox");
        cold.ingest(&row.envelope);
    }
    assert_eq!(
        cold.parity_bytes(),
        live.parity_bytes(),
        "cold == live (rebuilt purely from *.snapshot, no CI-DB read)"
    );

    // Re-run — idempotent (0 new; the deterministic snapshot ids are already present).
    let r2 = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("re-reindex");
    assert_eq!(r2.snapshots_emitted, 0, "a re-run emits 0 new (idempotent)");
    assert_eq!(r2.snapshots_skipped_duplicate, 2);
}

/// **One-run granular replay (2.6 — sub-artifact-granular).** A `run:r1` scope re-emits exactly that
/// run's `ci.run.snapshot`, not a sibling's — the post-restore re-erasure / single-run reindex path.
#[test]
fn replay_is_one_run_granular() {
    let src = ci_source_with_two_runs();
    let drafts = src.replay(&SnapshotScope::new("ci", "run:r1"), None);
    assert_eq!(drafts.len(), 1, "exactly the one run");
    assert_eq!(drafts[0].aggregate.0, "myelin://acme/ci/run/r1");
    assert_eq!(drafts[0].type_.0, "ci.run.snapshot");
}

/// **An erased run is SKIPPED by replay (X-7 — the post-restore re-erasure path).** A tombstoned run
/// is not re-snapshotted; the erasure stays erased across a reindex (replay never resurrects a
/// shredded aggregate). This is the replay-side twin of the index restriction gate.
#[test]
fn replay_skips_an_erased_run() {
    let mut src = ci_source_with_two_runs();
    assert!(src.erase("myelin://acme/ci/run/r1"));
    let drafts = src.replay(&SnapshotScope::new("ci", "run:all"), None);
    assert_eq!(drafts.len(), 1, "the erased run is not re-snapshotted");
    assert_eq!(drafts[0].aggregate.0, "myelin://acme/ci/run/r2");
}

/// **The deterministic snapshot id is stable for a CI aggregate@version (re-run idempotency).**
#[test]
fn ci_snapshot_id_is_deterministic() {
    let drafts = ci_source_with_two_runs().replay(&SnapshotScope::new("ci", "run:all"), None);
    let a = &drafts[0];
    assert_eq!(
        snapshot_event_id(&a.aggregate, a.version),
        snapshot_event_id(&a.aggregate, a.version)
    );
    assert_ne!(
        snapshot_event_id(&a.aggregate, a.version),
        snapshot_event_id(&a.aggregate, a.version + 1)
    );
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// humanise (7.3) — confirmed re-export resolves through the ONE template registry
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The humanise re-export resolves: `ci_summary` builds a `(template_key, args)` HumanisedRef
/// (NEVER a raw string), and the key is one CI registered on the ONE humanise surface (7.3).** This
/// CONFIRMS the NOTIF-P23 / P-344 registration is the surfacing humanise half (no second template
/// set). The X-1 "never a raw string" invariant holds by construction (there is no raw-string
/// summary constructor).
#[test]
fn humanise_summary_is_a_registered_humanised_ref() {
    let s = ci_summary(CheckVerdict::Failure, "build");
    assert_eq!(s.template_key, "ci.check.failure");
    assert_eq!(s.args.get("context"), Some(&"build".to_string()));
    // the key is one of the registered template bodies (resolves through humanise, not the fallback).
    assert!(
        CI_SUMMARY_TEMPLATES
            .iter()
            .any(|(k, _, _)| *k == s.template_key),
        "the summary key resolves to a registered humanise template body"
    );
    // there is no raw-string summary field (the serialised shape is exactly {template_key, args}).
    let v = serde_json::to_value(&s).unwrap();
    assert!(
        v.get("text").is_none(),
        "no raw-string summary field exists"
    );
}

/// **Every check verdict's summary template key is registered (the humanise vocabulary is whole).**
#[test]
fn every_verdict_summary_key_is_registered() {
    for v in [
        CheckVerdict::Queued,
        CheckVerdict::InProgress,
        CheckVerdict::Success,
        CheckVerdict::Failure,
        CheckVerdict::Error,
        CheckVerdict::Neutral,
        CheckVerdict::Cancelled,
    ] {
        let key = summary_template_key(v);
        assert!(
            CI_SUMMARY_TEMPLATES.iter().any(|(k, _, _)| *k == key),
            "verdict {v:?} key `{key}` must have a registered humanise body"
        );
    }
}
