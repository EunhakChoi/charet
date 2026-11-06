---
name: charet
description: Work with charet, a code-first design board where an app's real React UI is rendered with mock APIs as screens, flows and component stories that a team reviews and comments on. Use this skill whenever the user mentions charet or a charet project, design or UI review comments to handle, screens, flows or stories, a `.charet/` folder or a charet checkout, putting an app on charet (import), or wants a UI change checked, captured or shown to the team — even if they only say "check the screen", "the designer left comments" or "make this reviewable". Also use it when the product repo was edited outside charet and charet may be out of date ("I changed the UI in the repo directly", "charet looks old", repo drift, `charet import --update`), and before editing the repo directly while charet holds versions not handed off yet.
---

# charet

A charet project is a real app (React + its own components) plus a `.charet/` folder that declares
**screens** (a page in one state, rendered with mock API data), **connections** between screens,
**flows** (the order a user goes through a feature) and **stories** (component states). The team
looks at them in the web tool and pins **comments** to elements. You change code in a checkout;
the `charet` CLI is how you see what the team sees.

| Who | Does |
|---|---|
| Team (web tool) | browse screens, flows and stories; click an element to comment |
| You (code) | edit the app and `.charet/` in a checkout, `charet push` to a branch |
| You (CLI) | read structure and comments, check and capture, reply and resolve |

The server keeps its own copy of the project. Changes travel as branches: a push from a checkout of
main starts a branch (a draft) and the checkout follows it; the team reviews it in the web tool,
and merging it makes a new version of main. The product repository is not touched by charet; a
checkout is a working copy of charet's copy, not of the product repo.

## Start here

```bash
charet status            # inside a checkout: its branch (if any), newer versions, local changes
charet comments          # open comments (exit 1 while some are open)
charet ls                # screens, stories, flows and whether their captures are current
```

- Commands run inside a checkout use its project and server. Elsewhere add `-p <project>`
  (and `--server <url>` when the server is not the default `http://127.0.0.1:5280`).
- No checkout yet: `charet projects`, then `charet checkout <project> <dir>`.
- If `charet status` says main moved, `charet pull` before editing — it saves a refused push later.
- "Cannot reach the charet server" → `charet doctor` says which part is down. No charet at all
  yet: in a clone of the charet repository, `./start.sh` starts it with Docker (ask the user before
  installing anything; the first build takes minutes).
- "Enter your token first" / "token was reissued or removed" → the CLI is not signed in. When the
  server runs on this machine (`charet doctor` reaches it at localhost), run `charet setup --yes`
  yourself: it reads the owner's token from the server and keeps it for the CLI — no need to ask
  anyone. It does not install this skill; that is `charet skill install`, only when the user wants
  it. Its output has the owner's admin link, which carries the token:
  whoever set charet up needs it once to sign their browser in, so hand it to them in this
  conversation when they ask for it, and on a fresh setup they have not opened yet. Keep it out of
  anything that outlives the chat (commits, files, PR text, channels, other people); the team gets
  the plain addresses from `charet url`. Otherwise (a team server
  elsewhere) ask the person for the token an admin gave them and have them run
  `charet login --token <token>`; CI uses `CHARET_TOKEN`.
- On a team server, `charet inbox` lists what concerns the person (new comments, answers in their
  threads) with links — a quick way to find what to handle first.
- Every command takes `--json` when you need to parse the output.

## Workflow 1 · Handle comments

0. Recommended: check charet matches the product repo first (Workflow 4c). A comment about a screen
   that is out of date is about the wrong UI.
1. `charet comments`, then `charet comments show c12`. It saves the snapshot the comment was made
   on and prints its path, the pinned element, its component chain (outermost › innermost) and
   the source location.
2. Read the snapshot image. A comment is about *that element in that state*; the text alone is
   often ambiguous ("make this clearer").
3. The innermost component is often a shared primitive (Button, Card). Fix it there only when the
   comment is really about the primitive — a shared change alters every screen using it.
4. If you cannot tell what is wanted, ask on the comment instead of guessing:
   `charet comments reply c12 -m "…"`. The team reads replies where they left the comment.
5. Fix the code, push, verify (Workflow 3).
6. `charet comments resolve c12 -m "what changed (branch #n or vN)"` — after you have looked at
   the result. The note is what the reviewer reads to decide whether to reopen.

If a reply or resolve fails with "changed meanwhile", someone touched the comment. Re-read it and
act on the latest content.

## Workflow 2 · Build a UI feature (flow first)

A flow written before the code is the plan the team can review; planned steps are your to-do list.

0. Recommended: check charet matches the product repo first (Workflow 4c), so you build on the UI
   people actually run.
1. In `.charet/project.tsx`, add or extend a flow. Steps that do not exist yet are planned:
   `{ id: "export-dialog", title: "Export dialog", note: "CSV, date range" }`.
2. Build the UI in the app.
3. Add a screen with the **same id** as each planned step: its page, mocks for the state it shows
   and, if it needs interaction to reach that state, a `setup`. Add connections from the screens
   that lead to it. Empty, loading and error states are screens of their own.
   (`references/project.md`, `references/mocks.md`)
4. Push and verify (Workflow 3); `charet flow <id> --sheet /tmp/flow.jpg` shows the whole flow.
5. When the feature changes shape (a step added, split or dropped), change the flow too, so it
   keeps describing what was built.

## Workflow 3 · Verify a UI change

```bash
charet push -m "what and why"       # from main: a new branch #n; in a branch checkout: adds to it
charet check <id…>                  # renders this checkout's branch: errors, console, missing mocks
charet capture <id…> --out /tmp/cv  # in a branch checkout: pictures of the branch, for you to look at
charet branch review                # ask the team to look (a push without --merge printed the branch link)
charet branch merge                 # once agreed (or `charet push -m … --merge` when working alone)
charet capture <id…>                # on main after merging: update what the team sees
charet ls --stale                   # screens whose capture is older than the code
```

- Push first: `check` builds and renders what was pushed (the checkout's branch), so an unpushed
  fix is invisible to it. `capture` stores what everyone sees, so it only stores on main; on a
  branch `--out` saves the branch's pictures and leaves the board alone. On main, `--out` stores
  as usual and also saves a copy to look at. Look at them before saying a change is done.
- Merge only when the user (or the team, in review) agreed; a draft is the safe default. When the
  user asked for the change outright and works alone, `charet push -m "…" --merge` does both.
- After a merge the checkout that followed the branch is back on main (the output says so); with
  local changes it stays on the branch and says that instead. Capture for the board from a checkout
  on main (`charet status`): from a branch checkout `capture --out` only saves pictures here and
  prints that the board is unchanged. An empty `charet ls --stale` shows the board is current.
- Merge refused with a clash: someone changed the same lines on main. `charet branch catch-up`,
  `charet pull`, settle the files named in the message, push, then merge again.
- A merge first builds what merging gives and opens the screens the change can reach (it waits
  while that builds). New errors there refuse it with the list: fix them on the branch, push,
  merge again.
- Changing `package.json`, lockfiles, `vite.config.*`, `tsconfig*.json`, `.npmrc` or `charet.json`
  needs an admin's approval (`charet branch approve`); until then the branch's previews use main's
  versions of those files, so a new dependency will not show up in `check` yet. Tell the user.
- `[charet] missing mock GET /api/…` means the screen calls an API no handler answers. Add the
  handler; a silent failure there would show the team a broken screen.
- `[charet] unmocked request reached the network: GET /api/…` is a `fetch` or XHR that went out
  although mocks answer in the page: mostly an API client the app made before the mocks started
  (the frame imports the app at the top). Import the app inside `renderScreen`
  (`references/project.md`, frame.tsx). A request you meant to let through (`passthrough()`) shows
  up the same way.
- Open the saved pictures and look at them before saying it works. A page can pass `check` and
  still be wrong (empty table, clipped dialog, wrong state).
- A shared component change makes many screens stale: `charet capture $(charet ls --stale)`.
- Add `--theme both` when the change can look different in dark mode.
- Push refused because someone pushed to the same branch first: a checkout is a git work tree, so
  `git stash && charet pull && git stash pop`, then push again. Their commit stays; yours lands on
  top.
- A version that broke something: `charet restore vN` makes a new version undoing it.

## Workflow 4 · Put an existing app on charet

`charet import <app>` copies the app into charet; then you add `.charet/` (charet-only dependencies,
vite config, frame, project, mocks) in the checkout, push (with `--merge`) and check. The app's own `package.json`
and lockfile stay exactly as in its repository. Follow `references/import.md` step by step — it
covers getting the SDK, the files to write and the usual failures.

## Workflow 4b · Show components (the Components view)

The web tool's tabs are Screens, Flows, Components, Branches, Versions and Code. Three of them show
what a project declares: **Screens** (a page in one state), **Flows** (the order of screens) and
**Components** (parts, each shown bare with example text); the others are about changes (Branches,
Versions) and source (Code). A **story** is one state of a component, as in Storybook; it is not a
flow. Write them in `.charet/project.tsx`:

- Make every combination of a component's props with `variants({ component, props, render, code })`
  instead of one story per state. The view gets a prop picker, a live preview and the code to copy.
- A component that needs data, context or a provider (a page header, a list row) is a feature
  component: give its stories `group: "Feature"` and a representative sample as the default. Never
  show it with empty props.
- A component that reaches the app's API client when it loads is imported lazily inside the story.

`references/project.md` has the rules and examples. Then push and check like any other change.

## Workflow 4c · The product repo and charet disagree

charet keeps its own copy of the app, so the two drift apart when someone edits one without the
other. Check before changing UI through charet (Workflows 1 and 2), and before editing the repo
directly. Importing and handing off are admin actions; a non-admin reports
the drift to an admin instead of fixing it.

**Repo ahead of charet** (someone changed the UI in the product repo):

```bash
cd <product repo>
charet handoff -p <project>                    # lists the repo's commits and uncommitted edits to imported files
charet import --update -p <project> --dry-run  # what would come in; a file both sides changed is refused
charet import --update -p <project>            # brings the repo's files in as an import version
```

The import advances main on the server only. Go back to the charet checkout and refresh it before
editing: a checkout made earlier still holds the old UI, and work started from it verifies the
wrong app. On main, `charet pull`. On a branch, `charet pull` fetches the branch's own head, so run
`charet branch catch-up` first (`charet status` says which one you are on). If catch-up names clashing
files, the branch did not move: reconcile those files against main, or start from a clean main
checkout, before editing. Do not continue on the old branch head. Then fix what the new code broke in `.charet/` (screens, mocks, stories),
push, `charet check`, and capture again so the team sees the real UI. When both sides changed the same file, do not choose
silently: show the user both changes and ask which one wins.

**charet ahead of the repo** (merged versions not in the repo yet): `charet handoff` lists them.
Export them (Workflow 5) before editing the repo directly, or the next import-and-export pair
fights over the same lines.

If you cannot reach the repo or the server, say you could not check and which side you did not
see. Do not call charet "up to date" because nothing was compared.

## Workflow 5 · Hand changes to the product repo (admins)

Merged versions live in charet until someone puts them in the product repo. Do it only when the
user asks, and follow the repo's own rules (work branch, its checks, a PR — never push to its main).

```bash
cd <product repo>                            # on a fresh work branch
charet handoff -p <project>                  # what is waiting, and whether the repo drifted
charet handoff export -p <project> --out /tmp/charet.patch
git apply /tmp/charet.patch                  # then the repo's checks, commit, PR
charet handoff mark vN -p <project>          # after the PR is merged, at that commit (--commit sha)
```

- Drift first: if the repo changed the imported files since the baseline, `charet import --update -p
  <project>` brings them in before you export, so the patch applies cleanly.
- `git apply` fails: the repo moved on in the same lines. Import (`--update`) and export again rather
  than editing the patch by hand.
- Mark only what is really in the repo's main; the mark tells the team those versions are shipped.

## Understanding structure

```bash
charet show <id|c12>              # details, connections in/out, flows, sources, open comments
charet connections --from <id>    # transitions (click · auto · error)
charet path <from> <to>           # how a user gets from one screen to another
charet map --format mermaid       # every page and screen with connections
charet source <screen>            # the files a screen renders from; `charet source <path[:line]>` reads one
charet url <id>                   # link to give the team
```

Connections and flows are written by hand and can lag behind the UI. When one looks wrong, check
the app code (router, buttons, links) and answer from the code. If you are already changing the
project, fix `.charet/project.tsx` too. If you were only asked a question, say what is wrong and
offer the fix (or leave a charet comment) instead of pushing a change nobody asked for — even a
draft branch shows up for the whole team.

## Ground rules

- Change comments and captures only through the CLI or the web tool; the server tracks revisions
  and pin positions.
- Leave your own questions or notes on the screen (`charet comments add <id> --at x,y|selector -m
  "…"`), not only in chat, so the team finds them where they look.
- Deleting a project (`charet delete`) cannot be undone; do it only when the user asks.
- Vague requests ("clean this up", "make it nicer"): tidying charet itself is fine — missing or
  stale captures, connections that disagree with the code. Changes to how the app looks or behaves
  are design decisions: list them, or leave them as comments on the screens, and let the user
  choose before you change the app.
- Report back with what changed, the branch link (or version, once merged), and `charet url <id>`
  links for the screens.

## References

- `references/cli.md` — every command and its output
- `references/project.md` — the `.charet/` contract: project.tsx rules (screens, flows, stories and `variants()`), setup kit, frame, static files
- `references/mocks.md` — writing MSW handlers
- `references/import.md` — bringing an existing app onto charet
