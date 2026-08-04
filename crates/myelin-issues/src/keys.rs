use myelin_events::ArtifactRef;
use myelin_tenancy::TenantId;
use std::collections::HashMap;
use std::sync::Mutex;

pub const INITIAL_BLOCK_SIZE: u32 = 50;
pub const MAX_BLOCK_SIZE: u32 = 1000;
pub const BLOCK_GROWTH_FACTOR: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CanonicalKey {
    pub prefix: String,
    pub seqno: u64,
}

impl CanonicalKey {
    pub fn render(&self) -> String {
        format!("{}-{}", self.prefix, self.seqno)
    }

    pub fn render_display_key(&self) -> String {
        format!("#{}", self.seqno)
    }

    pub fn issue_artifact_ref(&self, tenant: &TenantId) -> ArtifactRef {
        ArtifactRef(format!(
            "myelin://{}/issue/issue/{}",
            tenant.0,
            self.render()
        ))
    }
}

pub fn render_display_key(seqno: u64) -> String {
    format!("#{seqno}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReservedBlock {
    pub lo: u64,
    pub hi: u64,
}

impl ReservedBlock {
    pub fn len(&self) -> u64 {
        self.hi - self.lo
    }
    pub fn is_empty(&self) -> bool {
        self.hi <= self.lo
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReserveError {
    Backend(String),
}

impl std::fmt::Display for ReserveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReserveError::Backend(why) => {
                write!(
                    f,
                    "prefix_counter reserve failed (allocation fails closed): {why}"
                )
            }
        }
    }
}

impl std::error::Error for ReserveError {}

pub trait PrefixReserve: Send + Sync {
    fn reserve(
        &self,
        tenant: &TenantId,
        prefix: &str,
        block_size: u32,
    ) -> Result<ReservedBlock, ReserveError>;
}

#[derive(Default)]
pub struct InMemoryPrefixCounter {
    high_water: Mutex<HashMap<(String, String), u64>>,
}

impl InMemoryPrefixCounter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn high_water(&self, tenant: &TenantId, prefix: &str) -> u64 {
        *self
            .high_water
            .lock()
            .expect("prefix_counter mutex poisoned")
            .get(&(tenant.0.clone(), prefix.to_string()))
            .unwrap_or(&0)
    }
}

impl PrefixReserve for InMemoryPrefixCounter {
    fn reserve(
        &self,
        tenant: &TenantId,
        prefix: &str,
        block_size: u32,
    ) -> Result<ReservedBlock, ReserveError> {
        let mut map = self
            .high_water
            .lock()
            .map_err(|e| ReserveError::Backend(format!("mutex poisoned: {e}")))?;
        let key = (tenant.0.clone(), prefix.to_string());
        let lo = *map.get(&key).unwrap_or(&0);
        let hi = lo + block_size as u64;
        map.insert(key, hi);
        Ok(ReservedBlock { lo, hi })
    }
}

#[derive(Clone, Copy, Debug)]
struct PrefixLocalBlock {
    next: u64,
    block_hi: u64,
    block_size: u32,
}

impl PrefixLocalBlock {
    fn cold() -> Self {
        Self {
            next: 1,
            block_hi: 0,
            block_size: INITIAL_BLOCK_SIZE,
        }
    }
    fn is_empty(&self) -> bool {
        self.next > self.block_hi
    }
}

pub struct HiLoKeyAllocator<R: PrefixReserve> {
    reserve: R,
    blocks: Mutex<HashMap<(String, String), PrefixLocalBlock>>,
}

impl<R: PrefixReserve> HiLoKeyAllocator<R> {
    pub fn new(reserve: R) -> Self {
        Self {
            reserve,
            blocks: Mutex::new(HashMap::new()),
        }
    }

    pub fn allocate(&self, tenant: &TenantId, prefix: &str) -> Result<CanonicalKey, ReserveError> {
        let mut blocks = self
            .blocks
            .lock()
            .map_err(|e| ReserveError::Backend(format!("blocks mutex poisoned: {e}")))?;
        let key = (tenant.0.clone(), prefix.to_string());
        let block = blocks.entry(key).or_insert_with(PrefixLocalBlock::cold);

        if block.is_empty() {
            if block.block_hi > 0 {
                block.block_size = grow_block_size(block.block_size);
            }
            let reserved = self.reserve.reserve(tenant, prefix, block.block_size)?;
            block.next = reserved.lo + 1;
            block.block_hi = reserved.hi;
        }

        let seqno = block.next;
        block.next += 1;
        Ok(CanonicalKey {
            prefix: prefix.to_string(),
            seqno,
        })
    }
}

fn grow_block_size(current: u32) -> u32 {
    current
        .saturating_mul(BLOCK_GROWTH_FACTOR)
        .min(MAX_BLOCK_SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    #[test]
    fn canonical_key_is_projectkey_dash_seqno_render_is_hash_seqno() {
        let k = CanonicalKey {
            prefix: "ENG".into(),
            seqno: 1421,
        };
        assert_eq!(k.render(), "ENG-1421");
        assert_eq!(k.render_display_key(), "#1421");
        assert_eq!(render_display_key(1421), "#1421");
        assert_eq!(
            k.issue_artifact_ref(&tenant()).0,
            "myelin://acme/issue/issue/ENG-1421"
        );
    }

    #[test]
    fn stored_key_parses_and_display_form_is_not_a_scope() {
        let k = CanonicalKey {
            prefix: "ENG".into(),
            seqno: 7,
        };
        let stored = k.issue_artifact_ref(&tenant());
        myelin_refs::parse(&stored.0).expect("the stored <PROJECTKEY>-<seqno> id is a valid ref");
        assert!(
            myelin_refs::parse(&k.render_display_key()).is_err(),
            "the #<seqno> display form is render-time only, never a scope"
        );
    }

    #[test]
    fn keys_are_monotonic_per_prefix_starting_at_one() {
        let a = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
        let mut last = 0;
        for i in 1..=130u64 {
            let k = a.allocate(&tenant(), "ENG").expect("allocate");
            assert_eq!(k.seqno, i, "seqno is contiguous + monotonic per prefix");
            assert!(k.seqno > last, "strictly increasing");
            last = k.seqno;
        }
        let first = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
        assert_eq!(first.allocate(&tenant(), "ENG").unwrap().seqno, 1);
    }

    #[test]
    fn per_prefix_isolation_two_prefixes_have_independent_seqno_spaces() {
        let a = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
        assert_eq!(a.allocate(&tenant(), "ENG").unwrap().seqno, 1);
        assert_eq!(a.allocate(&tenant(), "OPS").unwrap().seqno, 1);
        assert_eq!(a.allocate(&tenant(), "ENG").unwrap().seqno, 2);
        assert_eq!(a.allocate(&tenant(), "OPS").unwrap().seqno, 2);
        assert_eq!(a.allocate(&tenant(), "ENG").unwrap().seqno, 3);
        let other = TenantId("globex".into());
        assert_eq!(a.allocate(&other, "ENG").unwrap().seqno, 1);
    }

    #[test]
    fn gap_tolerant_a_leaked_block_is_benign_no_reuse() {
        let counter = Arc::new(InMemoryPrefixCounter::new());
        {
            let a1 = HiLoKeyAllocator::new(SharedReserve(Arc::clone(&counter)));
            assert_eq!(a1.allocate(&tenant(), "ENG").unwrap().seqno, 1);
            assert_eq!(a1.allocate(&tenant(), "ENG").unwrap().seqno, 2);
            assert_eq!(a1.allocate(&tenant(), "ENG").unwrap().seqno, 3);
            assert_eq!(counter.high_water(&tenant(), "ENG"), 50);
        }

        let a2 = HiLoKeyAllocator::new(SharedReserve(Arc::clone(&counter)));
        let next = a2.allocate(&tenant(), "ENG").unwrap();
        assert_eq!(
            next.seqno, 51,
            "continues from the durable high-water - a gap, never a reuse"
        );
    }

    #[test]
    fn adaptive_block_size_grows_on_a_hot_prefix() {
        let counter = Arc::new(InMemoryPrefixCounter::new());
        let a = HiLoKeyAllocator::new(SharedReserve(Arc::clone(&counter)));
        for _ in 0..INITIAL_BLOCK_SIZE {
            a.allocate(&tenant(), "ENG").unwrap();
        }
        assert_eq!(
            counter.high_water(&tenant(), "ENG"),
            50,
            "first block is the cold size 50"
        );
        a.allocate(&tenant(), "ENG").unwrap();
        assert_eq!(
            counter.high_water(&tenant(), "ENG"),
            150,
            "the second block grew to 100"
        );
        for _ in 0..99 {
            a.allocate(&tenant(), "ENG").unwrap();
        }
        a.allocate(&tenant(), "ENG").unwrap();
        assert_eq!(
            counter.high_water(&tenant(), "ENG"),
            350,
            "the third block grew to 200"
        );
    }

    #[test]
    fn block_size_growth_is_capped_at_max() {
        let mut sz = INITIAL_BLOCK_SIZE;
        for _ in 0..20 {
            sz = grow_block_size(sz);
        }
        assert_eq!(
            sz, MAX_BLOCK_SIZE,
            "growth saturates at the ceiling, never overflows"
        );
        assert_eq!(grow_block_size(50), 100);
        assert_eq!(
            grow_block_size(800),
            1000,
            "the step toward the cap is clamped"
        );
        assert_eq!(grow_block_size(1000), 1000);
    }

    #[test]
    fn create_storm_on_one_hot_prefix_zero_dup_monotonic() {
        const WORKERS: usize = 16;
        const PER_WORKER: usize = 500;
        let allocator = Arc::new(HiLoKeyAllocator::new(InMemoryPrefixCounter::new()));
        let mut handles = Vec::new();
        for _ in 0..WORKERS {
            let a = Arc::clone(&allocator);
            handles.push(thread::spawn(move || {
                let mut got = Vec::with_capacity(PER_WORKER);
                for _ in 0..PER_WORKER {
                    got.push(a.allocate(&tenant(), "ENG").unwrap().seqno);
                }
                got
            }));
        }
        let mut all: Vec<u64> = handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect();
        let total = WORKERS * PER_WORKER;
        assert_eq!(all.len(), total);
        all.sort_unstable();
        let distinct = {
            let mut d = all.clone();
            d.dedup();
            d.len()
        };
        assert_eq!(
            distinct, total,
            "0 duplicate key under a {WORKERS}-worker storm"
        );
        assert_eq!(all.first(), Some(&1));
        assert_eq!(all.last(), Some(&(total as u64)));
        for (i, seq) in all.iter().enumerate() {
            assert_eq!(*seq, (i + 1) as u64, "contiguous monotonic 1..=total");
        }
    }

    struct SharedReserve(Arc<InMemoryPrefixCounter>);
    impl PrefixReserve for SharedReserve {
        fn reserve(
            &self,
            tenant: &TenantId,
            prefix: &str,
            block_size: u32,
        ) -> Result<ReservedBlock, ReserveError> {
            self.0.reserve(tenant, prefix, block_size)
        }
    }

    #[test]
    fn reserve_error_display_is_loud() {
        let e = ReserveError::Backend("conn reset".into());
        assert!(format!("{e}").contains("fails closed"));
    }

    #[test]
    fn reserved_block_len_and_empty() {
        let b = ReservedBlock { lo: 0, hi: 50 };
        assert_eq!(b.len(), 50);
        assert!(!b.is_empty());
        assert!(ReservedBlock { lo: 7, hi: 7 }.is_empty());
    }
}
