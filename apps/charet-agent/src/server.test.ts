import assert from "node:assert/strict";
import type { AddressInfo } from "node:net";
import { after, before, test } from "node:test";
import type { RunStart } from "./protocol.ts";
import type { Runs } from "./runs.ts";
import { createAgentServer } from "./server.ts";

const started: RunStart[] = [];
const runs = {
  async start(run: RunStart) {
    started.push(run);
  },
  active: () => 0,
} as unknown as Runs;

const server = createAgentServer({
  runs,
  token: undefined,
  version: "test",
  ready: async () => undefined,
});
let url = "";

before(async () => {
  await new Promise<void>((done) => server.listen(0, "127.0.0.1", done));
  url = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});

after(() => {
  server.close();
});

const run = {
  id: 1,
  agent: "codex",
  model: "",
  apiKey: "",
  oauth: { provider: "openai", accessToken: "access", idToken: "id", accountId: "acct" },
  project: "web",
  branch: 2,
  token: "ab".repeat(16),
  prompt: "go",
  context: null,
  images: [],
  history: [],
};

function post(body: string): Promise<Response> {
  return fetch(`${url}/v1/runs`, { method: "POST", body });
}

test("starts a run that comes with a login", async () => {
  const res = await post(JSON.stringify(run));
  assert.equal(res.status, 202);
  assert.deepEqual(started.at(-1)?.oauth, run.oauth);
});

test("refuses a login of the wrong vendor", async () => {
  const count = started.length;
  const res = await post(JSON.stringify({ ...run, agent: "claude" }));
  assert.equal(res.status, 400);
  assert.equal(started.length, count);
});

test("an answer to a body that is not JSON does not quote the body", async () => {
  const token = "sk-ant-oat-0123456789abcdef";
  const res = await post(`{"oauth": {"accessToken": ${token}}}`);
  assert.equal(res.status, 400);
  const text = await res.text();
  assert.equal(text.includes(token), false, text);
  assert.equal(text.includes("sk-ant"), false, text);
});
