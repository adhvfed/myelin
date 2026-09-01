use std::collections::HashSet;
use std::ffi::CString;
use std::fs::{DirBuilder, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
pub const MAX_WORKSPACE_FILE_BYTES: usize = 256 * 1024;
const MAX_WORKSPACE_PATH_BYTES: usize = 1024;
const MAX_WORKSPACE_PATH_DEPTH: usize = 32;
const MAX_FED_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_REVIEW_PUBLICATION_COMMITS: usize = 10_000;
const MAX_REVIEW_PUBLICATION_DELTAS: usize = 100_000;
const MAX_REVIEW_PUBLICATION_OBJECTS: usize = 100_000;
const MAX_REVIEW_PUBLICATION_BYTES: usize = 512 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceEnvironmentManifest {
    pub provider: String,
    pub config_path: String,
    pub content_digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvisionedWorkspace {
    pub locator: String,
    pub environment: Option<WorkspaceEnvironmentManifest>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryWorkspaceSource {
    pub repository: String,
    pub repository_path: PathBuf,
    pub clone_url: String,
    pub base_ref: String,
    pub base_oid: String,
    pub branch: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryReviewPublication {
    pub head_oid: String,
    pub changed: bool,
}

#[derive(Clone, Debug)]
pub struct RepositoryReviewPublicationRequest<'a> {
    pub source: &'a RepositoryWorkspaceSource,
    pub storage_locator: &'a str,
    pub protected_paths: &'a [String],
}

#[derive(Debug)]
pub enum WorkspaceProvisionError {
    InvalidRoot(String),
    UnsafeRoot(String),
    Io(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum WorkspaceAccessError {
    InvalidPath(String),
    LocatorMismatch,
    NotFound,
    NotRegularFile,
    TooLarge,
    UnsafeStorage(String),
    Io(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum WorkspaceDeletionError {
    LocatorMismatch,
    UnsafeStorage(String),
    Io(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum WorkspacePublicationError {
    LocatorMismatch,
    WorkspaceUnavailable,
    InvalidRepository(String),
    DirtyWorkspace(String),
    ProtectedWorkspaceData(String),
    SecretDetected(String),
    ObjectTooLarge(String),
    HistoryTooLarge,
    NoChanges,
    Diverged,
    Io(String),
}

impl core::fmt::Display for WorkspacePublicationError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::LocatorMismatch => {
                formatter.write_str("workspace locator does not identify the publication target")
            }
            Self::WorkspaceUnavailable => formatter.write_str("workspace is unavailable"),
            Self::InvalidRepository(reason) => {
                write!(formatter, "workspace repository is invalid: {reason}")
            }
            Self::DirtyWorkspace(path) => {
                write!(formatter, "workspace has an uncommitted change at {path}")
            }
            Self::ProtectedWorkspaceData(path) => {
                write!(
                    formatter,
                    "workspace commit includes private data at {path}"
                )
            }
            Self::SecretDetected(path) => {
                write!(
                    formatter,
                    "workspace commit contains a possible secret at {path}"
                )
            }
            Self::ObjectTooLarge(path) => {
                write!(
                    formatter,
                    "workspace commit contains an oversized object at {path}"
                )
            }
            Self::HistoryTooLarge => {
                formatter.write_str("workspace review history exceeds safe publication limits")
            }
            Self::NoChanges => formatter.write_str("workspace branch has no committed changes"),
            Self::Diverged => {
                formatter.write_str("workspace branch no longer descends from its pinned base")
            }
            Self::Io(reason) => write!(formatter, "workspace publication failed: {reason}"),
        }
    }
}

impl std::error::Error for WorkspacePublicationError {}

impl core::fmt::Display for WorkspaceDeletionError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::LocatorMismatch => {
                formatter.write_str("workspace locator does not identify the deletion target")
            }
            Self::UnsafeStorage(reason) => write!(formatter, "unsafe workspace storage: {reason}"),
            Self::Io(reason) => write!(formatter, "workspace deletion failed: {reason}"),
        }
    }
}

impl std::error::Error for WorkspaceDeletionError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceDeletion {
    Deleted,
    AlreadyAbsent,
}

impl core::fmt::Display for WorkspaceAccessError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidPath(reason) => write!(formatter, "invalid workspace path: {reason}"),
            Self::LocatorMismatch => {
                formatter.write_str("workspace locator does not identify this workspace")
            }
            Self::NotFound => formatter.write_str("workspace file not found"),
            Self::NotRegularFile => formatter.write_str("workspace path is not a regular file"),
            Self::TooLarge => formatter.write_str("workspace file exceeds the interactive limit"),
            Self::UnsafeStorage(reason) => write!(formatter, "unsafe workspace storage: {reason}"),
            Self::Io(reason) => write!(formatter, "workspace access failed: {reason}"),
        }
    }
}

impl std::error::Error for WorkspaceAccessError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceFile {
    pub path: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct WrittenWorkspaceFile {
    pub path: String,
    pub byte_len: usize,
    pub content_digest: String,
}

pub fn validate_workspace_file_path(path: &str) -> Result<(), WorkspaceAccessError> {
    WorkspaceRelativePath::parse(path).map(|_| ())
}

pub struct VerifiedWorkspaceDirectory {
    descriptor: File,
    path: PathBuf,
    identity: DirectoryIdentity,
}

impl VerifiedWorkspaceDirectory {
    pub fn revalidated_mount_source(&self) -> Result<&Path, WorkspaceAccessError> {
        let opened = self
            .descriptor
            .metadata()
            .map_err(access_io("reinspect open workspace directory"))?;
        let current = self
            .path
            .symlink_metadata()
            .map_err(|error| match error.kind() {
                io::ErrorKind::NotFound => WorkspaceAccessError::NotFound,
                _ => access_io("reinspect workspace mount source")(error),
            })?;
        require_private_directory(&self.path, &current).map_err(map_unsafe_storage)?;
        let opened_identity = DirectoryIdentity::from_metadata(&opened);
        let current_identity = DirectoryIdentity::from_metadata(&current);
        if opened_identity != self.identity || current_identity != self.identity {
            return Err(WorkspaceAccessError::UnsafeStorage(
                "workspace directory identity changed after admission".into(),
            ));
        }
        Ok(&self.path)
    }
}

impl std::fmt::Debug for VerifiedWorkspaceDirectory {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("VerifiedWorkspaceDirectory")
            .field("device", &self.identity.device)
            .field("inode", &self.identity.inode)
            .finish_non_exhaustive()
    }
}

impl myelin_ci_sandbox::gvisor::VerifiedWorkspaceMount for VerifiedWorkspaceDirectory {
    fn revalidated_mount_source(&self) -> Result<&Path, String> {
        VerifiedWorkspaceDirectory::revalidated_mount_source(self)
            .map_err(|error| error.to_string())
    }
}

impl core::fmt::Display for WorkspaceProvisionError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidRoot(reason) => write!(formatter, "invalid workspace root: {reason}"),
            Self::UnsafeRoot(reason) => write!(formatter, "unsafe workspace root: {reason}"),
            Self::Io(reason) => write!(formatter, "workspace storage failed: {reason}"),
        }
    }
}

impl std::error::Error for WorkspaceProvisionError {}

pub trait AgentWorkspaceProvisioner: Send + Sync {
    fn provision(
        &self,
        tenant: &str,
        workspace_id: Uuid,
    ) -> Result<ProvisionedWorkspace, WorkspaceProvisionError>;

    fn provision_from_repository(
        &self,
        tenant: &str,
        workspace_id: Uuid,
        source: &RepositoryWorkspaceSource,
    ) -> Result<ProvisionedWorkspace, WorkspaceProvisionError>;
}

pub trait AgentWorkspaceStore: AgentWorkspaceProvisioner {
    fn read_file(
        &self,
        tenant: &str,
        workspace_id: Uuid,
        path: &str,
    ) -> Result<WorkspaceFile, WorkspaceAccessError>;

    fn write_file(
        &self,
        tenant: &str,
        workspace_id: Uuid,
        path: &str,
        bytes: &[u8],
    ) -> Result<WrittenWorkspaceFile, WorkspaceAccessError>;

    fn delete_workspace(
        &self,
        tenant: &str,
        workspace_id: Uuid,
        storage_locator: Option<&str>,
    ) -> Result<WorkspaceDeletion, WorkspaceDeletionError>;

    fn publish_repository_for_review(
        &self,
        tenant: &str,
        workspace_id: Uuid,
        request: RepositoryReviewPublicationRequest<'_>,
    ) -> Result<RepositoryReviewPublication, WorkspacePublicationError>;
}

#[derive(Clone)]
pub struct LocalDevelopmentWorkspaceProvisioner {
    root: Arc<PathBuf>,
    root_identity: DirectoryIdentity,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct DirectoryIdentity {
    device: u64,
    inode: u64,
}

impl DirectoryIdentity {
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
}

impl LocalDevelopmentWorkspaceProvisioner {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, WorkspaceProvisionError> {
        let root = root.as_ref();
        validate_root_spelling(root)?;
        if !root.exists() {
            DirBuilder::new()
                .recursive(true)
                .mode(PRIVATE_DIRECTORY_MODE)
                .create(root)
                .map_err(io_error("create workspace root"))?;
        }
        let canonical = root
            .canonicalize()
            .map_err(io_error("canonicalize workspace root"))?;
        validate_canonical_root(&canonical)?;
        let metadata = canonical
            .symlink_metadata()
            .map_err(io_error("inspect workspace root"))?;
        require_private_directory(&canonical, &metadata)?;
        let root_identity = DirectoryIdentity::from_metadata(&metadata);
        Ok(Self {
            root: Arc::new(canonical),
            root_identity,
        })
    }

    pub fn root(&self) -> &Path {
        self.root.as_path()
    }

    pub fn open_verified_directory(
        &self,
        tenant: &str,
        workspace_id: Uuid,
        storage_locator: &str,
    ) -> Result<VerifiedWorkspaceDirectory, WorkspaceAccessError> {
        if storage_locator != Self::locator(tenant, workspace_id) {
            return Err(WorkspaceAccessError::LocatorMismatch);
        }
        let descriptor = self.open_workspace(tenant, workspace_id)?;
        let metadata = descriptor
            .metadata()
            .map_err(access_io("inspect open workspace directory"))?;
        let identity = DirectoryIdentity::from_metadata(&metadata);
        let verified = VerifiedWorkspaceDirectory {
            descriptor,
            path: self.tenant_directory(tenant).join(workspace_id.to_string()),
            identity,
        };
        verified.revalidated_mount_source()?;
        Ok(verified)
    }

    fn verify_root(&self) -> Result<(), WorkspaceProvisionError> {
        let metadata = self
            .root
            .symlink_metadata()
            .map_err(io_error("reinspect workspace root"))?;
        require_private_directory(self.root.as_path(), &metadata)?;
        if metadata.dev() != self.root_identity.device || metadata.ino() != self.root_identity.inode
        {
            return Err(WorkspaceProvisionError::UnsafeRoot(
                "workspace root identity changed after startup".into(),
            ));
        }
        Ok(())
    }

    fn tenant_directory(&self, tenant: &str) -> PathBuf {
        let mut digest = blake3::Hasher::new();
        digest.update(b"myelin.agent-workspace.tenant.v1\0");
        digest.update(tenant.as_bytes());
        self.root.join(digest.finalize().to_hex().as_str())
    }

    fn locator(tenant: &str, workspace_id: Uuid) -> String {
        let mut digest = blake3::Hasher::new();
        digest.update(b"myelin.agent-workspace.locator.v1\0");
        digest.update(tenant.as_bytes());
        format!(
            "workspace:v1:{}:{workspace_id}",
            &digest.finalize().to_hex()[..32]
        )
    }
}

impl AgentWorkspaceProvisioner for LocalDevelopmentWorkspaceProvisioner {
    fn provision(
        &self,
        tenant: &str,
        workspace_id: Uuid,
    ) -> Result<ProvisionedWorkspace, WorkspaceProvisionError> {
        self.verify_root()?;
        let tenant_directory = self.tenant_directory(tenant);
        create_private_directory(&tenant_directory)?;
        let workspace_directory = tenant_directory.join(workspace_id.to_string());
        create_private_directory(&workspace_directory)?;
        let canonical_workspace = workspace_directory
            .canonicalize()
            .map_err(io_error("canonicalize provisioned workspace"))?;
        if canonical_workspace.parent() != Some(tenant_directory.as_path()) {
            return Err(WorkspaceProvisionError::UnsafeRoot(
                "provisioned workspace escaped its tenant directory".into(),
            ));
        }
        let metadata = canonical_workspace
            .symlink_metadata()
            .map_err(io_error("inspect provisioned workspace"))?;
        require_private_directory(&canonical_workspace, &metadata)?;
        sync_directory(&tenant_directory)?;
        sync_directory(self.root.as_path())?;
        Ok(ProvisionedWorkspace {
            locator: Self::locator(tenant, workspace_id),
            environment: None,
        })
    }

    fn provision_from_repository(
        &self,
        tenant: &str,
        workspace_id: Uuid,
        source: &RepositoryWorkspaceSource,
    ) -> Result<ProvisionedWorkspace, WorkspaceProvisionError> {
        validate_repository_source(source)?;
        self.verify_root()?;
        let tenant_directory = self.tenant_directory(tenant);
        create_private_directory(&tenant_directory)?;
        let workspace_directory = tenant_directory.join(workspace_id.to_string());
        if workspace_directory.exists() {
            verify_repository_workspace(&workspace_directory, source)?;
            return Ok(ProvisionedWorkspace {
                locator: Self::locator(tenant, workspace_id),
                environment: repository_environment_manifest(&workspace_directory, source)?,
            });
        }

        let staging_directory =
            tenant_directory.join(format!(".{}-repository-{}", workspace_id, Uuid::new_v4()));
        create_private_directory(&staging_directory)?;
        let seeded = seed_repository_workspace(&staging_directory, source);
        if let Err(error) = seeded {
            let _ = std::fs::remove_dir_all(&staging_directory);
            return Err(error);
        }
        if let Err(error) = std::fs::rename(&staging_directory, &workspace_directory) {
            let _ = std::fs::remove_dir_all(&staging_directory);
            return Err(io_error("publish repository workspace")(error));
        }
        sync_directory(&tenant_directory)?;
        sync_directory(self.root.as_path())?;
        verify_repository_workspace(&workspace_directory, source)?;
        Ok(ProvisionedWorkspace {
            locator: Self::locator(tenant, workspace_id),
            environment: repository_environment_manifest(&workspace_directory, source)?,
        })
    }
}

fn repository_environment_manifest(
    workspace: &Path,
    source: &RepositoryWorkspaceSource,
) -> Result<Option<WorkspaceEnvironmentManifest>, WorkspaceProvisionError> {
    let repository = git2::Repository::open(workspace)
        .map_err(git_error("open repository for environment discovery"))?;
    let oid = git2::Oid::from_str(&source.base_oid)
        .map_err(git_error("parse environment source commit"))?;
    let commit = repository
        .find_commit(oid)
        .map_err(git_error("find environment source commit"))?;
    let tree = commit
        .tree()
        .map_err(git_error("open environment source tree"))?;
    let Ok(entry) = tree.get_path(Path::new("fed.yaml")) else {
        return Ok(None);
    };
    if !matches!(entry.filemode(), 0o100644 | 0o100755) {
        return Ok(None);
    }
    let blob = repository
        .find_blob(entry.id())
        .map_err(git_error("read Service Federation manifest"))?;
    if blob.is_binary()
        || blob.content().is_empty()
        || blob.content().len() > MAX_FED_MANIFEST_BYTES
        || std::str::from_utf8(blob.content()).is_err()
        || !is_service_federation_manifest(blob.content())
    {
        return Ok(None);
    }
    let digest = blake3::hash(blob.content());
    Ok(Some(WorkspaceEnvironmentManifest {
        provider: "service_federation".into(),
        config_path: "fed.yaml".into(),
        content_digest: format!("blake3:{digest}"),
    }))
}

fn is_service_federation_manifest(content: &[u8]) -> bool {
    let Ok(serde_yaml_ng::Value::Mapping(root)) = serde_yaml_ng::from_slice(content) else {
        return false;
    };
    let key = |name: &str| serde_yaml_ng::Value::String(name.into());
    let mapping = |name: &str| {
        root.get(key(name))
            .is_none_or(|value| matches!(value, serde_yaml_ng::Value::Mapping(_)))
    };
    let sequence = |name: &str| {
        root.get(key(name))
            .is_none_or(|value| matches!(value, serde_yaml_ng::Value::Sequence(_)))
    };
    let string = |name: &str| {
        root.get(key(name))
            .is_none_or(|value| matches!(value, serde_yaml_ng::Value::String(_)))
    };
    let known = [
        "parameters",
        "services",
        "compose",
        "templates",
        "dependencies",
        "entrypoint",
        "entrypoints",
        "scripts",
        "packages",
        "metadata",
        "env_file",
    ];

    known.iter().any(|name| root.contains_key(key(name)))
        && [
            "parameters",
            "services",
            "templates",
            "dependencies",
            "scripts",
            "metadata",
        ]
        .iter()
        .all(|name| mapping(name))
        && ["compose", "entrypoints", "packages", "env_file"]
            .iter()
            .all(|name| sequence(name))
        && string("entrypoint")
}

fn validate_repository_source(
    source: &RepositoryWorkspaceSource,
) -> Result<(), WorkspaceProvisionError> {
    let clean = |value: &str, maximum: usize| {
        !value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
    };
    if !clean(&source.repository, 255)
        || !clean(&source.clone_url, 2048)
        || !clean(&source.base_ref, 1024)
        || !clean(&source.branch, 1024)
    {
        return Err(WorkspaceProvisionError::InvalidRoot(
            "repository workspace source contains invalid bounded text".into(),
        ));
    }
    if !source.base_ref.starts_with("refs/heads/")
        || !git2::Reference::is_valid_name(&source.base_ref)
    {
        return Err(WorkspaceProvisionError::InvalidRoot(
            "repository workspace base_ref must be a canonical branch ref".into(),
        ));
    }
    let workspace_ref = format!("refs/heads/{}", source.branch);
    if !git2::Reference::is_valid_name(&workspace_ref) {
        return Err(WorkspaceProvisionError::InvalidRoot(
            "repository workspace branch is invalid".into(),
        ));
    }
    git2::Oid::from_str(&source.base_oid).map_err(|_| {
        WorkspaceProvisionError::InvalidRoot(
            "repository workspace base_oid must be a canonical Git object id".into(),
        )
    })?;
    if source.base_oid.len() != 40
        || source
            .base_oid
            .bytes()
            .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
    {
        return Err(WorkspaceProvisionError::InvalidRoot(
            "repository workspace base_oid must be a canonical Git object id".into(),
        ));
    }
    let metadata = source
        .repository_path
        .symlink_metadata()
        .map_err(io_error("inspect repository workspace source"))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(WorkspaceProvisionError::UnsafeRoot(
            "repository workspace source is not a real directory".into(),
        ));
    }
    Ok(())
}

fn seed_repository_workspace(
    destination: &Path,
    source: &RepositoryWorkspaceSource,
) -> Result<(), WorkspaceProvisionError> {
    let repository_path = source.repository_path.to_str().ok_or_else(|| {
        WorkspaceProvisionError::InvalidRoot(
            "repository workspace source path is not clean UTF-8".into(),
        )
    })?;
    let base_branch = source
        .base_ref
        .strip_prefix("refs/heads/")
        .expect("validated repository workspace branch ref");
    let mut builder = git2::build::RepoBuilder::new();
    builder.branch(base_branch);
    let repository = builder
        .clone(repository_path, destination)
        .map_err(git_error("clone repository workspace source"))?;
    std::fs::set_permissions(
        destination,
        std::fs::Permissions::from_mode(PRIVATE_DIRECTORY_MODE),
    )
    .map_err(io_error("make repository workspace private"))?;

    let oid = git2::Oid::from_str(&source.base_oid)
        .map_err(git_error("parse repository workspace base commit"))?;
    let commit = repository
        .find_commit(oid)
        .map_err(git_error("find repository workspace base commit"))?;
    let workspace_ref = format!("refs/heads/{}", source.branch);
    match repository.find_reference(&workspace_ref) {
        Ok(mut reference) => {
            reference
                .set_target(oid, "pin Myelin workspace base")
                .map_err(git_error("pin repository workspace branch"))?;
        }
        Err(error) if error.code() == git2::ErrorCode::NotFound => {
            repository
                .branch(&source.branch, &commit, false)
                .map_err(git_error("create repository workspace branch"))?;
        }
        Err(error) => return Err(git_error("open repository workspace branch")(error)),
    }
    repository
        .set_head(&workspace_ref)
        .map_err(git_error("select repository workspace branch"))?;
    repository
        .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .map_err(git_error("checkout repository workspace branch"))?;
    repository
        .remote_set_url("origin", &source.clone_url)
        .map_err(git_error("publish repository workspace origin"))?;
    let mut config = repository
        .config()
        .map_err(git_error("open repository workspace configuration"))?;
    config
        .set_str("myelin.workspace.repository", &source.repository)
        .and_then(|()| config.set_str("myelin.workspace.base-ref", &source.base_ref))
        .and_then(|()| config.set_str("myelin.workspace.base-oid", &source.base_oid))
        .map_err(git_error("record repository workspace receipt"))?;
    Ok(())
}

fn verify_repository_workspace(
    path: &Path,
    source: &RepositoryWorkspaceSource,
) -> Result<(), WorkspaceProvisionError> {
    let metadata = path
        .symlink_metadata()
        .map_err(io_error("inspect repository workspace"))?;
    require_private_directory(path, &metadata)?;
    let repository =
        git2::Repository::open(path).map_err(git_error("open existing repository workspace"))?;
    let head = repository
        .head()
        .map_err(git_error("read existing repository workspace HEAD"))?;
    let expected_head = format!("refs/heads/{}", source.branch);
    if head.name().ok() != Some(expected_head.as_str())
        || head.target().map(|oid| oid.to_string()).as_deref() != Some(source.base_oid.as_str())
    {
        return Err(WorkspaceProvisionError::UnsafeRoot(
            "existing repository workspace does not match its durable branch receipt".into(),
        ));
    }
    let config = repository.config().map_err(git_error(
        "open existing repository workspace configuration",
    ))?;
    for (key, expected) in [
        ("myelin.workspace.repository", source.repository.as_str()),
        ("myelin.workspace.base-ref", source.base_ref.as_str()),
        ("myelin.workspace.base-oid", source.base_oid.as_str()),
        ("remote.origin.url", source.clone_url.as_str()),
    ] {
        if config.get_string(key).ok().as_deref() != Some(expected) {
            return Err(WorkspaceProvisionError::UnsafeRoot(
                "existing repository workspace does not match its durable source receipt".into(),
            ));
        }
    }
    Ok(())
}

fn git_error(context: &'static str) -> impl FnOnce(git2::Error) -> WorkspaceProvisionError {
    move |error| WorkspaceProvisionError::Io(format!("{context}: {error}"))
}

impl AgentWorkspaceStore for LocalDevelopmentWorkspaceProvisioner {
    fn read_file(
        &self,
        tenant: &str,
        workspace_id: Uuid,
        path: &str,
    ) -> Result<WorkspaceFile, WorkspaceAccessError> {
        let relative = WorkspaceRelativePath::parse(path)?;
        let workspace = self.open_workspace(tenant, workspace_id)?;
        let (parent, filename) = open_parent(workspace, &relative, false)?;
        let mut file = open_regular_at(&parent, filename)?;
        let metadata = file
            .metadata()
            .map_err(access_io("inspect workspace file"))?;
        if !metadata.file_type().is_file() {
            return Err(WorkspaceAccessError::NotRegularFile);
        }
        if metadata.len() > MAX_WORKSPACE_FILE_BYTES as u64 {
            return Err(WorkspaceAccessError::TooLarge);
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        Read::by_ref(&mut file)
            .take(MAX_WORKSPACE_FILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(access_io("read workspace file"))?;
        if bytes.len() > MAX_WORKSPACE_FILE_BYTES {
            return Err(WorkspaceAccessError::TooLarge);
        }
        Ok(WorkspaceFile {
            path: relative.rendered,
            bytes,
        })
    }

    fn write_file(
        &self,
        tenant: &str,
        workspace_id: Uuid,
        path: &str,
        bytes: &[u8],
    ) -> Result<WrittenWorkspaceFile, WorkspaceAccessError> {
        if bytes.len() > MAX_WORKSPACE_FILE_BYTES {
            return Err(WorkspaceAccessError::TooLarge);
        }
        let relative = WorkspaceRelativePath::parse(path)?;
        let workspace = self.open_workspace(tenant, workspace_id)?;
        let (parent, filename) = open_parent(workspace, &relative, true)?;
        let temporary_name = format!(".myelin-write-{}", Uuid::new_v4());
        let temporary = cstring(&temporary_name)?;
        let destination = cstring(filename)?;
        let descriptor = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                temporary.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if descriptor < 0 {
            return Err(access_io("create atomic workspace file")(
                io::Error::last_os_error(),
            ));
        }
        let mut file = unsafe { File::from_raw_fd(descriptor) };
        let write_result = file
            .write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(access_io("persist workspace file"));
        drop(file);
        if let Err(error) = write_result {
            unlink_at(&parent, &temporary);
            return Err(error);
        }
        if unsafe {
            libc::renameat(
                parent.as_raw_fd(),
                temporary.as_ptr(),
                parent.as_raw_fd(),
                destination.as_ptr(),
            )
        } < 0
        {
            let error = access_io("publish workspace file")(io::Error::last_os_error());
            unlink_at(&parent, &temporary);
            return Err(error);
        }
        parent
            .sync_all()
            .map_err(access_io("sync workspace directory"))?;
        Ok(WrittenWorkspaceFile {
            path: relative.rendered,
            byte_len: bytes.len(),
            content_digest: blake3::hash(bytes).to_hex().to_string(),
        })
    }

    fn delete_workspace(
        &self,
        tenant: &str,
        workspace_id: Uuid,
        storage_locator: Option<&str>,
    ) -> Result<WorkspaceDeletion, WorkspaceDeletionError> {
        if storage_locator.is_some_and(|locator| locator != Self::locator(tenant, workspace_id)) {
            return Err(WorkspaceDeletionError::LocatorMismatch);
        }
        self.verify_root().map_err(deletion_storage_error)?;
        let workspace = match self.open_workspace(tenant, workspace_id) {
            Ok(workspace) => workspace,
            Err(WorkspaceAccessError::NotFound) => return Ok(WorkspaceDeletion::AlreadyAbsent),
            Err(error) => return Err(deletion_access_error(error)),
        };
        let workspace_path = self.tenant_directory(tenant).join(workspace_id.to_string());
        let opened = workspace
            .metadata()
            .map_err(|error| deletion_io("inspect open workspace deletion target", error))?;
        let current = workspace_path
            .symlink_metadata()
            .map_err(|error| match error.kind() {
                io::ErrorKind::NotFound => WorkspaceDeletionError::UnsafeStorage(
                    "workspace disappeared after its deletion target was opened".into(),
                ),
                _ => deletion_io("inspect workspace deletion target", error),
            })?;
        require_private_directory(&workspace_path, &current).map_err(deletion_storage_error)?;
        if DirectoryIdentity::from_metadata(&opened) != DirectoryIdentity::from_metadata(&current) {
            return Err(WorkspaceDeletionError::UnsafeStorage(
                "workspace identity changed before deletion".into(),
            ));
        }

        match std::fs::remove_dir_all(&workspace_path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(WorkspaceDeletion::AlreadyAbsent)
            }
            Err(error) => return Err(deletion_io("remove workspace tree", error)),
        }
        sync_directory(&self.tenant_directory(tenant)).map_err(deletion_storage_error)?;
        match workspace_path.symlink_metadata() {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(WorkspaceDeletion::Deleted),
            Err(error) => Err(deletion_io("verify workspace deletion", error)),
            Ok(_) => Err(WorkspaceDeletionError::UnsafeStorage(
                "workspace remained present after deletion".into(),
            )),
        }
    }

    fn publish_repository_for_review(
        &self,
        tenant: &str,
        workspace_id: Uuid,
        request: RepositoryReviewPublicationRequest<'_>,
    ) -> Result<RepositoryReviewPublication, WorkspacePublicationError> {
        validate_repository_source(request.source)
            .map_err(|error| WorkspacePublicationError::InvalidRepository(error.to_string()))?;
        let workspace = self
            .open_verified_directory(tenant, workspace_id, request.storage_locator)
            .map_err(|error| match error {
                WorkspaceAccessError::LocatorMismatch => WorkspacePublicationError::LocatorMismatch,
                WorkspaceAccessError::NotFound => WorkspacePublicationError::WorkspaceUnavailable,
                other => WorkspacePublicationError::Io(other.to_string()),
            })?;
        publish_repository_branch(workspace.path.as_path(), request)
    }
}

fn publish_repository_branch(
    workspace_path: &Path,
    request: RepositoryReviewPublicationRequest<'_>,
) -> Result<RepositoryReviewPublication, WorkspacePublicationError> {
    let source = request.source;
    let repository = git2::Repository::open(workspace_path)
        .map_err(publication_git_error("open workspace repository"))?;
    let expected_head = format!("refs/heads/{}", source.branch);
    let head = repository
        .head()
        .map_err(publication_git_error("read workspace HEAD"))?;
    if head.name().ok() != Some(expected_head.as_str()) {
        return Err(WorkspacePublicationError::InvalidRepository(
            "HEAD does not identify the durable workspace branch".into(),
        ));
    }
    let head_oid = head.target().ok_or_else(|| {
        WorkspacePublicationError::InvalidRepository("workspace HEAD is symbolic or unborn".into())
    })?;
    let base_oid = git2::Oid::from_str(&source.base_oid)
        .map_err(publication_git_error("parse pinned workspace base"))?;
    let config = repository.config().map_err(publication_git_error(
        "open workspace repository configuration",
    ))?;
    for (key, expected) in [
        ("myelin.workspace.repository", source.repository.as_str()),
        ("myelin.workspace.base-ref", source.base_ref.as_str()),
        ("myelin.workspace.base-oid", source.base_oid.as_str()),
        ("remote.origin.url", source.clone_url.as_str()),
    ] {
        if config.get_string(key).ok().as_deref() != Some(expected) {
            return Err(WorkspacePublicationError::InvalidRepository(format!(
                "stored {key} receipt changed"
            )));
        }
    }
    if head_oid == base_oid {
        return Err(WorkspacePublicationError::NoChanges);
    }
    if !repository
        .graph_descendant_of(head_oid, base_oid)
        .map_err(publication_git_error("verify workspace ancestry"))?
    {
        return Err(WorkspacePublicationError::Diverged);
    }

    let protected = request
        .protected_paths
        .iter()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let mut status_options = git2::StatusOptions::new();
    status_options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false);
    for entry in repository
        .statuses(Some(&mut status_options))
        .map_err(publication_git_error("inspect workspace status"))?
        .iter()
    {
        let path = entry.path().map_err(|_| {
            WorkspacePublicationError::InvalidRepository(
                "workspace status contains a non-UTF-8 path".into(),
            )
        })?;
        if !protected.contains(path) {
            return Err(WorkspacePublicationError::DirtyWorkspace(path.into()));
        }
    }

    validate_review_history_content(&repository, base_oid, head_oid, &protected)?;

    let destination = git2::Repository::open_bare(&source.repository_path)
        .map_err(publication_git_error("open durable review repository"))?;
    if destination.find_commit(base_oid).is_err() {
        return Err(WorkspacePublicationError::InvalidRepository(
            "durable repository no longer contains the pinned base".into(),
        ));
    }
    let destination_tip = destination
        .find_reference(&expected_head)
        .ok()
        .and_then(|reference| reference.target());
    if destination_tip == Some(head_oid) {
        return Ok(RepositoryReviewPublication {
            head_oid: head_oid.to_string(),
            changed: false,
        });
    }
    if let Some(previous) = destination_tip {
        if !repository
            .graph_descendant_of(head_oid, previous)
            .map_err(publication_git_error("verify published branch ancestry"))?
        {
            return Err(WorkspacePublicationError::Diverged);
        }
    }
    let destination_path = source.repository_path.to_str().ok_or_else(|| {
        WorkspacePublicationError::InvalidRepository(
            "durable repository path is not clean UTF-8".into(),
        )
    })?;
    let refspec = format!("{expected_head}:{expected_head}");
    repository
        .remote_anonymous(destination_path)
        .and_then(|mut remote| remote.push(&[&refspec], None))
        .map_err(publication_git_error("publish workspace review branch"))?;
    let published_tip = destination
        .find_reference(&expected_head)
        .ok()
        .and_then(|reference| reference.target());
    if published_tip != Some(head_oid) {
        return Err(WorkspacePublicationError::Io(
            "durable review branch did not reach the committed workspace head".into(),
        ));
    }
    Ok(RepositoryReviewPublication {
        head_oid: head_oid.to_string(),
        changed: true,
    })
}

fn validate_review_history_content(
    repository: &git2::Repository,
    base_oid: git2::Oid,
    head_oid: git2::Oid,
    protected_paths: &std::collections::BTreeSet<&str>,
) -> Result<(), WorkspacePublicationError> {
    let policy = myelin_git::receive_pack::PushPolicy::default();
    let odb = repository
        .odb()
        .map_err(publication_git_error("open workspace object database"))?;
    let mut revwalk = repository
        .revwalk()
        .map_err(publication_git_error("open workspace review history"))?;
    revwalk
        .push(head_oid)
        .and_then(|()| revwalk.hide(base_oid))
        .map_err(publication_git_error("bound workspace review history"))?;
    let mut commits = 0_usize;
    let mut deltas = 0_usize;
    let mut objects = HashSet::new();
    let mut bytes = 0_usize;

    for oid in revwalk {
        let oid = oid.map_err(publication_git_error("walk workspace review history"))?;
        commits = commits
            .checked_add(1)
            .filter(|count| *count <= MAX_REVIEW_PUBLICATION_COMMITS)
            .ok_or(WorkspacePublicationError::HistoryTooLarge)?;
        scan_review_object(
            &policy,
            &odb,
            oid,
            &format!("commit {oid}"),
            &mut objects,
            &mut bytes,
        )?;

        let commit = repository
            .find_commit(oid)
            .map_err(publication_git_error("read workspace review commit"))?;
        let tree = commit
            .tree()
            .map_err(publication_git_error("read workspace review commit tree"))?;
        scan_review_object(
            &policy,
            &odb,
            tree.id(),
            &format!("tree for commit {oid}"),
            &mut objects,
            &mut bytes,
        )?;
        let parent_tree = if commit.parent_count() == 0 {
            None
        } else {
            Some(
                commit
                    .parent(0)
                    .and_then(|parent| parent.tree())
                    .map_err(publication_git_error("read workspace review parent tree"))?,
            )
        };
        let diff = repository
            .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)
            .map_err(publication_git_error(
                "inspect workspace review commit content",
            ))?;
        for delta in diff.deltas() {
            deltas = deltas
                .checked_add(1)
                .filter(|count| *count <= MAX_REVIEW_PUBLICATION_DELTAS)
                .ok_or(WorkspacePublicationError::HistoryTooLarge)?;
            for path in [delta.old_file().path(), delta.new_file().path()]
                .into_iter()
                .flatten()
            {
                let path = path.to_str().ok_or_else(|| {
                    WorkspacePublicationError::InvalidRepository(
                        "workspace review history contains a non-UTF-8 path".into(),
                    )
                })?;
                if private_review_path(path, protected_paths) {
                    return Err(WorkspacePublicationError::ProtectedWorkspaceData(
                        path.into(),
                    ));
                }
            }
            let oid = delta.new_file().id();
            if !oid.is_zero() {
                if let Some(path) = delta.new_file().path() {
                    scan_review_tree_ancestors(
                        &tree,
                        path,
                        &policy,
                        &odb,
                        &mut objects,
                        &mut bytes,
                    )?;
                }
            }
            if oid.is_zero() || objects.contains(&oid) {
                continue;
            }
            let object = odb
                .read(oid)
                .map_err(publication_git_error("read workspace review object"))?;
            if object.kind() != git2::ObjectType::Blob {
                continue;
            }
            let path = delta
                .new_file()
                .path()
                .and_then(Path::to_str)
                .ok_or_else(|| {
                    WorkspacePublicationError::InvalidRepository(
                        "workspace review history contains a non-UTF-8 path".into(),
                    )
                })?;
            scan_review_object(&policy, &odb, oid, path, &mut objects, &mut bytes)?;
        }
    }
    Ok(())
}

fn scan_review_tree_ancestors(
    root: &git2::Tree<'_>,
    path: &Path,
    policy: &myelin_git::receive_pack::PushPolicy,
    odb: &git2::Odb<'_>,
    objects: &mut HashSet<git2::Oid>,
    bytes: &mut usize,
) -> Result<(), WorkspacePublicationError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let mut ancestor = std::path::PathBuf::new();
    for component in parent.components() {
        let std::path::Component::Normal(component) = component else {
            return Err(WorkspacePublicationError::InvalidRepository(
                "workspace review history contains an unsafe path".into(),
            ));
        };
        ancestor.push(component);
        let entry = root
            .get_path(&ancestor)
            .map_err(publication_git_error("read workspace review subtree"))?;
        if entry.kind() != Some(git2::ObjectType::Tree) {
            return Err(WorkspacePublicationError::InvalidRepository(
                "workspace review history path parent is not a tree".into(),
            ));
        }
        let oid = entry.id();
        scan_review_object(policy, odb, oid, &format!("tree {oid}"), objects, bytes)?;
    }
    Ok(())
}

fn scan_review_object(
    policy: &myelin_git::receive_pack::PushPolicy,
    odb: &git2::Odb<'_>,
    oid: git2::Oid,
    path: &str,
    objects: &mut HashSet<git2::Oid>,
    bytes: &mut usize,
) -> Result<(), WorkspacePublicationError> {
    if !objects.insert(oid) {
        return Ok(());
    }
    if objects.len() > MAX_REVIEW_PUBLICATION_OBJECTS {
        return Err(WorkspacePublicationError::HistoryTooLarge);
    }
    let object = odb.read(oid).map_err(publication_git_error(
        "read workspace review object content",
    ))?;
    *bytes = bytes
        .checked_add(object.data().len())
        .filter(|total| *total <= MAX_REVIEW_PUBLICATION_BYTES)
        .ok_or(WorkspacePublicationError::HistoryTooLarge)?;
    let policy_oid = myelin_git::receive_pack::Oid::new(oid.to_string());
    match policy.evaluate_object_content(&policy_oid, object.data()) {
        Ok(()) => Ok(()),
        Err(myelin_git::receive_pack::RejectReason::SecretDetected { .. }) => {
            Err(WorkspacePublicationError::SecretDetected(path.into()))
        }
        Err(myelin_git::receive_pack::RejectReason::ObjectTooLarge { .. }) => {
            Err(WorkspacePublicationError::ObjectTooLarge(path.into()))
        }
        Err(_) => Err(WorkspacePublicationError::InvalidRepository(
            "workspace review object failed content policy".into(),
        )),
    }
}

fn private_review_path(path: &str, personalized: &std::collections::BTreeSet<&str>) -> bool {
    personalized.contains(path)
        || path == ".fed"
        || (path.starts_with(".fed/") && !matches!(path, ".fed/cloud.yaml" | ".fed/.gitignore"))
}

fn publication_git_error(
    context: &'static str,
) -> impl FnOnce(git2::Error) -> WorkspacePublicationError {
    move |error| WorkspacePublicationError::Io(format!("{context}: {error}"))
}

impl LocalDevelopmentWorkspaceProvisioner {
    fn open_workspace(
        &self,
        tenant: &str,
        workspace_id: Uuid,
    ) -> Result<File, WorkspaceAccessError> {
        self.verify_root().map_err(map_unsafe_storage)?;
        let root = open_directory_path(self.root.as_path())?;
        let metadata = root
            .metadata()
            .map_err(access_io("inspect open workspace root"))?;
        if metadata.dev() != self.root_identity.device || metadata.ino() != self.root_identity.inode
        {
            return Err(WorkspaceAccessError::UnsafeStorage(
                "workspace root identity changed while opening it".into(),
            ));
        }
        let tenant_name = self
            .tenant_directory(tenant)
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| WorkspaceAccessError::UnsafeStorage("tenant key is invalid".into()))?
            .to_string();
        let tenant_directory = open_directory_at(&root, &tenant_name)?;
        open_directory_at(&tenant_directory, &workspace_id.to_string())
    }
}

struct WorkspaceRelativePath {
    components: Vec<String>,
    rendered: String,
}

impl WorkspaceRelativePath {
    fn parse(path: &str) -> Result<Self, WorkspaceAccessError> {
        if path.is_empty() || path.len() > MAX_WORKSPACE_PATH_BYTES || path.contains('\0') {
            return Err(WorkspaceAccessError::InvalidPath(
                "path must contain 1..=1024 bytes".into(),
            ));
        }
        let parsed = Path::new(path);
        let components = parsed
            .components()
            .map(|component| match component {
                Component::Normal(name) => name
                    .to_str()
                    .filter(|name| !name.is_empty() && name.len() <= 255)
                    .map(str::to_string)
                    .ok_or_else(|| {
                        WorkspaceAccessError::InvalidPath(
                            "every component must be clean UTF-8 of at most 255 bytes".into(),
                        )
                    }),
                _ => Err(WorkspaceAccessError::InvalidPath(
                    "absolute, current, and parent components are forbidden".into(),
                )),
            })
            .collect::<Result<Vec<_>, _>>()?;
        if components.is_empty()
            || components.len() > MAX_WORKSPACE_PATH_DEPTH
            || components.join("/") != path
        {
            return Err(WorkspaceAccessError::InvalidPath(
                "path must be one canonical relative spelling with at most 32 components".into(),
            ));
        }
        Ok(Self {
            rendered: components.join("/"),
            components,
        })
    }
}

fn open_parent(
    workspace: File,
    path: &WorkspaceRelativePath,
    create: bool,
) -> Result<(File, &str), WorkspaceAccessError> {
    let (filename, parents) = path
        .components
        .split_last()
        .expect("validated workspace paths have at least one component");
    let mut directory = workspace;
    for component in parents {
        directory = if create {
            create_or_open_directory_at(&directory, component)?
        } else {
            open_directory_at(&directory, component)?
        };
    }
    Ok((directory, filename))
}

fn open_directory_path(path: &Path) -> Result<File, WorkspaceAccessError> {
    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        WorkspaceAccessError::UnsafeStorage("workspace root contains a NUL byte".into())
    })?;
    let descriptor = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    owned_descriptor(descriptor, "open workspace root")
}

fn open_directory_at(parent: &File, name: &str) -> Result<File, WorkspaceAccessError> {
    let name = cstring(name)?;
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    owned_descriptor(descriptor, "open workspace directory")
}

fn create_or_open_directory_at(parent: &File, name: &str) -> Result<File, WorkspaceAccessError> {
    let name_c = cstring(name)?;
    if unsafe { libc::mkdirat(parent.as_raw_fd(), name_c.as_ptr(), PRIVATE_DIRECTORY_MODE) } < 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::AlreadyExists {
            return Err(access_io("create workspace directory")(error));
        }
    }
    open_directory_at(parent, name)
}

fn open_regular_at(parent: &File, name: &str) -> Result<File, WorkspaceAccessError> {
    let name = cstring(name)?;
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        let error = io::Error::last_os_error();
        return Err(match error.kind() {
            io::ErrorKind::NotFound => WorkspaceAccessError::NotFound,
            _ => access_io("open workspace file")(error),
        });
    }
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

fn owned_descriptor(
    descriptor: i32,
    operation: &'static str,
) -> Result<File, WorkspaceAccessError> {
    if descriptor < 0 {
        let error = io::Error::last_os_error();
        return Err(match error.kind() {
            io::ErrorKind::NotFound => WorkspaceAccessError::NotFound,
            _ => access_io(operation)(error),
        });
    }
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

fn cstring(value: &str) -> Result<CString, WorkspaceAccessError> {
    CString::new(value).map_err(|_| {
        WorkspaceAccessError::InvalidPath("path components must not contain NUL bytes".into())
    })
}

fn unlink_at(parent: &File, name: &CString) {
    unsafe {
        libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0);
    }
}

fn map_unsafe_storage(error: WorkspaceProvisionError) -> WorkspaceAccessError {
    WorkspaceAccessError::UnsafeStorage(error.to_string())
}

fn deletion_storage_error(error: WorkspaceProvisionError) -> WorkspaceDeletionError {
    match error {
        WorkspaceProvisionError::InvalidRoot(reason)
        | WorkspaceProvisionError::UnsafeRoot(reason) => {
            WorkspaceDeletionError::UnsafeStorage(reason)
        }
        WorkspaceProvisionError::Io(reason) => WorkspaceDeletionError::Io(reason),
    }
}

fn deletion_access_error(error: WorkspaceAccessError) -> WorkspaceDeletionError {
    match error {
        WorkspaceAccessError::LocatorMismatch => WorkspaceDeletionError::LocatorMismatch,
        WorkspaceAccessError::Io(reason) => WorkspaceDeletionError::Io(reason),
        WorkspaceAccessError::InvalidPath(reason) | WorkspaceAccessError::UnsafeStorage(reason) => {
            WorkspaceDeletionError::UnsafeStorage(reason)
        }
        WorkspaceAccessError::NotFound => {
            WorkspaceDeletionError::UnsafeStorage("workspace disappeared during deletion".into())
        }
        WorkspaceAccessError::NotRegularFile | WorkspaceAccessError::TooLarge => {
            WorkspaceDeletionError::UnsafeStorage(
                "workspace deletion resolved through an invalid storage object".into(),
            )
        }
    }
}

fn deletion_io(context: &'static str, error: io::Error) -> WorkspaceDeletionError {
    WorkspaceDeletionError::Io(format!("{context}: {error}"))
}

fn access_io(context: &'static str) -> impl FnOnce(io::Error) -> WorkspaceAccessError {
    move |error| WorkspaceAccessError::Io(format!("{context}: {error}"))
}

fn validate_root_spelling(root: &Path) -> Result<(), WorkspaceProvisionError> {
    if !root.is_absolute() {
        return Err(WorkspaceProvisionError::InvalidRoot(
            "workspace root must be absolute".into(),
        ));
    }
    if root.parent().is_none() {
        return Err(WorkspaceProvisionError::InvalidRoot(
            "workspace root must not be the filesystem root".into(),
        ));
    }
    if root
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(WorkspaceProvisionError::InvalidRoot(
            "workspace root must not contain `.` or `..` components".into(),
        ));
    }
    Ok(())
}

fn validate_canonical_root(root: &Path) -> Result<(), WorkspaceProvisionError> {
    if root.parent().is_none() {
        return Err(WorkspaceProvisionError::InvalidRoot(
            "workspace root must not resolve to the filesystem root".into(),
        ));
    }
    let temporary = std::env::temp_dir()
        .canonicalize()
        .unwrap_or_else(|_| std::env::temp_dir());
    if root.starts_with(temporary) {
        return Err(WorkspaceProvisionError::InvalidRoot(
            "workspace root must not live under the operating-system temporary directory".into(),
        ));
    }
    Ok(())
}

fn create_private_directory(path: &Path) -> Result<(), WorkspaceProvisionError> {
    match DirBuilder::new().mode(PRIVATE_DIRECTORY_MODE).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(io_error("create private workspace directory")(error)),
    }
    let metadata = path
        .symlink_metadata()
        .map_err(io_error("inspect private workspace directory"))?;
    require_private_directory(path, &metadata)
}

fn require_private_directory(
    path: &Path,
    metadata: &std::fs::Metadata,
) -> Result<(), WorkspaceProvisionError> {
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(WorkspaceProvisionError::UnsafeRoot(format!(
            "{} is not a real directory",
            path.display()
        )));
    }
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(WorkspaceProvisionError::UnsafeRoot(format!(
            "{} must not be accessible by group or other users (mode {mode:o})",
            path.display()
        )));
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), WorkspaceProvisionError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(io_error("sync workspace directory"))
}

fn io_error(context: &'static str) -> impl FnOnce(io::Error) -> WorkspaceProvisionError {
    move |error| WorkspaceProvisionError::Io(format!("{context}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn durable_test_root() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("myelin-agent-workspace-")
            .tempdir_in("/var/tmp")
            .unwrap()
    }

    #[test]
    fn provisioning_is_private_tenant_blind_and_idempotent() {
        let temporary = durable_test_root();
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = LocalDevelopmentWorkspaceProvisioner::open(temporary.path()).unwrap();
        let workspace_id = Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap();

        let first = store.provision("acme", workspace_id).unwrap();
        let replay = store.provision("acme", workspace_id).unwrap();

        assert_eq!(first, replay);
        assert!(!first.locator.contains("acme"));
        assert!(!first.locator.contains(temporary.path().to_str().unwrap()));
        let entries = std::fs::read_dir(store.root())
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].metadata().unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn environment_receipts_require_a_structural_fed_manifest() {
        assert!(is_service_federation_manifest(
            b"services:\n  app:\n    process: ./dev\nentrypoint: app\n"
        ));
        assert!(is_service_federation_manifest(
            b"compose:\n  - ./compose.yaml\n"
        ));
        assert!(!is_service_federation_manifest(
            b"services:\n  app: [unterminated\n"
        ));
        assert!(!is_service_federation_manifest(b"services: app\n"));
        assert!(!is_service_federation_manifest(
            b"title: this is an unrelated yaml document\n"
        ));
        assert!(!is_service_federation_manifest(b"just a scalar\n"));
    }

    #[test]
    fn repository_workspace_is_an_exact_idempotent_checkout() {
        let source_root = durable_test_root();
        let source_path = source_root.path().join("source.git");
        let source_repository = git2::Repository::init_bare(&source_path).unwrap();
        let blob = source_repository.blob(b"hello from Myelin\n").unwrap();
        let fed_manifest = b"services:\n  app:\n    process: ./dev\nentrypoint: app\n";
        let fed_blob = source_repository.blob(fed_manifest).unwrap();
        let mut tree = source_repository.treebuilder(None).unwrap();
        tree.insert("README.md", blob, 0o100644).unwrap();
        tree.insert("fed.yaml", fed_blob, 0o100644).unwrap();
        let tree_oid = tree.write().unwrap();
        let tree = source_repository.find_tree(tree_oid).unwrap();
        let signature = git2::Signature::now("Myelin", "myelin@example.invalid").unwrap();
        let base_oid = source_repository
            .commit(
                Some("refs/heads/main"),
                &signature,
                &signature,
                "seed",
                &tree,
                &[],
            )
            .unwrap();
        source_repository.set_head("refs/heads/main").unwrap();

        let temporary = durable_test_root();
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = LocalDevelopmentWorkspaceProvisioner::open(temporary.path()).unwrap();
        let workspace_id = Uuid::parse_str("66666666-6666-4666-8666-666666666666").unwrap();
        let source = RepositoryWorkspaceSource {
            repository: "sweet-platform".into(),
            repository_path: source_path,
            clone_url: "https://myelin.example/acme/eu/sweet-platform.git".into(),
            base_ref: "refs/heads/main".into(),
            base_oid: base_oid.to_string(),
            branch: "myelin/thread-delight".into(),
        };

        let first = store
            .provision_from_repository("acme", workspace_id, &source)
            .unwrap();
        let replay = store
            .provision_from_repository("acme", workspace_id, &source)
            .unwrap();
        assert_eq!(first, replay);
        assert_eq!(
            first.environment,
            Some(WorkspaceEnvironmentManifest {
                provider: "service_federation".into(),
                config_path: "fed.yaml".into(),
                content_digest: format!("blake3:{}", blake3::hash(fed_manifest)),
            })
        );
        let workspace = store
            .tenant_directory("acme")
            .join(workspace_id.to_string());
        assert_eq!(
            std::fs::read_to_string(workspace.join("README.md")).unwrap(),
            "hello from Myelin\n"
        );
        let checkout = git2::Repository::open(&workspace).unwrap();
        assert_eq!(checkout.head().unwrap().target(), Some(base_oid));
        assert_eq!(
            checkout.head().unwrap().name().unwrap(),
            "refs/heads/myelin/thread-delight"
        );
        assert_eq!(
            checkout.find_remote("origin").unwrap().url().unwrap(),
            source.clone_url.as_str()
        );
    }

    #[test]
    fn review_publication_is_fast_forward_idempotent_and_excludes_private_augmentation() {
        let personalized = std::collections::BTreeSet::from(["AGENTS.md"]);
        assert!(private_review_path("AGENTS.md", &personalized));
        assert!(private_review_path(".fed", &personalized));
        assert!(private_review_path(".fed/lock.db", &personalized));
        assert!(private_review_path(".fed/logs/app.log", &personalized));
        assert!(private_review_path(
            ".fed/secrets.generated.env",
            &personalized
        ));
        assert!(!private_review_path(".fed/cloud.yaml", &personalized));
        assert!(!private_review_path(".fed/.gitignore", &personalized));

        let source_root = durable_test_root();
        let source_path = source_root.path().join("source.git");
        let source_repository = git2::Repository::init_bare(&source_path).unwrap();
        let blob = source_repository.blob(b"private cloud branch\n").unwrap();
        let mut tree = source_repository.treebuilder(None).unwrap();
        tree.insert("README.md", blob, 0o100644).unwrap();
        let tree = source_repository.find_tree(tree.write().unwrap()).unwrap();
        let signature = git2::Signature::now("Myelin", "myelin@example.invalid").unwrap();
        let base_oid = source_repository
            .commit(
                Some("refs/heads/main"),
                &signature,
                &signature,
                "seed",
                &tree,
                &[],
            )
            .unwrap();
        source_repository.set_head("refs/heads/main").unwrap();

        let temporary = durable_test_root();
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = LocalDevelopmentWorkspaceProvisioner::open(temporary.path()).unwrap();
        let workspace_id = Uuid::parse_str("76767676-7676-4676-8676-767676767676").unwrap();
        let source = RepositoryWorkspaceSource {
            repository: "sweet-platform".into(),
            repository_path: source_path,
            clone_url: "https://myelin.example/acme/eu/sweet-platform.git".into(),
            base_ref: "refs/heads/main".into(),
            base_oid: base_oid.to_string(),
            branch: "myelin/private-review".into(),
        };
        let provisioned = store
            .provision_from_repository("acme", workspace_id, &source)
            .unwrap();
        store
            .write_file(
                "acme",
                workspace_id,
                ".myelin/private-context.md",
                b"owner-only context\n",
            )
            .unwrap();
        let workspace = store
            .tenant_directory("acme")
            .join(workspace_id.to_string());
        std::fs::write(workspace.join("README.md"), "ready for a human review\n").unwrap();
        let checkout = git2::Repository::open(&workspace).unwrap();
        let mut index = checkout.index().unwrap();
        index.add_path(Path::new("README.md")).unwrap();
        index.write().unwrap();
        let tree = checkout.find_tree(index.write_tree().unwrap()).unwrap();
        let parent = checkout.find_commit(base_oid).unwrap();
        let review_oid = checkout
            .commit(
                Some("refs/heads/myelin/private-review"),
                &signature,
                &signature,
                "prepare review",
                &tree,
                &[&parent],
            )
            .unwrap();

        let protected = vec![".myelin/private-context.md".to_string()];
        let request = || RepositoryReviewPublicationRequest {
            source: &source,
            storage_locator: &provisioned.locator,
            protected_paths: &protected,
        };
        let published = store
            .publish_repository_for_review("acme", workspace_id, request())
            .unwrap();
        assert_eq!(
            published,
            RepositoryReviewPublication {
                head_oid: review_oid.to_string(),
                changed: true,
            }
        );
        assert_eq!(
            source_repository
                .find_reference("refs/heads/myelin/private-review")
                .unwrap()
                .target(),
            Some(review_oid)
        );
        assert!(source_repository
            .find_commit(review_oid)
            .unwrap()
            .tree()
            .unwrap()
            .get_path(Path::new(".myelin/private-context.md"))
            .is_err());
        assert!(
            !store
                .publish_repository_for_review("acme", workspace_id, request())
                .unwrap()
                .changed
        );

        std::fs::write(workspace.join(".fed"), "PRIVATE_RUNTIME_STATE=local-only\n").unwrap();
        let mut index = checkout.index().unwrap();
        index.add_path(Path::new(".fed")).unwrap();
        index.write().unwrap();
        let tree = checkout.find_tree(index.write_tree().unwrap()).unwrap();
        let parent = checkout.find_commit(review_oid).unwrap();
        let fed_state_oid = checkout
            .commit(
                Some("refs/heads/myelin/private-review"),
                &signature,
                &signature,
                "accidentally stage Fed runtime state",
                &tree,
                &[&parent],
            )
            .unwrap();
        std::fs::remove_file(workspace.join(".fed")).unwrap();
        let mut index = checkout.index().unwrap();
        index.remove_path(Path::new(".fed")).unwrap();
        index.write().unwrap();
        let tree = checkout.find_tree(index.write_tree().unwrap()).unwrap();
        let parent = checkout.find_commit(fed_state_oid).unwrap();
        checkout
            .commit(
                Some("refs/heads/myelin/private-review"),
                &signature,
                &signature,
                "remove Fed runtime state from the visible tree",
                &tree,
                &[&parent],
            )
            .unwrap();
        assert_eq!(
            store.publish_repository_for_review("acme", workspace_id, request()),
            Err(WorkspacePublicationError::ProtectedWorkspaceData(
                ".fed".into()
            ))
        );
        assert_eq!(
            source_repository
                .find_reference("refs/heads/myelin/private-review")
                .unwrap()
                .target(),
            Some(review_oid),
            "Fed runtime state hidden in review history never advances the durable branch"
        );
        checkout
            .reset(
                &checkout.find_commit(review_oid).unwrap().into_object(),
                git2::ResetType::Hard,
                None,
            )
            .unwrap();

        let credential = [b"CLOUD_KEY=AK".as_slice(), b"IA_TEST_ONLY\n"].concat();
        std::fs::write(workspace.join("credentials.env"), credential).unwrap();
        let mut index = checkout.index().unwrap();
        index.add_path(Path::new("credentials.env")).unwrap();
        index.write().unwrap();
        let tree = checkout.find_tree(index.write_tree().unwrap()).unwrap();
        let parent = checkout.find_commit(review_oid).unwrap();
        let secret_oid = checkout
            .commit(
                Some("refs/heads/myelin/private-review"),
                &signature,
                &signature,
                "accidentally commit a credential",
                &tree,
                &[&parent],
            )
            .unwrap();
        std::fs::remove_file(workspace.join("credentials.env")).unwrap();
        let mut index = checkout.index().unwrap();
        index.remove_path(Path::new("credentials.env")).unwrap();
        index.write().unwrap();
        let tree = checkout.find_tree(index.write_tree().unwrap()).unwrap();
        let parent = checkout.find_commit(secret_oid).unwrap();
        let cleaned_oid = checkout
            .commit(
                Some("refs/heads/myelin/private-review"),
                &signature,
                &signature,
                "remove credential from the visible tree",
                &tree,
                &[&parent],
            )
            .unwrap();
        assert!(checkout
            .find_commit(cleaned_oid)
            .unwrap()
            .tree()
            .unwrap()
            .get_path(Path::new("credentials.env"))
            .is_err());
        assert_eq!(
            store.publish_repository_for_review("acme", workspace_id, request()),
            Err(WorkspacePublicationError::SecretDetected(
                "credentials.env".into()
            )),
            "content policy must inspect deleted intermediate objects, not only the head tree"
        );
        assert_eq!(
            source_repository
                .find_reference("refs/heads/myelin/private-review")
                .unwrap()
                .target(),
            Some(review_oid),
            "a secret hidden in review history never advances the durable branch"
        );
        checkout
            .reset(
                &checkout.find_commit(review_oid).unwrap().into_object(),
                git2::ResetType::Hard,
                None,
            )
            .unwrap();

        let credential_filename = ["leaked-AK", "IAIOSFODNN7EXAMPLE"].concat();
        std::fs::write(workspace.join(&credential_filename), "harmless file body\n").unwrap();
        let mut index = checkout.index().unwrap();
        index.add_path(Path::new(&credential_filename)).unwrap();
        index.write().unwrap();
        let tree = checkout.find_tree(index.write_tree().unwrap()).unwrap();
        let parent = checkout.find_commit(review_oid).unwrap();
        checkout
            .commit(
                Some("refs/heads/myelin/private-review"),
                &signature,
                &signature,
                "accidentally expose a credential through a filename",
                &tree,
                &[&parent],
            )
            .unwrap();
        assert_eq!(
            store.publish_repository_for_review("acme", workspace_id, request()),
            Err(WorkspacePublicationError::SecretDetected(format!(
                "tree for commit {}",
                checkout.head().unwrap().target().unwrap()
            ))),
            "tree metadata crosses the same content policy as blobs and commits"
        );
        assert_eq!(
            source_repository
                .find_reference("refs/heads/myelin/private-review")
                .unwrap()
                .target(),
            Some(review_oid),
            "a secret in tree metadata never advances the durable branch"
        );
        checkout
            .reset(
                &checkout.find_commit(review_oid).unwrap().into_object(),
                git2::ResetType::Hard,
                None,
            )
            .unwrap();

        let mut index = checkout.index().unwrap();
        index
            .add_path(Path::new(".myelin/private-context.md"))
            .unwrap();
        index.write().unwrap();
        let tree = checkout.find_tree(index.write_tree().unwrap()).unwrap();
        let parent = checkout.find_commit(review_oid).unwrap();
        checkout
            .commit(
                Some("refs/heads/myelin/private-review"),
                &signature,
                &signature,
                "accidentally stage private context",
                &tree,
                &[&parent],
            )
            .unwrap();
        assert_eq!(
            store.publish_repository_for_review("acme", workspace_id, request()),
            Err(WorkspacePublicationError::ProtectedWorkspaceData(
                ".myelin/private-context.md".into()
            ))
        );
        assert_eq!(
            source_repository
                .find_reference("refs/heads/myelin/private-review")
                .unwrap()
                .target(),
            Some(review_oid),
            "a rejected private-data commit never advances the durable branch"
        );
    }

    #[test]
    fn a_durable_locator_opens_only_its_inode_pinned_workspace() {
        let temporary = durable_test_root();
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = LocalDevelopmentWorkspaceProvisioner::open(temporary.path()).unwrap();
        let workspace_id = Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap();
        let provisioned = store.provision("acme", workspace_id).unwrap();

        let verified = store
            .open_verified_directory("acme", workspace_id, &provisioned.locator)
            .expect("the durable locator should resolve to the workspace it names");
        assert!(verified.revalidated_mount_source().unwrap().is_dir());
        assert!(!format!("{verified:?}").contains(temporary.path().to_str().unwrap()));

        assert!(matches!(
            store.open_verified_directory("other", workspace_id, &provisioned.locator),
            Err(WorkspaceAccessError::LocatorMismatch)
        ));
        assert!(matches!(
            store.open_verified_directory("acme", Uuid::from_u128(23), &provisioned.locator),
            Err(WorkspaceAccessError::LocatorMismatch)
        ));
    }

    #[test]
    fn an_admitted_workspace_path_cannot_be_swapped_before_mounting() {
        let temporary = durable_test_root();
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = LocalDevelopmentWorkspaceProvisioner::open(temporary.path()).unwrap();
        let workspace_id = Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap();
        let provisioned = store.provision("acme", workspace_id).unwrap();
        let verified = store
            .open_verified_directory("acme", workspace_id, &provisioned.locator)
            .unwrap();
        let original = verified.revalidated_mount_source().unwrap().to_path_buf();
        let parked = original.with_extension("parked");
        std::fs::rename(&original, &parked).unwrap();
        std::fs::create_dir(&original).unwrap();
        std::fs::set_permissions(&original, std::fs::Permissions::from_mode(0o700)).unwrap();

        assert!(matches!(
            verified.revalidated_mount_source(),
            Err(WorkspaceAccessError::UnsafeStorage(_))
        ));
    }

    #[test]
    fn a_world_readable_or_temporary_root_is_refused() {
        let temporary = durable_test_root();
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            LocalDevelopmentWorkspaceProvisioner::open(temporary.path()),
            Err(WorkspaceProvisionError::UnsafeRoot(_))
        ));
        assert!(matches!(
            LocalDevelopmentWorkspaceProvisioner::open(std::env::temp_dir().join("myelin-work")),
            Err(WorkspaceProvisionError::InvalidRoot(_))
        ));
    }

    #[test]
    fn files_survive_calls_and_nested_paths_cannot_follow_symlinks() {
        let temporary = durable_test_root();
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = LocalDevelopmentWorkspaceProvisioner::open(temporary.path()).unwrap();
        let workspace_id = Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap();
        store.provision("acme", workspace_id).unwrap();

        let written = store
            .write_file(
                "acme",
                workspace_id,
                "notes/continuity.txt",
                b"still here\n",
            )
            .unwrap();
        assert_eq!(written.path, "notes/continuity.txt");
        assert_eq!(written.byte_len, 11);
        assert_eq!(
            store
                .read_file("acme", workspace_id, "notes/continuity.txt")
                .unwrap()
                .bytes,
            b"still here\n"
        );

        for path in ["../outside", "/etc/passwd", "notes//ambiguous", "./notes"] {
            assert!(matches!(
                store.read_file("acme", workspace_id, path),
                Err(WorkspaceAccessError::InvalidPath(_))
            ));
        }

        let workspace = store
            .tenant_directory("acme")
            .join(workspace_id.to_string());
        std::os::unix::fs::symlink("/etc", workspace.join("escape")).unwrap();
        assert!(store
            .read_file("acme", workspace_id, "escape/passwd")
            .is_err());
    }

    #[test]
    fn deletion_is_exact_idempotent_and_never_follows_workspace_symlinks() {
        let temporary = durable_test_root();
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = LocalDevelopmentWorkspaceProvisioner::open(temporary.path()).unwrap();
        let workspace_id = Uuid::parse_str("44444444-4444-4444-8444-444444444444").unwrap();
        let provisioned = store.provision("acme", workspace_id).unwrap();
        store
            .write_file("acme", workspace_id, "notes/final.txt", b"done")
            .unwrap();
        let outside = store.root().join("must-survive");
        create_private_directory(&outside).unwrap();
        std::fs::write(outside.join("marker"), b"outside").unwrap();
        std::os::unix::fs::symlink(
            &outside,
            store
                .tenant_directory("acme")
                .join(workspace_id.to_string())
                .join("outside-link"),
        )
        .unwrap();

        assert_eq!(
            store.delete_workspace("acme", workspace_id, Some(&provisioned.locator)),
            Ok(WorkspaceDeletion::Deleted)
        );
        assert_eq!(std::fs::read(outside.join("marker")).unwrap(), b"outside");
        assert_eq!(
            store.delete_workspace("acme", workspace_id, Some(&provisioned.locator)),
            Ok(WorkspaceDeletion::AlreadyAbsent)
        );

        let other = Uuid::parse_str("55555555-5555-4555-8555-555555555555").unwrap();
        store.provision("acme", other).unwrap();
        assert_eq!(
            store.delete_workspace("acme", other, Some(&provisioned.locator)),
            Err(WorkspaceDeletionError::LocatorMismatch)
        );
        assert!(store.open_workspace("acme", other).is_ok());
    }
}
