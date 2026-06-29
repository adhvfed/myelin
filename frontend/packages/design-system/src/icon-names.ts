// The 42 strok icons in the self-hosted sprite (design-planning/08-design-system/04-icons/dist).
// Kept in lockstep with that sprite's <symbol id="..."> set + manifest.json. A typed union so a
// typo in an icon name is a compile error, not a silent missing-glyph at runtime.
export const ICON_NAMES = [
  "agent", "approve", "branch", "channel", "check-fail", "check-pass",
  "check-pending", "chevron", "close", "commit", "cycle", "database",
  "doc", "edit", "external-link", "file", "folder", "gate", "human",
  "inbox", "issue", "kebab", "link", "merge", "message", "nav-chat",
  "nav-ci", "nav-code", "nav-issues", "nav-knowledge", "priority",
  "pull-request", "reject", "repo", "rerun", "roadmap", "run", "search",
  "settings", "sub-issue", "tag", "team",
] as const;

export type IconName = (typeof ICON_NAMES)[number];
