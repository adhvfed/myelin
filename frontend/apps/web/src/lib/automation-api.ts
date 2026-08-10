import { action, json, query, redirect } from "@solidjs/router";
import { edgeGet, edgePost, GatewayError, isUnauthorized } from "../server/gateway";
import {
  AUTOMATION_PAGE_LIMIT,
  isAutomationId,
  parseAutomation,
  parseAutomationErasure,
  parseAutomationFiringPage,
  parseAutomationLifecycle,
  parseAutomationPage,
  parseAutomationResult,
  type AutomationErasureVM,
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
      { idempotencyKey: crypto.randomUUID() },
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
      { idempotencyKey: crypto.randomUUID() },
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
