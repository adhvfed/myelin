import { Dialog, Icon } from "@myelin/design-system";
import { useAction } from "@solidjs/router";
import { createEffect, createSignal, Show } from "solid-js";
import {
  issuesMutate,
  type IssueCreateReceipt,
  type IssueErrorKind,
} from "~/lib/api";
import { issueTitleError } from "~/lib/issue-view";

export interface IssueCreateDialogProps {
  open: boolean;
  onClose: () => void;
  onAccepted: (receipt: IssueCreateReceipt) => void;
}

function actionError(kind: IssueErrorKind): string {
  switch (kind) {
    case "bad-input":
      return "Check the title and try again.";
    case "not-found":
      return "Issue creation isn't available to you.";
    case "unavailable":
      return "Issue authorization is temporarily unavailable. Try again shortly.";
    case "configuration":
      return "Issue creation isn't configured on this deployment.";
    default:
      return "We couldn't create the issue. Nothing was submitted twice; try again.";
  }
}

export function IssueCreateDialog(props: IssueCreateDialogProps) {
  const mutate = useAction(issuesMutate);
  const [title, setTitle] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const [submitting, setSubmitting] = createSignal(false);
  let titleInput: HTMLInputElement | undefined;

  createEffect(() => {
    if (!props.open) return;
    setTitle("");
    setError(null);
    setSubmitting(false);
  });

  const close = () => {
    if (!submitting()) props.onClose();
  };

  const submit = async (event: SubmitEvent) => {
    event.preventDefault();
    const validation = issueTitleError(title());
    if (validation) {
      setError(validation);
      titleInput?.focus();
      return;
    }
    setSubmitting(true);
    setError(null);
    try {
      const result = await mutate({ op: "create", title: title() });
      if (!result.ok) {
        setError(actionError(result.error));
        titleInput?.focus();
        return;
      }
      if (result.op !== "create") {
        setError("We couldn't confirm the create response. Try again.");
        return;
      }
      props.onAccepted(result.receipt);
      props.onClose();
    } catch {
      setError("We couldn't create the issue. Try again.");
      titleInput?.focus();
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog
      open={props.open}
      onClose={close}
      title="New issue"
      description="Capture work in the canonical Myelin dogfood project. Access activates safely after acceptance."
      size="md"
      dismissable={!submitting()}
      initialFocus={() => titleInput}
      footer={
        <>
          <button
            type="button"
            onClick={close}
            disabled={submitting()}
            class="issues-button issues-button-secondary"
          >
            Cancel
          </button>
          <button
            type="submit"
            form="issue-create-form"
            disabled={submitting()}
            class="issues-button issues-button-primary"
          >
            <Show when={submitting()} fallback={<Icon name="issue" />}>
              <Icon name="cycle" />
            </Show>
            {submitting() ? "Creating…" : "Create issue"}
          </button>
        </>
      }
    >
      <form id="issue-create-form" onSubmit={submit}>
        <label class="issues-field-label">
          Title
          <input
            ref={titleInput}
            id="issue-title"
            name="title"
            type="text"
            value={title()}
            onInput={(event) => {
              setTitle(event.currentTarget.value);
              if (error()) setError(null);
            }}
            aria-invalid={Boolean(error())}
            aria-describedby={error() ? "issue-title-error" : "issue-title-hint"}
            disabled={submitting()}
            autocomplete="off"
            class="issues-text-input"
          />
        </label>
        <p id="issue-title-hint" class="issues-field-hint">
          Up to 512 UTF-8 bytes. Project, type, and key prefix are set by this deployment.
        </p>
        <Show when={error()}>
          {(message) => <p id="issue-title-error" role="alert" class="issues-field-error">{message()}</p>}
        </Show>
      </form>
    </Dialog>
  );
}
