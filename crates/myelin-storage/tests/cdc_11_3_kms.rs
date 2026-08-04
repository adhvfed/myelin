use myelin_storage::{DekId, KekId, KeyClass, KmsEngine, PiiKeyRef};
use myelin_tenancy::{Region, TenantId};

struct EncryptedProfileStore<'a> {
    kms: &'a KmsEngine,
    tenant: TenantId,
    region: Region,
    rows: std::collections::HashMap<String, (PiiKeyRef, [u8; 12], Vec<u8>)>,
}

impl<'a> EncryptedProfileStore<'a> {
    fn boot(kms: &'a KmsEngine, tenant: TenantId, region: Region) -> Self {
        kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
        EncryptedProfileStore {
            kms,
            tenant,
            region,
            rows: Default::default(),
        }
    }

    fn write_bio(&mut self, subject: &str, bio: &[u8]) {
        let key_ref = self
            .kms
            .ensure_dek(
                &self.tenant,
                &self.region,
                KeyClass::Subject(subject.into()),
            )
            .expect("provider: ensure per-subject DEK");
        let dek = self
            .kms
            .resolve_dek(&key_ref, &self.region)
            .expect("provider: resolve DEK");
        let (nonce, ct) = dek.seal(bio);
        self.rows.insert(subject.into(), (key_ref, nonce, ct));
    }

    fn read_bio(&self, subject: &str) -> Option<Vec<u8>> {
        let (key_ref, nonce, ct) = self.rows.get(subject)?;
        let dek = self.kms.resolve_dek(key_ref, &self.region).ok()?;
        dek.open(nonce, ct)
    }
}

#[test]
fn cdc_11_3_encrypted_store_wraps_and_unwraps_through_the_kms() {
    let kms = KmsEngine::new();
    let (tenant, region) = (TenantId("acme".into()), Region("eu-west".into()));
    let mut store = EncryptedProfileStore::boot(&kms, tenant.clone(), region.clone());

    store.write_bio("alice", b"Alice's free-text bio with PII");
    store.write_bio("bob", b"Bob's free-text bio with PII");

    assert_eq!(
        store.read_bio("alice").as_deref(),
        Some(&b"Alice's free-text bio with PII"[..])
    );
    assert_eq!(
        store.read_bio("bob").as_deref(),
        Some(&b"Bob's free-text bio with PII"[..])
    );

    let (alice_ref, _, _) = &store.rows["alice"];
    assert_eq!(alice_ref.to_uri(), "kms://acme/0/subject:alice");

    let alice_dek = DekId::new(tenant, KeyClass::Subject("alice".into()));
    assert!(
        kms.destroy_dek(&alice_dek),
        "provider: per-subject DEK present to shred"
    );

    assert_eq!(
        store.read_bio("alice"),
        None,
        "11.3: crypto-shred makes the column unrecoverable"
    );
    assert_eq!(
        store.read_bio("bob").as_deref(),
        Some(&b"Bob's free-text bio with PII"[..]),
        "11.3: another subject's per-subject DEK is untouched by alice's shred"
    );
}
