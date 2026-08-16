import { test, expect, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";

// Browser coverage for repository browsing and the PR overview, including accessibility, keyboard
// navigation, blocked merge details, and authentication redirects.
// FRONTEND-CONTRACT: git-read-dev-edge-parity

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
  await page.waitForLoadState("networkidle");
  await page.getByTestId("dev-login").click();
  await page.waitForURL("**/git/repos");
}

test.describe("GT-004 Git web UI — real browser", () => {
  test.afterEach(async ({ request }) => {
    const response = await request.post(`${EDGE}/__test/config`, {
      data: { forceUnauthorized: false },
    });
    expect(response.ok()).toBe(true);
  });

  test("the 401→/login floor holds on a deep git screen (unauthenticated)", async ({ page }) => {
    await page.goto("/git/repos/myelin/prs/1");
    await page.waitForURL("**/login");
    await expect(page.getByTestId("dev-login")).toBeVisible();
  });

  test("repo list links into the repo home, which renders the RepoHome ViewModel", async ({ page }) => {
    await devLogin(page);
    // Reach the page through the list link.
    await page.getByRole("link", { name: "acme/myelin", exact: true }).first().click();
    await page.waitForURL("**/git/repos/myelin");

    await expect(page.getByRole("heading", { name: "acme/myelin", level: 1 })).toBeVisible();
    await expect(page.getByRole("button", { name: "Copy reference" }))
      .toHaveAttribute("title", "myelin://acme/git/repo/myelin");
    // Clone URL plus the viewer-specific privacy setup and top-level tree from the ViewModel.
    await expect(page.getByTestId("clone-url")).toContainText("/acme/eu-west/myelin.git");
    const setup = page.getByTestId("git-setup");
    await setup.getByText("Set up Git").click();
    await expect(setup).toContainText("u_dev_operator@acme.noreply");
    await expect(setup).toContainText("myelin auth configure-git");
    await expect(setup).toContainText("Git never needs a pasted API key");
    const tree = page.getByTestId("repo-tree");
    await expect(tree.getByText("README.md", { exact: true })).toBeVisible();
    await expect(tree.getByText("crates/", { exact: true })).toBeVisible(); // a directory entry

    await expectNoAxeViolations(page, "repo home");
  });

  test("the ref switcher keeps the current branch pinned while searching the server", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin");
    await page.getByTestId("ref-switcher-trigger").click();

    const switcher = page.getByRole("dialog", { name: "Switch branch or tag" });
    const search = switcher.getByRole("textbox", { name: "Search branches and tags" });
    await expect(switcher.getByRole("group", { name: "Pinned" }))
      .toContainText("main");

    await search.fill("feature");
    const feature = switcher.getByRole("link", { name: "feature", exact: true });
    await expect(feature).toHaveAttribute(
      "href",
      "/git/repos/myelin/tree/refs%2Fheads%2Ffeature",
    );
    await expect(switcher.getByRole("group", { name: "Pinned" }))
      .toContainText("main");
    await expectNoAxeViolations(page, "searched ref switcher");
  });

  test("blob view renders the file contents from the WebEditForm ViewModel", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin");
    await page.getByTestId("repo-tree").getByRole("link", { name: "README.md" }).click();
    await page.waitForURL("**/git/repos/myelin/blob/refs%2Fheads%2Fmain/README.md");

    await expect(page.getByTestId("blob-contents")).toContainText("The make-it-real spine.");
    await expect(page.getByText(/blake3:readmecontentaddress/)).toBeVisible(); // the content-address
    await expectNoAxeViolations(page, "blob view");
  });

  test("blame traces each line to a commit without losing the file context", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/blob/main/README.md");
    await page.getByTestId("blame-link").click();
    await page.waitForURL("**/git/repos/myelin/blame/main/README.md");

    await expect(page.getByRole("heading", { name: /Line history/ })).toContainText("README.md");
    const viewer = page.getByTestId("blame-viewer");
    await expect(viewer).toContainText("The make-it-real spine.");
    await expect(viewer.getByRole("link", { name: C2.slice(0, 10) })).toBeVisible();
    await expect(viewer.getByText("docs: expand the README")).toBeVisible();
    await expect(page.getByText(`snapshot ${C2.slice(0, 12)}`)).toBeVisible();
    await expect(page.getByRole("link", { name: "View file" })).toBeVisible();
    await expectNoAxeViolations(page, "blame view");
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

    // The header includes the title, PR number, and visible state label.
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

  test("keyboard: Enter on the repo-home tree file link activates the blob view", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin");
    await expect(page.locator(".app-shell")).toHaveAttribute("data-shortcuts-ready", "true");
    const fileLink = page.getByTestId("repo-tree").getByRole("link", { name: "README.md" });
    // Locator.press focuses the semantic link and dispatches a real Enter key event. The resulting
    // Verify the resulting navigation rather than a transient focus state.
    await fileLink.press("Enter");
    await page.waitForURL("**/git/repos/myelin/blob/refs%2Fheads%2Fmain/README.md");
    await expect(page.getByTestId("blob-contents")).toBeVisible();
  });

  // ── R3.4 repo-browsing completeness ──

  test("navigate into a NESTED dir and open a file (tree-at-path + nested blob)", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin");

    // Into crates/ (a directory row → the tree route).
    await page.getByTestId("repo-tree").getByRole("link", { name: "crates" }).click();
    await page.waitForURL("**/git/repos/myelin/tree/refs%2Fheads%2Fmain/crates");
    await expectNoAxeViolations(page, "tree-at-path (crates)");

    // Into crates/myelin-edge/ (deeper).
    await page.getByTestId("repo-tree").getByRole("link", { name: "myelin-edge" }).click();
    await page.waitForURL("**/git/repos/myelin/tree/refs%2Fheads%2Fmain/crates/myelin-edge");

    // Open lib.rs (a file → the nested blob route; contents are readable, not garbled).
    await page.getByTestId("repo-tree").getByRole("link", { name: "lib.rs" }).click();
    await page.waitForURL("**/git/repos/myelin/blob/refs%2Fheads%2Fmain/crates/myelin-edge/lib.rs");
    await expect(page.getByTestId("blob-contents")).toContainText("the product edge");
    // The Download affordance is present (gateway-proxied attachment).
    await expect(page.getByTestId("blob-download")).toBeVisible();
    await expectNoAxeViolations(page, "nested blob");
  });

  test("tree next-page follows the server cursor and renders the next UTF-8-ordered row", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/tree/refs%2Fheads%2Fmain?limit=1");

    const tree = page.getByTestId("repo-tree");
    await expect(tree.getByText("crates/", { exact: true })).toBeVisible();
    const next = page.getByTestId("tree-next-page");
    await expect(next).toBeVisible();
    await next.click();

    await expect(page).toHaveURL(/cursor=gt1_[A-Za-z0-9_-]+/);
    await expect(tree.getByText("A.txt", { exact: true })).toBeVisible();
    await expect(tree.getByText("crates/", { exact: true })).toHaveCount(0);
  });

  test("a stale tree cursor renders the 409 state and reload drops the cursor", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/tree/refs%2Fheads%2Fmain?limit=1");
    const href = await page.getByTestId("tree-next-page").getAttribute("href");
    if (!href) throw new Error("expected a tree continuation href");
    const target = new URL(href, "http://myelin.test");
    const cursor = target.searchParams.get("cursor");
    if (!cursor?.startsWith("gt1_")) throw new Error("expected an opaque tree cursor");
    const frame = JSON.parse(Buffer.from(cursor.slice("gt1_".length), "base64url").toString("utf8"));
    frame[4] = "1".repeat(40);
    target.searchParams.set(
      "cursor",
      `gt1_${Buffer.from(JSON.stringify(frame), "utf8").toString("base64url")}`,
    );

    await page.goto(`${target.pathname}${target.search}`);
    await expect(page.getByTestId("repo-error")).toHaveAttribute("data-kind", "stale-tree");
    await page.getByRole("link", { name: "Reload directory" }).click();

    await expect(page).not.toHaveURL(/(?:\?|&)cursor=/);
    await expect(page.getByTestId("repo-tree")).toBeVisible();
    await expect(page.getByTestId("repo-error")).toHaveCount(0);
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

  test("the catch-all route distinguishes a missing page from areas under construction", async ({ page }) => {
    await devLogin(page);
    await page.goto("/this/path/does/not/exist");
    await expect(page.getByTestId("not-available")).toBeVisible();
    await expect(page.getByTestId("availability-status")).toHaveText("Not found");
    await expect(page.getByText("Under construction", { exact: true })).toHaveCount(0);
    await expect(page.getByRole("navigation", { name: "Primary" })).toBeVisible();
  });

  test("graduated product areas no longer present a construction state", async ({ page }) => {
    await devLogin(page);
    await page.goto("/knowledge");
    await expect(page.getByTestId("knowledge-screen")).toBeVisible();
    await expect(page.getByTestId("availability-status")).toHaveCount(0);
    await expect(page.getByText("Under construction", { exact: true })).toHaveCount(0);
  });

  test("an expired access and refresh credential redirects before CSP-protected streaming", async ({ page }) => {
    await devLogin(page);
    const consoleErrors: string[] = [];
    page.on("console", (message) => {
      if (message.type() === "error") consoleErrors.push(message.text());
    });

    const forced = await page.request.post(`${EDGE}/__test/config`, {
      data: { forceUnauthorized: true },
    });
    expect(forced.ok()).toBe(true);
    await page.goto("/git/repos/myelin");
    await page.waitForURL("**/login");
    expect(consoleErrors.filter((message) => message.includes("Content Security Policy"))).toEqual([]);
  });
});
