import { existsSync, readdirSync, readFileSync, rmSync, statSync } from "node:fs";
import { basename, join, resolve } from "node:path";
import { type ApiError, type Client, createClient, type Schemas } from "@charet/api-client";
import { type Key, type Locale, t } from "@charet/i18n";
import {
  type CheckoutState,
  commitBase,
  findCheckout,
  git,
  initBase,
  localChanges,
  unpack,
  writeState,
} from "./checkout.ts";
import { repoCommit } from "./handoff.ts";
import { describeSource, listFiles, resolveScope } from "./scope.ts";

export interface Context {
  server: string;
  token?: string;
  cwd: string;
  locale: Locale;
  json: boolean;
  out: (line: string) => void;
  /** Progress and notes (stderr), kept apart from output scripts parse. */
  err: (line: string) => void;
  env: NodeJS.ProcessEnv;
  fetch?: typeof globalThis.fetch;
  /** Asks a person at the terminal; absent when nobody is there (CI, pipes, `--yes`). */
  ask?: (question: string) => Promise<string>;
  /** Runs a program for its output (tests replace it); `undefined` = missing or failed. */
  sh?: (command: string, args: string[]) => string | undefined;
  /** How often to poll a building preview. */
  pollMs?: number;
}

/** A failure to report to the user, already translated. */
export class CliError extends Error {}

export function client(ctx: Context, server = ctx.server): Client {
  return createClient({
    baseUrl: server,
    ...(ctx.token ? { token: ctx.token } : {}),
    ...(ctx.fetch ? { fetch: ctx.fetch } : {}),
  });
}

export function say(ctx: Context, key: Key, params: Record<string, string> = {}): void {
  ctx.out(t(key, params, ctx.locale));
}

export function fail(error: ApiError | undefined, fallback: string): never {
  throw new CliError(error ? `${error.message} (${error.code})` : fallback);
}

export function emit(ctx: Context, value: unknown, human: () => void): void {
  if (ctx.json) ctx.out(JSON.stringify(value, null, 2));
  else human();
}

/** The project named by `-p`, else the checkout the command runs in. */
export function projectOf(ctx: Context, flag: string | undefined): string {
  const project = flag ?? findCheckout(ctx.cwd)?.state.project;
  if (!project) throw new CliError(t("cli.error.noProject", {}, ctx.locale));
  return project;
}

export async function projects(ctx: Context): Promise<void> {
  const { data, error } = await client(ctx).GET("/api/projects");
  if (!data) fail(error, "request failed");
  emit(ctx, data, () => {
    if (data.projects.length === 0) say(ctx, "cli.projects.none");
    for (const p of data.projects) ctx.out(`${p.key}\t${p.title}\t${p.origin}`);
  });
}

/** Deletes a project for good, after the user types its key (or with `--yes`). */
export async function deleteProject(ctx: Context, key: string, yes: boolean): Promise<void> {
  if (!yes) {
    if (!ctx.ask) throw new CliError(t("cli.delete.needsYes", { key }, ctx.locale));
    const answer = await ctx.ask(t("cli.delete.confirm", { key }, ctx.locale));
    if (answer.trim() !== key) throw new CliError(t("cli.delete.cancelled", {}, ctx.locale));
  }
  const { error, response } = await client(ctx).DELETE("/api/projects/{key}", {
    params: { path: { key } },
  });
  if (!response.ok) fail(error, "request failed");
  emit(ctx, { deleted: key }, () => say(ctx, "cli.delete.done", { key }));
}

export interface ImportOptions {
  path: string;
  key?: string;
  title?: string;
  include: string[];
  dryRun: boolean;
}

export async function importProject(ctx: Context, options: ImportOptions): Promise<void> {
  const scope = resolveScope(resolve(ctx.cwd, options.path), options.include);
  const files = listFiles(scope);
  const key = options.key ?? defaultKey(scope.app === "." ? scope.root : scope.app);
  const source = describeSource(scope.root);
  const commit = repoCommit(scope.root);
  const charetJson = {
    source,
    app: scope.app,
    packages: scope.packages,
    rootFiles: scope.rootFiles,
  };
  if (options.dryRun) {
    emit(ctx, { key, ...charetJson, files }, () => {
      say(ctx, "cli.import.plan", { key, count: String(files.length), source });
      // Small apps: every file. Large ones: the folders and root files (`--json` has them all).
      if (files.length <= 50) {
        for (const file of files) ctx.out(`  ${file}`);
      } else {
        for (const dir of scope.packages) ctx.out(`  ${dir}/`);
        for (const file of scope.rootFiles) ctx.out(`  ${file}`);
        say(ctx, "cli.import.planMore");
      }
    });
    return;
  }
  const payload = files.map((path) => {
    const abs = join(scope.root, path);
    return {
      path,
      content: readFileSync(abs).toString("base64"),
      executable: (statSync(abs).mode & 0o111) !== 0,
    };
  });
  payload.push({
    path: "charet.json",
    content: Buffer.from(`${JSON.stringify(charetJson, null, 2)}\n`).toString("base64"),
    executable: false,
  });
  const { data, error } = await client(ctx).POST("/api/projects", {
    body: {
      key,
      ...(options.title ? { title: options.title } : {}),
      import: { source, ...(commit ? { commit } : {}), files: payload },
    },
  });
  if (!data) fail(error, "import failed");
  emit(ctx, data, () =>
    say(ctx, "cli.import.done", { key: data.project.key, count: String(files.length) }),
  );
}

/** A project key from a folder name: lowercase letters, digits and dashes. */
export function defaultKey(path: string): string {
  const key = basename(path)
    .toLowerCase()
    .replace(/[^a-z0-9-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 63);
  return key || "project";
}

export async function fetchArchive(
  ctx: Context,
  server: string,
  project: string,
  rev?: string,
): Promise<{ commit: string; archive: Buffer }> {
  const { data, error, response } = await client(ctx, server).GET("/api/projects/{key}/checkout", {
    params: { path: { key: project }, query: rev ? { rev } : {} },
    parseAs: "arrayBuffer",
  });
  if (!data) fail(error as ApiError | undefined, "checkout failed");
  const commit = response.headers.get("x-charet-commit");
  if (!commit) throw new CliError("server sent no commit");
  return { commit, archive: Buffer.from(data as ArrayBuffer) };
}

export async function checkout(
  ctx: Context,
  project: string,
  dir: string | undefined,
  rev: string | undefined,
  branch?: number,
): Promise<void> {
  const target = resolve(ctx.cwd, dir ?? project);
  if (existsSync(target) && readdirSync(target).length > 0) {
    throw new CliError(t("cli.error.notEmpty", { dir: target }, ctx.locale));
  }
  const { commit, archive } = await fetchArchive(
    ctx,
    ctx.server,
    project,
    branch ? `b${branch}` : rev,
  );
  unpack(target, archive);
  writeState(target, { server: ctx.server, project, base: commit, ...(branch ? { branch } : {}) });
  initBase(target, commit);
  emit(ctx, { dir: target, project, base: commit, ...(branch ? { branch } : {}) }, () =>
    say(ctx, branch ? "cli.checkout.branch" : "cli.checkout.done", {
      project,
      dir: target,
      commit: commit.slice(0, 10),
      number: String(branch ?? ""),
    }),
  );
}

export function requireCheckout(ctx: Context): { dir: string; state: CheckoutState } {
  const found = findCheckout(ctx.cwd);
  if (!found) throw new CliError(t("cli.error.noCheckout", {}, ctx.locale));
  return found;
}

/**
 * Sends local changes. A checkout of main starts a new branch (a draft) with them and moves over to
 * it; a branch checkout adds them to its branch. `merge` merges the branch right after, and the
 * checkout returns to main.
 */
export async function push(
  ctx: Context,
  message: string | undefined,
  merge = false,
): Promise<void> {
  const { dir, state } = requireCheckout(ctx);
  const changes = localChanges(dir);
  if (changes.length === 0 && !(merge && state.branch)) {
    say(ctx, "cli.push.nothing");
    return;
  }
  const api = client(ctx, state.server);
  let branch: Schemas["Branch"] | undefined;
  if (changes.length > 0) {
    const files = changes.map((c) =>
      c.content
        ? { path: c.path, content: c.content.toString("base64"), executable: c.executable }
        : { path: c.path },
    );
    const sent = state.branch
      ? await api.POST("/api/projects/{key}/branches/{number}/push", {
          params: { path: { key: state.project, number: state.branch } },
          body: { base: state.base, files, ...(message ? { message } : {}) },
        })
      : await api.POST("/api/projects/{key}/branches", {
          params: { path: { key: state.project } },
          body: { title: message ?? "", from: state.base, files },
        });
    if (!sent.data) {
      if (sent.error?.code === "conflict") {
        throw new CliError(t("cli.push.conflict", {}, ctx.locale));
      }
      fail(sent.error, "push failed");
    }
    branch = sent.data;
    writeState(dir, { ...state, branch: branch.number, base: branch.head });
    commitBase(dir, branch.head);
  }
  const number = branch?.number ?? (state.branch as number);
  if (!merge) {
    const done = branch as Schemas["Branch"];
    emit(ctx, done, () =>
      say(ctx, "cli.push.branch", {
        count: String(changes.length),
        number: String(done.number),
        url: branchUrl(state.server, state.project, done.number),
      }),
    );
    return;
  }
  const version = await mergeBranch(ctx, state.server, state.project, number);
  await switchToMain(ctx, dir);
  emit(ctx, version, () => {
    say(ctx, "cli.merge.done", { number: String(number), version: String(version.number) });
    say(ctx, "cli.merge.backOnMain", { version: String(version.number) });
  });
}

/**
 * After `charet branch merge`: a checkout that followed the merged branch goes back to main, so
 * the next `capture` from it updates the board. Local changes keep it where it is.
 */
export async function returnToMain(ctx: Context, number: number, version: number): Promise<void> {
  const found = findCheckout(ctx.cwd);
  if (found?.state.branch !== number) return;
  if (localChanges(found.dir).length > 0) {
    if (!ctx.json) say(ctx, "cli.merge.stayed", { number: String(number) });
    return;
  }
  await switchToMain(ctx, found.dir);
  if (!ctx.json) say(ctx, "cli.merge.backOnMain", { version: String(version) });
}

/** Where a branch is shown in the web tool. */
export function branchUrl(server: string, project: string, number: number): string {
  return `${server}/p/${encodeURIComponent(project)}/branches/${number}`;
}

/** Merges branch `number` into main; a clash names the files. */
export async function mergeBranch(
  ctx: Context,
  server: string,
  project: string,
  number: number,
): Promise<Schemas["Version"]> {
  const merge = () =>
    client(ctx, server).POST("/api/projects/{key}/branches/{number}/merge", {
      params: { path: { key: project, number } },
    });
  let { data, error } = await merge();
  // The server first builds and checks what merging gives; it answers `pending` meanwhile.
  for (let i = 0; !data && error?.code === "pending" && i < 600; i++) {
    if (i === 0) ctx.err(t("cli.merge.checking", {}, ctx.locale));
    await new Promise((done) => setTimeout(done, ctx.pollMs ?? 1000));
    ({ data, error } = await merge());
  }
  if (!data) {
    const detail = error?.detail as { files?: string[]; issues?: string[] } | undefined;
    const vars = { number: String(number), files: (detail?.files ?? []).join(", ") };
    if (error?.code === "clash") throw new CliError(t("cli.merge.clash", vars, ctx.locale));
    if (error?.code === "approval_required") {
      throw new CliError(t("cli.merge.approval", vars, ctx.locale));
    }
    if (error?.code === "check_failed") {
      const issues = (detail?.issues ?? []).map((i) => `\n  ${i}`).join("");
      throw new CliError(t("cli.merge.checkFailed", { ...vars, issues }, ctx.locale));
    }
    fail(error, "merge failed");
  }
  return data;
}

/** Replaces the checkout's files with main's (no local changes may be pending). */
async function switchToMain(ctx: Context, dir: string): Promise<string> {
  const found = findCheckout(dir);
  if (!found) throw new CliError(t("cli.error.noCheckout", {}, ctx.locale));
  const { state } = found;
  const { commit, archive } = await fetchArchive(ctx, state.server, state.project);
  replaceFiles(dir, archive);
  const { branch: _left, ...rest } = state;
  writeState(dir, { ...rest, base: commit });
  commitBase(dir, commit);
  return commit;
}

/** Swaps the base's files for an archive's. Ignored files (node_modules, build output) stay. */
function replaceFiles(dir: string, archive: Buffer): void {
  for (const path of git(dir, ["ls-files", "-z"]).toString().split("\0").filter(Boolean)) {
    rmSync(join(dir, path), { force: true });
  }
  unpack(dir, archive);
}

/** Updates the checkout to its branch's head, or to main. A branch that was merged or archived
 * hands the checkout back to main. */
export async function pull(ctx: Context): Promise<void> {
  const { dir, state } = requireCheckout(ctx);
  if (localChanges(dir).length > 0) throw new CliError(t("cli.pull.dirty", {}, ctx.locale));
  if (state.branch) {
    const { data, error } = await client(ctx, state.server).GET(
      "/api/projects/{key}/branches/{number}",
      { params: { path: { key: state.project, number: state.branch } } },
    );
    if (!data) fail(error, "request failed");
    if (data.status === "merged" || data.status === "archived") {
      const commit = await switchToMain(ctx, dir);
      emit(ctx, { base: commit }, () =>
        say(ctx, "cli.pull.closed", {
          number: String(state.branch),
          status: data.status,
          commit: commit.slice(0, 10),
        }),
      );
      return;
    }
  }
  const { commit, archive } = await fetchArchive(
    ctx,
    state.server,
    state.project,
    state.branch ? `b${state.branch}` : undefined,
  );
  if (commit === state.base) {
    say(ctx, "cli.pull.upToDate");
    return;
  }
  replaceFiles(dir, archive);
  writeState(dir, { ...state, base: commit });
  commitBase(dir, commit);
  emit(ctx, { base: commit }, () => say(ctx, "cli.pull.done", { commit: commit.slice(0, 10) }));
}

export async function versions(ctx: Context, project: string): Promise<void> {
  const { data, error } = await client(ctx).GET("/api/projects/{key}/versions", {
    params: { path: { key: project } },
  });
  if (!data) fail(error, "request failed");
  emit(ctx, data, () => {
    for (const v of data.versions as Schemas["Version"][]) {
      ctx.out(`v${v.number}\t${v.kind}\t${v.commit.slice(0, 10)}\t${v.actor}\t${v.note}`);
    }
  });
}

export async function diff(
  ctx: Context,
  project: string,
  from: string,
  to?: string,
): Promise<void> {
  const { data, error } = await client(ctx).GET("/api/projects/{key}/diff", {
    params: { path: { key: project }, query: { from, ...(to ? { to } : {}) } },
  });
  if (!data) fail(error, "request failed");
  const mark = { added: "A", modified: "M", deleted: "D" } as const;
  emit(ctx, data, () => {
    for (const c of data.changes) ctx.out(`${mark[c.kind]}\t${c.path}`);
  });
}
