//! # CHAT-D16 — the streaming-UX drill against the `--use-mock` runtime (CHAT-P24 / P-418, M4-C9)
//!
//! **Drill (the GATE):** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` row
//! **CHAT-D16** — drive the streaming UX against the mock runtime → partials stream; the FINAL
//! durable `chat.message.created` REPLACES the partial; a **mid-stream reconnect resumes the FINAL,
//! NEVER a half-message**. The half-message-on-reconnect signal MUST be **0**.
//!
//! **AG-D4 (the permanent sandbox-escape gate, contract 8.4 / X-6 #4):** before chat streams ANY
//! agent-compute output it asserts AG-D4 is GREEN and runs NO compute over a RED gate. This drill
//! consumes the **REAL** [`myelin_ci_sandbox::EscapeAttestation`] artifact (minted ONLY from a green
//! drill — the drill is UPSTREAM, AG-P17 → P-229 / CI-P5 → P-239; chat reads it, never re-runs it) and
//! proves chat's [`myelin_chat::presence::ag_d4_attestation_is_green`] predicate admits the green
//! artifact and refuses a red one — keyed on the SAME green invariant the Fabric's `AgentExecGate`
//! uses (chat does not fork the attestation type; the production crate carries no ci-sandbox edge).
//!
//! **FLOOR named (VISION §3):** the agent runtime is the MOCK (`--use-mock`,
//! scripted-deterministic); the real `LlmAgentRuntime` is the post-M5 swap behind the SAME `step`
//! seam (AG-P25). The streaming UX is proven here WITHOUT a real LLM precisely because the partial
//! stream rides the same firehose path the real runtime will.
//!
//! **Mutation-floor note:** the partial→final / resume core is mandatory-core (the zero-half-message
//! property is a correctness invariant). The `0 half-messages` GATE below is a stronger assertion than
//! a mutation score — a single half-message on ANY reconnect boundary is a RED drill. (The numeric
//! cargo-mutants floor for the chat crate is the crate-wide one; this drill's floor is `0`.)

use myelin_ci_sandbox::escape_corpus::{BEGIN_MARKER, END_MARKER};
use myelin_ci_sandbox::{parse_console, Backend, BackendRun, EscapeAttestation, CORPUS};

use myelin_agent::{AgentRuntime, Conversation, StepOutcome, Submission};
use myelin_chat::presence::{
    ag_d4_attestation_is_green, resume_view, AgD4Attestation, MockStreamRuntime, ResumeView,
    StreamSession,
};

// ─────────────────────── the REAL AG-D4 attestation (the field-view chat asserts over) ─────────────

/// Implement chat's `AgD4Attestation` FIELD-view over the REAL `myelin_ci_sandbox::EscapeAttestation`
/// — proving chat's green predicate keys on the genuine artifact fields, not a chat-local fake. This
/// impl lives in the DRILL (the production chat crate carries no ci-sandbox edge — the predicate is
/// generic over the field view; this binds it to the real type for the drill only).
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

/// A REAL green (or red) AG-D4 attestation, minted from the corpus parser — NEVER hardcoded. `escaped`
/// flips one attack to ESCAPED to model a red drill (which mints NO attestation — the source guard).
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

/// **AG-D4 re-confirmed GREEN before any agent compute (the gate chat asserts).** Chat admits the
/// REAL green attestation and refuses (1) a missing one (fail-closed) and (2) a red one. No compute
/// streams over a red AG-D4.
#[test]
fn ag_d4_re_confirmed_green_before_any_agent_compute() {
    // a REAL green attestation (minted from the green corpus parse) is admitted.
    let green = real_attestation(false).expect("a green drill mints a green attestation");
    assert!(
        ag_d4_attestation_is_green(Some(&RealAtt(&green))),
        "chat must admit the REAL green AG-D4 attestation"
    );

    // fail-closed: with NO attestation, chat runs no compute (the structural default is REFUSE).
    let none: Option<&RealAtt> = None;
    assert!(!ag_d4_attestation_is_green(none));

    // a RED drill mints NO attestation at all (the source guard) — a red AG-D4 is a dated no-go.
    assert!(
        real_attestation(true).is_err(),
        "a red drill must NOT mint an attestation (no green over a red)"
    );
}

// ─────────────────────── the streaming UX against --use-mock (partials → final) ────────────────────

/// **CHAT-D16 happy path: partials STREAM, then the FINAL durable message REPLACES the partial.**
/// Driven against the `--use-mock` runtime: the scripted answer is streamed token-by-token, then the
/// brain SUBMITS the same answer — and the final body is byte-identical to the last partial.
#[test]
fn chat_d16_partials_stream_then_final_replaces_partial() {
    // AG-D4 must be green before we stream any agent compute output.
    let green = real_attestation(false).unwrap();
    assert!(ag_d4_attestation_is_green(Some(&RealAtt(&green))));

    let runtime = MockStreamRuntime::new("run-d16", "the quick brown fox jumps");
    let mut session = StreamSession::open(runtime.correlation_id());

    // stream every partial (the firehose frames) into the session.
    for frame in runtime.partials() {
        assert!(session.apply_partial(&frame), "in-order partial must apply");
    }
    assert!(!session.is_finalized(), "still streaming before the submit");

    // the brain submits — the FINAL durable message replaces the partial.
    let StepOutcome::Submit(Submission(body)) = runtime.step(&Conversation::default()) else {
        panic!("the streaming mock must submit");
    };
    session.finalize("msg-d16", body.clone());
    assert!(session.is_finalized());

    // the final body == the scripted answer (== the last partial cumulative) — replacement is exact.
    match resume_view(&session) {
        ResumeView::Final { final_text, .. } => assert_eq!(final_text, "the quick brown fox jumps"),
        ResumeView::InProgress { .. } => panic!("a submitted run must resume the FINAL"),
    }
}

// ─────────────────────── THE GATE: a mid-stream reconnect resumes the FINAL, never a half-message ──

/// **THE CHAT-D16 GATE — `0 half-messages`.** Inject a reconnect at EVERY token boundary of the
/// scripted stream (and after the submit). At each boundary the resume answer is EITHER the FINAL
/// durable message (if the run had submitted) OR the "working…" marker (the resume cursor) — and is
/// NEVER the live partial body. We count half-messages across all reconnect points and assert 0.
#[test]
fn chat_d16_reconnect_at_every_boundary_never_a_half_message() {
    // AG-D4 green is the precondition for streaming compute output.
    let green = real_attestation(false).unwrap();
    assert!(ag_d4_attestation_is_green(Some(&RealAtt(&green))));

    let runtime = MockStreamRuntime::new("run-gate", "alpha beta gamma delta");
    let partials = runtime.partials();
    let final_body = "alpha beta gamma delta";

    let mut half_messages = 0usize;

    // k = number of partials that have streamed before the reconnect. k == len() ⇒ after the submit.
    for k in 0..=partials.len() {
        let mut session = StreamSession::open(runtime.correlation_id());
        for frame in partials.iter().take(k) {
            assert!(session.apply_partial(frame));
        }
        let submitted = k == partials.len();
        if submitted {
            // the run submits only after all partials.
            let StepOutcome::Submit(Submission(body)) = runtime.step(&Conversation::default())
            else {
                panic!("must submit");
            };
            session.finalize("msg-gate", body);
        }

        // THE RESUME: a reconnecting client gets the final or the marker — NEVER a half-message.
        match resume_view(&session) {
            ResumeView::Final { final_text, .. } => {
                // a Final is correct ONLY after the submit, and is the FULL durable body.
                if !submitted || final_text != final_body {
                    half_messages += 1;
                }
            }
            ResumeView::InProgress { resume_from_seq } => {
                // an InProgress is correct ONLY before the submit; the cursor is the last partial seq.
                // Critically, NO partial BODY is surfaced — the marker carries only the cursor.
                if submitted || resume_from_seq != k as u64 {
                    half_messages += 1;
                }
            }
        }
    }

    // THE GATE: zero half-messages across every reconnect boundary.
    assert_eq!(
        half_messages, 0,
        "CHAT-D16 GATE FAILED: {half_messages} half-message(s) on reconnect (must be 0)"
    );

    // the dated green artifact line (observability is part of the pass, EI-01 §3).
    println!(
        "[CHAT-D16 GREEN] 2026-06-24 streaming-UX(--use-mock) reconnect-boundaries={} \
         half-messages=0 final-replaces-partial=exact AG-D4=green",
        partials.len() + 1
    );
}
