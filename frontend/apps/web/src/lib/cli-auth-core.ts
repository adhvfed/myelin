const USER_CODE_SYMBOLS = /^[A-HJ-NP-Z2-9]{8}$/;

export type CliApprovalResult = "approved" | "expired" | "forbidden" | "unavailable";

/** Accept the code as printed by the CLI, with a forgiving case/dash boundary for manual entry. */
export function canonicalCliUserCode(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const canonical = value.trim().toUpperCase();
  const symbols = /^[A-HJ-NP-Z2-9]{4}-[A-HJ-NP-Z2-9]{4}$/.test(canonical)
    ? canonical.replace("-", "")
    : canonical;
  if (!USER_CODE_SYMBOLS.test(symbols)) return null;
  return `${symbols.slice(0, 4)}-${symbols.slice(4)}`;
}

export function cliApprovalPath(code: string, result?: CliApprovalResult): string {
  const params = new URLSearchParams({ code });
  if (result) params.set("result", result);
  return `/cli/auth?${params.toString()}`;
}
