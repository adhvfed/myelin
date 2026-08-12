use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use myelin_tenancy::TenantId;

use crate::blob::{BlobError, BlobStore, ContentHash};

pub const CI_CACHE_MAX_ARTIFACT_BYTES: usize = 512 * 1024 * 1024;
const CI_CACHE_MAX_KEY_PART_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TrustTier {
    Trusted,
    UntrustedFork,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CacheScope {
    Trusted,
    Fork { pr_id: String },
    Branch { name: String },
}

impl CacheScope {
    pub fn segment(&self) -> String {
        match self {
            CacheScope::Trusted => "trusted".to_string(),
            CacheScope::Fork { pr_id } => format!("fork:{pr_id}"),
            CacheScope::Branch { name } => format!("branch:{name}"),
        }
    }

    pub fn is_trusted(&self) -> bool {
        matches!(self, CacheScope::Trusted)
    }

    pub fn write_permitted_for(&self, trust_tier: TrustTier, run_pr_id: &str) -> bool {
        match trust_tier {
            TrustTier::Trusted => true,
            TrustTier::UntrustedFork => match self {
                CacheScope::Fork { pr_id } => pr_id == run_pr_id,
                CacheScope::Trusted | CacheScope::Branch { .. } => false,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CacheScopeError {
    ForkWriteToTrusted {
        attempted_scope: String,
        run_pr_id: String,
    },
    Miss {
        scope: String,
        name: String,
    },
    Blob(BlobError),
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

#[derive(Debug, Default)]
pub struct CacheScopeTelemetry {
    cache_scope_violation: AtomicU64,
}

impl CacheScopeTelemetry {
    pub fn cache_scope_violation(&self) -> u64 {
        self.cache_scope_violation.load(Ordering::SeqCst)
    }

    fn record_violation(&self) {
        self.cache_scope_violation.fetch_add(1, Ordering::SeqCst);
    }
}

pub struct CiCacheNamespace<'b> {
    tenant: TenantId,
    base: &'b dyn BlobStore,
    index: Mutex<HashMap<String, ContentHash>>,
    telemetry: CacheScopeTelemetry,
}

impl<'b> CiCacheNamespace<'b> {
    pub fn over(tenant: TenantId, base: &'b dyn BlobStore) -> CiCacheNamespace<'b> {
        CiCacheNamespace {
            tenant,
            base,
            index: Mutex::new(HashMap::new()),
            telemetry: CacheScopeTelemetry::default(),
        }
    }

    pub fn telemetry(&self) -> &CacheScopeTelemetry {
        &self.telemetry
    }

    fn scope_key(&self, scope: &CacheScope, name: &str) -> String {
        format!("{}/ci/cache/{}/{}", self.tenant.0, scope.segment(), name)
    }

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
        if !scope.write_permitted_for(trust_tier, run_pr_id) {
            self.telemetry.record_violation();
            return Err(CacheScopeError::ForkWriteToTrusted {
                attempted_scope: scope.segment(),
                run_pr_id: run_pr_id.to_string(),
            });
        }
        let hash = self.base.put(&self.tenant, bytes)?;
        let key = self.scope_key(scope, name);
        self.index
            .lock()
            .expect("ci cache index mutex")
            .insert(key, hash.clone());
        Ok(hash)
    }

    pub fn get(&self, scope: &CacheScope, name: &str) -> Result<Vec<u8>, CacheScopeError> {
        self.get_bounded(scope, name, CI_CACHE_MAX_ARTIFACT_BYTES)
    }

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
            Some(h) => Ok(self.base.get_bounded(&self.tenant, &h, maximum_bytes)?),
            None => Err(CacheScopeError::Miss {
                scope: scope.segment(),
                name: name.to_string(),
            }),
        }
    }

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
        assert_eq!(cache.telemetry().cache_scope_violation(), 1);
        assert!(!cache.contains(&CacheScope::Trusted, "build-cache"));
    }

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

        assert!(cache.contains(&scope, "build-cache"));
        assert!(!cache.contains(&CacheScope::Trusted, "build-cache"));
        assert_eq!(cache.telemetry().cache_scope_violation(), 0);
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
        assert_eq!(hash, ContentHash::blake3(b"fork-bytes"));
    }

    #[test]
    fn untrusted_fork_may_read_the_trusted_scope() {
        let base = FsBlobStore::new();
        let cache = CiCacheNamespace::over(tenant(), &base);

        cache
            .put(
                TrustTier::Trusted,
                "main",
                &CacheScope::Trusted,
                "deps",
                b"trusted-deps",
            )
            .expect("trusted run writes trusted");

        let got = cache
            .get(&CacheScope::Trusted, "deps")
            .expect("a fork may read the trusted scope");
        assert_eq!(got, b"trusted-deps");
        assert_eq!(cache.telemetry().cache_scope_violation(), 0);
    }

    #[test]
    fn fork_write_is_invisible_to_a_trusted_read_of_the_same_name() {
        let base = FsBlobStore::new();
        let cache = CiCacheNamespace::over(tenant(), &base);
        let fork = CacheScope::Fork {
            pr_id: "7".to_string(),
        };

        cache
            .put(
                TrustTier::UntrustedFork,
                "7",
                &fork,
                "artifact",
                b"fork-artifact",
            )
            .expect("fork writes own scope");

        let miss = cache.get(&CacheScope::Trusted, "artifact");
        assert!(
            matches!(miss, Err(CacheScopeError::Miss { .. })),
            "the trusted read of a fork-written name must MISS, got {miss:?}"
        );
        assert_eq!(
            cache.get(&fork, "artifact").expect("fork reads own scope"),
            b"fork-artifact"
        );
    }

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
        assert!(!format!(
            "{}",
            CacheScopeError::Miss {
                scope: "trusted".to_string(),
                name: "k".to_string()
            }
        )
        .is_empty());
    }

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

    #[test]
    fn the_write_decision_is_a_pure_function_of_the_input_tier() {
        assert!(CacheScope::Trusted.write_permitted_for(TrustTier::Trusted, "x"));
        assert!(!CacheScope::Trusted.write_permitted_for(TrustTier::UntrustedFork, "x"));
        let own = CacheScope::Fork {
            pr_id: "x".to_string(),
        };
        let other = CacheScope::Fork {
            pr_id: "y".to_string(),
        };
        assert!(own.write_permitted_for(TrustTier::UntrustedFork, "x"));
        assert!(!other.write_permitted_for(TrustTier::UntrustedFork, "x"));
    }

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
        let direct = base.get(&tenant(), &hash).expect("direct base read");
        assert_eq!(direct, b"shared-bytes");
    }
}
