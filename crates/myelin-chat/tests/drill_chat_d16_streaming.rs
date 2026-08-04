use myelin_ci_sandbox::escape_corpus::{BEGIN_MARKER, END_MARKER};
use myelin_ci_sandbox::{parse_console, Backend, BackendRun, EscapeAttestation, CORPUS};

use myelin_agent::{AgentRuntime, Conversation, StepOutcome, Submission};
use myelin_chat::presence::{
    ag_d4_attestation_is_green, resume_view, AgD4Attestation, MockStreamRuntime, ResumeView,
    StreamSession,
};

struct RealAtt<'a>(&'a EscapeAttestation);
impl AgD4Attestation for RealAtt<'_> {
    fn artifact_tag(&self) -> &str {
        &self.0.artifact
    }
    fn drill_id(&self) -> &str {
        &self.0.drill
    }
    fn total_escapes(&self) -> u32 {
        self.0.total_escapes
    }
}

fn real_attestation(escaped: bool) -> Result<EscapeAttestation, String> {
    let mut console = format!("{BEGIN_MARKER} corpus_version=1 kernel=6.1.168 guest_euid=0\n");
    for atk in CORPUS {
        console.push_str(&format!("{} CONTAINED\n", atk.id));
    }
    if escaped {
        console = console.replace("K1_module CONTAINED", "K1_module ESCAPED");
    }
    console.push_str(&format!("{END_MARKER}\n"));
    let report = parse_console(&console);
    EscapeAttestation::from_green_drill(
        "2026-06-24",
        &report,
        vec![
            BackendRun {
                backend: Backend::FirecrackerMicrovm,
                exercised: true,
                residual_note: None,
            },
            BackendRun {
                backend: Backend::GvisorRunsc,
                exercised: false,
                residual_note: Some("runsc residual (CI-P28)".into()),
            },
        ],
        Backend::FirecrackerMicrovm,
        "7a2bc8ed2c64ed78994971439b00c234b1ce46d247123314d683df7579c77923",
        "467367e6b8e88323dd23dedae3119ade9c9fca6a102a84fc2155e3ef1bec00eb",
        "6.1.168",
    )
}

#[test]
fn ag_d4_re_confirmed_green_before_any_agent_compute() {
    let green = real_attestation(false).expect("a green drill mints a green attestation");
    assert!(
        ag_d4_attestation_is_green(Some(&RealAtt(&green))),
        "chat must admit the REAL green AG-D4 attestation"
    );

    let none: Option<&RealAtt> = None;
    assert!(!ag_d4_attestation_is_green(none));

    assert!(
        real_attestation(true).is_err(),
        "a red drill must NOT mint an attestation (no green over a red)"
    );
}

#[test]
fn chat_d16_partials_stream_then_final_replaces_partial() {
    let green = real_attestation(false).unwrap();
    assert!(ag_d4_attestation_is_green(Some(&RealAtt(&green))));

    let runtime = MockStreamRuntime::new("run-d16", "the quick brown fox jumps");
    let mut session = StreamSession::open(runtime.correlation_id());

    for frame in runtime.partials() {
        assert!(session.apply_partial(&frame), "in-order partial must apply");
    }
    assert!(!session.is_finalized(), "still streaming before the submit");

    let StepOutcome::Submit(Submission(body)) = runtime.step(&Conversation::default()) else {
        panic!("the streaming mock must submit");
    };
    session.finalize("msg-d16", body.clone());
    assert!(session.is_finalized());

    match resume_view(&session) {
        ResumeView::Final { final_text, .. } => assert_eq!(final_text, "the quick brown fox jumps"),
        ResumeView::InProgress { .. } => panic!("a submitted run must resume the FINAL"),
    }
}

#[test]
fn chat_d16_reconnect_at_every_boundary_never_a_half_message() {
    let green = real_attestation(false).unwrap();
    assert!(ag_d4_attestation_is_green(Some(&RealAtt(&green))));

    let runtime = MockStreamRuntime::new("run-gate", "alpha beta gamma delta");
    let partials = runtime.partials();
    let final_body = "alpha beta gamma delta";

    let mut half_messages = 0usize;

    for k in 0..=partials.len() {
        let mut session = StreamSession::open(runtime.correlation_id());
        for frame in partials.iter().take(k) {
            assert!(session.apply_partial(frame));
        }
        let submitted = k == partials.len();
        if submitted {
            let StepOutcome::Submit(Submission(body)) = runtime.step(&Conversation::default())
            else {
                panic!("must submit");
            };
            session.finalize("msg-gate", body);
        }

        match resume_view(&session) {
            ResumeView::Final { final_text, .. } => {
                if !submitted || final_text != final_body {
                    half_messages += 1;
                }
            }
            ResumeView::InProgress { resume_from_seq } => {
                if submitted || resume_from_seq != k as u64 {
                    half_messages += 1;
                }
            }
        }
    }

    assert_eq!(
        half_messages, 0,
        "CHAT-D16 GATE FAILED: {half_messages} half-message(s) on reconnect (must be 0)"
    );

    println!(
        "[CHAT-D16 GREEN] 2026-06-24 streaming-UX(--use-mock) reconnect-boundaries={} \
         half-messages=0 final-replaces-partial=exact AG-D4=green",
        partials.len() + 1
    );
}
