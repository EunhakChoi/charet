import assert from "node:assert/strict";
import { type ChildProcess, execFileSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { after, before, test } from "node:test";
import { defaultKey } from "./commands.ts";
import { parse, run } from "./main.ts";

const SERVER_BIN =
  process.env.CHARET_SERVER_BIN ??
  resolve(import.meta.dirname, "../../../target/debug/charet-server");

async function freePort(): Promise<number> {
  return new Promise((done) => {
    const server = createServer().listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close(() => done(typeof address === "object" && address ? address.port : 0));
    });
  });
}

let server: ChildProcess | undefined;
let url = "";
let work = "";
/** The owner's token: nobody is the admin by address, the CLI here included. */
let token = "";

before(async () => {
  assert.ok(existsSync(SERVER_BIN), `build charet-server first (${SERVER_BIN})`);
  work = mkdtempSync(join(tmpdir(), "charet-cli-"));
  const port = await freePort();
  const previewPort = await freePort();
  const remotePort = await freePort();
  url = `http://127.0.0.1:${port}`;
  const serverEnv = {
    ...process.env,
    CHARET_HOME: join(work, "home"),
    CHARET_BIND: `127.0.0.1:${port}`,
    CHARET_PREVIEW_BIND: `127.0.0.1:${previewPort}`,
    CHARET_REMOTE_BIND: `127.0.0.1:${remotePort}`,
    CHARET_UPDATE_CHECK: "false",
    // No build service here to check merges with.
    CHARET_MERGE_CHECK: "false",
  };
  server = spawn(SERVER_BIN, [], { env: serverEnv, stdio: "ignore" });
  for (let i = 0; i < 100; i++) {
    try {
      if ((await fetch(`${url}/api/health`)).ok) {
        token = execFileSync(SERVER_BIN, ["admin-token"], { env: serverEnv }).toString().trim();
        return;
      }
    } catch {}
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error("charet-server did not start");
});

after(() => {
  server?.kill();
  rmSync(work, { recursive: true, force: true });
});

async function charet(
  args: string[],
  cwd = work,
  env: Record<string, string> = { CHARET_URL: url, CHARET_TOKEN: token },
) {
  const out: string[] = [];
  const err: string[] = [];
  const code = await run(args, {
    cwd,
    locale: "en",
    env,
    out: (line) => out.push(line),
    err: (line) => err.push(line),
  });
  return { code, out: out.join("\n"), err: err.join("\n") };
}

function write(root: string, files: Record<string, string>) {
  for (const [path, content] of Object.entries(files)) {
    mkdirSync(dirname(join(root, path)), { recursive: true });
    writeFileSync(join(root, path), content);
  }
}

test("parses flags, values and repeated lists", () => {
  const args = parse(["import", "app", "--key=web", "--include", "a", "--include", "b", "--json"]);
  assert.deepEqual(args.positional, ["import", "app"]);
  assert.equal(args.flags.get("--key"), "web");
  assert.equal(args.flags.get("--json"), true);
  assert.deepEqual(args.lists.get("--include"), ["a", "b"]);
});

test("derives a project key from a folder", () => {
  assert.equal(defaultKey("/x/Admin Web_2"), "admin-web-2");
  assert.equal(defaultKey("/x/___"), "project");
});

test("prints the version and usage", async () => {
  assert.deepEqual(await charet(["--version"]), { code: 0, out: "charet 0.0.0", err: "" });
  assert.match((await charet([])).out, /^Usage: charet/);
  assert.equal((await charet(["nope"])).code, 1);
  // --help on a command prints usage instead of running it.
  const help = await charet(["import", "--help"]);
  assert.equal(help.code, 0);
  assert.match(help.out, /^Usage: charet/);
  assert.doesNotMatch((await charet(["projects"])).out, /^charet-cli/m);
});

test("deletes a project only when confirmed", async () => {
  const app = join(work, "gone-app");
  write(app, { "package.json": "{}", "src/a.ts": "export {};\n" });
  assert.equal((await charet(["import", app, "--key", "gone"])).code, 0);
  const refused = await charet(["delete", "gone"]);
  assert.equal(refused.code, 1);
  assert.match(refused.err, /add --yes/);
  const deleted = await charet(["delete", "gone", "--yes"]);
  assert.equal(deleted.code, 0, deleted.err);
  assert.match(deleted.out, /Deleted project gone/);
  assert.doesNotMatch((await charet(["projects"])).out, /^gone\t/m);
  assert.equal((await charet(["delete", "gone", "--yes"])).code, 1);
});

test("import, checkout, push, pull, versions and diff", async () => {
  const repo = join(work, "repo");
  write(repo, {
    "pnpm-workspace.yaml": "packages:\n  - apps/*\n  - packages/*\n",
    "package.json": "{}",
    "apps/web/package.json": JSON.stringify({
      name: "web",
      dependencies: { "@x/api": "workspace:*" },
    }),
    "apps/web/src/page.tsx": "export const title = 'one';\n",
    "apps/server/package.json": JSON.stringify({ name: "server" }),
    "packages/api/package.json": JSON.stringify({ name: "@x/api" }),
    "packages/api/index.ts": "export {};\n",
  });

  const plan = await charet(["import", join(repo, "apps/web"), "--dry-run", "--json"]);
  const planned = JSON.parse(plan.out);
  assert.equal(planned.key, "web");
  assert.deepEqual(planned.packages, ["apps/web", "packages/api"]);
  const dry = await charet(["import", join(repo, "apps/web"), "--dry-run"]);
  assert.match(dry.out, /^ {2}apps\/web\/src\/page\.tsx$/m, "a small app lists every file");

  const imported = await charet(["import", join(repo, "apps/web"), "--key", "demo"]);
  assert.equal(imported.code, 0, imported.err);
  assert.match(imported.out, /Imported 6 files as project demo/);
  assert.match((await charet(["projects"])).out, /^demo\t/);

  const a = join(work, "a");
  assert.equal((await charet(["checkout", "demo", a])).code, 0);
  assert.equal(
    readFileSync(join(a, "apps/web/src/page.tsx"), "utf8"),
    "export const title = 'one';\n",
  );
  assert.ok(existsSync(join(a, "charet.json")));
  assert.match((await charet(["push"], a)).out, /Nothing to push/);
  // Inside a checkout, commands talk to its server, whatever CHARET_URL says.
  const listed = await charet(["versions"], a, {
    CHARET_URL: "http://127.0.0.1:9",
    CHARET_TOKEN: token,
  });
  assert.equal(listed.code, 0, listed.err);
  assert.match(listed.out, /v1/);

  // Ignored output never travels. A push from main starts a branch; --merge takes it in at once.
  write(a, {
    ".gitignore": "node_modules/\n",
    "node_modules/x/index.js": "",
    "apps/web/src/page.tsx": "export const title = 'two';\n",
  });
  const pushed = await charet(["push", "-m", "retitle", "--merge"], a);
  assert.equal(pushed.code, 0, pushed.err);
  assert.match(pushed.out, /Merged branch #1 as version 2/);
  assert.match((await charet(["status"], a)).out, /main {5}v2 \w+\nchanges {2}0$/);

  // A stale checkout still pushes (to a new branch), but merging it clashes.
  const b = join(work, "b");
  await charet(["checkout", "demo", b, "--rev", "v1"]);
  write(b, { "apps/web/src/page.tsx": "stale\n" });
  const drafted = await charet(["push", "-m", "stale title"], b);
  assert.equal(drafted.code, 0, drafted.err);
  assert.match(drafted.out, /to branch #2 .*\/p\/demo\/branches\/2/);
  assert.match((await charet(["status"], b)).out, /^branch {3}#2$/m);
  const clash = await charet(["branch", "merge"], b);
  assert.equal(clash.code, 1);
  assert.match(clash.err, /Branch #2 clashes with main in apps\/web\/src\/page\.tsx/);
  assert.match(
    (await charet(["branches", "-p", "demo"])).out,
    /^#2\topen\tAdmin\tstale title\tclash: apps\/web\/src\/page\.tsx$/m,
  );
  const shown = (await charet(["branch", "2", "-p", "demo"])).out;
  assert.match(shown, /^#2 stale title\nstatus {3}open\n/);
  assert.match(shown, /changes {2}1\n {2}M apps\/web\/src\/page\.tsx/);

  // Two checkouts of one branch: the second push from the old head is refused.
  const e = join(work, "e");
  assert.equal((await charet(["checkout", "demo", e, "--branch", "2"])).code, 0);
  write(e, { "extra.txt": "e\n" });
  assert.equal((await charet(["push"], e)).code, 0);
  write(b, { "other.txt": "b\n" });
  const conflict = await charet(["push"], b);
  assert.equal(conflict.code, 1);
  assert.match(conflict.err, /Someone pushed first/);
  assert.match((await charet(["pull"], b)).err, /unpushed changes/);
  rmSync(join(b, "other.txt"));
  assert.match((await charet(["pull"], b)).out, /Updated to/);
  assert.ok(existsSync(join(b, "extra.txt")));

  // Archived: the branch checkout goes back to main on pull.
  assert.match((await charet(["branch", "archive"], b)).out, /Archived branch #2/);
  assert.match(
    (await charet(["pull"], b)).out,
    /Branch #2 is archived; this checkout is back on main/,
  );
  assert.equal(
    readFileSync(join(b, "apps/web/src/page.tsx"), "utf8"),
    "export const title = 'two';\n",
  );
  assert.match((await charet(["branches", "-p", "demo"])).out, /No open branches/);

  const c = join(work, "c");
  await charet(["checkout", "demo", c, "--rev", "v1"]);
  write(c, { "apps/web/src/page.tsx": "local\n" });
  const behind = (await charet(["status"], c)).out;
  assert.match(behind, /^project {2}demo /);
  assert.match(behind, /base {5}v1 /);
  assert.match(behind, /main {5}v2 \w+ {2}newer than this checkout/);
  assert.match(behind, /changes {2}1\n {2}M apps\/web\/src\/page\.tsx$/);
  write(c, { "apps/web/src/page.tsx": "export const title = 'one';\n" });
  assert.match((await charet(["pull"], c)).out, /Updated to/);
  assert.equal(
    readFileSync(join(c, "apps/web/src/page.tsx"), "utf8"),
    "export const title = 'two';\n",
  );

  assert.match((await charet(["versions"], a)).out, /^v2\tmerge\t\w+\tAdmin\tretitle/);
  assert.equal(
    (await charet(["diff", "v1", "-p", "demo"])).out,
    "A\t.gitignore\nM\tapps/web/src/page.tsx",
  );
  assert.match((await charet(["versions"])).err, /Which project/);

  // A pull replaces the files of the base and leaves ignored ones (node_modules) alone.
  write(c, { "node_modules/kept.js": "" });
  rmSync(join(a, "apps/web/src/page.tsx"));
  assert.equal((await charet(["push", "-m", "drop page", "--merge"], a)).code, 0);
  assert.match((await charet(["pull"], c)).out, /Updated to/);
  assert.ok(existsSync(join(c, "node_modules/kept.js")));
  assert.ok(!existsSync(join(c, "apps/web/src/page.tsx")));

  // Restore: a new version that undoes v3 brings the page back.
  assert.match((await charet(["restore", "v3", "-p", "demo"])).out, /Version 4 undoes v3/);
  assert.match((await charet(["pull"], c)).out, /Updated to/);
  assert.ok(existsSync(join(c, "apps/web/src/page.tsx")));
  assert.equal((await charet(["restore", "v1", "-p", "demo"])).code, 1);

  // `branch merge` from the checkout that follows the branch brings it back to main, so a capture
  // from it updates the board; local changes keep it on the branch.
  const follow = join(work, "follow");
  await charet(["checkout", "demo", follow]);
  write(follow, { "apps/web/src/page.tsx": "export const title = 'three';\n" });
  assert.equal((await charet(["push", "-m", "retitle again"], follow)).code, 0);
  assert.match((await charet(["status"], follow)).out, /^branch {3}#\d+$/m);
  const merged = await charet(["branch", "merge"], follow);
  assert.equal(merged.code, 0, merged.err);
  assert.match(merged.out, /Merged branch #\d+ as version \d+\./);
  assert.match(merged.out, /back on main \(version \d+\)/);
  assert.match((await charet(["status"], follow)).out, /^main {5}v\d+/m);

  const dirty = join(work, "dirty");
  await charet(["checkout", "demo", dirty]);
  write(dirty, { "apps/web/src/page.tsx": "export const title = 'four';\n" });
  assert.equal((await charet(["push", "-m", "retitle once more"], dirty)).code, 0);
  write(dirty, { "apps/web/src/other.tsx": "export const other = 1;\n" });
  const kept = await charet(["branch", "merge"], dirty);
  assert.equal(kept.code, 0, kept.err);
  assert.match(kept.out, /still follows branch #\d+ \(it has local changes/);
  assert.match((await charet(["status"], dirty)).out, /^branch {3}#\d+$/m);
});

test("init creates a project from a template and checks it out", async () => {
  const dir = join(work, "tasks");
  const result = await charet(["init", "tasks", dir]);
  assert.equal(result.code, 0, result.err);
  assert.match(result.out, /Checked out tasks/);
  assert.ok(existsSync(join(dir, "templates/recommended/.charet/project.tsx")));
  assert.match((await charet(["status"], dir)).out, /changes {2}0$/);
  assert.match((await charet(["init", "x", "--template", "nope"])).err, /No template called nope/);
});

test("hand off to the product repo, then import its own changes", async () => {
  const repo = join(work, "product");
  const sh = (...args: string[]) =>
    execFileSync("git", args, {
      cwd: repo,
      env: {
        ...process.env,
        GIT_AUTHOR_NAME: "t",
        GIT_AUTHOR_EMAIL: "t@t",
        GIT_COMMITTER_NAME: "t",
        GIT_COMMITTER_EMAIL: "t@t",
      },
    })
      .toString()
      .trim();
  write(repo, {
    "package.json": JSON.stringify({ name: "shop" }),
    "src/page.tsx": "export const title = 'one';\n",
    "src/list.tsx": "export const rows = 1;\n",
  });
  sh("init", "-q", "-b", "main");
  sh("add", ".");
  sh("commit", "-qm", "start");
  const start = sh("rev-parse", "HEAD");
  assert.equal((await charet(["import", repo, "--key", "shop"])).code, 0);
  const first = await charet(["status", "-p", "shop"], repo);
  assert.match(first.out, /^baseline {2}v1 import · repo /m);
  assert.match(first.out, /nothing to hand off/);
  assert.match(first.out, /^drift {5}none/m);

  // A change merged in charet (and a mock, which stays in charet).
  const c = join(work, "shop-checkout");
  await charet(["checkout", "shop", c]);
  write(c, { "src/page.tsx": "export const title = 'two';\n", ".charet/mocks.ts": "export {};\n" });
  assert.equal((await charet(["push", "-m", "Title", "--merge"], c)).code, 0);
  assert.match((await charet(["handoff", "-p", "shop"], repo)).out, /to hand off: 1 file\(s\)/);

  const patch = join(work, "shop.patch");
  const exported = await charet(["handoff", "export", "-p", "shop", "--out", patch], repo);
  assert.equal(exported.code, 0, exported.err);
  assert.match(exported.out, /from v1 to v2 \(1 file\(s\)/);
  assert.doesNotMatch(readFileSync(patch, "utf8"), /\.charet/);
  sh("apply", patch);
  sh("commit", "-qam", "From charet");
  assert.equal(readFileSync(join(repo, "src/page.tsx"), "utf8"), "export const title = 'two';\n");
  const marked = await charet(["handoff", "mark", "v2", "-p", "shop"], repo);
  assert.equal(marked.code, 0, marked.err);
  assert.match(marked.out, /Marked v2/);
  assert.match((await charet(["handoff", "-p", "shop"], repo)).out, /baseline {2}v2 handoff/);

  // Someone changes the repo directly: drift, then import it.
  write(repo, { "src/list.tsx": "export const rows = 2;\n" });
  sh("commit", "-qam", "More rows");
  const drift = await charet(["status", "-p", "shop"], repo);
  assert.match(drift.out, /1 commit\(s\), 0 uncommitted/);
  assert.match(drift.out, /More rows/);
  const updated = await charet(["import", "--update", "-p", "shop"], repo);
  assert.equal(updated.code, 0, updated.err);
  assert.match(updated.out, /as version 3/);
  assert.match((await charet(["status", "-p", "shop"], repo)).out, /^drift {5}none/m);
  assert.match((await charet(["versions", "-p", "shop"])).out, /^v3\timport/m);
  assert.notEqual(start, sh("rev-parse", "HEAD"));
});
