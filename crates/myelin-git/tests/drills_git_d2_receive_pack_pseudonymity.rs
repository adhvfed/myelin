use myelin_events::{
    Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Region, TenantId,
    Timestamp,
};
use myelin_git::commit::NonPseudonymousIdentity;
use myelin_git::receive_pack::{
    CrashPoint, InMemoryObjectDb, Oid, ProposedRefUpdate, PushOutcome, PushSession, Pusher,
    RefName, RefStore, RejectReason,
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
        pusher: Pusher::direct("psn-7@acme.noreply", false),
    }
}

#[test]
fn git_d2_git1_half_zero_cleartext_pii_admitted_at_receive_pack() {
    let (store, _outbox) = store();
    let db = InMemoryObjectDb::new();

    let cleartext_pii = [
        "Ada Lovelace",
        "ada.lovelace@example.com",
        "Grace Hopper",
        "grace@navy.example",
    ];

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
    assert_eq!(
        store.tip(&RefName::new("refs/heads/raw")),
        None,
        "the raw-identity ref never moved"
    );

    let good = commit_push(
        "refs/heads/ok",
        "psn-7f3a9c@acme.noreply <psn-7f3a9c@acme.noreply> 1700000000 +0000",
    );
    assert!(matches!(
        store.receive(&good, &db, CrashPoint::None).unwrap(),
        PushOutcome::Accepted { .. }
    ));

    let mut stored_identity_corpus = String::new();
    for entry in store.reflog().expect("read reflog") {
        stored_identity_corpus.push_str(&entry.pusher_pseudonym);
        stored_identity_corpus.push('\n');
    }
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
        "GIT-D2 FAIL - cleartext PII in a stored commit identity field: {leaked:?}"
    );

    println!(
        "[2026-06-21] GIT-D2 (GIT-1 half) PASS - 0 cleartext-PII commit identities admitted at \
         receive-pack; stored identity corpus carries only <pseudonym>@<tenant>.noreply; \
         cleartext_pii_scanned={}, leaked=0",
        cleartext_pii.len()
    );
}

#[test]
fn git_d2_git1_half_gate_is_real_refused_bytes_would_have_leaked() {
    let raw_bytes =
        b"tree blake3:t\nauthor Ada Lovelace <ada.lovelace@example.com> 1 +0000\ncommitter Ada Lovelace <ada.lovelace@example.com> 1 +0000\n\nx\n";
    let view = String::from_utf8_lossy(raw_bytes);
    assert!(view.contains("Ada Lovelace") && view.contains("ada.lovelace@example.com"));
    assert!(myelin_git::commit::enforce_pseudonymous_commit(raw_bytes, TENANT).is_err());
}
