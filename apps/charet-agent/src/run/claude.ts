// Claude Agent SDK (in the run image only).

import { readFileSync } from "node:fs";
import type { RunSpec } from "../protocol.ts";
import {
  type Agent,
  type AgentInput,
  emit,
  instructions,
  request,
  say,
  summary,
  tool,
} from "./common.ts";

interface ClaudeSdk {
  query(params: {
    prompt: AsyncIterable<unknown>;
    options: Record<string, unknown>;
  }): AsyncIterable<Record<string, unknown>>;
}

interface Block {
  type: string;
  text?: string;
  name?: string;
  input?: unknown;
}

const SDK = "@anthropic-ai/claude-agent-sdk";

/** Claude Code takes a credential from the first of these it finds, before `CLAUDE_CODE_OAUTH_TOKEN`. */
const OUTRANKING_CREDENTIALS = ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"];

/**
 * The environment Claude Code runs with, on top of `base`: the run's credential and the flags that
 * keep it quiet. A run with a subscription login (`spec.oauth`) gets `CLAUDE_CODE_OAUTH_TOKEN`, and
 * `base` loses the variables that would outrank it; else `ANTHROPIC_API_KEY` is the run's key.
 */
export function claudeEnv(
  spec: RunSpec,
  base: NodeJS.ProcessEnv,
): Record<string, string | undefined> {
  const quiet = {
    DISABLE_TELEMETRY: "1",
    DISABLE_ERROR_REPORTING: "1",
    DISABLE_AUTOUPDATER: "1",
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
  };
  if (!spec.oauth) return { ...base, ANTHROPIC_API_KEY: spec.apiKey, ...quiet };
  if (spec.oauth.provider !== "anthropic") {
    throw new Error(`a Claude run cannot use the ${spec.oauth.provider} login`);
  }
  const env = { ...base };
  for (const name of OUTRANKING_CREDENTIALS) delete env[name];
  return { ...env, CLAUDE_CODE_OAUTH_TOKEN: spec.oauth.accessToken, ...quiet };
}

export const claude: Agent = {
  async run({ spec, work, images, signal }: AgentInput): Promise<void> {
    const sdk = (await import(SDK)) as ClaudeSdk;
    const abortController = new AbortController();
    signal.addEventListener("abort", () => abortController.abort(), { once: true });
    const content = [
      ...images.map((image) => ({
        type: "image",
        source: {
          type: "base64",
          media_type: image.mediaType,
          data: readFileSync(image.path).toString("base64"),
        },
      })),
      { type: "text", text: request(spec) },
    ];
    async function* prompt() {
      yield {
        type: "user",
        message: { role: "user", content },
        parent_tool_use_id: null,
      };
    }
    const messages = sdk.query({
      prompt: prompt(),
      options: {
        cwd: work,
        ...(spec.model ? { model: spec.model } : {}),
        permissionMode: "bypassPermissions",
        allowDangerouslySkipPermissions: true,
        systemPrompt: { type: "preset", preset: "claude_code", append: instructions(spec) },
        settingSources: ["user"],
        skills: "all",
        abortController,
        maxTurns: 100,
        env: claudeEnv(spec, process.env),
      },
    });
    for await (const message of messages) {
      if (message.type === "assistant" && message.parent_tool_use_id === null) {
        const body = message.message as { content?: Block[] } | undefined;
        for (const block of body?.content ?? []) {
          if (block.type === "text" && block.text) say(block.text);
          if (block.type === "tool_use" && block.name) tool(block.name, summary(block.input));
        }
      }
      if (message.type === "result") {
        const usage = Object.values(
          (message.modelUsage ?? {}) as Record<
            string,
            {
              inputTokens: number;
              outputTokens: number;
              cacheReadInputTokens?: number;
              cacheCreationInputTokens?: number;
            }
          >,
        );
        emit({
          type: "usage",
          tokensIn: usage.reduce(
            (n, u) =>
              n + u.inputTokens + (u.cacheReadInputTokens ?? 0) + (u.cacheCreationInputTokens ?? 0),
            0,
          ),
          tokensOut: usage.reduce((n, u) => n + u.outputTokens, 0),
          costUsd: typeof message.total_cost_usd === "number" ? message.total_cost_usd : null,
        });
        if (message.is_error) {
          const errors = (message.errors as string[] | undefined) ?? [];
          throw new Error(
            errors.join("; ") ||
              (typeof message.result === "string" ? message.result : String(message.subtype)),
          );
        }
      }
    }
  },
};
