import assert from "node:assert/strict";
import {
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { after, test } from "node:test";
import { run } from "./main.ts";
import { browserUrl, newer, publishes } from "./system.ts";

// The skill lookup walks up from the working directory, so the sandbox stays out of any project
// that has a charet skill of its own (a machine's TMPDIR can sit inside one).
const sandboxRoot = existsSync("/tmp") ? "/tmp" : tmpdir();
const home = mkdtempSync(join(sandboxRoot, "charet-system-"));
/** A second sandbox: `home` holds an installed skill, which a lookup from inside it would find. */
const alone = mkdtempSync(join(sandboxRoot, "charet-alone-"));
const SKILL_DIR = resolve(import.meta.dirname, "../../../skills/charet");
after(() => {
  rmSync(home, { recursive: true, force: true });
  rmSync(alone, { recursive: true, force: true });
});

const json = (body: unknown) =>
  new Response(JSON.stringify(body), { headers: { "content-type": "application/json" } });

function system(overrides: Record<string, unknown> = {}) {
  return {
    version: "0.0.0",
    latest: null,
    toolUrl: "http://localhost:5280",
    previewUrl: "http://127.0.0.1:5281",
    localMode: true,
    services: {
      workspace: { ok: true },
      render: { ok: false, error: "charet-render: connection refused" },
    },
    ...overrides,
  };
}

async function charet(
  args: string[],
  options: {
    answers?: string[];
    sh?: (c: string, a: string[]) => string | undefined;
    info?: unknown;
    env?: NodeJS.ProcessEnv;
    cwd?: string;
  } = {},
) {
  const lines: string[] = [];
  const asked: string[] = [];
  const answers = [...(options.answers ?? [])];
  const code = await run(args, {
    locale: "en",
    cwd: options.cwd ?? home,
    env: { HOME: home, CHARET_CONFIG_DIR: join(home, "config"), ...options.env },
    fetch: async () =>
      options.info === undefined
        ? Promise.reject(new TypeError("fetch failed"))
        : json(options.info),
    out: (line) => lines.push(line),
    err: (line) => lines.push(`[stderr] ${line}`),
    ...(options.answers
      ? {
          ask: async (q: string) => {
            asked.push(q);
            return answers.shift() ?? "";
          },
        }
      : {}),
    sh: options.sh ?? (() => undefined),
  });
  return { code, out: lines.join("\n"), asked };
}

test("compares versions numerically", () => {
  assert.ok(newer("0.10.0", "0.9.1"));
  assert.ok(newer("1.0.0", "1.0.0-rc.1"));
  assert.ok(!newer("0.1.0", "0.1.0"));
  assert.ok(!newer("0.1.0", "0.2.0"));
});

test("setup asks to star once, stars with gh, and installs the skill", async () => {
  mkdirSync(join(home, ".claude"), { recursive: true });
  const calls: string[] = [];
  const sh = (command: string, args: string[]) => {
    calls.push([command, ...args].join(" "));
    return command === "gh" ? "" : undefined;
  };
  const first = await charet(["setup"], { answers: ["", ""], sh, info: system() });
  assert.equal(first.code, 0, first.out);
  assert.deepEqual(first.asked, [
    "Star charet on GitHub? [Y/n] ",
    "Install the charet skill for claude? [Y/n] ",
  ]);
  assert.ok(calls.includes("gh api -X PUT /user/starred/EunhakChoi/charet --silent"));
  assert.match(first.out, /Starred/);
  assert.ok(existsSync(join(home, ".claude/skills/charet/SKILL.md")));
  assert.ok(existsSync(join(home, ".claude/skills/charet/references/cli.md")));
  assert.ok(existsSync(join(home, ".claude/skills/charet/references/import.md")));
  assert.ok(!existsSync(join(home, ".claude/skills/charet/evals")));
  assert.equal(JSON.parse(readFileSync(join(home, "config/settings.json"), "utf8")).star, "yes");

  // An identical copy is already there: said so, not asked about.
  const again = await charet(["setup"], { answers: [], sh, info: system() });
  assert.deepEqual(again.asked, []);
  assert.match(again.out, /charet skill for claude in .* is already up to date/);
});

test("setup signs the CLI in as the owner of a charet on this machine", async () => {
  const token = "ab".repeat(16);
  const lines: string[] = [];
  const calls: string[] = [];
  const code = await run(["setup", "--yes"], {
    locale: "en",
    cwd: home,
    env: {
      HOME: home,
      CHARET_CONFIG_DIR: join(home, "owner"),
      CHARET_URL: "http://localhost:5280",
    },
    fetch: (async (input: Request) => {
      if (new URL(input.url).pathname !== "/api/auth/me") return json(system());
      return input.headers.get("authorization") === `Bearer ${token}`
        ? json({ kind: "user", id: "u1", name: "Admin", role: "admin", localMode: true })
        : json({ kind: "visitor", id: "v1", name: "Fox", role: "viewer", localMode: true });
    }) as typeof fetch,
    out: (line) => lines.push(line),
    err: (line) => lines.push(line),
    sh: (command, args) => {
      calls.push([command, ...args].join(" "));
      if (args[0] === "ps") return "abc123 127.0.0.1:5280->5280/tcp, 127.0.0.1:5284->5284/tcp";
      if (args[0] === "exec") return token;
      return undefined;
    },
  });
  assert.equal(code, 0, lines.join("\n"));
  assert.ok(calls.includes("docker exec abc123 charet-server admin-token"));
  assert.match(lines.join("\n"), new RegExp(`http://localhost:5280/\\?t=${token}`));
  const saved = JSON.parse(readFileSync(join(home, "owner/credentials.json"), "utf8"));
  assert.equal(saved["http://localhost:5280"].token, token);
});

test("setup never asks without a terminal or with --yes", async () => {
  mkdirSync(join(alone, "h2/.claude"), { recursive: true });
  const quiet = await charet(["setup", "--yes"], {
    answers: [],
    cwd: alone,
    env: { CHARET_CONFIG_DIR: join(alone, "c2"), HOME: join(alone, "h2") },
  });
  assert.deepEqual(quiet.asked, []);
  assert.match(quiet.out, /No server at/);
  assert.match(quiet.out, /Installed the charet skill for claude/);
});

test("setup leaves the skill alone where an agent already finds one from here", async () => {
  const user = join(home, "h3");
  mkdirSync(join(user, ".claude"), { recursive: true });
  mkdirSync(join(user, ".codex"), { recursive: true });
  // A shared project folder above the working directory, as `.claude/skills` and `.agents/skills`.
  const project = join(home, "shared");
  for (const folder of [".claude", ".agents"]) {
    mkdirSync(join(project, folder, "skills/charet"), { recursive: true });
    writeFileSync(join(project, folder, "skills/charet/SKILL.md"), "mine\n");
  }
  const cwd = join(project, "app");
  mkdirSync(cwd, { recursive: true });
  const setup = await charet(["setup", "--yes"], { cwd, env: { HOME: user } });
  assert.match(
    setup.out,
    /claude already finds a charet skill at .*shared\/\.claude\/skills\/charet/,
  );
  assert.match(
    setup.out,
    /codex already finds a charet skill at .*shared\/\.agents\/skills\/charet/,
  );
  assert.ok(!existsSync(join(user, ".claude/skills/charet")));
  assert.ok(!existsSync(join(user, ".codex/skills/charet")));
  assert.equal(readFileSync(join(project, ".claude/skills/charet/SKILL.md"), "utf8"), "mine\n");

  // Asked for outright, `skill install` still puts a copy in the agent's own folder.
  const forced = await charet(["skill", "install", "--codex"], { cwd, env: { HOME: user } });
  assert.match(forced.out, /Installed the charet skill for codex/);
  assert.ok(existsSync(join(user, ".codex/skills/charet/SKILL.md")));
});

test("skill install skips a current copy, updates an old one and leaves a link to the repo", async () => {
  const user = join(home, "h4");
  mkdirSync(join(user, ".claude"), { recursive: true });
  const env = { HOME: user };
  const first = await charet(["skill", "install", "--claude"], { env });
  assert.match(first.out, /Installed the charet skill for claude/);
  const installed = join(user, ".claude/skills/charet/SKILL.md");
  assert.match((await charet(["skill", "install", "--claude"], { env })).out, /already up to date/);

  writeFileSync(installed, "an old copy\n");
  assert.match(
    (await charet(["skill", "install", "--claude"], { env })).out,
    /Installed the charet skill/,
  );
  assert.equal(readFileSync(installed, "utf8"), readFileSync(join(SKILL_DIR, "SKILL.md"), "utf8"));

  const linked = join(home, "h5");
  mkdirSync(join(linked, ".claude/skills"), { recursive: true });
  symlinkSync(SKILL_DIR, join(linked, ".claude/skills/charet"));
  const result = await charet(["skill", "install", "--claude"], { env: { HOME: linked } });
  assert.equal(result.code, 0);
  assert.match(result.out, /link to this repository's copy/);
  assert.ok(lstatSync(join(linked, ".claude/skills/charet")).isSymbolicLink());
});

test("skill install picks agents", async () => {
  const result = await charet(["skill", "install", "--codex"]);
  assert.equal(result.code, 0);
  assert.ok(existsSync(join(home, ".codex/skills/charet/SKILL.md")));
  assert.equal((await charet(["skill", "nope"])).code, 1);
});

test("doctor checks that a shared local charet's address answers", async () => {
  const result = await charet(["doctor"], {
    info: system({ toolUrl: "https://kim.tail1.ts.net" }),
  });
  assert.match(result.out, /✓ shared at https:\/\/kim\.tail1\.ts\.net/);
});

test("doctor reports failing services and newer releases", async () => {
  const sh = (command: string) => (command === "git" ? "git version 2.43.0" : undefined);
  const result = await charet(["doctor"], { sh, info: system({ latest: "0.2.0" }) });
  assert.equal(result.code, 1);
  assert.match(result.out, /✓ git version 2\.43\.0/);
  assert.match(result.out, /✓ charet-workspace/);
  assert.match(result.out, /✗ charet-render: charet-render: connection refused\n {4}Start it/);
  assert.match(
    result.out,
    /✓ web tool http:\/\/localhost:5280, previews http:\/\/127\.0\.0\.1:5281/,
  );
  assert.match(result.out, /! charet 0\.2\.0 is out/);
  assert.match(result.out, /- docker: not found/);

  const shared = await charet(["doctor", "--json"], {
    sh,
    info: system({
      previewUrl: "http://localhost:5280",
      services: { workspace: { ok: true }, render: { ok: true } },
    }),
  });
  const report = JSON.parse(shared.out);
  assert.ok(report.checks.some((c: { text: string }) => /served sandboxed/.test(c.text)));
  // Another port on the same host is its own origin.
  const ports = await charet(["doctor"], {
    sh,
    info: system({
      previewUrl: "http://localhost:5281",
      services: { workspace: { ok: true }, render: { ok: true } },
    }),
  });
  assert.equal(ports.code, 0, ports.out);

  const open = await charet(["doctor"], {
    sh,
    info: system({
      localMode: false,
      toolUrl: "http://charet.example.com",
      previewUrl: "http://preview.example.com",
      services: { workspace: { ok: true }, render: { ok: true } },
    }),
  });
  assert.equal(open.code, 1);
  assert.match(open.out, /✗ public address http:\/\/charet\.example\.com is not HTTPS/);

  const down = await charet(["doctor"], { sh });
  assert.match(down.out, /✗ server http:\/\/127\.0\.0\.1:5280: unreachable/);
});

test("finds the server container publishing a port, ranges included", () => {
  assert.equal(
    publishes("b53e 127.0.0.1:5280-5281->5280-5281/tcp, 127.0.0.1:5284->5284/tcp", 5280, 5280),
    true,
  );
  assert.equal(publishes("x 0.0.0.0:5380->5280/tcp", 5380, 5280), true);
  assert.equal(publishes("x 127.0.0.1:5380-5381->5280-5281/tcp", 5381, 5280), false);
  assert.equal(publishes("x 127.0.0.1:8300->8000/tcp", 8300, 5280), false);
  assert.equal(publishes("x 127.0.0.1:5280-5281->5280-5281/tcp", 5281, 5280), false);
});

test("the admin link names localhost, not a loopback IP the previews use", () => {
  assert.equal(browserUrl("http://127.0.0.1:5280"), "http://localhost:5280");
  assert.equal(browserUrl("http://[::1]:5280/"), "http://localhost:5280");
  assert.equal(browserUrl("https://charet.example.com/"), "https://charet.example.com");
});
