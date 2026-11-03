// Getting started and staying healthy: `setup`, `doctor`, `serve`, `dev`, `status`, `init`,
// `skill install`, and the new-version notice.

import { type ChildProcess, execFileSync, spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import {
  cpSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  realpathSync,
  writeFileSync,
} from "node:fs";
import { createConnection } from "node:net";
import { homedir } from "node:os";
import { delimiter, dirname, join, relative, resolve } from "node:path";
import { t } from "@charet/i18n";
import pkg from "../package.json" with { type: "json" };
import { rememberToken, withToken } from "./auth.ts";
import { findCheckout, localChanges } from "./checkout.ts";
import { CliError, type Context, checkout, client, importProject } from "./commands.ts";
import { fail, system } from "./data.ts";

/** The charet repository (releases, stars). */
export const REPOSITORY = "EunhakChoi/charet";
const REPO_ROOT = resolve(import.meta.dirname, "../../..");
// The skill sources live at the repository root (`skills/charet`).
const SKILL_DIR = resolve(import.meta.dirname, "../../../skills/charet");

// ---- user settings ----------------------------------------------------------------------------

interface Settings {
  /** The answer to the star question; asked once. */
  star?: "yes" | "no";
  /** When the new-version notice last ran (ISO time). */
  updateNoticeAt?: string;
}

/** Where the CLI keeps settings and credentials (`CHARET_CONFIG_DIR`, else `~/.config/charet`). */
export function configDir(env: NodeJS.ProcessEnv): string {
  return (
    env.CHARET_CONFIG_DIR ??
    join(env.XDG_CONFIG_HOME ?? join(env.HOME ?? homedir(), ".config"), "charet")
  );
}

function settingsFile(ctx: Context): string {
  return join(configDir(ctx.env), "settings.json");
}

export function readSettings(ctx: Context): Settings {
  try {
    return JSON.parse(readFileSync(settingsFile(ctx), "utf8")) as Settings;
  } catch {
    return {};
  }
}

function writeSettings(ctx: Context, settings: Settings): void {
  const file = settingsFile(ctx);
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(file, `${JSON.stringify(settings, null, 2)}\n`);
}

// ---- helpers ----------------------------------------------------------------------------------

/** Runs a program and returns its trimmed output, or `undefined` when it is missing or fails. */
function output(ctx: Context, command: string, args: string[]): string | undefined {
  if (ctx.sh) return ctx.sh(command, args);
  try {
    return execFileSync(command, args, { stdio: ["ignore", "pipe", "ignore"], timeout: 15_000 })
      .toString()
      .trim();
  } catch {
    return undefined;
  }
}

function which(ctx: Context, name: string): string | undefined {
  for (const dir of (ctx.env.PATH ?? "").split(delimiter)) {
    if (dir && existsSync(join(dir, name))) return join(dir, name);
  }
  return undefined;
}

async function yes(ctx: Context, question: string): Promise<boolean> {
  if (!ctx.ask) return false;
  const answer = (await ctx.ask(`${question} [Y/n] `)).trim().toLowerCase();
  return answer === "" || answer === "y" || answer === "yes";
}

/** Compares dotted versions numerically (`0.10.0` > `0.9.1`); pre-release tags sort first. */
export function newer(a: string, b: string): boolean {
  const parts = (v: string) => v.split(/[.-]/).map((x) => (/^\d+$/.test(x) ? Number(x) : -1));
  const [x, y] = [parts(a), parts(b)];
  for (let i = 0; i < Math.max(x.length, y.length); i++) {
    const d = (x[i] ?? 0) - (y[i] ?? 0);
    if (d !== 0) return d > 0;
  }
  return false;
}

// ---- setup ------------------------------------------------------------------------------------

export async function setup(ctx: Context, assumeYes: boolean): Promise<number> {
  ctx.out(t("cli.setup.welcome", { version: pkg.version }, ctx.locale));
  const reachable = await system(ctx).then(
    (info) => info,
    () => undefined,
  );
  if (reachable) {
    ctx.out(t("cli.setup.server", { url: reachable.toolUrl ?? ctx.server }, ctx.locale));
    await signInAsOwner(ctx, assumeYes);
  } else {
    ctx.out(t("cli.setup.noServer", { server: ctx.server }, ctx.locale));
  }

  // Star question: a person at a terminal only, asked once; never in CI, docker or with --yes.
  const settings = readSettings(ctx);
  if (!assumeYes && ctx.ask && !settings.star && !ctx.env.CI) {
    const star = await yes(ctx, t("cli.setup.star", {}, ctx.locale));
    writeSettings(ctx, { ...settings, star: star ? "yes" : "no" });
    if (star) starRepository(ctx);
  }

  const agents = detectAgents(ctx);
  if (agents.length === 0) {
    ctx.out(t("cli.setup.noAgents", {}, ctx.locale));
  } else {
    // Agents that already have the skill (or see one from here) are told so and not asked.
    const steps = skillPlan(ctx, agents, true);
    reportSkill(ctx, steps);
    const needed = steps.filter((step) => step.action === "install");
    if (
      needed.length > 0 &&
      (assumeYes ||
        (await yes(
          ctx,
          t("cli.setup.skill", { agents: needed.map((s) => s.agent).join(", ") }, ctx.locale),
        )))
    ) {
      installSkill(ctx, needed);
    }
  }
  ctx.out(t("cli.setup.next", {}, ctx.locale));
  return 0;
}

/**
 * The CLI comes in as the owner of a charet on this machine: it reads the owner's token from the
 * server (its container, or the `charet-server` next to this CLI), keeps it, and hands over the
 * admin link for the browser. Nobody is the admin by address alone.
 */
async function signInAsOwner(ctx: Context, assumeYes: boolean): Promise<void> {
  const me = await client(ctx).GET("/api/auth/me");
  if (me.data?.kind === "user") {
    ctx.out(t("cli.setup.signedIn", { name: me.data.name, role: me.data.role }, ctx.locale));
    return;
  }
  const token = ownerToken(ctx);
  const checked = token ? await withToken(ctx, token).GET("/api/auth/me") : undefined;
  if (!token || checked?.data?.kind !== "user" || checked.data.role !== "admin") {
    ctx.out(t("cli.setup.noOwnerToken", { server: browserUrl(ctx.server) }, ctx.locale));
    return;
  }
  rememberToken(ctx.env, ctx.server, token, checked.data.name);
  const link = `${browserUrl(ctx.server)}/?t=${token}`;
  ctx.out(t("cli.setup.adminLink", { link }, ctx.locale));
  if (!assumeYes && ctx.ask && !ctx.env.CI) {
    const opener =
      process.platform === "darwin"
        ? "open"
        : process.platform === "win32"
          ? "explorer"
          : "xdg-open";
    output(ctx, opener, [link]);
  }
}

/** The owner's token from the charet serving `ctx.server` on this machine, if it can be read. */
function ownerToken(ctx: Context): string | undefined {
  let url: URL;
  try {
    url = new URL(ctx.server);
  } catch {
    return undefined;
  }
  if (!["localhost", "127.0.0.1", "[::1]"].includes(url.hostname)) return undefined;
  const port = url.port || (url.protocol === "https:" ? "443" : "80");
  // Docker: the compose service `server` publishing this port.
  const listed = output(ctx, "docker", [
    "ps",
    "--filter",
    "label=com.docker.compose.service=server",
    "--format",
    "{{.ID}} {{.Ports}}",
  ]);
  const container = listed
    ?.split("\n")
    .find((line) => publishes(line, Number(port), 5280))
    ?.split(" ")[0];
  if (container) {
    const token = output(ctx, "docker", ["exec", container, "charet-server", "admin-token"]);
    if (token) return token.split("\n").at(-1)?.trim();
  }
  // `charet serve` or a server run by hand: the same binary with the same data reads it.
  const server = findProgram(ctx, "CHARET_SERVER_BIN", "charet-server", [
    "target/release/charet-server",
    "target/debug/charet-server",
  ]);
  if (!server) return undefined;
  return output(ctx, server, ["admin-token"])?.split("\n").at(-1)?.trim();
}

/**
 * The address a browser opens: `localhost` rather than a loopback IP, since previews are served
 * from `127.0.0.1` and must not get the tool's cookies.
 */
export function browserUrl(server: string): string {
  const url = new URL(server);
  if (url.hostname === "127.0.0.1" || url.hostname === "[::1]") url.hostname = "localhost";
  return url.origin + url.pathname.replace(/\/+$/, "");
}

/**
 * Whether a `docker ps` ports column maps host `port` to the container's `target`. Docker joins
 * neighbouring ports into ranges: `127.0.0.1:5280-5281->5280-5281/tcp`.
 */
export function publishes(ports: string, port: number, target: number): boolean {
  for (const [, from, to, inner] of ports.matchAll(/:(\d+)(?:-(\d+))?->(\d+)(?:-\d+)?\//g)) {
    const offset = target - Number(inner);
    if (offset >= 0 && Number(from) + offset === port && port <= Number(to ?? from)) return true;
  }
  return false;
}

function starRepository(ctx: Context): void {
  const viaGh =
    output(ctx, "gh", ["auth", "status"]) !== undefined &&
    output(ctx, "gh", ["api", "-X", "PUT", `/user/starred/${REPOSITORY}`, "--silent"]) !==
      undefined;
  if (viaGh) {
    ctx.out(t("cli.setup.starred", {}, ctx.locale));
    return;
  }
  const url = `https://github.com/${REPOSITORY}`;
  const opener =
    process.platform === "darwin" ? "open" : process.platform === "win32" ? "explorer" : "xdg-open";
  if (output(ctx, opener, [url]) === undefined)
    ctx.out(t("cli.setup.starManually", { url }, ctx.locale));
  else ctx.out(t("cli.setup.starOpened", { url }, ctx.locale));
}

// ---- skill ------------------------------------------------------------------------------------

type Agent = "claude" | "codex";

function home(ctx: Context): string {
  return ctx.env.HOME ?? homedir();
}

function skillTarget(ctx: Context, agent: Agent): string {
  return agent === "claude"
    ? join(ctx.env.CLAUDE_CONFIG_DIR ?? join(home(ctx), ".claude"), "skills", "charet")
    : join(ctx.env.CODEX_HOME ?? join(home(ctx), ".codex"), "skills", "charet");
}

function detectAgents(ctx: Context): Agent[] {
  const found: Agent[] = [];
  if (existsSync(dirname(dirname(skillTarget(ctx, "claude"))))) found.push("claude");
  if (existsSync(dirname(dirname(skillTarget(ctx, "codex"))))) found.push("codex");
  return found;
}

type SkillAction = "install" | "linked" | "upToDate" | "present";
interface SkillStep {
  agent: Agent;
  target: string;
  action: SkillAction;
  /** Where the skill is, for the notes (the target, or a project folder that has one). */
  dir: string;
}

/**
 * What each agent needs: a copy, or nothing because the skill is already there: a link to this
 * repository, an identical copy, or (with `reuse`) a project-level skill the agent finds from the
 * working directory. A copy that differs is updated.
 */
function skillPlan(ctx: Context, agents: Agent[], reuse: boolean): SkillStep[] {
  return agents.map((agent) => {
    const target = skillTarget(ctx, agent);
    const step = (action: SkillAction, dir = target): SkillStep => ({ agent, target, action, dir });
    if (existsSync(target)) {
      if (realpathSync(target) === realpathSync(SKILL_DIR)) return step("linked");
      return sameSkill(SKILL_DIR, target) ? step("upToDate") : step("install");
    }
    const project = reuse ? projectSkill(ctx, agent) : undefined;
    return project ? step("present", project) : step("install");
  });
}

/** A charet skill in a project folder above the working directory: `.claude/skills` is read by
 * Claude, `.agents/skills` by Codex. */
function projectSkill(ctx: Context, agent: Agent): string | undefined {
  const folder = agent === "claude" ? ".claude" : ".agents";
  let dir = resolve(ctx.cwd);
  for (;;) {
    const candidate = join(dir, folder, "skills", "charet");
    if (existsSync(join(candidate, "SKILL.md"))) return candidate;
    const parent = dirname(dir);
    if (parent === dir) return undefined;
    dir = parent;
  }
}

/** Every file of a skill folder by relative path; evals/ are the skill's own test prompts
 * (skill-creator), not something agents need. */
function skillFiles(root: string, dir = root, found = new Map<string, Buffer>()) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (path === join(root, "evals")) continue;
    if (entry.isDirectory()) skillFiles(root, path, found);
    else found.set(relative(root, path), readFileSync(path));
  }
  return found;
}

function sameSkill(a: string, b: string): boolean {
  const left = skillFiles(a);
  const right = skillFiles(b);
  if (left.size !== right.size) return false;
  for (const [file, bytes] of left) if (!right.get(file)?.equals(bytes)) return false;
  return true;
}

/** Says what is already in place; the steps to install are left to the caller. */
function reportSkill(ctx: Context, steps: SkillStep[]): void {
  for (const { agent, dir, action } of steps) {
    if (action !== "install") ctx.out(t(`cli.skill.${action}`, { agent, dir }, ctx.locale));
  }
}

function installSkill(ctx: Context, steps: SkillStep[]): void {
  for (const { agent, target } of steps) {
    mkdirSync(dirname(target), { recursive: true });
    cpSync(SKILL_DIR, target, {
      recursive: true,
      force: true,
      filter: (source) => !source.startsWith(join(SKILL_DIR, "evals")),
    });
    ctx.out(t("cli.skill.installed", { agent, dir: target }, ctx.locale));
  }
}

export function skill(
  ctx: Context,
  sub: string | undefined,
  claude: boolean,
  codex: boolean,
): number {
  if (sub !== "install") throw new CliError(t("cli.usage.skill", {}, ctx.locale));
  const chosen: Agent[] = [
    ...(claude ? ["claude" as const] : []),
    ...(codex ? ["codex" as const] : []),
  ];
  const agents = chosen.length ? chosen : detectAgents(ctx);
  // Asked for outright: only what is already in place is skipped, a project-level one is not.
  const steps = skillPlan(ctx, agents.length ? agents : ["claude"], false);
  reportSkill(ctx, steps);
  installSkill(
    ctx,
    steps.filter((step) => step.action === "install"),
  );
  return 0;
}

// ---- doctor -----------------------------------------------------------------------------------

type Check = { state: "ok" | "fail" | "warn" | "info"; text: string; hint?: string };

function portOpen(port: number): Promise<boolean> {
  return new Promise((done) => {
    const socket = createConnection({ host: "127.0.0.1", port, timeout: 500 });
    socket.once("connect", () => {
      socket.destroy();
      done(true);
    });
    socket.once("error", () => done(false));
    socket.once("timeout", () => {
      socket.destroy();
      done(false);
    });
  });
}

const isLocalUrl = (url: string) =>
  ["localhost", "127.0.0.1", "[::1]"].includes(new URL(url).hostname);

/** A local charet that is shared: does its address reach the remote door? */
async function sharingCheck(ctx: Context, url: string): Promise<Check> {
  const reached = await (ctx.fetch ?? fetch)(`${url}/api/health`, {
    signal: AbortSignal.timeout(5000),
  })
    .then((r) => r.ok)
    .catch(() => false);
  return reached
    ? { state: "ok", text: t("cli.doctor.shared", { url }, ctx.locale) }
    : {
        state: "fail",
        text: t("cli.doctor.sharedUnreachable", { url }, ctx.locale),
        hint: t("cli.doctor.sharedHint", {}, ctx.locale),
      };
}

/** A public server: HTTPS on a fixed address (tokens travel with every request). */
function publicChecks(ctx: Context, info: Awaited<ReturnType<typeof system>>): Check[] {
  const checks: Check[] = [];
  const local = isLocalUrl;
  if (info.toolUrl) {
    checks.push(
      info.toolUrl.startsWith("https://") || local(info.toolUrl)
        ? { state: "ok", text: `public address ${info.toolUrl}` }
        : {
            state: "fail",
            text: `public address ${info.toolUrl} is not HTTPS`,
            hint: t("cli.doctor.https", {}, ctx.locale),
          },
    );
  }
  return checks;
}

export async function doctor(ctx: Context): Promise<number> {
  const checks: Check[] = [];
  const node = process.versions.node;
  const [major = 0] = node.split(".").map(Number);
  checks.push(
    major >= 24
      ? { state: "ok", text: `node ${node}` }
      : { state: "fail", text: `node ${node}`, hint: t("cli.doctor.node", {}, ctx.locale) },
  );
  const git = output(ctx, "git", ["--version"]);
  checks.push(
    git
      ? { state: "ok", text: git }
      : { state: "fail", text: "git: not found", hint: t("cli.doctor.git", {}, ctx.locale) },
  );

  const info = await system(ctx).catch(() => undefined);
  if (!info) {
    checks.push({
      state: "fail",
      text: `server ${ctx.server}: unreachable`,
      hint: t("cli.doctor.serve", {}, ctx.locale),
    });
    for (const [port, name] of [
      [5280, "charet-server"],
      [5282, "charet-workspace"],
      [5283, "charet-render"],
    ] as const) {
      if (await portOpen(port))
        checks.push({ state: "info", text: `port ${port} (${name}) is in use` });
    }
  } else {
    checks.push({
      state: "ok",
      text: `server ${ctx.server} (charet ${info.version}${info.localMode ? ", local mode" : ""})`,
    });
    for (const [name, service] of [
      ["charet-workspace", info.services.workspace],
      ["charet-render", info.services.render],
    ] as const) {
      checks.push(
        service.ok
          ? { state: "ok", text: name }
          : {
              state: "fail",
              text: `${name}${service.error ? `: ${service.error}` : ""}`,
              hint: t("cli.doctor.serve", {}, ctx.locale),
            },
      );
    }
    if (info.toolUrl && info.previewUrl) {
      // Previews run project code: on the tool's own address the server sandboxes them.
      const shared = new URL(info.toolUrl).origin === new URL(info.previewUrl).origin;
      checks.push({
        state: "ok",
        text: shared
          ? t("cli.doctor.previewsShared", { url: info.toolUrl }, ctx.locale)
          : `web tool ${info.toolUrl}, previews ${info.previewUrl}`,
      });
    }
    if (!info.localMode) checks.push(...publicChecks(ctx, info));
    else if (info.toolUrl && !isLocalUrl(info.toolUrl))
      checks.push(await sharingCheck(ctx, info.toolUrl));
    if (info.latest && newer(info.latest, info.version)) {
      checks.push({
        state: "warn",
        text: t("cli.update.available", { latest: info.latest, version: info.version }, ctx.locale),
      });
    }
  }
  const docker = output(ctx, "docker", ["version", "--format", "{{.Server.Version}}"]);
  checks.push(
    docker
      ? { state: "info", text: `docker ${docker}` }
      : { state: "info", text: t("cli.doctor.noDocker", {}, ctx.locale) },
  );

  const failed = checks.some((c) => c.state === "fail");
  if (ctx.json) {
    ctx.out(JSON.stringify({ ok: !failed, checks }, null, 2));
  } else {
    const mark = { ok: "✓", fail: "✗", warn: "!", info: "-" } as const;
    for (const c of checks) {
      ctx.out(`${mark[c.state]} ${c.text}`);
      if (c.hint) ctx.out(`    ${c.hint}`);
    }
  }
  return failed ? 1 : 0;
}

// ---- serve ------------------------------------------------------------------------------------

function findProgram(
  ctx: Context,
  env: string,
  name: string,
  repoPaths: string[],
): string | undefined {
  const fromEnv = ctx.env[env];
  if (fromEnv) return fromEnv;
  return which(ctx, name) ?? repoPaths.map((p) => join(REPO_ROOT, p)).find((p) => existsSync(p));
}

/** Runs charet-server, and unless `serverOnly`, charet-workspace and charet-render next to it. */
export async function serve(
  ctx: Context,
  options: { public: boolean; serverOnly: boolean },
): Promise<number> {
  const server = findProgram(ctx, "CHARET_SERVER_BIN", "charet-server", [
    "target/release/charet-server",
    "target/debug/charet-server",
  ]);
  if (!server) throw new CliError(t("cli.serve.missing", { name: "charet-server" }, ctx.locale));
  const children: ChildProcess[] = [];
  const env: NodeJS.ProcessEnv = { ...ctx.env };
  if (!options.serverOnly) {
    const render = findProgram(ctx, "CHARET_RENDER_BIN", "charet-render", [
      "target/release/charet-render",
      "target/debug/charet-render",
    ]);
    const workspace = findProgram(ctx, "CHARET_WORKSPACE_MAIN", "charet-workspace", [
      "apps/charet-workspace/src/main.ts",
    ]);
    if (!render) throw new CliError(t("cli.serve.missing", { name: "charet-render" }, ctx.locale));
    if (!workspace)
      throw new CliError(t("cli.serve.missing", { name: "charet-workspace" }, ctx.locale));
    env.CHARET_WORKSPACE_TOKEN ??= randomBytes(24).toString("hex");
    env.CHARET_RENDER_TOKEN ??= randomBytes(24).toString("hex");
    children.push(spawn(render, [], { env, stdio: "inherit" }));
    children.push(
      workspace.endsWith(".ts")
        ? spawn(process.execPath, [workspace], { env, stdio: "inherit" })
        : spawn(workspace, [], { env, stdio: "inherit" }),
    );
  }
  children.push(spawn(server, options.public ? ["--public"] : [], { env, stdio: "inherit" }));
  return new Promise((done) => {
    let finished = false;
    const stop = (code: number) => {
      if (finished) return;
      finished = true;
      for (const child of children) child.kill("SIGTERM");
      done(code);
    };
    for (const child of children) child.once("exit", (code) => stop(code ?? 0));
    process.once("SIGINT", () => stop(0));
    process.once("SIGTERM", () => stop(0));
  });
}

// ---- checkout helpers: dev, status ------------------------------------------------------------

function appDir(dir: string): string {
  try {
    const app = (JSON.parse(readFileSync(join(dir, "charet.json"), "utf8")) as { app?: string })
      .app;
    return app && app !== "." ? join(dir, app) : dir;
  } catch {
    return dir;
  }
}

function packageManager(dir: string): "pnpm" | "npm" | "yarn" {
  if (existsSync(join(dir, "pnpm-lock.yaml"))) return "pnpm";
  if (existsSync(join(dir, "yarn.lock"))) return "yarn";
  return "npm";
}

/**
 * Runs the app's vite dev server in this checkout (installing first when needed): with the
 * charet-only config `.charet/vite.config.*` when there is one, else the app's `dev` script.
 */
export async function dev(ctx: Context): Promise<number> {
  const found = findCheckout(ctx.cwd);
  if (!found) throw new CliError(t("cli.error.noCheckout", {}, ctx.locale));
  const run = (command: string, args: string[], cwd: string) =>
    new Promise<number>((done) => {
      const child = spawn(command, args, { cwd, env: ctx.env, stdio: "inherit" });
      child.once("exit", (code) => done(code ?? 1));
      child.once("error", () => done(127));
    });
  const app = appDir(found.dir);
  const charet = join(app, ".charet");
  // The app's dependencies, then charet-only ones (`.charet/package.json`) installed apart.
  for (const dir of [found.dir, ...(existsSync(join(charet, "package.json")) ? [charet] : [])]) {
    if (existsSync(join(dir, "node_modules"))) continue;
    const installed = await run(packageManager(dir), ["install"], dir);
    if (installed !== 0) return installed;
  }
  const config = ["ts", "mts", "js", "mjs"]
    .map((ext) => join(charet, `vite.config.${ext}`))
    .find((file) => existsSync(file));
  const vite = config ? findViteBin(app, found.dir) : undefined;
  if (config && vite) return run(vite, ["--config", config], app);
  return run(packageManager(found.dir), ["run", "dev"], app);
}

/** `node_modules/.bin/vite` from the app up to the checkout root (monorepos hoist tools). */
function findViteBin(app: string, root: string): string | undefined {
  for (let dir = app; ; dir = dirname(dir)) {
    const bin = join(dir, "node_modules", ".bin", "vite");
    if (existsSync(bin)) return bin;
    if (dir === root || dirname(dir) === dir) return undefined;
  }
}

export async function status(ctx: Context): Promise<number> {
  const found = findCheckout(ctx.cwd);
  if (!found) throw new CliError(t("cli.error.noCheckout", {}, ctx.locale));
  const { dir, state } = found;
  const changes = localChanges(dir);
  const { data, error } = await client(ctx, state.server).GET("/api/projects/{key}/versions", {
    params: { path: { key: state.project } },
  });
  if (!data) fail(error, "request failed");
  const latest = data.versions[0];
  const base = data.versions.find((v) => v.commit === state.base);
  const behind = latest !== undefined && latest.commit !== state.base;
  if (ctx.json) {
    ctx.out(
      JSON.stringify(
        {
          dir,
          ...state,
          baseVersion: base?.number ?? null,
          latestVersion: latest?.number ?? null,
          behind,
          changes: changes.map((c) => ({ path: c.path, kind: c.kind })),
        },
        null,
        2,
      ),
    );
    return 0;
  }
  ctx.out(`project  ${state.project} (${state.server})`);
  if (state.branch) ctx.out(`branch   #${state.branch}`);
  ctx.out(`base     ${base ? `v${base.number} ` : ""}${state.base.slice(0, 10)}`);
  ctx.out(
    `main     ${latest ? `v${latest.number} ${latest.commit.slice(0, 10)}` : "?"}${behind ? `  ${t("cli.status.behind", {}, ctx.locale)}` : ""}`,
  );
  ctx.out(`changes  ${changes.length}`);
  for (const c of changes) ctx.out(`  ${c.kind} ${c.path}`);
  return 0;
}

// ---- init -------------------------------------------------------------------------------------

/** Creates a project from a template and checks it out into `dir`. */
export async function init(
  ctx: Context,
  key: string | undefined,
  dir: string | undefined,
  template: string,
  title: string | undefined,
): Promise<number> {
  if (!key) throw new CliError(t("cli.usage.init", {}, ctx.locale));
  const source = join(REPO_ROOT, "templates", template);
  if (!existsSync(join(source, "package.json"))) {
    throw new CliError(t("cli.init.noTemplate", { template }, ctx.locale));
  }
  const quiet = { ...ctx, out: () => {} };
  await importProject(quiet, {
    path: source,
    key,
    ...(title ? { title } : {}),
    include: [],
    dryRun: false,
  });
  await checkout(ctx, key, dir, undefined);
  return 0;
}

// ---- new-version notice -----------------------------------------------------------------------

/**
 * Once a day, tells a person at a terminal when the server knows of a newer release. The server
 * does the checking (`CHARET_UPDATE_CHECK`); the CLI only asks it, with a short timeout.
 */
export async function updateNotice(ctx: Context): Promise<void> {
  if (ctx.env.CHARET_UPDATE_CHECK === "false" || ctx.env.CI) return;
  const settings = readSettings(ctx);
  const last = settings.updateNoticeAt ? Date.parse(settings.updateNoticeAt) : 0;
  if (Date.now() - last < 24 * 60 * 60 * 1000) return;
  try {
    const response = await (ctx.fetch ?? fetch)(`${ctx.server}/api/system`, {
      signal: AbortSignal.timeout(800),
    });
    if (!response.ok) return;
    const info = (await response.json()) as { latest?: string | null };
    writeSettings(ctx, { ...settings, updateNoticeAt: new Date().toISOString() });
    if (info.latest && newer(info.latest, pkg.version)) {
      ctx.err(t("cli.update.available", { latest: info.latest, version: pkg.version }, ctx.locale));
    }
  } catch {
    // Offline or no server: say nothing.
  }
}
