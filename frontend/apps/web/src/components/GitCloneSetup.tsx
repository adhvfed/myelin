import { Icon } from "@myelin/design-system";

function shellQuote(value: string): string {
  return `'${value.replaceAll("'", `'"'"'`)}'`;
}

export function gitPseudonym(principalId: string, tenant: string): string {
  return `${principalId}@${tenant}.noreply`;
}

export function gitSetupCommands(
  url: string,
  principalId: string,
  tenant: string,
  defaultBranch: string,
): string {
  const pseudonym = gitPseudonym(principalId, tenant);
  const edge = edgeOriginFromCloneUrl(url);
  return [
    edge === undefined
      ? "myelin auth login"
      : `myelin --edge ${shellQuote(edge)} auth login`,
    "myelin auth configure-git",
    `git clone ${shellQuote(url)}`,
    `git config user.name ${shellQuote(pseudonym)}`,
    `git config user.email ${shellQuote(pseudonym)}`,
    `git push -u origin ${shellQuote(defaultBranch)}`,
  ].join("\n");
}

function edgeOriginFromCloneUrl(value: string): string | undefined {
  try {
    const url = new URL(value);
    return (url.protocol === "http:" || url.protocol === "https:") &&
      url.username === "" && url.password === ""
      ? url.origin
      : undefined;
  } catch {
    return undefined;
  }
}

export function CloneUrl(props: { url?: string; onCopy: () => void }) {
  return (
    <span style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-2)", "flex-wrap": "wrap" }}>
      <code data-testid="clone-url" style={{ "font-family": "var(--font-mono)", color: "var(--text-muted)" }}>{props.url}</code>
      <button
        type="button"
        onClick={() => {
          if (props.url) void navigator.clipboard?.writeText(props.url).catch(() => {});
          props.onCopy();
        }}
        style={{
          display: "inline-flex", "align-items": "center", gap: "var(--space-1)",
          padding: "var(--space-1) var(--space-2)", border: "var(--hairline) solid var(--border)",
          "border-radius": "var(--radius-1)", background: "var(--surface)", color: "var(--text-primary)", cursor: "pointer",
        }}
      >
        <Icon name="link" /> Copy
      </button>
    </span>
  );
}

export function GitSetupGuide(props: {
  url: string;
  principalId: string;
  tenant: string;
  defaultBranch: string;
}) {
  const pseudonym = () => gitPseudonym(props.principalId, props.tenant);
  const commands = () => gitSetupCommands(
    props.url,
    props.principalId,
    props.tenant,
    props.defaultBranch,
  );

  return (
    <details data-testid="git-setup" style={{ ...guide, "align-self": "stretch" }}>
      <summary style={{ cursor: "pointer", color: "var(--text-primary)", "font-weight": "600" }}>
        Set up Git
      </summary>
      <div style={{ display: "flex", "flex-direction": "column", gap: "var(--space-2)", "margin-block-start": "var(--space-2)" }}>
        <p style={{ margin: "0", color: "var(--text-muted)" }}>
          Myelin keeps personal email out of repository history. Configure this checkout with your
          tenant pseudonym, <code>{pseudonym()}</code>, before committing.
        </p>
        <pre data-testid="git-setup-commands" style={{ ...guide, "font-family": "var(--font-mono)", margin: "0", "white-space": "pre-wrap" }}>
          {commands()}
        </pre>
        <p style={{ margin: "0", color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>
          Browser approval creates an Edge-scoped CLI session; Git never needs a pasted API key.
          Make a commit before the first push to an empty repository.
        </p>
      </div>
    </details>
  );
}

const guide = {
  border: "var(--hairline) solid var(--border)",
  "border-radius": "var(--radius-1)",
  padding: "var(--space-2)",
  background: "var(--surface)",
} as const;
