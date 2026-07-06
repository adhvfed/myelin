# Issue tracker (myelin-issues)

_The myelin-issues crate is very healthy: permission gating is fail-closed per operation, the list_objects SetExpr lowering is injection-safe and leak-free by construction (confidential-as-set-difference, no post-filter), the reorder CAS and workflow FSM are correct and exhaustively tested, refs project() is permission-first, and the GDPR erase fan-out reaches every holder with loud-on-incomplete semantics. One real correctness bug exists in the SLA at-risk nudge recomputation across pause/resume; everything else reviewed is sound._

**Kept findings:** 1  (🟡 1 medium)

---

### 1. 🟡 SLA at-risk (80%) nudge is recomputed as 80% of remaining budget on resume, drifting the warning off the total-budget threshold

- **Severity:** medium  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** correctness
- **Location:** `crates/myelin-issues/src/sla_calendar.rs:744`

**What:** On initial arm, at_risk_fire_at is positioned at 80% of the TOTAL business budget (arm_with_bps line 647-648: at_risk_budget(target_business_secs, bps)). But resume() (line 744) recomputes it as at_risk_budget(run.remaining_business_secs, run.at_risk_bps) — 80% of the budget still REMAINING measured from now, ignoring how much was already consumed before the pause. Meanwhile the breach fire_at on resume (line 743) uses the full remaining_business_secs, correctly preserving the total budget across pause/resume. The two computations thus use inconsistent semantics: breach tracks the total budget, the at-risk nudge does not.

**Impact:** The proactive 'SLA at-risk' signal (crate::trigger TellWhenSlaAtRisk / escalation-prevention) fires late on every pause/resume. Using the repo's own test numbers (8h/28800s target, 80% at-risk, pause at 2h consumed leaving 6h): correct at-risk = 6.4h total = 4.4h after resume, but the code arms 0.8*6h = 4.8h after resume — 24 min late. When more than 80% of the budget was already consumed before the pause, the nudge re-arms to fire after the 6.4h threshold has already passed, so it fires uselessly late. Any SLA that pauses (e.g. waiting-on-customer) gets a mistimed at-risk nudge.

**Fix:** Position the at-risk instant against the TOTAL budget: consumed = target_business_secs - remaining_business_secs; at_risk_remaining = max(0, at_risk_budget(target_business_secs, bps) - consumed); at_risk_fire_at = business_fire_at(now, at_risk_remaining, cal). When the threshold is already passed, fire immediately (or emit at_risk on resume). Add a resume-time test asserting the at-risk fire point stays at 80% of the total budget across a mid-window pause.

> _Verifier note:_ Confirmed in source: line 647-648 computes at_risk_budget from target_business_secs at arm; line 743 recomputes breach fire_at from full remaining_business_secs (total budget preserved); line 744 recomputes at_risk_budget from remaining_business_secs (drifts). at_risk_budget (line 870-872) = target*bps/10000. Test pause_resume_preserves_budget_to_the_second (tests.rs line 293-324) asserts only resumed.fire_at (breach), never at_risk_fire_at; test at_risk_nudge_fires_only_while_running (line 405) tests only running-state gating, not timing across resume — so no test pins the at-risk timing after resume, as the finding claims. The finding's worst-case arithmetic (0.48h/7.48h) is slightly wrong (should be 0.8h/7.8h for 7h consumed) but the direction/severity hold. Medium severity confirmed: degrades a proactive signal, not data loss or security.
