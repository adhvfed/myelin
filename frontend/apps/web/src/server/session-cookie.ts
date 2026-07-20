import { SESSION_ABSOLUTE_TTL_MS } from "./session-store";

export interface SessionCookieSettings {
  name: string;
  secure: boolean;
  maxAgeSeconds: number;
}

export function sessionCookieSettings(production: boolean): SessionCookieSettings {
  return {
    name: production ? "__Host-myelin_session" : "myelin_session",
    secure: production,
    maxAgeSeconds: SESSION_ABSOLUTE_TTL_MS / 1_000,
  };
}
