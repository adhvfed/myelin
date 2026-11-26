// An invitation link. Opening it is how a person gets an account (or, for a reset link, a new
// password): the link is the proof, so the page only asks for a name and a password.
import { createSignal, Match, Show, Suspense, Switch } from "solid-js";
import { Title } from "@solidjs/meta";
import { createAsync, useParams, useSearchParams, useSubmission } from "@solidjs/router";
import { AuthShell } from "../../components/auth/AuthShell";
import { JOIN_CUT } from "../../components/auth/Sheath";
import { acceptInvitation, getInvitation } from "../../lib/auth";

const MIN_PASSWORD_LENGTH = 10;

function errorCopy(code: string): string {
  switch (code) {
    case "weak_password":
      return `That password is too short. Use at least ${MIN_PASSWORD_LENGTH} characters.`;
    case "invalid_name":
      return "Enter the name your teammates should see.";
    case "account_exists":
      return "An account with this email already exists. Sign in instead.";
    case "person_erased":
      return "This address belonged to an account that was erased, so it can't join again.";
    case "rate_limited":
      return "Too many attempts. Wait a minute, then try again.";
    case "link_invalid":
      return "This link has expired or was already used. Ask for a new one.";
    default:
      return "That didn't go through. Try again in a moment.";
  }
}

export default function Join() {
  const route = useParams();
  const [params] = useSearchParams();
  const view = createAsync(() => getInvitation(route.code ?? ""));
  const accepting = useSubmission(acceptInvitation);
  const [length, setLength] = createSignal(0);

  const error = () => (typeof params.error === "string" ? params.error : "");
  // The sheath wraps from the inside out as the password approaches the required length, and the
  // accent layer closes last, on the character that satisfies it. It shows only what the policy
  // checks, which is length.
  const layers = JOIN_CUT.rings.length;
  const accentRing = JOIN_CUT.accent[0];
  const turn = (ring: number) =>
    ring === accentRing ? layers - 1 : ring > accentRing ? ring - 1 : ring;
  const wrap = (ring: number) => {
    const progress = Math.min(length() / MIN_PASSWORD_LENGTH, 1) * layers;
    return 0.1 + 0.9 * Math.max(0, Math.min(1, progress - turn(ring)));
  };

  return (
    <AuthShell cut={JOIN_CUT} fill={wrap} guides busy={accepting.pending}>
      <Suspense fallback={<p class="auth-note">Opening your invitation…</p>}>
        <Switch>
          <Match when={view()?.state === "invalid"}>
            <Title>Invitation · Myelin</Title>
            <section aria-labelledby="join-heading" class="auth-form" data-testid="join-invalid">
              <h1 id="join-heading" class="auth-heading">
                This link can't be used
              </h1>
              <p class="auth-sub">
                It has expired or was already used. Ask whoever sent it for a new one.
              </p>
              <a class="auth-btn" href="/login">
                Go to sign in
              </a>
            </section>
          </Match>
          <Match when={view()?.state === "unavailable"}>
            <Title>Invitation · Myelin</Title>
            <section aria-labelledby="join-heading" class="auth-form" data-testid="join-unavailable">
              <h1 id="join-heading" class="auth-heading">
                We couldn't open this link
              </h1>
              <p class="auth-sub">
                Myelin couldn't check the invitation just now. The link is still good; try again in
                a moment.
              </p>
              <a class="auth-btn" href={`/join/${route.code ?? ""}`}>
                Try again
              </a>
            </section>
          </Match>
          <Match when={view()?.state === "ready" ? view() : undefined} keyed>
            {(ready) => {
              const invitation = ready.state === "ready" ? ready.invitation : undefined;
              const joining = invitation?.kind === "join";
              return (
                <section aria-labelledby="join-heading" class="auth-form">
                  <Title>{joining ? `Join ${invitation?.organization}` : "New password"} · Myelin</Title>
                  <h1 id="join-heading" class="auth-heading">
                    {joining ? `Join ${invitation?.organization}` : "Choose a new password"}
                  </h1>
                  <p class="auth-sub">
                    <Show
                      when={joining}
                      fallback={
                        <>
                          For your account at <strong>{invitation?.organization}</strong> on Myelin.
                        </>
                      }
                    >
                      You've been invited to <strong>{invitation?.organization}</strong> on Myelin.
                    </Show>
                  </p>

                  <Show when={error()}>
                    <p role="alert" id="join-error-msg" class="auth-alert" data-testid="join-error">
                      {errorCopy(error())}
                    </p>
                  </Show>

                  <form action={acceptInvitation} method="post" class="auth-fields" data-testid="join-form">
                    <input type="hidden" name="code" value={route.code ?? ""} />
                    <label class="auth-field">
                      <span class="auth-label">Email</span>
                      <input
                        type="email"
                        readonly
                        autocomplete="username"
                        value={invitation?.email ?? ""}
                        class="auth-input"
                        data-testid="join-email"
                      />
                    </label>
                    <Show when={joining}>
                      <label class="auth-field">
                        <span class="auth-label">Your name</span>
                        <input
                          name="display_name"
                          type="text"
                          required
                          maxlength="120"
                          autocomplete="name"
                          class="auth-input"
                          data-testid="join-name"
                        />
                      </label>
                    </Show>
                    <div class="auth-field">
                      <label class="auth-field">
                        <span class="auth-label">{joining ? "Choose a password" : "New password"}</span>
                        <input
                          name="password"
                          type="password"
                          required
                          minlength={MIN_PASSWORD_LENGTH}
                          autocomplete="new-password"
                          aria-describedby="join-password-hint"
                          aria-invalid={error() === "weak_password" ? "true" : undefined}
                          class="auth-input"
                          data-testid="join-password"
                          onInput={(event) => setLength([...event.currentTarget.value].length)}
                        />
                      </label>
                      <p id="join-password-hint" class="auth-hint">
                        <span>At least {MIN_PASSWORD_LENGTH} characters.</span>
                        <span
                          aria-hidden="true"
                          data-met={length() >= MIN_PASSWORD_LENGTH ? "" : undefined}
                          data-testid="join-password-count"
                        >
                          {Math.min(length(), MIN_PASSWORD_LENGTH)} / {MIN_PASSWORD_LENGTH}
                        </span>
                      </p>
                    </div>
                    <button
                      type="submit"
                      class="auth-btn auth-btn--primary"
                      data-testid="join-submit"
                      aria-busy={accepting.pending ? "true" : undefined}
                    >
                      {accepting.pending
                        ? joining ? "Creating your account…" : "Saving…"
                        : joining ? "Create account" : "Save password and sign in"}
                    </button>
                  </form>
                </section>
              );
            }}
          </Match>
        </Switch>
      </Suspense>
    </AuthShell>
  );
}
