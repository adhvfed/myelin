//! The secrets behind password sign-in: password hashing, the email lookup digest, device tokens,
//! and invitation codes. Nothing here touches storage, and nothing here logs.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine as _;
use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

pub(crate) const MIN_PASSWORD_CHARS: usize = 10;
pub(crate) const MAX_PASSWORD_CHARS: usize = 1024;
const MAX_PASSWORD_BYTES: usize = 4 * MAX_PASSWORD_CHARS;
const MAX_EMAIL_BYTES: usize = 254;
const CODE_BYTES: usize = 32;
const CODE_B64URL_LEN: usize = 43;
const EMAIL_NONCE_BYTES: usize = 12;

/// OWASP's recommended Argon2id configuration: 19 MiB, two passes, one lane.
const ARGON2_MEMORY_KIB: u32 = 19 * 1024;
const ARGON2_ITERATIONS: u32 = 2;
const ARGON2_LANES: u32 = 1;

fn argon2() -> Argon2<'static> {
    let params = Params::new(ARGON2_MEMORY_KIB, ARGON2_ITERATIONS, ARGON2_LANES, None)
        .expect("the fixed Argon2id parameters are valid");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// A password that met the policy. It is wiped from memory when dropped and never printed.
pub(crate) struct Password(Zeroizing<String>);

impl Password {
    /// Length is the only rule: 10 to 1024 characters, counted as Unicode scalar values.
    pub(crate) fn new(candidate: String) -> Result<Self, WeakPassword> {
        let candidate = Zeroizing::new(candidate);
        if candidate.len() > MAX_PASSWORD_BYTES {
            return Err(WeakPassword);
        }
        let chars = candidate.chars().count();
        if !(MIN_PASSWORD_CHARS..=MAX_PASSWORD_CHARS).contains(&chars) {
            return Err(WeakPassword);
        }
        Ok(Self(candidate))
    }

    /// A PHC string with a fresh random salt.
    pub(crate) fn hash(&self) -> String {
        let salt = SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
        argon2()
            .hash_password(self.0.as_bytes(), &salt)
            .expect("Argon2id hashing with valid parameters cannot fail")
            .to_string()
    }

    /// Checks this password against a stored PHC string. With no stored hash it still pays for a
    /// full verification against `dummy`, so an unknown email costs what a wrong password costs.
    ///
    /// Verification runs with the parameters written in the stored hash, so a hash this code
    /// would not have written (another algorithm, or a higher cost) is refused unverified.
    pub(crate) fn verify(&self, stored_phc: Option<&str>, dummy: &DummyHash) -> bool {
        let phc = stored_phc.unwrap_or(dummy.0.as_str());
        let Ok(parsed) = PasswordHash::new(phc) else {
            return false;
        };
        if !written_by_this_policy(&parsed) {
            return false;
        }
        let matched = argon2().verify_password(self.0.as_bytes(), &parsed).is_ok();
        matched && stored_phc.is_some()
    }
}

/// Argon2id v19 with a cost no higher than the one `hash` uses.
fn written_by_this_policy(hash: &PasswordHash<'_>) -> bool {
    if hash.algorithm != Algorithm::Argon2id.ident() || hash.version != Some(Version::V0x13.into())
    {
        return false;
    }
    Params::try_from(hash).is_ok_and(|params| {
        params.m_cost() <= ARGON2_MEMORY_KIB
            && params.t_cost() <= ARGON2_ITERATIONS
            && params.p_cost() <= ARGON2_LANES
    })
}

impl core::fmt::Debug for Password {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("Password(<redacted>)")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WeakPassword;

/// A hash of a random password nobody knows, verified against when the email has no account.
/// It is made once, when the service starts, so the first unknown-email sign-in is not slower
/// than the rest.
#[derive(Clone)]
pub(crate) struct DummyHash(String);

impl DummyHash {
    pub(crate) fn generate() -> Self {
        let mut random = [0_u8; 32];
        OsRng.fill_bytes(&mut random);
        Self(
            Password(Zeroizing::new(
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random),
            ))
            .hash(),
        )
    }

    #[cfg(test)]
    pub(crate) fn is_a_full_cost_hash(&self) -> bool {
        PasswordHash::new(&self.0).is_ok_and(|parsed| {
            written_by_this_policy(&parsed)
                && Params::try_from(&parsed).is_ok_and(|params| {
                    params.m_cost() == ARGON2_MEMORY_KIB && params.t_cost() == ARGON2_ITERATIONS
                })
        })
    }
}

/// A normalized email address: trimmed and lowercased, with a minimal shape check. It is the
/// person's to see, so it is redacted from debug output.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Email(String);

impl Email {
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        let email = raw.trim().to_lowercase();
        if email.is_empty()
            || email.len() > MAX_EMAIL_BYTES
            || email
                .chars()
                .any(|character| character.is_control() || character.is_whitespace())
        {
            return None;
        }
        let (local, domain) = email.split_once('@')?;
        if local.is_empty() || domain.is_empty() || domain.contains('@') {
            return None;
        }
        Some(Self(email))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Debug for Email {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("Email(<redacted>)")
    }
}

/// Keyed digest used to find an account by email without storing the address in the clear. The
/// key is derived from the cell's seal key, so the digest is useless without it.
#[derive(Clone)]
pub(crate) struct EmailDigester {
    key: Zeroizing<[u8; 32]>,
}

pub(crate) type EmailDigest = [u8; 32];

impl EmailDigester {
    const SEAL_KEY_CONTEXT: &'static str = "myelin 2026-11-24 account email digest v1";

    pub(crate) fn new(key: Zeroizing<[u8; 32]>) -> Self {
        Self { key }
    }

    pub(crate) fn digest(&self, email: &Email) -> EmailDigest {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(self.key.as_slice())
            .expect("HMAC accepts a 32-byte key");
        mac.update(b"myelin.edge.account.email.v1\0");
        mac.update(email.as_str().as_bytes());
        mac.finalize().into_bytes().into()
    }
}

/// The keys password accounts derive from the cell's seal key.
#[derive(Clone)]
pub struct AccountKeys {
    pub(crate) email: EmailDigester,
    pub(crate) devices: DeviceKey,
}

impl AccountKeys {
    pub fn derive(seal: &myelin_storage::SealKey) -> Self {
        Self {
            email: EmailDigester::new(seal.derive_service_key(EmailDigester::SEAL_KEY_CONTEXT)),
            devices: DeviceKey(seal.derive_service_key(DeviceKey::SEAL_KEY_CONTEXT)),
        }
    }

    #[cfg(test)]
    pub(crate) fn fixed(byte: u8) -> Self {
        Self {
            email: EmailDigester::new(Zeroizing::new([byte; 32])),
            devices: DeviceKey(Zeroizing::new([byte ^ 0xff; 32])),
        }
    }
}

pub(crate) type DeviceId = [u8; DEVICE_ID_BYTES];

const DEVICE_ID_BYTES: usize = 16;
const DEVICE_TOKEN_VERSION: u8 = 1;
const DEVICE_TOKEN_BYTES: usize = 1 + DEVICE_ID_BYTES + 8 + 32;
const DEVICE_TOKEN_B64URL_LEN: usize = 76;
pub(crate) const DEVICE_TOKEN_TTL_SECS: i64 = 365 * 24 * 60 * 60;
/// How far ahead of this Edge's clock another Edge may have issued a token.
const DEVICE_TOKEN_SKEW_SECS: i64 = 5 * 60;

/// Signs device tokens: proof that a browser once signed in to an email. A token binds the email
/// digest, a random device id and its issue time under a MAC. It is stored nowhere and grants
/// nothing but a sign-in allowance of its own, so strangers guessing at the email cannot use it up.
#[derive(Clone)]
pub(crate) struct DeviceKey(Zeroizing<[u8; 32]>);

impl DeviceKey {
    const SEAL_KEY_CONTEXT: &'static str = "myelin 2026-11-26 account device token v1";

    /// A token for `device`, or for a new device when `None`.
    pub(crate) fn issue(&self, email: &EmailDigest, device: Option<DeviceId>, now: i64) -> String {
        let device = device.unwrap_or_else(|| {
            let mut id = [0_u8; DEVICE_ID_BYTES];
            OsRng.fill_bytes(&mut id);
            id
        });
        let mut token = Vec::with_capacity(DEVICE_TOKEN_BYTES);
        token.push(DEVICE_TOKEN_VERSION);
        token.extend_from_slice(&device);
        token.extend_from_slice(&now.to_be_bytes());
        let mac = self.mac(email, &token).finalize().into_bytes();
        token.extend_from_slice(&mac);
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token)
    }

    /// The device a token names, if it was issued here for `email` and has not expired. Anything
    /// else, however malformed, is `None`.
    pub(crate) fn verify(&self, token: &str, email: &EmailDigest, now: i64) -> Option<DeviceId> {
        if token.len() != DEVICE_TOKEN_B64URL_LEN {
            return None;
        }
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(token)
            .ok()?;
        if bytes.len() != DEVICE_TOKEN_BYTES || bytes[0] != DEVICE_TOKEN_VERSION {
            return None;
        }
        let (signed, mac) = bytes.split_at(DEVICE_TOKEN_BYTES - 32);
        self.mac(email, signed).verify_slice(mac).ok()?;
        let device: DeviceId = signed[1..1 + DEVICE_ID_BYTES].try_into().ok()?;
        let issued_at = i64::from_be_bytes(signed[1 + DEVICE_ID_BYTES..].try_into().ok()?);
        (issued_at <= now.saturating_add(DEVICE_TOKEN_SKEW_SECS)
            && now < issued_at.saturating_add(DEVICE_TOKEN_TTL_SECS))
        .then_some(device)
    }

    fn mac(&self, email: &EmailDigest, signed: &[u8]) -> Hmac<Sha256> {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(self.0.as_slice())
            .expect("HMAC accepts a 32-byte key");
        mac.update(b"myelin.edge.account.device.v1\0");
        mac.update(email);
        mac.update(signed);
        mac
    }
}

/// The secret in an invitation link. The database keeps only its SHA-256 digest.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct InvitationCode([u8; CODE_BYTES]);

pub(crate) type CodeDigest = [u8; 32];

impl InvitationCode {
    pub(crate) fn generate() -> Self {
        let mut bytes = [0_u8; CODE_BYTES];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    /// Accepts only the canonical 43-character unpadded base64url form.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        if value.len() != CODE_B64URL_LEN
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return None;
        }
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(value)
            .ok()?;
        let bytes: [u8; CODE_BYTES] = decoded.try_into().ok()?;
        let code = Self(bytes);
        (code.encoded() == value).then_some(code)
    }

    pub(crate) fn encoded(&self) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(self.0)
    }

    pub(crate) fn digest(&self) -> CodeDigest {
        Sha256::digest(self.0).into()
    }

    /// Seals the invitee's email under a key only the link holder has, so the stored invitation
    /// reveals nothing to someone who reads the table.
    pub(crate) fn seal_email(&self, email: &Email) -> SealedEmail {
        let mut nonce = [0_u8; EMAIL_NONCE_BYTES];
        OsRng.fill_bytes(&mut nonce);
        let ciphertext = self
            .cipher()
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: email.as_str().as_bytes(),
                    aad: &self.digest(),
                },
            )
            .expect("AES-256-GCM encryption of a short email cannot fail");
        SealedEmail { nonce, ciphertext }
    }

    pub(crate) fn open_email(&self, sealed: &SealedEmail) -> Option<Email> {
        let plain = Zeroizing::new(
            self.cipher()
                .decrypt(
                    Nonce::from_slice(&sealed.nonce),
                    Payload {
                        msg: &sealed.ciphertext,
                        aad: &self.digest(),
                    },
                )
                .ok()?,
        );
        Email::parse(std::str::from_utf8(&plain).ok()?)
    }

    fn cipher(&self) -> Aes256Gcm {
        let key = Zeroizing::new(blake3::derive_key(
            "myelin 2026-11-24 invitation email seal v1",
            &self.0,
        ));
        Aes256Gcm::new_from_slice(key.as_slice()).expect("AES-256-GCM takes a 32-byte key")
    }
}

impl core::fmt::Debug for InvitationCode {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("InvitationCode(<redacted>)")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SealedEmail {
    pub(crate) nonce: [u8; EMAIL_NONCE_BYTES],
    pub(crate) ciphertext: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn password(value: &str) -> Password {
        Password::new(value.to_string()).expect("a valid test password")
    }

    fn dummy() -> DummyHash {
        DummyHash::generate()
    }

    #[test]
    fn a_password_is_judged_by_length_alone() {
        assert_eq!(Password::new("nine char".into()).unwrap_err(), WeakPassword);
        assert!(Password::new("ten chars!".into()).is_ok());
        assert!(
            Password::new("aaaaaaaaaa".into()).is_ok(),
            "no composition rules"
        );
        assert!(
            Password::new("ééééééééé".into()).is_err(),
            "nine characters stay too short even at eighteen bytes"
        );
        assert!(Password::new("é".repeat(MAX_PASSWORD_CHARS)).is_ok());
        assert!(Password::new("a".repeat(MAX_PASSWORD_CHARS + 1)).is_err());
        assert!(Password::new("🦀".repeat(MAX_PASSWORD_CHARS)).is_ok());
    }

    #[test]
    fn hashes_are_salted_argon2id_and_verify_only_their_password() {
        let secret = password("correct horse battery");
        let first = secret.hash();
        let second = secret.hash();
        assert!(
            first.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
            "{first}"
        );
        assert_ne!(first, second, "every hash has its own salt");
        assert!(secret.verify(Some(&first), &dummy()));
        assert!(secret.verify(Some(&second), &dummy()));
        assert!(!password("correct horse battery!").verify(Some(&first), &dummy()));
        assert!(!secret.verify(Some("not a phc string"), &dummy()));
    }

    #[test]
    fn a_hash_this_code_would_not_write_is_refused() {
        let secret = password("correct horse battery");
        let salt = SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
        let hash_with = |algorithm, params| {
            Argon2::new(algorithm, Version::V0x13, params)
                .hash_password(secret.0.as_bytes(), &salt)
                .unwrap()
                .to_string()
        };
        let refused = [
            hash_with(
                Algorithm::Argon2id,
                Params::new(ARGON2_MEMORY_KIB + 8, ARGON2_ITERATIONS, 1, None).unwrap(),
            ),
            hash_with(
                Algorithm::Argon2id,
                Params::new(ARGON2_MEMORY_KIB, ARGON2_ITERATIONS + 1, 1, None).unwrap(),
            ),
            hash_with(
                Algorithm::Argon2id,
                Params::new(ARGON2_MEMORY_KIB, ARGON2_ITERATIONS, 2, None).unwrap(),
            ),
            hash_with(
                Algorithm::Argon2i,
                Params::new(ARGON2_MEMORY_KIB, ARGON2_ITERATIONS, 1, None).unwrap(),
            ),
        ];
        for phc in &refused {
            assert!(!secret.verify(Some(phc), &dummy()), "verified {phc}");
        }
        let cheaper = hash_with(
            Algorithm::Argon2id,
            Params::new(8 * 1024, 1, 1, None).unwrap(),
        );
        assert!(
            secret.verify(Some(&cheaper), &dummy()),
            "a cheaper hash from an older policy still verifies"
        );
    }

    #[test]
    fn an_unknown_account_never_verifies() {
        assert!(!password("correct horse battery").verify(None, &dummy()));
    }

    #[test]
    fn debug_output_never_carries_secrets() {
        let secret = password("correct horse battery");
        let email = Email::parse("ada@example.com").unwrap();
        let code = InvitationCode::generate();
        for printed in [
            format!("{secret:?}"),
            format!("{email:?}"),
            format!("{code:?}"),
        ] {
            assert!(!printed.contains("horse"));
            assert!(!printed.contains("ada"));
            assert!(!printed.contains(&code.encoded()));
        }
    }

    #[test]
    fn emails_are_normalized_and_minimally_shaped() {
        assert_eq!(
            Email::parse("  Ada.Lovelace@Example.COM ")
                .unwrap()
                .as_str(),
            "ada.lovelace@example.com"
        );
        for invalid in [
            "",
            "ada",
            "@example.com",
            "ada@",
            "ada@@example.com",
            "a@b@example.com",
            "ada lovelace@example.com",
            "ada@exa\u{7}mple.com",
            "ada@exa\u{a0}mple.com",
        ] {
            assert!(Email::parse(invalid).is_none(), "accepted {invalid:?}");
        }
        assert!(Email::parse(&format!("{}@b.c", "a".repeat(250))).is_some());
        assert!(Email::parse(&format!("{}@b.c", "a".repeat(251))).is_none());
    }

    #[test]
    fn email_digests_are_keyed_and_normalized() {
        let digester = EmailDigester::new(Zeroizing::new([1; 32]));
        let other_cell = EmailDigester::new(Zeroizing::new([2; 32]));
        let ada = Email::parse("ada@example.com").unwrap();
        let shouting = Email::parse("ADA@EXAMPLE.COM").unwrap();
        assert_eq!(digester.digest(&ada), digester.digest(&shouting));
        assert_ne!(digester.digest(&ada), other_cell.digest(&ada));
        assert_ne!(
            digester.digest(&ada).to_vec(),
            Sha256::digest(ada.as_str()).to_vec(),
            "the digest is keyed, not a bare hash anyone can recompute"
        );
    }

    #[test]
    fn a_device_token_works_only_for_its_email_its_cell_and_its_lifetime() {
        let keys = AccountKeys::fixed(1);
        let devices = &keys.devices;
        let (ada, grace) = ([1_u8; 32], [2_u8; 32]);
        let now = 1_800_000_000;
        let token = devices.issue(&ada, None, now);
        assert_eq!(token.len(), DEVICE_TOKEN_B64URL_LEN);
        let device = devices.verify(&token, &ada, now).expect("a fresh token");
        assert_eq!(
            devices.verify(&devices.issue(&ada, Some(device), now + 60), &ada, now + 60),
            Some(device),
            "a reissued token keeps its device"
        );
        assert_ne!(
            devices.verify(&devices.issue(&ada, None, now), &ada, now),
            Some(device)
        );
        assert_eq!(devices.verify(&token, &grace, now), None, "another email");
        assert_eq!(
            AccountKeys::fixed(2).devices.verify(&token, &ada, now),
            None,
            "another cell"
        );
        assert_eq!(
            devices.verify(&token, &ada, now + DEVICE_TOKEN_TTL_SECS),
            None,
            "expired"
        );
        assert!(devices
            .verify(&token, &ada, now + DEVICE_TOKEN_TTL_SECS - 1)
            .is_some());
        assert_eq!(
            devices.verify(&token, &ada, now - DEVICE_TOKEN_SKEW_SECS - 1),
            None,
            "issued too far in the future"
        );
        let mut forged = token.into_bytes();
        forged[10] = if forged[10] == b'A' { b'B' } else { b'A' };
        let forged = String::from_utf8(forged).unwrap();
        assert_eq!(devices.verify(&forged, &ada, now), None);
        for junk in [
            "",
            "x",
            &"A".repeat(DEVICE_TOKEN_B64URL_LEN),
            &"é".repeat(38),
        ] {
            assert_eq!(devices.verify(junk, &ada, now), None, "{junk:?}");
        }
    }

    #[test]
    fn invitation_codes_are_canonical_256_bit_secrets() {
        let code = InvitationCode::generate();
        let encoded = code.encoded();
        assert_eq!(encoded.len(), CODE_B64URL_LEN);
        assert_eq!(InvitationCode::parse(&encoded), Some(code.clone()));
        assert_ne!(InvitationCode::generate(), code);
        assert_eq!(code.digest().to_vec(), Sha256::digest(code.0).to_vec());
        assert!(InvitationCode::parse(&encoded[1..]).is_none());
        assert!(InvitationCode::parse(&format!("{encoded}=")).is_none());
        assert!(InvitationCode::parse(&encoded.replace(|_: char| true, "+")).is_none());
    }

    #[test]
    fn only_the_link_holder_can_read_the_invited_email() {
        let code = InvitationCode::generate();
        let email = Email::parse("grace@example.com").unwrap();
        let sealed = code.seal_email(&email);
        assert!(!String::from_utf8_lossy(&sealed.ciphertext).contains("grace"));
        assert_eq!(code.open_email(&sealed), Some(email));
        assert_eq!(InvitationCode::generate().open_email(&sealed), None);
        let mut tampered = sealed.clone();
        tampered.ciphertext[0] ^= 1;
        assert_eq!(code.open_email(&tampered), None);
    }
}
