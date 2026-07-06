# Agent fabric (myelin-agent-service, myelin-agent)

_The agent-fabric unit is in excellent health: the plan-then-apply pipeline, per-run identity lifecycle, sandbox routing split, escape gate, loop guards, cost gate, and GDPR erasure are all fail-closed, invariant-driven, and covered by extensive tests (including mutation-floor and negative-leg tests). Authz/scoping and runaway control are structurally sound. I found one real gap: the batch/partial-approval HITL path weakens the AG-8 "declined effect makes 0 mutation" guarantee for sibling effects that share a tool name, because the step-6 apply gate is coarser (tool-name-keyed) than the per-effect ledger the batch loop claims is the authority. The relational-grant fail-closed in tool_scope is an intentional, documented, safe deferral, not a defect._

**Kept findings:** 1  (🟡 1 medium)

---

### 1. 🟡 Partial-approval batch HITL can bypass the approval gate for a declined sibling effect that shares a tool name with an approved one

- **Severity:** medium  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** correctness
- **Location:** `crates/myelin-agent-service/src/hitl_batch.rs:365`

**What:** run_batch_hitl_loop threads every Approved effect's bare tool name into the run-wide ApprovedTools set via approved.admit(&gate) (hitl_batch.rs:365 → hitl.rs:443 inserts gate.tool_name). The pipeline's HITL gate (effect_api.rs:567) reads only tool-name membership: `def.requires_approval && !self.approved.contains(&plan.tool.0)`. GateIds are per-(tool,object) (effect_api.rs:568), but the approved set is per-tool. So approving one sibling of a batch (e.g. git.merge on PR 40) admits `git.merge` run-wide; a re-drive of a DECLINED sibling (git.merge on PR 41) through apply_planned evaluates step 6 as `true && !true` -> false and proceeds to apply. The per-effect ApplyLedger the module names as the exactly-once authority (hitl_batch.rs:66-78) is never consulted by effect_api.rs's step 6 — the two mechanisms are disconnected, so the AG-8 0-mutation guarantee for a partially-approved batch is documented but not structurally enforced by the gate.

**Impact:** A human who approves some effects in a batch card and DECLINES a sibling on the same tool can have the declined effect applied anyway if the resume re-drives apply per proposed effect through PlanThenApply::apply_planned (the resume path hitl.rs:39 documents). This is a HITL bypass for consequential gated tools (git.merge, knowledge.publish, ci.deploy) whenever a batch mixes approve+decline across effects of the same tool.

**Fix:** Do not populate the coarse tool-name ApprovedTools set from a batch approval. Carry batch approvals at per-effect / (tool,object) granularity and make the step-6 gate key on the same per-effect idem_key the ApplyLedger already uses (or have plan_through_gate consult the ApplyLedger as the authority), so a declined sibling can never satisfy the gate.

> _Verifier note:_ Confirmed in source: hitl_batch.rs:365 calls approved.admit(&gate) into the shared &mut ApprovedTools; hitl.rs:439-445 admit() inserts only gate.tool_name (no object discriminator); effect_api.rs:567 step 6 checks only self.approved.contains(&plan.tool.0); effect_api.rs GateId at :568 is per-(tool,object) but the set is per-tool; grep of effect_api.rs shows apply_planned/plan_through_gate never reference ApplyLedger. The module note at hitl_batch.rs:66-78 admits the coarse set is 'too coarse for a batch' and would let a declined effect through step 6. The realized harm depends on a caller re-driving declined effects through apply_planned rather than strictly from the ledger — that wiring is not present in these files (the intended ledger-driven path is safe), so the exploit is contingent on caller behavior; hence severity medium not high. Category corrected from 'tenancy' (this is HITL/authorization correctness, not multi-tenant isolation).
