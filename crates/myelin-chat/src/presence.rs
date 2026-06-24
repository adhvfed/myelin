//! # `presence` — agent presence classes + streaming partials over the firehose
//! (mock-provable; final replaces partial; reconnect resumes the final) (CHAT-P24 → P-418, M4-C9)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/chat/architecture/02-internals-and-algorithms.md`
//! §7.2 (agent presence is its OWN class tied to agent-fabric HEALTH — `available` /
//! `busy` / `rate-limited` / `offline`, mapped to `chat.presence.*` / consuming
//! `agent.status_changed`; rides the FIREHOSE, never the durable bus, ADR-04.5), §7.3 (streaming
//! partials `agent.message.partial` → firehose; on the agent's Submit the FINAL durable
//! `chat.message.created` REPLACES the partial, reconciled by the run's `correlation_id`/`message_id`;
//! a reconnect mid-stream re-fetches the FINAL / in-progress marker — NEVER a half-message). And
//! `03-events-contracts-and-glue.md` §1.2 (`agent.message.partial` is FIREHOSE-only, never durable;
//! over the frozen resume-cursor protocol, contract 3.5). `04-views-cli-and-api.md` §1 (S5 thread
//! pane = where streaming output lives; S8 member roster = agent presence class).
//!
//! **Contract-index rows:**
//! - **8.3** `AgentRuntime::step --use-mock` — **CONSUMED** (the strategy seam, a real `--use-mock`
//!   flag): the streaming UX is driven against the MOCK runtime; `step` is a pure function of the
//!   conversation, so the scripted-deterministic mock streams partials without an LLM (VISION §3 —
//!   no real agents during development). The presence module never imports an LLM SDK
//!   (`no-llm-in-platform`, contract 1.6).
//! - **3.5** the firehose presence/partials — **CONSUMED**: presence frames + partial frames ride the
//!   Bus-owned firehose `subscribe/resume/scope` tier ([`myelin_events::firehose::FirehoseScope`], the
//!   `*`-rejecting chokepoint). Chat owns NO transport — it hands frames to a PORT
//!   ([`PresencePush`] / [`PartialPush`]) the gateway implements (the SAME port discipline
//!   [`crate::read_state::ReadStatePush`] takes; EI-01 §7 — one transport, the gateway's).
//! - **8.4 / AG-D4** the permanent sandbox-escape gate — **CONSUMED (asserted, not re-run)**: before
//!   streaming any agent-compute output, chat asserts AG-D4 is GREEN
//!   ([`ag_d4_attestation_is_green`]) and runs NO compute over a RED gate. The drill is UPSTREAM
//!   (M2 / AG-P17 → P-229 / CI-P5 → P-239); chat reads its green artifact, it does not re-run it.
//!
//! ## The two firehose streams this module models (arch §7.2 / §7.3)
//!
//! 1. **Agent presence** — [`AgentPresence`] is its own fabric-health-derived class
//!    (`available`/`busy`/`rate-limited`/`offline`), NOT a human idle timer. The transitions are a
//!    deterministic function of the consumed `agent.status_changed` health signal +
//!    a run's start/finish ([`AgentPresence::on_status`] / [`AgentPresence::on_run_start`] /
//!    [`AgentPresence::on_run_finish`]). A `chat.presence.changed` frame is published on the
//!    channel's bounded firehose scope when (and only when) the class actually changes — ephemeral,
//!    allowed-to-drop.
//! 2. **Streaming partials** — an agent run streams [`PartialFrame`]s (`agent.message.partial`) as the
//!    mock runtime produces them, then SUBMITS. On submit the FINAL durable `chat.message.created`
//!    REPLACES the partial ([`StreamSession::finalize`]), reconciled by `correlation_id`. The
//!    in-flight session is the source of the "working…" affordance (S5).
//!
//! ## The mid-stream-reconnect resume (the CHAT-D16 zero-half-message property)
//!
//! A reconnect mid-stream MUST resume the FINAL, never a half-message (arch §7.3, EI-01 §3 prove-it).
//! [`resume_view`] is the resume answer chat hands a reconnecting client: it consults the durable
//! session state + the partial resume cursor and returns EITHER the finalized
//! [`ResumeView::Final`] (if the run submitted before/at the reconnect) OR an
//! [`ResumeView::InProgress`] marker (the "working…" affordance) — but **NEVER** a partially-streamed
//! body. The partial body is live-only; if lost, the FINAL message is the truth (§1.3 / §7.3). The
//! [`crate::presence::tests`] drive this against `--use-mock` with the reconnect injected at every
//! token boundary and assert 0 half-messages (the gate).
//!
//! ## FLOOR named (VISION §3 — name-your-floors)
//!
//! - **The agent runtime is the MOCK** (`--use-mock`, scripted-deterministic — [`MockStreamRuntime`]
//!   over the frozen [`myelin_agent::AgentRuntime`] seam, 8.3). The real `LlmAgentRuntime` is the
//!   **post-M5 follow-on** (a config/impl swap behind the SAME `step` seam, never a rewrite — after
//!   AG-D4/D2/D3/D5 green; AG-P25, VISION §3). The streaming UX is proven HERE without a real LLM
//!   precisely because the partial stream rides the same path the real runtime will.
//! - **The firehose TRANSPORT is the gateway's** (the [`PresencePush`]/[`PartialPush`] ports): the
//!   wired publish/subscribe/resume lives in the chat gateway (CHAT-P9/P10); this module owns the
//!   presence/partial LOGIC + the frame shapes + the resume answer, not the socket.

use std::collections::BTreeMap;

use myelin_events::firehose::FirehoseScope;

use crate::events::{delivery_class, DeliveryClass, CHAT_PRESENCE_CHANGED};

// ─────────────────────────────── the firehose-only tokens this module rides ────────────────────────

/// The Agent-Fabric-owned streaming-partial token (arch §1.2 / §7.3). It carries the **`agent`**
/// subsystem prefix (an Agent-Fabric-owned token — chat does NOT register foreign-subsystem tokens,
/// the acyclic-producer invariant; see [`crate::events`]'s note). Chat PARTICIPATES in this firehose
/// frame (it is the producer of the partials when it drives the mock runtime); it does not own the
/// token. Named here as the literal the partial frames carry, never re-registered under `chat.*`.
pub const AGENT_MESSAGE_PARTIAL: &str = "agent.message.partial";

/// The Agent-Fabric-owned status token chat CONSUMES to derive [`AgentPresence`] (arch §7.2 / §1.3 —
/// "consume `agent.status_changed`"). A foreign-subsystem token chat reacts to, never re-registers.
pub const AGENT_STATUS_CHANGED: &str = "agent.status_changed";

// ───────────────────────────────────── §7.2 agent presence class ───────────────────────────────────

/// **Agent presence — its OWN class, tied to agent-fabric HEALTH (arch §7.2).** An agent is not a
/// human with an idle timer: presence is derived from runtime health + budget + the
/// protected-human-lane shed verdict + whether a run is in flight. Shown by **glyph + label +
/// position, never colour alone** (design-language §3.2/§4 — the renderer's obligation, surfaced via
/// [`AgentPresence::glyph`]/[`AgentPresence::label`]); **no sparkle/magic-wand** iconography ("agents
/// look like agents, not magic").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentPresence {
    /// Runtime healthy + within budget/quota, no run in flight. The agent can be dispatched.
    Available,
    /// In a run / streaming (a partial stream is in flight). The "working…" state.
    Busy,
    /// Shed by the protected-human-lane / per-tenant caps (OQ-K) — the fabric is up but this agent is
    /// throttled. Distinct from `offline`: the runtime is healthy, the agent is rate-limited.
    RateLimited,
    /// The runtime is unavailable (fabric health down) — no dispatch is possible.
    Offline,
}

impl AgentPresence {
    /// The stable presence-class key (the value carried in the `chat.presence.changed` frame; the
    /// names anchor X-5 — the roster renderer consumes these by name, never a literal/colour).
    pub fn key(self) -> &'static str {
        match self {
            AgentPresence::Available => "available",
            AgentPresence::Busy => "busy",
            AgentPresence::RateLimited => "rate-limited",
            AgentPresence::Offline => "offline",
        }
    }

    /// A NON-colour glyph for the class (design-language §3.2/§4 — status is glyph + label + position,
    /// never colour alone; no sparkle/magic-wand). The renderer pairs this with [`Self::label`].
    pub fn glyph(self) -> &'static str {
        match self {
            // a filled dot = present/idle; a spinner = working; a paused bar = throttled; a hollow
            // dot = absent. All shape-distinct so the class reads without colour (accessibility).
            AgentPresence::Available => "●",
            AgentPresence::Busy => "◐",
            AgentPresence::RateLimited => "⏸",
            AgentPresence::Offline => "○",
        }
    }

    /// The human label for the class (paired with [`Self::glyph`]; never colour-only).
    pub fn label(self) -> &'static str {
        match self {
            AgentPresence::Available => "Available",
            AgentPresence::Busy => "Working…",
            AgentPresence::RateLimited => "Rate-limited",
            AgentPresence::Offline => "Offline",
        }
    }

    /// Whether the agent may be DISPATCHED in this class. Only [`AgentPresence::Available`] admits a
    /// new run: `busy` is mid-run, `rate-limited` is shed (OQ-K), `offline` has no runtime. The
    /// explicit-first dispatch path (CHAT-P25) reads this; surfaced here as the presence semantics.
    pub fn dispatchable(self) -> bool {
        matches!(self, AgentPresence::Available)
    }
}

/// The agent-fabric HEALTH signal chat consumes from `agent.status_changed` (arch §7.2 / §1.3). A
/// thin, references-not-payloads view of the fabric verdict — chat does not own fabric health, it
/// derives presence from this signal. `Healthy` + a free quota ⇒ `available`; `Shed` ⇒ `rate-limited`;
/// `Down` ⇒ `offline`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FabricHealth {
    /// The runtime is healthy and within budget/quota.
    Healthy,
    /// The runtime is healthy but this agent is shed by the protected-human-lane / per-tenant caps
    /// (OQ-K) — throttled, not down.
    Shed,
    /// The runtime is unavailable (fabric health down).
    Down,
}

impl FabricHealth {
    /// The presence class this health verdict maps to **when no run is in flight** (arch §7.2). A run
    /// in flight overrides `Healthy → Busy` (see [`AgentPresence::on_run_start`]); a `Shed`/`Down`
    /// verdict is NOT overridden by a run (a shed/down agent is rate-limited/offline regardless).
    pub fn idle_presence(self) -> AgentPresence {
        match self {
            FabricHealth::Healthy => AgentPresence::Available,
            FabricHealth::Shed => AgentPresence::RateLimited,
            FabricHealth::Down => AgentPresence::Offline,
        }
    }
}

impl AgentPresence {
    /// **Apply a consumed `agent.status_changed` health verdict (arch §7.2).** Returns the new class.
    /// `Healthy` while a run is in flight stays `Busy`; `Shed`/`Down` take precedence over a run (a
    /// shed agent is rate-limited even mid-stream — the fabric throttled it). This is the
    /// presence-class transition function the unit tests pin.
    pub fn on_status(self, health: FabricHealth) -> AgentPresence {
        match health {
            // a shed/down verdict overrides everything, including an in-flight run.
            FabricHealth::Shed => AgentPresence::RateLimited,
            FabricHealth::Down => AgentPresence::Offline,
            // healthy: stay Busy if mid-run, else Available.
            FabricHealth::Healthy => {
                if self == AgentPresence::Busy {
                    AgentPresence::Busy
                } else {
                    AgentPresence::Available
                }
            }
        }
    }

    /// **A run started (a partial stream is opening) — the agent goes `Busy` (arch §7.2/§7.3).** Only
    /// a healthy/available agent transitions to `Busy`; a `rate-limited`/`offline` agent does NOT
    /// start a run (the dispatch gate would have refused — [`AgentPresence::dispatchable`]), so its
    /// class is unchanged.
    pub fn on_run_start(self) -> AgentPresence {
        match self {
            AgentPresence::Available => AgentPresence::Busy,
            // a shed/offline agent cannot start a run; an already-busy agent stays busy.
            other => other,
        }
    }

    /// **A run finished (the partial stream submitted/closed) — the agent returns to its idle class
    /// (arch §7.2/§7.3).** `Busy` returns to `Available` IFF the fabric is still healthy; if the
    /// fabric shed/went-down mid-run, the class reflects that (it is NOT reset to `available`). The
    /// `still_healthy` verdict is the latest consumed `agent.status_changed`.
    pub fn on_run_finish(self, still_healthy: FabricHealth) -> AgentPresence {
        if self == AgentPresence::Busy {
            still_healthy.idle_presence()
        } else {
            self
        }
    }
}

// ───────────────────────────────── the presence firehose PORT (3.5) ────────────────────────────────

/// **The presence-frame push seam — the port the gateway's live-delivery surface implements
/// (contract 3.5 / arch §7.2).** A presence-class change publishes a `chat.presence.changed` frame on
/// the channel's bounded firehose scope (`channel:<id>` — never `*`). FIREHOSE-only (ephemeral,
/// allowed-to-drop, ADR-04.5): if lost, the next presence frame / the roster re-fetch is the truth.
///
/// **Why a port, not a `firehose.publish` here (EI-01 §7 — one transport).** The firehose transport
/// is the gateway's (it owns the ONE `firehose.publish` call site); the presence service owns NO
/// transport handle. It hands the frame to this port; the gateway publishes it on the bounded scope.
pub trait PresencePush {
    /// Publish a `chat.presence.changed` frame on the bounded channel `scope` (contract 3.5). Returns
    /// the assigned firehose frame seq (the resume cursor a reconnecting roster backfills from).
    /// Allowed-to-drop (firehose semantics).
    fn push_presence(&self, scope: &FirehoseScope, agent: &str, class: AgentPresence) -> u64;
}

// ─────────────────────────────────── §7.3 streaming partials ───────────────────────────────────────

/// A single streaming-partial frame (`agent.message.partial`, arch §7.3). Carries the run
/// `correlation_id` (the reconciliation key — the FINAL `chat.message.created` REPLACES the partial
/// on this id), a monotonic `seq` within the run (the partial resume cursor), and the cumulative
/// rendered text SO FAR (the "working…" body the thread pane shows). FIREHOSE-only,
/// allowed-to-drop — if lost, the FINAL message is the truth.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartialFrame {
    /// The run correlation id — the reconciliation key the FINAL message replaces the partial on.
    pub correlation_id: String,
    /// The monotonic within-run partial sequence (`1`-based; the partial resume cursor).
    pub seq: u64,
    /// The CUMULATIVE rendered text so far (a partial render, NOT the final). Live-only.
    pub cumulative_text: String,
    /// Whether this is the LAST partial before the final submission (the stream-close marker). The
    /// final durable message follows; the partial is then replaced.
    pub is_last: bool,
}

/// **The partial-frame push seam — the port the gateway implements (contract 3.5 / arch §7.3).** A
/// run streams partials on the THREAD's firehose scope (the thread pane hosts streaming, S5). Same
/// port discipline as [`PresencePush`]: the service owns no transport; the gateway publishes.
pub trait PartialPush {
    /// Publish an `agent.message.partial` frame on the bounded thread `scope` (contract 3.5). Returns
    /// the assigned firehose seq (the partial resume cursor). Allowed-to-drop.
    fn push_partial(&self, scope: &FirehoseScope, frame: &PartialFrame) -> u64;
}

/// The lifecycle of one streaming agent run (arch §7.3). The session is the source of the "working…"
/// affordance (S5) and the reconciliation state the resume answer ([`resume_view`]) reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamState {
    /// Partials are streaming; the run has NOT submitted. The thread shows "working…". The
    /// `last_seq` is the highest partial seq published (the resume cursor); `cumulative_text` is the
    /// live partial body (NEVER returned to a reconnecting client — it is a half-message).
    Streaming {
        last_seq: u64,
        cumulative_text: String,
    },
    /// The run SUBMITTED — the FINAL durable `chat.message.created` replaced the partial. Carries the
    /// final message id (the durable truth) + the final body. A reconnect resumes THIS.
    Finalized {
        message_id: String,
        final_text: String,
    },
}

/// **One streaming agent-run session — the partial→final state machine (arch §7.3).** Built when a
/// run starts; fed partials as the mock runtime produces them; FINALIZED on the agent's Submit (the
/// FINAL durable `chat.message.created` replaces the partial). Keyed by `correlation_id` (the
/// reconciliation id).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamSession {
    /// The run correlation id (the reconciliation key).
    pub correlation_id: String,
    /// The current state (streaming partials, or finalized).
    pub state: StreamState,
}

impl StreamSession {
    /// Open a fresh streaming session for a run (no partials yet; `last_seq = 0` is the
    /// "before any frame" sentinel — the first partial is `seq = 1`).
    pub fn open(correlation_id: impl Into<String>) -> StreamSession {
        StreamSession {
            correlation_id: correlation_id.into(),
            state: StreamState::Streaming {
                last_seq: 0,
                cumulative_text: String::new(),
            },
        }
    }

    /// **Apply a streamed partial frame (arch §7.3).** Advances the partial resume cursor + the live
    /// cumulative body. A partial received on an ALREADY-finalized session is IGNORED (the final is
    /// the truth — a late partial never un-finalizes). The `seq` MUST be monotonic-by-one
    /// (`last_seq + 1`); an out-of-order partial is rejected (returns `false`) so the live cursor
    /// cannot rewind (the firehose seq invariant, contract 3.5).
    pub fn apply_partial(&mut self, frame: &PartialFrame) -> bool {
        match &mut self.state {
            StreamState::Streaming {
                last_seq,
                cumulative_text,
            } => {
                if frame.seq != *last_seq + 1 {
                    return false; // out-of-order / replayed partial — the cursor must not rewind.
                }
                *last_seq = frame.seq;
                cumulative_text.clone_from(&frame.cumulative_text);
                true
            }
            // a late partial after finalize is dropped — the final message is the truth.
            StreamState::Finalized { .. } => false,
        }
    }

    /// **Finalize the run — the FINAL durable `chat.message.created` REPLACES the partial (arch
    /// §7.3).** Idempotent: finalizing an already-finalized session is a no-op that keeps the first
    /// final (the durable message id is immutable). After this, the session resumes to the FINAL.
    pub fn finalize(&mut self, message_id: impl Into<String>, final_text: impl Into<String>) {
        if let StreamState::Streaming { .. } = self.state {
            self.state = StreamState::Finalized {
                message_id: message_id.into(),
                final_text: final_text.into(),
            };
        }
    }

    /// Whether the run has submitted (the partial was replaced by the final durable message).
    pub fn is_finalized(&self) -> bool {
        matches!(self.state, StreamState::Finalized { .. })
    }
}

/// **The reconnect resume answer (arch §7.3 — the CHAT-D16 zero-half-message property).** What chat
/// hands a client reconnecting mid-stream. It is NEVER a half-message: either the FINAL durable body
/// (if the run submitted) or an in-progress "working…" marker (the affordance), never the live
/// partial body. See [`resume_view`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeView {
    /// The run SUBMITTED — resume the FINAL durable message (the truth). Carries the message id + the
    /// final body. This is the ONLY body a resume ever returns.
    Final {
        message_id: String,
        final_text: String,
    },
    /// The run is still in flight — resume the "working…" affordance (NOT the partial body). The
    /// client re-subscribes to the partial firehose from this cursor; the body it shows is the
    /// affordance, not a half-message.
    InProgress { resume_from_seq: u64 },
}

/// **The reconnect resume answer (arch §7.3 / §1.3 / EI-01 §3 — prove-it).** Given the durable session
/// state of a run, return the [`ResumeView`] a reconnecting client gets. The INVARIANT this function
/// encodes (and [`tests`] drill): a reconnect resumes the FINAL, **never a half-message**. If the run
/// finalized at/before the reconnect, return [`ResumeView::Final`] (the durable message). If it is
/// still streaming, return [`ResumeView::InProgress`] with the resume cursor — the client gets the
/// "working…" affordance and re-subscribes, it does NOT receive the live partial body (which is a
/// half-message). The live `cumulative_text` is deliberately NOT surfaced here — that is the
/// structural guarantee.
pub fn resume_view(session: &StreamSession) -> ResumeView {
    match &session.state {
        StreamState::Finalized {
            message_id,
            final_text,
        } => ResumeView::Final {
            message_id: message_id.clone(),
            final_text: final_text.clone(),
        },
        StreamState::Streaming { last_seq, .. } => ResumeView::InProgress {
            // resume from the last published partial seq — the client backfills `(last_seq, now]`
            // off the partial firehose (contract 3.5). The half-message body is NEVER returned.
            resume_from_seq: *last_seq,
        },
    }
}

/// A registry of in-flight + recently-finalized streaming sessions keyed by `correlation_id` — the
/// state the gateway consults on a reconnect to build the [`resume_view`]. In-process here; the
/// durable session record lives in the message store (the FINAL `chat.message.created` IS the durable
/// truth — a dropped session map re-derives the resume answer from the durable message, §1.3).
#[derive(Clone, Debug, Default)]
pub struct StreamSessions {
    by_correlation: BTreeMap<String, StreamSession>,
}

impl StreamSessions {
    /// A fresh registry.
    pub fn new() -> StreamSessions {
        StreamSessions {
            by_correlation: BTreeMap::new(),
        }
    }

    /// Open (or return the existing) session for a run.
    pub fn open(&mut self, correlation_id: impl Into<String>) -> &mut StreamSession {
        let id = correlation_id.into();
        self.by_correlation
            .entry(id.clone())
            .or_insert_with(|| StreamSession::open(id))
    }

    /// Look up a session (for the resume answer).
    pub fn get(&self, correlation_id: &str) -> Option<&StreamSession> {
        self.by_correlation.get(correlation_id)
    }

    /// The reconnect resume answer for a run, or `None` if the run is unknown to this registry (the
    /// gateway then falls back to the durable message store — §1.3). Never a half-message.
    pub fn resume(&self, correlation_id: &str) -> Option<ResumeView> {
        self.by_correlation.get(correlation_id).map(resume_view)
    }
}

// ─────────────────────── §8.3 the MOCK streaming runtime (--use-mock, the FLOOR) ───────────────────

/// **The scripted-deterministic MOCK streaming runtime (`--use-mock`, contract 8.3 — the FLOOR).**
/// Over the frozen [`myelin_agent::AgentRuntime`] seam: it tokenizes its scripted answer and emits one
/// [`PartialFrame`] per token (cumulative), then the runtime's `step` SUBMITS the same answer — so the
/// FINAL `chat.message.created` body is byte-identical to the last partial's cumulative text (the
/// "final replaces partial" reconciliation is exact). NO LLM SDK appears (`no-llm-in-platform`,
/// contract 1.6) — the streaming UX is proven without a real LLM (VISION §3).
///
/// **FLOOR:** this is the mock; the real `LlmAgentRuntime` is the post-M5 swap behind the SAME `step`
/// seam (AG-P25). The partials ride the same firehose path the real runtime will.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MockStreamRuntime {
    /// The scripted answer the mock streams token-by-token then submits.
    answer: String,
    /// The run correlation id the partials + the final reconcile on.
    correlation_id: String,
}

impl MockStreamRuntime {
    /// A mock runtime scripted to stream `answer` (whitespace-tokenized) then submit it.
    pub fn new(correlation_id: impl Into<String>, answer: impl Into<String>) -> MockStreamRuntime {
        MockStreamRuntime {
            answer: answer.into(),
            correlation_id: correlation_id.into(),
        }
    }

    /// The scripted final answer (what `step` will `Submit`).
    pub fn answer(&self) -> &str {
        &self.answer
    }

    /// **Produce the scripted partial stream (arch §7.3, `--use-mock`).** One cumulative
    /// [`PartialFrame`] per whitespace token; the LAST frame is `is_last = true` and its
    /// `cumulative_text` equals the full scripted answer (so the final durable body == the last
    /// partial — the reconciliation is exact). An empty answer streams a single empty `is_last` frame.
    pub fn partials(&self) -> Vec<PartialFrame> {
        let tokens: Vec<&str> = self.answer.split_whitespace().collect();
        if tokens.is_empty() {
            return vec![PartialFrame {
                correlation_id: self.correlation_id.clone(),
                seq: 1,
                cumulative_text: String::new(),
                is_last: true,
            }];
        }
        let mut frames = Vec::with_capacity(tokens.len());
        let mut cumulative = String::new();
        let last = tokens.len() - 1;
        for (i, tok) in tokens.iter().enumerate() {
            if !cumulative.is_empty() {
                cumulative.push(' ');
            }
            cumulative.push_str(tok);
            frames.push(PartialFrame {
                correlation_id: self.correlation_id.clone(),
                seq: (i as u64) + 1,
                cumulative_text: cumulative.clone(),
                is_last: i == last,
            });
        }
        frames
    }

    /// The run correlation id.
    pub fn correlation_id(&self) -> &str {
        &self.correlation_id
    }
}

impl myelin_agent::AgentRuntime for MockStreamRuntime {
    /// The brain SUBMITS the scripted answer (the FINAL). `step` is a pure function of the
    /// conversation (8.3); the partial stream is produced by [`Self::partials`] on the SAME scripted
    /// answer, so the final submission body matches the last partial exactly. NO LLM SDK.
    fn step(&self, _conv: &myelin_agent::Conversation) -> myelin_agent::StepOutcome {
        myelin_agent::StepOutcome::Submit(myelin_agent::Submission(self.answer.clone()))
    }
}

/// **Drive a full streamed run against a `--use-mock` runtime and finalize it (arch §7.3, the
/// happy-path stream).** Streams every scripted partial through `session` (publishing on the partial
/// firehose port if `push`/`scope` are supplied), then SUBMITS via the runtime's `step` and FINALIZES
/// the session — the FINAL body is the submission (byte-identical to the last partial). Returns the
/// finalized session. This is the streaming UX driven against the mock (CHAT-D16); the gateway wires
/// the real socket, this drives the logic.
pub fn run_streamed(
    runtime: &MockStreamRuntime,
    message_id: impl Into<String>,
    mut push: Option<(&dyn PartialPush, &FirehoseScope)>,
) -> StreamSession {
    use myelin_agent::AgentRuntime;

    let mut session = StreamSession::open(runtime.correlation_id());
    for frame in runtime.partials() {
        // publish on the firehose port if wired (allowed-to-drop); the in-process session is the
        // reconciliation state.
        if let Some((port, scope)) = push.as_mut() {
            port.push_partial(scope, &frame);
        }
        // applying a well-ordered partial advances the live cursor (the gateway's "working…").
        let _ = session.apply_partial(&frame);
    }
    // the agent SUBMITS — the FINAL durable `chat.message.created` replaces the partial.
    let outcome = runtime.step(&myelin_agent::Conversation::default());
    let final_text = match outcome {
        myelin_agent::StepOutcome::Submit(myelin_agent::Submission(s)) => s,
        // a mock that streamed partials always submits; a UseTools here is a script error.
        myelin_agent::StepOutcome::UseTools(_) => runtime.answer().to_string(),
    };
    session.finalize(message_id, final_text);
    session
}

// ─────────────────────────── 8.4 / AG-D4 — the sandbox-gate assertion (consumed) ──────────────────

/// **The AG-D4 / CI-T1 green-escape attestation FIELDS chat asserts over (contract 8.4, the permanent
/// sandbox gate, X-6 #4).** Chat does NOT depend on `myelin-ci-sandbox` in production — it asserts
/// over this minimal FIELD view so a real [`myelin_ci_sandbox::EscapeAttestation`](https://docs)
/// (proven in the CDC) and the gateway's loaded artifact both satisfy it. The frozen field names match
/// the attestation byte-for-byte (the CDC pins parity); chat reads the artifact, it never re-runs the
/// drill (the drill is upstream — AG-P17 → P-229 / CI-P5 → P-239).
pub trait AgD4Attestation {
    /// The artifact kind tag — MUST be `ag-d4-green-escape-attestation`.
    fn artifact_tag(&self) -> &str;
    /// The drill id — MUST be `AG-D4 / CI-T1`.
    fn drill_id(&self) -> &str;
    /// The total escapes observed — MUST be `0` for green (one escape is catastrophic).
    fn total_escapes(&self) -> u32;
}

/// **The AG-D4 assertion chat runs BEFORE streaming any agent-compute output (arch §7 / contract
/// 8.4).** Green IFF the attestation is the green-escape ARTIFACT (`ag-d4-green-escape-attestation`),
/// the AG-D4 / CI-T1 DRILL, AND reports ZERO escapes. `None` (no attestation loaded) is FAIL-CLOSED —
/// the structural default with no green artifact is REFUSE (no green attestation ⇒ no untrusted
/// compute). This is the SAME predicate the Fabric's `AgentExecGate::admit` keys on (chat does not
/// fork it — it asserts the same green invariant before dispatching the streaming run). Chat runs NO
/// agent compute over a `false` here.
pub fn ag_d4_attestation_is_green<A: AgD4Attestation>(attestation: Option<&A>) -> bool {
    match attestation {
        // fail-closed: no attestation ⇒ no untrusted compute (the structural default is REFUSE).
        None => false,
        Some(att) => {
            att.artifact_tag() == "ag-d4-green-escape-attestation"
                && att.drill_id() == "AG-D4 / CI-T1"
                && att.total_escapes() == 0
        }
    }
}

// ─────────────────────────── the firehose-classification self-check (arch §1.2) ───────────────────

/// **Both presence + partials are FIREHOSE-only (arch §1.2 / §7.2 / §7.3).** A callable invariant the
/// drill asserts: the chat presence token classifies [`DeliveryClass::Firehose`] (it must NEVER ride
/// the durable bus — ADR-04.5), and the agent partial token is the Agent-owned firehose frame chat
/// participates in (NOT a `chat.*` durable token). Returns `true` iff the classification holds.
pub fn presence_and_partials_are_firehose_only() -> bool {
    // the chat presence token is firehose-only (the durable bus would be a contract violation).
    let presence_firehose = delivery_class(CHAT_PRESENCE_CHANGED) == Some(DeliveryClass::Firehose);
    // the agent partial token is foreign-owned (agent.*) and is NOT a registered chat durable token.
    let partial_is_agent_owned = AGENT_MESSAGE_PARTIAL.starts_with("agent.")
        && delivery_class(AGENT_MESSAGE_PARTIAL).is_none();
    presence_firehose && partial_is_agent_owned
}

#[cfg(test)]
mod tests {
    use super::*;

    // ───────────────────────── presence-class transitions (the unit suite) ─────────────────────────

    #[test]
    fn health_maps_to_the_idle_presence_class() {
        assert_eq!(
            FabricHealth::Healthy.idle_presence(),
            AgentPresence::Available
        );
        assert_eq!(
            FabricHealth::Shed.idle_presence(),
            AgentPresence::RateLimited
        );
        assert_eq!(FabricHealth::Down.idle_presence(), AgentPresence::Offline);
    }

    #[test]
    fn run_start_takes_an_available_agent_to_busy() {
        assert_eq!(AgentPresence::Available.on_run_start(), AgentPresence::Busy);
        // a non-available agent cannot start a run — its class is unchanged.
        assert_eq!(
            AgentPresence::RateLimited.on_run_start(),
            AgentPresence::RateLimited
        );
        assert_eq!(
            AgentPresence::Offline.on_run_start(),
            AgentPresence::Offline
        );
        assert_eq!(AgentPresence::Busy.on_run_start(), AgentPresence::Busy);
    }

    #[test]
    fn run_finish_returns_busy_to_the_current_idle_class() {
        // healthy fabric: Busy → Available.
        assert_eq!(
            AgentPresence::Busy.on_run_finish(FabricHealth::Healthy),
            AgentPresence::Available
        );
        // shed mid-run: Busy → RateLimited (NOT reset to available).
        assert_eq!(
            AgentPresence::Busy.on_run_finish(FabricHealth::Shed),
            AgentPresence::RateLimited
        );
        // down mid-run: Busy → Offline.
        assert_eq!(
            AgentPresence::Busy.on_run_finish(FabricHealth::Down),
            AgentPresence::Offline
        );
    }

    #[test]
    fn a_shed_verdict_overrides_an_in_flight_run() {
        // a Busy agent shed by the protected-human-lane goes RateLimited even mid-stream (OQ-K).
        assert_eq!(
            AgentPresence::Busy.on_status(FabricHealth::Shed),
            AgentPresence::RateLimited
        );
        // a Busy agent whose fabric goes down goes Offline.
        assert_eq!(
            AgentPresence::Busy.on_status(FabricHealth::Down),
            AgentPresence::Offline
        );
        // a Busy agent on a still-healthy fabric stays Busy (the run continues).
        assert_eq!(
            AgentPresence::Busy.on_status(FabricHealth::Healthy),
            AgentPresence::Busy
        );
        // an idle agent on a healthy fabric is Available.
        assert_eq!(
            AgentPresence::Offline.on_status(FabricHealth::Healthy),
            AgentPresence::Available
        );
    }

    #[test]
    fn only_available_is_dispatchable() {
        assert!(AgentPresence::Available.dispatchable());
        assert!(!AgentPresence::Busy.dispatchable());
        assert!(!AgentPresence::RateLimited.dispatchable());
        assert!(!AgentPresence::Offline.dispatchable());
    }

    #[test]
    fn presence_is_glyph_plus_label_never_colour_only() {
        // every class has a SHAPE-distinct glyph + a label (status reads without colour —
        // design-language §3.2/§4). No two classes share a glyph.
        let classes = [
            AgentPresence::Available,
            AgentPresence::Busy,
            AgentPresence::RateLimited,
            AgentPresence::Offline,
        ];
        let mut glyphs = std::collections::BTreeSet::new();
        for c in classes {
            assert!(!c.label().is_empty());
            assert!(
                glyphs.insert(c.glyph()),
                "glyph for {:?} is not shape-distinct",
                c
            );
        }
        assert_eq!(glyphs.len(), classes.len());
    }

    // ───────────────────────── the partial→final replacement (the core) ───────────────────────────

    #[test]
    fn partials_advance_the_cursor_monotonically() {
        let mut s = StreamSession::open("run-1");
        for seq in 1..=3 {
            let f = PartialFrame {
                correlation_id: "run-1".into(),
                seq,
                cumulative_text: format!("tok{seq}"),
                is_last: seq == 3,
            };
            assert!(s.apply_partial(&f), "in-order partial seq={seq} must apply");
        }
        match &s.state {
            StreamState::Streaming {
                last_seq,
                cumulative_text,
            } => {
                assert_eq!(*last_seq, 3);
                assert_eq!(cumulative_text, "tok3");
            }
            _ => panic!("must still be streaming"),
        }
    }

    #[test]
    fn an_out_of_order_partial_is_rejected_the_cursor_never_rewinds() {
        let mut s = StreamSession::open("run-1");
        let f1 = PartialFrame {
            correlation_id: "run-1".into(),
            seq: 1,
            cumulative_text: "a".into(),
            is_last: false,
        };
        assert!(s.apply_partial(&f1));
        // a replayed seq=1 (or a skipped seq=3) is rejected — the cursor must not rewind/jump.
        let replay = PartialFrame {
            correlation_id: "run-1".into(),
            seq: 1,
            cumulative_text: "a".into(),
            is_last: false,
        };
        assert!(!s.apply_partial(&replay));
        let skip = PartialFrame {
            correlation_id: "run-1".into(),
            seq: 3,
            cumulative_text: "abc".into(),
            is_last: false,
        };
        assert!(!s.apply_partial(&skip));
    }

    #[test]
    fn finalize_replaces_the_partial_with_the_final_durable_message() {
        let mut s = StreamSession::open("run-1");
        let f = PartialFrame {
            correlation_id: "run-1".into(),
            seq: 1,
            cumulative_text: "hel".into(),
            is_last: true,
        };
        assert!(s.apply_partial(&f));
        assert!(!s.is_finalized());
        s.finalize("msg-99", "hello world");
        assert!(s.is_finalized());
        // the final REPLACES the partial — the durable body is the final, not the partial.
        match &s.state {
            StreamState::Finalized {
                message_id,
                final_text,
            } => {
                assert_eq!(message_id, "msg-99");
                assert_eq!(final_text, "hello world");
            }
            _ => panic!("must be finalized"),
        }
    }

    #[test]
    fn a_late_partial_after_finalize_is_dropped_the_final_is_the_truth() {
        let mut s = StreamSession::open("run-1");
        s.finalize("msg-1", "final");
        let late = PartialFrame {
            correlation_id: "run-1".into(),
            seq: 1,
            cumulative_text: "half".into(),
            is_last: false,
        };
        assert!(
            !s.apply_partial(&late),
            "a late partial must NOT un-finalize"
        );
        assert!(s.is_finalized());
    }

    #[test]
    fn finalize_is_idempotent_the_durable_id_is_immutable() {
        let mut s = StreamSession::open("run-1");
        s.finalize("msg-1", "first");
        s.finalize("msg-2", "second"); // a second finalize is a no-op (the durable id is immutable).
        match &s.state {
            StreamState::Finalized {
                message_id,
                final_text,
            } => {
                assert_eq!(message_id, "msg-1");
                assert_eq!(final_text, "first");
            }
            _ => panic!("must be finalized"),
        }
    }

    // ───────────────────────── the mid-stream-reconnect resume (THE GATE: 0 half-messages) ─────────

    #[test]
    fn resume_mid_stream_returns_the_working_marker_never_a_half_message() {
        let mut s = StreamSession::open("run-1");
        let f = PartialFrame {
            correlation_id: "run-1".into(),
            seq: 2,
            cumulative_text: "half a".into(),
            is_last: false,
        };
        // (seq=2 won't apply on a fresh session; stream seq=1 then seq=2)
        let f1 = PartialFrame {
            correlation_id: "run-1".into(),
            seq: 1,
            cumulative_text: "half".into(),
            is_last: false,
        };
        assert!(s.apply_partial(&f1));
        assert!(s.apply_partial(&f));
        // a reconnect mid-stream resumes the "working…" affordance + the resume cursor — NEVER the
        // live half-message body.
        let view = resume_view(&s);
        match view {
            ResumeView::InProgress { resume_from_seq } => assert_eq!(resume_from_seq, 2),
            ResumeView::Final { .. } => panic!("must NOT return a final mid-stream"),
        }
    }

    #[test]
    fn resume_after_finalize_returns_the_final_durable_message() {
        let mut s = StreamSession::open("run-1");
        let f = PartialFrame {
            correlation_id: "run-1".into(),
            seq: 1,
            cumulative_text: "hel".into(),
            is_last: true,
        };
        assert!(s.apply_partial(&f));
        s.finalize("msg-7", "hello");
        match resume_view(&s) {
            ResumeView::Final {
                message_id,
                final_text,
            } => {
                assert_eq!(message_id, "msg-7");
                assert_eq!(final_text, "hello");
            }
            ResumeView::InProgress { .. } => panic!("a finalized run must resume the FINAL"),
        }
    }

    /// **THE CHAT-D16 GATE (0 half-messages): a reconnect injected at EVERY token boundary resumes
    /// either the FINAL (if submitted) or the working-marker — NEVER a half-message body.** Driven
    /// against the `--use-mock` runtime.
    #[test]
    fn reconnect_at_every_token_boundary_never_yields_a_half_message() {
        let runtime = MockStreamRuntime::new("run-42", "the quick brown fox");
        let partials = runtime.partials();
        // at each prefix length k (0..=n), simulate a reconnect AFTER k partials have streamed.
        for k in 0..=partials.len() {
            let mut s = StreamSession::open(runtime.correlation_id());
            for frame in partials.iter().take(k) {
                assert!(s.apply_partial(frame));
            }
            // the run finalizes ONLY after all partials (and the submit); before that it is streaming.
            let final_submitted = k == partials.len();
            if final_submitted {
                let outcome = {
                    use myelin_agent::AgentRuntime;
                    runtime.step(&myelin_agent::Conversation::default())
                };
                if let myelin_agent::StepOutcome::Submit(myelin_agent::Submission(body)) = outcome {
                    s.finalize("msg-42", body);
                }
            }
            // INVARIANT: resume is NEVER the live partial body. It is the final, or the marker.
            match resume_view(&s) {
                ResumeView::Final { final_text, .. } => {
                    // a final is only returned when the run actually submitted.
                    assert!(
                        final_submitted,
                        "a Final at k={k} but the run had not submitted"
                    );
                    assert_eq!(final_text, "the quick brown fox");
                }
                ResumeView::InProgress { resume_from_seq } => {
                    assert!(
                        !final_submitted,
                        "an InProgress at k={k} but the run HAD submitted"
                    );
                    assert_eq!(resume_from_seq, k as u64);
                }
            }
        }
    }

    // ───────────────────────── the mock streaming runtime (8.3 --use-mock) ─────────────────────────

    #[test]
    fn the_mock_streams_cumulative_partials_then_submits_the_same_answer() {
        let runtime = MockStreamRuntime::new("run-1", "hello brave world");
        let partials = runtime.partials();
        assert_eq!(partials.len(), 3);
        assert_eq!(partials[0].cumulative_text, "hello");
        assert_eq!(partials[1].cumulative_text, "hello brave");
        assert_eq!(partials[2].cumulative_text, "hello brave world");
        assert!(partials[2].is_last);
        // seqs are 1-based and monotonic.
        assert_eq!(
            partials.iter().map(|f| f.seq).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        // the FINAL submission body equals the LAST partial cumulative — final replaces partial exactly.
        use myelin_agent::AgentRuntime;
        let outcome = runtime.step(&myelin_agent::Conversation::default());
        match outcome {
            myelin_agent::StepOutcome::Submit(myelin_agent::Submission(s)) => {
                assert_eq!(s, partials[2].cumulative_text);
            }
            _ => panic!("the mock must submit"),
        }
    }

    #[test]
    fn run_streamed_drives_partials_then_finalizes_to_the_submission() {
        let runtime = MockStreamRuntime::new("run-9", "alpha beta");
        let session = run_streamed(&runtime, "msg-9", None);
        assert!(session.is_finalized());
        match &session.state {
            StreamState::Finalized {
                message_id,
                final_text,
            } => {
                assert_eq!(message_id, "msg-9");
                assert_eq!(final_text, "alpha beta");
            }
            _ => panic!("run_streamed must finalize"),
        }
    }

    #[test]
    fn run_streamed_publishes_each_partial_on_the_firehose_port() {
        #[derive(Default)]
        struct CapturePush {
            frames: std::cell::RefCell<Vec<PartialFrame>>,
        }
        impl PartialPush for CapturePush {
            fn push_partial(&self, _scope: &FirehoseScope, frame: &PartialFrame) -> u64 {
                self.frames.borrow_mut().push(frame.clone());
                self.frames.borrow().len() as u64
            }
        }
        let runtime = MockStreamRuntime::new("run-3", "a b c");
        let push = CapturePush::default();
        let scope = FirehoseScope::parse("channel:c-1").expect("bounded scope");
        let session = run_streamed(&runtime, "msg-3", Some((&push, &scope)));
        assert!(session.is_finalized());
        // every partial was published on the bounded firehose scope, in order.
        let frames = push.frames.borrow();
        assert_eq!(frames.len(), 3);
        assert_eq!(
            frames.iter().map(|f| f.seq).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    // ───────────────────────── the firehose classification (arch §1.2) ─────────────────────────────

    #[test]
    fn presence_and_partials_ride_the_firehose_only() {
        assert!(presence_and_partials_are_firehose_only());
        // spelled out: the chat presence token is firehose, NEVER durable.
        assert_eq!(
            delivery_class(CHAT_PRESENCE_CHANGED),
            Some(DeliveryClass::Firehose)
        );
        // the agent partial is a foreign (agent.*) firehose frame chat participates in — NOT a chat token.
        assert!(AGENT_MESSAGE_PARTIAL.starts_with("agent."));
        assert!(delivery_class(AGENT_MESSAGE_PARTIAL).is_none());
        assert!(AGENT_STATUS_CHANGED.starts_with("agent."));
    }

    // ───────────────────────── the StreamSessions registry ─────────────────────────────────────────

    #[test]
    fn sessions_registry_opens_and_resumes_by_correlation() {
        let mut sessions = StreamSessions::new();
        {
            let s = sessions.open("run-1");
            let f = PartialFrame {
                correlation_id: "run-1".into(),
                seq: 1,
                cumulative_text: "x".into(),
                is_last: true,
            };
            s.apply_partial(&f);
            s.finalize("msg-1", "done");
        }
        match sessions.resume("run-1") {
            Some(ResumeView::Final { message_id, .. }) => assert_eq!(message_id, "msg-1"),
            other => panic!("expected a final resume, got {other:?}"),
        }
        // an unknown run is None — the gateway falls back to the durable store.
        assert!(sessions.resume("run-unknown").is_none());
    }

    // ───────────────────────── AG-D4 — chat asserts green, refuses red ─────────────────────────────

    /// A field-view stand-in proving the predicate logic (the REAL `EscapeAttestation` is exercised
    /// in the cross-crate CDC/drill — `tests/drill_chat_d16_streaming.rs`).
    struct FakeAtt {
        artifact: String,
        drill: String,
        escapes: u32,
    }
    impl AgD4Attestation for FakeAtt {
        fn artifact_tag(&self) -> &str {
            &self.artifact
        }
        fn drill_id(&self) -> &str {
            &self.drill
        }
        fn total_escapes(&self) -> u32 {
            self.escapes
        }
    }

    #[test]
    fn ag_d4_assertion_is_fail_closed_without_an_attestation() {
        // no attestation ⇒ no untrusted compute (the structural default is REFUSE).
        assert!(!ag_d4_attestation_is_green::<FakeAtt>(None));
    }

    #[test]
    fn ag_d4_assertion_admits_a_green_attestation_refuses_a_red_one() {
        let green = FakeAtt {
            artifact: "ag-d4-green-escape-attestation".into(),
            drill: "AG-D4 / CI-T1".into(),
            escapes: 0,
        };
        assert!(ag_d4_attestation_is_green(Some(&green)));
        // a RED attestation (any escape) is REFUSED — one escape is catastrophic.
        let red = FakeAtt {
            artifact: "ag-d4-green-escape-attestation".into(),
            drill: "AG-D4 / CI-T1".into(),
            escapes: 1,
        };
        assert!(!ag_d4_attestation_is_green(Some(&red)));
        // a wrong-artifact / wrong-drill never admits.
        let wrong = FakeAtt {
            artifact: "some-other-artifact".into(),
            drill: "AG-D4 / CI-T1".into(),
            escapes: 0,
        };
        assert!(!ag_d4_attestation_is_green(Some(&wrong)));
    }
}
