use myelin_identity::{Consistency, ConsistencyMode, Decision, Zookie};
use myelin_substrate::{encode_authz_key, FailStaticAuthz, FailStaticThreshold, ServeError, TestClock};

fn threshold() -> FailStaticThreshold {
    FailStaticThreshold {
        status: "OPEN - LEGAL".into(),
        owner: "DPO / Legal".into(),
        static_max_secs: None,
        static_max_default_secs: 300,
        agent_token_ttl_secs: 60,
        constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
    }
}

fn authz_at(t0: u64) -> FailStaticAuthz<TestClock> {
    FailStaticAuthz::try_new_with_clock(300, &threshold(), TestClock::at(t0)).expect("valid bound")
}

fn bounded_stale() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::BoundedStale,
    }
}

fn allow() -> Result<Decision, ServeError> {
    Ok(Decision::Allow)
}
fn hiccup() -> Result<Decision, ServeError> {
    Err(ServeError("identity authz hiccup".into()))
}

fn naive_refs_key(t: &str, r: &str, pid: &str, perm: &str, obj: &str) -> String {
    format!("{t}|{r}|{pid}|{perm}@{obj}")
}
fn naive_git_key(t: &str, r: &str, pid: &str, perm: &str, obj: &str) -> String {
    format!("{t}/{r}/{pid}::{perm}@{obj}")
}

#[test]
fn refs_shaped_adversarial_pair_is_injective_and_does_not_alias() {
    let a = ["acme", "eu", "alice|view@repo:secret", "view", "repo:pub"];
    let b = ["acme", "eu", "alice", "view", "repo:secret|view@repo:pub"];

    assert_eq!(
        naive_refs_key(a[0], a[1], a[2], a[3], a[4]),
        naive_refs_key(b[0], b[1], b[2], b[3], b[4]),
        "the pre-R2.3b format! builder DID collide (this is the residual we are closing)"
    );

    let key_a = encode_authz_key(&a);
    let key_b = encode_authz_key(&b);
    assert_ne!(
        key_a, key_b,
        "length-prefixed encoding keeps the two distinct questions distinct"
    );

    let fs = authz_at(1_000);
    assert!(
        fs.serve(key_a, &bounded_stale(), false, allow).is_allow(),
        "A is a fresh authoritative ALLOW (cached)"
    );
    fs.clock().advance(31);
    let b_served = fs.serve(key_b, &bounded_stale(), false, hiccup);
    assert!(
        b_served.is_deny(),
        "B is a DIFFERENT question with no cached grant → fail CLOSED, never borrow A's ALLOW: {b_served:?}"
    );
}

#[test]
fn git_shaped_adversarial_pair_is_injective_and_does_not_alias() {
    let a = ["acme", "eu", "alice::read@repo:secret", "read", "repo:pub"];
    let b = ["acme", "eu", "alice", "read", "repo:secret::read@repo:pub"];

    assert_eq!(
        naive_git_key(a[0], a[1], a[2], a[3], a[4]),
        naive_git_key(b[0], b[1], b[2], b[3], b[4]),
        "the pre-R2.3b git format! builder DID collide"
    );

    let key_a = encode_authz_key(&a);
    let key_b = encode_authz_key(&b);
    assert_ne!(key_a, key_b, "injective encoding separates the git-shaped pair");

    let fs = authz_at(1_000);
    assert!(fs.serve(key_a, &bounded_stale(), false, allow).is_allow());
    fs.clock().advance(31);
    let b_served = fs.serve(key_b, &bounded_stale(), false, hiccup);
    assert!(
        b_served.is_deny(),
        "git-shaped B fails closed, no cross-principal cached-ALLOW replay: {b_served:?}"
    );
}

#[test]
fn colliding_pair_does_not_alias_through_the_static_background_refresh_path() {
    let key_a = encode_authz_key(&["acme", "eu", "alice|view@repo:secret", "view", "repo:pub"]);
    let key_b = encode_authz_key(&["acme", "eu", "alice", "view", "repo:secret|view@repo:pub"]);
    assert_ne!(key_a, key_b);

    let fs = authz_at(1_000);

    assert!(fs.serve(key_a.clone(), &bounded_stale(), false, allow).is_allow());
    fs.clock().advance(31);

    let a_calls = std::cell::Cell::new(0u32);
    let a_flaky = || {
        let n = a_calls.get();
        a_calls.set(n + 1);
        if n == 0 {
            Err(ServeError("foreground hiccup".into()))
        } else {
            Ok(Decision::Allow)
        }
    };
    let a_stale = fs.serve(key_a.clone(), &bounded_stale(), false, a_flaky);
    assert!(
        a_stale.is_allow() && a_stale.is_degraded(),
        "A is served its own STATIC (degraded) grant: {a_stale:?}"
    );

    let b_served = fs.serve(key_b.clone(), &bounded_stale(), false, hiccup);
    assert!(
        b_served.is_deny(),
        "B must fail closed even after A's background refresh re-stamped A's key: {b_served:?}"
    );

    let a_after = fs.serve(key_a, &bounded_stale(), false, hiccup);
    assert!(
        a_after.is_allow(),
        "A's own key was re-stamped by the background refresh (fresh again): {a_after:?}"
    );
    let b_after = fs.serve(key_b, &bounded_stale(), false, hiccup);
    assert!(b_after.is_deny(), "B still has no entry of its own: {b_after:?}");
}

#[test]
fn length_prefix_framing_is_not_forgeable_by_segment_content() {
    let p = encode_authz_key(&["a", "b"]);
    let q = encode_authz_key(&["a", "b:x"]);
    assert_ne!(p, q);

    let one = encode_authz_key(&["1:x2:yy"]);
    let two = encode_authz_key(&["x", "yy"]);
    assert_ne!(
        one, two,
        "a segment that embeds framing bytes is not confusable with real frames"
    );
}
