# Agent-fabric floors — designed, not built

The agent fabric ships its trait surface (`myelin-agent`) and a mock runtime behind
it. Three engineering floors and three policy questions are deliberately deferred.
Naming them here is the point: a floor that hides as "done" is the failure mode.

## Engineering floors (the trait seam exists; the body is the follow-on)

**The real vendor brain — `LlmAgentRuntime`.**
The `AgentRuntime` seam is frozen; the stateless brain boundary, the platform-owned
`Conversation` history, and `MockAgentRuntime` behind the same seam all exist. The
follow-on is the real adapter — the *only* place a model / SDK / prompt / model-name
string ever appears (the `no-llm-in-platform` lint enforces this everywhere else). It
is a config/impl swap behind the frozen seam, not a rewrite: EU-hostable, region-aware,
one cost event per model call (wholesale ≠ markup). The EU-sovereign sub-processor
selection is an open legal item; the structural seam ships regardless.
> Built in `myelin-agent-model` (`LlmAgentRuntime` / `LunaClient`) and metered via the
> `MeteredRuntime` override — the walking skeleton is live against Luna.

**The external MCP endpoint.**
`ToolDef` carries an `exposed_over_mcp` column and the internal consumption path is
built. The external surface is a *projection* of `ToolDef` (input_schema → MCP schema,
required_caps → identity-enforced, side_effecting/requires_approval → the same
plan-then-apply + HITL path) — no second governance model. An external MCP client is a
`Principal` with no carve-out, flowing through `EffectApi` like any internal agent. The
follow-on is the endpoint itself: its auth, agent-lane rate-limit, per-external-tenant
budget, and threat model.

**Agent long-term memory / RAG over prior runs.**
The agent-trace holder seam (the content-addressed trace document) exists; v1 agents are
stateless across runs except for that trace. The follow-on is an embedding store —
indexed via Search `semantic`, ACL-filtered during traversal, and purged on `*.erased`
(the structural erasure path already exists, so cross-run recall opens no un-erasable
PII path).

## Policy questions flagged to counsel (the structural floor ships regardless)

**Implicit auto-dispatch on a casual mention.**
v1 is explicit-first: a mention *notifies*, it does not auto-spawn a costed run, and no
auto-spawn path is wired. Implicit auto-wake (with intent/cost detection) is a
separately-decided product feature that needs a human-oversight basis (GDPR Art. 22 /
EU AI-Act) ratified before any such path is built.

**Trace verbosity / reasoning-capture.**
The tool-call / tool-result transcript is captured by default (load-bearing for audit +
replay). Capture of free-form chain-of-thought is gated behind a tenant setting tagged
`#[personal_data]` under the one erasure posture; its retention is a privacy + AI-Act
question for counsel.

**Build-data-as-training — foreclosed by default.**
No platform code path feeds tenant content to model training. Training on tenant data
would be a separately-ratified opt-in, never a default.
