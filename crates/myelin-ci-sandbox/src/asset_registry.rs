use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::canonical_tar;
use crate::ImageRef;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug)]
pub struct RootfsAssetBinding {
    pub image: ImageRef,
    pub rootfs: PathBuf,
}

#[derive(Clone, Debug)]
pub struct CargoVendorAssetBinding {
    pub reference: ImageRef,
    pub root: PathBuf,
    pub cargo_lock_sha256: String,
}

pub const CARGO_VENDOR_SMOKE_TREE_SHA256: &str =
    "9fcc19c65ae0a47de4b241c30f9eb3613cd74adf315f763e6f91d74b31eec8eb";
pub const CARGO_VENDOR_SMOKE_LOCK_SHA256: &str =
    "fc5b44e66527fdda3cbef94d7ee128f77f0919dc176e0ae8198a717b8ca7c603";
pub const ENV_GVISOR_CARGO_VENDOR: &str = "MYELIN_GVISOR_CARGO_VENDOR";

pub const CARGO_VENDOR_WORKSPACE_TREE_SHA256: &str =
    "c2f6229625dd25ac26f356a34764026eb61b7c227fbd60cc470ab333bdf89fd2";
pub const CARGO_VENDOR_WORKSPACE_LOCK_SHA256: &str =
    "9a503ad7a2d64a4eb7e6b98a4d3c7150d446fdf4bc73527abb0ff6b998e3d251";
pub const ENV_GVISOR_CARGO_VENDOR_WORKSPACE: &str = "MYELIN_GVISOR_CARGO_VENDOR_WORKSPACE";

pub fn cargo_vendor_smoke_reference() -> String {
    format!("myelin.local/cargo-vendor-smoke-v1@sha256:{CARGO_VENDOR_SMOKE_TREE_SHA256}")
}

pub fn cargo_vendor_workspace_reference() -> String {
    format!("myelin.local/cargo-vendor-workspace-v1@sha256:{CARGO_VENDOR_WORKSPACE_TREE_SHA256}")
}

pub fn cargo_lock_sha256_hex(cargo_lock_bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(cargo_lock_bytes))
}

pub fn select_registered_cargo_vendor(cargo_lock_sha256: &str) -> Option<String> {
    match cargo_lock_sha256 {
        CARGO_VENDOR_SMOKE_LOCK_SHA256 => Some(cargo_vendor_smoke_reference()),
        CARGO_VENDOR_WORKSPACE_LOCK_SHA256 => Some(cargo_vendor_workspace_reference()),
        _ => None,
    }
}

pub fn file_sha256_hex(path: &Path) -> std::io::Result<String> {
    std::fs::read(path).map(|bytes| format!("{:x}", Sha256::digest(bytes)))
}

pub fn resolved_gvisor_cargo_vendor() -> PathBuf {
    if let Ok(path) = std::env::var(ENV_GVISOR_CARGO_VENDOR) {
        return PathBuf::from(path);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("gvisor-assets")
        .join("cargo-vendor-smoke-v1")
}

pub fn resolved_gvisor_cargo_vendor_workspace() -> PathBuf {
    if let Ok(path) = std::env::var(ENV_GVISOR_CARGO_VENDOR_WORKSPACE) {
        return PathBuf::from(path);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("gvisor-assets")
        .join("cargo-vendor-workspace-v1")
}

#[derive(Debug)]
pub struct GvisorAssetRegistry {
    verified: HashMap<String, VerifiedRootfs>,
    cargo_vendor: HashMap<String, VerifiedCargoVendor>,
}

#[derive(Clone, Debug)]
pub struct VerifiedRootfs {
    path: PathBuf,
    digest_hex: String,
    device: u64,
    inode: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedCargoVendor {
    path: PathBuf,
    digest_hex: String,
    cargo_lock_sha256: String,
    expected_owner_uid: u32,
    device: u64,
    inode: u64,
}

impl VerifiedCargoVendor {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn digest_hex(&self) -> &str {
        &self.digest_hex
    }

    pub fn cargo_lock_sha256(&self) -> &str {
        &self.cargo_lock_sha256
    }

    pub(crate) fn identity(&self) -> (u64, u64) {
        (self.device, self.inode)
    }

    pub(crate) fn verify_before_spawn_at(
        &self,
        fd_bound_root: &Path,
        materialized_cargo_lock_sha256: &str,
    ) -> Result<(), String> {
        if materialized_cargo_lock_sha256 != self.cargo_lock_sha256 {
            return Err(format!(
                "cargo vendor asset lock mismatch: checked-out Cargo.lock is sha256:{materialized_cargo_lock_sha256}, but the selected asset is keyed to sha256:{}",
                self.cargo_lock_sha256
            ));
        }

        let actual_lock_sha256 =
            file_sha256_hex(&fd_bound_root.join("Cargo.lock")).map_err(|error| {
                format!(
                    "could not re-read fd-bound Cargo vendor asset lock {} before spawn: {error}",
                    self.path.join("Cargo.lock").display()
                )
            })?;
        if actual_lock_sha256 != self.cargo_lock_sha256 {
            return Err(format!(
                "Cargo vendor asset {} lock drifted before spawn: expected sha256:{}, computed sha256:{actual_lock_sha256}",
                self.path.display(),
                self.cargo_lock_sha256
            ));
        }

        let actual = GvisorAssetRegistry::verify_asset_tree(
            fd_bound_root,
            self.expected_owner_uid,
        )
        .map_err(|error| {
            format!(
                "could not re-verify fd-bound Cargo vendor asset {} before spawn: {error}",
                self.path.display()
            )
        })?;
        if actual != self.digest_hex {
            return Err(format!(
                "Cargo vendor asset {} drifted before spawn: expected canonical-tree sha256:{}, computed sha256:{actual}",
                self.path.display(),
                self.digest_hex
            ));
        }
        Ok(())
    }
}

impl VerifiedRootfs {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn digest_hex(&self) -> &str {
        &self.digest_hex
    }

    pub fn identity(&self) -> (u64, u64) {
        (self.device, self.inode)
    }
}

#[derive(Debug)]
pub enum AssetRegistryError {
    UnknownImage { reference: String },
    DuplicateReference { reference: String },
    UnsupportedDigestAlgorithm {
        reference: String,
        algorithm: String,
    },
    InvalidRootfsPath { rootfs: PathBuf, reason: String },
    InvalidWorkspaceMountpoint { rootfs: PathBuf, reason: String },
    DigestMismatch {
        reference: String,
        expected: String,
        actual: String,
    },
    Hashing { rootfs: PathBuf, reason: String },
    GroupOrWorldWritable {
        rootfs: PathBuf,
        path: PathBuf,
        mode: u32,
    },
    UnexpectedOwner {
        rootfs: PathBuf,
        path: PathBuf,
        expected_uid: u32,
        actual_uid: u32,
    },
    InvalidCargoVendor { root: PathBuf, reason: String },
}

impl std::fmt::Display for AssetRegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssetRegistryError::UnknownImage { reference } => write!(
                f,
                "gvisor asset registry: no runner asset is registered for image `{reference}` - an \
                 unregistered image can never launch (the env var is not a fallback authority)"
            ),
            AssetRegistryError::DuplicateReference { reference } => write!(
                f,
                "gvisor asset registry: image `{reference}` was registered more than once - a \
                 second binding under the same exact reference is a composition-time \
                 misconfiguration, refused rather than silently overwriting the first"
            ),
            AssetRegistryError::UnsupportedDigestAlgorithm { reference, algorithm } => write!(
                f,
                "gvisor asset registry: image `{reference}` is pinned with an unsupported digest \
                 algorithm `{algorithm}` - only `sha256` has a matching canonical-tree hasher today"
            ),
            AssetRegistryError::InvalidRootfsPath { rootfs, reason } => write!(
                f,
                "gvisor asset registry: registered rootfs {} does not resolve to a valid directory: \
                 {reason}",
                rootfs.display()
            ),
            AssetRegistryError::InvalidWorkspaceMountpoint { rootfs, reason } => write!(
                f,
                "gvisor asset registry: registered rootfs {} has an invalid pinned /workspace \
                 mountpoint: {reason}",
                rootfs.display()
            ),
            AssetRegistryError::DigestMismatch {
                reference,
                expected,
                actual,
            } => write!(
                f,
                "gvisor asset registry: registered rootfs for image `{reference}` has DRIFTED - \
                 expected canonical-tree sha256:{expected}, computed sha256:{actual}. Refusing to \
                 launch against content that does not match the image's own pin."
            ),
            AssetRegistryError::Hashing { rootfs, reason } => write!(
                f,
                "gvisor asset registry: failed to hash registered rootfs {}: {reason}",
                rootfs.display()
            ),
            AssetRegistryError::GroupOrWorldWritable {
                rootfs,
                path,
                mode,
            } => write!(
                f,
                "gvisor asset registry: registered rootfs {} contains entry {} with unsafe mode \
                 {mode:04o}: group/world-writable bits 0022 must be clear",
                rootfs.display(),
                path.display()
            ),
            AssetRegistryError::UnexpectedOwner {
                rootfs,
                path,
                expected_uid,
                actual_uid,
            } => write!(
                f,
                "gvisor asset registry: registered rootfs {} contains entry {} owned by uid \
                 {actual_uid}, expected runner/asset-store owner uid {expected_uid}",
                rootfs.display(),
                path.display()
            ),
            AssetRegistryError::InvalidCargoVendor { root, reason } => write!(
                f,
                "gvisor asset registry: Cargo vendor asset {} is invalid: {reason}",
                root.display()
            ),
        }
    }
}

impl std::error::Error for AssetRegistryError {}

impl GvisorAssetRegistry {
    pub fn from_bindings(
        bindings: Vec<RootfsAssetBinding>,
    ) -> Result<GvisorAssetRegistry, AssetRegistryError> {
        Self::from_bindings_with_cargo_vendor(bindings, Vec::new())
    }

    pub fn from_bindings_with_cargo_vendor(
        bindings: Vec<RootfsAssetBinding>,
        cargo_vendor_bindings: Vec<CargoVendorAssetBinding>,
    ) -> Result<GvisorAssetRegistry, AssetRegistryError> {
        let expected_owner_uid = unsafe { libc::geteuid() };
        Self::from_bindings_with_owner(bindings, cargo_vendor_bindings, expected_owner_uid)
    }

    fn from_bindings_with_owner(
        bindings: Vec<RootfsAssetBinding>,
        cargo_vendor_bindings: Vec<CargoVendorAssetBinding>,
        expected_owner_uid: u32,
    ) -> Result<GvisorAssetRegistry, AssetRegistryError> {
        let mut verified = HashMap::with_capacity(bindings.len());
        for binding in bindings {
            if verified.contains_key(&binding.image.reference) {
                return Err(AssetRegistryError::DuplicateReference {
                    reference: binding.image.reference,
                });
            }
            let result = Self::verify_binding(&binding, expected_owner_uid)?;
            verified.insert(binding.image.reference.clone(), result);
        }
        let mut cargo_vendor = HashMap::with_capacity(cargo_vendor_bindings.len());
        for binding in cargo_vendor_bindings {
            if verified.contains_key(&binding.reference.reference)
                || cargo_vendor.contains_key(&binding.reference.reference)
            {
                return Err(AssetRegistryError::DuplicateReference {
                    reference: binding.reference.reference,
                });
            }
            let result = Self::verify_cargo_vendor_binding(&binding, expected_owner_uid)?;
            cargo_vendor.insert(binding.reference.reference.clone(), result);
        }
        Ok(GvisorAssetRegistry {
            verified,
            cargo_vendor,
        })
    }

    fn verify_cargo_vendor_binding(
        binding: &CargoVendorAssetBinding,
        expected_owner_uid: u32,
    ) -> Result<VerifiedCargoVendor, AssetRegistryError> {
        let (algorithm, expected_hex) = binding.reference.parse_digest().ok_or_else(|| {
            AssetRegistryError::InvalidCargoVendor {
                root: binding.root.clone(),
                reason: "reference must carry a supported digest pin".to_string(),
            }
        })?;
        if algorithm != "sha256" {
            return Err(AssetRegistryError::UnsupportedDigestAlgorithm {
                reference: binding.reference.reference.clone(),
                algorithm: algorithm.to_string(),
            });
        }
        if binding.cargo_lock_sha256.len() != 64
            || !binding
                .cargo_lock_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(AssetRegistryError::InvalidCargoVendor {
                root: binding.root.clone(),
                reason: "cargo_lock_sha256 must be exactly 64 hexadecimal characters".to_string(),
            });
        }

        let canon = std::fs::canonicalize(&binding.root).map_err(|error| {
            AssetRegistryError::InvalidCargoVendor {
                root: binding.root.clone(),
                reason: error.to_string(),
            }
        })?;
        if canon == Path::new("/") || !canon.is_dir() {
            return Err(AssetRegistryError::InvalidCargoVendor {
                root: canon,
                reason: "must canonicalize to a real, non-root directory".to_string(),
            });
        }

        for relative in ["vendor", ".cargo"] {
            let path = canon.join(relative);
            let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
                AssetRegistryError::InvalidCargoVendor {
                    root: canon.clone(),
                    reason: format!("required {relative}/ directory is absent: {error}"),
                }
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(AssetRegistryError::InvalidCargoVendor {
                    root: canon.clone(),
                    reason: format!("required {relative}/ must be a real directory"),
                });
            }
        }
        for relative in [".cargo/config.toml", "Cargo.lock"] {
            let path = canon.join(relative);
            let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
                AssetRegistryError::InvalidCargoVendor {
                    root: canon.clone(),
                    reason: format!("required {relative} is absent: {error}"),
                }
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(AssetRegistryError::InvalidCargoVendor {
                    root: canon.clone(),
                    reason: format!("required {relative} must be a real regular file"),
                });
            }
        }

        let actual_lock_sha256 = file_sha256_hex(&canon.join("Cargo.lock")).map_err(|error| {
            AssetRegistryError::InvalidCargoVendor {
                root: canon.clone(),
                reason: format!("could not read required Cargo.lock: {error}"),
            }
        })?;
        if actual_lock_sha256 != binding.cargo_lock_sha256 {
            return Err(AssetRegistryError::InvalidCargoVendor {
                root: canon.clone(),
                reason: format!(
                    "embedded Cargo.lock is sha256:{actual_lock_sha256}, expected sha256:{}",
                    binding.cargo_lock_sha256
                ),
            });
        }

        let actual_hex = Self::verify_asset_tree(&canon, expected_owner_uid)?;
        if actual_hex != expected_hex {
            return Err(AssetRegistryError::DigestMismatch {
                reference: binding.reference.reference.clone(),
                expected: expected_hex.to_string(),
                actual: actual_hex,
            });
        }

        Self::verify_cargo_vendor_world_readable(&canon)?;
        let metadata = std::fs::symlink_metadata(&canon).map_err(|error| {
            AssetRegistryError::InvalidCargoVendor {
                root: canon.clone(),
                reason: format!("could not capture verified root identity: {error}"),
            }
        })?;
        use std::os::unix::fs::MetadataExt as _;
        Ok(VerifiedCargoVendor {
            path: canon,
            digest_hex: actual_hex,
            cargo_lock_sha256: binding.cargo_lock_sha256.clone(),
            expected_owner_uid,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    fn verify_cargo_vendor_world_readable(root: &Path) -> Result<(), AssetRegistryError> {
        use std::os::unix::fs::MetadataExt as _;
        let invalid = |reason: String| AssetRegistryError::InvalidCargoVendor {
            root: root.to_path_buf(),
            reason,
        };
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let entries = std::fs::read_dir(&dir)
                .map_err(|error| invalid(format!("could not read {}: {error}", dir.display())))?;
            for entry in entries {
                let entry = entry.map_err(|error| {
                    invalid(format!("could not read an entry under {}: {error}", dir.display()))
                })?;
                let path = entry.path();
                let metadata = std::fs::symlink_metadata(&path)
                    .map_err(|error| invalid(format!("could not stat {}: {error}", path.display())))?;
                let file_type = metadata.file_type();
                if file_type.is_symlink() {
                    continue;
                }
                let mode = metadata.mode() & 0o7777;
                if file_type.is_dir() {
                    if mode & 0o005 != 0o005 {
                        return Err(invalid(format!(
                            "vendor directory {} has mode {mode:04o}; every directory must be \
                             world-traversable (o+rx) so the sandbox's non-owner build uid can read \
                             the read-only vendor mount",
                            path.display()
                        )));
                    }
                    stack.push(path);
                } else if file_type.is_file() && mode & 0o004 == 0 {
                    return Err(invalid(format!(
                        "vendor file {} has mode {mode:04o}; every file must be world-readable (o+r) \
                         so the sandbox's non-owner build uid can read the read-only vendor mount",
                        path.display()
                    )));
                }
            }
        }
        Ok(())
    }

    fn verify_asset_tree(
        canon: &Path,
        expected_owner_uid: u32,
    ) -> Result<String, AssetRegistryError> {
        canonical_tar::verified_asset_tree_sha256_hex(canon, expected_owner_uid).map_err(|error| {
            match error {
                canonical_tar::AssetTreeVerificationError::Io(error) => {
                    AssetRegistryError::Hashing {
                        rootfs: canon.to_path_buf(),
                        reason: error.to_string(),
                    }
                }
                canonical_tar::AssetTreeVerificationError::GroupOrWorldWritable { path, mode } => {
                    AssetRegistryError::GroupOrWorldWritable {
                        rootfs: canon.to_path_buf(),
                        path,
                        mode,
                    }
                }
                canonical_tar::AssetTreeVerificationError::UnexpectedOwner {
                    path,
                    expected_uid,
                    actual_uid,
                } => AssetRegistryError::UnexpectedOwner {
                    rootfs: canon.to_path_buf(),
                    path,
                    expected_uid,
                    actual_uid,
                },
            }
        })
    }

    fn verify_binding(
        binding: &RootfsAssetBinding,
        expected_owner_uid: u32,
    ) -> Result<VerifiedRootfs, AssetRegistryError> {
        let image = &binding.image;
        let (algorithm, expected_hex) =
            image
                .parse_digest()
                .ok_or_else(|| AssetRegistryError::UnsupportedDigestAlgorithm {
                    reference: image.reference.clone(),
                    algorithm: "<unparseable / not digest-pinned>".to_string(),
                })?;
        if algorithm != "sha256" {
            return Err(AssetRegistryError::UnsupportedDigestAlgorithm {
                reference: image.reference.clone(),
                algorithm: algorithm.to_string(),
            });
        }

        let canon = std::fs::canonicalize(&binding.rootfs).map_err(|e| {
            AssetRegistryError::InvalidRootfsPath {
                rootfs: binding.rootfs.clone(),
                reason: e.to_string(),
            }
        })?;
        if canon == Path::new("/") || !canon.is_dir() {
            return Err(AssetRegistryError::InvalidRootfsPath {
                rootfs: canon,
                reason: "must canonicalize to a real, non-root directory".to_string(),
            });
        }

        let workspace = canon.join("workspace");
        let workspace_meta = std::fs::symlink_metadata(&workspace).map_err(|error| {
            AssetRegistryError::InvalidWorkspaceMountpoint {
                rootfs: canon.clone(),
                reason: format!(
                    "the empty directory must be precreated as part of the hashed asset: {error}"
                ),
            }
        })?;
        if workspace_meta.file_type().is_symlink() || !workspace_meta.is_dir() {
            return Err(AssetRegistryError::InvalidWorkspaceMountpoint {
                rootfs: canon.clone(),
                reason: "must be a real directory, never a symlink or non-directory".to_string(),
            });
        }
        let mut entries = std::fs::read_dir(&workspace).map_err(|error| {
            AssetRegistryError::InvalidWorkspaceMountpoint {
                rootfs: canon.clone(),
                reason: format!("could not verify that it is empty: {error}"),
            }
        })?;
        if entries.next().is_some() {
            return Err(AssetRegistryError::InvalidWorkspaceMountpoint {
                rootfs: canon.clone(),
                reason: "must be empty before it is hidden by the per-job bind mount".to_string(),
            });
        }

        use std::os::unix::fs::MetadataExt as _;
        let identity_before = std::fs::symlink_metadata(&canon).map_err(|error| {
            AssetRegistryError::InvalidRootfsPath {
                rootfs: canon.clone(),
                reason: format!("stat rootfs identity before verification: {error}"),
            }
        })?;
        let identity_before = (identity_before.dev(), identity_before.ino());
        let actual_hex = Self::verify_asset_tree(&canon, expected_owner_uid)?;
        if actual_hex != expected_hex {
            return Err(AssetRegistryError::DigestMismatch {
                reference: image.reference.clone(),
                expected: expected_hex.to_string(),
                actual: actual_hex,
            });
        }

        let identity_after = std::fs::symlink_metadata(&canon).map_err(|error| {
            AssetRegistryError::InvalidRootfsPath {
                rootfs: canon.clone(),
                reason: format!("stat rootfs identity after verification: {error}"),
            }
        })?;
        let identity_after = (identity_after.dev(), identity_after.ino());
        if identity_after != identity_before {
            return Err(AssetRegistryError::InvalidRootfsPath {
                rootfs: canon,
                reason: format!(
                    "rootfs identity changed during digest verification (started \
                     {identity_before:?}, ended {identity_after:?})"
                ),
            });
        }

        Ok(VerifiedRootfs {
            path: canon,
            digest_hex: actual_hex,
            device: identity_before.0,
            inode: identity_before.1,
        })
    }

    pub fn resolve(&self, image: &ImageRef) -> Result<&VerifiedRootfs, AssetRegistryError> {
        self.verified
            .get(&image.reference)
            .ok_or_else(|| AssetRegistryError::UnknownImage {
                reference: image.reference.clone(),
            })
    }

    pub fn resolve_cargo_vendor(
        &self,
        reference: &ImageRef,
    ) -> Result<&VerifiedCargoVendor, AssetRegistryError> {
        self.cargo_vendor.get(&reference.reference).ok_or_else(|| {
            AssetRegistryError::UnknownImage {
                reference: reference.reference.clone(),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    struct Fixture {
        dir: PathBuf,
        image: ImageRef,
    }

    impl Fixture {
        fn new(tag: &str, content: &[u8]) -> Fixture {
            let dir = std::env::temp_dir().join(format!(
                "myelin-asset-registry-test-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            fs::create_dir(dir.join("workspace")).unwrap();
            fs::write(dir.join("payload"), content).unwrap();
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
            fs::set_permissions(dir.join("workspace"), fs::Permissions::from_mode(0o755)).unwrap();
            fs::set_permissions(dir.join("payload"), fs::Permissions::from_mode(0o644)).unwrap();
            let digest = canonical_tar::canonical_tree_sha256_hex(&dir).unwrap();
            let image =
                ImageRef::pinned(format!("test.local/{tag}-rootfs@sha256:{digest}")).unwrap();
            Fixture { dir, image }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn binding(image: ImageRef, rootfs: impl Into<PathBuf>) -> RootfsAssetBinding {
        RootfsAssetBinding {
            image,
            rootfs: rootfs.into(),
        }
    }

    #[test]
    fn writable_asset_entry_refuses_registry_construction_with_path_and_mode() {
        for (tag, relative_path, mode, kind) in [
            ("group-writable-file", "payload", 0o664, "file"),
            ("world-writable-dir", "writable-dir", 0o757, "dir"),
        ] {
            let fixture = Fixture::new(tag, b"content whose mode is pinned");
            let offending_path = fixture.dir.join(relative_path);
            match kind {
                "file" => {}
                "dir" => fs::create_dir(&offending_path).unwrap(),
                _ => unreachable!(),
            }
            fs::set_permissions(&offending_path, fs::Permissions::from_mode(mode)).unwrap();

            let digest = canonical_tar::canonical_tree_sha256_hex(&fixture.dir).unwrap();
            let image =
                ImageRef::pinned(format!("test.local/{tag}-rootfs@sha256:{digest}")).unwrap();
            let error = GvisorAssetRegistry::from_bindings(vec![binding(image, &fixture.dir)])
                .expect_err("a group/world-writable asset entry must refuse construction");
            let rendered = error.to_string();
            assert!(
                rendered.contains(&offending_path.display().to_string()),
                "error must name offending path: {rendered}"
            );
            assert!(
                rendered.contains(&format!("{mode:04o}")),
                "error must name offending mode: {rendered}"
            );
        }
    }

    #[test]
    fn writable_symlink_is_accepted() {
        let fixture = Fixture::new("accepted-symlink", b"real content");
        std::os::unix::fs::symlink("payload", fixture.dir.join("link")).unwrap();
        let digest = canonical_tar::canonical_tree_sha256_hex(&fixture.dir).unwrap();
        let image =
            ImageRef::pinned(format!("test.local/accepted-symlink-rootfs@sha256:{digest}")).unwrap();
        GvisorAssetRegistry::from_bindings(vec![binding(image, &fixture.dir)])
            .expect("a 0o777 symlink must not refuse asset-registry construction");
    }

    #[test]
    fn foreign_owned_asset_entry_refuses_metadata_verification() {
        let fixture = Fixture::new("foreign-owner", b"owner-owned content");
        let path = fixture.dir.join("payload");
        let metadata = fs::symlink_metadata(&path).unwrap();
        let actual_uid = std::os::unix::fs::MetadataExt::uid(&metadata);
        let foreign_expected_uid = if actual_uid == 0 { 1 } else { 0 };

        let error =
            canonical_tar::verify_asset_entry_metadata(&path, &metadata, foreign_expected_uid)
                .expect_err("metadata owned by another uid must be refused");
        assert!(matches!(
            error,
            canonical_tar::AssetTreeVerificationError::UnexpectedOwner {
                path: ref offending_path,
                expected_uid,
                actual_uid: owner_uid,
            } if offending_path == &path
                && expected_uid == foreign_expected_uid
                && owner_uid == actual_uid
        ));
    }

    #[test]
    fn owner_owned_0755_dirs_and_0644_files_are_accepted() {
        let fixture = Fixture::new("safe-metadata", b"clean asset content");
        let registry =
            GvisorAssetRegistry::from_bindings(vec![binding(fixture.image.clone(), &fixture.dir)])
                .expect("owner-owned tree with conservative modes must verify");

        assert_eq!(
            registry.resolve(&fixture.image).unwrap().path(),
            fs::canonicalize(&fixture.dir).unwrap()
        );
    }

    #[test]
    fn unknown_image_refuses() {
        let registry = GvisorAssetRegistry::from_bindings(vec![]).unwrap();
        let unknown = ImageRef::pinned(
            "test.local/never-registered@sha256:0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap();
        let err = registry
            .resolve(&unknown)
            .expect_err("an unregistered image must never resolve");
        assert!(matches!(err, AssetRegistryError::UnknownImage { .. }));
    }

    #[test]
    fn pinned_workspace_mountpoint_keeps_digest_stable_across_simulated_job() {
        let fixture = Fixture::new("workspace-stability", b"immutable rootfs payload");
        let workspace_mountpoint = fixture.dir.join("workspace");
        assert!(workspace_mountpoint.is_dir());
        assert_eq!(fs::read_dir(&workspace_mountpoint).unwrap().count(), 0);

        let before = canonical_tar::canonical_tree_sha256_hex(&fixture.dir).unwrap();
        let registry = GvisorAssetRegistry::from_bindings(vec![binding(
            fixture.image.clone(),
            fixture.dir.clone(),
        )])
        .expect("the pinned asset already carries the fixed mountpoint");

        let job_workspace = fixture.dir.with_extension("simulated-job-workspace");
        fs::create_dir(&job_workspace).unwrap();
        fs::write(job_workspace.join("Cargo.lock"), b"job-owned bytes").unwrap();
        registry.resolve(&fixture.image).unwrap();
        fs::remove_dir_all(&job_workspace).unwrap();

        let after = canonical_tar::canonical_tree_sha256_hex(&fixture.dir).unwrap();
        assert_eq!(
            before, after,
            "a workspace job must not dirty the pinned rootfs"
        );
        GvisorAssetRegistry::from_bindings(vec![binding(
            fixture.image.clone(),
            fixture.dir.clone(),
        )])
        .expect("fresh registry construction still accepts the rootfs after the job");
    }

    #[test]
    fn absent_workspace_mountpoint_refuses_construction_before_hash_trust() {
        let dir = std::env::temp_dir().join(format!(
            "myelin-asset-registry-no-workspace-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let digest = canonical_tar::canonical_tree_sha256_hex(&dir).unwrap();
        let image =
            ImageRef::pinned(format!("test.local/no-workspace-rootfs@sha256:{digest}")).unwrap();

        let error = GvisorAssetRegistry::from_bindings(vec![binding(image, dir.clone())])
            .expect_err("launch must never create /workspace after the asset was hashed");
        assert!(matches!(
            error,
            AssetRegistryError::InvalidWorkspaceMountpoint { .. }
        ));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn digest_mismatch_refuses_construction() {
        let fixture = Fixture::new("mismatch-a", b"original content");
        fs::write(
            fixture.dir.join("payload"),
            b"mutated content - no longer matches the pin",
        )
        .unwrap();

        let err = GvisorAssetRegistry::from_bindings(vec![binding(
            fixture.image.clone(),
            fixture.dir.clone(),
        )])
        .expect_err("a drifted directory must refuse construction, not silently verify");
        assert!(matches!(err, AssetRegistryError::DigestMismatch { .. }));
    }

    #[test]
    fn drifted_content_refuses_construction() {
        let fixture = Fixture::new("drift", b"the content the digest was computed from");
        assert!(
            GvisorAssetRegistry::from_bindings(vec![binding(
                fixture.image.clone(),
                fixture.dir.clone()
            )])
            .is_ok(),
            "sanity: matches before drift"
        );

        fs::write(fixture.dir.join("extra-file-nobody-asked-for"), b"drift").unwrap();
        let err = GvisorAssetRegistry::from_bindings(vec![binding(
            fixture.image.clone(),
            fixture.dir.clone(),
        )])
        .expect_err("content drift must refuse construction post-drift");
        assert!(matches!(err, AssetRegistryError::DigestMismatch { .. }));
    }

    #[test]
    fn env_var_cannot_rescue_an_unregistered_image() {
        let other = Fixture::new("env-var-decoy", b"a perfectly real, valid, unrelated tree");
        std::env::set_var(crate::gvisor::ENV_GVISOR_ROOTFS, &other.dir);
        let registry = GvisorAssetRegistry::from_bindings(vec![]).unwrap();
        let unregistered = ImageRef::pinned(
            "test.local/still-unregistered@sha256:1111111111111111111111111111111111111111111111111111111111111111",
        )
        .unwrap();
        let result = registry.resolve(&unregistered);
        std::env::remove_var(crate::gvisor::ENV_GVISOR_ROOTFS);
        assert!(
            matches!(result, Err(AssetRegistryError::UnknownImage { .. })),
            "the env var must never be consulted by resolve - an unknown image refuses regardless \
             of what MYELIN_GVISOR_ROOTFS happens to point at"
        );
    }

    #[test]
    fn two_registered_images_resolve_to_two_distinct_correct_paths() {
        let a = Fixture::new("distinct-a", b"tree A content");
        let b = Fixture::new("distinct-b", b"tree B content, deliberately different");
        let registry = GvisorAssetRegistry::from_bindings(vec![
            binding(a.image.clone(), a.dir.clone()),
            binding(b.image.clone(), b.dir.clone()),
        ])
        .expect("both bindings verify");

        let verified_a = registry.resolve(&a.image).expect("a resolves");
        let verified_b = registry.resolve(&b.image).expect("b resolves");
        assert_ne!(verified_a.path(), verified_b.path());
        assert_eq!(verified_a.path(), fs::canonicalize(&a.dir).unwrap());
        assert_eq!(verified_b.path(), fs::canonicalize(&b.dir).unwrap());
        assert_ne!(verified_a.digest_hex(), verified_b.digest_hex());
    }

    #[test]
    fn duplicate_reference_refuses_construction() {
        let a = Fixture::new("dup-a", b"first content");
        let b_dir = std::env::temp_dir().join(format!(
            "myelin-asset-registry-test-dup-b-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&b_dir).unwrap();
        fs::write(b_dir.join("payload"), b"second content").unwrap();

        let err = GvisorAssetRegistry::from_bindings(vec![
            binding(a.image.clone(), a.dir.clone()),
            binding(a.image.clone(), b_dir.clone()),
        ])
        .expect_err("a duplicate reference must refuse construction, not silently overwrite");
        assert!(matches!(err, AssetRegistryError::DuplicateReference { .. }));
        let _ = fs::remove_dir_all(&b_dir);
    }

    #[test]
    fn unsupported_digest_algorithm_refuses_construction() {
        let fixture = Fixture::new("algo", b"irrelevant - never reaches the hasher");
        let sha384_reference = format!(
            "test.local/algo-rootfs@sha384:{}",
            "a".repeat(96)
        );
        let sha384_image = ImageRef::pinned(&sha384_reference).unwrap();
        assert!(
            sha384_image.digest_pinned(),
            "sanity: ImageRef accepts a well-formed sha384 pin"
        );

        let err = GvisorAssetRegistry::from_bindings(vec![binding(
            sha384_image.clone(),
            fixture.dir.clone(),
        )])
        .expect_err("sha384 has no matching canonical-tree hasher yet");
        assert!(matches!(
            err,
            AssetRegistryError::UnsupportedDigestAlgorithm { .. }
        ));
    }

    #[test]
    fn absent_rootfs_path_refuses_construction() {
        let image = ImageRef::pinned(
            "test.local/absent-rootfs@sha256:2222222222222222222222222222222222222222222222222222222222222222",
        )
        .unwrap();
        let err = GvisorAssetRegistry::from_bindings(vec![binding(
            image.clone(),
            PathBuf::from("/this/path/does/not/exist/anywhere"),
        )])
        .expect_err("an absent registered path must refuse construction, not panic");
        assert!(matches!(err, AssetRegistryError::InvalidRootfsPath { .. }));
    }

    fn cargo_vendor_fixture(tag: &str) -> (PathBuf, ImageRef, String) {
        let root = std::env::temp_dir().join(format!(
            "myelin-cargo-vendor-registry-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let crate_dir = root.join("vendor/itoa-1.0.15");
        let cargo_dir = root.join(".cargo");
        fs::create_dir_all(&crate_dir).unwrap();
        fs::create_dir_all(&cargo_dir).unwrap();
        fs::write(crate_dir.join("lib.rs"), b"external crate").unwrap();
        fs::write(
            cargo_dir.join("config.toml"),
            b"[source.crates-io]\nreplace-with='vendored-sources'\n",
        )
        .unwrap();
        let lock_bytes = b"# exact fixture lock\n";
        fs::write(root.join("Cargo.lock"), lock_bytes).unwrap();

        for dir in [&root, &root.join("vendor"), &crate_dir, &cargo_dir] {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
        }
        for file in [
            crate_dir.join("lib.rs"),
            cargo_dir.join("config.toml"),
            root.join("Cargo.lock"),
        ] {
            fs::set_permissions(file, fs::Permissions::from_mode(0o644)).unwrap();
        }

        let digest = canonical_tar::canonical_tree_sha256_hex(&root).unwrap();
        let reference =
            ImageRef::pinned(format!("test.local/cargo-vendor-{tag}@sha256:{digest}")).unwrap();
        let lock_sha256 = format!("{:x}", Sha256::digest(lock_bytes));
        (root, reference, lock_sha256)
    }

    fn cargo_vendor_binding(
        reference: ImageRef,
        root: impl Into<PathBuf>,
        cargo_lock_sha256: impl Into<String>,
    ) -> CargoVendorAssetBinding {
        CargoVendorAssetBinding {
            reference,
            root: root.into(),
            cargo_lock_sha256: cargo_lock_sha256.into(),
        }
    }

    #[test]
    fn lockfile_keyed_vendor_selection_maps_only_registered_locks_and_fails_closed_otherwise() {
        assert_eq!(
            select_registered_cargo_vendor(CARGO_VENDOR_SMOKE_LOCK_SHA256).as_deref(),
            Some(cargo_vendor_smoke_reference().as_str())
        );
        assert_eq!(
            select_registered_cargo_vendor(CARGO_VENDOR_WORKSPACE_LOCK_SHA256).as_deref(),
            Some(cargo_vendor_workspace_reference().as_str())
        );
        assert!(cargo_vendor_smoke_reference()
            .starts_with("myelin.local/cargo-vendor-smoke-v1@sha256:"));
        assert!(cargo_vendor_workspace_reference()
            .starts_with("myelin.local/cargo-vendor-workspace-v1@sha256:"));
        assert_eq!(select_registered_cargo_vendor(&"0".repeat(64)), None);
        assert_eq!(
            select_registered_cargo_vendor(&cargo_lock_sha256_hex(b"not a registered lock")),
            None
        );
        assert_eq!(
            cargo_lock_sha256_hex(CARGO_VENDOR_SMOKE_LOCK_SHA256.as_bytes()).len(),
            64
        );
    }

    #[test]
    fn cargo_vendor_with_a_non_world_readable_file_is_refused_fail_closed() {
        let (root, _reference, lock_sha256) = cargo_vendor_fixture("non-readable");
        let offending = root.join("vendor/itoa-1.0.15/lib.rs");
        fs::set_permissions(&offending, fs::Permissions::from_mode(0o640)).unwrap();
        let digest = canonical_tar::canonical_tree_sha256_hex(&root).unwrap();
        let reference =
            ImageRef::pinned(format!("test.local/cargo-vendor-nonreadable@sha256:{digest}")).unwrap();
        let err = GvisorAssetRegistry::from_bindings_with_cargo_vendor(
            Vec::new(),
            vec![cargo_vendor_binding(reference, &root, &lock_sha256)],
        )
        .expect_err("a vendor tree with a non-world-readable file must refuse construction");
        match err {
            AssetRegistryError::InvalidCargoVendor { reason, .. } => {
                assert!(
                    reason.contains("world-readable"),
                    "reason should name the readability rule: {reason}"
                );
                assert!(
                    reason.contains("lib.rs"),
                    "reason should name the offending path: {reason}"
                );
            }
            other => panic!("expected InvalidCargoVendor, got {other:?}"),
        }
    }

    #[test]
    fn verified_cargo_vendor_resolution_round_trips_all_pinned_identity() {
        let (root, reference, lock_sha256) = cargo_vendor_fixture("round-trip");
        let expected_tree_sha256 = reference.parse_digest().unwrap().1.to_string();
        let registry = GvisorAssetRegistry::from_bindings_with_cargo_vendor(
            Vec::new(),
            vec![cargo_vendor_binding(reference.clone(), &root, &lock_sha256)],
        )
        .expect("the complete vendor tree verifies at registry construction");

        let verified = registry
            .resolve_cargo_vendor(&reference)
            .expect("the exact registered reference resolves");
        assert_eq!(verified.path(), fs::canonicalize(&root).unwrap());
        assert_eq!(verified.digest_hex(), expected_tree_sha256);
        assert_eq!(verified.cargo_lock_sha256(), lock_sha256);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cargo_vendor_writable_or_foreign_owned_entry_is_refused_by_shared_tree_walk() {
        let (writable_root, writable_reference, lock_sha256) = cargo_vendor_fixture("writable");
        let writable_path = writable_root.join("vendor/itoa-1.0.15/lib.rs");
        fs::set_permissions(&writable_path, fs::Permissions::from_mode(0o664)).unwrap();
        let writable_error = GvisorAssetRegistry::from_bindings_with_cargo_vendor(
            Vec::new(),
            vec![cargo_vendor_binding(
                writable_reference,
                &writable_root,
                &lock_sha256,
            )],
        )
        .expect_err("a group-writable vendor entry must refuse registry construction");
        assert!(matches!(
            writable_error,
            AssetRegistryError::GroupOrWorldWritable { ref path, mode, .. }
                if path == &writable_path && mode == 0o664
        ));
        let _ = fs::remove_dir_all(writable_root);

        let (foreign_root, foreign_reference, foreign_lock_sha256) =
            cargo_vendor_fixture("foreign-owner");
        let actual_uid = unsafe { libc::geteuid() };
        let foreign_expected_uid = if actual_uid == 0 { 1 } else { 0 };
        let foreign_error = GvisorAssetRegistry::from_bindings_with_owner(
            Vec::new(),
            vec![cargo_vendor_binding(
                foreign_reference,
                &foreign_root,
                foreign_lock_sha256,
            )],
            foreign_expected_uid,
        )
        .expect_err("a foreign-owned vendor entry must refuse registry construction");
        assert!(matches!(
            foreign_error,
            AssetRegistryError::UnexpectedOwner {
                expected_uid,
                actual_uid: owner_uid,
                ..
            } if expected_uid == foreign_expected_uid && owner_uid == actual_uid
        ));
        let _ = fs::remove_dir_all(foreign_root);
    }
}
