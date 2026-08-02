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
use zeroize::Zeroize;

/// The marker a masked needle is replaced by in captured output (references-not-payloads: the fact a
/// secret was present is preserved; its value is not).
pub const REDACTION_MARKER: &[u8] = b"***";

/// A set of exact-value needles to mask from a job's captured output at the sandbox boundary. See the
/// module docs for the (precise, do-not-overclaim) security model and the choke-point rationale.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct RedactionPlan {
    /// The exact byte values to mask, populated from the same resolved bindings as the guest env via
    /// [`RedactionPlan::for_job`]. Needles are held as bytes (a secret is not necessarily UTF-8) and
    /// never logged/serialized.
    needles: Vec<Vec<u8>>,
}

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
        }
    }

    /// Derive the per-job plan from the SAME resolved bindings that become the guest environment.
    /// This constructor is deliberately private to the module: callers attach material through
    /// [`ResolvedJobSecrets::for_job`], which immediately checks exact coverage.
    fn for_job(_spec: &JobSpec, bindings: &[ResolvedSecretEnv]) -> RedactionPlan {
        RedactionPlan::for_needles(
            bindings
                .iter()
                .map(|binding| binding.value.as_bytes().to_vec()),
        )
    }

    /// Build a plan from explicit needles. Empty needles are dropped (masking the empty string would
    /// replace between every byte). Production injection rejects empty values before reaching this
    /// helper; the filtering keeps direct masker callers safe too.
    pub fn for_needles(needles: impl IntoIterator<Item = Vec<u8>>) -> RedactionPlan {
        let mut needles: Vec<Vec<u8>> = needles.into_iter().filter(|n| !n.is_empty()).collect();
        needles.sort_by(|left, right| {
            right
                .len()
                .cmp(&left.len())
                .then_with(|| left.cmp(right))
        });
        RedactionPlan { needles }
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
        expected.sort_by(|left, right| {
            right
                .len()
                .cmp(&left.len())
                .then_with(|| left.cmp(right))
        });
        self.needles.len() == expected.len()
            && self
                .needles
                .iter()
                .zip(expected)
                .all(|(needle, expected)| needle.as_slice() == expected)
    }

    /// Mask every needle occurrence in `bytes`, replacing each with [`REDACTION_MARKER`]. The empty
    /// plan returns the bytes unchanged. This is an exact-substring replacement; see the module's
    /// deliberately narrow security model.
    pub fn redact(&self, bytes: &[u8]) -> Vec<u8> {
        if self.needles.is_empty() {
            return bytes.to_vec();
        }
        let mut out = bytes.to_vec();
        for needle in &self.needles {
            out = replace_all(&out, needle, REDACTION_MARKER);
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
                output.extend_from_slice(REDACTION_MARKER);
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
    value: String,
}

impl ResolvedSecretEnv {
    /// Build one ephemeral binding from a broker outcome.
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
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

impl Drop for ResolvedSecretEnv {
    fn drop(&mut self) {
        self.value.zeroize();
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
            Self::CoverageMismatch => formatter.write_str(
                "injected secret environment and redaction-plan coverage disagree",
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
#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedJobSecrets {
    bindings: Vec<ResolvedSecretEnv>,
    redaction: RedactionPlan,
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

impl Default for ResolvedJobSecrets {
    fn default() -> Self {
        Self {
            bindings: Vec::new(),
            redaction: RedactionPlan::none(),
        }
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
        if let Some(binding) = bindings.iter().find(|binding| {
            spec.env
                .iter()
                .any(|literal| literal.name == binding.name)
        }) {
            return Err(SecretInjectionError::EnvNameCollision {
                name: binding.name.clone(),
            });
        }
        let redaction = RedactionPlan::for_job(spec, &bindings);
        let resolved = Self {
            bindings,
            redaction,
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
            .map(|binding| format!("{}={}", binding.name, binding.value))
    }

    pub(crate) fn redaction_plan(&self) -> &RedactionPlan {
        &self.redaction
    }

    #[cfg(test)]
    fn with_plan_for_test(
        bindings: Vec<ResolvedSecretEnv>,
        redaction: RedactionPlan,
    ) -> Self {
        Self {
            bindings,
            redaction,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn one_secret_job() -> JobSpec {
        let mut spec = crate::checkout_job_spec_for_tests();
        spec.secret_refs = vec![crate::SecretRef {
            name: "DEPLOY_TOKEN".into(),
            handle: "opaque:deploy".into(),
        }];
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
        let plan = RedactionPlan::for_needles([b"s3cr3t".to_vec(), b"hunter2".to_vec()]);
        let got = plan.redact(b"token=s3cr3t and again s3cr3t; pw=hunter2");
        assert_eq!(got, b"token=*** and again ***; pw=***".to_vec());
    }

    #[test]
    fn masks_non_utf8_needles() {
        let plan = RedactionPlan::for_needles([vec![0xff, 0x00, 0xfe]]);
        let mut input = b"before".to_vec();
        input.extend_from_slice(&[0xff, 0x00, 0xfe]);
        input.extend_from_slice(b"after");
        assert_eq!(plan.redact(&input), b"before***after".to_vec());
    }

    #[test]
    fn empty_needles_are_dropped_not_masked_between_every_byte() {
        let plan = RedactionPlan::for_needles([Vec::new(), b"keep".to_vec()]);
        assert_eq!(plan.redact(b"keep this"), b"*** this".to_vec());
    }

    #[test]
    fn adjacent_and_boundary_occurrences() {
        let plan = RedactionPlan::for_needles([b"ab".to_vec()]);
        assert_eq!(plan.redact(b"abab"), b"******".to_vec());
        assert_eq!(plan.redact(b"xabx"), b"x***x".to_vec());
    }

    #[test]
    fn overlapping_needles_mask_the_longest_injected_value_first() {
        let plan = RedactionPlan::for_needles([b"prefix".to_vec(), b"prefix-and-rest".to_vec()]);
        assert_eq!(plan.redact(b"prefix-and-rest"), b"***");
    }

    #[test]
    fn streaming_masker_covers_a_needle_split_across_capture_chunks() {
        let plan = RedactionPlan::for_needles([b"split-secret".to_vec()]);
        let mut stream = plan.streaming();
        let mut output = stream.push(b"before split-");
        output.extend(stream.push(b"secret after"));
        output.extend(stream.finish());
        assert_eq!(output, b"before *** after");
    }

    #[test]
    fn coverage_mismatch_rejects_launch_before_oci_composition() {
        let mut spec = one_secret_job();
        spec.resolved_secrets = ResolvedJobSecrets::with_plan_for_test(
            vec![ResolvedSecretEnv::new("DEPLOY_TOKEN", "injected-material")],
            RedactionPlan::none(),
        );

        assert_eq!(
            spec.validate_secret_coverage(),
            Err(SecretInjectionError::CoverageMismatch)
        );
        let error = crate::gvisor::GvisorBackend::oci_config(&spec)
            .expect_err("coverage mismatch must reject before OCI composition");
        assert!(error.to_string().contains("coverage disagree"));
    }
}
