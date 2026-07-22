import { test, expect, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";

const APP = `http://localhost:${process.env.PORT ?? 3000}`;

// A real-browser axe assertion on the live DOM (the thing jsdom couldn't do for the overlays). We scan
// the rendered page against WCAG 2.0/2.1 A+AA tags; a violation fails the gate loudly.
async function expectNoAxeViolations(page: Page, context?: string) {
  const results = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
    .analyze();
  expect(results.violations, `axe violations${context ? ` on ${context}` : ""}: ${JSON.stringify(results.violations, null, 2)}`).toEqual([]);
}

async function devLogin(page: Page) {
  await page.goto("/login");
  // SolidStart installs the server-action submission during hydration. A pre-hydration click can
  // remain on /login even though the button is already painted by SSR.
  await page.waitForLoadState("networkidle");
  await page.getByTestId("dev-login").click();
  await page.waitForURL("**/git/repos");
  // URL commit and SSR visibility can precede Solid's client mount. This marker flips only after
  // the document shortcut listener is installed, so keyboard assertions cannot race hydration.
  await expect(page.locator('.app-shell[data-shortcuts-ready="true"]')).toBeVisible();
}

test.describe("MR-019 app shell — real browser", () => {
  test("unsafe requests fail closed unless their full origin matches", async ({ page }) => {
    const missing = await page.request.post("/login");
    expect(missing.status()).toBe(403);
    expect(missing.headers()["x-content-type-options"]).toBe("nosniff");

    const crossOrigin = await page.request.post("/login", {
      headers: { Origin: "https://evil.example" },
    });
    expect(crossOrigin.status()).toBe(403);

    const schemeDowngrade = await page.request.put("/login", {
      headers: { Origin: APP.replace("http://", "https://") },
    });
    expect(schemeDowngrade.status()).toBe(403);

    const customUnsafeMethod = await page.request.fetch("/login", {
      method: "PURGE",
      headers: { Origin: "https://evil.example" },
    });
    expect(customUnsafeMethod.status()).toBe(403);

    const sameOrigin = await page.request.post("/login", {
      headers: { Origin: APP },
    });
    expect(sameOrigin.status()).not.toBe(403);

    const sameOriginReferer = await page.request.post("/login", {
      headers: { Referer: `${APP}/login` },
    });
    expect(sameOriginReferer.status()).not.toBe(403);
  });

  test("liveness and session-backed readiness are explicit and non-cacheable", async ({ page }) => {
    const health = await page.request.get("/healthz");
    expect(health.status()).toBe(200);
    expect(await health.json()).toEqual({ status: "ok" });

    const readiness = await page.request.get("/readyz");
    expect(readiness.status()).toBe(200);
    expect(await readiness.json()).toEqual({ status: "ready" });
    expect(readiness.headers()["cache-control"]).toBe("no-store");
    expect(readiness.headers()["x-content-type-options"]).toBe("nosniff");
  });

  test("SSR scripts carry a fresh CSP nonce on every response", async ({ page }) => {
    const observedNonces: string[] = [];
    for (let requestIndex = 0; requestIndex < 2; requestIndex++) {
      const response = await page.request.get("/login");
      const html = await response.text();
      const policy = response.headers()["content-security-policy"];
      const nonce = policy?.match(/'nonce-([^']+)'/)?.[1];
      const scriptTags = [...html.matchAll(/<script\b[^>]*>/g)].map((match) => match[0]);

      expect(nonce).toBeTruthy();
      expect(scriptTags.length).toBeGreaterThan(0);
      expect(scriptTags.every((tag) => tag.includes(`nonce="${nonce}"`))).toBe(true);
      observedNonces.push(nonce!);
    }
    expect(observedNonces[0]).not.toBe(observedNonces[1]);
  });

  test("unauthenticated /git/repos redirects to /login (the 401→/login floor)", async ({ page }) => {
    await page.goto("/git/repos");
    await page.waitForURL("**/login");
    await expect(page).toHaveTitle("Sign in · Myelin");
    await expect(page.getByTestId("dev-login")).toBeVisible();
    await expectNoAxeViolations(page, "/login");
  });

  test("dev login → the repos screen renders the edge ViewModel JSON (shell→gateway→edge→ViewModel)", async ({ page }) => {
    await devLogin(page);

    // The repos list is the real edge RepoHome ViewModel projection.
    await expect(page).toHaveTitle("Code · Myelin");
    await expect(page.getByTestId("repos-list")).toBeVisible();
    await expect(page.getByText("acme/myelin", { exact: true })).toBeVisible();
    await expect(page.getByText("The make-it-real spine.")).toBeVisible(); // the README excerpt field
    await expect(page.getByText("ssh://git@myelin/acme/myelin.git")).toBeVisible();
    // The empty-state repo (an unglamorous state served by the same envelope).
    await expect(page.getByText("acme/sandbox", { exact: true })).toBeVisible();
    await expect(page.getByText("empty · push to get started")).toBeVisible();

    // The chrome is present: residency cue (data region from whoami/session), identity, inbox.
    await expect(page.getByText("Data region:")).toBeVisible();
    await expect(page.getByText("eu-west")).toBeVisible();
    await expect(page.getByRole("button", { name: /Inbox/ })).toBeVisible();

    await expectNoAxeViolations(page, "the authenticated shell + repos screen");
  });

  test("the session cookie is opaque and re-authentication revokes the prior id", async ({ page }) => {
    await devLogin(page);
    const first = (await page.context().cookies()).find((cookie) => cookie.name === "myelin_session");
    expect(first).toMatchObject({
      httpOnly: true,
      sameSite: "Lax",
      secure: false,
      path: "/",
    });
    expect(first?.value).toMatch(/^sess_[A-Za-z0-9_-]{32}$/);
    expect(first!.expires - Date.now() / 1_000).toBeGreaterThan(7 * 60 * 60);

    await page.goto("/login");
    await page.waitForLoadState("networkidle");
    await page.getByTestId("dev-login").click();
    await page.waitForURL("**/git/repos");
    const second = (await page.context().cookies()).find((cookie) => cookie.name === "myelin_session");
    expect(second?.value).not.toBe(first?.value);

    const staleContext = await page.context().browser()!.newContext();
    try {
      await staleContext.addCookies([{ ...first!, expires: -1 }]);
      const stalePage = await staleContext.newPage();
      await stalePage.goto(`${APP}/git/repos`);
      await stalePage.waitForURL("**/login");
    } finally {
      await staleContext.close();
    }
  });

  test("⌘K opens the command palette (focus in the search), Escape closes it", async ({ page }) => {
    await devLogin(page);

    await page.keyboard.press("ControlOrMeta+k");
    const dialog = page.getByRole("dialog", { name: "Command palette" });
    await expect(dialog).toBeVisible();

    // Focus moved into the palette's search input (the combobox), per the Dialog focus-trap.
    const search = page.getByRole("combobox", { name: /Search or run a command/ });
    await expect(search).toBeFocused();

    // axe on the OPEN overlay — the real-browser check the overlays could not get under jsdom (MR-017).
    await expectNoAxeViolations(page, "the command palette (open)");

    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
  });

  test("the ⌘K trigger is keyboard-operable and Escape returns focus to it", async ({ page }) => {
    await devLogin(page);

    const trigger = page.getByRole("button", { name: /Search or run a command/ });
    await trigger.focus();
    await expect(trigger).toBeFocused();
    await page.keyboard.press("Enter"); // activate the trigger from the keyboard

    const dialog = page.getByRole("dialog", { name: "Command palette" });
    await expect(dialog).toBeVisible();

    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
    // Return-focus to the trigger (the Dialog primitive's return-focus mechanic).
    await expect(trigger).toBeFocused();
  });

  test("the autofocused palette input keeps the shared focus ring (no inline outline:none)", async ({ page }) => {
    await devLogin(page);
    await page.keyboard.press("ControlOrMeta+k");
    const search = page.getByRole("combobox", { name: /Search or run a command/ });
    await expect(search).toBeFocused();
    // Manual must-ship #5: the palette input must NOT zero its outline inline — the shared
    // zero-specificity :focus-visible ring must be able to paint on the flagship keyboard surface.
    expect(await search.evaluate((el) => (el as HTMLElement).style.outline)).toBe("");
    expect(await search.evaluate((el) => getComputedStyle(el).outlineWidth)).not.toBe("0px");
  });

  test("the active nav rail item is a --surface-hover fill, never an --accent fill (R1 binding)", async ({ page }) => {
    await devLogin(page);
    const active = page.locator('a.nav-rail-item[aria-current="page"]').first();
    await expect(active).toBeVisible();
    const { got, hover, accent } = await page.evaluate(() => {
      const link = document.querySelector('a.nav-rail-item[aria-current="page"]') as HTMLElement;
      const probe = document.createElement("div");
      document.body.appendChild(probe);
      probe.style.background = "var(--surface-hover)";
      const hover = getComputedStyle(probe).backgroundColor;
      probe.style.background = "var(--accent)";
      const accent = getComputedStyle(probe).backgroundColor;
      probe.remove();
      return { got: getComputedStyle(link).backgroundColor, hover, accent };
    });
    expect(got).toBe(hover); // active fill = --surface-hover
    expect(got).not.toBe(accent); // never the saturated accent tile
  });

  test("a command runs in-place: type 'inbox' + Enter opens the inbox overlay", async ({ page }) => {
    await devLogin(page);
    await page.keyboard.press("ControlOrMeta+k");
    const search = page.getByRole("combobox", { name: /Search or run a command/ });
    await search.fill("inbox");
    await page.keyboard.press("Enter");
    await expect(page.getByRole("dialog", { name: "Inbox" })).toBeVisible();
  });
});
