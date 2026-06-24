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

use myelin_gdpr_service::full_fanout::Holder;
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

// ───────── P-GA-32 → P-448: reconcile the gdpr §3.2 catalogue with the storage D-S5 catalogue ─────────
//
// `myelin-gdpr-service::full_fanout::Holder::ALL` is the gdpr §3.2 H1–H18 catalogue (the DSR-completeness
// grain — GA-D1, 0 holders missed); `myelin-storage::HolderClass::ALL` is the storage D-S5 H1–H18 catalogue
// (the crypto-shred-routing grain). The two use DIFFERENT H-numbering conventions (gdpr H6 = blob, storage
// H2 = blob) but describe the SAME 18 real holders. These CDC tests pin that the two catalogues AGREE on the
// real-holder set so neither re-derives the other and a holder added to one is forced into the other.

/// **Both H1–H18 catalogues have EXACTLY 18 holders.** Neither grain may silently shrink the set.
#[test]
fn cdc_both_catalogues_are_18_holders() {
    assert_eq!(
        Holder::ALL.len(),
        18,
        "the gdpr §3.2 catalogue is exhaustive (18)"
    );
    assert_eq!(
        HolderClass::ALL.len(),
        18,
        "the storage D-S5 catalogue is exhaustive (18)"
    );
}

/// **The storage catalogue covers every STORAGE-OWNED holder id the gdpr §3.2 catalogue names.** The
/// storage-owned holders (the stores whose crypto-shred runs IN the data layer: identity, blob, event
/// bus, caches/CDN, backups, authz tuples, search+vectors, agent memory, refs, notif, audit carve-out)
/// must name the SAME store id in both catalogues — so the gdpr completeness grain and the storage
/// crypto-shred grain reach the same store. (The gdpr per-subsystem-DB holders — H1 Git DB, H2 CI DB,
/// H3 Issues DB, H4 Knowledge DB, H5 Chat DB, H17 agent trace, H18 GDPR-owned — map onto storage's
/// store-type classes by mechanism, not by a shared store id, and are reconciled by erasure mechanism
/// at the orchestration seam, not here.)
#[test]
fn cdc_storage_catalogue_covers_the_gdpr_storage_owned_holder_ids() {
    let gdpr_storage_owned: Vec<&str> = [
        Holder::Identity,            // pseudonym map (the erasure lever)
        Holder::ObjectStore,         // the object/blob store
        Holder::EventBus,            // the event bus
        Holder::CachesAndCdn,        // caches / CDN
        Holder::Backups,             // the backup tier
        Holder::AuthzTuples,         // the authz tuples
        Holder::SearchIndex,         // the search index + vectors
        Holder::AgentMemory,         // agent memory / embeddings
        Holder::ReferenceGraph,      // the reference graph
        Holder::NotificationHistory, // the notification inbox
        Holder::AuditCarveOut,       // the audit carve-out
    ]
    .iter()
    .map(|h| h.holder_id())
    .collect();

    let not_covered = holder_ids_not_covered(&gdpr_storage_owned);
    assert!(
        not_covered.is_empty(),
        "the storage D-S5 catalogue must cover every storage-owned gdpr §3.2 holder id; missing: {not_covered:?}"
    );
}

/// **Each storage-owned gdpr holder id resolves to EXACTLY ONE storage `HolderClass`** — the two
/// catalogues name the store one-to-one (no ambiguity, no drift).
#[test]
fn cdc_each_gdpr_storage_owned_id_resolves_to_one_storage_class() {
    for h in [
        Holder::Identity,
        Holder::ObjectStore,
        Holder::EventBus,
        Holder::CachesAndCdn,
        Holder::Backups,
        Holder::AuthzTuples,
        Holder::SearchIndex,
        Holder::AgentMemory,
        Holder::ReferenceGraph,
        Holder::NotificationHistory,
        Holder::AuditCarveOut,
    ] {
        let id = h.holder_id();
        let matches: Vec<HolderClass> = HolderClass::ALL
            .iter()
            .copied()
            .filter(|c| c.holder_id() == id)
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "the gdpr holder {} ({id:?}) resolves to EXACTLY one storage HolderClass (got {matches:?})",
            h.h_label()
        );
    }
}

/// **The orchestrator's storage-owned `holder_ids` are a SUBSET of the gdpr §3.2 catalogue ids** — the
/// six ids the M1 orchestrator drives all resolve back to a gdpr §3.2 holder (so the orchestration grain
/// and the completeness grain agree on what those stores are called).
#[test]
fn cdc_orchestrator_holder_ids_resolve_in_the_gdpr_catalogue() {
    for id in [
        holder_ids::IDENTITY,
        holder_ids::BLOB,
        holder_ids::AUTHZ_TUPLES,
        holder_ids::BUS,
        holder_ids::CACHE,
        holder_ids::BACKUP,
    ] {
        assert!(
            Holder::from_id(id).is_some(),
            "the orchestrator holder id {id:?} resolves to a gdpr §3.2 H-class"
        );
    }
}
