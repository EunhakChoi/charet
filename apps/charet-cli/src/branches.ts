// Branches from the terminal: `charet branches`, `charet branch [n]` and its actions (review,
// approve, merge, archive, reopen, duplicate, catch-up), and `charet restore vN`. The branch defaults to the
// one the checkout pushes to.

import type { Schemas } from "@charet/api-client";
import { t } from "@charet/i18n";
import { findCheckout } from "./checkout.ts";
import {
  branchUrl,
  CliError,
  type Context,
  client,
  emit,
  fail,
  mergeBranch,
  returnToMain,
  say,
} from "./commands.ts";

type Branch = Schemas["Branch"];

const ACTIONS = [
  "review",
  "approve",
  "merge",
  "archive",
  "reopen",
  "duplicate",
  "catch-up",
] as const;
type Action = (typeof ACTIONS)[number];

/** Lists active branches (all with `all`), newest first. */
export async function list(ctx: Context, project: string, all: boolean): Promise<void> {
  const { data, error } = await client(ctx).GET("/api/projects/{key}/branches", {
    params: { path: { key: project } },
  });
  if (!data) fail(error, "request failed");
  const shown = data.branches.filter((b) => all || b.status === "open" || b.status === "in_review");
  emit(ctx, { branches: shown }, () => {
    if (shown.length === 0) say(ctx, "cli.branches.none");
    for (const b of shown) ctx.out(line(b));
  });
}

function line(b: Branch): string {
  const clash = b.conflicts.length > 0 ? `\tclash: ${b.conflicts.join(", ")}` : "";
  return `#${b.number}\t${b.status}\t${b.author.name}\t${b.title}${clash}`;
}

/** `charet branch [n]` shows one; `charet branch <action> [n]` acts on it. */
export async function branchCommand(
  ctx: Context,
  project: string,
  rest: string[],
): Promise<number> {
  const [first, second] = rest;
  if (first && (ACTIONS as readonly string[]).includes(first)) {
    await act(ctx, project, first as Action, numberOf(ctx, second));
    return 0;
  }
  if (first && !/^#?\d+$/.test(first)) {
    throw new CliError(t("cli.usage.branch", {}, ctx.locale));
  }
  await show(ctx, project, numberOf(ctx, first));
  return 0;
}

/** `n` or `#n`, else the checkout's branch. */
function numberOf(ctx: Context, text: string | undefined): number {
  if (text) {
    const number = Number(text.replace(/^#/, ""));
    if (Number.isInteger(number) && number > 0) return number;
    throw new CliError(t("cli.usage.branch", {}, ctx.locale));
  }
  const branch = findCheckout(ctx.cwd)?.state.branch;
  if (!branch) throw new CliError(t("cli.branch.which", {}, ctx.locale));
  return branch;
}

async function show(ctx: Context, project: string, number: number): Promise<void> {
  const { data, error } = await client(ctx).GET("/api/projects/{key}/branches/{number}", {
    params: { path: { key: project, number } },
  });
  if (!data) fail(error, "request failed");
  emit(ctx, data, () => {
    ctx.out(`#${data.number} ${data.title}`);
    ctx.out(`status   ${data.status}${data.mergedVersion ? ` (v${data.mergedVersion})` : ""}`);
    ctx.out(`author   ${data.author.name}`);
    ctx.out(`head     ${data.head.slice(0, 10)}${data.behind ? "  (main moved on)" : ""}`);
    if (data.conflicts.length > 0) ctx.out(`clash    ${data.conflicts.join(", ")}`);
    if (data.unapproved.length > 0) ctx.out(`approval ${data.unapproved.join(", ")}`);
    if (data.check && data.check.head === data.head) {
      ctx.out(`check    ${data.check.state}`);
      for (const issue of data.check.issues) ctx.out(`  ${issue}`);
    }
    ctx.out(`url      ${branchUrl(ctx.server, project, data.number)}`);
    ctx.out(`changes  ${data.changes.length}`);
    const mark = { added: "A", modified: "M", deleted: "D" } as const;
    for (const c of data.changes) ctx.out(`  ${mark[c.kind]} ${c.path}`);
    for (const e of data.events) {
      ctx.out(`  ${e.at.slice(0, 16).replace("T", " ")}  ${e.actor.name}  ${e.kind} ${e.detail}`);
    }
  });
}

async function act(ctx: Context, project: string, action: Action, number: number): Promise<void> {
  if (action === "merge") {
    const version = await mergeBranch(ctx, ctx.server, project, number);
    emit(ctx, version, () =>
      say(ctx, "cli.merge.done", { number: String(number), version: String(version.number) }),
    );
    await returnToMain(ctx, number, version.number);
    return;
  }
  const path = {
    review: "/api/projects/{key}/branches/{number}/request-review",
    approve: "/api/projects/{key}/branches/{number}/approve",
    archive: "/api/projects/{key}/branches/{number}/archive",
    reopen: "/api/projects/{key}/branches/{number}/reopen",
    duplicate: "/api/projects/{key}/branches/{number}/duplicate",
    "catch-up": "/api/projects/{key}/branches/{number}/catch-up",
  } as const;
  // Every one of these answers with the branch; one path type keeps the client's typing simple.
  const route = path[action] as "/api/projects/{key}/branches/{number}/archive";
  const { data, error } = await client(ctx).POST(route, {
    params: { path: { key: project, number } },
  });
  if (!data) fail(error, `${action} failed`);
  emit(ctx, data, () => {
    if (action === "catch-up" && data.conflicts.length > 0) {
      say(ctx, "cli.branch.clash", { number: String(number), files: data.conflicts.join(", ") });
      return;
    }
    const key = action === "catch-up" ? "caughtUp" : action;
    say(ctx, `cli.branch.${key}`, {
      number: String(data.number),
      from: String(number),
      url: branchUrl(ctx.server, project, data.number),
    });
  });
}

/** `charet restore vN`: a new version that undoes vN. */
export async function restore(
  ctx: Context,
  project: string,
  version: string | undefined,
): Promise<void> {
  const number = Number(version?.replace(/^v/, ""));
  if (!Number.isInteger(number) || number < 1) {
    throw new CliError(t("cli.usage.restore", {}, ctx.locale));
  }
  const { data, error } = await client(ctx).POST("/api/projects/{key}/versions/{number}/restore", {
    params: { path: { key: project, number } },
  });
  if (!data) {
    if (error?.code === "clash") {
      const files = ((error.detail as { files?: string[] } | undefined)?.files ?? []).join(", ");
      throw new CliError(t("cli.restore.clash", { version: String(number), files }, ctx.locale));
    }
    fail(error, "restore failed");
  }
  emit(ctx, data, () =>
    say(ctx, "cli.restore.done", { version: String(number), created: String(data.number) }),
  );
}
