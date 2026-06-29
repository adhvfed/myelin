// A dignified "not available" state (the unglamorous-state discipline) — used when a deep-link is
// missing a required route segment, so a bad URL renders a calm note rather than crashing. Semantic
// tokens only.
import { Icon } from "@myelin/design-system";

export function NotAvailable(props: { kind: string }) {
  return (
    <div
      role="note"
      data-testid="not-available"
      style={{
        border: "var(--hairline) solid var(--border)",
        "border-radius": "var(--radius-1)",
        padding: "var(--space-3)",
        background: "var(--surface-raised)",
        color: "var(--text-muted)",
        display: "flex",
        "align-items": "center",
        gap: "var(--space-2)",
      }}
    >
      <Icon name="gate" /> <span>This {props.kind} is not available.</span>
    </div>
  );
}
