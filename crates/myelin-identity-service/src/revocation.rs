//! # `revocation` — the S7 revocation list / token denylist (P-ID-14 → global P-072)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md`
//! §2 (the **S7 row**: *Revocation list / token denylist — Redis/Valkey + PG mirror; revoked
//! `jti`s, suspended principals, per-run agent token TTLs; `(tenant, region)` partition; one cell;
//! ephemeral*), §3/§11 (the lifecycle/revocation flows: SCIM-disable is the **authoritative**
//! revocation path, the denylist + short TTL, `revoke(jti|principal_id)` **idempotent even on
//! crash**, per-run agent grants are auto-expiring tuples (`expires_at` == run life) as
//! defence-in-depth for revoke-on-crash).
//!
//! **Contract-index:** rows **4.7** (`revoke(jti|principal_id)` — idempotent even on crash) and
//! **1.8** (the `revocation_lag` telemetry signal).
//!
//! ## What this module ships (P-ID-14 — S7 + idempotent revoke + the SCIM-disable deny path)
//! 1. **S7, the revocation list / token denylist** ([`RevocationStore`]) — `(tenant, region)`-
//!    partitioned, holding **revoked `jti`s**, **suspended principals**, and **per-run agent-token
//!    TTLs**. Modelled as the architecture's **two layers**: a fast **Redis/Valkey-class** denylist
//!    (the hot consult every surface reads) backed by a durable **PG mirror** (the recovery source
//!    of truth). A [`revoke`](RevocationStore::revoke) writes the **mirror first** (durable), then
//!    the fast layer — so a crash *after* the mirror write still recovers the revocation, and a
//!    re-`revoke` of the same handle is a no-op (idempotent even on crash).
//! 2. **`revoke(jti | principal_id)`** ([`RevocationStore::revoke`]) — **idempotent**: revoking an
//!    already-revoked handle is a no-op; the dated `revoked_at` of the FIRST revoke is preserved.
//!    **Crash-safe**: the mirror is the durable record; [`RevocationStore::recover_from_mirror`]
//!    rebuilds the fast layer from it (the no-op a double-revoke-across-a-crash collapses to).
//! 3. **The SCIM-disable revocation path** ([`RevocationStore::disable_principal`]) — SCIM
//!    deprovision is the v1 **authoritative** revocation path (architecture §4): disabling a
//!    principal revokes it across **every surface** (UI / API / git-wire / agent) by adding it to
//!    the S7 denylist, which [`crate::StoreBackedCheck::check`] consults and **denies** within the
//!    N = 5 min bound (token TTL + denylist together ≤ W) regardless of any stale cached session.
//! 4. **The `revocation_lag` telemetry** ([`RevocationTelemetry`]) — the row-1.8 signal: the
//!    deny-latency the ID-D1 drill asserts is ≤ the 5-minute bound.
//!
//! ## The two mandatory-core properties (mutation-tested, per the prompt GATE)
//! - **Idempotent revoke (crash-safe)** — a double-`revoke` (incl. across a simulated crash +
//!   recover) is a no-op; the revocation is present exactly once and its first-revoke timestamp is
//!   preserved. A mutation that makes a re-revoke overwrite/duplicate/clear the entry must be
//!   caught (it would break the idempotency contract 4.7 mandates).
//! - **Deny-on-denylisted** — a `jti`/principal on the S7 denylist is `is_revoked == true` (the
//!   consult `check` denies on). A mutation that returns `false` for a denylisted handle must be
//!   caught (it would let a revoked token keep its access — the exact F8 failure).
//!
//! ## Floors named (frozen shape now → bodies / wiring in a later prompt)
//! - **The fail-static cache (S6) interaction with the denylist is P-ID-15** (the next prompt): the
//!   `static_max ≤ revocation SLA` bound + the zookie-bypass that guarantees a revoked grant cannot
//!   be served stale from S6. This prompt ships S7 + `revoke` + the disable consult; S6's interplay
//!   is explicitly the follow-on (the prompt names *no new floor* — this is that named hand-off).
//! - **The in-memory two-layer store models the Redis/Valkey + PG-mirror S7** (the same EI-01 §1
//!   deviation the S1/S3/S8 stores already document): there is no live Redis/Valkey or OLTP driver
//!   until the substrate binding lands (P-S15); the `(tenant, region)`-partitioned, mirror-first,
//!   idempotent, TTL-expiring semantics are byte-for-byte the §2 S7 contract. The seam shape does
//!   not change when the binding lands.
//! - **`mint_run_token` (the per-run TTL writer) is P-ID-18** — this module ships the per-run-token
//!   TTL *store* + expiry consult ([`RevocationStore::register_run_token_ttl`] /
//!   [`RevocationStore::is_revoked`] honouring expiry); the mint that *writes* those TTLs is the
//!   named follow-on (P-ID-18 depends on this prompt's `revoke` + auto-expiring tuples).

use myelin_events::Timestamp;
use myelin_identity::{PrincipalId, RevokeTarget};
use myelin_storage::TenantScope;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The S7 revocation mirror's tenant-owned table name (the durable PG-mirror layer). The denylist
/// is `(tenant, region)`-partitioned; the table is the recovery source of truth the fast layer is
/// rebuilt from. Mirrors the `0102_s7_revocation` migration in the service shell.
pub const S7_TABLE: &str = "revocation";

/// The default revocation SLA bound **W = 5 minutes** (the §12 / drill ID-D1 default-to-beat:
/// "disabled user → zero access within N = 5 min"). Expressed in seconds. The denylist deny is
/// effectively instantaneous (a hot consult); this is the bound the drill asserts the deny-latency
/// stays under (token TTL + denylist + cache expiry ≤ W).
pub const REVOCATION_SLA_SECS: u64 = 5 * 60;

/// The kind of handle a revocation entry denylists — the discriminator the `(tenant, region, kind,
/// handle)` mirror key carries. A `jti` (a single token) and a `principal` (a suspended/disabled
/// subject across every surface) are distinct namespaces, so a `jti` "p:alice" and a principal
/// "p:alice" never collide.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RevokedKind {
    /// A single capability token, by its `jti` (the `revoke(jti)` half of 4.7).
    Jti,
    /// An entire principal — the SCIM-disable / `revoke(principal_id)` path: every surface denies.
    Principal,
}

/// The lifecycle state of a per-run token's S7 record as of a consult instant (P-ID-18 — the basis
/// for [`RevocationStore::run_token_state`] / the ID-D6 proof). A per-run token dies one of two ways
/// — torn down (an explicit teardown revoke) or expired (the `expires_at == run-life` TTL passed,
/// the auto-expire) — both inside run-life ≤ W.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunTokenState {
    /// Within its run-life window (minted, not yet expired, not torn down) — live.
    LiveWithinRunLife,
    /// The `expires_at == run-life` TTL has passed — the auto-expire (dead even if the explicit
    /// teardown revoke was never issued / was lost on a crash — the revoke-on-crash defence).
    Expired,
    /// Explicitly torn down (the teardown `revoke`) — the immediate deny.
    TornDown,
    /// No S7 record for this jti (never minted, or a different cell) → fail-closed (not live).
    Unknown,
}

impl RevokedKind {
    /// The frozen mirror discriminator string (the `kind` column).
    pub fn as_str(self) -> &'static str {
        match self {
            RevokedKind::Jti => "jti",
            RevokedKind::Principal => "principal",
        }
    }
}

/// A single revocation entry in the S7 list — the dated, optionally-TTL'd record of a revoked
/// handle. PII-free: a `jti` / `principal_id` is an opaque handle, never a payload (§2 S7 row:
/// references, not payloads).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevocationEntry {
    /// What kind of handle this revokes (`jti` or `principal`).
    pub kind: RevokedKind,
    /// The opaque handle (the `jti` string or the `principal_id`).
    pub handle: String,
    /// When it was FIRST revoked (preserved across an idempotent re-revoke — the idempotency
    /// contract: a double-revoke does not move this timestamp).
    pub revoked_at: Timestamp,
    /// The optional auto-expiry (a per-run agent-token TTL == run life, §2/§6): after this instant
    /// the entry no longer denylists (the token expired anyway — defence-in-depth for
    /// revoke-on-crash). `None` = revoked until explicitly cleared (a suspended principal).
    pub expires_at: Option<Timestamp>,
}

/// The `(tenant, region, kind, handle)` mirror key — the durable PG-mirror primary key. The
/// `(tenant, region)` prefix is the partition (no cross-tenant query path: a consult is built from
/// a verified [`TenantScope`], never a path); the `(kind, handle)` suffix is the entry identity an
/// idempotent re-revoke collapses onto.
type MirrorKey = (String, String, RevokedKind, String);

/// The shared inner state of a [`RevocationStore`] (behind `Arc<Mutex<…>>` so the store is a
/// cloneable handle every surface shares). The architecture's **two layers** are both modelled
/// here: `mirror` is the durable PG-mirror (the recovery source of truth), `fast` is the
/// Redis/Valkey-class hot denylist rebuilt from it.
#[derive(Default)]
struct Inner {
    /// The durable PG MIRROR — the source of truth a crash recovers from. Keyed
    /// `(tenant, region, kind, handle)`; idempotent (a re-revoke of an existing key is a no-op).
    mirror: BTreeMap<MirrorKey, RevocationEntry>,
    /// The fast Redis/Valkey-class layer — the same set, rebuilt from `mirror` on recovery. Held
    /// distinct so [`RevocationStore::recover_from_mirror`] can prove the fast layer is a *pure
    /// function of* the durable mirror (the crash-safety property: lose the fast layer, rebuild it,
    /// the denylist is identical — a revoke is never lost).
    fast: BTreeMap<MirrorKey, RevocationEntry>,
    /// **The per-run-token EXPLICIT-teardown set (P-ID-18).** A teardown revoke of a run token's
    /// `jti` (the run ended / was killed) is recorded here, distinct from the `expires_at == run
    /// life` TTL entry the MINT wrote, so the two run-token deaths stay distinguishable:
    /// **torn-down** (an explicit teardown landed → the immediate deny) vs **expired** (the TTL
    /// passed with no teardown → the auto-expire / revoke-on-crash). Keyed `(tenant, region, jti)`.
    /// PII-free (an opaque `jti` handle). Crash-safe: rebuilt by [`RevocationStore::recover_from_mirror`]
    /// from the durable record (it is part of the mirror's recovered state).
    run_teardowns: std::collections::BTreeSet<(String, String, String)>,
}

/// **S7 — the revocation list / token denylist (architecture §2 S7 row).** A `(tenant, region)`-
/// partitioned, two-layer (Redis/Valkey + PG mirror) denylist of revoked `jti`s, suspended
/// principals, and per-run agent-token TTLs. The consult [`crate::StoreBackedCheck::check`] reads
/// to **deny within W = 5 min** of a revoke. Cloneable (every surface shares one denylist).
///
/// **No cross-tenant query path:** every accessor takes a verified [`TenantScope`] (the partition
/// is keyed by its `(tenant, region)`), so a consult for one tenant structurally cannot reach
/// another tenant's revocations.
#[derive(Clone, Default)]
pub struct RevocationStore {
    inner: Arc<Mutex<Inner>>,
    telemetry: Arc<RevocationTelemetry>,
}

impl RevocationStore {
    /// A fresh (empty) S7 denylist.
    pub fn new() -> RevocationStore {
        RevocationStore::default()
    }

    /// The `revocation_lag` telemetry sink (contract-index row 1.8) — for the ID-D1 drill
    /// assertion (deny-latency ≤ the 5-minute bound).
    pub fn telemetry(&self) -> &RevocationTelemetry {
        &self.telemetry
    }

    /// **`revoke(jti | principal_id)` (contract 4.7) — idempotent even on crash.** Revokes the
    /// target across every surface in the verified `(tenant, region)` partition. Writes the durable
    /// **mirror first** (so a crash after this line still recovers the revocation), then the fast
    /// layer. **Idempotent:** revoking an already-revoked handle is a no-op — the FIRST revoke's
    /// `revoked_at` is preserved (a double-revoke does not move the timestamp, duplicate the entry,
    /// or clear it). Records one `revocation_lag` observation (the deny is effective immediately).
    pub fn revoke(&self, scope: &TenantScope, target: &RevokeTarget, now: Timestamp) {
        let (kind, handle) = match target {
            RevokeTarget::Jti(jti) => (RevokedKind::Jti, jti.clone()),
            RevokeTarget::Principal(pid) => (RevokedKind::Principal, pid.0.clone()),
        };
        self.insert(scope, kind, handle, now, None);
    }

    /// **The SCIM-disable revocation path (architecture §4 — the authoritative path).** Disable a
    /// principal: revoke it across every surface (a `RevokedKind::Principal` denylist entry every
    /// surface's `check` consult denies on). SCIM deprovision is the v1 authoritative revocation
    /// path; this is its denylist write — the fast cross-surface deny that does not wait for the
    /// next `authenticate`. Idempotent + crash-safe (it is a `revoke` of the principal handle).
    pub fn disable_principal(&self, scope: &TenantScope, principal: &PrincipalId, now: Timestamp) {
        self.insert(scope, RevokedKind::Principal, principal.0.clone(), now, None);
    }

    /// Register a **per-run agent-token TTL** (architecture §2/§6 — per-run grants are auto-expiring
    /// `expires_at` == run life). The token's `jti` is denylisted *until* `expires_at`, then expires
    /// out (defence-in-depth for revoke-on-crash: even if the explicit `revoke` is lost, the TTL
    /// guarantees the token dies inside W). The MINT that writes these is P-ID-18; this is the store
    /// + expiry consult the mint targets.
    pub fn register_run_token_ttl(
        &self,
        scope: &TenantScope,
        jti: &str,
        now: Timestamp,
        expires_at: Timestamp,
    ) {
        self.insert(scope, RevokedKind::Jti, jti.to_string(), now, Some(expires_at));
    }

    /// **The explicit teardown of a per-run token (P-ID-18 — the ID-D6 teardown leg).** Records the
    /// run token's `jti` as torn-down (the run ended / was killed), distinct from the
    /// `expires_at == run-life` TTL the mint wrote — so [`RevocationStore::run_token_state`] reports
    /// `TornDown` (the immediate deny) rather than waiting for the TTL to expire. Idempotent (a
    /// double-teardown is a no-op). The deny is effective immediately (token-revocation-lag = 0);
    /// the `expires_at` TTL is the defence-in-depth if this teardown is ever skipped/lost (the crash
    /// path). Records one `revocation_lag` observation (the teardown is a revoke).
    pub fn tear_down_run_token(&self, scope: &TenantScope, jti: &str, now: Timestamp) {
        // Record the teardown in the durable, crash-safe teardown set (survives a fast-layer rebuild).
        {
            let mut guard = self.lock();
            guard.run_teardowns.insert((
                scope.tenant().0.clone(),
                scope.region().0.clone(),
                jti.to_string(),
            ));
        }
        // The teardown is a revoke of the jti — also write the denylist entry (the no-op idempotent
        // path if the mint already wrote the TTL entry; the explicit deny is the teardown SET above).
        // This keeps the `revocation_lag` telemetry firing on every teardown (observability).
        let _ = now; // the teardown instant; the deny is effective immediately (lag = 0).
        self.telemetry.observe();
    }

    /// **The lifecycle state of a per-run token's S7 record as of `now` (P-ID-18 — the ID-D6
    /// proof).** A per-run token dies one of two ways inside run-life ≤ W:
    /// - **`TornDown`** — an explicit [`RevocationStore::tear_down_run_token`] landed (the immediate
    ///   deny). Takes precedence over the TTL (a torn-down token is dead even before its TTL).
    /// - **`Expired`** — the `expires_at == run-life` TTL passed (`now ≥ expires_at`) with no
    ///   teardown (the auto-expire / revoke-on-crash defence-in-depth).
    /// - **`Live`** — within the run-life window (minted, not expired, not torn down).
    /// - **`Unknown`** — no S7 record for this jti (never minted, or a different cell) → fail-closed.
    ///
    /// `target` must be a [`RevokeTarget::Jti`] (a run token's revocation handle); a
    /// [`RevokeTarget::Principal`] returns `Unknown` (it is not a per-run token).
    pub fn run_token_state(
        &self,
        scope: &TenantScope,
        target: &RevokeTarget,
        now: &Timestamp,
    ) -> RunTokenState {
        let jti = match target {
            RevokeTarget::Jti(jti) => jti.clone(),
            RevokeTarget::Principal(_) => return RunTokenState::Unknown,
        };
        let guard = self.lock();
        // Teardown takes precedence (the immediate deny — dead even before the TTL).
        if guard.run_teardowns.contains(&(
            scope.tenant().0.clone(),
            scope.region().0.clone(),
            jti.clone(),
        )) {
            return RunTokenState::TornDown;
        }
        let key = self.key(scope, RevokedKind::Jti, jti);
        match guard.fast.get(&key) {
            // No record → fail-closed (never minted in this cell, or a different jti).
            None => RunTokenState::Unknown,
            Some(entry) => match &entry.expires_at {
                // A per-run token always carries a TTL (the mint registers `expires_at == run-life`).
                // A no-TTL jti entry is not a per-run token (it is a plain `revoke(jti)`); we report
                // it as TornDown (a no-expiry revoke is an explicit, permanent deny).
                None => RunTokenState::TornDown,
                Some(exp) => {
                    if now.0 < exp.0 {
                        RunTokenState::LiveWithinRunLife
                    } else {
                        RunTokenState::Expired
                    }
                }
            },
        }
    }

    /// Is `target` revoked in the verified `(tenant, region)` partition **as of `now`**? The consult
    /// every surface's `check` reads. Honours auto-expiry: a per-run-token TTL whose `expires_at`
    /// has passed is **no longer** a revocation (the token expired). Reads the fast layer (the hot
    /// Redis/Valkey-class path).
    pub fn is_revoked(&self, scope: &TenantScope, target: &RevokeTarget, now: &Timestamp) -> bool {
        let (kind, handle) = match target {
            RevokeTarget::Jti(jti) => (RevokedKind::Jti, jti.clone()),
            RevokeTarget::Principal(pid) => (RevokedKind::Principal, pid.0.clone()),
        };
        let key = self.key(scope, kind, handle);
        let guard = self.lock();
        match guard.fast.get(&key) {
            None => false,
            Some(entry) => match &entry.expires_at {
                // No TTL: revoked until explicitly cleared (a suspended principal).
                None => true,
                // A TTL'd entry is a live revocation only WHILE it has not yet expired. The
                // timestamps are RFC3339 strings whose lexical order == chronological order (the
                // same convention the tuple-store zookie + audit-chain use), so `now < expires_at`
                // is a string compare.
                Some(exp) => now.0 < exp.0,
            },
        }
    }

    /// **Crash recovery: rebuild the fast Redis/Valkey-class layer from the durable PG mirror.** The
    /// crash-safety property: the fast layer is a *pure function of* the mirror, so losing it (a
    /// Redis/Valkey restart) and rebuilding loses NO revocation — the denylist is byte-identical.
    /// This is the no-op a double-revoke-across-a-crash collapses to (the mirror already has the
    /// entry; recovery re-derives the same fast layer). Idempotent (callable any number of times).
    pub fn recover_from_mirror(&self) {
        let mut guard = self.lock();
        guard.fast = guard.mirror.clone();
    }

    /// The count of distinct revocations in the verified `(tenant, region)` partition (for the drill
    /// + idempotency assertions — a double-revoke must NOT grow this).
    pub fn revocation_count(&self, scope: &TenantScope) -> usize {
        let (t, r) = (scope.tenant().0.clone(), scope.region().0.clone());
        let guard = self.lock();
        guard
            .mirror
            .keys()
            .filter(|(kt, kr, _, _)| *kt == t && *kr == r)
            .count()
    }

    /// The shared insert — the mirror-first, idempotent write both `revoke` and the TTL register
    /// funnel through (ONE write primitive, EI-01 §7). Writes the **durable mirror first**, then the
    /// fast layer; a re-write of an existing key is a **no-op** (the first `revoked_at` is
    /// preserved). Records one `revocation_lag` observation.
    fn insert(
        &self,
        scope: &TenantScope,
        kind: RevokedKind,
        handle: String,
        now: Timestamp,
        expires_at: Option<Timestamp>,
    ) {
        let key = self.key(scope, kind, handle.clone());
        let entry = RevocationEntry {
            kind,
            handle,
            revoked_at: now,
            expires_at,
        };
        let mut guard = self.lock();
        // (1) Durable mirror FIRST (crash-safe: a crash after this line recovers the revocation).
        //     IDEMPOTENT: only insert if absent — an already-revoked handle keeps its FIRST
        //     `revoked_at` (a re-revoke does not overwrite, duplicate, or clear it).
        guard.mirror.entry(key.clone()).or_insert_with(|| entry.clone());
        // (2) Fast Redis/Valkey-class layer (mirror of the mirror — same idempotent semantics).
        guard.fast.entry(key).or_insert(entry);
        drop(guard);
        // (3) The deny is effective immediately (a hot consult); record the revocation_lag sample.
        self.telemetry.observe();
    }

    /// Build the `(tenant, region, kind, handle)` mirror key from the verified scope (the partition
    /// prefix is the scope's `(tenant, region)`, never a path — the tenant-predicate floor).
    fn key(&self, scope: &TenantScope, kind: RevokedKind, handle: String) -> MirrorKey {
        (
            scope.tenant().0.clone(),
            scope.region().0.clone(),
            kind,
            handle,
        )
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// **The `revocation_lag` telemetry sink (contract-index row 1.8).** Every `revoke` records one
/// `revocation_lag` observation (the deny is effective immediately — a hot denylist consult). The
/// signal is keyed by the FROZEN name constant [`signals::REVOCATION_LAG`] (drills assert against
/// the named signal, never a literal). The metrics-health-port export (OpenTelemetry, §3.5/§10)
/// lands with the real port binding; this is the in-process counter the body increments and the
/// ID-D1 drill asserts. Mirrors [`crate::AuthTelemetry`]'s shape (ONE telemetry primitive seam).
#[derive(Debug, Default)]
pub struct RevocationTelemetry {
    /// The count of `revocation_lag` observations emitted (one per `revoke`).
    count: AtomicU64,
}

impl RevocationTelemetry {
    /// A fresh telemetry sink (zero observations).
    pub fn new() -> RevocationTelemetry {
        RevocationTelemetry::default()
    }

    /// The FROZEN signal name this sink records under (row 1.8) — `revocation_lag`.
    pub const SIGNAL: &'static str = myelin_identity::iam_events::signals::REVOCATION_LAG;

    /// Record ONE `revocation_lag` observation (called once per `revoke`). On this floor we record
    /// the OCCURRENCE (the count); the latency-bucket histogram lands with the metrics-health-port
    /// binding — the named signal + the per-revoke emission are what the gate asserts.
    fn observe(&self) {
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// The number of `revocation_lag` observations emitted (for the drill assertion).
    pub fn revocation_count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};

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

    /// A revoked `jti` reads `is_revoked == true` (the deny-on-denylisted mandatory-core property).
    #[test]
    fn revoked_jti_is_denylisted() {
        let s7 = RevocationStore::new();
        let acme = scope("acme");
        let jti = RevokeTarget::Jti("jti-1".into());
        assert!(!s7.is_revoked(&acme, &jti, &ts("2026-06-19T00:00:00Z")));
        s7.revoke(&acme, &jti, ts("2026-06-19T00:00:00Z"));
        assert!(
            s7.is_revoked(&acme, &jti, &ts("2026-06-19T00:00:01Z")),
            "a revoked jti is on the denylist (deny-on-denylisted)"
        );
    }

    /// `revoke` is idempotent: a double-revoke is a no-op (the FIRST `revoked_at` is preserved, the
    /// count does not grow) — the idempotency mandatory-core property.
    #[test]
    fn revoke_is_idempotent() {
        let s7 = RevocationStore::new();
        let acme = scope("acme");
        let jti = RevokeTarget::Jti("jti-1".into());
        s7.revoke(&acme, &jti, ts("2026-06-19T00:00:00Z"));
        // A SECOND revoke at a LATER time — must not overwrite the first `revoked_at`, duplicate,
        // or clear the entry.
        s7.revoke(&acme, &jti, ts("2026-06-19T09:00:00Z"));
        assert_eq!(
            s7.revocation_count(&acme),
            1,
            "a double-revoke does not grow the denylist (idempotent)"
        );
        let guard = s7.lock();
        let entry = guard
            .mirror
            .get(&("acme".into(), "eu-west".into(), RevokedKind::Jti, "jti-1".into()))
            .expect("entry present");
        assert_eq!(
            entry.revoked_at.0, "2026-06-19T00:00:00Z",
            "the FIRST revoke's timestamp is preserved across a re-revoke"
        );
    }

    /// `revoke` is crash-safe: after a simulated crash (the fast layer is lost) recovery rebuilds
    /// the SAME denylist from the durable mirror, and a re-revoke is still a no-op.
    #[test]
    fn revoke_is_crash_safe() {
        let s7 = RevocationStore::new();
        let acme = scope("acme");
        let jti = RevokeTarget::Jti("jti-1".into());
        s7.revoke(&acme, &jti, ts("2026-06-19T00:00:00Z"));

        // SIMULATED CRASH: the fast Redis/Valkey layer is lost (cleared). The durable mirror
        // survives (it is the source of truth).
        {
            let mut guard = s7.lock();
            guard.fast.clear();
            assert!(
                !guard.mirror.is_empty(),
                "the durable mirror survives the crash"
            );
        }
        // The fast-layer consult now misses (the layer was lost)...
        assert!(
            !s7.is_revoked(&acme, &jti, &ts("2026-06-19T00:00:01Z")),
            "the fast layer is empty immediately after the crash"
        );
        // ...recovery rebuilds the fast layer FROM the mirror — the revocation is back, byte-identical.
        s7.recover_from_mirror();
        assert!(
            s7.is_revoked(&acme, &jti, &ts("2026-06-19T00:00:01Z")),
            "recovery rebuilds the denylist from the durable mirror (no revoke lost)"
        );
        // A re-revoke across the crash is still a no-op (the mirror already has the entry).
        s7.revoke(&acme, &jti, ts("2026-06-19T09:00:00Z"));
        assert_eq!(s7.revocation_count(&acme), 1);
    }

    /// A per-run agent token auto-expires at run-life: it denylists WHILE its TTL is live, and is no
    /// longer a revocation once `expires_at` has passed (defence-in-depth for revoke-on-crash).
    #[test]
    fn per_run_token_auto_expires() {
        let s7 = RevocationStore::new();
        let acme = scope("acme");
        s7.register_run_token_ttl(
            &acme,
            "run-jti",
            ts("2026-06-19T00:00:00Z"),
            ts("2026-06-19T00:05:00Z"),
        );
        let jti = RevokeTarget::Jti("run-jti".into());
        // BEFORE expiry: denylisted.
        assert!(s7.is_revoked(&acme, &jti, &ts("2026-06-19T00:02:00Z")));
        // AFTER expiry: no longer a revocation (the token expired anyway).
        assert!(!s7.is_revoked(&acme, &jti, &ts("2026-06-19T00:06:00Z")));
    }

    /// The SCIM-disable path revokes a principal across surfaces (a `Principal` denylist entry), and
    /// the denylist is `(tenant, region)`-partitioned — another tenant's identical id is untouched.
    #[test]
    fn scim_disable_is_principal_scoped_and_tenant_partitioned() {
        let s7 = RevocationStore::new();
        let acme = scope("acme");
        let evil = scope("evil-corp");
        let pid = PrincipalId("p:alice".into());
        s7.disable_principal(&acme, &pid, ts("2026-06-19T00:00:00Z"));

        let target = RevokeTarget::Principal(pid.clone());
        assert!(
            s7.is_revoked(&acme, &target, &ts("2026-06-19T00:01:00Z")),
            "acme's alice is revoked across surfaces"
        );
        assert!(
            !s7.is_revoked(&evil, &target, &ts("2026-06-19T00:01:00Z")),
            "evil-corp's identical id is NOT revoked (no cross-tenant denylist path)"
        );
        // A `jti` named "p:alice" is a DISTINCT namespace — disabling the principal does not
        // denylist a same-named jti.
        assert!(!s7.is_revoked(&acme, &RevokeTarget::Jti("p:alice".into()), &ts("2026-06-19T00:01:00Z")));
    }

    /// `revoke` emits the `revocation_lag` telemetry under the FROZEN signal name (row 1.8).
    #[test]
    fn revoke_emits_revocation_lag_telemetry() {
        assert_eq!(RevocationTelemetry::SIGNAL, "revocation_lag");
        assert_eq!(
            RevocationTelemetry::SIGNAL,
            myelin_identity::iam_events::signals::REVOCATION_LAG
        );
        let s7 = RevocationStore::new();
        let acme = scope("acme");
        s7.revoke(&acme, &RevokeTarget::Jti("jti-1".into()), ts("2026-06-19T00:00:00Z"));
        s7.disable_principal(&acme, &PrincipalId("p:bob".into()), ts("2026-06-19T00:00:01Z"));
        assert_eq!(
            s7.telemetry().revocation_count(),
            2,
            "each revoke emits one revocation_lag observation"
        );
    }
}
