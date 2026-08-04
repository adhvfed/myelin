use std::sync::Arc;

use myelin_events::{InlinePiiShredder, PiiKeyRef as EventsPiiKeyRef, ShredError};
use myelin_tenancy::Region;

use crate::kms::{DekId, KmsEngine, KmsError, PiiKeyRef as KmsPiiKeyRef};

#[derive(Clone)]
pub struct KmsBusShredder {
    engine: Arc<KmsEngine>,
    region: Region,
}

impl KmsBusShredder {
    pub fn new(engine: Arc<KmsEngine>, region: Region) -> KmsBusShredder {
        KmsBusShredder { engine, region }
    }

    fn parse_ref(key_ref: &EventsPiiKeyRef) -> Option<KmsPiiKeyRef> {
        KmsPiiKeyRef::parse(&key_ref.0)
    }
}

impl InlinePiiShredder for KmsBusShredder {
    fn destroy_key(&self, key_ref: &EventsPiiKeyRef) -> Result<(), ShredError> {
        let Some(parsed) = Self::parse_ref(key_ref) else {
            return Err(ShredError::KmsUnavailable(key_ref.clone()));
        };
        let dek_id = DekId::new(parsed.tenant.clone(), parsed.class.clone());
        self.engine.destroy_dek(&dek_id);
        Ok(())
    }

    fn is_live(&self, key_ref: &EventsPiiKeyRef) -> bool {
        let Some(parsed) = Self::parse_ref(key_ref) else {
            return true;
        };
        match self.engine.resolve_dek(&parsed, &self.region) {
            Ok(_) => true,
            Err(KmsError::DekUnavailable(_)) => false,
            Err(_) => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kms::{KekId, KeyClass};
    use myelin_tenancy::TenantId;

    fn region() -> Region {
        Region("fr-par".into())
    }

    fn engine_with_subject(tenant: &TenantId, subject: &str) -> (Arc<KmsEngine>, EventsPiiKeyRef) {
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(tenant.clone(), region()));
        let kref = kms
            .ensure_dek(tenant, &region(), KeyClass::Subject(subject.into()))
            .expect("mint subject DEK");
        let events_ref = EventsPiiKeyRef(kref.to_uri());
        (Arc::new(kms), events_ref)
    }

    #[test]
    fn live_subject_dek_is_live_then_destroyed_is_not() {
        let tenant = TenantId("acme".into());
        let (kms, kref) = engine_with_subject(&tenant, "u42");
        let shredder = KmsBusShredder::new(kms.clone(), region());

        assert!(shredder.is_live(&kref), "a freshly minted DEK is live");
        shredder.destroy_key(&kref).expect("destroy");
        assert!(
            !shredder.is_live(&kref),
            "a destroyed DEK is not live (crypto-shred)"
        );
    }

    #[test]
    fn destroy_is_idempotent_for_the_reerasure_replay() {
        let tenant = TenantId("acme".into());
        let (kms, kref) = engine_with_subject(&tenant, "u7");
        let shredder = KmsBusShredder::new(kms, region());
        shredder.destroy_key(&kref).expect("first destroy");
        shredder
            .destroy_key(&kref)
            .expect("idempotent re-destroy succeeds");
        assert!(!shredder.is_live(&kref));
    }

    #[test]
    fn a_malformed_key_ref_is_loud_never_assumed_erased() {
        let kms = Arc::new(KmsEngine::new());
        let shredder = KmsBusShredder::new(kms, region());
        let bad = EventsPiiKeyRef("not-a-kms-uri".into());
        assert!(matches!(
            shredder.destroy_key(&bad),
            Err(ShredError::KmsUnavailable(_))
        ));
        assert!(shredder.is_live(&bad));
    }

    #[test]
    fn destroyed_subject_dek_is_excluded_from_the_backup_snapshot() {
        let tenant = TenantId("acme".into());
        let (kms, kref) = engine_with_subject(&tenant, "u99");
        let parsed = KmsPiiKeyRef::parse(&kref.0).unwrap();
        let dek_id = DekId::new(parsed.tenant.clone(), parsed.class.clone());
        assert!(
            kms.backup_snapshot().iter().any(|(d, _)| *d == dek_id),
            "the subject DEK is in the backup before erase"
        );
        let shredder = KmsBusShredder::new(kms.clone(), region());
        shredder.destroy_key(&kref).expect("destroy");
        assert!(
            !kms.backup_snapshot().iter().any(|(d, _)| *d == dek_id),
            "the destroyed subject DEK is EXCLUDED from the backup snapshot (§7.5)"
        );
    }
}
