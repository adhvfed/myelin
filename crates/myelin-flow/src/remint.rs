use crate::wfctx::WfCtx;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DelegationCaveats(pub Vec<String>);

#[derive(Clone, PartialEq, Eq)]
pub struct RunTokenHandle {
    pub token: String,
    pub jti: String,
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

impl Drop for RunTokenHandle {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.token.zeroize();
        self.jti.zeroize();
    }
}

pub trait RunTokenMinter {
    fn mint_run_token(
        &self,
        agent_id: &str,
        run_id: &str,
        caveats: &DelegationCaveats,
        ttl_secs: u64,
    ) -> Result<RunTokenHandle, RunTokenError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunTokenError(pub String);

impl core::fmt::Display for RunTokenError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "mint_run_token (re-mint on resume) failed: {}", self.0)
    }
}

impl std::error::Error for RunTokenError {}

#[derive(Clone)]
pub struct RunTokenLease {
    minter: std::sync::Arc<dyn RunTokenMinter + Send + Sync>,
    agent_id: String,
    caveats: DelegationCaveats,
    ttl_secs: u64,
}

impl RunTokenLease {
    pub const DEFAULT_TTL_SECS: u64 = 300;

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

    pub fn with_ttl_secs(mut self, ttl_secs: u64) -> RunTokenLease {
        self.ttl_secs = ttl_secs.max(1);
        self
    }

    pub fn ttl_secs(&self) -> u64 {
        self.ttl_secs
    }

    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    fn mint_for_run(&self, run_id: &str) -> Result<RunTokenHandle, RunTokenError> {
        let mut caveats = self.caveats.clone();
        caveats.0.push(format!("run:{run_id}"));
        self.minter
            .mint_run_token(&self.agent_id, run_id, &caveats, self.ttl_secs)
    }
}

impl WfCtx {
    pub fn with_run_identity(mut self, lease: RunTokenLease) -> Self {
        self.run_identity = Some(lease);
        self
    }

    pub(crate) fn run_identity(&self) -> Option<&RunTokenLease> {
        self.run_identity.as_ref()
    }

    pub fn remint_on_resume(&mut self) -> crate::WfResult<RunTokenHandle> {
        let lease = self.run_identity().cloned().ok_or_else(|| {
            crate::WfError::CoCommit(
                "remint_on_resume requires a run-identity lease (WfCtx::with_run_identity) - a \
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

    pub fn reminted_tokens(&self) -> u64 {
        self.reminted_tokens
    }

    pub(crate) fn remint_if_resuming(&mut self) -> crate::WfResult<()> {
        if self.run_identity().is_none() {
            return Ok(());
        }
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
        assert!(core::mem::needs_drop::<RunTokenHandle>());
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
            "the lease's grant chain is carried (attenuate-only - the chain only got tighter)"
        );
    }

    #[test]
    fn a_resume_without_a_prior_token_still_remints() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let m = Arc::new(RecordingMinter::default());
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
        );
        let err = ctx.remint_on_resume().expect_err("no lease → loud error");
        assert!(
            matches!(err, crate::WfError::CoCommit(ref msg) if msg.contains("re-mint a fresh per-run token")),
            "the missing lease is a loud CoCommit, got {err:?}"
        );
    }

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

    #[test]
    fn remint_does_not_advance_the_command_counter() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let m = Arc::new(RecordingMinter::default());
        let mut ctx = begin_with(&outbox, journal.clone(), lease(m));

        ctx.remint_on_resume().expect("re-mint");
        ctx.activity(crate::RetryPolicy::default_policy(), |_idem, _att| {
            Ok(vec![myelin_refs::ArtifactRef("myelin://acme/out".into())])
        })
        .expect("activity after re-mint");
        ctx.commit().expect("co-commit");
        let hist = journal.history_for(&tenant(), "R1");
        assert_eq!(
            hist.len(),
            1,
            "one history row (the activity) - the re-mint journaled nothing"
        );
        assert_eq!(
            hist[0].command_id, "agent.run:0",
            "the activity is at :0 - the re-mint consumed no command"
        );
    }

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
