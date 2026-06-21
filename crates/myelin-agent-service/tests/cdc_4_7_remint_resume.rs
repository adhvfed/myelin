//! # The CDC pair for contract 4.7 — `mint_run_token` re-mintable on resume + idempotent `revoke`
//! (CONSUMED; AG-P13 → P-225)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 4.7
//! (`mint_run_token(agent_id, run_id, delegation_caveats, ttl) → token` + `revoke(jti)` — Identity,
//! §4/§11; **re-mintable on resume** (C6), token life == run life, revoke idempotent even on crash,
//! auto-expiring tuple). Owning architecture: `agent-fabric.md` §5.7 (per-run identity: mint /
//! scrub / revoke + re-mint on resume — *the full form here; AG-P4 → P-216 shipped the simple form*).
//!
//! The Agent-Fabric run ([`RunIdentity`]) is the CONSUMER of the 4.7 *re-mintable-on-resume* form. It
//! drives the mint surface ([`RunTokenMinter`]) at dispatch AND on every resume from a multi-day park,
//! and the revoke surface ([`RunTokenRevoker`]) at teardown. This pair stands a REAL provider impl
//! against those seams and proves: a resume re-mints a FRESH attenuated token with the SAME caveats
//! and the REMAINING run life (token life == run life, never widening the attribution window beyond
//! the run's deadline); the run stays attributed (0 unattributed window); the revoke is idempotent
//! even on a doubled teardown.

use myelin_agent_service::{RunIdentity, RunTokenRevoker, FAIL_STATIC_W_SECS};
use myelin_flow::{DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenMinter};
use std::sync::Arc;

/// **PROVIDER side of 4.7 (the Identity mint, re-mintable form).** Mints a per-run attenuated token
/// whose `jti` is bound to `(agent_id, run_id, mint#)` (a fresh DISTINCT token per mint — a re-mint
/// is NEVER the prior token) under the supplied TTL, carrying the attenuate-only caveat chain. A real
/// impl on the frozen mint surface that records its calls so the consumer-side clamp can be asserted.
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
        // A per-run token MUST have a positive life (life == run life); a 0-TTL token is dead on
        // arrival — refuse it (never mint a no-expiry per-run token).
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

fn caveats() -> DelegationCaveats {
    DelegationCaveats(vec!["delegated:human-x".into(), "tenant:acme".into()])
}

/// **CONSUMER drives the mint surface at dispatch AND on resume (4.7 re-mintable form, §5.7 C6).** A
/// multi-day pause spanning the token TTL re-mints a FRESH attenuated token with the SAME caveats and
/// the REMAINING run life — the run stays attributed within the TTL bound, 0 unattributed window.
#[test]
fn remint_on_resume_keeps_a_multi_day_pause_attributed_within_ttl() {
    let provider = Arc::new(IdentityMintProvider::default());
    let mut id = RunIdentity::new(provider.clone(), "psn:agent-7", "R1", caveats());

    // Dispatch a long-lived run (3-day life) at t=1000. Token life == min(W, run life) == W (300).
    id.mint_at_dispatch(1000, 259_200).expect("dispatch mint");
    let dispatch_jti = id.current().unwrap().jti.clone();
    assert_eq!(id.current().unwrap().ttl_secs, FAIL_STATIC_W_SECS, "dispatch token TTL == W");

    // PARK ~2 days (the dispatch token's 300s TTL expired harmlessly while the run held no thread),
    // then RESUME. The driver re-mints a fresh token BEFORE the resumed work runs.
    let resume_at = 1000 + 172_800; // +2 days.
    let resumed = id.remint_on_resume(resume_at).expect("re-mint on resume").clone();
    assert_ne!(resumed.jti, dispatch_jti, "the re-mint is a FRESH token (not the dispatch one)");
    // remaining = (1000 + 259200) - 173800 = 86400 (1 day) > W → clamp to W (attributed within bound).
    assert!(resumed.ttl_secs <= FAIL_STATIC_W_SECS, "re-mint TTL within the W bound");

    // SAME caveats on re-mint (attenuate-only — a resume never widens the grant).
    let calls = provider.calls.lock().unwrap();
    assert_eq!(calls.len(), 2, "one mint at dispatch + one re-mint on resume");
    let (_, _, cav, _) = calls[1].clone();
    assert!(cav.0.contains(&"run:R1".to_string()), "per-run attenuation re-carried on resume");
    assert!(cav.0.contains(&"delegated:human-x".to_string()), "SAME delegation grant on resume");
    drop(calls);

    // 0 UNATTRIBUTED WINDOW across the pause: every executing instant opened a live segment.
    assert!(!id.attribution_window().has_unattributed_gap(), "0 unattributed window across the pause");
    assert!(id.attribution_window().max_segment_width() <= FAIL_STATIC_W_SECS,
        "no token widened the attribution window beyond W");
    assert_eq!(id.reminted(), 1, "exactly one re-mint on the resume");
}

/// **CONSUMER drives the revoke surface (4.7 revoke half, §5.7) — idempotent even on crash, on the
/// CURRENT re-minted token.** The teardown revokes the FRESH token (the dispatch one expired during
/// the park); a doubled teardown is a no-op; an un-revoked token auto-expires within W.
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
    // first teardown: revoke the CURRENT (re-minted) token (lag 0 — teardown == now).
    assert_eq!(id.revoke_on_teardown(&revoker, 2100, 2100), 0, "first revoke lands");
    assert!(revoker.is_dead(&fresh_jti, 2100), "the fresh token is dead after revoke");
    // a SECOND teardown (a crash-recovery sweep) is a no-op — idempotent even on crash.
    assert_eq!(id.revoke_on_teardown(&revoker, 2105, 2100), 0, "a re-revoke is a no-op (idempotent)");

    // belt-and-suspenders: even ABSENT the explicit revoke, the token auto-expires within W.
    let other = IdentityRevokeProvider {
        revoked: std::sync::Mutex::new(std::collections::HashSet::new()),
        minted_at: 2000,
        ttl_w: FAIL_STATIC_W_SECS as i64,
    };
    assert!(!other.is_dead("jti:never", 2000), "not yet expired before W");
    assert!(other.is_dead("jti:never", 2000 + FAIL_STATIC_W_SECS as i64), "auto-expires by minted_at + W");
}

/// **A resume PAST the run deadline is refused LOUD (never widen attribution past run life, §5.7).**
/// A run that parked beyond its OWN deadline has no remaining life; the consumer refuses to re-mint
/// rather than run the resumed work past the run's allotted life — surfaced as a `RunTokenError`.
#[test]
fn remint_past_the_run_deadline_is_refused() {
    let provider = Arc::new(IdentityMintProvider::default());
    let mut id = RunIdentity::new(provider, "psn:agent-7", "R3", caveats());
    id.mint_at_dispatch(1000, 300).expect("dispatch"); // deadline 1300.
    let err = id.remint_on_resume(1400).expect_err("resume past the deadline is refused");
    assert!(err.to_string().contains("no remaining life"), "refused LOUD: {err}");
}
