import { Bot, ImagePlus, LoaderCircle, Send, Square, Wrench, X } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { Link } from "react-router";
import { Markdown } from "@/components/Markdown";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { type AgentKind, type AgentRun, agentAttachment, HttpError } from "@/lib/api";
import { useT } from "@/lib/i18n";
import {
  type NewAgentRun,
  useAgent,
  useAgentActions,
  useAgentMessages,
  useAgentRuns,
  useBranches,
  useMe,
} from "@/lib/queries";
import { useCan } from "@/lib/role";
import { cn } from "@/lib/utils";

type ImageType = NonNullable<NewAgentRun["images"]>[number]["mediaType"];
type Picked = { mediaType: ImageType; data: string; url: string; name: string };

const TYPES: ImageType[] = ["image/png", "image/jpeg", "image/gif", "image/webp"];
const MAX_IMAGES = 4;
const MAX_BYTES = 5 * 1024 * 1024;

/**
 * The draft the person last worked on in this tab, kept per project: the panel starts again from
 * nothing when they leave the page and come back, and without this it would forget which
 * conversation it was showing. A tab of its own starts clean, so a request never lands on an old
 * draft by surprise.
 */
const draftMemory = {
  key: (project: string) => `charet.agent.draft.${project}`,
  read(project: string): number | undefined {
    try {
      const number = Number(sessionStorage.getItem(draftMemory.key(project)));
      return Number.isInteger(number) && number > 0 ? number : undefined;
    } catch {
      return undefined;
    }
  },
  write(project: string, target: number | "new"): void {
    try {
      if (target === "new") sessionStorage.removeItem(draftMemory.key(project));
      else sessionStorage.setItem(draftMemory.key(project), String(target));
    } catch {}
  },
};

/**
 * Asks the web agent for a change and follows it. Without `branch` the person picks a new draft or
 * one of their drafts; with it, the conversation stays on that branch.
 */
export function AgentPanel({
  projectKey,
  branch,
  context,
  contextLabel,
  onClearContext,
  readOnly = false,
}: {
  projectKey: string;
  branch?: number;
  /** Only the conversation (a closed branch). */
  readOnly?: boolean;
  /** What the person pointed at, sent with the request. */
  context?: Record<string, unknown> | undefined;
  contextLabel?: string | undefined;
  onClearContext?: (() => void) | undefined;
}) {
  const t = useT();
  const canEdit = useCan("editor");
  const agent = useAgent(canEdit);
  const me = useMe();
  const branches = useBranches(projectKey);
  const [target, setTarget] = useState<number | "new">(
    () => branch ?? draftMemory.read(projectKey) ?? "new",
  );
  useEffect(() => {
    if (branch) setTarget(branch);
  }, [branch]);
  // On a branch's own page the branch is given; elsewhere the person's choice is what is kept.
  useEffect(() => {
    if (!branch) draftMemory.write(projectKey, target);
  }, [branch, projectKey, target]);
  // A remembered branch that was merged or archived since cannot take a request: the server
  // refuses it. Once the list says so, go back to a new draft. (A branch the list does not hold
  // yet, such as one a request just made, is left alone.)
  useEffect(() => {
    if (branch || target === "new" || !me.data) return;
    const known = branches.data?.branches.find((b) => b.number === target);
    const writable =
      known?.status === "in_review" || (known?.status === "open" && known.author.id === me.data.id);
    if (known && !writable) setTarget("new");
  }, [branch, target, branches.data, me.data]);
  const drafts = useMemo(
    () =>
      (branches.data?.branches ?? []).filter(
        (b) => b.status === "open" && b.author.id === me.data?.id,
      ),
    [branches.data, me.data],
  );
  const on = target === "new" ? undefined : target;
  const runs = useAgentRuns(projectKey, on, on !== undefined);

  if (!canEdit) {
    return <p className="p-4 text-sm text-muted-foreground">{t("web.agent.editorsOnly")}</p>;
  }
  if (agent.error) {
    return <p className="p-4 text-sm text-destructive">{agent.error.message}</p>;
  }
  if (!agent.data) {
    return <p className="p-4 text-sm text-muted-foreground">{t("web.common.loading")}</p>;
  }
  if (!agent.data.enabled) {
    return (
      <div className="flex flex-col gap-1 p-4 text-sm">
        <p className="font-medium">{t("web.agent.off")}</p>
        <p className="text-muted-foreground">{agent.data.reason ?? t("web.agent.noKey")}</p>
      </div>
    );
  }
  const list = runs.data ?? [];
  const busy = list.some((r) => r.state === "queued" || r.state === "running");
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {!branch && (
        <div className="flex flex-col gap-1 border-b p-3">
          <label htmlFor="agent-branch" className="text-xs text-muted-foreground">
            {t("web.agent.where")}
          </label>
          <select
            id="agent-branch"
            className="h-8 rounded-md border bg-background px-2 text-sm"
            value={String(target)}
            onChange={(event) =>
              setTarget(event.target.value === "new" ? "new" : Number(event.target.value))
            }
          >
            <option value="new">{t("web.agent.newDraft")}</option>
            {drafts.map((d) => (
              <option key={d.number} value={d.number}>
                #{d.number} {d.title}
              </option>
            ))}
            {/* A remembered branch that is not in the list (yet), or that is in review. */}
            {on !== undefined && !drafts.some((d) => d.number === on) && (
              <option value={on}>#{on}</option>
            )}
          </select>
        </div>
      )}
      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto p-3">
        {on === undefined && <p className="text-sm text-muted-foreground">{t("web.agent.lead")}</p>}
        {list.map((run) => (
          <RunView key={run.id} projectKey={projectKey} run={run} showBranch={!branch} />
        ))}
      </div>
      {!readOnly && (
        <Composer
          projectKey={projectKey}
          branch={on}
          agents={agent.data.agents.filter((a) => a.ready).map((a) => a.agent)}
          fallback={agent.data.default ?? undefined}
          busy={busy}
          context={context}
          contextLabel={contextLabel}
          onClearContext={onClearContext}
          onStarted={(run) => setTarget(run.branch)}
        />
      )}
    </div>
  );
}

function RunView({
  projectKey,
  run,
  showBranch,
}: {
  projectKey: string;
  run: AgentRun;
  showBranch: boolean;
}) {
  const t = useT();
  const me = useMe();
  const isAdmin = useCan("admin");
  const live = run.state === "queued" || run.state === "running";
  const messages = useAgentMessages(projectKey, run.id, live);
  const { cancel } = useAgentActions(projectKey);
  const bottom = useRef<HTMLDivElement>(null);
  const count = messages.data?.length ?? 0;
  // biome-ignore lint/correctness/useExhaustiveDependencies: follow new lines as they come
  useEffect(() => {
    if (live) bottom.current?.scrollIntoView({ block: "nearest" });
  }, [count, live]);
  return (
    <article
      className="flex flex-col gap-2 rounded-lg border p-3 text-sm"
      data-testid={`agent-run-${run.id}`}
    >
      <header className="flex items-center gap-2 text-xs text-muted-foreground">
        <Bot className="size-3.5" />
        <span>
          {run.agent}
          {run.model ? ` · ${run.model}` : ""}
        </span>
        <RunState run={run} />
        <span className="flex-1" />
        {run.costUsd != null && run.costUsd > 0 && <span>${run.costUsd.toFixed(4)}</span>}
        {live && (me.data?.id === run.actor.id || isAdmin) && (
          <Button
            size="sm"
            variant="ghost"
            disabled={cancel.isPending}
            onClick={() => cancel.mutate(run.id)}
          >
            <Square className="size-3" />
            {t("web.agent.stop")}
          </Button>
        )}
      </header>
      {(messages.data ?? []).map((message) => {
        if (message.kind === "user") {
          return (
            <div key={message.seq} className="flex flex-col items-end gap-1">
              <p className="max-w-xs whitespace-pre-wrap rounded-lg bg-primary px-3 py-2 text-primary-foreground">
                {message.text}
              </p>
              {run.attachments > 0 && (
                <div className="flex gap-1">
                  {Array.from({ length: run.attachments }, (_, i) => (
                    <img
                      // biome-ignore lint/suspicious/noArrayIndexKey: attachments never move
                      key={i}
                      src={agentAttachment(projectKey, run.id, i)}
                      alt=""
                      className="size-12 rounded border object-cover"
                    />
                  ))}
                </div>
              )}
            </div>
          );
        }
        if (message.kind === "assistant") {
          return (
            <Markdown
              key={message.seq}
              text={message.text}
              className="rounded-lg bg-muted px-3 py-2"
            />
          );
        }
        if (message.kind === "tool") {
          return (
            <p
              key={message.seq}
              className="flex items-start gap-1.5 truncate font-mono text-xs text-muted-foreground"
              title={message.text}
            >
              <Wrench className="mt-0.5 size-3 shrink-0" />
              <span className="shrink-0">{message.tool}</span>
              <span className="truncate">{message.text}</span>
            </p>
          );
        }
        return (
          <p key={message.seq} className="text-xs text-muted-foreground italic">
            {message.text}
          </p>
        );
      })}
      {live && (
        <p className="flex items-center gap-2 text-xs text-muted-foreground">
          <LoaderCircle className="size-3.5 animate-spin" />
          {t(run.state === "queued" ? "web.agent.queued" : "web.agent.working")}
        </p>
      )}
      {!live && showBranch && (
        <Link
          to={`/p/${projectKey}/branches/${run.branch}`}
          className="text-xs text-info hover:underline"
        >
          {t("web.agent.openBranch", { number: run.branch })}
        </Link>
      )}
      <div ref={bottom} />
    </article>
  );
}

function RunState({ run }: { run: AgentRun }) {
  const t = useT();
  switch (run.state) {
    case "done":
      return <Badge variant="secondary">{t("web.agent.state.done")}</Badge>;
    case "failed":
      return (
        <Badge variant="destructive" title={run.error ?? undefined}>
          {t("web.agent.state.failed")}
        </Badge>
      );
    case "canceled":
      return <Badge variant="outline">{t("web.agent.state.canceled")}</Badge>;
    case "queued":
      return <Badge variant="warning">{t("web.agent.state.queued")}</Badge>;
    default:
      return <Badge variant="info">{t("web.agent.state.running")}</Badge>;
  }
}

function Composer({
  projectKey,
  branch,
  agents,
  fallback,
  busy,
  context,
  contextLabel,
  onClearContext,
  onStarted,
}: {
  projectKey: string;
  branch: number | undefined;
  agents: AgentKind[];
  fallback: AgentKind | undefined;
  busy: boolean;
  context: Record<string, unknown> | undefined;
  contextLabel: string | undefined;
  onClearContext: (() => void) | undefined;
  onStarted: (run: AgentRun) => void;
}) {
  const t = useT();
  const { start } = useAgentActions(projectKey);
  const [prompt, setPrompt] = useState("");
  const [images, setImages] = useState<Picked[]>([]);
  const [problem, setProblem] = useState<string | undefined>();
  const [agent, setAgent] = useState<AgentKind | undefined>();
  const [model, setModel] = useState("");
  const file = useRef<HTMLInputElement>(null);
  const chosen = agent ?? fallback ?? agents[0];

  const add = async (files: Iterable<File>) => {
    const next = [...images];
    for (const f of files) {
      if (!TYPES.includes(f.type as ImageType)) continue;
      if (next.length >= MAX_IMAGES || f.size > MAX_BYTES) {
        setProblem(t("web.agent.imageLimit"));
        continue;
      }
      const data = await read(f);
      next.push({
        mediaType: f.type as ImageType,
        data,
        url: URL.createObjectURL(f),
        name: f.name,
      });
    }
    setImages(next);
  };

  const submit = () => {
    if (!prompt.trim()) return;
    setProblem(undefined);
    start.mutate(
      {
        prompt: prompt.trim(),
        ...(context ? { context } : {}),
        ...(branch ? { branch } : {}),
        ...(chosen ? { agent: chosen } : {}),
        ...(model.trim() ? { model: model.trim() } : {}),
        images: images.map(({ mediaType, data }) => ({ mediaType, data })),
      },
      {
        onSuccess: (run) => {
          setPrompt("");
          for (const image of images) URL.revokeObjectURL(image.url);
          setImages([]);
          onStarted(run);
        },
      },
    );
  };

  const error = problem ?? (start.error instanceof HttpError ? start.error.message : undefined);
  return (
    <form
      className="flex flex-col gap-2 border-t p-3"
      onSubmit={(event) => {
        event.preventDefault();
        submit();
      }}
      onDragOver={(event) => event.preventDefault()}
      onDrop={(event) => {
        event.preventDefault();
        void add(event.dataTransfer.files);
      }}
    >
      {contextLabel && (
        <div className="flex items-center gap-1 self-start rounded-md bg-info/10 px-2 py-1 text-xs text-info">
          <span className="truncate">{contextLabel}</span>
          {onClearContext && (
            <button type="button" aria-label={t("web.agent.clearContext")} onClick={onClearContext}>
              <X className="size-3" />
            </button>
          )}
        </div>
      )}
      <Textarea
        value={prompt}
        maxLength={8000}
        onChange={(event) => setPrompt(event.target.value)}
        onPaste={(event) => {
          const pasted = [...event.clipboardData.files];
          if (pasted.length) void add(pasted);
        }}
        onKeyDown={(event) => {
          if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) submit();
        }}
        placeholder={t(branch ? "web.agent.placeholderMore" : "web.agent.placeholder")}
        aria-label={t("web.agent.prompt")}
        className="min-h-20"
      />
      {images.length > 0 && (
        <div className="flex flex-wrap gap-2">
          {images.map((image, index) => (
            <div key={image.url} className="relative">
              <img
                src={image.url}
                alt={image.name}
                className="size-14 rounded border object-cover"
              />
              <button
                type="button"
                aria-label={t("web.agent.removeImage", { name: image.name })}
                className="absolute -top-1.5 -right-1.5 rounded-full bg-background shadow"
                onClick={() => {
                  URL.revokeObjectURL(image.url);
                  setImages(images.filter((_, i) => i !== index));
                }}
              >
                <X className="size-3.5" />
              </button>
            </div>
          ))}
        </div>
      )}
      {error && <p className="text-xs text-destructive">{error}</p>}
      <div className="flex items-center gap-2">
        <input
          ref={file}
          type="file"
          accept={TYPES.join(",")}
          multiple
          hidden
          onChange={(event) => {
            if (event.target.files) void add(event.target.files);
            event.target.value = "";
          }}
        />
        <Button
          type="button"
          size="icon-sm"
          variant="ghost"
          aria-label={t("web.agent.attach")}
          onClick={() => file.current?.click()}
        >
          <ImagePlus />
        </Button>
        {agents.length > 1 && (
          <select
            aria-label={t("web.agent.agent")}
            className="h-8 rounded-md border bg-background px-1 text-xs"
            value={chosen}
            onChange={(event) => setAgent(event.target.value as AgentKind)}
          >
            {agents.map((a) => (
              <option key={a} value={a}>
                {a}
              </option>
            ))}
          </select>
        )}
        <input
          aria-label={t("web.agent.model")}
          placeholder={t("web.agent.modelDefault")}
          className="h-8 w-28 min-w-0 rounded-md border bg-background px-2 text-xs"
          value={model}
          onChange={(event) => setModel(event.target.value)}
        />
        <span className="flex-1" />
        <Button type="submit" size="sm" disabled={!prompt.trim() || start.isPending}>
          {start.isPending ? <LoaderCircle className="animate-spin" /> : <Send />}
          {t(busy ? "web.agent.queue" : "web.agent.send")}
        </Button>
      </div>
      <p className={cn("text-xs text-muted-foreground", branch && "hidden")}>
        {t("web.agent.newDraftNote")}
      </p>
    </form>
  );
}

/** A file as base64 (no `data:` prefix). */
function read(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result).replace(/^data:[^,]*,/, ""));
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(file);
  });
}
