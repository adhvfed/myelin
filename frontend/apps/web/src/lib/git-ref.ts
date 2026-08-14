const utf8 = new TextEncoder();
const MAX_GIT_REF_BYTES = 4 * 1024;

function boundedRef(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 &&
    utf8.encode(value).byteLength <= MAX_GIT_REF_BYTES;
}

function containsControl(value: string): boolean {
  return [...value].some((character) => {
    const point = character.codePointAt(0)!;
    return point <= 0x1f || point === 0x7f;
  });
}

export function isFullGitRef(value: unknown): value is string {
  if (!boundedRef(value) || containsControl(value)) return false;
  const name = value.startsWith("refs/heads/")
    ? value.slice("refs/heads/".length)
    : value.startsWith("refs/tags/")
      ? value.slice("refs/tags/".length)
      : "";
  const components = name.split("/");
  return name.length > 0 && name !== "@" && !name.endsWith(".") &&
    components.every((component) => component.length > 0 && !component.startsWith(".") &&
      !component.endsWith(".lock")) && !name.includes("..") && !name.includes("@{") &&
    ![" ", "~", "^", ":", "?", "*", "[", "\\"].some((character) => name.includes(character));
}

export function isBranchRef(value: unknown): value is string {
  return typeof value === "string" && value.startsWith("refs/heads/") && isFullGitRef(value);
}
