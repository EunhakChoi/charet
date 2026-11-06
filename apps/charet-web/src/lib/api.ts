import { createClient, type Schemas } from "@charet/api-client";

/** Same-origin client: charet-server serves the web tool next to the API. */
export const api = createClient();

export type Manifest = Schemas["Manifest"];
export type Capture = Schemas["Capture"];
export type ComponentGroup = Schemas["ComponentGroup"];
export type ComponentUse = Schemas["ComponentUse"];
export type Comment = Schemas["Comment"];
export type Anchor = Schemas["Anchor"];
export type Element = Schemas["Element"];
export type Preview = Schemas["Preview"];
export type Version = Schemas["Version"];
export type Branch = Schemas["Branch"];
export type BranchDetail = Schemas["BranchDetail"];
export type Project = Schemas["Project"];
export type User = Schemas["User"];
export type Member = Schemas["Member"];
export type Role = Schemas["Role"];
export type VisitorAccess = Schemas["VisitorAccess"];
export type Notification = Schemas["Notification"];
export type Webhook = Schemas["Webhook"];
export type ShareMode = Schemas["ShareMode"];
export type Sharing = Schemas["Sharing"];
export type AgentRun = Schemas["AgentRun"];
export type AgentMessage = Schemas["AgentMessage"];
export type AgentKind = Schemas["AgentKind"];
export type AgentStatus = Schemas["AgentStatus"];
export type Provider = Schemas["Provider"];
export type ProviderId = Schemas["ProviderId"];
export type KeyCheck = Schemas["KeyCheck"];
export type OAuthStatus = Schemas["OAuthStatus"];
export type Theme = "light" | "dark";
export type TargetKind = "screen" | "story";

/** An API error with its code (`pending`, `changed`, …). */
export class HttpError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
    readonly detail?: unknown,
  ) {
    super(message);
  }
}

/** The data of an openapi-fetch result, or its error thrown as an [`HttpError`]. */
export async function unwrap<T>(
  call: Promise<{ data?: T; error?: unknown; response: Response }>,
): Promise<T> {
  const { data, error, response } = await call;
  if (error !== undefined || data === undefined) {
    const body = (error ?? {}) as { code?: string; message?: string; detail?: unknown };
    throw new HttpError(
      response.status,
      body.code ?? "error",
      body.message ?? response.statusText,
      body.detail,
    );
  }
  return data;
}

/** Like [`unwrap`] for answers without a body (`204 No Content`). */
export async function unwrapEmpty(
  call: Promise<{ error?: unknown; response: Response }>,
): Promise<void> {
  const { error, response } = await call;
  if (!response.ok) {
    const body = (error ?? {}) as { code?: string; message?: string; detail?: unknown };
    throw new HttpError(
      response.status,
      body.code ?? "error",
      body.message ?? response.statusText,
      body.detail,
    );
  }
}

export function captureImage(
  key: string,
  capture: Pick<Capture, "kind" | "target" | "theme" | "capturedAt">,
) {
  const path = `/api/projects/${encodeURIComponent(key)}/captures/${capture.kind}/${encodeURIComponent(capture.target)}/${capture.theme}/image`;
  return `${path}?v=${encodeURIComponent(capture.capturedAt)}`;
}

export function snapshotImage(key: string, comment: Pick<Comment, "id">) {
  return `/api/projects/${encodeURIComponent(key)}/comments/${comment.id}/snapshot`;
}

export function agentAttachment(key: string, run: number, seq: number) {
  return `/api/projects/${encodeURIComponent(key)}/agent-runs/${run}/attachments/${seq}`;
}
