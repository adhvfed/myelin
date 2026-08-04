use std::collections::BTreeMap;

use myelin_ci_sandbox::events::CI_SUPPLY_CHAIN_VERIFICATION_FAILED;
use myelin_ci_sandbox::ImageRef;
use myelin_events::{AggregateKey, ArtifactRef, DataRole, EventDraft, EventType, Visibility};
use myelin_storage::ContentHash;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildIdentity {
    pub run_id: String,
    pub workload: String,
}

impl BuildIdentity {
    pub fn new(run_id: impl Into<String>, workload: impl Into<String>) -> BuildIdentity {
        BuildIdentity {
            run_id: run_id.into(),
            workload: workload.into(),
        }
    }

    fn binding(&self) -> String {
        format!("{}@{}", self.workload, self.run_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeylessSignature {
    pub component_digest: String,
    pub identity: BuildIdentity,
    pub signature: String,
}

impl KeylessSignature {
    pub fn sign(component_digest: impl Into<String>, identity: &BuildIdentity) -> KeylessSignature {
        let component_digest = component_digest.into();
        let signature = Self::expected(&component_digest, identity);
        KeylessSignature {
            component_digest,
            identity: identity.clone(),
            signature,
        }
    }

    fn expected(component_digest: &str, identity: &BuildIdentity) -> String {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(component_digest.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(identity.binding().as_bytes());
        ContentHash::blake3(&bytes).to_multihash_string()
    }

    pub fn verifies(&self) -> bool {
        self.signature == Self::expected(&self.component_digest, &self.identity)
    }
}

#[derive(Clone, Debug, Default)]
pub struct RekorLog {
    leaves: Vec<[u8; 32]>,
    index: BTreeMap<String, usize>,
}

impl RekorLog {
    pub fn new() -> RekorLog {
        RekorLog::default()
    }

    fn entry_string(sig: &KeylessSignature) -> String {
        format!(
            "{}|{}|{}",
            sig.component_digest,
            sig.identity.binding(),
            sig.signature
        )
    }

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

    pub fn contains(&self, sig: &KeylessSignature) -> bool {
        self.index.contains_key(&Self::entry_string(sig))
    }

    pub fn tree_size(&self) -> usize {
        self.leaves.len()
    }

    pub fn root(&self) -> String {
        if self.leaves.is_empty() {
            return ContentHash::blake3(b"").to_multihash_string();
        }
        let root = merkle_root(&self.leaves);
        format!("blake3:{}", hex_encode(&root))
    }
}

fn hex_encode(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn blake3_raw(bytes: &[u8]) -> [u8; 32] {
    let hex = ContentHash::blake3(bytes).digest_hex;
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        let byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .expect("ContentHash::blake3 yields valid 64-hex");
        *slot = byte;
    }
    out
}

fn leaf_hash(entry_digest: &[u8; 32]) -> [u8; 32] {
    let mut buf = Vec::with_capacity(33);
    buf.push(0x00);
    buf.extend_from_slice(entry_digest);
    blake3_raw(&buf)
}

fn node_hash(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut buf = Vec::with_capacity(65);
    buf.push(0x01);
    buf.extend_from_slice(left);
    buf.extend_from_slice(right);
    blake3_raw(&buf)
}

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
                next.push(level[i]);
            }
            i += 2;
        }
        level = next;
    }
    level[0]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlsaProvenance {
    pub artifact_digest: String,
    pub identity: BuildIdentity,
    pub definition_snapshot: ArtifactRef,
    pub input_digests: Vec<String>,
}

impl SlsaProvenance {
    fn statement(&self) -> String {
        let mut s = String::new();
        s.push_str(&self.artifact_digest);
        s.push('|');
        s.push_str(&self.identity.binding());
        s.push('|');
        s.push_str(&self.definition_snapshot.0);
        s.push('|');
        s.push_str(&self.input_digests.join(","));
        s
    }

    pub fn sign(&self) -> KeylessSignature {
        KeylessSignature::sign(
            ContentHash::blake3(self.statement().as_bytes()).to_multihash_string(),
            &self.identity,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SbomFormat {
    CycloneDx,
    Spdx,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sbom {
    pub format: SbomFormat,
    pub artifact_digest: String,
    pub components: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerificationFailure {
    FloatingTag {
        reference: String,
    },
    Unsigned {
        component_digest: String,
    },
    SignatureMismatch {
        component_digest: String,
    },
    NotInTransparencyLog {
        component_digest: String,
    },
}

impl std::fmt::Display for VerificationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerificationFailure::FloatingTag { reference } => write!(
                f,
                "component `{reference}` is a FLOATING TAG - refused fail-closed at RUN \
                 (digest-pin-or-fail-closed; 0 un-pinned executions). Resolve to `@<algo>:<hex>`."
            ),
            VerificationFailure::Unsigned { component_digest } => write!(
                f,
                "component `{component_digest}` is UNSIGNED - refused fail-closed \
                 (sign-before-use; 0 unsigned-component runs)."
            ),
            VerificationFailure::SignatureMismatch { component_digest } => write!(
                f,
                "component `{component_digest}` has a signature that does NOT attest its digest \
                 under its identity - TAMPERED/forged, refused fail-closed (verify-before-use)."
            ),
            VerificationFailure::NotInTransparencyLog { component_digest } => write!(
                f,
                "component `{component_digest}`'s signature is NOT in the Rekor transparency log - \
                 an un-logged signature is not trustworthy, refused fail-closed."
            ),
        }
    }
}

impl std::error::Error for VerificationFailure {}

impl VerificationFailure {
    pub fn reason_token(&self) -> &'static str {
        match self {
            VerificationFailure::FloatingTag { .. } => "floating_tag",
            VerificationFailure::Unsigned { .. } => "unsigned",
            VerificationFailure::SignatureMismatch { .. } => "signature_mismatch",
            VerificationFailure::NotInTransparencyLog { .. } => "not_in_transparency_log",
        }
    }

    pub fn component(&self) -> &str {
        match self {
            VerificationFailure::FloatingTag { reference } => reference,
            VerificationFailure::Unsigned { component_digest }
            | VerificationFailure::SignatureMismatch { component_digest }
            | VerificationFailure::NotInTransparencyLog { component_digest } => component_digest,
        }
    }
}

#[derive(Debug, Default)]
pub struct SupplyChainVerifier {
    rekor: RekorLog,
}

impl SupplyChainVerifier {
    pub fn new() -> SupplyChainVerifier {
        SupplyChainVerifier {
            rekor: RekorLog::new(),
        }
    }

    pub fn rekor(&self) -> &RekorLog {
        &self.rekor
    }

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

    pub fn verify_component(
        &self,
        reference: &str,
        signature: Option<&KeylessSignature>,
    ) -> Result<(), VerificationFailure> {
        let image = ImageRef {
            reference: reference.to_string(),
        };
        if !image.digest_pinned() {
            return Err(VerificationFailure::FloatingTag {
                reference: reference.to_string(),
            });
        }
        let digest = component_digest(reference);

        let Some(sig) = signature else {
            return Err(VerificationFailure::Unsigned {
                component_digest: digest,
            });
        };
        if sig.component_digest != digest {
            return Err(VerificationFailure::Unsigned {
                component_digest: digest,
            });
        }

        if !sig.verifies() {
            return Err(VerificationFailure::SignatureMismatch {
                component_digest: digest,
            });
        }

        if !self.rekor.contains(sig) {
            return Err(VerificationFailure::NotInTransparencyLog {
                component_digest: digest,
            });
        }
        Ok(())
    }

    pub fn attest(
        &mut self,
        artifact_digest: impl Into<String>,
        identity: &BuildIdentity,
        definition_snapshot: &ArtifactRef,
        input_references: &[String],
        sbom_format: SbomFormat,
    ) -> Result<(SlsaProvenance, Sbom), VerificationFailure> {
        let artifact_digest = artifact_digest.into();
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
        let prov_sig = provenance.sign();
        self.rekor.append(&prov_sig);

        let sbom = Sbom {
            format: sbom_format,
            artifact_digest,
            components,
        };
        Ok((provenance, sbom))
    }

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

fn component_digest(reference: &str) -> String {
    reference
        .rsplit_once('@')
        .map(|(_, d)| d.to_string())
        .unwrap_or_else(|| reference.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PINNED: &str = "registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000";
    const PINNED2: &str = "registry.example/test@sha256:ffeeddccbbaa0000000000000000000000000000000000000000000000000000";

    fn identity() -> BuildIdentity {
        BuildIdentity::new("run-0001", "ci-runner@acme")
    }

    fn snapshot() -> ArtifactRef {
        ArtifactRef("myelin://acme/ci/snapshot/blake3:deadbeef".into())
    }

    fn verifier_with_pinned_logged() -> (SupplyChainVerifier, KeylessSignature) {
        let mut v = SupplyChainVerifier::new();
        let sig = KeylessSignature::sign(component_digest(PINNED), &identity());
        v.record_signature(&sig)
            .expect("an honest signature records");
        (v, sig)
    }

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

    #[test]
    fn an_unsigned_component_is_refused() {
        let v = SupplyChainVerifier::new();
        let err = v
            .verify_component(PINNED, None)
            .expect_err("an unsigned component must be refused");
        assert!(matches!(err, VerificationFailure::Unsigned { .. }));
        assert_eq!(err.reason_token(), "unsigned");
    }

    #[test]
    fn a_tampered_component_is_refused() {
        let (v, sig_for_pinned) = verifier_with_pinned_logged();
        let err = v
            .verify_component(PINNED2, Some(&sig_for_pinned))
            .expect_err("a signature over a different digest is not for this component");
        assert!(matches!(err, VerificationFailure::Unsigned { .. }));
    }

    #[test]
    fn a_forged_signature_is_refused() {
        let v = SupplyChainVerifier::new();
        let mut forged = KeylessSignature::sign(component_digest(PINNED), &identity());
        forged.signature = "blake3:0000000000".into();
        let err = v
            .verify_component(PINNED, Some(&forged))
            .expect_err("a forged signature must be refused");
        assert!(matches!(err, VerificationFailure::SignatureMismatch { .. }));
        assert_eq!(err.reason_token(), "signature_mismatch");
        let mut v2 = SupplyChainVerifier::new();
        assert!(v2.record_signature(&forged).is_err());
    }

    #[test]
    fn a_signature_not_in_the_transparency_log_is_refused() {
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

    #[test]
    fn a_pinned_signed_logged_component_verifies() {
        let (v, sig) = verifier_with_pinned_logged();
        assert!(
            v.verify_component(PINNED, Some(&sig)).is_ok(),
            "a pinned + signed + logged component is the only admitted shape"
        );
    }

    #[test]
    fn the_rekor_log_is_a_deterministic_merkle_tree() {
        let mut a = RekorLog::new();
        let mut b = RekorLog::new();
        let empty_root = a.root();
        assert_eq!(a.tree_size(), 0);

        let s1 = KeylessSignature::sign(component_digest(PINNED), &identity());
        let s2 = KeylessSignature::sign(component_digest(PINNED2), &identity());
        a.append(&s1);
        a.append(&s2);
        b.append(&s1);
        b.append(&s2);
        assert_eq!(a.root(), b.root(), "the Merkle root is deterministic");
        assert_ne!(a.root(), empty_root, "the root changes on append");
        assert_eq!(a.tree_size(), 2);
        let idx = a.append(&s1);
        assert_eq!(idx, 0, "re-append returns the existing leaf index");
        assert_eq!(a.tree_size(), 2, "no new leaf for a duplicate entry");
        assert!(a.contains(&s1) && a.contains(&s2));
    }

    #[test]
    fn the_merkle_root_is_a_pinned_known_answer() {
        let entry = |s: &KeylessSignature| -> [u8; 32] {
            leaf_hash(&blake3_raw(RekorLog::entry_string(s).as_bytes()))
        };
        let s1 = KeylessSignature::sign("sha256:aa", &identity());
        let s2 = KeylessSignature::sign("sha256:bb", &identity());
        let s3 = KeylessSignature::sign("sha256:cc", &identity());
        let l1 = entry(&s1);
        let l2 = entry(&s2);
        let l3 = entry(&s3);

        let mut log1 = RekorLog::new();
        log1.append(&s1);
        assert_eq!(log1.root(), format!("blake3:{}", hex_encode(&l1)));

        let mut log2 = RekorLog::new();
        log2.append(&s1);
        log2.append(&s2);
        let expect2 = node_hash(&l1, &l2);
        assert_eq!(log2.root(), format!("blake3:{}", hex_encode(&expect2)));
        assert_ne!(node_hash(&l1, &l2), node_hash(&l2, &l1));
        assert_ne!(l1, node_hash(&[0u8; 32], &[0u8; 32]));

        let mut log3 = RekorLog::new();
        log3.append(&s1);
        log3.append(&s2);
        log3.append(&s3);
        let expect3 = node_hash(&node_hash(&l1, &l2), &l3);
        assert_eq!(log3.root(), format!("blake3:{}", hex_encode(&expect3)));
        assert_ne!(log3.root(), log2.root());
        let root_hex = log3.root();
        assert_eq!(root_hex.len(), "blake3:".len() + 64);
        assert!(root_hex
            .trim_start_matches("blake3:")
            .chars()
            .all(|c| c.is_ascii_hexdigit()));
    }

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
        assert_eq!(prov.identity, identity());
        assert_eq!(prov.definition_snapshot, snapshot());
        assert_eq!(
            prov.input_digests,
            vec![
                "sha256:abc123def4560000000000000000000000000000000000000000000000000000"
                    .to_string(),
                "sha256:ffeeddccbbaa0000000000000000000000000000000000000000000000000000"
                    .to_string()
            ],
            "input digests are sorted (deterministic provenance)"
        );
        assert!(prov.sign().verifies());
        assert_eq!(v.rekor().tree_size(), before + 1);
        assert_eq!(sbom.format, SbomFormat::CycloneDx);
        assert_eq!(
            sbom.components,
            vec![PINNED.to_string(), PINNED2.to_string()]
        );
        assert_eq!(sbom.artifact_digest, "blake3:artifact00");
    }

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
        assert!(!draft.contains_personal_data);
        assert!(draft.pii_key_ref.is_none());
    }

    #[test]
    fn every_refusal_path_emits_the_audit_event() {
        let (v, sig_for_pinned) = verifier_with_pinned_logged();
        let mut forged = KeylessSignature::sign(component_digest(PINNED2), &identity());
        forged.signature = "blake3:dead".into();
        let unlogged = KeylessSignature::sign(component_digest(PINNED2), &identity());

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
        assert!(v.verify_component(PINNED, Some(&sig_for_pinned)).is_ok());
    }
}
