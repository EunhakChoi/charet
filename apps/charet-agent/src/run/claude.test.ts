import assert from "node:assert/strict";
import { test } from "node:test";
import type { RunSpec } from "../protocol.ts";
import { claudeEnv } from "./claude.ts";

const spec: RunSpec = {
  id: 1,
  agent: "claude",
  model: "",
  apiKey: "sk-ant-api-key",
  project: "web",
  branch: 2,
  prompt: "go",
  context: null,
  history: [],
  images: [],
};

const login: RunSpec = {
  ...spec,
  apiKey: "",
  oauth: { provider: "anthropic", accessToken: "sk-ant-oat-token" },
};

const quiet = {
  DISABLE_TELEMETRY: "1",
  DISABLE_ERROR_REPORTING: "1",
  DISABLE_AUTOUPDATER: "1",
  CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
};

test("a run with a key gets it as ANTHROPIC_API_KEY, over the base environment", () => {
  const env = claudeEnv(spec, { PATH: "/bin", ANTHROPIC_API_KEY: "from-the-host" });
  assert.deepEqual(env, { PATH: "/bin", ANTHROPIC_API_KEY: "sk-ant-api-key", ...quiet });
});

test("a run with a login gets CLAUDE_CODE_OAUTH_TOKEN and no vendor key", () => {
  const env = claudeEnv(login, {
    PATH: "/bin",
    ANTHROPIC_API_KEY: "from-the-host",
    ANTHROPIC_AUTH_TOKEN: "from-the-host-too",
    CLAUDE_CODE_OAUTH_TOKEN: "stale",
  });
  assert.deepEqual(env, { PATH: "/bin", CLAUDE_CODE_OAUTH_TOKEN: "sk-ant-oat-token", ...quiet });
  assert.equal("ANTHROPIC_API_KEY" in env, false);
  assert.equal("ANTHROPIC_AUTH_TOKEN" in env, false);
});

test("a run with a login and an empty base has no ANTHROPIC_API_KEY, not even an empty one", () => {
  assert.equal("ANTHROPIC_API_KEY" in claudeEnv(login, {}), false);
});

test("the base environment is not changed", () => {
  const base = { ANTHROPIC_API_KEY: "from-the-host" };
  claudeEnv(login, base);
  assert.deepEqual(base, { ANTHROPIC_API_KEY: "from-the-host" });
});

test("a login of another vendor is refused", () => {
  const other: RunSpec = {
    ...login,
    oauth: { provider: "openai", accessToken: "a", idToken: "i", accountId: "c" },
  };
  assert.throws(() => claudeEnv(other, {}), /openai login/);
});
