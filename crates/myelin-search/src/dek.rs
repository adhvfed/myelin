use std::sync::Arc;

use myelin_storage::{
    DekId, IndexAdmission, KekId, KeyClass, KeyOrigin, KmsEngine, KmsError, PiiKeyRef,
};
use myelin_tenancy::{Region, TenantId};

#[derive(Clone)]
pub struct SearchDekPin {
    kms: Arc<KmsEngine>,
}

impl SearchDekPin {
    pub fn new(kms: Arc<KmsEngine>) -> SearchDekPin {
        SearchDekPin { kms }
    }

    pub fn tenant_index_dek_class() -> KeyClass {
        KeyClass::Tenant
    }

    pub fn subject_source_dek_class(subject_id: &str) -> KeyClass {
        KeyClass::Subject(subject_id.to_string())
    }

    pub fn reserve(&self, tenant: &TenantId, region: &Region) -> Result<PiiKeyRef, KmsError> {
        self.kms
            .ensure_kek(&KekId::new(tenant.clone(), region.clone()))?;
        self.kms
            .ensure_dek(tenant, region, Self::tenant_index_dek_class())
    }

    pub fn reserve_subject_source_backstop(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject_id: &str,
    ) -> Result<PiiKeyRef, KmsError> {
        self.kms
            .ensure_kek(&KekId::new(tenant.clone(), region.clone()))?;
        self.kms
            .ensure_dek(tenant, region, Self::subject_source_dek_class(subject_id))
    }

    pub fn destroy_tenant_index_dek(
        &self,
        tenant: &TenantId,
        region: &Region,
    ) -> Result<bool, KmsError> {
        self.kms
            .destroy_kek(&KekId::new(tenant.clone(), region.clone()))
    }

    pub fn destroy_subject_backstop(
        &self,
        tenant: &TenantId,
        subject_id: &str,
    ) -> Result<bool, KmsError> {
        self.kms.destroy_dek(&DekId::new(
            tenant.clone(),
            Self::subject_source_dek_class(subject_id),
        ))
    }

    pub fn resolve(
        &self,
        key_ref: &PiiKeyRef,
        region: &Region,
    ) -> Result<myelin_storage::DekHandle, KmsError> {
        self.kms.resolve_dek(key_ref, region)
    }

    pub fn engine(&self) -> &Arc<KmsEngine> {
        &self.kms
    }
}

impl std::fmt::Debug for SearchDekPin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchDekPin")
            .field("kms", &self.kms)
            .finish()
    }
}

pub fn hyok_skips_index(origin: &dyn KeyOrigin) -> bool {
    matches!(IndexAdmission::for_origin(origin), IndexAdmission::SkipHyok)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InheritedGate {
    pub id: &'static str,
    pub guarantees: &'static str,
}

pub fn srch_p03_inherited_gates() -> Vec<InheritedGate> {
    vec![
        InheritedGate {
            id: "STOR-D1",
            guarantees: "restore-verify: a restored copy is byte-faithful + the cross-seam \
                         consistency point holds (the permanent store gate; Search cannot build the \
                         index over an unrestorable source-of-truth store - it reindexes from it)",
        },
        InheritedGate {
            id: "STOR-D2",
            guarantees: "cell-kill RTO restore-verify: a cell can be rebuilt within RTO from \
                         backups (the permanent store gate; re-run on every store-touching change)",
        },
        InheritedGate {
            id: "ID-D3",
            guarantees: "cross-tenant authz = 0: no check ever leaks across tenants (Search's \
                         permission-aware query leans on this; a cross-tenant index read is \
                         impossible - SRCH-D3)",
        },
        InheritedGate {
            id: "ID-D2",
            guarantees: "fail-static authz: a KMS/Identity hiccup degrades to bounded-staleness, \
                         never fail-open (Search's DEK resolve + ACL filter inherit this posture)",
        },
        InheritedGate {
            id: "ID-D1",
            guarantees: "disabled-user revocation within N≥5 min: a revoked principal stops \
                         surfacing in results (Search's zookie/consistency path inherits the \
                         revocation SLA - TTL ≤ revocation SLA)",
        },
        InheritedGate {
            id: "CP-D2",
            guarantees: "misroute rejection: a request to the wrong cell is rejected, never served \
                         (the Search index is cell-local; a cross-cell index read is impossible)",
        },
        InheritedGate {
            id: "CP-D3",
            guarantees: "residency-pin: no cross-region read path on personal data (the per-tenant \
                         index directory is residency-pinned; the per-tenant index DEK is \
                         region-scoped via the KEK - §3.4)",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_storage::{
        Byok, Dek, DekHandle, Hyok, HyokKeyService, HyokServiceDenied, PlatformManaged, WrappedDek,
    };

    fn kms() -> Arc<KmsEngine> {
        Arc::new(KmsEngine::new())
    }
    fn t() -> TenantId {
        TenantId("acme".into())
    }
    fn r() -> Region {
        Region("fr-par".into())
    }

    #[test]
    fn per_tenant_index_dek_is_reserved_and_resolvable() {
        let pin = SearchDekPin::new(kms());
        let key_ref = pin
            .reserve(&t(), &r())
            .expect("reserve the per-tenant Search index DEK");
        assert_eq!(
            key_ref.class,
            KeyClass::Tenant,
            "the Search index class is per-tenant"
        );
        assert_eq!(
            key_ref.to_uri(),
            "kms://acme/0/tenant",
            "the encrypted-from-birth key ref"
        );

        let dek = pin
            .resolve(&key_ref, &r())
            .expect("resolve the reserved per-tenant index DEK");
        let (nonce, ct) = dek.seal(b"a future index segment's encrypted body");
        assert_eq!(
            dek.open(&nonce, &ct).as_deref(),
            Some(&b"a future index segment's encrypted body"[..])
        );
    }

    #[test]
    fn reserve_is_idempotent() {
        let pin = SearchDekPin::new(kms());
        let a = pin.reserve(&t(), &r()).expect("first reserve");
        let b = pin.reserve(&t(), &r()).expect("second reserve");
        assert_eq!(
            a, b,
            "re-reserving returns the same per-tenant index DEK ref (no silent rotation)"
        );
    }

    #[test]
    fn per_subject_source_backstop_is_distinct_from_the_tenant_index_dek() {
        let pin = SearchDekPin::new(kms());
        let tk = pin.reserve(&t(), &r()).expect("tenant index dek");
        let sk = pin
            .reserve_subject_source_backstop(&t(), &r(), "u-1")
            .expect("subject source backstop");
        assert_ne!(
            tk, sk,
            "the per-subject source backstop is a distinct key ref"
        );
        assert_eq!(sk.class, KeyClass::Subject("u-1".into()));

        let tdek = pin.resolve(&tk, &r()).expect("resolve tenant index dek");
        let sdek = pin.resolve(&sk, &r()).expect("resolve subject source dek");
        let (nonce, ct) = sdek.seal(b"a CI log segment naming the subject");
        assert!(
            tdek.open(&nonce, &ct).is_none(),
            "the tenant index DEK must not open a subject-backstop ciphertext (GD-4 subject grain)"
        );
    }

    #[test]
    fn destroy_tenant_index_dek_is_callable_and_renders_the_key_unrecoverable() {
        let pin = SearchDekPin::new(kms());
        let key_ref = pin.reserve(&t(), &r()).expect("reserve");
        assert!(
            pin.resolve(&key_ref, &r()).is_ok(),
            "resolvable before the shred"
        );

        assert!(
            pin.destroy_tenant_index_dek(&t(), &r()).unwrap(),
            "destroy is callable + a key was present"
        );
        assert!(
            !pin.destroy_tenant_index_dek(&t(), &r()).unwrap(),
            "a second destroy reports nothing left"
        );

        assert!(
            matches!(
                pin.resolve(&key_ref, &r()),
                Err(KmsError::KekUnavailable(_))
            ),
            "a crypto-shredded per-tenant index DEK resolves to a LOUD error, never a plaintext"
        );
    }

    #[test]
    fn tenant_decommission_shreds_every_subject_backstop() {
        let pin = SearchDekPin::new(kms());
        let tk = pin.reserve(&t(), &r()).expect("tenant index dek");
        let s1 = pin
            .reserve_subject_source_backstop(&t(), &r(), "u-1")
            .expect("s1");
        let s2 = pin
            .reserve_subject_source_backstop(&t(), &r(), "u-2")
            .expect("s2");

        assert!(
            pin.destroy_tenant_index_dek(&t(), &r()).unwrap(),
            "tenant KEK destroyed"
        );

        for kr in [&tk, &s1, &s2] {
            assert!(
                pin.resolve(kr, &r()).is_err(),
                "every Search DEK under the destroyed tenant KEK is unrecoverable"
            );
        }
    }

    #[test]
    fn destroy_subject_backstop_is_individual_grained() {
        let pin = SearchDekPin::new(kms());
        let tk = pin.reserve(&t(), &r()).expect("tenant index dek");
        let s1 = pin
            .reserve_subject_source_backstop(&t(), &r(), "u-1")
            .expect("s1");
        let s2 = pin
            .reserve_subject_source_backstop(&t(), &r(), "u-2")
            .expect("s2");

        assert!(
            pin.destroy_subject_backstop(&t(), "u-1").unwrap(),
            "subject backstop present to destroy"
        );
        assert!(
            !pin.destroy_subject_backstop(&t(), "u-1").unwrap(),
            "a second destroy finds nothing"
        );

        assert!(
            pin.resolve(&s1, &r()).is_err(),
            "u-1's source backstop key is shredded"
        );
        assert!(
            pin.resolve(&tk, &r()).is_ok(),
            "the tenant index DEK is untouched"
        );
        assert!(
            pin.resolve(&s2, &r()).is_ok(),
            "u-2's backstop is untouched"
        );
    }

    #[test]
    fn shredded_search_tenant_is_excluded_from_backup() {
        let kms = kms();
        let pin = SearchDekPin::new(Arc::clone(&kms));
        let live = TenantId("live-co".into());
        let dead = TenantId("offboarded-co".into());
        pin.reserve(&live, &r()).expect("live");
        pin.reserve(&dead, &r()).expect("dead");

        assert!(
            pin.destroy_tenant_index_dek(&dead, &r()).unwrap(),
            "offboard the dead tenant"
        );

        let snap = kms.backup_snapshot().unwrap();
        assert!(
            snap.iter().any(|(d, _)| d.tenant == live),
            "live tenant index DEK backed up"
        );
        assert!(
            !snap.iter().any(|(d, _)| d.tenant == dead),
            "a crypto-shredded Search tenant is EXCLUDED from backup (stays dead across restore)"
        );
    }

    #[test]
    fn search_uses_the_one_cell_engine_not_a_second_kms() {
        let kms = kms();
        let pin = SearchDekPin::new(Arc::clone(&kms));
        let key_ref = pin.reserve(&t(), &r()).expect("reserve through the pin");
        assert!(
            kms.resolve_dek(&key_ref, &r()).is_ok(),
            "the shared cell engine resolves the DEK the Search pin reserved (one hierarchy)"
        );
        assert!(
            Arc::ptr_eq(pin.engine(), &kms),
            "the pin holds the very same cell engine"
        );
    }

    #[test]
    fn hyok_class_is_structurally_skipped_no_index_no_dek() {
        struct DenyAllHyok;
        impl HyokKeyService for DenyAllHyok {
            fn wrap(&self, _dek: &Dek) -> Result<WrappedDek, HyokServiceDenied> {
                Err(HyokServiceDenied)
            }
            fn unwrap(&self, _w: &WrappedDek) -> Result<DekHandle, HyokServiceDenied> {
                Err(HyokServiceDenied)
            }
            fn destroy(&self) {}
        }

        let engine = KmsEngine::new();
        engine
            .ensure_kek(&KekId::new(t(), r()))
            .expect("seed the in-memory KEK");
        let platform = PlatformManaged::new(&engine, r());
        let byok = Byok::new(&engine, r(), "kms-customer://acme/k1");
        let hyok = Hyok::new(DenyAllHyok);

        assert!(
            !hyok_skips_index(&platform),
            "platform-managed class IS indexed (full search)"
        );
        assert!(
            !hyok_skips_index(&byok),
            "BYOK class IS indexed (plaintext reachable while live)"
        );
        assert!(
            hyok_skips_index(&hyok),
            "a HYOK class is structurally SKIPPED - no plaintext index"
        );
    }

    #[test]
    fn srch_p03_inherited_gates_name_every_precondition() {
        let gates = srch_p03_inherited_gates();
        let ids: Vec<&str> = gates.iter().map(|g| g.id).collect();
        for required in [
            "STOR-D1", "STOR-D2", "ID-D3", "ID-D2", "ID-D1", "CP-D2", "CP-D3",
        ] {
            assert!(
                ids.contains(&required),
                "the SRCH-P03 precondition list names {required}"
            );
        }
        for g in &gates {
            assert!(
                !g.guarantees.is_empty(),
                "gate {} states what it guarantees",
                g.id
            );
        }
    }
}
