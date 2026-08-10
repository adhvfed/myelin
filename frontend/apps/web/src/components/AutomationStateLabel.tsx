import { Icon } from "@myelin/design-system";
import type { AutomationState } from "~/lib/automation-response";

export function AutomationStateLabel(props: { state: AutomationState }) {
  return (
    <span class="automation-status" data-state={props.state} title={`State: ${stateLabel(props.state)}`}>
      <Icon name={props.state === "active" ? "check-pass" : props.state === "paused" ? "check-pending" : "close"} />
      {stateLabel(props.state)}
    </span>
  );
}

function stateLabel(state: AutomationState): string {
  return state[0]!.toUpperCase() + state.slice(1);
}
