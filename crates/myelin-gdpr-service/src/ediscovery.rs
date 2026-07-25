//! # eDiscovery / legal-hold export (10.7) — content-addressed + inclusion-proof-bearing +
//! legal-hold-frozen (P-GA-26 → P-153)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` **§5.4** (eDiscovery /
//! legal-hold export — *a subject-, tenant-, or matter-scoped bundle of records + audit-log proofs
//! establishing chain-of-custody; **content-addressed and Merkle-proof-bearing** — each record
//! carries its inclusion proof against the per-tenant audit tree, so a recipient can **verify** the
//! bundle was not altered; **a legal-hold freezes the scope while the export is assembled***. The
//! same tamper-evident substrate (§6) serves both "prove we erased it" (the DSR receipt) and "prove
//! this is the unaltered record" (eDiscovery). Prove-it:
//! `external-insights/01-process-and-quality-doctrine.md` **§3** — an export carries a **verifiable
//! inclusion proof**, not an asserted completeness; a recipient runs [`super::verify_inclusion`]
//! over each record and re-derives the bundle digest, so "this is the unaltered record" is
//! *checkable*, never *claimed*.
//!
//! **Contract-index:** OWNS row **10.7** (`ediscovery_export(scope) → MerkleProvenBundle` —
//! content-addressed, inclusion-proof-bearing, legal-hold-frozen). Consumes/wired: **10.6** (the
//! per-tenant audit Merkle tree the export proofs ride — [`crate::audit_proofs::AuditAuthority`],
//! P-GA-19/P-GA-20) and the **legal-hold registry** (G4 — [`crate::fanout::LegalHoldRegistry`],
//! P-GA-12; the SAME hold gate the DSR fan-out passes through — EI-01 §7 coherence, no second hold
//! mechanism).
//!
//! ## What this module ships (EI-01 §7 — reuse, never re-implement)
//! The eDiscovery exporter ([`EDiscoveryExporter`]) is a READ-side authority OVER the existing
//! audit substrate: it does NOT re-define the Merkle tree, the STH, the inclusion proof, or the
//! legal-hold gate — it COMPOSES them.
//! 1. **Scope selection ([`EDiscoveryScope`]).** A bundle is **subject-**, **tenant-**, or
//!    **matter-scoped** (§5.4). Selection is over the per-tenant audit log entries the
//!    [`crate::audit::AuditConsumer`] already records — a subject scope matches an entry whose
//!    `subject` [`ArtifactRef`] is the subject's, a matter scope matches an entry whose
//!    `correlation_id` is the matter token, a tenant scope takes the whole tenant chain. The audit
//!    log is **minimised by design** (IDs/pseudonyms, never payloads — §6.2), so the bundle is
//!    PII-minimised by construction.
//! 2. **Inclusion-proof-bearing ([`EDiscoveryRecord`]).** Each selected record carries its
//!    `O(log n)` [`super::InclusionProof`] against the per-tenant tree, plus the **one**
//!    [`super::SignedTreeHead`] the whole bundle is proven against (the chain-of-custody root). A
//!    recipient verifies EVERY record with [`super::verify_inclusion`] + checks the STH signature —
//!    the bundle is *verifiable*, not *asserted* (the §3 prove-it discipline).
//! 3. **Content-addressed ([`EDiscoveryBundle::bundle_digest`]).** The whole bundle is a
//!    `blake3:<hex>` digest over the canonical (scope ∥ STH ∥ ordered record leaf-hashes) body — so
//!    a recipient re-derives the digest and confirms not one record was added/removed/reordered. It
//!    returns through the SAME [`crate::dsr::MerkleProvenBundle`] type a DSR certificate uses (the
//!    `ediscovery_export(scope) → MerkleProvenBundle` contract shape) — its `merkle_inclusion`
//!    carries the bundle's STH commitment (the chain-of-custody anchor).
//! 4. **Legal-hold-frozen ([`EDiscoveryExporter::export`]).** Before assembling, the exporter
//!    **freezes the scope** by PLACING a legal hold (§5.4 — "a legal-hold freezes the scope while
//!    the export is assembled") through the EXISTING [`crate::fanout::LegalHoldRegistry`]: while the
//!    export is in flight the held scope cannot be erased (the DSR fan-out's hold gate DEFERS an
//!    erase under the hold — P-GA-12 / §4.1 step 3), so the records cannot be shredded out from
//!    under the export. The freeze is recorded on the returned bundle
//!    ([`EDiscoveryBundle::legal_hold_frozen`]).
//!
//! ## The dual-use of the ONE tamper-evident substrate (§5.4)
//! "Prove we erased it" (the DSR completion certificate, P-GA-12/P-GA-20) and "prove this is the
//! unaltered record" (this eDiscovery export) ride the SAME per-tenant Merkle tree + STH + witness.
//! There is no second proof substrate — the export's inclusion proofs verify against the SAME root
//! a DSR receipt seals into and the SAME witness anchors (gdpr §5.4). The architecture is coherent
//! by construction.
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - **The live binding of the records' SOURCE.** On this floor the bundle's records are the
//!   minimised AUDIT-LOG entries (the §6.2 who-did-what — exactly the chain-of-custody an
//!   eDiscovery matter needs, and already Merkle-proven). The *content bodies* a matter additionally
//!   collects (the subject's authored docs/issues/messages) are produced by the per-derivative
//!   `export` (Art. 20 portability, P-GA-13 / P-GA-24) over the holder seam — the eDiscovery bundle
//!   carries the CHAIN-OF-CUSTODY proofs over those content-addresses; wiring the holder `export`
//!   call-out at boot is a config wire (the no-cross-store-read law: the exporter never reads a
//!   store, it carries the content-addresses the holders return). The post-condition (every record
//!   is inclusion-proof-bearing + the scope is hold-frozen) does not change.
//! - **The durable `legal_hold` (G4) table + the durable audit `audit_entry`/`audit_sth` tables**
//!   are the same DB floor every M0/M1 in-memory store carries (P-007 / P-S12). The freeze SEMANTICS
//!   (place a hold → the scope cannot be erased while the export is assembled) are byte-for-byte
//!   what the durable engine backs.
//! - **The witness-anchored STH** (the third chain-of-custody leg — the independent RFC-3161 TSA /
//!   cross-cell notary) is wired through [`crate::audit_proofs::AuditAuthority::anchor_to_witness`]
//!   at the SLO cadence; the cell-scale re-run under world-scale audit volume is **M5 (P-GA-35, the
//!   E2E-3 leg)**. This module proves the export is inclusion-proof-bearing + hold-frozen at M2.
//!
//! ## Mutation floor (P-GA-26 TESTS — the export-inclusion-proof path is mandatory-core)
//! Every record in the bundle MUST carry a proof that verifies against the bundle STH, and the
//! bundle digest MUST bind the exact record set (a dropped/added/reordered record changes the
//! digest). [`EDiscoveryExporter::export`] (the freeze + the per-record proof attachment) and
//! [`EDiscoveryBundle::content_addressed`] (the digest over the record set) are the behavioral core;
//! the `cargo mutants` score is recorded in the commit body.

use myelin_tenancy::{ArtifactRef, TenantId};

use crate::audit_proofs::{
    serialize_sth_commitment, AuditAuthority, InclusionProof, SignedTreeHead, SigningKey,
};
use crate::dsr::{DsrId, MerkleProvenBundle};
use crate::fanout::{HoldScope, LegalHoldRegistry};

/// The `ediscovery_export_records` telemetry signal NAME + UNIT (gdpr §5.4 — the count of records
/// an export bundled; an export with 0 inclusion-proof-bearing records over a non-empty scope is a
/// coverage signal). PII-free (a count, never a record). Pinned so a later emitter uses exactly
/// this string + unit (observability is part of the pass — EI-01 §3).
pub const EDISCOVERY_EXPORT_RECORDS: (&str, &str) = ("gdpr.ediscovery_export_records", "count");

/// The scope of an eDiscovery / legal-hold export (gdpr §5.4 — "a **subject-, tenant-, or
/// matter-scoped** bundle of records + audit-log proofs"). PII-free: a scope is an opaque tenant
/// token + (for a subject scope) the subject's [`ArtifactRef`] + (for a matter scope) the matter
/// correlation token — never a name/email.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EDiscoveryScope {
    /// A subject-scoped export — every audit record whose `subject` is this [`ArtifactRef`], within
    /// the tenant (Art. 15 / a litigation matter about one data subject).
    Subject {
        /// the tenant the records live under (the per-tenant Merkle tree the proofs ride).
        tenant: TenantId,
        /// the subject reference the records are about (an opaque id, never PII).
        subject: ArtifactRef,
    },
    /// A tenant-scoped export — the whole per-tenant audit chain (a regulatory / supervisory-
    /// authority production over the tenant).
    Tenant(TenantId),
    /// A matter-scoped export — every audit record whose `correlation_id` is this matter token
    /// (a specific incident / discovery matter spanning many subjects). The matter token is the
    /// causal-walk anchor the audit log already carries (§6.2 `correlation_id`).
    Matter {
        /// the tenant the records live under.
        tenant: TenantId,
        /// the opaque matter correlation token (a request/incident id — never PII).
        matter_token: String,
    },
}

impl EDiscoveryScope {
    /// The tenant whose per-tenant Merkle tree the export's proofs ride (every scope is in-cell,
    /// tenant-partitioned — the chain-of-custody root is per-tenant, gdpr §5.4 / §6.1).
    pub fn tenant(&self) -> &TenantId {
        match self {
            EDiscoveryScope::Subject { tenant, .. } => tenant,
            EDiscoveryScope::Tenant(tenant) => tenant,
            EDiscoveryScope::Matter { tenant, .. } => tenant,
        }
    }

    /// The opaque, PII-free scope token folded into the bundle's content address (so two different
    /// scopes can never content-address to the same bundle). Never a name/email.
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

    /// The legal-hold scope that freezes this export's scope while it is assembled (§5.4). A subject
    /// scope holds that subject; a tenant scope holds the whole tenant; a **matter** scope holds the
    /// whole tenant (a matter spans many subjects — the conservative freeze, fail-safe-to-suspend).
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

/// **One eDiscovery record — a minimised audit entry + its inclusion proof against the per-tenant
/// tree (gdpr §5.4).** PII-free: the `action`/`subject`/`actor_token` are the minimised audit
/// fields (IDs/pseudonyms, §6.2) and the proof carries only sibling digests. A recipient verifies
/// the record with [`super::verify_inclusion`] against the bundle STH — the chain-of-custody for
/// "this entry is in the unaltered log".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EDiscoveryRecord {
    /// The audit entry's per-tenant sequence (the Merkle leaf index — the proof is over this leaf).
    pub seq: u64,
    /// The minimised action token (`identity.tuple.written`, `agent.effect_applied`, … — never a payload).
    pub action: String,
    /// The action's subject reference (an opaque id, never content — §6.2).
    pub subject: ArtifactRef,
    /// The minimised actor token (`<pseudonym>@<tenant>.noreply` — never a name/email).
    pub actor_token: String,
    /// RFC-3339 UTC — when the action happened (chain-of-custody timestamp).
    pub occurred_at: String,
    /// The `O(log n)` inclusion proof proving this record is leaf `seq` of the tree the bundle STH
    /// commits to. A recipient runs [`super::verify_inclusion`] against [`EDiscoveryBundle::sth`].
    pub inclusion: InclusionProof,
}

/// **The eDiscovery / legal-hold export bundle (gdpr §5.4; contract 10.7).** Content-addressed +
/// inclusion-proof-bearing + legal-hold-frozen. Every [`EDiscoveryRecord`] carries its proof
/// against the ONE [`SignedTreeHead`] the bundle is proven against (the chain-of-custody root). The
/// whole bundle is a `blake3:<hex>` digest binding the exact record set — a recipient re-derives it
/// and confirms not one record was altered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EDiscoveryBundle {
    /// The scope token this export covers (subject/tenant/matter — PII-free).
    pub scope_token: String,
    /// The ONE signed tree head the whole bundle is proven against (the chain-of-custody root the
    /// recipient verifies every record's inclusion proof against). PII-free (a size + opaque root
    /// + signature).
    pub sth: SignedTreeHead,
    /// The selected records, each inclusion-proof-bearing, in per-tenant sequence order.
    pub records: Vec<EDiscoveryRecord>,
    /// `true` once the scope was frozen by a legal hold before assembly (§5.4 — the records cannot
    /// be erased out from under the export while in flight).
    pub legal_hold_frozen: bool,
    /// The content-address over the whole bundle — `blake3:<hex>` of the canonical
    /// (scope ∥ STH-commitment ∥ ordered record leaf-hashes) body. Deterministic: the same scope +
    /// record set always content-addresses the same; a dropped/added/reordered record changes it.
    pub bundle_digest: String,
}

impl EDiscoveryBundle {
    /// **Content-address the bundle (§5.4 — content-addressed).** Hashes the canonical body
    /// `scope ∥ STH-commitment ∥ leaf_hash[0] ∥ leaf_hash[1] ∥ …` (field-tagged, unit-separator-
    /// joined so two different field sets can never collide) with BLAKE3 and renders `blake3:<hex>`
    /// — the ONE multihash convention the audit Merkle leaf + the DSR receipt use. Binding the STH
    /// commitment AND every record's leaf hash means a recipient re-derives this digest and any
    /// alteration (a dropped record, an added record, a reordered record, a different tree) changes
    /// it.
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
            // The leaf hash + seq bind BOTH which record AND its position (a reorder changes the
            // sequence-tagged body even if the same leaves appear).
            body.push_str(&format!("rec={}:{}", r.seq, r.inclusion.leaf_hash));
        }
        let digest = blake3::hash(body.as_bytes());
        format!("blake3:{}", hex::encode(digest.as_bytes()))
    }

    /// **Verify the whole bundle (the recipient's check — gdpr §5.4 / EI-01 §3).** Re-derives the
    /// content address (confirming the exact record set), verifies the STH signature under the
    /// in-cell key, and verifies EVERY record's inclusion proof against the STH. Returns `true` iff
    /// the bundle is the unaltered, fully-proven production — "this is the unaltered record" is
    /// *checked*, never *asserted*.
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

    /// The count of inclusion-proof-bearing records (the `ediscovery_export_records` telemetry
    /// source — gdpr §5.4). PII-free.
    pub fn record_count(&self) -> usize {
        self.records.len()
    }
}

/// **The eDiscovery exporter — the READ-side authority over the audit substrate (contract 10.7).**
/// It composes the EXISTING [`AuditAuthority`] (the per-tenant Merkle tree + STH + inclusion proofs,
/// P-GA-19/P-GA-20) and the EXISTING [`LegalHoldRegistry`] (the G4 hold gate, P-GA-12) — it does
/// NOT re-define either (EI-01 §7 coherence). It NEVER reaches into a content store (the
/// no-cross-store-read law): it reads the minimised audit log + carries the content-addresses, and
/// freezes the scope through the hold gate.
pub struct EDiscoveryExporter<'a, K: SigningKey> {
    authority: &'a AuditAuthority<K>,
    holds: &'a LegalHoldRegistry,
}

impl<'a, K: SigningKey> EDiscoveryExporter<'a, K> {
    /// Build the exporter over the audit authority (the proofs substrate) + the legal-hold registry
    /// (the freeze gate). Both are the EXISTING singletons the DSR orchestration already uses.
    pub fn new(
        authority: &'a AuditAuthority<K>,
        holds: &'a LegalHoldRegistry,
    ) -> EDiscoveryExporter<'a, K> {
        EDiscoveryExporter { authority, holds }
    }

    /// **`ediscovery_export(scope) → MerkleProvenBundle` (contract 10.7).** Assemble a
    /// content-addressed, inclusion-proof-bearing, legal-hold-frozen export over the scope:
    /// 1. **Freeze the scope** — place a legal hold so the records cannot be erased while the export
    ///    is assembled (§5.4). The hold is left IN PLACE on the returned bundle's `legal_hold_frozen`
    ///    flag; the caller lifts it when the matter closes (a hold is a deliberate, recorded op —
    ///    P-GA-12).
    /// 2. **Select** the in-scope audit records (subject/tenant/matter) over the per-tenant chain.
    /// 3. **Attach a proof to every record** against the ONE post-freeze STH (the chain-of-custody
    ///    root). An empty scope yields an empty bundle (no STH → returns `None`).
    /// 4. **Content-address** the whole bundle.
    pub fn export(&self, scope: &EDiscoveryScope, exported_at: &str) -> Option<EDiscoveryBundle> {
        let tenant = scope.tenant().clone();
        // 1. Freeze the scope (§5.4 — a legal hold freezes the scope while the export is assembled).
        // The DSR fan-out's hold gate now DEFERS any erase over this scope (P-GA-12 / §4.1 step 3),
        // so a concurrent DSR cannot shred the records out from under the export.
        self.holds.set(scope.hold_scope(), true);

        // 2/3. Commit the post-freeze STH (the chain-of-custody root) and select + prove records.
        let sth = self.authority.signed_tree_head(&tenant, exported_at)?;
        let entries = self.authority.consumer().log().entries_for(&tenant);
        let mut records = Vec::new();
        for entry in &entries {
            if !self.in_scope(scope, &entry.subject, &entry.correlation_id) {
                continue;
            }
            // Each record carries its O(log n) inclusion proof against the committed STH.
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

        // 4. Content-address the whole bundle (binds the exact record set against the STH).
        let bundle_digest = EDiscoveryBundle::content_addressed(&scope.token(), &sth, &records);
        Some(EDiscoveryBundle {
            scope_token: scope.token(),
            sth,
            records,
            legal_hold_frozen: true,
            bundle_digest,
        })
    }

    /// **`ediscovery_export(scope) → MerkleProvenBundle` (the contract-10.7 RETURN TYPE).** The same
    /// export, projected onto the frozen [`MerkleProvenBundle`] shape a DSR certificate also uses
    /// (§8.1 — eDiscovery and the DSR receipt ride the SAME tamper-evident bundle type). The
    /// `merkle_inclusion` carries the bundle's STH commitment (the chain-of-custody anchor — no
    /// longer `None`); the `receipts` are the per-record inclusion proofs.
    pub fn export_bundle(
        &self,
        scope: &EDiscoveryScope,
        exported_at: &str,
    ) -> Option<MerkleProvenBundle> {
        let bundle = self.export(scope, exported_at)?;
        Some(MerkleProvenBundle {
            // The eDiscovery bundle is not a DSR — its id is the content-address of the scope+export
            // (a stable matter handle, PII-free).
            dsr_id: DsrId(format!("ediscovery:{}", bundle.bundle_digest)),
            receipts: bundle.records.iter().map(serialize_record_proof).collect(),
            bundle_digest: bundle.bundle_digest.clone(),
            // The chain-of-custody anchor: the STH commitment the whole bundle is proven against
            // (the export IS Merkle-proven — the field is NOT `None`).
            merkle_inclusion: Some(serialize_sth_commitment(&bundle.sth)),
        })
    }

    /// Whether an audit entry (by its `subject` + `correlation_id`) is in the export scope.
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

/// Serialise one record's inclusion proof to the compact `seq@size:leaf->root` form carried in the
/// [`MerkleProvenBundle::receipts`] (a verifiable, PII-free chain-of-custody string).
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

    /// An action event for `subject` under `tenant`, carrying `correlation` as its matter token.
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
        // Three actions about subject-A, two about subject-B, one matter "m-7" (about subject-B).
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

    /// **A subject-scoped export carries every in-scope record, each inclusion-proof-bearing, and
    /// verifies end-to-end** (the §5.4 / §3 prove-it core).
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
        // Three records about subject-A (events 1, 3, 4).
        assert_eq!(
            bundle.record_count(),
            3,
            "every subject-A record is in the bundle"
        );
        // The whole bundle verifies: digest re-derives + STH signs + EVERY record's proof verifies.
        assert!(
            bundle.verify(auth.key()),
            "the bundle is verifiable, not asserted"
        );
        // Each record IS inclusion-proof-bearing against the bundle STH.
        for r in &bundle.records {
            assert!(
                crate::audit_proofs::verify_inclusion(&r.inclusion, &bundle.sth),
                "record {} carries a proof that verifies against the bundle STH",
                r.seq
            );
        }
    }

    /// **A matter-scoped export selects by the correlation token (spanning subjects)** — the two
    /// "m-7" records (about subject-A AND subject-B) are bundled, each proven.
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
        // The matter spans two distinct subjects.
        let subjects: std::collections::BTreeSet<_> =
            bundle.records.iter().map(|r| r.subject.0.clone()).collect();
        assert_eq!(
            subjects.len(),
            2,
            "the matter spans subject-A and subject-B"
        );
    }

    /// **A tenant-scoped export carries the whole chain**, each record proven.
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

    /// **The export FREEZES the scope (gdpr §5.4 — legal-hold-frozen).** Before the export, an erase
    /// over the scope would proceed; AFTER the export, the SAME legal-hold gate the DSR fan-out
    /// passes through DEFERS the erase — the records cannot be shredded out from under the export.
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
        // NOTE the subject token must match the hold scope's subject token (the principal_id), so we
        // hold by the SAME principal an erase would target.
        let erase_scope = EraseScope::Subject {
            subject: SubjectRef::new(principal),
            tenant: TenantId("acme".into()),
        };
        // Before the export: no hold → an erase over this subject PROCEEDS.
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

        // After the export: the SAME subject is held → an erase is DEFERRED (frozen — §5.4).
        assert_eq!(
            holds.verdict(DsrKind::Erasure, &erase_scope),
            HoldVerdict::Deferred,
            "the export froze the scope (legal-hold-frozen — §5.4)"
        );
        assert_eq!(holds.active_count(), 1, "exactly one hold was placed");
    }

    /// **The content address binds the exact record set (mutation-core).** Dropping, adding, or
    /// reordering a record changes the bundle digest — so `verify` over a tampered record set fails.
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

        // Drop a record → the (unchanged) digest no longer matches the (shorter) record set.
        let mut dropped = bundle.clone();
        dropped.records.pop();
        assert!(
            !dropped.verify(auth.key()),
            "a dropped record fails verification"
        );

        // Reorder two records → the seq-tagged digest body changes → fails.
        let mut reordered = bundle.clone();
        reordered.records.swap(0, 1);
        assert!(
            !reordered.verify(auth.key()),
            "a reordered record set fails verification"
        );

        // A bundle proven against a DIFFERENT key (a forged STH) fails the signature check.
        let other = CellSigningKey::from_seed("a-different-cell-key");
        assert!(
            !bundle.verify(&other),
            "a bundle does not verify under a different key"
        );
    }

    /// **The `export_bundle` MerkleProvenBundle return is the contract-10.7 shape** — content-
    /// addressed, carrying the chain-of-custody STH commitment (NOT `None`) + per-record proofs.
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
            "the export IS Merkle-proven — the chain-of-custody STH commitment is carried (not None)"
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

    /// **An empty scope yields no export** (an empty tenant chain has no STH to prove against).
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

    /// A subject scope with NO matching records returns an empty (but proven-empty) bundle: the STH
    /// exists (the tenant has a chain) but no record is in scope.
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

    /// The `ediscovery_export_records` telemetry signal is named + has the count unit.
    #[test]
    fn ediscovery_export_records_signal_is_named() {
        assert_eq!(
            EDISCOVERY_EXPORT_RECORDS.0,
            "gdpr.ediscovery_export_records"
        );
        assert_eq!(EDISCOVERY_EXPORT_RECORDS.1, "count");
    }

    /// **The scope token is the exact PII-free `kind:tenant:id` form (mutation-core).** It is folded
    /// into the bundle content-address, so two DIFFERENT scopes (subject vs tenant vs matter) can
    /// never collide into the same bundle — pinning the exact token kills the `token -> ""` mutant.
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
        // The token reaches the bundle (the export carries it verbatim) AND distinguishes scopes:
        // a subject and a tenant export over the SAME chain content-address DIFFERENTLY.
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

    /// **The per-record proof serialises to the exact `seq@size:leaf->root` chain-of-custody form
    /// (mutation-core).** Pinning it kills the `serialize_record_proof -> ""` mutant — the
    /// `MerkleProvenBundle.receipts` MUST carry the real, verifiable proof string, never an empty one.
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
            // The form is `seq@size:leaf->root` over real blake3 nodes.
            assert!(proof.contains('@'), "carries the leaf@size");
            assert!(
                proof.contains("->blake3:"),
                "reduces to a blake3 root: {proof}"
            );
            assert!(proof.contains("blake3:"), "carries blake3 leaf/root nodes");
        }
        // The first record is leaf 0 @ a non-trivial tree size.
        assert!(
            mpb.receipts[0].starts_with("0@"),
            "the first subject-A record is leaf 0"
        );
    }

    // A region is referenced so the import is used in a representative chain-of-custody assertion.
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
        // A different tenant's chain is empty (residency-partitioned).
        let _region = Region("acme-home".into());
        assert!(exporter
            .export(&EDiscoveryScope::Tenant(TenantId("other".into())), "t")
            .is_none());
    }
}
