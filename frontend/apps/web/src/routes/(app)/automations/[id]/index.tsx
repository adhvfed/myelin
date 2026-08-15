import { ErrorBoundary, For, Show, Suspense, createSignal, onMount } from "solid-js";
import { Title } from "@solidjs/meta";
import { A, createAsync, revalidate, useAction, useParams, useSearchParams } from "@solidjs/router";
import { ConfirmDialog, Icon, Skeleton, useToast } from "@myelin/design-system";
import { AutomationErrorState, automationErrorKind } from "~/components/AutomationErrorState";
import { AutomationStateLabel } from "~/components/AutomationStateLabel";
import {
  changeAutomationLifecycle,
  getAutomation,
  getAutomationFirings,
  type AutomationLifecycleAction,
} from "~/lib/automation-api";
import {
  AUTOMATION_PAGE_LIMIT,
  type AutomationFiringVM,
  type AutomationVM,
} from "~/lib/automation-response";

export default function AutomationDetail() {
  const params = useParams();
  const [search] = useSearchParams();
  const mutate = useAction(changeAutomationLifecycle);
  const toast = useToast();
  const [replacement, setReplacement] = createSignal<AutomationVM | null>(null);
  const [pendingAction, setPendingAction] = createSignal<AutomationLifecycleAction | null>(null);
  const [confirmingDisable, setConfirmingDisable] = createSignal(false);
  const [mutationError, setMutationError] = createSignal(false);
  const [interactive, setInteractive] = createSignal(false);
  onMount(() => setInteractive(true));
  const automationId = () => params.id ?? "";
  const cursor = () => search.cursor as string | undefined;
  const detail = createAsync(async () => {
    const firingInput = cursor() === undefined
      ? { limit: AUTOMATION_PAGE_LIMIT }
      : { limit: AUTOMATION_PAGE_LIMIT, cursor: cursor() };
    const [automation, firings] = await Promise.all([
      getAutomation(automationId()),
      getAutomationFirings(automationId(), firingInput),
    ]);
    return { automation, firings };
  }, { deferStream: true });
  const current = () => replacement() ?? detail()?.automation;

  const applyLifecycle = async (action: AutomationLifecycleAction) => {
    if (pendingAction()) return;
    setPendingAction(action);
    setMutationError(false);
    try {
      const result = await mutate({ automationId: automationId(), action });
      if (!result.ok) {
        setMutationError(true);
        return;
      }
      setReplacement(result.receipt.trigger);
      void revalidate("automations");
      void revalidate("automation-detail");
      toast.show({
        title: `${action === "disable" ? "Disabled" : action === "pause" ? "Paused" : "Resumed"} automation`,
        variant: "success",
      });
    } catch {
      setMutationError(true);
    } finally {
      setConfirmingDisable(false);
      setPendingAction(null);
    }
  };

  return (
    <section aria-labelledby="automation-heading" class="automation-screen">
      <Title>Automation · Myelin</Title>
      <nav aria-label="Breadcrumb" class="automation-breadcrumb">
        <A href="/automations">Automations</A>
        <span aria-hidden="true">/</span>
        <span aria-current="page">{automationId().slice(0, 8)}</span>
      </nav>
      <ErrorBoundary
        fallback={(error, reset) => (
          <AutomationErrorState kind={automationErrorKind(error)} onRetry={reset} />
        )}
      >
        <Suspense fallback={<Skeleton label="Loading automation…" rows={6} rowHeight="3.5rem" />}>
          <Show when={detail()} keyed>
            {(view) => (
              <>
                <header class="automation-detail-header">
                  <div>
                    <p class="automation-eyebrow">{view.automation.event_type}</p>
                    <h1 id="automation-heading">{view.automation.task}</h1>
                  </div>
                  <Show when={current()}>{(item) => <AutomationStateLabel state={item().state} />}</Show>
                </header>

                <Show when={current()}>
                  {(item) => (
                    <>
                      <dl class="automation-facts">
                        <div><dt>Run as</dt><dd><code>agent:{item().run_as_agent_id}</code></dd></div>
                        <div><dt>Budget per run</dt><dd>{item().budget_minor_units.toLocaleString("en-US")} minor-units</dd></div>
                        <div><dt>Firings</dt><dd>{item().firings_used} of {item().max_firings}</dd></div>
                        <div><dt>Human approval</dt><dd>{item().require_human_approval ? "Required" : "Not required"}</dd></div>
                        <div><dt>Personal data</dt><dd>{item().require_no_personal_data ? "Refused" : "Allowed by policy"}</dd></div>
                        <div><dt>Causal depth</dt><dd>{item().max_causal_depth}</dd></div>
                      </dl>
                      <Show when={item().last_evaluation_error}>
                        {(diagnostic) => (
                          <section
                            class="automation-evaluation-error"
                            role="alert"
                            aria-labelledby="automation-evaluation-error-heading"
                          >
                            <Icon name="check-fail" title="Rule evaluation failed" />
                            <div>
                              <h2 id="automation-evaluation-error-heading">
                                Latest event could not be evaluated
                              </h2>
                              <p>{diagnostic().detail}</p>
                              <p class="automation-evaluation-error-meta">
                                Event <code>{diagnostic().event_id}</code> · {formatAutomationTime(diagnostic().event_recorded_at)}
                              </p>
                            </div>
                          </section>
                        )}
                      </Show>
                      <Show when={item().condition}>
                        {(condition) => (
                          <section class="automation-condition" aria-labelledby="automation-condition-heading">
                            <h2 id="automation-condition-heading">Event condition</h2>
                            <code>{condition()}</code>
                          </section>
                        )}
                      </Show>
                      <div class="automation-actions">
                        <Show when={item().state === "active"}>
                          <button
                            type="button"
                            class="automation-button automation-button-secondary"
                            disabled={!interactive() || pendingAction() !== null}
                            onClick={() => void applyLifecycle("pause")}
                          >
                            <Icon name="check-pending" /> {pendingAction() === "pause" ? "Pausing…" : "Pause"}
                          </button>
                        </Show>
                        <Show when={item().state === "paused"}>
                          <button
                            type="button"
                            class="automation-button automation-button-primary"
                            disabled={!interactive() || pendingAction() !== null}
                            onClick={() => void applyLifecycle("resume")}
                          >
                            <Icon name="cycle" /> {pendingAction() === "resume" ? "Resuming…" : "Resume"}
                          </button>
                        </Show>
                        <Show when={item().state !== "disabled"}>
                          <button
                            type="button"
                            class="automation-button automation-button-danger"
                            disabled={!interactive() || pendingAction() !== null}
                            onClick={() => setConfirmingDisable(true)}
                          >
                            <Icon name="close" /> Disable
                          </button>
                        </Show>
                        <Show when={mutationError()}>
                          <p role="alert" class="automation-mutation-error">
                            The durable state could not be confirmed. Refresh before retrying.
                          </p>
                        </Show>
                      </div>
                      <ConfirmDialog
                        open={confirmingDisable()}
                        onCancel={() => !pendingAction() && setConfirmingDisable(false)}
                        onConfirm={() => void applyLifecycle("disable")}
                        title="Disable this automation?"
                        description="This is irreversible. Unstarted firings are canceled and future matching events remain quiet."
                        confirmLabel="Disable automation"
                        cancelLabel="Keep automation"
                        variant="destructive"
                      />
                    </>
                  )}
                </Show>

                <section class="automation-history" aria-labelledby="automation-history-heading">
                  <div class="automation-section-heading">
                    <div>
                      <p class="automation-eyebrow">Durable execution record</p>
                      <h2 id="automation-history-heading">Firing history</h2>
                    </div>
                    <Show when={search.cursor}>
                      <A href={`/automations/${automationId()}`} class="automation-button automation-button-secondary">
                        <Icon name="cycle" /> Latest
                      </A>
                    </Show>
                  </div>
                  <Show
                    when={view.firings.items.length > 0}
                    fallback={<p class="automation-history-empty">No matching events have reserved work yet.</p>}
                  >
                    <ul class="automation-firing-list">
                      <For each={view.firings.items}>{(firing) => <FiringRow automationId={automationId()} firing={firing} />}</For>
                    </ul>
                    <Show when={view.firings.page.next_cursor}>
                      {(next) => (
                        <nav aria-label="Firing history pages" class="automation-pagination">
                          <A
                            href={`/automations/${automationId()}?cursor=${encodeURIComponent(next())}`}
                            class="automation-button automation-button-secondary"
                          >
                            Older firings <Icon name="chevron" />
                          </A>
                        </nav>
                      )}
                    </Show>
                  </Show>
                </section>
              </>
            )}
          </Show>
        </Suspense>
      </ErrorBoundary>
    </section>
  );
}

function FiringRow(props: { automationId: string; firing: AutomationFiringVM }) {
  const resultAvailable = () => props.firing.result_state === "available" && props.firing.run_id !== null;
  return (
    <li class="automation-firing-row">
      <span class="automation-status" data-state={props.firing.outcome ?? props.firing.state}>
        <Icon name={props.firing.outcome === "succeeded" ? "check-pass" : props.firing.outcome ? "check-fail" : "check-pending"} />
        {firingLabel(props.firing)}
      </span>
      <span class="automation-firing-main">
        <code>{props.firing.event_id}</code>
        <span>{firingDetail(props.firing)}</span>
      </span>
      <span class="automation-firing-meta">
        <time datetime={props.firing.created_at}>{formatAutomationTime(props.firing.created_at)}</time>
        <Show when={resultAvailable()}>
          <A href={`/automations/${props.automationId}/runs/${props.firing.run_id}`}>Read result</A>
        </Show>
        <Show when={props.firing.result_state === "erased"}>
          <span>Result erased</span>
        </Show>
      </span>
    </li>
  );
}

function firingDetail(firing: AutomationFiringVM): string {
  if (firing.terminal_reason) return firing.terminal_reason;
  if (firing.approval) return `${firing.approval.decision} by ${firing.approval.decided_by}`;
  if (firing.state === "awaiting_approval") return "Waiting for its owner’s decision";
  return "No human decision required";
}

function firingLabel(firing: AutomationFiringVM): string {
  if (firing.outcome) return firing.outcome.replaceAll("_", " ");
  if (firing.terminal_reason) return "could not start";
  if (firing.state === "terminal" && firing.run_id === null) return "canceled before start";
  return firing.state.replaceAll("_", " ");
}

function formatAutomationTime(value: string): string {
  return new Date(value).toISOString().replace("T", " ").slice(0, 16) + " UTC";
}
