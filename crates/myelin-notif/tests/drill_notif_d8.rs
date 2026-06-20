//! # NOTIF-D8 — critical escalation pierces quiet-hours; a watching item is suppressed (P-192)
//!
//! **Drill source:**
//! `planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! row **NOTIF-D8** ("Set DND; fire a `critical` escalation → it PIERCES quiet-hours; a `watching`
//! item is suppressed." Artifact: **critical pierces; non-crit suppressed**; lane CI), and
//! `notifications.md` §2.4 (`pierce_classes` default critical — you cannot silence an on-call page)
//! / §2.2 (a non-piercing class inside a quiet window is suppressed from DELIVERY, never the inbox row).
//!
//! **The dated GREEN artifact (2026-06-20).** With the recipient in DND (a quiet window covering the
//! instant), two items are evaluated against the SAME quiet-hours: a CRITICAL escalation page and a
//! WATCHING ambient item. The drill measures + asserts, with NO threshold weakened:
//!
//! 1. **critical pierces** — the escalation [`notify_for`] decision for `Class::Critical` delivers on
//!    EVERY step channel despite the recipient being in a quiet window (`recipient_in_quiet = true`).
//!    `quiet_hours_pierce_count` increments (the §1.8 signal — counted here as the piercing-channel
//!    delivery). The on-call page is NOT silenced. Threshold: critical pierces — never softened.
//! 2. **non-crit suppressed** — the WATCHING item, evaluated through the router-side
//!    [`route`](myelin_notif::prefs::route) with the SAME quiet window, is suppressed off-cell:
//!    the off-cell push channels are dropped (only the in-cell in-app row remains; the inbox row is
//!    NEVER suppressed — §2.2). Threshold: non-crit suppressed — never softened.
//!
//! The pierce decision is the chain-walk POLICY this prompt owns (`notify_for` — critical ALWAYS
//! pierces); the watching-suppression half reuses the frozen `route` (NOTIF-P10) so the drill proves
//! BOTH halves over the SAME `QuietHours`, no second decision path.

use myelin_notif::escalation::notify_for;
use myelin_notif::prefs::{
    route, Channel, NotifPrefs, QuietHours, QuietWindow, RoutingRule, Tz,
};
use myelin_notif::list_inbox::Subsystem;
use myelin_notif::{Class, Reason};
use myelin_query::{Predicate, QueryAst};

/// The always-match matcher (one node — trivially within the static bound).
fn always() -> QueryAst {
    QueryAst::compiled(Predicate::True).expect("True is one node (within the bound)")
}

/// A DND quiet window covering the whole day in UTC (the recipient is in DND at any instant).
fn dnd_quiet() -> QuietHours {
    QuietHours {
        tz: Tz::from_offset_minutes(0),
        windows: vec![QuietWindow { from: 0, to: 1440, days: vec![] }],
        // The DEFAULT pierce set — critical pierces (you cannot silence an on-call page). The drill
        // asserts the default holds; a config that dropped Critical from pierce_classes would FAIL.
        pierce_classes: vec![Class::Critical],
    }
}

/// **NOTIF-D8 — a critical escalation pierces DND; a watching item is suppressed.**
#[test]
fn notif_d8_critical_escalation_pierces_dnd_watching_suppressed() {
    let quiet = dnd_quiet();
    // The recipient is IN a quiet window at this instant (DND covers all day).
    let recipient_in_quiet = quiet.is_quiet_at(/* utc_minute_of_day */ 600, /* weekday */ 2);
    assert!(recipient_in_quiet, "the recipient is in DND (the quiet window covers the instant)");

    // === 1) CRITICAL escalation PIERCES quiet-hours — pages every step channel despite DND ===
    let step_channels = vec![Channel::InApp, Channel::WebPush, Channel::MobilePush];
    let paged = notify_for(&step_channels, Class::Critical, &quiet, recipient_in_quiet);
    assert_eq!(
        paged, step_channels,
        "a CRITICAL escalation pierces DND — pages EVERY channel (you cannot silence an on-call page)"
    );
    // quiet_hours_pierce_count (§1.8): the count of off-cell channels that pierced (here all but in-app).
    let pierce_count = paged.iter().filter(|c| **c != Channel::InApp).count();
    assert!(pierce_count >= 1, "quiet_hours_pierce_count incremented — the page pierced off-cell");

    // === 2) a WATCHING item is SUPPRESSED off-cell by the SAME quiet window (router-side route) ===
    // The recipient routes a `watching` reason to in-app + email (off-cell). Inside DND, route
    // suppresses the off-cell push and keeps ONLY the in-cell in-app (the inbox row is never lost).
    let prefs = NotifPrefs {
        routing: vec![
            RoutingRule { channel: Channel::InApp, matcher: always() },
            RoutingRule { channel: Channel::Email, matcher: always() },
        ],
        digest: Default::default(),
    };
    let delivered = route(
        &prefs,
        &quiet,
        Reason::Watched,
        Class::Watching,
        Subsystem::Issue,
        600, // utc_minute_of_day (inside DND)
        2,   // weekday
    );
    assert_eq!(
        delivered,
        vec![Channel::InApp],
        "a non-critical WATCHING item inside DND is suppressed off-cell — in-app only (§2.2)"
    );
    assert!(
        !delivered.contains(&Channel::Email),
        "the off-cell email push is suppressed (non-crit suppressed)"
    );

    // GREEN ARTIFACT (2026-06-20): critical pierces (pages all channels, pierce_count ≥ 1);
    // non-crit suppressed (watching → in-app only off-cell). Both over the SAME QuietHours. No threshold weakened.
}
