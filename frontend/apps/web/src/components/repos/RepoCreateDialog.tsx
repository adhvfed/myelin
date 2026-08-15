import { Dialog, Icon } from "@myelin/design-system";
import { useAction } from "@solidjs/router";
import { Show, createEffect, createSignal } from "solid-js";
import { createRepo, type RepoCreateError } from "~/lib/api";
import { repositorySlugError } from "~/lib/repo-create";

export interface RepoCreateDialogProps {
  open: boolean;
  onClose: () => void;
  onCreated: (slug: string) => void;
}

function createError(kind: RepoCreateError): string {
  switch (kind) {
    case "bad-input":
      return "Check the repository name and try again.";
    case "exists":
      return "A repository with this name already exists, but you cannot write to it.";
    case "forbidden":
      return "You do not have permission to create repositories.";
    default:
      return "We couldn't confirm whether the repository was created. Retrying this unchanged name is safe.";
  }
}

export function RepoCreateDialog(props: RepoCreateDialogProps) {
  const create = useAction(createRepo);
  const [slug, setSlug] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const [submitting, setSubmitting] = createSignal(false);
  let input: HTMLInputElement | undefined;

  createEffect(() => {
    if (!props.open) return;
    setSlug("");
    setError(null);
    setSubmitting(false);
  });

  const close = () => {
    if (!submitting()) props.onClose();
  };

  const submit = async (event: SubmitEvent) => {
    event.preventDefault();
    const validation = repositorySlugError(slug());
    if (validation) {
      setError(validation);
      input?.focus();
      return;
    }
    setSubmitting(true);
    setError(null);
    try {
      const result = await create(slug());
      if (!result.ok) {
        setError(createError(result.error));
        input?.focus();
        return;
      }
      props.onCreated(result.receipt.slug);
      props.onClose();
    } catch {
      setError(createError("error"));
      input?.focus();
    } finally {
      setSubmitting(false);
    }
  };

  const buttonStyle = {
    display: "inline-flex",
    "align-items": "center",
    gap: "var(--space-1)",
    padding: "var(--space-2) var(--space-3)",
    border: "var(--hairline) solid var(--border-strong)",
    "border-radius": "var(--radius-1)",
    cursor: "pointer",
  } as const;

  return (
    <Dialog
      open={props.open}
      onClose={close}
      title="New repository"
      description="Create an empty repository in this tenant. You can push its first branch next."
      size="md"
      dismissable={!submitting()}
      initialFocus={() => input}
      footer={
        <>
          <button type="button" onClick={close} disabled={submitting()} style={{ ...buttonStyle, background: "var(--surface)" }}>
            Cancel
          </button>
          <button type="submit" form="repo-create-form" disabled={submitting()} style={{ ...buttonStyle, background: "var(--accent)", color: "var(--on-accent)", border: "none" }}>
            <Icon name={submitting() ? "cycle" : "repo"} />
            {submitting() ? "Creating…" : "Create repository"}
          </button>
        </>
      }
    >
      <form id="repo-create-form" onSubmit={submit} style={{ display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
        <label style={{ display: "flex", "flex-direction": "column", gap: "var(--space-1)" }}>
          Name or namespace/name
          <input
            ref={input}
            name="slug"
            value={slug()}
            maxLength={255}
            autocomplete="off"
            disabled={submitting()}
            aria-invalid={Boolean(error())}
            aria-describedby={error() ? "repo-create-error" : "repo-create-hint"}
            onInput={(event) => {
              setSlug(event.currentTarget.value);
              if (error()) setError(null);
            }}
          />
        </label>
        <p id="repo-create-hint" style={{ margin: "0", color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>
          Letters, numbers, dots, dashes, and underscores are supported. Use slashes for namespaces.
        </p>
        <Show when={error()}>
          {(message) => <p id="repo-create-error" role="alert" style={{ margin: "0", color: "var(--danger)" }}>{message()}</p>}
        </Show>
      </form>
    </Dialog>
  );
}
