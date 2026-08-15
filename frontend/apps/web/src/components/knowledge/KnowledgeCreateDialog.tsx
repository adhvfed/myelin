import { Dialog, Icon } from "@myelin/design-system";
import { useAction } from "@solidjs/router";
import { createEffect, createSignal, For, Show } from "solid-js";
import { knowledgeMutate, type KnowledgeCreateReceipt, type KnowledgeErrorKind, type KnowledgeVisibility } from "~/lib/knowledge-api";

export interface KnowledgeCreateDialogProps {
  open: boolean;
  onClose: () => void;
  onCreated: (receipt: KnowledgeCreateReceipt) => void;
}

const TEMPLATES = [
  { id: "blank", title: "Blank page", copy: "Start with a clean writing surface." },
  { id: "product-spec", title: "Product spec", copy: "Frame the problem, outcomes, approach, and risks." },
  { id: "runbook", title: "Runbook", copy: "Capture signals, response steps, and recovery checks." },
] as const;

function errorCopy(kind: KnowledgeErrorKind): string {
  if (kind === "bad-input") return "Use a title without leading or trailing spaces.";
  if (kind === "unavailable") return "Knowledge is temporarily unavailable. The page was not confirmed.";
  return "We couldn’t confirm the new page. This draft can be retried safely until you edit it.";
}

export function KnowledgeCreateDialog(props: KnowledgeCreateDialogProps) {
  const mutate = useAction(knowledgeMutate);
  const [title, setTitle] = createSignal("");
  const [template, setTemplate] = createSignal<typeof TEMPLATES[number]["id"]>("blank");
  const [visibility, setVisibility] = createSignal<KnowledgeVisibility>("private");
  const [clientNonce, setClientNonce] = createSignal(crypto.randomUUID());
  const [submitting, setSubmitting] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  let titleInput: HTMLInputElement | undefined;

  createEffect(() => {
    if (!props.open) return;
    setTitle(""); setTemplate("blank"); setVisibility("private"); setClientNonce(crypto.randomUUID()); setSubmitting(false); setError(null);
  });

  const close = () => { if (!submitting()) props.onClose(); };
  const submit = async (event: SubmitEvent) => {
    event.preventDefault();
    if (!title().trim() || title().trim() !== title()) { setError("A clean page title is required."); titleInput?.focus(); return; }
    setSubmitting(true); setError(null);
    try {
      const result = await mutate({ op: "create", title: title(), template: template(), visibility: visibility(), clientNonce: clientNonce() });
      if (!result.ok || result.op !== "create") { setError(errorCopy(result.ok ? "error" : result.error)); return; }
      props.onClose(); props.onCreated(result.receipt);
    } catch { setError(errorCopy("error")); }
    finally { setSubmitting(false); }
  };

  return (
    <Dialog open={props.open} onClose={close} title="Create a Knowledge page" description="Choose a useful starting point. Every page stays private until you share it with your team." size="lg" dismissable={!submitting()} initialFocus={() => titleInput}
      footer={<><button type="button" class="knowledge-button secondary" onClick={close} disabled={submitting()}>Cancel</button><button type="submit" form="knowledge-create-form" class="knowledge-button primary" disabled={submitting()}><Icon name={submitting() ? "cycle" : "doc"} />{submitting() ? "Creating…" : "Create page"}</button></>}>
      <form id="knowledge-create-form" class="knowledge-create-form" onSubmit={submit}>
        <label class="knowledge-field">Page title<input ref={titleInput} value={title()} maxlength={512} autocomplete="off" placeholder="Service ownership model" disabled={submitting()} aria-invalid={Boolean(error())} onInput={(event) => { setTitle(event.currentTarget.value); setClientNonce(crypto.randomUUID()); setError(null); }} /></label>
        <fieldset class="knowledge-template-fieldset"><legend>Start from</legend><div class="knowledge-template-grid"><For each={TEMPLATES}>{(item) => <label class="knowledge-template-card" classList={{ selected: template() === item.id }}><input type="radio" name="template" value={item.id} checked={template() === item.id} onChange={() => { setTemplate(item.id); setClientNonce(crypto.randomUUID()); }} /><strong>{item.title}</strong><span>{item.copy}</span></label>}</For></div></fieldset>
        <label class="knowledge-field">Visibility<select value={visibility()} onChange={(event) => { setVisibility(event.currentTarget.value as KnowledgeVisibility); setClientNonce(crypto.randomUUID()); }}><option value="private">Private — only me</option><option value="team">Team — everyone in this workspace</option></select></label>
        <Show when={error()}>{(message) => <p role="alert" class="knowledge-error">{message()}</p>}</Show>
      </form>
    </Dialog>
  );
}
