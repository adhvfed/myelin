use crate::skeleton::{ChildEnv, RunTokenRevoker};
use myelin_flow::{DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenMinter};
use std::sync::Arc;

pub const FAIL_STATIC_W_SECS: u64 = 300;

pub struct RunIdentity {
    minter: Arc<dyn RunTokenMinter + Send + Sync>,
    agent_id: String,
    run_id: String,
    caveats: DelegationCaveats,
    fail_static_w: u64,
    deadline_secs: i64,
    current: Option<RunTokenHandle>,
    reminted: u64,
    window: AttributionWindow,
}

impl RunIdentity {
    pub fn new(
        minter: Arc<dyn RunTokenMinter + Send + Sync>,
        agent_id: impl Into<String>,
        run_id: impl Into<String>,
        caveats: DelegationCaveats,
    ) -> RunIdentity {
        RunIdentity {
            minter,
            agent_id: agent_id.into(),
            run_id: run_id.into(),
            caveats,
            fail_static_w: FAIL_STATIC_W_SECS,
            deadline_secs: 0,
            current: None,
            reminted: 0,
            window: AttributionWindow::new(),
        }
    }

    pub fn with_fail_static_w(mut self, w_secs: u64) -> RunIdentity {
        self.fail_static_w = w_secs.max(1);
        self
    }

    pub fn mint_at_dispatch(
        &mut self,
        now_secs: i64,
        run_life_secs: u64,
    ) -> Result<&RunTokenHandle, RunTokenError> {
        self.deadline_secs = now_secs.saturating_add(run_life_secs.min(i64::MAX as u64) as i64);
        let ttl = self.fail_static_w.min(run_life_secs.max(1));
        let handle = self.mint(ttl)?;
        let expiry = now_secs.saturating_add(ttl.min(i64::MAX as u64) as i64);
        self.window.open_segment(now_secs, expiry);
        self.current = Some(handle);
        Ok(self.current.as_ref().expect("just minted"))
    }

    pub fn remint_on_resume(&mut self, now_secs: i64) -> Result<&RunTokenHandle, RunTokenError> {
        let remaining = self.deadline_secs.saturating_sub(now_secs);
        if remaining <= 0 {
            return Err(RunTokenError(format!(
                "run {} has no remaining life at resume (deadline {} <= now {}) - refusing to \
                 re-mint past the run's own deadline (never widen the attribution window, §5.7)",
                self.run_id, self.deadline_secs, now_secs
            )));
        }
        let remaining_u = remaining as u64;
        let ttl = self.fail_static_w.min(remaining_u);
        let handle = self.mint(ttl)?;
        let expiry = now_secs.saturating_add(ttl.min(i64::MAX as u64) as i64);
        self.window.open_segment(now_secs, expiry);
        self.current = Some(handle);
        self.reminted = self.reminted.saturating_add(1);
        Ok(self.current.as_ref().expect("just re-minted"))
    }

    pub fn revoke_on_teardown(
        &self,
        revoker: &dyn RunTokenRevoker,
        now_secs: i64,
        teardown_secs: i64,
    ) -> u64 {
        match &self.current {
            Some(token) => revoker.revoke(&token.jti, now_secs, teardown_secs),
            None => 0,
        }
    }

    pub fn child_env(&self) -> Option<ChildEnv> {
        self.current.as_ref().map(|t| ChildEnv::for_run(&t.jti))
    }

    pub fn current(&self) -> Option<&RunTokenHandle> {
        self.current.as_ref()
    }

    pub fn reminted(&self) -> u64 {
        self.reminted
    }

    pub fn deadline_secs(&self) -> i64 {
        self.deadline_secs
    }

    pub fn attribution_window(&self) -> &AttributionWindow {
        &self.window
    }

    fn mint(&self, ttl: u64) -> Result<RunTokenHandle, RunTokenError> {
        let mut caveats = self.caveats.clone();
        caveats.0.push(format!("run:{}", self.run_id));
        self.minter
            .mint_run_token(&self.agent_id, &self.run_id, &caveats, ttl)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AttributionWindow {
    segments: Vec<(i64, i64)>,
}

impl AttributionWindow {
    pub fn new() -> AttributionWindow {
        AttributionWindow::default()
    }

    fn open_segment(&mut self, start: i64, expiry: i64) {
        self.segments.push((start, expiry));
    }

    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    pub fn has_unattributed_gap(&self) -> bool {
        if self.segments.is_empty() {
            return true;
        }
        self.segments.iter().any(|&(start, expiry)| expiry <= start)
    }

    pub fn max_segment_width(&self) -> u64 {
        self.segments
            .iter()
            .map(|&(start, expiry)| (expiry - start).max(0) as u64)
            .max()
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingMinter {
        calls: std::sync::Mutex<Vec<(String, String, DelegationCaveats, u64)>>,
    }
    impl RunTokenMinter for RecordingMinter {
        fn mint_run_token(
            &self,
            agent_id: &str,
            run_id: &str,
            caveats: &DelegationCaveats,
            ttl_secs: u64,
        ) -> Result<RunTokenHandle, RunTokenError> {
            let mut c = self.calls.lock().unwrap();
            let n = c.len();
            c.push((agent_id.into(), run_id.into(), caveats.clone(), ttl_secs));
            Ok(RunTokenHandle {
                token: format!("tok:{run_id}:{n}"),
                jti: format!("jti:{run_id}:{n}"),
                ttl_secs,
            })
        }
    }

    #[derive(Default)]
    struct DenylistRevoker {
        revoked: std::sync::Mutex<std::collections::HashSet<String>>,
    }
    impl RunTokenRevoker for DenylistRevoker {
        fn revoke(&self, jti: &str, now_secs: i64, teardown_secs: i64) -> u64 {
            let mut g = self.revoked.lock().unwrap();
            if !g.insert(jti.to_string()) {
                return 0;
            }
            (now_secs - teardown_secs).max(0) as u64
        }
        fn is_dead(&self, jti: &str, _now_secs: i64) -> bool {
            self.revoked.lock().unwrap().contains(jti)
        }
    }

    fn caveats() -> DelegationCaveats {
        DelegationCaveats(vec!["delegated:human-x".into(), "tenant:acme".into()])
    }

    fn identity(minter: Arc<RecordingMinter>) -> RunIdentity {
        RunIdentity::new(minter, "psn:agent-7", "R1", caveats())
    }

    #[test]
    fn mint_at_dispatch_binds_token_and_records_deadline() {
        let m = Arc::new(RecordingMinter::default());
        let mut id = identity(m.clone());
        let token = id
            .mint_at_dispatch(1000, 200)
            .expect("mint succeeds")
            .clone();
        assert_eq!(token.jti, "jti:R1:0", "the jti is bound to the run");
        assert_eq!(token.ttl_secs, 200, "token life == run life (200 < W=300)");
        assert_eq!(id.deadline_secs(), 1200, "deadline == dispatch + run life");
        let calls = m.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "exactly one mint at dispatch");
        let (agent, run, cav, ttl) = calls[0].clone();
        assert_eq!(agent, "psn:agent-7");
        assert_eq!(run, "R1");
        assert_eq!(ttl, 200);
        assert!(
            cav.0.contains(&"run:R1".to_string()),
            "per-run attenuation carried"
        );
        assert!(
            cav.0.contains(&"delegated:human-x".to_string()),
            "grant chain carried"
        );
    }

    #[test]
    fn mint_at_dispatch_clamps_to_w_when_run_life_is_longer() {
        let m = Arc::new(RecordingMinter::default());
        let mut id = identity(m);
        let token = id.mint_at_dispatch(1000, 600).expect("mint").clone();
        assert_eq!(
            token.ttl_secs, FAIL_STATIC_W_SECS,
            "TTL clamped to W when run life > W"
        );
        assert_eq!(
            id.deadline_secs(),
            1600,
            "the deadline is still the full run life"
        );
    }

    #[test]
    fn ag_d8_remint_leg_attributed_within_ttl_zero_gap_zero_leak() {
        let m = Arc::new(RecordingMinter::default());
        let mut id = identity(m.clone());
        let dispatch_at = 1000i64;
        let run_life = 259_200u64;
        id.mint_at_dispatch(dispatch_at, run_life)
            .expect("dispatch mint");
        assert_eq!(
            id.current().unwrap().ttl_secs,
            FAIL_STATIC_W_SECS,
            "dispatch token TTL == W"
        );

        let resume_at = dispatch_at + 172_800;
        let reminted = id
            .remint_on_resume(resume_at)
            .expect("re-mint on resume")
            .clone();
        assert_eq!(
            reminted.jti, "jti:R1:1",
            "a FRESH token (not the dispatch one)"
        );
        assert_ne!(reminted.jti, "jti:R1:0", "the re-mint is a NEW token");
        assert_eq!(
            reminted.ttl_secs, FAIL_STATIC_W_SECS,
            "re-mint TTL == min(W, remaining) == W"
        );
        assert!(
            reminted.ttl_secs <= FAIL_STATIC_W_SECS,
            "never widens beyond W"
        );

        let calls = m.calls.lock().unwrap();
        let (_, _, cav, ttl) = calls[1].clone();
        assert!(
            cav.0.contains(&"run:R1".to_string()),
            "per-run attenuation re-carried"
        );
        assert!(
            cav.0.contains(&"delegated:human-x".to_string()),
            "SAME grant chain on re-mint"
        );
        assert_eq!(ttl, FAIL_STATIC_W_SECS);
        drop(calls);

        assert!(
            !id.attribution_window().has_unattributed_gap(),
            "0 unattributed window"
        );
        assert_eq!(
            id.attribution_window().segment_count(),
            2,
            "dispatch + one resume segment"
        );
        assert!(
            id.attribution_window().max_segment_width() <= FAIL_STATIC_W_SECS,
            "no segment wider than W"
        );
        assert_eq!(id.reminted(), 1, "exactly one re-mint on the resume");

        let child = id.child_env().expect("a child env after re-mint");
        assert!(
            !child.leaked_shared_token(),
            "0 shared platform token leaked into the child env"
        );
        assert_eq!(
            child.run_token_jti, "jti:R1:1",
            "the child inherits the FRESH per-run jti"
        );
    }

    #[test]
    fn remint_clamps_to_remaining_run_life_near_the_deadline() {
        let m = Arc::new(RecordingMinter::default());
        let mut id = identity(m);
        id.mint_at_dispatch(1000, 600).expect("dispatch");
        let reminted = id
            .remint_on_resume(1560)
            .expect("re-mint near deadline")
            .clone();
        assert_eq!(
            reminted.ttl_secs, 40,
            "TTL clamped to the remaining run life (40 < W)"
        );
        assert_eq!(
            id.attribution_window().max_segment_width(),
            FAIL_STATIC_W_SECS,
            "the widest segment is still the dispatch W; the resume segment is only 40"
        );
        assert!(
            !id.attribution_window().has_unattributed_gap(),
            "still 0 unattributed"
        );
    }

    #[test]
    fn remint_past_the_deadline_is_a_loud_error() {
        let m = Arc::new(RecordingMinter::default());
        let mut id = identity(m);
        id.mint_at_dispatch(1000, 300).expect("dispatch");
        let err = id
            .remint_on_resume(1400)
            .expect_err("no remaining life → refuse");
        assert!(
            err.to_string().contains("no remaining life"),
            "refused LOUD: {err}"
        );
        assert_eq!(id.reminted(), 0, "no re-mint past the deadline");
    }

    #[test]
    fn two_resumes_mint_two_distinct_tokens_no_gap() {
        let m = Arc::new(RecordingMinter::default());
        let mut id = identity(m);
        id.mint_at_dispatch(1000, 100_000).expect("dispatch");
        let h1 = id.remint_on_resume(1500).expect("first resume").clone();
        let h2 = id.remint_on_resume(5000).expect("second resume").clone();
        assert_ne!(h1.jti, h2.jti, "distinct token per resume");
        assert_eq!(id.reminted(), 2, "two re-mints across two bursts");
        assert_eq!(
            id.attribution_window().segment_count(),
            3,
            "dispatch + two resumes"
        );
        assert!(
            !id.attribution_window().has_unattributed_gap(),
            "0 unattributed across both"
        );
    }

    #[test]
    fn revoke_on_teardown_is_idempotent_even_on_crash() {
        let m = Arc::new(RecordingMinter::default());
        let mut id = identity(m);
        id.mint_at_dispatch(1000, 300).expect("dispatch");
        let revoker = DenylistRevoker::default();
        assert_eq!(
            id.revoke_on_teardown(&revoker, 1000, 1000),
            0,
            "first revoke lands"
        );
        assert!(
            revoker.is_dead("jti:R1:0", 1000),
            "the current token is dead after revoke"
        );
        assert_eq!(
            id.revoke_on_teardown(&revoker, 1005, 1000),
            0,
            "re-revoke is a no-op"
        );
    }

    #[test]
    fn revoke_after_resume_targets_the_fresh_token() {
        let m = Arc::new(RecordingMinter::default());
        let mut id = identity(m);
        id.mint_at_dispatch(1000, 100_000).expect("dispatch");
        id.remint_on_resume(2000).expect("resume re-mint");
        let revoker = DenylistRevoker::default();
        id.revoke_on_teardown(&revoker, 2100, 2100);
        assert!(
            revoker.is_dead("jti:R1:1", 2100),
            "the FRESH token is revoked"
        );
        assert!(
            !revoker.is_dead("jti:R1:0", 2100),
            "the stale dispatch token was NOT re-revoked"
        );
    }

    #[test]
    fn revoke_before_dispatch_is_a_noop() {
        let m = Arc::new(RecordingMinter::default());
        let id = identity(m);
        let revoker = DenylistRevoker::default();
        assert_eq!(
            id.revoke_on_teardown(&revoker, 1000, 1000),
            0,
            "no token → no-op"
        );
        assert!(id.child_env().is_none(), "no child env before dispatch");
        assert!(id.current().is_none(), "no current token before dispatch");
    }

    #[test]
    fn unattributed_gap_predicate_is_exact() {
        let mut w = AttributionWindow::new();
        assert!(
            w.has_unattributed_gap(),
            "empty window is a gap (un-minted run)"
        );
        w.open_segment(1000, 1300);
        assert!(
            !w.has_unattributed_gap(),
            "a positive segment is attributed"
        );
        w.open_segment(2000, 2040);
        assert!(
            !w.has_unattributed_gap(),
            "two positive segments are attributed"
        );
        let mut bad = AttributionWindow::new();
        bad.open_segment(1000, 1000);
        assert!(
            bad.has_unattributed_gap(),
            "a 0-length segment is an unattributed instant (kills <=)"
        );
    }

    #[test]
    fn max_segment_width_is_exact() {
        let mut w = AttributionWindow::new();
        assert_eq!(
            w.max_segment_width(),
            0,
            "empty window has width 0 (kills -> 1)"
        );
        w.open_segment(1000, 1040);
        assert_eq!(w.max_segment_width(), 40);
        w.open_segment(2000, 2300);
        assert_eq!(
            w.max_segment_width(),
            300,
            "the MAX (kills a first/last/min mutant)"
        );
        w.open_segment(3000, 3010);
        assert_eq!(
            w.max_segment_width(),
            300,
            "a narrower segment does not lower the max"
        );
    }

    #[test]
    fn with_fail_static_w_overrides_and_clamps_zero() {
        let m = Arc::new(RecordingMinter::default());
        let mut id = identity(m).with_fail_static_w(0);
        let token = id.mint_at_dispatch(1000, 100).expect("mint").clone();
        assert_eq!(token.ttl_secs, 1, "W clamped to 1 (never dead-on-arrival)");
    }
}
