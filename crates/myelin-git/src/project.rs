use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, Permission, Principal, Zookie,
};
use myelin_refs::ArtifactRef;
use std::collections::HashMap;

use crate::check_status::GateOutcome;
use crate::lifecycle::{PrState, PullRequest, Review, ReviewState, ReviewVerdict};

pub const VIEW: &str = "view";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitRefError {
    InvalidComponent { component: &'static str },
    Parse(myelin_refs::ParseError),
}

impl std::fmt::Display for GitRefError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitRefError::InvalidComponent { component } => write!(
                f,
                "git artifact reference component `{component}` is empty or contains characters \
                 outside its canonical grammar"
            ),
            GitRefError::Parse(error) => write!(f, "invalid git artifact reference: {error}"),
        }
    }
}

impl std::error::Error for GitRefError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GitRefError::InvalidComponent { .. } => None,
            GitRefError::Parse(error) => Some(error),
        }
    }
}

impl From<myelin_refs::ParseError> for GitRefError {
    fn from(error: myelin_refs::ParseError) -> Self {
        GitRefError::Parse(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitArtifactType {
    Pr,
    Commit,
    Review,
    Repo,
    Blob,
}

fn classify(r: &ArtifactRef) -> Result<GitArtifactType, ProjectError> {
    let parsed = myelin_refs::parse_scoped(&r.0).map_err(|_| ProjectError::NotAGitArtifact {
        reference: r.0.clone(),
    })?;
    if parsed.subsystem != "git" {
        return Err(ProjectError::NotAGitArtifact {
            reference: r.0.clone(),
        });
    }
    match parsed.type_.as_str() {
        "pr" => Ok(GitArtifactType::Pr),
        "commit" => Ok(GitArtifactType::Commit),
        "review" => Ok(GitArtifactType::Review),
        "repo" => Ok(GitArtifactType::Repo),
        "blob" => Ok(GitArtifactType::Blob),
        other => Err(ProjectError::UnknownGitType {
            ty: other.to_string(),
        }),
    }
}

pub fn git_pr_ref(tenant: &str, repo: &str, number: u64) -> Result<ArtifactRef, GitRefError> {
    validate_ref_components(&[("tenant", tenant)])?;
    validate_repo(repo, "repo")?;
    parse_git(&format!("myelin://{tenant}/git/pr/{repo}:{number}"))
}

pub fn git_commit_ref(tenant: &str, repo: &str, sha: &str) -> Result<ArtifactRef, GitRefError> {
    validate_ref_components(&[("tenant", tenant), ("sha", sha)])?;
    validate_repo(repo, "repo")?;
    parse_git(&format!("myelin://{tenant}/git/commit/{repo}:{sha}"))
}

pub fn git_review_ref(
    tenant: &str,
    repo: &str,
    pr_number: u64,
    reviewer_pseudonym: &str,
) -> Result<ArtifactRef, GitRefError> {
    validate_ref_components(&[
        ("tenant", tenant),
        ("reviewer_pseudonym", reviewer_pseudonym),
    ])?;
    validate_repo(repo, "repo")?;
    parse_git(&format!(
        "myelin://{tenant}/git/review/{repo}:{pr_number}:{reviewer_pseudonym}"
    ))
}

pub fn git_repo_ref(tenant: &str, repo_id: &str) -> Result<ArtifactRef, GitRefError> {
    validate_ref_components(&[("tenant", tenant)])?;
    validate_repo(repo_id, "repo_id")?;
    parse_git(&format!("myelin://{tenant}/git/repo/{repo_id}"))
}

fn validate_ref_components(components: &[(&'static str, &str)]) -> Result<(), GitRefError> {
    for (component, value) in components {
        if value.is_empty() || value.contains(['/', '#']) {
            return Err(GitRefError::InvalidComponent { component });
        }
    }
    Ok(())
}

fn validate_repo(value: &str, component: &'static str) -> Result<(), GitRefError> {
    if value.split('/').any(|part| {
        part.is_empty()
            || part == "."
            || part == ".."
            || !part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    }) || value.contains('#')
    {
        return Err(GitRefError::InvalidComponent { component });
    }
    Ok(())
}

fn parse_git(urn: &str) -> Result<ArtifactRef, GitRefError> {
    myelin_refs::parse(urn).map_err(GitRefError::from)
}

pub fn display_key(r: &ArtifactRef) -> Option<String> {
    let ty = classify(r).ok()?;
    let id = canonical_id(r)?;
    match ty {
        GitArtifactType::Pr => id.rsplit(':').next().map(|n| format!("#{n}")),
        GitArtifactType::Commit => id.rsplit(':').next().map(short_sha),
        GitArtifactType::Repo | GitArtifactType::Review | GitArtifactType::Blob => None,
    }
}

fn canonical_id(r: &ArtifactRef) -> Option<String> {
    let parsed = myelin_refs::parse_scoped(&r.0).ok()?;
    (parsed.subsystem == "git").then_some(parsed.id)
}

fn short_sha(sha: &str) -> String {
    let hex = sha.split_once(':').map(|(_, h)| h).unwrap_or(sha);
    hex.chars().take(7).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Projection {
    pub title: String,
    pub state: String,
    pub icon: String,
    pub render_hint: Option<RenderHint>,
    pub sub_anchor: Option<SubAnchor>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderHint {
    pub checks: ChecksSummary,
    pub approvals: (u32, u32),
    pub is_draft: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChecksSummary {
    Green,
    Red,
    Neutral,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubAnchor {
    pub kind: String,
    pub excerpt: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tombstone {
    pub reason: TombstoneReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TombstoneReason {
    Unauthorized,
    Erased,
    Restricted,
    ContentGone,
}

impl Tombstone {
    pub fn display_text(&self) -> &'static str {
        "(not available)"
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Projected {
    Visible(Projection),
    Tombstoned(Tombstone),
}

impl Projected {
    pub fn is_visible(&self) -> bool {
        matches!(self, Projected::Visible(_))
    }

    pub fn is_tombstone(&self) -> bool {
        matches!(self, Projected::Tombstoned(_))
    }

    pub fn title(&self) -> Option<&str> {
        match self {
            Projected::Visible(p) => Some(&p.title),
            Projected::Tombstoned(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectError {
    NotAGitArtifact { reference: String },
    UnknownGitType { ty: String },
    NotFound { reference: String },
    BlobFloor { reference: String },
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectError::NotAGitArtifact { reference } => {
                write!(
                    f,
                    "not a git artifact: `{reference}` - git's projector does not own this ref"
                )
            }
            ProjectError::UnknownGitType { ty } => {
                write!(f, "unknown git artifact type `{ty}`")
            }
            ProjectError::NotFound { reference } => {
                write!(f, "no git artifact found for `{reference}` (dangling ref)")
            }
            ProjectError::BlobFloor { reference } => write!(
                f,
                "blob projection for `{reference}` is the GIT-P24 content-anchored resolver floor"
            ),
        }
    }
}

impl std::error::Error for ProjectError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoMeta {
    pub slug: String,
    pub visibility: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitMeta {
    pub subject: String,
    pub verified: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ArtifactStore {
    prs: HashMap<String, PullRequest>,
    pr_render: HashMap<String, (GateOutcome, u32, u32)>,
    reviews: HashMap<String, Review>,
    commits: HashMap<String, CommitMeta>,
    repos: HashMap<String, RepoMeta>,
    comments: HashMap<String, String>,
    erased: std::collections::HashSet<String>,
    restricted: std::collections::HashSet<String>,
}

impl ArtifactStore {
    pub fn new() -> ArtifactStore {
        ArtifactStore::default()
    }

    pub fn put_pr(
        &mut self,
        canonical_ref: &ArtifactRef,
        pr: PullRequest,
        gate: GateOutcome,
        current_approvals: u32,
        required_approvals: u32,
    ) {
        self.pr_render.insert(
            canonical_ref.0.clone(),
            (gate, current_approvals, required_approvals),
        );
        self.prs.insert(canonical_ref.0.clone(), pr);
    }

    pub fn put_review(&mut self, canonical_ref: &ArtifactRef, review: Review) {
        self.reviews.insert(canonical_ref.0.clone(), review);
    }

    pub fn put_commit(&mut self, canonical_ref: &ArtifactRef, meta: CommitMeta) {
        self.commits.insert(canonical_ref.0.clone(), meta);
    }

    pub fn put_repo(&mut self, canonical_ref: &ArtifactRef, meta: RepoMeta) {
        self.repos.insert(canonical_ref.0.clone(), meta);
    }

    pub fn put_comment_excerpt(&mut self, comment_ref: &ArtifactRef, excerpt: impl Into<String>) {
        self.comments.insert(comment_ref.0.clone(), excerpt.into());
    }

    pub fn mark_erased(&mut self, canonical_ref: &ArtifactRef) {
        self.erased.insert(canonical_ref.0.clone());
    }

    pub fn mark_restricted(&mut self, canonical_ref: &ArtifactRef) {
        self.restricted.insert(canonical_ref.0.clone());
    }
}

pub struct Projector<I: IdentityService> {
    id: I,
    store: ArtifactStore,
}

impl<I: IdentityService> Projector<I> {
    pub fn new(id: I, store: ArtifactStore) -> Projector<I> {
        Projector { id, store }
    }

    pub fn store_mut(&mut self) -> &mut ArtifactStore {
        &mut self.store
    }

    pub fn project(
        &self,
        reference: &ArtifactRef,
        viewer: &Principal,
        zookie: Zookie,
    ) -> Result<Projected, ProjectError> {
        let ty = classify(reference)?;
        if ty == GitArtifactType::Blob {
            return Err(ProjectError::BlobFloor {
                reference: reference.0.clone(),
            });
        }

        let acl_object = myelin_refs::strip_sub(reference);
        let at = Consistency {
            at_least: zookie,
            mode: ConsistencyMode::Strong,
        };
        let permission = Permission(VIEW.to_string());
        let decision = self.id.check(viewer, &permission, &acl_object, &at, None);
        match decision {
            Ok(Decision::Allow) => {}
            Ok(Decision::Deny) | Ok(Decision::Conditional) | Err(_) => {
                return Ok(Projected::Tombstoned(Tombstone {
                    reason: TombstoneReason::Unauthorized,
                }));
            }
        }

        if self.store.erased.contains(&acl_object.0) || self.store.erased.contains(&reference.0) {
            return Ok(Projected::Tombstoned(Tombstone {
                reason: TombstoneReason::Erased,
            }));
        }
        if self.store.restricted.contains(&acl_object.0)
            || self.store.restricted.contains(&reference.0)
        {
            return Ok(Projected::Tombstoned(Tombstone {
                reason: TombstoneReason::Restricted,
            }));
        }

        let sub_anchor = self.project_sub_anchor(reference);
        let projection = match ty {
            GitArtifactType::Pr => self.project_pr(&acl_object, sub_anchor)?,
            GitArtifactType::Commit => self.project_commit(&acl_object)?,
            GitArtifactType::Review => self.project_review(&acl_object)?,
            GitArtifactType::Repo => self.project_repo(&acl_object)?,
            GitArtifactType::Blob => unreachable!("blob handled as the GIT-P24 floor above"),
        };
        Ok(Projected::Visible(projection))
    }

    fn project_pr(
        &self,
        root: &ArtifactRef,
        sub_anchor: Option<SubAnchor>,
    ) -> Result<Projection, ProjectError> {
        let pr = self
            .store
            .prs
            .get(&root.0)
            .ok_or_else(|| ProjectError::NotFound {
                reference: root.0.clone(),
            })?;
        let (gate, current, required) = self.store.pr_render.get(&root.0).cloned().unwrap_or((
            GateOutcome::AllRequiredGreen,
            0,
            0,
        ));
        let checks = match (&gate, required) {
            (_, 0) => ChecksSummary::Neutral,
            (GateOutcome::AllRequiredGreen, _) => ChecksSummary::Green,
            (GateOutcome::Blocked { .. }, _) => ChecksSummary::Red,
        };
        Ok(Projection {
            title: pr_title(pr),
            state: pr_state_token(pr.state).to_string(),
            icon: "pr".to_string(),
            render_hint: Some(RenderHint {
                checks,
                approvals: (current, required),
                is_draft: pr.state == PrState::Draft,
            }),
            sub_anchor,
        })
    }

    fn project_commit(&self, root: &ArtifactRef) -> Result<Projection, ProjectError> {
        let meta = self
            .store
            .commits
            .get(&root.0)
            .ok_or_else(|| ProjectError::NotFound {
                reference: root.0.clone(),
            })?;
        let short = canonical_id(root)
            .and_then(|id| id.rsplit(':').next().map(short_sha))
            .unwrap_or_default();
        Ok(Projection {
            title: format!("{short} {}", meta.subject),
            state: if meta.verified {
                "verified"
            } else {
                "unverified"
            }
            .to_string(),
            icon: "commit".to_string(),
            render_hint: None,
            sub_anchor: None,
        })
    }

    fn project_review(&self, root: &ArtifactRef) -> Result<Projection, ProjectError> {
        let review = self
            .store
            .reviews
            .get(&root.0)
            .ok_or_else(|| ProjectError::NotFound {
                reference: root.0.clone(),
            })?;
        Ok(Projection {
            title: review_title(review),
            state: review_state_token(&review.state).to_string(),
            icon: "review".to_string(),
            render_hint: None,
            sub_anchor: None,
        })
    }

    fn project_repo(&self, root: &ArtifactRef) -> Result<Projection, ProjectError> {
        let meta = self
            .store
            .repos
            .get(&root.0)
            .ok_or_else(|| ProjectError::NotFound {
                reference: root.0.clone(),
            })?;
        Ok(Projection {
            title: meta.slug.clone(),
            state: meta.visibility.clone(),
            icon: "repo".to_string(),
            render_hint: None,
            sub_anchor: None,
        })
    }

    fn project_sub_anchor(&self, reference: &ArtifactRef) -> Option<SubAnchor> {
        let sub = myelin_refs::sub_kind(reference)?;
        match sub {
            myelin_refs::Sub::Comment(_) => Some(SubAnchor {
                kind: "comment".to_string(),
                excerpt: self
                    .store
                    .comments
                    .get(&reference.0)
                    .cloned()
                    .unwrap_or_default(),
            }),
            myelin_refs::Sub::Thread(_) => Some(SubAnchor {
                kind: "thread".to_string(),
                excerpt: self
                    .store
                    .comments
                    .get(&reference.0)
                    .cloned()
                    .unwrap_or_default(),
            }),
            _ => None,
        }
    }
}

fn pr_title(pr: &PullRequest) -> String {
    let first_line = pr.body.md.lines().find(|l| !l.trim().is_empty());
    match first_line {
        Some(line) => line.trim().to_string(),
        None => format!("PR #{}", pr.number),
    }
}

fn pr_state_token(state: PrState) -> &'static str {
    match state {
        PrState::Draft => "draft",
        PrState::Open => "open",
        PrState::Merged => "merged",
        PrState::Closed => "closed",
    }
}

fn review_title(review: &Review) -> String {
    match review.state {
        ReviewState::Requested => "review requested".to_string(),
        ReviewState::Submitted(ReviewVerdict::Approve) => "approved".to_string(),
        ReviewState::Submitted(ReviewVerdict::RequestChanges) => "changes requested".to_string(),
        ReviewState::Submitted(ReviewVerdict::Comment) => "commented".to_string(),
        ReviewState::Dismissed => "dismissed".to_string(),
    }
}

fn review_state_token(state: &ReviewState) -> &'static str {
    match state {
        ReviewState::Requested => "requested",
        ReviewState::Submitted(_) => "submitted",
        ReviewState::Dismissed => "dismissed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::Body;
    use crate::check_status::{CheckContext, GateOutcome};
    use myelin_identity::{
        AuthzError, CaveatContext, Credential, ListObjectsResult, ObjectId, ObjectType,
        PrincipalId, PrincipalKind, Result as IdResult, RewriteTrace, SubjectTree, TupleDelta,
    };
    use myelin_tenancy::{Region, TenantId};
    use std::collections::HashSet;

    struct StubId {
        allow: HashSet<String>,
        hiccup: bool,
    }

    impl StubId {
        fn new() -> Self {
            Self {
                allow: HashSet::new(),
                hiccup: false,
            }
        }
        fn allow_view(mut self, object: &ArtifactRef) -> Self {
            self.allow.insert(format!("view@{}", object.0));
            self
        }
        fn with_hiccup(mut self) -> Self {
            self.hiccup = true;
            self
        }
    }

    impl IdentityService for StubId {
        fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn check(
            &self,
            _s: &Principal,
            permission: &Permission,
            object: &ArtifactRef,
            _at: &Consistency,
            _caveat: Option<&CaveatContext>,
        ) -> IdResult<Decision> {
            if self.hiccup {
                return Err(AuthzError::Unavailable("forced Id break".into()));
            }
            let key = format!("{}@{}", permission.0, object.0);
            Ok(if self.allow.contains(&key) {
                Decision::Allow
            } else {
                Decision::Deny
            })
        }
        fn list_objects(
            &self,
            _s: &Principal,
            _p: &Permission,
            _t: &ObjectType,
            _at: &Consistency,
        ) -> IdResult<ListObjectsResult> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn list_subjects(
            &self,
            _o: &ObjectId,
            _p: &Permission,
            _at: &Consistency,
        ) -> IdResult<SubjectTree> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn explain(
            &self,
            _s: &Principal,
            _p: &Permission,
            _o: &ObjectId,
            _at: &Consistency,
        ) -> IdResult<RewriteTrace> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn delegation(
            &self,
            _a: &Principal,
            _t: &Principal,
        ) -> IdResult<myelin_identity::EffectivePolicy> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn write_tuples(
            &self,
            _d: &[TupleDelta],
            _p: Option<&myelin_identity::Precondition>,
        ) -> IdResult<Zookie> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn mint_run_token(
            &self,
            _a: &PrincipalId,
            _r: &myelin_identity::RunId,
            _d: &myelin_identity::DelegationCaveats,
            _t: &myelin_identity::FailStaticBound,
        ) -> IdResult<myelin_identity::RunToken> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn revoke(&self, _t: &myelin_identity::RevokeTarget) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn admit_fragment(
            &self,
            _f: &myelin_identity::NamespaceFragment,
        ) -> IdResult<myelin_identity::FragmentAdmit> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
    }

    fn viewer(id: &str) -> Principal {
        Principal::new(
            TenantId("acme".into()),
            Region("fr-par".into()),
            PrincipalId(id.into()),
            PrincipalKind::Human,
            myelin_identity::DataRole::Controller,
            myelin_identity::PrincipalStatus::Active,
        )
    }

    fn a_pr() -> PullRequest {
        let mut pr = PullRequest::open(
            42,
            "refs/heads/main",
            "refs/heads/feature",
            "psn:alice",
            false,
        );
        pr.body = Body::new("Fix the charge race condition\n\nmore detail", vec![]);
        pr
    }

    fn z() -> Zookie {
        Zookie("z0".into())
    }

    #[test]
    fn git_pr_and_commit_refs_round_trip_canonical_keys() {
        let pr = git_pr_ref("acme", "repo7", 4291).unwrap();
        assert_eq!(myelin_refs::format(&pr), "myelin://acme/git/pr/repo7:4291");
        assert_eq!(myelin_refs::parse(&myelin_refs::format(&pr)).unwrap(), pr);

        let c = git_commit_ref("acme", "repo7", "blake3:deadbeefcafe0000").unwrap();
        assert_eq!(
            myelin_refs::format(&c),
            "myelin://acme/git/commit/repo7:blake3:deadbeefcafe0000"
        );

        let repo = git_repo_ref("acme", "repo7").unwrap();
        assert_eq!(myelin_refs::format(&repo), "myelin://acme/git/repo/repo7");

        let rv = git_review_ref("acme", "repo7", 4291, "psn:bob").unwrap();
        assert_eq!(
            myelin_refs::format(&rv),
            "myelin://acme/git/review/repo7:4291:psn:bob"
        );
    }

    #[test]
    fn git_ref_builders_reject_empty_and_scope_escaping_components() {
        assert!(matches!(
            git_pr_ref("acme", "", 42),
            Err(GitRefError::InvalidComponent { component: "repo" })
        ));
        assert!(matches!(
            git_repo_ref("acme/another-tenant", "repo7"),
            Err(GitRefError::InvalidComponent {
                component: "tenant"
            })
        ));
        assert!(matches!(
            git_review_ref("acme", "repo7", 42, "psn:bob#comment-injected"),
            Err(GitRefError::InvalidComponent {
                component: "reviewer_pseudonym"
            })
        ));
    }

    #[test]
    fn git_ref_builders_preserve_hierarchical_repository_names() {
        assert_eq!(
            git_repo_ref("acme", "platform/api").unwrap().0,
            "myelin://acme/git/repo/platform/api"
        );
        assert_eq!(
            git_pr_ref("acme", "platform/api", 42).unwrap().0,
            "myelin://acme/git/pr/platform/api:42"
        );
        assert!(matches!(
            git_repo_ref("acme", "platform//api"),
            Err(GitRefError::InvalidComponent {
                component: "repo_id"
            })
        ));
        assert!(git_repo_ref("acme", "platform:api").is_err());
    }

    #[test]
    fn display_key_is_render_time_only_never_a_stored_scope() {
        let pr = git_pr_ref("acme", "repo7", 1421).unwrap();
        assert_eq!(display_key(&pr).as_deref(), Some("#1421"));
        assert!(myelin_refs::parse("#1421").is_err());

        let c = git_commit_ref("acme", "repo7", "blake3:deadbeefcafef00d").unwrap();
        assert_eq!(display_key(&c).as_deref(), Some("deadbee"));
        assert!(myelin_refs::parse("deadbee").is_err());

        assert_eq!(display_key(&git_repo_ref("acme", "r").unwrap()), None);
        assert_eq!(
            display_key(&git_review_ref("acme", "r", 1, "psn:x").unwrap()),
            None
        );
    }

    #[test]
    fn classify_rejects_a_non_git_ref() {
        let issue = myelin_refs::parse("myelin://acme/issue/issue/ENG-1").unwrap();
        assert!(matches!(
            classify(&issue),
            Err(ProjectError::NotAGitArtifact { .. })
        ));
    }

    #[test]
    fn authorized_viewer_gets_the_pr_projection() {
        let pr_ref = git_pr_ref("acme", "repo7", 42).unwrap();
        let mut store = ArtifactStore::new();
        store.put_pr(
            &pr_ref,
            a_pr(),
            GateOutcome::Blocked {
                unmet: vec![CheckContext::ci("ci/build")],
            },
            1,
            2,
        );
        let id = StubId::new().allow_view(&pr_ref);
        let p = Projector::new(id, store);

        let got = p.project(&pr_ref, &viewer("alice"), z()).unwrap();
        assert!(got.is_visible());
        assert_eq!(got.title(), Some("Fix the charge race condition"));
        if let Projected::Visible(proj) = got {
            assert_eq!(proj.state, "open");
            assert_eq!(proj.icon, "pr");
            let hint = proj.render_hint.expect("a PR carries a render hint");
            assert_eq!(hint.checks, ChecksSummary::Red);
            assert_eq!(hint.approvals, (1, 2));
            assert!(!hint.is_draft);
        }
    }

    #[test]
    fn unauthorized_viewer_gets_a_tombstone_never_the_title() {
        let pr_ref = git_pr_ref("acme", "repo7", 42).unwrap();
        let mut store = ArtifactStore::new();
        store.put_pr(&pr_ref, a_pr(), GateOutcome::AllRequiredGreen, 2, 0);
        let p = Projector::new(StubId::new(), store);

        let got = p.project(&pr_ref, &viewer("mallory"), z()).unwrap();
        assert!(
            got.is_tombstone(),
            "an unauthorized viewer must get a tombstone"
        );
        assert_eq!(
            got.title(),
            None,
            "0 title leak - the denied viewer never gets the title"
        );
        if let Projected::Tombstoned(t) = got {
            assert_eq!(t.reason, TombstoneReason::Unauthorized);
            assert_eq!(t.display_text(), "(not available)");
        }
    }

    #[test]
    fn an_id_hiccup_fails_closed_to_a_tombstone() {
        let pr_ref = git_pr_ref("acme", "repo7", 42).unwrap();
        let mut store = ArtifactStore::new();
        store.put_pr(&pr_ref, a_pr(), GateOutcome::AllRequiredGreen, 0, 0);
        let id = StubId::new().allow_view(&pr_ref).with_hiccup();
        let p = Projector::new(id, store);

        let got = p.project(&pr_ref, &viewer("alice"), z()).unwrap();
        assert!(
            got.is_tombstone(),
            "an Id hiccup fails closed to a tombstone (never a leak)"
        );
        assert_eq!(got.title(), None);
    }

    #[test]
    fn an_erased_artifact_projects_to_a_tombstone() {
        let pr_ref = git_pr_ref("acme", "repo7", 42).unwrap();
        let mut store = ArtifactStore::new();
        store.put_pr(&pr_ref, a_pr(), GateOutcome::AllRequiredGreen, 0, 0);
        store.mark_erased(&pr_ref);
        let id = StubId::new().allow_view(&pr_ref);
        let p = Projector::new(id, store);

        let got = p.project(&pr_ref, &viewer("alice"), z()).unwrap();
        assert!(got.is_tombstone());
        assert_eq!(
            got.title(),
            None,
            "an erased artifact never leaks its (gone) title"
        );
        if let Projected::Tombstoned(t) = got {
            assert_eq!(t.reason, TombstoneReason::Erased);
        }
    }

    #[test]
    fn a_restricted_subject_projects_to_a_tombstone() {
        let pr_ref = git_pr_ref("acme", "repo7", 42).unwrap();
        let mut store = ArtifactStore::new();
        store.put_pr(&pr_ref, a_pr(), GateOutcome::AllRequiredGreen, 0, 0);
        store.mark_restricted(&pr_ref);
        let p = Projector::new(StubId::new().allow_view(&pr_ref), store);
        let got = p.project(&pr_ref, &viewer("alice"), z()).unwrap();
        assert!(got.is_tombstone());
        if let Projected::Tombstoned(t) = got {
            assert_eq!(t.reason, TombstoneReason::Restricted);
        }
    }

    #[test]
    fn project_commit_review_and_repo_for_authorized_viewer() {
        let commit_ref = git_commit_ref("acme", "repo7", "blake3:deadbeefcafe").unwrap();
        let review_ref = git_review_ref("acme", "repo7", 42, "psn:bob").unwrap();
        let repo_ref = git_repo_ref("acme", "repo7").unwrap();
        let mut store = ArtifactStore::new();
        store.put_commit(
            &commit_ref,
            CommitMeta {
                subject: "Fix the leak".into(),
                verified: true,
            },
        );
        let mut review = Review::request("psn:bob", false);
        review.submit(ReviewVerdict::Approve).unwrap();
        store.put_review(&review_ref, review);
        store.put_repo(
            &repo_ref,
            RepoMeta {
                slug: "acme/payments".into(),
                visibility: "private".into(),
            },
        );
        let id = StubId::new()
            .allow_view(&commit_ref)
            .allow_view(&review_ref)
            .allow_view(&repo_ref);
        let p = Projector::new(id, store);

        let c = p.project(&commit_ref, &viewer("alice"), z()).unwrap();
        assert_eq!(c.title(), Some("deadbee Fix the leak"));
        if let Projected::Visible(proj) = &c {
            assert_eq!(proj.state, "verified");
            assert_eq!(proj.icon, "commit");
        }

        let r = p.project(&review_ref, &viewer("alice"), z()).unwrap();
        assert_eq!(r.title(), Some("approved"));
        if let Projected::Visible(proj) = &r {
            assert_eq!(proj.state, "submitted");
            assert_eq!(proj.icon, "review");
        }

        let repo = p.project(&repo_ref, &viewer("alice"), z()).unwrap();
        assert_eq!(repo.title(), Some("acme/payments"));
        if let Projected::Visible(proj) = &repo {
            assert_eq!(proj.state, "private");
            assert_eq!(proj.icon, "repo");
        }
    }

    #[test]
    fn a_pr_comment_sub_anchor_projects_an_excerpt_and_inherits_the_parent_permission() {
        let pr_ref = git_pr_ref("acme", "repo7", 42).unwrap();
        let comment_ref =
            myelin_refs::mint(&pr_ref, myelin_refs::Sub::Comment("c9".into())).unwrap();
        let mut store = ArtifactStore::new();
        store.put_pr(&pr_ref, a_pr(), GateOutcome::AllRequiredGreen, 0, 0);
        store.put_comment_excerpt(&comment_ref, "this looks risky");
        let p = Projector::new(StubId::new().allow_view(&pr_ref), store);

        let got = p.project(&comment_ref, &viewer("alice"), z()).unwrap();
        assert!(got.is_visible());
        if let Projected::Visible(proj) = got {
            let anchor = proj.sub_anchor.expect("a comment sub carries a sub_anchor");
            assert_eq!(anchor.kind, "comment");
            assert_eq!(anchor.excerpt, "this looks risky");
        }
    }

    #[test]
    fn a_comment_sub_is_tombstoned_when_the_parent_pr_is_denied() {
        let pr_ref = git_pr_ref("acme", "repo7", 42).unwrap();
        let comment_ref =
            myelin_refs::mint(&pr_ref, myelin_refs::Sub::Comment("c9".into())).unwrap();
        let mut store = ArtifactStore::new();
        store.put_pr(&pr_ref, a_pr(), GateOutcome::AllRequiredGreen, 0, 0);
        store.put_comment_excerpt(&comment_ref, "secret excerpt");
        let p = Projector::new(StubId::new(), store);
        let got = p.project(&comment_ref, &viewer("mallory"), z()).unwrap();
        assert!(got.is_tombstone());
        assert_eq!(got.title(), None);
    }

    #[test]
    fn a_blob_ref_is_the_named_git_p24_floor() {
        let blob = myelin_refs::parse("myelin://acme/git/blob/repo7:main:lib.rs").unwrap();
        let p = Projector::new(StubId::new().allow_view(&blob), ArtifactStore::new());
        assert!(matches!(
            p.project(&blob, &viewer("alice"), z()),
            Err(ProjectError::BlobFloor { .. })
        ));
    }

    #[test]
    fn a_dangling_ref_is_not_found_not_a_tombstone() {
        let pr_ref = git_pr_ref("acme", "repo7", 999).unwrap();
        let p = Projector::new(StubId::new().allow_view(&pr_ref), ArtifactStore::new());
        assert!(matches!(
            p.project(&pr_ref, &viewer("alice"), z()),
            Err(ProjectError::NotFound { .. })
        ));
    }
}
