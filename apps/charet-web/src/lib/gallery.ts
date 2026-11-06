import type { Manifest } from "./api";

type Story = Manifest["stories"][number];

/** One component with its states. */
export interface ComponentStories {
  name: string;
  stories: Story[];
}

/** A prop of a component and the values its stories set, in the order they first appear. */
export interface PropChoice {
  name: string;
  values: string[];
}

/** The stories of each component, components in the order they first appear. */
export function byComponent(stories: Story[]): ComponentStories[] {
  const found = new Map<string, Story[]>();
  for (const story of stories)
    found.set(story.component, [...(found.get(story.component) ?? []), story]);
  return [...found].map(([name, list]) => ({ name, stories: list }));
}

/**
 * What a story sets, in order. A project built with the first SDK that had `props` wrote an
 * object (`{ variant: "outline" }`, key order unreliable), later ones an array; builds stay in
 * history and in use, so both are read.
 */
export function propsOf(story: Story): { name: string; value: string }[] {
  const props: unknown = story.props;
  if (Array.isArray(props)) return props as { name: string; value: string }[];
  if (props && typeof props === "object")
    return Object.entries(props).map(([name, value]) => ({ name, value: String(value) }));
  return [];
}

/** What `story` sets `name` to, when it sets it. */
export function propValue(story: Story, name: string): string | undefined {
  return propsOf(story).find((prop) => prop.name === name)?.value;
}

/** The props the stories of a component set (none when they are plain, titled states). */
export function propChoices(stories: Story[]): PropChoice[] {
  const found = new Map<string, string[]>();
  for (const story of stories) {
    for (const { name, value } of propsOf(story)) {
      const values = found.get(name) ?? [];
      if (!values.includes(value)) values.push(value);
      found.set(name, values);
    }
  }
  return [...found].map(([name, values]) => ({ name, values }));
}

/**
 * The story to show after `name` is set to `value`: of the stories that set it, the one that keeps
 * the most of the other props as they were (the first when several do). A matrix with gaps never
 * leaves the picker on a state that does not exist.
 */
export function pickProp(stories: Story[], current: Story, name: string, value: string): Story {
  let best: Story | undefined;
  let bestKept = -1;
  for (const story of stories) {
    if (propValue(story, name) !== value) continue;
    const kept = propsOf(current).filter(
      (other) => other.name !== name && propValue(story, other.name) === other.value,
    ).length;
    if (kept > bestKept) {
      best = story;
      bestKept = kept;
    }
  }
  return best ?? current;
}

/** The story to open with: the one whose props are all a prop's first value or `default`. */
export function firstStory(stories: Story[]): Story | undefined {
  const choices = propChoices(stories);
  const wanted = Object.fromEntries(
    choices.map(({ name, values }) => [name, values.includes("default") ? "default" : values[0]]),
  );
  return (
    stories.find((story) =>
      Object.entries(wanted).every(([name, v]) => propValue(story, name) === v),
    ) ?? stories[0]
  );
}

/** The stories that set no props: states picked by their title, next to the prop picker. */
export function plainStories(stories: Story[]): Story[] {
  return stories.filter((story) => propsOf(story).length === 0);
}
