// Public sign-in. A person signs in with their email and password; single sign-on appears beside it
// where the deployment has configured one. The edge config decides which methods render.
import { Show, Suspense } from "solid-js";
import { Title } from "@solidjs/meta";
import { createAsync, useSearchParams, useSubmission } from "@solidjs/router";
import { AuthShell } from "../components/auth/AuthShell";
import { SIGN_IN_CUT } from "../components/auth/Sheath";
import { getAuthConfig, loginDev, loginWithPassword, startSso } from "../lib/auth";
import { safeAuthReturnTo } from "../lib/auth-return";

/** What went wrong, in the visitor's terms. Unknown codes get the generic line. */
function errorCopy(code: string): string {
  switch (code) {
    case "credentials":
      return "That email and password don't match an account. Check both and try again.";
    case "rate_limited":
      return "Too many attempts. Wait a minute, then try again.";
    default:
      return "Sign-in couldn't be completed. Try again, or contact your Myelin administrator if it keeps happening.";
  }
}

export default function Login() {
  const config = createAsync(() => getAuthConfig());
  const [params] = useSearchParams();
  const passwordPending = useSubmission(loginWithPassword);
  const ssoPending = useSubmission(startSso);
  const devPending = useSubmission(loginDev);

  const error = () => (typeof params.error === "string" ? params.error : "");
  const credentialsRejected = () => error() === "credentials";
  const returnTo = () => safeAuthReturnTo(params.return_to);
  const busy = () => passwordPending.pending || ssoPending.pending || devPending.pending;
  const resetHelpHref = () => {
    const query = new URLSearchParams({ help: "password" });
    if (typeof params.return_to === "string") query.set("return_to", returnTo());
    return `/login?${query}`;
  };

  return (
    <AuthShell cut={SIGN_IN_CUT} busy={busy()}>
      <Title>Sign in · Myelin</Title>
      <section aria-labelledby="login-heading" class="auth-form">
        <h1 id="login-heading" class="auth-heading">
          Sign in
        </h1>
        <p class="auth-sub">Welcome back to Myelin.</p>

        <Show when={error()}>
          <p role="alert" id="login-error-msg" class="auth-alert" data-testid="login-error">
            {errorCopy(error())}
          </p>
        </Show>
        <Show when={params.help === "password" && !error()}>
          <p role="status" class="auth-note" data-testid="password-help">
            Password resets come from whoever runs this Myelin deployment. Ask them for a reset
            link; it opens a page where you choose a new password.
          </p>
        </Show>

        <Suspense fallback={<p class="auth-note">Loading sign-in…</p>}>
          <Show when={config()} keyed>
            {(cfg) => (
              <>
                <Show when={cfg.password_login_enabled}>
                  <form
                    action={loginWithPassword}
                    method="post"
                    class="auth-fields"
                    data-testid="password-login-form"
                  >
                    <input type="hidden" name="return_to" value={returnTo()} />
                    <label class="auth-field">
                      <span class="auth-label">Email</span>
                      <input
                        name="email"
                        type="email"
                        required
                        autocomplete="username"
                        autocapitalize="off"
                        spellcheck={false}
                        class="auth-input"
                        data-testid="email-input"
                        aria-invalid={credentialsRejected() ? "true" : undefined}
                        aria-describedby={credentialsRejected() ? "login-error-msg" : undefined}
                      />
                    </label>
                    <div class="auth-field auth-field--linked">
                      <label class="auth-field">
                        <span class="auth-label">Password</span>
                        <input
                          name="password"
                          type="password"
                          required
                          autocomplete="current-password"
                          class="auth-input"
                          data-testid="password-input"
                          aria-invalid={credentialsRejected() ? "true" : undefined}
                          aria-describedby={credentialsRejected() ? "login-error-msg" : undefined}
                        />
                      </label>
                      <a class="auth-link" href={resetHelpHref()} data-testid="forgot-password">
                        Forgot password?
                      </a>
                    </div>
                    <button
                      type="submit"
                      class="auth-btn auth-btn--primary"
                      data-testid="password-login"
                      aria-busy={passwordPending.pending ? "true" : undefined}
                    >
                      {passwordPending.pending ? "Signing in…" : "Sign in"}
                    </button>
                  </form>
                </Show>

                <Show when={cfg.sso_configured}>
                  <Show when={cfg.password_login_enabled}>
                    <p class="auth-or">or</p>
                  </Show>
                  <form action={startSso} method="post" class="auth-fields">
                    <input type="hidden" name="return_to" value={returnTo()} />
                    <button
                      type="submit"
                      class="auth-btn"
                      classList={{ "auth-btn--primary": !cfg.password_login_enabled }}
                      data-testid="sso-login"
                      aria-busy={ssoPending.pending ? "true" : undefined}
                    >
                      {ssoPending.pending
                        ? "Redirecting to your provider…"
                        : `Continue with ${cfg.providers[0]?.label ?? "single sign-on"}`}
                    </button>
                  </form>
                </Show>

                <Show when={!cfg.password_login_enabled && !cfg.sso_configured}>
                  <p role="status" class="auth-note" data-testid="login-unavailable">
                    Sign-in isn't available right now. Myelin couldn't reach its sign-in service;
                    try again in a moment.
                  </p>
                </Show>

                <Show when={cfg.password_login_enabled}>
                  <p class="auth-foot" data-testid="invitation-hint">
                    New here? Accounts are created by invitation. Ask a teammate for a link.
                  </p>
                </Show>

                <Show when={cfg.dev_login_enabled}>
                  <div class="auth-seam" data-testid="dev-seam">
                    <form action={loginDev} method="post">
                      <input type="hidden" name="return_to" value={returnTo()} />
                      <button type="submit" class="auth-btn" data-testid="dev-login">
                        Continue as Dev Operator
                      </button>
                    </form>
                    <p>Development build only. Production builds never offer this.</p>
                  </div>
                </Show>
              </>
            )}
          </Show>
        </Suspense>
      </section>
    </AuthShell>
  );
}
