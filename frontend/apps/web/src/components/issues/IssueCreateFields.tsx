import { For, Show } from "solid-js";
import type { ProjectVM } from "~/lib/project-contract";

interface ProjectSetupFieldsProps {
  name: string;
  prefix: string;
  error: string | null;
  disabled: boolean;
  nameRef: (element: HTMLInputElement) => void;
  prefixRef: (element: HTMLInputElement) => void;
  onName: (value: string) => void;
  onPrefix: (value: string) => void;
}

export function ProjectSetupFields(props: ProjectSetupFieldsProps) {
  return (
    <>
      <label class="issues-field-label">
        Project name
        <input
          ref={props.nameRef}
          name="project-name"
          type="text"
          value={props.name}
          onInput={(event) => props.onName(event.currentTarget.value)}
          disabled={props.disabled}
          autocomplete="off"
          class="issues-text-input"
        />
      </label>
      <label class="issues-field-label">
        Issue key
        <input
          ref={props.prefixRef}
          name="project-prefix"
          type="text"
          maxlength="10"
          value={props.prefix}
          onInput={(event) => props.onPrefix(event.currentTarget.value.toUpperCase())}
          disabled={props.disabled}
          autocomplete="off"
          class="issues-text-input"
          aria-describedby="project-prefix-hint"
        />
      </label>
      <p id="project-prefix-hint" class="issues-field-hint">
        2–10 uppercase letters or numbers. Issues will look like {props.prefix || "DX"}-1.
      </p>
      <Show when={props.error}>
        {(message) => <p role="alert" class="issues-field-error">{message()}</p>}
      </Show>
    </>
  );
}

interface IssueDraftFieldsProps {
  projects: ProjectVM[];
  selectedId: string;
  selectedPrefix?: string;
  title: string;
  error: string | null;
  disabled: boolean;
  hasMore: boolean;
  loadingMore: boolean;
  loadMoreError: boolean;
  titleRef: (element: HTMLInputElement) => void;
  onProject: (id: string) => void;
  onTitle: (title: string) => void;
  onLoadMore: () => void;
  onCreateProject: () => void;
}

export function IssueDraftFields(props: IssueDraftFieldsProps) {
  return (
    <>
      <label class="issues-field-label">
        Project
        <select
          value={props.selectedId}
          onChange={(event) => props.onProject(event.currentTarget.value)}
          disabled={props.disabled}
          class="issues-text-input issues-project-select"
        >
          <For each={props.projects}>{(project) => (
            <option value={project.id}>{project.name} — {project.issue_prefix}</option>
          )}</For>
        </select>
      </label>
      <p class="issues-field-hint">New issues will be numbered {props.selectedPrefix ?? "…"}-…</p>
      <div class="issues-project-more">
        <button
          type="button"
          class="issues-button issues-button-secondary"
          disabled={props.disabled}
          onClick={() => props.onCreateProject()}
        >
          New project
        </button>
        <Show when={props.hasMore}>
          <button
            type="button"
            class="issues-button issues-button-secondary"
            disabled={props.loadingMore || props.disabled}
            onClick={() => props.onLoadMore()}
          >
            {props.loadingMore ? "Loading projects…" : "Load more projects"}
          </button>
          <Show when={props.loadMoreError}>
            <span role="alert">More projects couldn't be loaded. Try again.</span>
          </Show>
        </Show>
      </div>
      <label class="issues-field-label">
        Title
        <input
          ref={props.titleRef}
          id="issue-title"
          name="title"
          type="text"
          value={props.title}
          onInput={(event) => props.onTitle(event.currentTarget.value)}
          aria-invalid={Boolean(props.error)}
          aria-describedby={props.error ? "issue-title-error" : "issue-title-hint"}
          disabled={props.disabled}
          autocomplete="off"
          class="issues-text-input"
        />
      </label>
      <p id="issue-title-hint" class="issues-field-hint">Up to 512 UTF-8 bytes.</p>
      <Show when={props.error}>
        {(message) => <p id="issue-title-error" role="alert" class="issues-field-error">{message()}</p>}
      </Show>
    </>
  );
}
