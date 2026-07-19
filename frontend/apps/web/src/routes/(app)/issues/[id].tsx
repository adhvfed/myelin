import { Title } from "@solidjs/meta";
import { A, createAsync, revalidate, useAction, useParams } from "@solidjs/router";
import {
  ConfirmDialog,
  Icon,
  Skeleton,
  SkeletonBlock,
  StatusPill,
  useToast,
} from "@myelin/design-system";
import { ErrorBoundary, Show, Suspense, createMemo, createSignal } from "solid-js";
import { getIssue, issuesMutate, type IssueErrorKind, type IssueVM } from "~/lib/api";
import { isClosedCategory, issueErrorKind, issueTimestamp } from "~/lib/issue-view";

export default function IssueDetail() {
  const params = useParams();
  const toast = useToast();
  const mutate = useAction(issuesMutate);
  const [replacement, setReplacement] = createSignal<IssueVM | null>(null);
  const [confirming, setConfirming] = createSignal(false);
  const [closing, setClosing] = createSignal(false);
  const [closeError, setCloseError] = createSignal<IssueErrorKind | null>(null);
  const issue = createAsync(async (): Promise<{
    issue: IssueVM | null;
    error: IssueErrorKind | null;
  }> => {
    if (!params.id) return { issue: null, error: "not-found" };
    try {
      return { issue: await getIssue(params.id), error: null };
    } catch (error) {
      return { issue: null, error: issueErrorKind(error) };
    }
  });
  const current = createMemo(() => replacement() ?? issue()?.issue);

  const close = async () => {
    const row = current();
    if (!row || closing()) return;
    setClosing(true);
    setCloseError(null);
    try {
      const result = await mutate({ op: "close", issueId: row.id });
      if (!result.ok) {
        setCloseError(result.error);
        setConfirming(false);
        return;
      }
      if (result.op !== "close") {
        setCloseError("error");
        setConfirming(false);
        return;
      }
      setReplacement(result.issue);
      setConfirming(false);
      void revalidate("issue-detail");
      void revalidate("issues-list");
      toast.show({ title: `${result.issue.key} closed`, variant: "success" });
    } catch {
      setCloseError("error");
      setConfirming(false);
    } finally {
      setClosing(false);
    }
  };

  return (
    <ErrorBoundary fallback={(error) => <IssueDetailError kind={issueErrorKind(error)} />}>
      <Suspense fallback={<IssueDetailSkeleton />}>
        <Show when={issue()?.error} fallback={<Show when={current()}>
          {(row) => (
            <article class="issue-detail" aria-labelledby="issue-detail-heading">
              <Title>{row().key} · Issues · Myelin</Title>
              <nav aria-label="Breadcrumb" class="issues-breadcrumb">
                <A href="/issues">Issues</A>
                <span aria-hidden="true">/</span>
                <span aria-current="page">{row().key}</span>
              </nav>

              <header class="issue-detail-header">
                <div>
                  <code>{row().key}</code>
                  <h1 id="issue-detail-heading">{row().title}</h1>
                </div>
                <StatusPill kind="issue-state" category={row().state_category} label={row().state} />
              </header>

              <dl class="issue-detail-meta">
                <div><dt>Created</dt><dd><time datetime={row().created_at}>{issueTimestamp(row().created_at)}</time></dd></div>
                <div><dt>Updated</dt><dd><time datetime={row().updated_at}>{issueTimestamp(row().updated_at)}</time></dd></div>
              </dl>

              <Show when={!isClosedCategory(row().state_category)}>
                <div class="issue-detail-actions">
                  <button type="button" class="issues-button issues-button-danger" onClick={() => setConfirming(true)}>
                    <Icon name="close" /> Close issue
                  </button>
                  <Show when={closeError()}>
                    {(kind) => <p role="alert" class="issues-field-error">{closeErrorText(kind())}</p>}
                  </Show>
                </div>
              </Show>

              <ConfirmDialog
                open={confirming()}
                onCancel={() => !closing() && setConfirming(false)}
                onConfirm={() => void close()}
                title={`Close ${row().key}?`}
                description="This moves the issue to Done. The current web floor cannot reopen it."
                confirmLabel={closing() ? "Closing…" : "Close issue"}
                cancelLabel="Cancel"
                variant="destructive"
              />
            </article>
          )}
        </Show>}>
          {(kind) => <IssueDetailError kind={kind()} />}
        </Show>
      </Suspense>
    </ErrorBoundary>
  );
}

function closeErrorText(kind: IssueErrorKind): string {
  if (kind === "not-found") return "This issue is not available to you.";
  return "We couldn't confirm whether this issue was closed. Refresh and check its current state before retrying.";
}

function IssueDetailError(props: { kind: IssueErrorKind }) {
  const unavailable = () => props.kind === "unavailable";
  return (
    <section role={unavailable() ? "alert" : "note"} class="issues-error" data-testid="issue-detail-error">
      <Icon name={unavailable() ? "check-fail" : "issue"} title={unavailable() ? "Error" : "Not available"} />
      <div>
        <h1>{unavailable() ? "Issue authorization is catching up" : props.kind === "not-found" ? "This issue is not available to you" : "We couldn't load this issue"}</h1>
        <p>{unavailable() ? "Try again when the authorization projection is current." : props.kind === "not-found" ? "It may not exist, or you may not have access." : "Something went wrong on our side."}</p>
        <A href="/issues" class="issues-button issues-button-secondary">Back to issues</A>
      </div>
    </section>
  );
}

function IssueDetailSkeleton() {
  return (
    <Skeleton label="Loading issue…" rows={3} rowHeight="2rem" data-testid="issue-detail-loading">
      <SkeletonBlock width="8rem" height="0.8rem" />
      <SkeletonBlock width="70%" height="2rem" />
      <SkeletonBlock width="18rem" height="1rem" />
    </Skeleton>
  );
}
