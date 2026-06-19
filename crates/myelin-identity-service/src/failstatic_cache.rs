//! # `failstatic_cache` — S6, the fail-static availability cache (P-ID-15 → global P-073)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md`
//! §10 (*the fail-static availability cache*): **authorization correctness stays fail-closed**
//! (deny when genuinely unsure); **availability fails static** — an Id-dependency hiccup keeps
//! already-authenticated traffic alive on a bounded-staleness cached `{actor_active,
//! coarse_grants}` (S6). The zookie interplay (zookie-stamped reads BYPASS S6 and
//! fail-closed-or-wait; default-consistency reads are served static during a hiccup) and the bound
//! `static_max ≤ revocation SLA` / `≥ agent/CI token TTL` (W = 5 min default-to-beat, the
//! `[OPEN — LEGAL]` L-1 ratification) are §10/C11. §2 (the **S6 row**: Redis/Valkey-class, NEVER
//! a source of truth, `(tenant, region, subject)` keyed, TTL ≤ revocation SLA).
//!
//! **Contract-index:** rows **4.11** (the fail-static bound — OWNED here), **4.10** (the
//! zookie-bypass-S6 half — OWNED here), **1.8** (`cache_hit_ratio` / `staleness_age` telemetry),
//! **1.9** (`ResilientClient` — the critical-dep caller wraps `check` in it; the CDC pair),
//! **1.10** (`FailStatic<T>` — the substrate mechanism S6 is built on), **4.7** (the S7 denylist a
//! just-revoked grant is still denied against).
//!
//! ## What this module ships (P-ID-15 — S6 + the 4.11 staleness bound + the zookie-bypass)
//! 1. **S6, the fail-static cache** ([`FailStaticCache`]) built on the M0 [`FailStatic<T>`]
//!    primitive (P-S18): bounded-staleness `{actor_active, coarse_grants}` ([`CoarseGrant`]) +
//!    a decision cache, **NEVER a source of truth**, keyed `(tenant, region, subject)`, with
//!    `static_max ≤ revocation SLA`. **Correctness stays fail-closed** (deny when genuinely
//!    unsure); **availability fails static** (an Id-dependency hiccup keeps already-authenticated
//!    traffic alive on the cache).
//! 2. **The zookie-bypass (4.10 half)** — a zookie-stamped (`ConsistencyMode::Strong`) read
//!    **BYPASSES S6** and is fail-closed-or-wait: it goes straight to the authoritative source
//!    `check` (and on a hiccup it fails CLOSED, it does NOT serve stale). A default-consistency
//!    (`ConsistencyMode::BoundedStale`) read is served static during a hiccup.
//! 3. **The fail-static bound (4.11)** — `static_max ≤ revocation SLA` and `≥ agent/CI token TTL`
//!    is enforced STRUCTURALLY by the [`FailStatic`] constructor (P-S18); the W = 5 min
//!    default-to-beat is written + dated to `thresholds.toml` (the `[fail_static]` row, P-S22)
//!    with the `[OPEN — LEGAL]` L-1 follow-on. [`FailStaticCache::try_new`] reads that bound.
//! 4. **The just-revoked-grant deny (the F7 / ID-D2 correctness floor)** — even when S6 still
//!    holds a stale ALLOW for a subject, a `revoke` / SCIM-disable of that subject is **still
//!    denied**: the cache consults the S7 [`RevocationStore`] (the §4 authoritative deny path) on
//!    every served answer, so a revoked subject is denied THROUGH the stale cache. This is the
//!    zero-successful-authz-after-the-cache property the drill quantifies.
//!
//! ## The two mandatory-core branches (mutation-tested, per the prompt GATE)
//! - **fail-closed-vs-fail-static** — a `Strong` read fails CLOSED on a hiccup (deny, never
//!   stale); a `BoundedStale` read fails STATIC (serve the last coarse grant). A mutation that
//!   swaps these branches (serves a `Strong` read stale, or fails a `BoundedStale` read closed
//!   when a coarse grant exists) MUST be caught.
//! - **zookie-bypass** — a `Strong`/zookie-stamped read NEVER reads S6. A mutation that serves a
//!   `Strong` read from the cache MUST be caught (it would defeat the new-enemy guard).
//! - **revoked-subject-denied-from-S6** — a stale ALLOW for a revoked subject is still DENIED. A
//!   mutation that serves a revoked subject from S6 (skips the S7 consult) MUST be caught (it is
//!   the exact F7 failure).
//!
//! ## Floors named (frozen mechanism now → ratification / wiring follow-ons)
//! - **W = 5 min is the engineering default-to-beat, structurally enforced regardless; the DPO
//!   ratification of the *number* is the `[OPEN — LEGAL]` L-1 follow-on** (parallel legal — the
//!   floor does NOT wait). The bound's VALUE is `[OPEN — LEGAL]` in `thresholds.toml`; the
//!   `static_max ≤ revocation-SLA ≥ agent-token-TTL` CONSTRAINT and the W = 300 s engineering seed
//!   ship + are enforced now (P-S18's [`FailStatic::try_new`]). Reading W as a ratified number is a
//!   loud error until counsel clears it ([`FailStaticThreshold::ratified_static_max_secs`]).
//! - **The in-memory `CoarseGrant` cache models the Redis/Valkey-class S6** (the same EI-01 §1
//!   deviation the S1/S3/S7/S8 stores already document): there is no live Redis/Valkey until the
//!   substrate binding lands (P-S15). The `(tenant, region, subject)`-keyed, bounded-staleness,
//!   never-a-source-of-truth semantics are byte-for-byte the §2/§10 S6 contract; the seam shape
//!   does not change when the binding lands.
//! - **Fail-static is PROVEN against a real Identity hiccup cross-seam in P-S25 (SUB-D4)**; this
//!   prompt drills S6 at the Identity authz boundary (ID-D2). The two are the same mechanism at
//!   two seams (the substrate primitive + the Identity authz usage).

use myelin_events::Timestamp;
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, PrincipalId, RevokeTarget,
};
use myelin_storage::TenantScope;
use myelin_substrate::thresholds::FailStaticThreshold;
use myelin_substrate::{
    Answer, Clock, FailStatic, FailStaticError, FailStaticSignals, Seconds, ServeError,
    StalenessBound, SystemClock,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::revocation::RevocationStore;

/// The S6 cache's logical table/store name (the §2 S6 row label) — a PII-free identifier for the
/// telemetry/holder seam. S6 is ephemeral (Redis/Valkey-class) and NEVER a source of truth, so it
/// is not a `PersonalDataHolder` of erasable PII: it caches only the coarse `{actor_active,
/// coarse_grants}` reference-grade decision, TTL-bounded ≤ the revocation SLA (it tombstones for
/// free on expiry). The durable authority is S3 (tuples) / S1 (principals); S6 is a derived,
/// reconstructible projection (rebuilt by the next authoritative `check`).
pub const S6_STORE: &str = "authz_failstatic_cache";

/// The freshness window seed (seconds) — the `fresh_ttl` of the S6 fail-static ladder: within it a
/// cached coarse grant is served as [`Answer::Fresh`] even on a hiccup (no degradation). A v1 seed
/// well under `static_max`; the measured value is tuned alongside `cache_hit_ratio` at scale
/// (P-ID-31 / P-074). The `static_max` (W) is read from the thresholds file (the `[fail_static]`
/// row), NOT hardcoded.
pub const FRESH_TTL_SECS: Seconds = 30;

/// The coarse authorization answer S6 caches (architecture §10: bounded-staleness `{actor_active,
/// coarse_grants}`). It is **coarse on purpose** — it is the availability fallback, not the
/// authoritative fine-grained decision. `actor_active` is the "is this subject still a live,
/// non-suspended principal" bit; `coarse_grant` is the last authoritative [`Decision`] the source
/// `check` returned for this `(subject, permission, object)`. The cache NEVER fabricates an
/// `Allow`: a `CoarseGrant` is only ever written from a real authoritative `Allow`/`Deny`, and the
/// availability fallback can only ever REPLAY a previously-authoritative answer (never escalate).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoarseGrant {
    /// Was the subject a live, non-suspended principal at cache time? (the `{actor_active}` half).
    /// A stale `actor_active == true` is STILL gated by the S7 revocation consult on read — a
    /// just-disabled subject is denied through the cache regardless of this cached bit.
    pub actor_active: bool,
    /// The last authoritative coarse decision (the `{coarse_grants}` half) — `Allow` or `Deny`.
    /// Never a fabricated escalation: written only from a real authoritative `check` result.
    pub coarse_grant: Decision,
}

impl CoarseGrant {
    /// The coarse cached answer from an authoritative `check` decision. `actor_active` is true
    /// unless the authoritative answer was an explicit `Deny` for a suspended/absent principal —
    /// the cache stores the answer it was given; the revocation gate (S7) is what makes a *later*
    /// disable take effect through a stale entry.
    pub fn from_decision(decision: Decision, actor_active: bool) -> CoarseGrant {
        CoarseGrant { actor_active, coarse_grant: decision }
    }
}

/// The S6 cache key (architecture §2 S6 row: keyed `(tenant, region, subject)`). The
/// `(tenant, region)` prefix is the partition — built from a verified [`TenantScope`], never a
/// path — so a cache lookup for one tenant structurally cannot reach another tenant's coarse
/// grants (the tenant-predicate floor). The `subject` + a per-`(permission, object)` discriminator
/// complete the key so two distinct authz questions never share one cached grant (the cross-actor
/// / cross-object leak the [`FailStatic`] bucket-hashing prevents).
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct S6Key {
    tenant: String,
    region: String,
    subject: String,
    /// the `permission@object` discriminator — distinct authz questions never collide.
    question: String,
}

/// **S6 — the fail-static availability cache (architecture §10 / §2 S6 row).** Built on the M0
/// [`FailStatic<T>`] primitive (P-S18); caches the coarse `{actor_active, coarse_grants}` answer
/// keyed `(tenant, region, subject)`, **NEVER a source of truth**. Cloneable (every surface shares
/// one cache + one S7 consult — EI-01 §7, one cache primitive).
///
/// **The whole point (§10).** Correctness stays fail-CLOSED (deny when genuinely unsure);
/// availability fails STATIC (an Id-dependency hiccup keeps already-authenticated traffic alive on
/// the bounded-staleness cache). A zookie-stamped (`Strong`) read BYPASSES the cache and is
/// fail-closed-or-wait; a default-consistency (`BoundedStale`) read is served static during a
/// hiccup. A just-revoked subject is denied THROUGH the stale cache (the S7 consult on every read).
#[derive(Clone)]
pub struct FailStaticCache<C: Clock = SystemClock> {
    /// The substrate bounded-staleness fail-static mechanism (P-S18). The `static_max` (W) it was
    /// constructed with is the thresholds-file bound; the constructor enforced
    /// `agent_token_ttl ≤ static_max ≤ revocation_sla` structurally.
    inner: Arc<FailStatic<CoarseGrant, C>>,
    /// The S7 revocation list / token denylist (P-ID-14) consulted on EVERY served answer — the
    /// just-revoked / SCIM-disabled subject is denied through a stale cache (the §4 authoritative
    /// deny path; the F7 / ID-D2 correctness floor). Shared so a `revoke` elsewhere is seen here.
    revocations: RevocationStore,
    /// The `cache_hit_ratio` / `staleness_age` telemetry (contract-index row 1.8). Observability is
    /// part of the pass (EI-01 §3).
    telemetry: Arc<CacheTelemetry>,
}

impl FailStaticCache<SystemClock> {
    /// Construct S6 against the wall clock, reading the **fail-static bound from the thresholds
    /// file** (the `[fail_static]` row, P-S22). The `static_max` (W) is the engineering seed
    /// (`static_max_default_secs`, == revocation SLA, the largest the constraint admits — the
    /// ratified W is `[OPEN — LEGAL]`); the constructor enforces `agent_token_ttl ≤ static_max ≤
    /// revocation_sla` structurally, so a bad bound does NOT construct (P-S18). `revocation_sla_secs`
    /// is the N from the `[revocation]` row.
    pub fn try_new(
        revocation_sla_secs: Seconds,
        threshold: &FailStaticThreshold,
        revocations: RevocationStore,
    ) -> Result<FailStaticCache<SystemClock>, FailStaticError> {
        FailStaticCache::try_new_with_clock(
            revocation_sla_secs,
            threshold,
            revocations,
            SystemClock,
        )
    }
}

impl<C: Clock> FailStaticCache<C> {
    /// Construct S6 against an injected clock (the boundary drills use a `TestClock`). The W is the
    /// thresholds-file engineering seed `static_max_default_secs`; `fresh_ttl` is [`FRESH_TTL_SECS`].
    /// The bound is enforced structurally by [`FailStatic::try_new_with_clock`] — a `static_max`
    /// violating `agent_token_ttl ≤ static_max ≤ revocation_sla` REJECTS here, never reaches a read.
    pub fn try_new_with_clock(
        revocation_sla_secs: Seconds,
        threshold: &FailStaticThreshold,
        revocations: RevocationStore,
        clock: C,
    ) -> Result<FailStaticCache<C>, FailStaticError> {
        // The structural staleness bound (4.11 / §8.2): static_max ≤ revocation SLA ≥ agent-token
        // TTL. Both halves come from the thresholds file (the VALUE W is [OPEN — LEGAL], the
        // CONSTRAINT ships regardless). The W used is the engineering SEED, NOT the ratified number.
        let bound = StalenessBound::from_threshold(revocation_sla_secs, threshold);
        let static_max = threshold.static_max_default_secs;
        let inner = FailStatic::try_new_with_clock(FRESH_TTL_SECS, static_max, bound, clock)?;
        Ok(FailStaticCache {
            inner: Arc::new(inner),
            revocations,
            telemetry: Arc::new(CacheTelemetry::default()),
        })
    }

    /// The staleness budget W (seconds) S6 was constructed with (the thresholds-file engineering
    /// seed; the ratified W is `[OPEN — LEGAL]`). `static_max ≤ revocation SLA`.
    pub fn static_max(&self) -> Seconds {
        self.inner.static_max()
    }

    /// A borrow of the injected clock — drills advance a `TestClock` across the staleness
    /// boundaries through it.
    pub fn clock(&self) -> &C {
        self.inner.clock()
    }

    /// The shared S7 denylist this cache consults on every read (the just-revoked deny path).
    pub fn revocations(&self) -> &RevocationStore {
        &self.revocations
    }

    /// The `cache_hit_ratio` / `staleness_age` telemetry snapshot (contract-index row 1.8).
    pub fn telemetry(&self) -> &CacheTelemetry {
        &self.telemetry
    }

    /// The underlying fail-static fresh/stale/closed signals (architecture §10.2 row 6) — the
    /// drill reads the fresh/stale/closed answer ratio + the staleness age off this.
    pub fn fail_static_signals(&self) -> FailStaticSignals {
        self.inner.signals()
    }

    /// **The S6 authz read (architecture §10) — the load-bearing entrypoint.** Serve a `check`
    /// answer through the fail-static availability cache, honouring the zookie-bypass.
    ///
    /// `source` is the authoritative `check` (the depth-bounded Zanzibar evaluation) — on a healthy
    /// path it succeeds (and its answer is cached); on an Id-dependency hiccup it returns `Err`
    /// (the [`ServeError`] the [`ResilientClient`](myelin_client) surfaces). The behaviour split:
    ///
    /// - **`ConsistencyMode::Strong` (zookie-stamped) → BYPASS S6.** Go straight to `source`. On a
    ///   hiccup, fail CLOSED (`Deny`) — never serve stale (the new-enemy guard, §8.7 / 4.10). The
    ///   cache is **not read and not written** on a `Strong` read.
    /// - **`ConsistencyMode::BoundedStale` (default-consistency) → consult S6.** On a healthy
    ///   `source` read, cache the coarse answer + return it (`Fresh`). On a hiccup, serve the last
    ///   coarse grant within the staleness budget (`Static`, degraded), or fail CLOSED past the
    ///   budget / with no fallback (`Closed → Deny`) — never fail open.
    ///
    /// **The just-revoked deny (every path):** before returning ANY served `Allow` (fresh OR
    /// stale), the subject is checked against the S7 [`RevocationStore`]. A revoked/disabled
    /// subject is `Deny` regardless of the cached grant — the F7 / ID-D2 correctness floor.
    pub fn check_cached(
        &self,
        scope: &TenantScope,
        subject: &PrincipalId,
        question: &str,
        consistency: &Consistency,
        now: &Timestamp,
        source: impl Fn() -> Result<Decision, ServeError>,
    ) -> CachedDecision {
        // The just-revoked / SCIM-disabled deny (the §4 authoritative path) is ALWAYS applied — a
        // revoked subject is denied on EVERY consistency mode, BEFORE the cache is even consulted.
        // This is the F7 / ID-D2 correctness floor: a stale ALLOW in S6 never overrides a revoke.
        if self.subject_revoked(scope, subject, now) {
            return CachedDecision { decision: Decision::Deny, served: Served::Revoked };
        }

        match consistency.mode {
            // ── Zookie-stamped (Strong) read → BYPASS S6 (4.10 bypass half). ──
            // Fail-closed-or-wait: hit the authoritative source; on a hiccup fail CLOSED. The cache
            // is neither read nor written — a strong read never serves (or caches) stale.
            ConsistencyMode::Strong => match source() {
                Ok(decision) => {
                    self.telemetry.observe_bypass();
                    CachedDecision { decision, served: Served::SourceBypass }
                }
                // The new-enemy guard: a zookie read does NOT fall back to stale — it fails closed.
                Err(_hiccup) => {
                    self.telemetry.observe_bypass_closed();
                    CachedDecision { decision: Decision::Deny, served: Served::BypassClosed }
                }
            },

            // ── Default-consistency (BoundedStale) read → consult S6 (the availability win). ──
            ConsistencyMode::BoundedStale => {
                let key = self.key(scope, subject, question);
                // `FailStatic::get` runs the source; on success it caches the coarse answer + serves
                // it Fresh; on a hiccup it serves the last coarse grant Static (within W) or Closed
                // (past W / no fallback). We map the source `Decision` into a `CoarseGrant` for the
                // cache (only a real authoritative answer is ever cached — never a fabricated allow).
                let answer = self.inner.get(key, || {
                    source().map(|d| CoarseGrant::from_decision(d, !matches!(d, Decision::Deny)))
                });
                self.serve_answer(answer)
            }
        }
    }

    /// Map a [`FailStatic`] `Answer<CoarseGrant>` into the served authz decision, recording the
    /// `cache_hit_ratio` / `staleness_age` telemetry. A `Closed` answer (budget spent / no
    /// fallback) is the fail-CLOSED `Deny` — never an open fall-through.
    fn serve_answer(&self, answer: Answer<CoarseGrant>) -> CachedDecision {
        match answer {
            // Fresh: a live authoritative read (or a cached grant still inside fresh_ttl). A cache
            // HIT for telemetry purposes only when it was served from cache; a fresh upstream read
            // is a MISS (it went to source). FailStatic does not distinguish those for us, so we
            // record fresh-served as a hit-eligible served answer; the fresh/stale/closed ratio off
            // `fail_static_signals` is the authoritative survival signal.
            Answer::Fresh(grant) => {
                self.telemetry.observe_hit();
                CachedDecision { decision: grant.coarse_grant, served: Served::Fresh }
            }
            // Static (degraded): the bounded-staleness availability win — the last coarse grant is
            // served while the upstream hiccups. The drill asserts authenticated traffic survives
            // here. `staleness_age` is the underlying FailStatic `last_staleness_secs`.
            Answer::Static(grant) => {
                let age = self.inner.signals().last_staleness_secs;
                self.telemetry.observe_stale(age);
                CachedDecision { decision: grant.coarse_grant, served: Served::Static }
            }
            // Closed: budget spent or no fallback → fail CLOSED (deny is correct). NEVER open.
            Answer::Closed => {
                self.telemetry.observe_closed();
                CachedDecision { decision: Decision::Deny, served: Served::Closed }
            }
        }
    }

    /// Is `subject` revoked/disabled in the verified partition as of `now`? Consults the S7
    /// denylist for a `Principal` revocation (the SCIM-disable / `revoke(principal_id)` path). This
    /// is the gate that makes a just-disabled subject denied THROUGH a stale S6 entry.
    fn subject_revoked(&self, scope: &TenantScope, subject: &PrincipalId, now: &Timestamp) -> bool {
        self.revocations
            .is_revoked(scope, &RevokeTarget::Principal(subject.clone()), now)
    }

    /// Build the `(tenant, region, subject, question)` cache key from the verified scope (the
    /// partition prefix is the scope's `(tenant, region)`, never a path — the tenant-predicate
    /// floor; `question` keeps distinct authz questions from sharing one cached grant).
    fn key(&self, scope: &TenantScope, subject: &PrincipalId, question: &str) -> S6Key {
        S6Key {
            tenant: scope.tenant().0.clone(),
            region: scope.region().0.clone(),
            subject: subject.0.clone(),
            question: question.to_string(),
        }
    }
}

/// What S6 served — the observable provenance of a [`CachedDecision`] (for the drill assertions +
/// the mutation floor). It names which branch produced the answer so a drill can assert e.g. "a
/// `Strong` read bypassed the cache" or "a `BoundedStale` read survived on the static fallback".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Served {
    /// A zookie-stamped (`Strong`) read served the authoritative source directly (cache bypassed).
    SourceBypass,
    /// A zookie-stamped (`Strong`) read failed CLOSED on a hiccup (never served stale).
    BypassClosed,
    /// A `BoundedStale` read served a fresh answer (live source, or cached within `fresh_ttl`).
    Fresh,
    /// A `BoundedStale` read served the last coarse grant STATIC (degraded) during a hiccup — the
    /// availability win the drill asserts authenticated traffic survives on.
    Static,
    /// A `BoundedStale` read failed CLOSED (staleness budget spent / no fallback). Never open.
    Closed,
    /// The subject was revoked/disabled (S7) — denied regardless of any cached grant (the F7 floor).
    Revoked,
}

/// The S6 served decision + its provenance. The `decision` is the authorization answer; `served`
/// names which fail-static branch produced it (so the drill / mutation floor can assert the branch
/// as well as the answer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CachedDecision {
    /// The authorization decision served (fail-closed `Deny` on a bypass-closed / closed / revoked).
    pub decision: Decision,
    /// Which fail-static branch produced it (the observable provenance).
    pub served: Served,
}

impl CachedDecision {
    /// Was this an explicit `Allow`? (the "authenticated traffic survives" assertion).
    pub fn is_allow(&self) -> bool {
        matches!(self.decision, Decision::Allow)
    }

    /// Was this a `Deny`? (the "0 successful authz for a revoked subject" assertion).
    pub fn is_deny(&self) -> bool {
        matches!(self.decision, Decision::Deny)
    }

    /// Was this served from the static (degraded) fallback — the availability-survival rung?
    pub fn is_degraded(&self) -> bool {
        matches!(self.served, Served::Static)
    }
}

/// **The `cache_hit_ratio` / `staleness_age` telemetry (contract-index row 1.8).** Every S6 read
/// records its outcome; the drill reads the hit ratio + the staleness age off this. The signals are
/// keyed by the FROZEN name constants (`cache_hit_ratio`, `staleness_age`) so drills assert against
/// the named signal, never a literal. The metrics-health-port export (OpenTelemetry, §3.5/§10)
/// lands with the real port binding; this is the in-process counter the body increments. Observability
/// is part of the pass (EI-01 §3).
#[derive(Debug, Default)]
pub struct CacheTelemetry {
    /// cache HITs (fresh/stale served from the cache path) — the `cache_hit_ratio` numerator.
    hits: AtomicU64,
    /// cache MISSes (fail-closed: bypass-closed / closed) — counted toward the ratio denominator.
    misses: AtomicU64,
    /// zookie-bypass reads (served straight from source — neither a hit nor a miss of the cache).
    bypasses: AtomicU64,
    /// the `staleness_age` (seconds) of the MOST-RECENT static (degraded) answer. 0 when the last
    /// answer was not stale. The drill asserts this never exceeds `static_max` (≤ the revocation SLA).
    last_staleness_secs: AtomicU64,
}

impl CacheTelemetry {
    /// The FROZEN `cache_hit_ratio` signal name (contract-index row 1.8).
    pub const CACHE_HIT_RATIO: &'static str =
        myelin_identity::iam_events::signals::CACHE_HIT_RATIO;
    /// The FROZEN `staleness_age` signal name (contract-index row 1.8).
    pub const STALENESS_AGE: &'static str = myelin_identity::iam_events::signals::STALENESS_AGE;

    /// A served-from-cache HIT (a fresh-from-cache or stale answer the cache supplied).
    fn observe_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    /// A static (degraded) answer — a HIT that also records the staleness age (the `staleness_age`
    /// signal). The age never exceeds `static_max` (the FailStatic budget bounds it structurally).
    fn observe_stale(&self, age_secs: u64) {
        self.hits.fetch_add(1, Ordering::Relaxed);
        self.last_staleness_secs.store(age_secs, Ordering::Relaxed);
    }

    /// A fail-CLOSED answer (the staleness budget was spent / no fallback) — a cache MISS.
    fn observe_closed(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    /// A zookie-bypass read served straight from the authoritative source (cache not consulted).
    fn observe_bypass(&self) {
        self.bypasses.fetch_add(1, Ordering::Relaxed);
    }

    /// A zookie-bypass read that failed CLOSED on a hiccup (the new-enemy guard fired).
    fn observe_bypass_closed(&self) {
        self.bypasses.fetch_add(1, Ordering::Relaxed);
    }

    /// The count of cache HITs (fresh/stale served from cache).
    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    /// The count of cache MISSes (fail-closed served).
    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }

    /// The count of zookie-bypass reads (served from source, cache untouched).
    pub fn bypasses(&self) -> u64 {
        self.bypasses.load(Ordering::Relaxed)
    }

    /// The `cache_hit_ratio` as an integer percentage (0..=100), or `None` before any
    /// cache-consulting read (no ratio over zero — an empty ratio is honestly absent, never a
    /// fabricated 100). Bypass reads are NOT in the denominator (they never consulted the cache).
    pub fn cache_hit_ratio_pct(&self) -> Option<u64> {
        let hits = self.hits();
        let total = hits + self.misses();
        (hits * 100).checked_div(total)
    }

    /// The `staleness_age` (seconds) of the most-recent static answer (0 when not stale).
    pub fn last_staleness_secs(&self) -> u64 {
        self.last_staleness_secs.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalKind};
    use myelin_substrate::TestClock;
    use myelin_tenancy::{Region, TenantId};

    /// The drill bound: agent-token TTL = 60s (lower), revocation SLA = 300s (upper). `static_max`
    /// seed = 300 (the largest the constraint admits). Mirrors the thresholds.toml `[fail_static]`
    /// row (the engineering seed; the ratified W is `[OPEN — LEGAL]`).
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

    fn scope(tenant: &str) -> TenantScope {
        let p = Principal::stub(
            PrincipalId("p-admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        );
        TenantScope::from_verified_token(&p, Region("eu-west".into()))
    }

    fn ts(s: &str) -> Timestamp {
        Timestamp(s.into())
    }

    fn strong(z: &str) -> Consistency {
        Consistency {
            at_least: myelin_identity::Zookie(z.into()),
            mode: ConsistencyMode::Strong,
        }
    }

    fn bounded_stale() -> Consistency {
        Consistency {
            at_least: myelin_identity::Zookie(String::new()),
            mode: ConsistencyMode::BoundedStale,
        }
    }

    fn cache_at(t0: u64) -> FailStaticCache<TestClock> {
        FailStaticCache::try_new_with_clock(300, &threshold(), RevocationStore::new(), TestClock::at(t0))
            .expect("valid bound")
    }

    fn allow() -> Result<Decision, ServeError> {
        Ok(Decision::Allow)
    }
    fn hiccup() -> Result<Decision, ServeError> {
        Err(ServeError("identity hiccup".into()))
    }

    /// **The structural bound (4.11) is enforced by the constructor.** A `static_max` over the
    /// revocation SLA REJECTS — S6 cannot be built with a window that outlives the revocation SLA.
    #[test]
    fn constructor_enforces_the_fail_static_bound() {
        // A threshold whose seed exceeds the revocation SLA (301 > 300) does not construct.
        let mut bad = threshold();
        bad.static_max_default_secs = 301;
        match FailStaticCache::try_new_with_clock(300, &bad, RevocationStore::new(), TestClock::at(0)) {
            Err(FailStaticError::ExceedsRevocationSla { .. }) => {}
            Err(other) => panic!("expected ExceedsRevocationSla, got {other:?}"),
            Ok(_) => panic!("a static_max over the revocation SLA must reject (4.11 / §8.2)"),
        }
        // The valid seed (300 == SLA, ≥ token TTL) constructs; its static_max is the seed W.
        let c = cache_at(0);
        assert_eq!(c.static_max(), 300, "S6's W is the thresholds-file engineering seed");
    }

    /// **S6 serves coarse grants during an injected Id-hiccup (availability fails static).** A
    /// `BoundedStale` read caches a fresh authoritative Allow, then — on a hiccup — keeps serving it
    /// STATIC (degraded) within the staleness budget: authenticated traffic survives the hiccup.
    #[test]
    fn bounded_stale_serves_static_during_a_hiccup() {
        let c = cache_at(1_000);
        let acme = scope("acme");
        let subj = PrincipalId("p:alice".into());

        // 1) a healthy read caches the Allow + serves it Fresh.
        let fresh = c.check_cached(&acme, &subj, "read@doc:1", &bounded_stale(), &ts("z"), allow);
        assert_eq!(fresh.served, Served::Fresh);
        assert!(fresh.is_allow(), "the healthy read allows");

        // advance past fresh_ttl (age 31 > 30) but within static_max → STATIC (degraded) on a hiccup.
        c.clock().advance(31);
        let stale = c.check_cached(&acme, &subj, "read@doc:1", &bounded_stale(), &ts("z"), hiccup);
        assert_eq!(stale.served, Served::Static, "the hiccup is survived on the static fallback");
        assert!(stale.is_allow(), "authenticated traffic survives the hiccup (still Allow)");
        assert!(stale.is_degraded(), "the answer is marked degraded (bounded-staleness win)");
    }

    /// **A zookie-stamped (`Strong`) read BYPASSES S6 (the 4.10 bypass half).** It goes straight to
    /// the authoritative source — even when S6 holds a stale grant for the same subject — and on a
    /// hiccup fails CLOSED (never serves stale). This is the new-enemy guard.
    #[test]
    fn strong_read_bypasses_the_cache_and_fails_closed_on_hiccup() {
        let c = cache_at(1_000);
        let acme = scope("acme");
        let subj = PrincipalId("p:alice".into());

        // Warm S6 with a stale Allow via a BoundedStale read.
        let _ = c.check_cached(&acme, &subj, "read@doc:1", &bounded_stale(), &ts("z"), allow);
        c.clock().advance(31); // S6 now holds a stale-eligible Allow

        // A STRONG read with a hiccup does NOT serve the stale S6 allow — it fails CLOSED.
        let strong_hiccup =
            c.check_cached(&acme, &subj, "read@doc:1", &strong("z2"), &ts("z2"), hiccup);
        assert_eq!(strong_hiccup.served, Served::BypassClosed, "strong read bypassed the cache");
        assert!(strong_hiccup.is_deny(), "a strong read fails CLOSED on a hiccup (never stale)");

        // A STRONG read with a healthy source serves the AUTHORITATIVE answer (bypass), not the cache.
        let strong_ok =
            c.check_cached(&acme, &subj, "read@doc:1", &strong("z2"), &ts("z2"), || Ok(Decision::Deny));
        assert_eq!(strong_ok.served, Served::SourceBypass, "strong read served from source");
        assert!(strong_ok.is_deny(), "the strong read returns the live authoritative Deny, not the cached Allow");
    }

    /// **A just-revoked grant is still denied (the F7 / ID-D2 correctness floor).** Even when S6
    /// holds a stale ALLOW for the subject, a SCIM-disable / `revoke(principal)` of that subject is
    /// DENIED through the stale cache — the S7 consult on every read. 0 successful authz after the
    /// cache for a revoked subject.
    #[test]
    fn revoked_subject_is_denied_even_with_a_stale_s6_allow() {
        let c = cache_at(1_000);
        let acme = scope("acme");
        let subj = PrincipalId("p:alice".into());

        // Warm S6 with a stale Allow.
        let _ = c.check_cached(&acme, &subj, "read@doc:1", &bounded_stale(), &ts("z"), allow);
        c.clock().advance(31);
        // Confirm it WOULD serve a stale allow absent a revocation.
        let before = c.check_cached(&acme, &subj, "read@doc:1", &bounded_stale(), &ts("z"), hiccup);
        assert!(before.is_allow() && before.is_degraded(), "the stale allow is live before revoke");

        // SCIM-disable the subject (the §4 authoritative deny path).
        c.revocations()
            .disable_principal(&acme, &subj, ts("2026-06-19T00:00:00Z"));

        // Now even the stale S6 allow is DENIED — the revoke reaches through the cache.
        let after = c.check_cached(
            &acme,
            &subj,
            "read@doc:1",
            &bounded_stale(),
            &ts("2026-06-19T00:01:00Z"),
            hiccup,
        );
        assert_eq!(after.served, Served::Revoked, "the revoke is enforced before the cache is read");
        assert!(after.is_deny(), "0 successful authz after the cache for a revoked subject (F7)");
    }

    /// **Correctness fails CLOSED while availability fails STATIC.** Past the staleness budget a
    /// `BoundedStale` read fails CLOSED (deny) — it does NOT keep serving stale forever (the budget
    /// is bounded ≤ the revocation SLA). Never fail open.
    #[test]
    fn past_the_budget_bounded_stale_fails_closed() {
        let c = cache_at(1_000);
        let acme = scope("acme");
        let subj = PrincipalId("p:alice".into());
        let _ = c.check_cached(&acme, &subj, "read@doc:1", &bounded_stale(), &ts("z"), allow);

        // advance one past static_max (age 301 > 300) → fail CLOSED (deny), never open.
        c.clock().advance(301);
        let closed = c.check_cached(&acme, &subj, "read@doc:1", &bounded_stale(), &ts("z"), hiccup);
        assert_eq!(closed.served, Served::Closed, "past the budget the cache fails closed");
        assert!(closed.is_deny(), "fail CLOSED (deny is correct), never fail open");
    }

    /// **A cold `BoundedStale` read with no fallback fails CLOSED (never open).** A hiccup before any
    /// cached grant exists denies — S6 never fabricates an allow.
    #[test]
    fn cold_bounded_stale_hiccup_fails_closed() {
        let c = cache_at(0);
        let acme = scope("acme");
        let subj = PrincipalId("p:bob".into());
        let cold = c.check_cached(&acme, &subj, "read@doc:1", &bounded_stale(), &ts("z"), hiccup);
        assert_eq!(cold.served, Served::Closed, "no fallback → fail closed");
        assert!(cold.is_deny(), "S6 never fabricates an allow (never fail open)");
    }

    /// **The cache is `(tenant, region, subject, question)`-keyed — no cross-tenant / cross-question
    /// leak.** Alice's cached grant for one question in one tenant does not serve bob, another
    /// tenant, or another question.
    #[test]
    fn cache_key_isolates_tenant_subject_and_question() {
        let c = cache_at(1_000);
        let acme = scope("acme");
        let evil = scope("evil-corp");
        let alice = PrincipalId("p:alice".into());
        let bob = PrincipalId("p:bob".into());

        // Warm acme/alice/read@doc:1 with an Allow.
        let _ = c.check_cached(&acme, &alice, "read@doc:1", &bounded_stale(), &ts("z"), allow);
        c.clock().advance(31);

        // A DIFFERENT tenant (same id) has no cached grant → a hiccup fails closed (not alice's allow).
        let cross_tenant = c.check_cached(&evil, &alice, "read@doc:1", &bounded_stale(), &ts("z"), hiccup);
        assert!(cross_tenant.is_deny(), "no cross-tenant cache leak");
        // A DIFFERENT subject has no cached grant → fails closed.
        let cross_subject = c.check_cached(&acme, &bob, "read@doc:1", &bounded_stale(), &ts("z"), hiccup);
        assert!(cross_subject.is_deny(), "no cross-subject cache leak");
        // A DIFFERENT question has no cached grant → fails closed.
        let cross_question = c.check_cached(&acme, &alice, "write@doc:1", &bounded_stale(), &ts("z"), hiccup);
        assert!(cross_question.is_deny(), "no cross-question cache leak");
    }

    /// **The telemetry (contract 1.8) records cache_hit_ratio + staleness_age under the FROZEN signal
    /// names.** Observability is part of the pass (EI-01 §3).
    #[test]
    fn telemetry_records_hit_ratio_and_staleness_under_frozen_names() {
        // the frozen names (the drill asserts against these, never a literal).
        assert_eq!(CacheTelemetry::CACHE_HIT_RATIO, "cache_hit_ratio");
        assert_eq!(CacheTelemetry::STALENESS_AGE, "staleness_age");

        let c = cache_at(1_000);
        let acme = scope("acme");
        let subj = PrincipalId("p:alice".into());

        // fresh hit
        let _ = c.check_cached(&acme, &subj, "read@doc:1", &bounded_stale(), &ts("z"), allow);
        // stale hit (records a staleness age)
        c.clock().advance(31);
        let _ = c.check_cached(&acme, &subj, "read@doc:1", &bounded_stale(), &ts("z"), hiccup);
        // closed miss
        c.clock().advance(301);
        let _ = c.check_cached(&acme, &subj, "read@doc:1", &bounded_stale(), &ts("z"), hiccup);

        let t = c.telemetry();
        assert_eq!(t.hits(), 2, "two cache hits (fresh + stale)");
        assert_eq!(t.misses(), 1, "one fail-closed miss");
        assert_eq!(t.cache_hit_ratio_pct(), Some(66), "2/3 hits ≈ 66%");
        assert!(t.last_staleness_secs() > 0, "a staleness age was recorded");
        assert!(
            t.last_staleness_secs() <= c.static_max(),
            "staleness_age never exceeds static_max (≤ the revocation SLA)"
        );
    }

    /// **A zookie-bypass read is NOT in the cache_hit_ratio denominator.** It never consulted the
    /// cache, so it counts as a bypass, not a hit or a miss (the ratio reflects cache effectiveness).
    #[test]
    fn bypass_reads_are_not_in_the_hit_ratio() {
        let c = cache_at(1_000);
        let acme = scope("acme");
        let subj = PrincipalId("p:alice".into());
        // two strong reads (one ok, one hiccup) — both bypass; neither is a hit or miss.
        let _ = c.check_cached(&acme, &subj, "read@doc:1", &strong("z"), &ts("z"), allow);
        let _ = c.check_cached(&acme, &subj, "read@doc:1", &strong("z"), &ts("z"), hiccup);
        let t = c.telemetry();
        assert_eq!(t.bypasses(), 2, "both strong reads bypassed the cache");
        assert_eq!(t.hits(), 0);
        assert_eq!(t.misses(), 0);
        assert_eq!(t.cache_hit_ratio_pct(), None, "no cache-consulting read → no ratio (never fabricated)");
    }

    /// **The fresh/stale/closed survival signals are exposed (architecture §10.2 row 6).** The drill
    /// reads the fail-static answer ratio off this — the `{actor_active, coarse_grants}` survival
    /// signal the F7 family asserts.
    #[test]
    fn fail_static_survival_signals_are_exposed() {
        let c = cache_at(1_000);
        let acme = scope("acme");
        let subj = PrincipalId("p:alice".into());
        let _ = c.check_cached(&acme, &subj, "read@doc:1", &bounded_stale(), &ts("z"), allow); // fresh
        c.clock().advance(31);
        let _ = c.check_cached(&acme, &subj, "read@doc:1", &bounded_stale(), &ts("z"), hiccup); // stale
        let s = c.fail_static_signals();
        assert_eq!(s.fresh, 1, "one fresh answer");
        assert_eq!(s.stale, 1, "one stale (degraded) answer — the survival rung");
        assert!(s.last_staleness_secs <= c.static_max(), "staleness ≤ the budget");
    }

    /// **`CoarseGrant::from_decision` never fabricates an allow + caches only authoritative answers.**
    /// A `Deny` source caches a Deny (not an actor-active allow); the cache replays it, never an
    /// escalation.
    #[test]
    fn coarse_grant_never_escalates() {
        let c = cache_at(1_000);
        let acme = scope("acme");
        let subj = PrincipalId("p:alice".into());
        // a DENY source caches a Deny.
        let d = c.check_cached(&acme, &subj, "read@doc:1", &bounded_stale(), &ts("z"), || Ok(Decision::Deny));
        assert!(d.is_deny(), "a Deny source serves Deny");
        // on a hiccup the cached Deny is replayed STALE — still a Deny (never escalated to Allow).
        c.clock().advance(31);
        let stale = c.check_cached(&acme, &subj, "read@doc:1", &bounded_stale(), &ts("z"), hiccup);
        assert_eq!(stale.served, Served::Static);
        assert!(stale.is_deny(), "the stale fallback replays the cached Deny — never escalates to Allow");
    }
}
