use myelin_agent::{EventInbox, InboxEvent};
use std::cell::RefCell;

struct RecordingInbox {
    delivered: RefCell<Vec<InboxEvent>>,
}

impl EventInbox for RecordingInbox {
    fn deliver(&self, ev: InboxEvent) {
        self.delivered.borrow_mut().push(ev);
    }
}

#[test]
fn cdc_8_6_deliver_lands_the_matched_event_explicit_first() {
    let inbox = RecordingInbox {
        delivered: RefCell::new(vec![]),
    };

    inbox.deliver(InboxEvent("issue.mention".into()));

    let got = inbox.delivered.borrow();
    assert_eq!(
        got.len(),
        1,
        "explicit-first: deliver notifies exactly once"
    );
    assert_eq!(got[0], InboxEvent("issue.mention".into()));
}
