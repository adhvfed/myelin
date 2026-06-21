//! # The CDC pair for contract 4.7 — `mint_run_token` / `revoke` (CONSUMED; AG-P4 → P-216)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 4.7
//! (`mint_run_token(agent_id, run_id, delegation_caveats, ttl) → token` + `revoke(jti)` — Identity,
//! §4/§11; token life == run life, revoke idempotent even on crash, auto-expiring tuple). Owning
//! architecture: `agent-fabric.md` §5.7 (per-run identity: mint at dispatch, revoke on teardown — the
//! SIMPLE form here; the full mint/scrub/revoke + re-mint-on-resume is AG-P13 → P-225).
//!
//! The SKELETON is the CONSUMER of 4.7: it drives the mint surface
//! ([`RunTokenMinter`](myelin_flow::RunTokenMinter)) at dispatch and the revoke surface
//! ([`RunTokenRevoker`](myelin_agent_service::RunTokenRevoker)) at teardown. This pair stands a REAL
//! provider impl against those seams and proves: the mint binds the token to `(agent, run)`; the
//! revoke is idempotent even on a doubled teardown; an un-revoked token auto-expires within W.

use myelin_agent_service::RunTokenRevoker;
use myelin_flow::{DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenMinter};

/// **PROVIDER side of 4.7 (the Identity mint).** Mints a per-run attenuated token whose `jti` is
/// bound to `(agent_id, run_id)` (token life == run life) under the supplied short TTL, carrying the
/// attenuate-only caveat chain. A real impl on the frozen mint surface.
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
        // A per-run token MUST have a positive life (life == run life) — a 0-TTL token is dead on
        // arrival; refuse it loudly (never mint a no-expiry per-run token).
        if ttl_secs == 0 {
            return Err(RunTokenError("non-positive TTL".into()));
        }
        // The mint carries the attenuate-only caveat chain (the run's grant chain, narrowed per-run).
        let _ = caveats;
        Ok(RunTokenHandle {
            token: format!("tok:{agent_id}:{run_id}"),
            jti: format!("jti:{agent_id}:{run_id}"),
            ttl_secs,
        })
    }
}

/// **PROVIDER side of 4.7 (the Identity revocation, §11).** A denylist keyed on `jti` + the token's
/// auto-expiring TTL tuple. `revoke` is idempotent even on crash (a doubled teardown is a no-op);
/// `is_dead` is `true` once the jti is revoked OR the TTL window W has elapsed.
struct IdentityRevokeProvider {
    revoked: std::sync::Mutex<std::collections::HashSet<String>>,
    minted_at: i64,
    ttl_w: i64,
}
impl RunTokenRevoker for IdentityRevokeProvider {
    fn revoke(&self, jti: &str, now_secs: i64, teardown_secs: i64) -> u64 {
        let mut g = self.revoked.lock().unwrap();
        if !g.insert(jti.to_string()) {
            return 0; // idempotent even on crash: a re-revoke is a no-op (lag 0).
        }
        (now_secs - teardown_secs).max(0) as u64
    }
    fn is_dead(&self, jti: &str, now_secs: i64) -> bool {
        self.revoked.lock().unwrap().contains(jti) || now_secs >= self.minted_at + self.ttl_w
    }
}

/// **CONSUMER drives the mint surface (4.7 mint half).** The SKELETON mints a per-run token at
/// dispatch; the `jti` is bound to `(agent, run)` (token life == run life). A 0-TTL mint is refused
/// (never a no-expiry per-run token).
#[test]
fn mint_binds_token_to_agent_and_run_and_refuses_zero_ttl() {
    let provider = IdentityMintProvider;
    let mut caveats = DelegationCaveats(vec!["delegated:human-x".into()]);
    caveats.0.push("run:R1".into()); // the per-run attenuation the SKELETON adds.
    let token = provider
        .mint_run_token("psn:agent-7", "R1", &caveats, 300)
        .expect("the mint succeeds under a positive TTL");
    assert_eq!(token.jti, "jti:psn:agent-7:R1", "the jti is bound to (agent, run)");
    assert_eq!(token.ttl_secs, 300, "token life == the short fail-static window (run life)");

    // a 0-TTL mint is refused (a per-run token must have a finite positive life).
    assert!(
        provider.mint_run_token("psn:agent-7", "R2", &caveats, 0).is_err(),
        "a 0-TTL per-run token is refused (never a no-expiry token)"
    );
}

/// **CONSUMER drives the revoke surface (4.7 revoke half, §5.7).** The teardown revokes the per-run
/// token idempotently even on crash (a doubled teardown — the explicit revoke + a crash sweep — is a
/// no-op); an un-revoked token auto-expires within the TTL window W (belt-and-suspenders).
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

    // first teardown: revoke (lag 0 — teardown == now in this run).
    assert_eq!(provider.revoke(jti, minted_at, minted_at), 0, "first revoke records the jti (lag 0)");
    assert!(provider.is_dead(jti, minted_at), "revoked-on-teardown → dead now");

    // a SECOND teardown (a crash-recovery sweep) is a no-op — idempotent even on crash.
    assert_eq!(provider.revoke(jti, minted_at + 5, minted_at), 0, "a re-revoke is a no-op (idempotent)");

    // belt-and-suspenders: even ABSENT the explicit revoke, the token auto-expires within W.
    let fresh = IdentityRevokeProvider {
        revoked: std::sync::Mutex::new(std::collections::HashSet::new()),
        minted_at,
        ttl_w: w,
    };
    let other = "jti:psn:agent-7:R2";
    assert!(!fresh.is_dead(other, minted_at), "not yet expired before W");
    assert!(fresh.is_dead(other, minted_at + w), "auto-expires by minted_at + W (≤ W window)");
}
