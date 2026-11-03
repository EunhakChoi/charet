// Starting a run: in its own container (docker), or as a plain child process (tests only).

import { type ChildProcess, spawn } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { request } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Config } from "./config.ts";
import type { RunSpec, RunStart } from "./protocol.ts";
import { tar } from "./tar.ts";

/** A started run: its stdout lines go to `onLine`; `exited` resolves with its exit code. */
export interface Launched {
  exited: Promise<number>;
  /** Asks it to stop (SIGTERM; it saves what it has), then kills it after a grace period. */
  stop(): void;
  /** The end of its stderr, for errors. */
  stderr(): string;
}

export interface Launcher {
  launch(run: RunStart, onLine: (line: string) => void): Promise<Launched>;
  /** Removes runs left over from before a restart. */
  cleanup(): Promise<void>;
}

const GRACE_SECS = 20;

/** The spec file and images a run reads, with images as separate files. The spec is where a run
 * gets its credentials (`apiKey` or `oauth`): they are in no environment variable or argument. */
export function inputFiles(run: RunStart): { path: string; data: Buffer }[] {
  const images = run.images.map((image, i) => ({
    mediaType: image.mediaType,
    file: `image-${i}.${extension(image.mediaType)}`,
    data: Buffer.from(image.data, "base64"),
  }));
  const spec: RunSpec = {
    id: run.id,
    agent: run.agent,
    model: run.model,
    apiKey: run.apiKey,
    ...(run.oauth ? { oauth: run.oauth } : {}),
    project: run.project,
    branch: run.branch,
    prompt: run.prompt,
    context: run.context,
    history: run.history,
    images: images.map(({ mediaType, file }) => ({ mediaType, file })),
  };
  return [
    { path: "spec.json", data: Buffer.from(JSON.stringify(spec)) },
    ...images.map((i) => ({ path: i.file, data: i.data })),
  ];
}

function extension(mediaType: string): string {
  return (
    { "image/png": "png", "image/jpeg": "jpg", "image/gif": "gif", "image/webp": "webp" }[
      mediaType
    ] ?? "bin"
  );
}

/** Splits a byte stream into lines. */
function lines(onLine: (line: string) => void): (chunk: Buffer | string) => void {
  let rest = "";
  return (chunk) => {
    rest += chunk.toString();
    let index = rest.indexOf("\n");
    while (index >= 0) {
      const line = rest.slice(0, index).trim();
      rest = rest.slice(index + 1);
      if (line) onLine(line);
      index = rest.indexOf("\n");
    }
  };
}

function tail(): { add(text: string): void; text(): string } {
  let kept = "";
  return {
    add(text) {
      kept = (kept + text).slice(-4000);
    },
    text: () => kept.trim(),
  };
}

// ---- none: a child process (no isolation; tests and development only) ----------------------------

export class ProcessLauncher implements Launcher {
  private readonly config: Config;

  constructor(config: Config) {
    this.config = config;
  }

  async launch(run: RunStart, onLine: (line: string) => void): Promise<Launched> {
    const dir = mkdtempSync(join(tmpdir(), `charet-run-${run.id}-`));
    chmodSync(dir, 0o700);
    const input = join(dir, "input");
    mkdirSync(input);
    for (const file of inputFiles(run)) writeFileSync(join(input, file.path), file.data);
    const env: NodeJS.ProcessEnv = {
      PATH: process.env.PATH,
      HOME: join(dir, "home"),
      CHARET_URL: this.config.doorUrl,
      CHARET_TOKEN: run.token,
      CHARET_CONFIG_DIR: join(dir, "config"),
      CHARET_RUN_SPEC: join(input, "spec.json"),
      CHARET_RUN_WORK: join(dir, "work"),
      ...(process.env.CHARET_CLI ? { CHARET_CLI: process.env.CHARET_CLI } : {}),
    };
    const child: ChildProcess = spawn(process.execPath, [this.config.runner], {
      env,
      stdio: ["ignore", "pipe", "pipe"],
    });
    const errors = tail();
    child.stdout?.on("data", lines(onLine));
    child.stderr?.on("data", (chunk: Buffer) => errors.add(chunk.toString()));
    const exited = new Promise<number>((resolve) => {
      child.once("exit", (code, signal) => {
        rmSync(dir, { recursive: true, force: true });
        resolve(code ?? (signal ? 143 : 1));
      });
    });
    return {
      exited,
      stop() {
        child.kill("SIGTERM");
        setTimeout(() => child.kill("SIGKILL"), GRACE_SECS * 1000).unref();
      },
      stderr: errors.text,
    };
  }

  async cleanup(): Promise<void> {}
}

// ---- docker: one container per run -------------------------------------------------------------

interface DockerResponse {
  status: number;
  body: Buffer;
}

export class DockerLauncher implements Launcher {
  private readonly config: Config;

  constructor(config: Config) {
    this.config = config;
  }

  private call(
    method: string,
    path: string,
    body?: unknown,
    contentType = "application/json",
  ): Promise<DockerResponse> {
    const payload =
      body === undefined
        ? undefined
        : Buffer.isBuffer(body)
          ? body
          : Buffer.from(JSON.stringify(body));
    return new Promise((resolve, reject) => {
      const req = request(
        {
          socketPath: this.config.docker.socket,
          method,
          path: `/v1.43${path}`,
          headers: payload ? { "content-type": contentType, "content-length": payload.length } : {},
        },
        (res) => {
          const chunks: Buffer[] = [];
          res.on("data", (c: Buffer) => chunks.push(c));
          res.on("end", () =>
            resolve({ status: res.statusCode ?? 0, body: Buffer.concat(chunks) }),
          );
          res.on("error", reject);
        },
      );
      req.on("error", reject);
      req.end(payload);
    });
  }

  private async must(method: string, path: string, body?: unknown, type?: string): Promise<Buffer> {
    const res = await this.call(method, path, body, type);
    if (res.status >= 300) {
      throw new Error(
        `docker ${method} ${path.split("?")[0]}: ${res.status} ${res.body.toString().slice(0, 300)}`,
      );
    }
    return res.body;
  }

  async launch(run: RunStart, onLine: (line: string) => void): Promise<Launched> {
    const { docker, doorBind, proxyBind } = this.config;
    const proxy = `http://${docker.host}:${proxyBind.port}`;
    const created = await this.must(
      "POST",
      `/containers/create?name=charet-run-${run.id}-${Date.now()}`,
      {
        Image: docker.image,
        Labels: { "charet.run": String(run.id) },
        Env: [
          `CHARET_URL=http://${docker.host}:${doorBind.port}`,
          `CHARET_TOKEN=${run.token}`,
          "CHARET_RUN_SPEC=/input/spec.json",
          "CHARET_RUN_WORK=/work/project",
          `HTTPS_PROXY=${proxy}`,
          `https_proxy=${proxy}`,
          `NO_PROXY=${docker.host}`,
          `no_proxy=${docker.host}`,
        ],
        HostConfig: {
          NetworkMode: docker.network,
          CapDrop: ["ALL"],
          SecurityOpt: ["no-new-privileges"],
          Memory: docker.memoryBytes,
          NanoCpus: Math.round(docker.cpus * 1e9),
          PidsLimit: 1024,
          Init: true,
        },
      },
    );
    const id = (JSON.parse(created.toString()) as { Id: string }).Id;
    try {
      const files = inputFiles(run).map((f) => ({ ...f, path: `input/${f.path}` }));
      await this.must(
        "PUT",
        `/containers/${id}/archive?path=/`,
        tar(files, { uid: 1000, gid: 1000 }),
        "application/x-tar",
      );
      await this.must("POST", `/containers/${id}/start`);
    } catch (error) {
      await this.call("DELETE", `/containers/${id}?force=1`).catch(() => {});
      throw error;
    }
    const errors = tail();
    const logs = this.follow(id, lines(onLine), errors.add);
    const exited = (async () => {
      const waited = await this.must("POST", `/containers/${id}/wait`).catch(() => undefined);
      await logs.catch(() => {});
      await this.call("DELETE", `/containers/${id}?force=1`).catch(() => {});
      return waited ? (JSON.parse(waited.toString()) as { StatusCode: number }).StatusCode : 1;
    })();
    return {
      exited,
      stop: () => {
        this.call("POST", `/containers/${id}/stop?t=${GRACE_SECS}`).catch(() => {});
      },
      stderr: errors.text,
    };
  }

  /** Streams a container's output (Docker multiplexes stdout and stderr in 8-byte framed chunks). */
  private follow(
    id: string,
    stdout: (chunk: Buffer) => void,
    stderr: (text: string) => void,
  ): Promise<void> {
    return new Promise((resolve, reject) => {
      const req = request(
        {
          socketPath: this.config.docker.socket,
          method: "GET",
          path: `/v1.43/containers/${id}/logs?follow=1&stdout=1&stderr=1`,
        },
        (res) => {
          let pending = Buffer.alloc(0);
          res.on("data", (chunk: Buffer) => {
            pending = Buffer.concat([pending, chunk]);
            while (pending.length >= 8) {
              const size = pending.readUInt32BE(4);
              if (pending.length < 8 + size) break;
              const frame = pending.subarray(8, 8 + size);
              if (pending[0] === 2) stderr(frame.toString());
              else stdout(frame);
              pending = pending.subarray(8 + size);
            }
          });
          res.on("end", resolve);
          res.on("error", reject);
        },
      );
      req.on("error", reject);
      req.end();
    });
  }

  async cleanup(): Promise<void> {
    const filters = encodeURIComponent(JSON.stringify({ label: ["charet.run"] }));
    const listed = await this.must("GET", `/containers/json?all=1&filters=${filters}`);
    for (const container of JSON.parse(listed.toString()) as { Id: string }[]) {
      await this.call("DELETE", `/containers/${container.Id}?force=1`).catch(() => {});
    }
  }

  /** Whether the Docker daemon answers and the run image exists. */
  async ready(): Promise<string | undefined> {
    try {
      const res = await this.call(
        "GET",
        `/images/${encodeURIComponent(this.config.docker.image)}/json`,
      );
      if (res.status === 404) return `the run image ${this.config.docker.image} is not built`;
      if (res.status >= 300) return `docker: ${res.status}`;
      return undefined;
    } catch (error) {
      return `cannot reach docker at ${this.config.docker.socket}: ${(error as Error).message}`;
    }
  }
}
