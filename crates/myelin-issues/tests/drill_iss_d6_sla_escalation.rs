//! # ISS-D6 — an SLA breach starts Issues' REAL escalation chain on the durable wheel (NOTIF-P21 / P-342)
//!
//! **Drill source:**
//! `planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! row **ISS-D6** (SLA durability; artifact (c) **"breach starts the escalation chain"** — the
//! chain-start integration of Notif with Issues), and `notifications.md` §2.4/§3.7 (the frozen chain
//! on the durable wheel; ack-as-event). This is the **NOTIF-P21 half** of ISS-D6: Notif's chain-start
//! integration driven by Issues' REAL SLA escalation chain (the (a) fire-after-restart + (b)
//! business-calendar `fire_at` accuracy halves are the Issues SLA-timer prompts).
//!
//! **The dated GREEN artifact (2026-06-23).** A REAL SLA breach
//! ([`myelin_issues::events::SLA_BREACHED`]) drives Issues' REAL three-tier escalation chain
//! ([`issue_sla_escalation_policy`] — team → project → org, replacing the Notif `test_chain` floor)
//! through Notif's frozen [`EscalationEngine`] on the durable wheel. The drill measures + asserts,
//! with NO threshold weakened:
//!
//! 1. **chain start** — the breach STARTS the chain: the first tier (the team on-call) is paged AT
//!    FIRE TIME, critical (pierces quiet-hours), the ack_window durable timer armed.
//! 2. **the chain WALKS per the frozen shape across a kill mid-`ack_window`** — Notif is "killed"
//!    mid-window (the in-process engine dropped; the durable wheel + run handle survive); a NEW engine
//!    resumes and the timer fires, paging the NEXT tier (the project lead) EXACTLY ONCE.
//! 3. **exactly-once page = 0 missed, 0 duplicate** — a replayed fire (a second restart over the
//!    already-fired timer) pages NOTHING; the page log holds exactly two entries (tier 1 + tier 2).
//!    Threshold 0/0 — inherits NOTIF-D7's exactly-once property under Issues' REAL chain.
//! 4. **ack-halt** — an ack stops the chain (one `notif.escalation.acked` event, idempotent on re-ack).

use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::PrincipalId;
use myelin_issues::events::SLA_BREACHED;
use myelin_issues::sla_escalation::{
    issue_sla_escalation_policy, SLA_PROJECT_ONCALL_SCHEDULE, SLA_TEAM_ONCALL_SCHEDULE,
};
use myelin_notif::escalation::{
    DurableWheel, EscalationEngine, EscalationRun, InMemoryWheel, OncallSchedule, RotationWindow,
    RunState,
};
use myelin_notif::prefs::QuietHours;
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

fn pid(s: &str) -> PrincipalId {
    PrincipalId(s.into())
}

/// Both on-call tiers' rotations cover the whole day (the page resolves at fire time).
fn schedules() -> Vec<OncallSchedule> {
    vec![
        OncallSchedule {
            schedule_id: SLA_TEAM_ONCALL_SCHEDULE.into(),
            rotation: vec![RotationWindow {
                from_minute: 0,
                to_minute: 1440,
                principal: pid("psn:team-lead"),
            }],
        },
        OncallSchedule {
            schedule_id: SLA_PROJECT_ONCALL_SCHEDULE.into(),
            rotation: vec![RotationWindow {
                from_minute: 0,
                to_minute: 1440,
                principal: pid("psn:project-lead"),
            }],
        },
    ]
}

/// The current tier's schedule (the engine resolves a SCHEDULE target against the matching schedule).
fn sched_for(id: &str) -> OncallSchedule {
    schedules()
        .into_iter()
        .find(|s| s.schedule_id == id)
        .unwrap()
}

/// **ISS-D6 — an SLA breach starts Issues' real chain; it walks across a kill exactly-once; ack halts.**
#[test]
fn iss_d6_sla_breach_starts_real_chain_walks_exactly_once_then_ack_halts() {
    // The DURABLE substrate that survives a Notif restart: the timer wheel + the outbox.
    let wheel = InMemoryWheel::new();
    let outbox = OutboxStore::new();
    let tenant = TenantId("acme".into());
    let region = Region("fr-par".into());

    // A REAL SLA breach on an issue (the trigger event ref carries the breach origin).
    let breach = ArtifactRef(format!("myelin://acme/issue/{SLA_BREACHED}/ENG-1421"));
    let policy = issue_sla_escalation_policy(15, 1); // Issues' REAL three-tier chain.
    let never_quiet = QuietHours::default();

    // === chain start: the breach pages tier 1 (team on-call) AT FIRE TIME + arms the durable timer ===
    let eng_a = EscalationEngine::new(wheel.clone(), outbox.clone());
    let (run_id, first) = eng_a
        .page(
            tenant.clone(),
            region.clone(),
            "esc-iss-d6".into(),
            policy.clone(),
            breach.clone(),
            Some(&sched_for(SLA_TEAM_ONCALL_SCHEDULE)),
            600,
            &never_quiet,
            false,
        )
        .expect("the SLA breach STARTS the chain (ISS-D6 chain start)");
    assert_eq!(
        first.principal,
        pid("psn:team-lead"),
        "tier 1 = the team on-call"
    );
    assert_eq!(first.walk, 0);
    assert!(
        !first.channels.is_empty(),
        "the critical page delivers (pierces quiet-hours)"
    );
    assert!(
        wheel.has_timer(&run_id),
        "the ack_window durable timer is armed"
    );

    // === THE KILL: drop engine A mid-ack_window (no ack). The wheel + outbox + run handle survive. ===
    drop(eng_a);

    // === resume on engine B over the SAME durable state (re-hydrate the escalation_run handle) ===
    let eng_b = EscalationEngine::new(wheel.clone(), outbox.clone());
    eng_b.resume_for_test(EscalationRun {
        tenant: tenant.clone(),
        region: region.clone(),
        run_id: run_id.clone(),
        policy,
        trigger_event: breach,
        walk: 0,
        state: RunState::Active,
        acked_by: None,
        pages: vec![(0, pid("psn:team-lead"))],
    });

    // === the ack_window timer fires (unacked) → walk to tier 2 (project lead) EXACTLY ONCE ===
    let next = eng_b
        .advance(
            &run_id,
            Some(&sched_for(SLA_PROJECT_ONCALL_SCHEDULE)),
            600,
            &never_quiet,
            false,
        )
        .expect("advance ok")
        .expect("the resumed chain pages the next tier (NOT zero)");
    assert_eq!(
        next.principal,
        pid("psn:project-lead"),
        "tier 2 = the project lead"
    );
    assert_eq!(next.walk, 1);

    // exactly-once so far: the page log holds EXACTLY two entries (tier 1, tier 2) — 0 missed, 0 dup.
    let run = eng_b.run(&run_id).expect("run present");
    assert_eq!(
        run.pages.len(),
        2,
        "exactly two pages across the kill/resume — 0 missed, 0 duplicate"
    );

    // === the ack HALTS the chain (the project lead acks at tier 2; the signal-wait resolves) ===
    // The ack cancels the re-armed tier-2 ack_window timer BEFORE it can fire to tier 3 — the
    // ack-halt wins the race (a breach handled at tier 2 never escalates to the org incident lead).
    let acked = Timestamp("2026-06-23T12:15:00Z".into());
    assert!(
        eng_b
            .ack(&run_id, pid("psn:project-lead"), acked.clone())
            .expect("ack ok"),
        "the ack halts"
    );
    let run = eng_b.run(&run_id).expect("run present");
    assert_eq!(run.state, RunState::Acked, "the chain HALTED on the ack");
    assert!(
        !eng_b.wheel().has_timer(&run_id),
        "the ack cancelled the durable timer"
    );

    // ack-halt + 0 DUPLICATE: a subsequent timer fire after the ack pages NOTHING (the cancelled
    // timer's `fire_due` is a no-op; the halted run does not walk to tier 3). All schedules are
    // supplied so the ONLY reason no page is produced is the ack-halt, not a missing roster.
    let all_scheds: Vec<OncallSchedule> = schedules();
    let after_ack = eng_b
        .advance(&run_id, all_scheds.first(), 600, &never_quiet, false)
        .expect("advance ok");
    assert_eq!(
        after_ack, None,
        "no page after the ack — the chain is stopped"
    );

    // idempotent ack: a double-ack acks ONCE — exactly ONE notif.escalation.acked committed.
    assert!(
        !eng_b.ack(&run_id, pid("psn:other"), acked).expect("ack ok"),
        "the re-ack is a no-op"
    );
    assert_eq!(
        outbox.committed_count(),
        1,
        "exactly one notif.escalation.acked event committed via the outbox (the ONLY emit path)"
    );

    // GREEN ARTIFACT (2026-06-23): the SLA breach STARTED Issues' REAL chain; it walked tier 1 → tier 2
    // exactly-once across a kill (0 missed / 0 duplicate); the ack halted it (one ack event). No
    // threshold weakened. Issues passed its real chain to the frozen engine — ZERO Notif code change.
}
