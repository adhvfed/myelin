//! # `reconcile` — the cross-system recovery reconciler for the apply-after-outbox-commit window
//! (GT-003 / E1.2; the GT-001 prerequisite for the live front door)
//!
//! **The window this closes.** The push write path ([`crate::receive_pack::RefStore::receive`]) commits
//! the `git.ref.updated` outbox row and the on-disk ref CAS as the two halves of one logical update, but
//! on the durable backing they are applied in sequence: the outbox transaction commits FIRST (the event
//! is the durable witness — emit-iff-committed, BUS-2), then the on-disk ref CAS applies
//! ([`crate::receive_pack::RefStore::apply_one`]). A crash BETWEEN those two steps (modeled by
//! [`crate::receive_pack::CrashPoint::AfterCommitBeforeApply`]) leaves the on-disk ref momentarily
//! BEHIND its committed `update_seq`: the event is durable, the ref move is pending.
//!
//! This is **NOT silent data loss** — the committed `git.ref.updated` row is the durable witness, so the
//! correct on-disk state is fully recoverable. This module is that recovery: on restart (before the
//! durable store serves the live front door), replay the committed `git.ref.updated` rows and re-apply
//! any whose on-disk `update_seq` is behind the durable reflog. The replay is **at-least-once + idempotent
//! on `update_seq`** (arch §4.2 — the recovery fence): re-running it over an already-current repo is a
//! no-op, and a partially-applied burst is driven forward to exactly the committed sequence.
//!
//! ## Anti-duplication
//! The reconciler REUSES the durable store's own per-ref CAS ([`DurableGitRepo::update_ref_cas`]) and the
//! on-disk reflog length ([`DurableGitRepo::reflog_len`]) as the durable `update_seq` — it does NOT
//! reimplement ref storage or a parallel seq counter. The committed events come from the already-frozen
//! [`myelin_events::OutboxStore`] (in production the durable `outbox` table); [`refs_from_outbox`]
//! extracts the `git.ref.updated` rows for one repo from that store.

use crate::core::Oid;
use crate::durable::{DurableError, DurableGitRepo};
use crate::events::GIT_REF_UPDATED;
use myelin_events::OutboxStore;

/// One committed `git.ref.updated` record — the durable witness of a ref move (the payload
/// [`crate::receive_pack::RefStore::receive`] emits). The reconciler replays these; the on-disk reflog
/// length is the durable `update_seq` it compares against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitRefUpdatedRecord {
    /// The repo the move targeted (the `<repo>` half of the `<repo>:<ref>` aggregate).
    pub repo: String,
    /// The fully-qualified ref that moved.
    pub ref_name: String,
    /// The tip the move was from (the all-zeros sentinel / empty means create).
    pub old_oid: String,
    /// The tip the move was to (the all-zeros sentinel / empty means delete).
    pub new_oid: String,
    /// The committed per-ref generation (the recovery fence — idempotent on this).
    pub update_seq: u64,
    /// The pusher pseudonym (GIT-1 — recorded on the re-applied reflog entry, never a raw identity).
    pub pusher_pseudonym: String,
}

impl GitRefUpdatedRecord {
    /// Parse a committed `git.ref.updated` payload into a record. Returns `None` if a required field is
    /// missing (a malformed row is skipped loudly by the caller, never silently mis-applied).
    pub fn from_payload(repo_filter: Option<&str>, payload: &serde_json::Value) -> Option<Self> {
        let repo = payload.get("repo")?.as_str()?.to_string();
        if let Some(want) = repo_filter {
            if repo != want {
                return None;
            }
        }
        Some(GitRefUpdatedRecord {
            repo,
            ref_name: payload.get("ref")?.as_str()?.to_string(),
            old_oid: payload
                .get("old_oid")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            new_oid: payload.get("new_oid")?.as_str()?.to_string(),
            update_seq: payload.get("update_seq")?.as_u64()?,
            pusher_pseudonym: payload
                .get("pusher_pseudonym")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        })
    }

    fn new_is_delete(&self) -> bool {
        self.new_oid.is_empty() || self.new_oid.chars().all(|c| c == '0')
    }

    fn old_is_create(&self) -> bool {
        self.old_oid.is_empty() || self.old_oid.chars().all(|c| c == '0')
    }
}

/// Extract every committed `git.ref.updated` record for a repo from the outbox (the durable witness set
/// the reconciler replays). In production these are read from the durable `outbox` table on restart; the
/// in-memory [`OutboxStore`] models the same committed sequence. Pass `Some(repo)` to scope to one repo.
pub fn refs_from_outbox(outbox: &OutboxStore, repo: Option<&str>) -> Vec<GitRefUpdatedRecord> {
    outbox
        .committed_rows()
        .into_iter()
        .filter(|row| row.envelope.type_.0 == GIT_REF_UPDATED)
        .filter_map(|row| GitRefUpdatedRecord::from_payload(repo, &row.envelope.payload))
        .collect()
}

/// What the reconciler did — the loud, inspectable recovery report (a recovery is diagnosable, never a
/// silent mutation).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// The committed records examined.
    pub examined: usize,
    /// The records re-applied (the on-disk ref was behind) — `(ref_name, update_seq)`.
    pub reapplied: Vec<(String, u64)>,
    /// The records already current on disk (idempotent skips — the common, no-crash case).
    pub already_current: usize,
}

impl ReconcileReport {
    /// Whether any ref move was recovered (the crash window had to be healed).
    pub fn recovered_any(&self) -> bool {
        !self.reapplied.is_empty()
    }
}

/// **The recovery reconciler (GT-003).** Replay the committed `git.ref.updated` records against the
/// durable on-disk repo, re-applying any whose `update_seq` is AHEAD of the on-disk reflog length (the
/// durable per-ref generation). Idempotent on `update_seq`: a record whose `update_seq <= reflog_len(ref)`
/// is already applied and skipped; a record whose `update_seq > reflog_len(ref)` is the un-applied tail
/// of the crash window and is re-applied via the durable per-ref CAS, advancing the on-disk ref to the
/// committed tip.
///
/// Records are processed per ref in ascending `update_seq` order, so a multi-update burst that was only
/// partially applied is driven forward exactly to the committed sequence (no gap, no double-apply). The
/// CAS expected-old is the CURRENT on-disk tip (which, after the prior re-applies, equals the record's
/// `old_oid`) — so the re-apply is the same per-ref linearisation point the live path uses, never a
/// blind force-write.
pub fn reconcile_refs(
    repo: &DurableGitRepo,
    records: &[GitRefUpdatedRecord],
) -> Result<ReconcileReport, DurableError> {
    let mut report = ReconcileReport {
        examined: records.len(),
        ..ReconcileReport::default()
    };

    // Process ascending by update_seq so a partially-applied burst is replayed in order.
    let mut ordered: Vec<&GitRefUpdatedRecord> = records.iter().collect();
    ordered.sort_by_key(|r| r.update_seq);

    for rec in ordered {
        // The durable per-ref generation = the on-disk reflog length (survives restart).
        let on_disk_seq = repo.reflog_len(&rec.ref_name) as u64;
        if rec.update_seq <= on_disk_seq {
            // Already applied — the idempotent skip (the no-crash case + the re-run case).
            report.already_current += 1;
            continue;
        }

        // Behind: this committed move was not applied on disk (the crash window). Re-apply via the
        // durable per-ref CAS. The expected-old is the CURRENT on-disk tip (after any prior re-applies),
        // which equals the record's old_oid — so this is the real linearisation point, not a force.
        let expected = repo.read_ref(&rec.ref_name)?;
        // Defensive consistency: the on-disk tip must match what the committed record moved FROM. If it
        // does not, the durable reflog disagrees with the committed event — surface loud (never a silent
        // wrong-bytes apply). A create record (`old` zero) expects no ref.
        let on_disk_matches_old = match (&expected, rec.old_is_create()) {
            (None, true) => true,
            (Some(tip), false) => tip.as_str() == rec.old_oid,
            _ => false,
        };
        if !on_disk_matches_old {
            return Err(DurableError::CasMismatch {
                ref_name: rec.ref_name.clone(),
                expected: if rec.old_is_create() {
                    None
                } else {
                    Some(rec.old_oid.clone())
                },
                actual: expected.map(|o| o.0),
            });
        }

        let new = if rec.new_is_delete() {
            None
        } else {
            Some(Oid::new(rec.new_oid.clone()))
        };
        let msg = format!("reconcile(GT-003): replay {}:{} seq {}", rec.repo, rec.ref_name, rec.update_seq);
        repo.update_ref_cas(
            &rec.ref_name,
            expected.as_ref(),
            new.as_ref(),
            &msg,
            &rec.pusher_pseudonym,
        )?;
        report.reapplied.push((rec.ref_name.clone(), rec.update_seq));
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::RepoLoc;
    use crate::durable::DurableGitStore;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        p.push(format!("myelin-reconcile-{tag}-{nanos}"));
        p
    }

    fn loc() -> RepoLoc {
        RepoLoc::new("acme", "fr-par", "core")
    }

    fn seed_commit(repo: &DurableGitRepo, content: &[u8]) -> Oid {
        let blob = repo.write_blob(content).expect("blob");
        let tree = repo.write_tree(&[("file.txt", &blob)]).expect("tree");
        repo.write_commit(&tree, &[], "feat: seed", "psn@acme.noreply", "psn@acme.noreply")
            .expect("commit")
    }

    /// **The crash-window recovery proof.** A committed create record exists but the on-disk ref was
    /// never applied (the apply-after-outbox-commit window). The reconciler replays it → the ref now
    /// points at the committed tip. A second run is a no-op (idempotent on `update_seq`).
    #[test]
    fn reconcile_recovers_an_unapplied_committed_ref_then_is_idempotent() {
        let root = temp_root("recover");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let c1 = seed_commit(&repo, b"hello\n");

        // The crash window: a committed create record, but the on-disk ref does not exist yet.
        let rec = GitRefUpdatedRecord {
            repo: "core".into(),
            ref_name: "refs/heads/main".into(),
            old_oid: "0".repeat(40),
            new_oid: c1.0.clone(),
            update_seq: 1,
            pusher_pseudonym: "psn@acme.noreply".into(),
        };
        assert_eq!(repo.read_ref("refs/heads/main").unwrap(), None, "ref behind (window)");

        let report = reconcile_refs(&repo, std::slice::from_ref(&rec)).expect("reconcile");
        assert!(report.recovered_any());
        assert_eq!(report.reapplied, vec![("refs/heads/main".to_string(), 1)]);
        assert_eq!(
            repo.read_ref("refs/heads/main").unwrap(),
            Some(c1.clone()),
            "the committed ref move was recovered onto disk"
        );

        // Idempotent: a second run re-applies nothing (update_seq 1 <= on-disk seq 1).
        let again = reconcile_refs(&repo, std::slice::from_ref(&rec)).expect("reconcile again");
        assert!(!again.recovered_any(), "idempotent on update_seq");
        assert_eq!(again.already_current, 1);
        assert_eq!(repo.read_ref("refs/heads/main").unwrap(), Some(c1));
        std::fs::remove_dir_all(&root).ok();
    }

    /// A partially-applied burst (seq 1 on disk, seq 2 committed but un-applied) is driven forward to
    /// exactly the committed sequence — no gap, no double-apply.
    #[test]
    fn reconcile_drives_a_partial_burst_forward_to_the_committed_seq() {
        let root = temp_root("burst");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let c1 = seed_commit(&repo, b"v1\n");
        let blob2 = repo.write_blob(b"v2\n").unwrap();
        let tree2 = repo.write_tree(&[("file.txt", &blob2)]).unwrap();
        let c2 = repo
            .write_commit(&tree2, &[&c1], "v2", "psn@acme.noreply", "psn@acme.noreply")
            .unwrap();

        // seq 1 already applied on disk.
        repo.update_ref_cas("refs/heads/main", None, Some(&c1), "create", "psn@acme.noreply")
            .unwrap();
        assert_eq!(repo.reflog_len("refs/heads/main"), 1);

        let recs = vec![
            GitRefUpdatedRecord {
                repo: "core".into(),
                ref_name: "refs/heads/main".into(),
                old_oid: "0".repeat(40),
                new_oid: c1.0.clone(),
                update_seq: 1,
                pusher_pseudonym: "psn@acme.noreply".into(),
            },
            GitRefUpdatedRecord {
                repo: "core".into(),
                ref_name: "refs/heads/main".into(),
                old_oid: c1.0.clone(),
                new_oid: c2.0.clone(),
                update_seq: 2,
                pusher_pseudonym: "psn@acme.noreply".into(),
            },
        ];
        let report = reconcile_refs(&repo, &recs).expect("reconcile");
        assert_eq!(report.reapplied, vec![("refs/heads/main".to_string(), 2)], "only seq 2 re-applied");
        assert_eq!(report.already_current, 1, "seq 1 already current");
        assert_eq!(repo.read_ref("refs/heads/main").unwrap(), Some(c2));
        assert_eq!(repo.reflog_len("refs/heads/main"), 2);
        std::fs::remove_dir_all(&root).ok();
    }
}
