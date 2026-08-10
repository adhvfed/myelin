import { A } from "@solidjs/router";
import { Icon } from "@myelin/design-system";
import {
  AUTOMATION_ERR_PREFIX,
  AutomationRouteError,
  type AutomationErrorKind,
} from "~/lib/automation-error";

export function automationErrorKind(error: unknown): AutomationErrorKind {
  if (error instanceof AutomationRouteError) return error.kind;
  const message = error instanceof Error ? error.message : String(error ?? "");
  if (message.startsWith(AUTOMATION_ERR_PREFIX)) {
    const kind = message.slice(AUTOMATION_ERR_PREFIX.length);
    if (["bad-input", "not-found", "unavailable", "error"].includes(kind)) {
      return kind as AutomationErrorKind;
    }
  }
  return "error";
}

export function AutomationErrorState(props: {
  kind: AutomationErrorKind;
  onRetry?: () => void;
}) {
  const absent = () => props.kind === "not-found" || props.kind === "bad-input";
  return (
    <section
      role={absent() ? "note" : "alert"}
      class="automation-empty"
      data-testid="automation-error"
      data-kind={props.kind}
    >
      <Icon
        name={absent() ? "search" : "check-fail"}
        size={28}
        title={absent() ? "Not available" : "Unavailable"}
      />
      <h1>{absent() ? "This automation is not available to you" : "Automations are unavailable"}</h1>
      <p>
        {absent()
          ? "It may not exist, or it may belong to another operator."
          : "We couldn’t read the durable automation state. No action was inferred."}
      </p>
      <div class="automation-actions">
        <A href="/automations" class="automation-button automation-button-secondary">
          <Icon name="run" /> Automations
        </A>
        {props.onRetry && !absent() ? (
          <button
            type="button"
            class="automation-button automation-button-secondary"
            onClick={() => props.onRetry?.()}
          >
            <Icon name="rerun" /> Retry
          </button>
        ) : null}
      </div>
    </section>
  );
}
