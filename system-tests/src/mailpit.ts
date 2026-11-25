// Reads the development inbox (Mailpit) that the edge sends to under `fed`. Mail arrives through
// the edge's outbox worker, so every read polls.

import { systemTestConfig } from "./config.js";
import { eventually } from "./eventually.js";
import { array, record, string } from "./json.js";

export interface Mail {
  subject: string;
  text: string;
  html: string;
  messageId: string;
}

async function mailpit(path: string): Promise<unknown> {
  const response = await fetch(new URL(path, systemTestConfig.mailpitUrl), {
    headers: { accept: "application/json" },
  });
  if (!response.ok) throw new Error(`Mailpit ${path} returned ${response.status}`);
  return response.json();
}

async function messagesTo(address: string): Promise<Mail[]> {
  const query = new URLSearchParams({ query: `to:"${address}"`, limit: "50" });
  const found = record(await mailpit(`/api/v1/search?${query}`), "Mailpit search");
  const ids = array(found.messages, "Mailpit messages").map((summary) =>
    string(record(summary, "Mailpit message").ID, "Mailpit message id"),
  );
  // Search lists the newest first; callers read in the order mail was sent.
  const mail = await Promise.all(ids.reverse().map(async (id) => {
    const message = record(await mailpit(`/api/v1/message/${encodeURIComponent(id)}`), "Mailpit message");
    return {
      subject: string(message.Subject, "subject"),
      text: string(message.Text, "text body"),
      html: string(message.HTML, "HTML body"),
      messageId: string(message.MessageID, "Message-ID"),
    };
  }));
  return mail;
}

/** Waits until `address` has received `count` messages and returns them, oldest first. */
export async function mailTo(address: string, count = 1): Promise<Mail[]> {
  return eventually(async () => {
    const mail = await messagesTo(address);
    return mail.length >= count ? mail : undefined;
  }, { description: `${count} message(s) to ${address} in Mailpit`, timeoutMs: 30_000, intervalMs: 250 });
}

/** Everything `address` has received so far, without waiting. */
export async function mailSoFar(address: string): Promise<Mail[]> {
  return messagesTo(address);
}

/** The one Myelin link in a message, as written out in its text. */
export function linkIn(mail: Mail): string {
  const links = mail.text.match(/https?:\/\/\S+\/join\/[A-Za-z0-9_-]{43}/g) ?? [];
  if (links.length !== 1) throw new Error(`expected one link in "${mail.subject}", found ${links.length}`);
  return links[0]!;
}
