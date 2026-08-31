import { action, json, query, redirect } from "@solidjs/router";
import { edgeGet, edgePost, GatewayError, isUnauthorized } from "../server/gateway";
import {
  AUTOMATION_PAGE_LIMIT,
  isAutomationId,
  parseAutomationApproval,
  parseAutomation,
  parseAutomationCreate,
  parseAutomationErasure,
  parseAutomationFiringPage,
  parseAutomationLifecycle,
  parseAutomationPage,
  parseAutomationResult,
  type AutomationErasureVM,
  type AutomationApprovalReceiptVM,
  type AutomationCreateReceipt,
  type AutomationFiringPage,
  type AutomationLifecycleVM,
  type AutomationPage,
  type AutomationResultVM,
  type AutomationVM,
} from "./automation-response";
import {
  AutomationRouteError,
  type AutomationErrorKind,
} from "./automation-error";

export { AutomationRouteError, type AutomationErrorKind } from "./automation-error";

export interface AutomationPageInput {
  cursor?: string;
  limit?: number;
}

export type AutomationLifecycleAction = "pause" | "resume" | "disable";
export type AutomationApprovalAction = "approve" | "reject";
export type AutomationMutationResult<T> =
  | { ok: true; receipt: T }
  | { ok: false; error: AutomationErrorKind };

function segment(value: string): string {
  return encodeURIComponent(value);
}

function validEventCursor(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 &&
    new TextEncoder().encode(value).byteLength <= 255 && !/[\p{Cc}]/u.test(value);
}

function parsePageInput(value: AutomationPageInput, cursorKind: "automation" | "event"):
  Required<Pick<AutomationPageInput, "limit">> & Pick<AutomationPageInput, "cursor"> | null {
  if (!value || typeof value !== "object") return null;
  const keys = Object.keys(value);
  if (keys.some((key) => key !== "cursor" && key !== "limit")) return null;
  const limit = value.limit ?? AUTOMATION_PAGE_LIMIT;
  if (!Number.isSafeInteger(limit) || limit < 1 || limit > 100) return null;
  if (value.cursor !== undefined &&
      !(cursorKind === "automation" ? isAutomationId(value.cursor) : validEventCursor(value.cursor))) {
    return null;
  }
  return value.cursor === undefined ? { limit } : { limit, cursor: value.cursor };
}

function pageQuery(input: { limit: number; cursor?: string }): string {
  const query = new URLSearchParams({ limit: String(input.limit) });
  if (input.cursor) query.set("cursor", input.cursor);
  return query.toString();
}

async function authed<T>(fetcher: () => Promise<T>): Promise<T> {
  try {
    return await fetcher();
  } catch (error) {
    if (isUnauthorized(error)) throw redirect("/login");
    if (error instanceof GatewayError) {
      if (error.status === 400) throw new AutomationRouteError("bad-input");
      if (error.status === 403 || error.status === 404) throw new AutomationRouteError("not-found");
      if (error.status === 503) throw new AutomationRouteError("unavailable");
    }
    if (error instanceof AutomationRouteError) throw error;
    throw new AutomationRouteError("error");
  }
}

export const getAutomations = query(async (
  request: AutomationPageInput = {},
): Promise<AutomationPage> => {
  "use server";
  const input = parsePageInput(request, "automation");
  if (!input) throw new AutomationRouteError("bad-input");
  return authed(async () => {
    const page = parseAutomationPage(await edgeGet(`/v1/triggers?${pageQuery(input)}`));
    if (!page) throw new AutomationRouteError("error");
    return page;
  });
}, "automations");

export const getAutomation = query(async (id: string): Promise<AutomationVM> => {
  "use server";
  if (!isAutomationId(id)) throw new AutomationRouteError("bad-input");
  return authed(async () => {
    const automation = parseAutomation(await edgeGet(`/v1/triggers/${segment(id)}`));
    if (!automation) throw new AutomationRouteError("error");
    return automation;
  });
}, "automation-detail");

export const getAutomationFirings = query(async (
  id: string,
  request: AutomationPageInput = {},
): Promise<AutomationFiringPage> => {
  "use server";
  if (!isAutomationId(id)) throw new AutomationRouteError("bad-input");
  const input = parsePageInput(request, "event");
  if (!input) throw new AutomationRouteError("bad-input");
  return authed(async () => {
    const page = parseAutomationFiringPage(await edgeGet(
      `/v1/triggers/${segment(id)}/firings?${pageQuery(input)}`,
    ));
    if (!page) throw new AutomationRouteError("error");
    return page;
  });
}, "automation-firings");

export const getAutomationResult = query(async (
  automationId: string,
  runId: string,
): Promise<AutomationResultVM> => {
  "use server";
  if (!isAutomationId(automationId) || !isAutomationId(runId)) {
    throw new AutomationRouteError("bad-input");
  }
  return authed(async () => {
    const result = parseAutomationResult(await edgeGet(
      `/v1/triggers/${segment(automationId)}/runs/${segment(runId)}/result`,
    ));
    if (!result) throw new AutomationRouteError("error");
    return result;
  });
}, "automation-result");

const utf8 = new TextEncoder();
const EVENT_TYPE = /^[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)+$/;

export const createAutomation = action(async (input: unknown) => {
  "use server";
  const respond = (value: AutomationMutationResult<AutomationCreateReceipt>) =>
    json(value, { revalidate: [] });
  if (!input || typeof input !== "object" || Array.isArray(input)) {
    return respond({ ok: false, error: "bad-input" });
  }
  const value = input as Record<string, unknown>;
  if (!Object.keys(value).every((key) => [
    "eventType", "agentId", "task", "budgetMinorUnits", "maxFirings",
    "requireHumanApproval", "clientNonce",
  ].includes(key)) || Object.keys(value).length !== 7 ||
      typeof value.eventType !== "string" || !EVENT_TYPE.test(value.eventType) ||
      utf8.encode(value.eventType).byteLength > 255 || !isAutomationId(value.agentId) ||
      typeof value.task !== "string" || value.task.trim() !== value.task ||
      utf8.encode(value.task).byteLength > 4 * 1_024 || value.task.length === 0 ||
      [...value.task].some((character) => {
        const point = character.codePointAt(0)!;
        return point === 0 || point === 0x7f || (point < 0x20 && character !== "\n" && character !== "\t");
      }) || !Number.isSafeInteger(value.budgetMinorUnits) ||
      (value.budgetMinorUnits as number) < 1 ||
      (value.budgetMinorUnits as number) > 1_000_000_000_000 ||
      !Number.isSafeInteger(value.maxFirings) || (value.maxFirings as number) < 1 ||
      (value.maxFirings as number) > 1_000_000 ||
      typeof value.requireHumanApproval !== "boolean" ||
      typeof value.clientNonce !== "string" ||
      !/^[A-Za-z0-9_-]{1,128}$/.test(value.clientNonce)) {
    return respond({ ok: false, error: "bad-input" });
  }
  try {
    const receipt = parseAutomationCreate(await edgePost("/v1/triggers", {
      event_type: value.eventType,
      run_as_agent_id: value.agentId,
      task: value.task,
      budget_minor_units: value.budgetMinorUnits,
      max_firings: value.maxFirings,
      delegation_caveats: [],
      require_no_personal_data: true,
      require_human_approval: value.requireHumanApproval,
    }, { idempotencyKey: value.clientNonce }));
    return receipt ? respond({ ok: true, receipt }) : respond({ ok: false, error: "error" });
  } catch (error) {
    if (isUnauthorized(error)) throw redirect("/login");
    if (error instanceof GatewayError) {
      if (error.status === 400) return respond({ ok: false, error: "bad-input" });
      if (error.status === 403 || error.status === 404) {
        return respond({ ok: false, error: "not-found" });
      }
      if (error.status === 409) return respond({ ok: false, error: "conflict" });
      if (error.status === 503) return respond({ ok: false, error: "unavailable" });
    }
    return respond({ ok: false, error: "error" });
  }
}, "automation-create");

export const changeAutomationLifecycle = action(async (input: {
  automationId: string;
  action: AutomationLifecycleAction;
}) => {
  "use server";
  const respond = (value: AutomationMutationResult<AutomationLifecycleVM>) =>
    json(value, { revalidate: [] });
  if (!input || !isAutomationId(input.automationId) ||
      !["pause", "resume", "disable"].includes(input.action)) {
    return respond({ ok: false, error: "bad-input" });
  }
  try {
    const receipt = parseAutomationLifecycle(await edgePost(
      `/v1/triggers/${segment(input.automationId)}/${input.action}`,
      {},
    ));
    return receipt
      ? respond({ ok: true, receipt })
      : respond({ ok: false, error: "error" });
  } catch (error) {
    if (isUnauthorized(error)) throw redirect("/login");
    if (error instanceof GatewayError) {
      if (error.status === 400) return respond({ ok: false, error: "bad-input" });
      if (error.status === 403 || error.status === 404) {
        return respond({ ok: false, error: "not-found" });
      }
    }
    return respond({ ok: false, error: error instanceof GatewayError && error.status === 503
      ? "unavailable"
      : "error" });
  }
}, "automation-lifecycle");

export const decideAutomationFiring = action(async (input: {
  automationId: string;
  eventId: string;
  action: AutomationApprovalAction;
}) => {
  "use server";
  const respond = (value: AutomationMutationResult<AutomationApprovalReceiptVM>) =>
    json(value, { revalidate: [] });
  if (!input || !isAutomationId(input.automationId) || !validEventCursor(input.eventId) ||
      !["approve", "reject"].includes(input.action)) {
    return respond({ ok: false, error: "bad-input" });
  }
  try {
    const receipt = parseAutomationApproval(await edgePost(
      `/v1/triggers/${segment(input.automationId)}/firings/${input.action}`,
      { event_id: input.eventId },
    ));
    return receipt
      ? respond({ ok: true, receipt })
      : respond({ ok: false, error: "error" });
  } catch (error) {
    if (isUnauthorized(error)) throw redirect("/login");
    if (error instanceof GatewayError) {
      if (error.status === 400) return respond({ ok: false, error: "bad-input" });
      if (error.status === 403 || error.status === 404) {
        return respond({ ok: false, error: "not-found" });
      }
      if (error.status === 409) return respond({ ok: false, error: "conflict" });
      if (error.status === 503) return respond({ ok: false, error: "unavailable" });
    }
    return respond({ ok: false, error: "error" });
  }
}, "automation-firing-decision");

export const eraseAutomationResult = action(async (input: {
  automationId: string;
  runId: string;
}) => {
  "use server";
  const respond = (value: AutomationMutationResult<AutomationErasureVM>) =>
    json(value, { revalidate: [] });
  if (!input || !isAutomationId(input.automationId) || !isAutomationId(input.runId)) {
    return respond({ ok: false, error: "bad-input" });
  }
  try {
    const receipt = parseAutomationErasure(await edgePost(
      `/v1/triggers/${segment(input.automationId)}/runs/${segment(input.runId)}/result/erase`,
      {},
    ));
    return receipt
      ? respond({ ok: true, receipt })
      : respond({ ok: false, error: "error" });
  } catch (error) {
    if (isUnauthorized(error)) throw redirect("/login");
    if (error instanceof GatewayError && (error.status === 403 || error.status === 404)) {
      return respond({ ok: false, error: "not-found" });
    }
    return respond({ ok: false, error: error instanceof GatewayError && error.status === 503
      ? "unavailable"
      : "error" });
  }
}, "automation-result-erase");
