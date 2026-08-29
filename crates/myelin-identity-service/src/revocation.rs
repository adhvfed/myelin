use myelin_events::Timestamp;
use myelin_identity::{PrincipalId, RevokeTarget};
use myelin_storage::TenantScope;
#[cfg(any(test, feature = "test-support"))]
use std::collections::BTreeMap;
use std::sync::Arc;
#[cfg(any(test, feature = "test-support"))]
use std::sync::Mutex;

fn now_strictly_before(now: &str, expires_at: &str) -> Option<bool> {
    match (
        chrono::DateTime::parse_from_rfc3339(now),
        chrono::DateTime::parse_from_rfc3339(expires_at),
    ) {
        (Ok(n), Ok(e)) => Some(n < e),
        _ => None,
    }
}

pub const S7_TABLE: &str = "revocation";

pub const REVOCATION_SLA_SECS: u64 = 5 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RevokedKind {
    Jti,
    Principal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunTokenState {
    LiveWithinRunLife,
    Expired,
    TornDown,
    Unknown,
}

impl RevokedKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RevokedKind::Jti => "jti",
            RevokedKind::Principal => "principal",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevocationEntry {
    pub kind: RevokedKind,
    pub handle: String,
    pub revoked_at: Timestamp,
    pub expires_at: Option<Timestamp>,
}

#[cfg(any(test, feature = "test-support"))]
type MirrorKey = (String, String, RevokedKind, String);

#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
struct Inner {
    mirror: BTreeMap<MirrorKey, RevocationEntry>,
    fast: BTreeMap<MirrorKey, RevocationEntry>,
    run_teardowns: std::collections::BTreeSet<(String, String, String)>,
}

#[derive(Clone)]
pub struct RevocationStore {
    backend: RevocationBackend,
}

#[derive(Clone)]
enum RevocationBackend {
    #[cfg(any(test, feature = "test-support"))]
    Memory(Arc<Mutex<Inner>>),
    Pg(PgRevocationBacking),
}

#[derive(Clone)]
struct PgRevocationBacking {
    backing: Arc<myelin_storage::DurableRevocationBacking>,
    rt: tokio::runtime::Handle,
}

#[cfg(any(test, feature = "test-support"))]
impl Default for RevocationStore {
    fn default() -> RevocationStore {
        RevocationStore::new()
    }
}

impl RevocationStore {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new() -> RevocationStore {
        RevocationStore {
            backend: RevocationBackend::Memory(Arc::new(Mutex::new(Inner::default()))),
        }
    }

    pub fn with_pg(
        backing: myelin_storage::DurableRevocationBacking,
        rt: tokio::runtime::Handle,
    ) -> RevocationStore {
        RevocationStore {
            backend: RevocationBackend::Pg(PgRevocationBacking {
                backing: Arc::new(backing),
                rt,
            }),
        }
    }

    pub fn revoke(
        &self,
        scope: &TenantScope,
        target: &RevokeTarget,
        now: Timestamp,
    ) -> Result<(), myelin_storage::ProviderError> {
        let (kind, handle) = match target {
            RevokeTarget::Jti(jti) => (RevokedKind::Jti, jti.clone()),
            RevokeTarget::Principal(pid) => (RevokedKind::Principal, pid.0.clone()),
        };
        self.insert(scope, kind, handle, now, None)
    }

    pub fn disable_principal(
        &self,
        scope: &TenantScope,
        principal: &PrincipalId,
        now: Timestamp,
    ) -> Result<(), myelin_storage::ProviderError> {
        self.insert(
            scope,
            RevokedKind::Principal,
            principal.0.clone(),
            now,
            None,
        )
    }

    pub fn register_run_token_ttl(
        &self,
        scope: &TenantScope,
        jti: &str,
        now: Timestamp,
        expires_at: Timestamp,
    ) -> Result<(), myelin_storage::ProviderError> {
        self.insert(
            scope,
            RevokedKind::Jti,
            jti.to_string(),
            now,
            Some(expires_at),
        )
    }

    pub fn tear_down_run_token(
        &self,
        scope: &TenantScope,
        jti: &str,
    ) -> Result<(), myelin_storage::ProviderError> {
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
                pg.block(pg.backing.insert_teardown(&scope.tenant().0, jti))?;
            }
        }
        Ok(())
    }

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
        let decide = |torn_down: bool, expires_at: Option<&str>| -> RunTokenState {
            if torn_down {
                return RunTokenState::TornDown;
            }
            match expires_at {
                None => RunTokenState::TornDown,
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
                    None => RunTokenState::Unknown,
                    Some(entry) => decide(false, entry.expires_at.as_ref().map(|t| t.0.as_str())),
                }
            }
            RevocationBackend::Pg(pg) => {
                let torn_down = pg
                    .block(pg.backing.is_teardown(&scope.tenant().0, &jti))
                    .unwrap_or(true);
                if torn_down {
                    return RunTokenState::TornDown;
                }
                match pg.block(pg.backing.get_revocation(
                    &scope.tenant().0,
                    RevokedKind::Jti.as_str(),
                    &jti,
                )) {
                    Err(_) => RunTokenState::Unknown,
                    Ok(None) => RunTokenState::Unknown,
                    Ok(Some(row)) => decide(false, row.expires_at.as_deref()),
                }
            }
        }
    }

    pub fn is_revoked(&self, scope: &TenantScope, target: &RevokeTarget, now: &Timestamp) -> bool {
        let (kind, handle) = match target {
            RevokeTarget::Jti(jti) => (RevokedKind::Jti, jti.clone()),
            RevokeTarget::Principal(pid) => (RevokedKind::Principal, pid.0.clone()),
        };
        let revoked_if_present = |expires_at: Option<&str>| -> bool {
            match expires_at {
                None => true,
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

    pub fn recover_from_mirror(&self) {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RevocationBackend::Memory(inner) => {
                let mut guard = inner.lock().unwrap_or_else(|e| e.into_inner());
                guard.fast = guard.mirror.clone();
            }
            RevocationBackend::Pg(_) => {}
        }
    }

    pub fn revocation_count(
        &self,
        scope: &TenantScope,
    ) -> Result<usize, myelin_storage::ProviderError> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RevocationBackend::Memory(inner) => {
                let (t, r) = (scope.tenant().0.clone(), scope.region().0.clone());
                let guard = inner.lock().unwrap_or_else(|e| e.into_inner());
                Ok(guard
                    .mirror
                    .keys()
                    .filter(|(kt, kr, _, _)| *kt == t && *kr == r)
                    .count())
            }
            RevocationBackend::Pg(pg) => pg
                .block(pg.backing.count(&scope.tenant().0))
                .map(|n| n as usize),
        }
    }

    fn insert(
        &self,
        scope: &TenantScope,
        kind: RevokedKind,
        handle: String,
        now: Timestamp,
        expires_at: Option<Timestamp>,
    ) -> Result<(), myelin_storage::ProviderError> {
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
                guard
                    .mirror
                    .entry(key.clone())
                    .or_insert_with(|| entry.clone());
                guard.fast.entry(key).or_insert(entry);
            }
            RevocationBackend::Pg(pg) => {
                pg.block(pg.backing.insert_revocation(
                    &scope.tenant().0,
                    kind.as_str(),
                    &handle,
                    &now.0,
                    expires_at.as_ref().map(|t| t.0.as_str()),
                ))?;
            }
        }
        Ok(())
    }

    #[cfg(any(test, feature = "test-support"))]
    fn key(&self, scope: &TenantScope, kind: RevokedKind, handle: String) -> MirrorKey {
        (
            scope.tenant().0.clone(),
            scope.region().0.clone(),
            kind,
            handle,
        )
    }

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
    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
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

    #[test]
    fn expiry_is_instant_compared_not_lexical_and_fails_closed() {
        let acme = scope("acme");
        let run = RevokeTarget::Jti("run-jti".into());

        let cases = [
            ("2026-06-19T00:05:00Z", "2026-06-19T00:05:00.5Z"),
            ("2026-06-19T02:05:00+02:00", "2026-06-19T00:06:00Z"),
            ("2026-06-19T00:05:00Z", "2026-06-19T00:05:00.000Z"),
        ];
        for (exp, now) in cases {
            let s7 = RevocationStore::new();
            s7.register_run_token_ttl(&acme, "run-jti", ts("2026-06-19T00:00:00Z"), ts(exp))
                .expect("in-memory run lifetime is recorded");
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

        let s7 = RevocationStore::new();
        s7.register_run_token_ttl(
            &acme,
            "run-jti",
            ts("2026-06-19T00:00:00Z"),
            ts("2026-06-19T02:05:00+02:00"),
        )
        .expect("in-memory run lifetime is recorded");
        assert!(s7.is_revoked(&acme, &run, &ts("2026-06-19T00:04:00Z")));
        assert_eq!(
            s7.run_token_state(&acme, &run, &ts("2026-06-19T00:04:00Z")),
            RunTokenState::LiveWithinRunLife
        );

        let s7 = RevocationStore::new();
        s7.register_run_token_ttl(
            &acme,
            "run-jti",
            ts("2026-06-19T00:00:00Z"),
            ts("not-a-timestamp"),
        )
        .expect("in-memory run lifetime is recorded");
        assert!(
            s7.is_revoked(&acme, &run, &ts("2026-06-19T00:04:00Z")),
            "a malformed expires_at fails CLOSED: the handle stays revoked (deny), never reads not-revoked"
        );
        assert_eq!(
            s7.run_token_state(&acme, &run, &ts("2026-06-19T00:04:00Z")),
            RunTokenState::Expired,
            "a malformed expires_at fails CLOSED in run_token_state (Expired, never Live)"
        );
        let s7 = RevocationStore::new();
        s7.register_run_token_ttl(
            &acme,
            "run-jti",
            ts("2026-06-19T00:00:00Z"),
            ts("2026-06-19T00:05:00Z"),
        )
        .expect("in-memory run lifetime is recorded");
        assert!(
            s7.is_revoked(&acme, &run, &ts("garbage-now")),
            "a malformed `now` fails CLOSED (stays revoked)"
        );
    }

    #[test]
    fn revoked_jti_is_denylisted() {
        let s7 = RevocationStore::new();
        let acme = scope("acme");
        let jti = RevokeTarget::Jti("jti-1".into());
        assert!(!s7.is_revoked(&acme, &jti, &ts("2026-06-19T00:00:00Z")));
        s7.revoke(&acme, &jti, ts("2026-06-19T00:00:00Z"))
            .expect("in-memory revocation is recorded");
        assert!(
            s7.is_revoked(&acme, &jti, &ts("2026-06-19T00:00:01Z")),
            "a revoked jti is on the denylist (deny-on-denylisted)"
        );
    }

    #[test]
    fn revoke_is_idempotent() {
        let s7 = RevocationStore::new();
        let acme = scope("acme");
        let jti = RevokeTarget::Jti("jti-1".into());
        s7.revoke(&acme, &jti, ts("2026-06-19T00:00:00Z"))
            .expect("first revocation is recorded");
        s7.revoke(&acme, &jti, ts("2026-06-19T09:00:00Z"))
            .expect("idempotent revocation is recorded");
        assert_eq!(
            s7.revocation_count(&acme).expect("count revocations"),
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

    #[test]
    fn revoke_is_crash_safe() {
        let s7 = RevocationStore::new();
        let acme = scope("acme");
        let jti = RevokeTarget::Jti("jti-1".into());
        s7.revoke(&acme, &jti, ts("2026-06-19T00:00:00Z"))
            .expect("in-memory revocation is recorded");

        {
            let mut guard = s7.lock();
            guard.fast.clear();
            assert!(
                !guard.mirror.is_empty(),
                "the durable mirror survives the crash"
            );
        }
        assert!(
            !s7.is_revoked(&acme, &jti, &ts("2026-06-19T00:00:01Z")),
            "the fast layer is empty immediately after the crash"
        );
        s7.recover_from_mirror();
        assert!(
            s7.is_revoked(&acme, &jti, &ts("2026-06-19T00:00:01Z")),
            "recovery rebuilds the denylist from the durable mirror (no revoke lost)"
        );
        s7.revoke(&acme, &jti, ts("2026-06-19T09:00:00Z"))
            .expect("idempotent revocation is recorded");
        assert_eq!(s7.revocation_count(&acme).expect("count revocations"), 1);
    }

    #[test]
    fn per_run_token_auto_expires() {
        let s7 = RevocationStore::new();
        let acme = scope("acme");
        s7.register_run_token_ttl(
            &acme,
            "run-jti",
            ts("2026-06-19T00:00:00Z"),
            ts("2026-06-19T00:05:00Z"),
        )
        .expect("in-memory run lifetime is recorded");
        let jti = RevokeTarget::Jti("run-jti".into());
        assert!(s7.is_revoked(&acme, &jti, &ts("2026-06-19T00:02:00Z")));
        assert!(!s7.is_revoked(&acme, &jti, &ts("2026-06-19T00:06:00Z")));
    }

    #[test]
    fn scim_disable_is_principal_scoped_and_tenant_partitioned() {
        let s7 = RevocationStore::new();
        let acme = scope("acme");
        let evil = scope("evil-corp");
        let pid = PrincipalId("p:alice".into());
        s7.disable_principal(&acme, &pid, ts("2026-06-19T00:00:00Z"))
            .expect("in-memory principal disablement is recorded");

        let target = RevokeTarget::Principal(pid.clone());
        assert!(
            s7.is_revoked(&acme, &target, &ts("2026-06-19T00:01:00Z")),
            "acme's alice is revoked across surfaces"
        );
        assert!(
            !s7.is_revoked(&evil, &target, &ts("2026-06-19T00:01:00Z")),
            "evil-corp's identical id is NOT revoked (no cross-tenant denylist path)"
        );
        assert!(!s7.is_revoked(
            &acme,
            &RevokeTarget::Jti("p:alice".into()),
            &ts("2026-06-19T00:01:00Z")
        ));
    }
}
