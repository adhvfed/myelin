//! # `remint` — mint_run_token mid-workflow re-mint on resume (P-FLOW-17 → P-213, M2)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/durable-workflow.md` §6.2 (mid-workflow
//! `mint_run_token` re-mint on resume — *token life == activity life, NOT the days-long workflow
//! life; the workflow never holds a long-lived privileged token*) + §5.2 (the contract-4.7 pin —
//! `mint_run_token` callable mid-workflow on resume).
//!
//! **Contract-index cluster:** OWNS nothing. CONSUMES contract 4.7
//! (`mint_run_token(agent_id, run_id, delegation_caveats, ttl) → token` — Identity, §4/§11) in its
//! *mid-workflow re-mint on resume* form. The provider is Identity's
//! `IdentityService::mint_run_token` (body → P-ID-18, M1); this engine is the **consumer** that
//! calls it on every resume across a multi-day wait.
//!
//! ## The doctrine this closes (VISION / EI-01 §3 — no long-lived privileged token)
//!
//! A multi-day HITL workflow (a [`crate::request_approval_and_wait`] approval card, a
//! [`crate::WfCtx::schedule_and_run_job`] long-park) is `state=waiting` for hours-to-days, holding
//! **no runtime**. Its per-run agent token's TTL is bounded by the fail-static window W (§10,
//! ≈ 5 min) — far shorter than the wait. So the token a run held when it PARKED has **expired** by
//! the time the wait resumes. If the engine re-used that token it would either be dead (a fail-static
//! deny) or — worse — the workflow would have had to hold a long-lived privileged token across the
//! wait, which the doctrine forbids: *a multi-day workflow holds no long-lived privileged token*.
//!
//! The resolution (contract 4.7, recon §1): on EVERY resume the workflow **re-mints** a fresh
//! short-lived attenuated per-run token via `mint_run_token`, so **token life == ACTIVITY life**
//! (the active burst between two parks), not the days-long WORKFLOW life. The workflow holds the
//! token only while it is actually running; across the park it holds nothing.
//!
//! ## What this prompt (P-FLOW-17) ships — the re-mint, wired into both resume paths
//!
//! - [`RunTokenMinter`] — the engine's view of the contract-4.7 mint surface (`mint_run_token`). A
//!   trait so `myelin-flow` does NOT take a production runtime dependency on `myelin-identity` (the
//!   DAG stays acyclic — the same decoupling [`crate::JobRunner`] / [`crate::EffectApplier`] use).
//!   The Identity `IdentityService::mint_run_token` consumer is paired with this seam in the CDC
//!   `tests/cdc_4_7_remint.rs` (dev-dep only).
//! - [`RunTokenLease`] — the per-run mint context [`crate::WfCtx::with_run_identity`] holds: the
//!   minter + the agent principal id + the delegation caveats (the attenuate-only grant chain) + the
//!   TTL bound (the short fail-static window — *token life == activity life*). Cheap-to-clone.
//! - [`RunTokenHandle`] — a freshly minted per-run token (the opaque bearer material + its `jti`),
//!   carrying the TTL it was minted under so a caller can assert *short-lived, not workflow-lifetime*.
//! - [`crate::WfCtx::remint_on_resume`] — re-mints a fresh token. Called by the resume legs of
//!   [`crate::WfCtx::wait_for_signal`] (the HITL approval resume) and
//!   [`crate::WfCtx::schedule_and_run_job`] (the long-park `job.done` resume): when a wait that had
//!   PARKED resumes (a `Signalled` / `TimedOut` after a prior `signal_waited`), the engine re-mints
//!   BEFORE the body runs the resumed work, so the resumed activity executes under a fresh token.
//!
//! ## The three structural properties (the §6.2 gate)
//!
//! 1. **A resume re-mints a SHORT-LIVED token, not the workflow-lifetime token.** The minted token's
//!    TTL is the fail-static window (W, ≈ 5 min), bounded by [`RunTokenLease::ttl`] — never the
//!    days-long workflow life. [`RunTokenHandle::ttl`] carries it so the gate can assert it.
//! 2. **The re-minted token is ATTENUATED to the run's scope.** The mint carries the lease's
//!    [`DelegationCaveats`] (the agent.policy ∩ delegation ∩ tenant.policy chain, §6) AND a per-run
//!    caveat naming THIS run — so the token cannot act outside the run it was minted for.
//! 3. **A resume WITHOUT a prior token still re-mints.** The re-mint is unconditional on resume — a
//!    run that parked before it ever minted a token (or whose first activity IS post-resume) still
//!    gets a fresh per-run token; the engine never assumes a token survives a park.
//!
//! ## FLOORS named (recorded, not owned here)
//!
//! - **The `mint_run_token` BODY is Identity's** (contract 4.7, body → P-ID-18 M1). This engine is
//!   the CONSUMER; it drives the frozen surface. The CDC pairs it with a real `IdentityService`
//!   impl (the M0 stub returns `NotYetImplemented`; the CDC fixture is a real minting impl that
//!   proves the engine calls the surface with the right args).
//! - **The E2E-2 re-mint assertion** (the spine that asserts the re-mint end-to-end across the whole
//!   multi-day HITL + long-park flow) lands at **P-FLOW-27**. This prompt ships the engine-side
//!   re-mint + the structural gate; the spine-level assertion is recorded there.
//! - **`revoke` of the prior (expired) token** is unnecessary — it expired on its own short TTL (the
//!   fail-static window is the revocation SLA bound, §10/C11). The engine does not explicitly revoke
//!   a token that the TTL already killed; an explicit `revoke` path (suspend-a-principal) is
//!   Identity's (contract 4.7, P-ID-14).

use crate::wfctx::WfCtx;

/// **The delegation caveats carried into a per-run token mint (contract 4.7; §6).** The delegating
/// human's grant expressed as the token's caveat chain (attenuate-only — a mint can only NARROW the
/// chain, never widen it). The engine carries it opaquely (each caveat is an opaque
/// macaroon/biscuit-class string); it does NOT interpret the chain — Identity's `mint_run_token`
/// composes `agent.policy ∩ delegation ∩ tenant.policy` (§6, AG-2). Mirrors the shape of
/// `myelin_identity::DelegationCaveats` so the CDC adapter maps 1:1 without `myelin-flow` taking a
/// production dependency on `myelin-identity` (the DAG stays acyclic).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DelegationCaveats(pub Vec<String>);

/// **A freshly minted per-run attenuated capability token (contract 4.7) — the result of a re-mint
/// on resume (§6.2).** Carries the opaque bearer material + its revocation id (`jti`) AND the TTL it
/// was minted under (the short fail-static window — *token life == activity life*), so a caller /
/// the gate can assert it is SHORT-LIVED (not the days-long workflow life) and attenuated per-run.
#[derive(Clone, PartialEq, Eq)]
pub struct RunTokenHandle {
    /// The opaque bearer material the resumed activity authenticates with.
    pub token: String,
    /// The token's revocation id (the `jti` the denylist keys on, §S7).
    pub jti: String,
    /// **The TTL this token was minted under, in seconds (the fail-static window W, §10).** It is
    /// the SHORT activity-life bound — never the days-long workflow life. The §6.2 gate asserts this
    /// is `<=` the lease's bound (a short-lived token) and `> 0`.
    pub ttl_secs: u64,
}

impl core::fmt::Debug for RunTokenHandle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RunTokenHandle")
            .field("token", &"<redacted>")
            .field("jti", &"<redacted>")
            .field("ttl_secs", &self.ttl_secs)
            .finish()
    }
}

/// **The engine's view of the contract-4.7 `mint_run_token` surface (CONSUMED, §6.2).** A trait so
/// `myelin-flow` does NOT take a production runtime dependency on `myelin-identity` (the DAG stays
/// acyclic — the same decoupling [`crate::JobRunner`] / [`crate::EffectApplier`] use). The Identity
/// `IdentityService::mint_run_token` provider is paired with this consumer seam in the CDC
/// `tests/cdc_4_7_remint.rs` (dev-dep only).
///
/// A re-mint mints a per-run attenuated token scoped to `(agent_id, run_id)` with the delegation
/// `caveats` and a TTL of `ttl_secs` (the short fail-static window — *token life == activity life*).
/// The mint is the ONLY token path: the engine never fabricates a token, it always asks Identity.
pub trait RunTokenMinter {
    /// **`mint_run_token(agent_id, run_id, caveats, ttl) → token` (contract 4.7).** Mint a fresh
    /// per-run attenuated token for `(agent_id, run_id)` with the delegation `caveats` and a TTL of
    /// `ttl_secs` seconds. Returns the [`RunTokenHandle`] (the bearer material + `jti` + the TTL it
    /// was minted under), or a [`RunTokenError`] if the mint failed (Identity unavailable / refused).
    fn mint_run_token(
        &self,
        agent_id: &str,
        run_id: &str,
        caveats: &DelegationCaveats,
        ttl_secs: u64,
    ) -> Result<RunTokenHandle, RunTokenError>;
}

/// An error from a re-mint (the contract-4.7 mint surface failed). A machine error string (NO subject
/// data) — surfaced LOUD as a [`crate::WfError::CoCommit`] so a failed re-mint never silently runs a
/// resumed activity under a stale/absent token (EI-01 §2 — never a silent correctness bug).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunTokenError(pub String);

impl core::fmt::Display for RunTokenError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "mint_run_token (re-mint on resume) failed: {}", self.0)
    }
}

impl std::error::Error for RunTokenError {}

/// **The per-run mint context a [`WfCtx`] holds so it can re-mint a fresh token on resume (contract
/// 4.7, §6.2).** A cheap-to-clone handle over the [`RunTokenMinter`] + the agent principal id the
/// token is minted for + the delegation `caveats` (the attenuate-only grant chain) + the TTL bound
/// (the short fail-static window — *token life == activity life*). Supplied via
/// [`WfCtx::with_run_identity`].
///
/// **The TTL is the WHOLE point (§6.2):** [`RunTokenLease::ttl_secs`] is the SHORT fail-static window
/// (W ≈ 5 min, §10), NOT the days-long workflow life. Every re-mint mints under THIS short TTL, so a
/// token never outlives the active burst between two parks.
#[derive(Clone)]
pub struct RunTokenLease {
    minter: std::sync::Arc<dyn RunTokenMinter + Send + Sync>,
    /// The agent principal the per-run token is minted FOR (the run's agent identity).
    agent_id: String,
    /// The delegation caveats the mint attenuates the token with (the §6 grant chain).
    caveats: DelegationCaveats,
    /// **The TTL bound the token is minted under, in seconds (the fail-static window W, §10).** The
    /// SHORT activity-life bound — never the days-long workflow life. A mint under this TTL is what
    /// makes *token life == activity life* true.
    ttl_secs: u64,
}

impl RunTokenLease {
    /// **The default fail-static TTL window (W ≈ 5 min, §10/C11).** The short activity-life bound a
    /// re-mint defaults to — far shorter than any multi-day wait. Identity's `FailStaticBound`
    /// proposes the same `300`; the structural bound holds regardless of the legal ratification of
    /// the number (L-1).
    pub const DEFAULT_TTL_SECS: u64 = 300;

    /// Build a lease over a `minter` for `agent_id` with the delegation `caveats`, minting under the
    /// default short fail-static TTL ([`RunTokenLease::DEFAULT_TTL_SECS`] — *token life == activity
    /// life*).
    pub fn new(
        minter: std::sync::Arc<dyn RunTokenMinter + Send + Sync>,
        agent_id: impl Into<String>,
        caveats: DelegationCaveats,
    ) -> RunTokenLease {
        RunTokenLease {
            minter,
            agent_id: agent_id.into(),
            caveats,
            ttl_secs: Self::DEFAULT_TTL_SECS,
        }
    }

    /// Override the TTL bound (the fail-static window). Chainable on [`RunTokenLease::new`]. A TTL of
    /// 0 is clamped to 1 (a token must live for SOME positive activity burst; a 0-TTL token is dead
    /// on arrival — never legal).
    pub fn with_ttl_secs(mut self, ttl_secs: u64) -> RunTokenLease {
        self.ttl_secs = ttl_secs.max(1);
        self
    }

    /// The TTL bound the lease mints under (the short fail-static window — *token life == activity
    /// life*). The §6.2 gate reads it to assert a re-mint is SHORT-LIVED.
    pub fn ttl_secs(&self) -> u64 {
        self.ttl_secs
    }

    /// The agent principal id the per-run token is minted for.
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    /// **Re-mint a fresh per-run attenuated token for `run_id` (contract 4.7, §6.2).** Mints under
    /// the lease's short TTL (so token life == activity life) with the delegation caveats ATTENUATED
    /// by a per-run caveat naming `run_id` (so the token cannot act outside THIS run — property 2).
    /// The mint is the contract-4.7 surface ([`RunTokenMinter::mint_run_token`]); a failure surfaces
    /// the error LOUD (never a silent run under a stale token).
    fn mint_for_run(&self, run_id: &str) -> Result<RunTokenHandle, RunTokenError> {
        // ATTENUATE per-run (property 2): the minted token's caveat chain carries the lease's
        // grant chain PLUS a per-run caveat naming THIS run, so the token is scoped to the run it was
        // minted for (it cannot act outside this run). Attenuate-only — we only ADD a caveat (narrow),
        // never remove one (the chain can only get tighter, §6 / AG-2).
        let mut caveats = self.caveats.clone();
        caveats.0.push(format!("run:{run_id}"));
        self.minter
            .mint_run_token(&self.agent_id, run_id, &caveats, self.ttl_secs)
    }
}

impl WfCtx {
    /// **Supply the per-run mint context so this `WfCtx` can re-mint a fresh token on resume (contract
    /// 4.7, §6.2).** A `WfCtx` built WITHOUT this cannot re-mint — [`WfCtx::remint_on_resume`] returns
    /// a [`crate::WfError::CoCommit`] naming the missing lease rather than silently running a resumed
    /// activity under no token (a missing re-mint is a silent privilege bug, EI-01 §2). The dispatcher
    /// calls this when it builds the drive's `WfCtx` from the run's agent identity so a resume across a
    /// multi-day wait re-mints a short-lived attenuated per-run token (token life == activity life).
    /// Chainable on `begin`/`resume`.
    pub fn with_run_identity(mut self, lease: RunTokenLease) -> Self {
        self.run_identity = Some(lease);
        self
    }

    /// The mint context (if one was supplied via [`WfCtx::with_run_identity`]). `None` on a `WfCtx`
    /// with no run-identity wired (a body that never crosses a multi-day wait, or a unit test of the
    /// pure activity surface).
    pub(crate) fn run_identity(&self) -> Option<&RunTokenLease> {
        self.run_identity.as_ref()
    }

    /// **`remint_on_resume()` (contract 4.7 CONSUMED, §6.2) — re-mint a fresh short-lived attenuated
    /// per-run token on resume.** Called by the resume legs of [`WfCtx::wait_for_signal`] (the HITL
    /// approval resume) and [`WfCtx::schedule_and_run_job`] (the long-park `job.done` resume) BEFORE
    /// the resumed body runs, so the resumed activity executes under a FRESH token whose life ==
    /// activity life (not the days-long workflow life — the workflow held NO token across the park).
    ///
    /// **The three properties (§6.2):**
    /// 1. the minted token is SHORT-LIVED (the lease's fail-static TTL, not the workflow life);
    /// 2. it is ATTENUATED per-run (a per-run caveat scopes it to THIS run);
    /// 3. a resume re-mints UNCONDITIONALLY — even if no token survived the park (the engine never
    ///    assumes a token outlives a wait).
    ///
    /// Returns the freshly minted [`RunTokenHandle`] (the bearer material + `jti` + the TTL it was
    /// minted under). A `WfCtx` with NO run-identity ([`WfCtx::with_run_identity`]) returns a loud
    /// [`crate::WfError::CoCommit`] — never a silent run under no token. A mint failure (Identity
    /// unavailable / refused) surfaces the [`RunTokenError`] as a `CoCommit` too (the resumed activity
    /// must NOT run under a stale/absent token).
    ///
    /// **Replay-safety:** the re-mint is NOT a journaled command — it mints a fresh (different)
    /// token on each drive BY DESIGN (a token is ephemeral, never replayed; re-deriving the SAME
    /// token would defeat the short-TTL point). It does not advance the command counter, so the
    /// deterministic replay cursor over `wf_history` is untouched. Re-minting is idempotent in the
    /// only sense that matters: each drive that resumes gets its OWN fresh short-lived token.
    pub fn remint_on_resume(&mut self) -> crate::WfResult<RunTokenHandle> {
        let lease = self.run_identity().cloned().ok_or_else(|| {
            crate::WfError::CoCommit(
                "remint_on_resume requires a run-identity lease (WfCtx::with_run_identity) — a \
                 resume across a multi-day wait must re-mint a fresh per-run token (contract 4.7, \
                 §6.2); refusing to run a resumed activity under no token"
                    .into(),
            )
        })?;
        let run_id = self.run_id().to_string();
        let handle = lease
            .mint_for_run(&run_id)
            .map_err(|e| crate::WfError::CoCommit(e.to_string()))?;
        self.reminted_tokens += 1;
        Ok(handle)
    }

    /// **The count of fresh per-run tokens re-minted on this drive (the §6.2 re-mint probe).** Each
    /// resume across a multi-day wait re-mints exactly one fresh short-lived token; the gate reads it
    /// to assert a resume DID re-mint (a regression that drops the re-mint reads `0` and reds the
    /// gate). A drive that never resumed (a cold first drive that parked, or a pure non-waiting body)
    /// reads `0`.
    pub fn reminted_tokens(&self) -> u64 {
        self.reminted_tokens
    }

    /// **Re-mint a fresh per-run token IF a run-identity lease is wired (the resume-leg hook, §6.2).**
    /// Called by the resume legs of [`WfCtx::wait_for_signal`] + [`WfCtx::schedule_and_run_job`] when a
    /// previously-parked wait RESUMES (a buffered signal is consumed, or the timeout fires) — the
    /// resumed body is about to run, so it must run under a FRESH short-lived token (token life ==
    /// activity life, §6.2). A `WfCtx` with NO lease wired (a unit test of the pure wait surface, an
    /// un-privileged body) SKIPS the re-mint silently — there is no token to re-mint when no agent
    /// identity is attached. A wired lease whose mint FAILS surfaces the error LOUD (the resumed
    /// activity must not run under a stale/absent token).
    ///
    /// This is the engine-internal counterpart to the public [`WfCtx::remint_on_resume`]: the public
    /// form is the explicit body-callable surface (it requires a lease, erroring if absent); this
    /// internal form is the automatic resume-leg hook (it re-mints only when a lease is present, so
    /// the broad existing wait/long-park tests that wire no identity are unaffected).
    pub(crate) fn remint_if_resuming(&mut self) -> crate::WfResult<()> {
        if self.run_identity().is_none() {
            // no agent identity attached — there is no privileged token to re-mint (an un-privileged
            // body, or a unit test of the pure wait surface). Skip silently.
            return Ok(());
        }
        // a lease is wired — re-mint a fresh short-lived attenuated per-run token (token life ==
        // activity life). A mint failure surfaces LOUD (the resumed activity must not run under a
        // stale/absent token).
        self.remint_on_resume().map(|_handle| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{WfCtx, WfJournal};
    use myelin_events::{
        Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    #[test]
    fn run_token_handle_debug_redacts_bearer_and_jti() {
        let handle = RunTokenHandle {
            token: "secret-bearer".into(),
            jti: "secret-jti".into(),
            ttl_secs: 300,
        };
        let rendered = format!("{handle:?}");
        assert!(!rendered.contains("secret-bearer"));
        assert!(!rendered.contains("secret-jti"));
        assert!(rendered.contains("<redacted>"));
        assert!(rendered.contains("ttl_secs: 300"));
    }

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: tenant(),
            region: region(),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Service,
                tenant(),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }
    fn minter() -> Arc<dyn IdMinter> {
        Arc::new(MonotonicMinter::new())
    }

    /// **The re-mint minter fixture (the contract-4.7 `mint_run_token` consumer side).** It mints a
    /// fresh token per call (a monotone counter makes each token DISTINCT — a re-mint is a NEW token,
    /// never the prior one), RECORDS the `(agent_id, run_id, caveats, ttl)` it was called with (so a
    /// test can assert the short TTL + the per-run attenuation), and counts the mints (so a test can
    /// prove exactly-one-re-mint-per-resume).
    #[derive(Default)]
    struct RecordingMinter {
        calls: AtomicU64,
        last: std::sync::Mutex<Option<(String, String, DelegationCaveats, u64)>>,
    }
    impl RunTokenMinter for RecordingMinter {
        fn mint_run_token(
            &self,
            agent_id: &str,
            run_id: &str,
            caveats: &DelegationCaveats,
            ttl_secs: u64,
        ) -> Result<RunTokenHandle, RunTokenError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last.lock().unwrap() =
                Some((agent_id.into(), run_id.into(), caveats.clone(), ttl_secs));
            Ok(RunTokenHandle {
                // a fresh, DISTINCT token per mint (the monotone counter) — a re-mint is never the
                // prior token (the whole point: token life == activity life, a new short-lived token).
                token: format!("tok-{run_id}-{n}"),
                jti: format!("jti-{run_id}-{n}"),
                ttl_secs,
            })
        }
    }

    fn lease(minter: Arc<RecordingMinter>) -> RunTokenLease {
        RunTokenLease::new(
            minter,
            "agent://acme/agent/triage",
            DelegationCaveats(vec!["tenant:acme".into()]),
        )
    }

    fn begin_with(outbox: &OutboxStore, journal: WfJournal, lease: RunTokenLease) -> WfCtx {
        WfCtx::begin(
            outbox,
            minter(),
            journal,
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
        )
        .with_run_identity(lease)
    }

    /// **A re-mint yields a SHORT-LIVED token, not the workflow-lifetime token (property 1, §6.2).**
    /// The minted token's TTL is the lease's fail-static window (the short activity-life bound), NOT
    /// the days-long workflow life. The default is the 5-min W (§10).
    #[test]
    fn remint_yields_a_short_lived_token_not_the_workflow_lifetime() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let m = Arc::new(RecordingMinter::default());
        let mut ctx = begin_with(&outbox, journal, lease(m.clone()));

        let handle = ctx.remint_on_resume().expect("re-mint a fresh token");
        assert_eq!(
            handle.ttl_secs,
            RunTokenLease::DEFAULT_TTL_SECS,
            "the re-mint is SHORT-LIVED (the fail-static W), not the days-long workflow life"
        );
        assert!(
            handle.ttl_secs > 0,
            "a token must live for some positive activity burst"
        );
        // far shorter than even an hour — let alone a multi-day wait.
        assert!(
            handle.ttl_secs <= 3600,
            "the activity-life TTL is far shorter than a multi-day wait"
        );
        assert_eq!(
            m.calls.load(Ordering::SeqCst),
            1,
            "exactly one mint per resume"
        );
    }

    /// **The re-minted token is ATTENUATED to the run's scope (property 2, §6.2).** The mint carries
    /// the lease's delegation caveats PLUS a per-run caveat naming THIS run — so the token cannot act
    /// outside the run it was minted for. Attenuate-only: the chain only got TIGHTER.
    #[test]
    fn remint_is_attenuated_to_the_run_scope() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let m = Arc::new(RecordingMinter::default());
        let mut ctx = begin_with(&outbox, journal, lease(m.clone()));

        ctx.remint_on_resume().expect("re-mint");
        let last = m.last.lock().unwrap().clone().expect("a mint was recorded");
        let (agent_id, run_id, caveats, _ttl) = last;
        assert_eq!(
            agent_id, "agent://acme/agent/triage",
            "minted for the run's agent"
        );
        assert_eq!(run_id, "R1", "minted for THIS run");
        assert!(
            caveats.0.contains(&"run:R1".to_string()),
            "the token is attenuated by a per-run caveat (scoped to THIS run): {caveats:?}"
        );
        assert!(
            caveats.0.contains(&"tenant:acme".to_string()),
            "the lease's grant chain is carried (attenuate-only — the chain only got tighter)"
        );
    }

    /// **A resume WITHOUT a prior token still re-mints (property 3, §6.2).** The re-mint is
    /// unconditional on resume — a run that never minted a token before (or whose first activity is
    /// post-resume) still gets a fresh per-run token. The engine never assumes a token survives a
    /// park.
    #[test]
    fn a_resume_without_a_prior_token_still_remints() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let m = Arc::new(RecordingMinter::default());
        // NO prior mint — the run holds no token at all.
        let mut ctx = begin_with(&outbox, journal, lease(m.clone()));
        assert_eq!(m.calls.load(Ordering::SeqCst), 0, "no token minted yet");

        let handle = ctx
            .remint_on_resume()
            .expect("re-mint even with no prior token");
        assert_eq!(
            handle.token, "tok-R1-0",
            "a fresh token was minted from scratch"
        );
        assert_eq!(
            m.calls.load(Ordering::SeqCst),
            1,
            "the resume re-minted unconditionally"
        );
        assert_eq!(
            ctx.reminted_tokens(),
            1,
            "the re-mint probe counts one fresh token"
        );
    }

    /// **Two resumes mint two DISTINCT short-lived tokens (token life == activity life).** Each
    /// active burst between two parks gets its OWN fresh token; a re-mint is never the prior token.
    #[test]
    fn two_resumes_mint_two_distinct_short_lived_tokens() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let m = Arc::new(RecordingMinter::default());
        let mut ctx = begin_with(&outbox, journal, lease(m.clone()));

        let h1 = ctx.remint_on_resume().expect("first resume");
        let h2 = ctx.remint_on_resume().expect("second resume");
        assert_ne!(
            h1.token, h2.token,
            "each resume mints a DISTINCT fresh token (not the prior one)"
        );
        assert_ne!(h1.jti, h2.jti, "distinct jti per re-mint");
        assert_eq!(
            ctx.reminted_tokens(),
            2,
            "two fresh tokens across two activity bursts"
        );
    }

    /// **A `WfCtx` with NO run-identity refuses to re-mint LOUD (never a silent run under no token,
    /// EI-01 §2).** A body that reaches a resume with no lease wired surfaces a loud `CoCommit` rather
    /// than silently running a resumed activity under no/expired token.
    #[test]
    fn remint_without_a_lease_is_a_loud_error() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let mut ctx = WfCtx::begin(
            &outbox,
            minter(),
            journal,
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
        ); // NO with_run_identity.
        let err = ctx.remint_on_resume().expect_err("no lease → loud error");
        assert!(
            matches!(err, crate::WfError::CoCommit(ref msg) if msg.contains("re-mint a fresh per-run token")),
            "the missing lease is a loud CoCommit, got {err:?}"
        );
    }

    /// **A mint failure (Identity unavailable / refused) surfaces LOUD — the resumed activity does NOT
    /// run under a stale token (§6.2, EI-01 §2).** A minter that errors makes `remint_on_resume`
    /// return a loud `CoCommit` (the resumed work is blocked until a token can be minted).
    #[test]
    fn a_mint_failure_surfaces_loud() {
        struct FailingMinter;
        impl RunTokenMinter for FailingMinter {
            fn mint_run_token(
                &self,
                _a: &str,
                _r: &str,
                _c: &DelegationCaveats,
                _t: u64,
            ) -> Result<RunTokenHandle, RunTokenError> {
                Err(RunTokenError("identity unavailable (fail-static)".into()))
            }
        }
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let lease = RunTokenLease::new(
            Arc::new(FailingMinter),
            "agent://acme/agent/x",
            DelegationCaveats::default(),
        );
        let mut ctx = begin_with(&outbox, journal, lease);
        let err = ctx.remint_on_resume().expect_err("a mint failure is loud");
        assert!(
            matches!(err, crate::WfError::CoCommit(ref m) if m.contains("identity unavailable")),
            "the mint failure surfaces loud, got {err:?}"
        );
    }

    /// **The re-mint does NOT advance the deterministic command counter (replay-safety, §4.1).** A
    /// token is ephemeral (a fresh one per drive — never journaled/replayed); the re-mint must not
    /// perturb the `wf_history` replay cursor. After a re-mint, the next activity still lands at
    /// command position `:0` (the re-mint consumed no command id).
    #[test]
    fn remint_does_not_advance_the_command_counter() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let m = Arc::new(RecordingMinter::default());
        let mut ctx = begin_with(&outbox, journal.clone(), lease(m));

        ctx.remint_on_resume().expect("re-mint");
        // the FIRST activity after a re-mint still lands at command :0 (the re-mint is not a command).
        ctx.activity(crate::RetryPolicy::default_policy(), |_idem, _att| {
            Ok(vec![myelin_refs::ArtifactRef("myelin://acme/out".into())])
        })
        .expect("activity after re-mint");
        ctx.commit().expect("co-commit");
        let hist = journal.history_for(&tenant(), "R1");
        assert_eq!(
            hist.len(),
            1,
            "one history row (the activity) — the re-mint journaled nothing"
        );
        assert_eq!(
            hist[0].command_id, "agent.run:0",
            "the activity is at :0 — the re-mint consumed no command"
        );
    }

    /// **`with_ttl_secs` overrides the TTL bound; a 0 is clamped to a live 1 (never a dead-on-arrival
    /// token).** A lease can be pinned to a shorter/longer activity window; a 0 TTL is clamped up.
    #[test]
    fn with_ttl_secs_overrides_and_clamps_zero() {
        let m = Arc::new(RecordingMinter::default());
        let l = lease(m).with_ttl_secs(60);
        assert_eq!(l.ttl_secs(), 60, "the TTL override holds");
        let m2 = Arc::new(RecordingMinter::default());
        let l0 = lease(m2).with_ttl_secs(0);
        assert_eq!(
            l0.ttl_secs(),
            1,
            "a 0 TTL is clamped to a live 1 (never dead-on-arrival)"
        );
    }
}
