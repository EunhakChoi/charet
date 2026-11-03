#!/usr/bin/env node
// Inside a run: checks out the branch, lets the agent work, saves what is left, reports on stdout.
//
// Env: CHARET_RUN_SPEC (the spec file; images next to it), CHARET_RUN_WORK (where to check out),
// CHARET_URL + CHARET_TOKEN (the agent door), CHARET_CLI (optional, the CLI to run).

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import type { RunSpec } from "../protocol.ts";
import { secretsOf } from "../secrets.ts";
import { claude } from "./claude.ts";
import { codex } from "./codex.ts";
import { type Agent, cli, emit, hide, keepSecret, tool } from "./common.ts";
import { script } from "./script.ts";

const AGENTS: Record<RunSpec["agent"], Agent> = { claude, codex, script };

const specPath = process.env.CHARET_RUN_SPEC ?? "/input/spec.json";
const work = process.env.CHARET_RUN_WORK ?? "/work/project";
const spec = JSON.parse(readFileSync(specPath, "utf8")) as RunSpec;
keepSecret(secretsOf(spec));
const abort = new AbortController();
let canceled = false;
process.once("SIGTERM", () => {
  canceled = true;
  abort.abort();
});

/** Pushes what the agent left unsaved. */
async function save(): Promise<void> {
  const pushed = await cli(
    ["push", "--json", "-m", `Agent: ${spec.prompt.split("\n")[0]?.slice(0, 60)}`],
    work,
  );
  if (pushed.code !== 0) {
    tool(
      "charet",
      `could not save the last changes: ${(pushed.stderr || pushed.stdout).trim().slice(0, 500)}`,
    );
    return;
  }
  try {
    const branch = JSON.parse(pushed.stdout) as { number?: number };
    if (branch.number) tool("charet", `saved the last changes to branch #${branch.number}`);
  } catch {
    // Nothing to save.
  }
}

let checkedOut = false;
try {
  tool("charet", `checkout ${spec.project} --branch ${spec.branch}`);
  const out = await cli(["checkout", spec.project, work, "--branch", String(spec.branch)]);
  if (out.code !== 0) throw new Error(`checkout failed: ${(out.stderr || out.stdout).trim()}`);
  checkedOut = true;
  const images = spec.images.map((image) => ({
    mediaType: image.mediaType,
    path: join(dirname(specPath), image.file),
  }));
  await AGENTS[spec.agent].run({ spec, work, images, signal: abort.signal });
  await save();
  emit({ type: "result", state: canceled ? "canceled" : "done" });
} catch (error) {
  if (checkedOut) await save();
  if (canceled) emit({ type: "result", state: "canceled" });
  else {
    emit({ type: "result", state: "failed", error: hide((error as Error).message).slice(0, 2000) });
  }
}
process.exit(0);
