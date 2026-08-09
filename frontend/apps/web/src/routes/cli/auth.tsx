import { Title } from "@solidjs/meta";
import { createAsync, useSearchParams, useSubmission } from "@solidjs/router";
import { Icon } from "@myelin/design-system";
import { createMemo, Show, Suspense } from "solid-js";

import { getViewer, type Viewer } from "../../lib/auth";
import { approveCliLogin } from "../../lib/cli-auth";
import {
  canonicalCliUserCode,
  cliApprovalPath,
  type CliApprovalResult,
} from "../../lib/cli-auth-core";
import "./cli-auth.css";

function signInPath(code: string): string {
  const params = new URLSearchParams({ return_to: cliApprovalPath(code) });
  return `/login?${params.toString()}`;
}

function resultCopy(result: CliApprovalResult): { title: string; body: string } {
  switch (result) {
    case "approved":
      return {
        title: "CLI connected",
        body: "Return to your terminal. This request can’t be used to create another session.",
      };
    case "expired":
      return {
        title: "This request has expired",
        body: "Return to your terminal and start login again to get a fresh code.",
      };
    case "forbidden":
      return {
        title: "This session can’t approve CLI access",
        body: "Sign in with your human organization account, then try this request again.",
      };
    case "unavailable":
      return {
        title: "Approval is temporarily unavailable",
        body: "The request is still safe. Try approving again before the code expires.",
      };
  }
}

function validResult(value: unknown): CliApprovalResult | null {
  return value === "approved" || value === "expired" || value === "forbidden" ||
    value === "unavailable"
    ? value
    : null;
}

function ApprovalResult(props: { result: CliApprovalResult }) {
  const copy = () => resultCopy(props.result);
  const approved = () => props.result === "approved";
  return (
    <div
      role={approved() ? "status" : "alert"}
      class={`cli-auth-result ${approved() ? "is-approved" : "is-warning"}`}
      data-testid={`cli-approval-${props.result}`}
    >
      <strong><Icon name={approved() ? "check-pass" : "gate"} /> {copy().title}</strong>
      <p>{copy().body}</p>
    </div>
  );
}

function IdentitySummary(props: { viewer: Viewer }) {
  return (
    <dl class="cli-auth-identity">
      <dt>Signed in as</dt><dd>{props.viewer.displayName}</dd>
      <dt>Organization</dt><dd>{props.viewer.tenant}</dd>
      <dt>Data region</dt><dd>{props.viewer.region}</dd>
    </dl>
  );
}

function ApprovalChoice(props: { code: string; viewer: Viewer; pending: boolean }) {
  return (
    <div class="cli-auth-choice">
      <IdentitySummary viewer={props.viewer} />
      <p class="cli-auth-explanation">
        Approval creates a separate, time-bounded CLI session with no more access than this browser
        session. Your browser credential is never copied to the CLI.
      </p>
      <form action={approveCliLogin} method="post">
        <input type="hidden" name="user_code" value={props.code} />
        <button
          type="submit"
          class="cli-auth-primary"
          data-testid="cli-approve"
          aria-busy={props.pending ? "true" : undefined}
          disabled={props.pending}
        >
          <Icon name="approve" /> {props.pending ? "Approving…" : "Approve CLI access"}
        </button>
      </form>
    </div>
  );
}

export default function CliAuthorization() {
  const [params] = useSearchParams();
  const viewer = createAsync(() => getViewer());
  const approval = useSubmission(approveCliLogin);
  const userCode = createMemo(() => canonicalCliUserCode(params.code));
  const result = createMemo(() => validResult(params.result));
  const mayApprove = () => result() !== "approved" && result() !== "expired";

  return (
    <main class="cli-auth-page">
      <Title>Connect the Myelin CLI</Title>
      <section aria-labelledby="cli-auth-heading" class="cli-auth-card">
        <header class="cli-auth-header">
          <p class="cli-auth-eyebrow">Device authorization</p>
          <h1 id="cli-auth-heading"><Icon name="agent" /> Connect the Myelin CLI</h1>
          <p>Confirm that this browser and your terminal are part of the same sign-in request.</p>
        </header>

        <Show when={userCode()} keyed fallback={<InvalidCode />}>
          {(code) => (
            <>
              <div aria-label="CLI verification code" class="cli-auth-code" data-testid="cli-user-code">
                {code}
              </div>

              <Show when={result()} keyed>
                {(state) => <ApprovalResult result={state} />}
              </Show>

              <Show when={mayApprove()}>
                <Suspense fallback={<p class="cli-auth-muted">Checking your browser session…</p>}>
                  <Show
                    when={viewer()}
                    keyed
                    fallback={
                      <div class="cli-auth-choice">
                        <p class="cli-auth-muted">
                          Sign in with your organization account before approving this request.
                        </p>
                        <a href={signInPath(code)} class="cli-auth-primary" data-testid="cli-sign-in">
                          <Icon name="human" /> Sign in to continue
                        </a>
                      </div>
                    }
                  >
                    {(identity) => (
                      <ApprovalChoice
                        code={code}
                        viewer={identity}
                        pending={approval.pending === true}
                      />
                    )}
                  </Show>
                </Suspense>
              </Show>
            </>
          )}
        </Show>
      </section>
    </main>
  );
}

function InvalidCode() {
  return (
    <div role="alert" class="cli-auth-invalid" data-testid="cli-code-invalid">
      <strong><Icon name="gate" /> That CLI code isn’t valid</strong>
      <p>Return to your terminal and start login again. No access has been granted.</p>
    </div>
  );
}
