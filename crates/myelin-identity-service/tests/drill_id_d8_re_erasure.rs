//! # ID-D8 (F3) — restore-resurrects-no-authority + post-restore re-erasure (P-ID-20 / P-078)
//!
//! **Drill catalogue row ID-D8** (`testing-strategy/01` §4.2): *"Restore to a consistent point → no
//! resurrected grants past an erasure; post-restore re-erasure runs."* — artifact: **re-erasure
//! receipt**; cadence: SCHED. Rides STOR-D1/D2 (the silent-data-loss floor, the permanent restore-
//! verify gate; that gate is Storage-owned, P-061/P-100 — this drill is the IDENTITY half it drives).
//!
//! The quantified thresholds (architecture §12; EI-01 §3 prove-it):
//! - **0 resurrected grants past an erasure** — after the restore + re-erasure pass, no erased
//!   subject's real identity is recoverable AND the principal stays disabled;
//! - **0 recoverable PII for an erased subject post-restore** — `resolve_subject` is `None`;
//! - **a dated re-erasure receipt is emitted** — the green artifact (observability is part of the
//!   pass: a system that survives but emits no signal has FAILED the drill).
//!
//! **The scenario (the harness models a restore as a re-materialisation of pre-erasure state):**
//! 1. seed two subjects' pseudonym mappings + erase them (the per-subject DEK + row crypto-shredded,
//!    the erasure recorded in the PII-free ledger 10.8);
//! 2. **restore an OLDER (pre-erasure) backup** — modelled by re-`put_mapping` (which re-provisions
//!    the per-subject DEK and re-seals the link) — the subjects are RESURRECTED (resolvable again);
//! 3. **post-restore re-erasure runs** (`re_erase_after_restore`) — replays the ledger, re-runs the
//!    IDENTICAL crypto-shred → the subjects are erased AGAIN;
//! 4. assert: **0 resurrected** after the pass, a dated receipt, the ledger drove the re-erasure.

use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, PseudonymHandle, RevokeTarget};
use myelin_identity_service::{StoreBackedCheck, TupleStore};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

fn slot() -> StoreBackedCheck {
    StoreBackedCheck::new(TupleStore::new(OutboxStore::new()))
}

fn scope(tenant: &str) -> TenantScope {
    let p = Principal::stub(
        PrincipalId("admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    TenantScope::from_verified_token(&p, Region("eu-west".into()))
}

fn handle(p: &str, t: &str) -> PseudonymHandle {
    PseudonymHandle::new(p, t).expect("a well-formed handle")
}

fn at(t: &str) -> Timestamp {
    Timestamp(t.into())
}

/// **ID-D8: a restore resurrects nothing — post-restore re-erasure drives every ledger-recorded
/// erasure back to 0 recoverable, and emits a dated re-erasure receipt.**
#[test]
fn id_d8_restore_resurrects_no_authority_and_emits_a_dated_re_erasure_receipt() {
    let slot = slot();
    let s = scope("acme");
    let alice = PrincipalId("p:alice".into());
    let bob = PrincipalId("p:bob".into());

    // (1) Seed + erase both subjects.
    slot.pseudonyms()
        .put_mapping(&s, &alice, handle("anon-a", "acme"))
        .unwrap();
    slot.pseudonyms()
        .put_mapping(&s, &bob, handle("anon-b", "acme"))
        .unwrap();
    slot.erase_in(&s, &alice, at("2026-06-19T10:00:00Z"));
    slot.erase_in(&s, &bob, at("2026-06-19T10:00:01Z"));

    // Both are erased: unrecoverable, recorded in the PII-free ledger.
    assert!(slot.pseudonyms().resolve_subject(&s, &alice).is_none());
    assert!(slot.pseudonyms().resolve_subject(&s, &bob).is_none());
    assert!(slot.erasure_ledger().is_erased(&s, &alice));
    assert!(slot.erasure_ledger().is_erased(&s, &bob));

    // (2) RESTORE an older (pre-erasure) backup — modelled by re-materialising the mappings (the
    // restore brings back the per-subject DEK + the sealed link). This RESURRECTS the subjects.
    slot.pseudonyms()
        .put_mapping(&s, &alice, handle("anon-a", "acme"))
        .unwrap();
    slot.pseudonyms()
        .put_mapping(&s, &bob, handle("anon-b", "acme"))
        .unwrap();
    assert!(
        slot.pseudonyms().resolve_subject(&s, &alice).is_some(),
        "the restore RESURRECTED alice (resolvable again) — the bug ID-D8 catches"
    );
    assert!(
        slot.pseudonyms().resolve_subject(&s, &bob).is_some(),
        "the restore resurrected bob"
    );

    // (3) POST-RESTORE RE-ERASURE runs — replays the ledger, re-runs the IDENTICAL crypto-shred.
    let receipt = slot.re_erase_after_restore(&s, at("2026-06-19T11:00:00Z"));

    // (4) The quantified thresholds:
    assert_eq!(
        receipt.re_erased, 2,
        "the ledger drove re-erasure of BOTH recorded subjects"
    );
    assert_eq!(
        receipt.pre_pass_resurrected, 2,
        "the restore resurrected both (the honest 'what the backup brought back' signal)"
    );
    assert_eq!(
        receipt.resurrected, 0,
        "0 resurrected AFTER the re-erasure pass — the ID-D8 threshold (a restore resurrects nothing)"
    );
    assert!(receipt.is_green(), "the drill is GREEN: 0 resurrected");
    // A dated re-erasure receipt (the green artifact — observability is part of the pass).
    assert_eq!(receipt.ran_at, at("2026-06-19T11:00:00Z"), "dated");
    assert!(receipt.summary().contains("GREEN"));
    assert!(receipt.summary().contains("2026-06-19T11:00:00Z"));

    // 0 recoverable PII post-restore: neither subject resolves; both principals stay disabled.
    assert!(
        slot.pseudonyms().resolve_subject(&s, &alice).is_none(),
        "alice re-erased"
    );
    assert!(
        slot.pseudonyms().resolve_subject(&s, &bob).is_none(),
        "bob re-erased"
    );
    let nowt = at("2026-06-19T11:00:00Z");
    assert!(
        slot.revocations()
            .is_revoked(&s, &RevokeTarget::Principal(alice), &nowt),
        "alice's grants stay revoked (no resurrected authority)"
    );
    assert!(
        slot.revocations()
            .is_revoked(&s, &RevokeTarget::Principal(bob), &nowt),
        "bob's grants stay revoked"
    );

    // Each per-subject re-erase produced a dated, content-addressed receipt.
    assert_eq!(receipt.per_subject.len(), 2);
    for r in &receipt.per_subject {
        assert!(
            r.content_hash.starts_with("blake3:"),
            "content-addressed receipt"
        );
        assert_eq!(r.erased_at, at("2026-06-19T11:00:00Z"), "dated");
    }
}

/// **The re-erasure pass over an empty ledger is trivially green (0 erased ⇒ 0 resurrected).** A
/// cell with no erasures emits a green, dated receipt with `re_erased=0` (the gate never spuriously
/// reds on a clean cell).
#[test]
fn id_d8_empty_ledger_is_trivially_green() {
    let slot = slot();
    let s = scope("acme");
    let receipt = slot.re_erase_after_restore(&s, at("2026-06-19T11:00:00Z"));
    assert_eq!(receipt.re_erased, 0);
    assert_eq!(receipt.resurrected, 0);
    assert!(receipt.is_green());
}

/// **Mutation-floor (mandatory-core): the no-resurrection path is what makes the drill green.** If
/// re-erasure did NOT run, a restored backup leaves the subject resolvable — this test pins that the
/// resurrection is real BEFORE the pass (so a mutation skipping the pass would leave it resurrected,
/// reding the drill above). Here we prove: WITHOUT calling `re_erase_after_restore`, the restored
/// subject IS resolvable (the failure the pass must repair).
#[test]
fn id_d8_without_re_erasure_a_restore_resurrects_the_subject() {
    let slot = slot();
    let s = scope("acme");
    let alice = PrincipalId("p:alice".into());
    slot.pseudonyms()
        .put_mapping(&s, &alice, handle("anon-a", "acme"))
        .unwrap();
    slot.erase_in(&s, &alice, at("2026-06-19T10:00:00Z"));
    // RESTORE without re-erasure.
    slot.pseudonyms()
        .put_mapping(&s, &alice, handle("anon-a", "acme"))
        .unwrap();
    assert!(
        slot.pseudonyms().resolve_subject(&s, &alice).is_some(),
        "WITHOUT re-erasure, the restore resurrects the subject — the property re_erase_after_restore \
         exists to repair (a mutation skipping the pass leaves this resurrected, reding ID-D8)"
    );
}
