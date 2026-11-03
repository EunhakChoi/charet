import assert from "node:assert/strict";
import { test } from "node:test";
import type { Launched, Launcher } from "./launch.ts";
import type { RunStart } from "./protocol.ts";
import { price, Runs } from "./runs.ts";

const start: RunStart = {
  id: 1,
  agent: "codex",
  model: "m",
  apiKey: "k",
  project: "web",
  branch: 2,
  token: "ab".repeat(16),
  prompt: "go",
  context: null,
  images: [],
  history: [],
};

/** A launcher the test drives: `emit` sends lines, `exit` ends the run. */
function fake(stderr = "boom") {
  let onLine: (line: string) => void = () => {};
  let exit: (code: number) => void = () => {};
  let stopped = 0;
  const launcher: Launcher = {
    async launch(_run, listener): Promise<Launched> {
      onLine = listener;
      return {
        exited: new Promise((done) => {
          exit = done;
        }),
        stop: () => {
          stopped++;
        },
        stderr: () => stderr,
      };
    },
    async cleanup() {},
  };
  return {
    launcher,
    emit: (line: unknown) => onLine(JSON.stringify(line)),
    exit: (code: number) => exit(code),
    stopped: () => stopped,
  };
}

const tick = () => new Promise((r) => setImmediate(r));

test("collects events, prices usage and keeps the end", async () => {
  const f = fake();
  const runs = new Runs(f.launcher, { prices: { m: { input: 2, output: 8 } }, timeoutMs: 60_000 });
  await runs.start(start);
  f.emit({ type: "event", kind: "assistant", text: "hi" });
  f.emit({ type: "usage", tokensIn: 1_000_000, tokensOut: 500_000, costUsd: null, model: "m" });
  f.emit({ type: "result", state: "done" });
  f.exit(0);
  await tick();
  const progress = runs.progress(1, 0);
  assert.deepEqual(progress?.events, [{ kind: "assistant", text: "hi" }]);
  assert.equal(progress?.costUsd, 6);
  assert.equal(progress?.finished, "done");
  assert.deepEqual(runs.progress(1, 1)?.events, []);
  assert.equal(runs.progress(9, 0), undefined);
});

test("a run that dies without a result failed; a canceled one was canceled", async () => {
  const f = fake();
  const runs = new Runs(f.launcher, { prices: {}, timeoutMs: 60_000 });
  await runs.start(start);
  f.exit(3);
  await tick();
  assert.equal(runs.progress(1, 0)?.finished, "failed");
  assert.match(runs.progress(1, 0)?.error ?? "", /exit 3.*boom/);

  const g = fake();
  const more = new Runs(g.launcher, { prices: {}, timeoutMs: 60_000 });
  await more.start(start);
  assert.equal(more.cancel(1), true);
  assert.equal(g.stopped(), 1);
  g.emit({ type: "result", state: "done" });
  g.exit(0);
  await tick();
  assert.equal(more.progress(1, 0)?.finished, "canceled");
  assert.equal(more.cancel(5), false);
});

test("the end of a run's stderr, in its error, hides the run's login", async () => {
  const token = "sk-ant-oat-0123456789abcdef";
  const f = fake(`401 Unauthorized: bearer ${token}`);
  const runs = new Runs(f.launcher, { prices: {}, timeoutMs: 60_000 });
  await runs.start({
    ...start,
    agent: "claude",
    apiKey: "",
    oauth: { provider: "anthropic", accessToken: token },
  });
  f.exit(1);
  await tick();
  const error = runs.progress(1, 0)?.error ?? "";
  assert.match(error, /exit 1\): 401 Unauthorized/);
  assert.equal(error.includes(token), false, error);
});

test("prices by model, with cached input", () => {
  const prices = { m: { input: 1, output: 4, cachedInput: 0.1 } };
  assert.equal(
    price(prices, { tokensIn: 1_000_000, cachedIn: 500_000, tokensOut: 0, model: "m" }),
    0.55,
  );
  assert.equal(price(prices, { tokensIn: 5, tokensOut: 5 }), null);
});
