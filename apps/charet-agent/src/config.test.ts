import assert from "node:assert/strict";
import { test } from "node:test";
import { readConfig } from "./config.ts";

test("defaults to docker runs on loopback", () => {
  const config = readConfig({});
  assert.deepEqual(config.bind, { host: "127.0.0.1", port: 5290 });
  assert.equal(config.sandbox, "docker");
  assert.equal(config.doorUrl, "http://127.0.0.1:5285");
  assert.deepEqual(config.allowHosts, ["api.anthropic.com", "api.openai.com", "chatgpt.com"]);
  assert.equal(config.docker.image, "charet-agent-run:local");
  assert.equal(config.timeoutMs, 30 * 60_000);
  assert.deepEqual(config.prices, {});
});

test("the run time limit is a positive number of minutes a timer can hold", () => {
  assert.equal(readConfig({ CHARET_AGENT_TIMEOUT_MIN: "90" }).timeoutMs, 90 * 60_000);
  assert.equal(readConfig({ CHARET_AGENT_TIMEOUT_MIN: "35791" }).timeoutMs, 35791 * 60_000);
  for (const bad of ["0", "-1", "soon", "35792", "1e30"]) {
    assert.throws(() => readConfig({ CHARET_AGENT_TIMEOUT_MIN: bad }), /CHARET_AGENT_TIMEOUT_MIN/);
  }
});

test("CHARET_AGENT_ALLOW_HOSTS replaces the default hosts", () => {
  const config = readConfig({ CHARET_AGENT_ALLOW_HOSTS: "Gateway.example, api.openai.com" });
  assert.deepEqual(config.allowHosts, ["gateway.example", "api.openai.com"]);
});

test("needs a token off loopback and checks its input", () => {
  assert.throws(() => readConfig({ CHARET_AGENT_BIND: "0.0.0.0:5290" }), /CHARET_AGENT_TOKEN/);
  assert.equal(
    readConfig({ CHARET_AGENT_BIND: "0.0.0.0:5290", CHARET_AGENT_TOKEN: "t" }).token,
    "t",
  );
  assert.throws(() => readConfig({ CHARET_AGENT_SANDBOX: "vm" }), /docker or none/);
  assert.throws(() => readConfig({ CHARET_AGENT_PRICES: "{" }), /not JSON/);
  assert.throws(
    () => readConfig({ CHARET_AGENT_PRICES: '{"m":{"input":1}}' }),
    /CHARET_AGENT_PRICES/,
  );
  const prices = readConfig({ CHARET_AGENT_PRICES: '{"m":{"input":1,"output":4}}' }).prices;
  assert.deepEqual(prices, { m: { input: 1, output: 4 } });
});
