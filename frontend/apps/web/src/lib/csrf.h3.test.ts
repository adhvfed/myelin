// The server-function guard decides from the raw request target what h3 will route to the
// server-function handler. This mounts the h3 the app runs on (through vinxi), the way Nitro
// mounts it, and checks the two agree for a fixed, seeded corpus of awkward targets, so an h3 or
// ufo upgrade that changes the decoding cannot silently reopen a way around the guard.
import net from "node:net";
import type { AddressInfo } from "node:net";
import { createServer, type Server } from "node:http";

import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { createApp, eventHandler, toNodeListener } from "vinxi/http";

import { refusesServerFunctionRead, routedPath } from "./csrf";

const TOKENS = [
  "/", "/", "/", "_", "server", "s", "%5F", "%5f", "%73", "%53", "%2F", "%2f", "%25", "%255F",
  "%25%35F", "..", ".", "%2e", "%2E", "\\", "%5C", ";", "x", "%00", "%", "%E0%A4%A", "%C0%AF",
  "//", "*", "+", "%20", "%3F", "%23", "%u005f", "%c3%a9", "%EF%BC%8F",
];
const SEEDS = [
  "/_server", "/_server/", "/%5Fserver/", "/%5fserver", "/_%73erver", "/_serv%65r", "/_serverx",
  "/_server.json", "/_server/x/", "/_server%2F", "/_server%2f", "/_server%252f", "/_server/../",
  "/_server;x", "//_server", "/\\_server", "/%2F_server", "/./_server", "/a/../_server",
  "/%25%35Fserver", "/_Server", "/profile", "/api/_server",
];

/** A small seeded generator, so every run checks the same targets. */
function seeded(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4_294_967_296;
  };
}

function corpus(size: number): string[] {
  const random = seeded(20_261_126);
  const targets = SEEDS.map((seed) => `${seed}?id=a&name=b`);
  const disguises = ["%5Fserver", "_%73erver", "_serv%65r"];
  for (let n = 0; n < size; n += 1) {
    let target = "/";
    const parts = 1 + Math.floor(random() * 7);
    for (let i = 0; i < parts; i += 1) target += TOKENS[Math.floor(random() * TOKENS.length)];
    if (random() < 0.6) target = `/_server${target.slice(1)}`;
    if (random() < 0.3) target = target.replace("_server", disguises[n % 3]!);
    targets.push(`${target}?id=a`);
  }
  return targets;
}

let server: Server;
let port: number;

beforeAll(async () => {
  const app = createApp();
  // Nitro's order: the server-function handler at /_server, then the page renderer at "/".
  app.use("/_server", eventHandler(() => "SERVERFN"));
  app.use("/", eventHandler(() => "PAGES"));
  server = createServer(toNodeListener(app));
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  port = (server.address() as AddressInfo).port;
});

afterAll(async () => {
  await new Promise((resolve) => server.close(resolve));
});

/** Sends `target` exactly as written (fetch would normalize it) and returns who handled it. */
function routedBy(target: string): Promise<string> {
  return new Promise((resolve) => {
    const socket = net.connect(port, "127.0.0.1");
    let response = "";
    socket.on("data", (chunk) => (response += chunk));
    socket.on("close", () => {
      const handler = /(SERVERFN|PAGES)/.exec(response)?.[1];
      resolve(handler ?? `OTHER ${response.split("\r\n")[0]}`);
    });
    socket.on("error", () => resolve("ERROR"));
    socket.write(`GET ${target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n`);
  });
}

describe("the server-function guard against real h3 routing", () => {
  it("refuses exactly the cross-site reads h3 would hand to the server-function handler", async () => {
    const disagreements: string[] = [];
    let reachedHandler = 0;
    for (const target of corpus(1_500)) {
      const handler = await routedBy(target);
      const refused = refusesServerFunctionRead({
        method: "GET",
        rawTarget: target,
        instance: null,
        fetchSite: "cross-site",
      });
      if (handler === "SERVERFN") reachedHandler += 1;
      // A target the HTTP parser rejects reaches neither handler and decides nothing.
      const routed = handler === "SERVERFN" || handler === "PAGES";
      if (routed && (handler === "SERVERFN") !== refused) {
        disagreements.push(
          `${JSON.stringify(target)} → ${handler}, refused=${refused}, routed as ${routedPath(target)}`,
        );
      }
    }
    expect(disagreements).toEqual([]);
    expect(reachedHandler, "the corpus exercises the handler").toBeGreaterThan(500);
  });
});
