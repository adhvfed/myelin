//! # `ci_result_signal` — the X-1 `ci.result` ROLLUP SIGNAL that wakes Git's merge queue (CI-P19 → P-362, M4)
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §4 step 4 (emit the `ci.result` rollup SIGNAL once all required contexts terminal; what CI does
//! NOT do); `03-events-contracts-and-glue.md` §1.1 (the frozen `ci.result` token — a SIGNAL, not a
//! bus event) + §4 (the `CheckStatus` seam — what CI owns vs what Git owns).
//! **Reconciliation:** `00-reconciliation-decisions.md` X-1 / OQ-A (the `ci.result` merge-queue
//! signal; an `untrusted_fork` success neutral-for-gating until endorsed — CI emits the fact, Git
//! gates).
//! **Contracts:** OWNED (the producer half, the rollup) 5.9 (the `ci.result` rollup signal — CI is
//! the PRODUCER). CONSUMED 2.9 (the token), 9.4 (the durable `ci.result` signal Git's merge-queue
//! workflow waits on — `wait_for_signal("ci.result", idem_key=<merge_attempt_id>)`).
//!
//! ## What CI-P19 ships here — the ROLLUP SIGNAL (the seam-closer), distinct from the bus event
//!
//! CI's `ci.pipeline` body ([`crate::ci_pipeline`], CI-P15/P-358) already emits TWO X-1 producer flows
//! per run:
//!   1. the per-context `ci.check.updated` BUS EVENTS (the PR-checks UI feed + Git's projection), via
//!      the outbox (CI-P18 owns the `CheckStatus` assembly);
//!   2. the `ci.result` BUS EVENT (the per-aggregate-ordered carriage of the rollup, on the
//!      `(repo, commit_oid)` aggregate).
//!
//! BUT the merge-queue durable workflow does NOT wait on a bus event — it parks on a durable SIGNAL:
//! `wait_for_signal("ci.result", idem_key=<merge_attempt_id>)` (contract 9.4, §4 step 4). CI-P19 is
//! the seam-closer: it turns CI's rollup into THAT signal, idempotent on the `idem_token`. The signal
//! is references-not-payloads (the [`myelin_flow::encode_ci_result`] codec — `Vec<ArtifactRef>`, never
//! a PII body), delivered into the run's `wf_signal` store under the FROZEN `ci.result` signal name
//! ([`myelin_flow::CI_RESULT_SIGNAL`]). The `wf_signal` PK `(tenant, run_id, signal_name, idem_key)`
//! makes a doubly-delivered rollup a single buffered row — the merge-queue workflow wakes EXACTLY
//! ONCE (0 double-merge).
//!
//! ## The rollup derivation (the frozen 5.9 verdict — NO local divergence, EI-01 §7)
//!
//! [`CiResultSignal::rollup`] does NOT invent a verdict — it REUSES the FROZEN
//! [`myelin_events::check_seam::rollup_ci_result`] (success iff EVERY required context succeeded; a
//! missing required context closes the gate, never an implicit pass) over the run's CURRENT
//! per-context verdicts. This is the byte-identical rollup [`myelin_events::check_seam::ci_result_draft`]
//! carries on the Bus AND the verdict the merge-queue body decodes off the signal — ONE shape, no
//! second rollup language.
//!
//! ## The fork-gating discipline at the seam (X-1 / OQ-A — CI emits the fact, Git gates)
//!
//! CI rolls up the OVERALL verdict (`success`/`failure`) over the contexts' states. It does NOT carry
//! the per-context `trust_tier` on the rollup — that rides the per-context `ci.check.updated` facts
//! (CI-P18). A fork run can roll up `overall: success` (its pipeline reported green); Git's merge gate
//! re-asserts the per-context trust posture OFF its OWN projection at the merge step (an
//! `untrusted_fork` success is NEUTRAL for gating until endorsed — the poisoned-pipeline defence). **CI
//! emits the fact; Git gates; CI never merges.** This module never reads `trust_tier`, never decides
//! `required` (it CONSUMES Git's required set), never merges.
//!
//! ## NAMED FLOORS
//!
//! None new — this CLOSES the X-1 seam. The seam is frozen to the 5.9 shape EXACTLY; CI never diverges
//! it (a needed change is a whole-workspace contract PR, escalated and written down). The end-to-end
//! GIT-D10/CI-D8 gate (CI's REAL `run_ci_pipeline_body` → this rollup signal → Git's `run_merge_attempt`
//! merge queue; supersession + fork-neutral + endorse + 0 double-merge) is proven in
//! `tests/drills_ci_p19_seam_gate.rs`.

use myelin_events::check_seam::{rollup_ci_result, CiOverall, CiResult};
use myelin_flow::{encode_ci_result, SignalRow, SignalStore, CI_RESULT_SIGNAL};
use myelin_tenancy::{Region, TenantId};
use std::collections::BTreeMap;

/// **The X-1 `ci.result` rollup-signal producer (contract 5.9 / arch §4 step 4).** Bound to a run's
/// `wf_signal` store + its `(tenant, region)` residency partition + the run id the merge-queue
/// workflow keys on. CI DERIVES the rollup over Git's required gate set and DELIVERS it as the durable
/// `ci.result` signal the merge-queue workflow parks on — idempotent on the `idem_token` (=
/// `merge_attempt_id`). It owns ONLY the producer half (the rollup + the signal delivery); it never
/// merges, never decides `required`, never reads `trust_tier`.
pub struct CiResultSignal<'a> {
    /// The run's durable inbound-signal store (the `wf_signal` substrate, §3.4). A delivery is an
    /// `INSERT … ON CONFLICT (tenant, run_id, signal_name, idem_key) DO NOTHING` — the PK is the
    /// dedup, so a doubly-delivered rollup is ONE buffered row (one merge-queue wake).
    signals: &'a SignalStore,
    /// The `(tenant, region)` partition + residency pin every signal row carries (EI-02 §1).
    tenant: TenantId,
    /// The residency pin (the rollup signal lives in the run's region).
    region: Region,
    /// The merge-queue run id the signal is buffered for (the `wf_signal` PK's run dimension) — the
    /// run the merge-queue durable workflow drives, NOT CI's own run (CI delivers INTO the merge
    /// queue's wait).
    merge_queue_run: String,
}

impl<'a> CiResultSignal<'a> {
    /// A rollup-signal producer bound to the merge-queue run's signal store + `(tenant, region)`
    /// partition + the run id the merge-queue workflow waits on.
    pub fn new(
        signals: &'a SignalStore,
        tenant: TenantId,
        region: Region,
        merge_queue_run: impl Into<String>,
    ) -> CiResultSignal<'a> {
        CiResultSignal {
            signals,
            tenant,
            region,
            merge_queue_run: merge_queue_run.into(),
        }
    }

    /// **Derive the `ci.result` rollup CI emits (the FROZEN 5.9 verdict — REUSES
    /// [`rollup_ci_result`]).** `current` maps a context name → did-it-succeed (the run's
    /// post-supersession per-context verdict — the body emits every context the same overall verdict
    /// on a single attempt, but the shape supports a mixed set). `required` is Git's required gate set
    /// (CI CONSUMES it, never decides it). `idem_token` is the `merge_attempt_id` the merge queue
    /// minted (the no-coordination dedup key, OQ-F). Pure/deterministic: the same inputs → a
    /// byte-identical rollup (the idempotent-wake precondition), byte-identical to what
    /// [`myelin_events::check_seam::ci_result_draft`] carries on the Bus.
    pub fn rollup(
        &self,
        commit_oid: &str,
        current: &BTreeMap<String, bool>,
        required: &[String],
        idem_token: &str,
    ) -> CiResult {
        rollup_ci_result(commit_oid, current, required, idem_token)
    }

    /// **Emit the `ci.result` rollup SIGNAL once all required contexts are terminal (arch §4 step 4).**
    /// Derives the rollup over Git's required set, encodes it references-not-payloads
    /// ([`encode_ci_result`] — `Vec<ArtifactRef>`, never a PII body), and DELIVERS it into the
    /// merge-queue run's `wf_signal` store under the FROZEN `ci.result` signal name, keyed on the
    /// `idem_token`. Returns [`RollupDelivery`] distinguishing the FIRST delivery (the merge-queue
    /// workflow wakes) from an at-least-once DUPLICATE (absorbed by the `wf_signal` PK — one wake,
    /// never two). **This wakes Git's merge-queue durable workflow (9.4); CI does NOT merge.**
    pub fn signal_ci_result(
        &self,
        commit_oid: &str,
        current: &BTreeMap<String, bool>,
        required: &[String],
        idem_token: &str,
    ) -> RollupDelivery {
        let result = self.rollup(commit_oid, current, required, idem_token);
        self.deliver(&result)
    }

    /// **Deliver an already-derived [`CiResult`] rollup as the `ci.result` signal (idempotent on its
    /// `idem_token`).** The split-out delivery half: a re-drive that re-derives the SAME rollup
    /// delivers under the SAME `idem_key` → the `wf_signal` PK dedups it (one wake). The body is the
    /// references-not-payloads codec; the `idem_token` is the signal's `idem_key` (carried by the
    /// envelope, never the body — exactly the merge-queue's [`myelin_flow::decode_ci_result`] contract).
    pub fn deliver(&self, result: &CiResult) -> RollupDelivery {
        let row = SignalRow {
            tenant: self.tenant.clone(),
            region: self.region.clone(),
            run_id: self.merge_queue_run.clone(),
            signal_name: CI_RESULT_SIGNAL.to_string(),
            // The signal's idem_key IS the merge_attempt_id (the no-coordination dedup key) — NOT in
            // the payload body (the merge-queue decodes it off the envelope, §6.5).
            idem_key: result.idem_token.clone(),
            // references-not-payloads: the rollup flattened into PII-free machine tokens.
            payload: encode_ci_result(result),
            // No inline PII (references-not-payloads) → no crypto-shred key on the row.
            payload_key_ref: None,
            // This in-memory compatibility producer has no wall-clock input. Production delivery
            // uses PgFlowExecutor, which stamps the durable database receipt time.
            received_unix_ms: 0,
            consumed_seq: None,
        };
        // `deliver` models INSERT … ON CONFLICT DO NOTHING: `true` = a NEW buffered row (the workflow
        // wakes), `false` = a re-delivery under the SAME PK (absorbed — one wake, not two).
        if self.signals.deliver(row) {
            RollupDelivery::Woke
        } else {
            RollupDelivery::Duplicate
        }
    }

    /// Did the run's overall verdict roll up to `success`? A thin helper for the producer caller to
    /// log/branch on (the merge queue reads the verdict off the decoded signal, not this).
    pub fn is_success(result: &CiResult) -> bool {
        result.overall == CiOverall::Success
    }
}

/// **The outcome of delivering a `ci.result` rollup signal — a loud, typed distinction between a real
/// merge-queue wake and an absorbed at-least-once duplicate (never a silent drop).** The
/// doubly-delivered rollup is OBSERVABLE so the GIT-D10/CI-D8 drill can assert "exactly one wake →
/// merge-count == 1".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RollupDelivery {
    /// The FIRST delivery for this `idem_token` — the merge-queue workflow wakes (exactly once).
    Woke,
    /// A re-delivery of a `idem_token` already buffered — absorbed by the `wf_signal` PK (one wake
    /// total, never a second merge attempt; the X-1 / 9.4 idempotency).
    Duplicate,
}

#[cfg(test)]
#[path = "ci_result_signal_tests.rs"]
mod tests;
