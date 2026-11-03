// The credentials a run carries, and keeping them out of anything that is reported.
//
// Runs report through events and error text that charet-server stores and shows; a vendor key or
// login token in there would outlive the run. Whatever prints text for the server passes it
// through `scrub` first.

import type { RunOAuth } from "./protocol.ts";

/** What replaces a credential in text. */
export const HIDDEN = "[hidden]";

/** Shorter values are not credentials; hiding them would mangle ordinary text. */
const MIN_LENGTH = 8;

/** The values of a run's credentials: the vendor key, or the login's tokens. */
export function secretsOf(credentials: { apiKey?: string; oauth?: RunOAuth | null }): string[] {
  const values = [credentials.apiKey];
  const oauth = credentials.oauth;
  if (oauth) {
    values.push(oauth.accessToken);
    if (oauth.provider === "openai") values.push(oauth.idToken);
  }
  return values.filter((v): v is string => typeof v === "string" && v.length >= MIN_LENGTH);
}

/** `text` with every occurrence of the secrets (also as JSON string content) replaced. */
export function scrub(text: string, secrets: readonly string[]): string {
  let clean = text;
  for (const secret of secrets) {
    clean = clean.replaceAll(secret, HIDDEN);
    clean = clean.replaceAll(JSON.stringify(secret).slice(1, -1), HIDDEN);
  }
  return clean;
}
