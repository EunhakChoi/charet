// Settings from environment variables.

import { readFileSync } from "node:fs";
import { isIP } from "node:net";

/** Node's timers hold at most 2^31-1 ms (about 24.8 days); a longer delay fires at once. */
const MAX_TIMER_MS = 2_147_483_647;

export interface Address {
  host: string;
  port: number;
}

export interface Config {
  /** The API charet-server calls (`CHARET_AGENT_BIND`, default 127.0.0.1:5290). */
  bind: Address;
  /** Bearer token charet-server sends (`CHARET_AGENT_TOKEN` or `_FILE`). Required off loopback. */
  token: string | undefined;
  /** `docker` (default): one container per run. `none`: a plain child process, for tests only. */
  sandbox: "docker" | "none";
  /** The server's agent door (`CHARET_AGENT_DOOR_URL`, default http://127.0.0.1:5285). */
  doorUrl: string;
  /** Where run containers reach the agent door through this service (`CHARET_AGENT_DOOR_BIND`). */
  doorBind: Address;
  /** The HTTPS proxy run containers use for the model APIs (`CHARET_AGENT_PROXY_BIND`). */
  proxyBind: Address;
  /** Hosts the proxy lets runs reach on port 443 (`CHARET_AGENT_ALLOW_HOSTS`, comma separated). */
  allowHosts: string[];
  docker: {
    socket: string;
    image: string;
    /** An internal network (no way out) this service is also on. */
    network: string;
    /** This service's name on that network. */
    host: string;
    memoryBytes: number;
    cpus: number;
  };
  /** The runner a `none` sandbox starts (`CHARET_AGENT_RUNNER`). */
  runner: string;
  /** Longest a run may take (`CHARET_AGENT_TIMEOUT_MIN`, default 30). */
  timeoutMs: number;
  /** Price per million tokens by model (`CHARET_AGENT_PRICES`, JSON), for agents that report none. */
  prices: Prices;
}

export type Prices = Record<string, { input: number; output: number; cachedInput?: number }>;

/** The model APIs; `chatgpt.com` is where Codex with a ChatGPT login goes (`/backend-api`). */
const DEFAULT_ALLOW = ["api.anthropic.com", "api.openai.com", "chatgpt.com"];

export function readConfig(env: NodeJS.ProcessEnv): Config {
  const bind = address(env, "CHARET_AGENT_BIND", "127.0.0.1:5290");
  const token = secret(env, "CHARET_AGENT_TOKEN");
  if (!token && !loopback(bind.host)) {
    throw new Error(
      `${bind.host}:${bind.port} is reachable from other machines; set CHARET_AGENT_TOKEN`,
    );
  }
  const sandbox = env.CHARET_AGENT_SANDBOX ?? "docker";
  if (sandbox !== "docker" && sandbox !== "none") {
    throw new Error(`CHARET_AGENT_SANDBOX "${sandbox}" is docker or none`);
  }
  const doorUrl = (env.CHARET_AGENT_DOOR_URL ?? "http://127.0.0.1:5285").replace(/\/+$/, "");
  if (!/^https?:\/\//.test(doorUrl)) throw new Error("CHARET_AGENT_DOOR_URL must be an http URL");
  const timeoutMin = Number(env.CHARET_AGENT_TIMEOUT_MIN ?? "30");
  if (!(timeoutMin > 0) || timeoutMin * 60_000 > MAX_TIMER_MS) {
    throw new Error(
      `CHARET_AGENT_TIMEOUT_MIN must be a positive number of minutes, at most ${Math.floor(MAX_TIMER_MS / 60_000)}`,
    );
  }
  const memoryGb = Number(env.CHARET_AGENT_MEMORY_GB ?? "4");
  const cpus = Number(env.CHARET_AGENT_CPUS ?? "2");
  if (!(memoryGb > 0) || !(cpus > 0)) {
    throw new Error("CHARET_AGENT_MEMORY_GB and CHARET_AGENT_CPUS must be positive numbers");
  }
  return {
    bind,
    token,
    sandbox,
    doorUrl,
    doorBind: address(env, "CHARET_AGENT_DOOR_BIND", "127.0.0.1:5291"),
    proxyBind: address(env, "CHARET_AGENT_PROXY_BIND", "127.0.0.1:5292"),
    allowHosts: (env.CHARET_AGENT_ALLOW_HOSTS ?? DEFAULT_ALLOW.join(","))
      .split(",")
      .map((h) => h.trim().toLowerCase())
      .filter(Boolean),
    docker: {
      socket: env.CHARET_AGENT_DOCKER_SOCKET ?? "/var/run/docker.sock",
      image: env.CHARET_AGENT_IMAGE ?? "charet-agent-run:local",
      network: env.CHARET_AGENT_NETWORK ?? "charet-agent-runs",
      host: env.CHARET_AGENT_RUN_HOST ?? "charet-agent",
      memoryBytes: Math.round(memoryGb * 1024 ** 3),
      cpus,
    },
    runner: env.CHARET_AGENT_RUNNER ?? new URL("./run/main.ts", import.meta.url).pathname,
    timeoutMs: timeoutMin * 60_000,
    prices: prices(env.CHARET_AGENT_PRICES),
  };
}

function prices(text: string | undefined): Prices {
  if (!text) return {};
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    throw new Error("CHARET_AGENT_PRICES is not JSON");
  }
  const ok =
    typeof parsed === "object" &&
    parsed !== null &&
    Object.values(parsed).every(
      (p: unknown) =>
        typeof p === "object" &&
        p !== null &&
        typeof (p as Record<string, unknown>).input === "number" &&
        typeof (p as Record<string, unknown>).output === "number",
    );
  if (!ok) {
    throw new Error('CHARET_AGENT_PRICES is {"<model>": {"input": <$/M>, "output": <$/M>}}');
  }
  return parsed as Prices;
}

function address(env: NodeJS.ProcessEnv, name: string, fallback: string): Address {
  const text = env[name] ?? fallback;
  const match = /^(.+):(\d+)$/.exec(text);
  const host = match?.[1]?.replace(/^\[(.*)\]$/, "$1");
  const port = Number(match?.[2]);
  if (!host || !isIP(host) || !Number.isInteger(port) || port > 65535) {
    throw new Error(`${name} "${text}" is not an address like ${fallback}`);
  }
  return { host, port };
}

/** A secret from `NAME`, or from the file `NAME_FILE` names (docker secrets, shared volumes). */
function secret(env: NodeJS.ProcessEnv, name: string): string | undefined {
  const value = env[name];
  if (value) return value;
  const file = env[`${name}_FILE`];
  if (!file) return undefined;
  return readFileSync(file, "utf8").trim() || undefined;
}

function loopback(host: string): boolean {
  return host === "::1" || host.startsWith("127.");
}
