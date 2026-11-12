# Provenance of Warp-derived code

Reference: `C:\Projects\Libs\warp-terminal` @ `5bb7bcc6a` (2026-11-06)

One row per imported unit: a whole crate (Tier A) or a file/module (Tier B and C).
Status: `verbatim` (unchanged since import), `adapted` (modified), `rewritten` (only the ideas
remain), `snippet` (a few functions copied into new code). See AGENTS.md for the procedure.

| MatterLite path | Warp source | Commit | Status | License | Notes |
|---|---|---|---|---|---|
| crates/warp_errors/ | crates/warp_errors/ | 5bb7bcc6a | adapted | AGPL-3.0 | removed Sentry reporting and websocket classification; errors are logged only |
| crates/string-offset/ | crates/string-offset/ | 5bb7bcc6a | verbatim | AGPL-3.0 | |
| crates/sum_tree/ | crates/sum_tree/ | 5bb7bcc6a | verbatim | AGPL-3.0 | |
| crates/markdown_parser/ | crates/markdown_parser/ | 5bb7bcc6a | verbatim | AGPL-3.0 | |
| crates/settings_value/ | crates/settings_value/ | 5bb7bcc6a | adapted | AGPL-3.0 | derive tests enable the `derive` feature through a dev-dependency |
| crates/settings_value_derive/ | crates/settings_value_derive/ | 5bb7bcc6a | verbatim | AGPL-3.0 | |
| crates/command/ | crates/command/ | 5bb7bcc6a | verbatim | AGPL-3.0 | |
| crates/warp_util/ | crates/warp_util/ | 5bb7bcc6a | adapted | AGPL-3.0 | Tier C: kept content_version, file, file_type, path, standardized_path, user_input, windows; removed git, worktree, remote-host, sync and on_cancel helpers |
| crates/warpui_core/ | crates/warpui_core/ | 5bb7bcc6a | adapted | MIT | removed the TUI front-end (`tui` feature, elements/tui, runtime), wasm deps; Roboto path points at assets/; test GIF moved to test_data/ |
| crates/warpui/ | crates/warpui/ | 5bb7bcc6a | adapted | MIT | removed the wasm platform and wasm deps; only the list and scrolling examples; font tests read assets/bundled/fonts |
| crates/virtual_fs/ | crates/virtual_fs/ | 5bb7bcc6a | adapted | AGPL-3.0 | test helper for warpui; one clippy fix |
| crates/asset_cache/ | crates/asset_cache/ | 5bb7bcc6a | adapted | AGPL-3.0 | wasm branches removed |
| crates/asset_macro/ | crates/asset_macro/ | 5bb7bcc6a | adapted | AGPL-3.0 | assets resolved from the repository assets/ dir; remote_asset! removed; bundled_or_fetched_asset! always bundles; warp_util::assets constants inlined |
| crates/warpui_extras/ | crates/warpui_extras/ | 5bb7bcc6a | adapted | AGPL-3.0 | removed registry, UserDefaults and localStorage preference stores; TOML and JSON-file stores always built; wasm branch removed |
| crates/warp_features/ | crates/warp_features/ | 5bb7bcc6a | verbatim | AGPL-3.0 | Warp flag list still complete; pruned to MatterLite flags at the end of M0 |
| crates/settings/ | crates/settings/ | 5bb7bcc6a | verbatim | AGPL-3.0 | |
| crates/warp_core/ | crates/warp_core/ | 5bb7bcc6a | adapted | AGPL-3.0 | Tier B: kept app_id, assertions, async, features, platform, safe_log, ui (appearance, theme, icons, color, builder), user_preferences; removed channels, context flags, telemetry, execution mode, sync queue, semantic selection and the other Warp product modules; safe_log full output in debug builds |
| crates/warp_core/src/paths.rs | crates/warp_core/src/paths.rs | 5bb7bcc6a | rewritten | AGPL-3.0 | MatterLite storage layout (§9); portable data/ detection kept |
| crates/ui_components/ | crates/ui_components/ | 5bb7bcc6a | adapted | AGPL-3.0 | library example embeds the repository assets/ |
| crates/vim/ | crates/vim/ | 5bb7bcc6a | verbatim | AGPL-3.0 | |
| crates/ipynb_parser/ | crates/ipynb_parser/ | 5bb7bcc6a | verbatim | AGPL-3.0 | needed by warp_editor (notebook rendering) |
| crates/warp_editor/ | crates/editor/ | 5bb7bcc6a | adapted | AGPL-3.0 | debug check uses cfg!(debug_assertions) instead of Warp channels; clippy 1.98 fixes |
| crates/languages/ | crates/languages/ | 5bb7bcc6a | adapted | AGPL-3.0 | warp_editor dependency through the workspace |
| crates/syntax_tree/ | crates/syntax_tree/ | 5bb7bcc6a | verbatim | AGPL-3.0 | |
| assets/bundled/svg/ | app/assets/bundled/svg/ | 5bb7bcc6a | verbatim | AGPL-3.0 | only the icons referenced by imported code (341 of 383) |
| assets/bundled/fonts/roboto/ | app/assets/bundled/fonts/roboto/ | 5bb7bcc6a | verbatim | Apache-2.0 | font files; LICENSE.txt included |

Rows with status `verbatim` differ from the reference only by the provenance header.
