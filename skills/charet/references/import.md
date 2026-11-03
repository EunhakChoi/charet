# Put an existing app on charet

How to bring an app that already lives in its own repository into charet, so the team can review
its screens. The app's repository is never modified: charet keeps a copy, and everything charet
needs goes into `.charet/` inside that copy.

## 1. Import

Run from anywhere; the path is the app folder (in a monorepo, the app inside it).

```bash
charet import ../product/apps/web --key web --title "Web" --dry-run   # see what would be copied
charet import ../product/apps/web --key web --title "Web"            # version 1
charet checkout web ~/charet/web                                        # your working copy
```

- Without `--key` the key is the app folder's name (lowercased, other characters as `-`).
- The import copies the app, the workspace packages it uses and the root files a build needs
  (lockfile, workspace config). `--include dir` adds anything it missed. Ignored files
  (`node_modules`, `dist`) never travel.
- The checkout remembers its server and project in `.charet/checkout.json`, which stays local
  (never pushed); commands run inside it need no `--server`. A checkout of main sits on the git
  branch `charet-base` — that is normal, push from there. `charet push` sends the work tree as it
  is: no `git commit` needed.

## 2. Add `.charet/` next to the app's `package.json`

```
apps/web/.charet/
  package.json         charet-only dependencies
  pnpm-workspace.yaml  install settings for that folder
  pnpm-lock.yaml       written by `pnpm install` in .charet/
  vendor/              the @charet/sdk tarball (until the SDK is published)
  vite.config.ts       the app's own config + charet()
  frame.html           entry page of the frame
  frame.tsx            providers + router for a screen, wrapper for a story
  project.tsx          pages, screens, connections, flows, stories
  mocks.ts             MSW handlers (split into mocks/ when it grows)
```

Why a separate `package.json`: the app's own `package.json` and lockfile must stay identical to
its repository, or every later import and handoff turns into a merge. charet installs the app's
dependencies first, then `.charet/`'s.

### package.json, pnpm-workspace.yaml

```json
{
  "name": "web-charet",
  "private": true,
  "type": "module",
  "dependencies": {
    "@charet/sdk": "file:vendor/charet-sdk-<hash>.tgz",
    "msw": "2.15.0"
  }
}
```

```yaml
# React and vite come from the app; installing them again here would load React twice.
autoInstallPeers: false

allowBuilds:
  msw: false
```

- `"type": "module"` matters: without it vite loads `.charet/vite.config.ts` as CommonJS and fails
  with "is not a function" errors.
- The SDK is not on npm yet. Pack it from the charet repository (it needs that repository's
  `pnpm install` done; packing builds `packages/sdk/dist`) and vendor it under a name that
  changes with its content, so the lockfile notices a new SDK:

  ```bash
  pnpm --dir <charet-repo>/packages/sdk pack --pack-destination /tmp/sdk
  h=$(sha256sum /tmp/sdk/charet-sdk-0.0.0.tgz | cut -c1-8)
  mkdir -p .charet/vendor && cp /tmp/sdk/charet-sdk-0.0.0.tgz .charet/vendor/charet-sdk-$h.tgz
  ```

- Mock helpers the project needs (e.g. `openapi-msw`) go here too, never into the app.
- Then `pnpm install` inside `.charet/` and keep the lockfile it writes (it is pushed with
  `.charet/`). A peer-dependency warning there is expected — react and vite come from the app; do
  not add them.
- With npm instead of pnpm: same `package.json`, `npm install` inside `.charet/`, keep
  `package-lock.json`.

### vite.config.ts

Reuse the app's config so aliases, CSS and plugins match production, and add `charet()`:

```ts
import { charet } from "@charet/sdk/vite";
import { defineConfig, mergeConfig, type UserConfig } from "vite";
import app from "../vite.config.ts";

export default defineConfig(async (env) => {
  const base = (typeof app === "function" ? await app(env) : app) as UserConfig;
  return mergeConfig(base, { plugins: [charet()] });
});
```

charet runs vite from the app folder with this file, so `root` is the app and the app's `public/`
still works. Steps the app's `prebuild` script normally does (copying assets etc.) must happen in
a plugin here, because charet calls vite directly.

### frame.html, frame.tsx

`frame.html` is a plain page with a `#root` element loading `./frame.tsx` (example in `project.md`). `frame.tsx` renders a
screen the way the app does: same providers (query client, theme, i18n, auth stub) and the app's
routes in a memory router at the screen's route. Read the app's `main.tsx` and copy its provider
tree; leave out anything that talks to a real backend on start.

### project.tsx, mocks

See `project.md` and `mocks.md`. Start small: one page, its main screen, an empty state and an
error state. More screens are cheap once the first one renders.

The manifest is built by importing `project.tsx` in Node, so modules it pulls in must load
without a browser. Lazy-load browser-only libraries (PDF viewers, canvas editors) inside
components: `const Viewer = lazy(() => import("@/features/pdf/Viewer"))`.

## 3. Try it, push, check

```bash
charet dev          # installs the app's and .charet/'s dependencies, runs vite with
                   # .charet/vite.config.ts → open /.charet/frame.html?screen=<id> on its port
charet push -m "charet: first screens" --merge
charet check        # every screen and story; fix each missing mock and console error
charet capture --theme both
charet url <id>     # links for the team
```

`charet dev` keeps running and is for a person with a browser; an agent can skip it and go
straight to push and `check`, which reports more. `check` and `capture` run on what was pushed (and merged, for `capture`), so push before
checking: an unpushed fix is invisible to them. Later fixes: `charet push -m "…" --merge` again.

## Common problems

| Symptom | Cause · fix |
|---|---|
| `[charet] missing mock GET /api/…` | No handler: add one (screen `mocks` or project `mocks`) |
| `[charet] unmocked request reached the network: …` | The app's API client skipped the mocks: import the app inside `renderScreen` (see `project.md`, frame.tsx), not at the top of `frame.tsx` |
| Broken images or fonts on previews | Absolute paths into `public/` are rewritten only when written as string literals; build URLs from `import.meta.env.BASE_URL` otherwise |
| Times differ from what people expect | Rendering uses the render service's time zone (`CHARET_TZ`); pin dates in mocks |
| Hooks error / "Invalid hook call" | React installed twice: check `autoInstallPeers: false` and that `.charet/package.json` has no `react` |
| Preview build failed | `charet check` prints the reason; `curl <server>/api/projects/<key>/preview` lists each step (extract, install, build) with the end of its output |
