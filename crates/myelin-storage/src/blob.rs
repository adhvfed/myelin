#[cfg(any(test, feature = "test-support"))]
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(any(test, feature = "test-support"))]
use std::sync::Mutex;

use myelin_tenancy::TenantId;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
pub struct ContentHash {
    algo: HashAlgo,
    digest_hex: String,
}

impl ContentHash {
    const DIGEST_HEX_LEN: usize = 64;

    pub fn blake3(bytes: &[u8]) -> ContentHash {
        let digest = blake3::hash(bytes);
        ContentHash {
            algo: HashAlgo::Blake3,
            digest_hex: hex::encode(digest.as_bytes()),
        }
    }

    pub fn sha256(bytes: &[u8]) -> ContentHash {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(bytes);
        ContentHash {
            algo: HashAlgo::Sha256,
            digest_hex: hex::encode(digest),
        }
    }

    pub fn to_multihash_string(&self) -> String {
        format!("{}:{}", self.algo.tag(), self.digest_hex)
    }

    pub fn algorithm(&self) -> HashAlgo {
        self.algo
    }

    pub fn digest_hex(&self) -> &str {
        &self.digest_hex
    }

    pub fn parse(s: &str) -> std::result::Result<ContentHash, BlobError> {
        let (tag, hex_part) = s
            .split_once(':')
            .ok_or_else(|| BlobError::MalformedAddress(s.to_string()))?;
        let algo =
            HashAlgo::from_tag(tag).ok_or_else(|| BlobError::UnknownAlgo(tag.to_string()))?;
        ContentHash::from_parts(algo, hex_part.to_string())
    }

    fn from_parts(algo: HashAlgo, digest_hex: String) -> Result<ContentHash> {
        let is_canonical = digest_hex.len() == Self::DIGEST_HEX_LEN
            && digest_hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if !is_canonical {
            return Err(BlobError::MalformedAddress(format!(
                "{}:{digest_hex}",
                algo.tag()
            )));
        }
        Ok(ContentHash { algo, digest_hex })
    }
}

#[derive(serde::Deserialize)]
struct ContentHashWire {
    algo: HashAlgo,
    digest_hex: String,
}

impl<'de> serde::Deserialize<'de> for ContentHash {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = <ContentHashWire as serde::Deserialize>::deserialize(deserializer)?;
        ContentHash::from_parts(wire.algo, wire.digest_hex).map_err(serde::de::Error::custom)
    }
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum HashAlgo {
    Blake3,
    Sha256,
}

impl HashAlgo {
    pub fn tag(self) -> &'static str {
        match self {
            HashAlgo::Blake3 => "blake3",
            HashAlgo::Sha256 => "sha256",
        }
    }

    pub fn from_tag(tag: &str) -> Option<HashAlgo> {
        match tag {
            "blake3" => Some(HashAlgo::Blake3),
            "sha256" => Some(HashAlgo::Sha256),
            _ => None,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    fn rehash(self, bytes: &[u8]) -> std::result::Result<ContentHash, BlobError> {
        match self {
            HashAlgo::Blake3 => Ok(ContentHash::blake3(bytes)),
            HashAlgo::Sha256 => Ok(ContentHash::sha256(bytes)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobMeta {
    pub hash: ContentHash,
    pub stored_len: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlobError {
    NotFound {
        tenant: TenantId,
        hash: ContentHash,
    },
    IntegrityFail {
        requested: ContentHash,
        actual: ContentHash,
    },
    Backend(BlobDependencyError),
    SizeLimitExceeded {
        actual: usize,
        maximum: usize,
    },
    MalformedAddress(String),
    UnknownAlgo(String),
    AlgoNotVerifiable(HashAlgo),
}

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
                "blob integrity fail: requested {} but stored bytes hash to {} - serve refused",
                requested.to_multihash_string(),
                actual.to_multihash_string()
            ),
            BlobError::Backend(kind) => kind.fmt(f),
            BlobError::SizeLimitExceeded { actual, maximum } => write!(
                f,
                "blob operation refused: {actual} bytes exceeds the {maximum}-byte limit"
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

pub type Result<T> = std::result::Result<T, BlobError>;

pub trait BlobStore {
    fn put(&self, tenant: &TenantId, bytes: &[u8]) -> Result<ContentHash>;

    fn get(&self, tenant: &TenantId, hash: &ContentHash) -> Result<Vec<u8>>;

    fn get_bounded(
        &self,
        tenant: &TenantId,
        hash: &ContentHash,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>> {
        let metadata = self.head(tenant, hash)?;
        if metadata.stored_len > maximum_bytes {
            return Err(BlobError::SizeLimitExceeded {
                actual: metadata.stored_len,
                maximum: maximum_bytes,
            });
        }
        let bytes = self.get(tenant, hash)?;
        if bytes.len() > maximum_bytes {
            return Err(BlobError::SizeLimitExceeded {
                actual: bytes.len(),
                maximum: maximum_bytes,
            });
        }
        Ok(bytes)
    }

    fn head(&self, tenant: &TenantId, hash: &ContentHash) -> Result<BlobMeta>;

    fn delete(&self, tenant: &TenantId, hash: &ContentHash) -> Result<()>;
}

pub trait ContentWrap: Send + Sync {
    fn wrap(&self, tenant: &TenantId, plaintext: &[u8]) -> Vec<u8>;
    fn unwrap(&self, tenant: &TenantId, stored: &[u8]) -> Vec<u8>;
}

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

#[derive(Debug, Default)]
pub struct BlobTelemetry {
    blob_integrity_fail: AtomicU64,
}

impl BlobTelemetry {
    pub fn blob_integrity_fail(&self) -> u64 {
        self.blob_integrity_fail.load(Ordering::SeqCst)
    }

    #[cfg(any(test, feature = "test-support"))]
    fn record_integrity_fail(&self) {
        self.blob_integrity_fail.fetch_add(1, Ordering::SeqCst);
    }
}

#[cfg(any(test, feature = "test-support"))]
pub struct FsBlobStore {
    objects: Mutex<HashMap<String, Vec<u8>>>,
    wrap: Box<dyn ContentWrap>,
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
    pub fn new() -> FsBlobStore {
        FsBlobStore {
            objects: Mutex::new(HashMap::new()),
            wrap: Box::new(IdentityWrap),
            telemetry: BlobTelemetry::default(),
        }
    }

    pub fn with_wrap(wrap: Box<dyn ContentWrap>) -> FsBlobStore {
        FsBlobStore {
            objects: Mutex::new(HashMap::new()),
            wrap,
            telemetry: BlobTelemetry::default(),
        }
    }

    pub fn telemetry(&self) -> &BlobTelemetry {
        &self.telemetry
    }

    fn key_path(tenant: &TenantId, hash: &ContentHash) -> String {
        let (fan, rest) = hash.digest_hex().split_at(2);
        format!("{}/{}/{}/{}", tenant.0, hash.algorithm().tag(), fan, rest)
    }

    #[doc(hidden)]
    pub fn corrupt_for_drill(&self, tenant: &TenantId, hash: &ContentHash) -> bool {
        let path = Self::key_path(tenant, hash);
        let mut objects = self.objects.lock().expect("blob store mutex");
        if let Some(bytes) = objects.get_mut(&path) {
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
        let hash = ContentHash::blake3(bytes);
        let stored = self.wrap.wrap(tenant, bytes);
        let path = Self::key_path(tenant, &hash);
        let mut objects = self.objects.lock().expect("blob store mutex");
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
        let plaintext = self.wrap.unwrap(tenant, &stored);
        let actual = match hash.algorithm().rehash(&plaintext) {
            Ok(actual) => actual,
            Err(e) => {
                self.telemetry.record_integrity_fail();
                return Err(e);
            }
        };
        if &actual != hash {
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

    #[test]
    fn put_get_round_trips_exact_bytes_and_address_is_blake3_multihash() {
        let store = FsBlobStore::new();
        let acme = tenant("acme");
        let bytes = b"the quick brown fox";

        let h = store.put(&acme, bytes).expect("put");
        assert_eq!(h.algorithm(), HashAlgo::Blake3);
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
            Err(BlobError::SizeLimitExceeded {
                actual: bytes.len(),
                maximum: bytes.len() - 1,
            })
        );
    }

    #[test]
    fn multihash_prefix_is_self_describing_and_parses() {
        let h = ContentHash::blake3(b"x");
        let s = h.to_multihash_string();
        let parsed = ContentHash::parse(&s).expect("round-trip parse");
        assert_eq!(parsed, h);
        assert_eq!(parsed.algorithm(), HashAlgo::Blake3);
        assert_eq!(parsed.digest_hex().len(), ContentHash::DIGEST_HEX_LEN);

        assert_eq!(HashAlgo::from_tag("sha256"), Some(HashAlgo::Sha256));
        assert!(matches!(
            ContentHash::parse("md5:abcd"),
            Err(BlobError::UnknownAlgo(_))
        ));
    }

    #[test]
    fn parsing_rejects_every_noncanonical_digest_shape() {
        let noncanonical = [
            "blake3:",
            "blake3:a",
            "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "blake3:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "blake3:gggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggg",
            "sha256:00000000000000000000000000000000000000000000000000000000000000000",
        ];

        for address in noncanonical {
            assert!(
                matches!(
                    ContentHash::parse(address),
                    Err(BlobError::MalformedAddress(_))
                ),
                "accepted noncanonical address {address:?}"
            );
        }
    }

    #[test]
    fn deserialization_preserves_the_wire_shape_without_bypassing_validation() {
        let hash = ContentHash::sha256(b"wire format");
        let encoded = serde_json::to_value(&hash).expect("serialize content hash");
        assert_eq!(encoded["algo"], "Sha256");
        assert_eq!(encoded["digest_hex"], hash.digest_hex());
        assert_eq!(
            serde_json::from_value::<ContentHash>(encoded).expect("valid hash round-trips"),
            hash
        );

        let invalid = serde_json::json!({
            "algo": "Blake3",
            "digest_hex": "a",
        });
        assert!(
            serde_json::from_value::<ContentHash>(invalid).is_err(),
            "deserialization must not construct an invalid content address"
        );
    }

    #[test]
    fn two_tenants_identical_bytes_get_two_stored_objects() {
        let store = FsBlobStore::new();
        let acme = tenant("acme");
        let globex = tenant("globex");
        let bytes = b"shared-looking bytes";

        let h_acme = store.put(&acme, bytes).expect("acme put");
        let h_globex = store.put(&globex, bytes).expect("globex put");

        assert_eq!(h_acme, h_globex);

        let path_acme = FsBlobStore::key_path(&acme, &h_acme);
        let path_globex = FsBlobStore::key_path(&globex, &h_globex);
        assert_ne!(path_acme, path_globex);
        {
            let objects = store.objects.lock().unwrap();
            assert!(objects.contains_key(&path_acme));
            assert!(objects.contains_key(&path_globex));
            assert_eq!(objects.len(), 2, "two tenants => two stored objects");
        }

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

    #[test]
    fn get_on_corrupted_object_refuses_to_serve_and_signals_integrity_fail() {
        let store = FsBlobStore::new();
        let acme = tenant("acme");
        let h = store.put(&acme, b"trustworthy bytes").expect("put");

        assert_eq!(
            store.get(&acme, &h).expect("clean read"),
            b"trustworthy bytes"
        );
        assert_eq!(store.telemetry().blob_integrity_fail(), 0);

        assert!(
            store.corrupt_for_drill(&acme, &h),
            "object present to corrupt"
        );

        match store.get(&acme, &h) {
            Err(BlobError::IntegrityFail { requested, actual }) => {
                assert_eq!(requested, h);
                assert_ne!(actual, h, "the corrupt bytes hash to a different address");
            }
            Ok(bytes) => panic!("SILENT WRONG-BYTES SERVE - STOR-D7 floor breached: {bytes:?}"),
            Err(other) => panic!("expected IntegrityFail, got {other}"),
        }
        assert_eq!(
            store.telemetry().blob_integrity_fail(),
            1,
            "blob_integrity_fail must increment on a corrupt read"
        );
    }

    #[test]
    fn key_path_is_per_tenant_with_canonical_fanout() {
        let h = ContentHash::blake3(b"x");
        let path = FsBlobStore::key_path(&tenant("acme"), &h);
        let parts: Vec<&str> = path.split('/').collect();
        assert_eq!(parts[0], "acme");
        assert_eq!(parts[1], "blake3");
        assert_eq!(parts[2].len(), 2, "two-char Git-style fan-out dir");
        assert_eq!(format!("{}{}", parts[2], parts[3]), h.digest_hex());
    }

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
        assert!(integrity.contains(req.digest_hex()) && integrity.contains(act.digest_hex()));

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

    #[test]
    fn sha256_blob_verifies_correct_and_refuses_corrupt() {
        let store = FsBlobStore::new();
        let acme = tenant("acme");
        let object = b"blob 11\0hello world";
        let h = ContentHash::sha256(object);
        assert_eq!(h.algorithm(), HashAlgo::Sha256);
        let path = FsBlobStore::key_path(&acme, &h);
        store.objects.lock().unwrap().insert(path, object.to_vec());

        assert_eq!(
            store.get(&acme, &h).expect("sha256 object verifies"),
            object
        );
        assert_eq!(store.telemetry().blob_integrity_fail(), 0);

        assert!(store.corrupt_for_drill(&acme, &h));
        match store.get(&acme, &h) {
            Err(BlobError::IntegrityFail { requested, actual }) => {
                assert_eq!(requested, h);
                assert_eq!(
                    actual.algorithm(),
                    HashAlgo::Sha256,
                    "verified under the blob's own tag"
                );
                assert_ne!(actual, h);
            }
            other => panic!("a corrupt sha256 object must be refused, got {other:?}"),
        }
        assert_eq!(store.telemetry().blob_integrity_fail(), 1);
    }

    #[test]
    fn head_returns_meta_and_not_found_is_explicit() {
        let store = FsBlobStore::new();
        let acme = tenant("acme");
        let h = store.put(&acme, b"abc").expect("put");
        let meta = store.head(&acme, &h).expect("head");
        assert_eq!(meta.hash, h);
        assert_eq!(meta.stored_len, 3);

        let absent = ContentHash::blake3(b"never stored");
        assert!(matches!(
            store.head(&acme, &absent),
            Err(BlobError::NotFound { .. })
        ));
    }

    #[test]
    fn content_wrap_seam_stores_ciphertext_while_address_stays_plaintext_derived() {
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
        assert_eq!(store.get(&acme, &h).expect("get"), plaintext);
    }
}
