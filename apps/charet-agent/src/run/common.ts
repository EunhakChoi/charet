// What every agent in a run shares: reporting, the charet CLI, and the instructions.

import { spawn } from "node:child_process";
import type { RunnerLine, RunSpec } from "../protocol.ts";
import { scrub } from "../secrets.ts";

let secrets: readonly string[] = [];

/** From now on, what this run reports (`emit`, `hide`) never holds these credential values. */
export function keepSecret(values: readonly string[]): void {
  secrets = values;
}

/** `text` without the run's credentials. Cut text after this, not before: a cut credential no
 * longer matches. */
export function hide(text: string): string {
  return scrub(text, secrets);
}

/** Reports to charet-agent: one JSON line on stdout. */
export function emit(line: RunnerLine): void {
  process.stdout.write(`${hide(JSON.stringify(line))}\n`);
}

export function tool(name: string, text: string): void {
  emit({ type: "event", kind: "tool", tool: name, text: hide(text).slice(0, 2000) });
}

export function say(text: string): void {
  if (text.trim()) emit({ type: "event", kind: "assistant", text: text.trim() });
}

export interface Agent {
  run(input: AgentInput): Promise<void>;
}

export interface AgentInput {
  spec: RunSpec;
  work: string;
  /** Image files, absolute paths with their types. */
  images: { mediaType: string; path: string }[];
  signal: AbortSignal;
}

/** Runs the charet CLI (`CHARET_CLI`: a `.ts` entry for node, else a command; default `charet`). */
export function cli(
  args: string[],
  cwd?: string,
): Promise<{ code: number; stdout: string; stderr: string }> {
  const entry = process.env.CHARET_CLI ?? "charet";
  const [command, argv] = entry.endsWith(".ts")
    ? [process.execPath, [entry, ...args]]
    : [entry, args];
  return new Promise((resolve) => {
    const child = spawn(command, argv, {
      cwd,
      env: process.env,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (c: Buffer) => {
      stdout += c.toString();
    });
    child.stderr.on("data", (c: Buffer) => {
      stderr += c.toString();
    });
    child.on("error", (error) => resolve({ code: 127, stdout, stderr: String(error) }));
    child.on("close", (code) => resolve({ code: code ?? 1, stdout, stderr }));
  });
}

/** How the agent should work here; goes into its system prompt. */
export function instructions(spec: RunSpec): string {
  return [
    `You are the charet web agent. A teammate asked for a change from the charet web tool; they`,
    `may not be a developer. You work in a charet checkout of project "${spec.project}", on its`,
    `branch #${spec.branch} (their draft), in the current directory.`,
    "",
    "- Change the app's UI code (and `.charet/` screens, mocks, flows when needed) to do what they asked.",
    `- Save with \`charet push -m "<what changed>"\` after each meaningful step: it adds to branch #${spec.branch},`,
    "  and `charet check <screen…>` renders what was pushed. Anything unsaved is pushed for you at the end.",
    `- Look at the result: \`charet check <id>\` for errors, \`charet capture <id> --rev b${spec.branch} --out /tmp/shots\``,
    "  for pictures (read the PNGs). The charet skill describes the CLI.",
    "- Do not merge, request review, or touch other branches; the person decides that in the web tool.",
    "- Only the model API and the charet server are reachable; package installs from the internet fail.",
    "- When done, answer briefly in the language they wrote in: what you changed and anything they should check.",
  ].join("\n");
}

/** The request as the agent reads it: earlier turns, what they pointed at, then the ask. */
export function request(spec: RunSpec): string {
  const parts: string[] = [];
  if (spec.history.length > 0) {
    parts.push(
      "Earlier on this branch (oldest first):",
      ...spec.history.map((t) => `${t.role === "user" ? "They asked" : "You answered"}: ${t.text}`),
      "",
    );
  }
  const context = describeContext(spec.context);
  if (context) parts.push("What they pointed at in the web tool:", context, "");
  parts.push(spec.prompt);
  return parts.join("\n");
}

function describeContext(context: unknown): string {
  if (context === null || context === undefined) return "";
  if (typeof context === "object") {
    const { newBranch: _, ...rest } = context as Record<string, unknown>;
    if (Object.keys(rest).length === 0) return "";
    return JSON.stringify(rest, null, 2);
  }
  return String(context);
}

/** A tool call in one line. */
export function summary(input: unknown): string {
  if (typeof input !== "object" || input === null) return "";
  const v = input as Record<string, unknown>;
  for (const key of ["command", "file_path", "path", "pattern", "url", "description"]) {
    if (typeof v[key] === "string") return v[key] as string;
  }
  return JSON.stringify(input).slice(0, 300);
}
