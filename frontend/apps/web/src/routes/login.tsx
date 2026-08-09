// THE /login ROUTE (outside the authenticated `(app)` group) — the first-run entry (R3.5).
// Honestly-labelled paths, driven by the UNAUTHENTICATED `GET /v1/auth/config`:
//   1. The real OIDC/SSO login is the PRIMARY affordance — it rides the DERIVED button token
//      (`--c-btn-primary-bg`, R3.6 fix preserved), never raw --accent. When SSO is configured it is
//      enabled and posts to the `startSso` seam; when it is NOT, it is `aria-disabled` with a VISIBLE
//      reason (never a title tooltip) so keyboard/touch/AT users all see why.
//   2. The DEV-SESSION SEAM is relegated below a divider and RENDERS ONLY when the server flag
//      (`auth/config.dev_login_enabled`, itself belt-and-braced with the build-time PROD kill switch)
//      allows it — a production build never even paints it.
// Full-height (100dvh), honest error/loading states. Semantic tokens only.
import { Show, Suspense } from "solid-js";
import { Title } from "@solidjs/meta";
import { createAsync, useSearchParams, useSubmission } from "@solidjs/router";
import { Icon } from "@myelin/design-system";
import { getAuthConfig, loginDev, loginWithToken, startSso } from "../lib/auth";
import { safeAuthReturnTo } from "../lib/auth-return";

// The card chrome, shared by every state.
const card = {
  width: "100%",
  "max-width": "24rem",
  display: "flex",
  "flex-direction": "column",
  gap: "var(--space-4)",
  padding: "var(--space-5)",
  border: "var(--hairline) solid var(--border-strong)",
  "border-radius": "var(--radius-2)",
  background: "var(--surface-raised)",
} as const;

const primaryBtn = {
  display: "inline-flex",
  "align-items": "center",
  "justify-content": "center",
  gap: "var(--space-2)",
  width: "100%",
  padding: "var(--space-2) var(--space-3)",
  border: "none",
  "border-radius": "var(--radius-1)",
  // Primary rides the DERIVED button token (→ focus-ring, contrast floor), never raw --accent.
  background: "var(--c-btn-primary-bg)",
  color: "var(--c-btn-primary-text)",
  "font-weight": "600",
  cursor: "pointer",
} as const;

// A secondary (outline) button — used for the operator-token submit WHEN SSO is the primary. When SSO
// is unconfigured, the token card is the primary affordance instead and rides `primaryBtn`.
const secondaryBtn = {
  display: "inline-flex",
  "align-items": "center",
  "justify-content": "center",
  gap: "var(--space-2)",
  width: "100%",
  padding: "var(--space-2) var(--space-3)",
  border: "var(--hairline) solid var(--border-strong)",
  "border-radius": "var(--radius-1)",
  background: "var(--surface)",
  color: "var(--text-primary)",
  "font-weight": "500",
  cursor: "pointer",
} as const;

export default function Login() {
  const config = createAsync(() => getAuthConfig());
  const [params] = useSearchParams();
  const ssoPending = useSubmission(startSso);
  const tokenPending = useSubmission(loginWithToken);

  const hasError = () => Boolean(params.error);
  // R4.0 — the operator-token failure has its OWN honest copy (blames the token/bootstrap, not the
  // user), distinct from the SSO-not-wired message. Key off the `?error=` value.
  const isTokenError = () => params.error === "token_invalid";
  const returnTo = () => safeAuthReturnTo(params.return_to);

  return (
    <main
      class="login-main"
      style={{
        // 100dvh (not 100vh) so mobile browser chrome never clips the card — a11y #7.
        "min-height": "100dvh",
        display: "flex",
        "flex-direction": "column",
        "align-items": "center",
        "justify-content": "center",
        gap: "var(--space-6)",
        padding: "var(--space-6) var(--space-4)",
      }}
    >
      <Title>Sign in · Myelin</Title>

      <section aria-labelledby="login-heading" style={card}>
        <h1
          id="login-heading"
          style={{ "font-size": "var(--fs-h2)", margin: "0", display: "flex", "align-items": "center", gap: "var(--space-2)" }}
        >
          <Icon name="human" /> Sign in to Myelin
        </h1>

        {/* Login failure (error) — system-blaming, one line + a path; NEVER a raw err.message.
            role=alert announces assertively. */}
        <Show when={hasError()}>
          <p
            role="alert"
            id="login-error-msg"
            data-testid={isTokenError() ? "login-error-token" : "login-error"}
            style={{
              margin: "0",
              display: "flex",
              gap: "var(--space-2)",
              "align-items": "flex-start",
              "font-size": "var(--fs-body-sm)",
              border: "var(--hairline) solid var(--danger)",
              "border-radius": "var(--radius-1)",
              padding: "var(--space-2) var(--space-3)",
              background: "var(--danger-subtle)",
            }}
          >
            <Icon name="gate" />
            {/* Token failures blame the token/bootstrap step; every other error keeps the SSO copy. */}
            <Show
              when={isTokenError()}
              fallback={
                <span>
                  Sign-in couldn't be completed. You can try again, or contact your Myelin
                  administrator if the problem continues. Nothing's wrong on your end.
                </span>
              }
            >
              <span>
                That operator token didn't work — it's likely invalid or expired. Re-run{" "}
                <code style={{ "font-family": "var(--font-mono)" }}>edge bootstrap</code> to print a
                fresh token, then paste it below. Nothing's wrong on your end.
              </span>
            </Show>
          </p>
        </Show>

        <Suspense fallback={<p style={{ margin: "0", color: "var(--text-muted)" }}>Loading sign-in options…</p>}>
          <Show when={config()} keyed>
            {(cfg) => (
              <>
                {/* PRIMARY — real OIDC/SSO. Enabled when configured (posts to the startSso seam);
                    otherwise aria-disabled with a VISIBLE reason. */}
                <Show
                  when={cfg.sso_configured}
                  fallback={
                    <div style={{ display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
                      <button
                        type="button"
                        data-testid="sso-login"
                        aria-disabled="true"
                        aria-describedby="sso-reason"
                        style={{ ...primaryBtn, cursor: "not-allowed", opacity: "0.6" }}
                      >
                        <Icon name="human" /> Continue with single sign-on
                      </button>
                      {/* The reason is VISIBLE TEXT referenced by aria-describedby, not a title. */}
                      <p
                        id="sso-reason"
                        data-testid="sso-reason"
                        style={{
                          margin: "0",
                          display: "flex",
                          gap: "var(--space-2)",
                          "align-items": "flex-start",
                          "font-size": "var(--fs-body-sm)",
                          color: "var(--text-muted)",
                          border: "var(--hairline) solid var(--warning)",
                          "border-radius": "var(--radius-1)",
                          padding: "var(--space-2) var(--space-3)",
                          background: "var(--warning-subtle)",
                        }}
                      >
                        <Icon name="gate" />
                        <span>
                          Single sign-on isn't available yet on this deployment — an administrator needs
                          to configure the identity provider. Contact your Myelin administrator.
                        </span>
                      </p>
                    </div>
                  }
                >
                  <form action={startSso} method="post" style={{ margin: "0", display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
                    <input type="hidden" name="return_to" value={returnTo()} />
                    <button
                      type="submit"
                      data-testid="sso-login"
                      aria-busy={ssoPending.pending ? "true" : undefined}
                      style={primaryBtn}
                    >
                      <Icon name="human" />
                      {ssoPending.pending
                        ? "Redirecting to your provider…"
                        : `Continue with ${cfg.providers[0]?.label ?? "single sign-on"}`}
                    </button>
                    <p style={{ margin: "0", color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }} role={ssoPending.pending ? "status" : undefined}>
                      {ssoPending.pending
                        ? "Taking you to your identity provider."
                        : "You'll be redirected to your organization's identity provider, then back to Myelin."}
                    </p>
                  </form>
                </Show>

                {/* R4.0 — OPERATOR-TOKEN LOGIN. A REAL working path (verifies against the live edge),
                    gated on the edge flag. When SSO is unconfigured this is the PRIMARY affordance and
                    rides the derived primary token; otherwise it's the secondary/outline alternative. */}
                <Show when={cfg.token_login_enabled}>
                  <form
                    action={loginWithToken}
                    method="post"
                    data-testid="token-login-form"
                    style={{ margin: "0", display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}
                  >
                    <input type="hidden" name="return_to" value={returnTo()} />
                    {/* The control is NESTED in its label (the codebase's association convention) — the
                        visible label text is the <span>; the input is the labelled control. */}
                    <label style={{ display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
                      <span style={{ display: "flex", "align-items": "center", gap: "var(--space-1)", "font-size": "var(--fs-body-sm)", "font-weight": "500", color: "var(--text-primary)" }}>
                        <Icon name="agent" /> Operator token
                      </span>
                      <input
                        id="operator-token"
                        name="token"
                        type="password"
                        required
                        autocomplete="off"
                        autocapitalize="off"
                        autocorrect="off"
                        spellcheck={false}
                        data-testid="token-input"
                        placeholder="Paste your capability token"
                        // The error alert (when present) + the helper are both referenced — the reason
                        // is VISIBLE TEXT, never a title tooltip.
                        aria-describedby={isTokenError() ? "login-error-msg token-help" : "token-help"}
                        aria-invalid={isTokenError() ? "true" : undefined}
                        style={{
                          width: "100%",
                          padding: "var(--space-2) var(--space-3)",
                          border: `var(--hairline) solid ${isTokenError() ? "var(--danger)" : "var(--border-strong)"}`,
                          "border-radius": "var(--radius-1)",
                          background: "var(--surface)",
                          color: "var(--text-primary)",
                          "font-family": "var(--font-mono)",
                          "font-size": "var(--fs-body-sm)",
                        }}
                      />
                    </label>
                    <p id="token-help" data-testid="token-help" style={{ margin: "0", color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>
                      Paste the token from{" "}
                      <code style={{ "font-family": "var(--font-mono)" }}>edge bootstrap</code>. It never
                      leaves your session — it's sent once to verify, then held server-side.
                    </p>
                    <button
                      type="submit"
                      data-testid="token-login"
                      aria-busy={tokenPending.pending ? "true" : undefined}
                      style={cfg.sso_configured ? secondaryBtn : primaryBtn}
                    >
                      <Icon name="agent" />
                      {tokenPending.pending ? "Verifying token…" : "Sign in with operator token"}
                    </button>
                  </form>
                </Show>

                {/* Ambient residency cue (P9, T0) — glyph + text, never colour-alone. */}
                <p style={{ margin: "0", display: "flex", "align-items": "center", gap: "var(--space-1)", color: "var(--text-muted)", "font-size": "var(--fs-caption)" }}>
                  <Icon name="database" />
                  Data region: <strong style={{ color: "var(--text-primary)", "font-weight": "500" }}>EU-West</strong>
                </p>

                {/* DEV SEAM — relegated below the divider and RENDERED ONLY when the server flag
                    allows it (belt-and-braces with the build-time PROD kill switch). A prod build
                    never paints it; a non-prod build without the opt-in doesn't either. */}
                <Show when={cfg.dev_login_enabled}>
                  <hr style={{ border: "0", "border-block-start": "var(--hairline) solid var(--border)", margin: "0" }} />
                  <div
                    data-testid="dev-seam"
                    style={{
                      display: "flex",
                      "flex-direction": "column",
                      gap: "var(--space-2)",
                      border: "var(--hairline) dashed var(--border)",
                      "border-radius": "var(--radius-1)",
                      padding: "var(--space-3)",
                    }}
                  >
                    <p style={{ margin: "0", display: "inline-flex", "align-items": "center", gap: "var(--space-1)", color: "var(--text-subtle)", "font-size": "var(--fs-caption)", "text-transform": "uppercase", "letter-spacing": "0.04em" }}>
                      <Icon name="gate" /> Development builds only
                    </p>
                    <form action={loginDev} method="post" style={{ margin: "0" }}>
                      <input type="hidden" name="return_to" value={returnTo()} />
                      <button
                        type="submit"
                        data-testid="dev-login"
                        style={{
                          width: "100%",
                          padding: "var(--space-2) var(--space-3)",
                          border: "var(--hairline) solid var(--border-strong)",
                          "border-radius": "var(--radius-1)",
                          background: "var(--surface)",
                          color: "var(--text-primary)",
                          "font-weight": "500",
                          cursor: "pointer",
                        }}
                      >
                        Continue as Dev Operator
                      </button>
                    </form>
                    <p style={{ margin: "0", color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>
                      A local session seam — never issued by a production build.
                    </p>
                  </div>
                </Show>
              </>
            )}
          </Show>
        </Suspense>
      </section>
    </main>
  );
}
