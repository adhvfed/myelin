//! # `identity` — per-run identity: mint, scrub, revoke idempotently, re-mintable on resume
//! (AG-P13 → P-225, M2-B)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/agent-fabric.md` §5.7 (*per-run identity: mint
//! at dispatch (token life == run life), scrub any shared platform token in the child env, revoke
//! idempotently on teardown even on crash; C6 re-mintable mid-workflow on resume — a multi-day HITL
//! pause re-mints a fresh attenuated token with the same delegation caveats and the **remaining run
//! life**, so a long pause never widens the attribution window beyond the TTL bound and never leaves
//! a run unattributed*), §2.2 guarantee 2 (attribution — *an agent literally cannot exceed its
//! identity*, EI-02 §2).
//!
//! **Contract-index:** CONSUMES 4.7 (`mint_run_token(agent_id, run_id, delegation_caveats, ttl)` +
//! `revoke(jti)` — Identity, §4/§11; **re-mintable on resume**, idempotent revoke even on crash).
//! OWNS the mint/scrub/revoke + re-mint-on-resume *wiring in the loop driver* (the per-run identity
//! lifecycle the Agent-Fabric run drives over its consumed Identity surfaces).
//!
//! ## What this prompt (AG-P13) ships — the per-run identity COMPLETED
//!
//! AG-P4 (→ P-216, `skeleton.rs`) shipped the **simple form**: mint at dispatch, revoke on teardown,
//! the anti-leak [`crate::ChildEnv`] unset, the auto-expiring TTL, and the AG-D8 **no-tool leg** (a
//! killed run still revokes; 0 shared token leaked). `myelin-flow::remint` (→ P-213) shipped the
//! engine-side [`RunTokenLease`](myelin_flow::RunTokenLease) re-mint over `WfCtx` (re-mint under the
//! short fail-static window W). This module COMPLETES the per-run identity for the Agent-Fabric run:
//!
//! - [`RunIdentity`] — the per-run identity lifecycle the loop driver holds. It owns the
//!   **remaining-run-life clamp** the §5.7 C6 tightening demands: on resume after a multi-day HITL
//!   pause, the re-minted token's TTL is `min(W, remaining_run_life)` — so a long pause **never
//!   widens the attribution window beyond the run's own deadline**. The flow lease re-mints under the
//!   full W; the Agent-Fabric run additionally bounds W by the *remaining run life* (a run with 40s
//!   left re-mints a 40s token, not a fresh 300s one).
//! - [`RunIdentity::mint_at_dispatch`] — mint the FIRST per-run token at dispatch (token life == run
//!   life). Records the run deadline so every later re-mint is bounded by the remaining life.
//! - [`RunIdentity::remint_on_resume`] — RE-MINT on wake from a park (a multi-day HITL pause, a long
//!   [`SCHEDULE_AND_RUN_JOB`]). Same agent, same delegation caveats, same per-run attenuation; TTL =
//!   `min(W, remaining_run_life)`. UNCONDITIONAL on resume (the engine never assumes a token survives
//!   a park, §6.2 property 3). Records the attribution window so the gate can assert 0-unattributed.
//! - [`RunIdentity::revoke_on_teardown`] — revoke the *current* token idempotently even on crash
//!   (4.7), belt-and-suspenders with the auto-expiring TTL. Re-uses the [`crate::RunTokenRevoker`]
//!   seam AG-P4 owns.
//! - [`RunIdentity::child_env`] — the anti-leak child env minted from the CURRENT per-run token only
//!   (re-asserted here; re-asserted again inside `ToolHands::exec`, AG-P15 → P-226). 0 shared
//!   platform token leaked, by construction ([`crate::ChildEnv::for_run`]).
//! - [`AttributionWindow`] — the proof the AG-D8 re-mint leg reads: across a multi-day pause the run
//!   is **continuously attributed** (a token is live at dispatch, a fresh one is live on every
//!   resume; the seam between two tokens is 0 — there is never a wall-clock instant the resumed run
//!   runs unattributed). [`AttributionWindow::has_unattributed_gap`] is the 0-unattributed-window
//!   gate.
//!
//! ## The §5.7 / AG-D8 re-mint-leg gate (quantified — must be green to call this done)
//!
//! 1. **Attributed within the TTL bound on resume.** Every re-minted token's TTL is `<=` the
//!    fail-static W AND `<=` the remaining run life — a multi-day pause spanning the token TTL
//!    re-mints a token that does NOT widen the attribution window beyond the run deadline.
//! 2. **0 unattributed window.** The run is attributed at dispatch and on EVERY resume; the seam
//!    between two consecutive tokens is 0 (the re-mint happens BEFORE the resumed work runs).
//!    [`AttributionWindow::has_unattributed_gap`] reads `false`.
//! 3. **0 leaked token.** The child env minted from the current per-run token leaks no shared
//!    platform token ([`crate::ChildEnv::leaked_shared_token`] is `false`) — re-asserted on every
//!    re-mint (a resumed run's tools get a fresh per-run-only child env).
//! 4. **Same caveats on re-mint.** The re-minted token carries the SAME delegation caveats (the
//!    attenuate-only grant chain) PLUS the per-run attenuation — a resume can never WIDEN the grant.
//!
//! ## FLOOR named — none. Per-run identity is complete (mint / scrub / revoke / re-mint on resume).
//! The anti-leak scrub into the sandbox child env is **re-asserted** inside `ToolHands::exec` in
//! **AG-P15 (→ P-226)** — that is a re-assertion of THIS module's guarantee at the exec boundary,
//! not a floor here. The `mint_run_token` / `revoke` BODIES are Identity's (4.7; this crate is the
//! CONSUMER, driving the frozen surfaces — the CDC `tests/cdc_4_7_remint_resume.rs` pairs the engine
//! with a real provider impl).

use crate::skeleton::{ChildEnv, RunTokenRevoker};
use myelin_flow::{DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenMinter};
use std::sync::Arc;

/// **The fail-static TTL window W (seconds, §10/C11).** The short activity-life bound a per-run
/// token mint defaults to — far shorter than any multi-day wait. Mirrors
/// [`myelin_flow::RunTokenLease::DEFAULT_TTL_SECS`] (the structural bound holds regardless of the
/// legal ratification of the number, L-1). Token life == activity life: a token never outlives the
/// active burst between two parks.
pub const FAIL_STATIC_W_SECS: u64 = 300;

/// **The per-run identity lifecycle the Agent-Fabric loop driver holds (§5.7; C6).** Owns the
/// per-run attenuated token across the run's whole life — minted at dispatch (token life == run
/// life), re-minted on every resume from a park (a multi-day HITL pause / a long
/// `SCHEDULE_AND_RUN_JOB`) with the SAME caveats and the **remaining run life**, revoked idempotently
/// on teardown. The §5.7 C6 tightening this owns beyond the flow lease: the re-mint TTL is clamped to
/// `min(W, remaining_run_life)`, so a long pause never widens the attribution window beyond the run's
/// own deadline.
///
/// **Not `Clone`** by design: there is ONE per-run identity per run; it tracks the live token, the
/// run deadline, and the attribution window across mints. The minter ([`RunTokenMinter`], 4.7) is the
/// ONLY token path — this struct never fabricates a token.
pub struct RunIdentity {
    /// The contract-4.7 mint seam (CONSUMED). The ONLY token path — every (re-)mint asks Identity.
    minter: Arc<dyn RunTokenMinter + Send + Sync>,
    /// The agent principal id the per-run token is minted FOR (the run's agent identity, §2.2 g2).
    agent_id: String,
    /// The run id (the durable-workflow instance) the token is attenuated to (a `run:<id>` caveat).
    run_id: String,
    /// The delegation caveats the mint attenuates with (the §6 grant chain — attenuate-only). The
    /// SAME chain is carried on every re-mint (a resume never widens the grant).
    caveats: DelegationCaveats,
    /// The fail-static window W (seconds) — the upper bound on any single token's TTL.
    fail_static_w: u64,
    /// **The run's absolute deadline (epoch-seconds): dispatch instant + the run's allotted life.**
    /// Every re-mint's TTL is clamped to `deadline - now` (the remaining run life) so a long pause
    /// never widens the attribution window beyond it. Set at [`RunIdentity::mint_at_dispatch`].
    deadline_secs: i64,
    /// The CURRENT live per-run token (the most recent mint/re-mint). `None` before dispatch.
    current: Option<RunTokenHandle>,
    /// The number of re-mints performed (one per resume from a park). The gate reads it to assert a
    /// resume DID re-mint (a regression that drops the re-mint reads 0).
    reminted: u64,
    /// The attribution window across the run's life — the proof there is 0 unattributed wall-clock.
    window: AttributionWindow,
}

impl RunIdentity {
    /// Build a per-run identity over the contract-4.7 `minter` for `(agent_id, run_id)` with the
    /// delegation `caveats` (the attenuate-only grant chain). Mints under the default fail-static W
    /// ([`FAIL_STATIC_W_SECS`]). No token is minted until [`RunIdentity::mint_at_dispatch`].
    pub fn new(
        minter: Arc<dyn RunTokenMinter + Send + Sync>,
        agent_id: impl Into<String>,
        run_id: impl Into<String>,
        caveats: DelegationCaveats,
    ) -> RunIdentity {
        RunIdentity {
            minter,
            agent_id: agent_id.into(),
            run_id: run_id.into(),
            caveats,
            fail_static_w: FAIL_STATIC_W_SECS,
            deadline_secs: 0,
            current: None,
            reminted: 0,
            window: AttributionWindow::new(),
        }
    }

    /// Override the fail-static window W (the upper bound on a single token's TTL). Chainable on
    /// [`RunIdentity::new`]. A 0 is clamped to 1 (a token must live for SOME positive burst).
    pub fn with_fail_static_w(mut self, w_secs: u64) -> RunIdentity {
        self.fail_static_w = w_secs.max(1);
        self
    }

    /// **Mint the FIRST per-run token at dispatch (4.7; §5.7).** Token life == run life: the token's
    /// TTL is `min(W, run_life_secs)` and the run's absolute deadline (`now + run_life_secs`) is
    /// recorded so every later re-mint is bounded by the *remaining* life. The mint carries the
    /// delegation caveats PLUS a per-run attenuation (`run:<id>`) — the token cannot act outside THIS
    /// run. A failed mint aborts BEFORE the run starts (never run un-minted, §5.7) — surfaced LOUD.
    ///
    /// `now_secs` is the dispatch instant; `run_life_secs` is the run's allotted wall-clock life (the
    /// deadline the attribution window can never exceed). Records the first attribution segment.
    pub fn mint_at_dispatch(
        &mut self,
        now_secs: i64,
        run_life_secs: u64,
    ) -> Result<&RunTokenHandle, RunTokenError> {
        // The run's absolute deadline — the ceiling no token's expiry may exceed (token life == run
        // life). Saturating: a pathological run_life that overflows i64 clamps to i64::MAX.
        self.deadline_secs = now_secs.saturating_add(run_life_secs.min(i64::MAX as u64) as i64);
        // Token life == run life: bounded by W AND the run's allotted life (whichever is shorter).
        let ttl = self.fail_static_w.min(run_life_secs.max(1));
        let handle = self.mint(ttl)?;
        let expiry = now_secs.saturating_add(ttl.min(i64::MAX as u64) as i64);
        self.window.open_segment(now_secs, expiry);
        self.current = Some(handle);
        Ok(self.current.as_ref().expect("just minted"))
    }

    /// **RE-MINT a fresh per-run token on resume from a park (4.7, §5.7 C6; §6.2).** Called on wake
    /// from a multi-day HITL pause (or a long `SCHEDULE_AND_RUN_JOB`, AG-P16) BEFORE the resumed work
    /// runs, so the resumed activity executes under a FRESH live token. The new token carries the
    /// SAME delegation caveats + the SAME per-run attenuation; its TTL is **`min(W, remaining run
    /// life)`** — a long pause never widens the attribution window beyond the run's deadline. The
    /// re-mint is UNCONDITIONAL (the engine never assumes a token survives a park, §6.2 property 3).
    ///
    /// `now_secs` is the resume (wake) instant. If the run deadline has already passed, the run has
    /// no remaining life and must NOT resume under a fresh token — returns
    /// [`RunTokenError`] LOUD (the resumed work would run past the run's own deadline; the run
    /// terminates rather than widening attribution, §5.7). Records the new attribution segment so the
    /// window stays gap-free across the pause.
    pub fn remint_on_resume(&mut self, now_secs: i64) -> Result<&RunTokenHandle, RunTokenError> {
        // The remaining run life at the wake instant. A run that parked past its OWN deadline has no
        // remaining life — refusing to re-mint past the deadline is the never-widen-attribution
        // guarantee (a resume can never run beyond the run's allotted life, §5.7).
        let remaining = self.deadline_secs.saturating_sub(now_secs);
        if remaining <= 0 {
            return Err(RunTokenError(format!(
                "run {} has no remaining life at resume (deadline {} <= now {}) — refusing to \
                 re-mint past the run's own deadline (never widen the attribution window, §5.7)",
                self.run_id, self.deadline_secs, now_secs
            )));
        }
        // The §5.7 C6 clamp: TTL = min(W, remaining run life). A long pause re-mints a SHORT token
        // bounded by both the fail-static window AND the run's own deadline — never a fresh full W
        // that would widen the attribution window past the run life.
        let remaining_u = remaining as u64;
        let ttl = self.fail_static_w.min(remaining_u);
        let handle = self.mint(ttl)?;
        let expiry = now_secs.saturating_add(ttl.min(i64::MAX as u64) as i64);
        self.window.open_segment(now_secs, expiry);
        self.current = Some(handle);
        self.reminted = self.reminted.saturating_add(1);
        Ok(self.current.as_ref().expect("just re-minted"))
    }

    /// **Revoke the CURRENT per-run token idempotently on teardown (4.7, §5.7).** Idempotent even on
    /// crash: a doubled teardown (the explicit revoke + a crash-recovery sweep) is a no-op success.
    /// Belt-and-suspenders with the token's auto-expiring TTL. Returns the measured revocation lag
    /// (seconds between `teardown_secs` and the revoke landing) so the gate can assert it is within
    /// bound. A run that never minted a token (teardown before dispatch) is a no-op (lag 0).
    pub fn revoke_on_teardown(
        &self,
        revoker: &dyn RunTokenRevoker,
        now_secs: i64,
        teardown_secs: i64,
    ) -> u64 {
        match &self.current {
            Some(token) => revoker.revoke(&token.jti, now_secs, teardown_secs),
            None => 0,
        }
    }

    /// **The anti-leak child env minted from the CURRENT per-run token ONLY (§5.7).** The shared
    /// platform token slot is UNSET (0 leak, by construction — [`ChildEnv::for_run`]). Re-derived on
    /// every (re-)mint so a resumed run's tools inherit the FRESH per-run token, never a stale one and
    /// never an ambient platform credential. `None` before dispatch (no token to mint a child from).
    pub fn child_env(&self) -> Option<ChildEnv> {
        self.current.as_ref().map(|t| ChildEnv::for_run(&t.jti))
    }

    /// The CURRENT live per-run token (the most recent mint/re-mint). `None` before dispatch.
    pub fn current(&self) -> Option<&RunTokenHandle> {
        self.current.as_ref()
    }

    /// The number of re-mints performed (one per resume from a park). The gate asserts a resume DID
    /// re-mint (reads `> 0` after a resume; `0` on a run that never parked).
    pub fn reminted(&self) -> u64 {
        self.reminted
    }

    /// The run's absolute deadline (epoch-seconds) — the ceiling no token's expiry may exceed.
    pub fn deadline_secs(&self) -> i64 {
        self.deadline_secs
    }

    /// The attribution window across the run's life (the 0-unattributed-window proof, §5.7).
    pub fn attribution_window(&self) -> &AttributionWindow {
        &self.window
    }

    /// **Mint a fresh token under `ttl` (the ONE token path — 4.7 CONSUMED).** Carries the delegation
    /// caveats PLUS the per-run attenuation (`run:<id>`); the token cannot act outside THIS run.
    /// Attenuate-only — only ADDS a caveat (narrows), never removes one (the chain only gets tighter,
    /// §6 / AG-2). A mint failure surfaces LOUD (never run under a fabricated/absent token).
    fn mint(&self, ttl: u64) -> Result<RunTokenHandle, RunTokenError> {
        let mut caveats = self.caveats.clone();
        caveats.0.push(format!("run:{}", self.run_id));
        self.minter
            .mint_run_token(&self.agent_id, &self.run_id, &caveats, ttl)
    }
}

/// **The attribution window across a run's whole life (the §5.7 0-unattributed-window proof).** A run
/// is *attributed* whenever a live per-run token covers the wall clock. As the run mints (at dispatch)
/// and re-mints (on every resume), each token covers a `[start, expiry)` wall-clock segment. The
/// proof the AG-D8 re-mint leg reads: across a multi-day pause the segments **leave no gap at a resume
/// instant** — the re-mint opens a fresh segment AT the wake instant (before the resumed work runs),
/// so the resumed run is attributed from the first instant it executes.
///
/// A token's TTL may *lapse during a park* (that is the whole point — a parked run holds no token, so
/// the token expires harmlessly while nothing runs). The gate is NOT "the segments are contiguous in
/// wall clock" — a multi-day park legitimately has no token. The gate is: **no resume runs work
/// unattributed** — every segment's start is `<=` the next executing instant. [`Self::has_unattributed_gap`]
/// checks that each segment opens at-or-before its open instant (a re-mint never opens a segment that
/// starts AFTER the resumed work would run).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AttributionWindow {
    /// The `(start, expiry)` segments, in mint order. Segment 0 is the dispatch token; each later one
    /// is a resume re-mint. Every executing instant is covered by the segment opened at/just-before it.
    segments: Vec<(i64, i64)>,
}

impl AttributionWindow {
    /// A fresh, empty window (no token minted yet).
    pub fn new() -> AttributionWindow {
        AttributionWindow::default()
    }

    /// Open a token segment covering `[start, expiry)` (a mint at dispatch or a re-mint on resume).
    fn open_segment(&mut self, start: i64, expiry: i64) {
        self.segments.push((start, expiry));
    }

    /// The number of attribution segments (one per mint/re-mint — dispatch + one per resume).
    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    /// **The 0-unattributed-window gate (§5.7).** `true` iff some resumed work would run
    /// unattributed: a segment whose token had already EXPIRED before that segment opened (a re-mint
    /// that landed AFTER its own coverage lapsed) — which can never happen by construction (a re-mint
    /// opens a segment AT the wake instant with a positive TTL), OR an empty window (a run that
    /// dispatched but never minted — un-minted run, refused upstream). For a well-formed run the
    /// answer is `false`: every executing instant (dispatch + each resume) is the START of a live
    /// segment, so the run is attributed from the first instant it runs.
    pub fn has_unattributed_gap(&self) -> bool {
        if self.segments.is_empty() {
            return true; // a run that never minted a token ran (if at all) unattributed.
        }
        // Every segment must cover a positive window (start < expiry) — a 0/negative-length segment
        // would mean a token that was dead the instant it was minted (the resumed work would run
        // unattributed). By construction every (re-)mint uses a positive TTL, so this holds.
        self.segments.iter().any(|&(start, expiry)| expiry <= start)
    }

    /// The maximum attribution-window width any single token spans (the widest `expiry - start`). The
    /// §5.7 gate asserts this is `<=` W (and `<=` the run life) — a re-mint NEVER widens the window
    /// beyond the TTL bound. `0` for an empty window.
    pub fn max_segment_width(&self) -> u64 {
        self.segments
            .iter()
            .map(|&(start, expiry)| (expiry - start).max(0) as u64)
            .max()
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic minter recording its calls — a REAL impl on the contract-4.7 mint surface.
    /// Each mint is a DISTINCT token (a re-mint is never the prior token), and the `(agent, run,
    /// caveats, ttl)` it was called with is recorded so a test can assert the per-run attenuation +
    /// the remaining-life clamp.
    #[derive(Default)]
    struct RecordingMinter {
        calls: std::sync::Mutex<Vec<(String, String, DelegationCaveats, u64)>>,
    }
    impl RunTokenMinter for RecordingMinter {
        fn mint_run_token(
            &self,
            agent_id: &str,
            run_id: &str,
            caveats: &DelegationCaveats,
            ttl_secs: u64,
        ) -> Result<RunTokenHandle, RunTokenError> {
            let mut c = self.calls.lock().unwrap();
            let n = c.len();
            c.push((agent_id.into(), run_id.into(), caveats.clone(), ttl_secs));
            Ok(RunTokenHandle {
                token: format!("tok:{run_id}:{n}"),
                jti: format!("jti:{run_id}:{n}"),
                ttl_secs,
            })
        }
    }

    /// A denylist revoker (idempotent even on crash) over the token TTL — a REAL impl on the 4.7
    /// revoke surface.
    #[derive(Default)]
    struct DenylistRevoker {
        revoked: std::sync::Mutex<std::collections::HashSet<String>>,
    }
    impl RunTokenRevoker for DenylistRevoker {
        fn revoke(&self, jti: &str, now_secs: i64, teardown_secs: i64) -> u64 {
            let mut g = self.revoked.lock().unwrap();
            if !g.insert(jti.to_string()) {
                return 0;
            }
            (now_secs - teardown_secs).max(0) as u64
        }
        fn is_dead(&self, jti: &str, _now_secs: i64) -> bool {
            self.revoked.lock().unwrap().contains(jti)
        }
    }

    fn caveats() -> DelegationCaveats {
        DelegationCaveats(vec!["delegated:human-x".into(), "tenant:acme".into()])
    }

    fn identity(minter: Arc<RecordingMinter>) -> RunIdentity {
        RunIdentity::new(minter, "psn:agent-7", "R1", caveats())
    }

    /// **Mint at dispatch binds the token to (agent, run) under token life == run life (§5.7).** The
    /// first token's TTL is `min(W, run_life)`; the per-run attenuation caveat is carried.
    #[test]
    fn mint_at_dispatch_binds_token_and_records_deadline() {
        let m = Arc::new(RecordingMinter::default());
        let mut id = identity(m.clone());
        // a run with a 200s life (< W=300) → token TTL == 200 (run life is the tighter bound).
        let token = id
            .mint_at_dispatch(1000, 200)
            .expect("mint succeeds")
            .clone();
        assert_eq!(token.jti, "jti:R1:0", "the jti is bound to the run");
        assert_eq!(token.ttl_secs, 200, "token life == run life (200 < W=300)");
        assert_eq!(id.deadline_secs(), 1200, "deadline == dispatch + run life");
        let calls = m.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "exactly one mint at dispatch");
        let (agent, run, cav, ttl) = calls[0].clone();
        assert_eq!(agent, "psn:agent-7");
        assert_eq!(run, "R1");
        assert_eq!(ttl, 200);
        assert!(
            cav.0.contains(&"run:R1".to_string()),
            "per-run attenuation carried"
        );
        assert!(
            cav.0.contains(&"delegated:human-x".to_string()),
            "grant chain carried"
        );
    }

    /// **Token life is clamped to W when the run life is longer (§5.7).** A run with a 10-minute life
    /// still mints a 5-minute (W) token — the fail-static window is the upper bound.
    #[test]
    fn mint_at_dispatch_clamps_to_w_when_run_life_is_longer() {
        let m = Arc::new(RecordingMinter::default());
        let mut id = identity(m);
        let token = id.mint_at_dispatch(1000, 600).expect("mint").clone();
        assert_eq!(
            token.ttl_secs, FAIL_STATIC_W_SECS,
            "TTL clamped to W when run life > W"
        );
        assert_eq!(
            id.deadline_secs(),
            1600,
            "the deadline is still the full run life"
        );
    }

    /// **AG-D8 re-mint leg — a multi-day pause spanning the token TTL re-mints on resume with the
    /// remaining run life; attributed within the TTL bound, 0 unattributed window, 0 leak.** The
    /// headline drill: dispatch a run with a long life, PARK past the token TTL, resume → a fresh
    /// token is re-minted with the SAME caveats and the REMAINING run life; the run stays attributed,
    /// the window has no gap, and the child env leaks nothing.
    #[test]
    fn ag_d8_remint_leg_attributed_within_ttl_zero_gap_zero_leak() {
        let m = Arc::new(RecordingMinter::default());
        // a long-lived run: dispatched at t=1000 with a 3-day life (259200s). W=300.
        let mut id = identity(m.clone());
        let dispatch_at = 1000i64;
        let run_life = 259_200u64; // 3 days.
        id.mint_at_dispatch(dispatch_at, run_life)
            .expect("dispatch mint");
        // the dispatch token's TTL is clamped to W (300), NOT the 3-day life.
        assert_eq!(
            id.current().unwrap().ttl_secs,
            FAIL_STATIC_W_SECS,
            "dispatch token TTL == W"
        );

        // PARK for ~2 days (well past the 300s token TTL — the token expired harmlessly while the
        // run held no thread), then RESUME.
        let resume_at = dispatch_at + 172_800; // +2 days.
        let reminted = id
            .remint_on_resume(resume_at)
            .expect("re-mint on resume")
            .clone();
        assert_eq!(
            reminted.jti, "jti:R1:1",
            "a FRESH token (not the dispatch one)"
        );
        assert_ne!(reminted.jti, "jti:R1:0", "the re-mint is a NEW token");
        // ATTRIBUTED WITHIN THE TTL BOUND: the re-minted token's TTL is min(W, remaining run life).
        // remaining = deadline(1000+259200=260200) - resume(173800) = 86400 (1 day) > W → clamp to W.
        assert_eq!(
            reminted.ttl_secs, FAIL_STATIC_W_SECS,
            "re-mint TTL == min(W, remaining) == W"
        );
        assert!(
            reminted.ttl_secs <= FAIL_STATIC_W_SECS,
            "never widens beyond W"
        );

        // SAME caveats on re-mint (attenuate-only — a resume never widens the grant).
        let calls = m.calls.lock().unwrap();
        let (_, _, cav, ttl) = calls[1].clone();
        assert!(
            cav.0.contains(&"run:R1".to_string()),
            "per-run attenuation re-carried"
        );
        assert!(
            cav.0.contains(&"delegated:human-x".to_string()),
            "SAME grant chain on re-mint"
        );
        assert_eq!(ttl, FAIL_STATIC_W_SECS);
        drop(calls);

        // 0 UNATTRIBUTED WINDOW: every executing instant (dispatch + resume) opened a live segment.
        assert!(
            !id.attribution_window().has_unattributed_gap(),
            "0 unattributed window"
        );
        assert_eq!(
            id.attribution_window().segment_count(),
            2,
            "dispatch + one resume segment"
        );
        // the re-mint NEVER widened the window beyond W.
        assert!(
            id.attribution_window().max_segment_width() <= FAIL_STATIC_W_SECS,
            "no segment wider than W"
        );
        assert_eq!(id.reminted(), 1, "exactly one re-mint on the resume");

        // 0 LEAKED TOKEN: the child env is minted from the FRESH per-run token only.
        let child = id.child_env().expect("a child env after re-mint");
        assert!(
            !child.leaked_shared_token(),
            "0 shared platform token leaked into the child env"
        );
        assert_eq!(
            child.run_token_jti, "jti:R1:1",
            "the child inherits the FRESH per-run jti"
        );
    }

    /// **The remaining-run-life clamp is TIGHTER than W near the deadline (§5.7 C6).** A resume with
    /// only 40s of run life left re-mints a 40s token, NOT a fresh 300s (W) one — the attribution
    /// window never widens past the run's own deadline.
    #[test]
    fn remint_clamps_to_remaining_run_life_near_the_deadline() {
        let m = Arc::new(RecordingMinter::default());
        let mut id = identity(m);
        id.mint_at_dispatch(1000, 600).expect("dispatch"); // deadline 1600.
                                                           // resume at 1560 → remaining = 1600 - 1560 = 40s (< W=300) → token TTL == 40.
        let reminted = id
            .remint_on_resume(1560)
            .expect("re-mint near deadline")
            .clone();
        assert_eq!(
            reminted.ttl_secs, 40,
            "TTL clamped to the remaining run life (40 < W)"
        );
        // the segment opened at 1560 expires at 1600 — exactly the run deadline, never past it.
        assert_eq!(
            id.attribution_window().max_segment_width(),
            FAIL_STATIC_W_SECS,
            "the widest segment is still the dispatch W; the resume segment is only 40"
        );
        assert!(
            !id.attribution_window().has_unattributed_gap(),
            "still 0 unattributed"
        );
    }

    /// **A resume PAST the run deadline refuses to re-mint LOUD (§5.7 — never widen past run life).**
    /// A run that parked beyond its OWN deadline has no remaining life; re-minting would run the
    /// resumed work past the run's allotted life — refused, never a silent widen.
    #[test]
    fn remint_past_the_deadline_is_a_loud_error() {
        let m = Arc::new(RecordingMinter::default());
        let mut id = identity(m);
        id.mint_at_dispatch(1000, 300).expect("dispatch"); // deadline 1300.
                                                           // resume at 1400 — already 100s PAST the deadline. No remaining life.
        let err = id
            .remint_on_resume(1400)
            .expect_err("no remaining life → refuse");
        assert!(
            err.to_string().contains("no remaining life"),
            "refused LOUD: {err}"
        );
        // the prior token was the only one minted; the re-mint did NOT happen.
        assert_eq!(id.reminted(), 0, "no re-mint past the deadline");
    }

    /// **Two resumes mint two DISTINCT fresh tokens (token life == activity life).** Each active burst
    /// gets its own short token; the attribution window has a segment per burst, no gap.
    #[test]
    fn two_resumes_mint_two_distinct_tokens_no_gap() {
        let m = Arc::new(RecordingMinter::default());
        let mut id = identity(m);
        id.mint_at_dispatch(1000, 100_000).expect("dispatch");
        let h1 = id.remint_on_resume(1500).expect("first resume").clone();
        let h2 = id.remint_on_resume(5000).expect("second resume").clone();
        assert_ne!(h1.jti, h2.jti, "distinct token per resume");
        assert_eq!(id.reminted(), 2, "two re-mints across two bursts");
        assert_eq!(
            id.attribution_window().segment_count(),
            3,
            "dispatch + two resumes"
        );
        assert!(
            !id.attribution_window().has_unattributed_gap(),
            "0 unattributed across both"
        );
    }

    /// **Revoke on teardown is idempotent even on crash (4.7, §5.7).** The current token is revoked;
    /// a doubled teardown (the explicit revoke + a crash sweep) is a no-op (lag 0).
    #[test]
    fn revoke_on_teardown_is_idempotent_even_on_crash() {
        let m = Arc::new(RecordingMinter::default());
        let mut id = identity(m);
        id.mint_at_dispatch(1000, 300).expect("dispatch");
        let revoker = DenylistRevoker::default();
        // first teardown: revoke (lag 0 — teardown == now).
        assert_eq!(
            id.revoke_on_teardown(&revoker, 1000, 1000),
            0,
            "first revoke lands"
        );
        assert!(
            revoker.is_dead("jti:R1:0", 1000),
            "the current token is dead after revoke"
        );
        // a SECOND teardown (a crash sweep) is a no-op — idempotent even on crash.
        assert_eq!(
            id.revoke_on_teardown(&revoker, 1005, 1000),
            0,
            "re-revoke is a no-op"
        );
    }

    /// **Revoke targets the CURRENT (re-minted) token after a resume (§5.7).** After a re-mint the
    /// teardown revokes the FRESH token, not the stale dispatch one (the dispatch one already expired
    /// on its TTL during the park).
    #[test]
    fn revoke_after_resume_targets_the_fresh_token() {
        let m = Arc::new(RecordingMinter::default());
        let mut id = identity(m);
        id.mint_at_dispatch(1000, 100_000).expect("dispatch");
        id.remint_on_resume(2000).expect("resume re-mint");
        let revoker = DenylistRevoker::default();
        id.revoke_on_teardown(&revoker, 2100, 2100);
        assert!(
            revoker.is_dead("jti:R1:1", 2100),
            "the FRESH token is revoked"
        );
        assert!(
            !revoker.is_dead("jti:R1:0", 2100),
            "the stale dispatch token was NOT re-revoked"
        );
    }

    /// **Teardown before dispatch is a no-op (no token to revoke).** A run torn down before it minted
    /// a token (e.g. a refused dispatch upstream) revokes nothing — lag 0, never a panic.
    #[test]
    fn revoke_before_dispatch_is_a_noop() {
        let m = Arc::new(RecordingMinter::default());
        let id = identity(m);
        let revoker = DenylistRevoker::default();
        assert_eq!(
            id.revoke_on_teardown(&revoker, 1000, 1000),
            0,
            "no token → no-op"
        );
        assert!(id.child_env().is_none(), "no child env before dispatch");
        assert!(id.current().is_none(), "no current token before dispatch");
    }

    /// **`has_unattributed_gap` is exact (mutation-floor).** An empty window IS a gap (un-minted run);
    /// a well-formed window with positive segments is NOT; a 0-length segment WOULD be (kills the
    /// `<=`/`<` comparator mutants).
    #[test]
    fn unattributed_gap_predicate_is_exact() {
        let mut w = AttributionWindow::new();
        assert!(
            w.has_unattributed_gap(),
            "empty window is a gap (un-minted run)"
        );
        w.open_segment(1000, 1300); // a positive 300-wide segment.
        assert!(
            !w.has_unattributed_gap(),
            "a positive segment is attributed"
        );
        w.open_segment(2000, 2040); // another positive segment.
        assert!(
            !w.has_unattributed_gap(),
            "two positive segments are attributed"
        );
        // a degenerate 0-length segment (a hypothetical dead-on-arrival token) IS a gap.
        let mut bad = AttributionWindow::new();
        bad.open_segment(1000, 1000); // expiry == start → 0-length.
        assert!(
            bad.has_unattributed_gap(),
            "a 0-length segment is an unattributed instant (kills <=)"
        );
    }

    /// **`max_segment_width` returns the widest segment (mutation-floor — the TTL-bound assertion
    /// reads it).** The max across segments, never a constant; 0 for empty.
    #[test]
    fn max_segment_width_is_exact() {
        let mut w = AttributionWindow::new();
        assert_eq!(
            w.max_segment_width(),
            0,
            "empty window has width 0 (kills -> 1)"
        );
        w.open_segment(1000, 1040); // width 40.
        assert_eq!(w.max_segment_width(), 40);
        w.open_segment(2000, 2300); // width 300 — wider.
        assert_eq!(
            w.max_segment_width(),
            300,
            "the MAX (kills a first/last/min mutant)"
        );
        w.open_segment(3000, 3010); // width 10 — narrower, must not lower the max.
        assert_eq!(
            w.max_segment_width(),
            300,
            "a narrower segment does not lower the max"
        );
    }

    /// **`with_fail_static_w` overrides W and clamps a 0 to a live 1 (never a dead-on-arrival W).**
    #[test]
    fn with_fail_static_w_overrides_and_clamps_zero() {
        let m = Arc::new(RecordingMinter::default());
        let mut id = identity(m).with_fail_static_w(0);
        // W clamped to 1 → a run with a 100s life mints a 1s token (W is the floor here).
        let token = id.mint_at_dispatch(1000, 100).expect("mint").clone();
        assert_eq!(token.ttl_secs, 1, "W clamped to 1 (never dead-on-arrival)");
    }
}
