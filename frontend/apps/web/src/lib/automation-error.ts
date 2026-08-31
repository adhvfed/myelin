/** Safe presentation categories carried across the SolidStart server-function boundary. */
export type AutomationErrorKind = "bad-input" | "not-found" | "conflict" | "unavailable" | "error";
export const AUTOMATION_ERR_PREFIX = "AUTOMATION_ERR:";

export class AutomationRouteError extends Error {
  readonly kind: AutomationErrorKind;

  constructor(kind: AutomationErrorKind) {
    super(`${AUTOMATION_ERR_PREFIX}${kind}`);
    this.name = "AutomationRouteError";
    this.kind = kind;
  }
}
