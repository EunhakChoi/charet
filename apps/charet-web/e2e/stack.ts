// Starts charet-render, charet-workspace and charet-server on free ports with a scratch home,
// imports templates/recommended as project `demo` and waits for its preview.

import { type ChildProcess, execFileSync, spawn } from "node:child_process";
import { chmodSync, mkdtempSync, openSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dirname, "../../..");

/** Ports handed out so far: the OS may offer a closed one again, and two servers would clash. */
const taken = new Set<number>();

async function freePort(): Promise<number> {
  for (;;) {
    const port = await anyPort();
    if (!taken.has(port)) {
      taken.add(port);
      return port;
    }
  }
}

function anyPort(): Promise<number> {
  return new Promise((done, fail) => {
    const server = createServer();
    server.once("error", fail);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close(() => done(typeof address === "object" && address ? address.port : 0));
    });
  });
}

async function waitFor(
  url: string,
  ok: (body: unknown) => boolean,
  seconds: number,
  token?: string,
): Promise<unknown> {
  const until = Date.now() + seconds * 1000;
  let last: unknown;
  while (Date.now() < until) {
    const headers: Record<string, string> = token ? { authorization: `Bearer ${token}` } : {};
    const response = await fetch(url, { headers }).catch(() => undefined);
    if (response) {
      last = response.headers.get("content-type")?.includes("json")
        ? await response.json()
        : response.status;
      if (response.ok && ok(last)) return last;
    }
    await new Promise((r) => setTimeout(r, 300));
  }
  throw new Error(`timed out waiting for ${url}: ${JSON.stringify(last)}`);
}

export default async function setup() {
  const home = mkdtempSync(join(tmpdir(), "charet-e2e-"));
  const [api, preview, workspace, render, publicApi, publicPreview] = [
    await freePort(),
    await freePort(),
    await freePort(),
    await freePort(),
    await freePort(),
    await freePort(),
  ];
  const [remote, shareApi, sharePreview, shareRemote, agent, agentDoor] = [
    await freePort(),
    await freePort(),
    await freePort(),
    await freePort(),
    await freePort(),
    await freePort(),
  ];
  // A stand-in tailscale CLI for the shared server: connected, with HTTPS names.
  const tailscale = join(home, "tailscale");
  writeFileSync(
    tailscale,
    `#!/bin/sh\ncase "$1 $2" in "status --json") echo '{"BackendState":"Running","Self":{"DNSName":"e2e.tail1.ts.net."},"CertDomains":["e2e.tail1.ts.net"]}';; esac\n`,
  );
  chmodSync(tailscale, 0o755);
  const log = openSync(join(home, "stack.log"), "a");
  const start = (command: string, args: string[], env: Record<string, string>): ChildProcess =>
    spawn(command, args, { env: { ...process.env, ...env }, stdio: ["ignore", log, log] });
  const token = "e2e";
  const publicEnv = {
    CHARET_HOME: join(home, "public"),
    CHARET_BIND: `127.0.0.1:${publicApi}`,
    CHARET_PREVIEW_BIND: `127.0.0.1:${publicPreview}`,
    CHARET_PUBLIC_URL: `http://localhost:${publicApi}`,
    CHARET_WEB_DIR: join(root, "apps/charet-web/dist"),
    CHARET_WORKSPACE_URL: `http://127.0.0.1:${workspace}`,
    CHARET_WORKSPACE_TOKEN: token,
    CHARET_RENDER_URL: `http://127.0.0.1:${render}`,
    CHARET_UPDATE_CHECK: "false",
  };
  const children = [
    start(join(root, "target/debug/charet-render"), [], {
      CHARET_RENDER_BIND: `127.0.0.1:${render}`,
    }),
    start(process.execPath, [join(root, "apps/charet-workspace/src/main.ts")], {
      CHARET_WORKSPACE_HOME: join(home, "workspace"),
      CHARET_WORKSPACE_BIND: `127.0.0.1:${workspace}`,
      CHARET_WORKSPACE_TOKEN: token,
    }),
    // The web agent for the main server: runs as plain processes with the scripted agent.
    start(process.execPath, [join(root, "apps/charet-agent/src/main.ts")], {
      CHARET_AGENT_BIND: `127.0.0.1:${agent}`,
      CHARET_AGENT_TOKEN: token,
      CHARET_AGENT_SANDBOX: "none",
      CHARET_AGENT_DOOR_URL: `http://127.0.0.1:${agentDoor}`,
      CHARET_CLI: join(root, "apps/charet-cli/src/main.ts"),
    }),
    start(join(root, "target/debug/charet-server"), [], {
      CHARET_HOME: join(home, "server"),
      CHARET_AGENT_URL: `http://127.0.0.1:${agent}`,
      CHARET_AGENT_TOKEN: token,
      CHARET_AGENT_BIND: `127.0.0.1:${agentDoor}`,
      CHARET_AGENT_SCRIPT: "true",
      CHARET_BIND: `127.0.0.1:${api}`,
      CHARET_PREVIEW_BIND: `127.0.0.1:${preview}`,
      CHARET_REMOTE_BIND: `127.0.0.1:${remote}`,
      CHARET_PUBLIC_URL: `http://localhost:${api}`,
      CHARET_WEB_DIR: join(root, "apps/charet-web/dist"),
      CHARET_WORKSPACE_URL: `http://127.0.0.1:${workspace}`,
      CHARET_WORKSPACE_TOKEN: token,
      CHARET_RENDER_URL: `http://127.0.0.1:${render}`,
      CHARET_UPDATE_CHECK: "false",
    }),
    // A third, local one that gets shared from its Settings page (`sharing.spec.ts`).
    start(join(root, "target/debug/charet-server"), [], {
      CHARET_HOME: join(home, "share"),
      CHARET_BIND: `127.0.0.1:${shareApi}`,
      CHARET_PREVIEW_BIND: `127.0.0.1:${sharePreview}`,
      CHARET_REMOTE_BIND: `127.0.0.1:${shareRemote}`,
      CHARET_TAILSCALE: tailscale,
      CHARET_PUBLIC_URL: `http://localhost:${shareApi}`,
      CHARET_WEB_DIR: join(root, "apps/charet-web/dist"),
      CHARET_WORKSPACE_URL: `http://127.0.0.1:${workspace}`,
      CHARET_WORKSPACE_TOKEN: token,
      CHARET_RENDER_URL: `http://127.0.0.1:${render}`,
      CHARET_UPDATE_CHECK: "false",
    }),
    // A second server in public mode: everyone comes in by token (or as a visitor).
    start(join(root, "target/debug/charet-server"), ["--public"], publicEnv),
  ];
  const teardown = async () => {
    for (const child of children) child.kill();
    if (!process.env.CHARET_E2E_KEEP) {
      rmSync(home, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
    }
  };
  let adminToken = "";
  let ownerToken = "";
  let shareToken = "";
  // Each server's owner token, as `charet-server admin-token` prints it on that machine.
  const tokenOf = (home: string, args: string[] = []) =>
    execFileSync(join(root, "target/debug/charet-server"), ["admin-token", ...args], {
      env: { ...process.env, CHARET_HOME: home },
    })
      .toString()
      .trim();
  try {
    const base = `http://127.0.0.1:${api}`;
    await waitFor(`${base}/api/health`, () => true, 60);
    await waitFor(`http://127.0.0.1:${publicApi}/api/health`, () => true, 60);
    await waitFor(`http://127.0.0.1:${shareApi}/api/health`, () => true, 60);
    await waitFor(`http://127.0.0.1:${workspace}/health`, () => true, 60);
    await waitFor(`http://127.0.0.1:${render}/health`, () => true, 60);
    await waitFor(`http://127.0.0.1:${agent}/health`, () => true, 60);
    ownerToken = tokenOf(join(home, "server"));
    shareToken = tokenOf(join(home, "share"));
    execFileSync(
      process.execPath,
      [join(root, "apps/charet-cli/src/main.ts"), "import", "--key", "demo"],
      {
        cwd: join(root, "templates/recommended"),
        env: {
          ...process.env,
          CHARET_URL: base,
          CHARET_TOKEN: ownerToken,
          CHARET_CONFIG_DIR: join(home, "cli"),
        },
        stdio: ["ignore", log, log],
      },
    );
    // The public server's admin token, as `charet-server admin-token` prints it.
    adminToken = execFileSync(
      join(root, "target/debug/charet-server"),
      ["admin-token", "--public"],
      {
        env: { ...process.env, ...publicEnv },
      },
    )
      .toString()
      .trim();
    // The same template on the public server, for the realtime tests (two people, one charet).
    const publicBase = `http://127.0.0.1:${publicApi}`;
    execFileSync(
      process.execPath,
      [join(root, "apps/charet-cli/src/main.ts"), "import", "--key", "demo"],
      {
        cwd: join(root, "templates/recommended"),
        env: {
          ...process.env,
          CHARET_URL: publicBase,
          CHARET_TOKEN: adminToken,
          CHARET_CONFIG_DIR: join(home, "cli"),
        },
        stdio: ["ignore", log, log],
      },
    );
    await waitFor(
      `${base}/api/projects/demo/preview`,
      (body) => {
        const state = (body as { state?: string }).state;
        if (state === "failed") throw new Error(`preview failed: ${JSON.stringify(body)}`);
        return state === "ready";
      },
      180,
      ownerToken,
    );
  } catch (error) {
    for (const child of children) child.kill();
    throw new Error(`stack did not start (logs: ${join(home, "stack.log")}): ${error}`);
  }
  process.env.CHARET_E2E_URL = `http://localhost:${api}`;
  process.env.CHARET_E2E_PUBLIC_URL = `http://localhost:${publicApi}`;
  process.env.CHARET_E2E_ADMIN_TOKEN = adminToken;
  process.env.CHARET_E2E_OWNER_TOKEN = ownerToken;
  process.env.CHARET_E2E_SHARE_TOKEN = shareToken;
  process.env.CHARET_E2E_SHARE_URL = `http://localhost:${shareApi}`;
  process.env.CHARET_E2E_SHARE_REMOTE = `http://127.0.0.1:${shareRemote}`;
  return teardown;
}
