//! # CDC 10.9 — the structural erasure floor proven on the M1 stores (P-GA-17 → P-117)
//!
//! **Contract:** index row 10.9 (the structural floor of the ONE free-text/immutable erasure
//! posture — §7.1 the three levers) joined with rows 4.8 (the pseudonym lever), 11.4 (per-subject
//! DEK), 10.1 (the holders that honour `restrict`). P-GA-16's CDC (`cdc_10_9_erasure_posture.rs`)
//! proved the posture is the ONE source by reference; P-GA-17 **adds the
//! `restrict`-honoured-by-an-M1-holder pair** — the prompt's explicit "reuse the 10.1/10.9 CDC
//! pairs; add the `restrict`-honoured-by-M1-holder pair".
//!
//! - **provider** = an M1-store holder ([`M1Store`]) that HONOURS the structural floor: it crypto-
//!   shreds a subject's self-authored free-text to [`StoredContent::Unrecoverable`] (lever 1), it
//!   honours the [`RestrictRegistry`] flag by SUPPRESSING processing while RETAINING storage (lever
//!   3), and the pseudonym-map shred ([`shred_pseudonym_identity`]) leaves only the frozen
//!   `<pseudonym>@<tenant>.noreply` form (lever 2).
//! - **consumer** = the GDPR posture/orchestrator side that SETS the restriction + DRIVES the erase
//!   and OBSERVES the floor honoured through the holder contract (never reaching into the store —
//!   the no-cross-store-read law).
//!
//! The dated green artifact: a restricted subject's processing is suppressed (every op) while their
//! storage is retained; clearing the flag resumes processing (reversible); an erase renders the
//! self-authored content unrecoverable; the residual (a third-party mention under the author's DEK)
//! is restrict-suppressed, never crypto-shredded by the subject's key. If the §7.1 floor shape
//! drifts, this stops compiling/passing — that is the contract.

use myelin_gdpr::{SubjectRef, TenantId};
use myelin_gdpr_service::{
    classify_residual, shred_pseudonym_identity, Authorship, CryptoShredKms, InMemoryShredKms,
    LeverCoverage, M1Store, Processed, Processing, RestrictRegistry, StoredContent,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, PseudonymHandle};

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId::from_token("acme"),
    ))
}

/// **The `restrict`-honoured-by-M1-holder pair (10.9 §7.1 lever 3 + 10.1).** The provider is an M1
/// holder; the consumer SETS the restriction and observes every processing op SUPPRESSED while
/// storage is RETAINED — reversibly. This is the pair P-GA-17 adds.
#[test]
fn restrict_is_honoured_by_an_m1_holder_suppressing_processing_retaining_storage() {
    let tenant = TenantId::from_token("acme");
    let subj = subject("u-cdc-restrict");
    let restrict = RestrictRegistry::new();
    let kms = InMemoryShredKms::new();
    let store = M1Store::new("search_index", &restrict, &kms);
    kms.provision(M1Store::dek_handle(&subj, &tenant), 1);
    store.store_self_authored(&subj, &tenant, "indexed body");

    // unrestricted ⇒ processed.
    assert!(matches!(store.index(&subj, &tenant), Processed::Processed(_)));

    // The consumer SETS the restriction; the holder HONOURS it across all four §4.4 ops.
    restrict.set(&subj, &tenant, true);
    for op in Processing::all() {
        let r = match op {
            Processing::Index => store.index(&subj, &tenant),
            Processing::AgentRead => store.agent_read(&subj, &tenant),
            Processing::Analyse => store.analyse(&subj, &tenant),
            Processing::Notify => store.notify(&subj, &tenant),
        };
        assert_eq!(r, Processed::Suppressed, "{op:?} suppressed for a restricted subject");
    }
    // Storage retained (the holder did not delete the content).
    assert_eq!(
        store.fetch_stored(&subj, &tenant),
        Some(StoredContent::Recoverable("indexed body".into())),
        "storage is RETAINED while restricted (§4.4)"
    );
    // Reversible.
    restrict.set(&subj, &tenant, false);
    assert!(matches!(store.index(&subj, &tenant), Processed::Processed(_)));
}

/// **Lever 1 (11.4): per-subject DEK crypto-shred renders self-authored free-text unrecoverable.**
#[test]
fn per_subject_dek_shred_renders_self_authored_free_text_unrecoverable() {
    let tenant = TenantId::from_token("acme");
    let subj = subject("u-cdc-erase");
    let restrict = RestrictRegistry::new();
    let kms = InMemoryShredKms::new();
    let store = M1Store::new("chat_store", &restrict, &kms);
    kms.provision(M1Store::dek_handle(&subj, &tenant), 5);
    store.store_self_authored(&subj, &tenant, "message body");

    assert_eq!(store.erase_self_authored(&subj, &tenant), Some(5));
    assert_eq!(
        store.fetch_stored(&subj, &tenant),
        Some(StoredContent::Unrecoverable),
        "the per-subject DEK shred renders the self-authored content unrecoverable (§7.1.1)"
    );
    assert_eq!(kms.recoverable_in_backup(&M1Store::dek_handle(&subj, &tenant)), 0);
}

/// **Lever 2 (4.8): pseudonym-map shred leaves only `<pseudonym>@<tenant>.noreply`.**
#[test]
fn pseudonym_map_shred_leaves_only_the_frozen_grammar() {
    let handle = PseudonymHandle::new("anon-42", "acme").expect("valid pseudonym");
    let shredded = shred_pseudonym_identity(&handle);
    assert_eq!(shredded.immutable_bytes, "anon-42@acme.noreply");
    assert!(shredded.holds_only_the_pseudonym_form());
}

/// **The residual (§7.2 documented limit): a third-party mention is restrict-suppress-only, NEVER
/// crypto-shredded by the subject's key.** The consumer classifies the residual and confirms the
/// documented limit — this is the X-7 anti-pattern guard (never pretend the residual is solved).
#[test]
fn the_residual_is_restrict_suppress_only_not_crypto_shredded() {
    assert_eq!(classify_residual(Authorship::SelfAuthored), LeverCoverage::CryptoShred);
    assert_eq!(
        classify_residual(Authorship::ThirdPartyMention),
        LeverCoverage::RestrictSuppressOnly,
        "the third-party residual is governed ONLY by restrict (the documented limit), not the \
         subject's crypto-shred (§7.2)"
    );
}
