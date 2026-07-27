//! CT-007 slice 5b.3-2a: the [`CheckoutAuthorizationProof`] capability and the
//! [`crate::RunnerHooks::authorize_checkout`] implementation that mints it.
//!
//! Deliberately its OWN sibling module of the crate root (never inline in `lib.rs`, Sol's review):
//! Rust's privacy rules make a private field visible to every DESCENDANT module of its defining
//! module. If `CheckoutAuthorizationProof` were defined at the crate root, `crate::gvisor` (a
//! descendant of the crate root, like every module in this crate) could forge one via a bare struct
//! literal, bypassing `authorize_checkout` entirely and defeating the whole capability guarantee.
//! Living here instead means only THIS module — the one that actually invokes the hook — can ever
//! construct one; `crate::gvisor` and everything else can only consume an already-minted proof.

use crate::{CheckoutAuthorizationScope, HookError, JobSpec, RunnerHooks};

/// An unforgeable, one-shot proof that `RunnerHooks::authorize_checkout` genuinely succeeded for an
/// EXACT `CheckoutAuthorizationScope` AND an exact token generation (CT-007 slice 5b.3-2a, Sol's
/// review): binding the `run_token.jti` the authorization was actually checked against, not just the
/// scope, is what prevents a proof minted for one claim generation from being detached and paired
/// with a DIFFERENT attempt's transport/accounting inputs. Fields are private to this module — the
/// only way to obtain one is a real successful `authorize_checkout` call. Slice 5b.3-3's Hop A is
/// meant to consume this BY VALUE directly (never merely a scope extracted from it); the accessors
/// below are borrowing, for inspection/tests only, never a substitute for consuming the whole proof.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct CheckoutAuthorizationProof {
    scope: CheckoutAuthorizationScope,
    run_token_jti: String,
}

impl CheckoutAuthorizationProof {
    #[allow(dead_code)]
    pub(crate) fn scope(&self) -> &CheckoutAuthorizationScope {
        &self.scope
    }

    #[allow(dead_code)]
    pub(crate) fn run_token_jti(&self) -> &str {
        &self.run_token_jti
    }
}

impl RunnerHooks {
    /// CT-007 slice 5b.3-2: the pre-Hop-A checkout-authorization check. A READ-ONLY verification
    /// (never a state transition) that the job's durably authorized claim actually grants read
    /// access to the EXACT repo/commit `scope` names — refuses outright if no hook was configured,
    /// rather than silently treating "no hook" as "authorized." On success, mints the ONE
    /// `CheckoutAuthorizationProof` `fetch_checkout_pack` can consume for this attempt. `pub(crate)`,
    /// not `pub` — only the sandbox backend itself (same crate) ever calls this; external callers
    /// only ever SUPPLY the hook closure via `Self::with_checkout_authorization`.
    #[allow(dead_code)]
    pub(crate) fn authorize_checkout(
        &self,
        spec: &JobSpec,
        scope: CheckoutAuthorizationScope,
    ) -> Result<CheckoutAuthorizationProof, HookError> {
        match &self.checkout_authorization {
            Some(hook) => {
                hook(spec, &scope)?;
                Ok(CheckoutAuthorizationProof {
                    scope,
                    run_token_jti: spec.run_token.jti.clone(),
                })
            }
            None => Err(HookError(
                "checkout-bearing job requires a configured checkout-authorization hook, but \
                 none was provided"
                    .to_string(),
            )),
        }
    }
}
