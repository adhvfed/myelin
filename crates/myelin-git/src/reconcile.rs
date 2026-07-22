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
//! any whose on-disk `update_seq` is behind the durable per-ref generation. The replay is **at-least-once + idempotent
//! on `update_seq`** (arch §4.2 — the recovery fence): re-running it over an already-current repo is a
//! no-op, and a partially-applied burst is driven forward to exactly the committed sequence.
//!
//! ## Anti-duplication
//! The reconciler REUSES the durable store's own per-ref CAS ([`DurableGitRepo::update_ref_cas`]) and the
//! durable per-ref generation ([`DurableGitRepo::ref_generation`]) as the on-disk `update_seq` — it does
//! NOT reimplement ref storage or a parallel seq counter. (R0.4 / git #1 HIGH: this was the reflog LENGTH,
//! which RESET on a ref's delete+recreate and broke the fence — the generation is now a monotonic
//! config-backed counter keyed by ref name.) The committed events come from the already-frozen
//! [`myelin_events::OutboxStore`] (in production the durable `outbox` table);
//! [`refs_from_outbox_scoped`] extracts rows for one exact tenant/region/repository authority tuple.

use crate::core::Oid;
use crate::durable::{DurableError, DurableGitRepo};
use crate::events::GIT_REF_UPDATED;
use myelin_events::OutboxStore;
use std::collections::BTreeSet;

/// One committed `git.ref.updated` record — the durable witness of a ref move (the payload
/// [`crate::receive_pack::RefStore::receive`] emits). The reconciler replays these; the durable per-ref
/// generation ([`DurableGitRepo::ref_generation`]) is the on-disk `update_seq` it compares against.
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
    /// Parse a retained `git.ref.updated` witness. `Ok(None)` is reserved exclusively for a fully
    /// valid witness naming a different repository; malformed scoped witnesses fail boot loudly.
    pub fn from_payload(
        repo_filter: Option<&str>,
        payload: &serde_json::Value,
    ) -> Result<Option<Self>, DurableError> {
        fn required_string(
            payload: &serde_json::Value,
            field: &str,
        ) -> Result<String, DurableError> {
            payload
                .get(field)
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| {
                    DurableError::Io(format!(
                        "retained git.ref.updated witness has invalid `{field}`"
                    ))
                })
        }

        let record = GitRefUpdatedRecord {
            repo: required_string(payload, "repo")?,
            ref_name: required_string(payload, "ref")?,
            old_oid: required_string(payload, "old_oid")?,
            new_oid: required_string(payload, "new_oid")?,
            update_seq: payload
                .get("update_seq")
                .and_then(serde_json::Value::as_u64)
                .filter(|value| *value > 0)
                .ok_or_else(|| {
                    DurableError::Io(
                        "retained git.ref.updated witness has invalid `update_seq`".into(),
                    )
                })?,
            pusher_pseudonym: required_string(payload, "pusher_pseudonym")?,
        };
        if repo_filter.is_some_and(|wanted| wanted != record.repo) {
            return Ok(None);
        }
        Ok(Some(record))
    }

    fn new_is_delete(&self) -> bool {
        self.new_oid.is_empty() || self.new_oid.chars().all(|c| c == '0')
    }

    fn old_is_create(&self) -> bool {
        self.old_oid.is_empty() || self.old_oid.chars().all(|c| c == '0')
    }
}

/// Extract committed `git.ref.updated` records for one exact tenant/region/repository authority
/// tuple. Repository slugs are tenant-local, so filtering only the payload slug can replay another
/// tenant's same-named event into this repository. Authority is therefore checked on the envelope
/// before the payload is decoded.
pub fn refs_from_outbox_scoped(
    outbox: &OutboxStore,
    tenant: &str,
    region: &str,
    repo: &str,
) -> Result<Vec<GitRefUpdatedRecord>, DurableError> {
    outbox
        .try_retained_rows()
        .map_err(|_| DurableError::Io("durable outbox witness snapshot failed".into()))?
        .into_iter()
        .filter(|row| row.envelope.type_.0 == GIT_REF_UPDATED)
        .filter(|row| row.envelope.tenant.0 == tenant && row.envelope.region.0 == region)
        .map(|row| GitRefUpdatedRecord::from_payload(Some(repo), &row.envelope.payload))
        .filter_map(Result::transpose)
        .collect()
}

/// Repository slugs that have committed ref witnesses in one exact tenant/region partition. This is
/// the durable boot-recovery discovery set; callers union it with repositories named by pending PR
/// merge intents and never infer recovery authority from filesystem directory names.
pub fn repo_slugs_from_outbox_scoped(
    outbox: &OutboxStore,
    tenant: &str,
    region: &str,
) -> Result<BTreeSet<String>, DurableError> {
    outbox
        .try_retained_rows()
        .map_err(|_| DurableError::Io("durable outbox witness snapshot failed".into()))?
        .into_iter()
        .filter(|row| row.envelope.type_.0 == GIT_REF_UPDATED)
        .filter(|row| row.envelope.tenant.0 == tenant && row.envelope.region.0 == region)
        .map(|row| {
            GitRefUpdatedRecord::from_payload(None, &row.envelope.payload).and_then(|record| {
                record
                    .map(|record| record.repo)
                    .ok_or_else(|| DurableError::Io("valid ref witness unexpectedly skipped".into()))
            })
        })
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
    /// Ref CASes that were already applied but whose generation fence required repair after a crash.
    pub repaired_fences: Vec<(String, u64)>,
    /// The records already current on disk (idempotent skips — the common, no-crash case).
    pub already_current: usize,
}

impl ReconcileReport {
    /// Whether any ref move was recovered (the crash window had to be healed).
    pub fn recovered_any(&self) -> bool {
        !self.reapplied.is_empty() || !self.repaired_fences.is_empty()
    }
}

/// **The recovery reconciler (GT-003).** Replay the committed `git.ref.updated` records against the
/// durable on-disk repo, re-applying any whose `update_seq` is AHEAD of the durable per-ref generation
/// ([`DurableGitRepo::ref_generation`]). Idempotent on `update_seq`: a record whose `update_seq <= ref_generation(ref)`
/// is already applied and skipped; a record whose `update_seq > ref_generation(ref)` is the un-applied tail
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
        // The durable per-ref generation (R0.4 / git #1 HIGH — the config-backed counter, NOT the
        // reflog length). It survives restart AND is monotonic across a ref's delete+recreate, so the
        // idempotent `<=` comparison below is exact even after a branch was deleted and recreated
        // (reflog length would have RESET on the recreate and mis-compared here).
        let on_disk_seq = repo.ref_generation(&rec.ref_name)?;
        if rec.update_seq < on_disk_seq {
            // A historical record behind a later applied generation — the idempotent skip.
            report.already_current += 1;
            continue;
        }

        let expected = repo.read_ref(&rec.ref_name)?;
        let on_disk_matches_new = match (&expected, rec.new_is_delete()) {
            (None, true) => true,
            (Some(tip), false) => tip.as_str() == rec.new_oid,
            _ => false,
        };

        if rec.update_seq == on_disk_seq {
            // A witness claiming the CURRENT generation must describe the current tip. Reject a
            // conflicting same-sequence committed witness instead of silently marking both current.
            if !on_disk_matches_new {
                return Err(DurableError::CasMismatch {
                    ref_name: rec.ref_name.clone(),
                    expected: if rec.new_is_delete() { None } else { Some(rec.new_oid.clone()) },
                    actual: expected.map(|oid| oid.0),
                });
            }
            report.already_current += 1;
            continue;
        }

        let next_seq = crate::durable::next_ref_generation(on_disk_seq).ok_or_else(|| {
            DurableError::Io(format!("ref generation exhausted for {}", rec.ref_name))
        })?;
        if rec.update_seq != next_seq {
            return Err(DurableError::Io(format!(
                "committed ref witness sequence gap for {}: on-disk {}, witness {}",
                rec.ref_name, on_disk_seq, rec.update_seq
            )));
        }

        // The ref mutation can become durable immediately before its generation config write. If the
        // tip already equals the committed new state, repair only that missing fence; replaying the CAS
        // would fail against `old_oid` and wedge boot. This applies equally to an absent deleted ref.
        if on_disk_matches_new {
            let repaired = repo.repair_ref_generation(&rec.ref_name, on_disk_seq)?;
            debug_assert_eq!(repaired, rec.update_seq);
            report.repaired_fences.push((rec.ref_name.clone(), rec.update_seq));
            continue;
        }

        // Behind: this committed move was not applied on disk (the crash window). Re-apply via the
        // durable per-ref CAS. The expected-old is the CURRENT on-disk tip (after any prior re-applies),
        // which equals the record's old_oid — so this is the real linearisation point, not a force.
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

    /// A crash can land after libgit2 mutates the ref but before the separate generation config
    /// write. Recovery repairs only the missing fence for both an update and a deletion.
    #[test]
    fn reconcile_repairs_generation_when_update_or_delete_tip_already_landed() {
        let root = temp_root("repair-fence");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let c1 = seed_commit(&repo, b"v1\n");
        let blob2 = repo.write_blob(b"v2\n").unwrap();
        let tree2 = repo.write_tree(&[("file.txt", &blob2)]).unwrap();
        let c2 = repo
            .write_commit(&tree2, &[&c1], "v2", "psn@acme.noreply", "psn@acme.noreply")
            .unwrap();

        for ref_name in ["refs/heads/update", "refs/heads/delete"] {
            repo.update_ref_cas(ref_name, None, Some(&c1), "create", "psn@acme.noreply")
                .unwrap();
            assert_eq!(repo.ref_generation(ref_name), Ok(1));
        }

        // Simulate interruption inside `update_ref_cas`: change the refs with raw libgit2 while
        // deliberately leaving the config-backed generations at 1.
        let raw = git2::Repository::open_bare(repo.path()).unwrap();
        raw.reference_matching(
            "refs/heads/update",
            git2::Oid::from_str(&c2.0).unwrap(),
            true,
            git2::Oid::from_str(&c1.0).unwrap(),
            "raw update before generation crash",
        )
        .unwrap();
        raw.find_reference("refs/heads/delete").unwrap().delete().unwrap();

        let records = vec![
            GitRefUpdatedRecord {
                repo: "core".into(), ref_name: "refs/heads/update".into(),
                old_oid: c1.0.clone(), new_oid: c2.0.clone(), update_seq: 2,
                pusher_pseudonym: "psn@acme.noreply".into(),
            },
            GitRefUpdatedRecord {
                repo: "core".into(), ref_name: "refs/heads/delete".into(),
                old_oid: c1.0.clone(), new_oid: "0".repeat(40), update_seq: 2,
                pusher_pseudonym: "psn@acme.noreply".into(),
            },
        ];

        let report = reconcile_refs(&repo, &records).expect("repair applied-but-unfenced refs");
        assert!(report.reapplied.is_empty(), "the ref CASes had already landed");
        assert_eq!(
            report.repaired_fences,
            vec![("refs/heads/update".into(), 2), ("refs/heads/delete".into(), 2)]
        );
        assert_eq!(repo.read_ref("refs/heads/update").unwrap(), Some(c2));
        assert_eq!(repo.read_ref("refs/heads/delete").unwrap(), None);
        assert_eq!(repo.ref_generation("refs/heads/update"), Ok(2));
        assert_eq!(repo.ref_generation("refs/heads/delete"), Ok(2));

        let again = reconcile_refs(&repo, &records).expect("repaired fences are idempotent");
        assert_eq!(again.already_current, 2);
        assert!(!again.recovered_any());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reconcile_rejects_a_conflicting_witness_at_the_current_generation() {
        let root = temp_root("same-seq-conflict");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let landed = seed_commit(&repo, b"landed\n");
        let conflicting = seed_commit(&repo, b"conflicting\n");
        repo.update_ref_cas(
            "refs/heads/main", None, Some(&landed), "create", "psn@acme.noreply",
        )
        .unwrap();
        let record = GitRefUpdatedRecord {
            repo: "core".into(), ref_name: "refs/heads/main".into(),
            old_oid: "0".repeat(40), new_oid: conflicting.0, update_seq: 1,
            pusher_pseudonym: "psn@acme.noreply".into(),
        };

        assert!(matches!(
            reconcile_refs(&repo, &[record]),
            Err(DurableError::CasMismatch { .. })
        ));
        assert_eq!(repo.read_ref("refs/heads/main").unwrap(), Some(landed));
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
        assert_eq!(repo.reflog_len("refs/heads/main"), Ok(1));

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
        assert_eq!(repo.reflog_len("refs/heads/main"), Ok(2));
        std::fs::remove_dir_all(&root).ok();
    }

    /// **R0.4 / git #1 HIGH — THE CORE REGRESSION: reconcile is a clean no-op after a delete+recreate.**
    ///
    /// The whole committed history of a ref that was created, updated, DELETED, then RECREATED is on
    /// disk and fully applied (the ref points at the recreated tip). The recreated ref's REFLOG has
    /// restarted at length 1 (libgit2 destroys a ref's reflog on delete), but the durable generation is
    /// monotonic at 4. Reconciling all four committed records must be a clean no-op: the ref stays at the
    /// recreated tip, nothing is re-applied, and NO CAS-mismatch is raised.
    ///
    /// This is the test that FAILS on `reflog_len`-as-generation: with the restarted reflog reading 1,
    /// the reconciler would see seq 2 (2 > 1) as un-applied and try to replay a STALE move
    /// (old_oid = c1 vs the on-disk recreated tip) → a spurious `CasMismatch`. With the durable counter
    /// (4) every record is `<= 4` → skipped, and the ref is left correct (never reverted / deleted).
    #[test]
    fn reconcile_is_a_noop_after_delete_recreate_with_monotonic_generation() {
        let root = temp_root("delrecreate");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let c1 = seed_commit(&repo, b"v1\n");
        let blob2 = repo.write_blob(b"v2\n").unwrap();
        let tree2 = repo.write_tree(&[("file.txt", &blob2)]).unwrap();
        let c2 = repo
            .write_commit(&tree2, &[&c1], "v2", "psn@acme.noreply", "psn@acme.noreply")
            .unwrap();
        let c3 = seed_commit(&repo, b"reborn\n");

        // The full applied history on disk: create → update → delete → recreate.
        repo.update_ref_cas("refs/heads/main", None, Some(&c1), "create", "psn@acme.noreply").unwrap();
        repo.update_ref_cas("refs/heads/main", Some(&c1), Some(&c2), "ff", "psn@acme.noreply").unwrap();
        repo.update_ref_cas("refs/heads/main", Some(&c2), None, "delete", "psn@acme.noreply").unwrap();
        repo.update_ref_cas("refs/heads/main", None, Some(&c3), "recreate", "psn@acme.noreply").unwrap();

        // The reflog RESTARTED (the old, wrong generation source) but the durable generation is 4.
        assert_eq!(
            repo.reflog_len("refs/heads/main"),
            Ok(1),
            "recreated ref's reflog restarted"
        );
        assert_eq!(
            repo.ref_generation("refs/heads/main"),
            Ok(4),
            "durable generation monotonic across the delete — did NOT reset"
        );

        let zero = "0".repeat(40);
        let recs = vec![
            GitRefUpdatedRecord {
                repo: "core".into(), ref_name: "refs/heads/main".into(),
                old_oid: zero.clone(), new_oid: c1.0.clone(), update_seq: 1,
                pusher_pseudonym: "psn@acme.noreply".into(),
            },
            GitRefUpdatedRecord {
                repo: "core".into(), ref_name: "refs/heads/main".into(),
                old_oid: c1.0.clone(), new_oid: c2.0.clone(), update_seq: 2,
                pusher_pseudonym: "psn@acme.noreply".into(),
            },
            GitRefUpdatedRecord {
                repo: "core".into(), ref_name: "refs/heads/main".into(),
                old_oid: c2.0.clone(), new_oid: zero.clone(), update_seq: 3,
                pusher_pseudonym: "psn@acme.noreply".into(),
            },
            GitRefUpdatedRecord {
                repo: "core".into(), ref_name: "refs/heads/main".into(),
                old_oid: zero.clone(), new_oid: c3.0.clone(), update_seq: 4,
                pusher_pseudonym: "psn@acme.noreply".into(),
            },
        ];

        // Clean no-op: every record is already current at generation 4; NO stale replay, NO mismatch.
        let report = reconcile_refs(&repo, &recs).expect("reconcile must not raise CasMismatch");
        assert!(!report.recovered_any(), "nothing to recover — the ref is already at the committed tip");
        assert_eq!(report.already_current, 4, "all four records idempotently skipped");
        assert_eq!(
            repo.read_ref("refs/heads/main").unwrap(),
            Some(c3),
            "the ref is at the recreated tip — NOT left deleted, NOT reverted to a stale move"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// **R0.4 — a partial burst that SPANS a delete+recreate is driven forward.** Only the create (seq 1)
    /// was applied on disk before the crash; the committed records carry the delete (seq 2) and the
    /// recreate (seq 3) that were never applied. The reconciler replays the un-applied tail in order —
    /// delete then recreate — converging the ref to the recreated tip, with the durable generation
    /// advancing monotonically 1→2→3 across the delete.
    #[test]
    fn reconcile_drives_a_burst_across_delete_recreate_forward() {
        let root = temp_root("burstdel");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let c1 = seed_commit(&repo, b"v1\n");
        let c3 = seed_commit(&repo, b"reborn\n");

        // Only seq 1 (create) applied on disk; the crash left seq 2 (delete) + seq 3 (recreate) pending.
        repo.update_ref_cas("refs/heads/main", None, Some(&c1), "create", "psn@acme.noreply").unwrap();
        assert_eq!(repo.ref_generation("refs/heads/main"), Ok(1));

        let zero = "0".repeat(40);
        let recs = vec![
            GitRefUpdatedRecord {
                repo: "core".into(), ref_name: "refs/heads/main".into(),
                old_oid: zero.clone(), new_oid: c1.0.clone(), update_seq: 1,
                pusher_pseudonym: "psn@acme.noreply".into(),
            },
            GitRefUpdatedRecord {
                repo: "core".into(), ref_name: "refs/heads/main".into(),
                old_oid: c1.0.clone(), new_oid: zero.clone(), update_seq: 2,
                pusher_pseudonym: "psn@acme.noreply".into(),
            },
            GitRefUpdatedRecord {
                repo: "core".into(), ref_name: "refs/heads/main".into(),
                old_oid: zero.clone(), new_oid: c3.0.clone(), update_seq: 3,
                pusher_pseudonym: "psn@acme.noreply".into(),
            },
        ];

        let report = reconcile_refs(&repo, &recs).expect("reconcile");
        assert_eq!(
            report.reapplied,
            vec![("refs/heads/main".to_string(), 2), ("refs/heads/main".to_string(), 3)],
            "the delete AND the recreate were replayed forward in order"
        );
        assert_eq!(report.already_current, 1, "seq 1 (create) already applied");
        assert_eq!(
            repo.read_ref("refs/heads/main").unwrap(),
            Some(c3),
            "the ref converged to the recreated tip across the delete"
        );
        assert_eq!(
            repo.ref_generation("refs/heads/main"),
            Ok(3),
            "the durable generation advanced monotonically across the replayed delete"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
