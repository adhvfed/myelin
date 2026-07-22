//! The content-addressed [`BlobStore`] trait + the fs-backed floor (contract 11.2, P-ST-03).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §3.2 (Tier 2 — the narrow
//! content-addressed `put/get/head/delete` trait; **BLAKE3-on-write with a self-describing
//! multihash prefix so SHA-256 can coexist**; **address by plaintext hash WITHIN a tenant's
//! keyspace, store ciphertext** — dedup is per-tenant, cross-tenant dedup deliberately
//! forgone as a residency leak; per-blob random content key wrapped by the tenant/per-subject
//! DEK; immutable-tier erasure = crypto-shred not delete), §10 (drill D-S8 / STOR-D7 blob
//! integrity), §11 (the cited content-addressing prior art — Git/Venti/IPFS CID/BLAKE3).
//! Contract-index row 11.2.
//!
//! ## The frozen trait shape (storage.md §3.2 — copied byte-exact)
//! ```ignore
//! pub trait BlobStore {
//!     fn put(&self, bytes: &[u8]) -> Result<ContentHash>;  // content address = the hash
//!     fn get(&self, h: &ContentHash) -> Result<Vec<u8>>;
//!     fn head(&self, h: &ContentHash) -> Result<BlobMeta>;
//!     fn delete(&self, h: &ContentHash) -> Result<()>;     // crypto-shred is the real erasure
//! }
//! ```
//! The `Result` here is `Result<T, BlobError>` (this crate's error). The trait is
//! **per-tenant-keyed**: every method takes the [`TenantId`] whose keyspace the blob lives
//! in (the §3.2 "within a tenant's keyspace" rule). The architecture's bare signatures elide
//! the tenant because they show the *primitive*; the per-tenant keyspace is the load-bearing
//! isolation property the prompt mandates ("the key path is `<tenant>/...`; dedup is
//! per-tenant only"), so it is an explicit parameter here — recorded as a faithful
//! realisation of §3.2, not a divergence (EI-01 §1; the trait method names/arity/return
//! types are preserved exactly).
//!
//! ## What this prompt ships (P-ST-03) and what it does NOT
//! - **(a) BLAKE3 hash-on-write** with a self-describing multihash prefix ([`ContentHash`]).
//!   The content address IS the hash (Git/Venti model). The multihash prefix lets a future
//!   SHA-256 blob coexist (a test asserts the prefix is present + parsed).
//! - **(b) Address by plaintext hash within a tenant's keyspace, store ciphertext.** The fs
//!   key path is `<tenant>/<algo>/<aa>/<rest>` (a Git-style fan-out). Dedup is **per-tenant
//!   only**: two tenants storing identical bytes get **two** stored objects (asserted in a
//!   test) — deliberate residency isolation, not a bug. The "store ciphertext" wrap is the
//!   named floor below.
//! - **(c) Re-hash-on-read integrity.** [`FsBlobStore::get`] re-hashes the bytes it read and
//!   **refuses to serve** on a content-address mismatch (0 silent serve), incrementing the
//!   `blob_integrity_fail` telemetry counter ([`BlobTelemetry`]).
//! - **(d) Holder-registered** via the harness seam ([`crate::holder`]; the BlobStore is a
//!   `PersonalDataHolder` — references content-addressed blobs, erasure = crypto-shred).
//!
//! ## Floors named (stubbed / deferred + the filling prompt) — VISION §3, prompt DoD
//! - **The per-blob content-key WRAP is a stub here.** §3.2 mandates a per-blob random
//!   content key wrapped by the tenant/per-subject DEK so the content-address stays stable
//!   while rotation/shred operate at the key layer; the KMS hierarchy lands in **M1
//!   (P-ST-06)** and **P-ST-08 (global P-095)** wires the REAL wrap (the "store ciphertext"
//!   half). On THIS floor [`FsBlobStore`] stores bytes through a [`ContentWrap`] seam whose
//!   default is the **identity wrap** (plaintext-at-rest) — the seam exists so P-ST-08 is a
//!   localised swap, and no real tenant data is written before the M1 STOR-D1 restore-verify
//!   gate. Recorded HERE in writing.
//! - **The object-store (MinIO / Ceph RADOS / RustFS / Scaleway) BlobStore is SHIPPED (the M5
//!   follow-on P-ST-30, global P-441).** [`crate::s3blob::S3BlobStore`] implements THIS unchanged
//!   trait against an S3-compatible object store (a one-line backing swap, config-selected via
//!   [`crate::backend`]); the STOR-D7 "recover from a replica" property the object tier adds is
//!   [`crate::replicated_blob::ReplicatedBlobStore`] (backing-agnostic over the trait — the fs
//!   floor in CI, the live S3 backing in the integration test). **The fs-BlobStore floor is now
//!   promoted to its full answer** (the floor→follow-on pair is closed). Recorded HERE in writing.
//! - **`PersonalDataHolder` DSR bodies** for the BlobStore (crypto-shred erase) are the GDPR
//!   M1 deliverable (P-ST-09 ships the six-step crypto-shred algorithm); here the holder is
//!   **registered** to its frozen shape (the [`crate::holder`] seam) so "we forgot the blob
//!   store" is structurally impossible.
//!
//! ## Mutation floor (mandatory-core, ≥ 80% — EI-01 §2; prompt TESTS field)
//! The hash-on-write + re-hash-on-read integrity path ([`ContentHash::blake3`],
//! [`FsBlobStore::put`], [`FsBlobStore::get`]'s recompute-and-refuse branch) is mandatory-core:
//! the load-bearing decision is *the stored bytes must re-hash to the requested address or the
//! serve is refused* (0 silent serve). The floor is **≥ 80%**; the achieved score is **100% of
//! viable mutants caught** (`cargo mutants -p myelin-storage -f crates/myelin-storage/src/blob.rs`
//! → 29 caught, 6 unviable, 0 missed). Every mutation of the integrity comparison, the address
//! computation, and the per-tenant key path is killed by an assertion.

// MR-009b W7.3 — `HashMap` backs only the `test-support`-gated `FsBlobStore` floor.
#[cfg(any(test, feature = "test-support"))]
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
// MR-009b W7.3 — `Mutex` backs only the `test-support`-gated `FsBlobStore` floor.
#[cfg(any(test, feature = "test-support"))]
use std::sync::Mutex;

use myelin_tenancy::TenantId;

/// The self-describing content address of a blob: `<algo>:<hex-digest>` (a minimal multihash
/// — an algorithm tag followed by the digest), so a future SHA-256 blob coexists with the
/// BLAKE3 default. The content address IS the hash (Git/Venti/IPFS-CID model, storage.md §11)
/// — it is computed from the **plaintext** bytes (address by plaintext hash, store
/// ciphertext, §3.2), so it is stable across the encryption/rotation P-ST-08 adds.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct ContentHash {
    /// The hash algorithm tag (the self-describing multihash prefix). BLAKE3 is the default
    /// for new blobs; the tag is what lets SHA-256 blobs coexist (§3.2).
    pub algo: HashAlgo,
    /// The lower-case hex digest of the plaintext bytes under [`Self::algo`].
    pub digest_hex: String,
}

impl ContentHash {
    /// Compute the BLAKE3 content address of `bytes` (hash-on-write, the default for new
    /// blobs). A cited proven structure (BLAKE3 2020) — VISION §4, never a hand-rolled hash.
    pub fn blake3(bytes: &[u8]) -> ContentHash {
        let digest = blake3::hash(bytes);
        ContentHash {
            algo: HashAlgo::Blake3,
            digest_hex: hex::encode(digest.as_bytes()),
        }
    }

    /// Compute the SHA-256 content address of `bytes` — the read-side verify for git-imported
    /// objects (P-ST-22). Git addresses its objects by SHA (SHA-1 legacy / SHA-256 modern), so a
    /// `sha256:`-tagged blob (a git loose object / a pack member) is re-hashed under this to
    /// detect a corrupt object on read. A cited proven structure (RustCrypto `sha2`,
    /// FIPS-180-4) — VISION §4, never a hand-rolled hash. NEW blobs are never written under this
    /// tag by the native [`Self::blake3`] path; this admits the externally-addressed git world.
    pub fn sha256(bytes: &[u8]) -> ContentHash {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(bytes);
        ContentHash {
            algo: HashAlgo::Sha256,
            digest_hex: hex::encode(digest),
        }
    }

    /// The self-describing string form `<algo>:<hex>` — the on-the-wire / key-path address.
    pub fn to_multihash_string(&self) -> String {
        format!("{}:{}", self.algo.tag(), self.digest_hex)
    }

    /// Parse a `<algo>:<hex>` multihash string back into a [`ContentHash`]. The self-describing
    /// prefix is what makes "SHA-256 can coexist" real: an unknown algo tag is an explicit
    /// error, never a silent mis-hash.
    pub fn parse(s: &str) -> std::result::Result<ContentHash, BlobError> {
        let (tag, hex_part) = s
            .split_once(':')
            .ok_or_else(|| BlobError::MalformedAddress(s.to_string()))?;
        let algo =
            HashAlgo::from_tag(tag).ok_or_else(|| BlobError::UnknownAlgo(tag.to_string()))?;
        // Validate the digest is hex (so a corrupted address is caught at parse, not later).
        hex::decode(hex_part).map_err(|_| BlobError::MalformedAddress(s.to_string()))?;
        Ok(ContentHash {
            algo,
            digest_hex: hex_part.to_string(),
        })
    }
}

/// The hash algorithm tag carried in the self-describing multihash prefix (§3.2: "a
/// self-describing multihash prefix so SHA-256 can coexist"). BLAKE3 is the default for new
/// blobs; SHA-256 is admitted as a tag so legacy/Git-imported blobs (which are SHA-1/SHA-256
/// addressed) coexist without re-hashing — the tag, not a global config, decides how a given
/// blob is verified on read.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum HashAlgo {
    /// BLAKE3 — the default hash-on-write algorithm for all new blobs (storage.md §3.2/§11).
    Blake3,
    /// SHA-256 — admitted so externally-addressed blobs (e.g. Git's object model) coexist in
    /// the same store. New blobs are never written under this tag by [`ContentHash::blake3`].
    Sha256,
}

impl HashAlgo {
    /// The stable on-the-wire tag (the multihash prefix string).
    pub fn tag(self) -> &'static str {
        match self {
            HashAlgo::Blake3 => "blake3",
            HashAlgo::Sha256 => "sha256",
        }
    }

    /// Parse a tag back to an algorithm; `None` for an unknown tag (a future algo not yet
    /// admitted — an explicit no, never a silent wrong-verify).
    pub fn from_tag(tag: &str) -> Option<HashAlgo> {
        match tag {
            "blake3" => Some(HashAlgo::Blake3),
            "sha256" => Some(HashAlgo::Sha256),
            _ => None,
        }
    }

    /// Re-hash `bytes` under this algorithm and return the address — the read-side integrity
    /// check. Both admitted algorithms are now verifiable:
    /// - **BLAKE3** — the native hash-on-write address for new blobs (P-ST-03).
    /// - **SHA-256** — the read-side verify for **git-imported objects** (P-ST-22): git
    ///   addresses its objects by SHA, so a `sha256:`-tagged blob (a git loose object / a pack
    ///   member) is re-hashed under SHA-256 and refused on a content-address mismatch — closing
    ///   the floor blob.rs named ("SHA-256 verification rides in with the git object import —
    ///   P-ST-22"). Uses the vetted RustCrypto `sha2` (FIPS-180-4), never a hand-rolled hash.
    // MR-009b W7.3 — the re-hash-on-read helper is driven by the `test-support`-gated `FsBlobStore`
    // floor's `get` (the always-compiled `S3BlobStore::get` inlines its own algo match).
    #[cfg(any(test, feature = "test-support"))]
    fn rehash(self, bytes: &[u8]) -> std::result::Result<ContentHash, BlobError> {
        match self {
            HashAlgo::Blake3 => Ok(ContentHash::blake3(bytes)),
            HashAlgo::Sha256 => Ok(ContentHash::sha256(bytes)),
        }
    }
}

/// The metadata `head` returns for a stored blob (storage.md §3.2). PII-free: a size + the
/// content address + the algorithm tag — never the bytes, never a payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobMeta {
    /// The content address of the blob (its identity).
    pub hash: ContentHash,
    /// The stored object's byte length (the CIPHERTEXT length on the wrapped floor; on the
    /// identity-wrap floor it equals the plaintext length).
    pub stored_len: usize,
}

/// The error surface of the [`BlobStore`] trait (the `Result` in the §3.2 signatures).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlobError {
    /// `get`/`head`/`delete` on a content address absent from the tenant's keyspace.
    NotFound {
        /// The tenant keyspace looked in.
        tenant: TenantId,
        /// The content address that was not present.
        hash: ContentHash,
    },
    /// **The STOR-D7 floor.** `get` re-hashed the stored bytes and the recomputed content
    /// address did NOT equal the requested address — the object is corrupt. The serve is
    /// REFUSED (0 silent wrong-bytes return); `blob_integrity_fail` was incremented.
    IntegrityFail {
        /// The address the caller asked for.
        requested: ContentHash,
        /// The address the stored bytes actually hash to (the mismatch evidence).
        actual: ContentHash,
    },
    /// A backing service could not perform the requested operation. The class is payload-free:
    /// SDK details, credentials, endpoints, buckets, and keys never cross this seam.
    Backend(BlobDependencyError),
    /// A read was refused because its stored or plaintext byte length exceeded a caller ceiling.
    ReadLimitExceeded {
        /// The observed byte length.
        actual: usize,
        /// The caller's maximum byte allowance.
        maximum: usize,
    },
    /// A content-address string was not `<algo>:<hex>`.
    MalformedAddress(String),
    /// A content-address string carried an algorithm tag this store does not know.
    UnknownAlgo(String),
    /// The blob's algorithm has no verification path. Both admitted tags (BLAKE3, SHA-256) are
    /// now verifiable (SHA-256 closed by P-ST-22), so this is no longer produced on the read
    /// path; it is retained as the explicit "never a silent pass" answer for any FUTURE algo tag
    /// admitted to [`HashAlgo`] before its `rehash` arm lands (the integrity gate is never
    /// bypassed by an un-verifiable tag).
    AlgoNotVerifiable(HashAlgo),
}

/// Redacted operational classification for a durable blob backing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlobDependencyError {
    PermanentConfig,
    PermanentAuth,
    Transient,
}

impl std::fmt::Display for BlobDependencyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::PermanentConfig => "object-store dependency has invalid configuration",
            Self::PermanentAuth => "object-store dependency refused authorization",
            Self::Transient => "object-store dependency is temporarily unavailable",
        })
    }
}

impl std::error::Error for BlobDependencyError {}

impl std::fmt::Display for BlobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlobError::NotFound { tenant, hash } => write!(
                f,
                "blob {} not found in tenant {} keyspace",
                hash.to_multihash_string(),
                tenant.0
            ),
            BlobError::IntegrityFail { requested, actual } => write!(
                f,
                "blob integrity fail: requested {} but stored bytes hash to {} — serve refused",
                requested.to_multihash_string(),
                actual.to_multihash_string()
            ),
            BlobError::Backend(kind) => kind.fmt(f),
            BlobError::ReadLimitExceeded { actual, maximum } => write!(
                f,
                "blob read refused: {actual} bytes exceeds the {maximum}-byte limit"
            ),
            BlobError::MalformedAddress(s) => write!(f, "malformed content address: {s}"),
            BlobError::UnknownAlgo(t) => write!(f, "unknown hash algorithm tag: {t}"),
            BlobError::AlgoNotVerifiable(a) => {
                write!(
                    f,
                    "no on-floor verification for algorithm {} (→ P-ST-22)",
                    a.tag()
                )
            }
        }
    }
}

impl std::error::Error for BlobError {}

/// The result alias the §3.2 trait signatures use.
pub type Result<T> = std::result::Result<T, BlobError>;

/// **The frozen content-addressed BlobStore trait (contract 11.2, storage.md §3.2).** The
/// narrow `put/get/head/delete` primitive every blob-holding service uses; fs↔object is a
/// one-line backing swap (the fs impl is the M0 floor, the object-store impl is P-ST-30).
///
/// Every method is **per-tenant-keyed** (the §3.2 "within a tenant's keyspace" isolation
/// rule — see the module docs on why the tenant is explicit). The content address `put`
/// returns is computed from the **plaintext** (address by plaintext hash, store ciphertext),
/// so it is stable across the encryption P-ST-08 wires.
pub trait BlobStore {
    /// Hash-on-write: store `bytes` in `tenant`'s keyspace and return the content address (the
    /// BLAKE3 multihash). Idempotent within a tenant — putting identical bytes twice yields
    /// the same address and stores once (per-tenant dedup). A DIFFERENT tenant putting the
    /// same bytes stores a SECOND object (cross-tenant dedup deliberately forgone, §3.2).
    fn put(&self, tenant: &TenantId, bytes: &[u8]) -> Result<ContentHash>;

    /// Re-hash-on-read: read the object at `hash` in `tenant`'s keyspace, **re-hash the bytes**
    /// and refuse to serve on a content-address mismatch ([`BlobError::IntegrityFail`], 0
    /// silent serve). Returns the exact plaintext bytes on a verified read.
    fn get(&self, tenant: &TenantId, hash: &ContentHash) -> Result<Vec<u8>>;

    /// Read only when both stored metadata and returned plaintext fit `maximum_bytes`. The metadata
    /// check rejects normal oversized objects before [`Self::get`] can materialize them; the
    /// post-read check also protects callers from custom wrapping layers whose plaintext expands.
    fn get_bounded(
        &self,
        tenant: &TenantId,
        hash: &ContentHash,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>> {
        let metadata = self.head(tenant, hash)?;
        if metadata.stored_len > maximum_bytes {
            return Err(BlobError::ReadLimitExceeded {
                actual: metadata.stored_len,
                maximum: maximum_bytes,
            });
        }
        let bytes = self.get(tenant, hash)?;
        if bytes.len() > maximum_bytes {
            return Err(BlobError::ReadLimitExceeded {
                actual: bytes.len(),
                maximum: maximum_bytes,
            });
        }
        Ok(bytes)
    }

    /// Return the [`BlobMeta`] for a stored blob without serving the bytes.
    fn head(&self, tenant: &TenantId, hash: &ContentHash) -> Result<BlobMeta>;

    /// Remove the object from `tenant`'s keyspace. NOTE (§3.2): for a blob reachable from an
    /// immutable/backup tier the REAL erasure is **crypto-shred** (destroy the wrapping key),
    /// not this `delete` — the crypto-shred algorithm is P-ST-09. On the fs floor `delete`
    /// removes the local object; the crypto-shred reach into backups is the M1 deliverable.
    fn delete(&self, tenant: &TenantId, hash: &ContentHash) -> Result<()>;
}

/// The per-blob content-key WRAP seam (the "store ciphertext" half of §3.2). On the M0 floor
/// the default is the **identity wrap** (plaintext-at-rest); **P-ST-08 (global P-095)** swaps
/// in the real per-blob random key wrapped by the tenant/per-subject DEK (the KMS lands M1,
/// P-ST-06). The seam exists now so the swap is localised: the content address is computed
/// from the PLAINTEXT (before wrap) so it stays stable when the wrap becomes real.
pub trait ContentWrap: Send + Sync {
    /// Wrap (encrypt) plaintext into the bytes stored on disk. Identity on the M0 floor.
    fn wrap(&self, tenant: &TenantId, plaintext: &[u8]) -> Vec<u8>;
    /// Unwrap (decrypt) the stored bytes back to plaintext. Identity on the M0 floor.
    fn unwrap(&self, tenant: &TenantId, stored: &[u8]) -> Vec<u8>;
}

/// The M0 identity wrap (plaintext-at-rest) — the named floor. P-ST-08 replaces this with the
/// DEK-wrapped implementation; nothing else in [`FsBlobStore`] changes (the address is
/// plaintext-derived, so the swap does not move any content address).
#[derive(Clone, Copy, Debug, Default)]
pub struct IdentityWrap;

impl ContentWrap for IdentityWrap {
    fn wrap(&self, _tenant: &TenantId, plaintext: &[u8]) -> Vec<u8> {
        plaintext.to_vec()
    }
    fn unwrap(&self, _tenant: &TenantId, stored: &[u8]) -> Vec<u8> {
        stored.to_vec()
    }
}

/// The `blob_integrity_fail` telemetry counter (storage.md §9 telemetry; the STOR-D7 / D-S8
/// signal: "a corrupt blob must emit a detection signal" — EI-01 §3, observability is part of
/// the pass). It is a storage-DOMAIN counter, distinct from the frozen 18-signal contract-1.8
/// survival set in `myelin-harness` (which this prompt does not extend); the drill reads this
/// counter to prove the corrupt read was detected, not silently served.
#[derive(Debug, Default)]
pub struct BlobTelemetry {
    /// Count of integrity failures detected on read (the corrupt-serve-refused events).
    blob_integrity_fail: AtomicU64,
}

impl BlobTelemetry {
    /// The current `blob_integrity_fail` count — the STOR-D7 detection signal.
    pub fn blob_integrity_fail(&self) -> u64 {
        self.blob_integrity_fail.load(Ordering::SeqCst)
    }

    // MR-009b W7.3 — only the `test-support`-gated `FsBlobStore::get` records integrity fails on
    // this counter (the `blob_integrity_fail()` getter above stays always-compiled).
    #[cfg(any(test, feature = "test-support"))]
    fn record_integrity_fail(&self) {
        self.blob_integrity_fail.fetch_add(1, Ordering::SeqCst);
    }
}

/// The fs-backed BlobStore floor — an **in-memory-modelled filesystem** keyed by the same
/// `<tenant>/<algo>/<aa>/<rest>` key path a real on-disk store uses. This is the M0 floor: it
/// runs from the first commit (the prompt's "so it runs from the first commit") and proves
/// the content-address + per-tenant-keyspace + store-ciphertext + re-hash-on-read semantics
/// without a real Postgres/MinIO. The object-store backing (P-ST-30) and the real on-disk
/// backing are one-line swaps behind the [`BlobStore`] trait (the seam is the point).
///
/// The store deliberately keeps the **key-path string** (not a flat `(tenant, hash)` tuple)
/// as its map key so the per-tenant fan-out is exactly what an on-disk / object-store backing
/// would use — the floor models the real layout, not a shortcut.
///
/// **MR-009b W7.3 — `test-support`-gated TEST DOUBLE.** This `Mutex<HashMap<String, Vec<u8>>>`
/// floor is NOT byte-durable (its bytes die with the process); it survives from the first commit
/// as the DB-free unit/drill backing. The DURABLE production backing is
/// [`crate::s3blob::S3BlobStore`] (always-compiled, real `aws-sdk-s3`), config-selected via
/// [`crate::provider::SubstrateProvider::blob_store`] → [`crate::backend::blob_store`]. So this
/// type is gated behind `#[cfg(any(test, feature = "test-support"))]`: it is the test double
/// every drill/replica test drives via the [`BlobStore`] trait; downstream crates reach it via
/// the `myelin-storage/test-support` dev-dependency. Flipped GREEN out of the production graph
/// (SI-014/015/029 — the false "already byte-durable" premise corrected; P-ST-30 is the object
/// backing).
#[cfg(any(test, feature = "test-support"))]
pub struct FsBlobStore {
    /// The modelled object filesystem: key path → stored (wrapped) bytes.
    objects: Mutex<HashMap<String, Vec<u8>>>,
    /// The per-blob content-key wrap (identity on the M0 floor; real DEK wrap → P-ST-08).
    wrap: Box<dyn ContentWrap>,
    /// The `blob_integrity_fail` detection signal (STOR-D7).
    telemetry: BlobTelemetry,
}

#[cfg(any(test, feature = "test-support"))]
impl Default for FsBlobStore {
    fn default() -> Self {
        FsBlobStore::new()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl FsBlobStore {
    /// A fresh fs-backed store with the M0 identity wrap (plaintext-at-rest floor).
    pub fn new() -> FsBlobStore {
        FsBlobStore {
            objects: Mutex::new(HashMap::new()),
            wrap: Box::new(IdentityWrap),
            telemetry: BlobTelemetry::default(),
        }
    }

    /// A fs-backed store with a custom [`ContentWrap`] — the seam P-ST-08 uses to swap in the
    /// real per-blob-key-wrapped-by-DEK encryption (the "store ciphertext" half of §3.2).
    pub fn with_wrap(wrap: Box<dyn ContentWrap>) -> FsBlobStore {
        FsBlobStore {
            objects: Mutex::new(HashMap::new()),
            wrap,
            telemetry: BlobTelemetry::default(),
        }
    }

    /// The `blob_integrity_fail` telemetry the STOR-D7 drill asserts on.
    pub fn telemetry(&self) -> &BlobTelemetry {
        &self.telemetry
    }

    /// The per-tenant key path for an address: `<tenant>/<algo>/<aa>/<rest>` — a Git-style
    /// two-char fan-out within the tenant's keyspace. The `<tenant>/` prefix IS the per-tenant
    /// isolation: two tenants' identical content addresses produce DIFFERENT key paths, so the
    /// same bytes are stored twice (per-tenant dedup, no cross-tenant share — §3.2).
    fn key_path(tenant: &TenantId, hash: &ContentHash) -> String {
        let digest = &hash.digest_hex;
        // Git/Venti fan-out: first two hex chars as a directory, the rest as the file.
        let (fan, rest) = if digest.len() >= 2 {
            digest.split_at(2)
        } else {
            (digest.as_str(), "")
        };
        format!("{}/{}/{}/{}", tenant.0, hash.algo.tag(), fan, rest)
    }

    /// Test-only: corrupt the stored bytes at an address (flip them) WITHOUT touching the key
    /// — the STOR-D7 drill's "corrupt an fs-BlobStore object" step. This is how the drill
    /// proves re-hash-on-read catches a bit-rot / tampered object. Returns whether an object
    /// was present to corrupt.
    #[doc(hidden)]
    pub fn corrupt_for_drill(&self, tenant: &TenantId, hash: &ContentHash) -> bool {
        let path = Self::key_path(tenant, hash);
        let mut objects = self.objects.lock().expect("blob store mutex");
        if let Some(bytes) = objects.get_mut(&path) {
            // Replace the contents with bytes that will NOT re-hash to the address. Appending a
            // sentinel changes the BLAKE3 digest with overwhelming probability and works for an
            // empty object too (it becomes non-empty), guaranteeing a mismatch with no branch.
            bytes.push(0xFF);
            true
        } else {
            false
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl BlobStore for FsBlobStore {
    fn put(&self, tenant: &TenantId, bytes: &[u8]) -> Result<ContentHash> {
        // (a) BLAKE3 hash-on-write — the address is computed from the PLAINTEXT.
        let hash = ContentHash::blake3(bytes);
        // (b) store CIPHERTEXT (identity wrap on the M0 floor) under the per-tenant key path.
        let stored = self.wrap.wrap(tenant, bytes);
        let path = Self::key_path(tenant, &hash);
        let mut objects = self.objects.lock().expect("blob store mutex");
        // Per-tenant dedup + content-addressed OVERWRITE: the key is the content address, so
        // re-putting it with the SAME-addressed bytes is idempotent (dedup), and re-putting the
        // CORRECT bytes over a CORRUPT object at that address is a valid HEAL (the address proves
        // the bytes). We overwrite rather than `or_insert` so the replica-recovery heal path
        // (P-ST-30) restores the primary — matching S3's overwrite-on-PUT semantics so the
        // fs↔object swap behaves identically. (A re-put of correct bytes over correct bytes is a
        // no-op write of equal content; the dedup property — one stored object per address —
        // holds.)
        objects.insert(path, stored);
        Ok(hash)
    }

    fn get(&self, tenant: &TenantId, hash: &ContentHash) -> Result<Vec<u8>> {
        let path = Self::key_path(tenant, hash);
        let stored = {
            let objects = self.objects.lock().expect("blob store mutex");
            objects
                .get(&path)
                .cloned()
                .ok_or_else(|| BlobError::NotFound {
                    tenant: tenant.clone(),
                    hash: hash.clone(),
                })?
        };
        // Unwrap (decrypt) the stored ciphertext back to plaintext (identity on the floor).
        let plaintext = self.wrap.unwrap(tenant, &stored);
        // (c) RE-HASH-ON-READ integrity: recompute the address under the blob's OWN algorithm
        // (the self-describing tag drives verification) and refuse to serve on a mismatch.
        let actual = match hash.algo.rehash(&plaintext) {
            Ok(actual) => actual,
            Err(e) => {
                // An un-verifiable algorithm (SHA-256 floor) is NOT a silent serve — it is an
                // explicit refusal so the integrity gate is never bypassed.
                self.telemetry.record_integrity_fail();
                return Err(e);
            }
        };
        if &actual != hash {
            // 0 silent serve: increment the detection signal and REFUSE (never wrong bytes).
            self.telemetry.record_integrity_fail();
            return Err(BlobError::IntegrityFail {
                requested: hash.clone(),
                actual,
            });
        }
        Ok(plaintext)
    }

    fn head(&self, tenant: &TenantId, hash: &ContentHash) -> Result<BlobMeta> {
        let path = Self::key_path(tenant, hash);
        let objects = self.objects.lock().expect("blob store mutex");
        let stored = objects.get(&path).ok_or_else(|| BlobError::NotFound {
            tenant: tenant.clone(),
            hash: hash.clone(),
        })?;
        Ok(BlobMeta {
            hash: hash.clone(),
            stored_len: stored.len(),
        })
    }

    fn delete(&self, tenant: &TenantId, hash: &ContentHash) -> Result<()> {
        let path = Self::key_path(tenant, hash);
        let mut objects = self.objects.lock().expect("blob store mutex");
        objects
            .remove(&path)
            .map(|_| ())
            .ok_or_else(|| BlobError::NotFound {
                tenant: tenant.clone(),
                hash: hash.clone(),
            })
    }
}

/// A shared-backing blanket impl: an `Arc<B>` IS a `BlobStore` (it forwards to the inner `B`). This
/// lets multiple tenant-pinned consumers (e.g. the Search object-store index backstop, SRCH-P30)
/// share ONE underlying backing without forking the trait or making every backing `Clone`. It is a
/// pure forward — no new semantics — so the fs↔object swap and the per-tenant keyspace are
/// unchanged.
impl<B: BlobStore> BlobStore for std::sync::Arc<B> {
    fn put(&self, tenant: &TenantId, bytes: &[u8]) -> Result<ContentHash> {
        (**self).put(tenant, bytes)
    }
    fn get(&self, tenant: &TenantId, hash: &ContentHash) -> Result<Vec<u8>> {
        (**self).get(tenant, hash)
    }
    fn head(&self, tenant: &TenantId, hash: &ContentHash) -> Result<BlobMeta> {
        (**self).head(tenant, hash)
    }
    fn delete(&self, tenant: &TenantId, hash: &ContentHash) -> Result<()> {
        (**self).delete(tenant, hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant(s: &str) -> TenantId {
        TenantId(s.to_string())
    }

    /// put→get round-trips the EXACT bytes, and the content address equals the BLAKE3
    /// multihash of those bytes (the content address IS the hash).
    #[test]
    fn put_get_round_trips_exact_bytes_and_address_is_blake3_multihash() {
        let store = FsBlobStore::new();
        let acme = tenant("acme");
        let bytes = b"the quick brown fox";

        let h = store.put(&acme, bytes).expect("put");
        // The address is the self-describing BLAKE3 multihash.
        assert_eq!(h.algo, HashAlgo::Blake3);
        assert_eq!(h, ContentHash::blake3(bytes));
        assert!(h.to_multihash_string().starts_with("blake3:"));

        let got = store.get(&acme, &h).expect("get round-trips");
        assert_eq!(got, bytes, "get must return the exact bytes put");
    }

    #[test]
    fn bounded_get_accepts_exact_length_and_rejects_one_over_before_read() {
        let store = FsBlobStore::new();
        let acme = tenant("acme");
        let bytes = b"bounded";
        let hash = store.put(&acme, bytes).expect("put");

        assert_eq!(
            store
                .get_bounded(&acme, &hash, bytes.len())
                .expect("exact limit accepted"),
            bytes
        );
        assert_eq!(
            store.get_bounded(&acme, &hash, bytes.len() - 1),
            Err(BlobError::ReadLimitExceeded {
                actual: bytes.len(),
                maximum: bytes.len() - 1,
            })
        );
    }

    /// The multihash prefix is self-describing and parses round-trip — the property that lets
    /// SHA-256 coexist (§3.2). An unknown tag is an explicit error, never a silent mis-hash.
    #[test]
    fn multihash_prefix_is_self_describing_and_parses() {
        let h = ContentHash::blake3(b"x");
        let s = h.to_multihash_string();
        let parsed = ContentHash::parse(&s).expect("round-trip parse");
        assert_eq!(parsed, h);

        // SHA-256 is an admitted tag (so Git-imported blobs coexist).
        assert_eq!(HashAlgo::from_tag("sha256"), Some(HashAlgo::Sha256));
        // An unknown algorithm is an explicit no.
        assert!(matches!(
            ContentHash::parse("md5:abcd"),
            Err(BlobError::UnknownAlgo(_))
        ));
        // A non-hex digest is rejected at parse.
        assert!(matches!(
            ContentHash::parse("blake3:nothex!!"),
            Err(BlobError::MalformedAddress(_))
        ));
    }

    /// **Per-tenant dedup, no cross-tenant share (§3.2).** Two tenants storing IDENTICAL bytes
    /// get the same content address but TWO stored objects — deliberate residency isolation,
    /// asserted as a property (the prompt: "assert it in a test").
    #[test]
    fn two_tenants_identical_bytes_get_two_stored_objects() {
        let store = FsBlobStore::new();
        let acme = tenant("acme");
        let globex = tenant("globex");
        let bytes = b"shared-looking bytes";

        let h_acme = store.put(&acme, bytes).expect("acme put");
        let h_globex = store.put(&globex, bytes).expect("globex put");

        // Same content address (the hash is content-derived, tenant-independent)...
        assert_eq!(h_acme, h_globex);

        // ...but stored under DIFFERENT key paths — two physical objects, no cross-tenant share.
        let path_acme = FsBlobStore::key_path(&acme, &h_acme);
        let path_globex = FsBlobStore::key_path(&globex, &h_globex);
        assert_ne!(path_acme, path_globex);
        {
            let objects = store.objects.lock().unwrap();
            assert!(objects.contains_key(&path_acme));
            assert!(objects.contains_key(&path_globex));
            assert_eq!(objects.len(), 2, "two tenants => two stored objects");
        }

        // Deleting acme's object leaves globex's intact (no shared backing).
        store.delete(&acme, &h_acme).expect("delete acme");
        assert!(matches!(
            store.get(&acme, &h_acme),
            Err(BlobError::NotFound { .. })
        ));
        assert_eq!(
            store.get(&globex, &h_globex).expect("globex survives"),
            bytes
        );
    }

    /// Per-tenant DEDUP within a tenant: putting the same bytes twice stores once.
    #[test]
    fn within_tenant_put_is_deduped() {
        let store = FsBlobStore::new();
        let acme = tenant("acme");
        let h1 = store.put(&acme, b"dup").expect("put 1");
        let h2 = store.put(&acme, b"dup").expect("put 2");
        assert_eq!(h1, h2);
        let objects = store.objects.lock().unwrap();
        assert_eq!(objects.len(), 1, "within-tenant dedup: one stored object");
    }

    /// **STOR-D7 (the blob-integrity floor).** Corrupt a stored object → re-hash-on-read
    /// detects the content-address mismatch, REFUSES to serve (0 silent wrong-bytes return),
    /// and `blob_integrity_fail` increments. The single most load-bearing assertion of this
    /// module (mandatory-core).
    #[test]
    fn get_on_corrupted_object_refuses_to_serve_and_signals_integrity_fail() {
        let store = FsBlobStore::new();
        let acme = tenant("acme");
        let h = store.put(&acme, b"trustworthy bytes").expect("put");

        // Sanity: a clean read serves the bytes and does NOT signal.
        assert_eq!(
            store.get(&acme, &h).expect("clean read"),
            b"trustworthy bytes"
        );
        assert_eq!(store.telemetry().blob_integrity_fail(), 0);

        // Corrupt the stored object (bit-rot / tamper), keeping its key.
        assert!(
            store.corrupt_for_drill(&acme, &h),
            "object present to corrupt"
        );

        // Re-hash-on-read DETECTS the mismatch and REFUSES — 0 silent serve.
        match store.get(&acme, &h) {
            Err(BlobError::IntegrityFail { requested, actual }) => {
                assert_eq!(requested, h);
                assert_ne!(actual, h, "the corrupt bytes hash to a different address");
            }
            Ok(bytes) => panic!("SILENT WRONG-BYTES SERVE — STOR-D7 floor breached: {bytes:?}"),
            Err(other) => panic!("expected IntegrityFail, got {other}"),
        }
        // The detection signal incremented (observability is part of the pass — EI-01 §3).
        assert_eq!(
            store.telemetry().blob_integrity_fail(),
            1,
            "blob_integrity_fail must increment on a corrupt read"
        );
    }

    /// The key path is the per-tenant Git-style fan-out `<tenant>/<algo>/<aa>/<rest>` for a
    /// normal (64-hex) BLAKE3 digest, and degrades safely for a pathologically short digest
    /// (exercises the `< 2` guard branch — no panic, still tenant/algo-scoped). This pins the
    /// fan-out shape the on-disk / object-store backing reproduces.
    #[test]
    fn key_path_is_per_tenant_fanout_and_handles_short_digests() {
        let h = ContentHash::blake3(b"x");
        let path = FsBlobStore::key_path(&tenant("acme"), &h);
        // <tenant>/<algo>/<2-char fan>/<rest>
        let parts: Vec<&str> = path.split('/').collect();
        assert_eq!(parts[0], "acme");
        assert_eq!(parts[1], "blake3");
        assert_eq!(parts[2].len(), 2, "two-char Git-style fan-out dir");
        assert_eq!(format!("{}{}", parts[2], parts[3]), h.digest_hex);

        // A short (1-char) digest must not panic and stays tenant/algo-scoped (the guard branch).
        let short = ContentHash {
            algo: HashAlgo::Blake3,
            digest_hex: "a".to_string(),
        };
        let short_path = FsBlobStore::key_path(&tenant("acme"), &short);
        assert_eq!(short_path, "acme/blake3/a/");
    }

    /// Every `BlobError` renders a loud, specific message (so a corrupt-serve refusal is
    /// diagnosable, EI-01 §3) — the integrity-fail message names both addresses.
    #[test]
    fn errors_display_loud_and_specific() {
        let req = ContentHash::blake3(b"a");
        let act = ContentHash::blake3(b"b");
        let integrity = BlobError::IntegrityFail {
            requested: req.clone(),
            actual: act.clone(),
        }
        .to_string();
        assert!(integrity.contains("integrity fail"), "{integrity}");
        assert!(integrity.contains("serve refused"), "{integrity}");
        assert!(integrity.contains(&req.digest_hex) && integrity.contains(&act.digest_hex));

        assert!(BlobError::NotFound {
            tenant: tenant("acme"),
            hash: req.clone(),
        }
        .to_string()
        .contains("not found"));
        assert!(BlobError::MalformedAddress("zz".into())
            .to_string()
            .contains("malformed"));
        assert!(BlobError::UnknownAlgo("md5".into())
            .to_string()
            .contains("unknown"));
        assert!(BlobError::AlgoNotVerifiable(HashAlgo::Sha256)
            .to_string()
            .contains("no on-floor verification"));
    }

    /// **SHA-256 verification is now LIVE (P-ST-22 closes the P-ST-03 floor).** A `sha256:`-tagged
    /// blob (a git-imported object) re-hashes under SHA-256 on read: a CORRECT object round-trips
    /// the exact bytes; a CORRUPT one is detected as an `IntegrityFail` and refused (0 silent
    /// serve), incrementing `blob_integrity_fail`. This is the git-object integrity floor the
    /// blob.rs module named ("SHA-256 verification rides in with the git object import — P-ST-22").
    #[test]
    fn sha256_blob_verifies_correct_and_refuses_corrupt() {
        let store = FsBlobStore::new();
        let acme = tenant("acme");
        // A SHA-256-addressed object (the git world). Address it by the SHA-256 of its bytes.
        let object = b"blob 11\0hello world";
        let h = ContentHash::sha256(object);
        assert_eq!(h.algo, HashAlgo::Sha256);
        let path = FsBlobStore::key_path(&acme, &h);
        store.objects.lock().unwrap().insert(path, object.to_vec());

        // A correct SHA-256 object verifies + serves the exact bytes (no false positive).
        assert_eq!(
            store.get(&acme, &h).expect("sha256 object verifies"),
            object
        );
        assert_eq!(store.telemetry().blob_integrity_fail(), 0);

        // Corrupt it → re-hash-on-read (SHA-256) detects + refuses (0 silent serve).
        assert!(store.corrupt_for_drill(&acme, &h));
        match store.get(&acme, &h) {
            Err(BlobError::IntegrityFail { requested, actual }) => {
                assert_eq!(requested, h);
                assert_eq!(
                    actual.algo,
                    HashAlgo::Sha256,
                    "verified under the blob's own tag"
                );
                assert_ne!(actual, h);
            }
            other => panic!("a corrupt sha256 object must be refused, got {other:?}"),
        }
        assert_eq!(store.telemetry().blob_integrity_fail(), 1);
    }

    /// head returns PII-free metadata without serving the bytes; NotFound is explicit.
    #[test]
    fn head_returns_meta_and_not_found_is_explicit() {
        let store = FsBlobStore::new();
        let acme = tenant("acme");
        let h = store.put(&acme, b"abc").expect("put");
        let meta = store.head(&acme, &h).expect("head");
        assert_eq!(meta.hash, h);
        assert_eq!(meta.stored_len, 3); // identity wrap => stored len == plaintext len

        let absent = ContentHash::blake3(b"never stored");
        assert!(matches!(
            store.head(&acme, &absent),
            Err(BlobError::NotFound { .. })
        ));
    }

    /// The ContentWrap seam is real: a non-identity wrap stores DIFFERENT bytes than the
    /// plaintext (proving "store ciphertext"), yet the content address (plaintext-derived)
    /// is unchanged and re-hash-on-read still verifies. This is the localised seam P-ST-08
    /// swaps the real DEK wrap into.
    #[test]
    fn content_wrap_seam_stores_ciphertext_while_address_stays_plaintext_derived() {
        /// A trivial reversible "cipher" (XOR 0x5A) standing in for the P-ST-08 DEK wrap — it
        /// proves the seam stores ciphertext and the address stays plaintext-derived.
        struct XorWrap;
        impl ContentWrap for XorWrap {
            fn wrap(&self, _t: &TenantId, p: &[u8]) -> Vec<u8> {
                p.iter().map(|b| b ^ 0x5A).collect()
            }
            fn unwrap(&self, _t: &TenantId, s: &[u8]) -> Vec<u8> {
                s.iter().map(|b| b ^ 0x5A).collect()
            }
        }

        let store = FsBlobStore::with_wrap(Box::new(XorWrap));
        let acme = tenant("acme");
        let plaintext = b"secret payload";
        let h = store.put(&acme, plaintext).expect("put");

        // The address is the PLAINTEXT hash (stable across the wrap) — store ciphertext, not
        // plaintext.
        assert_eq!(h, ContentHash::blake3(plaintext));
        {
            let objects = store.objects.lock().unwrap();
            let stored = objects.values().next().expect("one object");
            assert_ne!(
                stored.as_slice(),
                plaintext,
                "must store CIPHERTEXT, not plaintext"
            );
        }
        // get unwraps + re-hash-verifies and returns the exact plaintext.
        assert_eq!(store.get(&acme, &h).expect("get"), plaintext);
    }
}
