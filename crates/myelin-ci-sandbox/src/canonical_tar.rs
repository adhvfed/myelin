//! # A pure-Rust canonical-tree SHA-256 hasher (CT-007 gate 2/4, registry slice)
//!
//! **Why this exists.** `crates/myelin-lints/tests/runner_asset_digest_pin.rs` and
//! `scripts/dogfood.sh`'s `verify_ci_rootfs()` both compute a staged gVisor rootfs directory's
//! "canonical-tree digest" by SHELLING OUT to the host `tar` + `sha256sum`:
//!
//! ```text
//! tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner --format=gnu -C <dir> -cf - . \
//!   | sha256sum
//! ```
//!
//! [`GvisorAssetRegistry::from_bindings`](crate::asset_registry::GvisorAssetRegistry::from_bindings)
//! needs the SAME digest, computed on the PRODUCTION launch path — BEFORE any resource is reserved
//! or any launch permit is granted. Spawning a host `tar` process from that trusted path would trip
//! this repo's own `no-host-exec` architecture lint (`crates/myelin-lints/src/lints.rs`) — a
//! production launch-authority decision may not shell out to the host kernel. This module
//! reproduces the EXACT SAME byte stream (and therefore the exact same SHA-256 digest) as the shell
//! recipe above, entirely in-process, streaming file content directly into the running hash so a
//! large rootfs (the Rust asset alone is >800 MiB) is never materialized in host memory.
//!
//! ## The exact GNU-tar byte format reproduced here (reverse-engineered against a REAL `tar`
//! (GNU tar) 1.35 invocation of the recipe above over real directories with nested dirs, regular
//! files, symlinks, and hardlinks — see the module test [`matches_real_tar_recipe_over_a_synthetic_tree`]):
//!
//! - Every archive member name is `./` + the path relative to the root (directories get a trailing
//!   `/`; the root itself is archived as the single entry `./`).
//! - `--sort=name` is a **per-directory, depth-first traversal**: within each directory, children
//!   are sorted by their BARE entry name (raw bytes, no path prefix, and — critically — no trailing
//!   `/` even for a subdirectory) before any is emitted or recursed into; the trailing `/` is
//!   appended to a directory's own archived name only AFTER that comparison, for display purposes.
//!   This is NOT equivalent to a global flat sort of the fully-assembled archived name strings: a
//!   real Debian-slim rootfs contains both a file `etc/ca-certificates.conf` and a directory
//!   `etc/ca-certificates/` in the same parent, and comparing the two FULL strings byte-wise would
//!   order the file first (`'.'` 0x2E < `'/'` 0x2F) — but real `tar --sort=name` emits the directory
//!   first, because `"ca-certificates"` (no slash) is a strict prefix of `"ca-certificates.conf"`.
//!   See [`collect_sorted_entries`]/[`visit_dir_sorted`]'s own doc comments for the full account of
//!   how this was found (it silently produced a WRONG digest for the real `linux-rust-v1` asset
//!   until fixed and re-verified).
//! - `--mtime=@0`: every entry's mtime field is all-zero.
//! - `--owner=0 --group=0`: every entry's uid/gid fields are zero.
//! - `--numeric-owner`: the uname/gname fields are left EMPTY (no `/etc/passwd` lookup), not "root".
//! - `--format=gnu`: magic bytes `"ustar  \0"` (note: two spaces, not the POSIX `"ustar\0" "00"`
//!   pair), the OLDGNU on-disk layout, and the classic `././@LongLink` mechanism for a name or link
//!   target whose raw byte length is > 100 (a name of EXACTLY 100 bytes fits with no NUL terminator
//!   and is NOT extended — the primary header's own name/linkname field is
//!   truncated to its first 100 bytes and disregarded by a GNU-aware reader).
//! - A file with `nlink() > 1` that repeats an already-emitted `(dev, ino)` pair is archived as a
//!   GNU hardlink entry (typeflag `'1'`, size 0, linkname = the FIRST entry's own archived name) —
//!   the first-seen occurrence in sort order gets the real content, every later occurrence is the
//!   hardlink.
//! - The checksum field is 6 octal digits + NUL + SPACE (computed with the field itself blanked to
//!   ASCII spaces) — this differs from a naive "N octal digits + NUL" encoding some other tar
//!   writers use for other numeric fields, so it is computed by hand here rather than borrowed from
//!   a general-purpose octal-field helper.
//! - The output is padded with zero bytes to a multiple of the *default GNU tar blocking factor*,
//!   10240 bytes (20 × 512-byte blocks) — this is NOT just "two zero blocks to end the archive"; the
//!   whole stream (headers + content + the two-block end marker) is padded again, out to the next
//!   10240-byte boundary. Skipping this step silently produces a DIFFERENT digest for any tree whose
//!   total archive size isn't already a multiple of 10240 bytes.
//!
//! **Known, deliberate limitation:** only directories, regular files, symlinks, and (via the
//! hardlink check) repeated regular files are supported. A character/block device, FIFO, or socket
//! node is refused with a clear error rather than guessed at — none of the two currently-staged
//! runner assets (`~/.local/share/gvisor-assets/rootfs`, `.../rust-rootfs`) contain one (both are
//! `docker export`s, which never carry `/dev` nodes), so there is no real content to validate such a
//! code path against. Likewise a single file whose size does not fit the 11-octal-digit size field
//! (>= 8 GiB) is refused rather than silently switched to GNU's base-256 binary size extension.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// GNU tar's default blocking factor: 20 × 512-byte blocks. The whole output stream (every header,
/// every content block, and the two-zero-block end-of-archive marker) is zero-padded out to the
/// next multiple of this — matching `tar`'s default behaviour writing to a pipe or a regular file.
const RECORD_SIZE: u64 = 20 * 512;
const BLOCK_SIZE: u64 = 512;

/// Compute the canonical-tree SHA-256 digest of `dir`, hex-encoded — byte-identical to
/// `tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner --format=gnu -C <dir> -cf - . \
/// | sha256sum`.
pub fn canonical_tree_sha256_hex(dir: &Path) -> io::Result<String> {
    let digest = canonical_tree_sha256(dir)?;
    Ok(hex_digest(digest))
}

fn hex_digest(digest: [u8; 32]) -> String {
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// A DAC invariant violation found while hashing a registry-backed shared asset tree.
///
/// This is deliberately separate from the generic canonical-tree hasher: callers that only need
/// to reproduce the canonical tar recipe can still hash arbitrary fixtures, while registry
/// construction uses [`verified_asset_tree_sha256_hex`] and refuses unsafe on-disk metadata.
#[derive(Debug)]
pub(crate) enum AssetTreeVerificationError {
    Io(io::Error),
    GroupOrWorldWritable {
        path: PathBuf,
        mode: u32,
    },
    UnexpectedOwner {
        path: PathBuf,
        expected_uid: u32,
        actual_uid: u32,
    },
}

impl std::fmt::Display for AssetTreeVerificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => error.fmt(f),
            Self::GroupOrWorldWritable { path, mode } => write!(
                f,
                "verified asset entry {} has unsafe mode {mode:04o}: group/world-writable bits \
                 0022 must be clear",
                path.display()
            ),
            Self::UnexpectedOwner {
                path,
                expected_uid,
                actual_uid,
            } => write!(
                f,
                "verified asset entry {} is owned by uid {actual_uid}, expected asset-store owner \
                 uid {expected_uid}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for AssetTreeVerificationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::GroupOrWorldWritable { .. } | Self::UnexpectedOwner { .. } => None,
        }
    }
}

impl From<io::Error> for AssetTreeVerificationError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Hash a registry-backed shared asset while enforcing that every entry is DAC-unwritable by
/// non-owners and is owned by `expected_uid`. Validation and hashing consume the same lstat result
/// for each root/file/directory/symlink entry, so unsafe metadata can never produce a verified
/// digest.
pub(crate) fn verified_asset_tree_sha256_hex(
    dir: &Path,
    expected_uid: u32,
) -> Result<String, AssetTreeVerificationError> {
    canonical_tree_sha256_impl(dir, Some(expected_uid)).map(hex_digest)
}

/// Compute the canonical-tree SHA-256 digest of `dir` as raw bytes.
///
/// `dir` is canonicalized FIRST (resolving every symlink component, including `dir` itself if it is
/// one) — matching `tar -C <dir> -cf - .`'s own semantics: `-C` `chdir`s into `dir` (transparently
/// following any symlink), and everything tar subsequently stats is relative to `.` post-chdir, so
/// the archived ROOT entry always reflects the REAL target directory's own metadata, never a
/// symlink's. A staged runner asset is commonly published as exactly this shape (a stable symlink,
/// e.g. `rust-rootfs`, pointing at a content-addressed `rust-rootfs.versions/sha256-<digest>`
/// directory) — hashing the symlink path WITHOUT this canonicalization would `lstat` the root as a
/// symlink and silently produce a completely different (wrong) digest.
pub fn canonical_tree_sha256(dir: &Path) -> io::Result<[u8; 32]> {
    match canonical_tree_sha256_impl(dir, None) {
        Ok(digest) => Ok(digest),
        Err(AssetTreeVerificationError::Io(error)) => Err(error),
        Err(
            AssetTreeVerificationError::GroupOrWorldWritable { .. }
            | AssetTreeVerificationError::UnexpectedOwner { .. },
        ) => unreachable!("no asset metadata policy is requested by the generic hasher"),
    }
}

fn canonical_tree_sha256_impl(
    dir: &Path,
    expected_uid: Option<u32>,
) -> Result<[u8; 32], AssetTreeVerificationError> {
    let dir = fs::canonicalize(dir)?;
    let entries = collect_sorted_entries(&dir)?;
    let mut sink = HashingSink {
        hasher: Sha256::new(),
        written: 0,
    };
    {
        let mut seen_hardlinks: HashMap<(u64, u64), Vec<u8>> = HashMap::new();
        for entry in &entries {
            let metadata = fs::symlink_metadata(&entry.abs_path)?;
            if let Some(expected_uid) = expected_uid {
                verify_asset_entry_metadata(&entry.abs_path, &metadata, expected_uid)?;
            }
            append_entry(&mut sink, entry, &metadata, &mut seen_hardlinks)?;
        }
        // End-of-archive: two zero blocks (1024 bytes), matching GNU tar.
        sink.write_zeros(2 * BLOCK_SIZE)?;
    }
    // Pad the WHOLE stream out to the next multiple of the default 10240-byte record size — this is
    // additional to (not a substitute for) the two-zero-block end marker above.
    let remainder = sink.written % RECORD_SIZE;
    if remainder != 0 {
        sink.write_zeros(RECORD_SIZE - remainder)?;
    }
    let digest = sink.hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    Ok(out)
}

/// Enforce the registry's per-entry DAC policy. Kept as a small helper so the ownership refusal can
/// be unit-tested without requiring privilege to chown a fixture to another uid.
pub(crate) fn verify_asset_entry_metadata(
    path: &Path,
    metadata: &fs::Metadata,
    expected_uid: u32,
) -> Result<(), AssetTreeVerificationError> {
    let mode = metadata.mode() & 0o7777;
    if mode & 0o022 != 0 {
        return Err(AssetTreeVerificationError::GroupOrWorldWritable {
            path: path.to_path_buf(),
            mode,
        });
    }

    let actual_uid = metadata.uid();
    if actual_uid != expected_uid {
        return Err(AssetTreeVerificationError::UnexpectedOwner {
            path: path.to_path_buf(),
            expected_uid,
            actual_uid,
        });
    }

    Ok(())
}

/// A single archive member: its exact archived name (raw bytes, `./`-prefixed, trailing `/` for a
/// directory), its absolute on-disk path, and enough `lstat` facts to build its header.
struct Entry {
    archive_name: Vec<u8>,
    abs_path: PathBuf,
}

/// Walk `dir` (never following symlinks) and return every member — the root itself first, then
/// every descendant — in GNU tar's actual `--sort=name` order.
///
/// **This is NOT a global flat sort of the full archived path strings.** It is a per-directory,
/// depth-first traversal: within EACH directory, children are sorted by their bare entry name (raw
/// bytes, no path prefix, and critically no trailing `/` even for a subdirectory) before any of them
/// is emitted or recursed into. The trailing `/` is appended to a directory's `archive_name` only
/// AFTER sorting, for display/header purposes — it plays NO part in the sort comparison itself.
///
/// This distinction is observable, and matters: a real Debian-slim rootfs contains BOTH a file
/// `etc/ca-certificates.conf` and a directory `etc/ca-certificates/` in the same parent. Comparing
/// the two FULL archived strings byte-wise (`"ca-certificates.conf"` vs `"ca-certificates/"`) orders
/// the file first (`'.'` 0x2E < `'/'` 0x2F) — but real `tar --sort=name` emits the DIRECTORY first,
/// because it compares the bare entry names (`"ca-certificates"` vs `"ca-certificates.conf"`, where
/// the shorter name is a strict prefix and therefore sorts first) with the slash appended only
/// afterward. Confirmed against a real `tar` (GNU tar) 1.35 run over the actual staged
/// `linux-rust-v1` rootfs, which contains exactly this shape.
fn collect_sorted_entries(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut entries = vec![Entry {
        archive_name: b"./".to_vec(),
        abs_path: dir.to_path_buf(),
    }];
    visit_dir_sorted(dir, dir, &mut entries)?;
    Ok(entries)
}

/// Depth-first helper for [`collect_sorted_entries`]: sort `current_abs`'s own children by bare
/// name, then for each (in that order) emit its `Entry` and — if it is itself a directory — recurse
/// into it immediately before moving on to the next sibling.
fn visit_dir_sorted(root: &Path, current_abs: &Path, out: &mut Vec<Entry>) -> io::Result<()> {
    let mut children: Vec<(Vec<u8>, PathBuf)> = Vec::new();
    for child in fs::read_dir(current_abs)? {
        let child = child?;
        children.push((
            child.file_name().as_os_str().as_bytes().to_vec(),
            child.path(),
        ));
    }
    children.sort_by(|a, b| a.0.cmp(&b.0));

    for (_bare_name, abs) in children {
        let meta = fs::symlink_metadata(&abs)?;
        let rel = abs
            .strip_prefix(root)
            .expect("child path is always under root");
        let mut archive_name = b"./".to_vec();
        archive_name.extend_from_slice(rel.as_os_str().as_bytes());
        let is_dir = meta.file_type().is_dir();
        if is_dir {
            archive_name.push(b'/');
        }
        out.push(Entry {
            archive_name,
            abs_path: abs.clone(),
        });
        if is_dir {
            visit_dir_sorted(root, &abs, out)?;
        }
    }
    Ok(())
}

/// A `Write` sink that feeds every byte into a running SHA-256 hash while counting total bytes
/// written (needed for the final record-size padding, and for the per-entry 512-byte content pad).
struct HashingSink {
    hasher: Sha256,
    written: u64,
}

impl HashingSink {
    fn write_zeros(&mut self, mut n: u64) -> io::Result<()> {
        const BUF: [u8; 512] = [0u8; 512];
        while n > 0 {
            let take = n.min(BUF.len() as u64) as usize;
            self.write_all(&BUF[..take])?;
            n -= take as u64;
        }
        Ok(())
    }

    /// Pad the CURRENT entry's just-written content (its length is `len`) with zero bytes out to the
    /// next 512-byte boundary — matching how GNU tar pads each member's data, independent of the
    /// final whole-archive record padding in [`canonical_tree_sha256`].
    fn pad_to_block(&mut self, len: u64) -> io::Result<()> {
        let remainder = len % BLOCK_SIZE;
        if remainder != 0 {
            self.write_zeros(BLOCK_SIZE - remainder)?;
        }
        Ok(())
    }
}

impl Write for HashingSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.hasher.update(buf);
        self.written += buf.len() as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Write an octal-encoded numeric field: `digits` zero-padded octal digits followed by a single NUL,
/// filling `field` exactly (`field.len() == digits + 1`). Fails closed (rather than silently
/// truncating/misrepresenting a value) if `value` does not fit in `digits` octal digits — this
/// hasher does not implement GNU's base-256 binary-size extension.
fn write_octal_field(field: &mut [u8], value: u64, digits: usize) -> io::Result<()> {
    debug_assert_eq!(field.len(), digits + 1);
    let rendered = format!("{value:0width$o}", width = digits);
    if rendered.len() != digits {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "value {value} does not fit in a {digits}-digit octal tar header field (this \
                 pure-Rust canonical-tar hasher does not implement GNU's base-256 extension for \
                 oversized fields — refusing rather than producing a header GNU tar would not)"
            ),
        ));
    }
    field[..digits].copy_from_slice(rendered.as_bytes());
    field[digits] = 0;
    Ok(())
}

/// Build one 512-byte GNU-tar header (mode/size supplied by the caller; uid/gid/mtime are ALWAYS
/// zero — the `--owner=0 --group=0 --mtime=@0` posture; uname/gname are ALWAYS empty —
/// `--numeric-owner`). `name`/`linkname` are truncated to their first 100 raw bytes if longer (a
/// preceding `././@LongLink` entry, written by the caller, carries the real value in that case).
fn build_header(
    name: &[u8],
    mode: u32,
    size: u64,
    typeflag: u8,
    linkname: &[u8],
) -> io::Result<[u8; 512]> {
    let mut h = [0u8; 512];
    let n = name.len().min(100);
    h[0..n].copy_from_slice(&name[..n]);
    write_octal_field(&mut h[100..108], mode as u64, 7)?; // mode
    write_octal_field(&mut h[108..116], 0, 7)?; // uid (always 0)
    write_octal_field(&mut h[116..124], 0, 7)?; // gid (always 0)
    write_octal_field(&mut h[124..136], size, 11)?; // size
    write_octal_field(&mut h[136..148], 0, 11)?; // mtime (always 0)
    h[148..156].copy_from_slice(b"        "); // chksum placeholder: 8 ASCII spaces during the sum
    h[156] = typeflag;
    let ln = linkname.len().min(100);
    h[157..157 + ln].copy_from_slice(&linkname[..ln]);
    h[257..265].copy_from_slice(b"ustar  \0"); // GNU magic("ustar ") + version(" \0")
                                               // uname/gname/devmajor/devminor/the GNU incremental-backup fields (atime/ctime/offset/
                                               // longnames/sparse/isextended/realsize) all stay zero — this hasher creates archives, never
                                               // incremental ones, matching what the shell recipe (a plain `tar -c`) itself produces.

    // The checksum field is 6 zero-padded octal digits + NUL + SPACE — NOT the "N digits + NUL" style
    // `write_octal_field` uses for every other numeric field (verified against a real `tar` header:
    // `chksum` bytes end `...\0 `, two bytes, where every other field ends in a lone `\0`).
    let sum: u64 = h.iter().map(|&b| b as u64).sum();
    let rendered = format!("{sum:06o}");
    // A 512-byte header's byte-sum cannot exceed 512*255 = 130560 = 0o377600 (6 octal digits) — this
    // can never overflow the 6-digit field.
    debug_assert_eq!(
        rendered.len(),
        6,
        "checksum {sum} did not render to exactly 6 octal digits"
    );
    h[148..154].copy_from_slice(rendered.as_bytes());
    h[154] = 0;
    h[155] = b' ';
    Ok(h)
}

/// Emit a GNU long-name (`typeflag = b'L'`) or long-link (`typeflag = b'K'`) auxiliary entry: the
/// fixed name `././@LongLink`, a hardcoded mode `0o644`, uid/gid/mtime zero, and `payload` (the real
/// full name/link bytes, NUL-terminated) as its content — matching real GNU tar's own encoding of
/// this GNU-specific extension exactly (verified against a real `tar` invocation with a >100-byte
/// path and confirmed byte-for-byte in the module test).
fn write_long_entry(sink: &mut HashingSink, typeflag: u8, payload: &[u8]) -> io::Result<()> {
    let mut data = Vec::with_capacity(payload.len() + 1);
    data.extend_from_slice(payload);
    data.push(0);
    let header = build_header(b"././@LongLink", 0o644, data.len() as u64, typeflag, b"")?;
    sink.write_all(&header)?;
    sink.write_all(&data)?;
    sink.pad_to_block(data.len() as u64)?;
    Ok(())
}

fn append_entry(
    sink: &mut HashingSink,
    entry: &Entry,
    meta: &fs::Metadata,
    seen_hardlinks: &mut HashMap<(u64, u64), Vec<u8>>,
) -> io::Result<()> {
    let mode = meta.mode() & 0o7777;
    let name = entry.archive_name.as_slice();

    if meta.file_type().is_dir() {
        return write_simple_entry(sink, name, mode, b'5', 0, &[], io::empty());
    }

    // Hardlink detection: GNU tar only even LOOKS at (dev, ino) for a file with more than one link;
    // the first occurrence (in sorted-name order — the SAME order this function is called in) gets
    // the real content, every later occurrence becomes a type-'1' hardlink entry referencing it.
    if meta.nlink() > 1 {
        let key = (meta.dev(), meta.ino());
        if let Some(first_name) = seen_hardlinks.get(&key) {
            let first_name = first_name.clone();
            return write_simple_entry(sink, name, mode, b'1', 0, &first_name, io::empty());
        }
        seen_hardlinks.insert(key, name.to_vec());
    }

    if meta.file_type().is_symlink() {
        let target = fs::read_link(&entry.abs_path)?;
        let target_bytes = target.as_os_str().as_bytes();
        return write_simple_entry(sink, name, mode, b'2', 0, target_bytes, io::empty());
    }

    if meta.file_type().is_file() {
        let size = meta.len();
        let file = File::open(&entry.abs_path)?;
        return write_simple_entry(sink, name, mode, b'0', size, &[], file);
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
            "canonical-tar hasher: unsupported file type at {} (only directories, regular files, \
             symlinks, and hardlinks to a regular file are supported — a device node / FIFO / \
             socket has never appeared in either staged runner-asset rootfs, so this refuses \
             rather than guessing at an unverified byte encoding)",
            entry.abs_path.display()
        ),
    ))
}

#[allow(clippy::too_many_arguments)]
fn write_simple_entry<R: Read>(
    sink: &mut HashingSink,
    name: &[u8],
    mode: u32,
    typeflag: u8,
    size: u64,
    linkname: &[u8],
    mut data: R,
) -> io::Result<()> {
    // A name/linkname of EXACTLY 100 bytes fits the header field with no room for a NUL terminator
    // but is still NOT truncated — GNU tar only engages the `././@LongLink` extension once the raw
    // byte length exceeds 100 (confirmed against a real `tar` run: the real staged `linux-rust-v1`
    // asset contains a path whose name is exactly 100 bytes and it is written as a plain entry, no
    // preceding `L` entry).
    if name.len() > 100 {
        write_long_entry(sink, b'L', name)?;
    }
    if linkname.len() > 100 {
        write_long_entry(sink, b'K', linkname)?;
    }
    let header = build_header(name, mode, size, typeflag, linkname)?;
    sink.write_all(&header)?;
    let copied = io::copy(&mut data, sink)?;
    if copied != size {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!(
                "canonical-tar hasher: read {copied} bytes but the header declared size {size} \
                 (the file changed size between stat and read — refusing rather than emitting a \
                 header that would not match the shell recipe's own tar run over the same race)"
            ),
        ));
    }
    sink.pad_to_block(copied)?;
    Ok(())
}

// NOTE: the tests proving this module reproduces the REAL `tar | sha256sum` shell recipe
// byte-for-byte (including the RED-first proof against `runner-assets.toml`'s committed
// `linux-rust-v1` pin) live in `tests/canonical_tar_matches_shell_recipe_test.rs`, NOT here. Those
// tests shell out to the host `tar`/`sha256sum` FOR COMPARISON ONLY (never as part of this crate's
// production launch path) — and this repo's `no-host-exec` architecture lint (`crates/myelin-lints/
// tests/workspace_clean.rs`) scans every `crates/*/src/**/*.rs` file (including `#[cfg(test)]`
// blocks) but excludes the whole `**/tests/**` directory, exactly for this "test fixture needs a
// real host tool to verify against" case. Keeping this file's own `src`-resident unit tests free of
// `Command::new` avoids adding a new named lint exclusion for what is genuinely test-only
// comparison code.
#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal in-process determinism/shape check (no host process spawned): hashing the same
    /// empty directory twice is stable, and a directory containing one file, one nested dir, one
    /// symlink, and a hardlink pair hashes without error and differs from the empty-directory digest.
    #[test]
    fn hashing_is_deterministic_and_shape_sensitive() {
        let dir = std::env::temp_dir().join(format!(
            "myelin-canonical-tar-unit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let empty_a = canonical_tree_sha256_hex(&dir).unwrap();
        let empty_b = canonical_tree_sha256_hex(&dir).unwrap();
        assert_eq!(
            empty_a, empty_b,
            "hashing an unchanged tree twice is deterministic"
        );

        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("a.txt"), b"hello").unwrap();
        fs::write(dir.join("sub/file.txt"), b"world").unwrap();
        std::os::unix::fs::symlink("file.txt", dir.join("sub/link.txt")).unwrap();
        fs::hard_link(dir.join("a.txt"), dir.join("sub/hard.txt")).unwrap();
        let populated = canonical_tree_sha256_hex(&dir).unwrap();
        assert_ne!(
            empty_a, populated,
            "a populated tree hashes differently from an empty one"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// An unsupported file type (a FIFO, which needs no root privilege to create, unlike a device
    /// node) is refused with a clear error rather than silently mis-encoded.
    #[test]
    fn unsupported_file_type_refuses_fail_closed() {
        let dir = std::env::temp_dir().join(format!(
            "myelin-canonical-tar-fifo-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let fifo_path = dir.join("a-fifo");
        let fifo_cstr = std::ffi::CString::new(fifo_path.as_os_str().as_bytes()).unwrap();
        // SAFETY: `mkfifo` is a plain libc syscall wrapper over a path this test owns and cleans up;
        // no untrusted input, no shared mutable state.
        let rc = unsafe { libc::mkfifo(fifo_cstr.as_ptr(), 0o644) };
        assert_eq!(rc, 0, "mkfifo needs no special privilege");

        let result = canonical_tree_sha256_hex(&dir);
        let _ = fs::remove_dir_all(&dir);
        assert!(
            result.is_err(),
            "a FIFO must be refused fail-closed, not silently hashed as if it were a regular file"
        );
    }
}
