//! # `redaction` — CT-004f sub-step 1: the boundary secret-redaction seam
//!
//! Boundary redaction masks the CI-managed secret values in a job's captured stdout/stderr at the
//! sandbox boundary — inside the backend's capture→result step, BEFORE the bytes populate
//! [`SandboxResult`](crate::SandboxResult) and cross back toward the durable log pipeline (CT-004f).
//! It is the safety property that lets the live log sink seal captured output to durable storage
//! without leaking a platform-injected secret into the content-addressed log store.
//!
//! ## Security model (precise — do not overclaim)
//! Exact-value masking protects ONLY **known needles**: the CI-managed secret values the platform
//! itself injects into a job. It does NOT — and by construction CANNOT — stop a job from printing a
//! credential it found in its own source tree, a restored cache, customer data, or a literal typed
//! into its own script. The guarantee this seam enforces is therefore:
//!
//! > "The durable logs contain no CI-managed secret plaintext, because every value the platform
//! > injects is masked at the boundary."
//!
//! NOT "the logs contain nothing sensitive."
//!
//! ## Unbypassable choke point + inseparable injection plan
//! [`ResolvedJobSecrets`] is the ephemeral, non-serializable launch value that couples every guest
//! secret environment entry to its exact-value redaction needle. Its checked constructor derives
//! both representations together through [`RedactionPlan::for_job`] and verifies exact coverage.
//! The only way to attach secret material to a [`JobSpec`](crate::JobSpec) is through that checked
//! constructor; the fields that feed OCI env and boundary redaction are private.
//!
//! - **A choke point that cannot be skipped:** every capture→result path takes a
//!   `&RedactionPlan` and applies it as the final step, so no backend can forward un-redacted captured
//!   bytes — a new backend cannot forget the step, it is a REQUIRED argument.
//! - **Coverage is checked, not assumed:** [`ResolvedJobSecrets::validate_coverage`] compares the
//!   injected values and plan needles exactly. A disagreement rejects launch before OCI composition.
//!
//! Exact plaintext matches are covered across capture-chunk boundaries. Encoded or workload-
//! transformed values remain outside this exact-value guarantee; this module does not claim to be a
//! general sensitive-data detector.
//!
use crate::JobSpec;
use std::fmt;
use zeroize::{Zeroize, Zeroizing};

/// A set of exact-value needles to mask from a job's captured output at the sandbox boundary. See the
/// module docs for the (precise, do-not-overclaim) security model and the choke-point rationale.
#[derive(Clone, PartialEq, Eq)]
pub struct RedactionPlan {
    /// The exact byte values to mask, populated from the same resolved bindings as the guest env via
    /// [`RedactionPlan::for_job`]. Needles are held as bytes (a secret is not necessarily UTF-8) and
    /// never logged/serialized.
    needles: Vec<Vec<u8>>,
    /// A per-plan marker made solely from a byte absent from every needle. That delimiter makes the
    /// marker non-collidable and prevents adjacent markers or marker boundaries reconstructing one.
    marker: Vec<u8>,
}

impl Default for RedactionPlan {
    fn default() -> Self {
        Self::none()
    }
}

/// Plan construction failed because no non-empty delimiter can separate redacted spans without
/// itself containing (or joining into) a configured needle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RedactionPlanError;

impl fmt::Display for RedactionPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("no collision-free non-empty redaction marker exists")
    }
}

impl std::error::Error for RedactionPlanError {}

impl fmt::Debug for RedactionPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RedactionPlan")
            .field("needle_count", &self.needles.len())
            .field("needles", &"[REDACTED]")
            .finish()
    }
}

impl RedactionPlan {
    /// The empty plan — "there are no CI-managed secret needles to mask for this job." A no-op mask.
    pub fn none() -> RedactionPlan {
        RedactionPlan {
            needles: Vec::new(),
            marker: b"[myelin-redacted]".to_vec(),
        }
    }

    /// Derive the per-job plan from the SAME resolved bindings that become the guest environment.
    /// This constructor is deliberately private to the module: callers attach material through
    /// [`ResolvedJobSecrets::for_job`], which immediately checks exact coverage.
    fn for_job(
        _spec: &JobSpec,
        bindings: &[ResolvedSecretEnv],
    ) -> Result<RedactionPlan, RedactionPlanError> {
        RedactionPlan::for_needles(
            bindings
                .iter()
                .map(|binding| binding.value.as_bytes().to_vec()),
        )
    }

    /// Build a plan from explicit needles. Empty needles are dropped (masking the empty string would
    /// replace between every byte). Production injection rejects empty values before reaching this
    /// helper; the filtering keeps direct masker callers safe too.
    pub fn for_needles(
        needles: impl IntoIterator<Item = Vec<u8>>,
    ) -> Result<RedactionPlan, RedactionPlanError> {
        let mut needles: Vec<Vec<u8>> = needles.into_iter().filter(|n| !n.is_empty()).collect();
        needles.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
        let marker = collision_free_marker(&needles)?;
        Ok(RedactionPlan { needles, marker })
    }

    /// `true` iff there is nothing to mask.
    pub fn is_empty(&self) -> bool {
        self.needles.is_empty()
    }

    fn exactly_covers(&self, bindings: &[ResolvedSecretEnv]) -> bool {
        let mut expected: Vec<&[u8]> = bindings
            .iter()
            .map(|binding| binding.value.as_bytes())
            .collect();
        expected.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
        self.needles.len() == expected.len()
            && self
                .needles
                .iter()
                .zip(expected)
                .all(|(needle, expected)| needle.as_slice() == expected)
    }

    /// Mask every needle occurrence in `bytes`, replacing each with the plan's collision-free marker. The empty
    /// plan returns the bytes unchanged. This is an exact-substring replacement; see the module's
    /// deliberately narrow security model.
    pub fn redact(&self, bytes: &[u8]) -> Vec<u8> {
        if self.needles.is_empty() {
            return bytes.to_vec();
        }
        let mut out = bytes.to_vec();
        for needle in &self.needles {
            out = replace_all(&out, needle, &self.marker);
        }
        out
    }

    pub(crate) fn streaming(&self) -> StreamingRedactor<'_> {
        StreamingRedactor {
            plan: self,
            pending: Vec::new(),
        }
    }
}

impl Drop for RedactionPlan {
    fn drop(&mut self) {
        for needle in &mut self.needles {
            needle.zeroize();
        }
    }
}

/// Per-stream exact-value masker. It retains at most `max_needle_len - 1` source bytes so a needle
/// split across arbitrary pipe reads can never be emitted in plaintext to the durable sink.
pub(crate) struct StreamingRedactor<'a> {
    plan: &'a RedactionPlan,
    pending: Vec<u8>,
}

impl StreamingRedactor<'_> {
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.pending.extend_from_slice(bytes);
        let keep = self
            .plan
            .needles
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(1)
            .saturating_sub(1);
        let safe_start_limit = self.pending.len().saturating_sub(keep);
        self.emit_starts_before(safe_start_limit)
    }

    pub(crate) fn finish(mut self) -> Vec<u8> {
        let len = self.pending.len();
        self.emit_starts_before(len)
    }

    fn emit_starts_before(&mut self, start_limit: usize) -> Vec<u8> {
        let mut output = Vec::new();
        let mut consumed = 0;
        while consumed < start_limit {
            if let Some(needle) = self
                .plan
                .needles
                .iter()
                .filter(|needle| self.pending[consumed..].starts_with(needle))
                .max_by_key(|needle| needle.len())
            {
                output.extend_from_slice(&self.plan.marker);
                consumed += needle.len();
            } else {
                output.push(self.pending[consumed]);
                consumed += 1;
            }
        }
        self.pending.drain(..consumed);
        output
    }
}

impl Drop for StreamingRedactor<'_> {
    fn drop(&mut self) {
        self.pending.zeroize();
    }
}

/// One broker-resolved secret binding before it is coupled to the job's redaction plan.
///
/// `Debug` never renders the material and `Drop` zeroizes it. This value is intentionally not
/// serializable; durable specs carry only [`crate::SecretRef`].
#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedSecretEnv {
    name: String,
    value: Zeroizing<String>,
}

impl ResolvedSecretEnv {
    /// Build one ephemeral binding from a broker outcome.
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: Zeroizing::new(value.into()),
        }
    }

    /// Move already-zeroizing broker material into the sandbox binding without ever creating a
    /// non-zeroizing plaintext `String` on the production resolution path.
    pub fn from_zeroizing(name: impl Into<String>, value: Zeroizing<String>) -> Self {
        Self {
            name: name.into(),
            value,
        }
    }
}

impl fmt::Debug for ResolvedSecretEnv {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedSecretEnv")
            .field("name", &self.name)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

/// Why resolved secret material could not be attached to an ephemeral launch spec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretInjectionError {
    /// The resolved bindings were not exactly the job's declared refs, in declaration order.
    BindingSetMismatch,
    /// An empty secret cannot have a meaningful exact-value redaction needle.
    EmptyValue { name: String },
    /// A literal env entry and secret binding attempted to own the same variable name.
    EnvNameCollision { name: String },
    /// The plan did not contain exactly one matching needle for every injected value.
    CoverageMismatch,
    /// No non-empty marker can make exact-value replacement safe for this pathological needle set.
    UnsafeRedactionPlan,
}

impl fmt::Display for SecretInjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BindingSetMismatch => formatter.write_str(
                "resolved secret bindings differ from the job's declared secret references",
            ),
            Self::EmptyValue { name } => write!(
                formatter,
                "resolved secret `{name}` is empty and cannot be covered by exact-value redaction"
            ),
            Self::EnvNameCollision { name } => write!(
                formatter,
                "resolved secret `{name}` collides with a literal environment entry"
            ),
            Self::CoverageMismatch => formatter
                .write_str("injected secret environment and redaction-plan coverage disagree"),
            Self::UnsafeRedactionPlan => formatter.write_str(
                "resolved secret set cannot be represented by a collision-free redaction plan",
            ),
        }
    }
}

impl std::error::Error for SecretInjectionError {}

/// The ONE ephemeral value from which both OCI secret env and boundary redaction are obtained.
///
/// Its fields are private, it has no serde implementation, and the checked constructor enforces the
/// exact declared-ref set plus exact needle coverage. Consequently, a caller cannot attach a guest
/// secret env entry without attaching its matching redaction needle.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ResolvedJobSecrets {
    bindings: Vec<ResolvedSecretEnv>,
    redaction: RedactionPlan,
    authorized_trust_tier: Option<crate::TrustTier>,
    authorized_refs: Vec<crate::SecretRef>,
}

impl fmt::Debug for ResolvedJobSecrets {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedJobSecrets")
            .field("bindings", &self.bindings)
            .field("redaction", &"[REDACTED PLAN]")
            .finish()
    }
}

impl ResolvedJobSecrets {
    /// Couple the complete broker resolution to `spec`. Partial or reordered resolution is refused:
    /// launch policy for a withhold is therefore decided in the control plane before this call.
    pub fn for_job(
        spec: &JobSpec,
        bindings: Vec<ResolvedSecretEnv>,
    ) -> Result<Self, SecretInjectionError> {
        if bindings.len() != spec.secret_refs.len()
            || bindings
                .iter()
                .zip(&spec.secret_refs)
                .any(|(binding, declared)| binding.name != declared.name)
        {
            return Err(SecretInjectionError::BindingSetMismatch);
        }
        if let Some(binding) = bindings.iter().find(|binding| binding.value.is_empty()) {
            return Err(SecretInjectionError::EmptyValue {
                name: binding.name.clone(),
            });
        }
        if let Some(binding) = bindings
            .iter()
            .find(|binding| spec.env.iter().any(|literal| literal.name == binding.name))
        {
            return Err(SecretInjectionError::EnvNameCollision {
                name: binding.name.clone(),
            });
        }
        let redaction = RedactionPlan::for_job(spec, &bindings)
            .map_err(|_| SecretInjectionError::UnsafeRedactionPlan)?;
        let resolved = Self {
            bindings,
            redaction,
            authorized_trust_tier: Some(spec.trust_tier),
            authorized_refs: spec.secret_refs.clone(),
        };
        resolved.validate_coverage()?;
        Ok(resolved)
    }

    /// Reject any launch whose guest-secret values and redaction needles are not exactly equal.
    pub fn validate_coverage(&self) -> Result<(), SecretInjectionError> {
        if self.redaction.exactly_covers(&self.bindings) {
            Ok(())
        } else {
            Err(SecretInjectionError::CoverageMismatch)
        }
    }

    pub(crate) fn validate_for_job(&self, spec: &JobSpec) -> Result<(), SecretInjectionError> {
        if spec.secret_refs.is_empty()
            && self.bindings.is_empty()
            && self.authorized_trust_tier.is_none()
            && self.authorized_refs.is_empty()
        {
            return self.validate_coverage();
        }
        if self.authorized_trust_tier != Some(spec.trust_tier)
            || self.authorized_refs != spec.secret_refs
        {
            return Err(SecretInjectionError::BindingSetMismatch);
        }
        if self.bindings.len() != spec.secret_refs.len()
            || self
                .bindings
                .iter()
                .zip(&spec.secret_refs)
                .any(|(binding, declared)| binding.name != declared.name)
        {
            return Err(SecretInjectionError::BindingSetMismatch);
        }
        self.validate_coverage()
    }

    /// Number of secret env entries this launch will inject.
    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    /// Whether this launch injects no secret env entries.
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    pub(crate) fn process_env(&self) -> impl Iterator<Item = String> + '_ {
        self.bindings
            .iter()
            .map(|binding| format!("{}={}", binding.name, binding.value.as_str()))
    }

    pub(crate) fn redaction_plan(&self) -> &RedactionPlan {
        &self.redaction
    }

    #[cfg(test)]
    fn with_plan_for_test(bindings: Vec<ResolvedSecretEnv>, redaction: RedactionPlan) -> Self {
        Self {
            bindings,
            redaction,
            authorized_trust_tier: None,
            authorized_refs: Vec::new(),
        }
    }
}

/// Replace every non-overlapping occurrence of `needle` in `haystack` with `with`. `needle` is assumed
/// non-empty (enforced by [`RedactionPlan::for_needles`]).
fn replace_all(haystack: &[u8], needle: &[u8], with: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(haystack.len());
    let mut i = 0;
    while i < haystack.len() {
        if haystack[i..].starts_with(needle) {
            out.extend_from_slice(with);
            i += needle.len();
        } else {
            out.push(haystack[i]);
            i += 1;
        }
    }
    out
}

/// Choose one delimiter byte absent from every needle and repeat it into a bounded, non-empty marker.
/// Because no needle contains the delimiter at all, no needle can occur inside a marker, across two
/// adjacent markers, or across a marker/plaintext boundary. Production secret values are UTF-8
/// strings, so at least one invalid standalone UTF-8 byte is absent. An arbitrary-byte needle set
/// that collectively covers all 256 byte values has no marker satisfying this proof and is rejected
/// fail-closed instead of silently substituting the empty string.
fn collision_free_marker(needles: &[Vec<u8>]) -> Result<Vec<u8>, RedactionPlanError> {
    if needles.is_empty() {
        return Ok(b"[myelin-redacted]".to_vec());
    }
    const PREFERRED: &[u8] = b"#~^|!%+?@";
    let delimiter = PREFERRED
        .iter()
        .copied()
        .chain((0u8..=u8::MAX).filter(|byte| !PREFERRED.contains(byte)))
        .find(|candidate| needles.iter().all(|needle| !needle.contains(candidate)));
    delimiter
        .map(|byte| vec![byte; 3])
        .ok_or(RedactionPlanError)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_secret_job() -> JobSpec {
        let mut spec = crate::checkout_job_spec_for_tests();
        spec.secret_refs = vec![crate::SecretRef {
            name: "DEPLOY_TOKEN".into(),
            handle: "opaque:deploy".into(),
        }];
        spec.trust_tier = crate::TrustTier::Trusted;
        spec
    }

    #[test]
    fn empty_plan_is_identity() {
        let plan = RedactionPlan::none();
        assert!(plan.is_empty());
        assert_eq!(
            plan.redact(b"nothing to mask here"),
            b"nothing to mask here"
        );
    }

    #[test]
    fn masks_every_occurrence_of_each_needle() {
        let plan =
            RedactionPlan::for_needles([b"s3cr3t".to_vec(), b"hunter2".to_vec()]).unwrap();
        let got = plan.redact(b"token=s3cr3t and again s3cr3t; pw=hunter2");
        assert!(!contains_bytes(&got, b"s3cr3t"));
        assert!(!contains_bytes(&got, b"hunter2"));
    }

    #[test]
    fn masks_non_utf8_needles() {
        let plan = RedactionPlan::for_needles([vec![0xff, 0x00, 0xfe]]).unwrap();
        let mut input = b"before".to_vec();
        input.extend_from_slice(&[0xff, 0x00, 0xfe]);
        input.extend_from_slice(b"after");
        let output = plan.redact(&input);
        assert!(!contains_bytes(&output, &[0xff, 0x00, 0xfe]));
    }

    #[test]
    fn empty_needles_are_dropped_not_masked_between_every_byte() {
        let plan = RedactionPlan::for_needles([Vec::new(), b"keep".to_vec()]).unwrap();
        assert!(!contains_bytes(&plan.redact(b"keep this"), b"keep"));
    }

    #[test]
    fn adjacent_and_boundary_occurrences() {
        let plan = RedactionPlan::for_needles([b"ab".to_vec()]).unwrap();
        assert!(!contains_bytes(&plan.redact(b"abab"), b"ab"));
        assert!(!contains_bytes(&plan.redact(b"xabx"), b"ab"));
    }

    #[test]
    fn overlapping_needles_mask_the_longest_injected_value_first() {
        let plan =
            RedactionPlan::for_needles([b"prefix".to_vec(), b"prefix-and-rest".to_vec()]).unwrap();
        assert!(!contains_bytes(&plan.redact(b"prefix-and-rest"), b"prefix"));
    }

    #[test]
    fn streaming_masker_covers_a_needle_split_across_capture_chunks() {
        let plan = RedactionPlan::for_needles([b"split-secret".to_vec()]).unwrap();
        let mut stream = plan.streaming();
        let mut output = stream.push(b"before split-");
        output.extend(stream.push(b"secret after"));
        output.extend(stream.finish());
        assert!(!contains_bytes(&output, b"split-secret"));
    }

    #[test]
    fn marker_like_and_one_byte_secrets_are_non_colliding() {
        let needles = [
            b"*".to_vec(),
            b"**".to_vec(),
            b"***".to_vec(),
            b"[redacted]".to_vec(),
            b"x".to_vec(),
        ];
        let plan = RedactionPlan::for_needles(needles.clone()).unwrap();
        let output = plan.redact(b"***[redacted]x** *");
        for needle in needles {
            assert!(
                !contains_bytes(&output, &needle),
                "redacted output retained an adversarial needle"
            );
        }
    }

    #[test]
    fn adjacent_markers_cannot_synthesize_another_secret() {
        let synthesized = b"######".to_vec();
        let plan = RedactionPlan::for_needles([
            b"left".to_vec(),
            b"right".to_vec(),
            synthesized.clone(),
        ])
        .unwrap();
        let output = plan.redact(b"leftright");
        assert!(!contains_bytes(&output, &synthesized));
    }

    #[test]
    fn all_byte_alphabet_is_rejected_instead_of_using_an_empty_marker() {
        let all_bytes: Vec<u8> = (0u8..=u8::MAX).collect();
        let error = RedactionPlan::for_needles([
            all_bytes,
            b"ab".to_vec(),
            b"X".to_vec(),
        ])
        .expect_err("a marker-less plan must be refused fail-closed");
        assert_eq!(error, RedactionPlanError);
    }

    #[test]
    fn replacing_the_middle_byte_cannot_synthesize_another_secret() {
        let plan = RedactionPlan::for_needles([b"ab".to_vec(), b"X".to_vec()]).unwrap();
        let output = plan.redact(b"aXb");
        assert!(!contains_bytes(&output, b"ab"));
        assert!(!contains_bytes(&output, b"X"));
    }

    #[test]
    fn coverage_mismatch_rejects_launch_before_oci_composition() {
        let mut spec = one_secret_job();
        spec.resolved_secrets = ResolvedJobSecrets::with_plan_for_test(
            vec![ResolvedSecretEnv::new("DEPLOY_TOKEN", "injected-material")],
            RedactionPlan::none(),
        );
        spec.resolved_secrets.authorized_trust_tier = Some(spec.trust_tier);
        spec.resolved_secrets.authorized_refs = spec.secret_refs.clone();

        assert_eq!(
            spec.validate_secret_coverage(),
            Err(SecretInjectionError::CoverageMismatch)
        );
        let error = crate::gvisor::GvisorBackend::oci_config(&spec)
            .expect_err("coverage mismatch must reject before OCI composition");
        assert!(error.to_string().contains("coverage disagree"));
    }

    #[test]
    fn launch_rejects_trust_tier_or_handle_changed_after_resolution() {
        let resolved = one_secret_job()
            .with_resolved_secrets(vec![ResolvedSecretEnv::new(
                "DEPLOY_TOKEN",
                "injected-material",
            )])
            .unwrap();

        let mut tier_flipped = resolved.clone();
        tier_flipped.trust_tier = crate::TrustTier::UntrustedFork;
        assert_eq!(
            tier_flipped.resolved_secrets.authorized_trust_tier,
            Some(crate::TrustTier::Trusted)
        );
        assert_eq!(
            tier_flipped.validate_secret_coverage(),
            Err(SecretInjectionError::BindingSetMismatch)
        );
        assert!(crate::gvisor::GvisorBackend::oci_config(&tier_flipped).is_err());

        let mut handle_flipped = resolved;
        handle_flipped.secret_refs[0].handle = "myelin://victim/ci/secret/deploy".into();
        assert_eq!(
            handle_flipped.validate_secret_coverage(),
            Err(SecretInjectionError::BindingSetMismatch)
        );
        assert!(crate::gvisor::GvisorBackend::oci_config(&handle_flipped).is_err());
    }

    fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }
}
