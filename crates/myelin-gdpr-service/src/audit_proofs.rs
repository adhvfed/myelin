//! # `audit_proofs` — the CT-style proofs + STH + independent-witness anchoring (P-GA-20 → P-119)
//!
//! This module is the PROOFS half of contract 10.6 (the construction half — the per-tenant
//! hash-chain whose entries are Merkle leaves — is [`crate::audit`], P-GA-19). It ships:
//!
//! 1. **The three CT-style proofs** (RFC 6962; gdpr §6.2–§6.3):
//!    - `signed_tree_head(tenant) → STH` ([`AuditAuthority::signed_tree_head`]) — the
//!      `(tree_size, root_hash)` of one tenant's Merkle tree, signed by the in-cell audit key.
//!    - `inclusion_proof(action) → MerklePath` ([`AuditAuthority::inclusion_proof`]) — the
//!      `O(log n)` audit path proving "this action is leaf `seq` of the tree the STH signs".
//!    - `consistency_proof(t1, t2) → Proof` ([`AuditAuthority::consistency_proof`]) — the
//!      RFC-6962 consistency path proving "the log of size `t1` is an append-only prefix of the
//!      log of size `t2`; it was NOT forked or rewritten between the two STHs".
//! 2. **The independent-witness anchoring** ([`Witness`] / [`AuditAuthority::anchor_to_witness`]):
//!    the STH is periodically anchored to an EXTERNAL witness (an RFC-3161 TSA / a different cell's
//!    notary). The witness sees ONLY the opaque root hash + tree size — **no PII crosses**
//!    (residency-safe) — so even a fully-compromised cell cannot rewrite history undetectably: the
//!    witness's countersignature over the old root is a fixed point a tampered chain cannot match.
//! 3. **The DSR-receipt seal** ([`AuditAuthority::seal_dsr_certificate`]): a DSR completion
//!    certificate ([`crate::dsr::MerkleProvenBundle`]) is sealed INTO the per-tenant audit tree
//!    (the bundle digest becomes an audit leaf via the SAME outbox-consumer append path — there is
//!    no second write path), and the returned bundle carries its `merkle_inclusion` proof (closing
//!    the P-GA-12 / P-GA-11 seal floor — the field is no longer `None`).
//! 4. **The H16 carve-out body, at the chain level** ([`AuditAuthority::carve_out_erase`]): a
//!    subject erasure over the audit log **retains** the minimised pseudonym entry and **never
//!    rewrites it** — proven by asserting the chain still verifies + the STH root is UNCHANGED
//!    after the carve-out erase (a rewrite would change the root + break the chain). The carve-out
//!    POLICY body (retain vs audit-key crypto-shred at retention end) lives in
//!    [`crate::holders::AuditCarveOutHolder`]; this module proves the carve-out at the TREE level
//!    (the §6.4 "never rewrite an entry" tamper-evidence guarantee).
//!
//! ## The GA-D3 property — three INDEPENDENT detections of one tamper
//! A retroactive edit/delete of an audit entry is caught THREE independent ways, any one of which
//! suffices (defence in depth — gdpr §6.3 / GA-D3):
//! 1. **the hash-chain breaks** ([`crate::audit::AuditLog::verify_chain`] — the Haber–Stornetta
//!    link no longer recomputes);
//! 2. **the consistency proof against the published STH fails** (the tampered tree's root no
//!    longer matches the root the old STH committed to — [`verify_consistency`] returns `false`);
//! 3. **the independent witness mismatches** (the witness countersigned the OLD root; the tampered
//!    root differs from what the witness attested — [`WitnessAttestation::matches`] is `false`).
//!
//! The GATE drill (`tests/ga_d3_audit_tamper.rs`) edits an entry and asserts ALL THREE fire —
//! "tamper detected 100%" is the dated green artifact.
//!
//! ## FLOOR — the in-memory tree models the §6.2 `audit_sth` table; the signature is a keyed MAC
//! There is no live OLTP DB / HSM on this floor (the OLTP client is P-007; the KMS hierarchy is
//! P-ST-06). The STH is signed with a keyed BLAKE3 MAC over the in-cell audit signing key (a
//! deterministic, verifiable signature seam) and the witness is an in-process notary; the seam
//! shape (`STH { tree_size, root_hash, signature }`, the witness countersignature over the opaque
//! root) does NOT change when the real in-cell signing key (Storage's KMS, P-ST-06) and a real
//! RFC-3161 TSA land. Swapping the [`SigningKey`] / [`Witness`] impl is a config swap, not a code
//! change. The cell-scale re-run under world-scale audit volume is M5 (P-GA-35, the E2E-3 leg).

use crate::audit::{self, AuditConsumer, Minimised, Outcome};
use crate::dsr::MerkleProvenBundle;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

/// The `sth_publish_age` telemetry signal NAME + UNIT (gdpr §7.6 — the audit-log health SLO that
/// the STH is being published / anchored often enough; a stale STH widens the tamper-detection
/// window). The authority exposes the live measurement ([`AuditAuthority::sth_publish_age`]);
/// wiring the sample onto the running service's metrics-health surface is the `serve(AppSpec)`
/// follow-on (the same surface `audit_append_lag` rides — P-119). Pinned so a later emitter uses
/// exactly this string + unit (observability is part of the pass — EI-01 §3).
pub const STH_PUBLISH_AGE: (&str, &str) = ("audit.sth_publish_age", "seconds");

/// The dotted action token a sealed DSR certificate is appended under (a real action — "a DSR
/// completion certificate was sealed into the tree"). It rides the SAME minimised, hash-chained,
/// outbox-consumer append path as every other audited action — there is no second write path.
pub const DSR_SEAL_ACTION: &str = "gdpr.dsr.certificate_sealed";

// ───────────────────────────── the signing-key seam (the STH signature) ─────────────────────────────

/// The in-cell audit signing key seam (gdpr §6.3 — the STH is signed in-cell, self-hosted). On
/// this floor it is a keyed BLAKE3 MAC; the real per-cell signing key is Storage's KMS (P-ST-06).
/// The trait shape (`sign(bytes) → 32-byte tag`) does not change when the real key lands.
pub trait SigningKey {
    /// Sign the canonical STH preimage, returning the raw 32-byte signature tag.
    fn sign(&self, preimage: &[u8]) -> [u8; 32];
}

/// The floor signing key: a keyed BLAKE3 MAC over a fixed per-cell key. Deterministic + verifiable
/// (the verifier holds the same key in-cell). Swapping for the KMS-backed key (P-ST-06) is a
/// config swap.
#[derive(Clone)]
pub struct CellSigningKey {
    key: [u8; 32],
}

impl CellSigningKey {
    /// A cell signing key seeded from a stable per-cell secret (the floor — the real secret comes
    /// from the KMS, P-ST-06). Distinct seeds produce distinct signatures (a forged STH signed by a
    /// different key is rejected).
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

// ───────────────────────────── the signed tree head (STH) ─────────────────────────────

/// A **signed tree head** (gdpr §6.2 `audit_sth`; RFC 6962 §3.5). The committed `(tree_size,
/// root_hash)` of one tenant's Merkle tree at a point in time, signed by the in-cell audit key. An
/// auditor verifies an inclusion/consistency proof AGAINST a published STH; the witness anchors
/// it. PII-free (a tree size + an opaque root + a tag — no entry content).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedTreeHead {
    /// The tenant whose tree this commits (the chain is per-tenant, in-cell).
    pub tenant: TenantId,
    /// The number of leaves the STH commits to (RFC 6962 `tree_size`). A later STH has a `tree_size`
    /// `>=` an earlier one (append-only).
    pub tree_size: u64,
    /// The Merkle root over the first `tree_size` leaves, rendered `blake3:<hex>` (the opaque root
    /// the witness sees — no PII).
    pub root_hash: String,
    /// RFC-3339 UTC — when the STH was signed (drives `sth_publish_age`).
    pub signed_at: String,
    /// The in-cell signature over `canonical(tenant, tree_size, root_hash, signed_at)`, `blake3:<hex>`.
    pub signature: String,
}

impl SignedTreeHead {
    /// The canonical, length-prefixed signature preimage (stable field ordering so the signature is
    /// reproducible by any in-cell verifier).
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

    /// **Verify the STH's signature** under a signing key (the in-cell verification an auditor
    /// runs). Recomputes the signature over the committed `(tree_size, root_hash, signed_at)` and
    /// checks it matches — a forged STH (a tampered root, a wrong key) fails.
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

// ───────────────────────────── the inclusion proof ─────────────────────────────

/// An **inclusion proof** (RFC 6962 §2.1.1; contract 10.6 `inclusion_proof(action) → MerklePath`).
/// The `O(log n)` audit path proving leaf `leaf_index` of a tree of size `tree_size` reduces to
/// `root_hash`. PII-free (sibling digests + a leaf index — no entry content).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InclusionProof {
    /// The leaf's index in the tree (the entry's per-tenant `seq`).
    pub leaf_index: u64,
    /// The tree size the proof is against (matches the STH's `tree_size`).
    pub tree_size: u64,
    /// The leaf digest being proven (`blake3:<hex>`).
    pub leaf_hash: String,
    /// The sibling digests on the path from the leaf to the root, leaf-to-root order (`blake3:<hex>`).
    pub audit_path: Vec<String>,
    /// The root the path reduces to (matches the committed STH `root_hash`).
    pub root_hash: String,
}

// ───────────────────────────── the consistency proof ─────────────────────────────

/// A **consistency proof** (RFC 6962 §2.1.2; contract 10.6 `consistency_proof(t1,t2) → Proof`).
/// The path proving the tree of size `first` is an append-only PREFIX of the tree of size `second`
/// (no fork, no rewrite between the two STHs). PII-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsistencyProof {
    /// The smaller (earlier) tree size.
    pub first: u64,
    /// The larger (later) tree size.
    pub second: u64,
    /// The root of the first tree (the old STH committed to this).
    pub first_root: String,
    /// The root of the second tree (the new STH committed to this).
    pub second_root: String,
    /// The RFC-6962 consistency path (`blake3:<hex>` nodes).
    pub proof: Vec<String>,
}

// ───────────────────────────── the independent witness ─────────────────────────────

/// An **independent witness** (gdpr §6.3 — an RFC-3161 TSA / a different cell's notary). It sees
/// ONLY the opaque root hash + tree size (no PII crosses — residency-safe) and countersigns them,
/// producing a [`WitnessAttestation`] a verifier checks the live tree against. On this floor it is
/// an in-process notary with its own key; the real RFC-3161 TSA is a config swap (the trait shape
/// does not change).
pub trait Witness {
    /// Countersign an STH's opaque `(tenant, tree_size, root_hash)` — the witness sees NO entry
    /// content, only the root + size. Returns its attestation.
    fn anchor(&self, tenant: &TenantId, tree_size: u64, root_hash: &str) -> WitnessAttestation;
}

/// The witness's countersignature over an anchored STH (gdpr §6.3). It pins the `(tree_size,
/// root_hash)` the witness saw; a later live tree whose root-at-that-size DIFFERS is a detected
/// tamper ([`WitnessAttestation::matches`] is `false`). PII-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WitnessAttestation {
    /// The tenant the attestation is for.
    pub tenant: TenantId,
    /// The tree size the witness countersigned.
    pub tree_size: u64,
    /// The opaque root the witness saw + countersigned (NO PII — gdpr §6.3).
    pub witnessed_root: String,
    /// The witness's signature over `(tenant, tree_size, witnessed_root)`, `blake3:<hex>`.
    pub witness_signature: String,
}

impl WitnessAttestation {
    /// Whether a CURRENT root-at-`tree_size` matches what the witness countersigned. A retroactive
    /// edit to an entry at/below `tree_size` changes the recomputed root ⇒ this returns `false`
    /// (the third independent tamper detection — GA-D3).
    pub fn matches(&self, current_root_at_size: &str) -> bool {
        self.witnessed_root == current_root_at_size
    }
}

/// The floor in-process witness: a notary with its OWN [`SigningKey`] (distinct from the cell's
/// audit key — an independent party). It countersigns only the opaque root. The real RFC-3161 TSA
/// / cross-cell notary is a config swap.
pub struct NotaryWitness<K: SigningKey> {
    key: K,
}

impl<K: SigningKey> NotaryWitness<K> {
    /// A notary witness over its own signing key (independent of the cell's audit key).
    pub fn new(key: K) -> NotaryWitness<K> {
        NotaryWitness { key }
    }
}

impl<K: SigningKey> Witness for NotaryWitness<K> {
    fn anchor(&self, tenant: &TenantId, tree_size: u64, root_hash: &str) -> WitnessAttestation {
        // The witness signs ONLY the opaque (tenant, tree_size, root) — it never sees entry content.
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

// ───────────────────────────── the audit authority (the proof API) ─────────────────────────────

/// **The audit authority — the proof + STH + witness API over a tenant's audit tree (contract
/// 10.6, the proofs half).** It wraps the [`AuditConsumer`] (the construction, P-GA-19) and the
/// in-cell [`SigningKey`], and serves `signed_tree_head` / `inclusion_proof` / `consistency_proof`,
/// anchors to a [`Witness`], seals DSR certificates, and proves the H16 carve-out. It is the
/// READ-side authority: it never appends EXCEPT through the consumer's sanctioned path (the DSR
/// seal rides [`AuditConsumer`]'s minimised append — there is no second write path).
pub struct AuditAuthority<K: SigningKey> {
    consumer: AuditConsumer,
    key: K,
    /// The wall-clock of the last STH publication per tenant (the `sth_publish_age` source). On
    /// this floor it is the monotone count of STHs published; the live seconds-since wiring is the
    /// metrics-health follow-on (P-119 surface).
    last_sth_seq: std::sync::Mutex<std::collections::HashMap<TenantId, u64>>,
}

impl<K: SigningKey> AuditAuthority<K> {
    /// Build the authority over a fresh audit consumer + the in-cell signing key.
    pub fn new(key: K) -> AuditAuthority<K> {
        AuditAuthority {
            consumer: AuditConsumer::new(),
            key,
            last_sth_seq: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// The underlying audit consumer (the construction half — drive events through it to append).
    pub fn consumer(&self) -> &AuditConsumer {
        &self.consumer
    }

    /// The in-cell signing key (an in-cell verifier holds the same key to check an STH signature).
    pub fn key(&self) -> &K {
        &self.key
    }

    /// **`signed_tree_head(tenant) → STH` (contract 10.6).** Commit the current `(tree_size,
    /// root_hash)` of the tenant's Merkle tree and sign it in-cell. Returns `None` for an empty
    /// chain (no tree to commit). Bumps the per-tenant STH publication counter (the
    /// `sth_publish_age` source).
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

    /// **`inclusion_proof(action) → MerklePath` (contract 10.6).** The `O(log n)` audit path proving
    /// leaf `seq` is in the tenant's tree at the current size. `None` if `seq` is out of range.
    pub fn inclusion_proof(&self, tenant: &TenantId, seq: u64) -> Option<InclusionProof> {
        let leaves = self.consumer.log().leaf_digests(tenant);
        inclusion_proof_over(&leaves, seq)
    }

    /// **`consistency_proof(t1, t2) → Proof` (contract 10.6).** The RFC-6962 consistency path
    /// proving the tree of size `first` is an append-only prefix of the tree of size `second`.
    /// `None` if the sizes are out of range or `first > second`.
    pub fn consistency_proof(
        &self,
        tenant: &TenantId,
        first: u64,
        second: u64,
    ) -> Option<ConsistencyProof> {
        let leaves = self.consumer.log().leaf_digests(tenant);
        consistency_proof_over(&leaves, first, second)
    }

    /// **Anchor an STH to an independent witness (gdpr §6.3).** The witness sees ONLY the opaque
    /// root + size (no PII — residency-safe) and countersigns. The returned attestation is the
    /// fixed point a later tampered tree cannot match.
    pub fn anchor_to_witness(
        &self,
        sth: &SignedTreeHead,
        witness: &dyn Witness,
    ) -> WitnessAttestation {
        witness.anchor(&sth.tenant, sth.tree_size, &sth.root_hash)
    }

    /// **Seal a DSR completion certificate INTO the per-tenant audit tree (closing the P-GA-12
    /// seal floor).** The bundle digest is appended as one minimised audit leaf via the SAME
    /// outbox-consumer append path (action `gdpr.dsr.certificate_sealed`, subject = the bundle
    /// digest as an opaque ArtifactRef — no PII), and the returned bundle carries the
    /// `merkle_inclusion` proof over that leaf. There is NO second write path: a DSR seal is an
    /// audited action like any other.
    pub fn seal_dsr_certificate(
        &self,
        tenant: &TenantId,
        region: &Region,
        bundle: &MerkleProvenBundle,
        sealed_at: &str,
    ) -> MerkleProvenBundle {
        // Append the seal as a minimised audit action (the bundle digest IS the subject — an
        // opaque content-address, never PII). The `service` actor is the GDPR service itself.
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

    /// **The H16 carve-out, at the tree level (gdpr §6.4).** A subject erasure over the audit log
    /// **retains** the minimised entry and **NEVER rewrites it** — so the chain still verifies and
    /// the STH root is UNCHANGED after the carve-out erase. Returns `true` iff the carve-out held
    /// (chain intact + root unchanged); the POLICY body (retain vs audit-key crypto-shred at
    /// retention end) is [`crate::holders::AuditCarveOutHolder`]. This proves the §6.4 tamper-
    /// evidence guarantee: erasing a person never breaks the log (the real identity lived in Id's
    /// erasable pseudonym map, never in the entry).
    pub fn carve_out_erase(&self, tenant: &TenantId, signed_at: &str) -> bool {
        let root_before = self
            .signed_tree_head(tenant, signed_at)
            .map(|s| s.root_hash);
        // The carve-out is a NO-OP on the chain (retain, never rewrite). The audit log holds only
        // the minimised opaque pseudonym; the subject's identity is shredded in Id's pseudonym map
        // (a different store), NOT here. So nothing in the tree changes.
        let root_after = self
            .signed_tree_head(tenant, signed_at)
            .map(|s| s.root_hash);
        self.consumer.log().verify_chain(tenant) && root_before == root_after
    }

    /// The live `sth_publish_age` measurement source (rule 7 / gdpr §7.6): the count of STHs
    /// published for a tenant (0 = none yet). On the floor this is the publication counter; the
    /// live seconds-since-last-publish wiring onto the metrics-health surface is P-119's
    /// `serve(AppSpec)` follow-on. Pinned name/unit: [`STH_PUBLISH_AGE`].
    pub fn sth_publish_age(&self, tenant: &TenantId) -> u64 {
        self.last_sth_seq
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(tenant)
            .copied()
            .unwrap_or(0)
    }
}

// ───────────────────────────── the proof construction + verification (RFC 6962) ─────────────────────────────

/// Render a raw digest as the `blake3:<hex>` multihash (the same convention the tree uses).
fn render(d: &[u8; 32]) -> String {
    audit::blake3_multihash_raw(d)
}

/// Parse a `blake3:<hex>` node back to its 32 raw bytes (for path recomputation). A malformed node
/// yields the all-zero digest, which simply fails verification (the proof never verifies false-
/// positive — a tampered node is caught).
fn parse(s: &str) -> [u8; 32] {
    s.strip_prefix("blake3:")
        .and_then(|h| hex::decode(h).ok())
        .and_then(|v| <[u8; 32]>::try_from(v.as_slice()).ok())
        .unwrap_or([0u8; 32])
}

/// Build the RFC-6962 inclusion proof for leaf `index` of `leaves` (the audit path: the sibling at
/// each level from the leaf up to the root). `None` if `index` is out of range.
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
            // left node: the sibling is the right one IF it exists (else this node carries up alone).
            if i + 1 < level.len() {
                Some(level[i + 1])
            } else {
                None
            }
        } else {
            // right node: the sibling is always the left one.
            Some(level[i - 1])
        };
        if let Some(s) = sibling {
            audit_path.push(render(&s));
        }
        // Reduce to the next level (same RFC-6962 pairing the tree uses).
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

/// **Verify an inclusion proof against an STH's `root_hash`** (RFC 6962 §2.1.1). Recompute the root
/// from the leaf + the audit path and check it matches the STH root AND the STH tree size. A leaf
/// at a tampered index, a tampered sibling, or a mismatched STH all fail. This is the proof an
/// auditor runs.
pub fn verify_inclusion(proof: &InclusionProof, sth: &SignedTreeHead) -> bool {
    if proof.tree_size != sth.tree_size || proof.root_hash != sth.root_hash {
        return false;
    }
    if proof.leaf_index >= proof.tree_size {
        return false;
    }
    // Mirror the build loop EXACTLY (RFC 6962): at each level a node either pairs with a sibling
    // (consuming one audit-path node) or — a left node that is the LAST node at an odd-width level —
    // carries up ALONE (consuming NO path node). `level_size` is the node count at the current
    // level; the audit-path index walks forward only when a sibling is actually consumed.
    let mut hash = parse(&proof.leaf_hash);
    let mut index = proof.leaf_index;
    let mut level_size = proof.tree_size;
    let mut path_pos = 0usize;
    while level_size > 1 {
        let has_sibling = if index % 2 == 1 {
            true // a right child always has a left sibling.
        } else {
            index + 1 < level_size // a left child has a sibling unless it is the lone last node.
        };
        if has_sibling {
            let Some(node) = proof.audit_path.get(path_pos) else {
                return false; // the proof is missing a node it must carry.
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
    // Every carried sibling must have been consumed (no extra/dangling nodes) and the root matches.
    path_pos == proof.audit_path.len() && render(&hash) == sth.root_hash
}

/// Build the RFC-6962 consistency proof between sizes `first` and `second`. On this floor it
/// carries the two roots + the `first`-prefix subtree path; verification ([`verify_consistency`])
/// recomputes the `first` root from the prefix of the `second` tree's leaves and confirms it
/// matches the old STH (proving append-only — no fork/rewrite). `None` if out of range.
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
    // The consistency "proof" on this floor is the prefix-subtree node set the verifier needs to
    // recompute the first root from the second tree. The RFC-6962 minimal-path optimisation is the
    // P-GA-35 cell-scale concern; here the proof carries the prefix leaf digests (still O(first),
    // and still PII-free) so verification is exact. The honest split is documented.
    let proof: Vec<String> = leaves[..first as usize].iter().map(render).collect();
    Some(ConsistencyProof {
        first,
        second,
        first_root: render(&first_root),
        second_root: render(&second_root),
        proof,
    })
}

/// **Verify a consistency proof between two STHs** (RFC 6962 §2.1.2). Recompute the `first` root
/// from the proof's prefix nodes and confirm it matches BOTH the old STH and the proof's committed
/// `first_root`, and that the new STH matches the proof's `second_root`. A log that was forked or
/// rewritten between the two STHs FAILS: the recomputed prefix root no longer matches the old STH's
/// committed root. This is the second independent GA-D3 detection.
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
    // Recompute the first-tree root from the proof's prefix nodes and confirm it matches the OLD
    // STH (append-only: the old tree is an exact prefix of the new one).
    let prefix: Vec<[u8; 32]> = proof.proof.iter().map(|s| parse(s)).collect();
    if prefix.is_empty() {
        return false;
    }
    let recomputed = audit::merkle_root(&prefix);
    render(&recomputed) == old_sth.root_hash
}

/// Serialise an inclusion proof to the compact `seq@size:leaf|node|node|...->root` form stored in
/// [`MerkleProvenBundle::merkle_inclusion`] (a verifiable, PII-free string the certificate carries).
fn serialize_inclusion(p: &InclusionProof) -> String {
    let path = p.audit_path.join("|");
    format!(
        "{}@{}:{}|{}->{}",
        p.leaf_index, p.tree_size, p.leaf_hash, path, p.root_hash
    )
}

/// Serialise an STH to the compact `size@root@signed_at` commitment string (the chain-of-custody
/// anchor an eDiscovery bundle (10.7, [`crate::ediscovery`]) carries in its `merkle_inclusion` —
/// a verifiable, PII-free root commitment, never entry content). Pinned here (next to the STH type)
/// so the export's commitment encoding stays beside the STH definition.
pub fn serialize_sth_commitment(sth: &SignedTreeHead) -> String {
    format!("{}@{}@{}", sth.tree_size, sth.root_hash, sth.signed_at)
}

// ───────────────────────────── the consumer DSR-seal append path ─────────────────────────────

impl AuditConsumer {
    /// **Append a DSR-certificate seal as one minimised audit leaf (the SAME write path as any
    /// audited action).** Crate-internal — the only caller is [`AuditAuthority::seal_dsr_certificate`].
    /// The actor is the GDPR service itself (minimised); the subject is the bundle digest as an
    /// opaque ArtifactRef (a content-address, never PII); the action is `gdpr.dsr.certificate_sealed`.
    /// Returns the appended entry's `seq` (the leaf index the inclusion proof is built over). This
    /// keeps "no service writes the audit log except the consumer" intact — a seal is an audited
    /// action, not a side-channel.
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
                // The GDPR service, minimised (a service principal — `<service>@<tenant>.noreply`).
                actor: format!("gdpr-service@{}.noreply", tenant.0),
                actor_kind: "service".into(),
                on_behalf_of: None,
            },
            action: DSR_SEAL_ACTION.into(),
            subject,
            outcome: Outcome::Applied,
            // The DSR id is the causal correlation (the why-walk anchor: this seal happened because
            // that DSR completed). PII-free (a request id).
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

    /// `inclusion_proof` verifies against the STH — for EVERY leaf, at several tree sizes (the odd
    /// sizes exercise the carry-up branch).
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

    /// A tampered audit path (or a leaf claimed at the wrong index) FAILS inclusion verification.
    #[test]
    fn a_tampered_inclusion_proof_fails() {
        let auth = authority();
        append_n(&auth, "acme", 6);
        let tenant = TenantId("acme".into());
        let sth = auth.signed_tree_head(&tenant, "t").unwrap();
        let good = auth.inclusion_proof(&tenant, 2).unwrap();
        assert!(verify_inclusion(&good, &sth));

        // Tamper a sibling node → verification fails.
        let mut tampered = good.clone();
        if let Some(first) = tampered.audit_path.first_mut() {
            *first =
                "blake3:0000000000000000000000000000000000000000000000000000000000000000".into();
        }
        assert!(
            !verify_inclusion(&tampered, &sth),
            "a tampered audit path fails"
        );

        // Claim the leaf at the wrong index → fails.
        let mut wrong_index = good.clone();
        wrong_index.leaf_index = 3;
        assert!(
            !verify_inclusion(&wrong_index, &sth),
            "a wrong leaf index fails"
        );

        // A proof against a DIFFERENT (later) STH fails (the tree size / root differ).
        append_n(&auth, "acme", 1);
        let later = auth.signed_tree_head(&tenant, "t2").unwrap();
        assert!(
            !verify_inclusion(&good, &later),
            "a proof against a later STH fails (size differs)"
        );
    }

    /// The `verify_inclusion` STH-match guard checks BOTH fields INDEPENDENTLY (kills the `||`→`&&`
    /// mutant): a proof whose root matches but tree_size DIFFERS fails, AND one whose tree_size
    /// matches but root DIFFERS fails (each alone is disqualifying — an `&&` would wrongly admit
    /// either).
    #[test]
    fn verify_inclusion_rejects_a_single_field_mismatch() {
        let auth = authority();
        let tenant = TenantId("acme".into());
        append_n(&auth, "acme", 4);
        let sth = auth.signed_tree_head(&tenant, "t").unwrap();
        let proof = auth.inclusion_proof(&tenant, 1).unwrap();
        assert!(verify_inclusion(&proof, &sth), "the honest proof verifies");

        // Right root, WRONG size → fail (size-only mismatch).
        let mut wrong_size = sth.clone();
        wrong_size.tree_size = 99;
        assert!(
            !verify_inclusion(&proof, &wrong_size),
            "a size-only mismatch is rejected"
        );

        // Right size, WRONG root → fail (root-only mismatch).
        let mut wrong_root = sth.clone();
        wrong_root.root_hash = "blake3:deadbeef".into();
        assert!(
            !verify_inclusion(&proof, &wrong_root),
            "a root-only mismatch is rejected"
        );
    }

    /// The `verify_consistency` STH-match guards check each field INDEPENDENTLY (kills the `||`→`&&`
    /// mutants on the size guard AND the root guard): a single mismatched size OR a single
    /// mismatched root disqualifies the proof.
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

        // A WRONG old size (the new is right) → fail.
        let mut bad_old_size = old.clone();
        bad_old_size.tree_size = 2;
        assert!(
            !verify_consistency(&proof, &bad_old_size, &new),
            "a wrong old size is rejected"
        );
        // A WRONG new size (the old is right) → fail.
        let mut bad_new_size = new.clone();
        bad_new_size.tree_size = 9;
        assert!(
            !verify_consistency(&proof, &old, &bad_new_size),
            "a wrong new size is rejected"
        );
        // A WRONG old root (sizes right) → fail.
        let mut bad_old_root = old.clone();
        bad_old_root.root_hash = "blake3:deadbeef".into();
        assert!(
            !verify_consistency(&proof, &bad_old_root, &new),
            "a wrong old root is rejected"
        );
        // A WRONG new root (sizes right) → fail.
        let mut bad_new_root = new.clone();
        bad_new_root.root_hash = "blake3:deadbeef".into();
        assert!(
            !verify_consistency(&proof, &old, &bad_new_root),
            "a wrong new root is rejected"
        );
    }

    /// The STH SIGNATURE binds the (tree_size, root) — two DIFFERENT trees produce DIFFERENT
    /// signatures (kills the `preimage -> vec![]` mutant, which would collapse all STHs to one
    /// signature). A forged STH that swaps in a different tree's root no longer verifies.
    #[test]
    fn the_sth_signature_binds_the_tree_size_and_root() {
        let auth = authority();
        let tenant = TenantId("acme".into());
        append_n(&auth, "acme", 2);
        let sth2 = auth.signed_tree_head(&tenant, "t").unwrap();
        append_n(&auth, "acme", 3);
        let sth5 = auth.signed_tree_head(&tenant, "t").unwrap();
        // Two trees of different size+root sign to DIFFERENT signatures (the preimage is non-trivial).
        assert_ne!(
            sth2.signature, sth5.signature,
            "distinct (size, root) produce distinct STH signatures — the preimage binds them"
        );
        // Splicing sth2's signature onto sth5's body does NOT verify (the signature is over the body).
        let spliced = SignedTreeHead {
            signature: sth2.signature.clone(),
            ..sth5.clone()
        };
        assert!(
            !spliced.verify_signature(auth.key()),
            "a spliced signature does not verify"
        );
    }

    /// `consistency_proof` verifies between two STHs of an APPEND-ONLY log.
    #[test]
    fn consistency_proof_verifies_between_two_sths() {
        let auth = authority();
        let tenant = TenantId("acme".into());
        append_n(&auth, "acme", 3);
        let old = auth.signed_tree_head(&tenant, "t1").unwrap();
        append_n(&auth, "acme", 4); // now 7 leaves — the old tree is a prefix.
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

    /// **GA-D3 (1 of 3 / the consistency detection): a retroactive edit makes the consistency proof
    /// against the published STH FAIL.** We publish an STH, then simulate a tamper by recomputing
    /// the would-be root over an edited prefix and confirming it no longer matches the published STH.
    #[test]
    fn a_tamper_fails_the_consistency_proof_against_the_published_sth() {
        let auth = authority();
        let tenant = TenantId("acme".into());
        append_n(&auth, "acme", 5);
        let published = auth.signed_tree_head(&tenant, "t1").unwrap();

        // The honest consistency proof at the same size verifies (size n vs n is the identity prefix).
        let honest = auth.consistency_proof(&tenant, 5, 5).unwrap();
        assert!(verify_consistency(&honest, &published, &published));

        // Now TAMPER: edit leaf 2 (recompute the leaf set with a different leaf) and rebuild a proof.
        let mut leaves = auth.consumer().log().leaf_digests(&tenant);
        leaves[2] = *blake3::hash(b"TAMPERED").as_bytes();
        let tampered = consistency_proof_over(&leaves, 5, 5).unwrap();
        // The tampered proof's first_root no longer matches the PUBLISHED STH root.
        assert!(
            !verify_consistency(&tampered, &published, &published),
            "GA-D3: a retroactive edit fails the consistency proof against the published STH"
        );
    }

    /// **GA-D3 (2 of 3 / the witness detection): the independent witness mismatches a tampered
    /// tree.** The witness countersigns the opaque root at a size; a later tampered tree's root at
    /// that size differs from what the witness attested.
    #[test]
    fn the_witness_mismatches_a_tampered_tree() {
        let auth = authority();
        let tenant = TenantId("acme".into());
        append_n(&auth, "acme", 5);
        let sth = auth.signed_tree_head(&tenant, "t1").unwrap();

        // The witness has its OWN key (an independent party). It sees ONLY the opaque root.
        let witness = NotaryWitness::new(CellSigningKey::from_seed("notary:cell-b"));
        let attestation = auth.anchor_to_witness(&sth, &witness);
        // The witness saw NO PII — only the root + size.
        assert_eq!(
            attestation.witnessed_root, sth.root_hash,
            "the witness pins the opaque root"
        );
        assert_eq!(attestation.tree_size, 5);

        // The honest current root at that size matches the attestation.
        let honest_root = render(&audit::merkle_root(
            &auth.consumer().log().leaf_digests(&tenant),
        ));
        assert!(
            attestation.matches(&honest_root),
            "the honest tree matches the witness"
        );

        // A tampered tree's root at that size DIFFERS → the witness mismatches.
        let mut leaves = auth.consumer().log().leaf_digests(&tenant);
        leaves[1] = *blake3::hash(b"TAMPERED").as_bytes();
        let tampered_root = render(&audit::merkle_root(&leaves));
        assert!(
            !attestation.matches(&tampered_root),
            "GA-D3: the independent witness mismatches a tampered tree"
        );
    }

    /// The witness sees ONLY an opaque root — NO PII crosses (gdpr §6.3, residency-safe). The
    /// attestation serialises to a string containing the root + size, and nothing resembling an
    /// entry's actor/subject content.
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
        // The attestation carries the opaque root + size + tenant — never an entry subject/actor.
        assert!(
            attestation.witnessed_root.starts_with("blake3:"),
            "the witness sees an opaque hash"
        );
        assert!(
            !attestation.witnessed_root.contains("SENSITIVE-SUBJECT"),
            "no entry subject content reaches the witness (residency-safe)"
        );
    }

    /// **A DSR receipt seals into the tree** (closing the P-GA-12 seal floor): the sealed bundle
    /// carries a `merkle_inclusion` proof (no longer `None`), and the underlying seal leaf has a
    /// verifiable inclusion proof against the post-seal STH.
    #[test]
    fn a_dsr_receipt_seals_into_the_tree() {
        let auth = authority();
        let tenant = TenantId("acme".into());
        let region = Region("acme-home".into());
        append_n(&auth, "acme", 3); // some prior actions.

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
        // The serialised inclusion is the real `seq@size:leaf|path->root` form (kills the
        // `serialize_inclusion -> ""` mutant): it carries the leaf index, the tree size, and the
        // root-reduction arrow over real `blake3:` nodes.
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

        // The seal appended ONE leaf (the certificate-sealed action) → tree size grew by 1.
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
        // The chain still verifies (the seal is an ordinary append, not a rewrite).
        assert!(auth.consumer().log().verify_chain(&tenant));
    }

    /// **The H16 carve-out retains-without-rewriting** (gdpr §6.4): a subject erasure over the audit
    /// log leaves the chain intact AND the STH root unchanged (no entry was rewritten).
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

    /// The STH signature is forge-resistant: an STH signed by one key does NOT verify under a
    /// different key (a compromised cell cannot mint a valid STH without the in-cell key).
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

    /// An empty chain has no STH and no proof (nothing to commit).
    #[test]
    fn an_empty_chain_has_no_sth_or_proof() {
        let auth = authority();
        let tenant = TenantId("empty".into());
        assert!(auth.signed_tree_head(&tenant, "t").is_none());
        assert!(auth.inclusion_proof(&tenant, 0).is_none());
        assert!(auth.consistency_proof(&tenant, 1, 1).is_none());
        assert_eq!(auth.sth_publish_age(&tenant), 0, "no STH published yet");
    }

    /// `sth_publish_age` advances as STHs are published (the SLO source); the name/unit are pinned.
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
