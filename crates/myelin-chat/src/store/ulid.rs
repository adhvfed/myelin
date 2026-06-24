//! The k-sortable `MessageId` ULID — the intrinsic per-conversation order key (arch
//! [01 §3](../../../../planning/04-subsystem-architectures/chat/architecture/01-tech-and-data-model.md)).
//!
//! A ULID is a 128-bit value: a 48-bit big-endian millisecond timestamp followed by 80 bits of
//! randomness, rendered as 26 Crockford-base32 characters. The crucial property Chat relies on is
//! **lexicographic order == time order** — appending a later message yields a `message_id` that
//! sorts AFTER an earlier one, so per-conversation total order is intrinsic to the id and never
//! wall-clock-derived at read time (clock-skew-at-scale is designed out; arch [01 §3]).
//!
//! There is no ULID crate in the workspace, and this is correctness-critical ordering, so the
//! minter is a small, explicit, deterministic seam ([`UlidSource`]) — exactly the
//! `myelin_events::IdMinter` shape the outbox uses. The default test/floor source is a
//! **monotonic** minter ([`MonotonicUlidSource`]) whose lexical order equals mint order without a
//! wall-clock, so the ULID-monotonicity gate is deterministic in CI. A real wall-clock + random
//! source ([`SystemUlidSource`]) implements the SAME trait; the store does not change (the same
//! escape-hatch-behind-a-trait posture as `IdMinter`).

use std::sync::atomic::{AtomicU64, Ordering};

/// Crockford's base32 alphabet (excludes I, L, O, U to avoid transcription ambiguity) — the ULID
/// canonical alphabet. 32 symbols, so each char carries 5 bits; 26 chars == 130 bits, of which
/// the top 2 are always 0 for a 128-bit ULID.
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A k-sortable ULID message id (arch [01 §3]). The stable, opaque id behind the frozen `#sub`
/// anchor `message-<message_id>`; stable across edits (the `edited_seq` counter bumps, the id does
/// not). `Ord` is the byte order of the 128-bit value, which — because the timestamp is the
/// high-order 48 bits — is **time order**, the per-conversation total order Chat appends in.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MessageId(pub String);

impl MessageId {
    /// Build a `MessageId` from its 128-bit value, rendered as 26 Crockford-base32 chars. The
    /// rendering preserves order: a numerically-greater value renders to a lexically-greater
    /// string (the property the per-conversation total order rests on).
    pub fn from_u128(value: u128) -> MessageId {
        // 26 base-32 digits, most-significant first. 26*5 = 130 bits; the top two bits of the
        // first digit are the always-zero ULID pad bits.
        let mut buf = [0u8; 26];
        let mut v = value;
        for slot in buf.iter_mut().rev() {
            *slot = CROCKFORD[(v & 0x1f) as usize];
            v >>= 5;
        }
        // SAFETY-free: every byte is an ASCII char from CROCKFORD.
        MessageId(String::from_utf8(buf.to_vec()).expect("crockford bytes are ASCII"))
    }

    /// The string form (the `#sub` anchor body, the cursor wire form).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The ULID minting seam — the same shape as `myelin_events::IdMinter`, kept local to the store so
/// the ordering source is explicit and swappable (test-deterministic now, wall-clock later). A
/// source MUST be monotone: a later `mint` returns an id that sorts after an earlier one (the
/// per-conversation total-order invariant).
pub trait UlidSource: Send + Sync {
    /// Mint the next message id. MUST be strictly increasing across calls (lexical == mint order).
    fn mint(&self) -> MessageId;
}

/// A deterministic, monotone ULID source (the test/floor source). The 128-bit value is a simple
/// counter, so lexical order == mint order WITHOUT a wall-clock — the per-conversation ULID
/// monotonicity gate is deterministic in CI. The real wall-clock source is [`SystemUlidSource`].
#[derive(Default)]
pub struct MonotonicUlidSource {
    next: AtomicU64,
}

impl MonotonicUlidSource {
    /// A fresh source starting at 0.
    pub fn new() -> MonotonicUlidSource {
        MonotonicUlidSource::default()
    }

    /// A source whose first minted id is `start` (so two conversations in one test can be given
    /// disjoint, ordered id ranges, or a resume cursor can be placed mid-stream).
    pub fn starting_at(start: u64) -> MonotonicUlidSource {
        MonotonicUlidSource {
            next: AtomicU64::new(start),
        }
    }
}

impl UlidSource for MonotonicUlidSource {
    fn mint(&self) -> MessageId {
        let n = self.next.fetch_add(1, Ordering::SeqCst);
        MessageId::from_u128(n as u128)
    }
}

/// The real ULID source: a 48-bit wall-clock millisecond timestamp + 80 bits of randomness, with a
/// **monotonic guard** within the same millisecond (if two mints land in the same ms, the second
/// increments the previous value rather than re-rolling, so order is never violated under burst —
/// the canonical ULID monotonic-within-ms rule). This is the production source; the floor source
/// ([`MonotonicUlidSource`]) is what the deterministic tests use.
pub struct SystemUlidSource {
    last: std::sync::Mutex<u128>,
}

impl Default for SystemUlidSource {
    fn default() -> Self {
        SystemUlidSource::new()
    }
}

impl SystemUlidSource {
    /// A fresh wall-clock + random ULID source.
    pub fn new() -> SystemUlidSource {
        SystemUlidSource {
            last: std::sync::Mutex::new(0),
        }
    }

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    fn rand80() -> u128 {
        // A non-crypto PRNG is sufficient here: the 80 random bits only have to make the id
        // unlikely-to-collide WITHIN a millisecond; ORDER comes from the timestamp + the
        // monotonic guard, never from the randomness. Mix the nanosecond clock + a thread-local
        // counter so concurrent mints in the same ms diverge.
        use std::cell::Cell;
        thread_local!(static SEED: Cell<u128> = const { Cell::new(0) });
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let bump = SEED.with(|s| {
            let next = s.get().wrapping_add(0x9E37_79B9_7F4A_7C15);
            s.set(next);
            next
        });
        // splitmix-style avalanche of (nanos ^ bump), masked to 80 bits.
        let mut z = nanos ^ bump;
        z = (z ^ (z >> 33)).wrapping_mul(0xFF51_AFD7_ED55_8CCD);
        z = (z ^ (z >> 33)).wrapping_mul(0xC4CE_B9FE_1A85_EC53);
        z ^= z >> 33;
        z & ((1u128 << 80) - 1)
    }
}

impl UlidSource for SystemUlidSource {
    fn mint(&self) -> MessageId {
        let ms = SystemUlidSource::now_ms() as u128;
        let candidate = (ms << 80) | SystemUlidSource::rand80();
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        // The monotonic guard: never return a value <= the last one. Within the same ms (or under
        // a clock that ticked backwards) bump the previous value by 1 so order is preserved.
        let value = if candidate > *last {
            candidate
        } else {
            last.wrapping_add(1)
        };
        *last = value;
        MessageId::from_u128(value)
    }
}
