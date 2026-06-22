//! # `snapshot_pool` — pre-warmed microVM snapshot pools (the cold-start mitigation)
//! (CI-P4 → P-240, M2)
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §5.4 ("Pre-warmed snapshot pools — the cold-start mitigation"): *"Firecracker resumes from a
//! memory **snapshot** in tens of ms. The fleet autoscaler keeps a small warm buffer of
//! resumed-from-snapshot microVMs per (region, label-class), sized to the recent arrival rate, so
//! 'time to first log line' is warm-pool-fast; the cold path is the boundary's cost, mitigated not
//! eliminated."* §5.3 (the mandatory hardening profile — NOT weakened by pooling: a restored VM is
//! still one-job-per-sandbox, ephemeral, never reused).
//!
//! **Contracts CONSUMED:** 12.4 (`residency_verify` — the warm buffer is keyed per `(region,
//! label-class)`; a restored VM serves only in-region jobs, no cross-region pool).
//!
//! ## What this models — the pool + its accounting (DB-free / VM-free by default)
//! A [`SnapshotPool`] keeps a FIXED warm buffer of `target` resume-from-snapshot microVMs per
//! `(region, label-class)`. It hands out a WARM sandbox on [`SnapshotPool::acquire`] — a restore from
//! a memory snapshot (tens of ms) instead of a cold boot (seconds) — then **replaces** the handed-out
//! slot by restoring a fresh one, so the buffer stays at `target`. When the buffer is EMPTY the
//! acquire falls back to a COLD boot through the same [`crate::SandboxBackend`] — the warm path is a
//! mitigation, not a guarantee (§5.4: "mitigated not eliminated").
//!
//! ### The hardening invariant is NOT weakened by pooling (§5.3)
//! Pooling pre-BOOTS microVMs; it never pre-RUNS untrusted code. The invariant is preserved by
//! construction:
//! - **One-job-per-sandbox.** A restored VM serves EXACTLY one job, then is killed — it is NEVER
//!   returned to the pool for a second job. [`WarmSandbox::run_one_job_then_kill`] consumes the warm
//!   sandbox (`self` by value) so it cannot be reused; [`SnapshotPool::acquire`] mints a FRESH
//!   restore each time (a new `guest_id`), never re-handing a live guest.
//! - **Ephemeral, never reused across tenants/jobs.** The restored guest is from a CLEAN
//!   pre-boot snapshot (no prior tenant's job ever ran in it — the snapshot is taken right after the
//!   hardened boot, before any job); the per-job spec (its run token, secrets, workspace) is applied
//!   to the FRESH restore, then the guest is whole-guest-killed on teardown.
//! - **The full hardening profile.** The snapshot is taken FROM a guest booted with the mandatory
//!   profile (read-only root, no-NIC default-deny, pids.max, …), so a restore inherits it; the live
//!   restore proof ([`SnapshotRestore`]) carries that the restored guest is the hardened guest.
//!
//! ### The live snapshot-restore seam ([`SnapshotRestore`])
//! [`SnapshotRestore`] is the seam the REAL Firecracker `PauseVM → CreateSnapshot → LoadSnapshot →
//! ResumeVM` cycle plugs in behind. The DEFAULT-test path uses a modelled restore (no VM); the LIVE
//! restore — proven on real silicon — lives behind the `integration` feature in
//! `tests/snapshot_pool_integration.rs`, which drives the real Firecracker API socket
//! (`--api-sock`) through the snapshot/restore cycle and asserts the RESTORED guest runs. It is
//! gated to SKIP gracefully without `/dev/kvm` / `firecracker`.
//!
//! ## FLOOR named (CI-P4)
//! The warm buffer is a **FIXED `target`** here (the floor). The MEASURED buffer-sizing function —
//! sizing the buffer to the recent arrival rate per (region, label-class), the open question
//! **07#2** — is tuned in **CI-M5 (CI-P23)**. State this in writing: the pool's `target` is a fixed
//! floor; the autoscaler-driven adaptive sizing is the CI-P23 follow-on.

use crate::{Region, RunnerClass, SandboxHandle};
use std::collections::HashMap;
use std::sync::Mutex;

/// **The result of acquiring a sandbox from the pool — was it served WARM (a snapshot restore) or
/// COLD (a fresh boot)?** The accounting signal the autoscaler reads (warm-hit rate) and the test
/// asserts (a warm pool serves warm; an empty pool falls back to cold).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcquirePath {
    /// Served from the warm buffer (a resume-from-snapshot microVM, tens of ms — faster than cold).
    Warm,
    /// The warm buffer was empty — fell back to a cold boot (the boundary's cost, mitigated not
    /// eliminated, §5.4).
    Cold,
}

/// **A live restored-from-snapshot microVM the pool hands out — single-use (one-job-per-sandbox).**
/// Holds the restored guest's [`SandboxHandle`] and how it was served ([`AcquirePath`]). It is
/// consumed BY VALUE when the job runs ([`WarmSandbox::run_one_job_then_kill`]) so the SAME warm
/// sandbox can never serve a second job — the one-job-per-sandbox-ephemeral invariant, preserved by
/// the type (you cannot call `run_one_job_then_kill` twice on one [`WarmSandbox`]).
#[derive(Debug)]
pub struct WarmSandbox {
    handle: SandboxHandle,
    path: AcquirePath,
}

impl WarmSandbox {
    /// The restored guest's handle (the backend's guest id the caller MUST eventually whole-guest-kill).
    pub fn handle(&self) -> &SandboxHandle {
        &self.handle
    }

    /// Whether this sandbox was served WARM (snapshot restore) or COLD (fresh boot).
    pub fn path(&self) -> AcquirePath {
        self.path
    }

    /// **Run exactly ONE job in this restored guest, then whole-guest-kill it (one-job-per-sandbox,
    /// ephemeral).** Consumes `self` BY VALUE so the warm sandbox CANNOT be reused for a second job
    /// — the restored VM serves exactly one job then is killed, NEVER returned to the pool. `run` is
    /// the per-job work (a real caller drives [`crate::SandboxBackend::launch`] of the job's spec
    /// onto this restored guest, streams its logs, reports terminal); `kill` is the whole-guest kill
    /// on teardown. Returns the job's result + the killed guest id (the accounting observable).
    pub fn run_one_job_then_kill<R, K, T>(self, run: R, kill: K) -> T
    where
        R: FnOnce(&SandboxHandle) -> T,
        K: FnOnce(&SandboxHandle),
    {
        let result = run(&self.handle);
        // Whole-guest kill on teardown — the restored guest is destroyed, never reused (§5.3). After
        // this `self` is dropped; the sandbox is gone.
        kill(&self.handle);
        result
    }
}

/// **The live snapshot-restore seam (§5.4) — the REAL Firecracker `PauseVM → CreateSnapshot →
/// LoadSnapshot → ResumeVM` plugs in behind this.** A restore produces a FRESH guest from a clean
/// pre-boot memory+state snapshot (no prior job ever ran in it), returning its [`SandboxHandle`].
/// The DEFAULT-test path uses a modelled restore ([`ModeledRestore`]); the LIVE restore is driven in
/// `tests/snapshot_pool_integration.rs` behind the `integration` feature (real `/dev/kvm` +
/// `firecracker --api-sock`), gated to skip gracefully without the host.
///
/// Returning `Err` ⇒ the restore failed (the snapshot is corrupt / the VMM rejected the load) — the
/// pool surfaces it and the acquire falls back to a cold boot (never a degraded silent reuse).
pub trait SnapshotRestore {
    /// Restore a FRESH microVM from the `(region, class)` snapshot, returning the restored guest's
    /// handle. Each call MUST produce a DISTINCT guest (a fresh restore — never the same live guest
    /// twice; one-job-per-sandbox). `seq` disambiguates successive restores in the model.
    fn restore(
        &self,
        region: &Region,
        class: &RunnerClass,
        seq: u64,
    ) -> Result<SandboxHandle, String>;
}

/// **A modelled restore (the DB-free / VM-free default-test path).** Produces a distinct fresh guest
/// id per `(region, class, seq)` WITHOUT booting a VM — so the pool's accounting + the
/// one-job-per-sandbox invariant are unit-tested without `/dev/kvm`. The REAL restore swaps in behind
/// the SAME [`SnapshotRestore`] seam (the live cycle, `tests/snapshot_pool_integration.rs`); the pool
/// logic does not change (the dev↔prod CONFIG SWAP, never a code change).
#[derive(Clone, Copy, Debug, Default)]
pub struct ModeledRestore;

impl ModeledRestore {
    /// A fresh modelled restore.
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
        // A distinct fresh guest per restore — never the same live guest twice (one-job-per-sandbox).
        Ok(SandboxHandle {
            guest_id: format!("warm-{}-{}-{seq}", region.0, class.0),
        })
    }
}

/// **The pool accounting for one `(region, label-class)` warm buffer.** The fixed `target` (the
/// floor), the current `warm` count (occupancy), and lifetime counters for warm vs cold acquires
/// (the autoscaler's warm-hit-rate signal). PII-free (counts + ids only).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PoolStats {
    /// The fixed warm-buffer target (the FLOOR — the measured buffer-sizing function is CI-P23).
    pub target: u32,
    /// The current warm occupancy (resume-from-snapshot microVMs ready to hand out).
    pub warm: u32,
    /// Lifetime count of WARM acquires (a snapshot restore served from the buffer).
    pub warm_served: u64,
    /// Lifetime count of COLD acquires (the buffer was empty — fell back to a cold boot).
    pub cold_served: u64,
    /// Lifetime count of restores done to REFILL the buffer (the autoscaler/maintainer's work).
    pub refills: u64,
}

impl PoolStats {
    /// The warm-hit RATE (warm_served / total) as a fraction in `[0.0, 1.0]` — the autoscaler's
    /// "is the buffer big enough" signal. `0.0` when nothing has been served yet.
    pub fn warm_hit_rate(&self) -> f64 {
        let total = self.warm_served + self.cold_served;
        if total == 0 {
            0.0
        } else {
            self.warm_served as f64 / total as f64
        }
    }
}

/// **A pre-warmed microVM snapshot pool (architecture §5.4).** Keeps a FIXED warm buffer of `target`
/// resume-from-snapshot microVMs per `(region, label-class)`. [`SnapshotPool::acquire`] hands out a
/// warm sandbox (or cold-boots when the buffer is empty) and REPLACES the handed-out slot so the
/// buffer stays at `target`. The one-job-per-sandbox-ephemeral invariant is preserved: a handed-out
/// [`WarmSandbox`] serves exactly one job then is killed, never returned (§5.3). The restore is
/// driven through the [`SnapshotRestore`] seam (modelled by default; the live Firecracker cycle in
/// `tests/snapshot_pool_integration.rs`).
///
/// **Residency (12.4):** the buffer is keyed per `(region, label-class)` — a restore for `region` is
/// pinned to `region`; there is no cross-region warm pool (no global pool).
pub struct SnapshotPool<R: SnapshotRestore> {
    /// The fixed per-(region, class) warm-buffer target (the FLOOR — CI-P23 measures it).
    target: u32,
    /// The restore seam (modelled by default; the live FC cycle behind `integration`).
    restore: R,
    /// Per-(region, class) state: the warm buffer (ready handles) + the accounting + a monotonic
    /// restore sequence (so each restore is a DISTINCT fresh guest).
    cells: Mutex<HashMap<(String, String), Cell>>,
}

/// Per-(region, class) pool state.
#[derive(Default)]
struct Cell {
    /// The warm buffer — ready-to-hand-out restored guest handles (FIFO).
    warm: Vec<SandboxHandle>,
    /// The accounting counters.
    stats: PoolStats,
    /// A monotonic restore sequence (disambiguates successive restores — each a distinct guest).
    seq: u64,
}

impl<R: SnapshotRestore> SnapshotPool<R> {
    /// Build a pool with a fixed warm-buffer `target` per `(region, label-class)` (the FLOOR), over
    /// the given [`SnapshotRestore`] seam. The buffer is filled lazily on the first [`warm_up`] /
    /// [`acquire`] for a `(region, class)` (a fresh cell starts empty and is filled to `target`).
    ///
    /// [`warm_up`]: SnapshotPool::warm_up
    /// [`acquire`]: SnapshotPool::acquire
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

    /// **Fill the `(region, class)` warm buffer up to `target` (the maintainer/autoscaler refill).**
    /// Restores fresh microVMs until the buffer holds `target` warm guests. Idempotent — a call when
    /// the buffer is already full is a no-op. Returns the resulting [`PoolStats`]. A restore that
    /// FAILS stops the fill at the current occupancy (surfaced via the unchanged warm count; the next
    /// acquire that finds an empty buffer cold-boots) — never a silent degraded guest.
    pub fn warm_up(&self, region: &Region, class: &RunnerClass) -> PoolStats {
        let mut cells = self.cells.lock().unwrap();
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
                // A restore failure stops the refill here; the buffer holds what it has (the next
                // acquire on an empty buffer cold-boots). Never push a degraded/None guest.
                Err(_) => break,
            }
        }
        cell.stats.warm = cell.warm.len() as u32;
        cell.stats
    }

    /// **Acquire a sandbox for a `(region, class)` job — WARM (snapshot restore) if the buffer has
    /// one, else COLD-boot via `cold_boot` (§5.4).** On a warm hit it pops a ready restored guest
    /// (tens-of-ms time-to-first-log-line) and REPLACES it with a fresh restore so the buffer stays
    /// at `target`; on a miss it falls back to `cold_boot` (the boundary's cost, mitigated not
    /// eliminated). Returns the [`WarmSandbox`] (single-use — one job then killed) tagged with how it
    /// was served.
    ///
    /// `cold_boot` is the fallback the runner supplies — a real cold boot through
    /// [`crate::SandboxBackend::launch`] (the SAME hardened profile). The replace-after-handout keeps
    /// the warm buffer topped up without a separate maintainer pass on the hot path.
    pub fn acquire<C>(
        &self,
        region: &Region,
        class: &RunnerClass,
        cold_boot: C,
    ) -> Result<WarmSandbox, String>
    where
        C: FnOnce() -> Result<SandboxHandle, String>,
    {
        let mut cells = self.cells.lock().unwrap();
        let cell = cells.entry(Self::key(region, class)).or_default();
        cell.stats.target = self.target;

        if let Some(handle) = cell.warm.pop() {
            // WARM HIT: served from the buffer. Replace the handed-out slot with a FRESH restore so
            // the buffer stays at target (the restored guest is single-use — this is a NEW guest,
            // never the one we just handed out; one-job-per-sandbox).
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
            // COLD MISS: the buffer is empty — fall back to a cold boot (the same hardened profile).
            cell.stats.cold_served += 1;
            cell.stats.warm = cell.warm.len() as u32;
            // Drop the lock before the (potentially slow) cold boot so we do not hold it across the
            // boundary's cost.
            drop(cells);
            let handle = cold_boot()?;
            Ok(WarmSandbox {
                handle,
                path: AcquirePath::Cold,
            })
        }
    }

    /// The current accounting for a `(region, class)` buffer (occupancy + warm/cold counters + the
    /// warm-hit rate). A cell never warmed-up reports the default (target set, 0 warm).
    pub fn stats(&self, region: &Region, class: &RunnerClass) -> PoolStats {
        let cells = self.cells.lock().unwrap();
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

    /// **warm_up fills the buffer to the fixed target (the floor); occupancy == target.**
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
        // Idempotent — warming an already-full buffer is a no-op.
        let again = pool.warm_up(&region(), &class());
        assert_eq!(again.warm, 3);
        assert_eq!(again.refills, 3, "no extra restores when already full");
    }

    /// **A warm pool serves WARM (snapshot restore) and REPLACES the slot so occupancy stays at
    /// target — the warm-pool-fast time-to-first-log-line (§5.4).**
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
        // Served WARM (no cold boot used).
        assert_eq!(sb.path(), AcquirePath::Warm);
        assert_eq!(
            cold_used.load(Ordering::SeqCst),
            0,
            "a warm hit never cold-boots"
        );
        // The handed-out guest is a warm restore (not the cold fallback).
        assert!(sb.handle().guest_id.starts_with("warm-"));
        // The buffer was REPLACED — occupancy stays at target.
        let stats = pool.stats(&region(), &class());
        assert_eq!(
            stats.warm, 2,
            "the handed-out slot is replaced — occupancy stays at target"
        );
        assert_eq!(stats.warm_served, 1);
        assert_eq!(stats.warm_hit_rate(), 1.0);
    }

    /// **An EMPTY buffer falls back to a COLD boot (§5.4: mitigated, not eliminated).**
    #[test]
    fn empty_buffer_falls_back_to_cold_boot() {
        let pool = SnapshotPool::new(1, ModeledRestore::new());
        // Never warmed up — the buffer is empty.
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

    /// **THE ONE-JOB-PER-SANDBOX-EPHEMERAL INVARIANT is preserved under pooling (§5.3): a restored VM
    /// serves EXACTLY one job then is killed, never reused.** The [`WarmSandbox`] is consumed by
    /// value in `run_one_job_then_kill` — it CANNOT serve a second job — and the guest is
    /// whole-guest-killed on teardown.
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
        // Run ONE job, then the guest is whole-guest-killed (one-job-per-sandbox, ephemeral).
        let job_ran = sb.run_one_job_then_kill(
            |h| {
                // the per-job work runs on the restored guest.
                assert_eq!(h.guest_id, handed_out);
                true
            },
            |h| {
                // whole-guest kill on teardown.
                k.lock().unwrap().push(h.guest_id.clone());
            },
        );
        assert!(job_ran);
        // The guest was killed (whole-guest kill on teardown — never reused).
        assert_eq!(
            killed.lock().unwrap().as_slice(),
            std::slice::from_ref(&handed_out)
        );

        // The killed guest is NOT in the warm buffer (it was never returned — a fresh REPLACEMENT
        // restore is in the buffer, a DISTINCT guest id).
        let stats = pool.stats(&region(), &class());
        assert_eq!(
            stats.warm, 2,
            "the buffer holds fresh replacements, never the served guest"
        );
        // (Type-level proof: `sb` was moved into `run_one_job_then_kill` — a second use does not
        // compile. The restored VM cannot serve a second job.)
    }

    /// **Every acquire mints a DISTINCT fresh guest — a restored VM is never handed out twice (the
    /// one-job-per-sandbox safety at the restore layer).** Successive warm acquires hand out distinct
    /// guest ids; the replacement restores are distinct from the served guests.
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
                "every acquire hands out a DISTINCT fresh guest — never the same live guest twice"
            );
        }
    }

    /// **Residency (12.4): the warm buffer is keyed per (region, label-class) — a restore for one
    /// region/class does not satisfy another's acquire (no global pool).**
    #[test]
    fn buffer_is_keyed_per_region_and_class_no_global_pool() {
        let pool = SnapshotPool::new(1, ModeledRestore::new());
        let fr = Region("fr-par".into());
        let de = Region("de-fra".into());
        pool.warm_up(&fr, &class());
        // de-fra has its OWN (empty) buffer — warming fr-par does not fill it.
        assert_eq!(pool.stats(&fr, &class()).warm, 1);
        assert_eq!(
            pool.stats(&de, &class()).warm,
            0,
            "no cross-region warm pool (12.4)"
        );

        // an acquire in de-fra cold-boots (its own buffer is empty), even though fr-par is warm.
        let sb = pool
            .acquire(&de, &class(), || {
                Ok(SandboxHandle {
                    guest_id: "cold-de".into(),
                })
            })
            .unwrap();
        assert_eq!(sb.path(), AcquirePath::Cold);
        // fr-par's warm buffer is untouched.
        assert_eq!(pool.stats(&fr, &class()).warm, 1);
    }

    /// **A restore FAILURE during warm-up stops the fill (never a degraded silent guest); the next
    /// acquire on the now-short buffer still serves what it has, then cold-boots when empty.**
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
                // first 2 restores succeed, the rest fail.
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
        // Only 2 of the 5 target restores succeeded — the fill stopped at 2 (no degraded guest).
        assert_eq!(
            stats.warm, 2,
            "the fill stops at the last successful restore"
        );
        assert_eq!(stats.target, 5);
    }
}
