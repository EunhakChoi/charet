// Codex SDK (in the run image only).

import { chmodSync, mkdirSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import type { RunOAuth, RunSpec } from "../protocol.ts";
import { type Agent, type AgentInput, emit, instructions, request, say, tool } from "./common.ts";

type OpenAiOAuth = Extract<RunOAuth, { provider: "openai" }>;

interface CodexSdk {
  Codex: new (
    options: Record<string, unknown>,
  ) => {
    startThread(options: Record<string, unknown>): {
      runStreamed(
        input: unknown,
        options: { signal: AbortSignal },
      ): Promise<{ events: AsyncIterable<CodexEvent> }>;
    };
  };
}

interface CodexEvent {
  type: string;
  message?: string;
  error?: { message: string };
  usage?: { input_tokens: number; cached_input_tokens: number; output_tokens: number };
  item?: {
    type: string;
    text?: string;
    command?: string;
    changes?: { path: string; kind: string }[];
    message?: string;
  };
}

const SDK = "@openai/codex-sdk";

/** Environment variables that give Codex a vendor key, which wins over a login file. */
const OUTRANKING_CREDENTIALS = ["CODEX_API_KEY", "OPENAI_API_KEY"];

/**
 * The login file in Codex's `auth.json` format, for a run with a ChatGPT subscription.
 *
 * The refresh token is a placeholder on purpose: the real one never enters the run. charet-server
 * refreshes the login and hands each run a fresh access token, so a run has nothing to refresh
 * (and when its token runs out, Codex's refresh fails and the run ends).
 */
export function codexAuthFile(oauth: OpenAiOAuth, now: Date): string {
  return JSON.stringify({
    auth_mode: "chatgpt",
    OPENAI_API_KEY: null,
    tokens: {
      id_token: oauth.idToken,
      access_token: oauth.accessToken,
      refresh_token: "charet-managed",
      account_id: oauth.accountId,
    },
    last_refresh: now.toISOString(),
  });
}

/**
 * What `new Codex(...)` gets for a run, on top of `base` as its environment.
 *
 * With a key (`spec.apiKey`) that is the key. With a subscription login (`spec.oauth`) there is no
 * key anywhere in the environment: the login is written to `auth.json` in Codex's home, which is
 * `<home>/.codex` (private, mode 0700; it exists before Codex starts, as `CODEX_HOME` must). That
 * is Codex's default home, so the skills the run image keeps there stay found.
 */
export function codexOptions(
  spec: RunSpec,
  base: NodeJS.ProcessEnv,
  home: string,
  now = new Date(),
): Record<string, unknown> {
  const env: Record<string, string> = {};
  for (const [key, value] of Object.entries(base)) if (value !== undefined) env[key] = value;
  if (!spec.oauth) return { apiKey: spec.apiKey, env };
  if (spec.oauth.provider !== "openai") {
    throw new Error(`a Codex run cannot use the ${spec.oauth.provider} login`);
  }
  for (const name of OUTRANKING_CREDENTIALS) delete env[name];
  const codexHome = join(home, ".codex");
  mkdirSync(codexHome, { recursive: true, mode: 0o700 });
  chmodSync(codexHome, 0o700);
  const file = join(codexHome, "auth.json");
  writeFileSync(file, codexAuthFile(spec.oauth, now), { mode: 0o600 });
  chmodSync(file, 0o600);
  return {
    env: { ...env, CODEX_HOME: codexHome },
    // Passed as `--config cli_auth_credentials_store="file"`: read the login from `auth.json`.
    config: { cli_auth_credentials_store: "file" },
  };
}

export const codex: Agent = {
  async run({ spec, work, images, signal }: AgentInput): Promise<void> {
    const sdk = (await import(SDK)) as CodexSdk;
    const client = new sdk.Codex(codexOptions(spec, process.env, homedir()));
    const thread = client.startThread({
      workingDirectory: work,
      skipGitRepoCheck: true,
      // The run's container is the sandbox.
      sandboxMode: "danger-full-access",
      approvalPolicy: "never",
      networkAccessEnabled: true,
      ...(spec.model ? { model: spec.model } : {}),
    });
    const input = [
      { type: "text", text: `${instructions(spec)}\n\n---\n\n${request(spec)}` },
      ...images.map((image) => ({ type: "local_image", path: image.path })),
    ];
    const { events } = await thread.runStreamed(input, { signal });
    let tokensIn = 0;
    let cachedIn = 0;
    let tokensOut = 0;
    for await (const event of events) {
      if (event.type === "item.completed" && event.item) {
        const item = event.item;
        if (item.type === "agent_message" && item.text) say(item.text);
        if (item.type === "command_execution" && item.command) tool("shell", item.command);
        if (item.type === "file_change") {
          tool("edit", (item.changes ?? []).map((c) => `${c.kind} ${c.path}`).join(", "));
        }
        if (item.type === "error" && item.message) tool("error", item.message);
      }
      if (event.type === "turn.completed" && event.usage) {
        tokensIn += event.usage.input_tokens;
        cachedIn += event.usage.cached_input_tokens;
        tokensOut += event.usage.output_tokens;
        emit({
          type: "usage",
          tokensIn,
          tokensOut,
          cachedIn,
          costUsd: null,
          ...(spec.model ? { model: spec.model } : {}),
        });
      }
      if (event.type === "turn.failed") throw new Error(event.error?.message ?? "the turn failed");
      if (event.type === "error") throw new Error(event.message ?? "codex failed");
    }
  },
};
