# ideas-tui

Rust + ratatui terminal client for the idea pipeline (`Claude_Code_Pipeline`). Laptop-only: it is not
deployed, hosted or part of `make deploy`. Install with `cargo install --path .`; binary `ideas-tui`.
See README.md for features, keys, config and the API contract.

## Commands

- `cargo test` — everything (unit, `tests/api.rs` with wiremock, `tests/flow.rs` end to end). Must pass.
- `cargo clippy --all-targets` — keep it clean.
- `cargo build --release`.

## Rules

- **Never edit the pipeline server, Caddy or the dispatcher from here.** This repo is only the client. The server
  side (the Caddy-skipped `/client` prefix with bearer `CLIENT_TOKEN`) is a separate idea. The contract this client
  assumes is documented in README.md; change code and README together.
- Never hardcode a host or token. Config comes from `~/.config/ideas-tui/config` (+ env overrides); `config.example` is the template.
- The real request/response shapes come from the pipeline repo (`server/server.js`, `public/app.js` in a sibling
  checkout). Types in `types.rs` are deliberately lenient (`#[serde(default)]`, nulls accepted): keep them that way.

## Architecture

- `api.rs`: typed client, no UI. Returns `ApiError` (`Unauthorized`, `Http{status,message}`, `Network`, `Decode`) with
  user-readable `Display`. Redirects are NOT followed (a login redirect means a wrong prefix).
- `state.rs`: pure logic (answer collection `TurnForm`, `job_actions`, `handoff_locked`, `can_*` gates, `Filter`). Put new
  rules here with unit tests, not in the key handlers.
- `app.rs`: `App` state machine. Network calls run on spawned tokio tasks and return as `Msg`; the UI never awaits.
  `Modal` holds the active dialog. `mut_seq` drops fetch results older than the latest mutation. Handoff edits live in
  `App.drafts` until Send (the server has no endpoint for saving them separately).
- `ui.rs`: rendering only, a function of `App`. Detail view builds a pre-wrapped line list so scrolling is exact.
- `main.rs`: `ratatui::init()` (raw mode + panic hook that restores the terminal) / `ratatui::restore()`; a thread feeds
  key events and a 100 ms tick into the same channel as API results.

## Gotchas

- Use `ratatui-textarea` (not `tui-textarea`): 0.7 of the latter pins an older ratatui. Crossterm is used through
  `ratatui::crossterm`, so there is no separate crossterm dependency to keep in sync.
- Text boxes submit with Ctrl+S; Enter is a newline there.
- Tests that touch `App::open_idea` or request paths need `#[tokio::test]` (they spawn tasks).
