//! # R2.3b regression — INJECTIVE fail-static authz key encoding (delimiter-injection aliasing)
//!
//! Permanent regression for the residual an adversarial verifier found on the R2.3 core fix. The
//! core fix keys `FailStatic` by the REAL key compared with `Eq`, killing the 64-bit-hash aliasing
//! at the cache layer. But the two `FailStaticAuthz` key builders (`myelin-git`
//! `live_check::cache_key`, `myelin-refs-service` `resolve`) originally flattened UNCONSTRAINED
//! user-controlled segments with a `format!` delimiter join:
//!
//!   git  : `format!("{t}/{r}/{pid}::{perm}@{obj}")`
//!   refs : `format!("{t}|{r}|{pid}|{perm}@{obj}")`
//!
//! `PrincipalId(pub String)` / `ArtifactRef(pub String)` carry NO charset rule (they are sourced
//! from an external OIDC `sub` / SCIM id / an owner-supplied object ref). So two DISTINCT logical
//! authz questions could serialize to a BYTE-IDENTICAL key — and the full-key `Eq` comparison the
//! core fix added cannot tell them apart, because they really are the same string. One question then
//! borrows the OTHER's cached ALLOW during an Identity hiccup: a cross-principal cached-ALLOW replay.
//!
//! The fix (R2.3b): both builders route through [`myelin_substrate::encode_authz_key`], which frames
//! each segment length-prefixed (`{byte_len}:{segment}`) so no segment content can forge a frame
//! boundary — the segment→key map is injective. This test:
//!   1. documents the historical bug (the naive `format!` join DID collide the adversarial pair);
//!   2. proves `encode_authz_key` maps the same adversarial pair to DISTINCT keys (both formats);
//!   3. proves the pair does NOT alias through the REAL `FailStaticAuthz` — neither on the fresh/miss
//!      path NOR through the Static (stale-while-revalidate / background-refresh) path.
//!
//! Red→green: assertion (2) is `assert_ne!` on the encoded keys — it FAILS on the old `format!`
//! builders (which produced equal strings) and PASSES on the injective encoding. The git-side call
//! site is additionally pinned by a unit test on the real private `cache_key` in
//! `myelin-git/src/live_check.rs`.

use myelin_identity::{Consistency, ConsistencyMode, Decision, Zookie};
use myelin_substrate::{encode_authz_key, FailStaticAuthz, FailStaticThreshold, ServeError, TestClock};

fn threshold() -> FailStaticThreshold {
    FailStaticThreshold {
        status: "OPEN — LEGAL".into(),
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

/// The NAIVE (pre-R2.3b) refs key builder — reproduced ONLY to document that it collided.
fn naive_refs_key(t: &str, r: &str, pid: &str, perm: &str, obj: &str) -> String {
    format!("{t}|{r}|{pid}|{perm}@{obj}")
}
/// The NAIVE (pre-R2.3b) git key builder — reproduced ONLY to document that it collided.
fn naive_git_key(t: &str, r: &str, pid: &str, perm: &str, obj: &str) -> String {
    format!("{t}/{r}/{pid}::{perm}@{obj}")
}

/// **The refs-format adversarial pair no longer collides, and does not alias through FailStaticAuthz.**
///
/// Question A: principal id `alice|view@repo:secret` asking `view` on `repo:pub` → an ALLOW.
/// Question B: principal id `alice` asking `view` on the crafted object
///             `repo:secret|view@repo:pub` → a DIFFERENT (subject, object) pair.
/// Under the naive `format!` join these serialized to the SAME string (the historical leak). Under
/// the injective encoding they are DISTINCT, so B — never authoritatively allowed — must fail CLOSED
/// during a hiccup, never borrow A's cached ALLOW.
#[test]
fn refs_shaped_adversarial_pair_is_injective_and_does_not_alias() {
    let a = ["acme", "eu", "alice|view@repo:secret", "view", "repo:pub"];
    let b = ["acme", "eu", "alice", "view", "repo:secret|view@repo:pub"];

    // (1) the historical bug: the naive delimiter join collided the two distinct questions.
    assert_eq!(
        naive_refs_key(a[0], a[1], a[2], a[3], a[4]),
        naive_refs_key(b[0], b[1], b[2], b[3], b[4]),
        "the pre-R2.3b format! builder DID collide (this is the residual we are closing)"
    );

    // (2) the fix: the injective encoding maps them to DISTINCT keys.
    let key_a = encode_authz_key(&a);
    let key_b = encode_authz_key(&b);
    assert_ne!(
        key_a, key_b,
        "length-prefixed encoding keeps the two distinct questions distinct"
    );

    // (3) no runtime alias through the real FailStaticAuthz.
    let fs = authz_at(1_000);
    assert!(
        fs.serve(key_a, &bounded_stale(), false, allow).is_allow(),
        "A is a fresh authoritative ALLOW (cached)"
    );
    fs.clock().advance(31); // into the stale window
    let b_served = fs.serve(key_b, &bounded_stale(), false, hiccup);
    assert!(
        b_served.is_deny(),
        "B is a DIFFERENT question with no cached grant → fail CLOSED, never borrow A's ALLOW: {b_served:?}"
    );
}

/// **The git-format (`::` / `@`) adversarial pair — same class, different delimiters.**
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

/// **The Static / background-refresh path is also collision-safe.** Beyond the fresh/miss path, this
/// drills a colliding-encoded pair through the STALE rung: A is served `Static` (degraded) during a
/// hiccup — which triggers `FailStatic`'s stale-while-revalidate background refresh (that re-inserts
/// by the REAL key) — while B, a distinct question, must STILL fail closed. Proves the background
/// re-stamp writes under A's own injective key and never seeds B's.
#[test]
fn colliding_pair_does_not_alias_through_the_static_background_refresh_path() {
    // A's own question, encoded injectively.
    let key_a = encode_authz_key(&["acme", "eu", "alice|view@repo:secret", "view", "repo:pub"]);
    // B: a DISTINCT (subject, object) question that would collide with A under the naive join.
    let key_b = encode_authz_key(&["acme", "eu", "alice", "view", "repo:secret|view@repo:pub"]);
    assert_ne!(key_a, key_b);

    let fs = authz_at(1_000);

    // Prime A with a fresh ALLOW, then age into the stale window.
    assert!(fs.serve(key_a.clone(), &bounded_stale(), false, allow).is_allow());
    fs.clock().advance(31);

    // A is served STATIC (degraded); the source hiccups in the foreground but RECOVERS for the
    // stale-while-revalidate background refresh — which must re-insert under A's real key only.
    let a_calls = std::cell::Cell::new(0u32);
    let a_flaky = || {
        let n = a_calls.get();
        a_calls.set(n + 1);
        if n == 0 {
            Err(ServeError("foreground hiccup".into())) // A served stale to the caller
        } else {
            Ok(Decision::Allow) // the background revalidate succeeds → re-stamps A's key
        }
    };
    let a_stale = fs.serve(key_a.clone(), &bounded_stale(), false, a_flaky);
    assert!(
        a_stale.is_allow() && a_stale.is_degraded(),
        "A is served its own STATIC (degraded) grant: {a_stale:?}"
    );

    // B arrives during a hiccup. Despite colliding with A under the OLD encoding, B has no cached
    // grant of its own — not from A's fresh insert, and not from A's background re-stamp — so it MUST
    // fail closed. (If the background refresh had written under a shared/hashed key, B would leak.)
    let b_served = fs.serve(key_b.clone(), &bounded_stale(), false, hiccup);
    assert!(
        b_served.is_deny(),
        "B must fail closed even after A's background refresh re-stamped A's key: {b_served:?}"
    );

    // And A itself is now Fresh again (its background refresh re-stamped its OWN key) — proving the
    // re-stamp targeted A, and still did not seed B.
    let a_after = fs.serve(key_a, &bounded_stale(), false, hiccup);
    assert!(
        a_after.is_allow(),
        "A's own key was re-stamped by the background refresh (fresh again): {a_after:?}"
    );
    let b_after = fs.serve(key_b, &bounded_stale(), false, hiccup);
    assert!(b_after.is_deny(), "B still has no entry of its own: {b_after:?}");
}

/// **Injectivity unit check: a segment that embeds the framing punctuation cannot forge a boundary.**
/// A segment literally equal to `"3:abc"` must not be confusable with the length-3 segment `"abc"`.
#[test]
fn length_prefix_framing_is_not_forgeable_by_segment_content() {
    // A segment whose content mimics the framing punctuation cannot merge into a neighbour.
    let p = encode_authz_key(&["a", "b"]);
    let q = encode_authz_key(&["a", "b:x"]); // trailing content cannot merge into a neighbour
    assert_ne!(p, q);

    // A segment whose bytes spell another segment's frame does not collide with the two-segment form.
    let one = encode_authz_key(&["1:x2:yy"]); // a single 7-byte segment
    let two = encode_authz_key(&["x", "yy"]); // two segments
    assert_ne!(
        one, two,
        "a segment that embeds framing bytes is not confusable with real frames"
    );
}
