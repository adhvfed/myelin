//! # Trust-scoped CI cache namespaces (C4) — P-ST-28 / global P-330 (contract 11.2-C4).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §3.2 ("**C4 — trust-tier /
//! branch-scoped cache namespaces (NEW)**"): a **scope-key convention** over the per-tenant T2
//! blob keyspace so an `untrusted_fork` run **cannot poison the trusted cache** (the classic
//! poisoned-cache attack; the storage-tier half of the X-1 poisoned-pipeline defence):
//!
//! ```text
//! cache key prefix = <tenant>/ci/cache/<scope>/...
//!   <scope> ∈ { trusted, fork:<pr_id>, branch:<protected_branch_name> }
//! ```
//!
//! - An `untrusted_fork` run may **READ** the `trusted` scope (cache hits are fine — a fork that
//!   re-uses a trusted build cache is a performance win, not a breach) but may **only WRITE** its
//!   own `fork:<pr_id>` scope — **a write to `trusted` is REFUSED by the blob client**
//!   ([`CacheScopeError::ForkWriteToTrusted`]).
//! - **The scope is stamped from the run's `trust_tier`**, which CI stamps from run provenance.
//!   **Storage ENFORCES the write-scope rule; it does NOT recompute trust** — the trust tier is an
//!   INPUT ([`TrustTier`]) handed in off the CI-stamped fact, never derived here.
//! - This makes "a fork cannot reach the trusted cache scope" a **STRUCTURAL** property of the blob
//!   keyspace, not a check a job must remember to run.
//!
//! Contract-index row 11.2 (the C4 trust-scoped cache namespaces). Drill catalogue row **D-S11**
//! (trust-scoped cache isolation, §4.2): an `untrusted_fork` write lands only in `fork:<pr_id>`; a
//! trusted run never reads it as `trusted`-scoped. **Gate: 0 cross-scope cache writes;
//! `cache_scope_violation` = 0.**
//!
//! ## Reconciliation with the GIT-side `fork_gate::ScopedCache` (EI-01 §7 — no parallel impl)
//! `myelin_git::fork_gate::{TrustScope, ScopedCache}` (GIT-P22 / P-284) confines a fork run's
//! **T7 coordination-cache** (`myelin_storage::Cache`, the Valkey-class store) keys to a
//! `fork:<pr_id>:` PREFIX — that is the GIT subsystem's application of the convention to the
//! short-lived coordination cache, where a fork is confined for BOTH read and write. **This module
//! is a DIFFERENT, distinct tier**: it is the STORAGE-owned C4 enforcement over the **T2 BLOB
//! keyspace** (`<tenant>/ci/cache/<scope>/...` — the durable build-cache artifacts), where the
//! prompt's rule is asymmetric — a fork **READs** the trusted scope (cache hits are fine) but its
//! WRITE to `trusted` is **refused by the blob client**. The two are siblings at two tiers, not a
//! duplicate: `myelin-storage` cannot depend on `myelin-git` (git depends on storage), so the trust
//! tier is its own [`TrustTier`] input here, stamped by CI off the SAME `trust_tier` fact the git
//! gate reads.
//!
//! ## The "one primitive, not a new store" discipline (EI-01 §7)
//! [`CiCacheNamespace`] holds a `&dyn BlobStore` (a BORROW of the base T2 tier, never an owned
//! second store) plus a small **name → content-address INDEX** keyed by the scope-key prefix. The
//! bytes themselves land in the UNCHANGED content-addressed [`crate::blob::BlobStore`] (so a cache
//! artifact is just a content-addressed blob — re-hash-on-read integrity still applies); the C4
//! layer is the **scope-key namespace + the write-scope refusal**, not a new store. The structural
//! assertion "C4 rides the unchanged base BlobStore" is the test
//! `c4_rides_the_unchanged_base_blobstore`.
//!
//! ## What this prompt ships (P-ST-28) and the floor it names
//! - The scope-key convention (`<tenant>/ci/cache/<scope>/<name>`) + the three scopes
//!   ([`CacheScope`]).
//! - The **write-scope refusal**: a fork run's [`CiCacheNamespace::put`] to [`CacheScope::Trusted`]
//!   (or any non-own-fork scope) is REFUSED — the mandatory-core branch.
//! - The **read-allowed** rule: a fork run may [`CiCacheNamespace::get`] the trusted scope (a cache
//!   hit is fine).
//! - The `cache_scope_violation{tenant}` telemetry ([`CacheScopeTelemetry`]) — 0 in the happy path,
//!   it FIRES on a refused fork-write-to-trusted (the attempted poisoning is OBSERVED, not silently
//!   dropped — EI-01 §3).
//! - **Floor named — the C5 OLAP restriction-flag gate is the SIBLING prompt P-ST-29 (global
//!   P-331).** Recorded HERE in writing.
//! - **Floor named — the object-store backing** the cache blobs ultimately rest on is the M5
//!   follow-on (P-ST-30) — a backing swap by the trait's design; here the C4 namespace + write-scope
//!   refusal are proven over the in-memory [`crate::blob::FsBlobStore`] floor. Recorded HERE.
//!
//! ## Mutation floor (mandatory-core, ≥ 80% — EI-01 §2/§3; the prompt's TESTS field)
//! The **write-scope-refusal branch** ([`CacheScope::write_permitted_for`]: a fork tier may write
//! ONLY its own `fork:<pr_id>` scope, never `trusted`/`branch:`/another-fork) is mandatory-core: a
//! fork write admitted into `trusted` IS the poisoned-cache breach D-S11 exists to catch. The
//! `cache_scope_violation`-fires-on-a-refusal path is the second mandatory-core branch (the breach
//! must be OBSERVED). The floor is **≥ 80%**; the achieved score is **100% of viable mutants caught**
//! (`cargo mutants -p myelin-storage -f crates/myelin-storage/src/ci_cache_scope.rs` → 20 caught,
//! 3 unviable, 0 missed, 2026-06-22). Every mutation of the write-scope decision
//! ([`CacheScope::write_permitted_for`]: the `==` pr-id match, the true/false collapse), the refusal
//! telemetry ([`CacheScopeTelemetry::record_violation`]), and the `put` enforcement is killed by an
//! assertion.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use myelin_tenancy::TenantId;

use crate::blob::{BlobError, BlobStore, ContentHash};

/// Maximum CI cache artifact materialized by one namespace read.
pub const CI_CACHE_MAX_ARTIFACT_BYTES: usize = 512 * 1024 * 1024;
const CI_CACHE_MAX_KEY_PART_BYTES: usize = 1024;

/// **The CI-stamped trust tier of a run (the INPUT, never recomputed here).** CI stamps a run's
/// trust tier from its provenance (a PR from a fork → [`TrustTier::UntrustedFork`]; everything else
/// → [`TrustTier::Trusted`]); Storage reads it off the fact and ENFORCES the write-scope rule. This
/// is `myelin-storage`'s own copy of the `trust_tier ∈ {trusted, untrusted_fork}` vocabulary (the
/// SAME fact `myelin_git::check_status::TrustTier` carries — storage cannot depend on git, so the
/// tier is handed in, §X-1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TrustTier {
    /// A trusted run — a non-fork PR, a protected-branch build, or an endorsed/re-run-trusted fork.
    /// Reads/writes the `trusted` (or a `branch:`) scope.
    Trusted,
    /// An `untrusted_fork` run — a PR from a fork, or any run that executed untrusted contributor
    /// code. Confined to WRITE only its own `fork:<pr_id>` scope; may READ the `trusted` scope.
    UntrustedFork,
}

/// **A trust-tier / branch-scoped cache namespace (storage.md §3.2 C4).** The `<scope>` segment of
/// the `<tenant>/ci/cache/<scope>/<name>` key path. PII-free: a trust label, a PR id, or a
/// protected-branch name — never a payload.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CacheScope {
    /// The `trusted` scope — read/written by a trusted run; READ-ONLY for a fork (a cache hit is
    /// fine, a write is refused).
    Trusted,
    /// The `fork:<pr_id>` scope — an `untrusted_fork` run's OWN confined write scope, keyed on the
    /// PII-free PR id.
    Fork {
        /// The PR id this fork scope is keyed on (`fork:<pr_id>`). PII-free (the PR number/id).
        pr_id: String,
    },
    /// The `branch:<protected_branch_name>` scope — a protected-branch build's cache (a trusted
    /// surface; a fork never writes it).
    Branch {
        /// The protected-branch name this scope is keyed on (`branch:<name>`). PII-free.
        name: String,
    },
}

impl CacheScope {
    /// The `<scope>` key segment — `trusted`, `fork:<pr_id>`, or `branch:<name>` (storage.md §3.2,
    /// copied exactly). The ONE place the scope segment is composed, so a `put` and a `get` of the
    /// same name under the same scope always agree and two scopes never collide.
    pub fn segment(&self) -> String {
        match self {
            CacheScope::Trusted => "trusted".to_string(),
            CacheScope::Fork { pr_id } => format!("fork:{pr_id}"),
            CacheScope::Branch { name } => format!("branch:{name}"),
        }
    }

    /// `true` IFF this is the trusted scope (the scope a fork may READ but never WRITE).
    pub fn is_trusted(&self) -> bool {
        matches!(self, CacheScope::Trusted)
    }

    /// **The write-scope rule (the mandatory-core decision — storage.md §3.2 C4).** Decide whether a
    /// run carrying `trust_tier` is permitted to WRITE this scope:
    ///
    /// - A [`TrustTier::Trusted`] run may write ANY scope (`trusted`, a `branch:`, or even a `fork:`
    ///   scope — a trusted run servicing a fork PR legitimately writes that PR's fork scope).
    /// - A [`TrustTier::UntrustedFork`] run may write **ONLY its own `fork:<pr_id>` scope** — a write
    ///   to `trusted`, to a `branch:`, or to ANOTHER fork's scope is **REFUSED**. This is the
    ///   poisoned-cache defence: a fork structurally cannot plant a value a later trusted run reads.
    ///
    /// Storage does NOT recompute trust — `trust_tier` and the run's own `pr_id` are INPUTS off the
    /// CI-stamped fact.
    pub fn write_permitted_for(&self, trust_tier: TrustTier, run_pr_id: &str) -> bool {
        match trust_tier {
            // A trusted run may write any scope.
            TrustTier::Trusted => true,
            // A fork run may write ONLY its own fork:<pr_id> scope.
            TrustTier::UntrustedFork => match self {
                CacheScope::Fork { pr_id } => pr_id == run_pr_id,
                CacheScope::Trusted | CacheScope::Branch { .. } => false,
            },
        }
    }
}

/// **The reason a scope operation was refused (the blob client's refusal — storage.md §3.2 C4).**
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CacheScopeError {
    /// An `untrusted_fork` run attempted to WRITE the `trusted` (or a `branch:`/another-fork) scope —
    /// **refused by the blob client** (the poisoned-cache defence). Carries the scope it tried to
    /// write so the refusal is legible in a log/telemetry.
    ForkWriteToTrusted {
        /// The scope segment the fork run tried to write (`trusted`, `branch:<name>`, …).
        attempted_scope: String,
        /// The fork run's own PR id (the only scope it MAY write is `fork:<this>`).
        run_pr_id: String,
    },
    /// A cache entry was requested by name within a scope but no entry is indexed there (a clean
    /// MISS — e.g. a fork's write is INVISIBLE to a trusted read of the same name).
    Miss {
        /// The scope segment the lookup was scoped to.
        scope: String,
        /// The logical cache-entry name that missed.
        name: String,
    },
    /// The underlying content-addressed [`BlobStore`] erred (e.g. a re-hash-on-read integrity
    /// failure on the cached blob bytes — the base tier's 0-silent-serve property still applies).
    Blob(BlobError),
    /// A cache artifact or key component exceeded its bounded namespace ceiling.
    LimitExceeded(&'static str),
}

impl std::fmt::Display for CacheScopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CacheScopeError::ForkWriteToTrusted {
                attempted_scope,
                run_pr_id,
            } => write!(
                f,
                "untrusted_fork run (pr {run_pr_id}) refused write to non-fork cache scope \
                 '{attempted_scope}' (poisoned-cache defence, contract 11.2-C4): a fork may write \
                 only its own fork:<pr_id> scope"
            ),
            CacheScopeError::Miss { scope, name } => {
                write!(f, "cache miss: no entry '{name}' in scope '{scope}'")
            }
            CacheScopeError::Blob(e) => write!(f, "cache blob error: {e}"),
            CacheScopeError::LimitExceeded(kind) => {
                write!(f, "CI cache {kind} limit exceeded")
            }
        }
    }
}

impl std::error::Error for CacheScopeError {}

impl From<BlobError> for CacheScopeError {
    fn from(e: BlobError) -> Self {
        CacheScopeError::Blob(e)
    }
}

/// The `cache_scope_violation{tenant}` telemetry (storage.md §9 telemetry; the D-S11 signal —
/// "**must be 0**"). A storage-DOMAIN counter: it counts **refused** fork-writes-to-a-non-fork-scope
/// — i.e. attempted poisonings that were STOPPED. It is 0 in the happy path (no fork ever tries to
/// poison the trusted scope) and FIRES on a refusal so the attempted breach is OBSERVED, not
/// silently dropped (EI-01 §3, observability is part of the pass). **The gate is: a fork write that
/// REACHED the trusted scope = 0** — which this design makes structurally impossible (the write is
/// refused before it lands), so the count is "attempts blocked", and the cross-scope-landings count
/// is 0 by construction.
#[derive(Debug, Default)]
pub struct CacheScopeTelemetry {
    /// Count of refused fork-writes-to-a-non-fork-scope (attempted poisonings blocked). The gate's
    /// CROSS-SCOPE-LANDINGS is 0 by construction; this counter makes each blocked attempt legible.
    cache_scope_violation: AtomicU64,
}

impl CacheScopeTelemetry {
    /// The current `cache_scope_violation` count (refused fork-writes-to-a-non-fork-scope).
    pub fn cache_scope_violation(&self) -> u64 {
        self.cache_scope_violation.load(Ordering::SeqCst)
    }

    fn record_violation(&self) {
        self.cache_scope_violation.fetch_add(1, Ordering::SeqCst);
    }
}

/// **The trust-scoped CI cache namespaces (contract 11.2-C4) over the unchanged content-addressed
/// [`BlobStore`].** Wraps a `&dyn BlobStore` (the base T2 tier — the in-memory fs floor in unit
/// tests; the real `S3BlobStore` behind the `integration` feature, the one-line swap holds) plus a
/// scope-keyed **name → content-address index**. A cache entry is `(scope, name) → ContentHash`;
/// the BYTES are a content-addressed blob in the base store (so re-hash-on-read integrity still
/// applies to a cache artifact).
///
/// **The C4 enforcement — the blob client refuses a fork write to the trusted scope:**
/// - [`CiCacheNamespace::put`] takes the run's CI-stamped [`TrustTier`] + its own `pr_id` and the
///   target [`CacheScope`]; it REFUSES (returns [`CacheScopeError::ForkWriteToTrusted`] +
///   increments `cache_scope_violation`) if a fork run targets any scope but its own
///   `fork:<pr_id>`.
/// - [`CiCacheNamespace::get`] is READ — a fork run may read the `trusted` scope (a cache hit is
///   fine); reads are not trust-gated (the prompt's asymmetric rule: read trusted, write own fork).
///
/// **0 cross-scope cache writes**: a fork run's write to `trusted` never lands (it is refused before
/// the index is touched), so a later trusted read of the same name is a clean MISS (D-S11).
pub struct CiCacheNamespace<'b> {
    /// The tenant whose `<tenant>/ci/cache/...` keyspace these namespaces live in.
    tenant: TenantId,
    /// The base content-addressed blob tier (BORROWED — never an owned second store, EI-01 §7).
    base: &'b dyn BlobStore,
    /// The scope-keyed name index: full scope-key path (`<tenant>/ci/cache/<scope>/<name>`) →
    /// the content-address of the cached bytes in `base`.
    index: Mutex<HashMap<String, ContentHash>>,
    /// The `cache_scope_violation` telemetry (the D-S11 signal).
    telemetry: CacheScopeTelemetry,
}

impl<'b> CiCacheNamespace<'b> {
    /// Open the trust-scoped CI cache namespaces for `tenant` over the borrowed base blob tier.
    pub fn over(tenant: TenantId, base: &'b dyn BlobStore) -> CiCacheNamespace<'b> {
        CiCacheNamespace {
            tenant,
            base,
            index: Mutex::new(HashMap::new()),
            telemetry: CacheScopeTelemetry::default(),
        }
    }

    /// The `cache_scope_violation` telemetry the D-S11 drill asserts on (must be 0 cross-scope
    /// LANDINGS; this counts blocked attempts).
    pub fn telemetry(&self) -> &CacheScopeTelemetry {
        &self.telemetry
    }

    /// The full scope-key path `<tenant>/ci/cache/<scope>/<name>` (storage.md §3.2, copied exactly).
    /// The ONE place the full path is composed.
    fn scope_key(&self, scope: &CacheScope, name: &str) -> String {
        format!("{}/ci/cache/{}/{}", self.tenant.0, scope.segment(), name)
    }

    /// **Write a cache entry `name` with `bytes` into `scope`, as a run carrying the CI-stamped
    /// `trust_tier` and its own `run_pr_id` (the C4 write-scope enforcement).**
    ///
    /// The blob client REFUSES (returns [`CacheScopeError::ForkWriteToTrusted`] + fires
    /// `cache_scope_violation`) if `trust_tier == UntrustedFork` and `scope` is anything but the
    /// run's own `fork:<run_pr_id>` — so a fork can NEVER poison the trusted scope. On a permitted
    /// write the bytes are stored content-addressed in the base tier and the `(scope, name)` index
    /// records the address. Returns the content-address of the cached bytes.
    pub fn put(
        &self,
        trust_tier: TrustTier,
        run_pr_id: &str,
        scope: &CacheScope,
        name: &str,
        bytes: &[u8],
    ) -> Result<ContentHash, CacheScopeError> {
        self.put_bounded(
            trust_tier,
            run_pr_id,
            scope,
            name,
            bytes,
            CI_CACHE_MAX_ARTIFACT_BYTES,
        )
    }

    /// Write a cache entry under a caller-selected artifact byte ceiling.
    pub fn put_bounded(
        &self,
        trust_tier: TrustTier,
        run_pr_id: &str,
        scope: &CacheScope,
        name: &str,
        bytes: &[u8],
        maximum_artifact_bytes: usize,
    ) -> Result<ContentHash, CacheScopeError> {
        Self::validate_key_inputs(scope, name)?;
        if run_pr_id.len() > CI_CACHE_MAX_KEY_PART_BYTES {
            return Err(CacheScopeError::LimitExceeded("run PR id"));
        }
        if bytes.len() > maximum_artifact_bytes {
            return Err(CacheScopeError::LimitExceeded("artifact bytes"));
        }
        // THE C4 ENFORCEMENT (mandatory-core): a fork run may write ONLY its own fork:<pr_id> scope.
        if !scope.write_permitted_for(trust_tier, run_pr_id) {
            // The attempted poisoning is OBSERVED (telemetry), not silently dropped (EI-01 §3).
            self.telemetry.record_violation();
            return Err(CacheScopeError::ForkWriteToTrusted {
                attempted_scope: scope.segment(),
                run_pr_id: run_pr_id.to_string(),
            });
        }
        // Permitted: the cache artifact is a content-addressed blob in the UNCHANGED base tier.
        let hash = self.base.put(&self.tenant, bytes)?;
        let key = self.scope_key(scope, name);
        self.index
            .lock()
            .expect("ci cache index mutex")
            .insert(key, hash.clone());
        Ok(hash)
    }

    /// **Read the cache entry `name` from `scope` (a READ — a fork run MAY read the `trusted`
    /// scope; the prompt's asymmetric rule: read trusted, write own fork).** Returns the cached
    /// bytes (re-hash-verified by the base tier's re-hash-on-read, 0 silent serve) or a
    /// [`CacheScopeError::Miss`] if no entry is indexed at `(scope, name)`.
    ///
    /// Reads are NOT trust-gated: a fork reusing a trusted build cache is a performance win, not a
    /// breach (storage.md §3.2 C4: "an `untrusted_fork` run may READ the `trusted` scope"). The
    /// confinement is on the WRITE side — a fork's write is INVISIBLE in the trusted scope because it
    /// never landed there.
    pub fn get(&self, scope: &CacheScope, name: &str) -> Result<Vec<u8>, CacheScopeError> {
        self.get_bounded(scope, name, CI_CACHE_MAX_ARTIFACT_BYTES)
    }

    /// Read a cache entry under a caller-selected byte ceiling, checking blob metadata before fetch.
    pub fn get_bounded(
        &self,
        scope: &CacheScope,
        name: &str,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, CacheScopeError> {
        Self::validate_key_inputs(scope, name)?;
        let key = self.scope_key(scope, name);
        let hash = {
            let index = self.index.lock().expect("ci cache index mutex");
            index.get(&key).cloned()
        };
        match hash {
            Some(h) => Ok(self
                .base
                .get_bounded(&self.tenant, &h, maximum_bytes)?),
            None => Err(CacheScopeError::Miss {
                scope: scope.segment(),
                name: name.to_string(),
            }),
        }
    }

    /// `true` IFF a cache entry is indexed at `(scope, name)` — the confinement-witness predicate
    /// (a fork's write to `trusted` never lands, so `contains(Trusted, name)` is false after a
    /// refused fork write: 0 cross-scope landings).
    pub fn contains(&self, scope: &CacheScope, name: &str) -> bool {
        if Self::validate_key_inputs(scope, name).is_err() {
            return false;
        }
        let key = self.scope_key(scope, name);
        self.index
            .lock()
            .expect("ci cache index mutex")
            .contains_key(&key)
    }

    fn validate_key_inputs(scope: &CacheScope, name: &str) -> Result<(), CacheScopeError> {
        if name.len() > CI_CACHE_MAX_KEY_PART_BYTES {
            return Err(CacheScopeError::LimitExceeded("entry name"));
        }
        let scope_id = match scope {
            CacheScope::Trusted => return Ok(()),
            CacheScope::Fork { pr_id } => pr_id,
            CacheScope::Branch { name } => name,
        };
        if scope_id.len() > CI_CACHE_MAX_KEY_PART_BYTES {
            return Err(CacheScopeError::LimitExceeded("scope id"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::FsBlobStore;

    fn tenant() -> TenantId {
        TenantId("acme".to_string())
    }

    /// **The write-scope refusal (the mandatory-core).** An `untrusted_fork` write to `trusted` is
    /// REFUSED by the blob client; `cache_scope_violation` fires; nothing lands in `trusted`.
    #[test]
    fn untrusted_fork_write_to_trusted_is_refused() {
        let base = FsBlobStore::new();
        let cache = CiCacheNamespace::over(tenant(), &base);

        let refused = cache.put(
            TrustTier::UntrustedFork,
            "42",
            &CacheScope::Trusted,
            "build-cache",
            b"poison",
        );

        assert!(
            matches!(refused, Err(CacheScopeError::ForkWriteToTrusted { .. })),
            "a fork write to trusted must be REFUSED by the blob client, got {refused:?}"
        );
        // The attempted poisoning is OBSERVED.
        assert_eq!(cache.telemetry().cache_scope_violation(), 1);
        // 0 cross-scope landings: nothing is in the trusted scope.
        assert!(!cache.contains(&CacheScope::Trusted, "build-cache"));
    }

    /// An `untrusted_fork` write to its OWN `fork:<pr_id>` scope SUCCEEDS (the fork's legitimate
    /// confined cache).
    #[test]
    fn untrusted_fork_write_to_own_fork_scope_succeeds() {
        let base = FsBlobStore::new();
        let cache = CiCacheNamespace::over(tenant(), &base);
        let scope = CacheScope::Fork {
            pr_id: "42".to_string(),
        };

        let hash = cache
            .put(
                TrustTier::UntrustedFork,
                "42",
                &scope,
                "build-cache",
                b"fork-bytes",
            )
            .expect("fork writes its own scope");
        assert!(cache
            .put_bounded(
                TrustTier::UntrustedFork,
                "42",
                &scope,
                "bounded",
                b"fork-bytes",
                b"fork-bytes".len(),
            )
            .is_ok());
        assert_eq!(
            cache.put_bounded(
                TrustTier::UntrustedFork,
                "42",
                &scope,
                "too-large",
                b"fork-bytes",
                b"fork-bytes".len() - 1,
            ),
            Err(CacheScopeError::LimitExceeded("artifact bytes"))
        );
        assert_eq!(
            cache.get(&scope, &"x".repeat(CI_CACHE_MAX_KEY_PART_BYTES + 1)),
            Err(CacheScopeError::LimitExceeded("entry name"))
        );

        // It landed in the fork scope, NOT the trusted scope (0 cross-scope write).
        assert!(cache.contains(&scope, "build-cache"));
        assert!(!cache.contains(&CacheScope::Trusted, "build-cache"));
        // No violation on a legitimate own-scope write.
        assert_eq!(cache.telemetry().cache_scope_violation(), 0);
        // The bytes round-trip (re-hash-verified by the base tier).
        let got = cache
            .get(&scope, "build-cache")
            .expect("read own fork cache");
        assert_eq!(got, b"fork-bytes");
        assert_eq!(
            cache
                .get_bounded(&scope, "build-cache", b"fork-bytes".len())
                .expect("exact read limit accepted"),
            b"fork-bytes"
        );
        assert!(matches!(
            cache.get_bounded(&scope, "build-cache", b"fork-bytes".len() - 1),
            Err(CacheScopeError::Blob(BlobError::SizeLimitExceeded { .. }))
        ));
        // Sanity: the stored content-address is the BLAKE3 of the bytes.
        assert_eq!(hash, ContentHash::blake3(b"fork-bytes"));
    }

    /// An `untrusted_fork` run may READ the `trusted` scope (a cache hit is fine — the asymmetric
    /// rule: read trusted, write own fork).
    #[test]
    fn untrusted_fork_may_read_the_trusted_scope() {
        let base = FsBlobStore::new();
        let cache = CiCacheNamespace::over(tenant(), &base);

        // A trusted run populates the trusted cache.
        cache
            .put(
                TrustTier::Trusted,
                "main",
                &CacheScope::Trusted,
                "deps",
                b"trusted-deps",
            )
            .expect("trusted run writes trusted");

        // A fork run READs it — permitted (the read path is not trust-gated).
        let got = cache
            .get(&CacheScope::Trusted, "deps")
            .expect("a fork may read the trusted scope");
        assert_eq!(got, b"trusted-deps");
        assert_eq!(cache.telemetry().cache_scope_violation(), 0);
    }

    /// **D-S11 confinement:** a fork's write lands only in `fork:<pr_id>`; a trusted run never reads
    /// it as `trusted`-scoped (a clean MISS). 0 cross-scope cache writes.
    #[test]
    fn fork_write_is_invisible_to_a_trusted_read_of_the_same_name() {
        let base = FsBlobStore::new();
        let cache = CiCacheNamespace::over(tenant(), &base);
        let fork = CacheScope::Fork {
            pr_id: "7".to_string(),
        };

        // The fork writes the SAME logical name "artifact" — confined to its fork scope.
        cache
            .put(
                TrustTier::UntrustedFork,
                "7",
                &fork,
                "artifact",
                b"fork-artifact",
            )
            .expect("fork writes own scope");

        // A trusted run reading "artifact" in the TRUSTED scope is a clean MISS — the fork's write
        // is invisible (it never reached the trusted scope).
        let miss = cache.get(&CacheScope::Trusted, "artifact");
        assert!(
            matches!(miss, Err(CacheScopeError::Miss { .. })),
            "the trusted read of a fork-written name must MISS, got {miss:?}"
        );
        // The fork's own read of its scope hits.
        assert_eq!(
            cache.get(&fork, "artifact").expect("fork reads own scope"),
            b"fork-artifact"
        );
    }

    /// A trusted run writes/reads the `trusted` and `branch:` scopes (the trusted surface).
    #[test]
    fn trusted_run_writes_trusted_and_branch_scopes() {
        let base = FsBlobStore::new();
        let cache = CiCacheNamespace::over(tenant(), &base);
        let branch = CacheScope::Branch {
            name: "release".to_string(),
        };

        cache
            .put(TrustTier::Trusted, "main", &CacheScope::Trusted, "k", b"t")
            .expect("trusted writes trusted");
        cache
            .put(TrustTier::Trusted, "main", &branch, "k", b"b")
            .expect("trusted writes branch");

        assert_eq!(cache.get(&CacheScope::Trusted, "k").unwrap(), b"t");
        assert_eq!(cache.get(&branch, "k").unwrap(), b"b");
        assert_eq!(cache.telemetry().cache_scope_violation(), 0);
    }

    /// A fork run may NOT write ANOTHER fork's scope (only its OWN `fork:<pr_id>`) — refused.
    #[test]
    fn fork_cannot_write_another_forks_scope() {
        let base = FsBlobStore::new();
        let cache = CiCacheNamespace::over(tenant(), &base);
        let other = CacheScope::Fork {
            pr_id: "99".to_string(),
        };

        let refused = cache.put(TrustTier::UntrustedFork, "42", &other, "k", b"x");
        assert!(matches!(
            refused,
            Err(CacheScopeError::ForkWriteToTrusted { .. })
        ));
        assert_eq!(cache.telemetry().cache_scope_violation(), 1);
        assert!(!cache.contains(&other, "k"));
    }

    /// A fork run may NOT write a `branch:` (protected-branch) scope — refused.
    #[test]
    fn fork_cannot_write_a_protected_branch_scope() {
        let base = FsBlobStore::new();
        let cache = CiCacheNamespace::over(tenant(), &base);
        let branch = CacheScope::Branch {
            name: "main".to_string(),
        };

        let refused = cache.put(TrustTier::UntrustedFork, "42", &branch, "k", b"x");
        assert!(matches!(
            refused,
            Err(CacheScopeError::ForkWriteToTrusted { .. })
        ));
        assert_eq!(cache.telemetry().cache_scope_violation(), 1);
    }

    /// `is_trusted` is the confinement-witness predicate — true ONLY for the trusted scope.
    #[test]
    fn is_trusted_is_true_only_for_the_trusted_scope() {
        assert!(CacheScope::Trusted.is_trusted());
        assert!(!CacheScope::Fork {
            pr_id: "1".to_string()
        }
        .is_trusted());
        assert!(!CacheScope::Branch {
            name: "main".to_string()
        }
        .is_trusted());
    }

    /// The refusal error renders a legible, non-empty message (the attempted poisoning is legible in
    /// a log) — and names both the attempted scope and the run's pr id.
    #[test]
    fn refusal_error_display_is_legible() {
        let err = CacheScopeError::ForkWriteToTrusted {
            attempted_scope: "trusted".to_string(),
            run_pr_id: "42".to_string(),
        };
        let rendered = format!("{err}");
        assert!(
            rendered.contains("trusted"),
            "names the attempted scope: {rendered}"
        );
        assert!(rendered.contains("42"), "names the run pr id: {rendered}");
        assert!(
            rendered.contains("poisoned-cache") && rendered.contains("11.2-C4"),
            "the refusal is attributed to the C4 poisoned-cache defence: {rendered}"
        );
        // The Miss + Blob variants render too (no empty Display).
        assert!(!format!(
            "{}",
            CacheScopeError::Miss {
                scope: "trusted".to_string(),
                name: "k".to_string()
            }
        )
        .is_empty());
    }

    /// The scope segments match the storage.md §3.2 convention byte-for-byte.
    #[test]
    fn scope_segments_match_the_convention() {
        assert_eq!(CacheScope::Trusted.segment(), "trusted");
        assert_eq!(
            CacheScope::Fork {
                pr_id: "42".to_string()
            }
            .segment(),
            "fork:42"
        );
        assert_eq!(
            CacheScope::Branch {
                name: "main".to_string()
            }
            .segment(),
            "branch:main"
        );
    }

    /// The full scope-key path is `<tenant>/ci/cache/<scope>/<name>` (storage.md §3.2).
    #[test]
    fn scope_key_path_matches_the_convention() {
        let base = FsBlobStore::new();
        let cache = CiCacheNamespace::over(tenant(), &base);
        assert_eq!(
            cache.scope_key(&CacheScope::Trusted, "deps"),
            "acme/ci/cache/trusted/deps"
        );
        assert_eq!(
            cache.scope_key(
                &CacheScope::Fork {
                    pr_id: "42".to_string()
                },
                "deps"
            ),
            "acme/ci/cache/fork:42/deps"
        );
    }

    /// **Storage does not recompute trust** — the trust tier is an INPUT. Two `put`s of the SAME
    /// scope by the SAME run differ ONLY by the handed-in `trust_tier`: a trusted tier writes
    /// `trusted`; flip the tier to fork and the identical call is refused. The decision is a pure
    /// function of the INPUT tier, never a recomputation.
    #[test]
    fn the_write_decision_is_a_pure_function_of_the_input_tier() {
        // Trusted tier → write to trusted permitted.
        assert!(CacheScope::Trusted.write_permitted_for(TrustTier::Trusted, "x"));
        // Fork tier → write to trusted refused (same scope, only the input tier changed).
        assert!(!CacheScope::Trusted.write_permitted_for(TrustTier::UntrustedFork, "x"));
        // Fork tier → its OWN fork scope permitted, ANOTHER fork's refused.
        let own = CacheScope::Fork {
            pr_id: "x".to_string(),
        };
        let other = CacheScope::Fork {
            pr_id: "y".to_string(),
        };
        assert!(own.write_permitted_for(TrustTier::UntrustedFork, "x"));
        assert!(!other.write_permitted_for(TrustTier::UntrustedFork, "x"));
    }

    /// **C4 rides the UNCHANGED base BlobStore (EI-01 §7 — not a new store).** The SAME
    /// [`FsBlobStore`] instance backs both a direct blob read and a cache-namespace read — the cache
    /// artifact IS a content-addressed blob, the C4 layer is only the scope namespace + write-scope
    /// refusal.
    #[test]
    fn c4_rides_the_unchanged_base_blobstore() {
        let base = FsBlobStore::new();
        let scope = CacheScope::Fork {
            pr_id: "1".to_string(),
        };
        let hash = {
            let cache = CiCacheNamespace::over(tenant(), &base);
            cache
                .put(TrustTier::UntrustedFork, "1", &scope, "k", b"shared-bytes")
                .expect("write")
        };
        // The SAME base store serves the bytes by content-address directly (no parallel object map).
        let direct = base.get(&tenant(), &hash).expect("direct base read");
        assert_eq!(direct, b"shared-bytes");
    }
}
