import { createEffect, createSignal, on, Show } from "solid-js";
import { revalidate, useAction, useNavigate } from "@solidjs/router";
import { ConfirmDialog, Icon } from "@myelin/design-system";
import { ciRunMutate, getCiRun, getCiRuns } from "~/lib/api";
import {
  ciRunMutationMessage,
  retainsCiRunKey,
  type CiRunOp,
} from "~/lib/ci-run-control";
import type { CiRunControlsVM } from "~/lib/ci-read-response";

/** One idempotency key per user intent: kept across transport failures, dropped once the Edge decided. */
export function createCiRunControls(run: () => string) {
  const mutate = useAction(ciRunMutate);
  const navigate = useNavigate();
  const keys: Partial<Record<CiRunOp, string>> = {};
  const [pending, setPending] = createSignal<CiRunOp | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [confirmingCancel, setConfirmingCancel] = createSignal(false);

  createEffect(on(run, () => {
    delete keys.retry;
    delete keys.cancel;
    setPending(null);
    setError(null);
    setConfirmingCancel(false);
  }, { defer: true }));

  const submit = async (op: CiRunOp) => {
    if (pending()) return;
    const target = run();
    const key = keys[op] ??= crypto.randomUUID();
    setPending(op);
    setError(null);
    let navigated = false;
    try {
      const result = await mutate({ op, run: target, clientNonce: key });
      if (target !== run()) return;
      setConfirmingCancel(false);
      if (!result.ok) {
        if (!retainsCiRunKey(result.error)) delete keys[op];
        setError(ciRunMutationMessage(op, result.error));
        return;
      }
      delete keys[op];
      if (result.op === "retry") {
        navigated = true;
        void revalidate(getCiRuns.key);
        navigate(`/ci/runs/${result.receipt.run_id}`);
        return;
      }
      await revalidate([getCiRun.keyFor(target), getCiRuns.key]);
      document.getElementById("ci-run-heading")?.focus();
    } catch {
      if (target === run()) {
        setConfirmingCancel(false);
        setError(ciRunMutationMessage(op, "error"));
      }
    } finally {
      if (!navigated && target === run()) setPending(null);
    }
  };

  return {
    pending,
    error,
    confirmingCancel,
    askCancel: () => { if (!pending()) { setError(null); setConfirmingCancel(true); } },
    dismissCancel: () => { if (!pending()) setConfirmingCancel(false); },
    retry: () => submit("retry"),
    cancel: () => submit("cancel"),
  };
}

export type CiRunControlsState = ReturnType<typeof createCiRunControls>;

export function CiRunControls(props: { controls: CiRunControlsVM; state: CiRunControlsState }) {
  const state = () => props.state;
  return (
    <>
      <Show when={props.controls.retry}>
        <button
          type="button"
          class="ci-run-action"
          data-testid="ci-run-retry"
          disabled={state().pending() !== null}
          aria-busy={state().pending() === "retry"}
          onClick={() => void state().retry()}
        >
          <Icon name={state().pending() === "retry" ? "cycle" : "rerun"} />
          {state().pending() === "retry" ? "Re-running…" : "Re-run"}
        </button>
      </Show>
      <Show when={props.controls.cancel}>
        <button
          type="button"
          class="ci-run-action"
          data-testid="ci-run-cancel"
          disabled={state().pending() !== null}
          aria-busy={state().pending() === "cancel"}
          onClick={() => state().askCancel()}
        >
          <Icon name={state().pending() === "cancel" ? "cycle" : "close"} />
          {state().pending() === "cancel" ? "Cancelling…" : "Cancel run"}
        </button>
        <ConfirmDialog
          open={state().confirmingCancel()}
          onCancel={() => state().dismissCancel()}
          onConfirm={() => void state().cancel()}
          title="Cancel this run?"
          description="Running jobs stop now and the run ends as cancelled. You can re-run it later."
          confirmLabel={state().pending() === "cancel" ? "Cancelling…" : "Cancel run"}
          cancelLabel="Keep running"
          variant="destructive"
        />
      </Show>
      <Show when={state().error()}>
        {(message) => (
          <p class="ci-run-action-error" role="alert" data-testid="ci-run-action-error">
            {message()}
          </p>
        )}
      </Show>
    </>
  );
}
