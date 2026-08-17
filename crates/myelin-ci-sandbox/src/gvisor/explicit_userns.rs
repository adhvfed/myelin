use crate::user_namespace::RunscInvocationMode;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const ENV_EXPLICIT_USERNS_HELPER_DIR: &str = "MYELIN_EXPLICIT_USERNS_HELPER_DIR";

pub fn resolved_explicit_userns_helper_dir() -> &'static Path {
    static CACHED: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    CACHED.get_or_init(|| {
        std::env::var(ENV_EXPLICIT_USERNS_HELPER_DIR)
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/usr/bin"))
    })
}

pub const ENV_EXPLICIT_USERNS_RUNSC_ROOT: &str = "MYELIN_EXPLICIT_USERNS_RUNSC_ROOT";

pub fn resolved_explicit_userns_runsc_root() -> &'static Path {
    static CACHED: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    CACHED.get_or_init(|| {
        let configured = std::env::var(ENV_EXPLICIT_USERNS_RUNSC_ROOT)
            .ok()
            .map(PathBuf::from);
        let default = || {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
            PathBuf::from(home)
                .join(".local")
                .join("state")
                .join("myelin-runsc-explicit-userns")
        };
        let resolved = configured.unwrap_or_else(default);
        if resolved.is_absolute() {
            resolved
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(&resolved))
                .unwrap_or(resolved)
        }
    })
}

pub fn preflight_explicit_userns_helpers(helper_dir: &Path) -> Result<(), String> {
    if !helper_dir.is_absolute() {
        return Err(format!("{helper_dir:?} must be an absolute path"));
    }
    let dir_meta =
        std::fs::symlink_metadata(helper_dir).map_err(|e| format!("stat {helper_dir:?}: {e}"))?;
    if dir_meta.file_type().is_symlink() {
        return Err(format!("{helper_dir:?} must not be a symlink"));
    }
    if !dir_meta.is_dir() {
        return Err(format!("{helper_dir:?} is not a directory"));
    }
    if dir_meta.uid() != 0 {
        return Err(format!(
            "{helper_dir:?} must be owned by root (uid 0), got uid {}",
            dir_meta.uid()
        ));
    }
    if dir_meta.mode() & 0o022 != 0 {
        return Err(format!(
            "{helper_dir:?} must not be group/other-writable (mode {:o})",
            dir_meta.mode() & 0o777
        ));
    }
    crate::dirlock::verify_ancestors_not_writable_by_us(helper_dir).map_err(|reason| {
        format!("{helper_dir:?}'s ancestor chain is not safely anchored: {reason}")
    })?;
    const NEWUIDMAP_CAP_SETUID_EP: &[u8] =
        b"\x01\x00\x00\x02\x80\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";
    const NEWGIDMAP_CAP_SETGID_EP: &[u8] =
        b"\x01\x00\x00\x02\x40\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";
    for (helper, expected_file_capability) in [
        ("newuidmap", NEWUIDMAP_CAP_SETUID_EP),
        ("newgidmap", NEWGIDMAP_CAP_SETGID_EP),
    ] {
        let path = helper_dir.join(helper);
        let meta = std::fs::symlink_metadata(&path).map_err(|e| format!("stat {path:?}: {e}"))?;
        if meta.file_type().is_symlink() {
            return Err(format!("{path:?} must not be a symlink"));
        }
        if !meta.is_file() {
            return Err(format!("{path:?} must be a regular file"));
        }
        if meta.uid() != 0 {
            return Err(format!(
                "{path:?} must be owned by root (uid 0), got uid {}",
                meta.uid()
            ));
        }
        if meta.mode() & 0o4000 == 0 {
            return Err(format!(
                "{path:?} must be setuid (mode {:o} lacks the setuid bit)",
                meta.mode() & 0o7777
            ));
        }
        if meta.mode() & 0o022 != 0 {
            return Err(format!(
                "{path:?} must not be group/other-writable (mode {:o})",
                meta.mode() & 0o777
            ));
        }
        verify_helper_security_capability_xattr(&path, expected_file_capability)?;
        let path_c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
            .map_err(|e| format!("{path:?} contains an interior NUL: {e}"))?;
        let executable_by_us = unsafe {
            libc::faccessat(
                libc::AT_FDCWD,
                path_c.as_ptr(),
                libc::X_OK,
                libc::AT_EACCESS,
            )
        } == 0;
        if !executable_by_us {
            return Err(format!(
                "{path:?} is not executable by this process's effective identity"
            ));
        }
    }
    Ok(())
}

pub(super) const PINNED_EXPLICIT_USERNS_RUNSC_VERSION: &str = "runsc version release-20260608.0";
const PINNED_EXPLICIT_USERNS_RUNSC_SHA256_HEX: &str =
    "4ec073363641a44cc5d171f63f1e23b76016ef632eb3269395c79ac8aecb71bc";

pub(super) fn sha256_hex_of_file(path: &Path) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = io::Read::read(&mut file, &mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn security_capability_xattr(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let path_c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|error| format!("{path:?} contains an interior NUL: {error}"))?;
    const SECURITY_CAPABILITY: &[u8] = b"security.capability\0";
    let result = unsafe {
        libc::lgetxattr(
            path_c.as_ptr(),
            SECURITY_CAPABILITY.as_ptr().cast(),
            std::ptr::null_mut(),
            0,
        )
    };
    if result >= 0 {
        let size = usize::try_from(result)
            .map_err(|_| format!("{path:?} security.capability size is unrepresentable"))?;
        if size > 64 {
            return Err(format!(
                "{path:?} security.capability xattr is unexpectedly large ({size} bytes)"
            ));
        }
        let mut value = vec![0u8; size];
        let read = unsafe {
            libc::lgetxattr(
                path_c.as_ptr(),
                SECURITY_CAPABILITY.as_ptr().cast(),
                value.as_mut_ptr().cast(),
                value.len(),
            )
        };
        if read < 0 {
            return Err(format!(
                "read {path:?} security.capability xattr: {}",
                io::Error::last_os_error()
            ));
        }
        let read = usize::try_from(read)
            .map_err(|_| format!("{path:?} security.capability read size is unrepresentable"))?;
        if read != size {
            return Err(format!(
                "{path:?} security.capability xattr changed size during validation"
            ));
        }
        return Ok(Some(value));
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ENODATA) || error.raw_os_error() == Some(libc::ENOTSUP) {
        Ok(None)
    } else {
        Err(format!("query {path:?} security.capability xattr: {error}"))
    }
}

pub(super) fn reject_security_capability_xattr(path: &Path) -> Result<(), String> {
    reject_security_capability_xattr_given(path, security_capability_xattr)
}

fn reject_security_capability_xattr_given(
    path: &Path,
    read_xattr: impl FnOnce(&Path) -> Result<Option<Vec<u8>>, String>,
) -> Result<(), String> {
    if read_xattr(path)?.is_some() {
        Err(format!(
            "{path:?} carries an unexpected security.capability xattr; the pinned runsc binary \
             must not acquire authority through file capabilities"
        ))
    } else {
        Ok(())
    }
}

fn verify_helper_security_capability_xattr(path: &Path, expected: &[u8]) -> Result<(), String> {
    match security_capability_xattr(path)? {
        None => Ok(()),
        Some(actual) if actual == expected => Ok(()),
        Some(_) => Err(format!(
            "{path:?} carries an unexpected security.capability xattr; only its exact \
             distro-provided helper capability is accepted"
        )),
    }
}

pub(super) fn verify_pinned_explicit_userns_runsc(bin: &Path) -> Result<(), String> {
    let digest = sha256_hex_of_file(bin).map_err(|e| format!("hash {bin:?}: {e}"))?;
    if digest != PINNED_EXPLICIT_USERNS_RUNSC_SHA256_HEX {
        return Err(format!(
            "{bin:?}'s content digest {digest} does not match the pinned \
             {PINNED_EXPLICIT_USERNS_RUNSC_SHA256_HEX} - refusing to execute a candidate that \
             hasn't already been proven byte-identical to the trusted build"
        ));
    }
    reject_security_capability_xattr(bin)?;
    let output = Command::new(bin)
        .arg("--version")
        .output()
        .map_err(|e| format!("{bin:?} --version: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "{bin:?} --version exited {:?} (expected success)",
            output.status.code()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let version_line = stdout.lines().next().unwrap_or("");
    if version_line != PINNED_EXPLICIT_USERNS_RUNSC_VERSION {
        return Err(format!(
            "{bin:?} reports {version_line:?}, but ExplicitUserNamespace mode is pinned to \
             exactly {PINNED_EXPLICIT_USERNS_RUNSC_VERSION:?}"
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedExplicitUsernsPolicy {
    pub(super) helper_dir: PathBuf,
    pub(super) runsc_root: PathBuf,
    pub(super) runsc_root_identity: (u64, u64),
}

impl ResolvedExplicitUsernsPolicy {
    pub(super) fn revalidated_root_identity(&self) -> Result<(u64, u64), String> {
        let current = verify_explicit_userns_runsc_root_leaf(&self.runsc_root)?;
        if current != self.runsc_root_identity {
            return Err(format!(
                "{:?} no longer names the same state root this policy was validated against \
                 (expected identity {:?}, found {current:?})",
                self.runsc_root, self.runsc_root_identity
            ));
        }
        Ok(current)
    }
}

static EXPLICIT_USERNS_POLICY: std::sync::OnceLock<ResolvedExplicitUsernsPolicy> =
    std::sync::OnceLock::new();

pub(crate) fn revalidated_explicit_userns_root_identity() -> Result<(u64, u64), String> {
    revalidated_explicit_userns_root_identity_given(EXPLICIT_USERNS_POLICY.get())
}

pub(super) fn revalidated_explicit_userns_root_identity_given(
    policy: Option<&ResolvedExplicitUsernsPolicy>,
) -> Result<(u64, u64), String> {
    policy
        .ok_or_else(|| {
            "ExplicitUserNamespace mode was never validated via preflight_explicit_userns_policy"
                .to_string()
        })?
        .revalidated_root_identity()
}

pub(super) fn harden_explicit_userns_runsc_binary(bin: &Path) -> Result<(), String> {
    if !bin.is_absolute() {
        return Err(format!("{bin:?} must be an absolute path"));
    }
    let meta = std::fs::symlink_metadata(bin).map_err(|e| format!("stat {bin:?}: {e}"))?;
    if meta.file_type().is_symlink() {
        return Err(format!("{bin:?} must not be a symlink"));
    }
    if !meta.is_file() {
        return Err(format!("{bin:?} must be a regular file"));
    }
    if meta.uid() != 0 {
        return Err(format!(
            "{bin:?} must be owned by root (uid 0), got uid {}",
            meta.uid()
        ));
    }
    if meta.mode() & 0o022 != 0 {
        return Err(format!(
            "{bin:?} must not be group/other-writable (mode {:o})",
            meta.mode() & 0o777
        ));
    }
    reject_security_capability_xattr(bin)?;
    crate::dirlock::verify_ancestors_not_writable_by_us(bin)
        .map_err(|reason| format!("{bin:?}'s ancestor chain is not safely anchored: {reason}"))
}

pub(super) fn harden_explicit_userns_runsc_root(dir: &Path) -> Result<(u64, u64), String> {
    if !dir.is_absolute() {
        return Err(format!("{dir:?} must be an absolute path"));
    }
    crate::dirlock::verify_ancestors_not_writable_by_us(dir)
        .map_err(|reason| format!("{dir:?}'s ancestor chain is not safely anchored: {reason}"))?;
    verify_explicit_userns_runsc_root_leaf(dir)
}

fn harden_local_development_runsc_root(dir: &Path) -> Result<(u64, u64), String> {
    if !dir.is_absolute() {
        return Err(format!("{dir:?} must be an absolute path"));
    }
    verify_explicit_userns_runsc_root_leaf(dir)
}

pub(super) fn verify_explicit_userns_runsc_root_leaf(dir: &Path) -> Result<(u64, u64), String> {
    let meta = std::fs::symlink_metadata(dir).map_err(|e| {
        format!(
            "stat {dir:?}: {e} - the explicit-userns runsc state root must be pre-provisioned; \
             this preflight does not create it"
        )
    })?;
    if meta.file_type().is_symlink() {
        return Err(format!("{dir:?} must not be a symlink"));
    }
    if !meta.is_dir() {
        return Err(format!("{dir:?} must be a directory"));
    }
    let our_uid = unsafe { libc::geteuid() };
    if meta.uid() != our_uid {
        return Err(format!(
            "{dir:?} is owned by uid {} (expected this process's own euid {our_uid})",
            meta.uid()
        ));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(format!(
            "{dir:?} mode {:o} is group/other-accessible - expected 0700 or stricter",
            meta.mode() & 0o777
        ));
    }
    if meta.mode() & 0o700 != 0o700 {
        return Err(format!(
            "{dir:?} mode {:o} does not grant this process's own owner bits full rwx - required \
             to create/search state under it",
            meta.mode() & 0o777
        ));
    }
    Ok((meta.dev(), meta.ino()))
}

pub fn preflight_explicit_userns_policy(
    helper_dir: &Path,
    runsc_root: &Path,
) -> Result<(), String> {
    preflight_explicit_userns_policy_given(
        helper_dir,
        runsc_root,
        harden_explicit_userns_runsc_root,
    )
}

/// Validates explicit-userns execution for a single-user local-development runner.
///
/// The runsc binary, helper binaries, and subordinate-ID sources keep their production checks.
/// The state root may live below a developer-owned private directory because that same developer
/// already controls the runner process and its environment.
pub fn preflight_local_development_explicit_userns_policy(
    helper_dir: &Path,
    runsc_root: &Path,
) -> Result<(), String> {
    preflight_explicit_userns_policy_given(
        helper_dir,
        runsc_root,
        harden_local_development_runsc_root,
    )
}

fn preflight_explicit_userns_policy_given(
    helper_dir: &Path,
    runsc_root: &Path,
    validate_runsc_root: impl FnOnce(&Path) -> Result<(u64, u64), String>,
) -> Result<(), String> {
    let bin = super::runsc_bin();
    harden_explicit_userns_runsc_binary(bin)?;
    verify_pinned_explicit_userns_runsc(bin)?;
    preflight_explicit_userns_helpers(helper_dir)?;
    let runsc_root_identity = validate_runsc_root(runsc_root)?;
    let policy = ResolvedExplicitUsernsPolicy {
        helper_dir: helper_dir.to_path_buf(),
        runsc_root: runsc_root.to_path_buf(),
        runsc_root_identity,
    };
    if let Err(rejected) = EXPLICIT_USERNS_POLICY.set(policy) {
        let Some(already) = EXPLICIT_USERNS_POLICY.get() else {
            return Err(
                "explicit-userns policy initialization raced without retaining either policy"
                    .into(),
            );
        };
        if already != &rejected {
            return Err(format!(
                "explicit-userns policy already installed as {already:?}, which disagrees with \
                 this preflight's {rejected:?} - refusing rather than leaving some callers on a \
                 stale policy"
            ));
        }
    }
    Ok(())
}

pub(super) fn apply_runsc_invocation_policy(
    cmd: &mut Command,
    bin: &Path,
    mode: RunscInvocationMode,
) -> Result<(), String> {
    apply_runsc_invocation_policy_checked_given(
        cmd,
        bin,
        mode,
        EXPLICIT_USERNS_POLICY.get(),
        reject_security_capability_xattr,
    )
}

pub(super) fn apply_runsc_invocation_policy_checked_given(
    cmd: &mut Command,
    bin: &Path,
    mode: RunscInvocationMode,
    policy: Option<&ResolvedExplicitUsernsPolicy>,
    reject_file_capabilities: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    reject_file_capabilities(bin)?;
    apply_runsc_invocation_policy_given(cmd, mode, policy)
}

pub(super) fn apply_runsc_invocation_policy_given(
    cmd: &mut Command,
    mode: RunscInvocationMode,
    policy: Option<&ResolvedExplicitUsernsPolicy>,
) -> Result<(), String> {
    match mode {
        RunscInvocationMode::Rootless => {
            cmd.arg("--rootless");
            Ok(())
        }
        RunscInvocationMode::ExplicitUserNamespace(_) => {
            let policy = policy.ok_or_else(|| {
                "ExplicitUserNamespace mode requires preflight_explicit_userns_policy to have \
                 succeeded first - refusing rather than falling back to unvalidated resolution"
                    .to_string()
            })?;
            apply_explicit_userns_env(cmd, policy);
            Ok(())
        }
    }
}

pub(super) fn apply_explicit_userns_env(cmd: &mut Command, policy: &ResolvedExplicitUsernsPolicy) {
    cmd.arg("-ignore-cgroups");
    cmd.arg(format!("--root={}", policy.runsc_root.display()));
    cmd.env_clear();
    cmd.env("PATH", &policy.helper_dir);
}

pub(super) fn delete_container(bin: &Path, container_id: &str, mode: RunscInvocationMode) {
    let mut cmd = Command::new(bin);
    if apply_runsc_invocation_policy(&mut cmd, bin, mode).is_err() {
        return;
    }
    let _ = cmd.arg("delete").arg("-force").arg(container_id).output();
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::process::Command;

    use crate::gvisor::unique_suffix;
    use crate::user_namespace::{RunscInvocationMode, UserNamespaceConfig};
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};

    #[test]
    fn every_rootless_runtime_invocation_rejects_an_unexpected_file_capability() {
        let bin = Path::new("/vetted/runsc");
        let mut cmd = Command::new(bin);
        let result = apply_runsc_invocation_policy_checked_given(
            &mut cmd,
            bin,
            RunscInvocationMode::Rootless,
            None,
            |path| {
                Err(format!(
                    "{path:?} carries an unexpected security.capability xattr"
                ))
            },
        );
        assert!(result
            .unwrap_err()
            .contains("unexpected security.capability xattr"));
        assert_eq!(
            cmd.get_args().count(),
            0,
            "rejection must happen before even the rootless invocation policy is assembled"
        );
    }

    #[test]
    fn apply_runsc_invocation_policy_matches_the_mode_exactly() {
        let bin = Path::new("/bin/true");
        let mut rootless_cmd = Command::new(bin);
        apply_runsc_invocation_policy(&mut rootless_cmd, bin, RunscInvocationMode::Rootless)
            .unwrap();
        assert_eq!(
            rootless_cmd
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["--rootless"],
            "Rootless must be byte-identical to the pre-slice-2 flag"
        );
        assert_eq!(
            rootless_cmd.get_envs().count(),
            0,
            "Rootless must not alter the child's environment at all"
        );
    }

    #[test]
    fn apply_explicit_userns_env_matches_the_policy_exactly() {
        let policy = ResolvedExplicitUsernsPolicy {
            helper_dir: PathBuf::from("/usr/bin"),
            runsc_root: PathBuf::from("/var/lib/myelin-runsc-explicit-userns"),
            runsc_root_identity: (0, 0),
        };
        let mut cmd = Command::new("runsc");
        apply_explicit_userns_env(&mut cmd, &policy);
        let args = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(
            args.contains(&"-ignore-cgroups".to_string()),
            "ExplicitUserNamespace must add -ignore-cgroups: {args:?}"
        );
        assert!(
            !args.contains(&"--rootless".to_string()),
            "ExplicitUserNamespace must drop --rootless: {args:?}"
        );
        assert!(
            args.contains(&"--root=/var/lib/myelin-runsc-explicit-userns".to_string()),
            "ExplicitUserNamespace must pass the exact policy's absolute --root=: {args:?}"
        );
        let envs: Vec<_> = cmd
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert_eq!(
            envs,
            vec![("PATH".to_string(), Some("/usr/bin".to_string()))],
            "ExplicitUserNamespace must clear the environment and set PATH to ONLY the exact \
             policy's helper directory: {envs:?}"
        );
    }

    #[test]
    fn apply_runsc_invocation_policy_refuses_explicit_userns_without_a_validated_policy() {
        let mut cmd = Command::new("runsc");
        let result = apply_runsc_invocation_policy_given(
            &mut cmd,
            RunscInvocationMode::ExplicitUserNamespace(UserNamespaceConfig::for_tests(
                1000, 1000, 100_000, 200_000,
            )),
            None,
        );
        assert!(
            result.is_err(),
            "ExplicitUserNamespace must refuse without a validated policy, not silently proceed"
        );
    }

    #[test]
    fn preflight_explicit_userns_helpers_accepts_real_and_rejects_a_non_setuid_substitute() {
        let real = Path::new("/usr/bin");
        if !real.join("newuidmap").exists() || !real.join("newgidmap").exists() {
            eprintln!("skipping: this host has no /usr/bin/newuidmap or newgidmap");
            return;
        }
        preflight_explicit_userns_helpers(real)
            .expect("this host's real /usr/bin must pass preflight");

        use std::os::unix::fs::PermissionsExt;
        let tmp =
            std::env::temp_dir().join(format!("myelin-preflight-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        for helper in ["newuidmap", "newgidmap"] {
            std::fs::write(tmp.join(helper), b"#!/bin/sh\nexit 1\n").unwrap();
            let mut perms = std::fs::metadata(tmp.join(helper)).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(tmp.join(helper), perms).unwrap();
        }
        let result = preflight_explicit_userns_helpers(&tmp);
        std::fs::remove_dir_all(&tmp).ok();
        assert!(
            result.is_err(),
            "a non-root-owned, non-setuid substitute must be refused"
        );
    }

    #[test]
    fn sha256_hex_of_file_matches_a_known_vector() {
        let tmp = std::env::temp_dir().join(format!("myelin-sha256-test-{}", unique_suffix()));
        std::fs::write(&tmp, b"abc").unwrap();
        let digest = sha256_hex_of_file(&tmp).unwrap();
        std::fs::remove_file(&tmp).ok();
        assert_eq!(
            digest, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "must match the well-known SHA-256(\"abc\") test vector"
        );
    }

    #[test]
    fn verify_pinned_explicit_userns_runsc_rejects_a_forged_version_string_with_wrong_content() {
        let tmp = std::env::temp_dir().join(format!("myelin-forged-runsc-{}", unique_suffix()));
        std::fs::write(
            &tmp,
            format!("#!/bin/sh\necho '{PINNED_EXPLICIT_USERNS_RUNSC_VERSION}'\n"),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&tmp).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o755);
        std::fs::set_permissions(&tmp, perms).unwrap();
        let result = verify_pinned_explicit_userns_runsc(&tmp);
        std::fs::remove_file(&tmp).ok();
        assert!(
            result.is_err(),
            "a forged version string with the wrong content digest must be refused: {result:?}"
        );
    }

    #[test]
    fn harden_explicit_userns_runsc_binary_refuses_a_non_root_owned_file() {
        let tmp = std::env::temp_dir().join(format!("myelin-fake-runsc-{}", unique_suffix()));
        std::fs::write(&tmp, b"#!/bin/sh\nexit 0\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&tmp).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&tmp, perms).unwrap();
        let result = harden_explicit_userns_runsc_binary(&tmp);
        std::fs::remove_file(&tmp).ok();
        assert!(
            result.is_err(),
            "a binary owned by this process's own euid must be refused: {result:?}"
        );
    }

    #[test]
    fn harden_explicit_userns_runsc_binary_refuses_a_symlink() {
        let base =
            std::env::temp_dir().join(format!("myelin-fake-runsc-symlink-{}", unique_suffix()));
        std::fs::create_dir_all(&base).unwrap();
        let real = base.join("real-runsc");
        std::fs::write(&real, b"#!/bin/sh\nexit 0\n").unwrap();
        let link = base.join("runsc");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let result = harden_explicit_userns_runsc_binary(&link);
        let _ = std::fs::remove_dir_all(&base);
        assert!(
            result.is_err(),
            "a symlinked binary path must be refused rather than followed: {result:?}"
        );
    }

    #[test]
    fn harden_explicit_userns_runsc_root_refuses_a_leaf_under_a_writable_parent() {
        let base = std::env::temp_dir().join(format!(
            "myelin-runsc-root-writable-parent-{}",
            unique_suffix()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let leaf = base.join("runsc-root");
        let result = harden_explicit_userns_runsc_root(&leaf);
        let _ = std::fs::remove_dir_all(&base);
        assert!(
            result.is_err(),
            "a leaf whose parent is writable by this process must be refused: {result:?}"
        );
    }

    #[test]
    fn verify_explicit_userns_runsc_root_leaf_refuses_a_missing_leaf() {
        let missing =
            std::env::temp_dir().join(format!("myelin-missing-runsc-root-{}", unique_suffix()));
        let result = verify_explicit_userns_runsc_root_leaf(&missing);
        assert!(
            result.is_err(),
            "a non-pre-provisioned leaf must be refused, never auto-created: {result:?}"
        );
    }

    #[test]
    fn verify_explicit_userns_runsc_root_leaf_refuses_a_symlinked_leaf() {
        let base =
            std::env::temp_dir().join(format!("myelin-runsc-root-symlink-{}", unique_suffix()));
        std::fs::create_dir_all(&base).unwrap();
        let real_dir = base.join("real");
        std::fs::create_dir_all(&real_dir).unwrap();
        let link = base.join("runsc-root");
        std::os::unix::fs::symlink(&real_dir, &link).unwrap();
        let result = verify_explicit_userns_runsc_root_leaf(&link);
        let _ = std::fs::remove_dir_all(&base);
        assert!(
            result.is_err(),
            "a symlinked state-root leaf must be refused rather than followed: {result:?}"
        );
    }

    #[test]
    fn verify_explicit_userns_runsc_root_leaf_refuses_an_owner_non_writable_directory() {
        let dir = std::env::temp_dir().join(format!("myelin-runsc-root-0500-{}", unique_suffix()));
        std::fs::create_dir_all(&dir).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dir).unwrap().permissions();
        perms.set_mode(0o500);
        std::fs::set_permissions(&dir, perms).unwrap();
        let result = verify_explicit_userns_runsc_root_leaf(&dir);
        let mut restore = std::fs::metadata(&dir).unwrap().permissions();
        restore.set_mode(0o700);
        std::fs::set_permissions(&dir, restore).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            result.is_err(),
            "an owner-non-writable directory must be refused even with no group/other bits: \
             {result:?}"
        );
    }

    #[test]
    fn verify_explicit_userns_runsc_root_leaf_accepts_a_properly_pre_provisioned_leaf() {
        let dir = std::env::temp_dir().join(format!("myelin-runsc-root-ok-{}", unique_suffix()));
        std::fs::create_dir_all(&dir).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dir).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&dir, perms).unwrap();
        let result = verify_explicit_userns_runsc_root_leaf(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            result.is_ok(),
            "a real, owned, mode-0700 pre-provisioned directory must be accepted: {result:?}"
        );
    }

    #[test]
    fn resolved_explicit_userns_policy_revalidates_a_matching_identity() {
        let dir = std::env::temp_dir().join(format!(
            "myelin-gvisor-userns-root-identity-ok-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dir).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&dir, perms).unwrap();
        let meta = std::fs::metadata(&dir).unwrap();
        let identity = (meta.dev(), meta.ino());
        let policy = ResolvedExplicitUsernsPolicy {
            helper_dir: PathBuf::from("/usr/bin"),
            runsc_root: dir.clone(),
            runsc_root_identity: identity,
        };
        assert_eq!(policy.revalidated_root_identity(), Ok(identity));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolved_explicit_userns_policy_refuses_a_mode_drift_without_replacement() {
        let dir = std::env::temp_dir().join(format!(
            "myelin-gvisor-userns-root-identity-drift-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dir).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&dir, perms).unwrap();
        let meta = std::fs::metadata(&dir).unwrap();
        let identity = (meta.dev(), meta.ino());
        let policy = ResolvedExplicitUsernsPolicy {
            helper_dir: PathBuf::from("/usr/bin"),
            runsc_root: dir.clone(),
            runsc_root_identity: identity,
        };
        let mut drifted = std::fs::metadata(&dir).unwrap().permissions();
        drifted.set_mode(0o777);
        std::fs::set_permissions(&dir, drifted).unwrap();
        let result = policy.revalidated_root_identity();
        let mut restore = std::fs::metadata(&dir).unwrap().permissions();
        restore.set_mode(0o700);
        std::fs::set_permissions(&dir, restore).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            result.is_err(),
            "a same-inode mode drift (0700 -> 0777) must be refused, not just identity-compared: \
             {result:?}"
        );
    }

    #[test]
    fn resolved_explicit_userns_policy_refuses_a_replaced_root() {
        let dir = std::env::temp_dir().join(format!(
            "myelin-gvisor-userns-root-identity-replaced-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let moved_aside = std::env::temp_dir().join(format!(
            "myelin-gvisor-userns-root-identity-replaced-original-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dir).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&dir, perms).unwrap();
        let meta = std::fs::metadata(&dir).unwrap();
        let original_identity = (meta.dev(), meta.ino());
        let policy = ResolvedExplicitUsernsPolicy {
            helper_dir: PathBuf::from("/usr/bin"),
            runsc_root: dir.clone(),
            runsc_root_identity: original_identity,
        };
        std::fs::rename(&dir, &moved_aside).unwrap();
        std::fs::create_dir(&dir).unwrap();
        let mut fresh_perms = std::fs::metadata(&dir).unwrap().permissions();
        fresh_perms.set_mode(0o700);
        std::fs::set_permissions(&dir, fresh_perms).unwrap();
        let fresh_meta = std::fs::metadata(&dir).unwrap();
        let fresh_identity = (fresh_meta.dev(), fresh_meta.ino());
        assert_ne!(
            original_identity, fresh_identity,
            "the fixture must produce a genuinely different inode, not rely on chance"
        );
        assert!(policy.revalidated_root_identity().is_err());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&moved_aside);
    }

    #[test]
    fn revalidated_explicit_userns_root_identity_given_refuses_without_a_policy() {
        let result = revalidated_explicit_userns_root_identity_given(None);
        assert!(matches!(result, Err(ref reason) if reason.contains("never validated")));
    }
}
