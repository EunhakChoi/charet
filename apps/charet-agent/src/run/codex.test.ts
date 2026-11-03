import assert from "node:assert/strict";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, test } from "node:test";
import type { RunSpec } from "../protocol.ts";
import { codexAuthFile, codexOptions } from "./codex.ts";

const spec: RunSpec = {
  id: 1,
  agent: "codex",
  model: "",
  apiKey: "sk-api-key",
  project: "web",
  branch: 2,
  prompt: "go",
  context: null,
  history: [],
  images: [],
};

const oauth = {
  provider: "openai",
  accessToken: "access.jwt.token",
  idToken: "id.jwt.token",
  accountId: "acct-123",
} as const;

const login: RunSpec = { ...spec, apiKey: "", oauth };

const mode = (path: string) => statSync(path).mode & 0o777;

let root = "";
let count = 0;
/** A fresh home directory that does not exist yet. */
const newHome = () => join(root, `home-${count++}`);

before(() => {
  root = mkdtempSync(join(tmpdir(), "charet-codex-test-"));
});

after(() => {
  rmSync(root, { recursive: true, force: true });
});

test("the auth file is Codex's ChatGPT login with a placeholder refresh token", () => {
  const now = new Date("2026-11-03T12:34:56.789Z");
  assert.deepEqual(JSON.parse(codexAuthFile(oauth, now)), {
    auth_mode: "chatgpt",
    OPENAI_API_KEY: null,
    tokens: {
      id_token: "id.jwt.token",
      access_token: "access.jwt.token",
      refresh_token: "charet-managed",
      account_id: "acct-123",
    },
    last_refresh: "2026-11-03T12:34:56.789Z",
  });
});

test("a run with a key gets the key and the environment it had", () => {
  const home = newHome();
  const options = codexOptions(
    spec,
    { PATH: "/bin", CODEX_HOME: "/elsewhere", GONE: undefined },
    home,
  );
  assert.deepEqual(options, {
    apiKey: "sk-api-key",
    env: { PATH: "/bin", CODEX_HOME: "/elsewhere" },
  });
  assert.equal(existsSync(home), false, "no files for a key");
});

test("a run with a login gets a private CODEX_HOME with auth.json, and no key", () => {
  const home = newHome();
  const now = new Date("2026-11-03T00:00:00Z");
  const options = codexOptions(
    login,
    {
      PATH: "/bin",
      CODEX_API_KEY: "host-key",
      OPENAI_API_KEY: "host-key",
      CODEX_HOME: "/elsewhere",
    },
    home,
    now,
  );
  const codexHome = join(home, ".codex");
  assert.deepEqual(options, {
    env: { PATH: "/bin", CODEX_HOME: codexHome },
    config: { cli_auth_credentials_store: "file" },
  });
  assert.equal("apiKey" in options, false);
  assert.equal(mode(codexHome), 0o700);
  const file = join(codexHome, "auth.json");
  assert.equal(mode(file), 0o600);
  assert.equal(readFileSync(file, "utf8"), codexAuthFile(oauth, now));
});

test("an existing Codex home keeps its files (the skills) and becomes private", () => {
  const home = newHome();
  const codexHome = join(home, ".codex");
  mkdirSync(join(codexHome, "skills", "charet"), { recursive: true });
  chmodSync(codexHome, 0o755);
  codexOptions(login, {}, home);
  assert.equal(existsSync(join(codexHome, "skills", "charet")), true);
  assert.equal(mode(codexHome), 0o700);
  assert.equal(mode(join(codexHome, "auth.json")), 0o600);
});

test("a login of another vendor is refused before anything is written", () => {
  const home = newHome();
  const other: RunSpec = { ...login, oauth: { provider: "anthropic", accessToken: "t" } };
  assert.throws(() => codexOptions(other, {}, home), /anthropic login/);
  assert.equal(existsSync(home), false);
});
