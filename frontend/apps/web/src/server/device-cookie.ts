// The browser's device tokens: proof, from the edge, that this browser has signed in to an email
// before. They carry no authority; they only let this browser's sign-ins skip the allowance that
// strangers guessing at the email share. One cookie keeps a few, one per email, newest first.

import { createHash } from "node:crypto";

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

/** Entries are found by a hash of the email, so the cookie does not spell out who signed in. */
function emailKey(email: string): string {
  return createHash("sha256").update(email.trim().toLowerCase()).digest("base64url").slice(0, 22);
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

export function deviceTokenFor(cookie: string | undefined, email: string): string | undefined {
  const key = emailKey(email);
  return entries(cookie).find(([entryKey]) => entryKey === key)?.[1];
}

/** The cookie with `token` as the newest entry for `email`, the oldest dropped past the limit. */
export function withDeviceToken(cookie: string | undefined, email: string, token: string): string {
  const key = emailKey(email);
  return [[key, token], ...entries(cookie).filter(([entryKey]) => entryKey !== key)]
    .slice(0, MAX_DEVICE_TOKENS)
    .map(([entryKey, entryToken]) => `${entryKey}:${entryToken}`)
    .join(".");
}
