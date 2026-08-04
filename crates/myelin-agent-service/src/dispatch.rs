#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchTrigger {
    Mention(String),
    ExplicitRun(String),
    StructuredRef(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchDecision {
    Notify(String),
    Dispatch(String),
}

impl DispatchDecision {
    pub fn dispatches(&self) -> bool {
        matches!(self, DispatchDecision::Dispatch(_))
    }

    pub fn notifies(&self) -> bool {
        matches!(self, DispatchDecision::Notify(_))
    }
}

pub fn classify(trigger: &DispatchTrigger) -> DispatchDecision {
    match trigger {
        DispatchTrigger::Mention(r) => DispatchDecision::Notify(r.clone()),
        DispatchTrigger::ExplicitRun(r) | DispatchTrigger::StructuredRef(r) => {
            DispatchDecision::Dispatch(r.clone())
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DispatchCounter {
    auto_spawns: u64,
    notifications: u64,
}

impl DispatchCounter {
    pub fn new() -> DispatchCounter {
        DispatchCounter::default()
    }

    pub fn route(&mut self, decision: DispatchDecision) -> DispatchDecision {
        match &decision {
            DispatchDecision::Notify(_) => self.notifications += 1,
            DispatchDecision::Dispatch(_) => self.auto_spawns += 1,
        }
        decision
    }

    pub fn dispatch(&mut self, trigger: &DispatchTrigger) -> DispatchDecision {
        let decision = classify(trigger);
        self.route(decision)
    }

    pub fn auto_spawns(&self) -> u64 {
        self.auto_spawns
    }

    pub fn notifications(&self) -> u64 {
        self.notifications
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_casual_mention_notifies_never_dispatches() {
        let d = classify(&DispatchTrigger::Mention("myelin://acme/chat/msg/1".into()));
        assert!(d.notifies(), "a casual mention NOTIFIES");
        assert!(
            !d.dispatches(),
            "a casual mention does NOT auto-spawn a costed run (explicit-first)"
        );
        assert_eq!(
            d,
            DispatchDecision::Notify("myelin://acme/chat/msg/1".into())
        );
    }

    #[test]
    fn an_explicit_trigger_dispatches() {
        let d = classify(&DispatchTrigger::ExplicitRun(
            "myelin://acme/agent/run-req/9".into(),
        ));
        assert!(
            d.dispatches(),
            "an explicit trigger DISPATCHES a costed run"
        );
        assert!(!d.notifies());
    }

    #[test]
    fn a_structured_ref_re_trigger_dispatches() {
        let d = classify(&DispatchTrigger::StructuredRef(
            "myelin://acme/issues/issue/PROJ-1".into(),
        ));
        assert!(
            d.dispatches(),
            "a structured artifact-ref re-trigger dispatches"
        );
    }

    #[test]
    fn chat_d17_casual_mentions_zero_auto_spawn() {
        let mut counter = DispatchCounter::new();
        for i in 0..10 {
            let decision = counter.dispatch(&DispatchTrigger::Mention(format!(
                "myelin://acme/chat/msg/{i}"
            )));
            assert!(decision.notifies(), "each casual mention NOTIFIES");
        }
        assert_eq!(
            counter.auto_spawns(),
            0,
            "CHAT-D17: 0 auto-spawn on casual mentions (the dispatch-counter stays 0)"
        );
        assert_eq!(
            counter.notifications(),
            10,
            "all ten mentions were delivered as notifications (0 cost)"
        );
    }

    #[test]
    fn chat_d17_explicit_run_is_the_only_spawn() {
        let mut counter = DispatchCounter::new();
        counter.dispatch(&DispatchTrigger::Mention("msg/1".into()));
        counter.dispatch(&DispatchTrigger::Mention("msg/2".into()));
        let explicit = counter.dispatch(&DispatchTrigger::ExplicitRun("run-req/1".into()));
        assert!(
            explicit.dispatches(),
            "the explicit run dispatches (and passes reserve downstream)"
        );
        assert_eq!(
            counter.auto_spawns(),
            1,
            "exactly ONE costed run was dispatched - the explicit one (the two mentions spawned 0)"
        );
        assert_eq!(
            counter.notifications(),
            2,
            "the two casual mentions notified"
        );
    }

    #[test]
    fn no_mention_can_ever_dispatch_the_l3_floor() {
        for r in ["a", "b", "c", "@agent please ship it"] {
            let d = classify(&DispatchTrigger::Mention(r.into()));
            assert!(
                !d.dispatches(),
                "a mention can NEVER dispatch (the L-3 auto-dispatch floor is structural): {r}"
            );
        }
    }
}
