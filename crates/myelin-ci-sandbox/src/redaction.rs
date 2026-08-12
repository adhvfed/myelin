use crate::JobSpec;
use std::fmt;
use zeroize::{Zeroize, Zeroizing};

#[derive(Clone, PartialEq, Eq)]
pub struct RedactionPlan {
    needles: Vec<Vec<u8>>,
    marker: Vec<u8>,
}

impl Default for RedactionPlan {
    fn default() -> Self {
        Self::none()
    }
}

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
    pub fn none() -> RedactionPlan {
        RedactionPlan {
            needles: Vec::new(),
            marker: b"[myelin-redacted]".to_vec(),
        }
    }

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

    pub fn for_needles(
        needles: impl IntoIterator<Item = Vec<u8>>,
    ) -> Result<RedactionPlan, RedactionPlanError> {
        let mut needles: Vec<Vec<u8>> = needles.into_iter().filter(|n| !n.is_empty()).collect();
        needles.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
        let marker = collision_free_marker(&needles)?;
        Ok(RedactionPlan { needles, marker })
    }

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

#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedSecretEnv {
    name: String,
    value: Zeroizing<String>,
}

impl ResolvedSecretEnv {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: Zeroizing::new(value.into()),
        }
    }

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretInjectionError {
    BindingSetMismatch,
    EmptyValue { name: String },
    EnvNameCollision { name: String },
    CoverageMismatch,
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

    pub fn len(&self) -> usize {
        self.bindings.len()
    }

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
        let plan = RedactionPlan::for_needles([b"s3cr3t".to_vec(), b"hunter2".to_vec()]).unwrap();
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
        let plan =
            RedactionPlan::for_needles([b"left".to_vec(), b"right".to_vec(), synthesized.clone()])
                .unwrap();
        let output = plan.redact(b"leftright");
        assert!(!contains_bytes(&output, &synthesized));
    }

    #[test]
    fn all_byte_alphabet_is_rejected_instead_of_using_an_empty_marker() {
        let all_bytes: Vec<u8> = (0u8..=u8::MAX).collect();
        let error = RedactionPlan::for_needles([all_bytes, b"ab".to_vec(), b"X".to_vec()])
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
