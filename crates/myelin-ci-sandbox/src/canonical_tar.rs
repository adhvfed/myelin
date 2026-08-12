use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const RECORD_SIZE: u64 = 20 * 512;
const BLOCK_SIZE: u64 = 512;

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

pub(crate) fn verified_asset_tree_sha256_hex(
    dir: &Path,
    expected_uid: u32,
) -> Result<String, AssetTreeVerificationError> {
    canonical_tree_sha256_impl(dir, Some(expected_uid)).map(hex_digest)
}

pub fn canonical_tree_sha256(dir: &Path) -> io::Result<[u8; 32]> {
    match canonical_tree_sha256_impl(dir, None) {
        Ok(digest) => Ok(digest),
        Err(AssetTreeVerificationError::Io(error)) => Err(error),
        Err(
            AssetTreeVerificationError::GroupOrWorldWritable { .. }
            | AssetTreeVerificationError::UnexpectedOwner { .. },
        ) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "generic canonical tree hashing unexpectedly applied an asset metadata policy",
        )),
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
        sink.write_zeros(2 * BLOCK_SIZE)?;
    }
    let remainder = sink.written % RECORD_SIZE;
    if remainder != 0 {
        sink.write_zeros(RECORD_SIZE - remainder)?;
    }
    let digest = sink.hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    Ok(out)
}

pub(crate) fn verify_asset_entry_metadata(
    path: &Path,
    metadata: &fs::Metadata,
    expected_uid: u32,
) -> Result<(), AssetTreeVerificationError> {
    if metadata.file_type().is_symlink() {
        return Ok(());
    }

    let mode = metadata.mode() & 0o7777;
    if mode & 0o022 != 0 {
        return Err(AssetTreeVerificationError::GroupOrWorldWritable {
            path: path.to_path_buf(),
            mode,
        });
    }

    let actual_uid = metadata.uid();
    if actual_uid != expected_uid && actual_uid != 0 {
        return Err(AssetTreeVerificationError::UnexpectedOwner {
            path: path.to_path_buf(),
            expected_uid,
            actual_uid,
        });
    }

    Ok(())
}

struct Entry {
    archive_name: Vec<u8>,
    abs_path: PathBuf,
}

fn collect_sorted_entries(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut entries = vec![Entry {
        archive_name: b"./".to_vec(),
        abs_path: dir.to_path_buf(),
    }];
    visit_dir_sorted(dir, dir, &mut entries)?;
    Ok(entries)
}

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
        let rel = abs.strip_prefix(root).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("directory walk escaped its canonical root: {error}"),
            )
        })?;
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

fn write_octal_field(field: &mut [u8], value: u64, digits: usize) -> io::Result<()> {
    debug_assert_eq!(field.len(), digits + 1);
    let rendered = format!("{value:0width$o}", width = digits);
    if rendered.len() != digits {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "value {value} does not fit in a {digits}-digit octal tar header field (this \
                 pure-Rust canonical-tar hasher does not implement GNU's base-256 extension for \
                 oversized fields - refusing rather than producing a header GNU tar would not)"
            ),
        ));
    }
    field[..digits].copy_from_slice(rendered.as_bytes());
    field[digits] = 0;
    Ok(())
}

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
    write_octal_field(&mut h[100..108], mode as u64, 7)?;
    write_octal_field(&mut h[108..116], 0, 7)?;
    write_octal_field(&mut h[116..124], 0, 7)?;
    write_octal_field(&mut h[124..136], size, 11)?;
    write_octal_field(&mut h[136..148], 0, 11)?;
    h[148..156].copy_from_slice(b"        ");
    h[156] = typeflag;
    let ln = linkname.len().min(100);
    h[157..157 + ln].copy_from_slice(&linkname[..ln]);
    h[257..265].copy_from_slice(b"ustar  \0");

    let sum: u64 = h.iter().map(|&b| b as u64).sum();
    let rendered = format!("{sum:06o}");
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
             symlinks, and hardlinks to a regular file are supported - a device node / FIFO / \
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
                 (the file changed size between stat and read - refusing rather than emitting a \
                 header that would not match the shell recipe's own tar run over the same race)"
            ),
        ));
    }
    sink.pad_to_block(copied)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn verified_tree_accepts_world_moded_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!(
            "myelin-canonical-tar-symlink-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(dir.join("real"), b"content").unwrap();
        fs::set_permissions(dir.join("real"), fs::Permissions::from_mode(0o644)).unwrap();
        std::os::unix::fs::symlink("/proc/mounts", dir.join("mtab")).unwrap();
        let uid = unsafe { libc::geteuid() };
        let verified = verified_asset_tree_sha256_hex(&dir, uid)
            .expect("a symlink's 0o777 mode must not trip the writable refusal");
        assert_eq!(verified, canonical_tree_sha256_hex(&dir).unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

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
