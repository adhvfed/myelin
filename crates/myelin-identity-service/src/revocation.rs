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
// `BTreeMap`/`Mutex` back the in-memory two-layer test-double [`Inner`] only (MR-009b Wave 2 —
// `test-support`-gated); the durable production path uses the PG backing.
#[cfg(any(test, feature = "test-support"))]
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
#[cfg(any(test, feature = "test-support"))]
use std::sync::Mutex;

/// **Compare two RFC3339 timestamps as INSTANTS (not lexically).** Returns `Some(now < expires_at)`
/// — `true` iff `now` is STRICTLY BEFORE the expiry instant. A raw lexical string compare of the
/// `Timestamp(pub String)` form is a FAIL-OPEN bug in security code: differing fractional precision
/// (`…00.5Z` vs `…00Z`), a non-`Z` offset (`…+02:00` == an earlier UTC instant), or a boundary form
/// (`.000Z` vs `Z`) all order WRONG lexically, so an expired token could read as live/not-revoked.
/// Parsing both to a `DateTime<FixedOffset>` and comparing by instant (chrono compares the underlying
/// UTC moment) closes that. Returns `None` if EITHER timestamp is unparseable — the caller decides the
/// fail-closed direction for its context (deny: stay-revoked for `is_revoked`, `Expired` for
/// `run_token_state`). We never read the wall clock here — `now` is always the caller-supplied instant.
fn now_strictly_before(now: &str, expires_at: &str) -> Option<bool> {
    match (
        chrono::DateTime::parse_from_rfc3339(now),
        chrono::DateTime::parse_from_rfc3339(expires_at),
    ) {
        (Ok(n), Ok(e)) => Some(n < e),
        // A malformed `now` OR `expires_at` is a parse failure → the caller fails CLOSED (deny).
        _ => None,
    }
}

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
/// idempotent re-revoke collapses onto. Keys the in-memory test-double mirror/fast maps (MR-009b
/// Wave 2 — `test-support`-gated; the durable PG mirror keys on the same tuple in SQL).
#[cfg(any(test, feature = "test-support"))]
type MirrorKey = (String, String, RevokedKind, String);

/// The shared inner state of a [`RevocationStore`] (behind `Arc<Mutex<…>>` so the store is a
/// cloneable handle every surface shares). The architecture's **two layers** are both modelled
/// here: `mirror` is the durable PG-mirror (the recovery source of truth), `fast` is the
/// Redis/Valkey-class hot denylist rebuilt from it.
///
/// **MR-009b Wave 2 — TEST DOUBLE (compiled ONLY under `#[cfg(any(test, feature = "test-support"))]`).**
/// The PRODUCTION default is the durable PG backing ([`PgRevocationBacking`], via
/// [`RevocationStore::with_pg`]); this in-memory two-layer model is the DB-free unit-test double
/// (SI-020 leaves the baseline).
#[cfg(any(test, feature = "test-support"))]
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
#[derive(Clone)]
pub struct RevocationStore {
    /// The durable backing — the REAL PG `revocation`/`run_token_teardown` tables (MR-008) on the
    /// production path, or the in-memory two-layer (mirror+fast) test-double on the default DB-free
    /// build. The system-of-record is the Pg backing; the in-memory model is an explicit test double.
    backend: RevocationBackend,
    telemetry: Arc<RevocationTelemetry>,
}

/// The S7 store backing: the REAL durable PG tables (MR-008) — the PRODUCTION default (MR-009b Wave
/// 2) — or the in-memory two-layer test-double. Splitting the backing OUT of the role struct's direct
/// fields lets the `no-in-memory-durable-store` ratchet (enum-following, MR-007) record the shortcut's
/// removal: the PRODUCTION-compiled enum presents ONLY the pool-backed `Pg` variant (the `Memory`
/// variant is `test-support`-gated, which the scanner strips as a test double), so `RevocationStore`
/// no longer holds an in-memory collection in the production graph (SI-020 leaves the baseline).
#[derive(Clone)]
enum RevocationBackend {
    /// The in-memory two-layer (mirror + fast) test-double — MR-009b Wave 2: compiled ONLY under
    /// `#[cfg(any(test, feature = "test-support"))]`. NOT the production system-of-record.
    #[cfg(any(test, feature = "test-support"))]
    Memory(Arc<Mutex<Inner>>),
    /// The REAL durable PG backing over the MR-022 provider pool + `with_tenant_tx` convention — the
    /// PRODUCTION DEFAULT (always compiled as of MR-009b Wave 2).
    Pg(PgRevocationBacking),
}

/// The PG-backed S7 backing (MR-008): the durable `revocation` mirror + `run_token_teardown` set +
/// the sync→async bridge (`tokio::runtime::Handle` driving `block_in_place`+`block_on`). On this path
/// the durable table IS the recovery source of truth — the "fast Redis/Valkey layer" collapses into
/// the DB (reads hit the table directly; `recover_from_mirror` is a no-op, nothing to rebuild). The
/// production default (always compiled as of MR-009b Wave 2).
#[derive(Clone)]
struct PgRevocationBacking {
    backing: Arc<myelin_storage::DurableRevocationBacking>,
    rt: tokio::runtime::Handle,
}

/// The in-memory test-double `Default` (MR-009b Wave 2 — `test-support`-gated, it calls the
/// in-memory [`RevocationStore::new`]). The production store is built durably via
/// [`RevocationStore::with_pg`], which has no `Default`.
#[cfg(any(test, feature = "test-support"))]
impl Default for RevocationStore {
    fn default() -> RevocationStore {
        RevocationStore::new()
    }
}

impl RevocationStore {
    /// A fresh (empty) S7 denylist over the in-memory TEST-DOUBLE backing (MR-009b Wave 2: compiled
    /// ONLY under `#[cfg(any(test, feature = "test-support"))]`). The PRODUCTION constructor is
    /// [`RevocationStore::with_pg`]; this `::new` is the DB-free unit-test entry point downstream
    /// crates reach via the `test-support` dev-dependency.
    #[cfg(any(test, feature = "test-support"))]
    pub fn new() -> RevocationStore {
        RevocationStore {
            backend: RevocationBackend::Memory(Arc::new(Mutex::new(Inner::default()))),
            telemetry: Arc::new(RevocationTelemetry::new()),
        }
    }

    /// **Build the S7 store over the REAL durable PG backing (MR-008 / SI-020).** Revocation entries +
    /// run-token TTLs + teardowns persist through the MR-022 [`myelin_storage::SubstrateProvider`]
    /// pool + `with_tenant_tx` convention (RLS-scoped, no GUC bleed). `rt` is the tokio runtime handle
    /// the sync API drives the async backing on. Preserves the API + telemetry + `(tenant, region)`
    /// scoping; expiry (`expires_at`) is durable so a revoked/expired token reads correctly after a
    /// fresh store instance over the same pool. **The PRODUCTION default (MR-009b Wave 2) — always
    /// compiled.**
    pub fn with_pg(
        backing: myelin_storage::DurableRevocationBacking,
        rt: tokio::runtime::Handle,
    ) -> RevocationStore {
        RevocationStore {
            backend: RevocationBackend::Pg(PgRevocationBacking {
                backing: Arc::new(backing),
                rt,
            }),
            telemetry: Arc::new(RevocationTelemetry::new()),
        }
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
        self.insert(
            scope,
            RevokedKind::Principal,
            principal.0.clone(),
            now,
            None,
        );
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
        self.insert(
            scope,
            RevokedKind::Jti,
            jti.to_string(),
            now,
            Some(expires_at),
        );
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
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RevocationBackend::Memory(inner) => {
                let mut guard = inner.lock().unwrap_or_else(|e| e.into_inner());
                guard.run_teardowns.insert((
                    scope.tenant().0.clone(),
                    scope.region().0.clone(),
                    jti.to_string(),
                ));
            }
            RevocationBackend::Pg(pg) => {
                // A teardown that cannot durably land must NOT silently succeed (a lost teardown would
                // let a torn-down token validate) — fail LOUD.
                pg.block(pg.backing.insert_teardown(&scope.tenant().0, jti))
                    .expect("durable run-token teardown must persist (fail-closed: never a silent lost teardown)");
            }
        }
        // The teardown is a revoke of the jti — the explicit deny is the teardown record above. This
        // keeps the `revocation_lag` telemetry firing on every teardown (observability).
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
        // The expiry decision over `(teardown?, expires_at?)` — shared by both backends so the
        // TornDown-precedence + TTL semantics never drift between the in-memory model and PG.
        let decide = |torn_down: bool, expires_at: Option<&str>| -> RunTokenState {
            if torn_down {
                return RunTokenState::TornDown; // immediate deny — dead even before the TTL.
            }
            match expires_at {
                // A no-TTL jti entry is not a per-run token (it is a plain `revoke(jti)`); report it
                // as TornDown (a no-expiry revoke is an explicit, permanent deny).
                None => RunTokenState::TornDown,
                // Compared as INSTANTS (not lexically). FAIL-CLOSED: an unparseable timestamp reads
                // `Expired` (a deny state), never `LiveWithinRunLife`.
                Some(exp) => {
                    if now_strictly_before(now.0.as_str(), exp).unwrap_or(false) {
                        RunTokenState::LiveWithinRunLife
                    } else {
                        RunTokenState::Expired
                    }
                }
            }
        };
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RevocationBackend::Memory(inner) => {
                let guard = inner.lock().unwrap_or_else(|e| e.into_inner());
                let torn_down = guard.run_teardowns.contains(&(
                    scope.tenant().0.clone(),
                    scope.region().0.clone(),
                    jti.clone(),
                ));
                if torn_down {
                    return RunTokenState::TornDown;
                }
                let key = self.key(scope, RevokedKind::Jti, jti);
                match guard.fast.get(&key) {
                    None => RunTokenState::Unknown, // no record → fail-closed.
                    Some(entry) => decide(false, entry.expires_at.as_ref().map(|t| t.0.as_str())),
                }
            }
            RevocationBackend::Pg(pg) => {
                // Read the durable teardown set + the revocation row. On a DB error, fail CLOSED:
                // return `Unknown` (a non-Live state every caller denies on) — never report a token
                // Live because the consult could not complete.
                let torn_down = pg
                    .block(pg.backing.is_teardown(&scope.tenant().0, &jti))
                    .unwrap_or(true); // error → treat as torn-down (deny), never as live.
                if torn_down {
                    return RunTokenState::TornDown;
                }
                match pg.block(pg.backing.get_revocation(
                    &scope.tenant().0,
                    RevokedKind::Jti.as_str(),
                    &jti,
                )) {
                    Err(_) => RunTokenState::Unknown, // fail-closed (deny), never Live on a read error.
                    Ok(None) => RunTokenState::Unknown,
                    Ok(Some(row)) => decide(false, row.expires_at.as_deref()),
                }
            }
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
        // The expiry decision over an entry's `expires_at` — shared so the semantics never drift.
        // `None` TTL → revoked until cleared (a suspended principal). A TTL'd entry is a live
        // revocation only WHILE `now < expires_at`, compared as INSTANTS via `now_strictly_before`
        // (NOT a lexical string compare — that fails open under fractional/offset/boundary forms).
        let revoked_if_present = |expires_at: Option<&str>| -> bool {
            match expires_at {
                None => true,
                // Compared as INSTANTS (not lexically): still revoked iff `now < expires_at`.
                // FAIL-CLOSED: an unparseable timestamp stays REVOKED (deny), never reads not-revoked.
                Some(exp) => now_strictly_before(now.0.as_str(), exp).unwrap_or(true),
            }
        };
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RevocationBackend::Memory(inner) => {
                let key = self.key(scope, kind, handle);
                let guard = inner.lock().unwrap_or_else(|e| e.into_inner());
                match guard.fast.get(&key) {
                    None => false,
                    Some(entry) => {
                        revoked_if_present(entry.expires_at.as_ref().map(|t| t.0.as_str()))
                    }
                }
            }
            RevocationBackend::Pg(pg) => {
                // Read the durable row. On a DB error, fail CLOSED: return `true` (deny) — never
                // report a revoked handle as not-revoked because the consult could not complete (the
                // exact "missed revocation lets a revoked token validate" failure).
                match pg.block(
                    pg.backing
                        .get_revocation(&scope.tenant().0, kind.as_str(), &handle),
                ) {
                    Err(_) => true,
                    Ok(None) => false,
                    Ok(Some(row)) => revoked_if_present(row.expires_at.as_deref()),
                }
            }
        }
    }

    /// **Crash recovery: rebuild the fast Redis/Valkey-class layer from the durable PG mirror.** The
    /// crash-safety property: the fast layer is a *pure function of* the mirror, so losing it (a
    /// Redis/Valkey restart) and rebuilding loses NO revocation — the denylist is byte-identical.
    /// This is the no-op a double-revoke-across-a-crash collapses to (the mirror already has the
    /// entry; recovery re-derives the same fast layer). Idempotent (callable any number of times).
    pub fn recover_from_mirror(&self) {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RevocationBackend::Memory(inner) => {
                let mut guard = inner.lock().unwrap_or_else(|e| e.into_inner());
                guard.fast = guard.mirror.clone();
            }
            // On the Pg path the durable table IS the mirror — reads hit it directly, so there is no
            // fast layer to lose + rebuild. Recovery is a no-op (the durability is the DB's).
            RevocationBackend::Pg(_) => {}
        }
    }

    /// The count of distinct revocations in the verified `(tenant, region)` partition (for the drill
    /// + idempotency assertions — a double-revoke must NOT grow this).
    pub fn revocation_count(&self, scope: &TenantScope) -> usize {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RevocationBackend::Memory(inner) => {
                let (t, r) = (scope.tenant().0.clone(), scope.region().0.clone());
                let guard = inner.lock().unwrap_or_else(|e| e.into_inner());
                guard
                    .mirror
                    .keys()
                    .filter(|(kt, kr, _, _)| *kt == t && *kr == r)
                    .count()
            }
            RevocationBackend::Pg(pg) => pg
                .block(pg.backing.count(&scope.tenant().0))
                .map(|n| n as usize)
                .unwrap_or(0),
        }
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
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RevocationBackend::Memory(inner) => {
                let key = self.key(scope, kind, handle.clone());
                let entry = RevocationEntry {
                    kind,
                    handle,
                    revoked_at: now,
                    expires_at,
                };
                let mut guard = inner.lock().unwrap_or_else(|e| e.into_inner());
                // (1) Durable mirror FIRST (crash-safe). IDEMPOTENT: only insert if absent — an
                //     already-revoked handle keeps its FIRST `revoked_at`.
                guard
                    .mirror
                    .entry(key.clone())
                    .or_insert_with(|| entry.clone());
                // (2) Fast Redis/Valkey-class layer (mirror of the mirror — same idempotent semantics).
                guard.fast.entry(key).or_insert(entry);
            }
            RevocationBackend::Pg(pg) => {
                // The durable INSERT is idempotent (`ON CONFLICT DO NOTHING` preserves the FIRST
                // `revoked_at`). A revoke that cannot durably land must NOT silently succeed (a lost
                // revoke would let a revoked token validate — the F8 failure) → fail LOUD.
                pg.block(pg.backing.insert_revocation(
                    &scope.tenant().0,
                    kind.as_str(),
                    &handle,
                    &now.0,
                    expires_at.as_ref().map(|t| t.0.as_str()),
                ))
                .expect(
                    "durable revocation must persist (fail-closed: never a silent lost revoke)",
                );
            }
        }
        // (3) The deny is effective immediately (a hot consult); record the revocation_lag sample.
        self.telemetry.observe();
    }

    /// Build the `(tenant, region, kind, handle)` mirror key from the verified scope (the partition
    /// prefix is the scope's `(tenant, region)`, never a path — the tenant-predicate floor). Used by
    /// the in-memory test-double arms only (MR-009b Wave 2 — `test-support`-gated).
    #[cfg(any(test, feature = "test-support"))]
    fn key(&self, scope: &TenantScope, kind: RevokedKind, handle: String) -> MirrorKey {
        (
            scope.tenant().0.clone(),
            scope.region().0.clone(),
            kind,
            handle,
        )
    }

    /// Lock the in-memory test-double backing (the Memory arm; the unit tests' direct mirror/fast
    /// inspection uses it). Panics on the Pg backend (the durable table is the source of truth there,
    /// no in-process map) — only the in-memory tests call this.
    #[cfg(test)]
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        match &self.backend {
            RevocationBackend::Memory(inner) => inner.lock().unwrap_or_else(|e| e.into_inner()),
            RevocationBackend::Pg(_) => {
                panic!("lock() is the in-memory test-double accessor; the Pg backend has no map")
            }
        }
    }
}

impl PgRevocationBacking {
    /// Drive an async backing call from the sync store API (the `block_in_place`+`block_on` bridge).
    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
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

    /// **Expiry is compared by INSTANT, not lexically — the fail-open the verifier found is CLOSED.**
    /// Each adversarial pair would read LIVE/not-revoked under a raw `now.0 < exp` string compare, but
    /// must read EXPIRED/denied because `now` is chronologically at-or-past `expires_at`. Plus the
    /// fail-closed-on-malformed posture. Covers the Memory backend (same `now_strictly_before` path as
    /// the Pg backend).
    #[test]
    fn expiry_is_instant_compared_not_lexical_and_fails_closed() {
        let acme = scope("acme");
        let run = RevokeTarget::Jti("run-jti".into());

        // Each case: (expires_at, now) where `now` is chronologically >= expiry → token is EXPIRED,
        // but a lexical string compare would (wrongly) say `now < exp` → LIVE.
        let cases = [
            // (1) differing fractional precision: 0.5s PAST expiry, but '.' < 'Z' lexically.
            ("2026-06-19T00:05:00Z", "2026-06-19T00:05:00.5Z"),
            // (2) non-`Z` offset: exp == 00:05:00Z; now == 00:06:00Z (1 min past). Lexically "00" < "02".
            ("2026-06-19T02:05:00+02:00", "2026-06-19T00:06:00Z"),
            // (3) the exact boundary instant (equal): strict `<` → not live → Expired. '.' < 'Z'.
            ("2026-06-19T00:05:00Z", "2026-06-19T00:05:00.000Z"),
        ];
        for (exp, now) in cases {
            let s7 = RevocationStore::new();
            s7.register_run_token_ttl(&acme, "run-jti", ts("2026-06-19T00:00:00Z"), ts(exp));
            assert!(
                !s7.is_revoked(&acme, &run, &ts(now)),
                "is_revoked: now={now} is at/past expires_at={exp} → token expired (not revoked); a \
                 lexical compare would fail OPEN here"
            );
            assert_eq!(
                s7.run_token_state(&acme, &run, &ts(now)),
                RunTokenState::Expired,
                "run_token_state: now={now} at/past expires_at={exp} → Expired, never Live"
            );
        }

        // Sanity: a clearly-BEFORE instant with a tricky offset still reads LIVE (not over-denied).
        // now == 00:04:00Z is before exp == 00:05:00Z (written as +02:00) → still revoked / Live.
        let s7 = RevocationStore::new();
        s7.register_run_token_ttl(
            &acme,
            "run-jti",
            ts("2026-06-19T00:00:00Z"),
            ts("2026-06-19T02:05:00+02:00"),
        );
        assert!(s7.is_revoked(&acme, &run, &ts("2026-06-19T00:04:00Z")));
        assert_eq!(
            s7.run_token_state(&acme, &run, &ts("2026-06-19T00:04:00Z")),
            RunTokenState::LiveWithinRunLife
        );

        // FAIL-CLOSED on a malformed expires_at: is_revoked stays REVOKED (deny), state reads Expired.
        let s7 = RevocationStore::new();
        s7.register_run_token_ttl(
            &acme,
            "run-jti",
            ts("2026-06-19T00:00:00Z"),
            ts("not-a-timestamp"),
        );
        assert!(
            s7.is_revoked(&acme, &run, &ts("2026-06-19T00:04:00Z")),
            "a malformed expires_at fails CLOSED: the handle stays revoked (deny), never reads not-revoked"
        );
        assert_eq!(
            s7.run_token_state(&acme, &run, &ts("2026-06-19T00:04:00Z")),
            RunTokenState::Expired,
            "a malformed expires_at fails CLOSED in run_token_state (Expired, never Live)"
        );
        // FAIL-CLOSED on a malformed `now` too (deny).
        let s7 = RevocationStore::new();
        s7.register_run_token_ttl(
            &acme,
            "run-jti",
            ts("2026-06-19T00:00:00Z"),
            ts("2026-06-19T00:05:00Z"),
        );
        assert!(
            s7.is_revoked(&acme, &run, &ts("garbage-now")),
            "a malformed `now` fails CLOSED (stays revoked)"
        );
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
            .get(&(
                "acme".into(),
                "eu-west".into(),
                RevokedKind::Jti,
                "jti-1".into(),
            ))
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
        assert!(!s7.is_revoked(
            &acme,
            &RevokeTarget::Jti("p:alice".into()),
            &ts("2026-06-19T00:01:00Z")
        ));
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
        s7.revoke(
            &acme,
            &RevokeTarget::Jti("jti-1".into()),
            ts("2026-06-19T00:00:00Z"),
        );
        s7.disable_principal(
            &acme,
            &PrincipalId("p:bob".into()),
            ts("2026-06-19T00:00:01Z"),
        );
        assert_eq!(
            s7.telemetry().revocation_count(),
            2,
            "each revoke emits one revocation_lag observation"
        );
    }
}
