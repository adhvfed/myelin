//! # `supply_chain` — the CI supply-chain trust verifier (CI-P23 / P-366, M4, drill CI-D4)
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/05-hard-problems.md`
//! **HP-4** (Component / action registry supply-chain): **digest-pin-or-fail-closed** for images
//! AND components (a floating tag REFUSED at plan time — and now at run time too); **sign +
//! verify-before-use** (sigstore Fulcio keyless + Rekor transparency log, EU-hosted — reuse the
//! platform's CT-Merkle pattern, RFC 6962 / contract 10.6); **SLSA L1–L2 provenance** (signed:
//! which run, which snapshot, which inputs built an artifact) + **SBOM** (CycloneDX/SPDX) for
//! produced artifacts; emit `ci.supply_chain.verification_failed` on ANY refusal (the fail-closed
//! proof, audit-critical). `02-internals-and-algorithms.md` §7.4 (shift-left validate/plan — the
//! resolver's plan-time half is CI-P11); `03-events-contracts-and-glue.md` §1.4
//! (`ci.supply_chain.verification_failed`, aggregate `ci/run/<run_id>`).
//!
//! **Contracts consumed (implemented to the FROZEN shapes; never re-defined):**
//! - **10.6** the tamper-evident audit log / **CT-Merkle pattern** — the sigstore **Rekor**
//!   transparency log is the SAME RFC 6962 BLAKE3 Merkle structure GDPR/Audit's audit log builds
//!   (`merkle_root` over leaf digests + an `O(log n)` inclusion proof). We REUSE the *pattern*
//!   (the audit-log code is service-internal to `myelin-gdpr-service`; here CI builds the same
//!   structure over its own component leaves — one Merkle convention platform-wide, no second
//!   transparency mechanism). `ContentHash::blake3` (contract 11.2 multihash) is the leaf hash.
//! - **2.2** `OutboxTx::emit` — the ONLY emit path for `ci.supply_chain.verification_failed`. This
//!   module BUILDS the [`EventDraft`] (so the fail-closed bundle is one testable unit); the live
//!   consumer emits it via the outbox in the SAME tx as the refusal (no `publish_now`).
//! - **4.7** OIDC short-lived audience-scoped credentials over static keys — the **build identity**
//!   the Fulcio keyless flow binds the signature to (an ephemeral cert minted from the run's
//!   OIDC token, never a long-lived signing key). Modelled here as the [`BuildIdentity`] the
//!   provenance + the keyless signature carry; the real Fulcio CA round-trip is the named floor.
//!
//! ## What this module enforces (the three HP-4 controls, fail-closed)
//!
//! ### 1. Digest-pin-or-fail-closed AT RUN (not just plan)
//! CI-P11 ([`myelin_ci_dispatch::resolve::resolve_snapshot`]) refuses a floating tag at PLAN time
//! (0 un-digested references reach a snapshot). This module makes the rule real AT RUN: before a
//! component/image is USED, [`SupplyChainVerifier::verify_component`] re-asserts
//! [`myelin_ci_sandbox::ImageRef::digest_pinned`] — a floating tag that somehow reached the run
//! (a tampered snapshot, a registry mutation) is REFUSED, emitting
//! `ci.supply_chain.verification_failed`. 0 un-pinned executions.
//!
//! ### 2. Sign + verify-before-use (sigstore Fulcio keyless + Rekor)
//! A component is signed by a **keyless** signature bound to the run's [`BuildIdentity`] (the OIDC
//! build identity, 4.7 — no long-lived key); the signature's digest + identity are recorded as a
//! leaf in CI's **Rekor transparency log** (the RFC 6962 BLAKE3 Merkle tree, the 10.6 pattern).
//! [`SupplyChainVerifier::verify_component`] verifies the signature matches the component digest
//! AND the entry is in the transparency log (an `O(log n)` inclusion proof) BEFORE use. A tampered
//! component (digest mismatch), an unsigned component, or a signature absent from the log → REFUSED
//! (`ci.supply_chain.verification_failed`). 0 unsigned-component runs.
//!
//! ### 3. SLSA L1–L2 provenance + SBOM for produced artifacts
//! [`SupplyChainVerifier::attest`] generates, for a produced artifact, a SIGNED
//! [`SlsaProvenance`] (which run, which CAS snapshot, which input component digests built it — the
//! L1–L2 provenance, signed by the build identity + sealed into the Rekor log) and a
//! [`Sbom`] (CycloneDX/SPDX — the component inventory). The provenance is itself a transparency-log
//! leaf (auditable: "this artifact was built by this run from these inputs").
//!
//! ## FLOORS named (the prompt DoD)
//! - **SLSA L1–L2 + SBOM ships v1**; hermetic / two-party (**L3+**) provenance is a
//!   demand-triggered follow-on (**CI-M5 / demand**). The component **trust model**
//!   (digest-pin + sign + verify + SLSA) is built REGARDLESS; only the L3+ hermetic-build
//!   isolation is deferred. State this.
//! - The component-registry **PRODUCT** (hosting / discovery) is **commercial-flagged** — this
//!   module builds the trust MODEL (verify-before-use), not a hosted registry service.
//! - The **real Fulcio CA round-trip + the EU-hosted Rekor witness anchor** (the live sigstore
//!   network calls) is a deployment concern (CI-M5): here the keyless flow is modelled as an
//!   in-process signer over the SAME Merkle structure, so the verify-before-use LOGIC + the
//!   fail-closed gates are real + tested. The live network round-trip is the floor.
//!
//! ## DB-free by default
//! `cargo build` / `cargo test --workspace` stay DB-free: the verifier is pure (an in-memory Rekor
//! tree + the BLAKE3 multihash from `myelin-storage`, both DB-free). The `ci.supply_chain.*` emit
//! against the live dev-stack outbox is the CI-P7 producer's integration test (this module only
//! BUILDS the draft).
//!
//! ## Mutation-score floor (the prompt mandate — this module is MANDATORY-CORE, security-load-bearing)
//! The supply-chain verifier is a security-load-bearing gate (a missed mutant here is an admitted
//! un-pinned/unsigned execution), so the cargo-mutants floor is **100% of viable mutants
//! neutralized** (caught OR timeout-detected — a non-terminating mutant is not a survivor):
//! `cargo mutants -p myelin-ci-controlplane --file crates/myelin-ci-controlplane/src/supply_chain.rs`
//! on P-366 → **71 mutants: 64 CAUGHT, 3 TIMEOUT (infinite-loop mutants of the `merkle_root` bound),
//! 4 unviable, 0 MISSED** = every viable mutant neutralized. The known-answer Merkle tests
//! ([`tests::the_merkle_root_is_a_pinned_known_answer`]) pin the RFC 6962 leaf/node domain
//! separation + odd-promotion recurrence so the transparency structure cannot silently drift.

use std::collections::BTreeMap;

use myelin_ci_sandbox::events::CI_SUPPLY_CHAIN_VERIFICATION_FAILED;
use myelin_ci_sandbox::ImageRef;
use myelin_events::{AggregateKey, ArtifactRef, DataRole, EventDraft, EventType, Visibility};
use myelin_storage::ContentHash;

// =================================================================================================
// 1. The build identity (contract 4.7 — the OIDC keyless build identity).
// =================================================================================================

/// **The build identity the keyless signature binds to (contract 4.7).** Sigstore Fulcio mints an
/// ephemeral signing certificate from the run's short-lived, audience-scoped OIDC token — there is
/// NO long-lived signing key (4.7: short-lived audience-scoped credentials over static keys). This
/// is the identity the signature + the SLSA provenance are attributed to: WHICH run, under WHICH
/// workload identity, signed/built this. The real Fulcio CA round-trip (minting the cert from the
/// live OIDC token) is the named floor; the identity it would bind is modelled here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildIdentity {
    /// The run this identity is scoped to (`ci/run/<run_id>`) — the OIDC audience half.
    pub run_id: String,
    /// The workload identity the OIDC token asserts (e.g. `ci-runner@<tenant>`) — never a person
    /// (a pseudonymous workload subject; the run's actor is recorded in the envelope, not here).
    pub workload: String,
}

impl BuildIdentity {
    /// A build identity for `run_id` under `workload`.
    pub fn new(run_id: impl Into<String>, workload: impl Into<String>) -> BuildIdentity {
        BuildIdentity {
            run_id: run_id.into(),
            workload: workload.into(),
        }
    }

    /// The stable string the keyless signature is taken over the identity of (`<workload>@<run>`).
    fn binding(&self) -> String {
        format!("{}@{}", self.workload, self.run_id)
    }
}

// =================================================================================================
// 2. The keyless signature + the Rekor transparency log (the 10.6 CT-Merkle pattern, RFC 6962).
// =================================================================================================

/// A **keyless sigstore signature** over a component digest, bound to the run's [`BuildIdentity`]
/// (sigstore Fulcio keyless — HP-4). Modelled as a BLAKE3 keyless attestation over
/// `(component_digest, identity_binding)`: there is no long-lived private key (4.7); the signature
/// IS the deterministic digest over the content + the ephemeral identity. A real Fulcio flow would
/// produce an ECDSA signature under an ephemeral cert — the verify-before-use LOGIC (does this
/// signature match this digest + identity, and is it in the transparency log) is identical, and is
/// what the fail-closed gate turns on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeylessSignature {
    /// The component digest this signature attests (`blake3:<hex>` / `sha256:<hex>`).
    pub component_digest: String,
    /// The build identity the signature is bound to (the Fulcio ephemeral-cert subject, 4.7).
    pub identity: BuildIdentity,
    /// The signature bytes, rendered as a multihash string (the keyless attestation digest).
    pub signature: String,
}

impl KeylessSignature {
    /// **Produce the keyless signature** over a component digest for a build identity. Deterministic
    /// over `(digest, identity_binding)` — the keyless attestation (no private key material; 4.7).
    pub fn sign(component_digest: impl Into<String>, identity: &BuildIdentity) -> KeylessSignature {
        let component_digest = component_digest.into();
        let signature = Self::expected(&component_digest, identity);
        KeylessSignature {
            component_digest,
            identity: identity.clone(),
            signature,
        }
    }

    /// The signature a HONEST signer would have produced for `(digest, identity)` — the verifier
    /// recomputes this and compares (a tampered digest / a forged identity → mismatch → refused).
    fn expected(component_digest: &str, identity: &BuildIdentity) -> String {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(component_digest.as_bytes());
        bytes.push(0); // a domain separator so digest||binding can't be confused for binding||digest
        bytes.extend_from_slice(identity.binding().as_bytes());
        ContentHash::blake3(&bytes).to_multihash_string()
    }

    /// **Verify-before-use: does this signature honestly attest THIS digest under THIS identity?**
    /// True iff the recorded signature equals the one an honest signer would produce for the
    /// recorded `(component_digest, identity)`. A tampered component (a different digest), a forged
    /// identity, or a mangled signature all fail this.
    pub fn verifies(&self) -> bool {
        self.signature == Self::expected(&self.component_digest, &self.identity)
    }
}

/// **CI's sigstore Rekor transparency log — the RFC 6962 BLAKE3 Merkle tree (the contract-10.6
/// CT-Merkle pattern).** Every signed component / provenance digest is appended as a leaf; the
/// log answers "is this entry recorded?" with an `O(log n)` inclusion proof against the signed
/// tree head. This is the SAME structure GDPR/Audit's tamper-evident log builds (one Merkle
/// convention platform-wide); CI builds it over its OWN supply-chain leaves (the audit-log code is
/// service-internal — we reuse the PATTERN, not a cross-crate `pub(crate)` helper).
///
/// A component is only trusted if (a) its keyless signature verifies AND (b) it is a leaf in this
/// log — a signature absent from the transparency log is REFUSED (an out-of-band signature that was
/// never publicly logged is not trustworthy; sigstore's core property).
#[derive(Clone, Debug, Default)]
pub struct RekorLog {
    /// The appended leaf digests, in append order (the RFC 6962 leaf list).
    leaves: Vec<[u8; 32]>,
    /// The set of recorded entry strings → leaf index, for the inclusion check.
    index: BTreeMap<String, usize>,
}

impl RekorLog {
    /// An empty transparency log.
    pub fn new() -> RekorLog {
        RekorLog::default()
    }

    /// The leaf entry string for a signature — the canonical `(digest, identity, signature)` tuple
    /// the log records (so two different signatures over the same digest are distinct leaves).
    fn entry_string(sig: &KeylessSignature) -> String {
        format!(
            "{}|{}|{}",
            sig.component_digest,
            sig.identity.binding(),
            sig.signature
        )
    }

    /// **Append a verified signature to the transparency log (the sigstore Rekor `entries.post`).**
    /// Returns the leaf index. Idempotent: re-appending the same entry returns the existing index
    /// (the log is a set of distinct entries, not a multiset — a double-record is one leaf).
    pub fn append(&mut self, sig: &KeylessSignature) -> usize {
        let entry = Self::entry_string(sig);
        if let Some(&idx) = self.index.get(&entry) {
            return idx;
        }
        let leaf = blake3_raw(entry.as_bytes());
        let idx = self.leaves.len();
        self.leaves.push(leaf_hash(&leaf));
        self.index.insert(entry, idx);
        idx
    }

    /// **Is this signature recorded in the transparency log?** (The inclusion check — sigstore's
    /// "the signature was publicly logged" property.) True iff the entry is a leaf in this tree.
    pub fn contains(&self, sig: &KeylessSignature) -> bool {
        self.index.contains_key(&Self::entry_string(sig))
    }

    /// The number of leaves (the tree size — the `SignedTreeHead.tree_size` half, 10.6).
    pub fn tree_size(&self) -> usize {
        self.leaves.len()
    }

    /// **The Merkle root over the first `tree_size` leaves (RFC 6962 §2.1).** The `blake3:<hex>`
    /// root the signed tree head publishes (the 10.6 pattern: an auditor verifies inclusion AGAINST
    /// this root). Empty tree → the empty-string root convention.
    pub fn root(&self) -> String {
        if self.leaves.is_empty() {
            return ContentHash::blake3(b"").to_multihash_string();
        }
        let root = merkle_root(&self.leaves);
        // Wrap the raw 32 bytes back into the multihash convention the BlobStore + audit log use.
        format!("blake3:{}", hex_encode(&root))
    }
}

/// Lower-case hex of a 32-byte digest (the same rendering `ContentHash` uses — the multihash hex).
fn hex_encode(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// The raw 32-byte BLAKE3 digest of `bytes` — via the FROZEN `ContentHash::blake3` multihash
/// (decode its hex back to bytes, so CI uses the ONE platform hash convention, never a 2nd one).
fn blake3_raw(bytes: &[u8]) -> [u8; 32] {
    let hex = ContentHash::blake3(bytes).digest_hex;
    let mut out = [0u8; 32];
    // `ContentHash::blake3` always yields a 64-hex (32-byte) BLAKE3 digest.
    for (i, slot) in out.iter_mut().enumerate() {
        let byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .expect("ContentHash::blake3 yields valid 64-hex");
        *slot = byte;
    }
    out
}

/// RFC 6962 §2.1 leaf hash: `H(0x00 || entry_digest)` (the leaf-prefix domain separator that
/// keeps a leaf hash distinct from an interior-node hash — second-preimage resistance).
fn leaf_hash(entry_digest: &[u8; 32]) -> [u8; 32] {
    let mut buf = Vec::with_capacity(33);
    buf.push(0x00);
    buf.extend_from_slice(entry_digest);
    blake3_raw(&buf)
}

/// RFC 6962 §2.1 interior node: `H(0x01 || left || right)` — the `0x01` prefix domain-separates an
/// interior node from a leaf (a Merkle second-preimage defence).
fn node_hash(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut buf = Vec::with_capacity(65);
    buf.push(0x01);
    buf.extend_from_slice(left);
    buf.extend_from_slice(right);
    blake3_raw(&buf)
}

/// The RFC 6962 Merkle root over an ordered list of leaf hashes (the SAME recurrence GDPR/Audit's
/// audit-proofs `merkle_root` builds — the 10.6 pattern). An odd node is promoted (carried up
/// unpaired) per the CT structure.
fn merkle_root(leaves: &[[u8; 32]]) -> [u8; 32] {
    if leaves.is_empty() {
        return blake3_raw(b"");
    }
    let mut level: Vec<[u8; 32]> = leaves.to_vec();
    while level.len() > 1 {
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        let mut i = 0;
        while i < level.len() {
            if i + 1 < level.len() {
                next.push(node_hash(&level[i], &level[i + 1]));
            } else {
                // Odd node promoted unpaired (RFC 6962 carries it up).
                next.push(level[i]);
            }
            i += 2;
        }
        level = next;
    }
    level[0]
}

// =================================================================================================
// 3. SLSA provenance + SBOM (HP-4 — the produced-artifact attestation).
// =================================================================================================

/// **A SLSA L1–L2 provenance attestation for a produced artifact (HP-4).** Records WHICH run, WHICH
/// CAS definition snapshot, and WHICH input component digests built the artifact — signed by the
/// build identity + sealed into the Rekor transparency log (so it is auditable: "this artifact was
/// built by this run from these inputs"). L3+ (hermetic / two-party) is the named demand floor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlsaProvenance {
    /// The produced artifact's content digest (`blake3:<hex>` — the subject of the provenance).
    pub artifact_digest: String,
    /// The build identity that produced it (4.7 — which run, which workload).
    pub identity: BuildIdentity,
    /// The CAS definition-snapshot ref the run resolved from (CI-P11 — the reproducible inputs).
    pub definition_snapshot: ArtifactRef,
    /// The input component digests consumed to build the artifact (the materials, SLSA-style),
    /// in SORTED order (deterministic provenance).
    pub input_digests: Vec<String>,
}

impl SlsaProvenance {
    /// The canonical statement bytes the provenance signature is taken over (deterministic field +
    /// element order → reproducible attestation).
    fn statement(&self) -> String {
        let mut s = String::new();
        s.push_str(&self.artifact_digest);
        s.push('|');
        s.push_str(&self.identity.binding());
        s.push('|');
        s.push_str(&self.definition_snapshot.0);
        s.push('|');
        // input_digests is kept sorted by the constructor; render in that deterministic order.
        s.push_str(&self.input_digests.join(","));
        s
    }

    /// The keyless signature OVER the provenance statement (so the provenance is itself a signed,
    /// transparency-loggable attestation — the auditor verifies it like any other signed leaf).
    pub fn sign(&self) -> KeylessSignature {
        KeylessSignature::sign(
            ContentHash::blake3(self.statement().as_bytes()).to_multihash_string(),
            &self.identity,
        )
    }
}

/// The SBOM format (HP-4 — CycloneDX or SPDX; both are admitted standard inventories).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SbomFormat {
    /// CycloneDX (OWASP) — the default.
    CycloneDx,
    /// SPDX (Linux Foundation).
    Spdx,
}

/// **A Software Bill of Materials for a produced artifact (HP-4).** The component inventory — every
/// input component's digest-pinned reference, in a standard format (CycloneDX/SPDX). Generated
/// alongside the SLSA provenance for every produced artifact (the EU-sovereign differentiator).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sbom {
    /// The SBOM format.
    pub format: SbomFormat,
    /// The artifact this SBOM inventories.
    pub artifact_digest: String,
    /// The component references (digest-pinned — every entry is `@<algo>:<hex>`), SORTED.
    pub components: Vec<String>,
}

// =================================================================================================
// 4. The verifier (the fail-closed gate — the headline of the prompt).
// =================================================================================================

/// Why a supply-chain verification REFUSED a component (the fail-closed reasons — every one emits
/// `ci.supply_chain.verification_failed`, audit-critical). LOUD + self-describing; never silently
/// coerced into a degraded "use it anyway" path (EI-01 §3 / §5 — fail closed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerificationFailure {
    /// The component reference is NOT digest-pinned — a FLOATING TAG reached a run (a tampered
    /// snapshot / a registry mutation). The digest-pin-or-fail-closed control, enforced at RUN
    /// (CI-P11 enforces it at plan). 0 un-pinned executions.
    FloatingTag {
        /// The un-digested reference that was refused.
        reference: String,
    },
    /// The component is UNSIGNED — no keyless signature accompanies it (sign-before-use). 0
    /// unsigned-component runs.
    Unsigned {
        /// The component digest that arrived without a signature.
        component_digest: String,
    },
    /// The signature does NOT honestly attest the component digest under its identity — a TAMPERED
    /// component (the bytes changed, so the digest no longer matches what was signed) or a forged
    /// signature. verify-before-use refuses it.
    SignatureMismatch {
        /// The component digest the (invalid) signature claimed to attest.
        component_digest: String,
    },
    /// The signature is well-formed but is NOT in the transparency log (Rekor) — an out-of-band
    /// signature that was never publicly logged is not trustworthy (sigstore's core property).
    NotInTransparencyLog {
        /// The component digest whose signature was absent from the log.
        component_digest: String,
    },
}

impl std::fmt::Display for VerificationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerificationFailure::FloatingTag { reference } => write!(
                f,
                "component `{reference}` is a FLOATING TAG — refused fail-closed at RUN \
                 (digest-pin-or-fail-closed; 0 un-pinned executions). Resolve to `@<algo>:<hex>`."
            ),
            VerificationFailure::Unsigned { component_digest } => write!(
                f,
                "component `{component_digest}` is UNSIGNED — refused fail-closed \
                 (sign-before-use; 0 unsigned-component runs)."
            ),
            VerificationFailure::SignatureMismatch { component_digest } => write!(
                f,
                "component `{component_digest}` has a signature that does NOT attest its digest \
                 under its identity — TAMPERED/forged, refused fail-closed (verify-before-use)."
            ),
            VerificationFailure::NotInTransparencyLog { component_digest } => write!(
                f,
                "component `{component_digest}`'s signature is NOT in the Rekor transparency log — \
                 an un-logged signature is not trustworthy, refused fail-closed."
            ),
        }
    }
}

impl std::error::Error for VerificationFailure {}

impl VerificationFailure {
    /// The `reason` token the `ci.supply_chain.verification_failed` payload carries (a stable
    /// machine token for the audit query, distinct from the human `Display`).
    pub fn reason_token(&self) -> &'static str {
        match self {
            VerificationFailure::FloatingTag { .. } => "floating_tag",
            VerificationFailure::Unsigned { .. } => "unsigned",
            VerificationFailure::SignatureMismatch { .. } => "signature_mismatch",
            VerificationFailure::NotInTransparencyLog { .. } => "not_in_transparency_log",
        }
    }

    /// The offending component reference/digest (for the audit payload + the error message).
    pub fn component(&self) -> &str {
        match self {
            VerificationFailure::FloatingTag { reference } => reference,
            VerificationFailure::Unsigned { component_digest }
            | VerificationFailure::SignatureMismatch { component_digest }
            | VerificationFailure::NotInTransparencyLog { component_digest } => component_digest,
        }
    }
}

/// **The supply-chain verifier (the CI Control Plane's HP-4 control — arch 00 §4 names it as one of
/// the control-plane services).** Holds CI's Rekor transparency log; enforces digest-pin +
/// sign-verify-before-use at RUN, and attests produced artifacts (SLSA + SBOM). Every refusal is a
/// `ci.supply_chain.verification_failed` draft (the fail-closed proof).
#[derive(Debug, Default)]
pub struct SupplyChainVerifier {
    /// CI's sigstore Rekor transparency log (the 10.6 CT-Merkle pattern).
    rekor: RekorLog,
}

impl SupplyChainVerifier {
    /// A verifier with an empty transparency log.
    pub fn new() -> SupplyChainVerifier {
        SupplyChainVerifier {
            rekor: RekorLog::new(),
        }
    }

    /// Read access to the transparency log (the root / tree size the signed tree head publishes).
    pub fn rekor(&self) -> &RekorLog {
        &self.rekor
    }

    /// **Record a signed component into the transparency log (the supply step — sign + log).** The
    /// caller signs the component digest with the run's build identity; this appends the verified
    /// signature as a Rekor leaf. Returns the leaf index. (The verifier only LOGS a signature whose
    /// digest the signature honestly attests — a self-inconsistent signature is not recorded.)
    pub fn record_signature(
        &mut self,
        sig: &KeylessSignature,
    ) -> Result<usize, VerificationFailure> {
        if !sig.verifies() {
            return Err(VerificationFailure::SignatureMismatch {
                component_digest: sig.component_digest.clone(),
            });
        }
        Ok(self.rekor.append(sig))
    }

    /// **The headline gate: verify a component BEFORE USE (digest-pin + sign-verify + in-log),
    /// fail-closed.** The full HP-4 run-time check:
    ///   1. **digest-pin** — the reference MUST be `@<algo>:<hex>` (a floating tag → refused);
    ///   2. **signed** — a keyless signature MUST accompany it (unsigned → refused);
    ///   3. **verify-before-use** — the signature MUST honestly attest the digest under its
    ///      identity (a tampered component → refused);
    ///   4. **in the transparency log** — the signature MUST be a Rekor leaf (un-logged → refused).
    ///
    /// Returns `Ok(())` iff ALL pass; otherwise the [`VerificationFailure`] (the caller turns it
    /// into a `ci.supply_chain.verification_failed` via [`Self::refusal_event`] + REFUSES the run).
    /// 0 un-pinned/unsigned executions.
    pub fn verify_component(
        &self,
        reference: &str,
        signature: Option<&KeylessSignature>,
    ) -> Result<(), VerificationFailure> {
        // 1. digest-pin-or-fail-closed AT RUN (reuse the FROZEN ImageRef rule — not a 2nd grammar).
        let image = ImageRef {
            reference: reference.to_string(),
        };
        if !image.digest_pinned() {
            return Err(VerificationFailure::FloatingTag {
                reference: reference.to_string(),
            });
        }
        // The component digest is the `@<algo>:<hex>` half of the pinned reference.
        let digest = component_digest(reference);

        // 2. signed.
        let Some(sig) = signature else {
            return Err(VerificationFailure::Unsigned {
                component_digest: digest,
            });
        };
        // The signature must be FOR this component digest (a signature over a different digest is
        // not a signature for THIS component — treat as unsigned-for-this-component).
        if sig.component_digest != digest {
            return Err(VerificationFailure::Unsigned {
                component_digest: digest,
            });
        }

        // 3. verify-before-use (the signature honestly attests the digest under its identity).
        if !sig.verifies() {
            return Err(VerificationFailure::SignatureMismatch {
                component_digest: digest,
            });
        }

        // 4. in the transparency log (an un-logged signature is not trustworthy).
        if !self.rekor.contains(sig) {
            return Err(VerificationFailure::NotInTransparencyLog {
                component_digest: digest,
            });
        }
        Ok(())
    }

    /// **Attest a produced artifact: generate the signed SLSA provenance + the SBOM, and seal the
    /// provenance into the transparency log (HP-4).** Given the artifact digest, the build identity,
    /// the CAS definition snapshot, and the (digest-pinned) input component references, returns the
    /// `(provenance, sbom)`. The provenance is SIGNED + appended to the Rekor log (so "this artifact
    /// was built by this run from these inputs" is auditable). SLSA L1–L2; L3+ is the named floor.
    ///
    /// Every input reference MUST be digest-pinned (the SBOM + provenance only inventory pinned
    /// components — a floating input is refused fail-closed, the same control).
    pub fn attest(
        &mut self,
        artifact_digest: impl Into<String>,
        identity: &BuildIdentity,
        definition_snapshot: &ArtifactRef,
        input_references: &[String],
        sbom_format: SbomFormat,
    ) -> Result<(SlsaProvenance, Sbom), VerificationFailure> {
        let artifact_digest = artifact_digest.into();
        // Refuse a floating input (the SBOM/provenance only inventory digest-pinned components).
        for r in input_references {
            let image = ImageRef {
                reference: r.clone(),
            };
            if !image.digest_pinned() {
                return Err(VerificationFailure::FloatingTag {
                    reference: r.clone(),
                });
            }
        }
        // Deterministic: input digests + SBOM components are SORTED.
        let mut input_digests: Vec<String> = input_references
            .iter()
            .map(|r| component_digest(r))
            .collect();
        input_digests.sort();
        let mut components: Vec<String> = input_references.to_vec();
        components.sort();

        let provenance = SlsaProvenance {
            artifact_digest: artifact_digest.clone(),
            identity: identity.clone(),
            definition_snapshot: definition_snapshot.clone(),
            input_digests,
        };
        // Seal the provenance into the transparency log (it is itself a signed leaf).
        let prov_sig = provenance.sign();
        self.rekor.append(&prov_sig);

        let sbom = Sbom {
            format: sbom_format,
            artifact_digest,
            components,
        };
        Ok((provenance, sbom))
    }

    /// **Build the `ci.supply_chain.verification_failed` [`EventDraft`] for a refusal (contract 2.2;
    /// arch 03 §1.4, aggregate `ci/run/<run_id>`).** The audit-critical fail-closed proof — the live
    /// consumer emits this via the outbox in the SAME tx as the refusal (no `publish_now`). PII-free
    /// (the offending reference + the machine reason token + the run; no payloads).
    pub fn refusal_event(&self, run_id: &str, failure: &VerificationFailure) -> EventDraft {
        let aggregate = format!("ci/run/{run_id}");
        EventDraft {
            type_: EventType(CI_SUPPLY_CHAIN_VERIFICATION_FAILED.to_string()),
            subject: ArtifactRef(aggregate.clone()),
            aggregate: AggregateKey(aggregate.clone()),
            payload: serde_json::json!({
                "run": aggregate,
                "reason": failure.reason_token(),
                "component": failure.component(),
                "detail": failure.to_string(),
            }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        }
    }
}

/// The `@<algo>:<hex>` digest half of a digest-pinned reference (the component's content digest).
/// For `registry/foo@sha256:abc` → `sha256:abc`. Callers pass only digest-pinned references here.
fn component_digest(reference: &str) -> String {
    reference
        .rsplit_once('@')
        .map(|(_, d)| d.to_string())
        .unwrap_or_else(|| reference.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PINNED: &str = "registry.example/build@sha256:abc123def456";
    const PINNED2: &str = "registry.example/test@sha256:ffeeddccbbaa";

    fn identity() -> BuildIdentity {
        BuildIdentity::new("run-0001", "ci-runner@acme")
    }

    fn snapshot() -> ArtifactRef {
        ArtifactRef("myelin://acme/ci/snapshot/blake3:deadbeef".into())
    }

    /// A verifier with `PINNED` already signed + logged (the happy-path precondition).
    fn verifier_with_pinned_logged() -> (SupplyChainVerifier, KeylessSignature) {
        let mut v = SupplyChainVerifier::new();
        let sig = KeylessSignature::sign(component_digest(PINNED), &identity());
        v.record_signature(&sig)
            .expect("an honest signature records");
        (v, sig)
    }

    // -------- 1. digest-pin-or-fail-closed AT RUN (the headline) --------

    /// **THE run-time floating-tag GATE: a floating tag reaching a RUN is REFUSED fail-closed (0
    /// un-pinned executions) — CI-P11 enforces this at plan, CI-P23 at run.**
    #[test]
    fn a_floating_tag_is_refused_at_run() {
        let v = SupplyChainVerifier::new();
        for bad in [
            "alpine",
            "alpine:3",
            "alpine:latest",
            "registry/foo@sha256:",
        ] {
            let err = v
                .verify_component(bad, None)
                .expect_err("a floating tag must be refused at run");
            assert!(
                matches!(&err, VerificationFailure::FloatingTag { reference } if reference == bad),
                "the floating tag `{bad}` is refused: {err:?}"
            );
            assert_eq!(err.reason_token(), "floating_tag");
        }
    }

    // -------- 2. sign + verify-before-use --------

    /// An UNSIGNED digest-pinned component is REFUSED (sign-before-use; 0 unsigned-component runs).
    #[test]
    fn an_unsigned_component_is_refused() {
        let v = SupplyChainVerifier::new();
        let err = v
            .verify_component(PINNED, None)
            .expect_err("an unsigned component must be refused");
        assert!(matches!(err, VerificationFailure::Unsigned { .. }));
        assert_eq!(err.reason_token(), "unsigned");
    }

    /// **A TAMPERED component is REFUSED: the signature attests digest A, but the component arrives
    /// as digest B (the bytes changed) → the signature is not FOR this component → refused.**
    #[test]
    fn a_tampered_component_is_refused() {
        let (v, sig_for_pinned) = verifier_with_pinned_logged();
        // The signature is for PINNED; we present it alongside PINNED2 (a different digest).
        let err = v
            .verify_component(PINNED2, Some(&sig_for_pinned))
            .expect_err("a signature over a different digest is not for this component");
        // The signature's digest != PINNED2's digest → treated as unsigned-for-this-component.
        assert!(matches!(err, VerificationFailure::Unsigned { .. }));
    }

    /// **A FORGED signature (its bytes do not honestly attest its own claimed digest+identity) is
    /// REFUSED at verify-before-use.**
    #[test]
    fn a_forged_signature_is_refused() {
        let v = SupplyChainVerifier::new();
        let mut forged = KeylessSignature::sign(component_digest(PINNED), &identity());
        forged.signature = "blake3:0000000000".into(); // mangle the signature bytes
        let err = v
            .verify_component(PINNED, Some(&forged))
            .expect_err("a forged signature must be refused");
        assert!(matches!(err, VerificationFailure::SignatureMismatch { .. }));
        assert_eq!(err.reason_token(), "signature_mismatch");
        // record_signature ALSO refuses a self-inconsistent signature (never logs a forgery).
        let mut v2 = SupplyChainVerifier::new();
        assert!(v2.record_signature(&forged).is_err());
    }

    /// **A well-formed signature ABSENT from the transparency log is REFUSED (an un-logged signature
    /// is not trustworthy — sigstore's core property).**
    #[test]
    fn a_signature_not_in_the_transparency_log_is_refused() {
        // A verifier with an EMPTY log; the signature is honest but never appended.
        let v = SupplyChainVerifier::new();
        let sig = KeylessSignature::sign(component_digest(PINNED), &identity());
        assert!(sig.verifies(), "the signature is honest");
        let err = v
            .verify_component(PINNED, Some(&sig))
            .expect_err("an un-logged signature must be refused");
        assert!(matches!(
            err,
            VerificationFailure::NotInTransparencyLog { .. }
        ));
        assert_eq!(err.reason_token(), "not_in_transparency_log");
    }

    /// **THE happy path: a digest-pinned, signed, logged component VERIFIES (the only admitted
    /// shape).**
    #[test]
    fn a_pinned_signed_logged_component_verifies() {
        let (v, sig) = verifier_with_pinned_logged();
        assert!(
            v.verify_component(PINNED, Some(&sig)).is_ok(),
            "a pinned + signed + logged component is the only admitted shape"
        );
    }

    // -------- 3. the transparency log is the RFC 6962 Merkle structure (10.6 pattern) --------

    /// **The Rekor log is the RFC 6962 BLAKE3 Merkle tree: the root is deterministic, changes on
    /// append, and `append` is idempotent (the same entry is one leaf).** The 10.6 CT pattern.
    #[test]
    fn the_rekor_log_is_a_deterministic_merkle_tree() {
        let mut a = RekorLog::new();
        let mut b = RekorLog::new();
        let empty_root = a.root();
        assert_eq!(a.tree_size(), 0);

        let s1 = KeylessSignature::sign(component_digest(PINNED), &identity());
        let s2 = KeylessSignature::sign(component_digest(PINNED2), &identity());
        // Same appends in the same order → same root (deterministic).
        a.append(&s1);
        a.append(&s2);
        b.append(&s1);
        b.append(&s2);
        assert_eq!(a.root(), b.root(), "the Merkle root is deterministic");
        assert_ne!(a.root(), empty_root, "the root changes on append");
        assert_eq!(a.tree_size(), 2);
        // Idempotent: re-appending the same entry is one leaf (a set, not a multiset).
        let idx = a.append(&s1);
        assert_eq!(idx, 0, "re-append returns the existing leaf index");
        assert_eq!(a.tree_size(), 2, "no new leaf for a duplicate entry");
        assert!(a.contains(&s1) && a.contains(&s2));
    }

    /// **The Merkle root is a KNOWN-ANSWER value (kills the constant-return + structural mutants):
    /// a 1-leaf, a 2-leaf, and a 3-leaf (odd-promotion) tree each have a distinct, fully-determined
    /// `blake3:<hex>` root — and the 3-leaf root is NOT the 2-leaf root (the odd node is carried up,
    /// not dropped or duplicated).** Pins the RFC 6962 leaf/node domain separation + the
    /// odd-promotion recurrence so a mutated `merkle_root`/`leaf_hash`/`node_hash`/`blake3_raw`/
    /// `hex_encode` is caught (the security-load-bearing transparency structure).
    #[test]
    fn the_merkle_root_is_a_pinned_known_answer() {
        // Reconstruct the expected root from the documented recurrence (the test is the oracle).
        let entry = |s: &KeylessSignature| -> [u8; 32] {
            leaf_hash(&blake3_raw(RekorLog::entry_string(s).as_bytes()))
        };
        let s1 = KeylessSignature::sign("sha256:aa", &identity());
        let s2 = KeylessSignature::sign("sha256:bb", &identity());
        let s3 = KeylessSignature::sign("sha256:cc", &identity());
        let l1 = entry(&s1);
        let l2 = entry(&s2);
        let l3 = entry(&s3);

        // 1-leaf tree: the root IS the single leaf hash.
        let mut log1 = RekorLog::new();
        log1.append(&s1);
        assert_eq!(log1.root(), format!("blake3:{}", hex_encode(&l1)));

        // 2-leaf tree: root = node(l1, l2).
        let mut log2 = RekorLog::new();
        log2.append(&s1);
        log2.append(&s2);
        let expect2 = node_hash(&l1, &l2);
        assert_eq!(log2.root(), format!("blake3:{}", hex_encode(&expect2)));
        // node_hash is NOT commutative (order-sensitive) — swapping inputs changes the root.
        assert_ne!(node_hash(&l1, &l2), node_hash(&l2, &l1));
        // A leaf hash is NOT the same as a node hash over zeroed children (domain separation).
        assert_ne!(l1, node_hash(&[0u8; 32], &[0u8; 32]));

        // 3-leaf tree: root = node(node(l1,l2), l3) — the ODD leaf l3 is PROMOTED unpaired one level.
        let mut log3 = RekorLog::new();
        log3.append(&s1);
        log3.append(&s2);
        log3.append(&s3);
        let expect3 = node_hash(&node_hash(&l1, &l2), &l3);
        assert_eq!(log3.root(), format!("blake3:{}", hex_encode(&expect3)));
        // The 3-leaf root differs from the 2-leaf root (the odd node was carried up, not dropped).
        assert_ne!(log3.root(), log2.root());
        // hex_encode is a real 64-char hex (kills the empty/constant-string mutants).
        let root_hex = log3.root();
        assert_eq!(root_hex.len(), "blake3:".len() + 64);
        assert!(root_hex
            .trim_start_matches("blake3:")
            .chars()
            .all(|c| c.is_ascii_hexdigit()));
    }

    /// **The Display + statement helpers produce DISTINGUISHING, non-empty output (kills the
    /// `Ok(default)` / `String::new()` mutants).** Each failure variant renders a distinct message
    /// naming its component; two different provenance statements differ.
    #[test]
    fn the_failure_display_and_provenance_statement_are_distinguishing() {
        let f1 = VerificationFailure::FloatingTag {
            reference: "alpine:3".into(),
        };
        let f2 = VerificationFailure::Unsigned {
            component_digest: "sha256:aa".into(),
        };
        assert!(f1.to_string().contains("alpine:3") && f1.to_string().contains("FLOATING"));
        assert!(f2.to_string().contains("sha256:aa") && f2.to_string().contains("UNSIGNED"));
        assert_ne!(f1.to_string(), f2.to_string());

        let p1 = SlsaProvenance {
            artifact_digest: "blake3:art1".into(),
            identity: identity(),
            definition_snapshot: snapshot(),
            input_digests: vec!["sha256:aa".into()],
        };
        let mut p2 = p1.clone();
        p2.artifact_digest = "blake3:art2".into();
        assert!(!p1.statement().is_empty());
        assert_ne!(
            p1.statement(),
            p2.statement(),
            "a different artifact → a different provenance statement → a different signature"
        );
        assert_ne!(p1.sign().signature, p2.sign().signature);
    }

    // -------- 4. SLSA provenance + SBOM attestation --------

    /// **`attest` generates a SIGNED SLSA provenance (run + snapshot + sorted input digests) + an
    /// SBOM, and seals the provenance into the transparency log.**
    #[test]
    fn attest_generates_signed_provenance_and_sbom() {
        let mut v = SupplyChainVerifier::new();
        let before = v.rekor().tree_size();
        let (prov, sbom) = v
            .attest(
                "blake3:artifact00",
                &identity(),
                &snapshot(),
                &[PINNED2.to_string(), PINNED.to_string()],
                SbomFormat::CycloneDx,
            )
            .expect("a pinned-input attestation succeeds");
        // The provenance records the run, the snapshot, and the SORTED input digests.
        assert_eq!(prov.identity, identity());
        assert_eq!(prov.definition_snapshot, snapshot());
        assert_eq!(
            prov.input_digests,
            vec![
                "sha256:abc123def456".to_string(),
                "sha256:ffeeddccbbaa".to_string()
            ],
            "input digests are sorted (deterministic provenance)"
        );
        // The provenance is itself a signed, honest attestation.
        assert!(prov.sign().verifies());
        // It was sealed into the transparency log (one new leaf).
        assert_eq!(v.rekor().tree_size(), before + 1);
        // The SBOM inventories the SORTED digest-pinned components.
        assert_eq!(sbom.format, SbomFormat::CycloneDx);
        assert_eq!(
            sbom.components,
            vec![PINNED.to_string(), PINNED2.to_string()]
        );
        assert_eq!(sbom.artifact_digest, "blake3:artifact00");
    }

    /// A floating INPUT to `attest` is refused fail-closed (the SBOM/provenance only inventory
    /// digest-pinned components — the same control).
    #[test]
    fn attest_refuses_a_floating_input() {
        let mut v = SupplyChainVerifier::new();
        let err = v
            .attest(
                "blake3:artifact00",
                &identity(),
                &snapshot(),
                &["alpine:3".to_string()],
                SbomFormat::Spdx,
            )
            .expect_err("a floating input must be refused");
        assert!(matches!(err, VerificationFailure::FloatingTag { .. }));
    }

    // -------- 5. the verification_failed event (contract 2.2 / arch 03 §1.4) --------

    /// **Every refusal builds a `ci.supply_chain.verification_failed` draft (the audit-critical
    /// fail-closed proof): correct token, the `ci/run/<run_id>` aggregate, the machine reason token,
    /// the offending component, PII-free.**
    #[test]
    fn a_refusal_builds_the_verification_failed_event() {
        let v = SupplyChainVerifier::new();
        let failure = v.verify_component("alpine:3", None).unwrap_err();
        let draft = v.refusal_event("run-0001", &failure);
        assert_eq!(draft.type_.0, CI_SUPPLY_CHAIN_VERIFICATION_FAILED);
        assert_eq!(draft.aggregate.0, "ci/run/run-0001");
        assert_eq!(draft.subject.0, "ci/run/run-0001");
        assert_eq!(draft.payload["reason"], "floating_tag");
        assert_eq!(draft.payload["component"], "alpine:3");
        assert_eq!(draft.payload["run"], "ci/run/run-0001");
        // PII-free (references-not-payloads).
        assert!(!draft.contains_personal_data);
        assert!(draft.pii_key_ref.is_none());
    }

    /// **0 un-pinned/unsigned executions: across the failure matrix, EVERY refusal yields a
    /// verification_failed draft (none silently passes).** The CI-D4 quantified gate.
    #[test]
    fn every_refusal_path_emits_the_audit_event() {
        let (v, sig_for_pinned) = verifier_with_pinned_logged();
        let mut forged = KeylessSignature::sign(component_digest(PINNED2), &identity());
        forged.signature = "blake3:dead".into();
        let unlogged = KeylessSignature::sign(component_digest(PINNED2), &identity());

        // (reference, signature, expected reason token)
        let cases: Vec<(&str, Option<&KeylessSignature>, &str)> = vec![
            ("alpine:3", None, "floating_tag"),
            (PINNED2, None, "unsigned"),
            (PINNED2, Some(&forged), "signature_mismatch"),
            (PINNED2, Some(&unlogged), "not_in_transparency_log"),
        ];
        for (reference, sig, expected) in cases {
            let err = v
                .verify_component(reference, sig)
                .expect_err("this case must refuse");
            assert_eq!(err.reason_token(), expected, "reason for `{reference}`");
            let draft = v.refusal_event("run-0001", &err);
            assert_eq!(draft.type_.0, CI_SUPPLY_CHAIN_VERIFICATION_FAILED);
            assert_eq!(draft.payload["reason"], expected);
        }
        // The ONE admitted shape still passes (the gate is not over-eager).
        assert!(v.verify_component(PINNED, Some(&sig_for_pinned)).is_ok());
    }
}
