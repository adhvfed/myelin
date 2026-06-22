//! # CDC 11.6 (+ 10.1 derived-store faces) — `restrict` suppression into the derived stores
//! (P-GA-25 → P-152)
//!
//! **Contracts:** index row **11.6** (the OLAP read store **honours the restriction flag** — no
//! analytics for a restricted subject) + the Search/Refs/Notif/Agent restriction faces of **10.1**
//! (every holder honours `restrict` — no indexing / agent-use / notification for a restricted
//! subject, §4.4). The `restrict`-into-derived FAN-OUT (the orchestration leg of 10.1) wires the five
//! derived-store holders as the orchestrator's per-holder `restrict` calls and reads back their
//! processing verdicts. This is the consumer-driven contract test the coverage scanner (P-S21) reads
//! both halves of:
//!
//! - **provider** = a DERIVED-store holder ([`DerivedStoreHolder`] over a [`DerivedStore`] of each
//!   §4.4 kind — the faithful M2 store double whose `restrict` sets the shared flag and whose
//!   processing chokepoint HONOURS it) IMPLEMENTING the contract — the store owns its processing op +
//!   honours the flag; GDPR calls it.
//! - **consumer** = the [`RestrictFanOutDriver`] (the DSR orchestrator's `restrict`-fan-out stage)
//!   CALLING the derived holders through the [`PersonalDataHolder`] contract — it NEVER reaches into
//!   a store (the no-cross-store-read law, gdpr §3.1).
//!
//! The dated green artifacts:
//! - **11.6** — the consumer fans `restrict(set)` to OLAP (provider): OLAP suppresses analytics (no
//!   analytics for a restricted subject); `restrict(clear)` resumes it (reversible).
//! - **10.1 (Search/Refs/Notif/Agent faces)** — the consumer fans `restrict` to each derived store
//!   (provider): each suppresses its processing op (index / edge-projection / notification /
//!   agent-read) while retaining the row; reversible.
//!
//! If 11.6's (or the 10.1 `restrict` face's) shape drifts, this stops compiling/passing — that is
//! the contract.

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

/// **11.6 (provider OLAP ⇄ consumer driver): `restrict` ⇒ no analytics for a restricted subject;
/// reversible.** The consumer sets the restriction through the OLAP provider via the contract; the
/// OLAP read store suppresses analytics (the row is retained). Clearing resumes analytics.
#[test]
fn cdc_11_6_olap_honours_the_restriction_flag_no_analytics() {
    let tenant = t("acme");
    let subj = subject("u-cdc-11-6");
    let restrict = RestrictRegistry::new();
    let olap = DerivedStore::new(DerivedProcessing::OlapAnalyse, &restrict);
    olap.seed_row(&subj, &tenant);
    assert_eq!(olap.holder_id(), restrict_holder_ids::OLAP_READ_STORE);

    let provider = DerivedStoreHolder::new(&olap);
    // The CONSUMER calls the provider via `dyn PersonalDataHolder` — never into the store.
    let consumer: &dyn PersonalDataHolder = &provider;

    // Unrestricted: OLAP analyses the subject's rows.
    assert!(matches!(
        olap.process(&subj, &tenant),
        DerivedProcessed::Processed(_)
    ));

    // SET the restriction through the contract ⇒ OLAP suppresses analytics (11.6).
    let set = consumer
        .restrict(&subj, true)
        .expect("OLAP restrict honours 11.6");
    assert_eq!(set.receipt.operation, "restrict");
    assert!(
        set.receipt.content_hash.starts_with("blake3:"),
        "11.6: the restrict receipt is content-addressed"
    );
    assert_eq!(
        olap.process(&subj, &tenant),
        DerivedProcessed::Suppressed,
        "11.6: no analytics for a restricted subject"
    );
    // Storage retained (the OLAP row survives the restriction).
    assert!(
        olap.has_row(&subj, &tenant),
        "11.6: storage retained while restricted"
    );

    // CLEAR the restriction through the contract ⇒ analytics resume (reversible).
    consumer
        .restrict(&subj, false)
        .expect("OLAP un-restrict honours 11.6");
    assert!(
        matches!(olap.process(&subj, &tenant), DerivedProcessed::Processed(_)),
        "11.6: analytics resume (reversible)"
    );
}

/// **10.1 (Search/Refs/Notif/Agent faces): each derived store honours `restrict` through the
/// contract.** The consumer fans the restriction to each derived store provider; each suppresses its
/// processing op while retaining the row; reversible.
#[test]
fn cdc_10_1_search_refs_notif_agent_honour_restrict() {
    let tenant = t("acme");
    let subj = subject("u-cdc-10-1");
    let restrict = RestrictRegistry::new();

    for kind in [
        DerivedProcessing::SearchIndex,
        DerivedProcessing::RefsProject,
        DerivedProcessing::NotifNotify,
        DerivedProcessing::AgentRead,
    ] {
        let store = DerivedStore::new(kind, &restrict);
        store.seed_row(&subj, &tenant);
        let provider = DerivedStoreHolder::new(&store);
        let consumer: &dyn PersonalDataHolder = &provider;

        // Unrestricted: the store processes.
        assert!(
            matches!(
                store.process(&subj, &tenant),
                DerivedProcessed::Processed(_)
            ),
            "{} processes before restriction",
            kind.holder_id()
        );
        // SET via the contract ⇒ suppressed, row retained.
        consumer
            .restrict(&subj, true)
            .expect("derived restrict honours 10.1");
        assert_eq!(
            store.process(&subj, &tenant),
            DerivedProcessed::Suppressed,
            "{}: processing suppressed for the restricted subject (§4.4)",
            kind.holder_id()
        );
        assert!(
            store.has_row(&subj, &tenant),
            "{}: storage retained",
            kind.holder_id()
        );
        // CLEAR via the contract ⇒ resumes (reversible).
        consumer
            .restrict(&subj, false)
            .expect("derived un-restrict honours 10.1");
        assert!(
            matches!(
                store.process(&subj, &tenant),
                DerivedProcessed::Processed(_)
            ),
            "{}: processing resumes (reversible)",
            kind.holder_id()
        );
    }
}

/// **The fan-out leg (consumer driver over the five providers): 0 processing of a restricted subject
/// across all five derived stores, reversible.** The contract is honoured by every derived store at
/// once — the orchestration leg of 10.1 + 11.6.
#[test]
fn cdc_restrict_fan_out_zero_processing_across_all_five() {
    let tenant = t("acme");
    let subj = subject("u-cdc-fan");
    let restrict = RestrictRegistry::new();
    let search = DerivedStore::new(DerivedProcessing::SearchIndex, &restrict);
    let refs = DerivedStore::new(DerivedProcessing::RefsProject, &restrict);
    let notif = DerivedStore::new(DerivedProcessing::NotifNotify, &restrict);
    let agents = DerivedStore::new(DerivedProcessing::AgentRead, &restrict);
    let olap = DerivedStore::new(DerivedProcessing::OlapAnalyse, &restrict);
    let stores: [&DerivedStore; 5] = [&search, &refs, &notif, &agents, &olap];
    for s in stores {
        s.seed_row(&subj, &tenant);
    }
    let sh = DerivedStoreHolder::new(&search);
    let rh = DerivedStoreHolder::new(&refs);
    let nh = DerivedStoreHolder::new(&notif);
    let ah = DerivedStoreHolder::new(&agents);
    let oh = DerivedStoreHolder::new(&olap);
    let holders: [&dyn PersonalDataHolder; 5] = [&sh, &rh, &nh, &ah, &oh];

    let set = RestrictFanOutDriver::fan_out_restrict(&subj, &tenant, true, &stores, &holders)
        .expect("the restrict fan-out honours 10.1 / 11.6");
    assert_eq!(
        set.processed_count(),
        0,
        "0 processing of a restricted subject across all five"
    );
    assert!(set.all_rows_retained(), "every derived row retained (§4.4)");
    assert_eq!(
        set.holder_receipts.len(),
        5,
        "one restrict receipt per derived store provider"
    );

    let clear = RestrictFanOutDriver::fan_out_restrict(&subj, &tenant, false, &stores, &holders)
        .expect("the un-restrict fan-out honours 10.1 / 11.6");
    assert!(!clear.all_suppressed(), "processing resumes (reversible)");
}
