// Public login route. The edge config controls the available methods; development login also
// requires a non-production build.
import { Show, Suspense } from "solid-js";
import { Title } from "@solidjs/meta";
import { createAsync, useSearchParams, useSubmission } from "@solidjs/router";
import { Icon } from "@myelin/design-system";
import { getAuthConfig, loginDev, loginWithToken, startSso } from "../lib/auth";
import { safeAuthReturnTo } from "../lib/auth-return";

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
  background: "var(--c-btn-primary-bg)",
  color: "var(--c-btn-primary-text)",
  "font-weight": "600",
  cursor: "pointer",
} as const;

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
  // Token failures use different guidance from SSO failures.
  const isTokenError = () => params.error === "token_invalid";
  const returnTo = () => safeAuthReturnTo(params.return_to);

  return (
    <main
      class="login-main"
      style={{
        // Account for dynamic mobile browser chrome.
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

        {/* Announce login failures without exposing raw server errors. */}
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

                <Show when={cfg.token_login_enabled}>
                  <form
                    action={loginWithToken}
                    method="post"
                    data-testid="token-login-form"
                    style={{ margin: "0", display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}
                  >
                    <input type="hidden" name="return_to" value={returnTo()} />
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

                <p style={{ margin: "0", display: "flex", "align-items": "center", gap: "var(--space-1)", color: "var(--text-muted)", "font-size": "var(--fs-caption)" }}>
                  <Icon name="database" />
                  Data region: <strong style={{ color: "var(--text-primary)", "font-weight": "500" }}>EU-West</strong>
                </p>

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
