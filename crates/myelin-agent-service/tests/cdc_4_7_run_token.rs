use myelin_agent_service::RunTokenRevoker;
use myelin_flow::{DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenMinter};

#[derive(Default)]
struct IdentityMintProvider;
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
        let _ = caveats;
        Ok(RunTokenHandle {
            token: format!("tok:{agent_id}:{run_id}"),
            jti: format!("jti:{agent_id}:{run_id}"),
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
    fn revoke(&self, jti: &str, now_secs: i64, teardown_secs: i64) -> Result<u64, String> {
        let mut g = self.revoked.lock().unwrap();
        if !g.insert(jti.to_string()) {
            return Ok(0);
        }
        Ok(now_secs.saturating_sub(teardown_secs).max(0) as u64)
    }
    fn is_dead(&self, jti: &str, now_secs: i64) -> bool {
        self.revoked.lock().unwrap().contains(jti) || now_secs >= self.minted_at + self.ttl_w
    }
}

#[test]
fn mint_binds_token_to_agent_and_run_and_refuses_zero_ttl() {
    let provider = IdentityMintProvider;
    let mut caveats = DelegationCaveats(vec!["delegated:human-x".into()]);
    caveats.0.push("run:R1".into());
    let token = provider
        .mint_run_token("psn:agent-7", "R1", &caveats, 300)
        .expect("the mint succeeds under a positive TTL");
    assert_eq!(
        token.jti, "jti:psn:agent-7:R1",
        "the jti is bound to (agent, run)"
    );
    assert_eq!(
        token.ttl_secs, 300,
        "token life == the short fail-static window (run life)"
    );

    assert!(
        provider
            .mint_run_token("psn:agent-7", "R2", &caveats, 0)
            .is_err(),
        "a 0-TTL per-run token is refused (never a no-expiry token)"
    );
}

#[test]
fn revoke_is_idempotent_even_on_crash_and_token_auto_expires() {
    let minted_at = 1000i64;
    let w = 300i64;
    let provider = IdentityRevokeProvider {
        revoked: std::sync::Mutex::new(std::collections::HashSet::new()),
        minted_at,
        ttl_w: w,
    };
    let jti = "jti:psn:agent-7:R1";

    assert_eq!(
        provider.revoke(jti, minted_at, minted_at),
        Ok(0),
        "first revoke records the jti (lag 0)"
    );
    assert!(
        provider.is_dead(jti, minted_at),
        "revoked-on-teardown → dead now"
    );

    assert_eq!(
        provider.revoke(jti, minted_at + 5, minted_at),
        Ok(0),
        "a re-revoke is a no-op (idempotent)"
    );

    let fresh = IdentityRevokeProvider {
        revoked: std::sync::Mutex::new(std::collections::HashSet::new()),
        minted_at,
        ttl_w: w,
    };
    let other = "jti:psn:agent-7:R2";
    assert!(!fresh.is_dead(other, minted_at), "not yet expired before W");
    assert!(
        fresh.is_dead(other, minted_at + w),
        "auto-expires by minted_at + W (≤ W window)"
    );
}
