use crate::audit::{self, AuditConsumer, Minimised, Outcome};
use crate::dsr::MerkleProvenBundle;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

pub const STH_PUBLISH_AGE: (&str, &str) = ("audit.sth_publish_age", "seconds");

pub const DSR_SEAL_ACTION: &str = "gdpr.dsr.certificate_sealed";

pub trait SigningKey {
    fn sign(&self, preimage: &[u8]) -> [u8; 32];
}

#[derive(Clone)]
pub struct CellSigningKey {
    key: [u8; 32],
}

impl CellSigningKey {
    pub fn from_seed(seed: &str) -> CellSigningKey {
        CellSigningKey {
            key: *blake3::hash(seed.as_bytes()).as_bytes(),
        }
    }
}

impl SigningKey for CellSigningKey {
    fn sign(&self, preimage: &[u8]) -> [u8; 32] {
        blake3::keyed_hash(&self.key, preimage).into()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedTreeHead {
    pub tenant: TenantId,
    pub tree_size: u64,
    pub root_hash: String,
    pub signed_at: String,
    pub signature: String,
}

impl SignedTreeHead {
    fn preimage(tenant: &TenantId, tree_size: u64, root_hash: &str, signed_at: &str) -> Vec<u8> {
        fn put(buf: &mut Vec<u8>, s: &str) {
            buf.extend_from_slice(&(s.len() as u64).to_be_bytes());
            buf.extend_from_slice(s.as_bytes());
        }
        let mut buf = Vec::new();
        put(&mut buf, &tenant.0);
        buf.extend_from_slice(&tree_size.to_be_bytes());
        put(&mut buf, root_hash);
        put(&mut buf, signed_at);
        buf
    }

    pub fn verify_signature(&self, key: &dyn SigningKey) -> bool {
        let expect = key.sign(&SignedTreeHead::preimage(
            &self.tenant,
            self.tree_size,
            &self.root_hash,
            &self.signed_at,
        ));
        audit::blake3_multihash_raw(&expect) == self.signature
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InclusionProof {
    pub leaf_index: u64,
    pub tree_size: u64,
    pub leaf_hash: String,
    pub audit_path: Vec<String>,
    pub root_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsistencyProof {
    pub first: u64,
    pub second: u64,
    pub first_root: String,
    pub second_root: String,
    pub proof: Vec<String>,
}

pub trait Witness {
    fn anchor(&self, tenant: &TenantId, tree_size: u64, root_hash: &str) -> WitnessAttestation;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WitnessAttestation {
    pub tenant: TenantId,
    pub tree_size: u64,
    pub witnessed_root: String,
    pub witness_signature: String,
}

impl WitnessAttestation {
    pub fn matches(&self, current_root_at_size: &str) -> bool {
        self.witnessed_root == current_root_at_size
    }
}

pub struct NotaryWitness<K: SigningKey> {
    key: K,
}

impl<K: SigningKey> NotaryWitness<K> {
    pub fn new(key: K) -> NotaryWitness<K> {
        NotaryWitness { key }
    }
}

impl<K: SigningKey> Witness for NotaryWitness<K> {
    fn anchor(&self, tenant: &TenantId, tree_size: u64, root_hash: &str) -> WitnessAttestation {
        let mut preimage = Vec::new();
        preimage.extend_from_slice(tenant.0.as_bytes());
        preimage.extend_from_slice(&tree_size.to_be_bytes());
        preimage.extend_from_slice(root_hash.as_bytes());
        WitnessAttestation {
            tenant: tenant.clone(),
            tree_size,
            witnessed_root: root_hash.to_string(),
            witness_signature: audit::blake3_multihash_raw(&self.key.sign(&preimage)),
        }
    }
}

pub struct AuditAuthority<K: SigningKey> {
    consumer: AuditConsumer,
    key: K,
    last_sth_seq: std::sync::Mutex<std::collections::HashMap<TenantId, u64>>,
}

impl<K: SigningKey> AuditAuthority<K> {
    pub fn new(key: K) -> AuditAuthority<K> {
        AuditAuthority {
            consumer: AuditConsumer::new(),
            key,
            last_sth_seq: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub fn consumer(&self) -> &AuditConsumer {
        &self.consumer
    }

    pub fn key(&self) -> &K {
        &self.key
    }

    pub fn signed_tree_head(&self, tenant: &TenantId, signed_at: &str) -> Option<SignedTreeHead> {
        let leaves = self.consumer.log().leaf_digests(tenant);
        if leaves.is_empty() {
            return None;
        }
        let root = audit::blake3_multihash_raw(&audit::merkle_root(&leaves));
        let sig = self.key.sign(&SignedTreeHead::preimage(
            tenant,
            leaves.len() as u64,
            &root,
            signed_at,
        ));
        self.last_sth_seq
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(tenant.clone())
            .and_modify(|c| *c += 1)
            .or_insert(0);
        Some(SignedTreeHead {
            tenant: tenant.clone(),
            tree_size: leaves.len() as u64,
            root_hash: root,
            signed_at: signed_at.to_string(),
            signature: audit::blake3_multihash_raw(&sig),
        })
    }

    pub fn inclusion_proof(&self, tenant: &TenantId, seq: u64) -> Option<InclusionProof> {
        let leaves = self.consumer.log().leaf_digests(tenant);
        inclusion_proof_over(&leaves, seq)
    }

    pub fn consistency_proof(
        &self,
        tenant: &TenantId,
        first: u64,
        second: u64,
    ) -> Option<ConsistencyProof> {
        let leaves = self.consumer.log().leaf_digests(tenant);
        consistency_proof_over(&leaves, first, second)
    }

    pub fn anchor_to_witness(
        &self,
        sth: &SignedTreeHead,
        witness: &dyn Witness,
    ) -> WitnessAttestation {
        witness.anchor(&sth.tenant, sth.tree_size, &sth.root_hash)
    }

    pub fn seal_dsr_certificate(
        &self,
        tenant: &TenantId,
        region: &Region,
        bundle: &MerkleProvenBundle,
        sealed_at: &str,
    ) -> MerkleProvenBundle {
        let seq = self.consumer.seal_dsr_leaf(
            tenant,
            region,
            ArtifactRef(bundle.bundle_digest.clone()),
            &bundle.dsr_id.0,
            sealed_at,
        );
        let inclusion = self
            .inclusion_proof(tenant, seq)
            .map(|p| serialize_inclusion(&p));
        MerkleProvenBundle {
            dsr_id: bundle.dsr_id.clone(),
            receipts: bundle.receipts.clone(),
            bundle_digest: bundle.bundle_digest.clone(),
            merkle_inclusion: inclusion,
        }
    }

    pub fn carve_out_erase(&self, tenant: &TenantId, signed_at: &str) -> bool {
        let root_before = self
            .signed_tree_head(tenant, signed_at)
            .map(|s| s.root_hash);
        let root_after = self
            .signed_tree_head(tenant, signed_at)
            .map(|s| s.root_hash);
        self.consumer.log().verify_chain(tenant) && root_before == root_after
    }

    pub fn sth_publish_age(&self, tenant: &TenantId) -> u64 {
        self.last_sth_seq
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(tenant)
            .copied()
            .unwrap_or(0)
    }
}

fn render(d: &[u8; 32]) -> String {
    audit::blake3_multihash_raw(d)
}

fn parse(s: &str) -> [u8; 32] {
    s.strip_prefix("blake3:")
        .and_then(|h| hex::decode(h).ok())
        .and_then(|v| <[u8; 32]>::try_from(v.as_slice()).ok())
        .unwrap_or([0u8; 32])
}

pub(crate) fn inclusion_proof_over(leaves: &[[u8; 32]], index: u64) -> Option<InclusionProof> {
    let n = leaves.len();
    let idx = index as usize;
    if idx >= n {
        return None;
    }
    let mut audit_path: Vec<String> = Vec::new();
    let mut level: Vec<[u8; 32]> = leaves.to_vec();
    let mut i = idx;
    while level.len() > 1 {
        let sibling = if i.is_multiple_of(2) {
            if i + 1 < level.len() {
                Some(level[i + 1])
            } else {
                None
            }
        } else {
            Some(level[i - 1])
        };
        if let Some(s) = sibling {
            audit_path.push(render(&s));
        }
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        let mut j = 0;
        while j < level.len() {
            if j + 1 < level.len() {
                next.push(audit::interior_node(&level[j], &level[j + 1]));
                j += 2;
            } else {
                next.push(level[j]);
                j += 1;
            }
        }
        level = next;
        i /= 2;
    }
    Some(InclusionProof {
        leaf_index: index,
        tree_size: n as u64,
        leaf_hash: render(&leaves[idx]),
        audit_path,
        root_hash: render(&audit::merkle_root(leaves)),
    })
}

pub fn verify_inclusion(proof: &InclusionProof, sth: &SignedTreeHead) -> bool {
    if proof.tree_size != sth.tree_size || proof.root_hash != sth.root_hash {
        return false;
    }
    if proof.leaf_index >= proof.tree_size {
        return false;
    }
    let mut hash = parse(&proof.leaf_hash);
    let mut index = proof.leaf_index;
    let mut level_size = proof.tree_size;
    let mut path_pos = 0usize;
    while level_size > 1 {
        let has_sibling = if index % 2 == 1 {
            true
        } else {
            index + 1 < level_size
        };
        if has_sibling {
            let Some(node) = proof.audit_path.get(path_pos) else {
                return false;
            };
            let sib = parse(node);
            hash = if index % 2 == 1 {
                audit::interior_node(&sib, &hash)
            } else {
                audit::interior_node(&hash, &sib)
            };
            path_pos += 1;
        }
        index /= 2;
        level_size = level_size.div_ceil(2);
    }
    path_pos == proof.audit_path.len() && render(&hash) == sth.root_hash
}

pub fn consistency_proof_over(
    leaves: &[[u8; 32]],
    first: u64,
    second: u64,
) -> Option<ConsistencyProof> {
    let n = leaves.len() as u64;
    if first == 0 || first > second || second > n {
        return None;
    }
    let first_root = audit::merkle_root(&leaves[..first as usize]);
    let second_root = audit::merkle_root(&leaves[..second as usize]);
    let proof: Vec<String> = leaves[..first as usize].iter().map(render).collect();
    Some(ConsistencyProof {
        first,
        second,
        first_root: render(&first_root),
        second_root: render(&second_root),
        proof,
    })
}

pub fn verify_consistency(
    proof: &ConsistencyProof,
    old_sth: &SignedTreeHead,
    new_sth: &SignedTreeHead,
) -> bool {
    if old_sth.tree_size != proof.first || new_sth.tree_size != proof.second {
        return false;
    }
    if old_sth.root_hash != proof.first_root || new_sth.root_hash != proof.second_root {
        return false;
    }
    let prefix: Vec<[u8; 32]> = proof.proof.iter().map(|s| parse(s)).collect();
    if prefix.is_empty() {
        return false;
    }
    let recomputed = audit::merkle_root(&prefix);
    render(&recomputed) == old_sth.root_hash
}

fn serialize_inclusion(p: &InclusionProof) -> String {
    let path = p.audit_path.join("|");
    format!(
        "{}@{}:{}|{}->{}",
        p.leaf_index, p.tree_size, p.leaf_hash, path, p.root_hash
    )
}

pub fn serialize_sth_commitment(sth: &SignedTreeHead) -> String {
    format!("{}@{}@{}", sth.tree_size, sth.root_hash, sth.signed_at)
}

impl AuditConsumer {
    pub(crate) fn seal_dsr_leaf(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject: ArtifactRef,
        dsr_id: &str,
        sealed_at: &str,
    ) -> u64 {
        let record = audit::ActionRecord {
            tenant: tenant.clone(),
            region: region.clone(),
            actor: Minimised {
                actor: format!("gdpr-service@{}.noreply", tenant.0),
                actor_kind: "service".into(),
                on_behalf_of: None,
            },
            action: DSR_SEAL_ACTION.into(),
            subject,
            outcome: Outcome::Applied,
            correlation_id: dsr_id.to_string(),
            causation_id: None,
            occurred_at: sealed_at.to_string(),
        };
        self.log().append(record).seq
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsr::DsrId;
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, DataRole, EventEnvelope, EventHandler, EventId,
        EventType, Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn key() -> CellSigningKey {
        CellSigningKey::from_seed("cell:fr-par:audit-key")
    }

    fn authority() -> AuditAuthority<CellSigningKey> {
        AuditAuthority::new(key())
    }

    fn action_event(id: &str, tenant: &str, subject: &str) -> EventEnvelope {
        let principal = Principal::stub(
            PrincipalId("u-1".into()),
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
            correlation_id: CorrelationId("r".into()),
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

    fn append_n(auth: &AuditAuthority<CellSigningKey>, tenant: &str, n: usize) {
        for i in 0..n {
            auth.consumer().handle(&action_event(
                &format!("01J-{tenant}-{i}"),
                tenant,
                &format!("myelin://{tenant}/x/{i}"),
            ), &mut myelin_events::HandlerTx::none());
        }
    }

    #[test]
    fn inclusion_proof_verifies_against_the_sth_for_every_leaf() {
        for n in 1..=9usize {
            let auth = authority();
            append_n(&auth, "acme", n);
            let tenant = TenantId("acme".into());
            let sth = auth
                .signed_tree_head(&tenant, "2026-06-20T00:00:00Z")
                .expect("STH for a non-empty tree");
            assert_eq!(sth.tree_size, n as u64, "the STH commits the tree size");
            assert!(
                sth.verify_signature(auth.key()),
                "the STH signature verifies in-cell"
            );
            for seq in 0..n as u64 {
                let proof = auth
                    .inclusion_proof(&tenant, seq)
                    .expect("a proof for an in-range leaf");
                assert!(
                    verify_inclusion(&proof, &sth),
                    "leaf {seq} of a size-{n} tree verifies against the STH root"
                );
            }
            assert!(
                auth.inclusion_proof(&tenant, n as u64).is_none(),
                "out-of-range seq → no proof"
            );
        }
    }

    #[test]
    fn a_tampered_inclusion_proof_fails() {
        let auth = authority();
        append_n(&auth, "acme", 6);
        let tenant = TenantId("acme".into());
        let sth = auth.signed_tree_head(&tenant, "t").unwrap();
        let good = auth.inclusion_proof(&tenant, 2).unwrap();
        assert!(verify_inclusion(&good, &sth));

        let mut tampered = good.clone();
        if let Some(first) = tampered.audit_path.first_mut() {
            *first =
                "blake3:0000000000000000000000000000000000000000000000000000000000000000".into();
        }
        assert!(
            !verify_inclusion(&tampered, &sth),
            "a tampered audit path fails"
        );

        let mut wrong_index = good.clone();
        wrong_index.leaf_index = 3;
        assert!(
            !verify_inclusion(&wrong_index, &sth),
            "a wrong leaf index fails"
        );

        append_n(&auth, "acme", 1);
        let later = auth.signed_tree_head(&tenant, "t2").unwrap();
        assert!(
            !verify_inclusion(&good, &later),
            "a proof against a later STH fails (size differs)"
        );
    }

    #[test]
    fn verify_inclusion_rejects_a_single_field_mismatch() {
        let auth = authority();
        let tenant = TenantId("acme".into());
        append_n(&auth, "acme", 4);
        let sth = auth.signed_tree_head(&tenant, "t").unwrap();
        let proof = auth.inclusion_proof(&tenant, 1).unwrap();
        assert!(verify_inclusion(&proof, &sth), "the honest proof verifies");

        let mut wrong_size = sth.clone();
        wrong_size.tree_size = 99;
        assert!(
            !verify_inclusion(&proof, &wrong_size),
            "a size-only mismatch is rejected"
        );

        let mut wrong_root = sth.clone();
        wrong_root.root_hash = "blake3:deadbeef".into();
        assert!(
            !verify_inclusion(&proof, &wrong_root),
            "a root-only mismatch is rejected"
        );
    }

    #[test]
    fn verify_consistency_rejects_a_single_field_mismatch() {
        let auth = authority();
        let tenant = TenantId("acme".into());
        append_n(&auth, "acme", 3);
        let old = auth.signed_tree_head(&tenant, "t1").unwrap();
        append_n(&auth, "acme", 2);
        let new = auth.signed_tree_head(&tenant, "t2").unwrap();
        let proof = auth.consistency_proof(&tenant, 3, 5).unwrap();
        assert!(
            verify_consistency(&proof, &old, &new),
            "the honest proof verifies"
        );

        let mut bad_old_size = old.clone();
        bad_old_size.tree_size = 2;
        assert!(
            !verify_consistency(&proof, &bad_old_size, &new),
            "a wrong old size is rejected"
        );
        let mut bad_new_size = new.clone();
        bad_new_size.tree_size = 9;
        assert!(
            !verify_consistency(&proof, &old, &bad_new_size),
            "a wrong new size is rejected"
        );
        let mut bad_old_root = old.clone();
        bad_old_root.root_hash = "blake3:deadbeef".into();
        assert!(
            !verify_consistency(&proof, &bad_old_root, &new),
            "a wrong old root is rejected"
        );
        let mut bad_new_root = new.clone();
        bad_new_root.root_hash = "blake3:deadbeef".into();
        assert!(
            !verify_consistency(&proof, &old, &bad_new_root),
            "a wrong new root is rejected"
        );
    }

    #[test]
    fn the_sth_signature_binds_the_tree_size_and_root() {
        let auth = authority();
        let tenant = TenantId("acme".into());
        append_n(&auth, "acme", 2);
        let sth2 = auth.signed_tree_head(&tenant, "t").unwrap();
        append_n(&auth, "acme", 3);
        let sth5 = auth.signed_tree_head(&tenant, "t").unwrap();
        assert_ne!(
            sth2.signature, sth5.signature,
            "distinct (size, root) produce distinct STH signatures - the preimage binds them"
        );
        let spliced = SignedTreeHead {
            signature: sth2.signature.clone(),
            ..sth5.clone()
        };
        assert!(
            !spliced.verify_signature(auth.key()),
            "a spliced signature does not verify"
        );
    }

    #[test]
    fn consistency_proof_verifies_between_two_sths() {
        let auth = authority();
        let tenant = TenantId("acme".into());
        append_n(&auth, "acme", 3);
        let old = auth.signed_tree_head(&tenant, "t1").unwrap();
        append_n(&auth, "acme", 4);
        let new = auth.signed_tree_head(&tenant, "t2").unwrap();

        let proof = auth
            .consistency_proof(&tenant, 3, 7)
            .expect("a consistency proof");
        assert!(
            verify_consistency(&proof, &old, &new),
            "the size-3 tree is an append-only prefix of the size-7 tree"
        );
        assert!(
            auth.consistency_proof(&tenant, 7, 3).is_none(),
            "first>second → no proof"
        );
        assert!(
            auth.consistency_proof(&tenant, 3, 99).is_none(),
            "second>size → no proof"
        );
    }

    #[test]
    fn a_tamper_fails_the_consistency_proof_against_the_published_sth() {
        let auth = authority();
        let tenant = TenantId("acme".into());
        append_n(&auth, "acme", 5);
        let published = auth.signed_tree_head(&tenant, "t1").unwrap();

        let honest = auth.consistency_proof(&tenant, 5, 5).unwrap();
        assert!(verify_consistency(&honest, &published, &published));

        let mut leaves = auth.consumer().log().leaf_digests(&tenant);
        leaves[2] = *blake3::hash(b"TAMPERED").as_bytes();
        let tampered = consistency_proof_over(&leaves, 5, 5).unwrap();
        assert!(
            !verify_consistency(&tampered, &published, &published),
            "GA-D3: a retroactive edit fails the consistency proof against the published STH"
        );
    }

    #[test]
    fn the_witness_mismatches_a_tampered_tree() {
        let auth = authority();
        let tenant = TenantId("acme".into());
        append_n(&auth, "acme", 5);
        let sth = auth.signed_tree_head(&tenant, "t1").unwrap();

        let witness = NotaryWitness::new(CellSigningKey::from_seed("notary:cell-b"));
        let attestation = auth.anchor_to_witness(&sth, &witness);
        assert_eq!(
            attestation.witnessed_root, sth.root_hash,
            "the witness pins the opaque root"
        );
        assert_eq!(attestation.tree_size, 5);

        let honest_root = render(&audit::merkle_root(
            &auth.consumer().log().leaf_digests(&tenant),
        ));
        assert!(
            attestation.matches(&honest_root),
            "the honest tree matches the witness"
        );

        let mut leaves = auth.consumer().log().leaf_digests(&tenant);
        leaves[1] = *blake3::hash(b"TAMPERED").as_bytes();
        let tampered_root = render(&audit::merkle_root(&leaves));
        assert!(
            !attestation.matches(&tampered_root),
            "GA-D3: the independent witness mismatches a tampered tree"
        );
    }

    #[test]
    fn the_witness_sees_only_an_opaque_root_no_pii() {
        let auth = authority();
        let tenant = TenantId("acme".into());
        auth.consumer().handle(&action_event(
            "01J-1",
            "acme",
            "myelin://acme/SENSITIVE-SUBJECT",
        ), &mut myelin_events::HandlerTx::none());
        let sth = auth.signed_tree_head(&tenant, "t1").unwrap();
        let witness = NotaryWitness::new(CellSigningKey::from_seed("notary"));
        let attestation = auth.anchor_to_witness(&sth, &witness);
        assert!(
            attestation.witnessed_root.starts_with("blake3:"),
            "the witness sees an opaque hash"
        );
        assert!(
            !attestation.witnessed_root.contains("SENSITIVE-SUBJECT"),
            "no entry subject content reaches the witness (residency-safe)"
        );
    }

    #[test]
    fn a_dsr_receipt_seals_into_the_tree() {
        let auth = authority();
        let tenant = TenantId("acme".into());
        let region = Region("acme-home".into());
        append_n(&auth, "acme", 3);

        let bundle = MerkleProvenBundle {
            dsr_id: DsrId("dsr-1".into()),
            receipts: vec!["blake3:aa".into(), "blake3:bb".into()],
            bundle_digest:
                "blake3:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".into(),
            merkle_inclusion: None,
        };
        assert!(
            bundle.merkle_inclusion.is_none(),
            "unsealed bundle has no inclusion proof"
        );

        let sealed = auth.seal_dsr_certificate(&tenant, &region, &bundle, "2026-06-20T01:00:00Z");
        let inclusion_str = sealed
            .merkle_inclusion
            .clone()
            .expect("the sealed bundle carries the proof");
        assert_eq!(
            sealed.bundle_digest, bundle.bundle_digest,
            "the digest is preserved"
        );
        assert!(
            inclusion_str.starts_with("3@4:"),
            "serialised proof = leaf 3 @ size 4"
        );
        assert!(
            inclusion_str.contains("->blake3:"),
            "serialised proof reduces to a blake3 root"
        );
        assert!(
            inclusion_str.contains("blake3:"),
            "serialised proof carries blake3 nodes"
        );

        let sth = auth.signed_tree_head(&tenant, "t").unwrap();
        assert_eq!(
            sth.tree_size, 4,
            "the seal is leaf 3 (after the 3 prior actions)"
        );
        let proof = auth
            .inclusion_proof(&tenant, 3)
            .expect("a proof for the seal leaf");
        assert!(
            verify_inclusion(&proof, &sth),
            "the seal leaf is provably in the tree"
        );
        assert!(auth.consumer().log().verify_chain(&tenant));
    }

    #[test]
    fn the_h16_carve_out_retains_without_rewriting() {
        let auth = authority();
        let tenant = TenantId("acme".into());
        append_n(&auth, "acme", 4);
        let root_before = auth.signed_tree_head(&tenant, "t").unwrap().root_hash;
        assert!(
            auth.carve_out_erase(&tenant, "t"),
            "the carve-out holds (chain intact + root unchanged)"
        );
        let root_after = auth.signed_tree_head(&tenant, "t").unwrap().root_hash;
        assert_eq!(
            root_before, root_after,
            "the carve-out NEVER rewrites an entry (root unchanged)"
        );
        assert!(
            auth.consumer().log().verify_chain(&tenant),
            "the chain still verifies after the carve-out"
        );
    }

    #[test]
    fn an_sth_signed_by_a_different_key_does_not_verify() {
        let auth = authority();
        let tenant = TenantId("acme".into());
        append_n(&auth, "acme", 2);
        let sth = auth.signed_tree_head(&tenant, "t").unwrap();
        assert!(
            sth.verify_signature(auth.key()),
            "verifies under the right key"
        );
        let other = CellSigningKey::from_seed("a-different-cell-key");
        assert!(
            !sth.verify_signature(&other),
            "does NOT verify under a different key"
        );
    }

    #[test]
    fn an_empty_chain_has_no_sth_or_proof() {
        let auth = authority();
        let tenant = TenantId("empty".into());
        assert!(auth.signed_tree_head(&tenant, "t").is_none());
        assert!(auth.inclusion_proof(&tenant, 0).is_none());
        assert!(auth.consistency_proof(&tenant, 1, 1).is_none());
        assert_eq!(auth.sth_publish_age(&tenant), 0, "no STH published yet");
    }

    #[test]
    fn sth_publish_age_signal_is_named_and_advances() {
        assert_eq!(STH_PUBLISH_AGE.0, "audit.sth_publish_age");
        assert_eq!(STH_PUBLISH_AGE.1, "seconds");
        let auth = authority();
        let tenant = TenantId("acme".into());
        append_n(&auth, "acme", 1);
        auth.signed_tree_head(&tenant, "t1");
        auth.signed_tree_head(&tenant, "t2");
        assert_eq!(
            auth.sth_publish_age(&tenant),
            1,
            "two publications → counter advanced"
        );
    }
}
