use myelin_notif::escalation::notify_for;
use myelin_notif::list_inbox::Subsystem;
use myelin_notif::prefs::{route, Channel, NotifPrefs, QuietHours, QuietWindow, RoutingRule, Tz};
use myelin_notif::{Class, Reason};
use myelin_query::{Predicate, QueryAst};

fn always() -> QueryAst {
    QueryAst::compiled(Predicate::True).expect("True is one node (within the bound)")
}

fn dnd_quiet() -> QuietHours {
    QuietHours {
        tz: Tz::from_offset_minutes(0),
        windows: vec![QuietWindow {
            from: 0,
            to: 1440,
            days: vec![],
        }],
        pierce_classes: vec![Class::Critical],
    }
}

#[test]
fn notif_d8_critical_escalation_pierces_dnd_watching_suppressed() {
    let quiet = dnd_quiet();
    let recipient_in_quiet =
        quiet.is_quiet_at( 600,  2);
    assert!(
        recipient_in_quiet,
        "the recipient is in DND (the quiet window covers the instant)"
    );

    let step_channels = vec![Channel::InApp, Channel::WebPush, Channel::MobilePush];
    let paged = notify_for(&step_channels, Class::Critical, &quiet, recipient_in_quiet);
    assert_eq!(
        paged, step_channels,
        "a CRITICAL escalation pierces DND - pages EVERY channel (you cannot silence an on-call page)"
    );
    let pierce_count = paged.iter().filter(|c| **c != Channel::InApp).count();
    assert!(
        pierce_count >= 1,
        "quiet_hours_pierce_count incremented - the page pierced off-cell"
    );

    let prefs = NotifPrefs {
        routing: vec![
            RoutingRule {
                channel: Channel::InApp,
                matcher: always(),
            },
            RoutingRule {
                channel: Channel::Email,
                matcher: always(),
            },
        ],
        digest: Default::default(),
    };
    let delivered = route(
        &prefs,
        &quiet,
        Reason::Watched,
        Class::Watching,
        Subsystem::Issue,
        600,
        2,
    );
    assert_eq!(
        delivered,
        vec![Channel::InApp],
        "a non-critical WATCHING item inside DND is suppressed off-cell - in-app only (§2.2)"
    );
    assert!(
        !delivered.contains(&Channel::Email),
        "the off-cell email push is suppressed (non-crit suppressed)"
    );

}
