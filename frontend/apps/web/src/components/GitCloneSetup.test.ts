import { describe, expect, it } from "vitest";

import { gitPseudonym, gitSetupCommands } from "./GitCloneSetup";

describe("Git clone setup", () => {
  it("derives the tenant-scoped no-reply identity required by receive-pack", () => {
    expect(gitPseudonym("u_operator", "acme")).toBe("u_operator@acme.noreply");
  });

  it("quotes server-provided values in pasteable commands", () => {
    expect(gitSetupCommands("https://git.test/o'ne/repo.git", "u'one", "tenant", "main")).toBe(
      [
        "myelin --edge 'https://git.test' auth login",
        "myelin auth configure-git",
        `git clone 'https://git.test/o'"'"'ne/repo.git'`,
        `git config user.name 'u'"'"'one@tenant.noreply'`,
        `git config user.email 'u'"'"'one@tenant.noreply'`,
        `git push -u origin 'main'`,
      ].join("\n"),
    );
  });

  it("keeps local development clone paths usable without inventing an Edge origin", () => {
    expect(gitSetupCommands("/acme/eu/repo.git", "u_one", "acme", "main").split("\n").slice(0, 3))
      .toEqual([
        "myelin auth login",
        "myelin auth configure-git",
        "git clone '/acme/eu/repo.git'",
      ]);
  });
});
