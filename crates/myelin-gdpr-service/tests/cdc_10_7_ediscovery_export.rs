//! # CDC 10.7 — eDiscovery / legal-hold export (P-GA-26 → P-153)
//!
//! **Contract:** index row 10.7 — `ediscovery_export(scope) → MerkleProvenBundle`; content-
//! addressed, inclusion-proof-bearing, legal-hold-frozen (gdpr §5.4). The same tamper-evident
//! substrate (the per-tenant audit Merkle tree, contract 10.6) that proves "we erased it" (the DSR
//! receipt) here proves "this is the unaltered record".
//!
//! The contract-coverage scanner (P-S21) reads BOTH halves of the pair from this file:
//! - **provider** = `myelin_gdpr_service::EDiscoveryExporter` — the GDPR/Audit service's eDiscovery
//!   authority. It assembles a subject/tenant/matter-scoped bundle over the per-tenant audit log,
//!   attaches each record's inclusion proof against the bundle's signed tree head, freezes the scope
//!   with a legal hold, and content-addresses the whole bundle.
//! - **consumer** = a LEGAL / AUDITOR recipient — a downstream party (a regulator, opposing counsel,
//!   a customer's legal team) that, holding ONLY the returned bundle + the published STH signing key,
//!   VERIFIES the production was not altered: it re-derives the bundle content-address, checks the
//!   STH signature, and verifies EVERY record's inclusion proof. This is the chain-of-custody an
//!   eDiscovery production requires (§5.4 — "a recipient can *verify* the bundle was not altered").
//!
//! The dated green artifact: the provider exports a scope; the consumer (the auditor) verifies the
//! whole bundle with only PII-free material — and a tampered/forged bundle is REJECTED. The export
//! is legal-hold-frozen (a concurrent erase over the scope is deferred while the bundle is in flight).

use myelin_events::{
    Actor, AggregateKey, CorrelationId, DataRole, EventEnvelope, EventHandler, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_gdpr::{EraseScope, SubjectRef};
use myelin_gdpr_service::{
    verify_inclusion, AuditAuthority, CellSigningKey, EDiscoveryBundle, EDiscoveryExporter,
    EDiscoveryScope, HoldVerdict, LegalHoldRegistry, SigningKey,
};
use myelin_gdpr_service::DsrKind;
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
        type_: EventType("iam.tuple_written".into()),
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

/// The CONSUMER (the legal/auditor recipient): given the bundle + the published STH signing key,
/// verify the whole production was not altered — re-derive the content-address, check the STH
/// signature, verify EVERY record's inclusion proof. PII-free material only.
fn auditor_verifies(bundle: &EDiscoveryBundle, published_key: &dyn SigningKey) -> bool {
    // The bundle's own end-to-end verify (digest + STH + every proof).
    if !bundle.verify(published_key) {
        return false;
    }
    // And independently: the auditor re-runs each record's inclusion proof against the bundle STH.
    bundle
        .records
        .iter()
        .all(|r| verify_inclusion(&r.inclusion, &bundle.sth))
}

#[test]
fn provider_exports_a_scope_and_the_auditor_consumer_verifies_the_chain_of_custody() {
    // PROVIDER: the GDPR/Audit service builds the audit substrate + the exporter.
    let key = CellSigningKey::from_seed("cell:fr-par:audit-key");
    let authority = AuditAuthority::new(key);
    let holds = LegalHoldRegistry::new();

    // Seed the per-tenant audit log: 3 actions about subject-A, 2 about subject-B.
    authority.consumer().handle(&action("1", "acme", "myelin://acme/subj/A", "r-1"));
    authority.consumer().handle(&action("2", "acme", "myelin://acme/subj/B", "r-2"));
    authority.consumer().handle(&action("3", "acme", "myelin://acme/subj/A", "r-3"));
    authority.consumer().handle(&action("4", "acme", "myelin://acme/subj/A", "r-4"));
    authority.consumer().handle(&action("5", "acme", "myelin://acme/subj/B", "r-5"));

    let exporter = EDiscoveryExporter::new(&authority, &holds);
    let scope = EDiscoveryScope::Subject {
        tenant: TenantId("acme".into()),
        subject: ArtifactRef("myelin://acme/subj/A".into()),
    };
    let bundle = exporter.export(&scope, "2026-06-20T01:00:00Z").expect("a non-empty export");

    // CONSUMER (the legal/auditor): verifies the chain of custody with only the bundle + the key.
    assert!(
        auditor_verifies(&bundle, authority.key()),
        "the auditor verifies the export was not altered (content-addressed + inclusion-proof-bearing)"
    );
    assert_eq!(bundle.record_count(), 3, "every subject-A record is in the production");
}

#[test]
fn the_auditor_consumer_rejects_a_tampered_or_forged_bundle() {
    let key = CellSigningKey::from_seed("cell:fr-par:audit-key");
    let authority = AuditAuthority::new(key);
    let holds = LegalHoldRegistry::new();
    authority.consumer().handle(&action("1", "acme", "myelin://acme/subj/A", "r-1"));
    authority.consumer().handle(&action("2", "acme", "myelin://acme/subj/A", "r-2"));
    authority.consumer().handle(&action("3", "acme", "myelin://acme/subj/A", "r-3"));

    let exporter = EDiscoveryExporter::new(&authority, &holds);
    let bundle = exporter
        .export(
            &EDiscoveryScope::Tenant(TenantId("acme".into())),
            "2026-06-20T01:00:00Z",
        )
        .expect("a non-empty export");
    assert!(auditor_verifies(&bundle, authority.key()), "the honest production verifies");

    // A DROPPED record (the producer tried to omit one) → the content-address no longer matches.
    let mut dropped = bundle.clone();
    dropped.records.pop();
    assert!(!auditor_verifies(&dropped, authority.key()), "the auditor rejects a dropped record");

    // A FORGED STH (signed by a different cell key) → the signature check fails.
    let forged_key = CellSigningKey::from_seed("an-attackers-key");
    assert!(!auditor_verifies(&bundle, &forged_key), "the auditor rejects a forged STH");
}

#[test]
fn the_export_is_legal_hold_frozen_so_a_concurrent_erase_is_deferred() {
    // PROVIDER assembles the export; the legal-hold freeze means the CONSUMER's production cannot be
    // shredded out from under it (gdpr §5.4 — legal-hold-frozen).
    let key = CellSigningKey::from_seed("cell:fr-par:audit-key");
    let authority = AuditAuthority::new(key);
    let holds = LegalHoldRegistry::new();
    authority.consumer().handle(&action("1", "acme", "u-A", "r-1"));

    let principal = Principal::stub(
        PrincipalId("u-A".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    let erase_scope = EraseScope::Subject {
        subject: SubjectRef::new(principal),
        tenant: TenantId("acme".into()),
    };
    // Before the export, a DSR erase over the subject would PROCEED.
    assert_eq!(holds.verdict(DsrKind::Erasure, &erase_scope), HoldVerdict::Proceed);

    let exporter = EDiscoveryExporter::new(&authority, &holds);
    let scope = EDiscoveryScope::Subject {
        tenant: TenantId("acme".into()),
        subject: ArtifactRef("u-A".into()),
    };
    let bundle = exporter.export(&scope, "t").expect("export");
    assert!(bundle.legal_hold_frozen, "the bundle records the freeze");

    // After the export, the SAME hold gate the DSR fan-out passes through DEFERS the erase — the
    // production is frozen while a legal matter is open.
    assert_eq!(
        holds.verdict(DsrKind::Erasure, &erase_scope),
        HoldVerdict::Deferred,
        "the export froze the scope (legal-hold-frozen)"
    );
}
