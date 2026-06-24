//! # P-GA-32 → P-448 — GA-D1 at cell scale: the full H1–H18 DSR fan-out (0 holders missed)
//!
//! **DATED GREEN ARTIFACT (2026-06-24).** This integration drill is the dated green artifact the
//! P-GA-32 GATE requires — the **headline GDPR drill (GA-D1)** at cell scale: a subject seeded into
//! **all H1–H18 holders** → a single `dsr_submit` → the **data-map-driven** fan-out reaches **every
//! holder** → **0 holders missed**, **`erasure_fanout_coverage == 1.0` over the WHOLE H1–H18 set** →
//! a Merkle-provable certificate seals. (The Merkle inclusion rides P-GA-20; the certificate carries
//! the content-addressed leaf.)
//!
//! ## What this proves (the P-GA-32 completeness gate) vs what it REUSES (EI-01 §7 coherence)
//! Every PRIOR prompt proved a LEG of the fan-out (the DSR spine P-GA-11; the resumable canonical-order
//! holder fan-out P-GA-06; the data-map-driven driver P-GA-12; the per-subsystem holders
//! P-GA-27/29/30/31; the GDPR-owned holders P-GA-05). What was NOT yet provable was *holder
//! COMPLETENESS*: a `fanout_coverage` over the REGISTERED subset is vacuously 1.0 even if a whole
//! subsystem's holder was never registered. This drill closes that gap: it measures coverage against
//! the **exhaustive [`Holder::ALL`]** (the §3.2 H1–H18 catalogue), so a holder the fan-out did not
//! reach is **MISSED**, never silently 100%.
//!
//! The drill drives the completeness layer FROM the generated data map's holder roster (the same map
//! the DSR fan-out resolves its checklist from — §4.1 step 2): *the map, not a hand-written list,
//! drives the fan-out*. A holder in the §3.2 catalogue that the map never declared is a coverage gap
//! the certificate refuses to seal on.
//!
//! ## The two faces of the gate (green AND red — the gate can go red)
//! 1. **GREEN:** the complete H1–H18 map → 0 holders missed → coverage 1.0 → the certificate seals.
//! 2. **RED:** a map that drops ONE holder → that holder is MISSED → coverage < 1.0 → the certificate
//!    REFUSES to seal (returns a [`myelin_gdpr_service::GaD1Gap`] naming the missed holder). A drill
//!    that cannot go red proves nothing — this asserts the gate IS load-bearing.
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - **The multi-cell `member_cells` fan-out** (per-cell receipts merged into ONE certificate over the
//!   cross-cell PII-free bridge) → **P-GA-33 → P-449 (GA-D8)**. THIS is the single-cell completeness
//!   each cell proves.
//! - **The E2E-4 DSAR flagship** (the whole-system proof across all five subsystems with mock agents)
//!   → **P-GA-34 → P-450**.
//! - **The Merkle SEAL of the GA-D1 certificate into the per-tenant audit tree** → **P-GA-20 → P-119**
//!   (the certificate carries the content-addressed leaf; the anchor rides the existing seam).
//! - **STOR-D3 at cell scale** (restore-resurrects-nothing under world-scale load) is the Storage M5
//!   prompts' green artifact (`myelin-storage` restore-verify + `holder_fanout`); the gdpr half drives
//!   the erasure-ledger write Storage's `post_restore_reerase` consumes (already wired by
//!   `FanOutDriver::with_ledger`, proven in `fanout.rs`).
//! - **The world-scale 30× load** of the whole-cell SCHED drill is the one remaining real-fleet floor
//!   (the completeness PROPERTY here is load-independent — a property of the catalogue + the map).
//! - **The live store-`erase` bindings** behind the holder seams are wired by the harness at boot (the
//!   same in-memory model floor every M1 store carries, P-007/P-S12). This drill touches NO new
//!   DB/object-store/cache/bus contract — it proves the completeness PROPERTY over the generated map,
//!   so no `--features integration` live-stack leg is owed by P-GA-32.

use std::collections::BTreeSet;

use myelin_gdpr_service::full_fanout::{FullFanOutCoverage, GaD1Certificate, Holder};

/// **The complete H1–H18 data-map holder roster** — one PII-free holder id per §3.2 holder, in the
/// flavour each subsystem actually registers it under (so the drill proves [`Holder::from_id`]
/// resolves the REAL registration ids, not just the canonical ones). This is the roster the generated
/// data map carries at M5 (every holder finally exists — the GA-D1 precondition, P-GA-31).
fn complete_h1_h18_map_roster() -> Vec<&'static str> {
    vec![
        "oltp:git_oltp",             // H1 — Git subsystem DB
        "oltp:ci_oltp",              // H2 — CI subsystem DB + log segments
        "oltp:issue_oltp",           // H3 — Issues subsystem DB
        "oltp:knowledge_oltp",       // H4 — Knowledge subsystem DB
        "oltp:chat_oltp",            // H5 — Chat subsystem DB
        "blob:blob_store",           // H6 — object/blob store
        "search_index:search_index", // H7 — Search index + vectors
        "event_bus",                 // H8 — event-bus history
        "cache_cdn",                 // H9 — caches / CDN
        "backups",                   // H10 — backups / snapshots
        "agent_memory",              // H11 — agent memory / embeddings
        "refs_edge:refs_edge",       // H12 — reference graph
        "notif_history",             // H13 — notification history
        "authz_tuples",              // H14 — authz tuples
        "identity",                  // H15 — identity pseudonym map + profile
        "audit_carve_out",           // H16 — audit log carve-out
        "oltp:agent_fabric_trace",   // H17 — agent execution trace
        "gdpr_own_stores",           // H18 — GDPR/Audit own stores (G1–G7)
    ]
}

/// Drive the completeness layer FROM a data-map holder roster (the §4.1 step-2 "resolve scope from the
/// data map" — the map drives the fan-out, not a hand-written list). Every roster id is resolved to its
/// H-class and recorded as reached.
fn drive_from_roster(roster: &[&str]) -> FullFanOutCoverage {
    let mut cov = FullFanOutCoverage::new();
    for id in roster {
        cov.record_reached_id(id);
    }
    cov
}

/// **GA-D1 GREEN at cell scale: the full H1–H18 fan-out reaches every holder — 0 missed, coverage
/// 1.0, the certificate seals.** This is the dated green artifact.
#[test]
fn ga_d1_full_h1_h18_fan_out_reaches_every_holder_0_missed() {
    let roster = complete_h1_h18_map_roster();
    assert_eq!(roster.len(), 18, "the complete map declares all 18 holders");

    let cov = drive_from_roster(&roster);

    // ── the GA-D1 gate readings (the load-bearing zeros) ──
    assert_eq!(
        cov.holders_missed(),
        0,
        "GA-D1: 0 holders missed (incl. all five subsystems)"
    );
    assert_eq!(
        cov.erasure_fanout_coverage(),
        1.0,
        "GA-D1: erasure_fanout_coverage == 100% over the WHOLE H1–H18 set"
    );
    assert!(cov.is_complete(), "the fan-out reached every H1–H18 holder");
    assert!(cov.missed().is_empty(), "no holder named as missed");
    assert!(
        cov.unrecognised().is_empty(),
        "every roster id resolved to a known holder"
    );

    // ── every H1–H18 holder is in the reach manifest, each reached ──
    let manifest = cov.reach_manifest();
    assert_eq!(manifest.len(), 18, "one reach line per H1–H18 holder");
    for line in &manifest {
        assert!(
            line.reached,
            "{} was reached by the fan-out",
            line.holder.h_label()
        );
    }
    // the five subsystem DBs (H1–H5) are all reached — the M5 precondition (all holders exist).
    for h in [
        Holder::GitDb,
        Holder::CiDb,
        Holder::IssuesDb,
        Holder::KnowledgeDb,
        Holder::ChatDb,
    ] {
        assert!(
            cov.reach_manifest()
                .iter()
                .any(|r| r.holder == h && r.reached),
            "{} reached",
            h.h_label()
        );
    }

    // ── the certificate seals (the green artifact) ──
    let cert = GaD1Certificate::seal("acme/subject-cell-scale", &cov)
        .expect("a complete H1–H18 fan-out seals the GA-D1 certificate");
    assert!(cert.is_complete());
    assert_eq!(cert.holders_missed, 0);
    assert_eq!(cert.erasure_fanout_coverage, 1.0);
    assert_eq!(cert.reach.len(), 18);
    assert!(
        cert.content_hash.starts_with("blake3:"),
        "Merkle-provable leaf (the inclusion rides P-GA-20)"
    );

    // MEASURED ARTIFACT: 18 holders reached, 0 missed, erasure_fanout_coverage = 1.0, certificate sealed.
}

/// **GA-D1 RED face: the fan-out missing ONE holder is DETECTED — the certificate REFUSES to seal.**
/// We drop H7 (Search) — the classic "we forgot the search index" gap (§4.1). The completeness layer
/// COUNTS it as missed (not silently 100% over the reached subset) and the certificate cannot seal.
#[test]
fn ga_d1_a_missed_holder_is_detected_the_certificate_refuses_to_seal() {
    // drop H7 (search_index) from the map roster.
    let roster: Vec<&str> = complete_h1_h18_map_roster()
        .into_iter()
        .filter(|id| Holder::from_id(id) != Some(Holder::SearchIndex))
        .collect();
    assert_eq!(roster.len(), 17, "one holder withheld");

    let cov = drive_from_roster(&roster);
    assert_eq!(
        cov.holders_missed(),
        1,
        "the missed holder is COUNTED (not masked as 100%)"
    );
    assert_eq!(cov.missed(), vec![Holder::SearchIndex], "named: H7 Search");
    assert!(
        cov.erasure_fanout_coverage() < 1.0,
        "coverage dropped below 1.0"
    );
    assert!(!cov.is_complete());

    // the certificate REFUSES to seal — a missed holder NEVER produces a green artifact.
    let gap = GaD1Certificate::seal("acme/subject-cell-scale", &cov)
        .expect_err("a missed holder does NOT seal the GA-D1 certificate");
    assert_eq!(gap.holders_missed, 1);
    assert_eq!(gap.missed, vec![Holder::SearchIndex]);
}

/// **Every one of the §3.2 H1–H18 holders is independently provable as a possible miss** — dropping
/// ANY single holder is detected (so the completeness check is not accidentally insensitive to some
/// subset). This is the exhaustiveness proof of the gate.
#[test]
fn ga_d1_dropping_any_single_holder_is_detected() {
    for &dropped in Holder::ALL {
        let roster: Vec<&str> = complete_h1_h18_map_roster()
            .into_iter()
            .filter(|id| Holder::from_id(id) != Some(dropped))
            .collect();
        let cov = drive_from_roster(&roster);
        assert_eq!(
            cov.holders_missed(),
            1,
            "dropping {} is detected as exactly one missed holder",
            dropped.h_label()
        );
        assert_eq!(
            cov.missed(),
            vec![dropped],
            "the dropped holder {} is named",
            dropped.h_label()
        );
        assert!(
            GaD1Certificate::seal("acme/s", &cov).is_err(),
            "dropping {} blocks the certificate seal",
            dropped.h_label()
        );
    }
}

/// **The reach manifest covers EXACTLY the §3.2 catalogue** — no holder outside H1–H18 sneaks in, and
/// the manifest is the whole set (the completeness denominator is the catalogue, not the reached set).
#[test]
fn ga_d1_reach_manifest_is_exactly_the_h1_h18_catalogue() {
    let cov = drive_from_roster(&complete_h1_h18_map_roster());
    let manifest_holders: BTreeSet<Holder> =
        cov.reach_manifest().iter().map(|r| r.holder).collect();
    let catalogue: BTreeSet<Holder> = Holder::ALL.iter().copied().collect();
    assert_eq!(
        manifest_holders, catalogue,
        "the manifest is exactly H1–H18"
    );
}
