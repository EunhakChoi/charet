import { Link } from "react-router";
import { Thumb } from "@/components/Thumb";
import {
  type Capture,
  type ComponentGroup,
  type ComponentUse,
  captureImage,
  type Manifest,
} from "@/lib/api";
import { useT } from "@/lib/i18n";
import { cardSize } from "@/lib/layout";
import { useProjectContext } from "@/lib/project";
import { useComponents } from "@/lib/queries";
import { useScreenTheme } from "@/lib/theme";
import { Empty } from "./Screens";

type Story = Manifest["stories"][number];

/** A component with where the screens use it (when they do) and its stories (when it has any). */
interface Entry {
  name: string;
  used: ComponentGroup | undefined;
  stories: Story[];
}

/** Longest side of a use's picture, and how far a small one is enlarged to be legible. */
const CROP_MAX = { w: 640, h: 160 };
const CROP_ZOOM = 2;

/** The directory of a path, `""` at the top. */
function dirOf(path: string): string {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? "" : path.slice(0, slash);
}

export function Components() {
  const t = useT();
  const { key, manifest, captureOf, commentsOn } = useProjectContext();
  const [theme] = useScreenTheme();
  const used = useComponents(key, theme);
  if (!manifest || used.isPending) return <Empty text={t("web.common.loading")} />;

  // Components the screens use come first, in the folders of their files; components that only
  // have stories (written in project.tsx) follow. A failed read of the uses leaves the stories.
  const storiesOf = new Map<string, Story[]>();
  for (const story of manifest.stories)
    storiesOf.set(story.component, [...(storiesOf.get(story.component) ?? []), story]);
  const found = used.data?.components ?? [];
  const sections = new Map<string, Entry[]>();
  for (const group of found) {
    const folder = dirOf(group.file) || t("web.components.rootFolder");
    sections.set(folder, [
      ...(sections.get(folder) ?? []),
      { name: group.name, used: group, stories: storiesOf.get(group.name) ?? [] },
    ]);
  }
  const known = new Set(found.map((group) => group.name));
  const storyOnly = [...storiesOf]
    .filter(([name]) => !known.has(name))
    .map(([name, stories]): Entry => ({ name, used: undefined, stories }));
  if (storyOnly.length > 0) sections.set(t("web.components.stories"), storyOnly);
  if (sections.size === 0) return <Empty text={t("web.components.empty")} />;

  const query = theme === "dark" ? "?theme=dark" : "";
  return (
    <div className="flex-1 overflow-auto bg-canvas p-6">
      <div className="flex flex-col gap-10">
        {[...sections].map(([folder, entries]) => (
          <section key={folder} className="flex flex-col gap-6">
            <h2 className="font-mono text-xs text-muted-foreground">{folder}</h2>
            {entries.map((entry) => (
              <article
                key={entry.name}
                data-testid={`component-${entry.name}`}
                className="flex flex-col gap-3"
              >
                <div className="flex flex-wrap items-baseline gap-x-3">
                  <h3 className="font-mono text-sm font-semibold">{`<${entry.name}>`}</h3>
                  {entry.used && (
                    <>
                      <span className="font-mono text-xs text-muted-foreground">
                        {entry.used.file}
                      </span>
                      <span className="text-xs text-muted-foreground">
                        {t("web.components.uses", { count: entry.used.uses })}
                      </span>
                    </>
                  )}
                </div>
                {entry.used && entry.used.instances.length > 0 && (
                  <ul className="flex flex-wrap items-end gap-4">
                    {entry.used.instances.map((use, index) => {
                      const capture = captureOf("screen", use.screen, theme);
                      if (!capture) return null;
                      return (
                        <li
                          // biome-ignore lint/suspicious/noArrayIndexKey: the uses never reorder
                          key={index}
                        >
                          <Link
                            to={`/p/${key}/t/screen/${use.screen}${query}`}
                            data-testid={`use-${entry.name}-${index}`}
                            title={t("web.components.openScreen", {
                              name: entry.name,
                              screen: use.screen,
                            })}
                            className="block"
                          >
                            <Crop
                              projectKey={key}
                              capture={capture}
                              use={use}
                              label={use.text || entry.name}
                            />
                          </Link>
                        </li>
                      );
                    })}
                  </ul>
                )}
                {entry.stories.length > 0 && (
                  <div className="flex flex-wrap gap-6">
                    {entry.stories.map((story) => {
                      const size = cardSize(story.size, 140);
                      return (
                        <Link
                          key={story.id}
                          to={`/p/${key}/t/story/${story.id}${query}`}
                          data-testid={`story-${story.id}`}
                          className="flex flex-col gap-2"
                          style={{ width: size.w }}
                        >
                          <div style={{ height: size.h }}>
                            <Thumb
                              projectKey={key}
                              capture={captureOf("story", story.id, theme)}
                              title={story.title}
                              openComments={
                                commentsOn("story", story.id).filter((c) => c.status === "open")
                                  .length
                              }
                              className="size-full"
                            />
                          </div>
                          <span className="text-sm">{story.title}</span>
                        </Link>
                      );
                    })}
                  </div>
                )}
              </article>
            ))}
          </section>
        ))}
      </div>
    </div>
  );
}

/** One use of a component: its box cut out of the screen's capture, enlarged when it is small. */
function Crop({
  projectKey,
  capture,
  use,
  label,
}: {
  projectKey: string;
  capture: Capture;
  use: ComponentUse;
  label: string;
}) {
  const scale = Math.min(CROP_ZOOM, CROP_MAX.w / use.w, CROP_MAX.h / use.h);
  return (
    <div
      className="overflow-hidden rounded border bg-background shadow-sm"
      style={{ width: use.w * scale, height: use.h * scale }}
    >
      <img
        src={captureImage(projectKey, capture)}
        alt={label}
        draggable={false}
        loading="lazy"
        className="max-w-none"
        style={{
          width: capture.width * scale,
          marginLeft: -use.x * scale,
          marginTop: -use.y * scale,
        }}
      />
    </div>
  );
}
