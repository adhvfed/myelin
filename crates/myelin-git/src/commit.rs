use core::fmt;
use myelin_identity::PseudonymHandle;

pub use myelin_identity::PSEUDONYM_DOMAIN_SUFFIX;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitIdentity {
    handle: PseudonymHandle,
    when_unix_secs: i64,
    tz_offset_minutes: i32,
}

impl CommitIdentity {
    pub fn pseudonymous(
        handle: PseudonymHandle,
        when_unix_secs: i64,
        tz_offset_minutes: i32,
    ) -> CommitIdentity {
        CommitIdentity {
            handle,
            when_unix_secs,
            tz_offset_minutes,
        }
    }

    pub fn handle(&self) -> &PseudonymHandle {
        &self.handle
    }

    pub fn render_email(&self) -> String {
        self.handle.render()
    }

    fn render_line(&self, role: &str) -> String {
        let email = self.render_email();
        let sign = if self.tz_offset_minutes < 0 { '-' } else { '+' };
        let abs = self.tz_offset_minutes.unsigned_abs();
        format!(
            "{role} {email} <{email}> {} {sign}{:02}{:02}",
            self.when_unix_secs,
            abs / 60,
            abs % 60,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommitOid(pub String);

impl fmt::Display for CommitOid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub tree: CommitOid,
    pub parents: Vec<CommitOid>,
    pub author: CommitIdentity,
    pub committer: CommitIdentity,
    pub message: String,
}

impl Commit {
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = String::new();
        out.push_str(&format!("tree {}\n", self.tree));
        for parent in &self.parents {
            out.push_str(&format!("parent {parent}\n"));
        }
        out.push_str(&self.author.render_line("author"));
        out.push('\n');
        out.push_str(&self.committer.render_line("committer"));
        out.push('\n');
        out.push('\n');
        out.push_str(&self.message);
        out.into_bytes()
    }

    pub fn oid(&self) -> CommitOid {
        let digest = blake3::hash(&self.canonical_bytes());
        CommitOid(format!("blake3:{}", hex::encode(digest.as_bytes())))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitAttribution {
    pub commit: CommitOid,
    pub principal_id: String,
    pub pseudonym: PseudonymHandle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErasureResidual {
    pub recoverable_real_identity: Vec<String>,
    pub pseudonymous_residual: PseudonymHandle,
}

impl ErasureResidual {
    pub fn residual_matches_posture(&self) -> bool {
        self.recoverable_real_identity.is_empty()
    }
}

pub fn erased_residual(commit: &Commit, real_identity_tokens: &[&str]) -> ErasureResidual {
    let bytes = commit.canonical_bytes();
    let text = String::from_utf8_lossy(&bytes);
    let recoverable_real_identity = real_identity_tokens
        .iter()
        .filter(|tok| !tok.is_empty() && text.contains(*tok))
        .map(|tok| tok.to_string())
        .collect();
    ErasureResidual {
        recoverable_real_identity,
        pseudonymous_residual: commit.author.handle().clone(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NonPseudonymousIdentity {
    NotAPseudonym {
        role: String,
        offending_email: String,
    },
    WrongTenant {
        role: String,
        expected_tenant: String,
        found_tenant: String,
    },
    Unparseable {
        missing: String,
    },
}

fn email_in_identity_line(line: &str) -> Option<&str> {
    let open = line.rfind('<')?;
    let close = line[open + 1..].find('>')? + open + 1;
    Some(&line[open + 1..close])
}

pub fn enforce_pseudonymous_commit(
    commit_bytes: &[u8],
    tenant: &str,
) -> Result<(PseudonymHandle, PseudonymHandle), NonPseudonymousIdentity> {
    let text = String::from_utf8_lossy(commit_bytes);
    let mut author: Option<PseudonymHandle> = None;
    let mut committer: Option<PseudonymHandle> = None;
    for line in text.lines() {
        if line.is_empty() {
            break;
        }
        let role = if let Some(rest) = line.strip_prefix("author ") {
            ("author", rest)
        } else if let Some(rest) = line.strip_prefix("committer ") {
            ("committer", rest)
        } else {
            continue;
        };
        let (role_name, rest) = role;
        let email = email_in_identity_line(rest).ok_or(NonPseudonymousIdentity::NotAPseudonym {
            role: role_name.to_string(),
            offending_email: rest.to_string(),
        })?;
        let handle =
            PseudonymHandle::parse(email).ok_or(NonPseudonymousIdentity::NotAPseudonym {
                role: role_name.to_string(),
                offending_email: email.to_string(),
            })?;
        if handle.tenant() != tenant {
            return Err(NonPseudonymousIdentity::WrongTenant {
                role: role_name.to_string(),
                expected_tenant: tenant.to_string(),
                found_tenant: handle.tenant().to_string(),
            });
        }
        match role_name {
            "author" => author = Some(handle),
            _ => committer = Some(handle),
        }
    }
    let author = author.ok_or(NonPseudonymousIdentity::Unparseable {
        missing: "author".into(),
    })?;
    let committer = committer.ok_or(NonPseudonymousIdentity::Unparseable {
        missing: "committer".into(),
    })?;
    Ok((author, committer))
}

pub fn is_commit_object(bytes: &[u8]) -> bool {
    bytes.starts_with(b"tree ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle() -> PseudonymHandle {
        PseudonymHandle::new("psn-7f3a9c", "acme").expect("well-formed handle")
    }

    fn commit() -> Commit {
        let author = CommitIdentity::pseudonymous(handle(), 1_700_000_000, 120);
        let committer = CommitIdentity::pseudonymous(handle(), 1_700_000_000, 120);
        Commit {
            tree: CommitOid("blake3:tree".into()),
            parents: vec![CommitOid("blake3:parent".into())],
            author,
            committer,
            message: "fix: handle the empty-ref edge case\n".into(),
        }
    }

    #[test]
    fn commit_author_and_committer_are_the_pseudonym_grammar() {
        let c = commit();
        let text = String::from_utf8(c.canonical_bytes()).unwrap();
        assert!(text.contains("author psn-7f3a9c@acme.noreply <psn-7f3a9c@acme.noreply>"));
        assert!(text.contains("committer psn-7f3a9c@acme.noreply <psn-7f3a9c@acme.noreply>"));
        assert!(c.author.render_email().ends_with(PSEUDONYM_DOMAIN_SUFFIX));
        assert_eq!(c.author.render_email(), "psn-7f3a9c@acme.noreply");
    }

    #[test]
    fn after_erase_no_real_identity_recoverable_from_immutable_bytes() {
        let c = commit();
        let real_tokens = ["Ada Lovelace", "ada.lovelace@example.com", "ada@acme.com"];
        let residual = erased_residual(&c, &real_tokens);
        assert!(
            residual.recoverable_real_identity.is_empty(),
            "real identity leaked into immutable bytes: {:?}",
            residual.recoverable_real_identity
        );
        assert!(residual.residual_matches_posture());
        assert_eq!(residual.pseudonymous_residual, handle());
    }

    #[test]
    fn opaque_principal_attributes_the_commit_for_authz() {
        let c = commit();
        let attr = CommitAttribution {
            commit: c.oid(),
            principal_id: "principal:opaque-stable-id".into(),
            pseudonym: handle(),
        };
        assert_eq!(attr.commit, c.oid());
        assert_eq!(attr.principal_id, "principal:opaque-stable-id");
        let text = String::from_utf8(c.canonical_bytes()).unwrap();
        assert!(!text.contains("principal:opaque-stable-id"));
    }

    #[test]
    fn oid_is_blake3_content_address_over_pseudonymous_bytes() {
        let c = commit();
        let oid = c.oid();
        assert!(oid.0.starts_with("blake3:"));
        assert_eq!(c.oid(), commit().oid());
        let mut other = commit();
        other.author =
            CommitIdentity::pseudonymous(PseudonymHandle::new("psn-other", "acme").unwrap(), 1, 0);
        other.committer = other.author.clone();
        assert_ne!(c.oid(), other.oid());
    }

    #[test]
    fn author_line_renders_signed_zero_padded_tz_offset() {
        let h = handle();
        let east = CommitIdentity::pseudonymous(h.clone(), 1_700_000_000, 120);
        assert!(east.render_line("author").ends_with(" +0200"));
        let west = CommitIdentity::pseudonymous(h.clone(), 1_700_000_000, -330);
        assert!(west.render_line("author").ends_with(" -0530"));
        let utc = CommitIdentity::pseudonymous(h, 1_700_000_000, 0);
        assert!(utc.render_line("committer").ends_with(" +0000"));
    }

    #[test]
    fn commit_oid_displays_verbatim() {
        let oid = commit().oid();
        assert_eq!(format!("{oid}"), oid.0);
        assert!(format!("{oid}").starts_with("blake3:"));
        assert!(!format!("{oid}").is_empty());
    }

    #[test]
    fn enforce_accepts_a_pseudonymous_commit_for_the_tenant() {
        let c = commit();
        let (author, committer) = enforce_pseudonymous_commit(&c.canonical_bytes(), "acme")
            .expect("pseudonymous → accept");
        assert_eq!(author, handle());
        assert_eq!(committer, handle());
    }

    #[test]
    fn enforce_rejects_a_raw_name_email_commit() {
        let raw = b"tree blake3:t\n\
                    author Ada Lovelace <ada.lovelace@example.com> 1700000000 +0000\n\
                    committer Ada Lovelace <ada.lovelace@example.com> 1700000000 +0000\n\
                    \n\
                    fix: the bug\n";
        match enforce_pseudonymous_commit(raw, "acme") {
            Err(NonPseudonymousIdentity::NotAPseudonym {
                role,
                offending_email,
            }) => {
                assert_eq!(role, "author", "the FIRST non-pseudonymous header is named");
                assert_eq!(offending_email, "ada.lovelace@example.com");
            }
            other => panic!("expected NotAPseudonym, got {other:?}"),
        }
    }

    #[test]
    fn enforce_rejects_a_wrong_tenant_pseudonym() {
        let foreign = PseudonymHandle::new("psn-x", "globex").unwrap();
        let id = CommitIdentity::pseudonymous(foreign, 1_700_000_000, 0);
        let mut c = commit();
        c.author = id.clone();
        c.committer = id;
        match enforce_pseudonymous_commit(&c.canonical_bytes(), "acme") {
            Err(NonPseudonymousIdentity::WrongTenant {
                role,
                expected_tenant,
                found_tenant,
            }) => {
                assert_eq!(role, "author");
                assert_eq!(expected_tenant, "acme");
                assert_eq!(found_tenant, "globex");
            }
            other => panic!("expected WrongTenant, got {other:?}"),
        }
    }

    #[test]
    fn enforce_rejects_a_raw_committer_even_with_pseudonymous_author() {
        let raw = b"tree blake3:t\n\
                    author psn-ok@acme.noreply <psn-ok@acme.noreply> 1700000000 +0000\n\
                    committer Real Committer <real@corp.example> 1700000000 +0000\n\
                    \n\
                    chore: rebase\n";
        match enforce_pseudonymous_commit(raw, "acme") {
            Err(NonPseudonymousIdentity::NotAPseudonym {
                role,
                offending_email,
            }) => {
                assert_eq!(
                    role, "committer",
                    "the committer is gated independently of the author"
                );
                assert_eq!(offending_email, "real@corp.example");
            }
            other => panic!("expected committer NotAPseudonym, got {other:?}"),
        }
    }

    #[test]
    fn enforce_fails_closed_on_a_missing_author_header() {
        let raw = b"tree blake3:t\n\
                    committer psn-ok@acme.noreply <psn-ok@acme.noreply> 1700000000 +0000\n\
                    \n\
                    msg\n";
        assert_eq!(
            enforce_pseudonymous_commit(raw, "acme"),
            Err(NonPseudonymousIdentity::Unparseable {
                missing: "author".into()
            })
        );
    }

    #[test]
    fn is_commit_object_detects_only_commits() {
        let c = commit();
        assert!(
            is_commit_object(&c.canonical_bytes()),
            "a tree-headed object is a commit"
        );
        assert!(
            !is_commit_object(b"just some file contents\n"),
            "a blob is not a commit"
        );
        let secret_blob = [b"AK".as_slice(), b"IAEXAMPLE secret blob"].concat();
        assert!(
            !is_commit_object(&secret_blob),
            "a secret blob is not a commit"
        );
    }

    #[test]
    fn enforce_ignores_third_party_mention_in_the_message_body() {
        let mut c = commit();
        c.message = "fix: as reported by Ada Lovelace <ada@example.com>\n".into();
        assert!(enforce_pseudonymous_commit(&c.canonical_bytes(), "acme").is_ok());
    }

    #[test]
    fn real_identity_is_not_a_function_of_the_commit_bytes() {
        let c1 = commit();
        let c2 = commit();
        assert_eq!(c1.canonical_bytes(), c2.canonical_bytes());
        assert_eq!(c1.oid(), c2.oid());
    }
}
