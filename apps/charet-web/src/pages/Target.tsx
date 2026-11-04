import {
  ArrowLeft,
  Camera,
  CircleAlert,
  Eye,
  FileCode,
  LoaderCircle,
  MonitorPlay,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { Link, useParams, useSearchParams } from "react-router";
import { AgentPanel } from "@/components/AgentPanel";
import { CommentThread } from "@/components/CommentThread";
import { DesignPanel, type Located } from "@/components/DesignPanel";
import { Live } from "@/components/Live";
import { Cursors, Selections, usePointerShare } from "@/components/Presence";
import { Button } from "@/components/ui/button";
import { Segmented } from "@/components/ui/segmented";
import { Textarea } from "@/components/ui/textarea";
import { type Anchor, captureImage, type Element, HttpError, type TargetKind } from "@/lib/api";
import { useT } from "@/lib/i18n";
import { elementAt, parseSource, repoPath } from "@/lib/layout";
import { useProjectContext } from "@/lib/project";
import {
  useAddComment,
  useAgent,
  useAppDir,
  useBranches,
  useCapture,
  useElements,
  useLook,
  useMe,
} from "@/lib/queries";
import { useRoom } from "@/lib/realtime";
import { useCan } from "@/lib/role";
import { useScreenTheme } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { Empty } from "./Screens";

type Selection = { kind: "element"; element: Element } | { kind: "point"; x: number; y: number };

export function Target() {
  const t = useT();
  const params = useParams();
  const kind = (params.kind === "story" ? "story" : "screen") satisfies TargetKind;
  const id = params.target ?? "";
  const { key, manifest, captureOf, commentsOn } = useProjectContext();
  const [theme] = useScreenTheme();
  const [mode, setMode] = useState<"capture" | "live">("capture");
  const [pick, setPick] = useState<Selection | undefined>();
  const [focus, setFocus] = useState<number | undefined>();
  const [side, setSide] = useState<"comments" | "agent" | "design">("comments");
  const canEdit = useCan("editor");
  const agent = useAgent(canEdit);
  const app = useAppDir(key);
  const me = useMe();
  const branches = useBranches(key);
  const drafts = useMemo(
    () =>
      (branches.data?.branches ?? []).filter(
        (b) => (b.status === "open" || b.status === "in_review") && b.author.id === me.data?.id,
      ),
    [branches.data, me.data],
  );
  // Hand edits work on a draft: its picture replaces main's while the Design panel is open.
  const [draft, setDraft] = useState<number | "new">("new");
  const [want, setWant] = useState<Located | undefined>();
  const head = draft === "new" ? undefined : drafts.find((d) => d.number === draft)?.head;
  const designing = side === "design" && canEdit;
  // `?comment=12` (from a notification) opens on that comment.
  const [search] = useSearchParams();
  const linked = Number(search.get("comment")) || undefined;
  useEffect(() => {
    if (linked) setFocus(linked);
  }, [linked]);

  const item = useMemo(() => {
    if (!manifest) return undefined;
    if (kind === "screen") {
      const screen = manifest.screens.find((s) => s.id === id);
      return (
        screen && { title: screen.title, note: screen.note, size: screen.size, route: screen.route }
      );
    }
    const story = manifest.stories.find((s) => s.id === id);
    return (
      story && { title: story.title, note: story.component, size: story.size, route: undefined }
    );
  }, [manifest, kind, id]);
  const capture = captureOf(kind, id, theme);
  const elements = useElements(key, kind, id, theme, !!capture);
  const look = useLook(key, head, id, theme, designing && !!head);
  const onDraft = designing && !!head;
  const comments = commentsOn(kind, id);
  const recapture = useCapture(key);

  // A new theme or capture invalidates what was picked on the old picture.
  // biome-ignore lint/correctness/useExhaustiveDependencies: reset on these changes only
  useEffect(() => setPick(undefined), [theme, capture?.capturedAt]);
  // After a hand edit, pick the same element again on the draft's new picture.
  useEffect(() => {
    if (!want || !look.data) return;
    const found = look.data.elements.find(
      (e) => (want.use && e.use === want.use) || (want.src && e.src === want.src),
    );
    if (found) setPick({ kind: "element", element: found });
    setWant(undefined);
  }, [want, look.data]);

  if (!manifest) return <Empty text={t("web.common.loading")} />;
  if (!item) return <Empty text={`${kind} ${id}?`} />;
  const back = `/p/${key}/${kind === "screen" ? "screens" : "components"}${theme === "dark" ? "?theme=dark" : ""}`;
  return (
    <div className="flex min-h-0 flex-1">
      <section className="flex min-w-0 flex-1 flex-col">
        <div className="flex items-center gap-3 border-b px-4 py-2">
          <Button asChild variant="ghost" size="icon-sm">
            <Link to={back} aria-label={t("web.common.close")}>
              <ArrowLeft />
            </Link>
          </Button>
          <div className="min-w-0 flex-1">
            <h1 className="truncate font-semibold">{item.title}</h1>
            <p className="truncate text-xs text-muted-foreground">
              {[id, item.route, t("web.target.size", { w: item.size.w, h: item.size.h })]
                .filter(Boolean)
                .join(" · ")}
            </p>
          </div>
          <Segmented
            value={mode}
            onChange={setMode}
            options={[
              {
                value: "capture",
                label: (
                  <>
                    <Eye />
                    {t("web.target.capture")}
                  </>
                ),
              },
              {
                value: "live",
                label: (
                  <>
                    <MonitorPlay />
                    {t("web.target.live")}
                  </>
                ),
              },
            ]}
          />
          <Button
            size="sm"
            variant="outline"
            disabled={recapture.isPending}
            onClick={() => recapture.mutate({ targets: [id], themes: [theme], force: true })}
          >
            {recapture.isPending ? <LoaderCircle className="animate-spin" /> : <Camera />}
            {t("web.target.recapture")}
          </Button>
        </div>
        <div className="flex-1 overflow-auto bg-canvas p-6">
          {mode === "live" ? (
            <Live projectKey={key} kind={kind} id={id} theme={theme} size={item.size} />
          ) : onDraft ? (
            look.data ? (
              <CaptureView
                src={look.data.image}
                size={item.size}
                elements={look.data.elements}
                anchors={[]}
                pick={pick}
                focus={focus}
                onPick={setPick}
                onFocus={setFocus}
              />
            ) : (
              <Empty
                text={
                  look.error && !(look.error instanceof HttpError && look.error.code === "pending")
                    ? look.error.message
                    : t("web.design.building")
                }
              />
            )
          ) : capture ? (
            <CaptureView
              src={captureImage(key, capture)}
              size={item.size}
              elements={elements.data ?? []}
              anchors={comments
                .filter((c) => c.status === "open" && c.theme === theme)
                .map((c, index) => ({ id: c.id, index: index + 1, anchor: c.anchor }))}
              pick={pick}
              focus={focus}
              onPick={setPick}
              onFocus={setFocus}
            />
          ) : (
            <Empty text={t("web.capture.none")} />
          )}
          {capture && capture.issues.length > 0 && mode === "capture" && (
            <div className="mx-auto mt-4 max-w-3xl rounded-lg border border-destructive/30 bg-destructive/5 p-3 text-sm">
              <p className="mb-1 flex items-center gap-2 font-medium text-destructive">
                <CircleAlert className="size-4" />
                {t("web.target.issues")}
              </p>
              <ul className="list-disc pl-6 font-mono text-xs">
                {capture.issues.map((issue) => (
                  <li key={issue}>{issue}</li>
                ))}
              </ul>
            </div>
          )}
        </div>
      </section>
      <aside className="flex w-96 shrink-0 flex-col overflow-y-auto border-l">
        {canEdit && (
          <div className="border-b px-4 py-2">
            <Segmented
              label={t("web.target.side")}
              value={side}
              onChange={setSide}
              options={[
                { value: "comments", label: t("web.comments.title") },
                { value: "design", label: t("web.design.title") },
                ...(agent.data?.enabled
                  ? [{ value: "agent" as const, label: t("web.agent.title") }]
                  : []),
              ]}
            />
          </div>
        )}
        {!designing && <Inspector projectKey={key} pick={pick} />}
        {designing ? (
          <DesignPanel
            projectKey={key}
            screen={item.title}
            app={app}
            drafts={drafts}
            draft={draft}
            onDraft={(next) => {
              if (next !== draft) setPick(undefined);
              setDraft(next);
            }}
            pick={pick?.kind === "element" ? pick.element : undefined}
            onEdited={setWant}
            onAskAgent={() => setSide("agent")}
          />
        ) : side === "agent" && canEdit && agent.data?.enabled ? (
          <AgentPanel
            key={key}
            projectKey={key}
            context={agentContext(kind, id, theme, pick)}
            contextLabel={contextLabel(kind, id, pick)}
            onClearContext={pick ? () => setPick(undefined) : undefined}
          />
        ) : (
          <>
            <Composer
              projectKey={key}
              kind={kind}
              id={id}
              theme={theme}
              pick={pick}
              onDone={() => setPick(undefined)}
            />
            <div className="flex flex-col gap-3 p-4">
              <h2 className="text-sm font-semibold">{t("web.comments.title")}</h2>
              {comments.length === 0 && (
                <p className="text-sm text-muted-foreground">{t("web.comments.none")}</p>
              )}
              {[...comments]
                .sort((a, b) =>
                  a.status === b.status ? a.id - b.id : a.status === "open" ? -1 : 1,
                )
                .map((comment) => (
                  <CommentThread
                    key={comment.id}
                    projectKey={key}
                    comment={comment}
                    active={focus === comment.id}
                    onFocus={() => setFocus(comment.id)}
                  />
                ))}
            </div>
          </>
        )}
      </aside>
    </div>
  );
}

/** What the agent hears about where the person is looking. */
function agentContext(
  kind: TargetKind,
  id: string,
  theme: "light" | "dark",
  pick: Selection | undefined,
): Record<string, unknown> {
  const where = { [kind]: id, theme };
  if (pick?.kind === "element") {
    const { chain, src, text, sel, tag } = pick.element;
    return { ...where, element: { chain, src, text, selector: sel, tag } };
  }
  if (pick?.kind === "point") return { ...where, point: { x: pick.x, y: pick.y } };
  return where;
}

function contextLabel(kind: TargetKind, id: string, pick: Selection | undefined): string {
  if (pick?.kind === "element") {
    const name = pick.element.chain.at(-1) ?? `<${pick.element.tag}>`;
    return `${kind} ${id} · ${name}${pick.element.text ? ` "${pick.element.text.slice(0, 30)}"` : ""}`;
  }
  if (pick?.kind === "point") return `${kind} ${id} · (${pick.x}, ${pick.y})`;
  return `${kind} ${id}`;
}

/** The capture with element highlights, the picked element or point, and comment pins. */
function CaptureView({
  src,
  size,
  elements,
  anchors,
  pick,
  focus,
  onPick,
  onFocus,
}: {
  src: string;
  size: { w: number; h: number };
  elements: Element[];
  anchors: { id: number; index: number; anchor: Anchor }[];
  pick: Selection | undefined;
  focus: number | undefined;
  onPick: (pick: Selection) => void;
  onFocus: (id: number) => void;
}) {
  const [hover, setHover] = useState<Element | undefined>();
  const { share } = useRoom();
  const pointer = usePointerShare((clientX, clientY) => {
    const rect = surface.current?.getBoundingClientRect();
    if (!rect) return { x: 0, y: 0 };
    return {
      x: ((clientX - rect.left) / rect.width) * size.w,
      y: ((clientY - rect.top) / rect.height) * size.h,
    };
  });
  const surface = useRef<HTMLDivElement>(null);
  useEffect(() => {
    share({
      selection:
        pick?.kind === "element"
          ? { x: pick.element.x, y: pick.element.y, w: pick.element.w, h: pick.element.h }
          : null,
    });
  }, [pick, share]);
  const box = (b: { x: number; y: number; w: number; h: number }) => ({
    left: `${(b.x / size.w) * 100}%`,
    top: `${(b.y / size.h) * 100}%`,
    width: `${(b.w / size.w) * 100}%`,
    height: `${(b.h / size.h) * 100}%`,
  });
  const point = (event: React.PointerEvent<HTMLDivElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    return {
      x: Math.round(((event.clientX - rect.left) / rect.width) * size.w),
      y: Math.round(((event.clientY - rect.top) / rect.height) * size.h),
    };
  };
  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: picking a spot on a picture needs a pointer
    // biome-ignore lint/a11y/useKeyWithClickEvents: same; the comment list stays keyboard-usable
    <div
      ref={surface}
      data-testid="capture"
      className="relative mx-auto w-full cursor-crosshair overflow-hidden rounded-lg border bg-background shadow-sm"
      style={{ maxWidth: size.w, aspectRatio: `${size.w} / ${size.h}` }}
      onPointerMove={(event) => {
        const { x, y } = point(event);
        setHover(elementAt(elements, x, y));
        pointer.onPointerMove(event);
      }}
      onPointerLeave={() => {
        setHover(undefined);
        pointer.onPointerLeave();
      }}
      onClick={(event) => {
        const { x, y } = point(event as unknown as React.PointerEvent<HTMLDivElement>);
        const element = elementAt(elements, x, y);
        onPick(element ? { kind: "element", element } : { kind: "point", x, y });
      }}
    >
      <img src={src} alt="" draggable={false} className="absolute inset-0 size-full select-none" />
      {hover && (
        <div
          className="pointer-events-none absolute border-2 border-info/70 bg-info/10"
          style={box(hover)}
        />
      )}
      {pick?.kind === "element" && (
        <div
          className="pointer-events-none absolute border-2 border-info"
          style={box(pick.element)}
        />
      )}
      {pick?.kind === "point" && (
        <div
          className="pointer-events-none absolute size-4 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-info bg-info/30"
          style={{ left: `${(pick.x / size.w) * 100}%`, top: `${(pick.y / size.h) * 100}%` }}
        />
      )}
      <Selections place={box} />
      <Cursors
        place={(c) => ({ left: `${(c.x / size.w) * 100}%`, top: `${(c.y / size.h) * 100}%` })}
      />
      {anchors.map(({ id, index, anchor }) => (
        <div key={id} className="pointer-events-none absolute" style={box(anchor)}>
          {focus === id && <div className="absolute inset-0 border-2 border-warning" />}
          <button
            type="button"
            data-testid={`pin-${id}`}
            onClick={(event) => {
              event.stopPropagation();
              onFocus(id);
            }}
            className={cn(
              "pointer-events-auto absolute -top-3 -left-3 flex size-6 items-center justify-center rounded-full rounded-bl-none bg-primary text-xs font-semibold text-primary-foreground shadow",
              anchor.detached &&
                "border-2 border-dashed border-primary bg-background text-foreground",
            )}
          >
            {index}
          </button>
        </div>
      ))}
    </div>
  );
}

function Inspector({ projectKey, pick }: { projectKey: string; pick: Selection | undefined }) {
  const t = useT();
  const app = useAppDir(projectKey);
  if (pick?.kind !== "element") {
    return <p className="border-b p-4 text-sm text-muted-foreground">{t("web.inspect.hint")}</p>;
  }
  const { element } = pick;
  const source = element.src ? parseSource(element.src) : undefined;
  return (
    <div className="flex flex-col gap-3 border-b p-4 text-sm">
      <h2 className="font-semibold">{t("web.inspect.title")}</h2>
      <Field label={t("web.inspect.chain")}>
        <span className="font-medium">
          {element.chain.length ? element.chain.join(" › ") : `<${element.tag}>`}
        </span>
      </Field>
      {source && (
        <Field label={t("web.inspect.source")}>
          <Link
            className="inline-flex items-center gap-1 font-mono text-xs text-info hover:underline"
            to={`/p/${projectKey}/code?path=${encodeURIComponent(repoPath(app, source.path))}&line=${source.line}`}
          >
            <FileCode className="size-3.5" />
            {element.src}
          </Link>
        </Field>
      )}
      {element.text && <Field label={t("web.inspect.text")}>{element.text}</Field>}
      <Field label={t("web.inspect.selector")}>
        <code className="break-all text-xs text-muted-foreground">{element.sel}</code>
      </Field>
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-0.5">
      <span className="text-xs text-muted-foreground">{label}</span>
      <div>{children}</div>
    </div>
  );
}

function Composer({
  projectKey,
  kind,
  id,
  theme,
  pick,
  onDone,
}: {
  projectKey: string;
  kind: TargetKind;
  id: string;
  theme: "light" | "dark";
  pick: Selection | undefined;
  onDone: () => void;
}) {
  const t = useT();
  const [body, setBody] = useState("");
  const add = useAddComment(projectKey);
  const canComment = useCan("commenter");
  const input = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    if (pick) input.current?.focus();
  }, [pick]);
  if (!canComment)
    return <p className="border-b p-4 text-sm text-muted-foreground">{t("web.access.viewOnly")}</p>;
  if (!pick)
    return <p className="border-b p-4 text-sm text-muted-foreground">{t("web.comments.pick")}</p>;
  const submit = () => {
    if (!body.trim()) return;
    const at = pick.kind === "element" ? { selector: pick.element.sel } : { x: pick.x, y: pick.y };
    add.mutate(
      { kind, target: id, theme, at, body },
      {
        onSuccess: () => {
          setBody("");
          onDone();
        },
      },
    );
  };
  return (
    <form
      className="flex flex-col gap-2 border-b p-4"
      onSubmit={(event) => {
        event.preventDefault();
        submit();
      }}
    >
      <Textarea
        ref={input}
        value={body}
        onChange={(event) => setBody(event.target.value)}
        placeholder={t("web.comments.placeholder")}
        onKeyDown={(event) => {
          if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) submit();
        }}
      />
      {add.error && (
        <p className="text-xs text-destructive">
          {add.error instanceof HttpError ? add.error.message : String(add.error)}
        </p>
      )}
      <div className="flex justify-end gap-2">
        <Button type="button" size="sm" variant="ghost" onClick={onDone}>
          {t("web.common.cancel")}
        </Button>
        <Button type="submit" size="sm" disabled={!body.trim() || add.isPending}>
          {t("web.comments.add")}
        </Button>
      </div>
    </form>
  );
}
