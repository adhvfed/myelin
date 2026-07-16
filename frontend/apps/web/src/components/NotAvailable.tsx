// The teaching "not available yet" state (R3.4 / firstrun #1) — rendered INSIDE the shell by the
// catch-all route and the unbuilt-subsystem indexes (Issues/Chat/CI/Knowledge), NOT a framework 404.
// Honest chrome: a heading naming the subsystem, a neutral "soon" tag (never accent, never color-alone
// — the tag CARRIES the meaning), a calm body, and a primary "Go to Code" action so the operator is
// never stranded. `role="note"` (calm, not an error to fix). Semantic tokens only.
import { A } from "@solidjs/router";
import { Icon } from "@myelin/design-system";

export interface NotAvailableProps {
  /** The subsystem/surface name (e.g. "Issues"). Falls back to the legacy `kind` label. */
  subsystem?: string;
  /** Legacy: a short kind label ("repository", "file", …) for missing-route-segment guards. */
  kind?: string;
}

export function NotAvailable(props: NotAvailableProps) {
  const label = () => props.subsystem ?? capitalize(props.kind ?? "This page");
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
      <Icon name="gate" size={28} title="Not available yet" />
      {/* The neutral "soon" tag — muted fill + a text label; the meaning is the WORD, not a colour
          (WCAG 1.4.1). */}
      <span
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
        Coming soon
      </span>
      <h2 id="not-available-heading" style={{ "font-size": "var(--fs-h3)", margin: "0" }}>
        {label()} isn&rsquo;t here yet
      </h2>
      <p style={{ color: "var(--text-muted)", margin: "0", "max-width": "42ch" }}>
        This surface lands with its subsystem. Your place is kept &mdash; nothing here blames you for
        its absence.
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
        <Icon name="nav-code" /> Go to Code
      </A>
    </div>
  );
}

function capitalize(s: string): string {
  return s.length ? s[0]!.toUpperCase() + s.slice(1) : s;
}
