# charet CLI reference

All commands accept `--json` (machine-readable output), `-p <project>` (default: the checkout you
are in) and `--server <url>` (default: the checkout's server, else `CHARET_URL`, else
`http://127.0.0.1:5280`).

## Get started

| Command | Does |
|---|---|
| `charet setup [--yes]` | First-run guide: checks the server, offers to install this skill |
| `charet login [--token t]` | Signs in to a team server with the token an admin handed over (asks for it without `--token`). Not needed in local mode |
| `charet whoami` · `charet logout` | Who the CLI acts as · forget the saved token on this machine |
| `charet doctor` | Checks node, git, the server, charet-workspace, charet-render, cookie separation (exit 1 on failure) |
| `charet serve [--server-only]` | Runs charet-server with charet-workspace and charet-render |
| `charet skill install [--claude] [--codex]` | Installs this skill for Claude Code (`~/.claude/skills/charet`) and / or Codex (`~/.codex/skills/charet`) |

## Projects and versions

| Command | Does |
|---|---|
| `charet projects` | Lists projects |
| `charet delete <project> [--yes]` | Deletes a project with its versions, captures and comments (no undo; asks for the key). Only when the user asks |
| `charet init <key> [dir] [--template recommended]` | Creates a project from a template and checks it out |
| `charet import [path] [--key k] [--include dir]… [--dry-run]` | Imports an existing app (and the workspace packages it uses) |
| `charet checkout <project> [dir] [--rev vN] [--branch N]` | Downloads main (or a version, or one branch) into a folder |
| `charet status` | Its branch (if any), base, whether main moved, local changes |
| `charet push [-m msg] [--merge]` | From a checkout of main: starts a branch (draft) titled `msg` with the changes and the checkout follows it. From a branch checkout: adds to it (fails if someone pushed to it first: `charet pull`, then push). `--merge` merges it at once and brings the checkout back to main |
| `charet pull` | Updates the checkout to its branch's head or main (refuses with unpushed changes); a merged or archived branch hands the checkout back to main |
| `charet dev` | Runs the app's vite dev server in the checkout |
| `charet versions` · `charet diff <from> [to]` | Version list (kind import / merge / restore) · changed paths |
| `charet restore vN` | A new version undoing vN (clashes with later changes are refused) |

## Product repo (admins)

| Command | What it does |
|---|---|
| `charet handoff [status] -p <project>` | Baseline (last import or handoff, with its repo commit), the files main changed since then, and — run inside the product repo — the repo's own commits and uncommitted edits to the imported files since the baseline (drift). `charet status -p <project>` outside a checkout shows the same |
| `charet handoff export [--since vN] [--with-mocks] [--out f]` | The patch of main since the baseline (or vN), for `git apply` in the product repo. Without `--out` the patch alone goes to stdout. `.charet/` and `charet.json` are left out unless `--with-mocks` |
| `charet handoff mark vN [--commit sha]` | Records vN (and every version before it) as in the product repo at that commit (default: HEAD of the repo you run it in). The web tool shows it on Versions |
| `charet import --update [path] -p <project> [--dry-run]` | Brings the product repo's files in again as an import version. They are laid on the baseline and joined into main, so merged work not handed off yet stays; `.charet/` stays as it is. A file both sides changed is refused |
| `charet branches [--all]` | Drafts and branches in review (`--all`: merged, archived): `#n status author title [clash: files]` |
| `charet branch [n]` | One branch: status, head, clash, link, changed files, history (defaults to the checkout's branch) |
| `charet branch review [n]` | Asks the team to review a draft (author or admin) |
| `charet branch approve [n]` | Approves the branch's protected-file changes (admin) |
| `charet branch merge [n]` | Builds and checks what merging gives, then merges into main as a new version; refused when files clash, protected files wait for approval, or the check finds new problems (each named in the message) |
| `charet branch catch-up [n]` | Takes the newest main into the branch; clashes are marked, then `charet pull` |
| `charet branch archive\|reopen\|duplicate [n]` | Close without merging · bring back as a draft · copy as your own draft (drafts are their author's) |

## Structure

| Command | Output |
|---|---|
| `charet ls` | Screens (id, title, page, capture state, open comments), stories, flows |
| `charet ls --stale` | Ids that need a capture (never captured or stale), one per line |
| `charet show <id>` | Screen: page, route, size, note, from/to connections, flows, sources, issues, open comments. Also pages, flows, components (`Button`), comments (`c12`) |
| `charet url [id]` | Web tool link |
| `charet connections [--from s] [--to s] [--page p]` | `from → to "label" (kind)` |
| `charet path <from> <to> [--sheet f.jpg]` | Shortest chain of connections; exit 1 when there is none |
| `charet map [--format text\|mermaid]` | Pages, screens and their outgoing connections |
| `charet flows` · `charet flow <id> [--sheet f.jpg]` | Flows · numbered steps with capture state, planned steps marked |
| `charet sheet <id>… --out f.jpg [--theme dark]` | Captures side by side (grey = planned or not captured) |
| `charet source <path[:line]> [--context n]` | A file of main with line numbers (`>` marks the line). Paths may be relative to the app |
| `charet source <screen\|story>` | The files that screen rendered from (needs a capture) |

## Check and capture

`check` runs on the checkout's branch (else main); `capture` only on main, since captures are what everyone sees. `--rev vN` picks another; both wait for the preview build.

| Command | Does |
|---|---|
| `charet check [id…] [--theme light\|dark\|both]` | Opens pages, reports page errors, console errors, missing mocks and requests that reached the network instead of a mock. Nothing is stored. Exit 1 on problems. It does not judge how the page looks: a login page or an error screen is not a problem to it |
| `charet capture [id…] [--theme …] [--force] [--out dir]` | Stores captures the web tool shows and re-anchors comment pins. Already captured at this commit = `unchanged` unless `--force`. `--out` saves `<id>.jpg` / `<id>.dark.jpg`. On a branch (a branch checkout, or `--rev b<n>`) with `--out`: only the pictures of that branch, nothing stored |

No ids = every screen and story.

## Comments

| Command | Does |
|---|---|
| `charet comments [--all\|--resolved] [--on id] [--branch N]` | Lists open comments (exit 1 while any listed one is open); `--branch N` lists the comments reviewers left on branch N's screens instead of main's |
| `charet comments show c12 [--out dir]` | Body, box, element, components, source, replies; saves the snapshot image and prints its path |
| `charet comments add <id> --at x,y\|selector -m text [--theme dark]` | Pins a comment to the smallest element at the point (or the selector) |
| `charet comments reply c12 -m text` | Replies |
| `charet comments resolve c12 [-m note]` · `reopen c12 [-m note]` | Resolves · reopens |
| `charet inbox [--all]` | Your notifications on a team server (new comments, answers in your threads), each with a link; unread only unless `--all` |
| `charet inbox read [id...]` | Marks those notifications (or all) read |
