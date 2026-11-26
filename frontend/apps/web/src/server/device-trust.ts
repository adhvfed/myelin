// Reads and writes this browser's device cookie (see device-cookie.ts). Server-only.

import { getCookie, setCookie } from "vinxi/http";
import {
  deviceCookieSettings,
  deviceTokenFor,
  validDeviceToken,
  withDeviceToken,
} from "./device-cookie";

const cookie = deviceCookieSettings(import.meta.env.PROD);

export function deviceTokenForEmail(email: string): string | undefined {
  return deviceTokenFor(getCookie(cookie.name), email);
}

export function rememberDevice(email: string, token: string): void {
  if (!validDeviceToken(token)) return;
  setCookie(cookie.name, withDeviceToken(getCookie(cookie.name), email, token), {
    httpOnly: true,
    sameSite: "lax",
    path: "/",
    secure: cookie.secure,
    maxAge: cookie.maxAgeSeconds,
  });
}
