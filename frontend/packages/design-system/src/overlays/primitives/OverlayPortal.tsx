// Render overlays at the document root so transformed or clipped trigger ancestors do not contain
// the floating layer.

import { Portal } from "solid-js/web";
import type { JSX } from "solid-js";

export function OverlayPortal(props: { children: JSX.Element }): JSX.Element {
  // mount defaults to document.body; named here for clarity and to keep the seam explicit.
  return <Portal mount={document.body}>{props.children}</Portal>;
}
