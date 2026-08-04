use myelin_agent::{
    Agent, AgentRuntime, Conversation, InboxEvent, RunOutcome, StepOutcome, Submission,
};

struct SubmitRuntime;
impl AgentRuntime for SubmitRuntime {
    fn step(&self, _conv: &Conversation) -> StepOutcome {
        StepOutcome::Submit(Submission("done".into()))
    }
}

struct ProviderLoop;
impl Agent for ProviderLoop {
    fn handle(&self, _inbox: InboxEvent, runtime: &dyn AgentRuntime) -> RunOutcome {
        match runtime.step(&Conversation::default()) {
            StepOutcome::Submit(Submission(s)) => RunOutcome(format!("submitted:{s}")),
            StepOutcome::UseTools(_) => RunOutcome("used-tools".into()),
        }
    }
}

#[test]
fn cdc_8_5_handle_drives_the_brain_through_the_dyn_seam() {
    let loop_provider = ProviderLoop;
    let runtime = SubmitRuntime;
    let out = loop_provider.handle(InboxEvent("mention".into()), &runtime);
    assert_eq!(out, RunOutcome("submitted:done".into()));
}
