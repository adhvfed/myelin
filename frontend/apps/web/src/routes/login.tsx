// Public sign-in. A person signs in with their email and password; single sign-on appears beside it
// where the deployment has configured one. The edge config decides which methods render, and
// whether a forgotten password is reset by email or by asking an administrator.
import { Show, Suspense } from "solid-js";
import { Title } from "@solidjs/meta";
import { createAsync, useSearchParams, useSubmission } from "@solidjs/router";
import { AuthShell } from "../components/auth/AuthShell";
import { SIGN_IN_CUT } from "../components/auth/Sheath";
import {
  getAuthConfig,
  loginDev,
  loginWithPassword,
  requestPasswordReset,
  startSso,
} from "../lib/auth";
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

function resetErrorCopy(code: string): string {
  switch (code) {
    case "invalid_email":
      return "That doesn't look like an email address.";
    case "rate_limited":
      return "Too many requests. Wait a minute, then try again.";
    default:
      return "The request couldn't be sent. Try again in a moment.";
  }
}

export default function Login() {
  const config = createAsync(() => getAuthConfig());
  const [params] = useSearchParams();
  const passwordPending = useSubmission(loginWithPassword);
  const ssoPending = useSubmission(startSso);
  const devPending = useSubmission(loginDev);
  const resetPending = useSubmission(requestPasswordReset);

  const error = () => (typeof params.error === "string" ? params.error : "");
  const credentialsRejected = () => error() === "credentials";
  const signedOut = () => {
    if (params.signed_out === "everywhere") return "You were signed out of all devices.";
    if (params.signed_out === "session") return "You were signed out.";
    if (params.signed_out === "password_changed") {
      return "Your password was changed. Sign in again.";
    }
    if (params.signed_out === "password_changed_maybe") {
      return "Your password may have changed. Sign in with your new password, or your old one if it didn't.";
    }
    return "";
  };
  const returnTo = () => safeAuthReturnTo(params.return_to);
  const busy = () =>
    passwordPending.pending || ssoPending.pending || devPending.pending || resetPending.pending;
  const resetHelpHref = () => {
    const query = new URLSearchParams({ help: "password" });
    if (typeof params.return_to === "string") query.set("return_to", returnTo());
    return `/login?${query}`;
  };
  const signInHref = () =>
    typeof params.return_to === "string"
      ? `/login?${new URLSearchParams({ return_to: returnTo() })}`
      : "/login";
  const resettingByEmail = () =>
    params.help === "password" && config()?.mail_delivery_enabled === true;

  return (
    <AuthShell cut={SIGN_IN_CUT} busy={busy()}>
      <Suspense fallback={<p class="auth-note">Loading sign-in…</p>}>
        <Show when={resettingByEmail()} fallback={signIn()}>
          <PasswordReset signInHref={signInHref()} />
        </Show>
      </Suspense>
    </AuthShell>
  );

  function signIn() {
    return (
      <section aria-labelledby="login-heading" class="auth-form">
        <Title>Sign in · Myelin</Title>
        <h1 id="login-heading" class="auth-heading">
          Sign in
        </h1>
        <p class="auth-sub">Welcome back to Myelin.</p>

        <Show when={error()}>
          <p role="alert" id="login-error-msg" class="auth-alert" data-testid="login-error">
            {errorCopy(error())}
          </p>
        </Show>
        <Show when={signedOut() && !error()}>
          <p role="status" class="auth-note" data-testid="signed-out">
            {signedOut()}
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
    );
  }
}

/** Ask for a reset link by email. The page never learns whether the address has an account. */
function PasswordReset(props: { signInHref: string }) {
  const requesting = useSubmission(requestPasswordReset);
  const sent = () => (requesting.result?.ok ? requesting.result.email : undefined);
  const failure = () =>
    requesting.result && !requesting.result.ok ? requesting.result.reason : undefined;

  return (
    <section aria-labelledby="reset-heading" class="auth-form" data-testid="password-reset">
      <Title>Reset password · Myelin</Title>
      <h1 id="reset-heading" class="auth-heading">
        Reset your password
      </h1>
      <p class="auth-sub">We'll email you a link to choose a new one.</p>

      <Show when={failure()} keyed>
        {(reason) => (
          <p role="alert" id="reset-error-msg" class="auth-alert" data-testid="reset-error">
            {resetErrorCopy(reason)}
          </p>
        )}
      </Show>
      <Show
        when={sent()}
        keyed
        fallback={
          <form
            action={requestPasswordReset}
            method="post"
            class="auth-fields"
            data-testid="reset-form"
          >
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
                data-testid="reset-email"
                aria-invalid={failure() === "invalid_email" ? "true" : undefined}
                aria-describedby={failure() ? "reset-error-msg" : undefined}
              />
            </label>
            <button
              type="submit"
              class="auth-btn auth-btn--primary"
              data-testid="reset-submit"
              aria-busy={requesting.pending ? "true" : undefined}
            >
              {requesting.pending ? "Sending…" : "Send reset link"}
            </button>
          </form>
        }
      >
        {(email) => (
          <p role="status" class="auth-note" data-testid="reset-sent">
            If an account exists for <strong>{email}</strong>, a reset link is on its way.
          </p>
        )}
      </Show>
      <a class="auth-btn" href={props.signInHref} data-testid="reset-back">
        Back to sign in
      </a>
    </section>
  );
}
