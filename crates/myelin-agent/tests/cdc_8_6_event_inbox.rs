//! # The CDC pair for contract 8.6 — `EventInbox::deliver(InboxEvent)`
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 8.6
//! (`EventInbox::deliver(InboxEvent)` — platform delivers matched events (envelope + binding +
//! token + budget); agents don't poll. **Explicit-first dispatch** (CHAT-1): a mention notifies,
//! does not auto-spawn a costed run; implicit auto-dispatch is L-3, counsel-gated). Owning
//! architecture: `agent-fabric.md` §1.3/§3.4. AG-P1 / P-130 ships the SIGNATURE half; the
//! dispatch-tier wiring is the Bus's (§3.6), the agent-side consumer lands in AG-P4 (→ P-216).
//!
//! ## What this pair pins (the signature half of 8.6)
//! - the **PROVIDER** is the platform delivery seam (the Bus → Agent): `deliver` accepts a matched
//!   `InboxEvent`; agents don't poll. Explicit-first: deliver NOTIFIES, it does not itself spawn a
//!   costed run.
//! - the **CONSUMER** is the agent fabric: it receives the delivered event and records it (the
//!   SKELETON consumer body is AG-P4 → P-216). Here a recording inbox proves the event lands.

use myelin_agent::{EventInbox, InboxEvent};
use std::cell::RefCell;

/// **PROVIDER + CONSUMER side of 8.6.** A recording inbox: the platform PROVIDER calls `deliver`;
/// the agent-fabric CONSUMER records the delivered event (explicit-first — recording a notify is
/// NOT spawning a costed run). The dispatch-tier wiring is the Bus's; this pins the delivery shape.
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

    // PROVIDER (platform delivery): deliver a matched event. CONSUMER (agent): it is recorded.
    inbox.deliver(InboxEvent("issue.mention".into()));

    let got = inbox.delivered.borrow();
    assert_eq!(
        got.len(),
        1,
        "explicit-first: deliver notifies exactly once"
    );
    assert_eq!(got[0], InboxEvent("issue.mention".into()));
}
