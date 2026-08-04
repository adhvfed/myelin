use myelin_tenancy::{ArtifactRef, TenantId};

use crate::audit_proofs::{
    serialize_sth_commitment, AuditAuthority, InclusionProof, SignedTreeHead, SigningKey,
};
use crate::dsr::{DsrId, MerkleProvenBundle};
use crate::fanout::{HoldScope, LegalHoldRegistry};

pub const EDISCOVERY_EXPORT_RECORDS: (&str, &str) = ("gdpr.ediscovery_export_records", "count");

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EDiscoveryScope {
    Subject {
        tenant: TenantId,
        subject: ArtifactRef,
    },
    Tenant(TenantId),
    Matter {
        tenant: TenantId,
        matter_token: String,
    },
}

impl EDiscoveryScope {
    pub fn tenant(&self) -> &TenantId {
        match self {
            EDiscoveryScope::Subject { tenant, .. } => tenant,
            EDiscoveryScope::Tenant(tenant) => tenant,
            EDiscoveryScope::Matter { tenant, .. } => tenant,
        }
    }

    fn token(&self) -> String {
        match self {
            EDiscoveryScope::Subject { tenant, subject } => {
                format!("subject:{}:{}", tenant.0, subject.0)
            }
            EDiscoveryScope::Tenant(tenant) => format!("tenant:{}", tenant.0),
            EDiscoveryScope::Matter {
                tenant,
                matter_token,
            } => {
                format!("matter:{}:{}", tenant.0, matter_token)
            }
        }
    }

    fn hold_scope(&self) -> HoldScope {
        match self {
            EDiscoveryScope::Subject { tenant, subject } => HoldScope::Subject {
                tenant: tenant.0.clone(),
                subject: subject.0.clone(),
            },
            EDiscoveryScope::Tenant(tenant) => HoldScope::Tenant(tenant.0.clone()),
            EDiscoveryScope::Matter { tenant, .. } => HoldScope::Tenant(tenant.0.clone()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EDiscoveryRecord {
    pub seq: u64,
    pub action: String,
    pub subject: ArtifactRef,
    pub actor_token: String,
    pub occurred_at: String,
    pub inclusion: InclusionProof,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EDiscoveryBundle {
    pub scope_token: String,
    pub sth: SignedTreeHead,
    pub records: Vec<EDiscoveryRecord>,
    pub legal_hold_frozen: bool,
    pub bundle_digest: String,
}

impl EDiscoveryBundle {
    fn content_addressed(
        scope_token: &str,
        sth: &SignedTreeHead,
        records: &[EDiscoveryRecord],
    ) -> String {
        let mut body = format!(
            "scope={scope_token}\u{1f}sth={}",
            serialize_sth_commitment(sth)
        );
        for r in records {
            body.push('\u{1f}');
            body.push_str(&format!("rec={}:{}", r.seq, r.inclusion.leaf_hash));
        }
        let digest = blake3::hash(body.as_bytes());
        format!("blake3:{}", hex::encode(digest.as_bytes()))
    }

    pub fn verify(&self, key: &dyn SigningKey) -> bool {
        if EDiscoveryBundle::content_addressed(&self.scope_token, &self.sth, &self.records)
            != self.bundle_digest
        {
            return false;
        }
        if !self.sth.verify_signature(key) {
            return false;
        }
        self.records
            .iter()
            .all(|r| crate::audit_proofs::verify_inclusion(&r.inclusion, &self.sth))
    }

    pub fn record_count(&self) -> usize {
        self.records.len()
    }
}

pub struct EDiscoveryExporter<'a, K: SigningKey> {
    authority: &'a AuditAuthority<K>,
    holds: &'a LegalHoldRegistry,
}

impl<'a, K: SigningKey> EDiscoveryExporter<'a, K> {
    pub fn new(
        authority: &'a AuditAuthority<K>,
        holds: &'a LegalHoldRegistry,
    ) -> EDiscoveryExporter<'a, K> {
        EDiscoveryExporter { authority, holds }
    }

    pub fn export(&self, scope: &EDiscoveryScope, exported_at: &str) -> Option<EDiscoveryBundle> {
        let tenant = scope.tenant().clone();
        self.holds.set(scope.hold_scope(), true);

        let sth = self.authority.signed_tree_head(&tenant, exported_at)?;
        let entries = self.authority.consumer().log().entries_for(&tenant);
        let mut records = Vec::new();
        for entry in &entries {
            if !self.in_scope(scope, &entry.subject, &entry.correlation_id) {
                continue;
            }
            let Some(inclusion) = self.authority.inclusion_proof(&tenant, entry.seq) else {
                continue;
            };
            records.push(EDiscoveryRecord {
                seq: entry.seq,
                action: entry.action.clone(),
                subject: entry.subject.clone(),
                actor_token: entry.actor.actor.clone(),
                occurred_at: entry.occurred_at.clone(),
                inclusion,
            });
        }

        let bundle_digest = EDiscoveryBundle::content_addressed(&scope.token(), &sth, &records);
        Some(EDiscoveryBundle {
            scope_token: scope.token(),
            sth,
            records,
            legal_hold_frozen: true,
            bundle_digest,
        })
    }

    pub fn export_bundle(
        &self,
        scope: &EDiscoveryScope,
        exported_at: &str,
    ) -> Option<MerkleProvenBundle> {
        let bundle = self.export(scope, exported_at)?;
        Some(MerkleProvenBundle {
            dsr_id: DsrId(format!("ediscovery:{}", bundle.bundle_digest)),
            receipts: bundle.records.iter().map(serialize_record_proof).collect(),
            bundle_digest: bundle.bundle_digest.clone(),
            merkle_inclusion: Some(serialize_sth_commitment(&bundle.sth)),
        })
    }

    fn in_scope(
        &self,
        scope: &EDiscoveryScope,
        subject: &ArtifactRef,
        correlation_id: &str,
    ) -> bool {
        match scope {
            EDiscoveryScope::Subject {
                subject: target, ..
            } => subject == target,
            EDiscoveryScope::Tenant(_) => true,
            EDiscoveryScope::Matter { matter_token, .. } => correlation_id == matter_token,
        }
    }
}

fn serialize_record_proof(r: &EDiscoveryRecord) -> String {
    format!(
        "{}@{}:{}->{}",
        r.inclusion.leaf_index, r.inclusion.tree_size, r.inclusion.leaf_hash, r.inclusion.root_hash
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit_proofs::CellSigningKey;
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, DataRole, EventEnvelope, EventHandler, EventId,
        EventType, Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::Region;

    fn key() -> CellSigningKey {
        CellSigningKey::from_seed("cell:fr-par:audit-key")
    }

    fn authority() -> AuditAuthority<CellSigningKey> {
        AuditAuthority::new(key())
    }

    fn action_event(id: &str, tenant: &str, subject: &str, correlation: &str) -> EventEnvelope {
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

    fn seed(auth: &AuditAuthority<CellSigningKey>) {
        auth.consumer()
            .handle(&action_event("1", "acme", "myelin://acme/subj/A", "r-1"), &mut myelin_events::HandlerTx::none());
        auth.consumer()
            .handle(&action_event("2", "acme", "myelin://acme/subj/B", "r-2"), &mut myelin_events::HandlerTx::none());
        auth.consumer()
            .handle(&action_event("3", "acme", "myelin://acme/subj/A", "r-3"), &mut myelin_events::HandlerTx::none());
        auth.consumer()
            .handle(&action_event("4", "acme", "myelin://acme/subj/A", "m-7"), &mut myelin_events::HandlerTx::none());
        auth.consumer()
            .handle(&action_event("5", "acme", "myelin://acme/subj/B", "m-7"), &mut myelin_events::HandlerTx::none());
    }

    #[test]
    fn a_subject_scoped_export_is_inclusion_proof_bearing_and_verifies() {
        let auth = authority();
        let holds = LegalHoldRegistry::new();
        seed(&auth);
        let exporter = EDiscoveryExporter::new(&auth, &holds);

        let scope = EDiscoveryScope::Subject {
            tenant: TenantId("acme".into()),
            subject: ArtifactRef("myelin://acme/subj/A".into()),
        };
        let bundle = exporter
            .export(&scope, "2026-06-20T01:00:00Z")
            .expect("a non-empty export");
        assert_eq!(
            bundle.record_count(),
            3,
            "every subject-A record is in the bundle"
        );
        assert!(
            bundle.verify(auth.key()),
            "the bundle is verifiable, not asserted"
        );
        for r in &bundle.records {
            assert!(
                crate::audit_proofs::verify_inclusion(&r.inclusion, &bundle.sth),
                "record {} carries a proof that verifies against the bundle STH",
                r.seq
            );
        }
    }

    #[test]
    fn a_matter_scoped_export_selects_by_correlation_across_subjects() {
        let auth = authority();
        let holds = LegalHoldRegistry::new();
        seed(&auth);
        let exporter = EDiscoveryExporter::new(&auth, &holds);

        let scope = EDiscoveryScope::Matter {
            tenant: TenantId("acme".into()),
            matter_token: "m-7".into(),
        };
        let bundle = exporter
            .export(&scope, "t")
            .expect("a non-empty matter export");
        assert_eq!(
            bundle.record_count(),
            2,
            "both m-7 records (across subjects) are in scope"
        );
        assert!(bundle.verify(auth.key()));
        let subjects: std::collections::BTreeSet<_> =
            bundle.records.iter().map(|r| r.subject.0.clone()).collect();
        assert_eq!(
            subjects.len(),
            2,
            "the matter spans subject-A and subject-B"
        );
    }

    #[test]
    fn a_tenant_scoped_export_carries_the_whole_chain() {
        let auth = authority();
        let holds = LegalHoldRegistry::new();
        seed(&auth);
        let exporter = EDiscoveryExporter::new(&auth, &holds);

        let bundle = exporter
            .export(&EDiscoveryScope::Tenant(TenantId("acme".into())), "t")
            .expect("a non-empty tenant export");
        assert_eq!(
            bundle.record_count(),
            5,
            "every tenant record is in the bundle"
        );
        assert!(bundle.verify(auth.key()));
    }

    #[test]
    fn the_export_freezes_the_scope_with_a_legal_hold() {
        use crate::dsr::DsrKind;
        use crate::fanout::HoldVerdict;
        use myelin_gdpr::{EraseScope, SubjectRef};

        let auth = authority();
        let holds = LegalHoldRegistry::new();
        seed(&auth);
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
            HoldVerdict::Proceed,
            "before the export the scope is not frozen"
        );

        let exporter = EDiscoveryExporter::new(&auth, &holds);
        let scope = EDiscoveryScope::Subject {
            tenant: TenantId("acme".into()),
            subject: ArtifactRef("u-A".into()),
        };
        let bundle = exporter.export(&scope, "t").expect("export");
        assert!(bundle.legal_hold_frozen, "the bundle records the freeze");

        assert_eq!(
            holds.verdict(DsrKind::Erasure, &erase_scope),
            HoldVerdict::Deferred,
            "the export froze the scope (legal-hold-frozen - §5.4)"
        );
        assert_eq!(holds.active_count(), 1, "exactly one hold was placed");
    }

    #[test]
    fn the_bundle_digest_binds_the_exact_record_set() {
        let auth = authority();
        let holds = LegalHoldRegistry::new();
        seed(&auth);
        let exporter = EDiscoveryExporter::new(&auth, &holds);
        let bundle = exporter
            .export(&EDiscoveryScope::Tenant(TenantId("acme".into())), "t")
            .unwrap();
        assert!(bundle.verify(auth.key()));

        let mut dropped = bundle.clone();
        dropped.records.pop();
        assert!(
            !dropped.verify(auth.key()),
            "a dropped record fails verification"
        );

        let mut reordered = bundle.clone();
        reordered.records.swap(0, 1);
        assert!(
            !reordered.verify(auth.key()),
            "a reordered record set fails verification"
        );

        let other = CellSigningKey::from_seed("a-different-cell-key");
        assert!(
            !bundle.verify(&other),
            "a bundle does not verify under a different key"
        );
    }

    #[test]
    fn the_export_bundle_is_the_contract_10_7_merkleproven_shape() {
        let auth = authority();
        let holds = LegalHoldRegistry::new();
        seed(&auth);
        let exporter = EDiscoveryExporter::new(&auth, &holds);
        let scope = EDiscoveryScope::Subject {
            tenant: TenantId("acme".into()),
            subject: ArtifactRef("myelin://acme/subj/A".into()),
        };
        let mpb = exporter
            .export_bundle(&scope, "t")
            .expect("a MerkleProvenBundle");
        assert!(
            mpb.bundle_digest.starts_with("blake3:"),
            "content-addressed"
        );
        assert!(
            mpb.merkle_inclusion.is_some(),
            "the export IS Merkle-proven - the chain-of-custody STH commitment is carried (not None)"
        );
        assert_eq!(
            mpb.receipts.len(),
            3,
            "one per-record proof per subject-A record"
        );
        assert!(
            mpb.dsr_id.0.starts_with("ediscovery:"),
            "a PII-free matter handle"
        );
    }

    #[test]
    fn an_empty_scope_yields_no_export() {
        let auth = authority();
        let holds = LegalHoldRegistry::new();
        let exporter = EDiscoveryExporter::new(&auth, &holds);
        assert!(
            exporter
                .export(&EDiscoveryScope::Tenant(TenantId("empty".into())), "t")
                .is_none(),
            "an empty chain → no proof-bearing export"
        );
    }

    #[test]
    fn a_subject_with_no_records_returns_an_empty_proven_bundle() {
        let auth = authority();
        let holds = LegalHoldRegistry::new();
        seed(&auth);
        let exporter = EDiscoveryExporter::new(&auth, &holds);
        let scope = EDiscoveryScope::Subject {
            tenant: TenantId("acme".into()),
            subject: ArtifactRef("myelin://acme/subj/NOBODY".into()),
        };
        let bundle = exporter
            .export(&scope, "t")
            .expect("the tenant has a chain → a bundle");
        assert_eq!(
            bundle.record_count(),
            0,
            "no records about an unknown subject"
        );
        assert!(
            bundle.verify(auth.key()),
            "an empty bundle still verifies (the digest binds 0 records)"
        );
    }

    #[test]
    fn ediscovery_export_records_signal_is_named() {
        assert_eq!(
            EDISCOVERY_EXPORT_RECORDS.0,
            "gdpr.ediscovery_export_records"
        );
        assert_eq!(EDISCOVERY_EXPORT_RECORDS.1, "count");
    }

    #[test]
    fn the_scope_token_is_the_exact_pii_free_form() {
        let subj = EDiscoveryScope::Subject {
            tenant: TenantId("acme".into()),
            subject: ArtifactRef("u-A".into()),
        };
        assert_eq!(subj.token(), "subject:acme:u-A", "subject scope token");
        assert_eq!(
            EDiscoveryScope::Tenant(TenantId("acme".into())).token(),
            "tenant:acme",
            "tenant scope token"
        );
        assert_eq!(
            EDiscoveryScope::Matter {
                tenant: TenantId("acme".into()),
                matter_token: "m-7".into()
            }
            .token(),
            "matter:acme:m-7",
            "matter scope token"
        );
        let auth = authority();
        let holds = LegalHoldRegistry::new();
        seed(&auth);
        let exporter = EDiscoveryExporter::new(&auth, &holds);
        let s = exporter.export(&subj, "t").unwrap();
        assert_eq!(
            s.scope_token, "subject:acme:u-A",
            "the bundle carries the token verbatim"
        );
        let t = exporter
            .export(&EDiscoveryScope::Tenant(TenantId("acme".into())), "t")
            .unwrap();
        assert_ne!(
            s.scope_token, t.scope_token,
            "distinct scopes → distinct tokens"
        );
        assert_ne!(
            s.bundle_digest, t.bundle_digest,
            "distinct scopes → distinct bundle digests"
        );
    }

    #[test]
    fn the_record_proof_serialises_to_the_exact_chain_of_custody_form() {
        let auth = authority();
        let holds = LegalHoldRegistry::new();
        seed(&auth);
        let exporter = EDiscoveryExporter::new(&auth, &holds);
        let scope = EDiscoveryScope::Subject {
            tenant: TenantId("acme".into()),
            subject: ArtifactRef("myelin://acme/subj/A".into()),
        };
        let mpb = exporter.export_bundle(&scope, "t").unwrap();
        for proof in &mpb.receipts {
            assert!(!proof.is_empty(), "the proof string is never empty");
            assert!(proof.contains('@'), "carries the leaf@size");
            assert!(
                proof.contains("->blake3:"),
                "reduces to a blake3 root: {proof}"
            );
            assert!(proof.contains("blake3:"), "carries blake3 leaf/root nodes");
        }
        assert!(
            mpb.receipts[0].starts_with("0@"),
            "the first subject-A record is leaf 0"
        );
    }

    #[test]
    fn the_records_are_tenant_partitioned() {
        let auth = authority();
        let holds = LegalHoldRegistry::new();
        seed(&auth);
        let exporter = EDiscoveryExporter::new(&auth, &holds);
        let bundle = exporter
            .export(&EDiscoveryScope::Tenant(TenantId("acme".into())), "t")
            .unwrap();
        assert_eq!(
            bundle.sth.tenant,
            TenantId("acme".into()),
            "the STH is the per-tenant root"
        );
        let _region = Region("acme-home".into());
        assert!(exporter
            .export(&EDiscoveryScope::Tenant(TenantId("other".into())), "t")
            .is_none());
    }
}
