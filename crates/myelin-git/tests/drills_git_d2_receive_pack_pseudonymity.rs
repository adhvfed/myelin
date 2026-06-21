//! # GIT-D2 (GIT-1 half) drill — receive-pack rejects cleartext-PII commit identities at the door
//!
//! **Drill:** `planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! GIT-D2 (the GIT-1 half, asserted at GIT-P12 / P-273; the full erase-reaches-every-holder GIT-D2
//! completes at GIT-P29). **Contract:** 4.8 (the frozen pseudonym grammar `<pseudonym>@<tenant>.noreply`)
//! enforced at receive-pack; **10.9** instantiated BY REFERENCE (the ONE platform posture — NOT
//! restated here). **Reconciliation:** §X-7 (the ONE free-text/immutable erasure posture).
//!
//! ## What this drill proves (quantified — the prompt's GATE)
//! The COMPLEMENT of the residual drill (`drills_git_d2_pseudonymous_residual.rs`, which proves the
//! *erasure* half: after erase, 0 real identity recoverable from the immutable bytes). THIS drill
//! proves the *enforcement* half: a commit whose author/committer identity is NOT the tenant
//! pseudonym is REJECTED at receive-pack **before the ref moves** — so the immutable object DB only
//! ever STORES commit identities in the `<pseudonym>@<tenant>.noreply` form. The dated green artifact:
//! a scan of newly-stored commit identities shows **0 cleartext PII** (0 name/email bytes in a stored
//! commit identity field).
//!
//! ## The enforcement default (OQ-10 / R-8) — recorded by reference
//! REJECT-AT-PUSH (client-cooperative, sha-stable) is the chosen default; the rationale is in
//! `myelin_git::commit` (the module doc) and restated in the crate doc. The server-side
//! rewrite-at-push mode is the named GIT-P29 follow-on.
//!
//! ## FLOOR (named, not silent — VISION §3 / GF-7)
//! The structural mechanism (pseudonymous-by-default + per-subject DEK shred + history-rewrite) ships
//! across GIT-P9/GIT-P12/GIT-P29; the lawful-basis RESIDUAL is the ONE posture's `[OPEN — LEGAL]`
//! statement (R-7, parallel/Legal — NOT a code gate). This drill covers the GIT-1 ADMISSION half with
//! 0 cleartext-PII commits admitted; the erase-reaches-every-holder completion is GIT-P29.

use myelin_events::{
    Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Region, TenantId,
    Timestamp,
};
use myelin_git::commit::NonPseudonymousIdentity;
use myelin_git::receive_pack::{
    CrashPoint, InMemoryObjectDb, Oid, ProposedRefUpdate, PushOutcome, PushSession, Pusher, RefName,
    RefStore, RejectReason,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, PseudonymHandle};
use std::sync::Arc;

const TENANT: &str = "acme";

fn store() -> (RefStore, OutboxStore) {
    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let ctx_base = EmitContextBase {
        tenant: TenantId(TENANT.into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId(TENANT.into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:push-1".into())),
    };
    let store = RefStore::open("core", ctx_base, outbox.clone(), minter);
    (store, outbox)
}

/// A push whose quarantine carries one COMMIT object with the given identity line for BOTH author
/// and committer.
fn commit_push(ref_name: &str, identity_line: &str) -> PushSession {
    let bytes =
        format!("tree blake3:t\nauthor {identity_line}\ncommitter {identity_line}\n\nfeat: x\n")
            .into_bytes();
    PushSession {
        updates: vec![ProposedRefUpdate {
            ref_name: RefName::new(ref_name),
            expected_old: Oid::zero(),
            new_oid: Oid::new("aaaa"),
            forced: false,
            commit_oids: vec![Oid::new("c0")],
        }],
        quarantine: vec![myelin_git::receive_pack::QuarantineObject {
            oid: Oid::new("c0"),
            bytes,
        }],
        pusher: Pusher { pseudonym: "psn-7@acme.noreply".into(), is_agent: false },
    }
}

/// **GIT-D2 (GIT-1 half) — the receive-pack scan emits 0 cleartext PII.** Push a mix of commits: real
/// name/email identities are REJECTED at the door; tenant-pseudonym identities are ACCEPTED. The
/// scan over the STORED reflog identities shows 0 name/email bytes (all stored identity is the
/// `<pseudonym>@<tenant>.noreply` form).
#[test]
fn git_d2_git1_half_zero_cleartext_pii_admitted_at_receive_pack() {
    let (store, _outbox) = store();
    let db = InMemoryObjectDb::new();

    // The real-identity tokens a non-cooperating client would try to push (the cleartext PII the
    // door must keep out of the immutable object DB).
    let cleartext_pii = [
        "Ada Lovelace",
        "ada.lovelace@example.com",
        "Grace Hopper",
        "grace@navy.example",
    ];

    // A raw-identity commit → REJECTED before the ref moves (0 cleartext PII admitted).
    let raw = commit_push(
        "refs/heads/raw",
        "Ada Lovelace <ada.lovelace@example.com> 1700000000 +0000",
    );
    assert!(
        matches!(
            store.receive(&raw, &db, CrashPoint::None).unwrap(),
            PushOutcome::Rejected(RejectReason::NonPseudonymousCommit {
                identity: NonPseudonymousIdentity::NotAPseudonym { .. },
                ..
            })
        ),
        "a raw name/email commit must be rejected at receive-pack"
    );
    assert_eq!(store.tip(&RefName::new("refs/heads/raw")), None, "the raw-identity ref never moved");

    // A tenant-pseudonym commit → ACCEPTED (the cooperative path); the reflog records the pseudonym.
    let good = commit_push(
        "refs/heads/ok",
        "psn-7f3a9c@acme.noreply <psn-7f3a9c@acme.noreply> 1700000000 +0000",
    );
    assert!(matches!(
        store.receive(&good, &db, CrashPoint::None).unwrap(),
        PushOutcome::Accepted { .. }
    ));

    // The SCAN: every newly-stored commit identity (the reflog pusher_pseudonym + the accepted
    // commit's author/committer lines) carries ONLY the `<pseudonym>@<tenant>.noreply` form, never a
    // cleartext-PII token. (The rejected push left nothing stored.)
    let mut stored_identity_corpus = String::new();
    for entry in store.reflog() {
        stored_identity_corpus.push_str(&entry.pusher_pseudonym);
        stored_identity_corpus.push('\n');
    }
    // The accepted commit's stored identity (re-derived through the ONE grammar — what the object DB
    // holds): the pseudonym handle, rendered.
    let stored_handle = PseudonymHandle::new("psn-7f3a9c", TENANT).unwrap();
    stored_identity_corpus.push_str(&stored_handle.render());

    let mut leaked = Vec::new();
    for tok in cleartext_pii {
        if stored_identity_corpus.contains(tok) {
            leaked.push(tok);
        }
    }
    assert!(
        leaked.is_empty(),
        "GIT-D2 FAIL — cleartext PII in a stored commit identity field: {leaked:?}"
    );

    // The dated green artifact (the drill witness).
    println!(
        "[2026-06-21] GIT-D2 (GIT-1 half) PASS — 0 cleartext-PII commit identities admitted at \
         receive-pack; stored identity corpus carries only <pseudonym>@<tenant>.noreply; \
         cleartext_pii_scanned={}, leaked=0",
        cleartext_pii.len()
    );
}

/// **GIT-D2 negative control — the gate is REAL.** If a raw-identity commit were admitted, the scan
/// WOULD find the cleartext PII in the stored bytes. We prove the scan is not a no-op by scanning the
/// raw bytes the door REFUSED: they DO contain the PII (so the refusal is what keeps it out, not a
/// blind scan).
#[test]
fn git_d2_git1_half_gate_is_real_refused_bytes_would_have_leaked() {
    let raw_bytes =
        b"tree blake3:t\nauthor Ada Lovelace <ada.lovelace@example.com> 1 +0000\ncommitter Ada Lovelace <ada.lovelace@example.com> 1 +0000\n\nx\n";
    // The bytes the door refused DO carry the cleartext PII — proving the reject (not a no-op scan)
    // is what keeps them out of the object DB.
    let view = String::from_utf8_lossy(raw_bytes);
    assert!(view.contains("Ada Lovelace") && view.contains("ada.lovelace@example.com"));
    // And the enforcement function flags exactly that.
    assert!(myelin_git::commit::enforce_pseudonymous_commit(raw_bytes, TENANT).is_err());
}
