// Tier-1 overlay primitives (MR-017) — six hand-built Solid components sharing ONE substrate
// (overlay-core + createOverlay + OverlayPortal + position). See overlays.md.
export { Dialog, type DialogProps, type DialogSize } from "./Dialog";
export { ConfirmDialog, type ConfirmDialogProps } from "./ConfirmDialog";
export { Popover, type PopoverProps } from "./Popover";
export { Menu, type MenuProps, type MenuItemSpec } from "./Menu";
export { Tooltip, type TooltipProps, type TooltipTriggerProps } from "./Tooltip";
export { ToastProvider, useToast, type ToastOptions, type ToastVariant } from "./Toast";

// Shared substrate (exported for testing / advanced composition; consumers normally use the six above).
export { OverlayPortal } from "./primitives/OverlayPortal";
export { createOverlay, type CreateOverlayOptions } from "./primitives/createOverlay";
export { computePosition, type Placement } from "./primitives/position";
export {
  getFocusable,
  trapFocus,
  lockScroll,
  unlockScroll,
  hideOthers,
  overlayDepth,
} from "./primitives/overlay-core";
