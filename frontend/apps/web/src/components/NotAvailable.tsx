import { A } from "@solidjs/router";
import { Icon } from "@myelin/design-system";

export interface NotAvailableProps {
  subsystem?: string;
  kind?: string;
  status?: "planned" | "missing";
}

export function NotAvailable(props: NotAvailableProps) {
  const label = () => props.subsystem ?? capitalize(props.kind ?? "This page");
  const missing = () => props.status === "missing";
  return (
    <div
      role="note"
      aria-labelledby="not-available-heading"
      data-testid="not-available"
      style={{
        border: "var(--hairline) solid var(--border)",
        "border-radius": "var(--radius-1)",
        padding: "var(--space-5)",
        background: "var(--surface-raised)",
        display: "flex",
        "flex-direction": "column",
        "align-items": "center",
        gap: "var(--space-3)",
        "text-align": "center",
      }}
    >
      <Icon name="gate" size={28} title={missing() ? "Not found" : "Not available yet"} />
      <span
        data-testid="availability-status"
        style={{
          display: "inline-flex",
          "align-items": "center",
          padding: "0 var(--space-2)",
          "font-size": "var(--fs-caption)",
          color: "var(--text-subtle)",
          border: "var(--hairline) solid var(--border)",
          "border-radius": "var(--radius-pill)",
        }}
      >
        {missing() ? "Not found" : "Coming soon"}
      </span>
      <h2 id="not-available-heading" style={{ "font-size": "var(--fs-h3)", margin: "0" }}>
        {label()} {missing() ? "wasn’t found" : "isn’t here yet"}
      </h2>
      <p style={{ color: "var(--text-muted)", margin: "0", "max-width": "42ch" }}>
        {missing()
          ? "Check the address or return to your repositories."
          : "This area is planned but is not available in this build."}
      </p>
      <A
        href="/git/repos"
        style={{
          display: "inline-flex",
          "align-items": "center",
          gap: "var(--space-1)",
          padding: "var(--space-2) var(--space-3)",
          border: "var(--hairline) solid var(--border)",
          "border-radius": "var(--radius-1)",
          color: "var(--text-primary)",
          background: "var(--surface)",
        }}
      >
        <Icon name="nav-code" /> Back to repositories
      </A>
    </div>
  );
}

function capitalize(s: string): string {
  return s.length ? s[0]!.toUpperCase() + s.slice(1) : s;
}
