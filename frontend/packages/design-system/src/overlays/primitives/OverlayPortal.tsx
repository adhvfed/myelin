// The ONE portal (overlays.md §0.1: "Portal-always to the document root"). Every overlay in this
// package renders through THIS component, never in the triggering subtree — which forecloses the
// "panel clipped by a transform/overflow:hidden ancestor / rendered inside the 240px sidebar" bug
// class by construction. It is a thin, documented wrapper over Solid's `<Portal>` (mount =
// document.body) so there is a single, greppable portal seam shared by all six primitives.

import { Portal } from "solid-js/web";
import type { JSX } from "solid-js";

export function OverlayPortal(props: { children: JSX.Element }): JSX.Element {
  // mount defaults to document.body; named here for clarity and to keep the seam explicit.
  return <Portal mount={document.body}>{props.children}</Portal>;
}
