//! **Shared panic-safe test-state teardown.** Every test in this crate's `tests/*.rs` that writes
//! process/suffix-tagged `cell`/`tenant_placement`/`cell_provisioning` rows (e.g. `cellself<pid>`,
//! `cellw6c<pid>`) called its own `cleanup(pool, &suffix)` helper ONLY as a bare statement at the
//! very end of the happy path — a mid-test `assert_eq!`/`.expect(..)` panic (of which several of
//! these tests have many, by design: they probe adversarial/negative-boot paths) skips that call
//! entirely, leaving the tagged rows behind forever. Confirmed live on this host: 8 orphaned
//! `cellself*` rows in `public.cell`, none ever cleaned up, one old enough to make a LATER,
//! unrelated invocation's own `assert_eq!(sh.cell().cell_id...)` pick up a stale prior run's row
//! instead of its own fresh one (`cell()`'s query does not filter to "this exact run", by design —
//! this crate's registry models a SINGLE-cell fleet) — the exact same structural bug already
//! root-caused and fixed across 22+ files in 8 sibling crates (`myelin-storage`'s
//! `tests/common::with_cleanup` is the identical shape); this crate was missed from that sweep.
//!
//! [`with_cleanup`] closes the gap: it runs the test's real body, then UNCONDITIONALLY runs the
//! test's own `cleanup(...)` call afterward — success, a failed assertion, or a panic all still
//! clean up. A synchronous `Drop` impl cannot safely run an async cleanup query, so this catches an
//! in-flight panic with `FutureExt::catch_unwind`, always runs cleanup, then resumes the unwind so
//! the test still fails/reports exactly as it did before (`cargo test` output is unchanged either
//! way).
#![cfg(feature = "integration")]

use futures::FutureExt;

/// Run `body()`, then unconditionally run `cleanup()` afterward — regardless of whether `body`
/// finished normally or panicked (a failed `assert!`/`assert_eq!`/`.expect(..)`). The panic (if
/// any) is re-raised AFTER `cleanup` has run, so the test still fails/reports correctly; it is
/// never swallowed.
pub async fn with_cleanup<BodyFut, CleanupFut>(
    body: impl FnOnce() -> BodyFut,
    cleanup: impl FnOnce() -> CleanupFut,
) where
    BodyFut: std::future::Future<Output = ()>,
    CleanupFut: std::future::Future<Output = ()>,
{
    let result = std::panic::AssertUnwindSafe(body()).catch_unwind().await;
    cleanup().await;
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}
