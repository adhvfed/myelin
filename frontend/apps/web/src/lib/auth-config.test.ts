import { describe, expect, it } from "vitest";
import { toAuthConfig } from "./auth-config";

// The mapping behind `getAuthConfig` — the login page's render source. (The `getAuthConfig` query
// wrapper itself imports @solidjs/router, a client-only API that throws when imported in plain node,
// so — exactly like `devSeamAllowed`/`gateway-core` — the testable logic is this pure mapper.)
const devEnv = { NODE_ENV: "development", MYELIN_DEV_LOGIN: "1" };

describe("toAuthConfig — carries password_login_enabled (R4.0)", () => {
  it("advertises SSO only when both the edge verifier and web code flow are configured", () => {
    const edge = {
      sso_configured: true,
      providers: [{ id: "oidc", label: "Single sign-on" }],
    };
    expect(toAuthConfig(edge, {}, true, false)).toMatchObject({
      sso_configured: false,
      providers: [],
    });
    expect(toAuthConfig(edge, {}, true, true)).toMatchObject({
      sso_configured: true,
      providers: edge.providers,
    });
  });

  it("passes the edge's password_login_enabled=true through", () => {
    const cfg = toAuthConfig(
      { sso_configured: false, providers: [], dev_login_enabled: false, password_login_enabled: true },
      {},
      false,
    );
    expect(cfg.password_login_enabled).toBe(true);
  });

  it("defaults password_login_enabled to false when the edge omits it (fail-closed)", () => {
    const cfg = toAuthConfig({ sso_configured: false, providers: [] }, {}, false);
    expect(cfg.password_login_enabled).toBe(false);
  });

  it("fail-closed: the edge-unreachable stub ({}) yields password_login_enabled=false", () => {
    const cfg = toAuthConfig({}, {}, false);
    expect(cfg.password_login_enabled).toBe(false);
    expect(cfg.sso_configured).toBe(false);
  });

  it("password_login_enabled is INDEPENDENT of the dev-seam gates (a real path, no build/env kill switch)", () => {
    // A production build kills the dev seam, but password sign-in still follows the edge.
    const cfg = toAuthConfig(
      { password_login_enabled: true, dev_login_enabled: true },
      devEnv,
      /* isProdBuild */ true,
    );
    expect(cfg.dev_login_enabled).toBe(false); // dev seam killed in a prod build
    expect(cfg.password_login_enabled).toBe(true); // password sign-in unaffected
  });

  it("still composes the dev seam via devSeamAllowed (unchanged behaviour)", () => {
    expect(toAuthConfig({ dev_login_enabled: true }, devEnv, false).dev_login_enabled).toBe(true);
    expect(toAuthConfig({ dev_login_enabled: false }, devEnv, false).dev_login_enabled).toBe(false);
  });

  it.each([null, "enabled", [], 42])("is total over a non-object edge response: %j", (edge) => {
    expect(toAuthConfig(edge, devEnv, false, true)).toEqual({
      sso_configured: false,
      providers: [],
      dev_login_enabled: false,
      password_login_enabled: false,
      mail_delivery_enabled: false,
    });
  });

  it("requires exact booleans and projects only bounded provider labels", () => {
    const cfg = toAuthConfig({
      sso_configured: true,
      dev_login_enabled: "true",
      password_login_enabled: 1,
      providers: [
        { id: "oidc", label: "Work SSO" },
        { id: "Bad ID", label: "Invalid" },
        { id: "control", label: "Line\nBreak" },
        { id: "huge", label: "x".repeat(129) },
        "not-an-object",
      ],
    }, devEnv, false, true);

    expect(cfg).toEqual({
      sso_configured: true,
      providers: [{ id: "oidc", label: "Work SSO" }],
      dev_login_enabled: false,
      password_login_enabled: false,
      mail_delivery_enabled: false,
    });
  });

  it("reports mail delivery only alongside password sign-in", () => {
    expect(toAuthConfig({ password_login_enabled: true, mail_delivery_enabled: true }, {}, true))
      .toMatchObject({ mail_delivery_enabled: true });
    expect(toAuthConfig({ password_login_enabled: false, mail_delivery_enabled: true }, {}, true))
      .toMatchObject({ mail_delivery_enabled: false });
    expect(toAuthConfig({ password_login_enabled: true, mail_delivery_enabled: "yes" }, {}, true))
      .toMatchObject({ mail_delivery_enabled: false });
  });
});
