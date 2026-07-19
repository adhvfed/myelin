// @myelin/design-system — Tier-0 wired for Solid (E0.7 / MR-016) + Tier-1 overlays (MR-017).
// The Tauri shell is MR-018; the app shell MR-019. Not here.
export { Icon, SPRITE_HREF_DEFAULT, type IconProps } from "./Icon";
export { ICON_NAMES, type IconName } from "./icon-names";
// The loading primitive (structure-matching skeleton + one debounced polite live region; §5.3/§6).
export { Skeleton, SkeletonBlock, type SkeletonProps, type SkeletonBlockProps } from "./Skeleton";
// The shared status pill (glyph+label; pr-state + check-verdict variants; R3.1). Status is TEXT,
// never colour alone; the verdict ring stays reserved for the CI trio.
export {
  StatusPill,
  checkVerdictLabel,
  type StatusPillProps,
  type PrStateValue,
  type CheckVerdict,
  type IssueStateCategory,
  type IssueStatePillProps,
} from "./StatusPill";
// The inline reference chip (<ReferenceChip>) + the labelled context-pane slot (R3.3, contributed
// DOWN). Chip owns the reference-chip §5 state renders (no_access withholds its title — non-leak);
// PaneSection owns the W1 "the pane assembles itself" contract (label-before-content, fail-static).
export { Chip, type ChipProps, type ChipType, type ChipState } from "./Chip";
export { PaneSection, type PaneSectionProps } from "./PaneSection";
// The R-17 §5.1 hard component — the ONE diff / files-changed viewer (R3.2 · G-7), consumed by the PR
// diff (G-7), compare (G-4), and commit detail (G-3). Change kind is TEXT (SR prefix + line numbers),
// never colour; the line grid is one tab stop (roving focus); side-by-side + unified.
export {
  DiffViewer,
  DiffToolbar,
  ExpandContextControl,
  type DiffViewerProps,
  type DiffViewerFile,
  type DiffViewerHunk,
  type DiffViewerLine,
  type ExpandedContext,
} from "./DiffViewer";
// Tier-1 overlay primitives (Dialog · ConfirmDialog · Popover · Menu · Tooltip · Toast) + the shared
// substrate (one focus-trap/portal/scroll-lock/z-index layer). See ./overlays.
export * from "./overlays";
// Generated token constants (z-index for JS, theme color maps, the semantic var() surface).
// The runtime styling source is the CSS in `@myelin/design-system/tokens.css`.
export * from "../generated/tokens";
