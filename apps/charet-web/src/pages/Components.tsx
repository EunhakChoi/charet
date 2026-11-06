import { Check, Copy, ExternalLink } from "lucide-react";
import { useState } from "react";
import { Link } from "react-router";
import { copyText } from "@/components/CopyOnce";
import { Live } from "@/components/Live";
import { Button } from "@/components/ui/button";
import type { Manifest } from "@/lib/api";
import { byComponent, firstStory, pickProp, plainStories, propChoices } from "@/lib/gallery";
import { useT } from "@/lib/i18n";
import { useProjectContext } from "@/lib/project";
import { useScreenTheme } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { Empty } from "./Screens";

type Story = Manifest["stories"][number];

/** A row of mutually exclusive choices; the picked one is highlighted. */
function Chips({
  label,
  testId,
  options,
}: {
  label: string;
  testId: string;
  options: { value: string; picked: boolean; onPick: () => void }[];
}) {
  return (
    <div className="flex items-start gap-3 border-t py-3">
      <code className="w-24 shrink-0 pt-1 text-xs font-semibold">{label}</code>
      <div className="flex flex-wrap gap-1.5">
        {options.map((option) => (
          <Button
            key={option.value}
            size="sm"
            variant={option.picked ? "default" : "secondary"}
            aria-pressed={option.picked}
            data-testid={`${testId}-${option.value}`}
            onClick={option.onPick}
          >
            {option.value}
          </Button>
        ))}
      </div>
    </div>
  );
}

function CodeBlock({ code }: { code: string }) {
  const t = useT();
  const [copied, setCopied] = useState(false);
  return (
    <div className="relative">
      <pre className="overflow-x-auto rounded-lg border bg-muted p-4 pr-14 font-mono text-xs leading-relaxed">
        {code}
      </pre>
      <Button
        size="icon-sm"
        variant="ghost"
        className="absolute top-2 right-2"
        aria-label={t("web.common.copy")}
        onClick={() =>
          copyText(code).then((ok) => {
            setCopied(ok);
            setTimeout(() => setCopied(false), 1400);
          })
        }
      >
        {copied ? <Check /> : <Copy />}
      </Button>
    </div>
  );
}

/** One component: pick a state on the left, see it live on the right. */
function Gallery({ name, stories }: { name: string; stories: Story[] }) {
  const t = useT();
  const { key } = useProjectContext();
  const [theme] = useScreenTheme();
  const [picked, setPicked] = useState<string | undefined>();
  const story = stories.find((s) => s.id === picked) ?? firstStory(stories);
  if (!story) return null;
  const props = propChoices(stories);
  const plain = plainStories(stories);
  return (
    <article
      id={`component-${name}`}
      data-testid={`component-${name}`}
      className="grid scroll-mt-16 gap-x-14 gap-y-6 border-t py-10 first:border-t-0 md:grid-cols-2"
    >
      <div className="flex min-w-0 flex-col gap-4">
        <div className="flex items-baseline gap-3">
          <h2 className="font-mono text-2xl font-semibold">{`<${name}>`}</h2>
          <Link
            to={`/p/${key}/t/story/${story.id}${theme === "dark" ? "?theme=dark" : ""}`}
            data-testid={`story-${story.id}`}
            title={t("web.components.openStory")}
            className="inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
          >
            <ExternalLink className="size-3.5" />
            {t("web.components.openStory")}
          </Link>
        </div>
        <div>
          {props.map((prop) => (
            <Chips
              key={prop.name}
              label={prop.name}
              testId={`prop-${name}-${prop.name}`}
              options={prop.values.map((value) => ({
                value,
                picked: story.props?.[prop.name] === value,
                onPick: () => setPicked(pickProp(stories, story, prop.name, value).id),
              }))}
            />
          ))}
          {plain.length > 0 && (props.length > 0 || plain.length > 1) && (
            <Chips
              label={t("web.components.states")}
              testId={`state-${name}`}
              options={plain.map((s) => ({
                value: s.title,
                picked: s.id === story.id,
                onPick: () => setPicked(s.id),
              }))}
            />
          )}
        </div>
        {story.code && <CodeBlock code={story.code} />}
      </div>
      <div className="min-w-0">
        <div
          className={cn(
            "sticky top-4 flex min-h-72 items-center justify-center rounded-2xl border bg-background p-6",
          )}
        >
          <Live projectKey={key} kind="story" id={story.id} theme={theme} size={story.size} />
        </div>
      </div>
    </article>
  );
}

export function Components() {
  const t = useT();
  const { manifest } = useProjectContext();
  if (!manifest) return <Empty text={t("web.common.loading")} />;
  if (manifest.stories.length === 0) return <Empty text={t("web.components.empty")} />;
  const components = byComponent(manifest.stories);
  return (
    <div className="flex-1 overflow-auto bg-canvas">
      <div className="mx-auto flex max-w-6xl flex-col px-6 pb-16">
        <nav
          aria-label={t("web.components.title")}
          className="sticky top-0 z-10 -mx-6 flex flex-wrap gap-1.5 border-b bg-canvas/95 px-6 py-3 backdrop-blur"
        >
          {components.map(({ name }) => (
            <a
              key={name}
              href={`#component-${name}`}
              onClick={(event) => {
                event.preventDefault();
                document
                  .getElementById(`component-${name}`)
                  ?.scrollIntoView({ behavior: "smooth", block: "start" });
              }}
              className="rounded-full bg-secondary px-3 py-1 text-xs hover:bg-accent"
            >
              {name}
            </a>
          ))}
        </nav>
        {components.map(({ name, stories }) => (
          <Gallery key={name} name={name} stories={stories} />
        ))}
      </div>
    </div>
  );
}
