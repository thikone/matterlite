# MatterLite — a native Rust Mattermost client for Windows x64, Linux x64 and macOS arm64

MatterLite is a lightweight native desktop client for Mattermost, written in Rust. It
reuses the UI framework and selected crates from the Warp terminal fork at
`C:\Projects\Libs\warp-terminal`.

This document records the **design decisions**, the **Warp reuse plan**, the
**architecture**, and **how to build and package** the app on all three platforms. Protocol
and behaviour details (REST, WebSocket, unreads, rendering rules) are in
[`WebApp.md`](WebApp.md) — this document refers to its sections as `WebApp §N`. Features
that go beyond the webapp have their own design documents:
[`MatterLite-Collaborate.md`](MatterLite-Collaborate.md) (collaborative drafts),
[`MatterLite-ComposerVim.md`](MatterLite-ComposerVim.md) (Vim mode in the composer).

> Planned home: a new repository at `C:\Projects\Libs\matterlite`. This file lives next to
> `WebApp.md` until that repository exists, then moves there.

---

## Contents

1. [Decisions](#1-decisions)
2. [Licensing](#2-licensing)
3. [What to reuse from Warp](#3-what-to-reuse-from-warp)
4. [Bringing in Warp code (copy and adapt)](#4-bringing-in-warp-code-copy-and-adapt)
5. [Workspace layout](#5-workspace-layout)
6. [Architecture](#6-architecture)
7. [Networking layer](#7-networking-layer)
8. [State, sync and the SQLite cache](#8-state-sync-and-the-sqlite-cache)
9. [Storage locations and the token](#9-storage-locations-and-the-token)
10. [UI with WarpUI](#10-ui-with-warpui): layout, chat tabs, tool panel, History and Files panes, search bar
11. [Themes and Appearance](#11-themes-and-appearance)
12. [Message rendering](#12-message-rendering)
13. [Composer (Warp editor)](#13-composer-warp-editor)
14. [Files: viewing and editing with Warp's viewers](#14-files-viewing-and-editing-with-warps-viewers)
15. [Building](#15-building)
16. [Packaging and release](#16-packaging-and-release)
17. [Testing](#17-testing)
18. [Roadmap](#18-roadmap)
19. [Risks](#19-risks)

---

## 1. Decisions

| Topic | Decision | Alternatives considered |
|---|---|---|
| License | **AGPL-3.0-only** | MIT/Apache (would allow only `warpui`/`warpui_core` and force rewriting their AGPL deps); proprietary |
| UI toolkit | **WarpUI** (`warpui` + `warpui_core` from Warp) | Iced, Slint, Tauri (webview) |
| Async runtime | **Tokio on a dedicated background thread**; UI talks to it over channels | WarpUI's executor + `async-compat` (as Warp's `http_client` does) |
| MVP scope | **Core chat**: login, teams, channels, DMs/GMs, read/send, live updates, unread badges, Markdown, emoji | + threads/reactions, files/previews, notifications/search (later milestones) |
| Warp reuse | **Copy and adapt, tiered** (§4). `..\warp-terminal` is a read-only reference; the MatterLite repo is self-sufficient and holds only needed code. Framework crates are copied whole then pruned (Warp names kept); app-level pieces are extracted file by file into `ml_*` crates; helpers are inlined or rewritten. Provenance via `PROVENANCE.md`, per-file headers, import/adapt commit pairs and `AGENTS.md` rules | Pull-on-demand only; whole-crate copy of everything; re-implement from reference; path/git deps; `vendor/` split |
| Markdown | **comrak** (GFM AST) + own Mattermost pass + own WarpUI renderer | Warp `markdown_parser`; pulldown-cmark |
| Composer | **Warp's terminal-input editor** (`EditorView` from `app/src/editor`, extracted into `ml_editor`, §13) | Warp's `editor` crate (`warp_editor`, the editor behind Warp's code and notebook views; kept for the file viewers, §14); WarpUI `text_input` |
| Themes | **Warp's theme system and `Appearance`**, including all Warp built-in themes, **plus new themes matching Mattermost's** (Denim, Sapphire, Quartz, Indigo, Onyx) and the user's server-side Mattermost theme | Mattermost themes only |
| Window layout | **Warp workspace layout**: tab bar of recent chats (focus-or-open, LRU cap, pinning); left **tool panel** with 6 tabs (Chats, Activity, Channel, History, Files, Search); **right panel** for threads; split panes in a tab after the MVP | Webapp-style fixed sidebar + right-hand sidebar |
| Channel list | **Tool panel "Chats" tab** (team switcher on top) | Permanent column; vertical tabs only |
| New panes | **History** (active chat by date, collapsible groups, search incl. attached files, click-to-navigate, filter-the-chat mode) and **Files explorer** (all files grouped by chat, collapsed by default), both using the **search bar from Warp's Session history pane** | — |
| File viewing/editing | **Reuse Warp's file viewers**: code editor, Markdown/Jupyter viewer, image lightbox, and the `warp_files` load/save model, for files sent and received in chat (from milestone M5) | Open everything in the system default app |
| Local storage | **SQLite cache** (diesel + embedded migrations, like Warp) in `data/` next to the binary if it exists, else the per-user app-data dir (`warp_core::paths` convention) | Settings-only; plain portable folder |
| Data fetching | **Eager sync** (§8.5): the local DB is the primary read source. A background scheduler fills it with all channels' recent posts, people, emoji, threads, then a history backfill window (default 180 days), politely rate-limited | Webapp-style lazy loading (only unread channels prefetched) |
| Attachments | **Downloaded when the message is received or cached**, then text-extracted into a local full-text index (§8.6). File search runs on the actual files and reports its coverage (searched / pending / skipped) | Download on click; server-side file search only |
| Token storage | **Warp `warpui_extras::secure_storage`**: Keychain (macOS), Secret Service (Linux), DPAPI-encrypted file in the data dir (Windows) | Plain file in `data/`; never persist |
| Login methods (MVP) | **Password + MFA** | Personal access token; SSO via system browser (later) |
| Servers | **One active server in MVP**, store keyed by `ServerId` so multi-server is cheap later | Multi-server from day one |
| Code highlighting | **Warp `syntax_tree` + `languages`** (tree-sitter via `arborium`) | syntect; none |
| Emoji | **Embedded image set** (emoji-datasource PNGs, same as the webapp); custom emoji from `/emoji/{id}/image` | System color emoji font |
| Packaging | **Portable archives**, like the Warp fork: Windows zip, Linux tar.gz, macOS arm64 tar.gz (ad-hoc codesigned) | Installers (Inno Setup, .dmg, deb/rpm/AppImage); both |
| Targets | **Windows x64, Linux x64, macOS arm64 only** (no Intel macOS, no universal binary) | — |
| Linux support | **Whatever Warp supports**: X11 and Wayland via Warp's winit fork, Vulkan rendering with wgpu's GLES/EGL backend as fallback, D-Bus notifications, Secret Service | — |
| Proxy | **No proxy settings UI for now**; reqwest picks up the system/env proxy (`HTTPS_PROXY`, etc.) automatically | Proxy settings page |
| Toolchain | **Rust 1.98.1**, pinned to match the Warp-derived code (Warp fork uses 1.98.1) | — |

---

## 2. Licensing

The Warp fork is dual-licensed per crate:

- `warpui`, `warpui_core` — **MIT** (`LICENSE-MIT`).
- Everything else, including crates those two depend on (`warp_errors`, `warp_util`,
  `sum_tree`, `markdown_parser`, `string-offset`, `settings_value`) — **AGPL-3.0-only**
  (`license.workspace = true` → `AGPL-3.0-only`).

Because MatterLite is AGPL-3.0, any Warp code can be copied and adapted. Obligations to keep:

- Keep Warp's copyright notice in every derived file (header, §4.4), ship `LICENSE`
  (AGPL-3.0) and `LICENSE-MIT`, and keep files derived from `warpui`/`warpui_core` under
  MIT (SPDX header). The source commit of each piece is recorded in `PROVENANCE.md`.
- Distribute source with binaries (link the repository in the About dialog and the release
  notes). AGPL §13 (network use) doesn't change much for a desktop client, but the
  source offer must be visible.
- Mattermost itself is only accessed over its public API; no Mattermost code is copied.
  The emoji images come from `emoji-datasource` (MIT); check the licence of the image set you
  choose (the Apple set has restrictive terms; prefer the Google/Twitter set, as the
  webapp's build allows).
- Run `cargo deny check licenses` in CI (Warp has a `deny.toml` to start from).

---

## 3. What to reuse from Warp

All paths are relative to `C:\Projects\Libs\warp-terminal`. *How* the code is brought in
(whole-crate copy, selective extraction, or inline/rewrite) is defined in §4.2; "as-is"
below means "copied whole, then pruned and adapted as needed".

### 3.1 Reuse largely as-is

| Crate | What it gives MatterLite |
|---|---|
| `crates/warpui_core` | App/Entity model (`App`, `AppContext`, `ModelHandle`, `ViewHandle`), actions, keymaps, elements (flex, stack, scrollable, `uniform_list`, `viewported_list`, `formatted_text_element`, image, hoverable, tooltip…), text layout, image cache, clipboard, notification types, `ui_components` (button, checkbox, switch, text_input, tooltip, segmented control, progress bar…) |
| `crates/warpui` | Platform layer: macOS (Cocoa + Metal), Windows/Linux (Warp's `winit` fork + `wgpu`), font loading (`cosmic-text`, `font-kit`, DirectWrite), OS notifications (`notify-rust`, `tauri-winrt-notification`, macOS UserNotifications), file dialogs (`native-dialog`), `open` URLs, global hotkeys |
| `crates/warpui_extras` | `secure_storage` (Keychain / Secret Service / DPAPI) and `user_preferences` (file-backed key/value) |
| `crates/sum_tree` | B-tree with summaries — used by the editor; also a good fit for the timeline (§10.4) |
| `crates/string-offset` | Byte/char offset newtypes used throughout the editor |
| `crates/warp_errors` | Error-reporting helpers imported throughout `warpui_core` (copied whole; tiny) |
| `crates/warp_util` | Only `path`, `standardized_path`, `content_version`, `user_input`, `file`, `file_type` are used; Tier C (§4.2), so `crates/command` isn't needed |
| `crates/markdown_parser` | Required by `warpui_core`/`editor` (MatterLite's message Markdown uses comrak, §12) |
| `crates/settings`, `settings_value`, `settings_value_derive` | Typed settings with a TOML backing file and change notifications |
| `crates/warp_features` | Feature flags; folded into `warp_core::features` with only MatterLite's flags (Tier C) |
| `crates/editor` | `warp_editor`: the editing model behind Warp's code and notebook views, used by the file viewers (§14). Not the composer, which uses `ml_editor` (§3.2, §13) |
| `crates/vim` | Required by `editor` (optional Vim mode) |
| `crates/asset_cache` | Async asset loading from URL/data URI with caching — reuse for avatars, emoji and images |
| `crates/ipynb_parser` | Required by `editor` (can be stubbed later) |
| `crates/languages`, `crates/syntax_tree` | Tree-sitter grammars (`arborium`) and highlighting queries for code blocks |
| `crates/warp_logging` | Log setup with rotation; rewritten as a small `ml_logging` (Tier C) |
| `crates/fuzzy_match` | Fuzzy matching for the channel switcher (Ctrl/Cmd+K) and autocomplete |
| `crates/warp_files`, `crates/watcher` | `FileModel`: central async load/save of files, change notifications and file watching. It backs the file viewers (§14). |
| `crates/asset_macro` | `bundled_asset!`/`bundled_or_fetched_asset!` macros used by the theme definitions (e.g. Phenomenon's background image). Point it at MatterLite's `assets/` folder. |
| `mermaid_to_svg` (git dep) | Mermaid diagrams in Markdown files and messages (used by `editor` and the notebook viewer) |

### 3.2 Reuse after trimming

| Crate | Keep | Remove / stub |
|---|---|---|
| `crates/warp_core` | `features` (`FeatureFlag`), `ui::{appearance, theme, icons, color}`, `paths` (incl. `portable_root()`), `platform`, `safe_log`, `errors`, `sync_queue`, `interval_timer` | Terminal/agent/telemetry/session code (`cli_agent_protocol`, `semantic_selection`, `telemetry`, `execution_mode`, `command`), the dependency on `command-corrections` (git), `websocket` if unused. Replace `ChannelState`-derived directory names with constants (`"MatterLite"`). |
| `crates/ui_components` | `dialog`, `button`, `switch`, `tooltip`, `lightbox` (image viewer), `keyboard_shortcut` | Warp-specific styling hooks |
| `app/src/editor/` (+ `settings/editor.rs`, `settings/select.rs`, small `util` helpers) → new crate `ml_editor` (done in M0) | `EditorView` in one file, `EditorView.rs`: single-line and multi-line editing, CRDT buffer with undo, multiple cursors, soft wrap, IME, placeholders, decorators; cursor and selection settings. Details: [`EditorView.md`](EditorView.md) | Vim mode, voice input, AI context menu and image attachments, autosuggestions, command x-ray, smart select, fold commands, shared-session peers and remote edits (to restore for collaborative drafts: [`MatterLite-Collaborate.md`](MatterLite-Collaborate.md) §8), terminal-only actions |
| `app/src/themes/` → new crate `ml_themes` | `default_themes.rs` (all built-in `WarpTheme`s and their ANSI palettes), `theme.rs` (`ThemeKind`, custom YAML themes from `themes_dir()`, `RespectSystemTheme` light/dark pairs), `theme_chooser.rs` (picker UI), optionally `theme_creator*` (builds a theme from an image) | Referral-reward themes (`SentReferralReward`, `ReceivedReferralReward`), telemetry, Warp Drive/cloud hooks. Then add the Mattermost themes (§11). |
| `app/src/workspace/view/{left_panel.rs, right_panel.rs, vertical_tabs*}`, `app/src/tab.rs`, `app/src/pane_group/`, `app/src/undo_close/` → new crate `ml_workspace` | Toolbelt + tool panel (`ToolPanelView` with MatterLite's 6 variants, resizing, persisted state), right panel shell (resizable/maximize/close), tab bar (`TabBarState`, `TabComponent`, pin/colour/rename/drag/close/reopen, `ActivateTabByNumber`), vertical tabs, pane groups (for split panes after the MVP) | Terminal sessions, code review contents of the right panel, agent management, Warp Drive, tab configs/launch configs, telemetry. `workspace/view.rs` is about 29k lines: **copy patterns and the needed parts, don't port it whole**. |
| `app/src/workspace/view/session_history.rs` → `ml_workspace::search_bar` | The search field + toggle bar + `Matcher` (§10.10) as a reusable `SearchBarView`; the row list and selection patterns as the model for the History pane | Block-list/terminal coupling, block menus, capture commands |
| `app/src/code/file_tree/` + `app/src/code/icon.rs` | Tree view (collapsible groups, keyboard navigation, context menu) and file-type icons for the Files explorer (§10.9) | File-system watching, repository detection, file editing operations in the tree |
| `app/src/code/` + `app/src/notebooks/` → new crate `ml_file_views` | Code editor view (`code/editor/*`: element, model, find, goto_line, nav_bar, scroll, Vim handler), `local_code_editor.rs` (load/save + editor model), Markdown/Jupyter viewer with Rendered/Raw toggle (`notebooks/file/`, the needed parts of `notebooks/editor/`), `util/openable_file_type.rs`, `util/image.rs` | LSP (`lsp`, find references, language server extension), AI diff validation, code review comments/inline diff, `remote_server`, `repo_metadata`, cloud objects, workspace/pane-group/terminal coupling. Replace them with small traits (§14.4). About 15k + 16k lines in Warp; the largest reuse job. |

What `warp_editor` needs from `warp_core` (measured by grep): `features::FeatureFlag` (incl.
`MarkdownTables`, `MarkdownMermaid`), `ui::theme::Fill`, `ui::theme::color::internal_colors`,
`ui::appearance::{Appearance, DEFAULT_UI_FONT_SIZE}`, `ui::Icon`, `platform::SessionPlatform`,
`safe_error`, `channel::ChannelState`. That's a small surface, so a trimmed `warp_core` is
realistic.

### 3.3 Reuse as a pattern (copy ideas, not crates)

| Source | Pattern |
|---|---|
| `app/src/persistence/sqlite.rs`, `crates/persistence` | diesel + `embed_migrations!`, a dedicated writer thread fed by a channel, read connections for queries. The schema is terminal-specific: write MatterLite's own. |
| `crates/warp_core/src/paths.rs` | Portable `data/` detection (`portable_root()`), per-platform config/state/cache dirs. |
| `crates/http_client` | reqwest client construction (proxy, TLS, user agent) and error mapping; MatterLite runs reqwest on Tokio directly, so `async-compat` isn't needed. |
| `crates/websocket` | Native/proxy-aware connect via `async-tungstenite`; MatterLite uses `tokio-tungstenite` but can copy the proxy handling. |
| `app/src/bin/oss.rs` | Hardcoded app config, embedded `Info.plist` (`embed_plist`) for a bare macOS binary. |
| `app/build.rs`, `crates/warpui/build.rs` | Windows resources (icon, subsystem), macOS bindgen only on macOS. |
| `script/windows/build-portable.ps1`, `package-release.ps1`, `script/portable/*.sh`, `.github/workflows/build-portable.yml` | Build + verify + package portable archives (§16). |
| `app/src/workspace/view.rs`, settings pages | Window layout, sidebar/tab patterns, settings UI structure. |
| `crates/warpui/examples/*` | Minimal WarpUI apps (list, scrolling, formatted text, image, table) — the best documentation of the framework. |

### 3.4 Don't reuse

Terminal (`warp_terminal`, `warp_completer`, `command-signatures-v2`, `warp_ripgrep`),
AI/agents (`ai`, `mcp`, `warp_multi_agent_client`, `computer_use`, `voice_input`),
Warp cloud (`graphql`, `warp_graphql_schema`, `cloud_object*`, `warp_server_*`, `firebase`,
`managed_secrets*`), `remote_server`, `lsp`, `node_runtime`, `warp_js`, `integration`,
`warp_tui`, `onboarding`, `channel_versions`, and the rest of `app/` (only `themes/`,
`code/`, `notebooks/`, parts of `workspace/`, `tab.rs`, `pane_group/`, `undo_close/` and a
few `util/` files are extracted, as listed in §3.2).

### 3.5 Gaps WarpUI doesn't cover (needs new code)

- **Tray / menu-bar icon** — not present in WarpUI. Use `tray-icon` (+ `muda` for its
  menu) and integrate with the event loop on each platform; macOS needs main-thread calls.
- **Taskbar / dock unread badge** — no API found. Windows: `ITaskbarList3::SetOverlayIcon`;
  macOS: `NSApp.dockTile.badgeLabel`; Linux: Unity LauncherEntry D-Bus (optional).
- **Custom URL scheme** (later, for SSO) — Warp registers `warposs://` on Windows
  (`app/src/app_services/windows/mod.rs`); copy and rename.
- **Animated GIF/WebP emoji and images** — check `warpui` `animated_images` example; may be
  sufficient.

---

## 4. Bringing in Warp code (copy and adapt)

### 4.1 Principles

- **Self-sufficient repository.** MatterLite builds, tests and releases without
  `..\warp-terminal` being present. There are no `path = "../warp-terminal/…"` and no
  git dependencies on the Warp repository. CI enforces this (§4.7).
- **Only needed code.** Nothing is kept "in case"; dead code found after an import is
  deleted, not commented out.
- **Freely adapted.** Copied code belongs to MatterLite from the moment it lands. It is
  renamed, restructured, simplified or rewritten whenever that helps. There is **no
  obligation to stay close to Warp** and no upstream sync process.
- **Warp stays a reference.** `C:\Projects\Libs\warp-terminal` (pinned commit
  **`5bb7bcc6a`**, 2026-11-06) is read by humans and agents to understand code, look up
  how something was solved, or copy more pieces later. The provenance records (§4.4) say
  where each piece came from.
- **Third-party crates stay crates.io/git dependencies** (`wgpu`, `cosmic-text`,
  `arborium`, `diesel`, …), including Warp's own published git forks (`winit` fork,
  `mermaid_to_svg`). Those are separate repositories, not the Warp source tree.

### 4.2 Three tiers

Strategy depends on how cohesive the Warp code is:

| Tier | Strategy | Applies to | Naming |
|---|---|---|---|
| **A: framework** | Copy the **whole crate**, get it compiling, then **prune** unused modules, features and dependencies. Adapt APIs where MatterLite needs it. | `warpui_core` (63k lines), `warpui` (24k), `warp_editor` (`crates/editor`, 32k), `sum_tree`, `string-offset`, `markdown_parser`, `vim`, `syntax_tree`, `languages`, `warpui_extras`, `asset_cache`, `asset_macro`, `warp_files`, `watcher`, `fuzzy_match`, `settings` + `settings_value` + `settings_value_derive`, `ui_components`, `warp_errors` (tiny, imported by 29 framework files) | **Keep Warp's crate and directory names** so code maps 1:1 to the reference |
| **B: app-level** | **Selective extraction**: copy only the needed files, types or functions from Warp's `app/` (and from `warp_core`). Cut every link to terminal, AI, Drive, LSP or telemetry; replace app-crate dependencies with small MatterLite traits. | `warp_core` (kept name: `features` with MatterLite flags only, `ui::{appearance, theme, icons, color}`, `paths`, `platform`, `safe_log`, `errors`, `async` helpers); `ml_themes` ← `app/src/themes`; `ml_editor` ← `app/src/editor`, `settings/{editor, select}.rs`; `ml_workspace` ← `app/src/{tab.rs, pane_group/, undo_close/}`, `workspace/view/{left_panel, right_panel, vertical_tabs*, session_history}.rs`, `code/file_tree/`, `code/icon.rs`; `ml_file_views` ← `app/src/code/`, `app/src/notebooks/`, `app/src/util/{openable_file_type, image}.rs` | `warp_core` keeps its name (the editor and themes import it widely); everything extracted from `app/` goes into `ml_*` crates |
| **C: helpers** | **Inline or rewrite** small utilities rather than carrying whole crates. | `warp_util` → keep only `path`, `standardized_path`, `content_version`, `user_input`, `file`, `file_type` (and `local_or_remote_path`/`remote_path` only if `ml_file_views` still needs them after cutting remote support); `command` → dropped (only needed by `warp_util` parts that aren't kept); `warp_features` → folded into `warp_core::features`; `warp_logging` → rewritten as a small `ml_logging` (env_logger + file rotation, using Warp's `rotation.rs` as reference) | Keep `warp_util` as a small crate if many imports use it, otherwise inline into callers |

Tiers are a starting point, not a rule. If a Tier A crate turns out to be mostly unused,
demote it to selective extraction. If a Tier B extraction keeps pulling in more and more
files, stop and redesign that piece as new code.

### 4.3 Procedure

**Step 0: bootstrap the repository**

1. `cargo new --vcs git` workspace at `C:\Projects\Libs\matterlite`.
2. Copy and adapt `rust-toolchain.toml` (`1.98.1`), `.rustfmt.toml`, `.clippy.toml` and
   `deny.toml`. Add `LICENSE` (AGPL-3.0) and `LICENSE-MIT` (Warp's MIT text, for the files
   derived from `warpui`/`warpui_core`).
3. Create `PROVENANCE.md`, `AGENTS.md` (§4.5) and `script/provenance` (§4.7).
4. Write the root `Cargo.toml` from scratch: `[workspace.package] license =
   "AGPL-3.0-only"`, `[workspace.dependencies]` with only the third-party versions
   MatterLite needs (look them up in Warp's root `Cargo.toml`: `wgpu = "30"`,
   `cosmic-text`, `objc2*`, `windows*`, `arborium`, `diesel`, …), the
   **`winit` git fork pin** (`warpdotdev/winit` rev `a4e0ecb…`), `mermaid_to_svg` (git), and
   the `rlto` release profile.

**Step 1: framework (Tier A), bottom-up.** Import in dependency order, so every
commit builds:

```
warp_errors → string-offset → sum_tree → markdown_parser → settings_value(+_derive)
→ warpui_core → warpui            (milestone check: `cargo run -p warpui --example list`)
→ asset_cache → asset_macro → warp_core (Tier B extract) → warpui_extras → settings
→ ui_components → vim → warp_editor → languages → syntax_tree
→ watcher → warp_files → fuzzy_match
```

For each crate:

1. **Import commit**: copy `crates/<name>/` **verbatim** from Warp at the pinned commit,
   **without** adding it to `[workspace] members`, so the build stays green. Message:
   `import(warp): crates/<name> @ 5bb7bcc6a`.
2. **Adapt commits**: add it to the workspace; point its dependencies at MatterLite's
   workspace entries; delete Warp-only features and modules (telemetry, wasm,
   `integration_tests`, terminal hooks); fix compile errors; add provenance headers.
   Message: `adapt(<name>): …`.
3. **Prune**: build with `-W dead_code -W unused`, run `cargo machete` (unused
   dependencies), then delete unused modules, features and `pub` items that nothing in
   MatterLite uses (`cargo +nightly udeps` and `cargo modules` help). Repeat after each
   feature milestone, because usage grows over time.

**Step 2: app-level extractions (Tier B), as features need them.**

| When | Extraction |
|---|---|
| M0 | `ml_workspace` shell (tab bar, toolbelt/tool panel, right panel) + `SearchBarView`; `ml_themes`; `ml_editor` (`EditorView`, for the search bar, login form and composer) |
| M3 | History-pane row/selection patterns from `session_history.rs` |
| M5 | `ml_file_views` (read-only spike in M0), `code/file_tree` for the Files explorer |
| M8 | `pane_group` for split panes, vertical tabs |

For each extraction:

1. **Import commit**: copy only the chosen files verbatim into
   `crates/ml_<x>/src/warp_import/…` (not yet `mod`-declared, so it isn't compiled).
2. **Adapt commits**: move them into place, declare modules, replace `crate::…` app imports
   with MatterLite types or traits (`FileViewHost`, `LinkHandler`, …), delete coupled
   features, and rename freely.
3. Record each file in `PROVENANCE.md`.

**Step 3: helpers (Tier C).** Copy individual functions into the crate that uses them, or
rewrite them. Record in `PROVENANCE.md` with status `rewritten` or `snippet`.

**Step 4: new code.** `mm_*` crates and MatterLite-specific `ml_*` code are written fresh.
Read Warp for patterns (e.g. the SQLite writer thread) but don't copy wholesale. Files
inspired by Warp without copied code get no provenance header.

### 4.4 Provenance

**`PROVENANCE.md`** (repository root): one row per imported unit. That's a whole crate
for Tier A, or a file/module for Tier B and C.

```markdown
Reference: C:\Projects\Libs\warp-terminal @ 5bb7bcc6a (2026-11-06)

| MatterLite path | Warp source | Commit | Status | License | Notes |
|---|---|---|---|---|---|
| crates/warpui_core/ | crates/warpui_core/ | 5bb7bcc6a | adapted | MIT | removed wasm, integration_tests, telemetry |
| crates/ml_workspace/src/tab_bar.rs | app/src/tab.rs | 5bb7bcc6a | adapted | AGPL-3.0 | kept TabBarState/TabComponent; no terminal panes |
| crates/ml_workspace/src/search_bar.rs | app/src/workspace/view/session_history.rs | 5bb7bcc6a | adapted | AGPL-3.0 | search field, toggle bar, Matcher only |
| crates/ml_logging/src/lib.rs | crates/warp_logging/src/rotation.rs | 5bb7bcc6a | rewritten | AGPL-3.0 | |
```

Status values: `verbatim` (unchanged since import), `adapted` (modified), `rewritten` (only
the ideas remain), `snippet` (a few functions copied into new code).

**Header per derived file** (added in the first adapt commit; `script/provenance`
can add and check them):

```rust
// SPDX-License-Identifier: AGPL-3.0-only        (MIT for files from warpui / warpui_core)
// Derived from warp-terminal `app/src/tab.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.
```

**Import/adapt commit pairs**: the verbatim import commit is the exact reference
point. To see everything MatterLite changed in a piece, compare against that commit with
`git diff <import-commit> -- <path>`, or against the Warp source directly with
`git diff --no-index ../warp-terminal/app/src/tab.rs crates/ml_workspace/src/tab_bar.rs`.

### 4.5 `AGENTS.md` rules (MatterLite repository)

```markdown
## Warp reference code
- `C:\Projects\Libs\warp-terminal` (commit 5bb7bcc6a) is READ-ONLY reference material.
  Read it freely to understand framework code, find existing solutions, or copy more pieces.
- Never add a Cargo dependency (path or git) on the warp-terminal repository. MatterLite
  must build without it.
- To reuse Warp code: (1) commit a verbatim copy (`import(warp): <path> @ <sha>`), not yet
  compiled; (2) adapt in follow-up commits; (3) add the SPDX + "Derived from" header;
  (4) add or update the row in PROVENANCE.md.
- Copy only what is needed. Delete what becomes unused; don't keep Warp features "for later".
- Code from crates/warpui and crates/warpui_core stays MIT; everything else is AGPL-3.0-only.
- When a derived file is rewritten beyond recognition, set its PROVENANCE status to
  `rewritten`, but keep the row.
- Framework crates keep Warp's names (warpui, warpui_core, warp_editor, sum_tree, …) so that
  paths map to the reference. App-level code lives in ml_* crates and Mattermost logic in mm_*.
```

Also carry over the WarpUI rules from Warp's own `AGENTS.md` that still apply, e.g. the
`MouseStateHandle` rule (§10.1) and the Element/View conventions.

### 4.6 Changing the reference commit

Warp keeps evolving, and MatterLite doesn't follow it. When a later Warp fix is wanted:

1. Update the local reference checkout and note the new commit.
2. Diff the relevant Warp file between the old and new reference commits.
3. Apply the change by hand to the MatterLite code (it has diverged), as a commit
   `port(warp): <what> from <sha>`.
4. Update the commit column for those rows in `PROVENANCE.md`.

### 4.7 Checks (CI and `script/provenance`)

- **No reference dependency**: fail if any `Cargo.toml` mentions `warp-terminal`, or a path
  dependency leaves the repository root.
- **Headers vs. table**: every file with a "Derived from warp-terminal" header has a
  `PROVENANCE.md` row (directly or through its crate row), and every row's path exists.
- **Licenses**: `cargo deny check licenses bans sources`, with only crates.io and the
  allow-listed git forks (`warpdotdev/winit`, `warpdotdev/mermaid-to-svg`).
- **Pruning**: `cargo machete` and `clippy -D warnings` (including `dead_code`) in CI.
  Run a periodic `cargo udeps` job.
- **Clean build without the reference**: CI clones only the MatterLite repository, so
  any hidden dependency on `..\warp-terminal` fails there.

---

## 5. Workspace layout

```
matterlite/
  Cargo.toml                 workspace, [workspace.dependencies], profiles
  rust-toolchain.toml        1.98.1
  LICENSE (AGPL-3.0)
  crates/
    mm_model/                serde types: Post, Channel, ChannelMember, User, Team, Preference,
                             Reaction, FileInfo, Thread, ClientConfig, AppError, WS events
    mm_api/                  REST client on reqwest (Bearer auth, AppError, NDJSON, 429 handling)
    mm_ws/                   WebSocket client on tokio-tungstenite (auth, seq, resume, ping, backoff)
    mm_store/                normalized in-memory state + apply(event) + derived views
                             (unreads, sidebar, timeline); no UI deps
    mm_cache/                diesel/SQLite schema, migrations, writer thread, hydrate/persist
    mm_sync/                 bootstrap, channel loading, resync, mark-read, optimistic send;
                             owns the Tokio runtime and talks to UI via channels
    mm_render/               comrak → MessageDoc (styled spans/blocks) incl. mentions, ~channels,
                             emoji, hashtags, code (syntax_tree), system messages
    mm_emoji/                emoji-datasource index + embedded PNG set (rust-embed)
    ml_extract/              attachment text extraction (text/PDF/Office/ODF/sheets/archives/
                             ipynb) with limits, timeouts and panic isolation (§8.6)
    ml_themes/               Warp themes (extracted from app/src/themes) + Mattermost themes,
                             ChatColors, Mattermost theme JSON ⇄ WarpTheme conversion
    ml_editor/               Warp's EditorView (single- and multi-line text input) in one file,
                             EditorView.rs, extracted from app/src/editor; used by the search
                             bar, login form, dialogs and the composer
    ml_file_views/           Warp code editor + Markdown/Jupyter viewer + image lightbox,
                             extracted from app/src/{code,notebooks}, without LSP/AI
    ml_workspace/            Warp-style shell: tab bar, tool panel + toolbelt, right panel,
                             pane groups, SearchBarView (from session_history.rs), tree view
    ml_chat/                 chat models and views: Session (owns the engine), login, window
                             root, Chats tab, chat view, composer, History (M3)
    ml_logging/              small logger with rotation (rewritten from warp_logging)
    matterlite/              the binary: WarpUI app, views, keymaps, settings, platform glue

    # Warp-derived framework crates (Tier A/B, §4.2), Warp names kept, pruned + adapted:
    warpui_core/  warpui/  warp_core/  warp_editor/ (Warp dir: crates/editor)  sum_tree/
    string-offset/  markdown_parser/  vim/  syntax_tree/  languages/  warpui_extras/
    asset_cache/  asset_macro/  warp_files/  watcher/  fuzzy_match/  settings/
    settings_value/  settings_value_derive/  ui_components/  warp_errors/  warp_util/
  assets/                    icons, fonts, emoji images, theme backgrounds (from Warp app/assets)
  PROVENANCE.md              Warp-derived pieces → source path @ commit, status, license (§4.4)
  AGENTS.md                  rules for agents, incl. Warp-as-reference rules (§4.5)
  LICENSE, LICENSE-MIT       AGPL-3.0 for the project; MIT for warpui/warpui_core-derived files
  script/                    build-portable.{ps1,sh}, package-release.{ps1,sh}
  .github/workflows/         build-portable.yml (Linux + macOS), presubmit
```

`mm_model`, `mm_api`, `mm_ws`, `mm_store`, `mm_sync`, `mm_render` have **no WarpUI
dependency** — they can be unit-tested headless and reused for a TUI later.

---

## 6. Architecture

```
┌──────────────────────── UI thread (WarpUI event loop) ────────────────────────┐
│  Views: Login, TeamSidebar, ChannelSidebar, ChannelView(Timeline+Composer),     │
│         ChannelSwitcher, Settings                                               │
│  Models (singletons): SessionModel, StoreModel (wraps mm_store::Store),         │
│         SettingsModel, AppearanceModel                                          │
│        ▲ spawn_stream_local(ui_rx)              │ cmd_tx.send(Command)          │
└────────┼────────────────────────────────────────┼──────────────────────────────┘
         │ async-channel (StoreDelta / UiEvent)   ▼ async-channel (Command)
┌────────┴──────────── Sync thread (Tokio multi-thread runtime) ──────────────────┐
│  mm_sync::Engine                                                                │
│   ├─ mm_api::Client (reqwest)          REST calls                               │
│   ├─ mm_ws::Connection                 events → Store::apply → deltas           │
│   ├─ mm_store::Store (authoritative)   normalized state                         │
│   └─ mm_cache::Writer ──► DB thread (diesel, SQLite)                             │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### 6.1 Threads and channels

- **UI thread**: WarpUI's event loop (`platform::AppBuilder::run`). All views and models live
  here. Never block it.
- **Sync thread**: `std::thread::spawn` → `tokio::runtime::Builder::new_multi_thread()`.
  Owns the REST client, WebSocket, the authoritative `Store`, and decides what to persist.
- **DB thread**: diesel `SqliteConnection` is `!Sync`; one writer thread receives batched
  writes over a channel (Warp's `app/src/persistence/sqlite.rs` pattern). Reads at startup
  (hydration) happen on the sync thread before connecting.
- Channels: use **`async-channel`** (runtime-agnostic) in both directions so the UI side can
  `await` it on WarpUI's executor and the sync side on Tokio.

### 6.2 Data flow

1. UI dispatches `Command`s: `Login{server, login_id, password, mfa}`, `SelectTeam`,
   `OpenChannel`, `LoadOlder{channel}`, `SendPost{channel, root, text, pending_id}`,
   `MarkViewed`, `SetPreference`, `Logout`, …
2. The engine performs REST calls / handles WS events, mutates `Store`, and emits
   **`StoreDelta`s** (e.g. `PostsUpserted{channel, ids}`, `ChannelUpdated(id)`,
   `UnreadsChanged`, `MembersChanged`, `ConnectionState(..)`).
3. `StoreModel` on the UI thread receives deltas via
   `ModelContext::spawn_stream_local(ui_rx, …)` (`warpui_core/src/core/model/context.rs`),
   applies them to its **UI-side copy** (cheap `Arc` snapshots of entities), then
   `ctx.emit(StoreEvent::…)`/`ctx.notify()` so subscribed views re-render.

Keeping the authoritative store on the sync thread and sending deltas (with the changed
entities as `Arc<T>`) avoids locks on the UI thread. An alternative is
`Arc<RwLock<Store>>` read by the UI; simpler, but risks contention while paging large
channels.

M2 implements the `Arc<RwLock<Store>>` variant (`mm_sync::SharedStore`): the engine holds the
write lock for one change at a time and reports the changes as `Event::Changed(deltas)`, so
the UI re-reads only what changed. If contention shows up in M3, the deltas already name
everything a UI-side copy would need.

---

## 7. Networking layer

Protocol details: `WebApp §3–§7`.

### 7.1 `mm_api`

- `reqwest` with `rustls`, gzip, HTTP/2, system proxy. One `Client` per server.
- Every request: `X-Requested-With: XMLHttpRequest`, `Authorization: Bearer <token>`,
  `User-Agent: MatterLite/<version> (<os>)`.
- `POST /users/login` → token from the `Token` response header (WebApp §3.2); MFA retry on
  `mfa.validate_token.authenticate.app_error` / `api.user.check_user_mfa.bad_code.app_error`.
- Error type: `ApiError::{App(AppError), RateLimited{retry_after}, Unauthorized, Http(..),
  Decode(..)}`. 401 → emit `SessionExpired` → UI returns to login.
- NDJSON for `GET /users/{id}/channel_members?page=-1` (`bytes_stream` + line splitter).
- Config/license as `HashMap<String, String>` with typed accessors
  (`cfg.bool("EnableLatex")`, `cfg.collapsed_threads()`).
- Mirror endpoint names from `server/public/model/client4.go` for discoverability.

### 7.2 `mm_ws`

- `tokio-tungstenite` (rustls). URL from `WebsocketURL` or site URL; query
  `connection_id`, `sequence_number` (WebApp §7.1).
- Send `authentication_challenge` right after connect (also set the Bearer header on the
  upgrade request: the server only resumes connections authenticated that way, and an invalid
  token still gets a `101` followed by a dropped socket; WebApp §7.2).
- Track `expected_seq`; on mismatch close with 4001 and reconnect; on a new
  `connection_id` in `hello` emit `MissedEvents` → engine runs a **full resync**
  (WebApp §7.4).
- App ping every 30 s, close with 4000 if no pong; backoff 3 s → `3 s × n²` after 7
  failures, cap 5 min, + up to 2 s jitter (WebApp §7.5).
- Decode events into `enum WsEvent { Posted{post: Post, mentions: Vec<String>, …},
  PostEdited(Post), … , Unknown{event, data} }`, doing the **second JSON parse** of string
  fields here (WebApp §7.7).
- Outgoing: `user_typing`, `presence`, `user_update_active_status`, `get_statuses_by_ids`.
- Treat OS sleep/resume and network changes as reasons to reconnect immediately
  (reset backoff).

---

## 8. State, sync and the SQLite cache

### 8.1 `mm_store`

Normalized, like the webapp's redux `entities` (WebApp §8.3):

```rust
pub struct Store {
    pub server: ServerId,
    pub config: ClientConfig, pub license: ClientLicense,
    pub me: UserId,
    pub users: HashMap<UserId, Arc<User>>, pub statuses: HashMap<UserId, Status>,
    pub teams: HashMap<TeamId, Arc<Team>>, pub team_members: HashMap<TeamId, TeamMember>,
    pub team_unreads: HashMap<TeamId, TeamUnread>,
    pub channels: HashMap<ChannelId, Arc<Channel>>,
    pub channel_members: HashMap<ChannelId, ChannelMember>,   // mine
    pub channels_in_team: HashMap<TeamId, BTreeSet<ChannelId>>,   // "" = DMs/GMs
    pub posts: HashMap<PostId, Arc<Post>>,
    pub timelines: HashMap<ChannelId, Timeline>,              // §10.4
    pub threads: HashMap<PostId, Vec<PostId>>,                // root → replies
    pub preferences: HashMap<(String, String), String>,
    pub categories: HashMap<TeamId, Vec<Category>>,
    pub roles: HashMap<String, Role>,
    pub pending: HashMap<String /*pending_post_id*/, PendingState>,
}
impl Store { pub fn apply(&mut self, ev: &WsEvent) -> Vec<StoreDelta>; }
```

Derived views (pure functions, unit-tested): `unread(channel)` (WebApp §10.1),
`team_badges()`, `sidebar(team)` (WebApp §14), `display_name(user)`, `dm_name(channel)`,
`timeline_items(channel)` with date separators, "new messages" line, combined join/leave
(WebApp §9.5).

### 8.2 Sync engine (`mm_sync`)

Startup (WebApp §6, "Recommended startup"):

1. Hydrate from SQLite (if present) → send a full snapshot to the UI → **render
   immediately** (stale data marked as such).
2. Validate token: `GET /users/me`. On 401 → login screen.
3. Fetch config/license, preferences, teams, team members, all my channels, channel members
   (NDJSON), team unreads, categories of the selected team, roles.
4. Connect WebSocket. Then for each cached timeline, `GET /channels/{id}/posts?since=<max
   update_at>`; for the open channel, the unread window.
5. Diff/merge into the store, emit deltas, persist.
6. Start the **eager sync scheduler** (§8.5): people and emoji, recent posts of every
   channel, threads, attachments with text extraction (§8.6), then history backfill.

Optimistic send (WebApp §9.3): create pending post with
`pending_post_id = "{user_id}:{now_ms}"`, insert into timeline, `POST /posts`; replace on
response or on the matching `posted` event; mark failed on error with retry.

Mark read: on channel open and on incoming posts while the window is focused and the
channel visible → `POST /channels/members/me/view`.

### 8.3 SQLite cache (`mm_cache`)

- **diesel 2.3** with `sqlite` + `returning_clauses_for_sqlite_3_35`, `diesel_migrations`
  with `embed_migrations!("migrations")`; bundled SQLite (`libsqlite3-sys` with `bundled`)
  so no system SQLite is needed on any OS.
- One DB file per server: `<state_dir>/servers/<server_id>/cache.sqlite`
  (`server_id` = hash of the normalized site URL + user id).
- `PRAGMA journal_mode=WAL; synchronous=NORMAL; foreign_keys=ON`.
- Store server entities mostly as **JSON blobs** keyed by id plus the few columns needed
  for queries — this survives server schema changes without migrations:

```sql
CREATE TABLE users     (id TEXT PRIMARY KEY, update_at INTEGER NOT NULL, json TEXT NOT NULL);
CREATE TABLE teams     (id TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE channels  (id TEXT PRIMARY KEY, team_id TEXT NOT NULL, json TEXT NOT NULL);
CREATE TABLE channel_members (channel_id TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE posts     (id TEXT PRIMARY KEY, channel_id TEXT NOT NULL, root_id TEXT NOT NULL,
                        create_at INTEGER NOT NULL, update_at INTEGER NOT NULL,
                        delete_at INTEGER NOT NULL, json TEXT NOT NULL);
CREATE INDEX posts_channel_time ON posts(channel_id, create_at DESC);
CREATE TABLE timeline_ranges (channel_id TEXT NOT NULL, newest_id TEXT, oldest_id TEXT,
                        reached_newest INTEGER, reached_oldest INTEGER);
CREATE TABLE preferences (category TEXT, name TEXT, value TEXT, PRIMARY KEY(category, name));
CREATE TABLE kv        (key TEXT PRIMARY KEY, value TEXT);   -- last team/channel, sync times
CREATE TABLE drafts    (channel_id TEXT, root_id TEXT, message TEXT, updated_at INTEGER,
                        PRIMARY KEY(channel_id, root_id));
-- Files explorer index (§10.9): every FileInfo seen in post.metadata.files or file search
CREATE TABLE files     (id TEXT PRIMARY KEY, post_id TEXT NOT NULL, channel_id TEXT NOT NULL,
                        user_id TEXT NOT NULL, name TEXT NOT NULL, extension TEXT, size INTEGER,
                        mime_type TEXT, create_at INTEGER NOT NULL, delete_at INTEGER NOT NULL,
                        local_path TEXT,            -- set when downloaded (§8.6)
                        -- download state: pending | downloading | done | evicted
                        --                | skipped_size | skipped_policy | denied | gone | failed
                        dl_state TEXT NOT NULL DEFAULT 'pending', dl_error TEXT, dl_attempts INTEGER DEFAULT 0,
                        sha256 TEXT, last_access INTEGER,
                        -- text index state: pending | indexed | unsupported | too_large | failed
                        text_state TEXT NOT NULL DEFAULT 'pending', extractor TEXT,
                        json TEXT NOT NULL);
CREATE INDEX files_channel_time ON files(channel_id, create_at DESC);
CREATE INDEX files_dl_queue ON files(dl_state, create_at DESC);
CREATE TABLE channel_file_sync (channel_id TEXT PRIMARY KEY, fully_listed INTEGER, synced_at INTEGER);
-- Full-text search for the History pane (§10.8): plain-text message + attachment names
CREATE VIRTUAL TABLE posts_fts USING fts5(post_id UNINDEXED, channel_id UNINDEXED,
                        text, file_names, tokenize = 'unicode61 remove_diacritics 2');
-- Local content index of downloaded attachments (§8.6); kept even if the file bytes are evicted
CREATE VIRTUAL TABLE file_text_fts USING fts5(file_id UNINDEXED, channel_id UNINDEXED,
                        post_id UNINDEXED, name, content, tokenize = 'unicode61 remove_diacritics 2');
-- Per-channel sync bookkeeping for eager backfill (§8.5)
CREATE TABLE channel_sync (channel_id TEXT PRIMARY KEY,
                        newest_synced_at INTEGER,   -- max(update_at) seen via since-sync
                        backfilled_to INTEGER,      -- oldest create_at fetched (0 = reached start)
                        last_full_check INTEGER, priority INTEGER);
```

FTS5 needs SQLite compiled with `SQLITE_ENABLE_FTS5`; the bundled `libsqlite3-sys` build
enables it. Diesel runs FTS queries through `sql_query`.

The implemented schema is `crates/mm_cache/migrations/` (M2): as above, except that ranges
belong to a channel or a thread (`timeline_ranges.owner_kind`), "known empty" timelines have
their own table, categories, roles, custom emoji and followed threads have JSON tables, and
the `files`, `file_text_fts` and `outbox` tables come with later migrations (M5, M2 step 6).

- **Local retention = the history backfill window** (§8.5). It's the single rule for how
  long messages stay in the cache, whatever the server's own retention policy (§8.7):
  - Posts inside the window are kept, including ones the server has since purged.
  - Posts older than the window are pruned in the background, along with their
    attachments, text index rows and FTS rows. Exceptions: **saved (flagged)** posts and
    their files are always kept. Older posts fetched explicitly (scrolling back, search
    results, permalinks) are kept for 7 days after last access, then pruned.
  - "All history" never prunes by age; only the attachment quota (bytes) applies.
  - Shrinking the window asks for confirmation and shows how much would be deleted.
  - Vacuum occasionally.
- Writes are batched (e.g. every 250 ms or 200 rows) on the writer thread. Eager sync
  writes a lot, so FTS inserts happen in the same batches.
- The cache is **rebuildable**: on migration failure or corruption, rename it aside
  (`cache.sqlite.broken-<ts>`, so the attachments and the user's data aren't lost silently)
  and refetch.
- On logout the data is **kept by default**; deleting it is an explicit choice (§8.7).

### 8.4 What the webapp caches (and doesn't)

The webapp is **in-memory, lazy and short-lived**. A native client can do much better.

| Layer | What | Where |
|---|---|---|
| Redux store | All entities (users, channels, members, posts, threads, preferences…) | **Memory only**, per browser tab; refetched on every page load (WebApp §6) |
| redux-persist / IndexedDB | **Drafts only** | `webapp/channels/src/store/index.ts`, `reducers/storage.ts` |
| localStorage | Last team/channel, recent emoji, login/logout signals | `stores/local_storage_store.ts` |
| Post prefetch | Posts of **unread channels only**: mentions first (max 10), then other unreads, **skipped entirely if more than 20 channels are unread**; 2 requests at a time; 0–1 s jitter after new posts | `components/data_prefetch/` (`prefetchQueue`, `PrefetchLimits`) |
| Channel open | Unread window (30 before / 30 after) or `since` sync; older pages only on scroll | `actions/views/channel.ts` |
| Profiles/statuses | Fetched for visible posts, sidebar DMs/GMs, autocomplete; statuses polled for visible users | `batchFetchStatusesProfilesGroupsFromPosts`, status polling |
| Browser HTTP cache | `FileInfo` JSON `max-age=30d, private`; avatars `max-age=24h` (default avatar 5 min); static assets/emoji 1 year; file bodies via `http.ServeContent` (Last-Modified, Range) | `server/channels/api4/file.go`, `api4/user.go`, `web/static.go`, `server/platform/shared/web/files.go` |
| Attachments | **Never prefetched.** Thumbnails/previews load when visible; files download only on click | — |
| File content search | **Server-side only** (`files/search`, needs `ExtractContent` on the server) | — |
| Negative cache | Unknown custom emoji names (`nonExistentEmoji`) | `MR/reducers/entities/emojis.ts` |

Result: on startup the webapp knows only metadata and the posts of the channel you open,
plus up to 20 unread channels. History, files and search are server round trips.

### 8.5 Eager sync: what to fetch early and keep

MatterLite treats the local DB as the **primary read source**. The UI reads only from the
store and cache, and a background **sync scheduler** in `mm_sync` keeps the cache filled
and fresh. Candidates, in priority order:

| Prio | Data | How | Why early |
|---|---|---|---|
| **P0 startup** (blocking only until the first render from cache) | Client config, license, me, preferences, teams, team members, **all my channels**, **all my channel memberships** (NDJSON), categories, roles, team unreads, thread counts | WebApp §6 bootstrap | Small; needed for the sidebar, badges and permissions |
| **P1 people and emoji** | Profiles of **all users I share a channel with**: `GET /users/known` → ids → `POST /users/ids` in batches of 100. Their statuses: `POST /users/status/ids`. **All custom emoji**: `GET /emoji?page&per_page=200`. Avatars of known users: `/users/{id}/image?_=<last_picture_update>` into `cache/avatars/` | Mentions, display names, autocomplete, reactions and avatars all render with no request. Incremental refresh: `POST /users/ids?since=<last sync>` (as the webapp's `checkForModifiedUsers` does) |
| **P2 recent posts, every channel** | For **every joined, non-archived channel**: newest page (`GET /channels/{id}/posts?per_page=60`, CRT-aware), ordered: mentions → unread → open tabs → favourites → DMs/GMs → recently viewed → rest | Every channel opens instantly; History pane and badges work offline |
| **P3 threads** | Followed threads per team (`/users/{uid}/teams/{tid}/threads`, paged) and their replies (`GET /posts/{id}/thread`) for threads active within the backfill window | Threads view and right panel open instantly |
| **P4 attachments** | Every `FileInfo` from fetched posts → **download the file, thumbnail and preview, then extract text** (§8.6) | Viewing is instant; **file search runs on actual files** |
| **P5 history backfill** | Walk each channel backwards (`before=<oldest_id>&per_page=200`) until the **backfill window** is covered. Same priority order as P2, round-robin between channels so one huge channel can't starve the rest | History pane and local search cover real history, not just the last page |
| **P6 small extras** | Pinned posts per channel, channel bookmarks, flagged posts, drafts, scheduled posts, channel stats (member counts via `POST /channels/stats/member_count`), link-preview images from `metadata.embeds` | Activity and Channel tabs open instantly |

**Freshness:**

- **Live**: WebSocket events update the store and DB directly. `posted` events also queue
  the new post's attachments at the front of the P4 queue (§8.6).
- **After a reconnect without missed events**: nothing to do (sequence-number replay,
  WebApp §7.4).
- **After a resync** (new `connection_id`): for every channel whose
  `last_post_at`/`last_root_post_at` is newer than `channel_sync.newest_synced_at`,
  `GET /channels/{id}/posts?since=<newest_synced_at>`. That picks up new, edited and
  deleted posts.
- Edits, deletes and reactions in channels with no new posts don't move `last_post_at`.
  A **low-priority sweep** repeats the `since` call for every channel (e.g. every 30 min,
  and once after each resync), so they're caught too.
- Users: `POST /users/ids?since=`; emoji: `emoji_added` event plus a daily re-list;
  threads: `getCountsAndThreadsSince` (WebApp §7.4).

**Scheduler rules:**

- **Single priority queue** with classes P0…P6. User actions (opening a chat, scrolling,
  opening a file) jump ahead as **P0-interactive**.
- **Politeness**: a global token bucket, default **8 requests/s with burst 20**, under the
  server's default rate limit of 10/s with burst 100 when rate limiting is enabled
  (`RateLimitSettings`; WebApp §4.3). Default concurrency: REST 4, downloads 3.
  - Honour `429` + `Retry-After` and `X-RateLimit-Remaining`; back off globally.
  - Add 0–1 s jitter after bursts of `posted` events (the webapp does this so thousands of
    clients don't hit the server together).
- **Resumable**: progress lives in `channel_sync` and `files.dl_state`, so a restart
  continues where it stopped.
- **Visible state**: a small status indicator ("Syncing history 63% · 412 files queued"),
  with pause/resume and a "Sync now" button; an optional "pause on battery".

**Settings** (Settings → Offline & sync; defaults in bold):

| Setting | Options |
|---|---|
| History backfill window (also the **local retention** period, §8.3) | 30 days · **180 days** · 1 year · All history · Off (recent page only) |
| Former chats retention (left, removed or archived chats, §8.7) | Delete immediately · N days (number field, **default 90**) · Keep forever |
| Channels to backfill | **All joined channels** · Favourites + DMs/GMs + open tabs · Manual per channel |
| Auto-download attachments | **All files ≤ size limit** · Images only · Off (on demand) |
| Per-file size limit for auto-download | 10 MB · **100 MB** · 1 GB · No limit (bounded by the server's `MaxFileSize`) |
| Attachment storage quota | 2 GB · **10 GB** · 50 GB · Unlimited; LRU eviction of file **bytes** only. Extracted text and metadata are kept, so search still covers evicted files |
| Extract text from attachments | **On** · Off |
| Request rate | Gentle (4/s) · **Normal (8/s)** · Fast (no client limit; server limits still apply) |

Server-side gates: no attachment downloads if `EnableFileAttachments` is false. A `403`
on a file (e.g. an access-control policy `download_file_attachment`) is recorded as
`denied` and not retried. A `404`/`410` (deleted file or post) is recorded as `gone`.

### 8.6 Attachment cache and local content index

Each attachment goes through a pipeline, driven by the `files` table states:

```
FileInfo seen ──► pending ──download──► done ──extract──► indexed
   (post fetched,      │                  │                 (file_text_fts)
    posted event,      ├─► skipped_size / skipped_policy   ├─► unsupported / too_large / failed
    file search)       ├─► denied (403) / gone (404/410)   │
                       └─► failed (retry with backoff)     └─ quota pressure ─► evicted
                                                               (text kept, bytes deleted)
```

- **When**: as soon as a `FileInfo` is known: a live `posted` event (front of the queue),
  P2/P3/P5 page fetches, or file search results. Files of the **open chat** and of
  **visible messages** jump the queue.
- **Where**: `state/servers/<server_id>/files/<file_id>/<sanitized name>`. Per server (and
  per user), kept across restarts and logouts (§8.7), and part of the portable `data/` folder
  in portable mode. Write to a temp
  file and rename atomically. Store a sha256 for integrity.
  - **Thumbnails and previews** for images: `/files/{id}/thumbnail` and
    `/files/{id}/preview` into `…/files/<file_id>/thumb.jpg` and `preview.jpg`. They're
    small, so they're fetched even when the full file is skipped for size.
- **Download**: `GET /files/{id}` streamed to disk, resumable with HTTP `Range` (the server
  uses `http.ServeContent`, which supports ranges), with up to 3 concurrent downloads.
  - **Mattermost files never change**, so a completed download is never revalidated.
  - Deleted posts/files: on `post_deleted` or a `delete_at > 0` FileInfo, delete the
    bytes and the text, and mark the file `gone`.
- **Text extraction** (`ml_extract`, on a worker thread pool, never on the UI thread):

| Formats | Extractor |
|---|---|
| Plain text, code, logs, CSV/TSV, JSON/YAML/XML/TOML, Markdown, SVG source | read as text; encoding detection with `chardetng` + `encoding_rs` |
| HTML | `html5ever` (already used by Warp's `markdown_parser`) → text |
| PDF | `pdf-extract` (or `lopdf`) |
| DOCX, PPTX, ODT, ODP | `zip` + `quick-xml`: collect `w:t` / `a:t` / `text:p` runs |
| XLSX, XLS, ODS | `calamine` → cell text, sheet by sheet |
| RTF | simple control-word stripper |
| Jupyter `.ipynb` | Warp's `ipynb_parser` → markdown + outputs as text |
| ZIP / TAR / GZ archives | list entry names (always indexed); extract text entries recursively up to a depth/size budget (mirrors the server's `ArchiveRecursion`) |
| Images, audio, video, binaries | name and metadata only (`unsupported`); OCR is a possible later add-on |

  Limits: skip extraction for inputs > 50 MB (`too_large`) and cap extracted text at
  5 MB per file. Extractors run with a timeout and panic isolation, because
  malformed files from chat are untrusted input.
- **Index**: `file_text_fts(name, content)`, kept **even after the bytes are evicted**,
  so search still covers evicted files. Opening an evicted file re-downloads it on demand.

**Search coverage instead of silent gaps.** Every local search (History "files" scope,
Files explorer "content" scope) reports what it actually covered, using the `files`
states. That answers the "no match, or not checked yet?" question directly:

```
3 matches in 1,208 files searched
⏳ 14 not downloaded yet  ·  📏 3 over size limit  ·  🚫 2 access denied  ·  ❔ 9 unsupported format
[Download & search remaining]  [Search on server]
```

- **Download & search remaining**: queues the pending and over-limit files of the current
  scope as interactive priority, then re-runs the query as files get indexed. Results
  update live.
- **Search on server**: runs `files/search` for the same terms, useful for files outside the
  backfill window and formats the server extracts but MatterLite doesn't.
- The History pane shows the same summary for **messages**: "Local history for #dev covers
  9 Jun 2026 → now", plus "Search older on server" (§10.8).

**Disk and privacy:**

- Eager caching puts message history and attachments on disk. Settings show the space
  used per server and per chat, with a "Clear attachments" button. Logout **asks** whether
  to delete local data and keeps it by default (§8.7).
- Portable mode keeps it all inside `data/`. Encrypting the cache at rest (SQLCipher, plus
  encrypted attachment storage) is a later option; until then rely on OS disk encryption.
  Say so in the README. Because data survives logout, this matters more.

### 8.7 Cache lifecycle: restart, crash, long offline, logout

**What survives a restart.** Everything on disk: the SQLite DB (entities, timelines,
`channel_sync` cursors, FTS indexes, drafts, tab and pane state), the attachments and their
text index, settings, themes, and the token in secure storage. Only process state is lost,
and it's rebuilt:

| In-memory state | After restart |
|---|---|
| `mm_store` | **Hydrated from SQLite** before any network call; first frame drawn from cache (stale marker on badges until P0 finishes) |
| Sync scheduler queue | Rebuilt from durable state: `channel_sync` (backfill progress), `files.dl_state = pending/downloading` (→ re-queued), `text_state = pending` (→ re-extracted) |
| WebSocket `connection_id` + next `seq` | **Persisted** in `kv` (on clean shutdown and every ~30 s). On start, reconnect with them. The server keeps a disconnected connection for about **5 minutes** and queues up to **256** events for it, plus the 128 last sent (`sendQueueSize`/`deadQueueSize` in `web_conn.go`, `inactiveConnReaperInterval` in `web_hub.go`; WebApp.md §7.4). A quick restart therefore **resumes with no resync at all**; otherwise the server sends a new `connection_id` and the normal resync runs (§8.5). |
| Statuses, typing indicators | Shown as unknown/stale until `get_statuses_by_ids` and live events arrive |
| Ephemeral posts | Dropped (they never existed on the server; the webapp behaves the same) |
| Unread "New messages" line position | Restored from `channel_members.last_viewed_at` in the cache, then corrected by P0 |

**Startup sequence after a restart** (extends §8.2):

1. Open DB → run migrations → hydrate store → render tabs, sidebar and timelines from cache.
2. Load the token from secure storage; `GET /users/me`. A 401 means the session expired or
   was revoked: show the login screen **over the cached UI**. Data is kept, and after
   re-login, sync continues where it stopped.
3. Compare server identity: client config `DiagnosticId` plus the user id against the values
   stored at last sync. If the **server was reinstalled or reset** (`DiagnosticId`
   changed), ask: "This server's data has changed. Keep the old local copy as an archive,
   or start fresh?"
4. P0 bootstrap (§8.5), then try to resume the WebSocket with the saved `connection_id`/`seq`.
5. Reconcile, then continue the scheduler.

**Reconcile after being offline** (restart, sleep or network loss):

- **Channels**: for each channel with `last_post_at` newer than `channel_sync.newest_synced_at`,
  run `GET /channels/{id}/posts?since=…`. **The server returns at most 1000 posts per
  `since` call** (`GetPostsSince` in `server/channels/store/sqlstore/post_store.go`).
  If a reply has 1000 posts, treat it as a **gap**: fetch the newest page and mark the
  range between as a hole in `timeline_ranges`. Backfill (P5) then fills the hole
  from both ends. Never show a gap as continuous history; the timeline shows "Loading
  missing messages…" there.
- **Membership changes while offline**: channels you left or were removed from, archived
  channels, and DMs with deactivated users are detected by the P0 channel/member lists.
  - Their tabs close (with a notice).
  - Their data is **hidden from the normal views immediately** and kept read-only under
    **"Former chats"** in the Chats tab and the Files explorer. It's purged after the
    **former-chat retention** period: configurable, **default 90 days** (§8.5 settings). The
    countdown starts when the membership loss is detected; rejoining the chat cancels it and
    the data becomes live again.
- **Users**: `POST /users/ids?since=<last sync>`; **emoji**: re-list if the last sync is older
  than a day; **preferences, categories, roles**: replaced wholesale by P0.
- **Server data retention does not prune the local cache.** Licensed servers can delete
  old messages and files (`DataRetentionEnableMessageDeletion` /
  `DataRetentionEnableFileDeletion`, `…RetentionHours` in client config; granular policies
  via `GET /users/{id}/data_retention/team_policies` and `…/channel_policies`). Those
  deletions are silent: no `post_deleted` events, the posts just stop existing. MatterLite
  keeps its local copies. **Local history is governed only by the history backfill
  window** (§8.3, §8.5):
  - Backfill stops where the server's history ends (`prev_post_id == ""`); messages cached
    earlier that are older than that stay local, marked "no longer on server" in the
    History pane.
  - Attachments whose server copy is gone (`404`/`410` → `dl_state = gone`) keep their
    downloaded bytes and text. They are **evicted last** under quota pressure, because they
    can't be downloaded again.
  - Settings → Offline & sync shows an info line when the two differ, e.g. "This server
    deletes messages after 90 days; MatterLite keeps local copies for 180 days (history
    window)". This keeps the choice visible, since it can conflict with an organisation's
    retention policy.
  - **Explicit deletions are still applied**: a message deleted by its author or an admin
    (`post_deleted` event, or `delete_at > 0` from `since` sync) is removed locally, with its
    attachments and text, as in the webapp. Only the silent retention purge is ignored.

**Unsent messages (outbox).** Pending posts and their uploads are persisted in an
`outbox` table:

```sql
CREATE TABLE outbox (pending_post_id TEXT PRIMARY KEY, channel_id TEXT, root_id TEXT,
                     message TEXT, props_json TEXT, file_ids TEXT, local_upload_paths TEXT,
                     first_attempt_at INTEGER, last_attempt_at INTEGER, state TEXT);
```

- The server de-duplicates by `pending_post_id` only for **30 seconds**
  (`pendingPostIDsCacheTTL` in `server/channels/app/post.go`). It doesn't store the id with
  the post, so a later fetch can't match by it.
- On restart, a post whose last attempt was less than 30 s ago is **re-sent automatically**
  with the same `pending_post_id`.
- Older attempts are **reconciled** first: fetch the channel `since=first_attempt_at - 60 s`
  and look for a post by me with the same `channel_id`, `root_id`, message and `file_ids`.
  - Found → mark as sent.
  - Not found → show it as **failed** with Retry/Delete. Never re-send it blindly, to
    avoid duplicates.
- Files uploaded before the restart (the post wasn't sent yet): server file ids stay valid
  for a while. If the post fails with a file error, re-upload from the copy kept in
  `state/servers/<id>/outbox/`.

**Crash safety.**

- WAL mode; every batch is one transaction. **Sync cursors (`channel_sync`,
  `timeline_ranges`) are written in the same transaction as the data they describe**, so
  the cache can never claim data it doesn't have. A crash loses at most the last unwritten
  batch (≤ 250 ms), which the next sync refetches.
- Downloads write to `*.part` files and resume with HTTP `Range`; rename on completion,
  then set `dl_state = done`. On start, a `.part` without a matching `downloading` row is
  deleted.
- Extraction is idempotent: `text_state = pending` is simply redone.
- Schema migrations run in a transaction. If one fails, the DB is kept aside (see §8.3)
  rather than deleted.

**Logout: keep data by default, ask first.**

```
┌ Log out of chat.example.com (alice) ─────────────────────────────┐
│ Your messages and 2.3 GB of downloaded files stay on this         │
│ computer so the next login is instant.                            │
│                                                                    │
│ [ ] Also delete local data for this account (history, files,      │
│     search index, drafts)                                          │
│                                                                    │
│                                   [Cancel]   [Log out]             │
└────────────────────────────────────────────────────────────────────┘
```

- **Log out** (default): `POST /users/logout`, then delete the token from secure storage and
  close the WebSocket. **Keep** `state/servers/<server_id>/` (DB, attachments, text index,
  outbox, tab state). Logging in again as the same user on the same server reuses it; only
  the delta is synced.
- **With the checkbox**: also delete `state/servers/<server_id>/` and that account's
  avatars, emoji and link-preview images in `cache/`. Drafts and outbox entries that were
  never sent are listed in the confirmation, so nothing unsent disappears silently.
- **Session expired or revoked by the server** (401 or `session_expired`): not a logout.
  Data is kept, and the UI stays readable offline under a "Log in again" banner.
- **Data isolation**: `server_id` includes the user id, so another account on the same
  server gets its own directory and never sees the previous user's cache. Before
  logging in, the login screen lists **stored accounts** (server, user, size, last used),
  each with **Delete local data**.
- **Settings → Storage**: per-account usage (messages, files, index), with Clear
  attachments, Clear history, Delete everything for this account, and an optional
  **"Delete local data for accounts not used for N days"** (default off).
- **Portable mode**: the same rules apply inside `data/`. Copying the folder to another
  machine keeps the cache, but the Windows token (DPAPI) won't decrypt there, so it asks
  for login and then reuses the cache.

---

## 9. Storage locations and the token

Reuse `warp_core::paths` (trimmed) with the app name `MatterLite`:

| Mode | Root | Layout |
|---|---|---|
| Portable — a directory named `data` exists next to the executable | `<exe dir>/data/` | `config/settings.toml`, `config/keybindings.yaml`, `config/themes/`, `state/servers/<id>/cache.sqlite`, `state/servers/<id>/files/<file_id>/…` (attachments, thumbnails, previews; §8.6), `state/logs/`, `state/secure/` (Windows token files), `cache/` (avatars, custom emoji, link-preview images; re-fetchable) |
| Installed | Windows: `%APPDATA%\MatterLite` (config) + `%LOCALAPPDATA%\MatterLite` (state, cache); macOS: `~/Library/Application Support/MatterLite`, `~/Library/Caches/MatterLite`; Linux: `$XDG_CONFIG_HOME/matterlite`, `$XDG_STATE_HOME/matterlite`, `$XDG_CACHE_HOME/matterlite` | same subfolders |

Like the Warp fork, portable mode collapses roaming/local into one `state/`.

The config directory is watched while MatterLite runs: edits to `settings.toml` apply at once
(a file that doesn't parse keeps the current settings) and changes in `themes/` reload the
custom themes.

**Token**: `warpui_extras::secure_storage` with service name `MatterLite`, key
`token-<account id>` (no `:`: on Windows the key is part of a file name):

- macOS → Keychain (`security-framework`).
- Linux → Secret Service (`secret-service`, D-Bus). If no Secret Service is running
  (minimal WMs), fall back to `register_unavailable` → user must log in each start; show a
  hint.
- Windows → DPAPI-encrypted file in the storage dir (`register_with_dir`), i.e. inside
  `data/state/secure/` in portable mode. DPAPI binds it to the Windows user account: copying
  the portable folder to another PC/user makes the token unreadable → app asks to log in
  again (expected behaviour).

Never write the token to logs (`safe_log`), the SQLite cache, or `settings.toml`.

---

## 10. UI with WarpUI

### 10.1 Getting productive with WarpUI

- Start from `crates/warpui/examples/` (`list`, `scrolling`, `formatted-text`, `image`,
  `table-sample`, `typed_actions`, `resizable`) and Warp's own `app/src/workspace/view.rs`.
- Concepts (Warp `AGENTS.md`): a global `App` owns entities; views hold `ViewHandle<T>`;
  `AppContext`/`ViewContext`/`ModelContext` give temporary access; **actions** for
  input handling; `Element`s describe layout (Flutter-like) and are rendered on the GPU.
- **Gotcha**: create each `MouseStateHandle` once at construction and reuse it — an
  inline `MouseStateHandle::default()` during render makes mouse interaction silently fail.
- Fonts: bundle a UI font and a monospace font with `rust-embed` (Warp embeds its fonts),
  so text looks the same everywhere; rely on system fallback for CJK.

### 10.2 Window layout (Warp-style)

The window copies Warp's workspace layout: a **tab bar** across the top, a **tool panel**
on the left with a toolbelt of icons, the **active tab's content** in the middle, and a
**right panel** for threads.

```
┌──────────────────────────────────────────────────────────────────────────────────────┐
│ [📌# general 3] [@carol] [🔒dev @2] [🧵 Release plan] [+]                    ⚙   ☰    │ tab bar
├──┬────────────────────────┬─────────────────────────────────────────┬────────────────┤
│💬│ CHATS          Team ▾  │ 🔒 dev  ☆  header text…          👤 12   │ Thread      ✕  │
│🔔│ [🔍 filter chats    ]  │  ── Today ──                            │ bob: root post │
│ⓘ │ ▾ FAVORITES            │  [av] alice  10:02                      │  ↳ alice: …    │
│🕘│   # general         3  │       Hello **world** :wave:            │  ↳ you: …      │
│📁│ ▾ CHANNELS             │  ────────── New messages ──────────     │                │
│🔍│   # random             │  [av] bob  10:05   💬 3 replies         │ ┌────────────┐ │
│  │   🔒 dev           @2  │  carol is typing…                       │ │ reply…     │ │
│  │ ▾ DIRECT MESSAGES      │ ┌─────────────────────────────────────┐ │ └────────────┘ │
│  │   ● carol              │ │ composer (EditorView)        Send ▶ │ │                │
└──┴────────────────────────┴─┴─────────────────────────────────────┴─┴────────────────┘
 toolbelt  tool panel (Alt+1…6)        chat tab content                  right panel
```

- **Toolbelt and tool panel** (Warp's `app/src/workspace/view/left_panel.rs`,
  `ToolPanelView`): an icon per tab, `Alt+1…6` to switch, the same shortcut again to hide
  the panel, resizable, and the open tab and width persisted. Six tabs (§10.6).
- **Tab bar** (Warp's `app/src/tab.rs`, `TabBarState`, `TabComponent`): one tab per
  recent chat (§10.7). Optional vertical tabs, as in Warp
  (`app/src/workspace/view/vertical_tabs*`).
- **Chat tab content**: channel header, timeline, typing indicator, composer. Later (after
  the MVP) a tab can hold **split panes** using Warp's pane groups
  (`app/src/pane_group`), e.g. two chats or a chat next to a thread. Design the tab content
  as a single-pane `PaneGroup` from the start so splits need no restructuring.
- **Right panel** (Warp's `app/src/workspace/view/right_panel.rs`, which hosts code
  review in Warp): the reply thread with its own composer, and post edit history.
  Resizable, maximizable and closable; "Open in tab" turns the thread into a chat tab.
- Other views: `LoginView` (server URL → ping → login → MFA), `SettingsView` (Warp's
  settings page layout: Appearance/themes, Display, Keyboard, Notifications later),
  `ChannelSwitcher` (Ctrl/Cmd+K with `fuzzy_match`, like Warp's command palette), and
  `ConnectionBanner` ("Reconnecting…").

### 10.3 Theming

See §11. All views read colors from Warp's `Appearance` (`Appearance::as_ref(ctx).theme()`)
plus the chat palette `ChatColors` that comes with the active theme.

### 10.4 Timeline

- Per channel, a `Timeline` in `mm_store`: ordered post ids (`BTreeMap<(create_at, id), ()>`
  or Warp's `sum_tree` with a height summary), list of contiguous loaded ranges,
  `reached_oldest/newest` flags (WebApp §9.2).
- Render with WarpUI's **virtualized list** (`viewported_list` / `uniform_list` —
  check which supports variable item heights; chat items vary) **anchored to the bottom**;
  load older posts when scrolled near the top (`before=<oldest_id>&per_page=60`), keeping
  the scroll anchor stable.
- Cache laid-out message elements by `(post_id, update_at, width bucket, theme)`.
- Items: post (full header or compact continuation), date separator, new-messages line,
  combined system message, pending/failed post with retry.

### 10.5 Keyboard

Warp's keymap system (`warpui_core::keymap`, `keybindings.yaml`). Defaults combine Warp and
the webapp:

| Keys | Action |
|---|---|
| `Alt+1…6` | Show/hide tool panel tab (Chats, Activity, Channel, History, Files, Search) |
| `Ctrl/Cmd+K` | Chat switcher (opens or focuses a tab) |
| Warp's `ActivateNextTab` / `ActivatePrevTab` bindings, `Ctrl/Cmd+1…9` | Next/previous tab, go to tab N (`ActivateTabByNumber`, `app/src/workspace/mod.rs`) |
| `Ctrl/Cmd+W`, reopen-closed-tab binding | Close tab, reopen closed tab (Warp's `undo_close`) |
| `Alt+↑/↓`, `Alt+Shift+↑/↓` | Previous/next chat in the list, previous/next unread |
| `Ctrl/Cmd+F` | Focus the History search bar for the active chat |
| `Ctrl/Cmd+Shift+F` | Server-wide Search tab |
| `Esc` | Mark the active chat read / close the right panel |
| `↑` in empty composer | Edit last own message |

### 10.6 Tool panel: mapping the webapp's panes

The webapp has **9 right-hand-sidebar states** (`RHSStates` in
`webapp/channels/src/utils/constants.tsx`: mention, search, flag, pin, plugin,
channel-files, channel-info, channel-members, edit-history), plus the **reply thread** and
the plugin **post card** (`components/sidebar_right/sidebar_right.tsx`). It also has
**4 full-page centre views** (Threads, Drafts, Scheduled posts, Recaps;
`components/channel_layout/center_channel/center_channel.tsx`) and the **left sidebar**
(team rail plus channel categories). That's about 14 surfaces. MatterLite groups them
into **6 tool-panel tabs and the right panel**:

| Tab (shortcut) | Contents | Webapp equivalent → API |
|---|---|---|
| **Chats** (`Alt+1`) | Team switcher (dropdown plus unread dots) at the top; filter field; sidebar categories with channels, DMs and GMs; unread/mention badges; "+" to browse, create or open a DM | Team rail + LHS → categories API (WebApp §14) |
| **Activity** (`Alt+2`), sub-tabs | **Threads** (followed threads, unread first; CRT) · **Mentions** · **Saved** · **Drafts** · **Scheduled** | Threads view → `/users/{uid}/teams/{tid}/threads`; RHS *mention* → OR search of your mention keys; RHS *flag* → `/users/{uid}/posts/flagged`; Drafts view → `/users/{uid}/teams/{tid}/drafts`; Scheduled view → `/posts/scheduled/team/{tid}` |
| **Channel** (`Alt+3`), sub-tabs; follows the active chat tab | **Info** (header, purpose, favourite, mute, notification preferences, copy link, leave) · **Members** (with status, add/remove if permitted) · **Pinned** · **Bookmarks** | RHS *channel-info*, *channel-members*, *pin*; channel bookmarks bar → `/channels/{id}`, `/channels/{id}/members`, `/channels/{id}/pinned`, `/channels/{id}/bookmarks` |
| **History** (`Alt+4`) | The active chat's messages, one row each, grouped by date (§10.8) | New (closest webapp equivalent: searching `in:channel`) |
| **Files** (`Alt+5`) | Every sent and received file across all chats, grouped by chat (§10.9) | RHS *channel-files*, extended to all chats → `/teams/{tid}/files/search` |
| **Search** (`Alt+6`) | Server-wide search: **Messages \| Files** results with modifiers (`from:`, `in:`, `before:`, `after:`, `on:`, `ext:`) and the same search bar (§10.10) | RHS *search* → `/teams/{tid}/posts/search`, `/teams/{tid}/files/search` |
| **Right panel** (not a tab) | Reply thread with composer; post edit history | RHS thread; RHS *edit-history* |
| Not supported | Plugin RHS, plugin post cards, Recaps (AI) | Plugins can't run natively (WebApp §17) |

Clicking a result anywhere (Activity, Pinned, History, Files, Search) **focuses or opens
the chat's tab and scrolls to the message**, using the permalink flow (WebApp §18).
Replies open in the right panel.

### 10.7 Chat tabs

- Opening a chat from the Chats tab, the switcher, a notification or a permalink **focuses
  its tab if one is open, otherwise opens a new tab** next to the active one.
- **LRU cap**: at most N unpinned tabs (setting, default 12). Opening one more closes the
  least-recently-used unpinned tab; its draft is already saved (§13). Pinned tabs are
  never auto-closed.
- Warp tab features reused: pin, rename (local alias), colour, drag reorder, close/reopen
  (`undo_close`), tab groups later.
- The tab label shows the channel icon (`#`, lock, `@` avatar, GM count), the display name,
  an **unread dot** or **mention count**, and a typing indicator.
- A **thread** can also be a tab (from "Open in tab" in the right panel), labelled with a
  thread icon and the root message excerpt.
- Open tabs (order, pins, scroll anchor per tab) are persisted per server in SQLite `kv`
  and restored on start.
- Each tab keeps its own timeline scroll position, unsent draft, History search state and
  filter mode.

### 10.8 History pane

Shows the **history of the active chat tab** and follows tab switches. It works like Warp's
Session history pane (`app/src/workspace/view/session_history.rs`), with chat messages
instead of terminal blocks.

```
┌ HISTORY · #dev ─────────────────────────────┐
│ [🔍 search messages…       ] (x) | .* Aa    │  search field (§10.10)
│ 💬 messages  📎 files  |  ⏚ filter chat      │  toggle bar
│ ▾ Today (14)                                 │
│   10:05 bob     Deploy is done, see logs …   │
│   10:02 alice   Hello world 👋               │
│ ▾ Yesterday (32)                             │
│   …                                          │
│ ▸ Mon 9 Nov 2026 (57)                        │
│ ▸ Fri 6 Nov 2026 (12)                        │
│ … load older                                 │
└──────────────────────────────────────────────┘
```

- **Rows**: one per message (root posts and, with CRT off, replies; with CRT on, replies are
  folded under their root as "↳ N replies"). Each row shows time, author display name and a
  single-line plain-text excerpt (`stripMarkdown`, WebApp §11.1). It also shows icons for
  attachments (📎 count), pinned, edited, and an urgent/important priority label.
  System messages are shown dimmed; join/leave rows are hidden unless the chat shows them.
- **Date groups**: one group per calendar day in the user's timezone, header
  "Today / Yesterday / Mon 9 Nov 2026 (count)". Each group can be collapsed or expanded. By
  default **Today and Yesterday are expanded** and older days collapsed. The state is
  remembered per chat (and per tab) in SQLite `kv`. Expanding an old group loads its
  messages from the cache, or from the server with `before=`/`after=` paging, if not loaded.
- **Data**: the local cache, which eager sync (§8.5) fills with the whole backfill window
  (default 180 days) for every channel. Anything older loads on demand ("load older" at
  the bottom). The pane and the chat share the same `mm_store` timeline, so new messages,
  edits and deletes appear in both live. The pane footer shows the covered range: "Local
  history: 9 Jun 2026 → now".
- **Search**: the shared search bar (§10.10) with two **scope toggles**:
  - 💬 **Messages** (on by default): matches message text, author and attachment names.
  - 📎 **Files** (off by default): also matches the **content of attached files**, searched
    **locally in the downloaded files** through `file_text_fts` (§8.6). Each match marks the
    owning message and shows the filename plus a snippet.

  How a query runs:
  1. **Locally**: an instant search over cached messages and, with 📎, over the extracted
     text of cached attachments. Literal, case-insensitive by default; optional **regex**
     and **case-sensitive** toggles. Plain queries use SQLite FTS5; regex and case-sensitive
     queries scan the FTS candidates (or all rows) in a worker thread.
  2. **Coverage line** (§8.6): "3 matches · 1,208 files searched · ⏳ 14 not downloaded yet
     · 📏 3 over size limit", with **Download & search remaining**. You always know whether
     "no match" really means no match.
  3. **On the server, only on request** ("Search older on server", or automatically when the
     query reaches past the local history range):
     `POST /teams/{tid}/posts/search {terms: "<query> in:<channel-name>"}` (`in:@username`
     for DMs), plus `files/search` with 📎. Results merge into their date groups, marked
     "from server", and their posts and files go into the cache and download queue.
     Regex and case-sensitive are local-only, because server search is word-based. With
     either toggle on, the server pass is unavailable and the pane says so.
  - Groups with matches expand automatically and show match counts; groups without
    matches are hidden while a query is active.
- **Navigate**: click a row → scroll the chat tab to that message and highlight it
  (permalink-style flash); `Enter`/`↑`/`↓` move through results from the keyboard.
  Shift/Ctrl-click to multi-select (copy as text, later "forward").
- **Filter the chat** (⏚ toggle, Warp's "filter-on-select" idea): when on, the chat tab
  itself shows **only matching messages**, with a banner "Showing 12 of 340 messages
  matching 'deploy' · Show all". Turning it off or clearing the query restores the full
  timeline at the same anchor. Filtering is per tab and never touches the server or the
  cache.

### 10.9 Files explorer pane

Replaces Warp's Project explorer (`app/src/code/file_tree/`): instead of a directory tree, a
tree of **chats → files**.

```
┌ FILES ──────────────────────────────────────┐
│ [🔍 search files…          ] (x) | .* Aa    │
│ 📄 names  📝 content  |  ◎ active chat only   │
│ ▸ # general (23)                             │
│ ▾ 🔒 dev (5)                                 │
│     ⬇ build.log        12 KB  bob   10:05    │
│     ⬆ config.yaml       2 KB  you   Mon      │
│     🖼 screenshot.png  340 KB alice  Mon      │
│ ▸ @ carol (8)                                │
│ ▸ 👥 alice, bob, carol (2)                   │
│ … more chats                                 │
└──────────────────────────────────────────────┘
```

- **Groups**: one per chat (channel, DM, GM), named like the sidebar, sorted by the most
  recent file. **All groups are collapsed by default**; the expand state is remembered.
  The count is known files, with "…" until the chat's listing is complete.
- **Rows**: icon by type (reusing Warp's file icons from `app/src/code/icon.rs`), name,
  size, sender, date, and a ⬆ sent / ⬇ received marker (sender = you). A **state badge**
  from `files.dl_state` / `text_state` (§8.6): ✓ downloaded and indexed, ⏳ queued,
  📏 over the size limit, 🚫 denied, ❔ content not searchable, ☁ evicted (text kept).
  Thumbnails appear on hover for images.
- **Data sources**:
  - The **local index**: every `FileInfo` seen in `post.metadata.files` is stored in the
    SQLite `files` table (§8.3), and eager sync downloads and indexes the files (§8.6). The
    explorer works offline and instantly.
  - **Completing a chat's list**: files posted before the backfill window are found with
    `POST /teams/{tid}/files/search {terms: "channel:<channel_id>"}`, paged. The webapp's
    channel-files pane does exactly this (`showChannelFiles` in
    `webapp/channels/src/actions/views/rhs.ts`); DMs/GMs search across teams. This runs as
    a low-priority P6 job per chat, and immediately when a group is expanded. Found files
    enter the normal download/index pipeline.
  - Chats with no known files are hidden until a search or "Show all chats" reveals them.
  - Footer: totals and the same coverage breakdown as search ("4,812 files · 4,790 indexed ·
    14 queued · 8 over limit"), plus storage used.
- **Actions**:
  - Click → open in the Warp viewer (§14).
  - Double-click → open with the system app (with a safety prompt for executables).
  - Context menu: Save as…, Copy link (if public links are on), **Go to message** (focus or
    open the tab and scroll to the post), Open containing chat, Remove from local cache.
- **Search bar** (§10.10) with scope toggles:
  - 📄 **Names** (on): instant local filter on file names (and on archive entry names).
  - 📝 **Content** (off): searches the **extracted text of the downloaded files** locally
    (`file_text_fts`, §8.6), showing a snippet under each matching file. Works whether or
    not the server has `ExtractContent` enabled.
  - Coverage line and **Download & search remaining** / **Search on server**, as in §8.6.
    The server `files/search` pass is optional and only needed for files the local cache
    doesn't hold.
  - ◎ **Active chat only**: restrict to the active tab's chat. This is the webapp's
    "Channel files" pane.
  - `ext:` filter chips (Documents, Images, Code, Archives…) like the webapp's file
    filter menu (`components/search_results/files_filter_menu.tsx`).
  - Matching groups expand automatically while a query is active; non-matching ones hide.

### 10.10 Shared search bar (from Warp's Session history)

All the new panes (History, Files, Search, plus the filters in Chats and Activity) use one
reusable **`SearchBarView`**. It is extracted from Warp's
`app/src/workspace/view/session_history.rs` (about 1,150 lines; the search parts are
`render_search_field`, `render_toggle_bar`, `render_toggle`, `render_group_separator` and the
`Matcher` enum):

- **Field**: magnifier icon + single-line `EditorView` (`EditorView::single_line`, UI font,
  placeholder text) inside one bordered, rounded container without fill (tool-panel
  style), followed by **Clear (x)**, a separator, **Regex (.\*)** and **Case-sensitive (Aa)**
  toggles.
- **Toggle bar** below it: a configurable list of icon toggles in groups separated by
  1-px rules. In Warp: scope toggles (command / directory / output) | on-select toggles
  (filter / find). In MatterLite, each pane supplies its own toggles (History: messages,
  files | filter chat; Files: names, content | active chat only).
- **Matcher**: `Regex` / `Literal` / `LiteralInsensitive`. An invalid or half-typed regex
  doesn't blank the list (`None` → no filtering), exactly as in Warp.
- **Caching**: match counts are cached per query and dropped on any query or option change
  (Warp's `output_match_cache` pattern).
- **Keyboard**: arrows navigate rows (`PropagateAndNoOpNavigationKeys::Always`), Enter
  activates, Esc clears then blurs.
- **API** (`ml_workspace::search_bar`):
  `SearchBarView::new(SearchBarOptions { placeholder, query_options, toggle_groups }, ctx)`.
  Each `ToggleGroup` holds `ToggleSpec { id, icon, tooltip, on }` and can keep one toggle
  on (scope toggles). `query_options: false` hides Regex and Aa (server-only search). It
  emits `SearchBarEvent::{QueryChanged, ToggleChanged { id, on }, Submit, Cleared,
  Dismissed, Navigate(key)}`; the pane reads `query`, `matcher`, `is_toggle_on`. The pane
  owns the matching, local and server-side, and debounces server requests (~300 ms).
- **Server results** are merged asynchronously, with a small spinner in the field while a
  server pass runs and a "server search unavailable for regex" hint when it's skipped.

---

## 11. Themes and Appearance

MatterLite uses Warp's theme system unchanged as its base: `WarpTheme` and
`Appearance` from the trimmed `warp_core::ui`, and the theme catalogue, YAML custom themes
and theme chooser extracted from `app/src/themes` into `ml_themes`. On top of that it adds
the Mattermost themes, and a chat palette for the colors a terminal theme doesn't have.

### 11.1 What Warp provides (reused)

- **`WarpTheme`** (`crates/warp_core/src/ui/theme/mod.rs`): `background: Fill` (solid or
  vertical/horizontal gradient), `foreground`, `accent: Fill`, optional `cursor`,
  optional `background_image` (+ opacity), `details: Darker | Lighter | Custom`,
  `terminal_colors { normal, bright }` (16 ANSI colors), `name`. The color scheme
  (light/dark) is inferred from the foreground color.
- **`Appearance`** (`crates/warp_core/src/ui/appearance.rs`): the active theme, UI font,
  monospace font family/size/weight, line height, and `UiBuilder`. Views read
  `Appearance::as_ref(ctx)`, and **all views re-render automatically when it changes**
  (`AppearanceEvent::ThemeChanged`, …).
- **Built-in themes** (`app/src/themes/default_themes.rs`): Dark (default), Light, Dracula,
  Fancy Dracula, Cyber Wave, Solar Flare, Solarized Dark/Light, Gruvbox Dark/Light,
  Willow Dream, Dark City, Red Rock, Jellyfish, Leafy, Koi, Snowy, Pink City, Marble,
  Adeberry, Phenomenon. Several use gradients or a background image
  (`app/assets/async/jpg/*_bg.jpg`). The two referral-reward themes are dropped.
- **Custom themes**: YAML files in `themes_dir()` (here `config/themes/`, portable-aware),
  the same format as Warp:

  ```yaml
  name: My Theme
  accent: '#01a0e4'
  background: '#090300'
  foreground: '#a5a2a2'
  details: darker
  terminal_colors:
    normal: { black: '#090300', red: '#db2d20', green: '#01a252', yellow: '#fded02',
              blue: '#01a0e4', magenta: '#a16a94', cyan: '#b5e4f4', white: '#a5a2a2' }
    bright: { … }
  ```
  Base16 themes work too (`ThemeKind::CustomBase16`).
- **Follow the system theme**: `RespectSystemTheme::On { light, dark }` switches between
  two themes when the OS switches between light and dark mode.
- **Theme chooser** (`ml_themes::theme_chooser`) with live preview, shown as a panel left of
  the tool panel, as in Warp (brush button in its header, or Ctrl+Alt+Shift+T / Cmd+Ctrl+T). Each entry is a
  small chat drawn in the theme's colours (`ml_themes::preview`) instead of Warp's terminal
  listing. Up/Down apply and save at once. A reload button re-reads the custom themes.
  Optionally, later, the **theme creator**, which builds a theme from an image.
- **Syntax highlighting colors come from the theme's ANSI colors**:
  `syntax_highlighting_color_map` in `app/src/code/editor/model.rs` maps keyword → magenta,
  function → blue, string/number → green, type → red, comment → yellow, … into
  `syntax_tree::ColorMap`. Code blocks in messages and the file viewers (§14) therefore
  follow the theme automatically.

### 11.2 What MatterLite adds: `ChatColors`

`WarpTheme` has one background and one foreground; a chat UI also needs a sidebar, status
indicators, mention badges and highlights. Add an **optional `chat` section** to the theme
(serde field on a MatterLite wrapper, or a field added to MatterLite's copy of `WarpTheme`, so
existing Warp YAML files still load):

```yaml
chat:                      # all keys optional
  sidebar_bg: '#1e325c'
  sidebar_text: '#ffffff'
  sidebar_unread_text: '#ffffff'
  sidebar_hover_bg: '#28427b'
  sidebar_active_border: '#5d89ea'
  sidebar_active_text: '#ffffff'
  sidebar_header_bg: '#192a4d'
  sidebar_header_text: '#ffffff'
  team_bar_bg: '#162545'
  online: '#3db887'
  away: '#ffbc1f'
  dnd: '#d24b4e'
  mention_badge_bg: '#ffffff'
  mention_badge_text: '#1e325c'
  mention_highlight_bg: '#ffd470'
  mention_highlight_link: '#1b1d22'
  new_message_separator: '#cc8f00'
  link: '#1c58d9'
  button_bg: '#1c58d9'
  button_text: '#ffffff'
  error_text: '#d24b4e'
```

The key names map 1:1 to Mattermost's theme JSON (`sidebarBg`, `sidebarText`,
`sidebarUnreadText`, `sidebarTextHoverBg`, `sidebarTextActiveBorder`,
`sidebarTextActiveColor`, `sidebarHeaderBg`, `sidebarHeaderTextColor`, `sidebarTeamBarBg`,
`onlineIndicator`, `awayIndicator`, `dndIndicator`, `mentionBg`, `mentionColor`,
`mentionHighlightBg`, `mentionHighlightLink`, `newMessageSeparator`, `linkColor`,
`buttonBg`, `buttonColor`, `errorTextColor`), so conversion is mechanical.

**Derived palette for Warp themes** (no `chat` section), computed by
`ChatColors::derive(&WarpTheme)`:

| Chat color | Derived from |
|---|---|
| `sidebar_bg`, `team_bar_bg`, `sidebar_header_bg` | `background`, one and two steps darker/lighter following `details` (or the background image/gradient with a tint) |
| `sidebar_text`, `sidebar_unread_text` | `foreground` (unread: full opacity + bold; read: reduced opacity) |
| `sidebar_hover_bg` | `foreground` at about 8% over the sidebar background |
| `sidebar_active_border`, `link`, `button_bg` | `accent` |
| `button_text`, `mention_badge_text` | `pick_best_foreground_color` against the accent (`warp_core::ui::color::contrast`) |
| `mention_badge_bg` | `accent` |
| `online` / `away` / `dnd`, `error_text` | ANSI green / yellow / red, red |
| `mention_highlight_bg` | ANSI yellow at about 25% opacity |
| `new_message_separator` | ANSI red |

### 11.3 New Mattermost themes

Add five built-ins to the catalogue, defined from Mattermost's own values
(`webapp/channels/src/packages/mattermost-redux/src/constants/preferences.ts`, `THEMES`):

| Theme | `background` (centerChannelBg) | `foreground` (centerChannelColor) | `accent` (buttonBg) | Sidebar (sidebarBg) | `details` | ANSI palette (from `codeTheme`) |
|---|---|---|---|---|---|---|
| Mattermost Denim | `#ffffff` | `#3f4350` | `#1c58d9` | `#1e325c` | Lighter | GitHub (light) |
| Mattermost Sapphire | `#ffffff` | `#3f4350` | `#1c58d9` | `#1543a3` | Lighter | GitHub (light) |
| Mattermost Quartz | `#ffffff` | `#3f4350` | `#1c58d9` | `#f4f4f6` | Lighter | GitHub (light) |
| Mattermost Indigo | `#111827` | `#dddfe4` | `#4a7ce8` | `#151e32` | Darker | Solarized Dark (Warp's existing palette) |
| Mattermost Onyx | `#191b1f` | `#e3e4e8` | `#4a7ce8` | `#202228` | Darker | Monokai |

- Each gets the **full `chat` section** copied from the Mattermost definition, so the
  sidebar, badges and indicators look exactly like the webapp.
- `terminal_colors` are chosen to match Mattermost's `codeTheme` (github,
  solarized-dark, monokai), so code blocks are highlighted the way the webapp does it. Add a
  GitHub-light and a Monokai `AnsiColors` pair to `default_themes.rs`; Solarized Dark already
  exists.
- Ship them as Rust definitions next to Warp's (`ThemeKind::MattermostDenim`, …), not as
  YAML, so they're always available.
- Default theme for a new install: **Mattermost Denim** (the webapp's default), with
  system light/dark following set to Quartz / Onyx.

### 11.4 The user's server-side Mattermost theme

Mattermost stores the user's theme as a preference (category `theme`, name `""` for all
teams or `<team_id>` per team) whose value is the theme JSON above (possibly a custom
theme made in the webapp).

- `ml_themes::from_mattermost_json(&str) -> WarpTheme` converts it: background, foreground
  and accent as in §11.3, `details` from luminance, `terminal_colors` from `codeTheme`
  (`github`, `solarized-dark`, `solarized-light`, `monokai`; fall back to Warp
  Dark/Light), and the full `chat` section.
- Setting **Theme source**:
  - **Mattermost account** (default): follow the server preference live, applying
    `preferences_changed` events, including per-team themes when switching teams. Shown in
    the chooser as `ThemeKind::InMemory` named "Mattermost: <type>".
  - **Local**: any Warp or Mattermost built-in, or a custom YAML theme, optionally with
    system light/dark following. This is stored in MatterLite's `settings.toml` and never
    written to the server.
- **Export**: "Save current theme as YAML" writes the converted theme (with its `chat`
  section) into `config/themes/` so the user can edit it like any Warp theme.
- Optional later: "Upload to Mattermost" converts a Warp theme to Mattermost JSON and
  saves it as the server preference, so the webapp matches too.

### 11.5 Fonts

`Appearance` also carries the fonts. Bundle a UI font and a monospace font
(Warp bundles its fonts with `rust-embed`); expose monospace family/size (used by code
blocks, the composer and the file viewers) in Settings, as Warp does.

---

## 12. Message rendering

Rules: `WebApp §11–§12`.

Pipeline in `mm_render`:

```
post.message ──comrak (GFM: tables, strikethrough, autolink, tasklist; unsafe HTML off)──► AST
  └─ walk text nodes (not code) → split into inline tokens:
       @mention (users, @here/@channel/@all, groups) · ~channel (props.channel_mentions)
       :emoji: and emoticons · #hashtag · email · highlight own mention keys
  └─ fenced code → syntax_tree highlight spans (language from info string)
  └─ $latex$ / ```latex → MVP: show as code; later: render via external engine
  ──► MessageDoc { blocks: Vec<Block> }   (UI-independent)
         Block = Paragraph(Vec<Span>) | Heading | Quote | List | CodeBlock | Table | Rule | Image
         Span  = Text{style} | Link{url} | Mention{user} | ChannelLink | Emoji{id|unicode} | Hashtag
```

The `matterlite` crate converts `MessageDoc` into WarpUI elements
(`formatted_text_element`, flex/stack, image elements for emoji/inline images).

- Emoji-only messages (≤ some count) → large emoji (WebApp §11.2).
- Edited marker, `(message deleted)`, webhook/bot `BOT` tag, `override_username`/icon.
- System messages: build text from `post.type` + `props` (WebApp §12); fall back to
  `post.message`. Combine join/leave runs.
- Unknown `custom_*` types → render `message` + attachments.
- Comrak options mirror the webapp: `sanitize` → do **not** render raw HTML; autolink only
  allowed schemes; image size syntax `![alt](url =WxH)` needs a small custom pre-pass.
- **Emoji (`mm_emoji`)**: generate an index at build time from `emoji-datasource` JSON
  (short names, aliases, unified code points, categories, skin tones) — the same data the
  webapp uses (`webapp/channels/src/utils/emoji.json`). Embed the PNG set with `rust-embed`
  (64px, ≈ 3–6 MB) and load through `asset_cache`. Custom emoji: fetch metadata via
  `POST /emoji/names` for unknown names (negative-cache misses), images from
  `/emoji/{id}/image`, cached on disk in `cache/emoji/`.
- Avatars: `/users/{id}/image?_=<last_picture_update>`, disk cache keyed by user id +
  `last_picture_update`.

---

## 13. Composer (Warp editor)

- The `Composer` view embeds `ml_editor::EditorView`: Warp's terminal-input editor
  (`app/src/editor`), extracted into `crates/ml_editor/src/EditorView.rs`. Its features,
  public API and size are in [`EditorView.md`](EditorView.md). Create it with
  `EditorView::new` (multi-line, `autogrow`, `soft_wrap`, UI font). The thread composer in
  the right panel uses the same setup.
- `warp_editor` (Warp's `crates/editor`) is a different editor: the one behind Warp's code
  and notebook editors. It stays in the workspace for the file viewers (§14) and is not used
  by the composer.
- Behaviour to implement around it (WebApp §15, §17), and the `EditorView` features that
  carry it:
  - Enter sends, Shift+Enter newline: `EnterSettings` (Enter emits `Event::Enter`, the others
    insert a newline). For the `advanced_settings/send_on_ctrl_enter` preference, Enter
    inserts a newline and `CtrlEnter` / `CmdEnter` send. Code-block handling (Enter inside an
    open ``` block inserts a newline) is decided by the composer before sending.
  - Autocomplete popovers: `@` → `GET /users/autocomplete?in_team&in_channel&name`;
    `~` → channels; `:` → emoji (local index + custom emoji search); `/` → commands
    (`/teams/{tid}/commands/autocomplete_suggestions`). Use `fuzzy_match` for local ranking.
    The trigger comes from the text before the cursor (`chars_preceding_selections`). The
    popover is anchored at the cached cursor position (`position_id_for_cursor`). A keymap
    flag from `keymap_context_modifier` routes Up/Down/Enter/Tab to the popover. The choice is
    inserted with `select_and_replace` or `replace_last_n_characters`, so it can be undone.
  - Typing indicator: send WS `user_typing` at most every
    `TimeBetweenUserTypingUpdatesMilliseconds` (default 5000), on `Event::Edited` when
    `EditOrigin::is_user()`.
  - Drafts: save per `(channel_id, root_id)` to SQLite on `Event::Edited` (debounced) and
    restore on channel switch with `system_reset_buffer_text`, which also clears the undo
    stack. Server-synced drafts later.
  - ↑ in an empty composer edits the last own post: `propagate_and_no_op_vertical_navigation_keys:
    AtBoundary` delivers `Event::Navigate(Up)` on the first row; act on it when the editor
    `is_empty`.
  - Max length: `MaxPostSize` from client config, as `max_buffer_len`.
  - Pasting files and images: `delegate_paste_handling` hands every paste to the composer,
    which uploads files (§14.2) and inserts text itself. Dropped files already reach the
    composer, because the editor doesn't handle drops.
  - Read-only or archived channels: `set_interaction_state(InteractionState::Disabled)`.
- Keep the editor in **plain Markdown source mode** for the MVP (what you type is what is
  sent). WYSIWYG formatting can come later. Highlighting `@mentions`, `~channels` and Markdown
  syntax in the source is possible without changing the text (`update_buffer_styles`).
- Vim mode was removed from `ml_editor` during the extraction. Bringing it back means porting
  Warp's `VimHandler` implementation for `EditorView` and the vim parts of the editor model
  (`app/src/editor/view/mod.rs`, `view/model/mod.rs`, in import commit `79a4f9f`) on top of
  the `vim` crate, which is already in the workspace. Then expose it as a setting. Planned as
  an opt-in for the multi-line composers around M5, shared with the code editor's Vim mode;
  features, Warp locations, the Esc/Enter rules for the composer and the plan are in
  [`MatterLite-ComposerVim.md`](MatterLite-ComposerVim.md).
- **Collaborative drafts** (optional, M9): the user invites channel members to see and edit an
  unsent message together; the owner sends the result as a normal post. Needs a small server
  plugin as a relay, since Mattermost clients can't exchange custom data. Design, protocol and
  the parts of Warp's editor to restore: [`MatterLite-Collaborate.md`](MatterLite-Collaborate.md).

---

## 14. Files: viewing and editing with Warp's viewers

Once sending and receiving files is supported (milestone M5), MatterLite opens attachments in
Warp's own viewers and editors instead of handing everything to external apps. Protocol:
`WebApp §5.6` (upload field order, `FileInfo`, download/preview/thumbnail endpoints).

### 14.1 What Warp has

| Warp source | Capability |
|---|---|
| `app/src/code/editor/*`, `app/src/code/local_code_editor.rs` | Full code editor view: syntax highlighting (`syntax_tree` + `languages`, tree-sitter), find/replace, go to line, soft wrap, Vim mode, undo/redo, large-file scrolling, load/save with dirty tracking |
| `app/src/notebooks/file/` (+ parts of `notebooks/editor/`) | Markdown viewer/editor with a **Rendered / Raw** toggle (tables, task lists, images, links, Mermaid via `mermaid_to_svg`), Jupyter `.ipynb` rendering (`ipynb_parser`) |
| `app/src/util/openable_file_type.rs`, `warp_util::file_type` | Routing by file type: `is_markdown_file`, `is_jupyter_notebook_file`, `is_binary_file` / `is_file_content_binary`, `is_supported_code_file` (via `languages`), with targets `MarkdownViewer`, `CodeEditor`, `SystemDefault`, `SystemGeneric` |
| `app/src/util/image.rs`, `ui_components::lightbox`, WarpUI image elements | Image detection by MIME type, full-window image viewer, animated images |
| `crates/warp_files` (`FileModel`), `crates/watcher` | Central async file load/save, change events, watching for external modifications |

### 14.2 Behaviour in MatterLite

**Receiving (attachments in posts)**

1. The timeline shows file cards from `post.metadata.files` (`FileInfo`): name, size, icon by
   extension. Image thumbnails come from `/files/{id}/thumbnail`, with `mini_preview` as an
   instant blurred placeholder.
2. Attachments are normally **already on disk**: eager sync downloads them when the message
   arrives or is cached (§8.6), into `state/servers/<id>/files/<file_id>/`. Clicking a file
   that isn't there yet (still queued, over the size limit, or evicted) downloads it at
   interactive priority, with progress and cancel. Files never change on the server, so a
   cached copy is always valid.
3. The file is opened by type (routing reused from `openable_file_type`):

| Type | Opens in |
|---|---|
| Markdown (`.md`, `.markdown`) | Notebook viewer, Rendered by default, Raw toggle |
| Jupyter (`.ipynb`) | Notebook viewer (rendered) |
| Code / text with a known language, plus `.txt`, `.csv`, `.json`, `.log`, `.svg` source… | Code editor (read-only until "Edit") |
| Images (png, jpg, gif, webp, bmp, svg-as-image) | Lightbox, with zoom and next/previous across the channel's images |
| Binary / unknown / too large (e.g. text > 20 MB) | "Open with system app" (`open` / `FileTarget::SystemGeneric`) or "Save as…" (`native-dialog`) |

4. Viewers open as a **file tab** in the tab bar (next to the chat tabs, with a file icon),
   or in the **right panel** next to the chat, chosen in settings. These are Warp's
   `EditorLayout::{NewTab, SplitPane}` options; once split panes land, "SplitPane" opens the
   file beside the chat in the same tab. Opened from the Files explorer (§10.9) or from a
   file card in a message.

**Editing**

Mattermost files are immutable: there is no API to replace a file's content. So editing
works on the local cached copy, and "saving" offers:

- **Send as new version**: uploads the edited file (`POST /files`) and posts it as a
  **reply in the original post's thread**, message e.g. "Updated `config.yaml`". If CRT is
  off, it goes to the channel.
- **Save as…**: writes to a location the user picks on disk.
- **Discard changes**: reverts to the cached server copy.

Dirty state, "unsaved changes" prompts and external-change detection come from
`warp_files::FileModel`.

**Sending**

- Attach via button or file dialog, drag-and-drop onto the channel, or paste an image from the
  clipboard (WarpUI clipboard). Attachments show as chips above the composer, with upload
  progress (`client_ids` correlate each file to its upload, `WebApp §5.6`).
- Click an attachment chip **before sending** to open it in the same viewer/editor and
  adjust it (e.g. fix a config file, crop or rename later).
- **New snippet**: create a new untitled file in the code editor (choose a language), then
  attach it. Pasting a very long text block into the composer offers "Send as file".
- Respect the client config: `EnableFileAttachments`, `MaxFileSize`, and the per-post file
  limit (10). Show server rejections (`file_upload_rejected` WebSocket event, 413
  responses) inline.

### 14.3 Security

- Files from chat are untrusted. Viewers never execute content: Markdown is rendered with
  raw HTML disabled, Jupyter outputs are rendered as text/images only, and links require a
  click. Never auto-open binaries; "Open with system app" asks for confirmation for
  executable extensions (`.exe`, `.bat`, `.ps1`, `.sh`, `.app`, `.command`, …).
- Downloads go only into MatterLite's cache directory; filenames are sanitized (no path
  separators, no reserved Windows names).

### 14.4 Extraction plan (`ml_file_views`)

> **M0 spike result** ([FileViews.md](FileViews.md)): a read-only `FileView` built directly on
> `warp_editor` (about 1,300 lines) already renders Markdown, notebooks, highlighted code and
> text. The plan below is replaced by building thin views on `warp_editor` and porting only the
> pieces listed in FileViews.md (horizontal scroll, line numbers, find, go to line, links,
> lightbox, editing), instead of extracting and trimming Warp's `code/` and `notebooks/`.

The viewers are **app-level code** in Warp (about 15k lines for the code editor and 16k for
notebooks), tied to the rest of Warp's app. The extraction:

1. Copy `app/src/code/{editor/, local_code_editor.rs, footer.rs, icon.rs, opened_files.rs}`,
   `app/src/notebooks/{file/, editor/ (needed parts), styles.rs, link.rs}`,
   `app/src/util/{openable_file_type.rs, image.rs}` into `crates/ml_file_views`.
2. Remove features MatterLite doesn't need: LSP (`lsp`, `find_references_view`,
   `language_server_*`), AI (`ai::diff_validation`, agent hooks), code review
   (`code_review`, `inline_diff`, comment editors), `remote_server`, `repo_metadata`, cloud
   objects and Warp Drive, terminal/workspace/pane-group integration, telemetry.
3. Replace `crate::…` app dependencies with small traits implemented by the `matterlite`
   binary:
   - `FileViewHost`: open, close, set title, request save-as, "send as new version" callback
   - `ViewerSettings`: font, soft wrap, Vim mode (shared with the composer, see
     [`MatterLite-ComposerVim.md`](MatterLite-ComposerVim.md)), from `ml_settings`/`Appearance`
   - `LinkHandler`: open URLs and Mattermost permalinks
4. Keep `#[cfg(feature = "local_fs")]` paths enabled (MatterLite always works on local
   cached files).
5. Start with a **read-only spike** (open a local `.rs` and a `.md` file in a WarpUI window)
   before wiring edit/save, to size the effort early (M0/M1 spare time).

---

## 15. Building

### 15.1 Prerequisites

| Platform | Needs |
|---|---|
| All | Rust 1.98.1 via `rustup` (picked up from `rust-toolchain.toml`), Git |
| Windows x64 | VS Build Tools (MSVC + Windows SDK), CMake, NASM (if still required by deps), protoc only if a Warp-derived crate still needs it. **No LLVM** — `bindgen` only runs for macOS (`warpui/build.rs`). |
| macOS arm64 (Apple Silicon only; no Intel, no universal binary) | Xcode Command Line Tools (clang for `bindgen`, Metal), `cargo install cargo-bundle` only if a `.app` is wanted |
| Linux x64 | Build deps from Warp's `script/linux/install_build_deps` (X11/XCB, xkbcommon, Wayland, fontconfig, freetype, D-Bus/libsecret headers, pkg-config). Build on **Ubuntu 24.04** for an older glibc baseline. |

Re-check after trimming: Warp needed CMake/NASM/protoc for terminal/AI crates that
MatterLite drops; the remaining list may be shorter.

### 15.2 Commands

```bash
cargo run -p matterlite                         # debug
cargo build -p matterlite --profile rlto --features release_bundle
cargo nextest run --workspace                   # tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

`release_bundle` (copy from Warp): enables `rust-embed` `debug-embed` so assets are embedded
in release binaries (otherwise read from absolute build-machine paths), and sets
`#![windows_subsystem = "windows"]` (no console window).

### 15.3 Platform specifics

- **Windows**: `build.rs` embeds icon + version info (`winresource` or Warp's
  `compile_icon` approach); wgpu uses DX12 (DXC DLLs: Warp stages `dxcompiler.dll`/`dxil.dll`
  — check whether MatterLite needs them or can use FXC/Vulkan fallback); link the VC++
  runtime statically (`+crt-static`) or ship `vcruntime140*.dll` like Warp.
- **macOS**: Metal renderer; embed `Info.plist` into the bare binary with `embed_plist`
  (Warp's `app/src/bin/oss.rs`) so notifications and bundle id work without a `.app`;
  ad-hoc codesign is mandatory on arm64 (`codesign --sign - matterlite`). Notification
  permission is requested on first use.
- **Linux**: support exactly what Warp supports and nothing more:
  - **X11 and Wayland** sessions through Warp's winit fork (`x11rb`/`x11-dl`, Wayland
    client libraries).
  - Rendering with **wgpu**, with all backends enabled. Vulkan is preferred and GLES/EGL is
    the fallback; `WGPU_BACKEND=gl|vulkan` overrides the choice
    (`crates/warpui/src/rendering/wgpu/mod.rs`).
  - Notifications via D-Bus (`notify-rust`), file dialogs via `native-dialog`, clipboard
    via `arboard`, fonts via fontconfig.
  - Token storage via Secret Service (gnome-keyring or KWallet).

  The binary `dlopen`s its windowing and graphics libraries. Runtime packages (Warp's list):
  `libx11-6 libx11-xcb1 libxcb1 libxi6 libxcursor1 libxkbcommon0 libxkbcommon-x11-0
  libwayland-client0 libwayland-egl1 libegl1 libvulkan1 mesa-vulkan-drivers fontconfig`.
  Build on Ubuntu 24.04 and set `CARGO_PROFILE_RLTO_DEBUG=0` (Warp OOMs with ThinLTO plus
  debuginfo on 16 GB).
- **Proxy**: no settings UI. reqwest uses the system proxy and the
  `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` environment variables; the WebSocket connector
  should honour the same variables (copy the proxy handling from Warp's `crates/websocket`).

---

## 16. Packaging and release

Mirror the Warp fork's portable scheme (`script/windows/build-portable.ps1`,
`package-release.ps1`, `script/portable/*.sh`):

| Platform | Artifact | Contents |
|---|---|---|
| Windows x64 | `matterlite-<tag>-windows-x64.zip` | `matterlite.exe`, needed DLLs (if any), `data/` marker, `LICENSE*` |
| Linux x64 | `matterlite-<tag>-linux-x64.tar.gz` | binary, `data/`, `install.sh` (desktop entry + icon under `$HOME`, checks runtime libs and prints the apt line), `LICENSE*` |
| macOS arm64 | `matterlite-<tag>-macos-arm64.tar.gz` | ad-hoc signed binary with embedded `Info.plist`, `data/`, `LICENSE*` |

- Shipping the `data/` folder makes the archive portable by default; deleting it switches to
  per-user dirs.
- Version from `GIT_RELEASE_TAG` (`v<semver>` or Warp-style date tag) read with
  `option_env!` at build time; the packaging script reads it back from the built binary
  (`matterlite --version`) so archive names can't lie.
- Build scripts verify: version string, PE subsystem 2 on Windows (proves
  `release_bundle`), embedded assets present.
- Package scripts audit the staged folder for user data (no `data/state/*` from test runs).
- CI: `.github/workflows/build-portable.yml` on `ubuntu-24.04` and `macos-latest` for tags;
  Windows built locally (as Warp does) or add a `windows-latest` job.
- Windows SmartScreen will warn on unsigned binaries; code signing (Authenticode) and macOS
  notarization (Developer ID) are optional later steps.

---

## 17. Testing

- **Unit tests** (headless, fast): `mm_model` serde round-trips against JSON fixtures captured
  from a real server; `mm_store` (`apply` events, unread math, sidebar sorting, DM
  auto-close, timeline merging, pending post replacement); `mm_render` (Markdown → MessageDoc
  snapshots with `insta`); `mm_ws` (seq mismatch, resume, new connection id → resync) against
  a mock server (`tokio-tungstenite` server in tests).
- **Integration tests** against a real Mattermost: run the official Docker image (or
  `make run-server` from this repo), create users/teams via `mmctl --local` or the API,
  then drive `mm_api`/`mm_sync` end-to-end (login, MFA, send, receive via WS, reconnect).
  The seeded test server on the Ubuntu VM is described in
  [`MatterLite-TestServer.md`](MatterLite-TestServer.md).
- **UI tests**: WarpUI has an integration-test framework (`crates/integration`, behind the
  `integration_tests` feature) — use selectively; mostly verify UI manually.
- Run `cargo nextest`, `clippy -D warnings`, `fmt --check`, `cargo deny` in CI on all three
  OSes.

---

## 18. Roadmap

| Milestone | Content |
|---|---|
| M0 Skeleton | New repo, Tier A framework crates imported in dependency order (§4.3) and compiling, WarpUI window with list example on Win/Linux/macOS, CI; `ml_themes` extracted (Warp themes + chooser) with `Appearance` wired up; `ml_workspace` shell (tab bar, toolbelt + tool panel, right panel) with placeholder panes and `SearchBarView` extracted; read-only spike of `ml_file_views` |
| M1 Protocol | `mm_model`, `mm_api` (login + MFA, bootstrap endpoints), `mm_ws` (auth, seq, resume, backoff) — CLI test harness prints events |
| M2 Store + cache | `mm_store` with unread math and timelines; diesel cache with hydration; sync engine bootstrap and resync; **eager sync scheduler** P0–P3 + P5 (priority queue, rate limit, resumable `channel_sync`), FTS for messages; **lifecycle** (§8.7): hydrate-then-reconcile on start, persisted WebSocket resume, 1000-post `since` gap handling, outbox with 30 s dedupe rules, crash-safe cursors, logout dialog that keeps data by default, stored-accounts list |
| M3 Core chat UI (**MVP**) | Login; Warp-style layout: chat tabs (focus-or-open, LRU cap, pin, persisted), tool panel **Chats** tab (team switcher + categories), **Channel** tab (Info, Members, Pinned), **History** pane (date groups, local search, navigate, filter-the-chat); channel view with paging, Markdown + emoji + code highlighting, composer (editor) with send/pending/retry, mark read, unread badges, typing indicator, DMs/GMs, channel switcher, reconnect banner, portable packaging; themes: all Warp themes + 5 Mattermost themes + `ChatColors`, theme chooser, follow server theme |
| M4 Threads + reactions | Inline replies, thread in the **right panel** (+ "Open in tab"), **Activity** tab (Threads, Mentions, Saved, Drafts, Scheduled), reactions, edit/delete, edit history in the right panel; History server-side search pass |
| M5 Files + previews | Upload (multipart order!), **eager attachment pipeline** (P4: download on receive/cache, thumbnails, quota/eviction, `ml_extract` text extraction, `file_text_fts`, coverage reporting), file cards and thumbnails, OpenGraph/permalink embeds, message attachments; **Warp viewers** (`ml_file_views`): code editor, Markdown/Jupyter viewer, image lightbox; editing with "send as new version" / "save as"; snippets; edit attachments before sending; **Files explorer** pane (grouped by chat, local index + per-chat server listing, name/content search); History "files" search scope; Channel › Bookmarks; optional **Vim mode** for the code editor and the multi-line composers ([`MatterLite-ComposerVim.md`](MatterLite-ComposerVim.md)) |
| M6 Notifications + search | OS notifications (WarpUI), mention highlighting/keys, tray + dock/taskbar badges, **Search** tab (server-wide messages/files with modifiers), slash commands, interactive buttons/dialogs |
| M7 Auth + multi-server | Personal access tokens, SSO via system browser + custom URL scheme, multiple servers (store already keyed by `ServerId`) |
| M8 Polish | **Split panes** in chat tabs (pane groups), vertical tabs option, tab groups, settings (notify props, display), theme creator, export/upload themes to Mattermost, i18n, drafts sync, LaTeX, accessibility |
| M9 Collaborative drafts (optional) | Restore Warp's collaborative editing in `ml_editor` (remote edits, remote cursors, CRDT tests); `matterlite-collab` server plugin (session store, relay over plugin WebSocket events); `ml_collab` session layer; "Write together" in the composer, shared draft in the right panel, owner-only send — see [`MatterLite-Collaborate.md`](MatterLite-Collaborate.md) |

---

## 19. Risks

- **WarpUI learning curve / no docs.** Mitigation: M0 spike building a scrolling chat list
  with variable-height formatted text items before committing further.
- **Warp's winit fork** (git dependency) may lag upstream winit; pinning keeps builds
  reproducible, but platform fixes must come from the fork.
- **Variable-height virtualized list anchored at bottom** — confirm `viewported_list`
  supports it; otherwise implement a custom element on top of `warpui_core` layout.
- **Editor coupling** — the `editor` crate assumes Warp's `Appearance`, feature flags and
  asset sources; the trimmed `warp_core` must keep those APIs. Budget time for this.
- **Linux secure storage** without a Secret Service → no persisted login; document it.
- **Emoji image licensing** — pick a set whose license allows redistribution (avoid the
  Apple set).
- **Binary size** — wgpu + tree-sitter grammars + emoji PNGs; expect 30–60 MB. Limit
  `arborium` features to common languages.
- **Server compatibility** — target Mattermost server versions supported by the current
  webapp (see `X-Version-Id`); tolerate unknown fields and events.
- **File viewer extraction** (`ml_file_views`) is the largest reuse job: about 31k lines of
  Warp app code tied to LSP, AI, code review and the workspace. Mitigation: the read-only
  spike in M0; cut features rather than port them; the system-app fallback stays available.
  The spike found that `warp_editor` carries most of the work, so the viewers are built on it
  directly ([FileViews.md](FileViews.md)).
- **Keeping themes and viewers in sync with Warp**: app-level code (`themes`, `code`,
  `notebooks`) changes more often than the framework crates. Record the Warp revision and
  re-diff deliberately; don't chase upstream.
- **Workspace extraction**: Warp's `workspace/view.rs` (about 29k lines) mixes the layout
  shell with terminal, AI and Drive logic. Build `ml_workspace` by copying the left/right
  panel, tab bar and pane-group pieces plus `session_history.rs`'s search bar, not by
  trimming `view.rs`.
- **Search semantics differ**: History and Files run regex/case-sensitive matching locally
  only; the server search is word-based and only runs for plain queries. Results from older,
  uncached history therefore depend on the mode. The UI must say which pass produced a
  result.
- **"All files" has no single API**: the Files explorer combines the eagerly built local
  index with a low-priority per-chat `files/search` for files older than the backfill
  window. A complete listing for a large server costs one search per chat.
- **Server load from eager sync**: many channels times deep backfill times attachment
  downloads is far more traffic than the webapp generates. Mitigations: client-side
  token bucket below the server's default limit, 429/`Retry-After` handling, round-robin
  backfill, jitter, resumable state, user-visible pause, and conservative defaults
  (180 days, 100 MB per file). Check with the server admin before setting "All history".
- **Disk usage and data at rest**: attachments and history on local disk (10 GB default
  quota), **kept after logout by default** (the user opts in to deletion, §8.7). Show
  usage, LRU-evict bytes but keep text, offer per-account deletion; encryption at rest
  later.
- **Untrusted file parsing**: text extractors process arbitrary files from chat. Run them
  with input/output size caps, timeouts and `catch_unwind` in a worker pool. Prefer pure-Rust
  parsers; never execute content.
- **Theme field additions**: adding `chat` to `WarpTheme` is a local change to the Warp-derived
  code. Keep it optional so Warp YAML themes load unchanged.

Resolved earlier questions: **no Intel macOS** (arm64 only); **no proxy settings UI** for now
(environment/system proxy only); **Linux = what Warp supports** (X11 + Wayland,
Vulkan/GLES), nothing extra.
