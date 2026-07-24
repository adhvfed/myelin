//! # GT-003 — the cross-system reconciler: crash-window recovery proof (builder ≠ verifier oracle).
//!
//! Drives the REAL durable push path ([`myelin_git::receive_pack::RefStore::open_durable`]) into the
//! apply-after-outbox-commit crash window ([`CrashPoint::AfterCommitBeforeApply`]) and proves the
//! reconciler ([`myelin_git::reconcile`]) recovers the on-disk ref to the committed `update_seq`,
//! idempotently, with `git fsck` clean afterward (the external oracle).

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EmitContextBase,
    EventEnvelope, EventId, EventType, IdMinter, InProcessBus, MonotonicMinter, OutboxRow,
    OutboxStore, Region, Relay, TenantId, Timestamp, Visibility, MAX_PUBLISH_ATTEMPTS,
};
use myelin_git::core::RepoLoc;
use myelin_git::durable::{DurableGitRepo, DurableGitStore};
use myelin_git::receive_pack::{
    CrashPoint, InMemoryObjectDb, Oid, ProposedRefUpdate, PushOutcome, PushSession, Pusher,
    RefName, RefStore,
};
use myelin_git::reconcile::{
    reconcile_refs, refs_by_repo_from_outbox_scoped_bounded, refs_from_outbox_scoped_bounded,
    repo_slugs_from_outbox_scoped_bounded,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

const SNAPSHOT_MAX_ROWS: usize = 100;
const SNAPSHOT_MAX_BYTES: usize = 1024 * 1024;

fn temp_root(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    p.push(format!("myelin-gt003-{tag}-{nanos}"));
    p
}

fn ctx_base(tenant: &str) -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId(tenant.into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-29T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-29T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:gt003".into())),
    }
}

fn seed_commit(repo: &DurableGitRepo, content: &[u8]) -> Oid {
    let blob = repo.write_blob(content).expect("blob");
    let tree = repo.write_tree(&[("file.txt", &blob)]).expect("tree");
    Oid::new(
        repo.write_commit(
            &tree,
            &[],
            "feat: seed",
            "psn@acme.noreply",
            "psn@acme.noreply",
        )
        .expect("commit")
        .0,
    )
}

fn push_create(ref_name: &str, new: &Oid) -> PushSession {
    PushSession {
        updates: vec![ProposedRefUpdate {
            ref_name: RefName::new(ref_name),
            expected_old: Oid::zero(),
            new_oid: new.clone(),
            forced: false,
            commit_oids: vec![new.clone()],
        }],
        quarantine: vec![],
        pusher: Pusher {
            pseudonym: "psn@acme.noreply".into(),
            is_agent: false,
        },
    }
}

fn git_fsck(repo_path: &std::path::Path) -> (bool, String) {
    match Command::new("git")
        .arg("--git-dir")
        .arg(repo_path)
        .args(["fsck", "--full", "--strict"])
        .output()
    {
        Ok(out) => (
            out.status.success(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        ),
        Err(e) => (false, format!("git not runnable: {e}")),
    }
}

fn retained_ref_witness(id: &str, payload: serde_json::Value) -> OutboxRow {
    let event_id = EventId(id.into());
    let aggregate = AggregateKey("ref:core:refs%2Fheads%2Fmain".into());
    let subject = ArtifactRef("myelin://acme/git/ref/core:refs%2Fheads%2Fmain".into());
    OutboxRow {
        event_id: event_id.clone(),
        aggregate: aggregate.clone(),
        seq: 1,
        subject: subject.clone(),
        envelope: EventEnvelope {
            event_id,
            type_: EventType("git.ref.updated".into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: ctx_base("acme").actor,
            subject,
            aggregate,
            causation_id: None,
            correlation_id: CorrelationId(format!("corr-{id}")),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Processor,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-29T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-29T00:00:01Z".into()),
            payload,
        },
        published_at: None,
        attempts: 0,
    }
}

#[test]
fn malformed_scoped_retained_witnesses_fail_discovery_and_replay_loudly() {
    let discovery = OutboxStore::new();
    discovery.restore_committed_row_for_test(retained_ref_witness(
        "malformed-discovery",
        serde_json::json!({
            "repo":"core", "ref":"refs/heads/main", "old_oid":"0000",
            "new_oid":"aaaa", "pusher_pseudonym":"git-event:pusher"
        }),
    ));
    assert!(repo_slugs_from_outbox_scoped_bounded(
        &discovery,
        "acme",
        "fr-par",
        SNAPSHOT_MAX_ROWS,
        SNAPSHOT_MAX_BYTES,
    )
    .is_err());

    let replay = OutboxStore::new();
    replay.restore_committed_row_for_test(retained_ref_witness(
        "valid-other",
        serde_json::json!({
            "repo":"other", "ref":"refs/heads/main", "old_oid":"0000",
            "new_oid":"bbbb", "update_seq":1, "pusher_pseudonym":"git-event:pusher"
        }),
    ));
    let grouped = refs_by_repo_from_outbox_scoped_bounded(
        &replay,
        "acme",
        "fr-par",
        SNAPSHOT_MAX_ROWS,
        SNAPSHOT_MAX_BYTES,
    )
    .unwrap();
    assert_eq!(
        grouped.keys().map(String::as_str).collect::<Vec<_>>(),
        ["other"]
    );
    assert_eq!(grouped["other"].len(), 1);
    assert!(
        refs_from_outbox_scoped_bounded(
            &replay,
            "acme",
            "fr-par",
            "core",
            SNAPSHOT_MAX_ROWS,
            SNAPSHOT_MAX_BYTES,
        )
        .unwrap()
        .is_empty(),
        "a fully valid different-repository witness is the only skippable payload"
    );
    replay.restore_committed_row_for_test(retained_ref_witness(
        "malformed-replay",
        serde_json::json!({
            "repo":"core", "ref":"", "old_oid":"0000", "new_oid":"aaaa",
            "update_seq":1, "pusher_pseudonym":"git-event:pusher"
        }),
    ));
    assert!(refs_from_outbox_scoped_bounded(
        &replay,
        "acme",
        "fr-par",
        "core",
        SNAPSHOT_MAX_ROWS,
        SNAPSHOT_MAX_BYTES,
    )
    .is_err());
}

/// **THE CRASH-WINDOW PROOF.** A push is crashed AFTER the outbox committed but BEFORE the on-disk ref
/// CAS applied. The committed `git.ref.updated` is durable; the on-disk ref is BEHIND. The reconciler
/// replays the committed event and recovers the ref onto disk — then is idempotent on a re-run, and the
/// repo is `git fsck` clean.
#[test]
fn reconciler_recovers_the_apply_after_outbox_commit_window_idempotently_fsck_clean() {
    let root = temp_root("window");
    let loc = RepoLoc::new("acme", "fr-par", "core");

    let store = DurableGitStore::rooted(&root);
    let repo = Arc::new(store.create_repo(&loc).expect("create"));
    let c1 = seed_commit(&repo, b"hello reconciler\n");

    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let rs = RefStore::open_durable(
        repo.clone(),
        "core",
        ctx_base("acme"),
        outbox.clone(),
        minter.clone(),
    );

    // Crash in the apply-after-outbox-commit window.
    let db = InMemoryObjectDb::new();
    let outcome = rs
        .receive(
            &push_create("refs/heads/main", &c1),
            &db,
            CrashPoint::AfterCommitBeforeApply,
        )
        .expect("receive");
    assert!(
        matches!(outcome, PushOutcome::Crashed(c) if c.at == CrashPoint::AfterCommitBeforeApply),
        "the push crashed in the reconciler window: {outcome:?}"
    );
    // A different tenant can have the same local repository slug. Its committed witness must never
    // be selected while reconciling acme/core.
    let foreign = RefStore::open_durable(
        repo.clone(),
        "core",
        ctx_base("other-tenant"),
        outbox.clone(),
        minter,
    );
    foreign
        .receive(
            &push_create("refs/heads/foreign", &c1),
            &db,
            CrashPoint::AfterCommitBeforeApply,
        )
        .expect("commit same-slug foreign-tenant witness");
    // The event is the durable witness (committed); the on-disk ref is BEHIND (not yet applied).
    assert_eq!(
        outbox.committed_count(),
        2,
        "both tenant witnesses committed"
    );
    assert_eq!(
        repo.read_ref("refs/heads/main").expect("read"),
        None,
        "the on-disk ref is behind its committed update_seq (the crash window)"
    );

    // Publication can independently exhaust its retry budget. Dead-lettering changes the ordinary
    // `committed_rows` live-set view, but it must never erase the retained state-change witness that
    // boot recovery consumes.
    let bus = InProcessBus::new();
    bus.sever();
    let relay = Relay::new(outbox.clone(), bus, || {
        Timestamp("2026-06-29T00:00:02Z".into())
    });
    for _ in 0..MAX_PUBLISH_ATTEMPTS {
        relay.drain_once();
    }
    assert!(
        outbox.committed_rows().is_empty(),
        "live-set semantics unchanged"
    );
    assert_eq!(outbox.dead_letters().len(), 2);
    assert_eq!(
        outbox.try_retained_rows().unwrap().len(),
        2,
        "dead-lettered ref witnesses remain retained for recovery"
    );

    // Simulate restart: a FRESH durable store + repo handle over the same root, replay the committed
    // events through the reconciler.
    let store2 = DurableGitStore::rooted(&root);
    let repo2 = store2.open_repo(&loc).expect("open after restart");
    assert!(refs_from_outbox_scoped_bounded(
        &outbox,
        "acme",
        "fr-par",
        "core",
        1,
        SNAPSHOT_MAX_BYTES,
    )
    .is_err());
    assert!(refs_from_outbox_scoped_bounded(
        &outbox,
        "acme",
        "fr-par",
        "core",
        SNAPSHOT_MAX_ROWS,
        0,
    )
    .is_err());
    let records = refs_from_outbox_scoped_bounded(
        &outbox,
        "acme",
        "fr-par",
        "core",
        SNAPSHOT_MAX_ROWS,
        SNAPSHOT_MAX_BYTES,
    )
    .unwrap();
    assert_eq!(records.len(), 1, "same-slug foreign tenant row excluded");
    let report = reconcile_refs(&repo2, &records).expect("reconcile");
    assert_eq!(
        report.reapplied,
        vec![("refs/heads/main".to_string(), 1)],
        "the window was recovered"
    );
    assert_eq!(
        repo2
            .read_ref("refs/heads/main")
            .expect("read")
            .map(|o| o.0),
        Some(c1.0.clone()),
        "the committed ref move is now on disk (recovered to the committed update_seq)"
    );

    // Idempotent: a second reconcile re-applies nothing.
    let again = reconcile_refs(&repo2, &records).expect("reconcile again");
    assert!(!again.recovered_any(), "idempotent on update_seq");
    assert_eq!(again.already_current, 1);

    // External oracle: git fsck clean after the recovery.
    let (ok, err) = git_fsck(repo2.path());
    assert!(ok, "git fsck clean after reconcile: {err}");

    std::fs::remove_dir_all(&root).ok();
}
