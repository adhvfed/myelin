import { describe, expect, it } from "vitest";

import {
  authFailureDestination,
  authenticationDestination,
  DEFAULT_AUTH_RETURN_TO,
  MAX_AUTH_RETURN_TO_BYTES,
  safeAuthReturnTo,
} from "./auth-return";

describe("safeAuthReturnTo", () => {
  it("keeps a local path and its query while refusing a fragment", () => {
    expect(safeAuthReturnTo("/cli/auth?code=ABCD-EFGH")).toBe(
      "/cli/auth?code=ABCD-EFGH",
    );
    expect(safeAuthReturnTo("/cli/auth#secret")).toBe(DEFAULT_AUTH_RETURN_TO);
  });

  it.each([
    "https://outside.example/steal",
    "//outside.example/steal",
    "/\\outside.example/steal",
    "/%2f%2foutside.example/steal",
    "/%5coutside.example/steal",
    "/cli/auth\r\nlocation: https://outside.example",
    " /cli/auth",
    "",
  ])("refuses the unsafe return destination %j", (value) => {
    expect(safeAuthReturnTo(value)).toBe(DEFAULT_AUTH_RETURN_TO);
  });

  it("bounds both code units and encoded bytes", () => {
    expect(safeAuthReturnTo(`/${"a".repeat(MAX_AUTH_RETURN_TO_BYTES)}`)).toBe(
      DEFAULT_AUTH_RETURN_TO,
    );
    expect(safeAuthReturnTo(`/${"ø".repeat(MAX_AUTH_RETURN_TO_BYTES / 2)}`)).toBe(
      DEFAULT_AUTH_RETURN_TO,
    );
  });

  it("carries a safe return path through an honest login error", () => {
    expect(authFailureDestination("token_invalid", "/cli/auth?code=ABCD-EFGH")).toBe(
      "/login?error=token_invalid&return_to=%2Fcli%2Fauth%3Fcode%3DABCD-EFGH",
    );
  });

  it("keeps the ordinary login URL quiet while preserving an intentional destination", () => {
    expect(authenticationDestination(DEFAULT_AUTH_RETURN_TO)).toBe("/login");
    expect(authenticationDestination("/cli/auth?code=ABCD-EFGH")).toBe(
      "/login?return_to=%2Fcli%2Fauth%3Fcode%3DABCD-EFGH",
    );
    expect(authFailureDestination("sso_failed", DEFAULT_AUTH_RETURN_TO)).toBe(
      "/login?error=sso_failed",
    );
  });
});
