import { Dialog, Icon } from "@myelin/design-system";
import { useAction } from "@solidjs/router";
import { createEffect, createSignal, Show } from "solid-js";
import { createProjectCatalogue } from "~/components/projects/project-catalogue";
import {
  issuesMutate,
  type IssueCreateReceipt,
  type IssueErrorKind,
} from "~/lib/issue-api";
import { issueTitleError } from "~/lib/issue-view";
import { createProject, type ProjectErrorKind } from "~/lib/project-api";
import { projectNameError, projectPrefixError } from "~/lib/project-contract";
import { IssueDraftFields, ProjectSetupFields } from "./IssueCreateFields";

export interface IssueCreateDialogProps {
  open: boolean;
  onClose: () => void;
  onAccepted: (receipt: IssueCreateReceipt) => void;
}

function issueActionError(kind: IssueErrorKind): string {
  switch (kind) {
    case "bad-input":
      return "Check the title and project, then try again.";
    case "not-found":
      return "That project is no longer available to you.";
    default:
      return "We couldn't confirm whether the issue was created. Retrying this unchanged draft is safe.";
  }
}

function projectActionError(kind: ProjectErrorKind): string {
  switch (kind) {
    case "bad-input":
      return "Check the project name and issue key, then try again.";
    case "conflict":
      return "That issue key is already used by another project.";
    case "not-found":
      return "Project creation isn't available to you.";
    default:
      return "We couldn't confirm whether the project was created. Retrying this unchanged draft is safe.";
  }
}

export function IssueCreateDialog(props: IssueCreateDialogProps) {
  const mutateIssue = useAction(issuesMutate);
  const mutateProject = useAction(createProject);
  const catalogue = createProjectCatalogue();
  const [title, setTitle] = createSignal("");
  const [projectName, setProjectName] = createSignal("");
  const [projectPrefix, setProjectPrefix] = createSignal("");
  const [issueClientNonce, setIssueClientNonce] = createSignal(crypto.randomUUID());
  const [projectClientNonce, setProjectClientNonce] = createSignal(crypto.randomUUID());
  const [error, setError] = createSignal<string | null>(null);
  const [submitting, setSubmitting] = createSignal<"project" | "issue" | null>(null);
  const [creatingProject, setCreatingProject] = createSignal(false);
  let titleInput: HTMLInputElement | undefined;
  let projectNameInput: HTMLInputElement | undefined;
  let projectPrefixInput: HTMLInputElement | undefined;
  let focusStep = "";

  createEffect(() => {
    if (!props.open) return;
    setTitle("");
    setProjectName("");
    setProjectPrefix("");
    setIssueClientNonce(crypto.randomUUID());
    setProjectClientNonce(crypto.randomUUID());
    setError(null);
    setSubmitting(null);
    setCreatingProject(false);
  });

  const showProjectSetup = () => catalogue.empty() || creatingProject();

  createEffect(() => {
    if (!props.open) {
      focusStep = "";
      return;
    }
    const step = catalogue.loading() || catalogue.unavailable()
      ? "waiting"
      : showProjectSetup() ? "project" : "issue";
    if (step === focusStep) return;
    focusStep = step;
    queueMicrotask(() => {
      if (step === "project") projectNameInput?.focus();
      if (step === "issue") titleInput?.focus();
    });
  });

  const busy = () => submitting() !== null;
  const close = () => {
    if (!busy()) props.onClose();
  };
  const backOrClose = () => {
    if (busy()) return;
    if (creatingProject() && !catalogue.empty()) {
      setCreatingProject(false);
      setError(null);
    } else {
      props.onClose();
    }
  };
  const clearError = () => {
    if (error()) setError(null);
  };

  const submitIssue = async (event: SubmitEvent) => {
    event.preventDefault();
    const project = catalogue.selected();
    const validation = issueTitleError(title());
    if (validation || !project) {
      setError(validation ?? "Choose a project for this issue.");
      (validation ? titleInput : undefined)?.focus();
      return;
    }
    setSubmitting("issue");
    setError(null);
    try {
      const result = await mutateIssue({
        op: "create",
        projectId: project.id,
        title: title(),
        clientNonce: issueClientNonce(),
      });
      if (!result.ok) {
        setError(issueActionError(result.error));
        titleInput?.focus();
        return;
      }
      if (result.op !== "create") {
        setError(issueActionError("error"));
        titleInput?.focus();
        return;
      }
      props.onAccepted(result.receipt);
      props.onClose();
    } catch {
      setError(issueActionError("error"));
      titleInput?.focus();
    } finally {
      setSubmitting(null);
    }
  };

  const submitProject = async (event: SubmitEvent) => {
    event.preventDefault();
    const nameError = projectNameError(projectName());
    const prefixError = projectPrefixError(projectPrefix());
    if (nameError || prefixError) {
      setError(nameError ?? prefixError);
      (nameError ? projectNameInput : projectPrefixInput)?.focus();
      return;
    }
    setSubmitting("project");
    setError(null);
    try {
      const result = await mutateProject({
        name: projectName(),
        issuePrefix: projectPrefix(),
        clientNonce: projectClientNonce(),
      });
      if (!result.ok) {
        setError(projectActionError(result.error));
        (result.error === "conflict" ? projectPrefixInput : projectNameInput)?.focus();
        return;
      }
      catalogue.add(result.receipt.project);
      setIssueClientNonce(crypto.randomUUID());
      setCreatingProject(false);
      setError(null);
    } catch {
      setError(projectActionError("error"));
      projectNameInput?.focus();
    } finally {
      setSubmitting(null);
    }
  };

  const formId = () => showProjectSetup() ? "project-create-form" : "issue-create-form";

  return (
    <Dialog
      open={props.open}
      onClose={close}
      title={catalogue.empty() ? "Set up issue tracking" : creatingProject() ? "New project" : "New issue"}
      description={showProjectSetup()
        ? catalogue.empty()
          ? "Issues live in projects. Create your first one, then capture its first piece of work."
          : "Create another project, then capture work in it."
        : "Capture work in a project you can access. Access activates safely after acceptance."}
      size="md"
      dismissable={!busy()}
      initialFocus={() => showProjectSetup() ? projectNameInput : titleInput}
      footer={
        <>
          <button type="button" onClick={backOrClose} disabled={busy()} class="issues-button issues-button-secondary">
            {creatingProject() && !catalogue.empty() ? "Back" : "Cancel"}
          </button>
          <Show when={catalogue.unavailable()}>
            <button
              type="button"
              onClick={() => void catalogue.retry()}
              class="issues-button issues-button-primary"
            >
              Try again
            </button>
          </Show>
          <Show when={!catalogue.loading() && !catalogue.unavailable()}>
            <button
              type="submit"
              form={formId()}
              disabled={busy()}
              class="issues-button issues-button-primary"
            >
              <Show when={busy()} fallback={<Icon name="issue" />}><Icon name="cycle" /></Show>
              {submitting() === "project"
                ? "Creating project…"
                : submitting() === "issue"
                  ? "Creating issue…"
                  : showProjectSetup() ? "Create project" : "Create issue"}
            </button>
          </Show>
        </>
      }
    >
      <Show when={catalogue.loading()}>
        <p role="status" class="issues-dialog-status"><Icon name="cycle" /> Loading projects…</p>
      </Show>
      <Show when={catalogue.unavailable()}>
        <p role="alert" class="issues-field-error">
          We couldn't load your projects. No issue destination has been guessed.
        </p>
      </Show>
      <Show when={!catalogue.loading() && !catalogue.unavailable() && showProjectSetup()}>
        <form id="project-create-form" onSubmit={submitProject} class="issues-project-setup">
          <ProjectSetupFields
            name={projectName()}
            prefix={projectPrefix()}
            error={error()}
            disabled={busy()}
            nameRef={(element) => { projectNameInput = element; }}
            prefixRef={(element) => { projectPrefixInput = element; }}
            onName={(value) => { setProjectName(value); setProjectClientNonce(crypto.randomUUID()); clearError(); }}
            onPrefix={(value) => { setProjectPrefix(value); setProjectClientNonce(crypto.randomUUID()); clearError(); }}
          />
        </form>
      </Show>
      <Show when={!catalogue.loading() && !catalogue.unavailable() && !showProjectSetup()}>
        <form id="issue-create-form" onSubmit={submitIssue} class="issues-project-setup">
          <IssueDraftFields
            projects={catalogue.projects()}
            selectedId={catalogue.selectedId()}
            selectedPrefix={catalogue.selected()?.issue_prefix}
            title={title()}
            error={error()}
            disabled={busy()}
            hasMore={catalogue.nextCursor() !== null}
            loadingMore={catalogue.loadingMore()}
            loadMoreError={catalogue.loadMoreError()}
            titleRef={(element) => { titleInput = element; }}
            onProject={(value) => { catalogue.select(value); setIssueClientNonce(crypto.randomUUID()); clearError(); }}
            onTitle={(value) => { setTitle(value); setIssueClientNonce(crypto.randomUUID()); clearError(); }}
            onLoadMore={() => void catalogue.loadMore()}
            onCreateProject={() => {
              setProjectName("");
              setProjectPrefix("");
              setProjectClientNonce(crypto.randomUUID());
              setError(null);
              setCreatingProject(true);
            }}
          />
        </form>
      </Show>
    </Dialog>
  );
}
