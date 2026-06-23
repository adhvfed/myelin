//! # The CDC pair for CI's OWNED cross-fabric surfacing rows 6.3 / 2.6 / 7.3 (CI-P26 → P-369, M4)
//!
//! **Contracts:** `planning/05-refined-shared-systems-architecture/contract-index.md`
//! - **6.3** `declare_indexable(IndexSpec)` — CI declares the `ci/run` projection shape; Search
//!   ADMITS it (the build-time registration into a live indexer's per-tenant facet union);
//! - **2.6** `replay(scope, since)` — CI re-emits `ci.run.snapshot` through the outbox → the live
//!   consumer; the derived view rebuilds WITHOUT reading CI's DB (cold == live, no-cross-db);
//! - **7.3** the `CheckStatus.summary` `HumanisedRef` — CI builds a `(template_key, args)` pair that
//!   resolves through Notif's ONE humanise surface (never a raw string).
//!
//! Owning architecture:
//! `planning/04-subsystem-architectures/continuous-integration/architecture/03-events-contracts-and-glue.md`
//! §7.3 (replay) + §7.4 (the IndexSpec). Reconciliation: `00-reconciliation-decisions.md` §X-1.
//!
//! ## What this CDC pins (PROVIDER ↔ CONSUMER no-drift)
//! - **6.3 PROVIDER** (CI): `ci_run_index_spec()` is CI's owned frozen `ci/run` spec (subsystem ci,
//!   type run, acl_object_type ci_run, the seven struct facets, semantic). **CONSUMER** (Search):
//!   `register_ci_run_index_spec()` ADMITS it into a live `IncrementalIndexer` (Search is the
//!   authority that admits; CI does not assert acceptance).
//! - **2.6 PROVIDER** (CI): the `*.snapshot` replay re-emit through the outbox. **CONSUMER** (a
//!   derived store): rebuilds from the snapshots alone, byte-identical to live (no CI-DB read).
//! - **7.3 PROVIDER** (CI): the summary `(template_key, args)` from `ci_summary`. **CONSUMER**
//!   (Notif's humanise template store): the key resolves to a registered ICU body, per-viewer-safe.

use myelin_ci_controlplane::{
    ci_run_index_spec, ci_summary, register_ci_run_index_spec, register_ci_summary_templates,
    summary_template_key, CheckVerdict, CiReindexSource, CiReplayKind, CI_SUMMARY_TEMPLATES,
};
use myelin_events::{
    reindex, Actor, CorrelationId, DerivedStore, EmitContextBase, EventEnvelope, OutboxStore,
    Region, ReindexSource, SnapshotDraft, SnapshotScope, TenantId, Timestamp,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_query::FieldType;

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 6.3 — declare_indexable: CI declares the ci/run spec ↔ Search admits it
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **6.3 PROVIDER: CI's owned `ci/run` spec is the frozen shape.** A rename of a Search-owned
/// `IndexSpec` field breaks this — the registrant catches it.
#[test]
fn cdc_6_3_provider_ci_run_spec_is_the_frozen_shape() {
    let s = ci_run_index_spec();
    assert_eq!(s.subsystem, "ci");
    assert_eq!(s.type_, "run");
    assert_eq!(s.acl_object_type, "ci_run");
    assert!(s.semantic, "the failure_summary semantic field (§7.4)");
    assert_eq!(s.struct_fields.get("state"), Some(&FieldType::Select));
    assert_eq!(s.struct_fields.get("commit_oid"), Some(&FieldType::Text));
    assert_eq!(s.struct_fields.get("repo_ref"), Some(&FieldType::Relation));
}

/// **6.3 CONSUMER: Search ADMITS CI's run spec verbatim (the build-time registration GATE).** The
/// accepted spec is byte-equal to the declared one — Search neither mutates nor rejects the shape.
#[test]
fn cdc_6_3_consumer_search_admits_the_ci_run_spec() {
    let accepted = register_ci_run_index_spec();
    assert_eq!(
        accepted,
        ci_run_index_spec(),
        "Search accepts CI's declared run spec verbatim (0 schema mismatch)"
    );
}

/// **6.3 wire shape: the spec serializes to the frozen contract-6.3 key set.** A wire rename of any
/// key is caught here.
#[test]
fn cdc_6_3_spec_serializes_to_the_wire_shape() {
    let json = serde_json::to_value(ci_run_index_spec()).unwrap();
    let obj = json.as_object().unwrap();
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
        ]
    );
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 2.6 — replay: cold == live, rebuilt WITHOUT reading CI's DB
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

fn ci_source() -> CiReindexSource {
    let mut src = CiReindexSource::new();
    src.upsert(
        CiReplayKind::Run,
        "myelin://acme/ci/run/r1",
        1,
        "myelin://acme/ci/run/r1",
        serde_json::json!({ "overall": "success" }),
    );
    src.upsert(
        CiReplayKind::Run,
        "myelin://acme/ci/run/r2",
        3,
        "myelin://acme/ci/run/r2",
        serde_json::json!({ "overall": "failure" }),
    );
    src
}

/// **2.6 PROVIDER+CONSUMER: replay is the ONLY rebuild path — cold == live with NO CI-DB read.** The
/// derived view rebuilds purely from the `ci.run.snapshot` drafts CI's replay re-emits through the
/// outbox; the rebuild never reads CI's OLTP / the projector. Re-run is idempotent (deterministic id).
#[test]
fn cdc_2_6_replay_rebuilds_without_ci_db_cold_equals_live() {
    let src = ci_source();
    let scope = SnapshotScope::new("ci", "run:all");

    let mut live = DerivedStore::new();
    for d in src.replay(&scope, None) {
        live.ingest(&snapshot_envelope(&d));
    }

    let mut cold = DerivedStore::new();
    let sources: &[&dyn ReindexSource] = &[&src];
    let mut outbox = OutboxStore::new();
    let r1 = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex");
    assert_eq!(r1.snapshots_emitted, 2);
    for d in src.replay(&scope, None) {
        cold.ingest(&outbox.row(&d.event_id()).unwrap().envelope);
    }
    assert_eq!(
        cold.parity_bytes(),
        live.parity_bytes(),
        "cold == live (rebuilt purely from *.snapshot, no CI-DB read)"
    );

    let r2 = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("re-reindex");
    assert_eq!(r2.snapshots_emitted, 0, "idempotent re-run");
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 7.3 — the CheckStatus.summary HumanisedRef resolves through humanise
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **7.3 PROVIDER: CI builds the summary as a `(template_key, args)` HumanisedRef — never a raw
/// string.** The serialised shape is exactly `{template_key, args}` (no `text` field).
#[test]
fn cdc_7_3_provider_summary_is_a_humanised_ref() {
    let s = ci_summary(CheckVerdict::Failure, "build");
    assert_eq!(s.template_key, "ci.check.failure");
    let v = serde_json::to_value(&s).unwrap();
    assert_eq!(v["template_key"], "ci.check.failure");
    assert_eq!(v["args"]["context"], "build");
    assert!(v.get("text").is_none(), "no raw-string summary field");
}

/// **7.3 CONSUMER: Notif's humanise template store ADMITS CI's summary templates; every summary key
/// resolves to a registered ICU body (not the generic fallback).** The `{0}` subject slot is present
/// so the per-viewer resolve binds it (NOTIF-D4 — a private-repo check humanises to a tombstone).
#[test]
fn cdc_7_3_consumer_humanise_resolves_every_summary_key() {
    use myelin_notif::{TemplateStore, DEFAULT_LOCALE, PLATFORM_DEFAULT_TENANT};
    let mut store = TemplateStore::with_platform_defaults();
    register_ci_summary_templates(&mut store);

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
        let t = store
            .lookup(PLATFORM_DEFAULT_TENANT, key, DEFAULT_LOCALE)
            .unwrap_or_else(|| panic!("CI summary key `{key}` resolves through humanise"));
        assert!(
            t.body.contains("{0}"),
            "the per-viewer subject slot is present"
        );
    }
    // the registered bodies are exactly CI's owned vocabulary.
    assert_eq!(
        CI_SUMMARY_TEMPLATES.len(),
        7,
        "the seven check-verdict bodies"
    );
}
