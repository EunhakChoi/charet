import assert from "node:assert/strict";
import { test } from "node:test";
import { inputFiles } from "./launch.ts";
import type { RunStart } from "./protocol.ts";

const start: RunStart = {
  id: 7,
  agent: "codex",
  model: "m",
  apiKey: "",
  project: "web",
  branch: 2,
  token: "ab".repeat(16),
  prompt: "go",
  context: { screen: "home" },
  images: [],
  history: [],
};

function spec(run: RunStart): Record<string, unknown> {
  const file = inputFiles(run).find((f) => f.path === "spec.json");
  assert.ok(file, "spec.json");
  return JSON.parse(file.data.toString());
}

test("the spec holds a login, and never the agent door token", () => {
  const oauth = {
    provider: "openai",
    accessToken: "access",
    idToken: "id",
    accountId: "acct",
  } as const;
  const written = spec({ ...start, oauth });
  assert.deepEqual(written.oauth, oauth);
  assert.equal(written.apiKey, "");
  assert.equal("token" in written, false);
});

test("the spec of a run with a key has no oauth", () => {
  for (const run of [
    { ...start, apiKey: "k" },
    { ...start, apiKey: "k", oauth: null },
  ]) {
    const written = spec(run);
    assert.equal("oauth" in written, false);
    assert.equal(written.apiKey, "k");
  }
});
