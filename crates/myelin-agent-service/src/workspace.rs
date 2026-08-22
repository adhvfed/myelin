use std::fs::{DirBuilder, File};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use uuid::Uuid;

const PRIVATE_DIRECTORY_MODE: u32 = 0o700;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvisionedWorkspace {
    pub locator: String,
}

#[derive(Debug)]
pub enum WorkspaceProvisionError {
    InvalidRoot(String),
    UnsafeRoot(String),
    Io(String),
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
}

#[derive(Clone)]
pub struct LocalDevelopmentWorkspaceProvisioner {
    root: Arc<PathBuf>,
    root_identity: DirectoryIdentity,
}

#[derive(Clone, Copy)]
struct DirectoryIdentity {
    device: u64,
    inode: u64,
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
        let root_identity = DirectoryIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        Ok(Self {
            root: Arc::new(canonical),
            root_identity,
        })
    }

    pub fn root(&self) -> &Path {
        self.root.as_path()
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
        })
    }
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
}
