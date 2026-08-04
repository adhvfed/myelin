//! The hardened OCI `config.json` the backend hands `runsc`: the guest mount vocabulary, the
//! digest-pinned Cargo-vendor boundary, and the fd-bound execution layout.

use super::*;
use crate::hardening::HardeningProfile;
use crate::user_namespace::{RunscInvocationMode, UserNamespaceConfig};
use crate::{ImageRef, JobKind, JobSpec};
use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// The unprivileged uid/gid the untrusted `spec.command` runs as INSIDE the gVisor sandbox. Untrusted
/// code must NEVER be uid 0 even within the userspace-kernel boundary (defense in depth + hygiene);
/// 65534 = nobody/nogroup (numeric ⇒ no `/etc/passwd` lookup). Unlike Firecracker, gVisor's exit
/// capture needs no forge defense — `runsc run` returns the container process's REAL exit code
/// directly to THIS host process (there is no shared serial console to spoof) — but the workload is
/// still dropped to this non-root uid/gid in the OCI config so it never runs as root in the sandbox.
const UNTRUSTED_UID: u32 = 65534;
const UNTRUSTED_GID: u32 = 65534;

/// The fixed guest mount point a workspace is ALWAYS bound at — never caller-selectable (matching
/// [`WorkspaceStorageMode::Disabled`]'s own doc: "`/workspace` is never mounted" when disabled).
const OCI_WORKSPACE_MOUNT: &str = "/workspace";
/// Fixed read-only destination for the dependency directories inside a verified Cargo vendor
/// asset. Slice #32 precreated this empty mountpoint in the digest-pinned Rust rootfs.
pub const OCI_CARGO_VENDOR_MOUNT: &str = "/opt/myelin/cargo-vendor";
/// Server-only selector carried by the structured Cargo launch translation. Its value is an exact
/// digest-pinned [`VerifiedCargoVendor`] registry reference, never a tenant path.
pub const ENV_CARGO_VENDOR_ASSET: &str = "MYELIN_CARGO_VENDOR_ASSET";
/// Cargo needs a writable home for its package-cache lock. This directory lives inside the job's
/// already size-bounded writable `/tmp` tmpfs; its `config.toml` is over-mounted read-only.
pub const STRUCTURED_CARGO_HOME: &str = "/tmp/cargo-home";
pub const CARGO_SOURCE_REPLACE_ENV: &str = "CARGO_SOURCE_CRATES_IO_REPLACE_WITH";
pub const CARGO_VENDOR_DIRECTORY_ENV: &str = "CARGO_SOURCE_VENDORED_DIRECTORY";
/// Cargo CLI `KEY=VALUE` TOML snippets installed only by the platform-owned structured argv.
/// Command-line config has Cargo's highest precedence, above environment and every discovered
/// `.cargo/config{,.toml}` file.
pub const CARGO_SOURCE_REPLACE_CONFIG: &str = "source.crates-io.replace-with=\"vendored\"";
pub const CARGO_VENDOR_DIRECTORY_CONFIG: &str =
    "source.vendored.directory=\"/opt/myelin/cargo-vendor\"";
const OCI_CARGO_CONFIG_MOUNT: &str = "/tmp/cargo-home/config.toml";
const TEST_SERVER_CARGO_CONFIG_SOURCE: &str = "/server-owned/cargo/config.toml";
const TEST_CARGO_VENDOR_MOUNT_SOURCE: &str =
    "/var/lib/myelin/gvisor-assets/cargo-vendor-smoke-v1/vendor";
pub(super) const CARGO_VENDOR_SOURCE_NAME: &str = "vendored";

/// Canonical server policy artifact mounted read-only at `$CARGO_HOME/config.toml`. This and the
/// corresponding environment variables are defense in depth; the platform-owned Cargo `--config`
/// argv is authoritative because Cargo gives CLI config precedence over environment and every
/// discovered config file (including workspace config and legacy `$CARGO_HOME/config`).
pub const SERVER_CARGO_CONFIG_TOML: &str = "[source.crates-io]\nreplace-with = \"vendored\"\n\n[source.vendored]\ndirectory = \"/opt/myelin/cargo-vendor\"\n";

/// The exact lowered argvs the platform-owned structured Cargo grammar produces
/// (`myelin_ci_controlplane::run_plan`'s `StructuredBuildV1::platform_argv` over its
/// `CARGO_RECIPE_ALLOWLIST`): `build`, unit `test --lib` (and `--workspace`), and `clippy`. The
/// sandbox RE-VALIDATES `spec.command` against this closed set as defense in depth — it does not
/// trust the control-plane's lowering; a job whose argv is not one of these cannot select a Cargo
/// vendor asset. Kept in lockstep with the grammar by a cross-crate sync test in
/// myelin-ci-controlplane. The vendor `--config` pairs land BEFORE clippy's `--` separator so the
/// `-D warnings` driver flags still reach clippy.
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

/// A path proven absolute AT CONSTRUCTION — every OCI `root.path` override this crate ever uses is
/// meant to be absolute (a symlinked/relative `root.path` COMBINED with a host bind mount makes the
/// rootless `runsc` gofer fail to bring up the sandbox), so this wrapper makes "claims absolute but
/// isn't" unrepresentable rather than trusting every call site to remember to check.
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

/// A serialization-only projection of a [`crate::workspace_manager::ManagedWorkspace`] for embedding
/// into [`OciConfig`] — deliberately NEVER the real `ManagedWorkspace` itself, which is non-`Clone`,
/// owns real leased capacity, and poisons its manager on an unconsumed drop; `OciConfig` is `Clone`
/// and may be held/inspected well past the workspace's own real lifecycle. The destination, mount
/// mode, and mount options are all fixed (never caller-selectable) — only the host source varies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OciWorkspaceMount {
    host_source: PathBuf,
}

impl OciWorkspaceMount {
    /// Piece 7 constructs this from a REAL `ManagedWorkspace` it keeps alive separately for the
    /// duration of the launch; this projection carries only what `to_json` needs to render the bind
    /// mount, never ownership of the workspace itself.
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

/// The git-wire host mounts, in a FIXED shape — deliberately no free-form `guest_dest`/`readonly`
/// fields a caller could set to smuggle e.g. a writable `/workspace`-shaped bind mount into a
/// ROOTLESS layout, contradicting [`OciExecutionLayout`]'s whole invariant that a workspace
/// requires explicit user-namespace support (and risking a destination collision with a reserved
/// mount like `/tmp`). Destinations are fixed at [`WIRE_REPO_MOUNT`] (always read-only) and
/// [`WIRE_QUARANTINE_MOUNT`] (always writable, only present when actually requested) — never
/// caller-selectable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GitWireMounts {
    repo_source: PathBuf,
    quarantine_source: Option<PathBuf>,
}

/// The structured Cargo build's server-owned dependency boundary. The verified asset is selected
/// before workspace acquisition; the independently host-computed materialized lock digest is bound
/// only after Hop B succeeds and before the workload can reach its launch permit/spawn path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CargoVendorBoundary {
    asset: crate::asset_registry::VerifiedCargoVendor,
    materialized_cargo_lock_sha256: Option<String>,
}

/// A per-launch capability for the exact vendor-tree inode verified immediately before bundle
/// staging. The held descriptor content-verifies the tree TOCTOU-safely (through the fd, not the
/// pathname); the OCI mount source is the verified REAL canonical path, because the gVisor gofer
/// opens the mount source from outside the runner's mount namespace and cannot resolve a
/// `/proc/<pid>/fd/N` source (it fails to `setns`, `join container mntns: operation not permitted`).
/// The pinned rootfs — an equally security-critical, content-addressed asset-store tree — is mounted
/// by real path for the same reason; this makes the vendor mount consistent with it.
///
/// Trust boundary (NOT same-uid immunity): renaming/replacing the registry pathname between the
/// final identity check and the gofer's open CAN redirect the mount — but only for a host actor
/// running as the trusted asset-store owner uid (or root). The sandbox tenant/subuid and any
/// different-uid co-tenant CANNOT: the asset store is platform-owned, not tenant-writable, and
/// registry verification enforces owner + no-group/world-write on the asset root and descendants.
/// So this is the same trusted-service-account / host-compromise class already scoped for the CoW
/// and checkout fd-binding work (the real remedy there is genuinely immutable storage — EROFS /
/// fs-verity / a root-owned publication boundary — not an fd-pinned mount), NOT a tenant or
/// cross-tenant escape. The asset PATH (incl. any `MYELIN_GVISOR_CARGO_VENDOR` override) is trusted
/// deployment configuration, exactly like the rootfs and runsc-binary paths.
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

/// The execution layout an [`OciConfig`] renders — a SINGLE enum, never independent `Option` fields
/// for rootfs-path override / user-namespace / workspace-mount: exactly the 4 combinations this
/// crate actually produces, each with its own fixed, internally-consistent shape. Rootfs-path
/// resolution is conceptually orthogonal to user-namespace mode, but NOT orthogonal to host-mount
/// behavior — a host bind mount (git-wire's repo/quarantine mounts, or a workspace mount) always
/// requires an absolute `root.path` override in this codebase's own empirically-established
/// gofer/rootless behavior, so those two are always paired here, never independent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum OciExecutionLayout {
    /// Ordinary CI/agent job launch — the ONLY production behavior before CT-007 slice 2/3 added
    /// the other variants. Bundle-relative `rootfs`, no host mounts, no user namespace.
    Rootless,
    /// The git-wire path: an absolute rootfs override (required alongside its host bind mounts),
    /// still fully rootless (no user namespace, no workspace).
    RootlessWithHostMounts {
        absolute_rootfs: AbsoluteRootfs,
        mounts: GitWireMounts,
    },
    /// CT-007 slice 2: an explicit user-namespace mapping, no workspace mount — bundle-relative
    /// `rootfs` (no host mounts are involved, so no absolute-root requirement applies).
    ExplicitUserNamespace { config: UserNamespaceConfig },
    /// CT-007 slice 3: an explicit user-namespace mapping WITH a disk-backed workspace mount.
    /// Workspace-mount support REQUIRES explicit user-namespace support (the guest's unprivileged
    /// uid, 65534, is unmapped under plain `--rootless`) — this variant is the only place a
    /// workspace can appear, never combined with `Rootless`. The workspace mount is itself a host
    /// bind mount, so (matching `RootlessWithHostMounts`) an absolute rootfs override is required
    /// here too.
    ExplicitUserNamespaceWithWorkspace {
        config: UserNamespaceConfig,
        workspace: OciWorkspaceMount,
        absolute_rootfs: AbsoluteRootfs,
        cargo_vendor: Option<CargoVendorBoundary>,
    },
}

/// The OCI runtime config (`config.json`) the gVisor `runsc` path consumes, built from a [`JobSpec`]
/// and the mandatory [`HardeningProfile`]. Every hardening field maps to a real OCI posture: the
/// root is `readonly: true`, all capabilities are dropped, `no_new_privileges: true`, a seccomp
/// profile is attached, the network namespace carries no interface when egress is default-deny, and
/// the untrusted process runs as a NON-ROOT uid/gid ([`UNTRUSTED_UID`]/[`UNTRUSTED_GID`]). This is a
/// RUNNABLE OCI config (`process.cwd` + `process.env` are set) — `runsc run --bundle` executes it.
#[derive(Clone, PartialEq, Eq)]
pub struct OciConfig {
    pub(super) args: Vec<String>,
    pub(super) root_readonly: bool,
    pub(super) drop_all_caps: bool,
    pub(super) no_new_privileges: bool,
    pub(super) seccomp: bool,
    pub(super) has_network: bool,
    /// Emitted twice deliberately: OCI `linux.resources.pids.limit` for cgroup-capable runtimes and
    /// `RLIMIT_NPROC` for rootless `runsc`, which cannot install the host pids cgroup itself.
    pub(super) pids_max: u32,
    /// The memory ceiling (bytes) — emitted as `linux.resources.memory.limit`. IMPORTANT (CT-003b /
    /// SI-017): `runsc --rootless` does NOT enforce this OCI field (rootless runsc cannot manage a
    /// host cgroup), so this value is ADVISORY here (it would be honored by a non-rootless `runsc`).
    /// The REAL host-RAM bound for the gVisor workload is the OUT-OF-BAND [`MemoryCgroup`] the
    /// production run path places the `runsc` process tree into — that is what OOM-kills a memory hog
    /// within the limit and keeps it from consuming host RAM beyond `mem_bytes`.
    pub(super) mem_bytes: u64,
    /// The aggregate RAM-backed writable-tmpfs ceiling (bytes) (CT-003a). Ordinarily all of it is
    /// assigned to `/tmp`; a structured Cargo launch partitions it between `/tmp` and the nested
    /// owned Cargo-home tmpfs. gVisor would otherwise auto-mount an UNBOUNDED host-RAM-backed tmpfs
    /// at `/tmp`; sizing it caps a disk fill at ENOSPC (the SI-017 host-DoS escape D2 surfaced
    /// through the production `launch()`). Sourced from
    /// [`ResourceLimits::tmpfs_bytes`](crate::ResourceLimits::tmpfs_bytes), NOT
    /// `disk_bytes` (that field is the disk-backed ephemeral-workspace quota — unrelated to this
    /// RAM-backed tmpfs).
    pub(super) tmpfs_bytes: u64,
    /// CT-006a / CI-1: EXTRA `process.env` entries (`"KEY=VALUE"`) appended after the base `PATH`.
    /// Ordinary jobs carry literal env plus broker-resolved secret env; the git-wire path sets
    /// `GIT_PROTOCOL=version=2` / `GIT_EXEC_PATH` so sandboxed canonical `git` finds its helpers.
    pub(super) extra_env: Vec<String>,
    /// CT-007 slice 3, piece 6: the ONE source of truth for rootfs-path resolution, host mounts,
    /// user-namespace mode, and workspace mounting — replacing what were three independent fields
    /// (`root_path`/`user_namespace`/`extra_mounts`) whose combinations could silently drift out of
    /// sync with each other. [`Self::invocation_mode`] and [`Self::to_json`] both derive everything
    /// from this ONE field, so neither can ever disagree with what the other implies.
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

/// Render one OCI `mounts` bind-mount entry. Source/dest are JSON-escaped via `{:?}` so a path can
/// carry no JSON-injection.
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
    /// CT-007 slice 5b.3-6e.1b (`test-support`): the host source path of this config's workspace
    /// bind mount, if it has one. The deterministic checkout-capsule execution seam reads the
    /// substituted Hop B sentinel through THIS path — the OCI-config-recorded mount source — and
    /// asserts it equals the capsule's own workspace host path, proving Hop B and the workload
    /// shared the one provenance workspace. `None` for any layout without a workspace mount.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn workspace_host_source_for_tests(&self) -> Option<&Path> {
        match &self.layout {
            OciExecutionLayout::ExplicitUserNamespaceWithWorkspace { workspace, .. } => {
                Some(&workspace.host_source)
            }
            _ => None,
        }
    }

    /// Build the OCI config from a job + its derived hardening profile (the same profile the
    /// Firecracker backend enforces — backend-independent).
    pub fn from_spec(spec: &JobSpec, profile: &HardeningProfile) -> OciConfig {
        // CT-007 gate 2 + CI-1: literals and broker-resolved secrets enter only OCI `process.env`.
        // `ResolvedJobSecrets` is the one inseparable value that also owns the redaction plan; its
        // private fields prevent an env-only injection. The base PATH is emitted first.
        let mut extra_env: Vec<String> = spec
            .env
            .iter()
            .map(|e| format!("{}={}", e.name, e.value))
            .collect();
        extra_env.extend(spec.resolved_secrets().process_env());
        Self::for_fixed_command(spec.command.clone(), spec.limits.mem_bytes, profile)
            .with_extra_env(extra_env)
    }

    /// CT-007 slice 5b.2: the shared constructor beneath [`Self::from_spec`] for a caller with a
    /// fixed guest command + real [`HardeningProfile`] but no real `JobSpec` to derive one from
    /// (mirrors [`HardeningProfile::for_execution`]'s reasoning exactly) — the checkout-preparation
    /// runtime's guest command is a fixed script, never a billed job's `spec.command`.
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
            // The hardening profile's scratch-tmpfs quota (= `spec.limits.tmpfs_bytes`).
            tmpfs_bytes: profile.scratch_quota_bytes,
            extra_env: Vec::new(),
            // Rootless by default — every other layout is an explicit opt-in via a builder below;
            // this does not change any existing caller's default behavior.
            layout: OciExecutionLayout::Rootless,
        }
    }

    /// Every layout-selecting builder below requires this — layout selection is ONE-SHOT (Sol's
    /// round-1 review of piece 6): without this guard, chaining two layout builders (e.g.
    /// `.with_user_namespace(cfg).with_rootless_host_mounts(...)`) would silently discard whichever
    /// was selected first, even though the enum itself prevents an invalid FINAL combination —
    /// the transition API could still silently erase an already-selected security obligation.
    fn require_still_rootless(&self) -> Result<(), String> {
        if matches!(self.layout, OciExecutionLayout::Rootless) {
            Ok(())
        } else {
            Err(
                "an execution-layout selection was already made on this config — layout \
                 selection is one-shot and must never be silently overwritten"
                    .to_string(),
            )
        }
    }

    /// CT-006a (the git wire): attach an ABSOLUTE staged-rootfs override plus its host bind mounts
    /// (the RO repo + optional writable quarantine) atomically — these two were previously set via
    /// two independent builders (`with_root_path`/`with_extra_mounts`), which could leave one set
    /// without the other despite this codebase's own empirically-established requirement that a
    /// host bind mount always needs an absolute `root.path` alongside it (a symlinked/relative one
    /// COMBINED with a bind mount makes the rootless `runsc` gofer fail to bring up the sandbox —
    /// "cannot read client sync file"). Fails if `absolute_rootfs` is not actually absolute — the
    /// canonicalize-with-fallback pattern the git-wire call site used to use could otherwise
    /// silently retain a relative path when the configured rootfs was absent. Takes the repo/
    /// quarantine sources directly, never a free-form mount descriptor (see [`GitWireMounts`]'s
    /// doc for why). `pub(crate)` — specialized to the git-wire path; the caller (not this type)
    /// is responsible for the source paths already having passed [`resolve_bare_repo_path`]'s
    /// confinement check, which the real backend call path always performs before this is ever
    /// reached. Consuming builder.
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

    /// CT-007 slice 2: attach an explicit user-namespace mapping (no workspace mount) — the
    /// resulting `to_json` gains a `user` namespace entry plus the exact two-entry
    /// `uidMappings`/`gidMappings`, and [`Self::invocation_mode`] reports
    /// [`RunscInvocationMode::ExplicitUserNamespace`]. Consuming builder.
    pub fn with_user_namespace(mut self, config: UserNamespaceConfig) -> Result<OciConfig, String> {
        self.require_still_rootless()?;
        self.layout = OciExecutionLayout::ExplicitUserNamespace { config };
        Ok(self)
    }

    /// CT-007 slice 3, piece 6: attach an explicit user-namespace mapping WITH a disk-backed
    /// workspace mount — the resulting `to_json` gains the `user` namespace entry/mappings AND
    /// exactly one fixed writable bind mount at [`OCI_WORKSPACE_MOUNT`], with an absolute rootfs
    /// override (workspace mounting is itself a host bind mount, so the same absolute-root
    /// requirement `with_rootless_host_mounts` documents applies here too). `pub(crate)`, not yet
    /// consumed by any production launch path — that's piece 7, which supplies the real
    /// `ManagedWorkspace` this config's [`OciWorkspaceMount`] merely projects from. Consuming
    /// builder; fails if `absolute_rootfs` is not actually absolute.
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

    /// Attach a registry-verified Cargo vendor asset to the workspace layout. Both destinations are
    /// fixed by the platform. The rootfs mountpoint must already be a real empty directory in the
    /// digest-pinned rootfs; launch code never creates or mutates it after rootfs verification.
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

    /// Bind Hop B's independently host-computed materialized lock digest into the retained launch
    /// config. A non-structured/free-form job has no Cargo boundary and remains a no-op.
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

    /// Open the registered canonical tree with `O_PATH|O_NOFOLLOW`, bind it to the identity captured
    /// at registry verification, re-verify the content through that held descriptor, and return the
    /// exact verified REAL-PATH source the OCI runtime must mount (the gVisor gofer cannot open a
    /// `/proc/<pid>/fd/N` magic-symlink source — it fails to `setns` into the runner's mount
    /// namespace under the sandbox's empty capabilities). The descriptor remains owned by the staged
    /// bundle until after runtime teardown, pinning the verified inode.
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
                // SAFETY: `path_c` is a live NUL-terminated pathname. `O_NOFOLLOW` atomically
                // refuses a symlink at the verified canonical leaf, and the successful fd is
                // transferred immediately into one `OwnedFd` below.
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
                // SAFETY: `raw_fd` was returned by the successful `open` above and has no other
                // owner. `O_CLOEXEC` prevents accidental inheritance; runsc opens the explicit
                // parent-process fd path while this descriptor remains held by the bundle.
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

                // Mount the VERIFIED REAL PATH, not the `/proc/<pid>/fd/N` magic symlink. The
                // gVisor gofer opens the mount source from OUTSIDE the runner's mount namespace and
                // cannot `setns` into it under the sandbox's empty capabilities — a `/proc/pid/fd`
                // source fails with `join container mntns: operation not permitted`, exactly as the
                // pinned rootfs bind (also a verified, content-addressed asset-store tree) is mounted
                // by its real path. Verification stays fd-bound (content-checked through the held
                // descriptor above); the real path was just re-confirmed to resolve to that exact
                // inode, and `_root_fd` keeps the inode pinned through teardown.
                //
                // Honest residual scope (NOT same-uid immunity — the held fd does NOT snapshot
                // directory contents, and swapping the `vendor` child does not change the root's
                // (dev,ino)): between the final identity check and the gofer's open, a host actor
                // running as the trusted asset-store owner uid (or root) COULD rename/replace the
                // path and redirect the mount. The sandbox tenant/subuid and any different-uid
                // co-tenant CANNOT — the store is platform-owned, not tenant-writable, and registry
                // verification enforces owner + no-group/world-write. This is the trusted-service /
                // host-compromise class (contrast the tenant-writable checkout tree, [[#27]]); its
                // real remedy is immutable storage (EROFS / fs-verity / root-owned publication), out
                // of scope here. The asset path is trusted deployment configuration, like the rootfs.
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

    /// The [`RunscInvocationMode`] this config implies — the ONE place that decision is made,
    /// derived structurally from [`Self::layout`] so it can never disagree with what
    /// [`Self::to_json`] actually serializes.
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

    /// CT-006a: append extra `process.env` entries (`"KEY=VALUE"`) after the base `PATH`. Consuming
    /// builder; used to set `GIT_PROTOCOL=version=2` / `GIT_EXEC_PATH` for the sandboxed `git`.
    pub fn with_extra_env(mut self, env: Vec<String>) -> OciConfig {
        self.extra_env = env;
        self
    }

    /// Serialize to a minimal OCI `config.json` (`runsc run --bundle <dir>` consumes it). The
    /// posture flags reflect the real enforced state, so a test over this JSON asserts the posture.
    pub fn to_json(&self) -> Result<String, String> {
        self.to_json_zeroizing().map(|json| json.to_string())
    }

    /// Production serializer for config files that may contain injected secret environment values.
    /// Every secret-bearing buffer created here is zeroized when it leaves scope.
    pub(super) fn to_json_zeroizing(&self) -> Result<zeroize::Zeroizing<String>, String> {
        let cargo_config_source = self
            .has_cargo_vendor()
            .then(|| Path::new(TEST_SERVER_CARGO_CONFIG_SOURCE));
        let cargo_vendor_source = self
            .has_cargo_vendor()
            .then(|| Path::new(TEST_CARGO_VENDOR_MOUNT_SOURCE));
        self.to_json_zeroizing_with_cargo_sources(cargo_config_source, cargo_vendor_source)
    }

    /// Production bundle staging supplies the just-created server-owned config path here. Keeping
    /// that host path out of [`OciConfig`] avoids turning a mutable external pathname into part of
    /// the long-lived verified asset capability.
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
            // No network namespace interface — egress closed at the namespace level.
            "{ \"type\": \"network\", \"path\": \"\" }"
        };
        // `process.env`: the base PATH first, then any extra entries (e.g. GIT_PROTOCOL) — JSON-quoted.
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
        // `mounts`: the size-bounded writable `/tmp` tmpfs first, then whatever
        // host mounts this config's layout implies (CT-006a's git-wire repo/quarantine binds, or
        // CT-007 slice 3's fixed workspace bind) — `OciExecutionLayout` only ever produces one
        // shape or the other, never both. Source/dest are JSON-escaped via `{:?}` so a path can
        // carry no JSON-injection.
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
                // Always exactly one fixed writable mount — never caller-selectable readonly/dest.
                mounts.push(bind_mount_json(
                    OCI_WORKSPACE_MOUNT,
                    &workspace.host_source,
                    false,
                ));
                if cargo_vendor.is_some() {
                    // The nested config bind needs its parent mount to exist before process start,
                    // so Cargo home remains a distinct tmpfs for deterministic ownership. Its quota
                    // is SUBTRACTED from `/tmp` above: the two writable tmpfs mounts partition, and
                    // can never double, the job's one declared scratch-tmpfs bound.
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
                    // The verified asset root also carries its lock/config metadata. Only the actual
                    // `cargo vendor` directory is projected at Cargo's fixed directory-source path.
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
        // `root.path`: absolute for either host-mount-bearing layout (required alongside a bind
        // mount — see `AbsoluteRootfs`'s doc), else the bundle-relative `rootfs`.
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
        // Workspace-backed jobs execute from the checked-out tree. Keep every non-workspace
        // layout at `/`: merely selecting an explicit user namespace must not imply that
        // `/workspace` exists. Deriving cwd from the same closed layout enum that controls the
        // mount prevents the two from drifting apart.
        let process_cwd = match &self.layout {
            OciExecutionLayout::ExplicitUserNamespaceWithWorkspace { .. } => OCI_WORKSPACE_MOUNT,
            OciExecutionLayout::Rootless
            | OciExecutionLayout::RootlessWithHostMounts { .. }
            | OciExecutionLayout::ExplicitUserNamespace { .. } => "/",
        };
        // CT-007 slice 2/3: `Rootless`/`RootlessWithHostMounts` (the ONLY production behavior
        // before user namespaces existed) emit BYTE-IDENTICAL namespace/mapping JSON — no `user`
        // namespace, no `uidMappings`/`gidMappings`. Either explicit-userns layout adds a `user`
        // namespace entry alongside the always-present network one, plus the exact two-entry
        // uid/gid maps (container 0 -> this process's real identity, container 65534 -> the
        // leased subordinate host uid/gid).
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

    /// True iff the OCI root is read-only.
    pub fn root_readonly(&self) -> bool {
        self.root_readonly
    }
    /// True iff a network interface is present (false == egress closed at the namespace level).
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
        // The exact four lowered argvs the control-plane grammar produces are all admitted.
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
        // Anything outside the closed set is refused — a tenant cannot drop `--locked`, run a
        // non-allowlisted subcommand, reorder the vendor `--config` after clippy's `--`, or shell out.
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

    /// **Path A (real-path vendor mount) contract.** The gVisor gofer cannot open a `/proc/pid/fd`
    /// magic-symlink source (it fails to `setns` into the runner mntns: `join container mntns:
    /// operation not permitted`), so the OCI mount source is the VERIFIED CANONICAL REAL PATH — like
    /// the pinned rootfs. This is deliberately NOT swap-immune against the trusted asset-store owner
    /// uid (that host-compromise class needs immutable storage, out of scope). The guarantees it DOES
    /// keep, asserted here: (1) the mount source is a real path (never a `/proc/fd` symlink the gofer
    /// rejects) resolving to the verified vendored crate, (2) the OCI config consumes exactly that
    /// path, and (3) a persistent pathname replacement makes the NEXT launch fail closed on the
    /// re-open identity check.
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

        // (1) A real path the gofer can open — NOT a `/proc/<pid>/fd/N` magic symlink — resolving to
        // the verified vendored crate.
        assert!(
            !source.starts_with("/proc/"),
            "vendor mount source must be a real path the gofer can open, not a /proc/fd symlink: \
             {source:?}"
        );
        assert_eq!(
            std::fs::read_to_string(source.join("itoa-1.0.15/lib.rs")).unwrap(),
            "pub fn fixture() {}",
        );
        // (2) The OCI config consumes exactly that real-path source.
        let staged_json = std::fs::read_to_string(staged.path.join("config.json")).unwrap();
        assert!(
            staged_json.contains(&format!("\"source\": {:?}", source.to_string_lossy())),
            "the OCI mount must consume the verified canonical real-path source: {staged_json}"
        );

        // (3) A persistent pathname replacement makes the NEXT launch fail closed: the re-open +
        // identity check refuses the replacement inode. (This does NOT claim same-launch swap
        // immunity — the mount source is now the real path — but the fd-bound re-verification still
        // catches a durable swap at the next stage.)
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
        // CT-002b: the untrusted process runs NON-ROOT (defense in depth — never uid 0 in the
        // sandbox) and the config is RUNNABLE (`cwd` set, else `runsc run` rejects the spec).
        assert!(
            json.contains("\"uid\": 65534") && json.contains("\"gid\": 65534"),
            "the untrusted process must run as a non-root uid/gid (65534)"
        );
        assert!(
            json.contains("\"cwd\": \"/\""),
            "process.cwd must be set or the OCI runtime rejects the spec"
        );
        // CT-003a/CT-003b (SI-017): the OCI emits an (advisory — rootless runsc ignores it) memory
        // ceiling from spec.limits.mem_bytes; the REAL host-RAM bound is the out-of-band MemoryCgroup
        // the production run path places the runsc tree into (see MemoryCgroup). It also mounts a
        // SIZE-BOUNDED writable `/tmp` tmpfs (sized from the scratch quota) so a disk fill hits
        // ENOSPC instead of an unbounded host-RAM-backed tmpfs. spec()'s limits are mem=256 MiB,
        // tmpfs=1 GiB.
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
             — runsc --rootless installs its own, and a doubly-declared userns fails the gofer"
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

    /// CT-007 slice 3, piece 6 test matrix: the git-wire-shaped `RootlessWithHostMounts` layout —
    /// an absolute rootfs override alongside its host bind mounts, still fully rootless (no user
    /// namespace, no uid/gid mappings).
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

    /// CT-007 slice 3, piece 6 test matrix: the workspace-mount layout — absolute root, explicit
    /// user-namespace mappings, AND exactly one fixed writable workspace bind mount.
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
        // Never readonly, never a caller-selectable destination — only ONE workspace mount entry.
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

    /// Sol's round-1 review of piece 6: layout selection must be ONE-SHOT — chaining two
    /// layout-selecting builders must never silently discard whichever was selected first, even
    /// though the enum itself already prevents an invalid FINAL combination.
    #[test]
    fn oci_config_layout_selection_is_one_shot() {
        let config = UserNamespaceConfig::for_tests(1000, 1000, 100_005, 200_005);
        // userns first, THEN an attempt at rootless host mounts — must refuse, not silently revert.
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
        // host mounts first, THEN an attempt at a user namespace — must refuse, not silently
        // discard the mounts.
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

    /// Sol's round-1 review of piece 6: `RootlessWithHostMounts` must never accept a free-form
    /// mount descriptor with a caller-controlled destination/mode — that could otherwise smuggle a
    /// writable `/workspace`-shaped mount into a layout that reports `Rootless`. This test proves
    /// there is no way to reach that state: the builder's signature only ever accepts fixed
    /// repo/quarantine host-source paths, never an arbitrary destination or mode.
    #[test]
    fn oci_config_rootless_with_host_mounts_never_accepts_an_arbitrary_destination_or_mode() {
        // The only two mounts `RootlessWithHostMounts` can ever produce are the fixed
        // WIRE_REPO_MOUNT (always ro) and WIRE_QUARANTINE_MOUNT (always rw, only if requested) —
        // proven by construction (the builder takes no `guest_dest`/`readonly` parameters at all),
        // and confirmed here by asserting the JSON never contains anything else.
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

    /// CT-007 gate 2: a job's declared (non-secret) environment must reach `process.env`. Before
    /// this, `from_spec` dropped `spec.env` entirely, so a real build job's `CARGO_*` / PATH
    /// extensions never took effect in the sandbox. Secret values are NOT carried in `spec.env`
    /// (resolved in-boundary from `SecretRef`), so a plain `NAME=VALUE` render is correct.
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
        // The base PATH is still emitted (declared env is APPENDED after it, not replaced).
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

    /// Empty host-mount collections are still valid (git-wire's OWN quarantine mount is genuinely
    /// optional — a read-only serve with no push in flight has none) — the repo mount alone is
    /// never itself optional, so "empty" here means "repo only," never "no mounts at all," which
    /// this test confirms is exactly what a `None` quarantine source produces.
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

    /// CT-007 slice 2: `with_user_namespace` must produce the EXACT two-entry OCI mapping the
    /// design specifies, alongside a declared `user` namespace — and `invocation_mode()` must
    /// report `ExplicitUserNamespace` carrying the SAME config back out.
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
        // Every OTHER hardening assertion from the Rootless test must still hold — attaching a
        // user namespace changes ONLY the namespaces/mappings, nothing else.
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
