use myelin_identity::{PrincipalId, PseudonymHandle};
use myelin_storage::{
    KeyClass, KmsEngine, KmsError, PiiKeyRef, TenantQuery, TenantScope, TenantTable,
};
use myelin_tenancy::{Region, TenantId};
#[cfg(any(test, feature = "test-support"))]
use std::collections::HashMap;
use std::sync::Arc;
#[cfg(any(test, feature = "test-support"))]
use std::sync::Mutex;

pub const S2_TABLE: &str = "pseudonym_map";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PseudonymError {
    Kms(String),
    CorruptMapping,
    GrammarMismatch { handle: String },
    Storage(String),
}

impl core::fmt::Display for PseudonymError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PseudonymError::Kms(why) => write!(
                f,
                "pseudonym-map KMS error (the read/write did NOT succeed - never \
                 plaintext-without-key; a crypto-shredded subject resolves to THIS): {why}"
            ),
            PseudonymError::CorruptMapping => write!(
                f,
                "pseudonym real-identity decrypted to a non-conforming shape (a wrong-key/corrupt \
                 open - refused, never silently coerced)"
            ),
            PseudonymError::GrammarMismatch { handle } => write!(
                f,
                "pseudonym handle `{handle}` does not match the frozen \
                 `<pseudonym>@<tenant>.noreply` grammar for the verified tenant (refused)"
            ),
            PseudonymError::Storage(why) => write!(
                f,
                "pseudonym-map durable backing error (the read/write did NOT succeed - never a \
                 silent partial write): {why}"
            ),
        }
    }
}

impl std::error::Error for PseudonymError {}

impl From<KmsError> for PseudonymError {
    fn from(e: KmsError) -> PseudonymError {
        PseudonymError::Kms(e.to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SealedRealIdentity {
    nonce: [u8; myelin_storage::NONCE_LEN],
    ciphertext: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PseudonymRow {
    pub tenant: TenantId,
    pub region: Region,
    pub pseudonym: PseudonymHandle,
    pub real_id_key_ref: PiiKeyRef,
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
struct Inner {
    by_subject: HashMap<(String, String), HashMap<String, PseudonymRow>>,
    by_pseudonym: HashMap<(String, String), HashMap<String, String>>,
    sealed: HashMap<(String, String), HashMap<String, SealedRealIdentity>>,
}

#[derive(Clone)]
pub struct PseudonymStore {
    backend: PseudonymBackend,
    kms: Arc<KmsEngine>,
}

#[derive(Clone)]
enum PseudonymBackend {
    #[cfg(any(test, feature = "test-support"))]
    Memory(Arc<Mutex<Inner>>),
    Pg(PgPseudonymBacking),
}

#[derive(Clone)]
struct PgPseudonymBacking {
    backing: Arc<myelin_storage::DurablePseudonymBacking>,
    rt: tokio::runtime::Handle,
}

impl PgPseudonymBacking {
    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

impl PseudonymStore {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(kms: Arc<KmsEngine>) -> PseudonymStore {
        PseudonymStore {
            backend: PseudonymBackend::Memory(Arc::new(Mutex::new(Inner::default()))),
            kms,
        }
    }

    pub fn with_pg(
        kms: Arc<KmsEngine>,
        backing: myelin_storage::DurablePseudonymBacking,
        rt: tokio::runtime::Handle,
    ) -> PseudonymStore {
        PseudonymStore {
            backend: PseudonymBackend::Pg(PgPseudonymBacking {
                backing: Arc::new(backing),
                rt,
            }),
            kms,
        }
    }

    pub fn subject_dek_class(subject: &PrincipalId) -> KeyClass {
        KeyClass::Subject(subject.0.clone())
    }

    pub fn tenant_dek_class() -> KeyClass {
        KeyClass::Tenant
    }

    pub fn put_mapping(
        &self,
        scope: &TenantScope,
        subject: &PrincipalId,
        pseudonym: PseudonymHandle,
    ) -> Result<PseudonymRow, PseudonymError> {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S2_TABLE));

        if pseudonym.tenant() != scope.tenant().0 {
            return Err(PseudonymError::GrammarMismatch {
                handle: pseudonym.render(),
            });
        }

        let (key_ref, sealed) = self.seal_real_identity(scope, subject)?;

        let row = PseudonymRow {
            tenant: scope.tenant().clone(),
            region: scope.region().clone(),
            pseudonym: pseudonym.clone(),
            real_id_key_ref: key_ref,
        };

        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PseudonymBackend::Memory(inner_arc) => {
                let part_key = Self::part_key(scope);
                let mut inner = inner_arc.lock().unwrap_or_else(|e| e.into_inner());
                inner
                    .by_subject
                    .entry(part_key.clone())
                    .or_default()
                    .insert(subject.0.clone(), row.clone());
                inner
                    .by_pseudonym
                    .entry(part_key.clone())
                    .or_default()
                    .insert(pseudonym.render(), subject.0.clone());
                inner
                    .sealed
                    .entry(part_key)
                    .or_default()
                    .insert(subject.0.clone(), sealed);
            }
            PseudonymBackend::Pg(pg) => {
                let drow = myelin_storage::DurablePseudonymRow {
                    principal_id: subject.0.clone(),
                    pseudonym_render: pseudonym.render(),
                    real_id_key_ref: row.real_id_key_ref.to_uri(),
                    nonce: sealed.nonce.to_vec(),
                    ciphertext: sealed.ciphertext.clone(),
                };
                pg.block(pg.backing.put_mapping(&scope.tenant().0, drow))
                    .map_err(|e| PseudonymError::Storage(e.to_string()))?;
            }
        }
        Ok(row)
    }

    fn durable_to_row(
        scope: &TenantScope,
        drow: &myelin_storage::DurablePseudonymRow,
    ) -> Result<PseudonymRow, PseudonymError> {
        let pseudonym = PseudonymHandle::parse(&drow.pseudonym_render).ok_or_else(|| {
            PseudonymError::Storage(format!(
                "malformed stored pseudonym rendering `{}`",
                drow.pseudonym_render
            ))
        })?;
        let real_id_key_ref = PiiKeyRef::parse(&drow.real_id_key_ref).ok_or_else(|| {
            PseudonymError::Storage(format!(
                "malformed stored key_ref `{}`",
                drow.real_id_key_ref
            ))
        })?;
        Ok(PseudonymRow {
            tenant: scope.tenant().clone(),
            region: scope.region().clone(),
            pseudonym,
            real_id_key_ref,
        })
    }

    fn durable_to_sealed(
        drow: &myelin_storage::DurablePseudonymRow,
    ) -> Result<SealedRealIdentity, PseudonymError> {
        if drow.nonce.len() != myelin_storage::NONCE_LEN {
            return Err(PseudonymError::CorruptMapping);
        }
        let mut nonce = [0u8; myelin_storage::NONCE_LEN];
        nonce.copy_from_slice(&drow.nonce);
        Ok(SealedRealIdentity {
            nonce,
            ciphertext: drow.ciphertext.clone(),
        })
    }

    fn seal_real_identity(
        &self,
        scope: &TenantScope,
        subject: &PrincipalId,
    ) -> Result<(PiiKeyRef, SealedRealIdentity), PseudonymError> {
        let kek_id = myelin_storage::KekId::new(scope.tenant().clone(), scope.region().clone());
        self.kms.ensure_kek(&kek_id)?;
        let key_ref = self.kms.ensure_dek(
            scope.tenant(),
            scope.region(),
            Self::subject_dek_class(subject),
        )?;
        let dek = self.kms.resolve_dek(&key_ref, scope.region())?;
        let (nonce, ciphertext) = dek.seal(subject.0.as_bytes());
        Ok((key_ref, SealedRealIdentity { nonce, ciphertext }))
    }

    pub fn mapping_of(
        &self,
        scope: &TenantScope,
        subject: &PrincipalId,
    ) -> Result<Option<PseudonymRow>, PseudonymError> {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S2_TABLE));
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PseudonymBackend::Memory(inner_arc) => {
                let inner = inner_arc.lock().unwrap_or_else(|e| e.into_inner());
                Ok(inner
                    .by_subject
                    .get(&Self::part_key(scope))
                    .and_then(|p| p.get(&subject.0).cloned()))
            }
            PseudonymBackend::Pg(pg) => {
                let row = pg
                    .block(pg.backing.get_by_principal(&scope.tenant().0, &subject.0))
                    .map_err(|error| PseudonymError::Storage(error.to_string()))?;
                row.map(|durable| Self::durable_to_row(scope, &durable))
                    .transpose()
            }
        }
    }

    pub fn resolve(
        &self,
        scope: &TenantScope,
        pseudonym: &PseudonymHandle,
    ) -> Result<Option<PrincipalId>, PseudonymError> {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S2_TABLE));
        let (key_ref, sealed) = match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PseudonymBackend::Memory(inner_arc) => {
                let part_key = Self::part_key(scope);
                let inner = inner_arc.lock().unwrap_or_else(|e| e.into_inner());
                let subject_id = match inner
                    .by_pseudonym
                    .get(&part_key)
                    .and_then(|m| m.get(&pseudonym.render()))
                {
                    Some(s) => s.clone(),
                    None => return Ok(None),
                };
                let row = match inner
                    .by_subject
                    .get(&part_key)
                    .and_then(|p| p.get(&subject_id))
                {
                    Some(r) => r.clone(),
                    None => return Ok(None),
                };
                let sealed = match inner.sealed.get(&part_key).and_then(|p| p.get(&subject_id)) {
                    Some(s) => s.clone(),
                    None => return Ok(None),
                };
                (row.real_id_key_ref, sealed)
            }
            PseudonymBackend::Pg(pg) => {
                let drow = match pg
                    .block(
                        pg.backing
                            .get_by_pseudonym(&scope.tenant().0, &pseudonym.render()),
                    )
                    .map_err(|e| PseudonymError::Storage(e.to_string()))?
                {
                    Some(d) => d,
                    None => return Ok(None),
                };
                let row = Self::durable_to_row(scope, &drow)?;
                let sealed = Self::durable_to_sealed(&drow)?;
                (row.real_id_key_ref, sealed)
            }
        };
        let dek = self.kms.resolve_dek(&key_ref, scope.region())?;
        let plain = dek
            .open(&sealed.nonce, &sealed.ciphertext)
            .ok_or(PseudonymError::CorruptMapping)?;
        let subject = String::from_utf8(plain).map_err(|_| PseudonymError::CorruptMapping)?;
        Ok(Some(PrincipalId(subject)))
    }

    pub fn resolve_subject(
        &self,
        scope: &TenantScope,
        subject: &PrincipalId,
    ) -> Result<Option<PrincipalId>, PseudonymError> {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S2_TABLE));
        let (key_ref, sealed) = match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PseudonymBackend::Memory(inner_arc) => {
                let part_key = Self::part_key(scope);
                let inner = inner_arc.lock().unwrap_or_else(|e| e.into_inner());
                let Some(row) = inner
                    .by_subject
                    .get(&part_key)
                    .and_then(|p| p.get(&subject.0))
                else {
                    return Ok(None);
                };
                let Some(sealed) = inner.sealed.get(&part_key).and_then(|p| p.get(&subject.0))
                else {
                    return Err(PseudonymError::CorruptMapping);
                };
                (row.real_id_key_ref.clone(), sealed.clone())
            }
            PseudonymBackend::Pg(pg) => {
                let Some(drow) = pg
                    .block(pg.backing.get_by_principal(&scope.tenant().0, &subject.0))
                    .map_err(|e| PseudonymError::Storage(e.to_string()))?
                else {
                    return Ok(None);
                };
                let row = Self::durable_to_row(scope, &drow)?;
                let sealed = Self::durable_to_sealed(&drow)?;
                (row.real_id_key_ref, sealed)
            }
        };
        let dek = match self.kms.resolve_dek(&key_ref, scope.region()) {
            Ok(dek) => dek,
            Err(KmsError::KekUnavailable(_) | KmsError::DekUnavailable(_)) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let plain = dek
            .open(&sealed.nonce, &sealed.ciphertext)
            .ok_or(PseudonymError::CorruptMapping)?;
        let subject = String::from_utf8(plain).map_err(|_| PseudonymError::CorruptMapping)?;
        Ok(Some(PrincipalId(subject)))
    }

    pub fn shred_key_for(
        &self,
        scope: &TenantScope,
        subject: &PrincipalId,
    ) -> Result<Option<PiiKeyRef>, PseudonymError> {
        self.mapping_of(scope, subject)
            .map(|row| row.map(|row| row.real_id_key_ref))
    }

    pub fn shred_row(
        &self,
        scope: &TenantScope,
        subject: &PrincipalId,
    ) -> Result<bool, PseudonymError> {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S2_TABLE));
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PseudonymBackend::Memory(inner_arc) => {
                let part_key = Self::part_key(scope);
                let mut inner = inner_arc.lock().unwrap_or_else(|e| e.into_inner());
                let pseudonym_rendering = inner
                    .by_subject
                    .get(&part_key)
                    .and_then(|p| p.get(&subject.0))
                    .map(|r| r.pseudonym.render());
                let removed = inner
                    .by_subject
                    .get_mut(&part_key)
                    .and_then(|p| p.remove(&subject.0))
                    .is_some();
                inner
                    .sealed
                    .get_mut(&part_key)
                    .and_then(|p| p.remove(&subject.0));
                if let Some(rendering) = pseudonym_rendering {
                    inner
                        .by_pseudonym
                        .get_mut(&part_key)
                        .and_then(|m| m.remove(&rendering));
                }
                Ok(removed)
            }
            PseudonymBackend::Pg(pg) => pg
                .block(pg.backing.shred(&scope.tenant().0, &subject.0))
                .map_err(|error| PseudonymError::Storage(error.to_string())),
        }
    }

    pub fn mappings_in(&self, scope: &TenantScope) -> Result<Vec<PseudonymRow>, PseudonymError> {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S2_TABLE));
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PseudonymBackend::Memory(inner_arc) => {
                let inner = inner_arc.lock().unwrap_or_else(|e| e.into_inner());
                Ok(inner
                    .by_subject
                    .get(&Self::part_key(scope))
                    .map(|p| p.values().cloned().collect())
                    .unwrap_or_default())
            }
            PseudonymBackend::Pg(pg) => pg
                .block(pg.backing.mappings_in(&scope.tenant().0))
                .map_err(|e| PseudonymError::Storage(e.to_string()))?
                .iter()
                .map(|drow| Self::durable_to_row(scope, drow))
                .collect(),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    fn part_key(scope: &TenantScope) -> (String, String) {
        (scope.tenant().0.clone(), scope.region().0.clone())
    }

    #[cfg(test)]
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        match &self.backend {
            PseudonymBackend::Memory(arc) => arc.lock().unwrap_or_else(|e| e.into_inner()),
            PseudonymBackend::Pg(_) => {
                panic!("lock() is the in-memory test-double accessor; the Pg backend has no map")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalKind};

    fn kms() -> Arc<KmsEngine> {
        Arc::new(KmsEngine::new())
    }

    fn scope(tenant: &str) -> TenantScope {
        scope_region(tenant, "eu-west")
    }

    fn scope_region(tenant: &str, region: &str) -> TenantScope {
        let p = Principal::stub(
            PrincipalId("admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        );
        TenantScope::from_verified_token(&p, Region(region.into()))
    }

    fn handle(pseudonym: &str, tenant: &str) -> PseudonymHandle {
        PseudonymHandle::new(pseudonym, tenant).expect("a well-formed handle")
    }

    #[test]
    fn s2_mapping_round_trips_under_rls() {
        let store = PseudonymStore::new(kms());
        let s = scope("acme");
        let h = handle("anon-7f3a", "acme");
        let written = store
            .put_mapping(&s, &PrincipalId("p:alice".into()), h.clone())
            .expect("write");
        assert_eq!(written.pseudonym, h, "the public pseudonym is stored");

        let read = store
            .mapping_of(&s, &PrincipalId("p:alice".into()))
            .expect("mapping directory read succeeds")
            .expect("the row round-trips under the same scope");
        assert_eq!(
            read, written,
            "the S2 row round-trips byte-for-byte under RLS"
        );
        assert_eq!(
            store
                .mappings_in(&s)
                .expect("mapping directory scan succeeds"),
            vec![written]
        );

        let subject = store
            .resolve(&s, &h)
            .expect("resolve succeeds")
            .expect("the pseudonym resolves");
        assert_eq!(
            subject,
            PrincipalId("p:alice".into()),
            "the pseudonym resolves back to the real subject (the real-identity link)"
        );
    }

    #[test]
    fn fallible_resurrection_probe_rejects_corrupt_ciphertext() {
        let store = PseudonymStore::new(kms());
        let s = scope("acme");
        let subject = PrincipalId("p:alice".into());
        store
            .put_mapping(&s, &subject, handle("anon-7f3a", "acme"))
            .expect("write mapping");

        let part = (s.tenant().0.clone(), s.region().0.clone());
        let mut inner = store.lock();
        let sealed = inner
            .sealed
            .get_mut(&part)
            .and_then(|rows| rows.get_mut(&subject.0))
            .expect("sealed mapping");
        sealed.ciphertext[0] ^= 0xff;
        drop(inner);

        assert_eq!(
            store.resolve_subject(&s, &subject),
            Err(PseudonymError::CorruptMapping),
            "corruption must invalidate an erasure proof instead of looking erased"
        );
    }

    #[test]
    fn cross_tenant_read_returns_nothing() {
        let store = PseudonymStore::new(kms());
        let acme = scope("acme");
        let globex = scope("globex");
        let h = handle("anon-7f3a", "acme");
        store
            .put_mapping(&acme, &PrincipalId("p:alice".into()), h.clone())
            .expect("acme write");

        assert!(
            store
                .mapping_of(&globex, &PrincipalId("p:alice".into()))
                .expect("globex's mapping partition remains readable")
                .is_none(),
            "no cross-tenant read path: globex cannot see acme's mapping"
        );
        assert_eq!(
            store.resolve(&globex, &h).expect("resolve"),
            None,
            "globex cannot resolve acme's pseudonym"
        );
        assert!(
            store
                .mappings_in(&globex)
                .expect("globex's mapping partition remains readable")
                .is_empty(),
            "globex's partition is empty"
        );
        assert_eq!(
            store
                .mappings_in(&acme)
                .expect("acme's mapping partition remains readable")
                .len(),
            1
        );
    }

    #[test]
    fn cross_region_read_returns_nothing() {
        let store = PseudonymStore::new(kms());
        let eu = scope_region("acme", "eu-west");
        let us = scope_region("acme", "us-east");
        store
            .put_mapping(
                &eu,
                &PrincipalId("p:alice".into()),
                handle("anon-7f3a", "acme"),
            )
            .expect("eu write");
        assert!(
            store
                .mapping_of(&us, &PrincipalId("p:alice".into()))
                .expect("the us-east mapping partition remains readable")
                .is_none(),
            "residency partition: the us-east partition cannot see the eu-west mapping"
        );
        assert_eq!(
            store
                .mappings_in(&eu)
                .expect("the eu-west mapping partition remains readable")
                .len(),
            1
        );
    }

    #[test]
    fn each_mapping_is_under_a_distinct_per_subject_key() {
        let store = PseudonymStore::new(kms());
        let s = scope("acme");
        store
            .put_mapping(&s, &PrincipalId("p:alice".into()), handle("anon-a", "acme"))
            .unwrap();
        store
            .put_mapping(&s, &PrincipalId("p:bob".into()), handle("anon-b", "acme"))
            .unwrap();

        let alice_ref = store
            .shred_key_for(&s, &PrincipalId("p:alice".into()))
            .expect("alice's key lookup succeeds")
            .expect("alice has a shred key");
        let bob_ref = store
            .shred_key_for(&s, &PrincipalId("p:bob".into()))
            .expect("bob's key lookup succeeds")
            .expect("bob has a shred key");

        assert_eq!(alice_ref.class, KeyClass::Subject("p:alice".into()));
        assert_ne!(
            alice_ref.class,
            PseudonymStore::tenant_dek_class(),
            "the real-identity link is keyed under the PER-SUBJECT DEK, not the per-tenant DEK"
        );
        assert_ne!(
            alice_ref.class, bob_ref.class,
            "distinct subjects get distinct per-subject DEKs"
        );
    }

    #[test]
    fn per_subject_key_boundary_a_does_not_open_b() {
        let store = PseudonymStore::new(kms());
        let s = scope("acme");
        store
            .put_mapping(&s, &PrincipalId("p:alice".into()), handle("anon-a", "acme"))
            .unwrap();
        store
            .put_mapping(&s, &PrincipalId("p:bob".into()), handle("anon-b", "acme"))
            .unwrap();
        let bob_ref = store
            .shred_key_for(&s, &PrincipalId("p:bob".into()))
            .expect("bob's key lookup succeeds")
            .expect("bob has a shred key");

        let inner = store.lock();
        let part = (s.tenant().0.clone(), s.region().0.clone());
        let alice_sealed = inner
            .sealed
            .get(&part)
            .unwrap()
            .get("p:alice")
            .unwrap()
            .clone();
        drop(inner);
        let bob_dek = store.kms.resolve_dek(&bob_ref, s.region()).unwrap();
        assert!(
            bob_dek
                .open(&alice_sealed.nonce, &alice_sealed.ciphertext)
                .is_none(),
            "bob's per-subject DEK must NOT open alice's real-identity link (the GD-4 boundary)"
        );
    }

    #[test]
    fn crypto_shredded_resolve_fails_loud_but_pseudonym_survives() {
        let store = PseudonymStore::new(kms());
        let s = scope("acme");
        let h = handle("anon-7f3a", "acme");
        store
            .put_mapping(&s, &PrincipalId("p:alice".into()), h.clone())
            .unwrap();

        let key_ref = store
            .shred_key_for(&s, &PrincipalId("p:alice".into()))
            .expect("alice's key lookup succeeds")
            .expect("alice has a shred key");
        let dek_id = myelin_storage::DekId::new(key_ref.tenant.clone(), key_ref.class.clone());
        assert!(
            store.kms.destroy_dek(&dek_id).unwrap(),
            "the per-subject DEK is destroyed (crypto-shred)"
        );

        let r = store.resolve(&s, &h);
        assert!(
            matches!(r, Err(PseudonymError::Kms(_))),
            "a crypto-shredded resolve fails loud (KmsError), never plaintext-without-key"
        );
        assert!(
            store
                .mapping_of(&s, &PrincipalId("p:alice".into()))
                .expect("the public mapping directory remains readable")
                .is_some(),
            "the public pseudonym row survives the crypto-shred (historic attribution intact)"
        );
    }

    #[test]
    fn cross_tenant_handle_is_refused() {
        let store = PseudonymStore::new(kms());
        let s = scope("acme");
        let forged = handle("anon-7f3a", "globex");
        let r = store.put_mapping(&s, &PrincipalId("p:alice".into()), forged);
        assert!(
            matches!(r, Err(PseudonymError::GrammarMismatch { .. })),
            "a handle whose tenant label != the verified tenant is refused"
        );
        assert!(
            store
                .mapping_of(&s, &PrincipalId("p:alice".into()))
                .expect("the unchanged mapping directory remains readable")
                .is_none(),
            "nothing was written on rejection (no partial write)"
        );
    }

    #[test]
    fn stored_handle_renders_the_frozen_grammar() {
        let store = PseudonymStore::new(kms());
        let s = scope("acme");
        let h = handle("anon-7f3a", "acme");
        let row = store
            .put_mapping(&s, &PrincipalId("p:alice".into()), h)
            .unwrap();
        assert_eq!(
            row.pseudonym.render(),
            "anon-7f3a@acme.noreply",
            "the stored handle renders the frozen `<pseudonym>@<tenant>.noreply` grammar"
        );
        assert_eq!(
            PseudonymHandle::parse(&row.pseudonym.render()),
            Some(row.pseudonym.clone()),
            "the stored handle round-trips through the frozen grammar"
        );
    }

    #[test]
    fn pseudonym_errors_render_loud_distinct_messages() {
        let kms = PseudonymError::Kms("dek destroyed".into()).to_string();
        let corrupt = PseudonymError::CorruptMapping.to_string();
        let mismatch = PseudonymError::GrammarMismatch {
            handle: "anon@globex.noreply".into(),
        }
        .to_string();
        for (msg, needle) in [
            (&kms, "KMS"),
            (&corrupt, "non-conforming"),
            (&mismatch, "grammar"),
        ] {
            assert!(!msg.is_empty(), "the error renders a non-empty message");
            assert!(
                msg.contains(needle),
                "the error names its cause ({needle}): {msg}"
            );
        }
        assert!(
            kms.contains("dek destroyed"),
            "the KMS error carries the underlying reason"
        );
        assert!(
            mismatch.contains("anon@globex.noreply"),
            "the mismatch names the offending handle"
        );
    }
}
