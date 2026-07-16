// @myelin/design-system — Tier-0 wired for Solid (E0.7 / MR-016) + Tier-1 overlays (MR-017).
// The Tauri shell is MR-018; the app shell MR-019. Not here.
export { Icon, SPRITE_HREF_DEFAULT, type IconProps } from "./Icon";
export { ICON_NAMES, type IconName } from "./icon-names";
// The loading primitive (structure-matching skeleton + one debounced polite live region; §5.3/§6).
export { Skeleton, SkeletonBlock, type SkeletonProps, type SkeletonBlockProps } from "./Skeleton";
// Tier-1 overlay primitives (Dialog · ConfirmDialog · Popover · Menu · Tooltip · Toast) + the shared
// substrate (one focus-trap/portal/scroll-lock/z-index layer). See ./overlays.
export * from "./overlays";
// Generated token constants (z-index for JS, theme color maps, the semantic var() surface).
// The runtime styling source is the CSS in `@myelin/design-system/tokens.css`.
export * from "../generated/tokens";
