use std::sync::Arc;

use myelin_storage::{DekId, KekId, KeyClass, KmsEngine, KmsError, PiiKeyRef};
use myelin_tenancy::{Region, TenantId};

#[derive(Clone)]
pub struct RefsDekPin {
    kms: Arc<KmsEngine>,
}

impl RefsDekPin {
    pub fn new(kms: Arc<KmsEngine>) -> RefsDekPin {
        RefsDekPin { kms }
    }

    pub fn tenant_dek_class() -> KeyClass {
        KeyClass::Tenant
    }

    pub fn subject_dek_class(subject_id: &str) -> KeyClass {
        KeyClass::Subject(subject_id.to_string())
    }

    pub fn reserve(&self, tenant: &TenantId, region: &Region) -> Result<PiiKeyRef, KmsError> {
        self.kms
            .ensure_kek(&KekId::new(tenant.clone(), region.clone()))?;
        self.kms
            .ensure_dek(tenant, region, Self::tenant_dek_class())
    }

    pub fn reserve_subject_backstop(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject_id: &str,
    ) -> Result<PiiKeyRef, KmsError> {
        self.kms
            .ensure_kek(&KekId::new(tenant.clone(), region.clone()))?;
        self.kms
            .ensure_dek(tenant, region, Self::subject_dek_class(subject_id))
    }

    pub fn destroy_tenant_dek(&self, tenant: &TenantId, region: &Region) -> Result<bool, KmsError> {
        self.kms
            .destroy_kek(&KekId::new(tenant.clone(), region.clone()))
    }

    pub fn subject_backstop_is_live(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject_id: &str,
    ) -> bool {
        let key_ref = PiiKeyRef::new(tenant.clone(), 0, Self::subject_dek_class(subject_id));
        self.kms.resolve_dek(&key_ref, region).is_ok()
    }

    pub fn destroy_subject_backstop(
        &self,
        tenant: &TenantId,
        subject_id: &str,
    ) -> Result<bool, KmsError> {
        self.kms.destroy_dek(&DekId::new(
            tenant.clone(),
            Self::subject_dek_class(subject_id),
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

impl std::fmt::Debug for RefsDekPin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RefsDekPin")
            .field("kms", &self.kms)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InheritedGate {
    pub id: &'static str,
    pub guarantees: &'static str,
}

pub fn ref_p5_inherited_gates() -> Vec<InheritedGate> {
    vec![
        InheritedGate {
            id: "STOR-D1",
            guarantees: "restore-verify: a restored copy is byte-faithful + the cross-seam \
                         consistency point holds (the permanent store gate; the edge index cannot \
                         be built over an unrestorable store)",
        },
        InheritedGate {
            id: "STOR-D2",
            guarantees: "cell-kill RTO restore-verify: a cell can be rebuilt within RTO from \
                         backups (the permanent store gate; re-run on every store-touching change)",
        },
        InheritedGate {
            id: "ID-D3",
            guarantees: "cross-tenant authz = 0: no check ever leaks across tenants (Refs' \
                         per-viewer resolution leans on this; a cross-tenant edge read is impossible)",
        },
        InheritedGate {
            id: "ID-D2",
            guarantees: "fail-static authz: a KMS/Identity hiccup degrades to bounded-staleness, \
                         never fail-open (Refs' DEK resolve + ACL filter inherit this posture)",
        },
        InheritedGate {
            id: "ID-D1",
            guarantees: "disabled-user revocation within N≥5 min: a revoked principal stops \
                         resolving edges (Refs' per-viewer chokepoint inherits the revocation SLA)",
        },
        InheritedGate {
            id: "CP-D2",
            guarantees: "misroute rejection: a request to the wrong cell is rejected, never served \
                         (Refs state is cell-local; a cross-cell edge read is impossible)",
        },
        InheritedGate {
            id: "CP-D3",
            guarantees: "residency-pin: no cross-region read path (the Refs edge table + R2 cache \
                         are residency-pinned; the per-tenant DEK is region-scoped via the KEK)",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn per_tenant_dek_is_reserved_and_resolvable() {
        let pin = RefsDekPin::new(kms());
        let key_ref = pin
            .reserve(&t(), &r())
            .expect("reserve the per-tenant Refs DEK");
        assert_eq!(
            key_ref.class,
            KeyClass::Tenant,
            "the Refs bulk class is per-tenant"
        );
        assert_eq!(
            key_ref.to_uri(),
            "kms://acme/0/tenant",
            "the encrypted-from-birth key ref"
        );

        let dek = pin
            .resolve(&key_ref, &r())
            .expect("resolve the reserved per-tenant DEK");
        let (nonce, ct) = dek.seal(b"a future edge row's bulk column");
        assert_eq!(
            dek.open(&nonce, &ct).as_deref(),
            Some(&b"a future edge row's bulk column"[..])
        );
    }

    #[test]
    fn reserve_is_idempotent() {
        let pin = RefsDekPin::new(kms());
        let a = pin.reserve(&t(), &r()).expect("first reserve");
        let b = pin.reserve(&t(), &r()).expect("second reserve");
        assert_eq!(
            a, b,
            "re-reserving returns the same per-tenant DEK ref (no silent rotation)"
        );
    }

    #[test]
    fn per_subject_backstop_is_distinct_from_the_tenant_dek() {
        let pin = RefsDekPin::new(kms());
        let tk = pin.reserve(&t(), &r()).expect("tenant dek");
        let sk = pin
            .reserve_subject_backstop(&t(), &r(), "u-1")
            .expect("subject backstop");
        assert_ne!(tk, sk, "the per-subject backstop is a distinct key ref");
        assert_eq!(sk.class, KeyClass::Subject("u-1".into()));

        let tdek = pin.resolve(&tk, &r()).expect("resolve tenant");
        let sdek = pin.resolve(&sk, &r()).expect("resolve subject");
        let (nonce, ct) = sdek.seal(b"a name in a cached title");
        assert!(
            tdek.open(&nonce, &ct).is_none(),
            "the tenant DEK must not open a subject-backstop ciphertext (GD-4 subject grain)"
        );
    }

    #[test]
    fn destroy_tenant_dek_is_callable_and_renders_the_key_unrecoverable() {
        let pin = RefsDekPin::new(kms());
        let key_ref = pin.reserve(&t(), &r()).expect("reserve");
        assert!(
            pin.resolve(&key_ref, &r()).is_ok(),
            "resolvable before the shred"
        );

        assert!(
            pin.destroy_tenant_dek(&t(), &r()).unwrap(),
            "destroy is callable + a key was present"
        );
        assert!(
            !pin.destroy_tenant_dek(&t(), &r()).unwrap(),
            "a second destroy reports nothing left"
        );

        assert!(
            matches!(
                pin.resolve(&key_ref, &r()),
                Err(KmsError::KekUnavailable(_))
            ),
            "a crypto-shredded per-tenant DEK resolves to a LOUD error, never a plaintext"
        );
    }

    #[test]
    fn tenant_decommission_shreds_every_subject_backstop() {
        let pin = RefsDekPin::new(kms());
        let tk = pin.reserve(&t(), &r()).expect("tenant dek");
        let s1 = pin.reserve_subject_backstop(&t(), &r(), "u-1").expect("s1");
        let s2 = pin.reserve_subject_backstop(&t(), &r(), "u-2").expect("s2");

        assert!(
            pin.destroy_tenant_dek(&t(), &r()).unwrap(),
            "tenant KEK destroyed"
        );

        for kr in [&tk, &s1, &s2] {
            assert!(
                pin.resolve(kr, &r()).is_err(),
                "every Refs DEK under the destroyed tenant KEK is unrecoverable"
            );
        }
    }

    #[test]
    fn destroy_subject_backstop_is_individual_grained() {
        let pin = RefsDekPin::new(kms());
        let tk = pin.reserve(&t(), &r()).expect("tenant");
        let s1 = pin.reserve_subject_backstop(&t(), &r(), "u-1").expect("s1");
        let s2 = pin.reserve_subject_backstop(&t(), &r(), "u-2").expect("s2");

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
            "u-1's cached-title key is shredded"
        );
        assert!(
            pin.resolve(&tk, &r()).is_ok(),
            "the tenant DEK is untouched"
        );
        assert!(
            pin.resolve(&s2, &r()).is_ok(),
            "u-2's backstop is untouched"
        );
    }

    #[test]
    fn shredded_refs_tenant_is_excluded_from_backup() {
        let kms = kms();
        let pin = RefsDekPin::new(Arc::clone(&kms));
        let live = TenantId("live-co".into());
        let dead = TenantId("offboarded-co".into());
        pin.reserve(&live, &r()).expect("live");
        pin.reserve(&dead, &r()).expect("dead");

        assert!(
            pin.destroy_tenant_dek(&dead, &r()).unwrap(),
            "offboard the dead tenant"
        );

        let snap = kms.backup_snapshot().unwrap();
        assert!(
            snap.iter().any(|(d, _)| d.tenant == live),
            "live tenant DEK backed up"
        );
        assert!(
            !snap.iter().any(|(d, _)| d.tenant == dead),
            "a crypto-shredded Refs tenant is EXCLUDED from backup (stays dead across restore)"
        );
    }

    #[test]
    fn refs_uses_the_one_cell_engine_not_a_second_kms() {
        let kms = kms();
        let pin = RefsDekPin::new(Arc::clone(&kms));
        let key_ref = pin.reserve(&t(), &r()).expect("reserve through the pin");
        assert!(
            kms.resolve_dek(&key_ref, &r()).is_ok(),
            "the shared cell engine resolves the DEK the Refs pin reserved (one hierarchy)"
        );
        assert!(
            Arc::ptr_eq(pin.engine(), &kms),
            "the pin holds the very same cell engine"
        );
    }

    #[test]
    fn ref_p5_inherited_gates_name_every_precondition() {
        let gates = ref_p5_inherited_gates();
        let ids: Vec<&str> = gates.iter().map(|g| g.id).collect();
        for required in [
            "STOR-D1", "STOR-D2", "ID-D3", "ID-D2", "ID-D1", "CP-D2", "CP-D3",
        ] {
            assert!(
                ids.contains(&required),
                "the REF-P5 precondition list names {required}"
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
