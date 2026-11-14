# AGENTS.md

MatterLite is a native Rust desktop client for Mattermost servers (Windows x64, Linux x64,
macOS arm64), built on WarpUI, the UI framework from the Warp terminal. The design document
is `docs/MatterLite.md`; the Mattermost webapp/protocol reference is `docs/WebApp.md` (written
from the Mattermost server/webapp checkout at `C:\Projects\Libs\mattermost`). Milestone
progress is tracked in `docs/M0.md`, `docs/M1.md` (and later milestone files).

## Commands

- `cargo run -p matterlite` - run the app
- `cargo run -p warpui --example list` - WarpUI smoke test (window + scrolling list)
- `cargo run -p mm_cli -- <server-url> <ping|bootstrap|events> [--login <user>]` - protocol
  harness against a real server (`MM_TOKEN` or a password login; never prints the token)
- `cargo test --workspace` - tests
- `cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings` - must pass
- `script/provenance` - checks provenance headers, `PROVENANCE.md` and that nothing depends on
  the Warp reference checkout

## Repository layout

- `crates/matterlite` - the binary (WarpUI app, views, keymaps, settings, platform glue)
- `crates/mm_*` - Mattermost logic (model, REST, WebSocket, store, cache, sync, render). No
  WarpUI dependency; testable headless.
- `crates/ml_*` - MatterLite app-level code, partly extracted from Warp's `app/` crate.
- Framework crates keep Warp's names (`warpui`, `warpui_core`, `warp_editor`, `sum_tree`, ...)
  so their paths map 1:1 to the Warp reference.

## Warp reference code

- `C:\Projects\Libs\warp-terminal` (commit 5bb7bcc6a) is READ-ONLY reference material.
  Read it freely to understand framework code, find existing solutions, or copy more pieces.
- Never add a Cargo dependency (path or git) on the warp-terminal repository. MatterLite
  must build without it.
- To reuse Warp code: (1) commit a verbatim copy (`import(warp): <path> @ <sha>`), not yet
  compiled; (2) adapt in follow-up commits (`adapt(<crate>): ...`); (3) add the SPDX +
  "Derived from" header; (4) add or update the row in PROVENANCE.md.
- Copy only what is needed. Delete what becomes unused; don't keep Warp features "for later".
- Code from crates/warpui and crates/warpui_core stays MIT; everything else is AGPL-3.0-only.
- When a derived file is rewritten beyond recognition, set its PROVENANCE status to
  `rewritten`, but keep the row.
- Framework crates keep Warp's names (warpui, warpui_core, warp_editor, sum_tree, ...) so that
  paths map to the reference. App-level code lives in ml_* crates and Mattermost logic in mm_*.
- To port a later Warp fix: diff the Warp file between the old and new reference commits,
  apply by hand as `port(warp): <what> from <sha>`, and update the commit column in
  PROVENANCE.md.

## WarpUI rules (carried over from Warp)

- Entity-Handle system: a global `App` owns all views and models; views hold `ViewHandle<T>`
  references; `AppContext`/`ViewContext`/`ModelContext` give temporary access.
- `Element`s describe layout (Flutter-inspired) and are rebuilt on every render; state that
  must survive a render lives in the `View`.
- Mouse input uses `MouseStateHandle`: create it once during construction and clone it into
  elements. An inline `MouseStateHandle::default()` while rendering means no mouse
  interactions work.
- Network and disk work never runs on the UI thread. It runs on the Tokio runtime owned by
  `mm_sync`, and results come back over channels (`ModelContext::spawn_stream_local`).

## Coding style

- If a function takes a context parameter (`AppContext`, `ViewContext`, `ModelContext`), name
  it `ctx` and put it last, unless the function also takes a closure, which goes last.
- Remove unused parameters completely rather than prefixing them with `_`.
- Prefer inline format arguments (`format!("{name}")`).
- Avoid wildcard `_` arms in matches on our own enums; exhaustive matches catch new variants.
- Avoid unnecessary type annotations and long path qualifiers; import at the top of the file
  (cfg-guarded branches may import locally).
- Comments explain why, not what. No comments narrating edits ("this used to ...").
- Unit tests go in `<file>_tests.rs`, included at the end of the module with
  `#[cfg(test)] #[path = "<file>_tests.rs"] mod tests;`.
- Use `command::blocking::Command` / `command::r#async::Command`, never
  `std::process::Command` (console window flash on Windows).
