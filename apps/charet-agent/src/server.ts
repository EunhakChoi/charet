// Internal HTTP API. Only charet-server calls it (bearer token).
//
//   GET  /health
//   POST /v1/runs                   start a run (RunStart JSON)
//   GET  /v1/runs/{id}?after=N      what it did after its first N events, and how it ended
//   POST /v1/runs/{id}/cancel       stop it; it saves what it made first

import { timingSafeEqual } from "node:crypto";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { isRunStart } from "./protocol.ts";
import type { Runs } from "./runs.ts";

export interface ServerOptions {
  runs: Runs;
  token: string | undefined;
  version: string;
  /** Why runs cannot start (docker down, image missing), checked on `/health`. */
  ready: () => Promise<string | undefined>;
}

/** Four images of 5 MB in base64, plus the prompt. */
const MAX_BODY = 32 * 1024 * 1024;

export function createAgentServer(options: ServerOptions): Server {
  return createServer((req, res) => {
    handle(options, req, res).catch((error: unknown) => {
      if (!res.headersSent) send(res, 500, { code: "internal", message: String(error) });
      else res.destroy();
    });
  });
}

async function handle(
  options: ServerOptions,
  req: IncomingMessage,
  res: ServerResponse,
): Promise<void> {
  const url = new URL(req.url ?? "/", "http://agent");
  if (url.pathname === "/health" && req.method === "GET") {
    const problem = await options.ready();
    return problem
      ? send(res, 503, { status: "unavailable", message: problem })
      : send(res, 200, { status: "ok", version: options.version, running: options.runs.active() });
  }
  if (!authorized(options.token, req.headers.authorization)) {
    return send(res, 401, { code: "unauthorized", message: "missing or wrong token" });
  }
  const parts = url.pathname.split("/").filter(Boolean);
  if (parts[0] !== "v1" || parts[1] !== "runs") {
    return send(res, 404, { code: "not_found", message: "no such route" });
  }
  if (parts.length === 2 && req.method === "POST") {
    let body: unknown;
    try {
      body = JSON.parse((await read(req, MAX_BODY)).toString());
    } catch (error) {
      // A JSON syntax error quotes the body, and the body holds the run's credentials.
      const message = error instanceof SyntaxError ? "not JSON" : (error as Error).message;
      return send(res, 400, { code: "invalid", message });
    }
    if (!isRunStart(body)) return send(res, 400, { code: "invalid", message: "not a run" });
    await options.runs.start(body);
    return send(res, 202, { id: body.id });
  }
  const id = Number(parts[2]);
  if (!Number.isSafeInteger(id))
    return send(res, 404, { code: "not_found", message: "no such run" });
  if (parts.length === 3 && req.method === "GET") {
    const after = Number(url.searchParams.get("after") ?? "0");
    const progress = options.runs.progress(
      id,
      Number.isSafeInteger(after) && after > 0 ? after : 0,
    );
    return progress
      ? send(res, 200, progress)
      : send(res, 404, { code: "not_found", message: "no such run" });
  }
  if (parts.length === 4 && parts[3] === "cancel" && req.method === "POST") {
    return options.runs.cancel(id)
      ? send(res, 200, { id })
      : send(res, 404, { code: "not_found", message: "no such run" });
  }
  return send(res, 404, { code: "not_found", message: "no such route" });
}

function read(req: IncomingMessage, max: number): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    let size = 0;
    req.on("data", (chunk: Buffer) => {
      size += chunk.length;
      if (size > max) {
        reject(new Error(`body is over ${max} bytes`));
        req.destroy();
        return;
      }
      chunks.push(chunk);
    });
    req.on("end", () => resolve(Buffer.concat(chunks)));
    req.on("error", reject);
  });
}

function authorized(token: string | undefined, header: string | undefined): boolean {
  if (token === undefined) return true;
  const given = Buffer.from(header?.startsWith("Bearer ") ? header.slice(7) : "");
  const wanted = Buffer.from(token);
  return given.length === wanted.length && timingSafeEqual(given, wanted);
}

function send(res: ServerResponse, status: number, body: unknown): void {
  const text = JSON.stringify(body);
  res.writeHead(status, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(text),
  });
  res.end(text);
}
