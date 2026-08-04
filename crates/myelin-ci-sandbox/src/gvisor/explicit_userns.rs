//! Explicit-user-namespace `runsc` invocation policy: boot-time hardening and pinning of
//! the `runsc` binary, its `newuidmap`/`newgidmap` helpers, and its state root, plus the
//! per-invocation global-flag/environment policy applied to every production run/kill/delete.
//! Extracted verbatim from `gvisor.rs` (pure move); the call sites live there.

use crate::user_namespace::RunscInvocationMode;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The `runsc` GLOBAL flags (BEFORE the subcommand) `mode` implies — the ONE place any of
/// `run`/`kill`/`delete` decides this, so no call site makes an independent flag decision (CT-007
/// slice 2). `Rootless` is byte-identical to the pre-slice-2 flag (just `--rootless`).
/// `ExplicitUserNamespace` drops `--rootless` and adds `-ignore-cgroups` — confirmed by a live
/// spike against the pinned `runsc` build that dropping `--rootless` surfaces a REAL cgroup-setup
/// requirement `runsc`'s own cgroupfs manager cannot satisfy without root (even under a cgroup
/// path nested entirely under this process's own delegated slice); `-ignore-cgroups` makes it skip
/// that internal management entirely WITHOUT weakening [`MemoryCgroup`], which places the spawned
/// `runsc` process into a real, already-owned cgroup externally, independent of this flag.
/// Env var naming the directory `runsc` resolves `newuidmap`/`newgidmap` through, under
/// [`RunscInvocationMode::ExplicitUserNamespace`] — the ONLY entry in the (otherwise cleared)
/// `PATH` that mode's `runsc` invocation sees. Defaults to `/usr/bin` (where this host's real
/// setuid helpers live); a production deployment SHOULD point this at a dedicated, curated
/// directory containing ONLY the two validated helpers (Sol's review) — [`preflight_explicit_userns_helpers`]
/// validates whatever directory is actually configured, it does not require it to be minimal.
pub const ENV_EXPLICIT_USERNS_HELPER_DIR: &str = "MYELIN_EXPLICIT_USERNS_HELPER_DIR";

/// Resolved ONCE and cached (Sol's review, round 3: re-reading the env var inside every
/// `run`/`kill`/`delete` call meant an environment mutation mid-process could launch a container
/// under one helper directory and later kill/delete it under a DIFFERENT one — caching makes "one
/// resolved value for the whole process" an actual invariant). Not itself validated (this is a
/// plain resolver, matching [`resolved_explicit_userns_runsc_root`]'s role) — a caller enabling
/// [`RunscInvocationMode::ExplicitUserNamespace`] in production reads this value and passes it to
/// [`preflight_explicit_userns_policy`] once at startup (mirroring how [`preflight_gvisor_runner_host`]'s
/// own caller resolves `MYELIN_RUNSC_BIN` itself before calling in).
pub fn resolved_explicit_userns_helper_dir() -> &'static Path {
    static CACHED: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    CACHED.get_or_init(|| {
        std::env::var(ENV_EXPLICIT_USERNS_HELPER_DIR)
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/usr/bin"))
    })
}

/// Env var naming the `runsc` state-root directory used ONLY under
/// [`RunscInvocationMode::ExplicitUserNamespace`] — passed explicitly (`--root=<path>`) so
/// container-state lookup never depends on `$XDG_RUNTIME_DIR` (cleared, along with the rest of
/// the environment, for this mode — Sol's review: clearing the environment without ALSO fixing
/// `--root` could make startup fail or state lookup diverge from whatever `runsc`'s own default
/// resolution would have picked).
pub const ENV_EXPLICIT_USERNS_RUNSC_ROOT: &str = "MYELIN_EXPLICIT_USERNS_RUNSC_ROOT";

/// Resolved ONCE and cached (Sol's review, round 3: a relative `--root=` would resolve against
/// `runsc`'s own current working directory at spawn time, which this process does not control — a
/// state root that silently moved between launches would fragment container-state lookup). A
/// relative configured value is joined onto this process's current directory AT THE MOMENT OF
/// FIRST RESOLUTION, making the RESULT absolute in the ordinary case — but this resolver alone does
/// NOT guarantee absoluteness (if `current_dir()` itself fails, the relative value is returned
/// as-is; round 4's doc comment overclaimed this). What actually enforces absoluteness is
/// [`preflight_explicit_userns_policy`], which explicitly refuses a non-absolute `runsc_root`
/// before ever installing it into [`EXPLICIT_USERNS_POLICY`] — this resolver is a best-effort
/// convenience the caller feeds INTO that real gate, not the gate itself.
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

/// Boot preflight for [`RunscInvocationMode::ExplicitUserNamespace`] (mirrors
/// [`preflight_gvisor_runner_host`]'s role for the base runtime): verify `helper_dir` is an
/// absolute, non-symlinked, root-owned, not-group/other-writable directory whose own ANCESTOR
/// chain this process cannot rename/replace (Sol's review, round 3: an earlier version used
/// `std::fs::metadata`, which FOLLOWS a symlink at `helper_dir` itself, silently validating
/// whatever the symlink pointed at instead of refusing it — fixed by checking
/// `symlink_metadata` first), and that it contains `newuidmap`/`newgidmap` as regular, root-owned,
/// setuid files, never group/other-writable, and executable BY THIS PROCESS'S OWN EFFECTIVE
/// IDENTITY specifically (checked via the kernel's own `faccessat(..., AT_EACCESS)` rather than
/// "some execute bit is set somewhere," which could pass on a root-only-executable file this
/// process could never actually run) — the trust chain `runsc`'s own internal resolution depends
/// on once `PATH` is fixed to exactly this directory. Never called automatically; a caller enabling
/// `ExplicitUserNamespace` mode in production calls this once at startup.
pub fn preflight_explicit_userns_helpers(helper_dir: &Path) -> Result<(), String> {
    if !helper_dir.is_absolute() {
        return Err(format!("{helper_dir:?} must be an absolute path"));
    }
    // `symlink_metadata` (NOT `metadata`) so a symlinked `helper_dir` is refused outright rather
    // than transparently validating whatever it points at.
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
    for helper in ["newuidmap", "newgidmap"] {
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
        // Some distributions ship these already-setuid helpers with the corresponding single file
        // capability as defense in depth. Accept absence or that one exact v2 xattr; reject every
        // extra bit/set/encoding so a helper cannot add unrelated authority at exec.
        const NEWUIDMAP_CAP_SETUID_EP: &[u8] =
            b"\x01\x00\x00\x02\x80\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";
        const NEWGIDMAP_CAP_SETGID_EP: &[u8] =
            b"\x01\x00\x00\x02\x40\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";
        let expected_file_capability = match helper {
            "newuidmap" => NEWUIDMAP_CAP_SETUID_EP,
            "newgidmap" => NEWGIDMAP_CAP_SETGID_EP,
            _ => unreachable!("the helper list above is closed"),
        };
        verify_helper_security_capability_xattr(&path, expected_file_capability)?;
        let path_c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
            .map_err(|e| format!("{path:?} contains an interior NUL: {e}"))?;
        // SAFETY: `path_c` is a valid, NUL-terminated path; `faccessat` only queries permission
        // bits, it never mutates anything. `AT_EACCESS` checks this process's EFFECTIVE identity
        // (matching the identity `runsc` — spawned by this same process — will actually run as),
        // rather than "does some execute bit exist," which could pass for a root-only-executable
        // file this process could never actually invoke.
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

/// The runsc release this slice's `ExplicitUserNamespace` OCI/CLI contract (multi-ID `uidMappings`/
/// `gidMappings`, `-ignore-cgroups`, explicit `--root=`) was actually validated against (the live
/// spike + every drill run in this repo's own development). Sol's review, round 4: the new
/// contract is "explicitly justified and accepted against that release" specifically, not against
/// "whatever identifies itself as runsc" — pin the exact version string AND the binary's own
/// content digest, rather than accepting a same-named-but-different build.
pub(super) const PINNED_EXPLICIT_USERNS_RUNSC_VERSION: &str = "runsc version release-20260608.0";
/// SHA-256 of the exact `runsc` binary this repo's `ExplicitUserNamespace` contract was validated
/// against (computed once, off this development host's own pinned install).
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

/// Read a host executable's file capability under a tiny size bound. Content hashing does not cover
/// xattrs, but a file capability participates in `execve`'s capability calculation.
fn security_capability_xattr(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let path_c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|error| format!("{path:?} contains an interior NUL: {error}"))?;
    const SECURITY_CAPABILITY: &[u8] = b"security.capability\0";
    // SAFETY: both pointers name live NUL-terminated byte strings. A null value pointer and zero
    // size make lgetxattr a read-only size/absence query, and using lgetxattr avoids following a
    // leaf symlink (which the surrounding hardening rejects independently).
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
        // SAFETY: the same live NUL-terminated strings are used, and `value` has exactly the size
        // returned by the read-only probe. A concurrent size change fails closed below.
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

/// Injectable decision core for the file-capability rejection. Keeping the policy separate from
/// Linux's privileged xattr setup lets an ordinary unprivileged unit test prove that an unexpected
/// value rejects before execution, while production always supplies the real no-follow reader.
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

/// Verify `bin` is EXACTLY the runsc release+build this slice's `ExplicitUserNamespace` contract
/// was validated against — both the binary's own content digest AND the reported version string.
/// Sol's review, round 6: HASH FIRST, EXECUTE ONLY AFTER the digest matches — the previous version
/// ran `bin --version` before ever checking the digest, meaning ANY candidate at `bin` (forged,
/// corrupted, or attacker-planted) got arbitrary host execution before this function could ever
/// reject it. Hashing first means a candidate that doesn't match the pinned digest is NEVER
/// executed at all. This function alone does not close the TOCTOU between the hash-read and the
/// `--version` exec a moment later — that gap is closed by requiring the CALLER to have already
/// established the path's immutability (via [`harden_explicit_userns_runsc_binary`]) before this
/// function ever runs, so nothing could swap the file's content in between.
pub(super) fn verify_pinned_explicit_userns_runsc(bin: &Path) -> Result<(), String> {
    let digest = sha256_hex_of_file(bin).map_err(|e| format!("hash {bin:?}: {e}"))?;
    if digest != PINNED_EXPLICIT_USERNS_RUNSC_SHA256_HEX {
        return Err(format!(
            "{bin:?}'s content digest {digest} does not match the pinned \
             {PINNED_EXPLICIT_USERNS_RUNSC_SHA256_HEX} — refusing to execute a candidate that \
             hasn't already been proven byte-identical to the trusted build"
        ));
    }
    // Keep this direct `--version` exec independently fail-closed even if a future caller forgets
    // the surrounding explicit-userns hardening order.
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

/// The fully resolved, atomically-validated set of values `ExplicitUserNamespace` mode's `runsc`
/// invocation depends on (Sol's review, round 4: three independently-cached `OnceLock`s do not
/// bind VALIDATION to USE — a caller could validate directory B while a stale, independently
/// resolved cache still points at directory A). Installed ONCE, as a single unit, only after every
/// field has been checked TOGETHER — there is no way to observe a partially-validated policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedExplicitUsernsPolicy {
    pub(super) helper_dir: PathBuf,
    pub(super) runsc_root: PathBuf,
    /// CT-007 slice 3, piece 7a: `runsc_root`'s own (device, inode), captured at the moment this
    /// policy was validated — the identity a [`crate::user_namespace::UserNamespaceLease`] binds
    /// to before `runsc` ever execs in `ExplicitUserNamespace` mode, and the identity a completed
    /// run's teardown must reconfirm before trusting that the SAME state root is still in play.
    pub(super) runsc_root_identity: (u64, u64),
}

impl ResolvedExplicitUsernsPolicy {
    /// Re-run the FULL leaf hardening check ([`verify_explicit_userns_runsc_root_leaf`]) against
    /// `runsc_root` NOW, and confirm the identity it returns still matches what was captured at
    /// preflight time — a stale-handle consistency check (matching this crate's established
    /// pattern for `AbsoluteRootfs`/`cgroup_identity`/`classify_identity_check`), not full TOCTOU
    /// protection: this policy's own `runsc_root`/resolved `runsc` binary were already hardened
    /// against untrusted REPLACEMENT by [`harden_explicit_userns_runsc_root`]/
    /// [`harden_explicit_userns_runsc_binary`] at preflight time.
    ///
    /// Deliberately re-runs the WHOLE leaf check, not a bare `stat` for `(dev, ino)` alone (Sol's
    /// round-1 review of piece 7a): the leaf is owned by THIS process's own euid, so its MODE can
    /// silently drift out from under an unchanged inode (e.g. `chmod 0700 -> 0777` on the exact
    /// same directory) without dev/ino ever disagreeing — ancestor hardening prevents an untrusted
    /// same-euid replacement, and identity comparison alone catches a replacement whose identity
    /// genuinely differs, but neither catches a mode drift on the SAME inode. A bare
    /// identity check would keep returning `Ok` for a state root that no longer satisfies the
    /// security posture `verify_explicit_userns_runsc_root_leaf` exists to enforce. Re-running the
    /// full check makes "the identity still matches" also mean "the hardening still holds" — the
    /// ONE call this module ever makes to confirm either.
    ///
    /// Called immediately before durably binding a lease to this identity, and again during
    /// checked runtime retirement, so the same value backs both ends of one run's proof.
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

/// CT-007 slice 3, piece 7a: the ONE way any future caller obtains a freshly-revalidated
/// `runsc_root_identity` to bind a lease to — refuses if `ExplicitUserNamespace` mode was never
/// validated via [`preflight_explicit_userns_policy`], or if the state root no longer matches what
/// was validated. `pub(crate)`, not yet called by any production path — that's piece 7c.
pub(crate) fn revalidated_explicit_userns_root_identity() -> Result<(u64, u64), String> {
    revalidated_explicit_userns_root_identity_given(EXPLICIT_USERNS_POLICY.get())
}

/// The actual decision logic behind [`revalidated_explicit_userns_root_identity`], taking the
/// installed policy as an EXPLICIT `Option` parameter — mirroring
/// [`apply_runsc_invocation_policy_given`]'s exact same reasoning: reading the real
/// process-global `OnceLock` directly would make a "no policy installed yet" test
/// non-deterministic (once ANY test in the same process installs one, it stays installed for
/// every other test sharing that process).
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

/// Verify `bin` (the runsc binary `ExplicitUserNamespace` mode will execute) cannot be replaced
/// between THIS preflight and any later `run`/`kill`/`delete` — a real, non-symlinked, root-owned,
/// non-group/other-writable regular file, whose FULL ancestor chain is neither owned nor writable
/// by this process (Sol's review, round 5: the version+digest pin alone only proves what WAS true
/// AT preflight time via a path-based open+hash — a runner-writable binary, or a runner-writable
/// ANCESTOR of it, could still be replaced before or between any later invocation, which would
/// silently execute the replacement despite the installed policy claiming a validated binary).
/// Reuses the exact same ancestor-walk [`crate::user_namespace`]'s leases directory relies on.
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

/// This module's own hardening policy for the `runsc` explicit-userns state root. Sol's review,
/// round 6: the earlier version auto-created the leaf with `create_dir_all` before checking
/// anything — which is INTERNALLY CONTRADICTORY with the ancestor-writability requirement below,
/// since creating a missing leaf REQUIRES write access to its parent, meaning "auto-create
/// succeeded" and "the parent chain is safely non-writable by us" can never BOTH be true at once.
/// It also meant a FAILED preflight could still leave a freshly-created directory behind as a side
/// effect. Fixed by performing NO MUTATION at all: verifies the ancestor chain FIRST (so an unsafe
/// deployment is rejected before even looking at the leaf), then requires the leaf to ALREADY
/// EXIST as a real (non-symlink) directory, owned by this process's own euid, with a private mode
/// (`0700` or stricter) — pre-provisioning the leaf is now the CALLER's responsibility (a real
/// deployment's install step, or a test fixture), exactly mirroring the split
/// [`crate::user_namespace`]'s own leases-directory hardening now uses between its strict
/// production path and non-strict test setup.
pub(super) fn harden_explicit_userns_runsc_root(dir: &Path) -> Result<(u64, u64), String> {
    if !dir.is_absolute() {
        return Err(format!("{dir:?} must be an absolute path"));
    }
    crate::dirlock::verify_ancestors_not_writable_by_us(dir)
        .map_err(|reason| format!("{dir:?}'s ancestor chain is not safely anchored: {reason}"))?;
    verify_explicit_userns_runsc_root_leaf(dir)
}

/// The LEAF-only checks [`harden_explicit_userns_runsc_root`] applies, pulled out into its own
/// function so a test can exercise them directly against a fixture whose ANCESTORS are not
/// necessarily hardened (the full function's own ancestor check would otherwise refuse first
/// against any fixture a non-privileged test creates under a writable temp directory, proving
/// nothing about the leaf-specific checks this function targets). Returns the leaf's own
/// (device, inode) — CT-007 slice 3, piece 7a (Sol's round-1 review): this is the ONE `stat` this
/// module ever performs against the leaf, so the identity a caller later revalidates against is
/// GUARANTEED to have come from a check that also confirmed ownership/mode at that exact moment —
/// a separate, independent `metadata()` call for identity alone could observe dev/ino unchanged
/// while the leaf's mode had already drifted (e.g. `0700` chmod'd to `0777` — the leaf is owned by
/// THIS process, so only mode/ownership can drift under us, never a swap; a swap would need a NEW
/// inode, which dev/ino alone already catches). Re-running this SAME function later — not merely
/// re-`stat`ing — is what makes "the identity still matches" also mean "the hardening still holds".
pub(super) fn verify_explicit_userns_runsc_root_leaf(dir: &Path) -> Result<(u64, u64), String> {
    let meta = std::fs::symlink_metadata(dir).map_err(|e| {
        format!(
            "stat {dir:?}: {e} — the explicit-userns runsc state root must be pre-provisioned; \
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
            "{dir:?} mode {:o} is group/other-accessible — expected 0700 or stricter",
            meta.mode() & 0o777
        ));
    }
    // Sol's review, round 7: rejecting group/other bits alone still admits `0500`/`0000` — modes
    // this process itself could never actually write into (state-marker creation) or search
    // through. The owner must retain full `rwx`.
    if meta.mode() & 0o700 != 0o700 {
        return Err(format!(
            "{dir:?} mode {:o} does not grant this process's own owner bits full rwx — required \
             to create/search state under it",
            meta.mode() & 0o777
        ));
    }
    Ok((meta.dev(), meta.ino()))
}

/// Validate `helper_dir` (via [`preflight_explicit_userns_helpers`]), harden+validate `runsc_root`
/// (via [`harden_explicit_userns_runsc_root`]), and validate the currently-resolved `runsc` binary
/// ([`runsc_bin`]) both against the exact pinned release+digest this contract was accepted against
/// ([`verify_pinned_explicit_userns_runsc`]) AND against replacement
/// ([`harden_explicit_userns_runsc_binary`]) — then atomically install all of it as the ONE
/// [`ResolvedExplicitUsernsPolicy`] [`apply_runsc_invocation_policy`]'s `ExplicitUserNamespace`
/// branch will use for the rest of this process's lifetime. Never called automatically; a caller
/// enabling `ExplicitUserNamespace` mode in production calls this once at startup —
/// `apply_runsc_invocation_policy` REFUSES that mode outright (rather than falling back to ad hoc
/// unvalidated resolution) if this was never called or never succeeded.
pub fn preflight_explicit_userns_policy(
    helper_dir: &Path,
    runsc_root: &Path,
) -> Result<(), String> {
    let bin = super::runsc_bin();
    // Order matters (Sol's review, round 6): harden the PATHNAME first (no execution at all in
    // this step) so the file cannot be swapped out from under us — only THEN does
    // `verify_pinned_explicit_userns_runsc` hash and (only on a matching digest) execute it.
    harden_explicit_userns_runsc_binary(bin)?;
    verify_pinned_explicit_userns_runsc(bin)?;
    preflight_explicit_userns_helpers(helper_dir)?;
    // The identity comes from THIS SAME hardening check's own `stat` (Sol's round-1 review of
    // piece 7a) — never a separate, independent `metadata()` call, which could observe dev/ino
    // alone without re-confirming ownership/mode at that exact moment.
    let runsc_root_identity = harden_explicit_userns_runsc_root(runsc_root)?;
    let policy = ResolvedExplicitUsernsPolicy {
        helper_dir: helper_dir.to_path_buf(),
        runsc_root: runsc_root.to_path_buf(),
        runsc_root_identity,
    };
    if EXPLICIT_USERNS_POLICY.set(policy.clone()).is_err() {
        let already = EXPLICIT_USERNS_POLICY
            .get()
            .expect("set() just failed, so the cell must already be initialized");
        if already != &policy {
            return Err(format!(
                "explicit-userns policy already installed as {already:?}, which disagrees with \
                 this preflight's {policy:?} — refusing rather than leaving some callers on a \
                 stale policy"
            ));
        }
    }
    Ok(())
}

/// Apply the COMPLETE `runsc` invocation policy for `mode` to `cmd` — the ONE place `run`/`kill`/
/// `delete` decide BOTH the global flags AND the environment, so no call site makes an
/// independent decision (Sol's review). `Rootless` is BYTE-IDENTICAL to the pre-slice-2 behavior
/// (only `--rootless`; the inherited environment is untouched). `ExplicitUserNamespace` REFUSES
/// outright unless [`preflight_explicit_userns_policy`] has already succeeded (Sol's review, round
/// 4: binding validation to use, not merely resolving a value that happens to usually agree with
/// what was validated) — otherwise drops `--rootless`, adds `-ignore-cgroups` and an absolute
/// `--root=<state-root>` (never depending on `$XDG_RUNTIME_DIR`), clears the ENTIRE inherited
/// environment, and sets `PATH` to name ONLY the trusted helper directory `runsc` resolves
/// `newuidmap`/`newgidmap` through internally (per the live spike + gVisor's own docs: OCI-native
/// multi-ID mappings make `runsc` itself invoke these helpers — this process never does — so the
/// only lever we have is WHERE `runsc`'s own lookup can find them).
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
    // This wrapper is the common pre-exec gate for production run/kill/delete invocations. Check
    // the actual runtime path separately from `cmd.get_program()`: fenced launches execute
    // `/bin/sh` as the durable gate and pass `bin` as its eventual `exec "$@"` target.
    reject_file_capabilities(bin)?;
    apply_runsc_invocation_policy_given(cmd, mode, policy)
}

/// The actual decision logic behind [`apply_runsc_invocation_policy`], taking the installed policy
/// as an EXPLICIT `Option` parameter rather than reading the process-global `OnceLock` itself. Pulled
/// out so a test can deterministically prove the "no policy installed yet" refusal by passing `None`
/// directly — reading the real global would be ordering-dependent (once ANY test in the same test
/// binary's process installs a policy, it stays installed for every other test sharing that
/// process; Sol's review, round 5, flagged the previous test's silent skip-on-wrong-ordering as
/// non-deterministic).
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
                 succeeded first — refusing rather than falling back to unvalidated resolution"
                    .to_string()
            })?;
            apply_explicit_userns_env(cmd, policy);
            Ok(())
        }
    }
}

/// The pure `Command` mutation `ExplicitUserNamespace` mode applies, GIVEN an already-validated
/// [`ResolvedExplicitUsernsPolicy`]. Factored out of [`apply_runsc_invocation_policy`] so a test can
/// exercise this mechanism directly against a hand-built policy value, without depending on the
/// process-global [`EXPLICIT_USERNS_POLICY`] `OnceLock` (which, once set by any test in the same
/// test binary, stays set for every other test sharing that process) or on a real pinned `runsc`
/// binary being present to satisfy [`preflight_explicit_userns_policy`]'s digest check.
pub(super) fn apply_explicit_userns_env(cmd: &mut Command, policy: &ResolvedExplicitUsernsPolicy) {
    cmd.arg("-ignore-cgroups");
    cmd.arg(format!("--root={}", policy.runsc_root.display()));
    cmd.env_clear();
    cmd.env("PATH", &policy.helper_dir);
}

/// Best-effort idempotent container delete (`runsc <mode's global args> delete -force <cid>`).
/// Deleting an already-gone container is a harmless no-op — called on EVERY teardown path so no
/// container leaks. `mode` MUST be the same one the container was launched with (CT-007 slice 2:
/// [`SpawnedRunsc`] carries it alongside `bin`/`container_id` for exactly this reason). If the
/// invocation policy can't be applied (only possible if `ExplicitUserNamespace`'s policy was
/// somehow never validated, which the ORIGINAL launch that created this container already
/// required — practically unreachable), this is a silent no-op: there is nothing safe to delete
/// with, and this path is best-effort cleanup already, never the sole source of truth for container
/// lifecycle.
pub(super) fn delete_container(bin: &Path, container_id: &str, mode: RunscInvocationMode) {
    let mut cmd = Command::new(bin);
    if apply_runsc_invocation_policy(&mut cmd, bin, mode).is_err() {
        return;
    }
    let _ = cmd.arg("delete").arg("-force").arg(container_id).output();
}
