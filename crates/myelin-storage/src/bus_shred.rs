//! # `bus_shred` — the REAL `KmsEngine`-backed crypto-shred seam for the Bus's holder (EB-29 / P-420)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/event-bus.md` §4.8 (retention + crypto-shred +
//! tombstones — the inline-PII event is envelope-encrypted with a `pii_key_ref`; erasure = destroy
//! the key, which renders the ciphertext unrecoverable **in the live log AND in every backup**) and
//! `storage.md` §7.5 (a crypto-shredded key is **excluded from `backup_snapshot`** — it must stay
//! dead across a restore). **Contracts:** 2.7 (the Bus as a `PersonalDataHolder` — crypto-shred to
//! the KMS hierarchy), 2.7-to-backups (the EB-29 BUS-D8-backups leg), 11.5 (the restore cross-seam).
//!
//! ## What this module is (the floor the Bus's holder named, now bound to the REAL KMS)
//! `myelin-events`'s [`BusHolder`](myelin_events::BusHolder) crypto-shreds inline-PII events through
//! the [`InlinePiiShredder`](myelin_events::InlinePiiShredder) trait — a seam it defines but does NOT
//! bind to a real key store (events sits BELOW storage in the DAG, so it cannot pull
//! [`KmsEngine`](crate::kms::KmsEngine); the events-side [`InMemoryShredder`] is its test/floor
//! backing, and the real binding was named as the downstream adapter floor P-GA-06). **This is that
//! binding**: [`KmsBusShredder`] implements the events-side `InlinePiiShredder` trait over the REAL
//! [`KmsEngine`], so the Bus's holder destroys the SAME per-subject DEK the OLTP columns / blobs /
//! firehose segments resolve through — one key hierarchy, one crypto-shred lever, never a parallel
//! key store (the cold == live invariant, EI-01 §7).
//!
//! `destroy_key` maps the events `PiiKeyRef` URI → the storage [`PiiKeyRef`](crate::kms::PiiKeyRef)
//! → its [`DekId`](crate::kms::DekId) → [`KmsEngine::destroy_dek`]; `is_live` resolves the DEK
//! ([`KmsEngine::resolve_dek`]) and reports liveness — `false` once destroyed. Because the destroy
//! goes through the real engine, a destroyed DEK is ALSO excluded from
//! [`KmsEngine::backup_snapshot`] by construction (§7.5) — which is what lets the EB-29 BUS-D8
//! **reaches-backups** drill prove "0 recoverable inline-PII in backups" against a REAL restored copy
//! (`tests/drills_bus_d8_backups.rs`), not just the live log (the live-store leg, proven at EB-15).
//!
//! ## Loud, never fail-open (the 0-fail-open invariant)
//! A `resolve_dek` that fails for any reason OTHER than "the key is gone" (e.g. a transient KEK
//! unavailability) must NOT be reported as "live=false" (that would silently claim an erase
//! succeeded when it did not) NOR as a destroy success. `is_live` reports liveness strictly: a
//! cleanly-resolvable DEK is live; a [`KmsError::DekUnavailable`] is the only "not live" answer; any
//! OTHER `KmsError` is surfaced as live=true conservatively (so the holder re-verify treats the
//! erase as INCOMPLETE rather than falsely green). `destroy_key` is idempotent (destroying an absent
//! key is a no-op success — the re-erasure-after-restore replay, EB-16) and only ever LOUD on a
//! genuinely un-parseable key ref (a malformed ref is a programming error, never silently swallowed).

use std::sync::Arc;

use myelin_events::{InlinePiiShredder, PiiKeyRef as EventsPiiKeyRef, ShredError};
use myelin_tenancy::Region;

use crate::kms::{DekId, KmsEngine, KmsError, PiiKeyRef as KmsPiiKeyRef};

/// **The REAL `KmsEngine`-backed [`InlinePiiShredder`] for the Bus's holder (EB-29 / contract 2.7).**
///
/// Wraps a shared [`KmsEngine`] + the cell [`Region`] (the engine resolves a DEK through its region's
/// KEK). Hand it to [`BusHolder::new`](myelin_events::BusHolder::new) and the Bus's holder
/// crypto-shreds inline-PII events through the SAME key hierarchy storage owns — so a Bus erase
/// reaches the backup snapshot (the DEK is excluded once destroyed, §7.5) exactly as an OLTP-column
/// erase does. This is the adapter that lets the BUS-D8 **reaches-backups** drill run against the
/// real engine.
#[derive(Clone)]
pub struct KmsBusShredder {
    engine: Arc<KmsEngine>,
    region: Region,
}

impl KmsBusShredder {
    /// Bind the Bus's crypto-shred seam to a shared [`KmsEngine`] in a `region`. The `engine` is an
    /// [`Arc`] so the Bus's holder shares the SAME engine instance the rest of storage resolves keys
    /// through — never a parallel copy (the one-key-hierarchy invariant).
    pub fn new(engine: Arc<KmsEngine>, region: Region) -> KmsBusShredder {
        KmsBusShredder { engine, region }
    }

    /// Parse the events-side `PiiKeyRef` URI into the storage [`PiiKeyRef`](crate::kms::PiiKeyRef). A
    /// malformed ref is a loud `None` (a wrong key ref is NEVER silently coerced — it would be a
    /// wrong-key read; the holder surfaces it as an incomplete erase).
    fn parse_ref(key_ref: &EventsPiiKeyRef) -> Option<KmsPiiKeyRef> {
        KmsPiiKeyRef::parse(&key_ref.0)
    }
}

impl InlinePiiShredder for KmsBusShredder {
    /// Destroy the per-subject DEK named by `key_ref` through the REAL engine
    /// ([`KmsEngine::destroy_dek`]). Idempotent: destroying an absent key is a no-op success (the
    /// EB-16 re-erasure-after-restore replay re-runs this over the ledger). A malformed key ref is
    /// surfaced LOUD as [`ShredError::KmsUnavailable`] (the erase is INCOMPLETE — never "assume
    /// erased" off a ref we could not parse).
    fn destroy_key(&self, key_ref: &EventsPiiKeyRef) -> Result<(), ShredError> {
        let Some(parsed) = Self::parse_ref(key_ref) else {
            // A ref we cannot parse cannot be safely destroyed — surface it LOUD (incomplete erase).
            return Err(ShredError::KmsUnavailable(key_ref.clone()));
        };
        let dek_id = DekId::new(parsed.tenant.clone(), parsed.class.clone());
        // `destroy_dek` returns false if the DEK was already absent; that is the idempotent
        // re-erasure case (a restore brought nothing back, or it was already shredded) — a SUCCESS,
        // not a failure (the key is gone, which is the post-condition we want).
        self.engine.destroy_dek(&dek_id);
        Ok(())
    }

    /// Whether the DEK named by `key_ref` is still resolvable (live). `false` once
    /// [`KmsEngine::destroy_dek`] has removed it. A [`KmsError::DekUnavailable`] is the only "not
    /// live" answer; any OTHER resolve error (e.g. a transient KEK outage) is reported live=true
    /// CONSERVATIVELY, so the holder's re-verify treats the erase as INCOMPLETE rather than falsely
    /// claiming 0-recoverable (0-fail-open: never silently claim erased).
    fn is_live(&self, key_ref: &EventsPiiKeyRef) -> bool {
        let Some(parsed) = Self::parse_ref(key_ref) else {
            // A ref we cannot parse: we cannot prove it dead → conservatively live (incomplete).
            return true;
        };
        match self.engine.resolve_dek(&parsed, &self.region) {
            Ok(_) => true,
            Err(KmsError::DekUnavailable(_)) => false,
            // Any other error (KEK unavailable, unwrap failed) is NOT a proof of destruction —
            // report live so the erase is treated as incomplete, never falsely green.
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
        // Re-running destroy over an already-shredded key is a no-op success (EB-16 replay).
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
        // destroy a malformed ref → LOUD incomplete, never a silent success.
        assert!(matches!(
            shredder.destroy_key(&bad),
            Err(ShredError::KmsUnavailable(_))
        ));
        // and is_live conservatively reports live (we cannot prove a malformed ref destroyed).
        assert!(shredder.is_live(&bad));
    }

    #[test]
    fn destroyed_subject_dek_is_excluded_from_the_backup_snapshot() {
        // The headline §7.5 property the BUS-D8 reaches-backups leg leans on: once the Bus's shredder
        // destroys the per-subject DEK, it is gone from the engine AND the backup snapshot.
        let tenant = TenantId("acme".into());
        let (kms, kref) = engine_with_subject(&tenant, "u99");
        let parsed = KmsPiiKeyRef::parse(&kref.0).unwrap();
        let dek_id = DekId::new(parsed.tenant.clone(), parsed.class.clone());
        // present before erase.
        assert!(
            kms.backup_snapshot().iter().any(|(d, _)| *d == dek_id),
            "the subject DEK is in the backup before erase"
        );
        let shredder = KmsBusShredder::new(kms.clone(), region());
        shredder.destroy_key(&kref).expect("destroy");
        // excluded after erase — a restore cannot resurrect it (§7.5).
        assert!(
            !kms.backup_snapshot().iter().any(|(d, _)| *d == dek_id),
            "the destroyed subject DEK is EXCLUDED from the backup snapshot (§7.5)"
        );
    }
}
