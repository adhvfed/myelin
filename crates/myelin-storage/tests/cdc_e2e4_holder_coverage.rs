//! E2E-4 holder-coverage CDC pair — the **full DSAR / crypto-shred fan-out across all H1–H18 holders**
//! (P-ST-35 / global P-446; contract 10.4 "the DSR fan-out", 11.4 "the crypto-shred reach").
//!
//! The E2E-4 spine has TWO grains that MUST agree on the same holder-coverage contract:
//!   - the **PROVIDER** = `myelin-storage` — [`HolderClass::ALL`] is the exhaustive H1–H18 catalogue
//!     the storage-side crypto-shred fan-out reaches (the real key-destroy that runs IN the data layer);
//!   - the **CONSUMER** = `myelin-gdpr-service` — its `orchestration::holder_ids` are the storage-owned
//!     holder ids the data-map-driven DSR fan-out (GA-D1) drives, expecting each to be reached.
//!
//! This CDC pair pins that **the storage catalogue is a SUPERSET of the orchestrator's storage-owned
//! holder ids** — i.e. every holder the orchestrator dispatches an erase to has a real crypto-shred
//! reach in storage's H1–H18 catalogue. If a storage-owned holder id the orchestrator drives drifts
//! away from what the storage catalogue covers (a holder renamed, dropped, or never added), this stops
//! passing — so "the orchestrator fans out to a holder storage cannot crypto-shred" is structurally
//! impossible. `myelin-gdpr-service` does NOT depend on `myelin-storage`, so this dev-only edge (the
//! CDC consumer reaching DOWN to its provider's test) introduces no build cycle (coherence EI-01 §7).

use myelin_gdpr_service::orchestration::holder_ids;
use myelin_storage::{holder_ids_not_covered, HolderClass};

/// **The PROVIDER (storage `HolderClass::ALL`) covers EVERY holder id the CONSUMER (the orchestrator's
/// `holder_ids`) drives.** The orchestrator's storage-owned holder ids — identity, blob, authz tuples,
/// bus, cache/CDN, backups — must each be a holder the storage catalogue can crypto-shred.
#[test]
fn cdc_storage_catalogue_covers_the_orchestrator_storage_holder_ids() {
    let orchestrator_storage_holders = [
        holder_ids::IDENTITY,     // H15 — the pseudonym map (the erasure lever)
        holder_ids::BLOB,         // H6/H2 — the object/blob store
        holder_ids::AUTHZ_TUPLES, // H14 — the authz tuples
        holder_ids::BUS,          // H10/H8 — the event bus
        holder_ids::CACHE,        // H17/H9 — caches/CDN
        holder_ids::BACKUP,       // H18/H10 — the backup tier
    ];

    let not_covered = holder_ids_not_covered(&orchestrator_storage_holders);
    assert!(
        not_covered.is_empty(),
        "the storage H1–H18 catalogue MUST cover every storage-owned orchestrator holder id; \
         not covered: {not_covered:?}"
    );
}

/// The two grains agree on the EXACT id strings (the contract is the id, not just the count). Each
/// orchestrator id resolves to exactly one `HolderClass` whose `holder_id()` round-trips to it.
#[test]
fn cdc_each_orchestrator_holder_id_resolves_to_one_storage_holder() {
    for id in [
        holder_ids::IDENTITY,
        holder_ids::BLOB,
        holder_ids::AUTHZ_TUPLES,
        holder_ids::BUS,
        holder_ids::CACHE,
        holder_ids::BACKUP,
    ] {
        let matches: Vec<HolderClass> = HolderClass::ALL
            .iter()
            .copied()
            .filter(|h| h.holder_id() == id)
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "the orchestrator holder id {id:?} resolves to EXACTLY one storage HolderClass (got {matches:?})"
        );
    }
}

/// The catalogue is the EXHAUSTIVE H1–H18 set the E2E-4 gate names — neither grain may silently shrink
/// it. (The completeness contract is "all H1–H18", not "the six storage-owned ids the orchestrator
/// happens to drive today" — the producer/consumer holders join the orchestrator's map as they ship.)
#[test]
fn cdc_catalogue_is_the_full_h1_h18_set() {
    assert_eq!(
        HolderClass::ALL.len(),
        18,
        "the E2E-4 holder catalogue is the exhaustive H1–H18 set"
    );
}
