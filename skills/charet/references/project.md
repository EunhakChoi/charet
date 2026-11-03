# The `.charet/` contract

A charet project is an ordinary vite + React app with a `.charet/` folder:

```
.charet/
  project.tsx   what the charet shows: pages, screens, connections, flows, stories
  frame.tsx     how a screen or story is rendered (router, providers, global CSS)
  frame.html    the frame's HTML entry
  mocks.ts      mock data and handlers (any name; imported by project.tsx)
```

The `charet()` vite plugin (from `@charet/sdk/vite`) builds the frame and the manifest the server
reads. It only acts in dev and in charet's preview builds; the app's own build is untouched.

## project.tsx

```tsx
import { defineProject } from "@charet/sdk";
import { HttpResponse, http } from "msw";
import { Button } from "@/components/ui/button";
import { sampleTasks, tasks } from "./mocks.ts";

export default defineProject({
  title: "Tasks",
  screenSize: { w: 1440, h: 900 },          // default viewport
  mocks: [],                                // handlers every screen falls back to
  pages: [{ id: "tasks", route: "/", title: "Tasks" }],
  screens: [
    { id: "tasks-list", page: "tasks", title: "Task list", mocks: [tasks(sampleTasks)] },
    {
      id: "tasks-open",
      page: "tasks",
      title: "Open tasks only",
      mocks: [tasks(sampleTasks)],
      setup: (kit) => kit.click("button", "Open"),   // bring the screen into its state
    },
    {
      id: "tasks-error",
      page: "tasks",
      title: "Load failed",
      mocks: [http.get("/api/tasks", () => new HttpResponse(null, { status: 500 }))],
    },
  ],
  connections: [{ from: "tasks-list", to: "tasks-open", label: "Open", kind: "click" }],
  flows: [
    {
      id: "triage",
      title: "Triage tasks",
      steps: ["tasks-list", "tasks-open", { id: "task-detail", title: "Task detail", note: "planned" }],
    },
  ],
  stories: [
    { id: "button-default", component: "Button", title: "Default", render: () => <Button>Save</Button> },
  ],
});
```

Rules (checked when the project loads):

- Ids are lowercase letters, digits and dashes, unique within their kind.
- A screen's `page` names a page; `route` defaults to the page's route; `size` to `screenSize`.
- Connections and non-planned flow steps name screens. Kinds: `click` (default), `auto`, `error`.
- A planned step (`{ id, title, note? }`) is a screen that does not exist yet. Adding a screen with
  that id turns it into a real step.
- A flow step object whose id is an existing screen stays a real step with its own `title` / `note`.
- Pages and flows take an optional `note`, shown in the web tool and the CLI.
- A screen's `query` is an object, `{ tab: "runs" }` (not a `"?tab=runs"` string). It is set on the
  URL before the screen renders and added to the `route` that `renderScreen` gets, so a memory
  router shows the tab too. A frame built on an older SDK passes the route without it: serialize
  `screen.query` into `initialEntries` yourself.
- `setup(kit)`: `kit.click(selector, text?)`, `kit.fill(selector, value)`, `kit.find(selector, text?)`,
  `kit.wait(ms)`. Selectors are CSS, searched in the whole document (dialogs in portals too); text
  matches the trimmed text or the `aria-label`.
- Stories are grouped by `component` in the Components view. A story's `mocks` start MSW like a
  screen's.

## Imported apps

An imported app keeps its own `package.json` and lockfile untouched; charet-only dependencies and
the vite config live in `.charet/` (`package.json`, `pnpm-workspace.yaml`, `vite.config.ts`). See
`import.md`.

## Static files

Files in `.charet/public/` are served at the root of the frame (dev and preview builds), next to
the app's own `public/`. The MSW worker comes from the `msw` package unless the app ships its own
`mockServiceWorker.js`. Previews are served under a sub path, so string literals naming a public
file (`"/logo.png"`) are rewritten to `import.meta.env.BASE_URL` at build time.

## frame.html

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="./frame.tsx"></script>
  </body>
</html>
```

`startFrame` renders into `#root` (another id: `rootId`).

## frame.tsx

```tsx
import { startFrame } from "@charet/sdk/frame";
import { createMemoryRouter, RouterProvider } from "react-router";
import "@/index.css";
import project from "./project.tsx";

void startFrame({
  project,
  // Load the app here, after the mocks run (see below).
  renderScreen: async (_screen, route) => {
    const { routes } = await import("@/routes");
    return <RouterProvider router={createMemoryRouter(routes, { initialEntries: [route] })} />;
  },
  wrapStory: (node) => <div className="p-6">{node}</div>,
});
```

**Import the app inside `renderScreen`, not at the top of `frame.tsx`.** The mocks answer `fetch` and
XHR from the moment `startFrame` has started them. An app that builds its API client while its
modules load (a module-level `createClient(...)`, or `const f = fetch`) keeps the `fetch` it saw
then: imported at the top, that client talks to the real network, the screen shows a login page
or `HTTP 404` instead of the mock data. `renderScreen` may be
async for this reason; `project.tsx` should import only types and leaf components from the app.

Wrap screens in whatever providers the app needs (query client, theme, auth stubs), and in
`<StrictMode>` when the app's `main.tsx` uses it. The frame
reads `?screen=` / `?story=` and `?theme=dark` (it sets the `dark` class on `<html>`).

## charet.json

Written by `charet import` at the repository root: where the project came from, the app folder
(`app`), the workspace packages it uses and the root files kept. Paths in captures and comment
sources are relative to the app folder.
