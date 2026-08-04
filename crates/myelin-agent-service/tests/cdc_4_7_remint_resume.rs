use myelin_agent_service::{RunIdentity, RunTokenRevoker, FAIL_STATIC_W_SECS};
use myelin_flow::{DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenMinter};
use std::sync::Arc;

#[derive(Default)]
struct IdentityMintProvider {
    calls: std::sync::Mutex<Vec<(String, String, DelegationCaveats, u64)>>,
}
impl RunTokenMinter for IdentityMintProvider {
    fn mint_run_token(
        &self,
        agent_id: &str,
        run_id: &str,
        caveats: &DelegationCaveats,
        ttl_secs: u64,
    ) -> Result<RunTokenHandle, RunTokenError> {
        if ttl_secs == 0 {
            return Err(RunTokenError("non-positive TTL".into()));
        }
        let mut c = self.calls.lock().unwrap();
        let n = c.len();
        c.push((agent_id.into(), run_id.into(), caveats.clone(), ttl_secs));
        Ok(RunTokenHandle {
            token: format!("tok:{agent_id}:{run_id}:{n}"),
            jti: format!("jti:{agent_id}:{run_id}:{n}"),
            ttl_secs,
        })
    }
}

struct IdentityRevokeProvider {
    revoked: std::sync::Mutex<std::collections::HashSet<String>>,
    minted_at: i64,
    ttl_w: i64,
}
impl RunTokenRevoker for IdentityRevokeProvider {
    fn revoke(&self, jti: &str, now_secs: i64, teardown_secs: i64) -> u64 {
        let mut g = self.revoked.lock().unwrap();
        if !g.insert(jti.to_string()) {
            return 0;
        }
        (now_secs - teardown_secs).max(0) as u64
    }
    fn is_dead(&self, jti: &str, now_secs: i64) -> bool {
        self.revoked.lock().unwrap().contains(jti) || now_secs >= self.minted_at + self.ttl_w
    }
}

fn caveats() -> DelegationCaveats {
    DelegationCaveats(vec!["delegated:human-x".into(), "tenant:acme".into()])
}

#[test]
fn remint_on_resume_keeps_a_multi_day_pause_attributed_within_ttl() {
    let provider = Arc::new(IdentityMintProvider::default());
    let mut id = RunIdentity::new(provider.clone(), "psn:agent-7", "R1", caveats());

    id.mint_at_dispatch(1000, 259_200).expect("dispatch mint");
    let dispatch_jti = id.current().unwrap().jti.clone();
    assert_eq!(
        id.current().unwrap().ttl_secs,
        FAIL_STATIC_W_SECS,
        "dispatch token TTL == W"
    );

    let resume_at = 1000 + 172_800;
    let resumed = id
        .remint_on_resume(resume_at)
        .expect("re-mint on resume")
        .clone();
    assert_ne!(
        resumed.jti, dispatch_jti,
        "the re-mint is a FRESH token (not the dispatch one)"
    );
    assert!(
        resumed.ttl_secs <= FAIL_STATIC_W_SECS,
        "re-mint TTL within the W bound"
    );

    let calls = provider.calls.lock().unwrap();
    assert_eq!(
        calls.len(),
        2,
        "one mint at dispatch + one re-mint on resume"
    );
    let (_, _, cav, _) = calls[1].clone();
    assert!(
        cav.0.contains(&"run:R1".to_string()),
        "per-run attenuation re-carried on resume"
    );
    assert!(
        cav.0.contains(&"delegated:human-x".to_string()),
        "SAME delegation grant on resume"
    );
    drop(calls);

    assert!(
        !id.attribution_window().has_unattributed_gap(),
        "0 unattributed window across the pause"
    );
    assert!(
        id.attribution_window().max_segment_width() <= FAIL_STATIC_W_SECS,
        "no token widened the attribution window beyond W"
    );
    assert_eq!(id.reminted(), 1, "exactly one re-mint on the resume");
}

#[test]
fn revoke_after_resume_is_idempotent_even_on_crash() {
    let provider = Arc::new(IdentityMintProvider::default());
    let mut id = RunIdentity::new(provider, "psn:agent-7", "R2", caveats());
    id.mint_at_dispatch(1000, 100_000).expect("dispatch");
    id.remint_on_resume(2000).expect("resume re-mint");
    let fresh_jti = id.current().unwrap().jti.clone();

    let revoker = IdentityRevokeProvider {
        revoked: std::sync::Mutex::new(std::collections::HashSet::new()),
        minted_at: 2000,
        ttl_w: FAIL_STATIC_W_SECS as i64,
    };
    assert_eq!(
        id.revoke_on_teardown(&revoker, 2100, 2100),
        0,
        "first revoke lands"
    );
    assert!(
        revoker.is_dead(&fresh_jti, 2100),
        "the fresh token is dead after revoke"
    );
    assert_eq!(
        id.revoke_on_teardown(&revoker, 2105, 2100),
        0,
        "a re-revoke is a no-op (idempotent)"
    );

    let other = IdentityRevokeProvider {
        revoked: std::sync::Mutex::new(std::collections::HashSet::new()),
        minted_at: 2000,
        ttl_w: FAIL_STATIC_W_SECS as i64,
    };
    assert!(
        !other.is_dead("jti:never", 2000),
        "not yet expired before W"
    );
    assert!(
        other.is_dead("jti:never", 2000 + FAIL_STATIC_W_SECS as i64),
        "auto-expires by minted_at + W"
    );
}

#[test]
fn remint_past_the_run_deadline_is_refused() {
    let provider = Arc::new(IdentityMintProvider::default());
    let mut id = RunIdentity::new(provider, "psn:agent-7", "R3", caveats());
    id.mint_at_dispatch(1000, 300).expect("dispatch");
    let err = id
        .remint_on_resume(1400)
        .expect_err("resume past the deadline is refused");
    assert!(
        err.to_string().contains("no remaining life"),
        "refused LOUD: {err}"
    );
}
