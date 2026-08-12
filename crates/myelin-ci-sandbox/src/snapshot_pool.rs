use crate::{Region, RunnerClass, SandboxHandle};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcquirePath {
    Warm,
    Cold,
}

#[derive(Debug)]
pub struct WarmSandbox {
    handle: SandboxHandle,
    path: AcquirePath,
}

impl WarmSandbox {
    pub fn handle(&self) -> &SandboxHandle {
        &self.handle
    }

    pub fn path(&self) -> AcquirePath {
        self.path
    }

    pub fn run_one_job_then_kill<R, K, T>(self, run: R, kill: K) -> T
    where
        R: FnOnce(&SandboxHandle) -> T,
        K: FnOnce(&SandboxHandle),
    {
        let result = run(&self.handle);
        kill(&self.handle);
        result
    }
}

pub trait SnapshotRestore {
    fn restore(
        &self,
        region: &Region,
        class: &RunnerClass,
        seq: u64,
    ) -> Result<SandboxHandle, String>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ModeledRestore;

impl ModeledRestore {
    pub fn new() -> ModeledRestore {
        ModeledRestore
    }
}

impl SnapshotRestore for ModeledRestore {
    fn restore(
        &self,
        region: &Region,
        class: &RunnerClass,
        seq: u64,
    ) -> Result<SandboxHandle, String> {
        Ok(SandboxHandle {
            guest_id: format!("warm-{}-{}-{seq}", region.0, class.0),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PoolStats {
    pub target: u32,
    pub warm: u32,
    pub warm_served: u64,
    pub cold_served: u64,
    pub refills: u64,
}

impl PoolStats {
    pub fn warm_hit_rate(&self) -> f64 {
        let total = self.warm_served + self.cold_served;
        if total == 0 {
            0.0
        } else {
            self.warm_served as f64 / total as f64
        }
    }
}

pub struct SnapshotPool<R: SnapshotRestore> {
    target: u32,
    restore: R,
    cells: Mutex<HashMap<(String, String), Cell>>,
}

#[derive(Default)]
struct Cell {
    warm: Vec<SandboxHandle>,
    stats: PoolStats,
    seq: u64,
}

impl<R: SnapshotRestore> SnapshotPool<R> {
    pub fn new(target: u32, restore: R) -> SnapshotPool<R> {
        SnapshotPool {
            target,
            restore,
            cells: Mutex::new(HashMap::new()),
        }
    }

    fn key(region: &Region, class: &RunnerClass) -> (String, String) {
        (region.0.clone(), class.0.clone())
    }

    pub fn warm_up(&self, region: &Region, class: &RunnerClass) -> PoolStats {
        let mut cells = crate::sync::lock_recovering_poison(&self.cells);
        let cell = cells.entry(Self::key(region, class)).or_default();
        cell.stats.target = self.target;
        while (cell.warm.len() as u32) < self.target {
            let seq = cell.seq;
            cell.seq += 1;
            match self.restore.restore(region, class, seq) {
                Ok(handle) => {
                    cell.warm.push(handle);
                    cell.stats.refills += 1;
                }
                Err(_) => break,
            }
        }
        cell.stats.warm = cell.warm.len() as u32;
        cell.stats
    }

    pub fn acquire<C>(
        &self,
        region: &Region,
        class: &RunnerClass,
        cold_boot: C,
    ) -> Result<WarmSandbox, String>
    where
        C: FnOnce() -> Result<SandboxHandle, String>,
    {
        let mut cells = crate::sync::lock_recovering_poison(&self.cells);
        let cell = cells.entry(Self::key(region, class)).or_default();
        cell.stats.target = self.target;

        if let Some(handle) = cell.warm.pop() {
            cell.stats.warm_served += 1;
            let seq = cell.seq;
            cell.seq += 1;
            if let Ok(fresh) = self.restore.restore(region, class, seq) {
                cell.warm.push(fresh);
                cell.stats.refills += 1;
            }
            cell.stats.warm = cell.warm.len() as u32;
            Ok(WarmSandbox {
                handle,
                path: AcquirePath::Warm,
            })
        } else {
            cell.stats.cold_served += 1;
            cell.stats.warm = cell.warm.len() as u32;
            drop(cells);
            let handle = cold_boot()?;
            Ok(WarmSandbox {
                handle,
                path: AcquirePath::Cold,
            })
        }
    }

    pub fn stats(&self, region: &Region, class: &RunnerClass) -> PoolStats {
        let cells = crate::sync::lock_recovering_poison(&self.cells);
        cells
            .get(&Self::key(region, class))
            .map(|c| {
                let mut s = c.stats;
                s.target = self.target;
                s.warm = c.warm.len() as u32;
                s
            })
            .unwrap_or(PoolStats {
                target: self.target,
                ..PoolStats::default()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn region() -> Region {
        Region("fr-par".into())
    }
    fn class() -> RunnerClass {
        RunnerClass("eu-west".into())
    }

    #[test]
    fn warm_up_fills_the_buffer_to_target() {
        let pool = SnapshotPool::new(3, ModeledRestore::new());
        let stats = pool.warm_up(&region(), &class());
        assert_eq!(stats.target, 3);
        assert_eq!(
            stats.warm, 3,
            "the warm buffer is filled to the fixed target (the floor)"
        );
        assert_eq!(stats.refills, 3);
        let again = pool.warm_up(&region(), &class());
        assert_eq!(again.warm, 3);
        assert_eq!(again.refills, 3, "no extra restores when already full");
    }

    #[test]
    fn acquire_serves_warm_and_replaces_the_slot() {
        let pool = SnapshotPool::new(2, ModeledRestore::new());
        pool.warm_up(&region(), &class());
        assert_eq!(pool.stats(&region(), &class()).warm, 2);

        let cold_used = Arc::new(AtomicUsize::new(0));
        let cu = cold_used.clone();
        let sb = pool
            .acquire(&region(), &class(), || {
                cu.fetch_add(1, Ordering::SeqCst);
                Ok(SandboxHandle {
                    guest_id: "cold".into(),
                })
            })
            .unwrap();
        assert_eq!(sb.path(), AcquirePath::Warm);
        assert_eq!(
            cold_used.load(Ordering::SeqCst),
            0,
            "a warm hit never cold-boots"
        );
        assert!(sb.handle().guest_id.starts_with("warm-"));
        let stats = pool.stats(&region(), &class());
        assert_eq!(
            stats.warm, 2,
            "the handed-out slot is replaced - occupancy stays at target"
        );
        assert_eq!(stats.warm_served, 1);
        assert_eq!(stats.warm_hit_rate(), 1.0);
    }

    #[test]
    fn empty_buffer_falls_back_to_cold_boot() {
        let pool = SnapshotPool::new(1, ModeledRestore::new());
        let cold_used = Arc::new(AtomicUsize::new(0));
        let cu = cold_used.clone();
        let sb = pool
            .acquire(&region(), &class(), || {
                cu.fetch_add(1, Ordering::SeqCst);
                Ok(SandboxHandle {
                    guest_id: "cold-boot".into(),
                })
            })
            .unwrap();
        assert_eq!(sb.path(), AcquirePath::Cold, "an empty buffer cold-boots");
        assert_eq!(
            cold_used.load(Ordering::SeqCst),
            1,
            "the cold-boot fallback was used"
        );
        assert_eq!(sb.handle().guest_id, "cold-boot");
        let stats = pool.stats(&region(), &class());
        assert_eq!(stats.cold_served, 1);
        assert_eq!(stats.warm_hit_rate(), 0.0);
    }

    #[test]
    fn restored_vm_serves_exactly_one_job_then_is_killed() {
        let pool = SnapshotPool::new(2, ModeledRestore::new());
        pool.warm_up(&region(), &class());

        let sb = pool
            .acquire(&region(), &class(), || {
                Ok(SandboxHandle {
                    guest_id: "cold".into(),
                })
            })
            .unwrap();
        let handed_out = sb.handle().guest_id.clone();

        let killed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let k = killed.clone();
        let job_ran = sb.run_one_job_then_kill(
            |h| {
                assert_eq!(h.guest_id, handed_out);
                true
            },
            |h| {
                k.lock().unwrap().push(h.guest_id.clone());
            },
        );
        assert!(job_ran);
        assert_eq!(
            killed.lock().unwrap().as_slice(),
            std::slice::from_ref(&handed_out)
        );

        let stats = pool.stats(&region(), &class());
        assert_eq!(
            stats.warm, 2,
            "the buffer holds fresh replacements, never the served guest"
        );
    }

    #[test]
    fn successive_acquires_hand_out_distinct_fresh_guests() {
        let pool = SnapshotPool::new(2, ModeledRestore::new());
        pool.warm_up(&region(), &class());
        let mut seen = std::collections::HashSet::new();
        for _ in 0..5 {
            let sb = pool
                .acquire(&region(), &class(), || {
                    Ok(SandboxHandle {
                        guest_id: "cold".into(),
                    })
                })
                .unwrap();
            assert!(
                seen.insert(sb.handle().guest_id.clone()),
                "every acquire hands out a DISTINCT fresh guest - never the same live guest twice"
            );
        }
    }

    #[test]
    fn buffer_is_keyed_per_region_and_class_no_global_pool() {
        let pool = SnapshotPool::new(1, ModeledRestore::new());
        let fr = Region("fr-par".into());
        let de = Region("de-fra".into());
        pool.warm_up(&fr, &class());
        assert_eq!(pool.stats(&fr, &class()).warm, 1);
        assert_eq!(
            pool.stats(&de, &class()).warm,
            0,
            "no cross-region warm pool (12.4)"
        );

        let sb = pool
            .acquire(&de, &class(), || {
                Ok(SandboxHandle {
                    guest_id: "cold-de".into(),
                })
            })
            .unwrap();
        assert_eq!(sb.path(), AcquirePath::Cold);
        assert_eq!(pool.stats(&fr, &class()).warm, 1);
    }

    #[test]
    fn restore_failure_during_warm_up_stops_the_fill() {
        struct FlakyRestore {
            ok_count: AtomicUsize,
        }
        impl SnapshotRestore for FlakyRestore {
            fn restore(
                &self,
                _r: &Region,
                _c: &RunnerClass,
                seq: u64,
            ) -> Result<SandboxHandle, String> {
                if self.ok_count.fetch_add(1, Ordering::SeqCst) < 2 {
                    Ok(SandboxHandle {
                        guest_id: format!("warm-{seq}"),
                    })
                } else {
                    Err("snapshot corrupt".into())
                }
            }
        }
        let pool = SnapshotPool::new(
            5,
            FlakyRestore {
                ok_count: AtomicUsize::new(0),
            },
        );
        let stats = pool.warm_up(&region(), &class());
        assert_eq!(
            stats.warm, 2,
            "the fill stops at the last successful restore"
        );
        assert_eq!(stats.target, 5);
    }
}
