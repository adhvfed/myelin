// THE /login ROUTE (outside the authenticated `(app)` group). Two paths, honestly labelled:
//   1. The DEV-SESSION SEAM — mints a real session (real gateway client + real cookie machinery) so
//      the shell + Playwright run authenticated now. Clearly NOT production auth.
//   2. The real OIDC/SSO login — DEFERRED (MR-012): the edge's human login REFUSES until JWKS/trust-
//      anchors land (refuse-not-mock). Shown disabled so the seam it replaces is explicit.
import { Title } from "@solidjs/meta";
import { Icon } from "@myelin/design-system";
import { loginDev } from "../lib/auth";

export default function Login() {
  return (
    <main
      class="login-main"
      style={{
        display: "flex",
        "align-items": "center",
        "justify-content": "center",
        padding: "var(--space-5)",
      }}
    >
      <Title>Sign in · Myelin</Title>
      <section
        aria-labelledby="login-heading"
        style={{
          width: "100%",
          "max-width": "24rem",
          display: "flex",
          "flex-direction": "column",
          gap: "var(--space-4)",
          padding: "var(--space-5)",
          border: "var(--hairline) solid var(--border-strong)",
          "border-radius": "var(--radius-2)",
          background: "var(--surface-raised)",
        }}
      >
        <h1 id="login-heading" style={{ "font-size": "var(--fs-h2)", margin: "0", display: "flex", "align-items": "center", gap: "var(--space-2)" }}>
          <Icon name="human" /> Sign in to Myelin
        </h1>

        <form action={loginDev} method="post" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
          <p style={{ margin: "0", color: "var(--text-muted)", "font-size": "var(--fs-body-sm)" }}>
            Development session seam — not production auth.
          </p>
          <button
            type="submit"
            data-testid="dev-login"
            // Primary action rides the DERIVED button token (→ focus-ring, 6.55:1), never raw
            // --accent (which sits at the AA floor in light) — DESIGN-MANUAL §3.1.
            style={{
              padding: "var(--space-2) var(--space-3)",
              border: "none",
              "border-radius": "var(--radius-1)",
              background: "var(--c-btn-primary-bg)",
              color: "var(--c-btn-primary-text)",
              cursor: "pointer",
              "font-weight": "600",
            }}
          >
            Continue as Dev Operator
          </button>
        </form>

        <div style={{ "border-block-start": "var(--hairline) solid var(--border)", "padding-block-start": "var(--space-3)", display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
          <button
            type="button"
            disabled
            aria-describedby="sso-disabled-reason"
            style={{
              width: "100%",
              padding: "var(--space-2) var(--space-3)",
              border: "var(--hairline) solid var(--border)",
              "border-radius": "var(--radius-1)",
              background: "transparent",
              color: "var(--text-subtle)",
              cursor: "not-allowed",
            }}
          >
            Sign in with SSO (deferred)
          </button>
          {/* The disabled reason is VISIBLE text, not a title tooltip (which is hidden from keyboard
              and touch users and most AT) — everyone sees why SSO is unavailable. */}
          <p id="sso-disabled-reason" style={{ margin: "0", color: "var(--text-muted)", "font-size": "var(--fs-caption)" }}>
            The real OIDC/SSO login is deferred (MR-012): the edge refuses until JWKS/trust-anchors are configured.
          </p>
        </div>
      </section>
    </main>
  );
}
