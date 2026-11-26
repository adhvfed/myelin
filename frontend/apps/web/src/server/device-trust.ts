// Reads and writes this browser's device cookie (see device-cookie.ts). Server-only.
//
// Two tabs signing in at once each rewrite the cookie from what they read, so one tab's new
// entry can be lost. That is accepted: the cost is one browser treated as new until its next
// sign-in, never a wrong token sent for an email.

import { createHmac, randomBytes } from "node:crypto";
import { deleteCookie, getCookie, setCookie } from "vinxi/http";
import {
  deviceCookieSettings,
  deviceTokenFor,
  validDeviceToken,
  withDeviceToken,
  withoutDeviceToken,
} from "./device-cookie";
import { edgeWhoami } from "./gateway";
import { decodeSessionKey } from "./session-cipher";

const cookie = deviceCookieSettings(import.meta.env.PROD);
const SECRET_LABEL = "myelin web device cookie email key v1";

/** The key the cookie's entries are found by, derived from the session key under its own label.
 * Without a session key (local development) it is random per process, so a restart forgets. The
 * global keeps one value across Vinxi's separate module graphs. */
function secret(): Buffer {
  const holder = globalThis as unknown as { __myelinDeviceCookieSecret?: Buffer };
  holder.__myelinDeviceCookieSecret ??= process.env.MYELIN_WEB_SESSION_KEY?.trim()
    ? createHmac("sha256", decodeSessionKey(process.env.MYELIN_WEB_SESSION_KEY))
      .update(SECRET_LABEL)
      .digest()
    : randomBytes(32);
  return holder.__myelinDeviceCookieSecret;
}

const attributes = () => ({
  httpOnly: true,
  sameSite: "lax" as const,
  path: "/",
  secure: cookie.secure,
});

export function deviceTokenForEmail(email: string): string | undefined {
  return deviceTokenFor(getCookie(cookie.name), secret(), email);
}

export function rememberDevice(email: string, token: string): void {
  if (!validDeviceToken(token)) return;
  setCookie(cookie.name, withDeviceToken(getCookie(cookie.name), secret(), email, token), {
    ...attributes(),
    maxAge: cookie.maxAgeSeconds,
  });
}

/** Drops `email`'s entry, and the cookie itself once it holds nothing else. */
export function forgetDevice(email: string): void {
  const rest = withoutDeviceToken(getCookie(cookie.name), secret(), email);
  if (rest) setCookie(cookie.name, rest, { ...attributes(), maxAge: cookie.maxAgeSeconds });
  else deleteCookie(cookie.name, attributes());
}

/** The signed-in person's email, if the edge says; `undefined` when it cannot be asked. */
export async function viewerEmail(): Promise<string | undefined> {
  try {
    return (await edgeWhoami()).email || undefined;
  } catch {
    return undefined;
  }
}

/** The device token this browser holds for the signed-in person, if any. */
export async function deviceTokenForViewer(): Promise<string | undefined> {
  const email = await viewerEmail();
  return email ? deviceTokenForEmail(email) : undefined;
}
