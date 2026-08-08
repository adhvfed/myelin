import { BlockEditor, type EditorBlock } from "@myelin/design-system";

export interface SharedComposerProps {
  value: string;
  onValue: (value: string) => void;
  label: string;
  placeholder?: string;
  disabled?: boolean;
  focusOnMount?: boolean;
  onSubmit?: () => void;
  onEscape?: () => void;
  submitShortcut?: "enter" | "mod-enter";
  invalid?: boolean;
}

/** The compact form of Myelin's shared block editor used by comments, reviews, and Chat. */
export function SharedComposer(props: SharedComposerProps) {
  const blocks = (): EditorBlock[] => props.value.split("\n").map((markdown) => ({
    type: "paragraph",
    markdown,
  }));
  return <div class="shared-composer" classList={{ invalid: props.invalid }}>
    <BlockEditor
      value={blocks()}
      onChange={(next) => props.onValue(next.map((block) => block.markdown).join("\n"))}
      readOnly={props.disabled}
      inputLabel={props.label}
      placeholder={props.placeholder}
      focusOnMount={props.focusOnMount}
      onSubmit={props.onSubmit}
      onEscape={props.onEscape}
      submitShortcut={props.submitShortcut}
    />
  </div>;
}
