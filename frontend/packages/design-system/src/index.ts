// @myelin/design-system — Tier-0 wired for Solid (E0.7 / MR-016).
// Tier-1 overlay primitives are MR-017; the Tauri shell MR-018; the app shell MR-019. Not here.
export { Icon, SPRITE_HREF_DEFAULT, type IconProps } from "./Icon";
export { ICON_NAMES, type IconName } from "./icon-names";
// Generated token constants (z-index for JS, theme color maps, the semantic var() surface).
// The runtime styling source is the CSS in `@myelin/design-system/tokens.css`.
export * from "../generated/tokens";
