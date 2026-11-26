// The browser's device tokens: proof, from the edge, that this browser has signed in to an email
// before. They carry no authority; they only let this browser's sign-ins skip the allowance that
// strangers guessing at the email share. One cookie keeps a few, one per email, newest first.

import { createHmac } from "node:crypto";

export const MAX_DEVICE_TOKENS = 5;
const DEVICE_COOKIE_MAX_AGE_SECONDS = 365 * 24 * 60 * 60;

export interface DeviceCookieSettings {
  name: string;
  secure: boolean;
  maxAgeSeconds: number;
}

export function deviceCookieSettings(production: boolean): DeviceCookieSettings {
  return {
    name: production ? "__Host-myelin_device" : "myelin_device",
    secure: production,
    maxAgeSeconds: DEVICE_COOKIE_MAX_AGE_SECONDS,
  };
}

export function validDeviceToken(value: unknown): value is string {
  return typeof value === "string" && /^[A-Za-z0-9_-]{16,256}$/.test(value);
}

/** Entries are found by an HMAC of the email under the web server's secret, so someone reading
 * the cookie cannot test it against a list of addresses. */
function emailKey(secret: Uint8Array, email: string): string {
  return createHmac("sha256", secret)
    .update(email.trim().toLowerCase())
    .digest("base64url")
    .slice(0, 22);
}

function entries(cookie: string | undefined): [string, string][] {
  if (!cookie) return [];
  return cookie
    .split(".")
    .map((entry) => entry.split(":"))
    .filter((parts): parts is [string, string] =>
      parts.length === 2 && /^[A-Za-z0-9_-]{22}$/.test(parts[0]!) && validDeviceToken(parts[1]),
    )
    .slice(0, MAX_DEVICE_TOKENS);
}

function serialize(list: [string, string][]): string {
  return list.map(([key, token]) => `${key}:${token}`).join(".");
}

export function deviceTokenFor(
  cookie: string | undefined,
  secret: Uint8Array,
  email: string,
): string | undefined {
  const key = emailKey(secret, email);
  return entries(cookie).find(([entryKey]) => entryKey === key)?.[1];
}

/** The cookie with `token` as the newest entry for `email`, the oldest dropped past the limit. */
export function withDeviceToken(
  cookie: string | undefined,
  secret: Uint8Array,
  email: string,
  token: string,
): string {
  const key = emailKey(secret, email);
  return serialize(
    [[key, token] as [string, string], ...entries(cookie).filter(([entryKey]) => entryKey !== key)]
      .slice(0, MAX_DEVICE_TOKENS),
  );
}

/** The cookie without `email`'s entry; `undefined` when nothing is left. */
export function withoutDeviceToken(
  cookie: string | undefined,
  secret: Uint8Array,
  email: string,
): string | undefined {
  const key = emailKey(secret, email);
  const rest = entries(cookie).filter(([entryKey]) => entryKey !== key);
  return rest.length ? serialize(rest) : undefined;
}
