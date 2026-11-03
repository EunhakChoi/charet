# Running charet locally

**English** | [한국어](local.ko.md)

charet runs as three services: **charet-server** (API, web tool, previews), **charet-workspace**
(installs and builds project code) and **charet-render** (Chromium: captures and checks). No address
is trusted: the owner comes in with their **admin link**, members with their token, everyone else is a
visitor. In local mode the ports stay on this machine's loopback; others come in through sharing.

## With Docker (recommended)

Needs Docker with Compose v2.

```bash
git clone https://github.com/EunhakChoi/charet.git
cd charet
./start.sh
```

`start.sh` runs `docker compose up -d`, waits for the server and prints the **admin link**
(`http://localhost:5280/?t=<token>`); right after install it also opens it in the browser. Open it
once — `localhost`, not `127.0.0.1` — and the browser stays signed in as the owner, the first admin.
Run `./start.sh` again any time to start charet and see the link; `charet setup` (signs the CLI in) and
`docker compose exec server charet-server admin-token` (the token alone) show it too. Opening
**http://localhost:5280** without it makes you a visitor (by default: look only); until the owner
first comes in, the page there says where the link is. Plain `docker compose up -d` works as well —
the server prints the link to its log (`docker compose logs server`) on its first start. On Windows,
run `start.sh` from WSL or Git Bash, or use the plain commands.

Previews are served from `127.0.0.1:5281`, a different site, so a project's code can never read the
tool's cookies.

The first build of the images takes a while (Rust and the web tool are compiled).

| Setting (environment or `.env`) | Default | |
|---|---|---|
| `CHARET_PORT` | `5280` | web tool and API |
| `CHARET_PREVIEW_PORT` | `5281` | previews |
| `CHARET_UPDATE_CHECK` | `true` | daily check for a new release (the only outside call) |
| `CHARET_MERGE_CHECK` | `true` | build and open what a merge gives before main moves (`false` for a server without previews) |
| `CHARET_TZ` | `UTC` | time zone of rendered screens (e.g. `Asia/Seoul`) |
| `CHARET_DATA_DIR` | (volume `charet_data`) | a folder on this machine for projects, comments and captures |
| `CHARET_USER` | `charet` | user the server runs as; with `CHARET_DATA_DIR`, the folder's owner (`uid:gid`) |

Data lives in Docker volumes:

| Volume | Holds |
|---|---|
| `charet_data` | projects (git), versions, comments, captures, the preview signing key |
| `charet_workspace` | dependency caches and builds (can be dropped; rebuilt on demand) |
| `charet_secrets` | the token the services use with each other |

```bash
docker compose logs -f server        # logs
docker compose up -d --build         # update after `git pull`
docker compose down                  # stop (data is kept)
docker compose down -v               # stop and delete all data (not a CHARET_DATA_DIR folder)

# back up projects, comments and captures
docker run --rm -v charet_data:/data -v "$PWD":/backup debian:trixie-slim \
  tar -czf /backup/charet-data.tgz -C /data .
```

To keep the data in a folder of your choice instead of a volume, create the folder and point
`CHARET_DATA_DIR` at it, with `CHARET_USER` set to its owner (git refuses a repository owned by
someone else):

```bash
mkdir -p ~/charet-data
printf 'CHARET_DATA_DIR=%s\nCHARET_USER=%s\n' ~/charet-data "$(id -u):$(id -g)" >> .env
docker compose up -d
```

## Team and sharing

**Team** (in the header) lists the members. Add someone with a name and a role (admin, editor,
commenter, viewer) and hand over their **token**, or the link that carries it
(`…/?t=<token>`): it signs their browser in once and stays signed in. Tokens are shown and copied
from the Team page at any time; **New token** stops the old one. When the same token signs in on a
second browser, the first one is signed out (the CLI is not). People without a token are
**visitors**: they look at the team's projects under a made-up name. *Visitors* on the Team page sets
whether they may look, also comment, or need a token; a project can set its own level under
**Access**.

The same rules hold on every address, `localhost:5280` included. The owner (marked *Owner*) is the
first admin and cannot be removed or demoted.

To let other machines in, charet has a second port, the **remote door** (`127.0.0.1:5284`,
`CHARET_REMOTE_BIND`). In **Team › Sharing**, pick how others reach it:

- **Tailscale network** — people on your tailnet (Tailscale, or Headscale over plain HTTP inside
  the tailnet). charet runs `tailscale serve` and shows the machine's name.
- **Tailscale Funnel** — anyone with the address, at the machine's `ts.net` name. If Funnel is not
  enabled for the machine, the page shows the admin-console link.
- **ngrok** — the server runs an ngrok tunnel itself (no ngrok program needed). Paste the authtoken
  from your ngrok dashboard (kept on the server, never shown again) and, if you have one, your
  ngrok domain so the address stays the same. `CHARET_NGROK_AUTHTOKEN` also works.
- **My address** — a tunnel or reverse proxy you set up (ngrok, cloudflared, nginx…) pointing at
  `127.0.0.1:5284`, not 5280: through the remote door previews come on the same address, sandboxed.
- **Local network** — the remote door also listens on this machine's LAN address, over plain HTTP.
  For trying things out: links show the IP, and tokens travel unencrypted. (In Docker, publish
  port 5284 instead.)

Links (member links, notifications) use the shared address while sharing is on. `charet doctor`
says whether it answers. The Tailscale choices need the `tailscale` CLI where charet-server runs
(`CHARET_TAILSCALE`, `CHARET_TAILSCALE_SOCKET`). In Docker use **My address** and run
`tailscale serve --bg http://127.0.0.1:5284` (or `tailscale funnel …`) on the host. Previews come
through the same address, sandboxed. For a server that is only ever used by a team, see
[public mode](public.md).

## The CLI

The CLI is not published yet; run it from the clone (Node.js 24+, pnpm 12):

```bash
pnpm install
alias charet="node $PWD/apps/charet-cli/src/main.ts"
charet doctor                          # checks node, git, the server and its services
charet setup                           # signs in as the owner, prints the admin link; offers the skill
charet init demo                       # a new project from the recommended template
charet import ../my-app                # or an existing app (with the workspace packages it uses)
```

An alias exists only in your own shell. A coding agent's shell has none, and often not your `node`
on its `PATH` either (nvm, fnm). For it, put a small script on the `PATH` that records where `node`
and the clone are now:

```bash
mkdir -p ~/.local/bin
printf '#!/bin/sh\nexec "%s" "%s/apps/charet-cli/src/main.ts" "$@"\n' "$(command -v node)" "$PWD" > ~/.local/bin/charet
chmod +x ~/.local/bin/charet                # ~/.local/bin has to be on the PATH
```

`CHARET_URL` points the CLI at another server (default `http://127.0.0.1:5280`).

## Without Docker

Needs Rust (stable), Node.js 24, pnpm 12, git and Chromium (or
`pnpm --filter charet-web exec playwright install chromium-headless-shell`).

```bash
make setup
cargo build --release -p charet-server -p charet-render
pnpm --filter charet-web run build
node apps/charet-cli/src/main.ts serve    # server + workspace + render, Ctrl-C stops all
```

Data goes to `~/.charet` (`CHARET_HOME`).

## Security notes

- Every request needs a member's token (or session) to do more than visitors may. Keep the ports on
  loopback anyway and share through the remote door; for a server only a team uses see
  [public mode](public.md).
- Project code (dependency installs, builds) runs only in charet-workspace, with install scripts
  turned off and a minimal environment. Previews run in the browser on their own origin with a
  signed, short-lived token.
- Changes to files that run code at install or build time (package manifests, lockfiles, build
  config) wait for an admin's approval; until then previews use `main`'s versions
  ([workflow](workflow.md#merging)).
- The only outside call is the daily release check (`CHARET_UPDATE_CHECK=false` turns it off). No
  code, screens or usage data are sent.

## Troubleshooting

- `charet doctor` names what is wrong and how to start it.
- Previews stay blank in the tool: open the tool as `http://localhost:<port>`, not `127.0.0.1`.
- Port in use: set `CHARET_PORT` / `CHARET_PREVIEW_PORT` and run `docker compose up -d` again.

## Upgrading from the working name `board`

- The server takes over an old `board.db` in the data folder on its first start.
- Environment variables are now `CHARET_*` (was `BOARD_*`): rename them in your `.env`.
- The Docker Compose project is now `charet`, so its volumes are new. Keep data in a folder
  (`CHARET_DATA_DIR`) or copy the old `board_data` volume over before switching.
- In each project: rename `.board/` to `.charet/`, `@board/sdk` to `@charet/sdk` and the vite plugin
  `board()` to `charet()`, then `charet push`.
