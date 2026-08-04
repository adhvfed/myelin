use crate::JobKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitObjectFormat {
    Sha1,
    Sha256,
}

impl GitObjectFormat {
    pub(crate) fn hex_width(self) -> usize {
        match self {
            GitObjectFormat::Sha1 => 40,
            GitObjectFormat::Sha256 => 64,
        }
    }

    pub(crate) fn capability_token(self) -> Option<&'static str> {
        match self {
            GitObjectFormat::Sha1 => None,
            GitObjectFormat::Sha256 => Some("object-format=sha256"),
        }
    }

    pub(crate) fn init_token(self) -> &'static str {
        match self {
            GitObjectFormat::Sha1 => "sha1",
            GitObjectFormat::Sha256 => "sha256",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExpectedGitCommitId {
    hex: String,
    format: GitObjectFormat,
}

impl ExpectedGitCommitId {
    #[allow(dead_code)]
    pub(crate) fn new(hex: impl Into<String>, format: GitObjectFormat) -> Result<Self, String> {
        let hex = hex.into();
        if hex.len() != format.hex_width() {
            return Err(format!(
                "expected a {}-character lowercase-hex commit id for {format:?}, got {} \
                 characters",
                format.hex_width(),
                hex.len()
            ));
        }
        if !hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(format!("commit id {hex:?} is not lowercase hex"));
        }
        if hex.bytes().all(|b| b == b'0') {
            return Err(
                "commit id is the all-zero null id -- never a valid checkout target".to_string(),
            );
        }
        Ok(ExpectedGitCommitId { hex, format })
    }

    pub(crate) fn parse_exact(hex: impl Into<String>) -> Result<Self, String> {
        let hex = hex.into();
        let format = match hex.len() {
            40 => GitObjectFormat::Sha1,
            64 => GitObjectFormat::Sha256,
            other => {
                return Err(format!(
                    "expected a full 40-character (SHA-1) or 64-character (SHA-256) commit \
                     object id, got {other} characters -- a ref name or an abbreviated hash is \
                     never accepted"
                ))
            }
        };
        Self::new(hex, format)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.hex
    }

    pub(crate) fn format(&self) -> GitObjectFormat {
        self.format
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum WorkspaceIntent {
    Compute,
    Checkout(ValidatedCheckoutRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct ValidatedCheckoutRequest {
    artifact_ref: myelin_events::ArtifactRef,
    tenant: myelin_tenancy::TenantId,
    repo_id: String,
    commit: ExpectedGitCommitId,
}

impl ValidatedCheckoutRequest {
    #[allow(dead_code)]
    pub(crate) fn artifact_ref(&self) -> &myelin_events::ArtifactRef {
        &self.artifact_ref
    }

    #[allow(dead_code)]
    pub(crate) fn tenant(&self) -> &myelin_tenancy::TenantId {
        &self.tenant
    }

    #[allow(dead_code)]
    pub(crate) fn repo_id(&self) -> &str {
        &self.repo_id
    }

    #[allow(dead_code)]
    pub(crate) fn commit(&self) -> &ExpectedGitCommitId {
        &self.commit
    }

    #[allow(dead_code)]
    pub(crate) fn to_authorization_scope(&self) -> crate::CheckoutAuthorizationScope {
        crate::CheckoutAuthorizationScope::new(
            self.tenant.clone(),
            self.artifact_ref.clone(),
            self.repo_id.clone(),
            self.commit.as_str().to_string(),
            self.commit.format(),
        )
    }
}

#[allow(dead_code)]
pub(crate) fn derive_workspace_intent(
    kind: JobKind,
    workspace: &crate::WorkspaceSpec,
) -> Result<WorkspaceIntent, String> {
    let (repo_ref, commit) = match (&workspace.repo_ref, &workspace.commit) {
        (None, None) => return Ok(WorkspaceIntent::Compute),
        (Some(_), None) | (None, Some(_)) => {
            return Err(
                "WorkspaceSpec must set both repo_ref and commit, or neither -- a mixed \
                 combination is refused"
                    .to_string(),
            )
        }
        (Some(repo_ref), Some(commit)) => (repo_ref, commit),
    };
    if kind != JobKind::Ci {
        return Err(
            "checkout-bearing jobs are CI-only today -- an agent job's WorkspaceSpec must be \
             (None, None)"
                .to_string(),
        );
    }
    let parsed = myelin_refs::parse_scoped(repo_ref)
        .map_err(|e| format!("repo_ref {repo_ref:?} is not a valid scoped artifact ref: {e}"))?;
    if parsed.subsystem != "git" {
        return Err(format!(
            "repo_ref {repo_ref:?} names subsystem {:?}, expected \"git\"",
            parsed.subsystem
        ));
    }
    if parsed.type_ != "repo" {
        return Err(format!(
            "repo_ref {repo_ref:?} names type {:?}, expected \"repo\"",
            parsed.type_
        ));
    }
    if parsed.sub.is_some() {
        return Err(format!(
            "repo_ref {repo_ref:?} carries a #sub component -- a checkout target must name the \
             bare repo, never a sub-resource"
        ));
    }
    let commit = ExpectedGitCommitId::parse_exact(commit.clone())
        .map_err(|e| format!("commit {commit:?} is invalid: {e}"))?;
    Ok(WorkspaceIntent::Checkout(ValidatedCheckoutRequest {
        artifact_ref: parsed.artifact_ref,
        tenant: parsed.tenant,
        repo_id: parsed.id,
        commit,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha1_oid(byte: u8) -> String {
        format!("{byte:02x}").repeat(20)
    }

    fn workspace_spec(repo_ref: Option<&str>, commit: Option<&str>) -> crate::WorkspaceSpec {
        crate::WorkspaceSpec {
            repo_ref: repo_ref.map(str::to_string),
            commit: commit.map(str::to_string),
        }
    }

    #[test]
    fn compute_job_with_no_workspace_fields_is_compute() {
        let intent = derive_workspace_intent(JobKind::Ci, &workspace_spec(None, None)).unwrap();
        assert_eq!(intent, WorkspaceIntent::Compute);
    }

    #[test]
    fn agent_job_with_no_workspace_fields_is_compute() {
        let intent = derive_workspace_intent(JobKind::Agent, &workspace_spec(None, None)).unwrap();
        assert_eq!(intent, WorkspaceIntent::Compute);
    }

    #[test]
    fn mixed_repo_ref_only_is_refused() {
        let err = derive_workspace_intent(
            JobKind::Ci,
            &workspace_spec(Some("myelin://acme/git/repo/widgets"), None),
        )
        .unwrap_err();
        assert!(err.contains("mixed combination"));
    }

    #[test]
    fn mixed_commit_only_is_refused() {
        let oid = sha1_oid(0x11);
        let err =
            derive_workspace_intent(JobKind::Ci, &workspace_spec(None, Some(&oid))).unwrap_err();
        assert!(err.contains("mixed combination"));
    }

    #[test]
    fn checkout_is_refused_for_agent_jobs() {
        let oid = sha1_oid(0x22);
        let err = derive_workspace_intent(
            JobKind::Agent,
            &workspace_spec(Some("myelin://acme/git/repo/widgets"), Some(&oid)),
        )
        .unwrap_err();
        assert!(err.contains("CI-only"));
    }

    #[test]
    fn valid_sha1_checkout_request_is_accepted_and_preserves_identity() {
        let oid = sha1_oid(0x33);
        let intent = derive_workspace_intent(
            JobKind::Ci,
            &workspace_spec(Some("myelin://acme/git/repo/widgets"), Some(&oid)),
        )
        .unwrap();
        let WorkspaceIntent::Checkout(request) = intent else {
            panic!("expected Checkout");
        };
        assert_eq!(request.tenant().0, "acme");
        assert_eq!(request.repo_id(), "widgets");
        assert_eq!(request.commit().as_str(), oid);
        assert_eq!(request.commit().format(), GitObjectFormat::Sha1);
        assert_eq!(request.artifact_ref().0, "myelin://acme/git/repo/widgets");

        let scope = request.to_authorization_scope();
        assert_eq!(scope.tenant().0, "acme");
        assert_eq!(scope.repo_ref().0, "myelin://acme/git/repo/widgets");
        assert_eq!(scope.repo_id(), "widgets");
        assert_eq!(scope.commit_hex(), oid);
        assert_eq!(scope.commit_format(), GitObjectFormat::Sha1);
    }

    #[test]
    fn valid_sha256_checkout_request_is_accepted() {
        let oid = "a".repeat(64);
        let intent = derive_workspace_intent(
            JobKind::Ci,
            &workspace_spec(Some("myelin://acme/git/repo/widgets"), Some(&oid)),
        )
        .unwrap();
        let WorkspaceIntent::Checkout(request) = intent else {
            panic!("expected Checkout");
        };
        assert_eq!(request.commit().format(), GitObjectFormat::Sha256);
    }

    #[test]
    fn wrong_subsystem_is_refused() {
        let oid = sha1_oid(0x44);
        let err = derive_workspace_intent(
            JobKind::Ci,
            &workspace_spec(Some("myelin://acme/notif/repo/widgets"), Some(&oid)),
        )
        .unwrap_err();
        assert!(err.contains("subsystem"));
    }

    #[test]
    fn wrong_type_is_refused() {
        let oid = sha1_oid(0x55);
        let err = derive_workspace_intent(
            JobKind::Ci,
            &workspace_spec(Some("myelin://acme/git/check/widgets"), Some(&oid)),
        )
        .unwrap_err();
        assert!(err.contains("type"));
    }

    #[test]
    fn a_sub_component_is_refused() {
        let oid = sha1_oid(0x66);
        let err = derive_workspace_intent(
            JobKind::Ci,
            &workspace_spec(
                Some("myelin://acme/git/repo/widgets#check:ci:build"),
                Some(&oid),
            ),
        )
        .unwrap_err();
        assert!(err.contains("#sub"));
    }

    #[test]
    fn a_malformed_ref_is_refused() {
        let oid = sha1_oid(0x77);
        let err =
            derive_workspace_intent(JobKind::Ci, &workspace_spec(Some("not-a-ref"), Some(&oid)))
                .unwrap_err();
        assert!(err.contains("not a valid scoped artifact ref"));
    }

    #[test]
    fn an_abbreviated_hash_is_refused() {
        let err = derive_workspace_intent(
            JobKind::Ci,
            &workspace_spec(Some("myelin://acme/git/repo/widgets"), Some("abc1234")),
        )
        .unwrap_err();
        assert!(err.contains("40-character"));
    }

    #[test]
    fn a_ref_like_commit_is_refused() {
        let err = derive_workspace_intent(
            JobKind::Ci,
            &workspace_spec(
                Some("myelin://acme/git/repo/widgets"),
                Some("refs/heads/main"),
            ),
        )
        .unwrap_err();
        assert!(err.contains("40-character"));
    }

    #[test]
    fn an_uppercase_commit_is_refused() {
        let oid = "A".repeat(40);
        let err = derive_workspace_intent(
            JobKind::Ci,
            &workspace_spec(Some("myelin://acme/git/repo/widgets"), Some(&oid)),
        )
        .unwrap_err();
        assert!(err.contains("not lowercase hex"));
    }

    #[test]
    fn an_all_zero_commit_is_refused() {
        let oid = "0".repeat(40);
        let err = derive_workspace_intent(
            JobKind::Ci,
            &workspace_spec(Some("myelin://acme/git/repo/widgets"), Some(&oid)),
        )
        .unwrap_err();
        assert!(err.contains("all-zero"));
    }
}
