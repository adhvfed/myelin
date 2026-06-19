//! Contract 11.3 CDC pair — the three-level KMS hierarchy half (P-ST-06 / global P-058).
//!
//! The prompt requires "CDC: provider+consumer pair for 11.3 (an encrypted-store caller
//! wrapping/unwrapping)". This is the consumer-driven contract test:
//!
//! - the **PROVIDER** is `myelin-storage` — the [`KmsEngine`] + the [`KeyClass`] / [`PiiKeyRef`]
//!   hierarchy this prompt ships;
//! - the **CONSUMER** is an encrypted store (modelled here as a tiny `EncryptedProfileStore`)
//!   that, on write, ensures a per-SUBJECT DEK, seals the column under it, and persists the
//!   [`PiiKeyRef`] alongside the ciphertext; on read it resolves the DEK named by that ref and
//!   opens the column. This is exactly the wrap/unwrap call shape every encrypted store (the
//!   P-ST-08 OLTP/blob wiring) relies on — if `ensure_dek` / `resolve_dek` / the `pii_key_ref`
//!   shape drift, this stops compiling/passing.
//!
//! It also pins the load-bearing contract property the consumer depends on: a per-subject DEK is
//! the GD-4 individual-erasure lever — destroying it renders THAT subject's column unrecoverable
//! while every other subject + the tenant bulk DEK are untouched.
//!
//! NOTE on row 11.3: the contract-index row 11.3 spans BOTH the KMS hierarchy (this prompt,
//! P-ST-06) AND the [`KeyOrigin`] trait (the sibling P-ST-07 / global P-094). This CDC pair
//! covers the HIERARCHY half (the wrap/unwrap an encrypted store calls); P-094 adds the
//! KeyOrigin-trait consumer (the index-builder consulting `can_derive_plaintext_index`) to the
//! same row.

use myelin_storage::{DekId, KeyClass, KekId, KmsEngine, PiiKeyRef};
use myelin_tenancy::{Region, TenantId};

/// A consumer of 11.3: an OLTP-style store that keeps a per-subject-encrypted free-text column
/// (a profile bio — the §5 per-subject erasure class). It holds CIPHERTEXT + the `pii_key_ref`
/// that names the sealing DEK, never plaintext at rest.
struct EncryptedProfileStore<'a> {
    kms: &'a KmsEngine,
    tenant: TenantId,
    region: Region,
    // (subject -> (pii_key_ref, nonce, ciphertext)) — the at-rest, envelope-encrypted column.
    rows: std::collections::HashMap<String, (PiiKeyRef, [u8; 12], Vec<u8>)>,
}

impl<'a> EncryptedProfileStore<'a> {
    fn boot(kms: &'a KmsEngine, tenant: TenantId, region: Region) -> Self {
        // The tenant's KEK must exist before any encrypted store can wrap a DEK (the harness
        // provisions it at cell-onboard; here we ensure it explicitly).
        kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
        EncryptedProfileStore { kms, tenant, region, rows: Default::default() }
    }

    /// Write a subject's bio: ensure the per-subject DEK (11.3), seal the value, persist the
    /// ciphertext + the `pii_key_ref` (the provider's frozen wrap shape the consumer calls).
    fn write_bio(&mut self, subject: &str, bio: &[u8]) {
        let key_ref = self
            .kms
            .ensure_dek(&self.tenant, &self.region, KeyClass::Subject(subject.into()))
            .expect("provider: ensure per-subject DEK");
        let dek = self.kms.resolve_dek(&key_ref, &self.region).expect("provider: resolve DEK");
        let (nonce, ct) = dek.seal(bio);
        self.rows.insert(subject.into(), (key_ref, nonce, ct));
    }

    /// Read a subject's bio: resolve the DEK named by the persisted `pii_key_ref` (the provider's
    /// frozen unwrap shape) and open the column. `None` if the key was crypto-shredded
    /// (unrecoverable — the correct, loud erasure outcome).
    fn read_bio(&self, subject: &str) -> Option<Vec<u8>> {
        let (key_ref, nonce, ct) = self.rows.get(subject)?;
        let dek = self.kms.resolve_dek(key_ref, &self.region).ok()?;
        dek.open(nonce, ct)
    }
}

/// THE CDC pair: an encrypted store writes through the provider's wrap shape, reads back through
/// the provider's unwrap shape, and the per-subject crypto-shred lever behaves as the consumer
/// (the DSR orchestrator) relies on.
#[test]
fn cdc_11_3_encrypted_store_wraps_and_unwraps_through_the_kms() {
    let kms = KmsEngine::new();
    let (tenant, region) = (TenantId("acme".into()), Region("eu-west".into()));
    let mut store = EncryptedProfileStore::boot(&kms, tenant.clone(), region.clone());

    // Two subjects' bios are written, each sealed under its OWN per-subject DEK.
    store.write_bio("alice", b"Alice's free-text bio with PII");
    store.write_bio("bob", b"Bob's free-text bio with PII");

    // Both round-trip through resolve+open (the wrap→unwrap contract).
    assert_eq!(store.read_bio("alice").as_deref(), Some(&b"Alice's free-text bio with PII"[..]));
    assert_eq!(store.read_bio("bob").as_deref(), Some(&b"Bob's free-text bio with PII"[..]));

    // The persisted ref is the frozen §4 shape `kms://<tenant>/<epoch>/subject:<id>`.
    let (alice_ref, _, _) = &store.rows["alice"];
    assert_eq!(alice_ref.to_uri(), "kms://acme/0/subject:alice");

    // The GD-4 individual-erasure lever the consumer (DSR) calls: crypto-shred ONLY alice's DEK.
    let alice_dek = DekId::new(tenant, KeyClass::Subject("alice".into()));
    assert!(kms.destroy_dek(&alice_dek), "provider: per-subject DEK present to shred");

    // alice's bio is now unrecoverable (loud None); bob's is untouched — the contract the DSR
    // orchestrator depends on (one person's Art. 17 erasure, the tenant + others intact).
    assert_eq!(store.read_bio("alice"), None, "11.3: crypto-shred makes the column unrecoverable");
    assert_eq!(
        store.read_bio("bob").as_deref(),
        Some(&b"Bob's free-text bio with PII"[..]),
        "11.3: another subject's per-subject DEK is untouched by alice's shred"
    );
}
