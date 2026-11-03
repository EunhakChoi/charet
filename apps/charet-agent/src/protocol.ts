// What charet-server sends to start a run, what the runner inside a run reports, and what the
// server reads back. The runner talks JSON lines on its stdout.

export type AgentKind = "claude" | "codex" | "script";
export type MessageKind = "user" | "assistant" | "tool" | "status";
export type Finished = "done" | "failed" | "canceled";

/** A subscription login a run uses instead of a vendor key; its tokens are good for this run. */
export type RunOAuth =
  /** A Claude subscription (the `claude` agent). */
  | { provider: "anthropic"; accessToken: string }
  /** A ChatGPT subscription (the `codex` agent). */
  | { provider: "openai"; accessToken: string; idToken: string; accountId: string };

/** `POST /v1/runs` from charet-server. */
export interface RunStart {
  id: number;
  agent: AgentKind;
  /** Empty: the agent's default. */
  model: string;
  /** Vendor key; empty for the script agent, and when `oauth` is set. */
  apiKey: string;
  /** A subscription login instead of `apiKey`; absent or `null`: the run uses `apiKey`. */
  oauth?: RunOAuth | null;
  project: string;
  branch: number;
  /** For the agent door: good for this run's branch while it runs. */
  token: string;
  prompt: string;
  context: unknown;
  images: { mediaType: string; data: string }[];
  history: { role: "user" | "assistant"; text: string }[];
}

/** What a run container finds in its spec file (`CHARET_RUN_SPEC`). */
export interface RunSpec extends Omit<RunStart, "images" | "token" | "oauth"> {
  /** The login, when the run has one (else `apiKey` is the credential). */
  oauth?: RunOAuth;
  /** Image files next to the spec. */
  images: { mediaType: string; file: string }[];
}

export interface RunEvent {
  kind: MessageKind;
  text: string;
  tool?: string;
}

/** One line on the runner's stdout. */
export type RunnerLine =
  | ({ type: "event" } & RunEvent)
  | {
      type: "usage";
      tokensIn: number;
      tokensOut: number;
      /** Reported by the agent; `null`: work it out from the price table. */
      costUsd: number | null;
      model?: string;
      cachedIn?: number;
    }
  | { type: "result"; state: Finished; error?: string };

/** `GET /v1/runs/{id}?after=N` to charet-server. */
export interface Progress {
  events: RunEvent[];
  next: number;
  finished: Finished | null;
  error: string | null;
  tokensIn: number;
  tokensOut: number;
  costUsd: number | null;
}

export function isRunStart(value: unknown): value is RunStart {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  return (
    Number.isSafeInteger(v.id) &&
    (v.agent === "claude" || v.agent === "codex" || v.agent === "script") &&
    typeof v.model === "string" &&
    typeof v.apiKey === "string" &&
    validOAuth(v.oauth, v.agent, v.apiKey) &&
    typeof v.project === "string" &&
    /^[a-z0-9][a-z0-9-]{0,62}$/.test(v.project) &&
    Number.isSafeInteger(v.branch) &&
    typeof v.token === "string" &&
    /^[0-9a-f]{16,128}$/.test(v.token) &&
    typeof v.prompt === "string" &&
    Array.isArray(v.images) &&
    v.images.every(
      (i: unknown) =>
        typeof i === "object" &&
        i !== null &&
        typeof (i as Record<string, unknown>).mediaType === "string" &&
        typeof (i as Record<string, unknown>).data === "string",
    ) &&
    Array.isArray(v.history)
  );
}

/** Absent or `null`; else the login of the run's own vendor, and not next to a vendor key. */
function validOAuth(oauth: unknown, agent: AgentKind, apiKey: string): boolean {
  if (oauth === undefined || oauth === null) return true;
  if (typeof oauth !== "object" || apiKey !== "") return false;
  const o = oauth as Record<string, unknown>;
  const filled = (...names: string[]) =>
    names.every((name) => typeof o[name] === "string" && o[name] !== "");
  if (agent === "claude") return o.provider === "anthropic" && filled("accessToken");
  if (agent === "codex") {
    return o.provider === "openai" && filled("accessToken", "idToken", "accountId");
  }
  return false;
}
