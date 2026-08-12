use crate::core::Oid;
use crate::durable::{DurableError, DurableGitRepo};
use crate::events::GIT_REF_UPDATED;
use myelin_events::OutboxStore;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitRefUpdatedRecord {
    pub repo: String,
    pub ref_name: String,
    pub old_oid: String,
    pub new_oid: String,
    pub update_seq: u64,
    pub pusher_pseudonym: String,
}

impl GitRefUpdatedRecord {
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

pub fn refs_from_outbox_scoped_bounded(
    outbox: &OutboxStore,
    tenant: &str,
    region: &str,
    repo: &str,
    maximum_retained_rows: usize,
    maximum_envelope_bytes: usize,
) -> Result<Vec<GitRefUpdatedRecord>, DurableError> {
    Ok(refs_by_repo_from_outbox_scoped_bounded(
        outbox,
        tenant,
        region,
        maximum_retained_rows,
        maximum_envelope_bytes,
    )?
    .remove(repo)
    .unwrap_or_default())
}

pub fn refs_by_repo_from_outbox_scoped_bounded(
    outbox: &OutboxStore,
    tenant: &str,
    region: &str,
    maximum_retained_rows: usize,
    maximum_envelope_bytes: usize,
) -> Result<BTreeMap<String, Vec<GitRefUpdatedRecord>>, DurableError> {
    let rows = outbox
        .try_retained_rows_bounded(maximum_retained_rows, maximum_envelope_bytes)
        .map_err(|_| DurableError::Io("durable outbox witness snapshot failed".into()))?;
    let mut grouped = BTreeMap::<String, Vec<GitRefUpdatedRecord>>::new();
    for row in rows {
        if row.envelope.type_.0 != GIT_REF_UPDATED
            || row.envelope.tenant.0 != tenant
            || row.envelope.region.0 != region
        {
            continue;
        }
        let record = GitRefUpdatedRecord::from_payload(None, &row.envelope.payload)?
            .ok_or_else(|| DurableError::Io("valid ref witness unexpectedly skipped".into()))?;
        grouped.entry(record.repo.clone()).or_default().push(record);
    }
    Ok(grouped)
}

pub fn repo_slugs_from_outbox_scoped_bounded(
    outbox: &OutboxStore,
    tenant: &str,
    region: &str,
    maximum_retained_rows: usize,
    maximum_envelope_bytes: usize,
) -> Result<BTreeSet<String>, DurableError> {
    Ok(refs_by_repo_from_outbox_scoped_bounded(
        outbox,
        tenant,
        region,
        maximum_retained_rows,
        maximum_envelope_bytes,
    )?
    .into_keys()
    .collect())
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    pub examined: usize,
    pub reapplied: Vec<(String, u64)>,
    pub repaired_fences: Vec<(String, u64)>,
    pub already_current: usize,
}

impl ReconcileReport {
    pub fn recovered_any(&self) -> bool {
        !self.reapplied.is_empty() || !self.repaired_fences.is_empty()
    }
}

pub fn reconcile_refs(
    repo: &DurableGitRepo,
    records: &[GitRefUpdatedRecord],
) -> Result<ReconcileReport, DurableError> {
    let mut report = ReconcileReport {
        examined: records.len(),
        ..ReconcileReport::default()
    };

    let mut ordered: Vec<&GitRefUpdatedRecord> = records.iter().collect();
    ordered.sort_by_key(|r| r.update_seq);

    for rec in ordered {
        let on_disk_seq = repo.ref_generation(&rec.ref_name)?;
        if rec.update_seq < on_disk_seq {
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
            if !on_disk_matches_new {
                return Err(DurableError::CasMismatch {
                    ref_name: rec.ref_name.clone(),
                    expected: if rec.new_is_delete() {
                        None
                    } else {
                        Some(rec.new_oid.clone())
                    },
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

        if on_disk_matches_new {
            let repaired = repo.repair_ref_generation(&rec.ref_name, on_disk_seq)?;
            debug_assert_eq!(repaired, rec.update_seq);
            report
                .repaired_fences
                .push((rec.ref_name.clone(), rec.update_seq));
            continue;
        }

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
        let msg = format!(
            "reconcile(GT-003): replay {}:{} seq {}",
            rec.repo, rec.ref_name, rec.update_seq
        );
        repo.update_ref_cas(
            &rec.ref_name,
            expected.as_ref(),
            new.as_ref(),
            &msg,
            &rec.pusher_pseudonym,
        )?;
        report
            .reapplied
            .push((rec.ref_name.clone(), rec.update_seq));
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
        repo.write_commit(
            &tree,
            &[],
            "feat: seed",
            "psn@acme.noreply",
            "psn@acme.noreply",
        )
        .expect("commit")
    }

    #[test]
    fn reconcile_recovers_an_unapplied_committed_ref_then_is_idempotent() {
        let root = temp_root("recover");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let c1 = seed_commit(&repo, b"hello\n");

        let rec = GitRefUpdatedRecord {
            repo: "core".into(),
            ref_name: "refs/heads/main".into(),
            old_oid: "0".repeat(40),
            new_oid: c1.0.clone(),
            update_seq: 1,
            pusher_pseudonym: "psn@acme.noreply".into(),
        };
        assert_eq!(
            repo.read_ref("refs/heads/main").unwrap(),
            None,
            "ref behind (window)"
        );

        let report = reconcile_refs(&repo, std::slice::from_ref(&rec)).expect("reconcile");
        assert!(report.recovered_any());
        assert_eq!(report.reapplied, vec![("refs/heads/main".to_string(), 1)]);
        assert_eq!(
            repo.read_ref("refs/heads/main").unwrap(),
            Some(c1.clone()),
            "the committed ref move was recovered onto disk"
        );

        let again = reconcile_refs(&repo, std::slice::from_ref(&rec)).expect("reconcile again");
        assert!(!again.recovered_any(), "idempotent on update_seq");
        assert_eq!(again.already_current, 1);
        assert_eq!(repo.read_ref("refs/heads/main").unwrap(), Some(c1));
        std::fs::remove_dir_all(&root).ok();
    }

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

        let raw = git2::Repository::open_bare(repo.path()).unwrap();
        raw.reference_matching(
            "refs/heads/update",
            git2::Oid::from_str(&c2.0).unwrap(),
            true,
            git2::Oid::from_str(&c1.0).unwrap(),
            "raw update before generation crash",
        )
        .unwrap();
        raw.find_reference("refs/heads/delete")
            .unwrap()
            .delete()
            .unwrap();

        let records = vec![
            GitRefUpdatedRecord {
                repo: "core".into(),
                ref_name: "refs/heads/update".into(),
                old_oid: c1.0.clone(),
                new_oid: c2.0.clone(),
                update_seq: 2,
                pusher_pseudonym: "psn@acme.noreply".into(),
            },
            GitRefUpdatedRecord {
                repo: "core".into(),
                ref_name: "refs/heads/delete".into(),
                old_oid: c1.0.clone(),
                new_oid: "0".repeat(40),
                update_seq: 2,
                pusher_pseudonym: "psn@acme.noreply".into(),
            },
        ];

        let report = reconcile_refs(&repo, &records).expect("repair applied-but-unfenced refs");
        assert!(
            report.reapplied.is_empty(),
            "the ref CASes had already landed"
        );
        assert_eq!(
            report.repaired_fences,
            vec![
                ("refs/heads/update".into(), 2),
                ("refs/heads/delete".into(), 2)
            ]
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
            "refs/heads/main",
            None,
            Some(&landed),
            "create",
            "psn@acme.noreply",
        )
        .unwrap();
        let record = GitRefUpdatedRecord {
            repo: "core".into(),
            ref_name: "refs/heads/main".into(),
            old_oid: "0".repeat(40),
            new_oid: conflicting.0,
            update_seq: 1,
            pusher_pseudonym: "psn@acme.noreply".into(),
        };

        assert!(matches!(
            reconcile_refs(&repo, &[record]),
            Err(DurableError::CasMismatch { .. })
        ));
        assert_eq!(repo.read_ref("refs/heads/main").unwrap(), Some(landed));
        std::fs::remove_dir_all(&root).ok();
    }

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

        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&c1),
            "create",
            "psn@acme.noreply",
        )
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
        assert_eq!(
            report.reapplied,
            vec![("refs/heads/main".to_string(), 2)],
            "only seq 2 re-applied"
        );
        assert_eq!(report.already_current, 1, "seq 1 already current");
        assert_eq!(repo.read_ref("refs/heads/main").unwrap(), Some(c2));
        assert_eq!(repo.reflog_len("refs/heads/main"), Ok(2));
        std::fs::remove_dir_all(&root).ok();
    }

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

        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&c1),
            "create",
            "psn@acme.noreply",
        )
        .unwrap();
        repo.update_ref_cas(
            "refs/heads/main",
            Some(&c1),
            Some(&c2),
            "ff",
            "psn@acme.noreply",
        )
        .unwrap();
        repo.update_ref_cas(
            "refs/heads/main",
            Some(&c2),
            None,
            "delete",
            "psn@acme.noreply",
        )
        .unwrap();
        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&c3),
            "recreate",
            "psn@acme.noreply",
        )
        .unwrap();

        assert_eq!(
            repo.reflog_len("refs/heads/main"),
            Ok(1),
            "recreated ref's reflog restarted"
        );
        assert_eq!(
            repo.ref_generation("refs/heads/main"),
            Ok(4),
            "durable generation monotonic across the delete - did NOT reset"
        );

        let zero = "0".repeat(40);
        let recs = vec![
            GitRefUpdatedRecord {
                repo: "core".into(),
                ref_name: "refs/heads/main".into(),
                old_oid: zero.clone(),
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
            GitRefUpdatedRecord {
                repo: "core".into(),
                ref_name: "refs/heads/main".into(),
                old_oid: c2.0.clone(),
                new_oid: zero.clone(),
                update_seq: 3,
                pusher_pseudonym: "psn@acme.noreply".into(),
            },
            GitRefUpdatedRecord {
                repo: "core".into(),
                ref_name: "refs/heads/main".into(),
                old_oid: zero.clone(),
                new_oid: c3.0.clone(),
                update_seq: 4,
                pusher_pseudonym: "psn@acme.noreply".into(),
            },
        ];

        let report = reconcile_refs(&repo, &recs).expect("reconcile must not raise CasMismatch");
        assert!(
            !report.recovered_any(),
            "nothing to recover - the ref is already at the committed tip"
        );
        assert_eq!(
            report.already_current, 4,
            "all four records idempotently skipped"
        );
        assert_eq!(
            repo.read_ref("refs/heads/main").unwrap(),
            Some(c3),
            "the ref is at the recreated tip - NOT left deleted, NOT reverted to a stale move"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reconcile_drives_a_burst_across_delete_recreate_forward() {
        let root = temp_root("burstdel");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let c1 = seed_commit(&repo, b"v1\n");
        let c3 = seed_commit(&repo, b"reborn\n");

        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&c1),
            "create",
            "psn@acme.noreply",
        )
        .unwrap();
        assert_eq!(repo.ref_generation("refs/heads/main"), Ok(1));

        let zero = "0".repeat(40);
        let recs = vec![
            GitRefUpdatedRecord {
                repo: "core".into(),
                ref_name: "refs/heads/main".into(),
                old_oid: zero.clone(),
                new_oid: c1.0.clone(),
                update_seq: 1,
                pusher_pseudonym: "psn@acme.noreply".into(),
            },
            GitRefUpdatedRecord {
                repo: "core".into(),
                ref_name: "refs/heads/main".into(),
                old_oid: c1.0.clone(),
                new_oid: zero.clone(),
                update_seq: 2,
                pusher_pseudonym: "psn@acme.noreply".into(),
            },
            GitRefUpdatedRecord {
                repo: "core".into(),
                ref_name: "refs/heads/main".into(),
                old_oid: zero.clone(),
                new_oid: c3.0.clone(),
                update_seq: 3,
                pusher_pseudonym: "psn@acme.noreply".into(),
            },
        ];

        let report = reconcile_refs(&repo, &recs).expect("reconcile");
        assert_eq!(
            report.reapplied,
            vec![
                ("refs/heads/main".to_string(), 2),
                ("refs/heads/main".to_string(), 3)
            ],
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
