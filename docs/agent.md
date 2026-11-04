# The web agent

**English** | [한국어](agent.ko.md)

Members who are not developers can ask for a change in the web tool. A coding agent (Claude Agent
SDK or Codex SDK) makes it on a draft branch; the team looks at the draft and merges it like any
other ([workflow](workflow.md)).

## Turn it on

The web agent runs with the Docker install. Add the agent overlay:

```bash
docker compose -f compose.yaml -f compose.agent.yaml up -d --build
# public mode: docker compose -f compose.yaml -f compose.public.yaml -f compose.agent.yaml up -d --build
```

This adds two things: the **agent** service, which starts one container per run through this
machine's Docker socket, and the **charet-agent-run** image those containers use (the agents, the
`charet` CLI and the charet skill; about 1.5 GB).

Then give it an account in **Settings › Web agent** (admins). Per provider (Anthropic for Claude,
OpenAI for Codex) use an API key, a subscription sign-in, or both:

- **API key**: paste it and press **Check**. The provider says whether it works; only a key that
  works can be **Saved**. It is stored encrypted with the server key and only its last 4
  characters are ever shown.
- **Sign in with Claude / ChatGPT**: opens the provider's own login in a new window.
  - Claude: after signing in, the provider's page shows a code; paste it back into charet.
  - ChatGPT: after signing in, the new window ends on a page that does not load (its address starts
    with `http://localhost:1455/auth/callback`). That is expected: copy the address from the
    address bar and paste it into charet. This is the browser login of the provider's own command
    line tool, so it does not depend on a workspace's device code setting.
  - charet keeps the login encrypted, renews it before each run (and every 5 minutes in the
    background, when the one in use would not last until the next check and a run), and hands the
    run's container only the short-lived access token. The refresh token never leaves the server.
  - One account per provider, shared by everyone's web agent. One sign-in at a time: **Cancel**
    ends the one that waits and nothing else. **Sign in with another account** replaces the
    account when the new sign-in succeeds (a failed one leaves it). **Sign out** forgets the
    login here; the provider is not told.
- With both set, pick which one the agent uses.
- `.env` next to `compose.yaml`: `CHARET_ANTHROPIC_API_KEY=…` (Claude), `CHARET_OPENAI_API_KEY=…`
  (Codex), `CHARET_GOOGLE_API_KEY=…` (kept for later; nothing runs on Google yet). These win over
  the page and cannot be changed there.

The same page sets the default agent and a model per agent (empty is the agent's own default).

## Asking

- On a screen, switch the side panel to **Agent**. Click an element first if the request is about
  it: the screen, theme and the element (component chain, source location, text) go along with
  the request. Attach images (screenshots, sketches) with the image button, by pasting or dropping.
- **Work on**: a new draft (named after the request) or one of your drafts.
- The panel shows what the agent does as it goes: the tools it uses, what it says, and how it
  ended. **Stop** ends a run; whatever it already saved stays on the branch.
- On the branch page, the **Agent** tab keeps the conversation and takes the next request. Earlier
  requests and answers on the branch go along, so "a bit bigger" means what you think it means.
- Every save is a push to the draft, so the branch page's before/after shows the change. When it
  is right, **Request review**; an editor merges it.

One run works on a branch at a time; a second request waits. A few runs (default 3,
`CHARET_AGENT_MAX_RUNNING`) go at once on the server. A run that saves nothing on a new draft
leaves no empty draft behind.

## Costs

**Settings › Web agent** also lists each person's runs, tokens and cost. Claude reports its cost. For Codex
the cost comes from a price table you give charet-agent (`CHARET_AGENT_PRICES`, JSON of dollars
per million tokens, e.g. `{"gpt-5.5": {"input": 1.25, "output": 10, "cachedInput": 0.125}}`); runs
on a model without a price are counted as unpriced. There is no spending limit; set one with your
model provider.

## How a run is kept in

- Each run is its own container: all capabilities dropped, no privilege escalation, memory, CPU
  and process limits (`CHARET_AGENT_MEMORY_GB`, `CHARET_AGENT_CPUS`), and a time limit
  (`CHARET_AGENT_TIMEOUT_MIN`, default 30).
- Its network is internal. The only ways out are the agent service's relay to the server's
  **agent door** and an HTTPS proxy that opens tunnels only to the model APIs
  (`CHARET_AGENT_ALLOW_HOSTS`, default `api.anthropic.com,api.openai.com,chatgpt.com`). Package
  installs from the internet fail. If a run on a subscription cannot connect, add the host it
  reports to this list.
- At the agent door (`CHARET_AGENT_BIND`, not published on the host) a run's token works only while
  it runs, only for its project, and only to read the project, push to its own branch, check and
  capture. It cannot merge, comment, see other projects or reach the admin API. Its pushes show as
  *Name · agent*.
- The vendor key (or, for a sign-in, its short-lived access token) goes into the run's container
  (the agent needs it). Use a key made for charet so it can be revoked on its own. A run cannot
  renew a login by itself, so charet renews it first when it has less than the run time limit
  (default 30 minutes) plus 5 minutes left. A run that outlasts a login's own lifetime (ChatGPT:
  about an hour) still fails.
- The agent service holds the Docker socket, which is root on this machine. It is not reachable
  from run containers except through its two relays and its token-protected API.

## Settings

| Variable | Where | Default | |
|---|---|---|---|
| `CHARET_ANTHROPIC_API_KEY`, `CHARET_OPENAI_API_KEY`, `CHARET_GOOGLE_API_KEY` | `.env` | — | keys (or Settings › Web agent) |
| `CHARET_AGENT_MAX_RUNNING` | `.env` | 3 | runs at once |
| `CHARET_AGENT_NETWORK` | `.env` | `charet-agent-runs` | the runs' internal network |
| `CHARET_AGENT_TIMEOUT_MIN` | `.env` | 30 | longest run, in minutes, up to 35791 (the server renews logins to outlast it) |
| `CHARET_AGENT_MEMORY_GB`, `CHARET_AGENT_CPUS` | agent service | 4, 2 | per run |
| `CHARET_AGENT_PRICES` | agent service | — | Codex price table |
| `CHARET_AGENT_ALLOW_HOSTS` | agent service | model APIs | proxy allowlist |
