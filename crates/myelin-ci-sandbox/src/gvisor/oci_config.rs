use super::*;
use crate::hardening::HardeningProfile;
use crate::user_namespace::{RunscInvocationMode, UserNamespaceConfig};
use crate::{ImageRef, JobKind, JobSpec};
use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const UNTRUSTED_UID: u32 = 65534;
const UNTRUSTED_GID: u32 = 65534;

const OCI_WORKSPACE_MOUNT: &str = "/workspace";
pub const OCI_CARGO_VENDOR_MOUNT: &str = "/opt/myelin/cargo-vendor";
pub const ENV_CARGO_VENDOR_ASSET: &str = "MYELIN_CARGO_VENDOR_ASSET";
pub const STRUCTURED_CARGO_HOME: &str = "/tmp/cargo-home";
pub const CARGO_SOURCE_REPLACE_ENV: &str = "CARGO_SOURCE_CRATES_IO_REPLACE_WITH";
pub const CARGO_VENDOR_DIRECTORY_ENV: &str = "CARGO_SOURCE_VENDORED_DIRECTORY";
pub const CARGO_SOURCE_REPLACE_CONFIG: &str = "source.crates-io.replace-with=\"vendored\"";
pub const CARGO_VENDOR_DIRECTORY_CONFIG: &str =
    "source.vendored.directory=\"/opt/myelin/cargo-vendor\"";
const OCI_CARGO_CONFIG_MOUNT: &str = "/tmp/cargo-home/config.toml";
const TEST_SERVER_CARGO_CONFIG_SOURCE: &str = "/server-owned/cargo/config.toml";
const TEST_CARGO_VENDOR_MOUNT_SOURCE: &str =
    "/var/lib/myelin/gvisor-assets/cargo-vendor-smoke-v1/vendor";
pub(super) const CARGO_VENDOR_SOURCE_NAME: &str = "vendored";

pub const SERVER_CARGO_CONFIG_TOML: &str = "[source.crates-io]\nreplace-with = \"vendored\"\n\n[source.vendored]\ndirectory = \"/opt/myelin/cargo-vendor\"\n";

fn is_admitted_structured_cargo_argv(command: &[String]) -> bool {
    let r = CARGO_SOURCE_REPLACE_CONFIG;
    let v = CARGO_VENDOR_DIRECTORY_CONFIG;
    let admitted: [Vec<&str>; 4] = [
        vec!["cargo", "build", "--locked", "--config", r, "--config", v],
        vec![
            "cargo", "test", "--locked", "--lib", "--config", r, "--config", v,
        ],
        vec![
            "cargo",
            "test",
            "--locked",
            "--lib",
            "--workspace",
            "--config",
            r,
            "--config",
            v,
        ],
        vec![
            "cargo",
            "clippy",
            "--locked",
            "--all-targets",
            "--config",
            r,
            "--config",
            v,
            "--",
            "-D",
            "warnings",
        ],
    ];
    admitted
        .iter()
        .any(|argv| command.iter().map(String::as_str).eq(argv.iter().copied()))
}

pub(super) fn validated_cargo_vendor_reference(spec: &JobSpec) -> Result<Option<ImageRef>, String> {
    let values = |name: &str| {
        spec.env
            .iter()
            .filter(|entry| entry.name == name)
            .map(|entry| entry.value.as_str())
            .collect::<Vec<_>>()
    };
    let selectors = values(ENV_CARGO_VENDOR_ASSET);
    if selectors.is_empty() {
        return Ok(None);
    }
    if spec.kind != JobKind::Ci || !is_admitted_structured_cargo_argv(&spec.command) {
        return Err(
            "a Cargo vendor asset may be selected only for a platform-owned structured CI Cargo recipe (build / test --lib / clippy)"
                .to_string(),
        );
    }
    if selectors.len() != 1 {
        return Err(format!(
            "{ENV_CARGO_VENDOR_ASSET} must appear exactly once when selecting a Cargo vendor asset"
        ));
    }
    if !spec.egress.allow.is_empty() {
        return Err(
            "a structured Cargo vendor build requires empty egress (network=none)".to_string(),
        );
    }
    for (name, expected) in [
        ("CARGO_HOME", STRUCTURED_CARGO_HOME),
        ("CARGO_NET_OFFLINE", "true"),
        (CARGO_SOURCE_REPLACE_ENV, CARGO_VENDOR_SOURCE_NAME),
        (CARGO_VENDOR_DIRECTORY_ENV, OCI_CARGO_VENDOR_MOUNT),
    ] {
        if values(name) != [expected] {
            return Err(format!(
                "a structured Cargo vendor build requires exactly {name}={expected} in the job environment"
            ));
        }
        if spec.secret_refs.iter().any(|secret| secret.name == name) {
            return Err(format!(
                "structured Cargo boundary variable {name} cannot be supplied through a secret"
            ));
        }
    }
    ImageRef::pinned(selectors[0].to_string())
        .map(Some)
        .map_err(|error| {
            format!("{ENV_CARGO_VENDOR_ASSET} is not a digest-pinned asset reference: {error}")
        })
}

pub(super) fn selected_cargo_vendor(
    spec: &JobSpec,
    registry: &crate::asset_registry::GvisorAssetRegistry,
) -> Result<Option<crate::asset_registry::VerifiedCargoVendor>, String> {
    let Some(reference) = validated_cargo_vendor_reference(spec)? else {
        return Ok(None);
    };
    registry
        .resolve_cargo_vendor(&reference)
        .cloned()
        .map(Some)
        .map_err(|error| error.to_string())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AbsoluteRootfs(PathBuf);

impl AbsoluteRootfs {
    pub(super) fn new(path: PathBuf) -> Result<Self, String> {
        if path.is_absolute() {
            Ok(Self(path))
        } else {
            Err(format!(
                "an OCI root.path override must be an absolute path, got {path:?}"
            ))
        }
    }

    fn as_path(&self) -> &Path {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OciWorkspaceMount {
    host_source: PathBuf,
}

impl OciWorkspaceMount {
    pub(crate) fn from_managed_workspace(
        workspace: &crate::workspace_manager::ManagedWorkspace,
    ) -> Self {
        OciWorkspaceMount {
            host_source: workspace.host_path().to_path_buf(),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_tests(host_source: PathBuf) -> Self {
        OciWorkspaceMount { host_source }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GitWireMounts {
    repo_source: PathBuf,
    quarantine_source: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CargoVendorBoundary {
    asset: crate::asset_registry::VerifiedCargoVendor,
    materialized_cargo_lock_sha256: Option<String>,
}

pub(super) struct FdBoundCargoVendor {
    _root_fd: OwnedFd,
    root_identity: (u64, u64),
    pub(super) vendor_mount_source: PathBuf,
}

impl std::fmt::Debug for FdBoundCargoVendor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FdBoundCargoVendor")
            .field("root_identity", &self.root_identity)
            .field("vendor_mount_source", &self.vendor_mount_source)
            .finish_non_exhaustive()
    }
}

impl GitWireMounts {
    pub(super) fn new(repo_source: PathBuf, quarantine_source: Option<PathBuf>) -> Self {
        GitWireMounts {
            repo_source,
            quarantine_source,
        }
    }

    fn bind_mounts_json(&self) -> Vec<String> {
        let mut mounts = vec![bind_mount_json(WIRE_REPO_MOUNT, &self.repo_source, true)];
        if let Some(quarantine_source) = &self.quarantine_source {
            mounts.push(bind_mount_json(
                WIRE_QUARANTINE_MOUNT,
                quarantine_source,
                false,
            ));
        }
        mounts
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum OciExecutionLayout {
    Rootless,
    RootlessWithHostMounts {
        absolute_rootfs: AbsoluteRootfs,
        mounts: GitWireMounts,
    },
    ExplicitUserNamespace { config: UserNamespaceConfig },
    ExplicitUserNamespaceWithWorkspace {
        config: UserNamespaceConfig,
        workspace: OciWorkspaceMount,
        absolute_rootfs: AbsoluteRootfs,
        cargo_vendor: Option<CargoVendorBoundary>,
    },
}

#[derive(Clone, PartialEq, Eq)]
pub struct OciConfig {
    pub(super) args: Vec<String>,
    pub(super) root_readonly: bool,
    pub(super) drop_all_caps: bool,
    pub(super) no_new_privileges: bool,
    pub(super) seccomp: bool,
    pub(super) has_network: bool,
    pub(super) pids_max: u32,
    pub(super) mem_bytes: u64,
    pub(super) tmpfs_bytes: u64,
    pub(super) extra_env: Vec<String>,
    pub(super) layout: OciExecutionLayout,
}

impl std::fmt::Debug for OciConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let env_names: Vec<&str> = self
            .extra_env
            .iter()
            .map(|entry| {
                entry
                    .split_once('=')
                    .map_or(entry.as_str(), |(name, _)| name)
            })
            .collect();
        formatter
            .debug_struct("OciConfig")
            .field("args", &self.args)
            .field("root_readonly", &self.root_readonly)
            .field("drop_all_caps", &self.drop_all_caps)
            .field("no_new_privileges", &self.no_new_privileges)
            .field("seccomp", &self.seccomp)
            .field("has_network", &self.has_network)
            .field("pids_max", &self.pids_max)
            .field("mem_bytes", &self.mem_bytes)
            .field("tmpfs_bytes", &self.tmpfs_bytes)
            .field("env_names", &env_names)
            .field("layout", &self.layout)
            .finish()
    }
}

impl Drop for OciConfig {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        for entry in &mut self.extra_env {
            entry.zeroize();
        }
    }
}

fn bind_mount_json(guest_dest: &str, host_source: &Path, readonly: bool) -> String {
    let src = host_source.to_string_lossy();
    let mode = if readonly { "ro" } else { "rw" };
    format!(
        "{{ \"destination\": {dest:?}, \"type\": \"bind\", \"source\": {src:?}, \
         \"options\": [\"bind\", \"{mode}\", \"nosuid\", \"nodev\"] }}",
        dest = guest_dest,
    )
}

impl OciConfig {
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn workspace_host_source_for_tests(&self) -> Option<&Path> {
        match &self.layout {
            OciExecutionLayout::ExplicitUserNamespaceWithWorkspace { workspace, .. } => {
                Some(&workspace.host_source)
            }
            _ => None,
        }
    }

    pub fn from_spec(spec: &JobSpec, profile: &HardeningProfile) -> OciConfig {
        let mut extra_env: Vec<String> = spec
            .env
            .iter()
            .map(|e| format!("{}={}", e.name, e.value))
            .collect();
        extra_env.extend(spec.resolved_secrets().process_env());
        Self::for_fixed_command(spec.command.clone(), spec.limits.mem_bytes, profile)
            .with_extra_env(extra_env)
    }

    pub(crate) fn for_fixed_command(
        command: Vec<String>,
        mem_bytes: u64,
        profile: &HardeningProfile,
    ) -> OciConfig {
        OciConfig {
            args: command,
            root_readonly: profile.read_only_root,
            drop_all_caps: profile.drop_all_caps,
            no_new_privileges: profile.no_new_privileges,
            seccomp: profile.seccomp,
            has_network: profile.network_device,
            pids_max: profile.pids_max,
            mem_bytes,
            tmpfs_bytes: profile.scratch_quota_bytes,
            extra_env: Vec::new(),
            layout: OciExecutionLayout::Rootless,
        }
    }

    fn require_still_rootless(&self) -> Result<(), String> {
        if matches!(self.layout, OciExecutionLayout::Rootless) {
            Ok(())
        } else {
            Err(
                "an execution-layout selection was already made on this config - layout \
                 selection is one-shot and must never be silently overwritten"
                    .to_string(),
            )
        }
    }

    pub(crate) fn with_rootless_host_mounts(
        mut self,
        absolute_rootfs: PathBuf,
        repo_source: PathBuf,
        quarantine_source: Option<PathBuf>,
    ) -> Result<OciConfig, String> {
        self.require_still_rootless()?;
        self.layout = OciExecutionLayout::RootlessWithHostMounts {
            absolute_rootfs: AbsoluteRootfs::new(absolute_rootfs)?,
            mounts: GitWireMounts::new(repo_source, quarantine_source),
        };
        Ok(self)
    }

    pub fn with_user_namespace(mut self, config: UserNamespaceConfig) -> Result<OciConfig, String> {
        self.require_still_rootless()?;
        self.layout = OciExecutionLayout::ExplicitUserNamespace { config };
        Ok(self)
    }

    pub(crate) fn with_explicit_user_namespace_and_workspace(
        mut self,
        config: UserNamespaceConfig,
        workspace: OciWorkspaceMount,
        absolute_rootfs: PathBuf,
    ) -> Result<OciConfig, String> {
        self.require_still_rootless()?;
        self.layout = OciExecutionLayout::ExplicitUserNamespaceWithWorkspace {
            config,
            workspace,
            absolute_rootfs: AbsoluteRootfs::new(absolute_rootfs)?,
            cargo_vendor: None,
        };
        Ok(self)
    }

    pub(crate) fn with_cargo_vendor(
        mut self,
        asset: crate::asset_registry::VerifiedCargoVendor,
    ) -> Result<OciConfig, String> {
        let (absolute_rootfs, slot) =
            match &mut self.layout {
                OciExecutionLayout::ExplicitUserNamespaceWithWorkspace {
                    absolute_rootfs,
                    cargo_vendor,
                    ..
                } => (absolute_rootfs, cargo_vendor),
                _ => return Err(
                    "a Cargo vendor asset requires the explicit-user-namespace workspace layout"
                        .to_string(),
                ),
            };
        if slot.is_some() {
            return Err(
                "a Cargo vendor asset was already selected for this OCI config".to_string(),
            );
        }
        let destination = absolute_rootfs
            .as_path()
            .join(OCI_CARGO_VENDOR_MOUNT.trim_start_matches('/'));
        let metadata = std::fs::symlink_metadata(&destination).map_err(|error| {
            format!(
                "pinned rootfs is missing the precreated {} mount destination: {error}",
                OCI_CARGO_VENDOR_MOUNT
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(format!(
                "pinned rootfs {} mount destination must be a real directory",
                OCI_CARGO_VENDOR_MOUNT
            ));
        }
        if std::fs::read_dir(&destination)
            .map_err(|error| format!("read Cargo vendor mount destination: {error}"))?
            .next()
            .is_some()
        {
            return Err(format!(
                "pinned rootfs {} mount destination must be empty",
                OCI_CARGO_VENDOR_MOUNT
            ));
        }
        *slot = Some(CargoVendorBoundary {
            asset,
            materialized_cargo_lock_sha256: None,
        });
        Ok(self)
    }

    pub(super) fn bind_materialized_cargo_lock(&mut self, digest: &str) -> Result<(), String> {
        match &mut self.layout {
            OciExecutionLayout::ExplicitUserNamespaceWithWorkspace {
                cargo_vendor: Some(boundary),
                ..
            } => {
                if boundary.materialized_cargo_lock_sha256.is_some() {
                    return Err(
                        "the materialized Cargo.lock digest was already bound to this launch"
                            .to_string(),
                    );
                }
                if digest != boundary.asset.cargo_lock_sha256() {
                    return Err(format!(
                        "cargo vendor asset lock mismatch: checked-out Cargo.lock is sha256:{digest}, but the selected asset is keyed to sha256:{}",
                        boundary.asset.cargo_lock_sha256()
                    ));
                }
                boundary.materialized_cargo_lock_sha256 = Some(digest.to_string());
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub(super) fn fd_bind_cargo_vendor_before_spawn(
        &self,
    ) -> Result<Option<FdBoundCargoVendor>, String> {
        match &self.layout {
            OciExecutionLayout::ExplicitUserNamespaceWithWorkspace {
                cargo_vendor: Some(boundary),
                ..
            } => {
                let materialized = boundary
                    .materialized_cargo_lock_sha256
                    .as_deref()
                    .ok_or_else(|| {
                        "structured Cargo launch reached verify-to-use without a materialized Cargo.lock digest"
                            .to_string()
                    })?;
                let path_c = CString::new(boundary.asset.path().as_os_str().as_encoded_bytes())
                    .map_err(|error| {
                        format!(
                            "Cargo vendor asset path {} contains an interior NUL: {error}",
                            boundary.asset.path().display()
                        )
                    })?;
                let raw_fd = unsafe {
                    libc::open(
                        path_c.as_ptr(),
                        libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    )
                };
                if raw_fd < 0 {
                    return Err(format!(
                        "open verified Cargo vendor tree {} with O_PATH|O_NOFOLLOW: {}",
                        boundary.asset.path().display(),
                        io::Error::last_os_error()
                    ));
                }
                let root_fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
                let opened_identity = crate::dirlock::fd_identity(&root_fd)
                    .map_err(|error| format!("fstat fd-bound Cargo vendor tree: {error}"))?;
                if opened_identity != boundary.asset.identity() {
                    return Err(format!(
                        "Cargo vendor asset pathname no longer names its registry-verified inode: \
                         expected {:?}, opened {opened_identity:?}",
                        boundary.asset.identity()
                    ));
                }

                let fd_bound_root = PathBuf::from(format!(
                    "/proc/{}/fd/{}",
                    std::process::id(),
                    root_fd.as_raw_fd()
                ));
                let fd_path_metadata = std::fs::metadata(&fd_bound_root).map_err(|error| {
                    format!(
                        "resolve held Cargo vendor descriptor {}: {error}",
                        fd_bound_root.display()
                    )
                })?;
                if (fd_path_metadata.dev(), fd_path_metadata.ino()) != opened_identity {
                    return Err(
                        "Cargo vendor /proc fd source did not resolve to the held inode"
                            .to_string(),
                    );
                }

                boundary
                    .asset
                    .verify_before_spawn_at(&fd_bound_root, materialized)?;

                let current_path_metadata =
                    std::fs::metadata(boundary.asset.path()).map_err(|error| {
                        format!(
                            "Cargo vendor pathname changed during fd-bound verification: {error}"
                        )
                    })?;
                let current_path_identity =
                    (current_path_metadata.dev(), current_path_metadata.ino());
                let after_identity = crate::dirlock::fd_identity(&root_fd)
                    .map_err(|error| format!("re-fstat fd-bound Cargo vendor tree: {error}"))?;
                if current_path_identity != opened_identity || after_identity != opened_identity {
                    return Err(format!(
                        "Cargo vendor tree identity changed during verification: opened \
                         {opened_identity:?}, pathname {current_path_identity:?}, fd after \
                         verification {after_identity:?}"
                    ));
                }

                Ok(Some(FdBoundCargoVendor {
                    vendor_mount_source: boundary.asset.path().join("vendor"),
                    _root_fd: root_fd,
                    root_identity: opened_identity,
                }))
            }
            _ => Ok(None),
        }
    }

    pub(super) fn has_cargo_vendor(&self) -> bool {
        matches!(
            self.layout,
            OciExecutionLayout::ExplicitUserNamespaceWithWorkspace {
                cargo_vendor: Some(_),
                ..
            }
        )
    }

    pub fn invocation_mode(&self) -> RunscInvocationMode {
        match &self.layout {
            OciExecutionLayout::Rootless | OciExecutionLayout::RootlessWithHostMounts { .. } => {
                RunscInvocationMode::Rootless
            }
            OciExecutionLayout::ExplicitUserNamespace { config }
            | OciExecutionLayout::ExplicitUserNamespaceWithWorkspace { config, .. } => {
                RunscInvocationMode::ExplicitUserNamespace(*config)
            }
        }
    }

    pub fn with_extra_env(mut self, env: Vec<String>) -> OciConfig {
        self.extra_env = env;
        self
    }

    pub fn to_json(&self) -> Result<String, String> {
        self.to_json_zeroizing().map(|json| json.to_string())
    }

    pub(super) fn to_json_zeroizing(&self) -> Result<zeroize::Zeroizing<String>, String> {
        let cargo_config_source = self
            .has_cargo_vendor()
            .then(|| Path::new(TEST_SERVER_CARGO_CONFIG_SOURCE));
        let cargo_vendor_source = self
            .has_cargo_vendor()
            .then(|| Path::new(TEST_CARGO_VENDOR_MOUNT_SOURCE));
        self.to_json_zeroizing_with_cargo_sources(cargo_config_source, cargo_vendor_source)
    }

    pub(super) fn to_json_zeroizing_with_cargo_sources(
        &self,
        cargo_config_host_source: Option<&Path>,
        cargo_vendor_host_source: Option<&Path>,
    ) -> Result<zeroize::Zeroizing<String>, String> {
        use std::fmt::Write as _;

        let args = self
            .args
            .iter()
            .map(|a| format!("{a:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        let net_ns = if self.has_network {
            "{ \"type\": \"network\" }"
        } else {
            "{ \"type\": \"network\", \"path\": \"\" }"
        };
        let mut env_json = zeroize::Zeroizing::new(String::new());
        write!(
            &mut *env_json,
            "{:?}",
            "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
        )
        .expect("writing JSON into a String cannot fail");
        for e in &self.extra_env {
            env_json.push_str(", ");
            write!(&mut *env_json, "{e:?}").expect("writing JSON into a String cannot fail");
        }
        let has_cargo_vendor = self.has_cargo_vendor();
        let cargo_home_tmpfs_bytes = if has_cargo_vendor {
            if self.tmpfs_bytes < 2 {
                return Err(
                    "a structured Cargo build requires at least two bytes of writable tmpfs quota"
                        .to_string(),
                );
            }
            self.tmpfs_bytes / 2
        } else {
            0
        };
        let general_tmpfs_bytes = self.tmpfs_bytes - cargo_home_tmpfs_bytes;
        let mut mounts = vec![format!(
            "{{ \"destination\": \"/tmp\", \"type\": \"tmpfs\", \"source\": \"tmpfs\", \
             \"options\": [\"nosuid\", \"nodev\", \"mode=1777\", \"size={}\"] }}",
            general_tmpfs_bytes
        )];
        match &self.layout {
            OciExecutionLayout::Rootless | OciExecutionLayout::ExplicitUserNamespace { .. } => {}
            OciExecutionLayout::RootlessWithHostMounts {
                mounts: wire_mounts,
                ..
            } => {
                mounts.extend(wire_mounts.bind_mounts_json());
            }
            OciExecutionLayout::ExplicitUserNamespaceWithWorkspace {
                workspace,
                cargo_vendor,
                ..
            } => {
                mounts.push(bind_mount_json(
                    OCI_WORKSPACE_MOUNT,
                    &workspace.host_source,
                    false,
                ));
                if cargo_vendor.is_some() {
                    mounts.push(format!(
                        "{{ \"destination\": {destination:?}, \"type\": \"tmpfs\", \
                         \"source\": \"tmpfs\", \"options\": [\"rw\", \"nosuid\", \
                         \"nodev\", \"mode=0700\", \"uid={uid}\", \"gid={gid}\", \
                         \"size={size}\"] }}",
                        destination = STRUCTURED_CARGO_HOME,
                        uid = UNTRUSTED_UID,
                        gid = UNTRUSTED_GID,
                        size = cargo_home_tmpfs_bytes,
                    ));
                    let vendor_source = cargo_vendor_host_source.ok_or_else(|| {
                        "refusing Cargo vendor OCI config without a verified vendor source"
                            .to_string()
                    })?;
                    mounts.push(bind_mount_json(OCI_CARGO_VENDOR_MOUNT, vendor_source, true));
                    let config_source = cargo_config_host_source.ok_or_else(|| {
                        "refusing Cargo vendor OCI config without a server config source"
                            .to_string()
                    })?;
                    mounts.push(bind_mount_json(OCI_CARGO_CONFIG_MOUNT, config_source, true));
                }
            }
        }
        let mounts_json = mounts.join(", ");
        let root_path = match &self.layout {
            OciExecutionLayout::Rootless | OciExecutionLayout::ExplicitUserNamespace { .. } => {
                "rootfs".to_string()
            }
            OciExecutionLayout::RootlessWithHostMounts {
                absolute_rootfs, ..
            }
            | OciExecutionLayout::ExplicitUserNamespaceWithWorkspace {
                absolute_rootfs, ..
            } => absolute_rootfs.as_path().to_string_lossy().to_string(),
        };
        let process_cwd = match &self.layout {
            OciExecutionLayout::ExplicitUserNamespaceWithWorkspace { .. } => OCI_WORKSPACE_MOUNT,
            OciExecutionLayout::Rootless
            | OciExecutionLayout::RootlessWithHostMounts { .. }
            | OciExecutionLayout::ExplicitUserNamespace { .. } => "/",
        };
        let (namespaces_json, id_mappings_json) = match &self.layout {
            OciExecutionLayout::Rootless | OciExecutionLayout::RootlessWithHostMounts { .. } => {
                (net_ns.to_string(), String::new())
            }
            OciExecutionLayout::ExplicitUserNamespace { config }
            | OciExecutionLayout::ExplicitUserNamespaceWithWorkspace { config, .. } => (
                format!("{net_ns}, {{ \"type\": \"user\" }}"),
                format!(
                    ",\n    \"uidMappings\": [ {{ \"containerID\": 0, \"hostID\": {ruid}, \
                     \"size\": 1 }}, {{ \"containerID\": {untrusted_uid}, \"hostID\": {suid}, \
                     \"size\": 1 }} ],\n    \
                     \"gidMappings\": [ {{ \"containerID\": 0, \"hostID\": {rgid}, \"size\": 1 }}, \
                     {{ \"containerID\": {untrusted_gid}, \"hostID\": {sgid}, \"size\": 1 }} ]",
                    ruid = config.runner_uid(),
                    rgid = config.runner_gid(),
                    suid = config.subordinate_uid(),
                    sgid = config.subordinate_gid(),
                    untrusted_uid = UNTRUSTED_UID,
                    untrusted_gid = UNTRUSTED_GID,
                ),
            ),
        };
        Ok(zeroize::Zeroizing::new(format!(
            "{{\n  \"ociVersion\": \"1.0.0\",\n  \"process\": {{\n    \
             \"user\": {{ \"uid\": {uid}, \"gid\": {gid} }},\n    \
             \"args\": [{args}],\n    \"cwd\": {process_cwd:?},\n    \
             \"env\": [{env_json}],\n    \
             \"noNewPrivileges\": {nnp},\n    \
             \"rlimits\": [{{ \"type\": \"RLIMIT_NPROC\", \"hard\": {pids}, \"soft\": {pids} }}],\n    \
             \"capabilities\": {{ \"bounding\": [], \"effective\": [], \"permitted\": [] }}\n  }},\n  \
             \"root\": {{ \"path\": {root_path:?}, \"readonly\": {ro} }},\n  \
             \"mounts\": [ {mounts_json} ],\n  \
             \"linux\": {{\n    \"resources\": {{ \"memory\": {{ \"limit\": {mem} }}, \
             \"pids\": {{ \"limit\": {pids} }} }},\n    \
             \"seccomp\": {{ \"defaultAction\": \"SCMP_ACT_ERRNO\" }},\n    \
             \"namespaces\": [ {namespaces_json} ]{id_mappings_json}\n  }}\n}}",
            uid = UNTRUSTED_UID,
            gid = UNTRUSTED_GID,
            args = args,
            process_cwd = process_cwd,
            env_json = &*env_json,
            nnp = self.no_new_privileges,
            ro = self.root_readonly,
            mounts_json = mounts_json,
            mem = self.mem_bytes,
            pids = self.pids_max,
            namespaces_json = namespaces_json,
            id_mappings_json = id_mappings_json,
        )))
    }

    pub fn root_readonly(&self) -> bool {
        self.root_readonly
    }
    pub fn has_network(&self) -> bool {
        self.has_network
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::gvisor::test_fixtures::*;
    use crate::hardening::HardeningProfile;
    use crate::user_namespace::{RunscInvocationMode, UserNamespaceConfig};
    use std::path::{Path, PathBuf};

    #[test]
    fn structured_cargo_launch_spec_has_verified_ro_vendor_server_config_and_bounded_writable_home()
    {
        let fixture = cargo_boundary_fixture("structured-launch");
        let cfg = wired_cargo_config(&fixture);
        let json = cfg.to_json().unwrap();
        assert!(json.contains("CARGO_HOME=/tmp/cargo-home"), "{json}");
        assert!(json.contains("CARGO_NET_OFFLINE=true"), "{json}");
        assert!(json.contains("CARGO_SOURCE_CRATES_IO_REPLACE_WITH=vendored"));
        assert!(json.contains("CARGO_SOURCE_VENDORED_DIRECTORY=/opt/myelin/cargo-vendor"));
        assert!(json.contains("\"destination\": \"/tmp\""), "{json}");
        assert_eq!(
            json.matches("\"type\": \"tmpfs\"").count(),
            2,
            "the structured launch has exactly /tmp plus its nested Cargo-home tmpfs: {json}"
        );
        assert_eq!(
            json.matches("\"size=33554432\"").count(),
            2,
            "the two tmpfs quotas partition 64 MiB into 32 MiB + 32 MiB, totaling exactly the one declared bound: {json}"
        );
        assert!(
            json.contains("\"destination\": \"/tmp/cargo-home\"")
                && json.contains("\"uid=65534\"")
                && json.contains("\"gid=65534\"")
                && json.contains("\"mode=0700\"")
                && json.contains("\"rw\""),
            "the structured Cargo home must be an explicit writable mount owned by the workload: {json}"
        );
        assert!(json.contains("\"destination\": \"/opt/myelin/cargo-vendor\""));
        assert!(json.contains("\"destination\": \"/tmp/cargo-home/config.toml\""));
        assert_eq!(json.matches("\"ro\"").count(), 2, "{json}");

        let staged = stage_production_bundle(&cfg, &fixture.rootfs).unwrap();
        let staged_config = std::fs::read_to_string(staged.path.join("cargo-config.toml")).unwrap();
        assert_eq!(staged_config, SERVER_CARGO_CONFIG_TOML);
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(staged.path.join("cargo-config.toml"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o444
        );
        let staged_json = std::fs::read_to_string(staged.path.join("config.json")).unwrap();
        assert!(staged_json.contains(&format!(
            "\"source\": {:?}",
            staged.path.join("cargo-config.toml").to_string_lossy()
        )));
        assert!(staged_json.contains("\"destination\": \"/tmp/cargo-home/config.toml\""));
    }

    #[test]
    fn cargo_vendor_serialization_missing_sources_returns_typed_refusal_without_panicking() {
        let fixture = cargo_boundary_fixture("typed-source-refusal");
        let cfg = wired_cargo_config(&fixture);
        let config_source = Path::new(TEST_SERVER_CARGO_CONFIG_SOURCE);
        let vendor_source = Path::new(TEST_CARGO_VENDOR_MOUNT_SOURCE);

        let missing_vendor = cfg
            .to_json_zeroizing_with_cargo_sources(Some(config_source), None)
            .expect_err("a missing verified vendor source must be a typed refusal");
        assert!(missing_vendor.contains("without a verified vendor source"));

        let missing_config = cfg
            .to_json_zeroizing_with_cargo_sources(None, Some(vendor_source))
            .expect_err("a missing server config source must be a typed refusal");
        assert!(missing_config.contains("without a server config source"));
    }

    #[test]
    fn free_form_command_launch_spec_gets_no_cargo_vendor_boundary() {
        let fixture = cargo_boundary_fixture("free-form");
        let mut free_form = structured_cargo_spec(&fixture.reference);
        free_form.command = vec!["/bin/test".into()];
        free_form.env.clear();
        assert!(selected_cargo_vendor(&free_form, &fixture.registry)
            .unwrap()
            .is_none());
        let profile = HardeningProfile::derive(&free_form);
        let json = OciConfig::from_spec(&free_form, &profile)
            .with_explicit_user_namespace_and_workspace(
                UserNamespaceConfig::for_tests(1000, 1000, 100_005, 200_005),
                OciWorkspaceMount::for_tests(PathBuf::from("/host/workspace")),
                PathBuf::from("/abs/staged-rootfs"),
            )
            .unwrap()
            .to_json()
            .unwrap();
        assert!(!json.contains(OCI_CARGO_VENDOR_MOUNT), "{json}");
        assert!(!json.contains(OCI_CARGO_CONFIG_MOUNT), "{json}");
        assert!(!json.contains("CARGO_HOME="), "{json}");
        assert!(!json.contains("CARGO_NET_OFFLINE="), "{json}");
    }

    #[test]
    fn structured_cargo_argv_allowlist_admits_build_test_clippy_and_rejects_others() {
        let s = |v: &[&str]| v.iter().map(|x| (*x).to_string()).collect::<Vec<_>>();
        let r = CARGO_SOURCE_REPLACE_CONFIG;
        let v = CARGO_VENDOR_DIRECTORY_CONFIG;
        for argv in [
            vec!["cargo", "build", "--locked", "--config", r, "--config", v],
            vec![
                "cargo", "test", "--locked", "--lib", "--config", r, "--config", v,
            ],
            vec![
                "cargo",
                "test",
                "--locked",
                "--lib",
                "--workspace",
                "--config",
                r,
                "--config",
                v,
            ],
            vec![
                "cargo",
                "clippy",
                "--locked",
                "--all-targets",
                "--config",
                r,
                "--config",
                v,
                "--",
                "-D",
                "warnings",
            ],
        ] {
            assert!(
                is_admitted_structured_cargo_argv(&s(&argv)),
                "must admit {argv:?}"
            );
        }
        for argv in [
            vec!["cargo", "build"],
            vec!["cargo", "run", "--locked", "--config", r, "--config", v],
            vec!["cargo", "test", "--config", r, "--config", v],
            vec![
                "cargo",
                "clippy",
                "--locked",
                "--all-targets",
                "--",
                "-D",
                "warnings",
                "--config",
                r,
                "--config",
                v,
            ],
            vec!["/bin/sh", "-c", "cargo build"],
        ] {
            assert!(
                !is_admitted_structured_cargo_argv(&s(&argv)),
                "must reject {argv:?}"
            );
        }
    }

    #[test]
    fn structured_cargo_vendor_selection_refuses_nonempty_egress_defense_in_depth() {
        let fixture = cargo_boundary_fixture("egress-refusal");
        let mut job = structured_cargo_spec(&fixture.reference);
        job.egress.allow = vec!["registry.example:443".into()];
        let error = selected_cargo_vendor(&job, &fixture.registry)
            .expect_err("the sandbox boundary must independently require network=none");
        assert!(error.contains("empty egress (network=none)"), "{error}");
    }

    #[test]
    fn server_cargo_config_replaces_crates_io_with_the_verified_vendor_directory() {
        assert_eq!(
            SERVER_CARGO_CONFIG_TOML,
            "[source.crates-io]\nreplace-with = \"vendored\"\n\n[source.vendored]\ndirectory = \"/opt/myelin/cargo-vendor\"\n"
        );
    }

    #[test]
    fn verified_cargo_vendor_mount_uses_canonical_real_path_and_reverifies_next_launch() {
        let fixture = cargo_boundary_fixture("vendor-real-path");
        let cfg = wired_cargo_config(&fixture);
        let staged = stage_production_bundle(&cfg, &fixture.rootfs)
            .expect("the unchanged tree must verify and stage");
        let source = staged
            ._cargo_vendor
            .as_ref()
            .expect("structured staging holds the verified vendor capability")
            .vendor_mount_source
            .clone();

        assert!(
            !source.starts_with("/proc/"),
            "vendor mount source must be a real path the gofer can open, not a /proc/fd symlink: \
             {source:?}"
        );
        assert_eq!(
            std::fs::read_to_string(source.join("itoa-1.0.15/lib.rs")).unwrap(),
            "pub fn fixture() {}",
        );
        let staged_json = std::fs::read_to_string(staged.path.join("config.json")).unwrap();
        assert!(
            staged_json.contains(&format!("\"source\": {:?}", source.to_string_lossy())),
            "the OCI mount must consume the verified canonical real-path source: {staged_json}"
        );

        let asset_path = fixture.root.join("asset");
        let moved_path = fixture.root.join("asset-moved-after-verify");
        std::fs::rename(&asset_path, &moved_path).unwrap();
        std::fs::create_dir_all(asset_path.join("vendor/itoa-1.0.15")).unwrap();
        std::fs::write(
            asset_path.join("vendor/itoa-1.0.15/lib.rs"),
            b"pub fn replacement() {}",
        )
        .unwrap();

        let error = match stage_production_bundle(&cfg, &fixture.rootfs) {
            Ok(_) => panic!("a later launch must refuse the replacement pathname inode"),
            Err(error) => error,
        };
        assert!(
            error.contains("no longer names its registry-verified inode"),
            "{error}"
        );
    }

    #[test]
    fn workspace_cargo_config_cannot_shadow_structured_source_boundary() {
        let fixture = cargo_boundary_fixture("precedence");
        let workspace = fixture.root.join("workspace");
        std::fs::create_dir_all(workspace.join(".cargo")).unwrap();
        std::fs::write(
            workspace.join(".cargo/config.toml"),
            b"[source.crates-io]\nreplace-with='tenant'\n[source.tenant]\ndirectory='/workspace/tenant'\n",
        )
        .unwrap();
        let legacy_cargo_home = fixture.root.join("legacy-cargo-home");
        std::fs::create_dir(&legacy_cargo_home).unwrap();
        std::fs::write(
            legacy_cargo_home.join("config"),
            b"[source.crates-io]\nreplace-with='legacy'\n[source.legacy]\ndirectory='/tmp/legacy'\n",
        )
        .unwrap();
        let cfg = wired_cargo_config(&fixture);
        let json = cfg.to_json().unwrap();
        assert_eq!(
            cfg.args,
            [
                "cargo",
                "build",
                "--locked",
                "--config",
                "source.crates-io.replace-with=\"vendored\"",
                "--config",
                "source.vendored.directory=\"/opt/myelin/cargo-vendor\"",
            ],
            "platform CLI config must outrank both workspace and legacy Cargo-home config files"
        );
        assert!(json.contains("CARGO_SOURCE_CRATES_IO_REPLACE_WITH=vendored"));
        assert!(json.contains("CARGO_SOURCE_VENDORED_DIRECTORY=/opt/myelin/cargo-vendor"));
        assert!(json.contains("\"destination\": \"/tmp/cargo-home/config.toml\""));
        assert!(json.contains("\"ro\""));
        assert!(!json.contains("/workspace/tenant"));
    }

    #[test]
    fn oci_config_enforces_the_backend_independent_hardening() {
        let cfg = GvisorBackend::oci_config(&spec(vec![])).unwrap();
        assert!(cfg.root_readonly());
        assert!(!cfg.has_network(), "no allowlist ⇒ no network interface");
        let json = cfg.to_json().unwrap();
        assert!(json.contains("\"readonly\": true"));
        assert!(json.contains("\"noNewPrivileges\": true"));
        assert!(
            json.contains("SCMP_ACT_ERRNO"),
            "a seccomp profile is attached"
        );
        assert!(
            json.contains("\"bounding\": []"),
            "all capabilities dropped"
        );
        assert!(
            json.contains("\"type\": \"RLIMIT_NPROC\"")
                && json.contains("\"hard\": 64")
                && json.contains("\"soft\": 64"),
            "rootless gVisor gets an in-sandbox process ceiling independent of host cgroups"
        );
        assert!(
            json.contains("\"uid\": 65534") && json.contains("\"gid\": 65534"),
            "the untrusted process must run as a non-root uid/gid (65534)"
        );
        assert!(
            json.contains("\"cwd\": \"/\""),
            "process.cwd must be set or the OCI runtime rejects the spec"
        );
        assert!(
            json.contains(&format!("\"limit\": {}", 256u64 << 20)),
            "the OCI config must carry the memory ceiling (linux.resources.memory.limit) from spec.limits.mem_bytes"
        );
        assert!(
            json.contains("\"destination\": \"/tmp\"") && json.contains("\"type\": \"tmpfs\""),
            "a size-bounded writable /tmp tmpfs must be mounted (no unbounded host-RAM-backed scratch)"
        );
        assert!(
            json.contains(&format!("size={}", 1u64 << 30)) && json.contains("mode=1777"),
            "the /tmp tmpfs must be sized from spec.limits.tmpfs_bytes and writable by the non-root payload"
        );
        assert!(
            !json.contains("\"type\": \"user\"") && !json.contains("uidMappings"),
            "Rootless mode (the default) must never declare a user namespace or uid/gid mappings \
             - runsc --rootless installs its own, and a doubly-declared userns fails the gofer"
        );
        assert!(
            json.contains("\"path\": \"rootfs\""),
            "ordinary rootless launch must use the bundle-relative rootfs: {json}"
        );
        assert_eq!(
            cfg.invocation_mode(),
            RunscInvocationMode::Rootless,
            "a config with no explicit user namespace attached must report Rootless"
        );
    }

    #[test]
    fn oci_config_rootless_with_host_mounts_emits_absolute_root_and_the_bind_mounts() {
        let cfg = GvisorBackend::oci_config(&spec(vec![]))
            .unwrap()
            .with_rootless_host_mounts(
                PathBuf::from("/abs/staged-rootfs"),
                PathBuf::from("/host/repo"),
                Some(PathBuf::from("/host/quarantine")),
            )
            .expect("an absolute rootfs override must be accepted");
        assert_eq!(
            cfg.invocation_mode(),
            RunscInvocationMode::Rootless,
            "host mounts alone must not imply a user namespace"
        );
        let json = cfg.to_json().unwrap();
        assert!(
            json.contains("\"path\": \"/abs/staged-rootfs\""),
            "the absolute rootfs override must be emitted verbatim: {json}"
        );
        assert!(
            json.contains("\"destination\": \"/repo\"") && json.contains("\"ro\""),
            "the RO repo bind mount must be present: {json}"
        );
        assert!(
            json.contains("\"destination\": \"/quarantine\"") && json.contains("\"rw\""),
            "the writable quarantine bind mount must be present: {json}"
        );
        assert!(
            !json.contains("\"type\": \"user\"") && !json.contains("uidMappings"),
            "RootlessWithHostMounts must never declare a user namespace or uid/gid mappings"
        );
    }

    #[test]
    fn oci_config_with_rootless_host_mounts_refuses_a_relative_rootfs() {
        let result = GvisorBackend::oci_config(&spec(vec![]))
            .unwrap()
            .with_rootless_host_mounts(
                PathBuf::from("relative/staged-rootfs"),
                PathBuf::from("/host/repo"),
                None,
            );
        assert!(
            result.is_err(),
            "a non-absolute rootfs override must be refused, not silently accepted"
        );
    }

    #[test]
    fn oci_config_explicit_userns_with_workspace_emits_absolute_root_mappings_and_the_fixed_mount()
    {
        let config = UserNamespaceConfig::for_tests(1000, 1000, 100_005, 200_005);
        let workspace = OciWorkspaceMount::for_tests(PathBuf::from("/host/workspace-subvol"));
        let cfg = GvisorBackend::oci_config(&spec(vec![]))
            .unwrap()
            .with_explicit_user_namespace_and_workspace(
                config,
                workspace,
                PathBuf::from("/abs/staged-rootfs"),
            )
            .expect("an absolute rootfs override must be accepted");
        assert_eq!(
            cfg.invocation_mode(),
            RunscInvocationMode::ExplicitUserNamespace(config)
        );
        let json = cfg.to_json().unwrap();
        assert!(
            json.contains("\"path\": \"/abs/staged-rootfs\""),
            "the workspace layout must use an absolute rootfs override: {json}"
        );
        assert!(
            json.contains("\"type\": \"user\""),
            "a user namespace must be declared: {json}"
        );
        assert!(
            json.contains("\"containerID\": 65534, \"hostID\": 100005, \"size\": 1"),
            "container uid 65534 must map to the leased subordinate host uid: {json}"
        );
        assert!(
            json.contains("\"destination\": \"/workspace\"")
                && json.contains("\"source\": \"/host/workspace-subvol\"")
                && json.contains("\"rw\""),
            "exactly one fixed writable workspace bind mount must be present: {json}"
        );
        assert!(
            json.contains("\"cwd\": \"/workspace\""),
            "workspace-backed workloads must start in the checked-out tree: {json}"
        );
        assert_eq!(
            json.matches("\"destination\": \"/workspace\"").count(),
            1,
            "exactly one workspace mount, never more: {json}"
        );
    }

    #[test]
    fn oci_config_with_explicit_user_namespace_and_workspace_refuses_a_relative_rootfs() {
        let config = UserNamespaceConfig::for_tests(1000, 1000, 100_005, 200_005);
        let workspace = OciWorkspaceMount::for_tests(PathBuf::from("/host/workspace-subvol"));
        let result = GvisorBackend::oci_config(&spec(vec![]))
            .unwrap()
            .with_explicit_user_namespace_and_workspace(
                config,
                workspace,
                PathBuf::from("relative/staged-rootfs"),
            );
        assert!(
            result.is_err(),
            "a non-absolute rootfs override must be refused, not silently accepted"
        );
    }

    #[test]
    fn oci_config_layout_selection_is_one_shot() {
        let config = UserNamespaceConfig::for_tests(1000, 1000, 100_005, 200_005);
        let result = GvisorBackend::oci_config(&spec(vec![]))
            .unwrap()
            .with_user_namespace(config)
            .unwrap()
            .with_rootless_host_mounts(
                PathBuf::from("/abs/staged-rootfs"),
                PathBuf::from("/host/repo"),
                None,
            );
        assert!(
            result.is_err(),
            "attaching host mounts after a user namespace was already selected must refuse, not \
             silently discard the user namespace"
        );
        let result = GvisorBackend::oci_config(&spec(vec![]))
            .unwrap()
            .with_rootless_host_mounts(
                PathBuf::from("/abs/staged-rootfs"),
                PathBuf::from("/host/repo"),
                None,
            )
            .unwrap()
            .with_user_namespace(config);
        assert!(
            result.is_err(),
            "attaching a user namespace after host mounts were already selected must refuse, not \
             silently discard the mounts"
        );
    }

    #[test]
    fn oci_config_rootless_with_host_mounts_never_accepts_an_arbitrary_destination_or_mode() {
        let cfg = GvisorBackend::oci_config(&spec(vec![]))
            .unwrap()
            .with_rootless_host_mounts(
                PathBuf::from("/abs/staged-rootfs"),
                PathBuf::from("/host/repo"),
                Some(PathBuf::from("/host/quarantine")),
            )
            .unwrap();
        let json = cfg.to_json().unwrap();
        assert!(!json.contains("\"destination\": \"/workspace\""));
        assert!(json.contains(WIRE_REPO_MOUNT));
        assert!(json.contains(WIRE_QUARANTINE_MOUNT));
    }

    #[test]
    fn oci_config_propagates_the_jobs_declared_env_into_process_env() {
        let mut s = spec(vec![]);
        s.env = vec![
            crate::EnvVar {
                name: "CARGO_NET_OFFLINE".into(),
                value: "true".into(),
            },
            crate::EnvVar {
                name: "CARGO_HOME".into(),
                value: "/workspace/.cargo".into(),
            },
        ];
        let json = GvisorBackend::oci_config(&s).unwrap().to_json().unwrap();
        assert!(
            json.contains("CARGO_NET_OFFLINE=true"),
            "declared env dropped: {json}"
        );
        assert!(
            json.contains("CARGO_HOME=/workspace/.cargo"),
            "declared env dropped: {json}"
        );
        assert!(
            json.contains("PATH=/usr/local/sbin"),
            "base PATH lost: {json}"
        );
    }

    #[test]
    fn injected_secret_reaches_oci_process_env_without_entering_debug_records() {
        let mut s = spec(vec![]);
        s.secret_refs = vec![crate::SecretRef {
            name: "DEPLOY_TOKEN".into(),
            handle: "opaque:deploy".into(),
        }];
        let material = ["boundary", "-only-material"].concat();
        let s = s
            .with_resolved_secrets(vec![crate::ResolvedSecretEnv::new(
                "DEPLOY_TOKEN",
                material.clone(),
            )])
            .expect("the exact declared binding set must couple to redaction");

        let cfg = GvisorBackend::oci_config(&s).expect("covered injection is launchable");
        let json = cfg.to_json().unwrap();
        assert!(json.contains(&format!("DEPLOY_TOKEN={material}")));
        assert!(!format!("{s:?}").contains(&material));
        assert!(!format!("{:?}", s.resolved_secrets().redaction_plan()).contains(&material));
        assert!(!format!("{cfg:?}").contains(&material));
    }

    #[test]
    fn oci_config_rootless_with_host_mounts_omits_the_quarantine_mount_when_absent() {
        let cfg = GvisorBackend::oci_config(&spec(vec![]))
            .unwrap()
            .with_rootless_host_mounts(
                PathBuf::from("/abs/staged-rootfs"),
                PathBuf::from("/host/repo"),
                None,
            )
            .unwrap();
        let json = cfg.to_json().unwrap();
        assert!(json.contains(WIRE_REPO_MOUNT));
        assert!(!json.contains(WIRE_QUARANTINE_MOUNT));
    }

    #[test]
    fn oci_config_with_user_namespace_emits_the_exact_two_entry_mapping() {
        let config = UserNamespaceConfig::for_tests(1000, 1000, 100_005, 200_005);
        let cfg = GvisorBackend::oci_config(&spec(vec![]))
            .unwrap()
            .with_user_namespace(config)
            .expect("a fresh Rootless config must accept a user-namespace layout selection");
        assert_eq!(
            cfg.invocation_mode(),
            RunscInvocationMode::ExplicitUserNamespace(config)
        );
        let json = cfg.to_json().unwrap();
        assert!(
            json.contains("\"type\": \"user\""),
            "a user namespace must be declared: {json}"
        );
        assert!(
            json.contains("\"containerID\": 0, \"hostID\": 1000, \"size\": 1"),
            "container uid/gid 0 must map to the runner's own real identity: {json}"
        );
        assert!(
            json.contains("\"containerID\": 65534, \"hostID\": 100005, \"size\": 1"),
            "container uid 65534 must map to the leased subordinate host uid: {json}"
        );
        assert!(
            json.contains("\"containerID\": 65534, \"hostID\": 200005, \"size\": 1"),
            "container gid 65534 must map to the leased subordinate host gid: {json}"
        );
        assert!(json.contains("\"readonly\": true"));
        assert!(json.contains("\"noNewPrivileges\": true"));
        assert!(json.contains("\"uid\": 65534") && json.contains("\"gid\": 65534"));
        assert!(
            json.contains("\"path\": \"rootfs\""),
            "explicit userns WITHOUT a workspace mount involves no host bind mount, so it must \
             still use the bundle-relative rootfs, not an absolute override: {json}"
        );
    }
}
