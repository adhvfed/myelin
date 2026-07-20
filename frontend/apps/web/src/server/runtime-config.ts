import { sessionBackend } from "./session-backend";

/** Nitro startup plugin: reject a production process before it accepts traffic when durable session
 * storage is absent. Failing during a streamed auth loader would otherwise strand a partial 200. */
export default function validateRuntimeConfig(): void {
  sessionBackend(process.env.NODE_ENV === "production", process.env.REDIS_URL);
}
