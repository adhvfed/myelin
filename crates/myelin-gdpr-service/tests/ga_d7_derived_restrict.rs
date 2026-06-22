//! # P-GA-25 → P-152 — `restrict` suppression into the derived stores (GATE drill, GA-D7)
//!
//! **DATED GREEN ARTIFACT (2026-06-20).** This integration drill is the dated green artifact the
//! P-GA-25 GATE requires (the GDPR prompts record their drill artifacts as the test itself — there
//! is no GDPR scorecard binary yet). It proves the GATE row of P-GA-25 / drill-catalogue **GA-D7**:
//! *Restrict a subject → no indexing/agent-use/analytics/notification while storage retained;
//! reversible. **0 processing of a restricted subject**.* — fanned into the FIVE M2 derived stores
//! (Search / Refs / Notif / Agents / OLAP), the full restriction-into-derived-stores proof P-GA-17
//! named as its floor (P-117 proved the M1 holders honour the flag; THIS proves the derived stores).
//!
//! ## What this PROVES (the GA-D7 conjunction, OBSERVED across all five derived stores)
//! 1. **0 processing of a restricted subject across Search/Refs/Notif/Agents/OLAP.** With the
//!    restriction SET, every derived store's processing chokepoint (index / edge-projection /
//!    notification / agent-read / **OLAP analytics, contract 11.6**) reads
//!    [`DerivedProcessed::Suppressed`] — the fan-out's [`RestrictFanOutOutcome::processed_count`] is
//!    **0** and [`RestrictFanOutOutcome::all_suppressed`] holds.
//! 2. **Storage RETAINED while restricted (§4.4 "while retaining storage").** Every derived row is
//!    still present after the restriction — a restriction suppresses PROCESSING, never the row
//!    ([`RestrictFanOutOutcome::all_rows_retained`]).
//! 3. **Reversible.** Clearing the restriction resumes processing across all five derived stores.
//! 4. **One shared flag (the §4.4 "every holder honours" property).** The SAME
//!    [`RestrictRegistry`] every M1 holder reads (P-GA-17) is the flag the five derived stores read —
//!    setting it through any one holder suppresses all five (proven in the unit suite; here the
//!    fan-out sets it through all five and the OLAP-specific 11.6 leg is asserted).
//!
//! ## What this REUSES (EI-01 §7 coherence — no second flag, no parallel mechanism)
//! This drill ADDS NO production code beyond `restrict_fanout` — it chains the
//! [`RestrictFanOutDriver`] over the five [`DerivedStore`] models, every one reading the ONE
//! [`RestrictRegistry`] flag shipped in P-GA-17 ([`myelin_gdpr_service::structural_floor`]). The live
//! Search/Refs/Notif/Agent/OLAP bindings behind the [`myelin_gdpr::PersonalDataHolder`] seam are the
//! named floor; each reads the SAME flag at its processing chokepoint — the binding is a config swap
//! at boot, never a code change. This drill touches NO new DB/object-store/cache/bus contract (it
//! composes the already-shipped in-memory [`RestrictRegistry`] seam), so **no `--features
//! integration` live-stack leg is owed** by P-GA-25.

use myelin_gdpr::{PersonalDataHolder, SubjectRef, TenantId};
use myelin_gdpr_service::{
    restrict_holder_ids, DerivedProcessed, DerivedProcessing, DerivedStore, DerivedStoreHolder,
    RestrictFanOutDriver, RestrictRegistry,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

fn t(s: &str) -> TenantId {
    TenantId::from_token(s)
}

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        t("acme"),
    ))
}

/// The GA-D7 drill: `restrict` honoured END-TO-END across the FIVE M2 derived stores — 0 processing
/// of a restricted subject, storage retained, reversible.
#[test]
fn restrict_is_honoured_end_to_end_across_the_five_derived_stores() {
    let tenant = t("acme");
    let subj = subject("u-d7");

    // ONE shared restrict registry across the derived-store SET — the SAME flag every M1 holder reads
    // (P-GA-17). A single restriction suppresses EVERY derived store (§4.4 "every holder honours").
    let restrict = RestrictRegistry::new();
    let search = DerivedStore::new(DerivedProcessing::SearchIndex, &restrict);
    let refs = DerivedStore::new(DerivedProcessing::RefsProject, &restrict);
    let notif = DerivedStore::new(DerivedProcessing::NotifNotify, &restrict);
    let agents = DerivedStore::new(DerivedProcessing::AgentRead, &restrict);
    let olap = DerivedStore::new(DerivedProcessing::OlapAnalyse, &restrict);
    let stores: [&DerivedStore; 5] = [&search, &refs, &notif, &agents, &olap];

    // The subject's derived rows live across all five derived stores (the live consumers projected
    // them). They are RETAINED across a restriction (a restriction is reversible, not an erase).
    for s in stores {
        s.seed_row(&subj, &tenant);
    }

    // ─────── PRE: every derived store processes the subject (unrestricted) ───────
    for s in stores {
        assert!(
            matches!(s.process(&subj, &tenant), DerivedProcessed::Processed(_)),
            "{} processes the subject before restriction",
            s.holder_id()
        );
    }

    // The holders the orchestrator drives the restriction through (the no-cross-store-read seam).
    let sh = DerivedStoreHolder::new(&search);
    let rh = DerivedStoreHolder::new(&refs);
    let nh = DerivedStoreHolder::new(&notif);
    let ah = DerivedStoreHolder::new(&agents);
    let oh = DerivedStoreHolder::new(&olap);
    let holders: [&dyn PersonalDataHolder; 5] = [&sh, &rh, &nh, &ah, &oh];

    // ─────── GA-D7: SET the restriction → 0 processing across all five, storage retained ───────
    let set = RestrictFanOutDriver::fan_out_restrict(&subj, &tenant, true, &stores, &holders)
        .expect("the restrict fan-out succeeds");

    // 0 PROCESSING of a restricted subject (the GA-D7 number) — every derived store Suppressed.
    assert_eq!(
        set.processed_count(),
        0,
        "0 processing of a restricted subject across Search/Refs/Notif/Agents/OLAP (GA-D7)"
    );
    assert!(
        set.all_suppressed(),
        "every derived store SUPPRESSED the restricted subject"
    );
    assert_eq!(
        set.verdicts.len(),
        5,
        "Search + Refs + Notif + Agents + OLAP"
    );

    // No-indexing / no-agent-use / no-analytics / no-notification — assert per derived store.
    assert_eq!(
        search.process(&subj, &tenant),
        DerivedProcessed::Suppressed,
        "Search: no indexing"
    );
    assert_eq!(
        refs.process(&subj, &tenant),
        DerivedProcessed::Suppressed,
        "Refs: no edge projection"
    );
    assert_eq!(
        notif.process(&subj, &tenant),
        DerivedProcessed::Suppressed,
        "Notif: no notification"
    );
    assert_eq!(
        agents.process(&subj, &tenant),
        DerivedProcessed::Suppressed,
        "Agents: no agent-use"
    );
    assert_eq!(
        olap.process(&subj, &tenant),
        DerivedProcessed::Suppressed,
        "OLAP: no analytics (11.6)"
    );

    // Storage RETAINED while restricted (§4.4 "while retaining storage") — suppression ≠ delete.
    assert!(
        set.all_rows_retained(),
        "every derived row RETAINED while restricted (§4.4)"
    );
    for s in stores {
        assert!(
            s.has_row(&subj, &tenant),
            "{} retains its derived row while restricted",
            s.holder_id()
        );
    }

    // The OLAP read store is explicitly in the suppression set (contract 11.6 / GA-9 — the §8
    // restriction-flag-into-OLAP propagation; "no analytics for a restricted subject").
    assert!(
        set.verdicts
            .iter()
            .any(|v| v.holder_id == restrict_holder_ids::OLAP_READ_STORE
                && matches!(v.outcome, DerivedProcessed::Suppressed)),
        "OLAP honours the restriction flag — no analytics for a restricted subject (11.6)"
    );

    // ─────── REVERSIBLE: CLEAR the restriction → processing resumes across all five ───────
    let clear = RestrictFanOutDriver::fan_out_restrict(&subj, &tenant, false, &stores, &holders)
        .expect("the un-restrict fan-out succeeds");
    assert!(
        !clear.all_suppressed(),
        "processing resumes after the restriction is lifted (reversible)"
    );
    let processed_again = clear
        .verdicts
        .iter()
        .filter(|v| matches!(v.outcome, DerivedProcessed::Processed(_)))
        .count();
    assert_eq!(
        processed_again, 5,
        "all five derived stores process again (reversible)"
    );
    for s in stores {
        assert!(
            matches!(s.process(&subj, &tenant), DerivedProcessed::Processed(_)),
            "{} processes again after un-restrict",
            s.holder_id()
        );
    }
}

/// **GA-D7 isolation: the restriction is per-`(tenant, subject)` — a restricted subject's derived
/// rows are suppressed, an UNRELATED subject's are still processed across all five derived stores.**
/// Proves the suppression is scoped (no over-suppression), the load-bearing precision of the flag.
#[test]
fn the_restriction_is_scoped_other_subjects_still_process_across_the_derived_stores() {
    let tenant = t("acme");
    let restricted = subject("u-restricted");
    let other = subject("u-other");

    let restrict = RestrictRegistry::new();
    let search = DerivedStore::new(DerivedProcessing::SearchIndex, &restrict);
    let olap = DerivedStore::new(DerivedProcessing::OlapAnalyse, &restrict);
    for s in [&search, &olap] {
        s.seed_row(&restricted, &tenant);
        s.seed_row(&other, &tenant);
    }

    // Restrict ONLY `u-restricted`.
    restrict.set(&restricted, &tenant, true);

    // The restricted subject is suppressed across the derived stores.
    assert_eq!(
        search.process(&restricted, &tenant),
        DerivedProcessed::Suppressed
    );
    assert_eq!(
        olap.process(&restricted, &tenant),
        DerivedProcessed::Suppressed
    );
    // The UNRELATED subject still processes (the restriction did not over-suppress).
    assert!(matches!(
        search.process(&other, &tenant),
        DerivedProcessed::Processed(_)
    ));
    assert!(matches!(
        olap.process(&other, &tenant),
        DerivedProcessed::Processed(_)
    ));
}
