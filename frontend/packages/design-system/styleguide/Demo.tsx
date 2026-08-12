// Styleguide smoke target for theme tokens, icons, and accessibility.

import { createSignal, For, type JSX } from "solid-js";
import { Icon } from "../src/Icon";
import { Z_INDEX, type ThemeName } from "../generated/tokens";

const card: JSX.CSSProperties = {
  background: "var(--surface-raised)",
  color: "var(--text-primary)",
  border: "var(--hairline) solid var(--border)",
  "border-radius": "var(--radius-2)",
  padding: "var(--space-4)",
  "font-family": "var(--font-sans)",
  "font-size": "var(--fs-body)",
};

const statusStyle = (kind: "success" | "warning" | "danger"): JSX.CSSProperties => ({
  display: "inline-flex",
  "align-items": "center",
  gap: "var(--space-1)",
  color: `var(--${kind})`,
  background: `var(--${kind}-subtle)`,
  border: `var(--hairline) solid var(--${kind})`,
  "border-radius": "var(--radius-1)",
  padding: "var(--space-1) var(--space-2)",
  "font-size": "var(--fs-caption)",
});

const STATUSES = [
  { kind: "success" as const, icon: "check-pass" as const, label: "Passed" },
  { kind: "warning" as const, icon: "check-pending" as const, label: "At risk" },
  { kind: "danger" as const, icon: "check-fail" as const, label: "Failed" },
];

export function Demo(): JSX.Element {
  const [theme, setTheme] = createSignal<ThemeName>("dark");
  return (
    // The data-theme wrapper is what re-skins the subtree (the 3-theme switch).
    <div data-theme={theme()} style={{ background: "var(--surface)", padding: "var(--space-5)" }}>
      <section aria-labelledby="demo-heading" style={card}>
        <h1 id="demo-heading" style={{ "font-size": "var(--fs-h2)", margin: "0 0 var(--space-3)" }}>
          <Icon name="agent" size={20} title="Myelin" /> Design-system smoke
        </h1>

        <label style={{ display: "block", "margin-bottom": "var(--space-3)" }}>
          Theme
          <select
            value={theme()}
            onChange={(e) => setTheme(e.currentTarget.value as ThemeName)}
            style={{ "margin-inline-start": "var(--space-2)" }}
          >
            <option value="dark">Dark</option>
            <option value="light">Light</option>
            <option value="high-contrast">High contrast</option>
          </select>
        </label>

        <div style={{ display: "flex", gap: "var(--space-2)", "flex-wrap": "wrap" }}>
          <For each={STATUSES}>
            {(s) => (
              <span style={statusStyle(s.kind)}>
                {/* glyph + LABEL — status is never carried by color alone (WCAG 1.4.1) */}
                <Icon name={s.icon} size={14} />
                {s.label}
              </span>
            )}
          </For>
        </div>

        <p style={{ color: "var(--text-muted)", "font-size": "var(--fs-body-sm)", "margin-top": "var(--space-3)" }}>
          Modal layer sits at z-index {Z_INDEX.modal}; toast above it at {Z_INDEX.toast}.
        </p>

        <button
          type="button"
          style={{
            "margin-top": "var(--space-3)",
            background: "var(--c-btn-primary-bg)",
            color: "var(--c-btn-primary-text)",
            border: "none",
            "border-radius": "var(--radius-1)",
            padding: "var(--space-2) var(--space-4)",
            height: "var(--control-h)",
            cursor: "pointer",
          }}
        >
          Primary action
        </button>
      </section>
    </div>
  );
}
