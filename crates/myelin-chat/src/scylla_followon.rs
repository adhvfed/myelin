//! # `scylla_followon` — the CHAT-M5 measured-trigger-gated ScyllaDB hot-tier floor (CHAT-P28 →
//! global P-502).
//!
//! **Status note (DATED 2026-06-25; re-date on any change — a claim that outlives its verification
//! misleads the next agent, VISION §3 / EI-01 §1).** This module is a *gap-report*: it NAMES the
//! ScyllaDB hot-tier promotion (M5-C-S2; the named M4-C1 floor, R-C6/R-5) and records whether its
//! measured trigger has FIRED. Per VISION §3 (name-your-floors — promotion is *triggered*, never
//! premature), EI-04 §4/§5 ("don't add it before the volume is *measured*"), and the chat
//! architecture's own measure-before-shard mandate (05-hard-problems §2; ADR-10), a promotion whose
//! trigger has NOT fired stays a **named floor — it is NOT built speculatively**, and its trigger
//! status is recorded here, dated.
//!
//! ## Why a gap-report, not a build (the trigger has not fired)
//! The ScyllaDB hot tier is the **named measured promotion, not the v1 default** (architecture 05
//! §2: "ScyllaDB the named measured promotion (R-5)"). It is taken ONLY when a cell's **measured
//! per-cell message-store write/partition volume crosses the hot-tier budget** (R-C6/R-5). That
//! signal has NOT been measured: the chat M5 surge family (CHAT-P26 / P-500) drove the 30× surge and
//! measured the **gateway SHED budgets** (the `ConnectionTier` / `AgentMention` lanes — the
//! delivery-side fairness signal), NOT the **message-store write/partition volume** crossing a
//! hot-tier budget. No per-cell write-volume measurement against a hot-tier budget exists, so the v1
//! **Postgres-partitioned hot tier ([`crate::store::pg::PgMessageStore`]) is RETAINED** and the
//! promotion **remains a named floor**. Building it now would be exactly the "add it before the
//! volume is measured" anti-pattern EI-04 §5 forbids and the "floor that masquerades as done"
//! VISION §3 forbids.
//!
//! ## What is already BUILT — the seam the promotion swaps behind (no rewrite)
//! The [`crate::store::MessageStore`] trait IS the hot-engine swap seam (architecture 01 §3.1): the
//! `append` / `range` / `revise` / `tombstone` / `resync_from` surface is identical under any hot
//! engine, and the cold tier ([`crate::store::ColdSegments`], now object-store-backed — the SIBLING
//! leg of this prompt, built + proven) is engine-independent. So the ScyllaDB promotion is a
//! **construction-time engine swap behind the unchanged trait**, NOT a redesign: a third
//! `MessageStore` impl (alongside [`crate::store::MemHotTier`] + `PgMessageStore`), residency-pinned
//! and crypto-shred-capable per cell, and CHAT-D2 + CHAT-D8 re-run across the swap (they were
//! written to survive it).
//!
//! ## The object-store BlobStore swap (the SIBLING leg) is BUILT, not floored
//! The cold-segment fs→object-store swap (contract 11.2) that rides this promotion **is shipped**
//! (it is a one-line, provable-now backing change): [`crate::store::ColdSegments`] is generic over
//! `B: BlobStore`, production seals to [`myelin_storage::s3blob::S3BlobStore`], and
//! [`crate::store::chat_cold_blob_store_parity`] proves byte-identity fs↔object (CI fs↔fs +
//! `--features integration` fs↔S3 against the live dev-stack RustFS). That leg is NOT in this
//! gap-report — only the trigger-gated Scylla hot-tier promotion is.
//!
//! ## The gap-report invariant (this prompt's gate)
//! The follow-on below is recorded as a [`FloorFollowOn`] with a NON-EMPTY trigger, follow-on,
//! preserved-contract, and a dated [`TriggerStatus`]. [`scylla_floor_gap_report`] asserts **0
//! invisible gaps** (every must-be-non-empty field recorded) AND the **honest-floor invariant**:
//! while the trigger is `NotFired`, the promotion MUST be a named floor (`built == false`) — a
//! promotion built without a fired trigger is the premature-promotion failure EI-04 §5 / VISION §3
//! forbid. The vocabulary mirrors the CI control-plane's `floor_followons` manifest (EI-01 §7 — the
//! same red-until-proven shape across subsystems; chat cannot depend on the CI crate, so the shape
//! is re-realised, not imported).

/// The measured status of the promotion's trigger. Red-until-proven: the promotion is built ONLY
/// once its trigger is `Fired` with dated, measured evidence (never speculatively).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerStatus {
    /// The measured trigger has **not** fired — the promotion REMAINS a named floor (not built).
    /// Carries the dated note recording *why* it is unfired (what measurement is still owed). EI-04
    /// §5: don't add it before the measurement.
    NotFired {
        /// The dated note (e.g. `"2026-06-25: no per-cell write-volume measurement exists"`).
        as_of: &'static str,
    },
    /// The measured trigger HAS fired — the promotion is unblocked. Carries the dated measured
    /// evidence (the write/partition-volume reading that crossed the hot-tier budget).
    Fired {
        /// The dated measured evidence.
        evidence: &'static str,
    },
}

impl TriggerStatus {
    /// True iff the trigger has fired (the promotion may be built). While false, the promotion MUST
    /// remain a named floor.
    pub fn has_fired(&self) -> bool {
        matches!(self, TriggerStatus::Fired { .. })
    }

    /// The dated note/evidence string (non-empty in both arms — an undated status is itself a gap).
    pub fn dated(&self) -> &'static str {
        match self {
            TriggerStatus::NotFired { as_of } => as_of,
            TriggerStatus::Fired { evidence } => evidence,
        }
    }
}

/// One measured-trigger-gated floor follow-on row, machine-checked by the gap-report.
///
/// The gap-report asserts every must-be-non-empty field IS non-empty (no invisible gap) AND the
/// honest-floor invariant: `built` is true ONLY when `trigger.has_fired()` (no premature promotion).
#[derive(Clone, Copy, Debug)]
pub struct FloorFollowOn {
    /// A short stable id (e.g. `"scylla-hot-tier"`).
    pub id: &'static str,
    /// One line: what this promotion IS (the thing the floor is promoted TO).
    pub what: &'static str,
    /// What is already BUILT — the seam the promotion swaps behind (so it is a swap, not a rewrite).
    pub built_seam: &'static str,
    /// The contract the promotion MUST preserve across the swap. MUST be non-empty.
    pub preserved_contract: &'static str,
    /// The MEASURED trigger that must fire to start the work (which signal measures it). Non-empty.
    pub trigger: &'static str,
    /// The measured status of that trigger (red-until-proven).
    pub status: TriggerStatus,
    /// What the follow-on actually delivers once the trigger fires. MUST be non-empty.
    pub follow_on: &'static str,
    /// The gate that must be green to call the promotion done (only relevant once built). Non-empty.
    pub promotion_gate: &'static str,
    /// `true` iff the promotion has been BUILT. The honest-floor invariant requires this be `false`
    /// while `status` is `NotFired` — a built promotion with an unfired trigger is a premature
    /// promotion (EI-04 §5 / VISION §3 forbid it).
    pub built: bool,
}

impl FloorFollowOn {
    /// True iff this row is fully recorded — no invisible gap.
    pub fn is_fully_recorded(&self) -> bool {
        !self.id.is_empty()
            && !self.what.is_empty()
            && !self.built_seam.is_empty()
            && !self.preserved_contract.is_empty()
            && !self.trigger.is_empty()
            && !self.status.dated().is_empty()
            && !self.follow_on.is_empty()
            && !self.promotion_gate.is_empty()
    }

    /// The honest-floor invariant (EI-04 §5 / VISION §3): a promotion is built ONLY once its trigger
    /// has fired. While the trigger is `NotFired`, the promotion MUST remain a named floor.
    pub fn honours_no_premature_promotion(&self) -> bool {
        // built ⇒ trigger fired. (Equivalently: ¬fired ⇒ ¬built.)
        !self.built || self.status.has_fired()
    }
}

/// **The ScyllaDB hot-tier promotion — the ONE measured-trigger-gated chat-M5 floor this prompt
/// (CHAT-P28 / P-502) names.** Stays a named floor until a cell's measured per-cell message-store
/// write/partition volume crosses the hot-tier budget; the gap-report enforces no premature
/// promotion.
pub const SCYLLA_HOT_TIER_FLOOR: FloorFollowOn = FloorFollowOn {
    id: "scylla-hot-tier",
    what: "a ScyllaDB (wide-column) message hot tier promoted from the Postgres-partitioned v1 \
           floor — the proven infinite-scale chat-log shape (Discord's Cassandra->ScyllaDB), \
           residency-pinned + crypto-shred-capable per cell",
    built_seam: "the MessageStore trait (store::MessageStore) — the hot-engine swap seam (arch 01 \
                 §3.1): append/range/revise/tombstone/resync_from is identical under any hot engine, \
                 and the cold tier (store::ColdSegments, now object-store-backed) is \
                 engine-independent; MemHotTier + PgMessageStore are the two v1 impls",
    preserved_contract: "11.4 — the per-subject DEK crypto-shred the Scylla tier must preserve; \
                         12.1/12.4 — the (tenant, region) partition + residency-pin per cell; the \
                         MessageStore trait surface (0 behavioural divergence). Only the hot engine \
                         behind the trait changes, never the contract",
    trigger: "a cell's MEASURED per-cell message-store write/partition volume crossing the hot-tier \
              budget (R-C6/R-5); the measure-before-shard mandate (ADR-10) — not before measured",
    status: TriggerStatus::NotFired {
        as_of: "2026-06-25: NO per-cell message-store write/partition-volume measurement against a \
                hot-tier budget exists. The chat M5 surge family (CHAT-P26 / P-500) measured the \
                gateway SHED budgets (ConnectionTier/AgentMention lanes), NOT the message-store \
                write/partition volume crossing a hot-tier budget; a cell bounds the scale (one \
                region's tenants, ADR-11, not the planet), so the Postgres-partitioned hot tier is \
                correct. Floor remains named (measured-not-predicted: the measurement is owed).",
    },
    follow_on: "add a third MessageStore impl (ScyllaMessageStore) behind the unchanged trait, \
                residency-pinned + crypto-shred-capable per cell; migrate the hot partitions; the \
                object-segment cold tier is unchanged (engine-independent)",
    promotion_gate: "CHAT-D2 (per-conversation total order) + CHAT-D8 (0 recoverable PII) re-run \
                     GREEN across the swap — the order-violation + recoverable-PII signals = 0 \
                     post-swap (the drills were written to survive the swap) — SCHED",
    built: false,
};

/// Every floor follow-on this gap-report accounts for (the one Scylla hot-tier promotion). A `slice`
/// (not a single value) so the manifest extends uniformly if a future chat-M5 promotion is named.
pub const MEASURED_TRIGGER_FLOORS: &[FloorFollowOn] = &[SCYLLA_HOT_TIER_FLOOR];

/// **The gap-report gate (CHAT-P28 / P-502 — the "If NOT triggered" branch's dated row).** Asserts,
/// over every named floor: (1) it is FULLY recorded (0 invisible gaps), and (2) the honest-floor
/// invariant holds (a promotion is built ONLY once its trigger has fired). Returns `Ok(())` when the
/// gap-report is honest; an `Err` names the offending floor. This is the machine-checked equivalent
/// of the dated gap-report row the prompt requires.
pub fn scylla_floor_gap_report() -> Result<(), String> {
    gap_report_over(MEASURED_TRIGGER_FLOORS)
}

/// The gap-report verdict over an ARBITRARY slice of floors (so the invariant can be checked against
/// a deliberately-broken row in a test — the verdict's two failure modes are then load-bearing, not
/// vacuously `Ok`). An invisible gap OR a premature promotion is an `Err` naming the floor.
fn gap_report_over(floors: &[FloorFollowOn]) -> Result<(), String> {
    for f in floors {
        if !f.is_fully_recorded() {
            return Err(format!(
                "floor follow-on `{}` is an invisible gap (a must-be-non-empty field is empty)",
                f.id
            ));
        }
        if !f.honours_no_premature_promotion() {
            return Err(format!(
                "floor follow-on `{}` is a PREMATURE promotion — built with an unfired trigger \
                 (EI-04 §5 forbids adding it before the measurement)",
                f.id
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ScyllaDB hot-tier promotion is the one measured-trigger-gated chat-M5 floor this prompt
    /// names, and it is fully recorded (0 invisible gaps).
    #[test]
    fn the_scylla_floor_is_recorded_with_no_invisible_gap() {
        let ids: Vec<&str> = MEASURED_TRIGGER_FLOORS.iter().map(|f| f.id).collect();
        assert_eq!(ids, vec!["scylla-hot-tier"]);
        let floor = std::hint::black_box(SCYLLA_HOT_TIER_FLOOR);
        assert!(
            floor.is_fully_recorded(),
            "the Scylla hot-tier floor must be fully recorded (no must-be-non-empty field empty)"
        );
    }

    /// The honest-floor invariant: at 2026-06-25 the trigger has NOT fired (no per-cell write-volume
    /// measurement exists), so the promotion MUST remain a named floor (`built == false`) — no
    /// premature promotion. The whole gap-report passes.
    #[test]
    fn no_premature_promotion_trigger_unfired_stays_a_floor() {
        // Route the const through black_box so the checks are runtime assertions on the EXPORTED
        // manifest value (not const-folded) — the same value the rest of the platform reads.
        let floor = std::hint::black_box(SCYLLA_HOT_TIER_FLOOR);
        assert!(
            !floor.status.has_fired(),
            "the measured per-cell write/partition-volume trigger has NOT fired at this prompt's \
             execution"
        );
        assert!(
            !floor.built,
            "the Scylla hot-tier promotion is a NAMED FLOOR — not built speculatively"
        );
        assert!(
            floor.honours_no_premature_promotion(),
            "the honest-floor invariant holds: ¬fired ⇒ ¬built"
        );
        scylla_floor_gap_report().expect("the gap-report is honest — 0 invisible gaps");
    }

    /// The trigger names the MEASURED signal (write/partition volume) + the measure-before-shard
    /// mandate; the dated status is non-empty (an undated floor is itself a gap).
    #[test]
    fn the_trigger_names_the_measured_signal_and_is_dated() {
        let floor = std::hint::black_box(SCYLLA_HOT_TIER_FLOOR);
        assert!(floor.trigger.contains("write/partition volume"));
        assert!(floor.trigger.contains("ADR-10"));
        assert!(!floor.status.dated().is_empty());
        // The preserved-contract names the crypto-shred (11.4) + residency-pin (12.1/12.4) the swap
        // must hold — the reconciliation requirement on the promotion.
        assert!(floor.preserved_contract.contains("11.4"));
        assert!(floor.preserved_contract.contains("residency-pin"));
    }

    // ── the gap-report PREDICATES are load-bearing — drive them against BROKEN rows ──────────────
    // (the mutation floor for this manifest module: every conjunct of is_fully_recorded, the
    // honest-floor invariant, dated(), has_fired(), and the gap-report verdict must be exercised by
    // a row that FLIPS it, so a mutated predicate is caught — EI-01 §2/§3.)

    /// A fully-recorded, honest, unfired floor — the fixture every "break ONE field" case starts from.
    fn good_floor() -> FloorFollowOn {
        FloorFollowOn {
            id: "x",
            what: "x",
            built_seam: "x",
            preserved_contract: "x",
            trigger: "x",
            status: TriggerStatus::NotFired { as_of: "x" },
            follow_on: "x",
            promotion_gate: "x",
            built: false,
        }
    }

    /// `is_fully_recorded` is true for a complete row, and FALSE when ANY single must-be-non-empty
    /// field is empty — so every `&&` conjunct (and the dated() path) is load-bearing.
    #[test]
    fn is_fully_recorded_catches_each_empty_field() {
        assert!(good_floor().is_fully_recorded());
        // Each mutation empties exactly ONE field; the row must then be an invisible gap.
        let breakers: Vec<(&str, FloorFollowOn)> = vec![
            (
                "id",
                FloorFollowOn {
                    id: "",
                    ..good_floor()
                },
            ),
            (
                "what",
                FloorFollowOn {
                    what: "",
                    ..good_floor()
                },
            ),
            (
                "built_seam",
                FloorFollowOn {
                    built_seam: "",
                    ..good_floor()
                },
            ),
            (
                "preserved_contract",
                FloorFollowOn {
                    preserved_contract: "",
                    ..good_floor()
                },
            ),
            (
                "trigger",
                FloorFollowOn {
                    trigger: "",
                    ..good_floor()
                },
            ),
            (
                "status.dated",
                FloorFollowOn {
                    status: TriggerStatus::NotFired { as_of: "" },
                    ..good_floor()
                },
            ),
            (
                "follow_on",
                FloorFollowOn {
                    follow_on: "",
                    ..good_floor()
                },
            ),
            (
                "promotion_gate",
                FloorFollowOn {
                    promotion_gate: "",
                    ..good_floor()
                },
            ),
        ];
        for (field, broken) in breakers {
            assert!(
                !broken.is_fully_recorded(),
                "an empty `{field}` must make the row an invisible gap (the conjunct is load-bearing)"
            );
        }
    }

    /// `has_fired` distinguishes the two arms; `honours_no_premature_promotion` is the implication
    /// `built ⇒ fired` — a built+unfired row VIOLATES it, a built+fired row HONOURS it, an
    /// unbuilt+unfired row honours it.
    #[test]
    fn honest_floor_invariant_and_has_fired_are_load_bearing() {
        let unfired = TriggerStatus::NotFired { as_of: "d" };
        let fired = TriggerStatus::Fired { evidence: "e" };
        assert!(!unfired.has_fired());
        assert!(fired.has_fired());
        assert_eq!(unfired.dated(), "d");
        assert_eq!(fired.dated(), "e");

        // built + unfired = PREMATURE promotion (the invariant is violated).
        let premature = FloorFollowOn {
            built: true,
            status: unfired,
            ..good_floor()
        };
        assert!(!premature.honours_no_premature_promotion());
        // built + fired = honest (a real, triggered promotion).
        let honest_built = FloorFollowOn {
            built: true,
            status: fired,
            ..good_floor()
        };
        assert!(honest_built.honours_no_premature_promotion());
        // unbuilt + unfired = honest (the named-floor state).
        assert!(good_floor().honours_no_premature_promotion());
    }

    /// The gap-report verdict is load-bearing: it is `Ok` over the honest manifest, `Err` (naming the
    /// floor) over an invisible-gap row, and `Err` over a premature-promotion row.
    #[test]
    fn gap_report_verdict_distinguishes_honest_from_broken() {
        // Ok over the real (honest) manifest.
        assert!(scylla_floor_gap_report().is_ok());
        // Ok over an honest custom slice.
        assert!(gap_report_over(&[good_floor()]).is_ok());
        // Err (invisible gap) when a row has an empty must-be-non-empty field — naming the floor.
        let invisible_gap = FloorFollowOn {
            id: "gap-row",
            trigger: "",
            ..good_floor()
        };
        let err = gap_report_over(&[invisible_gap]).expect_err("an invisible gap is an Err");
        assert!(err.contains("gap-row") && err.contains("invisible gap"));
        // Err (premature promotion) when a row is built with an unfired trigger — naming the floor.
        let premature = FloorFollowOn {
            id: "premature-row",
            built: true,
            status: TriggerStatus::NotFired { as_of: "d" },
            ..good_floor()
        };
        let err = gap_report_over(&[premature]).expect_err("a premature promotion is an Err");
        assert!(err.contains("premature-row") && err.contains("PREMATURE"));
    }
}
