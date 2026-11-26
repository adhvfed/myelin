import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const jar = vi.hoisted(() => new Map<string, string>());
const http = vi.hoisted(() => ({
  getCookie: vi.fn((name: string) => jar.get(name)),
  setCookie: vi.fn((name: string, value: string) => void jar.set(name, value)),
  deleteCookie: vi.fn((name: string) => void jar.delete(name)),
}));
const gateway = vi.hoisted(() => ({ edgeWhoami: vi.fn() }));

vi.mock("vinxi/http", () => http);
vi.mock("./gateway", () => gateway);

import {
  deviceTokenForEmail,
  deviceTokenForViewer,
  forgetDevice,
  rememberDevice,
} from "./device-trust";

const TOKEN = "device-token-00000001";

beforeEach(() => jar.clear());
afterEach(() => vi.clearAllMocks());

describe("device trust", () => {
  it("remembers a token in an HttpOnly, SameSite=Lax cookie that lasts a year", () => {
    rememberDevice("ada@example.com", TOKEN);
    expect(http.setCookie).toHaveBeenCalledWith("myelin_device", expect.any(String), {
      httpOnly: true,
      sameSite: "lax",
      path: "/",
      secure: false,
      maxAge: 31_536_000,
    });
    expect(deviceTokenForEmail(" Ada@Example.com")).toBe(TOKEN);
    expect(deviceTokenForEmail("grace@example.com")).toBeUndefined();
  });

  it("ignores a token that is not one", () => {
    rememberDevice("ada@example.com", "not a token!");
    expect(http.setCookie).not.toHaveBeenCalled();
  });

  it("finds the signed-in person's token through the edge, and none when it cannot ask", async () => {
    rememberDevice("ada@example.com", TOKEN);
    gateway.edgeWhoami.mockResolvedValueOnce({ email: "ada@example.com" });
    expect(await deviceTokenForViewer()).toBe(TOKEN);
    gateway.edgeWhoami.mockResolvedValueOnce({ email: "" });
    expect(await deviceTokenForViewer()).toBeUndefined();
    gateway.edgeWhoami.mockRejectedValueOnce(new Error("edge down"));
    expect(await deviceTokenForViewer()).toBeUndefined();
  });

  it("forgets one email, and deletes the cookie with the last", () => {
    rememberDevice("ada@example.com", TOKEN);
    rememberDevice("grace@example.com", "device-token-00000002");
    forgetDevice("ada@example.com");
    expect(deviceTokenForEmail("ada@example.com")).toBeUndefined();
    expect(deviceTokenForEmail("grace@example.com")).toBe("device-token-00000002");
    expect(http.deleteCookie).not.toHaveBeenCalled();
    forgetDevice("grace@example.com");
    expect(http.deleteCookie).toHaveBeenCalledWith("myelin_device", {
      httpOnly: true,
      sameSite: "lax",
      path: "/",
      secure: false,
    });
    expect(jar.has("myelin_device")).toBe(false);
  });
});
