import { Icon, useToast } from "@myelin/design-system";
import { createSignal, onMount } from "solid-js";

export interface CopyArtifactRefProps {
  reference: string;
  label?: string;
  class?: string;
}

export function CopyArtifactRef(props: CopyArtifactRefProps) {
  const toast = useToast();
  const [interactive, setInteractive] = createSignal(false);
  onMount(() => setInteractive(true));

  const copy = async () => {
    try {
      if (!navigator.clipboard) throw new Error("clipboard unavailable");
      await navigator.clipboard.writeText(props.reference);
      toast.show({ title: "Reference copied", variant: "success" });
    } catch {
      toast.show({ title: "Reference couldn’t be copied", variant: "danger" });
    }
  };

  return <button
    type="button"
    class={`artifact-ref-copy${props.class ? ` ${props.class}` : ""}`}
    title={props.reference}
    disabled={!interactive()}
    onClick={() => void copy()}
  >
    <Icon name="link" /> {props.label ?? "Copy reference"}
  </button>;
}
