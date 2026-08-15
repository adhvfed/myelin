import { describe, expect, it } from "vitest";

import {
  parseRepoCreateReceipt,
  parseRepositorySlug,
  repositorySlugError,
} from "./repo-create";

describe("repository creation boundary", () => {
  it("normalizes a human name that the storage layer can address", () => {
    expect(parseRepositorySlug("  platform/api  ")).toBe("platform/api");
    expect(repositorySlugError("release.git")).toBeNull();
  });

  it("refuses a namespace that would resolve inside another bare repository", () => {
    expect(parseRepositorySlug("platform.git/api")).toBeNull();
    expect(repositorySlugError("platform.git/api")).not.toBeNull();
  });

  it("accepts only the exact durable receipt for the requested repository", () => {
    const receipt = {
      durable: true,
      created: true,
      applied: { action: "git.repo.create", slug: "platform/api" },
    };
    expect(parseRepoCreateReceipt(receipt, "platform/api"))
      .toEqual({ slug: "platform/api", created: true });
    expect(parseRepoCreateReceipt(receipt, "platform/other")).toBeNull();
  });
});
