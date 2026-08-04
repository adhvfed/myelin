use myelin_tenancy::TenantId;

use crate::blob::{BlobError, BlobMeta, BlobStore, ContentHash, Result};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Default)]
pub struct ReplicaTelemetry {
    blob_recovered_from_replica: AtomicU64,
    blob_unrecoverable: AtomicU64,
}

impl ReplicaTelemetry {
    pub fn blob_recovered_from_replica(&self) -> u64 {
        self.blob_recovered_from_replica.load(Ordering::SeqCst)
    }

    pub fn blob_unrecoverable(&self) -> u64 {
        self.blob_unrecoverable.load(Ordering::SeqCst)
    }

    fn record_recovered(&self) {
        self.blob_recovered_from_replica
            .fetch_add(1, Ordering::SeqCst);
    }

    fn record_unrecoverable(&self) {
        self.blob_unrecoverable.fetch_add(1, Ordering::SeqCst);
    }
}

pub struct ReplicatedBlobStore<B: BlobStore> {
    primary: B,
    replicas: Vec<B>,
    telemetry: ReplicaTelemetry,
}

impl<B: BlobStore> ReplicatedBlobStore<B> {
    pub fn new(primary: B, replicas: Vec<B>) -> ReplicatedBlobStore<B> {
        ReplicatedBlobStore {
            primary,
            replicas,
            telemetry: ReplicaTelemetry::default(),
        }
    }

    pub fn telemetry(&self) -> &ReplicaTelemetry {
        &self.telemetry
    }

    pub fn replica_count(&self) -> usize {
        self.replicas.len()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl ReplicatedBlobStore<crate::blob::FsBlobStore> {
    #[doc(hidden)]
    pub fn corrupt_primary_for_drill(&self, tenant: &TenantId, hash: &ContentHash) -> bool {
        self.primary.corrupt_for_drill(tenant, hash)
    }

    #[doc(hidden)]
    pub fn corrupt_all_for_drill(&self, tenant: &TenantId, hash: &ContentHash) -> bool {
        let mut any = self.primary.corrupt_for_drill(tenant, hash);
        for replica in &self.replicas {
            any |= replica.corrupt_for_drill(tenant, hash);
        }
        any
    }
}

impl<B: BlobStore> BlobStore for ReplicatedBlobStore<B> {
    fn put(&self, tenant: &TenantId, bytes: &[u8]) -> Result<ContentHash> {
        let hash = self.primary.put(tenant, bytes)?;
        for replica in &self.replicas {
            let r = replica.put(tenant, bytes)?;
            debug_assert_eq!(r, hash, "every copy is content-addressed identically");
        }
        Ok(hash)
    }

    fn get(&self, tenant: &TenantId, hash: &ContentHash) -> Result<Vec<u8>> {
        match self.primary.get(tenant, hash) {
            Ok(bytes) => Ok(bytes),
            Err(primary_err @ (BlobError::IntegrityFail { .. } | BlobError::NotFound { .. })) => {
                for replica in &self.replicas {
                    if let Ok(bytes) = replica.get(tenant, hash) {
                        let _ = self.primary.put(tenant, &bytes);
                        self.telemetry.record_recovered();
                        return Ok(bytes);
                    }
                }
                self.telemetry.record_unrecoverable();
                Err(primary_err)
            }
            Err(other) => Err(other),
        }
    }

    fn head(&self, tenant: &TenantId, hash: &ContentHash) -> Result<BlobMeta> {
        match self.primary.head(tenant, hash) {
            Ok(meta) => Ok(meta),
            Err(BlobError::NotFound { .. }) => {
                for replica in &self.replicas {
                    if let Ok(meta) = replica.head(tenant, hash) {
                        return Ok(meta);
                    }
                }
                Err(BlobError::NotFound {
                    tenant: tenant.clone(),
                    hash: hash.clone(),
                })
            }
            Err(other) => Err(other),
        }
    }

    fn delete(&self, tenant: &TenantId, hash: &ContentHash) -> Result<()> {
        let swallow_not_found = |r: Result<()>| -> Result<()> {
            match r {
                Ok(()) | Err(BlobError::NotFound { .. }) => Ok(()),
                Err(e) => Err(e),
            }
        };
        swallow_not_found(self.primary.delete(tenant, hash))?;
        for replica in &self.replicas {
            swallow_not_found(replica.delete(tenant, hash))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::FsBlobStore;

    fn tenant(s: &str) -> TenantId {
        TenantId(s.to_string())
    }

    #[test]
    fn put_get_head_delete_round_trip_through_unchanged_trait() {
        let store = ReplicatedBlobStore::new(
            FsBlobStore::new(),
            vec![FsBlobStore::new(), FsBlobStore::new()],
        );
        let acme = tenant("acme");
        let bytes = b"object-store backing, replicated";

        let h = store.put(&acme, bytes).expect("put");
        assert_eq!(
            h,
            ContentHash::blake3(bytes),
            "content address is unchanged"
        );
        assert_eq!(store.get(&acme, &h).expect("get"), bytes);
        assert_eq!(store.head(&acme, &h).expect("head").stored_len, bytes.len());

        store.delete(&acme, &h).expect("delete reaches every copy");
        assert!(matches!(
            store.get(&acme, &h),
            Err(BlobError::IntegrityFail { .. }) | Err(BlobError::NotFound { .. })
        ));
        assert_eq!(store.telemetry().blob_recovered_from_replica(), 0);
    }

    #[test]
    fn corrupt_primary_recovers_from_replica_and_heals() {
        let primary = FsBlobStore::new();
        let replica_a = FsBlobStore::new();
        let replica_b = FsBlobStore::new();
        let acme = tenant("acme");
        let bytes = b"trustworthy-replicated-bytes";
        let h = primary.put(&acme, bytes).unwrap();
        replica_a.put(&acme, bytes).unwrap();
        replica_b.put(&acme, bytes).unwrap();
        assert!(primary.corrupt_for_drill(&acme, &h));

        let store = ReplicatedBlobStore::new(primary, vec![replica_a, replica_b]);
        let served = store.get(&acme, &h).expect("recovered from replica");
        assert_eq!(served, bytes, "recovered bytes are the correct content");
        assert_eq!(store.telemetry().blob_recovered_from_replica(), 1);
        assert_eq!(store.telemetry().blob_unrecoverable(), 0);

        assert_eq!(store.get(&acme, &h).expect("primary healed"), bytes);
        assert_eq!(
            store.telemetry().blob_recovered_from_replica(),
            1,
            "the healed primary serves without a second recovery"
        );
    }

    #[test]
    fn missing_primary_recovers_from_replica() {
        let primary = FsBlobStore::new();
        let replica = FsBlobStore::new();
        let acme = tenant("acme");
        let bytes = b"only-on-the-replica-after-loss";
        let h = replica.put(&acme, bytes).unwrap();

        let store = ReplicatedBlobStore::new(primary, vec![replica]);
        assert_eq!(store.get(&acme, &h).expect("recovered"), bytes);
        assert_eq!(store.telemetry().blob_recovered_from_replica(), 1);
        assert_eq!(store.head(&acme, &h).expect("head fallback").hash, h);
    }

    #[test]
    fn all_copies_corrupt_refuses_to_serve() {
        let primary = FsBlobStore::new();
        let replica = FsBlobStore::new();
        let acme = tenant("acme");
        let bytes = b"doomed-bytes";
        let h = primary.put(&acme, bytes).unwrap();
        replica.put(&acme, bytes).unwrap();
        assert!(primary.corrupt_for_drill(&acme, &h));
        assert!(replica.corrupt_for_drill(&acme, &h));

        let store = ReplicatedBlobStore::new(primary, vec![replica]);
        match store.get(&acme, &h) {
            Err(BlobError::IntegrityFail { requested, .. }) => assert_eq!(requested, h),
            Ok(b) => panic!("SILENT SERVE - STOR-D7 breached with all copies corrupt: {b:?}"),
            Err(other) => panic!("expected IntegrityFail, got {other}"),
        }
        assert_eq!(store.telemetry().blob_recovered_from_replica(), 0);
        assert_eq!(store.telemetry().blob_unrecoverable(), 1);
    }

    #[test]
    fn delete_reaches_every_copy() {
        let store = ReplicatedBlobStore::new(
            FsBlobStore::new(),
            vec![FsBlobStore::new(), FsBlobStore::new()],
        );
        let acme = tenant("acme");
        let h = store.put(&acme, b"to-be-erased").unwrap();
        store.delete(&acme, &h).expect("delete");
        assert!(matches!(
            store.get(&acme, &h),
            Err(BlobError::NotFound { .. }) | Err(BlobError::IntegrityFail { .. })
        ));
        assert_eq!(store.telemetry().blob_recovered_from_replica(), 0);
        store.delete(&acme, &h).expect("idempotent delete");
    }

    #[test]
    fn non_corruption_error_is_not_recovered() {
        let store = ReplicatedBlobStore::new(FsBlobStore::new(), vec![FsBlobStore::new()]);
        let acme = tenant("acme");
        let absent = ContentHash::blake3(b"never stored anywhere");
        assert!(matches!(
            store.get(&acme, &absent),
            Err(BlobError::NotFound { .. })
        ));
        assert_eq!(store.telemetry().blob_recovered_from_replica(), 0);
        assert_eq!(store.telemetry().blob_unrecoverable(), 1);
    }
}
