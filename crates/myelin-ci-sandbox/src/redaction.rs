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
//! > injects is masked at the boundary (and today the platform injects none)."
//!
//! NOT "the logs contain nothing sensitive."
//!
//! ## Unbypassable choke point — currently empty BY CONSTRUCTION
//! Today the platform injects **no** secrets into any job: [`SecretBroker`] exists but is not wired
//! into any launch path (the guest receives env-var NAMES only, never resolved material). So every
//! [`RedactionPlan`] is [`RedactionPlan::none`] and the mask is a runtime no-op — there is nothing to
//! redact. Be PRECISE about what this seam guarantees today (co-review 2026-07-17, P1):
//!
//! - **A choke point that cannot be skipped (holds now):** every capture→result path takes a
//!   `&RedactionPlan` and applies it as the final step, so no backend can forward un-redacted captured
//!   bytes — a new backend cannot forget the step, it is a REQUIRED argument.
//!
//! What this seam does NOT yet guarantee — and is the **named obligation of the future CI-1 secret
//! injection feature**, not buildable here because injection does not exist — is that the plan is
//! CORRECTLY POPULATED with every injected secret. A required argument ensures *a* plan is passed, not
//! that it covers what was injected. So injection MUST, in ITS own change:
//!   - derive the guest secret env AND this plan from ONE inseparable resolved-secrets value (you
//!     cannot inject a secret env entry without its needle), threaded via [`RedactionPlan::for_job`]; and
//!   - REJECT the launch if the injected-secret set and the plan's coverage disagree.
//!
//! Until injection exists there is nothing to couple; this module is the choke point it plugs into.
//!
//! Per the co-review (2026-07-17), the production MASKER semantics — streaming across capture-chunk
//! boundaries, overlapping/encoded/transformed values, a minimum-length floor, performance — are
//! deliberately deferred to when real injection exists; this seam is the correctly-placed,
//! unbypassable choke point + enough coverage to prove that placement.
//!
//! [`SecretBroker`]: (the ci-controlplane in-boundary secret broker — not yet wired into launch)

/// The marker a masked needle is replaced by in captured output (references-not-payloads: the fact a
/// secret was present is preserved; its value is not).
pub const REDACTION_MARKER: &[u8] = b"***";

/// A set of exact-value needles to mask from a job's captured output at the sandbox boundary. See the
/// module docs for the (precise, do-not-overclaim) security model and the choke-point rationale.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RedactionPlan {
    /// The exact byte values to mask. EMPTY today (the platform injects no secrets); populated by CI-1
    /// secret injection via [`RedactionPlan::for_job`]. Needles are held as bytes (a secret is not
    /// necessarily UTF-8) and never logged/serialized.
    needles: Vec<Vec<u8>>,
}

impl RedactionPlan {
    /// The empty plan — "there are no CI-managed secret needles to mask for this job." The ONLY plan
    /// produced today (nothing injects secrets). A no-op mask.
    pub fn none() -> RedactionPlan {
        RedactionPlan {
            needles: Vec::new(),
        }
    }

    /// **The per-job plan seam CI-1 secret injection must populate.** TODAY the platform injects no
    /// secrets, so this is [`RedactionPlan::none`]. When secret injection is wired, the resolved secret
    /// material for `spec` MUST be built into this plan at the SAME site it is injected into the guest
    /// env (see the module docs) — masking and injection land together, or neither does. The `spec`
    /// carries only opaque `SecretRef`s (never material), so there is nothing to mask from it here yet;
    /// the argument is threaded so the seam has the job in hand the day resolution exists.
    pub fn for_job(_spec: &crate::JobSpec) -> RedactionPlan {
        RedactionPlan::none()
    }

    /// Build a plan from explicit needles (the shape CI-1 injection will use). Empty needles are
    /// dropped (masking the empty string would replace between every byte). The production masker will
    /// add a minimum-length floor + encoding handling; this is the seam.
    pub fn for_needles(needles: impl IntoIterator<Item = Vec<u8>>) -> RedactionPlan {
        RedactionPlan {
            needles: needles.into_iter().filter(|n| !n.is_empty()).collect(),
        }
    }

    /// `true` iff there is nothing to mask (the only state reachable today).
    pub fn is_empty(&self) -> bool {
        self.needles.is_empty()
    }

    /// Mask every needle occurrence in `bytes`, replacing each with [`REDACTION_MARKER`]. The empty
    /// plan returns the bytes unchanged (identity — the no-op today). A simple exact-substring replace:
    /// enough to prove the boundary seam; the streaming/overlap/encoding-aware production masker is
    /// deferred to when real injection exists (see the module docs).
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
}
