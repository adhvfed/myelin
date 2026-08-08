use super::{BlobPathLookup, DurableError, DurableGitBackend};
use myelin_git::core::ReadBackend;
use myelin_git::gix_backend::{GixCore, RootedResolver};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const BLAME_MAX_PATH_BYTES: usize = 4 * 1024;
const BLAME_MAX_BLOB_BYTES: usize = 512 * 1024;
const BLAME_MAX_HUNKS: usize = 20_000;
const BLAME_MAX_LINES: usize = 10_000;

impl DurableGitBackend {
    pub(super) fn blame_json(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        gitref: &str,
        path: &str,
    ) -> Result<Value, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let repo = self.store.open_repo(&loc)?;
        let snapshot_oid = repo
            .resolve_commit_oid(gitref)?
            .ok_or_else(|| DurableError::NotFound(format!("no such ref `{gitref}`")))?;

        let contents = match repo.read_blob_at_commit_oid_bounded(
            &snapshot_oid,
            path,
            BLAME_MAX_BLOB_BYTES,
        )? {
            BlobPathLookup::Found {
                bytes,
                is_binary: false,
                ..
            } => String::from_utf8_lossy(&bytes).into_owned(),
            BlobPathLookup::Found {
                is_binary: true, ..
            } => {
                return Err(DurableError::Git(
                    "blame unavailable: binary files cannot be attributed line by line".into(),
                ))
            }
            BlobPathLookup::TooLarge { size, .. } => {
                return Err(DurableError::Git(format!(
                    "blame limit exceeded: file is {size} bytes; maximum is {BLAME_MAX_BLOB_BYTES}"
                )))
            }
            BlobPathLookup::IsDir => {
                return Err(DurableError::NotFound(format!(
                    "`{path}` is a directory at `{gitref}`"
                )))
            }
            BlobPathLookup::Missing => {
                return Err(DurableError::NotFound(format!(
                    "no such file `{path}` at `{gitref}`"
                )))
            }
        };
        let line_count = repository_line_count(&contents);
        if line_count > BLAME_MAX_LINES {
            return Err(DurableError::Git(format!(
                "blame limit exceeded: file has {line_count} lines; maximum is {BLAME_MAX_LINES}"
            )));
        }

        let reader = GixCore::new(RootedResolver::new(self.root.clone()));
        let hunks = reader
            .blame_bounded(
                &loc,
                path,
                &snapshot_oid,
                BLAME_MAX_PATH_BYTES,
                BLAME_MAX_BLOB_BYTES,
                BLAME_MAX_HUNKS,
            )
            .map_err(|error| DurableError::Git(error.to_string()))?;
        validate_hunk_coverage(&contents, &hunks)?;

        let mut commits = BTreeMap::new();
        for hunk in &hunks {
            if !commits.contains_key(&hunk.commit) {
                let metadata = repo.commit_meta_at_oid(&hunk.commit)?.ok_or_else(|| {
                    DurableError::Git(format!(
                        "blame attribution references missing commit {}",
                        hunk.commit.as_str()
                    ))
                })?;
                commits.insert(hunk.commit.clone(), metadata);
            }
        }

        let hunks = hunks
            .into_iter()
            .map(|hunk| {
                let metadata = commits
                    .get(&hunk.commit)
                    .expect("metadata was loaded for every blame hunk");
                json!({
                    "start_line": hunk.final_start_line,
                    "line_count": hunk.lines,
                    "commit": {
                        "oid": metadata.oid,
                        "summary": metadata.summary,
                        "author": metadata.author_name,
                        "committed_at": metadata.time,
                    },
                })
            })
            .collect::<Vec<_>>();

        Ok(json!({
            "path": path,
            "ref": gitref,
            "snapshot_oid": snapshot_oid.as_str(),
            "contents": contents,
            "hunks": hunks,
        }))
    }
}

fn validate_hunk_coverage(
    contents: &str,
    hunks: &[myelin_git::core::BlameHunk],
) -> Result<(), DurableError> {
    let line_count = repository_line_count(contents);
    let mut expected_start = 1usize;
    for hunk in hunks {
        if hunk.lines == 0 || hunk.final_start_line != expected_start {
            return Err(DurableError::Git(
                "blame attribution does not form a contiguous file view".into(),
            ));
        }
        expected_start = expected_start
            .checked_add(hunk.lines)
            .ok_or_else(|| DurableError::Git("blame attribution line count overflow".into()))?;
    }
    if expected_start.saturating_sub(1) != line_count {
        return Err(DurableError::Git(format!(
            "blame attribution covers {} lines but the file has {line_count}",
            expected_start.saturating_sub(1)
        )));
    }
    Ok(())
}

fn repository_line_count(contents: &str) -> usize {
    if contents.is_empty() {
        return 0;
    }
    contents.bytes().filter(|byte| *byte == b'\n').count() + usize::from(!contents.ends_with('\n'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_git::core::BlameHunk;

    fn hunk(start: usize, lines: usize) -> BlameHunk {
        BlameHunk {
            final_start_line: start,
            lines,
            commit: myelin_git::core::Oid::new("a".repeat(40)),
        }
    }

    #[test]
    fn repository_line_count_matches_git_line_semantics() {
        assert_eq!(repository_line_count(""), 0);
        assert_eq!(repository_line_count("one"), 1);
        assert_eq!(repository_line_count("one\n"), 1);
        assert_eq!(repository_line_count("one\ntwo"), 2);
        assert_eq!(repository_line_count("one\ntwo\n"), 2);
    }

    #[test]
    fn coverage_requires_ordered_contiguous_hunks() {
        assert!(validate_hunk_coverage("one\ntwo\nthree\n", &[hunk(1, 2), hunk(3, 1)]).is_ok());
        assert!(validate_hunk_coverage("one\ntwo\nthree\n", &[hunk(1, 1), hunk(3, 2)]).is_err());
        assert!(validate_hunk_coverage("one\ntwo\nthree\n", &[hunk(1, 2)]).is_err());
    }
}
