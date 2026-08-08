// Icons available in the package's self-hosted sprite.
// Kept in lockstep with that sprite's <symbol id="..."> set + manifest.json. A typed union so a
// typo in an icon name is a compile error, not a silent missing-glyph at runtime.
// `download` was registered by R3.4 (repo-browsing) for the blob/large-file/binary Download
// affordance — added through the pipeline (manifest + sprite), NOT drawn ad hoc; semantically
// distinct from `external-link` ("open raw").
export const ICON_NAMES = [
  "agent", "approve", "branch", "channel", "check-fail", "check-pass",
  "check-pending", "chevron", "close", "commit", "cycle", "database",
  "doc", "download", "edit", "external-link", "file", "folder", "gate", "human",
  "inbox", "issue", "kebab", "link", "merge", "message", "nav-chat",
  "nav-ci", "nav-code", "nav-issues", "nav-knowledge", "priority",
  "pull-request", "reject", "repo", "rerun", "roadmap", "run", "search",
  "settings", "sub-issue", "tag", "team",
] as const;

export type IconName = (typeof ICON_NAMES)[number];
