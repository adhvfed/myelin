import { Icon } from "@myelin/design-system";
import type { CiRunState } from "~/lib/ci-read-response";

export function CiStatus(props: { state: CiRunState | string }) {
  const view = () => ciStatusView(props.state);
  return (
    <span class="ci-status" data-state={props.state} title={`State: ${view().label}`}>
      <Icon name={view().icon} />
      <span>{view().label}</span>
    </span>
  );
}

export function ciStatusView(state: string): {
  label: string;
  icon: "check-pass" | "check-fail" | "check-pending" | "close";
} {
  switch (state) {
    case "succeeded":
    case "passed":
      return { label: state === "passed" ? "Passed" : "Succeeded", icon: "check-pass" };
    case "failed":
      return { label: "Failed", icon: "check-fail" };
    case "running":
      return { label: "Running", icon: "check-pending" };
    case "queued":
      return { label: "Queued", icon: "check-pending" };
    case "leased":
      return { label: "Leased", icon: "check-pending" };
    case "cancelled":
      return { label: "Cancelled", icon: "close" };
    case "timed_out":
      return { label: "Timed out", icon: "close" };
    case "reaped":
      return { label: "Reaped", icon: "close" };
    case "skipped":
      return { label: "Skipped", icon: "close" };
    default:
      return { label: "Unknown", icon: "check-pending" };
  }
}

export function formatCiDate(value: string): string {
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(value));
}
