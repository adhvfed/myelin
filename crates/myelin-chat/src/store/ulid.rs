use std::sync::atomic::{AtomicU64, Ordering};

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MessageId(pub String);

impl MessageId {
    pub fn from_u128(value: u128) -> MessageId {
        let mut buf = [0u8; 26];
        let mut v = value;
        for slot in buf.iter_mut().rev() {
            *slot = CROCKFORD[(v & 0x1f) as usize];
            v >>= 5;
        }
        MessageId(String::from_utf8(buf.to_vec()).expect("crockford bytes are ASCII"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn timestamp_ms(&self) -> Option<u64> {
        if self.0.len() != 26 {
            return None;
        }
        self.0
            .bytes()
            .take(10)
            .try_fold(0u64, |value, byte| {
                let digit = CROCKFORD.iter().position(|candidate| *candidate == byte)? as u64;
                value.checked_mul(32)?.checked_add(digit)
            })
            .filter(|value| *value <= 0xFFFF_FFFF_FFFF)
    }
}

pub trait UlidSource: Send + Sync {
    fn mint(&self) -> MessageId;
}

#[derive(Default)]
pub struct MonotonicUlidSource {
    next: AtomicU64,
}

impl MonotonicUlidSource {
    pub fn new() -> MonotonicUlidSource {
        MonotonicUlidSource::default()
    }

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

pub struct SystemUlidSource {
    last: std::sync::Mutex<u128>,
}

impl Default for SystemUlidSource {
    fn default() -> Self {
        SystemUlidSource::new()
    }
}

impl SystemUlidSource {
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
        let value = if candidate > *last {
            candidate
        } else {
            last.wrapping_add(1)
        };
        *last = value;
        MessageId::from_u128(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_round_trips_from_the_ulid_prefix() {
        let timestamp = 1_719_446_400_123u64;
        let id = MessageId::from_u128((timestamp as u128) << 80);
        assert_eq!(id.timestamp_ms(), Some(timestamp));
        assert_eq!(MessageId("not-a-ulid".into()).timestamp_ms(), None);
    }
}
