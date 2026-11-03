import assert from "node:assert/strict";
import { test } from "node:test";
import { HIDDEN, scrub, secretsOf } from "./secrets.ts";

test("a run's secrets are its key, or its login's tokens", () => {
  assert.deepEqual(secretsOf({ apiKey: "sk-ant-api-key" }), ["sk-ant-api-key"]);
  assert.deepEqual(
    secretsOf({ apiKey: "", oauth: { provider: "anthropic", accessToken: "sk-ant-oat" } }),
    ["sk-ant-oat"],
  );
  assert.deepEqual(
    secretsOf({
      apiKey: "",
      oauth: {
        provider: "openai",
        accessToken: "access-token",
        idToken: "id-token-x",
        accountId: "acct-123",
      },
    }),
    ["access-token", "id-token-x"],
  );
});

test("values too short to be a credential are not secrets", () => {
  assert.deepEqual(secretsOf({ apiKey: "k" }), []);
  assert.deepEqual(secretsOf({}), []);
});

test("scrub replaces every occurrence, also inside JSON", () => {
  const secrets = ["sk-ant-oat-token", 'we"ird\\token'];
  assert.equal(scrub("a sk-ant-oat-token b sk-ant-oat-token", secrets), `a ${HIDDEN} b ${HIDDEN}`);
  const line = JSON.stringify({ text: 'bad we"ird\\token here' });
  assert.deepEqual(JSON.parse(scrub(line, secrets)), { text: `bad ${HIDDEN} here` });
});

test("scrub leaves text without secrets alone", () => {
  assert.equal(scrub("nothing to hide", ["sk-ant-oat-token"]), "nothing to hide");
  assert.equal(scrub("nothing to hide", []), "nothing to hide");
});
