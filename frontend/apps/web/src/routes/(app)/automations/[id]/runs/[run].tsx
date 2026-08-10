import { ErrorBoundary, Show, Suspense, createSignal, onMount } from "solid-js";
import { Title } from "@solidjs/meta";
import { A, createAsync, useAction, useParams } from "@solidjs/router";
import { ConfirmDialog, Icon, Skeleton, useToast } from "@myelin/design-system";
import { AutomationErrorState, automationErrorKind } from "~/components/AutomationErrorState";
import { eraseAutomationResult, getAutomationResult } from "~/lib/automation-api";
import type { AutomationErasureVM } from "~/lib/automation-response";

export default function AutomationResult() {
  const params = useParams();
  const erase = useAction(eraseAutomationResult);
  const toast = useToast();
  const automationId = () => params.id ?? "";
  const runId = () => params.run ?? "";
  const result = createAsync(() => getAutomationResult(automationId(), runId()), { deferStream: true });
  const [erasure, setErasure] = createSignal<AutomationErasureVM | null>(null);
  const [confirming, setConfirming] = createSignal(false);
  const [erasing, setErasing] = createSignal(false);
  const [eraseError, setEraseError] = createSignal(false);
  const [interactive, setInteractive] = createSignal(false);
  onMount(() => setInteractive(true));

  const eraseResult = async () => {
    if (erasing()) return;
    setErasing(true);
    setEraseError(false);
    try {
      const response = await erase({ automationId: automationId(), runId: runId() });
      if (!response.ok) {
        setEraseError(true);
        setConfirming(false);
        return;
      }
      setErasure(response.receipt);
      setConfirming(false);
      toast.show({ title: "Erased agent result", variant: "success" });
    } catch {
      setEraseError(true);
      setConfirming(false);
    } finally {
      setErasing(false);
    }
  };

  return (
    <section aria-labelledby="automation-result-heading" class="automation-screen">
      <Title>Agent result · Automations · Myelin</Title>
      <nav aria-label="Breadcrumb" class="automation-breadcrumb">
        <A href="/automations">Automations</A>
        <span aria-hidden="true">/</span>
        <A href={`/automations/${automationId()}`}>{automationId().slice(0, 8)}</A>
        <span aria-hidden="true">/</span>
        <span aria-current="page">run {runId().slice(0, 8)}</span>
      </nav>
      <ErrorBoundary
        fallback={(error, reset) => (
          <AutomationErrorState kind={automationErrorKind(error)} onRetry={reset} />
        )}
      >
        <Suspense fallback={<Skeleton label="Loading agent result…" rows={5} rowHeight="3rem" />}>
          <Show when={result()} keyed>
            {(view) => (
              <Show
                when={erasure()}
                fallback={
                  <article class="automation-result">
                    <header class="automation-detail-header">
                      <div>
                        <p class="automation-eyebrow">Immutable hosted-agent work product</p>
                        <h1 id="automation-result-heading">Agent result</h1>
                      </div>
                      <span class="automation-status" data-state="succeeded">
                        <Icon name="check-pass" /> Complete
                      </span>
                    </header>
                    <pre data-testid="automation-result-answer">{view.answer}</pre>
                    <dl class="automation-facts">
                      <div><dt>Agent</dt><dd><code>{view.agent_principal}</code></dd></div>
                      <div><dt>Charge</dt><dd>{view.charged_micro.toLocaleString("en-US")} micro-units</dd></div>
                      <div><dt>Recorded</dt><dd><time datetime={view.recorded_at}>{new Date(view.recorded_at).toISOString()}</time></dd></div>
                      <div><dt>Knowledge trace</dt><dd><code>{view.trace_ref}</code></dd></div>
                    </dl>
                    <div class="automation-result-erasure">
                      <div>
                        <h2>Erase this work product</h2>
                        <p>Erasure removes the available result and durably blocks a worker retry from recreating it.</p>
                      </div>
                      <button
                        type="button"
                        class="automation-button automation-button-danger"
                        disabled={!interactive() || erasing()}
                        onClick={() => setConfirming(true)}
                      >
                        <Icon name="close" /> Erase result
                      </button>
                    </div>
                    <Show when={eraseError()}>
                      <p role="alert" class="automation-mutation-error">
                        Erasure could not be confirmed. Refresh before trying again.
                      </p>
                    </Show>
                    <ConfirmDialog
                      open={confirming()}
                      onCancel={() => !erasing() && setConfirming(false)}
                      onConfirm={() => void eraseResult()}
                      title="Erase this agent result?"
                      description="The result will no longer be readable and the same run cannot recreate it. This cannot be undone."
                      confirmLabel={erasing() ? "Erasing…" : "Erase result"}
                      cancelLabel="Keep result"
                      variant="destructive"
                    />
                  </article>
                }
              >
                {(receipt) => (
                  <div role="status" class="automation-empty" data-testid="automation-result-erased">
                    <Icon name="check-pass" size={28} title="Erased" />
                    <h1 id="automation-result-heading">Agent result erased</h1>
                    <p>
                      No result remains available for run <code>{receipt().run_id}</code>. Its durable
                      tombstone prevents recreation by a retry.
                    </p>
                    <A href={`/automations/${automationId()}`} class="automation-button automation-button-secondary">
                      Back to firing history
                    </A>
                  </div>
                )}
              </Show>
            )}
          </Show>
        </Suspense>
      </ErrorBoundary>
    </section>
  );
}
