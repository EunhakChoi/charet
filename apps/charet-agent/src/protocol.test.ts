import assert from "node:assert/strict";
import { test } from "node:test";
import { isRunStart, type RunOAuth } from "./protocol.ts";

const base = {
  id: 1,
  agent: "claude",
  model: "",
  apiKey: "",
  project: "web",
  branch: 2,
  token: "ab".repeat(16),
  prompt: "go",
  context: null,
  images: [],
  history: [],
};

const anthropic: RunOAuth = { provider: "anthropic", accessToken: "sk-ant-oat-token" };
const openai: RunOAuth = {
  provider: "openai",
  accessToken: "access",
  idToken: "id",
  accountId: "acct",
};

test("a run with a key and no login is a run, with oauth absent or null", () => {
  assert.equal(isRunStart({ ...base, apiKey: "k" }), true);
  assert.equal(isRunStart({ ...base, apiKey: "k", oauth: null }), true);
  assert.equal(isRunStart({ ...base, agent: "script" }), true);
});

test("a login of the run's vendor is a run", () => {
  assert.equal(isRunStart({ ...base, agent: "claude", oauth: anthropic }), true);
  assert.equal(isRunStart({ ...base, agent: "codex", oauth: openai }), true);
});

test("a login of another vendor, or on the script agent, is not a run", () => {
  assert.equal(isRunStart({ ...base, agent: "claude", oauth: openai }), false);
  assert.equal(isRunStart({ ...base, agent: "codex", oauth: anthropic }), false);
  assert.equal(isRunStart({ ...base, agent: "script", oauth: anthropic }), false);
  assert.equal(isRunStart({ ...base, agent: "script", oauth: openai }), false);
});

test("a login needs every token, as a non-empty string", () => {
  const codexRun = (change: object) => ({
    ...base,
    agent: "codex",
    oauth: { ...openai, ...change },
  });
  assert.equal(isRunStart(codexRun({})), true);
  for (const field of ["accessToken", "idToken", "accountId"]) {
    assert.equal(isRunStart(codexRun({ [field]: "" })), false, `${field} empty`);
    assert.equal(isRunStart(codexRun({ [field]: 7 })), false, `${field} a number`);
    assert.equal(isRunStart(codexRun({ [field]: undefined })), false, `${field} missing`);
  }
  assert.equal(isRunStart({ ...base, oauth: { ...anthropic, accessToken: "" } }), false);
  assert.equal(isRunStart({ ...base, oauth: { provider: "anthropic" } }), false);
  assert.equal(isRunStart({ ...base, oauth: { accessToken: "t" } }), false);
});

test("a login is an object, and not next to a vendor key", () => {
  assert.equal(isRunStart({ ...base, oauth: "token" }), false);
  assert.equal(isRunStart({ ...base, oauth: [] }), false);
  assert.equal(isRunStart({ ...base, apiKey: "k", oauth: anthropic }), false);
});
