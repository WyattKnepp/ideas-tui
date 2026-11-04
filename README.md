# ideas-tui

A terminal client for the idea pipeline (`Claude_Code_Pipeline`): capture ideas,
get grilled by Claude, write and edit the handoff brief, send it to a Claude
Code agent, and follow the deploy / droplet job. It is a second client for the
same server the web app uses, built with [ratatui](https://ratatui.rs).

It runs on your laptop only. It is not deployed, hosted or part of `make deploy`.

## Install

```sh
cargo install --path .      # installs the `ideas-tui` binary
```

## Config

`~/.config/ideas-tui/config` (or `$XDG_CONFIG_HOME/ideas-tui/config`), plain `KEY=value` lines:

| Key | Required | Meaning |
| --- | --- | --- |
| `SERVER_URL` | yes | Base URL of the pipeline, e.g. `https://ideas.example.com` |
| `CLIENT_TOKEN` | yes | Bearer token the server accepts for the client API (not the dispatcher's `AGENT_TOKEN`) |
| `API_PREFIX` | no | Path prefix Caddy skips the Google login for. Default `/client`. Empty means no prefix |

Environment variables with the same names override the file. See `config.example`;
then `chmod 600` the file (the app warns if group or others can read it). With a
missing or incomplete config the app prints what to create and exits non-zero.
No host or token is hardcoded anywhere.

## Keys

`?` shows this in the app; the footer always lists the keys valid right now.

**Idea list**: `j/k` or arrows move, `Enter` open, `n` new idea, `Tab`/`Shift+Tab` cycle the filter
(Open, Inbox, Grilling, Sent, Archived, All), `A` toggle archived, `r` refresh, `q` quit.

**An idea**: `j/k`/`Space`/`PgUp`/`PgDn` scroll (`G` end, `Home` top), `Esc`/`q` back.

| Key | Action | When |
| --- | --- | --- |
| `g` | Grill me | untouched inbox idea |
| `a` | answer Claude's questions | open structured turn |
| `t` | type a free-text reply | any grill turn |
| `C` | cancel the grill (confirm) | grilling |
| `w` | write handoff | grilling / review |
| `e` | edit handoff | handoff exists and isn't locked |
| `s` | send, then confirm | same |
| `u` | pull a queued idea back | `ready` |
| `S` / `X` / `B` | ship / discard / roll back the droplet job (confirm) | only where the job state allows it |
| `l` | show or hide the deploy log tail | deploy has a log |
| `z` | archive / restore | |
| `D` | delete (confirm) | `inbox` or `archived` only |

**Answering a turn**: `j/k` move, `Space`/`Enter` select (single = radio, multi = toggle), `Tab` next
question, selecting **Other…** lets you type free text (`Enter` finishes), `e` asks Claude to explain
the question, `Ctrl+S` submits all answers together, `Esc` closes (answers are kept for that turn).

**Text boxes** (new idea, reply, brief): `Ctrl+S` submits, `Esc` cancels. A failed submit keeps your text.

**Handoff editor**: `Tab`/`Shift+Tab` move between title, project, visibility and brief;
`Ctrl+N`/`Ctrl+P` cycle known projects (or type a new folder name); `Space` toggles
public/private for a new repo; `Ctrl+S` keeps the edits, `Esc` discards them. The server has no
endpoint for saving a handoff on its own, so edits are held locally (marked "edited locally")
and go to the server with **Send**, exactly like the web app.

**Send**: `1` laptop, `2` droplet (disabled for new projects), then `p` preview first / `v` go live,
`Enter`, and `y` to confirm.

The list refreshes every 10 s. An open idea refreshes every 2.5 s while it is `grilling`, queued, or
has a busy droplet job. Requests never block the UI.

## API contract the client expects

Every path below is under `SERVER_URL` + `API_PREFIX` (default `/client`). The routes, bodies and
responses are **identical to the web app's `/api/*`**, so `GET /client/api/ideas` mirrors `GET /api/ideas`.
Every request carries `Authorization: Bearer <CLIENT_TOKEN>`. The prefix must bypass Caddy's Google
login and be checked by the server itself. The client does not follow redirects: a redirect (for
example to a login page) is reported as an auth/prefix error.

Errors are `{ "error": "<message>" }` with a 4xx/5xx status. 401/403 show "check `CLIENT_TOKEN` and
`API_PREFIX`"; 409 messages (e.g. deleting a grilling idea) are shown as-is.

| Method and path | Body | Response |
| --- | --- | --- |
| `GET  /api/ideas` | | `[Idea]` |
| `GET  /api/ideas/:id` | | `Idea` |
| `POST /api/ideas` | `{text}` | `Idea` (status `inbox`) |
| `PATCH /api/ideas/:id` | `{archived: bool}` | `Idea` |
| `DELETE /api/ideas/:id` | | `{ok: true}`; 409 unless `inbox`/`archived` |
| `POST /api/ideas/:id/grill` | `{message}` (empty on a fresh idea starts the grill) | `Idea` |
| `POST /api/ideas/:id/cancel-grill` | `{}` | `Idea` (fresh `inbox`) |
| `POST /api/ideas/:id/elaborate` | `{qid}` | `{text}` |
| `POST /api/ideas/:id/handoff` | `{}` | `Idea` (status `review`) |
| `POST /api/ideas/:id/send` | `{title, project, visibility, brief, destination: "laptop"\|"droplet", mode?: "preview"\|"live"}` (`mode` only for the droplet) | `Idea` |
| `POST /api/ideas/:id/unsend` | `{}` | `Idea` (back to `review`) |
| `POST /api/ideas/:id/job/actions` | `{action: "ship"\|"discard"\|"rollback"}` | `Idea` |
| `GET  /api/projects` | | `{projects: [{name, description?}]}`, used to decide `isNew` and to cycle projects |

`Idea` (camelCase; every field is optional to the client and unknown fields are ignored):

```jsonc
{
  "id": "…", "text": "…", "status": "inbox|grilling|review|ready|dispatching|dispatched|failed|archived",
  "createdAt": "ISO", "updatedAt": "ISO", "busy": false, "grillSession": "…",
  "messages": [
    {"role": "me", "text": "…", "at": "…"},
    // structured Claude turn; a plain turn has only role/text/at
    {"role": "claude", "text": "comment", "at": "…", "done": false,
     "questions": [{"id": "q1", "text": "…", "type": "single|multi", "options": ["…"]}]}
  ],
  "handoff": {"title": "…", "project": "…", "isNew": false, "visibility": "private|public", "brief": "…"},
  "dispatch": {"destination": "laptop|droplet", "jobId": 1, "mode": "preview|live", "error": null,
               "workspaceLabel": "…", "tabLabel": "…", "agent": "…", "note": "…"},
  "deploy": {"status": "ok|failed", "commit": "…", "message": "…", "log": "…", "at": "…"},
  "job": {"id": 1, "status": "queued|running|testing|preview|deploying|live|rolling_back|rolled_back|discarded|failed|rejected",
          "mode": "preview|live", "note": "…", "branch": "…", "commit": "…", "preview_url": "…",
          "action": {"type": "ship|discard|rollback"} /* pending, or null */,
          "tests": {"ok": true, "summary": "…"}, "events": [{"at": "…", "status": "…", "message": "…"}]}
}
```

Behaviour the client relies on:

- A structured answer is sent as one `grill` message: for each question `"<question text>\n→ <picks joined by '; '>"`
  (with `Other: <text>` appended for free text), questions separated by a blank line. This matches the web app.
- `isNew` is decided from `GET /api/projects` (a project not in the list is new); the droplet refuses new projects.
- Job actions are offered exactly where the server accepts them: `ship` on `preview`; `discard` on `preview`
  or `failed`; `rollback` on `live`; none while `job.action` is pending.
- A droplet job ending in `failed`, `rejected`, `discarded` or `rolled_back` unlocks the handoff for a resend.
  `ready`, `dispatching` and other `dispatched` ideas are locked.
- Deploys (`idea.deploy`) never change `status`; they only drive the "Deployed" / "Deploy failed" pill and panel.

## Development

```sh
cargo test                 # unit tests, mock-server API tests, and an end-to-end key-driven flow
cargo clippy --all-targets
cargo build --release
```

Layout: `config.rs` (config file + env), `types.rs` (lenient wire types), `api.rs` (typed HTTP client, no UI),
`state.rs` (pure logic: answers, allowed actions, filters), `app.rs` (state machine and key handling),
`ui.rs` (rendering), `main.rs` (terminal setup and event loop). `tests/api.rs` tests the client against
`wiremock`; `tests/flow.rs` drives the real `App` through the whole pipeline against a stateful fake server.
