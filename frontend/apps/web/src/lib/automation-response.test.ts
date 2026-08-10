import { describe, expect, it } from "vitest";
import {
  parseAutomation,
  parseAutomationErasure,
  parseAutomationFiringPage,
  parseAutomationLifecycle,
  parseAutomationPage,
  parseAutomationResult,
} from "./automation-response";

const automationId = "44444444-4444-4444-8444-444444444444";
const runId = "55555555-5555-4555-8555-555555555555";
const tenant = "acme";

const automation = {
  id: automationId,
  ref: `myelin://${tenant}/identity/trigger/${automationId}`,
  owner_principal_id: "founder",
  run_as_agent_id: "22222222-2222-4222-8222-222222222222",
  event_type: "ci.run.failed",
  subject_type: "run",
  condition: "payload.source_ref == 'refs/heads/main'",
  matcher: { object_type: "run", predicate: { raw: "payload.source_ref == 'refs/heads/main'" } },
  task: "Read the failed run and open one triage issue.",
  delegation_caveats: ["repo=alpha"],
  budget_minor_units: 250_000,
  max_firings: 10,
  firings_used: 1,
  max_causal_depth: 4,
  require_no_personal_data: true,
  require_human_approval: true,
  state: "active",
  created_at: "2026-08-10T12:00:00Z",
};

const firing = {
  event_id: "ci-failed-01JABC",
  event_type: "ci.run.failed",
  trigger_ref: automation.ref,
  state: "terminal",
  run_id: runId,
  run_ref: `myelin://${tenant}/agent/run/${runId}`,
  outcome: "succeeded",
  result_state: "available",
  approval: {
    decision: "approved",
    decided_by: "founder",
    decided_at: "2026-08-10T12:01:00Z",
  },
  created_at: "2026-08-10T12:00:30Z",
};

const result = {
  run_id: runId,
  run_ref: `myelin://${tenant}/agent/run/${runId}`,
  trace_ref: `myelin://${tenant}/knowledge/doc/blake3:${"a".repeat(64)}`,
  agent_principal: "agent:22222222-2222-4222-8222-222222222222",
  answer: "The contract stage failed.\nOpened ENG-41 for the owning team.",
  charged_micro: 42_000,
  recorded_at: "2026-08-10T12:02:00Z",
};

describe("automation response decoding", () => {
  it("accepts the complete owner-scoped workspace story", () => {
    expect(parseAutomationPage({ items: [automation], page: { next_cursor: null, limit: 25 } }))
      .toEqual({ items: [automation], page: { next_cursor: null, limit: 25 } });
    expect(parseAutomation({ trigger: automation })).toEqual(automation);
    expect(parseAutomationFiringPage({ items: [firing], page: { next_cursor: null, limit: 25 } }))
      .toEqual({ items: [firing], page: { next_cursor: null, limit: 25 } });
    expect(parseAutomationResult({ result })).toEqual(result);
    expect(parseAutomationLifecycle({
      action: "pause",
      changed: true,
      canceled_firings: 0,
      durable: true,
      trigger: { ...automation, state: "paused" },
    })).toMatchObject({ action: "pause", changed: true, trigger: { state: "paused" } });
  });

  it("accepts an idempotent, recreation-blocking erasure receipt", () => {
    const erasure = {
      run_id: runId,
      run_ref: result.run_ref,
      trace_ref: result.trace_ref,
      erased: true,
      already_erased: true,
      available_results: 0,
      recreation_blocked: true,
    };
    expect(parseAutomationErasure({ erasure })).toEqual(erasure);
  });

  it("fails closed on surplus fields and incoherent run references", () => {
    expect(parseAutomationPage({
      items: [{ ...automation, integration_api_key: "must-never-cross" }],
      page: { next_cursor: null, limit: 25 },
    })).toBeNull();
    expect(parseAutomationFiringPage({
      items: [{ ...firing, run_ref: `myelin://${tenant}/agent/run/another-run` }],
      page: { next_cursor: null, limit: 25 },
    })).toBeNull();
    expect(parseAutomationResult({ result: { ...result, charged_micro: -1 } })).toBeNull();
    expect(parseAutomationFiringPage({
      items: [{ ...firing, run_id: null, run_ref: null, result_state: "available" }],
      page: { next_cursor: null, limit: 25 },
    })).toBeNull();
  });

  it("rejects malformed pagination and partial approvals", () => {
    expect(parseAutomationPage({ items: [automation], page: { next_cursor: null, limit: 0 } }))
      .toBeNull();
    expect(parseAutomationFiringPage({
      items: [{ ...firing, approval: { decision: "approved", decided_by: "founder" } }],
      page: { next_cursor: null, limit: 25 },
    })).toBeNull();
  });
});
