use myelin_events::{
    Actor, AggregateKey, CorrelationId, DataRole, EventEnvelope, EventHandler, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_gdpr::{EraseScope, SubjectRef};
use myelin_gdpr_service::DsrKind;
use myelin_gdpr_service::{
    verify_inclusion, AuditAuthority, CellSigningKey, EDiscoveryBundle, EDiscoveryExporter,
    EDiscoveryScope, HoldVerdict, LegalHoldRegistry, SigningKey,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{ArtifactRef, TenantId};

fn action(id: &str, tenant: &str, subject: &str, correlation: &str) -> EventEnvelope {
    let principal = Principal::stub(
        PrincipalId(format!("u-{id}")),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    let region = principal.region.clone();
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("identity.tuple.written".into()),
        schema_ver: 1,
        tenant: TenantId(tenant.into()),
        region,
        actor: Actor(principal),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey("agg:1".into()),
        causation_id: None,
        correlation_id: CorrelationId(correlation.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        payload: serde_json::json!({}),
    }
}

fn auditor_verifies(bundle: &EDiscoveryBundle, published_key: &dyn SigningKey) -> bool {
    if !bundle.verify(published_key) {
        return false;
    }
    bundle
        .records
        .iter()
        .all(|r| verify_inclusion(&r.inclusion, &bundle.sth))
}

#[test]
fn provider_exports_a_scope_and_the_auditor_consumer_verifies_the_chain_of_custody() {
    let key = CellSigningKey::from_seed("cell:fr-par:audit-key");
    let authority = AuditAuthority::new(key);
    let holds = LegalHoldRegistry::new();

    authority.consumer().handle(
        &action("1", "acme", "myelin://acme/subj/A", "r-1"),
        &mut myelin_events::HandlerTx::none(),
    );
    authority.consumer().handle(
        &action("2", "acme", "myelin://acme/subj/B", "r-2"),
        &mut myelin_events::HandlerTx::none(),
    );
    authority.consumer().handle(
        &action("3", "acme", "myelin://acme/subj/A", "r-3"),
        &mut myelin_events::HandlerTx::none(),
    );
    authority.consumer().handle(
        &action("4", "acme", "myelin://acme/subj/A", "r-4"),
        &mut myelin_events::HandlerTx::none(),
    );
    authority.consumer().handle(
        &action("5", "acme", "myelin://acme/subj/B", "r-5"),
        &mut myelin_events::HandlerTx::none(),
    );

    let exporter = EDiscoveryExporter::new(&authority, &holds);
    let scope = EDiscoveryScope::Subject {
        tenant: TenantId("acme".into()),
        subject: ArtifactRef("myelin://acme/subj/A".into()),
    };
    let bundle = exporter
        .export(&scope, "2026-06-20T01:00:00Z")
        .expect("a non-empty export");

    assert!(
        auditor_verifies(&bundle, authority.key()),
        "the auditor verifies the export was not altered (content-addressed + inclusion-proof-bearing)"
    );
    assert_eq!(
        bundle.record_count(),
        3,
        "every subject-A record is in the production"
    );
}

#[test]
fn the_auditor_consumer_rejects_a_tampered_or_forged_bundle() {
    let key = CellSigningKey::from_seed("cell:fr-par:audit-key");
    let authority = AuditAuthority::new(key);
    let holds = LegalHoldRegistry::new();
    authority.consumer().handle(
        &action("1", "acme", "myelin://acme/subj/A", "r-1"),
        &mut myelin_events::HandlerTx::none(),
    );
    authority.consumer().handle(
        &action("2", "acme", "myelin://acme/subj/A", "r-2"),
        &mut myelin_events::HandlerTx::none(),
    );
    authority.consumer().handle(
        &action("3", "acme", "myelin://acme/subj/A", "r-3"),
        &mut myelin_events::HandlerTx::none(),
    );

    let exporter = EDiscoveryExporter::new(&authority, &holds);
    let bundle = exporter
        .export(
            &EDiscoveryScope::Tenant(TenantId("acme".into())),
            "2026-06-20T01:00:00Z",
        )
        .expect("a non-empty export");
    assert!(
        auditor_verifies(&bundle, authority.key()),
        "the honest production verifies"
    );

    let mut dropped = bundle.clone();
    dropped.records.pop();
    assert!(
        !auditor_verifies(&dropped, authority.key()),
        "the auditor rejects a dropped record"
    );

    let forged_key = CellSigningKey::from_seed("an-attackers-key");
    assert!(
        !auditor_verifies(&bundle, &forged_key),
        "the auditor rejects a forged STH"
    );
}

#[test]
fn the_export_is_legal_hold_frozen_so_a_concurrent_erase_is_deferred() {
    let key = CellSigningKey::from_seed("cell:fr-par:audit-key");
    let authority = AuditAuthority::new(key);
    let holds = LegalHoldRegistry::new();
    authority.consumer().handle(
        &action("1", "acme", "u-A", "r-1"),
        &mut myelin_events::HandlerTx::none(),
    );

    let principal = Principal::stub(
        PrincipalId("u-A".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    let erase_scope = EraseScope::Subject {
        subject: SubjectRef::new(principal),
        tenant: TenantId("acme".into()),
    };
    assert_eq!(
        holds.verdict(DsrKind::Erasure, &erase_scope),
        HoldVerdict::Proceed
    );

    let exporter = EDiscoveryExporter::new(&authority, &holds);
    let scope = EDiscoveryScope::Subject {
        tenant: TenantId("acme".into()),
        subject: ArtifactRef("u-A".into()),
    };
    let bundle = exporter.export(&scope, "t").expect("export");
    assert!(bundle.legal_hold_frozen, "the bundle records the freeze");

    assert_eq!(
        holds.verdict(DsrKind::Erasure, &erase_scope),
        HoldVerdict::Deferred,
        "the export froze the scope (legal-hold-frozen)"
    );
}
