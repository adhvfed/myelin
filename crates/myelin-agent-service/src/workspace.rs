use std::ffi::CString;
use std::fs::{DirBuilder, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use uuid::Uuid;

const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
pub const MAX_WORKSPACE_FILE_BYTES: usize = 256 * 1024;
const MAX_WORKSPACE_PATH_BYTES: usize = 1024;
const MAX_WORKSPACE_PATH_DEPTH: usize = 32;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WrittenWorkspaceFile {
    pub path: String,
    pub byte_len: usize,
    pub content_digest: String,
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
        })
    }
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
