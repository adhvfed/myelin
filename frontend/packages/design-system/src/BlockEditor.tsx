import { For, Index, Show, createEffect, createSignal, onCleanup, type JSX } from "solid-js";

export const BLOCK_TYPES = [
  "paragraph", "heading", "bullet_list", "ordered_list", "task_list", "blockquote",
  "code_block", "callout", "divider",
] as const;

export type BlockType = typeof BLOCK_TYPES[number];

export interface EditorBlock {
  id?: string;
  type: BlockType;
  markdown: string;
  references?: string[];
  state?: "active" | "tombstoned";
}

export interface BlockEditorProps {
  value: readonly EditorBlock[];
  onChange: (blocks: EditorBlock[]) => void;
  readOnly?: boolean;
  label?: string;
  focusOnMount?: boolean;
  inputLabel?: string;
  onSubmit?: () => void;
  submitShortcut?: "enter" | "mod-enter";
  onEscape?: () => void;
  placeholder?: string;
  referenceLabel?: (reference: string) => string;
  referenceHref?: (reference: string) => string | undefined;
}

const TYPE_LABEL: Record<BlockType, string> = {
  paragraph: "Text",
  heading: "Heading",
  bullet_list: "Bulleted list",
  ordered_list: "Numbered list",
  task_list: "Task",
  blockquote: "Quote",
  code_block: "Code",
  callout: "Callout",
  divider: "Divider",
};

export function balanceInlineMarks(source: string): string {
  let result = source;
  for (const mark of ["**", "__", "`"] as const) {
    const occurrences = result.split(mark).length - 1;
    if (occurrences % 2 !== 0) result += mark;
  }
  return result;
}

export function splitBlock(block: EditorBlock, offset: number): [EditorBlock, EditorBlock] {
  const before = balanceInlineMarks(block.markdown.slice(0, offset));
  const after = block.markdown.slice(offset);
  const referencesBefore = block.markdown.slice(0, offset).split("\uFFFC").length - 1;
  const references = block.references;
  return [
    {
      ...block,
      markdown: before,
      ...(references === undefined ? {} : { references: references.slice(0, referencesBefore) }),
    },
    {
      type: block.type === "heading" ? "paragraph" : block.type,
      markdown: after,
      ...(references === undefined ? {} : { references: references.slice(referencesBefore) }),
    },
  ];
}

export function mergeBlocks(previous: EditorBlock, current: EditorBlock): EditorBlock {
  const references = previous.references === undefined && current.references === undefined
    ? undefined
    : [...(previous.references ?? []), ...(current.references ?? [])];
  return {
    ...previous,
    markdown: previous.markdown + current.markdown,
    ...(references === undefined ? {} : { references }),
  };
}

export function toggleInlineMark(source: string, start: number, end: number, mark: "**" | "__"): string {
  if (start === end) return `${source.slice(0, start)}${mark}${mark}${source.slice(end)}`;
  const selected = source.slice(start, end);
  if (selected.startsWith(mark) && selected.endsWith(mark) && selected.length >= mark.length * 2) {
    return source.slice(0, start) + selected.slice(mark.length, -mark.length) + source.slice(end);
  }
  return `${source.slice(0, start)}${mark}${selected}${mark}${source.slice(end)}`;
}

function selectionOffset(element: HTMLElement): { start: number; end: number } | null {
  const selection = window.getSelection();
  if (!selection?.rangeCount) return null;
  const range = selection.getRangeAt(0);
  if (!element.contains(range.startContainer) || !element.contains(range.endContainer)) return null;
  const beforeStart = range.cloneRange();
  beforeStart.selectNodeContents(element);
  beforeStart.setEnd(range.startContainer, range.startOffset);
  const beforeEnd = range.cloneRange();
  beforeEnd.selectNodeContents(element);
  beforeEnd.setEnd(range.endContainer, range.endOffset);
  return { start: beforeStart.toString().length, end: beforeEnd.toString().length };
}

function restoreCaret(element: HTMLElement, offset: number): void {
  const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
  let remaining = offset;
  let node = walker.nextNode();
  while (node) {
    const length = node.textContent?.length ?? 0;
    if (remaining <= length) {
      const range = document.createRange();
      range.setStart(node, remaining);
      range.collapse(true);
      const selection = window.getSelection();
      selection?.removeAllRanges();
      selection?.addRange(range);
      return;
    }
    remaining -= length;
    node = walker.nextNode();
  }
  element.focus();
}

function cleanEditableText(element: HTMLElement): string {
  return (element.innerText ?? element.textContent ?? "").replace(/\r/g, "").replace(/\n$/, "");
}

const REFERENCE_MARKER = "\uFFFC";
const REFERENCE_SELECTOR = "[data-block-reference]";

function blockSignature(block: EditorBlock, readOnly: boolean): string {
  return JSON.stringify([block.markdown, block.references ?? null, readOnly]);
}

function renderBlock(
  element: HTMLElement,
  block: EditorBlock,
  labelFor: (reference: string) => string,
  hrefFor?: (reference: string) => string | undefined,
): void {
  const references = block.references ?? [];
  const fragment = document.createDocumentFragment();
  const parts = block.markdown.split(REFERENCE_MARKER);
  parts.forEach((part, index) => {
    if (part) fragment.append(document.createTextNode(part));
    const reference = references[index];
    if (index >= parts.length - 1 || reference === undefined) return;
    const href = hrefFor?.(reference);
    const chip = href ? document.createElement("a") : document.createElement("span");
    if (href && chip instanceof HTMLAnchorElement) chip.href = href;
    chip.className = "block-editor-reference";
    chip.dataset.blockReference = reference;
    chip.dataset.label = labelFor(reference);
    chip.title = reference;
    chip.setAttribute("aria-label", `Reference: ${labelFor(reference)}`);
    chip.contentEditable = "false";
    chip.textContent = REFERENCE_MARKER;
    fragment.append(chip);
  });
  element.replaceChildren(fragment);
}

function readBlock(element: HTMLElement): { markdown: string; references: string[] } {
  const clone = element.cloneNode(true) as HTMLElement;
  const references: string[] = [];

  const text = document.createTreeWalker(clone, NodeFilter.SHOW_TEXT);
  let node = text.nextNode();
  while (node) {
    if (!(node.parentElement?.closest(REFERENCE_SELECTOR))) {
      node.textContent = node.textContent?.replaceAll(REFERENCE_MARKER, "") ?? "";
    }
    node = text.nextNode();
  }

  clone.querySelectorAll<HTMLElement>(REFERENCE_SELECTOR).forEach((chip) => {
    const reference = chip.dataset.blockReference;
    if (reference !== undefined) {
      references.push(reference);
      chip.replaceWith(document.createTextNode(REFERENCE_MARKER));
    } else {
      chip.remove();
    }
  });

  return { markdown: cleanEditableText(clone), references };
}

export function BlockEditor(props: BlockEditorProps): JSX.Element {
  const [slashAt, setSlashAt] = createSignal<number | null>(null);
  const [composing, setComposing] = createSignal(false);
  const elements = new Map<number, HTMLElement>();
  const lastEmitted = new Map<number, string>();

  const update = (index: number, patch: Partial<EditorBlock>) => {
    props.onChange(props.value.map((block, at) => at === index ? { ...block, ...patch } : { ...block }));
  };

  const focusAt = (index: number, offset = 0) => queueMicrotask(() => {
    const element = elements.get(index);
    if (element) restoreCaret(element, offset);
  });

  const splitAt = (index: number, offset: number) => {
    const block = props.value[index];
    if (!block) return;
    const [before, after] = splitBlock(block, offset);
    props.onChange([
      ...props.value.slice(0, index).map((item) => ({ ...item })),
      before,
      after,
      ...props.value.slice(index + 1).map((item) => ({ ...item })),
    ]);
    focusAt(index + 1);
  };

  const mergeAt = (index: number) => {
    if (index <= 0) return;
    const previous = props.value[index - 1];
    const current = props.value[index];
    if (!previous || !current) return;
    const caret = previous.markdown.length;
    props.onChange([
      ...props.value.slice(0, index - 1).map((item) => ({ ...item })),
      mergeBlocks(previous, current),
      ...props.value.slice(index + 1).map((item) => ({ ...item })),
    ]);
    focusAt(index - 1, caret);
  };

  const keyDown = (event: KeyboardEvent, index: number) => {
    const element = event.currentTarget as HTMLElement;
    const selection = selectionOffset(element);
    if (!selection) return;
    const submit = event.key === "Enter" && !composing() && props.onSubmit &&
      (props.submitShortcut === "mod-enter" ? event.metaKey || event.ctrlKey : !event.shiftKey);
    if (submit) {
      event.preventDefault();
      props.onSubmit?.();
      return;
    }
    if (event.key === "Escape" && props.onEscape) {
      event.preventDefault();
      props.onEscape();
      return;
    }
    if (event.key === "Enter" && !event.shiftKey && !composing()) {
      event.preventDefault();
      splitAt(index, selection.start);
      return;
    }
    if (event.key === "Backspace" && selection.start === 0 && selection.end === 0 && index > 0) {
      event.preventDefault();
      mergeAt(index);
      return;
    }
    if ((event.metaKey || event.ctrlKey) && ["b", "i"].includes(event.key.toLowerCase())) {
      event.preventDefault();
      const mark = event.key.toLowerCase() === "b" ? "**" : "__";
      update(index, { markdown: toggleInlineMark(props.value[index]?.markdown ?? "", selection.start, selection.end, mark) });
      focusAt(index, selection.end + mark.length * 2);
      return;
    }
    if (event.key === "Escape") setSlashAt(null);
  };

  createEffect(() => {
    const readOnly = Boolean(props.readOnly);
    props.value.forEach((block, index) => {
      const element = elements.get(index);
      const signature = blockSignature(block, readOnly);
      if (element && (element.textContent !== block.markdown || lastEmitted.get(index) !== signature) &&
          (document.activeElement !== element || lastEmitted.get(index) !== signature)) {
        renderBlock(element, block, props.referenceLabel ?? ((reference) => reference), props.referenceHref);
        lastEmitted.set(index, signature);
      }
    });
  });

  onCleanup(() => { elements.clear(); lastEmitted.clear(); });

  return (
    <div class="block-editor" aria-label={props.label ?? "Document editor"}>
      <Index each={props.value}>
        {(block, index) => (
          <div class="block-editor-row" data-block-type={block().type} data-state={block().state ?? "active"}>
            <Show when={!props.readOnly && block().state !== "tombstoned"}>
              <button
                type="button"
                class="block-editor-handle"
                aria-label={`Change ${TYPE_LABEL[block().type]} block type`}
                aria-expanded={slashAt() === index}
                onClick={() => setSlashAt(slashAt() === index ? null : index)}
              >
                ⋮⋮
              </button>
            </Show>
            <Show
              when={block().state !== "tombstoned"}
              fallback={<p class="block-editor-tombstone">This block was erased.</p>}
            >
              <div
                ref={(element) => {
                  elements.set(index, element);
                  renderBlock(
                    element,
                    block(),
                    props.referenceLabel ?? ((reference) => reference),
                    props.referenceHref,
                  );
                  lastEmitted.set(index, blockSignature(block(), Boolean(props.readOnly)));
                  if (props.focusOnMount && index === 0) queueMicrotask(() => element.focus());
                }}
                class="block-editor-input"
                classList={{ "block-editor-divider": block().type === "divider" }}
                data-placeholder={index === 0 ? props.placeholder : undefined}
                contentEditable={!props.readOnly}
                tabIndex={0}
                role="textbox"
                aria-label={props.inputLabel ?? `${TYPE_LABEL[block().type]} block ${index + 1}`}
                aria-multiline="true"
                dir="auto"
                spellcheck={block().type !== "code_block"}
                onCompositionStart={() => setComposing(true)}
                onCompositionEnd={(event) => {
                  setComposing(false);
                  const value = readBlock(event.currentTarget);
                  const patch = block().references === undefined && value.references.length === 0
                    ? { markdown: value.markdown }
                    : value;
                  const next = { ...block(), ...patch };
                  lastEmitted.set(index, blockSignature(next, Boolean(props.readOnly)));
                  update(index, patch);
                }}
                onInput={(event) => {
                  if (composing()) return;
                  const value = readBlock(event.currentTarget);
                  const patch = block().references === undefined && value.references.length === 0
                    ? { markdown: value.markdown }
                    : value;
                  const next = { ...block(), ...patch };
                  lastEmitted.set(index, blockSignature(next, Boolean(props.readOnly)));
                  update(index, patch);
                  setSlashAt(value.markdown === "/" ? index : null);
                }}
                onKeyDown={(event) => keyDown(event, index)}
              />
            </Show>
            <Show when={!props.readOnly && slashAt() === index}>
              <div class="block-editor-menu" role="menu" aria-label="Block type">
                <For each={BLOCK_TYPES}>
                  {(type) => (
                    <button
                      type="button"
                      role="menuitem"
                      onClick={() => {
                        update(index, { type, markdown: block().markdown === "/" ? "" : block().markdown });
                        setSlashAt(null);
                        focusAt(index);
                      }}
                    >
                      {TYPE_LABEL[type]}
                    </button>
                  )}
                </For>
              </div>
            </Show>
          </div>
        )}
      </Index>
    </div>
  );
}
