//! # `GvisorAssetRegistry` — making `JobSpec.image` the real launch authority (CT-007 gate 2/4)
//!
//! **The gap this closes.** [`crate::gvisor::run_production_container_streaming`] used to resolve
//! the rootfs to actually launch PURELY from the `MYELIN_GVISOR_ROOTFS` env var
//! ([`crate::gvisor::resolved_gvisor_rootfs`]), completely ignoring `spec.image` (a digest-pinned
//! [`crate::ImageRef`]) — so the "digest-pinned runner asset" claims were disconnected from what
//! actually executes: any syntactically valid `ImageRef` could describe a job while a completely
//! different, mutable directory (whatever the env var happened to point at) is what actually ran.
//!
//! [`GvisorAssetRegistry`] is an explicit, closed map from an EXACT registered `ImageRef` reference
//! to the on-disk rootfs directory that reference is pinned to. **Verification happens ONCE, at
//! construction** ([`GvisorAssetRegistry::from_bindings`]): parse the digest out of each bound
//! image, canonicalize its registered directory, recompute ITS canonical-tree digest with the
//! pure-Rust hasher ([`crate::canonical_tar`] — no host `tar` process, so this is safe to call from
//! the trusted composition-root path), and refuse (a typed [`AssetRegistryError`], never a panic or
//! a silent warning) on ANY mismatch: unknown reference, unsupported digest algorithm, an
//! invalid/absent/root rootfs path, or a digest that does not match — for ANY single binding, the
//! WHOLE construction refuses (a runner must never start holding even one asset it cannot prove).
//! [`GvisorAssetRegistry::resolve`] is the ONLY thing a launch calls: an O(1) map lookup against the
//! already-verified results, no I/O, no hashing. [`crate::gvisor::GvisorBackend::launch_with`] calls
//! `resolve` AFTER `hooks.enforce_isolation_floor` + the hardening assert but BEFORE `hooks.reserve`
//! /`hooks.acquire_launch_permit`/anything else — an unknown image still refuses before any resource
//! is reserved or any launch permit is granted, but a red isolation floor now refuses BEFORE the
//! (now-cheap, but still a lookup that should never run on a floor that already failed) registry
//! consultation, matching this crate's mandated hook ordering (see the Firecracker backend's own
//! `launch_with` for the pattern this crate follows consistently). Before this, `resolve_verified`
//! re-canonicalized AND re-hashed the ENTIRE registered directory on EVERY launch (~15s measured on
//! the >800MiB Rust asset on this host) — paid before the isolation floor even ran.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::canonical_tar;
use crate::ImageRef;

/// One raw, UNVERIFIED asset binding: the image reference it is pinned under, and the on-disk
/// rootfs directory as given by the caller (typically the result of an env-var-backed resolver like
/// [`crate::gvisor::resolved_gvisor_rootfs`] at the moment the caller built this binding).
/// [`GvisorAssetRegistry::from_bindings`] canonicalizes and hashes this rootfs EXACTLY ONCE, at
/// construction — the registry entry it produces does NOT track any later change to an env var or
/// to whatever the path resolved to when the binding was built; only re-pointing the registered
/// directory's OWN symlink (if `rootfs` itself is a symlink) before construction would matter, since
/// canonicalization happens once, up front.
#[derive(Clone, Debug)]
pub struct RootfsAssetBinding {
    pub image: ImageRef,
    pub rootfs: PathBuf,
}

/// A closed, explicit map from an EXACT `ImageRef` reference string to the ALREADY-VERIFIED rootfs
/// it is pinned to. Constructed once at composition-root time via [`GvisorAssetRegistry::from_bindings`]
/// (see `myelin-ci-controlplane`'s `runner_bind.rs` for the real founder-pipeline registry, and each
/// gVisor test file for its own test-scoped registry) and handed to [`crate::gvisor::GvisorBackend::new`]
/// as the sole authority an ordinary (non-git-wire) launch consults to turn `spec.image` into an
/// actual rootfs. Every value behind this map is a [`VerifiedRootfs`] — the expensive canonicalize +
/// hash work already happened at construction, so [`GvisorAssetRegistry::resolve`] is a cheap,
/// per-launch-safe O(1) lookup.
#[derive(Debug)]
pub struct GvisorAssetRegistry {
    verified: HashMap<String, VerifiedRootfs>,
}

/// A rootfs whose canonical-tree digest has been recomputed, in-process, and PROVEN to match the
/// digest embedded in a registered [`ImageRef`]. The `path` field is deliberately private — the only
/// way to construct a value of this type is [`GvisorAssetRegistry::from_bindings`], so no other
/// code in this crate can fabricate a "verified" rootfs without actually going through verification.
#[derive(Clone, Debug)]
pub struct VerifiedRootfs {
    path: PathBuf,
    digest_hex: String,
}

impl VerifiedRootfs {
    /// The canonicalized, verified rootfs directory the launch path may stage a bundle from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The canonical-tree SHA-256 digest that was verified to match the resolved image's pin, hex
    /// encoded.
    pub fn digest_hex(&self) -> &str {
        &self.digest_hex
    }
}

/// Why construction ([`GvisorAssetRegistry::from_bindings`]) or lookup ([`GvisorAssetRegistry::resolve`])
/// refused. Every variant is a FAIL-CLOSED refusal — there is no "verified with a warning" outcome.
#[derive(Debug)]
pub enum AssetRegistryError {
    /// No binding is registered for this EXACT reference string (no fuzzy/prefix match is ever
    /// attempted).
    UnknownImage { reference: String },
    /// [`GvisorAssetRegistry::from_bindings`] was given two bindings under the EXACT same reference
    /// string — composition-time misconfiguration, refused rather than silently letting the later
    /// one overwrite the earlier.
    DuplicateReference { reference: String },
    /// The reference is not digest-pinned at all, or the digest algorithm named in it has no
    /// matching hasher yet (today: only `sha256`).
    UnsupportedDigestAlgorithm {
        reference: String,
        algorithm: String,
    },
    /// The registered rootfs path does not canonicalize to a real, non-root directory.
    InvalidRootfsPath { rootfs: PathBuf, reason: String },
    /// A registered workload rootfs does not already contain the fixed, empty `/workspace` bind
    /// target. The mountpoint is pinned content: launch code must never create it after hashing.
    InvalidWorkspaceMountpoint { rootfs: PathBuf, reason: String },
    /// The recomputed canonical-tree digest of the registered directory does not match the digest
    /// embedded in the image reference.
    DigestMismatch {
        reference: String,
        expected: String,
        actual: String,
    },
    /// Hashing the registered directory failed (I/O error, unsupported file type, …).
    Hashing { rootfs: PathBuf, reason: String },
    /// A shared asset entry is writable by its group or by everyone. Such an entry could be
    /// mutated by a non-owner after its content was hashed.
    GroupOrWorldWritable {
        rootfs: PathBuf,
        path: PathBuf,
        mode: u32,
    },
    /// A shared asset entry is not owned by the runner process's effective uid (the asset-store
    /// owner), so another uid retains authority to mutate it after verification.
    UnexpectedOwner {
        rootfs: PathBuf,
        path: PathBuf,
        expected_uid: u32,
        actual_uid: u32,
    },
}

impl std::fmt::Display for AssetRegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssetRegistryError::UnknownImage { reference } => write!(
                f,
                "gvisor asset registry: no runner asset is registered for image `{reference}` — an \
                 unregistered image can never launch (the env var is not a fallback authority)"
            ),
            AssetRegistryError::DuplicateReference { reference } => write!(
                f,
                "gvisor asset registry: image `{reference}` was registered more than once — a \
                 second binding under the same exact reference is a composition-time \
                 misconfiguration, refused rather than silently overwriting the first"
            ),
            AssetRegistryError::UnsupportedDigestAlgorithm { reference, algorithm } => write!(
                f,
                "gvisor asset registry: image `{reference}` is pinned with an unsupported digest \
                 algorithm `{algorithm}` — only `sha256` has a matching canonical-tree hasher today"
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
                "gvisor asset registry: registered rootfs for image `{reference}` has DRIFTED — \
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
        }
    }
}

impl std::error::Error for AssetRegistryError {}

impl GvisorAssetRegistry {
    /// **The sole construction path.** Canonicalizes + hashes EACH `binding` ONCE, right here, and
    /// REFUSES (returns `Err`) the WHOLE construction the moment any single one fails verification —
    /// a runner must never start holding even one asset it cannot prove. Also refuses two bindings
    /// registered under the same exact `image.reference` string
    /// ([`AssetRegistryError::DuplicateReference`]) — composition-time misconfiguration, not
    /// permitted. On success every entry behind the returned registry is an already-[`VerifiedRootfs`];
    /// [`Self::resolve`] never does I/O or hashing again.
    pub fn from_bindings(
        bindings: Vec<RootfsAssetBinding>,
    ) -> Result<GvisorAssetRegistry, AssetRegistryError> {
        // The runner account is also the owner of its private asset store. Capture the effective
        // uid once so every binding in this registry is checked against one stable authority. An
        // externally configurable uid would let configuration redefine which foreign owner is
        // trusted and is unnecessary for the current single-owner store model.
        // SAFETY: `geteuid` takes no pointers and has no preconditions.
        let expected_owner_uid = unsafe { libc::geteuid() };
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
        Ok(GvisorAssetRegistry { verified })
    }

    /// The one-time verification a construction-time binding undergoes: a supported (`sha256`)
    /// digest algorithm, a real canonicalized non-root directory, and a recomputed canonical-tree
    /// digest that matches the digest embedded in `binding.image`. Every failure mode is a typed,
    /// fail-closed [`AssetRegistryError`] — never a panic, never a silent warning.
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

        let actual_hex = canonical_tar::verified_asset_tree_sha256_hex(&canon, expected_owner_uid)
            .map_err(|error| match error {
                canonical_tar::AssetTreeVerificationError::Io(error) => {
                    AssetRegistryError::Hashing {
                        rootfs: canon.clone(),
                        reason: error.to_string(),
                    }
                }
                canonical_tar::AssetTreeVerificationError::GroupOrWorldWritable { path, mode } => {
                    AssetRegistryError::GroupOrWorldWritable {
                        rootfs: canon.clone(),
                        path,
                        mode,
                    }
                }
                canonical_tar::AssetTreeVerificationError::UnexpectedOwner {
                    path,
                    expected_uid,
                    actual_uid,
                } => AssetRegistryError::UnexpectedOwner {
                    rootfs: canon.clone(),
                    path,
                    expected_uid,
                    actual_uid,
                },
            })?;
        if actual_hex != expected_hex {
            return Err(AssetRegistryError::DigestMismatch {
                reference: image.reference.clone(),
                expected: expected_hex.to_string(),
                actual: actual_hex,
            });
        }

        Ok(VerifiedRootfs {
            path: canon,
            digest_hex: actual_hex,
        })
    }

    /// Resolve `image` to its already-[`VerifiedRootfs`] — an O(1) map lookup ONLY, no
    /// canonicalize, no hash, no I/O beyond the lookup itself. [`AssetRegistryError::UnknownImage`]
    /// is the only way this ever refuses: there is nothing left to fail once construction succeeded.
    pub fn resolve(&self, image: &ImageRef) -> Result<&VerifiedRootfs, AssetRegistryError> {
        self.verified
            .get(&image.reference)
            .ok_or_else(|| AssetRegistryError::UnknownImage {
                reference: image.reference.clone(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    /// A throwaway real directory containing one file, hashed with the SAME pure-Rust hasher the
    /// registry itself uses — so a test can register a genuinely digest-matching binding without
    /// depending on any staged host asset.
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
        // NOTE: symlinks are deliberately NOT in this "refuses" set — a symlink's 0o777 mode is
        // meaningless (see `writable_symlink_is_accepted` below and canonical_tar's symlink skip).
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

            // Pin the tree only after introducing the unsafe mode. Before the hardening, its
            // content digest is valid and registry construction therefore accepts it.
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

    /// A `0o777` symlink (every real rootfs carries them — `/etc/mtab`, `/bin/sh`, `lib64`) must NOT
    /// refuse registry construction: a symlink's own mode is ignored by the kernel (no write-through),
    /// and retargeting it needs write on its PARENT DIRECTORY, which IS mode-verified. Rejecting it
    /// would refuse every real staged rootfs asset.
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

    /// (a) An unknown `ImageRef` refuses before any reserve/spawn is attempted — here, before
    /// anything at all, since `resolve` is a pure lookup with no side effects.
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

        // Simulate the only host-side filesystem lifecycle a workspace job owns: the writable
        // source is created and populated outside the rootfs, then discarded. gVisor binds that
        // source over the already-pinned empty target; no target creation belongs to launch.
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

    /// (b) A binding whose actual directory content does NOT match the pinned digest refuses
    /// CONSTRUCTION itself — never "verified with a warning", and never a registry that limps along
    /// holding an unverifiable entry.
    #[test]
    fn digest_mismatch_refuses_construction() {
        let fixture = Fixture::new("mismatch-a", b"original content");
        // Mutate the directory BEFORE construction so its content no longer matches the pin the
        // image names.
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

    /// (c) An existing digest-named directory that has drifted from what its binding expects
    /// refuses construction — mirrors the drift bug already fixed in `build-rust-rootfs.sh` (same
    /// failure class: a directory whose name/label claims a digest its content no longer produces),
    /// just caught at the registry layer instead of the staging-script layer. Distinguished from (b)
    /// only by intent/comment — same mechanism, so the same assertion applies.
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

    /// (d) `MYELIN_GVISOR_ROOTFS` pointing at some OTHER valid tree cannot rescue an
    /// unregistered/unknown image — the registry is a closed map; an env var is never consulted by
    /// `resolve` at all, so it cannot act as an authority-bearing fallback for anything routed
    /// through it.
    #[test]
    fn env_var_cannot_rescue_an_unregistered_image() {
        let other = Fixture::new("env-var-decoy", b"a perfectly real, valid, unrelated tree");
        // Point the base rootfs env var at a real, valid, hash-computable directory...
        std::env::set_var(crate::gvisor::ENV_GVISOR_ROOTFS, &other.dir);
        let registry = GvisorAssetRegistry::from_bindings(vec![]).unwrap(); // ...but NOTHING is registered.
        let unregistered = ImageRef::pinned(
            "test.local/still-unregistered@sha256:1111111111111111111111111111111111111111111111111111111111111111",
        )
        .unwrap();
        let result = registry.resolve(&unregistered);
        std::env::remove_var(crate::gvisor::ENV_GVISOR_ROOTFS);
        assert!(
            matches!(result, Err(AssetRegistryError::UnknownImage { .. })),
            "the env var must never be consulted by resolve — an unknown image refuses regardless \
             of what MYELIN_GVISOR_ROOTFS happens to point at"
        );
    }

    /// (e) Two different registered images resolve to two distinct, correct rootfs paths.
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

    /// A second binding under the EXACT same reference string refuses construction — never a silent
    /// overwrite of the first.
    #[test]
    fn duplicate_reference_refuses_construction() {
        let a = Fixture::new("dup-a", b"first content");
        // Build a SECOND binding under the exact same reference string but a different (also
        // genuinely valid) directory — the duplicate check must fire before either is trusted.
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

    /// An unsupported digest algorithm (only `sha256` has a matching hasher today) refuses
    /// construction with a clear typed error rather than silently accepting or panicking — even
    /// though `ImageRef` itself treats `sha384`/`sha512` as validly "digest-pinned".
    #[test]
    fn unsupported_digest_algorithm_refuses_construction() {
        let fixture = Fixture::new("algo", b"irrelevant - never reaches the hasher");
        let sha384_reference = format!(
            "test.local/algo-rootfs@sha384:{}",
            "a".repeat(96) // the correct hex length for sha384, so ImageRef itself accepts the pin
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

    /// A registered rootfs path that does not canonicalize to a real directory (absent, or a plain
    /// file) refuses construction with `InvalidRootfsPath`, never a panic.
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
}
