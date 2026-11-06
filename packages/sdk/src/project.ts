// The project contract: what a charet project declares in `.charet/project.ts`.
// Pure data plus functions that only run inside the frame (render, mocks, setup).
// `toManifest` turns it into the JSON the server reads.

/** How one screen leads to another. */
export type ConnectionKind = "click" | "auto" | "error";

export interface Size {
  w: number;
  h: number;
}

/** A route of the app. Screens are states of a page. */
export interface PageDef {
  id: string;
  route: string;
  title: string;
  /** What the page is for, shown next to it. */
  note?: string;
}

/** One state of a page, rendered with mock data. */
export interface ScreenDef<Mock = unknown> {
  id: string;
  page: string;
  title: string;
  note?: string;
  /** Route to open; defaults to the page's route. */
  route?: string;
  /** Viewport; defaults to the project's `screenSize`. */
  size?: Size;
  /**
   * Query parameters of the screen, e.g. `{ tab: "runs" }` (an object, not a `"?tab=runs"` string).
   * They are set on `window.location` and also added to the route `renderScreen` gets, so a memory
   * router sees them too.
   */
  query?: Record<string, string>;
  /** Request handlers for this screen, tried before the project's `mocks`. */
  mocks?: Mock[];
  /** Interactions that bring the screen into its state (click a tab, fill a form). */
  setup?: (kit: SetupKit) => Promise<void>;
}

/**
 * Helpers a screen's `setup` gets. Selectors are CSS over the whole document (dialogs and menus
 * render outside the app root); text matches the trimmed text or the `aria-label` exactly.
 */
export interface SetupKit {
  root: HTMLElement;
  find(selector: string, text?: string): Promise<HTMLElement>;
  click(selector: string, text?: string): Promise<void>;
  fill(selector: string, value: string): Promise<void>;
  wait(ms: number): Promise<void>;
}

/** A move from one screen to another (Figma prototype connection). */
export interface ConnectionDef {
  from: string;
  to: string;
  label?: string;
  kind?: ConnectionKind;
}

/**
 * A step of a flow: a screen id, or `{ id, title, note? }` — a planned screen that does not exist
 * yet, or an existing screen with a title and note for this step.
 */
export type FlowStepDef = string | { id: string; title: string; note?: string };

/** The order a user goes through a feature (Figma prototype flow). */
export interface FlowDef {
  id: string;
  title: string;
  /** What the user is trying to do, shown with the flow. */
  note?: string;
  steps: FlowStepDef[];
}

/** One state of a component (Storybook meaning), shown in the Components view. */
export interface StoryDef<Node = unknown, Mock = unknown> {
  id: string;
  /** Component name, e.g. `Button`. Stories of one component are grouped. */
  component: string;
  title: string;
  render: () => Node;
  size?: Size;
  /** Request handlers for components that fetch, tried before the project's `mocks`. */
  mocks?: Mock[];
  /**
   * What this state sets (`{ variant: "outline", size: "sm" }`). The Components view lists each
   * prop with its values and shows the story that matches the picked ones.
   */
  props?: Record<string, string>;
  /** The code that makes this state, shown next to the preview. */
  code?: string;
  /**
   * Which part of the Components view the component is listed under (`"Feature"`). The Components
   * view switches between groups; a story with none is in the default group (`Base`).
   */
  group?: string;
}

/** One component with every combination of some props to show (see `variants`). */
export interface VariantsDef<Node = unknown, Mock = unknown> {
  /** Component name, e.g. `Button`. */
  component: string;
  /** Each prop and the values to show; one story is made for every combination. */
  props: Record<string, string[]>;
  /** Renders the component with one combination (the example text goes here). */
  render: (props: Record<string, string>) => Node;
  /** The code that makes one combination. */
  code?: (props: Record<string, string>) => string;
  /** The Components view group of the component (see `StoryDef.group`). */
  group?: string;
  size?: Size;
  mocks?: Mock[];
}

/** The most stories one `variants` call makes: a bigger matrix is a mistake, not a gallery. */
export const MAX_VARIANTS = 200;

/**
 * The stories of a component for every combination of `props`, so one declaration gives the
 * Components view a prop picker and a preview for each state. Ids are `button-variant-outline-size-sm`.
 */
export function variants<Node, Mock>(def: VariantsDef<Node, Mock>): StoryDef<Node, Mock>[] {
  const slug = (text: string) =>
    text
      .replace(/([a-z0-9])([A-Z])/g, "$1-$2")
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "");
  const names = Object.keys(def.props);
  const total = names.reduce((count, name) => count * (def.props[name]?.length ?? 0), 1);
  if (total > MAX_VARIANTS)
    throw new Error(`variants of ${def.component} make ${total} stories (at most ${MAX_VARIANTS})`);
  const all = names.reduce<Record<string, string>[]>(
    (found, name) =>
      found.flatMap((one) => (def.props[name] ?? []).map((v) => ({ ...one, [name]: v }))),
    [{}],
  );
  return all.map((props) => {
    const parts = names.map((name) => `${name}-${props[name]}`);
    return {
      id: [slug(def.component), ...parts.map(slug)].join("-"),
      component: def.component,
      title: names.map((name) => props[name]).join(" · ") || "Default",
      render: () => def.render(props),
      props,
      ...(def.code ? { code: def.code(props) } : {}),
      ...(def.group ? { group: def.group } : {}),
      ...(def.size ? { size: def.size } : {}),
      ...(def.mocks ? { mocks: def.mocks } : {}),
    };
  });
}

export interface ProjectDef<Mock = unknown, Node = unknown> {
  title: string;
  pages: PageDef[];
  screens: ScreenDef<Mock>[];
  connections?: ConnectionDef[];
  flows?: FlowDef[];
  stories?: StoryDef<Node, Mock>[];
  /** Handlers every screen falls back to. */
  mocks?: Mock[];
  /** Default screen viewport. */
  screenSize?: Size;
}

/** Declares a project. Checks it eagerly so mistakes surface at load time. */
export function defineProject<Mock, Node>(project: ProjectDef<Mock, Node>): ProjectDef<Mock, Node> {
  const problems = validate(project);
  if (problems.length > 0) {
    throw new Error(`invalid charet project:\n${problems.map((p) => `  - ${p}`).join("\n")}`);
  }
  return project;
}

/**
 * The route a screen opens: its route (or its page's) with the screen's `query` merged in; the
 * route's own query stays, the screen's value wins. A memory router has no `window.location` to
 * read the query from, so the frame hands it over inside the route.
 */
export function routeWithQuery(route: string, query?: Record<string, string>): string {
  if (!query || Object.keys(query).length === 0) return route;
  const url = new URL(route, "http://charet.invalid");
  for (const [key, value] of Object.entries(query)) url.searchParams.set(key, value);
  return `${url.pathname}${url.search}${url.hash}`;
}

export const DEFAULT_SCREEN_SIZE: Size = { w: 1440, h: 900 };
export const DEFAULT_STORY_SIZE: Size = { w: 480, h: 320 };

/** Every problem in `project`, as messages naming the offending id. */
export function validate(project: ProjectDef): string[] {
  const problems: string[] = [];
  const unique = (kind: string, ids: string[]) => {
    const seen = new Set<string>();
    for (const id of ids) {
      if (!/^[a-z0-9][a-z0-9-]*$/.test(id))
        problems.push(`${kind} id "${id}" must be lowercase letters, digits or dashes`);
      if (seen.has(id)) problems.push(`${kind} id "${id}" is used twice`);
      seen.add(id);
    }
    return seen;
  };
  const pages = unique(
    "page",
    project.pages.map((p) => p.id),
  );
  const screens = unique(
    "screen",
    project.screens.map((s) => s.id),
  );
  unique(
    "flow",
    (project.flows ?? []).map((f) => f.id),
  );
  unique(
    "story",
    (project.stories ?? []).map((s) => s.id),
  );
  for (const page of project.pages) {
    if (!page.route.startsWith("/")) problems.push(`page "${page.id}" route must start with /`);
  }
  for (const screen of project.screens) {
    if (!pages.has(screen.page))
      problems.push(`screen "${screen.id}" is on unknown page "${screen.page}"`);
    if (screen.size && !validSize(screen.size))
      problems.push(`screen "${screen.id}" size must be positive`);
  }
  for (const c of project.connections ?? []) {
    for (const end of [c.from, c.to]) {
      if (!screens.has(end))
        problems.push(`connection ${c.from} → ${c.to} names unknown screen "${end}"`);
    }
  }
  for (const flow of project.flows ?? []) {
    if (flow.steps.length === 0) problems.push(`flow "${flow.id}" has no steps`);
  }
  if (project.screenSize && !validSize(project.screenSize))
    problems.push("screenSize must be positive");
  return problems;
}

function validSize(size: Size): boolean {
  return size.w > 0 && size.h > 0 && Number.isFinite(size.w) && Number.isFinite(size.h);
}

/** JSON the server reads (`.charet/manifest.json`). Functions and handlers are left out. */
export interface Manifest {
  manifestVersion: 1;
  title: string;
  pages: Required<PageDef>[];
  screens: {
    id: string;
    page: string;
    title: string;
    note: string;
    route: string;
    size: Size;
    hasSetup: boolean;
  }[];
  connections: Required<ConnectionDef>[];
  flows: {
    id: string;
    title: string;
    note: string;
    steps: { id: string; title: string; note: string; planned: boolean }[];
  }[];
  stories: {
    id: string;
    component: string;
    title: string;
    size: Size;
    props?: Record<string, string>;
    /** The props' names in the order the story declared them (JSON object keys do not keep it). */
    propOrder?: string[];
    code?: string;
    group?: string;
  }[];
}

export function toManifest(project: ProjectDef): Manifest {
  const pageRoute = new Map(project.pages.map((p) => [p.id, p.route]));
  const screenTitle = new Map(project.screens.map((s) => [s.id, s.title]));
  const screenSize = project.screenSize ?? DEFAULT_SCREEN_SIZE;
  return {
    manifestVersion: 1,
    title: project.title,
    pages: project.pages.map(({ id, route, title, note }) => ({
      id,
      route,
      title,
      note: note ?? "",
    })),
    screens: project.screens.map((s) => ({
      id: s.id,
      page: s.page,
      title: s.title,
      note: s.note ?? "",
      route: s.route ?? pageRoute.get(s.page) ?? "/",
      size: s.size ?? screenSize,
      hasSetup: typeof s.setup === "function",
    })),
    connections: (project.connections ?? []).map((c) => ({
      from: c.from,
      to: c.to,
      label: c.label ?? "",
      kind: c.kind ?? "click",
    })),
    flows: (project.flows ?? []).map((f) => ({
      id: f.id,
      title: f.title,
      note: f.note ?? "",
      steps: f.steps.map((step) =>
        typeof step === "string"
          ? {
              id: step,
              title: screenTitle.get(step) ?? step,
              note: "",
              planned: !screenTitle.has(step),
            }
          : {
              id: step.id,
              title: step.title,
              note: step.note ?? "",
              planned: !screenTitle.has(step.id),
            },
      ),
    })),
    stories: (project.stories ?? []).map((s) => ({
      id: s.id,
      component: s.component,
      title: s.title,
      size: s.size ?? DEFAULT_STORY_SIZE,
      ...(s.props && Object.keys(s.props).length > 0
        ? { props: s.props, propOrder: Object.keys(s.props) }
        : {}),
      ...(s.code ? { code: s.code } : {}),
      ...(s.group ? { group: s.group } : {}),
    })),
  };
}
