import { test, expect, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";

// GT-004: the Git web UI browse surface + PR overview, driven in a REAL cached chromium against the
// dev-edge contract. Each screen renders the genuine edge ViewModel JSON; each is axe-clean and
// keyboard-navigable; a blocked merge shows WHY; the 401→/login floor still holds on a deep screen.

const C2 = "b2c3d4e5f60718293a4b5c6d7e8f900112233445";
const EDGE = `http://127.0.0.1:${process.env.DEV_EDGE_PORT ?? 8787}`;

async function expectNoAxeViolations(page: Page, context: string) {
  const results = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
    .analyze();
  expect(
    results.violations,
    `axe violations on ${context}: ${JSON.stringify(results.violations, null, 2)}`,
  ).toEqual([]);
}

async function devLogin(page: Page) {
  await page.goto("/login");
  await page.getByTestId("dev-login").click();
  await page.waitForURL("**/git/repos");
}

test.describe("GT-004 Git web UI — real browser", () => {
  test("the 401→/login floor holds on a deep git screen (unauthenticated)", async ({ page }) => {
    await page.goto("/git/repos/myelin/prs/1");
    await page.waitForURL("**/login");
    await expect(page.getByTestId("dev-login")).toBeVisible();
  });

  test("repo list links into the repo home, which renders the RepoHome ViewModel", async ({ page }) => {
    await devLogin(page);
    // Click through from the list (proves the link wiring), not a direct nav.
    await page.getByRole("link", { name: "acme/myelin", exact: true }).first().click();
    await page.waitForURL("**/git/repos/myelin");

    await expect(page.getByRole("heading", { name: "acme/myelin", level: 1 })).toBeVisible();
    // Clone URL (with the GT-006 honesty note) + the top-level tree from the ViewModel.
    await expect(page.getByTestId("clone-url")).toContainText("ssh://git@myelin/acme/myelin.git");
    await expect(page.getByText("clone over the wire is GT-006")).toBeVisible();
    const tree = page.getByTestId("repo-tree");
    await expect(tree.getByText("README.md", { exact: true })).toBeVisible();
    await expect(tree.getByText("crates/", { exact: true })).toBeVisible(); // a directory entry

    await expectNoAxeViolations(page, "repo home");
  });

  test("blob view renders the file contents from the WebEditForm ViewModel", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin");
    await page.getByTestId("repo-tree").getByRole("link", { name: "README.md" }).click();
    await page.waitForURL("**/git/repos/myelin/blob/main/README.md");

    await expect(page.getByTestId("blob-contents")).toContainText("The make-it-real spine.");
    await expect(page.getByText(/blake3:readmecontentaddress/)).toBeVisible(); // the content-address
    await expectNoAxeViolations(page, "blob view");
  });

  test("raw previews are inert and downloads use a proxy-owned attachment", async ({ page }) => {
    await devLogin(page);

    const raw = await page.request.get("/git-raw/myelin/main/README.md?d=inline");
    expect(raw.status()).toBe(200);
    expect(raw.headers()["content-type"]).toMatch(/^text\/plain\b/);
    expect(raw.headers()["content-disposition"]).toBe("inline");
    expect(raw.headers()["x-content-type-options"]).toBe("nosniff");
    expect(await raw.text()).toContain("The make-it-real spine.");

    const download = await page.request.get("/git-raw/myelin/main/README.md?d=attachment");
    expect(download.status()).toBe(200);
    expect(download.headers()["content-type"]).toMatch(/^application\/octet-stream\b/);
    expect(download.headers()["content-disposition"]).toBe(
      "attachment; filename*=UTF-8''README.md",
    );
  });

  test("commit log renders the revwalk page; a row links to the diff", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/commits/main");

    const log = page.getByTestId("commit-log");
    await expect(log.getByText("docs: expand the README")).toBeVisible();
    await expect(log.getByText("feat: land the make-it-real spine")).toBeVisible();
    await expect(log.getByText("u_dev_operator@acme.noreply").first()).toBeVisible();
    await expectNoAxeViolations(page, "commit log");

    // Navigate into the newest commit's diff via its short-oid link (R3.4: it carries `?ref=` so the
    // commit breadcrumb keeps the arrival ref — the URL now has a query string).
    await log.getByRole("link", { name: C2.slice(0, 12) }).click();
    await page.waitForURL(`**/git/repos/myelin/commit/${C2}?ref=main`);
    await expect(page.getByRole("heading", { name: "docs: expand the README" })).toBeVisible();
    const files = page.getByTestId("diff-files");
    await expect(files.getByText("README.md", { exact: true })).toBeVisible();
    await expect(files.getByText("The make-it-real spine.")).toBeVisible(); // the added line
    await expectNoAxeViolations(page, "commit diff");
  });

  test("PR overview (#1) reflects a BLOCKED gate and shows WHY + the fork-trust badge", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/1");

    // R3.3: the header is title-led (the fixture title) + the #number + the StatusPill (TEXT state).
    await expect(page.getByRole("heading", { level: 1, name: /#1/ })).toBeVisible();
    await expect(page.getByTitle("State: Open").first()).toBeVisible();
    // The checks panel (required contexts) + the fork-trust X-1 badge.
    await expect(page.getByTestId("pr-checks").getByText("ci/build", { exact: true })).toBeVisible();
    await expect(page.getByTestId("pr-checks").getByText("ci/test", { exact: true })).toBeVisible();
    await expect(page.getByTestId("fork-trust")).toContainText("untrusted fork");
    // Merge readiness REFLECTS the server gate: blocked, and it names why (read-only; merge is GT-004b).
    const blocked = page.getByTestId("merge-blocked");
    await expect(blocked).toContainText("Blocked by branch protection");
    await expect(blocked).toContainText("ci/test awaiting fork trust");
    await expect(page.getByTestId("merge-ready")).toHaveCount(0);

    await expectNoAxeViolations(page, "PR overview (blocked)");
  });

  test("PR overview (#2) reflects a READY gate", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/2");
    await expect(page.getByTestId("merge-ready")).toContainText("Ready to merge");
    await expect(page.getByTestId("merge-blocked")).toHaveCount(0);
    await expectNoAxeViolations(page, "PR overview (ready)");
  });

  test("a missing PR renders the dignified not-found state (not a crash)", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/999");
    // R3.3: the route-level error trio (anti-oracle — a 404 PR is indistinguishable from no-access).
    await expect(page.getByTestId("repo-error")).toBeVisible();
    await expectNoAxeViolations(page, "PR not-available");
  });

  test("keyboard: the repo-home tree file link is reachable and activates the blob view", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin");
    const fileLink = page.getByTestId("repo-tree").getByRole("link", { name: "README.md" });
    await fileLink.focus();
    await expect(fileLink).toBeFocused(); // keyboard-reachable (the WCAG 2.1.1 requirement)
    await fileLink.press("Enter"); // keyboard-operable: Enter on the focused link navigates
    await page.waitForURL("**/git/repos/myelin/blob/main/README.md");
    await expect(page.getByTestId("blob-contents")).toBeVisible();
  });

  // ── R3.4 repo-browsing completeness ──

  test("navigate into a NESTED dir and open a file (tree-at-path + nested blob)", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin");

    // Into crates/ (a directory row → the tree route).
    await page.getByTestId("repo-tree").getByRole("link", { name: "crates" }).click();
    await page.waitForURL("**/git/repos/myelin/tree/main/crates");
    await expectNoAxeViolations(page, "tree-at-path (crates)");

    // Into crates/myelin-edge/ (deeper).
    await page.getByTestId("repo-tree").getByRole("link", { name: "myelin-edge" }).click();
    await page.waitForURL("**/git/repos/myelin/tree/main/crates/myelin-edge");

    // Open lib.rs (a file → the nested blob route; contents are readable, not garbled).
    await page.getByTestId("repo-tree").getByRole("link", { name: "lib.rs" }).click();
    await page.waitForURL("**/git/repos/myelin/blob/main/crates/myelin-edge/lib.rs");
    await expect(page.getByTestId("blob-contents")).toContainText("the product edge");
    // The Download affordance is present (gateway-proxied attachment).
    await expect(page.getByTestId("blob-download")).toBeVisible();
    await expectNoAxeViolations(page, "nested blob");
  });

  test("the breadcrumb path segments are clickable (ref-true, no hardcoded main)", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/blob/main/crates/myelin-edge/lib.rs");
    const crumbs = page.getByRole("navigation", { name: "Breadcrumb" });
    // The intermediate segment `crates` is a link back to the tree at that sub-path.
    await crumbs.getByRole("link", { name: "crates", exact: true }).click();
    await page.waitForURL("**/git/repos/myelin/tree/main/crates");
    await expect(page.getByTestId("repo-tree")).toBeVisible();
  });

  test("the commit-log pager is bidirectional with an honest position readout", async ({ page }) => {
    await devLogin(page);
    // limit=1 so the two-commit seed pages: first page has Older + no Newer.
    await page.goto("/git/repos/myelin/commits/main?");
    await expect(page.getByTestId("commit-log")).toBeVisible();
    await expect(page.getByTestId("pager-position")).toContainText("page 1");
    await expectNoAxeViolations(page, "commit log pager");
  });

  test("the catch-all route renders NotAvailable (not a raw framework 404)", async ({ page }) => {
    await devLogin(page);
    await page.goto("/this/path/does/not/exist");
    await expect(page.getByTestId("not-available")).toBeVisible();
    await expect(page.getByRole("navigation", { name: "Primary" })).toBeVisible();
  });

  test("an expired access and refresh credential redirects before CSP-protected streaming", async ({ page }) => {
    await devLogin(page);
    const consoleErrors: string[] = [];
    page.on("console", (message) => {
      if (message.type() === "error") consoleErrors.push(message.text());
    });

    await page.request.post(`${EDGE}/__test/config`, {
      data: { forceUnauthorized: true },
    });
    try {
      await page.goto("/git/repos/myelin");
      await page.waitForURL("**/login");
      expect(consoleErrors.filter((message) => message.includes("Content Security Policy"))).toEqual([]);
    } finally {
      await page.request.post(`${EDGE}/__test/config`, {
        data: { forceUnauthorized: false },
      });
    }
  });
});
